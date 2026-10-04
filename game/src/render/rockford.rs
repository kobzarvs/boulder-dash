//! Rockford rendering.
//!
//! Legacy NES art (`RockfordArt`): the 2-sprite 16x8 head overlay sheet with
//! one row per suit color — still used by the title/map screens.
//!
//! Gameplay uses the HD Blender robot instead (`render::robot` atlas):
//! `RockfordAnim` tracks the cell-to-cell slide, eases a float azimuth
//! towards the travel direction (0 = facing camera, +90 = right, -90 = left,
//! 180 = back) and picks atlas rows/frames for idle foot-tapping, run
//! cycles, boulder pushes, periodic fidgets (head scratch / look around)
//! and the death arc.

use macroquad::prelude::*;

use crate::data::cave_params::{NES_PALETTE, ROCKFORD_COLORS};
use crate::data::sprites::{HeadFrame, ROCKFORD_HEAD_TILES};
use crate::data::tiles::TILES;
use crate::engine::{Cave, Direction};
use crate::engine::WIDTH;
use crate::render::robot::{self, FRAMES, ROW_DANCE, ROW_IDLE, ROW_LOOK, ROW_PUSH, ROW_RUN, ROW_SCRATCH};

/// CHR bank of the head tiles (sprite pattern table 0 in gameplay).
const HEAD_BANK: usize = 4;
/// Sprite palette 0 color 1 ($A7E2): Rockford's skin.
const SKIN_NES: u8 = 0x36;
/// Sprite palette 0 color 3: outline black.
const OUTLINE_NES: u8 = 0x0F;

/// Head sprite sheet: columns = [`ROCKFORD_HEAD_TILES`], rows = suit colors.
pub struct RockfordArt {
    tex: Texture2D,
}

impl Default for RockfordArt {
    fn default() -> Self {
        Self::new()
    }
}

impl RockfordArt {
    pub fn new() -> RockfordArt {
        let cols = ROCKFORD_HEAD_TILES.len();
        let rows = ROCKFORD_COLORS.len();
        let mut img = Image::gen_image_color(cols as u16 * 8, rows as u16 * 8, Color::new(0.0, 0.0, 0.0, 0.0));
        for (row, &suit) in ROCKFORD_COLORS.iter().enumerate() {
            let pal = [0u8, SKIN_NES, suit, OUTLINE_NES];
            for (col, &tile) in ROCKFORD_HEAD_TILES.iter().enumerate() {
                let px = &TILES[HEAD_BANK * 256 + tile as usize];
                for y in 0..8 {
                    for x in 0..8 {
                        let v = px[y * 8 + x] as usize;
                        if v == 0 {
                            continue;
                        }
                        let [r, g, b] = NES_PALETTE[(pal[v] & 0x3F) as usize];
                        img.set_pixel(
                            (col * 8 + x) as u32,
                            (row * 8 + y) as u32,
                            Color::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0),
                        );
                    }
                }
            }
        }
        let tex = Texture2D::from_image(&img);
        tex.set_filter(FilterMode::Nearest);
        RockfordArt { tex }
    }

    pub fn texture(&self) -> &Texture2D {
        &self.tex
    }

    /// Draw the 16x8 head overlay with the body's top-left at (`x`, `y`)
    /// (record offsets Y+16 / X+8 place the head there; attr $40 h-flips),
    /// scaled by `scale` (CELL_PX/16 for the cave field).
    pub fn draw_head(&self, frame: &HeadFrame, color_idx: usize, x: f32, y: f32, scale: f32) {
        let row = color_idx % ROCKFORD_COLORS.len();
        for s in frame {
            let col = ROCKFORD_HEAD_TILES
                .iter()
                .position(|&t| t == s[1])
                .expect("head tile missing from ROCKFORD_HEAD_TILES");
            let dx = (s[3] as i8 as f32 + 8.0) * scale;
            let dy = (s[0] as i8 as f32 + 16.0) * scale;
            draw_texture_ex(
                &self.tex,
                x + dx,
                y + dy,
                WHITE,
                DrawTextureParams {
                    source: Some(Rect::new(col as f32 * 8.0, row as f32 * 8.0, 8.0, 8.0)),
                    dest_size: Some(vec2(8.0 * scale, 8.0 * scale)),
                    flip_x: s[2] & 0x40 != 0,
                    ..Default::default()
                },
            );
        }
    }
}

