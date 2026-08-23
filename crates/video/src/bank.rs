//! The graphics-ROM bank mapper.
//!
//! CPS-1 boards route tile codes through a PAL, and MAME models one function per
//! PAL (`cps1_v.cpp:1109` onward for SF2's `mapper_STF29`). A code is shifted
//! into 8×8 ROM units, matched against a table of ranges, masked into its bank,
//! and shifted back (`cps1_v.cpp:2385-2424`).
//!
//! A code no range covers has no ROM behind it. MAME returns −1 and the caller
//! substitutes an empty tile; this module returns [`None`], and the distinction
//! matters: a miss that answered 0 would draw tile 0 across the layer, which
//! reads as a tilemap bug and sends the reader to the wrong file.
//!
//! # One type per range
//!
//! MAME's `gfx_range::type` is a **bitmask**, so one row can serve several
//! graphics types (`cps1_v.cpp:597`, `:915`). Every row of STF29's table names
//! exactly one type, so [`BankRange`] carries one [`GfxType`] rather than a mask.
//! A board whose table shares rows between types needs that field widened —
//! which is a visible change here, not a silent misread.

use crate::tiles::TileKind;

/// Which set of ROM banks a tile is fetched from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GfxType {
    /// Sprites: 16×16.
    Sprite,
    /// Scroll 1: 8×8.
    Scroll1,
    /// Scroll 2: 16×16.
    Scroll2,
    /// Scroll 3: 32×32.
    Scroll3,
}

impl GfxType {
    /// How far a code shifts to become an 8×8-unit ROM offset.
    ///
    /// `cps1_v.cpp:2392-2397`. A 16×16 tile is two 8×8 units and a 32×32 is
    /// eight, so the shifts are the base-2 logs of 1, 2, and 8.
    pub const fn shift(self) -> u32 {
        match self {
            Self::Scroll1 => 0,
            Self::Sprite | Self::Scroll2 => 1,
            Self::Scroll3 => 3,
        }
    }

    /// The layout a tile of this type is decoded with.
    ///
    /// Scroll 1 answers [`TileKind::Tile8x8`]; the odd-column variant is chosen
    /// by the tilemap code, which knows the column, and not here.
    pub const fn tile_kind(self) -> TileKind {
        match self {
            Self::Scroll1 => TileKind::Tile8x8,
            Self::Sprite | Self::Scroll2 => TileKind::Tile16x16,
            Self::Scroll3 => TileKind::Tile32x32,
        }
    }
}

/// One row of a PAL's range table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BankRange {
    /// The graphics type this row applies to.
    pub kind: GfxType,
    /// First 8×8-unit offset in the range, inclusive.
    pub start: u32,
    /// Last 8×8-unit offset in the range, inclusive.
    pub end: u32,
    /// Which of the four banks the range lands in.
    pub bank: usize,
}

/// A board's code-to-ROM-offset mapping.
#[derive(Debug, Clone, Copy)]
pub struct BankMapper {
    /// Sizes of the four banks, in 8×8 units. A zero size means the bank is
    /// absent.
    pub bank_sizes: [u32; 4],
    /// Ranges, tried in order. The first match wins.
    pub ranges: &'static [BankRange],
}

/// SF2's ranges (`cps1_v.cpp:1112-1126`), from a PAL dump per MAME's comment.
static STF29_RANGES: [BankRange; 6] = [
    BankRange {
        kind: GfxType::Sprite,
        start: 0x0_0000,
        end: 0x0_7FFF,
        bank: 0,
    },
    BankRange {
        kind: GfxType::Sprite,
        start: 0x0_8000,
        end: 0x0_FFFF,
        bank: 1,
    },
    BankRange {
        kind: GfxType::Sprite,
        start: 0x1_0000,
        end: 0x1_1FFF,
        bank: 2,
    },
    BankRange {
        kind: GfxType::Scroll3,
        start: 0x0_2000,
        end: 0x0_3FFF,
        bank: 2,
    },
    BankRange {
        kind: GfxType::Scroll1,
        start: 0x0_4000,
        end: 0x0_4FFF,
        bank: 2,
    },
    BankRange {
        kind: GfxType::Scroll2,
        start: 0x0_5000,
        end: 0x0_7FFF,
        bank: 2,
    },
];

