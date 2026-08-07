//! Controls and DIP switches, as the 68000 sees them.
//!
//! # Active low
//!
//! Every bit is **active low**: 1 is released, 0 is pressed
//! (`IP_ACTIVE_LOW` throughout `cps1.cpp:830-942`). An idle board therefore reads
//! 0xFF across the whole port block. A model that returns 0 for "nothing pressed"
//! boots with every button held, which looks like a game bug rather than a bus
//! bug and costs a day to find.
//!
//! Bit assignments cited to MAME `master`, `src/mame/capcom/cps1.cpp`:
//! `IN0` at 830-838, `IN1` at 840-856, `IN2` at 934-943.

/// Button, stick, and coin state.
///
/// Set fields to `true` for **pressed**; the conversion to active-low happens in
/// this module, once, so no caller has to remember the polarity.
#[derive(Debug, Clone, Copy)]
pub struct Inputs {
    /// Coin slot 1 (`IN0` bit 0).
    pub coin1: bool,
    /// Coin slot 2 (`IN0` bit 1).
    pub coin2: bool,
    /// Service coin (`IN0` bit 2).
    pub service: bool,
    /// Player 1 start (`IN0` bit 4).
    pub start1: bool,
    /// Player 2 start (`IN0` bit 5).
    pub start2: bool,
    /// The test switch (`IN0` bit 6, `PORT_SERVICE` at `cps1.cpp:837`). Holding
    /// this at boot enters the service menu.
    pub test: bool,
    /// Player 1's stick and buttons.
    pub p1: PlayerInput,
    /// Player 2's stick and buttons.
    pub p2: PlayerInput,
    /// DSWA, DSWB, DSWC. All-ones means every switch off, which is what
    /// [`Inputs::idle`] gives.
    pub dsw: [u8; 3],
}

impl Default for Inputs {
    /// [`Inputs::idle`] — **not** all-zero.
    ///
    /// `#[derive(Default)]` would give `dsw: [0; 3]`, which reads as every DIP
    /// switch on. Writing this out is what keeps `Inputs::default()` from being a
    /// differently-configured board than `Inputs::idle()`.
    fn default() -> Self {
        Self::idle()
    }
}

/// One player's stick and six attack buttons.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerInput {
    /// Stick right.
    pub right: bool,
    /// Stick left.
    pub left: bool,
    /// Stick down.
    pub down: bool,
    /// Stick up.
    pub up: bool,
    /// Jab, strong, fierce — `IN1` bits 4-6 (`BUTTON1`-`BUTTON3`).
    pub punch: [bool; 3],
    /// Short, forward, roundhouse — `IN2` bits 0-2 (`BUTTON4`-`BUTTON6`), read
    /// through CPS-B rather than the port block.
    pub kick: [bool; 3],
}

impl Inputs {
    /// A board with nothing pressed and every DIP switch off.
    pub const fn idle() -> Self {
        Self {
            coin1: false,
            coin2: false,
            service: false,
            start1: false,
            start2: false,
            test: false,
            p1: PlayerInput::none(),
            p2: PlayerInput::none(),
            dsw: [0xFF; 3],
        }
    }

    /// `IN0` — coins, starts, service, test. `cps1.cpp:830-838`.
    ///
    /// Bit 3 and bit 7 are `IPT_UNKNOWN`: not wired, so they read as released.
    pub fn in0(&self) -> u8 {
        active_low(&[
            (0, self.coin1),
            (1, self.coin2),
            (2, self.service),
            (4, self.start1),
            (5, self.start2),
            (6, self.test),
        ])
    }

    /// `IN1` — both sticks and three punches each. `cps1.cpp:840-856`: P1 in the
    /// low byte, P2 in the high.
    pub fn in1(&self) -> u16 {
        let lo = self.p1.stick_and_punch();
        let hi = self.p2.stick_and_punch();
        (u16::from(hi) << 8) | u16::from(lo)
    }

