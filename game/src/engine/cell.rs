//! Cell object ids and per-cell state bits, mirroring the NES original: the
//! object id lives in the high nibble of the field byte, state in the low
//! nibble (confirmed by the Ghidra disassembly — see
//! `analysis/ghidra_findings.md` Q1).

use super::input::Direction;

/// Object id stored in each cell (values match the ROM's high-nibble ids).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Obj {
    Space = 0,
    /// "Cell vacated this cycle" marker ($10): written whenever something
    /// leaves a cell; the cleanup scan turns it into real space. The fall
    /// scan only treats exact `Space` as empty, so a just-vacated cell cannot
    /// be re-entered within the same 8-frame cycle.
    Vacated = 1,
    Mud = 2,
    Steel = 3,
    Door = 4,
    Brick = 5,
    MagicWall = 6,
    Boulder = 7,
    Diamond = 8,
    /// Butterfly blast product ($90|slot): ripens into a diamond when its
    /// rotating slot timer (8 slots at $07F8) hits 0.
    PendingDiamond = 9,
    /// Firefly/generic explosion remnant ($A0): clears to `Vacated` when the
    /// explosion linger timer ($B6) hits 0. Rockford may walk through it.
    ExplosionRemnant = 0xA,
    Firefly = 0xB,
    Butterfly = 0xC,
    Amoeba = 0xD,
    /// Rockford occupying a cell ($E0).
    Rockford = 0xE,
    /// Dead amoeba residue ($F0): inert, decorative. Unreachable in normal
    /// play — the cleanup branch that produces it requires the loader's
    /// "no amoeba in cave" state, which by construction has no amoeba cells.
    DeadAmoeba = 0xF,
}

impl Obj {
    pub fn from_u8(v: u8) -> Obj {
        match v {
            0 => Obj::Space,
            1 => Obj::Vacated,
            2 => Obj::Mud,
            3 => Obj::Steel,
            4 => Obj::Door,
            5 => Obj::Brick,
            6 => Obj::MagicWall,
            7 => Obj::Boulder,
            8 => Obj::Diamond,
            9 => Obj::PendingDiamond,
            0xA => Obj::ExplosionRemnant,
            0xB => Obj::Firefly,
            0xC => Obj::Butterfly,
            0xD => Obj::Amoeba,
            0xE => Obj::Rockford,
            0xF => Obj::DeadAmoeba,
            _ => Obj::Space,
        }
    }

    /// Supporting objects a boulder/diamond rolls off ($C514 set): boulder,
    /// diamond, brick wall and magic wall. Mud and steel are NOT rounded.
    pub fn is_rounded(self) -> bool {
        matches!(
            self,
            Obj::Boulder | Obj::Diamond | Obj::Brick | Obj::MagicWall
        )
    }

    pub fn is_enemy(self) -> bool {
        matches!(self, Obj::Firefly | Obj::Butterfly)
    }
}

/// Low-nibble bit 0 ($01): boulder/diamond is falling.
pub const F_FALLING: u8 = 0x01;
/// Low-nibble bit 3 ($08): object already moved this cycle (set for moves
/// into not-yet-scanned cells; cleared by the cleanup scan).
pub const F_MOVED: u8 = 0x08;
/// Enemy direction lives in low-nibble bits 0-1 ($03).
const DIR_MASK: u8 = 0x03;
/// Pending-diamond timer slot lives in low-nibble bits 0-2 ($07).
const SLOT_MASK: u8 = 0x07;

/// One cave cell: object id + low-nibble state (falling bit, enemy direction,
/// moved-this-cycle bit, pending-diamond slot).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub obj: Obj,
    pub flags: u8,
}

impl Cell {
    pub fn new(obj: Obj) -> Cell {
        Cell { obj, flags: 0 }
    }

    pub fn falling(self) -> bool {
        self.flags & F_FALLING != 0
    }

    pub fn set_falling(&mut self, v: bool) {
        self.flags = if v { self.flags | F_FALLING } else { self.flags & !F_FALLING };
    }

    pub fn moved(self) -> bool {
        self.flags & F_MOVED != 0
    }

    pub fn set_moved(&mut self, v: bool) {
        self.flags = if v { self.flags | F_MOVED } else { self.flags & !F_MOVED };
    }

    /// Movement direction carried by enemies (0 = up at load, confirmed).
    pub fn dir(self) -> Direction {
        Direction::from_bits(self.flags & DIR_MASK)
    }

    pub fn set_dir(&mut self, d: Direction) {
        self.flags = (self.flags & !DIR_MASK) | (d as u8);
    }

    /// Rotating timer slot carried by pending diamonds.
    pub fn slot(self) -> u8 {
        self.flags & SLOT_MASK
    }

    pub fn set_slot(&mut self, slot: u8) {
        self.flags = (self.flags & !SLOT_MASK) | (slot & SLOT_MASK);
    }
}
