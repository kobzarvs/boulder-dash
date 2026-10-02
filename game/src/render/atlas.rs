//! Tile atlas: every CHR tile pre-rendered in every palette variant.
//!
//! The NES selects colors per-tile via palette attributes; we instead bake
//! `2048 tiles x 30 palettes` into one texture at startup and pick the source
//! rect per draw. Rows 0-23 are the per-WORLD background groups (6 worlds x
//! 4, from the ROM's $BC28 table — byte 0 of each group is that world's
//! backdrop, drawn opaque), 24-27 the sprite groups (pixel value 0 =
//! transparent), 28 blue diamonds, 29 the text palette.

use macroquad::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::data::cave_params::{NES_PALETTE, PALETTE_MAIN, WORLD_PALETTES};
use crate::data::tiles::{TILES, TILE_COUNT};

/// Background palette rows: 6 worlds x 4 groups from WORLD_PALETTES.
pub const BG_PALETTES: usize = 24;
/// Sprite palette rows (high 16 bytes of PALETTE_MAIN); pixel 0 transparent.
pub const SPRITE_PALETTES: usize = 4;
pub const PALETTE_COUNT: usize = BG_PALETTES + SPRITE_PALETTES + 3;

/// Extra baked palette row: light-blue diamonds. The ROM colors diamonds
/// with the orange wall palette (attr 0); the remake uses the classic blue.
pub const DIAMOND_PAL: usize = 28;
/// [backdrop, shade, body, sparkle] for DIAMOND_PAL.
const DIAMOND_BLUE: [u8; 4] = [0x0F, 0x11, 0x21, 0x30];

/// Extra baked palette row: HUD/overlay text. The ROM font tiles are SOLID
/// (background pixels = value 1, glyph strokes = 2/3). Map 1 -> black,
/// 2 -> BLACK shadow (reads as an outline on bright backgrounds), 3 -> white.
pub const FONT_PAL: usize = 29;
const FONT_COLORS: [u8; 4] = [0x0F, 0x0F, 0x0F, 0x20];

/// Extra baked palette row: all-white sparkle flash (diamond shimmer,
/// door/magic-wall/explosion flash).
pub const SPARKLE_PAL: usize = 30;
const SPARKLE_COLORS: [u8; 4] = [0x0F, 0x30, 0x30, 0x30];

/// Palette row for a world's background group: world 0-5, attr 0-3.
pub fn world_pal(world: usize, attr: usize) -> usize {
    world.min(5) * 4 + (attr & 3)
}

/// Atlas geometry: 128 tiles wide, PALETTE_COUNT*16 rows of 8x8 px.
pub const ATLAS_COLS: usize = 128;
pub const ATLAS_SIDE: u16 = (ATLAS_COLS * 8) as u16;
pub const ATLAS_H: u16 = ((PALETTE_COUNT * TILE_COUNT) / ATLAS_COLS * 8) as u16;

pub fn nes_rgb(idx: u8) -> (u8, u8, u8) {
    let [r, g, b] = NES_PALETTE[(idx & 0x3F) as usize];
    (r, g, b)
}

/// Resolve a baked palette row to its 4 NES color ids (custom rows included).
fn palette_table(pal: usize) -> &'static [u8; 4] {
    match pal {
        0..=23 => WORLD_PALETTES[pal / 4][(pal % 4) * 4..(pal % 4) * 4 + 4]
            .try_into()
            .unwrap(),
        24..=27 => PALETTE_MAIN[16 + (pal - 24) * 4..16 + (pal - 24) * 4 + 4]
            .try_into()
            .unwrap(),
        DIAMOND_PAL => &DIAMOND_BLUE,
        FONT_PAL => &FONT_COLORS,
        SPARKLE_PAL => &SPARKLE_COLORS,
        _ => unreachable!("bad palette row {pal}"),
    }
}

pub struct Atlas {
    pub texture: Texture2D,
    /// Transparent metatile-quad textures (map/menu markers), built on demand.
    quad_tex: RefCell<HashMap<u64, Texture2D>>,
    /// Transparent text textures (font background value 1 skipped), cached
    /// per (text, palette).
    text_tex: RefCell<HashMap<(String, u8), Texture2D>>,
}

