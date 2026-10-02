//! Sound/music data extraction.
//!
//! ROM layout (CPU addresses):
//! - $805C: NTSC note period table, 96 LE u16 entries (C2..B9). Not strictly
//!   decreasing: equal pairs at the tail (rounding clamp, e.g. two $0007).
//! - $8949: track pointer table, 36 LE u16 slots. Slot 0 is padding ($FF60,
//!   no data). Slots 1-17 are short SFX, 18+ mostly full music tracks, but
//!   every slot 1..=35 shares the same header format:
//!   byte0 = channel bitmask in the low nibble (bit0=pulse1, bit1=pulse2,
//!   bit2=triangle, bit3=noise; high nibble observed as $0 or $8, likely a
//!   flag), bytes1-2 = LE pointer into a 14x5-byte aux record table at
//!   $8991-$89D6 (SFX params; $8991 for music slots), then one LE u16 data
//!   pointer per enabled channel in order pulse1, pulse2, triangle, noise.
//! - Sound data region: $89D7-$9FDF (inclusive). Slot data spans from its
//!   pointer to the next higher pointer; the region is thereby partitioned
//!   contiguously. Channel data of one slot may live inside another slot's
//!   span (e.g. SFX slots 1-17 point into the span of slot 26).
//! - Region end: the last slot ($9E87) has channels at $9E8E/$9EA1 whose data
//!   ends with the track-terminator command $BF at $9FDF; $9FE0-$9FFF is $FF
//!   filler before the fixed-bank reset code at $A000. We detect the end as
//!   the first run of >=16 consecutive $FF bytes after the max pointer.

use crate::{rom::Rom, Ctx};
use anyhow::{ensure, Result};
use std::fmt::Write as _;

const PERIODS_ADDR: u16 = 0x805C;
const PERIODS_LEN: usize = 96;
const PTR_TABLE_ADDR: u16 = 0x8949;
const TRACK_COUNT: usize = 36;
const REGION_LO: u16 = 0x8000;
const REGION_HI: u16 = 0xA000;
const AUX_TABLE_ADDR: u16 = 0x8991; // 14 records x 5 bytes, up to first slot
const FF_RUN: usize = 16;

struct Slot {
    ptr: u16,
    size: usize,
    mask: u8,
    chan_ptrs: [u16; 4],
}

