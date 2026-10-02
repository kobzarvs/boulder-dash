# Boulder Dash (NES) — Ghidra Reverse-Engineering Findings

Environment: independent project `BoulderDashKimi` at `~/prj/tools/bd_ghidra_kimi`
(Ghidra 12.1.4 shared install untouched; PRG extracted as `prg.bin` — note the raw
`tail -c +17` yields 65536 bytes = PRG+CHR, so the PRG was cut to exactly 32768 bytes,
mapped at CPU $8000). Seeding via `scripts/Seed.java` (named entries + 19 state handlers
+ raw `JSR` scan in $D800-$E800 + all `JSR $A4CB` inline jump-table targets ROM-wide);
queries via `helper.py` (pyghidra: functions/decompile/disasm/xrefs/data/rename/force-disasm).

Engine architecture context (needed by every section below):

- Field RAM $03E0-$074F: 22 rows x 40 cells, object ID in HIGH nibble; row pointer table
  at $D091 (22 LE pointers, stride 40). Row 0/21 and col 0/39 are border.
- Loader ($B792+) reads CHR via $2007, one byte → two cells (`ASL x4` then `AND #$F0`):
  `b81a LDA $2007 / PHA / ASL x4 / STA ($57),Y` … `PLA / AND #$F0 / STA ($57),Y`.
- Per-frame physics is phased: `field_phase_dispatch` ($C419) jumps on `frame & 7` through
  the inline table at $C420 → [c430 nop, c431, c438, c43f, c446, c9a6, c9ad, c9b4].
  Phases 1-4 = bottom-up scan of rows 20-16/15-11/10-6/5-1 calling `cell_physics_fall`
  ($C46E); phases 5-7 = top-down cleanup scan rows 1-7/8-14/15-20 calling
  `cell_cleanup_pass` ($C9D9). So each cell is updated once per 8-frame cycle.
- Dirty-cell queue at $0750 (count)/$0751(wr)/$0752(rd), 55 triplets (value,row,col) at
  $0753+, pushed by `dirty_cell_push` ($D0BD), popped by `dirty_cell_pop` ($D0F8),
  rendered by `redraw_dirty_cells` ($CA53, ≤6/frame).
- Frame counters: $1E = NMI vblank counter (inc in `nmi_handler` $A2C7); $FE = gameplay
  frame counter (inc per state-11/12 tick).

## Q1. Runtime-only object IDs 1, 9, A, E, F, and the metatile attribute byte

**ID 1 ($10) = "cell vacated this cycle" marker.** Written whenever something leaves a
cell: boulder/diamond starts falling (`c60e LDA #$10 / STA ($55),Y`), rolls
(`c5a0`/`c5ba`), Rockford walks (`c1e6`) or pushes (`c2fb`). Converted to real space (0)
by the phases 5-7 cleanup scan:
```
ca05  LDA #0x0      ; cell == 0x10 branch of cell_cleanup_pass
ca07  STA (0x55),Y
```
Purpose: prevents a second object from entering a just-vacated cell within the same
8-frame cycle (the fall scan only treats exact 0 as empty, e.g. `c49b CMP #0x0`).

**ID 9 ($90|slot) = pending diamond (butterfly explosion product).** Created by
`explode_butterfly` ($C7EA): `$50 = ($FD & 7) | $90`, timer `$07F8[slot] = $10`,
`$FD++` (rotating slot). Ripens to diamond when its slot timer (8 countdown bytes at
$07F8-$07FF, decremented every gameplay frame at `ac8b-ac97`) hits 0:
```
ca0a  LDA (0x55),Y / AND #7 / TAX / LDA $07F8,X / BNE done
ca14  LDA #$80 / STA ($55),Y ... JSR $D0BD   ; -> diamond 0x80, redraw
```

**ID A ($A0) = explosion remnant (firefly/generic blast).** Blast writer ($C841) fills
the 3x3 area with $50=$A0; linger timer $B6=$10 set at `c7d5`. Cleanup:
```
ca22  LDA $B6 / BNE done                     ; wait until $B6 == 0
ca26  LDA #$10 / STA ($55),Y ...             ; -> vacated marker -> space
```
Note Rockford may walk through $A0 (move accept list at `c19e`: 0x00/0xA0/0x10).

**ID E ($E0) = Rockford.** Written at spawn (`be8a LDA #$E0 / JSR field_write`) and on
every move (`c1ef`, `c2ea`). Death checks: falling object landing on $E0
(`c4ea CMP #$E0 ... AND #1 (falling bit) ... JSR player_kill`), blast filter `c932`
counts $E0 hits, adjacency check `c408` (see Q3).

**ID F ($F0) = dead amoeba residue (inert).** Written only by `cell_cleanup_pass`:
```
ca3b  LDA #$80 / LDX $B7 / BEQ store         ; $B7==0   -> diamond 0x80
ca41  CPX #$FF / BNE keep                    ; $B7==$FF -> 0xF0
ca45  LDA #$F0 ; store: STA ($55),Y + dirty push
```
$B7==$FF is only set by the cave loader when *no* amoeba exists ($b910), so this branch
is unreachable with a live amoeba — see Q5. $F0 never falls (not in the 0x70/0x80 set at
`c474-c47a`) and is never cleaned; it is a static decorative object (metatile 15,
animated list entry in `metatile_variant_select` at `b59a`).

**Attribute byte = 2-bit palette index per object.** The 16 bytes live at **$F453**
(immediately after the last tile quad at $F44F; the 16 LE metatile pointers at $F383
span $F383-$F3A2 and point into the quad pool $F3A3+):
```
f453: 01 01 02 00 00 00 00 01 00 00 00 03 03 00 00 00
```
`redraw_dirty_cells` ($CA53) fetches `DAT_f453[cell>>4]` into $50, then
`attribute_merge` ($B620) shifts it left 0/2/4/6 bits by quadrant and ORs it into the
nametable attribute staging buffer $0136 (`b668 ASL $50 x2 … b698 ORA $50 / STA $0136,Y`).
All values are 0-3 → pure palette number (no flip/priority bits). Palette assignments:
space=1, vacated=1, mud=2, steel/door/brick/magic=0, boulder=1, diamond=0,
firefly/butterfly=3, amoeba/rockford/explosion/pending=0.

Related rendering facts: `metatile_variant_select` ($B586) adds `(cave>>2)*4` to the tile
quad pointer for static objects → per-cave-group tile variants; door ($40) uses open-door
quad at $F3F3 when $90=1 (`b5b1-b5c1`); magic wall ($60) uses active quad at $F3EB when
$B5&1. Global object animation (diamond sparkle, amoeba wobble) is done by CHR bank
switching in the NMI (`b01e: $6E -> $BFFF (MMC1 CHR0)`, `b033: $6F -> $DFFF (CHR1)`),
not by metatile changes.

## Q2. Cave parameter bytes b0/b1 and the cave timer

