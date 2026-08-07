//! Composition: layer order, layer enables, the background fill, and the flip.
//!
//! This is the module that turns a board's registers into a finished frame.
//! Everything below it draws one thing; this decides what is drawn, in what
//! order, and where the result ends up.
//!
//! # Back to front, and what "order" means
//!
//! `render_layers` (`cps1_v.cpp:2971-2999`) reads four 2-bit fields out of the
//! layer-control register and draws them in field order, `l0` first. So `l0` is
//! the **back** and `l3` the front: a later field lands on top. Each field's
//! value selects what goes at that depth — 0 for the sprites, 1, 2, 3 for the
//! corresponding scroll layer. A value can repeat, and SF2's own 0x1B40 does
//! repeat 1, which is why [`layer_order`]'s tests also pin a value with four
//! distinct fields.
//!
//! # Positions
//!
//! Sprites and layers are placed in raster coordinates; see the crate
//! documentation. This module only crops and mirrors, and both operations are
//! stated there in terms of the same pivots.

use crate::bank::BankMapper;
use crate::layers::{draw_tilemap, Layer, ScrollRows};
use crate::palette::{self, build_palette, entry_to_rgb, BACKGROUND_PEN};
use crate::regs::{
    cps_a_base, VideoConfig, SCROLL1_BASE, SCROLL1_X, SCROLL1_Y, SCROLL2_BASE, SCROLL2_X,
    SCROLL2_Y, SCROLL3_BASE, SCROLL3_X, SCROLL3_Y, SCROLL_BOUNDARY, VIDEOCONTROL,
};
use crate::sprites::{draw_sprites, ObjLatch};
use crate::{HEIGHT, WIDTH};

/// The four depths `layercontrol` selects a layer for.
const DEPTHS: usize = 4;

/// A finished frame: one palette pen per pixel, and the sprite priority mask.
#[derive(Debug, Clone)]
pub struct Framebuffer {
    /// Palette pens, row-major, `WIDTH * HEIGHT`.
    pub pens: Box<[u16]>,
    /// 1 where a high-priority tile pixel occludes the sprites, else 0.
    pub prio: Box<[u8]>,
}

impl Default for Framebuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Framebuffer {
    /// A frame cleared to the background pen.
    pub fn new() -> Self {
        Self {
            pens: vec![BACKGROUND_PEN; WIDTH * HEIGHT].into_boxed_slice(),
            prio: vec![0u8; WIDTH * HEIGHT].into_boxed_slice(),
        }
    }

    /// Clears to the background pen, and the priority mask to zero.
    ///
    /// `cps1_v.cpp:3050-3052`: "Games use pen 0xbff as background color". The
    /// priority fill is `screen.priority().fill(0, cliprect)` at `:2978`.
    fn clear(&mut self) {
        self.pens.fill(BACKGROUND_PEN);
        self.prio.fill(0);
    }

    /// Mirrors the frame about its own centre, both axes.
    ///
    /// One pass over the finished buffer, which the crate documentation shows is
    /// equivalent to MAME's per-primitive flip: the visible window is symmetric
    /// within the raster pivots 511 and 255, so mirroring the crop is the same
    /// picture as cropping the mirror. `prio` is mirrored too — it describes the
    /// same pixels.
    fn flip(&mut self) {
        self.pens.reverse();
        self.prio.reverse();
    }
}

/// What is drawn at each of the four depths, back to front.
///
/// Four 2-bit fields of the layer-control register, from bit 6 up
/// (`cps1_v.cpp:2974-2977`). A value of 0 is the sprites; 1, 2, 3 are scroll 1,
/// 2, 3.
pub fn layer_order(layercontrol: u16) -> [u8; DEPTHS] {
    let field = |shift: u32| ((layercontrol >> shift) & 0x03) as u8;
    [field(6), field(8), field(10), field(12)]
}

/// A CPS-1 video subsystem: the configuration, the graphics, and a frame.
#[derive(Debug, Clone)]
pub struct Video {
    /// Which CPS-B registers this board uses.
    pub cfg: VideoConfig,
    /// The board's graphics-ROM bank mapping.
    pub mapper: BankMapper,
    /// The most recently rendered frame.
    pub fb: Framebuffer,
    /// The graphics ROM. Supplied by the caller; this crate holds no ROM.
    gfx: Vec<u8>,
    /// The previous frame's object table.
    obj: ObjLatch,
    /// The palette, rebuilt each frame.
    pal: Box<[u16; palette::PENS]>,
}

impl Video {
    /// A video subsystem for a board, with its graphics ROM.
    pub fn new(cfg: VideoConfig, mapper: BankMapper, gfx: Vec<u8>) -> Self {
        Self {
            cfg,
            mapper,
            fb: Framebuffer::new(),
            gfx,
            obj: ObjLatch::new(),
            pal: Box::new([0u16; palette::PENS]),
        }
    }

    /// Latches the object table for the next frame.
    ///
    /// Call once per frame, at vblank: CPS-1 sprites are delayed one frame
    /// (`cps1_v.cpp:3067-3068`), so the frame [`Self::render`] draws uses the
    /// table as it stood when this was last called.
    pub fn latch_objects(&mut self, gfxram: &[u16], cps_a: &[u16]) {
        self.obj.latch(gfxram, cps_a);
    }

