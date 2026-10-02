//! Per-tick player input.

/// Cardinal direction. Discriminant values match the 2-bit direction encoding
/// stored in cell flags.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Up = 0,
    Right = 1,
    Down = 2,
    Left = 3,
}

impl Direction {
    /// 90° clockwise turn.
    pub fn cw(self) -> Direction {
        match self {
            Direction::Up => Direction::Right,
            Direction::Right => Direction::Down,
            Direction::Down => Direction::Left,
            Direction::Left => Direction::Up,
        }
    }

    /// 90° counter-clockwise turn.
    pub fn ccw(self) -> Direction {
        match self {
            Direction::Up => Direction::Left,
            Direction::Left => Direction::Down,
            Direction::Down => Direction::Right,
            Direction::Right => Direction::Up,
        }
    }

    /// "Turn left" relative to heading; the absolute sense depends on the
    /// configured rotation convention (`Config::firefly_ccw`).
    pub fn turn_left(self, ccw: bool) -> Direction {
        if ccw { self.ccw() } else { self.cw() }
    }

    /// "Turn right" relative to heading; see [`Direction::turn_left`].
    pub fn turn_right(self, ccw: bool) -> Direction {
        if ccw { self.cw() } else { self.ccw() }
    }

    pub fn delta(self) -> (i32, i32) {
        match self {
            Direction::Up => (0, -1),
            Direction::Right => (1, 0),
            Direction::Down => (0, 1),
            Direction::Left => (-1, 0),
        }
    }

    pub fn is_horizontal(self) -> bool {
        matches!(self, Direction::Left | Direction::Right)
    }

    pub(crate) fn from_bits(v: u8) -> Direction {
        match v & 3 {
            0 => Direction::Up,
            1 => Direction::Right,
            2 => Direction::Down,
            _ => Direction::Left,
        }
    }
}

/// Buttons sampled for one tick. `grab` is A-or-B (act on the adjacent cell
/// without moving); `suicide` is A+B held together.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Input {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub grab: bool,
    pub suicide: bool,
}

impl Input {
    pub const NONE: Input = Input {
        up: false,
        down: false,
        left: false,
        right: false,
        grab: false,
        suicide: false,
    };

    /// Resolve the directional pad to a single direction. Diagonal input:
    /// horizontal wins. Opposing directions cancel.
    pub fn direction(&self) -> Option<Direction> {
        let dx = self.right as i8 - self.left as i8;
        let dy = self.down as i8 - self.up as i8;
        if dx > 0 {
            Some(Direction::Right)
        } else if dx < 0 {
            Some(Direction::Left)
        } else if dy > 0 {
            Some(Direction::Down)
        } else if dy < 0 {
            Some(Direction::Up)
        } else {
            None
        }
    }
}
