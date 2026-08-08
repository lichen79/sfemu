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
    /// A test that walks the source tree is unusual, and justified: asserting a `use`
    /// is *absent* cannot be done any other way. What the type system can express is
    /// already expressed — `frontend` does not depend on `minifb`, so a `use` there
    /// would not compile — but `sfemu` does, and nothing stops a later `use minifb`
    /// in `loop_.rs` or `main.rs` from quietly moving a decision out of reach.
    ///
    /// # Comments are allowed to name it
    ///
    /// `frontend::keys` and `frontend::pixels` both explain themselves by reference
    /// to `minifb` — "a `minifb::Key` here would make this module part of the display
    /// boundary" is the clearest statement of the rule in the project, and a check
    /// that forbade it would delete the documentation to protect the constraint. So
    /// the check is on *code* lines: any line whose first non-space characters are
    /// `//` is prose.
    ///
    /// The heuristic's limit, stated rather than hidden: a `/* */` block comment
    /// naming `minifb` would be reported as code. That is a false positive, which
    /// fails loudly and is fixed by rewording — the failure this test exists to
    /// catch cannot hide behind it.
    #[test]
    fn the_windowing_library_is_named_in_one_file() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("crates/sfemu has two ancestors")
            .join("crates");
        assert!(root.is_dir(), "the crates directory must exist: {root:?}");

        let mut offenders = Vec::new();
        let mut checked = 0usize;
        let mut manifests = Vec::new();
        walk(&root, &mut |path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let is_rs = name.ends_with(".rs");
            let is_manifest = name == "Cargo.toml";
            if !is_rs && !is_manifest {
                return;
            }
            checked += 1;
            let text = std::fs::read_to_string(path).expect("a source file this crate can read");
            let rel = path.strip_prefix(&root).unwrap_or(path).to_path_buf();
            for (n, line) in text.lines().enumerate() {
                if !line.contains("minifb") {
                    continue;
                }
                if is_manifest {
                    manifests.push(rel.clone());
                    continue;
                }
                // Prose may name it; code may not.
                if line.trim_start().starts_with("//") {
                    continue;
                }
                offenders.push(format!("{}:{}: {}", rel.display(), n + 1, line.trim()));
            }
        });

        assert!(
            checked > 20,
            "the walk must have found the tree: {checked} files"
        );
        // This file names it, and no other. Compared as a whole list rather than by
        // counting: a count of one passes for the wrong one file.
        let mine: Vec<_> = offenders
            .iter()
            .filter(|o| o.starts_with("sfemu/src/display.rs:"))
            .collect();
        assert!(
            !mine.is_empty(),
            "the premise: this file names `minifb` in code, so the check can fail"
        );
        assert_eq!(
            offenders.len(),
            mine.len(),
            "`minifb` must be named in code only in sfemu/src/display.rs, found:\n{}",
            offenders.join("\n")
        );
        // And the dependency edge itself is sfemu's alone.
        assert_eq!(
            manifests,
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
            M::Up,
            M::Down,
            M::Left,
            M::Right,
            M::A,
            M::S,
            M::D,
            M::Z,
            M::X,
            M::C,
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
            M::F12,
            M::P,
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
        assert_eq!(translate(M::Q), None, "an unmapped key is None");
        assert_eq!(
            translate(M::F11),
            None,
            "including a neighbouring function key"
        );
    }

    /// Calls `f` for every file under `dir`, recursively.
    ///
    /// Skips `target` — a build directory holds vendored sources that would make this
    /// check report someone else's `use minifb` as ours. It is git-ignored and not
    /// normally under `crates/`, but a stray one would turn a real check into a
    /// permanent failure.
    fn walk(dir: &std::path::Path, f: &mut impl FnMut(&std::path::Path)) {
        let entries = std::fs::read_dir(dir).expect("a directory this crate can read");
        for e in entries {
            let path = e.expect("a readable directory entry").path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) == Some("target") {
                    continue;
                }
                walk(&path, f);
            } else {
                f(&path);
            }
        }
    }
}