pub fn extract(rom: &Rom, ctx: &Ctx) -> Result<()> {
    // (a) NTSC period table.
    let periods: Vec<u16> = (0..PERIODS_LEN)
        .map(|i| rom.cpu_u16(PERIODS_ADDR + 2 * i as u16))
        .collect();
    ensure!(periods[0] == 0x06AE, "periods[0] = ${:04X}, want $06AE", periods[0]);
    for i in 1..PERIODS_LEN {
        ensure!(
            periods[i] <= periods[i - 1],
            "periods not monotone at {i}: ${:04X} -> ${:04X}",
            periods[i - 1],
            periods[i]
        );
    }

    // (b) Pointer table.
    let raw_ptrs: Vec<u16> = (0..TRACK_COUNT)
        .map(|i| rom.cpu_u16(PTR_TABLE_ADDR + 2 * i as u16))
        .collect();
    for (i, &p) in raw_ptrs.iter().enumerate().skip(1) {
        ensure!(
            (REGION_LO..REGION_HI).contains(&p),
            "slot {i} pointer ${p:04X} outside sound region"
        );
    }
    let mut sorted: Vec<u16> = raw_ptrs[1..].to_vec();
    sorted.sort_unstable();
    ensure!(sorted.windows(2).all(|w| w[0] != w[1]), "duplicate slot pointers");
    let max_ptr = *sorted.last().unwrap();

    // Region end: first long $FF filler run after the last slot's data.
    let mut region_end = None;
    let mut a = max_ptr;
    while a <= REGION_HI - FF_RUN as u16 {
        if rom.cpu_slice(a, FF_RUN).iter().all(|&b| b == 0xFF) {
            region_end = Some(a);
            break;
        }
        a += 1;
    }
    let region_end = region_end.expect("no $FF filler run found before $A000");
    eprintln!("music: sound region ${:04X}-${:04X} (end excl. ${:04X})",
        sorted[0], region_end - 1, region_end);

    // (b)+(c) Per-slot raw span (next higher pointer / region end) and header.
    let mut slots = Vec::with_capacity(TRACK_COUNT);
    let mut dump = String::new();
    writeln!(dump, "Boulder Dash NES - sound/music dump").unwrap();
    writeln!(dump, "period table @ ${PERIODS_ADDR:04X}: {} entries, first ${:04X}, last ${:04X}",
        PERIODS_LEN, periods[0], periods[PERIODS_LEN - 1]).unwrap();
    writeln!(dump, "pointer table @ ${PTR_TABLE_ADDR:04X}: {TRACK_COUNT} slots").unwrap();
    writeln!(dump, "sound region ${:04X}-${:04X}; aux table ${AUX_TABLE_ADDR:04X}-${:04X} (14 x 5 bytes)",
        sorted[0], region_end - 1, sorted[0] - 1).unwrap();
    writeln!(dump).unwrap();

    for (i, &ptr) in raw_ptrs.iter().enumerate() {
        if i == 0 {
            // Padding slot ($FF60): no data, zero header.
            writeln!(dump, "slot  0  ptr=${ptr:04X}  PADDING (no data)").unwrap();
            slots.push(Slot { ptr, size: 0, mask: 0, chan_ptrs: [0; 4] });
            continue;
        }
        let next = sorted.iter().copied().filter(|&q| q > ptr).min();
        let end = next.unwrap_or(region_end);
        let size = (end - ptr) as usize;

        let mask = rom.cpu(ptr);
        let aux = rom.cpu_u16(ptr + 1);
        let nch = (mask & 0x0F).count_ones() as usize;
        let hdr_len = 3 + 2 * nch;
        ensure!(hdr_len <= size, "slot {i} header ({hdr_len} B) exceeds span ({size} B)");
        let mut chan_ptrs = [0u16; 4];
        let mut ch = 0;
        for bit in 0..4 {
            if mask & (1 << bit) != 0 {
                let p = rom.cpu_u16(ptr + 3 + 2 * ch as u16);
                ensure!(
                    (REGION_LO..REGION_HI).contains(&p),
                    "slot {i} channel {bit} ptr ${p:04X} outside sound region"
                );
                chan_ptrs[bit] = p;
                ch += 1;
            }
        }
        let names = ["pulse1", "pulse2", "triangle", "noise"];
        let chans: Vec<String> = (0..4)
            .map(|b| format!("{}={}", names[b], if chan_ptrs[b] != 0 {
                format!("${:04X}", chan_ptrs[b])
            } else {
                "-".into()
            }))
            .collect();
        writeln!(
            dump,
            "slot {i:2}  ptr=${ptr:04X}  size={size:4}  mask=${mask:02X}  aux=${aux:04X}  {}",
            chans.join(" ")
        )
        .unwrap();
        let hex: Vec<String> = rom
            .cpu_slice(ptr, size.min(32))
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect();
        writeln!(dump, "  bytes[0..{}]: {}", hex.len(), hex.join(" ")).unwrap();
        slots.push(Slot { ptr, size, mask, chan_ptrs });
    }

    // Emit game/src/data/music.rs.
    let mut out = String::new();
    writeln!(out, "//! Generated by `cargo run -p xtask -- extract-music Boulder_Dash.nes`.").unwrap();
    writeln!(out, "//! NES Boulder Dash (Data East 1990) sound data, CPU $805C/$8949-$9FDF.").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "/// NTSC note periods (C2..B9), from CPU ${PERIODS_ADDR:04X}.").unwrap();
    writeln!(out, "/// Non-increasing; equal pairs at the tail are a rounding clamp in the ROM.").unwrap();
    writeln!(out, "pub const PERIODS: [u16; {PERIODS_LEN}] = [").unwrap();
    for row in periods.chunks(8) {
        let cells: Vec<String> = row.iter().map(|p| format!("0x{p:04X}")).collect();
        writeln!(out, "    {},", cells.join(", ")).unwrap();
    }
    writeln!(out, "];\n").unwrap();

    writeln!(out, "pub const TRACK_COUNT: usize = {TRACK_COUNT};\n").unwrap();
    writeln!(out, "/// Raw bytes per pointer-table slot. Slot 0 is padding (empty).").unwrap();
    writeln!(out, "/// Each slot spans from its pointer to the next higher pointer in the").unwrap();
    writeln!(out, "/// sound region; the last slot ends at ${:04X}.", region_end).unwrap();
    writeln!(out, "pub const TRACKS: [&[u8]; TRACK_COUNT] = [").unwrap();
    for s in &slots {
        writeln!(out, "    {},", byte_string(rom, s.ptr, s.size)).unwrap();
    }
    writeln!(out, "];\n").unwrap();

    writeln!(out, "#[derive(Debug, Clone, Copy)]").unwrap();
    writeln!(out, "pub struct TrackHeader {{").unwrap();
    writeln!(out, "    /// bit0=pulse1, bit1=pulse2, bit2=triangle, bit3=noise; high nibble is a flag.").unwrap();
    writeln!(out, "    pub channel_mask: u8,").unwrap();
    writeln!(out, "    /// Data pointer per channel (0 = disabled), order pulse1,pulse2,triangle,noise.").unwrap();
    writeln!(out, "    pub channel_ptrs: [u16; 4],").unwrap();
    writeln!(out, "}}\n").unwrap();
    writeln!(out, "pub const TRACK_HEADERS: [TrackHeader; TRACK_COUNT] = [").unwrap();
    for s in &slots {
        writeln!(
            out,
            "    TrackHeader {{ channel_mask: 0x{:02X}, channel_ptrs: [0x{:04X}, 0x{:04X}, 0x{:04X}, 0x{:04X}] }},",
            s.mask, s.chan_ptrs[0], s.chan_ptrs[1], s.chan_ptrs[2], s.chan_ptrs[3]
        )
        .unwrap();
    }
    writeln!(out, "];\n").unwrap();

    writeln!(out, "/// Aux record table at ${AUX_TABLE_ADDR:04X} (14 x 5 bytes), referenced by").unwrap();
    writeln!(out, "/// track-header bytes 1-2 (SFX parameter records; $8991 for music slots).").unwrap();
    writeln!(out, "pub const TRACK_AUX_TABLE: [u8; 70] = [").unwrap();
    for row in rom.cpu_slice(AUX_TABLE_ADDR, 70).chunks(14) {
        let cells: Vec<String> = row.iter().map(|b| format!("0x{b:02X}")).collect();
        writeln!(out, "    {},", cells.join(", ")).unwrap();
    }
    writeln!(out, "];").unwrap();

    std::fs::write(ctx.data_dir.join("music.rs"), out)?;
    std::fs::write(ctx.assets_dir.join("music_dump.txt"), dump)?;
    Ok(())
}

/// Render a slot's raw bytes as a Rust byte-string literal (`b"\xAE\x06..."`).
fn byte_string(rom: &Rom, ptr: u16, size: usize) -> String {
    if size == 0 {
        return "b\"\"".into();
    }
    let mut s = String::with_capacity(4 * size + 4);
    s.push_str("b\"");
    for &b in rom.cpu_slice(ptr, size) {
        match b {
            0x22 => s.push_str("\\\""),
            0x5C => s.push_str("\\\\"),
            0x20..=0x7E => s.push(b as char),
            _ => write!(s, "\\x{b:02X}").unwrap(),
        }
    }
    s.push('"');
    s
}
