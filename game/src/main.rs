//! Boulder Dash (NES remake) — game flow + playable loop + 256x240 renderer.
//!
//! `main` is a thin runner over the [`flow`] state machine (title -> color
//! select -> password -> world map -> pre-cave card -> gameplay -> cave
//! complete -> map ... -> game over/continue -> ending, plus the attract
//! demo); see `flow/mod.rs` for the mapping to the ROM's 19-state dispatch.
//! The engine runs at exactly 60 ticks/s via an accumulator over
//! `get_frame_time()`; rendering happens once per window frame into a
//! 256x240 render target (nearest-neighbor, integer-scaled letterbox blit).
//!
//! Audio: all needed sound-driver slots (title/menu/map themes, world themes
//! plus hurry variants, cave-complete jingle, screen jingles, SFX) are
//! pre-rendered to WAV bytes by the `boulder_dash::audio` synth on worker
//! threads and uploaded with `load_sound_from_bytes` as they finish, so the
//! window appears instantly and music fades in once its track is ready.
//! Engine `SoundCue` events are mapped to the slots the NES ROM plays at the
//! corresponding call sites (see game/assets/audio_notes.md); per-slot
//! cooldowns keep bursty cues from retriggering the same sample every frame.
//!
//! Screenshot modes (CI/visual verification; audio fully disabled so headless
//! runs never hang on a missing audio device):
//!
//! - `BDSHOT=<cave>,<frames>,<output.png>[,<script>] cargo run -p boulder-dash`
//!   ticks cave `cave` `frames` times (optional scripted input holds like
//!   `R30,D20,G8`), then saves the window and exits. Also writes
//!   `<out>.rt.png` (raw 256x240 render target) and `<out>.rf.png` (Rockford
//!   head-sprite sheet, one row per suit color; vertically flipped by
//!   macroquad's texture export). Set BDDEBUG=1 for a per-10-tick state trace
//!   on stderr. BDSUIT=<0-15> picks the suit color table index.
//! - `BDSHOT=_,<frames>,<output.png> BDSHOT_FLOW=<screen> cargo run ...`
//!   renders a flow screen instead; <screen> is one of title, color,
//!   password, map, mapc (map with cleared towns), splash, precave,
//!   gameover, ending, demo.

use macroquad::audio::{self as mq, PlaySoundParams, Sound};
use macroquad::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};

use boulder_dash::audio::{render_wav_bytes, slots};
use boulder_dash::engine::{DeathCause, Input, SoundCue};
use boulder_dash::render::Renderer;

mod flow;
use flow::{Flow, Pad};

pub const SCREEN_W: f32 = 640.0;
pub const SCREEN_H: f32 = 416.0;
const TICK: f32 = 1.0 / 60.0;
const CAVE_COUNT: usize = 24;

/// Volume balance: music sits modestly under the effects.
const MUSIC_VOLUME: f32 = 0.35;
const SFX_VOLUME: f32 = 0.9;
const JINGLE_VOLUME: f32 = 0.7;

/// SFX slots the game can trigger, in render-priority order.
const SFX_SLOTS: [usize; 15] = [
    slots::SFX_SPAWN,       // 11: cave entry
    slots::SFX_STING,       // 1:  diamond collect (near-silent by design)
    slots::SFX_EXPLOSION,   // 2
    slots::SFX_FALL_START,  // 3
    slots::SFX_THUD,        // 4
    slots::SFX_DOOR_OPEN,   // 5
    slots::SFX_AMOEBA,      // 6: also the menu cursor blip (ROM reuses it)
    slots::SFX_DEATH,       // 7:  timeout / suicide
    slots::SFX_PUSH,        // 8
    slots::SFX_GAME_OVER_A, // 9
    slots::SFX_GAME_OVER_B, // 10
    slots::SFX_SCREEN_A,    // 12: password/game-over screen confirm
    slots::SFX_SCREEN_B,    // 17: password/game-over screen
    slots::SFX_CARD_PAUSE,  // 25: cave-card drone (music pause)
    slots::SFX_CARD_RESUME, // 26: cave-card drone (music resume)
];

/// Custom (non-ROM) sound id for the diamond-collect chime. The ROM's own
/// collect slot (1) is a near-silent music-cut sting by design; the remake
/// plays an audible sparkle instead.
const CHIME_SLOT: usize = 1000;

