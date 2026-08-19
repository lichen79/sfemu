//! SF1's three input ports, as the 68000 sees them.
//!
//! # Why this is not `crate::inputs`
//!
//! Same board family, four incompatible differences. The short version: the DIPs
//! are two words rather than three bytes, six of the twelve attack buttons are on
//! a different port than CPS-1 puts them, coins and starts are split across two
//! ports rather than sharing `IN0`, and one bit is active **high**.
//!
//! [`PlayerInput`] itself is reused: four stick booleans and two arrays of three
//! is the same shape on both boards, and only the bit positions differ.
//!
//! Bit assignments cited to `src/mame/capcom/sf.cpp` at tag `mame0261`:
//! `DSW1` at 477-515, `DSW2` at 517-547, `SYSTEM` at 549-566, `IN0` at 568-585
//! with `sfus`'s overrides at 636-640, `IN1` at 587-604 with `sfus`'s overrides
//! at 642-651.

use crate::inputs::PlayerInput;

/// `DSW1` bit 8: screen flip (`DSW2.13E:1`, `sf.cpp:498`).
///
/// The board does not act on this itself — the 68000 reads the port and writes
/// bit 2 of `gfxctrl` (`sf.cpp:351`), so screen flip reaches the video through
/// software. Named here because a frontend DIP panel needs it.
pub const DSW1_FLIP_SCREEN: u16 = 0x0100;
/// `DSW1` bit 13: demo sounds (`DSW2.13E:6`, `sf.cpp:509`).
///
/// ⚠️ **Clear means on.** The only switch in either port whose default is the
/// cleared state.
pub const DSW1_DEMO_SOUNDS: u16 = 0x2000;
/// `DSW1` bit 15: the self-test switch (`DSW2.13E:8`, `sf.cpp:515`).
/// `PORT_SERVICE_DIPLOC` with `IP_ACTIVE_LOW`, so clear enters the service menu.
pub const DSW1_SERVICE_MODE: u16 = 0x8000;

/// `DSW1`'s power-on value: every switch at MAME's default.
///
/// All ones except [`DSW1_DEMO_SOUNDS`]. Written as an expression rather than the
/// literal 0xDFFF so the one exception is visible. The `u16` complement is already
/// all-ones-but-that-bit, so there is no `0xFFFF &` to write.
const DSW1_DEFAULT: u16 = !DSW1_DEMO_SOUNDS;
/// `DSW2`'s power-on value. Every named switch and every unused location
/// defaults set (`sf.cpp:517-547`).
const DSW2_DEFAULT: u16 = 0xFFFF;

/// Controls and DIP switches for the `sf` set.
///
/// Set a field to `true` for **pressed**. The active-low conversion happens here,
/// once, so no caller has to remember the polarity — nor that one `SYSTEM` bit
/// runs the other way.
#[derive(Debug, Clone, Copy)]
pub struct Sf1Inputs {
    /// Coin slot 1 (`IN0` bit 0).
    pub coin1: bool,
    /// Coin slot 2 (`IN0` bit 1).
    pub coin2: bool,
    /// Service coin (`SYSTEM` bit 2) — **not** on `IN0`, where CPS-1 puts it.
    pub service: bool,
    /// Player 1 start (`SYSTEM` bit 0).
    pub start1: bool,
    /// Player 2 start (`SYSTEM` bit 1).
    pub start2: bool,
    /// Player 1's stick and six buttons, spread across `IN0` and `IN1`.
    pub p1: PlayerInput,
    /// Player 2's stick and six buttons.
    pub p2: PlayerInput,
    /// `DSW1` and `DSW2`, each read as a whole word.
    pub dsw: [u16; 2],
}

impl Default for Sf1Inputs {
    /// [`Sf1Inputs::idle`] — **not** the derived all-zero, which would be a board
    /// with every DIP switch on: four coins per credit, the screen flipped, and
    /// the service menu up.
    fn default() -> Self {
        Self::idle()
    }
}

impl Sf1Inputs {
    /// Nothing pressed, every DIP switch at MAME's default.
    #[must_use]
    pub const fn idle() -> Self {
        Self {
            coin1: false,
            coin2: false,
            service: false,
            start1: false,
            start2: false,
            p1: PlayerInput::none(),
            p2: PlayerInput::none(),
            dsw: [DSW1_DEFAULT, DSW2_DEFAULT],
        }
    }

