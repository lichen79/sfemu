# Design: Street Fighter 1 driver (sfemu sub-project F)

Date: 2026-08-17
Scope: Sub-project F of the sfemu arcade emulator
Status: draft
Depends on: A (68000 core), B (CPS-1 graphics), C (CPS-1 machine), D1 (Z80 core), D2 (YM2151), D3 (OKI + audio output), E1/E2/E3 (debugger, save states, graphics viewers)

**One sentence:** Add Capcom's 1987 Street Fighter board — a pre-CPS design with no CPS-A/CPS-B, three CPUs, ROM-resident tilemaps and two MSM5205s in stereo — as a second machine behind the same cores, frontend and save-state format the CPS-1 already uses.

---

## Context

The emulator runs Street Fighter II. It does not run Street Fighter I, and the
original request named both. Sub-project F is the second board.

A second board is not a feature; it is the test of every boundary A–E3 drew.
Everything the CPS-1 work called "the machine" is about to be asked whether it
meant *this* machine or *a* machine. Where the answer is "this one," F either
generalizes the seam or documents why the duplication is real. The list of
those seams is the bulk of this document, and it was produced by measurement,
not by guessing: 80 `Cps1` mentions across ~40 signatures in 7 frontend files,
51 `video::<module>::` references across 10 files, and five mono audio
signatures.

**SF1 is not a CPS-1 variant.** It is the board Capcom built before CPS-1, and
the driver is a different driver — `src/mame/capcom/sf.cpp` (© Olivier
Galibert, 1,428 lines), not `cps1.cpp`. There is no CPS-A register file, no
CPS-B register file, no graphics RAM, and no bank mapper. Palette is plain RAM
at `0xB00000` and I/O is plain decoding at `0xC00000`. Tile *maps* live in a
dedicated ROM region, not in RAM. Two of the three CPUs are Z80s, and the
second one has no RAM at all.

Two statements already committed to this repository are wrong, and this
document corrects them:

- `docs/superpowers/specs/2026-08-05-m68000-core-design.md:45` says SF1 has
  "dual YM2151". It has **one** YM2151 and **two** MSM5205, and the output is
  **stereo**.
- `crates/machine/src/board.rs:1220` says SF1 "has a different CPS-B row". SF1
  has no CPS-B at all, so it needs no `BoardConfig` whatsoever.

Both are corrected in place as part of F, not left for a reader to trip over.

### Which set

`sf` — "Street Fighter (US, set 1)", `GAME(1987, sf, 0, sfus, sfus, sf_state,
empty_init, ROT0, "Capcom", "Street Fighter (US, set 1)",
MACHINE_SUPPORTS_SAVE)` (`sf.cpp:1421`).

`sf` is the right first target for one decisive reason: it is the only
family whose machine config needs neither the i8751 protection MCU nor the
pneumatic-pedal input path. `sfus(config)` is two lines — `sfan(config);` plus
one `set_addrmap` — and its ROM set has no `protcpu` region. The `sfjp` family
(`sfua`, `sfj`, `sfjbl`, `sfw`) needs an 8751 core this repo does not have;
the `sfan` family (`sfjan`, `sfan`) needs the deluxe cabinet's punch/kick
pressure sensors. Both are out of scope, named explicitly below.

### The ROM rule, unchanged

No ROM is fetched, downloaded, bundled, committed, or embedded. SF1 is still
commercial Capcom code. `crates/romset` gains an `SF1` `GameSpec` holding
**file names, offsets, lengths and CRC-32s only** — the same rule
`crates/romset/src/games.rs:6` and `spec.rs:4-8` already state — and the user
supplies a MAME-format set at a path at runtime. Legal sources remain Capcom
Arcade Stadium, Capcom Fighting Collection, or dumping a board you own.

Missing data fails loudly, naming the file and what to do about it. No
environment-variable escape hatch, no `#[ignore]`. The three existing real-ROM
integration tests (`crates/sfemu/tests/{boot,sound_boot,audio_boot}.rs`) are
the only gated tests in the workspace and stay the only ones; F adds SF1
counterparts under the same single `SFEMU_ROMS` gate or adds none.

---

## What this is not

**Not a rewrite of the cores.** The 68000, Z80, YM2151 and OKI ADPCM decoder
are used as they are. Where SF1 needs something they do not do, the change is
additive and justified below, one item at a time.

**Not an i8751 emulator.** `sf`'s protection MCU does not exist in the `sf` set.
`protection_w` (`sf.cpp:273-318`) asserts `MCS51_INT0_LINE` and halts the
68000; `prot_p3_w` releases both; `prot_ram_r/w` window the MCU into the
68000's own program space. That is a fourth CPU core and a bidirectional
address-space bridge, and it buys exactly the Japanese sets. Out of scope.

**Not the pneumatic cabinet.** `sfan`'s `PUNCH` and `KICK` ports read pressure
pads. `sf` reads `nopr()` at both addresses. Out of scope.

**Not a new ADPCM decoder.** MAME's `msm5205.cpp` and `okiadpcm.cpp` compute
the same numbers by the same integer path; §"The MSM5205 is the OKI decoder"
proves it line by line. `oki::adpcm::Adpcm` is reused verbatim, including
`restore(signal, step)`, which already exists for exactly this purpose.

**Not a compressor.** MAME's output compressor is
`m_compressor_enabled(machine.options().compressor())`
(`emusound.cpp:1080`) — a user option, not board behaviour. sfemu does not
implement it, on SF1 or SF2.

**Not the `proms` region.** `ROM_START(sf)` declares a 0x320-byte `proms`
region of four files, and every one carries MAME's own comment
`/* unknown */`. Nothing in the driver reads it. It is not in the `GameSpec`.

**Not netplay, not WASM, not a shader.** Unchanged from A–E3.

---

## Verification: what proves this right

SF1 has no public cycle-exact test suite. The cores it runs on do, and they
already pass: SingleStepTests 127/127 (317,500 cases) for the 68000, 1,604/1,604
for the Z80, 1,000/1,000 against ymfm for the YM2151, 1,000/1,000 against
MAME's `okiadpcm` for the ADPCM decoder. F adds no new external suite because
none exists; it adds three other kinds of evidence.

**1. Transcription tests.** Every table copied out of `sf.cpp` gets a test that
re-states the MAME source's own numbers independently of the table under test.
This is `romset`'s existing discipline — `games.rs`'s `#[rustfmt::skip]` exists
because "the only way to check a transcription is to read it beside the MAME
source" — extended to the memory maps, the DIP layout, the gfx layouts and the
tile-info decode. A transcription test that reads the same constant the code
sets proves nothing; each one restates the source value as a literal.

**2. Derivation tests.** Where a number is computed rather than copied — element
counts from `RGN_FRAC`, cycles per line from the crystal, palette expansion —
the test asserts the derivation's *inputs* and the *result* as separate
literals, so a wrong formula cannot pass by matching itself. `timing.rs`'s
module doctrine states the failure mode being defended against: "an
`assert_eq!(a / b, a / b)` passes for every value of `a` and `b`, including
wrong ones."

**3. Golden frames from the guest, once a ROM is present.** The three
`SFEMU_ROMS` tests get SF1 siblings: the board boots, reaches a stable PC
distribution, writes a non-uniform palette, and emits non-silent samples. These
are the only tests that need the user's ROM, and they fail loudly naming
`SFEMU_ROMS` when it is absent.

**No test in this sub-project asserts an internal flag the code under test just
set.** The artifact is the frame, the sample, the decoded tile, the loaded
region — not the bit that says the frame was drawn.

---

## The measured facts

Everything in this section was read from MAME at tag `mame0261` (BSD-3) on
2026-08-16 and 2026-08-17. Line numbers are that tag's.

### The ROM set: eight regions

`ROM_START(sf)` (`sf.cpp:829-895`). Sizes are the literal
`ROM_REGION` sizes, which is what `romset::RegionSpec` records.

| Region | Size | Files | Load kind |
|---|---|---|---|
| `maincpu` | `0x60000` | 6 × `0x10000` | `Word16Byte` |
| `audiocpu` | `0x10000` | 1 × `0x8000` | `Byte` |
| `audio2` | `0x40000` | 2 × `0x20000` | `Byte` |
| `gfx1` | `0x80000` | 4 × `0x20000` | `Byte` |
| `gfx2` | `0x100000` | 8 × `0x20000` | `Byte` |
| `gfx3` | `0x1c0000` | 14 × `0x20000` | `Byte` |
| `gfx4` | `0x4000` | 1 × `0x4000` | `Byte` |
| `tilerom` | `0x40000` | 4 × `0x10000` | `Byte` |

Only two of `LoadKind`'s four variants are used: `Byte` and `Word16Byte`.
`Word64Word` and `Continue` are not.

`maincpu` is six `ROM_LOAD16_BYTE`s: `sfd-19.2a`/`sfd-22.2c` at `0x00000`/
`0x00001`, `sfd-20.3a`/`sfd-23.3c` at `0x20000`/`0x20001`,
`sfd-21.4a`/`sfd-24.4c` at `0x40000`/`0x40001`. Even offset is the high byte of
the big-endian word — `spec.rs`'s `Word16Byte` doc already warns that getting
this backwards byte-swaps every instruction word, and the CRC check catches a
mis-ordered *file*, not a mis-ordered *byte*, so this needs its own test.

Two size facts are load-bearing and easy to lose:

- `audiocpu`'s region is `0x10000` but its only file is `0x8000`. The upper
  half is whatever the region initialises to. The Z80's map only decodes
  `0x0000-0x7fff` as ROM, so nothing reads it — but the region must still be
  allocated at its declared size, because `RegionSpec` records the region, and
  a spec claiming `0x8000` would be a transcription error even though no test
  would notice.
- `audio2`'s region is `0x40000`, and `machine_start` configures **256 banks of
  `0x8000`** starting at `base() + 0x8000` (`sf.cpp:739`). 256 × `0x8000` is
  8 MB from a 256 KB region: only entries 0–6 are inside it. MAME's own
  configuration overruns. §"The second Z80" says what sfemu does instead.

### Three memory maps, one per region set

`sf.cpp:139-207`. All three call `map.unmap_value_high()`. Shared entries:

```
0x000000-0x04ffff  rom
0x800000-0x800fff  ram, w videoram_w, share "videoram"
0xb00000-0xb007ff  ram, w palette_device::write16, share "palette"
0xc00000  r IN0
0xc00002  r IN1
0xc00008  r DSW1
0xc0000a  r DSW2
0xc0000c  r SYSTEM
0xc0000e  nopr()
0xc00011  w coin_w        (byte)
0xc00014  w fg_scroll_w
0xc00018  w bg_scroll_w
0xc0001a  w gfxctrl_w
0xc0001d  w soundcmd_w    (byte)
0xff8000-0xffdfff  ram
0xffe000-0xffffff  ram, share "objectram"
```