/// Map an engine sound cue to the sound-driver slot the NES ROM plays at the
/// corresponding call site (game/assets/audio_notes.md). `None` is
/// intentional silence: Dig's call site plays slot 0 (stop-all), MagicWall
/// and ExtraLife have no documented slot, and HurryUp switches the music
/// instead of playing an effect.
fn cue_slot(cue: SoundCue) -> Option<usize> {
    match cue {
        SoundCue::Dig => None,
        SoundCue::Push => Some(slots::SFX_PUSH),
        SoundCue::FallStart => Some(slots::SFX_FALL_START),
        SoundCue::Thud => Some(slots::SFX_THUD),
        SoundCue::Diamond => Some(CHIME_SLOT),
        SoundCue::Explosion => Some(slots::SFX_EXPLOSION),
        SoundCue::MagicWall => None,
        SoundCue::DoorOpen => Some(slots::SFX_DOOR_OPEN),
        SoundCue::Amoeba => Some(slots::SFX_AMOEBA),
        SoundCue::ExtraLife => None,
        SoundCue::HurryUp => None,
    }
}

/// Synthesize the diamond chime: three rising square-wave notes with an
/// exponential decay (A6 -> C#7 -> F#7), 16-bit mono WAV at 44.1 kHz.
fn chime_wav() -> Vec<u8> {
    const SR: u32 = 44100;
    let mut pcm: Vec<i16> = Vec::new();
    for freq in [1760.0f32, 2217.46, 2959.96] {
        let n = (SR as f32 * 0.05) as usize;
        for i in 0..n {
            let t = i as f32 / SR as f32;
            let sq = if (t * freq) % 1.0 < 0.5 { 1.0 } else { -1.0 };
            let env = (1.0 - i as f32 / n as f32).powf(1.5);
            pcm.push((sq * env * 0.45 * i16::MAX as f32) as i16);
        }
    }
    let data_len = (pcm.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SR.to_le_bytes());
    wav.extend_from_slice(&(SR * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}

/// Minimum engine ticks (60 Hz) before the same SFX slot may retrigger, so
/// a burst of identical cues over consecutive ticks does not restart the
/// sample every frame.
fn sfx_cooldown(slot: usize) -> u64 {
    match slot {
        CHIME_SLOT => 3,
        slots::SFX_STING => 3,
        slots::SFX_THUD => 4,
        slots::SFX_FALL_START => 5,
        slots::SFX_EXPLOSION => 6,
        slots::SFX_PUSH => 10,
        slots::SFX_AMOEBA => 24,
        _ => 12,
    }
}

/// Pre-rendered sound bank + music state. Slots are rendered to WAV bytes
/// on worker threads (the synth is offline and CPU-bound; see
/// `audio::render`), then uploaded with `load_sound_from_bytes` as they
/// arrive, so the game starts immediately and music fades in once ready.
/// On wasm there are no threads: the queue lives in `pending` and `pump`
/// renders a couple of slots inline per frame instead.
struct Audio {
    rx: Receiver<(usize, Vec<u8>)>,
    #[cfg(target_arch = "wasm32")]
    tx: mpsc::Sender<(usize, Vec<u8>)>,
    #[cfg(target_arch = "wasm32")]
    pending: VecDeque<usize>,
    sounds: HashMap<usize, Sound>,
    /// Music slot that should be looping (may not be uploaded yet).
    want_music: Option<usize>,
    /// Music slot actually playing right now.
    now_playing: Option<usize>,
    world: usize,
    hurry: bool,
    paused: bool,
    /// Master music on/off (M key). SFX are unaffected.
    music_enabled: bool,
    /// Last engine tick at which each SFX slot was triggered.
    last_sfx: HashMap<usize, u64>,
}

impl Audio {
    /// Spawn the render workers. The title theme renders first (it is the
    /// first thing the player hears), then the menu/map themes and world 1
    /// (the demo caves and the first world), then everything else.
    fn start() -> Audio {
        let mut order: Vec<usize> = vec![
            slots::MUSIC_UNK_18, // title/attract ($D8DE)
            slots::MUSIC_UNK_19, // menu/interstitial screens ($A788)
            slots::MUSIC_TITLE,  // 35: world-map hub ($EB52, sound $23)
            slots::WORLD_MUSIC[0],
            slots::WORLD_MUSIC_HURRY[0],
        ];
        order.extend(SFX_SLOTS);
        order.push(slots::JINGLE_CAVE_COMPLETE);
        for w in 1..slots::WORLD_MUSIC.len() {
            order.push(slots::WORLD_MUSIC[w]);
            order.push(slots::WORLD_MUSIC_HURRY[w]);
        }
        let (tx, rx) = mpsc::channel();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let queue = Arc::new(Mutex::new(VecDeque::from(order)));
            let workers = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2)
                .clamp(2, 4);
            for _ in 0..workers {
                let queue = Arc::clone(&queue);
                let tx = tx.clone();
                std::thread::spawn(move || loop {
                    let slot = queue.lock().unwrap().pop_front();
                    match slot {
                        Some(slot) => {
                            let wav = render_wav_bytes(slot);
                            if tx.send((slot, wav)).is_err() {
                                return;
                            }
                        }
                        None => return,
                    }
                });
            }
        }
        Audio {
            rx,
            #[cfg(target_arch = "wasm32")]
            tx,
            #[cfg(target_arch = "wasm32")]
            pending: VecDeque::from(order),
            sounds: HashMap::new(),
            want_music: None,
            now_playing: None,
            world: 0,
            hurry: false,
            paused: false,
            music_enabled: false,
            last_sfx: HashMap::new(),
        }
    }

    /// Upload any sounds that finished rendering; start pending music.
    async fn pump(&mut self) {
        // wasm has no worker threads: render up to two queued slots inline
        // per frame (keeps startup responsive; music fades in within seconds).
        #[cfg(target_arch = "wasm32")]
        for _ in 0..2 {
            let Some(slot) = self.pending.pop_front() else { break };
            let wav = render_wav_bytes(slot);
            if self.tx.send((slot, wav)).is_err() {
                break;
            }
        }
        while let Ok((slot, wav)) = self.rx.try_recv() {
            if let Ok(sound) = mq::load_sound_from_bytes(&wav).await {
                self.sounds.insert(slot, sound);
                if self.want_music == Some(slot) && self.now_playing != Some(slot) {
                    self.start_music(slot);
                }
            }
        }
    }

    fn start_music(&mut self, slot: usize) {
        if !self.music_enabled {
            return;
        }
        if let Some(sound) = self.sounds.get(&slot) {
            mq::play_sound(
                sound,
                PlaySoundParams {
                    looped: true,
                    volume: if self.paused { 0.0 } else { MUSIC_VOLUME },
                },
            );
            self.now_playing = Some(slot);
        }
    }

    /// Master music switch (M key): stops the loop when off, resumes the
    /// wanted track when back on. `want_music` is preserved either way.
    fn toggle_music(&mut self) {
        self.music_enabled = !self.music_enabled;
        if self.music_enabled {
            if let Some(slot) = self.want_music {
                self.now_playing = None;
                self.start_music(slot);
            }
        } else if let Some(cur) = self.now_playing.take() {
            if let Some(sound) = self.sounds.get(&cur) {
                mq::stop_sound(sound);
            }
        }
    }

    /// Register a custom (non-ROM-slot) sound under a synthetic slot id.
    fn add_sound(&mut self, slot: usize, sound: Sound) {
        self.sounds.insert(slot, sound);
    }

    /// Request `slot` as the looping background music (`None` = silence).
    /// Stops whatever is playing; if the wanted track is not uploaded yet
    /// it starts automatically in `pump` once it arrives.
    fn set_music(&mut self, slot: Option<usize>) {
        if self.want_music == slot && self.now_playing == slot {
            return;
        }
        self.want_music = slot;
        if self.now_playing != slot {
            if let Some(cur) = self.now_playing.take() {
                if let Some(sound) = self.sounds.get(&cur) {
                    mq::stop_sound(sound);
                }
            }
            if let Some(slot) = slot {
                self.start_music(slot);
            }
        }
    }

    fn stop_jingle(&mut self) {
        if let Some(sound) = self.sounds.get(&slots::JINGLE_CAVE_COMPLETE) {
            mq::stop_sound(sound);
        }
    }

    /// Cave entry: reset to the world's normal-tempo theme + spawn SFX.
    fn enter_cave(&mut self, cave_idx: usize, now: u64) {
        self.world = (cave_idx / 4).min(slots::WORLD_MUSIC.len() - 1);
        self.hurry = false;
        self.stop_jingle();
        self.set_music(Some(slots::WORLD_MUSIC[self.world]));
        self.play_sfx(slots::SFX_SPAWN, now);
    }

    fn on_cue(&mut self, cue: SoundCue, now: u64) {
        if cue == SoundCue::HurryUp {
            // 30 time units left: switch to the world's hurry-up variant
            // (ROM table $D060).
            self.hurry = true;
            self.set_music(Some(slots::WORLD_MUSIC_HURRY[self.world]));
            return;
        }
        if let Some(slot) = cue_slot(cue) {
            self.play_sfx(slot, now);
        }
    }

    fn on_death(&mut self, cause: DeathCause, now: u64) {
        // Death sting: the music stops when Rockford dies (restarted on
        // respawn); timeout/suicide have their own documented SFX.
        self.set_music(None);
        if matches!(cause, DeathCause::Timeout | DeathCause::Suicide) {
            self.play_sfx(slots::SFX_DEATH, now);
        }
    }

    fn respawned(&mut self) {
        self.hurry = false;
        self.set_music(Some(slots::WORLD_MUSIC[self.world]));
    }

    fn cave_complete(&mut self) {
        self.set_music(None);
        if let Some(sound) = self.sounds.get(&slots::JINGLE_CAVE_COMPLETE) {
            mq::play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: JINGLE_VOLUME,
                },
            );
        }
    }

    fn game_over(&mut self, now: u64) {
        self.set_music(None);
        self.play_sfx(slots::SFX_GAME_OVER_A, now);
    }

    /// Start pressed in gameplay: mute the looping music (macroquad has no
    /// per-sound pause; volume 0 keeps the position and resumes instantly).
    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if let Some(cur) = self.now_playing {
            if let Some(sound) = self.sounds.get(&cur) {
                mq::set_sound_volume(sound, if paused { 0.0 } else { MUSIC_VOLUME });
            }
        }
    }

    fn play_sfx(&mut self, slot: usize, now: u64) {
        if let Some(&last) = self.last_sfx.get(&slot) {
            if now.saturating_sub(last) < sfx_cooldown(slot) {
                return;
            }
        }
        self.last_sfx.insert(slot, now);
        if let Some(sound) = self.sounds.get(&slot) {
            mq::play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: SFX_VOLUME,
                },
            );
        }
    }
}

