//! The three scroll layers: scan mappers, tile fetch, scroll, and flip.
//!
//! Each layer is a 64×64 grid of tiles — 8×8, 16×16, or 32×32 — held in gfxram
//! as two words per tile, a code and an attribute. A *scan mapper* turns a
//! `(col, row)` pair into that tile's index in the layer's table; MAME creates
//! the three tilemaps at `cps1_v.cpp:2545-2547` and its mappers are at
//! `:2433-2450`.
//!
//! # Pen granularity is 16
//!
//! A drawn pixel's palette pen is `colour * 16 + pen`, where `colour` is the
//! tile's colour scheme and `pen` its 4-bit pixel. The 16 is `gfx_element`'s
//! granularity for a 4-bits-per-pixel layout, and four readings of the reference
//! agree on it:
//!
//! - `GFXDECODE_ENTRY(..., 0, 0x80)` (`cps1.cpp:3882-3885`) gives 0x80 colour
//!   schemes, and 0x80 × 16 = 0x800 — exactly the four users' 0x20 schemes each.
//! - The star layers write pens `0x800 + col` and `0xa00 + col`
//!   (`cps1_v.cpp:2900`, `:2926`), immediately above that 0x800. At a
//!   granularity of 32 the tile pens would reach 0x1000 and swallow them.
//! - `set_entries(0xc00)` (`cps1.cpp:3932`) is 0x800 tile pens plus 0x400 star
//!   pens.
//! - [`crate::palette::BACKGROUND_PEN`] 0xBFF then falls in the star region,
//!   unreachable from a tilemap — which is what makes "a solid layer hides the
//!   background" a real invariant rather than a coincidence.
//!
//! The `32` in `m_palette_size = CPS1_PALETTE_ENTRIES * 32` (`cps1_v.cpp:2542`)
//! is bytes per scheme in gfxram, two per entry, and is the trap here.

use crate::bank::{BankMapper, GfxType};
use crate::regs::{cps_a_base, OBJ_BOUNDARY, OTHER_BASE, ROWSCROLL_OFFS};
use crate::tiles::{tile_pen, TileKind, TRANSPARENT_PEN};
use crate::{HEIGHT, VISIBLE_X, VISIBLE_Y, WIDTH};

/// Tiles along each edge of a layer's map (`cps1_v.cpp:2545`: `64, 64`).
pub const MAP_TILES: u32 = 64;

/// Palette pens per colour scheme, for a 4-bits-per-pixel layout.
///
/// See the module documentation for why this is 16 and not 32.
pub const PEN_GRANULARITY: u16 = 16;

/// Words in the row-scroll table — `m_other_size` is 0x800 bytes
/// (`cps1_v.cpp:2540`), and MAME indexes it `& 0x3ff` (`:3027`).
const ROWSCROLL_WORDS: usize = 0x400;

/// Which of the three scroll layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// The 8×8 layer, usually text and the HUD.
    Scroll1,
    /// The 16×16 layer, the only one with row scroll.
    Scroll2,
    /// The 32×32 layer, usually the far background.
    Scroll3,
}

impl Layer {
    /// The graphics-ROM type this layer's codes are mapped through.
    pub const fn gfx_type(self) -> GfxType {
        match self {
            Self::Scroll1 => GfxType::Scroll1,
            Self::Scroll2 => GfxType::Scroll2,
            Self::Scroll3 => GfxType::Scroll3,
        }
    }

    /// The colour scheme added to a tile's low five attribute bits.
    ///
    /// `cps1_v.cpp:2466`, `:2485`, `:2502`. The bases partition the 0x80 schemes:
    /// sprites take 0x00-0x1F and the three layers 0x20, 0x40, 0x60 upward.
    pub const fn colour_base(self) -> u16 {
        match self {
            Self::Scroll1 => 0x20,
            Self::Scroll2 => 0x40,
            Self::Scroll3 => 0x60,
        }
    }

    /// The tile's edge in pixels.
    pub const fn tile_edge(self) -> u32 {
        match self {
            Self::Scroll1 => 8,
            Self::Scroll2 => 16,
            Self::Scroll3 => 32,
        }
    }

    /// The mask applied to a code word before the bank mapper sees it.
    ///
    /// Only scroll 3 masks (`cps1_v.cpp:2495`: `& 0x3fff`). Scrolls 1 and 2 take
    /// the whole word (`:2453`, `:2477`), which `0xFFFF` states without a special
    /// case at the call site.
    pub const fn code_mask(self) -> u32 {
        match self {
            Self::Scroll1 | Self::Scroll2 => 0xFFFF,
            Self::Scroll3 => 0x3FFF,
        }
    }

    /// The tile's index in the layer's table, from its map coordinates.
    ///
    /// `cps1_v.cpp:2433-2450`. Each is a permutation of `0..0x1000`, which
    /// `tests::every_scan_mapper_is_a_bijection_over_the_tile_grid` checks:
    /// transcribed arithmetic with a wrong shift still draws a plausible
    /// picture, but it cannot stay a bijection.
    pub fn scan(self, col: u32, row: u32) -> usize {
        (match self {
            Self::Scroll1 => (row & 0x1F) + ((col & 0x3F) << 5) + ((row & 0x20) << 6),
            Self::Scroll2 => (row & 0x0F) + ((col & 0x3F) << 4) + ((row & 0x30) << 6),
            Self::Scroll3 => (row & 0x07) + ((col & 0x3F) << 3) + ((row & 0x38) << 6),
        }) as usize
    }
}

/// One tile's table entry, decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileInfo {
    /// The code word, masked by [`Layer::code_mask`]. Not yet bank-mapped.
    pub code: u32,
    /// The colour scheme, `colour_base + (attr & 0x1F)`.
    pub colour: u16,
    /// Mirror the tile horizontally (attribute bit 5).
    pub flip_x: bool,
    /// Mirror the tile vertically (attribute bit 6).
    pub flip_y: bool,
    /// Priority group 0-3 (attribute bits 7-8), selecting which of the four
    /// CPS-B priority registers decides whether this tile's pens occlude
    /// sprites.
    pub group: u8,
}

/// Reads one tile's entry out of a layer's table.
///
/// `table` is a **word** index into `gfxram`, as [`cps_a_base`] returns. The
/// reads wrap, because a CPS-A base register can point past gfxram entirely —
/// see `regs::tests::cps_a_base_can_point_past_gfxram_so_callers_must_wrap`.
pub fn tile_info(gfxram: &[u16], table: usize, layer: Layer, col: u32, row: u32) -> TileInfo {
    let i = table + 2 * layer.scan(col, row);
    let n = gfxram.len();
    let code = u32::from(gfxram[i % n]) & layer.code_mask();
    let attr = gfxram[(i + 1) % n];
    TileInfo {
        code,
        colour: layer.colour_base() + (attr & 0x1F),
        flip_x: attr & 0x20 != 0,
        flip_y: attr & 0x40 != 0,
        group: ((attr & 0x0180) >> 7) as u8,
    }
}

/// A layer's horizontal scroll per screen row, and its one vertical scroll.
///
/// Scroll 2 is the only layer with per-row horizontal scroll: `tilemap[1]` is
/// the one MAME calls `set_scroll_rows(1024)` on (`cps1_v.cpp:3022`), and
/// scrolls 1 and 3 take a single `set_scrollx` at `:3015` and `:3035`. Carrying
/// the per-row array for all three lets [`draw_tilemap`] have one code path
/// instead of a flag it could get the wrong way round.
#[derive(Debug, Clone, Copy)]
pub struct ScrollRows {
    /// The vertical scroll, shared by every row.
    pub scroll_y: i32,
    /// The horizontal scroll of each visible screen row.
    pub x: [i32; HEIGHT],
}

