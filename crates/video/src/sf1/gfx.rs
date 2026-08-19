//! Tile decoding, with the layout as data.
//!
//! SF1 has two `gfx_layout`s and four `GFXDECODE` entries over them
//! (`sf.cpp:701-729`): an 8×8 2-plane char layout and a 16×16 4-plane sprite
//! layout whose planes are split across the halves of its region. So the two
//! things `crate::tiles` hardcodes for CPS-1 — four planes, and a fixed
//! interleave selected by a tile size — are both variables here.
//!
//! This module therefore holds the layout the way MAME does: plane offsets,
//! x-offsets, a row step and a per-element increment, all in **bits**. A third
//! board adds a `GfxLayout` constant rather than a third arm of a `match`.
//!
//! # Bits, not bytes
//!
//! `RGN_FRAC` and every offset here are bit quantities. `digfx.cpp:149` sets
//! `region_length = 8 * region->bytes()`, `:185-186` computes
//! `region_length / charincrement * FRAC_NUM / FRAC_DEN`, and `:223` resolves a
//! plane offset as `FRAC_OFFSET(v) + region_length * num / den`. Reading any of
//! those in bytes divides the element count by eight.
//!
//! # MSB-first, and no xor mask
//!
//! `readbit` (`drawgfx.cpp:24`) is `src[bitnum / 8] & (0x80 >> (bitnum % 8))`.
//! `decode` (`drawgfx.cpp:289-318`) also xors the bit index with
//! `m_layout_xormask`, which is **0 for SF1**: `digfx.cpp:125` sets it from
//! `GFXENTRY_ISREVERSE` (which `gfx_sf` does not use) and otherwise ORs
//! 0x08/0x18/0x38 only for 2-, 4- and 8-byte-wide regions. All four of SF1's
//! are byte-wide. The mask is therefore absent from this module rather than
//! present and zero: a field that is always zero is a field no test can check.
//!
//! # Never panics on a guest code
//!
//! `gfx_element::get_data` is `assert(code < elements())`. Both tilemaps and the
//! object table are guest-writable, so that assert is reachable from a running
//! game — [`GfxLayout::pen`] returns `None` instead, and every caller decides
//! what an absent tile looks like.

/// One plane's bit offset within an element, possibly a fraction of the region.
///
/// `RGN_FRAC(num, den)` in a plane-offset list means "plus `num/den` of the
/// region's length in bits" (`digfx.cpp:223`). A plain offset is
/// `frac_num: 0, frac_den: 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneOffset {
    /// The offset within the element, in bits.
    pub bits: u32,
    /// Numerator of the region fraction added to [`PlaneOffset::bits`].
    pub frac_num: u32,
    /// Denominator of that fraction. Never zero.
    pub frac_den: u32,
}

/// A `gfx_layout`, in bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GfxLayout {
    /// Tile width in pixels.
    pub width: u32,
    /// Tile height in pixels.
    pub height: u32,
    /// Bitplanes per pixel. `plane_offsets.len()` equals this.
    pub planes: u32,
    /// Where each plane's bit lives, **plane 0 first**.
    ///
    /// ⚠️ Plane 0 is the **most significant** pen bit: `decode` uses
    /// `planebit = 1 << (planes - 1 - plane)` (`drawgfx.cpp:296`). Reversing
    /// this list mirrors every pen within its colour.
    pub plane_offsets: &'static [PlaneOffset],
    /// The bit offset of each pixel column. `x_offsets.len()` equals `width`.
    pub x_offsets: &'static [u32],
    /// Bits between one pixel row and the next.
    ///
    /// Both SF1 layouts use a uniform `STEP` for their y-offsets, so one stride
    /// replaces the list. A layout with an irregular y-offset table would need
    /// the list back; neither of SF1's does, and a `&[u32]` that is always
    /// arithmetic is a table nobody can read.
    pub y_step: u32,
    /// Bits per element — MAME's `charincrement`.
    pub char_increment: u32,
    /// `RGN_FRAC` numerator for the element count.
    pub frac_num: u32,
    /// `RGN_FRAC` denominator. Never zero.
    pub frac_den: u32,
}

