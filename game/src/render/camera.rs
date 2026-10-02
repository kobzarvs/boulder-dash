//! Smooth pixel-level camera following Rockford, clamped to the cave.

/// Cave pixel size: 40x22 cells of 16 px.
pub const CAVE_W: f32 = 640.0;
pub const CAVE_H: f32 = 352.0;
/// Visible cave viewport (below the 32 px HUD bar).
pub const VIEW_W: f32 = 256.0;
pub const VIEW_H: f32 = 208.0;

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

    fn clamped(cx: f32, cy: f32) -> (f32, f32) {
        (
            cx.clamp(0.0, CAVE_W - VIEW_W),
            cy.clamp(0.0, CAVE_H - VIEW_H),
        )
    }

    /// Jump straight to the target (cave load / respawn).
    pub fn snap(&mut self, px: f32, py: f32) {
        let (x, y) = Self::clamped(px - (VIEW_W - 16.0) / 2.0, py - (VIEW_H - 16.0) / 2.0);
        self.x = x;
        self.y = y;
    }

    /// Ease towards the target; `dt` is real frame time in seconds.
    pub fn follow(&mut self, px: f32, py: f32, dt: f32) {
        let (tx, ty) = Self::clamped(px - (VIEW_W - 16.0) / 2.0, py - (VIEW_H - 16.0) / 2.0);
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
