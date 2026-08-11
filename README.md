# sfemu

A Street Fighter arcade emulator, built from the hardware up. Point it at a
CPS-1 ROM set you own and `--play` opens a window:

```bash
cargo run -p sfemu --release -- /path/to/your/sf2.zip --play
```

Eight sub-projects are complete: the **68000 core** (A), the **bus and timing
framework with a MAME ROM-set loader** (B), the **CPS-1 scanline renderer** (C),
the **Z80 audio CPU** (D1), the **YM2151 FM chip and the sound board's wiring**
(D2), the **frontend** — window, frame clock, keyboard, and save states (E1) — the
**debugger** (E2): `F1` for an in-window overlay, `F4` to step one instruction,
`F7` for a breakpoint — and the **graphics viewers** (E3): `F9` for a tile,
tilemap, palette and layer browser, `F10` to cycle it.

**There is still no sound**, and the reason is now narrower than it was. The whole
chain up to the samples exists: the Z80 executes SF2's driver from `audiocpu`,
reads the 68000's command latch, and programs a YM2151 that is sample-exact against
ymfm over 1,000 vector cases. Those samples then go into a buffer nothing drains.
D3 is what reaches a speaker — the OKI MSM6295's ADPCM, the mono mix the cabinet
actually gets, and resampling to the host's rate. The Street Fighter 1 driver (F)
is not built yet either.

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

## The Z80 core

Validated against the [SingleStepTests/z80][sstz] vector suite: **1,604 of 1,604
files green, 1,604,000 of 1,604,000 cases.** Per case: every register including
`IX`/`IY`, the shadow set, `I`, `R`, both interrupt flip-flops, `f` — with the two
**undocumented** flag bits, which the vectors compare on every case, so they are
neither optional nor a curiosity — every touched RAM byte, the T-state count, and
the bus access sequence in order.

Unlike `m68k`, this core carries per-instruction cycle costs. The 68000 obeys a
measured law (every bus access is four cycles, so a count falls out of the access
sequence); the Z80 obeys no such law — an opcode fetch is 4 T-states and a memory
access 3, with per-instruction internal cycles — so each handler returns its own
count, taken from the vectors rather than from a table someone typed.

**What a green suite here does not establish.** No vector reaches interrupt
acceptance: SingleStepTests drives instructions, and accepting an interrupt is not
one. So `NMI`, the maskable modes, the `EI` arming delay and `RETN`'s flip-flop
restore are covered by hand-written tests only — which is why `scripts/mutate.py`
has a `z80int` set whose survivors would mean genuinely unverified behaviour, and
why it is run rather than trusted.

[sstz]: https://github.com/SingleStepTests/z80

## The YM2151

There is no public vector suite for an FM chip, so this one is validated against
[ymfm][ymfm] — the BSD-3 implementation MAME itself uses — by generating 1,000
cases from it and requiring this core to match **every stereo sample and every
status-register read, exactly**: 1,000 of 1,000 cases, no tolerance, not a
correlation. The generator and the vectors are this repository's, and no Capcom
code is involved in either.

**A suite that passes proves nothing until you know it could have failed.** Four
premises about the vectors are therefore asserted alongside them: the cases are
audible (a suite of silence matches a chip that outputs silence), every case decays
after its key-off, the status trace actually varies, and the runner reports a
sample deliberately corrupted by the test itself. The 1,000 cases are also checked
to be 1,000 *different* cases.

The audibility threshold is 95%, not 100%, and the reason is worth stating because a
tolerance is usually where a test stops being able to fail: **993 of 1,000 cases are
audible**, and the seven that are not have a total level near the generator's cap
with a slow decay, which puts the whole case under the DAC's quantisation. That is
real, correct data. The premise this guards is a measured one — a purely random
register script produced **0** non-zero samples across 500 cases — so the failure it
exists to catch is off by two orders of magnitude from the threshold, not by 5%.

**The one measurement that shaped the design.** The chip prepares each operator's
cached state lazily. Forced eager, it produces bit-for-bit identical output over
40,000 samples — *unless* CSM is on with timer A running, which is a mode nothing
else in the suite exercises. So at least 100 CSM cases are required, and
`scripts/mutate.py` proves the requirement rather than asserting it: the mutant that
forces eager preparation dies to the suite, dies again to the unit tests, and with
the CSM cases skipped **survives the suite completely**. That surviving control is
the evidence that the 100 cases are load-bearing.

[ymfm]: https://github.com/aaronsgiles/ymfm

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

