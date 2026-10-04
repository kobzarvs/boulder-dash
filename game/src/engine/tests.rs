//! Engine unit tests. Synthetic fields are built from ASCII art; ascii cell
//! (x, y) maps to field cell (x + 1, y + 1) via `ascii_idx`.
//!
//! Timing reference (frame = one `tick`): fall scans run on phases 1-4
//! (row bands 16-20 / 11-15 / 6-10 / 1-5), cleanup on phases 5-7, so objects
//! in rows 1-5 first act at frame 4, then every 8 frames.

use super::ascii::{ascii_cave, ascii_idx, ascii_params};
use super::*;

fn frames(cave: &mut Cave, n: usize, input: Input) {
    for _ in 0..n {
        cave.tick(input);
    }
}

fn dir(d: Direction) -> Input {
    let mut input = Input::NONE;
    match d {
        Direction::Up => input.up = true,
        Direction::Down => input.down = true,
        Direction::Left => input.left = true,
        Direction::Right => input.right = true,
    }
    input
}

fn grab(d: Direction) -> Input {
    let mut input = dir(d);
    input.grab = true;
    input
}

#[test]
fn boulder_falls_and_comes_to_rest() {
    let mut cave = ascii_cave(&["o"], ascii_params());
    assert_eq!(cave.cell_at_idx(ascii_idx(0, 0)).obj, Obj::Boulder);
    assert!(!cave.cell_at_idx(ascii_idx(0, 0)).falling());

    // Row 1 is scanned on phase 4: first fall at frame 4, one cell per cycle.
    let mut fall_start = false;
    for _ in 0..4 {
        fall_start |= cave
            .tick(Input::NONE)
            .contains(&Event::Sound(SoundCue::FallStart));
    }
    assert!(fall_start, "no fall-start sound");
    assert_eq!(cave.cell_at_idx(ascii_idx(0, 1)).obj, Obj::Boulder);
    assert!(cave.cell_at_idx(ascii_idx(0, 1)).falling());
    // The just-vacated cell reads as Vacated until the cleanup phase...
    assert_eq!(cave.cell_at_idx(ascii_idx(0, 0)).obj, Obj::Vacated);
    cave.tick(Input::NONE); // frame 5: cleanup rows 1-7
    assert_eq!(cave.cell_at_idx(ascii_idx(0, 0)).obj, Obj::Space);

    // One cell per 8 frames: row 2 at frame 4, row 20 at frame 148, lands 156.
    frames(&mut cave, 200, Input::NONE);
    let rest = ascii_idx(0, 19);
    assert_eq!(cave.cell_at_idx(rest).obj, Obj::Boulder);
    assert!(!cave.cell_at_idx(rest).falling());
}

#[test]
fn boulder_rolls_off_left_first() {
    let mut cave = ascii_cave(&[" o ", " o+", "+++"], ascii_params());

    // Frame 4: the top boulder rolls LEFT off the boulder below (deterministic
    // left-first; side and down-side empty, up-side not a boulder/diamond).
    frames(&mut cave, 4, Input::NONE);
    assert_eq!(cave.cell_at_idx(ascii_idx(0, 0)).obj, Obj::Boulder);
    assert!(cave.cell_at_idx(ascii_idx(0, 0)).falling());
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 0)).obj, Obj::Vacated);
    // Bottom boulder is boxed in by brick and stays put.
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 1)).obj, Obj::Boulder);
    assert!(!cave.cell_at_idx(ascii_idx(1, 1)).falling());

    // The rolled boulder falls down the flank and lands on the brick floor.
    frames(&mut cave, 30, Input::NONE);
    assert_eq!(cave.cell_at_idx(ascii_idx(0, 1)).obj, Obj::Boulder);
    assert!(!cave.cell_at_idx(ascii_idx(0, 1)).falling());
}

#[test]
fn rockford_walks_one_cell_per_8_frames() {
    let mut cave = ascii_cave(&["r   "], ascii_params());
    cave.tick(dir(Direction::Right)); // frame 1: move
    assert_eq!(cave.rockford_pos(), ascii_idx(1, 0));
    frames(&mut cave, 7, dir(Direction::Right)); // frames 2-8: walking
    assert_eq!(cave.rockford_pos(), ascii_idx(1, 0));
    cave.tick(dir(Direction::Right)); // frame 9: next cell
    assert_eq!(cave.rockford_pos(), ascii_idx(2, 0));
}

