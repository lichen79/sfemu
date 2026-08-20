//! The graphics viewer's state: which view, where it is looking, what is masked.
//!
//! # Why the state is here and not in the loop
//!
//! `debug.rs`'s reasoning, unchanged: the loop owns a window, a clock and a file
//! system, and every decision this module makes is testable without any of them.
//! What the loop does with a [`GfxViewer`] is four calls and no arithmetic.
//!
//! # Three keys with four meanings each
//!
//! `Enter`, `[` and `]` mean something different in each view, which is what keeps
//! the key count to five for four views. The dispatch is one `match` per key over
//! [`gfxpanels::View`], with every arm written out — including the arms that do
//! nothing, named as such, because a silent `_ => {}` reads as an oversight rather
//! than as a decision.
//!
//! # The mask outlives the box
//!
//! Hiding the viewer with `F9` does not restore the layers you subtracted. "Show me
//! the game with scroll 1 off" is the whole point of a layer mask, and it is
//! unreachable if closing the box turns everything back on. `F9` hides a box; the
//! layers view is what changes layers.
//!
//! # Nothing here writes to the machine
//!
//! Every entry point takes `&Machine`. The viewer reads the scroll registers through
//! [`gfxpanels::map_origin`] or [`crate::sf1panels::map_origin`], and each board's
//! graphics regions through its own views, and that is all. `&Machine` rather than
//! `&Cps1` is also why there is no `as_cps1()` on `Machine`: a viewer that silently
//! did nothing on one board is a panel that goes blank with no error.

use crate::gfxpanels::{self, View, ViewState};
use crate::keys::Actions;
use crate::sf1panels::Sf1ViewState;
use machine::video::compose::LayerMask;
use machine::video::layers::{Layer, MAP_TILES};
use machine::video::palette::PENS;
use machine::video::sf1::tilemap::MapKind;
use machine::video::sf1::{LayerMask as Sf1LayerMask, Plane};
use machine::video::tiles::TileKind;
use machine::Machine;

/// How far `[` and `]` move the palette cursor: one row of swatches.
///
/// A row rather than one entry, because the palette is 3072 entries on CPS-1 and
/// 1024 on SF1, and stepping through either one at a time is not navigation. Both
/// boards' colour schemes are 16 pens wide, so a 64-wide row is four of them — and
/// `PEN_GRANULARITY` is not the right step here for the same reason: you are looking
/// for a *scheme*, and they sit in blocks.
const PAL_PAGE: usize = 64;

/// SF1's palette, for the same wrap CPS-1 does at [`PENS`].
///
/// A third of CPS-1's, so a shared constant would let `]` walk two thirds of the way
/// off the end of SF1's palette and show sixteen rows of entry 0.
const SF1_PENS: usize = machine::video::sf1::palette::ENTRIES;

/// The graphics viewer's whole state.
///
/// `on` is separate from the view: hiding and showing must not lose where you were
/// looking, because `F9` is how you compare the viewer's answer against the game's
/// own screen and you will press it repeatedly.
///
/// # Why `view` is hoisted out of the two states
///
/// The two boards' cursors are genuinely separate — a CPS-1 `TileKind` means nothing
/// to SF1 and an SF1 `Plane` means nothing to CPS-1 — but *which view* is chrome.
/// With a `view` inside each state, `F10` on one board would leave the other looking
/// somewhere else, and a save state loaded across a board change would open a
/// different panel than the one you were reading. One field cannot desynchronise, and
/// [`GfxViewer::view`] keeps its no-argument signature because of it.
#[derive(Debug, Clone)]
pub struct GfxViewer {
    /// Whether the box is drawn.
    on: bool,
    /// Which view is shown, on either board.
    view: View,
    /// Where CPS-1's four views are looking.
    cps1: ViewState,
    /// Where SF1's four views are looking.
    sf1: Sf1ViewState,
}

impl Default for GfxViewer {
    fn default() -> Self {
        Self::new()
    }
}

impl GfxViewer {
    /// A hidden viewer looking at the tile browser, with every layer permitted.
    ///
    /// Hidden because the box covers the whole screen, and a viewer that appeared
    /// uninvited would make the emulator look broken on first run — the same
    /// reasoning `Debugger::new` carries.
    ///
    /// The mask starts at `LayerMask::all()`, which is the identity: a fresh viewer
    /// must not change a single pixel of the game.
    pub fn new() -> Self {
        Self {
            on: false,
            view: View::Tiles,
            cps1: ViewState {
                view: View::Tiles,
                kind: TileKind::Tile16x16,
                layer: Layer::Scroll2,
                tile_at: 0,
                pal_at: 0,
                map_at: None,
                row: 0,
                mask: LayerMask::all(),
            },
            sf1: Sf1ViewState {
                view: View::Tiles,
                plane: Plane::Bg,
                map: MapKind::Bg,
                tile_at: 0,
                pal_at: 0,
                map_at: None,
                row: 0,
                mask: Sf1LayerMask::all(),
            },
        }
    }

    /// Whether the box is drawn.
    pub const fn shown(&self) -> bool {
        self.on
    }

    /// Which view is shown, on either board.
    pub const fn view(&self) -> View {
        self.view
    }

    /// Everything CPS-1's views read.
    ///
    /// ⚠️ By value, not by reference: `view` is the viewer's field and the two
    /// states', so this composes the answer rather than pointing at a stored one. A
    /// `&ViewState` would have to point at a `view` that could be stale.
    pub const fn state(&self) -> ViewState {
        ViewState {
            view: self.view,
            ..self.cps1
        }
    }

    /// Everything SF1's views read.
    pub const fn sf1_state(&self) -> Sf1ViewState {
        Sf1ViewState {
            view: self.view,
            ..self.sf1
        }
    }

    /// The mask the loop must give a CPS-1's `Video`.
    ///
    /// Never enables anything: `LayerMask::all()` is the start and every toggle
    /// clears or restores one bit. The hardware's own `&&` in `Video::render` is what
    /// makes that structural — see `compose`'s `the_mask_can_only_subtract`.
    pub const fn mask(&self) -> LayerMask {
        self.cps1.mask
    }

    /// The mask the loop must give an `Sf1Video`.
    ///
    /// The same "subtracts only" guarantee, over SF1's own four fields — see
    /// `Plane::permitted` and `Sf1Video::render`'s `&&`.
    pub const fn sf1_mask(&self) -> Sf1LayerMask {
        self.sf1.mask
    }

