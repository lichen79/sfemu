//! The three tilemaps: two 16×16 scrolling planes from ROM, one 8×8 from RAM.
//!
//! `sf.cpp:754-762` creates them:
//!
//! ```text
//! bg: TILEMAP_SCAN_COLS, 16,16, 2048,16   from m_tilerom[0x00000]
//! fg: TILEMAP_SCAN_COLS, 16,16, 2048,16   from m_tilerom[0x20000]
//! tx: TILEMAP_SCAN_ROWS,  8, 8,   64,32   from m_videoram
//! ```
//!
//! # Why this is not [`crate::layers`]
//!
//! CPS-1's tilemaps come from a scroll-RAM window whose base and size the CPS-B
//! registers choose, in three fixed sizes, with a per-tile priority bit feeding a
//! priority mask. SF1's geometries are fixed, unprioritised and scroll in X only,
//! and its two large maps are addressed out of a **ROM** region laid out as two
//! byte planes 0x10000 apart rather than as words. Only the window offset
//! survives the trip.
//!
//! # The sampling transform
//!
//! [`draw`] walks screen pixels and samples the tilemap, where MAME walks
//! tilemap instances and blits them. The two agree, and the derivation is worth
//! writing down because MAME spreads the answer across four functions.
//!
//! For a screen pixel, let `rx = VISIBLE_X + sx` and `ry = VISIBLE_Y + sy` be
//! the raster coordinate. `draw_common` places instances at
//! `xpos = scrollx - m_width + k * m_width` (`tilemap.cpp:1018-1020`), and
//! `draw_instance` samples the pixmap at `rx - xpos`, so the sampled pixmap x is
//! `(rx - scrollx) mod m_width` — the instance loop is only there to make the
//! modulo happen inside a blitter.
//!
//! `scrollx` is `effective_rowscroll(0, xextent)` (`tilemap.cpp:27-46`), which
//! with `m_dx == m_dx_flipped == 0` — `sf.cpp` never sets either — is:
//!
//! ```text
//! unflipped:  scrollx = (-scroll) mod m_width
//! flipped:    scrollx = (xextent - m_width + scroll) mod m_width
//! ```
//!
//! Substituting, the sampled **logical** pixmap x is `(rx + scroll) mod m_width`
//! unflipped — note the scroll **adds** — and
//! `(rx - xextent + m_width - scroll) mod m_width` flipped.
//!
//! Flip also reverses the tile grid (`mappings_update`, `tilemap.cpp:727-731`,
//! maps logical cell `(c, r)` to memory cell `(cols-1-c, rows-1-r)`) and xors
//! every tile's flip flags (`tile_update`, `tilemap.cpp:805`). Together those two
//! make logical pixmap pixel `X` equal **memory** pixmap pixel
//! `m_width - 1 - X`: the grid reversal handles the tile index and the flag xor
//! handles the pixel within it, and `m_width = cols * tilewidth` makes the two
//! compose exactly. Substituting again, the memory pixmap x this module wants is
//!
//! ```text
//! unflipped:  (rx + scroll)                mod m_width
//! flipped:    (xextent - 1 - rx + scroll)  mod m_width
//! ```
//!
//! which is one mirror of the raster coordinate about the visible area, before
//! the scroll. The same argument in y gives `ry` and `yextent - 1 - ry`, with no
//! scroll term because `sf.cpp` never calls `set_scrolly`.
//!
//! So this module implements the mirror and not the three parts. The parts are
//! individually invisible — a mirror about the *map's* centre rather than the
//! window's differs by `xextent - m_width`, which is **zero for the text map**
//! and 32,256 pixels for the scrolling planes.
//!
//! # Transparency is per-map
//!
//! `sf.cpp:764-765` sets pen 15 transparent on the foreground and pen 3 on the
//! text map. It never calls `set_transparent_pen` on the **background**, so a
//! background pen 15 is drawn. (MAME reaches this through the pixmap's flags
//! plane — a pixel draws iff `(flags & (0x0f | 0x10)) == 0x10` — which for a map
//! with one transparent pen and no per-tile category is exactly a pen
//! comparison.) [`draw`] takes the pen as a parameter and `None` means opaque.

use super::gfx::GfxSet;
use crate::{HEIGHT, VISIBLE_X, VISIBLE_Y, WIDTH};

/// `TILE_FLIPX`, `tilemap.h:39`.
pub const FLIP_X: u8 = 0x01;
/// `TILE_FLIPY`, `tilemap.h:40`.
pub const FLIP_Y: u8 = 0x02;

/// `xextent`: `visarea.left() + visarea.right() + 1` (`tilemap.cpp:1010`).
///
/// `sf.cpp:770` is `set_visarea(8*8, (64-8)*8-1, 2*8, 30*8-1)`, so the visible
/// area is (64, 447) × (16, 239) and this is 512. Written as the crate's window
/// plus twice its offset, because that is the identity screen flip depends on:
/// the mirror is about the window's centre, so it must be built from the window.
pub const X_EXTENT: i32 = 2 * VISIBLE_X + WIDTH as i32;

/// `yextent`: `visarea.top() + visarea.bottom() + 1` (`tilemap.cpp:1011`). 256.
pub const Y_EXTENT: i32 = 2 * VISIBLE_Y + HEIGHT as i32;

/// What a tile-info callback returns — MAME's
/// `tileinfo.set(gfx, code, color, flags)` less the `gfx` index, which the caller
/// has already chosen by picking a [`GfxSet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TileInfo {
    /// Element index into the graphics region.
    pub code: u32,
    /// Colour index, 0-15.
    pub colour: u16,
    /// [`FLIP_X`] and/or [`FLIP_Y`].
    pub flags: u8,
}

/// Which way a tilemap's memory walks its grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scan {
    /// `TILEMAP_SCAN_COLS` (`tilemap.h:123`): down a column, then across.
    Cols,
    /// `TILEMAP_SCAN_ROWS` (`tilemap.h:119`): across a row, then down.
    Rows,
}

