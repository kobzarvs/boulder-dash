//! Rockford sprite extraction (Ghidra Q7): body background-metatile quad +
//! head OAM metasprite tables, PNG proof sheet, and `src/data/sprites.rs`.
//!
//! ROM layout (all verified against the disassembly, see
//! analysis/ghidra_findings.md Q7):
//! - Body: metatile quad at $F44F = tiles $36/$38/$37/$39 of the background
//!   pattern table (CHR1 banks 0-3, animated by the per-frame bank swap
//!   $6F = ($FE&$18)>>3). This holds for EVERY cave: $E0 is in the animated
//!   metatile list (no per-cave-group variant offset), and bank 6's tiles
//!   $36-$39 are unrelated worlds-5/6 art — see the proof sheet's last row.
//! - Head: 2-sprite metasprites (16x8 overlay) at $D492-$D5E7, record
//!   format [Y, tile, attr, X] x2 + $7F terminator. Sprites use pattern
//!   table 0 = CHR bank 4. Record offsets place the head at the body's
//!   top-left corner (Y=-16, X=-8/0 relative to the draw reference).
//! - Tables: idle blink $D492 = [open, closed, open, closed] selected by
//!   ($FE&$60)>>5; direction list $D4AC = [up, down, left, right], each
//!   4 walk frames selected by ($FE&$18)>>3; death wobble $D564 (4 ptrs,
//!   ($FE&$30)>>4); door static $D586; float $D57E (4 frames, ($FE&$C)>>2);
//!   suicide flash $D5AA (4 ptrs); statics $D5D6/$D5DF.

use crate::{rom::Rom, Ctx};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

// ---------------------------------------------------------------------------
// ROM addresses
// ---------------------------------------------------------------------------

/// Body metatile quad ($F44F), reached via pointer $F3A1 (metatile 15).
const BODY_QUAD_ADDR: u16 = 0xF44F;
const BODY_QUAD: [u8; 4] = [0x36, 0x38, 0x37, 0x39];

const IDLE_TABLE: u16 = 0xD492; // 4 ptrs: [D49A, D4A3, D49A, D4A3]
const DIR_TABLE: u16 = 0xD4AC; // 4 ptrs: up/down/left/right frame-pointer lists
const DEATH_TABLE: u16 = 0xD564; // 4 ptrs: [D56C, D575, D56C, D575]
const FLOAT_TABLE: u16 = 0xD57E; // 4 ptrs: [D586, D58F, D598, D5A1]
const SUICIDE_TABLE: u16 = 0xD5AA; // 4 ptrs: [D5B2, D5BB, D5C4, D5CD]
const STATIC7_FRAME: u16 = 0xD5D6; // $BA/$BA h-flip, Y=-24
const STATIC8_FRAME: u16 = 0xD5DF; // $BC/$BC h-flip
/// First byte past the sprite data (sprite_draw_at_5f code starts at $D5E8;
/// $D618+ is the cave-complete walk-out data, not extracted here).
const TABLES_END: u16 = 0xD5E8;

/// CHR bank holding the head tiles (sprite pattern table 0 in gameplay).
const HEAD_BANK: usize = 4;
/// CHR banks holding the body animation frames (CHR1 swap in gameplay).
const BODY_BANKS: [usize; 4] = [0, 1, 2, 3];
/// Rendered in the proof sheet for contrast: bank 6's tiles $36-$39 are NOT
/// body art (worlds-5/6 tiles land at the same indices).
const BODY_BANK_W56: usize = 6;

// ---------------------------------------------------------------------------
// Metasprite parsing
// ---------------------------------------------------------------------------

/// One parsed head metasprite: the two [Y, tile, attr, X] sprite quads.
type Frame = [[u8; 4]; 2];

/// Read a 2-sprite metasprite at `addr` (must be exactly 2 quads + $7F).
fn parse_frame(rom: &Rom, addr: u16) -> Result<Frame> {
    ensure!(
        (0xD492..TABLES_END).contains(&addr),
        "frame pointer ${addr:04X} outside the metasprite region"
    );
    let b = rom.cpu_slice(addr, 9);
    ensure!(b[8] == 0x7F, "frame at ${addr:04X}: missing $7F terminator ({:02X?})", b);
    let f: Frame = [b[0..4].try_into().unwrap(), b[4..8].try_into().unwrap()];
    for (i, s) in f.iter().enumerate() {
        ensure!(
            s[2] == 0x00 || s[2] == 0x40,
            "frame at ${addr:04X} sprite {i}: unexpected attr {:02X}",
            s[2]
        );
    }
    // Left half at X=-8, right half at X=0, same Y.
    ensure!(f[0][3] == 0xF8 && f[1][3] == 0x00, "frame at ${addr:04X}: bad X offsets");
    ensure!(f[0][0] == f[1][0], "frame at ${addr:04X}: Y offsets differ");
    Ok(f)
}

