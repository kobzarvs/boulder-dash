use anyhow::{ensure, Context, Result};
use std::path::Path;

/// iNES image: Boulder_Dash.nes (mapper 1 / MMC1, no trainer).
pub struct Rom {
    /// 32 KB PRG-ROM. prg[0] = CPU $8000 (file offset 0x0010).
    pub prg: Vec<u8>,
    /// 32 KB CHR-ROM (file offset 0x8010), eight 4 KB MMC1 banks.
    pub chr: Vec<u8>,
}

impl Rom {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        ensure!(raw.len() >= 16 && &raw[0..4] == b"NES\x1A", "bad iNES magic");
        let prg_len = raw[4] as usize * 16384;
        let chr_len = raw[5] as usize * 8192;
        ensure!(raw.len() >= 16 + prg_len + chr_len, "truncated ROM");
        Ok(Self {
            prg: raw[16..16 + prg_len].to_vec(),
            chr: raw[16 + prg_len..16 + prg_len + chr_len].to_vec(),
        })
    }

    /// Byte at CPU address $8000-$FFFF.
    pub fn cpu(&self, addr: u16) -> u8 {
        self.prg[addr as usize - 0x8000]
    }

    /// Slice at CPU address $8000-$FFFF.
    pub fn cpu_slice(&self, addr: u16, len: usize) -> &[u8] {
        let off = addr as usize - 0x8000;
        &self.prg[off..off + len]
    }

    /// Little-endian u16 at CPU address.
    pub fn cpu_u16(&self, addr: u16) -> u16 {
        self.cpu(addr) as u16 | (self.cpu(addr + 1) as u16) << 8
    }

    /// 4 KB MMC1 CHR bank (0..8).
    pub fn chr_bank(&self, bank: usize) -> &[u8] {
        &self.chr[bank * 0x1000..bank * 0x1000 + 0x1000]
    }
}
