//! Rockford rendering (Ghidra Q7; data extracted to `data::sprites`).
//!
//! The original draws Rockford in two parts:
//! - **Body**: background metatile 15 ([`ROCKFORD_BODY_QUAD`]), drawn by the
//!   cave renderer at his cell like any other object, from CHR1 banks 0-3 —
//!   the per-frame bank swap $6F = ($FE&$18)>>3 supplies the 4 walk frames
//!   (bank 6 for caves 16-23). Palette: BG group 0 ($F453 attr for $E0 = 0).
//! - **Head**: a 2-sprite 16x8 OAM overlay from CHR bank 4 (sprite pattern
//!   table 0, sprite palette 0), sliding smoothly with his pixel position.
//!   Sprite palette color 2 is the suit color chosen on the color-select
//!   screen ($3D = $A769[$88]).
//!
//! This module owns the head overlay: a texture sheet with one tile column
//! per [`ROCKFORD_HEAD_TILES`] entry and one row per `ROCKFORD_COLORS` suit
//! color, plus the animation state (cell-to-cell slide, facing, walk cycle,
//! idle blink, death wobble). The body is drawn by `render::Renderer` in the
//! cell loop.

use macroquad::prelude::*;

use crate::data::cave_params::{NES_PALETTE, ROCKFORD_COLORS};
use crate::data::sprites::{
    HeadFrame, ROCKFORD_HEAD_DEATH, ROCKFORD_HEAD_IDLE, ROCKFORD_HEAD_TILES, ROCKFORD_HEAD_WALK_DOWN,
    ROCKFORD_HEAD_WALK_LEFT, ROCKFORD_HEAD_WALK_RIGHT, ROCKFORD_HEAD_WALK_UP,
};
use crate::data::tiles::TILES;
use crate::engine::{Cave, Direction};
use crate::engine::WIDTH;

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

/// Death head wobble lasts this many ticks (the ROM's 80-frame arc, $92=$50),
/// then the head is gone until respawn.
const DEATH_HEAD_TICKS: u32 = 80;
/// Ticks standing still before settling into the front-facing idle blink
/// (the ROM switches when the walk counter $9C runs out).
const IDLE_SETTLE_TICKS: u32 = 24;

/// Render-side Rockford animation state: smooth cell-to-cell sliding (the
/// engine moves him one cell per 8 ticks; the original slides 2 px/frame)
/// plus facing, walk cycle, idle blink and the death wobble.
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
    }

    pub fn moving(&self) -> bool {
        self.step < 8
    }

    /// Head metasprite to draw this tick (frame-select formulas match the
    /// ROM's $FE-based indexes), or `None` once the death arc has played out.
    pub fn head_frame(&self, tick: u64) -> Option<&'static HeadFrame> {
        if self.dead_ticks > 0 {
            if self.dead_ticks <= DEATH_HEAD_TICKS {
                return Some(&ROCKFORD_HEAD_DEATH[((tick / 16) % 4) as usize]);
            }
            return None;
        }
        if !self.moving() && self.idle_ticks >= IDLE_SETTLE_TICKS {
            return Some(&ROCKFORD_HEAD_IDLE[((tick / 32) % 4) as usize]);
        }
        let f = ((tick / 8) % 4) as usize;
        Some(match self.facing {
            Direction::Up => &ROCKFORD_HEAD_WALK_UP[f],
            Direction::Down => &ROCKFORD_HEAD_WALK_DOWN[f],
            Direction::Left => &ROCKFORD_HEAD_WALK_LEFT[f],
            Direction::Right => &ROCKFORD_HEAD_WALK_RIGHT[f],
        })
    }
}

fn cell_xy(i: usize) -> (f32, f32) {
    ((i % WIDTH) as f32, (i / WIDTH) as f32)
}
