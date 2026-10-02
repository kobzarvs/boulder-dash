//! Decoder for the Data East sound-driver format used by NES Boulder Dash.
//!
//! This mirrors the original 6502 driver (disassembled from the ROM:
//! `sound_play` $811C, NMI update $81B4, music parse $83D5/$83F8, SFX parse
//! $84B4/$849F, command dispatch $8631 -> table $8643, handlers $86A9-$8949).
//! Instead of shadow RAM it uses named fields, but the semantics — including
//! the SFX layer's aliased volume/duty registers and the tempo accumulator —
//! match the original. See `game/assets/audio_notes.md` for the command set.
//!
//! Layers:
//! * Music (header mask bit7 clear): durations count *driver ticks*; a tick
//!   happens when the 16-bit tempo accumulator (`$BB` command, default
//!   `$FFFF`) overflows, paced at the NMI rate of 60.0988 Hz.
//! * SFX (mask bit7 set): one tick per NMI (60.0988 Hz).
//!
//! Stream grammar (music layer): `duration... note|rest|end`, i.e. duration
//! bytes (< $80) set the length of the *next* note ($80-$8F) or rest ($90);
//! everything else is a command ($90-$C2). In the SFX layer any byte < $90
//! is a duration that ends the current parse.

use super::apu::Apu;
use crate::data::music::{PERIODS, TRACKS, TRACK_COUNT};

/// Sound-data region in the original address space ($89D7..$9FE0).
pub const REGION_LO: u16 = 0x89D7;
pub const REGION_HI: u16 = 0x9FE0;
const REGION_SIZE: usize = (REGION_HI - REGION_LO) as usize;

/// CPU address of each slot's byte blob (physical order recovered from the
/// ROM pointer table; see game/assets/music_dump.txt). Slot 0 is padding.
const SLOT_ADDRS: [u16; TRACK_COUNT] = [
    0x0000, // 0: empty
    0x89D7, 0x89E2, 0x89ED, 0x89F2, 0x89F9, 0x89FE, 0x8A03, 0x8A0A, //
    0x8A0F, 0x8A14, 0x8A19, 0x8A1E, 0x8A23, 0x8A2A, 0x8A2F, 0x8A34, //
    0x8A39, // 17
    0x8D14, 0x8EC7, 0x9130, 0x913B, 0x9305, 0x9310, // 18-23
    0x9E87, // 24
    0x8A3E, 0x8A49, // 25-26
    0x9489, 0x9494, 0x96CA, 0x96D5, 0x98A9, 0x98B4, // 27-32
    0x97C6, 0x97D1, 0x9B26, // 33-35
];

/// The whole sound region rebuilt from the per-slot blobs, so that channel
/// pointers / jumps / calls can be resolved by CPU address.
fn region() -> &'static [u8; REGION_SIZE] {
    use std::sync::OnceLock;
    static REGION: OnceLock<Box<[u8; REGION_SIZE]>> = OnceLock::new();
    REGION.get_or_init(|| {
        let mut map = Box::new([0u8; REGION_SIZE]);
        for (slot, blob) in TRACKS.iter().enumerate() {
            let addr = SLOT_ADDRS[slot];
            if addr == 0 {
                continue;
            }
            let off = (addr - REGION_LO) as usize;
            map[off..off + blob.len()].copy_from_slice(blob);
        }
        map
    })
}

fn read_region(addr: u16) -> u8 {
    if !(REGION_LO..REGION_HI).contains(&addr) {
        return 0xBF; // out of region: behave like "end of channel"
    }
    region()[(addr - REGION_LO) as usize]
}

fn read_region_u16(addr: u16) -> u16 {
    read_region(addr) as u16 | (read_region(addr.wrapping_add(1)) as u16) << 8
}

/// Duty cycle animation table at $8563 ($97 command): only 4 meaningful
/// entries; the original can read past it, we wrap instead.
const DUTY_ANIM: [u8; 4] = [0, 1, 0, 1];

const MAX_WARNINGS: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Music,
    Sfx,
}