    /// Applies one frame's viewer keys. Returns whether the viewer is shown.
    ///
    /// `m` is read, never written: [`gfxpanels::map_origin`] and
    /// [`crate::sf1panels::map_origin`] read the scroll registers when a tilemap
    /// cursor leaves the beam, and a region's length bounds the tile view's paging.
    /// Nothing else.
    pub fn update(&mut self, a: &Actions, m: &Machine) -> bool {
        if a.gfx_toggled {
            self.on = !self.on;
        }
        // A hidden viewer ignores its own keys, so `]` does not silently page a view
        // you cannot see — and so the game's own `[`/`]` remain free. The mask is
        // *not* touched here: see this module's documentation.
        if !self.on {
            return false;
        }
        // The view cycles before the board is looked at, because it is the one piece
        // of state both boards share.
        if a.gfx_view_cycled {
            self.view = self.view.cycled();
        }
        if a.gfx_back {
            self.step(m, false);
        }
        if a.gfx_forward {
            self.step(m, true);
        }
        if a.gfx_act {
            self.act(m);
        }
        true
    }

    /// `Enter`: one key, four meanings per board, one per view.
    fn act(&mut self, m: &Machine) {
        match m {
            Machine::Cps1(_) => match self.view {
                View::Tiles => self.cps1.kind = next_kind(self.cps1.kind),
                View::Tilemap => {
                    // A new layer's tiles are a different size, so a cell index from
                    // the old one means nothing. Back to following the beam.
                    self.cps1.layer = next_layer(self.cps1.layer);
                    self.cps1.map_at = None;
                }
                // The palette's cursor is moved by `[`/`]` and there is nothing else
                // to act on — the view has one axis. Deliberately nothing, and named
                // so, because a silent `_ => {}` reads as an oversight.
                View::Palette => {}
                View::Layers => self.toggle_row(),
            },
            Machine::Sf1(_) => match self.view {
                // Four regions, not four tile sizes: each SF1 plane has a fixed
                // layout, so cycling the plane is what cycling the kind was.
                View::Tiles => self.sf1.plane = self.sf1.plane.cycled(),
                View::Tilemap => {
                    // A new map has different dimensions, so a cell from the old one
                    // means nothing — BG's (2000, 3) is not a cell of TX at all.
                    self.sf1.map = self.sf1.map.cycled();
                    self.sf1.map_at = None;
                }
                View::Palette => {}
                View::Layers => self.toggle_sf1_row(),
            },
        }
    }

    /// `[` and `]`: also one meaning per view, per board.
    fn step(&mut self, m: &Machine, forward: bool) {
        match m {
            Machine::Cps1(c) => match self.view {
                View::Tiles => {
                    let (cols, rows) = gfxpanels::tile_grid(self.cps1.kind);
                    self.cps1.tile_at = paged(self.cps1.tile_at, (cols * rows) as u32, forward);
                }
                View::Tilemap => {
                    // The first move materialises the cursor from wherever the beam
                    // is — the same "`None` is not a value" step `Debugger::scroll`
                    // takes.
                    let (col, r) = self
                        .cps1
                        .map_at
                        .unwrap_or_else(|| gfxpanels::map_origin(c, self.cps1.layer));
                    // Wrapping, because the map itself wraps at 64: this is a
                    // coordinate in a torus, not an address.
                    let col = if forward {
                        (col + 1) % MAP_TILES
                    } else {
                        (col + MAP_TILES - 1) % MAP_TILES
                    };
                    self.cps1.map_at = Some((col, r));
                }
                View::Palette => {
                    self.cps1.pal_at = if forward {
                        (self.cps1.pal_at + PAL_PAGE) % PENS
                    } else {
                        (self.cps1.pal_at + PENS - PAL_PAGE) % PENS
                    };
                }
                // Four rows, so both directions wrap — a selection that stuck at the
                // ends would need two keys to reach the row you can see is there.
                View::Layers => {
                    self.cps1.row = if forward {
                        (self.cps1.row + 1) % ROWS
                    } else {
                        (self.cps1.row + ROWS - 1) % ROWS
                    };
                }
            },
            Machine::Sf1(s) => match self.view {
                View::Tiles => {
                    let (cols, rows) = crate::sf1panels::tile_grid(self.sf1.plane);
                    self.sf1.tile_at = paged(self.sf1.tile_at, (cols * rows) as u32, forward);
                }
                View::Tilemap => {
                    // ⚠️ Each map's own width, not a shared constant: BG and FG are
                    // 2,048 columns and TX is 64. A `% MAP_TILES` here would wrap the
                    // background at 64 and make 31/32 of the map unreachable.
                    let cols = self.sf1.map.map().cols;
                    let (col, r) = self
                        .sf1
                        .map_at
                        .unwrap_or_else(|| crate::sf1panels::map_origin(s, self.sf1.map));
                    let col = if forward {
                        (col + 1) % cols
                    } else {
                        // ⚠️ `- 1 % cols` and not `- 1`: a one-column map would
                        // underflow `col + cols - 1` at col 0 — the same guard
                        // `sf1panels`' window wrap carries.
                        (col + cols - 1 % cols) % cols
                    };
                    self.sf1.map_at = Some((col, r));
                }
                View::Palette => {
                    self.sf1.pal_at = if forward {
                        (self.sf1.pal_at + PAL_PAGE) % SF1_PENS
                    } else {
                        (self.sf1.pal_at + SF1_PENS - PAL_PAGE) % SF1_PENS
                    };
                }
                View::Layers => {
                    self.sf1.row = if forward {
                        (self.sf1.row + 1) % ROWS
                    } else {
                        (self.sf1.row + ROWS - 1) % ROWS
                    };
                }
            },
        }
    }

    /// Subtracts, or restores, the selected row's layer, on CPS-1.
    ///
    /// Row 0 is the sprites, because that is `layer_order`'s value 0 and the layers
    /// view lists them in that order.
    fn toggle_row(&mut self) {
        let m = &mut self.cps1.mask;
        match self.cps1.row {
            0 => m.sprites = !m.sprites,
            1 => m.scroll1 = !m.scroll1,
            2 => m.scroll2 = !m.scroll2,
            // The row index comes from `step`, which is `% ROWS`, so this is row 3 and
            // not a fallback. Written as `_` because a `3 =>` arm would need an
            // unreachable one beside it, and that is worse.
            _ => m.scroll3 = !m.scroll3,
        }
    }

    /// Subtracts, or restores, the selected row's plane, on SF1.
    ///
    /// ⚠️ Row 0 is the *background*, not the sprites. SF1's drawing order is fixed at
    /// BG, FG, OB, TX, which is `Plane::ALL`'s order and the layers panel's row
    /// order; CPS-1's row 0 is the sprites because `layer_order`'s value 0 is. The
    /// two boards' row 0 mean different things and the panels label them.
    fn toggle_sf1_row(&mut self) {
        let m = &mut self.sf1.mask;
        match self.sf1.row {
            0 => m.bg = !m.bg,
            1 => m.fg = !m.fg,
            2 => m.sprites = !m.sprites,
            _ => m.tx = !m.tx,
        }
    }

