//! 256x240 NES-style renderer (metatiles, camera, HUD, Rockford sprites).
//!
//! Rendering model, reconciled with the Ghidra findings and the ROM metatile
//! table ($F383):
//! - Objects 0-7 (space, vacated, mud, steel, door, brick, magic, boulder)
//!   are STATIC: their quad is selected per world group (`cave / 4`, i.e. the
//!   ROM's `(cave>>2)*4` variant offset in `metatile_variant_select` $B586),
//!   not animated. The 6/9 quad "frames" in `METATILE_SEQS` rows 0-7 are the
//!   per-world art variants; quads 6-8 of the shared wall record are the
//!   magic-wall-active ($F3EB) and open-door ($F3F3) quads. EXCEPTION: the
//!   boulder is drawn from the HD Blender sprite atlas (`boulder` module)
//!   with per-cell variants and pre-rendered spin frames, not from CHR quads.
//! - Firefly/butterfly (0xB/0xC) animate by cycling their 6 quads on the
//!   global frame counter.
//! - Diamond/pending/explosion/amoeba (8/9/A/D/F) have a single quad; the
//!   original sparkles them via CHR bank switching — we approximate with a
//!   palette flash. EXCEPTION: diamonds are drawn from the HD Blender
//!   sparkle atlas (`diamond` module) — a path-traced gem with an orbiting
//!   light rig, the counterpart of the palette shimmer.
//! - Rockford is two parts (Ghidra Q7): his body is background metatile 15,
//!   drawn in the cell loop from the animation banks like any other object;
//!   his head is a 2-sprite 16x8 overlay (see `rockford` module).

pub mod atlas;
pub mod boulder;
pub mod camera;
pub mod diamond;
pub mod hud;
pub mod nametable;
pub mod rockford;
pub mod shadow;
pub mod slide;
pub mod wall;

use macroquad::prelude::*;

use crate::data::sprites::ROCKFORD_BODY_QUAD;
use crate::data::tiles::{METATILE_ATTRS, METATILE_SEQS};
use crate::engine::{Cave, Obj, HEIGHT, WIDTH};

use atlas::Atlas;
use boulder::BoulderArt;
use camera::{Camera, CELL_PX, VIEW_H, VIEW_W};
use diamond::DiamondArt;
use hud::HUD_H;
use nametable::Screens;
use rockford::{RockfordAnim, RockfordArt};
use shadow::WallShadow;
use wall::WallArt;

