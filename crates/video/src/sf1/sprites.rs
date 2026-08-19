//! Sprites: 128 entries walked backwards, optionally as 2×2 quads.
//!
//! `sf.cpp:365-450`. Each entry is 0x20 words of which four are used:
//!
//! ```text
//! [0] code
//! [1] attr:  0x000f colour
//!            0x0100 flip x
//!            0x0200 flip y
//!            0x0400 large (a 2x2 quad of tiles)
//! [2] sy     raster y
//! [3] sx     raster x
//! ```
//!
//! # Why this is not [`crate::sprites`]
//!
//! CPS-1's object list is a latched copy with a terminator word, walked
//! **forwards**, with the tile size chosen per sprite by a CPS-B register. SF1's
//! is read live out of objectram, has no terminator, is walked **backwards**, and
//! its "size" bit selects a hardcoded quad of four 16×16 tiles rather than a
//! different `gfx_element`. And SF1 puts every sprite code through [`invert`]
//! first, which CPS-1 has no analogue of.
//!
//! # The walk direction is visible only where sprites overlap
//!
//! `for (offs = 0x1000 - 0x20; offs >= 0; offs -= 0x20)` draws the **last** entry
//! first, so low indices overwrite high ones. Reversing it reverses every sprite
//! overlap in the game — and a screenshot of one sprite looks identical either
//! way, so this is asserted directly.
//!
//! # Screen flip uses two pivots, and they are the driver's numbers
//!
//! `sx = 480 - sx; sy = 224 - sy;` for a large sprite and
//! `sx = 496 - sx; sy = 240 - sy;` for a small one, both also negating the
//! sprite's own flip bits. The 16-pixel difference in each axis is the large
//! sprite's extra extent — a quad is 32×32 and its top-left has to land 16 pixels
//! further up and left. These are written as literals rather than derived from
//! [`super::tilemap::X_EXTENT`]: the y pivots are 240 and 224 against a
//! `Y_EXTENT` of 256, so any derivation would need two different corrections and
//! would read as a rule where the hardware has a pair of constants.

use super::gfx::GfxSet;
use crate::{HEIGHT, VISIBLE_X, VISIBLE_Y, WIDTH};

/// The pen `transpen` skips (`sf.cpp:412` and friends, last argument).
pub const TRANSPARENT_PEN: u8 = 15;

/// Words per object entry — the walk's `0x20` stride.
pub const STRIDE: usize = 0x20;

/// Object entries: `0x1000 / 0x20`, the whole of objectram.
pub const ENTRIES: usize = 0x1000 / STRIDE;

/// Screen-flip pivot for a small sprite's x (`sf.cpp:436`).
pub const SMALL_PIVOT_X: i32 = 496;
/// Screen-flip pivot for a small sprite's y (`sf.cpp:437`).
pub const SMALL_PIVOT_Y: i32 = 240;
/// Screen-flip pivot for a large sprite's x (`sf.cpp:385`).
pub const LARGE_PIVOT_X: i32 = 480;
/// Screen-flip pivot for a large sprite's y (`sf.cpp:386`).
pub const LARGE_PIVOT_Y: i32 = 224;

/// The sprite ROM's code swizzle (`sf.cpp:359-363`).
///
/// ```c
/// static const int delta[4] = {0x00, 0x18, 0x18, 0x00};
/// return nb ^ delta[(nb >> 3) & 3];
/// ```
///
/// It is an involution, which is why MAME can do the quad's `+1`/`+16`/`+17`
/// arithmetic on the raw code and invert afterwards. Applying it first gives
/// different codes: with `c = 7`, `invert(c + 1)` is 0x10 and `invert(c) + 1` is 8.
#[must_use]
pub const fn invert(code: u32) -> u32 {
    const DELTA: [u32; 4] = [0x00, 0x18, 0x18, 0x00];
    code ^ DELTA[((code >> 3) & 3) as usize]
}