/// How a parse pass ended (mirrors which APU writes the driver runs next).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ParseEnd {
    /// Note/rest/duration consumed: run note-start or volume/period writes.
    Tick,
    /// Channel ended ($A2/$B6/top-level $BF): nothing more to write.
    Ended,
    /// $B9 wrote registers directly: skip the post-parse writes.
    Raw,
}

#[derive(Clone, Default)]
struct Channel {
    /// 0=pulse1, 1=pulse2, 2=triangle, 3=noise.
    index: usize,
    /// Stream position (CPU address).
    pc: u16,
    /// Duration counter ($034A music / $0324 sfx).
    dur_cnt: u8,
    /// Duration reload ($034E, music only).
    dur_reload: u8,
    /// Note-transpose state ($0352).
    transpose: u8,
    /// Current note nibble ($0356).
    note: u8,
    /// Signed period detune ($035A).
    detune: i8,
    /// Base volume / envelope step ($035E).
    vol_base: u8,
    /// Current volume ($032C).
    vol: u8,
    /// Duty index ($0334).
    duty_idx: u8,
    /// $0362: rest flag ($20, set by $90/$95), tie flag ($10, $95),
    /// duty-cycle animation enable ($80, $97).
    rest: bool,
    tie: bool,
    duty_anim: bool,
    /// Duty animation rate/counter ($0374/$0376).
    duty_rate: u8,
    duty_cnt: u8,
    /// $0366 envelope flags: bit6 active, bit5 gradual decay (else gated cut),
    /// bit4 = init counter from note duration, low nibble = factor.
    env_flags: u8,
    /// Envelope modulus ($036A).
    env_param: u8,
    /// Envelope counter ($036E).
    env_cnt: u8,
    /// Sweep register value written on note start ($0378).
    sweep: u8,
    /// Direct 16-bit period ($0318/$031C, SFX layer + slides).
    period16: u16,
    /// Period-changed flag ($0328 bit6).
    period_dirty: bool,
    /// SFX volume ($0330, aliased through the +4 index quirk).
    sfx_vol: u8,
    /// SFX duty index ($0338, init 2).
    sfx_duty: u8,
    /// Repeat stack ($A3/$A4), per-channel page in the original.
    stack: Vec<(u8, u16)>,
    /// Register loop ($BC/$BD).
    loop_ptr: u16,
    loop_cnt: u8,
    /// Call return address ($BE/$BF); None = top level.
    call_ptr: Option<u16>,
    /// Channel ended ($A2/$B6/$BF at top level).
    done: bool,
    /// SFX $A2: channel parked (keeps re-parsing a byte that does nothing).
    idle: bool,
    /// A backward jump was executed (loop point reached).
    looped: bool,
}

impl Channel {
    fn new(index: usize, pc: u16) -> Self {
        Channel {
            index,
            pc,
            dur_cnt: 1,
            dur_reload: 1,
            transpose: 0,
            note: 0,
            detune: 0,
            vol_base: 0,
            vol: 0,
            duty_idx: 0,
            rest: false,
            tie: false,
            duty_anim: false,
            duty_rate: 0,
            duty_cnt: 0,
            env_flags: 0,
            env_param: 0,
            env_cnt: 0,
            sweep: 0x08,
            period16: 0,
            period_dirty: false,
            sfx_vol: 0,
            sfx_duty: 2,
            stack: Vec::new(),
            loop_ptr: 0,
            loop_cnt: 0,
            call_ptr: None,
            done: false,
            idle: false,
            looped: false,
        }
    }

    fn is_pulse(&self) -> bool {
        self.index < 2
    }
}

/// One playing slot: header + per-channel sequencers, mirroring the driver.
pub struct Player {
    layer: Layer,
    channels: Vec<Channel>,
    /// Tempo accumulator addend ($0306/$0307), default $FFFF.
    tempo: u16,
    tempo_acc: u16,
    /// Ticks consumed so far.
    pub ticks: u64,
    /// Total note-on events (test/diagnostic aid).
    pub notes_played: u64,
    /// Recent note-on events as (tick, channel, note index), capped.
    /// Rests are recorded as note index 0xFF.
    pub note_log: Vec<(u64, usize, u8)>,
    /// Non-fatal decode anomalies (unknown/no-op commands, clamped notes).
    warnings: Vec<String>,
}

