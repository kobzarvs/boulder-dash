//! Flow-screen drawing: the decoded ROM nametables (`data::screens`) blitted
//! via `render::nametable::Screens`, plus the dynamic overlays — cursors,
//! password digit sprites, Rockford previews in the chosen suit color,
//! scores/lives readouts.
//!
//! Screen text baked into the nametables uses the ROM's own bank-5/6 font;
//! overlays use the same font tiles through the atlas (capitals + digits).
//! Positions of the dynamic digits/cursors follow the record addresses in
//! `data::screens` (e.g. STATUS_RECORDS' lives digit at row 10 col 18).

use macroquad::prelude::*;

use boulder_dash::data::cave_params::ROCKFORD_COLORS;
use boulder_dash::data::screens::{
    PASSWORD_DIGIT_TILE, PASSWORD_DIGIT_X, PASSWORD_DIGIT_Y,
};
use boulder_dash::data::sprites::{HeadFrame, ROCKFORD_BODY_QUAD, ROCKFORD_HEAD_IDLE};
use boulder_dash::data::tiles::{METATILE_ATTRS, METATILE_SEQS};
use boulder_dash::engine::Obj;
use boulder_dash::render::atlas::{nes_rgb, Atlas};
use boulder_dash::render::nametable::blit;
use boulder_dash::render::Renderer;

/// Screen is 256x240.
const W: f32 = 256.0;
/// Overlay text palette (kept for call sites; the built-in-font renderer
/// draws everything in white with a black outline regardless).
const PAL_TEXT: usize = boulder_dash::render::atlas::FONT_PAL;
const PAL_ACCENT: usize = boulder_dash::render::atlas::FONT_PAL;
/// Password digits: 8x16 SPRITE tiles with color-0 background — must use a
/// sprite palette row (pixel 0 transparent); row 26 = sprite group 2 ($D158).
const PAL_SPR_DIGITS: usize = 26;

/// World names in ROM order ($E909 caption scripts).
pub const WORLD_NAMES: [&str; 6] = ["BOULDER", "ICE", "SAND", "OCEAN", "RELIC", "VOLCANO"];

/// Overlay text via the built-in font (black-outlined, any background).
fn draw_text(atlas: &Atlas, x: f32, y: f32, text: &str) {
    let _ = atlas;
    boulder_dash::render::hud::text_outlined(x, y + boulder_dash::render::hud::BASELINE, text);
}

/// Centered 1x text line.
fn text_c(atlas: &Atlas, y: f32, text: &str) {
    let _ = atlas;
    let x = (W - boulder_dash::render::hud::text_width(text)) / 2.0;
    boulder_dash::render::hud::text_outlined(x, y + boulder_dash::render::hud::BASELINE, text);
}

fn blink(tick: u64) -> bool {
    (tick / 24).is_multiple_of(2)
}

/// Draw text in a non-default palette group (menu selection highlight).
fn text_pal(atlas: &Atlas, x: f32, y: f32, text: &str, pal: usize) {
    let _ = pal;
    draw_text(atlas, x, y, text);
}

/// Diamond quad (menu cursor / decoration), gently sparkling like in-game.
/// Transparent background (markers sit over bright map/menu art).
fn draw_diamond(atlas: &Atlas, x: f32, y: f32, tick: u64) {
    let quad: [u8; 4] = METATILE_SEQS[Obj::Diamond as usize][0..4].try_into().unwrap();
    let pal = if (tick / 16).is_multiple_of(2) {
        boulder_dash::render::atlas::DIAMOND_PAL
    } else {
        PAL_TEXT
    };
    draw_texture(&atlas.quad_texture(quad, 0, pal), x, y, WHITE);
}

/// Boulder quad in the given world-group art variant (map town marker).
fn draw_boulder(atlas: &Atlas, variant: usize, x: f32, y: f32) {
    let row = &METATILE_SEQS[Obj::Boulder as usize];
    let quad: [u8; 4] = row[variant * 4..variant * 4 + 4].try_into().unwrap();
    draw_texture(
        &atlas.quad_texture(
            quad,
            variant.min(3),
            boulder_dash::render::atlas::world_pal(
                variant,
                METATILE_ATTRS[Obj::Boulder as usize] as usize,
            ),
        ),
        x,
        y,
        WHITE,
    );
}

