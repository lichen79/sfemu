//! Drawing tiles into a CPS-1 graphics ROM.
//!
//! The layout is `video::tiles`' rule, written from the other side: that module
//! reads a pen out of bytes, this one writes bytes so a pen reads back. The bit
//! index of pixel `(x, y)` in plane `p` of a tile whose storage frame is `FW`
//! pixels wide is
//!
//! ```text
//! y * (4 * FW)  +  32 * (x >> 3)  +  (x & 7)  +  [24, 16, 8, 0][p]
//! ```
//!
//! # Why this is not `video::tiles::tile_pen` run backwards
//!
//! It could be — invert the function, or search for the byte that decodes to the
//! pen you want. Both would be a decoder checking itself. A tile drawn by
//! independently transcribed arithmetic and then *read back through the
//! renderer* is the only version of this that can fail, and
//! `tests::a_written_pen_reads_back_through_the_decoders_own_rule` is where the
//! two meet.

/// A tile kind's storage frame width in pixels, and its size in bytes.
///
/// The 8×8 kinds share a 16-pixel frame and each claim the whole 64-byte block,
/// which is `video::tiles::TileKind::bytes`' rule: a code indexes frames, not
/// half-frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 8×8, left half of a 16-pixel frame. Scroll 1's even columns.
    Tile8x8,
    /// 8×8, right half of the same frame. Scroll 1's odd columns.
    Tile8x8Odd,
    /// 16×16. Scroll 2 and sprites.
    Tile16x16,
    /// 32×32. Scroll 3.
    Tile32x32,
}

impl Kind {
    /// The tile's edge in pixels.
    pub const fn size(self) -> u32 {
        match self {
            Self::Tile8x8 | Self::Tile8x8Odd => 8,
            Self::Tile16x16 => 16,
            Self::Tile32x32 => 32,
        }
    }

    /// Bytes one tile's frame occupies.
    pub const fn bytes(self) -> usize {
        match self {
            Self::Tile8x8 | Self::Tile8x8Odd => 64,
            Self::Tile16x16 => 128,
            Self::Tile32x32 => 512,
        }
    }

    /// The storage frame's width in pixels.
    const fn frame_width(self) -> u32 {
        match self {
            Self::Tile8x8 | Self::Tile8x8Odd | Self::Tile16x16 => 16,
            Self::Tile32x32 => 32,
        }
    }

    /// The bit offset this tile's x=0 sits at within its frame.
    const fn x_bias(self) -> u32 {
        match self {
            Self::Tile8x8Odd => 32,
            _ => 0,
        }
    }
}