impl Player {
    /// Build a player for a track slot. Slot 0 yields a silent, finished player.
    pub fn new(slot: usize) -> Player {
        let mut player = Player {
            layer: Layer::Music,
            channels: Vec::new(),
            tempo: 0xFFFF,
            tempo_acc: 0,
            ticks: 0,
            notes_played: 0,
            note_log: Vec::new(),
            warnings: Vec::new(),
        };
        if slot >= TRACK_COUNT || TRACKS[slot].len() < 3 {
            return player; // slot 0 / empty: silent
        }
        let blob = TRACKS[slot];
        let mask = blob[0];
        if mask & 0x80 != 0 {
            player.layer = Layer::Sfx;
        }
        // Header: mask, aux ptr (2 bytes, priority bitmask — unused for
        // playback), then one LE channel pointer per set mask bit.
        let mut i = 3;
        for ch in 0..4 {
            if mask & (1 << ch) != 0 {
                let ptr = blob[i] as u16 | (blob[i + 1] as u16) << 8;
                i += 2;
                player.channels.push(Channel::new(ch, ptr));
            }
        }
        player
    }

    pub fn is_sfx(&self) -> bool {
        self.layer == Layer::Sfx
    }

    /// True for slot 0 / empty blobs: nothing to play.
    pub fn channels_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// Every channel ended or parked.
    pub fn is_finished(&self) -> bool {
        self.channels.iter().all(|c| c.done || c.idle)
    }

