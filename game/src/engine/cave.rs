//! [`Cave`] — one cave's full simulation state and the per-frame tick driver.
//!
//! Mechanics follow the Ghidra-confirmed NES behavior; the function/variable
//! addresses cited in comments reference the findings doc.

use crate::data::cave_params::{CaveParam, CAVE_PARAMS};
use crate::data::caves::{CAVES, CAVE_COUNT};

use super::cell::{Cell, Obj};
use super::event::{DeathCause, Event, SoundCue};
use super::field::Field;
use super::input::{Direction, Input};
use super::{
    AMOEBA_CHUNKS_PER_PASS, AMOEBA_INTERVAL_START, AMOEBA_INTERVAL_STEP,
    AMOEBA_PROBES_PER_CHUNK, CELLS, CYCLE_FRAMES, DEATH_ARC_FRAMES,
    EXPLOSION_LINGER_FRAMES, EXPLOSION_SCORE, EXTRA_LIFE_EVERY, FRAMES_PER_TIME_UNIT,
    HEIGHT, HURRY_UP_UNITS, MAGIC_WALL_FRAMES, MAX_LIVES, PENDING_DIAMOND_FRAMES,
    PENDING_SLOTS, PUSH_HOLD_FRAMES, ROCKFORD_MOVE_FRAMES, START_RESERVE_LIVES,
    SUICIDE_HOLD_FRAMES, WIDTH,
};

/// Lifecycle state of a cave.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaveStatus {
    Playing,
    /// Reserve lives exhausted (the manual's "4 Rockfords" are spent).
    GameOver,
    /// Rockford entered the open exit door.
    Complete,
}

/// Deterministic amoeba growth machine ($CE57): a spiral edge-walk from the
/// seed cell, 25 probes per 8-frame chunk, cursor persisting across chunks;
/// growth is paced by the countdown/interval pair ($BD/$BA), not by RNG.
#[derive(Clone, Copy)]
struct Amoeba {
    /// $B7 != 0/$FF: there was a live amoeba at cave load.
    active: bool,
    /// A full 4-chunk pass found no empty cell: cleanup converts all amoeba
    /// cells to diamonds.
    enclosed: bool,
    /// "Found empty space this pass" (bit7 of $B7).
    found_empty: bool,
    /// Chunk counter within the current pass (0..4).
    phase: u8,
    /// Spiral cursor ($BB/$BC), persists across chunks within a pass.
    cursor: usize,
    /// Spiral direction state ($4E).
    dir: Direction,
    /// Seed cell ($B8/$B9): first amoeba found by the bottom-up load scan.
    seed: usize,
    /// Growth pacing ($BA): interval between growths, shrinks by 4 per growth.
    interval: u8,
    /// Growth pacing ($BD): counts down on each empty probe.
    countdown: u8,
}

/// One cave attempt: field, timers, Rockford, score/lives. Fully
/// deterministic — the NES ROM contains no gameplay RNG.
pub struct Cave {
    field: Field,
    /// Raw object ids as loaded, for the cave reload after death.
    initial: Vec<u8>,
    params: CaveParam,
    cave_index: usize,
    level: usize,
    /// Spawn cell from the cave record (b0/b1), before an explicit Rockford
    /// cell override (test maps).
    params_spawn: Option<usize>,
    spawn: Option<usize>,
    /// Gameplay frame counter ($FE).
    frame: u64,
    status: CaveStatus,
    /// Cave timer in 64-frame units ($B1-$B3 BCD in the original).
    time_units: u32,
    /// Magic wall state ($B5): active window running / expired.
    magic_active: bool,
    magic_expired: bool,
    /// Frames left in the active window ($B4: 256 decrements x 16 frames).
    magic_frames_left: u32,
    /// Explosion remnant linger timer ($B6).
    linger: u8,
    /// Rotating pending-diamond timers ($07F8-$07FF) and slot counter ($FD).
    slot_timers: [u8; PENDING_SLOTS],
    slot_counter: u8,
    amoeba: Amoeba,
    rockford: usize,
    rockford_alive: bool,
    /// Death arc frames remaining ($92=$50 -> 80).
    dying: u8,
    /// Frames until Rockford may start another cell move.
    move_cooldown: u8,
    /// Boulder push in progress: direction held + frames left ($92=24).
    push_dir: Option<Direction>,
    push_timer: u8,
    /// Frames the suicide combo (both buttons) has been held ($9D >= 64).
    suicide_held: u8,
    /// Exit-open flag ($90).
    door_open: bool,
    door_pos: Option<usize>,
    diamonds_collected: u32,
    score: u32,
    /// Reserve lives ($77); the active Rockford is extra ("4 Rockfords" =
    /// 1 active + 3 reserve).
    lives: u8,
    next_extra_life: u32,
}

