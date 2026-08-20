//! The graphics viewer's four views, drawn into the framebuffer.
//!
//! # What this is for
//!
//! The debugger in [`crate::overlay`] answers questions about the 68000. This
//! answers the other half: the screen is wrong, and the question is *which stage* is
//! wrong. Is the tile in the ROM at all? Is the map pointing at it? Is the palette
//! entry the colour you meant? Is the layer even enabled?
//!
//! # Nothing here re-derives what the renderer knows
//!
//! Every fact on screen comes from the function `video` renders with —
//! `tile_pen`, `tile_info`, `map_axis`, `layer_enabled`, `feeds_sprites`,
//! `layer_order`, `entry_to_rgb`, `BankMapper::map`. That is the whole design of
//! this module, and `video`'s `map_axis` documents why: the raster bias, the
//! Euclidean division and the wrap at 64 are four decisions this crate would have
//! to get right a second time, and a viewer that named a tile the renderer never
//! fetched would be a diagnostic that lies at exactly the moment you are trusting
//! it.
//!
//! # Greyscale, on purpose
//!
//! The tile browser draws pens as sixteen shades of grey rather than through a
//! palette. A tinted tile makes a wrong decode and a wrong palette look the same,
//! and telling those two apart is what the browser is for. The palette view is
//! where colour lives.
//!
//! # Reading it back
//!
//! The tests read the characters off the pixels with `font::panel_contains` and
//! `font::read_text`, for the reason `overlay`'s module docs give: a test compared
//! against the same `format!` the view used asserts only that the formatter equals
//! itself.
//!
//! (Those two are plain code spans rather than rustdoc links because they are
//! `#[cfg(test)]` and do not exist in a doc build.)

use crate::font::{draw_text, fill_rect, swatch, ADVANCE, GLYPH_H, LINE};
use machine::video::compose::{feeds_sprites, layer_enabled, layer_order, LayerMask, DEPTHS};
use machine::video::layers::{map_axis, tile_info, Layer, MAP_TILES};
use machine::video::palette::{BACKGROUND_PEN, PENS};
use machine::video::regs::{
    cps_a_base, SCROLL1_BASE, SCROLL1_X, SCROLL1_Y, SCROLL2_BASE, SCROLL2_X, SCROLL2_Y,
    SCROLL3_BASE, SCROLL3_X, SCROLL3_Y, SCROLL_BOUNDARY, VIDEOCONTROL,
};
use machine::video::tiles::{tile_pen, TileKind};
use machine::video::{HEIGHT, VISIBLE_X, VISIBLE_Y, WIDTH};
use machine::Cps1;

/// The viewer's box: the whole frame, inset by two pixels.
pub(crate) const VX: usize = 2;
/// Ditto.
pub(crate) const VY: usize = 2;
/// Ditto.
pub(crate) const VW: usize = WIDTH - 4;
/// Ditto.
pub(crate) const VH: usize = HEIGHT - 4;

/// Background: darker than E2's panels, because this box is the whole screen and
/// E2's sits on top of it — two identical backgrounds would make the boundary
/// between them invisible.
pub(crate) const BG: u32 = 0x0000_0010;

/// Ordinary text.
pub(crate) const FG: u32 = 0x00D0_D0D0;

/// A heading, and the cursored item.
pub(crate) const HI: u32 = 0x0060_FF60;

/// A value the hardware says no to: a disabled layer, an unmapped code.
pub(crate) const OFF: u32 = 0x00FF_6060;

/// A swatch's border.
pub(crate) const EDGE: u32 = 0x0080_8080;

/// Padding inside the box.
pub(crate) const PAD: usize = 2;

/// The sixteen greys a tile pen is drawn as, black to white.
///
/// Written out rather than computed. `gfxpanels::tests::grey_literal` is the same
/// ramp typed a second time from the channel values, so the two agree only if both
/// are right — a test comparing this against `pen * 17` would be the same expression
/// written twice.
pub(crate) const GREYS: [u32; 16] = [
    0x0000_0000,
    0x0011_1111,
    0x0022_2222,
    0x0033_3333,
    0x0044_4444,
    0x0055_5555,
    0x0066_6666,
    0x0077_7777,
    0x0088_8888,
    0x0099_9999,
    0x00AA_AAAA,
    0x00BB_BBBB,
    0x00CC_CCCC,
    0x00DD_DDDD,
    0x00EE_EEEE,
    0x00FF_FFFF,
];

/// Which graphics view is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The graphics ROM as a grid of tiles.
    Tiles,
    /// One scroll layer's table, around a cursor.
    Tilemap,
    /// All 3072 palette entries as swatches.
    Palette,
    /// The four depths: what is enabled, what is masked, what feeds the sprites.
    Layers,
}

impl View {
    /// The next view, wrapping.
    pub const fn cycled(self) -> Self {
        match self {
            Self::Tiles => Self::Tilemap,
            Self::Tilemap => Self::Palette,
            Self::Palette => Self::Layers,
            Self::Layers => Self::Tiles,
        }
    }

    /// The view's name, as it appears on its own title line.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tiles => "TILES",
            Self::Tilemap => "TILEMAP",
            Self::Palette => "PALETTE",
            Self::Layers => "LAYERS",
        }
    }
}

/// Everything a view needs that is not the machine.
///
/// Owned by `gfx::GfxViewer`, which is where the keys move it. Split out so
/// that drawing is a pure function of a machine and a state, and every test below can
/// name the state it wants rather than pressing keys to reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewState {
    /// Which view.
    pub view: View,
    /// The tile view's layout.
    pub kind: TileKind,
    /// The tilemap view's layer.
    pub layer: Layer,
    /// The tile view's first ROM index.
    pub tile_at: u32,
    /// The palette view's cursor.
    pub pal_at: usize,
    /// The tilemap cursor, or [`None`] to follow the beam — see [`map_origin`].
    pub map_at: Option<(u32, u32)>,
    /// The layers view's selected row: 0 for the sprites, 1-3 for the scrolls.
    pub row: usize,
    /// Which layers the machine is permitted to draw.
    pub mask: LayerMask,
}

