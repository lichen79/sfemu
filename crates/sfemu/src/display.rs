//! The window. The only code in this project that talks to a windowing library.
//!
//! # The display boundary
//!
//! **No logic behind this line.** A window cannot be asserted about — `cargo test`
//! has no display, and "the right pixels reached the glass" is not something a test
//! can read back — so a decision made in this file is a decision that cannot be
//! tested. Every decision therefore lives in `crates/frontend`, which has never
//! heard of a window: how many frames a host tick owes, which board input a key is,
//! what colour a pen is, what bytes a save state is. The loop's ordering lives in
//! [`crate::loop_`], behind a trait a test can script.
//!
//! # Why this file has no tests
//!
//! Its whole content is calls into `minifb` and one total match. There is nothing
//! here a test could assert that would not be asserting about `minifb` — and a test
//! that opened a window would need a display to run, which CI does not have. The
//! honest statement is that this file is verified by running it, and the commit that
//! added it says so.
//!
//! Two things *are* checked. In `main.rs`: a bad ROM path with `--play` reports the
//! load error and exits, rather than opening a window onto a machine that never
//! booted. And here, in the one test below: that `minifb` is named in code in this
//! file alone.

use crate::loop_::Display;
use frontend::keys::{Key, KeySet};
use machine::video::{HEIGHT, WIDTH};
use minifb::{Scale, ScaleMode, WindowOptions};
use std::time::Instant;

/// How much larger than the board's own resolution the window opens.
///
/// 384×224 on a modern display is a postage stamp. Three is a scale a 1080p screen
/// fits with room for the title bar, and the window is resizable regardless.
const SCALE: usize = 3;

/// A real window.
pub struct Window {
    win: minifb::Window,
    /// When [`Display::elapsed_ns`] last ran. The one clock read in this project.
    last: Instant,
}

impl Window {
    /// Opens a window, or says why not.
    ///
    /// `AspectRatioStretch` so a resized window letterboxes rather than stretching
    /// SF2 into the wrong shape, and `set_target_fps(60)` so the library's own sleep
    /// holds the rate — the pacer then sees ticks near one frame's length and asks
    /// for one frame each, rather than spinning a core at thousands of ticks a
    /// second.
    pub fn open(title: &str) -> Result<Self, String> {
        let win = minifb::Window::new(
            title,
            WIDTH * SCALE,
            HEIGHT * SCALE,
            WindowOptions {
                resize: true,
                scale: Scale::X1,
                scale_mode: ScaleMode::AspectRatioStretch,
                ..Default::default()
            },
        )
        .map_err(|e| format!("cannot open a window: {e}"))?;
        let mut win = win;
        win.set_target_fps(60);
        Ok(Self {
            win,
            last: Instant::now(),
        })
    }
}

impl Display for Window {
    /// `update_with_buffer` takes `0x00RRGGBB`, which is what
    /// `frontend::pens_to_argb` produces and what `minifb`'s own `from_u8_rgb`
    /// example builds.
    fn present(&mut self, buf: &[u32]) -> Result<(), String> {
        self.win
            .update_with_buffer(buf, WIDTH, HEIGHT)
            .map_err(|e| format!("{e}"))
    }

    fn held_keys(&self) -> KeySet {
        let mut set = KeySet::new();
        for k in self.win.get_keys() {
            if let Some(k) = translate(k) {
                set.press(k);
            }
        }
        set
    }

    fn elapsed_ns(&mut self) -> u64 {
        let now = Instant::now();
        let ns = now.duration_since(self.last).as_nanos();
        self.last = now;
        // A host that was suspended for 585 years is not a case worth a branch, but
        // `as u64` would wrap it to a small number and a saturating cast reports the
        // stall the pacer is built to absorb.
        u64::try_from(ns).unwrap_or(u64::MAX)
    }

    fn is_open(&self) -> bool {
        self.win.is_open()
    }

    fn set_title(&mut self, title: &str) {
        self.win.set_title(title);
    }
}