impl BankMapper {
    /// SF2's `mapper_STF29` (`cps1_v.cpp:1109`).
    ///
    /// `const` for the same reason as [`crate::regs::VideoConfig::sf2`]: a board
    /// table in another crate should not have to stop being `const` to name it.
    pub const fn stf29() -> Self {
        Self {
            bank_sizes: [0x8000, 0x8000, 0x8000, 0],
            ranges: &STF29_RANGES,
        }
    }

    /// Champion Edition's `mapper_S9263B` (`cps1_v.cpp:1284`).
    ///
    /// A different PAL from [`Self::stf29`] on a different B-board, with a
    /// **byte-for-byte identical** range table and identical bank sizes. MAME
    /// writes the two out separately because they are separate parts verified from
    /// separate dumps; their equations happen to agree, and the pin assignments in
    /// the two comments differ — `STF29`'s bank 0 is ROMs 1,5,8,12 where this
    /// part's is ROMs 1,3 and 2,4.
    ///
    /// So this reuses `STF29_RANGES` rather than copying it. The copy would be six
    /// more chances to mistype a bound, for no behavioural difference; what a
    /// reader needs instead is a test asserting the tables really are equal, which
    /// `tests::s9263b_and_stf29_have_the_same_table` is. If a future MAME revision
    /// distinguishes them, that test fails and the ranges get split then.
    pub const fn s9263b() -> Self {
        Self {
            bank_sizes: [0x8000, 0x8000, 0x8000, 0],
            ranges: &STF29_RANGES,
        }
    }

