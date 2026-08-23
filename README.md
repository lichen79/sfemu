# sfemu

A Street Fighter arcade emulator, built from the hardware up. Point it at a ROM
set you own and `--play` opens a window:

```bash
cargo run -p sfemu --release -- /path/to/your/sf2.zip --play
cargo run -p sfemu --release -- /path/to/your/sf.zip --play --game sf1
```

Ten sub-projects are complete: the **68000 core** (A), the **bus and timing
framework with a MAME ROM-set loader** (B), the **CPS-1 scanline renderer** (C),
the **Z80 audio CPU** (D1), the **YM2151 FM chip and the sound board's wiring**
(D2), the **OKI MSM6295 ADPCM chip, the mono mix and host audio** (D3), the
**frontend** — window, frame clock, keyboard, and save states (E1) — the
**debugger** (E2): `F1` for an in-window overlay, `F4` to step one instruction,
`F7` for a breakpoint — the **graphics viewers** (E3): `F9` for a tile, tilemap,
palette and layer browser, `F10` to cycle it — and the **Street Fighter 1
driver** (F): a second board on the same core, selected with `--game sf1`.

**There is sound.** The chain runs end to end: the Z80 executes SF2's driver from
`audiocpu`, reads the 68000's command latch, and programs a YM2151 and an MSM6295
that are each exact against their reference — ymfm and MAME's own `okiadpcm` — over
1,000 vector cases apiece. Those two streams are collapsed into the single mono
output the cabinet's one speaker gets, at MAME's own weights, and handed to the
host device through a bounded ring. The one link in that chain that is *not* exact
is the last: no host sample rate is a rational multiple of the board's
55,930.390625 Hz, so the final conversion interpolates, and what it costs is
[written down](#the-oki-msm6295-and-the-mix) rather than glossed. Street Fighter 1
runs on its own board with `--game sf1`: no CPS-A or CPS-B, tile maps in ROM
instead of RAM, one YM2151 and two MSM5205s mixed in genuine stereo, and a
second sound Z80 driving them.

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

## The OKI MSM6295, and the mix

The ADPCM sample player, validated the same way and against the same kind of
reference: `testrunner` links [MAME][mame]'s own `okiadpcm.cpp` (BSD-3, © Andrew
Gardner and Aaron Giles, pinned to tag `mame0261` so `master` moving cannot silently
change the vectors), runs a deterministic command script against a **synthetic**
sample ROM this repository generates, and requires this core to reproduce every
sample and every status read exactly: **1,000 of 1,000 cases.** No Capcom data is
involved — the ROM the vectors play is a ladder of nibbles built for the purpose.

**The step table is the one thing not checked against the reference, deliberately.**
It is 49 entries of `floor(16 * 1.1^step)`, held as a literal because Rust has no
`const fn` float `pow` and the exact integer form needs 171 bits. Checking it
against MAME would only prove the transcription; it is checked instead against an
independently derived closed form, because the obvious shortcut — the recurrence
`v += v / 10` — is *not* the same function: it disagrees at **47 of the 49 entries**,
from step 2 onward, where it gives 18 for a correct 19. A test that compared against
the shortcut would have been the bug.

**The suite's premises are read out of the cases' own bytes.** The tempting way to
assert "a quarter of the cases carry a phrase the chip refuses" is `i % 4 == 3`,
which counts indices rather than refusals and keeps passing after a generator stops
producing them. So each premise is recomputed from what was recorded: the phrase
table entry, the command script, the nibble stream. Ten are checked — every case
audible; the chip's own ±65,536 clamp reached (measured 998 of 1,000, floor 90%) and
never exceeded; all four voices sounding at once (933 of 1,000); voices that both
start and stop mid-case; the status byte a *subset* of the voices that sounded; the
step index driven to both of its clamps, recomputed from the nibbles by MAME's own
rule rather than read back from this core; the ladder phrase intact; the top of the
address bus actually read, checked against the ROM the case carries rather than a
constant; the silent volume indices (9–15, whose table entries are exactly zero)
used, 3,026 times, so a core reading the table one entry short would not pass; and
both pin-7 states present.

