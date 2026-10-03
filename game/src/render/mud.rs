//! HD dirt (mud) panoramas rendered in Blender (`blender/mud.blend`,
//! EEVEE): a single-mesh field of ~9000 dirt clods over the whole cave
//! (40x22 cells), with per-clod vertex-color tints. Three switchable
//! styles, each a full 2560x1408 panorama (64 px per cell) covering the
//! entire field — zero seams, zero repetition:
//!   0 = orange soil (classic world-1 dirt), 1 = grassy moss, 2 = gray loam.
//! The F key cycles the style (see `Renderer::mud_variant`).

use macroquad::prelude::*;

use crate::engine::WIDTH;

/// Panorama pixels per cell edge.
const CELL_PX: f32 = 64.0;

pub const MUD_VARIANTS: usize = 3;

fn load_jpg(bytes: &[u8]) -> Texture2D {
    // macroquad's bundled `image` build lacks JPEG; decode via the direct
    // dependency (same crate, jpeg feature enabled).
    let decoded = image::load_from_memory(bytes)
        .expect("mud jpg decodes")
        .to_rgba8();
    let (w, h) = decoded.dimensions();
    let tex = Texture2D::from_image(&Image {
        bytes: decoded.into_raw(),
        width: w as u16,
        height: h as u16,
    });
    tex.set_filter(FilterMode::Linear);
    tex
}

pub struct MudArt {
    panoramas: [Texture2D; MUD_VARIANTS],
}

impl Default for MudArt {
    fn default() -> Self {
        Self::new()
    }
}

impl MudArt {
    pub fn new() -> MudArt {
        MudArt {
            panoramas: [
                load_jpg(include_bytes!("../../assets/mud_orange.jpg")),
                load_jpg(include_bytes!("../../assets/mud_grass.jpg")),
                load_jpg(include_bytes!("../../assets/mud_loam.jpg")),
            ],
        }
    }

    /// Draw cell `cell_idx`'s slice of panorama `variant` in the `size` px
    /// cell at px (x, y).
    pub fn draw(&self, variant: usize, cell_idx: usize, x: f32, y: f32, size: f32) {
        let cx = (cell_idx % WIDTH) as f32;
        let cy = (cell_idx / WIDTH) as f32;
        draw_texture_ex(
            &self.panoramas[variant % MUD_VARIANTS],
            x,
            y,
            WHITE,
            DrawTextureParams {
                source: Some(Rect::new(cx * CELL_PX, cy * CELL_PX, CELL_PX, CELL_PX)),
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            },
        );
    }
}
