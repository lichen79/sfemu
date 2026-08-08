//! A 4×6 bitmap font, drawn in this repository.
//!
//! # Why it is drawn here
//!
//! The same reason no ROM is bundled: a typeface is someone's copyrighted work
//! unless it demonstrably is not. Every glyph below was drawn for this project as
//! ASCII art in this file, so there is nothing to license and nothing to fetch.
//!
//! # Why 4×6
//!
//! The overlay is drawn into the emulated framebuffer, which is 384×224 — the real
//! CPS-1 resolution. A font large enough to be comfortable would cover the game it
//! is annotating. 4×6 fits 76 characters across and 32 lines down, which is enough
//! for a register dump beside a disassembly.
//!
//! The requirement that follows is **not** beauty, it is that the sixteen hex digits
//! are told apart at a glance: a debugger rendering `8` and `B` alike shows the
//! wrong address and looks right doing it. Two tests hold that line —
//! `every_glyph_is_distinct`, which proves no two of the 95 printable characters
//! share a bitmap, and `the_hex_digits_are_the_bitmaps_drawn_here`, which pins the
//! sixteen that matter against hand-written literals.
//!
//! # Two representations, deliberately
//!
//! The table is written as pictures and parsed to bits by a `const fn`. The test
//! pins the hex digits as binary literals. Those are independent transcriptions of
//! the same glyph, which is what makes the pin worth having: a picture read wrongly
//! into bits fails against the literals, and a literal typed wrongly fails against
//! the picture. A test that re-derived its expectation from `glyph()` would prove
//! only that the table equals itself.
//!
//! # Spacing
//!
//! [`GLYPH_W`] is the ink width and [`ADVANCE`] is what the cursor moves, one pixel
//! more. That blank column is why a hex dump reads as separate numbers rather than
//! as a hedge — several glyphs use all four columns, so without it `#..#` beside
//! `#..#` renders as one eight-pixel shape. [`LINE`] is the vertical equivalent.

use machine::video::{HEIGHT, WIDTH};

/// A glyph's ink width, in pixels.
pub const GLYPH_W: usize = 4;

/// A glyph's height, in pixels. Rows 0–4 hold capitals and digits; row 5 is where
/// the descenders of `g`, `j`, `p`, `q`, `y`, and the tail of `Q` live.
pub const GLYPH_H: usize = 6;

/// What the cursor advances per character: [`GLYPH_W`] plus one blank column.
///
/// See the module docs — the gap is load-bearing for reading hex, not decoration.
pub const ADVANCE: usize = GLYPH_W + 1;

/// What a panel adds per line of text: [`GLYPH_H`] plus one blank row.
pub const LINE: usize = GLYPH_H + 1;

/// One row of a glyph, from four characters of art. `#` is ink; anything else is not.
///
/// `const` so the whole table is built at compile time and `glyph` is a lookup.
const fn row(art: &str) -> u8 {
    let b = art.as_bytes();
    assert!(b.len() == GLYPH_W, "a row of art is exactly GLYPH_W wide");
    let mut bits = 0u8;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'#' {
            // Leftmost column in the high bit of the nibble, so the literals in
            // `the_hex_digits_are_the_bitmaps_drawn_here` read left to right.
            bits |= 1 << (GLYPH_W - 1 - i);
        }
        i += 1;
    }
    bits
}

/// One glyph, from six rows of art.
macro_rules! g {
    ($r0:literal, $r1:literal, $r2:literal, $r3:literal, $r4:literal, $r5:literal) => {
        [row($r0), row($r1), row($r2), row($r3), row($r4), row($r5)]
    };
}

/// The first character with a glyph. Below this is control codes, which have no
/// printable form and render as `?`.
const FIRST: char = ' ';

/// The last character with a glyph: `~`, 0x7E. Everything above is either `DEL` or
/// outside ASCII.
const LAST: char = '~';

