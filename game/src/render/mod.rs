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
pub mod explosion;
pub mod glow;
pub mod hud;
pub mod mud;
pub mod nametable;
pub mod robot;
pub mod rockford;
pub mod rowatlas;
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
use explosion::ExplosionFx;
use glow::DiamondGlow;
use hud::HUD_H;
use mud::MudArt;
use nametable::Screens;
use robot::RobotArt;
use rockford::{RockfordAnim, RockfordArt};
use rowatlas::RowAtlas;
use shadow::WallShadow;
use wall::WallArt;

/// Per-item art variants (the F1 menu): index 0 = original NES metatile,
/// 1.. = HD remaster variants. Cells so the main loop can flip them through
/// the shared &Renderer.
pub struct HdToggles {
    vals: [std::cell::Cell<usize>; ITEM_STYLES.len()],
}

/// The toggleable items: (menu label, variant names). Index 0 is always the
/// NES original. New HD items get appended at the end.
pub const ITEM_STYLES: [(&str, &[&str]); 12] = [
    ("ROCKFORD", &["NES", "HD"]),
    ("BOULDERS", &["NES", "HD"]),
    ("DIAMONDS", &["NES", "HD"]),
    ("DOOR", &["NES", "HD"]),
    ("WALLS", &["NES", "HD"]),
    ("MUD", &["NES", "HD"]),
    ("BACKDROP", &["NES", "HD"]),
    ("FIREFLY", &["NES", "BEETLE", "BOT"]),
    ("BUTTERFLY", &["NES", "MONARCH", "CRYSTAL"]),
    ("AMOEBA", &["NES", "SLIME", "TOXIC"]),
    ("MAGICWALL", &["NES", "AMETHYST", "AQUA"]),
    ("EXPLOSION", &["NES", "EMBERS"]),
];

// Row indexes for the draw code.
pub const HD_ROCKFORD: usize = 0;
pub const HD_BOULDER: usize = 1;
pub const HD_DIAMOND: usize = 2;
pub const HD_DOOR: usize = 3;
pub const HD_WALL: usize = 4;
pub const HD_MUD: usize = 5;
pub const HD_BACKDROP: usize = 6;
pub const HD_FIREFLY: usize = 7;
pub const HD_BUTTERFLY: usize = 8;
pub const HD_AMOEBA: usize = 9;
pub const HD_MAGICWALL: usize = 10;
pub const HD_EXPLOSION: usize = 11;

impl Default for HdToggles {
    fn default() -> Self {
        Self { vals: std::array::from_fn(|_| std::cell::Cell::new(1)) }
    }
}

impl HdToggles {
    /// Current variant index of row `idx`.
    pub fn get(&self, idx: usize) -> usize {
        self.vals[idx].get()
    }

    /// Menu rows: (label, variant name) per item.
    pub fn rows(&self) -> [(&'static str, &'static str); ITEM_STYLES.len()] {
        std::array::from_fn(|i| {
            let (label, variants) = ITEM_STYLES[i];
            (label, variants[self.vals[i].get()])
        })
    }

    /// Cycle one menu row forward/backward.
    pub fn toggle(&self, row: usize, back: bool) {
        let n = ITEM_STYLES[row].1.len();
        let v = self.vals[row].get();
        self.vals[row].set(if back { (v + n - 1) % n } else { (v + 1) % n });
    }

