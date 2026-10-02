//! Minimal NES APU (2A03) synth: 2 pulse channels, triangle, noise.
//!
//! Register-write driven: the sound-driver mirror in [`crate::audio::player`]
//! pokes virtual `$4000-$400F` registers via [`Apu::write`], exactly like the
//! original driver pokes the real chip. Rendering is mono f32 at 44100 Hz.
//!
//! Accuracy scope: timer/phase, hardware decay envelope, sweep unit, length
//! counter and the triangle linear counter are modeled. The DPCM channel is
//! absent (the game never uses it) and there is no frame-IRQ / $4015/$4017
//! handling (the driver never touches them after reset).

/// NTSC 2A03 CPU clock.
pub const CPU_HZ: f64 = 1_789_773.0;
/// Output sample rate.
pub const SAMPLE_RATE: u32 = 44_100;
/// NES frame (NMI) rate the sound driver is paced by.
pub const NMI_HZ: f64 = 60.0988;
/// CPU cycles per NMI frame.
pub const CYCLES_PER_FRAME: f64 = CPU_HZ / NMI_HZ; // ~29780.5

/// Envelope/linear-counter clock rate (APU frame counter steps 1-4).
const ENV_HZ: f64 = 240.0;
/// Length-counter and sweep clock rate (steps 2 and 4).
const LENGTH_SWEEP_HZ: f64 = 120.0;

const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 1, 0, 0, 0, 0, 0, 0], // 12.5%
    [0, 1, 1, 0, 0, 0, 0, 0], // 25%
    [0, 1, 1, 1, 1, 0, 0, 0], // 50%
    [1, 0, 0, 1, 1, 1, 1, 1], // 75% (25% inverted)
];

const TRIANGLE_SEQ: [u8; 32] = [
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, //
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

/// NTSC noise timer periods in CPU cycles.
const NOISE_PERIODS: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];

const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, //
    12, 16, 24, 18, 48, 20, 96, 22, 192, 24, 72, 26, 16, 28, 32, 30,
];

/// Hardware decay envelope shared by pulse and noise channels.
#[derive(Clone, Copy, Default)]
struct Envelope {
    /// $4000/$400C low nibble: constant volume or envelope period.
    param: u8,
    /// $400x bit4: constant-volume flag.
    constant: bool,
    /// $400x bit5: loop flag (also the length-counter halt flag).
    loop_flag: bool,
    start: bool,
    divider: u8,
    decay: u8,
}

impl Envelope {
    fn write(&mut self, val: u8) {
        self.param = val & 0x0F;
        self.constant = val & 0x10 != 0;
        self.loop_flag = val & 0x20 != 0;
    }

    /// 240 Hz clock.
    fn clock(&mut self) {
        if self.start {
            self.start = false;
            self.decay = 15;
            self.divider = self.param;
        } else if self.divider == 0 {
            self.divider = self.param;
            if self.decay == 0 {
                if self.loop_flag {
                    self.decay = 15;
                }
            } else {
                self.decay -= 1;
            }
        } else {
            self.divider -= 1;
        }
    }

    fn volume(&self) -> u8 {
        if self.constant {
            self.param
        } else {
            self.decay
        }
    }
}

/// Pulse channel ($4000-$4003 / $4004-$4007).
#[derive(Clone, Copy, Default)]
pub struct Pulse {
    env: Envelope,
    duty: u8,
    sweep_reg: u8,
    period: u16,
    timer: u16,
    seq_step: u8,
    length: u8,
    sweep_divider: u8,
    sweep_reload: bool,
    /// True for pulse1 (negate adds an extra -1 on the hardware).
    is_pulse1: bool,
}

impl Pulse {
    fn new(is_pulse1: bool) -> Self {
        Pulse {
            is_pulse1,
            ..Default::default()
        }
    }

    fn write(&mut self, reg: u8, val: u8) {
        match reg & 3 {
            0 => {
                self.env.write(val);
                self.duty = val >> 6;
            }
            1 => {
                self.sweep_reg = val;
                self.sweep_reload = true;
            }
            2 => self.period = (self.period & 0x700) | val as u16,
            _ => {
                self.period = (self.period & 0xFF) | (((val as u16) & 7) << 8);
                self.length = LENGTH_TABLE[(val >> 3) as usize];
                self.env.start = true;
            }
        }
    }

    fn sweep_enabled(&self) -> bool {
        self.sweep_reg & 0x80 != 0 && self.sweep_reg & 7 != 0
    }

    /// Target period after one sweep step; may exceed 11 bits.
    fn sweep_target(&self) -> i32 {
        let change = (self.period >> (self.sweep_reg & 7)) as i32;
        if self.sweep_reg & 0x08 != 0 {
            let extra = if self.is_pulse1 { 1 } else { 0 };
            self.period as i32 - change - extra
        } else {
            self.period as i32 + change
        }
    }

