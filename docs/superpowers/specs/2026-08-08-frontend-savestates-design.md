# Design: Frontend and save states — a window you can play (sfemu sub-project E1)

Date: 2026-08-08
Status: Approved (design calls made autonomously under a standing instruction to
proceed without check-ins; every call and its rationale is recorded inline)
Scope: Sub-project E1 of the sfemu arcade emulator

## Context

Sub-projects A, B, and C are complete:

- **A** — `crates/m68k`, a cycle-counted 68000, at 127/127 groups and
  317,500/317,500 cases of the SingleStepTests/m68000 vector suite.
- **B** — `crates/romset` and `crates/machine`: a MAME-format loader, the CPS-1
  memory map, the scanline scheduler, and the vblank interrupt.
- **C** — `crates/video`: tilemaps, sprites, palette, priority, and a 384×224
  framebuffer of palette pens plus an RGB conversion.

`sfemu <rom-set> [frames] --ppm out.ppm` runs the board and writes one frame to a
file. **Nothing has ever displayed a frame, and nothing has ever pressed a
button.** The renderer's only end-to-end check is a human opening a PPM.

E1 closes that: a window that runs at 59.63 Hz, a keyboard wired to the board's
inputs, and save states.

### The ROM constraint, restated

**No ROM is bundled, fetched, downloaded, or committed, by any code in this
repository, for any purpose — including diagnostics and test fixtures.** Unchanged
from A, B, and C, and it has a specific consequence for E1: the frontend's tests
may not boot a game. Every automated test here drives a synthetic 68000 program
this repository writes, or drives the frontend's pure functions directly. No URL to
any ROM appears anywhere in the repository, and no test reads an environment
variable to decide whether to run — with the single pre-existing exception of
`crates/sfemu/tests/boot.rs`, whose justification is that there is no command we
may legally put in a failure message.

## Scope check: the README's "E" is three sub-projects, not one

The roadmap row reads "Frontend, debugger, save states — step, breakpoints, VRAM
and tile viewers". That is three independent subsystems with three different risk
profiles, and bundling them would produce a plan whose first half is untestable
until the second half exists. Decomposed:

- **E1 (this spec).** The window, the frame clock, the keyboard, save states,
  screenshots. Deliverable: **SF2 is playable.**
- **E2.** The debugger: single-step, breakpoints, a disassembly view, register and
  memory inspection. Builds on E1's loop but is a separate surface, and `m68k`
  already ships `disasm`.
- **E3.** The graphics viewers: tile browser, tilemap and palette views, layer
  toggles. Only worth building once E2's stepping exists, because a viewer's value
  is watching state change one frame at a time.

E1 first, because it is the only one of the three that changes what the project
*is* rather than what can be inspected about it. E2 and E3 get their own specs.

---

## The verification problem, and the answer

Each sub-project has had a different answer to "how do you know this is right",
and stating E1's before the architecture is what keeps the architecture honest.

**A window cannot be asserted about.** `cargo test` has no display, and even with
one, "the right pixels appeared on the glass" is not something a test can read
back. This is a harder version of C's problem: C at least produced a buffer a test
could index.

The answer has three parts, and the architecture exists to serve them:

1. **The window is a sink, not a component.** Everything that decides *what* to
   show — the frame pacing, the key mapping, the pen-to-u32 conversion, the state
   machine of pause/step/reset, the save-state format — is a pure function or a
   plain struct in a testable crate. The windowing library is called from one thin
   module that does nothing but hand it a finished buffer and read keys back.
   **The rule: no logic behind the display boundary.** If a decision is made
   inside the module that talks to `minifb`, it cannot be tested, so it must not
   be made there.

2. **The pacing is fed a clock, never asked for one.** A loop calling
   `Instant::now()` internally is untestable and non-deterministic. The pacer is
   a struct with a method taking the elapsed nanoseconds as an argument, so a test
   drives it through a hundred simulated frames of a jittery, slow, or stalled
   host and asserts exactly how many emulated frames it asked for. The real loop
   passes a real duration; that call is the only clock access in the sub-project
   and it lives in the display module.