#[test]
fn diamond_collect_increments_quota_counter() {
    let mut cave = ascii_cave(&["r*"], ascii_params());
    let ev = cave.tick(dir(Direction::Right));
    assert_eq!(cave.diamonds_collected(), 1);
    assert_eq!(cave.score(), 10);
    assert!(ev.contains(&Event::DiamondCollected { count: 1 }));
    assert_eq!(cave.rockford_pos(), ascii_idx(1, 0));
    assert!(!cave.door_open());
}

#[test]
fn door_opens_at_quota_and_completion_pays_time_bonus() {
    let mut params = ascii_params();
    params.diamonds_needed = [1, 1, 1, 1];
    let mut cave = ascii_cave(&["r*x"], params);
    let door = ascii_idx(2, 0);

    let ev = cave.tick(dir(Direction::Right)); // frame 1: collect
    assert_eq!(cave.diamonds_collected(), 1);
    assert!(cave.door_open());
    // The door cell keeps id Door; opening is the global flag ($90).
    assert_eq!(cave.cell_at_idx(door).obj, Obj::Door);
    assert!(ev.iter().any(|e| matches!(e, Event::DoorOpened { pos: Some(p) } if *p == door)));

    // Frame 1: collect + quota met. Frame 9 (after the 8-frame walk cadence):
    // enter the open door.
    let mut complete = None;
    for _ in 0..9 {
        let ev = cave.tick(dir(Direction::Right));
        if let Some(e) = ev.iter().find(|e| matches!(e, Event::CaveComplete { .. })) {
            complete = Some(*e);
        }
    }
    // Time bonus: +1 per remaining unit; all 100 units remain at frame 9.
    assert_eq!(complete, Some(Event::CaveComplete { time_bonus: 100 }));
    assert_eq!(cave.status(), CaveStatus::Complete);
    assert_eq!(cave.score(), 10 + 100);
}

#[test]
fn falling_boulder_kills_rockford_and_respawn_spends_reserve_life() {
    let mut cave = ascii_cave(&["o", " ", "r"], ascii_params());
    frames(&mut cave, 11, Input::NONE);
    let ev = cave.tick(Input::NONE); // frame 12: falling boulder lands on Rockford
    assert!(ev.contains(&Event::RockfordDied {
        cause: DeathCause::Crushed
    }));
    assert!(!cave.rockford_alive());
    assert!(cave.lives() == 3, "life is spent after the 80-frame arc, not at kill");

    frames(&mut cave, 80, Input::NONE); // death arc 80 frames -> reload
    assert!(cave.rockford_alive());
    assert_eq!(cave.lives(), 2);
    assert_eq!(cave.rockford_pos(), ascii_idx(0, 2));
    assert_eq!(cave.status(), CaveStatus::Playing);
}

#[test]
fn boulder_above_firefly_explodes_it_to_space_and_scores() {
    // A boulder/diamond in ANY fall state directly above the enemy triggers
    // the explosion when the enemy is scanned.
    let mut cave = ascii_cave(&[" o ", "   ", "   ", "+f+", "+++"], ascii_params());

    let mut boom = None;
    for _ in 0..16 {
        let ev = cave.tick(Input::NONE);
        if let Some(e) = ev.iter().find(|e| matches!(e, Event::Explosion { .. })) {
            boom = Some(*e);
        }
    }
    let center = ascii_idx(1, 2); // the firefly moved up one cell first
    assert_eq!(boom, Some(Event::Explosion { center, to_diamond: false }));
    assert_eq!(cave.score(), 200, "firefly blast pays 200 in cave group 0");
    assert_eq!(cave.cell_at_idx(center).obj, Obj::ExplosionRemnant);
    assert_eq!(cave.count_obj(Obj::Boulder), 0);
    assert_eq!(cave.count_obj(Obj::Firefly), 0);

    // Remnants linger 16 frames, then clear (via Vacated) to space.
    frames(&mut cave, 60, Input::NONE);
    for y in 1..=3 {
        for x in 0..=2 {
            assert_eq!(cave.cell_at_idx(ascii_idx(x, y)).obj, Obj::Space);
        }
    }
    // The brick floor outside the 3x3 survives.
    assert_eq!(cave.cell_at_idx(ascii_idx(0, 4)).obj, Obj::Brick);
}

