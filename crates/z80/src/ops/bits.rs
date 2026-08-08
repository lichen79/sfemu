//! Rotates, shifts, and bit operations.
//!
//! The four accumulator rotates here are **not** the `CB` page's versions of the
//! same operations. `RLCA` writes carry, H, N, F3 and F5, preserving S, Z and
//! P/V; `RLC A` on the `CB` page writes all eight. Both are one byte, both exist,
//! and the flag difference is the entire reason.

use crate::flags::{C, F3, F5, PV, S, Z};
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::{H, N};

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
}