/// Sets pixel `(x, y)` of tile `code` to `pen`, in a ROM addressed in 8×8 units.
///
/// `pen` is 4 bits; higher bits are ignored rather than masked into a neighbour,
/// because a caller passing 16 has an arithmetic bug and a silent `& 0x0F` would
/// draw pen 0 and look deliberate.
///
/// Out-of-range writes — a code past the end of `rom`, or `x`/`y` outside
/// `kind.size()` — are dropped. A generator that ran off the end of its own ROM
/// has a bug, and `tests::a_pixel_outside_the_rom_is_dropped_not_wrapped` pins
/// that it does not corrupt tile 0 instead.
pub fn put_pixel(rom: &mut [u8], kind: Kind, code: u32, x: u32, y: u32, pen: u8) {
    if x >= kind.size() || y >= kind.size() {
        return;
    }
    let start = (code as usize).saturating_mul(kind.bytes());
    let Some(tile) = rom.get_mut(start..start.saturating_add(kind.bytes())) else {
        return;
    };
    let base = y * 4 * kind.frame_width() + 32 * (x >> 3) + (x & 7) + kind.x_bias();
    for (p, off) in [24u32, 16, 8, 0].into_iter().enumerate() {
        let bit = base + off;
        let mask = 0x80u8 >> (bit % 8);
        let byte = &mut tile[(bit / 8) as usize];
        // Plane 0 carries the pen's bit 3 — `0x08 >> p`, the same table
        // `tile_pen` reads with.
        if pen & (0x08 >> p) != 0 {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
    }
}

/// Fills tile `code` with one pen.
pub fn solid(rom: &mut [u8], kind: Kind, code: u32, pen: u8) {
    for y in 0..kind.size() {
        for x in 0..kind.size() {
            put_pixel(rom, kind, code, x, y, pen);
        }
    }
}

/// Draws a hollow rectangle border one pixel wide, inside a tile filled with
/// `fill`.
///
/// The demo's background tiles: a flat colour is indistinguishable from a
/// renderer that ignores `x` and `y` entirely, and a border is the cheapest
/// pattern whose position on screen is checkable by eye.
pub fn framed(rom: &mut [u8], kind: Kind, code: u32, fill: u8, edge: u8) {
    let n = kind.size();
    for y in 0..n {
        for x in 0..n {
            let on_edge = x == 0 || y == 0 || x == n - 1 || y == n - 1;
            put_pixel(rom, kind, code, x, y, if on_edge { edge } else { fill });
        }
    }
}

/// Draws a filled circle centred in the tile, on a transparent background.
///
/// Pen 15 is transparent to every CPS-1 draw path
/// (`video::tiles::TRANSPARENT_PEN`), so a sprite drawn with this shows the
/// layers behind it at its corners — which is what makes sprite transparency
/// visible on screen rather than a claim in a test.
pub fn disc(rom: &mut [u8], kind: Kind, code: u32, pen: u8) {
    let n = kind.size() as i32;
    for y in 0..n {
        for x in 0..n {
            // Centres at `r - 0.5` in both axes, doubled to stay in integers.
            let dx = 2 * x - (n - 1);
            let dy = 2 * y - (n - 1);
            let inside = dx * dx + dy * dy <= (n - 1) * (n - 1);
            let p = if inside { pen } else { TRANSPARENT_PEN };
            put_pixel(rom, kind, code, x as u32, y as u32, p);
        }
    }
}

/// The pen every CPS-1 draw path treats as transparent (`cps1_v.cpp:2551`).
pub const TRANSPARENT_PEN: u8 = 0x0F;

/// Draws a digit 0-9 as a 5×7 figure in the top-left of an 8×8 tile.
///
/// The demo's text layer spells out a frame counter, which is the one thing on
/// screen that proves the 68000 is still executing rather than showing a frozen
/// first frame. A five-by-seven font is the smallest that stays legible at 8×8.
///
/// # Both halves of the frame
///
/// The glyph is written into [`Kind::Tile8x8`] *and* [`Kind::Tile8x8Odd`] — the
/// two halves of one 64-byte frame. Scroll 1 chooses between them by bit 5 of
/// the tile's scan index, which under its scan mapper is the column's low bit
/// (`video::layers::draw_tilemap`), so one code renders as the even half in one
/// column and the odd half in the next. A digit written to one half only would
/// be legible at even columns and blank at odd ones, and as the layer scrolls
/// each glyph would blink — a symptom that reads as a renderer fault.
pub fn digit(rom: &mut [u8], code: u32, value: u8, pen: u8) {
    // Row bitmaps, bit 4 leftmost. Written out rather than generated: a font is
    // data, and the only check that matters is whether it reads as a number.
    const GLYPHS: [[u8; 7]; 10] = [
        [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E], // 0, with a slash
        [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E], // 1
        [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F], // 2
        [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E], // 3
        [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02], // 4
        [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E], // 5
        [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E], // 6
        [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08], // 7
        [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E], // 8
        [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C], // 9
    ];
    let glyph = GLYPHS[usize::from(value % 10)];
    for y in 0..8u32 {
        for x in 0..8u32 {
            let on = y < 7 && x < 5 && glyph[y as usize] & (0x10 >> x) != 0;
            let p = if on { pen } else { TRANSPARENT_PEN };
            put_pixel(rom, Kind::Tile8x8, code, x, y, p);
            put_pixel(rom, Kind::Tile8x8Odd, code, x, y, p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reader this crate's tiles are ultimately read by, transcribed from
    /// `video::tiles::tile_pen`.
    ///
    /// A copy and not a dependency, deliberately: `video` reading a tile
    /// `testrom` wrote is the property the demo relies on, and if this crate
    /// called `video`'s decoder then a shared sign error would cancel and every
    /// test here would pass on a ROM the real renderer draws as noise. Two
    /// independent transcriptions of one documented formula disagree when either
    /// is wrong. `crates/sfemu`'s demo tests close the loop by rendering
    /// through the real `video`.
    fn read_pen(rom: &[u8], kind: Kind, code: u32, x: u32, y: u32) -> u8 {
        let start = (code as usize) * kind.bytes();
        let tile = &rom[start..start + kind.bytes()];
        let base = y * 4 * kind.frame_width() + 32 * (x >> 3) + (x & 7) + kind.x_bias();
        let mut pen = 0u8;
        for (p, off) in [24u32, 16, 8, 0].into_iter().enumerate() {
            let bit = base + off;
            if tile[(bit / 8) as usize] & (0x80 >> (bit % 8)) != 0 {
                pen |= 0x08 >> p;
            }
        }
        pen
    }

    /// Every pen of every pixel of every kind survives the round trip.
    ///
    /// The exhaustive form matters more than it looks: the plane table and the
    /// `32 * (x >> 3)` term both mean a wrong bit lands on a *different pixel*
    /// rather than nowhere, so a spot check on one pixel of one tile passes
    /// under a swapped plane order.
    #[test]
    fn a_written_pen_reads_back_through_the_decoders_own_rule() {
        for kind in [
            Kind::Tile8x8,
            Kind::Tile8x8Odd,
            Kind::Tile16x16,
            Kind::Tile32x32,
        ] {
            let mut rom = vec![0u8; kind.bytes()];
            for y in 0..kind.size() {
                for x in 0..kind.size() {
                    for pen in 0..16u8 {
                        put_pixel(&mut rom, kind, 0, x, y, pen);
                        assert_eq!(
                            read_pen(&rom, kind, 0, x, y),
                            pen,
                            "{kind:?} pixel ({x},{y}) pen {pen}"
                        );
                    }
                }
            }
        }
    }

    /// A pen written at one pixel does not disturb any other pixel.
    ///
    /// The companion to the round trip: a `put_pixel` that wrote a whole byte
    /// instead of one bit per plane would pass the test above — each pixel reads
    /// back what was last written to it — while destroying the seven pixels
    /// sharing its byte. Drawing a gradient and reading the whole tile is what
    /// catches that.
    #[test]
    fn writing_one_pixel_leaves_its_neighbours_alone() {
        let kind = Kind::Tile16x16;
        let mut rom = vec![0u8; kind.bytes()];
        let want = |x: u32, y: u32| ((x + 3 * y) % 16) as u8;
        for y in 0..kind.size() {
            for x in 0..kind.size() {
                put_pixel(&mut rom, kind, 0, x, y, want(x, y));
            }
        }
        for y in 0..kind.size() {
            for x in 0..kind.size() {
                assert_eq!(read_pen(&rom, kind, 0, x, y), want(x, y), "({x},{y})");
            }
        }
    }

    /// The two 8×8 kinds occupy disjoint halves of one 64-byte frame.
    ///
    /// This is the `Tile8x8Odd` `x_bias` doing its job: scroll 1 draws both
    /// halves of the same frame at adjacent columns, so a zero bias would make
    /// every odd column a copy of its even neighbour — a picture that looks
    /// deliberate.
    #[test]
    fn the_two_8x8_kinds_share_a_frame_without_overlapping() {
        let mut rom = vec![0u8; 64];
        solid(&mut rom, Kind::Tile8x8, 0, 0x0A);
        solid(&mut rom, Kind::Tile8x8Odd, 0, 0x05);
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(
                    read_pen(&rom, Kind::Tile8x8, 0, x, y),
                    0x0A,
                    "even ({x},{y})"
                );
                assert_eq!(
                    read_pen(&rom, Kind::Tile8x8Odd, 0, x, y),
                    0x05,
                    "odd ({x},{y})"
                );
            }
        }
    }

    /// A write past the end of the ROM is dropped, not folded back to tile 0.
    #[test]
    fn a_pixel_outside_the_rom_is_dropped_not_wrapped() {
        let mut rom = vec![0u8; 128];
        solid(&mut rom, Kind::Tile16x16, 9, 0x0F);
        assert!(rom.iter().all(|&b| b == 0), "nothing was written");
        // And an out-of-range coordinate, which is the other guard.
        put_pixel(&mut rom, Kind::Tile16x16, 0, 16, 0, 0x0F);
        put_pixel(&mut rom, Kind::Tile16x16, 0, 0, 16, 0x0F);
        assert!(rom.iter().all(|&b| b == 0), "still nothing");
    }

    /// `framed` draws its border on the edge and its fill inside.
    #[test]
    fn a_framed_tile_has_its_edge_pen_only_on_the_edge() {
        let kind = Kind::Tile32x32;
        let mut rom = vec![0u8; kind.bytes()];
        framed(&mut rom, kind, 0, 0x03, 0x0C);
        assert_eq!(read_pen(&rom, kind, 0, 0, 0), 0x0C, "top-left corner");
        assert_eq!(read_pen(&rom, kind, 0, 31, 31), 0x0C, "bottom-right corner");
        assert_eq!(read_pen(&rom, kind, 0, 15, 0), 0x0C, "top edge");
        assert_eq!(read_pen(&rom, kind, 0, 0, 15), 0x0C, "left edge");
        assert_eq!(read_pen(&rom, kind, 0, 1, 1), 0x03, "just inside");
        assert_eq!(read_pen(&rom, kind, 0, 16, 16), 0x03, "the middle");
    }

    /// `disc` leaves the corners transparent and the centre opaque.
    ///
    /// Both halves are the point: an opaque centre alone would pass on a `disc`
    /// that filled the tile, and that sprite would show as a square.
    #[test]
    fn a_disc_is_opaque_in_the_middle_and_transparent_at_the_corners() {
        let kind = Kind::Tile16x16;
        let mut rom = vec![0u8; kind.bytes()];
        disc(&mut rom, kind, 0, 0x07);
        assert_eq!(read_pen(&rom, kind, 0, 8, 8), 0x07, "the centre is drawn");
        for (x, y) in [(0, 0), (15, 0), (0, 15), (15, 15)] {
            assert_eq!(
                read_pen(&rom, kind, 0, x, y),
                TRANSPARENT_PEN,
                "corner ({x},{y}) shows what is behind it"
            );
        }
    }

    /// Each digit glyph is distinguishable from every other.
    ///
    /// The assertion a font actually needs: "some pixels are set" passes on ten
    /// identical blobs, and a counter of ten identical blobs is not a counter.
    #[test]
    fn the_ten_digits_are_ten_different_pictures() {
        let mut seen: Vec<Vec<u8>> = Vec::new();
        for value in 0..10u8 {
            let mut rom = vec![0u8; 64];
            digit(&mut rom, 0, value, 0x01);
            assert!(
                !seen.contains(&rom),
                "digit {value} draws the same pixels as an earlier one"
            );
            assert!(rom.iter().any(|&b| b != 0), "digit {value} drew nothing");
            seen.push(rom);
        }
    }

    /// A digit reads the same through both halves of its 8×8 frame.
    ///
    /// Scroll 1 picks the odd layout on odd columns of the same code, so a glyph
    /// present in only one half is legible in one column and blank in the next.
    /// Asserting on both halves of the same tile is the only way to see that: a
    /// single-half check passes on exactly the ROM that blinks.
    #[test]
    fn a_digit_is_drawn_into_both_halves_of_its_frame() {
        let mut rom = vec![0u8; 64];
        digit(&mut rom, 0, 7, 0x0C);
        let mut drawn = 0;
        for y in 0..8 {
            for x in 0..8 {
                let even = read_pen(&rom, Kind::Tile8x8, 0, x, y);
                let odd = read_pen(&rom, Kind::Tile8x8Odd, 0, x, y);
                assert_eq!(even, odd, "({x},{y}) differs between the halves");
                if even != TRANSPARENT_PEN {
                    drawn += 1;
                }
            }
        }
        // `7` is five pixels across the top plus one per row for six more rows.
        assert_eq!(drawn, 11, "the glyph's own pixel count");
    }

    /// `digit` takes its argument modulo ten rather than panicking or reading
    /// past its glyph table.
    #[test]
    fn a_digit_above_nine_wraps() {
        let mut ten = vec![0u8; 64];
        let mut zero = vec![0u8; 64];
        digit(&mut ten, 0, 10, 0x01);
        digit(&mut zero, 0, 0, 0x01);
        assert_eq!(ten, zero, "10 draws the same glyph as 0");
    }
}
