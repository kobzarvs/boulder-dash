//! Game-flow layer: the title -> map -> cave -> game-over state machine,
//! modeled on the ROM's 19-state dispatch table ($A5F2, Q9) with the
//! screen-transition states merged. See the module list:
//!
//! * [`demo`] — attract-mode script tables + input feeder (Q11).
//! * [`password`] — the 24-row literal password lookup (Q10).
//! * [`screens`] — draw functions for every non-gameplay screen.
//!
//! State mapping vs the ROM (19 states):
//! * 0 title + 1 attract/menu -> [`State::Title`] (idle ~30 s -> demo).
//! * 2 -> [`State::ColorSelect`] (Q7 color table; in 2P it runs per player).
//! * 3 -> [`State::Password`] (silent on failure, faithful).
//! * 4/6/15/16 map intro/walk/hub/transition -> [`State::Map`].
//! * 5/7 screen-prep transitions -> merged (no-ops here).
//! * 8 -> [`State::PreCave`] status card.
//! * 9/10/11 cave load/intro-pan/gameplay -> [`State::Playing`] (the intro
//!   pan is skipped: camera snaps to Rockford).
//! * 12 -> [`State::CaveComplete`] (banner + time-bonus tally; the 24-step
//!   walk-out sprite animation is simplified to the frozen cave + banner).
//! * 13 death -> inside [`State::Playing`]: the engine runs the death arc and
//!   respawn; 2P alternation happens on `Event::Respawned` (Q12).
//! * 14 -> [`State::GameOver`] (password + CONTINUE/END).
//! * 17 -> [`State::Ending`]; 18 per-difficulty card -> [`State::QuestSplash`].
//!
//! Simplifications vs the ROM (read-only engine constraints):
//! * Reserve lives reset to 3 when entering a new cave (the engine owns
//!   lives per `Cave` instance and exposes no setter); they persist across
//!   deaths and 2P swaps within a cave visit, exactly like the original.
//! * Score is banked per player at cave completion and shown as a carried
//!   total over the HUD; the engine's per-cave extra-life threshold still
//!   runs per visit.
//! * Password entry applies to player 1 only (the ROM runs state 3 once,
//!   on the active player, which is always P1 at game start).

pub mod demo;
pub mod password;
pub mod screens;

use macroquad::prelude::*;

use boulder_dash::audio::slots;
use boulder_dash::engine::{Cave, CaveStatus, Event, Input, START_RESERVE_LIVES};
use boulder_dash::render::camera::{Camera, CELL_PX};
use boulder_dash::render::rockford::RockfordAnim;
use boulder_dash::render::slide::SlideTracker;
use boulder_dash::render::Renderer;

use crate::{Audio, SCREEN_W, TICK};

/// Title idles this long before the attract demo starts (~30 s at 60 Hz).
const TITLE_IDLE_TICKS: u32 = 30 * 60;
/// Post-confirm flash on the password screen before the map appears.
const PASSWORD_CONFIRM_TICKS: u32 = 48;
/// Pre-cave card auto-advance (the ROM waits for A; we time out too).
const PRECAVE_TICKS: u32 = 240;
/// Quest/world splash auto-advance.
const SPLASH_TICKS: u32 = 240;
/// Tally pace: bonus units counted per tick ($AD50 does 1 per 2 frames).
const TALLY_PER_TICK: u32 = 2;
/// Extra hold after the tally finishes, in ticks.
const TALLY_HOLD_TICKS: u32 = 75;

pub const WORLD_COUNT: usize = 6;
pub const TOWNS_PER_WORLD: usize = 4;
pub const QUEST_COUNT: usize = 4;

/// NES-style controller state for the flow layer. Gameplay maps to the
/// engine's `Input` (grab = A or B, suicide = A+B held).
#[derive(Default, Clone, Copy)]
pub struct Pad {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub a: bool,
    pub b: bool,
    pub start: bool,
    pub select: bool,
}

impl Pad {
    /// Rising-edge buttons: set only where pressed now but not before.
    fn just(self, prev: Pad) -> Pad {
        macro_rules! edge {
            ($f:ident) => {
                self.$f && !prev.$f
            };
        }
        Pad {
            up: edge!(up),
            down: edge!(down),
            left: edge!(left),
            right: edge!(right),
            a: edge!(a),
            b: edge!(b),
            start: edge!(start),
            select: edge!(select),
        }
    }

    fn any(self) -> bool {
        self.up || self.down || self.left || self.right || self.a || self.b || self.start || self.select
    }

