# sfemu user guide

How to build it, how to give it a game, how to play it, and what every key does.

This is the practical companion to [`architecture.md`](architecture.md), which
explains why the program is shaped the way it is. Where this page and the README
overlap, the README is the summary and this page is the detail.

⚠️ **sfemu ships no game data.** It emulates the hardware; you supply the ROM set.
See [Supplying your own ROM set](#supplying-your-own-rom-set) — there is no bundled
fallback, no download, and no environment variable that fetches one. If you have no
ROM set, `--demo` still runs, and it is a real run of the real board.

---

## Building it

You need a Rust toolchain at **1.93 or newer** (`rust-version` in the workspace
manifest) and, for sound, a working audio output device.

```sh
git clone <this repo> && cd sfemu
cargo build -p sfemu --release
```

**`--release` is not optional advice.** A debug build does not hold 59.6 Hz. The
release profile in this workspace is `opt-level = 3`, `lto = true`,
`codegen-units = 1`, so the first build takes a while and later ones are quick.

Nothing here is fetched at build time, and building the binary needs no test data.
The vector suites in `testdata/` are for `cargo test`, not for playing; if you only
want to run the emulator you never need them. See
[Running the tests](#running-the-tests) if you do.

Two crates.io dependencies reach a system library: `minifb` for the window and
`cpal` for audio. On Linux you will need the usual X11/Wayland and ALSA development
packages for them; on macOS and Windows they build against the system frameworks
with no extra setup.

---

## Supplying your own ROM set

A ROM set is a **MAME-format zip or a directory of loose files** that you supply by
path. The loader reads the path you give it, checks each file's CRC-32, and
interleaves the bytes into the regions the board wants.

Legal ways to obtain a set you may use:

- **Capcom Arcade Stadium** (Steam) — includes Street Fighter II and ships the
  original ROM data.
- **Capcom Fighting Collection** — likewise.
- **Dumping a board you own.** The most defensible route, and the only one that gets
  you a set for hardware Capcom has not re-released.

Both Street Fighter II and Champion Edition are still commercial Capcom code, and
their age does not change that. This program will not fetch a set for you.

### The board is chosen, not guessed

```sh
cargo run -p sfemu --release -- /path/to/your/sf2.zip --play
cargo run -p sfemu --release -- /path/to/your/sf2ce.zip --play --game sf2ce
```

`--game` names the hardware **and** the file list. A set of files does not say what
machine it came out of, so nothing is inferred from the path or the archive's name.

| `--game` | What it is | MAME set |
|---|---|---|
| `sf2` (default) | Street Fighter II: The World Warrior, World 910214 | `sf2` |
| `sf2eb` | The same game, the 910214 revision with six different program files | `sf2eb` |
| `sf2ce` | Street Fighter II: Champion Edition, World 920313 | `sf2ce` |
| `sf1` | Street Fighter, on its own 1987 board | `sf` |

**The three CPS-1 sets are not interchangeable.** Each carries a different CPS-B
custom part, and `--game` selects both the file list and the register row — and, since
2026-08-25, the CPU clock: **Champion Edition's 68000 runs at 12 MHz where both World
Warrior sets run at 10** (MAME's `cps1_12MHz`, `cps1.cpp:3963`, "verified on pcb"). The
refresh rate is the same 59.637 Hz on both — the extra cycles fit inside the same frame —
so a `--game sf2ce` run before that date played the whole game at 83.3% speed, which is
not something the picture or the sound tells you. There is no default clock for the same
reason there is no default register row. Naming
the wrong one usually fails at load time with the name of the file it could not
find — which is the good outcome, because it is a load error rather than a bad
picture. The two measured bad outcomes, for reference:

- `sf2eb` under `sf2`'s row **boots to an idle loop** with no unmapped access at
  all. Every counter looks healthy.
- `sf2ce` under the wrong row **draws**, with the background layer missing: 184
  distinct pens against 123 at 1,100 frames.

That is why there is no default CPS-B row and no fallback. A missing row is an
error naming the function to add it to.

⚠️ **`sf1` is written but has never been run.** No SF1 set has been available to
this project. The driver, the video path and three gated tests are in the tree,
unexercised. If you have a set, `--game sf1` is the flag; treat anything it does as
unverified.

### No ROM set? Run the demo

```sh
cargo run -p sfemu --release -- --demo --play
```

`--demo` needs no files and no path. It runs a CPS-1 image this workspace
generates from nothing (`crates/testrom`): scrolling tilemaps, a sprite on a path,
a frame counter, and FM music. It is homebrew, not any Capcom game — and it boots
down the *same* path a real set does, through the same region names, the same four
lookups, the same board config and the same timing. Every key below works in it.

---

## Running it

Three shapes, from the usage text:

```
sfemu <path-to-rom-set> [frames] [--game <name>] [--ppm <path>]
sfemu <path-to-rom-set> --play [--game <name>] [--state <path>]
sfemu --demo [frames] [--play] [--ppm <path>] [--state <path>]
```

| Flag | What it does |
|---|---|
| `--play` | Open a window and run until you close it or press `Esc`. Without it, no window. |
| `--game <name>` | The board and file list. `sf2` if omitted. |
| `--demo` | Run the generated CPS-1 image. Takes no ROM path. |
| `--state <path>` | Put the save state somewhere other than beside the ROM set. Needs `--play`. |
| `--ppm <path>` | Write the last frame as a binary PPM. Headless runs only. |
| `[frames]` | A positional frame count for a headless run. 60 if omitted; **ignored** with `--play`. |

Argument parsing is strict on purpose. A frame count is parsed rather than
defaulted, so `6O` with a letter O is an error and not a 60-frame run.
`--state`, `--game` and `--ppm` each reject being given twice and reject a missing
value. `--state` without `--play` is an error rather than a no-op, because nothing
but the window reads or writes a state and you would otherwise be left wondering
where your file went. A path handed to `--demo` gets a message saying so, not just
"not a frame count".

Exit codes: **0** success, **2** a usage error (no arguments — the usage text goes
to stderr), **1** a run that failed. A script driving this can tell the last two
apart.

### The headless report

Without `--play`, sfemu runs a fixed number of frames and prints what the board
saw:

```sh
cargo run -p sfemu --release -- /path/to/your/sf2.zip 600 --ppm frame.ppm
```

This is a real run — of `--demo`, so you can reproduce it without a ROM set
(`cargo run -p sfemu --release -- --demo 600`, measured 2026-08-24):

```
board         CPS-1
frames        600
vblanks       600  acks 599
cycles        100608002
cpu           pc 0x00007e  running
framebuffer   86016 of 86016 pixels drawn, 4 palette page(s)
cps-a writes  1210
cps-b writes  3
gfxram writes 34848
sound latch   9
rom writes    0
unmapped      0 reads, 0 writes
```

`acks 599` against `vblanks 600` is the last frame's interrupt still pending when
the run ended, not a missed one. A real game's counters are larger throughout: it
writes CPS-B registers, drives the sound latch every few frames, and fills more of
gfxram.

That exists because **a black window is indistinguishable from a boot that hangs on
the first instruction**, whereas a count of vblanks, acknowledges and video-register
writes says which — in a form CI, a bisect and a commit message can hold. `--ppm`
gives a headless run a picture to look at as well.

Reading it:

- `vblanks` and `acks` should track each other. Vblanks climbing with acks flat
  means the driver is not taking the interrupt.
- `cpu ... HALTED (double bus fault)` means the 68000 took a fault while already
  taking one — usually a wrong memory map or a wrong ROM interleave.
  `stopped (waiting for an interrupt)` is the `STOP` instruction and is normal in
  some idle loops.
- `unmapped` lists the worst eight addresses in each direction. If the
  1,024-distinct-address cap was hit, a `…N more accesses` line says so — a total
  printed without it would read as a complete list.
- The three `cps-` and `gfxram` lines appear on CPS-1 only. On SF1 they are omitted
  rather than printed as zero, because `cps-a writes 0` would read as a finding
  about a driver rather than a fact about a board.

---

## Playing it

```sh
cargo run -p sfemu --release -- /path/to/your/sf2.zip --play
```

**Each player has one half of the keyboard.** Player 1 is on the letters, player 2
on the arrows and the numeric keypad.

```
        Player 1                          Player 2

          Z   (W on QWERTY)                 ↑                7 8 9  kicks
        Q S D    I O P  kicks             ← ↓ →              4 5 6  punches
        ^        K L M  punches
        (A)      (last is `;`)            stick               keypad
        stick     on QWERTY
```

The key names below are **AZERTY** labels, because that is what the usage text
tells you to press. Keys bind to physical positions, not to letters, so on a US
QWERTY board P1's stick reads `W` `S` `A` `D` — the same four keys in the same
diamond — and the third punch is the semicolon. See
[Keyboard layouts](#keyboard-layouts).

### Game controls

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

**To start a game: press `5` to insert a coin, then `1` to start.** The board is a
coin-op; it will sit in attract mode forever otherwise. The DIP switches are set to
Capcom's factory defaults, which is 1 coin per credit.

**Punches sit on the bottom row and kicks directly above them** — the reverse of a
real six-button cabinet, deliberately. On an AZERTY keyboard `K` `L` `M` is a run of
three on the home row, so putting the punches there puts them under the resting
fingers. Both clusters are arranged the same way, which is the property that matters
once the arrangement is unconventional: learning one half teaches you the other. If
you want the cabinet order instead, the key menu has it — see
[The key menu](#the-key-menu).

**Player 2's buttons need a numeric keypad.** Without one the arrow keys still move
but the six attacks are unreachable, so two-player play is a desktop arrangement.

**No letter key is a control.** Pause is `F11`, not `P` — `P` is P1's roundhouse
now. Every non-game control is a function or navigation key, which is what keeps a
punch from also saving a state.

### Session controls

| Key | Does |
|---|---|
| `F3` | Reset the machine |
| `F11` | Pause / resume |
| `.` | Step one frame while paused |
| `F5` / `F8` | Save state / load state |
| `F12` | Screenshot, as a binary PPM |
| `Tab` | Key menu on / off |
| `Esc` | Quit (or close the key menu, if it is open) |

### Debugger and viewers

| Key | Does |
|---|---|
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

### The title bar

Carries `[paused]`, `[no audio]`, `[CPU halted]`, and a dropped-frame count when
there is one. `[CPU halted]` means the 68000 double bus faulted and the loop is
still running so you can see it: that is a bug in this emulator or in the ROM set,
and `F1` is the tool it wants — the status panel says `HALT` and the registers show
where it went.

---

## Keyboard layouts

`minifb::Key` names a **hardware position**, not a letter. On macOS it passes the
raw `[event keyCode]` through a fixed table that names each position after the
letter a US QWERTY keyboard prints there; the active layout is never consulted. So
`minifb`'s `Q` means "position 0x0c", which types `q` on QWERTY and `a` on a French
keyboard.

P1's keys are mapped by position and set up for AZERTY. Three are places the
layouts disagree:

| Board input | `minifb` name | AZERTY label | QWERTY label |
|---|---|---|---|
| P1 up | `W` | **Z** | W |
| P1 left | `A` | **Q** | A |
| P1 fierce | `Semicolon` | **M** | ; |
| P1 down | `S` | S | S |
| P1 right | `D` | D | D |

The third is worth stating twice, because AZERTY does not merely shift `M` — it
moves it off the bottom row entirely, to the home row's right end next to `L`. That
position is the one QWERTY prints `;` on, so `minifb` calls it `Semicolon`, and
`minifb`'s own `M` is the key labelled `,` on a French board. Verified against the
live layout with Carbon's `UCKeyTranslate` rather than reasoned about: on
`French – PC`, position 0x29 types `m` and 0x2e types `,`.

Everything else is layout-stable — `I` `O` `P` `K` `L`, the number row, the keypad,
the arrows and the function keys sit in the same place on both. **So a QWERTY player
uses `W` `S` `A` `D` for the stick, `K` `L` `;` for the punches, and reads every
other row as printed** — or picks a QWERTY preset in the key menu and gets `J` `K`
`L` instead.

---

## The key menu

`Tab` opens it. Four button layouts and a row that puts the default back:

```
   KEYS
 > AZERTY  punches low   (current)
   AZERTY  punches high
   QWERTY  punches low
   QWERTY  punches high
   restore defaults
 P1  Z S Q D   K L M / I O P
 P2  arrows    456 / 789
 up/down move   Enter apply
 Tab close      Esc cancel
```

Up and down move, `Enter` applies and **leaves the menu up** so you can see what
changed, `Tab` or `Esc` closes it. The two summary rows preview the *highlighted*
row's keys and not the active ones, so you can read a layout before choosing it.

**The four presets are two axes, not four**: which row punches, and which triple of
letters the punches are. AZERTY's home-row run of three is `K L M`; QWERTY's is
`J K L`.

| Preset | Tag in the file | P1 punches | P1 kicks |
|---|---|---|---|
| AZERTY punches low (default) | `azerty-punch-low` | `K L M` | `I O P` |
| AZERTY punches high | `azerty-cabinet` | `I O P` | `K L M` |
| QWERTY punches low | `qwerty-punch-low` | `J K L` | `I O P` |
| QWERTY punches high | `qwerty-cabinet` | `I O P` | `J K L` |

**The stick is not on the menu**, and that is a discovery rather than an omission.
AZERTY's `Z S Q D` and QWERTY's `W A S D` are the *same four physical keys*, so one
map already reads correctly on both — only the printed letters differ.

One consequence is visible in play: **which keys are live depends on the preset.**
`J` presses nothing under either AZERTY preset, and `M` presses nothing under
either QWERTY one.

**The menu captures the keyboard.** While it is up the board sees nothing held and
every control but `Tab` is swallowed. `Esc` closing the menu rather than quitting is
the whole point of that capture — and it only closes: pressed with no menu up, it
still quits.

One rough edge, documented rather than fixed: the menu reads the *previous* frame's
actions, so the board is live for the single frame on which `Tab` goes down.

Your choice is written beside the ROM set as `sf2.keys` — **one line of text you
can edit by hand**, holding one of the tags above — and is in force before the first
frame of the next session. A missing, unreadable or unrecognised file is the default
and says nothing: a first run has no file, and a tag from a future version is not
something a player can act on. A failed *write* is a notice, because you asked for
it.

---

## Save states, screenshots and where files go

Everything a session writes goes **beside the ROM set**, named from it:

| Ask | File, for `/games/sf2.zip` |
|---|---|
| `F5` / `F8` save and load a state | `/games/sf2.sfs` |
| `F12` screenshot | `/games/sf2.ppm` |
| The key menu's choice | `/games/sf2.keys` |

Per ROM set rather than one file for the program, so an SF2 session and a CE session
never share one — and the program writes nothing outside the directory you pointed
it at. `--state <path>` moves the state file if you want it elsewhere.

**One state file, not a numbered series: `F5` overwrites.** A state is tagged with
the board it came from and refused by the other, and a state that is damaged,
truncated, or from a future version of the format is refused rather than
half-applied. The machine you are playing keeps running, and the title bar is not
where you find out — failures print as `notice` lines when the session ends, once
each rather than once per frame.

⚠️ **An `F12` screenshot taken with a layer subtracted in the graphics viewer is
missing that layer.** The mask reaches the renderer, and the screenshot comes from
the renderer.

---

## Sound

Audio opens on the default output device and needs no flag. Both rates are printed
at startup:

```
audio: device 48000 Hz, board 55930.391 Hz
```

Both, because the interesting number is the pair: the board's rate is no device's
rate, so the samples are converted rather than played as they are. At the 48 kHz
shown above, handing them over unconverted would play them **14.2% slow** — 2.65
semitones flat.

If no device can be opened, that is a notice on stderr and a `[no audio]` tag in
the title bar — **not** a refusal to run. No sound is a degradation, and a machine
you can watch is better than one that would not start.

If you hear nothing with no `[no audio]` tag, `F1`'s sound panel is the tool:
`CLP` is the mix clamping, `DRP` is the ring overflowing (the emulator outran the
device), `UND` is the ring starving (the device outran the emulator). Silence with
all three at zero and the register counts climbing is a mix problem; `UND` climbing
is a pacing one.

---

## The debugger

`F1` draws the debugger into the emulated framebuffer, over the paused game. Four
panels, three on by default:

- **Registers** — D0–D7 beside A0–A7, then PC, SR, the cycle count and the frame
  count. A7 comes from `a[7]` and never from the `usp`/`ssp` shadows, which are
  stale inside an exception handler — which is where you are when you are reading
  it. The PC shown is the *executing* instruction's, four bytes behind `cpu.pc`,
  because the 68000 prefetches two words.
- **Disassembly** — eight instructions from the follow address, `>` on the executing
  line and `*` on a breakpoint. It follows the machine until you scroll; `Home`
  makes it follow again.
- **Memory** — twelve rows of four words. Off by default: it needs an address to be
  worth its width, and `F6` is how you ask for it.
- **Status** — the flags `XNZVC`, the beam position, and `HALT` or `STOP`.

**`--` in the memory view means nothing decodes at that address** — no chip
answered. That is a different fact from `FFFF`, which means something answered and
read as all ones, and the two are never conflated: conflating them sends you
looking for a chip that is not there.

**Nothing here is writable, and that is a decision rather than a missing feature.**
Every panel reads through a `&self` peek with no side-effect path: a dump scrolled
over the input latch must not acknowledge an interrupt, and a listing pointed at a
vector must not consume it. A debugger that perturbed the machine while you watched
it would make every intermittent bug unreproducible — which is the class of bug you
open a debugger for.

`F4` steps one instruction; `.` steps one whole frame. Both are needed and they are
not interchangeable: a frame is 167,680 cycles, which is where a bug usually is, and
an instruction is where you can see it. A breakpoint stops the machine **mid-frame**,
at the instruction, not at the next frame boundary.

---

## The graphics viewers

`F1` answers questions about the 68000. `F9` answers the other half: the screen is
wrong, and the question is *which stage*. `F10` cycles four views, and `F1`'s panels
draw on top of whichever is up, so you can read a register beside the tile it
produced.

- **Tiles** — the graphics ROM as a greyscale grid. `Enter` cycles the four layouts
  (8×8, 8×8-odd, 16×16, 32×32) and `[`/`]` page by one screenful of whichever is
  shown. *Is the tile in the ROM at all, and does it decode?*
- **Tilemap** — one scroll layer's table around a cursor: the base, the signed
  scroll, 8×8 codes, the cursored code's colour scheme, flip bits and tile group, the
  ROM offset the bank mapper gives it, and the tile itself. `Enter` cycles the layer,
  `[`/`]` walk the cursor; until you move it, it follows the tile the renderer
  fetches for the visible top-left pixel. *Is the map pointing at the tile you
  meant?*
- **Palette** — all 3,072 entries as swatches, with the cursored one's raw hex and
  page, and `0BFF` named as the background pen. *Is the palette entry the colour you
  meant?*
- **Layers** — the four depths back to front: what the hardware enables, what you
  have masked, which depth each layer draws at, and which feeds the sprite occlusion
  mask. `[`/`]` select a row, `Enter` subtracts that layer. *Is the layer even
  enabled, and is it where you think in the stack?*

Three things the screen alone will not tell you:

**Tiles are greyscale on purpose.** A colour scheme is the palette's reading of the
ROM, and the palette has a view of its own. Tinting the browser would make a wrong
decode and a wrong palette look the same — and telling those two apart is what the
browser is for.

**`----` in the tilemap view is the bank mapper saying no.** The tile is *absent*,
not wrong: no bank range covers that code, so the renderer skips it silently. That
is the one failure the composed picture cannot show, which is why printing the
mapper's "no" as `0000` would be worse than useless.

**Turning a layer off changes the picture, not the machine.** The mask reaches the
renderer and nothing else; the 68000 runs the same cycles, writes the same memory
and takes the same interrupts either way. The mask is also kept out of save states,
because one that round-tripped would come back with someone else's layers
subtracted.

And the asymmetry: **you cannot turn a layer *on*.** The mask subtracts only, and
that is structural — the renderer combines the hardware's answer and the mask with
`&&`, hardware first. Forcing a layer on would draw a tilemap from a base register
the game never set up, which is garbage that looks exactly like the tile-decode bug
the viewer exists to rule out.

`F9` hides the box without restoring the layers you subtracted: "show me the game
with scroll 1 off" is the whole point of a layer mask, and it is unreachable if
closing the box turns everything back on.

---

## Speed

The frame budget is **16.768 ms** — CPS-1 runs at 8,000,000 / (512 × 262) =
59.6374 Hz. Against that, 0.749 ms per frame for CPU, render and RGB conversion: a
22× margin, measured on the author's machine on 2026-08-08 with synthetic
worst-case content, and quoted from the spec rather than freshly measured. It is not
a performance guarantee.

Emulation itself is comfortably inside the budget on a real game, measured rather
than quoted — 1,200 frames of `sf2eb` headless in 1.08 s on the author's machine on
2026-08-24, i.e. **0.90 ms per frame, an 18.6× margin**:

```sh
/usr/bin/time -p ./target/release/sfemu ~/roms/sf2eb 1200 --game sf2eb
```

The pacer is what that margin buys: sleep-free and catch-up-bounded. A host tick
that took longer than a frame owes the frames it missed, **up to four**; beyond that
they are dropped and counted, because a machine that fell a second behind should
resync rather than fast-forward through a second of the game. The title bar shows
the dropped count, and so does the report a session prints on the way out:

```
frames        18656
dropped       3246
```

Pausing owes nothing — the clock is only read on a running tick, so a pause does not
accumulate a debt that stampedes when you resume.

⚠️ **Dropped frames are known, unexplained, and can be large.** That report above is
a real 2026-08-24 windowed session: **3,246 of 18,656 frames, 17.4%**. Earlier runs
showed 2–3% and a later one 4.7%, so the rate varies a great deal between sessions.
What is ruled out is emulation cost — at 0.90 ms per 16.768 ms frame, the board is not
what misses the deadline. A drop requires a host tick longer than four frames (67 ms),
so the cause is on the window/present side. Play is unaffected in the sense that the
game does not run slow; it resyncs.

**If your session drops frames, the report says more.** Three extra lines follow
`dropped` whenever it is non-zero:

```
late ticks    812
worst tick    412.3 ms
owed/tick     96 10397 2140 189 41 812
```

- `late ticks` — how many single host ticks dropped anything. `dropped / late ticks` is
  the mean number of frames lost per stall, and it is the number that matters: 3,246 lost
  over 1 late tick is one long freeze, and over 3,246 late ticks it is a loop that is
  persistently a fraction too slow. Different causes, and the old two-line report could
  not tell them apart. Here it is 4.0 frames per stall.
- `worst tick` — the longest single tick in the session. Anything over 67 ms dropped
  something; 412 ms means the host went away for a quarter of a second.
- `owed/tick` — the distribution: how many ticks owed 0 frames, then 1, 2, 3, 4, and then
  the last column, every tick that owed more than the cap. A healthy session is almost
  entirely in the second column. Weight in the fifth means the host is close to the edge
  without having gone over it.

The columns are a complete account of `frames`: each tick in column *n* rendered *n*
frames, each of the 812 over-cap ticks rendered 4, and 10,397 + 4,280 + 567 + 164 +
3,248 = 18,656.

**Those three numbers are invented** — they show the format, not a measurement. No real
reading exists yet, because the report prints on the way out: it takes a windowed session
that a person closes with `Escape` or the close button. If you run one, that output is the
whole diagnosis.

---

## Running the tests

You do not need any of this to play. If you want to run the suite, the vector data
is fetched once per checkout into gitignored `testdata/`:

```sh
# 68000 vectors: ~138 MB over 127 files. Shells out to curl.
cargo run -p testrunner --bin fetch --release
# Z80 vectors: upstream is 1.37 GB of JSON, so each file is converted to a binary
# form and its JSON deleted before the next starts.
cargo run -p testrunner --bin fetchz80 --release
# The two generated suites, each from its own BSD-3 reference implementation.
cargo run -p testrunner --bin genym --release
cargo run -p testrunner --bin genoki --release

# Both profiles. Neither subsumes the other: --release is where the timing law is
# measured, and debug is where debug_assert! is evaluated.
cargo test --workspace --release
cargo test --workspace
```

No game code is fetched by any of that. The suites are freely licensed and
machine-generated.

**Seven tests are `#[ignore]`d**, and they are the only ignored tests in the
project. All seven need a ROM set you supply, and all seven read **one**
environment variable:

```sh
SFEMU_ROMS=/path/to/your/sf2.zip cargo test -p sfemu --test audio_boot -- --ignored
```

`SFEMU_ROMS` is **not** how the binary is pointed at a ROM set — that is a
positional argument. It exists only for these seven, it holds **one** path, and the
CPS-1 tests and the SF1 tests want different sets, so a user with both runs the
suite twice with it pointing at each in turn. There is deliberately no second
variable: it would multiply as games are added and leave every test silently unrun
when a name is misspelled. Pointing it at the wrong set fails loudly with a loader
error naming the file it could not find.

Three of the seven — the `sf1_*` ones — have never been executed, because no SF1 set
has been available.

---

## Troubleshooting

| Symptom | What it usually is |
|---|---|
| Usage text and exit 2 | No arguments. The three shapes are at the top of the usage text. |
| `error: … no such file` naming a ROM file | Wrong `--game` for the set you have, or an incomplete set. The message names the file, and the CRC-32 is checked as well as the name. |
| Window opens, stays black | Look at the headless report for the same set: `cargo run … -- /path/to/set 600`. Vblanks climbing with acks flat is a driver not taking the interrupt; `HALTED` is a double bus fault. |
| Picture draws but a layer is missing | The wrong CPS-B row — check `--game`. This is exactly `sf2ce`'s wrong-row failure mode. Also check you have not left a layer subtracted in `F9`. |
| Game sits in attract mode | Insert a coin (`5`), then start (`1`). |
| P1's stick is on the wrong keys | You are on QWERTY reading AZERTY labels. `W` `S` `A` `D` is the same diamond; see [Keyboard layouts](#keyboard-layouts). |
| P2's buttons do nothing | They are on the numeric keypad, and a keyboard without one cannot reach them. |
| Some P1 punches do nothing | Preset mismatch: `J` is dead under an AZERTY preset, `M` under a QWERTY one. `Tab`. |
| Simultaneous keys drop | Cheap membrane keyboards genuinely drop simultaneous keys. Nothing in software can see this. |
| `[no audio]` in the title | No output device could be opened. The stderr notice says why. |
| Silence with no `[no audio]` | `F1`'s sound panel: `CLP`/`DRP`/`UND`. All zero with register counts climbing is a mix problem. |
| The key menu does not stick | A failed write to the `.keys` file is a notice at session end. Check the directory is writable. |
| A load state is refused | It is from another board, another format version, or damaged. The notice at session end says which. The running machine is untouched. |
| Dropped frames on a fast machine | Known and uninvestigated; see [Speed](#speed). |
| It runs at the wrong speed in a debug build | Build with `--release`. |
