//! Nametable blitter: pre-renders the decoded ROM screen nametables
//! (`data::screens`) to textures at startup, using the same pixel rules as
//! `xtask extract-screens`, so the flow screens match the reference PNGs in
//! `game/assets/screens/` exactly. Dynamic content (cursors, digits, scores)
//! is drawn over the blit by `flow::screens`.

use macroquad::prelude::*;

use crate::data::cave_params::NES_PALETTE;
use crate::data::screens::{self, Nametable};
use crate::data::tiles::TILES;

/// Render one decoded nametable to an RGBA image: 32x30 tiles of 8x8 px,
/// palette group per attribute quadrant, `bank` = CHR bank (pattern table).
/// Pixel value 0 uses the group's slot-0 color (descriptors keep the
/// universal backdrop $3F00 mirrored into every slot 0).
pub fn nametable_image(nt: &Nametable, bank: usize, palette: &[u8; 16]) -> Image {
    let mut img = Image::gen_image_color(256, 240, BLACK);
    for ty in 0..screens::SCREEN_ROWS {
        for tx in 0..screens::SCREEN_COLS {
            let tile = nt.tiles[ty * 32 + tx] as usize;
            let attr = nt.attrs[(ty / 4) * 8 + tx / 4];
            let shift = (if ty % 4 >= 2 { 4 } else { 0 }) + (if tx % 4 >= 2 { 2 } else { 0 });
            let group = ((attr >> shift) & 3) as usize;
            let px = &TILES[bank * 256 + tile];
            for r in 0..8 {
                for c in 0..8 {
                    let ci = px[r * 8 + c] as usize;
                    let [rr, gg, bb] = NES_PALETTE[(palette[group * 4 + ci] & 0x3F) as usize];
                    img.set_pixel(
                        (tx * 8 + c) as u32,
                        (ty * 8 + r) as u32,
                        Color::new(rr as f32 / 255.0, gg as f32 / 255.0, bb as f32 / 255.0, 1.0),
                    );
                }
            }
        }
    }
    img
}

/// Apply a 50-byte VRAM record's segments (`(ppu_addr, tile bytes)`) onto a
/// nametable (title menu text over the legal page, etc.).
fn apply_record(nt: &mut Nametable, record: &[(u16, &[u8])]) {
    for &(addr, bytes) in record {
        for (i, &tile) in bytes.iter().enumerate() {
            let a = addr as usize + i;
            if (0x2000..0x2000 + 960).contains(&a) {
                nt.tiles[a - 0x2000] = tile;
            }
        }
    }
}

/// Title screen with the attract menu text composited on top: TITLE_NT
/// (legal page) + ATTRACT_RECORDS 0-3 (score line, 1P/2P menu, "TM",
/// copyright) — what ROM state 1 shows on nametable $2000.
fn title_menu_image() -> Image {
    let mut nt = screens::TITLE_NT.clone();
    for rec in screens::ATTRACT_RECORDS.iter().take(4) {
        apply_record(&mut nt, rec);
    }
    nametable_image(&nt, 5, &screens::PAL_TITLE)
}

fn tex(img: Image) -> Texture2D {
    let t = Texture2D::from_image(&img);
    t.set_filter(FilterMode::Nearest);
    t
}

/// Every decoded screen as a ready-to-blit texture (built once at startup;
/// ~4.7 MB of VRAM). CHR banks and palettes per `data::screens` docs.
pub struct Screens {
    /// Title/legal + menu (bank 5, PAL_TITLE).
    pub title: Texture2D,
    /// World-map walk backdrop (bank 6, PAL_GAMEPLAY).
    pub map_walk: Texture2D,
    /// Per-world map intros (bank 6, PAL_GAMEPLAY).
    pub world_map: [Texture2D; 6],
    /// World-map hub (bank 6, PAL_MAP_HUB).
    pub map_hub: Texture2D,
    /// Per-world ending credits (bank 6, PAL_MAP_HUB).
    pub ending: [Texture2D; 6],
    /// Per-quest splash interstitials (bank 6, PAL_SPLASH).
    pub splash: [Texture2D; 4],
    /// Color select frame (bank 6, PAL_GAMEPLAY).
    pub color_select: Texture2D,
    /// Password entry (bank 6, PAL_GAMEPLAY; digits are sprite overlays).
    pub password: Texture2D,
    /// Pre-cave status card (bank 6, PAL_GAMEPLAY).
    pub status: Texture2D,
    /// Game over / continue (bank 6, PAL_GAMEPLAY).
    pub game_over: Texture2D,
}

impl Default for Screens {
    fn default() -> Self {
        Self::new()
    }
}

impl Screens {
    pub fn new() -> Screens {
        let g = &screens::PAL_GAMEPLAY;
        Screens {
            title: tex(title_menu_image()),
            map_walk: tex(nametable_image(&screens::MAP_WALK_NT, 6, g)),
            world_map: [
                tex(nametable_image(&screens::WORLD_MAP_NT_0, 6, g)),
                tex(nametable_image(&screens::WORLD_MAP_NT_1, 6, g)),
                tex(nametable_image(&screens::WORLD_MAP_NT_2, 6, g)),
                tex(nametable_image(&screens::WORLD_MAP_NT_3, 6, g)),
                tex(nametable_image(&screens::WORLD_MAP_NT_4, 6, g)),
                tex(nametable_image(&screens::WORLD_MAP_NT_5, 6, g)),
            ],
            map_hub: tex(nametable_image(&screens::MAP_HUB_NT, 6, &screens::PAL_MAP_HUB)),
            ending: [
                tex(nametable_image(&screens::ENDING_NT_0, 6, &screens::PAL_MAP_HUB)),
                tex(nametable_image(&screens::ENDING_NT_1, 6, &screens::PAL_MAP_HUB)),
                tex(nametable_image(&screens::ENDING_NT_2, 6, &screens::PAL_MAP_HUB)),
                tex(nametable_image(&screens::ENDING_NT_3, 6, &screens::PAL_MAP_HUB)),
                tex(nametable_image(&screens::ENDING_NT_4, 6, &screens::PAL_MAP_HUB)),
                tex(nametable_image(&screens::ENDING_NT_5, 6, &screens::PAL_MAP_HUB)),
            ],
            splash: [
                tex(nametable_image(&screens::SPLASH_NT_0, 6, &screens::PAL_SPLASH[0])),
                tex(nametable_image(&screens::SPLASH_NT_1, 6, &screens::PAL_SPLASH[0])),
                tex(nametable_image(&screens::SPLASH_NT_2, 6, &screens::PAL_SPLASH[1])),
                tex(nametable_image(&screens::SPLASH_NT_3, 6, &screens::PAL_SPLASH[2])),
            ],
            color_select: tex(nametable_image(&screens::COLOR_SELECT_NT, 6, g)),
            password: tex(nametable_image(&screens::PASSWORD_NT, 6, g)),
            status: tex(nametable_image(&screens::STATUS_NT, 6, g)),
            game_over: tex(nametable_image(&screens::GAME_OVER_NT, 6, g)),
        }
    }
}

/// Blit a screen texture at the top-left of the 256x240 frame.
pub fn blit(t: &Texture2D) {
    draw_texture(t, 0.0, 0.0, WHITE);
}
