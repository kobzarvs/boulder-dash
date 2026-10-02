//! Headless, deterministic simulation core for the NES Boulder Dash remake
//! (Data East 1990), reconciled with the Ghidra disassembly
//! (see `analysis/ghidra_findings.md`; all mechanics below are confirmed
//! unless marked otherwise).
//!
//! # Timing model
//!
//! [`Cave::tick`] advances exactly one NMI frame (60 Hz assumed; the ROM has
//! no PAL/NTSC detection, so on PAL hardware everything simply runs at 50 Hz).
//! Object movement is gated by the original's counters:
//!
//! - Field physics is phased on `frame & 7` ($C419): phases 1-4 run the
//!   bottom-up fall scan over row bands 20-16 / 15-11 / 10-6 / 5-1, phases
//!   5-7 run the top-down cleanup scan over rows 1-7 / 8-14 / 15-20. Each
//!   cell is therefore updated once per 8-frame cycle: boulders, diamonds and
//!   enemies move at <= 1 cell per 8 frames (7.5 Hz @ 60 fps).
//! - Rockford walks one cell per 8 frames (2 px/frame over 16 px cells).
//! - Cave timer decrements once per 64 frames ($CFCC: `$1E & $3F == 0`).
//! - Magic wall active window: 4096 frames ($B4 wraps 0 -> 255, decremented
//!   every 16 frames while active).
//! - Rockford/enemy adjacency kill check: every 8th frame ($C379).
//! - Amoeba: one 25-probe spiral chunk per 8 frames ($CE57).
//! - Boulder push: 24-frame hold ($C278); snap-push via button is instant.
//! - Death arc: 80 frames ($92=$50), then the cave reloads if reserve lives
//!   remain.
//!
//! There is no RNG anywhere in the original engine; the `seed` parameters in
//! the API are reserved (unused) for cosmetic layers.
//!
//! Rendering notes for the parallel render agent: per-object 2-bit palette
//! indices live at $F453 (informational only; the engine does not model
//! palettes), and there is NO shake-before-falling animation in the original,
//! so the engine deliberately emits no such event.

mod cave;
mod cell;
mod event;
mod field;
mod input;

pub mod ascii;

pub use cave::{Cave, CaveStatus};
pub use cell::{Cell, Obj};
pub use event::{DeathCause, Event, SoundCue};
pub use field::Field;
pub use input::{Direction, Input};

/// Cave field width in cells.
pub const WIDTH: usize = 40;
/// Cave field height in cells.
pub const HEIGHT: usize = 22;
/// Total cells in a cave field (row-major, index = y * WIDTH + x).
pub const CELLS: usize = WIDTH * HEIGHT; // 880

/// Assumed frame rate (NTSC). The ROM does no region detection, so the same
/// counters simply tick at 50 Hz on PAL hardware.
pub const FRAMES_PER_SECOND: u32 = 60;

/// Length of one field-physics cycle in frames (`frame & 7` phase dispatch).
pub const CYCLE_FRAMES: u64 = 8;
/// One displayed cave-time unit = 64 NMI frames (~1.07 s @ 60 Hz).
pub const FRAMES_PER_TIME_UNIT: u64 = 64;
/// Frames for Rockford to walk one cell (2 px/frame over 16 px cells).
pub const ROCKFORD_MOVE_FRAMES: u8 = 8;
/// Frames Rockford must hold against a boulder before the push executes.
pub const PUSH_HOLD_FRAMES: u8 = 24;
/// Frames both buttons must be held for the suicide combo.
pub const SUICIDE_HOLD_FRAMES: u8 = 64;
/// Death arc length before the life is deducted and the cave reloads.
pub const DEATH_ARC_FRAMES: u8 = 80;
/// Magic wall active window after the first falling boulder touches it.
pub const MAGIC_WALL_FRAMES: u32 = 4096;
/// Frames an explosion remnant ($A0) lingers before clearing.
pub const EXPLOSION_LINGER_FRAMES: u8 = 16;
/// Frames a pending diamond ($90|slot) takes to ripen.
pub const PENDING_DIAMOND_FRAMES: u8 = 16;
/// Rotating pending-diamond timer slots ($07F8-$07FF).
pub const PENDING_SLOTS: usize = 8;
/// Hurry-up music trigger: this many time units left.
pub const HURRY_UP_UNITS: u32 = 30;

/// Score interval granting an extra life ($CCE8: threshold starts at 2000,
/// += 2000 per award).
pub const EXTRA_LIFE_EVERY: u32 = 2000;
/// Reserve lives at start ($77 = 3; the manual's "4 Rockfords" = 1 active +
/// 3 reserve).
pub const START_RESERVE_LIVES: u8 = 3;
/// Maximum reserve lives (clamped at 9).
pub const MAX_LIVES: u8 = 9;

/// Firefly explosion score by cave group (`cave_index / 4`), table $C99A.
pub const EXPLOSION_SCORE: [u32; 6] = [200, 250, 300, 350, 400, 450];

/// Amoeba spiral scan: probes per chunk (one chunk per 8 frames).
pub const AMOEBA_PROBES_PER_CHUNK: usize = 25;
/// Chunks per full pass; a pass with no empty probe found => enclosed.
pub const AMOEBA_CHUNKS_PER_PASS: u8 = 4;
/// Initial growth pacing ($BA/$BD): every 255th empty probe grows a cell.
pub const AMOEBA_INTERVAL_START: u8 = 255;
/// The probe interval shrinks by this much per successful growth.
pub const AMOEBA_INTERVAL_STEP: u8 = 4;

#[cfg(test)]
mod tests;
