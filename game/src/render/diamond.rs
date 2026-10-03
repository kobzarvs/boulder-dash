//! HD diamond sprite rendered in Blender (`blender/diamond.blend`, Cycles):
//! a brilliant-cut gem with real dispersion (three-IOR glass), lit by a
//! jewelry light rig. The 3072x128 atlas (`game/assets/diamond_sparkle.png`)
//! holds 24 frames: the gem stays still while the light rig orbits it, so
//! glints and rainbow fire sweep across the facets — the HD counterpart of
//! the NES palette-shimmer sparkle. Diamonds don't rotate while falling;
//! the sparkle loop simply keeps playing.

use macroquad::prelude::*;

/// Sparkle frames in the atlas (one loop of the orbiting light rig).
pub const SPARKLE_FRAMES: u64 = 24;
/// One atlas frame edge, px.
const FRAME_PX: f32 = 128.0;
/// Engine ticks per atlas frame: 24 frames x 5 ticks = a 2-second loop.
const TICKS_PER_FRAME: u64 = 5;

pub struct DiamondArt {
    atlas: Texture2D,
}

impl Default for DiamondArt {
    fn default() -> Self {
        Self::new()
    }
}

impl DiamondArt {
    pub fn new() -> DiamondArt {
        let atlas = Texture2D::from_file_with_format(
            include_bytes!("../../assets/diamond_sparkle.png"),
            Some(ImageFormat::Png),
        );
        atlas.set_filter(FilterMode::Linear);
        DiamondArt { atlas }
    }

    /// Draw the sparkle frame for engine tick `tick` in the `size` px cell
    /// at px (x, y).
    pub fn draw(&self, tick: u64, x: f32, y: f32, size: f32) {
        let frame = ((tick / TICKS_PER_FRAME) % SPARKLE_FRAMES) as f32;
        draw_texture_ex(
            &self.atlas,
            x,
            y,
            WHITE,
            DrawTextureParams {
                source: Some(Rect::new(frame * FRAME_PX, 0.0, FRAME_PX, FRAME_PX)),
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            },
        );
    }
}