/// Draws the current view over whatever is in `buf`.
///
/// # Panics
///
/// If `buf` is not a `WIDTH × HEIGHT` frame, as `font::draw_text`.
pub fn draw(buf: &mut [u32], m: &Cps1, s: &ViewState) {
    assert_eq!(buf.len(), WIDTH * HEIGHT, "not a frame");
    fill_rect(buf, VX, VY, VW, VH, BG);
    match s.view {
        View::Tiles => draw_tiles(buf, m, s),
        View::Tilemap => draw_tilemap_view(buf, m, s),
        View::Palette => draw_palette(buf, m, s),
        View::Layers => draw_layers(buf, m, s),
    }
}

/// The map tile the renderer fetches for the visible top-left pixel of `layer`.
///
/// The tilemap view's cursor default. [`map_axis`] is called rather than re-derived,
/// because the raster bias, the Euclidean division, and the wrap at 64 are four
/// decisions the renderer has already made — see `video`'s `map_axis`.
pub fn map_origin(m: &Cps1, layer: Layer) -> (u32, u32) {
    let (_, sx, sy) = layer_regs(layer);
    // `as i16` before widening: 0xFFC0 is −64, not 65472. The registers are
    // unsigned words holding signed scrolls, which is the trap `compose.rs`
    // documents at length.
    //
    // **No test can tell this from `as i32`**, and that is provable rather than a
    // gap: the two readings differ by exactly 65536, and 65536 is a whole number of
    // map spans for every layer — 64×8, 64×16 and 64×32 all divide it — so
    // `map_axis`'s `div_euclid`/`rem_euclid` land on the same tile *and* the same
    // offset for either. `a_map_span_divides_the_register_range` pins that
    // precondition, since it is a fact about the hardware's numbers rather than
    // about this line. The `as i16` stays because the intermediate value is a
    // scroll, and a scroll of −64 is not 65472 whatever the wrap does with it
    // afterwards.
    let x = VISIBLE_X + i32::from(m.board.cps_a[sx] as i16);
    let y = VISIBLE_Y + i32::from(m.board.cps_a[sy] as i16);
    let edge = layer.tile_edge();
    (map_axis(edge, x).0, map_axis(edge, y).0)
}

/// A layer's table-base and scroll registers.
///
/// The same three indices `Video::render` selects. Written here as one function used
/// by both [`map_origin`] and the tilemap view, rather than twice: three register
/// numbers per layer is exactly the kind of table that is wrong in one copy and right
/// in the other.
const fn layer_regs(layer: Layer) -> (usize, usize, usize) {
    match layer {
        Layer::Scroll1 => (SCROLL1_BASE, SCROLL1_X, SCROLL1_Y),
        Layer::Scroll2 => (SCROLL2_BASE, SCROLL2_X, SCROLL2_Y),
        Layer::Scroll3 => (SCROLL3_BASE, SCROLL3_X, SCROLL3_Y),
    }
}

/// A layer's two-character tag, as the views label it.
const fn layer_tag(layer: Layer) -> &'static str {
    match layer {
        Layer::Scroll1 => "S1",
        Layer::Scroll2 => "S2",
        Layer::Scroll3 => "S3",
    }
}

/// A tile layout's name, short enough for a title line.
const fn kind_name(kind: TileKind) -> &'static str {
    match kind {
        TileKind::Tile8x8 => "8X8",
        TileKind::Tile8x8Odd => "8X8ODD",
        TileKind::Tile16x16 => "16X16",
        TileKind::Tile32x32 => "32X32",
    }
}

/// Draws `s` at `(x, y)`, truncated to what fits inside the box.
///
/// Not merely clipped: `draw_text` clips to the *frame*, and this box is inset from
/// it, so a string long enough to reach the frame's edge would draw over the two
/// columns the box leaves alone. `every_view_stays_inside_its_box` is what would
/// catch that, and this is what stops it happening.
pub(crate) fn text(buf: &mut [u32], x: usize, y: usize, s: &str, fg: u32) {
    if y + GLYPH_H > VY + VH || x >= VX + VW {
        return;
    }
    let cells = (VX + VW - x) / ADVANCE;
    let cut: String = s.chars().take(cells).collect();
    draw_text(buf, x, y, &cut, fg);
}

/// One pixel, if it is inside the box.
pub(crate) fn put(buf: &mut [u32], x: usize, y: usize, c: u32) {
    if (VX..VX + VW).contains(&x) && (VY..VY + VH).contains(&y) {
        buf[y * WIDTH + x] = c;
    }
}

/// The first line of text inside the box: every view's title.
pub(crate) const fn title_at() -> (usize, usize) {
    (VX + PAD, VY + PAD)
}

/// Where a view's content starts: one line below its title.
pub(crate) const fn content_y() -> usize {
    VY + PAD + LINE + 1
}

/// A tile pen as one of [`GREYS`].
///
/// Pen 15 is drawn white rather than skipped, even though it is
/// `tiles::TRANSPARENT_PEN`. A browser exists to show what is *in* the ROM, and a
/// transparent pen and an absent tile are different facts — see
/// [`tile_in_rom`], which is how the second one is shown.
pub(crate) const fn grey(pen: u8) -> u32 {
    GREYS[(pen & 0x0F) as usize]
}

/// Whether tile `code` of `kind` lies wholly inside a ROM of `len` bytes.
///
/// `tile_pen` returns the transparent pen for a tile past the end, which on screen is
/// a solid white square — indistinguishable from a tile that is genuinely all pen 15.
/// The views use this to draw "not in the ROM" as something else entirely.
fn tile_in_rom(len: usize, kind: TileKind, code: u32) -> bool {
    (code as usize)
        .checked_mul(kind.bytes())
        .and_then(|start| start.checked_add(kind.bytes()))
        .is_some_and(|end| end <= len)
}

/// Cells of the tile view's row label: four hex digits and a space.
const TILE_LABEL: usize = 5;

/// How many tiles the tile view shows: `(columns, rows)`.
///
/// Published with [`tile_cell`] so the tests can read a tile's pixels back at the
/// coordinates the view drew them, rather than computing a second layout that could
/// agree with nothing.
pub fn tile_grid(kind: TileKind) -> (usize, usize) {
    let step = kind.size() as usize + 1;
    // A row is at least a line of text high, so the row label always fits beside it.
    let row_step = if step > LINE { step } else { LINE };
    let x0 = VX + PAD + TILE_LABEL * ADVANCE;
    let y0 = content_y();
    let cols = (VX + VW - PAD).saturating_sub(x0) / step;
    let rows = (VY + VH - PAD).saturating_sub(y0) / row_step;
    (cols, rows)
}