/// Read a pointer table of `count` LE entries and parse each pointed frame.
fn parse_table(rom: &Rom, table: u16, count: usize) -> Result<Vec<Frame>> {
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        out.push(parse_frame(rom, rom.cpu_u16(table + 2 * i as u16))?);
    }
    Ok(out)
}

struct Sprites {
    idle: Vec<Frame>,   // blink cycle: open, closed, open, closed
    walk_up: Vec<Frame>, // 4 per direction
    walk_down: Vec<Frame>,
    walk_left: Vec<Frame>,
    walk_right: Vec<Frame>,
    death: Vec<Frame>,   // wobble: A, B, A, B
    float: Vec<Frame>,   // rising: head Y -8/-16/-8 above the door pose
    suicide: Vec<Frame>,
    door: Frame,
    static7: Frame,
    static8: Frame,
    /// Every distinct head tile used by the frames above, sorted.
    head_tiles: Vec<u8>,
}

fn extract_all(rom: &Rom) -> Result<Sprites> {
    // Spec checks on the documented table shapes.
    let idle_ptrs: Vec<u16> = (0..4).map(|i| rom.cpu_u16(IDLE_TABLE + 2 * i)).collect();
    ensure!(idle_ptrs == [0xD49A, 0xD4A3, 0xD49A, 0xD4A3], "idle table: {idle_ptrs:04X?}");
    let dir_ptrs: Vec<u16> = (0..4).map(|i| rom.cpu_u16(DIR_TABLE + 2 * i)).collect();
    ensure!(dir_ptrs == [0xD4B4, 0xD4E0, 0xD50C, 0xD538], "dir table: {dir_ptrs:04X?}");
    let death_ptrs: Vec<u16> = (0..4).map(|i| rom.cpu_u16(DEATH_TABLE + 2 * i)).collect();
    ensure!(death_ptrs == [0xD56C, 0xD575, 0xD56C, 0xD575], "death table: {death_ptrs:04X?}");
    let float_ptrs: Vec<u16> = (0..4).map(|i| rom.cpu_u16(FLOAT_TABLE + 2 * i)).collect();
    ensure!(float_ptrs == [0xD586, 0xD58F, 0xD598, 0xD5A1], "float table: {float_ptrs:04X?}");
    let sui_ptrs: Vec<u16> = (0..4).map(|i| rom.cpu_u16(SUICIDE_TABLE + 2 * i)).collect();
    ensure!(sui_ptrs == [0xD5B2, 0xD5BB, 0xD5C4, 0xD5CD], "suicide table: {sui_ptrs:04X?}");

    // Body quad + per-bank animation (banks 0-3 must differ = walk frames).
    let quad: [u8; 4] = rom.cpu_slice(BODY_QUAD_ADDR, 4).try_into().unwrap();
    ensure!(quad == BODY_QUAD, "body quad at $F44F: {quad:02X?}");
    for &t in &BODY_QUAD {
        let b0 = &rom.chr[t as usize * 16..t as usize * 16 + 16];
        let animated = (1..4).any(|b| rom.chr[b * 0x1000 + t as usize * 16..b * 0x1000 + t as usize * 16 + 16] != *b0);
        ensure!(animated, "body tile ${t:02X}: banks 0-3 identical, no animation");
    }
    let walk_up = parse_table(rom, dir_ptrs[0], 4)?;
    let walk_down = parse_table(rom, dir_ptrs[1], 4)?;
    let walk_left = parse_table(rom, dir_ptrs[2], 4)?;
    let walk_right = parse_table(rom, dir_ptrs[3], 4)?;
    // Documented tile families (Q7).
    ensure!(walk_up[0][0][1] == 0x2C && walk_up[0][1][1] == 0x2E, "walk up tiles");
    ensure!(walk_down[0][0][1] == 0x34 && walk_down[0][1][1] == 0x36, "walk down tiles");
    ensure!(walk_left[0][0][1] == 0x20 && walk_left[0][1][1] == 0x22, "walk left tiles");
    // Right = left tiles h-flipped ($40 attr).
    ensure!(walk_right[0][0][1] == 0x22 && walk_right[0][0][2] == 0x40, "walk right = flipped left");

    let idle = parse_table(rom, IDLE_TABLE, 4)?;
    ensure!(idle[0][0][1] == 0x3C && idle[1][0][1] == 0x40, "idle blink tiles");
    let death = parse_table(rom, DEATH_TABLE, 4)?;
    ensure!(death[0][0][1] == 0x44 && death[0][1][1] == 0x46, "death tiles");
    let float = parse_table(rom, FLOAT_TABLE, 4)?;
    ensure!(float[0][0][1] == 0x1C && float[1][0][1] == 0x1E, "float tiles");
    let suicide = parse_table(rom, SUICIDE_TABLE, 4)?;
    let door = parse_frame(rom, 0xD586)?;
    let static7 = parse_frame(rom, STATIC7_FRAME)?;
    let static8 = parse_frame(rom, STATIC8_FRAME)?;

    let mut tiles = BTreeSet::new();
    for f in idle
        .iter()
        .chain(&walk_up)
        .chain(&walk_down)
        .chain(&walk_left)
        .chain(&walk_right)
        .chain(&death)
        .chain(&float)
        .chain(&suicide)
        .chain([&door, &static7, &static8])
    {
        tiles.insert(f[0][1]);
        tiles.insert(f[1][1]);
    }

    Ok(Sprites {
        idle,
        walk_up,
        walk_down,
        walk_left,
        walk_right,
        death,
        float,
        suicide,
        door,
        static7,
        static8,
        head_tiles: tiles.into_iter().collect(),
    })
}

