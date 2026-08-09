# Design: YM2151 FM and the sound board (sfemu sub-project D2)

Date: 2026-08-09
Scope: Sub-project D2 of the sfemu arcade emulator
Status: approved
Depends on: B (board and bus), C (video and the frame schedule), D1 (the Z80 core)

**One sentence:** a cycle-accurate YM2151 in `crates/ym2151`, plus the CPS-1 sound
board that wires it to D1's Z80 and to the 68000's existing sound latches — and it
still makes no sound.

---

## Context

Seven sub-projects are complete: the 68000 core (A), the board and ROM loader (B),
CPS-1 video (C), the Z80 core (D1), and the frontend's three surfaces (E1 window
and save states, E2 debugger, E3 graphics viewers).

D1 delivered a Z80 verified against 1,604 vector files and 1,604,000 cases. It has
never executed a byte of the SF2 sound program, because nothing is attached to it:
`Cps1` has no Z80, and the Z80 has no board.

Both ends of the seam already exist and are traced:

- **The 68000 side.** `Board::write_lanes` decodes `0x800180` and `0x800188` into
  `sound_latch: [u8; 2]`, counts `trace.sound_latch_writes`, and reproduces MAME's
  lane quirk — a high-byte-only write to the second latch is decoded and discarded
  (`cps1.cpp:308-312`). Verified by four tests in `board.rs:1269-1327`.
- **The ROM.** `romset` populates `audiocpu` (0x18000 bytes, one 64 KB file split
  across a 64 KB gap by `ROM_CONTINUE`) and `oki` (0x40000, two concatenated
  128 KB samples), both CRC-verified, both carrying the comment "Loaded for
  sub-project D; nothing reads it in B."
- **The CPU.** `z80::Bus` already has `port_in`/`port_out` and an `irq_ack`
  default, and `Z80::service` already implements NMI, the three maskable modes and
  the `EI` delay.

So D2 does not invent a seam. It builds the chip that sits on the far side of one,
and the board that connects them.

## What this is not

- **Not audio output.** No host audio device, no mixing, no resampling to a
  speaker rate. D2 generates YM2151 samples into a buffer and nothing plays them.
  **D3 is the one that ends "there is no sound."**
- **Not the OKI MSM6295.** The ADPCM sample player is D3. D2 decodes its two
  addresses on the Z80 bus (`0xF002` and the pin-7 latch at `0xF006`) because the
  sound program writes them from the first frame and an undecoded write is an
  unmapped access that would swamp the trace — but the values go to a recorded
  stub, not to a chip.
- **Not the YM2164.** ymfm implements the OPP variant in the same file. SF2 has a
  YM2151 (`cps1.cpp:3940`) and the variant's differences (mystery registers
  `00`-`07`, half-rate timer B) are not emulated.
- **Not an approximation.** "It sounds about right" is not a verification. See
  below: D2 is sample-exact against a reference or it is not done.

---

## Verification, and why this sub-project is different from every one before it

**A, C and D1 all had a published vector suite. D2 does not.** There is no
SingleStepTests for the YM2151, and no equivalent anywhere: FM synthesis has no
per-instruction granularity to enumerate.

This is the central design problem of D2, and it is exactly where this project's
characteristic defect — *the claim that cannot fail* — is most likely to land. The
tempting answer is a handful of hand-written tests asserting that a key-on produces
non-zero output and that a key-off eventually decays. **Every such test passes
against a chip that computes the wrong waveform**, and would pass against a sine
generator with none of the OPM's operator routing, envelope rates, LFO, or noise.

### Ground truth: ymfm, compiled locally, as a vector generator

`https://github.com/aaronsgiles/ymfm` — **BSD 3-Clause**, by Aaron Giles, the
implementation MAME itself uses for the YM2151. It is the closest thing to a
reference the OPM has: derived from die extractions of the OPN family's sine and
power tables, with the OPM's own LFO, noise and DAC round-trip.

**Verified 2026-08-09 by compiling and running it on this machine**, not assumed:

- The OPM path is **five files and no build system**: `ymfm.h`, `ymfm_fm.h`,
  `ymfm_fm.ipp`, `ymfm_opm.h`, `ymfm_opm.cpp` — 3,482 lines, no dependencies
  beyond the C++17 standard library. `c++ -std=c++17 -O2 probe.cpp ymfm_opm.cpp`
  builds clean with Apple clang 17.
- `chip.sample_rate(3'579'545)` returns **55,930**, which is
  `3'579'545 / (2 × 32)` — prescale 2, 32 operators. **One output sample is
  exactly 64 input clocks.**
- A minimal key-on (algorithm 7, TL 0, AR 31, both pan bits) produces output
  spanning −32,704 to +32,640 over 2,000 samples. The chip works.
- Output fits `i16`: `ym2151::generate` clips to ±32,767 and then calls
  `roundtrip_fp`, which simulates the external YM3012 DAC's 10.3
  mantissa/exponent truncation. So a vector record stores two `i16`, not two
  `i32`.
- **Timers are host-scheduled**, which is the one part of ymfm's interface a
  generator must supply itself: `ymfm_set_timer(tnum, duration_in_clocks)` asks
  the host to call `engine_timer_expired(tnum)` later, and `ymfm_update_irq`
  reports the IRQ line. A 60-line host that holds two deadlines and advances them
  64 clocks per sample was written and **verified to produce a timer-A IRQ and to
  clear it on a `0x14` bit-4 write** (`irq=1 status=01` → `irq=0 status=00`).

- **The lazy `prepare()` gate is semantics, not an optimisation** — measured, and
  the one finding here that would have produced a quietly wrong core. ymfm calls
  `prepare()` only when `m_modified_channels != 0` or every 4,096 samples
  (`ymfm_fm.ipp:1287`), and `fm_operator::prepare` *consumes* the CSM key-on flag
  (`m_keyon_live &= ~(1 << KEYON_CSM)`, `ymfm_fm.ipp:434`). Calling it every
  sample therefore eats CSM triggers one sample after they arrive. With CSM off,
  eager and lazy agree bit-for-bit over 40,000 samples; with CSM on
  (`0x14` bit 7) and a host that actually fires timers, they **diverge** —
  39,737 non-silent samples of 40,000 against 15,775. So the Rust port
  reproduces the gate and its 4,096 counter exactly, and the CSM test is the one
  that pins it.

  A corollary about measurement discipline: the first version of this comparison
  used a host that recorded timer deadlines but never called
  `engine_timer_expired`. No timer ever fired, so CSM never triggered and the
  CSM-on and CSM-off hashes came out identical — a passing comparison of nothing.
  The divergence only appeared once the host fired timers. The suite's generator
  host must fire them for the same reason.
- **The active-channel mask is a pure optimisation**, tested separately and in the
  opposite direction: deleting `chanmask &= m_active_channels`
  (`ymfm_fm.ipp`, `fm_engine_base::output`) changes no sample over the same
  40,000, with CSM on or off. The Rust port may therefore sum all eight channels
  unconditionally, and does — one less piece of state to restore in a save.

So D2's suite is **generated, not downloaded**: a small C++ program links ymfm,
runs a deterministic script of register writes, and writes a binary vector file
that the Rust side replays sample for sample. This is the same shape as D1 — a
binary vector format, a streaming converter, a loud failure when the data is
absent — with the fetch step replaced by a build-and-generate step.

### The generator is licence-clean and its output is not committed

ymfm is BSD-3, so quoting its interface and linking it in a dev-only generator is
permitted with attribution. **Nothing about it enters the shipping crates**: the
`ym2151` crate is written from the OPM register map, the Yamaha documentation, and
the algebra, and is *compared* against ymfm. The vector files land in
`testdata/ym2151/`, which is gitignored, exactly as `testdata/z80` is. No ymfm
source is vendored into this repository; the generator fetches it into a temporary
directory, as the D1 fetcher fetched vectors.

Per this project's standing rule, with no exemption for diagnostics: **a test whose
vector file is absent fails naming the file and the generate command.** No
`#[ignore]`, no environment-variable escape hatch, no silent skip.

### The measurement that shaped the vector script

A generated suite has a failure mode a downloaded one does not: **the generator can
produce cases that cannot fail.** So the script was measured before being specified.