/// Blit one 16×16 element at a raster position, skipping [`TRANSPARENT_PEN`].
///
/// MAME's `transpen` clips **before** flipping (`drawgfx_core`,
/// `drawgfxt.ipp`), so the pixel a screen position shows does not depend on how
/// much of the sprite is off-screen. Sampling per destination pixel, as here, has
/// that property automatically.
///
/// Eight parameters, so `clippy::too_many_arguments` fires (its default threshold
/// is seven). The attribute rather than a parameter struct: every one of these is
/// a distinct hardware field, and a struct built to satisfy a lint would have to
/// be constructed twice in [`draw`] — once per size branch — for no gain.
/// [`crate::layers::draw_tilemap`] carries the same attribute for the same reason.
#[allow(clippy::too_many_arguments)]
fn blit(
    dst: &mut [u16],
    gfx: &GfxSet<'_>,
    code: u32,
    colour: u16,
    flipx: bool,
    flipy: bool,
    sx: i32,
    sy: i32,
) {
    let palette_base = gfx.palette_base(colour);
    let (w, h) = (gfx.layout.width as i32, gfx.layout.height as i32);
    for ty in 0..h {
        let screen_y = sy + ty - VISIBLE_Y;
        if screen_y < 0 || screen_y >= HEIGHT as i32 {
            continue;
        }
        let src_y = if flipy { h - 1 - ty } else { ty };
        for tx in 0..w {
            let screen_x = sx + tx - VISIBLE_X;
            if screen_x < 0 || screen_x >= WIDTH as i32 {
                continue;
            }
            let src_x = if flipx { w - 1 - tx } else { tx };
            let Some(pen) = gfx.pen(code, src_x as u32, src_y as u32) else {
                continue;
            };
            if pen == TRANSPARENT_PEN {
                continue;
            }
            dst[screen_y as usize * WIDTH + screen_x as usize] = palette_base + u16::from(pen);
        }
    }
}