    /// Gameplay input: d-pad plus grab/suicide from the two buttons.
    fn engine_input(self) -> Input {
        Input {
            up: self.up,
            down: self.down,
            left: self.left,
            right: self.right,
            grab: self.a || self.b,
            suicide: self.a && self.b,
        }
    }
}

/// 1P / 2P mode ($72 bits 4/5 in the ROM).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    OnePlayer,
    TwoPlayer,
}

/// Per-player progress ($74-$87 struct in the ROM): world/town position,
/// quest (difficulty), town completion bits, banked score, suit color.
#[derive(Clone)]
pub struct Player {
    pub world: usize,
    pub town: usize,
    pub quest: usize,
    /// Bit per beaten town in the current world (ROM $76 masks 01/02/04/08).
    pub towns_cleared: u8,
    /// Score carried into the current cave visit (banked at completion).
    pub banked_score: u32,
    /// Index into `ROCKFORD_COLORS` (default 6, $D99B).
    pub color_idx: usize,
    /// 2P: still has lives left ($72 active bits).
    pub in_game: bool,
}

impl Player {
    fn new() -> Player {
        Player {
            world: 0,
            town: 0,
            quest: 0,
            towns_cleared: 0,
            banked_score: 0,
            color_idx: 6,
            in_game: true,
        }
    }

    /// Cave index 0-23 = world * 4 + town.
    fn cave_index(&self) -> usize {
        self.world * TOWNS_PER_WORLD + self.town
    }

    fn total_score(&self, cave: Option<&Cave>) -> u32 {
        self.banked_score + cave.map(|c| c.score()).unwrap_or(0)
    }
}

/// First unbeaten town index (map cursor default).
fn first_unbeaten(towns_cleared: u8) -> usize {
    (0..TOWNS_PER_WORLD)
        .find(|t| towns_cleared & (1 << t) == 0)
        .unwrap_or(0)
}

/// Flow states. Payloads carry per-screen UI state (cursors, timers).
pub enum State {
    /// Title + attract menu (ROM 0/1): logo, 1P/2P select, idle -> demo.
    Title { sel: usize, idle: u32 },
    /// Rockford suit color (ROM 2); runs for player 0 then 1 in 2P mode.
    ColorSelect { player: usize },
    /// 6-digit password (ROM 3). `confirm > 0` counts the post-A flash.
    Password { digits: [u8; 6], cursor: usize, confirm: u32 },
    /// World map (ROM 4/6/15/16): cursor over the 4 town nodes.
    Map { cursor: usize },
    /// World/quest advance interstitial (ROM 18).
    QuestSplash { ticks: u32 },
    /// Pre-cave status card (ROM 8).
    PreCave { ticks: u32 },
    /// Gameplay (ROM 9-11); `Some(feeder)` = attract demo.
    Playing { demo: Option<demo::DemoFeeder> },
    /// Cave complete (ROM 12): banner + time-bonus tally, then the map.
    CaveComplete { ticks: u32, bonus: u32 },
    /// Game over / continue (ROM 14).
    GameOver { sel: usize },
    /// All 6 worlds x 4 quests beaten (ROM 17).
    Ending { ticks: u32 },
}

/// The whole game flow: state machine + both players + live cave instances.
///
/// Each player with a cave in progress owns a `Cave` instance (score/lives
/// live in the engine), which is what makes the 2P death-swap lossless:
/// swapping players just swaps which instance ticks.
pub struct Flow {
    state: State,
    mode: Mode,
    active: usize,
    players: [Player; 2],
    caves: [Option<Cave>; 2],
    /// Cave index of the active session (engine exposes no accessor).
    cur_cave_idx: usize,
    rock: Option<RockfordAnim>,
    slides: SlideTracker,
    cam: Camera,
    magic_active: bool,
    paused: bool,
    /// Global flow tick (screen animation + SFX cooldown clock).
    tick: u64,
    prev_pad: Pad,
    demo_next: usize,
}

fn set_music(audio: &mut Option<Audio>, slot: usize) {
    if let Some(a) = audio {
        a.set_music(Some(slot));
    }
}

fn sfx(audio: &mut Option<Audio>, slot: usize, now: u64) {
    if let Some(a) = audio {
        a.play_sfx(slot, now);
    }
}

impl Flow {
    pub fn new() -> Flow {
        Flow {
            state: State::Title { sel: 0, idle: 0 },
            mode: Mode::OnePlayer,
            active: 0,
            players: [Player::new(), Player::new()],
            caves: [None, None],
            cur_cave_idx: 0,
            rock: None,
            slides: SlideTracker::default(),
            cam: Camera::new(),
            magic_active: false,
            paused: false,
            tick: 0,
            prev_pad: Pad::default(),
            demo_next: 0,
        }
    }

