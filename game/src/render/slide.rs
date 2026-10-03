//! Smooth cell-to-cell sliding for movable objects (boulders, diamonds,
//! fireflies, butterflies). The engine moves objects one cell per 8-frame
//! cycle instantly; this tracker diffs the field between consecutive ticks
//! and interpolates the visual position between the source and destination
//! cells, the same way RockfordAnim slides Rockford.
//!
//! Boulders additionally carry SPIN state (see `boulder.rs`): each slide
//! records the spin axis, the accumulated angle at slide start and the
//! angular rate, and the resting spin is written back to the destination
//! cell when the slide ends. The state travels with the boulder, so a
//! rolling boulder keeps its rotation direction when it starts falling,
//! while a boulder that was at rest tumbles forward toward the camera.

use crate::engine::{Cave, Obj, CELLS, WIDTH};
use std::collections::HashMap;

use super::boulder::{Spin, SpinAxis, SPIN_PER_CELL};

/// Duration of one slide in engine ticks (matches the 8-frame move cadence).
pub const SLIDE_TICKS: u64 = 8;

/// Objects that get slide interpolation.
fn movable(obj: Obj) -> bool {
    matches!(obj, Obj::Boulder | Obj::Diamond | Obj::Firefly | Obj::Butterfly)
}

/// Spin recorded on an active slide (boulders only).
#[derive(Clone, Copy)]
struct SlideSpin {
    axis: SpinAxis,
    /// Accumulated angle at slide start, in frame units (16 = revolution).
    frame0: f32,
    /// Angular rate in frames per cell; negative = rolling left.
    rate: f32,
}

/// Spin state of a boulder at rest in a cell: the pose it settled in, plus
/// the rate of the last motion (its sign decides how a following fall
/// keeps rotating).
#[derive(Clone, Copy)]
struct RestSpin {
    axis: SpinAxis,
    angle: f32,
    rate: f32,
}

struct Slide {
    obj: Obj,
    from: usize,
    to: usize,
    start: u64,
    spin: Option<SlideSpin>,
}

impl Slide {
    /// Resting spin this slide leaves behind at its destination cell.
    fn settle(&self) -> Option<(usize, RestSpin)> {
        self.spin.map(|s| {
            (
                self.to,
                RestSpin {
                    axis: s.axis,
                    angle: s.frame0 + s.rate,
                    rate: s.rate,
                },
            )
        })
    }
}

pub struct SlideTracker {
    prev: Vec<Obj>,
    slides: Vec<Slide>,
    /// Resting spins by cell (boulders that have moved at least once).
    rest_spins: HashMap<usize, RestSpin>,
}

impl Default for SlideTracker {
    fn default() -> Self {
        SlideTracker {
            prev: vec![Obj::Space; CELLS],
            slides: Vec::new(),
            rest_spins: HashMap::new(),
        }
    }
}

impl SlideTracker {
    /// Forget all movement (cave load, respawn, player swap).
    pub fn reset(&mut self, cave: &Cave) {
        for i in 0..CELLS {
            self.prev[i] = cave.cell_at_idx(i).obj;
        }
        self.slides.clear();
        self.rest_spins.clear();
    }

    /// Advance one engine tick: pair departures with adjacent arrivals.
    pub fn update(&mut self, cave: &Cave, tick: u64) {
        // Retire finished slides, settling their boulders' spin at rest.
        let mut kept = Vec::with_capacity(self.slides.len());
        for s in self.slides.drain(..) {
            if tick.saturating_sub(s.start) < SLIDE_TICKS {
                kept.push(s);
            } else if let Some((idx, rest)) = s.settle() {
                self.rest_spins.insert(idx, rest);
            }
        }
        self.slides = kept;
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
                    // The object keeps moving through `src`: drop the older
                    // slide that was arriving there, or the same object would
                    // be drawn twice (e.g. push then immediate fall). Settle
                    // its spin first so the new slide picks up the angle.
                    if let Some(pos) = self.slides.iter().position(|s| s.to == src) {
                        let old = self.slides.remove(pos);
                        if let Some((at, rest)) = old.settle() {
                            self.rest_spins.insert(at, rest);
                        }
                    }
                    let spin = if cur == Obj::Boulder {
                        let prev = self.rest_spins.remove(&src);
                        let dx = x as i32 - (src % WIDTH) as i32;
                        Some(next_spin(prev, dx))
                    } else {
                        None
                    };
                    self.slides.push(Slide { obj: cur, from: src, to: idx, start: tick, spin });
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
            let t = slide_t(s.start, tick);
            let (fx, fy) = ((s.from % WIDTH) as f32, (s.from / WIDTH) as f32);
            let (tx, ty) = ((s.to % WIDTH) as f32, (s.to / WIDTH) as f32);
            (s.obj, fx + (tx - fx) * t, fy + (ty - fy) * t)
        })
    }

    /// Spin pose of a boulder sliding INTO `idx` (default pose if none).
    pub fn spin_for(&self, idx: usize, tick: u64) -> Spin {
        self.slides
            .iter()
            .find(|s| s.to == idx)
            .and_then(|s| s.spin.map(|sp| (s, sp)))
            .map(|(s, sp)| Spin {
                axis: sp.axis,
                angle: sp.frame0 + slide_t(s.start, tick) * sp.rate,
            })
            .unwrap_or_default()
    }

    /// Settled spin pose of a boulder resting in `idx` (default if it has
    /// never moved: tumble frame 0, the never-rotated sprite).
    pub fn rest_spin(&self, idx: usize) -> Spin {
        self.rest_spins
            .get(&idx)
            .map(|r| Spin { axis: r.axis, angle: r.angle })
            .unwrap_or_default()
    }
}

