# Design: Zilog Z80 core (sfemu sub-project D1)

> **Status:** approved 2026-08-08. Supersedes the roadmap's single "D" row, which
> is now D1/D2/D3.

**One sentence:** a cycle-counted Z80 in `crates/z80`, verified instruction by
instruction against the SingleStepTests/z80 vectors, with no audio chip and no
sound in it.

## Context

Six sub-projects are complete: the 68000 core (A), the bus and ROM loader (B),
CPS-1 video (C), and the frontend's three surfaces (E1 window and save states, E2
debugger, E3 graphics viewers). SF2 runs and is visible. **There is no sound.**

The 68000 side of sound is already finished and traced. `Board::write_lanes`
decodes `0x800180` and `0x800188` into `sound_latch: [u8; 2]`, counts
`trace.sound_latch_writes`, and reproduces MAME's lane quirk — a high-byte-only
write to the second latch is decoded and discarded (`cps1.cpp:308-312`). The
ROM loader already populates both audio regions with CRC verification: `audiocpu`
(0x18000 bytes, one 64 KB file split across a 64 KB gap by `ROM_CONTINUE`) and
`oki` (0x40000 bytes, two concatenated 128 KB samples). Both carry the comment
"Loaded for sub-project D; nothing reads it in B."

So D1 does not need to invent a seam. The latch exists, the ROM is in memory, and
the Z80's program is sitting at `audiocpu[0]` unexecuted.

### Why D was split

The roadmap had one row for "Z80 and audio: YM2151, OKI MSM6295 ADPCM". That is
three unrelated subsystems, and E set the precedent for splitting on exactly this
ground: its three surfaces were independent, so it became E1/E2/E3.

| # | Sub-project | Deliverable | Sound? |
|---|-------------|-------------|--------|
| **D1** | Z80 core | A CPU verified against 1,604 vector files | no |
| D2 | YM2151 + sound-board wiring | The latch reaches a chip; registers move | no |
| D3 | OKI MSM6295 + host audio | Samples, mixing, and the host's speaker | **yes** |

The decisive number: sub-project A is 16,462 lines for one CPU against 127 vector
files. The Z80 suite is **1,604 files**. A single spec covering a CPU core *and* an
FM synthesizer *and* host audio output would produce a ~4,000-line plan and ask one
reviewer to gate three unrelated subsystems in one pass — which A's own spec warned
against when it decomposed the project in the first place.

D3 is the one that ends "there is no sound." D1 and D2 are both silent, and that is
not a defect in them.

**This document specifies sub-project D1 only.**

## What this is not

- **Not audio.** No YM2151, no OKI, no samples, no mixing, no host audio device.
  Nothing in D1 makes a sound, and no test in D1 asserts anything about one.
- **Not wired to the board.** `Cps1` does not gain a Z80 in D1. The core is a
  standalone crate with its own `Bus`, exactly as `m68k` was in A. D2 wires it.
- **Not a Game Boy CPU.** The Sharp LR35902 is a different chip; every `IX`/`IY`
  instruction, the `ED` page, and the shadow registers are real Z80 and all are
  covered by the vectors.
- **Not undocumented-hardware archaeology.** Where the suite specifies behaviour,
  the suite wins, including for the undocumented `DD CB`/`FD CB` pages, `SLL`, and
  the `Q` register. Where it does not, D1 does not guess.

## Architecture

A new crate, `crates/z80`, dependency-free and `no_std`-compatible in the same
sense `m68k` is: no threads, no wall-clock, no host I/O. This is not a preference —
it is the WASM/netplay constraint that binds A–D.

```
crates/z80/
  src/lib.rs        crate docs, re-exports
  src/bus.rs        the Bus trait: memory + I/O ports
  src/cpu.rs        registers, step(), interrupt acceptance
  src/decode.rs     the five pages, dispatch
  src/flags.rs      the flag rules, including F3/F5 and Q
  src/ops/mod.rs
  src/ops/load.rs   8- and 16-bit loads, EX, block moves
  src/ops/alu.rs    ADD/ADC/SUB/SBC/AND/OR/XOR/CP, DAA
  src/ops/bits.rs   the CB page: rotates, shifts, BIT/SET/RES, SLL
  src/ops/flow.rs   JP/JR/CALL/RET/DJNZ, RST, conditions
  src/ops/io.rs     IN/OUT and the ED block-I/O forms
  src/disasm.rs     one-instruction disassembly (for D2's debugger panel)
```