// ---------------------------------------------------------------------------
// sprites.rs emission
// ---------------------------------------------------------------------------

fn emit_frames(out: &mut String, name: &str, docs: &str, frames: &[Frame]) {
    for line in docs.lines() {
        let _ = writeln!(out, "/// {line}");
    }
    let _ = writeln!(out, "pub static {name}: [HeadFrame; {}] = [", frames.len());
    for f in frames {
        let _ = writeln!(
            out,
            "    [[0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}], [0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}]],",
            f[0][0], f[0][1], f[0][2], f[0][3], f[1][0], f[1][1], f[1][2], f[1][3]
        );
    }
    out.push_str("];\n\n");
}

fn emit_sprites_rs(sp: &Sprites, ctx: &Ctx) -> Result<()> {
    let mut out = String::with_capacity(16 * 1024);
    out.push_str(
        "//! Generated by `cargo run -p xtask -- extract-sprites Boulder_Dash.nes` — do not edit.\n\
         //! Rockford sprite data (Ghidra Q7, tables $D492-$D5E7).\n\
         //!\n\
         //! The body is background metatile 15 ([`ROCKFORD_BODY_QUAD`], tiles of the\n\
         //! background pattern table; CHR1 banks 0-3 hold the 4 walk-animation\n\
         //! frames, selected by the bank swap $6F = ($FE&$18)>>3 — in every cave,\n\
         //! since $E0 is in the animated metatile list and bank 6's tiles $36-$39\n\
         //! are unrelated worlds-5/6 art).\n\
         //! The head is a 2-sprite 16x8 OAM overlay from CHR bank 4 (sprite pattern\n\
         //! table 0, sprite palette 0; palette color 2 = the selected suit color,\n\
         //! written to $3D by the color-select screen). Record offsets are signed and\n\
         //! place the head at the body's top-left corner (Y=-16, X=-8 left / 0 right);\n\
         //! attr $40 = horizontal flip. Frame-select formulas match the ROM:\n\
         //! walk ($FE&$18)>>3, idle blink ($FE&$60)>>5, death ($FE&$30)>>4,\n\
         //! float ($FE&$C)>>2.\n\n",
    );
    out.push_str(
        "/// One OAM sprite entry: `[y_ofs, tile, attr, x_ofs]` (signed offsets, attr $40 = h-flip).\n\
         pub type SpriteQuad = [u8; 4];\n\
         /// Rockford head metasprite: left + right halves (16x8 pixels).\n\
         pub type HeadFrame = [SpriteQuad; 2];\n\n",
    );
    let _ = writeln!(
        out,
        "/// Body metatile quad ($F44F): tiles of the background pattern table; the\n\
         /// animation frame is the CHR1 bank ((gameplay frame/8)%4) in every cave.\n\
         pub const ROCKFORD_BODY_QUAD: [u8; 4] = [0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}];\n",
        BODY_QUAD[0], BODY_QUAD[1], BODY_QUAD[2], BODY_QUAD[3]
    );
    emit_frames(&mut out, "ROCKFORD_HEAD_IDLE", "Idle blink cycle ($D492): eyes open, closed, open, closed; index (frame/32)%4.", &sp.idle);
    emit_frames(&mut out, "ROCKFORD_HEAD_WALK_UP", "Walk up ($D4B4), 4 frames; index (frame/8)%4.", &sp.walk_up);
    emit_frames(&mut out, "ROCKFORD_HEAD_WALK_DOWN", "Walk down ($D4E0), 4 frames; index (frame/8)%4.", &sp.walk_down);
    emit_frames(&mut out, "ROCKFORD_HEAD_WALK_LEFT", "Walk left ($D50C), 4 frames; index (frame/8)%4.", &sp.walk_left);
    emit_frames(&mut out, "ROCKFORD_HEAD_WALK_RIGHT", "Walk right ($D538) = left tiles with the $40 h-flip attr, 4 frames.", &sp.walk_right);
    emit_frames(&mut out, "ROCKFORD_HEAD_DEATH", "Death wobble ($D564): A, B, A, B; index (frame/16)%4.", &sp.death);
    emit_frames(&mut out, "ROCKFORD_HEAD_FLOAT", "Door/float rise ($D57E): Y offsets -8/-16/-8 vs the door pose; index (frame/4)%4.", &sp.float);
    emit_frames(&mut out, "ROCKFORD_HEAD_SUICIDE", "Suicide flash ($D5AA): $B8/walk/$B6/flipped-walk cycle.", &sp.suicide);
    emit_frames(&mut out, "ROCKFORD_HEAD_DOOR", "Door-walk static pose ($D586, tiles $1C/$1C h-flip pair).", &[sp.door]);
    emit_frames(&mut out, "ROCKFORD_HEAD_STATIC7", "Static pose 7 ($D5D6, tiles $BA, head 8px above the body).", &[sp.static7]);
    emit_frames(&mut out, "ROCKFORD_HEAD_STATIC8", "Static pose 8 ($D5DF, tiles $BC).", &[sp.static8]);

    out.push_str("/// Every distinct head tile used by the frames above (CHR bank 4), sorted.\n");
    let _ = writeln!(out, "pub static ROCKFORD_HEAD_TILES: [u8; {}] = [", sp.head_tiles.len());
    for (i, t) in sp.head_tiles.iter().enumerate() {
        if i % 16 == 0 {
            out.push_str("    ");
        }
        let _ = write!(out, "0x{t:02X}, ");
        if i % 16 == 15 || i == sp.head_tiles.len() - 1 {
            out.push('\n');
        }
    }
    out.push_str("];\n");

    let path = ctx.data_dir.join("sprites.rs");
    std::fs::write(&path, &out).with_context(|| format!("write {}", path.display()))?;
    eprintln!("wrote {} ({} KiB)", path.display(), out.len() / 1024);
    Ok(())
}