/// Death spin+fade length in ticks (the ROM's 80-frame arc, $92=$50).
const DEATH_ARC_TICKS: u32 = 80;
/// Idle ticks after the last step before he settles out of the run pose
/// (keeps the run cycle from flickering at the 8-tick cell boundary).
const WALK_HOLD_TICKS: u32 = 12;
/// How long the push pose plays after a grab-button snap push (instant
/// pushes carry no engine push state, so the event opens a short window).
const SNAP_PUSH_TICKS: u32 = 16;
/// Idle ticks before fidgets (head scratch / look around) start firing.
const FIDGET_START: u32 = 150;
/// Ticks between fidget plays (deterministic — the engine has no RNG).
const FIDGET_PERIOD: u32 = 480;
/// Length of one fidget play in ticks (16 frames at half rate).
const FIDGET_LEN: u32 = 32;

/// One resolved draw command for the robot atlas.
pub struct RobotFrame {
    pub row: usize,
    pub frame: usize,
    pub flip: bool,
    pub rotation: f32,
    pub alpha: f32,
}

/// Wrap to (-180, 180] degrees.
fn wrap180(a: f32) -> f32 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

/// Visual facing for a travel direction: front for down, back for up,
/// sides for left/right (mirrored rows cover the negative half).
fn dir_azimuth(d: Direction) -> f32 {
    match d {
        Direction::Down => 0.0,
        Direction::Right => 90.0,
        Direction::Up => 180.0,
        Direction::Left => -90.0,
    }
}

/// Render-side Rockford animation state: smooth cell-to-cell sliding (the
/// engine moves him one cell per 8 ticks; the original slides 2 px/frame),
/// eased facing, walk/push cycles, idle fidgets and the death arc.
pub struct RockfordAnim {
    /// Cell position currently drawn (fractional while sliding).
    pub pos: (f32, f32),
    /// Cell the current slide started from.
    from: (f32, f32),
    /// Engine cell he is sliding towards / resting at.
    target: usize,
    /// Slide progress in ticks (0..=8).
    step: u8,
    facing: Direction,
    idle_ticks: u32,
    /// Ticks since the engine reported him dead (0 = alive).
    dead_ticks: u32,
    /// Visual facing in degrees, eased towards the travel direction.
    azimuth: f32,
    /// Direction of the boulder he is holding against (engine push state).
    push_dir: Option<Direction>,
    /// Ticks in the current uninterrupted push.
    push_ticks: u32,
    /// Grab-button snap push window: (direction, ticks left).
    snap_push: Option<(Direction, u32)>,
}

impl RockfordAnim {
    pub fn new(cave: &Cave) -> RockfordAnim {
        let pos = cave.rockford_pos();
        RockfordAnim {
            pos: cell_xy(pos),
            from: cell_xy(pos),
            target: pos,
            step: 8,
            facing: Direction::Down,
            idle_ticks: 0,
            dead_ticks: 0,
            azimuth: 0.0,
            push_dir: None,
            push_ticks: 0,
            snap_push: None,
        }
    }

    /// Snap to Rockford's cell (cave load / respawn).
    pub fn reset(&mut self, cave: &Cave) {
        *self = RockfordAnim::new(cave);
    }

