//! Attract-mode demo scripts, copied verbatim from the ROM (Q11).
//!
//! Each script is a list of `(input, duration)` byte pairs at CPU
//! $BF42/$BF90/$BFAE (pointer table $BF3C): the input byte is held for
//! `duration * 8` frames, then the next pair loads. `00 FF` terminates.
//! Input byte layout matches the `$9A` joystick shadow: high nibble = d-pad
//! ($10 up / $20 down / $40 left / $80 right), low bits = buttons
//! ($01 = A, $02 = B; both together = suicide combo). The demos replay real
//! engine gameplay of caves A/C/D and stay in sync because the engine has no
//! RNG (Q4).

use boulder_dash::engine::Input;

/// Demo caves per script index ($73 -> $DB4D[$73] = $75 values 00/20/30),
/// mapped to cave indices 0/2/3 = caves A/C/D.
pub const DEMO_CAVES: [usize; 3] = [0, 2, 3];

/// Script 0 (cave A), ROM $BF42-$BF8F.
const SCRIPT_A: [u8; 78] = [
    0x10, 0x01, 0x80, 0x08, 0x20, 0x01, 0x80, 0x0A, 0x10, 0x01, 0x40, 0x02, 0x80, 0x04, 0x20, 0x01,
    0x80, 0x02, 0x10, 0x01, 0x80, 0x08, 0x20, 0x0A, 0x80, 0x03, 0x20, 0x02, 0x40, 0x04, 0x10, 0x04,
    0x40, 0x03, 0x10, 0x01, 0x40, 0x05, 0x20, 0x04, 0x40, 0x04, 0x10, 0x03, 0x40, 0x08, 0x10, 0x01,
    0x40, 0x04, 0x20, 0x01, 0x40, 0x07, 0x20, 0x06, 0x80, 0x13, 0x20, 0x01, 0x80, 0x03, 0x20, 0x01,
    0x80, 0x05, 0x20, 0x02, 0x80, 0x05, 0x10, 0x04, 0x80, 0x05, 0x20, 0x01, 0x00, 0xFF,
];

/// Script 1 (cave C), ROM $BF90-$BFAB (followed by padding). Note the `00 50`
/// pair mid-script: a 640-frame deliberate pause, not a terminator (only
/// `00 FF` ends a script).
const SCRIPT_C: [u8; 28] = [
    0x10, 0x01, 0x80, 0x05, 0x10, 0x01, 0x20, 0x02, 0x80, 0x04, 0x20, 0x07, 0x00, 0x50, 0x80, 0x0A,
    0x10, 0x08, 0x80, 0x05, 0x10, 0x01, 0x20, 0x0B, 0x40, 0x03, 0x00, 0xFF,
];

/// Script 2 (cave D), ROM $BFAE-$BFCF.
const SCRIPT_D: [u8; 34] = [
    0x20, 0x03, 0x40, 0x05, 0x20, 0x05, 0x40, 0x04, 0x20, 0x0A, 0x80, 0x12, 0x00, 0x08, 0x11, 0x01,
    0x80, 0x05, 0x10, 0x02, 0x40, 0x01, 0x10, 0x06, 0x80, 0x02, 0x00, 0x10, 0x40, 0x02, 0x20, 0x08,
    0x00, 0xFF,
];

pub const DEMO_SCRIPTS: [&[u8]; 3] = [&SCRIPT_A, &SCRIPT_C, &SCRIPT_D];

/// Decode one `$9A` input byte into an engine [`Input`].
fn decode_input(byte: u8) -> Input {
    let a = byte & 0x01 != 0;
    let b = byte & 0x02 != 0;
    Input {
        up: byte & 0x10 != 0,
        down: byte & 0x20 != 0,
        left: byte & 0x40 != 0,
        right: byte & 0x80 != 0,
        grab: a || b,
        suicide: a && b,
    }
}

/// Parse a ROM script into `(input, frames)` holds, stopping at `00 FF`.
pub fn parse_script(script: &[u8]) -> Vec<(Input, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < script.len() {
        let (byte, dur) = (script[i], script[i + 1]);
        if byte == 0 && dur == 0xFF {
            break;
        }
        out.push((decode_input(byte), dur as u32 * 8));
        i += 2;
    }
    out
}

/// Feeds one demo script into the engine, one input per tick (`demo_input_feed`
/// $BF0D: input loads every frame, the duration counts down every 8 frames —
/// precomputed here as a flat frame count per pair).
pub struct DemoFeeder {
    holds: Vec<(Input, u32)>,
    idx: usize,
    left: u32,
}

impl DemoFeeder {
    pub fn new(demo: usize) -> DemoFeeder {
        let holds = parse_script(DEMO_SCRIPTS[demo % DEMO_SCRIPTS.len()]);
        let left = holds.first().map(|&(_, n)| n).unwrap_or(0);
        DemoFeeder { holds, idx: 0, left }
    }

    /// Input to apply for the current tick.
    pub fn input(&self) -> Input {
        self.holds.get(self.idx).map(|&(i, _)| i).unwrap_or(Input::NONE)
    }

    /// Advance one tick.
    pub fn advance(&mut self) {
        if self.idx >= self.holds.len() {
            return;
        }
        self.left = self.left.saturating_sub(1);
        if self.left == 0 {
            self.idx += 1;
            self.left = self.holds.get(self.idx).map(|&(_, n)| n).unwrap_or(0);
        }
    }

    /// The `00 FF` terminator was consumed: demo is over.
    pub fn finished(&self) -> bool {
        self.idx >= self.holds.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_parse_to_terminator() {
        // Hold counts from the raw ROM dumps (terminator excluded).
        assert_eq!(parse_script(&SCRIPT_A).len(), 38);
        assert_eq!(parse_script(&SCRIPT_C).len(), 13);
        assert_eq!(parse_script(&SCRIPT_D).len(), 16);
    }

    #[test]
    fn zero_input_with_normal_duration_is_not_a_terminator() {
        // SCRIPT_C holds "no input" for 0x50*8 frames mid-script.
        let holds = parse_script(&SCRIPT_C);
        assert!(holds.iter().any(|&(i, n)| i == Input::NONE && n == 0x50 * 8));
    }

    #[test]
    fn input_byte_bits() {
        let holds = parse_script(&SCRIPT_A);
        assert!(holds[0].0.up && !holds[0].0.grab);
        assert!(holds[1].0.right);
        // 0x11 in SCRIPT_D = up + A (snap/dig in place).
        let d = parse_script(&SCRIPT_D);
        let up_grab = d.iter().find(|&&(i, _)| i.up && i.grab);
        assert!(up_grab.is_some());
    }

    #[test]
    fn feeder_walks_holds_and_finishes() {
        let mut f = DemoFeeder::new(0);
        assert!(f.input().up);
        // First hold: up for 1*8 = 8 ticks.
        for _ in 0..8 {
            f.advance();
        }
        assert!(f.input().right);
        let total: u32 = parse_script(&SCRIPT_A).iter().map(|&(_, n)| n).sum();
        for _ in 8..total {
            f.advance();
        }
        assert!(f.finished());
        assert_eq!(f.input(), Input::NONE);
    }
}
