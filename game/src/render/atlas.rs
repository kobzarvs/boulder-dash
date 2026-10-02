//! Tile atlas: every CHR tile pre-rendered in every palette variant.
//!
//! The NES selects colors per-tile via palette attributes; we instead bake
//! `2048 tiles x 8 palettes` into one 1024x1024 texture at startup and pick
//! the source rect per draw. Palettes 0-3 are the background groups,
//! 4-7 the sprite groups (pixel value 0 = transparent for sprites).
//!
//! PROVISIONAL: the universal backdrop ($3F00) is rendered as solid black.
//! All palette tables store 0x22 (light blue) in slot 0, but every manual/
//! reference screenshot shows a black cave background, so black it is.

use macroquad::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::data::cave_params::{NES_PALETTE, PALETTE_MAIN};
use crate::data::tiles::{TILES, TILE_COUNT};

/// Background palette groups (from the low 16 bytes of the palette table).
pub const BG_PALETTES: usize = 4;
/// Sprite palette groups (high 16 bytes); pixel 0 is transparent there.
pub const SPRITE_PALETTES: usize = 4;
pub const PALETTE_COUNT: usize = BG_PALETTES + SPRITE_PALETTES + 2;

/// Extra baked palette row: light-blue diamonds. The ROM colors diamonds
/// with the orange wall palette (attr 0); the remake uses the classic blue.
pub const DIAMOND_PAL: usize = 8;
/// [backdrop, shade, body, sparkle] for DIAMOND_PAL.
const DIAMOND_BLUE: [u8; 4] = [0x0F, 0x11, 0x21, 0x30];

/// Extra baked palette row: HUD/overlay text. The ROM font tiles are SOLID
/// (background pixels = value 1, glyph strokes = 2/3), so the palette must
/// map 1 -> black, 2 -> gray shadow, 3 -> white glyph.
pub const FONT_PAL: usize = 9;
const FONT_COLORS: [u8; 4] = [0x0F, 0x0F, 0x10, 0x20];

/// Atlas geometry: 128x160 tiles of 8x8 px (10 palette rows of 2048 tiles).
pub const ATLAS_COLS: usize = 128;
pub const ATLAS_SIDE: u16 = (ATLAS_COLS * 8) as u16;
pub const ATLAS_H: u16 = ((PALETTE_COUNT * TILE_COUNT) / ATLAS_COLS * 8) as u16;

/// NES color id 0x0F — black, used as the universal backdrop (PROVISIONAL).
const BACKDROP_NES: u8 = 0x0F;

pub fn nes_rgb(idx: u8) -> (u8, u8, u8) {
    let [r, g, b] = NES_PALETTE[(idx & 0x3F) as usize];
    (r, g, b)
}

/// Resolve a baked palette row to its 4 NES color ids (custom rows included).
fn palette_table(pal: usize) -> &'static [u8; 4] {
    match pal {
        DIAMOND_PAL => &DIAMOND_BLUE,
        FONT_PAL => &FONT_COLORS,
        _ => PALETTE_MAIN[pal * 4..pal * 4 + 4].try_into().unwrap(),
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
                            let (r, g, b) = nes_rgb(BACKDROP_NES);
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
    /// pixels = value 1), so for overlays on bright screens value 1 is skipped
    /// and only the glyph strokes (2/3) are drawn, in palette `pal`.
    pub fn text_texture(&self, text: &str, pal: usize) -> Texture2D {
        let key = (text.to_owned(), pal as u8);
        self.text_tex.borrow_mut().entry(key).or_insert_with(|| {
            let table = palette_table(pal);
            let w = (text.chars().count().max(1) * 8) as u16;
            let mut img = Image::gen_image_color(w, 8, Color::new(0.0, 0.0, 0.0, 0.0));
            for (i, c) in text.chars().enumerate() {
                let px = &TILES[super::hud::font_tile(c)];
                for y in 0..8 {
                    for x in 0..8 {
                        let v = px[y * 8 + x] as usize;
                        if v <= 1 {
                            continue; // 0/1 = background
                        }
                        let [r, g, b] = NES_PALETTE[(table[v] & 0x3F) as usize];
                        img.set_pixel(
                            (i * 8 + x) as u32,
                            y as u32,
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

    /// Draw text with a transparent background (overlay on bright screens).
    pub fn draw_text_clear(&self, text: &str, pal: usize, x: f32, y: f32) {
        draw_texture(&self.text_texture(text, pal), x, y, WHITE);
    }

    /// Draw one 8x8 tile (global CHR index) tinted by palette `pal` at px (x, y).
    pub fn draw_tile(&self, tile: usize, pal: usize, x: f32, y: f32) {
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
                ..Default::default()
            },
        );
    }

    /// Draw a 2x2-tile metatile quad `[tl, tr, bl, br]`; entries are indices
    /// into the given CHR bank's 256-tile pattern table.
    pub fn draw_quad(&self, quad: [u8; 4], bank: usize, pal: usize, x: f32, y: f32) {
        for (pos, &t) in quad.iter().enumerate() {
            let dx = (pos % 2) as f32 * 8.0;
            let dy = (pos / 2) as f32 * 8.0;
            self.draw_tile(bank * 256 + t as usize, pal, x + dx, y + dy);
        }
    }
}
