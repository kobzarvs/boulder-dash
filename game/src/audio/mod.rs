//! Audio layer: NES APU synth + Data East sound-driver decoder.
//!
//! * [`apu`] — register-driven 2A03 synth (2 pulse, triangle, noise).
//! * [`player`] — mirrors the ROM's sound driver (tempo accumulator, music
//!   and SFX parse layers, command set $90-$C2).
//! * [`render`] — pre-render any slot to PCM / WAV.
//! * [`slots`] — semantic names for the sound ids used by the game code,
//!   recovered from the ROM call sites (see game/assets/audio_notes.md).

pub mod apu;
pub mod player;
pub mod render;

pub use render::{render_track, render_wav_bytes};

/// Semantic names for sound-driver slots (== ids passed to the ROM's
/// `sound_play`). Confidence noted per entry; music-slot world mapping is
/// proven by the tables at $BF07 (normal) and $D060 (hurry-up).
pub mod slots {
    /// Stops all sound (the ROM's id 0 has no data; used after digging).
    pub const STOP: usize = 0;

    // --- SFX (mask bit7 set; tick every NMI) --------------------------
    /// Diamond collected ($CCE2); also played on death ($C402) and when
    /// entering the exit door ($C1B9). Its channels point at a one-byte
    /// `$A2` (end) stream: the audible effect is that the music cuts out.
    /// Rendered PCM is (near-)silent — that is faithful, not a bug.
    pub const SFX_STING: usize = 1;
    /// Enemy explosion ($C92C). Channels point at a one-byte `$B6` (end)
    /// stream; effectively a click, like slot 1.
    pub const SFX_EXPLOSION: usize = 2;
    /// Boulder/diamond starts falling ($C5E1).
    pub const SFX_FALL_START: usize = 3;
    /// Boulder lands ($C50C).
    pub const SFX_THUD: usize = 4;
    /// Exit door opens ($CCD1).
    pub const SFX_DOOR_OPEN: usize = 5;
    /// Amoeba ambience, retriggered every 8 frames ($C06B).
    pub const SFX_AMOEBA: usize = 6;
    /// Death by time-out or suicide combo ($C3A1, $CFF2).
    pub const SFX_DEATH: usize = 7;
    /// Boulder push ($C306).
    pub const SFX_PUSH: usize = 8;
    /// Game-over sequence, first part ($AE52).
    pub const SFX_GAME_OVER_A: usize = 9;
    /// Game-over sequence, second part ($AE5B); also reused elsewhere.
    pub const SFX_GAME_OVER_B: usize = 10;
    /// Played at cave spawn ($BE22).
    pub const SFX_SPAWN: usize = 11;
    /// Played on the password/game-over screen ($AF83, $DA60).
    pub const SFX_SCREEN_A: usize = 12;
    /// Unidentified short SFX (pulse1+noise).
    pub const SFX_UNKNOWN_13: usize = 13;
    /// Unidentified short SFX, called from $E57E.
    pub const SFX_UNKNOWN_14: usize = 14;
    /// Unidentified short SFX, called from $DF00.
    pub const SFX_UNKNOWN_15: usize = 15;
    /// Unidentified short SFX, called from $E723.
    pub const SFX_UNKNOWN_16: usize = 16;
    /// Played on the password/game-over screen ($AF55).
    pub const SFX_SCREEN_B: usize = 17;
    /// Long "direct-write" SFX pair: 25 pauses the music tempo accumulator,
    /// 26 resumes it; both play the same drone/glissando data. Played around
    /// the cave-card screen ($ABC9/$AC3C).
    pub const SFX_CARD_PAUSE: usize = 25;
    pub const SFX_CARD_RESUME: usize = 26;

    // --- Music (tick = tempo accumulator overflow) --------------------
    /// 435-byte looping track; played from $D8DE (likely ending/credits).
    pub const MUSIC_UNK_18: usize = 18;
    /// 617-byte looping track; played from $A788/$E002 (likely title or
    /// interstitial).
    pub const MUSIC_UNK_19: usize = 19;
    /// World 1 (caves A-D) main theme; table $BF07[0].
    pub const MUSIC_WORLD1: usize = 20;
    /// World 1 hurry-up variant; table $D060[0].
    pub const MUSIC_WORLD1_HURRY: usize = 21;
    /// World 2 (caves E-H).
    pub const MUSIC_WORLD2: usize = 22;
    pub const MUSIC_WORLD2_HURRY: usize = 23;
    /// Short terminating jingle (triangle+noise only), played when a cave is
    /// completed ($AD1B, just before the walk-out sequence).
    pub const JINGLE_CAVE_COMPLETE: usize = 24;
    /// World 3 (caves I-L).
    pub const MUSIC_WORLD3: usize = 27;
    pub const MUSIC_WORLD3_HURRY: usize = 28;
    /// World 4 (caves M-P).
    pub const MUSIC_WORLD4: usize = 29;
    pub const MUSIC_WORLD4_HURRY: usize = 30;
    /// World 5 (caves Q-T).
    pub const MUSIC_WORLD5: usize = 31;
    pub const MUSIC_WORLD5_HURRY: usize = 32;
    /// World 6 (caves U-X).
    pub const MUSIC_WORLD6: usize = 33;
    pub const MUSIC_WORLD6_HURRY: usize = 34;
    /// 865-byte looping track, the largest; played from $EB52 — almost
    /// certainly the title/attract theme.
    pub const MUSIC_TITLE: usize = 35;

