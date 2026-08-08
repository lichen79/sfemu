//! Sprites: the one-frame object latch, the end marker, and blocked codes.
//!
//! A sprite is four words of the object table — `x`, `y`, `code`, `attr`
//! (`cps1_v.cpp:2652-2680`) — drawn as one or more 16×16 tiles from the sprite
//! graphics ROM. The attribute word carries the colour scheme, the two flips, and
//! the block size:
//!
//! ```text
//! attr & 0x001F   colour scheme
//! attr & 0x0020   X flip
//! attr & 0x0040   Y flip
//! attr & 0x0F00   X block size - 1, in 16-pixel tiles
//! attr & 0xF000   Y block size - 1
//! ```
//!
//! Sprites take palette schemes 0x00-0x1F, with no base added — the three
//! tilemaps take 0x20 upward (see [`crate::layers::Layer::colour_base`]).

use crate::bank::{BankMapper, GfxType};
use crate::layers::PEN_GRANULARITY;
use crate::regs::{cps_a_base, OBJ_BASE, OBJ_BOUNDARY};
use crate::tiles::{tile_pen, TileKind, TRANSPARENT_PEN};
use crate::{HEIGHT, VISIBLE_X, VISIBLE_Y, WIDTH};

/// Words in the object table — `m_obj_size` is 0x800 bytes (`cps1_v.cpp:2537`).
pub const OBJ_WORDS: usize = 0x400;

/// Words per sprite record: `x`, `y`, `code`, `attr`.
const RECORD_WORDS: usize = 4;

/// A sprite tile's edge in pixels. Sprites use `gfx(2)`, the 16×16 layout
/// (`cps1_v.cpp:2732`).
const SPRITE_EDGE: u32 = 16;

/// The mask a sprite's screen position takes (`cps1_v.cpp:2777`: `& 0x1ff`).
const POS_MASK: i32 = 0x1FF;

/// The object table as it looked at the *previous* vblank.
///
/// # Why a copy and not a borrow
///
/// `cps1_objram_latch` memcpys 0x800 bytes out of gfxram at vblank
/// (`cps1_v.cpp:3068`), under the comment "CPS1 sprites have to be delayed one
/// frame". A renderer reading live objram puts every sprite one frame ahead of
/// its layers, which on screen reads as jitter rather than as a bug — so the copy
/// is the behaviour, not an optimisation.
#[derive(Debug, Clone)]
pub struct ObjLatch {
    words: Box<[u16; OBJ_WORDS]>,
}

impl Default for ObjLatch {
    fn default() -> Self {
        Self::new()
    }
}

impl ObjLatch {
    /// An empty latch — every word zero, which is not an end marker.
    pub fn new() -> Self {
        Self {
            words: Box::new([0u16; OBJ_WORDS]),
        }
    }

    /// Copies the object table out of gfxram. Call once per vblank.
    ///
    /// The reads wrap for the reason [`cps_a_base`] documents: the register can
    /// resolve past the end of gfxram, and that is the hardware's arithmetic
    /// rather than something to clamp.
    pub fn latch(&mut self, gfxram: &[u16], cps_a: &[u16]) {
        let base = cps_a_base(cps_a, OBJ_BASE, OBJ_BOUNDARY);
        let n = gfxram.len();
        for (i, w) in self.words.iter_mut().enumerate() {
            *w = gfxram[(base + i) % n];
        }
    }

    /// The latched words.
    pub fn words(&self) -> &[u16; OBJ_WORDS] {
        &self.words
    }

    /// The latched words, for a save-state decoder to fill.
    ///
    /// ⚠️ **Not for the renderer or the beam.** [`ObjLatch::latch`] is what a vblank
    /// calls, and it reads the table the guest wrote. This exists because a decoder
    /// has bytes from a file and no gfxram to latch from — and because a decoder
    /// that had to build a `Box<[u16; OBJ_WORDS]>` to hand over would allocate a
    /// second one for no reason.
    pub fn words_mut(&mut self) -> &mut [u16; OBJ_WORDS] {
        &mut self.words
    }

    /// Word offset of the last drawable sprite record, or [`None`] for none.
    ///
    /// `find_last_sprite` (`cps1_v.cpp:2684`) walks forward in steps of four for
    /// an attribute word with `(attr & 0xFF00) == 0xFF00` and answers
    /// `offset - 4`, so the record immediately *before* the marker is skipped
    /// too: a marker in record 2 leaves records 0 and 1 drawable. With no marker
    /// the whole table is used (`:2716`, "Sprites must use full sprite RAM").
    ///
    /// A marker in record 0 gives −4. MAME holds that in a signed `int` and its
    /// `i >= 0` loop then draws nothing, which is what [`None`] means here. A
    /// `saturating_sub` to `Some(0)` would draw the very record the marker
    /// declares is not a sprite.
    pub fn last_offset(&self) -> Option<usize> {
        let mut i = 0;
        while i < OBJ_WORDS {
            // The test is on the high byte, not on the whole word: 0xFF01 is a
            // marker too.
            if self.words[i + 3] & 0xFF00 == 0xFF00 {
                return i.checked_sub(RECORD_WORDS);
            }
            i += RECORD_WORDS;
        }
        Some(OBJ_WORDS - RECORD_WORDS)
    }
}