    /// Advance one engine tick.
    pub fn update(&mut self, cave: &Cave) {
        if !cave.rockford_alive() {
            self.dead_ticks += 1;
            return;
        }
        let pos = cave.rockford_pos();
        if pos != self.target {
            // New cell move: slide from wherever we are now.
            self.from = self.pos;
            self.target = pos;
            self.step = 0;
            self.idle_ticks = 0;
        }
        // Facing is carried in the Rockford cell's low-nibble direction bits.
        let cell = cave.cell_at_idx(pos);
        if cell.obj == crate::engine::Obj::Rockford {
            self.facing = cell.dir();
        }
        if self.step < 8 {
            self.step += 1;
            let (fx, fy) = self.from;
            let (tx, ty) = cell_xy(pos);
            let t = self.step as f32 / 8.0;
            self.pos = (fx + (tx - fx) * t, fy + (ty - fy) * t);
            if self.step == 8 {
                self.pos = (tx, ty);
            }
        } else {
            self.idle_ticks += 1;
        }

        // Boulder push wind-up reported by the engine (24-frame hold).
        self.push_dir = cave.push_state();
        if self.push_dir.is_some() {
            self.push_ticks += 1;
        } else {
            self.push_ticks = 0;
        }
        // Snap-push window (grab button): runs out on its own.
        match self.snap_push {
            Some((d, left)) if left > 1 => self.snap_push = Some((d, left - 1)),
            Some(_) => self.snap_push = None,
            None => {}
        }

        // Ease the visual facing: towards the push/travel direction while
        // active, back to the camera at rest. Easing over the nearest
        // azimuth row reads as a smooth turn.
        let target = if let Some(d) = self.push_dir {
            dir_azimuth(d)
        } else if let Some((d, _)) = self.snap_push {
            dir_azimuth(d)
        } else if self.walking() {
            dir_azimuth(self.facing)
        } else {
            0.0
        };
        let delta = wrap180(target - self.azimuth);
        self.azimuth = wrap180(self.azimuth + delta * 0.3);
        if delta.abs() < 0.5 {
            self.azimuth = target;
        }
    }

    pub fn moving(&self) -> bool {
        self.step < 8
    }

    /// Grab-button snap push reported by the engine (`Event::BoulderPushed`):
    /// turn towards the boulder and play one shove cycle.
    pub fn pushed(&mut self, dir: Direction) {
        if self.dead_ticks > 0 {
            return;
        }
        self.snap_push = Some((dir, SNAP_PUSH_TICKS));
        self.facing = dir;
    }

    /// Run-cycle state: sliding between cells, or just arrived with the walk
    /// counter still running (the ROM settles into idle the same way — walk
    /// frames persist a few ticks after the last step).
    pub fn walking(&self) -> bool {
        self.step < 8 || self.idle_ticks < WALK_HOLD_TICKS
    }

    /// Alive (not in the death arc and not gone).
    pub fn alive(&self) -> bool {
        self.dead_ticks == 0
    }

    /// Atlas row/frame to draw this tick, or `None` once the death arc has
    /// played out.
    pub fn robot_frame(&self, tick: u64) -> Option<RobotFrame> {
        if self.dead_ticks > 0 {
            if self.dead_ticks <= DEATH_ARC_TICKS {
                // Spin out and fade (the NES wobbled the head; we spin him).
                return Some(RobotFrame {
                    row: ROW_IDLE,
                    frame: 0,
                    flip: false,
                    rotation: self.dead_ticks as f32 * 0.22,
                    alpha: 1.0 - self.dead_ticks as f32 / DEATH_ARC_TICKS as f32,
                });
            }
            return None;
        }
        if let Some(d) = self.push_dir {
            // Leaning into the boulder; shove pulses on a 32-tick loop.
            return Some(RobotFrame {
                row: ROW_PUSH,
                frame: (self.push_ticks as usize / 2) % FRAMES,
                flip: d == Direction::Left,
                rotation: 0.0,
                alpha: 1.0,
            });
        }
        if let Some((d, left)) = self.snap_push {
            // Snap push: one quick shove across the window.
            let elapsed = (SNAP_PUSH_TICKS - left) as usize;
            return Some(RobotFrame {
                row: ROW_PUSH,
                frame: elapsed % FRAMES,
                flip: d == Direction::Left,
                rotation: 0.0,
                alpha: 1.0,
            });
        }
        if self.walking() {
            // Two footfalls per crossed cell: the 16-frame cycle runs at
            // half rate, synced to the 8-tick slide.
            return Some(RobotFrame {
                row: ROW_RUN + robot::RobotArt::az_row(self.azimuth),
                frame: (self.step as usize * 2) % FRAMES,
                flip: self.azimuth < 0.0,
                rotation: 0.0,
                alpha: 1.0,
            });
        }
        if self.idle_ticks >= FIDGET_START {
            let t = self.idle_ticks - FIDGET_START;
            if t % FIDGET_PERIOD < FIDGET_LEN {
                let row = [ROW_SCRATCH, ROW_LOOK, ROW_DANCE][(t / FIDGET_PERIOD) as usize % 3];
                return Some(RobotFrame {
                    row,
                    frame: ((t % FIDGET_PERIOD) as usize / 2) % FRAMES,
                    flip: false,
                    rotation: 0.0,
                    alpha: 1.0,
                });
            }
        }
        // Standing straight, facing (back to) the camera, tapping a foot on
        // the beat (one loop = 48 ticks = 0.8 s -> 150 bpm).
        Some(RobotFrame {
            row: ROW_IDLE + robot::RobotArt::az_row(self.azimuth),
            frame: (tick as usize / 3) % FRAMES,
            flip: self.azimuth < 0.0,
            rotation: 0.0,
            alpha: 1.0,
        })
    }
}

