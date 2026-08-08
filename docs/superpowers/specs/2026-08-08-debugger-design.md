# Design: The debugger — an overlay you can stop the machine with (sfemu sub-project E2)

Date: 2026-08-08
Scope: Sub-project E2 of the sfemu arcade emulator
Status: approved
Depends on: A (68000 core), B (board and bus), C (video), E1 (frontend and loop)

---

## The problem

E1 made SF2 playable, and in doing so made every remaining bug invisible. The
tools it leaves you when the picture comes up wrong are a screenshot and a
scanline-granularity trace: you can see *that* the wrong sprite was drawn and
count how many interrupts were taken, but not *which instruction wrote it*.

The README says this in as many words — `[CPU halted]` in the title bar means the
68000 double bus faulted and "E2's debugger is the tool it wants". That is the
gap: the emulator can already tell you it has stopped believing in itself, and
cannot yet tell you why.

E2 closes it: stop the machine, look at the CPU and memory, step one instruction,
and set a breakpoint that stops it there again.

## What this is not

- **Not a symbolic debugger.** No symbols exist; a CPS-1 ROM set is stripped
  machine code. Addresses are the only names.
- **Not a memory editor, and not a register editor.** Writing state is a much
  larger surface than reading it (see "Why nothing is writable" below), and
  everything E2 exists for is answered by reading.
- **Not a tracer or a profiler.** `machine`'s `Trace` already counts frames,
  interrupts, acknowledges, unmapped accesses, and samples the PC per scanline.
  E2 displays what it has; it adds no new counters.
- **Not E3.** Tile browsers, tilemap views, palette views, and layer toggles are
  E3 and get their own spec. The line: E2 shows you the *CPU's* state, E3 shows
  you the *video's*. A palette is not a register.

## The surface: an overlay in the window

The debugger draws into the framebuffer, over the paused game, and the same
`present` call puts it on screen.

```
┌ sfemu ─────────────────────────────┐
│ D0 0000FFFF  A0 00FF8000  PC 001A4C│
│ D1 00000002  A1 00900000  SR 2704  │
│ 001A4C  move.w  d0,(a1)+   <-- pc  │
│ 001A50  dbra    d1,$1A4C           │
│ FF8000: 0000 FFFF 1234 ...         │
│ [paused]  line 128  cyc 4,203,776  │
└────────────────────────────────────┘
```

Chosen over a terminal REPL and over a headless batch mode, for one reason that
outweighs the rest: **an overlay is pixels, and pixels are testable.** The whole
architecture of E1 rests on the display boundary — every decision lives in
`frontend`, which has never heard of a window, because a window cannot be
asserted about. An overlay composed into a `Vec<u32>` that a test reads back
keeps the debugger on the testable side of that line. A REPL would put its
formatting behind `stdin`/`stdout` and its interleaving with the frame loop
behind a race.

The costs are real and accepted:

- **A bitmap font has to exist.** 384×224 is a small canvas; the font is 4×6 and
  the overlay is dense. See "The font" below.
- **The text is small.** The window opens at 3× so a 4×6 glyph is 12×18 physical
  pixels, which is legible. At 1× it is not.
- **The overlay covers the game.** That is what a paused game is for, and the
  overlay is toggleable.

## What it shows

Four panels, each independently toggleable, drawn in a fixed layout so a glance
finds the same thing in the same place:

1. **Registers.** `D0`-`D7`, `A0`-`A7`, `PC`, `SR` (hex, plus a decoded
   `T·S··III···XNZVC` flag line), `USP`, `SSP`, and the two prefetch words.
   `halted` and `stopped` are shown as flags when set, because a machine that is
   dead or asleep looks identical to one that is merely paused.
2. **Disassembly.** Sixteen instructions from a follow address, defaulting to the
   PC, with the executing instruction marked. `m68k::disasm::disassemble` already
   produces exactly this and takes a `read` closure, so E2 adds no disassembler.
3. **Memory.** A hex-and-ASCII dump of eight words per row from a follow address,
   defaulting to the stack pointer.
4. **Status.** Scanline, total cycles, frame count, and the `Trace` counters that
   indicate trouble: unmapped reads and writes, and the interrupt/acknowledge
   pair (an assert count running away from the ack count is the shape of a
   missed interrupt).

## Reads must not disturb the machine

**This is the single most important constraint in this spec, and it is not
obvious.** `Board::read_word` has side effects:

