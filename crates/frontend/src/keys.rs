//! The keyboard map, and the difference between holding a button and pressing a key.
//!
//! # Two kinds of input, and why they must not be treated alike
//!
//! A **game** input is level-triggered: the board reads what is held right now,
//! because holding down is how you crouch. A **control** — pause, step, save, load,
//! reset, screenshot, quit — is edge-triggered: it acts on the transition to
//! pressed. A held `.` that stepped every frame would run the game at full speed
//! while claiming to be paused, and a held F5 would write sixty save states a
//! second.
//!
//! That asymmetry is the reason [`Controls`] is a struct with a method rather than
//! a free function: the edge needs last frame's keys.
//!
//! # This module does not know about polarity
//!
//! Every field of [`machine::Inputs`] is `true` for *pressed*, and `machine` does
//! the active-low conversion in one place. This module sets booleans and nothing
//! else. Computing port values here would duplicate the project's only piece of
//! polarity logic — and `machine::inputs`' own module comment records what getting
//! it backwards costs: a board that "boots with every button held, which looks like
//! a game bug rather than a bus bug and costs a day to find".
//!
//! # Both players, on one keyboard
//!
//! P1 has the left of the keyboard — `Z`/`S`/`Q`/`D` for the stick, `K`/`L`/`M` for
//! the punches and `I`/`O`/`P` for the kicks directly above them. P2 has the right:
//! the arrow keys, and the numeric keypad's `4`/`5`/`6` under `7`/`8`/`9`.
//!
//! **Punches below kicks, in both clusters — the reverse of a six-button cabinet.**
//! That is deliberate and it was asked for: on an AZERTY keyboard `K L M` is a run of
//! three on the home row, so the punches land under the resting fingers and the kicks
//! go on the row above. The two clusters agree with each other, which is the property
//! that matters once the arrangement is unconventional — a player who learns one half
//! has learned the other.
//!
//! Five consequences, all of them things a later reader would otherwise rediscover:
//!
//! - **The letters are AZERTY labels, and this module cannot see a layout.** A variant
//!   named `Z` means "the key the player was told is Z" — on a French keyboard, the
//!   position a US QWERTY board calls W. `sfemu`'s `display::translate` owns that,
//!   because it is the only code that sees a keyboard: `minifb::Key` names a hardware
//!   position after a US letter and never consults the active layout, so `M::W` is what
//!   produces [`Key::Z`] here. Nothing in this file changes with the layout, which is
//!   the point of the split — but it means the names here are labels, not evidence.
//! - **[`Key::M`] is the sharpest case of that, and it is why the punches can be a
//!   home-row run at all.** AZERTY moves `M` off the bottom row to the home row's right
//!   end, right of `L`. That position is the one US QWERTY prints `;` on, so
//!   `display::translate` produces this key from `minifb`'s `Semicolon` and *not* from
//!   its `M`, which is the comma key here. On a QWERTY keyboard the third punch is
//!   therefore the semicolon, not the letter M.
//! - **P2 needs a numeric keypad.** A keyboard without one leaves P2's six buttons
//!   unreachable while its stick still works, which is worse than nothing. `Inputs`
//!   carries P2 either way, for a gamepad or netplay to fill in.
//! - **No letter key is a control any more.** Pause moved off `P`, which is now P1's
//!   roundhouse kick, onto `F11` — the one gap that was left in `F1`-`F12`. Every
//!   control is now a function key or a navigation key, so a letter arriving at a
//!   control is a bug with a shape.
//! - **The two halves must not leak.** `tests::each_game_key_clears_its_own_port_bit`
//!   asserts all three ports for every one of the 25 game keys, so a P1 key that also
//!   moved P2 fails its own row; `tests::no_control_key_reaches_the_board` covers the
//!   other direction over every remaining key.

use machine::Inputs;

/// A key this frontend understands.
///
/// The frontend's own enum, deliberately **not** the windowing library's. A
/// `minifb::Key` here would make this module — the key map, the thing most worth
/// testing — part of the display boundary. `sfemu`'s `display` module translates,
/// in a total match with no decisions in it.
///
/// The variants are named after the **physical key**, not the button it presses:
/// `Z` is a key called Z, and which of the twelve board inputs it is lives in
/// [`Controls::update`] alone. Naming them `P1Up` would put the map in two places, and
/// the two would then have to be changed together — this remap moved eleven of them
/// and touched no variant name.
///
/// "A key called Z" means **the key the player reads as Z**, which is an AZERTY label:
/// `sfemu`'s `display::translate` produces `Key::Z` from `minifb`'s `W`, since
/// `minifb::Key` names hardware positions after US QWERTY letters. That indirection is
/// deliberate and lives entirely at the display boundary; this enum is layout-blind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// P1 stick up.
    Z,
    /// P1 stick down.
    S,
    /// P1 stick left.
    Q,
    /// P1 stick right.
    D,
    /// P1 short kick.
    I,
    /// P1 forward kick.
    O,
    /// P1 roundhouse kick.
    P,
    /// P1 jab.
    K,
    /// P1 strong.
    L,
    /// P1 fierce.
    ///
    /// The key **labelled M**, which on AZERTY is the home row's rightmost letter, next
    /// to `L`. `display::translate` produces this from `minifb`'s `Semicolon` — position
    /// 0x29 — and not from `M::M`, which is the comma key here. One more reason the
    /// letters in this enum are labels rather than evidence.
    ///
    /// Live under the AZERTY presets and dead under the QWERTY ones, where [`Key::J`]
    /// takes its place in the row. See [`Preset`].
    M,
    /// P1's third button under a **QWERTY** preset, where the home row's run of three is
    /// `J K L` rather than AZERTY's `K L M`.
    ///
    /// This variant existed, was deleted when the punches moved to `K L M`, and is back
    /// for the presets: AZERTY moves `M` onto the home row and pushes `;` off it, so the
    /// two layouts genuinely need different letters for the same three positions. Dead
    /// under the AZERTY presets, exactly as [`Key::M`] is dead under the QWERTY ones —
    /// one physical key, one board input, and which keys are live is preset-dependent.
    J,
    /// P2 stick up.
    Up,
    /// P2 stick down.
    Down,
    /// P2 stick left.
    Left,
    /// P2 stick right.
    Right,
    /// P2 short kick.
    NumPad7,
    /// P2 forward kick.
    NumPad8,
    /// P2 roundhouse kick.
    NumPad9,
    /// P2 jab.
    NumPad4,
    /// P2 strong.
    NumPad5,
    /// P2 fierce.
    NumPad6,
    /// Start 1.
    Num1,
    /// Start 2.
    Num2,
    /// Coin 1.
    Num5,
    /// Coin 2.
    Num6,
    /// The test switch. Held at boot it enters the service menu.
    F2,
    /// Reset the machine.
    F3,
    /// Save state.
    F5,
    /// Load state.
    F8,
    /// Screenshot.
    F12,
    /// Pause / resume.
    ///
    /// `F11` and not `P`, which is P1's roundhouse kick. The letter area belongs to the
    /// players; `F11` was the one gap left in `F1`-`F12`.
    F11,
    /// Step one frame while paused.
    Period,
    /// Quit.
    Escape,
    /// Show or hide the debugger overlay.
    F1,
    /// Step one *instruction*, which is not the same as `Period`'s one frame.
    F4,
    /// Move the scroll focus between the disassembly and the memory dump.
    F6,
    /// Set or clear a breakpoint at the instruction about to execute.
    F7,
    /// Scroll the focused panel back.
    PageUp,
    /// Scroll the focused panel forward.
    PageDown,
    /// Return the focused panel to following the machine.
    Home,
    /// Show or hide the graphics viewer.
    GfxToggled,
    /// Cycle which graphics view is shown.
    GfxView,
    /// Page or move back within the graphics view.
    BracketLeft,
    /// Page or move forward within the graphics view.
    BracketRight,
    /// Act on the current graphics view.
    Enter,
    /// Open or close the key menu.
    ///
    /// `Tab` because there was nothing else. All twelve of `F1`-`F12` are mapped, and the
    /// three keys a menu would reach for by instinct are all taken and one of them is
    /// dangerous: `Enter` acts on the graphics view, `F1` is the debugger overlay, and
    /// `Escape` **quits**. `Tab` is position 0x30, unmapped until now, and prints the same
    /// on AZERTY and QWERTY — so the one key that reaches the key menu is not itself a
    /// layout question.
    Tab,
}

impl Key {
    /// Every variant, for the tests that must cover all of them.
    ///
    /// `tests::all_lists_every_key_exactly_once` fails if a variant is added and
    /// not listed here, which is what stops the tests that iterate this from
    /// quietly narrowing.
    pub const ALL: [Key; 46] = [
        Key::Z,
        Key::S,
        Key::Q,
        Key::D,
        Key::I,
        Key::O,
        Key::P,
        Key::K,
        Key::L,
        Key::M,
        Key::J,
        Key::Up,
        Key::Down,
        Key::Left,
        Key::Right,
        Key::NumPad7,
        Key::NumPad8,
        Key::NumPad9,
        Key::NumPad4,
        Key::NumPad5,
        Key::NumPad6,
        Key::Num1,
        Key::Num2,
        Key::Num5,
        Key::Num6,
        Key::F2,
        Key::F3,
        Key::F5,
        Key::F8,
        Key::F12,
        Key::F11,
        Key::Period,
        Key::Escape,
        Key::F1,
        Key::F4,
        Key::F6,
        Key::F7,
        Key::PageUp,
        Key::PageDown,
        Key::Home,
        Key::GfxToggled,
        Key::GfxView,
        Key::BracketLeft,
        Key::BracketRight,
        Key::Enter,
        Key::Tab,
    ];