/// The font, indexed by `c as usize - FIRST as usize`, in ASCII order.
#[rustfmt::skip]
const GLYPHS: [[u8; GLYPH_H]; 95] = [
    g!("....", "....", "....", "....", "....", "...."), // ' '
    g!(".#..", ".#..", ".#..", "....", ".#..", "...."), // '!'
    g!("#.#.", "#.#.", "....", "....", "....", "...."), // '"'
    g!("#.#.", "####", "#.#.", "####", "#.#.", "...."), // '#'
    g!(".#..", ".###", ".##.", "###.", ".#..", "...."), // '$'
    g!("#..#", "...#", ".##.", "#...", "#..#", "...."), // '%'
    g!(".##.", "#.#.", ".##.", "#.#.", ".###", "...."), // '&'
    g!(".#..", ".#..", "....", "....", "....", "...."), // '\''
    g!("..#.", ".#..", ".#..", ".#..", "..#.", "...."), // '('
    g!(".#..", "..#.", "..#.", "..#.", ".#..", "...."), // ')'
    g!("....", "#.#.", ".#..", "#.#.", "....", "...."), // '*'
    g!("....", ".#..", "####", ".#..", "....", "...."), // '+'
    g!("....", "....", "....", "....", ".##.", ".#.."), // ','
    g!("....", "....", "####", "....", "....", "...."), // '-'
    g!("....", "....", "....", "....", ".#..", "...."), // '.'
    g!("...#", "..#.", "..#.", ".#..", "#...", "...."), // '/'
    g!(".##.", "#..#", "#..#", "#..#", ".##.", "...."), // '0'
    g!("..#.", ".##.", "..#.", "..#.", ".###", "...."), // '1'
    g!(".##.", "#..#", "..#.", ".#..", "####", "...."), // '2'
    g!("###.", "...#", ".##.", "...#", "###.", "...."), // '3'
    g!("#..#", "#..#", "####", "...#", "...#", "...."), // '4'
    g!("####", "#...", "###.", "...#", "###.", "...."), // '5'
    g!(".##.", "#...", "###.", "#..#", ".##.", "...."), // '6'
    g!("####", "...#", "..#.", ".#..", ".#..", "...."), // '7'
    g!(".##.", "#..#", ".##.", "#..#", ".##.", "...."), // '8'
    g!(".##.", "#..#", ".###", "...#", ".##.", "...."), // '9'
    g!("....", ".#..", "....", ".#..", "....", "...."), // ':'
    g!("....", ".#..", "....", ".##.", "#...", "...."), // ';'
    g!("...#", "..#.", ".#..", "..#.", "...#", "...."), // '<'
    g!("....", "####", "....", "####", "....", "...."), // '='
    g!("#...", ".#..", "..#.", ".#..", "#...", "...."), // '>'
    g!(".##.", "#..#", "..#.", "....", "..#.", "...."), // '?'
    g!(".##.", "#..#", "#.##", "#.##", ".##.", "...."), // '@'
    g!(".##.", "#..#", "####", "#..#", "#..#", "...."), // 'A'
    g!("###.", "#..#", "###.", "#..#", "###.", "...."), // 'B'
    g!(".###", "#...", "#...", "#...", ".###", "...."), // 'C'
    g!("###.", "#..#", "#..#", "#..#", "###.", "...."), // 'D'
    g!("####", "#...", "###.", "#...", "####", "...."), // 'E'
    g!("####", "#...", "###.", "#...", "#...", "...."), // 'F'
    g!(".###", "#...", "#.##", "#..#", ".###", "...."), // 'G'
    g!("#..#", "#..#", "####", "#..#", "#..#", "...."), // 'H'
    g!("###.", ".#..", ".#..", ".#..", "###.", "...."), // 'I'
    g!("..##", "...#", "...#", "#..#", ".##.", "...."), // 'J'
    g!("#..#", "#.#.", "##..", "#.#.", "#..#", "...."), // 'K'
    g!("#...", "#...", "#...", "#...", "####", "...."), // 'L'
    g!("#..#", "####", "####", "#..#", "#..#", "...."), // 'M'
    g!("#..#", "##.#", "#.##", "#..#", "#..#", "...."), // 'N'
    g!("####", "#..#", "#..#", "#..#", "####", "...."), // 'O'
    g!("###.", "#..#", "###.", "#...", "#...", "...."), // 'P'
    g!("####", "#..#", "#..#", "#..#", "####", "..##"), // 'Q'
    g!("###.", "#..#", "###.", "#.#.", "#..#", "...."), // 'R'
    g!(".###", "#...", ".##.", "...#", "###.", "...."), // 'S'
    g!("####", ".#..", ".#..", ".#..", ".#..", "...."), // 'T'
    g!("#..#", "#..#", "#..#", "#..#", ".##.", "...."), // 'U'
    g!("#..#", "#..#", "#..#", ".##.", ".##.", "...."), // 'V'
    g!("#..#", "#..#", "####", "####", "#..#", "...."), // 'W'
    g!("#..#", "#..#", ".##.", "#..#", "#..#", "...."), // 'X'
    g!("#..#", "#..#", ".##.", "..#.", "..#.", "...."), // 'Y'
    g!("####", "...#", ".##.", "#...", "####", "...."), // 'Z'
    g!(".##.", ".#..", ".#..", ".#..", ".##.", "...."), // '['
    g!("#...", ".#..", ".#..", "..#.", "...#", "...."), // '\\'
    g!(".##.", "..#.", "..#.", "..#.", ".##.", "...."), // ']'
    g!(".#..", "#.#.", "....", "....", "....", "...."), // '^'
    g!("....", "....", "....", "....", "....", "####"), // '_'
    g!("#...", ".#..", "....", "....", "....", "...."), // '`'
    g!("....", ".##.", "#..#", "#..#", ".###", "...."), // 'a'
    g!("#...", "#...", "###.", "#..#", "###.", "...."), // 'b'
    g!("....", ".###", "#...", "#...", ".###", "...."), // 'c'
    g!("...#", "...#", ".###", "#..#", ".###", "...."), // 'd'
    g!("....", ".##.", "####", "#...", ".##.", "...."), // 'e'
    g!("..##", ".#..", "###.", ".#..", ".#..", "...."), // 'f'
    g!("....", ".###", "#..#", ".###", "...#", ".##."), // 'g'
    g!("#...", "#...", "###.", "#..#", "#..#", "...."), // 'h'
    g!("..#.", "....", "..#.", "..#.", "..#.", "...."), // 'i'
    g!("..#.", "....", "..#.", "..#.", "..#.", "##.."), // 'j'
    g!("#...", "#.#.", "##..", "#.#.", "#..#", "...."), // 'k'
    g!(".#..", ".#..", ".#..", ".#..", ".##.", "...."), // 'l'
    g!("....", "....", "####", "####", "#..#", "...."), // 'm'
    g!("....", "....", "###.", "#..#", "#..#", "...."), // 'n'
    g!("....", ".##.", "#..#", "#..#", ".##.", "...."), // 'o'
    g!("....", "###.", "#..#", "###.", "#...", "#..."), // 'p'
    g!("....", ".###", "#..#", ".###", "...#", "...#"), // 'q'
    g!("....", ".###", "#...", "#...", "#...", "...."), // 'r'
    g!("....", ".###", "##..", "..##", "###.", "...."), // 's'
    g!(".#..", "###.", ".#..", ".#..", ".###", "...."), // 't'
    g!("....", "....", "#..#", "#..#", ".###", "...."), // 'u'
    g!("....", "....", "#..#", "#..#", ".##.", "...."), // 'v'
    g!("....", "....", "#..#", "####", "####", "...."), // 'w'
    g!("....", "....", "#..#", ".##.", "#..#", "...."), // 'x'
    g!("....", "#..#", "#..#", ".###", "...#", ".##."), // 'y'
    g!("....", "####", "..#.", ".#..", "####", "...."), // 'z'
    g!("..##", ".#..", "##..", ".#..", "..##", "...."), // '{'
    g!(".#..", ".#..", ".#..", ".#..", ".#..", "...."), // '|'
    g!("##..", "..#.", "..##", "..#.", "##..", "...."), // '}'
    g!("....", "....", ".#.#", "#.#.", "....", "...."), // '~'
];