Loader ($B792, in `cave_load` $B78F):
```
b7a7 LDA ($55),Y / STA $93      ; b0 -> $93   (b7ad: $94=0)
b7b0 LDA ($55),Y / STA $95      ; b1 -> $95   (b7b6: $96=0)
b7b9 ... STA $8E                ; b2 -> $8E
b7be ... STA $8F                ; b3 -> $8F
b7c2 LDA $74,X / AND #3 -> Y = 4+2*difficulty
b7cb LDA ($55),Y / JSR u8_to_bcd3 -> $AE/$AF/$B0   ; diamonds needed (BCD digits)
b7dd LDA ($55),Y / JSR u8_to_bcd3 -> $B1/$B2/$B3   ; cave time (BCD digits)
```
Record example, cave A ($F263): `02 03 0a 0f | 0a 78 | 0c 64 | 0e 64 | 10 64`
→ b0=2,b1=3, diamond value 10, extra value 15; level 1: need 10, time 120.

**b0/b1 are Rockford's spawn cell (b0=row, b1=column) — not timers.** They gate nothing.
Evidence — spawn code (found via byte-scan for `LDA #$E0`), correctly aligned at $BE80:
```
be80  LDA #0 / STA $91 / STA $92
be86  LDY $93 / LDX $95 / LDA #$E0 / JSR field_write   ; field[row=b0][col=b1] = Rockford
be8f  ASL $93/ROL $94  x4        ; $93.$94 *= 16  (pixel pos, row axis)
be9f  ASL $95/ROL $96  x4        ; $95.$96 *= 16  (pixel pos, column axis)
```
(`field_write` $D07D: `TYA; ASL; -> row table $D091; X -> column`, so Y=row, X=column.)
Thereafter $93/$94 and $95/$96 are Rockford's 16-bit pixel position (walk = ±2 px/frame,
$c222-$c25d; drawn by `rockford_draw` $D39E at +$2F/+$48 screen offset; camera follow in
`camera_follow` $B1B5). They are never re-read as timers.

**Timer decrement rate:** `cave_time_tick` ($CFCC) runs every gameplay frame:
```
cfd5  LDA $1E / AND #$3F / BNE done      ; once per 64 NMI frames
cfdb  JSR time_bcd_dec                   ; $B1-$B3 BCD countdown (borrow -> 9)
```
$1E increments once per NMI/vblank. **One displayed unit = 64 frames** = 1.067 s at
60 Hz, 1.28 s at 50 Hz. No PAL/NTSC region detection exists in reset/init ($FF80/$A000
just wait for two vblanks), so the clock simply runs at the hardware's frame rate.
Cave A level 1 time = 120 units ≈ 154 s on PAL 50 Hz hardware — matching the C64 cave A
limit of 150 s, i.e. the ROM behaves as a straight PAL-timing port.
Time-out → death: `cfe6`: $91 = $91&$F0|6, $92=$40, sound 7, then substates $70=2/$71=0/
$20=10 (or 9 via `d050` centering check). Hurry-up music trigger when 30 units left
($B2==3, others 0): `ThunkFUN_811c(DAT_d060[cave>>2])` at `cff7-cfff` region.

