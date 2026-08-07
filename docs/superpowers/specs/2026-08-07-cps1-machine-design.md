# Design: CPS-1 Machine — bus, timing, ROM loader (sfemu sub-project B)

Date: 2026-08-07
Status: Approved (design calls made autonomously; rationale recorded inline)
Scope: Sub-project B of the sfemu arcade emulator

## Context

Sub-project A delivered `crates/m68k`: a cycle-counted 68000 core, dependency-free,
`no_std`-friendly, verified at **127/127 groups and 317,500/317,500 cases** of the
SingleStepTests/m68000 vector suite plus 221 unit tests and five hand-assembled
integration programs. It knows nothing about Capcom hardware and must stay that way.

Sub-project B is the first sub-project in which **real board code executes**. It
supplies the three things the core cannot supply itself:

1. A **memory map** — something for `Bus::read16` to actually read.
2. A **ROM set** — the bytes that map holds, loaded from a user-supplied file.
3. A **frame schedule** — the interleaving of CPU cycles with the video beam,
   including the vblank interrupt that every CPS-1 game's main loop waits on.

B deliberately does **not** render tiles (sub-project C) or run the Z80 (D). Its
deliverable is: *given a legal ROM set, the SF2 program counter walks through
Capcom's boot code, services IRQ 2 at the right scanline, and writes plausible
values into the CPS-A registers, gfxram, and the sound latch.* That last clause is
the whole point — it is the first evidence the 68000 core is right about something
other than a test vector.

### ROMs — the constraint from sub-project A, restated in full

**No ROM is bundled, fetched, downloaded, or committed, by any code in this
repository, for any purpose including diagnostics and test fixtures.** SF1 and SF2
are still-commercial Capcom code.

The loader accepts a **path supplied at runtime** to a MAME-format ROM set
(`sf2.zip`, `sf.zip`) that the user already owns. Legal sources: Capcom Arcade
Stadium, Capcom Fighting Collection, or dumping boards you own.

This has a concrete consequence for B's verification, addressed in §Verification:
**every automated test in B runs on synthetic ROM images this repository
generates**, and the one test that needs a real set is opt-in on a path the user
provides. There is no fixture directory holding "just the boot sector."

### The reference sources

Board facts below were read directly from MAME `master` (BSD-3-Clause,
copyright-holders Paul Leaman) at `src/mame/capcom/cps1.h`, `cps1.cpp`, and
`cps1_v.cpp`, on 2026-08-07. Every address, constant, and register offset in this
spec is quoted from those files, with the source line noted where it is not
obvious. **We read MAME as hardware documentation and reimplement; we do not
translate its code.** Naming follows MAME's conventions so that diffing behaviour
against it stays readable — which is exactly what B's debugging will consist of.

## The measured facts B is built on

### Clocks

| Component | Rate | Source |
|---|---|---|
| 68000 | 10 MHz (`XTAL(10'000'000)`, "verified on pcb") | `cps1.cpp:3912` |
| Z80 | 3.579545 MHz | `cps1.cpp:3918` |
| YM2151 | 3.579545 MHz | `cps1.cpp:3940` |
| OKI MSM6295 | 16 MHz / 4 / 4 = 1 MHz, PIN7_HIGH | `cps1.cpp:3946` |
| Pixel clock | 16 MHz / 2 = 8 MHz | `cps1.h:39` |

SF2 (the `sf2` parent set) uses the 10 MHz config. Some later CPS-1 games use
12 MHz; the machine takes the CPU clock as a parameter rather than hard-coding it.

### Video timing

```
CPS_HTOTAL 512   CPS_HBEND 64   CPS_HBSTART 448     (cps1.h:41-43)
CPS_VTOTAL 262   CPS_VBEND 16   CPS_VBSTART 240     (cps1.h:45-47)
```

Derived, and these derivations are load-bearing enough to be asserted in a test
rather than trusted:

- Line rate = 8 MHz / 512 = **15,625 Hz exactly**
- Frame rate = 8 MHz / (512 × 262) = **59.6374... Hz**
- Visible area = (448 − 64) × (240 − 16) = **384 × 224** — the CPS-1 resolution.
- 68000 cycles per scanline = 10 MHz / (8 MHz / 512) = **640 exactly**.
- 68000 cycles per frame = 640 × 262 = **167,680 exactly**.

