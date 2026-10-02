//! 256x240 NES-style renderer (metatiles, camera, HUD, Rockford sprites).
//!
//! Rendering model, reconciled with the Ghidra findings and the ROM metatile
//! table ($F383):
//! - Objects 0-7 (space, vacated, mud, steel, door, brick, magic, boulder)
//!   are STATIC: their quad is selected per world group (`cave / 4`, i.e. the
//!   ROM's `(cave>>2)*4` variant offset in `metatile_variant_select` $B586),
//!   not animated. The 6/9 quad "frames" in `METATILE_SEQS` rows 0-7 are the
//!   per-world art variants; quads 6-8 of the shared wall record are the
//!   magic-wall-active ($F3EB) and open-door ($F3F3) quads.
//! - Firefly/butterfly (0xB/0xC) animate by cycling their 6 quads on the
//!   global frame counter.
//! - Diamond/pending/explosion/amoeba (8/9/A/D/F) have a single quad; the
//!   original sparkles them via CHR bank switching — we approximate with a
//!   palette flash.
//! - Rockford is two parts (Ghidra Q7): his body is background metatile 15,
//!   drawn in the cell loop from the animation banks like any other object;
//!   his head is a 2-sprite 16x8 overlay (see `rockford` module).

pub mod atlas;
pub mod camera;
pub mod hud;
pub mod nametable;
pub mod rockford;

use macroquad::prelude::*;

use crate::data::sprites::ROCKFORD_BODY_QUAD;
use crate::data::tiles::{METATILE_ATTRS, METATILE_SEQS};
use crate::engine::{Cave, Obj, HEIGHT, WIDTH};

use atlas::Atlas;
use camera::{Camera, CELL_PX, VIEW_H, VIEW_W};
use hud::HUD_H;
use nametable::Screens;
use rockford::{RockfordAnim, RockfordArt};

pub struct Renderer {
    pub atlas: Atlas,
    /// Pre-rendered decoded screen nametables (title/map/password/...).
    pub screens: Screens,
    rockford_art: RockfordArt,
}

/// CHR bank holding the cave's world art: banks 0-3 for worlds 1-4,
/// bank 6 for worlds 5/6 (caves 16-23). Cave DATA for caves 16-23 lives in
/// bank 7, but its object art is in bank 6.
pub fn world_bank(cave_idx: usize) -> usize {
    if cave_idx >= 16 { 6 } else { (cave_idx / 4).min(3) }
}