/// Draw every sprite in `objectram` (`sf.cpp:365-450`).
///
/// `flip` is `flip_screen()`. A short `objectram` simply yields fewer sprites —
/// the region is the caller's and a truncated one must not panic.
///
/// # Panics
///
/// If `dst.len()` is not `WIDTH * HEIGHT`, which is a programming error in this
/// crate.
pub fn draw(dst: &mut [u16], gfx: &GfxSet<'_>, objectram: &[u16], flip: bool) {
    assert_eq!(
        dst.len(),
        WIDTH * HEIGHT,
        "destination must be WIDTH * HEIGHT"
    );
    // Backwards: low indices draw last and win. See the module documentation.
    for entry in (0..ENTRIES).rev() {
        let base = entry * STRIDE;
        let word = |o: usize| objectram.get(base + o).copied();
        let (Some(code), Some(attr), Some(sy), Some(sx)) = (word(0), word(1), word(2), word(3))
        else {
            continue;
        };
        let code = u32::from(code);
        let colour = attr & 0x000F;
        let mut flipx = attr & 0x0100 != 0;
        let mut flipy = attr & 0x0200 != 0;
        let (mut sx, mut sy) = (i32::from(sx), i32::from(sy));
        if attr & 0x0400 != 0 {
            // Large: a 2x2 quad of tiles.
            if flip {
                sx = LARGE_PIVOT_X - sx;
                sy = LARGE_PIVOT_Y - sy;
                flipx = !flipx;
                flipy = !flipy;
            }
            // `sf.cpp:391-394`. The `+16` is the sprite ROM's row stride, and
            // `invert` is applied after this arithmetic, not before.
            let (mut c1, mut c2, mut c3, mut c4) = (code, code + 1, code + 16, code + 17);
            if flipx {
                core::mem::swap(&mut c1, &mut c2);
                core::mem::swap(&mut c3, &mut c4);
            }
            if flipy {
                core::mem::swap(&mut c1, &mut c3);
                core::mem::swap(&mut c2, &mut c4);
            }
            for (c, dx, dy) in [(c1, 0, 0), (c2, 16, 0), (c3, 0, 16), (c4, 16, 16)] {
                blit(dst, gfx, invert(c), colour, flipx, flipy, sx + dx, sy + dy);
            }
        } else {
            if flip {
                sx = SMALL_PIVOT_X - sx;
                sy = SMALL_PIVOT_Y - sy;
                flipx = !flipx;
                flipy = !flipy;
            }
            blit(dst, gfx, invert(code), colour, flipx, flipy, sx, sy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sf1::gfx::SPRITE_LAYOUT;
    use crate::{HEIGHT, VISIBLE_X, VISIBLE_Y, WIDTH};

    /// The object table's shape, from the walk at `sf.cpp:369`.
    ///
    /// `for (offs = 0x1000 - 0x20; offs >= 0; offs -= 0x20)` indexes a `u16*`, so
    /// the table is 0x1000 **words** = 0x2000 bytes... except objectram is
    /// 0xffe000-0xffffff, which is 0x2000 bytes = 0x1000 words. They agree: the
    /// walk covers the whole region, 128 entries of 0x20 words each, of which the
    /// first four words are used.
    #[test]
    fn the_object_table_covers_the_whole_region_in_thirty_two_word_entries() {
        assert_eq!(ENTRIES * STRIDE, 0x1000, "the walk's full range, in words");
        assert_eq!(0x1000 * 2, 0x2000, "which is objectram's byte size");
        assert_eq!(ENTRIES, 128);
        assert_eq!(STRIDE, 0x20);
        assert_eq!(TRANSPARENT_PEN, 15);
    }

    /// `invert`, verbatim from `sf.cpp:359-363`.
    ///
    /// ```c
    /// static const int delta[4] = {0x00, 0x18, 0x18, 0x00};
    /// return nb ^ delta[(nb >> 3) & 3];
    /// ```
    ///
    /// It swizzles bits 3-4 of the code: the two middle values of a group of four
    /// get 0x18 xored in and the outer two are left alone. This is the sprite
    /// ROM's interleave, and it is an involution — `invert(invert(c)) == c` — which
    /// is the property that makes the quad arithmetic below safe to do *before*
    /// inverting, exactly as MAME does.
    #[test]
    fn invert_swizzles_bits_three_and_four_and_is_an_involution() {
        // (nb >> 3) & 3 == 0 -> delta 0x00
        assert_eq!(invert(0x00), 0x00);
        assert_eq!(invert(0x07), 0x07);
        // == 1 -> delta 0x18
        assert_eq!(invert(0x08), 0x08 ^ 0x18);
        assert_eq!(invert(0x08), 0x10);
        // == 2 -> delta 0x18
        assert_eq!(invert(0x10), 0x08);
        // == 3 -> delta 0x00
        assert_eq!(invert(0x18), 0x18);
        assert_eq!(invert(0x1F), 0x1F);
        // The pattern repeats every 0x20, because bit 5 and up are untouched.
        assert_eq!(invert(0x28), 0x30);
        assert_eq!(invert(0x1234), 0x1234 ^ 0x18, "(0x1234>>3)&3 == 2");
        for code in 0..0x400u32 {
            assert_eq!(invert(invert(code)), code, "involution at {code:#x}");
        }
    }

    /// A small sprite draws one 16×16 tile at its raw (sx, sy) in raster space.
    ///
    /// `sf.cpp:442-447` passes `sx, sy` straight to `transpen` with no offset, so
    /// the object coordinates are **raster** coordinates and the visible window's
    /// (64, 16) origin is subtracted here, not added there.
    #[test]
    fn a_small_sprite_draws_one_tile_at_a_raster_coordinate() {
        let rom = solid_rom();
        let gfx = solid_gfx(&rom);
        assert_eq!(gfx.elements(), 1);
        let mut ram = vec![0u16; ENTRIES * STRIDE];
        // Entry 0: code 0, attr 0 (colour 0, small, no flip), at raster (64, 16)
        // which is screen (0, 0).
        ram[0] = 0;
        ram[1] = 0;
        ram[2] = VISIBLE_Y as u16;
        ram[3] = VISIBLE_X as u16;
        let mut dst = vec![0u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &ram, false);
        assert_eq!(dst[0], 512, "colour_base 512 + pen 0");
        assert_eq!(dst[15 * WIDTH + 15], 512, "sixteen pixels square");
        assert_eq!(dst[16], 0, "and no wider");
        assert_eq!(dst[16 * WIDTH], 0, "nor taller");
    }

    /// Pen 15 is transparent — and it is the *pen*, not the palette entry.
    #[test]
    fn pen_fifteen_is_transparent() {
        let mut rom = vec![0u8; 128];
        rom[0] = 0x88; // planes 0,1 -> pen bits 3,2
        rom[64] = 0x88; // planes 2,3 -> pen bits 1,0 => pen 15 at x=0,y=0
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 512,
        };
        let mut ram = vec![0u16; ENTRIES * STRIDE];
        ram[2] = VISIBLE_Y as u16;
        ram[3] = VISIBLE_X as u16;
        let mut dst = vec![0xABCDu16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &ram, false);
        assert_eq!(dst[0], 0xABCD, "pen 15 left the pixel alone");
        assert_eq!(dst[1], 512, "pen 0 next to it drew");
    }

    /// The colour comes from `attr & 0x000f`, at granularity 16.
    #[test]
    fn the_colour_is_the_low_nibble_of_the_attribute() {
        let rom = solid_rom();
        let gfx = solid_gfx(&rom);
        let mut ram = vec![0u16; ENTRIES * STRIDE];
        ram[1] = 0x0003;
        ram[2] = VISIBLE_Y as u16;
        ram[3] = VISIBLE_X as u16;
        let mut dst = vec![0u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &ram, false);
        assert_eq!(dst[0], 512 + 16 * 3, "colour 3 at granularity 16");
    }

    /// The walk runs **backwards**, so entry 0 wins.
    ///
    /// `for (offs = 0x1000 - 0x20; offs >= 0; offs -= 0x20)` draws the last entry
    /// first and the first entry last, so **low indices overwrite high ones**.
    /// A forwards walk reverses every sprite overlap on screen — which shows up
    /// only where two sprites overlap, so a still frame of one sprite proves
    /// nothing. (This is also the opposite of CPS-1's `ObjLatch`, whose forwards
    /// walk `crate::sprites` documents; the boards genuinely differ.)
    #[test]
    fn the_walk_is_backwards_so_low_indices_draw_last() {
        let rom = solid_rom();
        let gfx = solid_gfx(&rom);
        let mut ram = vec![0u16; ENTRIES * STRIDE];
        // Entry 0 and entry 1 in the same place, different colours.
        for (entry, colour) in [(0usize, 1u16), (1, 2)] {
            let base = entry * STRIDE;
            ram[base] = 0;
            ram[base + 1] = colour;
            ram[base + 2] = VISIBLE_Y as u16;
            ram[base + 3] = VISIBLE_X as u16;
        }
        let mut dst = vec![0u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &ram, false);
        assert_eq!(dst[0], 512 + 16, "entry 0 drew last and won");
    }

    /// A large sprite is a 2×2 quad of consecutive-ish codes.
    ///
    /// `sf.cpp:391-394`: `c1 = c, c2 = c + 1, c3 = c + 16, c4 = c + 17`, placed at
    /// (sx, sy), (sx+16, sy), (sx, sy+16), (sx+16, sy+16). The `+16` is the sprite
    /// ROM's row stride, and each code is passed through `invert` **after** the
    /// arithmetic — so `invert(c + 1)`, not `invert(c) + 1`. Those differ: with
    /// c = 7, `invert(8)` is 0x10 while `invert(7) + 1` is 8.
    #[test]
    fn a_large_sprite_draws_a_quad_of_four_codes() {
        // 32 elements. Encode each element's low two code bits as the pen at its
        // (0,0), so one assertion per cell identifies which element landed there.
        let mut rom = vec![0u8; 128 * 32];
        // Plane 3 (bit offset `half`, pen bit 0) when e is odd; plane 2
        // (`half + 4`, pen bit 1) when e & 2. Both live in byte `half + e*64` —
        // plane 3 in its high nibble, plane 2 in its low.
        let half = rom.len() / 2;
        for e in 0..32usize {
            if e & 1 != 0 {
                rom[half + e * 64] |= 0x80; // plane 3, pixel x=0 -> pen bit 0
            }
            if e & 2 != 0 {
                rom[half + e * 64] |= 0x08; // plane 2, pixel x=0 -> pen bit 1
            }
        }
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 512,
        };
        assert_eq!(gfx.elements(), 32);
        let mut ram = vec![0u16; ENTRIES * STRIDE];
        ram[0] = 0; // c = 0
        ram[1] = 0x0400; // large, colour 0, no flip
        ram[2] = VISIBLE_Y as u16;
        ram[3] = VISIBLE_X as u16;
        let mut dst = vec![0u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &ram, false);
        // c1 = invert(0) = 0, pen 0; c2 = invert(1) = 1, pen 1;
        // c3 = invert(16) = 8, and 8 & 3 == 0, so pen 0;
        // c4 = invert(17) = 9, and 9 & 3 == 1, so pen 1.
        assert_eq!(dst[0], 512, "top-left is invert(0) = 0");
        assert_eq!(dst[16], 512 + 1, "top-right is invert(1) = 1");
        assert_eq!(dst[16 * WIDTH], 512, "bottom-left is invert(16) = 8");
        assert_eq!(
            dst[16 * WIDTH + 16],
            512 + 1,
            "bottom-right is invert(17) = 9"
        );
        // The quad is 32x32 and no larger.
        assert_eq!(dst[32], 0);
        assert_eq!(dst[32 * WIDTH], 0);
    }

    /// `invert` is applied after the quad arithmetic, not before.
    ///
    /// The distinguishing case: c = 7 gives c2 = 8, and `invert(8)` is 0x10 while
    /// `invert(7) + 1` is 8. So the top-right cell must show element 0x10.
    #[test]
    fn the_quad_arithmetic_happens_before_invert() {
        assert_eq!(invert(7), 7);
        assert_eq!(invert(8), 0x10);
        assert_ne!(invert(8), invert(7) + 1, "the case that tells them apart");
        // Element 0x10 gets pen 1 at its (0,0); every other element pen 0.
        let mut rom = vec![0u8; 128 * 32];
        let half = rom.len() / 2;
        rom[half + 0x10 * 64] = 0x80; // plane 3, pixel x=0 -> pen bit 0
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 512,
        };
        let mut ram = vec![0u16; ENTRIES * STRIDE];
        ram[0] = 7;
        ram[1] = 0x0400;
        ram[2] = VISIBLE_Y as u16;
        ram[3] = VISIBLE_X as u16;
        let mut dst = vec![0u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &ram, false);
        assert_eq!(dst[16], 512 + 1, "top-right is invert(7+1) = 0x10");
        assert_eq!(dst[0], 512, "top-left is invert(7) = 7");
    }

    /// Flipping a large sprite swaps the quad's cells as well as the pixels.
    ///
    /// `sf.cpp:396-405`: flipx swaps c1↔c2 and c3↔c4; flipy swaps c1↔c3 and
    /// c2↔c4. Without the swaps the quad's four tiles stay put while their
    /// contents mirror, which tears a flipped sprite into quarters.
    #[test]
    fn flipping_a_large_sprite_swaps_its_quad_cells() {
        let mut rom = vec![0u8; 128 * 32];
        let half = rom.len() / 2;
        // Give elements 0, 1, 8, 9 pens 0, 1, 2, 3 — the four the quad uses with
        // c = 0, after invert. A pen at *every* pixel, so mirroring within a tile
        // cannot move it and only the cell swap shows.
        for (e, planes) in [(0usize, 0u8), (1, 1), (8, 2), (9, 3)] {
            for byte in 0..64 {
                if planes & 1 != 0 {
                    // Plane 3 is bit offset `half + 0`, which is each byte's HIGH
                    // nibble. A whole 0xFF would set plane 2 as well and give
                    // pen 3 — the pens have to stay distinguishable.
                    rom[half + e * 64 + byte] |= 0xF0; // -> pen bit 0
                }
                if planes & 2 != 0 {
                    // Plane 2 is `half + 4`: the same four pixels, low nibble.
                    rom[half + e * 64 + byte] |= 0x0F; // -> pen bit 1
                }
            }
        }
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 0,
        };
        let quad = |attr: u16| {
            let mut ram = vec![0u16; ENTRIES * STRIDE];
            ram[0] = 0;
            ram[1] = attr;
            ram[2] = VISIBLE_Y as u16;
            ram[3] = VISIBLE_X as u16;
            let mut dst = vec![0xFFFFu16; WIDTH * HEIGHT];
            draw(&mut dst, &gfx, &ram, false);
            // The four cells' top-left pixels.
            [dst[0], dst[16], dst[16 * WIDTH], dst[16 * WIDTH + 16]]
        };
        // 0x400 large, no flip: cells are codes 0, 1, 8, 9 -> pens 0, 1, 2, 3.
        assert_eq!(quad(0x0400), [0, 1, 2, 3]);
        // flipx (0x100): c1<->c2 and c3<->c4.
        assert_eq!(quad(0x0500), [1, 0, 3, 2]);
        // flipy (0x200): c1<->c3 and c2<->c4.
        assert_eq!(quad(0x0600), [2, 3, 0, 1]);
        // both.
        assert_eq!(quad(0x0700), [3, 2, 1, 0]);
    }

    /// The screen-flip pivots are 480/224 for large sprites and 496/240 for small.
    ///
    /// `sf.cpp:385-388` and `:436-439`. They differ by 16 in **both** axes because
    /// the pivot has to account for the sprite's own extent: a large sprite is
    /// 32×32, so its top-left must land 16 pixels further up and left. Sharing one
    /// pivot offsets every large sprite by 16 pixels diagonally.
    ///
    /// Note also that these are **not** `X_EXTENT - 16` / `Y_EXTENT - 16`
    /// (496/240 and 480/224 respectively for x) applied uniformly: the x pivots are
    /// 496 and 480 while the y pivots are 240 and 224, and `Y_EXTENT` is 256. The
    /// numbers are the driver's, so this module writes them as literals rather
    /// than deriving them from the tilemap's extents and getting a plausible-looking
    /// off-by-16.
    #[test]
    fn the_two_screen_flip_pivots_differ_by_sixteen_in_both_axes() {
        assert_eq!((SMALL_PIVOT_X, SMALL_PIVOT_Y), (496, 240));
        assert_eq!((LARGE_PIVOT_X, LARGE_PIVOT_Y), (480, 224));
        assert_eq!(SMALL_PIVOT_X - LARGE_PIVOT_X, 16);
        assert_eq!(SMALL_PIVOT_Y - LARGE_PIVOT_Y, 16);
    }

    /// Screen flip moves a small sprite to the mirrored position and mirrors it.
    #[test]
    fn screen_flip_relocates_and_mirrors_a_small_sprite() {
        let mut rom = vec![0u8; 128];
        // Byte 0 bit 7 is bit offset 0, which is plane **1** (offset 0), and with
        // four planes plane 1 is pen bit 2 — so pen 4, not pen 8. Plane 0 lives at
        // offset 4, the same byte's low nibble.
        rom[0] = 0x80;
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 0,
        };
        let mut ram = vec![0u16; ENTRIES * STRIDE];
        ram[2] = VISIBLE_Y as u16; // 16
        ram[3] = VISIBLE_X as u16; // 64
        let mut dst = vec![0u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &ram, false);
        assert_eq!(dst[0], 4, "unflipped at screen (0,0)");
        dst.fill(0);
        draw(&mut dst, &gfx, &ram, true);
        // sx = 496 - 64 = 432, sy = 240 - 16 = 224, both raster; and flipx/flipy
        // both become set, so the pen moves to that tile's bottom-right at
        // raster (432+15, 224+15) = (447, 239) = screen (383, 223).
        assert_eq!(dst[HEIGHT * WIDTH - 1], 4, "flipped to the far corner");
        assert_eq!(dst[0], 0);
    }

    /// A sprite partly off-screen clips instead of wrapping or panicking.
    #[test]
    fn a_sprite_at_the_edge_clips() {
        let rom = solid_rom();
        let gfx = solid_gfx(&rom);
        let mut ram = vec![0u16; ENTRIES * STRIDE];
        // Raster (56, 8): eight pixels above and left of the window.
        ram[2] = 8;
        ram[3] = 56;
        let mut dst = vec![0u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &ram, false);
        assert_eq!(dst[0], 512, "the bottom-right eight-by-eight is visible");
        assert_eq!(dst[7 * WIDTH + 7], 512);
        assert_eq!(dst[8 * WIDTH], 0, "and it stops there");
        // Far off to the right and bottom: nothing drawn, nothing panics.
        ram[2] = 0xFFFF;
        ram[3] = 0xFFFF;
        dst.fill(0);
        draw(&mut dst, &gfx, &ram, false);
        assert!(dst.iter().all(|&p| p == 0));
    }

    /// A short or absent objectram draws nothing rather than panicking.
    #[test]
    fn a_short_objectram_is_tolerated() {
        let rom = solid_rom();
        let gfx = solid_gfx(&rom);
        let mut dst = vec![3u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &[], false);
        assert!(dst.iter().all(|&p| p == 3), "nothing drawn");
        // Four words is one entry's worth of fields but not one stride.
        let ram = vec![0u16, 0, VISIBLE_Y as u16, VISIBLE_X as u16];
        draw(&mut dst, &gfx, &ram, false);
        assert_eq!(dst[0], 512, "entry 0 still drew");
    }

    /// The buffer length is checked.
    #[test]
    #[should_panic(expected = "destination must be WIDTH * HEIGHT")]
    fn a_wrongly_sized_destination_is_a_programming_error() {
        let rom = solid_rom();
        let gfx = solid_gfx(&rom);
        let mut dst = vec![0u16; 10];
        draw(&mut dst, &gfx, &[], false);
    }

    /// One 16×16 element with every plane clear — pen 0 at all 256 pixels.
    ///
    /// Pen 0 is opaque (only pen 15 is transparent), so the block's extent is
    /// directly observable, which is what the geometric tests measure.
    fn solid_rom() -> Vec<u8> {
        vec![0u8; 128]
    }

    /// The sprite `GfxSet` over [`solid_rom`], at the object layer's colour base.
    fn solid_gfx(rom: &[u8]) -> GfxSet<'_> {
        GfxSet {
            rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 512,
        }
    }
}
