//! The key menu: pick one of four button arrangements, or go back to the default.
//!
//! # Why presets and not rebinding
//!
//! [`Key`](crate::Key) is **layout-blind by construction** — a variant names a physical
//! position, not a letter (see `sfemu::display::translate` for the measurements). So
//! "press the key you want" cannot name a key the map does not already reach, and a
//! rebinding UI would have to first turn `Key` into a full physical-position type. Presets
//! sidestep that by shipping complete, verified maps: every one of the four is asserted
//! against the board's own port bits in [`crate::keys`].
//!
//! # The stick is not a preset, and that is the surprise
//!
//! AZERTY's `Z S Q D` and QWERTY's `W A S D` are **the same four physical keys**. Because
//! `minifb` names positions, one map reads correctly on both layouts and only the printed
//! letters differ. So the obvious 2×2 matrix of {layout} × {row order} collapses: a preset
//! varies which row punches, and which three letters the button rows print. That is why
//! [`stick_label`] returns a different *string* per preset while
//! [`crate::Controls::update`] reads the same four `Key`s under all four.
//!
//! # This module owns the letters; `keys` owns the map
//!
//! The division matters and it is the one rule here worth stating twice. `crate::keys`
//! never spells a letter — it maps `Key` variants to port bits, and a preset there is a
//! set of variants. Every printed label lives in *this* file, because a label is a claim
//! about somebody's keyboard and belongs on the display side of the line, next to the
//! panel that draws it.
//!
//! # The one-frame lag, deliberately
//!
//! The menu learns that `Tab` was pressed from the [`Actions`] that
//! [`crate::Controls::update`] produced, so it cannot have been open on the frame that
//! opened it: the board is live for that one frame, and captured from the next. Closing is
//! the same in reverse. The alternative — having the menu read the raw [`KeySet`] itself —
//! would be a second copy of the edge detection, and a one-frame overlap between a
//! keypress and a menu appearing is not something a player can perceive.
//!
//! [`KeySet`]: crate::KeySet

use crate::font::{draw_text, ADVANCE, LINE};
use crate::keys::{Actions, Preset};
use crate::overlay::{box_at, FG, HI, PAD};
use machine::video::{HEIGHT, WIDTH};

/// One row of the menu.
///
/// `RestoreDefaults` is a variant rather than a fifth preset because it is not an
/// arrangement: it is "whatever the default happens to be", and a player who has never
/// opened this menu should be able to get back to what they started with without knowing
/// its name. Written as its own row, a change to [`Preset::default`] moves this row with
/// it — where a fifth entry naming `AzertyPunchLow` would silently stop being the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuRow {
    /// Switch to this arrangement.
    Use(Preset),
    /// Go back to [`Preset::default`].
    RestoreDefaults,
}

impl MenuRow {
    /// Every row, in the order the menu lists them: the four presets, then the restore.
    ///
    /// Written out rather than built from `Preset::ALL`, so the row count is a literal a
    /// test can pin and adding a preset without widening this array fails to compile.
    pub const ALL: [MenuRow; 5] = [
        MenuRow::Use(Preset::AzertyPunchLow),
        MenuRow::Use(Preset::AzertyCabinet),
        MenuRow::Use(Preset::QwertyPunchLow),
        MenuRow::Use(Preset::QwertyCabinet),
        MenuRow::RestoreDefaults,
    ];

    /// Which arrangement this row selects.
    ///
    /// Every row selects one, including the restore — which is what makes the panel able
    /// to preview any highlighted row's keys with no special case.
    pub fn preset(self) -> Preset {
        match self {
            MenuRow::Use(p) => p,
            MenuRow::RestoreDefaults => Preset::default(),
        }
    }

    /// The text this row shows, without the cursor or the `(current)` marker.
    pub fn label(self) -> &'static str {
        match self {
            MenuRow::Use(p) => p.name(),
            MenuRow::RestoreDefaults => "restore defaults",
        }
    }
}

/// The menu's state: open or shut, and which row the cursor is on.
///
/// Two fields and no window, no clock and no filesystem — the same as every other panel
/// here, so the whole state machine is asserted in this file's tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyMenu {
    open: bool,
    /// The cursor, an index into [`MenuRow::ALL`].
    ///
    /// Always in range: [`KeyMenu::update`] is the only thing that moves it and it
    /// saturates at both ends. `usize` and not a `MenuRow`, because the cursor is a
    /// position in a list and the list is what gives it meaning.
    sel: usize,
}

impl KeyMenu {
    /// A shut menu.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the menu is up — which is what the play loop passes to
    /// [`crate::Controls::set_menu_open`].
    pub fn open(&self) -> bool {
        self.open
    }

    /// Which row the cursor is on.
    pub fn selected(&self) -> MenuRow {
        MenuRow::ALL[self.sel]
    }