    /// Draws the current view of whichever board this is, if shown.
    pub fn draw(&self, buf: &mut [u32], m: &Machine) {
        if !self.on {
            return;
        }
        match m {
            Machine::Cps1(c) => gfxpanels::draw(buf, c, &self.state()),
            Machine::Sf1(s) => crate::sf1panels::draw(buf, s, &self.sf1_state()),
        }
    }
}

/// Rows in the layers view: the sprites and three scrolls.
const ROWS: usize = 4;

/// `at` moved one page of `page` tiles, saturating at both ends.
///
/// Saturating, not wrapping: the end of the ROM is a place you stop, and wrapping to
/// tile 0 there reads as the view resetting itself. `gfxpanels` draws whatever is out
/// past the ROM as "not in the ROM", so landing there is honest rather than a bug.
///
/// A function rather than two lines inline, so the ends can be tested at all: reaching
/// `u32::MAX` by pressing `]` is four billion presses, and a plain `+` there panics in
/// a debug build and wraps to a real tile in release — the worse of the two.
const fn paged(at: u32, page: u32, forward: bool) -> u32 {
    if forward {
        at.saturating_add(page)
    } else {
        at.saturating_sub(page)
    }
}

/// The next tile layout, wrapping.
///
/// All four, including `Tile8x8Odd` — which is not decoration: it is the layout SF2's
/// scroll 1 uses, and a browser that skipped it would show scroll 1's tiles at the
/// wrong x bias and look like a decoder bug.
///
/// A `match`, not an `as`-cast round-trip, for the reason `Key::bit` gives: a cast
/// silently maps a new variant to nothing.
const fn next_kind(kind: TileKind) -> TileKind {
    match kind {
        TileKind::Tile8x8 => TileKind::Tile8x8Odd,
        TileKind::Tile8x8Odd => TileKind::Tile16x16,
        TileKind::Tile16x16 => TileKind::Tile32x32,
        TileKind::Tile32x32 => TileKind::Tile8x8,
    }
}