/// Rockford standing pose (body metatile + head overlay) integer-scaled,
/// in the given suit color — the flow-screen counterpart of the in-cave
/// two-part rendering (Q7).
pub fn draw_rockford(r: &Renderer, x: f32, y: f32, color_idx: usize, scale: u8) {
    draw_rockford_pose(r, x, y, color_idx, scale, &ROCKFORD_HEAD_IDLE[0]);
}

/// Same, with an explicit head metasprite.
pub fn draw_rockford_pose(r: &Renderer, x: f32, y: f32, color_idx: usize, scale: u8, head: &HeadFrame) {
    let s = scale as f32;
    draw_texture_ex(
        &r.atlas.quad_texture(ROCKFORD_BODY_QUAD, 0, 0),
        x,
        y,
        WHITE,
        DrawTextureParams {
            dest_size: Some(vec2(16.0 * s, 16.0 * s)),
            ..Default::default()
        },
    );
    let row = (color_idx % ROCKFORD_COLORS.len()) as f32 * 8.0;
    for q in head {
        let col = boulder_dash::data::sprites::ROCKFORD_HEAD_TILES
            .iter()
            .position(|&t| t == q[1])
            .unwrap() as f32
            * 8.0;
        draw_texture_ex(
            r.rockford_art().texture(),
            x + (q[3] as i8 as f32 + 8.0) * s,
            y + (q[0] as i8 as f32 + 16.0) * s,
            WHITE,
            DrawTextureParams {
                source: Some(Rect::new(col, row, 8.0, 8.0)),
                dest_size: Some(vec2(8.0 * s, 8.0 * s)),
                flip_x: q[2] & 0x40 != 0,
                ..Default::default()
            },
        );
    }
}

/// Current suit color as an RGBA quad (color-swatch drawing).
pub fn suit_color(color_idx: usize) -> Color {
    let (r, g, b) = nes_rgb(ROCKFORD_COLORS[color_idx % ROCKFORD_COLORS.len()]);
    Color::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
}

// ---------------------------------------------------------------------------
// Screens
// ---------------------------------------------------------------------------

/// Title screen (ROM states 0+1 merged): the decoded legal page + score/menu
/// records, with the 1P/2P diamond cursor (menu text row 27: "1PLAYER" at
/// col 8, "2PLAYER" at col 18, per ATTRACT_RECORDS[2]).
pub fn draw_title(r: &Renderer, sel: usize, tick: u64) {
    blit(&r.screens.title);
    let x = if sel == 0 { 44.0 } else { 124.0 };
    draw_diamond(&r.atlas, x, 216.0, tick);
}

/// Color select (ROM state 2): the decoded frame + "1 PLAYER"/"COLOR SELECT"
/// text; below it the live preview — Rockford wears the color being chosen
/// ($A769[index] -> sprite palette 0 color 2 write at $A746).
pub fn draw_color_select(r: &Renderer, player: usize, two_player: bool, color_idx: usize, tick: u64) {
    blit(&r.screens.color_select);
    if two_player && player == 1 {
        draw_text(&r.atlas, 224.0, 8.0, "P2");
    }
    // Suit swatch + index under the frame box.
    draw_rectangle(96.0, 136.0, 64.0, 12.0, suit_color(color_idx));
    draw_rectangle_lines(96.0, 136.0, 64.0, 12.0, 1.0, WHITE);
    text_c(&r.atlas, 156.0, &format!("COLOR {:02}", color_idx + 1));
    draw_rockford(r, 112.0, 172.0, color_idx, 2);
    text_c(&r.atlas, 208.0, "LEFT RIGHT SELECT");
    if blink(tick) {
        text_c(&r.atlas, 224.0, "A CONFIRM");
    }
}

