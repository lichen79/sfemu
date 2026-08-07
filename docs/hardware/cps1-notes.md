# CPS-1 hardware notes

Confirmed behavior of the Capcom Play System 1 board, recorded as we verify it.
Same discipline as `68000-notes.md`: everything here is backed by a test in this
workspace, a line of MAME source read and cited, or an explicit measurement —
never by recollection.

Where a claim rests on MAME rather than on a test, it says so and names the file
and line. MAME `master` was read on 2026-08-07; the relevant files are
`src/mame/capcom/cps1.cpp`, `cps1.h`, and `cps1_v.cpp`, BSD-3-Clause,
copyright-holders Paul Leaman.

⚠️ **No ROM data appears in this repository, in this file, or in any test.** SF1
and SF2 are still-commercial Capcom code. The loader takes a path to a MAME-format
set the user supplies; legal sources are Capcom Arcade Stadium, Capcom Fighting
Collection, or a board you own and dumped.

---

## Clocks and derived timing

Two crystals, and every other number on this page is derived from them by exact
integer division. The primitives are MAME's `cps1.h:39-47`, which credits Charles
MacDonald's measurements of a real board (`cps1.h:30-38`).

```
CPS_PIXEL_CLOCK = XTAL(16'000'000) / 2 = 8,000,000 Hz     cps1.h:39
CPS_HTOTAL      = 512 pixel clocks per scanline           cps1.h:41
CPS_HBEND       = 64    first visible column              cps1.h:42
CPS_HBSTART     = 448   one past the last visible column  cps1.h:43
CPS_VTOTAL      = 262   scanlines per frame               cps1.h:45
CPS_VBEND       = 16    first visible scanline            cps1.h:46
CPS_VBSTART     = 240   one past the last visible line    cps1.h:47
68000 clock     = XTAL(10'000'000), "verified on pcb"     cps1.cpp:3911
```

Derived, each with its arithmetic:

| Figure | Derivation | Value |
|---|---|---|
| Visible width | `HBSTART - HBEND` = 448 − 64 | **384 px** |
| Visible height | `VBSTART - VBEND` = 240 − 16 | **224 px** |
| Line rate | `PIXEL_CLOCK / HTOTAL` = 8,000,000 / 512 | **15,625 Hz** |
| Frame rate | `PIXEL_CLOCK / (HTOTAL × VTOTAL)` = 8,000,000 / 134,144 | **59.6374 Hz** |
| CPU cycles per line | `CPU_HZ / line rate` = 10,000,000 / 15,625 | **640** |
| CPU cycles per frame | 640 × 262 | **167,680** |
| Vertical blanking | `VTOTAL - (VBSTART - VBEND)` = 262 − 224 | **38 lines** |
| Vblank CPU budget | 38 × 640 | **24,320 cycles** |

**Both divisions are exact.** 8,000,000 mod 512 = 0 and 10,000,000 mod 15,625 = 0,
so the scheduler needs no fractional accumulator: there is no remainder to
accumulate. The 12 MHz CPS-1 variant (`cps1_12MHz`) gives 12,000,000 / 15,625 =
**768**, exact as well. This is why `cycles_per_line` is a `Timing` field rather
than a constant, and why `cps1_frame_geometry_is_384x224_at_59_63_hz` asserts both
remainders are zero — a future board whose clocks do *not* divide evenly cannot be
added without that test failing.

MAME's own comment at `cps1.h:36` reads "Refresh rate: 59.63 MHz". The unit is a
typo there; the number is not.

**Why the literals matter more here than anywhere else in the project.** A timing
bug is not a crash. 639 cycles per line instead of 640 runs 0.16% slow: music
drifts against animation over the course of a match and nothing ever looks broken
enough to investigate. So every derived figure above is asserted in
`crates/machine/src/timing.rs` against a number written by hand from the
arithmetic. `assert_eq!(a / b, a / b)` passes for every value of `a` and `b`,
including wrong ones — that assertion shape is this branch's characteristic
defect, and the tests are written to avoid it. `the_default_timing_matches_the_
derivation` is the one that catches a hand-edited `cycles_per_line`: the geometry
test never reads the struct, so it would stay green.

One derived figure is checked *two* ways because the pair of real boards agree on
it: `cycles_per_frame()` is `cycles_per_line × lines_per_frame`, and both real
CPS-1 variants have 262 lines, so a mutant returning `cycles_per_line × 262`
survived every test until `cycles_per_frame_is_the_product_and_not_a_constant`
added a 100-line frame no board uses. A constant shared by every real
configuration cannot be pinned by real configurations alone.

### The scanline scheduler

`Cps1::run_scanline` grants `cycles_per_line + carry` cycles, where `carry` is the
previous line's overshoot as a value `≤ 0`.

