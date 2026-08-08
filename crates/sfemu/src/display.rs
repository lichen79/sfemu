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
//! The one thing that *is* checked without a display, in `main.rs`: a bad ROM path
//! with `--play` reports the load error and exits, rather than opening a window onto
//! a machine that never booted.

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
fn translate(k: minifb::Key) -> Option<Key> {
    use minifb::Key as M;
    Some(match k {
        M::Up => Key::Up,
        M::Down => Key::Down,
        M::Left => Key::Left,
        M::Right => Key::Right,
        M::A => Key::A,
        M::S => Key::S,
        M::D => Key::D,
        M::Z => Key::Z,
        M::X => Key::X,
        M::C => Key::C,
        M::Key1 => Key::Num1,
        M::Key2 => Key::Num2,
        M::Key5 => Key::Num5,
        M::Key6 => Key::Num6,
        M::F2 => Key::F2,
        M::F3 => Key::F3,
        M::F5 => Key::F5,
        M::F8 => Key::F8,
        M::F12 => Key::F12,
        M::P => Key::P,
        M::Period => Key::Period,
        M::Escape => Key::Escape,
        _ => return None,
    })
}