    fn muted(&self) -> bool {
        self.period < 8 || self.sweep_target() > 0x7FF
    }

    /// CPU-cycle clock: advances the 8-step duty sequencer.
    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period;
            self.seq_step = (self.seq_step + 1) & 7;
        } else {
            self.timer -= 1;
        }
    }

    /// 240 Hz clock.
    fn clock_envelope(&mut self) {
        self.env.clock();
    }

    /// 120 Hz clock.
    fn clock_length_sweep(&mut self) {
        if !self.env.loop_flag && self.length > 0 {
            self.length -= 1;
        }
        if self.sweep_reload {
            self.sweep_reload = false;
            self.sweep_divider = (self.sweep_reg >> 4) & 7;
        } else if self.sweep_divider == 0 {
            if self.sweep_enabled() && !self.muted() {
                self.period = self.sweep_target() as u16;
            }
            self.sweep_divider = (self.sweep_reg >> 4) & 7;
        } else {
            self.sweep_divider -= 1;
        }
    }

    fn output(&self) -> u8 {
        if self.length == 0 || self.muted() {
            return 0;
        }
        if DUTY_TABLE[self.duty as usize][self.seq_step as usize] == 0 {
            return 0;
        }
        self.env.volume()
    }
}

/// Triangle channel ($4008-$400B).
#[derive(Clone, Copy, Default)]
pub struct Triangle {
    /// $4008 bit7: control flag (halts both counters).
    control: bool,
    linear_reload: u8,
    linear_counter: u8,
    linear_reload_flag: bool,
    period: u16,
    timer: u16,
    seq_step: u8,
    length: u8,
}

impl Triangle {
    fn write(&mut self, reg: u8, val: u8) {
        match reg & 3 {
            0 => {
                self.control = val & 0x80 != 0;
                self.linear_reload = val & 0x7F;
            }
            2 => self.period = (self.period & 0x700) | val as u16,
            3 => {
                self.period = (self.period & 0xFF) | (((val as u16) & 7) << 8);
                self.length = LENGTH_TABLE[(val >> 3) as usize];
                self.linear_reload_flag = true;
            }
            _ => {}
        }
    }

    fn clock_timer(&mut self) {
        if self.linear_counter == 0 || self.length == 0 {
            return; // sequencer halted: frozen DC output
        }
        if self.timer == 0 {
            self.timer = self.period;
            self.seq_step = (self.seq_step + 1) & 31;
        } else {
            self.timer -= 1;
        }
    }

    /// 240 Hz clock.
    fn clock_linear(&mut self) {
        if self.linear_reload_flag {
            self.linear_counter = self.linear_reload;
        } else if self.linear_counter > 0 {
            self.linear_counter -= 1;
        }
        if !self.control {
            self.linear_reload_flag = false;
        }
    }

    /// 120 Hz clock.
    fn clock_length(&mut self) {
        if !self.control && self.length > 0 {
            self.length -= 1;
        }
    }

    fn output(&self) -> u8 {
        if self.length == 0 || self.linear_counter == 0 {
            return 0;
        }
        TRIANGLE_SEQ[self.seq_step as usize]
    }
}

/// Noise channel ($400C-$400F).
#[derive(Clone, Copy, Default)]
pub struct Noise {
    env: Envelope,
    mode: bool,
    period: u16,
    timer: u16,
    /// 15-bit LFSR; bit 0 clear = channel audibly "on".
    lfsr: u16,
    length: u8,
}

impl Noise {
    fn write(&mut self, reg: u8, val: u8) {
        match reg & 3 {
            0 => self.env.write(val),
            2 => {
                self.mode = val & 0x80 != 0;
                self.period = NOISE_PERIODS[(val & 0x0F) as usize];
            }
            3 => {
                self.length = LENGTH_TABLE[(val >> 3) as usize];
                self.env.start = true;
            }
            _ => {}
        }
    }

    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period;
            let tap = if self.mode {
                (self.lfsr >> 6) & 1
            } else {
                (self.lfsr >> 1) & 1
            };
            let feedback = (self.lfsr & 1) ^ tap;
            self.lfsr = (self.lfsr >> 1) | (feedback << 14);
        } else {
            self.timer -= 1;
        }
    }

    /// 240 Hz clock.
    fn clock_envelope(&mut self) {
        self.env.clock();
    }

    /// 120 Hz clock.
    fn clock_length(&mut self) {
        if !self.env.loop_flag && self.length > 0 {
            self.length -= 1;
        }
    }

    fn output(&self) -> u8 {
        if self.length == 0 || self.lfsr & 1 != 0 {
            return 0;
        }
        self.env.volume()
    }
}