/// One `minifb` key to one frontend key.
///
/// A total match, and the only reason this crate names `minifb::Key` at all. The map
/// itself — which board input each frontend key is — lives in `frontend::keys`,
/// where it is tested against the board's documented port bits.
///
/// `Escape` is mapped rather than handled here: the loop owns quitting, so a window
/// closed by its own button and one closed by Escape take the same path.
///
/// # `minifb::Key` names a position, not a letter
///
/// This is the one fact that makes this function more than plumbing, and it is not
/// documented anywhere in `minifb`. On macOS its `keyDown` handler passes
/// `[event keyCode]` — a **hardware scancode** — through a fixed table that names each
/// position after the letter a **US QWERTY** keyboard prints there. The active keyboard
/// layout never enters into it. So `M::Q` does not mean "the key that types q"; it means
/// "position 0x0c", which types `q` on QWERTY and **`a`** on AZERTY.
///
/// The consequence is that a map written by letter is correct on one layout and wrong on
/// others, silently: the keys do something, just not what the docs say. Verified on this
/// machine with Carbon's `UCKeyTranslate` against the `French - PC` layout — position
/// 0x0c types `a`, and position 0x0d (`M::W`) types `z`.
///
/// P1's keys are therefore mapped **by position**, and three of them are places where
/// AZERTY and QWERTY disagree:
///
/// | frontend key | this map | AZERTY label | QWERTY label |
/// |---|---|---|---|
/// | `Key::Z` (up)     | `M::W`         | **Z** | W |
/// | `Key::Q` (left)   | `M::A`         | **Q** | A |
/// | `Key::M` (fierce) | `M::Semicolon` | **M** | ; |
/// | `Key::S` (down)   | `M::S` | S | S |
/// | `Key::D` (right)  | `M::D` | D | D |
///
/// `Key::M` is the sharpest of the three, because AZERTY moves `M` off the bottom row
/// entirely: it is the home row's last letter, right of `L`, at position 0x29. `minifb`
/// calls that position `Semicolon`. Its own `M::M` is 0x2e, which types `,` here.
///
/// `frontend::Key`'s variants are named for the **AZERTY** label, because that is the
/// keyboard this is played on and `Z`/`S`/`Q`/`D` and `K`/`L`/`M` are what the README and
/// the usage text tell the player to press. The mismatch between `Key::M` and
/// `M::Semicolon` is not a bug and must not be "tidied" — see
/// `tests::player_ones_keys_are_mapped_by_position`, which pins all three.
fn translate(k: minifb::Key) -> Option<Key> {
    use minifb::Key as M;
    Some(match k {
        // Player 1's cluster. The stick is by position (see above): `M::W`/`M::A` are
        // the keys an AZERTY keyboard labels Z and Q, which is what the player is told
        // to press. `M::Z`/`M::Q` — the keys labelled W and A here — are deliberately
        // *not* mapped.
        M::W => Key::Z,
        M::S => Key::S,
        M::A => Key::Q,
        M::D => Key::D,
        M::I => Key::I,
        M::O => Key::O,
        M::P => Key::P,
        M::K => Key::K,
        M::L => Key::L,
        // Live only under a QWERTY preset, where the home row's run of three is `J K L`
        // rather than AZERTY's `K L M`. Mapped unconditionally all the same: this
        // function turns a scancode into a `frontend::Key` and knows nothing about
        // presets — `frontend::keys` decides which keys a preset reads, and under an
        // AZERTY one this key simply presses nothing. Position 0x26, the same letter on
        // both layouts, so it is not one of the traps above.
        M::J => Key::J,
        // The third punch, and the second position trap in this function. The key
        // **labelled M** is position 0x29, which `minifb` names `Semicolon` after the US
        // QWERTY letter there. `M::M` is position 0x2e — the key labelled `,` here — and
        // mapping it would put P1's fierce punch one key past the end of the home row.
        M::Semicolon => Key::M,
        // Player 2's. `NumPad4`-`NumPad9` and not `Key4`-`Key9`: the number row's 1, 2,
        // 5 and 6 are the cabinet's start and coin buttons, and a keypad is a separate
        // set of keycodes rather than an alias for them.
        M::Up => Key::Up,
        M::Down => Key::Down,
        M::Left => Key::Left,
        M::Right => Key::Right,
        M::NumPad7 => Key::NumPad7,
        M::NumPad8 => Key::NumPad8,
        M::NumPad9 => Key::NumPad9,
        M::NumPad4 => Key::NumPad4,
        M::NumPad5 => Key::NumPad5,
        M::NumPad6 => Key::NumPad6,
        M::Key1 => Key::Num1,
        M::Key2 => Key::Num2,
        M::Key5 => Key::Num5,
        M::Key6 => Key::Num6,
        M::F2 => Key::F2,
        M::F3 => Key::F3,
        M::F5 => Key::F5,
        M::F8 => Key::F8,
        M::F12 => Key::F12,
        M::F11 => Key::F11,
        M::Period => Key::Period,
        M::Escape => Key::Escape,
        M::F1 => Key::F1,
        M::F4 => Key::F4,
        M::F6 => Key::F6,
        M::F7 => Key::F7,
        M::PageUp => Key::PageUp,
        M::PageDown => Key::PageDown,
        M::Home => Key::Home,
        M::F9 => Key::GfxToggled,
        M::F10 => Key::GfxView,
        M::LeftBracket => Key::BracketLeft,
        M::RightBracket => Key::BracketRight,
        M::Enter => Key::Enter,
        // The key menu. `Tab` because there was nothing else: all twelve of `F1`-`F12`
        // are mapped above, and the three keys a menu reaches for by instinct are taken
        // — `Enter` acts on the graphics view, `F1` is the debugger overlay, and
        // `Escape` quits. Position 0x30, and the same key on both layouts.
        M::Tab => Key::Tab,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    /// `minifb` is named in code in this file and nowhere else.
    ///
    /// The whole testability argument rests on it: a `use minifb::Key` in `frontend`
    /// would put the key map — the thing most worth testing — behind the display
    /// boundary, and the compiler would not object. Nothing else in this project
    /// enforces it, so this does.
    ///
    /// The scan, why an absence has to be asserted this way, and why prose is exempt are
    /// all in [`crate::confine`] — shared with `audio.rs`'s identical check on `cpal`,
    /// because two hand-written copies of this rule drifted in exactly the way a
    /// confinement test cannot afford: a walk that quietly stopped covering the tree
    /// keeps passing.
    #[test]
    fn the_windowing_library_is_named_in_one_file() {
        let all = crate::confine::mentions("minifb", &[]);
        assert!(
            all.checked > 20,
            "the walk must have found the tree: {} files",
            all.checked
        );
        // This file names it, and no other. The premise first: without it, a scan that
        // found nothing anywhere would pass.
        assert!(
            all.code
                .iter()
                .any(|m| m.starts_with("sfemu/src/display.rs:")),
            "the premise: this file names `minifb` in code, so the check can fail"
        );
        let elsewhere = crate::confine::mentions("minifb", &["display.rs", "Cargo.toml"]);
        assert_eq!(
            elsewhere.code,
            Vec::<String>::new(),
            "`minifb` must be named in code only in sfemu/src/display.rs"
        );
        // And the dependency edge itself is sfemu's alone.
        assert_eq!(
            all.manifests,
            vec![std::path::PathBuf::from("sfemu/Cargo.toml")],
            "only sfemu may depend on a windowing library"
        );
    }

    /// Every frontend key is reachable from some keypress.
    ///
    /// `translate` ends in `_ => return None`, so it is **not** the total match the
    /// plan for this work assumed: adding a `Key` variant compiles fine and produces
    /// a key no keyboard can press. The failure is silent and total — the feature
    /// simply does nothing, with every unit test in `frontend` green, because
    /// `frontend` never sees a keyboard.
    ///
    /// The candidate list below is a second copy of the map, which is the thing this
    /// project usually refuses. Justified here: the two copies run in opposite
    /// directions, and what is asserted is that the *forward* map covers every
    /// variant of `Key::ALL`. A copy that drifted would fail this, which is the
    /// opposite of the usual duplicate-map problem, where drift is invisible.
    #[test]
    fn every_frontend_key_can_be_produced_by_a_keypress() {
        use super::translate;
        use frontend::Key;
        use minifb::Key as M;
        let candidates = [
            M::W,
            M::S,
            M::A,
            M::D,
            M::I,
            M::O,
            M::P,
            M::J,
            M::K,
            M::L,
            M::Semicolon,
            M::Up,
            M::Down,
            M::Left,
            M::Right,
            M::NumPad7,
            M::NumPad8,
            M::NumPad9,
            M::NumPad4,
            M::NumPad5,
            M::NumPad6,
            M::Key1,
            M::Key2,
            M::Key5,
            M::Key6,
            M::F1,
            M::F2,
            M::F3,
            M::F4,
            M::F5,
            M::F6,
            M::F7,
            M::F8,
            M::F11,
            M::F12,
            M::Period,
            M::Escape,
            M::PageUp,
            M::PageDown,
            M::Home,
            M::F9,
            M::F10,
            M::LeftBracket,
            M::RightBracket,
            M::Enter,
            M::Tab,
        ];
        for want in Key::ALL {
            let n = candidates
                .iter()
                .filter(|&&c| translate(c) == Some(want))
                .count();
            assert_eq!(
                n, 1,
                "{want:?} must be produced by exactly one key, not {n}"
            );
        }
        // And nothing is mapped that is not a `Key`: an unhandled key is `None`, not a
        // panic, because `minifb` reports keys this program has no opinion about.
        assert_eq!(translate(M::Y), None, "an unmapped letter is None");
        assert_eq!(translate(M::Space), None, "and so is the space bar");

        // The number row and the keypad are different keys. P2's six buttons are on the
        // keypad while 1, 2, 5 and 6 on the number row are the cabinet's start and coin
        // buttons — a translation that treated `Key5` and `NumPad5` as the same thing
        // would make inserting a coin throw P2's forward kick, and every assertion
        // above would still pass.
        assert_eq!(
            translate(M::Key5),
            Some(Key::Num5),
            "the number row's 5 is coin 1"
        );
        assert_eq!(
            translate(M::NumPad5),
            Some(Key::NumPad5),
            "the keypad's 5 is P2's forward kick"
        );
        assert_eq!(translate(M::Key4), None, "the number row's 4 is nothing");
        assert_eq!(translate(M::NumPad1), None, "nor is the keypad's 1");
    }

    /// P1's stick is mapped by keyboard **position**, so the diamond survives AZERTY.
    ///
    /// `minifb::Key` names a hardware scancode after the letter US QWERTY prints on it —
    /// the active layout is never consulted. So `M::Q` means "position 0x0c", which types
    /// `q` on QWERTY and `a` on a French keyboard. Verified on this machine with Carbon's
    /// `UCKeyTranslate` against `French - PC`: position 0x0c types `a`, 0x0d types `z`.
    ///
    /// This test exists because the correct map **looks like a typo**: `M::W => Key::Z`
    /// reads as a mistake, and "fixing" it to `M::Z => Key::Z` would move P1's up key from
    /// the one labelled Z to the one labelled W and break the diamond. Every other test in
    /// this file would stay green — `every_frontend_key_can_be_produced_by_a_keypress` only
    /// asks that *some* key produces each variant, and `frontend`'s tests never see a
    /// keyboard at all. Nothing else in the project would notice.
    ///
    /// Both directions are asserted. Without the negative half, a map that produced
    /// `Key::Z` from both `M::W` and `M::Z` would pass — and that is the specific shape a
    /// well-meaning "let me also support QWERTY" edit takes. It cannot be supported here:
    /// one physical key is one board input, and the positions that spell ZQSD on AZERTY
    /// spell WASD on QWERTY. Which is right is a layout question this program cannot see.
    #[test]
    fn player_ones_keys_are_mapped_by_position() {
        use super::translate;
        use frontend::Key;
        use minifb::Key as M;

        // The two keys AZERTY shifts. `M::W` is the key labelled Z; `M::A` is labelled Q.
        assert_eq!(
            translate(M::W),
            Some(Key::Z),
            "P1 up is position 0x0d, the key an AZERTY keyboard labels Z"
        );
        assert_eq!(
            translate(M::A),
            Some(Key::Q),
            "P1 left is position 0x00, the key an AZERTY keyboard labels Q"
        );
        // And the two US-QWERTY-named positions for those letters press nothing. These are
        // the keys labelled W and A on this keyboard: outside the diamond, so mapping them
        // would give one board input two keys and hide the bug above.
        assert_eq!(
            translate(M::Z),
            None,
            "position 0x06 is labelled W here and must press nothing"
        );
        assert_eq!(
            translate(M::Q),
            None,
            "position 0x0c is labelled A here and must press nothing"
        );
        // The other two stick keys are unaffected: S and D sit in the same place on both
        // layouts, which is why only two of the four move.
        assert_eq!(translate(M::S), Some(Key::S), "S is S on both layouts");
        assert_eq!(translate(M::D), Some(Key::D), "and D is D");
        // Five of P1's six buttons are layout-stable: I O P and K L are identical on
        // AZERTY and QWERTY, so they are mapped by name and position at once.
        for (m, k) in [
            (M::I, Key::I),
            (M::O, Key::O),
            (M::P, Key::P),
            (M::K, Key::K),
            (M::L, Key::L),
        ] {
            assert_eq!(translate(m), Some(k), "{k:?} is in the same place on both");
        }

        // The sixth is the third trap, and the one that would be hardest to diagnose from
        // the outside. AZERTY moves `M` off the bottom row to the home row's right end,
        // position 0x29, which `minifb` names `Semicolon`. Verified with `UCKeyTranslate`:
        // 0x29 types `m` on `French - PC`, and 0x2e — `M::M` — types `,`.
        assert_eq!(
            translate(M::Semicolon),
            Some(Key::M),
            "P1 fierce is position 0x29, the key an AZERTY keyboard labels M"
        );
        assert_eq!(
            translate(M::M),
            None,
            "position 0x2e is labelled `,` here and must press nothing"
        );
    }
}