/// `char_layout`, `sf.cpp:701-708`: 8×8, 2 planes, whole region.
pub const CHAR_LAYOUT: GfxLayout = GfxLayout {
    width: 8,
    height: 8,
    planes: 2,
    // `{4, 0}`
    plane_offsets: &[
        PlaneOffset {
            bits: 4,
            frac_num: 0,
            frac_den: 1,
        },
        PlaneOffset {
            bits: 0,
            frac_num: 0,
            frac_den: 1,
        },
    ],
    // `{STEP4(0,1), STEP4(4*2,1)}`
    x_offsets: &[0, 1, 2, 3, 8, 9, 10, 11],
    // `{STEP8(0,1*16)}`
    y_step: 16,
    // `16*8`
    char_increment: 128,
    frac_num: 1,
    frac_den: 1,
};

/// `sprite_layout`, `sf.cpp:710-722`: 16×16, 4 planes, `RGN_FRAC(1,2)`.
pub const SPRITE_LAYOUT: GfxLayout = GfxLayout {
    width: 16,
    height: 16,
    planes: 4,
    // `{4, 0, RGN_FRAC(1,2)+4, RGN_FRAC(1,2)}`
    plane_offsets: &[
        PlaneOffset {
            bits: 4,
            frac_num: 0,
            frac_den: 1,
        },
        PlaneOffset {
            bits: 0,
            frac_num: 0,
            frac_den: 1,
        },
        PlaneOffset {
            bits: 4,
            frac_num: 1,
            frac_den: 2,
        },
        PlaneOffset {
            bits: 0,
            frac_num: 1,
            frac_den: 2,
        },
    ],
    // `{STEP4(0,1), STEP4(4*2,1), STEP4(4*2*2*16,1), STEP4(4*2*2*16+8,1)}`
    x_offsets: &[
        0, 1, 2, 3, 8, 9, 10, 11, 256, 257, 258, 259, 264, 265, 266, 267,
    ],
    // `{STEP16(0,1*16)}`
    y_step: 16,
    // `64*8`
    char_increment: 512,
    frac_num: 1,
    frac_den: 2,
};

/// Colours per `GFXDECODE` entry — all four of SF1's are 16 (`sf.cpp:724-729`).
pub const COLOURS: u16 = 16;

impl GfxLayout {
    /// How many elements a region of `region_bytes` holds.
    ///
    /// `region_length / charincrement * FRAC_NUM / FRAC_DEN`, in bits
    /// (`digfx.cpp:185-186`). `u64` throughout: `0x1c0000 * 8` is 14.7 million,
    /// which fits a `u32`, but a larger region would not and the overflow would
    /// be a silent wrap in release.
    #[must_use]
    pub const fn elements(&self, region_bytes: usize) -> u32 {
        let bits = (region_bytes as u64) * 8;
        let raw = bits / (self.char_increment as u64);
        (raw * (self.frac_num as u64) / (self.frac_den as u64)) as u32
    }

    /// The pen of pixel `(x, y)` of element `code`, or `None`.
    ///
    /// `None` means the pixel is not in the region: `x` or `y` outside the tile,
    /// a code past the end, or an empty region. Never panics — see the module
    /// documentation for why that matters here specifically.
    #[must_use]
    pub fn pen(&self, rom: &[u8], code: u32, x: u32, y: u32) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let bits = (rom.len() as u64) * 8;
        let base = u64::from(code) * u64::from(self.char_increment);
        let row = u64::from(y) * u64::from(self.y_step);
        // `x < self.width` and `x_offsets.len() == width`, so this cannot be out
        // of bounds. The layouts are constants in this file and the invariant is
        // asserted by `the_two_layouts_are_mames_literals`.
        let col = u64::from(self.x_offsets[x as usize]);
        let mut pen = 0u8;
        for (plane, off) in self.plane_offsets.iter().enumerate() {
            // Plane 0 is the most significant pen bit — `drawgfx.cpp:296`.
            let planebit = 1u8 << (self.planes - 1 - plane as u32);
            let frac = bits * u64::from(off.frac_num) / u64::from(off.frac_den);
            let bit = base + u64::from(off.bits) + frac + row + col;
            // `?` rather than an index: a guest code puts this arbitrarily far
            // past the end.
            let byte = *rom.get((bit / 8) as usize)?;
            // MSB-first — `readbit`, `drawgfx.cpp:24`.
            if byte & (0x80 >> (bit % 8)) != 0 {
                pen |= planebit;
            }
        }
        Some(pen)
    }
}