The 68000 cannot be stopped mid-instruction — a `divs` costs 158 cycles and does
not divide at a scanline boundary — so overshoot is inherent. Carrying it forward
means the only error at any moment is the *current* line's overshoot, never a sum
of them: after ten frames the total is still within one instruction of
1,676,800, not within ten. `ten_frames_do_not_drift` asserts exactly that, and
`the_running_total_stays_within_one_instruction_of_the_budget_every_line` asserts
it line by line, because a frame-level bound is satisfiable by a scheduler that
runs some lines short and others long.

Two subtleties in testing a scheduler, both learned the hard way:

- **The test program's own cost must not divide the budget.** A lone `bra.s -2`
  costs 10 cycles and 640 / 10 = 64 **exactly**, so every line would end precisely
  on its budget, the carry would be zero for the whole run, and a test built on it
  cannot distinguish a working carry from `self.carry = 0`. The programs use
  `nop` + `bra.s` = 14 cycles, and 640 = 45 × 14 + 10, so the loop straddles nearly
  every boundary. `the_test_program_straddles_scanline_boundaries` asserts that
  premise directly, by counting lines that spend *fewer* than 640 cycles — only
  possible if a debt was carried in.
- **A bound on one line cannot see a leaked carry.** Spend is quantised to whole
  instructions, so a budget of 636 and a budget of 640 both spend 644: a `640..650`
  assertion on the first line after a reset left the `self.carry = 0`-removed
  mutant alive. `reset_restores_the_schedule_exactly` instead compares two runs
  line for line, and takes the reset after eight lines because the pattern has
  period three — resetting after a multiple of three finds the carry already zero
  and proves nothing.

Granularity is per scanline and not per instruction, because sub-project C's
renderer consumes `Board` state per scanline. Anything finer is work C cannot use.

---

## The memory map

`cps_state::main_map`, `cps1.cpp:577-594`. Ranges MAME gives no handler are
genuinely undecoded and float high.

The "MAME line" column cites the individual `map(...)` line where one was read
directly, and the handler function's line range in every case. Where a map line is
not cited individually, the range `577-594` is the citation and only the handler
line is exact — the map lines were not each read, and interpolating them from their
neighbours would be a guess dressed as a reference.

| Range | Direction | MAME line | What it is |
|---|---|---|---|
| `000000-3FFFFF` | R, W discarded | `cps1.cpp:4063` (`CODE_SIZE`) | Program ROM space. SF2's files populate only `000000-0FFFFF`; the rest reads as zero. |
| `800000-800007` | R | `:579-580` | IN1 — player controls. One 16-bit port across 8 bytes: all four word offsets read the same value. `:580` gives it no write handler, so it is read-only. |
| `800018-80001F` | R | handler `:257-272` | `cps1_dsw_r`: four word offsets select IN0, DSWA, DSWB, DSWC in that order, and there is no fifth. |
| `800020-800021` | R | `:583` | `nopr()` — decoded, returns nothing in particular. |
| `800030-800037` | W | handler `:316-327` | `cps1_coinctrl_w` — coin counters and lockouts. |
| `800100-80013F` | W | `:586`, handler `cps1_v.cpp:2115` | CPS-A register file, 32 words. **Write-only** — `:586` gives it no read handler. |
| `800140-80017F` | R/W | `:589`, handlers `cps1_v.cpp:2139`, `:2183` | CPS-B register file, 32 words. |
| `800180-800187` | W | handler `:300-306` | `cps1_soundlatch_w` — the sound command. |
| `800188-80018F` | W | handler `:308-312` | `cps1_soundlatch2_w` — the fade latch. |
| `900000-92FFFF` | R/W | `:592` | gfxram, 192 KB. Readable as well as writable: SF2CE executes code from it. |
| `FF0000-FFFFFF` | R/W | `:593` | Main RAM, 64 KB. |

Everything else is unmapped. `cps1.cpp:581` records that a handful of games read
`0x800010` as a mirror; SF2 is not one of them, and that range is undecoded here.

**Inputs are active low.** An unpressed button reads 1, so an idle board reads
`0xFFFF` everywhere in the port block, and a model returning 0 for "nothing
pressed" boots into every button held down.
`an_unpressed_board_reads_all_ones_through_the_dip_port` pins it.

The map as a whole — not any single port — is what an off-by-one in an arm's bounds
breaks, and no single-port test can see it.
`the_io_block_decodes_exactly_the_ranges_mame_maps` walks every word of
`800000-8001FF` against a table of literal inclusive bounds transcribed from
`cps1.cpp:579-591`, and then asserts that table **tiles** the block with no gap and
no overlap — otherwise a hole in the table would itself look like coverage.
`each_regions_first_and_last_word_is_mapped_and_its_neighbours_are_not` does the
same for the large regions, because an inclusive bound written exclusive silently
unmaps the last word of a region, and the CPS-1 uses it: the stack lives at the top
of main RAM.

