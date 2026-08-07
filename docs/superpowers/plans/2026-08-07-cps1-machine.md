# CPS-1 Machine Implementation Plan (sfemu sub-project B)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Load a user-supplied MAME ROM set and run Street Fighter II's 68000 boot code against a modelled CPS-1 memory map with a scanline-accurate frame schedule and a vblank interrupt.

**Architecture:** Three new crates. `romset` (host-facing, fallible, 2 deps) reads a zip or directory and reproduces MAME's ROM interleaves with CRC verification. `machine` (zero deps, never panics on guest input) is the CPS-1 board: it implements `m68k::Bus` over ROM, RAM, gfxram, and the I/O block, and drives the CPU 640 cycles per scanline for 262 scanlines. `sfemu` is a thin binary joining them. `crates/m68k` is not modified.

**Tech Stack:** Rust 1.93, edition 2021, workspace at repo root. `miniz_oxide` + `adler2` in `romset` only. No GUI, no async, no `unsafe`.

## Global Constraints

- **No ROM is bundled, fetched, downloaded, or committed, by any code in this repository, for any purpose including diagnostics and test fixtures.** SF1/SF2 are commercial Capcom code. ROM data arrives only from a path the user supplies at runtime. `GameSpec` tables hold names, offsets, lengths, and CRCs — metadata, never data.
- `crates/m68k` is **UNCHANGED**. Do not add to, edit, or reformat it. If a task appears to require a core change, that is a plan contradiction to surface, not a change to make: 317,500 verified vector cases run through that trait.
- Sub-project A's gate must still pass at the end of every task: **127/127 groups, 317,500/317,500 cases, 221 m68k unit tests, 18 harness tests, 128 suite tests**.
- `machine` has **zero** dependencies and does not depend on `romset`. `romset` has exactly two (`miniz_oxide`, `adler2`).
- `#![forbid(unsafe_code)]` in every new crate, matching `m68k`.
- `cargo fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean; `cargo doc --no-deps` with **0 warnings**.
- **Guest faults are emulated, never Rust errors.** Unmapped reads return `0xFFFF`, writes to ROM are discarded, both counted. **Host faults are `Result`** with the file name in every variant.
- **`machine` never panics on any guest address.** Index by masking (`(addr >> 1) & 0x7FFF`), never by a bounds-checked slice index that a wild address could blow.
- **Every expected value in a test is a hand-written literal, and every literal gets a watched mutant.** A test that recomputes its expectation from the constant under test is the project's characteristic defect (see `docs/hardware/68000-notes.md`). Procedure per task: write the literal, break the code deliberately, watch the test go red, revert with `cp` from a `/tmp` backup — **never `git checkout`**, which destroys uncommitted work in the same file — and confirm `git status --porcelain` is clean between mutants.
- Board facts come from MAME `master` (BSD-3-Clause, Paul Leaman), `src/mame/capcom/{cps1.h,cps1.cpp,cps1_v.cpp}`, read 2026-08-07. **Cite the file and line** in a comment beside any transcribed constant. We reimplement from it as documentation; we do not translate its code.
- Addresses are byte offsets; array indices are word indices; **write the `/2` conversion at every boundary**. Mixing them shifts the CPS-A file by one register and reads scroll-Y as scroll-X.

---

## File Structure

| File | Responsibility |
|---|---|
| `Cargo.toml` | workspace members += `romset`, `machine`, `sfemu` |
| `crates/romset/src/lib.rs` | crate docs, re-exports, `RomError` |
| `crates/romset/src/spec.rs` | `GameSpec`, `RegionSpec`, `RomEntry`, `LoadKind` |
| `crates/romset/src/assemble.rs` | interleave arithmetic — pure, no I/O |
| `crates/romset/src/crc32.rs` | CRC-32 (poly 0xEDB88320), no dependency |
| `crates/romset/src/zip.rs` | central-directory parse, stored + deflate |
| `crates/romset/src/load.rs` | `load()` over a zip or a directory |
| `crates/romset/src/games.rs` | the `sf2` table, transcribed from `cps1.cpp:7101` |
| `crates/machine/src/lib.rs` | crate docs, re-exports |
| `crates/machine/src/board.rs` | `Board`, the `m68k::Bus` impl, the memory map |
| `crates/machine/src/config.rs` | `BoardConfig` (cpsb_addr/value, in2_addr) |
| `crates/machine/src/inputs.rs` | `Inputs`, active-low port assembly |
| `crates/machine/src/timing.rs` | `Timing` and the derived-constant proofs |
| `crates/machine/src/cps1.rs` | `Cps1`, `run_scanline`, `run_frame`, vblank |
| `crates/machine/src/trace.rs` | `Trace` counters and the unmapped map |
| `crates/machine/tests/programs.rs` | the hand-assembled boot programs |
| `crates/machine/tests/boot.rs` | the one `#[ignore]`d real-ROM test |
| `crates/sfemu/src/main.rs` | argument parsing, load, run N frames, print trace |
| `docs/hardware/cps1-notes.md` | accumulated CPS-1 facts |

---

### Task 1: `romset` scaffold and the interleave arithmetic

The part that is easiest to get subtly wrong and fully testable with no I/O. Do it first and in isolation.

**Files:**
- Modify: `Cargo.toml` (workspace members)
- Create: `crates/romset/Cargo.toml`, `crates/romset/src/lib.rs`, `crates/romset/src/spec.rs`, `crates/romset/src/assemble.rs`

**Interfaces:**
- Produces: `LoadKind`, `RomEntry`, `RegionSpec`, `GameSpec`, `assemble::place(dest, src, entry) -> Result<(), RomError>`, `RomError`.

- [ ] **Step 1: Add the crate to the workspace**

In `Cargo.toml`, add `"crates/romset"` to `members`. Then `crates/romset/Cargo.toml`:

```toml
[package]
name = "romset"
version = "0.1.0"
edition = "2021"
rust-version = "1.93"
publish = false

[dependencies]
miniz_oxide = "0.9"
```

`adler2` arrives transitively through `miniz_oxide`; it is not named directly.

- [ ] **Step 2: Write `spec.rs`**

```rust
//! Static descriptions of MAME ROM sets: what files a set contains, where each
//! one lands, and what it must checksum to.
//!
//! ⚠️ These tables hold **file names, offsets, lengths, and CRCs — never ROM
//! data.** SF1 and SF2 are commercial Capcom code and this repository neither
//! bundles nor fetches it. A table of names and checksums is metadata about a
//! product, the same category as a package manifest, and it is what makes
//! "the user supplies the file" a checkable claim rather than a hope.

/// How one file's bytes are distributed into its region.
///
/// The names mirror MAME's `ROM_LOAD*` macros so a transcribed table can be
/// diffed against `cps1.cpp` line by line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadKind {
    /// `ROM_LOAD`: byte `i` lands at `offset + i`.
    Byte,
    /// `ROM_LOAD16_BYTE`: byte `i` lands at `offset + 2*i`.
    ///
    /// The 68000 is big-endian, so an entry at an **even** offset supplies the
    /// **high** byte of each word and an odd offset the low byte. Getting this
    /// backwards produces a ROM whose every instruction word is byte-swapped:
    /// the CPU takes an illegal-instruction or address-error exception within a
    /// few steps rather than running visibly wrong for a while, which is the one
    /// mercy in this failure mode.
    Word16Byte,
    /// `ROM_LOAD64_WORD`: source word `i` (2 bytes) lands at `offset + 8*i`.
    Word64Word,
    /// `ROM_CONTINUE`: the first `split` bytes land at `offset`, the remainder at
    /// `cont_at`.
    Continue { split: usize, cont_at: usize },
}

/// One file in a ROM set.
#[derive(Debug, Clone, Copy)]
pub struct RomEntry {
    pub name: &'static str,
    pub offset: usize,
    /// Length of the **source file**, not of the span it occupies in the region.
    /// For an interleaved kind the span is a multiple of this.
    pub len: usize,
    pub crc32: u32,
    pub load: LoadKind,
}

/// One region: a contiguous address space the board presents to a chip.
#[derive(Debug, Clone, Copy)]
pub struct RegionSpec {
    pub name: &'static str,
    pub size: usize,
    pub entries: &'static [RomEntry],
}

/// One supported ROM set.
#[derive(Debug, Clone, Copy)]
pub struct GameSpec {
    pub name: &'static str,
    pub regions: &'static [RegionSpec],
}

impl GameSpec {
    pub fn region(&self, name: &str) -> Option<&'static RegionSpec> {
        self.regions.iter().find(|r| r.name == name)
    }
}
```

- [ ] **Step 3: Write `lib.rs` with `RomError`**

```rust
//! Loading MAME-format ROM sets supplied by the user at runtime.
//!
//! # This crate never obtains a ROM
//!
//! It reads a path handed to it. It contains no URL, no download, no embedded
//! ROM data, and no test fixture holding any. Its tests build synthetic archives
//! from patterns they generate. See [`spec`] for why the static tables are
//! metadata rather than data.

#![forbid(unsafe_code)]
#![deny(rustdoc::private_intra_doc_links)]

pub mod assemble;
pub mod crc32;
pub mod games;
pub mod load;
pub mod spec;
pub mod zip;

pub use load::{load, RomSet};
pub use spec::{GameSpec, LoadKind, RegionSpec, RomEntry};

/// A host fault: our setup is wrong, not the guest's.
///
/// Every variant names the file, because the whole value of checking is that the
/// message says which of eight interleaved files is bad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RomError {
    /// The archive or directory itself could not be read.
    Io { path: String, detail: String },
    /// A file the spec requires is not in the set.
    Missing { region: &'static str, name: &'static str },
    /// The file is present but the wrong length.
    WrongLength { name: &'static str, want: usize, got: usize },
    /// The file is present and the right length but the wrong content.
    Crc { name: &'static str, want: u32, got: u32 },
    /// The spec places an entry past the end of its region — our bug, not the
    /// user's, so it says so.
    SpecOverflow { region: &'static str, name: &'static str, end: usize, size: usize },
    /// The archive is not a zip, or uses a compression method MAME sets do not.
    Zip { path: String, detail: String },
}

impl core::fmt::Display for RomError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io { path, detail } => write!(f, "cannot read {path}: {detail}"),
            Self::Missing { region, name } => {
                write!(f, "region `{region}` is missing `{name}` — is this the right ROM set?")
            }
            Self::WrongLength { name, want, got } => {
                write!(f, "`{name}` is {got} bytes, expected {want}")
            }
            Self::Crc { name, want, got } => write!(
                f,
                "`{name}` has CRC32 {got:08x}, expected {want:08x} — wrong revision or a bad dump"
            ),
            Self::SpecOverflow { region, name, end, size } => write!(
                f,
                "internal: `{name}` ends at {end:#x} but region `{region}` is only {size:#x} — the spec table is wrong"
            ),
            Self::Zip { path, detail } => write!(f, "{path} is not a usable zip: {detail}"),
        }
    }
}

impl std::error::Error for RomError {}
```

- [ ] **Step 4: Write the failing test for `place`**

Create `crates/romset/src/assemble.rs` containing only the test module first, so the test fails to compile against a missing `place` — that is the intended red.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{LoadKind, RomEntry};

    /// The synthetic pattern is chosen so that a wrong interleave is visible at
    /// byte 1.
    ///
    /// ⚠️ A zero-filled or constant source would make `Byte` and `Word16Byte`
    /// produce **identical** output, so the test would pass with the interleave
    /// completely wrong. That is the same shape as this project's crossed-widths
    /// defect: the input has to *discriminate*, not merely be present. `0xA0 |`
    /// and `0xB0 |` make the two source files distinguishable in every byte.
    fn pat(tag: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| tag | (i as u8 & 0x0F)).collect()
    }

    fn entry(offset: usize, len: usize, load: LoadKind) -> RomEntry {
        RomEntry { name: "t", offset, len, crc32: 0, load }
    }

    #[test]
    fn word16_byte_interleaves_even_file_into_the_high_byte() {
        let mut dest = vec![0u8; 16];
        place(&mut dest, &pat(0xA0, 4), &entry(0, 4, LoadKind::Word16Byte), "r").unwrap();
        place(&mut dest, &pat(0xB0, 4), &entry(1, 4, LoadKind::Word16Byte), "r").unwrap();
        assert_eq!(
            &dest[..8],
            &[0xA0, 0xB0, 0xA1, 0xB1, 0xA2, 0xB2, 0xA3, 0xB3],
            "even entry supplies the high byte of each big-endian word"
        );
        // The first 68000 word of this region is therefore 0xA0B0, not 0xB0A0.
        assert_eq!(u16::from_be_bytes([dest[0], dest[1]]), 0xA0B0);
    }

    #[test]
    fn byte_kind_is_a_straight_copy_and_differs_from_word16() {
        let mut dest = vec![0u8; 16];
        place(&mut dest, &pat(0xA0, 4), &entry(0, 4, LoadKind::Byte), "r").unwrap();
        assert_eq!(&dest[..4], &[0xA0, 0xA1, 0xA2, 0xA3]);
        // The discrimination this test exists for: the same source under the two
        // kinds must not agree.
        let mut other = vec![0u8; 16];
        place(&mut other, &pat(0xA0, 4), &entry(0, 4, LoadKind::Word16Byte), "r").unwrap();
        assert_ne!(dest, other, "Byte and Word16Byte must be distinguishable");
    }

    #[test]
    fn word64_word_strides_two_bytes_every_eight() {
        let mut dest = vec![0u8; 32];
        place(&mut dest, &pat(0xA0, 4), &entry(0, 4, LoadKind::Word64Word), "r").unwrap();
        assert_eq!(&dest[0..2], &[0xA0, 0xA1]);
        assert_eq!(&dest[8..10], &[0xA2, 0xA3]);
        assert_eq!(&dest[2..8], &[0, 0, 0, 0, 0, 0], "the gap stays untouched");
    }

    #[test]
    fn continue_splits_the_file_across_two_offsets() {
        let mut dest = vec![0u8; 0x20];
        let e = entry(0x00, 0x10, LoadKind::Continue { split: 0x08, cont_at: 0x10 });
        place(&mut dest, &pat(0xA0, 0x10), &e, "r").unwrap();
        assert_eq!(dest[0x00], 0xA0, "first half at offset");
        assert_eq!(dest[0x07], 0xA7);
        assert_eq!(dest[0x08], 0x00, "nothing between the halves");
        assert_eq!(dest[0x10], 0xA8, "second half at cont_at");
        assert_eq!(dest[0x17], 0xAF);
    }

    #[test]
    fn an_entry_past_the_end_of_its_region_is_our_bug_and_says_so() {
        let mut dest = vec![0u8; 4];
        let err = place(&mut dest, &pat(0xA0, 4), &entry(2, 4, LoadKind::Byte), "maincpu")
            .unwrap_err();
        assert_eq!(
            err,
            crate::RomError::SpecOverflow { region: "maincpu", name: "t", end: 6, size: 4 }
        );
    }
}
```

- [ ] **Step 5: Run it and watch it fail**

`cargo test -p romset` → fails to compile, `cannot find function place`.

- [ ] **Step 6: Implement `place`**

Prepend to `assemble.rs`:

```rust
//! Distributing a source file's bytes into its region.
//!
//! Pure arithmetic over slices: no I/O, no archive, no allocation beyond the
//! caller's `dest`. Separated from [`crate::load`] for exactly that reason — the
//! interleave is the part with an off-by-one in it, and it is testable against a
//! synthetic pattern with nothing else in the loop.

