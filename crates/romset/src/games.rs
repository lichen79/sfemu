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

// ---------------------------------------------------------------------------
// Street Fighter (US, set 1) — MAME set `sf`, `src/mame/capcom/sf.cpp:829-895`
// (BSD-3-Clause, copyright-holders Olivier Galibert), read 2026-08-17.
//
// ⚠️ Names, offsets, lengths and CRCs only. No ROM data.
//
// The set name here is `sf1`, not MAME's `sf`: this crate's names are what a
// user types, and `sf` alone is ambiguous with the family. `sf.cpp`'s eight
// other sets (sfua, sfj, sfjbl, sfw, sfjan, sfan, sfp) need either the i8751
// protection MCU or the pneumatic cabinet's pressure pads, and are not here.
// ---------------------------------------------------------------------------

/// 68000 program: three pairs of 64 KB files, byte-interleaved.
///
/// The **even** offset of each pair supplies the high byte of the big-endian
/// word. Six files fill 0x00000-0x5FFFF, but all three memory maps decode only
/// 0x000000-0x04FFFF (`sf.cpp:141`), so the top 64 KB is never fetched.
#[rustfmt::skip]
static SF1_MAINCPU: &[RomEntry] = &[
    RomEntry { name: "sfd-19.2a", offset: 0x0_0000, len: 0x1_0000, crc32: 0xfaaf_6255, load: W16 },
    RomEntry { name: "sfd-22.2c", offset: 0x0_0001, len: 0x1_0000, crc32: 0xe1fe_3519, load: W16 },
    RomEntry { name: "sfd-20.3a", offset: 0x2_0000, len: 0x1_0000, crc32: 0x44b9_15bd, load: W16 },
    RomEntry { name: "sfd-23.3c", offset: 0x2_0001, len: 0x1_0000, crc32: 0x79c4_3ff8, load: W16 },
    RomEntry { name: "sfd-21.4a", offset: 0x4_0000, len: 0x1_0000, crc32: 0xe8db_799b, load: W16 },
    RomEntry { name: "sfd-24.4c", offset: 0x4_0001, len: 0x1_0000, crc32: 0x466a_3440, load: W16 },
];

/// Music Z80: one 32 KB file in a 64 KB region.
///
/// MAME's comment is `/* 64k for the music CPU */`. The upper half is
/// unpopulated and `sound_map` (`sf.cpp:210`) decodes only 0x0000-0x7fff as
/// ROM, so nothing reads it. The region size is MAME's and stays MAME's.
#[rustfmt::skip]
static SF1_AUDIOCPU: &[RomEntry] = &[
    RomEntry { name: "sf-02.7k", offset: 0x0_0000, len: 0x0_8000, crc32: 0x4a9a_c534, load: BYTE },
];

/// ADPCM Z80: two 128 KB files, concatenated.
///
/// MAME's comment is `/* 256k for the samples CPU */`. This CPU banks
/// 0x8000-0xffff from here and **has no RAM at all** (`sf.cpp:217-223`).
#[rustfmt::skip]
static SF1_AUDIO2: &[RomEntry] = &[
    RomEntry { name: "sfu-00.1h", offset: 0x0_0000, len: 0x2_0000, crc32: 0xa7cc_e903, load: BYTE },
    RomEntry { name: "sf-01.1k",  offset: 0x2_0000, len: 0x2_0000, crc32: 0x86e0_f0d5, load: BYTE },
];

/// Background "b" tiles — gfx 0, the bg tilemap's 16×16 4-plane sprites.
///
/// Planes 0-1 in the first half, planes 2-3 in the second: `RGN_FRAC(1,2)`'s
/// plane offsets are `{4, 0, half+4, half}` in **bits**, where
/// `half = 8 * 0x80000 / 2`. MAME's own comments mark the split.
#[rustfmt::skip]
static SF1_GFX1: &[RomEntry] = &[
    RomEntry { name: "sf-39.2k", offset: 0x0_0000, len: 0x2_0000, crc32: 0xcee3_d292, load: BYTE },
    RomEntry { name: "sf-38.1k", offset: 0x2_0000, len: 0x2_0000, crc32: 0x2ea9_9676, load: BYTE },
    RomEntry { name: "sf-41.4k", offset: 0x4_0000, len: 0x2_0000, crc32: 0xe028_0495, load: BYTE },
    RomEntry { name: "sf-40.3k", offset: 0x6_0000, len: 0x2_0000, crc32: 0xc70b_30de, load: BYTE },
];

