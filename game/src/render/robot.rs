//! HD robot character rendered in Blender (`blender/rockford.blend`, EEVEE):
//! one 2048x1792 atlas (`game/assets/robot_anim.png`) of 16-frame loops shot
//! from five azimuths around the character (0 = facing the camera, 90 =
//! facing screen right, 180 = back view; the other side is mirrored).
//!
//! Rows:
//! - 0-4: idle — stands straight, taps his RIGHT foot on the beat.
//! - 5-9: run — contact / passing poses, arms counter-swinging (azimuth rows).
//! - 10: push — leaning into the boulder, arms forward, legs braced (az 90).
//! - 11: fidget A — scratching his head (az 0 only, plays when settled).
//! - 12: fidget B — looking around (az 0 only).
//! - 13: fidget C — a full 360 dancing spin with hops (baked rotation).
//!
//! Direction changes are NOT pre-rendered: the render layer keeps a float
//! azimuth that eases toward the travel direction and picks the nearest
//! azimuth row, which reads as a smooth turn.

use macroquad::prelude::*;

/// Atlas columns (frames per loop).
pub const FRAMES: usize = 16;
/// One atlas frame edge, px.
const FRAME_PX: f32 = 128.0;

/// Drawn sprite height in cells: he stands noticeably taller than one cell.
pub const DRAW_CELLS: f32 = 1.25;
/// Feet position inside the frame, fraction from the top (feet at world
/// z=0; ortho frame covers z -0.15..1.75 -> 0.15/1.9 from the bottom).
pub const FEET_FRACTION: f32 = 1.0 - 0.15 / 1.9;
/// While pushing, shift the sprite this far towards the boulder (fraction of
/// the drawn size) so his hands overlap the rock instead of pushing air.
pub const PUSH_REACH: f32 = 0.22;

/// First idle row; five azimuth rows (0/45/90/135/180 deg).
pub const ROW_IDLE: usize = 0;
/// First run row; five azimuth rows.
pub const ROW_RUN: usize = 5;
/// Push row, shot at azimuth 90 (flip for the left side).
pub const ROW_PUSH: usize = 10;
/// Head-scratch fidget row (azimuth 0).
pub const ROW_SCRATCH: usize = 11;
/// Look-around fidget row (azimuth 0).
pub const ROW_LOOK: usize = 12;
/// Dancing spin fidget row (rotation baked into the frames).
pub const ROW_DANCE: usize = 13;

pub struct RobotArt {
    atlas: Texture2D,
}

impl Default for RobotArt {
    fn default() -> Self {
        Self::new()
    }
}

impl RobotArt {
    pub fn new() -> RobotArt {
        let atlas = Texture2D::from_file_with_format(
            include_bytes!("../../assets/robot_anim.png"),
            Some(ImageFormat::Png),
        );
        atlas.set_filter(FilterMode::Linear);
        RobotArt { atlas }
    }

    /// Azimuth row index 0-4 for a (wrapped) visual azimuth in degrees.
    pub fn az_row(azimuth: f32) -> usize {
        ((azimuth.abs() / 45.0).round() as usize).min(4)
    }

    /// Draw one atlas frame in the `size` px cell at px (x, y). `rotation`
    /// (radians) spins the sprite around the cell center (death arc); `alpha`
    /// fades it.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&self, row: usize, frame: usize, x: f32, y: f32, size: f32, flip_x: bool, rotation: f32, alpha: f32) {
        let mut color = WHITE;
        color.a = alpha.clamp(0.0, 1.0);
        draw_texture_ex(
            &self.atlas,
            x,
            y,
            color,
            DrawTextureParams {
                source: Some(Rect::new(
                    (frame % FRAMES) as f32 * FRAME_PX,
                    row as f32 * FRAME_PX,
                    FRAME_PX,
                    FRAME_PX,
                )),
                dest_size: Some(vec2(size, size)),
                flip_x,
                rotation,
                pivot: Some(vec2(x + size / 2.0, y + size / 2.0)),
                ..Default::default()
            },
        );
    }
}