impl Scan {
    /// The memory index of grid cell `(col, row)`.
    #[must_use]
    pub const fn index(&self, col: u32, row: u32, cols: u32, rows: u32) -> u32 {
        match self {
            Self::Cols => col * rows + row,
            Self::Rows => row * cols + col,
        }
    }
}

/// A tilemap's geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tilemap {
    /// Memory order.
    pub scan: Scan,
    /// Tile width in pixels.
    pub tile_w: u32,
    /// Tile height in pixels.
    pub tile_h: u32,
    /// Tiles across.
    pub cols: u32,
    /// Tiles down.
    pub rows: u32,
}

impl Tilemap {
    /// Pixels across — MAME's `m_width`.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.cols * self.tile_w
    }

    /// Pixels down — MAME's `m_height`.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.rows * self.tile_h
    }

    /// Grid cells, which is also the highest index a callback will be handed
    /// plus one.
    #[must_use]
    pub const fn tiles(&self) -> u32 {
        self.cols * self.rows
    }
}

/// The background plane: 2048×16 tiles of 16×16, 32,768 pixels wide.
pub const BG: Tilemap = Tilemap {
    scan: Scan::Cols,
    tile_w: 16,
    tile_h: 16,
    cols: 2048,
    rows: 16,
};

/// The foreground plane — the same geometry, a different tilerom window.
pub const FG: Tilemap = Tilemap {
    scan: Scan::Cols,
    tile_w: 16,
    tile_h: 16,
    cols: 2048,
    rows: 16,
};

/// The text plane: 64×32 tiles of 8×8, which is exactly videoram.
pub const TX: Tilemap = Tilemap {
    scan: Scan::Rows,
    tile_w: 8,
    tile_h: 8,
    cols: 64,
    rows: 32,
};

/// The tilerom's plane separation: attr and code-high live 0x10000 above
/// colour and code-low (`sf.cpp:735-737`).
const PLANE: usize = 0x1_0000;

/// The foreground's window into the tilerom (`sf.cpp:743`).
const FG_BASE: usize = 0x2_0000;

/// One entry of a tilerom plane pair, with a short region reading as zero.
///
/// The index derives from a guest-written scroll register, so the bounds check is
/// not optional; reading a short region as blank also lets a caller build a
/// machine with no tilerom at all.
fn tilerom_entry(tilerom: &[u8], base: usize, index: u32) -> TileInfo {
    let Some(low) = base.checked_add(2 * (index as usize)) else {
        return TileInfo::default();
    };
    let byte = |o: usize| {
        low.checked_add(o)
            .and_then(|i| tilerom.get(i))
            .copied()
            .unwrap_or(0)
    };
    let colour = byte(0);
    let code_low = byte(1);
    let attr = byte(PLANE);
    let code_high = byte(PLANE + 1);
    TileInfo {
        code: (u32::from(code_high) << 8) | u32::from(code_low),
        colour: u16::from(colour),
        // `TILE_FLIPYX(attr & 3)` — `tilemap.h:44`, `u8(yx & 3)`; bit 0 is X.
        flags: attr & 0x03,
    }
}

/// `get_bg_tile_info`, `sf.cpp:731-738`.
#[must_use]
pub fn bg_tile_info(tilerom: &[u8], index: u32) -> TileInfo {
    tilerom_entry(tilerom, 0, index)
}

/// `get_fg_tile_info`, `sf.cpp:740-747`.
#[must_use]
pub fn fg_tile_info(tilerom: &[u8], index: u32) -> TileInfo {
    tilerom_entry(tilerom, FG_BASE, index)
}

/// `get_tx_tile_info`, `sf.cpp:749-752`.
///
/// One word: code `& 0x3ff`, colour `>> 12`, flip `(word & 0xc00) >> 10`. The
/// colour needs no mask — a `u16 >> 12` is already 0-15.
#[must_use]
pub fn tx_tile_info(videoram: &[u16], index: u32) -> TileInfo {
    let word = videoram.get(index as usize).copied().unwrap_or(0);
    TileInfo {
        code: u32::from(word & 0x03FF),
        colour: word >> 12,
        flags: ((word & 0x0C00) >> 10) as u8,
    }
}

/// One of SF1's three tilemaps.
///
/// Each map is five decisions the renderer makes together — which geometry, which
/// scroll register, which transparent pen, which entry decoder, and whether the
/// entries live in ROM or in guest RAM. `sf.cpp` makes them at three separate call
/// sites in `draw_common`. Naming them here lets a graphics viewer ask the same
/// questions and get the same answers, rather than a fourth reading of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapKind {
    /// The background map: 2,048 × 16 sixteen-pixel tiles, entries in ROM at 0.
    Bg,
    /// The foreground map: 2,048 × 16 sixteen-pixel tiles, entries in ROM at
    /// 0x20000 — read it from [`MapKind::tilerom_base`] rather than typing it.
    ///
    /// ⚠️ Written as a number rather than as a link to the `FG_BASE` constant it
    /// comes from: that constant is private, deliberately — a panel that read it
    /// would be re-deriving `tilerom_entry`'s layout — and `cargo doc` refuses a
    /// public link to a private item.
    Fg,
    /// The text map: 64 × 32 eight-pixel tiles, entries in guest `videoram`.
    Tx,
}

impl MapKind {
    /// Every map, in drawing order.
    pub const ALL: [MapKind; 3] = [MapKind::Bg, MapKind::Fg, MapKind::Tx];

    /// The plane this map draws into — its region, layout and colour base.
    #[must_use]
    pub const fn plane(self) -> crate::sf1::Plane {
        match self {
            Self::Bg => crate::sf1::Plane::Bg,
            Self::Fg => crate::sf1::Plane::Fg,
            Self::Tx => crate::sf1::Plane::Tx,
        }
    }