### The `Bus` trait

The Z80 has something the 68000 does not: a **separate 16-bit I/O address space**.
That is not an implementation detail — the sound board uses it, and the vectors
verify it as a distinct pin state (`r--i`/`-w-i` versus `r-m-`/`-wm-`). So the
trait has four methods, not two:

```rust
pub trait Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, val: u8);
    fn port_in(&mut self, port: u16) -> u8;
    fn port_out(&mut self, port: u16, val: u8);
}
```

`port_in`/`port_out` take the **full 16-bit** port address, because the suite's
`ports` array records 16 bits and because `IN A,(n)` puts `A` on the high half —
a core that masked to 8 bits would pass most tests and fail that one.

There is no `read16`: the Z80's data bus is 8 bits, and every 16-bit access is two
byte accesses in a defined order. Composing them in the core is what makes the
T-state counts fall out instead of needing a table, which is the same reasoning
`m68k::Bus` records for its missing `read32`.

### Cycle counting

`step()` returns the T-states consumed. The Z80, unlike the 68000, has no
"every access is four cycles" law — its timing is genuinely per-instruction, with
memory accesses of 3 T-states, opcode fetches of 4 (M1), and instruction-specific
internal cycles. **So D1 does have a cycle cost per instruction, and A's central
result does not transfer.**

This is worth stating plainly because it is the one place D1 is architecturally
unlike A: A eliminated its cycle table by discovering the timing law. D1 cannot,
and pretending otherwise would mean a table of invented numbers. Instead the costs
come from the vectors — every file's `cycles` array is the authority, and the
tests compare the returned T-state count against `cycles.len()`.

## Verification

**The vector suite is ground truth.** This project's rule, and D1 is where it is
most load-bearing: a Z80 has 1,604 opcode forms and no amount of hand-written
tests reaches the corners.

### Primary: SingleStepTests/z80

**Verified 2026-08-08 by direct inspection of the repository and its data** — this
section records what the suite is, not what it was assumed to be.

`https://github.com/SingleStepTests/z80`, directory `v1/`, **MIT licensed**.
Generated by translating Ares' Z80 core and then fixing bugs found in it.

- **1,604 files, 1,000 cases each — 1,604,000 cases.** 1.37 GB as JSON.
- Naming, by page: 252 plain (`00.json`…, absent: `cb`, `dd`, `ed`, `fd`, which
  are prefixes rather than instructions), 256 `cb NN`, 252 `dd NN`, 252 `fd NN`,
  80 `ed NN` (only the populated `40`–`bf` range), 256 `dd cb __ NN`, and 256
  `fd cb __ NN`. **Filenames contain spaces**, and the `dd cb` forms contain a
  literal `__` standing for the displacement byte.

Per case: `name`, `initial`, `final`, `cycles`, and — only for I/O instructions —
`ports`.

A state block has **26 fields**, identical in `initial` and `final` (verified: the
key sets are equal). Registers `pc sp a b c d e f h l i r ix iy wz af_ bc_ de_
hl_`, plus the mode and internal flags `ei im p q iff1 iff2`, plus `ram` as
`[addr, value]` pairs.

Three of those fields are why this suite is stricter than a register comparison:

- **`wz`** is the Z80's internal address latch. It is not architecturally visible,
  but it feeds the address pins during some cycles, so the suite checks it.
- **`q`** records whether the last instruction modified the flags, which is what
  `SCF`/`CCF` need to compute F3/F5 correctly — the single most commonly wrong
  detail in Z80 cores.
- **`p`** records whether `LD A,I` or `LD A,R` was the last instruction, for the
  same family of reasons.

`cycles` is a list of bus states **sampled between T-states**: `[addr, data,
pins]`. Confirmed pin patterns across the pages: `----` (internal), `r-m-` (read
+ MREQ), `-wm-` (write + MREQ), `r--i` (IN), `-w-i` (OUT). T-state counts run from
4 (`NOP`) to 23 (`DD CB __ 06`). `addr` was non-null in every one of the ~86,000
samples inspected, which is the documented consequence of the
last-address-during-wait generation option.

**The pins do not tell you whether `data` is valid, and assuming they do is the
trap in this format.** Measured across six files spanning every pin pattern:

| pins | data null | data set |
|------|-----------|----------|
| `----` | 48,000 | 17,000 |
| `r-m-` | 15,000 | 0 |
| `-wm-` | 0 | 3,000 |
| `r--i` | 2,000 | 0 |
| `-w-i` | 0 | 1,000 |