use crate::spec::{LoadKind, RomEntry};
use crate::RomError;

/// The last byte index this entry writes, exclusive. Used for the bounds check
/// and by the region-size test in `games.rs`.
pub fn end_of(entry: &RomEntry) -> usize {
    match entry.load {
        LoadKind::Byte => entry.offset + entry.len,
        // The final byte is at offset + 2*(len-1), so the exclusive end is one past it.
        LoadKind::Word16Byte => entry.offset + 2 * entry.len.saturating_sub(1) + 1,
        LoadKind::Word64Word => {
            let words = entry.len / 2;
            entry.offset + 8 * words.saturating_sub(1) + 2
        }
        LoadKind::Continue { split, cont_at } => {
            (entry.offset + split).max(cont_at + entry.len.saturating_sub(split))
        }
    }
}

/// Writes `src` into `dest` according to `entry.load`.
///
/// `region` is only used to name the region in [`RomError::SpecOverflow`].
pub fn place(
    dest: &mut [u8],
    src: &[u8],
    entry: &RomEntry,
    region: &'static str,
) -> Result<(), RomError> {
    let end = end_of(entry);
    if end > dest.len() {
        return Err(RomError::SpecOverflow {
            region,
            name: entry.name,
            end,
            size: dest.len(),
        });
    }
    match entry.load {
        LoadKind::Byte => dest[entry.offset..entry.offset + src.len()].copy_from_slice(src),
        LoadKind::Word16Byte => {
            for (i, &b) in src.iter().enumerate() {
                dest[entry.offset + 2 * i] = b;
            }
        }
        LoadKind::Word64Word => {
            for (i, pair) in src.chunks_exact(2).enumerate() {
                let at = entry.offset + 8 * i;
                dest[at] = pair[0];
                dest[at + 1] = pair[1];
            }
        }
        LoadKind::Continue { split, cont_at } => {
            let (a, b) = src.split_at(split.min(src.len()));
            dest[entry.offset..entry.offset + a.len()].copy_from_slice(a);
            dest[cont_at..cont_at + b.len()].copy_from_slice(b);
        }
    }
    Ok(())
}
```

- [ ] **Step 7: Run the tests**

`cargo test -p romset` → 5 passed.

- [ ] **Step 8: Mutate each literal and watch it die**

Back up first: `cp crates/romset/src/assemble.rs /tmp/assemble.rs.bak`.

| Mutant | Must kill |
|---|---|
| `Word16Byte`: `dest[entry.offset + 2 * i]` → `dest[entry.offset + i]` | `word16_byte_interleaves...` and `byte_kind_is...` |
| `Word64Word`: `8 * i` → `4 * i` | `word64_word_strides...` |
| `Continue`: `cont_at` → `entry.offset + split` | `continue_splits...` |
| `end_of` Word16Byte: drop the `+ 1` | `an_entry_past_the_end...` |

After each: run `cargo test -p romset`, confirm red, then `cp /tmp/assemble.rs.bak crates/romset/src/assemble.rs` and confirm `git status --porcelain` shows only the intended new files.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml crates/romset
git commit -m "feat(romset): ROM-set spec types and interleave arithmetic

place() reproduces MAME's four ROM_LOAD kinds over in-memory slices, with
no I/O in the loop. Tested against a synthetic pattern chosen so Byte and
Word16Byte are distinguishable at byte 1 — zero-filled sources would make
every interleave produce identical output and the test would pass with the
arithmetic wrong.

The spec tables hold names, offsets, lengths and CRCs. No ROM data."
```

---

### Task 2: CRC-32 and zip reading

**Files:**
- Create: `crates/romset/src/crc32.rs`, `crates/romset/src/zip.rs`

**Interfaces:**
- Consumes: `RomError` from Task 1.
- Produces: `crc32::of(&[u8]) -> u32`, `zip::Archive::open(path)`, `Archive::names()`, `Archive::read(name) -> Result<Vec<u8>, RomError>`.

- [ ] **Step 1: Write the failing CRC test**

`crates/romset/src/crc32.rs`, test module only:

```rust
#[cfg(test)]
mod tests {
    use super::of;

    /// The three standard CRC-32 check values, written as literals.
    ///
    /// `"123456789"` → `0xCBF43926` is the CRC-32 spec's own check vector, so
    /// this pins the polynomial, the reflection, and both the init and final
    /// XOR of 0xFFFFFFFF. A table generated with the wrong polynomial fails here
    /// rather than four tasks later against a real ROM, where the only symptom
    /// would be "every CRC mismatches" — indistinguishable from a bad dump.
    #[test]
    fn matches_the_standard_check_vectors() {
        assert_eq!(of(b""), 0x0000_0000);
        assert_eq!(of(b"123456789"), 0xCBF4_3926);
        assert_eq!(of(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }

    #[test]
    fn a_single_flipped_bit_changes_the_result() {
        let a = of(&[0x00; 64]);
        let mut b = [0u8; 64];
        b[63] = 0x01;
        assert_ne!(a, of(&b));
    }
}
```

- [ ] **Step 2: Run it — fails, `of` not found**

- [ ] **Step 3: Implement CRC-32**

```rust
//! CRC-32 (IEEE 802.3, reflected, poly 0xEDB88320) — what zip and MAME both use.
//!
//! Hand-written rather than taken from a crate: it is 20 lines, and the whole
//! point of this crate's dependency budget is that `romset` adds a DEFLATE
//! decoder and nothing else.

/// CRC-32 of `data`.
pub fn of(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            // The reflected polynomial. A non-reflected 0x04C11DB7 here would
            // produce plausible-looking but wrong values for every input.
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}
```

- [ ] **Step 4: Run — 2 passed. Mutate: change `0xEDB88320` to `0xEDB88321`; the check-vector test must go red.** Revert via `/tmp` backup.

- [ ] **Step 5: Write the failing zip test**

`crates/romset/src/zip.rs`, test module only. The test **builds** the archives it reads, so no fixture file exists anywhere:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal but real zip: local headers, then a central directory,
    /// then an EOCD. Every field is written explicitly so the parser is tested
    /// against the format rather than against a library that shares its bugs.
    fn build_zip(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data, deflate) in files {
            let stored: Vec<u8> = if *deflate {
                miniz_oxide::deflate::compress_to_vec(data, 6)
            } else {
                data.to_vec()
            };
            let method: u16 = if *deflate { 8 } else { 0 };
            let crc = crate::crc32::of(data);
            let local_off = out.len() as u32;
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // time
            out.extend_from_slice(&0u16.to_le_bytes()); // date
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&stored);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra
            central.extend_from_slice(&0u16.to_le_bytes()); // comment
            central.extend_from_slice(&0u16.to_le_bytes()); // disk
            central.extend_from_slice(&0u16.to_le_bytes()); // int attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // ext attrs
            central.extend_from_slice(&local_off.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        let cd_len = central.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // this disk
        out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_len.to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out
    }

    fn pat(tag: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| tag | (i as u8 & 0x0F)).collect()
    }

    #[test]
    fn reads_stored_and_deflated_members_by_name() {
        let a = pat(0xA0, 5000);
        let b = pat(0xB0, 300);
        let bytes = build_zip(&[("rom_a.bin", &a, false), ("rom_b.bin", &b, true)]);
        let ar = Archive::parse(bytes, "test.zip".into()).unwrap();
        let mut names = ar.names();
        names.sort();
        assert_eq!(names, vec!["rom_a.bin", "rom_b.bin"]);
        assert_eq!(ar.read("rom_a.bin").unwrap(), a, "stored member");
        assert_eq!(ar.read("rom_b.bin").unwrap(), b, "deflated member");
    }

    #[test]
    fn a_deflated_member_is_actually_compressed_in_the_archive() {
        // Guards against the test passing because `build_zip` silently stored
        // the member it claimed to deflate — in which case the deflate path
        // above would never run and would be untested while looking tested.
        let a = vec![0x5Au8; 8192];
        let bytes = build_zip(&[("z.bin", &a, true)]);
        assert!(bytes.len() < 4096, "8 KB of one byte must compress; archive is {} bytes", bytes.len());
        let ar = Archive::parse(bytes, "t.zip".into()).unwrap();
        assert_eq!(ar.read("z.bin").unwrap(), a);
    }

    #[test]
    fn a_missing_member_is_an_error_naming_the_archive() {
        let ar = Archive::parse(build_zip(&[("a.bin", b"x", false)]), "t.zip".into()).unwrap();
        assert!(ar.read("absent.bin").is_err());
    }

    #[test]
    fn a_non_zip_is_rejected_rather_than_misparsed() {
        assert!(Archive::parse(vec![0u8; 64], "t.zip".into()).is_err());
        assert!(Archive::parse(Vec::new(), "t.zip".into()).is_err());
    }

    #[test]
    fn an_unsupported_compression_method_is_named_not_silently_wrong() {
        let mut bytes = build_zip(&[("a.bin", b"0123456789", false)]);
        // Method field in the central directory: EOCD is 22 bytes, the central
        // record starts 46+8 bytes before it for a one-file 8-char-name archive.
        let cd = bytes.len() - 22 - (46 + 8);
        bytes[cd + 10] = 93; // zstd — a real method, not one MAME sets use
        let ar = Archive::parse(bytes, "t.zip".into()).unwrap();
        let err = ar.read("a.bin").unwrap_err();
        assert!(format!("{err}").contains("93"), "the message must name the method: {err}");
    }
}
```

- [ ] **Step 6: Run — fails, `Archive` not found**

- [ ] **Step 7: Implement `zip.rs`**

```rust
//! Just enough zip to read a MAME ROM set.
//!
//! # Why hand-written
//!
//! The `zip` crate pulls **75 crates** — AES, bzip2, zstd, PPMd, `time`, `sha1`
//! — to support methods no ROM set uses. MAME sets use exactly two: stored (0)
//! and deflate (8). Parsing a central directory for those is ~120 lines, so the
//! trade is 120 lines of ours against 73 crates of compile time and attack
//! surface, in a project whose defining property is a dependency-free core.
//! `miniz_oxide` supplies the DEFLATE stage and nothing else.
//!
//! # What is deliberately not supported
//!
//! Zip64, encryption, multi-disk archives, and data descriptors. A ROM set using
//! any of them is rejected **by name** rather than misparsed — see
//! [`Archive::read`]. Silence is the failure mode this crate exists to avoid.

use crate::RomError;
use std::collections::BTreeMap;
use std::path::Path;

const EOCD_SIG: u32 = 0x0605_4b50;
const CD_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
const EOCD_MIN: usize = 22;

struct Member {
    method: u16,
    comp_size: usize,
    uncomp_size: usize,
    local_off: usize,
    crc: u32,
}

/// A zip archive held in memory.
///
/// ROM sets are a few megabytes and read once, so the whole file is buffered:
/// it removes seek handling from the parser entirely, and the peak footprint is
/// the archive plus the region being assembled.
pub struct Archive {
    bytes: Vec<u8>,
    path: String,
    members: BTreeMap<String, Member>,
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        *b.get(at + 3)?,
    ]))
}