    /// Renders one frame into [`Self::fb`].
    ///
    /// The order is MAME's `screen_update_cps1` (`cps1_v.cpp:3040-3057`) followed
    /// by `render_layers` (`:2971-2999`): build the palette, clear to the
    /// background pen, then draw the four depths back to front, then flip.
    pub fn render(&mut self, gfxram: &[u16], cps_a: &[u16], cps_b: &[u16]) {
        build_palette(
            gfxram,
            cps_a,
            cps_b[self.cfg.palette_control],
            &mut self.pal,
        );
        self.fb.clear();

        let layercontrol = cps_b[self.cfg.layer_control];
        let videocontrol = cps_a[VIDEOCONTROL];
        let hi_pens = self.hi_pens(cps_b);

        for want in layer_order(layercontrol) {
            match want {
                0 => draw_sprites(
                    &mut self.fb.pens,
                    &self.fb.prio,
                    &self.obj,
                    &self.gfx,
                    &self.mapper,
                ),
                n => {
                    let layer = match n {
                        1 => Layer::Scroll1,
                        2 => Layer::Scroll2,
                        // `layer_order` masks to two bits, so this is 3.
                        _ => Layer::Scroll3,
                    };
                    if !layer_enabled(&self.cfg, layer, layercontrol, videocontrol) {
                        continue;
                    }
                    let (base, sx, sy) = match layer {
                        Layer::Scroll1 => (SCROLL1_BASE, SCROLL1_X, SCROLL1_Y),
                        Layer::Scroll2 => (SCROLL2_BASE, SCROLL2_X, SCROLL2_Y),
                        Layer::Scroll3 => (SCROLL3_BASE, SCROLL3_X, SCROLL3_Y),
                    };
                    // The registers are unsigned words holding signed scrolls, so
                    // they are read as `i16` before widening: 0xFFC0 is −64, not
                    // 65472, which is what MAME's `int` scrolls hold. On screen the
                    // two readings are indistinguishable — they differ by 65536,
                    // a whole multiple of every layer's map span — so this cannot
                    // be killed by a test; see
                    // `tests::the_unsigned_scroll_reading_is_an_equivalent_mutant`,
                    // which fails if that ever stops being true.
                    let rows = scroll_rows(
                        gfxram,
                        cps_a,
                        layer,
                        i32::from(cps_a[sx] as i16),
                        i32::from(cps_a[sy] as i16),
                        videocontrol,
                    );
                    draw_tilemap(
                        &mut self.fb.pens,
                        &mut self.fb.prio,
                        gfxram,
                        &self.gfx,
                        &self.mapper,
                        cps_a_base(cps_a, base, SCROLL_BOUNDARY),
                        layer,
                        &rows,
                        &hi_pens,
                    );
                }
            }
        }

        // `flip_screen_set(BIT(videocontrol, 15))` (`cps1_v.cpp:3044`).
        if videocontrol & 0x8000 != 0 {
            self.fb.flip();
        }
    }

    /// The four priority masks, one per tile group.
    ///
    /// A board without a register for a group occludes nothing there
    /// (`cps1_v.cpp:2523-2527`, "completely transparent if priority masks not
    /// defined"). Task 8 gives these their meaning; the inversion MAME applies is
    /// its own, and is documented there.
    fn hi_pens(&self, cps_b: &[u16]) -> [u16; DEPTHS] {
        let mut out = [0u16; DEPTHS];
        for (slot, reg) in out.iter_mut().zip(self.cfg.priority) {
            *slot = reg.map_or(0, |r| cps_b[r]);
        }
        out
    }

    /// The palette as built for the last rendered frame.
    pub fn palette(&self) -> &[u16; palette::PENS] {
        &self.pal
    }

    /// The frame as 8-bit RGB triples, `WIDTH * HEIGHT * 3` bytes.
    pub fn rgb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(WIDTH * HEIGHT * 3);
        for &pen in self.fb.pens.iter() {
            out.extend_from_slice(&entry_to_rgb(self.pal[usize::from(pen)]));
        }
        out
    }
}

/// Whether a scroll layer draws this frame.
///
/// Two conditions, both from `cps1_v.cpp:2331-2333`: the layer's bit in the
/// layer-control register, and — for scrolls 2 and 3 only — a bit of
/// `videocontrol`, bit 2 for scroll 2 and bit 3 for scroll 3. Scroll 1 has no
/// second condition.
fn layer_enabled(cfg: &VideoConfig, layer: Layer, layercontrol: u16, videocontrol: u16) -> bool {
    let (mask_index, extra) = match layer {
        Layer::Scroll1 => (0, None),
        Layer::Scroll2 => (1, Some(2)),
        Layer::Scroll3 => (2, Some(3)),
    };
    if layercontrol & cfg.layer_enable_mask[mask_index] == 0 {
        return false;
    }
    extra.is_none_or(|bit| videocontrol & (1 << bit) != 0)
}

