//! Screen/UI extraction: VRAM text records, the raw map backdrop, `$A1BD`
//! RLE streams (PRG + CHR bank tails), `$E866` map scripts, palettes.
//! Renders PNG previews to assets/screens/ and emits src/data/screens.rs.
//!
//! ROM formats (see analysis/ghidra_findings.md, Q9 state table):
//! - 50-byte VRAM records (`screen_block_copy` $A808 -> staging $0116 -> NMI
//!   bit3 uploader $B09D): segments of (addr_lo, addr_hi, count, data[count]),
//!   terminated by a lone $FF. Literal bytes only (FD/FE/FF are tiles here).
//! - `$A1BD` RLE streams: first 2 bytes are an implicit FE (PPU addr lo, hi).
//!   FE lo hi = set address + row anchor; FD = next row (anchor += $20, write
//!   addr = anchor — literal writes do NOT move the anchor); FC = toggle $2000
//!   bit 2 (+1/+32 increment); FB n = copy the next n bytes literally (the only
//!   way to emit tile values $FB-$FF); FF = end. Streams live in PRG and in CHR
//!   bank tails (the game first reads 512 bytes from $1DC0/$1E00/$1EE0... into
//!   RAM $0600/$0700 via $A313, then interprets them from there).
//! - `$E866` map scripts (one tile per 4 frames, typewriter): FE lo hi = set
//!   anchor + cursor, FD = next row (anchor += $20, cursor = anchor), FC lo hi =
//!   jump, FF = done, else tile at cursor++.

use crate::{rom::Rom, Ctx};
use anyhow::{ensure, Context, Result};
use std::fmt::Write as _;
use std::path::Path;

// ---------------------------------------------------------------------------
// ROM addresses
// ---------------------------------------------------------------------------

/// State-1 (attract/menu) text table: 13 LE pointers, indexed by substate $21
/// via `screen_block_copy` ($D93C). NOTE: the table base is $F634.
const ATTRACT_TABLE: u16 = 0xF634;
const ATTRACT_COUNT: usize = 13;

/// Raw 1024-byte nametable image (960 tiles + 64 attrs) copied verbatim to
/// nametable $2000 by `screen_prep` ($A794 -> $A334). Shared backdrop of the
/// map-walk / color-select / password / status / game-over screens.
const BACKDROP_ADDR: u16 = 0xF78E;

/// Block-record tables (pointer table of N entries, then the records).
const COLOR_BOX_TABLE: u16 = 0xFB8E; // state 2 sub1: frame box, 5 records
const COLOR_BOX_COUNT: usize = 5;
const COLOR_TEXT_TABLE: u16 = 0xFC48; // state 2 sub2: "1 PLAYER"/"COLOR SELECT", 2
const COLOR_TEXT_COUNT: usize = 2;
const PASSWORD_TABLE: u16 = 0xFC70; // state 3 sub0: "PASSWORD*", 1
const PASSWORD_ROWS_TABLE: u16 = 0xFC89; // state 3 sub2: backdrop restore rows, 5
const PASSWORD_ROWS_COUNT: usize = 5;
const STATUS_TABLE: u16 = 0xFD43; // state 8: "1 PLAYER"/"WORLD 00-00", 2
const STATUS_COUNT: usize = 2;
const GAME_OVER_TABLE: u16 = 0xFD6B; // state 14: "CONTINUE/END"/"PASSWORD*", 2
const GAME_OVER_COUNT: usize = 2;

/// Title screen stream: CHR bank 5, PPU window $1E00 (bank offset $E00).
const TITLE_BANK: usize = 5;
const TITLE_OFF: usize = 0xE00;
/// Map frame stream (sky/grass window): CHR bank 7, PPU $1DC0 (offset $DC0).
const MAP_FRAME_BANK: usize = 7;
const MAP_FRAME_OFF: usize = 0xDC0;
/// Hub/ending base image stream: CHR bank 7, PPU $1F2E (offset $F2E).
const HUB_IMG_OFF: usize = 0xF2E;
/// Per-world map image descriptors at $EA6E: 6 x [bank, unused, lo, hi].
const WORLD_IMG_TABLE: u16 = 0xEA6E;
/// Per-world attribute rows: 32 bytes each, CHR bank 3 offset $EE0 + w*$20
/// (written to $27C8 = attribute rows 1-4 of nametable $2400).
const WORLD_ATTRS_BANK: usize = 3;
const WORLD_ATTRS_OFF: usize = 0xEE0;
/// "TRY THE NEXT STAGE*" stream in PRG (state 4 setup draws it after the map).
const NEXT_STAGE_STREAM: u16 = 0xE02F;
/// Ending overlay stream in PRG (decorative tile rows at $26CD/$26FB).
const ENDING_OVERLAY_STREAM: u16 = 0xEFF5;
/// Difficulty splash streams: $FD9A table = [$FDA2, $FDA2, $FE35, $FEB5].
const SPLASH_TABLE: u16 = 0xFD9A;

/// Map caption scripts ("BOULDER WORLD" etc.): table $E909, 6 LE entries.
/// Scripts FC-jump into the shared "WORLD" tail at $E94F.
const CAPTION_TABLE: u16 = 0xE909;
const CAPTION_TAIL: u16 = 0xE94F;
const CAPTION_ANCHOR: u16 = 0x26A9;
/// Hub script ("WONDERFUL*") at $EBD2, anchor $26AB.
const HUB_SCRIPT: u16 = 0xEBD2;
const HUB_ANCHOR: u16 = 0x26AB;
/// Ending credits scripts: table $EEB7, 6 entries, anchor $26A6.
const ENDING_TABLE: u16 = 0xEEB7;
const ENDING_ANCHOR: u16 = 0x26A6;
/// Splash scripts ("TRY THE NEXT LEVEL* / PASSWORD ...... / PUSH START ..."):
/// table $F0FA, 4 entries, anchor $2686; all FC-jump to the tail at $F19B.
const SPLASH_SCRIPT_TABLE: u16 = 0xF0FA;
const SPLASH_SCRIPT_TAIL: u16 = 0xF19B;
const SPLASH_ANCHOR: u16 = 0x2686;

/// Palettes (16-byte BG descriptors unless noted).
const PAL_TITLE_ADDR: u16 = 0xA674;
const PAL_GAME_ADDR: u16 = 0xA7D2;
const PAL_GAME_SPR_ADDR: u16 = 0xA7E2;
const PAL_HUB_ADDR: u16 = 0xEBC2;
const PAL_SPLASH_ADDRS: [u16; 3] = [0xF0CA, 0xF0DA, 0xF0EA]; // diff 0/1, 2, 3
/// Map-screen sprite palettes (3 distinct, worlds 0-5 via table $EA92).
const PAL_MAP_SPR_ADDRS: [u16; 3] = [0xEA9E, 0xEAAE, 0xEABE];

/// CHR bank mapped at BG pattern table 1 on map/menu screens ($6F = 6, set by
/// `screen_prep` tail $A794). Title/attract use bank 5 ($6F = 5).
const MENU_BANK: u8 = 6;

