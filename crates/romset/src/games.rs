//! Supported ROM sets.
//!
//! Transcribed from MAME `master`, `src/mame/capcom/cps1.cpp:7101-7133`
//! (BSD-3-Clause, copyright-holders Paul Leaman), read 2026-08-07.
//!
//! ⚠️ Names, offsets, lengths and CRCs only. No ROM data — see [`crate::spec`].
//!
//! The tables carry `#[rustfmt::skip]`: one entry per line, columns aligned. The
//! only way to check a transcription is to read it beside the MAME source, and
//! `rustfmt`'s default expansion to one field per line turns 23 entries into 138
//! lines nobody will diff against anything.

use crate::spec::{GameSpec, LoadKind, RegionSpec, RomEntry};

const W16: LoadKind = LoadKind::Word16Byte;
const W64: LoadKind = LoadKind::Word64Word;
const BYTE: LoadKind = LoadKind::Byte;
const AUDIO_SPLIT: LoadKind = LoadKind::Continue {
    split: 0x0_8000,
    cont_at: 0x1_0000,
};

/// 68000 program: four pairs of 128 KB files, byte-interleaved.
///
/// The **even** offset of each pair supplies the high byte of the big-endian
/// word. 8 × 0x20000 = 1 MB at 0x000000-0x0FFFFF; 0x100000-0x3FFFFF is
/// unpopulated and reads as zero.
#[rustfmt::skip]
static SF2_MAINCPU: &[RomEntry] = &[
    RomEntry { name: "sf2e_30g.11e", offset: 0x0_0000, len: 0x2_0000, crc32: 0xfe39_ee33, load: W16 },
    RomEntry { name: "sf2e_37g.11f", offset: 0x0_0001, len: 0x2_0000, crc32: 0xfb92_cd74, load: W16 },
    RomEntry { name: "sf2e_31g.12e", offset: 0x4_0000, len: 0x2_0000, crc32: 0x69a0_a301, load: W16 },
    RomEntry { name: "sf2e_38g.12f", offset: 0x4_0001, len: 0x2_0000, crc32: 0x5e22_db70, load: W16 },
    RomEntry { name: "sf2e_28g.9e",  offset: 0x8_0000, len: 0x2_0000, crc32: 0x8bf9_f1e5, load: W16 },
    RomEntry { name: "sf2e_35g.9f",  offset: 0x8_0001, len: 0x2_0000, crc32: 0x626e_f934, load: W16 },
    RomEntry { name: "sf2_29b.10e",  offset: 0xc_0000, len: 0x2_0000, crc32: 0xbb4a_f315, load: W16 },
    RomEntry { name: "sf2_36b.10f",  offset: 0xc_0001, len: 0x2_0000, crc32: 0xc02a_13eb, load: W16 },
];

/// Graphics: twelve 512 KB files in three groups of four, 16-bit words strided
/// into a 64-bit layout.
///
/// Sub-project B loads this and decodes nothing; C owns the tile decode.
#[rustfmt::skip]
static SF2_GFX: &[RomEntry] = &[
    RomEntry { name: "sf2-5m.4a",   offset: 0x00_0000, len: 0x8_0000, crc32: 0x22c9_cc8e, load: W64 },
    RomEntry { name: "sf2-7m.6a",   offset: 0x00_0002, len: 0x8_0000, crc32: 0x5721_3be8, load: W64 },
    RomEntry { name: "sf2-1m.3a",   offset: 0x00_0004, len: 0x8_0000, crc32: 0xba52_9b4f, load: W64 },
    RomEntry { name: "sf2-3m.5a",   offset: 0x00_0006, len: 0x8_0000, crc32: 0x4b1b_33a8, load: W64 },
    RomEntry { name: "sf2-6m.4c",   offset: 0x20_0000, len: 0x8_0000, crc32: 0x2c7e_2229, load: W64 },
    RomEntry { name: "sf2-8m.6c",   offset: 0x20_0002, len: 0x8_0000, crc32: 0xb554_8f17, load: W64 },
    RomEntry { name: "sf2-2m.3c",   offset: 0x20_0004, len: 0x8_0000, crc32: 0x14b8_4312, load: W64 },
    RomEntry { name: "sf2-4m.5c",   offset: 0x20_0006, len: 0x8_0000, crc32: 0x5e9c_d89a, load: W64 },
    RomEntry { name: "sf2-13m.4d",  offset: 0x40_0000, len: 0x8_0000, crc32: 0x994b_fa58, load: W64 },
    RomEntry { name: "sf2-15m.6d",  offset: 0x40_0002, len: 0x8_0000, crc32: 0x3e66_ad9d, load: W64 },
    RomEntry { name: "sf2-9m.3d",   offset: 0x40_0004, len: 0x8_0000, crc32: 0xc1be_faa8, load: W64 },
    RomEntry { name: "sf2-11m.5d",  offset: 0x40_0006, len: 0x8_0000, crc32: 0x0627_c831, load: W64 },
];