/// The bitmap for `c`: one byte per row, low [`GLYPH_W`] bits used, leftmost column
/// in the high bit of the nibble.
///
/// Anything outside `' '..='~'` renders as `?` rather than panicking or drawing
/// nothing. A debugger asked to display a byte that is not text should show that it
/// could not, and an invisible answer is indistinguishable from an empty string.
pub fn glyph(c: char) -> [u8; GLYPH_H] {
    if c < FIRST || c > LAST {
        return GLYPHS['?' as usize - FIRST as usize];
    }
    GLYPHS[c as usize - FIRST as usize]
}

/// Draws `s` at `(x, y)` in `fg`, returning the x where the next character would go.
///
/// Only ink pixels are written, so text lands *over* whatever is already in the
/// buffer — the rendered game frame, or a panel background from [`fill_rect`].
///
/// Clipped on every side, and a string starting entirely off-screen draws nothing
/// rather than panicking. Clipping matters more than it sounds: a glyph that wrapped
/// at the right edge would reappear on the far left one row down, which looks like a
/// rendering bug in the *game* rather than in the overlay.
///
/// # Panics
///
/// If `buf` is not a `WIDTH × HEIGHT` frame. Every buffer here is one, and a
/// wrong-sized one is a programming error worth failing loudly for rather than
/// clipping silently into.
pub fn draw_text(buf: &mut [u32], x: usize, y: usize, s: &str, fg: u32) -> usize {
    assert_eq!(buf.len(), WIDTH * HEIGHT, "not a frame");
    let mut cx = x;
    for c in s.chars() {
        for (r, bits) in glyph(c).iter().enumerate() {
            let py = y + r;
            if py >= HEIGHT {
                continue;
            }
            for col in 0..GLYPH_W {
                if bits & (1 << (GLYPH_W - 1 - col)) == 0 {
                    continue;
                }
                let px = cx + col;
                if px >= WIDTH {
                    continue;
                }
                buf[py * WIDTH + px] = fg;
            }
        }
        // Advanced even for a glyph that was entirely clipped, so that a string
        // starting off the left edge — or one long enough to run off the right —
        // keeps its remaining characters at the pixels they would have had. A
        // cursor that only moved for visible glyphs would slide the tail of a
        // clipped string leftwards into the middle of the screen.
        cx += ADVANCE;
    }
    cx
}