impl Archive {
    /// Reads and parses the archive at `path`.
    pub fn open(path: &Path) -> Result<Self, RomError> {
        let bytes = std::fs::read(path).map_err(|e| RomError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        Self::parse(bytes, path.display().to_string())
    }

    /// Parses an in-memory archive. `path` is used only in error messages.
    pub fn parse(bytes: Vec<u8>, path: String) -> Result<Self, RomError> {
        let bad = |detail: &str| RomError::Zip { path: path.clone(), detail: detail.to_string() };
        if bytes.len() < EOCD_MIN {
            return Err(bad("shorter than an empty zip's end-of-central-directory record"));
        }
        // Scan backwards for the EOCD: it is last, but a trailing comment may
        // follow it, so its position is not fixed.
        let start = bytes.len() - EOCD_MIN;
        let floor = start.saturating_sub(0xFFFF);
        let eocd = (floor..=start)
            .rev()
            .find(|&i| le32(&bytes, i) == Some(EOCD_SIG))
            .ok_or_else(|| bad("no end-of-central-directory signature"))?;

        let count = le16(&bytes, eocd + 10).ok_or_else(|| bad("truncated EOCD"))? as usize;
        let cd_off = le32(&bytes, eocd + 16).ok_or_else(|| bad("truncated EOCD"))? as usize;

        let mut members = BTreeMap::new();
        let mut at = cd_off;
        for _ in 0..count {
            if le32(&bytes, at) != Some(CD_SIG) {
                return Err(bad("central directory entry has a bad signature"));
            }
            let method = le16(&bytes, at + 10).ok_or_else(|| bad("truncated CD entry"))?;
            let crc = le32(&bytes, at + 16).ok_or_else(|| bad("truncated CD entry"))?;
            let comp = le32(&bytes, at + 20).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let uncomp = le32(&bytes, at + 24).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let nlen = le16(&bytes, at + 28).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let elen = le16(&bytes, at + 30).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let clen = le16(&bytes, at + 32).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let local = le32(&bytes, at + 42).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let name_at = at + 46;
            let raw = bytes
                .get(name_at..name_at + nlen)
                .ok_or_else(|| bad("central directory name runs past the end of the file"))?;
            let name = String::from_utf8_lossy(raw).to_string();
            // MAME sets are flat; a path separator means a nested layout we
            // match on the base name instead.
            let name = name.rsplit('/').next().unwrap_or(&name).to_string();
            if !name.is_empty() {
                members.insert(
                    name,
                    Member { method, comp_size: comp, uncomp_size: uncomp, local_off: local, crc },
                );
            }
            at = name_at + nlen + elen + clen;
        }
        Ok(Self { bytes, path, members })
    }

    /// Every member's base name.
    pub fn names(&self) -> Vec<String> {
        self.members.keys().cloned().collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.members.contains_key(name)
    }

    /// Decompresses one member.
    ///
    /// The stored CRC is **not** checked here: [`crate::load`] checks against the
    /// spec's expected CRC, which is the value that matters. An archive whose own
    /// CRC agrees with corrupt data is still corrupt.
    pub fn read(&self, name: &str) -> Result<Vec<u8>, RomError> {
        let bad = |detail: String| RomError::Zip { path: self.path.clone(), detail };
        let m = self
            .members
            .get(name)
            .ok_or_else(|| bad(format!("no member named `{name}`")))?;
        if le32(&self.bytes, m.local_off) != Some(LOCAL_SIG) {
            return Err(bad(format!("`{name}` has a bad local header")));
        }
        let nlen = le16(&self.bytes, m.local_off + 26)
            .ok_or_else(|| bad(format!("`{name}` has a truncated local header")))? as usize;
        let elen = le16(&self.bytes, m.local_off + 28)
            .ok_or_else(|| bad(format!("`{name}` has a truncated local header")))? as usize;
        let data_at = m.local_off + 30 + nlen + elen;
        let comp = self
            .bytes
            .get(data_at..data_at + m.comp_size)
            .ok_or_else(|| bad(format!("`{name}` runs past the end of the archive")))?;
        let _ = m.crc; // see the doc comment above
        match m.method {
            0 => Ok(comp.to_vec()),
            8 => miniz_oxide::inflate::decompress_to_vec(comp).map_err(|e| {
                bad(format!("`{name}` failed to inflate: {:?}", e.status))
            }),
            other => Err(bad(format!(
                "`{name}` uses compression method {other}; only stored (0) and deflate (8) are supported"
            ))),
        }
        .and_then(|out| {
            if out.len() == m.uncomp_size {
                Ok(out)
            } else {
                Err(bad(format!(
                    "`{name}` inflated to {} bytes, the directory says {}",
                    out.len(),
                    m.uncomp_size
                )))
            }
        })
    }
}
```

Note `decompress_to_vec` (raw deflate), **not** `decompress_to_vec_zlib`: zip method 8 is a bare deflate stream with no zlib header. Verified 2026-08-07 that the raw variant round-trips `compress_to_vec` output.

- [ ] **Step 8: Run — 7 tests pass in `romset`**

- [ ] **Step 9: Mutate**

| Mutant | Must kill |
|---|---|
| `decompress_to_vec` → `decompress_to_vec_zlib` | `reads_stored_and_deflated...` |
| `at + 42` (local offset) → `at + 38` | `reads_stored_and_deflated...` |
| the `other =>` arm → `_ => Ok(comp.to_vec())` | `an_unsupported_compression_method...` |
| the EOCD backward scan → `bytes.len() - EOCD_MIN` only | nothing, if no comment is present — **note this in the notes file**: the test suite does not currently discriminate the scan, because none of the archives it builds has a trailing comment. Add a case that appends 8 comment bytes so the scan is load-bearing. |

That last row is the point of doing mutation testing rather than assuming: fix it by extending `build_zip` with a comment-length parameter and adding a test, then re-run the mutant and watch it die.

- [ ] **Step 10: Commit**

```bash
git add crates/romset/src/crc32.rs crates/romset/src/zip.rs crates/romset/src/lib.rs
git commit -m "feat(romset): CRC-32 and a minimal zip reader

Hand-written over the `zip` crate: 75 crates (AES, bzip2, zstd, PPMd,
sha1, time) for two methods MAME sets actually use. miniz_oxide supplies
DEFLATE and nothing else; romset's whole tree is 2 crates.

Zip64, encryption, and data descriptors are rejected by name rather than
misparsed. Tests build every archive they read, so no fixture exists."
```

---

### Task 3: `load()` over a zip or a directory, with CRC verification

**Files:**
- Create: `crates/romset/src/load.rs`
- Modify: `crates/romset/src/lib.rs` (already declares the module)

**Interfaces:**
- Consumes: `spec`, `assemble::place`, `crc32::of`, `zip::Archive`.
- Produces: `RomSet { regions: BTreeMap<String, Vec<u8>> }`, `load(&GameSpec, &Path) -> Result<RomSet, RomError>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{GameSpec, LoadKind, RegionSpec, RomEntry};

    fn pat(tag: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| tag | (i as u8 & 0x0F)).collect()
    }

    // A two-file interleaved region, the shape SF2's `maincpu` has.
    static ENTRIES: &[RomEntry] = &[
        RomEntry { name: "even.bin", offset: 0, len: 8, crc32: 0, load: LoadKind::Word16Byte },
        RomEntry { name: "odd.bin", offset: 1, len: 8, crc32: 0, load: LoadKind::Word16Byte },
    ];
    static REGIONS: &[RegionSpec] = &[RegionSpec { name: "maincpu", size: 16, entries: ENTRIES }];
    static SPEC: GameSpec = GameSpec { name: "synthetic", regions: REGIONS };

    /// `SPEC`'s CRCs are zero, so build a spec with the real ones at runtime.
    /// This is deliberately *not* done by having `load` skip zero CRCs: an
    /// exemption for "CRC not known yet" is exactly how verification stops
    /// verifying. The test computes the value it expects instead.
    ///
    /// `region_size` lets one caller widen the region past what the entries
    /// populate, so the zero-fill of unpopulated space is observable.
    fn spec_with_real_crcs(region_size: usize) -> (GameSpec, Vec<u8>, Vec<u8>) {
        let even = pat(0xA0, 8);
        let odd = pat(0xB0, 8);
        let entries: &'static [RomEntry] = Box::leak(Box::new([
            RomEntry {
                name: "even.bin", offset: 0, len: 8,
                crc32: crate::crc32::of(&pat(0xA0, 8)), load: LoadKind::Word16Byte,
            },
            RomEntry {
                name: "odd.bin", offset: 1, len: 8,
                crc32: crate::crc32::of(&pat(0xB0, 8)), load: LoadKind::Word16Byte,
            },
        ]));
        let regions: &'static [RegionSpec] =
            Box::leak(Box::new([RegionSpec { name: "maincpu", size: region_size, entries }]));
        (GameSpec { name: "synthetic", regions }, even, odd)
    }

    fn write_dir(files: &[(&str, &[u8])]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("romset-test-{}", files.len() * 7 + files[0].1.len()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (n, d) in files {
            std::fs::write(dir.join(n), d).unwrap();
        }
        dir
    }

    #[test]
    fn loads_a_directory_and_interleaves_correctly() {
        let (spec, even, odd) = spec_with_real_crcs(16);
        let dir = write_dir(&[("even.bin", &even), ("odd.bin", &odd)]);
        let set = load(&spec, &dir).unwrap();
        let r = &set.regions["maincpu"];
        assert_eq!(r.len(), 16);
        assert_eq!(&r[..4], &[0xA0, 0xB0, 0xA1, 0xB1], "even file is the high byte");
    }

    #[test]
    fn a_flipped_bit_fails_with_the_file_name_and_both_crcs() {
        let (spec, even, odd) = spec_with_real_crcs(16);
        let mut bad = odd.clone();
        bad[3] ^= 0x01;
        let dir = write_dir(&[("even.bin", &even), ("odd.bin", &bad)]);
        match load(&spec, &dir) {
            Err(crate::RomError::Crc { name, want, got }) => {
                assert_eq!(name, "odd.bin");
                assert_eq!(want, crate::crc32::of(&odd));
                assert_ne!(got, want);
            }
            other => panic!("a one-bit change must be a Crc error, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_file_names_the_region_and_the_file() {
        let (spec, even, _) = spec_with_real_crcs(16);
        let dir = write_dir(&[("even.bin", &even)]);
        assert_eq!(
            load(&spec, &dir).unwrap_err(),
            crate::RomError::Missing { region: "maincpu", name: "odd.bin" }
        );
    }

    #[test]
    fn a_short_file_is_a_length_error_not_a_crc_error() {
        // Distinct diagnoses: a truncated file is a different user problem from
        // a wrong revision, and collapsing both into "CRC mismatch" sends the
        // user looking for the wrong thing.
        let (spec, even, odd) = spec_with_real_crcs(16);
        let dir = write_dir(&[("even.bin", &even), ("odd.bin", &odd[..4])]);
        assert_eq!(
            load(&spec, &dir).unwrap_err(),
            crate::RomError::WrongLength { name: "odd.bin", want: 8, got: 4 }
        );
    }

    #[test]
    fn unpopulated_space_in_a_region_is_zero() {
        // SF2 populates 0x000000-0x0FFFFF of a 0x400000 region; the rest must
        // read as zero rather than as uninitialised memory, because that is what
        // an unpopulated socket returns and what the 68000 will fetch if it ever
        // jumps there.
        let (spec, even, odd) = spec_with_real_crcs(32);
        let dir = write_dir(&[("even.bin", &even), ("odd.bin", &odd)]);
        let set = load(&spec, &dir).unwrap();
        let r = &set.regions["maincpu"];
        assert_eq!(r.len(), 32);
        assert_eq!(&r[..4], &[0xA0, 0xB0, 0xA1, 0xB1], "the populated part is unchanged");
        assert_eq!(&r[16..], &[0u8; 16], "the unpopulated tail is zero");
    }
}
```

Note that `write_dir` derives its directory name from the file count and the
first file's length, so the two callers that pass different `region_size` values
with identical files share a directory. That is harmless — both write the same
bytes — but if a later test needs distinct contents at the same shape, give
`write_dir` an explicit name parameter rather than relying on the derivation.

- [ ] **Step 2: Run — fails, `load` not found**

- [ ] **Step 3: Implement `load.rs`**

```rust
//! Reading a ROM set from a zip or a directory.

use crate::assemble::place;
use crate::spec::GameSpec;
use crate::zip::Archive;
use crate::RomError;
use std::collections::BTreeMap;
use std::path::Path;

/// A loaded ROM set: one assembled byte vector per region.
pub struct RomSet {
    pub regions: BTreeMap<String, Vec<u8>>,
}

impl RomSet {
    pub fn region(&self, name: &str) -> Option<&[u8]> {
        self.regions.get(name).map(Vec::as_slice)
    }
}

enum Source {
    Zip(Archive),
    Dir(std::path::PathBuf),
}

impl Source {
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        match self {
            Self::Zip(a) => a.read(name).ok(),
            Self::Dir(d) => std::fs::read(d.join(name)).ok(),
        }
    }
}

/// Loads `spec` from `path`, which may be a zip archive or a directory of loose
/// files.
///
/// A directory is accepted because a user who owns the board and dumped it
/// themselves has loose files; requiring them to re-zip would serve nothing.
///
/// # Verification
///
/// Every entry's length **and** CRC-32 is checked, and a mismatch is an error
/// rather than a warning. A wrong or bad-dump ROM produces a 68000 executing
/// garbage, and the symptom surfaces thousands of instructions later as an
/// unexplained address error in code that looks like ours. Checking 32 bits here
/// converts a week of debugging into one line of output. There is deliberately
/// **no** "unknown CRC" exemption: an exemption is how verification stops
/// verifying.
pub fn load(spec: &GameSpec, path: &Path) -> Result<RomSet, RomError> {
    let src = if path.is_dir() {
        Source::Dir(path.to_path_buf())
    } else {
        Source::Zip(Archive::open(path)?)
    };

    let mut regions = BTreeMap::new();
    for region in spec.regions {
        let mut buf = vec![0u8; region.size];
        for entry in region.entries {
            let data = src.get(entry.name).ok_or(RomError::Missing {
                region: region.name,
                name: entry.name,
            })?;
            if data.len() != entry.len {
                return Err(RomError::WrongLength {
                    name: entry.name,
                    want: entry.len,
                    got: data.len(),
                });
            }
            let got = crate::crc32::of(&data);
            if got != entry.crc32 {
                return Err(RomError::Crc { name: entry.name, want: entry.crc32, got });
            }
            place(&mut buf, &data, entry, region.name)?;
        }
        regions.insert(region.name.to_string(), buf);
    }
    Ok(RomSet { regions })
}
```

- [ ] **Step 4: Run — all `romset` tests pass**

- [ ] **Step 5: Mutate**

| Mutant | Must kill |
|---|---|
| skip the CRC check when `entry.crc32 == 0` | `a_flipped_bit_fails...` (make one entry's CRC legitimately 0 first, or simply confirm the check is unconditional by removing it entirely — removal must kill the test) |
| `got != entry.crc32` → `got == entry.crc32` | `loads_a_directory...` |
| length check removed | `a_short_file_is_a_length_error...` |

- [ ] **Step 6: Commit**

---

### Task 4: The `sf2` ROM-set table

**Files:**
- Create: `crates/romset/src/games.rs`

**Interfaces:**
- Produces: `games::SF2: GameSpec`.

- [ ] **Step 1: Transcribe the table**

From `cps1.cpp:7101-7133`. `CODE_SIZE` is `0x400000` (`cps1.cpp:4063`).

```rust
//! Supported ROM sets.
//!
//! Transcribed from MAME `master`, `src/mame/capcom/cps1.cpp:7101-7133`
//! (BSD-3-Clause, copyright-holders Paul Leaman), read 2026-08-07.
//!
//! ⚠️ Names, offsets, lengths and CRCs only. No ROM data — see [`crate::spec`].

use crate::spec::{GameSpec, LoadKind, RegionSpec, RomEntry};

const W16: LoadKind = LoadKind::Word16Byte;
const W64: LoadKind = LoadKind::Word64Word;

/// 68000 program: four pairs of 128 KB files, byte-interleaved.
///
/// The **even** offset of each pair supplies the high byte of the big-endian
/// word. 8 x 0x20000 = 1 MB at 0x000000-0x0FFFFF; 0x100000-0x3FFFFF is
/// unpopulated and reads as zero.
static SF2_MAINCPU: &[RomEntry] = &[
    RomEntry { name: "sf2e_30g.11e", offset: 0x00000, len: 0x20000, crc32: 0xfe39ee33, load: W16 },
    RomEntry { name: "sf2e_37g.11f", offset: 0x00001, len: 0x20000, crc32: 0xfb92cd74, load: W16 },
    RomEntry { name: "sf2e_31g.12e", offset: 0x40000, len: 0x20000, crc32: 0x69a0a301, load: W16 },
    RomEntry { name: "sf2e_38g.12f", offset: 0x40001, len: 0x20000, crc32: 0x5e22db70, load: W16 },
    RomEntry { name: "sf2e_28g.9e",  offset: 0x80000, len: 0x20000, crc32: 0x8bf9f1e5, load: W16 },
    RomEntry { name: "sf2e_35g.9f",  offset: 0x80001, len: 0x20000, crc32: 0x626ef934, load: W16 },
    RomEntry { name: "sf2_29b.10e",  offset: 0xc0000, len: 0x20000, crc32: 0xbb4af315, load: W16 },
    RomEntry { name: "sf2_36b.10f",  offset: 0xc0001, len: 0x20000, crc32: 0xc02a13eb, load: W16 },
];

/// Graphics: twelve 512 KB files in three groups of four, 16-bit words strided
/// into a 64-bit layout. Sub-project B loads this and decodes nothing; C owns
/// the tile decode.
static SF2_GFX: &[RomEntry] = &[
    RomEntry { name: "sf2-5m.4a",  offset: 0x000000, len: 0x80000, crc32: 0x22c9cc8e, load: W64 },
    RomEntry { name: "sf2-7m.6a",  offset: 0x000002, len: 0x80000, crc32: 0x57213be8, load: W64 },
    RomEntry { name: "sf2-1m.3a",  offset: 0x000004, len: 0x80000, crc32: 0xba529b4f, load: W64 },
    RomEntry { name: "sf2-3m.5a",  offset: 0x000006, len: 0x80000, crc32: 0x4b1b33a8, load: W64 },
    RomEntry { name: "sf2-6m.4c",  offset: 0x200000, len: 0x80000, crc32: 0x2c7e2229, load: W64 },
    RomEntry { name: "sf2-8m.6c",  offset: 0x200002, len: 0x80000, crc32: 0xb5548f17, load: W64 },
    RomEntry { name: "sf2-2m.3c",  offset: 0x200004, len: 0x80000, crc32: 0x14b84312, load: W64 },
    RomEntry { name: "sf2-4m.5c",  offset: 0x200006, len: 0x80000, crc32: 0x5e9cd89a, load: W64 },
    RomEntry { name: "sf2-13m.4d", offset: 0x400000, len: 0x80000, crc32: 0x994bfa58, load: W64 },
    RomEntry { name: "sf2-15m.6d", offset: 0x400002, len: 0x80000, crc32: 0x3e66ad9d, load: W64 },
    RomEntry { name: "sf2-9m.3d",  offset: 0x400004, len: 0x80000, crc32: 0xc1befaa8, load: W64 },
    RomEntry { name: "sf2-11m.5d", offset: 0x400006, len: 0x80000, crc32: 0x0627c831, load: W64 },
];

/// Z80 program: one 64 KB file whose halves land 64 KB apart
/// (`ROM_LOAD` + `ROM_CONTINUE`). Loaded for sub-project D; nothing reads it in B.
static SF2_AUDIOCPU: &[RomEntry] = &[RomEntry {
    name: "sf2_9.12a",
    offset: 0x00000,
    len: 0x10000,
    crc32: 0xa4823a1b,
    load: LoadKind::Continue { split: 0x08000, cont_at: 0x10000 },
}];

/// OKI MSM6295 samples: two 128 KB files, concatenated.
static SF2_OKI: &[RomEntry] = &[
    RomEntry { name: "sf2_18.11c", offset: 0x00000, len: 0x20000, crc32: 0x7f162009, load: LoadKind::Byte },
    RomEntry { name: "sf2_19.12c", offset: 0x20000, len: 0x20000, crc32: 0xbeade53f, load: LoadKind::Byte },
];

static SF2_REGIONS: &[RegionSpec] = &[
    // CODE_SIZE, cps1.cpp:4063
    RegionSpec { name: "maincpu", size: 0x400000, entries: SF2_MAINCPU },
    RegionSpec { name: "gfx", size: 0x600000, entries: SF2_GFX },
    RegionSpec { name: "audiocpu", size: 0x18000, entries: SF2_AUDIOCPU },
    RegionSpec { name: "oki", size: 0x40000, entries: SF2_OKI },
];

/// Street Fighter II: The World Warrior (World 910214), MAME set `sf2`.
pub static SF2: GameSpec = GameSpec { name: "sf2", regions: SF2_REGIONS };

/// Every set this crate knows.
pub static ALL: &[&GameSpec] = &[&SF2];

pub fn by_name(name: &str) -> Option<&'static GameSpec> {
    ALL.iter().copied().find(|g| g.name == name)
}
```

- [ ] **Step 2: Write the table's consistency test**

The value here is catching a transcription slip — a wrong offset or a duplicated CRC — without any ROM present.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::assemble::end_of;

    #[test]
    fn sf2_has_four_regions_with_the_expected_file_counts() {
        assert_eq!(SF2.regions.len(), 4);
        let counts: Vec<usize> = SF2.regions.iter().map(|r| r.entries.len()).collect();
        assert_eq!(counts, vec![8, 12, 1, 2], "maincpu, gfx, audiocpu, oki");
    }

    #[test]
    fn every_entry_fits_inside_its_region() {
        for r in SF2.regions {
            for e in r.entries {
                assert!(
                    end_of(e) <= r.size,
                    "{} ends at {:#x}, region {} is {:#x}",
                    e.name, end_of(e), r.name, r.size
                );
            }
        }
    }

    #[test]
    fn maincpu_populates_exactly_the_first_megabyte() {
        let r = SF2.region("maincpu").unwrap();
        let top = r.entries.iter().map(end_of).max().unwrap();
        assert_eq!(top, 0x100000, "8 x 0x20000 interleaved into 1 MB");
        assert_eq!(r.size, 0x400000, "CODE_SIZE, cps1.cpp:4063");
    }

    #[test]
    fn maincpu_pairs_alternate_even_and_odd_offsets() {
        // A transcription slip that gives two files the same parity silently
        // byte-swaps half the program.
        let r = SF2.region("maincpu").unwrap();
        for pair in r.entries.chunks_exact(2) {
            assert_eq!(pair[0].offset % 2, 0, "{} must be the high byte", pair[0].name);
            assert_eq!(pair[1].offset % 2, 1, "{} must be the low byte", pair[1].name);
            assert_eq!(pair[0].offset + 1, pair[1].offset, "a pair shares a base");
        }
    }

    #[test]
    fn no_two_entries_share_a_crc() {
        // Twelve 512 KB gfx files with one copy-pasted CRC is the easiest
        // transcription error to make and the hardest to see by eye.
        let mut seen = std::collections::BTreeSet::new();
        for r in SF2.regions {
            for e in r.entries {
                assert!(seen.insert(e.crc32), "{} duplicates CRC {:08x}", e.name, e.crc32);
            }
        }
    }

    #[test]
    fn gfx_entries_stride_by_eight_within_each_group_of_four() {
        let r = SF2.region("gfx").unwrap();
        for group in r.entries.chunks_exact(4) {
            let base = group[0].offset;
            for (i, e) in group.iter().enumerate() {
                assert_eq!(e.offset, base + 2 * i, "{} in its group of four", e.name);
            }
        }
    }
}
```