pub struct Renderer {
    pub atlas: Atlas,
    /// Pre-rendered decoded screen nametables (title/map/password/...).
    pub screens: Screens,
    rockford_art: RockfordArt,
    boulder_art: BoulderArt,
    diamond_art: DiamondArt,
    wall_art: WallArt,
    wall_shadow: WallShadow,
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

/// Wall-contact shadow flags for a cell: walls cast onto their right and
/// bottom neighbors (the light is upper-left), so a cell shaded from the
/// left and/or top. A diagonal wall with no edge walls gives a corner
/// touch on both sides. Walls themselves cast but never receive.
fn wall_shadow_sides(cave: &Cave, cx: usize, cy: usize) -> (bool, bool) {
    let wall = |x: usize, y: usize| matches!(cave.cell_at(x, y).obj, Obj::Steel | Obj::Brick);
    if wall(cx, cy) {
        return (false, false);
    }
    let left = cx > 0 && wall(cx - 1, cy);
    let top = cy > 0 && wall(cx, cy - 1);
    let diag = !left && !top && cx > 0 && cy > 0 && wall(cx - 1, cy - 1);
    (left || diag, top || diag)
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
            boulder_art: BoulderArt::new(),
            diamond_art: DiamondArt::new(),
            wall_art: WallArt::new(),
            wall_shadow: WallShadow::new(),
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
    /// Boulders and diamonds bypass the atlas: HD Blender sprites, per-cell
    /// variant / sparkle frame. `backdrop` (the world's Space art) is drawn
    /// UNDER the HD sprite so the background shows around it instead of a
    /// black void.
    #[allow(clippy::too_many_arguments)]
    fn draw_cell(
        &self,
        obj: Obj,
        cell_idx: usize,
        spin: boulder::Spin,
        backdrop: Obj,
        cave_idx: usize,
        bank: usize,
        x: f32,
        y: f32,
        frame: u64,
        door_open: bool,
        magic_active: bool,
    ) {
        if obj == Obj::Boulder {
            self.draw_backdrop(backdrop, cave_idx, bank, x, y, frame, door_open, magic_active);
            self.boulder_art.draw(cell_idx, spin, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Diamond || obj == Obj::PendingDiamond {
            self.draw_backdrop(backdrop, cave_idx, bank, x, y, frame, door_open, magic_active);
            self.diamond_art.draw(frame, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Steel {
            self.wall_art.draw(cell_idx, x, y, CELL_PX);
            return;
        }
        let (quad, pal) = self.cell_quad_pal(obj, cave_idx, frame, door_open, magic_active);
        self.atlas.draw_quad_scaled(quad, bank, pal, x, y, CELL_PX / 16.0);
    }

    /// The backdrop quad (Mud/Space of this world's variant), drawn under
    /// HD item sprites.
    #[allow(clippy::too_many_arguments)]
    fn draw_backdrop(&self, obj: Obj, cave_idx: usize, bank: usize, x: f32, y: f32, frame: u64, door_open: bool, magic_active: bool) {
        let (quad, pal) = self.cell_quad_pal(obj, cave_idx, frame, door_open, magic_active);
        self.atlas.draw_quad_scaled(quad, bank, pal, x, y, CELL_PX / 16.0);
    }

    /// Same quad/palette selection as `draw_cell`, but drawn with a
    /// TRANSPARENT background — for objects sliding between cells, whose
    /// opaque backdrop pixels would otherwise erase the cells they pass over.
    #[allow(clippy::too_many_arguments)]
    fn draw_cell_sliding(
        &self,
        obj: Obj,
        cell_idx: usize,
        spin: boulder::Spin,
        cave_idx: usize,
        bank: usize,
        x: f32,
        y: f32,
        frame: u64,
        door_open: bool,
        magic_active: bool,
    ) {
        if obj == Obj::Boulder {
            self.boulder_art.draw(cell_idx, spin, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Diamond || obj == Obj::PendingDiamond {
            self.diamond_art.draw(frame, x, y, CELL_PX);
            return;
        }
        let (quad, pal) = self.cell_quad_pal(obj, cave_idx, frame, door_open, magic_active);
        let scale = CELL_PX / 16.0;
        draw_texture_ex(
            &self.atlas.quad_texture(quad, bank, pal),
            x,
            y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(16.0 * scale, 16.0 * scale)),
                ..Default::default()
            },
        );
    }

    /// The metatile quad + palette row for a cell object (per world variant,
    /// animations and flash effects; see the ROM's $F453 attribute table).
    fn cell_quad_pal(
        &self,
        obj: Obj,
        cave_idx: usize,
        frame: u64,
        door_open: bool,
        magic_active: bool,
    ) -> ([u8; 4], usize) {
        let var = variant(cave_idx);
        let attr = METATILE_ATTRS[obj as usize] as usize;
        match obj {
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
                // Blue diamonds with a glint traveling across the facets.
                (self.quad_of(Obj::Diamond), atlas::diamond_pal(frame))
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
                // Rockford is drawn AFTER the cell loop (body + head together
                // at his fractional slide position); leave clean space here.
                (
                    self.seq_quad(Obj::Space, var),
                    atlas::world_pal(var, METATILE_ATTRS[Obj::Space as usize] as usize),
                )
            }
        }
    }

    /// Cave field + Rockford's head overlay, clipped to the viewport under
    /// the HUD. `suit` is the player's color-table index (sprite palette 0
    /// color 2, $3D in the ROM). Objects with an active slide in `slides`
    /// are drawn at their interpolated position.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_world(
        &self,
        cave: &Cave,
        cave_idx: usize,
        cam: &Camera,
        frame: u64,
        magic_active: bool,
        rock: &RockfordAnim,
        slides: &slide::SlideTracker,
        suit: usize,
    ) {
        let bank = world_bank(cave_idx);
        let door_open = cave.door_open();
        let x0 = (cam.x / CELL_PX).floor().max(0.0) as usize;
        let x1 = ((cam.x + VIEW_W) / CELL_PX).ceil().min(WIDTH as f32) as usize;
        let y0 = (cam.y / CELL_PX).floor().max(0.0) as usize;
        let y1 = ((cam.y + VIEW_H) / CELL_PX).ceil().min(HEIGHT as f32) as usize;
        // Sliding objects are drawn AFTER the whole cell loop: their quads
        // overhang into the source cell, which would otherwise be drawn later
        // and clip them (visible on leftward slides).
        let mut sliding = Vec::new();
        for cy in y0..y1 {
            for cx in x0..x1 {
                let cell = cave.cell_at(cx, cy);
                let idx = cy * WIDTH + cx;
                let sx_c = cx as f32 * CELL_PX - cam.x;
                let sy_c = HUD_H + cy as f32 * CELL_PX - cam.y;
                match slides.slide_for(idx, frame) {
                    Some((obj, fx, fy)) => {
                        // Draw the space the object passes through FIRST —
                        // the sliding quad's transparent pixels must show the
                        // texture underneath, not the clear color.
                        self.draw_cell(
                            Obj::Space,
                            idx,
                            boulder::Spin::default(),
                            Obj::Space,
                            cave_idx,
                            bank,
                            sx_c,
                            sy_c,
                            frame,
                            door_open,
                            magic_active,
                        );
                        let sx = fx * CELL_PX - cam.x;
                        let sy = HUD_H + fy * CELL_PX - cam.y;
                        sliding.push((obj, idx, slides.spin_for(idx, frame), sx, sy));
                    }
                    None => {
                        // HD items sit on the world's BACKDROP (Space art),
                        // never on dirt: the background shows around the
                        // sprite instead of a black void or a mud patch.
                        self.draw_cell(
                            cell.obj,
                            idx,
                            slides.rest_spin(idx),
                            Obj::Space,
                            cave_idx,
                            bank,
                            sx_c,
                            sy_c,
                            frame,
                            door_open,
                            magic_active,
                        )
                    }
                }
                let (shade_left, shade_top) = wall_shadow_sides(cave, cx, cy);
                self.wall_shadow.draw(shade_left, shade_top, sx_c, sy_c, CELL_PX);
            }
        }
        for (obj, idx, spin, sx, sy) in sliding {
            self.draw_cell_sliding(obj, idx, spin, cave_idx, bank, sx, sy, frame, door_open, magic_active);
        }
        if let Some(head) = rock.head_frame(frame) {
            // Body + head TOGETHER at the fractional slide position (the NES
            // had to jump the background body cell-wise; we don't).
            let sx = rock.pos.0 * CELL_PX - cam.x;
            let sy = HUD_H + rock.pos.1 * CELL_PX - cam.y;
            let scale = CELL_PX / 16.0;
            if rock.alive() {
                // Body = background metatile 15 (quad $F44F), world palette 0,
                // animated via the CHR1 bank swap frames (banks 0-3).
                // Transparent background: it slides over other cells.
                let bbank = ((frame / 8) % 4) as usize;
                let scale = CELL_PX / 16.0;
                draw_texture_ex(
                    &self.atlas.quad_texture(
                        ROCKFORD_BODY_QUAD,
                        bbank,
                        atlas::world_pal(variant(cave_idx), 0),
                    ),
                    sx,
                    sy,
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(16.0 * scale, 16.0 * scale)),
                        ..Default::default()
                    },
                );
            }
            self.rockford_art.draw_head(head, suit, sx, sy, scale);
        }
    }

    pub fn draw_hud(&self, cave: &Cave, cave_idx: usize, level: u8, frame: u64) {
        hud::draw_hud(&self.atlas, cave, cave_idx, level, world_bank(cave_idx), frame);
    }

    /// Full-frame white flash for the door-open effect (15 ticks: 10 at
    /// full brightness, then a 5-tick fade).
    pub fn draw_door_flash(&self, frames_left: u8) {
        let alpha = 0.85 * (frames_left.min(5) as f32) / 5.0;
        draw_rectangle(
            0.0,
            0.0,
            camera::VIEW_W,
            hud::HUD_H + camera::VIEW_H,
            Color::new(1.0, 1.0, 1.0, alpha),
        );
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