/// One `GFXDECODE` entry: a region, a layout, and a colour base.
#[derive(Debug, Clone, Copy)]
pub struct GfxSet<'a> {
    /// The assembled graphics region.
    pub rom: &'a [u8],
    /// Which layout decodes it.
    pub layout: &'static GfxLayout,
    /// The first palette entry this set's colour 0 uses (`sf.cpp:724-729`).
    pub colour_base: u16,
}

impl GfxSet<'_> {
    /// Elements in this set's region.
    #[must_use]
    pub const fn elements(&self) -> u32 {
        self.layout.elements(self.rom.len())
    }

    /// Palette entries one colour occupies: `1 << planes`.
    ///
    /// `drawgfx.cpp:145`: `m_color_depth = m_color_granularity = 1 << gl.planes`.
    /// **16 for the sprite layout and 4 for the char layout** — the char layout
    /// uses only 64 of its 256 reserved entries, because two planes need four
    /// pens. A hardcoded 16 puts every text tile's colour four times too far up
    /// the palette.
    #[must_use]
    pub const fn granularity(&self) -> u16 {
        1u16 << self.layout.planes
    }

    /// The palette entry this set's colour `colour` starts at.
    ///
    /// `colorbase() + granularity() * (color % colors())` — the formula both
    /// `gfx_element::transpen` and `tile_data::set` (`tilemap.h:386-394`) use, so
    /// one function serves tiles and sprites.
    #[must_use]
    pub const fn palette_base(&self, colour: u16) -> u16 {
        self.colour_base + self.granularity() * (colour % COLOURS)
    }

    /// The pen of pixel `(x, y)` of `code`, or `None` — see [`GfxLayout::pen`].
    #[must_use]
    pub fn pen(&self, code: u32, x: u32, y: u32) -> Option<u8> {
        self.layout.pen(self.rom, code, x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two layouts, field by field, against `sf.cpp:701-722`.
    #[test]
    fn the_two_layouts_are_mames_literals() {
        assert_eq!((CHAR_LAYOUT.width, CHAR_LAYOUT.height), (8, 8));
        assert_eq!(CHAR_LAYOUT.planes, 2);
        assert_eq!(CHAR_LAYOUT.char_increment, 128, "16*8 bits");
        assert_eq!(CHAR_LAYOUT.y_step, 16, "STEP8(0, 1*16)");
        assert_eq!(CHAR_LAYOUT.x_offsets, &[0, 1, 2, 3, 8, 9, 10, 11]);
        assert_eq!((CHAR_LAYOUT.frac_num, CHAR_LAYOUT.frac_den), (1, 1));
        let cpo: Vec<(u32, u32, u32)> = CHAR_LAYOUT
            .plane_offsets
            .iter()
            .map(|p| (p.bits, p.frac_num, p.frac_den))
            .collect();
        assert_eq!(cpo, vec![(4, 0, 1), (0, 0, 1)], "{{4, 0}}");

        assert_eq!((SPRITE_LAYOUT.width, SPRITE_LAYOUT.height), (16, 16));
        assert_eq!(SPRITE_LAYOUT.planes, 4);
        assert_eq!(SPRITE_LAYOUT.char_increment, 512, "64*8 bits");
        assert_eq!(SPRITE_LAYOUT.y_step, 16, "STEP16(0, 1*16)");
        assert_eq!(
            SPRITE_LAYOUT.x_offsets,
            &[0, 1, 2, 3, 8, 9, 10, 11, 256, 257, 258, 259, 264, 265, 266, 267],
            "STEP4(0,1), STEP4(8,1), STEP4(256,1), STEP4(264,1)"
        );
        assert_eq!((SPRITE_LAYOUT.frac_num, SPRITE_LAYOUT.frac_den), (1, 2));
        let spo: Vec<(u32, u32, u32)> = SPRITE_LAYOUT
            .plane_offsets
            .iter()
            .map(|p| (p.bits, p.frac_num, p.frac_den))
            .collect();
        assert_eq!(
            spo,
            vec![(4, 0, 1), (0, 0, 1), (4, 1, 2), (0, 1, 2)],
            "{{4, 0, RGN_FRAC(1,2)+4, RGN_FRAC(1,2)}}"
        );
    }

    /// The four regions' element counts, each right-hand side a literal.
    ///
    /// `digfx.cpp:149` sets `region_length = 8 * region->bytes()` — **bits** —
    /// and `:185-186` is `region_length / charincrement * FRAC_NUM / FRAC_DEN`.
    /// Getting the bit/byte domain wrong divides every count by eight.
    #[test]
    fn the_four_gfx_regions_have_mames_element_counts() {
        assert_eq!(SPRITE_LAYOUT.elements(0x8_0000), 4_096, "gfx1");
        assert_eq!(SPRITE_LAYOUT.elements(0x10_0000), 8_192, "gfx2");
        assert_eq!(SPRITE_LAYOUT.elements(0x1c_0000), 14_336, "gfx3");
        assert_eq!(CHAR_LAYOUT.elements(0x4000), 1_024, "gfx4");
        // The bit domain, stated separately so the counts above cannot pass by
        // a compensating error: 0x1c0000 bytes is 14,680,064 bits, and
        // 14,680,064 / 512 = 28,672 raw elements which RGN_FRAC(1,2) halves.
        assert_eq!(0x1c_0000 * 8, 14_680_064);
        assert_eq!(14_680_064 / 512, 28_672);
        assert_eq!(28_672 / 2, 14_336);
    }

    /// Granularity is `1 << planes` — 16 for sprites, **4** for the char layout.
    ///
    /// `drawgfx.cpp:145`, `set_layout`: `m_color_depth = m_color_granularity =
    /// 1 << gl.planes`. A hardcoded 16 (which is what `layers.rs`'s
    /// `PEN_GRANULARITY` is) puts every text tile's colour in the wrong place.
    #[test]
    fn granularity_is_two_to_the_planes_and_differs_between_the_layouts() {
        let sprites = GfxSet {
            rom: &[],
            layout: &SPRITE_LAYOUT,
            colour_base: 512,
        };
        let text = GfxSet {
            rom: &[],
            layout: &CHAR_LAYOUT,
            colour_base: 768,
        };
        assert_eq!(sprites.granularity(), 16);
        assert_eq!(text.granularity(), 4);
        assert_eq!(COLOURS, 16, "gfx_sf gives every entry 16 colours");
    }

    /// `colorbase() + granularity() * (color % colors())`, per entry.
    ///
    /// `gfx_element::transpen` and `tile_data::set` (`tilemap.h:386-394`) use the
    /// same formula, so one function serves tiles and sprites.
    #[test]
    fn the_palette_base_formula_matches_both_gfx_paths() {
        let bg = GfxSet {
            rom: &[],
            layout: &SPRITE_LAYOUT,
            colour_base: 0,
        };
        let fg = GfxSet {
            rom: &[],
            layout: &SPRITE_LAYOUT,
            colour_base: 256,
        };
        let ob = GfxSet {
            rom: &[],
            layout: &SPRITE_LAYOUT,
            colour_base: 512,
        };
        let tx = GfxSet {
            rom: &[],
            layout: &CHAR_LAYOUT,
            colour_base: 768,
        };
        assert_eq!(bg.palette_base(0), 0);
        assert_eq!(bg.palette_base(1), 16);
        assert_eq!(fg.palette_base(0), 256);
        assert_eq!(ob.palette_base(15), 752, "512 + 16*15");
        assert_eq!(ob.palette_base(15) + 15, 767, "ends where gfx4 begins");
        // Granularity 4: colour 15 lands at 768 + 60 = 828, and the char
        // layout's four pens end at 831 — 64 of its 256 reserved entries.
        assert_eq!(tx.palette_base(15), 828);
        assert_eq!(tx.palette_base(15) + 3, 831);
        // `color % colors()`, not a mask on the raw value.
        assert_eq!(ob.palette_base(16), 512, "wraps at 16 colours");
        assert_eq!(ob.palette_base(0x1F), ob.palette_base(0x0F));
    }

    /// One char tile, byte for byte, against a hand-derived pen grid.
    ///
    /// Derivation. `decode` (`drawgfx.cpp:289-318`) walks
    /// `plane = 0..planes` with `planebit = 1 << (planes - 1 - plane)`, so
    /// plane 0 (offset **4**) is pen bit 1 and plane 1 (offset **0**) is pen
    /// bit 0. `readbit` (`drawgfx.cpp:24`) is MSB-first:
    /// `src[bit/8] & (0x80 >> (bit%8))`. Row 0's sixteen bits are two bytes:
    ///
    /// ```text
    ///          mask 0x80 0x40 0x20 0x10 | 0x08 0x04 0x02 0x01
    ///          bit     0    1    2    3 |    4    5    6    7
    /// byte 0:      plane 1, x = 0,1,2,3 | plane 0, x = 0,1,2,3
    /// byte 1:      plane 1, x = 4,5,6,7 | plane 0, x = 4,5,6,7
    /// ```
    ///
    /// A byte's **high** nibble is plane 1 (offset 0) and its **low** nibble is
    /// plane 0 (offset 4) — for the same four pixels, not the next four. So `0xF0`
    /// in byte 0 sets pen bit 0 for x=0..3 (pen 1), and `0x0F` in byte 1 sets pen
    /// bit 1 for x=4..7 (pen 2). A whole-byte `0xFF` would set both planes and give
    /// pen 3, which is what the second row below checks.
    #[test]
    fn a_char_tile_decodes_to_its_hand_written_pen_grid() {
        let mut rom = vec![0u8; 16]; // one tile: 128 bits
        rom[0] = 0xF0; // plane 1 -> pen bit 0, x = 0..3
        rom[1] = 0x0F; // plane 0 -> pen bit 1, x = 4..7
        rom[2] = 0xFF; // row 1: both planes, x = 0..3 -> pen 3
        let row0: Vec<u8> = (0..8)
            .map(|x| CHAR_LAYOUT.pen(&rom, 0, x, 0).unwrap())
            .collect();
        assert_eq!(row0, vec![1, 1, 1, 1, 2, 2, 2, 2]);
        let row1: Vec<u8> = (0..8)
            .map(|x| CHAR_LAYOUT.pen(&rom, 0, x, 1).unwrap())
            .collect();
        assert_eq!(row1, vec![3, 3, 3, 3, 0, 0, 0, 0]);
        // Every other row is blank, and row 7 is the last one inside the tile.
        for y in 2..8 {
            for x in 0..8 {
                assert_eq!(CHAR_LAYOUT.pen(&rom, 0, x, y), Some(0), "({x},{y})");
            }
        }
        assert_eq!(CHAR_LAYOUT.pen(&rom, 0, 8, 0), None, "x is out of the tile");
        assert_eq!(CHAR_LAYOUT.pen(&rom, 0, 0, 8), None, "y is out of the tile");
    }

    /// One sprite tile, with the half-region plane split and the 256-bit x jump.
    ///
    /// A 128-byte region is 1,024 bits, which is two raw 512-bit elements that
    /// `RGN_FRAC(1,2)` makes **one**: planes 0 and 1 come from bytes 0-63 and
    /// planes 2 and 3 from bytes 64-127, with `half = 512` bits = byte 64.
    ///
    /// Plane order: plane 0 (offset 4) is pen bit 3, plane 1 (offset 0) is pen
    /// bit 2, plane 2 (offset half+4) is pen bit 1, plane 3 (offset half) is pen
    /// bit 0.
    #[test]
    fn a_sprite_tile_splits_its_planes_at_the_region_midpoint() {
        let mut rom = vec![0u8; 128];
        assert_eq!(SPRITE_LAYOUT.elements(rom.len()), 1, "one element");
        // bit 0 -> plane 1 -> pen bit 2 (4); bit 4 -> plane 0 -> pen bit 3 (8).
        rom[0] = 0x88;
        // bit 512 -> plane 3 -> pen bit 0 (1); bit 516 -> plane 2 -> pen bit 1 (2).
        rom[64] = 0x88;
        assert_eq!(
            SPRITE_LAYOUT.pen(&rom, 0, 0, 0),
            Some(15),
            "all four planes"
        );
        assert_eq!(SPRITE_LAYOUT.pen(&rom, 0, 1, 0), Some(0));
        // Only the low half set: pens 12, not 3 — which is the assertion that
        // catches swapping the halves.
        let mut half_only = vec![0u8; 128];
        half_only[0] = 0x88;
        assert_eq!(SPRITE_LAYOUT.pen(&half_only, 0, 0, 0), Some(12));
        let mut top_only = vec![0u8; 128];
        top_only[64] = 0x88;
        assert_eq!(SPRITE_LAYOUT.pen(&top_only, 0, 0, 0), Some(3));
        // x = 8 jumps 256 bits = 32 bytes, which is the layout's third STEP4.
        let mut right = vec![0u8; 128];
        right[32] = 0x80; // bit 256 -> plane 1 -> pen bit 2
        assert_eq!(SPRITE_LAYOUT.pen(&right, 0, 8, 0), Some(4));
        assert_eq!(
            SPRITE_LAYOUT.pen(&right, 0, 0, 0),
            Some(0),
            "not the left half"
        );
        // y = 1 is 16 bits on, byte 2.
        let mut down = vec![0u8; 128];
        down[2] = 0x80;
        assert_eq!(SPRITE_LAYOUT.pen(&down, 0, 0, 1), Some(4));
        assert_eq!(SPRITE_LAYOUT.pen(&down, 0, 0, 0), Some(0));
    }

    /// A code past the end of the region returns `None` rather than panicking.
    ///
    /// MAME's `gfx_element::get_data` is `assert(code < elements())`
    /// (`drawgfx.h`), which is a host crash on a value the guest chooses. The
    /// object table and both tilemaps are guest-writable, so this path is
    /// reachable from a running game and must not panic — the workspace rule.
    #[test]
    fn a_code_past_the_region_returns_none_and_never_panics() {
        let rom = vec![0xFFu8; 128];
        assert_eq!(SPRITE_LAYOUT.elements(rom.len()), 1);
        assert_eq!(SPRITE_LAYOUT.pen(&rom, 0, 0, 0), Some(15));
        assert_eq!(SPRITE_LAYOUT.pen(&rom, 1, 0, 0), None, "one past the end");
        assert_eq!(
            SPRITE_LAYOUT.pen(&rom, u32::MAX, 0, 0),
            None,
            "and wildly past"
        );
        // An empty region — a machine built with no graphics — is absent, not a
        // panic and not pen 0.
        assert_eq!(SPRITE_LAYOUT.pen(&[], 0, 0, 0), None);
        assert_eq!(CHAR_LAYOUT.pen(&[], 0, 0, 0), None);
        assert_eq!(SPRITE_LAYOUT.elements(0), 0);
    }

    /// `GfxSet` forwards to its layout and adds the colour base.
    #[test]
    fn a_gfx_set_pairs_a_region_with_a_layout_and_a_base() {
        let mut rom = vec![0u8; 128];
        rom[0] = 0x88;
        rom[64] = 0x88;
        let g = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 512,
        };
        assert_eq!(g.elements(), 1);
        assert_eq!(g.pen(0, 0, 0), Some(15));
        assert_eq!(g.pen(1, 0, 0), None);
        assert_eq!(g.palette_base(3) + 15, 512 + 48 + 15);
    }
}