    /// `IN0`, 0xC00000 — the two coins and, oddly, four attack buttons.
    ///
    /// `sf.cpp:636-640`. The four buttons are the ones a six-button `sfus` panel
    /// could not fit on `IN1`, and they are not laid out per player: P2's
    /// roundhouse at bit 8 sits below P1's fierce at bit 9.
    ///
    /// Written out rather than routed through `active_low`: three of its wired
    /// bits are above the low byte, and this is the only port where that is true —
    /// `in1` composes two bytes and `system` has three bits in one.
    #[must_use]
    pub fn in0(&self) -> u16 {
        let mut v = 0xFFFFu16;
        for &(bit, pressed) in &[
            (0, self.coin1),
            (1, self.coin2),
            (2, self.p1.kick[2]),
            (8, self.p2.kick[2]),
            (9, self.p1.punch[2]),
            (10, self.p2.punch[2]),
        ] {
            if pressed {
                v &= !(1u16 << bit);
            }
        }
        v
    }

    /// `IN1`, 0xC00002 — both sticks and the other four buttons each.
    ///
    /// `sf.cpp:642-651` over `common`'s stick. P1 in the low byte, P2 in the high,
    /// and within each byte the buttons interleave: jab, strong, **short**,
    /// forward.
    #[must_use]
    pub fn in1(&self) -> u16 {
        let lo = stick_and_four_buttons(&self.p1);
        let hi = stick_and_four_buttons(&self.p2);
        (u16::from(hi) << 8) | u16::from(lo)
    }

    /// `SYSTEM`, 0xC0000C — starts and the service coin.
    ///
    /// ⚠️ Bit 7 is `IP_ACTIVE_HIGH` (`sf.cpp:709`, commented "Freezes the game ?").
    /// Unwired and active high means it reads **0** while its fourteen unwired
    /// neighbours read 1, so an idle `SYSTEM` is 0xFF7F. That is why this is not
    /// one `active_low` call.
    #[must_use]
    pub fn system(&self) -> u16 {
        let wired = active_low(&[(0, self.start1), (1, self.start2), (2, self.service)]);
        (u16::from(wired) & !SYSTEM_ACTIVE_HIGH_BIT) | 0xFF00
    }
}

/// `SYSTEM` bit 7 — the one active-high bit on the board (`sf.cpp:709`).
const SYSTEM_ACTIVE_HIGH_BIT: u16 = 0x0080;

/// One player's half of `IN1`: stick in bits 0-3, four buttons in bits 4-7.
///
/// The order is jab, strong, short, forward — a punch, a punch, a **kick**, a
/// kick. Writing `punch[2]` at bit 6 (CPS-1's layout) puts fierce where short
/// belongs, which plays as a game whose heavy punch does a light kick.
fn stick_and_four_buttons(p: &PlayerInput) -> u8 {
    active_low(&[
        (0, p.right),
        (1, p.left),
        (2, p.down),
        (3, p.up),
        (4, p.punch[0]),
        (5, p.punch[1]),
        (6, p.kick[0]),
        (7, p.kick[1]),
    ])
}