The same reasoning governs the debugger's font: it is **drawn in this repository**,
as ASCII art in `frontend/src/font.rs`, because a typeface is someone's copyrighted
work unless it demonstrably is not. Nothing to license, nothing to fetch.

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

# Mutation testing: 158 mutants, each an exact string replacement, each with a
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
| `F1` | Debugger overlay on / off |
| `F4` | Step one **instruction** |
| `F6` | Move the scroll focus: disassembly ⇄ memory |
| `F7` | Set / clear a breakpoint at the instruction shown |
| `PageUp` / `PageDown` | Scroll the focused panel |
| `Home` | Follow the machine again (memory: go to the stack pointer) |
| `F9` | Graphics viewer on / off |
| `F10` | Cycle the view: tiles, tilemap, palette, layers |
| `[` / `]` | Move within the view |
| `Enter` | Act on the view — cycle its tile layout or layer, or toggle a layer |

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
and `F1` is the tool it wants — the status panel says `HALT` and the registers show
where it went.

### Debugging it

`F1` draws the debugger into the emulated framebuffer, over the paused game. Four
panels, each independently toggleable in code and three of them on by default:

- **Registers** — D0-D7 beside A0-A7, then PC, SR, the cycle count, and the frame
  count. A7 comes from `a[7]`, never from the `usp`/`ssp` shadows, which are stale
  inside an exception handler — which is where you are when you are reading it. The
  PC shown is the *executing* instruction's, four bytes behind `cpu.pc`, because the
  68000 prefetches two words.
- **Disassembly** — eight instructions from the follow address, `>` on the
  executing line and `*` on a breakpoint. It follows the machine until you scroll;
  `Home` makes it follow again.
- **Memory** — twelve rows of four words. Off by default: it needs an address to be
  worth its width, and `F6` is how you ask for it.
- **Status** — the flags `XNZVC`, the beam position, and `HALT` or `STOP` when the
  CPU is one of those.

`--` in the memory view means **nothing decodes at that address** — no chip
answered. That is a different fact from `FFFF`, which means something answered and
read as all ones, and the two are not conflated: conflating them sends you looking
for a chip that is not there. $800020 genuinely reads `FFFF` and is decoded, which
is what makes the distinction real rather than theoretical.

**Nothing here is writable, and that is a design decision rather than a missing
feature.** Every panel reads through `Cps1::peek_word`, which is `&self` and takes
no side-effect path: a dump scrolled over the input latch at $800000 must not
acknowledge an interrupt, and a listing pointed at $68 must not consume the vector
it is there to explain. A debugger that perturbed the machine while you watched it
would make every intermittent bug unreproducible, which is the class of bug you
open a debugger for. `watching_the_machine_does_not_change_it` in
`sfemu/src/loop_.rs` is the test that holds the line: four frames with the overlay
on and four with it off, compared on cycles, every register, the beam, the
interrupt trace counters, and all of RAM.

`F4` steps one instruction; `.` still steps one whole frame. Both are needed and
they are not interchangeable — a frame is 167,680 cycles, which is where a bug
usually is, and an instruction is where you can see it. A breakpoint stops the
machine *mid-frame*, at the instruction, not at the next frame boundary.

### Looking at the graphics

`F1` answers questions about the 68000. `F9` answers the other half: the screen
is wrong, and the question is *which stage* is wrong. `F10` cycles four views,
one at a time, each filling the frame — and `F1`'s panels draw on top of it, so
you can read a register beside the tile it produced.

- **Tiles** — the graphics ROM as a grid, in greyscale. `Enter` cycles the four
  layouts (8×8, 8×8-odd, 16×16, 32×32) and `[`/`]` page by exactly one screenful
  of whichever is shown. *Is the tile in the ROM at all, and does it decode?*
- **Tilemap** — one scroll layer's table around a cursor: the table base, the
  signed scroll, eight by eight codes, the cursored code's colour scheme, flip
  bits and tile group, the ROM offset the bank mapper gives it, and the tile
  itself. `Enter` cycles the layer, `[`/`]` walk the cursor. Until you move it,
  the cursor follows the tile the renderer fetches for the visible top-left
  pixel. *Is the map pointing at the tile you meant?*
- **Palette** — all 3072 entries as swatches, with the cursored one's raw hex and
  its page, and `0BFF` named as the background pen. *Is the palette entry the
  colour you meant?*
- **Layers** — the four depths back to front: what the hardware enables, what you
  have masked, which depth each layer draws at, and which feeds the sprite
  occlusion mask. `[`/`]` select a row and `Enter` subtracts that layer from the
  picture. *Is the layer even enabled, and is it where you think in the stack?*