#[test]
fn butterfly_on_amoeba_contact_explodes_into_nine_diamonds() {
    // Butterfly probes right first (CW) from its initial up heading: the
    // amoeba to its right triggers the blast immediately.
    let mut cave = ascii_cave(&["   ", "   ", "  qa", "   "], ascii_params());

    let mut boom = None;
    for _ in 0..8 {
        let ev = cave.tick(Input::NONE);
        if let Some(e) = ev.iter().find(|e| matches!(e, Event::Explosion { .. })) {
            boom = Some(*e);
        }
    }
    let center = ascii_idx(2, 2);
    assert_eq!(boom, Some(Event::Explosion { center, to_diamond: true }));
    assert_eq!(cave.cell_at_idx(center).obj, Obj::PendingDiamond);
    assert_eq!(cave.count_obj(Obj::PendingDiamond), 9);

    // Pending diamonds ripen ~16 frames later via the rotating slot timers.
    // (They are falling subjects once ripe — count, don't pin positions.)
    frames(&mut cave, 30, Input::NONE);
    assert_eq!(cave.count_obj(Obj::PendingDiamond), 0);
    assert_eq!(cave.count_obj(Obj::Diamond), 9);
    assert_eq!(cave.count_obj(Obj::Butterfly), 0);
    assert_eq!(cave.count_obj(Obj::Amoeba), 0);
}

#[test]
fn magic_wall_converts_first_falling_boulder_and_expires_after_4096_frames() {
    let mut cave = ascii_cave(&["+o+", "+ +", "+~+", "+ +"], ascii_params());
    frames(&mut cave, 11, Input::NONE);

    let ev = cave.tick(Input::NONE); // frame 12: boulder falls onto the wall
    assert!(ev.contains(&Event::MagicWallActivated));
    // Unlike the C64, the FIRST object already converts: falling diamond two
    // rows below the boulder's cell, vacated marker behind.
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 3)).obj, Obj::Diamond);
    assert!(cave.cell_at_idx(ascii_idx(1, 3)).falling());
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 1)).obj, Obj::Vacated);
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 2)).obj, Obj::MagicWall);

    // The active window is a hardcoded 4096 frames; the wall then stays a
    // MagicWall object but is inert.
    let mut expired = false;
    for _ in 0..4200 {
        expired |= cave.tick(Input::NONE).contains(&Event::MagicWallExpired);
    }
    assert!(expired, "magic wall never expired");
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 2)).obj, Obj::MagicWall);
}

#[test]
fn resting_boulder_on_dormant_magic_wall_does_not_activate() {
    let mut cave = ascii_cave(&["+o+", "+~+"], ascii_params());
    let mut activated = false;
    for _ in 0..50 {
        activated |= cave.tick(Input::NONE).contains(&Event::MagicWallActivated);
    }
    assert!(!activated);
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 0)).obj, Obj::Boulder);
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 1)).obj, Obj::MagicWall);
}

#[test]
fn amoeba_enclosed_becomes_diamonds() {
    let mut cave = ascii_cave(&["+++", "+a+", "+++"], ascii_params());
    // A full 4-chunk pass (4 x 8 frames) finds no empty cell -> enclosed;
    // the cleanup scan then converts amoeba to diamonds.
    let mut converted = false;
    for _ in 0..48 {
        converted |= cave
            .tick(Input::NONE)
            .contains(&Event::AmoebaConverted { to_diamonds: true });
    }
    assert!(converted);
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 1)).obj, Obj::Diamond);
    assert_eq!(cave.count_obj(Obj::Amoeba), 0);
}