/// Z80 program: one 64 KB file whose halves land 64 KB apart
/// (`ROM_LOAD` of 0x8000 at 0x00000 + `ROM_CONTINUE` of 0x8000 at 0x10000).
///
/// `len` is the whole file, per [`RomEntry::len`] — MAME's `ROM_LOAD` length
/// field (0x8000) is only the first half, which reads like a transcription error
/// here and is not one.
///
/// Loaded for sub-project D; nothing reads it in B.
#[rustfmt::skip]
static SF2_AUDIOCPU: &[RomEntry] = &[
    RomEntry { name: "sf2_9.12a", offset: 0x0_0000, len: 0x1_0000, crc32: 0xa482_3a1b, load: AUDIO_SPLIT },
];

/// OKI MSM6295 samples: two 128 KB files, concatenated.
#[rustfmt::skip]
static SF2_OKI: &[RomEntry] = &[
    RomEntry { name: "sf2_18.11c", offset: 0x0_0000, len: 0x2_0000, crc32: 0x7f16_2009, load: BYTE },
    RomEntry { name: "sf2_19.12c", offset: 0x2_0000, len: 0x2_0000, crc32: 0xbead_e53f, load: BYTE },
];

#[rustfmt::skip]
static SF2_REGIONS: &[RegionSpec] = &[
    // maincpu is CODE_SIZE (cps1.cpp:4063), four times what the files populate.
    RegionSpec { name: "maincpu",  size: 0x40_0000, entries: SF2_MAINCPU  },
    RegionSpec { name: "gfx",      size: 0x60_0000, entries: SF2_GFX      },
    RegionSpec { name: "audiocpu", size: 0x01_8000, entries: SF2_AUDIOCPU },
    RegionSpec { name: "oki",      size: 0x04_0000, entries: SF2_OKI      },
];

/// Street Fighter II: The World Warrior (World 910214), MAME set `sf2`.
pub static SF2: GameSpec = GameSpec {
    name: "sf2",
    regions: SF2_REGIONS,
};

/// Every set this crate knows.
pub static ALL: &[&GameSpec] = &[&SF2];