Three things the screen alone will not tell you:

**Tiles are greyscale on purpose.** A colour scheme is the palette's reading of
the ROM, and the palette has a view of its own. Tinting the browser would make a
wrong decode and a wrong palette look the same, and telling those two apart is
what the browser is for.

**`----` in the tilemap view is the bank mapper saying no.** The tile is *absent*,
not wrong: no bank range covers that code, so `draw_tilemap` skips it silently.
That is the one failure the composed picture cannot show — which is why a viewer
that printed the mapper's `None` as `0000` would be worse than useless, sending
you to look at tile 0.

**Turning a layer off changes the picture, not the machine.** The mask reaches
`Video` and nothing else; the 68000 runs the same cycles, writes the same memory,
and takes the same interrupts either way. `looking_at_the_video_does_not_change_the_machine`
in `sfemu/src/loop_.rs` is the test that holds that line, and
`the_layer_mask_is_not_machine_state` keeps the mask out of save states, because a
mask that round-tripped would come back with someone else's layers subtracted. The
consequence worth remembering: **an `F12` screenshot taken with a layer off is
missing that layer.**

And the asymmetry: **you cannot turn a layer *on*.** The mask subtracts only, and
that is structural rather than a limitation not yet lifted — `Video::render`
combines the hardware's answer and the mask with `&&`, hardware first. Forcing a
layer on would draw a tilemap from a base register the game never set up, which is
garbage that looks exactly like the tile-decode bug the viewer is there to rule
out.

`F9` hides the box without restoring the layers you subtracted: "show me the game
with scroll 1 off" is the whole point of a layer mask, and it is unreachable if
closing the box turns everything back on.

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

### Seven things only you can check

Everything in this repository is tested without a display, which leaves exactly
seven claims no test here can make. Run it against your own ROM set and look:

1. **Does the window show Street Fighter II?** A test can assert the framebuffer
   changed, that a save state round-trips, and that a pen becomes the right ARGB
   word. None of that establishes that the picture is right.
2. **Does it run at the right speed?** The pacer is tested against a scripted
   clock, which proves the arithmetic and not the wall clock.
3. **Do the controls respond?** The key map is tested against the board's
   documented port bits, which proves `A` sets jab and not that `A` reaches the
   game.
4. **Is the debugger overlay legible on your display?** The font is 4×6 pixels,
   scaled by whatever the window is. A test can prove that no two of the 95 glyphs
   share a bitmap, that the sixteen hex digits are the bitmaps drawn here, that the
   blank column between characters is really blank, and that each panel's pixels land
   where the layout says. **None of that is "you can read it."** Distinguishing `8`
   from `B` at a glance in a moving window is the claim, and it is yours to make.
5. **Is a tile recognisable in `F9`'s browser?** A test can prove the pixels are
   the ROM's pens, in greyscale, at the cells the layout says. Whether a 16×16 tile
   in a 384-wide window reads as a character's fist is not a property of a buffer.
6. **Are the palette swatches distinguishable?** 3072 swatches on a 384×224 frame
   is about 5×4 pixels each. A test can prove every one holds the right colour
   through the same conversion the game's own frame goes through; it cannot prove
   two adjacent near-identical entries look different to you.
7. **Is the sound panel legible, and do its numbers look like a driver running?**
   A test reads the panel's own pixels back and proves the Z80's T-states, the two
   latch bytes, and the chip's register writes are drawn where the layout says. It
   cannot tell you whether a latch byte changing sixty times a second beside a
   register count climbing in the thousands reads, to you, as SF2 playing music —
   which is the only thing that number is for until D3 gives you the audio itself.

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
                     CPS-A/B registers, scanline renderer, and the subtractive
                     layer mask the viewer drives. No dependencies.
crates/z80/          the sound board's CPU core, on m68k's terms: no
                     dependencies, no unsafe, no clock access, no_std-friendly.
                     Includes a disassembler cross-checked against the core.
crates/ym2151/       the FM chip, on the same terms: no dependencies, no unsafe,
                     no clock access, no_std-friendly. Four tables built from
                     closed forms and checksummed, and a lazily-prepared operator
                     state the CSM vectors exist to pin.
crates/machine/      the board: memory map, bus, interrupts, scheduler, inputs,
                     the sound board and its rational Z80 clock, snapshot and
                     restore. Depends on m68k, video, z80 and ym2151 — all four
                     dependency-free, so the display boundary below still holds.
                     Three of the four build for a bare-metal target (verified on
                     thumbv7em-none-eabihf); `video` allocates a framebuffer and
                     needs `alloc`, which is a different claim and is not made
                     for it above.