Three scripts were run for 500 cases each, 512 samples per case:

| script | cases producing any non-zero sample |
|--------|-------------------------------------|
| 24 random `(reg, val)` writes | **0 of 500 (0%)** |
| 200 random `(reg, val)` writes | 61 of 500 (12%) |
| structured: full voice per channel, then key-on | **500 of 500 (100%)** |

**A random register script is silent.** That is not a surprise in hindsight — a
YM2151 needs a coherent envelope and a key-on before it makes anything — but a
suite built on it would have been 1,000 cases of `0 == 0`, passing against an
implementation that returned a constant zero. This is the single most important
measurement in this document, and it is why the script is structured: every case
writes all 32 operators' DT1/MUL, TL, KS/AR, AMS/D1R, DT2/D2R and D1L/RR, all 8
channels' pan/feedback/algorithm, key code, key fraction and PM sensitivity, then
the LFO, noise and timer registers, then keys on all eight channels.

### And the measurement that shaped the record

With the structured script, a **one-bit perturbation** of each register family was
run against 200 cases and the resulting sample streams compared:

| perturbed bit | cases whose audio changed |
|---------------|---------------------------|
| noise enable (`0F` bit 7) | 198/200 (99%) |
| key code (`28` bit 0) | 191/200 (96%) |
| algorithm (`20` bit 0) | 188/200 (94%) |
| key fraction (`30` bit 2) | 183/200 (92%) |
| total level (`60` bit 0) | 175/200 (88%) |
| attack rate (`80` bit 0) | 90/200 (45%) |
| first decay rate (`A0` bit 0) | 89/200 (44%) |
| LFO frequency (`18` bit 0) | 62/200 (31%) |
| sustain level (`E0` bit 4) | 57/200 (28%) |
| **release rate (`E0` bit 0)** | **0/200 (0%)** |
| **timer B (`12` bit 0)** | **0/200 (0%)** |

The two zeros are two different findings, and both change the design:

1. **Release rate is unobservable in a case that never keys off.** A note that is
   still held has not entered the release phase, so `RR` has had no effect on
   anything. Fixed by giving every case a **key-off phase**: writes are scheduled
   *in sample time*, and every case keys all eight channels off at sample 256 of
   512. Re-measured: `RR` bit 0 is detected in **77/200 (38%)** of cases. Without
   this, every release-rate rule in the implementation would have been unverified
   while the suite reported full coverage.

2. **Timer state is not audible at all.** Timers A and B drive the status register
   and the IRQ pin, and touch the audio path only through CSM. A record holding
   audio alone cannot see them. Fixed by giving each case a **status trace**: one
   byte per sample, holding the status register with the IRQ line in bit 7.
   Re-measured against the combined record: timer A high `98/200`, timer A low
   `98/200`, timer B `25/200`, mode enable-A `98/200`, CSM `97/200`. And the
   premise is checked too — **112 of 200 cases contain an IRQ edge and a
   non-constant status trace**, so the channel is not 200 copies of a constant.

A suite whose own discriminating power has been measured is the point of this
section. The figures above are re-asserted as tests, on the generated data, so a
future regeneration that produces a silent or insensitive suite fails loudly rather
than passing vacuously.

### The vector format

Little-endian, magic-and-count, mirroring `Z80V`'s spirit:

```
file:   u32 magic 0x564D_5941   u32 num_cases
        ('A','Y','M','V' in file order -> 0x564D5941 as a little-endian u32;
         the reversal is stated because getting it wrong reads fine in a spec
         and fails at runtime, exactly as it did for Z80V)
case:   u32 seed, u16 num_writes, write[num_writes],
        u16 num_samples, sample[num_samples], u8 final_status
write:  u16 at_sample, u8 reg, u8 val      (at_sample is when, in sample time)
sample: i16 left, i16 right, u8 status     (status bit7 = IRQ, bits 0-1 = timers)
```

`num_samples` is 512 and `num_writes` is 272 for every case in the shipped script
(264 setup writes + 8 key-offs). Both are fields rather than constants so a later
script can differ, and **the converter asserts every bound and fails naming the
case** — a checked bound is a bound that can be raised safely.

