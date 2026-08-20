//! SF1's graphics panels: what the four views draw for an `Sf1`.
//!
//! # Why this is a second module and not a parameter
//!
//! The chrome is shared — [`crate::gfxpanels`]'s box, colours, padding, greys and
//! text helpers are all `pub(crate)` and used verbatim here, so the two boards'
//! panels cannot drift apart in layout. What forks is the *content*, in six ways
//! that no parameter hides: four graphics regions instead of one, two tile
//! granularities instead of one, maps that live in ROM instead of in guest RAM,
//! maps that are 2048×16 and 64×32 instead of 64×64, 1,024 palette entries with a
//! different DAC rule, and no layer-order register at all.
//!
//! # The maps are in ROM, and the panels say so
//!
//! SF1's background and foreground map entries come from the tilemap ROM, not from
//! anything the 68000 can write. Every tilemap panel line names the source, because
//! a reader who assumes RAM will not understand why writes never change the
//! picture, and will go looking for a bug in the memory map.
//!
//! # This crate holds no ROM
//!
//! No ROM is bundled, fetched, or committed, including as a test fixture. The tests
//! here build their graphics regions from a program written inline.

use crate::font::{fill_rect, swatch, ADVANCE, LINE};
use crate::gfxpanels::{
    content_y, grey, put, text, title_at, View, BG, EDGE, FG, HI, OFF, PAD, VH, VW, VX, VY,
};
use machine::video::sf1::palette::ENTRIES as PAL_ENTRIES;
use machine::video::sf1::tilemap::{self, MapKind};
use machine::video::sf1::{LayerMask, Plane, ACTIVE_FLIP};
use machine::video::{HEIGHT, WIDTH};
use machine::Sf1;

/// Cells of a tile view's row label: four hex digits and a space.
///
/// Four digits reach 0xFFFF, and SF1's largest region — `gfx3` at 0x1C0000 bytes —
/// holds 14,336 sprite elements, which is 0x3800.
const TILE_LABEL: usize = 5;

/// Palette swatches per row. 64 columns of 5 pixels is 320, which fits the box.
const PAL_COLS: usize = 64;
/// One swatch's width.
const PAL_CW: usize = 5;
/// Ditto, height. All 1,024 entries fit in sixteen rows, so this view never pages.
const PAL_CH: usize = 4;

/// Columns of map codes the tilemap view shows around its cursor.
const MAP_COLS: usize = 8;
/// Ditto, rows.
const MAP_ROWS: usize = 8;
/// Cells one code takes: four hex digits and a space.
const MAP_CELL: usize = 5;
/// Cells of the tilemap window's row label: four decimal digits and a space.
///
/// Four, not CPS-1's two: the background map has 2,048 columns, and a two-digit
/// label would print row 2047 as "47".
const MAP_LABEL: usize = 5;

/// What an SF1 graphics view is looking at.
///
/// [`crate::gfxpanels::ViewState`]'s counterpart. `view` is the same type, because
/// the four views and the key that cycles them are chrome; `plane` and `map` replace
/// CPS-1's `kind` and `layer`, because SF1 has four graphics regions and three maps
/// where CPS-1 has one region and three layers of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sf1ViewState {
    /// Which view is shown.
    pub view: View,
    /// Which plane the tile view browses.
    pub plane: Plane,
    /// Which map the tilemap view walks.
    pub map: MapKind,
    /// The first tile code on the tile view's page.
    pub tile_at: u32,
    /// The cursored palette entry.
    pub pal_at: usize,
    /// The cursored map cell, or `None` to follow the renderer's own origin.
    pub map_at: Option<(u32, u32)>,
    /// The cursored row of the layers view, `0..4`.
    pub row: usize,
    /// Which planes the viewer permits.
    pub mask: LayerMask,
}

/// How many tiles the tile view shows: `(columns, rows)`.
///
/// Published with [`tile_cell`] so the tests read a tile's pixels back at the
/// coordinates the view drew them, rather than computing a second layout.
#[must_use]
pub fn tile_grid(plane: Plane) -> (usize, usize) {
    let step = plane.layout().width as usize + 1;
    // A row is at least a line of text high, so the row label always fits beside it.
    let row_step = if step > LINE { step } else { LINE };
    let x0 = VX + PAD + TILE_LABEL * ADVANCE;
    let y0 = content_y();
    let cols = (VX + VW - PAD).saturating_sub(x0) / step;
    let rows = (VY + VH - PAD).saturating_sub(y0) / row_step;
    (cols, rows)
}

/// The top-left pixel of the tile view's `slot`th cell, counting across then down.
#[must_use]
pub fn tile_cell(plane: Plane, slot: usize) -> (usize, usize) {
    let step = plane.layout().width as usize + 1;
    let row_step = if step > LINE { step } else { LINE };
    let (cols, _) = tile_grid(plane);
    let x0 = VX + PAD + TILE_LABEL * ADVANCE;
    let y0 = content_y();
    (x0 + (slot % cols) * step, y0 + (slot / cols) * row_step)
}

/// The top-left pixel of palette entry `n`'s swatch.
#[must_use]
pub fn pal_cell(n: usize) -> (usize, usize) {
    let x0 = VX + PAD + 3 * ADVANCE;
    let y0 = content_y();
    (x0 + (n % PAL_COLS) * PAL_CW, y0 + (n / PAL_COLS) * PAL_CH)
}

