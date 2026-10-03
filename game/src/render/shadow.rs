//! Contact shadows cast by walls (steel / brick) onto neighboring cells.
//! The scene light comes from the upper left, so a wall shades the cells
//! to its right and below: each cell next to a wall on its left or top
//! side gets a soft black gradient strip on that edge (both on corners).
//! Textures are tiny procedural alpha ramps, drawn with the usual sprite
//! pipeline — the same visual a shader AO pass would give, without GLSL.

use macroquad::prelude::*;

/// Shadow reach, as a fraction of the cell.
const REACH: f32 = 0.55;
/// Opacity at the contact edge.
const STRENGTH: f32 = 0.52;
/// Resolution of the gradient strip texture (one axis is 1 px).
const TEX_PX: u16 = 32;

pub struct WallShadow {
    /// Dark at the left edge, fading right.
    left: Texture2D,
    /// Dark at the top edge, fading down.
    top: Texture2D,
}

impl Default for WallShadow {
    fn default() -> Self {
        Self::new()
    }
}

fn ramp_texture(horizontal: bool) -> Texture2D {
    let mut img = Image::gen_image_color(TEX_PX, TEX_PX, Color::new(0.0, 0.0, 0.0, 0.0));
    let reach_px = TEX_PX as f32 * REACH;
    for i in 0..TEX_PX {
        let t = (i as f32 / reach_px).min(1.0);
        // Soft ease-out falloff.
        let a = STRENGTH * (1.0 - t).powf(1.5);
        for j in 0..TEX_PX {
            let (x, y) = if horizontal { (i, j) } else { (j, i) };
            img.set_pixel(x as u32, y as u32, Color::new(0.0, 0.0, 0.0, a));
        }
    }
    let tex = Texture2D::from_image(&img);
    tex.set_filter(FilterMode::Linear);
    tex
}

impl WallShadow {
    pub fn new() -> WallShadow {
        WallShadow {
            left: ramp_texture(true),
            top: ramp_texture(false),
        }
    }

    /// Draw the shadow strips on the `size` px cell at px (x, y).
    pub fn draw(&self, left: bool, top: bool, x: f32, y: f32, size: f32) {
        if left {
            draw_texture_ex(
                &self.left,
                x,
                y,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(size, size)),
                    ..Default::default()
                },
            );
        }
        if top {
            draw_texture_ex(
                &self.top,
                x,
                y,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(size, size)),
                    ..Default::default()
                },
            );
        }
    }
}