**What actually gates amoeba growth / magic wall duration** (since b0/b1 don't): both are
hardcoded mechanics, not cave params:
- Magic wall: activation sets $B5=1 (see Q4); `magic_wall_timer` ($CDB4):
  `($FE&$0F)==0 && ($B5&1) -> $B4-- ; on 0 -> $B5=2 + magic_wall_flash`. $B4 starts 0 at
  load ($b842) and is never reloaded → fixed active window of 256×16 = **4096 frames**
  (≈68 s @60 / 82 s @50) after the first boulder touches the wall, identical for all caves.
- Amoeba: paced by $BA/$BD counters, see Q5. The only per-cave amoeba/magic-wall data are
  position lists for the flash re-render ($CE1F cave 7 / $CE30 cave 14 / $CE37 cave 15 /
  $CE50 default, (row,col) pairs terminated by a negative byte, used by $CDCC) and the
  per-cave/difficulty map patch list ($B950 table, triplets row/col/value applied for
  difficulty ≥ 2, $B8CD-$B90E).

## Q3. Firefly / butterfly movement

Both enemies are processed during the bottom-up fall scan (`cell_physics_fall` dispatches
$ B0→`firefly_move` $C638, $C0→`butterfly_move` $C6D3), so each moves ≤1 cell per 8-frame
cycle (7.5 Hz @60 fps). Low nibble = direction **0=up, 1=right, 2=down, 3=left**, plus
bit3 ($08) = "already moved this cycle" (set only for up/left moves, i.e. directions the
scan hasn't passed yet; cleared by cleanup scan `AND #$F7` at `c9fe`/`ca34`).
**Starting direction = up** for both: cave data nibbles B/C load as bare $B0/$C0 = dir 0.

Neighbor probes return: A = cell, Z if empty(0), C if amoeba ($D0) (`c82f-c840`).

**Firefly = counter-clockwise (left-turn-first) wall follower** ($C638 raw):
```
dir0 up:    c826 (left)  -> empty: move left  (c6b7, new dir 3, $BB)
            else c809 (up) -> empty: move up  (c6be, dir 0, $B8)
            else turn in place to dir 1 ($B1)
dir1 right: c809 (up)    -> move up    (dir 0)      [left turn]
            else c82c (right) -> move right (dir 1) [straight]
            else in place dir 2 ($B2)
dir2 down:  c82c (right) -> move right (dir 1); else c81d (down) -> move down (dir 2);
            else in place dir 3 ($B3)
dir3 left:  c81d (down)  -> move down  (dir 2); else c826 (left) -> move left (dir 3);
            else in place dir 0 ($B0)
```
i.e. heading D: prefer D−1 (CCW), then D, then turn to D+1 without moving. Firefly treats
amoeba as an ordinary wall (carry result ignored).

**Butterfly = clockwise (right-turn-first) wall follower** ($C6D3, handlers $C70D/$C722/
$C737/$C74C): heading D: prefer D+1, then D, then D−1 in place — exact mirror of the
firefly. Additionally, if a preferred-or-straight probe returns carry (cell == amoeba
$ D0), it jumps to $C761 → `explode_butterfly`: **a butterfly explodes on amoeba
contact**, blasting a 3x3 of pending diamonds ($90|slot, ripening after 16 frames).

**Explosion triggers:**
- Boulder or diamond ($70/$80, any fall state) in the cell **directly above** the enemy:
  `c64c-c658` (firefly) / `c6e9-c6f5` (butterfly): `LDA ($57),Y [row above]; AND #$F0;
  CMP #$70 / CMP #$80` → firefly: `explosion_score` $C944 + `explode_firefly` $C7D3
  (3x3 of $A0, $B6=$10 linger); butterfly: `explode_butterfly` $C7EA (3x3 of $90).
- Rockford adjacency (the "proximity explosion"): `player_danger_check` $C379 runs every
  8th frame while player state <3 and probes the 4 orthogonal neighbors of Rockford via
  `cell_is_enemy` $C408 (`AND #$F0; CMP #$B0; CMP #$C0 -> carry`). On contact →
  `player_kill` $C3CF: explosion centered on Rockford ($C7D3), death arc, sound 1.
- Blast ($C841): center + 8 surrounding cells, each filtered by `blast_cell_filter`
  ($C932): steel wall ($30) and door ($40) are immune (carry → skip); every $E0 cell found
  increments $4F, and `$4F != 0` afterwards → `player_kill`. So any explosion (enemy,
  boulder-triggered, or timeout-free) inside the 3x3 kills Rockford.
- Firefly explosion score ($C944): adds `c99a[i]` + `c99b[i]`, i=(cave>>2)*2 — table
  $C99A = `64 64 7d 7d 96 96 af af c8 c8 e1 e1` → 200/250/300/350/400/450 points by cave
  group (caves A-D … U-X).

## Q4. Boulder/diamond physics, pushing, shaking, RNG

Falling (`cell_physics_fall` $C46E, runs on boulder $70/diamond $80):
- Below cell empty → fall: below = `id|1` (falling bit), current = $10 (`c5f4-c612`).
  Fall-start sound 3 only when leaving rest state (`c5dd AND #$0F / BNE skip / LDA #3 /
  JSR sound_play`). Landing: low nibble cleared; boulder (not diamond) → sound 4 thud
  (`c502-c50e`).
- Falling object onto Rockford: below==$E0 && (cell&1) → `player_kill` (`c4ea-c4f6`).
  A *resting* boulder on Rockford does not kill (bit0 gate).
- Magic wall ($60): below==$60 && boulder falling (`CMP #$71`) && ($B5&2)==0 → $B5=1,
  `magic_wall_flash` ($CDCC re-renders wall cells active), and if the cell two rows down
  is empty: boulder cell → $10, two-below → **$81 diamond falling** (`c4a2-c4e6`).
  After expiry ($B5=2) boulders just rest on the wall.

**Roll-off rule — deterministic LEFT-first, no randomness** ($C514-$C5D8 raw):
- Applies when the supporting cell below ∈ {$70 boulder, $80 diamond, $50 brick wall,
  $60 magic wall} (note: brick and magic walls count as round here).
- Left first: left cell == 0 AND down-left == 0 AND up-left is not $70/$80
  (`c541-c55b`) → roll left: left = `id|9`, current = $10.
- Else mirror test right (`c560-c590`) → roll right: right = `id|1`, current = $10.
- Else stay. No RNG call anywhere in the path.

**Push — also deterministic (no probability):**
- Walk-into push: entering a boulder cell horizontally (joy $40 left / $80 right) enters
  push state (`c1d4`: $91 = $20|$02 or $30|$02, $92 = $18 = 24 frames). `player_push_wait`
  ($C278) decrements $92 each frame while re-checking the direction is still held and the
  destination (two cells over) is empty; at 0 → `player_push_exec` ($C2A7): writes $70 two
  cells over, Rockford $E0 into the boulder's cell, $10 at Rockford's old cell, sound 0,
  then walk state. If the destination is blocked when the timer expires, the push simply
  doesn't happen that frame (retry only on $92 wraparound); releasing the direction aborts
  ($91 &= $F0 at `c2a0`).
- Snap-push (button held): `player_idle_input` ($C0AE, runs when $9A&3 != 0): adjacent
  boulder + joy == $41/$81 (button+left/right) + beyond-cell empty → boulder written one
  cell farther instantly, boulder cell cleared, Rockford does not move (`c10d-c135`).
  Same path grabs diamonds ($80 → `diamond_collect`) and digs mud ($20, sound 0) at
  distance without moving.

**"Shake before falling" — not implemented.** There is no wobble/delay state in $C46E: a
boulder with empty space below starts falling immediately (same scan visit). Metatile 7 is
not in the animated-metatile list ($B586 handles $80/$90/$A0/$D0/$E0/$F0 only).

**RNG — none exists in the engine.** No LFSR/EOR-shift PRNG anywhere in gameplay code
(byte-scan for EOR/ROL patterns only finds joystick edge-detect at $A17C-$A18A, camera
borrow math at $B1BA, and sound-data regions). Every stochastic-sounding C64 mechanism is
deterministic here: roll = left-first, push = 24-frame hold, butterfly blast timer slot =
rotating counter $FD&7, amoeba = deterministic spiral (Q5). Frame counters $1E/$FE are
used only for timing/animation phases, never as decision entropy.

## Q5. Amoeba

Init (`cave_patch_and_amoeba_find` $B8B4): after applying map patches, scans the field
bottom-up for $D0; if found: `$B7=$50` (active), `$BA=$BD=$FF`, seed stored
`$B8 = row | (row==1 ? $40 : $C0)`, `$B9 = col`; else `$B7=$FF` (inactive). $B7 is the
amoeba state byte: low 2 bits = scan phase, bit7 = "found empty space this pass".

Growth (`amoeba_spiral_scan` $CE57, runs once per 8 frames when $B7≠0/$FF): a spiral
edge-walk from the seed cell, 25 probes per chunk (`LDA #$19 / STA $4D`), direction state
$4E ∈ {0 up, $40 left, $80 down, $C0 right}; on probing an amoeba cell the cursor moves
onto it and turns CW; on empty → growth attempt ($CF9F) then step back; on anything else →
step back and try next direction. Position persists across chunks in $BB/$BC; every 4
chunks ($B7 = ($B7+1) & $83) the scan restarts from the seed.

**Per-cell growth probability: there is none — growth is deterministic and paced:**
```
cf9f  LDA $B7 / ORA #$80 / STA $B7      ; mark "space found"
cfa5  DEC $BD / BNE out (CLC)           ; only every $BD-th empty probe grows
cfab  LDY $4F / LDX $50 / LDA #$D0 / JSR field_write   ; empty cell -> amoeba
cfbd  LDA $BA / SBC #4 / STA $BA / STA $BD             ; interval shrinks by 4
cfc6  LDA #$80 / STA $B7 / SEC                          ; abort chunk after 1 growth
```
$BA=$BD=$FF initially → first growth after 255 empty-probe hits, then 251, 247, … —
i.e. growth **accelerates** (interval −4 probes per new cell, wrapping after 64 growths).
At 25 probes per 8-frame chunk this is ≈3 probes/frame → first growth ≈650 frames
(~11 s @60 Hz), at most one new amoeba cell per 8-frame cycle.

**Enclosed → diamonds, as implemented:** if a full 4-chunk pass finds *no* empty cell,
bit7 of $B7 is never set and `$B7 = ($B7+1) & $83` becomes 0; then `cell_cleanup_pass`
converts every amoeba cell: `$B7==0 → $80 diamond` (`ca3b-ca47`). Note this is "no empty
orthogonal cell found along the spiral walk", not a true flood-fill of enclosed cavities.

**Overgrowth → boulders threshold: absent.** The cleanup branch `$B7==$FF → $F0` exists
(`ca41-ca45`) but $B7==$FF only ever comes from the loader when the cave has no amoeba at
all — with a live amoeba $B7 cycles through {0..3, $80..$83} and never reaches $FF
(all writes verified: $B910=$FF loader-no-amoeba, $B936=$50, $CE6E=0, $CF61=+1&$83,
$CFA3=|$80, $CFC8=$80; no indexed/indirect writes). There is no cell counter and no
200-cell (0xC8) comparison anywhere in the ROM (the two `C9 C8` byte pairs are operand
coincidences). So the classic ">200 → boulders" rule is effectively dead code; an
overgrown amoeba simply becomes "enclosed" and turns to diamonds.

Amoeba ambience: `amoeba_ambient_sound` ($C04A) — caves 6, 12, 22 with active $B7 →
sound 6 every 8 frames.

## Q6. Cave completion, scoring, extra life, death/respawn

Diamond collection (`diamond_collect` $CC95): score += $8E (diamond value) via
`score_add_extra_life`, or $8F (extra value) once $90≠0; needs BCD $AE-$B0 decremented;
at 000 → $90=1 (exit opens): door dirty-pushed ($40), sound 5, 32-frame door-open
animation (`door_open_anim` $D724, per-cave position table $CD71/$CD72), and the door
metatile renders open ($B586 → $F3F3 quad).

Exit: walking into door $40 with $90≠0 (`c1b4-c1c1`) → sound 1 → state 12 ($ACA7)
completion sequence:
- sub0 $ACC9: waits for player state 4 (door-walk); picks walk-out animation variant $54
  by remaining time (thresholds at $ADF0: 61 and 21 units).
- sub1 $AD25 (a.k.a. old "lives_logic" seed): 24-step walk-out sprite animation.
- sub2 $AD50: **time bonus — every 2nd frame: `time_bcd_dec` + `score_add(1)` + tick
  sound (every 4th), until the timer hits 0 → +1 point per remaining time unit.**
- sub3 $AD7D: marks completion `$76 |= adf6[level&3]` (bits 1/2/4/8 per difficulty); when
  all 4 difficulty bits set → advance to next cave letter ($75+=$10, wrap $60 → $74++);
  → state 15.

**Extra life: every 2000 points — confirmed**, but it lives inside `score_add_extra_life`
($CCE8), not at $AD25:
```
ccf3  ADC $78,X (score += A, 24-bit binary at $78-$7A)
cd03  compare $7A:$79:$78 vs threshold $7D:$7C:$7B
cd1d  threshold += 0x07D0        ; $D0/$07 = 2000
cd30  INC $77,X ; CMP #$0A -> clamp 9 lives
cd3c  $A0=$20 -> "1UP" animation via extra_life_anim ($D6D4)
```
Threshold initialized from table $DB54 = `00 00 00 d0 07 00` → $7B:$7C:$7D =
**0x0007D0 = 2000**, bumped by 2000 on each award (so 2000/4000/6000…). Score is stored
as 24-bit binary at $78-$7A per player (10-byte player structs at $74/$7E, index via
`player_struct_index` $A828). Initial lives = 3 (`d995 LDA #3 / STA $77 / STA $81`).

Death sources: boulder/diamond landing on Rockford ($C4F6), any 3x3 explosion containing
Rockford ($C932→$C3CF), firefly/butterfly adjacency ($C379+$C408→$C3CF), suicide combo
(both buttons held ≥64 frames: $9D≥$40 at `c38f` → state 6 delayed death), and time-out
($CFCC → state 6).
Death flow: `player_kill` $C3CF → player state 3: 80-frame ($92=$50) death arc — Rockford
thrown upward at −4 px/frame with gravity +$40/frame ($C313-$C35E) — then `set_state(13)`
($ADFA): `death_lose_life` $AE05 decrements $77,X; if negative → game-over sequence
($AE38: music 9/0x0A via $22, camera recenter, scroll-off anim at $AE78) → state 14
(password/game-over screen, $AEA9). Otherwise (2-player: swaps active player when
applicable, $AE0C-$AE18) → state 2/5 → cave reload: the cave is fully re-read from CHR and
Rockford respawns at the record's b0/b1 cell ($BE80). Cave/level progress ($74/$75) is
unchanged on death — you retry the same cave.

## Newly identified / renamed functions (70 renames applied in project BoulderDashKimi)

Core loop/system: `init_and_state_dispatch` $A000 (state table $A5F2, dispatch via
$11/$12 JMP ind), `set_state` $A258 (→$1F, clears $20-$22), `jump_table_dispatch` $A4CB
(inline-tables after JSR), `nmi_handler` $A2C7, `reset` $FF80, `mmc1_write_control` $A263
(→$9FFF), `mmc1_write_regs` $A28B, `wait_vblank` $A4E1, `ppu_set_addr` $A1A6,
`player_struct_index` $A828 (X=0 or 10 by player), `input_select` $D066,
`sound_play` $BFD0.

Cave load/render: `cave_load` $B78F, `u8_to_bcd3` $B849, `cave_patch_and_amoeba_find`
$B8B4, `cave_intro_pan` $BB90, `camera_follow` $B1B5, `camera_update` $B19D,
`scroll_strip_render` $B24B, `scroll_step` $B392, `metatile_variant_select` $B586,
`attribute_merge` $B620, `redraw_dirty_cells` $CA53, `dirty_cell_push` $D0BD,
`dirty_cell_pop` $D0F8, `hud_render` $D2B3, `rockford_draw` $D39E, `sprite_draw_at_5f`
$D5E8, `sprite_draw_cam` $A505.

Field access: `field_read` $CC83 (Y=row,X=col→A), `field_write` $D07D (A=val,X=col,Y=row),
`rockford_cell` $CC5C, `skip_dirty_if_covered` $C615.

Physics: `field_phase_dispatch` $C419, `scan_fall_rows_20_16/15_11/10_6/5_1` $C431/$C438/
$C43F/$C446, `cell_physics_fall` $C46E, `scan_cleanup_rows_1_7/8_14/15_20` $C9A6/$C9AD/
$C9B4, `cell_cleanup_pass` $C9D9, `firefly_move` $C638, `butterfly_move` $C6D3,
`explode_firefly` $C7D3, `explode_butterfly` $C7EA, `blast_cell_filter` $C932,
`explosion_score` $C944, `explosion_anim` $D74C.

Amoeba/magic wall/timer: `amoeba_spiral_scan` $CE57, `amoeba_grow_cell` $CF9F,
`amoeba_ambient_sound` $C04A, `magic_wall_timer` $CDB4, `magic_wall_flash` $CDCC,
`cave_time_tick` $CFCC, `time_bcd_dec` $D03F.

Player/score: `player_dispatch` $C071, `player_idle_input` $C0AE, `player_walk_pixels`
$C20F, `player_push_wait` $C278, `player_death_arc` $C313, `player_danger_check` $C379,
`player_kill` $C3CF, `diamond_collect` $CC95, `score_add_extra_life` $CCE8,
`extra_life_anim` $D6D4, `door_open_anim` $D724, `cave_complete_seq` $AD25,
`death_lose_life` $AE05.

## Things that could NOT be determined / notable negatives

- b0/b1 are spawn coordinates, not timers (the premise of timer units is refuted by the
  ×16 conversion at $BE8F/$BE9F and the $E0 spawn write at $BE8A).
- No gameplay RNG exists; nothing matching the C64's random push chance, random roll
  direction, shake-before-fall animation, or 200-cell amoeba limit is implemented. The
  $F0 (amoeba→boulder-like) conversion is unreachable in normal play.
- PAL vs NTSC cannot be proven from code (no region check); the 64-frames-per-time-unit
  tick is hardware-rate dependent (1.067 s @60 Hz, 1.28 s @50 Hz).
- The sound engine ($81C0+, command handler $86A9) was seeded and partially mapped but not
  analyzed in depth (out of scope for Q1-Q6); music/sfx IDs cited above come from call-site
  arguments to `sound_play`/`thunk_FUN_811c`.
- Per-cave exit-door pixel positions ($CD71/$CD72) and the $BC98 9-byte camera/intro table
  were identified structurally but their exact field semantics (pan path targets) were only
  partially decoded — cosmetic intro behavior, not needed for Q1-Q6.

---

# APPENDIX — follow-up Q7-Q12 (session 2, project BoulderDashKimi)

New renames applied this session: none needed beyond Q1-Q6 set; key new labels used below:
`screen_block_copy` $A808 (50-byte record → staging $0116, sets $4C=8), `palette_load_bg` $A40E
(16 bytes → $2B-$3A), `palette_load_spr` $A419 (16 bytes → $3B-$4A, then copies backdrop $2B into
$3B/$3F/$43/$47), `oam_clear_tail` $A5E0 (fill $0200+$4B..$02FF with $F0, $4B=0),
`oam_clear_all` $A0DD, `sprite_draw_raw` $A5B0 (no camera subtract, no attr OR),
`demo_input_feed` $BF0D, `password_validate` $A8BA, `map_node_walk` $B6B0,
`map_script_engine` $E866, `cave_index` $B6A0.

## Q7. Rockford sprite graphics + color select

**OAM pipeline.** Shadow page $0200; DMA in NMI $A2C7 (only when $18 != 0):
```
a2d0  LDA $2002 / LDA #0 / STA $2003 / LDA #2 / STA $4014   ; page $0200 -> OAM
a2dd  JSR $B007        ; scroll/$2000, MMC1 CHR banks $6E/$6F, $4C-flag PPU ops
```
Sprites are appended at offset $4B by the blitter `sprite_draw_cam` $A505 (camera-relative,
subtracts $C0.$C1 vert / $BE.$BF horiz) or `sprite_draw_raw` $A5B0 (absolute). After all draws,
gameplay states call `oam_clear_tail` $A5E0 (`a5e6 STA $0200,X` loop, value $F0) to hide the
rest and reset $4B=0. Metasprite record format (both blitters): 4 bytes/sprite
`[Yofs, tile, attr, Xofs]`, terminated by a $7F byte; `sprite_draw_cam` ORs A into attr
(`a570 ORA $10`); signed offsets (sign-extend via BMI at a547/a578).

**rockford_draw $D39E:** screen pos = ($93.$94)+$2F vertical, ($95.$96)+$48 horizontal, then
`jump_table_dispatch($91 & $F)` over the 9-entry inline table at $D3BF:
`[d3d1, d40b, d40b, d428, d437, d43e, d44b, d458, d45f]`. All paths end at $D466/$D478:
`$4B=$50`, A=0, JSR $A505 — Rockford occupies OAM slots $14-$15 ($0250-$0257), 2 sprites =
a 16x8 head overlay only. The body is the background metatile 15 (object $E0): quad pointer
$F3A1 -> $F44F = tiles `36 38 37 39` (pattern table 1, CHR1 bank 0-3, animated by the per-frame
CHR1 bank swap $6F = ($FE&$18)>>3 at `ac82-ac89`).

**Metasprite data (all at $D492-$D617):**
- Direction frame-pointer tables list at $D4AC = `[D4B4(up), D4E0(down), D50C(left), D538(right)]`,
  each = 4 LE frame pointers; direction map $D482 indexed by $9A>>4 (1=up,2=down,4=left,8=right);
  walk frame = ($FE&$18)>>3 (new frame every 8 frames). Walk tiles:
  up $2C/$2E, $30/$32, $32'/$30', $2E'/$2C'; down $34/$36, $38/$3A, $3A'/$38', $36'/$34';
  left $20/$22, $24/$26, $20/$22, $28/$2A; right = left tiles with attr|$40 (h-flip).
  (' = attr $40 h-flipped.) Idle ($9C==0): table $D492 = [D49A,D4A3,D49A,D4A3], frame
  ($FE&$60)>>5 — blink every 32 frames, tiles $3C/$3E (open) vs $40/$42 (closed).
  Stopped mid-walk ($9C!=0): table $D4AC indexed by $91>>4 (facing) via d3db-d3e6.
- Per player-state ($91&$F): 0 = idle/walk dispatch (d3d1); 1,2 = walk/push (d40b); 3 = death
  (d428, table $D564 = [$D56C,$D575,..], 2-frame wobble ($FE&$30)>>4, tiles $44/$46);
  4 = door-walk static $D586 (tiles $1C/$1C'); 5 = table $D57E = [$D586,$D58F,$D598,$D5A1],
  frame ($FE&$C)>>2 — rising/float anim (Y offset $F0/$E8/$E0/$E8, tiles $1C/$1E);
  6 = suicide flash (d44b, table $D5AA = [$D5B2,$D5BB,$D5C4,$D5CD]: $B8/$B8', walk frame,
  $B6/$B6', flipped walk); 7 = static $D5D6 ($BA/$BA', Y=-24); 8 = static $D5DF ($BC/$BC').
- Cave-complete walk-out: `sprite_draw_at_5f` $D5E8 — variant $54 indexes table $D61C =
  [$D622,$D633,$D644] (5-sprite-wide metasprites, tiles $90/$94…, $6C-$74 attr 3), palette
  cycled by $D618 = `01 02 03 02` via ($FE&$18)>>3 (passed as the attr-OR arg → sprite
  palette 1/2/3 sparkle), OAM base $4B=$58.

**Which CHR bank:** PPUCTRL shadow $19 = $B0 from init (`a035 LDA #$B0 / STA $19`) and bit3 is
never set anywhere (verified: all `STA $19` sites only AND/EOR bits 0-2,7) → sprites always use
pattern table 0 = CHR0 register $6E, which is 4 in every gameplay path (init $A02F, screen prep
$A7A6, cave intro pan $BB92, attract $D7FB). Background uses pattern table 1 = CHR1 $6F, which
cycles 0-3 in gameplay. **Rockford's tiles live in CHR bank 4 only** (CHR offset $4000-$4FFF =
.nes file offset $8010+$10*tile): tiles $1C-$2A, $30-$46, $B6-$BC. That is why a visual scan of
the four animation banks 0-3 missed him; in banks 0-3 the same tile indices hold background
animation variants (diamond sparkle etc.) — e.g. tile $2C differs in all of banks 0-3, while the
replicated-in-0-3 set ($1C-$25,$3A-$41,$46,$B6,…) is exactly the overlap shared with his
metasprites that also appear on non-gameplay screens. (Verified by byte-comparing banks of the
CHR extracted from Boulder_Dash.nes; bank-4 tiles $2C/$2E render as the 16x8 head: hat, face,
eyes.)

**Color select screen = state 2** ($A68C, subs $A69C/$A6A7/$A6AD/$A6ED). Sub3 $A6ED draws
Rockford with forced $9A=$40 pose (`a6f0 LDA #$40 / STA $9A / JSR $D39E`) and handles input:
left/right ($C0 mask, debounced by $22) walks index $88/$89 (per player) mod 16
(`a740 TYA / AND #$0F / STA $88,X`), then:
```
a746  LDA $A769,Y / STA $3D / LDA #1 / STA $4C   ; new color -> palette staging
```
$A769 = 16-entry color table: `20 21 22 23 24 25 26 27 28 29 2a 2b 2c 00 10 16`.
**$3D is not a variable being copied — it IS palette staging byte $3F12** (sprite palette 0,
color 2): `palette_load_spr` $A419 copies a 16-byte descriptor into $3B-$4A, and every caller
that must preserve the chosen color does `LDA $3D / PHA / JSR $A419 / PLA / STA $3D`
(e.g. $E045-$E053, $EA28-$EA3B), or re-stores $A769[$88/$89] afterwards ($A7C1 gameplay prep,
$AFC2 game-over, $BC10). Default index 6 ($D99B: $88=$89=6 → color $26). A button confirms
(`a75f LDA #3 / JSR set_state` → state 3).

## Q8. Palettes per world + attribute composition

**Cave gameplay palettes are constant.** Screen prep $A779 (used by states 2,3,5,7,14) loads:
```
a7b3  LDX #$A7 / LDY #$D2 / JSR $A40E   ; BG palettes  <- descriptor $A7D2
a7ba  LDX #$A7 / LDY #$E2 / JSR $A419   ; sprite pals  <- descriptor $A7E2
```
$A7D2 (BG): `22 37 27 07 | 22 37 29 09 | 22 20 10 0f | 22 20 3a 1a`
$A7E2 (sprite): `22 36 26 0f | 22 20 10 0f | 22 20 22 0f | 22 20 10 22`, byte $3D overridden by
the selected Rockford color. No per-cave/per-world variation exists in cave gameplay — variety
comes from per-cave-group metatile tile variants (Q1) and the fixed palette.

**$EA9E / $F0CA are map-screen palettes, not gameplay.** $EA28 ($03E0 = world index 0-5):
```
ea30  LDY $EA92,X / LDA $EA93,X / JSR $A419   ; $EA92 = [EA9E, EAAE, EABE, EAAE, EABE, EABE]
```
→ 3 distinct 16-byte sprite/BG descriptors at $EA9E/$EAAE/$EABE, worlds 0-2 then repeated.
State 18 ($F008, per-difficulty splash): `f045 LDY $F0C2,X / LDA $F0C3,X / JSR $A40E` with
X = ($74&3)*2 → $F0C2 = [F0CA, F0CA, F0DA, F0EA] (difficulties 0/1 share). CHR descriptor
table $FD9A = [FDA2, FDA2, FE35, FEB5] likewise per difficulty.

**Attribute byte composition** — `attribute_merge` $B620 (param_1=Y? param_2=X? of the 2x2
metatile cell; full decompile): target attribute address = $23C0 + (computed from coordinates,
`DAT_0055 = $23C0 + …`), staging buffer $0136[index = addr & $3F]; the object's 2-bit value
$50 (from table $F453, see Q1) is shifted 0/2/4/6 by quadrant — quadrant bits are (param_2 bit6,
param_2 bit1) per the decompiled branch tree: neither → mask $FC shift 0; bit1 → $F3/shift 2;
bit6 → $CF/shift 4; both → $3F/shift 6 — then OR into staging. Upload: `screen_block_copy`
$A808 sets $4C=8; the NMI handler $B048+ shifts $4C right bit-by-bit and runs descriptor table
$B074 (bit0 = palette upload $2B-$4A → $3F00; bits 1/2 = $2001 on/off; bit3 = staged
block/attribute write). Palette fades: $A456 (out, subtract $10 steps to floor $0F) / $A48A (in).

## Q9. Game-flow state machine (table $A5F2, 19 LE entries)

Dispatch: `init_and_state_dispatch` $A000, state in $1F, substates in $20/$21/$22 via
`jump_table_dispatch` $A4CB. `set_state` $A258. Screen-descriptor records are 50 bytes
(copied by $A808 to $0116 staging; format starts with target pointer — e.g. $FC70 record 0
begins `72 fc 8b 21 …`).

| # | entry | behavior | screen data |
|---|-------|----------|-------------|
| 0 | $A618 | Title screen. Setup (CHR1=5, nametable fills $DA77/$DA8C, palette $A674), then wait: after $20≥$50 any of $29&$F, or on $20 wraparound → state 1 | palette $A674; CHR via $A313($1E00,bank5)→$0600/$0700, $A1BD($0600) |
| 1 | $A684 | Attract/menu. $D7DF substates: $D7EC setup (banks 4/5, clear sprite recs $03E8), $D8E6 vertical scroll-in ($1C), $D93C menu/attract main: timeout path sets demo mode $72=$90, inits structs ($74-$87 from $DB50, lives 3, colors 6) and cycles demo cave $73 (0-2, wrap, $DA0D: $75=$DB4D[$73]=00/20/30) → state 9; Start → $72=$10 (1P) or $30 (2P) per $53 toggle ($DA2A-$DA5E), copy $DB50 init record to both players, → state 2 | scroll/text via $DB75/$DC8B/$DD72, blocks $F634 |
| 2 | $A68C | Color select (see Q7). A → state 3 | prep $A779; blocks $FC70/$8EFB/$FD48; Rockford sprite |
| 3 | $A833 | Password entry: sub1 $A858 edits 6 digits $4D-$52 (cursor $53; left/right move, up/down inc/dec wrap 0-9; $D158/$D18A render; Rockford pose); A → `password_validate` (Q10); sub2 $A905 shows result blocks ($89FC), then $74|=$80 → state 4 | blocks $FC70, $89FC |
| 4 | $A9E7 | World-map intro (first arrival): $DF80 — fade, map setup ($E9B4 clears nametables), $03E0=$75>>4, script table $E909[world], per-world palette $EA28; subs $DF8F/$E014/$E045/$E05C; scroll-up exit ($1B-=3) → state 6 | scripts $E909..$E947, palettes $EA92→$EA9E |
| 5 | $A9ED | Transition: screen prep $A779 → state 6 | — |
| 6 | $A9F8 | Map walk: $AA06 ($4D=$75>>4 world), $AA1A draws node sprites ($D19B/$D1E0/$D284) + `map_node_walk` $B6B0 (path tables $B706[world]; d-pad moves cursor $4E node-to-node, sound 6). A at node $4E≠0: if $76 & $B78A[$4E] (already completed, masks 00 01 02 04 08) ignore; else $75 = ($75&$F0)|($4E-1) → state 8 | node sprites; $B706 path tables |
| 7 | $AA31 | Transition: prep $A779 → state 8 | — |
| 8 | $AA3C | Pre-cave status screen: blocks $8EFB, text $FD43, lives digit ($77+$81 → $0126), P2 flag $83→$0119; $AB1A waits with static Rockford; A → fade ($A4F5) → state 9 | $8EFB, $FD43 |
| 9 | $AB34 | Cave load: `cave_load` $B78F, patch/amoeba $B8B4, intro pan $BB90, fade-in $A456 → state 10 | cave CHR via $2007 (Q1/Q2) |
| 10 | $AB64 | Intro-pan wait: $BD70 walks 9-byte camera records $BC9C+9*$8D; non-demo + Start → state 1 (abort to attract) | cave field |
| 11 | $AB7C | Gameplay frame (see Q1-Q6): HUD/rockford/extra-life/explosion/door anims, `oam_clear_tail`, dispatch on $70 (0=physics+input, 1=pause); demo mode ($72 bit7): Start → state 1, else `demo_input_feed` $BF0D; $6F=($FE&$18)>>3 BG anim; timers $07F8/$B6; $FE++ | cave field + HUD |
| 12 | $ACA7 | Cave complete: $ACC9 wait door-walk (variant by time, $ADF0), $AD25 walk-out anim, $AD50 time bonus (+1/unit), $AD7D set completion bit $76|=$ADF6[$74&3], $75+=$10 wrap $60→$74++ & $0408=1 if ($74&7)≥4 (game beaten), → state 15 | walk-out metasprites (Q7) |
| 13 | $ADFA | Death: $AE05 lose life ($77,X--); lives<0 → game-over subs; else 2P swap ($72^=1 if ($72&$30)==$30), $4D=2 or 5 (by $74 bits 7/6), $74&=$BF; $AE66 wait → set_state($4D)+fade; $AE78 game-over scroll-off ($D695) → $76&=$F0 → state 14 | — |
| 14 | $AEA9 | Game over / continue menu: prep $A779, blocks $8EFB, text $FD6B; $AF34: up/down toggle $53, A → continue ($53=0: sound $0C, $74|=$40, $75/$76 low nibble cleared, lives=3, score/threshold reset via $AFF9←$DB54) or end ($53=1: sound $11, $72 &= $EEB5[player] (EF/DF clears per-player active bit); if ($72&$30)==0 → fade → state 1, else swap player $72^=1 → $AFB2 → state 6 via $AFD4/$74|=$80) | $FD6B, $89FC |
| 15 | $EACE | World-map hub (fixed bank): $EADB setup ($6D=$12, palette $EBC2 via $A40E + per-world $EA28, sprite records $0409←$EB5C, script ptrs $03E2=$EBD2/$03E4=$EBAB, fade-in, sound $23); $EB64 map anim ($EBA8 draws 6 node sprites via $EBDD table, $EBE9, `map_script_engine` $E866); $EB7E waits script end then A/B ($E025) → state 17 if $0408≠0 else state 16 | scripts $EBC2+; palettes $EA9E+ |
| 16 | $EC95 | Map transition: $ECA2 setup (script $ED0F, $EA42 string copy), $ECF8 walks sub-map $E0EA (dispatch on $03E0), $ED04 → state 18 + fade | per-world via $E07D/$E071 tables |
| 17 | $ED1C | Ending / beaten-game screen (reached only with $0408=1, i.e. all 6 worlds × difficulty 4 done): 6 subs $ED31/$ED3B/$EDBA/$EE0D/$EE1D/$EE37, setup uses script table $EEB7[$03E0], palette table $EDAE | $EEB7 scripts, $EDAE palettes |
| 18 | $F008 | Per-difficulty splash/interstitial: $F015 setup (CHR desc $FD9A[diff], BG palette $F0C2[diff]→$F0CA/$F0DA/$F0EA, sprite records $F0FA[diff]); $F096 draw+script ($F1AE: difficulty-dispatch text via $F20C, PRG banked write $8880); $F0A9 waits Start → state 5 | $FD9A/$F0C2/$F0FA tables |

Main loop: 0→1→(demo 9…) or (2→3→4→6→7→8→9→10→11→12→15→16→18→5→6…); death 11→13→(14→1 or →6); ending 12→15→17.

## Q10. Password system

Entry: state 3 sub1 $A858 (editing above). Confirm = A button (`a8af LDA $9B / AND #1 / BNE`,
sound 6) → falls through into `password_validate` $A8BA:
```
a8ba  $67 = 0                                  ; row index
a8be  X = $67*8                                ; row stride 8
a8c6  LDA $4D,Y / CMP $A927,X / BNE next       ; compare 6 digits $4D-$52
      ...loop Y=0..5
a8e2  JSR player_struct_index                  ; match: X = 0 or 10
a8eb  LDA $A92D,Y / STA $74,X                  ; row byte6 -> difficulty/flags
a8f0  LDA $A92E,Y / STA $75,X                  ; row byte7 -> world<<4 | cave
      $76,X = 0                                ; completion bits cleared
a8f9  INC $20                                  ; (both match and fail land here)
```
24 rows × 8 bytes at $A927 = 6-digit password | $74 | $75. Full table (digits → $74,$75):
```
000000→00,00  635807→00,10  840137→00,20  840967→00,30  225378→00,40  752053→00,50
423480→01,00  457397→01,10  432579→01,20  864101→01,30  995065→01,40  827100→01,50
532375→02,00  243481→02,10  606664→02,20  692551→02,30  724892→02,40  772974→02,50
724045→03,00  723846→03,10  231750→03,20  228733→03,30  923838→03,40  184904→03,50
```
i.e. rows = difficulty 0-3 (in $74, loader masks `AND #3` at $B7C2) × 6 worlds
($75 = world<<4; cave-in-world low nibble is always 0 in the table — passwords start you at the
world's first cave). No checksum: the digits are an arbitrary literal lookup. Failure leaves
$74/$75 unchanged (state 3 sub2 → state 4 regardless, so a bad password effectively starts the
default game). Level-5 content (difficulty 4) has no password row.

## Q11. Demo/attract mode

Yes — recorded scripts. Demo mode is $72=$90 (bit7 set, written at $D985 in the attract
sequence after the timeout path; player structs reset there too: lives 3, colors 6).
During gameplay (state 11):
```
aba0  LDA $72 / BPL normal            ; bit7 = demo
aba4  LDA $29 / AND #8 → start exits  ; (state 1)
abb6  JSR $BF0D                       ; demo_input_feed
```
`demo_input_feed` $BF0D: script pointer = table $BF3C[$73*2] (3 scripts: $BF42, $BF90, $BFAE;
cave for demo d = $DB4D[d] = $00/$20/$30 loaded into $75 at $DA0D). Script = (input, duration)
pairs: `$9A = script[$9E*2]`, every 8 frames ($FE&7==0) `$9F--`; at 0, `$9E++` and
`$9F = script[$9E*2+1]`. Terminator `00 ff` (input 0, duration $FF). Inputs are the usual
$9A high-nibble d-pad bits ($10 up/$20 down/$40 left/$80 right), e.g. script 0 at $BF42:
`10 01 80 08 20 01 80 0a 10 01 40 02 80 04 …`. $73 increments per attract loop ($DA01, wrap
≥3→0). So the attract demo is real engine gameplay of caves A/C/D ($75=00/20/30) driven by
fixed recordings — deterministic thanks to the RNG-free engine (Q4).

## Q12. Two-player mode

Mode bits live in $72: bit0 = active player index, bit4/bit5 = per-player "in game" flags
($10 = 1P-only game, $30 = 2P game; set from the title menu choice $53 at $DA56-$DA5E),
bit7 = demo ($90). Two 10-byte player structs at $74-$7D / $7E-$87 (world/cave $75/$7F,
difficulty+flags $74/$7E, completion $76/$80, lives $77/$81, score $78-$7A/$82-$84, extra-life
threshold $7B-$7D/$85-$87); `player_struct_index` $A828 maps $72&1 → X=0/10, and
`input_select` $D066 maps $9A/$9B to $23/$29 (P1) or $24/$2A (P2) by $72&1.

Alternation is death-driven, not turn-timer driven:
- Death (`death_lose_life` $AE05): after decrementing lives, if ($72&$30)==$30 → `$72 ^= 1`
  (`ae14-ae18`) — the other player takes the next attempt; each player keeps their own
  world/cave/difficulty, so the swap also switches which cave is loaded next.
- Game over (state 14, $AF5F-$AF73): "end" clears the current player's active bit
  ($72 &= $EEB5[player], masks $EF/$DF); if ($72&$30)==0 nobody is left → fade → state 1
  (title); otherwise $72 ^= 1 and play continues as the survivor (→ state 6).
- Status screens mark the P2 side: $83 written to staging $0119 when $72&1 (e.g. $AA5B-$AA63
  state 8, $AED4 state 14); lives digit = $77+$81 → $0126.
- Demo mode forces $72=$90 (bit7 + 1P flag clear) so the swap/input paths above all collapse
  to player 1 scripted input.

## Not determined / caveats (session 2)

- Exact semantic labels for states 16/18 ("map transition" vs "quest splash") are inferred from
  drawn content (difficulty-indexed CHR/palette tables $FD9A/$F0C2, script engine $E866); the
  map screens are bytecode-driven ($FC/$FD/$FE/$FF commands in $E866) and the script data was
  not fully decoded.
- The 50-byte screen-record format ($A808) has at least two sub-formats (PPU-address-prefixed
  at $FC70, plain block at $8EFB); individual records were not decoded byte-by-byte.
- $B6A0 cave-index math (`($75>>5) + ($75&3)`) is as disassembled; how it maps onto the 24-entry
  cave-record pointer table $F233 was not fully cross-checked against all 24 records.
- The $0408=1 ending condition (world wrap + ($74&7)≥4, $ADC4-$ADD7) implies a 5th difficulty
  loop exists even though the loader masks difficulty with `AND #3` — behavior of difficulty 4
  (wrap to 0 vs intended level 5) not traced further.

---

# APPENDIX 2 — screen/UI formats fully decoded (session 3, `xtask extract-screens`)

All screen drawing is one of three formats. Everything below is verified by rendering the
decoded data to PNGs (game/assets/screens/, 24 files) and reading the text.

**1. 50-byte VRAM records** (`screen_block_copy` $A808 copies ≤50 bytes to staging $0116,
sets $4C=8; NMI bit3 handler $B09D uploads): segments of `(addr_lo, addr_hi, count, data…)`
terminated by a lone $FF. No sub-formats — the "$8EFB"/"$89FC" entries in Q9 were byte-swapped
misreads of $FB8E/$FC89 (X=hi, Y=lo at the $A808 call). Tables (all decoded):
- $F634 = 13-entry attract/menu text table (state 1, indexed by $21; NOT $F638). Recs 0-3 →
  NT0: row 13 "1P … HI SCORE … 2P", row 14 three 000000 score groups, row 25 "TM", row 27
  "1PLAYER   2PLAYER", row 29 "©1990 VICTOR MUSICAL". Recs 4-11 → NT2 ($284F+): legal page 2
  ("…INDUSTRIES,INC." / "LICENSED BY NINTENDO." / "©1990 DATA EAST CORP." / "PUBLISHED UNDER
  LICENSE FROM" / "FIRST STAR SOFTWARE, INC." / "AUDIO VISUAL MATE(RIAL)" / "COPYRIGHT 1984,
  1990."). Rec 12 → $2BC0 NT2 attributes.
- $FB8E = color-select frame box (5 records, one per substate); $FC48 = "1 PLAYER" + "COLOR"
  "SELECT"; $FC70 = "PASS WORD★" ($218B, two words) + digit-row blanking; $FC89 = 5 records
  restoring backdrop tiles behind the password digit row (state 3 sub2); $FD43 = status
  "1 PLAYER ★ n" + "WORLD 00-00"; $FD6B = game over "1 PLAYER" / "CONTINUE" / "END" +
  "PASS WORD★".
- Password digits are 8x16 sprites ($D158): y=$64, x=$68+8i, tile $76+2d, CHR bank 4,
  sprite palette group 2.

**2. `$A1BD` RLE streams** (also read from CHR bank tails via $A313 → RAM $0600/$0700):
first 2 bytes = implicit start addr (lo, hi). FE lo hi = set addr + row anchor; FD = next row
(**anchor += $20, write addr = anchor — literal writes do NOT move the anchor**, so data is
stored as a diagonal snake that paints horizontal rows); FC = toggle $2000.2 (+1/+32 column
mode); FB n = n literal bytes (the only way to emit tiles $FB-$FF); FF = end. Streams:
- Title/legal screen (state 0): CHR bank 5 offset $E00 (PPU $1E00), over $17 fill on NT0.
- Map frame (sky/grass window): CHR bank 7 offset $DC0 (PPU $1DC0), over $80 fill on $2400.
- Per-world map images: $EA6E table, 6×[bank,-,lo,hi] → banks 0/0/1/1/2/2 at $1EE0/$1F60;
  per-world attr rows: CHR bank 3 offset $EE0+world*$20 → $27C8 (attr rows 1-4).
- Hub/ending base image (gold dome): bank 7 offset $F2E. Ending overlay: PRG $EFF5.
- "TRY THE NEXT STAGE★": PRG $E02F ($2446). Difficulty splashes: $FD9A = [$FDA2, $FDA2,
  $FE35, $FEB5] (PRG streams).
- BG palettes stay at the gameplay $A7D2 on map intros; hub/ending use $EBC2; splashes use
  $F0C2 table → $F0CA/$F0CA/$F0DA/$F0EA.

**3. `$E866` map scripts** (one tile per 4 frames, typewriter): FE lo hi = set row anchor
($03E4) + write cursor ($03E6); FD = next row (anchor += $20, cursor = anchor); FC lo hi =
jump; FF = done; else tile at cursor++. Captions ($E909 table, anchor
$26A9) write "<NAME> WORLD" — BOULDER/ICE/SAND/OCEAN/RELIC/VOLCANO — via a shared "WORLD" tail
at $E94F. Hub $EBD2 = "WONDERFUL★" ($26AB). Endings ($EEB7, anchor $26A6) = per-world credits
("PROGRAM FARMER IMAI ★★★ SAS / GRAPHIC …", "PRODUCERS KAWAI / NAKAMURA", … "The End."). Splash scripts
($F0FA, anchor $2686) = "TRY THE NEXT LEVEL★ / PASSWORD ******" where ****** is the literal
password (diff 0/1: 423480, 2: 532375, 3: 724045 — matching the Q10 table), then FC → $F19B
" PUSH START BUTTON…" tail.

**Raw backdrop**: `screen_prep` ($A794 → $A334) copies 1024 bytes from PRG $F78E to nametable
$2000 — the islands world-map image ("WORLD MAP★", node icons) shared by map-walk (state 6),
color select, password, status and game-over screens (states 2/3/8/14 draw records over it).

Extracted data lands in game/src/data/screens.rs (records, streams, scripts, palettes,
composited nametables, `decode_stream`/`decode_script`); PNG previews in game/assets/screens/.
