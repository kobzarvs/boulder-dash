//! Smooth cell-to-cell sliding for movable objects (boulders, diamonds,
//! fireflies, butterflies). The engine moves objects one cell per 8-frame
//! cycle instantly; this tracker diffs the field between consecutive ticks
//! and interpolates the visual position between the source and destination
//! cells, the same way RockfordAnim slides Rockford.

use crate::engine::{Cave, Obj, CELLS, WIDTH};

/// Duration of one slide in engine ticks (matches the 8-frame move cadence).
pub const SLIDE_TICKS: u64 = 8;

/// Objects that get slide interpolation.
fn movable(obj: Obj) -> bool {
    matches!(obj, Obj::Boulder | Obj::Diamond | Obj::Firefly | Obj::Butterfly)
}

struct Slide {
    obj: Obj,
    from: usize,
    to: usize,
    start: u64,
}

pub struct SlideTracker {
    prev: Vec<Obj>,
    slides: Vec<Slide>,
}

impl Default for SlideTracker {
    fn default() -> Self {
        SlideTracker { prev: vec![Obj::Space; CELLS], slides: Vec::new() }
    }
}

impl SlideTracker {
    /// Forget all movement (cave load, respawn, player swap).
    pub fn reset(&mut self, cave: &Cave) {
        for i in 0..CELLS {
            self.prev[i] = cave.cell_at_idx(i).obj;
        }
        self.slides.clear();
    }

    /// Advance one engine tick: pair departures with adjacent arrivals.
    pub fn update(&mut self, cave: &Cave, tick: u64) {
        self.slides.retain(|s| tick.saturating_sub(s.start) < SLIDE_TICKS);
        let mut used_from = [false; CELLS];
        let mut used_to = [false; CELLS];
        for s in &self.slides {
            used_from[s.from] = true;
            used_to[s.to] = true;
        }
        for idx in 0..CELLS {
            let cur = cave.cell_at_idx(idx).obj;
            if !movable(cur) || cur == self.prev[idx] || used_to[idx] {
                continue;
            }
            // An arrival: `cur` was not here last tick. Look for the cell it
            // came from: above (fall), diagonals above (roll-off), sides
            // (push / enemy walk).
            let (x, y) = (idx % WIDTH, idx / WIDTH);
            let mut cands = Vec::with_capacity(5);
            if y > 0 {
                cands.push(idx - WIDTH); // up
                if x > 0 {
                    cands.push(idx - WIDTH - 1);
                }
                if x + 1 < WIDTH {
                    cands.push(idx - WIDTH + 1);
                }
            }
            if x > 0 {
                cands.push(idx - 1);
            }
            if x + 1 < WIDTH {
                cands.push(idx + 1);
            }
            for src in cands {
                if used_from[src] {
                    continue;
                }
                if self.prev[src] == cur && cave.cell_at_idx(src).obj != cur {
                    self.slides.push(Slide { obj: cur, from: src, to: idx, start: tick });
                    used_from[src] = true;
                    used_to[idx] = true;
                    break;
                }
            }
        }
        for i in 0..CELLS {
            self.prev[i] = cave.cell_at_idx(i).obj;
        }
    }

    /// Fractional cell position of an object sliding INTO `idx`, if any.
    pub fn slide_for(&self, idx: usize, tick: u64) -> Option<(Obj, f32, f32)> {
        self.slides.iter().find(|s| s.to == idx).map(|s| {
            let t = ((tick.saturating_sub(s.start) + 1).min(SLIDE_TICKS)) as f32
                / SLIDE_TICKS as f32;
            let (fx, fy) = ((s.from % WIDTH) as f32, (s.from / WIDTH) as f32);
            let (tx, ty) = ((s.to % WIDTH) as f32, (s.to / WIDTH) as f32);
            (s.obj, fx + (tx - fx) * t, fy + (ty - fy) * t)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ascii::{ascii_idx, ascii_params, cells_from_ascii};
    use crate::engine::{Cave, Input};

    /// A boulder suspended over space must get a slide from its old cell to
    /// the new one within one 8-frame move cycle, at a fractional position.
    #[test]
    fn falling_boulder_slides() {
        let cells = cells_from_ascii(&[
            "     ",
            "  o  ",
            "     ",
            "     ",
            " r   ",
        ]);
        let mut cave = Cave::from_cells(&cells, ascii_params(), 0, 1, 0);
        let mut slides = SlideTracker::default();
        slides.reset(&cave);

        let from = ascii_idx(2, 1);
        let to = ascii_idx(2, 2);
        let mut seen_fractional = false;
        for tick in 0..8 {
            cave.tick(Input::NONE);
            slides.update(&cave, tick);
            if let Some((obj, _x, y)) = slides.slide_for(to, tick) {
                assert_eq!(obj, Obj::Boulder);
                let (from_y, to_y) = ((from / WIDTH) as f32, (to / WIDTH) as f32);
                assert!(y >= from_y && y < to_y, "slide pos {y} must be between cells");
                seen_fractional = true;
            }
        }
        assert!(seen_fractional, "no slide recorded for the falling boulder");
    }

    /// A firefly walking along a wall slides between cells.
    #[test]
    fn walking_firefly_slides() {
        // Rockford far away in the corner (default spawn is (1,1)).
        let cells = cells_from_ascii(&[
            "r   ",
            "    ",
            "    ",
            "    ",
            "    ",
            "++++",
            "f   ",
            "++++",
        ]);
        let mut cave = Cave::from_cells(&cells, ascii_params(), 0, 1, 0);
        let mut slides = SlideTracker::default();
        slides.reset(&cave);
        let mut found = false;
        // The firefly may turn in place a couple of cycles before moving.
        for tick in 0..48 {
            cave.tick(Input::NONE);
            slides.update(&cave, tick);
            for idx in 0..CELLS {
                if let Some((obj, _, _)) = slides.slide_for(idx, tick) {
                    if obj == Obj::Firefly {
                        found = true;
                    }
                }
            }
        }
        assert!(found, "no slide recorded for the walking firefly");
    }
}