impl Default for Atlas {
    fn default() -> Self {
        Self::new()
    }
}

impl Atlas {
    pub fn new() -> Atlas {
        let mut bytes = vec![0u8; ATLAS_SIDE as usize * ATLAS_H as usize * 4];
        for pal in 0..PALETTE_COUNT {
            let table: &[u8; 4] = palette_table(pal);
            let sprite = (BG_PALETTES..BG_PALETTES + SPRITE_PALETTES).contains(&pal);
            for (tile, px) in TILES.iter().enumerate() {
                let idx = pal * TILE_COUNT + tile;
                let ox = (idx % ATLAS_COLS) * 8;
                let oy = (idx / ATLAS_COLS) * 8;
                for py in 0..8 {
                    for px_i in 0..8 {
                        let v = px[py * 8 + px_i] as usize;
                        let (r, g, b, a) = if v == 0 {
                            // Backdrop: the palette's own color 0 (opaque for
                            // background rows, transparent for sprites).
                            let (r, g, b) = nes_rgb(table[0]);
                            (r, g, b, if sprite { 0 } else { 255 })
                        } else {
                            let (r, g, b) = nes_rgb(table[v]);
                            (r, g, b, 255)
                        };
                        let o = ((oy + py) * ATLAS_SIDE as usize + ox + px_i) * 4;
                        bytes[o] = r;
                        bytes[o + 1] = g;
                        bytes[o + 2] = b;
                        bytes[o + 3] = a;
                    }
                }
            }
        }
        let img = Image {
            bytes,
            width: ATLAS_SIDE,
            height: ATLAS_H,
        };
        let texture = Texture2D::from_image(&img);
        texture.set_filter(FilterMode::Nearest);
        Atlas {
            texture,
            quad_tex: RefCell::new(HashMap::new()),
            text_tex: RefCell::new(HashMap::new()),
        }
    }