- `note_possible_ack(addr)` clears `vblank_pending` and increments `trace.acks`
  when the address is the vector-26 longword and an interrupt is outstanding.
  A memory panel scrolled over `0x000068` would **acknowledge the pending
  interrupt** — the debugger would make the interrupt it was opened to
  investigate disappear.
- An unmapped read records the address in `trace.unmapped_reads`. A memory panel
  parked on unmapped space would fill the counter the status panel displays with
  the debugger's own reads.

So the debugger reads through a **separate, non-mutating path**: `machine` gains
`Cps1::peek_word(addr) -> Option<u16>`, a `&self` method that decodes the same
address map and performs none of the bookkeeping. `None` means unmapped, rendered
as `--` rather than as `FFFF`, because "nothing decodes here" and "this decodes
and reads as all ones" are different facts and a debugger that conflates them
sends you looking in the wrong place.

`peek_word` taking `&self` is the enforcement mechanism, not a stylistic
preference: a `&mut self` version could call `note_possible_ack` and the compiler
would not object. The test for this is behavioural rather than structural —
`peek_word` over the whole vector table with an interrupt outstanding must leave
`vblank_pending`, `trace.acks`, and `trace.unmapped_reads` all untouched.

⚠️ **Duplicating the address map would be the risk this introduces, and it turns
out not to arise.** `peek_word` and `read_word` must agree, or the debugger shows
you a different machine than the one running. This spec expected to duplicate the
map and mitigate the drift with an agreement test, on the grounds that the I/O
ranges — the DIP-switch selector, the CPS-B self-test — compute rather than
store, so `peek_word` would have to reproduce the computation.

**Implementation note (this section is what changed).** They do compute, but
every one of those computations reads only `Board::inputs` and `Board::cfg`, so
all of it is `&self`-safe. `peek_word` therefore holds the *whole* map and
`read_word` delegates to it, adding only the bookkeeping. The two cannot
disagree, and the agreement test remains as a **pin** — it fails if a later
change ever splits them back into two maps — rather than as a mitigation for a
live risk. `note_possible_ack` also became unconditional in the same change: the
address it tests for is 0x68, which is in ROM space, so the ROM-range guard it
sat behind could only ever be redundant with the test inside.

## Why nothing is writable

No register edit, no memory poke, no forced branch. Three reasons, in order of
weight:

1. **`cpu.sr` cannot be written safely by an obvious route, and the codebase
   already says so at length.** `crates/m68k/src/cpu.rs` documents that assigning
   the `sr` field skips the stack-pointer swap and the mask that `set_sr`
   performs, producing "a state the core cannot reach on its own, which then
   propagates" — and it names this exact feature as the way it bites: "a
   debugger's *edit SR* box written against the field: the S bit is exactly the
   one a user of such a box wants to toggle, and it is the one that must not be
   toggled this way." Building the box is a task; building it correctly is a task
   whose test surface is every field that shadows another.
2. **A written machine is not a machine you can reason about.** The bug you are
   chasing is in the emulator or the ROM. A poke makes the state one neither
   produced, and every observation afterwards is about a third machine.
3. **Nothing E2 exists for needs it.** Every question in the problem statement —
   which instruction wrote that, why did it halt, is the interrupt being
   acknowledged — is a read.

Writes are not forbidden forever; they are out of scope here, and the reason is
recorded so a later spec can overturn it deliberately rather than by omission.

## Stepping

`machine` today exposes `run_frame` (262 scanlines) and `run_scanline` (~640
cycles, tens of instructions). Neither is fine enough: a breakpoint that stops
"somewhere in this scanline" cannot tell you which instruction wrote the wrong
sprite.

`Cps1` gains:

```rust
/// Runs exactly one 68000 instruction. Returns the cycles it took.
pub fn step_instruction(&mut self) -> u32;
```

and **`run_scanline` is refactored to call it in a loop.** One code path, so the
debugger cannot drift from the emulator — a `step_instruction` that forgot to
re-drive the IRQ level would single-step a machine that takes no interrupts,
which is a debugger that lies about the exact thing it is most often opened to
investigate.

What one instruction has to carry, all of it currently inside `run_scanline`'s
loop and none of it optional:

- **Re-drive the IRQ level before the step**, from `board.vblank_pending()`. The
  existing comment explains why this is per-step rather than per-line: the level
  is a level, nothing in the core clears it, and the acknowledge happens on the
  board.
