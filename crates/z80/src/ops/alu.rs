//! The 8- and 16-bit arithmetic and logic, and the flags they produce.
//!
//! The flag rules are the substance here, not the arithmetic. Each function
//! documents which flags it touches and which it leaves, because "leaves" is a
//! specification too: `INC` not touching carry is what makes 16-bit loop counters
//! work on this chip.
//!
//! Every one of these writes the flags, so every one ends with `cpu.q = cpu.f` —
//! see the `Q` convention in [`crate::decode`]. `q` holds the flag *value*, not a
//! boolean.

use crate::flags::{self, C, F3, F5, H, N, PV, S, Z};
use crate::Z80;

/// `ADD A,v` or `ADC A,v`. All six flags are written.
pub fn add(cpu: &mut Z80, v: u8, with_carry: bool) {
    let a = cpu.a;
    let carry_in = u8::from(with_carry && cpu.f & C != 0);
    let sum = u16::from(a) + u16::from(v) + u16::from(carry_in);
    let r = sum as u8;
    // H is the carry out of bit 3, computed on the nibbles alone.
    let h = (a & 0x0F) + (v & 0x0F) + carry_in > 0x0F;
    // Signed overflow: both operands the same sign, result the other.
    let ovf = (a ^ v) & 0x80 == 0 && (a ^ r) & 0x80 != 0;
    cpu.a = r;
    cpu.f = flags::sz53(r)
        | if h { H } else { 0 }
        | if ovf { PV } else { 0 }
        | if sum > 0xFF { C } else { 0 };
    cpu.q = cpu.f;
}

/// `SUB v` or `SBC A,v`. All six flags; N is always set.
pub fn sub(cpu: &mut Z80, v: u8, with_carry: bool) {
    let a = cpu.a;
    let borrow_in = u8::from(with_carry && cpu.f & C != 0);
    let diff = i32::from(a) - i32::from(v) - i32::from(borrow_in);
    let r = diff as u8;
    let h = i32::from(a & 0x0F) - i32::from(v & 0x0F) - i32::from(borrow_in) < 0;
    // Signed overflow on subtraction: operands differ in sign, result matches the
    // subtrahend's.
    let ovf = (a ^ v) & 0x80 != 0 && (a ^ r) & 0x80 != 0;
    cpu.a = r;
    cpu.f = flags::sz53(r)
        | N
        | if h { H } else { 0 }
        | if ovf { PV } else { 0 }
        | if diff < 0 { C } else { 0 };
    cpu.q = cpu.f;
}

/// `CP v`: the flags of `SUB v`, but `A` is unchanged **and F3/F5 come from `v`**.
///
/// The deviation is real hardware behaviour, not a simplification: `CP` puts the
/// operand on the internal bus where the other operations put the result, and the
/// undocumented bits are a copy of whatever is there.
pub fn cp(cpu: &mut Z80, v: u8) {
    let a = cpu.a;
    sub(cpu, v, false);
    cpu.a = a;
    cpu.f = (cpu.f & !(F5 | F3)) | (v & (F5 | F3));
    // `sub` already set `q`, but to the flags *before* the F3/F5 substitution. `q`
    // is the value of `F` as the instruction left it, so it has to be re-taken.
    cpu.q = cpu.f;
}

/// `AND v`: parity in P/V, H **set**, carry cleared.
pub fn and(cpu: &mut Z80, v: u8) {
    cpu.a &= v;
    cpu.f = flags::sz53p(cpu.a) | H;
    cpu.q = cpu.f;
}

/// `OR v`: parity in P/V, H and carry cleared.
pub fn or(cpu: &mut Z80, v: u8) {
    cpu.a |= v;
    cpu.f = flags::sz53p(cpu.a);
    cpu.q = cpu.f;
}

/// `XOR v`: as [`or`].
pub fn xor(cpu: &mut Z80, v: u8) {
    cpu.a ^= v;
    cpu.f = flags::sz53p(cpu.a);
    cpu.q = cpu.f;
}