/// Fills a `w × h` rectangle at `(x, y)`, clipped to the frame. Panel backgrounds.
///
/// # Panics
///
/// If `buf` is not a `WIDTH × HEIGHT` frame, as [`draw_text`].
pub fn fill_rect(buf: &mut [u32], x: usize, y: usize, w: usize, h: usize, c: u32) {
    assert_eq!(buf.len(), WIDTH * HEIGHT, "not a frame");
    for py in y..(y + h).min(HEIGHT) {
        for px in x..(x + w).min(WIDTH) {
            buf[py * WIDTH + px] = c;
        }
    }
}

/// A filled cell with a one-pixel border, for a colour swatch.
///
/// Clipped like [`fill_rect`]. A swatch under 3 pixels in either axis is all border:
/// the interior is `w.saturating_sub(2)` wide, so a 1×1 swatch is one border pixel
/// rather than an underflow.
pub fn swatch(buf: &mut [u32], x: usize, y: usize, w: usize, h: usize, fill: u32, border: u32) {
    fill_rect(buf, x, y, w, h, border);
    fill_rect(
        buf,
        x + 1,
        y + 1,
        w.saturating_sub(2),
        h.saturating_sub(2),
        fill,
    );
}

/// Reads `n` characters back off the buffer at `(x, y)`, for the overlay's tests.
///
/// This is how a panel test asserts what a panel *shows* rather than re-running the
/// formatter it used and comparing that to itself. It proves everything between the
/// formatting and the pixels: layout, colour, clipping, and the format string.
///
/// Built by inverting [`GLYPHS`], which is the only way to build it and is also
/// exactly why `the_hex_digits_are_the_bitmaps_drawn_here` exists: a recogniser
/// derived from the table cannot detect an error *in* the table. Two glyphs whose
/// bitmaps were swapped would render wrongly and read back wrongly in the same way,
/// and every panel test would pass.
///
/// `fg` is which colour counts as ink. Not optional and not "anything non-zero":
/// panels draw on a filled background, where every pixel is non-zero.
///
/// A cell matching no glyph reads as [`NOT_A_GLYPH`].
#[cfg(test)]
pub(crate) fn read_text(buf: &[u32], x: usize, y: usize, n: usize, fg: u32) -> String {
    (0..n)
        .map(|i| read_cell(buf, x + i * ADVANCE, y, fg))
        .collect()
}