/// World group 0-5, the static-object metatile variant index.
fn variant(cave_idx: usize) -> usize {
    cave_idx / 4
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer {
            atlas: Atlas::new(),
            screens: Screens::new(),
            rockford_art: RockfordArt::new(),
        }
    }

    /// Rockford head-sprite sheet (columns = head tiles, rows = suit colors).
    pub fn rockford_art(&self) -> &RockfordArt {
        &self.rockford_art
    }

    fn quad_of(&self, obj: Obj) -> [u8; 4] {
        let row = &METATILE_SEQS[obj as usize];
        row[0..4].try_into().unwrap()
    }

    fn seq_quad(&self, obj: Obj, entry: usize) -> [u8; 4] {
        METATILE_SEQS[obj as usize][entry * 4..entry * 4 + 4]
            .try_into()
            .unwrap()
    }

    /// One cave cell: pick the quad + palette and draw it. Palettes follow
    /// the ROM's per-object attribute table ($F453); flash effects override.
    #[allow(clippy::too_many_arguments)]
    fn draw_cell(
        &self,
        obj: Obj,
        cave_idx: usize,
        bank: usize,
        x: f32,
        y: f32,
        frame: u64,
        door_open: bool,
        magic_active: bool,
    ) {
        let var = variant(cave_idx);
        let attr = METATILE_ATTRS[obj as usize] as usize;
        let (quad, pal): ([u8; 4], usize) = match obj {
            Obj::Space | Obj::Vacated | Obj::Mud | Obj::Steel | Obj::Brick | Obj::Boulder => {
                (self.seq_quad(obj, var), atlas::world_pal(var, attr))
            }
            Obj::Door => {
                if door_open {
                    // Open-door quad ($F3F3 = record quad 8), flashing bright.
                    let pal = if (frame / 8).is_multiple_of(2) {
                        atlas::SPARKLE_PAL
                    } else {
                        atlas::world_pal(var, attr)
                    };
                    (self.seq_quad(obj, 8), pal)
                } else {
                    (self.seq_quad(obj, var), atlas::world_pal(var, attr))
                }
            }
            Obj::MagicWall => {
                if magic_active {
                    // Active magic wall quad ($F3EB = record quad 6), flashing.
                    let pal = if (frame / 4).is_multiple_of(2) {
                        atlas::SPARKLE_PAL
                    } else {
                        atlas::world_pal(var, attr)
                    };
                    (self.seq_quad(obj, 6), pal)
                } else {
                    (self.seq_quad(obj, var), atlas::world_pal(var, attr))
                }
            }
            Obj::Diamond | Obj::PendingDiamond => {
                // Classic blue diamonds with a white sparkle flash.
                let pal = if (frame / 16).is_multiple_of(2) {
                    atlas::DIAMOND_PAL
                } else {
                    atlas::SPARKLE_PAL
                };
                (self.quad_of(Obj::Diamond), pal)
            }
            Obj::ExplosionRemnant => {
                let pal = if (frame / 4).is_multiple_of(2) {
                    atlas::SPARKLE_PAL
                } else {
                    atlas::world_pal(var, attr)
                };
                (self.quad_of(Obj::ExplosionRemnant), pal)
            }
            Obj::Firefly | Obj::Butterfly => {
                let f = ((frame / 8) % 6) as usize;
                (self.seq_quad(obj, f), atlas::world_pal(var, attr))
            }
            Obj::Amoeba | Obj::DeadAmoeba => (self.quad_of(Obj::Amoeba), atlas::world_pal(var, attr)),
            Obj::Rockford => {
                // Body = background metatile 15 (quad $F44F), BG palette 0.
                // The original animates it via the CHR1 bank swap
                // ($6F = ($FE&$18)>>3): banks 0-3 hold the 4 walk frames.
                // This applies to every cave — bank 6's tiles $36-$39 are
                // unrelated worlds-5/6 art, so the body never comes from
                // the cave's `bank` when that is 6.
                let bbank = ((frame / 8) % 4) as usize;
                self.atlas.draw_quad_scaled(ROCKFORD_BODY_QUAD, bbank, atlas::world_pal(var, 0), x, y, CELL_PX / 16.0);
                return;
            }
        };
        self.atlas.draw_quad_scaled(quad, bank, pal, x, y, CELL_PX / 16.0);
    }

    /// Cave field + Rockford's head overlay, clipped to the viewport under
    /// the HUD. `suit` is the player's color-table index (sprite palette 0
    /// color 2, $3D in the ROM).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_world(
        &self,
        cave: &Cave,
        cave_idx: usize,
        cam: &Camera,
        frame: u64,
        magic_active: bool,
        rock: &RockfordAnim,
        suit: usize,
    ) {
        let bank = world_bank(cave_idx);
        let door_open = cave.door_open();
        let x0 = (cam.x / CELL_PX).floor().max(0.0) as usize;
        let x1 = ((cam.x + VIEW_W) / CELL_PX).ceil().min(WIDTH as f32) as usize;
        let y0 = (cam.y / CELL_PX).floor().max(0.0) as usize;
        let y1 = ((cam.y + VIEW_H) / CELL_PX).ceil().min(HEIGHT as f32) as usize;
        for cy in y0..y1 {
            for cx in x0..x1 {
                let sx = cx as f32 * CELL_PX - cam.x;
                let sy = HUD_H + cy as f32 * CELL_PX - cam.y;
                let cell = cave.cell_at(cx, cy);
                self.draw_cell(
                    cell.obj,
                    cave_idx,
                    bank,
                    sx,
                    sy,
                    frame,
                    door_open,
                    magic_active,
                );
            }
        }
        if let Some(head) = rock.head_frame(frame) {
            let sx = rock.pos.0 * CELL_PX - cam.x;
            let sy = HUD_H + rock.pos.1 * CELL_PX - cam.y;
            self.rockford_art.draw_head(head, suit, sx, sy, CELL_PX / 16.0);
        }
    }

    pub fn draw_hud(&self, cave: &Cave, cave_idx: usize, level: u8, frame: u64) {
        hud::draw_hud(&self.atlas, cave, cave_idx, level, world_bank(cave_idx), frame);
    }

    /// Centered overlay text inside the cave viewport (messages, pause).
    pub fn draw_overlay(&self, lines: &[&str], frame: u64) {
        let blink = (frame / 24).is_multiple_of(2);
        let h = lines.len() as f32 * 12.0;
        let y0 = HUD_H + (VIEW_H - h) / 2.0;
        draw_rectangle(24.0, y0 - 6.0, VIEW_W - 48.0, h + 4.0, Color::new(0.0, 0.0, 0.0, 0.75));
        for (i, line) in lines.iter().enumerate() {
            let text = line.trim_start_matches('!');
            if !blink && line.starts_with('!') {
                continue; // flashing line
            }
            let w = text.chars().count() as f32 * 8.0;
            hud::draw_text(&self.atlas, (VIEW_W - w) / 2.0, y0 + i as f32 * 12.0, text);
        }
    }
}