crates/romset/       the MAME ROM-set loader: zip or a directory, CRC-checked,
                     interleaved into the regions the board wants.
crates/frontend/     every frontend decision, with no window: frame pacing, the
                     key map, pen-to-ARGB, the save-state file format, the
                     debugger's state, the graphics viewer's state and its four
                     views, and the 4x6 font drawn in this repository.
crates/sfemu/        the binary. The only crate that names a windowing library,
                     and only in one file (a test enforces it).
crates/testrunner/   dev-only harness for the external vector suite.
scripts/mutate.py    the mutation harness: 210 mutants over the above in 18 sets,
                     each an exact string replacement with a declared
                     expectation. 189 killed, 21 declared survivors (18 controls
                     and 3 proven equivalents), 210/210 as expected.
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
exactly four cycles, so there is no cycle table in `m68k` — a count falls out of
the access sequence a handler already has to schedule.

**And a green suite is not a tested codebase.** All 1,604,000 Z80 cases would pass
with every `#[test]` in the crate deleted, because the vectors live in a separate
crate and exercise instructions rather than assertions. `scripts/mutate.py` is what
measures the hand-written tests: each mutant is one exact string replacement with a
declared expectation, and a kill records *which* test noticed — because a mutant
killed only by a test with nothing to do with the mutated rule means the rule's own
test asserts nothing. Every set carries a control that must survive, so a clean
pass is distinguishable from a harness that reports success without running.

## Roadmap

| | Sub-project | Status |
|---|---|---|
| **A** | Workspace and M68000 core | **complete** — 127/127 groups, 317,500/317,500 cases |
| **B** | Bus/timing framework, MAME ROM-set loader | **complete** — first execution of real board code |
| **C** | CPS-1 video: tilemaps, sprites, palettes, CPS-A/B registers, scanline renderer | **complete** — the largest piece, and where SF2 becomes visible |
| **D1** | Z80 audio CPU | **complete** — 1,604/1,604 files, 1,604,000/1,604,000 cases. Still silent: a CPU with no chip attached |
| **D2** | YM2151 FM, and the sound board's wiring | **complete** — 1,000/1,000 vector cases against ymfm, sample-exact. Still silent: the samples reach no speaker |
| D3 | OKI MSM6295 ADPCM, mixing, host audio | the one that ends "there is no sound" |
| **E1** | Frontend: window, frame clock, keyboard, save states | **complete** — `--play` |
| **E2** | Debugger: single-step, breakpoints, disassembly, register and memory views | **complete** — `F1`, in-window, and it does not perturb the machine |
| **E3** | Graphics viewers: tile browser, tilemap and palette views, layer toggles | **complete** — `F9`, four views, and the mask subtracts only |
| F | Street Fighter 1 driver | a second board against a proven core |

E was split because its three surfaces are independent and only the first changes
what the project *is* rather than what can be inspected about it. E3 came last for
a reason rather than by preference: a tile browser's value is mostly in stopping
the machine at the frame you care about, which is E2's stepping — so E3 waited for
it, and then took it.

**D is split for the same reason, and the number that settled it is 1,604.** That
is how many vector files the Z80 suite has, against the 68000's 127 — and the
68000 took 16,462 lines and a spec of its own. A Z80 core, an FM synthesizer, and
a host audio path are three unrelated subsystems; asking one review pass to gate
all three is what the original decomposition was written to avoid. **D1 and D2 are
done**, and both were silent by design: D3 is the one that ends "there is no
sound."

The split paid off in a way worth recording, because it is an argument for the
decomposition rather than for the emulator. D2's 1,000-case suite includes at least
100 cases that enable CSM with timer A running — and that requirement exists
because the YM2151's `prepare()` gate is *invisible* without them: eager and lazy
preparation agree bit-for-bit over 40,000 samples with CSM off, so a suite lacking
those cases passes at 1,000/1,000 on a chip that is wrong. The mutation harness
measures exactly that: forcing the gate eager dies to the suite, and dies again to
the unit tests, but with the CSM cases skipped it *survives* the suite entirely.
A review pass covering the Z80, the FM chip, and the audio path at once would not
have had the attention to spend on one gate in one of them.

WASM and netplay are not stages. They are constraints on A–D: no threads, no
wall-clock access, no host I/O in the core, a frame-stepped API, and complete
serialization. Honouring that from the start makes both nearly free.

## License

Not yet chosen. The `m68k` core contains no third-party code.