    /// Reads this frame's [`Actions`], returning a preset to apply if one was chosen.
    ///
    /// `current` is the arrangement in force, used for one thing: opening the menu puts
    /// the cursor on it. A menu that always opened on the first row would make the second
    /// keypress of "switch back" a guess.
    ///
    /// Applying does **not** close the menu — the mockup lists apply and close as separate
    /// keys, and leaving it up is what lets the player see `(current)` move to the row they
    /// just chose. Confirmation matters here because the four names differ by one word.
    pub fn update(&mut self, a: &Actions, current: Preset) -> Option<Preset> {
        if a.menu_toggled {
            self.open = !self.open;
            if self.open {
                // Opening puts the cursor on the row that is in force. `position` and not
                // a match: `RestoreDefaults` also *has* a preset, and finding the first
                // row whose preset is `current` would land on the restore row whenever
                // the default is active — pointing the cursor at a row that does nothing.
                self.sel = MenuRow::ALL
                    .iter()
                    .position(|r| *r == MenuRow::Use(current))
                    .unwrap_or(0);
            }
            return None;
        }
        if !self.open {
            // Every action below is already gated in `Controls::update`, which is where
            // the capture lives. This is the second gate and it is not redundant: this
            // module's tests build `Actions` directly, so without it a passing test here
            // could rest on a guarantee made in another file.
            return None;
        }
        if a.menu_close {
            self.open = false;
            return None;
        }
        if a.menu_up {
            // Saturating, not wrapping. A five-row list is short enough that wrapping
            // saves nothing, and a cursor that jumps from the top to the bottom is how a
            // player lands on a row they did not mean to read.
            self.sel = self.sel.saturating_sub(1);
        }
        if a.menu_down {
            self.sel = (self.sel + 1).min(MenuRow::ALL.len() - 1);
        }
        if a.menu_apply {
            return Some(self.selected().preset());
        }
        None
    }
}

/// P1's stick, as this preset's keyboard prints it.
///
/// The same four `Key` variants under every preset — see this module's documentation.
/// Only the letters change, and that is the whole content of the "layout" half of a
/// preset's name.
pub fn stick_label(p: Preset) -> &'static str {
    match p {
        Preset::AzertyPunchLow | Preset::AzertyCabinet => "Z S Q D",
        Preset::QwertyPunchLow | Preset::QwertyCabinet => "W A S D",
    }
}

/// P1's punch row, as this preset's keyboard prints it.
///
/// `K L M` on AZERTY and `J K L` on QWERTY: both are the home row's run of three, and
/// they are *different physical keys*, because AZERTY moves `M` onto the home row and
/// pushes `;` off it. This is the one place the two layouts genuinely need different
/// keys rather than different letters for the same ones.
pub fn punch_label(p: Preset) -> &'static str {
    match p {
        Preset::AzertyPunchLow => "K L M",
        Preset::AzertyCabinet => "I O P",
        Preset::QwertyPunchLow => "J K L",
        Preset::QwertyCabinet => "I O P",
    }
}

/// P1's kick row, as this preset's keyboard prints it.
pub fn kick_label(p: Preset) -> &'static str {
    match p {
        Preset::AzertyPunchLow => "I O P",
        Preset::AzertyCabinet => "K L M",
        Preset::QwertyPunchLow => "I O P",
        Preset::QwertyCabinet => "J K L",
    }
}

/// P2's punch row on the keypad.
pub fn p2_punch_label(p: Preset) -> &'static str {
    match p {
        Preset::AzertyPunchLow | Preset::QwertyPunchLow => "4 5 6",
        Preset::AzertyCabinet | Preset::QwertyCabinet => "7 8 9",
    }
}

/// P2's kick row on the keypad.
pub fn p2_kick_label(p: Preset) -> &'static str {
    match p {
        Preset::AzertyPunchLow | Preset::QwertyPunchLow => "7 8 9",
        Preset::AzertyCabinet | Preset::QwertyCabinet => "4 5 6",
    }
}

/// The box's width in characters.
///
/// Measured, not guessed, and pinned by `the_box_is_wide_enough_for_every_row`: the widest
/// row is a selected preset that is also current — `"> "` (2) plus the longest
/// [`Preset::name`] (20, `AZERTY  punches high`) plus `"  (current)"` (11) — which is 33.
/// One column of slack.
///
/// A fixed width and not a maximum over the rows at draw time, for `sndpanel.rs`'s
/// reason: a box whose width depends on its content has to be measured at every value it
/// can hold, and `draw_text` clips rather than panicking, so an overflow would be silently
/// truncated ink rather than a crash.
pub const COLS: usize = 34;

/// The box's height in rows: a title, five choices, two summary rows, and two help rows.
pub const ROWS: usize = 10;