    /// A 2x2-tile metatile quad as a 16x16 texture with color 0 TRANSPARENT
    /// (cached). The atlas' background-palette tiles bake color 0 as opaque
    /// black — correct on the cave's black backdrop, wrong for menu/map
    /// markers over bright art, which is what this is for.
    pub fn quad_texture(&self, quad: [u8; 4], bank: usize, pal: usize) -> Texture2D {
        let key = u64::from_le_bytes([
            quad[0], quad[1], quad[2], quad[3], bank as u8, pal as u8, 0, 0,
        ]);
        self.quad_tex.borrow_mut().entry(key).or_insert_with(|| {
            let table = palette_table(pal);
            let mut img = Image::gen_image_color(16, 16, Color::new(0.0, 0.0, 0.0, 0.0));
            for (pos, &t) in quad.iter().enumerate() {
                let px = &TILES[bank * 256 + t as usize];
                for y in 0..8 {
                    for x in 0..8 {
                        let v = px[y * 8 + x] as usize;
                        if v == 0 {
                            continue;
                        }
                        let [r, g, b] = NES_PALETTE[(table[v] & 0x3F) as usize];
                        img.set_pixel(
                            (pos % 2 * 8 + x) as u32,
                            (pos / 2 * 8 + y) as u32,
                            Color::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0),
                        );
                    }
                }
            }
            let tex = Texture2D::from_image(&img);
            tex.set_filter(FilterMode::Nearest);
            tex
        })
        .clone()
    }

    /// Text as a transparent texture: the ROM font tiles are solid (background
    /// pixels = value 1), so for overlays on bright screens the background is
    /// skipped and the glyph strokes are drawn as a light glyph with a 1px
    /// dark OUTLINE (readable on any backdrop, unlike the font's own heavy
    /// shadow). Texture is 2 px wider/taller for the outline; draw it at
    /// (x-1, y-1) to keep the logical position.
    pub fn text_texture(&self, text: &str, pal: usize) -> Texture2D {
        let key = (text.to_owned(), pal as u8);
        self.text_tex.borrow_mut().entry(key).or_insert_with(|| {
            let table = palette_table(pal);
            let glyph_rgb = NES_PALETTE[(table[3] & 0x3F) as usize];
            let outline_rgb = NES_PALETTE[0x0F];
            let to_color = |[r, g, b]: [u8; 3]| {
                Color::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
            };
            let chars = text.chars().count().max(1);
            let (w, h) = ((chars * 8 + 2) as usize, 10usize);
            let mut glyph = vec![false; w * h];
            for (i, c) in text.chars().enumerate() {
                let px = &TILES[super::hud::font_tile(c)];
                for y in 0..8 {
                    for x in 0..8 {
                        if px[y * 8 + x] >= 2 {
                            glyph[(y + 1) * w + (i * 8 + x + 1)] = true;
                        }
                    }
                }
            }
            let mut img = Image::gen_image_color(w as u16, h as u16, Color::new(0.0, 0.0, 0.0, 0.0));
            // Outline first (glyph pixels painted over it below).
            for y in 0..h {
                for x in 0..w {
                    if glyph[y * w + x] {
                        continue;
                    }
                    let near = [[-1i32, 0], [1, 0], [0, -1], [0, 1]].iter().any(|[dx, dy]| {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h
                            && glyph[ny as usize * w + nx as usize]
                    });
                    if near {
                        img.set_pixel(x as u32, y as u32, to_color(outline_rgb));
                    }
                }
            }
            for y in 0..h {
                for x in 0..w {
                    if glyph[y * w + x] {
                        img.set_pixel(x as u32, y as u32, to_color(glyph_rgb));
                    }
                }
            }
            let tex = Texture2D::from_image(&img);
            tex.set_filter(FilterMode::Nearest);
            tex
        })
        .clone()
    }

    /// Draw text with a transparent background (overlay on bright screens).
    pub fn draw_text_clear(&self, text: &str, pal: usize, x: f32, y: f32) {
        draw_texture(&self.text_texture(text, pal), x - 1.0, y - 1.0, WHITE);
    }

    /// Draw one 8x8 tile (global CHR index) tinted by palette `pal` at px (x, y).
    pub fn draw_tile(&self, tile: usize, pal: usize, x: f32, y: f32) {
        self.draw_tile_scaled(tile, pal, x, y, 1.0);
    }

    /// Draw one 8x8 tile scaled to 8*scale px.
    pub fn draw_tile_scaled(&self, tile: usize, pal: usize, x: f32, y: f32, scale: f32) {
        let idx = pal * TILE_COUNT + (tile & 0x7FF);
        let sx = ((idx % ATLAS_COLS) * 8) as f32;
        let sy = ((idx / ATLAS_COLS) * 8) as f32;
        draw_texture_ex(
            &self.texture,
            x,
            y,
            WHITE,
            DrawTextureParams {
                source: Some(Rect::new(sx, sy, 8.0, 8.0)),
                dest_size: Some(vec2(8.0 * scale, 8.0 * scale)),
                ..Default::default()
            },
        );
    }

    /// Draw a 2x2-tile metatile quad `[tl, tr, bl, br]`; entries are indices
    /// into the given CHR bank's 256-tile pattern table.
    pub fn draw_quad(&self, quad: [u8; 4], bank: usize, pal: usize, x: f32, y: f32) {
        self.draw_quad_scaled(quad, bank, pal, x, y, 1.0);
    }

    /// Same as `draw_quad`, scaled to (16*scale) px cells.
    pub fn draw_quad_scaled(
        &self,
        quad: [u8; 4],
        bank: usize,
        pal: usize,
        x: f32,
        y: f32,
        scale: f32,
    ) {
        for (pos, &t) in quad.iter().enumerate() {
            let dx = (pos % 2) as f32 * 8.0 * scale;
            let dy = (pos / 2) as f32 * 8.0 * scale;
            self.draw_tile_scaled(bank * 256 + t as usize, pal, x + dx, y + dy, scale);
        }
    }
}
