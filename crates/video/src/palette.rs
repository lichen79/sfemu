//! The palette: six pages of 512 entries, and the brightness maths.
//!
//! `cps1_build_palette` (`cps1_v.cpp:2611-2645`) copies pages out of gfxram at
//! the CPS-A palette base, gated by a CPS-B register, and converts each 16-bit
//! entry to RGB.
//!
//! # Raw entries, then RGB
//!
//! [`build_palette`] writes the raw 16-bit entries and [`entry_to_rgb`]
//! converts. Keeping them apart is a testing decision: a page-placement bug and
//! a brightness-arithmetic bug then fail different tests instead of being
//! indistinguishable in one RGB buffer.

use crate::regs::{cps_a_base, PALETTE_BASE, PALETTE_BOUNDARY};

/// Palette pages (`cps1_v.cpp:2619`: `page < 6`).
pub const PAGES: usize = 6;
/// Entries in one page (`cps1_v.cpp:2623`: `offset < 0x200`).
pub const PAGE_ENTRIES: usize = 0x200;
/// Total pens the build loop fills.
///
/// MAME allocates more than this — `m_palette_size = CPS1_PALETTE_ENTRIES * 32`
/// is 192 × 32 = 6144 (`cps1_v.cpp:2542`, `cps1.h:173`) — but
/// `cps1_build_palette` writes only `0x200 * page + offset` for six pages, which
/// is 3072. [`BACKGROUND_PEN`] being the last of these 3072 is the corroboration.
pub const PENS: usize = PAGES * PAGE_ENTRIES;

/// The pen the screen is filled with before any layer draws.
///
/// `cps1_v.cpp:3042`: "Games use pen 0xbff as background color" — the last pen
/// the build loop writes.
pub const BACKGROUND_PEN: u16 = 0xBFF;

/// Maximum value of the brightness multiplier: `0x0f + (0x0f << 1)`.
const BRIGHT_MAX: u32 = 0x2D;

/// A palette entry's colour.
///
/// `cps1_v.cpp:2628-2634`. Blue is bits 0-3, green 4-7, red 8-11, brightness
/// 12-15. `0x11` scales a nibble to a byte; `bright / 0x2d` scales by
/// brightness, so brightness 15 is unity and brightness 0 is about a third —
/// MAME reads that off the schematics.
///
/// The division truncates, and that is the hardware's arithmetic as MAME models
/// it: entry 0x8777 gives 81, not 82.
pub fn entry_to_rgb(entry: u16) -> [u8; 3] {
    let e = u32::from(entry);
    let bright = 0x0F + ((e >> 12) << 1);
    let ch = |shift: u32| (((e >> shift) & 0x0F) * 0x11 * bright / BRIGHT_MAX) as u8;
    [ch(8), ch(4), ch(0)]
}