- [ ] **Step 3: Run — 6 tests pass. Mutate: swap two `maincpu` offsets so a pair shares parity; `maincpu_pairs_alternate...` must go red. Duplicate a gfx CRC; `no_two_entries_share_a_crc` must go red.**

- [ ] **Step 4: Commit**

---

### Task 5: `machine` — `Board`, memory, and the `Bus` impl

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/machine/Cargo.toml`, `crates/machine/src/lib.rs`, `crates/machine/src/board.rs`

**Interfaces:**
- Consumes: `m68k::{Bus, M68k, decode::Decoder}`. **Not** `romset`.
- Produces: `Board::new(prog: &[u8]) -> Board`, `impl m68k::Bus for Board`, the ROM/RAM/gfxram map.

- [ ] **Step 1: Create the crate**

Add `"crates/machine"` to workspace members.

```toml
[package]
name = "machine"
version = "0.1.0"
edition = "2021"
rust-version = "1.93"
publish = false

[dependencies]
m68k = { path = "../m68k" }
```

**No other dependency, ever.** `machine` must not gain `romset`: that would drag in `miniz_oxide` and `std` and forfeit the WASM posture A paid for.

- [ ] **Step 2: Write the failing test**

`crates/machine/src/board.rs`, test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use m68k::Bus;

    fn board() -> Board {
        Board::new(&[])
    }

    #[test]
    fn ram_stores_words_and_bytes_big_endian() {
        let mut b = board();
        b.write16(0xFF_0000, 0x1234);
        assert_eq!(b.read16(0xFF_0000), 0x1234);
        assert_eq!(b.read8(0xFF_0000), 0x12, "high byte at the even address");
        assert_eq!(b.read8(0xFF_0001), 0x34, "low byte at the odd address");
        b.write8(0xFF_0001, 0xAB);
        assert_eq!(b.read16(0xFF_0000), 0x12AB, "a byte write must not disturb its neighbour");
    }

    #[test]
    fn ram_is_64k_and_wraps_within_its_window() {
        // 0xFF0000-0xFFFFFF is 64 KB, and the 68000's address bus is 24 bits, so
        // there is nothing above it to alias into.
        let mut b = board();
        b.write16(0xFF_FFFE, 0xBEEF);
        assert_eq!(b.read16(0xFF_FFFE), 0xBEEF);
        assert_eq!(b.read16(0xFF_0000), 0x0000, "the top word is not the bottom word");
    }

    #[test]
    fn gfxram_is_192k_and_distinct_from_main_ram() {
        let mut b = board();
        b.write16(0x90_0000, 0xAAAA);
        b.write16(0x92_FFFE, 0x5555);
        assert_eq!(b.read16(0x90_0000), 0xAAAA);
        assert_eq!(b.read16(0x92_FFFE), 0x5555);
        assert_eq!(b.read16(0xFF_0000), 0x0000, "gfxram must not alias main RAM");
    }

    #[test]
    fn rom_reads_the_program_and_ignores_writes() {
        let mut b = Board::new(&[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(b.read16(0x00_0000), 0x1234);
        assert_eq!(b.read16(0x00_0002), 0x5678);
        b.write16(0x00_0000, 0xFFFF);
        assert_eq!(b.read16(0x00_0000), 0x1234, "ROM is read-only; the write is discarded");
    }

    #[test]
    fn rom_beyond_the_program_reads_zero_not_out_of_bounds() {
        let mut b = Board::new(&[0x12, 0x34]);
        assert_eq!(b.read16(0x3F_FFFE), 0x0000, "unpopulated ROM space");
    }

    /// The invariant inherited verbatim from sub-project A: no guest address may
    /// panic. A mis-emulated jump produces exactly these accesses.
    #[test]
    fn no_address_in_the_whole_24_bit_space_panics() {
        let mut b = board();
        let mut addr = 0u32;
        while addr < 0x100_0000 {
            let _ = b.read16(addr);
            let _ = b.read8(addr);
            b.write16(addr, 0xDEAD);
            b.write8(addr, 0xBE);
            // Step by a prime so the sweep hits odd addresses and every region
            // boundary neighbourhood without taking 16M iterations.
            addr += 0x3B;
        }
    }
}
```

- [ ] **Step 3: Run — fails, `Board` not found**

- [ ] **Step 4: Implement `board.rs`**