impl ScrollRows {
    /// The same horizontal scroll on every row — row scroll disabled, or a layer
    /// that never had it.
    pub fn flat(scroll_x: i32, scroll_y: i32) -> Self {
        Self {
            scroll_y,
            x: [scroll_x; HEIGHT],
        }
    }

    /// Per-row horizontal scroll read from the row-scroll table in gfxram.
    ///
    /// `x[y] = scroll_x + other[(y + VISIBLE_Y + ROWSCROLL_OFFS) & 0x3FF]`, with
    /// **no `scroll_y` term**: the table is indexed by raster row, and visible
    /// row `y` is raster row `y + VISIBLE_Y`. The derivation — and why the obvious
    /// reading of MAME's line is wrong — is on
    /// `tests::row_scroll_reads_a_per_line_offset_independent_of_the_vertical_scroll`.
    pub fn row_scrolled(gfxram: &[u16], cps_a: &[u16], scroll_x: i32, scroll_y: i32) -> Self {
        let base = cps_a_base(cps_a, OTHER_BASE, OBJ_BOUNDARY);
        let offs = usize::from(cps_a[ROWSCROLL_OFFS]);
        let n = gfxram.len();
        let mut x = [scroll_x; HEIGHT];
        for (y, slot) in x.iter_mut().enumerate() {
            // Indexed by **raster** row: visible row `y` is raster row
            // `y + VISIBLE_Y`. See the derivation on
            // `tests::row_scroll_reads_a_per_line_offset_independent_of_the_vertical_scroll`.
            let entry = (y + VISIBLE_Y as usize + offs) & (ROWSCROLL_WORDS - 1);
            *slot = scroll_x + i32::from(gfxram[(base + entry) % n]);
        }
        Self { scroll_y, x }
    }
}

/// One axis of a layer's map coordinate: `(tile, offset within the tile)`.
///
/// `edge` is the layer's tile edge in pixels and `raster` a **raster** position — the
/// caller has already added [`crate::VISIBLE_X`] or [`crate::VISIBLE_Y`] and the
/// scroll. Signed throughout, with Euclidean division, so a negative scroll is the
/// same arithmetic as a positive one rather than a branch: 0xFFC0 is −64, and `-1 / 16`
/// truncating to 0 would put the wrong tile at the left edge of the screen. The tile
/// wraps at [`MAP_TILES`], because a layer's map is 64×64 and a scroll past its span
/// shows the map again.
///
/// The `rem_euclid(MAP_TILES)` cannot be killed by a test, and that is provable rather
/// than a gap: the returned tile reaches nothing but [`Layer::scan`], whose masks
/// already cover exactly six bits (0x3F for the column; 0x1F|0x20, 0x0F|0x30,
/// 0x07|0x38 for the row), and for a power-of-two modulus the low bits of a
/// two's-complement `as u32` are the mathematical remainder — negatives included. It
/// stays because it makes the returned tile the in-range value its name claims, rather
/// than leaving that to a mask two functions away.
///
/// Published because a graphics viewer must name the tile the renderer fetched, and
/// four decisions live in these two lines: the raster bias's absence, `div_euclid`,
/// `rem_euclid`, and the wrap. This crate had to correct its raster-coordinate
/// doctrine three times; a viewer with a fourth reading of it would report a tile that
/// was never drawn, which is a diagnostic that lies exactly when it is being trusted.
pub fn map_axis(edge: u32, raster: i32) -> (u32, u32) {
    let step = edge as i32;
    let tile = raster.div_euclid(step).rem_euclid(MAP_TILES as i32) as u32;
    let offset = raster.rem_euclid(step) as u32;
    (tile, offset)
}