/// Keyboard -> NES pad: arrows/WASD = d-pad, X = A, Z = B, Enter = Start,
/// Shift = Select.
fn read_pad() -> Pad {
    Pad {
        up: is_key_down(KeyCode::Up) || is_key_down(KeyCode::W),
        down: is_key_down(KeyCode::Down) || is_key_down(KeyCode::S),
        left: is_key_down(KeyCode::Left) || is_key_down(KeyCode::A),
        right: is_key_down(KeyCode::Right) || is_key_down(KeyCode::D),
        a: is_key_down(KeyCode::X),
        b: is_key_down(KeyCode::Z),
        start: is_key_down(KeyCode::Enter),
        select: is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift),
    }
}

/// Scripted BDSHOT input -> pad. `S` (suicide) maps to A+B held.
fn input_to_pad(input: Input) -> Pad {
    Pad {
        up: input.up,
        down: input.down,
        left: input.left,
        right: input.right,
        a: input.grab || input.suicide,
        b: input.suicide,
        ..Default::default()
    }
}

/// Parsed BDSHOT: cave index, frame count, output path, scripted input holds.
type ShotSpec = (usize, u32, String, Vec<(Input, u32)>);

/// Parse `BDSHOT=<cave>,<frames>,<output.png>[,<script>]`.
/// The optional script is comma-separated `X<n>` holds: `R`/`L`/`U`/`D`
/// (direction), `G` (grab), `S` (suicide), `N` (no input) for n ticks,
/// e.g. `R30,G8,N20`.
fn parse_shot() -> Option<ShotSpec> {
    let s = std::env::var("BDSHOT").ok()?;
    let mut it = s.split(',');
    // `_` = no cave (used with BDSHOT_FLOW, which renders a flow screen).
    let cave = match it.next()? {
        "_" => 0,
        c => c.parse().ok()?,
    };
    let frames = it.next()?.parse().ok()?;
    let out = it.next()?.to_owned();
    let mut script = Vec::new();
    for tok in it {
        if tok.len() < 2 {
            continue;
        }
        // Split into leading letter codes ("R", "GL") and the digit count.
        let digit_at = tok.find(|c: char| c.is_ascii_digit()).unwrap_or(tok.len());
        let (code, digits) = tok.split_at(digit_at);
        let n: u32 = match digits.parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let mut input = Input::NONE;
        // Combos allowed: "GL" = grab+left, "GU"/"GD"/"GR" similarly.
        for c in code.chars() {
            match c {
                'R' => input.right = true,
                'L' => input.left = true,
                'U' => input.up = true,
                'D' => input.down = true,
                'G' => input.grab = true,
                'S' => input.suicide = true,
                'N' => {}
                _ => {}
            }
        }
        script.push((input, n));
    }
    Some((cave, frames, out, script))
}

