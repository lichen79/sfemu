# sfemu

A Street Fighter arcade emulator, built from the hardware up. Point it at a
CPS-1 ROM set you own and `--play` opens a window:

```bash
cargo run -p sfemu --release -- /path/to/your/sf2.zip --play
```

Four sub-projects are complete: the **68000 core** (A), the **bus and timing
framework with a MAME ROM-set loader** (B), the **CPS-1 scanline renderer** (C),
and the **frontend** — window, frame clock, keyboard, and save states (E1). The
Z80 and audio (D), the debugger (E2), the graphics viewers (E3), and the Street
Fighter 1 driver (F) are not built yet: **there is no sound.**

## The 68000 core

The core is validated against the [SingleStepTests/m68000][sst] vector suite:
**127 of 127 groups green, 317,500 of 317,500 cases**. Per case, all of the
following must match exactly — final registers, both stack pointers, SR, PC, both
prefetch queue words, every touched RAM byte, the total cycle count, and the bus
access sequence in order, with an address error's aborted access confirmed
*absent* from the bus log.

That list is not everything the vectors record. **One field is deliberately
unchecked: each transaction's function code**, which the suite supplies with four
distinct values over 1,450,409 non-idle transactions (user/supervisor ×
data/program). The `Bus` trait takes an address and nothing else, so the core
never states a function code and the harness has no value to compare — checking it
means widening `Bus`, not adding an assertion. Concretely, a green suite does
*not* establish that the core drives the right address space, so a vector fetch
issued as user-data rather than supervisor-data would pass all 317,500 cases.
Sub-project B is where that starts to matter, on a board deriving a chip select
from FC.

[sst]: https://github.com/SingleStepTests/m68000

## This project ships no ROMs, and never will

**No Street Fighter ROM, no Capcom code, and no diagnostic binary is contained
in, downloaded by, or committed to this repository.** sfemu emulates real
hardware; you supply the game data yourself, as a MAME-format ROM set given by
runtime path. There is no bundled fallback and no environment-variable escape
hatch, by design.

Legal ways to obtain a ROM set you may use:

- **Capcom Arcade Stadium** (Steam) — includes Street Fighter II and ships the
  original ROM data.
- **Capcom Fighting Collection** — likewise.
- **Dumping a board you own.** The most defensible route, and the only one that
  gets you a set for hardware Capcom has not re-released.

`testdata/` is gitignored, and no ROM or test data is ever committed. The test
vectors are a separate matter: they are freely licensed, machine-generated, and
contain no game code — but they are still fetched at runtime rather than vendored.

If a vector file is missing, the harness fails loudly, naming the file and the
command that fetches it. It does not skip, warn, or silently pass.

## Getting started

```bash
# Fetch the test vectors (~138 MB (132 MiB) over 127 files, into gitignored testdata/).
# Shells out to curl; no HTTP dependency is taken for a once-per-checkout job.
cargo run -p testrunner --bin fetch --release

# Unit tests, plus one test per suite group. Both profiles: `--release` is
# where the timing law is measured, and debug is where `debug_assert!` is
# evaluated. Neither run subsumes the other.
cargo test --workspace --release
cargo test --workspace

# The full-suite report: a per-group table, then the headline figures.
# Exits nonzero if any group is red.
cargo run -p testrunner --bin report --release

# Throughput. Read the caveat below before quoting a number from it.
cargo bench -p m68k

# Mutation testing: 71 mutants, each an exact string replacement, each with a
# declared KILL or SURVIVE. Every set carries at least one control that must
# survive — a pass where everything dies is more likely a broken harness than a
# thorough suite. Commit first: it edits files in place.
python3 scripts/mutate.py --all
```

The gate is **both** profiles. `--release` runs 210 `m68k` unit tests and 128
harness tests — one per suite group, plus one that fails if a file appears in
`testdata/` without a corresponding registered group, so adding a vector file
cannot silently go unrun.

⚠️ **The debug run is not redundant, and leaving it out hid a live defect.**
`[profile.release]` does not enable debug assertions, so under a release-only
gate the core's 9 `debug_assert!`s were never evaluated — including the one in
`ops::alu`'s `run_tail` that a whole task chose *over* deleting the field it
checks. Measured: inverting that assertion to `debug_assert!(!plan.writes, …)`,
which must fire on every write-back plan, leaves the release run entirely green
across all 11 binaries and all 128 groups, while failing 21 of 210 in debug. The
debug run costs 6 s, because `[profile.test] opt-level = 2` compiles the
dependencies optimised anyway; the suite itself takes 0.08 s of it.

## Playing it

```bash
# A window, at 59.6374 Hz, until you close it or press Esc.
cargo run -p sfemu --release -- /path/to/your/sf2.zip --play

# Somewhere other than beside the ROM set for the save state:
cargo run -p sfemu --release -- /path/to/your/sf2.zip --play --state /tmp/mine.sfs

# Without --play: run a fixed number of frames and print a report. No window,
# which is what CI and a bisect want.
cargo run -p sfemu --release -- /path/to/your/sf2.zip 600 --ppm frame.ppm
```

