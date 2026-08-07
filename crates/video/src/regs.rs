//! The CPS-A register file, and the per-board CPS-B register layout.
//!
//! # Word indices, not byte offsets
//!
//! A 68000 program writes CPS-A at byte offsets from 0x800100, and `machine`
//! stores the result in a `[u16]`. Every constant here is the **word** index —
//! MAME's `cps1.h:176-193` values, which are already written divided by two.
//! Mixing the two shifts the whole register file by one slot, and every value
//! then reads as plausible in the wrong place.
//!
//! Each index is written as the plain number rather than as `0x0A / 2`, because
//! clippy rejects `0x00 / 2` (`erasing_op`) and `0x02 / 2` (`eq_op`). The byte
//! offset lives in the doc comment, and
//! `tests::the_cps_a_indices_are_byte_offsets_divided_by_two` pairs the two
//! sets of literals — which is the stronger arrangement anyway, since the test
//! now compares two independently written tables rather than restating one
//! expression.

/// Object (sprite) table base — byte offset 0x00.
pub const OBJ_BASE: usize = 0;
/// Scroll-1 (8×8) tilemap base — byte offset 0x02.
pub const SCROLL1_BASE: usize = 1;
/// Scroll-2 (16×16) tilemap base — byte offset 0x04.
pub const SCROLL2_BASE: usize = 2;
/// Scroll-3 (32×32) tilemap base — byte offset 0x06.
pub const SCROLL3_BASE: usize = 3;
/// Row-scroll table base — byte offset 0x08.
pub const OTHER_BASE: usize = 4;
/// Palette base — byte offset 0x0A.
pub const PALETTE_BASE: usize = 5;
/// Scroll-1 horizontal scroll — byte offset 0x0C.
pub const SCROLL1_X: usize = 6;
/// Scroll-1 vertical scroll — byte offset 0x0E.
pub const SCROLL1_Y: usize = 7;
/// Scroll-2 horizontal scroll — byte offset 0x10.
pub const SCROLL2_X: usize = 8;
/// Scroll-2 vertical scroll — byte offset 0x12.
pub const SCROLL2_Y: usize = 9;
/// Scroll-3 horizontal scroll — byte offset 0x14.
pub const SCROLL3_X: usize = 10;
/// Scroll-3 vertical scroll — byte offset 0x16.
pub const SCROLL3_Y: usize = 11;
/// Row-scroll index offset, in words, into the row-scroll table — byte offset
/// 0x20.
pub const ROWSCROLL_OFFS: usize = 16;
/// Video control: bit 0 row scroll, bits 2-3 layer enables, bit 15 screen flip —
/// byte offset 0x22.
pub const VIDEOCONTROL: usize = 17;

/// Alignment of a scroll tilemap table, in bytes (`cps1_v.cpp:2101-2107`).
pub const SCROLL_BOUNDARY: u32 = 0x4000;
/// Alignment of the object table and the row-scroll table, in bytes.
pub const OBJ_BOUNDARY: u32 = 0x800;
/// Alignment of the palette, in bytes.
///
/// "minimum alignment is a single palette page (512 colors). Verified on pcb"
/// (`cps1_v.cpp:2541`).
pub const PALETTE_BOUNDARY: u32 = 0x400;

/// gfxram in words — 192 KB, `0x900000-0x92ffff` (`cps1.cpp:592`).
///
/// Duplicated here rather than imported, because this crate has no dependency on
/// `machine` and must not gain one. Only [`cps_a_base`]'s documentation and its
/// tests use it; the render path wraps against the slice it was handed, so a
/// caller with a differently sized gfxram is still safe. Hence `cfg(test)`: a
/// non-test use of this constant would be a render path that had assumed a size
/// instead of wrapping against the slice it was given.
#[cfg(test)]
const GFXRAM_WORDS: usize = 0x1_8000;

/// Where a CPS-A base register points, as a **word** index into gfxram.
///
/// `cps1_v.cpp:2099-2110`: the register is scaled by 256, truncated to the
/// table's alignment, and wrapped into 256 KB. MAME's comment records why the
/// truncation is hardware and not tidiness — games that fail to align their
/// tables exist, and it names Captain Commando's continue screen.
///
/// # The result can point past gfxram
///
/// `& 0x3FFFF` bounds the index to a 256 KB window and gfxram is 192 KB
/// (`cps1.cpp:592`), so some registers resolve to word indices from 0x18000 up to
/// 0x1FE00 — outside the array.
///
/// Which ones is less obvious than it looks: `* 256` then `& 0x3FFFF` keeps only
/// bits 8-17 of the product, so **the index depends on nothing but `reg & 0x3FF`**,
/// and it lands outside gfxram exactly when `(reg & 0x3FF) >= 0x300`. That is a
/// quarter of all register values, and it includes small ones — 0x0300 resolves to
/// 0x18000, the first word past the end, while 0xE000 resolves to 0. A reader who
/// expects "only large registers overflow" has it backwards.
/// `tests::cps_a_base_can_point_past_gfxram_so_callers_must_wrap` pins the
/// predicate, not just the maximum. That is the hardware's arithmetic
/// and MAME's too: `cps1_base` returns a pointer into a `required_shared_ptr`
/// with no bounds check. Callers wrap with `% gfxram.len()`; clamping here would
/// silently relocate a table the guest asked for, and
/// `tests::cps_a_base_can_point_past_gfxram_so_callers_must_wrap` exists so a
/// later reader does not remove a wrap believing it redundant.
pub fn cps_a_base(cps_a: &[u16], reg: usize, boundary: u32) -> usize {
    let base = u32::from(cps_a[reg]) * 256;
    let base = base & !(boundary - 1);
    ((base & 0x3_FFFF) / 2) as usize
}

