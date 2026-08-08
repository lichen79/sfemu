//! Rotates, shifts, and bit operations.
//!
//! The four accumulator rotates here are **not** the `CB` page's versions of the
//! same operations. `RLCA` writes carry, H, N, F3 and F5, preserving S, Z and
//! P/V; `RLC A` on the `CB` page writes all eight. Both are one byte, both exist,
//! and the flag difference is the entire reason.

use crate::flags::{C, F3, F5, H, PV, S, Z};
use crate::Z80;

/// The flag pattern the four accumulator rotates share: carry from the rotated-out
/// bit, F3/F5 from the result, H and N cleared, S/Z/P-V preserved.
fn rot_a_flags(cpu: &mut Z80, result: u8, carry: bool) {
    cpu.f = (cpu.f & (S | Z | PV)) | (result & (F5 | F3)) | u8::from(carry);
    cpu.q = cpu.f;
}

/// `RLCA`: bit 7 to bit 0 and to carry.
pub fn rlca(cpu: &mut Z80) {
    let carry = cpu.a & 0x80 != 0;
    cpu.a = cpu.a.rotate_left(1);
    rot_a_flags(cpu, cpu.a, carry);
}

/// `RRCA`: bit 0 to bit 7 and to carry.
pub fn rrca(cpu: &mut Z80) {
    let carry = cpu.a & 0x01 != 0;
    cpu.a = cpu.a.rotate_right(1);
    rot_a_flags(cpu, cpu.a, carry);
}

/// `RLA`: a nine-bit rotate through carry.
pub fn rla(cpu: &mut Z80) {
    let carry_in = cpu.f & C;
    let carry_out = cpu.a & 0x80 != 0;
    cpu.a = (cpu.a << 1) | carry_in;
    rot_a_flags(cpu, cpu.a, carry_out);
}

/// `RRA`: [`rla`] mirrored.
pub fn rra(cpu: &mut Z80) {
    let carry_in = (cpu.f & C) << 7;
    let carry_out = cpu.a & 0x01 != 0;
    cpu.a = (cpu.a >> 1) | carry_in;
    rot_a_flags(cpu, cpu.a, carry_out);
}

/// The `CB` page's eight rotate and shift operations, in their encoded order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RotOp {
    Rlc,
    Rrc,
    Rl,
    Rr,
    Sla,
    Sra,
    /// Undocumented: a left shift that puts **1** in bit 0.
    ///
    /// Absent from the Zilog manual, present in every real Z80, and shipped as
    /// sixteen vector files. There is nothing to decide about it.
    Sll,
    Srl,
}

impl RotOp {
    /// Decodes bits 5–3 of a `CB`-page opcode.
    #[must_use]
    pub fn from_index(i: u8) -> Self {
        match i {
            0 => RotOp::Rlc,
            1 => RotOp::Rrc,
            2 => RotOp::Rl,
            3 => RotOp::Rr,
            4 => RotOp::Sla,
            5 => RotOp::Sra,
            6 => RotOp::Sll,
            7 => RotOp::Srl,
            _ => unreachable!("rotate index {i} is not three bits"),
        }
    }
}

/// Applies a `CB`-page rotate or shift, writing the full flag set.
///
/// All eight write S, Z, P/V (as parity), F3 and F5 from the result and clear H
/// and N — unlike the accumulator rotates above, which preserve S, Z and P/V.
/// Verified as one model across all sixteen register-and-`(HL)` files, 1,000 cases
/// each, result and flags together.
#[must_use]
pub fn rot(cpu: &mut Z80, which: RotOp, v: u8) -> u8 {
    let old_c = cpu.f & C;
    let (r, carry) = match which {
        RotOp::Rlc => (v.rotate_left(1), v & 0x80 != 0),
        RotOp::Rrc => (v.rotate_right(1), v & 0x01 != 0),
        RotOp::Rl => ((v << 1) | old_c, v & 0x80 != 0),
        RotOp::Rr => ((v >> 1) | (old_c << 7), v & 0x01 != 0),
        RotOp::Sla => (v << 1, v & 0x80 != 0),
        RotOp::Sra => ((v >> 1) | (v & 0x80), v & 0x01 != 0),
        RotOp::Sll => ((v << 1) | 1, v & 0x80 != 0),
        RotOp::Srl => (v >> 1, v & 0x01 != 0),
    };
    cpu.f = crate::flags::sz53p(r) | u8::from(carry);
    cpu.q = cpu.f;
    r
}

