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
pub mod backdrop;
pub mod boulder;
pub mod camera;
pub mod diamond;
pub mod door;
pub mod glow;
pub mod hud;
pub mod mud;
pub mod nametable;
pub mod robot;
pub mod rockford;
pub mod shadow;
pub mod slide;
pub mod wall;

use macroquad::prelude::*;

use crate::data::sprites::ROCKFORD_BODY_QUAD;
use crate::data::tiles::{METATILE_ATTRS, METATILE_SEQS};
use crate::engine::{Cave, Obj, HEIGHT, WIDTH};

use atlas::Atlas;
use backdrop::BackdropArt;
use boulder::BoulderArt;
use camera::{Camera, CELL_PX, VIEW_H, VIEW_W};
use diamond::DiamondArt;
use door::DoorArt;
use glow::DiamondGlow;
use hud::HUD_H;
use mud::MudArt;
use nametable::Screens;
use robot::RobotArt;
use rockford::{RockfordAnim, RockfordArt};
use shadow::WallShadow;
use wall::WallArt;

/// Per-item art toggles (the F1 menu): true = HD Blender sprite, false =
/// original NES metatile. Cells so the main loop can flip them through the
/// shared &Renderer.
pub struct HdToggles {
    /// Rockford: HD robot vs NES body metatile + head overlay.
    pub rockford: std::cell::Cell<bool>,
    /// Boulders: HD spin atlas vs NES metatile (incl. their blob shadows).
    pub boulder: std::cell::Cell<bool>,
    /// Diamonds: HD sparkle sprite + glow halo vs NES glint metatile.
    pub diamond: std::cell::Cell<bool>,
    /// Exit door: HD stargate + shader horizon vs NES door metatile.
    pub door: std::cell::Cell<bool>,
    /// Steel/brick walls: HD masonry + contact shadows vs NES metatiles.
    pub wall: std::cell::Cell<bool>,
    /// Dirt: HD panorama vs NES metatile.
    pub mud: std::cell::Cell<bool>,
    /// Cave backdrop (space): HD rock texture vs NES space metatile.
    pub backdrop: std::cell::Cell<bool>,
}

impl Default for HdToggles {
    fn default() -> Self {
        Self {
            rockford: std::cell::Cell::new(true),
            boulder: std::cell::Cell::new(true),
            diamond: std::cell::Cell::new(true),
            door: std::cell::Cell::new(true),
            wall: std::cell::Cell::new(true),
            mud: std::cell::Cell::new(true),
            backdrop: std::cell::Cell::new(true),
        }
    }
}

impl HdToggles {
    /// Menu rows: (label, hd-on) per item.
    pub fn rows(&self) -> [(&'static str, bool); 7] {
        [
            ("ROCKFORD", self.rockford.get()),
            ("BOULDERS", self.boulder.get()),
            ("DIAMONDS", self.diamond.get()),
            ("DOOR", self.door.get()),
            ("WALLS", self.wall.get()),
            ("MUD", self.mud.get()),
            ("BACKDROP", self.backdrop.get()),
        ]
    }

    /// Flip one menu row.
    pub fn toggle(&self, row: usize) {
        let cells = [
            &self.rockford,
            &self.boulder,
            &self.diamond,
            &self.door,
            &self.wall,
            &self.mud,
            &self.backdrop,
        ];
        if let Some(c) = cells.get(row) {
            c.set(!c.get());
        }
    }

    /// Set a toggle by its row label (BDNES env debug helper).
    pub fn set_by_name(&self, name: &str, hd: bool) {
        let idx = ["ROCKFORD", "BOULDERS", "DIAMONDS", "DOOR", "WALLS", "MUD", "BACKDROP"]
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name));
        if let Some(i) = idx {
            let cells = [
                &self.rockford,
                &self.boulder,
                &self.diamond,
                &self.door,
                &self.wall,
                &self.mud,
                &self.backdrop,
            ];
            cells[i].set(hd);
        }
    }
}

