//! Static descriptions of MAME ROM sets: what files a set contains, where each
//! one lands, and what it must checksum to.
//!
//! ⚠️ These tables hold **file names, offsets, lengths, and CRCs — never ROM
//! data.** SF1 and SF2 are commercial Capcom code and this repository neither
//! bundles nor fetches it. A table of names and checksums is metadata about a
//! product, the same category as a package manifest, and it is what makes "the
//! user supplies the file" a checkable claim rather than a hope.

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
    /// `ROM_LOAD16_WORD_SWAP`: byte `i` lands at `offset + i`, but the two bytes
    /// of every 16-bit word are exchanged — source `i` and `i+1` land at `i+1`
    /// and `i` for even `i`.
    ///
    /// This is not [`Word16Byte`](LoadKind::Word16Byte) with one file: it is a
    /// whole 16-bit image in one file, at native width, byte-swapped. Champion
    /// Edition's three 512 KB program ROMs use it where World Warrior's eight
    /// 128 KB ones use `ROM_LOAD16_BYTE` pairs.
    ///
    /// ⚠️ A wrong choice here is **not** self-announcing. Both orderings of CE's
    /// first program ROM yield a vector table that looks reasonable — reset PC
    /// 0x3602 unswapped, 0x0236 swapped — and only disassembly separates them.
    /// `assemble.rs`'s test for this kind records the evidence.
    Word16WordSwap,
    /// `ROM_LOAD64_WORD`: source word `i` (2 bytes) lands at `offset + 8*i`.
    Word64Word,
    /// `ROM_CONTINUE`: the first `split` bytes land at `offset`, the remainder at
    /// `cont_at`.
    Continue {
        /// How many bytes go at `offset` before the jump.
        split: usize,
        /// Where the remainder resumes.
        cont_at: usize,
    },
}

/// One file in a ROM set.
#[derive(Debug, Clone, Copy)]
pub struct RomEntry {
    /// The file's name inside the archive or directory.
    pub name: &'static str,
    /// Where this entry's first byte lands in its region.
    pub offset: usize,
    /// Length of the **source file**, not of the span it occupies in the region.
    /// For an interleaved kind the span is a multiple of this.
    pub len: usize,
    /// The CRC-32 the file must have.
    pub crc32: u32,
    /// How the bytes are distributed.
    pub load: LoadKind,
}

/// One region: a contiguous address space the board presents to a chip.
#[derive(Debug, Clone, Copy)]
pub struct RegionSpec {
    /// MAME's region tag, e.g. `"maincpu"`.
    pub name: &'static str,
    /// The region's full size in bytes; space no entry populates reads as zero.
    pub size: usize,
    /// The files that populate it.
    pub entries: &'static [RomEntry],
}

/// One supported ROM set.
#[derive(Debug, Clone, Copy)]
pub struct GameSpec {
    /// MAME's set name, e.g. `"sf2"`.
    pub name: &'static str,
    /// Every region the set defines.
    pub regions: &'static [RegionSpec],
}

impl GameSpec {
    /// The region with this tag, if the set has one.
    pub fn region(&self, name: &str) -> Option<&'static RegionSpec> {
        self.regions.iter().find(|r| r.name == name)
    }
}
