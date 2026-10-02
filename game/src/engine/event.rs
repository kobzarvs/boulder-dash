//! Per-frame events emitted by the engine for the render/audio layers.

/// How Rockford died.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeathCause {
    /// Falling boulder/diamond landed on him.
    Crushed,
    /// Caught in a 3x3 blast (enemy explosion, enemy adjacency check).
    Explosion,
    /// Cave timer ran out.
    Timeout,
    /// Player held the suicide combo (both buttons) for 64 frames.
    Suicide,
}

/// Sound cue for the audio layer. The engine only reports what happened;
/// mapping cues to NES sound ids is the audio layer's job (reference: dig and
/// push = sound 0, explosion/death = 1, fall start = 3, boulder thud = 4,
/// door open = 5, amoeba ambience = 6, timeout = 7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SoundCue {
    Dig,
    Push,
    /// Object left its resting state and started falling.
    FallStart,
    /// Boulder (never diamond) landed.
    Thud,
    Diamond,
    Explosion,
    MagicWall,
    DoorOpen,
    Amoeba,
    ExtraLife,
    /// 30 time units left.
    HurryUp,
}

/// One observable engine event. `pos`/`center`/`at` are field cell indices
/// (`y * WIDTH + x`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// Rockford collected a diamond; `count` is the running total this attempt.
    DiamondCollected { count: u32 },
    /// Diamond quota met; the exit door opened (global flag — the door cell
    /// keeps id `Door`; render it open while `Cave::door_open()`).
    DoorOpened { pos: Option<usize> },
    /// A 3x3 explosion started at `center`; `to_diamond` selects the outcome
    /// (butterfly -> pending diamonds, anything else -> explosion remnants).
    Explosion { center: usize, to_diamond: bool },
    RockfordDied { cause: DeathCause },
    /// Death arc finished, a reserve life was spent and the cave reloaded.
    Respawned,
    ExtraLife { lives: u8 },
    /// Rockford entered the open exit door; `time_bonus` points were added
    /// (+1 per remaining time unit).
    CaveComplete { time_bonus: u32 },
    /// A falling boulder touched a dormant magic wall; the 4096-frame active
    /// window started.
    MagicWallActivated,
    /// The magic wall window expired; walls stay `MagicWall` but are inert.
    MagicWallExpired,
    /// The amoeba grew one cell (at most one per 8-frame chunk).
    AmoebaGrew { at: usize },
    /// A full amoeba scan pass found no empty cell: all amoeba converts to
    /// diamonds. (`to_diamonds` is always true — the NES ROM has no
    /// overgrowth-to-boulders rule; that C64 mechanic is dead code here.)
    AmoebaConverted { to_diamonds: bool },
    /// Sound cue.
    Sound(SoundCue),
}
