# sfemu architecture

What the eleven crates are, which edges the dependency graph has and why, where the
lines that cannot be crossed are drawn, and what a green test suite here does and
does not establish.

Same discipline as `docs/hardware/`: every claim here is backed by a file in this
workspace, a named test, or a stated measurement. Where a number is measured it
says so and gives its date. There is no "roughly" and no recollection. The last
section says how to check that a claim on this page is still true, because a
document nobody can falsify is a document that quietly rots.

This page is a **map with reasons**. It does not restate the eleven crate-level
module docs — those are the authority for their own crate, and each already carries
its own rationale. When this page and a module doc disagree, the module doc is
right and this page is stale.

⚠️ **No ROM data appears in this repository, in this file, or in any test.** See
[README's "This project ships no ROMs, and never will"](../README.md#this-project-ships-no-roms-and-never-will).
The loader takes a path the user supplies; `crates/romset`'s tables hold file
names, offsets, lengths and CRC-32s only.

---

## The crates

Eleven, measured 2026-08-24 by `find crates/<c>/src -name '*.rs' -exec cat {} +`
and by counting `#[test]` attributes:

| Crate | src lines | `#[test]`s | What it is |
|---|---|---|---|
| `m68k` | 16,462 | 222 | The 68000 core. No dependencies, no clock, no globals, `no_std`-friendly. Optional `serde` and a disassembler. |
| `machine` | 16,883 | 396 | The boards: memory map, bus, interrupts, the scanline scheduler, inputs, the sound board, the mono mix, the host resampler, snapshot and restore. |
| `frontend` | 14,465 | 260 | Every frontend decision, with no window: pacing, the key map, the key menu, pen-to-ARGB, the save-state codec, the debugger's state, the graphics viewers, and the 4×6 font. |
| `video` | 9,141 | 192 | CPS-1 graphics — tiles, three scroll layers, sprites, palettes, CPS-A/B registers, the scanline renderer, the layer mask — and SF1's in `sf1`. |
| `z80` | 7,811 | 184 | The sound board's CPU, on `m68k`'s terms, with a disassembler cross-checked against the core. |
| `testrunner` | 7,080 | 136 | Dev-only harness for the four external vector suites. Not shipped. |
| `sfemu` | 6,971 | 126 | The binary. The only crate that names a windowing or an audio library. |
| `ym2151` | 5,930 | 101 | The FM chip. Four tables built from closed forms and checksummed. |
| `testrom` | 3,330 | 46 | The homebrew CPS-1 demo image `--demo` runs. Generated, not a dump. |
| `romset` | 2,533 | 59 | The MAME-format loader: zip or directory, CRC-checked, interleaved into regions. |
| `oki` | 1,136 | 29 | The MSM6295 ADPCM chip: four voices, the phrase table, the 49-step table. |

1,865 tests pass and 7 are ignored on `cargo test --workspace` (measured
2026-08-24). The seven are the ROM-gated ones; see
[What a green suite does not establish](#what-a-green-suite-does-not-establish).

`crates/testrunner` and `crates/testrom` are not part of the emulator. The first
is scaffolding for the vector suites, the second is content. Both are listed
because a reader counting lines in `crates/` would otherwise attribute 10,411 of
them to the emulator.

---

## The dependency graph

```
m68k    video    z80    ym2151    oki          (five leaves, all dependency-free)
  \       |       |       |        /
   +------+---+---+-------+-------+
              |
           machine ────────────┐
              |                |
           frontend            |          romset ── miniz_oxide
              |                |            |
              +--------+-------+------------+------ testrom
                       |
                     sfemu ── minifb 0.28, cpal 0.18

testrunner ── m68k, oki, ym2151, z80        (dev harness, off the shipped graph)
```

Every edge below is quoted from the manifest that declares it, not inferred from
the graph. The comments in `Cargo.toml` are the authority; this table is an index
to them.

| Edge | Why it exists, and what forbids more |
|---|---|
| `m68k`, `video`, `z80`, `ym2151`, `oki` → nothing | Four of the five build for `thumbv7em-none-eabihf`; `video` allocates a framebuffer and needs `alloc`, which is a weaker claim and is not made for it. Each has an *optional* `serde` and nothing else. |
| `machine` → the five | `crates/machine/Cargo.toml`: "`machine` must not gain `romset`: that would drag in miniz_oxide and std and forfeit the WASM posture sub-project A paid for." `oki`'s `serde` feature is off here for the same reason — turning it on would pull serde into `machine`. |
| `machine` `pub use`s all five | So `frontend`'s manifest stays one dependency wide. A downstream crate reaches `m68k::M68k` through `machine::m68k`, and cannot name `m68k` directly without adding an edge someone has to justify. |
| `frontend` → `machine`, and only `machine` | `crates/frontend/Cargo.toml`: "One dependency, and it must stay one." |
| `romset` → `miniz_oxide` | DEFLATE for MAME zips. One of the workspace's two crates.io dependencies outside the binary. |
| `sfemu` → `testrom` | A normal dependency, not a dev-dependency: `--demo` is a shipped feature, and the point of it is that a user with no ROM set can still see the emulator run. |
| `sfemu` → `minifb 0.28` | "Reachable from `display.rs` alone." |
| `sfemu` → `cpal 0.18` | "Reachable from `audio.rs` alone, enforced by `the_audio_library_is_named_in_one_file`." |

**`sfemu`'s manifest must not gain `video`.** It reaches `video` through
`machine::video`, which is the same rule as `frontend`'s: one edge, and every
type that crosses it is one `machine` already re-exports.

### The two library confinements are asserted, not conventional

An absence cannot be asserted from inside the code that would violate it, so
`crates/sfemu/src/confine.rs` scans the source tree for the library's name:

- `display.rs`'s `the_windowing_library_is_named_in_one_file` runs
  `confine::mentions("minifb", &[])` and then again exempting
  `["display.rs", "Cargo.toml"]`, asserting the second finds only
  `sfemu/Cargo.toml`.
- `audio.rs`'s `the_audio_library_is_named_in_one_file` does the same for `cpal`.

Two properties of the scanner are worth knowing before trusting it. **Comments
naming the library are exempt** — a check that forbade them "would delete the
documentation to protect the constraint", and this page is one of the documents
that would go. And the stated limit: a `/* */` block naming the library is a false
positive, because the scanner recognises `//` only.

---

## The display boundary

This is the load-bearing line in the whole design, and it is stated in
`crates/frontend/src/lib.rs`:

> **The rule: no logic behind the display boundary.** A decision made inside the
> module that calls the windowing library cannot be tested, so it must not be made
> there.

The reason is not taste. `cargo test` has no display, and "the right pixels
reached the glass" is not something a test can read back. So:

- **`frontend` decides *what*** — how many frames this host tick owes, which board
  input a key is, what colour a pen is, what bytes a save state is, which row of
  the key menu is selected. It has never heard of a window.
- **`sfemu/src/loop_.rs` decides *when*** — the ordering inside an iteration. It
  holds no arithmetic, which is why every constant it uses comes from `frontend`.
- **`sfemu/src/display.rs` decides nothing.** It translates: keycodes to
  `frontend::KeySet`, a clock to nanoseconds, a buffer to the glass. Its own module
  doc has a section headed "Why this file has no tests".

**`frontend` also reads no clock.** `FramePacer::tick` is *given* the elapsed
nanoseconds, which is what lets a test drive it through a stalled host and assert
exactly how many frames it asks for. The one real clock read in the project is
behind the boundary, in `display.rs`.

The seam is a five-method trait, `loop_::Display`, with a recording fake on the
test side and a `minifb` window on the other.

### Audio is the same line drawn twice

A device has a clock we do not control and a buffer we cannot read back, so
`sfemu/src/audio.rs` is a device handle and five forwards. The parts with edge
cases — the rate conversion and the full-ring policy — live in
`machine::resample`, where a test drives them. What the *loop* decides about audio
(queue once per frame, drain the machine's buffer, report a held pause every tick,
treat a dead device as a notice rather than a stop) is asserted against a
recording fake, not by listening.

`loop_::run` takes `audio: &mut dyn Audio` where the display is `impl Display`, and
that asymmetry is deliberate: `main` picks its sink at runtime — a real device or
`NullAudio` when none can be opened — so it holds a `Box<dyn Audio>`, and a generic
parameter would make that the caller's problem.

---

## Two boards, one dispatcher

`machine::Machine` is a two-arm enum — `Cps1` and `Sf1`, both boxed — and not a
trait. `crates/machine/src/machine.rs` gives the argument:

> The frontend has about forty signatures that name a board — `debug.rs` 10,
> `gfx.rs` 11, `gfxpanels.rs` 12, `overlay.rs` 15, `state.rs` 5, `loop_.rs` 21,
> `main.rs` 6. They do not want a machine-shaped interface; they want *fields*.

A trait wide enough to serve them is forty methods and no abstraction, with a
virtual call in each of the debugger's inner loops, and it makes `size_of`
invisible — which on this codebase is not abstract: a `Cps1` measures 5,232 bytes,
and an unboxed enum would put the larger board by value in every `Machine`
anywhere. Generics fare no better: they monomorphize the frontend twice and turn
every `&Cps1` in five files into a signature whose caller has to name a type.

So `Machine` has a method **only where both boards answer the same question with
the same type**: `board`, `cpu_view`, `timing`, `frame_ns`, `peek_word`,
`step_instruction`, `run_scanline`, `run_frame`, `render`, `reset`, `samples`,
`drain_samples`, `frames`.

`CpuView<'_>` is the narrowing that makes that work. The 68000, its cycle count,
the beam position, the vblank-pending flag and the `Trace` are identical on both
boards, so `debug.rs` and the register, disassembly, memory and status panels take
a `CpuView<'_>` and **never learn there is a second board**. Its two big fields are
borrowed and its three scalars copied, and borrowed is a correctness property as
well as a cost one: a view built by value shows the registers as of when it was
made, and a panel drawn from a stale view reads as the emulator being slow.

**What is deliberately absent** (the full list is in the module doc, with reasons):
no `as_cps1`/`as_sf1` — `if let Some(c) = m.as_cps1()` silently does nothing on the
other board, which is a panel that goes blank with no error; no
`snapshot`/`restore` — the two states are different types with different payload
lengths and a board tag whose whole purpose is to refuse a cross-load; no
`sound_trace` — CPS-1 has one Z80 and an OKI, SF1 two Z80s and two MSM5205s; no
`framebuffer`; no `Deref`, which would make `m.cpu` compile for exactly one arm.

Two places genuinely cannot go through the dispatcher, and both are `match`es in
`main.rs` with a comment saying why: `summary` reads two different `Video` types,
and `screenshot` writes two PPMs by different palette rules. Nine of `loop_::run`'s
eleven steps fork on the board for the same reason.

---

## The frame and cycle model

### Two frame periods, and they are not one constant

| Constant | Value (ns) | Where it comes from |
|---|---|---|
| `CPS1_FRAME_NS` | 16,768,000 | **Derived**: 8,000,000 / (512 × 262) = 59.6374 Hz. A whole number of nanoseconds because the pixel clock divides evenly. |
| `SF1_FRAME_NS` | 16,666,667 | **Asserted**: `sf.cpp:766` is `set_refresh_hz(60)`. 1e9/60 is 16,666,666.67, rounded **up** — a period a hair long, so a pacer would rather drop a frame than run early. |

⚠️ Pacing either board at the other's period is a 0.6% speed error that nothing
surfaces: inaudible, invisible, permanent. `Machine::frame_ns` is why the loop
never has to know which board it is pacing. `frontend::pace::FRAME_NS` holds
CPS-1's number as the default pacer's period.

### `Timing` is `Copy` configuration

| Field | CPS-1 (`cps1_10mhz`) | CPS-1 (`cps1_12mhz`) | SF1 (`sf1_8mhz`) |
|---|---|---|---|
| `cpu_hz` | 10,000,000 | 12,000,000 | 8,000,000 |
| `line_cycles` | `(640, 1)` | `(768, 1)` | `(3125, 6)` |
| `lines_per_frame` | 262 | 262 | 256 |
| `vblank_line` | 240 (`CPS_VBSTART`, `cps1.cpp:394-396`) | 240 | 240 |

**Two CPS-1 rows, because the board is not one clock.** MAME's `cps1_12MHz`
(`cps1.cpp:3959-3964`) calls `cps1_10MHz` and then overrides exactly one thing —
`m_maincpu->set_clock(XTAL(12'000'000))`, marked "verified on pcb". Champion Edition
uses it (`cps1.cpp:15084`); **every** World Warrior set uses `cps1_10MHz` (all 26 in the
block at `cps1.cpp:15024`). sfemu ran all three of its CPS-1 sets at 10 MHz until
2026-08-25, which gave CE 5/6 of its cycles — 83.3% speed, and nothing about the picture
or the sound says so.

`machine::Timing::for_game(name)` selects the row, in the same shape as
`BoardConfig::for_game`: `Option`, and **no default**, because 10 MHz is right for two of
the three names and quietly wrong for the third. It is a *second* table rather than a
field on the CPS-B row, because the two facts do not predict each other — `sf2ce` shares
`sf2`'s `cpsb_addr` and differs in clock, and `sf2eb` is the reverse.

**The refresh rate does not change.** The line rate comes from the pixel clock, not the
CPU: `8_000_000 / 512 = 15,625` lines per second on both boards, so 12 MHz is more cycles
inside the same 16.768 ms frame. `12_000_000 / 15_625 = 768` exactly — the denominator
stays 1 — and the frame budget goes from 640×262 = 167,680 to 768×262 = 201,216, a ratio
of exactly 6/5. `frame_ns()` is per-board and correctly untouched; a "fix" that sped up
the pacer instead would have been wrong about the screen, which is why the tests assert
`lines_per_frame` and `vblank_line` are unchanged.

`line_cycles` is a ratio and not a count because SF1's is not an integer: 8 MHz
over 15,360 lines per second is 3125/6, and rounding it to 520 or 521 is a 0.16%
error — "audible over a match, never broken enough to investigate". The number 512
is the raster *width*; using it as a cycle count would silently assume 61.035 Hz.

**The remainder is not in `Timing`.** `Timing` is `Copy`, and a moving remainder
inside it would let two copies of the same board's timing disagree about the
future. It lives in the machine, beside its other fractional clocks —
`RationalAccumulator`, which is also how the Z80's `715_909/3_125` T-states per
line and the OKI's per-YM-sample ratio are carried. That denominator is 5^5, so the
Z80 ratio is exact in `u32`.

### `step_instruction` owns the schedule

`Cps1::step_instruction` is the only body that advances the machine.
`run_scanline` loops it while `self.line == line`; `run_frame` loops
`run_scanline` for `lines_per_frame`. One body, so **a debugger steps the same
machine that runs** — a second, simpler stepping path is how a breakpoint comes to
behave differently from full speed.

What that one body owns:

- the start-of-line work, guarded so it happens once per line however many
  instructions the line takes;
- the object latch at vblank, taken from the frame schedule rather than from the
  caller, so the one-frame sprite delay is exact for any caller — a debugger
  single-stepping included;
- the per-step IRQ re-drive, `set_irq(if vblank_pending { 2 } else { 0 })`. The
  measured argument for doing it per step: a 640-cycle line budget fits about seven
  passes of a 90-cycle handler, so a level asserted once per line is a level a
  handler can miss;
- the `carry: i64` overshoot, so an instruction that runs past the end of a line is
  charged to the next one instead of being lost;
- the end-of-line Z80 catch-up. The 68000 runs a whole line, then the Z80 catches
  up. The discriminating test for that interleave is
  `a_latch_written_mid_line_reaches_the_z80_in_the_same_line`.

### The pacer

`frontend::pace::FramePacer` is sleep-free and catch-up-bounded. A host tick that
took longer than a frame owes the frames it missed, up to `MAX_CATCH_UP = 4`;
beyond that they are dropped and counted, because a machine that fell a second
behind should resync rather than fast-forward through a second of the game.
Pausing owes nothing — the clock is only read on a running tick.

### The audio chain's one inexact link

The board's sample rate is 3,579,545 / 64 = 55,930.390625 Hz. **No host rate is a
rational multiple of it**, so `machine::resample` interpolates linearly rather
than pretending a ratio exists. On the commonest host rate, 48 kHz, handing the
stream over unconverted would play it **14.2% slow** — 48,000/55,930.390625 =
0.858, which is 2.65 semitones flat. The mono mix is MAME's: weights 7, 7, 6 over
20, with the OKI's term at 3 because its value arrives already doubled. The ring
holds 100 ms and is prefilled to 50 ms. Measured drift is +6.3 ppm ± 59.6 ppm —
below the method's own resolution, so there is nothing to slew towards; the bound
that matters is the ~60 ppm jitter, 3.6 ms a minute, which a 100 ms ring absorbs.

---

## The save-state format

`crates/frontend/src/state.rs`, version 4:

| Offset | Bytes | Field |
|---|---|---|
| 0 | 8 | `MAGIC` = `b"SFEMU\0\0\x04"` — the last byte **is** `VERSION` |
| 8 | 4 | board tag, LE: `BOARD_SF2` = `b"SF2\0"` = `0x5346_3200`, `BOARD_SF1` = `b"SF1\0"` |
| 12 | 8 | payload length, LE |
| 20 | *n* | payload |
| 20+*n* | 4 | CRC-32 of the payload, LE |

Hand-rolled rather than `serde` + `bincode`, and that is the point: the reader and
the writer are **two independent lists**, so a field added to one and forgotten in
the other is a length mismatch rather than a silently truncated state. A derived
codec would agree with itself about a state it got wrong.

The version living in the magic's last byte means a state from another version
fails the *magic* check and the *version* check at once, and `StateError` reports
which. A state that is damaged, truncated, from another board, or from a future
version is refused rather than half-applied: the running machine keeps running and
the failure is a notice at session end.

Deliberately not in a state: the graphics viewer's layer mask.
`the_layer_mask_is_not_machine_state` keeps it out, because a mask that
round-tripped would come back with someone else's layers subtracted.

---

## Where session state lives, and why not in `frontend`

`frontend` has no filesystem, and neither does `machine`. Nor does `sfemu`'s
`frontend`-facing code: **persistence lives in `sfemu::loop_`**, which is the one
module that both knows the paths and is allowed to touch a disk.

Three files, all named from the ROM set's own path and all beside it:

| Ask | File | Rule on failure |
|---|---|---|
| `F5` / `F8` save and load a state | `sf2.sfs` | A failed save or a refused load is a notice at session end; the machine keeps running. |
| `F12` screenshot | `sf2.ppm` | Notice. |
| The key menu's choice | `sf2.keys` | A missing, unreadable or unrecognised file **is the default and says nothing** — a first run has no file, and a tag from a future version is not something a player can act on. A failed *write* is a notice, because the player asked for it. |

Per ROM set rather than one file for the program, which is the interesting half of
that decision: a CE session and an SF2 session can want different arrangements,
and the program still writes nothing outside the directory it was pointed at.

Each notice is reported **once**, not per frame: sixty identical lines a second is
a way of hiding the message inside the message.

---

## The loop's ordering

`loop_::run`'s eleven steps, in order, each with the consequence that fixes it
there (the module doc has the full reasoning):

1. read the elapsed time — first, so a slow save is not charged to the game as
   owed frames;
2. read the keys, into `Actions`;
3. quit, if asked;
4. hand the board its inputs — **level**-triggered, so this happens whether or not
   a frame runs;
5. reset, pause, save, load;
6. the debugger's keys — before the frames, so a breakpoint set this tick is
   honoured by this tick's frames;
7. the graphics viewer's keys, and its layer mask into `Video` — before the render,
   so this tick's frame is the masked one;
8. run the frames this tick owes, **stopping mid-frame at a breakpoint**, plus one
   instruction if `F4` asked;
9. hand this tick's samples to the host and report whether the emulator is paused —
   after the frames, and *every* iteration so a held pause stays reported;
10. render and present — **every** iteration including a paused one, or the window
    goes black the moment you pause. Overlays are drawn after the pen-to-ARGB
    conversion, because they are ARGB and the pens are not, and the graphics viewer
    goes over the debugger;
11. screenshot, then the title.

Two invariants in that list are held by tests rather than by care.
`watching_the_machine_does_not_change_it` runs four frames with the debugger on and
four with it off and compares cycles, every register, the beam, the interrupt trace
counters and all of RAM. `looking_at_the_video_does_not_change_the_machine` does the
same for the graphics viewer. Both live in `sfemu/src/loop_.rs`. Every debugger
panel reads through `peek_word`, which is `&self` and takes no side-effect path: a
dump scrolled over the input latch must not acknowledge an interrupt.

`peek_word` returns `Option<u16>`, and the `None` reaches the panel as `--` while
`Some(0xFFFF)` reaches it as `FFFF`. Conflating them would send a reader looking for
a chip that is not there; $800020 genuinely reads `FFFF` and is decoded, which is
what makes the distinction real rather than theoretical.

---

## Instruments are not state

A `machine::Trace` counts what the board saw — vblanks, acknowledges, CPS-A and
CPS-B writes, gfxram writes, sound latches, ROM writes, unmapped accesses with a
1,024-address cap and a dropped count. It is an **instrument**, not machine state:
nothing in the simulation reads it, it is not in a save state, and a run with
tracing and a run without produce the same machine.

That is what lets the no-`--play` report exist at all. A black window is
indistinguishable from a boot that hangs on the first instruction, whereas a count
of vblanks, acknowledges and video-register writes says which — and says it in a
form CI, a bisect and a commit message can hold.

The `Trace` prints its unmapped log's `dropped()` count when the cap has been hit,
because printing a total without it would read as a complete list when the
distinct-address cap has silently made it a sample.

---

## Global constraints

These hold across the whole workspace. Each is a line someone has already tried to
cross.

- **`#![forbid(unsafe_code)]` in all nine library crates** — `m68k`, `z80`, `video`,
  `ym2151`, `oki`, `machine`, `romset`, `frontend`, `testrom`. Not `deny`: `forbid`
  cannot be locally overridden. ⚠️ The binary `sfemu` and the dev harness
  `testrunner` do **not** carry the attribute, and nothing enforces its absence of
  `unsafe` beyond the fact that (measured 2026-08-24) neither contains the word.
  Adding it to both would close a real gap: `sfemu` is where the two FFI-shaped
  dependencies live, which is exactly where an `unsafe` block would be tempting.
- **Never panic on a guest address.** Every index into guest memory is produced by
  masking or a nonzero remainder, never by a bounds-checked slice index on guest
  arithmetic. A mis-emulated jump produces wild addresses as a matter of course, and
  an emulator that panics on one has turned a guest fault into a host crash.
  `no_address_in_the_whole_24_bit_space_panics` sweeps all of it.
- **No clock, no filesystem, no network** in `machine`, `video`, `m68k`, `z80`,
  `ym2151`, `oki` — and none in `frontend` either, which is why persistence had to
  go into `sfemu::loop_`.
- **`machine` must never depend on `romset`.** miniz_oxide and std.
- **Every expected value in a test is a literal**, written by hand from the
  arithmetic rather than recomputed from the code under test. A test that recomputes
  its own expectation asserts that the code equals itself.
- **`#![deny(rustdoc::private_intra_doc_links)]`** in `frontend` and `machine`: an
  intra-doc link from a `pub` item to a private one is a doc-build failure, which is
  why some items on this page and in those crates are referenced as plain code
  spans rather than links.
- **WASM and netplay are constraints, not stages**: no threads, no wall-clock
  access, no host I/O in the core, a frame-stepped API, complete serialization.
  One resource requirement follows and is worth knowing before spawning anything —
  `m68k`'s dispatch table is 512 KB built on the stack, so it needs at least 1 MB;
  measured, 640 KB aborts and 1,024 KB succeeds, `Box` does not avoid it, and a Rust
  stack overflow is a process abort rather than a catchable panic. Build the decoder
  once and pass `&Decoder`.

---

## What a green suite does not establish

This is the part of the architecture that is easiest to get wrong by being
pleased with it.

**All 1,604,000 Z80 vector cases would pass with every `#[test]` in the crate
deleted.** The vectors live in a separate crate and exercise instructions rather
than assertions. The same is true of the other three suites. A vector suite
measures the *core*; it says nothing about the hand-written tests.

`scripts/mutate.py` is what measures those. Its posture, and each rule's reason:

- Each mutant is **one exact string replacement** with a declared `KILL` or
  `SURVIVE`. Not a generated mutation: a hand-declared expectation is falsifiable
  and a generated one is a statistic.
- A kill records **which** test noticed, because a mutant killed only by a test
  with nothing to do with the mutated rule means the rule's own test asserts
  nothing.
- **Every set carries a control that must survive.** A pass where everything dies
  is more likely a broken harness than a thorough suite.
- **A no-op is a distinct verdict.** A pattern that matches zero or several times
  is reported as `NO-OP`, not as a kill — a mutant that never applied is not a
  mutant that was caught.
- **A timeout scores as a kill**, so `MUTANT_TIMEOUT_S` has to exceed a full
  rebuild. It is 600 s: the tree takes over 70 s to compile, and at 120 s the
  harness was scoring rebuilds as kills.
- **The pass owns the tree.** It edits files in place; a `cargo` run alongside it
  fakes kills, and killing the run strands live mutated code in tracked source.
  Commit first.

Measured 2026-08-24 from `scripts/mutate.py` itself: **299 mutants in 21 sets,
273 declared `KILL` and 26 declared `SURVIVE` — 23 controls and 3 proven
equivalents.** The three equivalents are named, because an unexplained survivor is
a gap:

| Survivor | Why it is genuinely equivalent |
|---|---|
| `debug/EQUIVALENT-the-drawing-guard-is-redundant` | `overlay::draw` with `Panels::none()` already draws nothing (`nothing_enabled_draws_nothing`). What the guard buys is a `buf.len()` assert not firing on a frame-sized buffer the caller has yet to fill — which the loop's own ordering makes unreachable. Kept in the set so that the day the guard becomes load-bearing, this line stops surviving and says so. |
| `gfxpanels/EQUIVALENT-the-cursor-reads-the-scroll-unsigned` | The two readings differ by exactly 65,536, and 64×8, 64×16 and 64×32 all divide 65,536 — so `map_axis`'s Euclidean wrap gives the same tile and the same offset either way, for all 65,536 values of all three layers. The precondition is pinned by `a_map_span_divides_the_register_range`. |
| `z80flags/EQUIVALENT-bit-parity-of-a-single-bit-mask` | `masked` is `v & (1 << b)` with `b ≤ 7`, so it holds at most one bit: zero bits is both even parity and zero, one bit is both odd parity and non-zero. Checked exhaustively over all 256 × 8 = 2,048 reachable pairs. |

The one finding from that harness worth repeating here, because it is an argument
about *decomposition* rather than about the emulator: forcing the YM2151's
`prepare()` gate eager dies to the vector suite and dies again to the unit
tests — but with the CSM cases skipped it **survives the suite entirely**. Eager
and lazy preparation agree bit-for-bit over 40,000 samples with CSM off. A suite
lacking those cases passes at 1,000/1,000 on a chip that is wrong.

**Seven tests are `#[ignore]`d**, and they are the only ones in the project. All
seven need a ROM set the user supplies, and all seven are gated on **one**
environment variable, `SFEMU_ROMS`. A second variable per game —
`SFEMU_ROMS_SF1` — **is forbidden by name**, and the forbid is recorded in
`crates/sfemu/tests/boot.rs:22` and `crates/sfemu/tests/sf1_boot.rs:21`: it
multiplies as games are added and leaves every test silently unrun when a name is
misspelled. The cost is that each test must decide whether the set it was handed is
one it can speak for, and skip with a reason when it is not.

⚠️ **Three of those seven have never been executed.** No SF1 set has been
available to this project, so the three `sf1_*` gated tests are written and unrun.
SF1's driver code and those tests stay in the tree, unexercised, and no SF1 set is
being sought.

And the eight claims **no** test here can make — the picture is right, the speed is
right, the controls respond on your keyboard, the overlay is legible, a tile is
recognisable, the swatches are distinguishable, the sound panel reads like a driver
running, and it sounds like Street Fighter II — are enumerated in
[README's "Eight things only you can check"](../README.md#eight-things-only-you-can-check).
They are the boundary of this architecture, and they are stated there rather than
here because they need a human at a window.

---

## Open questions

Recorded because an unrecorded open question becomes a wrong assumption.

- **Frame drops, at a rate that varies by an order of magnitude between sessions.**
  Four early windowed runs showed 2–3%; a 2026-08-24 session reported **3,246 of
  18,656 frames, 17.4%**. Emulation cost is ruled out: 1,200 frames of `sf2eb`
  headless took 1.08 s, i.e. 0.90 ms against a 16.768 ms budget (18.6×). Since a drop
  needs a host tick exceeding `MAX_CATCH_UP` frames (67 ms), the cause is on the
  window/present side. Never investigated.
- **`scripts/mutate.py`'s patterns outside `keys`, `menu` and `layout` have not
  been audited** for drift against the current source. A pattern that no longer
  matches scores `NO-OP`, which is visible — but only in a full run.

---

## How to check that a claim on this page is still true

Four procedures, by kind of claim. All of them are cheap; that is the point.

**A count** (crates, lines, tests, mutants) — recompute it:

```sh
find crates/machine/src -name '*.rs' -exec cat {} + | wc -l
grep -rho --include='*.rs' '#\[test\]' crates/machine | wc -l
cargo test --workspace 2>&1 | grep 'test result'
python3 - <<'EOF'
import importlib.util
s = importlib.util.spec_from_file_location('m', 'scripts/mutate.py')
m = importlib.util.module_from_spec(s)
try: s.loader.exec_module(m)
except SystemExit: pass
print(len(m.SETS), 'sets', sum(len(v[1]) for v in m.SETS.values()), 'mutants')
EOF
```

**A dependency edge** — read the manifest. Every edge in the table above is a line
in a `Cargo.toml` with a comment beside it. If the comment is gone, the constraint
is undocumented even if the edge is still absent.

**A constant** — mutate it by hand, run `cargo test --workspace --release`, record
which tests died, revert, and confirm a clean tree **before the next mutant**. A
mutant that kills nothing means the constant is a comment, not a check. Commit
first: reverting a hand-written mutant with `git checkout` destroys any other
uncommitted work in that file.

**A boundary** (`minifb` in one file, no clock in `frontend`, no `romset` in
`machine`) — run the test that asserts it, then break it deliberately and watch the
test fail. A confinement test that has never been seen red is a confinement test
that might be scanning an empty list.

For a claim sourced from MAME rather than from a test, the check is **re-reading
the cited line**; `docs/hardware/cps1-notes.md` has the `curl` block that restores
the three reference files and the warning that its line numbers are pinned to a
moving `master`.