**Unmapped reads return `0xFFFF`, and that value is load-bearing.** The 68000's
data bus floats high on an access no chip answers, and a board with pull-ups reads
it back as all ones. Zero would be both wrong and dangerous: `0x0000` decodes as a
legal `ori.b #imm,d0`, so a runaway PC in unmapped space would execute quietly for
thousands of instructions instead of taking an exception immediately. `0xFFFF` is
illegal, which is what the hardware reads *and* the failure that surfaces at once.

**Unmapped writes are discarded and reported as undecoded**, which is a distinct
outcome from "discarded and decoded". A ROM write is the latter: a real board
decodes `000000-3FFFFF`, so a write there is a guest bug or a deliberate discard,
not evidence our map is missing a chip. The two are counted on separate
counters — see [The trace](#the-trace-what-the-board-saw). Found by mutation:
flipping the unmapped write arm to `true` survived the whole suite, because every
test checked only that the write changed nothing, which a discarded-but-claimed-
handled write also satisfies.

**Byte accesses assert a strobe; they are not read-modify-writes of the containing
word.** The 68000 has no byte-wide bus — it drives UDS and LDS to select halves of
a 16-bit access, and MAME passes the same information as `mem_mask`, branching on
it with `ACCESSING_BITS_0_7` / `ACCESSING_BITS_8_15` (`cps1.cpp:300-313`). A board
that models a byte write as read-modify-write gets write-only ports wrong: the read
half returns `0xFFFF` from a port that does not read, and the "preserved"
neighbouring byte is latched as `0xFF`. `Lanes` carries which strobe is asserted
down to the arm that owns the storage, and the read-modify-write happens only
there. Two ports depend on this: `cps1_coinctrl_w` ignores a low-half access
entirely, and `cps1_soundlatch_w` takes the low byte when the low lane is asserted
and the high byte otherwise.

**No guest address panics.** Every index is produced by masking or a nonzero
remainder, never by a bounds-checked slice index on guest arithmetic. A
mis-emulated jump produces wild addresses as a matter of course, and an emulator
that panics on one has turned a guest fault into a host crash.
`no_address_in_the_whole_24_bit_space_panics` sweeps all of it.

---

## CPS-A and CPS-B

Two 0x40-byte custom register files, `800100-80013F` and `800140-80017F`, 32 words
each. Sub-project B stores them and interprets nothing; C owns every meaning.

Both handlers begin with MAME's `COMBINE_DATA` (`cps1_v.cpp:2115`, `:2183`), so a
byte write merges into the register and leaves the other half alone.

### Addresses are byte offsets; array indices are word indices

**Write the `/2` at every boundary, and never carry a converted value in a field.**

MAME's per-game table gives byte offsets from `0x800140`, and MAME's *own layout
constants* in `cps1.h:176-193` are already divided by two because its array is
`uint16_t` — `CPS1_SCROLL1_SCROLLX = 0x0c / 2`. The two conventions sit in adjacent
files. Mixing them shifts the register file by exactly one entry, and every value
in it looks plausible in the wrong slot: nothing crashes, the screen is subtly
wrong, and there is no error to chase.

`BoardConfig`'s `cpsb_addr` and `in2_addr` are therefore documented as byte offsets
and stay byte offsets; `the_offsets_are_even_byte_offsets_inside_the_cps_b_window`
asserts both are even and both land inside the 0x40-byte window.

### CPS-B is not RAM, and the difference is boot-critical

CPS-B answers some reads with values the board wires in rather than what was
written. MAME keeps these per game (`cps1_v.cpp:1766-1900`); `BoardConfig` is the
same table with one row.

For SF2 — `cps1_v.cpp:1838`, `{"sf2", CPS_B_11, mapper_STF29, 0x36}`, with
`CPS_B_11` expanding at `cps1_v.cpp:491`:

```
cpsb_addr  = 0x32   ->  0x800172, the CPS-B ID register
cpsb_value = 0x0401 ->  what it reads back as, whatever was written
in2_addr   = 0x36   ->  0x800176, SF2's extra input port
```

**`cpsb_addr` is boot-critical.** The game reads `0x800172` during its self-test
and expects `0x0401`. A board that treats CPS-B as plain RAM returns the last value
written and the game stops at a self-test failure — the boot does not limp, it
halts. `the_cpsb_id_check_takes_the_pass_branch` and
`the_cpsb_id_check_fails_when_the_board_answers_wrongly` are the pair that pins it,
and `BoardConfig::plain()` exists so a test can show the behaviour comes from the
config and not from the address: with `plain()`, `0x800172` is plain RAM. Without
such a case a hardcoded `0x32` would pass every `sf2()` test.

**`in2_addr` is boot-critical for a subtler reason.** SF2's three kick buttons per
player are read **through the CPS-B space at `0x800176`** (`cps1_v.cpp:2155-2156`),
not through the `0x800000` port block. It is an 8-bit port read into a 16-bit
space, so the byte arrives with `0x00` above it. A board that misses this returns
whatever CPS-B holds, and three of six buttons per player never respond — a fault
that looks like an input-mapping problem and is a memory-map problem.

One MAME trap worth recording: `cps1_dsw_r` (`cps1.cpp:257-272`) and
`cps1_hack_dsw_r` (`:274`) are adjacent in the file and differ in one token —
`| 0xff` versus `| in`. SF2 gets `main_map` from `cps1_10MHz` (`cps1.cpp:3909`,
with `GAME(1991, sf2, …)` at `:15024`), and `main_map` wires `cps1_dsw_r`. The DIP
byte lands in the **high** half of the word with `0xFF` below it.

The ID register write is not protected: the write lands even at `cpsb_addr`,
because `COMBINE_DATA` runs before `cps1_cps_b_r` ever intercepts the read. The
register is readable-as-wired, not write-protected.

---

## The interrupt acknowledge

The one place where sub-project B's requirements exceed what the verified 68000
core provides. This section records the mechanism, the three options, the decision,
and the exact bound on its imprecision — because a workaround discovered
mid-implementation and left undocumented is how an emulator acquires a mystery.

### The hardware mechanism

CPS-1 asserts **IPL1** when the beam reaches line 240 — `cps1.cpp:394-396`,
`if (scanline == 240) set_input_line(M68K_IRQ_IPL1, ASSERT_LINE)`, which is
`CPS_VBSTART`, the line the beam leaves the visible area on.

The board wires the IPL pins **individually**: `set_interrupt_mixer(false)` at
`cps1.cpp:3913`. So IPL1 is interrupt **level 2**, not an encoded priority, and its
autovector is 24 + 2 = **26**, whose longword lives at 26 × 4 = **0x68**.

The line is cleared **by the 68000's own autovector fetch**. The CPU drives FC=7
with an address in `0xFFFFF2-0xFFFFFF`, and the board decodes that to drop both
IPL1 and IPL2 (`irqack_r`, `cps1.cpp:407-422`, wired through `cpu_space_map` at
`:419-422`).

**`m68k::Bus` carries no function code.** Its four methods are `read8`, `read16`,
`write8`, `write16`, each taking an address and nothing else. So an autovector
fetch of vector 26 and a `move.l $68,d0` are the same two `read16` calls, and the
board cannot see an acknowledge cycle directly.

### The three options

1. **Widen `Bus` with a function code.** The most faithful, and it closes the blind
   spot flagged at the end of sub-project A. But it is a **breaking change to a
   trait that 317,500 verified vector cases are wired through**, for one bit that
   one board needs.
2. **Deassert from the machine, one scanline after assertion.** Simple, and wrong
   in a way that *hides*: if the handler is slower than a line, the assertion is
   already gone when it returns and the next one is missed; if it is faster, the
   same interrupt is taken twice. The error is unbounded and silent.
3. **Deassert on the vector fetch, detected in the board's read path.** The board
   knows it just asserted IPL1, so a read of the vector-26 longword at `0x68` or
   `0x6A` while that assertion is outstanding is, on this board, the acknowledge.

**Decision: option 3, with option 1 recorded as the correct fix, deferred to
whichever sub-project first needs FC for a chip select.**

### The exact bound on option 3's imprecision

Option 3 is exact whenever the vector table is in ROM, which it is on CPS-1. The
inference fails only if a game reads its own vector-26 longword **as data** while a
vblank assertion is outstanding. No CPS-1 game does. And if one did, the *read
would return the same value either way* — only the deassertion would be early. That
is the whole error: one bounded, stated case, versus option 2's unbounded timing
error on every frame.

The mask is `& !3` rather than `== 0x68` because the vector is a longword and a
68000 with a 16-bit bus fetches it as two `read16` calls, `0x68` then `0x6A`;
either half is the same acknowledge cycle. ⚠️ That mask is **arithmetically dead
today**: `m68k`'s `exception::take` reads the high half first
(`exception.rs:371-372`), so `0x68` always arrives before `0x6A`. Mutation
confirmed no test in the crate can kill dropping it, and none was contorted to try.
It stays because it encodes the hardware fact — the acknowledge is the *longword*
fetch — and because the equivalence rests on one core's fetch order: a core reading
the low half first, or a future `read32` fast path, would make it load-bearing with
no test signalling that it had become so.

### The blind spot this leaves, with its measured shape

`Transaction.fc` is the vector suite's **one parsed-and-never-compared field**, and
the reason is structural rather than an oversight: `Bus` takes an address and
nothing else, so the core never states a function code and the harness has no value
to compare against.

It carries real information. Measured over the corpus's **1,450,409 non-idle
transactions**, exactly **four distinct values** appear:

```
fc = 1  user-data          156,086
fc = 2  user-program       179,995
fc = 5  supervisor-data    768,057
fc = 6  supervisor-program 346,271
        (idle transactions carry 0)
```

Concretely: the suite does **not** confirm the core drives the right address space.
A vector fetch issued as user-data rather than supervisor-data is invisible to every
one of the 317,500 cases.

A second consequence, in the core's cycle accounting. The manual's interrupt row is
`44(5/3)`, whose eighth access is the IACK cycle. This core does not emit one, and
`INTERRUPT_CYCLES` is spelled to say so:

```
4 × 7 accesses + 16 idle = 44     <- what check_interrupts emits
4 × 8 accesses + 12 idle = 44     <- the manual's, with the IACK modelled
```

The IACK's four cycles are **spent as idle rather than on the bus**. The total is
the manual's either way; the split is not.

**The condition under which option 1 must be done:** the first time any chip select
in this project is derived from FC, or the first time a CPS-1 variant's board
decodes the CPU space differently. At that point widening `Bus` is not optional,
and the four idle cycles above move back onto the bus with it. Until then the
deferral is a stated bound, not a hope.

### The trap this must not fall into

`vblank_pending` is an internal flag, and **a test that asserts the flag proves
nothing** — it reads the same field the code just wrote, and passes a half-done fix.
This project has produced that exact defect before; see `68000-notes.md` on
self-consistent assertions.

So the tests assert the observable artifact: that a guest handler's own increment
lands exactly once per frame, counted by the handler's instruction stream. Ten
frames rather than one, because the count must hold in *both* directions — an
unacknowledged line re-enters (the mask blocks it only during the handler, not
after the `rte`), and a line acknowledged before the CPU sampled it counts zero. A
frame is 167,680 cycles and the handler costs about 90, so an unacknowledged line
re-enters on the order of a thousand times per frame: the wrong answer is not 11,
it is four figures.

There is no public deassertion API on `Board` at all. The acknowledge is inferred
from the read, and nothing else can clear the line.

---

## The trace: what the board saw

Sub-project B renders nothing, so the trace is its entire observable surface — and
it is a better instrument for the question B actually answers than a black window
would be. A black window is indistinguishable from a boot that hangs on the first
instruction; a count of vblanks, acknowledges, and video-register writes tells you
which. "Does SF2 boot?" becomes checkable: after N frames, is `vblanks == N`, did
`cps_a_writes` happen, did the game ask the Z80 for the attract music, and are the
sampled PCs inside populated ROM rather than looping in an exception handler?

`acks` short of `vblanks` is the headline diagnostic: the game is not servicing the
interrupt, because either the mask never drops or the handler never returns.

Counters live on `Board` and not on `Cps1` because **the board is what decodes** —
only that file knows whether `0x810000` is a chip or a hole. Frames are counted on
the scanline **wrap** rather than in `run_frame`, so a caller driving scanlines by
hand (the debugger, and every test in the crate) counts the same frames a
`run_frame` caller does.

`Cps1::reset` deliberately leaves the trace alone. It is an instrument attached to
the machine, not part of it: a driver resetting mid-run wants to keep what it has
already observed, and a caller that raised `pc_sample_cap` before resetting would
otherwise find it silently back at zero.

Two bounds, both driven by guest behaviour rather than by taste:

- **`UnmappedLog` itemises at most 1024 distinct addresses** and reports what it
  dropped. A wild PC scanning memory produces millions of distinct unmapped
  addresses; an unbounded log is then a memory leak driven by the guest, and the
  sorted-`Vec` insert makes it quadratic — 300,000 distinct addresses is 300,000
  insertions each memmoving up to 3.6 MB. `board.rs`'s own 24-bit sweep test visits
  **313,239** distinct unmapped addresses; that is a measurement, taken by running
  the sweep's exact access pattern against a `HashSet`, not an estimate — the figure
  standing here before was 190,000 and was wrong by a third.
  Accesses past the cap still count in `total` and are reported by
  `dropped`, so the truncation is visible rather than silent: a report printing
  `total` beside an eight-row list reads as complete when it is a sample of 1024.
- **PC sampling is opt-in**, `pc_sample_cap` defaulting to 0. A 60-frame run is
  15,720 scanlines and a frontend running for an hour is 56 million.

The `sfemu` binary prints this report. It is the one crate that depends on both
`machine` and `romset`; **`machine` must never depend on `romset`**, which would
drag in `miniz_oxide` and forfeit the `no_std`/WASM posture. That is also why the
real-ROM boot test lives in `sfemu` rather than in `machine`.

---

## What the suite cannot see, restated for B

Sub-project A had 317,500 external vector cases as its oracle, over 127 groups, and
that corpus is what makes every claim in `68000-notes.md` checkable.

**There is no vector suite for a Capcom board.** No public corpus of expected CPS-1
bus transactions exists, and none is going to. Sub-project B's oracle is therefore
a set of hand-assembled 68000 programs in `crates/machine/tests/programs.rs`, plus
MAME's source read line by line — and this section exists to state plainly that
this is a **weaker oracle**, and exactly where it is weak.

What the programs do guarantee is that no expectation is self-consistent with the
code under test: each program's expected outcome is a number written by hand from
the 68000 manual and the memory map, every encoding was verified against
`m68k::disasm` with the rendering quoted beside the word, and each is
mutation-checked.

### What the programs do pin

| Claim | Test |
|---|---|
| Reset takes SSP and PC from vectors 0 and 1, and the SSP points at writable RAM | `the_stack_pointer_points_at_writable_ram` |
| Vblank fires once per frame and the handler runs once per frame | `vblank_increments_a_counter_once_per_frame` |
| The acknowledge is what makes the count 1 rather than four figures, over ten frames | `the_handler_runs_once_per_frame_over_ten_frames_neither_dropped_nor_re_entered` |
| Vblank fires on line 240 and on no earlier line | `the_vblank_interrupt_fires_on_line_240_and_not_before` |
| A masked interrupt stays pending across scanlines and is cleared by the fetch, not by any ROM read | `a_masked_interrupt_stays_pending_across_scanlines_and_is_cleared_by_the_fetch` |
| `STOP` parks the CPU and the vblank wakes it — and nothing past the `STOP` runs before line 240 | `a_stopped_cpu_is_woken_by_the_vblank_interrupt`, `a_stopped_cpu_executes_nothing_until_the_interrupt_arrives` |
| The CPS-B ID check takes the pass branch, and fails when the board answers wrongly | `the_cpsb_id_check_takes_the_pass_branch`, `the_cpsb_id_check_fails_when_the_board_answers_wrongly` |
| gfxram word writes read back as big-endian bytes | `gfxram_word_writes_are_readable_as_big_endian_bytes` |
| Every trace counter counts what the program did and nothing it did not | `the_trace_counts_what_the_program_actually_did` |
| A ROM write is counted separately from an unmapped one | `a_rom_write_is_counted_separately_from_an_unmapped_one` |
| An idle board reads all ones through the DIP port | `an_unpressed_board_reads_all_ones_through_the_dip_port` |

`STOP`'s wake path is worth singling out: **zero vector cases cover it.** `STOP`'s
access shape is empty and no vector case runs a second step, so these programs are
the only evidence in the project that the resume works at all.

### What they do not pin

- **Nothing about the video output.** CPS-A is stored and never interpreted; not one
  pixel is produced or checked. Every claim about what a register *means* is
  sub-project C's to make, and C has no oracle here either.
- **Nothing about the Z80, the YM2151, or the OKI.** The sound latches record what
  the 68000 wrote and no chip reads them. `sound_latch_writes` answers "has the
  68000 started talking to the Z80 at all?" and nothing finer.
- **Bus timing at the chip level.** The scheduler grants cycles per scanline; there
  is no model of DMA contention, of the video chip stealing cycles, or of
  wait states. If a real board's 68000 gets fewer than 640 cycles on some lines, we
  do not know it and no test here would notice.
- **The CPS-B mapper.** `mapper_STF29` — the third field of SF2's `cps1_v.cpp:1838`
  table row — is **not transcribed at all**: it appears in this repository only
  inside `config.rs`'s doc comment for that citation, as part of the quoted row.
  `BoardConfig` carries the row's fourth field (`0x36`, `in2_addr`) and the two
  values `CPS_B_11` expands to at `cps1_v.cpp:491`; the mapper is the one field it
  drops, because the graphics bank mapping it describes is sub-project C's to model.
- **Whether the map is *complete*.** Every range MAME gives a handler is
  implemented, but a chip MAME reaches through a mechanism we have not read is
  invisible. This is precisely what the unmapped log exists to surface at runtime:
  a boot stalling with 40,000 unmapped writes to one address has named the chip.
- **Function codes**, per the section above.
- **Real-ROM behaviour**, except through the one opt-in test. There is no ROM in
  this repository and no command we may legally print in a failure message, which
  is why that single test is `#[ignore]`d — the only exemption from this project's
  fail-loudly rule for missing test data.

### Two findings mutation testing produced

Both are recorded because they are the same shape: **a test whose input cannot
exercise the property it claims to check.** Neither was found by reading the code.

**Task 2 — the EOCD backward scan was not discriminated.** A zip's end-of-central-
directory record is last in the file, but a trailing archive comment can push it
away from the end, so the parser scans backwards for the signature. Every test built
its archive with `comment_len` = 0, which places the EOCD exactly 22 bytes from the
end — so a parser that looked *only* at that fixed offset passed every test in the
file. The scan was correct; the evidence that it was correct did not exist. Fixed by
`the_eocd_is_found_behind_a_trailing_archive_comment`, which builds the archive with
`comment_len` in `[0, 1, 8, 300]`.

**Task 8 — a per-frame vblank count cannot see which line vblank falls on.** A
vblank wrongly asserted on line 0 also fires exactly once per frame, so mutating
`line == vblank_line` to `line == 0` survived every test that counted by the frame.
Fixed by `the_vblank_interrupt_fires_on_line_240_and_not_before`, which records the
first `run_scanline` call by which the handler can have run and asserts the literal
**241** — hand-derived from the counter's semantics (`run_scanline` runs the line
held in `self.line` and then advances it, and reset leaves `line == 0`), not read
back from `m.line`.