/// Background "m" tiles — gfx 1, the fg tilemap. Same half-split as `gfx1`.
#[rustfmt::skip]
static SF1_GFX2: &[RomEntry] = &[
    RomEntry { name: "sf-25.1d", offset: 0x0_0000, len: 0x2_0000, crc32: 0x7f23_042e, load: BYTE },
    RomEntry { name: "sf-28.1e", offset: 0x2_0000, len: 0x2_0000, crc32: 0x92f8_b91c, load: BYTE },
    RomEntry { name: "sf-30.1g", offset: 0x4_0000, len: 0x2_0000, crc32: 0xb139_9856, load: BYTE },
    RomEntry { name: "sf-34.1h", offset: 0x6_0000, len: 0x2_0000, crc32: 0x96b6_ae2e, load: BYTE },
    RomEntry { name: "sf-26.2d", offset: 0x8_0000, len: 0x2_0000, crc32: 0x54ed_e9f5, load: BYTE },
    RomEntry { name: "sf-29.2e", offset: 0xa_0000, len: 0x2_0000, crc32: 0xf064_9a67, load: BYTE },
    RomEntry { name: "sf-31.2g", offset: 0xc_0000, len: 0x2_0000, crc32: 0x8f4d_d71a, load: BYTE },
    RomEntry { name: "sf-35.2h", offset: 0xe_0000, len: 0x2_0000, crc32: 0x70c0_0fb4, load: BYTE },
];

/// Sprites — gfx 2. `draw_sprites` uses this region and only this region
/// (`sf.cpp:365-450`, every draw is `gfx(2)->transpen(...)`).
///
/// Fourteen files, and note the **non-monotonic file numbering**: 15, 16, 11,
/// 12, 07, 08, 03, then 17, 18, 13, 14, 09, 10, 05. That is MAME's order and
/// the offsets are sequential; the names are not sorted and must not be.
#[rustfmt::skip]
static SF1_GFX3: &[RomEntry] = &[
    RomEntry { name: "sf-15.1m", offset: 0x00_0000, len: 0x2_0000, crc32: 0xfc01_13db, load: BYTE },
    RomEntry { name: "sf-16.2m", offset: 0x02_0000, len: 0x2_0000, crc32: 0x82e4_a6d3, load: BYTE },
    RomEntry { name: "sf-11.1k", offset: 0x04_0000, len: 0x2_0000, crc32: 0xe112_df1b, load: BYTE },
    RomEntry { name: "sf-12.2k", offset: 0x06_0000, len: 0x2_0000, crc32: 0x42d5_2299, load: BYTE },
    RomEntry { name: "sf-07.1h", offset: 0x08_0000, len: 0x2_0000, crc32: 0x49f3_40d9, load: BYTE },
    RomEntry { name: "sf-08.2h", offset: 0x0a_0000, len: 0x2_0000, crc32: 0x95ec_e9b1, load: BYTE },
    RomEntry { name: "sf-03.1f", offset: 0x0c_0000, len: 0x2_0000, crc32: 0x5ca0_5781, load: BYTE },
    RomEntry { name: "sf-17.3m", offset: 0x0e_0000, len: 0x2_0000, crc32: 0x69fa_c48e, load: BYTE },
    RomEntry { name: "sf-18.4m", offset: 0x10_0000, len: 0x2_0000, crc32: 0x71cf_d18d, load: BYTE },
    RomEntry { name: "sf-13.3k", offset: 0x12_0000, len: 0x2_0000, crc32: 0xfa2e_b24b, load: BYTE },
    RomEntry { name: "sf-14.4k", offset: 0x14_0000, len: 0x2_0000, crc32: 0xad95_5c95, load: BYTE },
    RomEntry { name: "sf-09.3h", offset: 0x16_0000, len: 0x2_0000, crc32: 0x41b7_3a31, load: BYTE },
    RomEntry { name: "sf-10.4h", offset: 0x18_0000, len: 0x2_0000, crc32: 0x91c4_1c50, load: BYTE },
    RomEntry { name: "sf-05.3f", offset: 0x1a_0000, len: 0x2_0000, crc32: 0x538c_7cbe, load: BYTE },
];

