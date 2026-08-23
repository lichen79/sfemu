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
    M,
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
    /// `F11` and not `P`, which is P1's fierce punch. The letter area belongs to the
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
}

impl Key {
    /// Every variant, for the tests that must cover all of them.
    ///
    /// `tests::all_lists_every_key_exactly_once` fails if a variant is added and
    /// not listed here, which is what stops the tests that iterate this from
    /// quietly narrowing.
    pub const ALL: [Key; 44] = [
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
        }
    }
}

/// Which keys are held.
///
/// A bitmask rather than a `Vec`, so [`Controls`] can keep last frame's set by
/// copy and the edge detection is one `&`.
///
/// `u64` and not `u32`: 44 keys hold bits 0-43. It was a `u32` through E2's 29 keys,
/// and the alternative to widening was overloading `PageUp`/`PageDown`/`Home` to
/// mean something else while the graphics viewer is up — which would have reached 31
/// keys, leaving exactly one free bit, and `scripts/mutate.py`'s control mutant needs
/// a free bit to move `Escape` to. Mapping player 2 then added ten more, which a `u32`
/// could not have held at all: 44 keys is 12 bits past its width.
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
        // Player 1, the left of the keyboard: ZSQD stick, KLM punches on the home row,
        // IOP kicks on the row above. Punches under the resting fingers, which inverts a
        // cabinet's own arrangement -- see the module docs.
        inputs.p1.up = now.contains(Key::Z);
        inputs.p1.down = now.contains(Key::S);
        inputs.p1.left = now.contains(Key::Q);
        inputs.p1.right = now.contains(Key::D);
        inputs.p1.punch = [
            now.contains(Key::K),
            now.contains(Key::L),
            now.contains(Key::M),
        ];
        inputs.p1.kick = [
            now.contains(Key::I),
            now.contains(Key::O),
            now.contains(Key::P),
        ];
        // Player 2, the right: arrow-key stick, keypad 456 punches under 789 kicks --
        // mirroring P1's inversion, so the two halves agree about which row is which.
        inputs.p2.up = now.contains(Key::Up);
        inputs.p2.down = now.contains(Key::Down);
        inputs.p2.left = now.contains(Key::Left);
        inputs.p2.right = now.contains(Key::Right);
        inputs.p2.punch = [
            now.contains(Key::NumPad4),
            now.contains(Key::NumPad5),
            now.contains(Key::NumPad6),
        ];
        inputs.p2.kick = [
            now.contains(Key::NumPad7),
            now.contains(Key::NumPad8),
            now.contains(Key::NumPad9),
        ];
        inputs.coin1 = now.contains(Key::Num5);
        inputs.coin2 = now.contains(Key::Num6);
        inputs.start1 = now.contains(Key::Num1);
        inputs.start2 = now.contains(Key::Num2);
        // Level-triggered, unlike every other function key: the service menu is
        // entered by *holding* the test switch, which is what the switch does on a
        // real cabinet.
        inputs.test = now.contains(Key::F2);

        let actions = Actions {
            inputs,
            pause_toggled: edge(Key::F11),
            step: edge(Key::Period),
            reset: edge(Key::F3),
            save: edge(Key::F5),
            load: edge(Key::F8),
            screenshot: edge(Key::F12),
            quit: edge(Key::Escape),
            overlay_toggled: edge(Key::F1),
            step_instruction: edge(Key::F4),
            focus_cycled: edge(Key::F6),
            breakpoint_toggled: edge(Key::F7),
            // The scroll keys are edge-triggered like the rest, not repeating. A held
            // `PageDown` walking sixty pages a second is not a usable way to find an
            // address, and auto-repeat would need a timer — which would put a clock in
            // the one crate that deliberately has none.
            scroll_up: edge(Key::PageUp),
            scroll_down: edge(Key::PageDown),
            follow_reset: edge(Key::Home),
            // Edge-triggered, every one, for the reason written just above: a held
            // `]` walking sixty pages a second is not a way to find a tile.
            gfx_toggled: edge(Key::GfxToggled),
            gfx_view_cycled: edge(Key::GfxView),
            gfx_back: edge(Key::BracketLeft),
            gfx_forward: edge(Key::BracketRight),
            gfx_act: edge(Key::Enter),
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
    #[test]
    fn the_port_bit_table_covers_every_game_key_and_only_those() {
        let control = [
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
        ];
        let tabled: Vec<Key> = GAME_KEY_PORTS.iter().map(|r| r.0).collect();
        // Every key that is not a control has a row.
        for k in Key::ALL {
            if control.contains(&k) {
                assert!(
                    !tabled.contains(&k),
                    "{k:?} is a control and must not have a port row"
                );
            } else {
                assert!(
                    tabled.contains(&k),
                    "{k:?} is a game key with no row in GAME_KEY_PORTS, so no port \
                     assertion covers it"
                );
            }
        }
        // And the count, which catches a row for a key that is not in `Key::ALL` at all.
        assert_eq!(
            tabled.len(),
            Key::ALL.len() - control.len(),
            "the table has {} rows for {} game keys",
            tabled.len(),
            Key::ALL.len() - control.len()
        );
        assert_eq!(
            tabled.len(),
            25,
            "25 game keys: 20 player inputs, 4 coin and \
                    start buttons, and the test switch"
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
        let control = [
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
        ];
        assert_eq!(control.len(), 19, "add a new control here too");
        for k in control {
            let mut c = Controls::new();
            let i = c.update(KeySet::from_keys(&[k])).inputs;
            assert_eq!(
                i.in0(),
                0xFF,
                "{k:?} reached a coin, start, service or test"
            );
            assert_eq!(i.in1(), 0xFFFF, "{k:?} reached a stick or a punch");
            assert_eq!(i.in2(), 0xFF, "{k:?} reached a kick");
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
    /// Checked as a table over all nineteen, because the natural implementation is one
    /// `edge` helper per action and the natural mistake is to forget it on one of
    /// them — which then works exactly once out of nineteen, in whichever action the
    /// author tested by hand.
    ///
    /// The debugger's seven and the graphics viewer's five are in the same table as
    /// the original seven rather than tables of their own: they are the same kind of
    /// thing, and a separate table is a second place to forget to add a row.
    #[test]
    fn every_control_action_is_edge_triggered() {
        /// Reads one action's flag. Named because clippy calls the inline array
        /// type too complex, and it is: a table of key-and-accessor pairs.
        type Reader = fn(&Actions) -> bool;
        let cases: [(Key, Reader); 19] = [
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
            44,
            "add new keys to ALL, and to this literal"
        );
        for (i, a) in Key::ALL.iter().enumerate() {
            for b in &Key::ALL[i + 1..] {
                assert_ne!(a, b, "{a:?} appears twice");
            }
        }
    }
}