Note `0x000000-0x04ffff` is `0x50000` bytes of a `0x60000` region: the top
`0x10000` of `maincpu` is not mapped by any of the three maps.

The differences are three addresses:

| | `0xc00004` | `0xc00006` | `0xc0001e` |
|---|---|---|---|
| `sfan_map` (:139) | `PUNCH` | `KICK` | — |
| `sfus_map` (:162) | `nopr()` | `nopr()` | — |
| `sfjp_map` (:185) | `IN2` | `nopr()` | `w protection_w` |

`sf` uses `sfus_map`. Its four `nopr()` windows are **not a distinct read
value**: `addrmap.h:137-139` sets `AMH_NOP`, and `memory.cpp:716-718` passes
`quiet = true` to `unmap_generic`. A `nopr()` read returns the map's unmap
value, which `unmap_value_high()` makes `0xFFFF` — exactly
`board.rs:51`'s `UNMAPPED: u16 = 0xFFFF`, whose doc argues that zero would be
"a dangerous one: `0x0000` decodes as a legal `ori.b #imm, d0`". SF1 inherits
that constant and that argument unchanged.

### Interrupts: autovector 0x64, not 0x68

`M68000(config, m_maincpu, XTAL(8'000'000))` with
`set_vblank_int("screen", FUNC(sf_state::irq1_line_hold))` (`sf.cpp:751-753`).

`sf.cpp` never calls `set_interrupt_mixer`, so the mixer is left at its
constructor default `true` (`m68kcommon.h:70`). With the mixer on,
`irq1_line_hold` → `set_input_line(1, HOLD_LINE)` (`driver.cpp:276`) is
interrupt **level 1**, and `autovector(level) = 0x18 + level`
(`m68kcommon.h:50`) gives vector number 25, at address **0x64**.

This is the one place SF1 and SF2 differ on the core's own interface. CPS-1
calls `set_interrupt_mixer(false)` (`cps1.cpp:3913`), so its IPL1 is *level 2*,
autovector 26, address `0x68` — which is what `board.rs:58`'s
`VEC_AUTOVECTOR_2 = 0x68` records, with that reasoning in its doc.

The core already generalizes: `m68k::exception::VEC_AUTOVECTOR_BASE: u8 = 24`
and `take(cpu, bus, VEC_AUTOVECTOR_BASE + level, pc)`, and `M68k::pending_irq`
is a *level* (`cpu.rs:142`) clamped by `set_irq` (`cpu.rs:433`). SF1 asserts
level 1 where CPS-1 asserts level 2, and the vector follows. No core change.

`execute_set_input` (`m68000.cpp:368-406`) picks the highest asserted level
when the mixer is on, and `update_interrupt` (:408-416) compares
`m_int_level > ((m_sr >> 8) & 7)` — a level-1 interrupt is masked at IPM 1 or
above, where CPS-1's level 2 is not. This is guest-visible and must be right.

For completeness: `sfp` (the prototype) uses `irq6_line_hold` → level 6 →
vector 30 → `0x78`. `sfp` is out of scope; the number is recorded so a later
reader does not have to re-derive it.

### Timing: nothing divides evenly, and the vblank period is zero

`sf.cpp:766-771`:

```
screen.set_refresh_hz(60);
screen.set_vblank_time(ATTOSECONDS_IN_USEC(0));
screen.set_size(64*8, 32*8);
screen.set_visarea(8*8, (64-8)*8-1, 2*8, 30*8-1);
```