impl Cave {
    /// Load cave `cave` (0..24) from the extracted ROM data at difficulty
    /// `level` (1..=4). `seed` is reserved (unused): the engine is fully
    /// deterministic.
    /// Load cave `cave` (0..24) at difficulty `level` (1..=4). The map comes
    /// from [`cave_ids`] (editable emoji files, ROM grid as fallback).
    pub fn new(cave: usize, level: u8, seed: u64) -> Self {
        assert!(cave < CAVE_COUNT, "cave index out of range");
        Self::from_cells(&cave_ids(cave), CAVE_PARAMS[cave], cave, level, seed)
    }

    /// Build a cave from raw object ids (as in [`CAVES`]) and parameters.
    /// Rockford spawns at the record cell (`spawn_row`/`spawn_col`), unless
    /// the data already contains an explicit Rockford cell (test maps).
    pub fn from_cells(
        ids: &[u8; CELLS],
        params: CaveParam,
        cave_index: usize,
        level: u8,
        seed: u64,
    ) -> Self {
        let _ = seed;
        let spawn = Some(params.spawn_row as usize * WIDTH + params.spawn_col as usize);
        Self::build(ids, params, cave_index, level, spawn)
    }

    /// Test path: no param spawn; Rockford only if the data contains an
    /// explicit Rockford cell (ascii maps with `r`).
    pub(crate) fn from_cells_unspawned(
        ids: &[u8; CELLS],
        params: CaveParam,
        cave_index: usize,
        level: u8,
    ) -> Self {
        Self::build(ids, params, cave_index, level, None)
    }

    fn build(
        ids: &[u8; CELLS],
        params: CaveParam,
        cave_index: usize,
        level: u8,
        params_spawn: Option<usize>,
    ) -> Self {
        let level = (level.max(1) - 1).min(3) as usize;
        let (field, door_pos, spawn) = build_field(ids, params_spawn);
        let mut cave = Cave {
            field,
            initial: ids.to_vec(),
            params,
            cave_index,
            level,
            params_spawn,
            spawn,
            frame: 0,
            status: CaveStatus::Playing,
            time_units: params.cave_time[level] as u32,
            magic_active: false,
            magic_expired: false,
            magic_frames_left: 0,
            linger: 0,
            slot_timers: [0; PENDING_SLOTS],
            slot_counter: 0,
            amoeba: Amoeba {
                active: false,
                enclosed: false,
                found_empty: false,
                phase: 0,
                cursor: 0,
                dir: Direction::Up,
                seed: 0,
                interval: AMOEBA_INTERVAL_START,
                countdown: AMOEBA_INTERVAL_START,
            },
            rockford: spawn.unwrap_or(0),
            rockford_alive: spawn.is_some(),
            dying: 0,
            move_cooldown: 0,
            push_dir: None,
            push_timer: 0,
            suicide_held: 0,
            door_open: false,
            door_pos,
            diamonds_collected: 0,
            score: 0,
            lives: START_RESERVE_LIVES,
            next_extra_life: EXTRA_LIFE_EVERY,
        };
        cave.amoeba_init();
        cave
    }

    /// Bottom-up load scan for the first amoeba cell ($B8B4).
    fn amoeba_init(&mut self) {
        for y in (0..HEIGHT).rev() {
            for x in 0..WIDTH {
                let i = Field::idx(x, y);
                if self.field.cells[i].obj == Obj::Amoeba {
                    self.amoeba.active = true;
                    self.amoeba.enclosed = false;
                    self.amoeba.found_empty = false;
                    self.amoeba.phase = 0;
                    self.amoeba.cursor = i;
                    self.amoeba.seed = i;
                    self.amoeba.dir = Direction::Up;
                    self.amoeba.interval = AMOEBA_INTERVAL_START;
                    self.amoeba.countdown = AMOEBA_INTERVAL_START;
                    return;
                }
            }
        }
        self.amoeba.active = false;
    }

    // ---- read-only accessors ------------------------------------------------

    pub fn status(&self) -> CaveStatus {
        self.status
    }

    pub fn cell_at_idx(&self, i: usize) -> Cell {
        self.field.cells[i]
    }

    pub fn cell_at(&self, x: usize, y: usize) -> Cell {
        self.field.cells[Field::idx(x, y)]
    }