/// `INC v`. Carry is **preserved**; P/V is signed overflow.
#[must_use]
pub fn inc8(cpu: &mut Z80, v: u8) -> u8 {
    let r = v.wrapping_add(1);
    cpu.f = (cpu.f & C)
        | flags::sz53(r)
        | if v & 0x0F == 0x0F { H } else { 0 }
        | if v == 0x7F { PV } else { 0 };
    cpu.q = cpu.f;
    r
}

/// `DEC v`. Carry preserved, N set, P/V signed overflow.
#[must_use]
pub fn dec8(cpu: &mut Z80, v: u8) -> u8 {
    let r = v.wrapping_sub(1);
    cpu.f = (cpu.f & C)
        | N
        | flags::sz53(r)
        | if v & 0x0F == 0x00 { H } else { 0 }
        | if v == 0x80 { PV } else { 0 };
    cpu.q = cpu.f;
    r
}

/// `ADD HL,rr` and the indexed forms.
///
/// Writes H, C, N and the two undocumented bits, and **preserves S, Z and P/V**.
/// The preservation is documented behaviour, not an omission: the 16-bit add's
/// sign and zero are simply not recorded, and F3/F5 come from the high byte of the
/// result rather than from the whole thing.
#[must_use]
pub fn add16(cpu: &mut Z80, a: u16, b: u16) -> u16 {
    let sum = u32::from(a) + u32::from(b);
    let r = sum as u16;
    let h = (a & 0x0FFF) + (b & 0x0FFF) > 0x0FFF;
    cpu.f = (cpu.f & (S | Z | PV))
        | (((r >> 8) as u8) & (F5 | F3))
        | if h { H } else { 0 }
        | if sum > 0xFFFF { C } else { 0 };
    cpu.q = cpu.f;
    r
}

/// `ADC HL,rr`: the full flag set, unlike [`add16`].
///
/// S, Z and P/V are all written here and all *preserved* by `ADD HL,rr` — the same
/// operation on the same data with different flags, distinguished only by which page
/// the opcode sits on. H is still the carry out of bit 11 and F3/F5 still come from
/// the high byte of the result.
#[must_use]
pub fn adc16(cpu: &mut Z80, a: u16, b: u16) -> u16 {
    let carry_in = u32::from(cpu.f & C != 0);
    let sum = u32::from(a) + u32::from(b) + carry_in;
    let r = sum as u16;
    let h = (a & 0x0FFF) + (b & 0x0FFF) + carry_in as u16 > 0x0FFF;
    let ovf = (a ^ b) & 0x8000 == 0 && (a ^ r) & 0x8000 != 0;
    cpu.f = (((r >> 8) as u8) & (S | F5 | F3))
        | if r == 0 { Z } else { 0 }
        | if h { H } else { 0 }
        | if ovf { PV } else { 0 }
        | if sum > 0xFFFF { C } else { 0 };
    cpu.q = cpu.f;
    r
}

/// `SBC HL,rr`: as [`adc16`], with N set and the borrow rules.
#[must_use]
pub fn sbc16(cpu: &mut Z80, a: u16, b: u16) -> u16 {
    let borrow = i32::from(cpu.f & C != 0);
    let diff = i32::from(a) - i32::from(b) - borrow;
    let r = diff as u16;
    let h = i32::from(a & 0x0FFF) - i32::from(b & 0x0FFF) - borrow < 0;
    let ovf = (a ^ b) & 0x8000 != 0 && (a ^ r) & 0x8000 != 0;
    cpu.f = (((r >> 8) as u8) & (S | F5 | F3))
        | N
        | if r == 0 { Z } else { 0 }
        | if h { H } else { 0 }
        | if ovf { PV } else { 0 }
        | if diff < 0 { C } else { 0 };
    cpu.q = cpu.f;
    r
}