// ---------------------------------------------------------------------------
// Decoders
// ---------------------------------------------------------------------------

/// One segment of a 50-byte VRAM record: PPU target address + tile bytes.
#[derive(Clone)]
struct Segment {
    addr: u16,
    data: Vec<u8>,
}

/// Parse a record at `addr`: (lo hi count data)* until a lone $FF byte.
fn decode_record(rom: &Rom, addr: u16) -> Result<Vec<Segment>> {
    let mut segs = Vec::new();
    let mut pos = addr;
    loop {
        let lo = rom.cpu(pos);
        if lo == 0xFF {
            break;
        }
        let hi = rom.cpu(pos + 1);
        let count = rom.cpu(pos + 2) as usize;
        ensure!(count > 0, "record at ${addr:04X}: zero-count segment at ${pos:04X}");
        ensure!(count <= 32, "record at ${addr:04X}: count {count} > 32 at ${pos:04X}");
        let ppu = (hi as u16) << 8 | lo as u16;
        ensure!(
            (0x2000..0x3000).contains(&ppu),
            "record at ${addr:04X}: bad PPU addr ${ppu:04X} at ${pos:04X}"
        );
        let data = rom.cpu_slice(pos + 3, count).to_vec();
        segs.push(Segment { addr: ppu, data });
        pos += 3 + count as u16;
        ensure!(pos <= addr + 50, "record at ${addr:04X}: overruns 50-byte staging");
    }
    Ok(segs)
}

/// Read a pointer table of `count` LE entries at `table`, decode each record.
fn decode_record_table(rom: &Rom, table: u16, count: usize) -> Result<Vec<Vec<Segment>>> {
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let ptr = rom.cpu_u16(table + 2 * i as u16);
        out.push(decode_record(rom, ptr)?);
    }
    Ok(out)
}

/// Decode an `$A1BD` RLE stream into (ppu_addr, tile) writes.
/// Also returns the consumed byte count (up to and including the FF).
fn decode_stream(buf: &[u8]) -> Result<(Vec<(u16, u8)>, usize)> {
    let mut writes = Vec::new();
    ensure!(buf.len() >= 2, "stream too short for start address");
    let mut anchor = buf[0] as usize | (buf[1] as usize) << 8;
    let mut addr = anchor;
    let mut pos = 2;
    let mut inc32 = false;
    loop {
        let b = *buf.get(pos).context("stream ran past end of slice without FF")?;
        pos += 1;
        match b {
            0xFF => break,
            0xFE => {
                anchor = buf[pos] as usize | (buf[pos + 1] as usize) << 8;
                addr = anchor;
                pos += 2;
            }
            0xFD => {
                anchor += 0x20;
                addr = anchor;
            }
            0xFC => inc32 = !inc32,
            0xFB => {
                let n = buf[pos] as usize;
                pos += 1;
                for i in 0..n {
                    writes.push((addr as u16, buf[pos + i]));
                    addr += if inc32 { 32 } else { 1 };
                }
                pos += n;
            }
            _ => {
                writes.push((addr as u16, b));
                addr += if inc32 { 32 } else { 1 };
            }
        }
    }
    Ok((writes, pos))
}

/// Decode an `$E866` map script into (ppu_addr, tile) writes, following FC jumps.
/// FD/FE semantics match the engine ($03E4 = row anchor, $03E6 = write cursor):
/// FE sets both, FD advances the anchor one row and resets the cursor to it,
/// literals advance only the cursor.
fn decode_script(rom: &Rom, start: u16, anchor: u16) -> Result<Vec<(u16, u8)>> {
    let mut writes = Vec::new();
    let mut pos = start;
    let mut anchor = anchor;
    let mut addr = anchor;
    for _ in 0..4096 {
        let b = rom.cpu(pos);
        pos += 1;
        match b {
            0xFF => return Ok(writes),
            0xFE => {
                anchor = rom.cpu_u16(pos);
                addr = anchor;
                pos += 2;
            }
            0xFD => {
                anchor += 0x20;
                addr = anchor;
            }
            0xFC => pos = rom.cpu_u16(pos),
            _ => {
                writes.push((addr, b));
                addr += 1;
            }
        }
    }
    anyhow::bail!("script at ${start:04X}: runaway/loop")
}

/// Raw stream bytes: decode and keep everything up to and including the FF.
fn stream_bytes(buf: &[u8]) -> Result<Vec<u8>> {
    let (_, used) = decode_stream(buf)?;
    Ok(buf[..used].to_vec())
}

/// Raw script bytes from `start` through the terminating FF. An FC jump ends
/// the contiguous head (operand bytes included); callers append the jump
/// target's bytes explicitly when emitting standalone blobs.
fn script_head_bytes(rom: &Rom, start: u16) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = start;
    for _ in 0..4096 {
        let b = rom.cpu(pos);
        out.push(b);
        pos += 1;
        match b {
            0xFF => return Ok(out),
            0xFE => {
                out.extend_from_slice(rom.cpu_slice(pos, 2));
                pos += 2;
            }
            0xFC => {
                out.extend_from_slice(rom.cpu_slice(pos, 2));
                return Ok(out);
            }
            _ => {}
        }
    }
    anyhow::bail!("script at ${start:04X}: no FF within 4096 bytes")
}

// ---------------------------------------------------------------------------
// Nametable compositing
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Nametable {
    base: u16, // $2000 / $2400 / $2800
    tiles: [u8; 960],
    attrs: [u8; 64],
}