    pub fn field(&self) -> &Field {
        &self.field
    }

    pub fn count_obj(&self, obj: Obj) -> usize {
        self.field.count_obj(obj)
    }

    pub fn score(&self) -> u32 {
        self.score
    }

    /// Reserve lives remaining ($77).
    pub fn lives(&self) -> u8 {
        self.lives
    }

    pub fn diamonds_collected(&self) -> u32 {
        self.diamonds_collected
    }

    pub fn diamonds_needed(&self) -> u32 {
        self.params.diamonds_needed[self.level] as u32
    }

    /// Exit-open flag ($90); the door cell keeps id `Door`.
    pub fn door_open(&self) -> bool {
        self.door_open
    }

    pub fn door_pos(&self) -> Option<usize> {
        self.door_pos
    }

    pub fn rockford_pos(&self) -> usize {
        self.rockford
    }

    pub fn rockford_alive(&self) -> bool {
        self.rockford_alive
    }

    /// Gameplay frame counter ($FE).
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Cave time left in 64-frame units.
    pub fn time_units_remaining(&self) -> u32 {
        self.time_units
    }

    /// Approximate seconds remaining (~1.07 s per unit at 60 Hz).
    pub fn seconds_remaining(&self) -> u32 {
        self.time_units * (FRAMES_PER_TIME_UNIT as u32) / super::FRAMES_PER_SECOND
    }

    /// Current amoeba growth probe interval ($BA); shrinks by 4 per growth.
    pub fn amoeba_probe_interval(&self) -> u8 {
        self.amoeba.interval
    }

    /// FNV-1a hash over the full simulation state. Equal hashes <=> identical
    /// visible state (used by determinism tests; there is no hidden RNG).
    pub fn hash(&self) -> u64 {
        fn mix(h: &mut u64, b: u8) {
            *h = (*h ^ b as u64).wrapping_mul(0x100000001b3);
        }
        let mut h = 0xcbf29ce484222325u64;
        for c in &self.field.cells {
            mix(&mut h, c.obj as u8);
            mix(&mut h, c.flags);
        }
        for b in self.score.to_le_bytes() {
            mix(&mut h, b);
        }
        for b in self.time_units.to_le_bytes() {
            mix(&mut h, b);
        }
        for b in (self.rockford as u32).to_le_bytes() {
            mix(&mut h, b);
        }
        for b in self.slot_timers {
            mix(&mut h, b);
        }
        mix(&mut h, self.magic_frames_left as u8);
        mix(&mut h, self.magic_active as u8);
        mix(&mut h, self.magic_expired as u8);
        mix(&mut h, self.linger);
        mix(&mut h, self.amoeba.active as u8);
        mix(&mut h, self.amoeba.enclosed as u8);
        mix(&mut h, self.amoeba.interval);
        mix(&mut h, self.amoeba.countdown);
        mix(&mut h, self.diamonds_collected as u8);
        mix(&mut h, self.lives);
        mix(&mut h, self.status as u8);
        mix(&mut h, self.rockford_alive as u8);
        mix(&mut h, self.dying);
        mix(&mut h, self.door_open as u8);
        h
    }

    // ---- frame driver ---------------------------------------------------------

    /// Advance the simulation one NMI frame. Returns the events produced.
    pub fn tick(&mut self, input: Input) -> Vec<Event> {
        let mut ev = Vec::new();
        if self.status != CaveStatus::Playing {
            return ev;
        }
        self.frame += 1;
        let f = self.frame;

        // Cave timer: once per 64 frames ($CFCC).
        if f % FRAMES_PER_TIME_UNIT == 0 && self.time_units > 0 {
            self.time_units -= 1;
            if self.time_units == HURRY_UP_UNITS {
                ev.push(Event::Sound(SoundCue::HurryUp));
            }
            if self.time_units == 0 {
                self.kill_rockford(DeathCause::Timeout, &mut ev);
            }
        }

        // Magic wall window ($CDB4; $B4 wraps 0 -> 255 on first decrement,
        // i.e. a fixed 256 x 16 = 4096-frame window).
        if self.magic_active && !self.magic_expired {
            self.magic_frames_left -= 1;
            if self.magic_frames_left == 0 {
                self.magic_expired = true;
                ev.push(Event::MagicWallExpired);
            }
        }

        // Rotating pending-diamond timers ($07F8), decremented every frame.
        for t in &mut self.slot_timers {
            if *t > 0 {
                *t -= 1;
            }
        }
        // Explosion linger timer ($B6).
        if self.linger > 0 {
            self.linger -= 1;
        }

        // Every-8th-frame systems.
        if f % CYCLE_FRAMES == 0 {
            self.amoeba_chunk(&mut ev);
            if self.rockford_alive && self.dying == 0 {
                self.danger_check(&mut ev);
            }
        }

        // Player.
        if self.dying > 0 {
            self.dying -= 1;
            if self.dying == 0 {
                self.finish_death(&mut ev);
            }
        } else if self.rockford_alive {
            self.step_rockford(input, &mut ev);
        }

        // Phased field scan ($C419: frame & 7).
        match f % CYCLE_FRAMES {
            1 => self.fall_scan(16, 20, &mut ev),
            2 => self.fall_scan(11, 15, &mut ev),
            3 => self.fall_scan(6, 10, &mut ev),
            4 => self.fall_scan(1, 5, &mut ev),
            5 => self.cleanup_scan(1, 7, &mut ev),
            6 => self.cleanup_scan(8, 14, &mut ev),
            _ => self.cleanup_scan(15, 20, &mut ev),
        }

        ev
    }

