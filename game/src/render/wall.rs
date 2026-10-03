//! HD steel (boundary) wall sprite rendered in Blender (`blender/wall.blend`,
//! Cycles): aged running-bond brick masonry — per-brick color variation,
//! efflorescence spots, worn surface bump, light old mortar.
//!
//! The 256x256 atlas (`game/assets/wall_brick.png`) is ONE continuous 2x2
//! cell window cut from a larger masonry field, so both the brick courses
//! and the weathering continue seamlessly across cell boundaries: the draw
//! picks the quadrant by `(cx % 2, cy % 2)` and the pattern repeats every
//! 2 cells in lockstep with the brick bond itself. No per-cell outline.
//! Walls are opaque and never slide.

use macroquad::prelude::*;

use crate::engine::WIDTH;

/// One cell-quadrant edge in the atlas, px.
const FRAME_PX: f32 = 128.0;

pub struct WallArt {
    atlas: Texture2D,
}

impl Default for WallArt {
    fn default() -> Self {
        Self::new()
    }
}

impl WallArt {
    pub fn new() -> WallArt {
        let atlas = Texture2D::from_file_with_format(
            include_bytes!("../../assets/wall_brick.png"),
            Some(ImageFormat::Png),
        );
        atlas.set_filter(FilterMode::Linear);
        WallArt { atlas }
    }

    /// Draw cell `cell_idx`'s wall tile in the `size` px cell at px (x, y):
    /// the quadrant of the continuous 2x2-cell masonry window.
    pub fn draw(&self, cell_idx: usize, x: f32, y: f32, size: f32) {
        let qx = (cell_idx % WIDTH % 2) as f32;
        let qy = (cell_idx / WIDTH % 2) as f32;
        draw_texture_ex(
            &self.atlas,
            x,
            y,
            WHITE,
            DrawTextureParams {
                source: Some(Rect::new(qx * FRAME_PX, qy * FRAME_PX, FRAME_PX, FRAME_PX)),
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            },
        );
    }
}