```rust
//! The CPS-1 board: everything the 68000 can address.
//!
//! # Why this is a separate struct from [`crate::Cps1`]
//!
//! `M68k::step_with(&dec, &mut bus)` borrows the CPU and the bus mutably at the
//! same time, so the CPU cannot live inside the thing it buses to. Splitting
//! them at the top level makes `self.cpu.step_with(&self.dec, &mut self.board)`
//! legal with no `RefCell`, no state swapping, and no `unsafe` — preserving
//! sub-project A's `forbid(unsafe_code)` posture through the board layer.
//!
//! # Never panics on a guest address
//!
//! Every index is produced by masking, not by a bounds-checked slice index. A
//! mis-emulated jump produces wild addresses as a matter of course, and an
//! emulator that panics on one has turned a guest fault into a host crash. See
//! the sweep test at the bottom of this file.

use m68k::Bus;

/// Main RAM, 0xFF0000-0xFFFFFF: 64 KB = 32 K words.
const RAM_WORDS: usize = 0x8000;
/// gfxram, 0x900000-0x92FFFF: 192 KB = 96 K words (`cps1.cpp:592`).
const GFXRAM_WORDS: usize = 0x18000;
/// Program ROM space, 0x000000-0x3FFFFF (`CODE_SIZE`, `cps1.cpp:4063`).
const ROM_BYTES: usize = 0x40_0000;

pub struct Board {
    /// The assembled `maincpu` region, zero-padded to the full ROM space.
    pub rom: Vec<u8>,
    pub ram: Box<[u16; RAM_WORDS]>,
    pub gfxram: Box<[u16; GFXRAM_WORDS]>,
}

impl Board {
    /// `prog` is the assembled 68000 program region, big-endian, up to
    /// `ROM_BYTES`. Longer input is truncated; shorter is zero-padded, which is
    /// what an unpopulated socket reads as.
    ///
    /// Takes `&[u8]` and **not** a `romset::RomSet`: `machine` does not depend on
    /// `romset`, so that this crate stays at one dependency and keeps working
    /// without `std`. Every test in this crate builds its program inline.
    pub fn new(prog: &[u8]) -> Self {
        let mut rom = vec![0u8; ROM_BYTES];
        let n = prog.len().min(ROM_BYTES);
        rom[..n].copy_from_slice(&prog[..n]);
        Self {
            rom,
            ram: Box::new([0u16; RAM_WORDS]),
            gfxram: Box::new([0u16; GFXRAM_WORDS]),
        }
    }

    #[inline]
    fn ram_index(addr: u32) -> usize {
        ((addr >> 1) as usize) & (RAM_WORDS - 1)
    }

    #[inline]
    fn gfx_index(addr: u32) -> usize {
        // 0x18000 is not a power of two, so this is a remainder rather than a
        // mask. `%` on a `usize` cannot panic for a nonzero divisor.
        (((addr - 0x90_0000) >> 1) as usize) % GFXRAM_WORDS
    }

    /// The word at `addr`, or `None` if `addr` is in no mapped range.
    fn read_word(&mut self, addr: u32) -> Option<u16> {
        match addr {
            0x00_0000..=0x3F_FFFF => {
                let i = (addr & !1) as usize;
                Some(u16::from_be_bytes([self.rom[i], self.rom[i + 1]]))
            }
            0x90_0000..=0x92_FFFF => Some(self.gfxram[Self::gfx_index(addr)]),
            0xFF_0000..=0xFF_FFFF => Some(self.ram[Self::ram_index(addr)]),
            _ => None,
        }
    }

    /// Writes the word at `addr`; returns false if `addr` is in no writable range.
    fn write_word(&mut self, addr: u32, val: u16) -> bool {
        match addr {
            // ROM: the write reaches no chip that latches it. Discarded, not an
            // error — guest behaviour, not our bug.
            0x00_0000..=0x3F_FFFF => true,
            0x90_0000..=0x92_FFFF => {
                self.gfxram[Self::gfx_index(addr)] = val;
                true
            }
            0xFF_0000..=0xFF_FFFF => {
                self.ram[Self::ram_index(addr)] = val;
                true
            }
            _ => false,
        }
    }
}

impl Bus for Board {
    fn read16(&mut self, addr: u32) -> u16 {
        // Addresses arrive already masked to 24 bits by the core, but mask again:
        // this is also called directly by tests and by the frontend.
        let addr = addr & 0x00FF_FFFF;
        self.read_word(addr).unwrap_or(0xFFFF)
    }

    fn read8(&mut self, addr: u32) -> u8 {
        let w = self.read16(addr & !1);
        if addr & 1 == 0 {
            (w >> 8) as u8
        } else {
            w as u8
        }
    }

    fn write16(&mut self, addr: u32, val: u16) {
        let addr = addr & 0x00FF_FFFF;
        let _ = self.write_word(addr, val);
    }

    fn write8(&mut self, addr: u32, val: u8) {
        let base = addr & !1;
        let old = self.read16(base);
        let new = if addr & 1 == 0 {
            (u16::from(val) << 8) | (old & 0x00FF)
        } else {
            (old & 0xFF00) | u16::from(val)
        };
        self.write16(base, new);
    }
}
```

**Note on `read8` of unmapped space:** it returns `0xFF`, derived from `read16`'s `0xFFFF`. That is right for an active-low board with pull-ups.

- [ ] **Step 5: Write `lib.rs`**

```rust
//! The CPS-1 arcade board: memory map, frame schedule, and vblank interrupt.
//!
//! Zero dependencies beyond [`m68k`]. No `std` requirement in the simulation
//! path, no host I/O, no clock access — the same constraints sub-project A
//! honoured, for the same reason: WASM and rollback netplay stay nearly free.
//!
//! Board facts are cited to MAME `master`,
//! `src/mame/capcom/{cps1.h,cps1.cpp,cps1_v.cpp}` (BSD-3-Clause, Paul Leaman),
//! read 2026-08-07. No ROM is bundled, fetched, or committed.

#![forbid(unsafe_code)]
#![deny(rustdoc::private_intra_doc_links)]

pub mod board;

pub use board::Board;
```

- [ ] **Step 6: Run — 6 tests pass**

- [ ] **Step 7: Mutate**

| Mutant | Must kill |
|---|---|
| `read8`'s parity: swap the two branches | `ram_stores_words_and_bytes_big_endian` |
| `write8`: replace with `write16(base, val as u16)` | same test's last assertion |
| ROM `write_word` arm → write into `rom` | `rom_reads_the_program_and_ignores_writes` |
| `gfx_index`: drop the `- 0x900000` | `gfxram_is_192k_and_distinct_from_main_ram` |
| `read16` unmapped → `0x0000` | nothing yet — Task 6 adds the test that discriminates it. Note it and move on. |

- [ ] **Step 8: Commit**

---

### Task 6: The I/O block — inputs, DIPs, latches, CPS-A, CPS-B

**Files:**
- Create: `crates/machine/src/config.rs`, `crates/machine/src/inputs.rs`
- Modify: `crates/machine/src/board.rs`, `crates/machine/src/lib.rs`

**Interfaces:**
- Produces: `BoardConfig`, `Inputs`, and `Board`'s handling of `0x800000-0x80018F`.
- Consumes: `Board` from Task 5. `Board::new` gains a `BoardConfig` parameter — update Task 5's tests to pass `BoardConfig::sf2()`.

- [ ] **Step 1: Write `config.rs`**

```rust
//! Per-game board configuration.
//!
//! CPS-B is not RAM: it answers some reads with values the board wires in rather
//! than what was written. MAME keeps these in a per-game table
//! (`cps1_v.cpp:1766`); this is the same table with one row.

/// The CPS-B behaviours a game's board exhibits.
///
/// Offsets are **byte offsets from 0x800140**, matching MAME's table. The `/2`
/// to a word index is written at the point of use, never carried in the field.
#[derive(Debug, Clone, Copy)]
pub struct BoardConfig {
    /// Byte offset of the CPSB ID register, or `None` if the board has none.
    pub cpsb_addr: Option<u8>,
    /// The value that register reads back as, regardless of what was written.
    pub cpsb_value: u16,
    /// Byte offset of the extra-input port (`IN2`), or `None`.
    pub in2_addr: Option<u8>,
}

impl BoardConfig {
    /// Street Fighter II (World), MAME set `sf2`.
    ///
    /// `cps1_v.cpp:1838` — `{"sf2", CPS_B_11, mapper_STF29, 0x36}` — and
    /// `cps1_v.cpp:491`, where `CPS_B_11` gives `cpsb_addr 0x32`,
    /// `cpsb_value 0x0401`, and multiply protection `__not_applicable__`.
    ///
    /// The trailing `0x36` is `in2_addr`: **SF2's three kick buttons per player
    /// are read through the CPS-B space at 0x800176**, not through the
    /// 0x800000 port block. Both of these are boot-critical — the game reads
    /// 0x800172 and expects 0x0401, and a board that treats CPS-B as plain RAM
    /// returns the last value written and stops at a self-test failure.
    pub const fn sf2() -> Self {
        Self { cpsb_addr: Some(0x32), cpsb_value: 0x0401, in2_addr: Some(0x36) }
    }
}
```

- [ ] **Step 2: Write `inputs.rs`**

```rust
//! Controls and DIP switches, as the 68000 sees them.
//!
//! # Active low
//!
//! Every bit is **active low**: 1 is released, 0 is pressed. An idle board
//! therefore reads 0xFFFF across the whole port block. A model that returns 0
//! for "nothing pressed" boots with every button held, which looks like a game
//! bug rather than a bus bug.

/// Button and coin state. Set fields to `true` for *pressed*; the conversion to
/// active-low happens here, once.
#[derive(Debug, Clone, Copy, Default)]
pub struct Inputs {
    pub coin1: bool,
    pub coin2: bool,
    pub service: bool,
    pub start1: bool,
    pub start2: bool,
    /// P1 then P2: right, left, down, up, jab, strong, fierce.
    pub p1: PlayerInput,
    pub p2: PlayerInput,
    /// DSWA, DSWB, DSWC. Defaults are all-ones (every switch off).
    pub dsw: [u8; 3],
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerInput {
    pub right: bool,
    pub left: bool,
    pub down: bool,
    pub up: bool,
    /// Jab, strong, fierce.
    pub punch: [bool; 3],
    /// Short, forward, roundhouse.
    pub kick: [bool; 3],
}

impl Inputs {
    /// A board with nothing pressed and every DIP switch off.
    pub const fn idle() -> Self {
        Self {
            coin1: false, coin2: false, service: false, start1: false, start2: false,
            p1: PlayerInput::none(), p2: PlayerInput::none(),
            dsw: [0xFF; 3],
        }
    }

    /// `IN0` — coins, starts, service. `cps1.cpp:830-838`.
    pub fn in0(&self) -> u8 {
        let mut v = 0xFFu8;
        for (bit, pressed) in [
            (0, self.coin1), (1, self.coin2), (2, self.service),
            (4, self.start1), (5, self.start2),
        ] {
            if pressed {
                v &= !(1u8 << bit);
            }
        }
        v
    }

    /// `IN1` — both sticks and three punches each. `cps1.cpp:840-849`, P2 in the
    /// high byte.
    pub fn in1(&self) -> u16 {
        let lo = self.p1.stick_and_punch();
        let hi = self.p2.stick_and_punch();
        (u16::from(hi) << 8) | u16::from(lo)
    }

    /// `IN2` — the six kick buttons, read through CPS-B at `in2_addr`.
    /// `cps1.cpp:934-942`: P1 in bits 0-2, P2 in bits 4-6.
    pub fn in2(&self) -> u8 {
        let mut v = 0xFFu8;
        for i in 0..3 {
            if self.p1.kick[i] { v &= !(1u8 << i); }
            if self.p2.kick[i] { v &= !(1u8 << (4 + i)); }
        }
        v
    }
}

impl PlayerInput {
    pub const fn none() -> Self {
        Self { right: false, left: false, down: false, up: false, punch: [false; 3], kick: [false; 3] }
    }

    fn stick_and_punch(&self) -> u8 {
        let mut v = 0xFFu8;
        for (bit, pressed) in [
            (0, self.right), (1, self.left), (2, self.down), (3, self.up),
            (4, self.punch[0]), (5, self.punch[1]), (6, self.punch[2]),
        ] {
            if pressed {
                v &= !(1u8 << bit);
            }
        }
        v
    }
}
```

- [ ] **Step 3: Write the failing tests for the I/O map**

Add to `board.rs`'s test module:

```rust
#[test]
fn an_idle_board_reads_all_ones_across_the_port_block() {
    // Active low. This is the assertion a model that returns 0 for "nothing
    // pressed" fails, and it is the difference between a game that boots and a
    // game that thinks every button is held.
    let mut b = Board::new(&[], BoardConfig::sf2());
    assert_eq!(b.read16(0x80_0000), 0xFFFF, "IN1");
    assert_eq!(b.read16(0x80_0018), 0xFFFF, "IN0 in the high byte, 0xFF in the low");
    assert_eq!(b.read16(0x80_001A), 0xFFFF, "DSWA");
}

#[test]
fn dsw_reads_put_the_selected_bank_in_the_high_byte() {
    // cps1_dsw_r returns (in << 8) | 0xff — cps1.cpp:271. A model that returns
    // the byte in the low half passes every "is it 0xFF" check and then fails
    // every actual DIP-switch read.
    let mut b = Board::new(&[], BoardConfig::sf2());
    b.inputs.dsw = [0x12, 0x34, 0x56];
    assert_eq!(b.read16(0x80_0018), 0xFFFF, "offset 0 is IN0, not DSWA");
    assert_eq!(b.read16(0x80_001A), 0x12FF, "DSWA");
    assert_eq!(b.read16(0x80_001C), 0x34FF, "DSWB");
    assert_eq!(b.read16(0x80_001E), 0x56FF, "DSWC");
}

#[test]
fn in1_carries_p1_in_the_low_byte_and_p2_in_the_high() {
    let mut b = Board::new(&[], BoardConfig::sf2());
    b.inputs.p1.punch[0] = true; // bit 4
    b.inputs.p2.right = true; // bit 8
    assert_eq!(b.read16(0x80_0000), 0xFEEF);
}

#[test]
fn in1_is_one_word_mirrored_across_its_eight_bytes() {
    // 0x800000-0x800007 is one 16-bit port (cps1.cpp:580), so all four word
    // addresses read the same value.
    let mut b = Board::new(&[], BoardConfig::sf2());
    b.inputs.p1.up = true;
    let v = b.read16(0x80_0000);
    assert_eq!(v, 0xFFF7);
    for a in [0x80_0002, 0x80_0004, 0x80_0006] {
        assert_eq!(b.read16(a), v, "{a:#x} mirrors IN1");
    }
}

#[test]
fn the_cpsb_id_register_reads_its_wired_value_not_what_was_written() {
    // SF2's boot self-test: read 0x800140 + 0x32 and expect 0x0401
    // (cps1_v.cpp:491 CPS_B_11, cps1_v.cpp:2140).
    let mut b = Board::new(&[], BoardConfig::sf2());
    assert_eq!(b.read16(0x80_0172), 0x0401);
    b.write16(0x80_0172, 0xDEAD);
    assert_eq!(b.read16(0x80_0172), 0x0401, "the ID register is wired, not RAM");
}

#[test]
fn other_cps_b_registers_are_read_write() {
    let mut b = Board::new(&[], BoardConfig::sf2());
    b.write16(0x80_0140, 0x1111);
    assert_eq!(b.read16(0x80_0140), 0x1111);
    assert_eq!(b.read16(0x80_0172), 0x0401, "and the ID register still is not");
}

#[test]
fn in2_is_read_through_cps_b_at_in2_addr() {
    // The kicks are not in the 0x800000 block. 0x800140 + 0x36 = 0x800176.
    let mut b = Board::new(&[], BoardConfig::sf2());
    assert_eq!(b.read16(0x80_0176), 0x00FF, "idle: 0xFF in the low byte");
    b.inputs.p1.kick[2] = true; // bit 2
    assert_eq!(b.read16(0x80_0176), 0x00FB);
}

#[test]
fn cps_a_writes_land_in_the_register_file_by_word_index() {
    // 0x800100 is word index 0; 0x80010C is CPS1_SCROLL1_SCROLLX (0x0C/2 = 6).
    // This is the byte-offset/word-index boundary: an index of 0x0C here reads
    // scroll-2's X as scroll-1's, one register off.
    let mut b = Board::new(&[], BoardConfig::sf2());
    b.write16(0x80_010C, 0x0040);
    assert_eq!(b.cps_a[6], 0x0040, "CPS1_SCROLL1_SCROLLX, cps1.h:182");
    assert_eq!(b.cps_a[0x0C], 0x0000, "not indexed by the byte offset");
}

#[test]
fn the_sound_latches_take_the_low_byte_of_a_word_write() {
    // cps1_soundlatch_w: the byte depends on which half is being accessed
    // (cps1.cpp:302-314). A word write touches both halves, so ACCESSING_BITS_0_7
    // holds and the low byte wins.
    let mut b = Board::new(&[], BoardConfig::sf2());
    b.write16(0x80_0180, 0x00AB);
    assert_eq!(b.sound_latch[0], 0xAB);
    b.write16(0x80_0188, 0x00CD);
    assert_eq!(b.sound_latch[1], 0xCD);
}

#[test]
fn an_unmapped_read_is_all_ones_and_an_unmapped_write_is_counted() {
    // The discrimination Task 5 could not make: 0x0000 and 0xFFFF are both
    // plausible for a floating bus, and only one is right for a board with
    // pull-ups. 0x810000 is in no range the PAL decodes.
    let mut b = Board::new(&[], BoardConfig::sf2());
    assert_eq!(b.read16(0x81_0000), 0xFFFF);
    b.write16(0x81_0000, 0x1234);
    assert_eq!(b.read16(0x81_0000), 0xFFFF, "nothing latched it");
}
```