    /// This key's bit in a [`KeySet`].
    ///
    /// A `match` and not `self as u32`: a cast makes the bit a function of
    /// declaration order, so reordering the enum for readability would silently
    /// remap every key. Written out, a reorder changes nothing.
    pub(crate) const fn bit(self) -> u32 {
        match self {
            Key::Z => 0,
            Key::S => 1,
            Key::Q => 2,
            Key::D => 3,
            Key::I => 4,
            Key::O => 5,
            Key::P => 6,
            // `M` takes the bit `J` had. Renaming a key is not a reason to move any
            // other key's bit, and `scripts/mutate.py`'s control mutant is parked on 62
            // on the strength of that.
            Key::M => 7,
            Key::K => 8,
            Key::L => 9,
            Key::Num1 => 10,
            Key::Num2 => 11,
            Key::Num5 => 12,
            Key::Num6 => 13,
            Key::F2 => 14,
            Key::F3 => 15,
            Key::F5 => 16,
            Key::F8 => 17,
            Key::F12 => 18,
            Key::F11 => 19,
            Key::Period => 20,
            Key::Escape => 21,
            Key::F1 => 22,
            Key::F4 => 23,
            Key::F6 => 24,
            Key::F7 => 25,
            Key::PageUp => 26,
            Key::PageDown => 27,
            Key::Home => 28,
            Key::GfxToggled => 29,
            Key::GfxView => 30,
            Key::BracketLeft => 31,
            Key::BracketRight => 32,
            Key::Enter => 33,
            // Player 2's ten, added last so no existing key's bit moved. `KeySet` is a
            // `u64` and this remap took it from 34 keys to 44 — bits 44 up are free, and
            // `scripts/mutate.py`'s control mutant parks `Escape` on 62.
            Key::Up => 34,
            Key::Down => 35,
            Key::Left => 36,
            Key::Right => 37,
            Key::NumPad7 => 38,
            Key::NumPad8 => 39,
            Key::NumPad9 => 40,
            Key::NumPad4 => 41,
            Key::NumPad5 => 42,
            Key::NumPad6 => 43,
            // The presets' two, appended for the same reason P2's ten were: an existing
            // key's bit moving is a silent remap of every `KeySet` in flight. 46 keys, and
            // bits 46 up stay free — `mutate.py`'s control mutant still has 62.
            Key::J => 44,
            Key::Tab => 45,
        }
    }
}

/// A complete, verified arrangement of the twelve player buttons.
///
/// # Why presets and not per-key rebinding
///
/// "Press the key you want" cannot work in this program without a different `Key` type.
/// This enum is layout-blind by construction — a variant is a *label*, and only
/// `sfemu`'s `display::translate` knows which hardware position produces it — so a
/// capture loop could only ever offer the positions the map already reaches. A player
/// pressing the key their keyboard prints `W` on would get nothing, with no way to tell
/// that from a bug. Shipping whole maps that have each been asserted against the board's
/// ports avoids the question entirely.
///
/// # Only two axes, because the stick is not one
///
/// The obvious matrix is {AZERTY, QWERTY} × {punches low, punches high}, and its first
/// axis is **half a fiction**: `Z S Q D` on AZERTY and `W A S D` on QWERTY are the *same
/// four physical keys*. `minifb` names positions, so one map reads correctly on both and
/// only the printed letters differ. A "QWERTY stick" preset would change nothing.
///
/// What genuinely varies is which row punches, and which three letters the rows use —
/// `K L M` is AZERTY's home-row run of three, `J K L` is QWERTY's, because AZERTY moves
/// `M` onto the home row and pushes `;` off it.
///
/// # Which keys are live depends on the preset
///
/// Under [`Preset::AzertyPunchLow`] and [`Preset::AzertyCabinet`], [`Key::J`] presses
/// nothing. Under the two QWERTY presets, [`Key::M`] does. That is not an oversight — one
/// physical key, one board input — but it means "this key does nothing" is a
/// preset-dependent claim, which is why the tests assert it per preset rather than once.
///
/// The controls are **not** part of a preset. A preset that could move `Escape` could
/// strand a player in a window they cannot close, so the function keys, coins and starts
/// are fixed for every one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Preset {
    /// AZERTY labels, punches on the home row **below** the kicks: `K L M` over `I O P`,
    /// and the keypad's `4 5 6` punching under `7 8 9`.
    ///
    /// The default, and what was asked for. It inverts a six-button cabinet on purpose:
    /// `K L M` is a run of three on the AZERTY home row, so the punches land under the
    /// resting fingers.
    #[default]
    AzertyPunchLow,
    /// AZERTY labels, punches **above** the kicks in a real cabinet's order: `I O P` over
    /// `K L M`, keypad `7 8 9` over `4 5 6`.
    AzertyCabinet,
    /// QWERTY labels, punches below the kicks: `J K L` over `I O P`, keypad `4 5 6` under
    /// `7 8 9`.
    QwertyPunchLow,
    /// QWERTY labels, punches above the kicks — the arrangement this project shipped
    /// before the remap, on a US keyboard: `I O P` over `J K L`, keypad `7 8 9` over
    /// `4 5 6`.
    QwertyCabinet,
}

impl Preset {
    /// Every preset, in the order the menu lists them.
    pub const ALL: [Preset; 4] = [
        Preset::AzertyPunchLow,
        Preset::AzertyCabinet,
        Preset::QwertyPunchLow,
        Preset::QwertyCabinet,
    ];

    /// The name the menu shows.
    pub const fn name(self) -> &'static str {
        match self {
            Preset::AzertyPunchLow => "AZERTY  punches low",
            Preset::AzertyCabinet => "AZERTY  punches high",
            Preset::QwertyPunchLow => "QWERTY  punches low",
            Preset::QwertyCabinet => "QWERTY  punches high",
        }
    }

    /// The three keys this preset puts P1's punches on, jab first.
    ///
    /// Written out per preset rather than derived from a "letters" and a "row order"
    /// field. Deriving them would make the four presets one expression with two
    /// booleans, and a sign error in it would move all four at once — where a written
    /// table can only ever be wrong about the row a reader is looking at.
    pub const fn p1_punch(self) -> [Key; 3] {
        match self {
            Preset::AzertyPunchLow => [Key::K, Key::L, Key::M],
            Preset::AzertyCabinet => [Key::I, Key::O, Key::P],
            Preset::QwertyPunchLow => [Key::J, Key::K, Key::L],
            Preset::QwertyCabinet => [Key::I, Key::O, Key::P],
        }
    }

    /// The three keys this preset puts P1's kicks on, short first.
    pub const fn p1_kick(self) -> [Key; 3] {
        match self {
            Preset::AzertyPunchLow => [Key::I, Key::O, Key::P],
            Preset::AzertyCabinet => [Key::K, Key::L, Key::M],
            Preset::QwertyPunchLow => [Key::I, Key::O, Key::P],
            Preset::QwertyCabinet => [Key::J, Key::K, Key::L],
        }
    }

    /// P2's punch row on the keypad, jab first.
    pub const fn p2_punch(self) -> [Key; 3] {
        match self {
            Preset::AzertyPunchLow | Preset::QwertyPunchLow => {
                [Key::NumPad4, Key::NumPad5, Key::NumPad6]
            }
            Preset::AzertyCabinet | Preset::QwertyCabinet => {
                [Key::NumPad7, Key::NumPad8, Key::NumPad9]
            }
        }
    }

    /// P2's kick row on the keypad, short first.
    pub const fn p2_kick(self) -> [Key; 3] {
        match self {
            Preset::AzertyPunchLow | Preset::QwertyPunchLow => {
                [Key::NumPad7, Key::NumPad8, Key::NumPad9]
            }
            Preset::AzertyCabinet | Preset::QwertyCabinet => {
                [Key::NumPad4, Key::NumPad5, Key::NumPad6]
            }
        }
    }

    /// The tag written to disk, and read back.
    ///
    /// A string and not the discriminant: a numbering is invisible in a config file and
    /// silently renumbers if a preset is ever inserted rather than appended, which would
    /// change what a saved file means without changing the file.
    pub const fn tag(self) -> &'static str {
        match self {
            Preset::AzertyPunchLow => "azerty-punch-low",
            Preset::AzertyCabinet => "azerty-cabinet",
            Preset::QwertyPunchLow => "qwerty-punch-low",
            Preset::QwertyCabinet => "qwerty-cabinet",
        }
    }

    /// The preset a tag names, or `None` — an unknown tag is not an error to the caller,
    /// which falls back to the default the way a missing save state does.
    pub fn from_tag(s: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.tag() == s.trim())
    }
}

/// Which keys are held.
///
/// A bitmask rather than a `Vec`, so [`Controls`] can keep last frame's set by
/// copy and the edge detection is one `&`.
///
/// `u64` and not `u32`: 46 keys hold bits 0-45. It was a `u32` through E2's 29 keys,
/// and the alternative to widening was overloading `PageUp`/`PageDown`/`Home` to
/// mean something else while the graphics viewer is up — which would have reached 31
/// keys, leaving exactly one free bit, and `scripts/mutate.py`'s control mutant needs
/// a free bit to move `Escape` to. Mapping player 2 then added ten more, which a `u32`
/// could not have held at all: 46 keys is 14 bits past its width.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeySet {
    bits: u64,
}

impl KeySet {
    /// Nothing held.
    pub const fn new() -> Self {
        Self { bits: 0 }
    }

    /// Marks `k` held.
    pub fn press(&mut self, k: Key) {
        self.bits |= 1u64 << k.bit();
    }

    /// Whether `k` is held.
    pub fn contains(&self, k: Key) -> bool {
        self.bits & (1u64 << k.bit()) != 0
    }

    /// A set of exactly these keys.
    pub fn from_keys(keys: &[Key]) -> Self {
        let mut s = Self::new();
        for &k in keys {
            s.press(k);
        }
        s
    }
}

/// What the loop should do this frame.
///
/// No `PartialEq`: [`machine::Inputs`] has none, and nothing compares two of
/// these. `Default` works because `Inputs`' hand-written one is `Inputs::idle()`
/// rather than a derived all-zero — which would read as every DIP switch on.
#[derive(Debug, Clone, Copy, Default)]
pub struct Actions {
    /// The board's inputs, level-triggered.
    pub inputs: Inputs,
    /// Pause was pressed this frame.
    pub pause_toggled: bool,
    /// Step one frame.
    pub step: bool,
    /// Reset the machine.
    pub reset: bool,
    /// Write a save state.
    pub save: bool,
    /// Read a save state.
    pub load: bool,
    /// Write a screenshot.
    pub screenshot: bool,
    /// Close the window.
    pub quit: bool,
    /// Show or hide the debugger overlay.
    pub overlay_toggled: bool,
    /// Step one instruction.
    ///
    /// Distinct from [`Self::step`], which is one *frame*. Both are needed and they
    /// are not interchangeable: a frame is 167,680 cycles, which is where a bug is,
    /// while an instruction is where you can see it.
    pub step_instruction: bool,
    /// Move the scroll focus to the other panel.
    pub focus_cycled: bool,
    /// Set or clear a breakpoint at the instruction about to execute.
    pub breakpoint_toggled: bool,
    /// Scroll the focused panel back.
    pub scroll_up: bool,
    /// Scroll the focused panel forward.
    pub scroll_down: bool,
    /// Return the focused panel to following the machine.
    pub follow_reset: bool,
    /// Show or hide the graphics viewer.
    pub gfx_toggled: bool,
    /// Cycle to the next graphics view.
    pub gfx_view_cycled: bool,
    /// Move back within the graphics view.
    pub gfx_back: bool,
    /// Move forward within the graphics view.
    pub gfx_forward: bool,
    /// Act on the current graphics view — cycle its tile kind or layer, or toggle.
    pub gfx_act: bool,
    /// Open or close the key menu.
    pub menu_toggled: bool,
    /// Move the menu's selection towards the top.
    ///
    /// Only ever set while the menu is open, where it comes from the same `Up` that is
    /// P2's stick the rest of the time. That overload is the whole reason the menu
    /// captures the keyboard: two meanings for one key are safe exactly when only one of
    /// them can be live at a time.
    pub menu_up: bool,
    /// Move the menu's selection towards the bottom.
    pub menu_down: bool,
    /// Apply the selected row.
    pub menu_apply: bool,
    /// Close the menu without applying.
    ///
    /// From `Escape`, which **quits** when the menu is shut. Keeping the two apart is the
    /// single most load-bearing line in the capture: the instinctive way to back out of a
    /// menu would otherwise end the session.
    pub menu_close: bool,
}