/// The map cell the renderer fetches for the visible top-left pixel of `map`.
///
/// The tilemap view's cursor default. [`tilemap::sample`] is called rather than
/// re-derived: the raster bias, the mirror under screen flip, the two `rem_euclid`s
/// and the scroll's reduction are five decisions the renderer has already made, and
/// a panel with a sixth reading of them would name a cell that was never drawn.
#[must_use]
pub fn map_origin(m: &Sf1, map: MapKind) -> (u32, u32) {
    let flip = m.board.active & ACTIVE_FLIP != 0;
    let scroll = map.scroll(m.board.bgscroll, m.board.fgscroll);
    let s = tilemap::sample(map.map(), scroll, flip, 0, 0);
    (s.col, s.row)
}

/// Draw the SF1 graphics view `s` names over the whole frame.
///
/// # Panics
///
/// If `buf` is not a `WIDTH × HEIGHT` frame, as [`crate::font::draw_text`].
pub fn draw(buf: &mut [u32], m: &Sf1, s: &Sf1ViewState) {
    assert_eq!(buf.len(), WIDTH * HEIGHT, "not a frame");
    fill_rect(buf, VX, VY, VW, VH, BG);
    match s.view {
        View::Tiles => draw_tiles(buf, m, s),
        View::Tilemap => draw_tilemap_view(buf, m, s),
        View::Palette => draw_palette(buf, m, s),
        View::Layers => draw_layers(buf, m, s),
    }
}

/// One plane's graphics region as a grid of tiles, in greyscale.
fn draw_tiles(buf: &mut [u32], m: &Sf1, s: &Sf1ViewState) {
    let gfx = m.video.gfx(s.plane);
    let (cols, rows) = tile_grid(s.plane);
    let page = (cols * rows) as u32;
    let size = s.plane.layout().width;
    let (tx, ty) = title_at();
    text(
        buf,
        tx,
        ty,
        &format!(
            "{} {} {:05X}-{:05X} OF {:05X} ENTER CYCLES",
            View::Tiles.name(),
            s.plane.name(),
            s.tile_at,
            s.tile_at.saturating_add(page.saturating_sub(1)),
            gfx.elements()
        ),
        HI,
    );
    // The two facts a reader cannot get from the picture: which tile size this
    // plane decodes with, and how far up the palette its colours start.
    text(
        buf,
        tx,
        ty + LINE,
        &format!(
            "{size}PX GRAN {:02} BASE {:03X} BYTES {:06X}",
            gfx.granularity(),
            s.plane.colour_base(),
            m.video.region(s.plane).len()
        ),
        FG,
    );
    for row in 0..rows {
        // Saturating, because `tile_at` is a `u32` the bracket keys drive and a page
        // past 0xFFFFFFFF must show as "not in the region", not panic in a debug
        // build.
        let first = s.tile_at.saturating_add((row * cols) as u32);
        let (_, cy) = tile_cell(s.plane, row * cols);
        text(buf, VX + PAD, cy, &format!("{first:04X}"), FG);
        for col in 0..cols {
            let code = first.saturating_add(col as u32);
            let (cx, cy) = tile_cell(s.plane, row * cols + col);
            // `elements()` is exactly the last code every pixel of which decodes —
            // there is no bank mapper here to move a code outside its own region, so
            // this needs no `tile_in_rom` equivalent.
            if code >= gfx.elements() {
                // One dot, not a fill: "past the end of the region" must not be
                // mistakable for a tile whose pens happen to be uniform.
                put(buf, cx, cy, OFF);
                continue;
            }
            for y in 0..size {
                for x in 0..size {
                    let pen = gfx.pen(code, x, y).unwrap_or(0);
                    put(buf, cx + x as usize, cy + y as usize, grey(pen));
                }
            }
        }
    }
}

