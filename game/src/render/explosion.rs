//! Explosion remnant effect: where the NES flashes a static quad while the
//! blast mark lingers, the HD variant smoulders — an fbm fire pocket with
//! ragged edges and flickering embers, phase-offset per cell so a blast
//! area never pulses in lockstep. Real-time GLSL, like the door horizon.

use macroquad::miniquad::UniformType;
use macroquad::prelude::*;

const VERT: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
varying lowp vec2 uv;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    uv = texcoord;
}
"#;

const FRAG: &str = r#"#version 100
precision highp float;
varying vec2 uv;
uniform float time;
uniform float seed;

float hash(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

float vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    float a = hash(i);
    float b = hash(i + vec2(1.0, 0.0));
    float c = hash(i + vec2(0.0, 1.0));
    float d = hash(i + vec2(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float fbm(vec2 p) {
    float v = 0.0;
    float a = 0.5;
    for (int i = 0; i < 4; i++) {
        v += a * vnoise(p);
        p = p * 2.03 + 17.0;
        a *= 0.5;
    }
    return v;
}

void main() {
    vec2 p = (uv - 0.5) * 2.0;
    float r = length(p);
    if (r >= 1.0) {
        gl_FragColor = vec4(0.0);
        return;
    }
    float t = time + seed * 13.7;
    // Embers crawl upwards (hot gases rise), flicker per cell.
    float n = fbm(p * 3.2 + vec2(seed, -t * 1.1));
    float flick = 0.8 + 0.2 * sin(t * 9.0 + n * 6.0);
    vec3 col = mix(vec3(0.12, 0.03, 0.01), vec3(0.85, 0.3, 0.04), smoothstep(0.3, 0.68, n));
    col = mix(col, vec3(1.0, 0.85, 0.4), smoothstep(0.72, 0.95, n));
    col *= flick;
    // Ragged scorch edge: the noise bites into the disc; the char stays
    // translucent so the cave floor shows through the burn.
    float alpha = smoothstep(1.1, 0.35, r + (n - 0.5) * 0.6) * 0.6;
    gl_FragColor = vec4(col, alpha);
}
"#;

pub struct ExplosionFx {
    /// 1 px binding for the material's draw call (unused by the shader).
    white: Texture2D,
    mat: Material,
}

impl Default for ExplosionFx {
    fn default() -> Self {
        Self::new()
    }
}

impl ExplosionFx {
    pub fn new() -> ExplosionFx {
        let white = Texture2D::from_image(&Image::gen_image_color(1, 1, WHITE));
        let mat = load_material(
            ShaderSource::Glsl { vertex: VERT, fragment: FRAG },
            MaterialParams {
                uniforms: vec![
                    UniformDesc::new("time", UniformType::Float1),
                    UniformDesc::new("seed", UniformType::Float1),
                ],
                ..Default::default()
            },
        )
        .expect("explosion material");
        ExplosionFx { white, mat }
    }

    /// Smouldering blast mark in the `size` px cell at px (x, y).
    pub fn draw(&self, cell_idx: usize, frame: u64, x: f32, y: f32, size: f32) {
        self.mat.set_uniform("time", frame as f32 / 60.0);
        self.mat
            .set_uniform("seed", (cell_idx.wrapping_mul(2_654_435_761) >> 8) as f32 % 256.0 / 256.0);
        gl_use_material(&self.mat);
        draw_texture_ex(
            &self.white,
            x,
            y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            },
        );
        gl_use_default_material();
    }
}