/// Characters — gfx 3, the tx tilemap. One 16 KB file, 8×8, **two** planes.
///
/// `RGN_FRAC(1,1)`: no half-split, and granularity 4 rather than 16 because
/// `1 << planes` is 4 (`drawgfx.cpp:145`).
#[rustfmt::skip]
static SF1_GFX4: &[RomEntry] = &[
    RomEntry { name: "sf-27.4d", offset: 0x0_0000, len: 0x0_4000, crc32: 0x2b09_b36d, load: BYTE },
];

/// The bg and fg **tile maps** — not tiles, maps. MAME's comment is
/// `/* background tilemaps */`.
///
/// Four 64 KB byte-planes: bg reads `[0x00000 + 2*i]` for its code/colour low
/// bytes and `[0x10000 + 2*i]` for attribute/code high; fg the same at
/// 0x20000/0x30000 (`sf.cpp:241-259`). This is why SF1 needs no tilemap RAM.
#[rustfmt::skip]
static SF1_TILEROM: &[RomEntry] = &[
    RomEntry { name: "sf-37.4h", offset: 0x0_0000, len: 0x1_0000, crc32: 0x23d0_9d3d, load: BYTE },
    RomEntry { name: "sf-36.3h", offset: 0x1_0000, len: 0x1_0000, crc32: 0xea16_df6c, load: BYTE },
    RomEntry { name: "sf-32.3g", offset: 0x2_0000, len: 0x1_0000, crc32: 0x72df_2bd9, load: BYTE },
    RomEntry { name: "sf-33.4g", offset: 0x3_0000, len: 0x1_0000, crc32: 0x3e99_d3d5, load: BYTE },
];

#[rustfmt::skip]
static SF1_REGIONS: &[RegionSpec] = &[
    RegionSpec { name: "maincpu",  size: 0x06_0000, entries: SF1_MAINCPU  },
    RegionSpec { name: "audiocpu", size: 0x01_0000, entries: SF1_AUDIOCPU },
    RegionSpec { name: "audio2",   size: 0x04_0000, entries: SF1_AUDIO2   },
    RegionSpec { name: "gfx1",     size: 0x08_0000, entries: SF1_GFX1     },
    RegionSpec { name: "gfx2",     size: 0x10_0000, entries: SF1_GFX2     },
    RegionSpec { name: "gfx3",     size: 0x1c_0000, entries: SF1_GFX3     },
    RegionSpec { name: "gfx4",     size: 0x00_4000, entries: SF1_GFX4     },
    RegionSpec { name: "tilerom",  size: 0x04_0000, entries: SF1_TILEROM  },
];

/// Street Fighter (US, set 1), MAME set `sf`.
///
/// The unprotected set: `sfus(config)` is `sfan(config)` plus one
/// `set_addrmap` (`sf.cpp:799`), it has no `protcpu` region, and its
/// `sfus_map` reads `nopr()` where the deluxe cabinet reads its pedals.
pub static SF1: GameSpec = GameSpec {
    name: "sf1",
    regions: SF1_REGIONS,
};

/// 68000 program for the 910214 revision, MAME set `sf2eb`.
///
/// The same eight-file shape as [`SF2_MAINCPU`] on the same board — only the
/// program revision differs, which is why every other region is shared rather
/// than copied. Two entries are byte-for-byte the ones rev G uses
/// (`sf2_29b.10e`, `sf2_36b.10f`); the other six are this revision's.
///
/// Transcribed from `cps1.cpp:7199-7208`, read 2026-08-23.
#[rustfmt::skip]
static SF2EB_MAINCPU: &[RomEntry] = &[
    RomEntry { name: "sf2e_30b.11e", offset: 0x0_0000, len: 0x2_0000, crc32: 0x57bd_7051, load: W16 },
    RomEntry { name: "sf2e_37b.11f", offset: 0x0_0001, len: 0x2_0000, crc32: 0x6269_1cdd, load: W16 },
    RomEntry { name: "sf2e_31b.12e", offset: 0x4_0000, len: 0x2_0000, crc32: 0xa673_143d, load: W16 },
    RomEntry { name: "sf2e_38b.12f", offset: 0x4_0001, len: 0x2_0000, crc32: 0x4c2c_cef7, load: W16 },
    RomEntry { name: "sf2_28b.9e",   offset: 0x8_0000, len: 0x2_0000, crc32: 0x4009_955e, load: W16 },
    RomEntry { name: "sf2_35b.9f",   offset: 0x8_0001, len: 0x2_0000, crc32: 0x8c1f_3994, load: W16 },
    RomEntry { name: "sf2_29b.10e",  offset: 0xc_0000, len: 0x2_0000, crc32: 0xbb4a_f315, load: W16 },
    RomEntry { name: "sf2_36b.10f",  offset: 0xc_0001, len: 0x2_0000, crc32: 0xc02a_13eb, load: W16 },
];