/// What [`read_text`] reports for a cell whose pixels are no glyph.
///
/// Not a space and not a dropped character: a panel test asserting `"D0 1234ABCD"`
/// against a cell of noise must fail, and both of those would let it pass.
#[cfg(test)]
pub(crate) const NOT_A_GLYPH: char = '\u{FFFD}';

/// Whether `needle` appears on any glyph row of `buf`, in `fg`.
///
/// Scans every candidate baseline *and* every horizontal phase, so a test asserting
/// some text is present does not also have to know which row and column it landed on
/// — that is what the `read_text` assertions against exact coordinates are for.
/// `ADVANCE` phases is enough: a panel's columns are `x0 + i * ADVANCE`, so starting
/// the scan at `x0 % ADVANCE` reads exactly its cells, whatever `x0` is.
///
/// Lives here rather than in `overlay`'s tests because `gfxpanels` reads its views
/// back the same way, and a second glyph scanner is a second answer.
#[cfg(test)]
pub(crate) fn panel_contains(buf: &[u32], needle: &str, fg: u32) -> bool {
    (0..HEIGHT.saturating_sub(GLYPH_H)).any(|y| {
        (0..ADVANCE).any(|phase| {
            let cols = (WIDTH - phase) / ADVANCE;
            read_text(buf, phase, y, cols, fg).contains(needle)
        })
    })
}

/// An empty frame.
#[cfg(test)]
pub(crate) fn frame() -> Vec<u32> {
    vec![0u32; WIDTH * HEIGHT]
}