    /// The cave currently being played (BDSHOT debug trace).
    pub fn active_cave(&self) -> Option<&Cave> {
        self.caves[self.active].as_ref()
    }

    /// Current state name, for tests and debug output.
    pub fn state_name(&self) -> &'static str {
        match &self.state {
            State::Title { .. } => "title",
            State::ColorSelect { .. } => "color",
            State::Password { .. } => "password",
            State::Map { .. } => "map",
            State::QuestSplash { .. } => "splash",
            State::PreCave { .. } => "precave",
            State::Playing { demo: Some(_) } => "demo",
            State::Playing { .. } => "playing",
            State::CaveComplete { .. } => "complete",
            State::GameOver { .. } => "gameover",
            State::Ending { .. } => "ending",
        }
    }

    /// One 60 Hz tick.
    pub fn update(&mut self, pad: Pad, audio: &mut Option<Audio>) {
        let just = pad.just(self.prev_pad);
        self.prev_pad = pad;
        self.tick += 1;
        // Take the state out so the arms can call &mut self helpers freely.
        let state = std::mem::replace(&mut self.state, State::Title { sel: 0, idle: 0 });
        self.state = self.step(state, pad, just, audio);
    }

    fn step(&mut self, state: State, pad: Pad, just: Pad, audio: &mut Option<Audio>) -> State {
        match state {
            State::Title { mut sel, mut idle } => {
                set_music(audio, slots::MUSIC_UNK_18);
                if just.left || just.right || just.up || just.down || just.select {
                    sel ^= 1;
                    sfx(audio, slots::SFX_AMOEBA, self.tick);
                    idle = 0;
                }
                if just.start || just.a {
                    self.mode = if sel == 0 { Mode::OnePlayer } else { Mode::TwoPlayer };
                    self.players = [Player::new(), Player::new()];
                    self.active = 0;
                    self.caves = [None, None];
                    return State::ColorSelect { player: 0 };
                }
                if pad.any() {
                    idle = 0;
                } else {
                    idle += 1;
                }
                if idle >= TITLE_IDLE_TICKS {
                    return self.start_demo(audio);
                }
                State::Title { sel, idle }
            }

            State::ColorSelect { player } => {
                set_music(audio, slots::MUSIC_UNK_19);
                {
                    // Index walks mod 16 ($A740); debounced to button edges.
                    let p = &mut self.players[player];
                    if just.left {
                        p.color_idx = (p.color_idx + 15) % 16;
                        sfx(audio, slots::SFX_AMOEBA, self.tick);
                    }
                    if just.right {
                        p.color_idx = (p.color_idx + 1) % 16;
                        sfx(audio, slots::SFX_AMOEBA, self.tick);
                    }
                }
                if just.a || just.start {
                    sfx(audio, slots::SFX_SCREEN_B, self.tick);
                    if self.mode == Mode::TwoPlayer && player == 0 {
                        return State::ColorSelect { player: 1 };
                    }
                    return State::Password { digits: [0; 6], cursor: 0, confirm: 0 };
                }
                State::ColorSelect { player }
            }

            State::Password { mut digits, mut cursor, confirm } => {
                set_music(audio, slots::MUSIC_UNK_19);
                if confirm > 0 {
                    if confirm + 1 >= PASSWORD_CONFIRM_TICKS {
                        let c = first_unbeaten(self.players[self.active].towns_cleared);
                        return State::Map { cursor: c };
                    }
                    return State::Password { digits, cursor, confirm: confirm + 1 };
                }
                if just.left {
                    cursor = (cursor + 5) % 6;
                    sfx(audio, slots::SFX_AMOEBA, self.tick);
                }
                if just.right {
                    cursor = (cursor + 1) % 6;
                    sfx(audio, slots::SFX_AMOEBA, self.tick);
                }
                if just.up {
                    digits[cursor] = (digits[cursor] + 1) % 10;
                    sfx(audio, slots::SFX_AMOEBA, self.tick);
                }
                if just.down {
                    digits[cursor] = (digits[cursor] + 9) % 10;
                    sfx(audio, slots::SFX_AMOEBA, self.tick);
                }
                if just.a || just.start {
                    sfx(audio, slots::SFX_SCREEN_A, self.tick);
                    // $A8BA literal lookup; failure leaves the default game
                    // untouched and the flow moves on without saying so.
                    if let Some(t) = password::validate(&digits) {
                        let pl = &mut self.players[self.active];
                        pl.world = t.world;
                        pl.quest = t.quest;
                        pl.town = 0;
                        pl.towns_cleared = 0;
                    }
                    return State::Password { digits, cursor, confirm: 1 };
                }
                State::Password { digits, cursor, confirm: 0 }
            }

            State::Map { mut cursor } => {
                set_music(audio, slots::MUSIC_TITLE);
                if just.left {
                    cursor = (cursor + TOWNS_PER_WORLD - 1) % TOWNS_PER_WORLD;
                    sfx(audio, slots::SFX_AMOEBA, self.tick);
                }
                if just.right {
                    cursor = (cursor + 1) % TOWNS_PER_WORLD;
                    sfx(audio, slots::SFX_AMOEBA, self.tick);
                }
                if just.a || just.start {
                    let pl = &mut self.players[self.active];
                    // Completed towns can't be re-entered ($B78A mask check).
                    if pl.towns_cleared & (1 << cursor) == 0 {
                        pl.town = cursor;
                        sfx(audio, slots::SFX_CARD_PAUSE, self.tick);
                        return State::PreCave { ticks: 0 };
                    }
                }
                State::Map { cursor }
            }

            State::QuestSplash { ticks } => {
                set_music(audio, slots::MUSIC_TITLE);
                if just.start || just.a || ticks + 1 >= SPLASH_TICKS {
                    let c = first_unbeaten(self.players[self.active].towns_cleared);
                    return State::Map { cursor: c };
                }
                State::QuestSplash { ticks: ticks + 1 }
            }

            State::PreCave { ticks } => {
                set_music(audio, slots::MUSIC_UNK_19);
                if just.a || just.start || ticks + 1 >= PRECAVE_TICKS {
                    sfx(audio, slots::SFX_CARD_RESUME, self.tick);
                    if self.caves[self.active].is_none() {
                        let pl = &self.players[self.active];
                        let idx = pl.cave_index();
                        let level = (pl.quest + 1) as u8;
                        self.caves[self.active] = Some(Cave::new(idx, level, 0));
                    }
                    self.cur_cave_idx = self.players[self.active].cave_index();
                    self.enter_session(audio);
                    return State::Playing { demo: None };
                }
                State::PreCave { ticks: ticks + 1 }
            }

            State::Playing { demo } => self.step_playing(demo, pad, just, audio),

            State::CaveComplete { ticks, bonus } => {
                let ticks = ticks + 1;
                let shown = (ticks * TALLY_PER_TICK).min(bonus);
                let done = shown >= bonus && ticks >= bonus / TALLY_PER_TICK + TALLY_HOLD_TICKS;
                if done || just.a || just.start {
                    return self.finish_cave(audio);
                }
                State::CaveComplete { ticks, bonus }
            }

            State::GameOver { mut sel } => {
                set_music(audio, slots::MUSIC_UNK_19);
                if just.up || just.down {
                    sel ^= 1;
                    sfx(audio, slots::SFX_AMOEBA, self.tick);
                }
                if just.a || just.start {
                    if sel == 0 {
                        // CONTINUE ($AFF9): town progress and score reset,
                        // lives back to 3 (fresh cave), world/quest kept.
                        sfx(audio, slots::SFX_SCREEN_A, self.tick);
                        let pl = &mut self.players[self.active];
                        pl.town = 0;
                        pl.towns_cleared = 0;
                        pl.banked_score = 0;
                        self.caves[self.active] = None;
                        return State::Map { cursor: 0 };
                    }
                    // END ($AF5F): this player leaves the game.
                    sfx(audio, slots::SFX_SCREEN_B, self.tick);
                    self.players[self.active].in_game = false;
                    self.caves[self.active] = None;
                    let other = self.active ^ 1;
                    if self.mode == Mode::TwoPlayer && self.players[other].in_game {
                        self.active = other;
                        let c = first_unbeaten(self.players[other].towns_cleared);
                        return State::Map { cursor: c };
                    }
                    return self.reset_to_title();
                }
                State::GameOver { sel }
            }

            State::Ending { ticks } => {
                set_music(audio, slots::MUSIC_UNK_18);
                if just.start || just.a {
                    return self.reset_to_title();
                }
                State::Ending { ticks: ticks + 1 }
            }
        }
    }

    /// Gameplay tick (also the attract demo when `demo` is `Some`).
    fn step_playing(
        &mut self,
        mut demo: Option<demo::DemoFeeder>,
        pad: Pad,
        just: Pad,
        audio: &mut Option<Audio>,
    ) -> State {
        let demo_mode = demo.is_some();
        // In demo mode Start exits to the title ($ABA4); in play it pauses
        // ($70 = 1 pause substate).
        if just.start {
            if demo_mode {
                return self.reset_to_title();
            }
            self.paused = !self.paused;
            if let Some(a) = audio {
                a.set_paused(self.paused);
            }
        }
        if self.paused {
            return State::Playing { demo };
        }

        let input = match &mut demo {
            Some(f) => {
                let i = f.input();
                f.advance();
                i
            }
            None => pad.engine_input(),
        };
        if demo.as_ref().map(|f| f.finished()).unwrap_or(false) {
            return self.reset_to_title();
        }

        let mut cave = self.caves[self.active].take().expect("playing without cave");
        let events = cave.tick(input);
        let mut complete = None;
        let mut respawned = false;
        for ev in events {
            match ev {
                Event::CaveComplete { time_bonus } => complete = Some(time_bonus),
                Event::Respawned => respawned = true,
                Event::RockfordDied { cause } => {
                    if let Some(a) = audio {
                        a.on_death(cause, self.tick);
                    }
                }
                Event::MagicWallActivated => self.magic_active = true,
                Event::MagicWallExpired => self.magic_active = false,
                Event::Sound(cue) => {
                    if let Some(a) = audio {
                        a.on_cue(cue, self.tick);
                    }
                }
                _ => {}
            }
        }
        if let Some(rock) = &mut self.rock {
            rock.update(&cave);
        }
        self.slides.update(&cave, self.tick);
        let game_over = matches!(cave.status(), CaveStatus::GameOver);
        self.caves[self.active] = Some(cave);

        if let Some(rock) = &self.rock {
            let (px, py) = rock.pos;
            self.cam.follow(px * CELL_PX, py * CELL_PX, TICK);
        }

        if respawned {
            let cave = self.caves[self.active].as_ref().unwrap();
            self.rock = Some(RockfordAnim::new(cave));
            self.slides.reset(cave);
            let (px, py) = self.rock.as_ref().unwrap().pos;
            self.cam.snap(px * CELL_PX, py * CELL_PX);
            self.magic_active = false;
            if let Some(a) = audio {
                a.respawned();
            }
            // 2P alternation is death-driven ($AE14: $72 ^= 1): the other
            // player takes the next attempt, on their own cave.
            if !demo_mode && self.mode == Mode::TwoPlayer && self.players[self.active ^ 1].in_game {
                self.active ^= 1;
                self.cur_cave_idx = self.players[self.active].cave_index();
                set_music(audio, slots::MUSIC_UNK_19);
                sfx(audio, slots::SFX_CARD_PAUSE, self.tick);
                return State::PreCave { ticks: 0 };
            }
        }
        if let Some(bonus) = complete {
            if demo_mode {
                return self.reset_to_title();
            }
            if let Some(a) = audio {
                a.cave_complete();
            }
            return State::CaveComplete { ticks: 0, bonus };
        }
        if game_over {
            if demo_mode {
                return self.reset_to_title();
            }
            if let Some(a) = audio {
                a.game_over(self.tick);
                a.play_sfx(slots::SFX_GAME_OVER_B, self.tick);
            }
            return State::GameOver { sel: 0 };
        }
        State::Playing { demo }
    }

    /// Attract mode ($D985): structs reset, cave from the demo table, scripted
    /// input; each attract loop cycles to the next demo cave/script.
    fn start_demo(&mut self, audio: &mut Option<Audio>) -> State {
        let d = self.demo_next;
        self.demo_next = (d + 1) % demo::DEMO_CAVES.len();
        let cave_idx = demo::DEMO_CAVES[d];
        self.players = [Player::new(), Player::new()];
        self.caves = [None, None];
        self.caves[0] = Some(Cave::new(cave_idx, 1, 0));
        self.active = 0;
        self.cur_cave_idx = cave_idx;
        self.enter_session(audio);
        State::Playing { demo: Some(demo::DemoFeeder::new(d)) }
    }

    /// Cave cleared ($AD7D): mark the town, bank the score, advance the
    /// world when all 4 towns are done; world 6 wrap raises the quest;
    /// quest 4 wrap = game beaten ($0408).
    fn finish_cave(&mut self, audio: &mut Option<Audio>) -> State {
        let cave_score = self.caves[self.active].as_ref().map(|c| c.score()).unwrap_or(0);
        let pl = &mut self.players[self.active];
        pl.towns_cleared |= 1 << pl.town;
        pl.banked_score += cave_score;
        self.caves[self.active] = None;

        if pl.towns_cleared & 0xF == 0xF {
            pl.towns_cleared = 0;
            pl.town = 0;
            pl.world += 1;
            if pl.world >= WORLD_COUNT {
                pl.world = 0;
                pl.quest += 1;
            }
            if pl.quest >= QUEST_COUNT {
                set_music(audio, slots::MUSIC_UNK_18);
                return State::Ending { ticks: 0 };
            }
            set_music(audio, slots::MUSIC_TITLE);
            return State::QuestSplash { ticks: 0 };
        }
        let c = first_unbeaten(pl.towns_cleared);
        State::Map { cursor: c }
    }

    /// Back to the title screen with everything reset.
    fn reset_to_title(&mut self) -> State {
        self.mode = Mode::OnePlayer;
        self.players = [Player::new(), Player::new()];
        self.caves = [None, None];
        self.active = 0;
        self.rock = None;
        self.paused = false;
        State::Title { sel: 0, idle: 0 }
    }

    /// Reset camera/Rockford animation to the active cave and start its music.
    fn enter_session(&mut self, audio: &mut Option<Audio>) {
        let cave = self.caves[self.active].as_ref().expect("session without cave");
        self.rock = Some(RockfordAnim::new(cave));
        self.slides.reset(cave);
        let (px, py) = self.rock.as_ref().unwrap().pos;
        self.cam.snap(px * CELL_PX, py * CELL_PX);
        self.magic_active = false;
        self.paused = false;
        if let Some(a) = audio {
            a.enter_cave(self.cur_cave_idx, self.tick);
        }
    }

    /// Draw the current state. The caller has the 256x240 camera active.
    pub fn render(&mut self, renderer: &Renderer) {
        match &self.state {
            State::Title { sel, .. } => screens::draw_title(renderer, *sel, self.tick),
            State::ColorSelect { player } => {
                screens::draw_color_select(
                    renderer,
                    *player,
                    self.mode == Mode::TwoPlayer,
                    self.players[*player].color_idx,
                    self.tick,
                );
            }
            State::Password { digits, cursor, confirm } => {
                screens::draw_password(renderer, digits, *cursor, *confirm > 0, self.tick);
            }
            State::Map { cursor } => {
                let cursor = *cursor;
                let pl = &self.players[self.active];
                let (world, quest, cleared) = (pl.world, pl.quest, pl.towns_cleared);
                let score = pl.total_score(self.caves[self.active].as_ref());
                let lives = self.caves[self.active]
                    .as_ref()
                    .map(|c| c.lives())
                    .unwrap_or(START_RESERVE_LIVES);
                screens::draw_map(
                    renderer,
                    world,
                    quest,
                    cursor,
                    cleared,
                    score,
                    lives,
                    self.active,
                    self.mode == Mode::TwoPlayer,
                    pl.color_idx,
                    self.tick,
                );
            }
            State::QuestSplash { .. } => {
                let pl = &self.players[self.active];
                screens::draw_quest_splash(renderer, pl.world, pl.quest, self.tick);
            }
            State::PreCave { .. } => {
                let pl = &self.players[self.active];
                let idx = pl.cave_index();
                let params = &boulder_dash::data::cave_params::CAVE_PARAMS[idx];
                let lives = self.caves[self.active]
                    .as_ref()
                    .map(|c| c.lives())
                    .unwrap_or(START_RESERVE_LIVES);
                screens::draw_precave(
                    renderer,
                    idx,
                    pl.quest,
                    params.diamonds_needed[pl.quest] as u32,
                    params.cave_time[pl.quest] as u32,
                    lives,
                    self.active,
                    self.mode == Mode::TwoPlayer,
                    self.tick,
                );
            }
            State::Playing { .. } | State::CaveComplete { .. } => self.render_cave_scene(renderer),
            State::GameOver { sel } => {
                let sel = *sel;
                let pl = &self.players[self.active];
                let pw = password::for_progress(pl.quest, pl.world);
                screens::draw_gameover(renderer, &pw, sel, self.active, self.mode == Mode::TwoPlayer, self.tick);
            }
            State::Ending { .. } => {
                let score = self.players.iter().map(|p| p.banked_score).sum();
                screens::draw_ending(renderer, score, self.tick);
            }
        }
    }

    /// Gameplay + cave-complete rendering: world, HUD, state overlays.
    fn render_cave_scene(&mut self, renderer: &Renderer) {
        let cave = self.caves[self.active].as_ref().expect("render without cave");
        if let Some(rock) = &self.rock {
            let suit = self.players[self.active].color_idx;
            renderer.draw_world(cave, self.cur_cave_idx, &self.cam, self.tick, self.magic_active, rock, &self.slides, suit);
        }
        // HUD bar: opaque over the top (covers viewport bleed), then contents.
        draw_rectangle(0.0, 0.0, SCREEN_W, 32.0, BLACK);
        let pl = &self.players[self.active];
        renderer.draw_hud(cave, self.cur_cave_idx, (pl.quest + 1) as u8, self.tick);
        let total = pl.total_score(Some(cave));
        if pl.banked_score > 0 {
            screens::draw_total_score(&renderer.atlas, total);
        }
        match &self.state {
            State::Playing { demo: Some(_) } => screens::draw_demo_label(&renderer.atlas, self.tick),
            State::Playing { .. } if self.paused => {
                renderer.draw_overlay(&["PAUSED", "!PRESS START"], self.tick)
            }
            State::CaveComplete { ticks, bonus } => {
                let shown = (*ticks * TALLY_PER_TICK).min(*bonus);
                screens::draw_clear_tally(&renderer.atlas, shown, total, self.tick);
            }
            _ => {}
        }
    }

    /// BDSHOT_FLOW helper: jump straight to a named screen.
    pub fn force_screen(&mut self, name: &str) {
        self.state = match name {
            "title" => State::Title { sel: 0, idle: 0 },
            "color" => State::ColorSelect { player: 0 },
            "password" => State::Password { digits: [0; 6], cursor: 0, confirm: 0 },
            "map" => State::Map { cursor: 0 },
            // Map with towns 1 and 3 cleared (marker rendering check).
            "mapc" => {
                self.players[0].towns_cleared = 0b0101;
                State::Map { cursor: 1 }
            }
            "splash" => State::QuestSplash { ticks: 0 },
            "precave" => State::PreCave { ticks: 0 },
            "gameover" => State::GameOver { sel: 0 },
            "ending" => {
                self.players[0].banked_score = 128_450;
                State::Ending { ticks: 0 }
            }
            "demo" => {
                let mut no_audio = None;
                self.start_demo(&mut no_audio)
            }
            _ => {
                eprintln!("BDSHOT_FLOW: unknown screen '{name}', using title");
                State::Title { sel: 0, idle: 0 }
            }
        };
    }

    /// Debug helpers for the interactive build: current cave index and
    /// difficulty level (1-4) of the active player.
    pub fn cur_cave_idx(&self) -> usize {
        self.cur_cave_idx
    }

    pub fn cur_level(&self) -> u8 {
        (self.players[self.active].quest + 1) as u8
    }

    /// BDSHOT helper: jump straight into gameplay of cave `cave_idx`.
    /// `BDSUIT=<0-15>` picks the suit color table index (default 6).
    pub fn debug_play(&mut self, cave_idx: usize, level: u8) {
        {
            let p = &mut self.players[0];
            p.world = cave_idx / TOWNS_PER_WORLD;
            p.town = cave_idx % TOWNS_PER_WORLD;
            p.quest = (level.max(1) - 1).min(3) as usize;
            if let Ok(idx) = std::env::var("BDSUIT") {
                if let Ok(idx) = idx.parse::<usize>() {
                    p.color_idx = idx % 16;
                }
            }
        }
        self.caves[0] = Some(Cave::new(cave_idx, level, 0));
        self.active = 0;
        self.cur_cave_idx = cave_idx;
        let mut no_audio = None;
        self.enter_session(&mut no_audio);
        self.state = State::Playing { demo: None };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none_audio() -> Option<Audio> {
        None
    }

    /// One button tap = press tick + release tick (edges are per-tick).
    fn tap(f: &mut Flow, pad: Pad) {
        f.update(pad, &mut none_audio());
        f.update(Pad::default(), &mut none_audio());
    }

    fn tap_start(f: &mut Flow) {
        tap(f, Pad { start: true, ..Default::default() });
    }

    #[test]
    fn title_start_begins_1p_color_select() {
        let mut f = Flow::new();
        tap_start(&mut f);
        assert_eq!(f.state_name(), "color");
        assert_eq!(f.mode, Mode::OnePlayer);
        // 1P: color select runs once, then password.
        tap(&mut f, Pad { a: true, ..Default::default() });
        assert_eq!(f.state_name(), "password");
    }

    #[test]
    fn title_2p_runs_color_select_per_player() {
        let mut f = Flow::new();
        tap(&mut f, Pad { select: true, ..Default::default() });
        tap_start(&mut f);
        assert_eq!(f.mode, Mode::TwoPlayer);
        assert!(matches!(f.state, State::ColorSelect { player: 0 }));
        tap(&mut f, Pad { a: true, ..Default::default() });
        assert!(matches!(f.state, State::ColorSelect { player: 1 }));
        tap(&mut f, Pad { a: true, ..Default::default() });
        assert_eq!(f.state_name(), "password");
    }

    #[test]
    fn idle_title_starts_demo() {
        let mut f = Flow::new();
        for _ in 0..TITLE_IDLE_TICKS {
            f.update(Pad::default(), &mut none_audio());
        }
        assert_eq!(f.state_name(), "demo");
        assert!(f.active_cave().is_some());
    }

    #[test]
    fn demo_script_drives_the_real_engine() {
        let mut f = Flow::new();
        f.force_screen("demo");
        let start = f.active_cave().unwrap().rockford_pos();
        let mut moved = false;
        for _ in 0..300 {
            f.update(Pad::default(), &mut none_audio());
            if f.state_name() != "demo" {
                break;
            }
            if f.active_cave().map(|c| c.rockford_pos()) != Some(start) {
                moved = true;
                break;
            }
        }
        assert!(moved, "demo script never moved Rockford");
    }

    #[test]
    fn demo_start_exits_to_title() {
        let mut f = Flow::new();
        f.force_screen("demo");
        tap_start(&mut f);
        assert_eq!(f.state_name(), "title");
    }

    #[test]
    fn password_635807_unlocks_world_2() {
        let mut f = Flow::new();
        f.force_screen("password");
        for (i, &d) in [6u8, 3, 5, 8, 7, 0].iter().enumerate() {
            for _ in 0..d {
                tap(&mut f, Pad { up: true, ..Default::default() });
            }
            if i < 5 {
                tap(&mut f, Pad { right: true, ..Default::default() });
            }
        }
        tap(&mut f, Pad { a: true, ..Default::default() });
        for _ in 0..PASSWORD_CONFIRM_TICKS {
            f.update(Pad::default(), &mut none_audio());
        }
        assert_eq!(f.state_name(), "map");
        assert_eq!((f.players[0].world, f.players[0].quest), (1, 0));
    }

    #[test]
    fn bad_password_silently_keeps_defaults() {
        let mut f = Flow::new();
        f.force_screen("password");
        // 111111 is not in the table.
        tap(&mut f, Pad { up: true, ..Default::default() });
        for _ in 0..5 {
            tap(&mut f, Pad { right: true, ..Default::default() });
            tap(&mut f, Pad { up: true, ..Default::default() });
        }
        tap(&mut f, Pad { a: true, ..Default::default() });
        for _ in 0..PASSWORD_CONFIRM_TICKS {
            f.update(Pad::default(), &mut none_audio());
        }
        assert_eq!(f.state_name(), "map");
        assert_eq!((f.players[0].world, f.players[0].quest), (0, 0));
    }

    #[test]
    fn map_enter_precave_then_playing() {
        let mut f = Flow::new();
        f.force_screen("map");
        tap(&mut f, Pad { a: true, ..Default::default() });
        assert_eq!(f.state_name(), "precave");
        tap(&mut f, Pad { a: true, ..Default::default() });
        assert_eq!(f.state_name(), "playing");
        let cave = f.active_cave().unwrap();
        assert_eq!(cave.diamonds_needed(), 10); // cave A quest 1
        assert_eq!(cave.lives(), START_RESERVE_LIVES);
    }

    #[test]
    fn completed_town_cannot_be_reentered() {
        let mut f = Flow::new();
        f.force_screen("mapc"); // towns 0 and 2 cleared, cursor on 1
        // Move to cleared town 0: A must be ignored.
        tap(&mut f, Pad { left: true, ..Default::default() });
        tap(&mut f, Pad { a: true, ..Default::default() });
        assert_eq!(f.state_name(), "map");
        // Town 1 is open.
        tap(&mut f, Pad { right: true, ..Default::default() });
        tap(&mut f, Pad { a: true, ..Default::default() });
        assert_eq!(f.state_name(), "precave");
        assert_eq!(f.players[0].town, 1);
    }

    #[test]
    fn cave_completion_marks_town_and_returns_to_map() {
        let mut f = Flow::new();
        f.force_screen("map");
        tap(&mut f, Pad { a: true, ..Default::default() });
        tap(&mut f, Pad { a: true, ..Default::default() });
        assert_eq!(f.state_name(), "playing");
        // Simulate the engine reporting completion.
        f.state = State::CaveComplete { ticks: 0, bonus: 42 };
        let next = f.finish_cave(&mut none_audio());
        f.state = next;
        assert_eq!(f.state_name(), "map");
        assert_eq!(f.players[0].towns_cleared, 0b0001);
    }

    #[test]
    fn world_clear_advances_and_quest_four_ends_game() {
        let mut f = Flow::new();
        f.debug_play(0, 1);
        // Clearing the 4th town of world 5 on quest 3 (last) -> ending.
        f.players[0].world = 5;
        f.players[0].town = 3;
        f.players[0].quest = 3;
        f.players[0].towns_cleared = 0b0111;
        f.caves[0] = Some(Cave::new(23, 4, 0));
        let next = f.finish_cave(&mut none_audio());
        f.state = next;
        assert_eq!(f.state_name(), "ending");
    }
}