3. **A save state round-trips to a byte-identical machine, and the check is a
   divergence test.** `Machine == Machine` is necessary and nowhere near
   sufficient: it passes for a serializer that drops a field the comparison also
   ignores. The load-bearing test is: run N frames, snapshot, run M more, restore
   the snapshot, run the same M frames again, and assert **the framebuffer and
   the full trace match the first run's**. A dropped field diverges the second run.
   This is the same discipline as C's "state the value as a literal in exactly one
   place": the snapshot is checked by its *consequences*, not by comparing it to
   itself.

**What this does not give us.** It does not prove the window shows the right
picture, that the frame rate feels right, or that the controls feel responsive.
Nothing available to an automated test can. Those are the user's checks, and the
deliverable names them explicitly rather than implying the tests cover them.

---

## Architecture

Two crates, split on exactly the line the verification argument draws.

```
crates/frontend/          NEW. No windowing dependency. Testable.
  src/lib.rs              the crate's contract, and why it has no window
  src/pace.rs             FramePacer: elapsed nanoseconds -> how many frames to run
  src/keys.rs             Key -> Inputs mapping, and the control state machine
  src/pixels.rs           pen buffer -> 0x00RRGGBB, for the display's buffer format
  src/state.rs            save-state encode/decode, versioned and self-describing

crates/sfemu/             MODIFIED. Gains the window and the loop.
  src/main.rs             argument parsing, dispatch to run-headless or run-window
  src/display.rs          the ONLY file that names `minifb`. No logic.
  src/loop_.rs            the run loop: pacer + machine + display, wired together
```

`crates/frontend` depends on `machine` (for `Inputs` and the machine it snapshots)
and on nothing else. **It must never depend on `minifb`**, and the plan states
that as a global constraint, because a single `use minifb::Key` in `keys.rs` would
put the key mapping behind the display boundary and forfeit the whole argument
above.

The mapping from `minifb::Key` to this crate's own key enum therefore lives in
`display.rs` — a total, mechanical match with no decisions in it.

### Why not one crate

`sfemu` is already the crate that joins host-facing code (`romset`, `std`, files)
to the dependency-free simulation. Putting the pacer and the key map there too
would work, and would mean the only way to test them is through a binary crate's
`#[cfg(test)]` module — which is where they would quietly start reaching for the
window that is a few lines away. A separate library crate makes "no logic behind
the display boundary" a compile-time fact rather than a habit.

### Why `minifb`

Verified on this host on 2026-08-08: builds in 9 s, opens a window, reports keys,
and pulls in exactly **two runtime dependencies** (`raw-window-handle`, plus `cc`
as a build dependency). It takes a `&[u32]` of `0x00RRGGBB` and a source width and
height, and scales with `ScaleMode::AspectRatioStretch` — which is what a 384×224
frame in a resizable window needs.

The alternatives and why not:

- **`winit` + `pixels`/`wgpu`.** The general answer, and 150+ crates of graphics
  stack for a job that is "put 86,016 pixels on the screen". Its event loop also
  wants to own `main`, which fights the frame-stepped API A through C were built
  around.
- **SDL2.** Needs a system library the user has to install. A `cargo run` that
  fails on a fresh machine is worse than one that works.

`minifb` is only reachable from `display.rs`, so replacing it later is a
single-file change. That is the point of the boundary, and it is worth stating
that it survives the decision being wrong.

### The dependency edge that must not appear

`machine` still must not depend on `romset`, on `frontend`, or on anything from
crates.io. E1 does not touch `machine`'s manifest except to add the optional
`serde` feature described below, which is off by default.

---

## The frame clock

CPS-1 runs at 8,000,000 / (512 × 262) = **59.6374 Hz**, which is 16,768,000 ns per
frame (a whole number of nanoseconds, because the pixel clock divides evenly —
`crates/machine/src/timing.rs` already pins this and asserts both divisions are
exact).