A related pair from the same task, both needing a program that runs from ROM for
thousands of cycles *while the line is asserted*: `vblank_pending = false` on every
ROM read, and `if self.line == 240` written in place of `self.timing.vblank_line`.
With the mask at 0 the acknowledge fetch happens in the same instruction that
recognised the interrupt, so clearing on any ROM read is indistinguishable from
clearing on the vector fetch — both land within one step. And 240 *is*
`vblank_line` for `cps1_10mhz()`, so no test using that `Timing` can tell the two
apart. The test that kills both supplies a 20-line frame with `vblank_line: 10`.

**The general rule this yields:** vary the configuration the tests share. A
constant that matches the shared fixture survives every mutant, and a rule scored
only on inputs that cannot disagree reads as exact while being unverified.

### Method

Manual mutation testing, no dependency added — the interesting mutants are specific
constants and arms, and choosing them by hand is the point. One string replacement
per mutant; assert the pattern occurs **exactly once** before replacing, because a
pattern found zero or many times is a **no-op, not a killed mutant**, and a
transcript that counts it as killed looks like success. Revert by restoring a file
copy and confirm with `diff -q` before the next mutant — never `git checkout`, which
also destroys uncommitted work elsewhere in that file. Record *which* tests died,
not just how many.

Three outcomes, not two: killed; a real test gap (fix the test); and an
**equivalent mutant** provable unkillable by arithmetic — document it at the line
with what would make it load-bearing, and do not contort a test to reach it. Three
of those are on record in this crate: the `& !3` acknowledge mask above,
`gfx_index`'s `wrapping_sub(GFXRAM_BASE)` (0x900000 >> 1 is exactly 48 × 0x18000,
so the remainder is the same with or without it), and `UnmappedLog::worst`'s
address tie-break (`entries` is already ascending and `sort_by` is stable).