/// Starts from all-released and clears one bit per pressed control.
///
/// A byte-wide copy of [`crate::inputs`]'s helper rather than a shared one: that
/// module's is private, this crate's two boards have no other overlap, and making
/// it `pub(crate)` to save eleven lines would couple the SF1 port layout to a
/// CPS-1 module for nothing.
fn active_low(bits: &[(u32, bool)]) -> u8 {
    let mut v = 0xFFu8;
    for &(bit, pressed) in bits {
        if pressed {
            v &= !(1u8 << bit);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An idle board is all ones — except `SYSTEM` bit 7.
    ///
    /// `sf.cpp:709` is `PORT_BIT(0x0080, IP_ACTIVE_HIGH, IPT_UNKNOWN)` with the
    /// comment "Freezes the game ?". Active high and unwired means it reads 0 while
    /// everything around it reads 1. A helper that only ever clears bits cannot
    /// produce this, which is the point of the test.
    #[test]
    fn an_idle_board_is_all_ones_except_system_bit_seven() {
        let i = Sf1Inputs::idle();
        assert_eq!(i.in0(), 0xFFFF);
        assert_eq!(i.in1(), 0xFFFF);
        assert_eq!(i.system(), 0xFF7F, "bit 7 is IP_ACTIVE_HIGH");
    }

    /// `default()` is `idle()`, and in particular not all-zero.
    ///
    /// A derived `Default` gives `dsw: [0, 0]` — every DIP switch on, which on this
    /// board means 4-coins-1-credit, screen flipped, and the service menu — so a
    /// caller writing `Sf1Inputs::default()` would run a differently configured
    /// machine and blame the CPU.
    #[test]
    fn default_is_idle_and_not_all_zero() {
        assert_eq!(Sf1Inputs::default().dsw, Sf1Inputs::idle().dsw);
        assert_eq!(Sf1Inputs::default().in0(), 0xFFFF);
        assert_eq!(Sf1Inputs::default().system(), 0xFF7F);
    }

    /// The two coin slots are `IN0` bits 0 and 1, and nothing else on `IN0` is a coin.
    #[test]
    fn the_coins_are_in0_bits_zero_and_one() {
        let mut i = Sf1Inputs::idle();
        i.coin1 = true;
        assert_eq!(i.in0(), 0xFFFE);
        i = Sf1Inputs::idle();
        i.coin2 = true;
        assert_eq!(i.in0(), 0xFFFD);
        i = Sf1Inputs::idle();
        i.coin1 = true;
        i.coin2 = true;
        assert_eq!(i.in0(), 0xFFFC);
    }

    /// Fierce and roundhouse are on `IN0`, at four scattered bits.
    ///
    /// `sf.cpp:637-640`, in the file's own order:
    /// ```text
    /// 0x0004  BUTTON6 P1   roundhouse
    /// 0x0100  BUTTON6 P2   roundhouse
    /// 0x0200  BUTTON3 P1   fierce
    /// 0x0400  BUTTON3 P2   fierce
    /// ```
    /// P2's roundhouse is *below* P1's fierce, so the two players do not occupy
    /// contiguous fields and a byte-per-player model gets this wrong.
    #[test]
    fn fierce_and_roundhouse_are_on_in0_at_scattered_bits() {
        let mut i = Sf1Inputs::idle();
        i.p1.kick[2] = true;
        assert_eq!(i.in0(), 0xFFFB, "P1 roundhouse, bit 2");
        i = Sf1Inputs::idle();
        i.p2.kick[2] = true;
        assert_eq!(i.in0(), 0xFEFF, "P2 roundhouse, bit 8");
        i = Sf1Inputs::idle();
        i.p1.punch[2] = true;
        assert_eq!(i.in0(), 0xFDFF, "P1 fierce, bit 9");
        i = Sf1Inputs::idle();
        i.p2.punch[2] = true;
        assert_eq!(i.in0(), 0xFBFF, "P2 fierce, bit 10");
    }

    /// Every remaining `IN0` bit is unwired and stays set.
    #[test]
    fn the_other_eleven_in0_bits_are_not_wired_to_anything() {
        let i = every_control_pressed();
        assert_eq!(i.in0(), 0xF8F8, "only the six wired bits clear");
    }

    /// Both sticks and four of the six buttons are on `IN1`.
    ///
    /// `sf.cpp:643-650` over `common`'s stick (`sf.cpp:731-742`):
    /// ```text
    /// P1  0x0001 right  0x0002 left  0x0004 down  0x0008 up
    ///     0x0010 BUTTON1 jab   0x0020 BUTTON2 strong
    ///     0x0040 BUTTON4 short 0x0080 BUTTON5 forward
    /// P2  the same, shifted up by 8
    /// ```
    /// Note `BUTTON4` at bit 6 sits between `BUTTON2` and `BUTTON5` — the punches
    /// and kicks interleave, so a model that gave punches bits 4-6 (as CPS-1 does)
    /// would put short where fierce belongs.
    #[test]
    fn each_in1_control_clears_its_own_bit_with_p1_low_and_p2_high() {
        let mut i = Sf1Inputs::idle();
        i.p1.right = true;
        assert_eq!(i.in1(), 0xFFFE);
        i = Sf1Inputs::idle();
        i.p1.left = true;
        assert_eq!(i.in1(), 0xFFFD);
        i = Sf1Inputs::idle();
        i.p1.down = true;
        assert_eq!(i.in1(), 0xFFFB);
        i = Sf1Inputs::idle();
        i.p1.up = true;
        assert_eq!(i.in1(), 0xFFF7);
        i = Sf1Inputs::idle();
        i.p1.punch[0] = true;
        assert_eq!(i.in1(), 0xFFEF, "jab, bit 4");
        i = Sf1Inputs::idle();
        i.p1.punch[1] = true;
        assert_eq!(i.in1(), 0xFFDF, "strong, bit 5");
        i = Sf1Inputs::idle();
        i.p1.kick[0] = true;
        assert_eq!(i.in1(), 0xFFBF, "short, bit 6 — not fierce");
        i = Sf1Inputs::idle();
        i.p1.kick[1] = true;
        assert_eq!(i.in1(), 0xFF7F, "forward, bit 7");
        i = Sf1Inputs::idle();
        i.p2.right = true;
        assert_eq!(i.in1(), 0xFEFF);
        i = Sf1Inputs::idle();
        i.p2.up = true;
        assert_eq!(i.in1(), 0xF7FF);
        i = Sf1Inputs::idle();
        i.p2.punch[0] = true;
        assert_eq!(i.in1(), 0xEFFF, "P2 jab, bit 12");
        i = Sf1Inputs::idle();
        i.p2.kick[1] = true;
        assert_eq!(i.in1(), 0x7FFF, "P2 forward, bit 15");
    }

    /// Every `IN1` bit is wired, so all sixteen clear at once.
    #[test]
    fn in1_has_no_unwired_bits() {
        assert_eq!(every_control_pressed().in1(), 0x0000);
    }

    /// Starts and the service coin are on `SYSTEM`, bits 0-2.
    #[test]
    fn the_starts_and_service_are_on_system() {
        let mut i = Sf1Inputs::idle();
        i.start1 = true;
        assert_eq!(i.system(), 0xFF7E);
        i = Sf1Inputs::idle();
        i.start2 = true;
        assert_eq!(i.system(), 0xFF7D);
        i = Sf1Inputs::idle();
        i.service = true;
        assert_eq!(i.system(), 0xFF7B);
        i = Sf1Inputs::idle();
        i.start1 = true;
        i.start2 = true;
        i.service = true;
        assert_eq!(i.system(), 0xFF78, "and bit 7 is still clear");
    }

    /// A control never appears on two ports.
    ///
    /// Six of the twelve attack buttons live on `IN0` and six on `IN1`. A model
    /// that put all twelve on both would pass each port's own bit test — every
    /// assertion above names one control and one port — and fail only here.
    #[test]
    fn no_control_leaks_between_the_three_ports() {
        let mut i = Sf1Inputs::idle();
        i.p1.punch = [true, true, false];
        i.p1.kick = [true, true, false];
        assert_eq!(i.in1(), 0xFF0F, "the four IN1 buttons");
        assert_eq!(i.in0(), 0xFFFF, "and none of them on IN0");
        i = Sf1Inputs::idle();
        i.p1.punch[2] = true;
        i.p1.kick[2] = true;
        assert_eq!(i.in0(), 0xFDFB, "fierce and roundhouse");
        assert_eq!(i.in1(), 0xFFFF, "and neither on IN1");
        i = Sf1Inputs::idle();
        i.coin1 = true;
        assert_eq!(i.system(), 0xFF7F, "a coin is not a start");
        i = Sf1Inputs::idle();
        i.start1 = true;
        assert_eq!(i.in0(), 0xFFFF, "a start is not a coin");
    }

    /// The DIP defaults are MAME's, and Demo Sounds defaults to **on**.
    ///
    /// `sf.cpp:509` is `PORT_DIPNAME(0x2000, 0x0000, Demo_Sounds)` with
    /// `PORT_DIPSETTING(0x0000, On)` — the one switch in either port whose default
    /// is the cleared state. Every other named switch and every
    /// `PORT_DIPUNUSED_DIPLOC` defaults set, and `PORT_SERVICE_DIPLOC` with
    /// `IP_ACTIVE_LOW` defaults set (service mode off).
    ///
    /// A plain `[0xFFFF; 2]` idle would boot with attract-mode music silent, which
    /// reads as a broken sound board rather than a DIP switch.
    #[test]
    fn the_dip_defaults_are_mames_and_demo_sounds_is_on() {
        let i = Sf1Inputs::idle();
        assert_eq!(i.dsw, [0xDFFF, 0xFFFF]);
        assert_eq!(i.dsw[0] & DSW1_DEMO_SOUNDS, 0, "demo sounds on");
        assert_ne!(i.dsw[0] & DSW1_FLIP_SCREEN, 0, "flip off");
        assert_ne!(i.dsw[0] & DSW1_SERVICE_MODE, 0, "service mode off");
    }

    /// The three named DIP masks are the driver's.
    #[test]
    fn the_named_dip_masks_are_the_drivers() {
        assert_eq!(DSW1_FLIP_SCREEN, 0x0100, "sf.cpp:498, DSW2.13E:1");
        assert_eq!(DSW1_DEMO_SOUNDS, 0x2000, "sf.cpp:509, DSW2.13E:6");
        assert_eq!(DSW1_SERVICE_MODE, 0x8000, "sf.cpp:515, DSW2.13E:8");
    }

    /// Everything pressed at once, for the unwired-bit tests.
    fn every_control_pressed() -> Sf1Inputs {
        let all = PlayerInput {
            right: true,
            left: true,
            down: true,
            up: true,
            punch: [true; 3],
            kick: [true; 3],
        };
        Sf1Inputs {
            coin1: true,
            coin2: true,
            service: true,
            start1: true,
            start2: true,
            p1: all,
            p2: all,
            ..Sf1Inputs::idle()
        }
    }
}
