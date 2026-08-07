//! Decoding a tile's pixels out of the graphics ROM.
//!
//! CPS-1 graphics are 4 bits per pixel with the four planes interleaved a byte
//! at a time. MAME describes the arrangement as four `gfx_layout`s
//! (`cps1.cpp:3837-3878`); the four share one rule.
//!
//! # The rule
//!
//! A tile occupies a storage *frame* `FW` pixels wide — 16 for the 8×8 and 16×16
//! kinds, 32 for 32×32 — and the bit index of pixel `(x, y)` in plane `p`,
//! counted from the tile's first byte with bits numbered MSB-first, is
//!
//! ```text
//! y * (4 * FW)  +  32 * (x >> 3)  +  (x & 7)  +  [24, 16, 8, 0][p]
//! ```
//!
//! Each group of eight horizontal pixels therefore occupies four consecutive
//! bytes, one per plane, and a frame row is `FW / 2` bytes. Plane 0 sits at bit
//! offset 24 and supplies the pen's **most** significant bit.
//!
//! The 8×8 kinds share a frame: [`TileKind::Tile8x8`] is `STEP8(0, 1)` and
//! [`TileKind::Tile8x8Odd`] is `STEP8(32, 1)` (`cps1.cpp:3843`, `:3854`), so one
//! 64-byte block holds two 8-pixel tiles side by side. `get_tile0_info`
//! (`cps1_v.cpp:2462`) picks between them with `BIT(tile_index, 5)`, which under
//! scroll-1's scan mapper is the column's low bit — MAME's comment records that
//! this was found with a Final Fight board carrying mixed-region ROMs.

/// The pen an out-of-range or absent tile decodes to.
///
/// `cps1_v.cpp:2551` fills MAME's `m_empty_tile` with 0x0f, and every draw path
/// treats pen 15 as transparent. Returning 0 instead would paint colour-index 0
/// wherever a tile is missing, which looks like a tilemap bug rather than a
/// missing ROM.
pub const TRANSPARENT_PEN: u8 = 0x0F;

/// Which of the four graphics layouts a tile uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileKind {
    /// 8×8, left half of a 16-pixel frame. Scroll 1, even columns.
    Tile8x8,
    /// 8×8, right half of a 16-pixel frame. Scroll 1, odd columns.
    Tile8x8Odd,
    /// 16×16. Scroll 2 and sprites.
    Tile16x16,
    /// 32×32. Scroll 3.
    Tile32x32,
}

impl TileKind {
    /// The tile's edge in pixels.
    pub const fn size(self) -> u32 {
        match self {
            Self::Tile8x8 | Self::Tile8x8Odd => 8,
            Self::Tile16x16 => 16,
            Self::Tile32x32 => 32,
        }
    }

    /// Bytes one tile occupies in the ROM.
    ///
    /// The two 8×8 kinds each claim the whole 64-byte frame they share, so a
    /// code indexes frames, not half-frames — which is what `get_tile0_info`
    /// does when it hands the same `code` to either gfxset.
    pub const fn bytes(self) -> usize {
        match self {
            Self::Tile8x8 | Self::Tile8x8Odd => 64,
            Self::Tile16x16 => 128,
            Self::Tile32x32 => 512,
        }
    }

    /// The storage frame's width in pixels — 16 for both 8×8 kinds.
    const fn frame_width(self) -> u32 {
        match self {
            Self::Tile8x8 | Self::Tile8x8Odd | Self::Tile16x16 => 16,
            Self::Tile32x32 => 32,
        }
    }

    /// The bit offset the tile's x=0 sits at within the frame.
    const fn x_bias(self) -> u32 {
        match self {
            Self::Tile8x8Odd => 32,
            _ => 0,
        }
    }
}