Measured on this host on 2026-08-08, in release, with all three layers enabled, a
full 256-record object table, opaque synthetic tiles, and a 4 MB graphics ROM:
**0.749 ms per frame** for CPU + render + RGB conversion — a 22× margin on the
16.768 ms budget. CPU alone is 0.095 ms; the renderer is the bulk at 0.430 ms.
Read that as "a very large margin on one machine", the same caveat the README
applies to the 68000 benchmark. It is not a performance guarantee, and E1 takes no
optimisation work on the strength of it.

That margin is why the pacer is simple: **sleep-free, catch-up-bounded**.

```rust
pub struct FramePacer {
    frame_ns: u64,
    owed_ns: u64,
    max_catch_up: u32,
    dropped: u64,
}

impl FramePacer {
    pub fn cps1() -> Self;                    // 16,768,000 ns, max_catch_up 4
    pub fn new(frame_ns: u64, max_catch_up: u32) -> Self;
    /// How many emulated frames this host tick owes. Advances internal debt.
    pub fn tick(&mut self, elapsed_ns: u64) -> u32;
    /// Frames abandoned because the host fell further behind than it may catch up.
    pub fn dropped(&self) -> u64;
    pub fn reset(&mut self);
}
```

`tick` accumulates `elapsed_ns` into `owed_ns`, divides out whole frames, and caps
the answer at `max_catch_up`. **The cap is the load-bearing part**, and it has to
discard the debt it refuses to serve rather than carry it: a host that stalls for
two seconds owes 119 frames, and a pacer that carried that debt would then run
flat out for two seconds of fast-forwarded game — the classic emulator "the window
was behind a breakpoint and now everything is in fast-forward" bug. So a capped
tick zeroes the remaining debt and counts the difference in `dropped`, which the
window title reports so the drop is visible rather than silent.

`max_catch_up = 4` because a hiccup of up to 67 ms should be caught up smoothly
and anything longer is better dropped than fast-forwarded.

The loop does not sleep. `minifb`'s `set_target_fps` blocks in `update_with_buffer`
to hold a rate, and it is the display's business; using it means the loop's own
timing code is only ever asked "how much time passed", which is exactly the
question `tick` answers. If `set_target_fps` proves unreliable in practice that is
a `display.rs` change and the pacer is unaffected — the loop already handles a
host tick of any size, which is what its tests establish.

---

## Controls

The default map, and the reason for each choice:

| Key | Board input | Why |
|---|---|---|
| Arrows | P1 stick | |
| `A` `S` `D` | P1 jab, strong, fierce (`IN1` bits 4-6) | Left hand, punches on the top row of the six-button layout |
| `Z` `X` `C` | P1 short, forward, roundhouse (`IN2` bits 0-2) | Directly under the punches, matching a real cabinet |
| `5` | Coin 1 | MAME's convention |
| `1` | Start 1 | MAME's convention |
| `6` `2` | Coin 2, Start 2 | |
| `F2` | Test switch (`IN0` bit 6) | Holding it at boot enters the service menu |
| `F3` | Reset the machine | |
| `P` | Pause / resume | |
| `.` | Step one frame while paused | The debugger's ancestor; E2 makes it an instruction |
| `F5` / `F8` | Save state / load state | |
| `F12` | Screenshot to a PPM | Reuses C's writer |
| `Esc` | Quit | |

Player 2 is **not** mapped by default. Two players on one keyboard needs a second
ten-key cluster, and the honest options are all bad; E1's `Inputs` already carries
P2 and netplay or a gamepad will supply it. `--p2-keys` is not in scope, and this
is a YAGNI call rather than an oversight: nothing in E1 can exercise a second
player, and a mapping nobody uses is a mapping nobody notices is wrong.

**Polarity is not this crate's problem, and that is deliberate.** `machine`'s
`Inputs` takes `true` for pressed and does the active-low conversion in one place
(`crates/machine/src/inputs.rs`). `keys.rs` sets booleans. A frontend that
computed port values itself would duplicate the one piece of polarity logic in the
project — and the module comment there records that getting it backwards "boots
with every button held, which looks like a game bug rather than a bus bug and
costs a day to find".

### The control state machine

