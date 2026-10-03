//! HD boulder sprites rendered in Blender (`blender/boulder.blend`, Cycles):
//! one 2048x1024 atlas (`game/assets/boulder_spin.png`) holding 16 frames of
//! a full revolution for 4 rock variants x 2 spin axes. Because every frame
//! is path-traced with the scene lights fixed in world space, highlights and
//! shadows sweep across the surface as the boulder turns — a plain sprite
//! rotation can't do that.
//!
//! Rows: `variant * 2 + axis` (axis 0 = Roll, 1 = Tumble), 16 columns each.
//! - Roll: rotation around the view axis, top of the rock leading — a boulder
//!   pushed/rolling right spins clockwise on screen; left is the same frames
//!   played backwards (negative rate).
//! - Tumble: forward somersault toward the camera, top tipping down over the
//!   front edge — the fall of a boulder that was at rest.

use macroquad::prelude::*;

pub const BOULDER_VARIANTS: usize = 4;
/// Frames per full revolution in the atlas (per axis row).
pub const SPIN_FRAMES: usize = 16;
/// One atlas frame edge, px.
const FRAME_PX: f32 = 128.0;

/// Spin rate in atlas frames per crossed cell: rolling without slipping for
/// a rock ~0.9 cells across (circumference ~2.83 cells -> 16/2.83 per cell).
pub const SPIN_PER_CELL: f32 = 5.66;

/// Which pre-rendered axis row to draw.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum SpinAxis {
    /// Screen-plane roll (horizontal travel, or a fall that continues one).
    Roll,
    /// Forward somersault toward the camera (fall from rest).
    #[default]
    Tumble,
}

/// Current spin pose: axis row + accumulated rotation in frame units
/// (16.0 = one revolution; may be negative when rolling left).
#[derive(Clone, Copy, Default)]
pub struct Spin {
    pub axis: SpinAxis,
    pub angle: f32,
}

pub struct BoulderArt {
    atlas: Texture2D,
}

impl Default for BoulderArt {
    fn default() -> Self {
        Self::new()
    }
}

impl BoulderArt {
    pub fn new() -> BoulderArt {
        let atlas = Texture2D::from_file_with_format(
            include_bytes!("../../assets/boulder_spin.png"),
            Some(ImageFormat::Png),
        );
        atlas.set_filter(FilterMode::Linear);
        BoulderArt { atlas }
    }

    /// Stable per-cell variant: the same cell keeps the same rock for the
    /// whole cave run (a falling boulder must not change shape mid-slide).
    pub fn variant(cell_idx: usize) -> usize {
        (cell_idx.wrapping_mul(2_654_435_761) >> 30) % BOULDER_VARIANTS
    }

    /// Draw cell `cell_idx`'s rock at spin pose `spin`, in the `size` px
    /// cell at px (x, y).
    pub fn draw(&self, cell_idx: usize, spin: Spin, x: f32, y: f32, size: f32) {
        let frame = (spin.angle.round() as i64).rem_euclid(SPIN_FRAMES as i64) as f32;
        let row = (Self::variant(cell_idx) * 2 + (spin.axis == SpinAxis::Tumble) as usize) as f32;
        draw_texture_ex(
            &self.atlas,
            x,
            y,
            WHITE,
            DrawTextureParams {
                source: Some(Rect::new(frame * FRAME_PX, row * FRAME_PX, FRAME_PX, FRAME_PX)),
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            },
        );
    }
}
