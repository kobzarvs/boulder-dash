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
    /// Inside-corner gradient (max of the two edge falloffs).
    corner: Texture2D,
    /// Soft radial blob for boulder ground shadows.
    blob: Texture2D,
}

impl Default for WallShadow {
    fn default() -> Self {
        Self::new()
    }
}

/// Shadow falloff curve: 1 at the contact edge, 0 at the reach end, ZERO
/// SLOPE at both ends (smoothstep) — a hard-slope cutoff reads as a line
/// (Mach band) along the whole dirt edge.
fn falloff(i: u16) -> f32 {
    let t = (i as f32 / (TEX_PX as f32 * REACH)).clamp(0.0, 1.0);
    let s = 1.0 - t;
    s * s * (1.0 + 2.0 * t)
}

fn ramp_texture(horizontal: bool) -> Texture2D {
    let mut img = Image::gen_image_color(TEX_PX, TEX_PX, Color::new(0.0, 0.0, 0.0, 0.0));
    for i in 0..TEX_PX {
        let a = STRENGTH * falloff(i);
        for j in 0..TEX_PX {
            let (x, y) = if horizontal { (i, j) } else { (j, i) };
            img.set_pixel(x as u32, y as u32, Color::new(0.0, 0.0, 0.0, a));
        }
    }
    let tex = Texture2D::from_image(&img);
    tex.set_filter(FilterMode::Linear);
    tex
}

/// Opacity at the boulder shadow's core.
const BLOB_ALPHA: f32 = 0.34;

fn blob_texture() -> Texture2D {
    let mut img = Image::gen_image_color(TEX_PX, TEX_PX, Color::new(0.0, 0.0, 0.0, 0.0));
    let c = (TEX_PX as f32 - 1.0) / 2.0;
    for y in 0..TEX_PX {
        for x in 0..TEX_PX {
            let dx = (x as f32 - c) / c;
            let dy = (y as f32 - c) / c;
            let r = (dx * dx + dy * dy).sqrt().min(1.0);
            let s = 1.0 - r;
            let a = BLOB_ALPHA * s * s * (1.0 + 2.0 * r);
            img.set_pixel(x as u32, y as u32, Color::new(0.0, 0.0, 0.0, a));
        }
    }
    let tex = Texture2D::from_image(&img);
    tex.set_filter(FilterMode::Linear);
    tex
}

/// Corner shadow texture for inside corners: alpha = f(x)+f(y)-f(x)f(y)
/// (soft probabilistic union of the two edge falloffs) — strongest right
/// in the corner, continuous across the diagonal (unlike max(), which
/// creases a visible line along x=y), never doubling.
fn corner_texture() -> Texture2D {
    let mut img = Image::gen_image_color(TEX_PX, TEX_PX, Color::new(0.0, 0.0, 0.0, 0.0));
    for y in 0..TEX_PX {
        for x in 0..TEX_PX {
            let (fx, fy) = (falloff(x), falloff(y));
            let a = STRENGTH * (fx + fy - fx * fy);
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
            corner: corner_texture(),
            blob: blob_texture(),
        }
    }

    /// Boulder's cast shadow: a soft blob offset down-right from the
    /// boulder's cell (the light is upper-left), so most of it falls on
    /// the ground and only its edge kisses the rock's base.
    pub fn draw_boulder_shadow(&self, cell_x: f32, cell_y: f32, cell: f32) {
        let size = cell * 1.35;
        let cx = cell_x + cell * (0.5 + 0.22);
        let cy = cell_y + cell * (0.5 + 0.28);
        draw_texture_ex(
            &self.blob,
            cx - size / 2.0,
            cy - size / 2.0,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            },
        );
    }

    /// Draw the shadows on the `size` px cell at px (x, y): edge strips
    /// for straight walls, the soft corner gradient for inside corners
    /// (at reduced strength for a diagonal-only touch). `variation`
    /// scales the strength per cell — a long straight wall edge must not
    /// cast a ruler-straight uniform band.
    pub fn draw(&self, left: bool, top: bool, diag: bool, x: f32, y: f32, size: f32, variation: f32) {
        let (tex, alpha) = match (left, top, diag) {
            (true, true, _) => (Some(&self.corner), 1.0),
            (true, false, _) => (Some(&self.left), 1.0),
            (false, true, _) => (Some(&self.top), 1.0),
            (false, false, true) => (Some(&self.corner), 0.55),
            _ => (None, 0.0),
        };
        if let Some(tex) = tex {
            draw_texture_ex(
                tex,
                x,
                y,
                Color::new(1.0, 1.0, 1.0, alpha * variation),
                DrawTextureParams {
                    dest_size: Some(vec2(size, size)),
                    ..Default::default()
                },
            );
        }
    }
}