**Two rates, and neither is a round number.** The MSM6295 divides its 1 MHz crystal
by 132 with pin 7 high and 165 with pin 7 low — 7,576 Hz and 6,061 Hz — while the FM
chip runs at 3,579,545/64 = 55,930.390625 Hz. Both OKI rates are *under one sample
per scanline* (16/33 and 64/165), so the mix is driven off the YM tick rather than
per line, with the two pin-7 ratios sharing a denominator so that flipping pin 7 is
a numerator swap and the phase does not jump. A fresh board starts pin 7 **high**,
because that is the state MAME constructs with and `device_reset()` leaves alone —
starting low would be a 25% pitch error until the driver's first `0xF006` write.

**The mix is MAME's, at integer weights.** CPS-1 has one speaker
(`cps1.cpp:3935`), with the two YM channels at 0.35 and the OKI at 0.30 — 7, 7 and
6 over 20, and the OKI term is 3 rather than 6 because the value it is given is
already twice the stream value, which is the widest form in which a voice's
`signal × volume` product stays an exact integer. No saturation, and that is
measured rather than omitted: the chip clamps its own sum before the mix sees it,
which bounds the numerator at ±655,360 = 20 × 32,768. The truncating divide deviates
from MAME's `f32` chain by at most 0.952 LSB — under one, so no rounding term is
worth the drift it would add.

**What is not exact, stated plainly.** The host conversion. No device rate is a
rational multiple of 55,930.390625 Hz, so `machine::resample` interpolates linearly:
at 1.165× downsampling to 48 kHz that attenuates the top of the band and folds
content above 24 kHz back down. A polyphase FIR would be better and is either a
dependency or 200 lines of DSP with its own verification burden. Handing the stream
over *unconverted* would play it **14.2% slow** — a device taking 48,000 samples a
second out of a 55,930 Hz stream needs 1.165 seconds of music to fill one second, so
SF2's would come out 2.65 semitones flat, A440 at 378 Hz. The choice is therefore not
between exact and approximate but between two approximations, and this is the one
whose error is bounded and written down.

The ring between the two clocks is also measured rather than designed: 100 ms of
capacity prefilled to 50 ms (the observed depth swing was 29.3–58.7 ms), drop the
oldest on overflow so latency stays bounded, hold the last sample on underrun
because a step to silence clicks, count both so "the audio is crackly" is
diagnosable, and **no clock slewing** — drift measured at +6.3 ppm ± 59.6 ppm, below
the method's own resolution, so correcting it would be correcting noise.

[mame]: https://github.com/mamedev/mame

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

**Six tests are the documented exception, and they are `#[ignore]`d for the reason
the rule itself gives.** "Fail loudly naming the command that fetches it" holds
because the vector data *is* fetchable and there is a command to name. A ROM set is
not, and there is no command we may put in a failure message. So `boot.rs`,
`sound_boot.rs` and `audio_boot.rs` for SF2, and `sf1_boot.rs`,
`sf1_sound_boot.rs` and `sf1_audio_boot.rs` for SF1 — the six tests that need real
Capcom code — skip by default and read a path you supply:

```bash
SFEMU_ROMS=/path/to/your/sf2.zip cargo test -p sfemu --test audio_boot -- --ignored
```

One variable, one panic message per test, no second escape hatch. `SFEMU_ROMS` is
**not** how the binary is pointed at a ROM set — that is a positional argument; the
variable exists only for these six. It holds **one** path and the two trios want
different sets, so a user with both runs the suite twice with it pointing at each in
turn — a second variable would be the second escape hatch this rule forbids by name,
and pointing it at the wrong set fails loudly with a `romset` error naming the file
it could not find. What they add over the unconditional suites is
narrow and specific: that SF2's own driver talks to the chips *where this code expects
it to*. `audio_boot.rs` exists because of one trap in particular — with no sample ROM,
every phrase-table entry reads `start == stop == 0`, the chip refuses every command,
and the OKI write counter climbs anyway. A rising counter over a silent chip is
exactly what a green test must not look like, so that test asserts on the samples that
left the mix rather than on the count.

## Getting started

