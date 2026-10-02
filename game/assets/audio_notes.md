# NES Boulder Dash — decoded sound-driver format

Reverse-engineered from `Boulder_Dash.nes` (disassembly of the sound driver at
CPU $8000-$8990, command jump table $8643-$86A8) and cross-validated against an
independent Python model plus golden note traces in `game/src/audio`.

## Driver architecture

- Entry `sound_play` ($811C, thunks $8003/$BFD0): A = slot id; table of 36 LE
  pointers at $8949. Slot header byte 0 = channel mask; **bit7 set → SFX
  layer** ($0302/$030A path), **clear → music layer** ($0301/$030B path).
  Header bytes 1-2 = LE pointer into the aux table ($8991, 14 records x 5
  bytes): a per-slot bitmask of *which incoming sound ids may preempt this
  slot* (checked at $8189). Not needed for playback; ignored by the player.
  Then one LE channel pointer per set mask bit (order pulse1, pulse2,
  triangle, noise).
- Music layer tick ($81B4): a 16-bit accumulator adds `$0306:$0307` every NMI
  (60.0988 Hz); a **driver tick** happens on carry. Default $FFFF → ~1
  tick/frame. `$BB` sets the addend, i.e. song tempo. Durations count ticks.
- SFX layer: one tick per NMI, no tempo; durations are raw frames.
- Music note start ($8270): unless tied, write period ($8567: sweep $4001,
  PERIODS[transpose+note] + signed detune → $4002/$4003|8); triangle gets
  $4008=$FF; noise gets $400E = note&$0F, $400F=8. Other ticks run the volume
  envelope ($85CD) and duty animation, then rewrite volume ($8502).
- SFX quirk: the post-parse APU writes run with X = channel+4, so the SFX
  volume register is $0330 (`A7`/`AB`/`AD`/`B0`/`B2` ramp it) and the SFX duty
  index is $0338 (`A9`/`AF`, init 2 = 50%). Period writes come from the
  16-bit $0318/$031C pair (`A8` set, `AC`/`B1` slide).

## Stream grammar

- Music: bytes < $80 = duration (ticks) for the *next* note/rest; $80-$8F =
  note nibble 0-15 (period = PERIODS[transpose + nibble]); >= $90 = command.
- SFX: any byte < $90 = duration in frames; ends the current parse pass.
- Channels end with `$A2` (music) / `$B6` (SFX) or loop forever with `$A1`.
  A top-level `$BF` (return without call) also ends a channel.

## Command set ($90-$C2; table at $8643 is 51 entries, not 43)

Used commands, with example streams from TRACKS:

| Cmd | Operands | Meaning | Example |
|-----|----------|---------|---------|
| $90 | — | rest for current duration | slot 18: `06 90` = 6-tick rest |
| $91 | — | transpose += 12 | slot 21 noise: `06 91 85 92 85` (octave up/down pair) |
| $92 | — | transpose -= 12 | same |
| $93 | n | transpose = n (base index into PERIODS) | `93 0C` |
| $94 | n | volume base ($035E; reloaded on each note) | `94 0F` |
| $95 | — | tie/slur: sustain (no envelope decay) and glide into the next note (no period rewrite) | slot 19 tri: `95 8B` |
| $96 | n | duty = n&3 (0=12.5% 1=25% 2=50% 3=75%) | `96 02` |
| $97 | n | duty-cycle animation on, rate n (unused in data) | — |
| $98 | — | duty animation off, duty=2 (unused) | — |
| $99 | n | envelope: cut to 0 after n ticks | — |
| $9A | n | envelope: gradual decay, volume -= 1 every n/vol_base ticks | `9A 01` (pluck) .. `9A 7F` (sustain) |
| $9B | n | envelope: gated by note length — hold (dur/4)*n ticks, then cut | `9B 03` = 75% gate |
| $9C | n | envelope: both bits set (unused in data) | — |
| $9D | — | envelope off | — |
| $9E | n | pulse sweep-register value for note starts ($0378) | — |
| $9F,$A0 | — | no-op | — |
| $A1 | addr | jump (song loop point) | slot 18 pulse1 ends `A1 1F 8D` |
| $A2 | — | end channel (music); in an SFX stream it just parks the channel | slot 24 |
| $A3 | n | repeat body n times (per-channel stack) | slot 3: `A3 0F 02 B2 B1 04 A4` |
| $A4 | — | repeat end | |
| $A5 | addr | jump (alias of $A1; unused in data) | — |
| $A6 | n | set SFX duration, end pass (unused in data) | — |
| $A7 | n | SFX volume = n ($0330) | slot 3: `A7 0F` |
| $A8 | lo hi | SFX 16-bit period = operand (noise: lo → $400E) | `A8 F7 00` |
| $A9 | n | SFX duty index ($0338) | `A9 02` |
| $AA | n | direct write to $4001/pulse sweep | — |
| $AB | n | SFX volume += n | |
| $AC | n | SFX period += n (pitch slide) | slot 5: `01 AC 02` |
| $AD | — | SFX volume += 1 | |
| $AE | — | no-op | |
| $AF | — | SFX duty index += 1 | |
| $B0 | n | SFX volume -= n | slot 6: `A3 0D 01 B0 01 A4` |
| $B1 | n | SFX period -= n | |
| $B2 | — | SFX volume -= 1 (volume ramp-down loops) | |
| $B3-$B5,$B7,$B8,$BA | — | no-op | |
| $B6 | — | end SFX channel + silence ($8869) | every SFX stream |
| $B9 | r0 r1 r2 r3 dur | raw APU poke $4000-$4003+ch*4, then duration | slot 26: `B9 8A AB F4 09 32` |
| $BB | lo hi | tempo: 16-bit accumulator addend ($0306/$0307) | slot 18: `BB 38 C7` (~46.7 Hz) |
| $BC | n | register repeat start (single level) | `BC 02` |
| $BD | — | register repeat end | |
| $BE | addr | call subroutine (single level) | slot 19: `BE 03 91` |
| $BF | — | return; top-level = end channel | slot 19 noise tail `89 BF` |
| $C0 | — | no-op | |
| $C1 | n | signed detune added to note period ($035A) | `C1 02` |
| $C2 | — | no-op | |

## Slots

- 1-17, 25, 26: SFX (mask bit7). 1 and 2 point at one-byte end commands
  ($A2/$B6) — playing them just cuts the music (that *is* the death sting).
  25/26 share a 50-tick direct-write buzz ($8D04) and pause/resume the music
  tempo accumulator ($0305) around the cave-card screen.
- 18, 19, 35: standalone looping music (35 = title, 865 B, the largest).
- (20,21), (22,23), (27,28), (29,30), (31,32), (33,34): six world themes in
  normal/hurry pairs sharing all channel data except a 6-byte pulse1 prefix
  that sets a faster tempo for hurry (e.g. world 1: $AD70 vs $C4E0).
  Proven by ROM tables $BF07 (cave-start music per cave>>2) and $D060
  (hurry-up music at 30 time units left).
- 24: the only terminating music slot (triangle+noise, ~3.3 s) — cave-complete
  jingle, played at $AD1B right before the walk-out sequence.

## Intentional deviations from hardware

- Out-of-region reads return $BF (end); note indices clamp to the 96-entry
  PERIODS table with a warning (noise channels never use the table on real
  hardware either — the note nibble *is* the noise period).
- A top-level `$BF` ends the channel (the original would jump through stale
  RAM; no shipped song relies on that).
- Duty-animation indices wrap modulo 4 (the original can read past its
  4-byte table at $8563; $97 is unused in the data).
- Envelope/duty/sweep/length emulation is cycle-approximate, not bit-perfect.