`--release` is not optional advice here: a debug build does not hold 59.6 Hz.

| Key | Does |
|---|---|
| Arrows | P1 stick |
| `A` `S` `D` | P1 jab, strong, fierce |
| `Z` `X` `C` | P1 short, forward, roundhouse |
| `5` / `1` | Coin 1 / Start 1 |
| `6` / `2` | Coin 2 / Start 2 |
| `F2` | Test switch — hold it at boot for the service menu |
| `F3` | Reset the machine |
| `P` | Pause / resume |
| `.` | Step one frame while paused |
| `F5` / `F8` | Save state / load state |
| `F12` | Screenshot, as a binary PPM |
| `Esc` | Quit |

Punches sit on the top row and kicks directly under them, matching a real
six-button cabinet. **Player 2 is not mapped**, deliberately: two players on one
keyboard needs a second ten-key cluster and every arrangement of one is bad. The
board's `Inputs` already carries P2, so a gamepad or netplay supplies it later.

Save states go beside the ROM set — `sf2.zip` next to `sf2.sfs` — so two games
never share one, and screenshots to `sf2.ppm` the same way. One state file, not a
numbered series: F5 overwrites. A state is tagged with the board it came from and
refused by a build of another, and a state that is damaged, truncated, or from a
future version of the format is refused rather than half-applied — the machine you
are playing keeps running and the title bar is not where you find out. Failures
print as `notice` lines when the session ends.

The title bar carries `[paused]`, `[CPU halted]`, and a dropped-frame count when
there is one. `[CPU halted]` means the 68000 double bus faulted and the loop is
still running so you can see it: that is a bug in this emulator or in the ROM set,
and E2's debugger is the tool it wants.

### Speed

The frame budget is 16.768 ms — CPS-1 runs at 8,000,000 / (512 × 262) =
59.6374 Hz, a whole number of nanoseconds because the pixel clock divides evenly.
Against that, **0.749 ms per frame** for CPU + render + RGB conversion: a 22×
margin. CPU alone is 0.095 ms and the renderer is the bulk at 0.430 ms.