/// One cell of [`read_text`].
#[cfg(test)]
fn read_cell(buf: &[u32], x: usize, y: usize, fg: u32) -> char {
    let mut cell = [0u8; GLYPH_H];
    for (r, bits) in cell.iter_mut().enumerate() {
        for col in 0..GLYPH_W {
            let (px, py) = (x + col, y + r);
            if px < WIDTH && py < HEIGHT && buf[py * WIDTH + px] == fg {
                *bits |= 1 << (GLYPH_W - 1 - col);
            }
        }
    }
    let i = GLYPHS.iter().position(|g| *g == cell);
    match i {
        Some(i) => char::from(FIRST as u8 + u8::try_from(i).expect("95 glyphs")),
        None => NOT_A_GLYPH,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every glyph differs from every other.
    ///
    /// The only legibility claim a test can make, and the one that matters: hex is
    /// what the panels are made of, and a debugger that renders `8` and `B`
    /// identically displays the wrong address and looks right doing it. It is also
    /// what makes [`read_text`] a function rather than a guess.
    #[test]
    fn every_glyph_is_distinct() {
        let mut seen: Vec<(char, [u8; GLYPH_H])> = Vec::new();
        for c in FIRST..=LAST {
            let g = glyph(c);
            if let Some((other, _)) = seen.iter().find(|(_, o)| *o == g) {
                panic!("{c:?} and {other:?} have the same bitmap: {g:?}");
            }
            seen.push((c, g));
        }
        assert_eq!(seen.len(), 95, "ASCII 0x20..=0x7E is 95 glyphs");
    }

    /// The hex digits are the bitmaps written here, not merely distinct.
    ///
    /// `every_glyph_is_distinct` cannot catch a **transposition** — two glyphs whose
    /// bitmaps are swapped are still distinct, and [`read_text`] is built by
    /// inverting the same table, so it reads a transposed font back exactly as
    /// wrongly as it renders it. Every panel test would still pass. So the sixteen
    /// characters the panels are actually made of are pinned against literals typed
    /// by hand, which is a second, independent transcription of the same pictures.
    #[test]
    fn the_hex_digits_are_the_bitmaps_drawn_here() {
        // 0     1     2     3     4     5     6     7
        // .##.  ..#.  .##.  ###.  #..#  ####  .##.  ####
        // #..#  .##.  #..#  ...#  #..#  #...  #...  ...#
        // #..#  ..#.  ..#.  .##.  ####  ###.  ###.  ..#.
        // #..#  ..#.  .#..  ...#  ...#  ...#  #..#  .#..
        // .##.  .###  ####  ###.  ...#  ###.  .##.  .#..
        assert_eq!(glyph('0'), [0b0110, 0b1001, 0b1001, 0b1001, 0b0110, 0b0000]);
        assert_eq!(glyph('1'), [0b0010, 0b0110, 0b0010, 0b0010, 0b0111, 0b0000]);
        assert_eq!(glyph('2'), [0b0110, 0b1001, 0b0010, 0b0100, 0b1111, 0b0000]);
        assert_eq!(glyph('3'), [0b1110, 0b0001, 0b0110, 0b0001, 0b1110, 0b0000]);
        assert_eq!(glyph('4'), [0b1001, 0b1001, 0b1111, 0b0001, 0b0001, 0b0000]);
        assert_eq!(glyph('5'), [0b1111, 0b1000, 0b1110, 0b0001, 0b1110, 0b0000]);
        assert_eq!(glyph('6'), [0b0110, 0b1000, 0b1110, 0b1001, 0b0110, 0b0000]);
        assert_eq!(glyph('7'), [0b1111, 0b0001, 0b0010, 0b0100, 0b0100, 0b0000]);
        // 8     9     A     B     C     D     E     F
        // .##.  .##.  .##.  ###.  .###  ###.  ####  ####
        // #..#  #..#  #..#  #..#  #...  #..#  #...  #...
        // .##.  .###  ####  ###.  #...  #..#  ###.  ###.
        // #..#  ...#  #..#  #..#  #...  #..#  #...  #...
        // .##.  .##.  #..#  ###.  .###  ###.  ####  #...
        assert_eq!(glyph('8'), [0b0110, 0b1001, 0b0110, 0b1001, 0b0110, 0b0000]);
        assert_eq!(glyph('9'), [0b0110, 0b1001, 0b0111, 0b0001, 0b0110, 0b0000]);
        assert_eq!(glyph('A'), [0b0110, 0b1001, 0b1111, 0b1001, 0b1001, 0b0000]);
        assert_eq!(glyph('B'), [0b1110, 0b1001, 0b1110, 0b1001, 0b1110, 0b0000]);
        assert_eq!(glyph('C'), [0b0111, 0b1000, 0b1000, 0b1000, 0b0111, 0b0000]);
        assert_eq!(glyph('D'), [0b1110, 0b1001, 0b1001, 0b1001, 0b1110, 0b0000]);
        assert_eq!(glyph('E'), [0b1111, 0b1000, 0b1110, 0b1000, 0b1111, 0b0000]);
        assert_eq!(glyph('F'), [0b1111, 0b1000, 0b1110, 0b1000, 0b1000, 0b0000]);
    }

    /// Text lands at the pixels the layout says, in the colour asked for.
    #[test]
    fn a_glyph_is_drawn_where_it_is_asked_for() {
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        let end = draw_text(&mut buf, 10, 20, "1", 0x00FF_FFFF);
        assert_eq!(end, 10 + ADVANCE, "the cursor advanced one cell");
        for (r, bits) in glyph('1').iter().enumerate() {
            for col in 0..GLYPH_W {
                let on = bits & (1 << (GLYPH_W - 1 - col)) != 0;
                assert_eq!(
                    buf[(20 + r) * WIDTH + 10 + col],
                    if on { 0x00FF_FFFF } else { 0 },
                    "pixel ({col},{r}) of '1'"
                );
            }
        }
        // The premise: that loop compared something. A glyph of all zeros would
        // satisfy every assertion above against an untouched buffer.
        assert!(
            buf.iter().filter(|&&p| p == 0x00FF_FFFF).count() > 4,
            "'1' is more than four pixels of ink"
        );
    }

    /// Only ink is written, so text lands over what is already there.
    ///
    /// The overlay is drawn on top of a rendered game frame. A `draw_text` that
    /// painted a background would leave a rectangle of it around every character.
    #[test]
    fn drawing_text_leaves_the_pixels_around_the_ink_alone() {
        let mut buf = vec![0x00AA_AAAA_u32; WIDTH * HEIGHT];
        draw_text(&mut buf, 0, 0, "1", 0x00FF_FFFF);
        // '1' has no ink in column 0 of any row, so that column must be untouched.
        for r in 0..GLYPH_H {
            assert_eq!(buf[r * WIDTH], 0x00AA_AAAA, "column 0, row {r}");
        }
        assert_eq!(buf[2 * WIDTH + 2], 0x00FF_FFFF, "and the ink did land");
    }

    /// Adjacent glyphs do not touch.
    ///
    /// Several glyphs use all four columns, so without the blank column [`ADVANCE`]
    /// adds, `#..#` beside `#..#` renders as one eight-pixel shape and a hex dump
    /// reads as a hedge. Checked with the two widest glyphs there are.
    #[test]
    fn adjacent_glyphs_do_not_touch() {
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        draw_text(&mut buf, 0, 0, "MM", 0x00FF_FFFF);
        for r in 0..GLYPH_H {
            assert_eq!(
                buf[r * WIDTH + GLYPH_W],
                0,
                "the column between two 'M's must be blank, row {r}"
            );
        }
        // The premise: 'M' really does reach the last of its own columns, so the gap
        // above is a gap between two glyphs rather than the edge of a narrow one.
        assert_eq!(glyph('M')[0] & 1, 1, "'M' inks its rightmost column");
    }

    /// Drawing off the edge clips rather than panicking or wrapping.
    ///
    /// A wrapped glyph would appear on the far side of the screen one row down,
    /// which looks like a rendering bug in the *game*.
    #[test]
    fn text_at_the_edge_is_clipped_not_wrapped() {
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        draw_text(&mut buf, WIDTH - 2, 0, "8", 0x00FF_FFFF);
        assert!(
            buf[..GLYPH_H * WIDTH]
                .iter()
                .enumerate()
                .all(|(i, &p)| p == 0 || i % WIDTH >= WIDTH - 2),
            "a clipped glyph must not wrap to the next row"
        );
        // The premise: the visible part of it was drawn, so the assertion above is
        // about clipping rather than about a glyph that never appeared.
        assert!(
            buf[..GLYPH_H * WIDTH].iter().any(|&p| p != 0),
            "the two columns that fit must still be drawn"
        );
        // And drawing entirely outside is a no-op, not a panic.
        draw_text(&mut buf, WIDTH + 100, HEIGHT + 100, "8", 0x00FF_FFFF);
        draw_text(&mut buf, 0, HEIGHT - 1, "88888", 0x00FF_FFFF);
    }

    /// The cursor advances for clipped glyphs too.
    ///
    /// A caller chaining `draw_text` calls — the disassembly panel does, one for the
    /// address and one for the mnemonic — uses the returned x. If the cursor only
    /// moved for glyphs that were actually drawn, a string running off the right edge
    /// would return a position back inside the frame, and the next call would land on
    /// top of the game rather than off it. Asserted as arithmetic on the return value,
    /// because the pixels of a clipped glyph are by definition not there to read.
    #[test]
    fn the_cursor_advances_for_clipped_glyphs_too() {
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        let start = WIDTH - ADVANCE;
        let end = draw_text(&mut buf, start, 0, "888", 0x00FF_FFFF);
        assert_eq!(
            end,
            start + 3 * ADVANCE,
            "three cells advance three cells, drawn or not"
        );
        assert!(
            end > WIDTH,
            "the premise: two of those three were off-screen"
        );
        // And from wholly outside the frame, where *nothing* is drawn: the cursor is
        // still the caller's arithmetic, not a report of what landed.
        let far = WIDTH + 100;
        assert_eq!(
            draw_text(&mut buf, far, 0, "88", 0x00FF_FFFF),
            far + 2 * ADVANCE
        );
        assert_eq!(
            draw_text(&mut buf, 4, 0, "", 0x00FF_FFFF),
            4,
            "empty moves nothing"
        );
    }

    /// An unprintable character renders as `?` rather than panicking.
    #[test]
    fn an_unknown_character_is_a_question_mark() {
        assert_eq!(glyph('\u{1F600}'), glyph('?'));
        assert_eq!(glyph('\n'), glyph('?'));
        assert_eq!(glyph('\u{7F}'), glyph('?'), "DEL is above the last glyph");
        assert_ne!(glyph('?'), [0; GLYPH_H], "and '?' is not blank");
    }

    /// `fill_rect` clips on every side and does not resize the frame.
    #[test]
    fn a_rectangle_is_clipped_to_the_frame() {
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        fill_rect(&mut buf, WIDTH - 2, HEIGHT - 2, 10, 10, 0x11);
        assert_eq!(buf[(HEIGHT - 1) * WIDTH + WIDTH - 1], 0x11, "the corner");
        assert_eq!(buf.len(), WIDTH * HEIGHT, "the buffer was not resized");
        assert_eq!(
            buf.iter().filter(|&&p| p == 0x11).count(),
            4,
            "and only the 2×2 that fits was filled"
        );
        // Entirely outside is a no-op.
        fill_rect(&mut buf, WIDTH, HEIGHT, 4, 4, 0x22);
        assert!(!buf.contains(&0x22));
    }

    /// A swatch is its fill, inside its border.
    ///
    /// `fill_rect` alone is not enough: two adjacent swatches of similar colours are
    /// one indistinguishable block, and the palette view draws 3072 of them.
    #[test]
    fn a_swatch_is_a_fill_inside_a_border() {
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        swatch(&mut buf, 10, 20, 5, 4, 0x0011_2233, 0x00FF_FFFF);
        // The border is the outermost ring.
        assert_eq!(buf[20 * WIDTH + 10], 0x00FF_FFFF, "top-left corner");
        assert_eq!(buf[20 * WIDTH + 14], 0x00FF_FFFF, "top-right corner");
        assert_eq!(buf[23 * WIDTH + 10], 0x00FF_FFFF, "bottom-left corner");
        assert_eq!(buf[21 * WIDTH + 10], 0x00FF_FFFF, "left edge");
        assert_eq!(buf[20 * WIDTH + 12], 0x00FF_FFFF, "top edge");
        // The fill is what the border encloses.
        assert_eq!(buf[21 * WIDTH + 11], 0x0011_2233, "the interior");
        assert_eq!(buf[22 * WIDTH + 13], 0x0011_2233, "the interior");
        // And nothing outside.
        assert_eq!(buf[19 * WIDTH + 10], 0, "one row above");
        assert_eq!(buf[20 * WIDTH + 15], 0, "one column right");
        assert_eq!(buf[24 * WIDTH + 10], 0, "one row below");
    }

    /// A swatch too small for a border is all border, not a panic.
    ///
    /// The palette view's swatches are about 5×4 and a narrower window would make
    /// them 1×1. An interior computed as `w - 2` would underflow.
    #[test]
    fn a_swatch_smaller_than_its_border_is_all_border() {
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        swatch(&mut buf, 0, 0, 1, 1, 0x0011_2233, 0x00FF_FFFF);
        assert_eq!(buf[0], 0x00FF_FFFF, "a 1x1 swatch is its border");
        swatch(&mut buf, 0, 2, 2, 2, 0x0011_2233, 0x00FF_FFFF);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert_eq!(buf[(2 + dy) * WIDTH + dx], 0x00FF_FFFF, "2x2 is all border");
        }
    }

    /// The recogniser reads back what was drawn.
    ///
    /// [`read_text`] is a test tool the panel tests rest on, so it gets its own
    /// test rather than being trusted. It cannot catch an error in the font table —
    /// see its docs — but it can be wrong about spacing, about which colour is ink,
    /// and about cells that hold no glyph at all.
    #[test]
    fn the_recogniser_reads_back_what_was_drawn() {
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        draw_text(&mut buf, 8, 16, "D0 1234ABCD", 0x00FF_FFFF);
        assert_eq!(read_text(&buf, 8, 16, 11, 0x00FF_FFFF), "D0 1234ABCD");
        // A colour that is not the ink reads as blanks: the recogniser must not
        // count the game's own pixels as text.
        assert_eq!(read_text(&buf, 8, 16, 4, 0x0000_FF00), "    ");
        // And a cell holding something that is no glyph is not silently a space.
        // A stray pixel in `D`'s last row, which the font leaves blank — the sort of
        // thing a neighbouring panel's border would leave behind.
        buf[(16 + GLYPH_H - 1) * WIDTH + 8] = 0x00FF_FFFF;
        assert_eq!(
            read_text(&buf, 8, 16, 1, 0x00FF_FFFF).chars().next(),
            Some(NOT_A_GLYPH),
            "a cell matching no glyph is neither a space nor dropped"
        );
    }
}