Measured size: 5 bytes per sample × 512 + 4 × 272 + 9 header/trailer bytes
(`u32` seed, two `u16` counts, `u8` final status) = **3,657 bytes per case**.
At 1,000 cases that is **3.7 MB**, against D1's 228 MB. Disk is not a risk here;
the machine has 1.4 GiB free and the suite is a rounding error against it.

Determinism was verified: the same seed produces a byte-identical file across runs
(FNV-1a `111bf4875f89fdb0` twice for a 4-case run). 5,000 structured cases generate
in **1.5 s**, so the whole suite is generated in well under a second and
regeneration is never a reason to skip it.

### One test per group, and the coverage tests are the substance

1,000 cases is one file, not 1,000. Following `z80suite.rs`: a handful of group
tests over case ranges, plus the three coverage tests that make emptiness fail —
**a loop over an empty case list passes**, which is this project's recurring
vacuous shape. Counts are literals, never `read_dir().count()` or
`cases.len()`.

Each case checks, in order:

1. every sample's left and right value, by sample index;
2. every sample's status byte, including the IRQ bit;
3. the final status register.

### Ours, hand-written

Read off the OPM register map and the Yamaha documentation, not from the vectors —
a test deriving its expectation from the thing under test proves nothing:

- The **register map decode**: which of `40`-`FF`'s five families a given address
  falls in, and which `(channel, operator)` it names. `channel = reg & 7` and
  `operator = (reg >> 3) & 3`, but that operator index is **not** the slot number
  in the algorithm chain: register-operator 0, 1, 2, 3 are slots **1, 3, 2, 4**.
  `ymfm_opm.cpp:117-138` states it as "the channel index order is 0,2,1,3, so we
  bitswap the index", with `operator_list(0, 16, 8, 24)` — map order carrier 1,
  carrier 2, modulator 1, modulator 2, against the natural wiring order carrier 1,
  modulator 1, carrier 2, modulator 2.

  **Measured 2026-08-09, not transcribed.** Silencing one register-operator at a
  time (TL = 127) under each of the eight algorithms and taking the peak of 400
  samples separates the two candidate maps. Under algorithm 0 — a pure four-deep
  chain whose only carrier is slot 4 — solely `0x78` silences the channel, so
  register-operator 3 is slot 4 under either map. Algorithm 4, two independent
  2-op chains with carriers on slots 2 and 4, is the discriminating case: the
  peak halves for `0x70` and `0x78` and is unchanged for `0x60` and `0x68`, so the
  carriers sit at register offsets `0x10` and `0x18`. The naive 0,8,16,24 map puts
  them at `0x08` and `0x18` and is refuted. The hand-written test is that
  algorithm-4 experiment, because it is the one that fails on the wrong map;
  a test that writes all four operators the same passes on both.
- The **eight connection algorithms**, each as an explicit modulator/carrier
  routing, written from the OPM documentation's diagrams.