A read's *request* T-state carries no data — the byte appears on a later `----`
sample, once the bus has settled. So `r-m-` is **always** null and `----` is data-
bearing about a quarter of the time. This is why the binary record has an explicit
`data_valid` flag rather than deriving validity from the pin bits: a comparison
keyed on "is this a read cycle" would skip every byte the CPU actually returned,
and would pass against a core that fabricated all of them.

Generation options that are baked into the data and must be matched:
simplified memory-access T-states (MREQ/RD pulse one T-state, not two), refresh
values on the address pins during opcode fetch, and last-address (not `null`)
during wait states. **NMOS, not CMOS.**

### Disk, and the format the fetcher writes

The suite is 1.37 GB of JSON and this machine has 2.9 GiB free. Downloading it
whole is not prudent, and this workspace has no JSON parser in any shipping crate
(deliberately — `machine` must stay dependency-free for WASM).

So `testrunner` gains a fetch step that **streams**: for each of the 1,604 files,
download the JSON to a temp path, convert it to a compact little-endian binary,
delete the JSON, continue. Peak extra disk is one JSON file (~1 MB) plus the
growing output.

Measured shrink for the layout below is **5.8×**, from 15 files sampled across all
seven pages: 6.4× on the cheapest (`00`, `76` — 4 T-states) down to 5.5× on the
dearest (`dd cb __ 06` — 23 T-states), with `d3`/`db` (I/O, carrying a `ports`
record), `ed b0` (LDIR), `ed 40`, `cb 06`, `dd 21`, `fd 09`, `fd cb __ 7e`, `36`,
`c9` and `dd 34` in between. Mean 144 KB per file, so the output is **~236 MB** in
`testdata/z80/`.

The sample is representative rather than convenient: extrapolating its mean JSON
size to 1,604 files gives 1.38 GB against the 1.37 GB the git-tree API reports for
the directory, so it is not skewed toward small files.

The JSON reading happens in `testrunner`, which is a dev-only crate that already
carries `curl` shell-outs and never ships. A minimal reader for this one known
shape belongs there — not a serde dependency, and not in `z80`.

Binary layout, little-endian, mirroring the m68000 format's spirit (a magic, a
count, then fixed records) but defined here because the upstream Z80 data has no
binary form to copy:

```
file:   u32 magic 0x5630_385A             u32 num_cases
        (the four bytes 'Z','8','0','V' in file order; as a little-endian
         u32 that is 0x5630_385A, not 0x5A38_3056 — the reversal is the
         kind of thing that reads fine in a spec and fails at runtime)
case:   state initial, state final, u16 num_cycles, cycle[num_cycles],
        u8 num_ports, port[num_ports]
state:  u16 pc sp ix iy wz af_ bc_ de_ hl_       (9 x u16)
        u8  a b c d e f h l i r                  (10 x u8)
        u8  ei im p q iff1 iff2                  (6 x u8)
        u8  num_ram, then num_ram x { u16 addr, u8 val }
cycle:  u16 addr, u8 data, u8 flags
        flags bit0 data_valid, bit1 rd, bit2 wr, bit3 mreq, bit4 ioreq
port:   u16 addr, u8 val, u8 dir (0 = in, 1 = out)
```

`num_ram` is a `u8` and `num_cycles` a `u16`, with headroom rather than a fit: the
largest RAM block observed across the block-transfer and block-I/O files is **5**
entries and the longest instruction **23** T-states, against ceilings of 255 and
65,535. `num_ports` is a `u8` and no case has more than one port transaction.

**The converter asserts every one of those bounds and fails loudly, naming the file
and case.** The point is not that 5 will not exceed 255 — it plainly will not — but
that a silent `as u8` is how a format grows a truncation bug when someone
regenerates the suite with different options. A bound that is checked is a bound
that can be raised safely.

Every case's name is **dropped**, not stored: it is `"<PAGE> <OP> <index>"`, fully
recoverable from the filename and the case index, and storing 1.6 M strings to
re-derive them would be 30 MB of nothing.

### Missing data fails loudly

Per this project's standing rule, with no exemption for diagnostics: a test whose
vector file is absent **fails naming the file and the fetch command**. No
`#[ignore]`, no environment-variable escape hatch, no silent skip. `testdata/` is
gitignored and nothing from it is ever committed.

### One test per file