/// Draws the latched sprites into `pens`, over whatever the layers left there.
///
/// `pens` and `prio` are `WIDTH * HEIGHT`, row-major. A sprite pixel is dropped
/// where `prio` is non-zero — that is a high-priority tile pixel occluding it,
/// MAME's `prio_transpen(..., screen.priority(), 0x02, 15)` (`cps1_v.cpp:2732`)
/// — and where the pixel decodes to [`TRANSPARENT_PEN`].
///
/// # Table order
///
/// Records draw **forwards**, so a later entry lands on top of an earlier one.
/// `cps1_v.cpp:2754` reads `for (int i = m_last_sprite_offset; i >= 0; i -= 4)`,
/// which looks like a backwards walk and is not: the record pointer is a separate
/// variable advancing `base += baseadd` (`:2836`) with `baseadd = 4` (`:2751`),
/// and `i` only counts how many records remain. The genuinely downward variant —
/// `base` starting at `m_last_sprite_offset`, `baseadd = -4` (`:2746`) — is
/// reached only under `bootleg_kludge` bit 6, commented "some sf2 hacks draw the
/// sprites in reverse order".
pub fn draw_sprites(
    pens: &mut [u16],
    prio: &[u8],
    latch: &ObjLatch,
    gfx: &[u8],
    mapper: &BankMapper,
) {
    assert_eq!(pens.len(), WIDTH * HEIGHT, "pens is the visible frame");
    assert_eq!(prio.len(), WIDTH * HEIGHT, "prio is the visible frame");

    let Some(last) = latch.last_offset() else {
        return;
    };
    let w = latch.words();

    for i in (0..=last).step_by(RECORD_WORDS) {
        let x = i32::from(w[i]);
        let y = i32::from(w[i + 1]);
        let attr = w[i + 3];

        // The mapper runs once, on the base code, before any block arithmetic
        // (`cps1_v.cpp:2764-2766`) — so a rejected code drops the whole sprite
        // rather than individual tiles of a block, and the block codes below are
        // derived from the *mapped* value.
        let Some(code) = mapper.map(GfxType::Sprite, u32::from(w[i + 2])) else {
            continue;
        };

        let colour = attr & 0x1F;
        let flip_x = attr & 0x0020 != 0;
        let flip_y = attr & 0x0040 != 0;
        // A lone sprite has `attr & 0xFF00 == 0`, which gives nx = ny = 1 and
        // takes this same path — no separate branch, so no untested branch.
        let nx = ((u32::from(attr) & 0x0F00) >> 8) + 1;
        let ny = ((u32::from(attr) & 0xF000) >> 12) + 1;

        for nys in 0..ny {
            let sy = (y + (nys * SPRITE_EDGE) as i32) & POS_MASK;
            for nxs in 0..nx {
                let sx = (x + (nxs * SPRITE_EDGE) as i32) & POS_MASK;
                // `cps1_v.cpp:2789` onward. The x term wraps within the low
                // nibble, so a block crossing a 16-code boundary repeats rather
                // than running on into the next sixteen codes; rows step by 0x10.
                let cx = if flip_x {
                    code + (nx - 1) - nxs
                } else {
                    code + nxs
                };
                let cy = if flip_y { ny - 1 - nys } else { nys };
                let tile = (code & !0x0F) + (cx & 0x0F) + 0x10 * cy;
                blit(pens, prio, gfx, tile, colour, flip_x, flip_y, sx, sy);
            }
        }
    }
}