- The **four lookup tables, split by provenance** — because three are formulas and
  one is a chip dump, and treating them alike is how 768 typed numbers get a
  plausible wrong entry nobody finds. Each was fitted against ymfm's own table on
  2026-08-09:
  - **Sine** (256 entries): `round(-log2(sin((i + 0.5) × π / 512)) × 256)`,
    **0 of 256 mismatched**. Computed, not transcribed.
  - **Power** (256 entries): `round(2^(-(i + 1) / 256) × 2048) - 1024`, **0 of 256
    mismatched**. Note the `i + 1` and the `- 1024`: the obvious
    `2^(-i/256) × 2048` misses all 256 entries, and truncation instead of rounding
    misses 139. Computed, with those two near-misses recorded as the reason the fit
    is asserted rather than assumed.
  - **Envelope increment** (64 entries of packed nibbles, `ymfm_fm.ipp:145`):
    transcribed, but it is 64 values with visible structure and its own checksum
    (sum `35,716,092,092`, FNV `0x7e8ea9566fb1810d`).
  - **Phase step** (768 entries, `ymfm_fm.ipp:230`): **must be transcribed
    verbatim.** ymfm's own comment says the computed table "differs in a few spots
    from the data verified from an actual chip" and that the table is David Viens'
    analysis. Measured: it diverges from a pure exponential by up to **106 units**,
    its consecutive deltas are quantised to 32/64/96/128/160, and its span is
    1.99615 rather than the formula's 1.99820. **No formula reproduces it**, so the
    transcription is gated by a checksum test rather than by reading: 768 entries,
    sum `46,015,744`, first `41,568` (`0xA260`), last `82,976` (`0x14420`), FNV-1a
    over little-endian `u32` `0x3b0f96a47792bdb3`. The sine and power tables get
    the same treatment for free — sum `65,406` / FNV `0x690166972613166b`, and sum
    `115,543` / FNV `0xc284cffe0e133896` — so a `const fn` that is subtly wrong
    fails on the checksum rather than on one obscure note.

  The FNV-1a used above is the standard 64-bit one (offset basis
  `0xcbf29ce484222325`, prime `0x100000001b3`) over each entry's little-endian
  bytes at the width named. It is **new to this repository** — `grep` finds no
  existing FNV in `crates/` or `scripts/` — so D2 adds exactly one implementation,
  in `crates/ym2151/src/tables.rs`, and the generator's determinism check reuses it
  rather than growing a second.
- The **timer periods**: timer A is `1024 - value` and timer B is
  `16 × (256 - value)`, both in units of `OPERATORS × prescale` = 64 input clocks
  (`ymfm_fm.ipp:1481`). So timer A at value 1000 is 24 × 64 = 1,536 clocks.
- The **`EI` delay's counterpart on this side**: the YM2151's IRQ is level-driven
  and wired to the Z80's `INT` pin (`cps1.cpp:3941`,
  `ym2151.irq_handler().set_inputline(m_audiocpu, 0)`), so it must stay asserted
  until the sound program clears it via `0x14`.
- A **mutation set** in `scripts/mutate.py` for the sound board's address decode
  and the schedule's fractional accumulator, which the vectors do not reach.

---

## The sound board

`crates/machine` gains a `SoundBoard` implementing `z80::Bus`, transcribed from
`cps_state::sub_map` (`cps1.cpp:631-642`):

| address | width | direction | what |
|---------|-------|-----------|------|
| `0x0000`-`0x7FFF` | 32 KB | read | `audiocpu[0x0000..0x8000]`, fixed ROM |
| `0x8000`-`0xBFFF` | 16 KB | read | banked ROM: `audiocpu[0x10000 + bank × 0x4000]`, **2 banks** |
| `0xD000`-`0xD7FF` | 2 KB | read/write | sound RAM |
| `0xF000`-`0xF001` | 2 | read/write | YM2151 address / data |
| `0xF002` | 1 | read/write | OKI MSM6295 — **D3**; a recorded stub in D2 |
| `0xF004` | 1 | write | ROM bank select, bit 0 (`cps1.cpp:292-295`) |
| `0xF006` | 1 | write | OKI pin 7, bit 0 (`cps1.cpp:297-300`) — recorded, not acted on |
| `0xF008` | 1 | read | sound latch 0, the command |
| `0xF00A` | 1 | read | sound latch 1, the timer fade |

Three facts about this map are worth stating because each is a bug that would look
like something else:

- **The banked window is 2 entries, not 6.** `MACHINE_START_MEMBER(cps_state,cps1)`
  configures `(0, 2, base + 0x10000, 0x4000)`; the 6-entry form is the QSound
  board's. Since `audiocpu` is 0x18000 bytes, bank 1 ends exactly at 0x18000 and a
  third bank would read past the region.
- **The Z80 has no I/O ports on this board.** `sub_map` is program space only, so
  `port_in`/`port_out` decode nothing. They are implemented as recorded no-ops
  returning `0xFF`, and a *counter* says how many times the program touched them —
  the sound program should never do so, and a non-zero count is a real finding, not
  a shrug.
- **Everything outside the table is unmapped**, counted in the trace the way
  `Board`'s unmapped accesses are, and reads as `0xFF`. Silence here is what let a
  wrong decode hide in B until the trace existed.