```bash
# Fetch the 68000 vectors (~138 MB (132 MiB) over 127 files, into gitignored
# testdata/). Shells out to curl; no HTTP dependency is taken for a
# once-per-checkout job.
cargo run -p testrunner --bin fetch --release

# The Z80 vectors. Upstream is 1.37 GB of JSON, so nothing is kept: each file is
# converted to a binary form and its JSON deleted before the next starts.
cargo run -p testrunner --bin fetchz80 --release

# The two generated suites. Each fetches its BSD-3 reference implementation
# (ymfm, and MAME's okiadpcm.cpp pinned to tag mame0261), compiles it, runs it,
# and re-parses its own output rather than trusting it. No game code anywhere.
cargo run -p testrunner --bin genym --release
cargo run -p testrunner --bin genoki --release

# Unit tests, plus one test per suite group. Both profiles: `--release` is
# where the timing law is measured, and debug is where `debug_assert!` is
# evaluated. Neither run subsumes the other.
cargo test --workspace --release
cargo test --workspace

# The four suite reports: a per-group or per-case table, then the headline
# figures. Each exits nonzero if anything is red. Note reportym takes NO
# argument — `reportym -- --test suite` printed usage and exited 0, which is a
# gate that silently ran nothing, so reportoki accepts both spellings and
# rejects anything else with 2.
cargo run -p testrunner --bin report     --release -- --test suite   # 68000
cargo run -p testrunner --bin reportz80  --release -- --test suite   # Z80
cargo run -p testrunner --bin reportym   --release                   # YM2151
cargo run -p testrunner --bin reportoki  --release -- --test suite   # MSM6295

# The oki crate's two other build shapes, since it claims to be no_std-friendly
# and a claim that is never compiled is not a claim.
cargo build -p oki --no-default-features --target thumbv7em-none-eabihf
cargo build -p oki --features serde

# Throughput. Read the caveat below before quoting a number from it.
cargo bench -p m68k

# Mutation testing: 262 mutants, each an exact string replacement, each with a
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

# Street Fighter 1, on its own board. The board is chosen, not guessed from the
# path: a set of files does not say what hardware it came from.
cargo run -p sfemu --release -- /path/to/your/sf.zip --play --game sf1

# Somewhere other than beside the ROM set for the save state:
cargo run -p sfemu --release -- /path/to/your/sf2.zip --play --state /tmp/mine.sfs

# Without --play: run a fixed number of frames and print a report. No window,
# which is what CI and a bisect want.
cargo run -p sfemu --release -- /path/to/your/sf2.zip 600 --ppm frame.ppm
```

`--release` is not optional advice here: a debug build does not hold 59.6 Hz.

Audio opens on the default output device and needs no flag. If none can be opened,
that is a notice on `stderr` and a `[no audio]` tag in the title bar, not a refusal
to run — no sound is a degradation, and a machine you can watch is better than one
that would not start. The device's own rate is printed beside the board's at startup,
because the interesting number is the pair: neither is the other's multiple, which is
why the samples are converted rather than played as they are.

```
        Player 1                          Player 2

          Z   (W on QWERTY)                 ↑                7 8 9  kicks
        Q S D    I O P  kicks             ← ↓ →              4 5 6  punches
        ^        K L M  punches
        (A)      (last is `;`)            stick               keypad
        stick     on QWERTY
```