pub struct Renderer {
    pub atlas: Atlas,
    /// Pre-rendered decoded screen nametables (title/map/password/...).
    pub screens: Screens,
    rockford_art: RockfordArt,
    robot_art: RobotArt,
    boulder_art: BoulderArt,
    diamond_art: DiamondArt,
    door_art: DoorArt,
    wall_art: WallArt,
    wall_shadow: WallShadow,
    diamond_glow: DiamondGlow,
    backdrop_art: BackdropArt,
    mud_art: MudArt,
    /// Current dirt panorama style (F cycles; Cell so the main loop can
    /// switch it through the shared &Renderer).
    pub mud_variant: std::cell::Cell<usize>,
    /// HD/NES art toggles (F1 menu).
    pub hd: HdToggles,
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
/// bottom neighbors (the light is upper-left). Dirt casts too, but only
/// onto the LOWER backdrop layer (non-mud cells): the soil edge reads as
/// a raised bank shading the ground behind it, while dirt-on-dirt stays
/// even. Walls themselves cast but never receive.
/// Returns (left edge shaded, top edge shaded, diagonal corner touch).
fn wall_shadow_sides(cave: &Cave, cx: usize, cy: usize) -> (bool, bool, bool) {
    let this = cave.cell_at(cx, cy).obj;
    if matches!(this, Obj::Steel | Obj::Brick) {
        return (false, false, false);
    }
    let onto_mud = this == Obj::Mud;
    let casts = |obj: Obj| matches!(obj, Obj::Steel | Obj::Brick) || (obj == Obj::Mud && !onto_mud);
    let left = cx > 0 && casts(cave.cell_at(cx - 1, cy).obj);
    let top = cy > 0 && casts(cave.cell_at(cx, cy - 1).obj);
    let diag = !left && !top && cx > 0 && cy > 0 && casts(cave.cell_at(cx - 1, cy - 1).obj);
    (left, top, diag)
}

/// Smooth spatial modulation of the shadow strength (0.7..1.0): breaks
/// the ruler-straight uniformity of long wall-edge shadow bands without
/// salt-and-pepper noise (the field is continuous between cells).
fn shadow_variation(cx: usize, cy: usize) -> f32 {
    let n = (cx as f32 * 1.31 + cy as f32 * 2.17).sin() * (cx as f32 * 2.73 - cy as f32 * 0.77).cos();
    0.85 + 0.15 * n
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
            robot_art: RobotArt::new(),
            boulder_art: BoulderArt::new(),
            diamond_art: DiamondArt::new(),
            door_art: DoorArt::new(),
            wall_art: WallArt::new(),
            wall_shadow: WallShadow::new(),
            diamond_glow: DiamondGlow::new(),
            backdrop_art: BackdropArt::new(),
            mud_art: MudArt::new(),
            mud_variant: std::cell::Cell::new(0),
            hd: HdToggles::default(),
        }
    }

    /// Next dirt panorama style (F key).
    pub fn cycle_mud(&self) {
        self.mud_variant
            .set((self.mud_variant.get() + 1) % mud::MUD_VARIANTS);
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
        if obj == Obj::Boulder && self.hd.boulder.get() {
            self.draw_backdrop(backdrop, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            self.boulder_art.draw(cell_idx, spin, x, y, CELL_PX);
            return;
        }
        if (obj == Obj::Diamond || obj == Obj::PendingDiamond) && self.hd.diamond.get() {
            self.draw_backdrop(backdrop, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            self.diamond_art.draw(frame, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Steel && self.hd.wall.get() {
            self.wall_art.draw_steel(cell_idx, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Brick && self.hd.wall.get() {
            self.wall_art.draw_brick(cell_idx, x, y, CELL_PX);
            return;
        }
        if (obj == Obj::Space || obj == Obj::Vacated) && self.hd.backdrop.get() {
            self.backdrop_art.draw(cell_idx, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Rockford {
            // His cell gets the same backdrop as every other cell; the man
            // himself (robot or NES body+head) is drawn after the cell loop.
            // (The NES Space metatile used to show through here as stale
            // pixel art.)
            self.draw_backdrop(Obj::Space, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            return;
        }
        if obj == Obj::Mud && self.hd.mud.get() {
            self.mud_art.draw(self.mud_variant.get(), cell_idx, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Door && self.hd.door.get() {
            // HD stargate on the cave backdrop (closed = inactive ring,
            // open = animated event horizon; the NES metatile door is gone).
            self.draw_backdrop(backdrop, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            self.door_art.draw(door_open, frame, x, y, CELL_PX);
            return;
        }
        let (quad, pal) = self.cell_quad_pal(obj, cave_idx, frame, door_open, magic_active);
        self.atlas.draw_quad_scaled(quad, bank, pal, x, y, CELL_PX / 16.0);
    }

    /// The backdrop drawn under HD item sprites: the cave's rock texture
    /// (or the given object's quad for non-Space backdrops).
    #[allow(clippy::too_many_arguments)]
    fn draw_backdrop(&self, obj: Obj, cell_idx: usize, cave_idx: usize, bank: usize, x: f32, y: f32, frame: u64, door_open: bool, magic_active: bool) {
        if (obj == Obj::Space || obj == Obj::Vacated) && self.hd.backdrop.get() {
            self.backdrop_art.draw(cell_idx, x, y, CELL_PX);
            return;
        }
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
        if obj == Obj::Boulder && self.hd.boulder.get() {
            self.boulder_art.draw(cell_idx, spin, x, y, CELL_PX);
            return;
        }
        if (obj == Obj::Diamond || obj == Obj::PendingDiamond) && self.hd.diamond.get() {
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

    /// Cave field + Rockford (HD robot sprite), clipped to the viewport under
    /// the HUD. `suit` is the player's color-table index (sprite palette 0
    /// color 2, $3D in the ROM) — used by the NES head overlay when the HD
    /// robot is toggled off. Objects with an active slide in `slides` are
    /// drawn at their interpolated position.
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
        // Diamond glow halos, drawn after the terrain so they light up the
        // cells around the gem (additive pass before sliding items).
        let mut glows = Vec::new();
        // Static diamonds are deferred and drawn AFTER the glow pass, so
        // the halo lights the surroundings without washing out the gem's
        // own sparkle.
        let mut diamonds = Vec::new();
        // Boulder ground shadows, drawn after the terrain so the blobs
        // darken the ground under/behind the rocks (not the rocks' tops).
        let mut boulder_shadows = Vec::new();
        for cy in y0..y1 {
            for cx in x0..x1 {
                let cell = cave.cell_at(cx, cy);
                let idx = cy * WIDTH + cx;
                let sx_c = cx as f32 * CELL_PX - cam.x;
                let sy_c = HUD_H + cy as f32 * CELL_PX - cam.y;
                let slide = slides.slide_for(idx, frame);
                match slide {
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
                        if matches!(cell.obj, Obj::Diamond | Obj::PendingDiamond) && self.hd.diamond.get() {
                            // Backdrop now, gem sprite after the glow pass.
                            self.draw_backdrop(
                                Obj::Space,
                                idx,
                                cave_idx,
                                bank,
                                sx_c,
                                sy_c,
                                frame,
                                door_open,
                                magic_active,
                            );
                            diamonds.push((sx_c, sy_c));
                        } else {
                            // HD items sit on the world's BACKDROP (Space art),
                            // never on dirt: the background shows around the
                            // sprite instead of a black void or a mud patch.
                            // The door cell keeps drawing the gate even with
                            // Rockford inside: the engine replaces the Door
                            // object with him on entry, but the portal must
                            // stay visible while it pulls him in.
                            let obj = if Some(idx) == cave.door_pos() {
                                Obj::Door
                            } else {
                                cell.obj
                            };
                            self.draw_cell(
                                obj,
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
                }
                if self.hd.wall.get() {
                    let (shade_left, shade_top, shade_diag) = wall_shadow_sides(cave, cx, cy);
                    self.wall_shadow.draw(
                        shade_left,
                        shade_top,
                        shade_diag,
                        sx_c,
                        sy_c,
                        CELL_PX,
                        shadow_variation(cx, cy),
                    );
                }
                if slide.is_none() {
                    // Resting-object effects only: while an object slides
                    // INTO this cell the engine already shows it here, and
                    // these would pop in at the destination ahead of the
                    // sliding sprite (its own shadow/glow travels with it).
                    if matches!(cell.obj, Obj::Diamond | Obj::PendingDiamond) && self.hd.diamond.get() {
                        glows.push((sx_c + CELL_PX / 2.0, sy_c + CELL_PX / 2.0));
                    }
                    if cell.obj == Obj::Boulder && self.hd.boulder.get() {
                        boulder_shadows.push((sx_c, sy_c));
                    }
                }
            }
        }
        for (bx, by) in boulder_shadows {
            self.wall_shadow.draw_boulder_shadow(bx, by, CELL_PX);
        }
        if !glows.is_empty() {
            self.diamond_glow.apply_material();
            for (gx, gy) in glows {
                self.diamond_glow.draw(frame, gx, gy, CELL_PX);
            }
            self.diamond_glow.reset_material();
        }
        for (dx, dy) in diamonds {
            self.diamond_art.draw(frame, dx, dy, CELL_PX);
        }
        for (obj, idx, spin, sx, sy) in sliding {
            if matches!(obj, Obj::Diamond | Obj::PendingDiamond) && self.hd.diamond.get() {
                self.diamond_glow.apply_material();
                self.diamond_glow.draw(frame, sx + CELL_PX / 2.0, sy + CELL_PX / 2.0, CELL_PX);
                self.diamond_glow.reset_material();
            }
            if obj == Obj::Boulder && self.hd.boulder.get() {
                self.wall_shadow.draw_boulder_shadow(sx, sy, CELL_PX);
            }
            self.draw_cell_sliding(obj, idx, spin, cave_idx, bank, sx, sy, frame, door_open, magic_active);
        }
        if self.hd.rockford.get() {
            if let Some(rf) = rock.robot_frame(frame) {
                // The HD robot is drawn AFTER the cell loop at his fractional
                // slide position. He stands taller than one cell: anchored so
                // his feet rest on the cell's base line, centered horizontally.
                // While pushing, the sprite shifts towards the boulder so his
                // hands actually touch it.
                let mut s = CELL_PX * robot::DRAW_CELLS;
                let mut pos = rock.pos;
                if let Some(center) = rock.suck_center() {
                    // Portal pull-in: drift towards the ring's center while
                    // shrinking into it (scale comes from robot_frame).
                    let k = 1.0 - rf.scale;
                    pos.0 += (center.0 - 0.5 - pos.0) * k;
                    pos.1 += (center.1 - 1.0 - pos.1) * k;
                    s *= rf.scale;
                }
                let mut dx = (CELL_PX - s) / 2.0;
                if rf.row == robot::ROW_PUSH {
                    dx += if rf.flip { -s * robot::PUSH_REACH } else { s * robot::PUSH_REACH };
                }
                let sx = pos.0 * CELL_PX - cam.x + dx;
                let sy = HUD_H + pos.1 * CELL_PX - cam.y + CELL_PX - s * robot::FEET_FRACTION;
                self.robot_art
                    .draw(rf.row, rf.frame, sx, sy, s, rf.flip, rf.rotation, rf.alpha);
            }
        } else if let Some(head) = rock.head_frame(frame) {
            // NES Rockford: body metatile + head overlay TOGETHER at the
            // fractional slide position (the NES had to jump the background
            // body cell-wise; we don't).
            let sx = rock.pos.0 * CELL_PX - cam.x;
            let sy = HUD_H + rock.pos.1 * CELL_PX - cam.y;
            let scale = CELL_PX / 16.0;
            if rock.alive() {
                // Body = background metatile 15 (quad $F44F), world palette 0,
                // animated via the CHR1 bank swap frames (banks 0-3).
                let bbank = ((frame / 8) % 4) as usize;
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

    /// The F1 graphics menu: one HD/NES toggle row per replaced item.
    /// Drawn in the cave viewport's 640x416 coordinate space (gameplay
    /// states only); `sel` is the highlighted row.
    pub fn draw_gfx_menu(&self, sel: usize) {
        let rows = self.hd.rows();
        let line_h = 15.0;
        let h = (rows.len() + 2) as f32 * line_h;
        let y0 = HUD_H + (VIEW_H - h) / 2.0;
        let x0 = (VIEW_W - 260.0) / 2.0;
        draw_rectangle(x0 - 18.0, y0 - 10.0, 296.0, h + 22.0, Color::new(0.0, 0.0, 0.0, 0.82));
        hud::draw_text(&self.atlas, x0 + 30.0, y0, "GRAPHICS  F1:CLOSE");
        for (i, (label, hd)) in rows.iter().enumerate() {
            let mark = if i == sel { ">" } else { " " };
            let mode = if *hd { "HD " } else { "NES" };
            hud::draw_text(
                &self.atlas,
                x0,
                y0 + (i + 1) as f32 * line_h,
                &format!("{mark} {label:<9} {mode}"),
            );
        }
        hud::draw_text(&self.atlas, x0 + 6.0, y0 + (rows.len() + 1) as f32 * line_h, "ARROWS:MOVE FLIP");
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