/// Copies the enabled palette pages out of gfxram into `out`.
///
/// Bit `n` of `page_enable` — the board's palette-control CPS-B register —
/// enables page `n`. A disabled page is left untouched in `out`.
///
/// # The compaction asymmetry
///
/// The source pointer advances past a *skipped* page only once some page has
/// already been copied (`cps1_v.cpp:2638-2643`). So a disabled page 2 leaves
/// pages 3-5 reading their own source words, while a disabled page 0 shifts
/// every later page's source down by one. MAME's comment: "if the first palette
/// pages are skipped, all the following pages are scaled down". It is an odd rule
/// and it is the hardware's.
pub fn build_palette(gfxram: &[u16], cps_a: &[u16], page_enable: u16, out: &mut [u16; PENS]) {
    let base = cps_a_base(cps_a, PALETTE_BASE, PALETTE_BOUNDARY);
    let mut src = base;
    let mut copied = false;
    for page in 0..PAGES {
        if page_enable & (1 << page) != 0 {
            for i in 0..PAGE_ENTRIES {
                // The modulo is what keeps a base near the top of gfxram from
                // reading past it. `cps_a_base` does not guarantee even the start
                // is inside — see
                // `regs::tests::cps_a_base_can_point_past_gfxram_so_callers_must_wrap`.
                out[page * PAGE_ENTRIES + i] = gfxram[(src + i) % gfxram.len()];
            }
            src += PAGE_ENTRIES;
            copied = true;
        } else if copied {
            src += PAGE_ENTRIES;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regs::{PALETTE_BASE, PALETTE_BOUNDARY};

    /// gfxram's size in words, 192 KB (`cps1.cpp:592`).
    const GFXRAM_WORDS: usize = 0x1_8000;

    /// The palette is six pages of 512, and the background is the last pen.
    #[test]
    fn the_palette_is_six_pages_of_five_hundred_and_twelve() {
        assert_eq!((PAGES, PAGE_ENTRIES, PENS), (6, 0x200, 0xC00));
        assert_eq!(BACKGROUND_PEN, 0xBFF);
        assert_eq!(BACKGROUND_PEN as usize, PENS - 1, "the last pen of page 5");
    }

    /// The brightness formula, with every expectation computed by hand from
    /// `cps1_v.cpp:2628-2634`.
    ///
    /// ```text
    /// bright = 0x0f + ((entry >> 12) << 1)
    /// c      = ((entry >> shift) & 0x0f) * 0x11 * bright / 0x2d
    /// ```
    ///
    /// `0x11` scales a nibble to a byte (0x0F -> 0xFF) and `bright / 0x2d` scales
    /// by brightness, where 0x2d = 45 is the maximum `bright` (0x0f + 15*2). So
    /// brightness 15 is unity and brightness 0 is roughly a third — MAME's note:
    /// "when the 'brightness' component is set to 0 it should reduce brightness
    /// to 1/3".
    #[test]
    fn the_brightness_formula_scales_each_nibble() {
        // Full brightness, full white: bright = 0x0f + 30 = 45 = 0x2d, so each
        // channel is 0x0F * 0x11 * 45 / 45 = 0xFF.
        assert_eq!(entry_to_rgb(0xFFFF), [0xFF, 0xFF, 0xFF]);
        // Full brightness, black.
        assert_eq!(entry_to_rgb(0xF000), [0x00, 0x00, 0x00]);
        // Zero brightness, full white: bright = 0x0f = 15, so
        // 0x0F * 0x11 * 15 / 45 = 255 * 15 / 45 = 85 = 0x55.
        assert_eq!(entry_to_rgb(0x0FFF), [0x55, 0x55, 0x55]);
        // The channels are r, g, b from bits 8-11, 4-7, 0-3. Full brightness
        // pure red: 0x0F * 0x11 = 255, green and blue 0.
        assert_eq!(entry_to_rgb(0xFF00), [0xFF, 0x00, 0x00]);
        assert_eq!(entry_to_rgb(0xF0F0), [0x00, 0xFF, 0x00]);
        assert_eq!(entry_to_rgb(0xF00F), [0x00, 0x00, 0xFF]);
        // A mid case where the truncation is visible: brightness 8 gives
        // bright = 0x0f + 16 = 31, and nibble 7 gives 7 * 17 * 31 / 45 =
        // 3689 / 45 = 81 (81.97 truncated), not 82.
        assert_eq!(entry_to_rgb(0x8777), [81, 81, 81]);
    }

    /// Every enabled page is copied from gfxram at the palette base.
    #[test]
    fn all_six_pages_are_copied_when_all_six_are_enabled() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        // Palette at gfxram word 0: register 0 * 256 = 0.
        let mut cps_a = [0u16; 0x20];
        cps_a[PALETTE_BASE] = 0;
        // Mark each page's first and last word with the page number.
        for p in 0..PAGES {
            gfxram[p * PAGE_ENTRIES] = 0x1000 + p as u16;
            gfxram[p * PAGE_ENTRIES + PAGE_ENTRIES - 1] = 0x2000 + p as u16;
        }
        let mut out = [0u16; PENS];
        build_palette(&gfxram, &cps_a, 0x3F, &mut out);
        for p in 0..PAGES {
            assert_eq!(out[p * PAGE_ENTRIES], 0x1000 + p as u16, "page {p} head");
            assert_eq!(
                out[p * PAGE_ENTRIES + PAGE_ENTRIES - 1],
                0x2000 + p as u16,
                "page {p} tail"
            );
        }
    }

    /// A disabled page is left alone, and the pages after it do not shift.
    ///
    /// Page 2 off with pages 0-1 already copied: `cps1_v.cpp:2638-2643` advances
    /// the source pointer past a skipped page **because at least one page has
    /// already been copied**, so pages 3-5 still come from their own source
    /// words.
    #[test]
    fn a_disabled_middle_page_leaves_later_pages_in_place() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        let cps_a = [0u16; 0x20];
        for p in 0..PAGES {
            gfxram[p * PAGE_ENTRIES] = 0x1000 + p as u16;
        }
        let mut out = [0xAAAAu16; PENS];
        // 0b111011: pages 0, 1, 3, 4, 5 on; page 2 off.
        build_palette(&gfxram, &cps_a, 0x3B, &mut out);
        assert_eq!(out[0], 0x1000);
        assert_eq!(out[PAGE_ENTRIES], 0x1001);
        assert_eq!(
            out[2 * PAGE_ENTRIES],
            0xAAAA,
            "a disabled page is not written at all"
        );
        assert_eq!(out[3 * PAGE_ENTRIES], 0x1003, "page 3 is not shifted down");
        assert_eq!(out[4 * PAGE_ENTRIES], 0x1004);
        assert_eq!(out[5 * PAGE_ENTRIES], 0x1005);
    }

    /// Leading disabled pages compact: the first *enabled* page reads the first
    /// source page.
    ///
    /// This asymmetry is the clause a reimplementation drops silently.
    /// `cps1_v.cpp:2642` advances the source pointer only `if (palette_ram !=
    /// palette_base)` — MAME's comment: "if the first palette pages are skipped,
    /// all the following pages are scaled down".
    #[test]
    fn leading_disabled_pages_compact_the_source() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        let cps_a = [0u16; 0x20];
        for p in 0..PAGES {
            gfxram[p * PAGE_ENTRIES] = 0x1000 + p as u16;
        }
        let mut out = [0xAAAAu16; PENS];
        // 0b111100: pages 0 and 1 off, 2-5 on.
        build_palette(&gfxram, &cps_a, 0x3C, &mut out);
        assert_eq!(out[0], 0xAAAA);
        assert_eq!(out[PAGE_ENTRIES], 0xAAAA);
        assert_eq!(
            out[2 * PAGE_ENTRIES],
            0x1000,
            "page 2 reads source page 0, because nothing was copied before it"
        );
        assert_eq!(out[3 * PAGE_ENTRIES], 0x1001);
        assert_eq!(out[4 * PAGE_ENTRIES], 0x1002);
        assert_eq!(out[5 * PAGE_ENTRIES], 0x1003);
    }

    /// The palette honours its base register and its 0x400-byte alignment.
    #[test]
    fn the_palette_reads_from_the_base_register() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        let mut cps_a = [0u16; 0x20];
        // 0x0100 * 256 = 0x10000, aligned already, & 0x3FFFF, / 2 = 0x8000.
        cps_a[PALETTE_BASE] = 0x0100;
        gfxram[0x8000] = 0x1234;
        let mut out = [0u16; PENS];
        build_palette(&gfxram, &cps_a, 0x01, &mut out);
        assert_eq!(out[0], 0x1234);
        assert_eq!(PALETTE_BOUNDARY, 0x400, "cps1_v.cpp:2541, verified on pcb");
    }

    /// A base register near the top of gfxram does not panic: the read wraps.
    ///
    /// `cps_a_base` does not guarantee its index is inside gfxram at all — see
    /// `regs::tests::cps_a_base_can_point_past_gfxram_so_callers_must_wrap`.
    /// Register 0xFFFF at a 0x400 boundary gives word 0x1FE00, which is 0x7E00
    /// past the end of a 0x18000-word array. Without the wrap this panics on a
    /// value a guest can write, and a guest that writes it is a guest bug, not
    /// ours.
    #[test]
    fn a_base_past_the_end_of_gfxram_wraps_rather_than_panicking() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        let mut cps_a = [0u16; 0x20];
        cps_a[PALETTE_BASE] = 0xFFFF;
        // 0xFFFF * 256 = 0xFFFF00, & !0x3FF = 0xFFFC00, & 0x3FFFF = 0x3FC00,
        // / 2 = 0x1FE00. Wrapped: 0x1FE00 - 0x18000 = 0x7E00.
        gfxram[0x7E00] = 0x4321;
        let mut out = [0u16; PENS];
        build_palette(&gfxram, &cps_a, 0x01, &mut out);
        assert_eq!(out[0], 0x4321, "page 0 read the wrapped word");
    }

    /// A page whose *tail* runs off the end of gfxram wraps too, not just its
    /// head.
    ///
    /// The wrap is inside the inner loop for this reason: a base 8 words short of
    /// the end is inside the array, so a check on the base alone would let the
    /// remaining 504 words of the page index past it.
    #[test]
    fn a_page_straddling_the_end_of_gfxram_wraps_mid_page() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        let mut cps_a = [0u16; 0x20];
        // The largest aligned base inside gfxram: 0x17C00 words. Register r
        // satisfies (r * 256 & 0x3FFFF) / 2 = 0x17C00, i.e. r * 256 = 0x2F800,
        // r = 0x2F8.
        cps_a[PALETTE_BASE] = 0x2F8;
        assert_eq!(
            cps_a_base(&cps_a, PALETTE_BASE, PALETTE_BOUNDARY),
            0x1_7C00,
            "inside gfxram, but only 0x400 words from the end"
        );
        // Page 0 spans words 0x17C00..0x17E00, which is still inside. Enable
        // pages 0 and 1 so page 1 spans 0x17E00..0x18000 — ending exactly at the
        // boundary — and page 2 would be the first to wrap. Use all six.
        gfxram[0x1_7FFF] = 0x1111; // last word of gfxram, page 1's tail
        gfxram[0] = 0x2222; // page 2's head after wrapping
        let mut out = [0u16; PENS];
        build_palette(&gfxram, &cps_a, 0x3F, &mut out);
        assert_eq!(out[2 * PAGE_ENTRIES - 1], 0x1111, "page 1's last entry");
        assert_eq!(out[2 * PAGE_ENTRIES], 0x2222, "page 2 wrapped to word 0");
    }
}