    /// `IN2` — the six kick buttons, read through CPS-B at `in2_addr`.
    /// `cps1.cpp:934-943`: P1 in bits 0-2, P2 in bits 4-6.
    ///
    /// Bits 3 and 7 are `IPT_UNKNOWN`, which is why P2 starts at bit 4 rather
    /// than bit 3.
    pub fn in2(&self) -> u8 {
        active_low(&[
            (0, self.p1.kick[0]),
            (1, self.p1.kick[1]),
            (2, self.p1.kick[2]),
            (4, self.p2.kick[0]),
            (5, self.p2.kick[1]),
            (6, self.p2.kick[2]),
        ])
    }
}

impl PlayerInput {
    /// Nothing pressed.
    pub const fn none() -> Self {
        Self {
            right: false,
            left: false,
            down: false,
            up: false,
            punch: [false; 3],
            kick: [false; 3],
        }
    }

    /// This player's half of `IN1`: stick in bits 0-3, punches in bits 4-6.
    fn stick_and_punch(&self) -> u8 {
        active_low(&[
            (0, self.right),
            (1, self.left),
            (2, self.down),
            (3, self.up),
            (4, self.punch[0]),
            (5, self.punch[1]),
            (6, self.punch[2]),
        ])
    }
}

/// Starts from all-released and clears one bit per pressed control.
///
/// The polarity lives here alone. Writing `|= 1 << bit` instead — the mutant
/// worth watching — inverts every control at once.
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

    #[test]
    fn an_idle_board_is_all_ones_everywhere() {
        let i = Inputs::idle();
        assert_eq!(i.in0(), 0xFF);
        assert_eq!(i.in1(), 0xFFFF);
        assert_eq!(i.in2(), 0xFF);
        assert_eq!(i.dsw, [0xFF; 3], "every DIP switch off");
    }

    /// `default()` must be the same board as `idle()`.
    ///
    /// A derived `Default` would give `dsw: [0x00; 3]` — every switch on — so a
    /// caller using `Inputs::default()` would silently run a differently
    /// configured machine.
    #[test]
    fn default_is_idle_and_not_all_zero() {
        let d = Inputs::default();
        assert_eq!(d.dsw, [0xFF; 3]);
        assert_eq!(d.in0(), 0xFF);
        assert_eq!(d.in1(), 0xFFFF);
        assert_eq!(d.in2(), 0xFF);
    }

    /// Each `IN0` control clears exactly its own documented bit.
    ///
    /// Expected values are literals: a loop deriving `!(1 << bit)` from the same
    /// table the implementation uses would pass with every bit wrong together.
    #[test]
    fn each_in0_control_clears_its_own_bit() {
        let mut i = Inputs::idle();
        i.coin1 = true;
        assert_eq!(i.in0(), 0xFE);
        i = Inputs::idle();
        i.coin2 = true;
        assert_eq!(i.in0(), 0xFD);
        i = Inputs::idle();
        i.service = true;
        assert_eq!(i.in0(), 0xFB);
        i = Inputs::idle();
        i.start1 = true;
        assert_eq!(i.in0(), 0xEF);
        i = Inputs::idle();
        i.start2 = true;
        assert_eq!(i.in0(), 0xDF);
        i = Inputs::idle();
        i.test = true;
        assert_eq!(i.in0(), 0xBF);
    }

    #[test]
    fn in0_bits_3_and_7_are_not_wired_to_anything() {
        // Every control at once still leaves the two IPT_UNKNOWN bits set
        // (cps1.cpp:834, 838).
        let i = Inputs {
            coin1: true,
            coin2: true,
            service: true,
            start1: true,
            start2: true,
            test: true,
            ..Inputs::idle()
        };
        assert_eq!(i.in0(), 0x88, "only bits 3 and 7 remain set");
    }

    #[test]
    fn each_in1_control_clears_its_own_bit_with_p1_low_and_p2_high() {
        let mut i = Inputs::idle();
        i.p1.right = true;
        assert_eq!(i.in1(), 0xFFFE);
        i = Inputs::idle();
        i.p1.left = true;
        assert_eq!(i.in1(), 0xFFFD);
        i = Inputs::idle();
        i.p1.down = true;
        assert_eq!(i.in1(), 0xFFFB);
        i = Inputs::idle();
        i.p1.up = true;
        assert_eq!(i.in1(), 0xFFF7);
        i = Inputs::idle();
        i.p1.punch = [true, false, false];
        assert_eq!(i.in1(), 0xFFEF, "jab");
        i = Inputs::idle();
        i.p1.punch = [false, true, false];
        assert_eq!(i.in1(), 0xFFDF, "strong");
        i = Inputs::idle();
        i.p1.punch = [false, false, true];
        assert_eq!(i.in1(), 0xFFBF, "fierce");
        i = Inputs::idle();
        i.p2.right = true;
        assert_eq!(i.in1(), 0xFEFF, "P2 is the high byte");
        i = Inputs::idle();
        i.p2.punch = [false, false, true];
        assert_eq!(i.in1(), 0xBFFF, "P2 fierce");
    }

    #[test]
    fn in1_bits_7_and_15_are_not_wired_to_anything() {
        let mut i = Inputs::idle();
        for p in [&mut i.p1, &mut i.p2] {
            p.right = true;
            p.left = true;
            p.down = true;
            p.up = true;
            p.punch = [true; 3];
        }
        assert_eq!(i.in1(), 0x8080, "cps1.cpp:848 and 856");
    }

    #[test]
    fn each_kick_clears_its_own_in2_bit_with_a_gap_at_bit_3() {
        let mut i = Inputs::idle();
        i.p1.kick = [true, false, false];
        assert_eq!(i.in2(), 0xFE, "P1 short");
        i = Inputs::idle();
        i.p1.kick = [false, true, false];
        assert_eq!(i.in2(), 0xFD, "P1 forward");
        i = Inputs::idle();
        i.p1.kick = [false, false, true];
        assert_eq!(i.in2(), 0xFB, "P1 roundhouse");
        i = Inputs::idle();
        i.p2.kick = [true, false, false];
        assert_eq!(i.in2(), 0xEF, "P2 short — bit 4, not bit 3");
        i = Inputs::idle();
        i.p2.kick = [false, true, false];
        assert_eq!(i.in2(), 0xDF, "P2 forward");
        i = Inputs::idle();
        i.p2.kick = [false, false, true];
        assert_eq!(i.in2(), 0xBF, "P2 roundhouse");
        i = Inputs::idle();
        i.p1.kick = [true; 3];
        i.p2.kick = [true; 3];
        assert_eq!(i.in2(), 0x88, "bits 3 and 7 are IPT_UNKNOWN");
    }

    /// Punches and kicks are different ports.
    ///
    /// P1's jab is `IN1` bit 4 and P1's short kick is `IN2` bit 0. A model that
    /// merged the two six-button sets into one port would still pass each port's
    /// own test in isolation.
    #[test]
    fn punches_and_kicks_do_not_leak_between_in1_and_in2() {
        let mut i = Inputs::idle();
        i.p1.punch = [true; 3];
        assert_eq!(i.in1(), 0xFF8F);
        assert_eq!(i.in2(), 0xFF, "a punch does not appear in IN2");
        i = Inputs::idle();
        i.p1.kick = [true; 3];
        assert_eq!(i.in2(), 0xF8);
        assert_eq!(i.in1(), 0xFFFF, "a kick does not appear in IN1");
    }

    /// The polarity is a single function, and this is the test that pins it.
    #[test]
    fn active_low_clears_rather_than_sets() {
        assert_eq!(active_low(&[]), 0xFF, "nothing pressed");
        assert_eq!(active_low(&[(0, true)]), 0xFE);
        assert_eq!(active_low(&[(7, true)]), 0x7F);
        assert_eq!(active_low(&[(0, false)]), 0xFF, "released changes nothing");
        assert_eq!(active_low(&[(0, true), (1, true)]), 0xFC);
    }
}