/// A layer's scroll, row scroll included where the board asks for it.
///
/// Row scroll is scroll 2's alone, and only when `videocontrol` bit 0 is set —
/// `if (BIT(videocontrol, 0)) // linescroll enable` (`cps1_v.cpp:3018`). With it
/// clear MAME calls `set_scroll_rows(1)` and one flat `set_scrollx` (`:3030-3032`).
///
/// The plan for this task did not name the selector; MAME does, and the design
/// document lists videocontrol bit 0 as row scroll. Without a test on it, a
/// correctly computed row-scroll table that was never selected would look exactly
/// like a working one.
fn scroll_rows(
    gfxram: &[u16],
    cps_a: &[u16],
    layer: Layer,
    scroll_x: i32,
    scroll_y: i32,
    videocontrol: u16,
) -> ScrollRows {
    if layer == Layer::Scroll2 && videocontrol & 0x0001 != 0 {
        ScrollRows::row_scrolled(gfxram, cps_a, scroll_x, scroll_y)
    } else {
        ScrollRows::flat(scroll_x, scroll_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::{BankRange, GfxType};
    use crate::layers::PEN_GRANULARITY;
    use crate::regs::{OBJ_BOUNDARY, PALETTE_BASE, PALETTE_BOUNDARY, ROWSCROLL_OFFS};
    use crate::tiles::{TileKind, TRANSPARENT_PEN};
    use crate::{VISIBLE_X, VISIBLE_Y};

    /// gfxram in words — 192 KB (`cps1.cpp:592`).
    const GFXRAM_WORDS: usize = 0x1_8000;

    /// The fixture's object-table base register, and the word it resolves to.
    ///
    /// 0x80 * 256 = byte 0x8000, already aligned to [`OBJ_BOUNDARY`], so word
    /// 0x4000. Pinned by [`the_fixture_bases_do_not_overlap`].
    const OBJ_BASE_REG: u16 = 0x80;
    const OBJ_WORD: usize = 0x4000;

    /// The fixture's palette base register, and the word it resolves to.
    ///
    /// 0xC0 * 256 = byte 0xC000, aligned to a palette page, so word 0x6000.
    const PALETTE_BASE_REG: u16 = 0xC0;
    const PALETTE_WORD: usize = 0x6000;

    /// The fixture's row-scroll base register, and the word it resolves to.
    ///
    /// 0x40 * 256 = byte 0x4000, so word 0x2000.
    const OTHER_BASE_REG: u16 = 0x40;
    const OTHER_WORD: usize = 0x2000;

    /// The four gfxram regions the fixture uses are disjoint.
    ///
    /// Not decoration: with every base left at zero the object table lands on top
    /// of the tilemaps, so writing a sprite record rewrites map tiles 0-3 instead
    /// of adding a sprite — which is a sprite test that silently tests a tilemap.
    /// The sizes are the hardware's: a tilemap is 0x1000 words (64x64 entries),
    /// the object table 0x400 (`m_obj_size` 0x800 bytes), the row-scroll table
    /// 0x400 (`m_other_size` 0x800 bytes), and the palette 0xC00 (six 0x200-word
    /// pages).
    #[test]
    fn the_fixture_bases_do_not_overlap() {
        let mut cps_a = [0u16; 0x20];
        cps_a[crate::regs::OBJ_BASE] = OBJ_BASE_REG;
        cps_a[PALETTE_BASE] = PALETTE_BASE_REG;
        cps_a[crate::regs::OTHER_BASE] = OTHER_BASE_REG;

        assert_eq!(
            cps_a_base(&cps_a, crate::regs::OBJ_BASE, OBJ_BOUNDARY),
            OBJ_WORD
        );
        assert_eq!(
            cps_a_base(&cps_a, PALETTE_BASE, PALETTE_BOUNDARY),
            PALETTE_WORD
        );
        assert_eq!(
            cps_a_base(&cps_a, crate::regs::OTHER_BASE, OBJ_BOUNDARY),
            OTHER_WORD
        );

        // A tilemap base of 0 is what `Board::scroll` leaves in place.
        let regions = [
            ("tilemap", 0usize, 0x1000usize),
            ("row scroll", OTHER_WORD, 0x400),
            ("object table", OBJ_WORD, 0x400),
            ("palette", PALETTE_WORD, 0xC00),
        ];
        for (i, &(a_name, a_start, a_len)) in regions.iter().enumerate() {
            assert!(a_start + a_len <= GFXRAM_WORDS, "{a_name} runs off gfxram");
            for &(b_name, b_start, b_len) in &regions[i + 1..] {
                assert!(
                    a_start + a_len <= b_start || b_start + b_len <= a_start,
                    "{a_name} overlaps {b_name}"
                );
            }
        }
    }

    /// The layer order is four 2-bit fields of the layer-control register.
    ///
    /// `cps1_v.cpp:2974-2977`. Two values: SF2's own, and one whose four fields
    /// are all different — the first repeats a 1, so on its own it cannot catch a
    /// field read at the wrong shift.
    #[test]
    fn the_layer_order_is_four_two_bit_fields() {
        // 0x1B40 = 0001 1011 0100 0000: bits 6-7 = 1, 8-9 = 3, 10-11 = 2,
        // 12-13 = 1.
        assert_eq!(layer_order(0x1B40), [1, 3, 2, 1]);
        // 0x3900 = 0011 1001 0000 0000: bits 6-7 = 0, 8-9 = 1, 10-11 = 2,
        // 12-13 = 3 — every field distinct, sprites at the back.
        assert_eq!(layer_order(0x3900), [0, 1, 2, 3]);
        // Bits 0-5 and 14-15 are not part of any field.
        assert_eq!(
            layer_order(0x003F),
            [0, 0, 0, 0],
            "low bits are not a field"
        );
        assert_eq!(layer_order(0xC000), [0, 0, 0, 0], "nor the top two");
    }

    /// An empty board renders the background pen everywhere.
    ///
    /// The path every `machine` test takes before a game has written anything:
    /// zeroed registers disable every layer, so the fill is all that remains.
    /// `cps1_v.cpp:3050-3052`.
    #[test]
    fn an_empty_board_renders_the_background_pen_everywhere() {
        let mut v = Video::new(VideoConfig::sf2(), fixture_mapper(), Vec::new());
        let gfxram = vec![0u16; GFXRAM_WORDS];
        v.render(&gfxram, &[0u16; 0x20], &[0u16; 0x20]);
        assert_eq!(BACKGROUND_PEN, 0xBFF, "cps1_v.cpp:3051");
        assert!(
            v.fb.pens.iter().all(|&p| p == BACKGROUND_PEN),
            "every pixel is the background pen"
        );
        assert!(v.fb.prio.iter().all(|&p| p == 0), "and nothing occludes");
        assert_eq!(v.fb.pens.len(), WIDTH * HEIGHT);
    }

    /// A layer whose enable bit is clear draws nothing, however full its map.
    ///
    /// `cps1_v.cpp:2331-2333`. Each layer is tested through its own mask, so a
    /// hardcoded index cannot pass.
    #[test]
    fn a_layer_absent_from_the_layer_control_is_not_drawn() {
        let cfg = VideoConfig::sf2();
        assert_eq!(cfg.layer_enable_mask, [0x08, 0x10, 0x20], "cps1_v.cpp:491");

        for (i, layer) in [Layer::Scroll1, Layer::Scroll2, Layer::Scroll3]
            .into_iter()
            .enumerate()
        {
            let f = Board::solid(layer);

            // Enabled: the layer covers the screen.
            let r = f.render(f.enable_only(), 0x000C);
            assert!(
                r.iter().all(|&p| p != BACKGROUND_PEN),
                "{layer:?} enabled covers the background"
            );

            // Its own bit cleared, every other bit set: nothing of it draws.
            let lc = !cfg.layer_enable_mask[i];
            let r = f.render(lc, 0x000C);
            assert!(
                r.iter().all(|&p| p == BACKGROUND_PEN),
                "{layer:?} disabled draws nothing"
            );
        }
    }

    /// Videocontrol bits 2 and 3 gate scrolls 2 and 3, and scroll 1 has no such
    /// gate.
    ///
    /// `cps1_v.cpp:2332-2333`. The two bits are checked separately and each
    /// against the *other* layer as well, so a swap fails.
    #[test]
    fn videocontrol_bits_two_and_three_gate_scroll_two_and_three() {
        // Scroll 2 draws with bit 2 set and not with it clear; bit 3 is not its
        // gate.
        let f = Board::solid(Layer::Scroll2);
        let lc = f.enable_only();
        assert!(f.drew(lc, 0x0004), "scroll 2 with videocontrol bit 2");
        assert!(!f.drew(lc, 0x0000), "scroll 2 without bit 2");
        assert!(!f.drew(lc, 0x0008), "bit 3 is not scroll 2's gate");

        // And the mirror image for scroll 3.
        let f = Board::solid(Layer::Scroll3);
        let lc = f.enable_only();
        assert!(f.drew(lc, 0x0008), "scroll 3 with videocontrol bit 3");
        assert!(!f.drew(lc, 0x0000), "scroll 3 without bit 3");
        assert!(!f.drew(lc, 0x0004), "bit 2 is not scroll 3's gate");

        // Scroll 1 has no videocontrol gate at all: it draws with both clear.
        let f = Board::solid(Layer::Scroll1);
        let lc = f.enable_only();
        assert!(f.drew(lc, 0x0000), "scroll 1 needs no videocontrol bit");
    }

    /// Videocontrol bit 0 selects row scroll, for scroll 2 only.
    ///
    /// `cps1_v.cpp:3018`: `if (BIT(videocontrol, 0)) // linescroll enable`. The
    /// plan for this task named the row-scroll *table* but not its selector, and
    /// a row-scroll path that is computed correctly and never selected fails no
    /// other test here.
    #[test]
    fn videocontrol_bit_zero_selects_row_scroll_for_scroll_two() {
        // A row-scroll table that moves visible row 0 far to the right and leaves
        // every other row alone. Entries are indexed by raster row.
        let mut f = Board::corner(Layer::Scroll2);
        f.gfxram[OTHER_WORD + VISIBLE_Y as usize] = (-100i32) as u16;
        f.cps_a[ROWSCROLL_OFFS] = 0;

        let lc = f.enable_only();

        // Bit 0 clear: the flat scroll applies to every row, so the corner pixel
        // stays at visible (0, 0).
        let r = f.render(lc, 0x0004);
        assert_eq!(r[0], corner_pen(Layer::Scroll2), "flat: corner at (0,0)");

        // Bit 0 set: row 0 alone is scrolled, so its pixel moves to x = 100 and
        // the rows below are untouched.
        let r = f.render(lc, 0x0005);
        assert_ne!(
            r[0],
            corner_pen(Layer::Scroll2),
            "row-scrolled: (0,0) empty"
        );
        assert_eq!(r[100], corner_pen(Layer::Scroll2), "moved to x = 100");
    }

    /// Row scroll is scroll 2's alone: scrolls 1 and 3 ignore bit 0.
    #[test]
    fn row_scroll_does_not_apply_to_scrolls_one_and_three() {
        for layer in [Layer::Scroll1, Layer::Scroll3] {
            let mut f = Board::corner(layer);
            f.gfxram[OTHER_WORD + VISIBLE_Y as usize] = (-100i32) as u16;
            let lc = f.enable_only();
            let vc = 0x000D; // bit 0 set, and both layer gates
            let r = f.render(lc, vc);
            assert_eq!(
                r[0],
                corner_pen(layer),
                "{layer:?} has no row scroll, so its corner stays at (0,0)"
            );
        }
    }

    /// Screen flip mirrors the finished frame.
    ///
    /// `VIDEOCONTROL` bit 15 (`cps1_v.cpp:3044`). An asymmetric pixel is pinned as
    /// well as a corner one, because a corner-only assertion passes under a
    /// transpose as well as under a mirror.
    #[test]
    fn screen_flip_mirrors_the_finished_frame() {
        let f = Board::corner(Layer::Scroll1);
        let lc = f.enable_only();
        let want = corner_pen(Layer::Scroll1);

        // Unflipped, the corner tile's one pixel is at (0, 0).
        let r = f.render(lc, 0x0000);
        assert_eq!(r[0], want);

        // Flipped, it is at the opposite corner.
        let r = f.render(lc, 0x8000);
        assert_eq!(at(&r, WIDTH - 1, HEIGHT - 1), want, "(0,0) -> (383,223)");
        assert_ne!(r[0], want, "and is no longer at the origin");

        // An asymmetric position, which a one-axis mirror would get wrong: (5, 7)
        // must land at (383-5, 223-7) = (378, 216).
        let mut f = Board::corner(Layer::Scroll1);
        f.scroll(5, 7);
        let r = f.render(lc, 0x0000);
        assert_eq!(at(&r, 5, 7), want, "the premise: unflipped at (5,7)");
        let r = f.render(lc, 0x8000);
        assert_eq!(at(&r, WIDTH - 1 - 5, HEIGHT - 1 - 7), want, "-> (378,216)");
        // Mirroring one axis alone would leave it at (378, 7) or (5, 216). The
        // fixture's corners repeat every 8 pixels, so this needs the arithmetic:
        // unflipped they sit at x = 5, y = 7 mod 8, flipped at x = 2, y = 0, and
        // neither half-mirrored position is on either grid.
        assert_ne!(at(&r, WIDTH - 1 - 5, 7), want, "not an x-only mirror");
        assert_ne!(at(&r, 5, HEIGHT - 1 - 7), want, "not a y-only mirror");
    }

    /// A full-screen opaque layer hides the background entirely.
    ///
    /// The structural invariant: a tilemap pen tops out at 0x7FF and
    /// [`BACKGROUND_PEN`] is 0xBFF, so "no pixel is still the background" is a
    /// real statement about coverage rather than a coincidence of pen values.
    #[test]
    fn a_full_screen_opaque_layer_hides_the_background() {
        for layer in [Layer::Scroll1, Layer::Scroll2, Layer::Scroll3] {
            let f = Board::solid(layer);
            let r = f.render(f.enable_only(), 0x000C);
            assert!(
                r.iter().all(|&p| p != BACKGROUND_PEN),
                "{layer:?} left a background pixel"
            );
        }
    }

    /// The layer control decides whether sprites are under or over a tilemap.
    ///
    /// Field order is depth order, `l0` at the back (`cps1_v.cpp:2981-2997`).
    /// Sprites are layer value 0, so putting 0 in field 0 versus field 3 swaps
    /// which of the two wins.
    #[test]
    fn sprites_draw_at_the_depth_the_layer_control_puts_them() {
        let mut f = Board::solid(Layer::Scroll2);
        f.put_sprite(0, 0);
        let tile = solid_pen(Layer::Scroll2);
        let sprite = SPRITE_COLOUR * PEN_GRANULARITY + u16::from(SOLID_PEN);
        assert_ne!(tile, sprite, "the two are distinguishable");

        // Sprites at the back, scroll 2 in front of them: the tile wins. The two
        // trailing fields repeat scroll 2 rather than defaulting to 0, which would
        // put the sprites back in front and make the assertion vacuous.
        let lc = f.enable_mask() | depths([0, 2, 2, 2]);
        assert_eq!(layer_order(lc), [0, 2, 2, 2], "sprites behind scroll 2");
        assert_eq!(f.render(lc, 0x000C)[0], tile, "the tile is in front");

        // Scroll 2 at the back, sprites in front of it: the sprite wins.
        let lc = f.enable_mask() | depths([2, 0, 0, 0]);
        assert_eq!(layer_order(lc), [2, 0, 0, 0], "sprites in front");
        assert_eq!(f.render(lc, 0x000C)[0], sprite, "the sprite is in front");
    }

    /// `rgb` is three bytes per pixel, converted through the palette.
    ///
    /// The expected triple is a literal, not `entry_to_rgb` called on both sides:
    /// entry 0x8777 is brightness 8, so `bright = 0x0f + (8 << 1) = 0x1f` and each
    /// channel is `7 * 0x11 * 0x1f / 0x2d = 81` — truncating, so 81 and not 82.
    #[test]
    fn rgb_is_three_bytes_per_pixel_from_the_palette() {
        let mut f = Board::corner(Layer::Scroll1);
        // Put a known entry at the pen the corner tile draws. The palette starts
        // at OBJ_WORD's neighbour PALETTE_WORD, so pen `p` is word
        // `PALETTE_WORD + p`.
        let pen = corner_pen(Layer::Scroll1);
        f.gfxram[PALETTE_WORD + usize::from(pen)] = 0x8777;
        // And a distinct entry at the background pen, so the two are separable.
        f.gfxram[PALETTE_WORD + usize::from(BACKGROUND_PEN)] = 0xF00F;

        let mut v = f.video();
        f.cps_b[VideoConfig::sf2().palette_control] = 0x3F; // all six pages
        let lc = f.enable_only();
        f.cps_b[VideoConfig::sf2().layer_control] = lc;
        v.render(&f.gfxram, &f.cps_a, &f.cps_b);

        assert_eq!(v.palette()[usize::from(pen)], 0x8777, "the entry was built");
        let rgb = v.rgb();
        assert_eq!(rgb.len(), WIDTH * HEIGHT * 3, "three bytes per pixel");
        assert_eq!(&rgb[0..3], &[81, 81, 81], "0x8777 -> 81 per channel");

        // Pixel (1, 0) is background: entry 0xF00F is brightness 15, so
        // `bright = 0x0f + (15 << 1) = 0x2d` and blue is `15 * 0x11 / 1 = 255`.
        assert_eq!(
            &rgb[3..6],
            &[0, 0, 255],
            "0xF00F -> pure blue at full bright"
        );
    }

    /// The object latch is a frame behind, and `render` uses the latched table.
    ///
    /// [`Video::latch_objects`] is the only way in, so a `render` reading live
    /// objram would show a sprite that was never latched.
    #[test]
    fn render_draws_the_latched_object_table_and_not_live_objram() {
        let mut f = Board::solid(Layer::Scroll2);
        // Scroll 2 at the back, the sprites in front of it.
        let lc = f.enable_mask() | depths([2, 0, 0, 0]);
        let sprite = SPRITE_COLOUR * PEN_GRANULARITY + u16::from(SOLID_PEN);

        // A sprite written into gfxram but never latched does not appear.
        f.put_sprite(0, 0);
        let mut v = f.video();
        f.cps_b[VideoConfig::sf2().layer_control] = lc;
        v.render(&f.gfxram, &f.cps_a, &f.cps_b);
        assert_ne!(v.fb.pens[0], sprite, "never latched, so never drawn");

        // Latching it makes the next frame show it.
        v.latch_objects(&f.gfxram, &f.cps_a);
        v.render(&f.gfxram, &f.cps_a, &f.cps_b);
        assert_eq!(v.fb.pens[0], sprite, "latched, so drawn");
    }

    /// A negative scroll register moves the layer the short way, not 65 thousand
    /// pixels the long way.
    ///
    /// MAME keeps the scrolls in `int` and games write negative values — 0xFFC0 is
    /// −64. This pins the visible behaviour, which is all a test can reach: the
    /// signed and unsigned readings are **provably indistinguishable** on screen,
    /// and [`the_unsigned_scroll_reading_is_an_equivalent_mutant`] is the proof
    /// rather than a hope. So `as i16` cannot be killed by mutation, and this test
    /// does not claim to; it exists to pin the direction and distance a negative
    /// register moves a layer.
    #[test]
    fn a_negative_scroll_register_moves_the_layer_right() {
        let mut f = Board::corner(Layer::Scroll1);
        let want = corner_pen(Layer::Scroll1);
        let lc = f.enable_only();

        // Scroll −8 moves the layer 8 pixels right, so a corner lands at (8, 0).
        // The fixture's corners repeat every 8 pixels, so this alone would also
        // hold at a scroll of 0; the offset is pinned by moving a non-multiple.
        f.cps_a[SCROLL1_X] = (-8i32 - VISIBLE_X) as u16;
        f.cps_a[SCROLL1_Y] = (-VISIBLE_Y) as u16;
        let r = f.render(lc, 0x0000);
        assert_eq!(at(&r, 8, 0), want, "-8 moves the layer right by 8");

        // −3 is not a multiple of the 8-pixel tile, so the corner column moves off
        // 0 and onto 3.
        f.cps_a[SCROLL1_X] = (-3i32 - VISIBLE_X) as u16;
        let r = f.render(lc, 0x0000);
        assert_eq!(at(&r, 3, 0), want, "-3 moves the layer right by 3");
        assert_ne!(at(&r, 0, 0), want, "and off the origin");
    }

    /// The pens are cleared each frame, so last frame's picture does not persist.
    ///
    /// A fresh [`Framebuffer`] is already background-filled, so a first frame looks
    /// right whether or not `render` clears. Only a second frame that draws *less*
    /// than the first shows the difference — a game blanking a layer between
    /// rounds would otherwise leave the old layer on screen forever.
    #[test]
    fn the_pens_are_cleared_each_frame() {
        let mut f = Board::solid(Layer::Scroll2);
        let cfg = VideoConfig::sf2();
        let mut v = f.video();

        // Frame 1: the layer covers the screen.
        f.cps_b[cfg.layer_control] = f.enable_only();
        f.cps_a[VIDEOCONTROL] = 0x0004;
        v.render(&f.gfxram, &f.cps_a, &f.cps_b);
        assert!(
            v.fb.pens.iter().all(|&p| p != BACKGROUND_PEN),
            "the premise: frame 1 covers every pixel"
        );

        // Frame 2, same `Video`: the layer is disabled, so every pixel must be
        // background again.
        f.cps_b[cfg.layer_control] = 0;
        v.render(&f.gfxram, &f.cps_a, &f.cps_b);
        assert!(
            v.fb.pens.iter().all(|&p| p == BACKGROUND_PEN),
            "frame 2 draws nothing, so nothing of frame 1 remains"
        );
    }

    /// The priority buffer is cleared each frame and mirrored with the pens.
    ///
    /// `prio` is not decoration: [`draw_sprites`] drops a sprite pixel wherever it
    /// is set, so a buffer that is never cleared, or that is left unmirrored under
    /// a flip, silently erases sprites at stale positions. It is the composition's
    /// output as much as the pens are, and nothing else in this module reads it.
    #[test]
    fn the_priority_buffer_is_cleared_each_frame_and_flipped_with_the_pens() {
        // Group 0's priority register with only the corner pen's bit set: the
        // corner tile marks its own pixel and nothing else does. Scroll 2, whose
        // tile is a single 16x16 kind — scroll 1 alternates two 8x8 layouts within
        // one frame and the fixture's corner is only in the even one, which would
        // make the count below a puzzle rather than a statement.
        let mut f = Board::corner(Layer::Scroll2);
        let cfg = VideoConfig::sf2();
        let reg = cfg.priority[0].expect("sf2 has a group-0 priority register");
        let want = corner_pen(Layer::Scroll2);
        f.cps_b[reg] = 1u16 << CORNER_PEN;
        f.cps_b[cfg.layer_control] = f.enable_only();
        f.cps_a[VIDEOCONTROL] = 0x0004; // scroll 2's gate, no flip

        let mut v = f.video();
        v.render(&f.gfxram, &f.cps_a, &f.cps_b);
        assert_eq!(v.fb.prio[0], 1, "the corner pixel occludes sprites");
        // One corner per 16x16 cell across the visible frame: 24 by 14.
        let marked = v.fb.prio.iter().filter(|&&p| p == 1).count();
        assert_eq!(marked, 24 * 14, "and only the corners");
        assert_eq!(
            marked,
            (WIDTH / 16) * (HEIGHT / 16),
            "which is one per cell"
        );
        // Every marked pixel is a corner pixel and every corner pixel is marked:
        // `prio` describes the pens beside it, not some other set of pixels.
        let pen_corners = v.fb.pens.iter().filter(|&&p| p == want).count();
        assert_eq!(pen_corners, marked, "prio marks exactly the corner pixels");

        // Rendering again with the register cleared must leave nothing marked: a
        // missing `prio.fill(0)` would keep last frame's marks.
        f.cps_b[reg] = 0;
        v.render(&f.gfxram, &f.cps_a, &f.cps_b);
        assert!(
            v.fb.prio.iter().all(|&p| p == 0),
            "the buffer is cleared, not accumulated"
        );

        // Under a flip, `prio` moves with the pens it describes. The corner is at
        // visible (3, 5), an asymmetric position no grid coincidence covers.
        f.cps_b[reg] = 1u16 << CORNER_PEN;
        f.scroll(3, 5);
        f.cps_a[VIDEOCONTROL] = 0x8004;
        v.render(&f.gfxram, &f.cps_a, &f.cps_b);
        let i = (HEIGHT - 1 - 5) * WIDTH + (WIDTH - 1 - 3);
        assert_eq!(v.fb.prio[i], 1, "(3,5) -> (380,218)");
        assert_eq!(v.fb.pens[i], want, "the same pixel the pens call a corner");
    }

    /// Reading a scroll register unsigned is an equivalent mutant, and here is why.
    ///
    /// The two readings differ by exactly 65536. Each layer's map is 64 tiles of
    /// `tile_edge` pixels — 512, 1024, 2048 — and 65536 is a whole multiple of
    /// every one of them, so `draw_tilemap`'s wrap maps both readings to the same
    /// map pixel on every row. No frame can tell them apart.
    ///
    /// This is stated as a test rather than a comment so that it fails if a future
    /// layer's map span stops dividing 65536 — at which point `as i16` becomes
    /// observable and needs a real test.
    #[test]
    fn the_unsigned_scroll_reading_is_an_equivalent_mutant() {
        for layer in [Layer::Scroll1, Layer::Scroll2, Layer::Scroll3] {
            let span = layer.tile_edge() * crate::layers::MAP_TILES;
            assert_eq!(
                65536 % span,
                0,
                "{layer:?} spans {span} px, which no longer divides 65536: the \
                 signed reading is now observable and needs its own test"
            );
        }
    }

    // ---------------------------------------------------------------- fixtures

    /// The pen the fixtures' solid tiles use.
    const SOLID_PEN: u8 = 0x0A;
    /// The pen of the corner tile's single opaque pixel.
    const CORNER_PEN: u8 = 0x05;
    /// The colour scheme every fixture tile uses, from its attribute's low bits.
    const TILE_COLOUR: u16 = 0;
    /// The colour scheme the fixture sprite uses.
    const SPRITE_COLOUR: u16 = 3;
    /// The code of the fixtures' drawing tile.
    const SOLID_CODE: u16 = 0;

    /// The pen a solid fixture tile of `layer` draws.
    fn solid_pen(layer: Layer) -> u16 {
        (layer.colour_base() + TILE_COLOUR) * PEN_GRANULARITY + u16::from(SOLID_PEN)
    }

    /// The pen a corner fixture tile of `layer` draws.
    fn corner_pen(layer: Layer) -> u16 {
        (layer.colour_base() + TILE_COLOUR) * PEN_GRANULARITY + u16::from(CORNER_PEN)
    }

    /// The pen at visible `(x, y)` of a rendered frame.
    fn at(pens: &[u16], x: usize, y: usize) -> u16 {
        pens[y * WIDTH + x]
    }

    /// A layer-control value placing `fields` at the four depths, back to front.
    ///
    /// The inverse of [`layer_order`], written independently of it: the shifts are
    /// literals here, so a test using both is not asserting a function against
    /// itself.
    fn depths(fields: [u16; DEPTHS]) -> u16 {
        (fields[0] << 6) | (fields[1] << 8) | (fields[2] << 10) | (fields[3] << 12)
    }

    static RANGES: [BankRange; 4] = [
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
        BankRange {
            kind: GfxType::Sprite,
            start: 0,
            end: 0xFFFF,
            bank: 0,
        },
    ];

    /// A mapper that is the identity on small codes, for every graphics type.
    ///
    /// The bank arithmetic is `bank.rs`'s subject, tested there against STF29's
    /// own literals; here it would only obscure the composition logic.
    fn fixture_mapper() -> BankMapper {
        BankMapper {
            bank_sizes: [0x1_0000, 0, 0, 0],
            ranges: &RANGES,
        }
    }

    /// A tile of `kind` every pixel of which is `pen`.
    ///
    /// Written from the plane byte structure, never from `tile_pen`'s within-byte
    /// arithmetic — the same helper, and the same reasoning, as in `layers.rs`.
    /// [`fixture_tiles_decode_as_intended`] pins it.
    fn solid_tile(kind: TileKind, pen: u8) -> Vec<u8> {
        let byte_for = |bit: u8| if pen & (1 << bit) != 0 { 0xFFu8 } else { 0x00 };
        let group = [byte_for(0), byte_for(1), byte_for(2), byte_for(3)];
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

    /// One code's graphics for `layer`, solid in `pen`.
    ///
    /// Scroll 1's code indexes a 64-byte frame holding two 8-pixel tiles, so both
    /// halves are filled.
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

    /// A tile whose only opaque pixel is its top-left one, pen [`CORNER_PEN`].
    fn corner_frame(layer: Layer) -> Vec<u8> {
        let mut rom = frame_bytes(layer, TRANSPARENT_PEN);
        // Row 0's first group is bytes 0-3, carrying pen bits 0-3 in memory order;
        // within each, the leftmost pixel is the most significant bit.
        for (bit, b) in rom[0..4].iter_mut().enumerate() {
            if CORNER_PEN & (1 << bit) == 0 {
                *b &= 0x7F;
            }
        }
        rom
    }

    /// The fixture tiles decode to the pens the tests expect.
    ///
    /// Without this, a test asserting "the layer covered the background" could
    /// pass on a fixture that drew a different pen than the one named.
    #[test]
    fn fixture_tiles_decode_as_intended() {
        for layer in [Layer::Scroll1, Layer::Scroll2, Layer::Scroll3] {
            let kind = layer.gfx_type().tile_kind();
            let solid = frame_bytes(layer, SOLID_PEN);
            let corner = corner_frame(layer);
            for y in 0..kind.size() {
                for x in 0..kind.size() {
                    assert_eq!(
                        crate::tiles::tile_pen(&solid, kind, 0, x, y),
                        SOLID_PEN,
                        "{layer:?} solid ({x},{y})"
                    );
                    let want = if (x, y) == (0, 0) {
                        CORNER_PEN
                    } else {
                        TRANSPARENT_PEN
                    };
                    assert_eq!(
                        crate::tiles::tile_pen(&corner, kind, 0, x, y),
                        want,
                        "{layer:?} corner ({x},{y})"
                    );
                }
            }
        }
        assert_ne!(SOLID_PEN, TRANSPARENT_PEN);
        assert_ne!(CORNER_PEN, TRANSPARENT_PEN);
    }

    /// A scratch board: gfxram, both register files, and one layer's graphics.
    struct Board {
        gfxram: Vec<u16>,
        cps_a: [u16; 0x20],
        cps_b: [u16; 0x20],
        gfx: Vec<u8>,
        layer: Layer,
    }

    impl Board {
        /// Every tile of `layer`'s map drawing a tile from `gfx`, scrolled so map
        /// (0, 0) is at the visible origin. `layer` is remembered, so the helpers
        /// below cannot be asked about a different one than the map holds.
        fn with_gfx(layer: Layer, gfx: Vec<u8>) -> Self {
            let mut b = Self {
                // A zeroed map already points every tile at code 0 with a zero
                // attribute, which is the fixture's drawing tile.
                gfxram: vec![0u16; GFXRAM_WORDS],
                cps_a: [0u16; 0x20],
                cps_b: [0u16; 0x20],
                gfx,
                layer,
            };
            // The object table needs a base of its own: left at zero it would sit
            // on top of the tilemaps, and writing a sprite record would silently
            // rewrite map tiles 0-3 rather than adding a sprite.
            // [`the_fixture_bases_do_not_overlap`] pins where each one lands.
            b.cps_a[crate::regs::OBJ_BASE] = OBJ_BASE_REG;
            b.cps_a[PALETTE_BASE] = PALETTE_BASE_REG;
            b.cps_a[crate::regs::OTHER_BASE] = OTHER_BASE_REG;
            b.scroll(0, 0);
            b
        }

        /// Every tile solid in [`SOLID_PEN`].
        fn solid(layer: Layer) -> Self {
            Self::with_gfx(layer, frame_bytes(layer, SOLID_PEN))
        }

        /// Every tile a corner tile — one opaque pixel at its top-left.
        fn corner(layer: Layer) -> Self {
            Self::with_gfx(layer, corner_frame(layer))
        }

        /// Places this board's layer's map pixel (0, 0) at visible (`x`, `y`).
        fn scroll(&mut self, x: i32, y: i32) {
            let (sx, sy) = match self.layer {
                Layer::Scroll1 => (SCROLL1_X, SCROLL1_Y),
                Layer::Scroll2 => (SCROLL2_X, SCROLL2_Y),
                Layer::Scroll3 => (SCROLL3_X, SCROLL3_Y),
            };
            self.cps_a[sx] = (-x - VISIBLE_X) as u16;
            self.cps_a[sy] = (-y - VISIBLE_Y) as u16;
        }

        /// The layer-enable bit for this board's layer, with no depth fields set.
        ///
        /// Field 0 is then the sprites, which draw nothing unless a test latches
        /// an object table — but so are fields 1, 2 and 3, so a test that puts a
        /// sprite on screen must fill the fields it is not using itself.
        fn enable_mask(&self) -> u16 {
            let i = match self.layer {
                Layer::Scroll1 => 0,
                Layer::Scroll2 => 1,
                Layer::Scroll3 => 2,
            };
            VideoConfig::sf2().layer_enable_mask[i]
        }

        /// The layer-control value enabling this board's layer and nothing else,
        /// with that layer at the back depth and the three remaining fields the
        /// sprites.
        ///
        /// With no object table latched the sprites draw nothing, so the layer
        /// under test is the only thing on screen.
        fn enable_only(&self) -> u16 {
            let depth = match self.layer {
                Layer::Scroll1 => 1u16,
                Layer::Scroll2 => 2,
                Layer::Scroll3 => 3,
            };
            self.enable_mask() | (depth << 6)
        }

        /// Writes a sprite record into gfxram at the object base, in **visible**
        /// coordinates.
        fn write_obj(&mut self, x: i32, y: i32) {
            let base = cps_a_base(&self.cps_a, crate::regs::OBJ_BASE, 0x800);
            let rec = [
                (x + VISIBLE_X) as u16,
                (y + VISIBLE_Y) as u16,
                SOLID_CODE,
                SPRITE_COLOUR,
            ];
            for (i, w) in rec.into_iter().enumerate() {
                self.gfxram[base + i] = w;
            }
            // An end marker in record 1, so only record 0 draws.
            self.gfxram[base + 7] = 0xFF00;
        }

        /// Writes a sprite record and includes its graphics, so a sprite drawn at
        /// `rec` 0 is opaque.
        fn put_sprite(&mut self, x: i32, y: i32) {
            // Sprite code 0 must decode to a solid 16x16 tile. The layer graphics
            // already start at byte 0, so for the layers whose tile is not 16x16
            // this appends nothing usable — instead the sprite graphics replace
            // the slice, and the layer's tile is re-appended after it.
            let sprite = solid_tile(TileKind::Tile16x16, SOLID_PEN);
            if self.gfx.len() < sprite.len() {
                self.gfx.resize(sprite.len(), 0);
            }
            self.gfx[..sprite.len()].copy_from_slice(&sprite);
            self.write_obj(x, y);
        }

        /// A [`Video`] over this board's graphics.
        fn video(&self) -> Video {
            Video::new(VideoConfig::sf2(), fixture_mapper(), self.gfx.clone())
        }

        /// Renders with the given layer control and video control, returning the
        /// pens. The object table is latched first, so a board with a sprite
        /// record draws it.
        fn render(&self, layercontrol: u16, videocontrol: u16) -> Vec<u16> {
            let mut v = self.video();
            let mut cps_a = self.cps_a;
            let mut cps_b = self.cps_b;
            cps_a[VIDEOCONTROL] = videocontrol;
            cps_b[VideoConfig::sf2().layer_control] = layercontrol;
            // The palette is irrelevant to a pen assertion, but a zero page
            // enable would leave `pal` zeroed and `rgb` uninformative; the pen
            // buffer is what these tests read.
            v.latch_objects(&self.gfxram, &cps_a);
            v.render(&self.gfxram, &cps_a, &cps_b);
            v.fb.pens.to_vec()
        }

        /// Whether the layer under test drew anything at all.
        fn drew(&self, layercontrol: u16, videocontrol: u16) -> bool {
            self.render(layercontrol, videocontrol)
                .iter()
                .any(|&p| p != BACKGROUND_PEN)
        }
    }

    /// The board fixture sets every base register the tests depend on.
    ///
    /// A base left at zero would silently overlap the tilemaps; this pins that the
    /// values [`the_fixture_bases_do_not_overlap`] checks are the ones in use.
    #[test]
    fn the_fixture_board_sets_every_base_it_relies_on() {
        let f = Board::solid(Layer::Scroll2);
        assert_eq!(f.cps_a[crate::regs::OBJ_BASE], OBJ_BASE_REG);
        assert_eq!(f.cps_a[PALETTE_BASE], PALETTE_BASE_REG);
        assert_eq!(f.cps_a[crate::regs::OTHER_BASE], OTHER_BASE_REG);
    }
}
