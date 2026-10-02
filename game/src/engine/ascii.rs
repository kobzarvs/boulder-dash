//! ASCII-art field builder, for tests and tooling.
//!
//! Maps are drawn on the interior (38x20) of a field and wrapped in a steel
//! border, so ascii cell (x, y) is field cell (x + 1, y + 1); use
//! [`ascii_idx`] to convert.

use crate::data::cave_params::CaveParam;

use super::cave::Cave;
use super::{CELLS, HEIGHT, WIDTH};

/// Field index of ascii-art cell (x, y).
pub fn ascii_idx(x: usize, y: usize) -> usize {
    (y + 1) * WIDTH + (x + 1)
}

/// Cell characters:
/// `' '` space, `.` mud, `#` steel, `+` brick, `~` magic wall, `o` boulder,
/// `*` diamond, `f` firefly, `q` butterfly, `a` amoeba, `r` Rockford,
/// `x` door.
pub fn cells_from_ascii(rows: &[&str]) -> [u8; CELLS] {
    assert!(rows.len() <= HEIGHT - 2, "ascii map too tall");
    let mut ids = [3u8; CELLS]; // steel
    for y in 1..HEIGHT - 1 {
        for x in 1..WIDTH - 1 {
            ids[y * WIDTH + x] = 0;
        }
    }
    for (y, row) in rows.iter().enumerate() {
        assert!(row.chars().count() <= WIDTH - 2, "ascii map row too wide");
        for (x, c) in row.chars().enumerate() {
            ids[(y + 1) * WIDTH + (x + 1)] = match c {
                ' ' => 0,
                '.' => 2,
                '#' => 3,
                'x' => 4,
                '+' => 5,
                '~' => 6,
                'o' => 7,
                '*' => 8,
                'f' => 0xB,
                'q' => 0xC,
                'a' => 0xD,
                'r' => 0xE,
                other => panic!("unknown ascii cell '{other}'"),
            };
        }
    }
    ids
}

/// Reasonable default parameters for synthetic test caves. The spawn cell is
/// unused for ascii caves (Rockford comes from an explicit `r`); it points at
/// interior cell (1, 1) if a test does use `Cave::from_cells` directly.
pub fn ascii_params() -> CaveParam {
    CaveParam {
        spawn_row: 1,
        spawn_col: 1,
        diamond_points: 10,
        diamond_points_bonus: 15,
        diamonds_needed: [2, 2, 2, 2],
        cave_time: [100, 100, 100, 100],
    }
}

/// Build a cave from ascii art at difficulty level 1. Rockford exists only if
/// the map contains `r` (so physics-only maps stay Rockford-free).
pub fn ascii_cave(rows: &[&str], params: CaveParam) -> Cave {
    Cave::from_cells_unspawned(&cells_from_ascii(rows), params, 0, 1)
}