Pause, step, reset, save, load, screenshot, and quit are **edge-triggered**: they
act on the transition to pressed, not on the held state. A held `.` must not step
sixty frames a second, and a held `F5` must not write sixty save states. Game
inputs are the opposite — level-triggered, because holding down is how you crouch.

That asymmetry is the whole substance of `keys.rs`, so it is a struct with a
method rather than a free function:

```rust
pub struct Controls { held: KeySet }

impl Controls {
    /// Feeds this frame's held keys, and returns what the loop should do.
    pub fn update(&mut self, now_held: &KeySet) -> Actions;
}

pub struct Actions {
    pub inputs: Inputs,           // level-triggered, for the board
    pub pause_toggled: bool,      // edge-triggered, below
    pub step: bool,
    pub reset: bool,
    pub save: bool,
    pub load: bool,
    pub screenshot: bool,
    pub quit: bool,
}
```

`KeySet` is this crate's own key enum in a fixed-size set — not `Vec<minifb::Key>`,
which would be the dependency the architecture forbids.

---

## Save states

### What has to be in one

| State | Where | Note |
|---|---|---|
| `M68k` | `cpu` | 88 bytes; derives `Clone`, `PartialEq`, and has an optional `serde` feature already |
| RAM | `board.ram` | 0x8000 words |
| gfxram | `board.gfxram` | 0x18000 words |
| CPS-A, CPS-B | `board.cps_a`, `board.cps_b` | 0x20 words each |
| `sound_latch`, `coin_ctrl` | `board` | |
| `vblank_pending` | `board` | Private, and **must** be in the state: restoring with it wrong means one missed or one doubled interrupt at the seam |
| `line`, `carry`, `total_cycles` | `Cps1` | `carry` is private and is exactly the scheduler's sub-frame position |
| The object latch | `video.obj` | Private. Sprites are delayed one frame, so a state restored without it draws one frame of wrong sprites |
| `Inputs` | `board.inputs` | Cheap, and a state that restores mid-move without it drops the held direction |

**Not** in one: the decoder table (rebuilt, 512 KB), the ROM and the graphics ROM
(the user supplied them; a save state that embedded them would be a ROM file this
project must not produce), the palette and the framebuffer (recomputed by the next
`render`), and the `Trace` (a record of the run, not state of the machine — and a
restored trace would make the divergence test compare a copy against itself).

Three of those fields are private, and two of them (`carry`, `obj`) are private
*for good reasons* the existing code documents. So E1 adds snapshot methods to
`machine` and `video` rather than making fields public: `Cps1::snapshot()` /
`restore()`, delegating to `Board` and `Video`. Widening the fields would let a
future caller write `carry` without the scheduler's invariant, which is the failure
mode `cps1.rs` spends a paragraph explaining.

### The format

A versioned, length-prefixed binary blob, hand-rolled, **no `serde`**.

```
magic     8 bytes   b"SFEMU\0\0\1"   -- the trailing byte is the format version
kind      4 bytes   the board's identity, so an SF2 state cannot load into SF1
len       8 bytes   payload length, little-endian
payload   len bytes fields in a fixed documented order, little-endian
crc32     4 bytes   of the payload, using romset's existing CRC-32
```