    // ---- fall scan (boulders, diamonds, enemies), bottom-up, right-to-left ----

    fn fall_scan(&mut self, y_lo: usize, y_hi: usize, ev: &mut Vec<Event>) {
        for y in (y_lo..=y_hi).rev() {
            for x in (0..WIDTH).rev() {
                let i = Field::idx(x, y);
                let cell = self.field.cells[i];
                if cell.moved() {
                    continue;
                }
                match cell.obj {
                    Obj::Boulder | Obj::Diamond => self.step_fall(i, ev),
                    Obj::Firefly => self.step_enemy(i, false, ev),
                    Obj::Butterfly => self.step_enemy(i, true, ev),
                    _ => {}
                }
            }
        }
    }

    fn step_fall(&mut self, i: usize, ev: &mut Vec<Event>) {
        let cell = self.field.cells[i];
        let obj = cell.obj;
        let falling = cell.falling();
        let Some(below) = self.field.neighbor(i, Direction::Down) else {
            return;
        };
        match self.field.cells[below].obj {
            Obj::Space => {
                // Fall one cell; the vacated cell becomes $10, not space, so
                // nothing else can enter it this cycle.
                let mut c = Cell::new(obj);
                c.set_falling(true);
                self.field.cells[below] = c;
                self.field.cells[i] = Cell::new(Obj::Vacated);
                if !falling {
                    ev.push(Event::Sound(SoundCue::FallStart));
                }
            }
            Obj::Rockford => {
                // Only a FALLING object kills; a resting one is harmless
                // (bit0 gate at $C4EA).
                if falling {
                    self.kill_rockford(DeathCause::Crushed, ev);
                }
            }
            Obj::MagicWall => {
                if obj == Obj::Boulder && falling && !self.magic_expired {
                    if !self.magic_active {
                        self.magic_active = true;
                        self.magic_frames_left = MAGIC_WALL_FRAMES;
                        ev.push(Event::MagicWallActivated);
                        ev.push(Event::Sound(SoundCue::MagicWall));
                    }
                    // Convert: boulder -> falling diamond two rows down if
                    // that cell is empty ($C4A2); otherwise rest on the wall.
                    let out = self
                        .field
                        .neighbor(below, Direction::Down)
                        .filter(|&o| self.field.cells[o].obj == Obj::Space);
                    if let Some(o) = out {
                        self.field.cells[i] = Cell::new(Obj::Vacated);
                        let mut c = Cell::new(Obj::Diamond);
                        c.set_falling(true);
                        self.field.cells[o] = c;
                        ev.push(Event::Sound(SoundCue::MagicWall));
                    } else {
                        self.land(i, ev);
                    }
                } else {
                    // Magic walls are in the roll-off set ($C514).
                    self.roll_off(i, ev);
                }
            }
            o if o.is_rounded() => self.roll_off(i, ev),
            _ => self.land(i, ev),
        }
    }

    fn land(&mut self, i: usize, ev: &mut Vec<Event>) {
        let cell = self.field.cells[i];
        if cell.falling() {
            // Landing clears the low nibble; boulders (not diamonds) thud.
            self.field.cells[i] = Cell::new(cell.obj);
            if cell.obj == Obj::Boulder {
                ev.push(Event::Sound(SoundCue::Thud));
            }
        }
    }