/// `NEG`: `0 - A`, with subtraction flags.
///
/// P/V is set for `A = 0x80` and no other value: −128 has no positive counterpart in
/// eight bits. A single-value condition is the kind a core hard-codes to `false` and
/// never notices.
///
/// Written as `sub(0, A)` rather than as its own flag expression, because that is
/// exactly what it is — measured 0 wrong over 1,000 cases on each of the eight
/// `NEG` encodings, which is the strongest available evidence that the two share
/// one rule and should share one implementation.
pub fn neg(cpu: &mut Z80) {
    let a = cpu.a;
    cpu.a = 0;
    sub(cpu, a, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ADD` sets H from a carry out of bit 3 and P/V from signed overflow.
    ///
    /// The two flags that distinguish an adder from an XOR. Values chosen so each
    /// fires alone: 0x0F + 0x01 half-carries without overflowing, and 0x7F + 0x01
    /// overflows without carrying out of bit 7.
    #[test]
    fn add_reports_half_carry_and_signed_overflow_separately() {
        let mut c = Z80::new();
        c.a = 0x0F;
        add(&mut c, 0x01, false);
        assert_eq!(c.a, 0x10);
        assert_eq!(c.f & H, H, "a carry out of bit 3");
        assert_eq!(c.f & PV, 0, "16 is not a signed overflow");
        assert_eq!(c.f & C, 0);

        c.a = 0x7F;
        c.f = 0;
        add(&mut c, 0x01, false);
        assert_eq!(c.a, 0x80);
        assert_eq!(c.f & PV, PV, "127 + 1 overflows into the sign");
        assert_eq!(c.f & S, S);
        assert_eq!(c.f & C, 0, "and does not carry out of bit 7");

        c.a = 0xFF;
        c.f = 0;
        add(&mut c, 0x01, false);
        assert_eq!(c.a, 0x00);
        assert_eq!(c.f & (C | Z | H), C | Z | H);
        assert_eq!(c.f & PV, 0, "-1 + 1 is not a signed overflow");
        assert_eq!(c.f & N, 0, "ADD always clears N");
    }

    /// `ADC` adds the carry in, and the flags describe the whole sum.
    #[test]
    fn adc_includes_the_incoming_carry() {
        let mut c = Z80::new();
        c.a = 0x0F;
        c.f = C;
        add(&mut c, 0x00, true);
        assert_eq!(c.a, 0x10, "0x0F + 0 + 1");
        assert_eq!(c.f & H, H, "and the half-carry comes from the carry in");
    }

    /// The carry only enters when the caller asked for it.
    ///
    /// `add(.., false)` with C set is `ADD`, not `ADC`, and the two share a body —
    /// so a `with_carry` that were ignored would make every `ADD` after a carrying
    /// operation off by one. The test above cannot see that: it passes `true`.
    #[test]
    fn add_ignores_the_carry_flag_when_it_is_not_adc() {
        let mut c = Z80::new();
        c.a = 0x0F;
        c.f = C;
        add(&mut c, 0x00, false);
        assert_eq!(c.a, 0x0F, "plain ADD does not add the carry");
        assert_eq!(c.f & (H | C), 0, "and nothing carried anywhere");

        let mut c = Z80::new();
        c.a = 0x10;
        c.f = C;
        sub(&mut c, 0x01, false);
        assert_eq!(c.a, 0x0F, "plain SUB does not subtract the carry either");
    }

    /// `SBC` subtracts the borrow in.
    #[test]
    fn sbc_includes_the_incoming_borrow() {
        let mut c = Z80::new();
        c.a = 0x10;
        c.f = C;
        sub(&mut c, 0x01, true);
        assert_eq!(c.a, 0x0E, "0x10 - 1 - 1");
        assert_eq!(c.f & H, H, "and the borrow crossed bit 4");
    }

    /// `SUB` sets N, and its H means a borrow from bit 4.
    #[test]
    fn sub_sets_n_and_borrow_flags() {
        let mut c = Z80::new();
        c.a = 0x10;
        sub(&mut c, 0x01, false);
        assert_eq!(c.a, 0x0F);
        assert_eq!(c.f & N, N, "SUB always sets N");
        assert_eq!(c.f & H, H, "a borrow from bit 4");
        assert_eq!(c.f & C, 0, "no borrow out");

        c.a = 0x00;
        c.f = 0;
        sub(&mut c, 0x01, false);
        assert_eq!(c.a, 0xFF);
        assert_eq!(c.f & C, C, "0 - 1 borrows out");
        assert_eq!(c.f & S, S);

        c.a = 0x80;
        c.f = 0;
        sub(&mut c, 0x01, false);
        assert_eq!(c.a, 0x7F);
        assert_eq!(c.f & PV, PV, "-128 - 1 overflows");
    }

    /// `CP` sets the flags of a subtraction and leaves `A` alone.
    ///
    /// And its F3/F5 come from the **operand**, not the result — the one place the
    /// undocumented bits deviate from the pattern, and a real vector failure if
    /// missed.
    #[test]
    fn cp_flags_a_subtraction_without_writing_a() {
        let mut c = Z80::new();
        c.a = 0x10;
        cp(&mut c, 0x28);
        assert_eq!(c.a, 0x10, "A is untouched");
        assert_eq!(c.f & C, C, "0x10 < 0x28 borrows");
        assert_eq!(c.f & N, N);
        assert_eq!(
            c.f & (F5 | F3),
            F5 | F3,
            "F5/F3 come from the operand 0x28, not the result 0xE8"
        );
        assert_eq!(c.q, c.f, "and Q is the flags as CP left them");
    }

    /// `CP`'s F3/F5 are a copy of the operand's, not a set of them.
    ///
    /// The case above cannot tell "copy bits 5 and 3 of `v`" from "set both
    /// whenever `v` has either": 0x28 has both. An operand with neither must give
    /// neither, even when the *result* has both.
    #[test]
    fn cp_takes_f3_and_f5_from_the_operand_even_when_the_result_has_them() {
        let mut c = Z80::new();
        c.a = 0x28;
        cp(&mut c, 0x00);
        assert_eq!(c.a, 0x28);
        assert_eq!(c.f & Z, 0, "0x28 - 0 is not zero");
        assert_eq!(
            c.f & (F5 | F3),
            0,
            "the operand is 0x00, so neither bit, though the result 0x28 has both"
        );
    }

    /// `AND`, `OR`, `XOR`: parity in P/V, and H set only by `AND`.
    #[test]
    fn the_logical_operations_use_parity_and_differ_only_in_h() {
        let mut c = Z80::new();
        c.a = 0x0F;
        and(&mut c, 0x03);
        assert_eq!(c.a, 0x03);
        assert_eq!(c.f & H, H, "AND sets H, alone among the three");
        assert_eq!(c.f & PV, PV, "two bits set is even parity");
        assert_eq!(c.f & (C | N), 0);

        c.a = 0x0F;
        c.f = C;
        or(&mut c, 0x30);
        assert_eq!(c.a, 0x3F);
        assert_eq!(c.f & H, 0, "OR clears H");
        assert_eq!(c.f & C, 0, "and clears carry");
        assert_eq!(c.f & PV, PV, "six bits");

        c.a = 0xFF;
        c.f = 0;
        xor(&mut c, 0xFF);
        assert_eq!(c.a, 0x00);
        assert_eq!(c.f & (Z | PV), Z | PV);
        assert_eq!(c.f & H, 0);
    }

    /// `INC` and `DEC` leave carry alone. That is their whole point.
    ///
    /// A 16-bit counter is incremented as two 8-bit halves using the carry from a
    /// separate compare, so an `INC` that touched carry would break every loop
    /// ever written for this chip.
    #[test]
    fn inc_and_dec_preserve_the_carry_flag() {
        let mut c = Z80::new();
        c.f = C;
        assert_eq!(inc8(&mut c, 0xFF), 0x00);
        assert_eq!(c.f & C, C, "INC must not touch carry");
        assert_eq!(c.f & (Z | H), Z | H, "0xFF + 1 zeroes and half-carries");
        assert_eq!(c.f & N, 0);

        c.f = C;
        assert_eq!(dec8(&mut c, 0x00), 0xFF);
        assert_eq!(c.f & C, C, "nor must DEC");
        assert_eq!(c.f & (S | H | N), S | H | N);

        c.f = 0;
        assert_eq!(inc8(&mut c, 0x7F), 0x80);
        assert_eq!(c.f & PV, PV, "127 + 1 is a signed overflow");
        c.f = 0;
        assert_eq!(dec8(&mut c, 0x80), 0x7F);
        assert_eq!(c.f & PV, PV, "-128 - 1 too");
    }

    /// 16-bit `ADD HL,rr` touches H, C, F3 and F5 — and **not** S, Z or P/V.
    ///
    /// The asymmetry is real and it is the reason this is not `add` twice: H comes
    /// from bit 11, and the sign and zero of the 16-bit result are not recorded at
    /// all. F3/F5 come from the **high byte** of the result.
    #[test]
    fn add16_reports_a_carry_from_bit_eleven_and_leaves_sign_and_zero() {
        let mut c = Z80::new();
        c.f = S | Z | PV;
        let r = add16(&mut c, 0x0FFF, 0x0001);
        assert_eq!(r, 0x1000);
        assert_eq!(c.f & H, H, "a carry out of bit 11");
        assert_eq!(c.f & C, 0);
        assert_eq!(c.f & (S | Z | PV), S | Z | PV, "S, Z and P/V are preserved");
        assert_eq!(c.f & N, 0, "N is cleared");

        c.f = 0;
        let r = add16(&mut c, 0xFFFF, 0x0001);
        assert_eq!(r, 0x0000);
        assert_eq!(c.f & C, C, "and a carry out of bit 15");
        assert_eq!(c.f & Z, 0, "which does not set Z, even at zero");

        c.f = 0;
        let r = add16(&mut c, 0x2000, 0x0800);
        assert_eq!(r, 0x2800);
        assert_eq!(c.f & (F5 | F3), F5 | F3, "F5/F3 from the high byte 0x28");
    }

    /// 16-bit `ADC HL,rr` writes sign, zero and overflow where `ADD HL,rr` preserves
    /// them.
    ///
    /// Two instructions, one operation, different flags — and `ADD` is on the base
    /// page while `ADC` is on `ED`, which is the only clue in the encoding. A core
    /// that routed both through [`add16`] would pass every `09`-family file and fail
    /// every `ed_4a`-family one.
    #[test]
    fn adc16_writes_sign_zero_and_overflow_where_add16_preserves_them() {
        let mut c = Z80::new();
        c.f = 0;
        let r = adc16(&mut c, 0x0000, 0x0000);
        assert_eq!(r, 0);
        assert_eq!(c.f & Z, Z, "ADC HL,rr sets Z -- ADD HL,rr never does");
        assert_eq!(c.f & N, 0);

        c.f = 0;
        let r = adc16(&mut c, 0x7FFF, 0x0001);
        assert_eq!(r, 0x8000);
        assert_eq!(c.f & PV, PV, "and it reports signed overflow");
        assert_eq!(c.f & S, S);
        assert_eq!(c.f & H, H, "0x7FF + 1 carries out of bit 11");

        c.f = C;
        let r = adc16(&mut c, 0x0000, 0x0000);
        assert_eq!(r, 1, "and it adds the carry in");
        assert_eq!(c.f & Z, 0, "which is what stops the result being zero");
    }

    /// `SBC HL,rr` sets N and borrows.
    #[test]
    fn sbc16_sets_n_and_reports_a_borrow() {
        let mut c = Z80::new();
        c.f = 0;
        let r = sbc16(&mut c, 0x0000, 0x0001);
        assert_eq!(r, 0xFFFF);
        assert_eq!(c.f & (N | C | S), N | C | S);

        c.f = 0;
        let r = sbc16(&mut c, 0x8000, 0x0001);
        assert_eq!(r, 0x7FFF);
        assert_eq!(c.f & PV, PV, "-32768 - 1 overflows");

        c.f = C;
        let r = sbc16(&mut c, 0x0002, 0x0001);
        assert_eq!(r, 0x0000, "and the borrow comes in");
        assert_eq!(c.f & Z, Z);
    }

    /// `NEG` is `0 - A`, and overflows only at 0x80.
    ///
    /// 0x80 is the one value with no positive counterpart in eight bits, so it is the
    /// only input that sets P/V — a single-value condition, which is exactly the kind
    /// a core implements as `false` and never notices.
    #[test]
    fn neg_overflows_only_at_the_most_negative_value() {
        let mut c = Z80::new();
        c.a = 0x80;
        c.f = 0;
        neg(&mut c);
        assert_eq!(c.a, 0x80, "negating -128 gives -128");
        assert_eq!(c.f & PV, PV, "which is the overflow");
        assert_eq!(c.f & (N | C | S), N | C | S);

        c.a = 0x01;
        c.f = 0;
        neg(&mut c);
        assert_eq!(c.a, 0xFF);
        assert_eq!(c.f & PV, 0, "and no other value does");
        assert_eq!(c.f & C, C, "a non-zero A borrows");

        c.a = 0x00;
        c.f = 0;
        neg(&mut c);
        assert_eq!(c.a, 0x00);
        assert_eq!(c.f & Z, Z);
        assert_eq!(c.f & C, 0, "zero is the only A that does not borrow");
        assert_eq!(c.f & N, N, "NEG is a subtraction whatever the operand");
    }

    /// Every operation here leaves `Q` equal to the flags it wrote.
    ///
    /// The `SCF`/`CCF` rule reads `f & !q`, so a handler that set `q` to `1` — or
    /// forgot it — corrupts the *next* instruction rather than its own, which is
    /// why no per-operation test above can catch it. Values are chosen so `f` is
    /// never `0` or `1`, the two constants a wrong `q` would coincide with.
    #[test]
    fn every_operation_leaves_q_equal_to_the_flags_it_wrote() {
        type Op = (&'static str, fn(&mut Z80));
        let ops: [Op; 14] = [
            ("ADD", |c| add(c, 0x28, false)),
            ("ADC", |c| add(c, 0x28, true)),
            ("SUB", |c| sub(c, 0x28, false)),
            ("SBC", |c| sub(c, 0x28, true)),
            ("AND", |c| and(c, 0x2C)),
            ("OR", |c| or(c, 0x28)),
            ("XOR", |c| xor(c, 0x0F)),
            ("CP", |c| cp(c, 0x28)),
            ("INC", |c| c.a = inc8(c, 0x2F)),
            ("DEC", |c| c.a = dec8(c, 0x30)),
            ("ADD16", |c| {
                let r = add16(c, 0x2000, 0x0800);
                c.set_hl(r);
            }),
            ("ADC16", |c| {
                let r = adc16(c, 0x2000, 0x0800);
                c.set_hl(r);
            }),
            ("SBC16", |c| {
                let r = sbc16(c, 0x3000, 0x0800);
                c.set_hl(r);
            }),
            ("NEG", neg),
        ];
        for (name, run) in ops {
            let mut c = Z80::new();
            c.a = 0xB7;
            c.f = C;
            c.q = 0;
            run(&mut c);
            assert_eq!(c.q, c.f, "{name} must leave Q equal to F");
            assert!(
                c.f > 1,
                "{name}: F is {:#04X}, which would coincide with a Q of 0 or 1",
                c.f
            );
        }
    }
}