That both per-scanline and per-frame CPU-cycle counts are *integers* is a fact
about this specific pair of clocks, and it removes an entire class of
accumulated-fractional-error bug from the scheduler. The scheduler still carries a
cycle *remainder* across scanlines, because the 68000 overshoots — a `DIVS` costs
158 cycles and cannot be cut in half at a scanline boundary — but the remainder is
"how far the last instruction overshot", never a rounding residue.

### Interrupts

CPS-1 wires the 68000's IPL pins directly (`set_interrupt_mixer(false)`), so the
lines are individual, not an encoded level:

- **IPL1 = IRQ 2** — vblank. Asserted at scanline 240 (`cps1.cpp:395-396`).
- **IPL2 = IRQ 4** — raster. Tied high on early B-boards; SF2 does not use it.
- Acknowledgement is a **read side effect**: MAME maps `0xfffff2-0xffffff` in the
  CPU space so that the 68000's autovector fetch itself clears IPL1 and IPL2
  (`cps1.cpp:407-422`).

That last point is where B meets a real gap in the core, and it drives a design
decision (§The interrupt-acknowledge problem).

### Main 68000 memory map (`cps1.cpp:577-594`)

| Range | Access | Meaning |
|---|---|---|
| `0x000000-0x3FFFFF` | R | Program ROM (`CODE_SIZE 0x400000`) |
| `0x800000-0x800007` | R | `IN1` — player controls, 16-bit |
| `0x800018-0x80001F` | R | `IN0` and DSWA/DSWB/DSWC, one per word offset |
| `0x800020-0x800021` | R | unmapped by the PAL; reads as no-op |
| `0x800030-0x800037` | W | coin counters / lockouts |
| `0x800100-0x80013F` | W | **CPS-A** registers (32 words) |
| `0x800140-0x80017F` | R/W | **CPS-B** registers (32 words) |
| `0x800180-0x800187` | W | sound command latch |
| `0x800188-0x80018F` | W | sound fade latch |
| `0x900000-0x92FFFF` | R/W | **gfxram**, 192 KB (SF2CE executes code from here) |
| `0xFF0000-0xFFFFFF` | R/W | main RAM, 64 KB |

`cps1_dsw_r` returns `(in << 8) | 0xff` (`cps1.cpp:271`) — the selected byte in the
**high** half and `0xFF` in the low half. Inputs are **active low**: an unpressed
button reads 1, so an idle board reads `0xFFFF` everywhere in the port block, and a
model that returns 0 for "nothing pressed" boots into every button held down.

`IN1` is a single 16-bit port mapped across 8 bytes, so `0x800000`, `0x800002`,
`0x800004`, and `0x800006` all read the same word. Likewise `cps1_dsw_r`'s four word
offsets over `0x800018-0x80001F` select `IN0`, `DSWA`, `DSWB`, `DSWC` in that order
(`cps1.cpp:257-272`) — offset 0 is `IN0`, offsets 1-3 are the DIP banks, and there
is no fifth.

For SF2, `IN0` bit layout (`cps1.cpp:830-838`, active low): `0x01` coin 1, `0x02`
coin 2, `0x04` service 1, `0x10` start 1, `0x20` start 2, `0x40` service switch.
`IN1` carries both players' sticks and three punches each (bits 0-6 for P1, 8-14 for
P2); the three kicks live in `IN2`, read through CPS-B at `in2_addr` — which is why
`in2_addr` is boot-relevant rather than a nicety.

### CPS-A register file (`cps1.h:176-193`)

Offsets below are **byte offsets from `0x800100`**, which is how MAME's table and
its per-game config both express them; MAME's `CPS1_*` constants are those offsets
already divided by 2, because its arrays are `uint16_t`. Every offset here is even,
so "byte offset" and "word index × 2" agree — but they are not the same number, and
mixing them is a one-register shift that reads scroll-Y as scroll-X. The rule for
this file: **an address is a byte offset; an array index is a word index; the
conversion is written at every boundary.**