/// One map's entries, around the cursor, with the cursored tile drawn beside them.
fn draw_tilemap_view(buf: &mut [u32], m: &Sf1, s: &Sf1ViewState) {
    let map = s.map.map();
    let (cur_c, cur_r) = s.map_at.unwrap_or_else(|| map_origin(m, s.map));
    let tilerom = m.video.tilerom();
    let videoram = &m.board.videoram[..];
    let index = map.scan.index(cur_c, cur_r, map.cols, map.rows);
    let info = s.map.tile_info(tilerom, videoram, index);
    let gfx = m.video.gfx(s.map.plane());
    let (tx, ty) = title_at();

    text(
        buf,
        tx,
        ty,
        &format!(
            "{} {} MAP {:04},{:02} CODE {:04X}",
            View::Tilemap.name(),
            s.map.name(),
            cur_c,
            cur_r,
            info.code
        ),
        HI,
    );
    let flip = |b: bool, c: char| if b { c } else { '-' };
    text(
        buf,
        tx,
        ty + LINE,
        &format!(
            "COL {:02X} FLIP {}{} CELL {:05X} OF {:05X}",
            info.colour,
            flip(info.flags & tilemap::FLIP_X != 0, 'X'),
            flip(info.flags & tilemap::FLIP_Y != 0, 'Y'),
            index,
            map.tiles()
        ),
        FG,
    );
    // Where this map's entries live. Two of the three are in the tilemap ROM and no
    // guest write can change them; saying so is the answer to the question a reader
    // otherwise spends an afternoon on.
    match s.map.tilerom_base() {
        Some(base) => text(
            buf,
            tx,
            ty + 2 * LINE,
            &format!(
                "ROM {:05X} SIZE {:06X} IN ROM",
                base + 2 * index as usize,
                tilerom.len()
            ),
            FG,
        ),
        None => text(
            buf,
            tx,
            ty + 2 * LINE,
            "RAM VIDEORAM: THE 68000 WRITES THIS",
            FG,
        ),
    }
    text(
        buf,
        tx,
        ty + 3 * LINE,
        &format!(
            "{}X{} SCR {:05} GRID {}X{} ENTER CYCLES MAP",
            map.tile_w,
            map.tile_h,
            s.map.scroll(m.board.bgscroll, m.board.fgscroll),
            map.cols,
            map.rows
        ),
        FG,
    );

    // The window of codes: the cursor three cells in, so there is context on both
    // sides of it, wrapped at *this map's* edges — BG and FG are 2048×16 and TX is
    // 64×32, so a shared 64×64 wrap would show rows the map does not have.
    let gx = VX + PAD + MAP_LABEL * ADVANCE;
    let gy = content_y() + 4 * LINE;
    // ⚠️ `- 3 % map.cols` and not `- 3`: a map with fewer than three columns — which
    // `Tilemap`'s public fields permit and this crate's constants never produce —
    // would make the plain subtraction go past zero and panic in a debug build.
    let first_c = (cur_c + map.cols - 3 % map.cols) % map.cols;
    let first_r = (cur_r + map.rows - 3 % map.rows) % map.rows;
    for r in 0..MAP_ROWS {
        let row = (first_r + r as u32) % map.rows;
        let y = gy + r * LINE;
        // Four digits, not two: a 2048-column map reaches 2047.
        text(buf, VX + PAD, y, &format!("{row:04}"), FG);
        for c in 0..MAP_COLS {
            let col = (first_c + c as u32) % map.cols;
            let t = s.map.tile_info(
                tilerom,
                videoram,
                map.scan.index(col, row, map.cols, map.rows),
            );
            let here = col == cur_c && row == cur_r;
            text(
                buf,
                gx + c * MAP_CELL * ADVANCE,
                y,
                &format!("{:04X}", t.code),
                if here { HI } else { FG },
            );
        }
    }

    // And the cursored tile itself, so "the map points at the wrong tile" and "the
    // tile is wrong" are two different pictures.
    let px = gx + MAP_COLS * MAP_CELL * ADVANCE + ADVANCE;
    if info.code < gfx.elements() {
        for y in 0..map.tile_h {
            for x in 0..map.tile_w {
                let pen = gfx.pen(info.code, x, y).unwrap_or(0);
                put(buf, px + x as usize, gy + y as usize, grey(pen));
            }
        }
    } else {
        text(buf, px, gy, "OFF ROM", OFF);
    }
}

/// All 1,024 palette entries as swatches, with the cursored one named.
fn draw_palette(buf: &mut [u32], m: &Sf1, s: &Sf1ViewState) {
    let pal = &m.board.palette[..];
    let at = s.pal_at.min(PAL_ENTRIES - 1);
    let entry = pal.get(at).copied().unwrap_or(0);
    let (tx, ty) = title_at();
    text(
        buf,
        tx,
        ty,
        &format!(
            "{} {:04X} ENTRY {:04X} OF {:04X}",
            View::Palette.name(),
            at,
            entry,
            PAL_ENTRIES
        ),
        HI,
    );
    for (n, &raw) in pal.iter().take(PAL_ENTRIES).enumerate() {
        // The window's own conversion, not a second one: [`crate::pixels::argb_sf1`]
        // is what the frame the game is drawn into goes through, so a swatch and the
        // game agree by construction rather than by inspection. CPS-1's `argb` would
        // halve every channel here.
        let fill = crate::pixels::argb_sf1(raw);
        let (x, y) = pal_cell(n);
        swatch(
            buf,
            x,
            y,
            PAL_CW,
            PAL_CH,
            fill,
            if n == at { HI } else { EDGE },
        );
    }
    // A label down the left edge at each plane's colour base, because an entry's
    // number alone does not say which plane can reach it. SF1's palette is a quarter
    // of the box, so all four fit with room to spare and no paging.
    for plane in Plane::ALL {
        let (_, y) = pal_cell(plane.colour_base() as usize);
        text(buf, VX + PAD, y, plane.name(), FG);
    }
}

