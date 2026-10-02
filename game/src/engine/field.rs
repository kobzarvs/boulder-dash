//! The 40x22 cell field.

use super::cell::{Cell, Obj};
use super::input::Direction;
use super::{HEIGHT, WIDTH};

/// Row-major cave field: index = y * WIDTH + x.
#[derive(Clone)]
pub struct Field {
    pub cells: Vec<Cell>,
}

impl Field {
    pub fn idx(x: usize, y: usize) -> usize {
        y * WIDTH + x
    }

    pub fn xy(i: usize) -> (usize, usize) {
        (i % WIDTH, i / WIDTH)
    }

    /// Index of the neighbor of cell `i` in direction `d`, or None off-field.
    pub fn neighbor(&self, i: usize, d: Direction) -> Option<usize> {
        let (x, y) = Self::xy(i);
        let (dx, dy) = d.delta();
        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
        if nx < 0 || ny < 0 || nx >= WIDTH as i32 || ny >= HEIGHT as i32 {
            None
        } else {
            Some(Self::idx(nx as usize, ny as usize))
        }
    }

    pub fn count_obj(&self, obj: Obj) -> usize {
        self.cells.iter().filter(|c| c.obj == obj).count()
    }
}