The key names below are **AZERTY** labels. Keys are bound to physical positions, not
to letters, so on a US QWERTY keyboard P1's stick reads `W` `S` `A` `D` — the same
four keys in the same diamond, with different letters printed on them — and the third
punch is the semicolon. See [Layouts](#layouts) below.

| Key | Does |
|---|---|
| `Z` `S` `Q` `D` | P1 stick — up, down, left, right |
| `K` `L` `M` | P1 jab, strong, fierce |
| `I` `O` `P` | P1 short, forward, roundhouse |
| Arrows | P2 stick |
| Keypad `4` `5` `6` | P2 jab, strong, fierce |
| Keypad `7` `8` `9` | P2 short, forward, roundhouse |
| `5` / `1` | Coin 1 / Start 1 |
| `6` / `2` | Coin 2 / Start 2 |
| `F2` | Test switch — hold it at boot for the service menu |
| `F3` | Reset the machine |
| `F11` | Pause / resume |
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

**Punches sit on the bottom row and kicks directly above them** — the reverse of a
real six-button cabinet, deliberately. On an AZERTY keyboard `K` `L` `M` is a run of
three on the home row, so putting the punches there puts them under the resting
fingers and pushes the kicks up a row. Both clusters are arranged the same way, which
is the property that matters once the arrangement is unconventional: learning one half
teaches you the other. Each player has one half of the keyboard.

Two things follow from that. **Player 2's buttons need a numeric keypad** — on a
keyboard without one the arrow keys still move but the six attacks are unreachable,
so P2 is a desktop arrangement. And **no letter key is a control any more**: pause
moved off `P`, which is P1's roundhouse kick now, onto `F11`, the one gap that was
left in `F1`–`F12`. Every control is a function or navigation key, which is what keeps
a punch from also saving a state.

The board's `Inputs` is the same either way, so a gamepad or netplay can drive
either player later without touching this map.

### Layouts

`minifb::Key` names a **hardware position**, not a letter. On macOS it passes the raw
`[event keyCode]` through a fixed table that names each position after the letter a US
QWERTY keyboard prints there; the active layout is never consulted. So `minifb`'s `Q`
means "position 0x0c", which types `q` on QWERTY and `a` on a French keyboard.

P1's keys are mapped by position and set up for AZERTY. Three of them are places the
layouts disagree about — up, left, and the third punch:

| Board input | `minifb` name | AZERTY label | QWERTY label |
|---|---|---|---|
| P1 up | `W` | **Z** | W |
| P1 left | `A` | **Q** | A |
| P1 fierce | `Semicolon` | **M** | ; |
| P1 down | `S` | S | S |
| P1 right | `D` | D | D |

The third is the one worth stating twice, because AZERTY does not merely shift `M` — it
moves it off the bottom row entirely, to the home row's right end next to `L`. That
position is the one QWERTY prints `;` on, so `minifb` calls it `Semicolon`, and
`minifb`'s own `M` is the key labelled `,` here. Mapping the punch by letter would put
it one key past the end of the home row. Verified against the live layout with Carbon's
`UCKeyTranslate` rather than reasoned about: on `French – PC`, position 0x29 types `m`
and 0x2e types `,`.

Everything else is layout-stable: `I` `O` `P` `K` `L`, the number row, the keypad, the
arrows and the function keys sit in the same place on both. So a QWERTY player uses
WSAD for the stick, `K` `L` `;` for the punches, and reads every other row as printed.

This is why `crates/sfemu/src/display.rs` maps `M::W => Key::Z` and
`M::Semicolon => Key::M`, which look like typos and are not — `frontend::Key`'s
variants carry the AZERTY label because that is what this README and the usage text
tell the player to press.
`display::tests::player_ones_keys_are_mapped_by_position` asserts both halves,
including that `M::Z`, `M::Q` and `M::M` press *nothing*, because "supporting both
layouts" by mapping the QWERTY positions as well would give one board input two keys
and silently undo the fix.

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

### Eight things only you can check

Everything in this repository is tested without a display and without an audio
device, which leaves exactly eight claims no test here can make. Run it against
your own ROM set, and look — and, for the last one, listen:

1. **Does the window show Street Fighter II?** A test can assert the framebuffer
   changed, that a save state round-trips, and that a pen becomes the right ARGB
   word. None of that establishes that the picture is right.
2. **Does it run at the right speed?** The pacer is tested against a scripted
   clock, which proves the arithmetic and not the wall clock.
3. **Do the controls respond?** The key map is tested against the board's
   documented port bits, which proves `I` sets P1's jab and not that `I` reaches the
   game. Two-player play is the same claim twice over, and adds one only a keyboard
   can answer: whether your keyboard reports P1's cluster and P2's keypad **at the
   same time**. Cheap membrane keyboards drop simultaneous keys in ways no test here
   can see.
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
   register count climbing in the thousands reads, to you, as SF2 playing music.
8. **Does it actually sound right?** This is the one claim in the project that no
   amount of testing can approach. The ADPCM decoder is exact against MAME's over
   1,000 vector cases, the YM2151 is exact against ymfm's over 1,000 more, the mix
   and the ring are tested against scripted producers and consumers, and the loop's
   queueing is asserted against a recording fake. All of that establishes that the
   right numbers were computed and handed over. Whether the result is *Street
   Fighter II* — right pitch, right tempo, voices where they belong, no clicks — is
   a judgement only your ears make:

   ```bash
   cargo run -p sfemu --release -- /path/to/your/sf2.zip --play
   ```

   If you hear nothing, the title bar says `[no audio]` when no device could be
   opened, and `F1`'s sound panel gives you `CLP` (the mix clamped), `DRP` (the ring
   overflowed, so the emulator outran the device) and `UND` (the ring starved, so
   the device outran the emulator). Silence with all three at zero and the register
   counts climbing is a mix problem; `UND` climbing is a pacing one.

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
crates/oki/          the MSM6295 ADPCM chip, on the same terms: no dependencies,
                     no unsafe, no clock access, no_std-friendly. Four voices,
                     the phrase table, and a 49-step table built from a closed
                     form and checksummed.