## The schedule, and the one number that is not exact

C's central timing result was that **both** divisions come out exact: 8 MHz / 512 =
15,625 lines per second, and 10 MHz / 15,625 = 640 68000 cycles per line, with no
remainder either time. `timing.rs` asserts both remainders are zero so that a board
needing a fractional accumulator cannot be added without noticing.

**The Z80 is that board.** Its clock is `XTAL(3'579'545)` (`cps1.cpp:3918`,
"verified on pcb"), and 3,579,545 / 15,625 = **229.09088 T-states per line** —
`715909/3125`, not an integer. A frame is 60,021.81056 T-states, also not an
integer.

Truncating to 229 loses 0.09088 T-states per line, which is 1,420 T-states per
second: the sound CPU runs **0.0397% slow**. That is inaudible in a second and is
a drift of about one T-state every 700 microseconds — over a three-minute match,
255,600 T-states, roughly 71 milliseconds of music against the animation. This is
precisely the failure `timing.rs`'s own module documentation describes: "music
drifts against animation over a match, and nothing ever looks broken enough to
investigate."

So the Z80's budget is a **rational accumulator**, not a truncated integer:
`num`/`den` = 715,909/3,125 added per line, with the integer part spent and the
remainder carried — the same shape as `Cps1::carry`, which already carries the
68000's instruction overshoot for the same reason, and which `machine`'s tests
already prove is load-bearing (`reset_restores_the_schedule_exactly` records that a
bounded-first-line assertion left the `carry = 0` mutant alive).

Two consequences, both stated so a reviewer can reject the design if they disagree:

- **The accumulator is exact.** 3,125 = 5^5 divides the frame count evenly across
  3,125 lines, so after 3,125 lines the carry is exactly zero and the Z80 has run
  exactly 715,909 T-states. There is no floating point anywhere.
- **`timing.rs`'s exactness assertions stay.** They are true of the 68000 and the
  pixel clock and remain the right claim; the Z80's inexactness is a *third* fact,
  asserted beside them as `3_579_545 % 15_625 == 1_420` — a non-zero remainder
  written as a literal, so a future edit that "fixes" it fails.

Interleaving: `Cps1::run_scanline` runs the 68000's 640 cycles, then the Z80's
229-or-230, then generates that line's YM2151 samples. Sample generation is
`T / 64`, itself fractional — 3.579545 samples per line — so it uses a second
accumulator over the T-states actually spent, not a third rate. Per frame that is
937.84079 samples, and the buffer is sized from the measured maximum rather than a
guess.

**Why the Z80 runs after the 68000 within a line and not before:** the 68000 writes
the sound command and the Z80 reads it. Running the Z80 first would give the sound
program a one-line-stale latch on every command, which is a one-line delay nobody
would hear — and that is the problem: it is unobservable, so it must be decided by
the reference rather than by ear. MAME interleaves per-timeslice with the 68000
first, and D2 matches it.

## Save states

`MachineState` gains the Z80, the sound RAM, the ROM bank, the YM2151's whole
register file and internal state, and both accumulators. Following
`snapshot.rs`'s existing rules: no ROM (the user supplied it), no framebuffer or
sample buffer (recomputed), no trace (a record of the session, not the machine).

The YM2151's state is the interesting one. Its envelope generators, phase
accumulators and LFO counters are all machine state — a save/restore that dropped
the phase would resume mid-note with a click, which is audible and would present
as a save-state bug in D3 rather than as a missing field here. And per this
project's rule, the codec is verified by **divergence**: restore, run, and require
the same samples. `snapshot == snapshot` passes for a codec that drops a field the
comparison also ignores.

## The debugger

E2's overlay gains a sound panel: the Z80's registers and disassembly (D1 already
wrote `z80::disasm` for exactly this), the two latches, the bank, and the YM2151's
key-on state per channel. `Cps1::step_instruction` steps the 68000; a
`step_sound_instruction` steps the Z80, and both share one code path with their
respective schedulers for the reason `step_instruction` records: "a separate
stepping path is a debugger that single-steps a machine subtly unlike the one that
runs."

---

## Architecture