/// `sf2eb`'s regions: its own program, and rev G's graphics, audio and samples.
///
/// The three shared statics are referenced rather than duplicated, which is the
/// property that matters: MAME's own driver loads the identical files for both
/// sets, so a copy here could drift from `SF2`'s and the symptom would be one
/// revision rendering correctly while the other did not.
#[rustfmt::skip]
static SF2EB_REGIONS: &[RegionSpec] = &[
    RegionSpec { name: "maincpu",  size: 0x40_0000, entries: SF2EB_MAINCPU },
    RegionSpec { name: "gfx",      size: 0x60_0000, entries: SF2_GFX      },
    RegionSpec { name: "audiocpu", size: 0x01_8000, entries: SF2_AUDIOCPU },
    RegionSpec { name: "oki",      size: 0x04_0000, entries: SF2_OKI      },
];

/// Street Fighter II: The World Warrior (World 910214), MAME set `sf2eb`.
///
/// The same CPS-1 hardware as [`SF2`], so `board_for` maps both to
/// `BoardKind::Cps1`. A separate spec and not a flag on `SF2`: a set is a list of
/// files with checksums, and "the same game with six different files" is a
/// different list.
pub static SF2EB: GameSpec = GameSpec {
    name: "sf2eb",
    regions: SF2EB_REGIONS,
};

