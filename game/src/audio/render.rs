//! Pre-render any sound slot to a PCM buffer (for `macroquad::audio`
//! one-shot playback) or to a WAV byte string.
//!
//! Rendering is offline and deterministic: the driver mirror ticks at the
//! NMI rate (60.0988 Hz) and each tick is followed by one frame's worth of
//! 44.1 kHz samples. Termination:
//! * slots whose channels all end (SFX, the slot-24 jingle) stop there;
//! * looping music stops after the second pass over the loop: once every
//!   channel has hit a backward jump (frame L), rendering continues to
//!   frame 2L + 1 s of tail;
//! * hard cap at [`MAX_SECONDS`].
//!
//! A 20 ms fade-out is applied to avoid an end click.

use super::apu::{Apu, CYCLES_PER_FRAME, NMI_HZ, SAMPLE_RATE};
use super::player::Player;

/// Hard cap on rendered length.
pub const MAX_SECONDS: u32 = 240;
/// Fade-out length at the end of a rendered track.
const FADE_MS: u32 = 20;

/// Render a slot to mono f32 PCM at 44.1 kHz. Empty/slot 0 -> empty buffer.
pub fn render_track(slot: usize) -> Vec<f32> {
    let mut player = Player::new(slot);
    if player.channels_empty() {
        return Vec::new();
    }
    let mut apu = Apu::new();
    let mut pcm = Vec::new();
    let max_frames = (MAX_SECONDS as f64 * NMI_HZ) as u64;
    let mut loop_frame: Option<u64> = None;
    for frame in 0..max_frames {
        player.tick(&mut apu);
        apu.render_cycles(CYCLES_PER_FRAME, &mut pcm);
        if player.is_finished() {
            // Short tail so the last note's envelope can settle.
            apu.render_cycles(CYCLES_PER_FRAME * 0.25, &mut pcm);
            break;
        }
        if loop_frame.is_none() && player.all_looped() {
            loop_frame = Some(frame);
        }
        if let Some(lf) = loop_frame {
            if frame >= lf * 2 + NMI_HZ as u64 {
                break;
            }
        }
    }
    fade_out(&mut pcm);
    pcm
}

/// Render a slot to a 16-bit mono PCM WAV byte string (44.1 kHz),
/// suitable for `macroquad::audio::load_sound_from_bytes`.
pub fn render_wav_bytes(slot: usize) -> Vec<u8> {
    let pcm = render_track(slot);
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM header size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in &pcm {
        let v = (s.clamp(-1.0, 1.0) * 0.9 * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn fade_out(pcm: &mut [f32]) {
    let n = (SAMPLE_RATE as usize * FADE_MS as usize / 1000).min(pcm.len());
    let start = pcm.len() - n;
    for (i, s) in pcm[start..].iter_mut().enumerate() {
        *s *= 1.0 - (i + 1) as f32 / n as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::music::TRACK_COUNT;

    /// Slots whose header has the SFX flag (mask bit7).
    fn is_sfx_slot(slot: usize) -> bool {
        !crate::data::music::TRACKS[slot].is_empty()
            && crate::data::music::TRACKS[slot][0] & 0x80 != 0
    }

    fn music_slots() -> Vec<usize> {
        (1..TRACK_COUNT).filter(|&s| !is_sfx_slot(s)).collect()
    }

    #[test]
    fn wav_header_is_well_formed() {
        let wav = render_wav_bytes(3); // short noise SFX
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[20..22], 1u16.to_le_bytes());
        assert_eq!(&wav[22..24], 1u16.to_le_bytes());
        assert_eq!(&wav[24..28], SAMPLE_RATE.to_le_bytes());
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
        assert_eq!(wav.len(), 44 + data_len);
        assert!(data_len > 0);
    }

    #[test]
    fn music_slots_render_non_silent() {
        for slot in music_slots() {
            let pcm = render_track(slot);
            assert!(!pcm.is_empty(), "slot {slot} rendered empty");
            let peak = pcm.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
            // Slot 1's channels end on the first byte by design.
            if slot == 1 {
                continue;
            }
            assert!(peak > 0.05, "slot {slot} peak {peak} (silent?)");
            assert!(peak <= 1.0, "slot {slot} peak {peak} (clipping?)");
        }
    }

    #[test]
    fn sfx_slots_render_non_silent() {
        for slot in (1..TRACK_COUNT).filter(|&s| is_sfx_slot(s)) {
            let pcm = render_track(slot);
            assert!(!pcm.is_empty(), "slot {slot} rendered empty");
            // Slot 2's channels end on the first byte by design.
            if slot == 2 {
                continue;
            }
            let peak = pcm.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
            assert!(peak > 0.01, "slot {slot} peak {peak} (silent?)");
        }
    }

    #[test]
    fn sfx_slots_end_silent() {
        // $B6/$A2 silence the channel at parse time, so by the time the
        // 20 ms fade-out starts the output must already be quiet (a hanging
        // tone here would mean the end-of-channel silence was skipped).
        for slot in (1..TRACK_COUNT).filter(|&s| is_sfx_slot(s)) {
            let pcm = render_track(slot);
            let fade = SAMPLE_RATE as usize / 50;
            if pcm.len() < fade * 2 {
                continue;
            }
            // Middle of the fade zone: the channel was silenced ~1 frame
            // before the render stopped, so this region must be quiet; a
            // hanging tone would still be at ~50% amplitude here.
            let mid = &pcm[pcm.len() - fade / 2..pcm.len() - fade / 4];
            let peak = mid.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
            assert!(peak < 0.02, "slot {slot} fade-zone peak {peak} (hanging tone)");
        }
    }

    #[test]
    fn render_is_deterministic() {
        for slot in [4, 18, 24, 35] {
            assert_eq!(render_track(slot), render_track(slot), "slot {slot}");
        }
    }

    #[test]
    fn renders_terminate_within_cap() {
        for slot in 1..TRACK_COUNT {
            let pcm = render_track(slot);
            let secs = pcm.len() as f64 / SAMPLE_RATE as f64;
            assert!(secs <= MAX_SECONDS as f64 + 1.0, "slot {slot}: {secs}s");
        }
    }
}