```
crates/ym2151/
  src/lib.rs        crate docs, re-exports
  src/regs.rs       the OPM register map: address -> (family, channel, operator)
  src/tables.rs     sine, power, and envelope-increment tables
  src/operator.rs   one operator: phase, envelope, output
  src/channel.rs    four operators, the eight algorithms, pan
  src/lfo.rs        LFO waveforms, AM and PM depth
  src/noise.rs      the noise generator (channel 8 operator 4 only)
  src/timer.rs      timers A and B, the status register, the IRQ line
  src/chip.rs       Ym2151: write/read, generate, reset, save state

crates/machine/
  src/sound.rs      SoundBoard: z80::Bus for the CPS-1 sound map
  src/cps1.rs       gains the Z80, the chip, and the two accumulators

crates/testrunner/
  src/ymgen/        the C++ generator: fetches ymfm, builds, emits vectors
  src/ymfmt.rs      the AYMV binary format: writer and parser
  src/ymfiles.rs    the inventory, EXPECTED, and the loud-failure hint
  src/ymrunner.rs   replays one case against crates/ym2151
  src/bin/genym.rs  the generate command
  src/bin/reportym.rs  the report binary
  tests/ymsuite.rs  the group tests and the three coverage tests
```

`crates/ym2151` is **dependency-free and `no_std`-compatible** in the same sense
`m68k` and `z80` are: no threads, no wall-clock, no host I/O. This is the
WASM/netplay constraint that binds A-F, and it is why sample generation writes into
a caller-supplied buffer rather than owning an audio device.

The split into eight small files rather than one is deliberate: the OPM's five
subsystems (operator, channel routing, LFO, noise, timers) are independently
testable and independently wrong, and a single 2,000-line `chip.rs` would make
"which subsystem does this failing case indict" unanswerable.

## Error handling

The YM2151 has no error states. What it has instead:

- **Register `01` bit 1 resets the LFO**, and the test register's other bits are
  undocumented. ymfm implements the documented bit and ignores the rest; D2 does
  the same and says so, rather than guessing.
- **Writes to `1A`** are ymfm's internal fake register for PM depth, not a real
  OPM address. D2 does not have it; `19`'s top bit selects AM or PM depth as the
  hardware does. This is called out because a reader comparing the two register
  maps will find `1A` in ymfm and must not transcribe it.
- **Reads.** `read_status` returns the status register; `read(offset)` on the OPM
  returns the same for any offset, since the chip has one readable register.
- **Unmapped sound-board addresses** are counted and read `0xFF`, as above.

## Definition of done for D2

1. `cargo test --workspace` and `--release` green.
2. The YM2151 suite at **1,000/1,000 cases**, sample-exact on both channels and
   on the status trace, reported as `cases: N/N`.
3. The suite's own discriminating power asserted: 100% of cases non-silent, and
   the IRQ-edge and status-variation premises from the table above.
4. The Z80 vector suite still at **1,604/1,604 files, 1,604,000/1,604,000 cases**,
   and the 68000 suite still at **127/127, 317,500/317,500**. D2 changes `machine`
   and adds crates; movement in either suite is a regression to investigate, not a
   tolerance.
5. `cargo clippy --all-targets --all-features -- -D warnings` clean.
6. `cargo doc --no-deps --workspace` clean.
7. The mutation pass at 100% as-expected, every survivor a declared control or a
   **proven** equivalent — with new sets for the sound-board decode and the
   fractional schedule.
8. `crates/ym2151` has no dependencies. `crates/z80` is unchanged.
   The four table checksums above hold, and the two computed tables are computed
   rather than typed.
9. **A CSM case in the suite.** The lazy-`prepare()` divergence is invisible with
   CSM off, so at least one generated case enables `0x14` bit 7 with timer A
   running and a host that fires it. Asserted as a premise on the generated data,
   like the non-silence and IRQ-edge premises: a suite whose cases all have CSM off
   cannot distinguish the two readings, and the wrong one is the natural way to
   write it.
10. SF2's sound program runs: with a user-supplied ROM set, the Z80 executes from
    `audiocpu`, reads the command latch, and writes YM2151 registers — asserted as
    trace counters over a bounded number of frames, in a test that skips loudly
    with the ROM absent (the single documented exception, as
    `crates/sfemu/tests/boot.rs` is for the 68000).