    /// Set a row's variant by item label (BDNES env debug helper sets the
    /// NES variant: `set_by_name(name, false)`; `true` picks the first HD
    /// variant).
    pub fn set_by_name(&self, name: &str, hd: bool) {
        if let Some(i) = ITEM_STYLES.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)) {
            self.vals[i].set(if hd { 1 } else { 0 });
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
    firefly_art: RowAtlas,
    butterfly_art: RowAtlas,
    amoeba_art: RowAtlas,
    magicwall_art: RowAtlas,
    explosion_fx: ExplosionFx,
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
            firefly_art: RowAtlas::new(include_bytes!("../../assets/firefly.png")),
            butterfly_art: RowAtlas::new(include_bytes!("../../assets/butterfly.png")),
            amoeba_art: RowAtlas::new(include_bytes!("../../assets/amoeba.png")),
            magicwall_art: RowAtlas::new(include_bytes!("../../assets/magicwall.png")),
            explosion_fx: ExplosionFx::new(),
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
        if obj == Obj::Boulder && self.hd.get(HD_BOULDER) > 0 {
            self.draw_backdrop(backdrop, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            self.boulder_art.draw(cell_idx, spin, x, y, CELL_PX);
            return;
        }
        if (obj == Obj::Diamond || obj == Obj::PendingDiamond) && self.hd.get(HD_DIAMOND) > 0 {
            self.draw_backdrop(backdrop, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            self.diamond_art.draw(frame, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Steel && self.hd.get(HD_WALL) > 0 {
            self.wall_art.draw_steel(cell_idx, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Brick && self.hd.get(HD_WALL) > 0 {
            self.wall_art.draw_brick(cell_idx, x, y, CELL_PX);
            return;
        }
        if (obj == Obj::Space || obj == Obj::Vacated) && self.hd.get(HD_BACKDROP) > 0 {
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
        if obj == Obj::Mud && self.hd.get(HD_MUD) > 0 {
            self.mud_art.draw(self.mud_variant.get(), cell_idx, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Door && self.hd.get(HD_DOOR) > 0 {
            // HD stargate on the cave backdrop (closed = inactive ring,
            // open = animated event horizon; the NES metatile door is gone).
            self.draw_backdrop(backdrop, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            self.door_art.draw(door_open, frame, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Firefly && self.hd.get(HD_FIREFLY) > 0 {
            self.draw_backdrop(backdrop, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            self.firefly_art.draw(self.hd.get(HD_FIREFLY) - 1, frame, 2, 0, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Butterfly && self.hd.get(HD_BUTTERFLY) > 0 {
            self.draw_backdrop(backdrop, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            self.butterfly_art
                .draw(self.hd.get(HD_BUTTERFLY) - 1, frame, 2, 0, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Amoeba && self.hd.get(HD_AMOEBA) > 0 {
            self.draw_backdrop(backdrop, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            // Per-cell phase: a growing mass never wobbles in lockstep.
            self.amoeba_art
                .draw(self.hd.get(HD_AMOEBA) - 1, frame, 4, (cell_idx % 8) as u64, x, y, CELL_PX);
            return;
        }
        if obj == Obj::MagicWall && self.hd.get(HD_MAGICWALL) > 0 {
            self.draw_backdrop(backdrop, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            let row = (self.hd.get(HD_MAGICWALL) - 1) * 2 + magic_active as usize;
            self.magicwall_art.draw(row, frame, 2, 0, x, y, CELL_PX);
            return;
        }
        if obj == Obj::ExplosionRemnant && self.hd.get(HD_EXPLOSION) > 0 {
            self.draw_backdrop(backdrop, cell_idx, cave_idx, bank, x, y, frame, door_open, magic_active);
            self.explosion_fx.draw(cell_idx, frame, x, y, CELL_PX);
            return;
        }
        let (quad, pal) = self.cell_quad_pal(obj, cave_idx, frame, door_open, magic_active);
        self.atlas.draw_quad_scaled(quad, bank, pal, x, y, CELL_PX / 16.0);
    }

    /// The backdrop drawn under HD item sprites: the cave's rock texture
    /// (or the given object's quad for non-Space backdrops).
    #[allow(clippy::too_many_arguments)]
    fn draw_backdrop(&self, obj: Obj, cell_idx: usize, cave_idx: usize, bank: usize, x: f32, y: f32, frame: u64, door_open: bool, magic_active: bool) {
        if (obj == Obj::Space || obj == Obj::Vacated) && self.hd.get(HD_BACKDROP) > 0 {
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
        if obj == Obj::Boulder && self.hd.get(HD_BOULDER) > 0 {
            self.boulder_art.draw(cell_idx, spin, x, y, CELL_PX);
            return;
        }
        if (obj == Obj::Diamond || obj == Obj::PendingDiamond) && self.hd.get(HD_DIAMOND) > 0 {
            self.diamond_art.draw(frame, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Firefly && self.hd.get(HD_FIREFLY) > 0 {
            self.firefly_art.draw(self.hd.get(HD_FIREFLY) - 1, frame, 2, 0, x, y, CELL_PX);
            return;
        }
        if obj == Obj::Butterfly && self.hd.get(HD_BUTTERFLY) > 0 {
            self.butterfly_art
                .draw(self.hd.get(HD_BUTTERFLY) - 1, frame, 2, 0, x, y, CELL_PX);
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
                        if matches!(cell.obj, Obj::Diamond | Obj::PendingDiamond) && self.hd.get(HD_DIAMOND) > 0 {
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
                if self.hd.get(HD_WALL) > 0 {
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
                    if matches!(cell.obj, Obj::Diamond | Obj::PendingDiamond) && self.hd.get(HD_DIAMOND) > 0 {
                        glows.push((sx_c + CELL_PX / 2.0, sy_c + CELL_PX / 2.0));
                    }
                    if cell.obj == Obj::Boulder && self.hd.get(HD_BOULDER) > 0 {
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
            if matches!(obj, Obj::Diamond | Obj::PendingDiamond) && self.hd.get(HD_DIAMOND) > 0 {
                self.diamond_glow.apply_material();
                self.diamond_glow.draw(frame, sx + CELL_PX / 2.0, sy + CELL_PX / 2.0, CELL_PX);
                self.diamond_glow.reset_material();
            }
            if obj == Obj::Boulder && self.hd.get(HD_BOULDER) > 0 {
                self.wall_shadow.draw_boulder_shadow(sx, sy, CELL_PX);
            }
            self.draw_cell_sliding(obj, idx, spin, cave_idx, bank, sx, sy, frame, door_open, magic_active);
        }
        if self.hd.get(HD_ROCKFORD) > 0 {
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

    /// The F1 graphics menu: one row per replaced item, cycling NES and the
    /// HD variants. Drawn in the cave viewport's 640x416 coordinate space
    /// (gameplay states only); `sel` is the highlighted row.
    pub fn draw_gfx_menu(&self, sel: usize) {
        let rows = self.hd.rows();
        let line_h = 14.0;
        let h = (rows.len() + 2) as f32 * line_h;
        let y0 = HUD_H + (VIEW_H - h) / 2.0;
        let x0 = (VIEW_W - 300.0) / 2.0;
        draw_rectangle(x0 - 18.0, y0 - 10.0, 336.0, h + 22.0, Color::new(0.0, 0.0, 0.0, 0.82));
        hud::draw_text(&self.atlas, x0 + 42.0, y0, "GRAPHICS  F1:CLOSE");
        for (i, (label, variant)) in rows.iter().enumerate() {
            let mark = if i == sel { ">" } else { " " };
            hud::draw_text(
                &self.atlas,
                x0,
                y0 + (i + 1) as f32 * line_h,
                &format!("{mark} {label:<10} {variant}"),
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
