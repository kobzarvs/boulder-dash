//! HD wall sprites rendered in Blender (`blender/wall.blend`, Cycles): aged
//! running-bond brick masonry — per-brick color variation, efflorescence
//! spots, worn surface bump, old mortar. Two styles sharing one geometry:
//! `brick` (aged red-brown clay) for the STEEL boundary walls and `cement`
//! (light gray concrete) for the destructible BRICK walls.
//!
//! Each 256x256 atlas is ONE continuous 2x2 cell window cut from a larger
//! masonry field, so both the brick courses and the weathering continue
//! seamlessly across cell boundaries: the draw picks the quadrant by
//! `(cx % 2, cy % 2)` and the pattern repeats every 2 cells in lockstep
//! with the brick bond itself. No per-cell outline. Walls are opaque and
//! never slide.

use macroquad::prelude::*;

use crate::engine::WIDTH;

/// One cell-quadrant edge in the atlas, px.
const FRAME_PX: f32 = 128.0;

pub struct WallArt {
    brick: Texture2D,
    cement: Texture2D,
}

impl Default for WallArt {
    fn default() -> Self {
        Self::new()
    }
}

fn load(bytes: &[u8]) -> Texture2D {
    let tex = Texture2D::from_file_with_format(bytes, Some(ImageFormat::Png));
    tex.set_filter(FilterMode::Linear);
    tex
}

impl WallArt {
    pub fn new() -> WallArt {
        WallArt {
            brick: load(include_bytes!("../../assets/wall_brick.png")),
            cement: load(include_bytes!("../../assets/wall_cement.png")),
        }
    }

    /// The steel boundary wall: aged red-brown brick.
    pub fn draw_steel(&self, cell_idx: usize, x: f32, y: f32, size: f32) {
        self.draw(&self.brick, cell_idx, x, y, size);
    }

    /// The destructible brick wall: same masonry in light cement gray.
    pub fn draw_brick(&self, cell_idx: usize, x: f32, y: f32, size: f32) {
        self.draw(&self.cement, cell_idx, x, y, size);
    }

    /// Draw cell `cell_idx`'s tile from `atlas`: the quadrant of the
    /// continuous 2x2-cell masonry window.
    fn draw(&self, atlas: &Texture2D, cell_idx: usize, x: f32, y: f32, size: f32) {
        let qx = (cell_idx % WIDTH % 2) as f32;
        let qy = (cell_idx / WIDTH % 2) as f32;
        draw_texture_ex(
            atlas,
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