/// Slide progress 0..1 for a slide started at `start` (shared by `slide_for`
/// and `spin_for` so position and rotation advance together).
fn slide_t(start: u64, tick: u64) -> f32 {
    ((tick.saturating_sub(start) + 1).min(SLIDE_TICKS)) as f32 / SLIDE_TICKS as f32
}

/// Spin for a boulder's new slide. Horizontal travel (incl. diagonal
/// roll-offs) is a Roll in the travel direction; a vertical fall keeps a
/// previous roll's axis AND direction, or — from rest — tumbles forward
/// toward the camera.
fn next_spin(prev: Option<RestSpin>, dx: i32) -> SlideSpin {
    let frame0 = prev.map_or(0.0, |p| p.angle);
    if dx != 0 {
        SlideSpin {
            axis: SpinAxis::Roll,
            frame0,
            rate: SPIN_PER_CELL * dx.signum() as f32,
        }
    } else {
        match prev {
            Some(p) if p.axis == SpinAxis::Roll => SlideSpin {
                axis: SpinAxis::Roll,
                frame0,
                rate: p.rate,
            },
            _ => SlideSpin {
                axis: SpinAxis::Tumble,
                frame0,
                rate: SPIN_PER_CELL,
            },
        }
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
    fn walking_firefly_slides() {        // Rockford far away in the corner (default spawn is (1,1)).
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

    /// Push-then-immediate-fall: the engine moves the boulder twice in one
    /// cycle (push, then the fall scan takes it further). The older slide
    /// must be cancelled, or the boulder is drawn twice.
    #[test]
    fn chained_move_has_single_slide() {
        // Rockford at (1,1), boulder at (3,1), space beyond, space below.
        let cells = cells_from_ascii(&[
            "r o   ",
            "      ",
            "      ",
        ]);
        let mut cave = Cave::from_cells(&cells, ascii_params(), 0, 1, 0);
        let mut slides = SlideTracker::default();
        slides.reset(&cave);
        for tick in 0..16 {
            let input = if tick == 0 {
                Input { right: true, grab: true, ..Input::NONE }
            } else {
                Input::NONE
            };
            cave.tick(input);
            slides.update(&cave, tick);
            assert!(
                slides.slides.len() <= 1,
                "tick {tick}: {} overlapping slides for one boulder",
                slides.slides.len()
            );
        }
    }

    /// A boulder that was at rest TUMBLES toward the camera when it falls.
    #[test]
    fn fall_from_rest_tumbles_forward() {
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
        let under = ascii_idx(2, 2);
        let mut last_angle = 0.0;
        for tick in 0..8 {
            cave.tick(Input::NONE);
            slides.update(&cave, tick);
            let spin = slides.spin_for(under, tick);
            assert_eq!(spin.axis, SpinAxis::Tumble, "tick {tick}: fall from rest must tumble");
            assert!(
                spin.angle >= last_angle,
                "tick {tick}: tumble angle must increase ({last_angle} -> {})",
                spin.angle
            );
            last_angle = spin.angle;
        }
        assert!(last_angle > 0.0, "tumble must accumulate angle");
    }

    /// A pushed boulder ROLLS in the push direction, and when it then falls
    /// it keeps the same axis and direction; the angle is continuous across
    /// the roll -> fall transition.
    #[test]
    fn roll_keeps_direction_into_fall() {
        // Rockford (1,1) pushes the boulder (2,1) right into open space;
        // the brick under it keeps it put until pushed, then it falls.
        let cells = cells_from_ascii(&[
            "ro   ",
            "++   ",
        ]);
        let mut cave = Cave::from_cells(&cells, ascii_params(), 0, 1, 0);
        let mut slides = SlideTracker::default();
        slides.reset(&cave);
        let mut saw_roll = false;
        for tick in 0..24 {
            let input = if tick < 8 {
                Input { right: true, grab: true, ..Input::NONE }
            } else {
                Input::NONE
            };
            cave.tick(input);
            slides.update(&cave, tick);
            for idx in 0..CELLS {
                if let Some((Obj::Boulder, _, _)) = slides.slide_for(idx, tick) {
                    let spin = slides.spin_for(idx, tick);
                    assert_eq!(spin.axis, SpinAxis::Roll, "tick {tick}: pushed boulder must roll");
                    assert!(spin.angle >= 0.0, "tick {tick}: rightward roll must spin forward");
                    saw_roll = true;
                }
            }
        }
        assert!(saw_roll, "no boulder slide observed");
    }

    /// Rolling left spins backwards (negative rate).
    #[test]
    fn roll_left_spins_backwards() {
        // Boulder at (4,1) on brick, Rockford at (5,1) pushes it left.
        let cells = cells_from_ascii(&[
            "   or",
            "   ++",
        ]);
        let mut cave = Cave::from_cells(&cells, ascii_params(), 0, 1, 0);
        let mut slides = SlideTracker::default();
        slides.reset(&cave);
        for tick in 0..16 {
            let input = if tick < 8 {
                Input { left: true, grab: true, ..Input::NONE }
            } else {
                Input::NONE
            };
            cave.tick(input);
            slides.update(&cave, tick);
            for idx in 0..CELLS {
                if let Some((Obj::Boulder, _, _)) = slides.slide_for(idx, tick) {
                    let spin = slides.spin_for(idx, tick);
                    assert_eq!(spin.axis, SpinAxis::Roll);
                    assert!(spin.angle <= 0.0, "tick {tick}: leftward roll must spin backwards");
                    return;
                }
            }
        }
        panic!("no leftward roll observed");
    }
}