// ---------------------------------------------------------------------------
// PNG proof sheet
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

/// Gameplay BG palette group 0 ($A7D2): the body metatile's palette.
const BODY_PAL: [u8; 4] = [0x22, 0x37, 0x27, 0x07];
/// Gameplay sprite palette group 0 ($A7E2) with the default suit color $26.
const HEAD_PAL: [u8; 4] = [0x22, 0x36, 0x26, 0x0F];

fn tile_pixel(rom: &Rom, bank: usize, tile: u8, row: usize, col: usize) -> u8 {
    let base = bank * 0x1000 + tile as usize * 16;
    let p0 = rom.chr[base + row];
    let p1 = rom.chr[base + 8 + row];
    let bit = 7 - col;
    ((p0 >> bit) & 1) | (((p1 >> bit) & 1) << 1)
}

fn put_px(img: &mut [u8], w: usize, x: usize, y: usize, scale: usize, rgb: [u8; 3]) {
    for dy in 0..scale {
        for dx in 0..scale {
            let o = ((y * scale + dy) * w + x * scale + dx) * 3;
            img[o..o + 3].copy_from_slice(&rgb);
        }
    }
}

/// Body metatile: 2x2 quad [tl, tr, bl, br], BG palette 0, opaque.
fn draw_body(img: &mut [u8], w: usize, rom: &Rom, body_bank: usize, x: usize, y: usize, scale: usize) {
    for (pos, &t) in BODY_QUAD.iter().enumerate() {
        let (qx, qy) = (pos % 2, pos / 2);
        for r in 0..8 {
            for c in 0..8 {
                let ci = tile_pixel(rom, body_bank, t, r, c) as usize;
                let rgb = NES_PALETTE[(BODY_PAL[ci] & 0x3F) as usize];
                put_px(img, w, x + qx * 8 + c, y + qy * 8 + r, scale, rgb);
            }
        }
    }
}