    /// Normal-tempo theme per world index 0-5 (ROM table $BF07).
    pub const WORLD_MUSIC: [usize; 6] = [20, 22, 27, 29, 31, 33];
    /// Hurry-up variant per world index 0-5 (ROM table $D060).
    pub const WORLD_MUSIC_HURRY: [usize; 6] = [21, 23, 28, 30, 32, 34];
}

#[cfg(test)]
mod tests {
    use super::apu::Apu;
    use super::player::Player;
    use crate::data::music::{PERIODS, TRACKS, TRACK_COUNT};

    #[test]
    fn periods_table_is_sane() {
        assert_eq!(PERIODS.len(), 96);
        // C2 .. B9: roughly 55 Hz .. 15.7 kHz on the pulse channels.
        assert_eq!(PERIODS[0], 0x06AE);
        assert_eq!(PERIODS[95], 0x0007);
        // Non-increasing (the ROM clamps rounding at the top octave).
        for w in PERIODS.windows(2) {
            assert!(w[0] >= w[1], "{:04X} < {:04X}", w[0], w[1]);
        }
        // Semitone ratio sanity: 12 steps ~= half the period.
        for i in 0..84 {
            let ratio = PERIODS[i] as f64 / PERIODS[i + 12] as f64;
            assert!(
                (1.85..=2.15).contains(&ratio),
                "octave ratio {ratio} at index {i}"
            );
        }
    }

    /// Every slot decodes without panicking; music slots produce notes;
    /// SFX slots terminate on their own.
    #[test]
    fn all_slots_decode() {
        for slot in 0..TRACK_COUNT {
            let mut player = Player::new(slot);
            let mut apu = Apu::new();
            // 20k frames >> any slot's natural length; looping music stops
            // early once every channel has looped twice-ish (we just run the
            // cap here — parse safety is what is under test).
            for _ in 0..20_000 {
                player.tick(&mut apu);
                if player.is_finished() {
                    break;
                }
                if player.all_looped() && player.ticks > 0 {
                    // a couple of extra passes to exercise the loop body
                    for _ in 0..600 {
                        player.tick(&mut apu);
                    }
                    break;
                }
            }
            let empty = TRACKS[slot].is_empty();
            if !empty {
                assert!(
                    player.warnings().is_empty(),
                    "slot {slot} warnings: {:?}",
                    player.warnings()
                );
            }
            if slot >= 18 && !player.is_sfx() {
                assert!(
                    player.notes_played > 0,
                    "music slot {slot} played no notes"
                );
            }
        }
    }

    /// Music slots must loop forever or terminate; SFX must terminate.
    #[test]
    fn slots_have_expected_lifecycle() {
        for slot in 1..TRACK_COUNT {
            let mut player = Player::new(slot);
            let mut apu = Apu::new();
            let cap = 30_000;
            let mut frames = 0;
            while frames < cap && !player.is_finished() && !player.all_looped() {
                player.tick(&mut apu);
                frames += 1;
            }
            assert!(
                player.is_finished() || player.all_looped(),
                "slot {slot} neither ended nor looped within {cap} frames"
            );
            if player.is_sfx() {
                assert!(player.is_finished(), "sfx slot {slot} did not end");
            }
        }
    }

    /// Decoding is deterministic (no clocks/RNG in the player).
    #[test]
    fn decode_is_deterministic() {
        fn trace(slot: usize) -> Vec<(u64, u64)> {
            let mut player = Player::new(slot);
            let mut apu = Apu::new();
            let mut marks = Vec::new();
            for _ in 0..3000 {
                player.tick(&mut apu);
                marks.push((player.ticks, player.notes_played));
            }
            marks
        }
        assert_eq!(trace(19), trace(19));
        assert_eq!(trace(26), trace(26));
    }

    /// Reference note-on trace, cross-validated against an independent
    /// Python model of the disassembled driver: (tick, hw_channel, note),
    /// note = effective index into PERIODS (0xFF = rest).
    #[test]
    fn reference_note_traces() {
        const REST: u8 = 0xFF;
        let golden18: &[(u64, usize, u8)] = &[
            (1, 0, 13), (1, 1, 21), (1, 2, 37), (1, 3, REST), (2, 0, 0),
            (3, 0, REST), (5, 3, 37), (9, 0, 0), (13, 0, 10), (13, 1, 33),
            (13, 2, 38), (17, 0, REST), (17, 3, 38), (21, 0, 0), (21, 1, 21),
            (21, 2, 37), (25, 0, 0), (25, 1, 16), (25, 3, 37), (29, 0, 0),
            (33, 0, 0), (33, 2, 35), (37, 0, 10), (37, 1, 28),
        ];
        let golden24: &[(u64, usize, u8)] = &[
            (1, 2, 17), (1, 3, REST), (4, 3, 17), (7, 2, 21), (10, 3, 21),
            (13, 2, 24), (16, 3, 24), (19, 2, 28), (22, 3, 28), (25, 2, 21),
            (28, 3, 21), (31, 2, 24), (34, 3, 24), (37, 2, 28), (40, 3, 28),
            (43, 2, 31), (46, 3, 31), (49, 2, 33), (49, 3, 29), (57, 2, 33),
            (57, 3, 29), (65, 2, 33), (65, 3, 29), (73, 2, 33),
        ];
        for (slot, golden) in [(18usize, golden18), (24, golden24)] {
            let mut player = Player::new(slot);
            let mut apu = Apu::new();
            for _ in 0..500 {
                player.tick(&mut apu);
            }
            assert_eq!(
                &player.note_log[..golden.len()],
                golden,
                "slot {slot} note trace diverged"
            );
        }
    }
}
