# sfemu

A Street Fighter arcade emulator, built from the hardware up. This repository
currently contains sub-project A: a cycle-accurate **Motorola 68000 CPU core**
and the test harness that verifies it.

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
```

The gate is **both** profiles. `--release` runs 210 `m68k` unit tests and 128
harness tests — one per suite group, plus one that fails if a file appears in
`testdata/` without a corresponding registered group, so adding a vector file
cannot silently go unrun.

⚠️ **The debug run is not redundant, and leaving it out hid a live defect.**
`[profile.release]` does not enable debug assertions, so under a release-only
gate the core's 12 `debug_assert!`s were never evaluated — including the one in
`ops::alu`'s `run_tail` that a whole task chose *over* deleting the field it
checks. Measured: inverting that assertion to `debug_assert!(!plan.writes, …)`,
which must fire on every write-back plan, leaves the release run entirely green
across all 11 binaries and all 128 groups, while failing 21 of 210 in debug. The
debug run costs 6 s, because `[profile.test] opt-level = 2` compiles the
dependencies optimised anyway; the suite itself takes 0.08 s of it.

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
crates/testrunner/   dev-only harness for the external vector suite.
docs/hardware/       what the vectors proved about the hardware, with evidence.
docs/superpowers/    design specs and implementation plans.
testdata/            gitignored; fetched vectors.
```

`m68k` knows nothing about Capcom hardware, which is what makes it testable
against third-party vectors and WASM-safe by construction. All state lives in
`M68k`, which derives `Clone` and `PartialEq`; every memory access goes through
the `Bus` trait. There is no wall-clock access, no randomness, and no interior
mutability — the properties that make save states, WASM, and rollback netplay
cheap later rather than a rewrite.

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
| B | Bus/timing framework, MAME ROM-set loader, minimal window | next; first execution of real board code |
| C | CPS-1 video: tilemaps, sprites, palettes, CPS-A/B registers, scanline renderer | the largest piece, and where SF2 becomes visible |
| D | Z80 and audio: YM2151, OKI MSM6295 ADPCM | deferrable; CPS-1 sound is a fire-and-forget latch |
| E | Frontend, debugger, save states | step, breakpoints, VRAM and tile viewers |
| F | Street Fighter 1 driver | a second board against a proven core |

WASM and netplay are not stages. They are constraints on A–D: no threads, no
wall-clock access, no host I/O in the core, a frame-stepped API, and complete
serialization. Honouring that from the start makes both nearly free.

## License

Not yet chosen. The `m68k` core contains no third-party code.
