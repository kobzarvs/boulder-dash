//! Top HUD bar (32 px, two cell rows): diamonds quota, score, time, cave id,
//! reserve lives. Font tiles live in CHR bank 6: digits 0-9 at 1665-1674,
//! A-Z at 1675-1700; tile 1712 is blank.
//!
//! PROVISIONAL: the original HUD uses dedicated diamond/clock icon tiles
//! (1701-1712 are box-frame pieces, no icon art found); we substitute the
//! world's diamond metatile for the diamond icon and a "TIME" label for the
//! clock.

use crate::data::tiles::METATILE_SEQS;
use crate::engine::Cave;

use super::atlas::Atlas;

pub const HUD_H: f32 = 32.0;

/// Right edge of the HUD bar (screen is 512 px wide).
const RIGHT: f32 = 508.0;

/// Palette group used for HUD text (white/gray glyph, black shadow).
const TEXT_PAL: usize = 2;

pub fn font_tile(c: char) -> usize {
    match c {
        '0'..='9' => 1665 + (c as usize - '0' as usize),
        'A'..='Z' => 1675 + (c as usize - 'A' as usize),
        _ => 1712, // blank
    }
}

pub fn draw_text(atlas: &Atlas, x: f32, y: f32, text: &str) {
    for (i, c) in text.chars().enumerate() {
        atlas.draw_tile(font_tile(c), TEXT_PAL, x + i as f32 * 8.0, y);
    }
}

/// Draw a right-aligned number, zero-padded to `width` digits.
fn draw_num(atlas: &Atlas, x_right: f32, y: f32, value: u32, width: usize) {
    let s = format!("{value:0width$}");
    let x = x_right - s.len() as f32 * 8.0;
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
        TEXT_PAL
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
    draw_text(atlas, 224.0, 20.0, &label);
    draw_num(atlas, 296.0, 20.0, level as u32, 1);
    let lives = format!("LIVES{}", cave.lives());
    draw_text(atlas, RIGHT - lives.len() as f32 * 8.0, 20.0, &lives);
}