/// Head overlay: two 8x8 sprites, offsets relative to the body top-left
/// (record Y + 16, record X + 8; negative = above/left of the body);
/// color index 0 transparent. Pixels outside the image are clipped.
#[allow(clippy::too_many_arguments)]
fn draw_head(img: &mut [u8], w: usize, h: usize, rom: &Rom, frame: &Frame, x: i32, y: i32, scale: usize) {
    for s in frame {
        let dx = s[3] as i8 as i32 + 8;
        let dy = s[0] as i8 as i32 + 16;
        let hflip = s[2] & 0x40 != 0;
        for r in 0..8i32 {
            for c in 0..8i32 {
                let sc = if hflip { 7 - c } else { c };
                let ci = tile_pixel(rom, HEAD_BANK, s[1], r as usize, sc as usize) as usize;
                if ci == 0 {
                    continue;
                }
                let (px, py) = (x + dx + c, y + dy + r);
                if px < 0 || py < 0 || px as usize >= w / scale || py as usize >= h / scale {
                    continue;
                }
                let rgb = NES_PALETTE[(HEAD_PAL[ci] & 0x3F) as usize];
                put_px(img, w, px as usize, py as usize, scale, rgb);
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

/// Proof sheet: every frame as a full Rockford (body + head) on black, 4x
/// scale, 8 frames per row. Each cell is 16px wide and 32px tall (16px of
/// headroom above the 16px body for the float/static poses).
fn render_proof(rom: &Rom, sp: &Sprites, ctx: &Ctx) -> Result<()> {
    let scale = 4;
    let cols = 8;
    let rows: Vec<(&str, Vec<Frame>)> = vec![
        ("idle blink", sp.idle.clone()),
        ("walk up", sp.walk_up.clone()),
        ("walk down", sp.walk_down.clone()),
        ("walk left", sp.walk_left.clone()),
        ("walk right", sp.walk_right.clone()),
        ("death", sp.death.clone()),
        ("door + float", [std::slice::from_ref(&sp.door).to_vec(), sp.float.clone()].concat()),
        ("suicide", sp.suicide.clone()),
        ("statics", vec![sp.static7, sp.static8]),
    ];
    let (cw, ch) = (18, 34); // cell size in source pixels (2px padding)
    let w = cols * cw * scale;
    let h = (rows.len() + 1) * ch * scale;
    let black = NES_PALETTE[0x0F];
    let mut img = vec![0u8; w * h * 3];
    for px in img.as_chunks_mut::<3>().0 {
        *px = black;
    }
    for (row, (_, frames)) in rows.iter().enumerate() {
        for (i, f) in frames.iter().enumerate() {
            let x = (i * cw + 1) as i32;
            let y = (row * ch + 17) as i32; // body top-left (16px headroom)
            draw_body(&mut img, w, rom, 0, x as usize, y as usize, scale);
            draw_head(&mut img, w, h, rom, f, x, y, scale);
        }
    }
    // Last row: body-only animation frames, banks 0-3, then bank 6 for
    // contrast (its tiles $36-$39 are worlds-5/6 art, NOT a body).
    let row = rows.len();
    for (i, bank) in BODY_BANKS.iter().chain([&BODY_BANK_W56]).enumerate() {
        let x = i * cw + 1;
        let y = row * ch + 17;
        draw_body(&mut img, w, rom, *bank, x, y, scale);
    }
    let path = ctx.assets_dir.join("sprites_rockford.png");
    write_png(&path, w as u32, h as u32, &img)?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn extract(rom: &Rom, ctx: &Ctx) -> Result<()> {
    let sp = extract_all(rom)?;
    emit_sprites_rs(&sp, ctx)?;
    render_proof(rom, &sp, ctx)?;
    println!(
        "rockford sprites: {} head tiles, frames idle/{} up/{} down/{} left/{} right/{} death/{} float/{} suicide/{}",
        sp.head_tiles.len(),
        sp.idle.len(),
        sp.walk_up.len(),
        sp.walk_down.len(),
        sp.walk_left.len(),
        sp.walk_right.len(),
        sp.death.len(),
        sp.float.len(),
        sp.suicide.len()
    );
    Ok(())
}