/// Every set this crate knows.
pub static ALL: &[&GameSpec] = &[&SF2, &SF2EB, &SF1];

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
        assert_eq!(by_name("sf1").map(|g| g.name), Some("sf1"), "F landed");
    }

    /// SF1's eight regions, their file counts and their sizes — all literals.
    ///
    /// `proms` (0x320, four files each commented `/* unknown */` in MAME) is
    /// deliberately absent: nothing in the driver reads it.
    #[test]
    fn sf1_has_eight_regions_with_the_expected_file_counts() {
        assert_eq!(SF1.regions.len(), 8);
        let names: Vec<&str> = SF1.regions.iter().map(|r| r.name).collect();
        assert_eq!(
            names,
            vec!["maincpu", "audiocpu", "audio2", "gfx1", "gfx2", "gfx3", "gfx4", "tilerom"]
        );
        let counts: Vec<usize> = SF1.regions.iter().map(|r| r.entries.len()).collect();
        assert_eq!(counts, vec![6, 1, 2, 4, 8, 14, 1, 4]);
        assert_eq!(
            counts.iter().sum::<usize>(),
            40,
            "44 in ROM_START less 4 proms"
        );
        assert_eq!(by_name("sf1").map(|g| g.name), Some("sf1"));
    }

    /// The eight `ROM_REGION` sizes, as literals.
    #[test]
    fn sf1_region_sizes_are_the_literal_mame_rom_region_sizes() {
        let sizes: Vec<usize> = SF1.regions.iter().map(|r| r.size).collect();
        assert_eq!(
            sizes,
            vec![0x6_0000, 0x1_0000, 0x4_0000, 0x8_0000, 0x10_0000, 0x1c_0000, 0x4000, 0x4_0000]
        );
    }

    /// Seven of the eight regions are exactly filled; `audiocpu` is not.
    ///
    /// `audiocpu`'s region is 0x10000 and its only file is 0x8000, so the upper
    /// half is unpopulated. The Z80's map decodes only 0x0000-0x7fff as ROM, so
    /// nothing reads it — but the region size is MAME's and a spec claiming
    /// 0x8000 would be a transcription error no other test would catch.
    #[test]
    fn every_sf1_region_but_audiocpu_is_exactly_filled_by_its_files() {
        for r in SF1.regions {
            let filled: usize = r.entries.iter().map(|e| e.len).sum();
            if r.name == "audiocpu" {
                assert_eq!(filled, 0x8000, "one 32k file");
                assert_eq!(r.size, 0x1_0000, "in a 64k region");
            } else {
                assert_eq!(filled, r.size, "{} should be exactly filled", r.name);
            }
        }
    }

    #[test]
    fn every_sf1_entry_fits_inside_its_region() {
        for r in SF1.regions {
            for e in r.entries {
                assert!(
                    end_of(e) <= r.size,
                    "{} ends at {:#x}, past {} ({:#x})",
                    e.name,
                    end_of(e),
                    r.name,
                    r.size
                );
            }
        }
    }

    /// Forty distinct files and forty distinct checksums.
    #[test]
    fn no_two_sf1_entries_share_a_name_or_a_crc() {
        let mut names = std::collections::BTreeSet::new();
        let mut crcs = std::collections::BTreeSet::new();
        for r in SF1.regions {
            for e in r.entries {
                assert!(names.insert(e.name), "{} appears twice", e.name);
                assert!(
                    crcs.insert(e.crc32),
                    "{} duplicates CRC {:08x}",
                    e.name,
                    e.crc32
                );
            }
        }
        assert_eq!(names.len(), 40);
        assert_eq!(crcs.len(), 40);
    }

    /// `maincpu`'s six files are three even/odd pairs at the 0x20000 bases.
    ///
    /// The **even** offset supplies the high byte of the big-endian word. A CRC
    /// check catches a swapped file; nothing but this catches a swapped byte,
    /// and getting it backwards byte-swaps every instruction word.
    #[test]
    fn sf1_maincpu_pairs_alternate_even_and_odd_at_three_bases() {
        let m = SF1.region("maincpu").expect("maincpu");
        assert_eq!(m.entries.len(), 6);
        for (i, e) in m.entries.iter().enumerate() {
            assert_eq!(e.load, LoadKind::Word16Byte, "{}", e.name);
            assert_eq!(e.len, 0x1_0000, "{}", e.name);
            assert_eq!(e.offset & 1, i % 2, "{} parity", e.name);
        }
        let bases: Vec<usize> = m.entries.iter().map(|e| e.offset & !1).collect();
        assert_eq!(
            bases,
            vec![0x0_0000, 0x0_0000, 0x2_0000, 0x2_0000, 0x4_0000, 0x4_0000]
        );
        assert_eq!(
            m.entries[0].name, "sfd-19.2a",
            "high byte of the first word"
        );
        assert_eq!(m.entries[1].name, "sfd-22.2c", "low byte");
    }

    /// Every non-maincpu entry is a plain `ROM_LOAD`, laid end to end.
    ///
    /// SF1 uses only two of `LoadKind`'s four variants. A stray `Word64Word` or
    /// `Continue` here would be a copy-paste from SF2's table.
    #[test]
    fn sf1_uses_only_byte_loads_outside_maincpu() {
        for r in SF1.regions {
            if r.name == "maincpu" {
                continue;
            }
            let mut next = 0;
            for e in r.entries {
                assert_eq!(e.load, LoadKind::Byte, "{}", e.name);
                assert_eq!(e.offset, next, "{} should follow the previous file", e.name);
                next += e.len;
            }
        }
    }

    /// The three gfx regions split at their midpoints, which is what
    /// `RGN_FRAC(1,2)`'s plane offsets depend on.
    ///
    /// MAME's own comments mark `gfx1`'s first two files "planes 0-1" and its
    /// second two "planes 2-3", and `gfx2`'s first four and second four the
    /// same way. Both splits land exactly halfway.
    #[test]
    fn sf1_sprite_gfx_regions_split_in_half_on_a_file_boundary() {
        for tag in ["gfx1", "gfx2", "gfx3"] {
            let r = SF1.region(tag).unwrap_or_else(|| panic!("{tag}"));
            let half = r.size / 2;
            assert!(
                r.entries.iter().any(|e| e.offset == half),
                "{tag} has no file starting at its midpoint {half:#x}"
            );
        }
    }

    /// `tilerom` is two 0x20000 tilemaps, each two byte-planes of 0x10000.
    ///
    /// bg reads [0x00000] paired with [0x10000]; fg reads [0x20000] paired with
    /// [0x30000] (`sf.cpp:239-262`). 2048 × 16 tiles × 2 bytes = 0x10000
    /// exactly, which is the check that the split is right.
    #[test]
    fn sf1_tilerom_is_four_64k_planes() {
        let t = SF1.region("tilerom").expect("tilerom");
        assert_eq!(t.size, 0x4_0000);
        let offsets: Vec<usize> = t.entries.iter().map(|e| e.offset).collect();
        assert_eq!(offsets, vec![0x0_0000, 0x1_0000, 0x2_0000, 0x3_0000]);
        assert_eq!(
            2048 * 16 * 2,
            0x1_0000,
            "one map's tile count times two bytes"
        );
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