---

## The ROM interleave

A CPS-1 program ROM is not one file. SF2's `maincpu` is **four pairs of 128 KB
files, byte-interleaved** into 1 MB at `000000-0FFFFF`; `100000-3FFFFF` is
unpopulated and reads as zero. Transcribed from `cps1.cpp:7101-7133`.

| MAME macro | `LoadKind` | Placement |
|---|---|---|
| `ROM_LOAD` | `Byte` | byte `i` → `offset + i` |
| `ROM_LOAD16_BYTE` | `Word16Byte` | byte `i` → `offset + 2i` |
| `ROM_LOAD64_WORD` | `Word64Word` | source word `i` → `offset + 8i` |
| `ROM_CONTINUE` | `Continue { split, cont_at }` | first `split` bytes at `offset`, remainder at `cont_at` |

The four `maincpu` pair bases are `0x00000`, `0x40000`, `0x80000`, `0xC0000`, and
within each pair the **even** offset supplies the **high** byte of the big-endian
word and the odd offset the low byte.

### The byte-swap failure mode

**Getting a pair's parity backwards byte-swaps every instruction word in that
quarter of the program.**

The one mercy in this failure mode is that it is loud. A byte-swapped 68000 program
takes an illegal-instruction or address-error exception within a few steps rather
than running visibly wrong for a while — the opcode space is dense enough that a
swapped word is usually either illegal or an instruction with a wildly wrong
operand, and word operands at odd addresses are address errors. Compare the
alternative: a slip that gives two files of a pair the *same* parity byte-swaps a
quarter of the program while leaving three quarters correct, and the symptom is a
68000 executing garbage thousands of instructions after the boot code that was
fine.