/// Scripted input for tick `t`: sum script durations to find the active hold.
fn scripted_input(script: &[(Input, u32)], t: u32) -> Input {
    let mut acc = 0;
    for &(input, n) in script {
        if t < acc + n {
            return input;
        }
        acc += n;
    }
    Input::NONE
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Boulder Dash".to_owned(),
        window_width: 1280,
        window_height: 832,
        window_resizable: true,
        // Crisp integer scaling on Retina: request the real (2x) framebuffer
        // so `screen_width()` returns physical pixels and the letterbox math
        // below lands on whole pixels instead of a blurry OS upscale.
        high_dpi: true,
        ..Default::default()
    }
}

/// Blit the 512x480 render target to the window, aspect-preserving fit.
/// Size comes from miniquad's real drawable (physical px / dpi = logical,
/// matching the default projection) — immune to macroquad's stale
/// `screen_width()` on Retina startup.
fn blit(rt: &RenderTarget) {
    set_default_camera();
    clear_background(BLACK);
    let dpi = macroquad::miniquad::window::dpi_scale();
    let (pw, ph) = macroquad::miniquad::window::screen_size();
    let (sw, sh) = (pw / dpi, ph / dpi);
    let scale = (sw / SCREEN_W).min(sh / SCREEN_H).max(0.1);
    let (dw, dh) = (SCREEN_W * scale, SCREEN_H * scale);
    draw_texture_ex(
        &rt.texture,
        (sw - dw) / 2.0,
        (sh - dh) / 2.0,
        WHITE,
        DrawTextureParams {
            dest_size: Some(vec2(dw, dh)),
            // Render-target textures are stored bottom-up; flip on blit.
            flip_y: true,
            ..Default::default()
        },
    );
}