/// Password entry (ROM state 3): the decoded "PASS WORD*" screen; the 6
/// digits are 8x16 sprites exactly like the original ($D158): y = $64 (drawn
/// at $65), x = $68+8i, tiles $76+2d/$77+2d of CHR bank 4, sprite palette
/// group 2.
pub fn draw_password(r: &Renderer, digits: &[u8; 6], cursor: usize, confirming: bool, tick: u64) {
    blit(&r.screens.password);
    let show = !confirming || !(tick / 4).is_multiple_of(2);
    if show {
        for (i, &d) in digits.iter().enumerate() {
            let t = PASSWORD_DIGIT_TILE as usize + 2 * d as usize;
            let x = PASSWORD_DIGIT_X as f32 + i as f32 * 8.0;
            let y = PASSWORD_DIGIT_Y as f32 + 1.0; // OAM Y is one less than drawn
            r.atlas.draw_tile(4 * 256 + t, PAL_SPR_DIGITS, x, y);
            r.atlas.draw_tile(4 * 256 + t + 1, PAL_SPR_DIGITS, x, y + 8.0);
        }
    }
    if !confirming && blink(tick) {
        draw_rectangle(
            PASSWORD_DIGIT_X as f32 + cursor as f32 * 8.0 - 1.0,
            PASSWORD_DIGIT_Y as f32 + 18.0,
            10.0,
            2.0,
            WHITE,
        );
    }
}

/// World map (ROM states 4/6/15/16 merged): the decoded islands backdrop.
/// The four big islands are the world's towns; cleared towns sparkle as
/// collected diamonds, and Rockford stands on the selected one.
#[allow(clippy::too_many_arguments)]
pub fn draw_map(
    r: &Renderer,
    world: usize,
    quest: usize,
    cursor: usize,
    towns_cleared: u8,
    score: u32,
    lives: u8,
    player: usize,
    two_player: bool,
    suit: usize,
    tick: u64,
) {
    blit(&r.screens.map_walk);
    let atlas = &r.atlas;
    draw_text(atlas, 8.0, 8.0, &format!("{} WORLD", WORLD_NAMES[world]));
    draw_text(atlas, 8.0, 16.0, &format!("QUEST {}", quest + 1));
    if two_player {
        draw_text(atlas, 232.0, 8.0, &format!("P{}", player + 1));
    }

    // Town nodes, one per big island (clockwise from top-left).
    const NODES: [(f32, f32); 4] = [(48.0, 72.0), (184.0, 64.0), (176.0, 168.0), (56.0, 176.0)];
    for (t, &(nx, ny)) in NODES.iter().enumerate() {
        if towns_cleared & (1 << t) != 0 {
            draw_diamond(atlas, nx, ny, tick + t as u64 * 5);
        } else {
            draw_boulder(atlas, world, nx, ny);
        }
        let letter = (b'A' + (world * 4 + t) as u8) as char;
        draw_text(atlas, nx + 4.0, ny + 18.0, &letter.to_string());
    }
    // Cursor: Rockford stands over the selected town, bobbing.
    let bob = if (tick / 16).is_multiple_of(2) { 0.0 } else { 2.0 };
    let (cx, cy) = NODES[cursor];
    draw_rockford(r, cx, cy - 18.0 - bob, suit, 1);

    draw_text(atlas, 8.0, 226.0, &format!("SCORE {:06}", score.min(999_999)));
    let lv = format!("LIVES {}", lives);
    draw_text(atlas, 248.0 - lv.len() as f32 * 8.0, 226.0, &lv);
}

/// Pre-cave status card (ROM state 8): the decoded "1 PLAYER"/"WORLD 00-00"
/// card with live digits; lives at row 10 col 18, world digits row 14 col
/// 13-14, town digit row 14 col 18 (STATUS_RECORDS addresses).
#[allow(clippy::too_many_arguments)]
pub fn draw_precave(
    r: &Renderer,
    cave_idx: usize,
    quest: usize,
    diamonds_needed: u32,
    time_units: u32,
    lives: u8,
    player: usize,
    two_player: bool,
    tick: u64,
) {
    blit(&r.screens.status);
    let atlas = &r.atlas;
    let world = cave_idx / 4;
    let town = cave_idx % 4;
    draw_text(atlas, 144.0, 80.0, &lives.to_string());
    draw_text(atlas, 104.0, 112.0, &format!("{:02}", world));
    draw_text(atlas, 144.0, 112.0, &(town + 1).to_string());
    if two_player && player == 1 {
        draw_text(atlas, 224.0, 8.0, "P2");
    }
    let letter = (b'A' + cave_idx as u8) as char;
    text_c(atlas, 168.0, &format!("CAVE {letter} QUEST {}", quest + 1));
    text_c(atlas, 184.0, &format!("DIAMONDS {diamonds_needed:02} TIME {time_units:03}"));
    if blink(tick) {
        text_c(atlas, 208.0, "PRESS A");
    }
}