```
0x00 OBJ_BASE        0x02 SCROLL1_BASE   0x04 SCROLL2_BASE   0x06 SCROLL3_BASE
0x08 OTHER_BASE      0x0A PALETTE_BASE
0x0C SCROLL1_SCROLLX 0x0E SCROLL1_SCROLLY
0x10 SCROLL2_SCROLLX 0x12 SCROLL2_SCROLLY
0x14 SCROLL3_SCROLLX 0x16 SCROLL3_SCROLLY
0x18 STARS1_SCROLLX  0x1A STARS1_SCROLLY
0x1C STARS2_SCROLLX  0x1E STARS2_SCROLLY
0x20 ROWSCROLL_OFFS  0x22 VIDEOCONTROL
```

B stores these as a plain `[u16; 32]` and interprets **none** of them. C does the
interpreting. B's only job is that a write lands and a subsequent read of the
CPS-B space does not accidentally see it.

### CPS-B and the boot-blocking protection

CPS-B is not plain RAM. `cps1_cps_b_r` (`cps1_v.cpp:2136-2161`) intercepts reads:

- The **CPSB ID register**: reading offset `cpsb_addr` returns the constant
  `cpsb_value` instead of what was written. Games check this on boot.
- A 16×16→32 **multiply protection** (later boards only): write two factors, read
  the 32-bit product back from two other registers.
- Extra input ports for 3/4-player boards.

For `sf2` the config row is (`cps1_v.cpp:1838`):

```
{"sf2",  CPS_B_11,  mapper_STF29,  0x36 }
```

and `CPS_B_11` expands to (`cps1_v.cpp:491`):

```
cpsb_addr 0x32   cpsb_value 0x0401   multiply: not applicable
layer_control 0x26   priority {0x28,0x2a,0x2c,0x2e}   palette_control 0x30
layer_enable_mask {0x08,0x10,0x20,0x00,0x00}
```

The trailing `0x36` is `in2_addr` — SF2's six-button kick inputs are read through
the CPS-B space at `0x800140 + 0x36 = 0x800176`, not through the `0x800000` port
block. **This is boot-critical and is the single most likely thing to be wrong
first**: SF2's ID check reads `0x800140 + 0x32 = 0x800172` and expects `0x0401`. A
board model that treats CPS-B as RAM returns whatever was last written there and
the game stops with a self-test failure. B implements the ID register and the
`in2_addr` port; it does **not** implement multiply protection, because
`__not_applicable__` on `CPS_B_11` says SF2's board has none. A later game that
needs it adds it to the same table.

### MAME ROM set for `sf2` (`cps1.cpp:7101-7133`)

The loader must reproduce this interleaving exactly.

**`maincpu`, 68000 code, `ROM_LOAD16_BYTE`** — pairs of 128 KB files interleaved
byte-wise, **even file to the even (high) byte**:

```
sf2e_30g.11e  0x00000  0x20000     sf2e_37g.11f  0x00001  0x20000
sf2e_31g.12e  0x40000  0x20000     sf2e_38g.12f  0x40001  0x20000
sf2e_28g.9e   0x80000  0x20000     sf2e_35g.9f   0x80001  0x20000
sf2_29b.10e   0xc0000  0x20000     sf2_36b.10f   0xc0001  0x20000
```

So 8 × 128 KB = 1 MB of code at `0x000000-0x0FFFFF`, and `0x100000-0x3FFFFF` is
unpopulated. `ROM_LOAD16_BYTE` with offset `2n` means "this file supplies every
byte at even addresses", i.e. the **high** byte of each big-endian word.