#[test]
fn amoeba_growth_is_deterministic_and_accelerates() {
    let mut cave = ascii_cave(&["   ", " a ", "   "], ascii_params());
    assert_eq!(cave.amoeba_probe_interval(), 255);

    let mut intervals = Vec::new();
    let mut growth_frames = Vec::new();
    for _ in 0..600 {
        let ev = cave.tick(Input::NONE);
        if ev.iter().any(|e| matches!(e, Event::AmoebaGrew { .. })) {
            intervals.push(cave.amoeba_probe_interval());
            growth_frames.push(cave.frame());
            if intervals.len() == 3 {
                break;
            }
        }
    }
    // Interval starts at 255 and shrinks by 4 per growth ($BA/$BD).
    assert_eq!(intervals, [251, 247, 243]);
    assert_eq!(growth_frames.len(), 3);
    assert_eq!(cave.count_obj(Obj::Amoeba), 4);

    // Identical setup evolves identically (there is no RNG to seed).
    let mut twin = ascii_cave(&["   ", " a ", "   "], ascii_params());
    frames(&mut twin, cave.frame() as usize, Input::NONE);
    assert_eq!(cave.hash(), twin.hash());
}

#[test]
fn amoeba_overgrowth_does_not_turn_into_boulders() {
    // 380 pre-placed amoeba cells (above the C64's 200 threshold), arranged as
    // a checkerboard so every probe finds space: the NES ROM has no
    // overgrowth rule, so the amoeba simply keeps growing.
    let mut rows = Vec::new();
    for y in 0..20 {
        let row: String = (0..38)
            .map(|x| if (x + y) % 2 == 0 { 'a' } else { ' ' })
            .collect();
        rows.push(row);
    }
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let mut cave = ascii_cave(&rows, ascii_params());
    assert_eq!(cave.count_obj(Obj::Amoeba), 380);

    let mut grew = false;
    for _ in 0..500 {
        let ev = cave.tick(Input::NONE);
        assert!(
            !ev.iter().any(|e| matches!(e, Event::AmoebaConverted { .. })),
            "growable amoeba must not convert"
        );
        grew |= ev.iter().any(|e| matches!(e, Event::AmoebaGrew { .. }));
    }
    assert!(grew, "amoeba never grew");
    assert_eq!(cave.count_obj(Obj::Boulder), 0);
    assert!(cave.count_obj(Obj::Amoeba) > 380);
}

#[test]
fn push_executes_after_24_frame_hold() {
    let mut cave = ascii_cave(&["ro  ", "++++"], ascii_params());
    cave.tick(dir(Direction::Right)); // frame 1: enter push state
    frames(&mut cave, 23, dir(Direction::Right)); // frames 2-24: holding
    assert_eq!(cave.rockford_pos(), ascii_idx(0, 0));
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 0)).obj, Obj::Boulder);

    cave.tick(dir(Direction::Right)); // frame 25: timer hits 0 -> execute
    assert_eq!(cave.cell_at_idx(ascii_idx(2, 0)).obj, Obj::Boulder);
    assert_eq!(cave.rockford_pos(), ascii_idx(1, 0));
}

#[test]
fn push_aborts_when_direction_released() {
    let mut cave = ascii_cave(&["ro  ", "++++"], ascii_params());
    frames(&mut cave, 10, dir(Direction::Right)); // frames 1-10: hold part-way
    cave.tick(Input::NONE); // frame 11: released -> abort
    frames(&mut cave, 20, dir(Direction::Right)); // re-enter, hold 20 < 24
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 0)).obj, Obj::Boulder);
    assert_eq!(cave.rockford_pos(), ascii_idx(0, 0));
    frames(&mut cave, 6, dir(Direction::Right)); // pass 24 held frames
    assert_eq!(cave.cell_at_idx(ascii_idx(2, 0)).obj, Obj::Boulder);
    assert_eq!(cave.rockford_pos(), ascii_idx(1, 0));
}

#[test]
fn push_never_happens_when_destination_blocked() {
    let mut cave = ascii_cave(&["ro+", "++++"], ascii_params());
    frames(&mut cave, 60, dir(Direction::Right));
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 0)).obj, Obj::Boulder);
    assert_eq!(cave.rockford_pos(), ascii_idx(0, 0));
}