`serde` + `bincode` would be two more dependencies and a format whose layout is
implied by struct definitions — so a field reordered during a later refactor
silently changes the format while every test still passes, because both sides moved
together. The characteristic defect of this branch, in a save-state costume. A
hand-written encoder has an explicit order that a test pins with a literal byte
count, and the version byte gives a real answer ("this state is version 1, I read
version 2") instead of a deserialization error.

CRC-32 because truncated or corrupted state files are the common failure. The
magic prevents loading an arbitrary file; `kind` prevents loading another board's
state, which would otherwise restore plausibly and behave insanely.

⚠️ **The CRC-32 is written fresh in `frontend`, not taken from `romset::crc32`.**
Reaching for the existing one would make `frontend` depend on `romset`, which
depends on `miniz_oxide` — exactly the edge `machine`'s manifest refuses, for the
same reason. It is twenty lines (`crates/romset/src/crc32.rs` says so, and says
why it was hand-written there too), and duplicating twenty lines is cheaper than
a DEFLATE decoder in the frontend. Its test pins the CRC-32 specification's own
check vector — `"123456789"` → `0xCBF43926`, the literal `romset`'s test uses — so
the two implementations are each verified against the standard rather than against
one another.

`load` returns `Result`. **A frontend must never panic on a bad file**, which is
the same posture `machine` takes toward guest input.

### Where states go

`--state <path>` names the file; without it, next to the ROM set as
`<rom-set-stem>.sfs`. One slot, because ten slots is ten times the UI for a feature
whose value is "I want to retry that jump", and the file path is already the
general mechanism.

---

## The loop

```rust
pub fn run(m: &mut Cps1, d: &mut impl Display, opts: &LoopOpts) -> Result<Summary>
```

`Display` is a trait in `sfemu` — `present(&[u32])`, `held_keys() -> KeySet`,
`elapsed_ns() -> u64`, `is_open() -> bool`, `set_title(&str)` — implemented by
`display.rs` over `minifb` and by a **recording fake** in the loop's own tests.
That fake is what makes the loop testable at all: it returns a scripted sequence of
key sets and elapsed times, records every buffer it is handed, and reports the
window closed after N ticks.

With that, the loop's tests are ordinary assertions: a scripted `P` pauses and the
next tick runs zero frames; `.` while paused runs exactly one; `F3` resets and the
cycle count returns to zero; a 500 ms host tick runs `max_catch_up` frames and not
30; the buffer handed to `present` is 86,016 pixels every tick, including while
paused; `Esc` ends the loop and the summary reports the frames run.

Per iteration: read keys → `Controls::update` → apply `inputs` to the board →
`FramePacer::tick` → run that many frames (or one, on a step) → `render` →
`pens_to_argb` → `present` → update the title if the drop count changed.

### The headless path stays

`sfemu <set> [frames] --ppm out.ppm` keeps working exactly as it does today, and
`--headless` is not needed: absence of a window request *is* the headless mode.
The window is opened by a new flag rather than by default, because the existing
behaviour is what every check in the project is written against — a binary that
opened a window when run under CI would hang.

```
sfemu <rom-set> [frames] [--ppm <path>]        unchanged: run and report
sfemu <rom-set> --play [--state <path>]        the window
```

`--play` ignores a frame count rather than erroring on one: `sfemu set.zip 60
--play` most plausibly means "play", and there is no reading under which it means
"open a window and close it after one second".

---

## Error handling

- **Bad ROM path** — already handled by `romset`, message names the path.
- **No display available** (headless host, SSH) — `Window::new` fails; report it as
  an error naming `--ppm` as the alternative that needs no display, and exit 1.
- **A corrupt or foreign save state** — refuse, name which check failed (magic,
  version, board kind, length, CRC), keep running. The state file is the user's,
  and a frontend that dies because one is truncated has lost their session too.
- **`--state` unwritable** — report on the frame it happens, keep running, and do
  not retry every frame. A save that silently fails is the worst outcome here.
- **Guest misbehaviour** (a halted CPU) — the title bar says `HALTED`; the loop
  keeps running, because the debugger in E2 is exactly what you want at that
  moment and quitting would deny it.

---

## Testing

| What | How | Where |
|---|---|---|
| `FramePacer` | Simulated elapsed sequences: exact, jittery, slow, stalled, zero | `frontend/src/pace.rs` |
| The catch-up cap | A 2 s stall runs `max_catch_up` frames, not 119, and `dropped` counts the rest | `frontend/src/pace.rs` |
| Key mapping | Each key sets exactly its own field; the port values come from `Inputs` | `frontend/src/keys.rs` |
| Edge triggering | Held `.` steps once, not every frame; released and pressed again steps again | `frontend/src/keys.rs` |
| Pen → ARGB | Literal palette entries against literal `0x00RRGGBB`, cross-checked against `entry_to_rgb` | `frontend/src/pixels.rs` |
| Save-state round trip | **Divergence:** snapshot, run 30 frames, restore, run 30 again, framebuffer and trace identical | `frontend/src/state.rs` |
| Save-state rejection | Bad magic, wrong version, wrong board kind, truncated, bad CRC — one test each, each naming its check | `frontend/src/state.rs` |
| Format stability | The encoded length of a known machine, as a literal | `frontend/src/state.rs` |
| The loop | The recording fake `Display`: pause, step, reset, quit, catch-up, present size | `sfemu/src/loop_.rs` |
| Argument parsing | `--play`, `--state`, and their interaction with the existing flags | `sfemu/src/main.rs` |

Two things are deliberately **not** tested, and the plan says so out loud rather
than leaving a reader to assume coverage: `display.rs` (it is the boundary; its
whole content is calls into `minifb` and a total `Key` match), and anything about
how the result looks or feels.

### The mutation pass

Every task ends with one, as in C. Two mutants are named now because they are the
ones this design most fears surviving:

- **`max_catch_up` ignored** — the cap not applied. Survives if no test drives a
  stall longer than the cap.
- **Edge triggering weakened to level** — `step` set from held rather than from the
  transition. Survives if every step test presses the key for exactly one tick.

Each pass includes a deliberate control mutant that must survive; a pass where
everything dies is more likely broken than thorough.

---

## Deliverables

1. `crates/frontend`: `FramePacer`, `Controls`/`Actions`/`KeySet`, `pens_to_argb`,
   and the save-state codec. Depends on `machine` and nothing else.
2. `Cps1::snapshot`/`restore`, with `Board` and `Video` delegates, so no private
   field needs to become public.
3. `crates/sfemu`: `--play`, `--state`, the run loop, and `display.rs` over
   `minifb`.
4. `docs/hardware/` gains nothing — E1 discovers no hardware facts. Instead
   `README.md` gains the frontend's controls, the `--play` invocation, and the
   measured frame cost with its one-machine caveat.
5. The user-only checks, stated as such: **does the window show Street Fighter II,
   does it run at the right speed, and do the controls respond?** No automated
   test in this repository can answer any of the three.

## Task decomposition

Eight tasks, each ending with a green `cargo test --workspace` in both profiles and
a commit:

1. `crates/frontend` scaffold and `FramePacer`, including the catch-up cap.
2. `KeySet`, `Controls`, `Actions`: the map and the edge/level asymmetry.
3. `pens_to_argb`.
4. `Cps1::snapshot`/`restore` in `machine` and `video` — state only, no encoding.
5. The save-state format: encode, decode, the five rejections, and the divergence
   test.
6. The `Display` trait, the recording fake, and the run loop.
7. `display.rs` over `minifb`, `--play`, `--state`, and the argument parsing.
8. `README.md`, and a mutation pass over `crates/frontend` and the new `sfemu`
   modules.

Task 7 is the only one whose deliverable a test cannot see, and it is deliberately
last and deliberately thin: by the time it runs, everything it wires together is
already tested.

## Risks

**The one that will actually bite.** The save state will be missing a field, and
the symptom will not be a crash — it will be a restored state that plays almost
right and desyncs a few seconds later. `carry`, `vblank_pending`, and the object
latch are the three most likely, all private, all easy to overlook. The divergence
test is aimed squarely at this, and it is why the test runs 30 frames after
restoring rather than comparing structs: 30 frames is long enough for a one-frame
sprite error or a missed interrupt to become a different framebuffer.

**The one that will look like a different bug.** A wrong pen-to-ARGB conversion —
channels swapped — looks like a palette bug, sending the reader into `video` where
everything is correct. `pixels.rs` is therefore tested against literal ARGB values
*and* cross-checked against `video::palette::entry_to_rgb`, so a channel swap fails
in the frontend where it lives.

**The one we cannot close.** Nothing here proves the window shows the right
picture. E1's deliverable is a `--play` flag the user runs against their own ROM
set; if it comes up wrong, the diagnostic path is E1's screenshot plus the trace B
built, and E2's debugger after that.