/// Draw the current flow state into the render target, then to the window.
fn render_frame(
    rt: &RenderTarget,
    game_cam: &Camera2D,
    screens_cam: &Camera2D,
    flow: &mut Flow,
    renderer: &Renderer,
) {
    // Gameplay states use the full 512x480 frame; flow screens draw in
    // 256x240 logical coordinates through a x2-zoom camera.
    let cam = match flow.state_name() {
        "playing" | "demo" | "complete" => game_cam,
        _ => screens_cam,
    };
    set_camera(cam);
    clear_background(BLACK);
    flow.render(renderer);
    blit(rt);
}

#[macroquad::main(window_conf)]
async fn main() {
    let rt = render_target(SCREEN_W as u32, SCREEN_H as u32);
    {
        let (pw, ph) = macroquad::miniquad::window::screen_size();
        eprintln!(
            "boulder-dash: framebuffer {pw}x{ph}, dpi_scale {}, screen {}x{}",
            macroquad::miniquad::window::dpi_scale(),
            screen_width(),
            screen_height()
        );
    }
    rt.texture.set_filter(FilterMode::Nearest);

    let mut game_cam = Camera2D::from_display_rect(Rect::new(0.0, 0.0, SCREEN_W, SCREEN_H));
    game_cam.render_target = Some(rt.clone());
    // 256x240 flow screens, aspect-preserved and zoomed to fill the frame
    // height (visible logical area computed from the frame aspect).
    let lw = 240.0 * SCREEN_W / SCREEN_H;
    let mut screens_cam =
        Camera2D::from_display_rect(Rect::new(-(lw - 256.0) / 2.0, 0.0, lw, 240.0));
    screens_cam.render_target = Some(rt.clone());

    boulder_dash::render::hud::init_font();
    let renderer = Renderer::new();

    // Headless flow-screen shot: BDSHOT_FLOW=<screen> with BDSHOT supplying
    // the frame count and output path (cave/script fields ignored).
    if let Ok(name) = std::env::var("BDSHOT_FLOW") {
        let (_, frames, out, _) =
            parse_shot().unwrap_or((0, 30, "/tmp/flow.png".to_owned(), Vec::new()));
        let mut flow = Flow::new();
        flow.force_screen(&name);
        let mut audio: Option<Audio> = None;
        for _ in 0..frames.max(1) {
            flow.update(Pad::default(), &mut audio);
        }
        render_frame(&rt, &game_cam, &screens_cam, &mut flow, &renderer);
        get_screen_data().export_png(&out);
        rt.texture.get_texture_data().export_png(&(out.clone() + ".rt.png"));
        eprintln!("BDSHOT_FLOW: screen {name} ({}), {frames} frames -> {out}", flow.state_name());
        std::process::exit(0);
    }

    // Headless gameplay shot (no audio: no render threads, no device access).
    if let Some((cave_idx, frames, out, script)) = parse_shot() {
        let mut audio: Option<Audio> = None;
        let mut flow = Flow::new();
        flow.debug_play(cave_idx.min(CAVE_COUNT - 1), 1);
        for t in 0..frames {
            let input = scripted_input(&script, t);
            if std::env::var("BDDEBUG").is_ok() && t % 10 == 0 {
                if let Some(cave) = flow.active_cave() {
                    let p = cave.rockford_pos();
                    eprintln!(
                        "t={t:3} in.right={} rockford=({},{}) alive={} status={:?} lives={}",
                        input.right,
                        p % 40,
                        p / 40,
                        cave.rockford_alive(),
                        cave.status(),
                        cave.lives()
                    );
                }
            }
            flow.update(input_to_pad(input), &mut audio);
            render_frame(&rt, &game_cam, &screens_cam, &mut flow, &renderer);
            next_frame().await;
        }
        // Render once more and capture the backbuffer before presenting.
        render_frame(&rt, &game_cam, &screens_cam, &mut flow, &renderer);
        get_screen_data().export_png(&out);
        // Also dump the raw 256x240 render target for pixel-precise checks.
        rt.texture.get_texture_data().export_png(&(out.clone() + ".rt.png"));
        // And the Rockford head-sprite sheet for art inspection.
        renderer
            .rockford_art()
            .texture()
            .get_texture_data()
            .export_png(&(out.clone() + ".rf.png"));
        eprintln!("BDSHOT: cave {cave_idx}, {frames} frames -> {out}");
        std::process::exit(0);
    }

    let mut audio = Some(Audio::start());
    if let Some(a) = &mut audio {
        if let Ok(s) = mq::load_sound_from_bytes(&chime_wav()).await {
            a.add_sound(CHIME_SLOT, s);
        }
    }
    let mut fullscreen = false;
    let mut flow = Flow::new();
    let mut acc = 0.0f32;

    loop {
        let pad = read_pad();

        // M = music on/off (SFX keep playing).
        if is_key_pressed(KeyCode::M) {
            if let Some(a) = &mut audio {
                a.toggle_music();
            }
        }

        // F = fullscreen toggle.
        if is_key_pressed(KeyCode::F) {
            fullscreen = !fullscreen;
            set_fullscreen(fullscreen);
        }

        // Debug shortcuts during gameplay: R = restart cave,
        // [ / ] = prev/next cave, 1-4 = difficulty level.
        if flow.state_name() == "playing" {
            let (cave, level) = (flow.cur_cave_idx(), flow.cur_level());
            if is_key_pressed(KeyCode::R) {
                flow.debug_play(cave, level);
            } else if is_key_pressed(KeyCode::LeftBracket) {
                flow.debug_play((cave + CAVE_COUNT - 1) % CAVE_COUNT, level);
            } else if is_key_pressed(KeyCode::RightBracket) {
                flow.debug_play((cave + 1) % CAVE_COUNT, level);
            } else {
                for (key, lvl) in [
                    (KeyCode::Key1, 1u8),
                    (KeyCode::Key2, 2),
                    (KeyCode::Key3, 3),
                    (KeyCode::Key4, 4),
                ] {
                    if is_key_pressed(key) {
                        flow.debug_play(cave, lvl);
                    }
                }
            }
        }

        // Upload sounds that finished rendering since the last frame.
        if let Some(a) = &mut audio {
            a.pump().await;
        }

        // Fixed 60 Hz flow/engine stepping.
        let dt = get_frame_time().min(0.1);
        acc += dt;
        let mut stepped = 0;
        while acc >= TICK && stepped < 4 {
            flow.update(pad, &mut audio);
            acc -= TICK;
            stepped += 1;
        }

        render_frame(&rt, &game_cam, &screens_cam, &mut flow, &renderer);
        next_frame().await;
    }
}