**`gfx`, 6 MB, `ROM_LOAD64_WORD`** — four 512 KB files interleaved at 16-bit
granularity into a 64-bit stride, three such groups. B loads this region and
exposes it; it decodes nothing (C's job).

**`audiocpu`, 0x18000** — one 32 KB file with a `ROM_CONTINUE`: the first 32 KB
lands at `0x00000` and the same file's second half at `0x10000`. B loads it for D
to use; nothing reads it in B.

**`oki`, 0x40000** — two 128 KB sample files, concatenated.

The PLD regions (`aboardplds`, `bboardplds`, `cboardplds`) are ignored: they are
PAL equations, not board data we simulate.

## Architecture

Two new crates, plus one addition to `m68k`.

```
crates/
  m68k/          # A — UNCHANGED. B adds nothing and edits nothing here.
  romset/        # B1 — MAME ROM-set loading: zip reading, interleave, verify
  machine/       # B2 — the CPS-1 board: memory map, scheduler, frame loop
  sfemu/         # B3 — thin binary: parse args, load ROMs, run frames, print trace
  testrunner/    # A — unchanged
```

`m68k` is listed to say explicitly that it is **not** modified. An earlier draft of
this line proposed a `Bus::read16_vector` addition; that is the same change as
option 1 in §The interrupt-acknowledge problem, which this spec decided *against*.
The two statements would have contradicted each other. If implementation shows the
core genuinely must change, that is a plan contradiction to surface before touching
it — the trait has 317,500 verified cases wired through it.

`sfemu` exists so that neither library crate has a `main`: `romset` needs `std` and
a zip decoder, `machine` needs neither, and the binary is where they meet. It is
~100 lines and holds no logic worth testing beyond argument parsing.

`romset` and `machine` are separate crates because they fail differently and are
tested differently. `romset` is fallible host-facing I/O returning `Result` with a
real error type; `machine` is an infallible pure-data simulation that must never
panic on guest input. Mixing them would make `machine` depend on `std` and on a
zip decoder, which forfeits the WASM and `no_std` posture that A paid for.

**No window in B.** The spec's original sketch for B included "minimal window".
Cut, deliberately: a window shows nothing until C renders tiles, so the only thing
it would prove is that `winit` links. B's observable output is a **trace** — a
text log of frames, PC samples, and register writes — which is a far better
debugging instrument for the actual question ("is the boot code progressing?") and
carries zero GUI dependencies. E owns the window. This is a scope *reduction*
against the sub-project table, recorded here so it is a decision and not a
forgotten requirement.

### `romset` — public interface

```rust
pub struct RomSet {
    pub regions: BTreeMap<String, Vec<u8>>,   // "maincpu", "gfx", "audiocpu", "oki"
}

pub struct RomEntry {
    pub name: &'static str,       // "sf2e_30g.11e"
    pub offset: usize,            // 0x00000
    pub len: usize,               // 0x20000
    pub crc32: u32,               // 0xfe39ee33
    pub load: LoadKind,
}

pub enum LoadKind {
    /// Straight copy at `offset`.
    Byte,
    /// `ROM_LOAD16_BYTE`: file byte i lands at `offset + 2*i`.
    Word16Byte,
    /// `ROM_LOAD64_WORD`: file word i lands at `offset + 8*i` (2 bytes).
    Word64Word,
    /// `ROM_CONTINUE`: after `len` bytes at `offset`, the rest goes to `cont_at`.
    Continue { split: usize, cont_at: usize },
}

pub struct RegionSpec { pub name: &'static str, pub size: usize, pub entries: &'static [RomEntry] }
pub struct GameSpec   { pub name: &'static str, pub regions: &'static [RegionSpec] }

pub fn load(spec: &GameSpec, zip_path: &Path) -> Result<RomSet, RomError>;
```

`GameSpec` is a const table, one row per supported set, transcribed from MAME's
`ROM_START`. It contains **file names, offsets, lengths, and CRCs — no ROM data**.
A table of names and checksums is metadata about a product, not the product; this
is the same category as a package manifest and it is what makes "the user supplies
the file" checkable at all.

**CRC-32 is verified per entry and a mismatch is an error, not a warning.** A
wrong or bad-dump ROM produces a 68000 that executes garbage, and the failure
surfaces thousands of instructions later as an unexplained address error. Checking
32 bits up front converts a week of debugging into one line of output.

Zip reading: **hand-written**, using `miniz_oxide` (2 crates, pure Rust, no C) for
the DEFLATE stage only. Rationale over the `zip` crate: `zip` pulls **75 crates**
including AES, bzip2, zstd, PPMd, `time`, and `sha1`. MAME sets use exactly two
methods, stored (0) and deflate (8), on a central directory we can parse in ~120
lines. The 75-crate tree is 73 crates of attack surface and compile time for
features no ROM set uses, in a project whose defining constraint is a
dependency-free core. `machine` and `m68k` stay at **zero** dependencies; only
`romset` gains two.

A directory of loose files is also accepted (`load` dispatches on
`path.is_dir()`), because a user who owns the board and dumped it themselves has
loose files, and refusing them would push them toward re-zipping for no reason.

### `machine` — public interface

```rust
pub struct Cps1 {
    pub cpu: m68k::M68k,
    pub board: Board,           // all mutable board state; implements m68k::Bus
    pub timing: Timing,
}

pub struct Board {
    pub rom: Vec<u8>,            // 0x400000, zero-filled beyond the populated part
    pub ram: Box<[u16; 0x8000]>, // 0xFF0000-0xFFFFFF, word-addressed
    pub gfxram: Box<[u16; 0x18000]>, // 0x900000-0x92FFFF
    pub cps_a: [u16; 0x20],
    pub cps_b: [u16; 0x20],
    pub inputs: Inputs,
    pub sound_latch: [u8; 2],
    pub coin_ctrl: u16,
    pub cfg: BoardConfig,        // cpsb_addr/value, in2_addr, ... from the table
    pub unmapped: UnmappedPolicy,
    pub trace: Trace,
}

pub struct Timing {
    pub cpu_hz: u32,             // 10_000_000
    pub cycles_per_line: u32,    // 640
    pub lines_per_frame: u32,    // 262
    pub vblank_line: u32,        // 240
}

impl Cps1 {
    /// `prog` is the assembled `maincpu` region: up to `0x400000` bytes, big-endian,
    /// copied in and zero-padded. Deliberately a byte slice and **not** a
    /// `romset::RomSet`.
    pub fn new(prog: &[u8], cfg: BoardConfig, timing: Timing) -> Self;
    pub fn reset(&mut self);
    pub fn run_scanline(&mut self) -> u32;   // returns cycles actually run
    pub fn run_frame(&mut self);
}
```

**`machine` does not depend on `romset`.** An earlier draft of this interface took
`&RomSet`, which would have made the zero-dependency claim below false by two
crates transitively — `machine` would inherit `miniz_oxide` and `adler2`, and with
them `std`, forfeiting the WASM posture. Taking `&[u8]` keeps the dependency arrow
pointing one way only: a thin binary crate (or a test) owns both and hands the
region across. It also means every `machine` test constructs its program inline,
with no archive anywhere in the loop — which is what makes §Verification's
synthetic programs possible at all.

`Board` is the `Bus` impl and is a separate struct from `Cps1` for a borrow-checker
reason that is worth stating because it shapes every call site: `step_with(&dec,
&mut bus)` needs `&mut cpu` and `&mut bus` simultaneously, so the CPU cannot live
inside the thing it buses to. Splitting them at the top level makes
`self.cpu.step_with(&self.dec, &mut self.board)` legal with no `RefCell`, no
`unsafe`, and no state swapping — preserving A's `forbid(unsafe_code)` posture.

### The frame schedule

```
for line in 0..262:
    if line == 240: board.assert_vblank_irq()   # IPL1 -> cpu.set_irq(2)
    budget = 640 + carry
    while budget > 0:
        budget -= cpu.step_with(&dec, &mut board)     # may overshoot
    carry = budget                                    # negative; the overshoot
```

`carry` is the *negative* overshoot of the last instruction, carried into the next
line's budget. Over a frame the total is exactly 167,680 plus the final line's
overshoot, so no drift accumulates.

Scanline granularity rather than per-instruction video sync: the accuracy target
locked in sub-project A is "cycle-counted CPUs with scanline video", and C's
renderer will consume `Board` state per scanline. Anything finer would be work C
cannot use.

### The interrupt-acknowledge problem

This is the one place where B's requirements exceed what A's core provides, and it
needs an explicit decision rather than a workaround discovered mid-implementation.

On real CPS-1 hardware the interrupt is cleared **by the 68000's own autovector
fetch**: the CPU asserts FC=7 with a vector address in `0xFFFFF2..0xFFFFFF`, the
board decodes that and drops IPL1. `crates/m68k`'s `set_irq` is documented as
level-triggered with the *caller* owning deassertion, and `Bus` has **no function
code**, so the board cannot see the acknowledge cycle: an autovector fetch of
vector 26 is indistinguishable from `MOVE.L $68,D0`.

Three options were considered.

1. **Widen `Bus` with a function code.** Most faithful, and it closes the blind
   spot flagged at the end of sub-project A (`Transaction.fc` is the vector
   suite's one unchecked field, with 4 distinct values over 1,450,409 non-idle
   transactions). But it is a **breaking change to a trait that 317,500 verified
   test cases are wired through**, for one board that needs one bit of it.
2. **Deassert from the machine, one scanline after assertion.** Simple; wrong in a
   way that hides. If the handler is slow the line is already gone by the time it
   returns and the next assert is missed; if it is fast the same interrupt is taken
   twice.
3. **Deassert on the vector fetch, detected in the board's `read16`.** The board
   knows the frame position and knows it just asserted IPL1; a `read16` of
   `0x000068` or `0x00006A` (vector 26 = autovector level 2) while that assertion
   is outstanding is, on this board, unambiguously the acknowledge cycle.

**Decision: option 3, with option 1 recorded as the correct fix and deferred to
whichever sub-project first needs FC for a chip select.** Option 3 is exact
whenever the vector table is in ROM, which it is on CPS-1 — no game reads its own
vector 26 longword as data, and if one did, the read would return the same value
either way. The imprecision is bounded and stated, rather than the *timing*
imprecision of option 2 which is unbounded and silent.

The trap this must not fall into: **`ack_pending` is an internal flag, and a test
that asserts the flag proves nothing.** The tests assert the observable artifact —
that a second `run_frame` with no new assertion takes vblank exactly once, counted
by the handler's own instruction stream — not that `board.ack_pending == false`.
(This project has produced that exact defect before; see
`docs/hardware/68000-notes.md` on self-consistent assertions.)

### Unmapped-access policy

`UnmappedPolicy` is `Warn` (record in the trace, return `0xFFFF` / discard) or
`Panic` (host-fault: our board model is incomplete). Default `Warn`, because a
guest write to an address MAME's PAL also leaves unmapped is guest behaviour and
must not stop the emulator — but the trace counter makes it visible, and a boot
that stalls with 40,000 unmapped writes to one address has just told you which
chip you forgot.

### Trace

```rust
pub struct Trace {
    pub frames: u64,
    pub vblanks: u64,
    pub cps_a_writes: u64,
    pub cps_b_writes: u64,
    pub gfxram_writes: u64,
    pub sound_latch_writes: u64,
    pub unmapped: BTreeMap<u32, u64>,   // address -> count
    pub pc_samples: Vec<u32>,           // one per scanline, capped
}
```

This is the deliverable's actual output surface. "Does SF2 boot?" becomes a
checkable question: after N frames, is `vblanks == N`, is `cps_a_writes > 0`, did
`sound_latch_writes` happen (the game asking the Z80 to play the attract music),
and are the `pc_samples` inside the populated ROM rather than in an exception
handler loop?

## Verification

This is the section that matters most, because **B has no vector suite**. A's
defining defect class was "the claim that cannot fail" — vacuous verification —
and A caught it only because 317,500 external cases existed to contradict it. B
has no such oracle. What replaces it:

### 1. Derived constants are asserted against their derivations

Every number in §Video timing is computed in a test from the MAME-quoted
primitives and compared to a **literal**:

```rust
#[test]
fn cps1_frame_geometry_is_384x224_at_59_63hz() {
    assert_eq!(CPS_HBSTART - CPS_HBEND, 384);
    assert_eq!(CPS_VBSTART - CPS_VBEND, 224);
    assert_eq!(PIXEL_CLOCK / CPS_HTOTAL, 15_625);           // lines per second
    assert_eq!(CPU_HZ / (PIXEL_CLOCK / CPS_HTOTAL), 640);   // cycles per line
    assert_eq!(640 * CPS_VTOTAL, 167_680);                  // cycles per frame
}
```

The literals are load-bearing and each one gets a **watched mutant**: change
`CPS_HTOTAL` to 511 and the test must go red. An identity that recomputes both
sides from the same constant is exactly the vacuous-assertion shape and is
forbidden here.

### 2. Synthetic ROM images — the standin for the vector suite

`machine`'s tests build ROM images **in the test**, as hand-assembled 68000
programs (the technique Task 12 of sub-project A already established in
`crates/testrunner/tests/integration_asm.rs`, five programs, no assembler needed).
Each is a few words of machine code with a literal expected outcome:

- a program that counts vblanks in RAM, run for 3 frames → RAM holds 3
- a program that reads `0x800172` and branches on `≠ 0x0401` to a marker → the
  non-marker path is taken (this is SF2's actual ID check, in miniature)
- a program that writes a pattern to gfxram and reads it back through both `.w`
  and `.b` accesses → high/low byte placement is right
- a program that reads `0x800018` with no buttons pressed → gets `0xFFFF`
- a program that spins in `STOP #$2000` → the vblank IRQ wakes it, once per frame

These are *not* a substitute for a vector suite and this spec does not pretend
otherwise: they cover the paths we thought of. Their virtue is that each has an
expected value written as a literal by hand, so none of them can be
self-consistent with the code under test.

### 3. The loader is tested against ROM sets we generate

`romset`'s tests build zip archives in a temp directory containing files of the
right names and lengths filled with a **known synthetic pattern**, and assert the
interleave placed byte `i` where the `LoadKind` says. Because we choose the
pattern, `Word16Byte` vs `Byte` is distinguishable — a pattern of all zeroes would
make every interleave look identical, which is the crossed-widths trap in another
costume. Concretely: file A is `0xA0 + (i & 0x0F)`, file B is `0xB0 + (i & 0x0F)`,
so the loaded region must read `A0 B0 A1 B1 ...` and any other interleave is
visibly wrong at byte 1.

CRC verification is tested in both directions: a correct synthetic CRC loads, a
single flipped bit fails with an error naming the file.

### 4. The real-ROM test is opt-in and never bundled

One integration test reads an environment variable naming a directory or zip:

```
SFEMU_ROMS=/path/to/sf2.zip cargo test -p machine --test boot -- --ignored
```

Marked `#[ignore]` — **this is the one place the fail-loudly rule does not apply,
and the reason is not convenience.** The rest of the project's rule ("missing test
data fails loudly, no env-var escape hatch") exists because that test data is
legally fetchable with a documented command. This data is not: there is no command
we may put in the failure message. A test that hard-fails on a machine that
legally cannot have the file is a broken test, not a strict one. So it skips by
default, and CI's absence of it is honest rather than hidden.

When it does run, it asserts on the trace: 60 frames complete, `vblanks == 60`,
`cps_a_writes > 0`, no unmapped access outside a documented allowlist, and every
`pc_sample` inside `0x000000-0x0FFFFF` or `0xFF0000-0xFFFFFF`.

### 5. Cross-checking against MAME's own numbers

Where MAME prints or computes a figure we can compute too — frame rate, the
`0x0401` ID value, region sizes summing to `CODE_SIZE` — the test compares against
the value read from the source, cited by file and line in a comment. A citation
that names a line is self-dating: if the line no longer says that, the comment is
wrong and can be found.

### Definition of done for sub-project B

- `romset` loads a `GameSpec` from both a zip and a directory, verifies every
  CRC-32, and reports a missing or corrupt file by name.
- `machine` implements the full main map above, with CPS-B's ID register and
  `in2_addr` port.
- The five synthetic programs of §2 pass, each with a watched mutant.
- Frame geometry test passes with all four literals mutation-checked.
- `cargo clippy -D warnings` and `cargo fmt --check` clean across the workspace;
  `cargo doc` with no warnings.
- Sub-project A's suite still at **127/127, 317,500/317,500** and 221 unit tests —
  B must not touch the core. If it must (the FC question), that is a plan
  contradiction to surface, not a change to make quietly.
- `machine` has **zero** dependencies; `romset` has exactly two
  (`miniz_oxide`, `adler2`).
- Hardware notes for CPS-1 accumulated in `docs/hardware/cps1-notes.md`, in the
  same style as `68000-notes.md`: every figure with its width, denominator, and
  scope, and every derived rule stating what would refute it.

## Error handling

Same split as A, and the crate boundary enforces it.

**Guest faults** — the emulated program reads unmapped space, jumps into gfxram,
writes to ROM. Never a Rust error. `machine` follows the board: reads of unmapped
space return `0xFFFF`, writes to ROM are discarded, both counted in the trace.
Access to a genuinely absent chip is board behaviour, not our bug.

**Host faults** — missing zip, bad CRC, a `GameSpec` whose entries overflow their
region. `Result<_, RomError>` in `romset`, with the file name in every variant.

**`machine` never panics on guest input**, inherited from A verbatim: every guest
address is masked into range by construction (`ram` indexed by
`(addr >> 1) & 0x7FFF`, not by a checked slice index that could panic on a wild
address). A `GameSpec` inconsistency, by contrast, is our bug and is a
`debug_assert` at construction plus a `Result` at load.

## Out of scope for B

- Tile decoding, tilemaps, sprites, palette — C.
- Z80 execution, YM2151, OKI — D. The sound latch is written and counted; nothing
  reads it. This is exactly how CPS-1 behaves from the 68000's side: the latch is
  fire-and-forget, so a stubbed sound CPU does not block anything.
- Window, input from a keyboard, audio output — E. `Inputs` is a plain struct the
  test or a future frontend sets; B has no host input path.
- The raster interrupt (IPL2). Tied high on SF2's B-board; a later game that needs
  it adds it to `Timing`.
- Multiply protection and the gfx bank mapper. Both are in MAME's per-game config;
  SF2's row needs neither for the 68000 to boot, and the bank mapper is a graphics
  concern (C).
- CPS-1 games other than SF2. `GameSpec` is a table, so adding a row is cheap; the
  spec is done when one row works end to end.

## Implementation order

Each stage ends green.

1. `romset` skeleton: `GameSpec`/`RomEntry` types, the four `LoadKind`s applied to
   in-memory byte slices, tested against the synthetic pattern. No file I/O yet —
   the interleave arithmetic is the part that is easy to get subtly wrong and it is
   testable in complete isolation.
2. Zip reading: central directory parse, stored and deflate, `miniz_oxide` for the
   latter. Tested against archives the test builds.
3. Directory loading, CRC verification, error variants. `load()` complete.
4. The `sf2` `GameSpec` table row, transcribed from `cps1.cpp:7101-7133`, with a
   test asserting the region sizes and the entry count.
5. `machine`: `Board` with RAM, gfxram, ROM, and the `Bus` impl for those three
   ranges only. Synthetic program: write and read back RAM and gfxram.
6. The I/O ranges: inputs, DSW, coin control, sound latch, CPS-A, CPS-B with the ID
   register and `in2_addr`. Synthetic programs for the ID check and the DSW read.
7. `Timing`, `run_scanline`, `run_frame`, `carry`. The geometry test with its four
   mutation-checked literals.
8. The vblank interrupt and the vector-fetch acknowledge. Synthetic programs: the
   vblank counter and the `STOP` wake.
9. `Trace`, the unmapped-access map, and the opt-in real-ROM boot test.
10. `docs/hardware/cps1-notes.md`.

## Sources

- MAME `master`, BSD-3-Clause, copyright-holders Paul Leaman:
  `src/mame/capcom/cps1.h`, `cps1.cpp`, `cps1_v.cpp`, read 2026-08-07. Every
  board fact in this spec is cited to a line in these files.
- Motorola M68000 User's Manual for the autovector acknowledge cycle and FC
  encoding.
- `docs/superpowers/specs/2026-08-05-m68000-core-design.md` for the sub-project
  decomposition and the ROM constraint this spec restates.
- `docs/hardware/68000-notes.md` for the defect classes §Verification is built to
  avoid.
