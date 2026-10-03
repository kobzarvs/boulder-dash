//! Smooth pixel-level camera following Rockford, clamped to the cave.

/// One cave cell in pixels (2x the NES's 16: the field scrolls again).
pub const CELL_PX: f32 = 32.0;
/// Cave pixel size: 40x22 cells.
pub const CAVE_W: f32 = 40.0 * CELL_PX;
pub const CAVE_H: f32 = 22.0 * CELL_PX;
/// Visible cave viewport (below the 48 px HUD bar, to the 416 px screen
/// bottom): 640x368 = 20x11.5 cells. The half-cell remainder means a
/// partially-visible row is always present at the top or bottom edge;
/// `draw_world` draws it and the HUD band / screen edge clip the bleed.
pub const VIEW_W: f32 = 640.0;
pub const VIEW_H: f32 = 368.0;

pub struct Camera {
    pub x: f32,
    pub y: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

impl Camera {
    pub fn new() -> Camera {
        Camera { x: 0.0, y: 0.0 }
    }

    /// Clamp on one axis; center the cave when the viewport is larger than it
    /// (both cave axes exceed the viewport here, so this always clamps into
    /// `0..=cave-view` in practice).
    fn clamp_axis(c: f32, cave: f32, view: f32) -> f32 {
        if view >= cave {
            (cave - view) / 2.0
        } else {
            c.clamp(0.0, cave - view)
        }
    }

    fn clamped(cx: f32, cy: f32) -> (f32, f32) {
        (
            Self::clamp_axis(cx, CAVE_W, VIEW_W),
            Self::clamp_axis(cy, CAVE_H, VIEW_H),
        )
    }

    /// Jump straight to the target (cave load / respawn).
    pub fn snap(&mut self, px: f32, py: f32) {
        let (x, y) = Self::clamped(px - (VIEW_W - CELL_PX) / 2.0, py - (VIEW_H - CELL_PX) / 2.0);
        self.x = x;
        self.y = y;
    }

    /// Ease towards the target; `dt` is real frame time in seconds.
    pub fn follow(&mut self, px: f32, py: f32, dt: f32) {
        let (tx, ty) = Self::clamped(px - (VIEW_W - CELL_PX) / 2.0, py - (VIEW_H - CELL_PX) / 2.0);
        // ~12 px/s convergence rate; fast enough to keep up with walking,
        // soft enough to feel smooth. Snap sub-pixel remainders.
        let k = (dt * 10.0).min(1.0);
        self.x += (tx - self.x) * k;
        self.y += (ty - self.y) * k;
        if (tx - self.x).abs() < 0.5 {
            self.x = tx;
        }
        if (ty - self.y).abs() < 0.5 {
            self.y = ty;
        }
    }
}