/// `BIT b,v`: sets Z when bit `b` of `v` is clear. `f35` supplies F3 and F5.
///
/// H is always set (the operation is an `AND` with a mask, and `AND` sets H), N is
/// cleared, **P/V is a copy of Z** rather than a parity, and S is set only by
/// `BIT 7` on a value whose bit 7 is set — because S is a copy of the result's bit
/// 7 and the result is `v & (1 << b)`.
///
/// F3 and F5 are a separate argument because their source is not the operand for
/// every form: the register forms take them from the operand, and `BIT b,(HL)`
/// takes them from the high byte of the address latch, which no instruction on
/// this page writes. See [`crate::decode::cb_page`].
pub fn bit_test(cpu: &mut Z80, b: u8, v: u8, f35: u8) {
    let masked = v & (1 << b);
    cpu.f =
        (cpu.f & C) | H | (f35 & (F5 | F3)) | if masked == 0 { Z | PV } else { 0 } | (masked & S);
    cpu.q = cpu.f;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::N;

    /// `RLCA` rotates left through carry and preserves S, Z and P/V.
    ///
    /// The distinction from `RLC A` on the `CB` page, which writes all of them.
    /// Both exist, both are one byte, and the reason to have both is exactly this
    /// flag difference.
    #[test]
    fn rlca_writes_only_carry_and_the_undocumented_bits() {
        let mut c = Z80::new();
        c.a = 0x88;
        c.f = S | Z | PV | H | N;
        rlca(&mut c);
        assert_eq!(c.a, 0x11, "0x88 rotated left is 0x11 with bit 7 into bit 0");
        assert_eq!(c.f & C, C, "and into carry");
        assert_eq!(c.f & (S | Z | PV), S | Z | PV, "S, Z and P/V are preserved");
        assert_eq!(c.f & (H | N), 0, "H and N are cleared");
        assert_eq!(c.f & (F5 | F3), 0x11 & (F5 | F3), "F5/F3 from the result");
    }

    /// `RRCA` rotates right, bit 0 into carry and into bit 7.
    #[test]
    fn rrca_moves_bit_zero_to_carry_and_to_bit_seven() {
        let mut c = Z80::new();
        c.a = 0x01;
        c.f = 0;
        rrca(&mut c);
        assert_eq!(c.a, 0x80);
        assert_eq!(c.f & C, C);
    }

    /// `RLA` rotates **through** carry: nine bits, not eight.
    ///
    /// The difference from `RLCA`, and the reason both exist: `RLA` is how a
    /// multi-byte shift is written, because the carry links the bytes.
    #[test]
    fn rla_rotates_through_the_carry_making_a_nine_bit_rotation() {
        let mut c = Z80::new();
        c.a = 0x80;
        c.f = C;
        rla(&mut c);
        assert_eq!(c.a, 0x01, "the old carry entered bit 0");
        assert_eq!(c.f & C, C, "and bit 7 became the new carry");

        c.a = 0x80;
        c.f = 0;
        rla(&mut c);
        assert_eq!(c.a, 0x00, "with no carry in, bit 0 is clear");
        assert_eq!(c.f & C, C);
    }

    /// `RRA` is `RLA`'s mirror.
    #[test]
    fn rra_rotates_right_through_the_carry() {
        let mut c = Z80::new();
        c.a = 0x01;
        c.f = C;
        rra(&mut c);
        assert_eq!(c.a, 0x80, "the old carry entered bit 7");
        assert_eq!(c.f & C, C, "and bit 0 became the new carry");
    }

    /// The `C` rotates wrap the bit round; the plain ones bring the carry in.
    ///
    /// The distinction each pair of tests above draws only at the extremes. With
    /// `A = 0x81` and carry clear, `RLCA` gives 0x03 (bit 7 wrapped to bit 0) while
    /// `RLA` gives 0x02 (the clear carry entered instead) — one bit apart, and the
    /// only assertion that separates a `rotate_left` from a shift-plus-carry.
    #[test]
    fn the_wrapping_rotates_differ_from_the_through_carry_ones_in_one_bit() {
        let mut c = Z80::new();
        c.a = 0x81;
        c.f = 0;
        rlca(&mut c);
        assert_eq!(c.a, 0x03, "RLCA wraps bit 7 into bit 0");

        let mut c = Z80::new();
        c.a = 0x81;
        c.f = 0;
        rla(&mut c);
        assert_eq!(c.a, 0x02, "RLA brings the carry in, and it was clear");

        let mut c = Z80::new();
        c.a = 0x81;
        c.f = 0;
        rrca(&mut c);
        assert_eq!(c.a, 0xC0, "RRCA wraps bit 0 into bit 7");

        let mut c = Z80::new();
        c.a = 0x81;
        c.f = 0;
        rra(&mut c);
        assert_eq!(c.a, 0x40, "RRA brings the clear carry into bit 7");
    }

    /// All four leave `Q` equal to the flags they wrote.
    ///
    /// They write flags, so `q` is `f` — and getting it wrong corrupts the *next*
    /// instruction's `SCF`, never the rotate's own result. `A` and the incoming
    /// flags are chosen so `f` ends as neither 0 nor 1.
    #[test]
    fn the_accumulator_rotates_leave_q_equal_to_the_flags() {
        type Rot = (&'static str, fn(&mut Z80));
        let rots: [Rot; 4] = [("RLCA", rlca), ("RRCA", rrca), ("RLA", rla), ("RRA", rra)];
        for (name, run) in rots {
            let mut c = Z80::new();
            c.a = 0x94;
            c.f = S | Z;
            c.q = 0;
            run(&mut c);
            assert_eq!(c.q, c.f, "{name} must leave Q equal to F");
            assert!(c.f > 1, "{name}: F is {:#04X}", c.f);
        }
    }

    /// The eight rotate and shift operations, each at the boundary that
    /// distinguishes it from its neighbours.
    ///
    /// `SLA` and `SLL` differ in exactly one bit of the result, and `SRA` and `SRL`
    /// in exactly one — so each pair is tested on a value where that bit shows.
    #[test]
    fn the_eight_rotates_and_shifts_differ_where_they_should() {
        let mut c = Z80::new();

        // RLC: bit 7 wraps to bit 0 and to carry.
        c.f = 0;
        assert_eq!(rot(&mut c, RotOp::Rlc, 0x85), 0x0B);
        assert_eq!(c.f & C, C);

        // RL: bit 7 to carry, the old carry to bit 0. Nine bits.
        c.f = C;
        assert_eq!(rot(&mut c, RotOp::Rl, 0x85), 0x0B, "0x85<<1 | old carry");
        c.f = 0;
        assert_eq!(rot(&mut c, RotOp::Rl, 0x85), 0x0A, "without the carry in");

        // RRC and RR, mirrored.
        c.f = 0;
        assert_eq!(rot(&mut c, RotOp::Rrc, 0x01), 0x80);
        assert_eq!(c.f & C, C);
        c.f = 0;
        assert_eq!(
            rot(&mut c, RotOp::Rr, 0x01),
            0x00,
            "no carry in, so bit 7 is 0"
        );
        assert_eq!(c.f & C, C, "and bit 0 went to carry");

        // SLA puts 0 in bit 0; SLL -- undocumented -- puts 1.
        c.f = 0;
        assert_eq!(rot(&mut c, RotOp::Sla, 0x80), 0x00);
        assert_eq!(c.f & (C | Z), C | Z);
        c.f = 0;
        assert_eq!(rot(&mut c, RotOp::Sll, 0x80), 0x01, "SLL's bit 0 is one");
        assert_eq!(c.f & Z, 0, "so the result is not zero");

        // SRA preserves the sign; SRL clears bit 7.
        c.f = 0;
        assert_eq!(rot(&mut c, RotOp::Sra, 0x85), 0xC2, "bit 7 is copied down");
        assert_eq!(c.f & (S | C), S | C);
        c.f = 0;
        assert_eq!(rot(&mut c, RotOp::Srl, 0x85), 0x42, "bit 7 becomes zero");
        assert_eq!(c.f & S, 0);
        assert_eq!(c.f & C, C);
    }

    /// All eight write S, Z, P/V, F3 and F5 from the result, and clear H and N.
    ///
    /// The contrast with the accumulator rotates, asserted rather than described:
    /// those preserve S, Z and P/V, and these do not. The incoming flags have every
    /// preserved-by-the-other-set bit on, so a handler that preserved them would be
    /// caught rather than merely failing to be exercised.
    #[test]
    fn every_cb_rotate_writes_the_full_flag_set() {
        let mut c = Z80::new();
        for op in [
            RotOp::Rlc,
            RotOp::Rrc,
            RotOp::Rl,
            RotOp::Rr,
            RotOp::Sla,
            RotOp::Sra,
            RotOp::Sll,
            RotOp::Srl,
        ] {
            c.f = S | Z | PV | H | N;
            let r = rot(&mut c, op, 0x02);
            assert_eq!(c.f & (H | N), 0, "{op:?} must clear H and N");
            assert_eq!(
                c.f & (S | Z | PV | F5 | F3),
                crate::flags::sz53p(r),
                "{op:?} must write S/Z/P-V/F5/F3 from the result {r:#04X}"
            );
            assert_eq!(c.q, c.f, "{op:?} wrote flags, so Q is F");
        }
    }

    /// `BIT b,r` sets Z when the bit is clear, and copies Z into P/V.
    ///
    /// H is always set and N always cleared — `BIT` is specified as an `AND` with a
    /// mask, and `AND` sets H. P/V is a copy of Z, which is the detail cores miss:
    /// every other P/V on the chip is a parity or an overflow.
    #[test]
    fn bit_tests_a_bit_and_copies_z_into_parity() {
        let mut c = Z80::new();
        c.f = 0;
        bit_test(&mut c, 3, 0x08, 0x08);
        assert_eq!(c.f & Z, 0, "bit 3 of 0x08 is set, so Z is clear");
        assert_eq!(c.f & PV, 0, "and P/V follows Z");
        assert_eq!(c.f & H, H, "H is always set");
        assert_eq!(c.f & N, 0);

        c.f = 0;
        bit_test(&mut c, 4, 0x08, 0x08);
        assert_eq!(c.f & Z, Z, "bit 4 is clear");
        assert_eq!(c.f & PV, PV, "and P/V is a copy of Z, not a parity");

        // Bit 7 set puts S in the flags -- the one bit position where BIT differs
        // from the others, because S is a copy of result bit 7 and the result of
        // `BIT 7,r` has bit 7 set exactly when the tested bit was.
        c.f = 0;
        bit_test(&mut c, 7, 0x80, 0x80);
        assert_eq!(c.f & S, S, "BIT 7 on a negative value sets S");
        c.f = 0;
        bit_test(&mut c, 6, 0xC0, 0xC0);
        assert_eq!(c.f & S, 0, "and BIT 6 does not, even on the same value");

        // Carry is the one flag BIT preserves.
        c.f = C;
        bit_test(&mut c, 0, 0x00, 0x00);
        assert_eq!(c.f & C, C, "carry survives");
    }

    /// `BIT`'s F3 and F5 come from `f35`, which is not always the operand.
    ///
    /// The register forms pass the operand and the `(HL)` form passes the latch's
    /// high byte, so the two arguments must be able to disagree. A `bit_test` that
    /// took F3/F5 from `v` would pass every register-form test and fail 76% of
    /// `cb_46.z80bin` — measured, because the plan claimed `H` was the source, and
    /// `H` was wrong on 760 of 1,000 cases.
    #[test]
    fn bit_takes_the_undocumented_flags_from_its_own_argument() {
        let mut c = Z80::new();
        c.f = 0;
        // The operand has neither F3 nor F5; the separate source has both.
        bit_test(&mut c, 0, 0x01, F5 | F3);
        assert_eq!(c.f & (F5 | F3), F5 | F3, "from f35, not from v");
        assert_eq!(c.f & Z, 0, "and the test itself still read bit 0 of v");

        // And the other way round, so "just copy v" and "just copy f35" differ.
        c.f = 0;
        bit_test(&mut c, 0, F5 | F3 | 0x01, 0x00);
        assert_eq!(c.f & (F5 | F3), 0);
    }
}