#[test]
fn snap_push_with_button_is_instant_and_does_not_move_rockford() {
    let mut cave = ascii_cave(&["ro  ", "++++"], ascii_params());
    let ev = cave.tick(grab(Direction::Right));
    assert_eq!(cave.cell_at_idx(ascii_idx(2, 0)).obj, Obj::Boulder);
    assert_eq!(cave.rockford_pos(), ascii_idx(0, 0));
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 0)).obj, Obj::Vacated);
    assert!(
        ev.contains(&Event::BoulderPushed { dir: Direction::Right }),
        "snap push must report BoulderPushed for the render layer"
    );
}

#[test]
fn grab_digs_and_collects_without_moving() {
    let mut cave = ascii_cave(&["r*"], ascii_params());
    let ev = cave.tick(grab(Direction::Right));
    assert_eq!(cave.diamonds_collected(), 1);
    assert!(ev.contains(&Event::DiamondCollected { count: 1 }));
    assert_eq!(cave.rockford_pos(), ascii_idx(0, 0));

    let mut cave = ascii_cave(&["r."], ascii_params());
    cave.tick(grab(Direction::Right));
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 0)).obj, Obj::Space);
    assert_eq!(cave.rockford_pos(), ascii_idx(0, 0));
}

#[test]
fn falling_diamond_kills_rockford_too() {
    // No catching falling diamonds on the NES: any falling object landing on
    // Rockford is fatal ($C4EA bit0 gate).
    let mut cave = ascii_cave(&["*", " ", "r"], ascii_params());
    let mut died = false;
    for _ in 0..16 {
        died |= cave
            .tick(Input::NONE)
            .contains(&Event::RockfordDied {
                cause: DeathCause::Crushed,
            });
    }
    assert!(died);
    assert!(!cave.rockford_alive());
}

#[test]
fn suicide_combo_kills_after_64_frames() {
    let mut cave = ascii_cave(&["r"], ascii_params());
    let mut input = Input::NONE;
    input.suicide = true;
    let mut died_at = None;
    for _ in 0..80 {
        let ev = cave.tick(input);
        if died_at.is_none()
            && ev.contains(&Event::RockfordDied {
                cause: DeathCause::Suicide,
            })
        {
            died_at = Some(cave.frame());
        }
    }
    assert_eq!(died_at, Some(64));
    frames(&mut cave, 90, Input::NONE);
    assert!(cave.rockford_alive());
    assert_eq!(cave.lives(), 2);
}

#[test]
fn cave_timer_ticks_once_per_64_frames_and_timeout_kills() {
    let mut params = ascii_params();
    params.cave_time = [5, 5, 5, 5];
    let mut cave = ascii_cave(&["r"], params);

    frames(&mut cave, 63, Input::NONE);
    assert_eq!(cave.time_units_remaining(), 5);
    cave.tick(Input::NONE); // frame 64
    assert_eq!(cave.time_units_remaining(), 4);
    frames(&mut cave, 64, Input::NONE); // frame 128
    assert_eq!(cave.time_units_remaining(), 3);

    let mut died = false;
    for _ in 0..200 {
        died |= cave
            .tick(Input::NONE)
            .contains(&Event::RockfordDied {
                cause: DeathCause::Timeout,
            });
    }
    assert!(died, "timeout never killed Rockford");
    frames(&mut cave, 90, Input::NONE);
    assert!(cave.rockford_alive());
    assert_eq!(cave.lives(), 2);
}

#[test]
fn enemy_adjacency_kills_rockford_on_the_8th_frame_check() {
    let mut cave = ascii_cave(&["rf"], ascii_params());
    let mut died = false;
    for _ in 0..8 {
        died |= cave
            .tick(Input::NONE)
            .contains(&Event::RockfordDied {
                cause: DeathCause::Explosion,
            });
    }
    assert!(died, "adjacent firefly did not kill Rockford");
    // The firefly itself is caught in the blast centered on Rockford.
    assert_eq!(cave.count_obj(Obj::Firefly), 0);
}