/// Where the box starts, horizontally: centred.
///
/// Centred because this is the one panel that is *modal* — the game is frozen behind it in
/// the sense that matters, since the board reads idle while it is up. The debugger's
/// panels tile the edges because you read them beside a running game; this one is the only
/// thing being read.
pub const MENU_X: usize = (WIDTH - (COLS * ADVANCE + 2 * PAD)) / 2;
/// Ditto, vertically.
pub const MENU_Y: usize = (HEIGHT - (ROWS * LINE + 2 * PAD)) / 2;

/// Draws the menu, if it is open.
///
/// `current` is the arrangement in force — the row it names gets `(current)`, and it is
/// *not* necessarily the highlighted row: the two summary rows preview whichever row the
/// cursor is on, so a player can read a preset's keys before committing to it.
///
/// The box is drawn with the same `box_at` chrome as every other panel rather than the
/// mockup's `┌─┐` rules: [`crate::font`] covers `' '` through `'~'` and has no box-drawing
/// glyphs, so a rule would render as a row of `?`.
///
/// (`box_at` is a plain code span and not a link, because it is `pub(crate)` in
/// `overlay.rs` and `#![deny(rustdoc::private_intra_doc_links)]` makes a link to one from a
/// `pub` item a doc-build failure — `sndpanel.rs` pays the same price.)
///
/// # Panics
///
/// If `buf` is not a `WIDTH × HEIGHT` frame, as [`crate::font::draw_text`].
pub fn draw(buf: &mut [u32], menu: &KeyMenu, current: Preset) {
    if !menu.open {
        return;
    }
    let (x, y) = box_at(buf, MENU_X, MENU_Y, COLS, ROWS);
    let mut row = 0usize;
    let mut line = |buf: &mut [u32], s: &str, fg: u32| {
        draw_text(buf, x, y + row * LINE, s, fg);
        row += 1;
    };

    line(buf, "KEYS", HI);
    for (i, r) in MenuRow::ALL.iter().enumerate() {
        let cursor = if i == menu.sel { "> " } else { "  " };
        let mark = if *r == MenuRow::Use(current) {
            "  (current)"
        } else {
            ""
        };
        // Highlighted where the cursor is, so the row the summary describes is the row
        // the eye is on — the cursor alone is two pixels wide at this font size.
        let fg = if i == menu.sel { HI } else { FG };
        line(buf, &format!("{cursor}{}{mark}", r.label()), fg);
    }
    // The *selected* row's keys, not the current one's. Previewing is the point: the four
    // names differ by one word, and "punches low" does not tell you which three letters.
    let p = menu.selected().preset();
    line(
        buf,
        &format!(
            "P1  {}   {} / {}",
            stick_label(p),
            punch_label(p),
            kick_label(p)
        ),
        FG,
    );
    line(
        buf,
        &format!("P2  arrows    {} / {}", p2_punch_label(p), p2_kick_label(p)),
        FG,
    );
    line(buf, "up/down move   Enter apply", FG);
    // `Esc` and not `Escape`: the same key, and the row has to fit. It closes rather than
    // quitting only while this menu is up — see `Actions::menu_close`.
    line(buf, "Tab close      Esc cancel", FG);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `Actions` with one menu field set, and nothing else.
    ///
    /// The tests build `Actions` directly rather than driving `Controls`, because what is
    /// under test here is the state machine and not the key map. `Controls`' own tests own
    /// the other direction — which key produces which of these fields — and
    /// `the_menu_and_the_key_map_agree_on_the_capture` is the seam between them.
    fn act(f: impl FnOnce(&mut Actions)) -> Actions {
        let mut a = Actions::default();
        f(&mut a);
        a
    }

    /// A fresh menu is shut, and reads nothing.
    #[test]
    fn a_fresh_menu_is_shut() {
        let mut m = KeyMenu::new();
        assert!(!m.open());
        // Every navigation action, with the menu shut, does nothing at all — including
        // apply, which must not change a preset from a menu nobody opened.
        for a in [
            act(|a| a.menu_up = true),
            act(|a| a.menu_down = true),
            act(|a| a.menu_apply = true),
            act(|a| a.menu_close = true),
        ] {
            assert_eq!(m.update(&a, Preset::AzertyPunchLow), None);
            assert!(!m.open(), "and it stays shut");
        }
        assert_eq!(
            m.selected(),
            MenuRow::Use(Preset::AzertyPunchLow),
            "the cursor did not move either"
        );
    }

    /// `Tab` opens it and `Tab` closes it.
    #[test]
    fn tab_toggles_the_menu() {
        let mut m = KeyMenu::new();
        let tab = act(|a| a.menu_toggled = true);
        assert_eq!(m.update(&tab, Preset::AzertyPunchLow), None);
        assert!(m.open(), "Tab opens it");
        assert_eq!(m.update(&tab, Preset::AzertyPunchLow), None);
        assert!(!m.open(), "and Tab closes it again");
    }

    /// Opening puts the cursor on the arrangement in force.
    ///
    /// One case per preset, because the natural implementation is a `position` over the
    /// rows and the natural mistake is an off-by-one that only shows on one of them.
    #[test]
    fn opening_puts_the_cursor_on_the_current_preset() {
        for p in Preset::ALL {
            let mut m = KeyMenu::new();
            m.update(&act(|a| a.menu_toggled = true), p);
            assert_eq!(
                m.selected(),
                MenuRow::Use(p),
                "opening with {p:?} in force must select its row"
            );
        }
    }

    /// The cursor never lands on `restore defaults` merely because the default is active.
    ///
    /// The trap this closes: `RestoreDefaults::preset()` *is* the default, so a search for
    /// "the first row whose preset is `current`" is correct for three presets and wrong for
    /// the fourth — it would open on the restore row whenever the default is in force,
    /// which is the most common case there is.
    #[test]
    fn opening_on_the_default_selects_its_own_row_not_the_restore() {
        let mut m = KeyMenu::new();
        m.update(&act(|a| a.menu_toggled = true), Preset::default());
        assert_eq!(m.selected(), MenuRow::Use(Preset::default()));
        assert_ne!(m.selected(), MenuRow::RestoreDefaults);
        assert_eq!(
            MenuRow::RestoreDefaults.preset(),
            Preset::default(),
            "and the two do share a preset, which is what makes the above a real trap"
        );
    }

    /// Up and down walk the rows and stop at the ends.
    #[test]
    fn the_cursor_walks_the_rows_and_saturates() {
        let mut m = KeyMenu::new();
        m.update(&act(|a| a.menu_toggled = true), Preset::AzertyPunchLow);
        let down = act(|a| a.menu_down = true);
        let up = act(|a| a.menu_up = true);
        // Down through all five.
        for want in &MenuRow::ALL[1..] {
            m.update(&down, Preset::AzertyPunchLow);
            assert_eq!(m.selected(), *want);
        }
        // And then it stops, rather than wrapping to the top.
        for _ in 0..3 {
            m.update(&down, Preset::AzertyPunchLow);
            assert_eq!(
                m.selected(),
                MenuRow::RestoreDefaults,
                "down must saturate at the last row, not wrap"
            );
        }
        // Back up through all five.
        for want in MenuRow::ALL.iter().rev().skip(1) {
            m.update(&up, Preset::AzertyPunchLow);
            assert_eq!(m.selected(), *want);
        }
        for _ in 0..3 {
            m.update(&up, Preset::AzertyPunchLow);
            assert_eq!(
                m.selected(),
                MenuRow::ALL[0],
                "up must saturate at the first row, not wrap"
            );
        }
    }

    /// Apply returns the selected row's preset — every row, and only on apply.
    #[test]
    fn apply_returns_the_selected_rows_preset() {
        let apply = act(|a| a.menu_apply = true);
        for (i, r) in MenuRow::ALL.iter().enumerate() {
            let mut m = KeyMenu::new();
            m.update(&act(|a| a.menu_toggled = true), Preset::AzertyPunchLow);
            for _ in 0..i {
                m.update(&act(|a| a.menu_down = true), Preset::AzertyPunchLow);
            }
            assert_eq!(m.selected(), *r, "row {i}");
            assert_eq!(
                m.update(&apply, Preset::AzertyPunchLow),
                Some(r.preset()),
                "row {i} applies its own preset"
            );
            assert!(m.open(), "applying leaves the menu up for confirmation");
            // And a frame with nothing pressed applies nothing, which is what makes the
            // return value an event rather than a reading.
            assert_eq!(m.update(&Actions::default(), Preset::AzertyPunchLow), None);
        }
    }

    /// The restore row applies the default, whatever the default is.
    #[test]
    fn the_restore_row_applies_the_default() {
        assert_eq!(
            MenuRow::RestoreDefaults.preset(),
            Preset::AzertyPunchLow,
            "the default is AZERTY punches low"
        );
        let mut m = KeyMenu::new();
        // Open with a non-default in force, so the cursor starts elsewhere.
        m.update(&act(|a| a.menu_toggled = true), Preset::QwertyCabinet);
        for _ in 0..MenuRow::ALL.len() {
            m.update(&act(|a| a.menu_down = true), Preset::QwertyCabinet);
        }
        assert_eq!(m.selected(), MenuRow::RestoreDefaults);
        assert_eq!(
            m.update(&act(|a| a.menu_apply = true), Preset::QwertyCabinet),
            Some(Preset::default())
        );
    }

    /// Close shuts it and applies nothing.
    ///
    /// The cancel path. A close that also applied the highlighted row would make arrowing
    /// through the list to read it a way to change your keys by accident.
    #[test]
    fn close_shuts_it_without_applying() {
        let mut m = KeyMenu::new();
        m.update(&act(|a| a.menu_toggled = true), Preset::AzertyPunchLow);
        m.update(&act(|a| a.menu_down = true), Preset::AzertyPunchLow);
        assert_eq!(m.selected(), MenuRow::Use(Preset::AzertyCabinet));
        assert_eq!(
            m.update(&act(|a| a.menu_close = true), Preset::AzertyPunchLow),
            None,
            "closing applies nothing"
        );
        assert!(!m.open());
    }

    /// Close and apply on the same frame: the apply is dropped.
    ///
    /// `Enter` and `Escape` can be pressed on one frame, and the order the two `if`s run
    /// in decides what happens. Cancel winning is the safe half of the choice, and this
    /// pins it so a reordering is a failing test rather than a surprise.
    #[test]
    fn close_beats_apply_on_the_same_frame() {
        let mut m = KeyMenu::new();
        m.update(&act(|a| a.menu_toggled = true), Preset::AzertyPunchLow);
        m.update(&act(|a| a.menu_down = true), Preset::AzertyPunchLow);
        let both = act(|a| {
            a.menu_apply = true;
            a.menu_close = true;
        });
        assert_eq!(m.update(&both, Preset::AzertyPunchLow), None);
        assert!(!m.open(), "and it closed");
    }

    /// A toggle on the same frame as anything else is only a toggle.
    ///
    /// `Tab` is the one action `Controls::update` does not gate on the menu being open, so
    /// it is the one that can arrive alongside a stale navigation flag. Handling it first
    /// and returning means opening the menu can never also apply a preset on its first
    /// frame — which, with the cursor freshly placed, would be a silent no-op reset.
    #[test]
    fn a_toggle_wins_over_everything_else_on_its_frame() {
        let mut m = KeyMenu::new();
        let both = act(|a| {
            a.menu_toggled = true;
            a.menu_apply = true;
            a.menu_down = true;
        });
        assert_eq!(m.update(&both, Preset::QwertyCabinet), None, "no apply");
        assert!(m.open());
        assert_eq!(
            m.selected(),
            MenuRow::Use(Preset::QwertyCabinet),
            "and the cursor is where opening put it, not one row down"
        );
    }

    /// Every row has a distinct label, and every label fits the box.
    ///
    /// The width is a constant, and `draw_text` clips rather than panicking — so a row
    /// that overflowed would be a silently truncated line, which is the failure
    /// `sndpanel.rs` records paying for once already.
    #[test]
    fn the_box_is_wide_enough_for_every_row() {
        // The widest row is a selected row that is also current.
        for r in MenuRow::ALL {
            let widest = format!("> {}  (current)", r.label());
            assert!(
                widest.len() <= COLS,
                "{:?} needs {} columns of {COLS}: {widest:?}",
                r,
                widest.len()
            );
        }
        // The fixed rows, including the two summaries at their longest.
        let mut fixed = vec![
            "KEYS".to_string(),
            "up/down move   Enter apply".to_string(),
            "Tab close      Esc cancel".to_string(),
        ];
        for p in Preset::ALL {
            fixed.push(format!(
                "P1  {}   {} / {}",
                stick_label(p),
                punch_label(p),
                kick_label(p)
            ));
            fixed.push(format!(
                "P2  arrows    {} / {}",
                p2_punch_label(p),
                p2_kick_label(p)
            ));
        }
        for s in fixed {
            assert!(s.len() <= COLS, "{} columns of {COLS}: {s:?}", s.len());
        }
        // And nothing is wasted: some row uses the full width, or `COLS` is a guess.
        let widest = MenuRow::ALL
            .iter()
            .map(|r| format!("> {}  (current)", r.label()).len())
            .max()
            .expect("five rows");
        assert_eq!(widest, 33, "the widest row is 33 columns");
        assert_eq!(COLS, 34, "one column of slack, and no more");
    }

    /// Every label is printable by [`crate::font`], which covers `' '` to `'~'` only.
    ///
    /// The approved mockup drew the box with `┌─ KEYS ─┐`. Those are not in the font, and
    /// [`crate::font::glyph`] substitutes `'?'` for anything outside its range — so a rule
    /// would have rendered as a row of question marks with every test green.
    #[test]
    fn every_label_is_ascii_the_font_can_draw() {
        let mut all: Vec<String> = MenuRow::ALL.iter().map(|r| r.label().to_string()).collect();
        all.push("KEYS".into());
        all.push("(current)".into());
        all.push("up/down move   Enter apply".into());
        all.push("Tab close      Esc cancel".into());
        for p in Preset::ALL {
            for s in [
                stick_label(p),
                punch_label(p),
                kick_label(p),
                p2_punch_label(p),
                p2_kick_label(p),
            ] {
                all.push(s.into());
            }
        }
        for s in all {
            for c in s.chars() {
                assert!(
                    (' '..='~').contains(&c),
                    "{c:?} in {s:?} is outside the font and draws as '?'"
                );
            }
        }
    }

    /// The four presets' key summaries are all different, and name the right rows.
    ///
    /// This is where the "the stick needs no preset" discovery is pinned: `stick_label`
    /// changes with the layout while [`crate::Controls`] reads the same four `Key`s, and
    /// the punch row is the *only* place the two layouts need different physical keys.
    #[test]
    fn the_labels_describe_the_presets_they_name() {
        // AZERTY and QWERTY print the same four physical keys differently.
        assert_eq!(stick_label(Preset::AzertyPunchLow), "Z S Q D");
        assert_eq!(stick_label(Preset::QwertyPunchLow), "W A S D");
        assert_eq!(stick_label(Preset::AzertyCabinet), "Z S Q D");
        assert_eq!(stick_label(Preset::QwertyCabinet), "W A S D");
        // Punches low: the home row punches. AZERTY's run of three is KLM, QWERTY's JKL.
        assert_eq!(punch_label(Preset::AzertyPunchLow), "K L M");
        assert_eq!(kick_label(Preset::AzertyPunchLow), "I O P");
        assert_eq!(punch_label(Preset::QwertyPunchLow), "J K L");
        assert_eq!(kick_label(Preset::QwertyPunchLow), "I O P");
        // Punches high: a cabinet's order, the same two rows traded.
        assert_eq!(punch_label(Preset::AzertyCabinet), "I O P");
        assert_eq!(kick_label(Preset::AzertyCabinet), "K L M");
        assert_eq!(punch_label(Preset::QwertyCabinet), "I O P");
        assert_eq!(kick_label(Preset::QwertyCabinet), "J K L");
        // The keypad, which has no layout question — only a row order.
        assert_eq!(p2_punch_label(Preset::AzertyPunchLow), "4 5 6");
        assert_eq!(p2_kick_label(Preset::AzertyPunchLow), "7 8 9");
        assert_eq!(p2_punch_label(Preset::AzertyCabinet), "7 8 9");
        assert_eq!(p2_kick_label(Preset::AzertyCabinet), "4 5 6");
        // No preset prints the same rows for its punches and its kicks, which a
        // copy-paste between the two functions would produce.
        for p in Preset::ALL {
            assert_ne!(punch_label(p), kick_label(p), "{p:?}: one row, two names");
            assert_ne!(p2_punch_label(p), p2_kick_label(p), "{p:?}: the keypad");
        }
    }

    /// The labels agree with the map: each named letter is the key the preset reads.
    ///
    /// The seam between this module and [`crate::keys`], and the reason both halves are
    /// worth having. The labels are strings and the map is `Key` variants, so nothing but
    /// this test stops a preset from *saying* `K L M` while pressing `I O P` — which is
    /// exactly the bug a player would report as "the menu is lying".
    ///
    /// `Key`'s variants are named for their **AZERTY** label, so only the AZERTY presets
    /// can be checked letter-by-letter; the QWERTY ones are checked by the shape of what
    /// they must be, which is the honest claim rather than a tidier false one.
    #[test]
    fn the_labels_agree_with_the_map() {
        use crate::keys::Key;
        let letters = |ks: [Key; 3]| ks.map(|k| format!("{k:?}")).join(" ").replace("NumPad", "");
        for p in [Preset::AzertyPunchLow, Preset::AzertyCabinet] {
            assert_eq!(letters(p.p1_punch()), punch_label(p), "{p:?}: punches");
            assert_eq!(letters(p.p1_kick()), kick_label(p), "{p:?}: kicks");
        }
        // The keypad's variants are `NumPad4`..`NumPad9`, so the digits do match after
        // the prefix comes off — under every preset, both layouts.
        for p in Preset::ALL {
            assert_eq!(
                letters(p.p2_punch()),
                p2_punch_label(p),
                "{p:?}: P2 punches"
            );
            assert_eq!(letters(p.p2_kick()), p2_kick_label(p), "{p:?}: P2 kicks");
        }
        // QWERTY's `J K L` is `Key::J`, `Key::K`, `Key::L` — the variants happen to be
        // named for the same three letters here, since J, K and L are in the same place
        // on both layouts. `M` is the one that is not, and it must be absent.
        assert_eq!(
            letters(Preset::QwertyPunchLow.p1_punch()),
            "J K L",
            "QWERTY punches low reads J, K and L"
        );
        assert!(
            !Preset::QwertyPunchLow.p1_punch().contains(&Key::M)
                && !Preset::QwertyPunchLow.p1_kick().contains(&Key::M),
            "no QWERTY preset may read Key::M, whose position prints ';' there"
        );
        assert!(
            !Preset::AzertyPunchLow.p1_punch().contains(&Key::J)
                && !Preset::AzertyPunchLow.p1_kick().contains(&Key::J),
            "no AZERTY preset may read Key::J, which is not in its home-row run"
        );
    }

    /// The menu and the key map agree on the capture.
    ///
    /// The other half of the seam: this drives a real [`crate::Controls`] with real key
    /// presses, so what is asserted is the whole path — `Tab` reaches `menu_toggled`,
    /// which opens this menu, whose `open()` the loop feeds back as `set_menu_open`, after
    /// which the board reads idle and `Escape` no longer quits. Every one of those steps
    /// is tested in isolation elsewhere; none of them proves they are wired together.
    #[test]
    fn the_menu_and_the_key_map_agree_on_the_capture() {
        use crate::keys::{Controls, Key, KeySet};
        let mut c = Controls::new();
        let mut m = KeyMenu::new();
        // One frame of the loop: hand the map the menu's state, read the keys, hand the
        // menu the actions, apply anything it returns.
        let frame = |c: &mut Controls, m: &mut KeyMenu, held: KeySet| {
            c.set_menu_open(m.open());
            let a = c.update(held);
            if let Some(p) = m.update(&a, c.preset()) {
                c.set_preset(p);
            }
            a
        };

        // A punch reaches the board with the menu shut.
        let punch = KeySet::from_keys(&[Key::K]);
        assert_eq!(
            frame(&mut c, &mut m, punch).inputs.in1(),
            0xFFEF,
            "K is P1's jab"
        );

        // Tab opens it. The board is live for this one frame — the documented lag.
        frame(&mut c, &mut m, KeySet::from_keys(&[Key::Tab]));
        assert!(m.open(), "Tab opened the menu");

        // From the next frame the board is idle, even with the punch still held.
        assert_eq!(
            frame(&mut c, &mut m, punch).inputs.in1(),
            0xFFFF,
            "the board is idle while the menu is up"
        );

        // Escape closes it and does not quit.
        let a = frame(&mut c, &mut m, KeySet::from_keys(&[Key::Escape]));
        assert!(!a.quit, "Escape must not quit while the menu is up");
        assert!(!m.open(), "it closed instead");

        // And the board is live again.
        assert_eq!(
            frame(&mut c, &mut m, punch).inputs.in1(),
            0xFFEF,
            "the board is live again"
        );

        // Now the whole point: open it, walk to `QWERTY punches low`, apply, close, and
        // the map has actually changed — `J` is the jab and `M` presses nothing.
        frame(&mut c, &mut m, KeySet::from_keys(&[Key::Tab]));
        let down = KeySet::from_keys(&[Key::Down]);
        for _ in 0..2 {
            frame(&mut c, &mut m, down);
            frame(&mut c, &mut m, KeySet::new());
        }
        assert_eq!(m.selected(), MenuRow::Use(Preset::QwertyPunchLow));
        frame(&mut c, &mut m, KeySet::from_keys(&[Key::Enter]));
        assert_eq!(c.preset(), Preset::QwertyPunchLow, "the preset was applied");
        frame(&mut c, &mut m, KeySet::from_keys(&[Key::Tab]));
        assert!(!m.open());

        assert_eq!(
            frame(&mut c, &mut m, KeySet::from_keys(&[Key::J]))
                .inputs
                .in1(),
            0xFFEF,
            "J is P1's jab under QWERTY punches low"
        );
        assert_eq!(
            frame(&mut c, &mut m, KeySet::from_keys(&[Key::M]))
                .inputs
                .in1(),
            0xFFFF,
            "and M presses nothing"
        );
    }

    /// The box fits on the screen, centred, with the game visible around it.
    ///
    /// Arithmetic on `WIDTH`/`HEIGHT` rather than two numbers, so a wider box moves itself
    /// instead of hanging off the right edge — the failure `overlay.rs` records paying for
    /// once, where two panels overlapped by 165 pixels.
    #[test]
    fn the_box_is_centred_and_on_screen() {
        let w = COLS * ADVANCE + 2 * PAD;
        let h = ROWS * LINE + 2 * PAD;
        assert_eq!(w, 172, "34 columns at 5 pixels, plus a pixel each side");
        assert_eq!(h, 72, "10 rows at 7 pixels, plus a pixel each side");
        assert_eq!(MENU_X, 106);
        assert_eq!(MENU_Y, 76);
        assert!(MENU_X + w <= WIDTH, "the box runs off the right edge");
        assert!(MENU_Y + h <= HEIGHT, "the box runs off the bottom");
        // Centred to within a pixel, which is all an odd remainder allows.
        assert!(WIDTH - (MENU_X + w) <= MENU_X, "not centred horizontally");
        assert!(HEIGHT - (MENU_Y + h) <= MENU_Y, "not centred vertically");
    }

    /// A shut menu draws nothing at all.
    ///
    /// The one property of `draw` a test can read: it writes no pixel. Asserted against a
    /// whole frame rather than a sample, because a box drawn at the wrong origin would
    /// miss any single pixel one might check.
    #[test]
    fn a_shut_menu_draws_nothing() {
        let mut buf = vec![0x00AB_CDEFu32; WIDTH * HEIGHT];
        draw(&mut buf, &KeyMenu::new(), Preset::AzertyPunchLow);
        assert!(
            buf.iter().all(|&p| p == 0x00AB_CDEF),
            "a shut menu wrote a pixel"
        );
    }

    /// An open menu draws its box, and only inside it.
    ///
    /// The complement of the test above, and what makes that one about the `open` flag
    /// rather than about a `draw` that never draws. Ink is confined to the box's extent,
    /// which is the property a wrong origin or an over-wide row would break.
    #[test]
    fn an_open_menu_draws_inside_its_box_only() {
        let mut buf = vec![0x00AB_CDEFu32; WIDTH * HEIGHT];
        let mut m = KeyMenu::new();
        m.update(&act(|a| a.menu_toggled = true), Preset::AzertyPunchLow);
        draw(&mut buf, &m, Preset::AzertyPunchLow);
        let w = COLS * ADVANCE + 2 * PAD;
        let h = ROWS * LINE + 2 * PAD;
        let mut changed = 0usize;
        for yy in 0..HEIGHT {
            for xx in 0..WIDTH {
                let inside =
                    (MENU_X..MENU_X + w).contains(&xx) && (MENU_Y..MENU_Y + h).contains(&yy);
                let p = buf[yy * WIDTH + xx];
                if inside {
                    if p != 0x00AB_CDEF {
                        changed += 1;
                    }
                } else {
                    assert_eq!(p, 0x00AB_CDEF, "ink at ({xx}, {yy}), outside the box");
                }
            }
        }
        assert_eq!(
            changed,
            w * h,
            "the whole box is opaque, background included"
        );
    }

    /// The highlighted row's keys are what the summary shows.
    ///
    /// Not the current preset's — previewing is the whole reason the summary is there. The
    /// two are deliberately different in this test: the cursor is walked away from the
    /// preset in force, and the drawn rows must follow the cursor.
    #[test]
    fn the_summary_previews_the_highlighted_row() {
        let mut m = KeyMenu::new();
        m.update(&act(|a| a.menu_toggled = true), Preset::AzertyPunchLow);
        assert_eq!(m.selected().preset(), Preset::AzertyPunchLow);
        // Two rows down is QWERTY punches low, whose stick prints differently.
        for _ in 0..2 {
            m.update(&act(|a| a.menu_down = true), Preset::AzertyPunchLow);
        }
        let p = m.selected().preset();
        assert_eq!(p, Preset::QwertyPunchLow);
        assert_eq!(stick_label(p), "W A S D", "the preview follows the cursor");
        assert_eq!(punch_label(p), "J K L");
        // Drawing with a *different* current preset must not change any of that: the
        // `(current)` marker is the only thing `current` decides.
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        draw(&mut buf, &m, Preset::AzertyPunchLow);
        let mut other = vec![0u32; WIDTH * HEIGHT];
        draw(&mut other, &m, Preset::QwertyCabinet);
        assert_ne!(
            buf, other,
            "the current marker moved and nothing else could"
        );
    }

    /// `MenuRow::ALL` lists every row once, and the presets exactly once each.
    ///
    /// The count is a literal for the reason `Key::ALL`'s is: several tests above iterate
    /// this array and would silently stop covering a row added to the enum and not to it.
    #[test]
    fn all_lists_every_row_exactly_once() {
        assert_eq!(MenuRow::ALL.len(), 5, "four presets and the restore");
        for (i, a) in MenuRow::ALL.iter().enumerate() {
            for b in &MenuRow::ALL[i + 1..] {
                assert_ne!(a, b, "{a:?} appears twice");
                assert_ne!(a.label(), b.label(), "{a:?} and {b:?} share a label");
            }
        }
        for p in Preset::ALL {
            assert_eq!(
                MenuRow::ALL
                    .iter()
                    .filter(|r| **r == MenuRow::Use(p))
                    .count(),
                1,
                "{p:?} must have exactly one row"
            );
        }
    }
}