    /// Roll off a rounded supporting object ($C514): deterministic LEFT-first.
    /// Requires the side cell and the cell diagonally below it empty, and the
    /// cell diagonally ABOVE not a boulder/diamond.
    fn roll_off(&mut self, i: usize, ev: &mut Vec<Event>) {
        let obj = self.field.cells[i].obj;
        for dir in [Direction::Left, Direction::Right] {
            let side = self.field.neighbor(i, dir);
            let side_below = side.and_then(|s| self.field.neighbor(s, Direction::Down));
            let side_above = side.and_then(|s| self.field.neighbor(s, Direction::Up));
            let ok = matches!(side, Some(s) if self.field.cells[s].obj == Obj::Space)
                && matches!(side_below, Some(s) if self.field.cells[s].obj == Obj::Space)
                && !matches!(side_above, Some(s)
                    if matches!(self.field.cells[s].obj, Obj::Boulder | Obj::Diamond));
            if ok {
                let s = side.unwrap();
                let mut c = Cell::new(obj);
                c.set_falling(true);
                c.set_moved(true);
                self.field.cells[s] = c;
                self.field.cells[i] = Cell::new(Obj::Vacated);
                return;
            }
        }
        self.land(i, ev);
    }

    // ---- enemies ----------------------------------------------------------------

    /// Firefly: left-turn-first (CCW) wall follower ($C638); amoeba is an
    /// ordinary wall to it. Butterfly: exact CW mirror ($C6D3), and explodes
    /// when a movement probe hits amoeba ($C761). Both start facing up.
    /// A boulder/diamond in ANY fall state directly above explodes the enemy.
    fn step_enemy(&mut self, i: usize, butterfly: bool, ev: &mut Vec<Event>) {
        if let Some(above) = self.field.neighbor(i, Direction::Up) {
            let o = self.field.cells[above].obj;
            if o == Obj::Boulder || o == Obj::Diamond {
                if butterfly {
                    self.explode(i, true, None, ev);
                } else {
                    // Firefly blast pays by cave group ($C944/$C99A).
                    let pts = EXPLOSION_SCORE[(self.cave_index >> 2).min(5)];
                    self.add_score(pts, ev);
                    self.explode(i, false, None, ev);
                }
                return;
            }
        }

        let dir = self.field.cells[i].dir();
        let (preferred, fallback) = if butterfly {
            (dir.cw(), dir.ccw())
        } else {
            (dir.ccw(), dir.cw())
        };
        for d in [preferred, dir] {
            if let Some(n) = self.field.neighbor(i, d) {
                let o = self.field.cells[n].obj;
                if butterfly && o == Obj::Amoeba {
                    self.explode(i, true, None, ev);
                    return;
                }
                if o == Obj::Space {
                    let mut c = Cell::new(self.field.cells[i].obj);
                    c.set_dir(d);
                    c.set_moved(true);
                    self.field.cells[n] = c;
                    self.field.cells[i] = Cell::new(Obj::Vacated);
                    return;
                }
            }
        }
        // Blocked both ways: turn the other way in place.
        self.field.cells[i].set_dir(fallback);
    }

    // ---- cleanup scan (top-down) -------------------------------------------------