/// The four audio channels plus the mixer and sample renderer.
pub struct Apu {
    pub pulse1: Pulse,
    pub pulse2: Pulse,
    pub triangle: Triangle,
    pub noise: Noise,
    /// Whole CPU cycles rendered so far.
    cycles: u64,
    /// Fractional-cycle carry between samples.
    cycle_frac: f64,
    env_phase: f64,
    length_phase: f64,
    /// One-pole high-pass state (DC blocker).
    hp_prev_in: f32,
    hp_prev_out: f32,
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl Apu {
    pub fn new() -> Self {
        let mut apu = Apu {
            pulse1: Pulse::new(true),
            pulse2: Pulse::new(false),
            triangle: Triangle::default(),
            noise: Noise::default(),
            cycles: 0,
            cycle_frac: 0.0,
            env_phase: 0.0,
            length_phase: 0.0,
            hp_prev_in: 0.0,
            hp_prev_out: 0.0,
        };
        // Power-up state programmed by the driver's sound_init ($8011):
        // sweep units parked at $08, triangle halted, noise LFSR non-zero.
        apu.pulse1.sweep_reg = 0x08;
        apu.pulse2.sweep_reg = 0x08;
        apu.triangle.control = true;
        apu.noise.lfsr = 1;
        apu
    }

    /// Write to a virtual APU register, $4000-$400F.
    pub fn write(&mut self, addr: u16, val: u8) {
        debug_assert!((0x4000..=0x400F).contains(&addr));
        let reg = (addr & 3) as u8;
        match addr {
            0x4000..=0x4003 => self.pulse1.write(reg, val),
            0x4004..=0x4007 => self.pulse2.write(reg, val),
            0x4008..=0x400B => self.triangle.write(reg, val),
            0x400C..=0x400F => self.noise.write(reg, val),
            _ => unreachable!(),
        }
    }

    /// Advance one CPU cycle.
    fn step_cycle(&mut self) {
        // Pulse timers tick at the APU rate (CPU/2); triangle and noise at
        // the full CPU rate.
        if self.cycles & 1 == 0 {
            self.pulse1.clock_timer();
            self.pulse2.clock_timer();
        }
        self.triangle.clock_timer();
        self.noise.clock_timer();
        self.cycles += 1;

        self.env_phase += ENV_HZ;
        if self.env_phase >= CPU_HZ {
            self.env_phase -= CPU_HZ;
            self.pulse1.clock_envelope();
            self.pulse2.clock_envelope();
            self.noise.clock_envelope();
            self.triangle.clock_linear();
        }
        self.length_phase += LENGTH_SWEEP_HZ;
        if self.length_phase >= CPU_HZ {
            self.length_phase -= CPU_HZ;
            self.pulse1.clock_length_sweep();
            self.pulse2.clock_length_sweep();
            self.triangle.clock_length();
            self.noise.clock_length();
        }
    }

    /// Current mixed mono output, roughly 0.0..1.0.
    fn mix(&self) -> f32 {
        let p = self.pulse1.output() + self.pulse2.output();
        let pulse_out = if p == 0 {
            0.0
        } else {
            95.88 / (8128.0 / p as f32 + 100.0)
        };
        let t = self.triangle.output() as f32 / 8227.0
            + self.noise.output() as f32 / 12241.0;
        let tnd_out = if t == 0.0 {
            0.0
        } else {
            159.79 / (1.0 / t + 100.0)
        };
        pulse_out + tnd_out
    }

    /// Render `n` mono samples, appending to `out`.
    pub fn render_samples(&mut self, n: usize, out: &mut Vec<f32>) {
        let cycles_per_sample = CPU_HZ / SAMPLE_RATE as f64;
        out.reserve(n);
        for _ in 0..n {
            self.cycle_frac += cycles_per_sample;
            while self.cycle_frac >= 1.0 {
                self.cycle_frac -= 1.0;
                self.step_cycle();
            }
            let s = self.mix();
            // Gentle DC blocker / high-pass (~35 Hz) so rests sit at 0.
            let filtered = s - self.hp_prev_in + 0.995 * self.hp_prev_out;
            self.hp_prev_in = s;
            self.hp_prev_out = filtered;
            out.push(filtered);
        }
    }

    /// Render as many samples as fit into `cycles` CPU cycles.
    pub fn render_cycles(&mut self, cycles: f64, out: &mut Vec<f32>) {
        let n = (cycles * SAMPLE_RATE as f64 / CPU_HZ) as usize;
        self.render_samples(n, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(apu: &mut Apu, frames: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for _ in 0..frames {
            apu.render_cycles(CYCLES_PER_FRAME, &mut out);
        }
        out
    }

    fn max_abs(pcm: &[f32]) -> f32 {
        pcm.iter().fold(0.0f32, |a, &b| a.max(b.abs()))
    }

    #[test]
    fn pulse_duty_cycle_shapes() {
        // Duty 0 = 12.5%: exactly 1/8 of samples near full volume.
        for (duty, expected_high_fraction) in [(0u8, 1.0 / 8.0), (2, 4.0 / 8.0)] {
            let mut apu = Apu::new();
            apu.write(0x4000, duty << 6 | 0x30 | 0x0F); // const vol 15, halt
            apu.write(0x4002, 0x10);
            apu.write(0x4003, 0x08); // period 16 -> ~6985 Hz
            let pcm = rendered(&mut apu, 4);
            let high = pcm.iter().filter(|&&s| s > 0.05).count();
            let frac = high as f32 / pcm.len() as f32;
            assert!(
                (frac - expected_high_fraction).abs() < 0.03,
                "duty {duty}: high fraction {frac} vs {expected_high_fraction}"
            );
        }
    }

    #[test]
    fn pulse_period_sets_pitch() {
        // Period 0x0FD (~440 Hz on pulse) should zero-cross at ~440 Hz.
        let mut apu = Apu::new();
        apu.write(0x4000, 0xBF); // 50% duty, const vol
        apu.write(0x4002, 0xFD);
        apu.write(0x4003, 0x08);
        let pcm = rendered(&mut apu, 30);
        // Count rising edges in the sign of the (high-passed) waveform.
        let mut crossings = 0;
        for w in pcm.windows(2) {
            if w[0] <= 0.0 && w[1] > 0.0 {
                crossings += 1;
            }
        }
        let secs = pcm.len() as f64 / SAMPLE_RATE as f64;
        let hz = crossings as f64 / secs;
        let expected = CPU_HZ / (16.0 * (0xFE) as f64);
        assert!(
            (hz - expected).abs() / expected < 0.06,
            "pitch {hz} Hz vs expected {expected} Hz"
        );
    }

    #[test]
    fn triangle_plays_32_step_wave() {
        let mut apu = Apu::new();
        apu.write(0x4008, 0xFF); // control + linear 127
        apu.write(0x400A, 0x40);
        apu.write(0x400B, 0x08); // period 64, length long
        let pcm = rendered(&mut apu, 10);
        assert!(max_abs(&pcm) > 0.05, "triangle silent");
        // 32-step triangle should produce intermediate levels.
        let distinct: std::collections::BTreeSet<u32> =
            pcm.iter().map(|&s| (s * 1000.0) as u32).collect();
        assert!(distinct.len() > 8, "triangle not stepped: {distinct:?}");
    }

    #[test]
    fn noise_lfsr_full_period_is_32767() {
        // Pure LFSR check: 15-bit mode-0 sequence repeats after 32767 steps.
        let mut lfsr: u16 = 1;
        let mut seen = 0u32;
        loop {
            let feedback = (lfsr & 1) ^ ((lfsr >> 1) & 1);
            lfsr = (lfsr >> 1) | (feedback << 14);
            seen += 1;
            if lfsr == 1 {
                break;
            }
            assert!(seen < 0x8000);
        }
        assert_eq!(seen, 32767);
    }

    #[test]
    fn noise_produces_output() {
        let mut apu = Apu::new();
        apu.write(0x400C, 0x30 | 0x0F); // const vol 15, halt
        apu.write(0x400E, 0x0C); // mid noise period
        apu.write(0x400F, 0x08); // length
        let pcm = rendered(&mut apu, 10);
        assert!(max_abs(&pcm) > 0.05, "noise silent");
    }

    #[test]
    fn silence_when_length_expires() {
        let mut apu = Apu::new();
        apu.write(0x4000, 0x10 | 0x0F); // const vol, NO halt (bit5 clear)
        apu.write(0x4002, 0x80);
        apu.write(0x4003, 0x00); // length idx 0 -> 10 (120Hz units ~ 83ms)
        let pcm = rendered(&mut apu, 12); // ~200ms
        let tail = &pcm[pcm.len() / 2..];
        assert!(max_abs(tail) < 0.01, "not silenced: {}", max_abs(tail));
    }

    #[test]
    fn sweep_bends_pitch_down() {
        let mut apu = Apu::new();
        apu.write(0x4000, 0xBF);
        apu.write(0x4001, 0x89); // enable, period 0, negate, shift 1
        apu.write(0x4002, 0x00);
        apu.write(0x4003, 0x0A); // period $200
        let first = apu.pulse1.period;
        rendered(&mut apu, 20);
        assert!(
            apu.pulse1.period < first || apu.pulse1.muted(),
            "sweep did not lower pitch: {}",
            apu.pulse1.period
        );
    }
}