    /// This map's geometry.
    #[must_use]
    pub const fn map(self) -> &'static Tilemap {
        match self {
            Self::Bg => &BG,
            Self::Fg => &FG,
            Self::Tx => &TX,
        }
    }

    /// This map's horizontal scroll, from the board's two registers.
    ///
    /// The text layer has no scroll register at all — `sf.cpp` never gives its
    /// tilemap a scroll — so it is a literal zero rather than a third field.
    #[must_use]
    pub const fn scroll(self, bgscroll: u16, fgscroll: u16) -> u32 {
        match self {
            Self::Bg => bgscroll as u32,
            Self::Fg => fgscroll as u32,
            Self::Tx => 0,
        }
    }

    /// The pen this map treats as a hole, if any.
    ///
    /// `None` for the background: `draw_common` never calls
    /// `set_transparent_pen` on it, so pen 15 draws.
    #[must_use]
    pub const fn transparent_pen(self) -> Option<u8> {
        match self {
            Self::Bg => None,
            Self::Fg => Some(15),
            Self::Tx => Some(3),
        }
    }

    /// The byte offset in the tilemap ROM where this map's entries begin, or
    /// `None` for a map whose entries live in guest RAM.
    ///
    /// A panel prints this so a reader can see *why* writing to RAM never changes
    /// the background: there is a ROM offset here and no RAM address at all.
    #[must_use]
    pub const fn tilerom_base(self) -> Option<usize> {
        match self {
            Self::Bg => Some(0),
            Self::Fg => Some(FG_BASE),
            Self::Tx => None,
        }
    }

    /// This map's entry at `index`.
    ///
    /// Takes both sources because which one it reads is the thing being named:
    /// two of the three maps ignore `videoram` and the third ignores `tilerom`.
    #[must_use]
    pub fn tile_info(self, tilerom: &[u8], videoram: &[u16], index: u32) -> TileInfo {
        match self {
            Self::Bg => bg_tile_info(tilerom, index),
            Self::Fg => fg_tile_info(tilerom, index),
            Self::Tx => tx_tile_info(videoram, index),
        }
    }

    /// A two-character label, for a panel with 4 pixels per character.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bg => "BG",
            Self::Fg => "FG",
            Self::Tx => "TX",
        }
    }

    /// The next map, wrapping. [`MapKind::ALL`]'s order.
    #[must_use]
    pub const fn cycled(self) -> Self {
        match self {
            Self::Bg => Self::Fg,
            Self::Fg => Self::Tx,
            Self::Tx => Self::Bg,
        }
    }
}

/// Which map cell, and which pixel of it, a screen pixel reads.
///
/// `col`/`row` index the map; `x`/`y` are the pixel's position inside the tile
/// *before* the tile's own flip flags are applied, because the flags come from the
/// map entry this sample names and cannot be known until it has been fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Map column, `0..map.cols`.
    pub col: u32,
    /// Map row, `0..map.rows`.
    pub row: u32,
    /// Pixel column within the tile, `0..map.tile_w`.
    pub x: u32,
    /// Pixel row within the tile, `0..map.tile_h`.
    pub y: u32,
}

/// Which map cell and tile pixel screen pixel `(sx, sy)` reads.
///
/// Published because a graphics viewer must name the cell the renderer fetched,
/// and five decisions live in these six lines: the `VISIBLE_X`/`VISIBLE_Y` bias,
/// the mirror about `X_EXTENT`/`Y_EXTENT` under flip, `rem_euclid` on both axes,
/// the scroll added after the mirror rather than before it, and the scroll's
/// reduction modulo the map's own width. A panel with a sixth reading of any of
/// them would report a cell that was never drawn, which is a diagnostic that lies
/// exactly when it is being trusted — the argument [`crate::layers::map_axis`]
/// already makes for CPS-1.
///
/// A degenerate map — `cols` or `rows` zero, which `Tilemap`'s public fields
/// permit and this module's constants never produce — samples the origin rather
/// than dividing by zero.
#[must_use]
pub fn sample(map: &Tilemap, scroll_x: u32, flip: bool, sx: usize, sy: usize) -> Sample {
    let (mw, mh) = (map.width() as i32, map.height() as i32);
    if mw == 0 || mh == 0 {
        return Sample {
            col: 0,
            row: 0,
            x: 0,
            y: 0,
        };
    }
    // `u16` on the hardware, so this cannot overflow the i32 sums below.
    let scroll = (scroll_x % map.width()) as i32;
    let ry = VISIBLE_Y + sy as i32;
    let my = if flip { Y_EXTENT - 1 - ry } else { ry }.rem_euclid(mh);
    let rx = VISIBLE_X + sx as i32;
    let base = if flip { X_EXTENT - 1 - rx } else { rx };
    let mx = (base + scroll).rem_euclid(mw);
    Sample {
        col: (mx / map.tile_w as i32) as u32,
        row: (my / map.tile_h as i32) as u32,
        x: (mx % map.tile_w as i32) as u32,
        y: (my % map.tile_h as i32) as u32,
    }
}

