//! Top HUD bar (32 px, two cell rows): diamonds quota, score, time, cave id,
//! reserve lives. Text is drawn with macroquad's built-in font (the ROM font
//! tiles are solid-background and proved unreadable at non-integer zoom).
//!
//! PROVISIONAL: the original HUD uses dedicated diamond/clock icon tiles
//! (1701-1712 are box-frame pieces, no icon art found); we substitute the
//! world's diamond metatile for the diamond icon and a "TIME" label for the
//! clock.

use macroquad::prelude::*;

use crate::data::tiles::METATILE_SEQS;
use crate::engine::Cave;

use super::atlas::Atlas;

pub const HUD_H: f32 = 32.0;

/// Right edge of the HUD bar (screen is 640 px wide).
const RIGHT: f32 = 636.0;

/// Built-in-font size for UI text (~8 px glyph, matching the old tile rows).
pub const FONT_SIZE: f32 = 12.0;
/// Rasterization size; drawn at FONT_SIZE via font_scale, so glyphs are
/// supersampled ~4x and stay smooth under camera/window zoom.
const RASTER_SIZE: u16 = 48;
/// Baseline offset from a tile-row top.
const BASELINE: f32 = 10.0;

fn text_params(color: Color) -> TextParams<'static> {
    TextParams {
        font_size: RASTER_SIZE,
        font_scale: FONT_SIZE / RASTER_SIZE as f32,
        color,
        ..Default::default()
    }
}

/// CHR font tile index (still used for the atlas text textures/sprites).
pub fn font_tile(c: char) -> usize {
    match c {
        '0'..='9' => 1665 + (c as usize - '0' as usize),
        'A'..='Z' => 1675 + (c as usize - 'A' as usize),
        _ => 1712, // blank
    }
}

/// Text width at FONT_SIZE with the built-in font.
pub fn text_width(text: &str) -> f32 {
    measure_text(text, None, RASTER_SIZE, FONT_SIZE / RASTER_SIZE as f32).width
}

/// Draw UI text with a 1px black outline using the built-in font.
pub fn text_outlined(x: f32, y_baseline: f32, text: &str) {
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
        macroquad::prelude::draw_text_ex(text, x + dx, y_baseline + dy, text_params(BLACK));
    }
    macroquad::prelude::draw_text_ex(text, x, y_baseline, text_params(WHITE));
}

pub fn draw_text(_atlas: &Atlas, x: f32, y: f32, text: &str) {
    text_outlined(x, y + BASELINE, text);
}

/// Draw a right-aligned number, zero-padded to `width` digits.
fn draw_num(atlas: &Atlas, x_right: f32, y: f32, value: u32, width: usize) {
    let s = format!("{value:0width$}");
    let x = x_right - text_width(&s);
    draw_text(atlas, x, y, &s);
}

#[allow(clippy::too_many_arguments)]
pub fn draw_hud(
    atlas: &Atlas,
    cave: &Cave,
    cave_idx: usize,
    level: u8,
    world_bank: usize,
    frame: u64,
) {
    // Row 0: diamond icon + remaining quota, score right-aligned.
    let door_open = cave.door_open();
    let dquad: [u8; 4] = METATILE_SEQS[8][0..4].try_into().unwrap();
    // Flash the icon once the exit is open.
    let dpal = if door_open && (frame / 8).is_multiple_of(2) {
        super::atlas::FONT_PAL
    } else {
        super::atlas::DIAMOND_PAL
    };
    atlas.draw_quad(dquad, world_bank, dpal, 2.0, 0.0);
    let remaining = cave.diamonds_needed().saturating_sub(cave.diamonds_collected());
    draw_num(atlas, 44.0, 4.0, remaining, 2);
    draw_num(atlas, RIGHT, 4.0, cave.score(), 6);

    // Row 1: time, cave letter + level, reserve lives.
    draw_text(atlas, 2.0, 20.0, "TIME");
    draw_num(atlas, 64.0, 20.0, cave.time_units_remaining(), 3);
    let label = format!("CAVE {}", (b'A' + cave_idx as u8) as char);
    draw_text(atlas, 288.0, 20.0, &label);
    draw_num(atlas, 368.0, 20.0, level as u32, 1);
    let lives = format!("LIVES{}", cave.lives());
    draw_text(atlas, RIGHT - lives.len() as f32 * 8.0, 20.0, &lives);
}