crates/machine/      the board: memory map, bus, interrupts, scheduler, inputs,
                     the sound board with its rational Z80 clock, the mono mix
                     and the host resampler, snapshot and restore. Depends on
                     m68k, video, z80, ym2151 and oki — all five
                     dependency-free, so the display boundary below still holds.
                     Four of the five build for a bare-metal target (verified on
                     thumbv7em-none-eabihf); `video` allocates a framebuffer and
                     needs `alloc`, which is a different claim and is not made
                     for it above.
crates/romset/       the MAME ROM-set loader: zip or a directory, CRC-checked,
                     interleaved into the regions the board wants.
crates/frontend/     every frontend decision, with no window: frame pacing, the
                     key map, pen-to-ARGB, the save-state file format, the
                     debugger's state, the graphics viewer's state and its four
                     views, and the 4x6 font drawn in this repository.
crates/sfemu/        the binary. The only crate that names a windowing library or
                     an audio library, each in one file and nowhere else (a test
                     per boundary enforces it).
crates/testrunner/   dev-only harness for the external vector suite.
scripts/mutate.py    the mutation harness: 262 mutants over the above in 19 sets,
                     each an exact string replacement with a declared
                     expectation. 240 killed, 22 declared survivors (19 controls
                     and 3 proven equivalents), 262/262 as expected.
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

**Audio has the same line, drawn for the same reason.** A device has a clock we do
not control and a buffer we cannot read back, so `sfemu/src/audio.rs` is a handle
and five forwards; the rate conversion and the full-ring policy — the parts with
edge cases — live in `machine::resample`, where a test drives them. What the loop
*decides* about audio (queue once per frame, drain the machine's buffer, report a
held pause every tick, treat a dead device as a notice rather than a stop) is
asserted against a recording fake, not by listening.

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
| **D3** | OKI MSM6295 ADPCM, mixing, host audio | **complete** — the samples reach a speaker: 1,000/1,000 ADPCM vector cases against MAME's decoder, mixed and queued to the device |
| **E1** | Frontend: window, frame clock, keyboard, save states | **complete** — `--play` |
| **E2** | Debugger: single-step, breakpoints, disassembly, register and memory views | **complete** — `F1`, in-window, and it does not perturb the machine |
| **E3** | Graphics viewers: tile browser, tilemap and palette views, layer toggles | **complete** — `F9`, four views, and the mask subtracts only |
| **F** | Street Fighter 1 driver: pre-CPS board, ROM-resident tile maps, two MSM5205s in stereo, second sound Z80 | **complete** — `--game sf1`, a second board on the same core, and the abstraction held: `m68k`, `z80` and `ym2151` took no changes at all, and `video` and `machine` each gained an `sf1` module beside the CPS-1 one rather than growing a board flag through the existing code |

E was split because its three surfaces are independent and only the first changes
what the project *is* rather than what can be inspected about it. E3 came last for
a reason rather than by preference: a tile browser's value is mostly in stopping
the machine at the frame you care about, which is E2's stepping — so E3 waited for
it, and then took it.

**D is split for the same reason, and the number that settled it is 1,604.** That
is how many vector files the Z80 suite has, against the 68000's 127 — and the
68000 took 16,462 lines and a spec of its own. A Z80 core, an FM synthesizer, and
a host audio path are three unrelated subsystems; asking one review pass to gate
all three is what the original decomposition was written to avoid. D1 and D2 were
each silent by design — a CPU with no chip attached, then a chip whose samples
reached no speaker — and **D3 is the one that ended "there is no sound."**

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