/// The next scroll layer, wrapping.
const fn next_layer(layer: Layer) -> Layer {
    match layer {
        Layer::Scroll1 => Layer::Scroll2,
        Layer::Scroll2 => Layer::Scroll3,
        Layer::Scroll3 => Layer::Scroll1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use machine::config::BoardConfig;
    use machine::timing::Timing;
    use machine::video::{HEIGHT, WIDTH};

    /// A booted CPS-1 machine with a small graphics ROM.
    ///
    /// Boxed inside the enum: a `Cps1` is half a megabyte by value, and returning one
    /// through a fixture overflows a test thread's stack — which it did, in
    /// `gfxpanels`, as a `SIGABRT` with no failing assertion.
    fn a_machine() -> Machine {
        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x1000..0x1002].copy_from_slice(&[0x60, 0xFE]);
        let mut m = Box::new(machine::Cps1::with_gfx(
            &rom,
            vec![0u8; 0x4000],
            BoardConfig::sf2(),
            Timing::cps1_10mhz(),
        ));
        m.reset();
        Machine::Cps1(m)
    }

    /// The `Cps1` inside a `Machine` built by [`a_machine`].
    ///
    /// Test-only, and deliberately *not* a method on `Machine`: an
    /// `as_cps1() -> Option<&Cps1>` in the library would let production code write
    /// `if let Some(c) = m.as_cps1() { … }` and silently do nothing on SF1 — a panel
    /// that goes blank on one board with no error. Here the panic is the point.
    ///
    /// ⚠️ Call this per use, never bound once across a test body: a
    /// `let c = cps1_mut(&mut m);` held across a body holds `m` mutably, and the
    /// `&Machine` the viewer needs is then rejected (E0502).
    fn cps1(m: &Machine) -> &machine::Cps1 {
        match m {
            Machine::Cps1(c) => c,
            Machine::Sf1(_) => unreachable!("a_machine builds a Cps1"),
        }
    }

    /// Ditto, mutably, for the two tests that set a scroll register.
    fn cps1_mut(m: &mut Machine) -> &mut machine::Cps1 {
        match m {
            Machine::Cps1(c) => c,
            Machine::Sf1(_) => unreachable!("a_machine builds a Cps1"),
        }
    }

    /// A reset SF1 wrapped in a `Machine`, with every plane enabled.
    fn an_sf1_machine() -> Machine {
        use machine::video::sf1::Sf1Video;
        let mut prog = vec![0u8; 0x2000];
        prog[0..4].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00]);
        prog[4..8].copy_from_slice(&[0x00, 0x00, 0x10, 0x00]);
        prog[0x1000..0x1002].copy_from_slice(&[0x60, 0xFE]);
        let mut m = machine::Sf1::new(
            &prog,
            Sf1Video::new(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()),
            vec![0x18, 0xFE],
            vec![0x00, 0x18, 0xFE],
        );
        m.reset();
        m.board.active = 0x20 | 0x40 | 0x80 | 0x08;
        Machine::Sf1(Box::new(m))
    }

    /// An `Actions` with one field set.
    ///
    /// Set directly rather than through a `KeySet`, unlike `debug.rs`'s `pressing`:
    /// `keys.rs` already pins which key produces which field, and
    /// `a_held_viewer_key_acts_once` below is what checks this module is driven by the
    /// edge rather than the level.
    fn act(f: impl FnOnce(&mut Actions)) -> Actions {
        let mut a = Actions::default();
        f(&mut a);
        a
    }

    /// A shown viewer, since almost every test needs one.
    fn shown(m: &Machine) -> GfxViewer {
        let mut g = GfxViewer::new();
        g.update(&act(|a| a.gfx_toggled = true), m);
        assert!(g.shown(), "the premise: F9 showed it");
        g
    }

    /// A shown viewer looking at `view`.
    fn looking_at(m: &Machine, view: View) -> GfxViewer {
        let mut g = shown(m);
        for _ in 0..4 {
            if g.view() == view {
                return g;
            }
            g.update(&act(|a| a.gfx_view_cycled = true), m);
        }
        panic!("four cycles did not reach {view:?}");
    }

    /// `F9` shows and hides it, and it starts hidden.
    #[test]
    fn the_viewer_starts_hidden_and_f9_toggles_it() {
        let m = a_machine();
        let mut g = GfxViewer::new();
        assert!(!g.shown(), "hidden on first run");
        assert!(g.update(&act(|a| a.gfx_toggled = true), &m));
        assert!(g.shown());
        assert!(!g.update(&act(|a| a.gfx_toggled = true), &m));
        assert!(!g.shown());
    }

    /// A fresh viewer's mask is the identity, so it cannot change a pixel.
    #[test]
    fn a_fresh_viewer_masks_nothing() {
        let g = GfxViewer::new();
        assert_eq!(g.mask(), LayerMask::all(), "every layer permitted");
        assert_eq!(GfxViewer::default().mask(), LayerMask::all());
    }

    /// `F10` cycles all four views and returns to the first.
    #[test]
    fn f10_cycles_the_four_views_and_comes_back() {
        let m = a_machine();
        let mut g = shown(&m);
        let mut seen = vec![g.view()];
        for _ in 0..4 {
            g.update(&act(|a| a.gfx_view_cycled = true), &m);
            seen.push(g.view());
        }
        assert_eq!(seen[4], seen[0], "four cycles return to the start");
        let distinct: std::collections::BTreeSet<_> =
            seen[..4].iter().map(|v| format!("{v:?}")).collect();
        assert_eq!(distinct.len(), 4, "and the four are distinct: {seen:?}");
    }

    /// `Enter` acts on the view you are looking at, and on no other.
    ///
    /// The one key with four meanings. A dispatch that acted on the wrong view would
    /// cycle a tile kind while you were looking at the palette, which reads as the
    /// key doing nothing.
    #[test]
    fn enter_acts_on_the_current_view_only() {
        let m = a_machine();
        let mut g = shown(&m);
        // On Tiles, Enter cycles the tile kind and touches nothing else.
        let before = (g.state().layer, g.state().row, g.state().mask);
        g.update(&act(|a| a.gfx_act = true), &m);
        assert_ne!(g.state().kind, TileKind::Tile16x16, "the kind moved");
        assert_eq!(
            (g.state().layer, g.state().row, g.state().mask),
            before,
            "and nothing else did"
        );

        // On Layers, Enter toggles the selected row's mask bit.
        let mut g = looking_at(&m, View::Layers);
        let kind = g.state().kind;
        g.update(&act(|a| a.gfx_act = true), &m);
        assert_ne!(g.mask(), LayerMask::all(), "a mask bit toggled");
        assert_eq!(g.state().kind, kind, "and the tile kind did not move");

        // On Palette, Enter is deliberately nothing — the view has one axis.
        let mut g = looking_at(&m, View::Palette);
        let before = format!("{:?}", g.state());
        g.update(&act(|a| a.gfx_act = true), &m);
        assert_eq!(format!("{:?}", g.state()), before, "the palette has no act");
    }

    /// `Enter` on the tile view reaches all four layouts, including `Tile8x8Odd`.
    ///
    /// The odd one is the layout SF2's scroll 1 uses. A cycle that skipped it would
    /// draw scroll 1's tiles at the wrong x bias, which looks like a decoder bug in
    /// the emulator rather than a missing arm in a viewer.
    #[test]
    fn enter_reaches_every_tile_layout() {
        let m = a_machine();
        let mut g = shown(&m);
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(format!("{:?}", g.state().kind));
            g.update(&act(|a| a.gfx_act = true), &m);
        }
        assert_eq!(
            g.state().kind,
            TileKind::Tile16x16,
            "four presses return to the start"
        );
        seen.sort();
        assert_eq!(
            seen,
            vec!["Tile16x16", "Tile32x32", "Tile8x8", "Tile8x8Odd"],
            "all four layouts"
        );
    }

    /// `Enter` on the tilemap reaches all three layers and returns.
    #[test]
    fn enter_reaches_every_scroll_layer() {
        let m = a_machine();
        let mut g = looking_at(&m, View::Tilemap);
        let first = g.state().layer;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(format!("{:?}", g.state().layer));
            g.update(&act(|a| a.gfx_act = true), &m);
        }
        assert_eq!(g.state().layer, first, "three presses return");
        seen.sort();
        assert_eq!(seen, vec!["Scroll1", "Scroll2", "Scroll3"]);
    }

    /// The mask only ever subtracts, whatever the keys do.
    ///
    /// `GfxViewer` cannot produce a mask that enables something: `all()` is the start
    /// and every toggle clears or restores a bit. Exhausting the four rows must reach
    /// all-off and, pressed again, exactly `all()`.
    #[test]
    fn the_viewer_can_only_subtract() {
        let m = a_machine();
        let mut g = looking_at(&m, View::Layers);
        for _ in 0..4 {
            g.update(&act(|a| a.gfx_act = true), &m);
            g.update(&act(|a| a.gfx_forward = true), &m);
        }
        assert_eq!(
            g.mask(),
            LayerMask {
                sprites: false,
                scroll1: false,
                scroll2: false,
                scroll3: false
            },
            "all four subtracted"
        );
        for _ in 0..4 {
            g.update(&act(|a| a.gfx_act = true), &m);
            g.update(&act(|a| a.gfx_forward = true), &m);
        }
        assert_eq!(g.mask(), LayerMask::all(), "and restored");
    }

    /// Each row toggles its own layer and no other.
    ///
    /// Four rows and four bits is exactly the table that is right in three places and
    /// wrong in the fourth — and the wrong one would subtract a layer you can still
    /// see, which reads as the mask not working at all.
    #[test]
    fn each_row_subtracts_its_own_layer() {
        let m = a_machine();
        let want = [
            LayerMask {
                sprites: false,
                ..LayerMask::all()
            },
            LayerMask {
                scroll1: false,
                ..LayerMask::all()
            },
            LayerMask {
                scroll2: false,
                ..LayerMask::all()
            },
            LayerMask {
                scroll3: false,
                ..LayerMask::all()
            },
        ];
        for (row, expect) in want.into_iter().enumerate() {
            let mut g = looking_at(&m, View::Layers);
            for _ in 0..row {
                g.update(&act(|a| a.gfx_forward = true), &m);
            }
            assert_eq!(g.state().row, row, "the selection reached row {row}");
            g.update(&act(|a| a.gfx_act = true), &m);
            assert_eq!(g.mask(), expect, "row {row}");
        }
    }

    /// The row selection wraps in both directions.
    #[test]
    fn the_row_selection_wraps_both_ways() {
        let m = a_machine();
        let mut g = looking_at(&m, View::Layers);
        assert_eq!(g.state().row, 0);
        g.update(&act(|a| a.gfx_back = true), &m);
        assert_eq!(g.state().row, 3, "back off row 0 is the last row");
        g.update(&act(|a| a.gfx_forward = true), &m);
        assert_eq!(g.state().row, 0, "and forward again");
    }

    /// The tilemap cursor follows the beam until you move it, then stops.
    ///
    /// `Option<(u32, u32)>` and not a pair kept equal to the origin: the same
    /// distinction `Debugger::disasm_at` documents. A cursor moved to a cell that
    /// happens to be the origin must not resume following, or scrolling the layer
    /// would yank the view away from the tile you were reading.
    #[test]
    fn the_tilemap_cursor_stops_following_once_moved() {
        let m = a_machine();
        let mut g = looking_at(&m, View::Tilemap);
        assert_eq!(g.state().map_at, None, "following the beam");
        g.update(&act(|a| a.gfx_forward = true), &m);
        assert!(g.state().map_at.is_some(), "and now pinned");
    }

    /// The first cursor move starts from where the beam is, not from cell zero.
    ///
    /// The `unwrap_or_else(map_origin)` is the substance: a first press that jumped to
    /// (1, 0) would silently discard the scroll position, which is the one piece of
    /// context that makes the cursor's default useful.
    #[test]
    fn the_first_move_starts_from_the_beam() {
        let mut m = a_machine();
        // Scroll 2 x = −80: visible pixel 0 is raster 64 − 80 = −16, which is map
        // column 63 after the wrap. So one step forward is column 0, not column 1.
        cps1_mut(&mut m).board.cps_a[machine::video::regs::SCROLL2_X] = (-80i16) as u16;
        let origin = gfxpanels::map_origin(cps1(&m), Layer::Scroll2);
        assert_eq!(origin.0, 63, "the premise: the beam is at column 63");
        let mut g = looking_at(&m, View::Tilemap);
        assert_eq!(g.state().layer, Layer::Scroll2, "the premise: scroll 2");
        g.update(&act(|a| a.gfx_forward = true), &m);
        assert_eq!(
            g.state().map_at,
            Some((0, origin.1)),
            "one column on from the beam, wrapped"
        );
    }

    /// The tilemap cursor wraps at the map's edge in both directions.
    ///
    /// Reached by stepping *from* column 0, because the beam's own default is not
    /// column 0: with zero scrolls the renderer's first visible pixel is raster
    /// `VISIBLE_X` = 64, which for scroll 2's 16-pixel tiles is column 4. That bias is
    /// exactly what `map_axis` exists to keep in one place, and assuming (0, 0) here
    /// was wrong for the same reason a re-derived viewer would be.
    #[test]
    fn the_tilemap_cursor_wraps_at_the_map_edge() {
        let m = a_machine();
        let mut g = looking_at(&m, View::Tilemap);
        let row = gfxpanels::map_origin(cps1(&m), Layer::Scroll2).1;
        g.cps1.map_at = Some((0, row));
        g.update(&act(|a| a.gfx_back = true), &m);
        assert_eq!(
            g.state().map_at,
            Some((MAP_TILES - 1, row)),
            "back off column zero is the last column, not an underflow"
        );
        g.update(&act(|a| a.gfx_forward = true), &m);
        assert_eq!(g.state().map_at, Some((0, row)), "and forward wraps back");
        // Every reachable column is a real map column, which is what `tile_info`'s
        // `col` needs: 64 would read the next row's entry.
        for _ in 0..MAP_TILES + 2 {
            g.update(&act(|a| a.gfx_forward = true), &m);
            let (c, _) = g.state().map_at.expect("pinned");
            assert!(c < MAP_TILES, "{c} is a map column");
        }
    }

    /// Cycling the tilemap's layer returns the cursor to following.
    ///
    /// A cursor kept across a layer change would point at a cell of a map with a
    /// different tile size, which is a coordinate that means nothing.
    #[test]
    fn changing_the_tilemap_layer_returns_the_cursor_to_the_beam() {
        let m = a_machine();
        let mut g = looking_at(&m, View::Tilemap);
        g.update(&act(|a| a.gfx_forward = true), &m);
        assert!(g.state().map_at.is_some());
        g.update(&act(|a| a.gfx_act = true), &m);
        assert_eq!(g.state().map_at, None, "a new layer, a new default");
    }

    /// The tile view pages by exactly a screenful, and stops at both ends.
    ///
    /// Saturating rather than wrapping: the end of the ROM is a place you stop, and
    /// wrapping to tile 0 there reads as the view resetting itself.
    #[test]
    fn the_tile_view_pages_by_a_screenful_and_stops_at_zero() {
        let m = a_machine();
        let mut g = shown(&m);
        let (cols, rows) = gfxpanels::tile_grid(g.state().kind);
        let page = (cols * rows) as u32;
        assert!(page > 0, "the premise: a page holds tiles");
        g.update(&act(|a| a.gfx_forward = true), &m);
        assert_eq!(g.state().tile_at, page, "one page on");
        g.update(&act(|a| a.gfx_back = true), &m);
        assert_eq!(g.state().tile_at, 0, "and back to where it started");
        g.update(&act(|a| a.gfx_back = true), &m);
        assert_eq!(g.state().tile_at, 0, "and no further: it saturates");
    }

    /// A page is the page the tile view actually draws, layout by layout.
    ///
    /// Read from `gfxpanels::tile_grid` rather than from a constant here, so paging
    /// through a 32×32 ROM does not skip 3/4 of it — which a fixed page size would,
    /// invisibly, because every tile it landed on would still be a real tile.
    #[test]
    fn a_page_is_the_layouts_own_page() {
        let m = a_machine();
        let mut g = shown(&m);
        let mut pages = Vec::new();
        for _ in 0..4 {
            let (cols, rows) = gfxpanels::tile_grid(g.state().kind);
            let mut g2 = g.clone();
            g2.update(&act(|a| a.gfx_forward = true), &m);
            assert_eq!(
                g2.state().tile_at,
                (cols * rows) as u32,
                "{:?} pages by its own grid",
                g.state().kind
            );
            pages.push(cols * rows);
            g.update(&act(|a| a.gfx_act = true), &m);
        }
        // And the four layouts do not all page by the same amount, or the assertion
        // above would hold for a hardcoded constant.
        let distinct: std::collections::BTreeSet<_> = pages.iter().collect();
        assert!(distinct.len() > 1, "the four pages differ: {pages:?}");
    }

    /// Paging saturates at both ends of the `u32` rather than wrapping or panicking.
    ///
    /// Asserted on `paged` rather than through the keys, because reaching the top by
    /// pressing `]` is four billion presses. Both ends matter: a plain `-` underflows
    /// at zero and a plain `+` at the top, and each panics in a debug build while
    /// wrapping to a plausible-looking tile in release.
    #[test]
    fn paging_saturates_at_both_ends() {
        assert_eq!(paged(0, 100, true), 100, "the ordinary case, forward");
        assert_eq!(paged(100, 100, false), 0, "and back");
        assert_eq!(paged(0, 100, false), 0, "back off zero stops");
        assert_eq!(paged(50, 100, false), 0, "and does not wrap under");
        assert_eq!(paged(u32::MAX, 100, true), u32::MAX, "forward at the top");
        assert_eq!(
            paged(u32::MAX - 50, 100, true),
            u32::MAX,
            "and lands on the end rather than wrapping past it"
        );
        // The far end is reachable and then leavable, which a wrapping page would
        // turn into a jump to tile 0.
        assert_eq!(
            paged(paged(u32::MAX, 100, true), 100, false),
            u32::MAX - 100
        );
    }

    /// The tile view really uses `paged`, and does not wrap at the top.
    ///
    /// The state is set directly, which is the one way to reach the top of a `u32`
    /// without four billion keypresses. Without this, the test above could pass while
    /// `step` did its own arithmetic.
    #[test]
    fn the_tile_view_saturates_at_the_top() {
        let m = a_machine();
        let mut g = shown(&m);
        let page = {
            let (cols, rows) = gfxpanels::tile_grid(g.state().kind);
            (cols * rows) as u32
        };
        g.cps1.tile_at = u32::MAX - 1;
        g.update(&act(|a| a.gfx_forward = true), &m);
        assert_eq!(g.state().tile_at, u32::MAX, "saturated, not wrapped");
        g.update(&act(|a| a.gfx_back = true), &m);
        assert_eq!(g.state().tile_at, u32::MAX - page, "and one page back");
    }

    /// The palette cursor pages within the palette and wraps at both ends.
    #[test]
    fn the_palette_cursor_wraps_within_the_palette() {
        let m = a_machine();
        let mut g = looking_at(&m, View::Palette);
        assert_eq!(g.state().pal_at, 0);
        g.update(&act(|a| a.gfx_forward = true), &m);
        assert_eq!(g.state().pal_at, PAL_PAGE, "one row on");
        g.update(&act(|a| a.gfx_back = true), &m);
        assert_eq!(g.state().pal_at, 0, "and back");
        g.update(&act(|a| a.gfx_back = true), &m);
        assert_eq!(g.state().pal_at, PENS - PAL_PAGE, "back off zero wraps");
        // Every reachable cursor is a real palette entry, which is what `draw`'s
        // `pal[at]` needs — a cursor of 0xC00 would index one past the end.
        let mut g = looking_at(&m, View::Palette);
        for _ in 0..2 * (PENS / PAL_PAGE) + 1 {
            g.update(&act(|a| a.gfx_forward = true), &m);
            assert!(g.state().pal_at < PENS, "{} is a pen", g.state().pal_at);
        }
    }

    /// `[` and `]` move the palette cursor by exactly one row of swatches.
    ///
    /// Read off `gfxpanels::pal_cell` — the layout the view actually draws — and not
    /// against `PAL_PAGE`, which is the number under test. `assert_eq!(pal_at,
    /// PAL_PAGE)` above cannot fail for any value of `PAL_PAGE`, so a half-row step
    /// would pass it while landing the cursor in the middle of a row of colours: the
    /// swatch under the highlight would not be the entry the title line names.
    #[test]
    fn the_palette_cursor_moves_by_one_row_of_swatches() {
        let m = a_machine();
        let mut g = looking_at(&m, View::Palette);
        let (x0, y0) = gfxpanels::pal_cell(g.state().pal_at);
        g.update(&act(|a| a.gfx_forward = true), &m);
        let (x1, y1) = gfxpanels::pal_cell(g.state().pal_at);
        assert_eq!(x1, x0, "the cursor stays in its column");
        assert!(y1 > y0, "and moves down a row");
        // And the next press is the same distance again, so the step is *a row*
        // rather than something that merely happens to change the row.
        g.update(&act(|a| a.gfx_forward = true), &m);
        let (x2, y2) = gfxpanels::pal_cell(g.state().pal_at);
        assert_eq!(x2, x0, "still the same column");
        assert_eq!(y2 - y1, y1 - y0, "and the same one row each time");
    }

    /// A hidden viewer ignores its own keys but keeps its mask.
    ///
    /// Two separate claims and both are deliberate. The keys are ignored so `]` does
    /// not silently page a view you cannot see. The mask persists because turning the
    /// *viewer* off is not turning the *layers* back on — you asked to look at the
    /// game with scroll 1 subtracted, and hiding the box is how you do that.
    #[test]
    fn a_hidden_viewer_ignores_keys_but_keeps_its_mask() {
        let m = a_machine();
        let mut g = looking_at(&m, View::Layers);
        g.update(&act(|a| a.gfx_act = true), &m);
        let masked = g.mask();
        assert_ne!(masked, LayerMask::all(), "the premise: a bit is subtracted");

        g.update(&act(|a| a.gfx_toggled = true), &m);
        assert!(!g.shown());
        let view = g.view();
        g.update(&act(|a| a.gfx_view_cycled = true), &m);
        assert_eq!(g.view(), view, "a hidden viewer does not cycle");
        assert_eq!(g.mask(), masked, "but it does not forget its mask");

        // Nor does it act, page, or move its selection.
        let before = format!("{:?}", g.state());
        for a in [
            act(|a| a.gfx_act = true),
            act(|a| a.gfx_forward = true),
            act(|a| a.gfx_back = true),
        ] {
            assert!(!g.update(&a, &m), "still hidden");
        }
        assert_eq!(format!("{:?}", g.state()), before, "and nothing moved");

        // And showing it again finds it where it was left.
        g.update(&act(|a| a.gfx_toggled = true), &m);
        assert_eq!(g.view(), view, "the view survived the round trip");
        assert_eq!(g.mask(), masked, "and so did the mask");
    }

    /// Toggling in the same frame as a view key applies the toggle first.
    ///
    /// `F9` and `F10` held together is a real frame — and the alternative ordering
    /// makes the first `F10` after showing the box do nothing, which reads as a
    /// dropped keypress.
    #[test]
    fn showing_and_cycling_in_one_frame_cycles() {
        let m = a_machine();
        let mut g = GfxViewer::new();
        let first = g.view();
        g.update(
            &act(|a| {
                a.gfx_toggled = true;
                a.gfx_view_cycled = true;
            }),
            &m,
        );
        assert!(g.shown());
        assert_ne!(g.view(), first, "the same frame's F10 was applied");
    }

    /// A hidden viewer draws nothing.
    #[test]
    fn a_hidden_viewer_draws_nothing() {
        let m = a_machine();
        let g = GfxViewer::new();
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        g.draw(&mut buf, &m);
        assert!(buf.iter().all(|&p| p == 0), "nothing shown, nothing drawn");
    }

    /// A shown viewer draws the view it is looking at.
    ///
    /// Compared against `gfxpanels::draw` with this module's own state, so passing the
    /// wrong `ViewState` is visible — a viewer that drew a default state would look
    /// like a plausible browser that ignored every key.
    #[test]
    fn a_shown_viewer_draws_its_own_state() {
        let m = a_machine();
        let mut g = looking_at(&m, View::Palette);
        g.update(&act(|a| a.gfx_forward = true), &m);

        let mut mine = vec![0u32; WIDTH * HEIGHT];
        g.draw(&mut mine, &m);
        let mut expected = vec![0u32; WIDTH * HEIGHT];
        gfxpanels::draw(&mut expected, cps1(&m), &g.state());
        assert_eq!(mine, expected, "the same frame, so the state matches");

        // And a different cursor gives a different frame, or the comparison above
        // would pass for a `draw` that ignored the state entirely.
        let mut other = vec![0u32; WIDTH * HEIGHT];
        gfxpanels::draw(
            &mut other,
            cps1(&m),
            &ViewState {
                pal_at: 0,
                ..g.state()
            },
        );
        assert_ne!(mine, other, "the premise: the cursor changes the frame");
    }

    /// A held viewer key acts once.
    ///
    /// Through the real `Controls`, because `Actions` built by hand cannot show the
    /// difference: a held `Enter` cycling the tile kind sixty times a second lands on
    /// whichever layout the frame count happens to give.
    #[test]
    fn a_held_viewer_key_acts_once() {
        use crate::keys::{Controls, Key, KeySet};
        let m = a_machine();
        let mut c = Controls::new();
        let mut g = GfxViewer::new();
        let held = KeySet::from_keys(&[Key::GfxToggled]);
        assert!(g.update(&c.update(held), &m), "the press shows it");
        for _ in 0..8 {
            assert!(
                g.update(&c.update(held), &m),
                "and holding does not hide it"
            );
        }

        let held = KeySet::from_keys(&[Key::Enter]);
        g.update(&c.update(held), &m);
        let kind = g.state().kind;
        assert_ne!(kind, TileKind::Tile16x16, "the press cycled the layout");
        for _ in 0..8 {
            g.update(&c.update(held), &m);
        }
        assert_eq!(g.state().kind, kind, "and holding does not cycle it again");
    }

    /// Nothing pressed changes nothing.
    #[test]
    fn an_idle_frame_changes_nothing() {
        let m = a_machine();
        let mut g = looking_at(&m, View::Tilemap);
        g.update(&act(|a| a.gfx_forward = true), &m);
        let before = format!("{g:?}");
        for _ in 0..10 {
            assert!(g.update(&Actions::default(), &m), "it stays shown");
        }
        assert_eq!(format!("{g:?}"), before, "and nothing moved");
    }

    /// Using the viewer does not disturb the machine.
    ///
    /// `&Machine` is the compiler's half of this. The behavioural half is worth
    /// stating because `map_origin` reads registers and a reader that went through the
    /// bus would acknowledge an interrupt — the trap `peek_word` documents.
    #[test]
    fn using_the_viewer_does_not_disturb_the_machine() {
        let m = a_machine();
        let before = {
            let c = cps1(&m);
            (c.total_cycles, c.cpu.pc, c.board.trace.acks)
        };
        let mut g = shown(&m);
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        for _ in 0..4 {
            for a in [
                act(|a| a.gfx_act = true),
                act(|a| a.gfx_forward = true),
                act(|a| a.gfx_back = true),
            ] {
                g.update(&a, &m);
            }
            g.draw(&mut buf, &m);
            g.update(&act(|a| a.gfx_view_cycled = true), &m);
        }
        let after = {
            let c = cps1(&m);
            (c.total_cycles, c.cpu.pc, c.board.trace.acks)
        };
        assert_eq!(
            before, after,
            "the viewer reached the machine through something with side effects"
        );
    }

    /// `view` is one field, so cycling on one board moves the other's panel too.
    #[test]
    fn the_view_is_one_field_so_a_board_change_cannot_desynchronise_it() {
        let cps = a_machine();
        let sf1 = an_sf1_machine();
        let mut g = GfxViewer::new();
        g.update(&act(|a| a.gfx_toggled = true), &cps);
        g.update(&act(|a| a.gfx_view_cycled = true), &cps);
        assert_eq!(g.view(), View::Tilemap);
        // The same viewer, now looking at an SF1: the view followed.
        assert_eq!(g.sf1_state().view, View::Tilemap);
        g.update(&act(|a| a.gfx_view_cycled = true), &sf1);
        assert_eq!(g.view(), View::Palette);
        assert_eq!(g.state().view, View::Palette, "and CPS-1's state agrees");
    }

    /// The cursors are not: `]` on one board leaves the other's alone.
    #[test]
    fn each_boards_cursor_is_its_own() {
        let cps = a_machine();
        let sf1 = an_sf1_machine();
        let mut g = GfxViewer::new();
        g.update(&act(|a| a.gfx_toggled = true), &cps);
        g.update(&act(|a| a.gfx_forward = true), &cps);
        let cps_at = g.state().tile_at;
        assert_ne!(cps_at, 0, "CPS-1's tile cursor moved");
        assert_eq!(g.sf1_state().tile_at, 0, "SF1's did not");
        g.update(&act(|a| a.gfx_forward = true), &sf1);
        assert_eq!(g.state().tile_at, cps_at, "and CPS-1's stayed where it was");
        assert_ne!(g.sf1_state().tile_at, 0, "while SF1's moved");
    }

    /// SF1's tilemap cursor wraps at the selected map's own width.
    ///
    /// ⚠️ `g.sf1.map` is set directly rather than pressed to: reaching `MapKind::Tx`
    /// costs one `act` per map, and a test that pressed its way there would be testing
    /// `act` twice and the wrap not at all. Same reason
    /// `the_tilemap_cursor_wraps_at_the_map_edge` writes `g.cps1.map_at`.
    #[test]
    fn the_sf1_tilemap_cursor_wraps_at_each_maps_own_edge() {
        let sf1 = an_sf1_machine();
        let mut g = GfxViewer::new();
        g.update(&act(|a| a.gfx_toggled = true), &sf1);
        g.update(&act(|a| a.gfx_view_cycled = true), &sf1);
        assert_eq!(g.view(), View::Tilemap);
        // The text map has 64 columns; sixty-six presses must not leave it.
        g.sf1.map = MapKind::Tx;
        for _ in 0..66 {
            g.update(&act(|a| a.gfx_forward = true), &sf1);
            let (c, _) = g.sf1_state().map_at.expect("the cursor materialised");
            assert!(c < 64, "column {c} is inside the text map");
        }
        // And the background has 2048, so it does not wrap at 64.
        g.sf1.map = MapKind::Bg;
        g.sf1.map_at = Some((0, 0));
        for _ in 0..70 {
            g.update(&act(|a| a.gfx_forward = true), &sf1);
        }
        let (c, _) = g.sf1_state().map_at.expect("still set");
        assert_eq!(c, 70, "a 64-wide wrap would have put this at 6");
    }

    /// SF1's palette cursor wraps at 1,024 entries, not CPS-1's 3,072.
    #[test]
    fn the_sf1_palette_cursor_wraps_at_1024_and_not_at_3072() {
        let sf1 = an_sf1_machine();
        let mut g = GfxViewer::new();
        g.update(&act(|a| a.gfx_toggled = true), &sf1);
        for _ in 0..2 {
            g.update(&act(|a| a.gfx_view_cycled = true), &sf1);
        }
        assert_eq!(g.view(), View::Palette);
        // Sixteen rows of 64 is the whole palette.
        for _ in 0..16 {
            g.update(&act(|a| a.gfx_forward = true), &sf1);
        }
        assert_eq!(g.sf1_state().pal_at, 0, "sixteen rows is one lap");
        g.update(&act(|a| a.gfx_back = true), &sf1);
        assert_eq!(g.sf1_state().pal_at, 1024 - 64);
    }

    /// `Enter` on SF1's tile view cycles the plane, because a plane's layout is fixed.
    #[test]
    fn acting_on_the_sf1_tile_view_cycles_the_plane_not_a_tile_kind() {
        let sf1 = an_sf1_machine();
        let mut g = GfxViewer::new();
        g.update(&act(|a| a.gfx_toggled = true), &sf1);
        assert_eq!(g.sf1_state().plane, Plane::Bg);
        for want in [Plane::Fg, Plane::Sprites, Plane::Tx, Plane::Bg] {
            g.update(&act(|a| a.gfx_act = true), &sf1);
            assert_eq!(g.sf1_state().plane, want);
        }
    }

    /// SF1's layers view toggles SF1's four fields, and row 0 is the background.
    #[test]
    fn the_sf1_layers_view_toggles_sf1s_own_four_fields() {
        let sf1 = an_sf1_machine();
        let mut g = GfxViewer::new();
        g.update(&act(|a| a.gfx_toggled = true), &sf1);
        for _ in 0..3 {
            g.update(&act(|a| a.gfx_view_cycled = true), &sf1);
        }
        assert_eq!(g.view(), View::Layers);
        assert_eq!(g.sf1_mask(), Sf1LayerMask::all());
        // Row 0 is the background — SF1's drawing order, not CPS-1's sprites-first.
        g.update(&act(|a| a.gfx_act = true), &sf1);
        assert!(!g.sf1_mask().bg, "row 0 is the background");
        assert!(g.sf1_mask().fg && g.sf1_mask().sprites && g.sf1_mask().tx);
        // And CPS-1's mask is untouched by any of it.
        assert_eq!(g.mask(), LayerMask::all());
    }

    /// A shown viewer draws whichever board it is handed, through that board's panels.
    ///
    /// ⚠️ The view is cycled off `View::Tiles` and the cursor moved before anything is
    /// drawn. Without that, `draw` could pass `sf1panels` a default `Sf1ViewState` —
    /// hardcoded view, cursor zero — and every assertion below would still hold,
    /// because a fresh viewer's state *is* the default. A mutant that did exactly that
    /// survived until this test moved off it.
    #[test]
    fn a_shown_viewer_draws_the_board_it_is_given() {
        let cps = a_machine();
        let sf1 = an_sf1_machine();
        let mut g = GfxViewer::new();
        g.update(&act(|a| a.gfx_toggled = true), &sf1);
        for _ in 0..2 {
            g.update(&act(|a| a.gfx_view_cycled = true), &sf1);
        }
        g.update(&act(|a| a.gfx_forward = true), &sf1);
        assert_eq!(g.view(), View::Palette, "the premise: not the default view");
        assert_ne!(g.sf1_state().pal_at, 0, "nor the default cursor");
        let mut on_sf1 = vec![0u32; WIDTH * HEIGHT];
        g.draw(&mut on_sf1, &sf1);
        let mut on_cps = vec![0u32; WIDTH * HEIGHT];
        g.draw(&mut on_cps, &cps);
        assert_ne!(on_sf1, on_cps, "two boards, two pictures");
        // And each matches its own panel module, called directly.
        let mut expected = vec![0u32; WIDTH * HEIGHT];
        match &sf1 {
            Machine::Sf1(s) => crate::sf1panels::draw(&mut expected, s, &g.sf1_state()),
            Machine::Cps1(_) => unreachable!("built as Sf1"),
        }
        assert_eq!(on_sf1, expected);
    }

    /// A hidden viewer draws nothing, on either board.
    #[test]
    fn a_hidden_viewer_draws_nothing_on_either_board() {
        let cps = a_machine();
        let sf1 = an_sf1_machine();
        let g = GfxViewer::new();
        for m in [&cps, &sf1] {
            let mut buf = vec![0u32; WIDTH * HEIGHT];
            g.draw(&mut buf, m);
            assert!(buf.iter().all(|&w| w == 0));
        }
    }

    /// And it does not disturb an SF1 either.
    #[test]
    fn using_the_viewer_does_not_disturb_an_sf1() {
        let mut sf1 = an_sf1_machine();
        sf1.run_frame();
        let before = match &sf1 {
            Machine::Sf1(s) => (s.total_cycles, s.cpu.pc, s.board.active),
            Machine::Cps1(_) => unreachable!("built as Sf1"),
        };
        let mut g = GfxViewer::new();
        g.update(&act(|a| a.gfx_toggled = true), &sf1);
        for _ in 0..4 {
            g.update(&act(|a| a.gfx_forward = true), &sf1);
            g.update(&act(|a| a.gfx_act = true), &sf1);
            let mut buf = vec![0u32; WIDTH * HEIGHT];
            g.draw(&mut buf, &sf1);
            g.update(&act(|a| a.gfx_view_cycled = true), &sf1);
        }
        let after = match &sf1 {
            Machine::Sf1(s) => (s.total_cycles, s.cpu.pc, s.board.active),
            Machine::Cps1(_) => unreachable!("built as Sf1"),
        };
        assert_eq!(before, after);
    }
}