    fn cleanup_scan(&mut self, y_lo: usize, y_hi: usize, ev: &mut Vec<Event>) {
        let _ = ev;
        for y in y_lo..=y_hi {
            for x in 0..WIDTH {
                let i = Field::idx(x, y);
                let cell = self.field.cells[i];
                // Clear the moved-this-cycle bit ($C9FE/$CA34).
                self.field.cells[i].set_moved(false);
                match cell.obj {
                    // Vacated-this-cycle marker -> real space.
                    Obj::Vacated => self.field.cells[i] = Cell::new(Obj::Space),
                    // Pending diamond ripens when its slot timer hits 0.
                    Obj::PendingDiamond => {
                        if self.slot_timers[cell.slot() as usize] == 0 {
                            self.field.cells[i] = Cell::new(Obj::Diamond);
                        }
                    }
                    // Explosion remnant clears once the linger timer hits 0
                    // (via Vacated, so it takes another cleanup visit).
                    Obj::ExplosionRemnant => {
                        if self.linger == 0 {
                            self.field.cells[i] = Cell::new(Obj::Vacated);
                        }
                    }
                    // Enclosed amoeba -> diamonds ($CA3B).
                    Obj::Amoeba => {
                        if self.amoeba.enclosed {
                            self.field.cells[i] = Cell::new(Obj::Diamond);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // ---- amoeba -----------------------------------------------------------------

    /// One 25-probe spiral chunk ($CE57), run once per 8 frames. Growth is
    /// deterministic: every `countdown`-th empty probe grows one cell, then
    /// the interval shrinks by 4 (accelerates). A full 4-chunk pass with no
    /// empty probe marks the amoeba enclosed -> cleanup turns it to diamonds.
    /// (No overgrowth-to-boulders threshold exists in the NES ROM.)
    fn amoeba_chunk(&mut self, ev: &mut Vec<Event>) {
        if !self.amoeba.active || self.amoeba.enclosed {
            return;
        }
        for _ in 0..AMOEBA_PROBES_PER_CHUNK {
            let ahead = self.field.neighbor(self.amoeba.cursor, self.amoeba.dir);
            let Some(ahead) = ahead else {
                // Off-field: step back, try next direction.
                self.amoeba.dir = self.amoeba.dir.ccw();
                continue;
            };
            match self.field.cells[ahead].obj {
                Obj::Amoeba => {
                    // Move onto the amoeba cell and turn CW.
                    self.amoeba.cursor = ahead;
                    self.amoeba.dir = self.amoeba.dir.cw();
                }
                Obj::Space => {
                    self.amoeba.found_empty = true;
                    self.amoeba.countdown = self.amoeba.countdown.wrapping_sub(1);
                    if self.amoeba.countdown == 0 {
                        self.field.cells[ahead] = Cell::new(Obj::Amoeba);
                        self.amoeba.interval =
                            self.amoeba.interval.wrapping_sub(AMOEBA_INTERVAL_STEP);
                        self.amoeba.countdown = self.amoeba.interval;
                        ev.push(Event::AmoebaGrew { at: ahead });
                        // At most one new cell per chunk.
                        break;
                    }
                    // Step back, try next direction.
                    self.amoeba.dir = self.amoeba.dir.ccw();
                }
                _ => {
                    // Obstacle: step back, try next direction.
                    self.amoeba.dir = self.amoeba.dir.ccw();
                }
            }
        }
        self.amoeba.phase += 1;
        if self.amoeba.phase == AMOEBA_CHUNKS_PER_PASS {
            self.amoeba.phase = 0;
            if self.amoeba.found_empty {
                self.amoeba.found_empty = false;
                // Restart the pass from the seed cell.
                self.amoeba.cursor = self.amoeba.seed;
                self.amoeba.dir = Direction::Up;
            } else {
                self.amoeba.enclosed = true;
                ev.push(Event::AmoebaConverted { to_diamonds: true });
                ev.push(Event::Sound(SoundCue::Amoeba));
            }
        }
    }

    // ---- Rockford -----------------------------------------------------------------

    fn step_rockford(&mut self, input: Input, ev: &mut Vec<Event>) {
        // Suicide combo: both buttons held for 64 frames ($9D >= $40).
        if input.suicide {
            self.suicide_held += 1;
            if self.suicide_held >= SUICIDE_HOLD_FRAMES {
                self.kill_rockford(DeathCause::Suicide, ev);
                return;
            }
        } else {
            self.suicide_held = 0;
        }

        // Boulder push in progress ($C278): the direction must stay held and
        // the destination (two cells over) must be empty when the timer ends.
        if let Some(dir) = self.push_dir {
            if input.direction() != Some(dir) {
                self.push_dir = None; // released: abort
            } else {
                self.push_timer -= 1;
                if self.push_timer == 0 {
                    let rock = self.field.neighbor(self.rockford, dir);
                    let beyond = rock.and_then(|r| self.field.neighbor(r, dir));
                    if matches!(rock, Some(r) if self.field.cells[r].obj != Obj::Boulder) {
                        // The boulder moved (fell) during the hold: abort.
                        self.push_dir = None;
                    } else if matches!(beyond, Some(b) if self.field.cells[b].obj == Obj::Space)
                        && rock.is_some()
                    {
                        let (r, b) = (rock.unwrap(), beyond.unwrap());
                        self.field.cells[b] = Cell::new(Obj::Boulder);
                        self.move_rockford(r, dir);
                        ev.push(Event::Sound(SoundCue::Push));
                        self.push_dir = None;
                    } else {
                        // Blocked at expiry: re-arm and keep holding.
                        self.push_timer = PUSH_HOLD_FRAMES;
                    }
                }
                return;
            }
        }

        if self.move_cooldown > 0 {
            self.move_cooldown -= 1;
            if self.move_cooldown > 0 {
                return;
            }
        }

        let Some(dir) = input.direction() else {
            return;
        };
        let Some(target) = self.field.neighbor(self.rockford, dir) else {
            return;
        };
        let tobj = self.field.cells[target].obj;

        if input.grab {
            // Snap actions ($C0AE): act on the adjacent cell, never move.
            match tobj {
                Obj::Mud => {
                    self.field.cells[target] = Cell::new(Obj::Space);
                    ev.push(Event::Sound(SoundCue::Dig));
                }
                Obj::Diamond => {
                    self.collect_diamond(ev);
                    self.field.cells[target] = Cell::new(Obj::Space);
                }
                Obj::Boulder if dir.is_horizontal() => {
                    let beyond = self.field.neighbor(target, dir);
                    if matches!(beyond, Some(b) if self.field.cells[b].obj == Obj::Space) {
                        let b = beyond.unwrap();
                        self.field.cells[b] = Cell::new(Obj::Boulder);
                        self.field.cells[target] = Cell::new(Obj::Vacated);
                        ev.push(Event::Sound(SoundCue::Push));
                    }
                }
                _ => {}
            }
            return;
        }

        match tobj {
            // Walk accept list ($C19E): space, explosion remnant, vacated.
            Obj::Space | Obj::Vacated | Obj::ExplosionRemnant => {
                self.move_rockford(target, dir);
            }
            Obj::Mud => {
                self.move_rockford(target, dir);
                ev.push(Event::Sound(SoundCue::Dig));
            }
            Obj::Diamond => {
                self.collect_diamond(ev);
                self.move_rockford(target, dir);
            }
            Obj::Door if self.door_open => {
                self.move_rockford(target, dir);
                // Time bonus: +1 per remaining time unit ($AD50).
                let bonus = self.time_units;
                self.add_score(bonus, ev);
                self.status = CaveStatus::Complete;
                ev.push(Event::CaveComplete { time_bonus: bonus });
            }
            Obj::Boulder if dir.is_horizontal() => {
                // Walk into a boulder: enter the push state ($92 = 24).
                self.push_dir = Some(dir);
                self.push_timer = PUSH_HOLD_FRAMES;
            }
            _ => {}
        }
    }

    fn move_rockford(&mut self, to: usize, dir: Direction) {
        let mut c = Cell::new(Obj::Rockford);
        c.set_dir(dir);
        self.field.cells[to] = c;
        self.field.cells[self.rockford] = Cell::new(Obj::Vacated);
        self.rockford = to;
        self.move_cooldown = ROCKFORD_MOVE_FRAMES;
    }

    // ---- shared ------------------------------------------------------------------

    fn collect_diamond(&mut self, ev: &mut Vec<Event>) {
        self.diamonds_collected += 1;
        let points = if self.door_open {
            self.params.diamond_points_bonus
        } else {
            self.params.diamond_points
        };
        self.add_score(points as u32, ev);
        ev.push(Event::DiamondCollected {
            count: self.diamonds_collected,
        });
        ev.push(Event::Sound(SoundCue::Diamond));
        if !self.door_open && self.diamonds_collected >= self.diamonds_needed() {
            self.door_open = true;
            ev.push(Event::DoorOpened { pos: self.door_pos });
            ev.push(Event::Sound(SoundCue::DoorOpen));
        }
    }

    /// $CCE8: add points; extra life every 2000 (threshold += 2000 per award),
    /// reserve lives clamped at 9.
    fn add_score(&mut self, points: u32, ev: &mut Vec<Event>) {
        self.score += points;
        while self.score >= self.next_extra_life {
            self.next_extra_life += EXTRA_LIFE_EVERY;
            self.lives = (self.lives + 1).min(MAX_LIVES);
            ev.push(Event::ExtraLife { lives: self.lives });
            ev.push(Event::Sound(SoundCue::ExtraLife));
        }
    }

    /// 3x3 blast centered on `center` ($C841). Steel walls and the door are
    /// immune ($C932); every Rockford cell inside kills him. `to_diamond`
    /// selects pending diamonds (butterfly, $C7EA) over remnants ($C7D3).
    fn explode(
        &mut self,
        center: usize,
        to_diamond: bool,
        cause: Option<DeathCause>,
        ev: &mut Vec<Event>,
    ) {
        let (cx, cy) = Field::xy(center);
        let mut killed = false;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
                    continue;
                }
                let c = Field::idx(x as usize, y as usize);
                match self.field.cells[c].obj {
                    Obj::Steel | Obj::Door => continue,
                    Obj::Rockford => {
                        if self.rockford_alive && self.dying == 0 {
                            killed = true;
                        }
                    }
                    _ => {}
                }
                let cell = if to_diamond {
                    let slot = self.slot_counter % PENDING_SLOTS as u8;
                    self.slot_counter = self.slot_counter.wrapping_add(1);
                    self.slot_timers[slot as usize] = PENDING_DIAMOND_FRAMES;
                    let mut cc = Cell::new(Obj::PendingDiamond);
                    cc.set_slot(slot);
                    cc
                } else {
                    Cell::new(Obj::ExplosionRemnant)
                };
                self.field.cells[c] = cell;
            }
        }
        if !to_diamond {
            self.linger = EXPLOSION_LINGER_FRAMES;
        }
        ev.push(Event::Explosion { center, to_diamond });
        ev.push(Event::Sound(SoundCue::Explosion));
        if killed {
            self.kill_rockford(cause.unwrap_or(DeathCause::Explosion), ev);
        }
    }

    /// $C3CF: explosion centered on Rockford, then the 80-frame death arc.
    fn kill_rockford(&mut self, cause: DeathCause, ev: &mut Vec<Event>) {
        if !self.rockford_alive || self.dying > 0 {
            return;
        }
        self.rockford_alive = false;
        self.dying = DEATH_ARC_FRAMES;
        self.push_dir = None;
        ev.push(Event::RockfordDied { cause });
        let pos = self.rockford;
        self.explode(pos, false, None, ev);
    }

    /// $AE05: spend a reserve life and reload the cave, or game over when the
    /// decrement would go negative.
    fn finish_death(&mut self, ev: &mut Vec<Event>) {
        if self.lives == 0 {
            self.status = CaveStatus::GameOver;
            return;
        }
        self.lives -= 1;
        self.respawn();
        ev.push(Event::Respawned);
    }

    /// Cave reload after death (state 2/5): field re-read, Rockford respawns
    /// at the record spawn cell, timers reset; score/lives carry over.
    fn respawn(&mut self) {
        let (field, door_pos, spawn) = build_field(&self.initial, self.params_spawn);
        self.field = field;
        self.door_pos = door_pos;
        self.spawn = spawn;
        self.rockford = spawn.unwrap_or(0);
        self.rockford_alive = spawn.is_some();
        self.dying = 0;
        self.move_cooldown = 0;
        self.push_dir = None;
        self.suicide_held = 0;
        self.time_units = self.params.cave_time[self.level] as u32;
        self.magic_active = false;
        self.magic_expired = false;
        self.magic_frames_left = 0;
        self.linger = 0;
        self.slot_timers = [0; PENDING_SLOTS];
        self.door_open = false;
        self.diamonds_collected = 0;
        self.amoeba_init();
    }

    /// $C379: every 8th frame, Rockford's orthogonal neighbors are probed for
    /// enemies; contact kills him.
    fn danger_check(&mut self, ev: &mut Vec<Event>) {
        for d in [
            Direction::Up,
            Direction::Left,
            Direction::Right,
            Direction::Down,
        ] {
            if let Some(n) = self.field.neighbor(self.rockford, d) {
                if self.field.cells[n].obj.is_enemy() {
                    self.kill_rockford(DeathCause::Explosion, ev);
                    return;
                }
            }
        }
    }
}

/// Decode raw object ids into a field. Returns (field, door cell, spawn).
/// Rockford spawns at the record's spawn cell unless the data contains an
/// explicit Rockford cell (test maps).
fn build_field(ids: &[u8], params_spawn: Option<usize>) -> (Field, Option<usize>, Option<usize>) {
    debug_assert_eq!(ids.len(), CELLS);
    let mut cells = Vec::with_capacity(CELLS);
    let mut door = None;
    let mut explicit = None;
    for (i, &id) in ids.iter().enumerate() {
        let obj = Obj::from_u8(id);
        match obj {
            Obj::Door => door = Some(i),
            Obj::Rockford => explicit = Some(i),
            _ => {}
        }
        // Bare enemy ids load facing up (direction 0 in the low nibble).
        cells.push(Cell::new(obj));
    }
    let spawn = explicit.or(params_spawn);
    if let Some(s) = spawn {
        cells[s] = Cell::new(Obj::Rockford);
    }
    (Field { cells }, door, spawn)
}

/// The grid a cave loads from: the editable emoji file
/// (`assets/caves/cave_XX.txt`), falling back to the ROM-extracted
/// reference grid if the file fails to parse.
pub fn cave_ids(cave: usize) -> [u8; CELLS] {
    use crate::data::caves::{parse_cave, CAVE_FILES};
    assert!(cave < CAVE_COUNT, "cave index out of range");
    parse_cave(CAVE_FILES[cave]).unwrap_or(CAVES[cave])
}