- [ ] **Step 4: Run — fails to compile (`Board::new` arity, missing fields)**

- [ ] **Step 5: Extend `Board`**

Add the fields and the I/O arms. `Board::new` becomes `new(prog: &[u8], cfg: BoardConfig)`; update Task 5's tests to `Board::new(&[], BoardConfig::sf2())`.

```rust
pub struct Board {
    pub rom: Vec<u8>,
    pub ram: Box<[u16; RAM_WORDS]>,
    pub gfxram: Box<[u16; GFXRAM_WORDS]>,
    /// CPS-A, 0x800100-0x80013F. Stored and **not interpreted** — sub-project C
    /// owns every meaning. `cps1.h:176-193` for the layout.
    pub cps_a: [u16; 0x20],
    /// CPS-B, 0x800140-0x80017F. Mostly RAM; see [`BoardConfig`] for the reads
    /// the board answers itself.
    pub cps_b: [u16; 0x20],
    pub inputs: Inputs,
    pub sound_latch: [u8; 2],
    pub coin_ctrl: u16,
    pub cfg: BoardConfig,
}
```

In `read_word`, before the `_ => None` arm:

```rust
// The I/O block. Ranges and handlers from cps1.cpp:577-594.
0x80_0000..=0x80_0007 => Some(self.inputs.in1()),
0x80_0018..=0x80_001F => {
    // cps1_dsw_r, cps1.cpp:257-272: four word offsets select IN0, DSWA,
    // DSWB, DSWC, and the byte lands in the HIGH half with 0xFF below it.
    let sel = ((addr - 0x80_0018) >> 1) & 3;
    let byte = match sel {
        0 => self.inputs.in0(),
        n => self.inputs.dsw[(n - 1) as usize],
    };
    Some((u16::from(byte) << 8) | 0x00FF)
}
0x80_0020..=0x80_0021 => Some(0xFFFF), // nopr(), cps1.cpp:583
0x80_0140..=0x80_017F => {
    let off = (addr - 0x80_0140) as u8 & !1;
    if self.cfg.cpsb_addr == Some(off) {
        // The boot self-test. cps1_v.cpp:2140.
        Some(self.cfg.cpsb_value)
    } else if self.cfg.in2_addr == Some(off) {
        // SF2's kicks. cps1_v.cpp:2155.
        Some(u16::from(self.inputs.in2()))
    } else {
        Some(self.cps_b[(off >> 1) as usize])
    }
}
```

In `write_word`:

```rust
0x80_0030..=0x80_0037 => {
    self.coin_ctrl = val;
    true
}
0x80_0100..=0x80_013F => {
    self.cps_a[(((addr - 0x80_0100) >> 1) & 0x1F) as usize] = val;
    true
}
0x80_0140..=0x80_017F => {
    self.cps_b[(((addr - 0x80_0140) >> 1) & 0x1F) as usize] = val;
    true
}
0x80_0180..=0x80_0187 => {
    self.sound_latch[0] = val as u8;
    true
}
0x80_0188..=0x80_018F => {
    self.sound_latch[1] = val as u8;
    true
}
```

Note the CPS-B **write** goes to `cps_b` even at `cpsb_addr`: the register is
readable-as-wired but the write still lands, matching MAME's `COMBINE_DATA`
before the read interception. The ID test above pins this — it writes `0xDEAD`
and still reads `0x0401`.

- [ ] **Step 6: Run — all `machine` tests pass**

- [ ] **Step 7: Mutate — this is the task with the most to get wrong**

| Mutant | Must kill |
|---|---|
| DSW: `(byte << 8) \| 0xFF` → `0xFF00 \| byte` | `dsw_reads_put_the_selected_bank...` |
| DSW: `sel` → `sel + 1` (drop the IN0 case) | same |
| CPS-B: return `cps_b[...]` unconditionally | `the_cpsb_id_register_reads_its_wired_value...` |
| CPS-B: `in2_addr` arm removed | `in2_is_read_through_cps_b_at_in2_addr` |
| CPS-A: index by `(addr - 0x800100)` without `>> 1` | `cps_a_writes_land_in_the_register_file_by_word_index` |
| `in1`: swap the P1/P2 halves | `in1_carries_p1_in_the_low_byte...` |
| `Inputs::idle`: `dsw: [0x00; 3]` | `an_idle_board_reads_all_ones...` |
| active-low: `v \|= 1 << bit` instead of `&= !` | `in1_carries_p1...` |
| unmapped read → `0x0000` | `an_unmapped_read_is_all_ones...` |

- [ ] **Step 8: Commit**

---

### Task 7: `Timing`, the frame schedule, and the derived constants

**Files:**
- Create: `crates/machine/src/timing.rs`, `crates/machine/src/cps1.rs`
- Modify: `crates/machine/src/lib.rs`

**Interfaces:**
- Produces: `Timing`, `Cps1::{new, reset, run_scanline, run_frame}`.

- [ ] **Step 1: Write `timing.rs` with the derivation test first**

```rust
//! Video and CPU timing.
//!
//! Primitives from MAME `cps1.h:39-47`; everything else here is derived from
//! them and **checked against a hand-written literal** in the tests below.

/// `CPS_PIXEL_CLOCK` — `XTAL(16'000'000)/2`, `cps1.h:39`.
pub const PIXEL_CLOCK: u32 = 8_000_000;
/// `cps1.h:41-43`.
pub const HTOTAL: u32 = 512;
pub const HBEND: u32 = 64;
pub const HBSTART: u32 = 448;
/// `cps1.h:45-47`.
pub const VTOTAL: u32 = 262;
pub const VBEND: u32 = 16;
pub const VBSTART: u32 = 240;

/// The 68000's clock: `XTAL(10'000'000)`, "verified on pcb", `cps1.cpp:3912`.
/// Some later CPS-1 games run at 12 MHz (`cps1.cpp:3964`), which is why this is
/// a [`Timing`] field and not a constant.
pub const CPU_HZ_10M: u32 = 10_000_000;

/// How the CPU is interleaved with the beam.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub cpu_hz: u32,
    pub cycles_per_line: u32,
    pub lines_per_frame: u32,
    /// The scanline on which IPL1 is asserted. `cps1.cpp:395`.
    pub vblank_line: u32,
}

impl Timing {
    /// The 10 MHz CPS-1 configuration — SF2's.
    ///
    /// # Why the integer division is safe here
    ///
    /// 8 MHz / 512 = 15,625 lines per second exactly, and 10 MHz / 15,625 = 640
    /// cycles per line exactly. **Both divisions are exact for this pair of
    /// clocks**, which removes accumulated fractional error from the scheduler
    /// entirely. The 12 MHz variant is 768, also exact. A board whose clocks did
    /// not divide evenly would need a fractional accumulator here, and the
    /// `cps1_frame_geometry` test is what would catch its absence.
    pub const fn cps1_10mhz() -> Self {
        Self {
            cpu_hz: CPU_HZ_10M,
            cycles_per_line: 640,
            lines_per_frame: VTOTAL,
            vblank_line: VBSTART,
        }
    }

    pub const fn cycles_per_frame(&self) -> u32 {
        self.cycles_per_line * self.lines_per_frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every derived figure against a literal.
    ///
    /// ⚠️ Each right-hand side is written by hand from the arithmetic, not
    /// recomputed from the left. An assertion of the form
    /// `assert_eq!(a / b, a / b)` is the project's characteristic defect: it
    /// passes for every value of `a` and `b`, including wrong ones.
    #[test]
    fn cps1_frame_geometry_is_384x224_at_59_63_hz() {
        assert_eq!(HBSTART - HBEND, 384, "visible width");
        assert_eq!(VBSTART - VBEND, 224, "visible height");
        assert_eq!(PIXEL_CLOCK / HTOTAL, 15_625, "lines per second, exact");
        assert_eq!(PIXEL_CLOCK % HTOTAL, 0, "and the division has no remainder");
        assert_eq!(CPU_HZ_10M / (PIXEL_CLOCK / HTOTAL), 640, "CPU cycles per line");
        assert_eq!(CPU_HZ_10M % (PIXEL_CLOCK / HTOTAL), 0, "also exact");
        assert_eq!(640 * VTOTAL, 167_680, "CPU cycles per frame");
        // 8_000_000 / (512 * 262) = 59.6374...; assert the milli-hertz so the
        // figure is pinned without a float comparison.
        assert_eq!(PIXEL_CLOCK * 1000 / (HTOTAL * VTOTAL), 59_637);
    }

    #[test]
    fn the_default_timing_matches_the_derivation() {
        let t = Timing::cps1_10mhz();
        assert_eq!(t.cycles_per_line, CPU_HZ_10M / (PIXEL_CLOCK / HTOTAL));
        assert_eq!(t.cycles_per_frame(), 167_680);
        assert_eq!(t.vblank_line, 240);
    }

    #[test]
    fn vblank_is_inside_the_frame_and_after_the_visible_area() {
        let t = Timing::cps1_10mhz();
        assert!(t.vblank_line < t.lines_per_frame);
        assert_eq!(t.vblank_line, VBSTART, "the beam leaves the visible area");
    }
}
```

- [ ] **Step 2: Run — 3 pass. Mutate `HTOTAL` to 511: `cps1_frame_geometry` must go red on the exactness assertion. Mutate `cycles_per_line` to 639: `the_default_timing_matches_the_derivation` must go red.**

This second mutant is the one that matters: it proves `cps1_10mhz()`'s hard-coded 640 is checked against the derivation rather than merely asserted to equal itself.