/// Blits one 16×16 sprite tile with its top-left corner at `(sx, sy)`.
///
/// The position is in the 512×256 space MAME's `prio_transpen` draws into, which
/// is wider and taller than the visible 384×224, so a sprite near the right or
/// bottom edge is partly or wholly outside — clipped here rather than wrapped.
#[allow(clippy::too_many_arguments)]
fn blit(
    pens: &mut [u16],
    prio: &[u8],
    gfx: &[u8],
    code: u32,
    colour: u16,
    flip_x: bool,
    flip_y: bool,
    sx: i32,
    sy: i32,
) {
    for ty in 0..SPRITE_EDGE {
        // `sy` is a raster row; the visible frame starts at VISIBLE_Y.
        let py = sy + ty as i32 - VISIBLE_Y;
        if py < 0 || py >= HEIGHT as i32 {
            continue;
        }
        let ty_eff = if flip_y { SPRITE_EDGE - 1 - ty } else { ty };
        for tx in 0..SPRITE_EDGE {
            let px = sx + tx as i32 - VISIBLE_X;
            if px < 0 || px >= WIDTH as i32 {
                continue;
            }
            let i = py as usize * WIDTH + px as usize;
            // A high-priority tile pixel hides the sprite behind it.
            if prio[i] != 0 {
                continue;
            }
            let tx_eff = if flip_x { SPRITE_EDGE - 1 - tx } else { tx };
            let pen = tile_pen(gfx, TileKind::Tile16x16, code, tx_eff, ty_eff);
            if pen == TRANSPARENT_PEN {
                continue;
            }
            // No colour base: sprites own schemes 0x00-0x1F.
            pens[i] = colour * PEN_GRANULARITY + u16::from(pen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::BankRange;

    /// gfxram's size in words — 192 KB (`cps1.cpp:592`).
    const GFXRAM_WORDS: usize = 0x1_8000;

    /// The object table is 0x800 bytes, holding four-word records.
    #[test]
    fn the_object_table_is_two_kilobytes_of_four_word_records() {
        assert_eq!(OBJ_WORDS, 0x400, "cps1_v.cpp:2537: m_obj_size = 0x0800");
        assert_eq!(OBJ_WORDS * 2, 0x800, "bytes");
        assert_eq!(OBJ_BOUNDARY, 0x800, "the base register's alignment");
        assert_eq!(RECORD_WORDS, 4, "x, y, code, attr");
        assert_eq!(
            SPRITE_EDGE, 16,
            "cps1_v.cpp:2732 draws with gfx(2), the 16x16"
        );
        assert_eq!(PEN_GRANULARITY, 16, "a pen is colour * 16 + pixel");

        // The highest pen a sprite can produce, which is the constraint
        // [`UNTOUCHED`] has to satisfy. Any value above it serves equally, so a
        // mutation from one such value to another is equivalent — this assertion
        // is what states the constraint rather than leaving it to the choice of
        // 0xFFFF.
        let max_pen = 0x1F * PEN_GRANULARITY + 15;
        assert_eq!(max_pen, 0x1FF);
        assert!(
            max_pen < UNTOUCHED,
            "no sprite pen can be the sentinel a fixture pre-fills the frame with"
        );
    }

    /// The latch copies the table the object base register points at, and nothing
    /// past its end.
    #[test]
    fn the_latch_copies_from_the_obj_base_register() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        let mut cps_a = [0u16; 0x20];
        // 0x40 * 256 = 0x4000 bytes, already aligned to 0x800 -> word 0x2000.
        cps_a[OBJ_BASE] = 0x0040;
        assert_eq!(cps_a_base(&cps_a, OBJ_BASE, OBJ_BOUNDARY), 0x2000);
        gfxram[0x2000] = 0x1111;
        gfxram[0x2000 + OBJ_WORDS - 1] = 0x2222;
        // One word past the table, which must not be copied.
        gfxram[0x2000 + OBJ_WORDS] = 0x3333;

        let mut latch = ObjLatch::new();
        latch.latch(&gfxram, &cps_a);
        assert_eq!(latch.words()[0], 0x1111);
        assert_eq!(
            latch.words()[OBJ_WORDS - 1],
            0x2222,
            "the table's last word"
        );
        assert!(
            !latch.words().contains(&0x3333),
            "the copy stops at the end of the table"
        );
    }

    /// A base register resolving past the end of gfxram wraps rather than panics.
    ///
    /// [`cps_a_base`] documents why it can: it bounds the index to a 256 KB
    /// window and gfxram is 192 KB.
    #[test]
    fn a_base_past_the_end_of_gfxram_wraps() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        let mut cps_a = [0u16; 0x20];
        cps_a[OBJ_BASE] = 0xFFFF;
        // 0xFFFF * 256 = 0xFFFF00; & !0x7FF = 0xFFF800; & 0x3FFFF = 0x3F800;
        // / 2 = word 0x1FC00, which is 0x7C00 past the 0x18000-word array.
        assert_eq!(cps_a_base(&cps_a, OBJ_BASE, OBJ_BOUNDARY), 0x1_FC00);
        gfxram[0x7C00] = 0x4321;

        let mut latch = ObjLatch::new();
        latch.latch(&gfxram, &cps_a);
        assert_eq!(latch.words()[0], 0x4321, "0x1FC00 - 0x18000");
    }

    /// The latch holds the *previous* frame's table.
    ///
    /// `cps1_v.cpp:3067-3068`: "CPS1 sprites have to be delayed one frame". A
    /// renderer reading live objram puts every sprite one frame ahead of its
    /// layers, which on screen reads as jitter rather than as a bug — so this is
    /// the test that fails if the latch is ever turned into a borrow.
    #[test]
    fn the_latch_delays_the_table_by_a_frame() {
        let mut gfxram = vec![0u16; GFXRAM_WORDS];
        let cps_a = [0u16; 0x20];
        gfxram[0] = 0x00AA;
        gfxram[1] = 0x00BB;
        let mut latch = ObjLatch::new();
        latch.latch(&gfxram, &cps_a);

        gfxram[0] = 0x00CC;
        gfxram[1] = 0x00DD;
        assert_eq!(latch.words()[0], 0x00AA, "still the latched value");
        assert_eq!(latch.words()[1], 0x00BB);

        // And the next latch picks the new values up, so it is a delay and not a
        // one-shot read.
        latch.latch(&gfxram, &cps_a);
        assert_eq!(latch.words()[0], 0x00CC);
        assert_eq!(latch.words()[1], 0x00DD);
    }

    /// The end marker stops the table two records early, and one in record 0
    /// leaves nothing to draw.
    ///
    /// `find_last_sprite` (`cps1_v.cpp:2704-2708`) answers `offset - 4` for a
    /// marker at `offset`, so the record immediately before the marker is skipped
    /// too: a marker in record 2 leaves records **0 and 1**. Answering `offset`
    /// instead is the natural way to get this wrong, and yields one extra sprite
    /// per frame.
    #[test]
    fn the_end_marker_stops_the_table_two_records_early() {
        let mut latch = ObjLatch::new();

        // No marker anywhere: the whole table (`cps1_v.cpp:2716`).
        assert_eq!(latch.last_offset(), Some(0x3FC));
        assert_eq!(0x3FC, OBJ_WORDS - RECORD_WORDS, "the table's last record");

        // Marker in record 2 (word offset 8) -> 4, so records 0 and 1 draw.
        latch.words[8 + 3] = 0xFF00;
        assert_eq!(latch.last_offset(), Some(4));

        // 0xFF01 is a marker too: the test is `(attr & 0xFF00) == 0xFF00`, not
        // `attr == 0xFF00`. In record 1 it gives 0 — record 0 alone.
        latch.words[8 + 3] = 0;
        latch.words[4 + 3] = 0xFF01;
        assert_eq!(latch.last_offset(), Some(0));

        // 0xFE00 is not a marker, so the scan runs on to the end of the table.
        latch.words[4 + 3] = 0xFE00;
        assert_eq!(latch.last_offset(), Some(0x3FC));

        // A marker in record 0 gives −4, which MAME's `i >= 0` loop skips
        // entirely. `None` is that case; `Some(0)` — what `saturating_sub` gives
        // — would draw the very record the marker declares is not a sprite.
        latch.words[4 + 3] = 0;
        latch.words[3] = 0xFF00;
        assert_eq!(latch.last_offset(), None);
    }

    /// A marker in record 0 leaves the framebuffer untouched.
    #[test]
    fn a_marker_in_record_zero_draws_no_sprites() {
        let mut f = Fixture::new();
        // Record 0 is an otherwise perfectly good solid sprite — its attribute is
        // also a marker, so nothing draws.
        f.put(0, 0, 0, SOLID_CODE, 0xFF00);
        assert_eq!(f.latch.last_offset(), None, "the premise");
        assert!(
            f.render().is_blank(),
            "no sprite drew, not even the marker record"
        );

        // The same record without the marker bits does draw, so the blank frame
        // above is the marker's doing and not a broken fixture.
        f.put(0, 0, 0, SOLID_CODE, 0x0002);
        assert_eq!(f.render().px(0, 0), Some(0x2A), "colour 2, pen 0x0A");
    }

    /// Sprites draw in table order, so a later entry lands on top.
    ///
    /// `cps1_v.cpp:2754` reads `for (int i = m_last_sprite_offset; i >= 0; i -= 4)`
    /// and looks like a backwards walk. It is not: the record pointer is a
    /// separate variable advancing `base += baseadd` (`:2836`) with `baseadd = 4`
    /// (`:2751`), and `i` only counts how many records remain. The genuinely
    /// downward variant — `base` from `m_last_sprite_offset`, `baseadd = -4`
    /// (`:2746`) — is reached only under `bootleg_kludge` bit 6, commented "some
    /// sf2 hacks draw the sprites in reverse order".
    ///
    /// Both overlap directions are asserted against literal pens, so the test
    /// says *which* sprite wins rather than merely that one of them does.
    #[test]
    fn sprites_draw_in_table_order_so_a_later_entry_lands_on_top() {
        let mut f = Fixture::new();
        // Two sprites in the same place, colours 1 and 2, with the marker moved
        // out to record 2 so both draw.
        f.put(0, 0, 0, SOLID_CODE, 0x0001);
        f.put(1, 0, 0, SOLID_CODE, 0x0002);
        f.mark(2);
        assert_eq!(f.latch.last_offset(), Some(4), "records 0 and 1 both draw");
        let r = f.render();
        assert_eq!(
            r.px(0, 0),
            Some(0x2A),
            "record 1 is later in the table, so its colour 2 wins"
        );
        // The two sprites coincide, so exactly one tile's worth of pixels is
        // opaque. A loop stepping by anything but four words reads the middle of a
        // record as the start of one, and those phantom sprites draw somewhere.
        assert_eq!(r.opaque(), 16 * 16, "two coincident tiles, one tile drawn");

        // Swap the two colours and the answer swaps with them.
        f.put(0, 0, 0, SOLID_CODE, 0x0002);
        f.put(1, 0, 0, SOLID_CODE, 0x0001);
        assert_eq!(
            f.render().px(0, 0),
            Some(0x1A),
            "record 1 is still later, so now colour 1 wins"
        );
    }

    /// A sprite lands at its x and y, masked to nine bits.
    #[test]
    fn a_sprite_lands_at_its_x_and_y_masked_to_nine_bits() {
        let mut f = Fixture::new();
        f.put(0, 40, 30, SOLID_CODE, 0x0001);
        let r = f.render();
        assert_eq!(r.px(40, 30), Some(0x1A), "the tile's top-left");
        assert_eq!(r.px(55, 45), Some(0x1A), "and its bottom-right, 16 apart");
        assert_eq!(r.px(39, 30), None);
        assert_eq!(r.px(56, 45), None);

        // A raster position past 511 wraps: 0x201 & 0x1FF = 1. Under a 0x3FF mask
        // it would stay at 513, off the right edge of the raster, and the sprite
        // would vanish instead of reappearing at raster column 1.
        assert_eq!(0x201 & POS_MASK, 1);
        // Raster (0x201, 0x202) wraps to raster (1, 2), which is inside the
        // blanking region — so the wrap is visible only as far as the window
        // reaches it. Place the wrapped sprite where the window can see it:
        // raster 0x240 wraps to 0x40 = 64 = VISIBLE_X, i.e. visible column 0.
        assert_eq!(0x240 & POS_MASK, VISIBLE_X);
        assert_eq!(0x210 & POS_MASK, VISIBLE_Y);
        f.put_raw(0, 0x240, 0x210, SOLID_CODE, 0x0001);
        assert_eq!(
            f.render().px(0, 0),
            Some(0x1A),
            "0x240 -> raster 64 -> visible 0; 0x210 -> raster 16 -> visible 0"
        );
    }

    /// The visible window is the raster sub-rectangle at (64, 16).
    ///
    /// The assertion that fixes the sprite offset, written against literals rather
    /// than against [`VISIBLE_X`]/[`VISIBLE_Y`] — so changing those constants fails
    /// here rather than silently moving every sprite. A sprite whose register
    /// position is (64, 16) lands at visible (0, 0); one at (0, 0) is entirely
    /// inside blanking and invisible.
    #[test]
    fn the_visible_window_is_the_raster_subrectangle_at_sixty_four_sixteen() {
        assert_eq!((VISIBLE_X, VISIBLE_Y), (64, 16), "cps1.h:42, :46");

        let mut f = Fixture::new();
        f.put_raw(0, 64, 16, SOLID_CODE, 0x0001);
        let r = f.render();
        assert_eq!(r.px(0, 0), Some(0x1A), "raster (64,16) is visible (0,0)");
        assert_eq!(r.px(15, 15), Some(0x1A));
        assert_eq!(r.opaque(), 16 * 16, "the whole tile, and nothing else");

        // A sprite at raster (0, 0) sits in the blanking region: its rows are
        // above the window and its columns left of it, so none of it is visible.
        f.put_raw(0, 0, 0, SOLID_CODE, 0x0001);
        assert!(
            f.render().is_blank(),
            "raster (0,0) is inside blanking, not the top-left pixel"
        );

        // One pixel short of the window in each axis still shows the overlap, so
        // the boundary is exact rather than "far enough away".
        f.put_raw(0, 64 - 1, 16 - 1, SOLID_CODE, 0x0001);
        let r = f.render();
        assert_eq!(r.px(0, 0), Some(0x1A), "the tile's last row and column");
        assert_eq!(
            r.opaque(),
            15 * 15,
            "exactly the 15x15 corner inside the window"
        );
    }

    /// Pen 15 of a sprite is transparent.
    #[test]
    fn pen_fifteen_of_a_sprite_is_transparent() {
        let mut f = Fixture::new();
        f.put(0, 0, 0, BLANK_CODE, 0x0001);
        assert!(f.render().is_blank(), "an all-pen-15 sprite draws nothing");

        // The same record with the solid code does draw, so the blank frame is
        // the pen's doing.
        f.put(0, 0, 0, SOLID_CODE, 0x0001);
        assert_eq!(f.render().px(0, 0), Some(0x1A));
    }

    /// A sprite's colour is its low five attribute bits, over palette page 0.
    ///
    /// Sprites own schemes 0x00-0x1F; the tilemaps add 0x20, 0x40, 0x60 (see
    /// [`crate::layers::Layer::colour_base`]).
    #[test]
    fn sprite_colours_come_from_the_low_five_bits_with_no_base() {
        let mut f = Fixture::new();
        // Scheme 0x0A, pen 0x0A -> 0x0A * 16 + 0x0A.
        f.put(0, 0, 0, SOLID_CODE, 0x000A);
        assert_eq!(f.render().px(0, 0), Some(0xAA));
        // Scheme 0x1F is the top of the sprite region.
        f.put(0, 0, 0, SOLID_CODE, 0x001F);
        assert_eq!(f.render().px(0, 0), Some(0x1FA));
        // Bit 5 is X flip, not a sixth colour bit.
        f.put(0, 0, 0, SOLID_CODE, 0x0020);
        assert_eq!(f.render().px(0, 0), Some(0x0A), "scheme 0, not 0x20");
    }

    /// Per-sprite flip mirrors the pixels within the tile.
    ///
    /// The block tests cannot see this: their tiles are solid, so mirroring one is
    /// invisible. This one's tile has a single opaque pixel in a known corner.
    #[test]
    fn per_sprite_flip_mirrors_the_pixels_within_the_tile() {
        let mut f = Fixture::with_gfx(corner_tile());
        for (attr, want_x, want_y) in [
            (0x0001, 0, 0),
            (0x0021, 15, 0),
            (0x0041, 0, 15),
            (0x0061, 15, 15),
        ] {
            f.put(0, 0, 0, SOLID_CODE, attr);
            let r = f.render();
            assert_eq!(
                r.opaque_region(),
                Some(Region {
                    x: want_x,
                    y: want_y,
                    w: 1,
                    h: 1,
                    count: 1,
                }),
                "attr {attr:#06x}"
            );
            // Colour 1, pen CORNER_PEN — so the flip bits moved the pixel without
            // also being read as colour bits.
            assert_eq!(
                r.px(want_x, want_y),
                Some(PEN_GRANULARITY + u16::from(CORNER_PEN)),
                "attr {attr:#06x}"
            );
        }
    }

    /// The code drawn is the one the bank mapper answers, not the raw code.
    ///
    /// The other tests use a mapper that is the identity, so they cannot tell the
    /// two apart. This one's mapper shifts every code by 0x10.
    #[test]
    fn a_sprite_draws_the_mapped_code_and_not_the_raw_one() {
        // bank 0 is 0x20 units wide and holds nothing; the sprite range starts at
        // unit 0x20 and lives in bank 1, so a code's unit gains 0x20 — half of
        // which, back in 16x16 codes, is 0x10.
        static SHIFTED: [BankRange; 1] = [BankRange {
            kind: GfxType::Sprite,
            start: 0x20,
            end: 0xFFFF,
            bank: 1,
        }];
        let mapper = BankMapper {
            bank_sizes: [0x20, 0x1_0000, 0, 0],
            ranges: &SHIFTED,
        };
        assert_eq!(mapper.map(GfxType::Sprite, 0x10), Some(0x20), "the premise");

        // Only tile 0x20 is opaque, and code 0x10 draws it.
        let mut f = Fixture::with_gfx(only_tile_gfx(0x20));
        f.mapper = mapper;
        f.put(0, 0, 0, 0x10, 0x0001);
        assert_eq!(f.render().opaque_region(), one_tile_at(0, 0));

        // Only tile 0x10 is opaque — the raw code — and now nothing draws.
        let mut f = Fixture::with_gfx(only_tile_gfx(0x10));
        f.mapper = mapper;
        f.put(0, 0, 0, 0x10, 0x0001);
        assert!(f.render().is_blank(), "the raw code is not the one drawn");
    }

    /// A blocked sprite tiles its codes, wrapping within the low nibble.
    ///
    /// `cps1_v.cpp:2789`: the tile code is
    /// `(code & ~0xF) + ((code + nxs) & 0xF) + 0x10 * nys`. The x term wraps
    /// within the nibble, so a block crossing a 16-code boundary repeats rather
    /// than running on into the next sixteen codes.
    #[test]
    fn a_blocked_sprite_tiles_its_codes_within_the_low_nibble() {
        // attr 0x1200 -> nx = ((0x1200 & 0x0F00) >> 8) + 1 = 3,
        //                ny = ((0x1200 & 0xF000) >> 12) + 1 = 2.
        assert_block(0x1E, 0x1200, &[&[0x1E, 0x1F, 0x10], &[0x2E, 0x2F, 0x20]]);
    }

    /// The mapper runs once, on the base code, so a rejected code drops the whole
    /// block.
    ///
    /// `cps1_v.cpp:2764-2766` maps before the block loops. A reimplementation
    /// mapping each block tile instead would differ wherever a bank boundary fell
    /// inside a block, and would drop single tiles rather than the sprite.
    #[test]
    fn a_blocked_sprite_the_mapper_rejects_draws_no_tile_at_all() {
        let mut f = Fixture::new();
        assert_eq!(
            f.mapper.map(GfxType::Sprite, 0x8000),
            None,
            "the fixture's premise"
        );
        f.put(0, 0, 0, 0x8000, 0x1200);
        assert!(f.render().is_blank(), "no tile of the 3x2 block drew");
    }

    /// Under flip, the block counts down from the far end.
    ///
    /// `cps1_v.cpp:2789` uses `(code + (nx - 1) - nxs) & 0xF` under X flip and
    /// `0x10 * (ny - 1 - nys)` under Y flip. Read as screen order, the expected
    /// grid is the unflipped one mirrored on the flipped axis — which is what
    /// makes `(nx - 1) - nxs` distinguishable from `nx - nxs`.
    #[test]
    fn a_blocked_sprite_counts_down_from_the_far_end_under_flip() {
        // The unflipped grid is [[0x1E, 0x1F, 0x10], [0x2E, 0x2F, 0x20]].
        // X flip mirrors the columns and leaves the rows alone.
        assert_block(
            0x1E,
            0x1200 | 0x0020,
            &[&[0x10, 0x1F, 0x1E], &[0x20, 0x2F, 0x2E]],
        );
        // Y flip mirrors the rows and leaves the columns alone.
        assert_block(
            0x1E,
            0x1200 | 0x0040,
            &[&[0x2E, 0x2F, 0x20], &[0x1E, 0x1F, 0x10]],
        );
        // Both mirror both.
        assert_block(
            0x1E,
            0x1200 | 0x0020 | 0x0040,
            &[&[0x20, 0x2F, 0x2E], &[0x10, 0x1F, 0x1E]],
        );
    }

    /// The block sizes are the attribute nibbles plus one, so a plain sprite is
    /// one tile.
    #[test]
    fn a_blocks_size_is_the_attribute_nibble_plus_one() {
        // attr 0x0000: a single tile. Without the `+ 1` nothing draws at all.
        assert_block(0x1E, 0x0000, &[&[0x1E]]);
        // 0x0100 is two wide, 0x1000 two tall — and `cells_of` scans the whole
        // frame, so each of these also asserts the other axis stayed at one.
        assert_block(0x1E, 0x0100, &[&[0x1E, 0x1F]]);
        assert_block(0x1E, 0x1000, &[&[0x1E], &[0x2E]]);
        // The nibbles do not overlap: 0x0F00 is sixteen wide by one tall.
        assert_eq!(region_of(0x00, 0x00, 0x0F00), one_tile_at(0, 0));
        assert_eq!(region_of(0x0F, 0x00, 0x0F00), one_tile_at(15, 0));
    }

    /// A high-priority tile pixel hides the sprite behind it.
    ///
    /// Task 8 fills `prio`; this pins that [`draw_sprites`] reads it.
    /// `cps1_v.cpp:2732` passes `screen.priority(), 0x02` to `prio_transpen`.
    #[test]
    fn a_high_priority_tile_pixel_occludes_a_sprite() {
        let mut f = Fixture::new();
        f.put(0, 0, 0, SOLID_CODE, 0x0001);
        assert_eq!(f.render().px(0, 0), Some(0x1A), "with nothing occluding");

        // One pixel marked, and only that pixel is dropped.
        f.prio[0] = 1;
        let r = f.render();
        assert_eq!(r.px(0, 0), None, "the marked pixel is occluded");
        assert_eq!(r.px(1, 0), Some(0x1A), "its neighbour is not");
    }

    /// A code no bank range covers draws nothing.
    #[test]
    fn an_out_of_range_sprite_code_draws_nothing() {
        let mut f = Fixture::new();
        assert_eq!(f.mapper.map(GfxType::Sprite, 0x8000), None);
        f.put(0, 0, 0, 0x8000, 0x0001);
        assert!(f.render().is_blank());
    }

    /// A sprite straddling the right or bottom edge is clipped, not wrapped.
    ///
    /// The position lives in the 512×262 raster, wider and taller than the visible
    /// 384×224, so a sprite over the window's edge must lose the part outside it
    /// rather than reappear on the opposite side.
    #[test]
    fn a_sprite_straddling_the_edge_is_clipped() {
        let mut f = Fixture::new();

        // Eight pixels past the right edge.
        f.put(0, WIDTH as i32 - 8, 0, SOLID_CODE, 0x0001);
        let r = f.render();
        assert_eq!(r.px(WIDTH - 1, 0), Some(0x1A), "the visible half drew");
        assert_eq!(r.px(0, 0), None, "and did not wrap to the left edge");
        // Eight columns of sixteen rows, and nothing else anywhere. Clipping at
        // 512 instead of the visible width would fold the off-screen columns onto
        // the start of the following row rather than dropping them.
        assert_eq!(r.opaque(), 8 * 16, "only the eight visible columns");

        // And eight past the bottom.
        f.put(0, 0, HEIGHT as i32 - 8, SOLID_CODE, 0x0001);
        let r = f.render();
        assert_eq!(r.px(0, HEIGHT - 1), Some(0x1A));
        assert_eq!(r.px(0, 0), None, "no wrap to the top");
    }

    // ---------------------------------------------------------------- fixtures

    /// The pen the fixture's opaque tiles are solid in.
    const SOLID_PEN: u8 = 0x0A;
    /// The code of that tile in [`Fixture::new`]'s graphics.
    const SOLID_CODE: u16 = 0;
    /// A tile of nothing but the transparent pen.
    const BLANK_CODE: u16 = 1;

    static SPRITE_RANGE: [BankRange; 1] = [BankRange {
        kind: GfxType::Sprite,
        start: 0,
        end: 0xFFFF,
        bank: 0,
    }];

    /// A mapper that is the identity on small codes.
    ///
    /// STF29's real sprite ranges are exercised in `bank.rs` against its own
    /// literals. Here a single 0x10000-unit bank makes `map` the identity below
    /// code 0x8000, so these tests pin the sprite logic rather than the bank
    /// arithmetic. Code 0x8000 shifts to unit 0x10000 and misses every range,
    /// which is where the rejection tests get their `None`.
    fn fixture_mapper() -> BankMapper {
        BankMapper {
            bank_sizes: [0x1_0000, 0, 0, 0],
            ranges: &SPRITE_RANGE,
        }
    }

    /// The fixture mapper is the identity below 0x8000 and rejects 0x8000.
    #[test]
    fn the_fixture_mapper_is_the_identity_on_small_codes() {
        let m = fixture_mapper();
        for code in [0u32, 1, 0x0F, 0x1E, 0x2F, 0x7FFF] {
            assert_eq!(m.map(GfxType::Sprite, code), Some(code), "{code:#x}");
        }
        assert_eq!(m.map(GfxType::Sprite, 0x8000), None);
    }

    /// A 16×16 tile every pixel of which is `pen`.
    ///
    /// Written from the plane *byte* structure, never from [`tile_pen`]'s
    /// within-byte bit arithmetic: a solid tile's plane bytes are all 0x00 or all
    /// 0xFF, and a group's four bytes are pen bits 0, 1, 2, 3 in memory order. A
    /// 16×16 tile's row is eight bytes — two groups, the left and right halves.
    /// [`solid_bytes_are_solid`] pins it.
    fn solid_tile(pen: u8) -> Vec<u8> {
        let byte_for = |bit: u8| if pen & (1 << bit) != 0 { 0xFFu8 } else { 0x00 };
        let group = [byte_for(0), byte_for(1), byte_for(2), byte_for(3)];
        let mut rom = vec![0u8; TileKind::Tile16x16.bytes()];
        for row in 0..16 {
            for half in [0usize, 4] {
                rom[row * 8 + half..][..4].copy_from_slice(&group);
            }
        }
        rom
    }

    /// The fixture's solid tiles really are solid, in every pen.
    #[test]
    fn solid_bytes_are_solid() {
        for pen in 0..16u8 {
            let rom = solid_tile(pen);
            assert_eq!(rom.len(), 128, "a 16x16 tile is 128 bytes");
            for y in 0..16 {
                for x in 0..16 {
                    assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 0, x, y), pen);
                }
            }
        }
    }

    /// The pen of [`corner_tile`]'s single opaque pixel.
    ///
    /// Distinct from [`SOLID_PEN`] so that every one of the four plane bytes has
    /// to be touched: the tile starts out solid in [`TRANSPARENT_PEN`], where all
    /// four plane bits are 1, so only a *zero* bit of the corner pen clears
    /// anything — and 0x05 has zeroes in bits 1 and 3, the two extreme bytes of
    /// the group.
    const CORNER_PEN: u8 = 0x05;

    /// A single 16×16 tile whose only opaque pixel is its top-left one.
    ///
    /// Written from the plane byte structure like [`solid_tile`]: start from a tile
    /// solid in [`TRANSPARENT_PEN`] and give pixel (0, 0) alone the bits of
    /// [`CORNER_PEN`]. Row 0's left group is bytes 0-3, carrying pen bits 0-3 in
    /// memory order, and within each of those the leftmost pixel is the most
    /// significant bit. [`corner_bytes_are_a_single_pixel`] pins it.
    fn corner_tile() -> Vec<u8> {
        let mut rom = solid_tile(TRANSPARENT_PEN);
        for (bit, b) in rom[0..4].iter_mut().enumerate() {
            if CORNER_PEN & (1 << bit) == 0 {
                *b &= 0x7F;
            }
        }
        rom
    }

    /// The corner tile is opaque in exactly one pixel.
    #[test]
    fn corner_bytes_are_a_single_pixel() {
        let rom = corner_tile();
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 0, 0, 0), CORNER_PEN);
        for y in 0..16 {
            for x in 0..16 {
                if (x, y) != (0, 0) {
                    assert_eq!(
                        tile_pen(&rom, TileKind::Tile16x16, 0, x, y),
                        TRANSPARENT_PEN,
                        "({x}, {y})"
                    );
                }
            }
        }
    }

    /// Graphics in which `code` is the only opaque tile.
    ///
    /// Every lower code is a tile of the transparent pen — not absent bytes, and
    /// not zeroes, because an all-zero tile decodes to pen 0, which is opaque.
    /// Codes above `code` fall outside the slice, which [`tile_pen`] already
    /// answers as transparent.
    fn only_tile_gfx(code: u32) -> Vec<u8> {
        let mut rom = solid_tile(TRANSPARENT_PEN).repeat(code as usize);
        rom.extend(solid_tile(SOLID_PEN));
        rom
    }

    /// Exactly one tile of `only_tile_gfx` is opaque.
    #[test]
    fn only_tile_gfx_has_a_single_opaque_tile() {
        let rom = only_tile_gfx(3);
        for code in 0..3 {
            assert_eq!(
                tile_pen(&rom, TileKind::Tile16x16, code, 0, 0),
                TRANSPARENT_PEN,
                "code {code} is transparent, not pen 0"
            );
        }
        assert_eq!(tile_pen(&rom, TileKind::Tile16x16, 3, 0, 0), SOLID_PEN);
        assert_eq!(
            tile_pen(&rom, TileKind::Tile16x16, 4, 0, 0),
            TRANSPARENT_PEN,
            "past the end of the slice"
        );
    }

    /// Where the opaque pixels of a render are: a bounding box and a count.
    ///
    /// A box of 16 by 16 holding 256 opaque pixels can only be a full 16×16 tile
    /// at that exact position, so the pair pins position and extent together —
    /// which sampling one pixel per tile cell would not: a block whose rows step
    /// by 8 instead of 16 still covers the pixel 16 rows down.
    #[derive(Debug, PartialEq, Eq)]
    struct Region {
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        count: usize,
    }

    /// The single 16×16 tile a correct block draws at tile cell `(col, row)`.
    fn one_tile_at(col: usize, row: usize) -> Option<Region> {
        Some(Region {
            x: col * 16,
            y: row * 16,
            w: 16,
            h: 16,
            count: 256,
        })
    }

    /// The opaque region of a sprite at (0, 0) whose only opaque tile is `only`.
    ///
    /// The whole visible frame is scanned, not just the expected cell, so a block
    /// one tile too wide, a stray extra tile, or a tile eight pixels out of place
    /// all change the answer.
    fn region_of(only: u32, base_code: u16, attr: u16) -> Option<Region> {
        let mut f = Fixture::with_gfx(only_tile_gfx(only));
        f.put(0, 0, 0, base_code, attr);
        f.render().opaque_region()
    }

    /// Asserts that a block of `base_code`/`attr` draws exactly the given codes,
    /// each as a full tile in exactly its own cell.
    ///
    /// One render per expected code, each against graphics in which that code is
    /// the only opaque tile — so every assertion is "code `c` drew in this cell,
    /// whole, and nowhere else", which also catches a block repeating a code it
    /// should not.
    fn assert_block(base_code: u16, attr: u16, want: &[&[u32]]) {
        for (row, codes) in want.iter().enumerate() {
            for (col, &code) in codes.iter().enumerate() {
                assert_eq!(
                    region_of(code, base_code, attr),
                    one_tile_at(col, row),
                    "base {base_code:#06x} attr {attr:#06x}: code {code:#x} \
                     belongs in cell ({col}, {row}) alone"
                );
            }
        }
    }

    /// A scratch board: an object latch, graphics, a mapper, a priority buffer.
    struct Fixture {
        latch: ObjLatch,
        gfx: Vec<u8>,
        mapper: BankMapper,
        prio: Vec<u8>,
    }

    impl Fixture {
        /// Graphics with tile 0 solid in [`SOLID_PEN`] and tile 1 transparent.
        fn new() -> Self {
            let mut gfx = solid_tile(SOLID_PEN);
            gfx.extend(solid_tile(TRANSPARENT_PEN));
            Self::with_gfx(gfx)
        }

        /// An end marker sits in record 1, so a test that writes record 0 gets one
        /// sprite and the 0x3FE zero-filled records behind it stay out of the way.
        fn with_gfx(gfx: Vec<u8>) -> Self {
            let mut f = Self {
                latch: ObjLatch::new(),
                gfx,
                mapper: fixture_mapper(),
                prio: vec![0u8; WIDTH * HEIGHT],
            };
            f.mark(1);
            f
        }

        /// Writes sprite record `rec`, with the position as the hardware register
        /// holds it — a **raster** coordinate.
        fn put_raw(&mut self, rec: usize, x: u16, y: u16, code: u16, attr: u16) {
            let i = rec * RECORD_WORDS;
            self.latch.words[i] = x;
            self.latch.words[i + 1] = y;
            self.latch.words[i + 2] = code;
            self.latch.words[i + 3] = attr;
        }

        /// Writes sprite record `rec` with the position given in **visible-frame**
        /// coordinates, converted to the raster value the register holds.
        ///
        /// Most tests here care about a sprite's pixels relative to the frame they
        /// assert against, not about the blanking offset, so they place sprites
        /// through this. The offset is not laundered by doing so: it is pinned
        /// against literals by
        /// [`the_visible_window_is_the_raster_subrectangle_at_sixty_four_sixteen`].
        fn put(&mut self, rec: usize, x: i32, y: i32, code: u16, attr: u16) {
            let rx = (x + VISIBLE_X) as u16;
            let ry = (y + VISIBLE_Y) as u16;
            self.put_raw(rec, rx, ry, code, attr);
        }

        /// Puts an end marker in record `rec`.
        fn mark(&mut self, rec: usize) {
            self.latch.words[rec * RECORD_WORDS + 3] = 0xFF00;
        }

        fn render(&self) -> Rendered {
            let mut r = Rendered {
                pens: vec![UNTOUCHED; WIDTH * HEIGHT],
            };
            draw_sprites(
                &mut r.pens,
                &self.prio,
                &self.latch,
                &self.gfx,
                &self.mapper,
            );
            r
        }
    }

    /// The sentinel a fixture pre-fills `pens` with.
    ///
    /// No real pen can be it: the palette is 0xC00 entries and a sprite reaches
    /// 0x1F * 16 + 15 = 0x1FF.
    const UNTOUCHED: u16 = 0xFFFF;

    struct Rendered {
        pens: Vec<u16>,
    }

    impl Rendered {
        fn px(&self, x: usize, y: usize) -> Option<u16> {
            match self.pens[y * WIDTH + x] {
                UNTOUCHED => None,
                p => Some(p),
            }
        }

        fn is_blank(&self) -> bool {
            self.opaque() == 0
        }

        /// How many pixels of the visible frame a sprite wrote.
        fn opaque(&self) -> usize {
            self.pens.iter().filter(|&&p| p != UNTOUCHED).count()
        }

        /// The bounding box of the opaque pixels, with their count.
        fn opaque_region(&self) -> Option<Region> {
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    if self.px(x, y).is_some() {
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x);
                        y1 = y1.max(y);
                    }
                }
            }
            if x0 == usize::MAX {
                return None;
            }
            Some(Region {
                x: x0,
                y: y0,
                w: x1 - x0 + 1,
                h: y1 - y0 + 1,
                count: self.opaque(),
            })
        }
    }
}