Three things guard it, all with **no ROM present** — the table is metadata, so its
internal consistency is checkable on its own:

- `maincpu_pairs_alternate_even_and_odd_offsets` — `pair[0].offset` even,
  `pair[1].offset` odd, and `pair[0].offset + 1 == pair[1].offset`.
- `maincpu_pair_bases_are_the_four_128k_word_boundaries` and
  `maincpu_populates_exactly_the_first_megabyte` — 8 × 0x20000 interleaved is
  exactly 1 MB, so a wrong offset shows up as a wrong top.
- `word16_byte_interleaves_even_file_into_the_high_byte` — the placement arithmetic
  against a synthetic pattern.

⚠️ **That last test only works because its input discriminates.** A zero-filled or
constant source makes `Byte` and `Word16Byte` produce **identical** output, so the
test would pass with the interleave completely wrong. The pattern is
`tag | (i & 0x0F)` with distinct tags per file, making the two sources
distinguishable in every byte. This is the same defect shape as the two mutation
findings above, and it is the reason `pat()` exists rather than a `vec![0; n]`.

Two more transcription traps, both recorded because they read like errors and are
not:

- **`RomEntry::len` is the whole source file, not the span it occupies.** For
  `Word16Byte` the span is twice the length; for `Word64Word`, four times. `end_of`
  computes the span per kind, and the final byte of a `Word16Byte` entry is at
  `offset + 2(len-1)`, so the exclusive end is one past *that* — not
  `offset + 2·len`.