**Still no sound.** D2's output is a buffer of samples nothing plays. That is not
a defect in it.

## Risks

- **The generated suite could be regenerated wrong.** A silent or insensitive
  suite is the failure mode, and it is mitigated by asserting the measured
  discriminating power as tests rather than trusting the generator. The figures in
  this document are the baseline.
- **ymfm is a reference, not the hardware.** It is derived from die extractions
  and is what MAME ships, which makes it the best available ground truth — but
  agreeing with it is agreeing with an implementation. Stated plainly so nobody
  later reads "sample-exact" as "verified against silicon." Where ymfm and the
  Yamaha documentation disagree, the disagreement is recorded rather than resolved
  by preference.
- **The fractional accumulator is new machinery.** It is the one piece of D2 that
  A and C did not need, and the mutation set exists for it specifically: a
  truncating accumulator passes every test that does not run thousands of lines.
- **Sample-exact comparison is unforgiving in a good way and a bad way.** A
  one-sample offset in when writes land makes every subsequent sample differ, so a
  timing bug in the *harness* looks like a total failure of the chip. The write
  schedule is in sample time for exactly this reason: it removes the ambiguity
  about when a write takes effect.

## Sources

- ymfm (`https://github.com/aaronsgiles/ymfm`), BSD-3-Clause, © 2021 Aaron Giles:
  `ymfm.h`, `ymfm_fm.h`, `ymfm_fm.ipp`, `ymfm_opm.h`, `ymfm_opm.cpp` — fetched,
  compiled and measured 2026-08-09. The OPM register map quoted in this document
  is `ymfm_opm.h:47-101`. Specific citations used above: the OPM constants at
  `ymfm_opm.h:107-124` (`CHANNELS = 8`, `OPERATORS = 32`, `DEFAULT_PRESCALE = 2`,
  `EG_CLOCK_DIVIDER = 3`, `REG_MODE = 0x14`); the operator-to-slot map at
  `ymfm_opm.cpp:117-138`; the tables at `ymfm_fm.ipp:46` (sine), `:86` (power),
  `:145` (envelope increment), `:206-320` (phase step, with the chip-data comment);
  `cache_operator_data` and `compute_phase_step` in `ymfm_opm.cpp`;
  `clock_noise_and_lfo` at `ymfm_opm.cpp:173`; the envelope state machine at
  `ymfm_fm.ipp` (`clock_envelope`, `start_attack`, `start_release`,
  `clock_keystate`); the 4-op algorithm table in `fm_channel::output_4op`; the
  lazy-`prepare()` gate at `ymfm_fm.ipp:1287` with the CSM key-on consumption at
  `:434` and the CSM trigger at `:1516-1522`; and `ym2151::generate` in
  `ymfm_opm.cpp` for the `roundtrip_fp` output stage.

  **The reference is re-fetchable and the measurements are reproducible.** The
  scratch copy used for the first half of this work was deleted by the OS mid-session
  (`/tmp` is not durable on macOS); re-fetching the archive and re-running the fits
  reproduced the 3,482-line count and every checksum above exactly. The generator
  therefore fetches into a temporary directory each run and verifies what it got,
  rather than assuming a copy is still there — the same reason the D1 fetcher
  re-verifies rather than trusting `testdata/`.
- MAME `src/mame/capcom/cps1.cpp` (BSD-3-Clause, copyright-holders Paul Leaman):
  `sub_map` at 631-642, `cps1_snd_bankswitch_w` at 292-295, `cps1_oki_pin7_w` at
  297-300, `cps1_soundlatch_w` at 302-308, the machine config at 3909-3947, and
  `MACHINE_START_MEMBER(cps_state,cps1)` at 3899-3902.
- Yamaha YM2151 (OPM) application manual for the documented register set, the
  eight connection algorithms, and the timer periods.
- The existing `crates/z80` for the `Bus` trait D2 implements, and
  `crates/machine/src/timing.rs` for the exactness assertions the Z80's clock is
  the first exception to.