fn cell_xy(i: usize) -> (f32, f32) {
    ((i % WIDTH) as f32, (i / WIDTH) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ascii::{ascii_cave, ascii_params};
    use crate::engine::Input;
    use crate::render::robot::{ROW_DANCE, ROW_IDLE, ROW_LOOK, ROW_PUSH, ROW_RUN, ROW_SCRATCH};

    fn dir(d: Direction) -> Input {
        let mut input = Input::NONE;
        match d {
            Direction::Up => input.up = true,
            Direction::Down => input.down = true,
            Direction::Left => input.left = true,
            Direction::Right => input.right = true,
        }
        input
    }

    fn frames(cave: &mut Cave, anim: &mut RockfordAnim, n: usize, input: Input) {
        for _ in 0..n {
            cave.tick(input);
            anim.update(cave);
        }
    }

    /// Moving right shows the right-facing (unflipped) run rows, left flips
    /// them, up shows the back row — never the mirrored-opposite side.
    #[test]
    fn run_faces_travel_direction() {
        let mut cave = ascii_cave(&["      r      "], ascii_params());
        let mut anim = RockfordAnim::new(&cave);

        frames(&mut cave, &mut anim, 20, dir(Direction::Right));
        let rf = anim.robot_frame(cave.frame()).expect("frame");
        assert!(rf.row >= ROW_RUN && rf.row < ROW_RUN + 5, "run row expected");
        assert!(!rf.flip, "moving right must use unflipped rows");
        assert_eq!(anim.azimuth, 90.0);

        frames(&mut cave, &mut anim, 60, dir(Direction::Left));
        let rf = anim.robot_frame(cave.frame()).expect("frame");
        assert!(rf.row >= ROW_RUN && rf.row < ROW_RUN + 5, "run row expected");
        assert!(rf.flip, "moving left must flip the right-facing rows");
        assert_eq!(anim.azimuth, -90.0);

        // Vertical travel needs headroom: fresh tall cave.
        let mut cave = ascii_cave(&[" ", " ", " ", " ", " ", " ", "r"], ascii_params());
        let mut anim = RockfordAnim::new(&cave);
        frames(&mut cave, &mut anim, 40, dir(Direction::Up));
        let rf = anim.robot_frame(cave.frame()).expect("frame");
        assert_eq!(rf.row, ROW_RUN + 4, "moving up shows the back row");
    }

    /// When movement stops, the visual facing eases back to the camera and
    /// the idle rows take over (the user-facing "stand straight" rule).
    #[test]
    fn idle_returns_to_face_camera() {
        let mut cave = ascii_cave(&["   ", " r ", "   "], ascii_params());
        let mut anim = RockfordAnim::new(&cave);
        frames(&mut cave, &mut anim, 20, dir(Direction::Right));
        frames(&mut cave, &mut anim, 40, Input::NONE);

        let rf = anim.robot_frame(cave.frame()).expect("frame");
        assert_eq!(anim.azimuth, 0.0);
        assert!(rf.row >= ROW_IDLE && rf.row < ROW_IDLE + 5, "idle row expected");
        assert_eq!(rf.rotation, 0.0);
    }

    /// Holding against a boulder plays the braced push loop.
    #[test]
    fn push_uses_push_row() {
        let mut cave = ascii_cave(&["ro  ", "++++"], ascii_params());
        let mut anim = RockfordAnim::new(&cave);
        frames(&mut cave, &mut anim, 5, dir(Direction::Right));

        let rf = anim.robot_frame(cave.frame()).expect("frame");
        assert_eq!(rf.row, ROW_PUSH);
        assert!(!rf.flip, "pushing right uses the unflipped push row");

        // Mirror side: push left.
        let mut cave = ascii_cave(&["  or", "++++"], ascii_params());
        let mut anim = RockfordAnim::new(&cave);
        frames(&mut cave, &mut anim, 5, dir(Direction::Left));
        let rf = anim.robot_frame(cave.frame()).expect("frame");
        assert_eq!(rf.row, ROW_PUSH);
        assert!(rf.flip, "pushing left flips the push row");
    }

    /// Grab-button snap push: turns towards the boulder and plays one shove
    /// even though the engine never enters the 24-frame push state.
    #[test]
    fn snap_push_turns_and_shoves() {
        let mut cave = ascii_cave(&["ro  ", "++++"], ascii_params());
        let mut anim = RockfordAnim::new(&cave);
        let mut grab_r = dir(Direction::Right);
        grab_r.grab = true;
        frames(&mut cave, &mut anim, 1, grab_r);
        anim.pushed(Direction::Right);
        anim.update(&cave);

        let rf = anim.robot_frame(cave.frame()).expect("frame");
        assert_eq!(rf.row, ROW_PUSH, "snap push must show the push row");
        assert!(!rf.flip, "snap push right is unflipped");
        // Facing syncs from the Rockford cell each update, so give the ease a
        // few ticks before checking the sign.
        anim.update(&cave);
        anim.update(&cave);
        assert!(anim.azimuth > 0.0, "azimuth eases towards the push side, got {}", anim.azimuth);

        // The window ends and he settles back to idle.
        frames(&mut cave, &mut anim, 24, Input::NONE);
        let rf = anim.robot_frame(cave.frame()).expect("frame");
        assert_ne!(rf.row, ROW_PUSH, "push pose must not stick");
    }

    /// No idle flicker at the 8-tick cell boundary while walking on.
    #[test]
    fn run_does_not_flicker_between_cells() {
        let mut cave = ascii_cave(&[" r      "], ascii_params());
        let mut anim = RockfordAnim::new(&cave);
        for i in 0..32 {
            frames(&mut cave, &mut anim, 1, dir(Direction::Right));
            let rf = anim.robot_frame(cave.frame()).expect("frame");
            assert!(
                rf.row >= ROW_RUN && rf.row < ROW_RUN + 5,
                "tick {i}: run row expected, got {}",
                rf.row
            );
        }
    }

    /// Long idling cycles through all three fidgets (scratch, look, dance).
    #[test]
    fn idle_fidgets_cycle_all_emotions() {
        let mut cave = ascii_cave(&["      r      "], ascii_params());
        let mut anim = RockfordAnim::new(&cave);
        frames(&mut cave, &mut anim, 20, dir(Direction::Right));
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..1250 {
            frames(&mut cave, &mut anim, 1, Input::NONE);
            let rf = anim.robot_frame(cave.frame()).expect("frame");
            if rf.row >= ROW_SCRATCH {
                seen.insert(rf.row);
            }
        }
        assert!(seen.contains(&ROW_SCRATCH), "scratch never played");
        assert!(seen.contains(&ROW_LOOK), "look never played");
        assert!(seen.contains(&ROW_DANCE), "dance never played");
    }
}