- **`sf2_9.12a`'s `len` is `0x10000`, where MAME's `ROM_LOAD` says `0x8000`.**
  MAME's length field there is only the first half; the `ROM_CONTINUE` carries the
  rest to `0x10000`. Our `len` is the file, per the field's definition, and
  `AUDIO_SPLIT` describes the split.

The gfx region is twelve 512 KB files in three groups of four, strided into a
64-bit layout at word offsets 0, 2, 4, 6 within each 2 MB group. B loads it and
decodes nothing.

Two table-wide checks catch the transcription errors that are easiest to make and
hardest to see: `no_two_entries_share_a_crc` (a copy-pasted CRC among twelve
similar gfx lines) and `no_two_entries_share_a_name`. The CRC test also asserts the
count — 23 distinct files, 8 + 12 + 1 + 2.

CRCs are verified in **both directions**: `loads_a_directory_and_interleaves_
correctly` accepts files whose CRCs match, and
`a_flipped_bit_fails_with_the_file_name_and_both_crcs` flips one bit and asserts a
`Crc` error naming the file and carrying both values. A checksum check tested only
on good input is not a check. Every `RomError` variant names the file, because the
whole value of checking is a message that says *which* of eight interleaved files
is bad.

One mutation finding here, recorded because the mutant is the change someone will
actually propose: adding `entry.crc32 != 0 &&` to the check survived the whole
suite, since no test entry had a zero expected CRC. "Skip the check when we don't
know the value yet" is the most natural edit to that function and the doc comment
already forbids it — a declaration no test enforces is exactly the defect this
project keeps producing. `a_spec_crc_of_zero_is_still_checked` is that test, and
zero is a legitimate CRC-32 (of empty input), so the exemption would be wrong even
as a convention. A length mismatch is reported as a length error rather than as a
CRC error (`a_short_file_is_a_length_error_not_a_crc_error`): a truncated member must
not reach the interleave stage at all.

The tables carry `#[rustfmt::skip]` deliberately: the only way to verify a
transcription is to read it beside the MAME source, and `rustfmt`'s one-field-per-
line expansion turns 23 entries into 138 lines nobody will diff against anything.

---

## How to check that a claim in this file is still checked

Same two procedures as `68000-notes.md`, with one addition specific to B.

For a constant: mutate it by hand, run `cargo test --workspace --release`, record
which tests died, revert, confirm the clean tree **before the next mutant**. A
mutant that kills nothing means the constant is a comment, not a check.

The addition, because B has no external oracle: for any claim sourced from MAME
rather than from a test, the check is **re-reading the cited line**. Every such
claim on this page names its file and line for that reason. A claim citing
`cps1.cpp:394-396` is checkable in ten seconds; a claim saying "MAME does X" is
not, and is the shape this file avoids.