/// Draws one scroll layer into `pens`, skipping transparent pixels.
///
/// `pens` and `prio` are `WIDTH * HEIGHT`, row-major. A pixel is written only
/// when its tile decodes to something other than [`TRANSPARENT_PEN`], so the
/// layers composite by drawing back to front with no per-layer buffer.
///
/// The horizontal scroll arrives only through `rows`, never as a separate
/// argument: two sources for one quantity is a disagreement waiting to happen,
/// and [`ScrollRows::flat`] covers the layers that have no per-row scroll.
///
/// `prio[i]` is set to 1 when bit `pen` of `hi_pens[group]` is set — that pen of
/// that tile group occludes sprites. Task 8 supplies the registers; a caller
/// passing `[0; 4]` gets no occlusion. `prio` is only ever set, never cleared,
/// which matches MAME: only the single layer immediately below the sprites
/// contributes to the priority bitmap (`cps1_v.cpp:2985-2996`), so no two layers
/// can disagree about a pixel.
///
/// `table` is a **word** index into `gfxram`, as [`cps_a_base`] returns.
// Nine parameters, and a struct wrapping them would be built at one call site
// and destructured at the other. The alternative to the arity is hidden state.
#[allow(clippy::too_many_arguments)]
pub fn draw_tilemap(
    pens: &mut [u16],
    prio: &mut [u8],
    gfxram: &[u16],
    gfx: &[u8],
    mapper: &BankMapper,
    table: usize,
    layer: Layer,
    rows: &ScrollRows,
    hi_pens: &[u16; 4],
) {
    assert_eq!(pens.len(), WIDTH * HEIGHT, "pens is the visible frame");
    assert_eq!(prio.len(), WIDTH * HEIGHT, "prio is the visible frame");

    let edge = layer.tile_edge();
    let step = edge as i32;
    let gfx_type = layer.gfx_type();
    let screen_w = WIDTH as i32;

    for y in 0..HEIGHT {
        // Visible row `y` is raster row `y + VISIBLE_Y`, and the scroll registers
        // are raster-space — see the crate documentation.
        let (row, ty) = map_axis(edge, y as i32 + VISIBLE_Y + rows.scroll_y);
        let scroll_x = rows.x[y];

        let mut x = 0i32;
        while x < screen_w {
            let (col, tx0) = map_axis(edge, x + VISIBLE_X + scroll_x);
            // The pixels of this tile still on screen: the rest of the tile, or
            // the rest of the line, whichever ends first.
            let run = (step - tx0 as i32).min(screen_w - x);

            let info = tile_info(gfxram, table, layer, col, row);
            let Some(code) = mapper.map(gfx_type, info.code) else {
                // No ROM behind the code: the whole tile is absent, not tile 0.
                x += run;
                continue;
            };

            // Scroll 1's two 8×8 layouts share a 64-byte frame, and
            // `BIT(tile_index, 5)` picks between them (`cps1_v.cpp:2461`). Under
            // scroll 1's scan mapper that bit is the column's low bit; written as
            // the mapper's own bit so the citation and the code say the same
            // thing.
            let kind = if layer == Layer::Scroll1 && layer.scan(col, row) & 0x20 != 0 {
                TileKind::Tile8x8Odd
            } else {
                gfx_type.tile_kind()
            };

            let ty_eff = if info.flip_y { edge - 1 - ty } else { ty };
            let hi = hi_pens[usize::from(info.group)];
            let base = y * WIDTH + x as usize;

            for k in 0..run {
                let tx = tx0 + k as u32;
                let tx_eff = if info.flip_x { edge - 1 - tx } else { tx };
                let pen = tile_pen(gfx, kind, code, tx_eff, ty_eff);
                if pen == TRANSPARENT_PEN {
                    continue;
                }
                let i = base + k as usize;
                pens[i] = info.colour * PEN_GRANULARITY + u16::from(pen);
                if hi & (1u16 << pen) != 0 {
                    prio[i] = 1;
                }
            }
            x += run;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::BankRange;
    use crate::palette::BACKGROUND_PEN;
    use std::collections::HashSet;

    /// gfxram's size in words, 192 KB (`cps1.cpp:592`).
    const GFXRAM_WORDS: usize = 0x1_8000;

    /// Each scan mapper is a bijection over the 64×64 tile grid.
    ///
    /// This is the test that catches the failure mode a rendered image would only
    /// show as "slightly wrong somewhere". The mappers are transcribed arithmetic
    /// (`cps1_v.cpp:2433-2450`), and a wrong shift or mask still produces a
    /// plausible picture — layers offset by a few tiles, or wrapping at the wrong
    /// column. It cannot produce a permutation of `0..0x1000`.
    #[test]
    fn every_scan_mapper_is_a_bijection_over_the_tile_grid() {
        for layer in [Layer::Scroll1, Layer::Scroll2, Layer::Scroll3] {
            let mut seen = HashSet::new();
            for col in 0..MAP_TILES {
                for row in 0..MAP_TILES {
                    let i = layer.scan(col, row);
                    assert!(i < 0x1000, "{layer:?} ({col},{row}) gave {i:#x}");
                    assert!(seen.insert(i), "{layer:?} collides at ({col},{row})");
                }
            }
            assert_eq!(seen.len(), 0x1000, "{layer:?} covers the whole grid");
        }
    }

    /// The mappers, at a handful of hand-computed points.
    ///
    /// The bijection test above proves each is *a* permutation; these pin *which*
    /// one, against `cps1_v.cpp:2433`, `:2439`, `:2445`.
    #[test]
    fn the_scan_mappers_are_the_ones_mame_documents() {
        // scroll1: (row & 0x1f) + ((col & 0x3f) << 5) + ((row & 0x20) << 6)
        assert_eq!(Layer::Scroll1.scan(0, 0), 0);
        assert_eq!(Layer::Scroll1.scan(0, 1), 1);
        assert_eq!(Layer::Scroll1.scan(1, 0), 0x20);
        assert_eq!(Layer::Scroll1.scan(0, 32), 0x800);
        assert_eq!(Layer::Scroll1.scan(63, 63), 0x800 + 31 + (63 << 5));
        // scroll2: (row & 0x0f) + ((col & 0x3f) << 4) + ((row & 0x30) << 6)
        assert_eq!(Layer::Scroll2.scan(0, 0), 0);
        assert_eq!(Layer::Scroll2.scan(1, 0), 0x10);
        assert_eq!(Layer::Scroll2.scan(0, 16), 0x400);
        assert_eq!(Layer::Scroll2.scan(0, 48), 0xC00);
        // scroll3: (row & 0x07) + ((col & 0x3f) << 3) + ((row & 0x38) << 6)
        assert_eq!(Layer::Scroll3.scan(0, 0), 0);
        assert_eq!(Layer::Scroll3.scan(1, 0), 0x08);
        assert_eq!(Layer::Scroll3.scan(0, 8), 0x200);
        assert_eq!(Layer::Scroll3.scan(0, 56), 0xE00);
    }

    /// Colour bases separate the layers' pens, and scroll3 masks its code.
    ///
    /// `cps1_v.cpp:2466`, `:2485`, `:2502`: the colour scheme is `(attr & 0x1f)`
    /// plus 0x20, 0x40, or 0x60, so a pen is `scheme * 16 + pixel` and the four
    /// users of the palette never overlap. Sprites take 0x00-0x1F.
    #[test]
    fn the_layer_colour_bases_partition_the_palette() {
        assert_eq!(Layer::Scroll1.colour_base(), 0x20);
        assert_eq!(Layer::Scroll2.colour_base(), 0x40);
        assert_eq!(Layer::Scroll3.colour_base(), 0x60);
        assert_eq!(Layer::Scroll3.code_mask(), 0x3FFF, "cps1_v.cpp:2495");
        assert_eq!(Layer::Scroll1.code_mask(), 0xFFFF);
        assert_eq!(Layer::Scroll2.code_mask(), 0xFFFF);
        assert_eq!(Layer::Scroll1.tile_edge(), 8);
        assert_eq!(Layer::Scroll2.tile_edge(), 16);
        assert_eq!(Layer::Scroll3.tile_edge(), 32);

        // The highest pen any tilemap can produce is (0x1F + 0x60) * 16 + 15 =
        // 0x7FF. The multiplier is 16 because the layouts are 4bpp
        // (`cps1.cpp:3837-3878`), and 0x80 colour codes (`cps1.cpp:3882-3885`)
        // × 16 = 0x800 is exactly the tile pen region. The star layers occupy
        // 0x800-0xBFF (`cps1_v.cpp:2900`, `:2926`).
        assert_eq!(PEN_GRANULARITY, 16);
        let max_pen = (Layer::Scroll3.colour_base() + 0x1F) * PEN_GRANULARITY + 15;
        assert_eq!(max_pen, 0x7FF);
        assert!(
            max_pen < BACKGROUND_PEN,
            "no tilemap pen can equal the background pen, which is what makes \
             'a solid layer hides the background' a real invariant"
        );
    }

    /// A tile entry is a code word and an attribute word, two words apart.
    #[test]
    fn a_tile_entry_is_a_code_and_an_attribute() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        // scroll2 tile at (col 1, row 0) -> index 0x10, words 0x20 and 0x21.
        gfxram[0x20] = 0x1234;
        // 0x05 colour | 0x20 X flip | 0x40 Y flip | 0x180 group 3.
        gfxram[0x21] = 0x01E5;
        let t = tile_info(&gfxram, 0, Layer::Scroll2, 1, 0);
        assert_eq!(t.code, 0x1234);
        assert_eq!(t.colour, 0x40 + 5);
        assert!(t.flip_x, "attr bit 5");
        assert!(t.flip_y, "attr bit 6");
        assert_eq!(t.group, 3, "(attr & 0x0180) >> 7");

        // Group and flip are independent: 0x0100 is group 2, no flip.
        gfxram[0x21] = 0x0100;
        let t = tile_info(&gfxram, 0, Layer::Scroll2, 1, 0);
        assert_eq!((t.group, t.flip_x, t.flip_y), (2, false, false));
        // X flip alone, and Y flip alone — so the two bits cannot be swapped.
        gfxram[0x21] = 0x0020;
        let t = tile_info(&gfxram, 0, Layer::Scroll2, 1, 0);
        assert_eq!((t.flip_x, t.flip_y), (true, false));
        gfxram[0x21] = 0x0040;
        let t = tile_info(&gfxram, 0, Layer::Scroll2, 1, 0);
        assert_eq!((t.flip_x, t.flip_y), (false, true));
        // Group 1 is bit 7 alone, so `>> 7` cannot be `>> 8`.
        gfxram[0x21] = 0x0080;
        assert_eq!(tile_info(&gfxram, 0, Layer::Scroll2, 1, 0).group, 1);

        // Both masks are checked at their *upper* edge, because every other case
        // here and every drawing fixture in this crate uses small values, and a
        // mask only fails where it stops.
        //
        // Colour bit 4: scheme 0x1F is the top of `attr & 0x1f`
        // (`cps1_v.cpp:2485`), and it is the only bit a 0x0F mask would drop.
        gfxram[0x21] = 0x001F;
        assert_eq!(
            tile_info(&gfxram, 0, Layer::Scroll2, 1, 0).colour,
            0x40 + 0x1F,
            "the colour mask reaches bit 4, so 0x1F is not 0x0F"
        );
        // And bit 9 is *not* part of the group. The field is two bits, so group 3
        // is the maximum; a mask widened to 0x0380 would read this as 7 and index
        // past `hi_pens`, which is `[u16; 4]`.
        gfxram[0x21] = 0x0380;
        assert_eq!(
            tile_info(&gfxram, 0, Layer::Scroll2, 1, 0).group,
            3,
            "the group is two bits (`attr & 0x0180`), so bit 9 is not in it"
        );

        // Scroll3 masks its code; scroll2 does not.
        gfxram[0x20] = 0xFFFF;
        assert_eq!(tile_info(&gfxram, 0, Layer::Scroll2, 1, 0).code, 0xFFFF);
        gfxram[0x10] = 0xFFFF; // scroll3 (col 1, row 0) -> index 8, word 0x10
        assert_eq!(tile_info(&gfxram, 0, Layer::Scroll3, 1, 0).code, 0x3FFF);
    }

    /// The table base offsets the whole layer, and the read wraps.
    #[test]
    fn the_table_base_offsets_the_layer_and_wraps() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        gfxram[0x2000] = 0x0777;
        gfxram[0x2001] = 0x0003;
        let t = tile_info(&gfxram, 0x2000, Layer::Scroll2, 0, 0);
        assert_eq!((t.code, t.colour), (0x0777, 0x43));

        // A base past the end of gfxram wraps rather than panicking, for the
        // reason `cps_a_base`'s own test records.
        gfxram[0] = 0x0123;
        let t = tile_info(&gfxram, GFXRAM_WORDS, Layer::Scroll2, 0, 0);
        assert_eq!(t.code, 0x0123, "the wrapped word");
    }

    /// One 16×16 tile lands where the scroll registers put it, right way up.
    ///
    /// A single solid tile with the scroll placing map (0, 0) at the visible
    /// origin must occupy visible pixels (0,0)-(15,15) and nothing else, and a
    /// scroll of 5 further right must shift it **left**, because the scroll
    /// register is the coordinate of the screen's left edge within the map.
    #[test]
    fn a_tile_lands_where_the_scroll_registers_put_it() {
        let f = Fixture::one_solid_tile(Layer::Scroll2, 0x0A);
        let want = 0x40 * PEN_GRANULARITY + 0x0A;

        let r = f.render(0, 0);
        assert_eq!(r.px(0, 0), Some(want));
        assert_eq!(r.px(15, 15), Some(want));
        assert_eq!(r.px(16, 0), None, "the next tile is blank");
        assert_eq!(r.px(0, 16), None);

        let r = f.render(5, 0);
        assert_eq!(r.px(0, 0), Some(want), "still covered");
        assert_eq!(r.px(10, 15), Some(want), "right edge moved left 5");
        assert_eq!(r.px(11, 0), None);

        let r = f.render(0, 3);
        assert_eq!(r.px(0, 12), Some(want), "bottom edge moved up 3");
        assert_eq!(r.px(0, 13), None);

        // With the raw register values the hardware holds, a zero scroll puts map
        // (0, 0) at the raster origin — inside blanking, so nothing is visible.
        assert!(
            f.render_raw(0, 0).is_blank(),
            "map (0,0) at a zero scroll is behind the blanking region"
        );
    }

    /// The visible window is the raster sub-rectangle at (64, 16).
    ///
    /// This is the assertion that fixes the offset, stated against literals
    /// rather than against [`VISIBLE_X`]/[`VISIBLE_Y`] themselves — so changing
    /// those constants fails here instead of silently moving every layer. A tile
    /// placed at map pixel (64, 16) must land at visible (0, 0) with no scroll
    /// at all.
    #[test]
    fn a_layers_origin_is_the_visible_window_not_the_raster_origin() {
        assert_eq!((VISIBLE_X, VISIBLE_Y), (64, 16), "cps1.h:42, :46");

        let mut f = Fixture::with_gfx(Layer::Scroll2, frame_bytes(Layer::Scroll2, 0x0A));
        for e in f.gfxram.chunks_mut(2) {
            e[0] = BLANK_CODE;
        }
        // Map tile (4, 1) covers map pixels x 64..79, y 16..31 — exactly the
        // visible window's first 16x16 cell.
        let i = 2 * Layer::Scroll2.scan(4, 1);
        f.gfxram[i] = SOLID_CODE;
        f.gfxram[i + 1] = 0;

        // The raw registers, zeroed: map (0, 0) sits at the raster origin.
        let r = f.render_raw(0, 0);
        let want = 0x40 * PEN_GRANULARITY + 0x0A;
        assert_eq!(r.px(0, 0), Some(want), "map (64,16) is visible (0,0)");
        assert_eq!(r.px(15, 15), Some(want));
        assert_eq!(r.px(16, 15), None, "and only that one tile");
        assert_eq!(r.px(15, 16), None);
        assert_eq!(
            r.opaque(),
            16 * 16,
            "exactly one tile drew, so the offset is (64,16) and not a near miss"
        );
    }

    /// A transparent pen leaves the framebuffer alone.
    ///
    /// Pen 15 within a tile is transparent (`prio_transpen(..., 15)`), so a tile
    /// whose pixels are all 15 draws nothing at all — distinct from drawing
    /// colour-index 15, which would be a solid block.
    #[test]
    fn pen_fifteen_within_a_tile_is_transparent() {
        let f = Fixture::one_solid_tile(Layer::Scroll2, 0x0F);
        let r = f.render(0, 0);
        assert_eq!(r.px(0, 0), None, "an all-pen-15 tile draws nothing");
        assert_eq!(r.px(8, 8), None);
        // Pen 14 in the same position does draw, so the skip is keyed to 15 and
        // not to "the top of the range".
        let f = Fixture::one_solid_tile(Layer::Scroll2, 0x0E);
        let r = f.render(0, 0);
        assert_eq!(r.px(0, 0), Some(0x40 * PEN_GRANULARITY + 0x0E));
    }

    /// Per-tile flip mirrors the tile in place.
    #[test]
    fn per_tile_flip_mirrors_within_the_tile() {
        // A tile whose only non-transparent pixel is (0, 0), pen 1.
        let mut f = Fixture::one_corner_tile(Layer::Scroll2);
        let want = 0x40 * PEN_GRANULARITY + 1;

        let r = f.render(0, 0);
        assert_eq!(r.px(0, 0), Some(want));
        assert_eq!(r.px(15, 0), None);
        assert_eq!(r.px(0, 15), None);

        f.set_attr(0x0020); // X flip
        let r = f.render(0, 0);
        assert_eq!(r.px(0, 0), None);
        assert_eq!(r.px(15, 0), Some(want));
        assert_eq!(
            r.px(0, 15),
            None,
            "X flip alone does not move it vertically"
        );

        f.set_attr(0x0040); // Y flip
        let r = f.render(0, 0);
        assert_eq!(r.px(0, 0), None);
        assert_eq!(r.px(0, 15), Some(want));
        assert_eq!(r.px(15, 0), None);

        f.set_attr(0x0060); // both
        let r = f.render(0, 0);
        assert_eq!(r.px(15, 15), Some(want));
        assert_eq!(r.px(0, 0), None);
    }

    /// A layer of solid tiles covers all 384×224 pixels, with no seams.
    ///
    /// The gap this catches is the one an image shows as a faint grid: a loop
    /// bound of `<` where it should be `<=`, or a tile stride short by one,
    /// leaves single-pixel lines undrawn. The negative and 511 scrolls make the
    /// wrap arithmetic load-bearing rather than incidental.
    #[test]
    fn a_solid_layer_leaves_no_pixel_undrawn() {
        for layer in [Layer::Scroll1, Layer::Scroll2, Layer::Scroll3] {
            let f = Fixture::solid_everywhere(layer, 0x0A);
            let want = layer.colour_base() * PEN_GRANULARITY + 0x0A;
            for (sx, sy) in [(0i32, 0i32), (1, 1), (7, 13), (-3, -5), (511, 511)] {
                let r = f.render(sx, sy);
                for y in 0..HEIGHT {
                    for x in 0..WIDTH {
                        assert_eq!(
                            r.px(x, y),
                            Some(want),
                            "{layer:?} scroll ({sx},{sy}) at ({x},{y})"
                        );
                    }
                }
            }
        }
    }

    /// `map_axis` is Euclidean, unbiased, and wraps at the map's edge.
    ///
    /// Four decisions live here and a viewer that re-derived them would get one wrong:
    /// the wrap at [`MAP_TILES`], `div_euclid` rather than `/`, `rem_euclid` rather
    /// than `%`, and no bias of its own — the caller adds `VISIBLE_X` or `VISIBLE_Y`.
    /// Every expectation below is computed by hand.
    #[test]
    fn map_axis_is_euclidean_and_wraps_at_the_map_edge() {
        // 16-pixel tiles. Raster 0 is tile 0, pixel 0, so this function adds no
        // bias of its own.
        assert_eq!(map_axis(16, 0), (0, 0));
        assert_eq!(map_axis(16, 15), (0, 15), "the last pixel of tile 0");
        assert_eq!(map_axis(16, 16), (1, 0), "the first pixel of tile 1");
        assert_eq!(map_axis(16, 40), (2, 8));

        // Negative: `/` truncates toward zero and would give tile 0 for −1, with `%`
        // giving pixel −1 — which as a `u32` is 4294967295. Euclidean gives the last
        // pixel of the tile *before* tile 0, which after the wrap is tile 63.
        assert_eq!(map_axis(16, -1), (63, 15));
        assert_eq!(map_axis(16, -16), (63, 0));
        assert_eq!(map_axis(16, -64), (60, 0), "SF2's bootleg scroll1xoff");

        // The wrap: 64 tiles of 16 pixels is 1024, and 1024 is tile 0 again.
        assert_eq!(map_axis(16, 1024), (0, 0));
        assert_eq!(map_axis(16, 1024 + 40), (2, 8), "the same as raster 40");
        assert_eq!(map_axis(16, -1024), (0, 0), "and a whole map back");

        // The other two edges, whose spans differ.
        assert_eq!(map_axis(8, -1), (63, 7), "8-pixel tiles: 64 * 8 = 512");
        assert_eq!(map_axis(8, 512), (0, 0));
        assert_eq!(map_axis(32, -1), (63, 31), "32-pixel: 64 * 32 = 2048");
        assert_eq!(map_axis(32, 2048), (0, 0));

        // And every layer's own edge, so the three the renderer uses are covered by
        // name rather than by their numbers happening to appear above.
        for layer in [Layer::Scroll1, Layer::Scroll2, Layer::Scroll3] {
            let e = layer.tile_edge();
            assert_eq!(map_axis(e, -1), (63, e - 1), "{layer:?} at raster −1");
            assert_eq!(
                map_axis(e, (MAP_TILES * e) as i32),
                (0, 0),
                "{layer:?} wraps after a whole map"
            );
        }
    }

    /// The map wraps at 64 tiles, so a scroll one map wide draws the same frame.
    ///
    /// Without this, the `rem_euclid(MAP_TILES)` could be dropped and every test
    /// above would still pass — none of them scrolls far enough to leave the
    /// grid. A scroll of `64 * edge` is a whole map and must equal a scroll of 0.
    #[test]
    fn the_map_wraps_at_sixty_four_tiles() {
        for layer in [Layer::Scroll1, Layer::Scroll2, Layer::Scroll3] {
            let f = Fixture::one_solid_tile(layer, 0x0A);
            let span = (MAP_TILES * layer.tile_edge()) as i32;
            let base = f.render(3, 5);
            assert!(
                base.pens.iter().any(|&p| p != UNTOUCHED),
                "{layer:?} draws something at (3,5), or this compares two blanks"
            );
            for (sx, sy) in [(3 + span, 5), (3, 5 + span), (3 - span, 5 - span)] {
                let r = f.render(sx, sy);
                assert_eq!(r.pens, base.pens, "{layer:?} at ({sx},{sy}) vs (3,5)");
            }
        }
    }

    /// A code the bank mapper rejects draws nothing, rather than tile 0.
    ///
    /// A mapper miss that fell back to 0 would paint the solid tile across the
    /// layer, which reads as a tilemap bug and sends the reader to the wrong
    /// file. `cps1_v.cpp:2474` substitutes the empty tile.
    #[test]
    fn a_tile_the_mapper_rejects_draws_nothing() {
        let mut f = Fixture::solid_everywhere(Layer::Scroll2, 0x0A);
        assert!(f.render(0, 0).px(0, 0).is_some(), "solid to begin with");

        // The fixture's mapper covers units 0..=0xFFFF at shift 1, so a code of
        // 0x8000 shifts to 0x10000 and no range matches.
        for e in f.gfxram.chunks_mut(2) {
            e[0] = 0x8000;
        }
        assert_eq!(
            f.mapper.map(GfxType::Scroll2, 0x8000),
            None,
            "the fixture's own premise"
        );
        let r = f.render(0, 0);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                assert_eq!(r.px(x, y), None, "({x},{y})");
            }
        }
    }

    /// Scroll 1's odd columns read the second half of their 64-byte frame.
    ///
    /// `BIT(tile_index, 5)` (`cps1_v.cpp:2461`) selects between the two 8×8
    /// layouts, and under scroll 1's scan mapper that bit is the column's low
    /// bit. So one 64-byte frame supplies two adjacent on-screen tiles, and a
    /// mutant reading a different bit of the index makes whole *rows* uniform
    /// instead of alternating columns.
    #[test]
    fn scroll_ones_odd_columns_read_the_frames_second_half() {
        let mut gfx = solid_tile(TileKind::Tile8x8, 1);
        for (b, o) in gfx.iter_mut().zip(solid_tile(TileKind::Tile8x8Odd, 2)) {
            *b |= o;
        }
        // Every gfxram entry is code 0, attribute 0, which a zeroed array says.
        let f = Fixture {
            gfxram: vec![0u16; GFXRAM_WORDS],
            gfx,
            mapper: fixture_mapper(),
            layer: Layer::Scroll1,
        };
        let even = 0x20 * PEN_GRANULARITY + 1;
        let odd = 0x20 * PEN_GRANULARITY + 2;
        let r = f.render(0, 0);
        // Four tile rows, so a mutant keyed to a row bit of the index — which
        // would make each row uniform — cannot pass by luck on one row.
        for y in [0usize, 7, 8, 23] {
            for x in 0..WIDTH {
                let want = if (x / 8) % 2 == 0 { even } else { odd };
                assert_eq!(r.px(x, y), Some(want), "({x},{y}) column {}", x / 8);
            }
        }
    }

    /// A disabled row-scroll table gives every row the same scroll.
    #[test]
    fn flat_scroll_rows_are_all_the_same() {
        let r = ScrollRows::flat(7, 3);
        assert_eq!(r.scroll_y, 3);
        assert!(r.x.iter().all(|&v| v == 7));
    }

    /// Row scroll adds a per-line offset from the row-scroll table, and the
    /// vertical scroll does **not** shift which entry a screen row reads.
    ///
    /// This one has to be derived rather than transcribed, because MAME's
    /// expression is written in tilemap coordinates and ours is in screen
    /// coordinates. `cps1_v.cpp:3027` is
    ///
    /// ```text
    /// for (i = 0; i < 256; i++)
    ///     tilemap[1]->set_scrollx((i - scrly) & 0x3ff,
    ///                             scrollx[1] + other[(i + otheroffs) & 0x3ff]);
    /// ```
    ///
    /// where `scrly = -scrolly[1]`, and `set_scrollx`'s row index is a **tilemap**
    /// row, not a raster row. A tilemap scrolled down by `scrolly` shows tilemap
    /// row `r + scrolly` at raster row `r`. So raster row `r` reads scroll row
    /// `t = r + scrolly`, which the loop set from entry `i` where
    /// `t = i - scrly = i + scrolly` — giving `i = r` and
    ///
    /// ```text
    /// x[r] = scrollx[1] + other[(r + otheroffs) & 0x3ff]
    /// ```
    ///
    /// The `- scrly` exists precisely to cancel the tilemap's own vertical
    /// scroll. Writing `(r + scroll_y + otheroffs)` here — the obvious reading of
    /// MAME's line — would make every row-scrolled layer shear as it scrolled
    /// vertically.
    ///
    /// `r` is a **raster** row, so this array's index `y` — a visible row —
    /// reads entry `y + VISIBLE_Y`. MAME's loop runs `i` over `0..256`, which
    /// covers the visible band 16..=239 exactly; indexing from `y` instead would
    /// read the 16 entries belonging to vertical blanking.
    #[test]
    fn row_scroll_reads_a_per_line_offset_independent_of_the_vertical_scroll() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        let mut cps_a = [0u16; 0x20];
        cps_a[OTHER_BASE] = 0; // table at word 0
        cps_a[ROWSCROLL_OFFS] = 0;

        // Visible row 0 is raster row VISIBLE_Y, so with a zero offset it reads
        // table entry VISIBLE_Y — not entry 0, which belongs to a blanking line.
        // The entries are written at their raster indices and the expected values
        // are literals, so an implementation indexing from the visible row reads
        // the zeroes below and fails.
        const FIRST: usize = VISIBLE_Y as usize;
        assert_eq!(FIRST, 16, "cps1.h:46");
        gfxram[FIRST] = 1;
        gfxram[FIRST + 1] = 2;
        let r = ScrollRows::row_scrolled(&gfxram, &cps_a, 100, 0);
        assert_eq!(r.x[0], 101, "visible row 0 reads entry 16");
        assert_eq!(r.x[1], 102);
        assert_eq!(r.x[2], 100, "the rest of the table is zero");

        // ROWSCROLL_OFFS shifts which table entry a row reads.
        cps_a[ROWSCROLL_OFFS] = 1;
        let r = ScrollRows::row_scrolled(&gfxram, &cps_a, 100, 0);
        assert_eq!(r.x[0], 102, "row 0 reads one entry further on");
        assert_eq!(r.x[1], 100, "the entry after that is zero");

        // The vertical scroll does not. This is the load-bearing assertion: a
        // `(y + scroll_y + offs)` implementation gives 102 for row 0 at
        // scroll_y 1, and shears every vertically-scrolling row-scrolled layer.
        cps_a[ROWSCROLL_OFFS] = 0;
        for sy in [-1i32, 1, 37, -200] {
            let r = ScrollRows::row_scrolled(&gfxram, &cps_a, 100, sy);
            assert_eq!(r.scroll_y, sy);
            assert_eq!(r.x[0], 101, "scroll_y {sy} must not move the table read");
            assert_eq!(r.x[1], 102, "scroll_y {sy}");
        }

        // The table index wraps at 0x400 words, not 0x200: the entry visible row
        // 0 reads is `VISIBLE_Y + offs`, so an offset of 0x3FF - VISIBLE_Y puts it
        // on the last entry and the row below wraps to entry 0.
        gfxram[0x3FF] = 9;
        gfxram[0] = 8;
        cps_a[ROWSCROLL_OFFS] = (0x3FF - FIRST) as u16;
        let r = ScrollRows::row_scrolled(&gfxram, &cps_a, 0, 0);
        assert_eq!(r.x[0], 9, "row 0 reads entry 0x3FF");
        assert_eq!(r.x[1], 8, "row 1 wraps to entry 0");
        cps_a[ROWSCROLL_OFFS] = (0x400 - FIRST) as u16;
        let r = ScrollRows::row_scrolled(&gfxram, &cps_a, 0, 0);
        assert_eq!(r.x[0], 8, "a whole table further on is entry 0 again");
        gfxram[0] = 0;

        // The table honours its base register, aligned to 0x800 bytes.
        cps_a[ROWSCROLL_OFFS] = 0;
        cps_a[OTHER_BASE] = 0x0040; // 0x40 * 256 = 0x4000 -> word 0x2000
        assert_eq!(cps_a_base(&cps_a, OTHER_BASE, OBJ_BOUNDARY), 0x2000);
        gfxram[0x2000 + FIRST] = 5;
        let r = ScrollRows::row_scrolled(&gfxram, &cps_a, 0, 0);
        assert_eq!(r.x[0], 5);

        // And every visible row is written — MAME's 256 iterations cover 224.
        cps_a[OTHER_BASE] = 0;
        for w in gfxram.iter_mut().take(ROWSCROLL_WORDS) {
            *w = 7;
        }
        let r = ScrollRows::row_scrolled(&gfxram, &cps_a, 100, -50);
        assert!(r.x.iter().all(|&v| v == 107), "no row is left unscrolled");
    }

    /// A per-row scroll actually shifts that row and no other.
    ///
    /// [`ScrollRows`] could compute the array correctly and [`draw_tilemap`]
    /// could still read `x[0]` for every line. This is the test that makes the
    /// array load-bearing in the draw path.
    #[test]
    fn draw_tilemap_uses_each_rows_own_scroll() {
        let f = Fixture::one_solid_tile(Layer::Scroll2, 0x0A);
        let want = 0x40 * PEN_GRANULARITY + 0x0A;
        let mut rows = Fixture::flat_rows(0, 0);
        // Row 3 alone scrolls left by 100 further, pushing the tile 100 pixels
        // right of where the other rows put it.
        rows.x[3] -= 100;
        let r = f.render_rows(&rows);
        assert_eq!(r.px(0, 2), Some(want), "row 2 is unscrolled");
        assert_eq!(r.px(0, 3), None, "row 3's tile moved off the left edge");
        assert_eq!(r.px(100, 3), Some(want), "and reappeared at x = 100");
        assert_eq!(r.px(0, 4), Some(want), "row 4 is unscrolled");
    }

    /// A priority-register bit marks that pen as occluding, and only that pen.
    ///
    /// `hi_pens` would otherwise be an unread parameter until Task 8, which is
    /// exactly the shape of a claim that cannot fail. The register semantics —
    /// MAME's double inversion through `set_transmask` — are Task 8's to pin;
    /// this only requires that the right bit reaches `prio`.
    #[test]
    fn a_high_pen_bit_marks_the_pixel_in_the_priority_buffer() {
        let mut f = Fixture::one_solid_tile(Layer::Scroll2, 0x0A);
        let rows = Fixture::flat_rows(0, 0);

        let r = f.render_with(&rows, &[0; 4]);
        assert_eq!(r.prio[0], 0, "no bits set, nothing occludes");

        // Bit 0x0A set for group 0, which is the fixture tile's group.
        let r = f.render_with(&rows, &[1 << 0x0A, 0, 0, 0]);
        assert_eq!(r.prio[0], 1, "pen 0x0A of group 0 occludes");
        assert_eq!(r.prio[16], 0, "the blank tile beside it is untouched");

        // A neighbouring bit does not: the shift is by the pen itself.
        let r = f.render_with(&rows, &[1 << 0x09, 0, 0, 0]);
        assert_eq!(r.prio[0], 0, "pen 9's bit does not mark pen 0x0A");

        // And another group's register does not answer for group 0.
        let r = f.render_with(&rows, &[0, 1 << 0x0A, 0, 0]);
        assert_eq!(r.prio[0], 0, "group 1's register is not group 0's");

        // A tile in group 2 reads register 2, and not register 0. Without this
        // the group index could be a hardcoded 0 — every test above uses the
        // fixture's group-0 tile, so nothing would notice.
        f.set_attr(0x0100); // group 2, no flip, colour 0
        let r = f.render_with(&rows, &[1 << 0x0A, 0, 0, 0]);
        assert_eq!(r.prio[0], 0, "group 0's register is not group 2's");
        let r = f.render_with(&rows, &[0, 0, 1 << 0x0A, 0]);
        assert_eq!(r.prio[0], 1, "group 2's tile reads register 2");
    }

    // ---------------------------------------------------------------- fixtures

    /// A tile every pixel of which is `pen`.
    ///
    /// # Why this helper is allowed
    ///
    /// The rule against encoder helpers bans a test that encodes with one
    /// function and decodes with its inverse. This helper is written from the
    /// plane *byte* structure and never restates the within-byte bit arithmetic
    /// that [`tile_pen`] implements: a solid tile's plane bytes are all 0x00 or
    /// all 0xFF, and the four bytes of a group are pen bits 0, 1, 2, 3 in memory
    /// order — `{24, 16, 8, 0}` at `cps1.cpp:3841`, read against `tiles.rs`'s
    /// `0x08 >> p`. So it cannot launder a bit-order bug.
    /// [`solid_bytes_are_solid`] pins the claim, and per-pixel positioning is
    /// `tiles.rs`'s business, tested there against literal pen grids.
    fn solid_tile(kind: TileKind, pen: u8) -> Vec<u8> {
        let byte_for = |bit: u8| {
            if pen & (1 << bit) != 0 {
                0xFFu8
            } else {
                0x00
            }
        };
        let group = [byte_for(0), byte_for(1), byte_for(2), byte_for(3)];
        // Bytes per frame row, and the byte offsets of the groups this kind owns.
        let (row_bytes, groups): (usize, &[usize]) = match kind {
            TileKind::Tile8x8 => (8, &[0]),
            TileKind::Tile8x8Odd => (8, &[4]),
            TileKind::Tile16x16 => (8, &[0, 4]),
            TileKind::Tile32x32 => (16, &[0, 4, 8, 12]),
        };
        let mut rom = vec![0u8; kind.bytes()];
        for r in 0..kind.size() as usize {
            for &g in groups {
                rom[r * row_bytes + g..][..4].copy_from_slice(&group);
            }
        }
        rom
    }

    /// The fixture's "solid" tiles really are solid, in every kind and every pen.
    ///
    /// Without this, `a_solid_layer_leaves_no_pixel_undrawn` could pass on a
    /// fixture that accidentally wrote pen 15 and a `draw_tilemap` that ignored
    /// transparency.
    #[test]
    fn solid_bytes_are_solid() {
        for kind in [
            TileKind::Tile8x8,
            TileKind::Tile8x8Odd,
            TileKind::Tile16x16,
            TileKind::Tile32x32,
        ] {
            // Every pen, so a helper right about only one plane is caught.
            for pen in 0..16u8 {
                let rom = solid_tile(kind, pen);
                assert_eq!(rom.len(), kind.bytes(), "{kind:?}");
                for y in 0..kind.size() {
                    for x in 0..kind.size() {
                        assert_eq!(tile_pen(&rom, kind, 0, x, y), pen, "{kind:?} {x},{y}");
                    }
                }
            }
        }
        // The two 8×8 kinds write disjoint bytes of the frame they share, which
        // is what lets `scroll_ones_odd_columns_read_the_frames_second_half` OR
        // them together.
        let even = solid_tile(TileKind::Tile8x8, 0x0F);
        let odd = solid_tile(TileKind::Tile8x8Odd, 0x0F);
        assert!(even.iter().zip(&odd).all(|(&a, &b)| a & b == 0));
    }

    /// A tile whose only non-transparent pixel is (0, 0), with pen 1.
    fn corner_tile(kind: TileKind) -> Vec<u8> {
        let mut rom = solid_tile(kind, TRANSPARENT_PEN);
        // Pixel (0, 0) is bit 7 — the MSB, x = 0 — of row 0's group, whose four
        // bytes are pen bits 0, 1, 2, 3 in order. Pen 1 keeps the first and
        // clears the rest. `corner_bytes_are_a_single_pixel` pins this.
        for b in rom[1..4].iter_mut() {
            *b &= 0x7F;
        }
        rom
    }

    /// The corner tile is one pixel of pen 1 in a field of transparency.
    #[test]
    fn corner_bytes_are_a_single_pixel() {
        for kind in [TileKind::Tile8x8, TileKind::Tile16x16, TileKind::Tile32x32] {
            let rom = corner_tile(kind);
            assert_eq!(tile_pen(&rom, kind, 0, 0, 0), 1, "{kind:?} corner");
            for y in 0..kind.size() {
                for x in 0..kind.size() {
                    if (x, y) == (0, 0) {
                        continue;
                    }
                    assert_eq!(
                        tile_pen(&rom, kind, 0, x, y),
                        TRANSPARENT_PEN,
                        "{kind:?} {x},{y}"
                    );
                }
            }
        }
    }

    /// The fixture's drawing tile.
    const SOLID_CODE: u16 = 0;
    /// The fixture's tile of nothing but the transparent pen.
    const BLANK_CODE: u16 = 1;

    static FIXTURE_RANGES: [BankRange; 3] = [
        BankRange {
            kind: GfxType::Scroll1,
            start: 0,
            end: 0xFFFF,
            bank: 0,
        },
        BankRange {
            kind: GfxType::Scroll2,
            start: 0,
            end: 0xFFFF,
            bank: 0,
        },
        BankRange {
            kind: GfxType::Scroll3,
            start: 0,
            end: 0xFFFF,
            bank: 0,
        },
    ];

    /// A mapper that is the identity on small codes.
    ///
    /// STF29's ranges put scroll 2 at ROM units 0x5000 upward, which would need a
    /// multi-megabyte graphics region to reach in a test. One 0x10000-unit bank
    /// covering units 0..=0xFFFF maps every small code to itself — the mask is
    /// `0xFFFF` and the base is 0 — so the draw tests exercise the draw path
    /// rather than the bank arithmetic, which `bank.rs` tests against STF29's own
    /// literals. `a_tile_the_mapper_rejects_draws_nothing` uses the top of the
    /// range to get a miss.
    fn fixture_mapper() -> BankMapper {
        BankMapper {
            bank_sizes: [0x1_0000, 0, 0, 0],
            ranges: &FIXTURE_RANGES,
        }
    }

    /// The fixture mapper really is the identity on the codes the fixtures use.
    #[test]
    fn the_fixture_mapper_is_the_identity_on_small_codes() {
        let m = fixture_mapper();
        for kind in [GfxType::Scroll1, GfxType::Scroll2, GfxType::Scroll3] {
            for code in [u32::from(SOLID_CODE), u32::from(BLANK_CODE), 2, 0x7F] {
                assert_eq!(m.map(kind, code), Some(code), "{kind:?} {code:#x}");
            }
        }
    }

    /// One code's worth of graphics bytes, solid in `pen`.
    ///
    /// Scroll 1's code indexes a 64-byte *frame* holding two 8-pixel tiles, so
    /// both halves are filled — otherwise odd columns would read the untouched
    /// second half and a coverage test would silently be checking pen 0 across
    /// half its columns.
    fn frame_bytes(layer: Layer, pen: u8) -> Vec<u8> {
        if layer == Layer::Scroll1 {
            let mut f = solid_tile(TileKind::Tile8x8, pen);
            for (b, o) in f.iter_mut().zip(solid_tile(TileKind::Tile8x8Odd, pen)) {
                *b |= o;
            }
            f
        } else {
            solid_tile(layer.gfx_type().tile_kind(), pen)
        }
    }

    /// A scratch board with one tilemap and a graphics ROM, for the draw tests.
    struct Fixture {
        gfxram: Vec<u16>,
        gfx: Vec<u8>,
        mapper: BankMapper,
        layer: Layer,
    }

    impl Fixture {
        /// A graphics ROM of `SOLID_CODE` followed by a blank `BLANK_CODE`.
        fn with_gfx(layer: Layer, solid: Vec<u8>) -> Self {
            let mut gfx = solid;
            gfx.extend(frame_bytes(layer, TRANSPARENT_PEN));
            Self {
                gfxram: vec![0u16; GFXRAM_WORDS],
                gfx,
                mapper: fixture_mapper(),
                layer,
            }
        }

        /// Points every tile of the map at `BLANK_CODE`, then map (0, 0) at
        /// `SOLID_CODE` with a zero attribute.
        fn one_tile(mut self) -> Self {
            for e in self.gfxram.chunks_mut(2) {
                e[0] = BLANK_CODE;
            }
            let i = 2 * self.layer.scan(0, 0);
            self.gfxram[i] = SOLID_CODE;
            self.gfxram[i + 1] = 0;
            self
        }

        /// One solid tile at map (0, 0); every other tile transparent.
        fn one_solid_tile(layer: Layer, pen: u8) -> Self {
            Self::with_gfx(layer, frame_bytes(layer, pen)).one_tile()
        }

        /// One tile at map (0, 0) whose only pixel is its top-left corner.
        fn one_corner_tile(layer: Layer) -> Self {
            let kind = layer.gfx_type().tile_kind();
            Self::with_gfx(layer, corner_tile(kind)).one_tile()
        }

        /// Every tile of the map solid: `SOLID_CODE` is 0 and the attribute 0,
        /// which a zeroed gfxram already says.
        fn solid_everywhere(layer: Layer, pen: u8) -> Self {
            Self::with_gfx(layer, frame_bytes(layer, pen))
        }

        /// Sets the attribute word of the tile at map (0, 0).
        fn set_attr(&mut self, attr: u16) {
            let i = 2 * self.layer.scan(0, 0) + 1;
            self.gfxram[i] = attr;
        }

        /// Renders with the scroll expressed so that map pixel (0, 0) lands at
        /// **visible** (`-scroll_x`, `-scroll_y`).
        ///
        /// The registers are raster-space, so this adds the window offset back in.
        /// Most tests here are about a tile's own pixels — flip, transparency,
        /// wrapping — and would only be obscured by carrying (64, 16) through
        /// every expected coordinate. The offset itself is pinned against literals
        /// by [`a_layers_origin_is_the_visible_window_not_the_raster_origin`], so
        /// routing these through here cannot hide it.
        fn render(&self, scroll_x: i32, scroll_y: i32) -> Rendered {
            self.render_raw(scroll_x - VISIBLE_X, scroll_y - VISIBLE_Y)
        }

        /// Renders with the scroll values the hardware registers hold.
        fn render_raw(&self, scroll_x: i32, scroll_y: i32) -> Rendered {
            self.render_rows(&ScrollRows::flat(scroll_x, scroll_y))
        }

        fn render_rows(&self, rows: &ScrollRows) -> Rendered {
            self.render_with(rows, &[0; 4])
        }

        /// [`ScrollRows::flat`] in the visible-frame convention [`Self::render`]
        /// uses, for the tests that then adjust individual rows.
        fn flat_rows(scroll_x: i32, scroll_y: i32) -> ScrollRows {
            ScrollRows::flat(scroll_x - VISIBLE_X, scroll_y - VISIBLE_Y)
        }

        fn render_with(&self, rows: &ScrollRows, hi_pens: &[u16; 4]) -> Rendered {
            let mut r = Rendered {
                pens: vec![UNTOUCHED; WIDTH * HEIGHT],
                prio: vec![0u8; WIDTH * HEIGHT],
            };
            draw_tilemap(
                &mut r.pens,
                &mut r.prio,
                &self.gfxram,
                &self.gfx,
                &self.mapper,
                0,
                self.layer,
                rows,
                hi_pens,
            );
            r
        }
    }

    /// The sentinel a fixture pre-fills `pens` with.
    ///
    /// No real pen can be this: the palette holds 0xC00 pens and a tilemap can
    /// reach only 0x7FF, so a `Some`/`None` from [`Rendered::px`] is unambiguous.
    const UNTOUCHED: u16 = 0xFFFF;

    struct Rendered {
        pens: Vec<u16>,
        prio: Vec<u8>,
    }

    impl Rendered {
        /// The pen drawn at `(x, y)`, or [`None`] where the layer wrote nothing.
        fn px(&self, x: usize, y: usize) -> Option<u16> {
            match self.pens[y * WIDTH + x] {
                UNTOUCHED => None,
                p => Some(p),
            }
        }

        /// How many pixels the layer wrote.
        fn opaque(&self) -> usize {
            self.pens.iter().filter(|&&p| p != UNTOUCHED).count()
        }

        /// Nothing was drawn anywhere.
        fn is_blank(&self) -> bool {
            self.opaque() == 0
        }
    }
}