#[test]
fn explosion_spares_steel_and_door() {
    let mut cave = ascii_cave(&["#o  ", " fx ", "    "], ascii_params());
    // Frame 4: the firefly sees the boulder above and explodes.
    let mut boom = false;
    for _ in 0..8 {
        boom |= cave
            .tick(Input::NONE)
            .iter()
            .any(|e| matches!(e, Event::Explosion { .. }));
    }
    assert!(boom);
    // Steel at (0,0) and the door at (2,1) are inside the 3x3 and immune.
    assert_eq!(cave.cell_at_idx(ascii_idx(0, 0)).obj, Obj::Steel);
    assert_eq!(cave.cell_at_idx(ascii_idx(2, 1)).obj, Obj::Door);
    assert_eq!(cave.count_obj(Obj::Boulder), 0);
    frames(&mut cave, 60, Input::NONE);
    assert_eq!(cave.cell_at_idx(ascii_idx(0, 0)).obj, Obj::Steel);
    assert_eq!(cave.cell_at_idx(ascii_idx(2, 1)).obj, Obj::Door);
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 1)).obj, Obj::Space);
}

#[test]
fn extra_life_every_2000_points_capped_at_9() {
    let mut params = ascii_params();
    params.diamond_points = 255;
    params.diamonds_needed = [9, 9, 9, 9]; // keep the door closed
    // Diamonds need support (brick floor) or they fall away before Rockford
    // arrives.
    let mut cave = ascii_cave(&["r********", "++++++++++"], params);
    let mut extra = false;
    for _ in 0..64 {
        extra |= cave
            .tick(dir(Direction::Right))
            .iter()
            .any(|e| matches!(e, Event::ExtraLife { lives: 4 }));
    }
    assert!(extra, "no extra life at 2000 points");
    assert_eq!(cave.diamonds_collected(), 8);
    assert_eq!(cave.score(), 8 * 255);
    assert_eq!(cave.lives(), 4);
}

#[test]
fn determinism_same_inputs_same_hashes() {
    let script = [
        Input::NONE,
        dir(Direction::Left),
        dir(Direction::Right),
        dir(Direction::Up),
        dir(Direction::Down),
        grab(Direction::Right),
    ];
    let mut a = Cave::new(0, 1, 42);
    let mut b = Cave::new(0, 1, 42);
    assert_eq!(a.hash(), b.hash());
    let initial = a.hash();
    let mut changed = false;
    for t in 0..400 {
        let input = script[t % script.len()];
        a.tick(input);
        b.tick(input);
        assert_eq!(a.hash(), b.hash(), "diverged at frame {t}");
        changed |= a.hash() != initial;
    }
    assert!(changed, "hash never changed; simulation appears stuck");
}

#[test]
fn seed_has_no_effect_on_simulation() {
    // Regression test for the removed RNG: different seeds, identical state.
    let mut a = Cave::new(0, 1, 1);
    let mut b = Cave::new(0, 1, 999_999);
    for t in 0..200 {
        let input = if t % 3 == 0 {
            dir(Direction::Right)
        } else {
            Input::NONE
        };
        a.tick(input);
        b.tick(input);
        assert_eq!(a.hash(), b.hash(), "seed leaked into simulation at frame {t}");
    }
}

#[test]
fn enemies_start_facing_up() {
    let cave = ascii_cave(&["fq"], ascii_params());
    assert_eq!(cave.cell_at_idx(ascii_idx(0, 0)).dir(), Direction::Up);
    assert_eq!(cave.cell_at_idx(ascii_idx(1, 0)).dir(), Direction::Up);

    // Real cave data: bare enemy nibbles load as direction 0 (up).
    let cave = Cave::new(1, 1, 42);
    let firefly = (0..CELLS)
        .find(|&i| cave.cell_at_idx(i).obj == Obj::Firefly)
        .expect("cave 1 has a firefly");
    assert_eq!(cave.cell_at_idx(firefly).dir(), Direction::Up);
}