- [ ] **Step 3: Write `cps1.rs`'s failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A program that never stops: `bra.s -2` at 0x1000.
    /// Verified against the disassembler: `0x60FE` at 0x1000 renders `bra $1000`.
    fn spin() -> Vec<u8> {
        let mut rom = vec![0u8; 0x2000];
        // Reset vector: SSP 0x00FF8000, PC 0x00001000.
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x1000..0x1002].copy_from_slice(&[0x60, 0xFE]);
        rom
    }

    #[test]
    fn a_scanline_runs_at_least_its_budget_of_cycles() {
        let mut m = Cps1::new(&spin(), BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        let ran = m.run_scanline();
        assert!(ran >= 640, "a scanline must run its full budget, ran {ran}");
        // `bra.s` is 10 cycles, so the overshoot is bounded by one instruction.
        assert!(ran < 640 + 40, "and must not overrun by more than an instruction: {ran}");
    }

    #[test]
    fn a_frame_of_a_spin_loop_costs_167680_cycles_plus_one_instruction() {
        let mut m = Cps1::new(&spin(), BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        let before = m.total_cycles;
        m.run_frame();
        let spent = m.total_cycles - before;
        assert!(
            (167_680..167_680 + 40).contains(&spent),
            "a frame is 167,680 cycles plus at most one instruction's overshoot, got {spent}"
        );
    }

    #[test]
    fn ten_frames_do_not_drift() {
        // The carry is the point: ten frames of a 10-cycle instruction against a
        // 640-cycle budget must not accumulate 10 x the per-frame overshoot.
        let mut m = Cps1::new(&spin(), BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        for _ in 0..10 {
            m.run_frame();
        }
        assert!(
            (1_676_800..1_676_800 + 40).contains(&m.total_cycles),
            "ten frames is 1,676,800 plus one overshoot, not ten. got {}",
            m.total_cycles
        );
    }
}
```

The third test is the one the carry exists for, and it fails loudly if `carry` is dropped.

- [ ] **Step 4: Run — fails, `Cps1` not found**

- [ ] **Step 5: Implement `cps1.rs`**

```rust
//! The machine: a CPU, a board, and the schedule that interleaves them.

use crate::board::Board;
use crate::config::BoardConfig;
use crate::timing::Timing;
use m68k::{decode::Decoder, M68k};

pub struct Cps1 {
    pub cpu: M68k,
    pub board: Board,
    pub timing: Timing,
    /// Total 68000 cycles since construction. `u64` because 167,680 per frame at
    /// 60 Hz overflows `u32` in under twelve minutes.
    pub total_cycles: u64,
    /// The current scanline, 0..lines_per_frame.
    pub line: u32,
    /// How far the last instruction overran its scanline budget, as a negative
    /// number carried into the next line.
    ///
    /// The 68000 cannot be stopped mid-instruction — a `DIVS` costs 158 cycles
    /// and does not divide at a scanline boundary — so overshoot is inherent.
    /// Carrying it forward means the *only* error is the current line's
    /// overshoot, never a sum of them; dropping it would make every scanline
    /// slightly long and the frame rate slightly slow, in a way no single-frame
    /// test would notice.
    carry: i64,
    dec: Decoder,
}

impl Cps1 {
    pub fn new(prog: &[u8], cfg: BoardConfig, timing: Timing) -> Self {
        Self {
            cpu: M68k::new(),
            board: Board::new(prog, cfg),
            timing,
            total_cycles: 0,
            line: 0,
            carry: 0,
            dec: Decoder::new(),
        }
    }

    /// Takes SSP and PC from vectors 0 and 1, as the hardware does on power-up.
    pub fn reset(&mut self) {
        self.cpu.reset(&mut self.board);
        self.total_cycles = 0;
        self.line = 0;
        self.carry = 0;
    }

    /// Runs one scanline's worth of CPU, returning the cycles actually consumed.
    ///
    /// Consumes at least `cycles_per_line + carry`; the excess becomes the next
    /// line's carry.
    pub fn run_scanline(&mut self) -> u32 {
        let mut budget = i64::from(self.timing.cycles_per_line) + self.carry;
        let mut spent = 0u32;
        while budget > 0 {
            let c = self.cpu.step_with(&self.dec, &mut self.board);
            // A halted CPU still burns time (the core returns 4), so this cannot
            // spin: budget always decreases.
            budget -= i64::from(c);
            spent += c;
        }
        self.carry = budget; // <= 0
        self.total_cycles += u64::from(spent);
        self.line = (self.line + 1) % self.timing.lines_per_frame;
        spent
    }

    /// Runs a whole frame.
    pub fn run_frame(&mut self) {
        for _ in 0..self.timing.lines_per_frame {
            self.run_scanline();
        }
    }
}
```

- [ ] **Step 6: Run — 3 pass**

- [ ] **Step 7: Mutate**

| Mutant | Must kill |
|---|---|
| `self.carry = budget` → `self.carry = 0` | `ten_frames_do_not_drift` (and *not* the single-frame test — confirm that, it is the evidence the third test was needed) |
| `while budget > 0` → `while budget > 40` | `a_scanline_runs_at_least_its_budget` |
| `run_frame`'s loop bound → `lines_per_frame - 1` | `a_frame_of_a_spin_loop_costs...` |

- [ ] **Step 8: Commit**

---

### Task 8: The vblank interrupt and the vector-fetch acknowledge

The task with the one real design subtlety. Read the spec's §The interrupt-acknowledge problem before starting.

**Files:**
- Modify: `crates/machine/src/cps1.rs`, `crates/machine/src/board.rs`
- Create: `crates/machine/tests/programs.rs`

**Interfaces:**
- Produces: `Board::{vblank_irq_pending, assert_vblank, ack_seen}`, `Cps1`'s per-line assertion.

- [ ] **Step 1: Understand the mechanism before writing it**

On CPS-1, IPL1 is cleared **by the 68000's own autovector fetch**: the CPU drives FC=7 with an address in `0xFFFFF2..0xFFFFFF` and the board decodes that to drop the line (`cps1.cpp:407-422`). `m68k::Bus` has no function code, so the board cannot see an acknowledge cycle directly.

What the board *can* see: a `read16` of the **vector 26 longword** — autovector level 2 is vector `24 + 2 = 26`, at address `26 * 4 = 0x68`, so the two halves are at `0x68` and `0x6A`. On this board, with an assertion outstanding, that read is unambiguously the acknowledge: the vector table is in ROM and no game reads its own vector 26 as data. The bound is stated, not hidden.

- [ ] **Step 2: Write the failing integration tests**

`crates/machine/tests/programs.rs`. These are hand-assembled 68000 programs; **every encoding below was verified against `m68k`'s disassembler on 2026-08-07** and the rendering is quoted beside it.

```rust
//! Hand-assembled 68000 programs run against the CPS-1 board.
//!
//! # What these replace
//!
//! Sub-project A had 317,500 external vector cases as its oracle. Sub-project B
//! has none: there is no public test suite for a Capcom board. These programs
//! are the standin, and this comment is explicit that they are a weaker one —
//! they cover the paths we thought of, no more.
//!
//! What they *do* guarantee is that no expectation is self-consistent with the
//! code under test: each program's expected outcome is a number written by hand
//! from the 68000 manual and the memory map, and each is mutation-checked.
//!
//! Encodings verified against `m68k::disasm` on 2026-08-07; the disassembly is
//! quoted beside each word.

use machine::{BoardConfig, Cps1, Timing};

/// Builds a ROM image: reset vector, an optional level-2 handler vector, the
/// program at 0x1000, and any extra blocks.
fn rom(prog: &[u16], vec2: Option<u32>, extra: &[(usize, &[u16])]) -> Vec<u8> {
    let mut r = vec![0u8; 0x4000];
    let put = |r: &mut Vec<u8>, at: usize, words: &[u16]| {
        for (i, w) in words.iter().enumerate() {
            let [h, l] = w.to_be_bytes();
            r[at + 2 * i] = h;
            r[at + 2 * i + 1] = l;
        }
    };
    // SSP = 0x00FF8000 (top of main RAM), PC = 0x00001000.
    put(&mut r, 0, &[0x0000, 0xFF80, 0x0000, 0x1000]);
    if let Some(h) = vec2 {
        // Autovector level 2 = vector 26, at 26 * 4 = 0x68.
        put(&mut r, 0x68, &[(h >> 16) as u16, h as u16]);
    }
    put(&mut r, 0x1000, prog);
    for (at, words) in extra {
        put(&mut r, *at, words);
    }
    r
}

fn machine(rom: &[u8]) -> Cps1 {
    let mut m = Cps1::new(rom, BoardConfig::sf2(), Timing::cps1_10mhz());
    m.reset();
    m
}

/// The vblank counter: the shape of every CPS-1 game's main loop.
///
/// ```text
/// 1000  46FC 2000   move #$2000,sr      supervisor, mask 0
/// 1004  60FE        bra   $1004          spin forever
///
/// 2000  5279 00FF 0000   addq.w #1,$FF0000
/// 2006  4E73             rte
/// ```
///
/// Expected: **exactly one increment per frame**. Not zero (the IRQ was never
/// recognised), not many (the line was never acknowledged and the handler
/// re-entered until the stack wrapped).
#[test]
fn vblank_increments_a_counter_once_per_frame() {
    let r = rom(
        &[0x46FC, 0x2000, 0x60FE],
        Some(0x2000),
        &[(0x2000, &[0x5279, 0x00FF, 0x0000, 0x4E73])],
    );
    let mut m = machine(&r);
    for want in 1..=3u16 {
        m.run_frame();
        assert_eq!(
            m.board.ram[0], want,
            "frame {want}: the handler must run exactly once per frame"
        );
    }
}

/// The acknowledge is what makes the count 1 rather than hundreds.
///
/// ⚠️ This asserts the **observable artifact** — the handler's own increment —
/// and deliberately not `board.vblank_irq_pending`. A test that reads the flag
/// the code sets passes a half-done fix; this project has produced that exact
/// defect before (`docs/hardware/68000-notes.md`).
#[test]
fn without_the_acknowledge_the_handler_would_re_enter_and_this_proves_it_does_not() {
    let r = rom(
        &[0x46FC, 0x2000, 0x60FE],
        Some(0x2000),
        &[(0x2000, &[0x5279, 0x00FF, 0x0000, 0x4E73])],
    );
    let mut m = machine(&r);
    m.run_frame();
    // One frame is 167,680 cycles; the handler is ~90. An unacknowledged level-2
    // line would re-enter on the order of a thousand times before the frame ended
    // (and the SR mask only blocks it *during* the handler, not after the rte).
    assert_eq!(m.board.ram[0], 1);
    assert!(m.board.ram[0] < 2, "re-entry would show here as a count above 1");
}

/// `STOP` parks the CPU; the vblank must wake it. Zero vector cases cover this
/// path — STOP's access shape is empty and no vector case runs a second step.
///
/// ```text
/// 1000  46FC 2000   move #$2000,sr
/// 1004  4E72 2000   stop  #$2000        stopped, mask 0, supervisor
/// 1008  5279 ...    addq.w #1,$FF0002   reached only after the handler returns
/// 100E  60FE        bra   $100E
/// ```
#[test]
fn a_stopped_cpu_is_woken_by_the_vblank_interrupt() {
    let r = rom(
        &[0x46FC, 0x2000, 0x4E72, 0x2000, 0x5279, 0x00FF, 0x0002, 0x60FE],
        Some(0x2000),
        &[(0x2000, &[0x5279, 0x00FF, 0x0000, 0x4E73])],
    );
    let mut m = machine(&r);
    m.run_frame();
    assert_eq!(m.board.ram[0], 1, "the handler ran");
    assert_eq!(m.board.ram[1], 1, "and execution resumed past the STOP");
    assert!(!m.cpu.stopped, "the CPU is running again");
}

/// SF2's actual boot self-test, in miniature: read the CPSB ID and branch.
///
/// ```text
/// 1000  3039 0080 0172   move.w $800172,d0
/// 1006  0C40 0401        cmpi.w #$0401,d0
/// 100A  6606             bne    $1012          -> the failure marker
/// 100C  33FC 00A5 00FF 0000   move.w #$00A5,$FF0000   pass
/// 1014  ...
/// ```
///
/// Rather than hand-compute the `bne` displacement, use two absolute writes and
/// a `bra`: encode as
/// ```text
/// 1000  3039 0080 0172   move.w $800172,d0
/// 1006  0C40 0401        cmpi.w #$0401,d0
/// 100A  6608             bne.s  +8  -> 0x1014
/// 100C  33FC 00A5 00FF 0000   move.w #$A5,$FF0000
/// 1014  4E72 2000        stop   #$2000        (both paths land here)
/// ```
/// The `bne.s` displacement byte is relative to the instruction's own address
/// + 2 = 0x100C, so `+8` targets 0x1014.
#[test]
fn the_cpsb_id_check_takes_the_pass_branch() {
    let r = rom(
        &[
            0x3039, 0x0080, 0x0172, // move.w $800172,d0
            0x0C40, 0x0401,         // cmpi.w #$0401,d0
            0x6608,                 // bne.s  -> 0x1014
            0x33FC, 0x00A5, 0x00FF, 0x0000, // move.w #$A5,$FF0000
            0x4E72, 0x2000,         // stop
        ],
        None,
        &[],
    );
    let mut m = machine(&r);
    m.run_frame();
    assert_eq!(
        m.board.ram[0], 0x00A5,
        "the board must answer 0x800172 with 0x0401, so the branch is not taken"
    );
}

/// The negative control for the test above, and the reason it is not vacuous:
/// with the ID register wrong, the same program must **fail**.
#[test]
fn the_cpsb_id_check_fails_when_the_board_answers_wrongly() {
    let r = rom(
        &[
            0x3039, 0x0080, 0x0172, 0x0C40, 0x0401, 0x6608,
            0x33FC, 0x00A5, 0x00FF, 0x0000, 0x4E72, 0x2000,
        ],
        None,
        &[],
    );
    let wrong = BoardConfig { cpsb_value: 0x0000, ..BoardConfig::sf2() };
    let mut m = Cps1::new(&r, wrong, Timing::cps1_10mhz());
    m.reset();
    m.run_frame();
    assert_eq!(m.board.ram[0], 0x0000, "the pass branch must be skipped");
}

/// gfxram is byte-addressable and big-endian, and distinct from main RAM.
///
/// ```text
/// 1000  33FC 1234 0090 0000   move.w #$1234,$900000
/// 1008  1639 0090 0000        move.b $900000,d3    -> 0x12
/// 100E  13C3 00FF 0000        move.b d3,$FF0000
/// 1014  4E72 2000             stop
/// ```
#[test]
fn gfxram_word_writes_are_readable_as_big_endian_bytes() {
    let r = rom(
        &[
            0x33FC, 0x1234, 0x0090, 0x0000,
            0x1639, 0x0090, 0x0000,
            0x13C3, 0x00FF, 0x0000,
            0x4E72, 0x2000,
        ],
        None,
        &[],
    );
    let mut m = machine(&r);
    m.run_frame();
    assert_eq!(m.board.gfxram[0], 0x1234);
    assert_eq!(m.board.ram[0] >> 8, 0x12, "the high byte of the word is at the even address");
}

/// An idle board reads all ones through the DIP-switch port.
///
/// ```text
/// 1000  3239 0080 001A   move.w $80001A,d1     DSWA
/// 1006  33C1 00FF 0000   move.w d1,$FF0000
/// 100C  4E72 2000        stop
/// ```
#[test]
fn an_unpressed_board_reads_all_ones_through_the_dip_port() {
    let r = rom(
        &[0x3239, 0x0080, 0x001A, 0x33C1, 0x00FF, 0x0000, 0x4E72, 0x2000],
        None,
        &[],
    );
    let mut m = machine(&r);
    m.run_frame();
    assert_eq!(m.board.ram[0], 0xFFFF, "active low, every switch off");
}
```

- [ ] **Step 3: Run — fails; the vblank tests get 0 increments**

- [ ] **Step 4: Add the interrupt to `Board`**

```rust
/// Set while IPL1 is asserted and the 68000 has not yet fetched its vector.
///
/// # Why this is not a public deassertion API
///
/// On hardware the line is cleared by the CPU's own autovector fetch: FC=7 with
/// an address in 0xFFFFF2..0xFFFFFF, decoded by the board (`cps1.cpp:407-422`).
/// [`m68k::Bus`] carries no function code, so that cycle is invisible here — an
/// autovector fetch of vector 26 and a `MOVE.L $68,D0` are the same two reads.
///
/// So the acknowledge is detected as a **read of the vector-26 longword**
/// (0x68/0x6A) while an assertion is outstanding. On this board that is exact:
/// the vector table is in ROM and no game reads its own vector 26 as data. If
/// one did, the read would return the same value either way — only the
/// deassertion would be early.
///
/// The alternative considered and rejected was deasserting a scanline later,
/// which is wrong in a way that hides: too slow a handler misses the next
/// assertion, too fast a one takes the same interrupt twice. Widening `Bus` with
/// a function code is the correct fix and is deferred — it would break the trait
/// 317,500 verified vector cases run through, for one bit one board needs.
vblank_pending: bool,
```

In `Board`:

```rust
/// Asserts IPL1, as the beam reaching line 240 does.
pub fn assert_vblank(&mut self) {
    self.vblank_pending = true;
}

pub fn vblank_pending(&self) -> bool {
    self.vblank_pending
}
```

In `read_word`'s ROM arm, before returning, detect the fetch:

```rust
0x00_0000..=0x3F_FFFF => {
    // The autovector-26 fetch is the acknowledge cycle; see `vblank_pending`.
    if self.vblank_pending && (addr & !3) == 0x68 {
        self.vblank_pending = false;
        self.trace.acks += 1;
    }
    let i = (addr & !1) as usize;
    Some(u16::from_be_bytes([self.rom[i], self.rom[i + 1]]))
}
```

In `Cps1::run_scanline`, before the budget loop:

```rust
// Vblank: IPL1 at line 240 (`cps1.cpp:395-396`). CPS-1 wires the IPL pins
// individually (`set_interrupt_mixer(false)`, `cps1.cpp:3913`), so IPL1 is
// level 2 and IPL2 is level 4 — not an encoded priority.
if self.line == self.timing.vblank_line {
    self.board.assert_vblank();
}
// The core samples a *level*, and the board owns deassertion, so re-drive the
// level every scanline from the board's own state rather than calling set_irq
// once: that keeps exactly one source of truth for "is the line asserted".
self.cpu.set_irq(if self.board.vblank_pending() { 2 } else { 0 });
```

- [ ] **Step 5: Run — all 7 programs pass**

- [ ] **Step 6: Mutate — every one of these must be watched**

| Mutant | Must kill |
|---|---|
| never clear `vblank_pending` on the fetch | `vblank_increments_a_counter_once_per_frame` (count >> 1) |
| clear it unconditionally in `assert_vblank`'s next line | the same test (count 0) |
| `(addr & !3) == 0x68` → `== 0x60` | the count test |
| `set_irq(2)` → `set_irq(1)` | the count test (vector 25, no handler) |
| `line == vblank_line` → `line == 0` | nothing — **both fire once per frame.** Note this: the current tests do not discriminate *which* line vblank falls on. Add an assertion that captures `m.line` inside the handler, or that `board.ram` is untouched for the first 240 lines' worth of `run_scanline` calls. Then re-run the mutant and watch it die. |
| `cpsb_value` intercept removed | `the_cpsb_id_check_takes_the_pass_branch` |

That penultimate row is the task's real finding: a per-frame count cannot see a per-line phase error. Fix the test before committing.

- [ ] **Step 7: Commit**

---

### Task 9: `Trace`, the unmapped map, the binary, and the opt-in boot test

**Files:**
- Create: `crates/machine/src/trace.rs`, `crates/machine/tests/boot.rs`, `crates/sfemu/Cargo.toml`, `crates/sfemu/src/main.rs`
- Modify: `Cargo.toml`, `crates/machine/src/{lib,board,cps1}.rs`

- [ ] **Step 1: Write `trace.rs`**

```rust
//! What the board saw.
//!
//! Sub-project B renders nothing, so this is its entire observable surface — and
//! it is a better instrument for the question B actually answers ("is the boot
//! code progressing?") than a black window would be. "Does SF2 boot?" becomes
//! checkable: after N frames, is `vblanks == N`, did `cps_a_writes` happen, did
//! the game ask the Z80 for the attract music, and are the sampled PCs inside
//! populated ROM rather than looping in an exception handler?

use core::cmp::Ordering;

/// Per-address counter for accesses the board does not decode.
///
/// A `Vec` of pairs kept sorted rather than a `BTreeMap`, so `machine` needs no
/// `alloc`-flavoured collection beyond `Vec` and stays `no_std`-able later.
#[derive(Debug, Default, Clone)]
pub struct UnmappedLog(Vec<(u32, u64)>);

impl UnmappedLog {
    pub fn record(&mut self, addr: u32) {
        match self.0.binary_search_by(|(a, _)| a.cmp(&addr)) {
            Ok(i) => self.0[i].1 += 1,
            Err(i) => self.0.insert(i, (addr, 1)),
        }
    }

    pub fn entries(&self) -> &[(u32, u64)] {
        &self.0
    }

    pub fn total(&self) -> u64 {
        self.0.iter().map(|(_, n)| n).sum()
    }

    /// The addresses with the highest counts, worst first.
    ///
    /// A boot that stalls with 40,000 unmapped writes to one address has just
    /// named the chip that is missing.
    pub fn worst(&self, n: usize) -> Vec<(u32, u64)> {
        let mut v = self.0.clone();
        v.sort_by(|a, b| match b.1.cmp(&a.1) {
            Ordering::Equal => a.0.cmp(&b.0),
            other => other,
        });
        v.truncate(n);
        v
    }
}

#[derive(Debug, Default, Clone)]
pub struct Trace {
    pub frames: u64,
    pub vblanks: u64,
    /// Autovector-26 fetches — the interrupt acknowledges.
    pub acks: u64,
    pub cps_a_writes: u64,
    pub cps_b_writes: u64,
    pub gfxram_writes: u64,
    pub sound_latch_writes: u64,
    pub rom_writes: u64,
    pub unmapped_reads: UnmappedLog,
    pub unmapped_writes: UnmappedLog,
    /// One PC per scanline, capped so a long run does not grow without bound.
    pub pc_samples: Vec<u32>,
    pub pc_sample_cap: usize,
}
```

- [ ] **Step 2: Wire the counters into `Board`'s arms and `Cps1`'s loop, and write tests**

```rust
#[test]
fn the_trace_counts_what_the_program_actually_did() {
    // A program that writes one CPS-A register, one gfxram word, one sound
    // latch byte, and touches one unmapped address. Every count is a literal.
    // 1000  33FC 0040 0080 010C   move.w #$40,$80010C
    // 1008  33FC 1234 0090 0000   move.w #$1234,$900000
    // 1010  33FC 00AB 0080 0180   move.w #$AB,$800180
    // 1018  33FC FFFF 0081 0000   move.w #-1,$810000     <- unmapped
    // 1020  4E72 2000             stop
    let r = rom(&[
        0x33FC, 0x0040, 0x0080, 0x010C,
        0x33FC, 0x1234, 0x0090, 0x0000,
        0x33FC, 0x00AB, 0x0080, 0x0180,
        0x33FC, 0xFFFF, 0x0081, 0x0000,
        0x4E72, 0x2000,
    ], None, &[]);
    let mut m = machine(&r);
    m.run_frame();
    let t = &m.board.trace;
    assert_eq!(t.cps_a_writes, 1);
    assert_eq!(t.gfxram_writes, 1);
    assert_eq!(t.sound_latch_writes, 1);
    assert_eq!(t.unmapped_writes.total(), 1);
    assert_eq!(t.unmapped_writes.entries(), &[(0x81_0000, 1)]);
    assert_eq!(t.frames, 1);
    assert_eq!(t.vblanks, 1);
}

#[test]
fn pc_samples_are_capped_rather_than_growing_without_bound() {
    let r = rom(&[0x60FE], None, &[]);
    let mut m = machine(&r);
    m.board.trace.pc_sample_cap = 100;
    for _ in 0..10 {
        m.run_frame(); // 2,620 scanlines
    }
    assert_eq!(m.board.trace.pc_samples.len(), 100);
}
```

- [ ] **Step 3: Write the `sfemu` binary**

```rust
//! Run a CPS-1 ROM set and report what the board saw.
//!
//! ```text
//! sfemu <path-to-rom-set> [frames]
//! ```
//!
//! `<path-to-rom-set>` is a MAME-format zip or a directory of loose files that
//! **you supply**. This program contains no ROM data and no way to obtain any.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: sfemu <path-to-sf2.zip-or-directory> [frames]");
        eprintln!();
        eprintln!("The ROM set is yours to supply: this program neither bundles nor");
        eprintln!("downloads one. Legal sources include Capcom Arcade Stadium, Capcom");
        eprintln!("Fighting Collection, or a board you own and dumped.");
        return ExitCode::from(2);
    };
    let frames: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);

    let set = match romset::load(&romset::games::SF2, std::path::Path::new(&path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let prog = set.region("maincpu").expect("the spec guarantees this region");
    let mut m = machine::Cps1::new(
        prog,
        machine::BoardConfig::sf2(),
        machine::Timing::cps1_10mhz(),
    );
    m.reset();
    for _ in 0..frames {
        m.run_frame();
    }
    let t = &m.board.trace;
    println!("frames        {}", t.frames);
    println!("vblanks       {}  acks {}", t.vblanks, t.acks);
    println!("cycles        {}", m.total_cycles);
    println!("cps-a writes  {}", t.cps_a_writes);
    println!("cps-b writes  {}", t.cps_b_writes);
    println!("gfxram writes {}", t.gfxram_writes);
    println!("sound latch   {}", t.sound_latch_writes);
    println!("rom writes    {}", t.rom_writes);
    println!("unmapped      {} reads, {} writes", t.unmapped_reads.total(), t.unmapped_writes.total());
    for (a, n) in t.unmapped_writes.worst(8) {
        println!("  W {a:#08x}  {n}");
    }
    for (a, n) in t.unmapped_reads.worst(8) {
        println!("  R {a:#08x}  {n}");
    }
    ExitCode::SUCCESS
}
```

- [ ] **Step 4: Write the opt-in boot test**

```rust
//! The one test that needs a real ROM set, and the one `#[ignore]` in this
//! project.
//!
//! # Why this is `#[ignore]`d when the rest of the project forbids it
//!
//! The project rule is that missing test data **fails loudly**, naming the file
//! and the command that fetches it — no environment-variable escape hatch. That
//! rule exists because sub-project A's test data is legally fetchable and there
//! *is* a command to name.
//!
//! This data is not. SF2 is commercial Capcom code; there is no command we may
//! put in a failure message. A test that hard-fails on a machine which legally
//! cannot hold the file is a broken test, not a strict one. So it skips by
//! default, and CI's not running it is honest rather than hidden.
//!
//! ```text
//! SFEMU_ROMS=/path/to/sf2.zip cargo test -p machine --test boot -- --ignored
//! ```

#[test]
#[ignore = "needs a user-supplied ROM set; set SFEMU_ROMS"]
fn sf2_boots_for_sixty_frames_without_wandering_off_the_map() {
    let Ok(path) = std::env::var("SFEMU_ROMS") else {
        panic!("set SFEMU_ROMS to your own sf2.zip or a directory of loose files");
    };
    let set = romset::load(&romset::games::SF2, std::path::Path::new(&path)).expect("load");
    let prog = set.region("maincpu").unwrap();
    let mut m = machine::Cps1::new(prog, machine::BoardConfig::sf2(), machine::Timing::cps1_10mhz());
    m.reset();
    m.board.trace.pc_sample_cap = 4096;
    for _ in 0..60 {
        m.run_frame();
    }
    let t = &m.board.trace;
    assert_eq!(t.frames, 60);
    assert_eq!(t.vblanks, 60, "one vblank per frame");
    assert!(t.acks >= 60, "every vblank must be acknowledged: {} acks", t.acks);
    assert!(t.cps_a_writes > 0, "the game must program the video registers");
    assert!(t.gfxram_writes > 0, "and write tilemap or palette data");
    assert!(!m.cpu.halted, "a double bus fault means the map is wrong");
    for &pc in &t.pc_samples {
        assert!(
            pc < 0x10_0000 || (0x90_0000..=0x92_FFFF).contains(&pc) || pc >= 0xFF_0000,
            "PC {pc:#08x} is outside populated ROM, gfxram, or RAM — \
             the program has jumped somewhere the map does not answer"
        );
    }
}
```

Note the PC range allows gfxram: `cps1.cpp:592` records that SF2CE executes code from there, so excluding it would be wrong for a near neighbour of this set.

- [ ] **Step 5: Run the default suite (the boot test skips), then commit**

---

### Task 10: CPS-1 hardware notes

**Files:**
- Create: `docs/hardware/cps1-notes.md`

- [ ] **Step 1: Write the notes**

Same discipline as `68000-notes.md`. Required content:

1. **Clocks and derived timing**, each figure with its derivation and the fact that both divisions are exact.
2. **The memory map**, with the MAME line for each range.
3. **CPS-A and CPS-B**, including the byte-offset/word-index rule and why `cpsb_addr`/`in2_addr` are boot-critical.
4. **The interrupt acknowledge**: the hardware mechanism, the three options, why option 3 was chosen, and the exact bound on its imprecision. State plainly that `Bus` carrying no function code is a known limitation with a measured shape (4 FC values over 1,450,409 non-idle vector transactions) and name the condition under which it must be fixed.
5. **What the suite cannot see, restated for B**: there is no vector suite for a Capcom board. List each thing the synthetic programs *do* pin and — more importantly — what they do not. Include the two findings mutation testing produced in Tasks 2 and 8 (the EOCD scan not being discriminated until a comment case was added; a per-frame vblank count not discriminating which line vblank falls on).
6. **The ROM interleave**, with the byte-swap failure mode spelled out.

- [ ] **Step 2: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps --workspace
cargo test --workspace
cargo test --workspace --release
```

Confirm sub-project A is untouched: **127/127 groups, 317,500/317,500 cases, 221 m68k + 18 harness + 128 suite tests**, and `git diff --stat HEAD~N -- crates/m68k` is empty for the whole of B.

- [ ] **Step 3: Commit**

---

## Self-Review

**Spec coverage.** Every §Verification item maps to a task: derived constants → 7; synthetic programs → 8 (five programs, plus two more in 6 and 9); loader interleaves → 1; CRC both directions → 3; the opt-in real-ROM test → 9; MAME cross-checks → 4 and 7. Every §Architecture component maps: `romset` → 1-4, `machine` → 5-8, `Trace`/`sfemu` → 9, notes → 10. The interrupt-acknowledge decision is implemented in 8 and documented in 10.

**Placeholders.** None left in code blocks. Two mutants are noted as *not currently killed* — Task 2's EOCD backward scan and Task 8's vblank line phase — and each names the test to add before its task commits. They are in the plan on purpose: mutation testing that never finds a gap is not being run, and predicting where the gaps will be is cheaper than discovering them.

**Type consistency.** `Board::new(&[u8], BoardConfig)` from Task 6 onward — Task 5 introduces the one-argument form and Task 6's step 5 states the update explicitly. `Cps1::new(&[u8], BoardConfig, Timing)` throughout. `place(dest, src, entry, region)` in Tasks 1 and 3. `RomError` variants are used in Tasks 1 and 3 exactly as declared. `Trace` fields are written in Task 9 and referenced only there and in the boot test. `romset::load` returns `RomSet` with `region(&str) -> Option<&[u8]>`, used in Task 9's binary and boot test.

**One risk worth naming.** Task 8's programs assume the reset vector's SSP (`0x00FF8000`) and the level-2 handler at `0x2000` behave as A's core documents. If the vblank tests show zero increments with the acknowledge logic clearly correct, suspect the handler *vector*, not the interrupt path: vector 26 lives at `0x68`, and A's `check_interrupts` computes it as `VEC_AUTOVECTOR_BASE (24) + level`. That is a two-minute check and it is the first thing to make.