/// The set with this MAME name, if it is supported.
pub fn by_name(name: &str) -> Option<&'static GameSpec> {
    ALL.iter().copied().find(|g| g.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assemble::end_of;

    /// These tests exist to catch a transcription slip — a wrong offset, a
    /// copy-pasted CRC, a missing file — with **no ROM present**. That is the
    /// whole trick: the table is metadata, so its internal consistency is
    /// checkable without any of the data it describes. Every expected value is a
    /// literal, never derived from the table it checks.
    #[test]
    fn sf2_has_four_regions_with_the_expected_file_counts() {
        assert_eq!(SF2.regions.len(), 4);
        let counts: Vec<usize> = SF2.regions.iter().map(|r| r.entries.len()).collect();
        assert_eq!(counts, vec![8, 12, 1, 2], "maincpu, gfx, audiocpu, oki");
        let names: Vec<&str> = SF2.regions.iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["maincpu", "gfx", "audiocpu", "oki"]);
        assert_eq!(by_name("sf2").map(|g| g.name), Some("sf2"));
        assert!(by_name("sf1").is_none());
    }

    #[test]
    fn every_region_size_is_the_literal_mame_rom_region_size() {
        // Pinned as literals so a region cannot be quietly widened to make a bad
        // offset fit: `every_entry_fits_inside_its_region` would still pass.
        let sizes: Vec<usize> = SF2.regions.iter().map(|r| r.size).collect();
        assert_eq!(sizes, vec![0x40_0000, 0x60_0000, 0x1_8000, 0x4_0000]);
    }

    #[test]
    fn every_entry_fits_inside_its_region() {
        for r in SF2.regions {
            for e in r.entries {
                assert!(
                    end_of(e) <= r.size,
                    "{} ends at {:#x}, region {} is {:#x}",
                    e.name,
                    end_of(e),
                    r.name,
                    r.size
                );
            }
        }
    }

    #[test]
    fn maincpu_populates_exactly_the_first_megabyte() {
        let r = SF2.region("maincpu").unwrap();
        let top = r.entries.iter().map(end_of).max().unwrap();
        assert_eq!(top, 0x10_0000, "8 x 0x20000 interleaved into 1 MB");
        for e in r.entries {
            assert_eq!(e.len, 0x2_0000, "{} is a 128 KB file", e.name);
            assert_eq!(e.load, LoadKind::Word16Byte, "{}", e.name);
        }
    }

    #[test]
    fn maincpu_pairs_alternate_even_and_odd_offsets() {
        // A slip that gives two files of a pair the same parity byte-swaps a
        // quarter of the program, and the symptom is a 68000 executing garbage
        // thousands of instructions later.
        let r = SF2.region("maincpu").unwrap();
        for pair in r.entries.chunks_exact(2) {
            assert_eq!(pair[0].offset % 2, 0, "{} is the high byte", pair[0].name);
            assert_eq!(pair[1].offset % 2, 1, "{} is the low byte", pair[1].name);
            assert_eq!(pair[0].offset + 1, pair[1].offset, "a pair shares a base");
        }
    }

    #[test]
    fn maincpu_pair_bases_are_the_four_128k_word_boundaries() {
        let bases: Vec<usize> = SF2
            .region("maincpu")
            .unwrap()
            .entries
            .chunks_exact(2)
            .map(|p| p[0].offset)
            .collect();
        assert_eq!(bases, vec![0x0_0000, 0x4_0000, 0x8_0000, 0xc_0000]);
    }

    #[test]
    fn no_two_entries_share_a_crc() {
        // Twelve 512 KB gfx files with one copy-pasted CRC is the easiest
        // transcription error to make and the hardest to see by eye.
        let mut seen = std::collections::BTreeSet::new();
        for r in SF2.regions {
            for e in r.entries {
                assert!(
                    seen.insert(e.crc32),
                    "{} duplicates CRC {:08x}",
                    e.name,
                    e.crc32
                );
            }
        }
        assert_eq!(seen.len(), 23, "8 + 12 + 1 + 2 distinct files");
    }

    #[test]
    fn no_two_entries_share_a_name() {
        let mut seen = std::collections::BTreeSet::new();
        for r in SF2.regions {
            for e in r.entries {
                assert!(seen.insert(e.name), "{} appears twice", e.name);
            }
        }
    }

    #[test]
    fn gfx_entries_stride_by_two_within_each_group_of_four() {
        // Four files interleaved into a 64-bit word: word offsets 0, 2, 4, 6
        // within each 2 MB group.
        let r = SF2.region("gfx").unwrap();
        assert_eq!(r.entries.len(), 12);
        let bases: Vec<usize> = r.entries.chunks_exact(4).map(|g| g[0].offset).collect();
        assert_eq!(bases, vec![0x00_0000, 0x20_0000, 0x40_0000]);
        for group in r.entries.chunks_exact(4) {
            for (i, e) in group.iter().enumerate() {
                assert_eq!(
                    e.offset,
                    group[0].offset + 2 * i,
                    "{} in its group of four",
                    e.name
                );
                assert_eq!(e.len, 0x8_0000, "{} is a 512 KB file", e.name);
                assert_eq!(e.load, LoadKind::Word64Word, "{}", e.name);
            }
        }
        let top = r.entries.iter().map(end_of).max().unwrap();
        assert_eq!(top, 0x60_0000, "12 x 0x80000 fills the region exactly");
    }

    #[test]
    fn audiocpu_splits_a_64k_file_across_a_64k_gap() {
        let r = SF2.region("audiocpu").unwrap();
        let e = &r.entries[0];
        assert_eq!(
            e.len, 0x1_0000,
            "one 64 KB file, not the 0x8000 ROM_LOAD half"
        );
        assert_eq!(
            e.load,
            LoadKind::Continue {
                split: 0x0_8000,
                cont_at: 0x1_0000
            },
            "ROM_LOAD 0x8000 at 0, ROM_CONTINUE 0x8000 at 0x10000"
        );
        assert_eq!(end_of(e), 0x1_8000, "and that fills the region exactly");
    }

    #[test]
    fn oki_samples_are_concatenated_without_a_gap() {
        let r = SF2.region("oki").unwrap();
        let offsets: Vec<usize> = r.entries.iter().map(|e| e.offset).collect();
        assert_eq!(offsets, vec![0x0_0000, 0x2_0000]);
        for e in r.entries {
            assert_eq!(e.len, 0x2_0000);
            assert_eq!(e.load, LoadKind::Byte);
        }
        assert_eq!(end_of(&r.entries[1]), 0x4_0000, "and fills the region");
    }
}