Raster 512 × **256** (CPS-1's is 512 × 262). Visible area (64, 447, 16, 239) =
**384 × 224 at (64, 16)** — identical to CPS-1, so `video::WIDTH 384`,
`HEIGHT 224`, `VISIBLE_X 64`, `VISIBLE_Y 16` (`video/src/lib.rs:75-82`)
transfer unchanged.

`set_vblank_time` sets `m_oldstyle_vblank_supplied = true` (`screen.h:272`), so
`configure` (`screen.cpp:1001-1005`) takes `m_vblank_period = m_vblank` = **0**
rather than deriving it from the non-visible line count. The frame period is
the nominal 60 Hz `set_refresh_hz` asked for; `m_scantime = frame_period /
height` (:997).

**This is the board `cps1_10mhz`'s doc warned about.** `Timing::cps1_10mhz`
says: "A board whose clocks did not divide evenly would need a fractional
accumulator here, and `cps1_frame_geometry_is_384x224_at_59_63_hz` asserts the
two remainders are zero so that a future board needing one cannot be added
without noticing." SF1 is that board, and the arithmetic must be stated
carefully because the tempting version of it is wrong.

The line rate is the *frame* rate times the line count — 60 × 256 = **15,360**
lines per second — because the frame period comes from `set_refresh_hz(60)`,
not from the pixel clock. Therefore:

```
68000 cycles per line = 8,000,000 / 15,360 = 3125 / 6 = 520.833...
68000 cycles per frame = 8,000,000 / 60    = 400000 / 3 = 133,333.333...
```

**Neither is an integer, and neither is 512.** The number 512 is the raster
*width* in pixels, and it would be the cycles per line only if the 68000's
8 MHz crystal were also the dot clock — which would make the refresh
8,000,000 / (512 × 256) = 61.035 Hz, not the 60 Hz MAME configures. The two are
independent: MAME derives the frame period from `set_refresh_hz` and the CPU's
cycle budget from `XTAL(8'000'000)`, and they are not commensurate.

CPS-1 got its exact 640 cycles per line from the *opposite* arrangement: its
refresh is *derived* from the pixel clock (`8,000,000 / (512 × 262)` ≈
59.637 Hz), so its 10 MHz CPU divides its 15,625 Hz line rate exactly.
SF1's refresh is *asserted* as a round 60, so nothing divides.

`Timing`'s four `u32` fields (`timing.rs:192-203`) cannot express 3125/6.
Rather than round `cycles_per_line` to 520 or 521 — a 0.16% error, which is
precisely the silent-drift failure `timing.rs`'s module doctrine describes:
"A frame that is 639 cycles per line instead of 640 runs 0.16% slow: music
drifts against animation over a match, and nothing ever looks broken enough to
investigate" — `Timing` gains a **fifth field**: a `RationalAccumulator` for the
68000's cycles per line, and `cycles_per_line` becomes the accumulator's
`advance()` rather than a constant. CPS-1's constructor builds a
`RationalAccumulator::new(640, 1)`, which advances by exactly 640 every line and
is arithmetically identical to today's constant, so the CPS-1 path is unchanged
in behaviour while gaining one shared code path with SF1.

`Timing::sf1_8mhz()` is then `cpu_hz: 8_000_000`, cycles per line
`RationalAccumulator::new(8_000_000, 15_360)` (= 3125/6 after reduction),
`lines_per_frame: 256`, `vblank_line: 240`. The test asserts the reduced
fraction as literals *and* that 256 advances sum to 133,333 with remainder 1/3
carried — the sum being the assertion that catches a wrong reduction, because a
per-line assertion cannot.

Making `cycles_per_line` fractional touches `Cps1`'s scheduler: `carry`
(`cps1.rs`, the private `i64`) is granted `cycles_per_line` at each line start,
and that grant becomes `advance()`. The remainder is state — it must join the
save state, exactly as `z80_carry`'s already does, and
`RationalAccumulator::with_remainder` exists for this. A restored machine that
zeroed it would be up to five cycles out per line and permanently out of step,
which is the same argument `with_remainder`'s own doc makes.

`vblank_line = 240` is `VBSTART`: the first line past the visible area, which is
where MAME's `set_vblank_int` fires. Because SF1's vblank *period* is zero, the
interrupt is a single edge at that line and nothing holds it for a span; the
existing `assert_vblank` / `vblank_pending` / `note_possible_ack` shape
(`board.rs:170-215`) is already an edge-plus-latch model, so this fits.

The frame period is **not** CPS-1's: `frontend::pace::FRAME_NS = 16_768_000`
(59.637 Hz) is CPS-1's derived rate. SF1's is nominal 60 Hz = 16,666,667 ns.
The host pacer must take the period from the board, not from a module constant.

### Audio clocks

`Z80(config, m_audiocpu, XTAL(3'579'545))` and
`Z80(config, "audio2", XTAL(3'579'545))` (`sf.cpp:756-762`) — both audio CPUs
run at the same crystal as CPS-1's single Z80. `timing.rs`'s
`SOUND_XTAL 3_579_545` therefore transfers, but `Z80_T_NUM 715_909` /
`Z80_T_DEN 3_125` do **not**: that fraction is T-states per line at *CPS-1's*
15,625 Hz line rate. At SF1's 15,360 Hz the fraction is

```
3,579,545 / 15,360 = 715,909 / 3,072 = 233.04...  T-states per line
```

— the same numerator after reduction, a different denominator, and a materially
different number (233.04 against CPS-1's 229.09). Two boards sharing a Z80
crystal do not share a T-states-per-line count, and the shared numerator makes
this an easy transcription to get wrong by copying the wrong constant.

The ADPCM interrupt is the third fraction. `audio2` takes
`set_periodic_int(FUNC(sf_state::irq0_line_hold), attotime::from_hz(8000))` —
MAME's own comment on that line is `// ?`. Eight thousand IRQ0s per second,
which is what actually paces ADPCM playback (see below):

```
8,000 / 15,360 = 25 / 48 interrupts per line
```

All three are `RationalAccumulator`s (`timing.rs:114`), and all three
remainders are save-state fields — `with_remainder` (:156) exists precisely so
a codec outside the crate can restore them. The uncertainty in MAME's `// ?`
is carried into the code as a comment rather than smoothed over.

### The first Z80: not CPS-1's sound board

`sound_map` (`sf.cpp:209-216`):

```
0x0000-0x7fff  rom
0xc000-0xc7ff  ram
0xc800         r soundlatch
0xe000-0xe001  rw ym2151
```

CPS-1's `SoundBoard` decodes ROM `0x0000-0xBFFF`, RAM `0xD000-0xD7FF`,
`0xF000/0xF001` YM, `0xF002` OKI, `0xF004` bank, `0xF006` pin 7,
`0xF008`/`0xF00A` latches, and answers `0xFF` unmapped. **Not one address
matches.** There is no ROM bank, no OKI, no pin-7 register, and one latch
rather than two. The map is new code; the Z80 core underneath is not.

`soundcmd_w` (`sf.cpp:118-122`) is
`m_soundlatch->write(data & 0xff); m_audiocpu->pulse_input_line(INPUT_LINE_NMI,
attotime::zero);` — an **NMI pulse**, where CPS-1's Z80 polls the latch. The
Z80 core already has what this needs: a public edge-triggered `nmi: bool`
(`cpu.rs:159`), `service` preferring NMI over IRQ and leaving a refused IRQ
pending (`interrupt.rs:23`), and `ack_nmi` (`interrupt.rs:36`) clearing `nmi`,
leaving halt, saving `iff1` into `iff2`, clearing `iff1` and `ei`, pushing PC,
jumping to `0x0066` and charging 11 T-states. No core change.

`ymsnd.irq_handler().set_inputline(m_audiocpu, 0)` (`sf.cpp:781`) wires the
YM2151's IRQ to this Z80's IRQ line — the same wiring CPS-1 has, and
`Cps1::step_sound` already does `z80.irq = sound.ym_ref().irq()`.

### The second Z80: no RAM, 256 banks, and a port that is both

`sound2_map` (`sf.cpp:217-223`), with MAME's own comments:

```
0x0000-0x7fff  rom
0x8000-0xffff  bankr "audiobank"
0x0000-0xffff  nopw()          /* Yes, _no_ ram */
```

The third entry overlays the first two for writes only, so every write anywhere
is discarded — MAME's comment says it is there to "avoid cluttering up
error.log", but the behaviour is the board's: there is no writable memory on
this CPU at all. A Z80 program with no RAM cannot use `call`, `push`, or any
stack operation meaningfully; sfemu must model the discard, not assert on it.

`sound2_io_map` (`sf.cpp:225-232`) with `map.global_mask(0xff)`:

```
0x00  w msm_w<0>
0x01  w msm_w<1>
0x01  r soundlatch
0x02  w sound2_bank_w
```

Port `0x01` is a write to the second MSM5205 *and* a read of the sound latch —
the same port number in both directions, which the Z80 core's split
`port_in`/`port_out` on its `Bus` trait handles naturally. Both audio CPUs read
the *same* `m_soundlatch`; there is only one latch device in the config
(`GENERIC_LATCH_8(config, m_soundlatch)`, `sf.cpp:780`).

`sound2_bank_w` is `m_audiobank->set_entry(data);` — the full byte, no mask, and
`machine_start` configured 256 entries of `0x8000` from `base() + 0x8000` of a
`0x40000` region. Entries 0–6 are in range; 7–255 are past the end of the
region. sfemu will not read out of bounds: the bank offset is
`0x8000 + entry * 0x8000` **masked into the region** so an out-of-range entry
aliases within `audio2` rather than reading foreign memory or panicking. That
choice is a divergence from MAME's undefined behaviour and is recorded as such;
the guest is not expected to select those banks, and if it does, a
deterministic alias is the only defensible answer. A `Trace` counter records
selections above 6 so the debugger can show whether it ever happens.

### One YM2151, two MSM5205, and stereo

`sf.cpp:778-796`:

```
SPEAKER(config, "lspeaker").front_left();
SPEAKER(config, "rspeaker").front_right();
GENERIC_LATCH_8(config, m_soundlatch);
ym2151_device &ymsnd(YM2151(config, "ymsnd", XTAL(3'579'545)));
ymsnd.irq_handler().set_inputline(m_audiocpu, 0);
ymsnd.add_route(0, "lspeaker", 0.60);
ymsnd.add_route(1, "rspeaker", 0.60);
MSM5205(config, m_msm[0], 384000);
m_msm[0]->set_prescaler_selector(msm5205_device::SEX_4B);   /* 8KHz playback ? */
m_msm[0]->add_route(ALL_OUTPUTS, "lspeaker", 1.0);
m_msm[0]->add_route(ALL_OUTPUTS, "rspeaker", 1.0);
MSM5205(config, m_msm[1], 384000);
... same, both speakers ...
```

The YM2151's two channels go to opposite speakers at 0.60; both MSM5205s go to
both speakers at 1.0. So:

```
left  = 0.60 * ym_l + 1.0 * msm0 + 1.0 * msm1
right = 0.60 * ym_r + 1.0 * msm0 + 1.0 * msm1
```

`ym2151::Ym2151::generate(&mut self, out: &mut [(i16, i16)])`
(`ym2151/src/chip.rs:349`) already produces stereo pairs, so the YM side needs
no change.

**Where the clamp comes from.** `speaker_device::mix` (`speaker.cpp:89-146`)
applies the pan and **does not clamp**. The clamp is in the final downmix,
`emusound.cpp:1598-1632`, which clamps each side to ±1.0 and then multiplies by
32767.0, interleaved L, R. SF1's mix must therefore **saturate each side
explicitly**.

This is the opposite of `machine::cps1::mix` (`cps1.rs:616`):

```rust
pub fn mix(ym_l: i16, ym_r: i16, oki_2x: i32) -> i16 {
    let numerator = 7 * (i32::from(ym_l) + i32::from(ym_r)) + 3 * oki_2x;
    (numerator / 20) as i16
}
```

whose doc proves saturation unnecessary *for CPS-1* because the OKI clamps to
±`oki::chip::CLAMP_2X` (65,536) first, bounding the numerator at ±655,360 =
20 × 32,768. That argument does not survive the change of coefficients or the
second ADPCM chip, so SF1's mix is a separate function with an explicit
`saturating` path per side, and its own test asserting that a full-scale YM
plus two full-scale MSMs pins at ±32,767 rather than wrapping.

The 0.60 and 1.0 route gains become an integer ratio in the same style as
`cps1::mix`'s 7/3/20, chosen so the divide is exact and the maximum is
reachable, with the chosen integers and the resulting worst case both asserted
as literals.

### The MSM5205 is the OKI decoder

MAME's `compute_tables` (`msm5205.cpp:139-168`):

```
stepval = floor(16.0 * pow(11.0/10.0, step))        for step = 0..=48
m_diff_lookup[step*16 + nib] = nbl2bit[nib][0] *
    (stepval * nbl2bit[nib][1] + stepval/2 * nbl2bit[nib][2] +
     stepval/4 * nbl2bit[nib][3] + stepval/8)
index_shift[8] = {-1,-1,-1,-1, 2, 4, 6, 8}          (:133)
```

`crates/oki/src/adpcm.rs` is the same arithmetic: `STEPS: usize = 49` (:9),
`STEP_TABLE: [i16; STEPS]` (:18) holding the 49 literals 16…1552 — with the doc
recording why they are literals ("Rust has no `const fn` float `pow`, and the
exact integer form `16 * 11^48 / 10^48` needs 171 bits"), `SIGNAL_MAX 2047`
(:25), `SIGNAL_MIN -2048` (:27), `INDEX_SHIFT: [i8; 8] = [-1,-1,-1,-1,2,4,6,8]`
(:32), and `diff(step, nibble)` (:49) doing `let mut d = sv / 8;` then
`+= sv` / `+= sv/2` / `+= sv/4` per bit with the sign from bit 3, noted as
"Each division truncates independently, exactly as MAME's does".

Same table, same clamps, same per-term truncation. `oki::adpcm::Adpcm` is
reused verbatim — including `restore(signal, step)` (:89), which clamps both
inputs precisely because "the values come from a save-state file, and an
out-of-range step index would panic in `diff`". That is exactly what SF1's save
state needs, for two chips.

Only the **wrapper** is new, and these are its parts:

- `data_w` (`msm5205.cpp:254-260`): `if (m_bitwidth == 4) m_data = data & 0x0f;
  else m_data = (data & 0x07) << 1;`. `SEX_4B` sets bitwidth 4
  (`set_prescaler_selector` is `m_s1 = BIT(select,1); m_s2 = BIT(select,0);
  m_bitwidth = (select & 4) ? 4 : 3;`, and `SEX_4B = 7`, `msm5205.h:21`), so SF1
  takes the `& 0x0f` path — one nibble per port write, low nibble only. The high
  nibble of the byte the Z80 writes is discarded except for bit 7.
- `msm_w` (`sf.cpp:130-137`):
  `m_msm[Chip]->reset_w(BIT(data,7)); /* ?? bit 6?? */ m_msm[Chip]->data_w(data);
  m_msm[Chip]->vclk_w(1); m_msm[Chip]->vclk_w(0);` — bit 7 is reset, bits 0–3
  are the nibble, and the write manually toggles VCK high then low. MAME's own
  `?? bit 6??` is preserved as a comment.
- **Slave VCK.** `get_prescaler()` (`msm5205.cpp:262-268`) is
  `if (m_s1) return m_s2 ? 0 : 64; else return m_s2 ? 48 : 96;`. `SEX_4B` sets
  both bits, so the prescaler is **0**, and `device_clock_changed` responds by
  setting the stream rate to `clock()` (384 kHz) and the VCK timer to
  `attotime::never`. The chip never clocks itself; the Z80's port writes clock
  it. `vclk_w` (:229-239) logs an error unless the prescaler is 0, and on the
  **falling** edge (`m_vck && !state`) schedules the capture at
  `attotime::from_hz(clock()/6)` = 64 kHz = 15.625 µs later — a delay, not an
  immediate decode (`adpcm_capture_divisor()` returns 6.0).
- `update_adpcm` (:184-221): a reset branch setting `new_signal = 0; m_step = 0;`,
  otherwise the `diff` add with clamps at 2047/−2048 and
  `m_step += index_shift[val & 7]` clamped to 0..=48, and a stream update only
  when the signal changed.
- **10-bit DAC.** `sound_stream_update` (:350-364) computes
  `dac_mask = (m_dac_bits >= 12) ? 0 : (1 << (12 - m_dac_bits)) - 1`, and
  `m_dac_bits` is 10 for MSM5205, so `dac_mask = 3`: the output is
  `(m_signal & !3) / 4096.0`, i.e. the low two bits of the 12-bit signal are
  masked off. It emits **silence when `m_signal == 0`** rather than the masked
  value, which is the same number but makes the reset state explicit.
- `device_reset` (:117-125) zeroes `m_data, m_vck, m_reset, m_signal, m_step` and
  **not** the selector fields — a reset does not undo `set_prescaler_selector`.
- `device_start` saves `m_data, m_vck, m_reset, m_s1, m_s2, m_bitwidth,
  m_signal, m_step`, which is the save-state field list, minus the selector
  fields sfemu fixes at construction.

Because the decode is scheduled 15.625 µs after the falling edge rather than
performed inline, a port write and its audible effect are separated by ~56 Z80
T-states. sfemu models this with a per-chip pending-nibble timer in the same
`RationalAccumulator` idiom, rather than decoding on write: the difference is
audible as a fixed group delay only, but the *ordering* of a reset against a
pending nibble is not, and getting that wrong makes reset clicks land on the
wrong sample.

### Palette: plain 4-4-4, no brightness

`PALETTE(config, m_palette).set_format(palette_device::xRGB_444, 1024)`
(`sf.cpp:775`). `emupal.h:213` declares `xRGB_444` as `xxxxRRRRGGGGBBBB`;
`emupal.cpp:171-175` resolves it to
`set_format(2, &raw_to_rgb_converter::standard_rgb_decoder<4,4,4, 8,4,0>,
entries)`; that decoder (`emupal.h:130-136`) is
`u8 const r = palexpand<RedBits>(raw >> RedShift);` per channel; and
`palexpand<4>` (`palette.h:236`) is `bits &= 0xf; return (bits << 4) | bits;`.

So an SF1 palette entry is:

```
r = expand4((e >> 8) & 0xF)     g = expand4((e >> 4) & 0xF)
b = expand4(e & 0xF)            expand4(n) = (n << 4) | n
```

with **no brightness term**. `video::palette::entry_to_rgb` (`palette.rs:46`)
computes `bright = 0x0F + ((e >> 12) << 1)` and
`nibble * 0x11 * bright / 0x2D` — that is CPS-1's format and does not transfer.
`build_palette` (`palette.rs:66`) is CPS-1-only too: `PAGES 6`,
`PAGE_ENTRIES 0x200`, `PENS 0xC00`, `BACKGROUND_PEN 0xBFF`. SF1 has 1,024 flat
entries, palette RAM is `0x800` bytes at `0xB00000`, and the write handler is
`palette_device::write16` (`emupal.cpp:405-409`) = store then
`update_for_write(offset*2, 2)`. No pages, no background pen.

Note `expand4(n) == n * 0x11`, so the *arithmetic* overlaps CPS-1's at
`bright == BRIGHT_MAX`. The functions are still separate: sharing them would
make SF1's correctness depend on CPS-1's brightness default staying at
maximum, which is a coupling no test would catch.

### Graphics: four gfx regions, MSB-first, fractions in bits

Layouts (`sf.cpp:701-722`):

```
char_layout   = {8,8,  RGN_FRAC(1,1), 2, {4,0},
                 {STEP4(0,1), STEP4(4*2,1)}, {STEP8(0,1*16)}, 16*8}
sprite_layout = {16,16, RGN_FRAC(1,2), 4, {4,0, RGN_FRAC(1,2)+4, RGN_FRAC(1,2)},
                 {STEP4(0,1), STEP4(4*2,1), STEP4(4*2*2*16,1), STEP4(4*2*2*16+8,1)},
                 {STEP16(0,1*16)}, 64*8}
```

`GFXDECODE_START(gfx_sf)` (`sf.cpp:724-729`):

| gfx index | region | layout | colour base | colours |
|---|---|---|---|---|
| 0 | `gfx1` | `sprite_layout` | 0 | 16 |
| 1 | `gfx2` | `sprite_layout` | 256 | 16 |
| 2 | `gfx3` | `sprite_layout` | 512 | 16 |
| 3 | `gfx4` | `char_layout` | 768 | 16 |

**`RGN_FRAC` resolves in bits, not bytes.** `digfx.cpp:149` sets
`region_length = 8 * region->bytes()`; `:185-186` computes
`glcopy.total = region_length / glcopy.charincrement * FRAC_NUM / FRAC_DEN`;
`:223` resolves plane offsets as `FRAC_OFFSET(v) + region_length * num/den`.
For SF1:

| region | bytes | bits | charincrement | fraction | elements |
|---|---|---|---|---|---|
| `gfx1` | `0x80000` | 4,194,304 | 512 | 1/2 | 4,096 |
| `gfx2` | `0x100000` | 8,388,608 | 512 | 1/2 | 8,192 |
| `gfx3` | `0x1c0000` | 14,680,064 | 512 | 1/2 | 14,336 |
| `gfx4` | `0x4000` | 131,072 | 128 | 1/1 | 1,024 |

`charincrement` is `64*8 = 512` bits for the sprite layout and `16*8 = 128` for
the char layout, and `RGN_FRAC(1,2)` then halves the sprite counts — a 16×16
4-plane tile draws two of its planes from the region's first half and two from
its second, so the region holds half as many tiles as its raw bit count
suggests. Sprite plane offsets are `{4, 0, half+4, half}` in bits, where
`half = region_length / 2`.

MAME's own comments in `ROM_START` confirm the split: `gfx1`'s first two files
are marked "Background b planes 0-1" and its second two "planes 2-3"; `gfx2`'s
first four are "Background m planes 0-1" and its second four "planes 2-3". Both
splits land exactly at the region's midpoint, which is the independent check
that `half` is `region_length / 2` and not something else. Every region's size
is the exact sum of its files (4 × `0x20000` = `0x80000`, 14 × `0x20000` =
`0x1c0000`, and so on), so there are no gaps for the fraction to land in.

**Bit order is MSB-first.** `readbit(src, bitnum)` (`drawgfx.cpp:24`) is
`src[bitnum / 8] & (0x80 >> (bitnum % 8))`. `gfx_element::decode`
(`drawgfx.cpp:289-318`) walks planes with `planebit = 1 << (m_layout_planes - 1)`
descending, computes `planeoffs = code * m_layout_charincrement +
m_layout_planeoffset[plane]`, and tests
`readbit(m_srcdata, (yoffs + m_layout_xoffset[x]) ^ m_layout_xormask)`.
**`m_layout_xormask` is 0 for SF1**: `digfx.cpp:125` sets it from
`GFXENTRY_ISREVERSE` (not set here) and otherwise ORs 0x08/0x18/0x38 only for
2/4/8-byte-wide regions — SF1's are all byte-wide.

`video::tiles::tile_pen` (`tiles.rs:97`) implements CPS-1's fixed 4-bpp
interleave with `TRANSPARENT_PEN: u8 = 0x0F` and a `TileKind` enum. It does not
transfer: SF1 needs a 2-plane 8×8 layout *and* a 4-plane 16×16 layout with
half-region plane offsets, and the transparent pen differs per layer (15 for
sprites and fg, **3** for tx). F implements a layout-driven decoder — plane
offsets, x-offsets, y-offsets and charincrement as data — because that is what
`gfx_layout` is, and hardcoding two more special cases is how the third board
becomes impossible.

**Palette base is one formula, shared by tiles and sprites.**
`gfx_element::transpen` does `color = colorbase() + granularity() * (color %
colors());`, and `tile_data::set` (`tilemap.h:386-394`) does
`palette_base = gfx->colorbase() + gfx->granularity() * (rawcolor % gfx->colors());`.
`set_layout` sets `m_color_depth = m_color_granularity = 1 << gl.planes`
(`drawgfx.cpp:145`) — **granularity 16** for the sprite layout, **4** for the
char layout. With `m_total_colors = 16` and bases 0/256/512/768, a tx tile's
colour `c` lands at `768 + 4 * (c % 16)`, spanning 768..831 — the char layout
uses only 64 of its 256 reserved entries, because 2 planes need only 4 pens.
That asymmetry is real and is exactly the kind of thing a hardcoded
`PEN_GRANULARITY = 16` (`layers.rs`) gets wrong.

**Sprite blit.** `PIXEL_OP_REBASE_TRANSPEN` (`drawgfxt.ipp:208-214`) is
`u32 srcdata = (SOURCE); if (srcdata != trans_pen) (DEST) = color + srcdata;`.
`drawgfx_core` (`drawgfxt.ipp:421-554`) clips **before** it flips: it computes
`destendx = destx + width() - 1`, applies the cliprect to `srcx`/`destx`, and
only then does `if (flipx) srcx = width() - 1 - srcx;` and
`s32 dy = rowbytes(); if (flipy) { srcy = height() - 1 - srcy; dy = -dy; }`.
Getting that order backwards mirrors a partially-clipped sprite about the wrong
axis, which is invisible until a character walks off the screen edge.

### Tilemaps: three of them, and the maps live in ROM

`video_start` (`sf.cpp:263-271`):

```
bg: TILEMAP_SCAN_COLS, 16, 16, 2048, 16
fg: TILEMAP_SCAN_COLS, 16, 16, 2048, 16
tx: TILEMAP_SCAN_ROWS,  8,  8,   64, 32
m_fg_tilemap->set_transparent_pen(15);
m_tx_tilemap->set_transparent_pen(3);
```

`scan_rows(col,row,nc,nr) = row*nc + col`; `scan_cols = col*nr + row`. bg is
32,768 pixels wide and 256 tall; fg the same; tx is the 512×256 raster exactly.

**bg and fg tile info comes from `tilerom`, not from RAM** (`sf.cpp:239-262`):

```
bg:  base = &m_tilerom[2 * tile_index];
     attr  = base[0x10000];
     color = base[0];
     code  = (base[0x10000 + 1] << 8) | base[1];
     tileinfo.set(0, code, color, TILE_FLIPYX(attr & 3));
fg:  identical, with base = &m_tilerom[0x20000 + 2 * tile_index], set(1, ...)
tx:  code = m_videoram[tile_index];
     tileinfo.set(3, code & 0x3ff, code >> 12, TILE_FLIPYX((code & 0xc00) >> 10));
```

So bg uses gfx 0 (`gfx1`), fg uses gfx 1 (`gfx2`), tx uses gfx 3 (`gfx4`), and
sprites use gfx 2 (`gfx3`) exclusively. bg's map is `tilerom[0x00000..0x10000]`
paired with `tilerom[0x10000..0x20000]`; fg's is `[0x20000..0x30000]` paired
with `[0x30000..0x40000]`. The whole `0x40000` region is used, as two maps of
two byte-planes each. 2,048 × 16 = 32,768 tiles × 2 bytes = `0x10000` — the map
size and the plane stride agree exactly, which is the check that the split is
right.

`m_videoram` is `0x1000` bytes at `0x800000` = 2,048 words for 64 × 32 = 2,048
tx tiles. `videoram_w` is `COMBINE_DATA(&m_videoram[offset]);
m_tx_tilemap->mark_tile_dirty(offset);`.

**Scrolling is X-only, one value per map.** `fg_scroll_w` is
`COMBINE_DATA(&m_fgscroll); m_fg_tilemap->set_scrollx(0, m_fgscroll);` and
`bg_scroll_w` the same for bg. There is no Y scroll register and no row-scroll
table. tx never scrolls.

`effective_rowscroll` (`tilemap.cpp:26-74`) unflipped is
`value = m_dx - m_rowscroll[index]`, then
`if (value < 0) value = m_width - (-value) % m_width; else value %= m_width;`.
**`m_dx` and `m_dx_flipped` are both 0 for SF1** — `tilemap.cpp:394-395`
initialises them to zero and `sf.cpp` never calls `set_scrolldx`. So the
effective scroll reduces to `(-scroll) mod width`, and F asserts that
reduction rather than reimplementing the general form.

**Transparency is a flags map, not a pen comparison.**
`set_transparent_pen(pen)` (`tilemap.cpp:557-564`) is two calls:
`map_pens_to_layer(0, 0, 0, TILEMAP_PIXEL_LAYER0)` then
`map_pen_to_layer(0, pen, TILEMAP_PIXEL_TRANSPARENT)`. `tile_draw`
(`tilemap.cpp:~840-877`) writes, per pixel, `pixptr[xoffs] = palette_base + pen;
flagsptr[xoffs] = penmap[pen] | category;`. `screen_update` calls
`draw(screen, bitmap, cliprect, 0, 0)` — flags 0, priority 0 — and
`configure_blit_parameters` sets `blit.mask = TILEMAP_PIXEL_CATEGORY_MASK
(0x0f); blit.value = 0;` then forces `flags |= TILEMAP_DRAW_LAYER0` and ORs
that into both. The blit is `scanline_draw_masked_ind16`
(`tilemap.cpp:188-210`):

```
for (int i = 0; i < count; i++)
    if ((maskptr[i] & mask) == value) dest[i] = source[i] + pal;
```

A pixel draws iff `(flags & (0x0f | 0x10)) == 0x10`. Because bg never calls
`set_transparent_pen`, every bg pixel's flags byte is `TILEMAP_PIXEL_LAYER0` and
**bg is fully opaque** — including its pen 15. fg is transparent at pen 15, tx
at pen 3. Composing SF1 by comparing pens against a single `TRANSPARENT_PEN`
constant would draw bg's pen-15 pixels as holes.

The wraparound is `draw_common`'s double loop: `for (ypos = scrolly - m_height;
...; ypos += m_height) for (xpos = scrollx - m_width; ...; xpos += m_width)
draw_instance(...)`, with `xextent = visarea.right() + visarea.left() + 1` and
`yextent = visarea.bottom() + visarea.top() + 1`. For SF1's visarea
(64, 447, 16, 239) that is `447 + 64 + 1 = 512` and `239 + 16 + 1 = 256` — the
raster exactly. `draw_common` also returns immediately when `!m_enable`.

### Layer enables and screen flip

`gfxctrl_w` (`sf.cpp:338-355`), with MAME's bit notes:

```
if (ACCESSING_BITS_0_7) {
    m_active = data & 0xff;
    flip_screen_set(data & 0x04);
    m_tx_tilemap->enable(data & 0x08);   // b3 character plane
    m_bg_tilemap->enable(data & 0x20);   // b5 background plane
    m_fg_tilemap->enable(data & 0x40);   // b6 middle plane
}
// b0 reset, b1 pulsed, b2 flip, b4 unused, b7 sprites
```

`screen_update` (`sf.cpp:453-467`):

```
if (m_active & 0x20) m_bg_tilemap->draw(screen, bitmap, cliprect, 0, 0);
else bitmap.fill(0, cliprect);
m_fg_tilemap->draw(screen, bitmap, cliprect, 0, 0);
if (m_active & 0x80) draw_sprites(bitmap, cliprect);
m_tx_tilemap->draw(screen, bitmap, cliprect, 0, 0);
```

Fixed order, no priority resolution at all: bg, fg, sprites, tx. Note bit 5 is
tested **twice** — once as `enable()` and once here, where a disabled bg fills
the bitmap with pen 0 instead. Bits 3 and 6 are tested only via `enable()`, and
`draw_common`'s `!m_enable` early-out is what makes them work. Bit 7 is tested
only here. This means SF1's compositor has no equivalent of CPS-1's
`layer_order(layercontrol)` / `LayerMask` / four `DEPTHS` priority machinery
(`compose.rs`), and `video::compose` does not transfer.

**Screen flip is three composed mechanisms.** `flip_screen_set`
(`driver.cpp:317-329`) normalizes to 0xff, sets both axes, and calls
`updateflip` → `machine().tilemap().set_flip_all(...)` (`driver.cpp:306-310`)
→ `tmap.set_flip(attributes)` for every tilemap (`tilemap.h:475`), which on
change calls `mappings_update()`. That remaps every logical index
(`tilemap.cpp:715-741`): `if (FLIPX) logical_col = (m_cols-1) - logical_col;
if (FLIPY) logical_row = (m_rows-1) - logical_row;` and ends with
`mark_all_dirty()`. Then `tile_update` (`tilemap.cpp:805`) computes
`u32 flags = m_tileinfo.flags ^ (m_attributes & 0x03);` so each tile's own flip
bits invert, and `tile_draw` walks the tile backwards
(`y0 += m_tileheight - 1; dy0 = -1;` and the x mirror). And
`effective_rowscroll`'s flipped branch is
`value = screen_width - m_width - (m_dx_flipped - m_rowscroll[index])`.

`video::compose::Framebuffer::flip` (`compose.rs:139-142`) is
`self.pens.reverse(); self.prio.reverse();` — a single mirror of the finished
visible frame, justified in `video/src/lib.rs`'s module doc by the visible
window being symmetric within the raster pivots. That argument holds for SF1
too — `xextent = 512`, `yextent = 256`, both exactly the raster, and the
visible window (64..447, 16..239) is symmetric within them — but it is a
*different* argument about a *different* geometry, so F re-derives it as a test
with the four numbers as literals rather than inheriting the conclusion.

Sprites are flipped by `draw_sprites`, not by the tilemap machinery, so the
whole-frame mirror is not automatically equivalent for them; see below.

`m_flip_screen_x` and `m_flip_screen_y` are saved (`driver.cpp:229-230`), so
flip is save-state state, not a derived value.

### Sprites: backwards, quads, and an inverted code

`draw_sprites` (`sf.cpp:365-450`) walks `objectram` **backwards** in `0x20`-word
strides from `0x1000 - 0x20` down to 0 — so lower addresses draw last, i.e. on
top. Per entry:

```
c    = m_objectram[offs];      attr = m_objectram[offs + 1];
sy   = m_objectram[offs + 2];  sx   = m_objectram[offs + 3];
color = attr & 0x000f;  flipx = attr & 0x0100;  flipy = attr & 0x0200;
large = attr & 0x0400;
```

A large sprite is a 32×32 quad of codes `c, c+1, c+16, c+17` at
`(sx, sy)`, `(sx+16, sy)`, `(sx, sy+16)`, `(sx+16, sy+16)`; `flipx` swaps
c1↔c2 and c3↔c4, `flipy` swaps c1↔c3 and c2↔c4. Screen flip pivots large
sprites at `sx = 480 - sx, sy = 224 - sy` and small ones at
`sx = 496 - sx, sy = 240 - sy`, negating both flip flags. Every draw is
`m_gfxdecode->gfx(2)->transpen(bitmap, cliprect, invert(cN), color, flipx,
flipy, sx, sy, 15)`.

The code passes through `invert`:

```
static const int delta[4] = { 0x00, 0x18, 0x18, 0x00 };
invert(nb) = nb ^ delta[(nb >> 3) & 3];
```

An XOR of bits 3 and 4 into the code, conditioned on those same two bits — a
ROM address scramble on the board, and a four-entry table that must be
transcribed rather than reasoned about.

`objectram` is `0x2000` bytes at `0xFFE000` = `0x1000` words, addressed as
`0x1000` in the loop, so the whole region is scanned as 128 entries of 8 words
of which 4 are used. `video::sprites::ObjLatch` (`OBJ_WORDS 0x400`) is CPS-1's
`0x400`-word latch with a `last_offset` terminator; SF1's is `0x1000` words,
strided, unterminated, and read straight out of main RAM. It does not transfer.

### Inputs

Read from `sf.cpp:476-703`. `sf` uses the `sfus` port set, which is `common`
plus overrides on IN0 and IN1.

`SYSTEM` (`0xc0000c`): START1 0x0001, START2 0x0002, SERVICE1 0x0004, bits
0x0008–0x0040 `IP_ACTIVE_LOW IPT_UNKNOWN`, and **0x0080
`IP_ACTIVE_HIGH`** with MAME's comment "Freezes the game ?" — the one bit whose
idle level is high rather than low.

`IN0` (`0xc00000`) in `sfus`: COIN1 0x0001, COIN2 0x0002, P1 BUTTON6 0x0004,
P2 BUTTON6 0x0100, P1 BUTTON3 0x0200, P2 BUTTON3 0x0400, rest UNKNOWN.

`IN1` (`0xc00002`) in `sfus`: P1 joystick R/L/D/U at 0x0001/0x0002/0x0004/0x0008
(`PORT_8WAY`), P1 BUTTON1 0x0010, BUTTON2 0x0020, BUTTON4 0x0040, BUTTON5
0x0080; P2 joystick at 0x0100/0x0200/0x0400/0x0800, P2 BUTTON1 0x1000, BUTTON2
0x2000, BUTTON4 0x4000, BUTTON5 0x8000.

Six buttons per player, split across two ports with buttons 3 and 6 in IN0 and
1/2/4/5 in IN1. `machine::inputs`'s `in0()`/`in1()`/`in2()` bit packing
(`inputs.rs:90-114`) is CPS-1's and does not transfer; its **doctrine** does —
active low, `Default` = `idle()` rather than all-zero, `PlayerInput` as a
struct of named booleans. F keeps the struct and replaces the packers.

`DSW1` (`0xc00008`) and `DSW2` (`0xc0000a`) each combine two physical dip banks
into one 16-bit read:

| Port | Mask | Setting | Default |
|---|---|---|---|
| DSW1 | 0x0007 | Coin_A (`DSW1.7E:1,2,3`) | 0x0007 |
| DSW1 | 0x0038 | Coin_B (`:4,5,6`) | 0x0038 |
| DSW1 | 0x0040, 0x0080 | unused (`:7,:8`) | — |
| DSW1 | 0x0100 | Flip_Screen (`DSW2.13E:1`) | 0x0100 (Off) |
| DSW1 | 0x0200 | "Attract Music" (`:2`) | 0x0200 (**On**) |
| DSW1 | 0x0400, 0x0800 | unused (`:3,:4`) | — |
| DSW1 | 0x1000 | "Speed" (`:5`) | 0x1000 (Normal) |
| DSW1 | 0x2000 | Demo_Sounds (`:6`) | 0x0000 (**On**) |
| DSW1 | 0x4000 | "Freeze" (`:7`) | 0x4000 (Off) |
| DSW1 | 0x8000 | `PORT_SERVICE_DIPLOC` self-test (`:8`) | — |
| DSW2 | 0x0007 | "Game Continuation" (`DSW3.6E:1,2,3`) | 0x0007 |
| DSW2 | 0x0018 | "Round Time Count" (`:4,5`) | 0x0018 (100) |
| DSW2 | 0x0060 | Difficulty (`:6,7`) | 0x0060 (Normal) |
| DSW2 | 0x0380 | "Buy-In Feature" (`DSW3.6E:8, DSW4.11E:1,2`) | 0x0380 |
| DSW2 | 0x0400 | "Number of Countries Selected" (`DSW4.11E:3`) | 0x0400 ("2") |
| DSW2 | 0x0800–0x8000 | unused (`DSW4.11E:4-8`) | — |

Two of these are traps worth naming. **Demo_Sounds' default is 0x0000**, the
opposite polarity to every other default in the table, so a "default = all
bits set" shortcut silently mutes the attract mode. And "Attract Music" at
0x0200 is a *different* setting from Demo_Sounds at 0x2000, both defaulting to
On by opposite bit values. `sfan` further inverts 0x0400 and marks 0x0100
unused, which is one more reason `sf` is the first target.

`coin_w` (`sf.cpp:109-116`) is a byte write at `0xc00011`:

```
coin_counter_w(0, data & 0x01);   coin_counter_w(1, data & 0x02);
coin_lockout_w(0, ~data & 0x10);  coin_lockout_w(1, ~data & 0x20);
coin_lockout_w(2, ~data & 0x40);  /* is there a third coin input? */
```

Three lockouts for two coin inputs, and MAME does not know why either. sfemu
records the byte and exposes it to the debugger; nothing else consumes it.

### Reset and state

`machine_reset` (`sf.cpp:742-748`) zeroes exactly `m_active`, `m_bgscroll`,
`m_fgscroll`, `m_prot_t0`. `machine_start` (:732-740) saves exactly those four,
then configures the `audio2` bank. The whole of SF1's *video* state is those
four scalars — `int m_active; bool m_prot_t0; uint16_t m_bgscroll; uint16_t
m_fgscroll;` — plus three `tilemap_t*` (caches, not state) and the RAM regions.
`m_prot_t0` is the 8751's, so `sf` has three.

That is a strikingly small state vector, and it is a useful check on the
implementation: if SF1's board struct needs more persistent video state than
four scalars plus RAM, something has been invented.

---

## Architecture

### The shape of the problem

`Cps1` (`crates/machine/src/cps1.rs`) is 17 fields and a scanline scheduler.
Roughly two thirds of it is board-independent — the CPU, the cycle carry, the
Z80 interleave accumulators, the sample accumulator, the boxed decoder — and
one third is CPS-1's: `board: Board` with its CPS-A/CPS-B register files,
`video: Video` with its layer priority, `sound: SoundBoard` with its bank and
pin-7 register, `mix`, and `samples: Vec<i16>` being mono.

There are three ways to add a second board, and the choice determines how much
of the frontend has to move.

**Rejected: a `Machine` trait.** Making `Cps1` and `Sf1` implement a common
trait would let the frontend hold `&dyn Machine`. But the frontend's ~40
signatures do not want a machine-shaped interface; they want *fields* —
`m.cpu.d[i]`, `m.total_cycles`, `m.board.trace.frames`, `m.video.palette()`,
`m.board.peek_word(addr)`. A trait wide enough to serve them is a trait with
forty methods and no abstraction, and every one of them costs a virtual call
in the debugger's inner loops. It also makes `size_of` invisible, which matters:
`cps1.rs:127`'s note records that an inline `Decoder` made
`size_of::<Cps1>()` 529,360 bytes and caused eleven tests to abort with
`fatal runtime error: stack overflow`.

**Rejected: generics over a board trait.** `Cps1<B: BoardLike>` pushes the same
forty-method problem into a type parameter and monomorphizes the frontend
twice, with the added cost that every `&Cps1` signature in five files becomes
`&Machine<B>` and every call site has to name a type.

**Chosen: a `Machine` enum in `machine`, and a narrow shared view for the
frontend.** Concretely:

```
crates/machine/src/
  cps1.rs            unchanged: the CPS-1 machine
  sf1/
    mod.rs           Sf1: the machine — CPUs, scheduler, sample buffer
    board.rs         68000 bus: ROM, videoram, palette RAM, I/O, RAM, objectram
    sound.rs         both Z80 buses: the YM board and the ADPCM board
    msm5205.rs       the MSM5205 wrapper around oki::adpcm::Adpcm
    mix.rs           the stereo mix and its saturation
  machine.rs         enum Machine { Cps1(Box<Cps1>), Sf1(Box<Sf1>) }
                     + the narrow accessors the frontend actually needs
  video/…            (in the video crate) sf1 rendering modules

crates/video/src/
  sf1/
    mod.rs           the compositor: bg, fg, sprites, tx, in that order
    gfx.rs           layout-driven tile decode (planes/offsets as data)
    tilemap.rs       ROM-backed bg/fg maps, RAM-backed tx, X-only scroll,
                     flags-map transparency, wraparound
    sprites.rs       the backwards strided walk, quads, invert(), flip pivots
    palette.rs       xRGB_444 with palexpand<4>
```

The enum is boxed on both arms so `size_of::<Machine>()` stays a pointer plus a
tag, and the stack-overflow class of failure cannot come back through this
door. A test asserts `size_of::<Machine>()` against a literal, for the same
reason `cps1.rs:127` records its two numbers.

**The frontend's forty signatures.** Measured: `debug.rs` 10, `gfx.rs` 11,
`gfxpanels.rs` 12, `overlay.rs` 15, `state.rs` 5, `loop_.rs` 21, `main.rs` 6.
They divide cleanly by what they actually read:

- **CPU and cycle state** — `m.cpu`, `m.total_cycles`, `m.line`,
  `m.board.trace` — is *identical* on both boards, because both hold an
  `M68k` and a `Trace`. These signatures take a small `CpuView<'_>` struct of
  borrowed references, produced by one method on `Machine`. `overlay.rs`'s
  `draw_regs`, `draw_disasm`, `draw_status`, and all of `debug.rs` move to
  `CpuView` and never learn there is a second board.
- **Memory peeking** — `peek_word`, `disasm_from`, `mem_at` — becomes two
  methods on `Machine` (`peek_word`, `peek_byte`) delegating to the board.
- **Graphics inspection** — `gfxpanels.rs`'s six functions and `gfx.rs`'s
  viewer — is genuinely board-specific: CPS-1 has layer masks, scroll pages,
  6 palette pages and a bank mapper; SF1 has three fixed-order tilemaps, four
  gfx regions with different layouts, and 1,024 flat palette entries. These
  match on `Machine` and dispatch to per-board draw functions. The viewer's
  *chrome* (panel layout, font, key handling, `ViewState`) is shared; only the
  content-producing functions fork. `LayerMask` stays CPS-1's.
- **Audio** — `samples`, `drain_samples` — see below.
- **The loop** — `loop_.rs::run` takes `&mut Machine`, and
  `loop_.rs:92`'s `const BOARD: u32 = frontend::BOARD_SF2` becomes a
  `LoopOpts` field. That file's own doc already prescribes this: "Only SF2
  exists so far. When the SF1 driver lands, this becomes a field on
  `LoopOpts` — the point of the tag is that loading one board's state into
  another is refused, which needs the loop to know which board it is running."

That is the whole generalization, and it is deliberately not more. Nothing
becomes a trait; nothing becomes generic; the CPS-1 path keeps its concrete
types and its inlining.

### Stereo: widening five signatures

The audio path is mono end to end, in five places:

| Location | Today | Becomes |
|---|---|---|
| `cps1.rs:214` `samples()` | `&[i16]` | `&[i16]` interleaved L,R |
| `cps1.rs:225` `drain_samples()` | `Vec<i16>` | interleaved |
| `audio.rs:28` `Audio::queue` | `&[i16]` mono | `&[i16]` interleaved |
| `resample.rs:108` `Resampler::feed` | mono in, mono out | interleaved, both |
| `resample.rs:264` `Ring::pop` | mono | interleaved |
| `audio.rs:151-156` cpal callback | fans one sample to N channels | copies pairs |

The choice is **interleaved `[i16]`, two channels, always** — not
`Vec<(i16,i16)>`, and not a channel-count parameter. Reasons: cpal's output
buffer is already interleaved, so the callback's copy becomes a memcpy instead
of a fan-out loop; `Ring`'s capacity arithmetic stays integer sample counts
with an even-pair invariant rather than gaining a stride; and CPS-1 emits the
same mono value in both slots, which costs one extra `i16` per sample and keeps
exactly one code path in the resampler. A single code path that CPS-1 also
exercises is worth more than the halved buffer, because the mono path is the
one with 317,500 test cases behind it and the stereo path is new.

`Resampler::feed` must resample the pair, not the samples: interpolating across
a channel boundary produces a signal that is neither channel. Its test asserts
that a hard-panned input (left full scale, right silent) stays hard-panned
through the resampler at a non-integer ratio — the assertion a per-sample
resampler fails and a per-frame one passes.

`Ring::pop`'s `paused` path fills silence; with pairs, "silence" is still zero,
but the *count* must stay even or the channels swap permanently after one
underrun. That is the failure this section exists to prevent, and it gets a
test that underruns mid-stream and asserts the channel assignment afterwards.

`cps1::mix` stays exactly as it is — it is proven and its doc's saturation
argument is sound. `Cps1` gains a two-line adapter that writes its mono result
into both slots. SF1 gets `sf1::mix::mix(ym: (i16,i16), msm0: i16, msm1: i16)
-> (i16, i16)` with explicit per-side saturation.

### Save states

`frontend::state` gains `BOARD_SF1: u32` beside `BOARD_SF2` (`state.rs:78`),
re-exported from `lib.rs`. The existing framing already does the hard part:
`encode(s, board)` and `decode(bytes, board)` take a board tag, and `decode`
refuses a mismatch — so an SF2 state cannot silently load into SF1.

`MachineState` (`snapshot.rs:47-154`) has 25 fields, all CPS-1's. SF1's state
vector is different enough that sharing the struct would mean a dozen fields
that are always zero on one board, and a codec whose `PAYLOAD` size is a lie
for both. Instead: `MachineState` stays CPS-1's, `Sf1State` is a sibling, and
`encode`/`decode` gain SF1 arms selected by the tag. `VERSION` goes to 4, and
its doc gets one more clause, matching the existing "3 since the ADPCM chip
joined the state, 2 since the sound board did".

`Sf1State`'s fields, derived from what the emulation actually holds:

- 68000: `cpu`, `ram` (`0xff8000-0xffdfff`, 0x3000 words), `objectram`
  (0x1000 words), `videoram` (0x800 words), `palette_ram` (0x400 words)
- video: `active`, `bgscroll`, `fgscroll`, `flip_x`, `flip_y`
- board: `coin_ctrl`, `sound_latch`, `vblank_pending`, `inputs`
- scheduler: `total_cycles`, `line`, `carry`, both Z80s' `carry`/`debt`/`total`,
  `sample_acc`, the ADPCM interrupt accumulator
- Z80 #1: `z80`, `ram` (0x800 bytes)
- Z80 #2: `z80`, `bank` — and no RAM, because there is none
- YM2151: `ym`, `ym_addr`
- MSM5205 ×2: `signal`, `step`, `data`, `vck`, `reset`, and the pending-capture
  timer's remaining count

`Adpcm::restore(signal, step)` already clamps both, which is the guard this
needs against a hand-edited file. The MSM5205 wrapper's own fields need the
same treatment: a `reset()`-style restore that cannot produce a state
`update_adpcm` would panic on.

Following `state.rs`'s existing discipline, `PAYLOAD` is a hand count written
out term by term — **not** `size_of::<Sf1State>()`. That doc's argument is
quoted here because it applies unchanged: "A hand count of the encoded fields,
not `size_of::<Z80>()`, which is 38 — the struct has a byte of alignment
padding, and a format taking its size from the layout would change on a field
reorder without any test noticing. That is this module's whole argument against
serde."

No sample buffer in the state, for `cps1.rs:214`'s stated reason: audio is
output, not state.

### The debugger

E1's overlay works on SF1 with no new panels, because the panels that matter
read `CpuView`: registers, disassembly, memory, status, and the
`CLP / DRP / UND` audio triage row. Three changes:

- The **sound panel** (`overlay.rs:365`) shows CPS-1's YM plus one OKI. SF1's
  shows one YM plus two MSM5205s, so it forks on `Machine`, sharing the row
  layout. It gains an `audio2` bank field, and the out-of-range-bank counter
  from §"The second Z80" appears there — a nonzero value is the signal that
  the bank aliasing assumption is being exercised.
- The **graphics viewers** (E3) fork their content functions per §"The
  frontend's forty signatures". SF1's tilemap view has to show a map that lives
  in ROM, which is new: the viewer reads `tilerom` rather than guest RAM, and
  the panel labels say so, because a reader who assumes RAM will not understand
  why writes never change it.
- **A second CPU in the CPU panel.** SF1 has two Z80s, and the existing panel
  shows one. Both get a row; the second is labelled `audio2` and shows its bank
  and the fact that it has no RAM.

`Trace` stays an instrument, not machine state — it is not in `Sf1State`.
Three of CPS-1's counters are CPS-1-specific and do not appear on SF1.

### Error handling

Unchanged from A–E3, and F does not get to relax any of it:

- `forbid(unsafe_code)` in every crate.
- **Never panic on a guest address.** Every SF1 read and write path masks or
  bounds-checks. The `audio2` bank aliasing above is an instance of this rule,
  not an exception to it.
- Missing ROM data fails loudly, naming the file and the region, at load time —
  `romset::load` already does this, and SF1 adds no new path.
- Unmapped reads return `UNMAPPED` and increment a `Trace` counter. SF1's four
  `nopr()` windows are *mapped* — they are `AMH_NOP` and return the same
  `0xFFFF` — so they must be decoded explicitly rather than falling through to
  the unmapped path, or the debugger's `UND` count reads as a bug during normal
  operation. This is a small thing that will look like a large one if it is
  missed.
- No logic behind the display or audio boundary. `confine::mentions` keeps
  `minifb` in `display.rs` and `cpal` in `audio.rs`; SF1 adds no dependency to
  either.
- No clock, no filesystem, no network in `machine`, `video`, `m68k`, `z80`,
  `ym2151`, `oki`.

### Workspace edges, preserved

Every existing dependency-edge comment stays true:

- `machine` must not gain `romset` — that would drag in miniz_oxide and std and
  forfeit sub-project A's WASM posture. SF1's `GameSpec` lives in `romset`; the
  bytes reach `machine` as slices, exactly as SF2's do.
- `video` keeps zero dependencies.
- `frontend` keeps exactly one dependency (`machine`), with `video` arriving
  through `machine`'s `pub use`.
- `oki` keeps zero runtime dependencies and keeps building for
  `thumbv7em-none-eabihf` with `--no-default-features`. SF1's MSM5205 wrapper
  lives in `machine`, not in `oki`: `oki` is the OKI chip, and putting a second
  chip's wrapper there would make the no-std target carry code it does not
  need. The shared part — `adpcm.rs` — is already `oki`'s and is already
  no-std.
- `testrunner` must not gain `machine`.
- `sfemu` keeps `minifb` and `cpal` each reachable from one file.

---

## Definition of done for F

1. `romset` has an `SF1` `GameSpec` — eight regions, **40 files** (the 44 in
   `ROM_START(sf)` less the four `proms`), names/offsets/lengths/CRCs only — in
   `ALL` (`games.rs:96`, today `&[&SF2]`) and findable by `by_name("sf1")`.
   Exactly one existing test changes: `games.rs:121`'s
   `assert!(by_name("sf1").is_none())` becomes an assertion that it resolves.
   Every other test in that module is scoped to `SF2.regions` and stays as it
   is — including `:199`'s `seen.len() == 23`, which counts SF2's files only.
   SF1 gets its own parallel set: eight regions with the file counts
   6/1/2/4/8/14/1/4, the eight region sizes as literals, every entry inside its
   region, 40 distinct CRCs and 40 distinct names, and `maincpu`'s six
   `Word16Byte` entries alternating even/odd at the three `0x20000` bases with
   the byte order asserted directly — a CRC check catches a swapped *file* but
   not a swapped *byte*, which is the error `spec.rs`'s `Word16Byte` doc warns
   byte-swaps every instruction word.
2. `Timing`'s `cycles_per_line` is a `RationalAccumulator`, `cps1_10mhz()`
   builds it as `640/1` with its existing behaviour and test count unchanged,
   and `Timing::sf1_8mhz()` is 8 MHz / `8_000_000:15_360` / 256 / 240. Tests
   assert the reduced fraction `3125/6` as literals, assert 256 advances sum to
   133,333 cycles with remainder 1/3 carried, and assert the Z80 fraction is
   `715_909/3_072` — not CPS-1's `715_909/3_125`. The 68000 remainder is in the
   save state and a restore round-trip proves it survives.
3. The SF1 68000 board answers all three maps' shared addresses plus `sfus`'s
   two `nopr()` windows, with `0xFFFF` from the nop windows and a counted
   `UNMAPPED` only for genuinely undecoded addresses. Every read and write path
   is bounds-safe for every 32-bit guest address.
4. The vblank interrupt is level 1 → vector 0x64, asserted as a literal
   alongside CPS-1's 0x68 so the two cannot be conflated.
5. Graphics decode: the layout-driven decoder produces the four gfx regions'
   element counts (4,096 / 8,192 / 14,336 / 1,024) from the region sizes, and
   decodes a known code from each of the four `GFXDECODE` entries with the plane
   offsets, MSB-first bit order and per-entry granularity (16 / 16 / 16 / 4)
   asserted against hand-computed pens.
6. Palette: `xRGB_444` through `palexpand<4>` for all 1,024 entries, with no
   brightness term, asserted against hand-computed RGB for the corner cases
   (0x000, 0xFFF, and one asymmetric value).
7. Tilemaps: bg and fg read `tilerom` at the derived offsets with the correct
   byte-plane split; tx reads `videoram`; scroll is X-only and reduces to
   `(-scroll) mod width`; transparency is per-layer (bg opaque, fg pen 15, tx
   pen 3) via the flags-map rule, with a test that bg's pen-15 pixels are
   **drawn**; wraparound covers the 512×256 extent.
8. Sprites: backwards strided walk, `invert()`'s four-entry table, quads with
   both flip swaps, and both flip pivots (480/224 and 496/240), each asserted
   with the MAME literals.
9. Compositing is the fixed order bg → fg → sprites → tx with `m_active` bits
   5/6/3/7 honoured, including the disabled-bg fill with pen 0, and no
   priority machinery.
10. Screen flip: the composed mechanism produces the same frame as the
    whole-frame mirror for SF1's geometry, with `xextent = 512`,
    `yextent = 256` and the visible window's symmetry asserted as literals
    rather than inherited from `video`'s CPS-1 argument.
11. Sound board 1: the Z80 map (`0xc000`/`0xc800`/`0xe000`), the NMI-pulse
    latch, and the YM2151 IRQ line, with a test that a `soundcmd_w` write
    lands the Z80 at `0x0066` and that a second write while the first is
    unserviced does not lose the NMI edge.
12. Sound board 2: no RAM (writes discarded everywhere), 256 bank entries with
    out-of-range selections aliased deterministically and counted, port `0x01`
    serving both an MSM write and the latch read, and the 8 kHz periodic IRQ0
    on its own rational accumulator.
13. Both MSM5205s: `oki::adpcm::Adpcm` reused verbatim, the 4-bit `data_w`
    path, the reset bit, the VCK falling-edge capture delayed by `clock()/6`,
    the 10-bit DAC mask, and silence at signal zero — each asserted against
    `msm5205.cpp`'s numbers.
14. The stereo mix saturates per side, with a test that full-scale YM plus two
    full-scale MSMs pins at ±32,767.
15. All five audio signatures carry interleaved stereo, the cpal callback
    copies pairs, and the resampler preserves a hard pan at a non-integer
    ratio. `Ring` keeps its pair invariant across an underrun.
16. `Machine` is the enum, boxed on both arms with `size_of` asserted; the
    frontend's CPU/memory panels run through `CpuView` unchanged; the graphics
    and sound panels fork per board; `loop_.rs`'s `BOARD` const is a
    `LoopOpts` field.
17. Save states: `BOARD_SF1`, `Sf1State`, `VERSION` 4 with its doc clause,
    `PAYLOAD` as a hand count, round-trip tests for both boards, and a test
    that each board's tag refuses the other's state.
18. `main.rs` selects the board rather than hardcoding `games::SF2`, four
    region lookups, `BoardConfig::sf2()` and `Timing::cps1_10mhz()`. The
    selection is explicit, not inferred from a filename.
19. The two committed errors are corrected:
    `2026-08-05-m68000-core-design.md:45`'s "dual YM2151" and
    `board.rs:1220`'s "different CPS-B row".
20. The full commit gate is green, every existing suite at its existing count:
    `cargo fmt --all --check`; `cargo clippy --all-targets --all-features -- -D
    warnings`; `cargo test --workspace` and `--release`;
    `cargo doc --no-deps --workspace`; `report -- --test suite` 127/127;
    `reportz80 -- --test suite` 1,604/1,604; `reportym` (no arguments)
    1,000/1,000; `reportoki -- --test suite` 1,000/1,000;
    `cargo build -p oki --no-default-features --target thumbv7em-none-eabihf`;
    `cargo test -p oki --no-default-features` 29 tests;
    `cargo build -p oki --features serde`.
21. README's roadmap row F is updated, and the three `SFEMU_ROMS`-gated tests
    have SF1 siblings under the same single gate.

**Item 21 is the end of "the emulator runs one game."**

---

## Risks

Each of these names what is *not* eliminated by the work above.

- **The `audio2` bank overrun is undefined behaviour on real hardware, and this
  document guesses.** MAME configures 256 banks from a region holding 8, and
  the guest presumably never selects one past 6. Deterministic aliasing plus a
  counter is the best available answer, but if the guest *does* select a high
  bank as part of normal operation, the aliased data is wrong and the symptom
  will be corrupted ADPCM, not a crash. The counter is what makes that
  diagnosable instead of mysterious.
- **SF1's 60 Hz refresh is MAME's assertion, not a measurement.** A real board's
  refresh is derived from its dot clock, and `set_refresh_hz(60)` with
  `set_vblank_time(0)` is the shape a driver takes when nobody measured it.
  Everything in §"Timing" follows from that 60, including all three rational
  accumulators. If the true rate is the dot-clock-derived 61.035 Hz — or
  anything else — every fraction shifts together, which is at least a coherent
  failure: the board would run uniformly fast, not drift internally. The
  mitigation is that the three fractions are derived from two named constants
  (the frame rate and the line count) in one place, so correcting the rate is a
  one-line change rather than three transcriptions.
- **`Timing` gaining a fractional `cycles_per_line` touches CPS-1's scheduler.**
  The 640/1 accumulator is arithmetically identical to today's constant, but it
  replaces a field read with an `advance()` in `Cps1::step_instruction`'s hot
  path, and it puts a new remainder into CPS-1's save state that is always zero.
  A zero-remainder field that nothing exercises is a field no test protects; the
  guard is that SF1's non-zero remainder runs through the same code.
- **`set_periodic_int(..., from_hz(8000))` carries MAME's own `// ?`.** The
  real ADPCM rate is set by hardware sfemu is not modelling. If music and
  sound effects drift against each other, this constant is the first suspect,
  and no test in this sub-project can distinguish 8,000 from 8,192.
- **The stereo mix's integer coefficients are a choice, not a measurement.**
  MAME's 0.60 / 1.0 / 1.0 routes are floats through a float speaker mixer;
  sfemu picks integers. `cps1::mix`'s doc records its own error as "within
  0.952 LSB of MAME's f32 chain"; SF1's will have a comparable bound, computed
  and recorded, but a bound is not zero, and two ADPCM channels at full scale
  is where it is largest.
- **The 15.625 µs ADPCM capture delay is modelled, not verified.** No test
  available to this repo can prove the delay's *effect*. What the tests can
  prove is that a reset arriving during a pending capture resolves the same way
  MAME's timer does, which is the part that produces audible artefacts.
- **The layout-driven gfx decoder is new code on the critical path.** It
  replaces `tiles::tile_pen`'s hardcoded interleave with data-driven plane
  offsets, and its bugs look like graphical corruption rather than test
  failures. The mitigation is that its inputs are transcribed tables with their
  own tests and its outputs are hand-computed pens, but a decoder is exactly
  the kind of code where a test and its implementation can share a
  misunderstanding.
- **The `Machine` enum is a bet that two boards is the right number to design
  for.** A third board — CPS-1.5, CPS-2, another pre-CPS Capcom design — would
  make the enum's per-arm frontend dispatch tedious, and the trait this
  document rejects would start to look right. That is a real cost deferred, not
  avoided; it is deferred because designing for a third board now means paying
  the forty-method abstraction today for a board nobody has asked for.
- **Screen flip's equivalence argument is re-derived, not proven in general.**
  The whole-frame mirror equals the composed mechanism for SF1's specific
  geometry. Sprites are flipped by their own pivots in `draw_sprites`, so the
  equivalence has to hold for two independent mechanisms landing on the same
  pixels. If they disagree by one pixel, the symptom is a one-pixel shift
  visible only in a flipped cabinet, which nobody will notice and the test
  asserting the pivots is the only guard.
- **`sf` may not be the set the user has.** The `sfjp` family is the Japanese
  and world release and needs the 8751; `sfan` needs the pedals. A user whose
  ROM set is `sfj` gets a clear "unsupported set" error naming what is missing,
  not a silent mis-boot — but they still do not get a running game.

---

## Out of scope

- The i8751 protection MCU, and therefore the `sfua`, `sfj`, `sfjbl` and `sfw`
  sets.
- The pneumatic punch/kick cabinet, and therefore the `sfan` and `sfjan` sets.
- The `sfp` prototype (level-6 interrupt, vector 0x78 — recorded above so it is
  not re-derived later).
- The `proms` region.
- MAME's optional output compressor.
- Netplay, WASM, shaders, rewind, cheats.

---

## Sources

All MAME sources read at tag `mame0261`, licence BSD-3-Clause, on 2026-08-16
and 2026-08-17. Nothing was compiled; every fact is a read of the source.

| Source | What was read |
|---|---|
| `src/mame/capcom/sf.cpp` (© Olivier Galibert, 1,428 lines) | The whole driver: `coin_w`/`soundcmd_w`/`sound2_bank_w`/`msm_w` (:109-137), all three 68000 maps and both Z80 maps and the I/O map (:139-232), the three tile-info callbacks and `video_start` (:239-271), the protection handlers (:273-318), `videoram_w`/both scroll writes/`gfxctrl_w` (:320-355), `invert`/`draw_sprites`/`screen_update` (:360-467), all input ports (:476-703), both gfx layouts and `GFXDECODE_START` (:701-729), `machine_start`/`machine_reset` (:732-748), all four machine configs (:750-810), `ROM_START(sf)` (:829-895), the `GAME` macros (:1421-1428) |
| `src/devices/sound/msm5205.cpp` / `.h` | `device_start`'s save list, `device_reset`, `index_shift`, `compute_tables`, `toggle_vck`, `update_adpcm`, `vclk_w`, `reset_w`, `data_w`, `get_prescaler` (:100-275), `sound_stream_update` (:344-374); `S48_4B`/`SEX_4B`, `set_prescaler_selector`, `adpcm_capture_divisor`, `m_dac_bits` |
| `src/emu/emupal.cpp` / `.h` | `xrgb_444_t` (h:213), the `xRGB_444` → `standard_rgb_decoder<4,4,4, 8,4,0>` resolution (cpp:171-175), `standard_rgb_decoder`'s body (h:130-136), `palette_device::write16` (cpp:405-409) |
| `src/lib/util/palette.h` | `palexpand<4>` (:236) |
| `src/emu/drawgfx.cpp` / `.h` | `readbit` (:24), `set_layout`'s granularity (:145), `gfx_element::decode` (:289-318), `transpen` |
| `src/emu/drawgfxt.ipp` | `PIXEL_OP_REBASE_TRANSPEN` (:208-214), `drawgfx_core`'s clip-before-flip (:421-554) |
| `src/emu/digfx.cpp` | `m_layout_xormask` from `GFXENTRY_ISREVERSE` (:125), bit-valued `region_length` (:149), `glcopy.total` (:185-186), fractional plane offsets (:223) |
| `src/emu/tilemap.cpp` / `.h` | `scanline_draw_masked_ind16` (:188-210), `effective_rowscroll`/`effective_colscroll` (:26-74), `m_dx`/`m_dx_flipped` init (:394-395), `set_transparent_pen` (:557-564), `mappings_update` (:715-741), `tile_update`/`tile_draw` (:795-877), `configure_blit_parameters`, `draw_common`; `tile_data::set` (h:386-394), the layer/category constants (h:330-332), `set_scrolldx`/`set_scrollx` (h:467-472), `set_flip` (h:475) |
| `src/emu/driver.cpp` | `save_item(m_flip_screen_x/_y)` (:229-230), the `irqN_line_hold`/`_assert` table (:265-295), `updateflip` (:306-310), `flip_screen_set` (:317-329), `flip_screen_x_set`/`_y_set` (:336-367) |
| `src/emu/screen.cpp` / `.h` | `configure`'s `m_scantime`/`m_pixeltime` and the `m_oldstyle_vblank_supplied` branch (cpp:997-1005); `set_refresh_hz`/`set_vblank_time`/`set_size`/`set_visarea` (h:250-311), the flag's accessor (h:193) and field (h:455) |
| `src/emu/speaker.cpp` | `speaker_device::mix`'s panning, and its absence of a clamp (:89-146) |
| `src/emu/emusound.cpp` | `m_compressor_enabled` from a machine option (:1080), the final per-side clamp and `× 32767.0` interleaved downmix (:1598-1632) |
| `src/emu/addrmap.h`, `src/emu/memory.cpp` | `nopr()` → `AMH_NOP` (addrmap.h:137-139), `unmap_generic(..., quiet = true)` (memory.cpp:716-718) |
| `src/devices/cpu/m68000/m68000.cpp`, `m68kcommon.h` | `execute_set_input` (cpp:368-406), `update_interrupt` (:408-416), `start_`/`end_interrupt_vector_lookup` (:450-476); `autovector(level) = 0x18 + level` (h:50), `set_interrupt_mixer` (h:54), `m_interrupt_mixer` default `true` (h:64, :70) |
| `src/mame/capcom/cps1.cpp` | `set_interrupt_mixer(false)` (:3913) and the mono `SPEAKER` (:3935), for the contrast this document turns on |

Repository sources read for the reuse boundary (all in this workspace, at
`61986de`): `crates/machine/src/{cps1,timing,config,board,inputs,resample,snapshot,sound,lib}.rs`;
`crates/oki/src/{adpcm,chip}.rs`; `crates/ym2151/src/chip.rs`;
`crates/z80/src/{cpu,interrupt}.rs`; `crates/m68k/src/{cpu,exception}.rs`;
`crates/video/src/{lib,compose,layers,sprites,tiles,palette,regs,bank}.rs`;
`crates/romset/src/{spec,games,load}.rs`;
`crates/frontend/src/{state,overlay,debug,gfx,gfxpanels,pace}.rs`;
`crates/sfemu/src/{main,loop_,audio,confine}.rs`; every `Cargo.toml`.