#[test]
fn cave_0_loads_from_rom_data_and_spawns_at_param_cell() {
    let cave = Cave::new(0, 1, 42);
    assert_eq!(cave.count_obj(Obj::Diamond), 18);
    assert_eq!(cave.count_obj(Obj::Boulder), 108);
    assert_eq!(cave.count_obj(Obj::Rockford), 1);

    // Cave 0 record bytes b0/b1 = 02/03: Rockford spawns at row 2, col 3.
    assert_eq!(cave.rockford_pos(), Field::idx(3, 2));
    assert_eq!(cave.cell_at_idx(Field::idx(3, 2)).obj, Obj::Rockford);

    // The door is a separate exit cell (cave 0: row 16, col 38).
    let door = cave.door_pos().expect("cave 0 has a door");
    assert_eq!(door, Field::idx(38, 16));
    assert_eq!(cave.cell_at_idx(door).obj, Obj::Door);
    assert!(!cave.door_open());
    assert_eq!(cave.diamonds_needed(), 10);
}


/// Regression guard: caves must load from the editable emoji files (with
/// the ROM grid as fallback), not bypass them. Catches the parse path
/// being silently dropped from `Cave::new`.
#[test]
fn caves_load_from_emoji_files() {
    use crate::data::caves::{parse_cave, CAVE_FILES};
    use crate::engine::cave::cave_ids;
    for i in 0..crate::data::caves::CAVE_COUNT {
        let parsed = parse_cave(CAVE_FILES[i]).expect("emoji cave file must parse");
        assert_eq!(cave_ids(i), parsed, "cave {i} must load from its emoji file");
    }
}

/// Regression (cave 6 bug): an amoeba embedded in MUD is NOT enclosed — the
/// ROM's probe ($CF91) treats mud ($20) as growable just like space ($00).
/// It must survive and eat dirt instead of converting to a diamond.
#[test]
fn amoeba_in_mud_survives_and_eats_dirt() {
    let mut cave = ascii_cave(&["...", ".a.", "..."], ascii_params());
    for _ in 0..64 {
        assert!(!cave
            .tick(Input::NONE)
            .contains(&Event::AmoebaConverted { to_diamonds: true }));
    }
    assert!(
        cave.count_obj(Obj::Amoeba) >= 1,
        "mud-surrounded amoeba must not convert to diamonds"
    );
    // Keep ticking: it should eventually eat a mud cell (grow).
    let mut grew = false;
    for _ in 0..60 * 8 {
        grew |= cave
            .tick(Input::NONE)
            .iter()
            .any(|e| matches!(e, Event::AmoebaGrew { .. }));
    }
    assert!(grew, "amoeba never grew into mud");
}

#[test]
fn amoeba_cave6_diag() {
    if std::env::var("BDDBG").is_err() {
        return;
    }
    let mut cave = Cave::new(6, 1, 0);
    for tick in 0..(60 * 30) {
        let evs = cave.tick(Input::NONE);
        if tick % 240 == 0 {
            eprintln!(
                "tick {tick}: amoeba cells={} interval={}",
                cave.count_obj(Obj::Amoeba),
                cave.amoeba_probe_interval()
            );
        }
        if evs.iter().any(|e| matches!(e, Event::AmoebaGrew { .. })) && tick % 240 != 0 {
            // print first growths sparsely
            if tick % 60 == 0 {
                eprintln!("tick {tick}: grew");
            }
        }
        if evs.contains(&Event::AmoebaConverted { to_diamonds: true }) {
            eprintln!("tick {tick}: CONVERTED, amoeba cells left={}", cave.count_obj(Obj::Amoeba));
            for _i in 0..crate::engine::CELLS {
                // replay: amoeba already converted; print what surrounded each amoeba cell
            }
            break;
        }
        if tick == 1054 {
            use crate::engine::{WIDTH, HEIGHT};
            for y in 1..HEIGHT - 1 {
                for x in 1..WIDTH - 1 {
                    let i = y * WIDTH + x;
                    if cave.cell_at_idx(i).obj == Obj::Amoeba {
                        let nb = |dx: i32, dy: i32| cave.cell_at((x as i32 + dx) as usize, (y as i32 + dy) as usize).obj;
                        eprintln!("amoeba ({x},{y}): L={:?} R={:?} U={:?} D={:?}",
                            nb(-1, 0), nb(1, 0), nb(0, -1), nb(0, 1));
                    }
                }
            }
        }
    }
    eprintln!("final: amoeba={} diamonds={}", cave.count_obj(Obj::Amoeba), cave.count_obj(Obj::Diamond));
}