1,604 `#[test]`s, generated by a macro over the file list, so a failure names
`dd_cb_06` rather than reporting "Z80 broken". Each case checks, in this order:

1. the 26 final register fields, individually named in the assertion;
2. final RAM, every address the case declares;
3. the T-state total against `cycles.len()`;
4. the per-T-state bus trace: address, the four pins, and data **on exactly the
   samples whose `data_valid` flag is set** — never on the pin pattern, per the
   table above;
5. the `ports` transactions, in order, with direction.

Steps 4 and 5 are what make this a cycle-accurate verification rather than an
instruction-level one. They are also the reason `Bus` records accesses in the test
harness: a logging bus, as A used.

### Ours

- Hand-computed flag tests for the F3/F5 (`SCF`/`CCF` with `Q`), `DAA`, and
  `BIT n,(HL)` cases, because reading them off the vectors is exactly the
  "assertion that cannot fail" this branch keeps catching — a test that derives
  its expectation from the thing under test proves nothing. These are written from
  the Zilog documentation and from the flag algebra, by hand.
- A mutation set in `scripts/mutate.py` for the flag rules and the interrupt
  acceptance path, which the vectors alone do not reach.

### Definition of done for D1

1. `cargo test --workspace` and `--release` green.
2. All 1,604 vector groups pass, all 1,604,000 cases, with per-T-state bus
   comparison enabled — reported the way the 68000 suite is reported, as
   `groups: N/N green   cases: M/M`.
3. `cargo clippy --all-targets --all-features -- -D warnings` clean.
4. `cargo doc --no-deps --workspace` clean.
5. The 68000 suite still at **127/127, 317,500/317,500** — D1 touches no shared
   code, so any movement here is a regression to investigate, not a tolerance.
6. The mutation pass at 100% as-expected, every survivor a declared control or a
   proven equivalent.
7. `crates/z80` has no dependencies. `machine` is unchanged.

**Any upstream known-bad groups are recorded in the spec and excluded explicitly,
never absorbed by lowering the target.** As of writing, none are known; the
suite's own CI validates it. If one is found during implementation it is
documented with the evidence, not quietly added to a skip list.

## Error handling

The Z80 has no address errors and no bus errors — an unmapped read returns
whatever the bus returns, and there are no alignment rules. So D1 has far less
exceptional behaviour than A did. What it does have:

- **`HALT`** is a state, not a stop: the CPU keeps consuming T-states and keeps
  accepting interrupts. `76.json` verifies it.
- **Interrupt modes 0, 1, 2** and NMI, with `IFF1`/`IFF2` handling and the
  `EI`-delays-one-instruction rule (`ei` in the state block is exactly that
  pending flag). The suite covers the flags; acceptance timing is covered by our
  own tests and the mutation set, since the vectors are single-instruction.
- **Undefined opcodes are defined**: `DD`/`FD` prefixing something with no index
  form behaves as the unprefixed instruction after burning the prefix's cycles,
  and the suite has a file for every one of those 252 forms. There is no
  "illegal instruction" path to write.

## Risks

- **1.6 M cases at per-T-state granularity is slow.** A's suite is 317,500 cases
  and takes ~30 s in release. This is 5× the cases with a finer comparison. If it
  exceeds a few minutes, the fix is `--release` for the suite run (already the
  documented workflow) and per-file parallelism, not fewer cases.
- **Fetching 1,604 files over the network.** The fetcher must be resumable —
  skip any file whose `.bin` already exists, exactly as the m68000 fetcher does —
  and must not leave partial output (write to `.part`, rename on success).
- **The `Q` register is the classic Z80 trap.** It is not in most documentation
  and cores routinely omit it. It is a first-class field of the state block here,
  so it is caught immediately rather than surviving as a mystery.
- **Disk.** 236 MB on a disk with 2.9 GiB free, plus a 2.2 GB `target/`. Worth
  noting that `testdata/` is untracked and unrecoverable except by re-fetching.

## Sources

- SingleStepTests/z80 `README.MD` and `v1/` data, inspected 2026-08-08.
- Zilog *Z80 CPU User Manual* (UM008011) for the documented instruction set and
  flag rules.
- MAME `cps1.cpp` for the sound board's memory map — needed by D2, quoted in D1
  only where it explains why the `Bus` trait has ports.
- The existing `crates/m68k` for every structural decision D1 copies: the `Bus`
  trait's shape, the ops-module split, the per-opcode test granularity, and the
  rule that the core owns no memory.
