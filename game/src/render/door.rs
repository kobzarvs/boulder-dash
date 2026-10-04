//! HD exit door rendered in Blender (`blender/door.blend`, EEVEE): the cave's
//! exit as a stargate — a metal ring with eight chevrons around an event
//! horizon. Two rows in `game/assets/door.png` (2048x256, 16 columns of
//! 128px frames):
//! - row 0: inactive gate (closed door) — dark interior disc, dim chevrons;
//! - row 1: active gate (quota met) — animated watery event horizon (4D
//!   noise evolving over the frames) with lit orange chevron cores.
//!
//! The opening moment itself is covered by the full-screen door flash; the
//! open row then loops forever at half rate (32 ticks per swirl cycle).

use macroquad::prelude::*;

/// Atlas columns (frames per row).
const FRAMES: usize = 16;
/// One atlas frame edge, px.
const FRAME_PX: f32 = 128.0;
/// Closed (inactive) row.
const ROW_CLOSED: usize = 0;
/// Open (active) row.
const ROW_OPEN: usize = 1;

pub struct DoorArt {
    atlas: Texture2D,
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
        DoorArt { atlas }
    }

    /// Draw the gate in the `size` px cell at px (x, y): closed shows the
    /// static inactive frame, open loops the event-horizon swirl.
    pub fn draw(&self, open: bool, frame: u64, x: f32, y: f32, size: f32) {
        let row = if open { ROW_OPEN } else { ROW_CLOSED } as f32;
        let col = if open { ((frame / 2) % FRAMES as u64) as usize } else { 0 } as f32;
        draw_texture_ex(
            &self.atlas,
            x,
            y,
            WHITE,
            DrawTextureParams {
                source: Some(Rect::new(col * FRAME_PX, row * FRAME_PX, FRAME_PX, FRAME_PX)),
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            },
        );
    }
}