- **Accrue `total_cycles`** by the cycles returned.
- **Charge the scanline budget**, so stepping instructions advances the beam. A
  `step_instruction` that left `carry` alone would let you single-step forever on
  scanline 0, and the video state would never move.

Crossing a scanline boundary mid-step is the case to get right: the beam advances
when the budget is exhausted, and vblank assertion and the object latch happen on
the line they happen on, whether the caller is stepping or running. The invariant
that makes this checkable: **N calls to `step_instruction` and one call to
`run_frame` must reach identical machine state** for a program whose instruction
count in a frame is known. That is a divergence test, and it is the one that
proves the refactor did not change the emulator.

⚠️ **This touches `machine`, so E2 re-gates on the vector suite at 127/127.**
`run_scanline` is on the path every board-level test runs through.

## The font

A 4×6 bitmap font, ASCII 0x20-0x7E, defined as one `[u8; 6]` per glyph with four
bits used per row. Written by hand, in this repository, for the same reason no ROM
is bundled: a font is someone's copyrighted work unless it is not, and 95 glyphs
at 4×6 is an afternoon.

4×6 is the smallest size at which hex digits stay distinguishable, which is the
only legibility requirement that matters here — `8` and `B` and `0` and `D` must
not be confusable when the whole point of the panel is reading addresses. That is
a testable claim, and it is tested: **every pair of glyphs must differ in at
least one pixel**, over the whole set. A font with two identical glyphs is a
debugger that displays the wrong address and looks right.

The font lives in `frontend`, not in `sfemu`: it is a decision about pixels, and
pixels are what `frontend` owns.

## Where the code goes

```
crates/frontend/src/font.rs      the 4x6 bitmap font and glyph blitting
crates/frontend/src/overlay.rs   panels: registers, disasm, memory, status
crates/frontend/src/debug.rs     the debugger's own state: enabled, panels,
                                 follow addresses, breakpoints
crates/machine/src/cps1.rs       step_instruction, peek_word (modified)
crates/sfemu/src/loop_.rs        the loop consults the debugger (modified)
```

**E2 makes no manifest change and adds no dependency.** Verified rather than
assumed: `frontend` depends on `machine`, `machine` does `pub use m68k`
(deliberately, so `frontend` reaches the core through `machine` rather than past
it), and `m68k`'s `std` feature — which gates `disasm`, because `Insn` holds a
`String` — is on by default. So `frontend` can call
`machine::m68k::disasm::disassemble` as the tree stands.

One consequence worth naming: that path is the *only* way `frontend` may reach
`m68k`. Adding `m68k` to `frontend`'s manifest would let it reach past `machine`
into the core, which `machine/src/lib.rs` documents as the thing the re-export
exists to prevent.

## Breakpoints

A `Vec<u32>` of addresses, checked before each instruction. Not a `HashSet`:
a human sets a handful, and a linear scan of four addresses beats a hash. The
comparison is against **the address of the instruction about to execute**, which
is *not* `cpu.pc`.

⚠️ **`cpu.pc` is four bytes beyond the instruction being executed**, because of
the two-word prefetch queue — the field's own documentation says so. A breakpoint
implementation that compares `cpu.pc == addr` fires four bytes late, or not at
all for a two-word instruction, and the symptom is a breakpoint that "doesn't
work sometimes". The executing instruction's address is `cpu.pc - 4`, and the
disassembly panel's marker has the same problem. One helper, used by both,
documented once.

This is exactly the class of defect this project has learned to test for
specifically: a breakpoint test whose program has one-word instructions only
cannot distinguish `pc` from `pc - 4` reliably enough to be believed. **The
fixture must contain a multi-word instruction at a known address**, and the
breakpoint must fire on it.

Stopping is a pause, not a halt: hitting a breakpoint sets the loop's paused
flag, so every existing pause behaviour (the overlay draws, the game does not
advance, `.` steps, `P` resumes) applies with no new machinery. A breakpoint at
the PC would otherwise re-fire immediately on resume; the rule is that a
breakpoint does not fire on the instruction the machine is currently stopped at.

## Controls

Added to E1's map, which has `F2`, `F3`, `F5`, `F8`, `F12`, `P`, and `.` already
taken:

| Key | Does |
|---|---|
| `F1` | Toggle the overlay |
| `F4` | Step one instruction (implies paused) |
| `F6` | Cycle which panel the follow address applies to |
| `F7` | Set or clear a breakpoint at the current PC |
| `PageUp` / `PageDown` | Scroll the focused panel's follow address |
| `Home` | Reset the focused panel's follow address to its default |

