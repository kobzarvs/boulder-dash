//! HD cave backdrop rendered in Blender (`blender/cave_bg.blend`, Cycles):
//! a field of ~13000 gravel pebbles scattered over the WHOLE cave
//! (40x22 cells, 80x44 units), lit by the same upper-left sun rig as the
//! brick walls. No tiling at all: the 2560x1408 panorama
//! (`game/assets/cave_bg.jpg`, 64 px per cell) covers the entire field, so
//! every cell draws its own unique piece — zero seams, zero repetition.
//! Replaces the Space and Vacated metatile quads (including under HD items
//! and sliding objects).

use macroquad::prelude::*;

use crate::engine::WIDTH;

/// Panorama pixels per cell edge.
const CELL_PX: f32 = 64.0;

pub struct BackdropArt {
    pano: Texture2D,
}

impl Default for BackdropArt {
    fn default() -> Self {
        Self::new()
    }
}

impl BackdropArt {
    pub fn new() -> BackdropArt {
        // macroquad's bundled `image` build lacks JPEG; decode via the
        // direct dependency (same crate, jpeg feature enabled).
        let decoded = image::load_from_memory(include_bytes!("../../assets/cave_bg.jpg"))
            .expect("cave_bg.jpg decodes")
            .to_rgba8();
        let (w, h) = decoded.dimensions();
        let pano = Texture2D::from_image(&Image {
            bytes: decoded.into_raw(),
            width: w as u16,
            height: h as u16,
        });
        pano.set_filter(FilterMode::Linear);
        BackdropArt { pano }
    }

    /// Draw cell `cell_idx`'s own slice of the panorama in the `size` px
    /// cell at px (x, y).
    pub fn draw(&self, cell_idx: usize, x: f32, y: f32, size: f32) {
        let cx = (cell_idx % WIDTH) as f32;
        let cy = (cell_idx / WIDTH) as f32;
        draw_texture_ex(
            &self.pano,
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