/// The keyboard, frame to frame.
#[derive(Debug, Clone, Default)]
pub struct Controls {
    /// Last frame's held keys, for the edge detection.
    was: KeySet,
    /// The board's DIP switches, which no key moves.
    ///
    /// Held here because [`Controls::update`] returns a whole [`Inputs`] and the play
    /// loop assigns it over the machine's own — so a setting made once at boot would
    /// be overwritten on the very next frame. The switches are cabinet
    /// configuration rather than controls, so they live beside the key state and are
    /// copied into every frame's `Inputs`.
    dsw: Option<[u8; 3]>,
    /// Which arrangement of the twelve player buttons is in force.
    ///
    /// Beside `dsw` and for the same reason: `update` returns a whole `Inputs` that the
    /// play loop assigns over the machine's own, so anything that must outlive a single
    /// frame lives here rather than being passed in.
    preset: Preset,
    /// Whether the key menu is open, in which case the board reads idle.
    ///
    /// Held here rather than taken as an argument because it gates the whole game-input
    /// half of [`Controls::update`], and a caller that forgot to pass it would get a menu
    /// you can play the game through.
    menu_open: bool,
}

impl Controls {
    /// A keyboard with nothing held.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the board's DIP switches for every frame from here on.
    ///
    /// Without this, [`Controls::update`] builds from [`Inputs::idle`], whose
    /// all-ones `dsw` means every switch *off* — including Demo Sounds, whose off
    /// state is silence. See [`Inputs::sf2_factory`] for the measured figures.
    pub fn set_dsw(&mut self, dsw: [u8; 3]) {
        self.dsw = Some(dsw);
    }

    /// Which button arrangement is in force.
    pub fn preset(&self) -> Preset {
        self.preset
    }

    /// Switches the twelve player buttons to another arrangement.
    ///
    /// Takes effect on the next [`Controls::update`]. Nothing is retained from the old
    /// preset: the keys held when this is called are read against the new map, which is
    /// correct — a key that was a punch and is now a kick should be whatever it is now.
    pub fn set_preset(&mut self, preset: Preset) {
        self.preset = preset;
    }

    /// Tells the map whether the key menu is up.
    ///
    /// While it is, [`Controls::update`] reports an **idle board**: navigating a menu must
    /// not throw punches. This matters more than it looks, because `Inputs` is
    /// level-triggered — without the explicit idle, a stick held at the moment the menu
    /// opened would stay held in the board's eyes for as long as the menu stayed up.
    ///
    /// The controls are gated too, and that is the other half of the capture: `Escape`
    /// would otherwise **quit** and `Enter` would act on the graphics view, so the two
    /// keys a menu needs most are the two that must not reach their old meanings. The
    /// exception is `Tab`, which is what closes the menu again.
    pub fn set_menu_open(&mut self, open: bool) {
        self.menu_open = open;
    }

