//! Generic row-of-frames sprite atlas for the small HD remasters
//! (firefly, butterfly, amoeba, magic wall): `FRAMES` 128px columns per
//! row, one row per variant/state. Blender EEVEE renders.

use macroquad::prelude::*;

/// One atlas frame edge, px.
pub const FRAME_PX: f32 = 128.0;
/// Frames per row in every row-atlas.
pub const FRAMES: u64 = 8;

pub struct RowAtlas {
    tex: Texture2D,
}

impl RowAtlas {
    pub fn new(bytes: &[u8]) -> RowAtlas {
        let tex = Texture2D::from_file_with_format(bytes, Some(ImageFormat::Png));
        tex.set_filter(FilterMode::Linear);
        RowAtlas { tex }
    }

    /// Draw row `row`, frame `tick / div` (looping), in the `size` px cell
    /// at px (x, y). `phase` offsets the loop (per-cell desync for blobs).
    pub fn draw(&self, row: usize, tick: u64, div: u64, phase: u64, x: f32, y: f32, size: f32) {
        let f = ((tick / div + phase) % FRAMES) as f32;
        draw_texture_ex(
            &self.tex,
            x,
            y,
            WHITE,
            DrawTextureParams {
                source: Some(Rect::new(f * FRAME_PX, row as f32 * FRAME_PX, FRAME_PX, FRAME_PX)),
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            },
        );
    }
}