    /// The ROM offset, in tiles of `kind`'s own size, for `code`.
    ///
    /// [`None`] means no range covers the code and nothing should be drawn.
    ///
    /// `code` comes from a 16-bit gfxram word, so `code << 3` is at most
    /// 0x7FFF8 and the shift cannot lose bits. A caller passing something wider
    /// would see the shift truncate — Rust's `<<` discards high bits rather than
    /// panicking — and could alias into a range it does not belong to. Nothing in
    /// this crate can produce such a code.
    pub fn map(&self, kind: GfxType, code: u32) -> Option<u32> {
        let shift = kind.shift();
        let unit = code << shift;
        let r = self
            .ranges
            .iter()
            .find(|r| r.kind == kind && unit >= r.start && unit <= r.end)?;
        let size = self.bank_sizes[r.bank];
        // A zero-sized bank is absent, not a bank with a `0xFFFF_FFFF` mask.
        if size == 0 {
            return None;
        }
        let base: u32 = self.bank_sizes[..r.bank].iter().sum();
        Some((base + (unit & (size - 1))) >> shift)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The per-type shifts, from `cps1_v.cpp:2392-2397`.
    ///
    /// They exist because the ROM is addressed in 8×8 units: a 16×16 tile is two
    /// of them and a 32×32 is eight, so a code is shifted into the common unit,
    /// masked against its bank, and shifted back.
    #[test]
    fn the_gfx_type_shifts_are_the_tile_sizes_in_eight_by_eight_units() {
        assert_eq!(GfxType::Scroll1.shift(), 0);
        assert_eq!(GfxType::Sprite.shift(), 1);
        assert_eq!(GfxType::Scroll2.shift(), 1);
        assert_eq!(GfxType::Scroll3.shift(), 3);
    }

    /// And the layout each type decodes with, which is the shift's other face.
    #[test]
    fn each_gfx_type_decodes_with_the_layout_of_its_own_size() {
        assert_eq!(GfxType::Scroll1.tile_kind(), TileKind::Tile8x8);
        assert_eq!(GfxType::Sprite.tile_kind(), TileKind::Tile16x16);
        assert_eq!(GfxType::Scroll2.tile_kind(), TileKind::Tile16x16);
        assert_eq!(GfxType::Scroll3.tile_kind(), TileKind::Tile32x32);
    }

    /// STF29's table, checked entry by entry against `cps1_v.cpp:1112-1126`.
    ///
    /// MAME's comment says the ranges come from a PAL dump. Each expectation is
    /// hand-computed: the code shifts left by the type's shift, the range is
    /// matched on the shifted value, the result is
    /// `bank_base + (shifted & (bank_size - 1))`, and it shifts back.
    ///
    /// Bank bases are the running sum of `bank_sizes`: bank 0 at 0, bank 1 at
    /// 0x8000, bank 2 at 0x10000, bank 3 at 0x18000.
    #[test]
    fn stf29_maps_each_range_to_its_bank() {
        let m = BankMapper::stf29();
        assert_eq!(m.bank_sizes, [0x8000, 0x8000, 0x8000, 0]);

        // Sprites 0x00000-0x07fff -> bank 0. Code 0 shifts to 0, matches the
        // first range, base 0 + (0 & 0x7FFF) = 0, shifts back to 0.
        assert_eq!(m.map(GfxType::Sprite, 0x0000), Some(0x0000));
        // Code 0x3FFF shifts to 0x7FFE, still in range, 0 + 0x7FFE = 0x7FFE,
        // back to 0x3FFF.
        assert_eq!(m.map(GfxType::Sprite, 0x3FFF), Some(0x3FFF));
        // Code 0x4000 shifts to 0x8000, which is the *second* sprite range ->
        // bank 1 at base 0x8000: 0x8000 + (0x8000 & 0x7FFF) = 0x8000, back to
        // 0x4000. The identity here is not a coincidence — bank 1 begins exactly
        // where the second range does — which is why the scroll ranges below
        // carry the load.
        assert_eq!(m.map(GfxType::Sprite, 0x4000), Some(0x4000));
        // Sprites 0x10000-0x11fff -> bank 2 (base 0x10000). Code 0x8000 shifts
        // to 0x10000: 0x10000 + (0x10000 & 0x7FFF) = 0x10000, back to 0x8000.
        assert_eq!(m.map(GfxType::Sprite, 0x8000), Some(0x8000));

        // Scroll1 0x04000-0x04fff -> bank 2. Shift 0, so code 0x4000 matches
        // directly: 0x10000 + (0x4000 & 0x7FFF) = 0x14000, shift back 0 ->
        // 0x14000.
        assert_eq!(m.map(GfxType::Scroll1, 0x4000), Some(0x1_4000));
        assert_eq!(m.map(GfxType::Scroll1, 0x4FFF), Some(0x1_4FFF));

        // Scroll2 0x05000-0x07fff -> bank 2, shift 1. Code 0x2800 shifts to
        // 0x5000: 0x10000 + 0x5000 = 0x15000, back to 0xA800.
        assert_eq!(m.map(GfxType::Scroll2, 0x2800), Some(0xA800));

        // Scroll3 0x02000-0x03fff -> bank 2, shift 3. Code 0x400 shifts to
        // 0x2000: 0x10000 + 0x2000 = 0x12000, back to 0x2400.
        assert_eq!(m.map(GfxType::Scroll3, 0x0400), Some(0x2400));
    }

    /// A code no range covers maps to nothing, and the caller draws nothing.
    ///
    /// This is worth its own test because the wrong answer is *plausible*: a
    /// mapper returning 0 on a miss draws tile 0 all over the layer, which reads
    /// as a bug in the tilemap code and sends the reader to the wrong file.
    /// `gfxrom_bank_mapper` returns −1 and `cps1_v.cpp:2474` substitutes the
    /// empty tile.
    #[test]
    fn a_code_outside_every_range_maps_to_nothing() {
        let m = BankMapper::stf29();
        // Scroll1's only range is 0x4000-0x4FFF, shift 0.
        assert_eq!(m.map(GfxType::Scroll1, 0x0000), None);
        assert_eq!(m.map(GfxType::Scroll1, 0x3FFF), None);
        assert_eq!(m.map(GfxType::Scroll1, 0x5000), None);
        // Scroll3's is 0x2000-0x3FFF at shift 3, i.e. codes 0x400-0x7FF.
        assert_eq!(m.map(GfxType::Scroll3, 0x03FF), None);
        assert_eq!(m.map(GfxType::Scroll3, 0x0800), None);
        // Sprites stop at 0x11FFF, i.e. code 0x8FFF.
        assert_eq!(m.map(GfxType::Sprite, 0x9000), None);
        // And a type's ranges do not leak into another's: scroll2's 0x5000 range
        // must not answer a scroll1 code.
        assert_eq!(m.map(GfxType::Scroll1, 0x2800), None);
    }

    /// The bank mask wraps a code within its bank rather than running past it.
    ///
    /// Bank sizes are 0x8000 and the mask is `size - 1`, so two codes 0x8000
    /// apart *in the same range* would alias. STF29's ranges are all narrower
    /// than a bank, so this is checked on a synthetic mapper with one wide range
    /// rather than left unexercised.
    #[test]
    fn the_bank_mask_wraps_within_the_bank() {
        static WIDE: [BankRange; 1] = [BankRange {
            kind: GfxType::Scroll1,
            start: 0x0000,
            end: 0x1_FFFF,
            bank: 0,
        }];
        let m = BankMapper {
            bank_sizes: [0x1000, 0, 0, 0],
            ranges: &WIDE,
        };
        assert_eq!(m.map(GfxType::Scroll1, 0x0123), Some(0x0123));
        assert_eq!(
            m.map(GfxType::Scroll1, 0x1123),
            Some(0x0123),
            "0x1123 & 0x0FFF"
        );
    }

    /// A range pointing at a later bank adds the sizes of the banks before it.
    ///
    /// STF29 exercises this, but only with three equal 0x8000 banks, where a
    /// running sum and a `bank * 0x8000` multiplication agree. Unequal sizes
    /// separate them: bank 2's base here is 0x1000 + 0x2000 = 0x3000, not
    /// 2 × anything.
    #[test]
    fn a_banks_base_is_the_sum_of_the_banks_before_it() {
        static AT2: [BankRange; 1] = [BankRange {
            kind: GfxType::Scroll1,
            start: 0,
            end: 0xFFFF,
            bank: 2,
        }];
        let m = BankMapper {
            bank_sizes: [0x1000, 0x2000, 0x4000, 0],
            ranges: &AT2,
        };
        assert_eq!(m.map(GfxType::Scroll1, 0x0123), Some(0x3123));
    }

    /// A zero-sized bank is not a divide-by-zero or a full-range mask.
    ///
    /// STF29's fourth bank size is 0, and a `size - 1` of `0xFFFF_FFFF` would
    /// turn a range pointing at bank 3 into an unmasked pass-through — and in
    /// debug builds `0u32 - 1` panics outright. No STF29 range uses bank 3, so
    /// this is checked on a synthetic mapper.
    #[test]
    fn a_zero_sized_bank_maps_to_nothing() {
        static AT3: [BankRange; 1] = [BankRange {
            kind: GfxType::Scroll1,
            start: 0,
            end: 0xFFFF,
            bank: 3,
        }];
        let m = BankMapper {
            bank_sizes: [0x1000, 0x1000, 0x1000, 0],
            ranges: &AT3,
        };
        assert_eq!(m.map(GfxType::Scroll1, 0x0123), None);
    }

    /// `mapper_S9263B` and `mapper_STF29` describe the same mapping.
    ///
    /// [`BankMapper::s9263b`] shares `STF29_RANGES` by reference on the strength of
    /// this, so the assertion is what licenses the sharing rather than a
    /// restatement of it. The table is transcribed here from `cps1_v.cpp:1284-1301`
    /// as literals and compared against **both** mappers: comparing the two
    /// mappers to each other would pass trivially while they share a pointer, and
    /// would keep passing if MAME's S9263B table changed under us.
    #[test]
    fn s9263b_and_stf29_have_the_same_table() {
        let expected: [(GfxType, u32, u32, usize); 6] = [
            (GfxType::Sprite, 0x0_0000, 0x0_7FFF, 0),
            (GfxType::Sprite, 0x0_8000, 0x0_FFFF, 1),
            (GfxType::Sprite, 0x1_0000, 0x1_1FFF, 2),
            (GfxType::Scroll3, 0x0_2000, 0x0_3FFF, 2),
            (GfxType::Scroll1, 0x0_4000, 0x0_4FFF, 2),
            (GfxType::Scroll2, 0x0_5000, 0x0_7FFF, 2),
        ];
        for m in [BankMapper::s9263b(), BankMapper::stf29()] {
            assert_eq!(m.bank_sizes, [0x8000, 0x8000, 0x8000, 0]);
            assert_eq!(m.ranges.len(), 6);
            for (r, &(kind, start, end, bank)) in m.ranges.iter().zip(expected.iter()) {
                assert_eq!(
                    *r,
                    BankRange {
                        kind,
                        start,
                        end,
                        bank
                    }
                );
            }
        }
        // And they agree on what they actually compute, over one code per range.
        for (kind, code) in [
            (GfxType::Sprite, 0x0100u32),
            (GfxType::Sprite, 0x5000),
            (GfxType::Sprite, 0x8800),
            (GfxType::Scroll3, 0x0400),
            (GfxType::Scroll1, 0x4800),
            (GfxType::Scroll2, 0x3000),
        ] {
            assert_eq!(
                BankMapper::s9263b().map(kind, code),
                BankMapper::stf29().map(kind, code),
                "{kind:?} code {code:#06x}"
            );
            assert!(
                BankMapper::s9263b().map(kind, code).is_some(),
                "{kind:?} code {code:#06x} must be inside a range, or the pair \
                 above agrees only on None"
            );
        }
    }
}