Edge-triggered, all of them, through E1's existing `Controls`: a held `F4` must
not step sixty instructions a second. `.` keeps its E1 meaning (step one *frame*)
and `F4` is the instruction-level sibling, because both are useful and a modifier
key is worse than a second key.

Verified free: E1's `Key` enum has 22 variants on bits 0-21, none of them these.
The six new ones take bits 22-27, which a `u32` `KeySet` holds with four to spare.
Three places must change together for each new key — the enum, `Key::ALL`, and
`Key::bit` — and `keys.rs` already carries the test that enforces it
(`all_lists_every_key_exactly_once` fails if a variant is added and not listed).
`bit` is a written-out `match` rather than an `as` cast, deliberately, so the new
variants must be given bits explicitly; `sfemu`'s `translate` is a total match and
will fail to compile until each is mapped, which is the design working.

## The verification problem

Same shape as E1's, and the same answer: everything that decides anything lives
in `frontend` and is tested against a buffer.

What is testable, and how:

- **Every glyph differs from every other.** The whole font, pairwise.
- **A rendered panel contains the expected text.** Not by re-deriving the text
  from the same formatter — by rendering to a buffer and reading the *glyphs*
  back out: a tiny recogniser that maps 4×6 pixel patterns back to characters,
  built from the font table by inversion. This is the discipline this branch has
  had to learn repeatedly — a test that derives its expectation from the thing
  under test cannot fail. Asserting `format!("D0 {:08X}", d0)` against the
  formatter's own output proves nothing; reading `"D0 0000FFFF"` off the pixels
  proves the glyphs, the layout, and the formatting at once.
- **The overlay does not disturb the game's pixels where it is not drawn.** A
  panel with a known bounding box, and every pixel outside it unchanged.
- **`peek_word` does not disturb the machine.** Behavioural, as above.
- **`peek_word` agrees with `read_word`** across the address map.
- **`step_instruction` × N equals `run_frame`.** Divergence, not comparison.
- **A breakpoint fires on a multi-word instruction at the right address.**
- **A breakpoint does not re-fire on resume.**

What is not testable here, and is therefore stated as a user check the way E1's
three are: whether the overlay is *legible* on a real display, and whether the
layout is usable. A test can prove the glyphs are distinct and the text is at the
right pixels; it cannot prove you can read it.

## Risks

1. **The `run_scanline` refactor changes emulator behaviour.** Highest-severity
   risk in this spec, because `machine` carries the 127/127 suite and every
   board test. Mitigated by the divergence test and by re-running the suite.
2. ~~**`peek_word` drifts from `read_word`.**~~ **Eliminated in implementation,
   not mitigated.** `read_word` delegates to `peek_word`, so there is one map and
   nothing to drift. There is no ongoing maintenance cost and no second place for
   a later change to the memory map to find. The agreement test stayed, as a pin
   against a future split.
3. **The overlay is unreadable at 384×224.** Cannot be settled here. The window
   is 3× by default and resizable, which is the mitigation; if it turns out
   unusable, the fix is a larger font or a scaled overlay, and both are local to
   `font.rs`.
4. **The recogniser is written from the same table it checks.** The glyph
   recogniser that reads text back off the buffer is built by inverting the font
   table, so a *transposed* font — two glyphs whose bitmaps are swapped — renders
   wrong and reads back wrong, and every panel test passes. This is the branch's
   characteristic defect wearing a new hat. Mitigated by pinning a handful of
   glyphs against hand-written literal bitmaps: not all 95, because that is
   transcription rather than testing, but every hex digit, since hex is what the
   panels are made of and a transposed `8`/`B` is exactly the failure that
   matters. The pairwise-distinctness test does not catch this — a transposition
   of two distinct glyphs is still distinct.

## Success criteria

E2 is done when, against a ROM set the user supplies:

- `F1` shows registers, disassembly at the PC, memory at the stack pointer, and
  the trace counters, over a paused game.
- `F4` advances exactly one instruction, and the disassembly marker moves to the
  next one.
- `F7` at an address, then resuming, stops the machine at that address again.
- The vector suite is 127/127 and both test profiles are green.
- Nothing in the debugger's reads changes what the machine does — provable by
  running a fixed number of frames with the overlay on and with it off, and
  getting identical state.

That last one is the criterion that matters most, and it is the one a debugger
most easily fails: the tool that observes the bug must not be part of it.