Read that the way the 68000 bench section below asks you to read its numbers: it
is one machine (the author's, 2026-08-08), with synthetic worst-case content — all
three layers on, a full 256-record object table, opaque tiles, a 4 MB graphics ROM
— and it is quoted from this sub-project's spec rather than freshly measured here.
There is no `machine` or `video` benchmark in the tree to re-run it with. It is not
a performance guarantee, and no optimisation work was done on the strength of it.

The pacer is what that margin buys: sleep-free and catch-up-bounded. A host tick
that took longer than a frame owes the frames it missed, up to four; beyond that
they are dropped and counted, because a machine that fell a second behind should
resync rather than fast-forward through a second of the game. Pausing owes nothing
— the clock is only read on a running tick.

### Three things only you can check

Everything in this repository is tested without a display, which leaves exactly
three claims no test here can make. Run it against your own ROM set and look:

1. **Does the window show Street Fighter II?** A test can assert the framebuffer
   changed, that a save state round-trips, and that a pen becomes the right ARGB
   word. None of that establishes that the picture is right.
2. **Does it run at the right speed?** The pacer is tested against a scripted
   clock, which proves the arithmetic and not the wall clock.
3. **Do the controls respond?** The key map is tested against the board's
   documented port bits, which proves `A` sets jab and not that `A` reaches the
   game.

If the picture comes up wrong, `F12` gives you a frame to look at and the trace
counters in the no-`--play` report give you the interrupts and bus activity behind
it.

### The benchmark is a liveness check, not a performance gate

`cargo bench -p m68k` measures a mixed workload — register ops, two memory
accesses, a shift, and a taken branch — and reports simulated MHz. CPS-1 clocks
its 68000 at 10 MHz; the core runs it at a **72x-82x margin** (719-820 MHz
simulated over nine runs on the author's machine, at 9.33 cycles per
instruction). The spread is host load, and the low end is reproducibly the first
run after a build — quote the range, not one sample. Of the three numbers the
bench prints, only the 9.33 cycles/instruction is stable across runs, because it
comes from the cycle model rather than the wall clock.

Both the MHz **and the margin** are that machine's. A reviewer's host measured
267.6 MHz, a 27x margin — outside the band by 3x — and the assertion passed,
correctly, because it is `>= 10.0`. Read "72x-82x" as "a very large margin on one
machine", not as a range this bench holds anywhere.

A 72x margin will not catch a 5x regression, or a 20x one. What the assertion
catches is "the core stopped executing", and — via a non-degeneracy census the
bench prints before measuring — "the core is still executing, but no longer
executing *this*". A throughput figure is meaningless until the workload is known
non-degenerate: the same MHz prints just as happily for a one-instruction spin
loop. Treat a green bench as evidence of liveness and of the mix, never as a
performance guarantee.

## Layout

```
crates/m68k/         the CPU core: no dependencies, no unsafe, no clock access,
                     no globals. no_std-friendly. Optional serde and disasm.
crates/video/        CPS-1 graphics: tiles, three layers, sprites, palettes,
                     CPS-A/B registers, scanline renderer. No dependencies.
crates/machine/      the board: memory map, bus, interrupts, scheduler, inputs,
                     snapshot and restore. Depends on m68k and video.
crates/romset/       the MAME ROM-set loader: zip or a directory, CRC-checked,
                     interleaved into the regions the board wants.
crates/frontend/     every frontend decision, with no window: frame pacing, the
                     key map, pen-to-ARGB, the save-state file format.
crates/sfemu/        the binary. The only crate that names a windowing library,
                     and only in one file (a test enforces it).
crates/testrunner/   dev-only harness for the external vector suite.
scripts/mutate.py    the mutation harness: 71 mutants over the above, each one
                     an exact string replacement with a declared expectation.
docs/hardware/       what the vectors proved about the hardware, with evidence.
docs/superpowers/    design specs and implementation plans.
testdata/            gitignored; fetched vectors.
```

**The display boundary is the load-bearing line in that list.** A window cannot be
asserted about — `cargo test` has no display, and "the right pixels reached the
glass" is not something a test can read back — so every decision lives in
`frontend`, which has never heard of a window, and `sfemu/src/display.rs` makes no
decisions at all. `frontend` also reads no clock: the pacer is *given* the elapsed
nanoseconds, which is what lets a test drive it through a stalled host. The one
real clock read in the project is behind that boundary.

`m68k` knows nothing about Capcom hardware, which is what makes it testable
against third-party vectors and WASM-safe by construction. All state lives in
`M68k`, which derives `Clone` and `PartialEq`; every memory access goes through
the `Bus` trait. There is no wall-clock access, no randomness, and no interior
mutability — the properties that make save states, WASM, and rollback netplay
cheap later rather than a rewrite.

"WASM-safe by construction" is about the *interfaces* — no threads, no clock, no
host I/O — and it does not extend to one resource requirement worth knowing
before you spawn anything. **The dispatch table is 512 KB and `Decoder::new()`
builds it on the stack, so it needs at least 1 MB;** measured, 640 KB aborts and
1024 KB succeeds, `Box` does not avoid it, and a Rust stack overflow is a process
abort rather than a catchable panic. `wasm32`'s default 1 MB stack clears that bar
but spends most of its margin on it, and the 8 MB main thread is why this never
shows up in testing. Build the decoder once on the main thread and pass
`&Decoder` to `step_with`. `decode.rs` has the full note.

## What the vectors actually establish

`docs/hardware/68000-notes.md` is the durable output of this sub-project, and it
is written to be trusted by the five sub-projects that build on it. Two habits it
follows throughout, both learned the hard way:

- **Every claim is marked measured or extrapolated, with its denominator.** A
  count without its scope reads as universal; `3,160/3,160` and `43,483/43,483`
  were the same law measured over different populations, and stating the first
  without its scope made it look like a contradiction of the second.
- **Every `0/N` has a control that must produce output.** "No case halts" is only
  informative once you have shown the query can see a halt where one exists.
  Several genuine gaps in the suite's coverage were found this way, and they are
  documented as gaps rather than papered over.

The central result is the timing law: `cycles = 4 × (non-idle bus accesses) +
(idle cycles)`, which holds in 317,500 of 317,500 cases. Every bus access is
exactly four cycles, so there is no cycle table in this codebase — a count falls
out of the access sequence a handler already has to schedule.

## Roadmap

| | Sub-project | Status |
|---|---|---|
| **A** | Workspace and M68000 core | **complete** — 127/127 groups, 317,500/317,500 cases |
| **B** | Bus/timing framework, MAME ROM-set loader | **complete** — first execution of real board code |
| **C** | CPS-1 video: tilemaps, sprites, palettes, CPS-A/B registers, scanline renderer | **complete** — the largest piece, and where SF2 becomes visible |
| D | Z80 and audio: YM2151, OKI MSM6295 ADPCM | deferrable; CPS-1 sound is a fire-and-forget latch |
| **E1** | Frontend: window, frame clock, keyboard, save states | **complete** — `--play` |
| E2 | Debugger: single-step, breakpoints, disassembly, register and memory views | next |
| E3 | Graphics viewers: tile browser, tilemap and palette views, layer toggles | after E2 |
| F | Street Fighter 1 driver | a second board against a proven core |

E was split because its three surfaces are independent and only the first changes
what the project *is* rather than what can be inspected about it. E3 is last for a
reason rather than by preference: a tile browser's value is mostly in stopping the
machine at the frame you care about, which is E2's stepping.

WASM and netplay are not stages. They are constraints on A–D: no threads, no
wall-clock access, no host I/O in the core, a frame-stepped API, and complete
serialization. Honouring that from the start makes both nearly free.

## License

Not yet chosen. The `m68k` core contains no third-party code.