/// Cave-complete tally (ROM state 12): banner + time bonus counting up.
/// Drawn over the frozen cave scene. The bonus is already in `total_score`;
/// `shown` is the tally animation value.
pub fn draw_clear_tally(atlas: &Atlas, shown: u32, total_score: u32, tick: u64) {
    draw_rectangle(216.0, 152.0, 208.0, 76.0, Color::new(0.0, 0.0, 0.0, 0.78));
    if blink(tick) {
        draw_text(atlas, 320.0 - 5.0 * 8.0, 160.0, "CAVE CLEAR");
    }
    let bonus = format!("TIME BONUS {shown:03}");
    draw_text(atlas, 320.0 - bonus.len() as f32 * 4.0, 192.0, &bonus);
    let score = format!("SCORE {:06}", total_score.min(999_999));
    draw_text(atlas, 320.0 - score.len() as f32 * 4.0, 208.0, &score);
}

/// Game over / continue (ROM state 14): the decoded screen + the password
/// for the current progress; CONTINUE (row 9) / END (row 10) selection.
pub fn draw_gameover(r: &Renderer, password: &[u8; 6], sel: usize, player: usize, two_player: bool, tick: u64) {
    blit(&r.screens.game_over);
    let atlas = &r.atlas;
    if two_player {
        draw_text(atlas, 224.0, 8.0, &format!("P{}", player + 1));
    }
    // Password for the current progress, under the baked "PASS WORD*".
    for (i, d) in password.iter().enumerate() {
        draw_text(atlas, 104.0 + i as f32 * 8.0, 112.0, &d.to_string());
    }
    // CONTINUE/END: redraw over the baked glyphs, selected = accent + cursor.
    text_pal(atlas, 96.0, 72.0, "CONTINUE", if sel == 0 { PAL_ACCENT } else { PAL_TEXT });
    text_pal(atlas, 96.0, 80.0, "END", if sel == 1 { PAL_ACCENT } else { PAL_TEXT });
    draw_diamond(atlas, 76.0, 70.0 + sel as f32 * 8.0, tick);
}

/// World/quest advance splash (ROM state 18): the decoded per-quest
/// interstitial ("TRY THE NEXT LEVEL* / PASS WORD ****** / PUSH START").
pub fn draw_quest_splash(r: &Renderer, world: usize, quest: usize, _tick: u64) {
    let _ = world;
    blit(&r.screens.splash[quest.min(3)]);
}

/// Ending screen (ROM state 17): the decoded world-1 credits card (all six
/// worlds roll through the same flow; the credits differ per world in the
/// ROM, we show world 1's) plus the final score.
pub fn draw_ending(r: &Renderer, score: u32, tick: u64) {
    blit(&r.screens.ending[0]);
    draw_text(&r.atlas, 8.0, 8.0, &format!("SCORE {:06}", score.min(999_999)));
    if blink(tick) {
        draw_text(&r.atlas, 168.0, 8.0, "PRESS START");
    }
}

/// "DEMO" label in the HUD bar during attract mode.
pub fn draw_demo_label(atlas: &Atlas, tick: u64) {
    if blink(tick) {
        draw_text(atlas, 304.0, 4.0, "DEMO");
    }
}

/// Total score (banked + in-cave) over the HUD's per-cave score field.
pub fn draw_total_score(atlas: &Atlas, total: u32) {
    let s = format!("{:06}", total.min(999_999));
    let w = boulder_dash::render::hud::text_width(&s);
    draw_rectangle(636.0 - w - 4.0, 0.0, w + 8.0, 26.0, BLACK);
    draw_text(atlas, 636.0 - w, 4.0, &s);
}