/// The four planes: hardware enable, mask bit, colour base and tile size.
///
/// No depth row. SF1 has no layer-order register — the drawing order is fixed in
/// silicon at background, foreground, sprites, text — and a panel that printed a
/// depth column would be inventing hardware the board does not have.
fn draw_layers(buf: &mut [u32], m: &Sf1, s: &Sf1ViewState) {
    let active = m.board.active;
    let (tx, ty) = title_at();
    text(
        buf,
        tx,
        ty,
        &format!(
            "{} GFXCTRL {:02X} FLIP {} ENTER TOGGLES MASK",
            View::Layers.name(),
            active,
            if active & ACTIVE_FLIP != 0 { 'Y' } else { '-' }
        ),
        HI,
    );
    // The order is fixed, so it is stated as a sentence rather than read off a
    // register: a reader who has just come from CPS-1's panel will look for one.
    text(buf, tx, ty + LINE, "ORDER FIXED: BG FG OB TX", FG);

    for (row, plane) in Plane::ALL.into_iter().enumerate() {
        let y = content_y() + (row + 1) * LINE;
        let enabled = active & plane.active_bit() != 0;
        let permitted = plane.permitted(&s.mask);
        let fg = if enabled { FG } else { OFF };
        if row == s.row {
            text(buf, VX + PAD, y, ">", HI);
        }
        text(
            buf,
            VX + PAD + ADVANCE,
            y,
            &format!(
                "{} BIT {:02X} {:3} MSK {:3} BASE {:03X} {:02}PX",
                plane.name(),
                plane.active_bit(),
                if enabled { "ON" } else { "OFF" },
                if permitted { "ON" } else { "OFF" },
                plane.colour_base(),
                plane.layout().width
            ),
            fg,
        );
    }

    text(
        buf,
        VX + PAD,
        content_y() + 6 * LINE,
        "MASK SUBTRACTS ONLY: IT CANNOT SHOW WHAT",
        FG,
    );
    text(
        buf,
        VX + PAD,
        content_y() + 7 * LINE,
        "THE HARDWARE HIDES",
        FG,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::{frame, panel_contains};
    use machine::video::sf1::Sf1Video;

    /// A reset SF1 with the five regions given, boxed.
    ///
    /// Boxed for the same reason `gfxpanels::tests::a_machine` is: an `Sf1` holds
    /// 0x3000 words of RAM, 0x1000 of objectram and two Z80 address spaces by
    /// value, and returning it through a wrapping fixture overflows a test thread's
    /// 2 MB stack.
    ///
    /// `machine::sf1::test_video` is `#[cfg(test)] pub(crate)` to its own crate, so
    /// this builds the video from `Sf1Video::new` directly — the same thing Task
    /// 18's `an_sf1_machine` does.
    fn a_machine(
        bg: Vec<u8>,
        fg: Vec<u8>,
        obj: Vec<u8>,
        tx: Vec<u8>,
        tilerom: Vec<u8>,
    ) -> Box<Sf1> {
        let mut prog = vec![0u8; 0x2000];
        // SSP 0x00FF8000, PC 0x00001000, then a branch to itself at 0x1000.
        prog[0..4].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00]);
        prog[4..8].copy_from_slice(&[0x00, 0x00, 0x10, 0x00]);
        prog[0x1000..0x1002].copy_from_slice(&[0x60, 0xFE]);
        let mut m = Sf1::new(
            &prog,
            Sf1Video::new(bg, fg, obj, tx, tilerom),
            vec![0x18, 0xFE],
            vec![0x00, 0x18, 0xFE],
        );
        m.reset();
        // Every plane enabled, so a view that drew nothing would not pass.
        m.board.active = 0x20 | 0x40 | 0x80 | 0x08;
        Box::new(m)
    }

    /// A region whose tile `code`'s pixel `(x, y)` has pen `(x + y + code)` masked
    /// to the layout's pen width, built forwards through the layout rule.
    ///
    /// The same shape as `gfxpanels::tests::gfx_rom`, for SF1's two layouts. The
    /// bit position is written out rather than read from the decoder, so a wrong
    /// decoder and a wrong fixture cannot agree.
    fn gfx_rom(plane: Plane, tiles: u32) -> Vec<u8> {
        let layout = plane.layout();
        let planes = layout.planes;
        let half_bits = if layout.frac_den == 2 {
            tiles as usize * layout.char_increment as usize
        } else {
            0
        };
        let bytes = tiles as usize * layout.char_increment as usize / 8 * layout.frac_den as usize
            / layout.frac_num as usize;
        let mut rom = vec![0u8; bytes];
        let x_offsets: &[u32] = if layout.width == 8 {
            &[0, 1, 2, 3, 8, 9, 10, 11]
        } else {
            &[
                0, 1, 2, 3, 8, 9, 10, 11, 256, 257, 258, 259, 264, 265, 266, 267,
            ]
        };
        for code in 0..tiles {
            for y in 0..layout.height {
                for x in 0..layout.width {
                    let pen = (x + y + code) & ((1 << planes) - 1);
                    for p in 0..planes {
                        // Plane 0 is the most significant pen bit.
                        if pen & (1 << (planes - 1 - p)) == 0 {
                            continue;
                        }
                        // Two planes per half; the second half starts at
                        // `half_bits`, which is 0 for a 1/1 layout.
                        //
                        // ⚠️ `p as usize` before the arithmetic: the plan wrote `p /
                        // 2` and `p % 2` straight into a `usize` sum, and `p` comes
                        // from `0..planes`, which is a `u32`. Every term below is a
                        // bit index into a `Vec<u8>`, so `usize` is the domain.
                        let p = p as usize;
                        let (half, within) = if half_bits == 0 {
                            (0, p)
                        } else {
                            (p / 2, p % 2)
                        };
                        let bit = half * half_bits
                            + code as usize * layout.char_increment as usize
                            + y as usize * layout.y_step as usize
                            + x_offsets[x as usize] as usize
                            + if within == 0 { 4 } else { 0 };
                        rom[bit / 8] |= 0x80u8 >> (bit % 8);
                    }
                }
            }
        }
        rom
    }

    /// A state looking at `view`, with every plane permitted.
    fn base_state(view: View) -> Sf1ViewState {
        Sf1ViewState {
            view,
            plane: Plane::Bg,
            map: MapKind::Bg,
            tile_at: 0,
            pal_at: 0,
            map_at: None,
            row: 0,
            mask: LayerMask::all(),
        }
    }

    /// The grey `gfxpanels::GREYS` draws pen `n` as, typed a second time from the
    /// channel values so the two agree only if both are right.
    fn grey_literal(pen: u8) -> u32 {
        let c = u32::from(pen) * 0x11;
        (c << 16) | (c << 8) | c
    }

    /// A swatch's interior pixel, one in from its border.
    fn swatch_fill(buf: &[u32], n: usize) -> u32 {
        let (x, y) = pal_cell(n);
        buf[(y + 1) * WIDTH + x + 1]
    }

    #[test]
    fn the_tile_grid_holds_more_small_tiles_than_large_ones() {
        // 8-pixel tiles step 9 and 16-pixel tiles step 17, in a box 351 pixels wide
        // after the four-digit row label, 208 tall below the title.
        assert_eq!(tile_grid(Plane::Tx), (39, 23));
        assert_eq!(tile_grid(Plane::Bg), (20, 12));
        assert_eq!(tile_grid(Plane::Fg), tile_grid(Plane::Bg));
        assert_eq!(tile_grid(Plane::Sprites), tile_grid(Plane::Bg));
    }

    #[test]
    fn a_tile_cell_is_inside_the_box_including_the_last_one_on_a_page() {
        for plane in Plane::ALL {
            let (cols, rows) = tile_grid(plane);
            let size = plane.layout().width as usize;
            let (x, y) = tile_cell(plane, cols * rows - 1);
            assert!(x + size <= VX + VW, "{} last cell right edge", plane.name());
            assert!(
                y + size <= VY + VH,
                "{} last cell bottom edge",
                plane.name()
            );
        }
        assert_eq!(tile_cell(Plane::Tx, 0), (29, 12));
        assert_eq!(tile_cell(Plane::Bg, 0), (29, 12));
        // 20 columns of 17, 12 rows of 17: slot 239 is the bottom right.
        assert_eq!(tile_cell(Plane::Bg, 239), (352, 199));
    }

    #[test]
    fn all_of_sf1s_palette_fits_in_a_quarter_of_the_box() {
        assert_eq!(PAL_ENTRIES, 1024);
        assert_eq!(pal_cell(0), (19, 12));
        // 64 columns of 5, so 16 rows of 4 pixels: 64 pixels tall in a 208 box.
        assert_eq!(pal_cell(PAL_ENTRIES - 1), (334, 72));
    }

    #[test]
    fn the_cursor_starts_at_the_cell_the_renderer_fetches_for_the_top_left_pixel() {
        let mut m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        // Scroll 0: `tilemap::sample` puts the visible origin at BG (4, 1) and TX
        // (8, 2) — the VISIBLE_X/VISIBLE_Y bias divided by the tile size.
        assert_eq!(map_origin(&m, MapKind::Bg), (4, 1));
        assert_eq!(map_origin(&m, MapKind::Tx), (8, 2));
        // One whole tile of background scroll moves the background cursor and
        // leaves the other two where they were.
        m.board.bgscroll = 16;
        assert_eq!(map_origin(&m, MapKind::Bg), (5, 1));
        assert_eq!(map_origin(&m, MapKind::Fg), (4, 1));
        assert_eq!(map_origin(&m, MapKind::Tx), (8, 2));
        m.board.fgscroll = 32;
        assert_eq!(map_origin(&m, MapKind::Fg), (6, 1));
    }

    #[test]
    fn the_cursor_follows_the_screen_flip() {
        let mut m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let upright = map_origin(&m, MapKind::Bg);
        m.board.active |= ACTIVE_FLIP;
        let flipped = map_origin(&m, MapKind::Bg);
        assert_ne!(upright, flipped, "flip moves the fetched cell");
        assert_eq!(flipped, (27, 14));
    }

    #[test]
    fn the_cursor_is_the_renderers_own_sampler_and_not_a_second_derivation() {
        let m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for map in MapKind::ALL {
            let s = tilemap::sample(
                map.map(),
                map.scroll(m.board.bgscroll, m.board.fgscroll),
                false,
                0,
                0,
            );
            assert_eq!(map_origin(&m, map), (s.col, s.row), "{}", map.name());
        }
    }

    #[test]
    fn the_tile_view_draws_the_pens_the_decoder_produces() {
        let m = a_machine(
            gfx_rom(Plane::Bg, 8),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let mut buf = frame();
        draw(&mut buf, &m, &base_state(View::Tiles));
        // Tile 0's pixel (0,0) has pen `(0 + 0 + 0) & 15` = 0; (1,0) has pen 1.
        let (cx, cy) = tile_cell(Plane::Bg, 0);
        assert_eq!(buf[cy * WIDTH + cx], grey_literal(0));
        assert_eq!(buf[cy * WIDTH + cx + 1], grey_literal(1));
        // Tile 1's pixel (0,0) has pen `(0 + 0 + 1) & 15` = 1.
        let (cx, cy) = tile_cell(Plane::Bg, 1);
        assert_eq!(buf[cy * WIDTH + cx], grey_literal(1));
    }

    #[test]
    fn a_code_past_the_end_of_the_region_is_one_dot_and_not_a_fill() {
        // Eight tiles held; the ninth cell is past the end.
        let m = a_machine(
            gfx_rom(Plane::Bg, 8),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let mut buf = frame();
        draw(&mut buf, &m, &base_state(View::Tiles));
        assert_eq!(m.video.gfx(Plane::Bg).elements(), 8);
        let (cx, cy) = tile_cell(Plane::Bg, 8);
        assert_eq!(buf[cy * WIDTH + cx], OFF, "one dot marks past the end");
        // And not a fill: the pixel beside it is untouched background.
        assert_eq!(buf[cy * WIDTH + cx + 1], BG);
    }

    #[test]
    fn the_tile_view_browses_the_plane_the_state_names() {
        // Only the text region is populated, so the text plane draws pens and the
        // other three draw nothing but the out-of-region dot.
        let m = a_machine(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            gfx_rom(Plane::Tx, 8),
            Vec::new(),
        );
        let mut bg = frame();
        draw(&mut bg, &m, &base_state(View::Tiles));
        let mut tx = frame();
        draw(
            &mut tx,
            &m,
            &Sf1ViewState {
                plane: Plane::Tx,
                ..base_state(View::Tiles)
            },
        );
        assert_ne!(bg, tx, "the two planes draw different pictures");
        let (cx, cy) = tile_cell(Plane::Tx, 0);
        assert_eq!(
            tx[cy * WIDTH + cx + 1],
            grey_literal(1),
            "text tile 0 pixel (1,0)"
        );
        let (cx, cy) = tile_cell(Plane::Bg, 0);
        assert_eq!(
            bg[cy * WIDTH + cx],
            OFF,
            "the empty background region is all dots"
        );
    }

    #[test]
    fn the_palette_view_shows_every_entry_through_sf1s_own_converter() {
        let mut m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        m.board.palette[0] = 0x0135;
        m.board.palette[PAL_ENTRIES - 1] = 0x0F00;
        let mut buf = frame();
        draw(&mut buf, &m, &base_state(View::Palette));
        assert_eq!(swatch_fill(&buf, 0), 0x0011_3355, "SF1 repeats the nibble");
        assert_eq!(swatch_fill(&buf, PAL_ENTRIES - 1), 0x00FF_0000);
        // CPS-1's converter would halve these; a shared one would draw 0x00112233.
        assert_ne!(swatch_fill(&buf, 0), crate::pixels::argb(0x0135));
    }

    #[test]
    fn the_cursored_swatch_is_the_only_one_highlighted() {
        let m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &Sf1ViewState {
                pal_at: 7,
                ..base_state(View::Palette)
            },
        );
        let (x, y) = pal_cell(7);
        assert_eq!(buf[y * WIDTH + x], HI, "the cursored border");
        let (x, y) = pal_cell(8);
        assert_eq!(buf[y * WIDTH + x], EDGE, "its neighbour's border");
    }

    #[test]
    fn a_palette_cursor_past_the_end_is_clamped_rather_than_panicking() {
        let m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &Sf1ViewState {
                pal_at: usize::MAX,
                ..base_state(View::Palette)
            },
        );
        let (x, y) = pal_cell(PAL_ENTRIES - 1);
        assert_eq!(buf[y * WIDTH + x], HI, "the last entry takes the cursor");
    }

    #[test]
    fn the_palette_view_names_each_planes_colour_base() {
        // The four bases are the fact a reader most needs here: an entry's number
        // alone does not say which plane can reach it.
        let m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut buf = frame();
        draw(&mut buf, &m, &base_state(View::Palette));
        for plane in Plane::ALL {
            // The label sits on the swatch row its colour base falls on, which is
            // what makes it a base rather than a legend: `pal_cell`'s y for the base
            // entry is the y the label was drawn at.
            let (_, y) = pal_cell(plane.colour_base() as usize);
            assert_eq!(
                crate::font::read_text(&buf, VX + PAD, y, 2, FG),
                plane.name(),
                "{} is labelled on its own row",
                plane.name()
            );
        }
    }

    /// A tilemap ROM whose background cell `i` holds code `i`, colour `i & 15` and
    /// no flip, and whose foreground cell `i` holds code `i + 0x100`.
    ///
    /// Built forwards from the hardware's layout — colour at `+0`, code low at `+1`,
    /// attribute at `+PLANE`, code high at `+PLANE+1`, where `PLANE` is 0x10000 —
    /// so it is not a copy of `tilerom_entry`.
    fn tilerom(cells: u32) -> Vec<u8> {
        let mut rom = vec![0u8; 0x4_0000];
        for i in 0..cells {
            let lo = 2 * i as usize;
            rom[lo] = (i & 0x0F) as u8;
            rom[lo + 1] = (i & 0xFF) as u8;
            rom[0x1_0000 + lo] = 0;
            rom[0x1_0000 + lo + 1] = (i >> 8) as u8;
            let fg = 0x2_0000 + lo;
            rom[fg] = (i & 0x0F) as u8;
            rom[fg + 1] = (i & 0xFF) as u8;
            rom[0x1_0000 + fg + 1] = ((i + 0x100) >> 8) as u8;
        }
        rom
    }

    #[test]
    fn the_tilemap_view_says_the_background_map_lives_in_rom() {
        let m = a_machine(
            gfx_rom(Plane::Bg, 8),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            tilerom(256),
        );
        let mut buf = frame();
        draw(&mut buf, &m, &base_state(View::Tilemap));
        // A reader who assumes RAM will go looking for a bug in the memory map when
        // writes never change the picture, so the word is on the panel.
        assert!(panel_contains(&buf, "IN ROM", FG), "the source is named");
        assert!(
            !panel_contains(&buf, "RAM", FG),
            "and the background is not RAM"
        );
    }

    #[test]
    fn the_text_maps_panel_says_ram_because_that_is_where_its_entries_are() {
        let m = a_machine(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            gfx_rom(Plane::Tx, 8),
            tilerom(256),
        );
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &Sf1ViewState {
                map: MapKind::Tx,
                ..base_state(View::Tilemap)
            },
        );
        assert!(
            panel_contains(&buf, "RAM VIDEORAM", FG),
            "the text map is guest RAM"
        );
    }

    #[test]
    fn the_tilemap_view_reads_the_entry_the_renderer_reads() {
        let mut m = a_machine(
            gfx_rom(Plane::Bg, 8),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            tilerom(256),
        );
        m.board.bgscroll = 0;
        let mut buf = frame();
        // Cell (4, 1) is the origin; `Scan::Cols` makes its index 4 * 16 + 1 = 65.
        draw(&mut buf, &m, &base_state(View::Tilemap));
        let info = MapKind::Bg.tile_info(m.video.tilerom(), &m.board.videoram[..], 65);
        assert_eq!(info.code, 65);
        assert_eq!(info.colour, 1);
        assert!(panel_contains(&buf, "0041", HI), "code 65 in hex");
    }

    #[test]
    fn the_two_rom_maps_show_different_byte_offsets() {
        let m = a_machine(
            gfx_rom(Plane::Bg, 8),
            gfx_rom(Plane::Fg, 8),
            Vec::new(),
            Vec::new(),
            tilerom(256),
        );
        let mut bg = frame();
        draw(&mut bg, &m, &base_state(View::Tilemap));
        let mut fg = frame();
        draw(
            &mut fg,
            &m,
            &Sf1ViewState {
                map: MapKind::Fg,
                ..base_state(View::Tilemap)
            },
        );
        assert_ne!(bg, fg, "the two maps draw different pictures");
        // 0 and 0x20000, plus twice the cell index: cell 65 is at 0x82 and 0x20082.
        assert!(
            panel_contains(&bg, "00082", FG),
            "background cell 65's byte offset"
        );
        assert!(
            panel_contains(&fg, "20082", FG),
            "foreground cell 65's byte offset"
        );
    }

    #[test]
    fn the_window_wraps_at_each_maps_own_edge_and_not_at_a_shared_constant() {
        // BG is 2048 columns by 16 rows and TX is 64 by 32. A 64×64 wrap would draw
        // rows 16-63 of a map that has sixteen.
        let m = a_machine(
            gfx_rom(Plane::Bg, 8),
            Vec::new(),
            Vec::new(),
            gfx_rom(Plane::Tx, 8),
            tilerom(256),
        );
        for (map, cols, rows) in [(MapKind::Bg, 2048u32, 16u32), (MapKind::Tx, 64, 32)] {
            let mut buf = frame();
            let s = Sf1ViewState {
                map,
                map_at: Some((0, 0)),
                ..base_state(View::Tilemap)
            };
            draw(&mut buf, &m, &s);
            assert_eq!((map.map().cols, map.map().rows), (cols, rows));
        }
        // The row labels of a three-cells-back cursor on BG: rows 13, 14, 15, 0, …
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &Sf1ViewState {
                map_at: Some((0, 0)),
                ..base_state(View::Tilemap)
            },
        );
        assert!(
            panel_contains(&buf, "0013", FG),
            "row 13 is the map's own last-but-two"
        );
        // ⚠️ And row 16 is not drawn. The plan's version of this test asserted only
        // `map().cols`/`rows`, which are `video`'s constants rather than anything
        // this panel does: a `% 64` in the row loop passed it while labelling rows
        // 16-20 of a map that has sixteen. That mutant fails here.
        //
        // 0x0016 is 22, which is background cell (1, 6) — a row this window does not
        // show — so the string can only come from a row label.
        assert!(
            !panel_contains(&buf, "0016", FG),
            "row 16 is past the background map's sixteen rows"
        );

        // And the columns wrap at 2,048, which needs a map whose far cells hold
        // something: with only 256 cells filled, column 2045 and a wrong column 61
        // both read code 0 and no assertion on the codes can tell them apart. That
        // is why the fixture below fills every background cell.
        let full = a_machine(
            gfx_rom(Plane::Bg, 8),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            tilerom(0x8000),
        );
        let mut buf = frame();
        draw(
            &mut buf,
            &full,
            &Sf1ViewState {
                map_at: Some((0, 0)),
                ..base_state(View::Tilemap)
            },
        );
        // `Scan::Cols`: the window's first column is 2045 and its first row 13, so
        // its top-left cell is index 2045 * 16 + 13 = 32,733 = 0x7FDD, and
        // `tilerom` puts code `i` in cell `i`. A 64-column wrap would show column
        // 61, whose cell 13 is index 989 = 0x03DD.
        assert!(
            panel_contains(&buf, "7FDD", FG),
            "column 2045 is the map's own last-but-two"
        );
        assert!(
            !panel_contains(&buf, "03DD", FG),
            "and column 61 is not, which a 64-column wrap would show"
        );
    }

    #[test]
    fn the_cursored_tile_is_previewed_beside_the_window() {
        let m = a_machine(
            gfx_rom(Plane::Bg, 8),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            tilerom(256),
        );
        let mut buf = frame();
        // Cell 0 of the background holds code 0, whose pixel (1,0) has pen 1.
        draw(
            &mut buf,
            &m,
            &Sf1ViewState {
                map_at: Some((0, 0)),
                ..base_state(View::Tilemap)
            },
        );
        let px = VX + PAD + MAP_LABEL * ADVANCE + MAP_COLS * MAP_CELL * ADVANCE + ADVANCE;
        let py = content_y() + 4 * LINE;
        assert_eq!(
            buf[py * WIDTH + px + 1],
            grey_literal(1),
            "the preview's pen"
        );
    }

    #[test]
    fn a_code_the_region_does_not_hold_is_named_rather_than_drawn_blank() {
        // Cell 65 holds code 65; a region of eight elements does not hold it.
        let m = a_machine(
            gfx_rom(Plane::Bg, 8),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            tilerom(256),
        );
        let mut buf = frame();
        draw(&mut buf, &m, &base_state(View::Tilemap));
        assert!(
            panel_contains(&buf, "OFF ROM", OFF),
            "the preview says why it is empty"
        );
    }

    #[test]
    fn the_layers_view_shows_all_four_planes_and_no_depth_row() {
        let m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut buf = frame();
        draw(&mut buf, &m, &base_state(View::Layers));
        for plane in Plane::ALL {
            assert!(
                panel_contains(&buf, plane.name(), FG),
                "{} has a row",
                plane.name()
            );
        }
        // SF1 has no layer-order register, so there is no depth row to draw and
        // printing one would invent hardware.
        assert!(
            !panel_contains(&buf, "DEPTH", FG),
            "no depth row on this board"
        );
    }

    #[test]
    fn a_plane_the_hardware_disables_reads_off_and_in_the_warning_colour() {
        let mut m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut on = frame();
        draw(&mut on, &m, &base_state(View::Layers));
        m.board.active &= !0x20; // background off
        let mut off = frame();
        draw(&mut off, &m, &base_state(View::Layers));
        assert_ne!(on, off, "the background's row changed");
        assert!(off.contains(&OFF), "and it is drawn in the warning colour");
    }

    #[test]
    fn the_mask_column_is_the_masks_own_field_for_each_plane() {
        let m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut all = frame();
        draw(&mut all, &m, &base_state(View::Layers));
        for (n, field) in [(0usize, "bg"), (1, "fg"), (2, "sprites"), (3, "tx")] {
            let mut mask = LayerMask::all();
            match n {
                0 => mask.bg = false,
                1 => mask.fg = false,
                2 => mask.sprites = false,
                _ => mask.tx = false,
            }
            let mut one = frame();
            draw(
                &mut one,
                &m,
                &Sf1ViewState {
                    mask,
                    ..base_state(View::Layers)
                },
            );
            assert_ne!(all, one, "clearing {field} changed the picture");
        }
    }

    #[test]
    fn the_cursored_row_is_marked_and_only_one_row_is() {
        let m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut a = frame();
        draw(
            &mut a,
            &m,
            &Sf1ViewState {
                row: 0,
                ..base_state(View::Layers)
            },
        );
        let mut b = frame();
        draw(
            &mut b,
            &m,
            &Sf1ViewState {
                row: 3,
                ..base_state(View::Layers)
            },
        );
        assert_ne!(a, b, "the cursor moved");
        assert!(panel_contains(&a, ">", HI), "and it is drawn");
    }

    #[test]
    fn the_layers_view_says_a_mask_can_only_subtract() {
        let m = a_machine(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut buf = frame();
        draw(&mut buf, &m, &base_state(View::Layers));
        assert!(
            panel_contains(&buf, "MASK SUBTRACTS ONLY", FG),
            "the same warning CPS-1 carries"
        );
    }

    #[test]
    fn every_view_stays_inside_its_box() {
        let m = a_machine(
            gfx_rom(Plane::Bg, 8),
            gfx_rom(Plane::Fg, 8),
            gfx_rom(Plane::Sprites, 8),
            gfx_rom(Plane::Tx, 8),
            tilerom(256),
        );
        let mut view = View::Tiles;
        for _ in 0..4 {
            let mut buf = frame();
            draw(&mut buf, &m, &base_state(view));
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let inside = (VX..VX + VW).contains(&x) && (VY..VY + VH).contains(&y);
                    if !inside {
                        assert_eq!(buf[y * WIDTH + x], 0, "{view:?} drew at ({x},{y})");
                    }
                }
            }
            view = view.cycled();
        }
    }

    #[test]
    fn every_view_draws_something() {
        let m = a_machine(
            gfx_rom(Plane::Bg, 8),
            gfx_rom(Plane::Fg, 8),
            gfx_rom(Plane::Sprites, 8),
            gfx_rom(Plane::Tx, 8),
            tilerom(256),
        );
        let mut view = View::Tiles;
        for _ in 0..4 {
            let mut buf = frame();
            draw(&mut buf, &m, &base_state(view));
            assert!(
                buf.iter().any(|&w| w != 0 && w != BG),
                "{view:?} drew content"
            );
            view = view.cycled();
        }
    }

    #[test]
    fn drawing_a_view_does_not_disturb_the_machine() {
        // `&Sf1` is the compiler's half of this; the assertion is the other half,
        // because a panel could still drive the machine through an interior-mutable
        // field or a `&mut` obtained from a `Box`.
        let mut m = a_machine(
            gfx_rom(Plane::Bg, 8),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            tilerom(256),
        );
        m.run_frame();
        let before = (m.total_cycles, m.cpu.pc, m.board.active, m.board.bgscroll);
        let mut view = View::Tiles;
        for _ in 0..4 {
            let mut buf = frame();
            draw(&mut buf, &m, &base_state(view));
            view = view.cycled();
        }
        assert_eq!(
            before,
            (m.total_cycles, m.cpu.pc, m.board.active, m.board.bgscroll)
        );
    }
}
