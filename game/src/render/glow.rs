//! Additive glow halo around diamonds: a soft icy-blue light that
//! illuminates the cells around the gem, pulsing gently in sync with the
//! sparkle loop. Rendered with a custom material whose blend state adds
//! the glow onto the framebuffer (src*alpha + dst) — real shader work,
//! not a pre-baked overlay. The halo texture is a procedural radial
//! falloff generated at startup.

use macroquad::miniquad::{BlendFactor, BlendState, BlendValue, Equation};
use macroquad::prelude::*;

/// Halo diameter in cells (the glow reaches ~1.6 cells from the gem).
const GLOW_CELLS: f32 = 3.2;
/// Halo texture edge, px.
const TEX_PX: u16 = 128;
/// Peak opacity at the glow center (several halos may overlap additively).
const CORE_ALPHA: f32 = 0.42;
/// Icy diamond light.
const TINT: (f32, f32, f32) = (0.65, 0.78, 1.0);

const VERT: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
varying lowp vec2 uv;
varying lowp vec4 color;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
}
"#;

const FRAG: &str = r#"#version 100
precision lowp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
void main() {
    gl_FragColor = texture2D(Texture, uv) * color;
}
"#;

pub struct DiamondGlow {
    tex: Texture2D,
    mat: Material,
}

impl Default for DiamondGlow {
    fn default() -> Self {
        Self::new()
    }
}

fn radial_texture() -> Texture2D {
    let mut img = Image::gen_image_color(TEX_PX, TEX_PX, Color::new(1.0, 1.0, 1.0, 0.0));
    let c = (TEX_PX as f32 - 1.0) / 2.0;
    for y in 0..TEX_PX {
        for x in 0..TEX_PX {
            let dx = (x as f32 - c) / c;
            let dy = (y as f32 - c) / c;
            let r = (dx * dx + dy * dy).sqrt().min(1.0);
            let a = CORE_ALPHA * (1.0 - r).powf(1.2);
            img.set_pixel(x as u32, y as u32, Color::new(1.0, 1.0, 1.0, a));
        }
    }
    let tex = Texture2D::from_image(&img);
    tex.set_filter(FilterMode::Linear);
    tex
}

impl DiamondGlow {
    pub fn new() -> DiamondGlow {
        let mat = load_material(
            ShaderSource::Glsl { vertex: VERT, fragment: FRAG },
            MaterialParams {
                pipeline_params: PipelineParams {
                    color_blend: Some(BlendState::new(
                        Equation::Add,
                        BlendFactor::Value(BlendValue::SourceAlpha),
                        BlendFactor::One,
                    )),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("glow material");
        DiamondGlow {
            tex: radial_texture(),
            mat,
        }
    }

    /// Draw the halo centered on the `cell` px diamond at px (cx, cy).
    /// Intensity breathes with the 120-tick sparkle loop.
    pub fn draw(&self, tick: u64, cx: f32, cy: f32, cell: f32) {
        let pulse = (tick as f32 * std::f32::consts::TAU / 120.0).sin();
        let intensity = 0.40 + 0.05 * pulse;
        let size = cell * GLOW_CELLS;
        draw_texture_ex(
            &self.tex,
            cx - size / 2.0,
            cy - size / 2.0,
            Color::new(TINT.0, TINT.1, TINT.2, intensity),
            DrawTextureParams {
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            },
        );
    }

    /// Set the additive material; pair with `reset` after the glow batch.
    pub fn apply_material(&self) {
        gl_use_material(&self.mat);
    }

    pub fn reset_material(&self) {
        gl_use_default_material();
    }
}