/// The top-left pixel of the tile view's `slot`th cell, counting across then down.
pub fn tile_cell(kind: TileKind, slot: usize) -> (usize, usize) {
    let step = kind.size() as usize + 1;
    let row_step = if step > LINE { step } else { LINE };
    let (cols, _) = tile_grid(kind);
    let x0 = VX + PAD + TILE_LABEL * ADVANCE;
    let y0 = content_y();
    (x0 + (slot % cols) * step, y0 + (slot / cols) * row_step)
}

/// The graphics ROM as a grid of tiles, in greyscale.
fn draw_tiles(buf: &mut [u32], m: &Cps1, s: &ViewState) {
    let rom = m.video.gfx();
    let (cols, rows) = tile_grid(s.kind);
    let page = (cols * rows) as u32;
    let (tx, ty) = title_at();
    let held = (rom.len() / s.kind.bytes()) as u32;
    text(
        buf,
        tx,
        ty,
        &format!(
            "{} {} {:05X}-{:05X} OF {:05X} ENTER CYCLES",
            View::Tiles.name(),
            kind_name(s.kind),
            s.tile_at,
            s.tile_at.saturating_add(page.saturating_sub(1)),
            held
        ),
        HI,
    );
    for row in 0..rows {
        // Saturating, because `tile_at` is a `u32` the bracket keys drive and a page
        // past 0xFFFFFFFF must show as "not in the ROM", not panic in a debug build.
        let first = s.tile_at.saturating_add((row * cols) as u32);
        let (_, cy) = tile_cell(s.kind, row * cols);
        text(buf, VX + PAD, cy, &format!("{first:04X}"), FG);
        for col in 0..cols {
            let code = first.saturating_add(col as u32);
            let (cx, cy) = tile_cell(s.kind, row * cols + col);
            if !tile_in_rom(rom.len(), s.kind, code) {
                // One dot, not a fill: "past the end of the ROM" must not be
                // mistakable for a tile whose pens happen to be uniform.
                put(buf, cx, cy, OFF);
                continue;
            }
            for y in 0..s.kind.size() {
                for x in 0..s.kind.size() {
                    let pen = tile_pen(rom, s.kind, code, x, y);
                    put(buf, cx + x as usize, cy + y as usize, grey(pen));
                }
            }
        }
    }
}

/// Columns of codes the tilemap view shows around its cursor.
const MAP_COLS: usize = 8;
/// Ditto, rows.
const MAP_ROWS: usize = 8;
/// Cells one code takes: four hex digits and a space.
const MAP_CELL: usize = 5;