    /// Every channel ended or reached a backward jump (music loop point).
    pub fn all_looped(&self) -> bool {
        !self.channels.is_empty()
            && self.channels.iter().all(|c| c.done || c.idle || c.looped)
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    fn warn(&mut self, msg: String) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(msg);
        }
    }

    fn log_note(&mut self, hw_channel: usize, note: u8) {
        if self.note_log.len() < 4096 {
            self.note_log.push((self.ticks, hw_channel, note));
        }
    }

    /// Advance one NMI frame (60.0988 Hz), poking the APU like the driver.
    pub fn tick(&mut self, apu: &mut Apu) {
        // Tempo accumulator ($824F): tick when acc+tempo carries. Music only;
        // the SFX layer runs every NMI.
        let tick_now = match self.layer {
            Layer::Sfx => true,
            Layer::Music => {
                let (new, carry) = self.tempo_acc.overflowing_add(self.tempo);
                self.tempo_acc = new;
                carry
            }
        };
        if !tick_now {
            return;
        }
        self.ticks += 1;
        for ci in 0..self.channels.len() {
            if self.channels[ci].done || self.channels[ci].idle {
                continue;
            }
            match self.layer {
                Layer::Music => self.tick_music_channel(ci, apu),
                Layer::Sfx => self.tick_sfx_channel(ci, apu),
            }
        }
    }

    fn tick_music_channel(&mut self, ci: usize, apu: &mut Apu) {
        let ch = &mut self.channels[ci];
        ch.dur_cnt = ch.dur_cnt.wrapping_sub(1);
        if ch.dur_cnt == 0 {
            // Pre-parse: remember tie, clear rest+tie ($81DB-$81EC).
            let tie_glide = ch.tie;
            ch.rest = false;
            ch.tie = false;
            self.parse(ci, apu);
            let ch = &mut self.channels[ci];
            ch.dur_cnt = ch.dur_reload;
            if ch.done {
                silence(apu, ch.index);
                return;
            }
            // Note-start tick ($8270).
            let (index, is_rest, eff) = {
                let ch = &self.channels[ci];
                (ch.index, ch.rest && !ch.tie, ch.transpose.wrapping_add(ch.note))
            };
            if is_rest {
                self.log_note(index, 0xFF);
                silence(apu, index);
                return;
            }
            self.log_note(index, eff);
            if !tie_glide {
                self.write_period(ci, apu);
            }
            if index == 2 {
                apu.write(0x4008, 0xFF); // triangle on (linear counter max)
            } else {
                self.write_music_volume(ci, apu);
            }
        } else {
            // Sustain tick ($85CD): envelope, duty animation, volume write.
            let ch = &self.channels[ci];
            if !ch.rest {
                self.envelope_update(ci);
                self.duty_anim_update(ci);
                self.write_music_volume(ci, apu);
            }
        }
    }

    fn tick_sfx_channel(&mut self, ci: usize, apu: &mut Apu) {
        let ch = &mut self.channels[ci];
        ch.dur_cnt = ch.dur_cnt.wrapping_sub(1);
        if ch.dur_cnt != 0 {
            return;
        }
        ch.period_dirty = false;
        match self.parse(ci, apu) {
            ParseEnd::Tick => {
                // $84BB/$84BE: aliased volume write, then period write.
                self.write_sfx_volume(ci, apu);
                self.write_sfx_period(ci, apu);
            }
            ParseEnd::Raw | ParseEnd::Ended => {}
        }
    }

    /// $8567/$85BF: note -> APU period write for the music layer.
    fn write_period(&mut self, ci: usize, apu: &mut Apu) {
        let index = self.channels[ci].index;
        if index != 3 {
            let period = self.note_period(ci);
            let ch = &self.channels[ci];
            match index {
                0 | 1 => {
                    apu.write(0x4001 + 4 * index as u16, ch.sweep);
                    apu.write(0x4002 + 4 * index as u16, (period & 0xFF) as u8);
                    apu.write(
                        0x4003 + 4 * index as u16,
                        ((period >> 8) as u8 & 7) | 0x08,
                    );
                }
                _ => {
                    apu.write(0x400A, (period & 0xFF) as u8);
                    apu.write(0x400B, ((period >> 8) as u8 & 7) | 0x08);
                }
            }
        } else {
            // Noise: the note nibble IS the noise period index ($85BF).
            let ch = &self.channels[ci];
            apu.write(0x400E, ch.note & 0x0F);
            apu.write(0x400F, 0x08);
        }
    }

    /// PERIODS[transpose + note] + detune, with the driver's signed carry.
    fn note_period(&mut self, ci: usize) -> u16 {
        let (idx, index, pc) = {
            let ch = &self.channels[ci];
            (ch.transpose.wrapping_add(ch.note), ch.index, ch.pc)
        };
        if idx as usize >= PERIODS.len() {
            self.warn(format!("note index {idx} out of range (ch {index}, pc ${pc:04X})"));
        }
        let base = PERIODS[(idx as usize).min(PERIODS.len() - 1)];
        let shifted = base as i32 + self.channels[ci].detune as i32;
        shifted.clamp(0, 0x7FF) as u16
    }

    /// $8502 with X = channel index (music): duty from $0334, volume $032C.
    fn write_music_volume(&mut self, ci: usize, apu: &mut Apu) {
        let ch = &self.channels[ci];
        match ch.index {
            0 | 1 => {
                let duty = if ch.duty_anim {
                    DUTY_ANIM[(ch.duty_idx & 3) as usize]
                } else {
                    ch.duty_idx & 3
                };
                apu.write(
                    0x4000 + 4 * ch.index as u16,
                    duty << 6 | 0x30 | (ch.vol & 0x0F),
                );
            }
            2 => {
                if ch.vol == 0 {
                    apu.write(0x4008, 0x80); // triangle off
                }
            }
            _ => {
                apu.write(0x400C, 0x30 | (ch.vol & 0x0F));
            }
        }
    }

    /// $8502 with X = channel+4 (SFX aliasing): duty from $0338, volume $0330.
    fn write_sfx_volume(&mut self, ci: usize, apu: &mut Apu) {
        let ch = &self.channels[ci];
        match ch.index {
            0 | 1 => {
                let duty = ch.sfx_duty & 3;
                apu.write(
                    0x4000 + 4 * ch.index as u16,
                    duty << 6 | 0x30 | (ch.sfx_vol & 0x0F),
                );
            }
            2 => {
                if ch.sfx_vol == 0 {
                    apu.write(0x4008, 0x80);
                }
            }
            _ => {
                apu.write(0x400C, 0x30 | (ch.sfx_vol & 0x0F));
            }
        }
    }

    /// $84C2/$84F4: SFX period write; $4003 only when the period changed.
    fn write_sfx_period(&mut self, ci: usize, apu: &mut Apu) {
        let ch = &self.channels[ci];
        if ch.index == 3 {
            apu.write(0x400E, (ch.period16 & 0xFF) as u8);
            apu.write(0x400F, 0x08);
            return;
        }
        let base = 0x4000 + 4 * ch.index as u16;
        apu.write(base + 2, (ch.period16 & 0xFF) as u8);
        if ch.period_dirty {
            apu.write(base + 3, (((ch.period16 >> 8) as u8) | 0x08) & 0x0F);
        }
    }

    /// Envelope ($85CD-$860B): gradual decay (bit5) or gated cut (bit4).
    fn envelope_update(&mut self, ci: usize) {
        let ch = &mut self.channels[ci];
        if ch.env_flags & 0x40 == 0 {
            return;
        }
        if ch.env_flags & 0x20 != 0 {
            // Gradual: accumulate vol_base, each wrap of env_param drops vol.
            ch.env_cnt = ch.env_cnt.wrapping_add(ch.vol_base);
            while ch.env_cnt >= ch.env_param && ch.vol > 0 {
                ch.env_cnt = ch.env_cnt.wrapping_sub(ch.env_param);
                ch.vol -= 1;
            }
        } else {
            // Gate: vol holds until the counter expires, then cuts to 0.
            ch.env_cnt = ch.env_cnt.wrapping_sub(1);
            if ch.env_cnt == 0 {
                ch.vol = 0;
            }
        }
    }

    /// Duty-cycle animation ($860B-$8621, pulse channels only).
    fn duty_anim_update(&mut self, ci: usize) {
        let ch = &mut self.channels[ci];
        if !ch.is_pulse() || !ch.duty_anim {
            return;
        }
        ch.duty_cnt = ch.duty_cnt.wrapping_sub(1);
        if ch.duty_cnt == 0 {
            ch.duty_cnt = ch.duty_rate;
            ch.duty_idx = ch.duty_idx.wrapping_add(1);
        }
    }

    /// Stream parser. Runs until the current note/rest/duration is complete.
    /// `$B9` and `$AA` poke APU registers directly, mid-parse, like the driver.
    fn parse(&mut self, ci: usize, apu: &mut Apu) -> ParseEnd {
        // Move the channel out so `self` stays usable for tempo/warnings.
        let mut ch = std::mem::take(&mut self.channels[ci]);
        let end = self.parse_loop(&mut ch, apu);
        self.channels[ci] = ch;
        end
    }

    fn parse_loop(&mut self, ch: &mut Channel, apu: &mut Apu) -> ParseEnd {
        let sfx = self.layer == Layer::Sfx;
        let mut guard = 0u32;
        loop {
            guard += 1;
            if guard > 100_000 {
                let pc = ch.pc;
                self.warn(format!("parse runaway at ${pc:04X}, channel parked"));
                ch.idle = true;
                silence(apu, ch.index);
                return ParseEnd::Ended;
            }
            let b = read_region(ch.pc);
            if sfx {
                // SFX parse ($849F): any byte < $90 is a duration; it also
                // ends this pass.
                if b < 0x90 {
                    ch.dur_cnt = b;
                    ch.pc += 1;
                    return ParseEnd::Tick;
                }
            } else if b < 0x80 {
                // Music parse ($83F8): duration byte, keep parsing.
                ch.dur_reload = b;
                ch.pc += 1;
                continue;
            } else if b < 0x90 {
                // Note ($8408).
                ch.note = b - 0x80;
                ch.vol = ch.vol_base;
                // Envelope counter init ($8414-$8451).
                if ch.env_flags & 0x40 != 0 && !(ch.env_flags & 0x20 != 0 && ch.index == 2) {
                    ch.env_cnt = if ch.env_flags & 0x10 != 0 {
                        (ch.dur_reload >> 2).wrapping_mul(ch.env_flags & 0x0F)
                    } else if ch.env_flags & 0x20 != 0 {
                        0
                    } else {
                        ch.env_param
                    };
                }
                ch.pc += 1;
                self.notes_played += 1;
                return ParseEnd::Tick;
            }
            let pc = ch.pc;
            match b {
                0x90 => {
                    // Rest: silence for the current duration ($86A9).
                    ch.rest = true;
                    ch.pc += 1;
                    return ParseEnd::Tick;
                }
                0x91 => {
                    ch.transpose = ch.transpose.wrapping_add(12);
                    ch.pc += 1;
                }
                0x92 => {
                    ch.transpose = ch.transpose.wrapping_sub(12);
                    ch.pc += 1;
                }
                0x93 => {
                    ch.transpose = read_region(pc + 1);
                    ch.pc += 2;
                }
                0x94 => {
                    ch.vol_base = read_region(pc + 1);
                    ch.pc += 2;
                }
                0x95 => {
                    // Tie: sustain (no envelope) and glide into the next note.
                    ch.tie = true;
                    ch.rest = true;
                    ch.pc += 1;
                }
                0x96 => {
                    ch.duty_idx = read_region(pc + 1);
                    ch.pc += 2;
                }
                0x97 => {
                    ch.duty_anim = true;
                    ch.duty_rate = read_region(pc + 1);
                    ch.duty_cnt = ch.duty_rate;
                    ch.pc += 2;
                }
                0x98 => {
                    ch.duty_anim = false;
                    ch.duty_idx = 2;
                    ch.pc += 1;
                }
                0x99 => {
                    ch.env_flags = (ch.env_flags | 0x40) & 0xCF;
                    ch.env_param = read_region(pc + 1);
                    ch.pc += 2;
                }
                0x9A => {
                    ch.env_flags = (ch.env_flags | 0x60) & 0xEF;
                    ch.env_param = read_region(pc + 1);
                    ch.pc += 2;
                }
                0x9B => {
                    ch.env_flags = ((ch.env_flags | 0x50) & 0xDF) | read_region(pc + 1);
                    ch.pc += 2;
                }
                0x9C => {
                    ch.env_flags = (ch.env_flags | 0x70) | read_region(pc + 1);
                    ch.pc += 2;
                }
                0x9D => {
                    ch.env_flags &= 0x8F;
                    ch.pc += 1;
                }
                0x9E => {
                    ch.sweep = read_region(pc + 1);
                    ch.pc += 2;
                }
                0x9F | 0xA0 => ch.pc += 1, // no-op in the driver
                0xA1 | 0xA5 => {
                    // Jump (used as song loop).
                    let target = read_region_u16(pc + 1);
                    if target < pc {
                        ch.looped = true;
                    }
                    ch.pc = target;
                }
                0xA2 => {
                    // End of channel + silence ($8759 -> $887A). In an SFX
                    // stream it only clears the music-layer mask, so the
                    // channel just parks forever.
                    if sfx {
                        ch.idle = true;
                    } else {
                        ch.done = true;
                    }
                    silence(apu, ch.index);
                    return ParseEnd::Ended;
                }
                0xA3 => {
                    // Repeat start: push (count, body address).
                    let count = read_region(pc + 1);
                    ch.stack.push((count, pc + 2));
                    ch.pc += 2;
                }
                0xA4 => {
                    // Repeat end.
                    match ch.stack.last_mut() {
                        Some((count, addr)) => {
                            *count = count.wrapping_sub(1);
                            if *count > 0 {
                                ch.pc = *addr;
                            } else {
                                ch.stack.pop();
                                ch.pc += 1;
                            }
                        }
                        None => {
                            self.warn(format!("$A4 with empty stack at ${pc:04X}"));
                            ch.done = true;
                            return ParseEnd::Ended;
                        }
                    }
                }
                0xA6 => {
                    // Set SFX duration via command; ends the parse pass.
                    ch.dur_cnt = read_region(pc + 1);
                    ch.pc += 2;
                    return ParseEnd::Tick;
                }
                0xA7 => {
                    ch.sfx_vol = read_region(pc + 1);
                    ch.pc += 2;
                }
                0xA8 => {
                    ch.period16 = read_region_u16(pc + 1);
                    ch.period_dirty = true;
                    ch.pc += 3;
                }
                0xA9 => {
                    ch.sfx_duty = read_region(pc + 1);
                    ch.pc += 2;
                }
                0xAA => {
                    // Direct sweep-register poke ($87FB): $4001+ch*4.
                    let val = read_region(pc + 1);
                    if ch.is_pulse() {
                        apu.write(0x4001 + 4 * ch.index as u16, val);
                    }
                    ch.pc += 2;
                }
                0xAB => {
                    ch.sfx_vol = ch.sfx_vol.wrapping_add(read_region(pc + 1));
                    ch.pc += 2;
                }
                0xAC => {
                    ch.period16 = ch.period16.wrapping_add(read_region(pc + 1) as u16);
                    ch.period_dirty = true;
                    ch.pc += 2;
                }
                0xAD => {
                    ch.sfx_vol = ch.sfx_vol.wrapping_add(1);
                    ch.pc += 1;
                }
                0xAE | 0xB3 | 0xB4 | 0xB5 | 0xB7 | 0xB8 | 0xBA | 0xC0 | 0xC2 => {
                    ch.pc += 1; // no-op in the driver
                }
                0xAF => {
                    ch.sfx_duty = ch.sfx_duty.wrapping_add(1);
                    ch.pc += 1;
                }
                0xB0 => {
                    ch.sfx_vol = ch.sfx_vol.wrapping_sub(read_region(pc + 1));
                    ch.pc += 2;
                }
                0xB1 => {
                    ch.period16 = ch.period16.wrapping_sub(read_region(pc + 1) as u16);
                    ch.period_dirty = true;
                    ch.pc += 2;
                }
                0xB2 => {
                    ch.sfx_vol = ch.sfx_vol.wrapping_sub(1);
                    ch.pc += 1;
                }
                0xB6 => {
                    // End of SFX channel: silence and stop ($8869 -> $887A).
                    ch.done = true;
                    silence(apu, ch.index);
                    return ParseEnd::Ended;
                }
                0xB9 => {
                    // Raw APU poke ($88A7): 4 register bytes to $4000+ch*4,
                    // then a duration byte; ends the parse pass and skips the
                    // post-parse volume/period writes.
                    let base = 0x4000 + 4 * ch.index as u16;
                    for r in 0..4 {
                        apu.write(base + r, read_region(pc + 1 + r));
                    }
                    ch.dur_cnt = read_region(pc + 5);
                    ch.pc += 6;
                    return ParseEnd::Raw;
                }
                0xBB => {
                    // Tempo (global): 16-bit accumulator addend.
                    self.tempo = read_region_u16(pc + 1);
                    ch.pc += 3;
                }
                0xBC => {
                    // Register repeat start.
                    ch.loop_cnt = read_region(pc + 1);
                    ch.loop_ptr = pc + 2;
                    ch.pc += 2;
                }
                0xBD => {
                    // Register repeat end.
                    ch.loop_cnt = ch.loop_cnt.wrapping_sub(1);
                    if ch.loop_cnt > 0 {
                        ch.pc = ch.loop_ptr;
                    } else {
                        ch.pc += 1;
                    }
                }
                0xBE => {
                    // Call subroutine (single level).
                    ch.call_ptr = Some(pc + 3);
                    ch.pc = read_region_u16(pc + 1);
                }
                0xBF => {
                    // Return; at top level it ends the channel.
                    match ch.call_ptr.take() {
                        Some(ret) => ch.pc = ret,
                        None => {
                            ch.done = true;
                            silence(apu, ch.index);
                            return ParseEnd::Ended;
                        }
                    }
                }
                0xC1 => {
                    ch.detune = read_region(pc + 1) as i8;
                    ch.pc += 2;
                }
                _ => {
                    self.warn(format!("unknown command ${b:02X} at ${pc:04X}"));
                    ch.pc += 1;
                }
            }
        }
    }
}

/// $887A: silence one APU channel.
fn silence(apu: &mut Apu, ch: usize) {
    match ch {
        0 | 1 => {
            apu.write(0x4000 + 4 * ch as u16, 0x30);
            apu.write(0x4001 + 4 * ch as u16, 0x08);
        }
        2 => apu.write(0x4008, 0x80),
        _ => {
            apu.write(0x400C, 0x30);
        }
    }
}

