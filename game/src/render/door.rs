//! HD exit door rendered in Blender (`blender/door.blend`, EEVEE): the cave's
//! exit as a stargate — a glyph ring with eight chevrons filling the whole
//! cell. `game/assets/door.png` (256x128) holds two 128px frames: the calm
//! gate (dim chevron cores, hollow — the cave backdrop shows through) and
//! the active gate (lit cores).
//!
//! The active event horizon is NOT baked into the atlas: a real-time GLSL
//! shader draws the watery wall inside the ring — fbm ripples spiralling
//! inwards towards a white-hot core, as if the portal pulls matter (and
//! travellers) into itself. The ring sprite is drawn over the shader disc,
//! whose edges tuck under the ring's inner rim.

use macroquad::miniquad::UniformType;
use macroquad::prelude::*;

/// One atlas frame edge, px; frame 0 = calm ring, frame 1 = lit ring.
const FRAME_PX: f32 = 128.0;
/// Horizon disc diameter as a fraction of the cell. The measured clear
/// opening is 0.67; the disc is a touch wider so its rim tucks under the
/// ring instead of leaving a backdrop gap.
const HORIZON: f32 = 0.72;

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
    float ang = atan(p.y, p.x);
    // Spiral arms winding tighter towards the core, slowly rotating; a
    // constant phase drifts to r = 0 over time, so the ripples STREAM
    // INWARDS — the suck-into-the-portal read.
    float sa = ang + (1.0 - r) * 2.5 - time * 0.8;
    float bands = fbm(vec2(sa * 2.0, r * 6.0 + time * 2.4));
    // Watery shimmer on top.
    float shim = fbm(p * 3.5 + vec2(time * 0.25, -time * 0.2));
    float n = bands * 0.7 + shim * 0.3;

    vec3 deep = vec3(0.02, 0.09, 0.32);
    vec3 mid = vec3(0.10, 0.45, 0.95);
    vec3 hi = vec3(0.65, 0.88, 1.0);
    vec3 col = mix(deep, mid, smoothstep(0.25, 0.7, n));
    col = mix(col, hi, smoothstep(0.68, 0.95, n) * 0.85);
    // White-hot core the matter falls into.
    float core = pow(max(0.0, 1.0 - r * 1.5), 2.0);
    col += vec3(1.0, 0.98, 0.92) * core * (1.1 + 0.25 * sin(time * 2.6));
    // Bright meniscus right at the rim (surface tension line).
    float rim = smoothstep(0.10, 0.0, 1.0 - r);
    col = mix(col, vec3(0.45, 0.8, 1.0), rim * 0.7);

    float alpha = smoothstep(1.0, 0.96, r);
    gl_FragColor = vec4(col, alpha);
}
"#;

pub struct DoorArt {
    atlas: Texture2D,
    /// 1 px binding for the horizon material's draw call (unused by it).
    white: Texture2D,
    horizon: Material,
}

impl Default for DoorArt {
    fn default() -> Self {
        Self::new()
    }
}

impl DoorArt {
    pub fn new() -> DoorArt {
        let atlas = Texture2D::from_file_with_format(
            include_bytes!("../../assets/door.png"),
            Some(ImageFormat::Png),
        );
        atlas.set_filter(FilterMode::Linear);
        let white = Texture2D::from_image(&Image::gen_image_color(1, 1, WHITE));
        let horizon = load_material(
            ShaderSource::Glsl { vertex: VERT, fragment: FRAG },
            MaterialParams {
                uniforms: vec![UniformDesc::new("time", UniformType::Float1)],
                ..Default::default()
            },
        )
        .expect("event horizon material");
        DoorArt { atlas, white, horizon }
    }

    /// Draw the gate in the `size` px cell at px (x, y). Closed is the bare
    /// ring with the backdrop showing through; open adds the shader event
    /// horizon under the lit ring.
    pub fn draw(&self, open: bool, frame: u64, x: f32, y: f32, size: f32) {
        if open {
            let hs = size * HORIZON;
            let hx = x + (size - hs) / 2.0;
            let hy = y + (size - hs) / 2.0;
            self.horizon.set_uniform("time", frame as f32 / 60.0);
            gl_use_material(&self.horizon);
            draw_texture_ex(
                &self.white,
                hx,
                hy,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(hs, hs)),
                    ..Default::default()
                },
            );
            gl_use_default_material();
        }
        draw_texture_ex(
            &self.atlas,
            x,
            y,
            WHITE,
            DrawTextureParams {
                source: Some(Rect::new(if open { FRAME_PX } else { 0.0 }, 0.0, FRAME_PX, FRAME_PX)),
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            },
        );
    }
}