/// One scroll layer's table, around the cursor.
fn draw_tilemap_view(buf: &mut [u32], m: &Cps1, s: &ViewState) {
    let (base_reg, sx, sy) = layer_regs(s.layer);
    let table = cps_a_base(&m.board.cps_a, base_reg, SCROLL_BOUNDARY);
    let (cur_c, cur_r) = s.map_at.unwrap_or_else(|| map_origin(m, s.layer));
    let info = tile_info(&m.board.gfxram[..], table, s.layer, cur_c, cur_r);
    let (tx, ty) = title_at();

    text(
        buf,
        tx,
        ty,
        &format!(
            "{} {} MAP {:02},{:02} CODE {:04X}",
            View::Tilemap.name(),
            layer_tag(s.layer),
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
            "COL {:02X} FLIP {}{} GRP {}",
            info.colour,
            flip(info.flip_x, 'X'),
            flip(info.flip_y, 'Y'),
            info.group
        ),
        FG,
    );
    // The mapper's `None` is the one failure the picture cannot show: `draw_tilemap`
    // skips an unmapped tile silently, which is correct and undiagnosable. Shown as
    // `----`, because a viewer that printed it as 0 would send you to tile 0.
    match m.video.mapper.map(s.layer.gfx_type(), info.code) {
        Some(off) => text(buf, tx, ty + 2 * LINE, &format!("ROM {off:05X}"), FG),
        None => text(buf, tx, ty + 2 * LINE, "ROM ----", OFF),
    }
    text(
        buf,
        tx,
        ty + 3 * LINE,
        &format!(
            "TAB {:05X} SX {:+05} SY {:+05} ENTER CYCLES LAYER",
            table, m.board.cps_a[sx] as i16, m.board.cps_a[sy] as i16
        ),
        FG,
    );

    // The window of codes: the cursor three cells in, so there is context on both
    // sides of it, and wrapped at the map's edge like the renderer's own fetch.
    let gx = VX + PAD + 4 * ADVANCE;
    let gy = content_y() + 4 * LINE;
    let first_c = (cur_c + MAP_TILES - 3) % MAP_TILES;
    let first_r = (cur_r + MAP_TILES - 3) % MAP_TILES;
    for r in 0..MAP_ROWS {
        let row = (first_r + r as u32) % MAP_TILES;
        let y = gy + r * LINE;
        text(buf, VX + PAD, y, &format!("{row:02}"), FG);
        for c in 0..MAP_COLS {
            let col = (first_c + c as u32) % MAP_TILES;
            let t = tile_info(&m.board.gfxram[..], table, s.layer, col, row);
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
    let kind = s.layer.gfx_type().tile_kind();
    let rom = m.video.gfx();
    if let Some(off) = m.video.mapper.map(s.layer.gfx_type(), info.code) {
        if tile_in_rom(rom.len(), kind, off) {
            for y in 0..kind.size() {
                for x in 0..kind.size() {
                    let pen = tile_pen(rom, kind, off, x, y);
                    put(buf, px + x as usize, gy + y as usize, grey(pen));
                }
            }
        } else {
            text(buf, px, gy, "OFF ROM", OFF);
        }
    }
}

/// Palette swatches per row: 64 columns of 5 pixels is 320, which fits the box.
const PAL_COLS: usize = 64;
/// One swatch's width.
const PAL_CW: usize = 5;
/// Ditto, height. All 3072 entries fit at once, so the palette view never pages.
const PAL_CH: usize = 4;

/// The top-left pixel of palette entry `n`'s swatch.
///
/// Published so the tests read the colour back off the pixel the view wrote, the
/// same relationship `overlay`'s tests have to `REGS_X`.
pub fn pal_cell(n: usize) -> (usize, usize) {
    let x0 = VX + PAD + 3 * ADVANCE;
    let y0 = content_y();
    (x0 + (n % PAL_COLS) * PAL_CW, y0 + (n / PAL_COLS) * PAL_CH)
}

/// All 3072 palette entries as swatches, with the cursored one named.
fn draw_palette(buf: &mut [u32], m: &Cps1, s: &ViewState) {
    let pal = m.video.palette();
    let at = s.pal_at.min(PENS - 1);
    let (tx, ty) = title_at();
    text(
        buf,
        tx,
        ty,
        &format!(
            "{} {:04X} ENTRY {:04X} PAGE {} BG {:04X}",
            View::Palette.name(),
            at,
            pal[at],
            at / 0x200,
            BACKGROUND_PEN
        ),
        HI,
    );
    for (n, &entry) in pal.iter().enumerate() {
        // The window's own conversion, not a second one: [`crate::pixels::argb`] is
        // what the frame the game is drawn into goes through, so a swatch and the
        // game agree by construction rather than by inspection.
        let fill = crate::pixels::argb(entry);
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
        // A page label down the left edge, every 512 entries.
        if n % 0x200 == 0 {
            text(buf, VX + PAD, y, &format!("P{}", n / 0x200), FG);
        }
    }
}

/// The four depths: hardware enable, mask bit, depth, and the sprite feed.
fn draw_layers(buf: &mut [u32], m: &Cps1, s: &ViewState) {
    // The machine's own configuration, not `VideoConfig::sf2()`: a viewer that
    // hardcoded SF2's register numbers would read the wrong words on the next board
    // this project drives, and be confidently wrong about it.
    let cfg = &m.video.cfg;
    let layercontrol = m.board.cps_b[cfg.layer_control];
    let videocontrol = m.board.cps_a[VIDEOCONTROL];
    let order = layer_order(layercontrol);
    let (tx, ty) = title_at();
    text(
        buf,
        tx,
        ty,
        &format!(
            "{} LC {:04X} VC {:04X} ENTER TOGGLES MASK",
            View::Layers.name(),
            layercontrol,
            videocontrol
        ),
        HI,
    );

    // What each depth draws, back to front, so a repeated field is visible as one.
    let mut depths = String::from("DEPTH");
    for want in order {
        depths.push(' ');
        depths.push_str(match want {
            0 => "OB",
            1 => "S1",
            2 => "S2",
            _ => "S3",
        });
    }
    text(buf, tx, ty + LINE, &depths, FG);

    for row in 0..4 {
        let y = content_y() + (row + 1) * LINE;
        // The sprites are row 0 because they are `layer_order`'s value 0.
        let (tag, want, enabled, permitted) = match row {
            0 => ("OB", 0u8, None, s.mask.permits(None)),
            n => {
                let layer = match n {
                    1 => Layer::Scroll1,
                    2 => Layer::Scroll2,
                    _ => Layer::Scroll3,
                };
                (
                    layer_tag(layer),
                    n as u8,
                    Some(layer_enabled(cfg, layer, layercontrol, videocontrol)),
                    s.mask.permits(Some(layer)),
                )
            }
        };
        // CPS-1 has no sprite enable bit, and saying so is the answer to the
        // question this column otherwise invites.
        let state = match enabled {
            None => "ALWAYS",
            Some(true) => "ON",
            Some(false) => "OFF",
        };
        // Which depths draw this layer, and whether any of them feeds the sprite
        // occlusion mask — both read off the renderer's own functions. A value can
        // appear at more than one depth: SF2's own 0x1B40 repeats scroll 1.
        let mut at = String::new();
        let mut feeds = false;
        for depth in 0..DEPTHS {
            if order[depth] == want {
                at.push_str(&format!("{depth}"));
                feeds |= feeds_sprites(&order, depth);
            }
        }
        if at.is_empty() {
            at.push('-');
        }
        let fg = if enabled == Some(false) { OFF } else { FG };
        if row == s.row {
            text(buf, VX + PAD, y, ">", HI);
        }
        text(
            buf,
            VX + PAD + ADVANCE,
            y,
            &format!(
                "{tag} {state:6} MSK {:3} AT {at:4} FEED {}",
                if permitted { "ON" } else { "OFF" },
                if feeds { 'Y' } else { '-' }
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
    use machine::config::BoardConfig;
    use machine::timing::Timing;
    use machine::video::bank::GfxType;
    use machine::video::regs::{VideoConfig, PALETTE_BASE};

    /// A booted machine with `gfx` as its graphics ROM, all three layers enabled.
    ///
    /// The layer control holds the three enable bits *and* four distinct depth
    /// fields, and videocontrol the two bits scrolls 2 and 3 need — so every view
    /// below has something to show. A fixture with the layers off would let a view
    /// that drew nothing pass.
    ///
    /// Boxed, and not for tidiness: a `Cps1` is half a megabyte of RAM and gfxram by
    /// value, and returning it by value through a wrapping fixture overflows a test
    /// thread's 2 MB stack — which it did, as a `SIGABRT` with no failing assertion.
    fn a_machine(gfx: Vec<u8>) -> Box<Cps1> {
        let mut rom = vec![0u8; 0x2000];
        // SSP 0x00FF8000, PC 0x1000, then a branch to itself.
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x1000..0x1002].copy_from_slice(&[0x60, 0xFE]);
        let mut m = Box::new(Cps1::with_gfx(
            &rom,
            gfx,
            BoardConfig::sf2(),
            Timing::cps1_10mhz(),
        ));
        m.reset();
        let cfg = VideoConfig::sf2();
        let enables = cfg.layer_enable_mask.iter().fold(0u16, |a, m| a | m);
        // Depths, back to front: scroll 1, 2, 3, then the sprites in front.
        m.board.cps_b[cfg.layer_control] = enables | (1 << 6) | (2 << 8) | (3 << 10);
        // Bit 2 enables scroll 2 and bit 3 scroll 3; scroll 1 has no such bit.
        m.board.cps_a[VIDEOCONTROL] = 0x000C;
        m
    }

    /// A machine whose rendered palette holds `entries`.
    ///
    /// The palette is built at render time, so the entries go into gfxram and the
    /// machine renders once — which is what `Video::palette` then reports and what
    /// the palette view reads.
    fn a_machine_with_palette(entries: &[(usize, u16)]) -> Box<Cps1> {
        let mut m = a_machine(Vec::new());
        let cfg = VideoConfig::sf2();
        for &(n, e) in entries {
            m.board.gfxram[n] = e;
        }
        // Palette base 0 resolves to word 0, and all six pages enabled so every
        // entry written above is copied.
        m.board.cps_a[PALETTE_BASE] = 0;
        m.board.cps_b[cfg.palette_control] = 0x003F;
        m.render();
        m
    }

    /// The state every test starts from and overrides one field of.
    fn base_state() -> ViewState {
        ViewState {
            view: View::Tiles,
            kind: TileKind::Tile16x16,
            layer: Layer::Scroll2,
            tile_at: 0,
            pal_at: 0,
            map_at: None,
            row: 0,
            mask: LayerMask::all(),
        }
    }

    /// A graphics ROM in which tile `t`'s pixel `(x, y)` has pen `(x + y) & 0x0F`.
    ///
    /// Written by encoding the layout rule from `tiles.rs`'s module documentation
    /// *forwards*, from pen to bits — the opposite direction to `tile_pen`, which
    /// decodes. Two independent directions through one rule: a bug in either shows
    /// as a disagreement, where a fixture built by calling `tile_pen` could not
    /// disagree with it at all.
    ///
    /// ```text
    /// bit = y * (4 * FW) + 32 * (x >> 3) + (x & 7) + [24, 16, 8, 0][plane]
    /// ```
    /// Plane 0 supplies the pen's most significant bit.
    fn gfx_rom(kind: TileKind, tiles: u32) -> Vec<u8> {
        let bytes = kind.bytes();
        let mut rom = vec![0u8; bytes * tiles as usize];
        let fw = match kind {
            TileKind::Tile32x32 => 32u32,
            _ => 16,
        };
        let bias = match kind {
            TileKind::Tile8x8Odd => 32u32,
            _ => 0,
        };
        for t in 0..tiles as usize {
            for y in 0..kind.size() {
                for x in 0..kind.size() {
                    let pen = ((x + y) & 0x0F) as u8;
                    let base = y * 4 * fw + 32 * (x >> 3) + (x & 7) + bias;
                    for (p, off) in [24u32, 16, 8, 0].into_iter().enumerate() {
                        if pen & (0x08 >> p) != 0 {
                            let bit = base + off;
                            rom[t * bytes + (bit / 8) as usize] |= 0x80 >> (bit % 8);
                        }
                    }
                }
            }
        }
        rom
    }

    /// The greyscale ramp, as hand-written literals.
    ///
    /// Sixteen shades from black to white. Not `pen * 17` computed in the test —
    /// that is the implementation, and a test that recomputes it cannot fail.
    fn grey_literal(pen: u8) -> u32 {
        let v = [
            0x00u32, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
            0xEE, 0xFF,
        ][pen as usize];
        (v << 16) | (v << 8) | v
    }

    /// The interior pixel of palette swatch `n`, which is its fill.
    fn swatch_fill(buf: &[u32], n: usize) -> u32 {
        let (x, y) = pal_cell(n);
        buf[(y + 1) * WIDTH + x + 1]
    }

    /// The ramp is sixteen distinct greys, black to white, and `R == G == B`.
    ///
    /// Against `grey_literal`, which is the same ramp typed a second time from the
    /// channel values — the check [`GREYS`]'s documentation promises. A tint in one
    /// channel is the failure worth catching: it would make the browser look like a
    /// palette view, which is the one thing it must not be.
    #[test]
    fn the_ramp_is_sixteen_distinct_greys() {
        for pen in 0u8..16 {
            assert_eq!(grey(pen), grey_literal(pen), "pen {pen}");
            let c = grey(pen);
            let (r, g, b) = ((c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF);
            assert_eq!((r, g), (g, b), "pen {pen} is tinted, not grey");
            assert_eq!(c >> 24, 0, "pen {pen} sets the unused top byte");
        }
        assert_eq!(grey(0), 0x0000_0000, "pen 0 is black");
        assert_eq!(grey(15), 0x00FF_FFFF, "pen 15 is white");
        let all: std::collections::BTreeSet<u32> = (0u8..16).map(grey).collect();
        assert_eq!(all.len(), 16, "two pens share a shade");
        // Monotonic, so a brighter pen is a brighter pixel — the property that makes
        // the picture readable as a picture rather than as sixteen arbitrary colours.
        for pen in 1u8..16 {
            assert!(grey(pen) > grey(pen - 1), "pen {pen} is not brighter");
        }
        // And the high nibble is ignored, because a pen is four bits.
        assert_eq!(grey(0xF3), grey(3), "only the low four bits are the pen");
    }

    /// The tile view draws the ROM's pens as greyscale, at the cells the layout says.
    ///
    /// Pen 0 black, pen 15 white, and the ramp between — pinned as literals, because
    /// a greyscale mapping compared against the function that computes it passes with
    /// both wrong.
    #[test]
    fn a_browsed_tile_is_the_roms_pens_in_grey() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        let mut buf = frame();
        let s = ViewState {
            view: View::Tiles,
            kind: TileKind::Tile16x16,
            tile_at: 0,
            ..base_state()
        };
        draw(&mut buf, &m, &s);

        // Tile 0's cell origin, from the layout the view publishes. Pixel (x, y) of
        // it has pen `(x + y) & 0x0F`, so x + y == 0 is pen 0 and x + y == 15 is 15.
        let (ox, oy) = tile_cell(TileKind::Tile16x16, 0);
        assert_eq!(buf[oy * WIDTH + ox], grey_literal(0), "pen 0 is black");
        assert_eq!(
            buf[oy * WIDTH + ox + 15],
            grey_literal(15),
            "pen 15 is white"
        );
        assert_eq!(buf[(oy + 8) * WIDTH + ox], grey_literal(8), "pen 8 is mid");
        assert_eq!(
            buf[(oy + 4) * WIDTH + ox + 4],
            grey_literal(8),
            "(4,4) is also pen 8"
        );
    }

    /// Every tile layout has a grid with room in it, and its cells do not overlap.
    ///
    /// `tile_cell` divides by the column count, so a layout whose tiles did not fit
    /// would panic rather than draw nothing — and 32×32 is the layout with the least
    /// room. Checked for all four kinds, including the two that are the same size for
    /// different reasons.
    #[test]
    fn every_tile_layout_has_a_grid_that_fits() {
        for kind in [
            TileKind::Tile8x8,
            TileKind::Tile8x8Odd,
            TileKind::Tile16x16,
            TileKind::Tile32x32,
        ] {
            let (cols, rows) = tile_grid(kind);
            assert!(cols > 0 && rows > 0, "{kind:?} has no room: {cols}x{rows}");
            let size = kind.size() as usize;
            for slot in 0..cols * rows {
                let (x, y) = tile_cell(kind, slot);
                assert!(
                    x + size <= VX + VW - PAD && y + size <= VY + VH,
                    "{kind:?} slot {slot} at ({x}, {y}) leaves the box"
                );
                // The row label sits to the left of column 0, and must not be drawn
                // over by it.
                assert!(
                    x >= VX + PAD + TILE_LABEL * ADVANCE,
                    "{kind:?} slot {slot} covers the row label"
                );
            }
            // Adjacent cells are one pixel apart, so no tile touches its neighbour —
            // the same claim `font`'s `adjacent_glyphs_do_not_touch` makes of text.
            let (x0, _) = tile_cell(kind, 0);
            let (x1, _) = tile_cell(kind, 1);
            assert_eq!(x1 - x0, size + 1, "{kind:?} columns abut");
            let (_, ya) = tile_cell(kind, 0);
            let (_, yb) = tile_cell(kind, cols);
            assert!(yb - ya > size, "{kind:?} rows overlap");
        }
    }

    /// A tile past the end of the ROM is not drawn as a white square.
    ///
    /// `tile_pen` returns the transparent pen for a tile that is not there, and pen
    /// 15 is white — so the honest picture and the missing one would be the same
    /// picture. The grid has room for far more tiles than this eight-tile ROM holds,
    /// which is the ordinary case, not an edge one.
    #[test]
    fn a_tile_past_the_end_of_the_rom_is_marked_not_drawn() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        let mut buf = frame();
        draw(&mut buf, &m, &base_state());
        let (ox, oy) = tile_cell(TileKind::Tile16x16, 8);
        assert_eq!(buf[oy * WIDTH + ox], OFF, "tile 8 is past the end");
        assert_eq!(
            buf[(oy + 1) * WIDTH + ox + 1],
            BG,
            "and the rest of its cell is background, not a white tile"
        );
        // The premise: tile 7 *is* in the ROM and was drawn.
        let (px, py) = tile_cell(TileKind::Tile16x16, 7);
        assert_eq!(buf[py * WIDTH + px], grey_literal(0), "tile 7 is drawn");
    }

    /// A tile code near `u32::MAX` is out of the ROM, not an arithmetic panic.
    ///
    /// `tile_at` is a `u32` the user drives with the bracket keys, and `code *
    /// bytes()` overflows a `usize` on a 32-bit host well before the code does. The
    /// `checked_mul` is why this is a `false` rather than a debug-build crash.
    #[test]
    fn an_absurd_tile_code_is_simply_not_in_the_rom() {
        assert!(!tile_in_rom(0x2_0000, TileKind::Tile16x16, u32::MAX));
        assert!(!tile_in_rom(0x2_0000, TileKind::Tile32x32, u32::MAX / 2));
        // The boundary: the last tile that fits, and the first that does not.
        assert!(tile_in_rom(0x80, TileKind::Tile16x16, 0), "0x00-0x7F");
        assert!(!tile_in_rom(0x80, TileKind::Tile16x16, 1), "0x80-0xFF");
        assert!(
            !tile_in_rom(0x7F, TileKind::Tile16x16, 0),
            "a tile one byte short of complete is not in the ROM"
        );
        assert!(
            !tile_in_rom(0, TileKind::Tile8x8, 0),
            "an empty ROM holds none"
        );
    }

    /// The tile view at the end of the `u32` draws, rather than overflowing.
    ///
    /// The whole page is past the end of any ROM, so every cell is the OFF dot — and
    /// the arithmetic that reaches it saturates. Without that, `first + col` panics in
    /// a debug build and silently wraps to tile 0 in release, which is the worse of
    /// the two.
    #[test]
    fn the_tile_view_at_the_end_of_the_address_space_does_not_overflow() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &ViewState {
                tile_at: u32::MAX - 1,
                ..base_state()
            },
        );
        let (ox, oy) = tile_cell(TileKind::Tile16x16, 0);
        assert_eq!(buf[oy * WIDTH + ox], OFF, "no tile lives out here");
        assert!(
            panel_contains(&buf, "TILES", HI),
            "and the view still drew its title"
        );
    }

    /// A palette swatch is the entry's colour, through the window's own conversion.
    ///
    /// Pinned against hand-written ARGB, for the reason `pixels.rs` documents about
    /// itself: compared only against `entry_to_rgb`, this would pass with both wrong
    /// in the same direction.
    #[test]
    fn a_palette_swatch_is_the_entrys_colour() {
        // Entry 0xFFFF is full brightness, full white; 0xF00F is full-brightness
        // blue; 0x0FFF is a third-brightness white. Every value hand-computed from
        // `bright = 0x0f + ((e >> 12) << 1)`, `c = nibble * 0x11 * bright / 0x2d`.
        let m = a_machine_with_palette(&[(0, 0xFFFF), (1, 0xF00F), (2, 0x0FFF)]);
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &ViewState {
                view: View::Palette,
                ..base_state()
            },
        );

        assert_eq!(swatch_fill(&buf, 0), 0x00FF_FFFF, "0xFFFF is white");
        assert_eq!(swatch_fill(&buf, 1), 0x0000_00FF, "0xF00F is blue");
        // Third brightness: bright = 0x0f, 0x0F * 0x11 * 0x0f / 0x2d = 0x55.
        assert_eq!(swatch_fill(&buf, 2), 0x0055_5555, "0x0FFF is a third white");
    }

    /// The background pen is marked, because "the screen is a colour I did not
    /// expect" is a palette question and 0xBFF is its answer.
    #[test]
    fn the_background_pen_is_marked() {
        let m = a_machine_with_palette(&[]);
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &ViewState {
                view: View::Palette,
                ..base_state()
            },
        );
        assert!(
            panel_contains(&buf, "BG 0BFF", HI),
            "the background pen is named"
        );
    }

    /// The tilemap view shows `tile_info`'s codes at the cells the map says.
    #[test]
    fn the_tilemap_view_shows_the_tables_codes() {
        // gfxram with scroll 2's table at word 0 and a known code at map (3, 1).
        let mut m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        let i = 2 * Layer::Scroll2.scan(3, 1);
        m.board.gfxram[i] = 0x0123;
        m.board.gfxram[i + 1] = 0x0045; // colour 5, no flip, group 0
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &ViewState {
                view: View::Tilemap,
                layer: Layer::Scroll2,
                map_at: Some((3, 1)),
                ..base_state()
            },
        );
        assert!(panel_contains(&buf, "0123", HI), "the cursored code");
        assert!(panel_contains(&buf, "COL 45", FG), "and its colour scheme");
    }

    /// A code no bank range covers reads `----`, not tile 0.
    ///
    /// The one failure the picture cannot show: `draw_tilemap` skips an unmapped
    /// tile silently, which is correct and undiagnosable. A viewer that showed the
    /// mapper's `None` as 0 would send you looking at tile 0.
    #[test]
    fn an_unmapped_code_is_not_shown_as_tile_zero() {
        let mut m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        // Scroll 2's only STF29 range is 0x5000-0x7FFF in 8×8 units, and a scroll-2
        // code is shifted left one to reach them — so 0xFFFF becomes 0x1FFFE and no
        // range covers it. That is also why SF2's scroll 2 draws from the middle of
        // the sprite ROM: its codes are small and its range is not.
        let i = 2 * Layer::Scroll2.scan(0, 0);
        m.board.gfxram[i] = 0xFFFF;
        assert_eq!(
            m.video.mapper.map(GfxType::Scroll2, 0xFFFF),
            None,
            "the premise: this code really is unmapped"
        );
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &ViewState {
                view: View::Tilemap,
                layer: Layer::Scroll2,
                map_at: Some((0, 0)),
                ..base_state()
            },
        );
        assert!(panel_contains(&buf, "ROM ----", OFF), "an unmapped code");
        assert!(
            !panel_contains(&buf, "ROM 0000", OFF),
            "and not shown as tile 0"
        );
    }

    /// The cursor's default is the tile the renderer draws at the top-left pixel.
    ///
    /// **Not asserted by calling `map_axis` twice** — that would compare the view to
    /// itself. The scroll is set so the renderer fetches a known map position for
    /// visible pixel (0, 0), and the cursor is required to name it. The scroll is
    /// negative, which is the case that separates `div_euclid` from `/`: with
    /// truncating division the answer is tile 0.
    #[test]
    fn the_cursor_follows_the_tile_at_the_visible_top_left() {
        let mut m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        // Scroll 2 x = -80, y = -32. Visible pixel (0, 0) is raster
        // (VISIBLE_X, VISIBLE_Y) = (64, 16), so the map position is
        // (64 - 80, 16 - 32) = (-16, -16) — one tile left and one tile up of the
        // origin, which after the wrap at 64 is map tile (63, 63).
        m.board.cps_a[SCROLL2_X] = (-80i16) as u16;
        m.board.cps_a[SCROLL2_Y] = (-32i16) as u16;
        assert_eq!(
            map_origin(&m, Layer::Scroll2),
            (63, 63),
            "one tile left and up of the origin, wrapped"
        );
        // And the truncating answer, which is what a re-derived viewer produces.
        assert_ne!(map_origin(&m, Layer::Scroll2), (0, 0));
        // The view with no cursor of its own shows that position.
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &ViewState {
                view: View::Tilemap,
                layer: Layer::Scroll2,
                map_at: None,
                ..base_state()
            },
        );
        assert!(panel_contains(&buf, "MAP 63,63", HI), "and it is on screen");
    }

    /// A whole number of map spans fits in the scroll registers' range.
    ///
    /// The precondition behind [`map_origin`]'s "no test can tell `as i16` from
    /// `as i32`": the two readings differ by 65536, so they agree exactly when 65536
    /// is a multiple of every layer's map span. Asserted here rather than left as a
    /// remark, because it is the claim the remark rests on — if a later board gave a
    /// layer a map that did not divide it, the sign of the scroll would start
    /// mattering to the cursor and the comment would be quietly wrong.
    ///
    /// The offsets are checked too, not just the tiles: `map_axis` returns both, and
    /// a span dividing the range settles the tile without saying anything about the
    /// pixel within it.
    #[test]
    fn a_map_span_divides_the_register_range() {
        for layer in [Layer::Scroll1, Layer::Scroll2, Layer::Scroll3] {
            let span = MAP_TILES * layer.tile_edge();
            assert_eq!(
                65536 % span,
                0,
                "{layer:?}: {span} pixels of map must divide the 16-bit range"
            );
        }
        // And the consequence, at the register values where the two readings are
        // furthest apart: the largest negative scroll, and the value either side of
        // the sign boundary.
        for layer in [Layer::Scroll1, Layer::Scroll2, Layer::Scroll3] {
            let edge = layer.tile_edge();
            for reg in [0x8000u16, 0x8001, 0xFFC0, 0xFFFF, 0x7FFF, 0xC000] {
                let signed = VISIBLE_X + i32::from(reg as i16);
                let unsigned = VISIBLE_X + i32::from(reg);
                assert_eq!(
                    map_axis(edge, signed),
                    map_axis(edge, unsigned),
                    "{layer:?} at scroll {reg:#06X}: the wrap hides the sign"
                );
            }
        }
    }

    /// The layers view's enable column is the renderer's answer, not its own.
    ///
    /// Disabling scroll 1 through the registers must change both the view's cell and
    /// the drawn frame. A view that re-derived "is scroll 1 enabled" could pass the
    /// first half and fail the second, which is the whole reason `layer_enabled` is
    /// public.
    #[test]
    fn the_layers_view_agrees_with_the_renderer() {
        let mut m = a_machine(gfx_rom(TileKind::Tile8x8, 8));
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &ViewState {
                view: View::Layers,
                ..base_state()
            },
        );
        assert!(panel_contains(&buf, "S1 ON", FG), "enabled in hardware");

        // Scroll 1's layer-control bit is `layer_enable_mask[0]` = 0x08 on SF2.
        m.board.cps_b[VideoConfig::sf2().layer_control] &= !0x08;
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &ViewState {
                view: View::Layers,
                ..base_state()
            },
        );
        assert!(panel_contains(&buf, "S1 OFF", OFF), "and now disabled");

        // And scroll 2's *second* gate: videocontrol bit 2, which the layer control
        // knows nothing about. Both halves above move the one bit a re-derived
        // "layercontrol & 0x08" would get right, so neither can tell the two apart —
        // a viewer that read only the layer control would report this layer as on
        // while the renderer draws nothing of it, which is the exact case
        // `layer_enabled` is public to prevent.
        m.board.cps_b[VideoConfig::sf2().layer_control] |= 0x08;
        m.board.cps_a[VIDEOCONTROL] &= !0x0004;
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &ViewState {
                view: View::Layers,
                ..base_state()
            },
        );
        assert!(panel_contains(&buf, "S1 ON", FG), "scroll 1 is on again");
        assert!(
            panel_contains(&buf, "S2 OFF", OFF),
            "and scroll 2 is off through videocontrol, not the layer control"
        );
    }

    /// Sprites read `ALWAYS`, because CPS-1 has no sprite enable bit.
    ///
    /// A fact about the hardware, on the screen, because "why can I not turn the
    /// sprites off in hardware" is the question the table otherwise invites.
    #[test]
    fn the_sprites_have_no_hardware_enable() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &ViewState {
                view: View::Layers,
                ..base_state()
            },
        );
        assert!(panel_contains(&buf, "OB ALWAYS", FG));
    }

    /// The mask column is the mask, and the two are independent of the hardware's.
    #[test]
    fn the_mask_column_is_the_masks_own_answer() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            &ViewState {
                view: View::Layers,
                mask: LayerMask {
                    scroll2: false,
                    ..LayerMask::all()
                },
                ..base_state()
            },
        );
        // Scroll 2 is on in hardware and off in the mask, which is the whole point
        // of two columns: one says what the guest asked for, the other what you did.
        assert!(panel_contains(&buf, "S2 ON     MSK OFF", FG));
        assert!(panel_contains(&buf, "S1 ON     MSK ON", FG));
    }

    /// Every view stays inside the frame and inside its box.
    ///
    /// The same claim `overlay`'s `a_panel_leaves_the_rest_of_the_frame_alone`
    /// makes, and for the same reason: a view that ran one pixel past its box would
    /// look like a rendering bug in the game.
    ///
    /// **The border is checked against literal twos, not against [`VX`] and [`VW`].**
    /// The loop below asks whether a view stayed inside whatever the box currently is,
    /// which is a claim that cannot fail when the box itself is what moved: widen `VW`
    /// to `WIDTH` and the loop's own `inside` widens with it. The two-pixel border is
    /// the visible promise — it is what keeps the viewer from looking like the game
    /// broke — so it is pinned as a number here and derived nowhere.
    #[test]
    fn every_view_stays_inside_its_box() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 64));
        for view in [View::Tiles, View::Tilemap, View::Palette, View::Layers] {
            let mut buf = vec![0x00FF_00FFu32; WIDTH * HEIGHT];
            draw(
                &mut buf,
                &m,
                &ViewState {
                    view,
                    ..base_state()
                },
            );
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let inside = (VX..VX + VW).contains(&x) && (VY..VY + VH).contains(&y);
                    if !inside {
                        assert_eq!(
                            buf[y * WIDTH + x],
                            0x00FF_00FF,
                            "{view:?} touched ({x}, {y}), outside its box"
                        );
                    }
                    // And the border, in numbers of its own.
                    if !(2..WIDTH - 2).contains(&x) || !(2..HEIGHT - 2).contains(&y) {
                        assert_eq!(
                            buf[y * WIDTH + x],
                            0x00FF_00FF,
                            "{view:?} touched ({x}, {y}), inside the two-pixel border"
                        );
                    }
                }
            }
        }
    }

    /// A view draws something. The premise every assertion above rests on.
    #[test]
    fn every_view_draws_something() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 64));
        for view in [View::Tiles, View::Tilemap, View::Palette, View::Layers] {
            let mut buf = frame();
            draw(
                &mut buf,
                &m,
                &ViewState {
                    view,
                    ..base_state()
                },
            );
            assert!(buf.iter().any(|&p| p != 0), "{view:?} drew nothing at all");
            assert!(
                panel_contains(&buf, view.name(), HI),
                "{view:?} names itself on its title line"
            );
        }
    }

    /// Cycling the view reaches all four and returns.
    #[test]
    fn cycling_the_view_visits_all_four() {
        let mut v = View::Tiles;
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(v);
            v = v.cycled();
        }
        assert_eq!(v, View::Tiles, "four steps return to the start");
        assert_eq!(
            seen,
            vec![View::Tiles, View::Tilemap, View::Palette, View::Layers]
        );
    }

    /// Drawing a view does not disturb the machine.
    ///
    /// Every entry point takes `&Cps1`, which the compiler enforces — and the
    /// tilemap view reads memory, so the behavioural claim is worth making too:
    /// `peek_word`'s trap is that a `&mut self` read would acknowledge an interrupt,
    /// and a view that reached for the bus instead of gfxram would do the same.
    #[test]
    fn drawing_a_view_does_not_disturb_the_machine() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 64));
        let before = (
            m.total_cycles,
            m.board.trace.acks,
            m.board.trace.unmapped_reads.total(),
            m.cpu.pc,
        );
        let mut buf = frame();
        for view in [View::Tiles, View::Tilemap, View::Palette, View::Layers] {
            draw(
                &mut buf,
                &m,
                &ViewState {
                    view,
                    ..base_state()
                },
            );
        }
        assert_eq!(
            before,
            (
                m.total_cycles,
                m.board.trace.acks,
                m.board.trace.unmapped_reads.total(),
                m.cpu.pc,
            ),
            "a view read the machine through something with side effects"
        );
    }
}