impl Nametable {
    fn new(base: u16, fill: u8, attr_fill: u8) -> Self {
        Self { base, tiles: [fill; 960], attrs: [attr_fill; 64] }
    }
    fn put(&mut self, addr: u16, tile: u8) {
        if (self.base..self.base + 0x400).contains(&addr) {
            let off = (addr - self.base) as usize;
            if off < 960 {
                self.tiles[off] = tile;
            } else {
                self.attrs[off - 960] = tile;
            }
        }
    }
    fn apply(&mut self, writes: &[(u16, u8)]) {
        for &(addr, tile) in writes {
            self.put(addr, tile);
        }
    }
    fn apply_record(&mut self, segs: &[Segment]) {
        for seg in segs {
            for (i, &tile) in seg.data.iter().enumerate() {
                self.put(seg.addr + i as u16, tile);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PNG rendering (true color, 256x240 per nametable)
// ---------------------------------------------------------------------------

/// Same 64-color table as game/src/data/cave_params.rs::NES_PALETTE.
#[rustfmt::skip]
const NES_PALETTE: [[u8; 3]; 64] = [
    [84, 84, 84], [0, 30, 116], [8, 16, 144], [48, 0, 136], [68, 0, 100], [92, 0, 48], [84, 4, 0], [60, 24, 0],
    [32, 42, 0], [8, 58, 0], [0, 64, 0], [0, 60, 0], [0, 50, 60], [0, 0, 0], [0, 0, 0], [0, 0, 0],
    [152, 150, 152], [8, 76, 196], [48, 50, 236], [92, 30, 228], [136, 20, 176], [160, 20, 100], [152, 34, 32], [120, 60, 0],
    [84, 90, 0], [40, 114, 0], [8, 124, 0], [0, 118, 40], [0, 102, 120], [0, 0, 0], [0, 0, 0], [0, 0, 0],
    [236, 238, 236], [76, 154, 236], [120, 124, 236], [176, 98, 236], [228, 84, 236], [236, 88, 180], [236, 106, 100], [212, 136, 32],
    [160, 170, 0], [116, 196, 0], [76, 208, 32], [56, 204, 108], [56, 180, 204], [60, 60, 60], [0, 0, 0], [0, 0, 0],
    [236, 238, 236], [168, 204, 236], [188, 188, 236], [212, 178, 236], [236, 174, 236], [236, 174, 212], [236, 180, 176], [228, 196, 144],
    [204, 210, 120], [180, 222, 120], [168, 226, 144], [152, 226, 180], [160, 214, 228], [160, 162, 160], [0, 0, 0], [0, 0, 0],
];

fn tile_pixel(rom: &Rom, bank: u8, tile: u8, row: usize, col: usize) -> u8 {
    let base = bank as usize * 0x1000 + tile as usize * 16;
    let p0 = rom.chr[base + row];
    let p1 = rom.chr[base + 8 + row];
    let bit = 7 - col;
    ((p0 >> bit) & 1) | (((p1 >> bit) & 1) << 1)
}

/// Render a nametable to RGB using BG palette descriptor `palette` (16 bytes).
fn render_nt(rom: &Rom, nt: &Nametable, bank: u8, palette: &[u8; 16]) -> Vec<u8> {
    let mut img = vec![0u8; 256 * 240 * 3];
    for ty in 0..30 {
        for tx in 0..32 {
            let tile = nt.tiles[ty * 32 + tx];
            let attr = nt.attrs[(ty / 4) * 8 + tx / 4];
            let shift = (if ty % 4 >= 2 { 4 } else { 0 }) + (if tx % 4 >= 2 { 2 } else { 0 });
            let group = ((attr >> shift) & 3) as usize;
            for r in 0..8 {
                for c in 0..8 {
                    let ci = tile_pixel(rom, bank, tile, r, c) as usize;
                    let rgb = NES_PALETTE[(palette[group * 4 + ci] & 0x3F) as usize];
                    let o = ((ty * 8 + r) * 256 + tx * 8 + c) * 3;
                    img[o..o + 3].copy_from_slice(&rgb);
                }
            }
        }
    }
    img
}

/// Draw an 8x16 sprite (tiles t/t+1 of `bank`) into an RGB image.
fn draw_sprite_8x16(
    img: &mut [u8],
    rom: &Rom,
    bank: u8,
    tile: u8,
    x: usize,
    y: usize,
    palette: [u8; 4],
) {
    for half in 0..2u8 {
        for r in 0..8 {
            for c in 0..8 {
                let ci = tile_pixel(rom, bank, tile + half, r, c) as usize;
                if ci == 0 {
                    continue;
                }
                let rgb = NES_PALETTE[(palette[ci] & 0x3F) as usize];
                let (py, px) = (y + half as usize * 8 + r, x + c);
                let o = (py * 256 + px) * 3;
                img[o..o + 3].copy_from_slice(&rgb);
            }
        }
    }
}

fn write_png(path: &Path, width: u32, height: u32, rgb: &[u8]) -> Result<()> {
    let file = std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut enc = png::Encoder::new(file, width, height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    writer
        .write_image_data(rgb)
        .with_context(|| format!("encode {}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

struct Extracted {
    attract: Vec<Vec<Segment>>,
    color_box: Vec<Vec<Segment>>,
    color_text: Vec<Vec<Segment>>,
    password: Vec<Vec<Segment>>,
    password_rows: Vec<Vec<Segment>>,
    status: Vec<Vec<Segment>>,
    game_over: Vec<Vec<Segment>>,
    backdrop: [u8; 1024],
    title_stream: Vec<u8>,
    map_frame_stream: Vec<u8>,
    world_img_streams: [Vec<u8>; 6],
    world_attrs: [[u8; 32]; 6],
    hub_img_stream: Vec<u8>,
    ending_overlay_stream: Vec<u8>,
    next_stage_stream: Vec<u8>,
    splash_streams: [Vec<u8>; 4],
    caption_scripts: [Vec<u8>; 6],
    hub_script: Vec<u8>,
    ending_scripts: [Vec<u8>; 6],
    splash_scripts: [Vec<u8>; 4],
    pal_title: [u8; 16],
    pal_game: [u8; 16],
    pal_game_spr: [u8; 16],
    pal_hub: [u8; 16],
    pal_splash: [[u8; 16]; 3],
    pal_map_spr: [[u8; 16]; 3],
}

fn pal16(rom: &Rom, addr: u16) -> [u8; 16] {
    rom.cpu_slice(addr, 16).try_into().unwrap()
}

fn extract_all(rom: &Rom) -> Result<Extracted> {
    let backdrop: [u8; 1024] = rom.cpu_slice(BACKDROP_ADDR, 1024).try_into().unwrap();

    let title_stream =
        stream_bytes(&rom.chr[TITLE_BANK * 0x1000 + TITLE_OFF..TITLE_BANK * 0x1000 + 0x1000])?;
    let map_frame_stream =
        stream_bytes(&rom.chr[MAP_FRAME_BANK * 0x1000 + MAP_FRAME_OFF..MAP_FRAME_BANK * 0x1000 + 0x1000])?;
    let hub_img_stream =
        stream_bytes(&rom.chr[MAP_FRAME_BANK * 0x1000 + HUB_IMG_OFF..MAP_FRAME_BANK * 0x1000 + 0x1000])?;

    let mut world_img_streams: [Vec<u8>; 6] = Default::default();
    let mut world_attrs = [[0u8; 32]; 6];
    for w in 0..6 {
        let e = WORLD_IMG_TABLE + 4 * w as u16;
        let bank = rom.cpu(e) as usize;
        let addr = rom.cpu_u16(e + 2);
        ensure!(bank < 8, "world {w}: CHR bank {bank} out of range");
        ensure!((0x1000..0x2000).contains(&addr), "world {w}: bad image PPU addr ${addr:04X}");
        let off = (addr - 0x1000) as usize;
        world_img_streams[w] = stream_bytes(&rom.chr[bank * 0x1000 + off..bank * 0x1000 + 0x1000])?;
        let aoff = WORLD_ATTRS_BANK * 0x1000 + WORLD_ATTRS_OFF + w * 0x20;
        world_attrs[w].copy_from_slice(&rom.chr[aoff..aoff + 32]);
    }

    let ending_overlay_stream = stream_bytes(rom.cpu_slice(ENDING_OVERLAY_STREAM, 64))?;
    let next_stage_stream = stream_bytes(rom.cpu_slice(NEXT_STAGE_STREAM, 64))?;
    let mut splash_streams: [Vec<u8>; 4] = Default::default();
    for d in 0..4 {
        let ptr = rom.cpu_u16(SPLASH_TABLE + 2 * d as u16);
        let avail = 0x10000 - ptr as usize; // clamp the window at PRG end
        splash_streams[d] = stream_bytes(rom.cpu_slice(ptr, avail.min(512)))?;
    }

    // Scripts: capture the contiguous head; caption/splash scripts then FC-jump
    // to shared tails which we append so each emitted blob decodes standalone.
    let caption_tail = script_head_bytes(rom, CAPTION_TAIL)?;
    let splash_tail = script_head_bytes(rom, SPLASH_SCRIPT_TAIL)?;
    let mut caption_scripts: [Vec<u8>; 6] = Default::default();
    for w in 0..6 {
        let ptr = rom.cpu_u16(CAPTION_TABLE + 2 * w as u16);
        let mut blob = script_head_bytes(rom, ptr)?;
        blob.truncate(blob.len() - 3); // drop the FC lo hi jump
        blob.extend_from_slice(&caption_tail);
        caption_scripts[w] = blob;
    }
    let mut splash_scripts: [Vec<u8>; 4] = Default::default();
    for d in 0..4 {
        let ptr = rom.cpu_u16(SPLASH_SCRIPT_TABLE + 2 * d as u16);
        let mut blob = script_head_bytes(rom, ptr)?;
        blob.truncate(blob.len() - 3);
        blob.extend_from_slice(&splash_tail);
        splash_scripts[d] = blob;
    }
    let hub_script = script_head_bytes(rom, HUB_SCRIPT)?;
    let mut ending_scripts: [Vec<u8>; 6] = Default::default();
    for w in 0..6 {
        let ptr = rom.cpu_u16(ENDING_TABLE + 2 * w as u16);
        ending_scripts[w] = script_head_bytes(rom, ptr)?;
    }

    Ok(Extracted {
        attract: decode_record_table(rom, ATTRACT_TABLE, ATTRACT_COUNT)?,
        color_box: decode_record_table(rom, COLOR_BOX_TABLE, COLOR_BOX_COUNT)?,
        color_text: decode_record_table(rom, COLOR_TEXT_TABLE, COLOR_TEXT_COUNT)?,
        password: decode_record_table(rom, PASSWORD_TABLE, 1)?,
        password_rows: decode_record_table(rom, PASSWORD_ROWS_TABLE, PASSWORD_ROWS_COUNT)?,
        status: decode_record_table(rom, STATUS_TABLE, STATUS_COUNT)?,
        game_over: decode_record_table(rom, GAME_OVER_TABLE, GAME_OVER_COUNT)?,
        backdrop,
        title_stream,
        map_frame_stream,
        world_img_streams,
        world_attrs,
        hub_img_stream,
        ending_overlay_stream,
        next_stage_stream,
        splash_streams,
        caption_scripts,
        hub_script,
        ending_scripts,
        splash_scripts,
        pal_title: pal16(rom, PAL_TITLE_ADDR),
        pal_game: pal16(rom, PAL_GAME_ADDR),
        pal_game_spr: pal16(rom, PAL_GAME_SPR_ADDR),
        pal_hub: pal16(rom, PAL_HUB_ADDR),
        pal_splash: [
            pal16(rom, PAL_SPLASH_ADDRS[0]),
            pal16(rom, PAL_SPLASH_ADDRS[1]),
            pal16(rom, PAL_SPLASH_ADDRS[2]),
        ],
        pal_map_spr: [
            pal16(rom, PAL_MAP_SPR_ADDRS[0]),
            pal16(rom, PAL_MAP_SPR_ADDRS[1]),
            pal16(rom, PAL_MAP_SPR_ADDRS[2]),
        ],
    })
}

// ---------------------------------------------------------------------------
// Composited screens (what the game displays, sprites excluded)
// ---------------------------------------------------------------------------

/// State 0 title: NT0 fill tile $17, attrs $FF, then the title stream.
fn build_title_nt(ex: &Extracted) -> Result<Nametable> {
    let mut nt = Nametable::new(0x2000, 0x17, 0xFF);
    let (writes, _) = decode_stream(&ex.title_stream)?;
    nt.apply(&writes);
    Ok(nt)
}

/// State 1 attract: records target NT0 ($2000) and NT2 ($2800) by address.
/// Returns (NT0 with title + menu/score/copyright records, NT2 legal page 2).
fn build_attract_nts(ex: &Extracted) -> Result<(Nametable, Nametable)> {
    let mut nt0 = build_title_nt(ex)?;
    let mut nt2 = Nametable::new(0x2800, 0x00, 0x00);
    for rec in &ex.attract {
        nt0.apply_record(rec);
        nt2.apply_record(rec);
    }
    Ok((nt0, nt2))
}

fn backdrop_nt(ex: &Extracted) -> Nametable {
    let mut nt = Nametable::new(0x2000, 0, 0);
    nt.tiles.copy_from_slice(&ex.backdrop[..960]);
    nt.attrs.copy_from_slice(&ex.backdrop[960..]);
    nt
}

/// State 4 world-map intro: $80 fill + frame + per-world image + per-world
/// attr rows + caption script + "TRY THE NEXT STAGE*" text.
fn build_world_map_nt(rom: &Rom, ex: &Extracted, w: usize) -> Result<Nametable> {
    let mut nt = Nametable::new(0x2400, 0x80, 0xFF);
    let (frame, _) = decode_stream(&ex.map_frame_stream)?;
    nt.apply(&frame);
    let (img, _) = decode_stream(&ex.world_img_streams[w])?;
    nt.apply(&img);
    nt.attrs[8..40].copy_from_slice(&ex.world_attrs[w]);
    nt.apply(&decode_script(rom, rom.cpu_u16(CAPTION_TABLE + 2 * w as u16), CAPTION_ANCHOR)?);
    let (text, _) = decode_stream(&ex.next_stage_stream)?;
    nt.apply(&text);
    Ok(nt)
}

/// State 15 hub / state 17 ending base: fill + frame + hub image.
fn build_hub_base_nt(ex: &Extracted) -> Result<Nametable> {
    let mut nt = Nametable::new(0x2400, 0x80, 0xFF);
    let (frame, _) = decode_stream(&ex.map_frame_stream)?;
    nt.apply(&frame);
    let (img, _) = decode_stream(&ex.hub_img_stream)?;
    nt.apply(&img);
    Ok(nt)
}

fn build_ending_nt(rom: &Rom, ex: &Extracted, w: usize) -> Result<Nametable> {
    let mut nt = build_hub_base_nt(ex)?;
    let (overlay, _) = decode_stream(&ex.ending_overlay_stream)?;
    nt.apply(&overlay);
    nt.apply(&decode_script(rom, rom.cpu_u16(ENDING_TABLE + 2 * w as u16), ENDING_ANCHOR)?);
    Ok(nt)
}

fn build_splash_nt(rom: &Rom, ex: &Extracted, d: usize) -> Result<Nametable> {
    let mut nt = Nametable::new(0x2400, 0x80, 0xFF);
    let (frame, _) = decode_stream(&ex.map_frame_stream)?;
    nt.apply(&frame);
    let (img, _) = decode_stream(&ex.splash_streams[d])?;
    nt.apply(&img);
    nt.apply(&decode_script(rom, rom.cpu_u16(SPLASH_SCRIPT_TABLE + 2 * d as u16), SPLASH_ANCHOR)?);
    Ok(nt)
}

// ---------------------------------------------------------------------------
// screens.rs emission
// ---------------------------------------------------------------------------

fn fmt_byte_slice(bytes: &[u8], indent: &str, per_line: usize) -> String {
    let mut s = String::new();
    for (i, b) in bytes.iter().enumerate() {
        if i % per_line == 0 {
            s.push_str(indent);
        }
        let _ = write!(s, "0x{b:02X},");
        if i % per_line == per_line - 1 || i == bytes.len() - 1 {
            s.push('\n');
        } else {
            s.push(' ');
        }
    }
    s
}

fn emit_records(name: &str, docs: &str, recs: &[Vec<Segment>], out: &mut String) {
    for line in docs.lines() {
        let _ = writeln!(out, "/// {line}");
    }
    let _ = writeln!(out, "pub static {name}: [&[(u16, &[u8])]; {}] = [", recs.len());
    for rec in recs {
        out.push_str("    &[");
        for seg in rec {
            let _ = write!(out, "\n        (0x{:04X}, &[", seg.addr);
            for (i, b) in seg.data.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                let _ = write!(out, "0x{b:02X},");
            }
            out.push_str("]),");
        }
        out.push_str("\n    ],\n");
    }
    out.push_str("];\n\n");
}

fn emit_nametable(name: &str, docs: &str, nt: &Nametable, out: &mut String) {
    for line in docs.lines() {
        let _ = writeln!(out, "/// {line}");
    }
    let _ = writeln!(out, "pub static {name}: Nametable = Nametable {{");
    out.push_str("    tiles: [\n");
    out.push_str(&fmt_byte_slice(&nt.tiles, "        ", 32));
    out.push_str("    ],\n    attrs: [\n");
    out.push_str(&fmt_byte_slice(&nt.attrs, "        ", 16));
    out.push_str("    ],\n};\n\n");
}

fn emit_stream(out: &mut String, name: &str, docs: &str, bytes: &[u8]) {
    for line in docs.lines() {
        let _ = writeln!(out, "/// {line}");
    }
    let _ = writeln!(out, "#[rustfmt::skip]\npub static {name}: &[u8] = &[");
    out.push_str(&fmt_byte_slice(bytes, "    ", 16));
    out.push_str("];\n\n");
}

fn emit_screens_rs(ex: &Extracted, rom: &Rom, ctx: &Ctx) -> Result<()> {
    let mut out = String::with_capacity(256 * 1024);
    out.push_str(
        "//! Generated by `cargo run -p xtask -- extract-screens Boulder_Dash.nes` — do not edit.\n\
         //! Screen/UI data decoded from the NES ROM (see analysis/ghidra_findings.md Q9).\n\
         //!\n\
         //! Source formats:\n\
         //! - 50-byte VRAM records: `(addr_lo, addr_hi, count, data..)` segments, $FF\n\
         //!   terminated (`screen_block_copy` $A808 + NMI uploader $B09D).\n\
         //! - `$A1BD` RLE streams (PRG + CHR bank tails): implicit FE start; FE lo hi =\n\
         //!   set PPU addr + row anchor; FD = next row (anchor += $20 — literal writes\n\
         //!   do NOT move the anchor); FC = toggle +32 write mode; FB n = n literal\n\
         //!   bytes (the only way to emit tiles $FB-$FF); FF = end. See [`decode_stream`].\n\
         //! - `$E866` map scripts: FE lo hi = set anchor + cursor, FD = next row\n\
         //!   (anchor += $20, cursor = anchor), FF = end, else tile at cursor++ (one\n\
         //!   tile per 4 frames, typewriter). FC jumps are already resolved in the\n\
         //!   emitted blobs. See [`decode_script`].\n\n",
    );
    out.push_str("pub const SCREEN_COLS: usize = 32;\npub const SCREEN_ROWS: usize = 30;\n\n");
    out.push_str(
        "/// One decoded 32x30 nametable: 960 tile indices (row-major) + 64 attribute bytes.\n\
         #[derive(Clone)]\n\
         pub struct Nametable {\n\
         \x20   /// Tile indices, row-major (index = row*32 + col).\n\
         \x20   pub tiles: [u8; 960],\n\
         \x20   /// Attribute table: 8x8 bytes, one per 4x4-tile block; 2 bits per 2x2 quadrant.\n\
         \x20   pub attrs: [u8; 64],\n\
         }\n\n",
    );

    // palettes
    let pals: [(&str, &str, [u8; 16]); 4] = [
        ("PAL_TITLE", "Title/attract BG palette (descriptor at $A674).", ex.pal_title),
        ("PAL_GAMEPLAY", "Gameplay BG palette ($A7D2). Also shown on the map-walk and world-map intro screens.", ex.pal_game),
        ("PAL_GAMEPLAY_SPR", "Gameplay sprite palette ($A7E2); password digits use group 2.", ex.pal_game_spr),
        ("PAL_MAP_HUB", "World-map hub / ending BG palette ($EBC2).", ex.pal_hub),
    ];
    for (name, docs, pal) in pals {
        let _ = writeln!(out, "/// {docs}");
        let _ = writeln!(out, "#[rustfmt::skip]\npub const {name}: [u8; 16] = [");
        out.push_str(&fmt_byte_slice(&pal, "    ", 16));
        out.push_str("];\n\n");
    }
    out.push_str("/// Difficulty-splash BG palettes ($F0CA/$F0DA/$F0EA; difficulties 0/1 share [0]).\n");
    out.push_str("#[rustfmt::skip]\npub const PAL_SPLASH: [[u8; 16]; 3] = [\n");
    for pal in &ex.pal_splash {
        out.push_str("    [\n");
        out.push_str(&fmt_byte_slice(pal, "        ", 16));
        out.push_str("    ],\n");
    }
    out.push_str("];\n\n");
    out.push_str("/// Map-screen sprite palettes ($EA9E/$EAAE/$EABE; worlds 0-5 via table $EA92).\n");
    out.push_str("#[rustfmt::skip]\npub const PAL_MAP_SPRITE: [[u8; 16]; 3] = [\n");
    for pal in &ex.pal_map_spr {
        out.push_str("    [\n");
        out.push_str(&fmt_byte_slice(pal, "        ", 16));
        out.push_str("    ],\n");
    }
    out.push_str("];\n\n");

    // raw backdrop
    out.push_str("\n/// Raw 1024-byte map/menu backdrop image (960 tiles + 64 attrs), copied to\n");
    out.push_str("/// nametable $2000 by `screen_prep` ($A794 -> $A334). Source: PRG $F78E.\n");
    out.push_str("#[rustfmt::skip]\npub const BACKDROP_IMAGE: [u8; 1024] = [\n");
    out.push_str(&fmt_byte_slice(&ex.backdrop, "    ", 32));
    out.push_str("];\n\n");

    // streams
    emit_stream(&mut out, "TITLE_STREAM", "Title/legal screen stream (CHR bank 5 tail, offset $E00). Draws onto nametable $2000 over a solid fill of tile $17, attrs $FF.", &ex.title_stream);
    emit_stream(&mut out, "MAP_FRAME_STREAM", "World-map frame stream: sky/grass window (CHR bank 7, offset $DC0). Draws onto nametable $2400 over an $80 fill / $FF attrs.", &ex.map_frame_stream);
    for w in 0..6 {
        emit_stream(
            &mut out,
            &format!("WORLD_MAP_STREAM_{w}"),
            &format!("World {} map image stream (CHR bank tail via the $EA6E table).", w + 1),
            &ex.world_img_streams[w],
        );
    }
    emit_stream(&mut out, "MAP_HUB_STREAM", "Map hub/ending base image stream (CHR bank 7, offset $F2E).", &ex.hub_img_stream);
    emit_stream(&mut out, "ENDING_OVERLAY_STREAM", "Ending screen overlay stream (PRG $EFF5): decorative tile rows.", &ex.ending_overlay_stream);
    emit_stream(&mut out, "NEXT_STAGE_STREAM", "\"TRY THE NEXT STAGE*\" stream (PRG $E02F), drawn by state 4 over the map.", &ex.next_stage_stream);
    for d in 0..4 {
        emit_stream(
            &mut out,
            &format!("SPLASH_STREAM_{d}"),
            &format!("Difficulty {d} splash image stream (PRG, via the $FD9A table)."),
            &ex.splash_streams[d],
        );
    }

    // per-world attr rows
    out.push_str("/// Per-world map attribute rows: 32 bytes each, written to $27C8\n");
    out.push_str("/// (attribute rows 1-4 of nametable $2400). CHR bank 3, offset $EE0 + world*$20.\n#[rustfmt::skip]\n");
    out.push_str("pub const WORLD_MAP_ATTRS: [[u8; 32]; 6] = [\n");
    for w in &ex.world_attrs {
        out.push_str("    [\n");
        out.push_str(&fmt_byte_slice(w, "        ", 16));
        out.push_str("    ],\n");
    }
    out.push_str("];\n\n");

    // scripts
    emit_stream(&mut out, "HUB_SCRIPT", "Hub map script: \"WONDERFUL*\" typewriter text (anchor $26AB).", &ex.hub_script);
    for w in 0..6 {
        emit_stream(
            &mut out,
            &format!("WORLD_CAPTION_SCRIPT_{w}"),
            &format!("World {} caption script (\"<NAME> WORLD\", anchor $26A9; FC jump to the shared tail resolved).", w + 1),
            &ex.caption_scripts[w],
        );
    }
    for w in 0..6 {
        emit_stream(
            &mut out,
            &format!("ENDING_SCRIPT_{w}"),
            &format!("Ending credits script for world {} (anchor $26A6).", w + 1),
            &ex.ending_scripts[w],
        );
    }
    for d in 0..4 {
        emit_stream(
            &mut out,
            &format!("SPLASH_SCRIPT_{d}"),
            &format!("Difficulty {d} splash script (\"TRY THE NEXT LEVEL* / PASSWORD ...... / PUSH START ...\", anchor $2686; FC jump resolved)."),
            &ex.splash_scripts[d],
        );
    }

    // records
    emit_records("ATTRACT_RECORDS", "State-1 attract/menu text records (table $F634, 13 entries).\n/// Records 0-3 target nametable $2000 (score line, 1P/2P menu, copyright),\n/// 4-11 target $2800 (legal page 2), 12 writes $2BC0 attribute bytes.", &ex.attract, &mut out);
    emit_records("COLOR_SELECT_BOX_RECORDS", "Color-select frame box records (table $FB8E, 5 records over 5 substates).", &ex.color_box, &mut out);
    emit_records("COLOR_SELECT_TEXT_RECORDS", "Color-select text records (table $FC48): \"1 PLAYER <icon> <lives>\", \"COLOR\", \"SELECT\".", &ex.color_text, &mut out);
    emit_records("PASSWORD_RECORDS", "Password screen title record (table $FC70): \"PASSWORD*\" + digit-row blanking.", &ex.password, &mut out);
    emit_records("PASSWORD_ROW_RECORDS", "Password result records (table $FC89, 5): restore map-backdrop tiles behind the digit row.", &ex.password_rows, &mut out);
    emit_records("STATUS_RECORDS", "Pre-cave status records (table $FD43): \"1 PLAYER <icon> <lives>\", \"WORLD 00-00\".", &ex.status, &mut out);
    emit_records("GAME_OVER_RECORDS", "Game-over/continue records (table $FD6B): \"1 PLAYER\", \"CONTINUE\", \"END\", \"PASSWORD*\".", &ex.game_over, &mut out);

    // composited nametables
    let title_nt = build_title_nt(ex)?;
    let (_, attract_bottom) = build_attract_nts(ex)?;
    emit_nametable("TITLE_NT", "Title/legal screen (state 0, nametable $2000): fill $17 + TITLE_STREAM.\n/// CHR bank 5, PAL_TITLE.", &title_nt, &mut out);
    emit_nametable("ATTRACT_BOTTOM_NT", "Attract nametable $2800: fill 0 + ATTRACT_RECORDS[4..13] (legal page 2).\n/// CHR bank 5, PAL_TITLE.", &attract_bottom, &mut out);
    let walk = backdrop_nt(ex);
    emit_nametable("MAP_WALK_NT", "World-map walk backdrop (state 6, nametable $2000): BACKDROP_IMAGE verbatim.\n/// CHR bank 6, PAL_GAMEPLAY.", &walk, &mut out);
    for w in 0..6 {
        let nt = build_world_map_nt(rom, ex, w)?;
        emit_nametable(
            &format!("WORLD_MAP_NT_{w}"),
            &format!("World {} map intro (state 4, nametable $2400): $80 fill + MAP_FRAME_STREAM\n/// + WORLD_MAP_STREAM_{w} + WORLD_CAPTION_SCRIPT_{w} + NEXT_STAGE_STREAM\n/// + WORLD_MAP_ATTRS[{w}]. CHR bank 6, PAL_GAMEPLAY.", w + 1),
            &nt,
            &mut out,
        );
    }
    let mut hub = build_hub_base_nt(ex)?;
    hub.apply(&decode_script(rom, HUB_SCRIPT, HUB_ANCHOR)?);
    emit_nametable("MAP_HUB_NT", "World-map hub (state 15, nametable $2400): frame + MAP_HUB_STREAM + HUB_SCRIPT.\n/// CHR bank 6, PAL_MAP_HUB.", &hub, &mut out);
    for w in 0..6 {
        let nt = build_ending_nt(rom, ex, w)?;
        emit_nametable(
            &format!("ENDING_NT_{w}"),
            &format!("Ending screen for world {} (state 17, nametable $2400): frame + MAP_HUB_STREAM\n/// + ENDING_OVERLAY_STREAM + ENDING_SCRIPT_{w}. CHR bank 6, PAL_MAP_HUB.", w + 1),
            &nt,
            &mut out,
        );
    }
    for d in 0..4 {
        let nt = build_splash_nt(rom, ex, d)?;
        emit_nametable(
            &format!("SPLASH_NT_{d}"),
            &format!("Difficulty {d} splash (state 18, nametable $2400): frame + SPLASH_STREAM_{d}\n/// + SPLASH_SCRIPT_{d}. CHR bank 6, PAL_SPLASH[{}].", d.min(2)),
            &nt,
            &mut out,
        );
    }
    let mut color = backdrop_nt(ex);
    for rec in ex.color_box.iter().chain(ex.color_text.iter()) {
        color.apply_record(rec);
    }
    emit_nametable("COLOR_SELECT_NT", "Color select (state 2, nametable $2000): backdrop + box + text records.\n/// CHR bank 6, PAL_GAMEPLAY.", &color, &mut out);
    let mut pw = backdrop_nt(ex);
    pw.apply_record(&ex.password[0]);
    emit_nametable("PASSWORD_NT", "Password entry (state 3, nametable $2000): backdrop + PASSWORD_RECORDS.\n/// Digits are 8x16 sprites (see PASSWORD_DIGIT_*). CHR bank 6, PAL_GAMEPLAY.", &pw, &mut out);
    let mut st = backdrop_nt(ex);
    for rec in &ex.status {
        st.apply_record(rec);
    }
    emit_nametable("STATUS_NT", "Pre-cave status (state 8, nametable $2000): backdrop + STATUS_RECORDS.\n/// CHR bank 6, PAL_GAMEPLAY.", &st, &mut out);
    let mut over = backdrop_nt(ex);
    for rec in &ex.game_over {
        over.apply_record(rec);
    }
    emit_nametable("GAME_OVER_NT", "Game over / continue (state 14, nametable $2000): backdrop + GAME_OVER_RECORDS.\n/// CHR bank 6, PAL_GAMEPLAY.", &over, &mut out);

    // password digit sprite layout ($D158)
    out.push_str(
        "/// Password digit sprites (drawn by $D158): 6 digits at y = PASSWORD_DIGIT_Y,\n\
         /// x = PASSWORD_DIGIT_X + 8*i, 8x16 sprite tiles PASSWORD_DIGIT_TILE + 2*digit\n\
         /// (top) and +1 (bottom) from CHR bank 4 (sprite pattern table 0), sprite\n\
         /// palette group 2 of PAL_GAMEPLAY_SPR.\n\
         pub const PASSWORD_DIGIT_Y: u8 = 0x64;\n\
         pub const PASSWORD_DIGIT_X: u8 = 0x68;\n\
         pub const PASSWORD_DIGIT_TILE: u8 = 0x76;\n\n",
    );

    // decoders
    out.push_str(
        "/// Decode an `$A1BD` RLE stream into (ppu_addr, tile) writes. Semantics verified\n\
         /// against the ROM interpreter at $A1BD: FE lo hi sets the address AND the row\n\
         /// anchor; FD advances the anchor one row ($20) — literal writes do not move the\n\
         /// anchor; FC toggles +32 column-write mode; FB n copies the next n bytes\n\
         /// literally (the only way to write tiles $FB-$FF); FF ends.\n\
         pub fn decode_stream(stream: &[u8]) -> Vec<(u16, u8)> {\n\
         \x20   let mut writes = Vec::new();\n\
         \x20   let mut anchor = stream[0] as usize | (stream[1] as usize) << 8;\n\
         \x20   let mut addr = anchor;\n\
         \x20   let mut pos = 2;\n\
         \x20   let mut inc32 = false;\n\
         \x20   loop {\n\
         \x20       let b = stream[pos];\n\
         \x20       pos += 1;\n\
         \x20       match b {\n\
         \x20           0xFF => break,\n\
         \x20           0xFE => { anchor = stream[pos] as usize | (stream[pos + 1] as usize) << 8; addr = anchor; pos += 2; }\n\
         \x20           0xFD => { anchor += 0x20; addr = anchor; }\n\
         \x20           0xFC => inc32 = !inc32,\n\
         \x20           0xFB => { let n = stream[pos] as usize; pos += 1;\n\
         \x20               for i in 0..n { writes.push((addr as u16, stream[pos + i])); addr += if inc32 { 32 } else { 1 }; }\n\
         \x20               pos += n; }\n\
         \x20           _ => { writes.push((addr as u16, b)); addr += if inc32 { 32 } else { 1 }; }\n\
         \x20       }\n\
         \x20   }\n\
         \x20   writes\n\
         }\n\n\
         /// Decode an `$E866` map script blob into (ppu_addr, tile) writes.\n\
         /// FD/FE semantics match the engine: FE sets anchor + cursor, FD advances the\n\
         /// anchor one row and resets the cursor to it, literals advance only the cursor.\n\
         /// FC jumps are already resolved in the emitted blobs.\n\
         pub fn decode_script(script: &[u8], anchor: u16) -> Vec<(u16, u8)> {\n\
         \x20   let mut writes = Vec::new();\n\
         \x20   let mut pos = 0usize;\n\
         \x20   let mut anchor = anchor;\n\
         \x20   let mut addr = anchor;\n\
         \x20   loop {\n\
         \x20       let b = script[pos];\n\
         \x20       pos += 1;\n\
         \x20       match b {\n\
         \x20           0xFF => break,\n\
         \x20           0xFE => { anchor = script[pos] as u16 | (script[pos + 1] as u16) << 8; addr = anchor; pos += 2; }\n\
         \x20           0xFD => { anchor += 0x20; addr = anchor; }\n\
         \x20           0xFC => unreachable!(\"emitted scripts have FC jumps resolved\"),\n\
         \x20           _ => { writes.push((addr, b)); addr += 1; }\n\
         \x20       }\n\
         \x20   }\n\
         \x20   writes\n\
         }\n",
    );

    let path = ctx.data_dir.join("screens.rs");
    std::fs::write(&path, &out).with_context(|| format!("write {}", path.display()))?;
    eprintln!("wrote {} ({} KiB)", path.display(), out.len() / 1024);
    Ok(())
}

// ---------------------------------------------------------------------------
// PNG previews
// ---------------------------------------------------------------------------

fn render_previews(rom: &Rom, ex: &Extracted, ctx: &Ctx) -> Result<usize> {
    let dir = ctx.assets_dir.join("screens");
    std::fs::create_dir_all(&dir)?;
    let mut n = 0;

    let (attract_top, attract_bottom) = build_attract_nts(ex)?;
    let title_nt = build_title_nt(ex)?;
    write_png(&dir.join("title.png"), 256, 240, &render_nt(rom, &title_nt, TITLE_BANK as u8, &ex.pal_title))?;
    // attract: NT0 (title+menu records) stacked over NT2 (legal page 2) — vertical scroll order
    let mut both = render_nt(rom, &attract_top, TITLE_BANK as u8, &ex.pal_title);
    both.extend_from_slice(&render_nt(rom, &attract_bottom, TITLE_BANK as u8, &ex.pal_title));
    write_png(&dir.join("attract.png"), 256, 480, &both)?;
    n += 2;

    let walk = backdrop_nt(ex);
    write_png(&dir.join("map_walk.png"), 256, 240, &render_nt(rom, &walk, MENU_BANK, &ex.pal_game))?;
    n += 1;

    for w in 0..6 {
        let nt = build_world_map_nt(rom, ex, w)?;
        write_png(&dir.join(format!("map_world_{w}.png")), 256, 240, &render_nt(rom, &nt, MENU_BANK, &ex.pal_game))?;
        n += 1;
    }
    let mut hub = build_hub_base_nt(ex)?;
    hub.apply(&decode_script(rom, HUB_SCRIPT, HUB_ANCHOR)?);
    write_png(&dir.join("map_hub.png"), 256, 240, &render_nt(rom, &hub, MENU_BANK, &ex.pal_hub))?;
    n += 1;
    for w in 0..6 {
        let nt = build_ending_nt(rom, ex, w)?;
        write_png(&dir.join(format!("ending_{w}.png")), 256, 240, &render_nt(rom, &nt, MENU_BANK, &ex.pal_hub))?;
        n += 1;
    }
    for d in 0..4 {
        let nt = build_splash_nt(rom, ex, d)?;
        write_png(&dir.join(format!("splash_{d}.png")), 256, 240, &render_nt(rom, &nt, MENU_BANK, &ex.pal_splash[d.min(2)]))?;
        n += 1;
    }

    let mut color = backdrop_nt(ex);
    for rec in ex.color_box.iter().chain(ex.color_text.iter()) {
        color.apply_record(rec);
    }
    write_png(&dir.join("color_select.png"), 256, 240, &render_nt(rom, &color, MENU_BANK, &ex.pal_game))?;
    n += 1;

    let mut pw = backdrop_nt(ex);
    pw.apply_record(&ex.password[0]);
    let mut pw_img = render_nt(rom, &pw, MENU_BANK, &ex.pal_game);
    // digit sprites "000000": OAM y=$64 (renders at $65), x=$68+8i, tiles $76/$77,
    // bank 4, sprite palette group 2 of the gameplay sprite palette.
    let spr_pal: [u8; 4] = ex.pal_game_spr[8..12].try_into().unwrap();
    for i in 0..6usize {
        draw_sprite_8x16(&mut pw_img, rom, 4, 0x76, 0x68 + i * 8, 0x65, spr_pal);
    }
    write_png(&dir.join("password.png"), 256, 240, &pw_img)?;
    n += 1;

    let mut st = backdrop_nt(ex);
    for rec in &ex.status {
        st.apply_record(rec);
    }
    write_png(&dir.join("status.png"), 256, 240, &render_nt(rom, &st, MENU_BANK, &ex.pal_game))?;
    n += 1;

    let mut over = backdrop_nt(ex);
    for rec in &ex.game_over {
        over.apply_record(rec);
    }
    write_png(&dir.join("game_over.png"), 256, 240, &render_nt(rom, &over, MENU_BANK, &ex.pal_game))?;
    n += 1;

    eprintln!("wrote {n} PNGs to {}", dir.display());
    Ok(n)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn extract(rom: &Rom, ctx: &Ctx) -> Result<()> {
    let ex = extract_all(rom)?;

    // Spec checks (hard-fail on structural drift):
    // - 13 attract records; rec0 writes 20 tiles at $21A4 ("1P ... HI SCORE ... 2P").
    ensure!(ex.attract.len() == 13, "attract table: {} records", ex.attract.len());
    ensure!(
        ex.attract[0][0].addr == 0x21A4 && ex.attract[0][0].data.len() == 20,
        "attract rec0 shape: ${:04X} x{}",
        ex.attract[0][0].addr,
        ex.attract[0][0].data.len()
    );
    // - backdrop starts with tile $80 (blank sky) and contains "WORLD MAP*" in bank-6 font.
    ensure!(ex.backdrop[0] == 0x80, "backdrop first tile = {:02X}", ex.backdrop[0]);
    const WORLD_MAP_TEXT: [u8; 10] = [0xA1, 0x99, 0x9C, 0x96, 0x8E, 0x80, 0x97, 0x8B, 0x9A, 0xA7];
    ensure!(
        ex.backdrop.windows(10).any(|w| w == WORLD_MAP_TEXT),
        "backdrop does not contain the WORLD MAP* title"
    );
    // - caption script 0 writes "BOULDER" (bank-6 font tiles) starting at $26A9.
    let cap0 = decode_script(rom, rom.cpu_u16(CAPTION_TABLE), CAPTION_ANCHOR)?;
    const BOULDER: [u8; 7] = [0x8C, 0x99, 0x9F, 0x96, 0x8E, 0x8F, 0x9C];
    ensure!(cap0.len() >= 7, "caption 0 too short");
    for (i, b) in BOULDER.iter().enumerate() {
        ensure!(
            cap0[i] == (CAPTION_ANCHOR + i as u16, *b),
            "caption 0 tile {i}: {:?} != expected",
            cap0[i]
        );
    }
    // - title stream anchors at $206C; splash script 0 contains password "423480".
    let (tw, _) = decode_stream(&ex.title_stream)?;
    ensure!(tw[0].0 == 0x206C, "title stream first write at ${:04X}", tw[0].0);
    const PW_D01: [u8; 6] = [0x85, 0x83, 0x84, 0x85, 0x89, 0x81]; // "423480"
    ensure!(
        ex.splash_scripts[0].windows(6).any(|w| w == PW_D01),
        "splash script 0 does not contain the difficulty-0 password digits"
    );

    emit_screens_rs(&ex, rom, ctx)?;
    let png_count = render_previews(rom, &ex, ctx)?;

    // Summary
    let (tw, used) = decode_stream(&ex.title_stream)?;
    println!("title stream: {} tiles ({} stream bytes)", tw.len(), used);
    let (fw, used) = decode_stream(&ex.map_frame_stream)?;
    println!("map frame:    {} tiles ({} stream bytes)", fw.len(), used);
    for w in 0..6 {
        let (ww, used) = decode_stream(&ex.world_img_streams[w])?;
        println!("world {w} map: {} tiles ({} stream bytes)", ww.len(), used);
    }
    println!("attract records: {}", ex.attract.len());
    println!("wrote {png_count} screen PNGs");
    Ok(())
}