/// Which CPS-B registers a board uses for layers, priority, and the palette.
///
/// Separate from `machine`'s `BoardConfig` because the two have no fields in
/// common: `machine` needs the ID address and the input latches, and has no use
/// for a layer-enable mask; this crate is the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoConfig {
    /// Word index of the layer-control register.
    pub layer_control: usize,
    /// Word indices of the four priority-mask registers. `None` where the board
    /// has none, in which case that group's pens never occlude sprites.
    pub priority: [Option<usize>; 4],
    /// Word index of the palette page-enable register.
    pub palette_control: usize,
    /// Per-layer bit in the layer-control register that enables scroll 1, 2, 3.
    pub layer_enable_mask: [u16; 3],
}

impl VideoConfig {
    /// SF2's `CPS_B_11` (`cps1_v.cpp:491`, selected by the table row at
    /// `cps1_v.cpp:1838`).
    ///
    /// The header comment at `cps1_v.cpp:487` gives the field order. The two
    /// trailing entries of MAME's five-element layer-enable mask are the star
    /// layers, both 0 on this board — consistent with SF2's `ROM_START` having no
    /// `stars` region, and the reason this struct carries three.
    /// `const` so a caller can embed this in its own `const fn` board table —
    /// `machine`'s `BoardConfig::sf2` does — rather than having to give up
    /// constness for a struct that is nothing but literals.
    pub const fn sf2() -> Self {
        Self {
            layer_control: 0x26 / 2,
            priority: [
                Some(0x28 / 2),
                Some(0x2A / 2),
                Some(0x2C / 2),
                Some(0x2E / 2),
            ],
            palette_control: 0x30 / 2,
            layer_enable_mask: [0x08, 0x10, 0x20],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The register indices are word indices, and they are the ones a 68000
    /// program's byte offsets divide down to.
    ///
    /// Pinned as literals against `cps1.h:176-193`, where MAME writes them
    /// already divided by two. The pairing with the byte offset is the
    /// load-bearing part: `machine`'s bus hands us a word array, and a program
    /// writes bytes.
    #[test]
    fn the_cps_a_indices_are_byte_offsets_divided_by_two() {
        for (byte, word) in [
            (0x00, OBJ_BASE),
            (0x02, SCROLL1_BASE),
            (0x04, SCROLL2_BASE),
            (0x06, SCROLL3_BASE),
            (0x08, OTHER_BASE),
            (0x0A, PALETTE_BASE),
            (0x0C, SCROLL1_X),
            (0x0E, SCROLL1_Y),
            (0x10, SCROLL2_X),
            (0x12, SCROLL2_Y),
            (0x14, SCROLL3_X),
            (0x16, SCROLL3_Y),
            (0x20, ROWSCROLL_OFFS),
            (0x22, VIDEOCONTROL),
        ] {
            assert_eq!(word, byte / 2, "byte offset {byte:#04x}");
        }
        // And the values themselves, so a uniform doubling of every constant
        // cannot pass the check above.
        assert_eq!(OBJ_BASE, 0);
        assert_eq!(PALETTE_BASE, 5);
        assert_eq!(VIDEOCONTROL, 17);
    }

    /// `cps1_base` scales by 256, truncates to the boundary, and wraps into a
    /// 256 KB window.
    ///
    /// Every expectation is hand-computed from `cps1_v.cpp:2099-2110`:
    /// `base = reg * 256; base &= ~(boundary-1); return (base & 0x3ffff) / 2`.
    #[test]
    fn cps_a_base_scales_truncates_and_wraps() {
        let mut a = [0u16; 0x20];

        // 0x9000 * 256 = 0x900000, masked to a 0x4000 boundary is unchanged,
        // & 0x3FFFF = 0x00000, / 2 = 0.
        a[SCROLL1_BASE] = 0x9000;
        assert_eq!(cps_a_base(&a, SCROLL1_BASE, SCROLL_BOUNDARY), 0);

        // 0x9040 * 256 = 0x904000 -> & 0x3FFFF = 0x04000 -> / 2 = 0x2000.
        a[SCROLL2_BASE] = 0x9040;
        assert_eq!(cps_a_base(&a, SCROLL2_BASE, SCROLL_BOUNDARY), 0x2000);

        // 0x9080 * 256 = 0x908000 -> 0x08000 -> 0x4000.
        a[SCROLL3_BASE] = 0x9080;
        assert_eq!(cps_a_base(&a, SCROLL3_BASE, SCROLL_BOUNDARY), 0x4000);

        // 0x9200 * 256 = 0x920000 -> 0x20000 -> 0x10000. The obj boundary is
        // 0x800 and 0x920000 is already aligned, so truncation is invisible here.
        a[OBJ_BASE] = 0x9200;
        assert_eq!(cps_a_base(&a, OBJ_BASE, OBJ_BOUNDARY), 0x1_0000);

        // Truncation is visible when the register is not aligned: 0x9042 * 256
        // = 0x904200, and & !0x3FFF drops the 0x200 -> 0x904000 -> 0x2000. A
        // missing truncation would give 0x2100.
        a[SCROLL2_BASE] = 0x9042;
        assert_eq!(
            cps_a_base(&a, SCROLL2_BASE, SCROLL_BOUNDARY),
            0x2000,
            "the boundary mask drops the low bits"
        );

        // 0xFFFF * 256 = 0xFFFF00 -> & 0x3FFFF = 0x3FF00, masked to a 0x800
        // boundary -> 0x3F800 -> 0x1FC00. That is **past** the end of gfxram,
        // which is the point of the next test.
        a[OTHER_BASE] = 0xFFFF;
        assert_eq!(cps_a_base(&a, OTHER_BASE, OBJ_BOUNDARY), 0x1_FC00);
    }

    /// `cps_a_base` can point past gfxram, so every caller must wrap.
    ///
    /// This is the opposite of what it is tempting to assume. `& 0x3FFFF` bounds
    /// the result to a **256 KB** window, but gfxram is 192 KB (`cps1.cpp:592`),
    /// so registers in the top eighth of the range resolve to word indices from
    /// 0x18000 to 0x1FE00 — outside the array. MAME has the same gap:
    /// `cps1_base` returns `&m_gfxram[(base & 0x3ffff)/2]` into a
    /// `required_shared_ptr` that does not bounds-check.
    ///
    /// Rather than clamp — which would silently move a table a guest asked for —
    /// this function returns the hardware's index and every read wraps with
    /// `% gfxram.len()`. The test exists so that a later reader does not
    /// "simplify" a wrap away on the false belief that the index is already in
    /// range.
    #[test]
    fn cps_a_base_can_point_past_gfxram_so_callers_must_wrap() {
        let mut worst = 0;
        for boundary in [PALETTE_BOUNDARY, OBJ_BOUNDARY, SCROLL_BOUNDARY] {
            for r in 0..=0xFFFFu16 {
                let mut a = [0u16; 0x20];
                a[SCROLL1_BASE] = r;
                let i = cps_a_base(&a, SCROLL1_BASE, boundary);
                assert!(i < 0x2_0000, "reg {r:#06x} gave {i:#x}, past 256 KB");
                worst = worst.max(i);
            }
        }
        assert_eq!(
            worst, 0x1_FE00,
            "the largest index any register can produce"
        );
        assert!(
            worst >= GFXRAM_WORDS,
            "and it is outside gfxram, so wrapping is mandatory, not defensive"
        );

        // *Which* registers overflow, not just that the maximum does. `* 256` then
        // `& 0x3FFFF` keeps only bits 8-17 of the product, so the index depends on
        // nothing but `reg & 0x3FF` — and it leaves gfxram exactly when that is
        // 0x300 or more. A quarter of all values, small ones included: the smallest
        // overflowing register is 0x0300, and 0xE000 resolves to 0.
        //
        // Stated as a predicate over every register rather than as a maximum,
        // because the maximum alone is satisfied by an implementation that
        // overflows for the wrong inputs — and the intuition it invites ("only
        // large registers overflow") is the false one.
        for boundary in [PALETTE_BOUNDARY, OBJ_BOUNDARY, SCROLL_BOUNDARY] {
            for r in 0..=0xFFFFu16 {
                let mut a = [0u16; 0x20];
                a[SCROLL1_BASE] = r;
                let outside = cps_a_base(&a, SCROLL1_BASE, boundary) >= GFXRAM_WORDS;
                assert_eq!(
                    outside,
                    r & 0x3FF >= 0x300,
                    "reg {r:#06x} at boundary {boundary:#x}"
                );
            }
        }
        let mut a = [0u16; 0x20];
        a[SCROLL1_BASE] = 0x0300;
        assert_eq!(cps_a_base(&a, SCROLL1_BASE, PALETTE_BOUNDARY), GFXRAM_WORDS);
        a[SCROLL1_BASE] = 0xE000;
        assert_eq!(cps_a_base(&a, SCROLL1_BASE, PALETTE_BOUNDARY), 0);
    }

    /// SF2's CPS-B layout, from `cps1_v.cpp:491`, as word indices.
    #[test]
    fn sf2s_video_config_is_cps_b_11() {
        let c = VideoConfig::sf2();
        assert_eq!(c.layer_control, 0x26 / 2);
        assert_eq!(
            c.priority,
            [
                Some(0x28 / 2),
                Some(0x2A / 2),
                Some(0x2C / 2),
                Some(0x2E / 2)
            ]
        );
        assert_eq!(c.palette_control, 0x30 / 2);
        assert_eq!(c.layer_enable_mask, [0x08, 0x10, 0x20]);
    }
}