/// The 4-bit pen of pixel `(x, y)` of tile `code`.
///
/// Returns [`TRANSPARENT_PEN`] when the tile is not wholly inside `rom`, which
/// covers both a code past the end of a real graphics region and the empty
/// region a caller with no graphics ROM supplies.
///
/// `x` and `y` are expected inside `kind.size()`; larger values read further
/// into the ROM and are the caller's arithmetic error, not a case this masks.
pub fn tile_pen(rom: &[u8], kind: TileKind, code: u32, x: u32, y: u32) -> u8 {
    let start = (code as usize).saturating_mul(kind.bytes());
    let end = start.saturating_add(kind.bytes());
    let Some(tile) = rom.get(start..end) else {
        return TRANSPARENT_PEN;
    };
    let base = y * 4 * kind.frame_width() + 32 * (x >> 3) + (x & 7) + kind.x_bias();
    let mut pen = 0u8;
    for (p, off) in [24u32, 16, 8, 0].into_iter().enumerate() {
        let bit = base + off;
        let byte = tile[(bit / 8) as usize];
        // MSB-first within the byte, MAME's convention.
        if byte & (0x80 >> (bit % 8)) != 0 {
            pen |= 0x08 >> p;
        }
    }
    pen
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 8×8 tile, byte-for-byte, decoded to a hand-written pen grid.
    ///
    /// The bytes are chosen so each plane is distinguishable: within the first
    /// row's four bytes, plane 0 (bit offset 24, the pen's bit 3) is
    /// `0b1000_0000`, plane 1 is `0b0100_0000`, plane 2 `0b0010_0000`, plane 3
    /// `0b0001_0000`. Bit 7 of each byte is pixel x=0, so x=0 takes bit 3 from
    /// plane 0 only and its pen is 8; x=1 takes bit 2 from plane 1 and its pen is
    /// 4; and so on.
    ///
    /// Byte order within a row: `cps1.cpp:3841`'s planes are `{24, 16, 8, 0}` and
    /// MAME numbers plane offsets in bits from the tile's start, so the four
    /// bytes of a row are planes 3, 2, 1, 0 in memory order. The literal grid
    /// below is what makes that claim checkable rather than asserted.
    #[test]
    fn an_8x8_tile_decodes_to_its_hand_written_pen_grid() {
        let mut rom = vec![0u8; 64];
        // Row 0, bytes 0-3 = planes 3, 2, 1, 0 (bit offsets 0, 8, 16, 24).
        rom[0] = 0b0001_0000; // plane 3 -> pen bit 0, at x = 3
        rom[1] = 0b0010_0000; // plane 2 -> pen bit 1, at x = 2
        rom[2] = 0b0100_0000; // plane 1 -> pen bit 2, at x = 1
        rom[3] = 0b1000_0000; // plane 0 -> pen bit 3, at x = 0
        let row0: [u8; 8] = [8, 4, 2, 1, 0, 0, 0, 0];
        for (x, &want) in row0.iter().enumerate() {
            assert_eq!(
                tile_pen(&rom, TileKind::Tile8x8, 0, x as u32, 0),
                want,
                "x = {x}"
            );
        }

        // A row of 0xFF in every plane is pen 15 across. Row 4 of an 8×8 tile
        // starts at bit 4 * 4 * 16 = 256, i.e. byte 32 — a *frame* row is 8
        // bytes, not 4, because the frame is 16 pixels wide and only the left
        // half belongs to this tile.
        for b in &mut rom[32..36] {
            *b = 0xFF;
        }
        for x in 0..8 {
            assert_eq!(tile_pen(&rom, TileKind::Tile8x8, 0, x, 4), 15, "x = {x}");
        }
        // and row 4 only: rows 1-3 and 5-7 stay 0.
        for y in [1, 2, 3, 5, 6, 7] {
            assert_eq!(tile_pen(&rom, TileKind::Tile8x8, 0, 0, y), 0, "y = {y}");
        }
    }

    /// A row is `4 * FW / 8` = 8 bytes for the 8×8 kinds, and the *second* four
    /// bytes of each row belong to the odd-column tile.
    ///
    /// The two 8×8 layouts differ only in x base — `STEP8(0,1)` versus
    /// `STEP8(32,1)` (`cps1.cpp:3843`, `:3854`) — so the same 64-byte block holds
    /// two 8-pixel-wide tiles side by side in a 16-pixel frame. This test writes
    /// a pattern into the high half and requires the low half to stay blank.
    #[test]
    fn the_odd_8x8_kind_reads_the_second_half_of_the_frame() {
        let mut rom = vec![0u8; 64];
        // Row 0's second group of four bytes: bit offsets 32..64.
        rom[4] = 0b0001_0000;
        rom[5] = 0b0010_0000;
        rom[6] = 0b0100_0000;
        rom[7] = 0b1000_0000;
        let want: [u8; 8] = [8, 4, 2, 1, 0, 0, 0, 0];
        for (x, &w) in want.iter().enumerate() {
            assert_eq!(
                tile_pen(&rom, TileKind::Tile8x8Odd, 0, x as u32, 0),
                w,
                "odd x = {x}"
            );
            assert_eq!(
                tile_pen(&rom, TileKind::Tile8x8, 0, x as u32, 0),
                0,
                "the even tile of the same block is blank at x = {x}"
            );
        }
    }

    /// A 16×16 tile is 128 bytes, its right half lives at bit offset 32 of each
    /// row, and rows are 8 bytes apart (`STEP16(0, 4*16)`, `cps1.cpp:3866`).
    #[test]
    fn a_16x16_tile_spans_both_halves_of_each_eight_byte_row() {
        let mut rom = vec![0u8; 128];
        rom[3] = 0x80; // plane 0, x = 0, y = 0 -> pen 8
        rom[7] = 0x80; // plane 0, x = 8, y = 0 -> pen 8
        rom[8 * 15 + 3] = 0x01; // plane 0, x = 7, y = 15 -> pen 8
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 0, 0, 0), 8);
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 0, 8, 0), 8);
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 0, 7, 15), 8);
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 0, 1, 0), 0);
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 0, 15, 0), 0);
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 0, 0, 1), 0);
    }

    /// A 32×32 tile is 512 bytes with a 16-byte row, and its four horizontal
    /// groups are at bit offsets 0, 32, 64, 96 (`cps1.cpp:3876`).
    #[test]
    fn a_32x32_tile_has_four_horizontal_groups_per_sixteen_byte_row() {
        let mut rom = vec![0u8; 512];
        for g in 0..4usize {
            rom[4 * g + 3] = 0x80; // plane 0, x = 8*g, y = 0
        }
        rom[16 * 31 + 3] = 0x80; // x = 0, y = 31
        for g in 0..4u32 {
            assert_eq!(tile_pen(&rom, TileKind::Tile32x32, 0, 8 * g, 0), 8, "g={g}");
            assert_eq!(
                tile_pen(&rom, TileKind::Tile32x32, 0, 8 * g + 1, 0),
                0,
                "and only the first pixel of the group"
            );
        }
        assert_eq!(tile_pen(&rom, TileKind::Tile32x32, 0, 0, 31), 8);
    }

    /// Tile sizes and byte counts, pinned as literals.
    #[test]
    fn a_tile_kinds_size_and_byte_count_are_the_layouts() {
        assert_eq!(
            (TileKind::Tile8x8.size(), TileKind::Tile8x8.bytes()),
            (8, 64)
        );
        assert_eq!(
            (TileKind::Tile8x8Odd.size(), TileKind::Tile8x8Odd.bytes()),
            (8, 64)
        );
        assert_eq!(
            (TileKind::Tile16x16.size(), TileKind::Tile16x16.bytes()),
            (16, 128)
        );
        assert_eq!(
            (TileKind::Tile32x32.size(), TileKind::Tile32x32.bytes()),
            (32, 512)
        );
    }

    /// The second tile of a ROM is at `code * bytes()`.
    ///
    /// Without this, every test above passes with the code multiplier missing:
    /// they all use tile 0.
    #[test]
    fn a_code_indexes_by_the_tile_byte_size() {
        let mut rom = vec![0u8; 3 * 128];
        rom[128 + 3] = 0x80; // tile 1, plane 0, x = 0, y = 0
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 1, 0, 0), 8);
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 0, 0, 0), 0);
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 2, 0, 0), 0);
    }

    /// A code past the end of the ROM reads as transparent rather than panicking.
    ///
    /// An empty ROM is the case `machine`'s existing tests hit: `Cps1::new` keeps
    /// its three-argument signature and builds a video state with no graphics
    /// region, so every synthetic-program test in sub-project B renders through
    /// this path.
    ///
    /// ⚠️ **The ROM must not be filled with 0xFF here.** An all-ones tile decodes
    /// to pen 15, which *is* [`TRANSPARENT_PEN`], so an assertion against a 0xFF
    /// fill holds whether the range check fires or not — a mutant shortening the
    /// range by one byte survived exactly that way. Every tile below decodes to
    /// pen 8, so "transparent" and "decoded" are distinguishable values.
    #[test]
    fn a_code_past_the_end_of_the_rom_is_transparent() {
        assert_eq!(tile_pen(&[], TileKind::Tile16x16, 0, 0, 0), TRANSPARENT_PEN);

        let mut rom = vec![0u8; 128];
        rom[3] = 0x80; // plane 0, x = 0, y = 0 -> pen 8
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 0, 0, 0), 8, "in range");
        assert_eq!(
            tile_pen(&rom, TileKind::Tile16x16, 1, 0, 0),
            TRANSPARENT_PEN,
            "one tile past the end"
        );

        // A partially-present tile is transparent too, not half-decoded — even
        // though the bytes this pixel needs are all present. The unit is the
        // whole tile, because a later row of the same tile would index past the
        // slice and panic.
        let short = &rom[..127];
        assert_eq!(
            tile_pen(short, TileKind::Tile16x16, 0, 0, 0),
            TRANSPARENT_PEN,
            "one byte short of a whole tile"
        );
    }

    /// A code large enough to overflow `code * bytes()` is transparent, not a
    /// panic.
    ///
    /// A sprite record is 16 bits so this is unreachable from the object table,
    /// but `tile_pen` is public and the arithmetic is cheap to make total.
    #[test]
    fn an_enormous_code_is_transparent_rather_than_an_overflow() {
        let rom = vec![0xFFu8; 512];
        assert_eq!(
            tile_pen(&rom, TileKind::Tile32x32, u32::MAX, 0, 0),
            TRANSPARENT_PEN
        );
    }

    /// `TRANSPARENT_PEN` is 15, the pen MAME fills `m_empty_tile` with.
    ///
    /// `cps1_v.cpp:2551` memsets the empty tile to 0x0f, and every draw path
    /// treats pen 15 as transparent (`prio_transpen(..., 15)`). The literal is
    /// load-bearing: a `TRANSPARENT_PEN` of 0 would make out-of-range tiles draw
    /// solid colour-index 0 instead of nothing.
    #[test]
    fn the_transparent_pen_is_fifteen() {
        assert_eq!(TRANSPARENT_PEN, 0x0F);
    }
}