/// Draw a tilemap into the 384×224 pen buffer.
///
/// `scroll_x` is the register's value as written; `flip` is `flip_screen()`.
/// Wraparound is implicit in the modulo — see the module documentation for the
/// derivation from `draw_common`'s instance loop, and for why the scroll adds.
///
/// `transparent_pen` of `None` draws every pen, including the highest.
///
/// # Panics
///
/// If `dst.len()` is not `WIDTH * HEIGHT`. That is a programming error in this
/// crate, not anything a guest can cause — the same guard
/// [`crate::layers::draw_tilemap`] uses.
pub fn draw(
    dst: &mut [u16],
    gfx: &GfxSet<'_>,
    map: &Tilemap,
    info: impl Fn(u32) -> TileInfo,
    scroll_x: u32,
    flip: bool,
    transparent_pen: Option<u8>,
) {
    assert_eq!(
        dst.len(),
        WIDTH * HEIGHT,
        "destination must be WIDTH * HEIGHT"
    );
    // Not constructible from this module's constants, but `Tilemap` is public and
    // `sample` would return the origin for every pixel rather than the map.
    if map.width() == 0 || map.height() == 0 {
        return;
    }
    for sy in 0..HEIGHT {
        for sx in 0..WIDTH {
            let s = sample(map, scroll_x, flip, sx, sy);
            let tile = info(map.scan.index(s.col, s.row, map.cols, map.rows));
            // Flip mirrors within the tile; the tile's screen position is fixed.
            let px = if tile.flags & FLIP_X != 0 {
                map.tile_w - 1 - s.x
            } else {
                s.x
            };
            let py = if tile.flags & FLIP_Y != 0 {
                map.tile_h - 1 - s.y
            } else {
                s.y
            };
            let Some(pen) = gfx.pen(tile.code, px, py) else {
                continue;
            };
            if Some(pen) == transparent_pen {
                continue;
            }
            dst[sy * WIDTH + sx] = gfx.palette_base(tile.colour) + u16::from(pen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sf1::gfx::{CHAR_LAYOUT, SPRITE_LAYOUT};
    use crate::{HEIGHT, VISIBLE_X, VISIBLE_Y, WIDTH};

    /// The three tilemap geometries, against `sf.cpp:754-762`.
    #[test]
    fn the_three_tilemaps_are_mames_geometries() {
        assert_eq!(BG.scan, Scan::Cols);
        assert_eq!((BG.tile_w, BG.tile_h, BG.cols, BG.rows), (16, 16, 2048, 16));
        assert_eq!((BG.width(), BG.height()), (32_768, 256));
        assert_eq!(FG.scan, Scan::Cols);
        assert_eq!((FG.tile_w, FG.tile_h, FG.cols, FG.rows), (16, 16, 2048, 16));
        assert_eq!(TX.scan, Scan::Rows);
        assert_eq!((TX.tile_w, TX.tile_h, TX.cols, TX.rows), (8, 8, 64, 32));
        assert_eq!((TX.width(), TX.height()), (512, 256));
        // The text map is exactly videoram: 64*32 = 2,048 words = 0x1000 bytes,
        // which is the 0x800000-0x800fff window.
        assert_eq!(TX.tiles(), 2_048);
        assert_eq!(TX.tiles() * 2, 0x1000);
        assert_eq!(BG.tiles(), 32_768);
    }

    /// The visible extents, and the reason the vertical scroll is always zero.
    ///
    /// `xextent = visarea.right() + visarea.left() + 1` and likewise for y
    /// (`tilemap.cpp:1010-1011`). `sf.cpp:770` is
    /// `set_visarea(8*8, (64-8)*8-1, 2*8, 30*8-1)` = (64, 447, 16, 239), so
    /// xextent is 512 and yextent 256 — and both are the crate's window plus
    /// twice its offset, which is where this module gets them from.
    ///
    /// All three maps are 256 pixels tall, and `effective_colscroll`'s flipped
    /// branch is `yextent - m_height - (m_dy_flipped - 0)`. With yextent 256,
    /// m_height 256 and `m_dy_flipped` 0, that is **zero** — the same as the
    /// unflipped branch. So neither flip state introduces a vertical scroll, and
    /// [`draw`] has no y-scroll parameter at all.
    #[test]
    fn the_extents_come_from_the_crate_and_leave_no_vertical_scroll() {
        assert_eq!(X_EXTENT, 512);
        assert_eq!(Y_EXTENT, 256);
        assert_eq!(X_EXTENT, 2 * VISIBLE_X + WIDTH as i32);
        assert_eq!(Y_EXTENT, 2 * VISIBLE_Y + HEIGHT as i32);
        for map in [BG, FG, TX] {
            assert_eq!(map.height(), Y_EXTENT as u32, "yextent - m_height == 0");
        }
    }

    /// `TILEMAP_SCAN_COLS` walks down a column; `TILEMAP_SCAN_ROWS` across a row.
    ///
    /// `tilemap.h:118-127`: COLS is `col * num_rows + row`, ROWS is
    /// `row * num_cols + col`. Swapping them scrambles both scrolling planes
    /// while leaving the text layer looking right — so both are asserted.
    #[test]
    fn the_two_scan_orders_index_the_way_mame_does() {
        assert_eq!(Scan::Cols.index(0, 0, 2048, 16), 0);
        assert_eq!(Scan::Cols.index(0, 1, 2048, 16), 1, "down first");
        assert_eq!(Scan::Cols.index(1, 0, 2048, 16), 16, "then across");
        assert_eq!(
            Scan::Cols.index(2047, 15, 2048, 16),
            32_767,
            "the last tile"
        );
        assert_eq!(Scan::Rows.index(0, 0, 64, 32), 0);
        assert_eq!(Scan::Rows.index(1, 0, 64, 32), 1, "across first");
        assert_eq!(Scan::Rows.index(0, 1, 64, 32), 64, "then down");
        assert_eq!(Scan::Rows.index(63, 31, 64, 32), 2_047);
    }

    /// The background tile-info callback, byte for byte (`sf.cpp:731-738`).
    ///
    /// `base = &m_tilerom[2 * tile_index]`, then `attr = base[0x10000]`,
    /// `color = base[0]`, `code = (base[0x10000 + 1] << 8) | base[1]`.
    ///
    /// So the tilerom is **two planes 0x10000 apart**: the low plane holds
    /// (colour, code-low) pairs and the high plane (attr, code-high) pairs.
    #[test]
    fn the_background_callback_reads_two_planes_sixty_four_k_apart() {
        let mut rom = vec![0u8; 0x4_0000];
        // tile_index 3 -> base = 6.
        rom[6] = 0x0A; // colour
        rom[7] = 0x34; // code low
        rom[0x1_0006] = 0x03; // attr: flip both
        rom[0x1_0007] = 0x12; // code high
        let info = bg_tile_info(&rom, 3);
        assert_eq!(info.colour, 0x0A);
        assert_eq!(info.code, 0x1234, "high byte from the far plane");
        assert_eq!(info.flags, FLIP_X | FLIP_Y);
        // `TILE_FLIPYX(attr & 3)` — only the low two bits, and x is bit 0.
        rom[0x1_0006] = 0xFD; // 0xfd & 3 == 1
        assert_eq!(bg_tile_info(&rom, 3).flags, FLIP_X);
        rom[0x1_0006] = 0xFE; // & 3 == 2
        assert_eq!(bg_tile_info(&rom, 3).flags, FLIP_Y);
    }

    /// The foreground callback is the same shape, shifted 0x20000 (`sf.cpp:740-747`).
    #[test]
    fn the_foreground_callback_reads_the_same_planes_at_a_two_hundred_k_offset() {
        let mut rom = vec![0u8; 0x4_0000];
        rom[0x2_0006] = 0x0B;
        rom[0x2_0007] = 0x78;
        rom[0x3_0006] = 0x02;
        rom[0x3_0007] = 0x56;
        let info = fg_tile_info(&rom, 3);
        assert_eq!(info.colour, 0x0B);
        assert_eq!(info.code, 0x5678);
        assert_eq!(info.flags, FLIP_Y);
        // The background of the same index is untouched — the offset is real.
        assert_eq!(bg_tile_info(&rom, 3), TileInfo::default());
    }

    /// A tilerom shorter than the offsets asked of it reads as blank, not a panic.
    ///
    /// The index derives from a guest-written scroll register, so it reaches
    /// 32,767 with a real ROM and any value with a truncated or absent one.
    #[test]
    fn a_short_tilerom_reads_as_blank_and_never_panics() {
        assert_eq!(bg_tile_info(&[], 0), TileInfo::default());
        assert_eq!(fg_tile_info(&[], 0), TileInfo::default());
        let rom = vec![0xFFu8; 0x10];
        // The far plane is missing, so attr and code-high read 0 while colour
        // and code-low come through.
        assert_eq!(
            bg_tile_info(&rom, 3),
            TileInfo {
                code: 0x00FF,
                colour: 0xFF,
                flags: 0
            }
        );
        assert_eq!(bg_tile_info(&rom, u32::MAX), TileInfo::default());
    }

    /// The text callback: one videoram word carries all three fields.
    ///
    /// `sf.cpp:749-752`: `code = m_videoram[tile_index]`, then
    /// `set(3, code & 0x3ff, code >> 12, TILE_FLIPYX((code & 0xc00) >> 10))`.
    /// Colour is the **top** nibble and needs no mask: a `u16 >> 12` is 0-15.
    #[test]
    fn the_text_callback_unpacks_one_word_into_code_colour_and_flip() {
        let ram = vec![0x0000u16, 0xCFFF, 0x5000, 0xF400];
        assert_eq!(tx_tile_info(&ram, 0), TileInfo::default());
        // 0xcfff, bit by bit: 0xfff & 0x3ff = 0x3ff, so the code mask drops the
        // two flip bits rather than folding them in; 0xc >> 12 is the colour; and
        // (0xcfff & 0xc00) >> 10 == 3 is both flip flags.
        assert_eq!(
            tx_tile_info(&ram, 1),
            TileInfo {
                code: 0x3FF,
                colour: 0xC,
                flags: FLIP_X | FLIP_Y
            },
            "0xcfff: code 0x3ff, colour 0xc, both flip bits"
        );
        assert_eq!(
            tx_tile_info(&ram, 2),
            TileInfo {
                code: 0,
                colour: 5,
                flags: 0
            }
        );
        assert_eq!(
            tx_tile_info(&ram, 3),
            TileInfo {
                code: 0,
                colour: 0xF,
                flags: FLIP_X
            },
            "0xf400: bit 10 is flip X"
        );
        assert_eq!(tx_tile_info(&ram, 4), TileInfo::default(), "past the end");
    }

    /// A 16-pixel map at scroll 0 puts tile pixel (0,0) at screen (0,0).
    ///
    /// Screen x 0 is raster x `VISIBLE_X` = 64, and 64 mod 16 is 0; screen y 0 is
    /// raster y 16, and 16 mod 16 is 0. Both offsets being multiples of 16 is why
    /// this comes out round — it is a property of the window, not a coincidence
    /// worth relying on elsewhere.
    ///
    /// ⚠️ Two things about `rom[64] = 0xF0` that a whole-byte fixture gets wrong.
    /// Byte 64 is bit 512, which is the sprite layout's `RGN_FRAC(1,2)` — so its
    /// **high** nibble is plane 3 (offset `half + 0`) at tile x 0..3 and its
    /// **low** nibble is plane 2 (offset `half + 4`) at the same four pixels. A
    /// `0xFF` there would light both planes and give pen 3, not pen 1. And the
    /// four pixels are x 0..3, not 0..7: the x-offsets are 0,1,2,3 then **8**,
    /// 9,10,11, so x 4..7 read byte 65.
    #[test]
    fn a_tile_lands_at_the_origin_when_the_scroll_is_zero() {
        let map = one_tile_16();
        let mut rom = vec![0u8; 128];
        rom[64] = 0xF0; // plane 3 only, tile x 0..3 -> pen 1
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 0,
        };
        let mut dst = vec![0u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &map, plain, 0, false, None);
        for (x, &pen) in dst.iter().take(16).enumerate() {
            let want = u16::from(x < 4);
            assert_eq!(pen, want, "x={x}");
        }
        assert_eq!(dst[16], 1, "and it wraps every 16 pixels");
        assert_eq!(dst[WIDTH], 0, "row 1 of the tile is blank");
    }

    /// The scroll **adds** to the sampled coordinate.
    ///
    /// `effective_rowscroll` is `m_dx - m_rowscroll[0]`, so the instance is
    /// placed at raster `-scroll` and the sampled pixmap x is `raster + scroll`.
    /// Subtracting instead scrolls the world backwards — which looks fine in a
    /// still frame. Pen 1 covers tile x 0..3, so at scroll 1 the run of 1s ends
    /// one pixel earlier on screen.
    #[test]
    fn the_scroll_moves_the_sampled_column_forwards() {
        let map = one_tile_16();
        let mut rom = vec![0u8; 128];
        rom[64] = 0xF0; // plane 3, tile x 0..3 — see the previous test
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 0,
        };
        let mut dst = vec![0u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &map, plain, 1, false, None);
        assert_eq!(dst[2], 1);
        assert_eq!(dst[3], 0, "scroll 1 pulled tile x 4 into screen x 3");
        dst.fill(0);
        draw(&mut dst, &gfx, &map, plain, 8, false, None);
        assert_eq!(dst[0], 0, "scroll 8 shows tile x 8, which is blank");
        assert_eq!(dst[8], 1, "and tile x 0 at screen x 8");
        // A scroll of one whole map width is indistinguishable from none.
        dst.fill(0);
        draw(&mut dst, &gfx, &map, plain, 16, false, None);
        let mut zero = vec![0u16; WIDTH * HEIGHT];
        draw(&mut zero, &gfx, &map, plain, 0, false, None);
        assert_eq!(dst, zero, "wraps at the map width");
    }

    /// Screen flip mirrors the sampled coordinate about the visible area.
    ///
    /// Raster x 64 (screen x 0) maps to `X_EXTENT - 1 - 64` = 447, and
    /// 447 mod 16 is 15; raster y 16 maps to `Y_EXTENT - 1 - 16` = 239, and
    /// 239 mod 16 is 15. So flipped screen (0,0) samples tile pixel (15,15).
    #[test]
    fn screen_flip_mirrors_the_sampled_coordinate() {
        let map = one_tile_16();
        let mut rom = vec![0u8; 128];
        // Just one pixel: plane 3 (bit offset `half`) at tile (0,0) -> pen 1.
        rom[64] = 0x80;
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 0,
        };
        let mut dst = vec![0u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &map, plain, 0, false, None);
        assert_eq!(dst[0], 1, "unflipped: tile (0,0) at screen (0,0)");
        assert_eq!(dst[15 * WIDTH + 15], 0);
        dst.fill(0);
        draw(&mut dst, &gfx, &map, plain, 0, true, None);
        assert_eq!(dst[0], 0);
        // Screen (15,15) is raster (79,31); 512-1-79 = 432, 432 mod 16 = 0, and
        // 256-1-31 = 224, 224 mod 16 = 0.
        assert_eq!(dst[15 * WIDTH + 15], 1, "flipped: tile (0,0) moved");
    }

    /// The mirror is about the window's centre — the whole frame reverses.
    ///
    /// This is what `xextent = left + right + 1` buys: with a map exactly as wide
    /// as the raster, flipping reverses the visible picture and nothing slides.
    /// A mirror about the *map's* centre instead would shift the picture by
    /// `X_EXTENT - m_width`, which is zero for the text map and 32,256 pixels for
    /// the scrolling planes — so only a full-window assertion catches it.
    #[test]
    fn flipping_the_text_map_reverses_the_visible_frame_exactly() {
        // The text map is 512x256, the same as the raster, so this is a pure
        // reversal with no wrap interaction.
        let mut rom = vec![0u8; 16 * 1024]; // 1,024 char elements
                                            // Element 1 starts at bit 128 = byte 16. Plane 1 (offset 0) is pen bit 0,
                                            // and `readbit` is MSB-first, so bit 128 is byte 16's 0x80.
        rom[16] = 0x80;
        let gfx = GfxSet {
            rom: &rom,
            layout: &CHAR_LAYOUT,
            colour_base: 0,
        };
        assert_eq!(gfx.elements(), 1_024);
        // Put element 1 in exactly one videoram cell: memory index for tile
        // (col 8, row 2), which is raster (64,16) = screen (0,0).
        let mut ram = vec![0u16; TX.tiles() as usize];
        ram[Scan::Rows.index(8, 2, TX.cols, TX.rows) as usize] = 1;
        let info = |i| tx_tile_info(&ram, i);
        let mut plain_buf = vec![0u16; WIDTH * HEIGHT];
        draw(&mut plain_buf, &gfx, &TX, info, 0, false, None);
        assert_eq!(plain_buf[0], 1, "pen 1 at screen (0,0)");
        assert_eq!(plain_buf.iter().filter(|&&p| p == 1).count(), 1);
        let mut flipped = vec![0u16; WIDTH * HEIGHT];
        draw(&mut flipped, &gfx, &TX, info, 0, true, None);
        // Reverse the unflipped frame and the two must agree pixel for pixel.
        let mut reversed = plain_buf.clone();
        reversed.reverse();
        assert_eq!(flipped, reversed, "flip is the frame reversed");
        assert_eq!(
            flipped[WIDTH * HEIGHT - 1],
            1,
            "the pen moved to the far corner"
        );
    }

    /// Wraparound covers the screen even when the map is narrower than it.
    ///
    /// `draw_common` loops `for (xpos = scrollx - m_width; xpos <= right();
    /// xpos += m_width)` (`tilemap.cpp:1018-1020`), so a 16-pixel map tiles the
    /// whole raster. A single-instance draw would leave most of the screen blank.
    #[test]
    fn a_map_narrower_than_the_screen_wraps_to_fill_it() {
        let map = one_tile_16();
        let rom = vec![0xFFu8; 128]; // every plane set: pen 15 everywhere
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 0,
        };
        let mut dst = vec![0u16; WIDTH * HEIGHT];
        draw(
            &mut dst,
            &gfx,
            &map,
            |_| TileInfo {
                code: 0,
                colour: 1,
                flags: 0,
            },
            0,
            false,
            None,
        );
        // colour 1, granularity 16, pen 15 -> entry 31, on every pixel.
        assert!(dst.iter().all(|&p| p == 31), "every pixel covered");
    }

    /// Flip flags mirror within the tile and leave its screen position alone.
    #[test]
    fn the_flip_flags_mirror_the_tile_in_place() {
        let map = one_tile_16();
        let mut rom = vec![0u8; 128];
        rom[64] = 0x80; // plane 3 at tile (0,0) -> pen 1
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 0,
        };
        let cases = [
            (0u8, 0usize, 0usize),
            (FLIP_X, 15, 0),
            (FLIP_Y, 0, 15),
            (FLIP_X | FLIP_Y, 15, 15),
        ];
        for (flags, wx, wy) in cases {
            let mut dst = vec![0u16; WIDTH * HEIGHT];
            draw(
                &mut dst,
                &gfx,
                &map,
                |_| TileInfo {
                    code: 0,
                    colour: 0,
                    flags,
                },
                0,
                false,
                None,
            );
            assert_eq!(dst[wy * WIDTH + wx], 1, "flags {flags:#x}");
            assert_eq!(
                dst.iter().filter(|&&p| p == 1).count(),
                24 * 14,
                "one per tile"
            );
        }
    }

    /// The transparent pen is per-map, and `None` means fully opaque.
    ///
    /// `sf.cpp:764-765` sets pen 15 transparent on the **foreground** and pen 3
    /// on the **text** map, and never calls `set_transparent_pen` on the
    /// background — so a background pen 15 draws. Assuming one universal
    /// transparent pen punches a hole in every background.
    #[test]
    fn transparency_is_per_map_and_the_background_is_opaque() {
        let map = one_tile_16();
        let rom = vec![0xFFu8; 128]; // pen 15 everywhere
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 0,
        };
        let mut dst = vec![0x1234u16; WIDTH * HEIGHT];
        draw(&mut dst, &gfx, &map, plain, 0, false, Some(15));
        assert!(
            dst.iter().all(|&p| p == 0x1234),
            "pen 15 left the buffer alone"
        );
        draw(&mut dst, &gfx, &map, plain, 0, false, None);
        assert!(
            dst.iter().all(|&p| p == 15),
            "opaque: pen 15 wrote entry 15"
        );
    }

    /// A code past the graphics region leaves the destination untouched.
    #[test]
    fn an_out_of_range_code_draws_nothing() {
        let map = one_tile_16();
        let rom = vec![0xFFu8; 128];
        let gfx = GfxSet {
            rom: &rom,
            layout: &SPRITE_LAYOUT,
            colour_base: 0,
        };
        assert_eq!(gfx.elements(), 1);
        let mut dst = vec![7u16; WIDTH * HEIGHT];
        draw(
            &mut dst,
            &gfx,
            &map,
            |_| TileInfo {
                code: 9_999,
                colour: 0,
                flags: 0,
            },
            0,
            false,
            None,
        );
        assert!(
            dst.iter().all(|&p| p == 7),
            "nothing drawn, nothing panicked"
        );
    }

    /// The buffer length is checked, like [`crate::layers::draw_tilemap`] checks its own.
    #[test]
    #[should_panic(expected = "destination must be WIDTH * HEIGHT")]
    fn a_wrongly_sized_destination_is_a_programming_error() {
        let gfx = GfxSet {
            rom: &[],
            layout: &CHAR_LAYOUT,
            colour_base: 768,
        };
        let mut dst = vec![0u16; 10];
        draw(&mut dst, &gfx, &TX, plain, 0, false, None);
    }

    /// A one-tile 16×16 map. Small enough that every expectation above is
    /// arithmetic a reader can redo.
    fn one_tile_16() -> Tilemap {
        Tilemap {
            scan: Scan::Rows,
            tile_w: 16,
            tile_h: 16,
            cols: 1,
            rows: 1,
        }
    }

    /// Tile 0, colour 0, no flip — the callback for the geometric tests.
    fn plain(_index: u32) -> TileInfo {
        TileInfo::default()
    }

    #[test]
    fn a_sample_names_the_tile_and_the_pixel_within_it() {
        let s = sample(&BG, 0, false, 0, 0);
        // VISIBLE_X 64 / 16 = 4, VISIBLE_Y 16 / 16 = 1, both exactly on a tile edge.
        assert_eq!(
            s,
            Sample {
                col: 4,
                row: 1,
                x: 0,
                y: 0
            }
        );
        assert_eq!(BG.scan.index(s.col, s.row, BG.cols, BG.rows), 65);
    }

    #[test]
    fn the_scroll_moves_the_sampled_pixel_not_the_tile() {
        let s = sample(&BG, 1, false, 0, 0);
        assert_eq!(
            s,
            Sample {
                col: 4,
                row: 1,
                x: 1,
                y: 0
            }
        );
    }

    #[test]
    fn a_scroll_of_one_whole_tile_moves_one_whole_column() {
        let s = sample(&BG, 16, false, 0, 0);
        assert_eq!(
            s,
            Sample {
                col: 5,
                row: 1,
                x: 0,
                y: 0
            }
        );
    }

    #[test]
    fn a_scroll_of_the_maps_whole_width_samples_the_same_pixel() {
        assert_eq!(BG.width(), 32_768);
        assert_eq!(
            sample(&BG, 32_768, false, 0, 0),
            sample(&BG, 0, false, 0, 0)
        );
    }

    #[test]
    fn flip_mirrors_the_screen_so_the_corners_swap() {
        // The top-left pixel under flip is the bottom-right pixel without it.
        assert_eq!(
            sample(&BG, 0, true, 0, 0),
            Sample {
                col: 27,
                row: 14,
                x: 15,
                y: 15
            }
        );
        assert_eq!(
            sample(&BG, 0, false, WIDTH - 1, HEIGHT - 1),
            sample(&BG, 0, true, 0, 0)
        );
    }

    #[test]
    fn the_text_layers_geometry_is_its_own() {
        let s = sample(&TX, 0, false, 0, 0);
        // 64 / 8 = 8, 16 / 8 = 2. `Scan::Rows`, so the index is row-major.
        assert_eq!(
            s,
            Sample {
                col: 8,
                row: 2,
                x: 0,
                y: 0
            }
        );
        assert_eq!(TX.scan.index(s.col, s.row, TX.cols, TX.rows), 136);
        assert_eq!(
            sample(&TX, 0, true, 0, 0),
            Sample {
                col: 55,
                row: 29,
                x: 7,
                y: 7
            }
        );
        assert_eq!(
            sample(&TX, 0, false, WIDTH - 1, HEIGHT - 1),
            sample(&TX, 0, true, 0, 0)
        );
    }

    #[test]
    fn a_degenerate_map_samples_the_origin_rather_than_dividing_by_zero() {
        let empty = Tilemap {
            scan: Scan::Rows,
            tile_w: 8,
            tile_h: 8,
            cols: 0,
            rows: 0,
        };
        assert_eq!(
            sample(&empty, 0, false, 0, 0),
            Sample {
                col: 0,
                row: 0,
                x: 0,
                y: 0
            }
        );
    }

    #[test]
    fn the_geometry_constants_are_what_the_panels_will_page_over() {
        assert_eq!((BG.cols, BG.rows, BG.tiles()), (2048, 16, 32_768));
        assert_eq!((TX.cols, TX.rows, TX.tiles()), (64, 32, 2048));
        assert_eq!((BG.width(), BG.height()), (32_768, 256));
        assert_eq!((TX.width(), TX.height()), (512, 256));
    }

    #[test]
    fn every_map_names_its_plane_and_its_geometry() {
        assert_eq!(MapKind::Bg.plane(), crate::sf1::Plane::Bg);
        assert_eq!(MapKind::Fg.plane(), crate::sf1::Plane::Fg);
        assert_eq!(MapKind::Tx.plane(), crate::sf1::Plane::Tx);
        assert_eq!((MapKind::Bg.map().cols, MapKind::Bg.map().rows), (2048, 16));
        assert_eq!((MapKind::Fg.map().cols, MapKind::Fg.map().rows), (2048, 16));
        assert_eq!((MapKind::Tx.map().cols, MapKind::Tx.map().rows), (64, 32));
    }

    #[test]
    fn each_map_reads_its_own_scroll_register_and_the_text_layer_reads_none() {
        assert_eq!(MapKind::Bg.scroll(0x0040, 0x0080), 0x40);
        assert_eq!(MapKind::Fg.scroll(0x0040, 0x0080), 0x80);
        assert_eq!(MapKind::Tx.scroll(0x0040, 0x0080), 0);
    }

    #[test]
    fn only_the_background_draws_every_pen() {
        // `draw_common` never calls `set_transparent_pen` for the background, so
        // pen 15 draws there and is a hole on the other two.
        assert_eq!(MapKind::Bg.transparent_pen(), None);
        assert_eq!(MapKind::Fg.transparent_pen(), Some(15));
        assert_eq!(MapKind::Tx.transparent_pen(), Some(3));
    }

    #[test]
    fn the_two_rom_maps_name_their_byte_offset_and_the_ram_map_names_none() {
        assert_eq!(MapKind::Bg.tilerom_base(), Some(0));
        assert_eq!(MapKind::Fg.tilerom_base(), Some(0x2_0000));
        assert_eq!(MapKind::Tx.tilerom_base(), None);
    }

    #[test]
    fn a_maps_tile_info_is_the_free_function_it_names() {
        let mut tilerom = vec![0u8; 0x4_0000];
        // Cell 3 of the background: colour at +6, code low at +7, attr and code
        // high one plane (0x10000) further on.
        tilerom[6] = 0x0A;
        tilerom[7] = 0x34;
        tilerom[PLANE + 6] = 0x02;
        tilerom[PLANE + 7] = 0x12;
        let videoram = vec![0u16; 2048];
        assert_eq!(
            MapKind::Bg.tile_info(&tilerom, &videoram, 3),
            TileInfo {
                code: 0x1234,
                colour: 0x0A,
                flags: 0x02
            }
        );
        assert_eq!(
            MapKind::Bg.tile_info(&tilerom, &videoram, 3),
            bg_tile_info(&tilerom, 3)
        );
        // The same bytes at FG_BASE are the foreground's cell 3.
        tilerom[FG_BASE + 6] = 0x0B;
        tilerom[FG_BASE + 7] = 0x56;
        assert_eq!(
            MapKind::Fg.tile_info(&tilerom, &videoram, 3),
            fg_tile_info(&tilerom, 3)
        );
        assert_eq!(MapKind::Fg.tile_info(&tilerom, &videoram, 3).colour, 0x0B);
        // The text layer ignores the tilerom entirely.
        let mut videoram = vec![0u16; 2048];
        videoram[3] = 0x5C21;
        assert_eq!(
            MapKind::Tx.tile_info(&tilerom, &videoram, 3),
            TileInfo {
                code: 0x021,
                colour: 5,
                flags: 0x03
            }
        );
    }

    #[test]
    fn cycling_a_map_visits_all_three_and_returns() {
        let mut k = MapKind::Bg;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(k.name());
            k = k.cycled();
        }
        assert_eq!(seen, ["BG", "FG", "TX"]);
        assert_eq!(k, MapKind::Bg);
        assert_eq!(MapKind::ALL.len(), 3);
    }
}