    /// Reads this frame's held keys.
    pub fn update(&mut self, now: KeySet) -> Actions {
        // Pressed *this* frame: held now and not held before. Per key, so two
        // controls pressed on the same frame both fire and releasing one does not
        // re-arm another.
        let edge = |k: Key| now.contains(k) && !self.was.contains(k);

        let mut inputs = Inputs::idle();
        if let Some(dsw) = self.dsw {
            inputs.dsw = dsw;
        }
        // The whole board half is skipped while the menu is up, leaving `Inputs::idle`.
        // Not "the buttons are ignored" -- *idle*, which is a different claim: a stick
        // held when the menu opened must stop being held, and `Inputs` is level-triggered,
        // so only writing the idle value achieves that.
        if !self.menu_open {
            let p = self.preset;
            // Player 1, the left of the keyboard. The stick is the same four physical keys
            // on every preset -- AZERTY's ZSQD and QWERTY's WASD are one map, and only the
            // printed letters differ -- so it is written here rather than in `Preset`.
            inputs.p1.up = now.contains(Key::Z);
            inputs.p1.down = now.contains(Key::S);
            inputs.p1.left = now.contains(Key::Q);
            inputs.p1.right = now.contains(Key::D);
            // The six buttons are the preset's to name. `map` over the triple rather than
            // three indexed reads: `[0]`, `[1]`, `[2]` written out is where a
            // copy-paste puts the jab's key under the strong's finger.
            inputs.p1.punch = p.p1_punch().map(|k| now.contains(k));
            inputs.p1.kick = p.p1_kick().map(|k| now.contains(k));
            // Player 2, the right: the arrow keys and the keypad, same shape.
            inputs.p2.up = now.contains(Key::Up);
            inputs.p2.down = now.contains(Key::Down);
            inputs.p2.left = now.contains(Key::Left);
            inputs.p2.right = now.contains(Key::Right);
            inputs.p2.punch = p.p2_punch().map(|k| now.contains(k));
            inputs.p2.kick = p.p2_kick().map(|k| now.contains(k));
            inputs.coin1 = now.contains(Key::Num5);
            inputs.coin2 = now.contains(Key::Num6);
            inputs.start1 = now.contains(Key::Num1);
            inputs.start2 = now.contains(Key::Num2);
            // Level-triggered, unlike every other function key: the service menu is
            // entered by *holding* the test switch, which is what the switch does on a
            // real cabinet.
            inputs.test = now.contains(Key::F2);
        }

        // While the menu is up it owns the keyboard, and `open` gates every control that
        // would otherwise fire underneath it. Three of them are the reason this exists:
        // `Escape` quits, `Enter` acts on the graphics view, and the arrows are P2's
        // stick. All twelve of `F1`-`F12` are mapped, so there was no free key to give the
        // menu instead -- capturing is not a shortcut here, it is the only option that
        // does not overload a key with two live meanings.
        //
        // `Tab` itself is *not* gated: it is what closes the menu again.
        let open = self.menu_open;
        let ctl = |k: Key| !open && edge(k);

        let actions = Actions {
            inputs,
            pause_toggled: ctl(Key::F11),
            step: ctl(Key::Period),
            reset: ctl(Key::F3),
            save: ctl(Key::F5),
            load: ctl(Key::F8),
            screenshot: ctl(Key::F12),
            // Gated like the rest, and this is the one that would cost a session: with the
            // menu open, `Escape` closes it and quits nothing.
            quit: ctl(Key::Escape),
            overlay_toggled: ctl(Key::F1),
            step_instruction: ctl(Key::F4),
            focus_cycled: ctl(Key::F6),
            breakpoint_toggled: ctl(Key::F7),
            // The scroll keys are edge-triggered like the rest, not repeating. A held
            // `PageDown` walking sixty pages a second is not a usable way to find an
            // address, and auto-repeat would need a timer — which would put a clock in
            // the one crate that deliberately has none.
            scroll_up: ctl(Key::PageUp),
            scroll_down: ctl(Key::PageDown),
            follow_reset: ctl(Key::Home),
            // Edge-triggered, every one, for the reason written just above: a held
            // `]` walking sixty pages a second is not a way to find a tile.
            gfx_toggled: ctl(Key::GfxToggled),
            gfx_view_cycled: ctl(Key::GfxView),
            gfx_back: ctl(Key::BracketLeft),
            gfx_forward: ctl(Key::BracketRight),
            // The other half of the `Enter` collision: with the menu open this is false and
            // `menu_apply` is true, so one keypress never means two things at once.
            gfx_act: ctl(Key::Enter),
            // Never gated -- this is the key that closes the menu again, and gating it
            // would make the menu impossible to leave except by `Escape`.
            menu_toggled: edge(Key::Tab),
            // The mirror image of `ctl`: live only while the menu is open. `Up` and `Down`
            // are P2's stick the rest of the time, and `Enter` and `Escape` are the two
            // collisions above. Two meanings for one key are safe exactly when only one of
            // them can be live at a time, which is what this pair of gates guarantees.
            menu_up: open && edge(Key::Up),
            menu_down: open && edge(Key::Down),
            menu_apply: open && edge(Key::Enter),
            menu_close: open && edge(Key::Escape),
        };
        self.was = now;
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every game key, and the three ports it must produce: `(key, in0, in1, in2)`.
    ///
    /// Shared by the two tests that need it — the one that presses each key and the one
    /// that checks the table covers every game key. A local array in the first would
    /// leave the second unable to see it, which is exactly the bug the second test's doc
    /// records: it asserted a count derived from `Key::ALL` and a deleted row went
    /// unnoticed.
    ///
    /// The **default** preset only. [`Key::J`] is deliberately absent: it presses nothing
    /// under an AZERTY preset, and [`PRESET_BUTTON_PORTS`] is where every preset's twelve
    /// buttons are pinned.
    ///
    /// The values are literals from `machine::inputs`' documented layout, computed by
    /// hand: `IN1` bits 0-3 stick and 4-6 punches, P1 in the low byte and P2 in the
    /// high; `IN2` bits 0-2 and 4-6 for the kicks; `IN0` for coins, starts and test.
    /// A loop deriving them from the same fields the map sets would pass with every key
    /// on the wrong button.
    const GAME_KEY_PORTS: [(Key, u8, u16, u8, &str); 25] = [
        // P1's stick, IN1 bits 0-3. ZSQD: Z above S, Q and D either side.
        (Key::Z, 0xFF, 0xFFF7, 0xFF, "P1 up"),
        (Key::S, 0xFF, 0xFFFB, 0xFF, "P1 down"),
        (Key::Q, 0xFF, 0xFFFD, 0xFF, "P1 left"),
        (Key::D, 0xFF, 0xFFFE, 0xFF, "P1 right"),
        // P1's punches, IN1 bits 4-6 — KLM, the home row, left to right.
        (Key::K, 0xFF, 0xFFEF, 0xFF, "P1 jab"),
        (Key::L, 0xFF, 0xFFDF, 0xFF, "P1 strong"),
        (Key::M, 0xFF, 0xFFBF, 0xFF, "P1 fierce"),
        // P1's kicks, IN2 bits 0-2 — IOP, the row *above* the punches. Note IN1 stays
        // 0xFFFF: a kick is not a punch, and the two are read through different chips.
        (Key::I, 0xFF, 0xFFFF, 0xFE, "P1 short"),
        (Key::O, 0xFF, 0xFFFF, 0xFD, "P1 forward"),
        (Key::P, 0xFF, 0xFFFF, 0xFB, "P1 roundhouse"),
        // P2's stick, IN1's *high* byte — the same four bits, eight up.
        (Key::Up, 0xFF, 0xF7FF, 0xFF, "P2 up"),
        (Key::Down, 0xFF, 0xFBFF, 0xFF, "P2 down"),
        (Key::Left, 0xFF, 0xFDFF, 0xFF, "P2 left"),
        (Key::Right, 0xFF, 0xFEFF, 0xFF, "P2 right"),
        // P2's punches, IN1 bits 12-14 — the keypad's *bottom* row, 456, mirroring P1's
        // punches-under-kicks.
        (Key::NumPad4, 0xFF, 0xEFFF, 0xFF, "P2 jab"),
        (Key::NumPad5, 0xFF, 0xDFFF, 0xFF, "P2 strong"),
        (Key::NumPad6, 0xFF, 0xBFFF, 0xFF, "P2 fierce"),
        // P2's kicks, IN2 bits 4-6 — the keypad's 789, above. Bit 3 is unwired, which is
        // why they do not start at 3.
        (Key::NumPad7, 0xFF, 0xFFFF, 0xEF, "P2 short"),
        (Key::NumPad8, 0xFF, 0xFFFF, 0xDF, "P2 forward"),
        (Key::NumPad9, 0xFF, 0xFFFF, 0xBF, "P2 roundhouse"),
        // Coins and starts, IN0. MAME's convention: 5 and 6 coin, 1 and 2 start.
        (Key::Num5, 0xFE, 0xFFFF, 0xFF, "coin 1"),
        (Key::Num6, 0xFD, 0xFFFF, 0xFF, "coin 2"),
        (Key::Num1, 0xEF, 0xFFFF, 0xFF, "start 1"),
        (Key::Num2, 0xDF, 0xFFFF, 0xFF, "start 2"),
        (Key::F2, 0xBF, 0xFFFF, 0xFF, "the test switch, IN0 bit 6"),
    ];

    /// The twelve player buttons under **every** preset: `(preset, key, in1, in2, what)`.
    ///
    /// [`GAME_KEY_PORTS`] covers the default preset only, which is not enough now that a
    /// preset changes *which* port a button reaches — and, for two keys, whether it
    /// reaches one at all. Forty-eight rows rather than a loop over
    /// `Preset::p1_punch()`: a loop would read the same four tables the map reads, so
    /// every preset could have its punches and kicks transposed and it would still pass.
    ///
    /// `in0` is not a column because no preset touches a coin, a start or the test
    /// switch — the presets cover the twelve player buttons and nothing else. That is
    /// asserted separately, by `no_preset_moves_a_coin_a_start_or_the_test_switch`.
    #[allow(clippy::type_complexity)]
    const PRESET_BUTTON_PORTS: [(Preset, Key, u16, u8, &str); 48] = [
        // AZERTY, punches low — the default. Punches KLM on IN1 4-6, kicks IOP on IN2 0-2.
        (Preset::AzertyPunchLow, Key::K, 0xFFEF, 0xFF, "P1 jab"),
        (Preset::AzertyPunchLow, Key::L, 0xFFDF, 0xFF, "P1 strong"),
        (Preset::AzertyPunchLow, Key::M, 0xFFBF, 0xFF, "P1 fierce"),
        (Preset::AzertyPunchLow, Key::I, 0xFFFF, 0xFE, "P1 short"),
        (Preset::AzertyPunchLow, Key::O, 0xFFFF, 0xFD, "P1 forward"),
        (
            Preset::AzertyPunchLow,
            Key::P,
            0xFFFF,
            0xFB,
            "P1 roundhouse",
        ),
        (Preset::AzertyPunchLow, Key::NumPad4, 0xEFFF, 0xFF, "P2 jab"),
        (
            Preset::AzertyPunchLow,
            Key::NumPad5,
            0xDFFF,
            0xFF,
            "P2 strong",
        ),
        (
            Preset::AzertyPunchLow,
            Key::NumPad6,
            0xBFFF,
            0xFF,
            "P2 fierce",
        ),
        (
            Preset::AzertyPunchLow,
            Key::NumPad7,
            0xFFFF,
            0xEF,
            "P2 short",
        ),
        (
            Preset::AzertyPunchLow,
            Key::NumPad8,
            0xFFFF,
            0xDF,
            "P2 forward",
        ),
        (
            Preset::AzertyPunchLow,
            Key::NumPad9,
            0xFFFF,
            0xBF,
            "P2 roundhouse",
        ),
        // AZERTY, a cabinet's order — the same six keys, the two rows traded.
        (Preset::AzertyCabinet, Key::I, 0xFFEF, 0xFF, "P1 jab"),
        (Preset::AzertyCabinet, Key::O, 0xFFDF, 0xFF, "P1 strong"),
        (Preset::AzertyCabinet, Key::P, 0xFFBF, 0xFF, "P1 fierce"),
        (Preset::AzertyCabinet, Key::K, 0xFFFF, 0xFE, "P1 short"),
        (Preset::AzertyCabinet, Key::L, 0xFFFF, 0xFD, "P1 forward"),
        (Preset::AzertyCabinet, Key::M, 0xFFFF, 0xFB, "P1 roundhouse"),
        (Preset::AzertyCabinet, Key::NumPad7, 0xEFFF, 0xFF, "P2 jab"),
        (
            Preset::AzertyCabinet,
            Key::NumPad8,
            0xDFFF,
            0xFF,
            "P2 strong",
        ),
        (
            Preset::AzertyCabinet,
            Key::NumPad9,
            0xBFFF,
            0xFF,
            "P2 fierce",
        ),
        (
            Preset::AzertyCabinet,
            Key::NumPad4,
            0xFFFF,
            0xEF,
            "P2 short",
        ),
        (
            Preset::AzertyCabinet,
            Key::NumPad5,
            0xFFFF,
            0xDF,
            "P2 forward",
        ),
        (
            Preset::AzertyCabinet,
            Key::NumPad6,
            0xFFFF,
            0xBF,
            "P2 roundhouse",
        ),
        // QWERTY, punches low — `J K L`, the *US* home-row run, in place of `K L M`.
        (Preset::QwertyPunchLow, Key::J, 0xFFEF, 0xFF, "P1 jab"),
        (Preset::QwertyPunchLow, Key::K, 0xFFDF, 0xFF, "P1 strong"),
        (Preset::QwertyPunchLow, Key::L, 0xFFBF, 0xFF, "P1 fierce"),
        (Preset::QwertyPunchLow, Key::I, 0xFFFF, 0xFE, "P1 short"),
        (Preset::QwertyPunchLow, Key::O, 0xFFFF, 0xFD, "P1 forward"),
        (
            Preset::QwertyPunchLow,
            Key::P,
            0xFFFF,
            0xFB,
            "P1 roundhouse",
        ),
        (Preset::QwertyPunchLow, Key::NumPad4, 0xEFFF, 0xFF, "P2 jab"),
        (
            Preset::QwertyPunchLow,
            Key::NumPad5,
            0xDFFF,
            0xFF,
            "P2 strong",
        ),
        (
            Preset::QwertyPunchLow,
            Key::NumPad6,
            0xBFFF,
            0xFF,
            "P2 fierce",
        ),
        (
            Preset::QwertyPunchLow,
            Key::NumPad7,
            0xFFFF,
            0xEF,
            "P2 short",
        ),
        (
            Preset::QwertyPunchLow,
            Key::NumPad8,
            0xFFFF,
            0xDF,
            "P2 forward",
        ),
        (
            Preset::QwertyPunchLow,
            Key::NumPad9,
            0xFFFF,
            0xBF,
            "P2 roundhouse",
        ),
        // QWERTY, a cabinet's order — what this project shipped on a US keyboard before
        // the remap.
        (Preset::QwertyCabinet, Key::I, 0xFFEF, 0xFF, "P1 jab"),
        (Preset::QwertyCabinet, Key::O, 0xFFDF, 0xFF, "P1 strong"),
        (Preset::QwertyCabinet, Key::P, 0xFFBF, 0xFF, "P1 fierce"),
        (Preset::QwertyCabinet, Key::J, 0xFFFF, 0xFE, "P1 short"),
        (Preset::QwertyCabinet, Key::K, 0xFFFF, 0xFD, "P1 forward"),
        (Preset::QwertyCabinet, Key::L, 0xFFFF, 0xFB, "P1 roundhouse"),
        (Preset::QwertyCabinet, Key::NumPad7, 0xEFFF, 0xFF, "P2 jab"),
        (
            Preset::QwertyCabinet,
            Key::NumPad8,
            0xDFFF,
            0xFF,
            "P2 strong",
        ),
        (
            Preset::QwertyCabinet,
            Key::NumPad9,
            0xBFFF,
            0xFF,
            "P2 fierce",
        ),
        (
            Preset::QwertyCabinet,
            Key::NumPad4,
            0xFFFF,
            0xEF,
            "P2 short",
        ),
        (
            Preset::QwertyCabinet,
            Key::NumPad5,
            0xFFFF,
            0xDF,
            "P2 forward",
        ),
        (
            Preset::QwertyCabinet,
            Key::NumPad6,
            0xFFFF,
            0xBF,
            "P2 roundhouse",
        ),
    ];

    /// Every key that reaches no board input under any preset.
    ///
    /// Shared by the coverage test and `no_control_key_reaches_the_board`, which would
    /// otherwise be two lists that have to be kept in step by hand — and were, until the
    /// menu added a twentieth control to one of them.
    const CONTROL_KEYS: [Key; 20] = [
        Key::F3,
        Key::F5,
        Key::F8,
        Key::F12,
        Key::F11,
        Key::Period,
        Key::Escape,
        Key::F1,
        Key::F4,
        Key::F6,
        Key::F7,
        Key::PageUp,
        Key::PageDown,
        Key::Home,
        Key::GfxToggled,
        Key::GfxView,
        Key::BracketLeft,
        Key::BracketRight,
        Key::Enter,
        Key::Tab,
    ];

    /// Nothing held is an idle board and no actions.
    #[test]
    fn nothing_held_is_an_idle_board() {
        let mut c = Controls::new();
        let a = c.update(KeySet::new());
        assert_eq!(a.inputs.in0(), 0xFF, "active low: nothing pressed");
        assert_eq!(a.inputs.in1(), 0xFFFF);
        assert_eq!(a.inputs.in2(), 0xFF);
        assert!(!a.pause_toggled && !a.step && !a.reset);
        assert!(!a.save && !a.load && !a.screenshot && !a.quit);
        assert!(!a.overlay_toggled && !a.step_instruction && !a.focus_cycled);
        assert!(!a.breakpoint_toggled && !a.scroll_up && !a.scroll_down);
        assert!(!a.follow_reset);
        assert!(!a.gfx_toggled && !a.gfx_view_cycled && !a.gfx_act);
        assert!(!a.gfx_back && !a.gfx_forward);
        assert!(!a.menu_toggled && !a.menu_up && !a.menu_down);
        assert!(!a.menu_apply && !a.menu_close);
    }

    /// Each game key clears exactly its own port bit, with the expected values as
    /// literals.
    ///
    /// The literals are the point: a map checked by reading the same `Inputs` field
    /// it sets would pass with every key on the wrong button. These values are
    /// `machine::inputs`' own documented bits — `IN1` bits 4-6 for the punches,
    /// `IN2` bits 0-2 for the kicks — computed by hand.
    ///
    /// This also pins the punch/kick split, which is the one part of the map that
    /// is not a free choice: SF2's kicks are read through CPS-B (`IN2`) and its
    /// punches through the port block (`IN1`), so a frontend that put all six on
    /// one port would leave three buttons dead in-game while every test that looked
    /// only at "some bit changed" stayed green.
    ///
    /// # All three ports, every key
    ///
    /// Each key asserts `in0`, `in1` and `in2` — not only the port it belongs to.
    /// That is what makes the two players' halves separable now that both are mapped:
    /// the old `no_key_presses_a_player_two_control` could state "no key reaches P2"
    /// as a blanket property, and cannot any more. A P1 key that also set the P2 field
    /// beside it — `p2.right` for `p1.right`, `scripts/mutate.py`'s
    /// `a-key-reaches-player-two` — changes `in1`'s *high* byte, which a
    /// `0xFF__`-shaped literal catches and a `_ & 0xFF` comparison would not.
    ///
    /// The table lives at module scope as [`GAME_KEY_PORTS`] so
    /// `the_port_bit_table_covers_every_game_key_and_only_those` can read it — see that
    /// test for why it has to.
    #[test]
    fn each_game_key_clears_its_own_port_bit() {
        let one = |k: Key| {
            let mut c = Controls::new();
            c.update(KeySet::from_keys(&[k])).inputs
        };
        let cases = GAME_KEY_PORTS;
        for (k, in0, in1, in2, what) in cases {
            let i = one(k);
            assert_eq!(i.in0(), in0, "{k:?} ({what}): IN0");
            assert_eq!(i.in1(), in1, "{k:?} ({what}): IN1");
            assert_eq!(i.in2(), in2, "{k:?} ({what}): IN2");
        }
        // No two rows claim the same key, and no two claim the same triple of ports —
        // a copy-paste that gave two keys one row's values would otherwise pass every
        // assertion above.
        for (i, a) in cases.iter().enumerate() {
            for b in &cases[i + 1..] {
                assert_ne!(a.0, b.0, "{:?} appears twice", a.0);
                assert_ne!(
                    (a.1, a.2, a.3),
                    (b.1, b.2, b.3),
                    "{:?} and {:?} press the same thing",
                    a.0,
                    b.0
                );
            }
        }
    }

    /// `GAME_KEY_PORTS` covers every game key, and nothing else.
    ///
    /// The table drives `each_game_key_clears_its_own_port_bit`, so a key missing from
    /// it is a key with no port assertion at all — and every assertion that *is* there
    /// still passes. This is the test that closes that: it reads the table's own keys
    /// and compares them against `Key::ALL` minus the controls, in both directions.
    ///
    /// Probed, because the first version of this test did not work. It derived the game
    /// keys from `Key::ALL` and asserted the *count* was 25, never reading the table —
    /// so deleting `NumPad6`'s row and changing the length annotation to 24 left P2's
    /// roundhouse kick untested with all 17 `keys` tests green. Comparing the two sets
    /// is what makes the claim in the name true.
    ///
    /// # Two tables, because liveness is preset-dependent
    ///
    /// A key is now covered if it appears in `GAME_KEY_PORTS` *or* in
    /// [`PRESET_BUTTON_PORTS`]. `Key::J` is only in the second, because it presses nothing
    /// under the default preset — and "presses nothing" is precisely the claim a missing
    /// row makes silently. Requiring the union closes that: a key in neither table has no
    /// port assertion under any preset.
    #[test]
    fn the_port_bit_table_covers_every_game_key_and_only_those() {
        let control = CONTROL_KEYS;
        let tabled: Vec<Key> = GAME_KEY_PORTS.iter().map(|r| r.0).collect();
        let preset_tabled: Vec<Key> = PRESET_BUTTON_PORTS.iter().map(|r| r.1).collect();
        // Every key that is not a control has a row in one table or the other.
        for k in Key::ALL {
            if control.contains(&k) {
                assert!(
                    !tabled.contains(&k) && !preset_tabled.contains(&k),
                    "{k:?} is a control and must not have a port row"
                );
            } else {
                assert!(
                    tabled.contains(&k) || preset_tabled.contains(&k),
                    "{k:?} is a game key with no row in GAME_KEY_PORTS or \
                     PRESET_BUTTON_PORTS, so no port assertion covers it"
                );
            }
        }
        // And the counts, which catch a row for a key that is not in `Key::ALL` at all.
        // `J` is the one game key outside `GAME_KEY_PORTS`, hence the `- 1`.
        assert_eq!(
            tabled.len(),
            Key::ALL.len() - control.len() - 1,
            "the table has {} rows for {} game keys",
            tabled.len(),
            Key::ALL.len() - control.len()
        );
        assert_eq!(
            tabled.len(),
            25,
            "25 game keys under the default preset: 20 player inputs, 4 coin and \
                    start buttons, and the test switch"
        );
        assert_eq!(
            Key::ALL.len() - control.len(),
            26,
            "26 game keys across all presets: the 25 above plus J, which is live only \
             under a QWERTY preset"
        );
    }

    /// No key moves a DIP switch, and with none set they stay at `idle()`'s value.
    ///
    /// `Inputs::idle()` sets them to 0xFF and no key path touches them — but building
    /// the struct field by field is exactly where an `Inputs::default()` swapped for a
    /// derived one would turn every switch on, and the board would then boot in a
    /// different configuration with no key involved.
    #[test]
    fn the_dip_switches_are_never_touched() {
        for k in Key::ALL {
            let mut c = Controls::new();
            let a = c.update(KeySet::from_keys(&[k]));
            assert_eq!(a.inputs.dsw, [0xFF; 3], "{k:?} moved a DIP switch");
        }
    }

    /// Switches set once survive every later frame, and no key disturbs them.
    ///
    /// This is the property the play loop needs and the reason `dsw` is held on
    /// `Controls` at all. `update` returns a whole `Inputs` built from
    /// `Inputs::idle()`, and the loop assigns it over the board's own — so a machine
    /// configured at construction is back to all-switches-off one frame later. That
    /// bug is silent: the board still boots, still plays, and is merely mute, on the
    /// easiest difficulty.
    ///
    /// The multi-frame loop is the point. A single `update` would pass with an
    /// implementation that applied the switches once and then forgot them.
    #[test]
    fn dip_switches_set_once_persist_across_frames() {
        let mut c = Controls::new();
        c.set_dsw([0xFF, 0xFC, 0x9F]);
        for frame in 0..4 {
            // A different key each frame, including none, so the carry-through is not
            // resting on an idle keyboard.
            let held = match frame {
                0 => vec![],
                1 => vec![Key::S, Key::K],
                2 => vec![Key::Num5],
                _ => vec![Key::M, Key::Right],
            };
            let a = c.update(KeySet::from_keys(&held));
            assert_eq!(
                a.inputs.dsw,
                [0xFF, 0xFC, 0x9F],
                "frame {frame} lost the DIP switches"
            );
            // Demo Sounds specifically, named rather than left inside the array
            // comparison: this is the bit whose loss is audible and nothing else.
            assert_eq!(a.inputs.dsw[2] & 0x20, 0x00, "frame {frame}: demo sounds");
        }
    }

    /// `set_dsw` changes the switches and no control.
    #[test]
    fn set_dsw_presses_nothing() {
        let mut c = Controls::new();
        c.set_dsw([0x00, 0x00, 0x00]);
        let a = c.update(KeySet::from_keys(&[]));
        assert_eq!(a.inputs.dsw, [0x00; 3], "the switches went through");
        assert_eq!(a.inputs.in0(), 0xFF, "no coin, start, service or test");
        assert_eq!(a.inputs.in1(), 0xFFFF, "no stick or punch");
        assert_eq!(a.inputs.in2(), 0xFF, "no kick");
    }

    /// Two keys at once clear two bits.
    ///
    /// Every case above holds one key, so all of them would pass with an
    /// implementation that overwrote `inputs` instead of accumulating into it —
    /// and holding down-and-punch is the first thing anyone does in a fighting
    /// game.
    #[test]
    fn several_keys_at_once_all_reach_the_board() {
        let mut c = Controls::new();
        let a = c.update(KeySet::from_keys(&[Key::S, Key::K, Key::I]));
        assert_eq!(a.inputs.in1(), 0xFFEB, "down (bit 2) and jab (bit 4)");
        assert_eq!(a.inputs.in2(), 0xFE, "and the kick, on its own port");
    }

    /// Both players at once, on the same frame.
    ///
    /// The case the whole remap exists for, and one no single-key row can make: two
    /// people playing. P1 holding down-back and a jab while P2 holds up-forward and a
    /// roundhouse kick puts eight bits across three ports at once, and the values are
    /// literals computed from `machine::inputs`' documented layout — P1 in `IN1`'s low
    /// byte, P2 in its high, the kicks on `IN2` at bits 0-2 and 4-6.
    ///
    /// A map that dropped one player while the other was active — the natural mistake
    /// being an `inputs.p2 = ...` assignment that overwrote rather than accumulated —
    /// passes every one-key row above and fails here.
    #[test]
    fn both_players_at_once_reach_their_own_halves() {
        let mut c = Controls::new();
        let a = c.update(KeySet::from_keys(&[
            // P1: down (S) and left (Q), jab (K), roundhouse (P).
            Key::S,
            Key::Q,
            Key::K,
            Key::P,
            // P2: up (Up) and right (Right), fierce (NumPad6), short (NumPad7).
            Key::Up,
            Key::Right,
            Key::NumPad6,
            Key::NumPad7,
        ]));
        // IN1 low byte: down is bit 2, left is bit 1, jab is bit 4 → 0xFF & !0x16 = 0xE9.
        // IN1 high byte: up is bit 3, right is bit 0, fierce is bit 6 → !0x49 = 0xB6.
        assert_eq!(a.inputs.in1(), 0xB6E9, "P2 in the high byte, P1 in the low");
        // IN2: P1's roundhouse is bit 2, P2's short is bit 4 → !0x14 = 0xEB.
        assert_eq!(a.inputs.in2(), 0xEB, "both players' kicks, one port");
        assert_eq!(a.inputs.in0(), 0xFF, "and no coin or start was involved");
    }

    /// No control key touches the board.
    ///
    /// This replaces `no_key_presses_a_player_two_control`, which asserted that P2 was
    /// idle no matter which key was held. That was true while P2 was unmapped and is
    /// now false by design, so the property had to be re-stated rather than loosened:
    /// what still holds in one blanket sweep is that a **control** — pause, step, save,
    /// the debugger's seven, the graphics viewer's five — reaches no board input at
    /// all. That is the direction with a real failure mode, because a control that also
    /// pressed a button would be invisible: F5 saves *and* throws a fierce punch, and
    /// the save still works.
    ///
    /// The other direction — a game key that reaches the wrong player — is
    /// `each_game_key_clears_its_own_port_bit`, which asserts all three ports per key.
    #[test]
    fn no_control_key_reaches_the_board() {
        // Every key that is not a game input. `F2` is not here: the test switch is a
        // board input, level-triggered, and `the_test_switch_is_held_not_pressed` owns
        // it.
        assert_eq!(
            CONTROL_KEYS.len(),
            20,
            "add a new control to CONTROL_KEYS too"
        );
        // Under **every** preset, not only the default: a preset that put a punch on
        // `Tab` or `Enter` would make the menu unreachable, and the row order alone
        // cannot show that.
        for p in Preset::ALL {
            for k in CONTROL_KEYS {
                let mut c = Controls::new();
                c.set_preset(p);
                let i = c.update(KeySet::from_keys(&[k])).inputs;
                assert_eq!(
                    i.in0(),
                    0xFF,
                    "{k:?} reached a coin, start, service or test under {p:?}"
                );
                assert_eq!(
                    i.in1(),
                    0xFFFF,
                    "{k:?} reached a stick or a punch under {p:?}"
                );
                assert_eq!(i.in2(), 0xFF, "{k:?} reached a kick under {p:?}");
            }
        }
    }

    /// The control keys are edge-triggered: held down, they act once.
    ///
    /// This is the whole substance of this module. A held `.` must not step sixty
    /// frames a second and a held F5 must not write sixty save states — while a
    /// held direction must absolutely keep pressing, because holding down is how
    /// you crouch. The asymmetry is deliberate and this test states both halves.
    #[test]
    fn control_keys_fire_once_per_press_and_game_keys_do_not() {
        let mut c = Controls::new();
        let held = KeySet::from_keys(&[Key::Period, Key::S]);

        let a = c.update(held);
        assert!(a.step, "the first frame of the press steps");
        assert_eq!(a.inputs.in1(), 0xFFFB, "and down is pressed");

        let a = c.update(held);
        assert!(!a.step, "the second frame does not step again");
        assert_eq!(a.inputs.in1(), 0xFFFB, "but down is still pressed");

        let a = c.update(held);
        assert!(!a.step, "nor the third");

        // Release and press again: a second step.
        c.update(KeySet::new());
        let a = c.update(held);
        assert!(a.step, "a fresh press steps again");
    }

    /// Every control key is edge-triggered, not just the one above.
    ///
    /// Checked as a table over all twenty, because the natural implementation is one
    /// `edge` helper per action and the natural mistake is to forget it on one of
    /// them — which then works exactly once out of twenty, in whichever action the
    /// author tested by hand.
    ///
    /// The debugger's seven and the graphics viewer's five are in the same table as
    /// the original seven rather than tables of their own: they are the same kind of
    /// thing, and a separate table is a second place to forget to add a row.
    ///
    /// The menu's four navigation actions are *not* here, and cannot be: they only fire
    /// while the menu is open, which `Controls::new()` is not.
    /// `the_menus_navigation_is_edge_triggered_too` runs this same four-frame press
    /// against an open menu.
    #[test]
    fn every_control_action_is_edge_triggered() {
        /// Reads one action's flag. Named because clippy calls the inline array
        /// type too complex, and it is: a table of key-and-accessor pairs.
        type Reader = fn(&Actions) -> bool;
        let cases: [(Key, Reader); 20] = [
            (Key::F11, |a| a.pause_toggled),
            (Key::Period, |a| a.step),
            (Key::F3, |a| a.reset),
            (Key::F5, |a| a.save),
            (Key::F8, |a| a.load),
            (Key::F12, |a| a.screenshot),
            (Key::Escape, |a| a.quit),
            (Key::F1, |a| a.overlay_toggled),
            (Key::F4, |a| a.step_instruction),
            (Key::F6, |a| a.focus_cycled),
            (Key::F7, |a| a.breakpoint_toggled),
            (Key::PageUp, |a| a.scroll_up),
            (Key::PageDown, |a| a.scroll_down),
            (Key::Home, |a| a.follow_reset),
            (Key::GfxToggled, |a| a.gfx_toggled),
            (Key::GfxView, |a| a.gfx_view_cycled),
            (Key::BracketLeft, |a| a.gfx_back),
            (Key::BracketRight, |a| a.gfx_forward),
            (Key::Enter, |a| a.gfx_act),
            (Key::Tab, |a| a.menu_toggled),
        ];
        // Every key that is not a game input and not the test switch must be in the
        // table. Without this, adding a key and forgetting the row leaves the new
        // action untested and every assertion below still passes.
        let game = [
            Key::Z,
            Key::S,
            Key::Q,
            Key::D,
            Key::I,
            Key::O,
            Key::P,
            Key::J,
            Key::K,
            Key::L,
            Key::M,
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::NumPad7,
            Key::NumPad8,
            Key::NumPad9,
            Key::NumPad4,
            Key::NumPad5,
            Key::NumPad6,
            Key::Num1,
            Key::Num2,
            Key::Num5,
            Key::Num6,
            Key::F2,
        ];
        for k in Key::ALL {
            assert!(
                game.contains(&k) || cases.iter().any(|&(c, _)| c == k),
                "{k:?} is neither a game input nor in the edge-trigger table"
            );
        }
        for (k, get) in cases {
            let mut c = Controls::new();
            let held = KeySet::from_keys(&[k]);
            assert!(get(&c.update(held)), "{k:?} must fire on the press");
            assert!(!get(&c.update(held)), "{k:?} must not fire while held");
            assert!(!get(&c.update(held)), "{k:?} still held");
            c.update(KeySet::new());
            assert!(
                get(&c.update(held)),
                "{k:?} must fire again after a release"
            );
        }
    }

    /// The test switch is the one function key that is *not* edge-triggered.
    ///
    /// A real cabinet's service switch is a switch: the menu is entered by holding
    /// it while the board boots. An edge-triggered F2 would present the board with
    /// a one-frame pulse, which the boot code would almost certainly miss — and the
    /// failure would look like "the service menu does not work" rather than like a
    /// key-map bug.
    #[test]
    fn the_test_switch_is_held_not_pressed() {
        let mut c = Controls::new();
        let held = KeySet::from_keys(&[Key::F2]);
        assert_eq!(
            c.update(held).inputs.in0(),
            0xBF,
            "pressed on the first frame"
        );
        assert_eq!(
            c.update(held).inputs.in0(),
            0xBF,
            "and still held on the next"
        );
        assert_eq!(c.update(KeySet::new()).inputs.in0(), 0xFF, "and released");
    }

    /// Two control keys pressed on the same frame both fire.
    ///
    /// The edge tracking is per key, not one "something changed" flag.
    #[test]
    fn two_control_keys_pressed_together_both_fire() {
        let mut c = Controls::new();
        let a = c.update(KeySet::from_keys(&[Key::F11, Key::F5]));
        assert!(a.pause_toggled && a.save);
    }

    /// Releasing one control key does not re-arm another.
    ///
    /// With a single "previous frame" set, releasing P while F5 stays held must not
    /// make F5 fire a second time. The natural bug is comparing set *sizes* or
    /// checking "any key released".
    #[test]
    fn releasing_one_key_does_not_refire_another() {
        let mut c = Controls::new();
        let a = c.update(KeySet::from_keys(&[Key::F11, Key::F5]));
        assert!(a.pause_toggled && a.save, "the premise");
        let a = c.update(KeySet::from_keys(&[Key::F5]));
        assert!(!a.save, "F5 was held throughout and must not fire again");
        assert!(!a.pause_toggled);
    }

    /// A `KeySet` holds each key independently.
    ///
    /// The set is a bitmask, so the failure mode is two keys sharing a bit — which
    /// would make one key press the other's button and is invisible in any test
    /// that holds one key at a time. Checked over every pair.
    #[test]
    fn every_key_has_its_own_slot() {
        for a in Key::ALL {
            let s = KeySet::from_keys(&[a]);
            assert!(s.contains(a), "{a:?} is not in a set containing it");
            for b in Key::ALL {
                if a != b {
                    assert!(!s.contains(b), "{a:?} and {b:?} share a slot");
                }
            }
        }
        // And the full set contains everything, which a mask that overflowed its
        // width would fail.
        let all = KeySet::from_keys(&Key::ALL);
        for k in Key::ALL {
            assert!(all.contains(k), "{k:?} missing from the full set");
        }
    }

    /// Every key's bit fits the set, and the set is wide enough for the next one.
    ///
    /// `every_key_has_its_own_slot` proves the bits are distinct; it does not prove
    /// they are *reachable*. `1u32 << 43` is a shift overflow — a debug-build panic
    /// and a release-build wrap to bit 11, which would silently alias P2's roundhouse
    /// to start 2. So the width is asserted against the highest bit any key uses.
    #[test]
    fn every_key_fits_the_set_with_room_left() {
        let highest = Key::ALL.iter().map(|k| k.bit()).max().expect("29+ keys");
        assert!(
            highest < u64::BITS,
            "key bit {highest} does not fit a u64 KeySet"
        );
        // Not merely "fits": every key must round-trip through a set, which a
        // wrapped shift would break by aliasing two keys onto one bit.
        for k in Key::ALL {
            let s = KeySet::from_keys(&[k]);
            assert!(
                s.contains(k),
                "{k:?} on bit {} does not round-trip",
                k.bit()
            );
            let others = Key::ALL.iter().filter(|&&o| o != k).count();
            assert_eq!(
                Key::ALL.iter().filter(|&&o| s.contains(o)).count(),
                1,
                "{k:?} aliases one of the other {others} keys"
            );
        }
    }

    /// `Key::ALL` lists every variant.
    ///
    /// Four tests above iterate `ALL` and would silently stop covering a key that
    /// was added to the enum and not to the list. The count is a literal, so adding
    /// a variant fails here — the one place that then tells you which tests to
    /// extend.
    #[test]
    fn all_lists_every_key_exactly_once() {
        assert_eq!(
            Key::ALL.len(),
            46,
            "add new keys to ALL, and to this literal"
        );
        for (i, a) in Key::ALL.iter().enumerate() {
            for b in &Key::ALL[i + 1..] {
                assert_ne!(a, b, "{a:?} appears twice");
            }
        }
    }

    /// Every preset puts each of the twelve player buttons on its own port bit.
    ///
    /// The literals are the point, as in `each_game_key_clears_its_own_port_bit`, and
    /// more so here: the four presets differ only in *which* of two rows is the punch
    /// row, so a table derived from `Preset::p1_punch()` would agree with a map that had
    /// every preset's rows transposed. [`PRESET_BUTTON_PORTS`] is written out by hand
    /// from `machine::inputs`' documented bits.
    #[test]
    fn every_preset_puts_the_twelve_buttons_on_their_own_ports() {
        for (p, k, in1, in2, what) in PRESET_BUTTON_PORTS {
            let mut c = Controls::new();
            c.set_preset(p);
            let i = c.update(KeySet::from_keys(&[k])).inputs;
            assert_eq!(i.in1(), in1, "{p:?}: {k:?} ({what}): IN1");
            assert_eq!(i.in2(), in2, "{p:?}: {k:?} ({what}): IN2");
            assert_eq!(i.in0(), 0xFF, "{p:?}: {k:?} ({what}) reached IN0");
        }
        // Within one preset no two buttons press the same thing, and no key appears
        // twice. A copy-paste that gave a preset's jab and strong one row's values
        // would otherwise pass every assertion above.
        for p in Preset::ALL {
            let rows: Vec<_> = PRESET_BUTTON_PORTS.iter().filter(|r| r.0 == p).collect();
            assert_eq!(rows.len(), 12, "{p:?} must have all twelve buttons");
            for (i, a) in rows.iter().enumerate() {
                for b in &rows[i + 1..] {
                    assert_ne!(a.1, b.1, "{p:?}: {:?} appears twice", a.1);
                    assert_ne!(
                        (a.2, a.3),
                        (b.2, b.3),
                        "{p:?}: {:?} and {:?} press the same thing",
                        a.1,
                        b.1
                    );
                }
            }
        }
    }

    /// A key the active preset does not use presses **nothing**.
    ///
    /// This is the consequence of bringing `Key::J` back, and it is the one property of
    /// the presets that is not a permutation: `J` is dead under the AZERTY presets and
    /// `M` is dead under the QWERTY ones. One physical key, one board input — a preset
    /// that left the old letter live as well would give P1 four punch keys for three
    /// buttons, and `every_preset_puts_the_twelve_buttons_on_their_own_ports` would
    /// still pass, because a spare key pressing a *duplicate* bit breaks none of its
    /// assertions.
    #[test]
    fn a_key_the_preset_does_not_use_presses_nothing() {
        let dead = [
            (Preset::AzertyPunchLow, Key::J),
            (Preset::AzertyCabinet, Key::J),
            (Preset::QwertyPunchLow, Key::M),
            (Preset::QwertyCabinet, Key::M),
        ];
        for (p, k) in dead {
            let mut c = Controls::new();
            c.set_preset(p);
            let i = c.update(KeySet::from_keys(&[k])).inputs;
            assert_eq!(i.in0(), 0xFF, "{p:?}: {k:?} is not in this preset");
            assert_eq!(i.in1(), 0xFFFF, "{p:?}: {k:?} reached a stick or a punch");
            assert_eq!(i.in2(), 0xFF, "{p:?}: {k:?} reached a kick");
        }
        // And the mirror: each of those keys *is* live under the other pair, so the
        // assertions above are about the preset and not about a key that never works.
        let live = [
            (Preset::QwertyPunchLow, Key::J, 0xFFEFu16, 0xFFu8),
            (Preset::QwertyCabinet, Key::J, 0xFFFF, 0xFE),
            (Preset::AzertyPunchLow, Key::M, 0xFFBF, 0xFF),
            (Preset::AzertyCabinet, Key::M, 0xFFFF, 0xFB),
        ];
        for (p, k, in1, in2) in live {
            let mut c = Controls::new();
            c.set_preset(p);
            let i = c.update(KeySet::from_keys(&[k])).inputs;
            assert_eq!(i.in1(), in1, "{p:?}: {k:?} must be live: IN1");
            assert_eq!(i.in2(), in2, "{p:?}: {k:?} must be live: IN2");
        }
    }

    /// No preset moves the sticks, the coins, the starts or the test switch.
    ///
    /// The presets cover the twelve player buttons and nothing else — deliberately. A
    /// preset that moved a coin would be a surprise; a preset that moved `Escape` could
    /// strand the player in a window they cannot close. Asserted with the same literals
    /// [`GAME_KEY_PORTS`] uses, once per preset.
    #[test]
    fn no_preset_moves_a_stick_a_coin_a_start_or_the_test_switch() {
        let fixed = [
            (Key::Z, 0xFFu8, 0xFFF7u16, 0xFFu8, "P1 up"),
            (Key::S, 0xFF, 0xFFFB, 0xFF, "P1 down"),
            (Key::Q, 0xFF, 0xFFFD, 0xFF, "P1 left"),
            (Key::D, 0xFF, 0xFFFE, 0xFF, "P1 right"),
            (Key::Up, 0xFF, 0xF7FF, 0xFF, "P2 up"),
            (Key::Down, 0xFF, 0xFBFF, 0xFF, "P2 down"),
            (Key::Left, 0xFF, 0xFDFF, 0xFF, "P2 left"),
            (Key::Right, 0xFF, 0xFEFF, 0xFF, "P2 right"),
            (Key::Num5, 0xFE, 0xFFFF, 0xFF, "coin 1"),
            (Key::Num6, 0xFD, 0xFFFF, 0xFF, "coin 2"),
            (Key::Num1, 0xEF, 0xFFFF, 0xFF, "start 1"),
            (Key::Num2, 0xDF, 0xFFFF, 0xFF, "start 2"),
            (Key::F2, 0xBF, 0xFFFF, 0xFF, "the test switch"),
        ];
        for p in Preset::ALL {
            for (k, in0, in1, in2, what) in fixed {
                let mut c = Controls::new();
                c.set_preset(p);
                let i = c.update(KeySet::from_keys(&[k])).inputs;
                assert_eq!(i.in0(), in0, "{p:?}: {k:?} ({what}): IN0");
                assert_eq!(i.in1(), in1, "{p:?}: {k:?} ({what}): IN1");
                assert_eq!(i.in2(), in2, "{p:?}: {k:?} ({what}): IN2");
            }
        }
    }

    /// The default preset is the one that was asked for, and `GAME_KEY_PORTS` describes it.
    ///
    /// Two claims in one, both worth pinning: a fresh `Controls` is
    /// `AzertyPunchLow` — which is what makes `each_game_key_clears_its_own_port_bit`
    /// meaningful, since it never calls `set_preset` — and the default is punches-low,
    /// so nobody's keyboard changes under them when the menu ships.
    #[test]
    fn the_default_preset_is_azerty_punches_low() {
        assert_eq!(Controls::new().preset(), Preset::AzertyPunchLow);
        assert_eq!(Preset::default(), Preset::AzertyPunchLow);
        assert_eq!(
            Preset::ALL[0],
            Preset::AzertyPunchLow,
            "and it is listed first"
        );
        assert_eq!(
            Preset::AzertyPunchLow.p1_punch(),
            [Key::K, Key::L, Key::M],
            "punches on the home row"
        );
    }

    /// Switching preset takes effect on the next frame, for keys already held.
    ///
    /// The alternative — latching the map at the moment a key went down — would need
    /// per-key state and would mean a key held across an apply kept its old meaning,
    /// which nobody can see from the keyboard. Re-reading is the simpler rule *and* the
    /// one a player can predict.
    #[test]
    fn switching_preset_rereads_the_keys_already_held() {
        let mut c = Controls::new();
        let held = KeySet::from_keys(&[Key::K]);
        // AZERTY, punches low: `K` is the jab, IN1 bit 4.
        assert_eq!(c.update(held).inputs.in1(), 0xFFEF, "K is P1's jab");
        assert_eq!(c.update(held).inputs.in2(), 0xFF, "and not a kick");
        // The same held key, a cabinet's order: `K` is now the short kick, IN2 bit 0.
        c.set_preset(Preset::AzertyCabinet);
        let i = c.update(held).inputs;
        assert_eq!(i.in1(), 0xFFFF, "K is no longer a punch");
        assert_eq!(i.in2(), 0xFE, "K is P1's short kick now");
    }

    /// Every preset's tag round-trips, and no two share one.
    ///
    /// The tags are what go to disk. A duplicate would make one preset unreachable on
    /// reload and `from_tag` would silently return the other — the same file meaning a
    /// different map, with nothing to see in the file.
    #[test]
    fn preset_tags_round_trip_and_are_distinct() {
        for p in Preset::ALL {
            assert_eq!(
                Preset::from_tag(p.tag()),
                Some(p),
                "{p:?} does not round-trip"
            );
            // Whitespace is trimmed, because a file written with a trailing newline is
            // the normal case and not a corrupt one.
            assert_eq!(Preset::from_tag(&format!("{}\n", p.tag())), Some(p));
        }
        for (i, a) in Preset::ALL.iter().enumerate() {
            for b in &Preset::ALL[i + 1..] {
                assert_ne!(a.tag(), b.tag(), "{a:?} and {b:?} share a tag");
                assert_ne!(a.name(), b.name(), "{a:?} and {b:?} share a name");
            }
        }
        // The tags are literals, so a rename that changed what saved files mean fails
        // here rather than silently resetting everyone to the default.
        assert_eq!(Preset::AzertyPunchLow.tag(), "azerty-punch-low");
        assert_eq!(Preset::AzertyCabinet.tag(), "azerty-cabinet");
        assert_eq!(Preset::QwertyPunchLow.tag(), "qwerty-punch-low");
        assert_eq!(Preset::QwertyCabinet.tag(), "qwerty-cabinet");
        // An unknown tag is `None` and not a panic: a hand-edited or older file falls
        // back to the default the way a missing save state does.
        assert_eq!(Preset::from_tag(""), None);
        assert_eq!(Preset::from_tag("azerty"), None);
        assert_eq!(Preset::from_tag("AZERTY-PUNCH-LOW"), None);
    }

    /// An open menu is an **idle board**, not merely a board whose new presses are dropped.
    ///
    /// The distinction has a real failure mode, and it is the reason this is stated as
    /// idle: `Inputs` is level-triggered, so a stick held at the moment the menu opened
    /// stays held in the board's eyes until something writes the released value. A menu
    /// that only ignored *fresh* presses would leave the player crouching, blocking or
    /// walking left for as long as they read it.
    #[test]
    fn an_open_menu_is_an_idle_board() {
        let mut c = Controls::new();
        // Down-left and a fierce punch, all held before the menu opens.
        let held = KeySet::from_keys(&[Key::S, Key::Q, Key::M, Key::NumPad8]);
        let i = c.update(held).inputs;
        assert_eq!(i.in1(), 0xFFB9, "held: P1 down, left and fierce");
        assert_eq!(i.in2(), 0xDF, "and P2's forward kick");

        // Open it without touching the keys. The same held set must now read released.
        c.set_menu_open(true);
        let i = c.update(held).inputs;
        assert_eq!(i.in0(), 0xFF, "idle: IN0");
        assert_eq!(i.in1(), 0xFFFF, "idle: the stick is no longer held");
        assert_eq!(i.in2(), 0xFF, "idle: nor the kick");

        // And a key pressed *while* it is open reaches nothing either.
        let more = KeySet::from_keys(&[Key::S, Key::Q, Key::M, Key::NumPad8, Key::K]);
        assert_eq!(
            c.update(more).inputs.in1(),
            0xFFFF,
            "a fresh press is idle too"
        );

        // Closing it hands the board back, on the very next frame, with no fresh press
        // needed — the keys never went up.
        c.set_menu_open(false);
        let i = c.update(held).inputs;
        assert_eq!(i.in1(), 0xFFB9, "the board is live again");
        assert_eq!(i.in2(), 0xDF);
    }

    /// The board reads idle under every preset while the menu is open.
    ///
    /// `an_open_menu_is_an_idle_board` presses one hand under the default. This presses
    /// **every** key there is, under all four presets, because the gate is one `if` around
    /// the whole game half and the mistake it is guarding against is a line left outside
    /// it. A single field assigned before the gate would show up here and nowhere else.
    #[test]
    fn no_key_reaches_the_board_while_the_menu_is_open() {
        for p in Preset::ALL {
            for k in Key::ALL {
                let mut c = Controls::new();
                c.set_preset(p);
                c.set_menu_open(true);
                let i = c.update(KeySet::from_keys(&[k])).inputs;
                assert_eq!(i.in0(), 0xFF, "{p:?}: {k:?} reached IN0 with the menu open");
                assert_eq!(
                    i.in1(),
                    0xFFFF,
                    "{p:?}: {k:?} reached IN1 with the menu open"
                );
                assert_eq!(i.in2(), 0xFF, "{p:?}: {k:?} reached IN2 with the menu open");
            }
        }
        // Every key at once, which catches a gate that only holds for a lone press.
        let mut c = Controls::new();
        c.set_menu_open(true);
        let i = c.update(KeySet::from_keys(&Key::ALL)).inputs;
        assert_eq!(i.in0(), 0xFF, "all 46 keys at once: IN0");
        assert_eq!(i.in1(), 0xFFFF, "all 46 keys at once: IN1");
        assert_eq!(i.in2(), 0xFF, "all 46 keys at once: IN2");
        // The DIP switches are cabinet configuration and are *not* part of the capture:
        // an idle board still boots in the configuration it was given.
        c.set_dsw([0x12, 0x34, 0x56]);
        assert_eq!(
            c.update(KeySet::new()).inputs.dsw,
            [0x12, 0x34, 0x56],
            "the switches survive an open menu"
        );
    }

    /// `Escape` closes the menu instead of quitting.
    ///
    /// The single most load-bearing line in the capture. `Escape` is the instinctive way
    /// to back out of a menu and it is also the key that ends the session, so a menu that
    /// did not take it away would kill the emulator the first time anyone tried to cancel.
    #[test]
    fn escape_closes_the_menu_instead_of_quitting() {
        let mut c = Controls::new();
        let esc = KeySet::from_keys(&[Key::Escape]);

        // Closed: `Escape` quits and there is no menu to close.
        let a = c.update(esc);
        assert!(a.quit, "with the menu shut, Escape quits");
        assert!(!a.menu_close);

        // Open: it closes the menu and quits nothing.
        c.update(KeySet::new());
        c.set_menu_open(true);
        let a = c.update(esc);
        assert!(!a.quit, "with the menu open, Escape must NOT quit");
        assert!(a.menu_close, "it closes the menu");
    }

    /// While the menu is open, every control is swallowed except `Tab`.
    ///
    /// `Tab` is the exception by necessity: it is what closes the menu, so gating it would
    /// make the menu impossible to leave except by `Escape`. Everything else must be
    /// inert — a reset, a save, a load or a screenshot triggered from inside a key menu is
    /// an action nobody asked for, and `Enter` in particular would apply a preset *and*
    /// cycle a tile layout from one keypress.
    #[test]
    fn an_open_menu_swallows_every_control_but_tab() {
        /// See `every_control_action_is_edge_triggered` for why this is named.
        type Reader = fn(&Actions) -> bool;
        let cases: [(Key, Reader, &str); 19] = [
            (Key::F11, |a| a.pause_toggled, "pause"),
            (Key::Period, |a| a.step, "step"),
            (Key::F3, |a| a.reset, "reset"),
            (Key::F5, |a| a.save, "save"),
            (Key::F8, |a| a.load, "load"),
            (Key::F12, |a| a.screenshot, "screenshot"),
            (Key::Escape, |a| a.quit, "quit"),
            (Key::F1, |a| a.overlay_toggled, "the debugger overlay"),
            (Key::F4, |a| a.step_instruction, "an instruction step"),
            (Key::F6, |a| a.focus_cycled, "the scroll focus"),
            (Key::F7, |a| a.breakpoint_toggled, "a breakpoint"),
            (Key::PageUp, |a| a.scroll_up, "a scroll"),
            (Key::PageDown, |a| a.scroll_down, "a scroll"),
            (Key::Home, |a| a.follow_reset, "the follow reset"),
            (Key::GfxToggled, |a| a.gfx_toggled, "the graphics viewer"),
            (Key::GfxView, |a| a.gfx_view_cycled, "the graphics view"),
            (Key::BracketLeft, |a| a.gfx_back, "the graphics view"),
            (Key::BracketRight, |a| a.gfx_forward, "the graphics view"),
            (Key::Enter, |a| a.gfx_act, "the graphics view"),
        ];
        for (k, get, what) in cases {
            let mut c = Controls::new();
            c.set_menu_open(true);
            let held = KeySet::from_keys(&[k]);
            assert!(
                !get(&c.update(held)),
                "{k:?} reached {what} with the menu open"
            );
            // Not merely "not on the first frame": held, released and pressed again.
            assert!(!get(&c.update(held)), "{k:?} still held");
            c.update(KeySet::new());
            assert!(!get(&c.update(held)), "{k:?} pressed again");
            // And with the menu shut it works, so the assertions above are about the
            // capture and not about a control that never fires.
            c.set_menu_open(false);
            c.update(KeySet::new());
            assert!(get(&c.update(held)), "{k:?} must reach {what} once closed");
        }
        // `Tab` is the exception, and it fires with the menu both open and shut.
        let tab = KeySet::from_keys(&[Key::Tab]);
        for open in [false, true] {
            let mut c = Controls::new();
            c.set_menu_open(open);
            assert!(
                c.update(tab).menu_toggled,
                "Tab must toggle the menu with it open={open}"
            );
        }
    }

    /// The menu's four navigation actions fire only while it is open.
    ///
    /// The mirror image of the test above, and the other half of what makes one key mean
    /// two things safely: `Up` and `Down` are P2's stick, `Enter` acts on the graphics
    /// view and `Escape` quits. If these fired with the menu shut, holding P2's up would
    /// walk a selection in a menu nobody can see.
    #[test]
    fn the_menus_navigation_fires_only_while_it_is_open() {
        /// See `every_control_action_is_edge_triggered` for why this is named.
        type Reader = fn(&Actions) -> bool;
        let cases: [(Key, Reader); 4] = [
            (Key::Up, |a| a.menu_up),
            (Key::Down, |a| a.menu_down),
            (Key::Enter, |a| a.menu_apply),
            (Key::Escape, |a| a.menu_close),
        ];
        for (k, get) in cases {
            let mut c = Controls::new();
            let held = KeySet::from_keys(&[k]);
            assert!(!get(&c.update(held)), "{k:?} must not navigate a shut menu");
            c.update(KeySet::new());
            c.set_menu_open(true);
            assert!(get(&c.update(held)), "{k:?} must navigate an open menu");
        }
    }

    /// The menu's navigation is edge-triggered, like every other control.
    ///
    /// Held `Down` must not walk sixty rows a second past the end of a five-row list.
    /// `every_control_action_is_edge_triggered` cannot cover these — they only fire with
    /// the menu open — so this runs the same four-frame press against an open one.
    #[test]
    fn the_menus_navigation_is_edge_triggered_too() {
        /// See `every_control_action_is_edge_triggered` for why this is named.
        type Reader = fn(&Actions) -> bool;
        let cases: [(Key, Reader); 5] = [
            (Key::Tab, |a| a.menu_toggled),
            (Key::Up, |a| a.menu_up),
            (Key::Down, |a| a.menu_down),
            (Key::Enter, |a| a.menu_apply),
            (Key::Escape, |a| a.menu_close),
        ];
        for (k, get) in cases {
            let mut c = Controls::new();
            c.set_menu_open(true);
            let held = KeySet::from_keys(&[k]);
            assert!(get(&c.update(held)), "{k:?} must fire on the press");
            assert!(!get(&c.update(held)), "{k:?} must not fire while held");
            assert!(!get(&c.update(held)), "{k:?} still held");
            c.update(KeySet::new());
            assert!(
                get(&c.update(held)),
                "{k:?} must fire again after a release"
            );
        }
    }
}
