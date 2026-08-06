//! Condition-code computation.
//!
//! These rules are the single most common source of subtle emulator bugs, so
//! each is unit-tested with hand-computed expectations in addition to being
//! covered by the vector suite.
//!
//! # The three rules that are not obvious
//!
//! Each was scored on the subset of cases where it and a deliberately-wrong
//! alternative predict *different* CCRs, because a rule scored over all cases
//! can be right by accident (task-6-addendum §3).
//!
//! 1. **`ADDX`/`SUBX`/`NEGX` accumulate `Z`**: `Z_final = (result == 0) && Z_initial`.
//!    They can only ever *clear* `Z`, never set it — that is what makes a chain
//!    of `ADDX`s report "every limb was zero". [`accumulate_z`] applies it.
//!    146 disagreeing cases, 146/146 for the accumulating rule and 0/146 for
//!    the own-result rule, with a control group: `ADD.b` 2, `SUB.l` 10,
//!    `AND.b` 110 and `NEG.b` 9 cases go `Z=0 -> Z=1`, against **0** for every
//!    size of `ADDX`, `SUBX` and `NEGX`.
//! 2. **`X` is preserved, not derived**, by `CMP`, `CMPM`, `CMPA`, `AND`, `OR`,
//!    `EOR`, `NOT`, `TST` and `CLR`. 20,755 disagreeing cases, 20,755/20,755.
//! 3. **The logical ops clear `V` and `C`** rather than preserving them.
//!    24,738 disagreeing cases, 24,738/24,738.
//!
//! `ADDA`/`SUBA` and `ADDQ`/`SUBQ` with an `An` destination touch no flag at
//! all (6,906 disagreeing cases, 6,906/6,906), so they never come here.

use crate::ea::Size;

/// Computes `a + b + carry_in`, returning `(result, n, z, v, c)`.
///
/// `result` is masked to `size`; `n`, `z` and `v` describe that masked value.
pub fn add_flags(a: u32, b: u32, carry_in: bool, size: Size) -> (u32, bool, bool, bool, bool) {
    let m = size.mask();
    let msb = size.msb();
    let a = a & m;
    let b = b & m;
    let full = (a as u64) + (b as u64) + carry_in as u64;
    let res = (full as u32) & m;

    let c = full & (m as u64 + 1) != 0;
    // Overflow: operands agree in sign, result disagrees.
    let v = ((a ^ res) & (b ^ res) & msb) != 0;
    let n = res & msb != 0;
    let z = res == 0;
    (res, n, z, v, c)
}

/// Computes `dst - src - borrow_in`, returning `(result, n, z, v, c)`.
pub fn sub_flags(dst: u32, src: u32, borrow_in: bool, size: Size) -> (u32, bool, bool, bool, bool) {
    let m = size.mask();
    let msb = size.msb();
    let dst = dst & m;
    let src = src & m;
    let full = (dst as u64)
        .wrapping_sub(src as u64)
        .wrapping_sub(borrow_in as u64);
    let res = (full as u32) & m;

    let c = (src as u64) + (borrow_in as u64) > (dst as u64);
    // Overflow: operands differ in sign, result differs from dst.
    let v = ((dst ^ src) & (dst ^ res) & msb) != 0;
    let n = res & msb != 0;
    let z = res == 0;
    (res, n, z, v, c)
}

/// Flags for logical ops: N and Z from the result, V and C cleared, X kept.
///
/// Returns `(n, z, v, c)` — X is the caller's, because it is *preserved* here
/// rather than computed (see the module docs, rule 2).
pub fn logic_flags(res: u32, size: Size) -> (bool, bool, bool, bool) {
    let r = res & size.mask();
    (r & size.msb() != 0, r == 0, false, false)
}

/// Turns a fresh `Z` into the accumulating `Z` the X-carrying instructions use.
///
/// `ADDX`, `SUBX` and `NEGX` (and, in Task 9, `ABCD`, `SBCD` and `NBCD`) AND
/// their own result's zero-ness into the incoming `Z`, so a multi-precision
/// chain reports zero only if every limb was zero. Nothing else does this: see
/// the module docs for the control group.
#[inline]
pub fn accumulate_z(z_from_result: bool, z_initial: bool) -> bool {
    z_from_result && z_initial
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_byte_carry_and_overflow() {
        // 0x7F + 0x01 = 0x80: signed overflow, no carry.
        let (r, n, z, v, c) = add_flags(0x7F, 0x01, false, Size::Byte);
        assert_eq!(r, 0x80);
        assert!(n && !z && v && !c);

        // 0xFF + 0x01 = 0x00: carry out, no signed overflow.
        let (r, n, z, v, c) = add_flags(0xFF, 0x01, false, Size::Byte);
        assert_eq!(r, 0x00);
        assert!(!n && z && !v && c);

        // 0x80 + 0x80 = 0x00: both negative, result zero -> overflow and carry.
        let (r, _, z, v, c) = add_flags(0x80, 0x80, false, Size::Byte);
        assert_eq!(r, 0);
        assert!(z && v && c);
    }

    #[test]
    fn add_uses_carry_in() {
        let (r, _, _, _, c) = add_flags(0xFF, 0x00, true, Size::Byte);
        assert_eq!(r, 0x00);
        assert!(c, "carry-in must be able to produce a carry-out");
    }

    #[test]
    fn sub_borrow_and_overflow() {
        // 0x00 - 0x01 = 0xFF: borrow, no signed overflow.
        let (r, n, z, v, c) = sub_flags(0x00, 0x01, false, Size::Byte);
        assert_eq!(r, 0xFF);
        assert!(n && !z && !v && c);

        // 0x80 - 0x01 = 0x7F: signed overflow (negative minus positive
        // becomes positive), no borrow.
        let (r, n, _, v, c) = sub_flags(0x80, 0x01, false, Size::Byte);
        assert_eq!(r, 0x7F);
        assert!(!n && v && !c);

        // Equal operands: zero, no flags.
        let (r, n, z, v, c) = sub_flags(0x42, 0x42, false, Size::Byte);
        assert_eq!(r, 0);
        assert!(!n && z && !v && !c);
    }

    #[test]
    fn sub_borrow_in_triggers_carry_at_boundary() {
        // 0x00 - 0x00 - 1 = 0xFF with borrow.
        let (r, _, _, _, c) = sub_flags(0x00, 0x00, true, Size::Byte);
        assert_eq!(r, 0xFF);
        assert!(c);
    }

    #[test]
    fn long_size_boundaries() {
        let (r, n, _, v, c) = add_flags(0x7FFF_FFFF, 1, false, Size::Long);
        assert_eq!(r, 0x8000_0000);
        assert!(n && v && !c);

        let (r, _, z, v, c) = add_flags(0xFFFF_FFFF, 1, false, Size::Long);
        assert_eq!(r, 0);
        assert!(z && !v && c);
    }

    /// The upper bits of an operand must not leak into a narrower operation:
    /// a byte add of two values whose *word* sum would carry must not carry.
    #[test]
    fn operands_are_masked_to_the_operation_size() {
        let (r, _, _, _, c) = add_flags(0xFFFF_FF01, 0x1234_5601, false, Size::Byte);
        assert_eq!(r, 0x02, "only the low byte participates");
        assert!(!c);

        let (r, _, _, _, c) = sub_flags(0xAAAA_0000, 0xBBBB_0001, false, Size::Word);
        assert_eq!(r, 0xFFFF);
        assert!(c, "borrow comes from the word, not the long");
    }

    /// The accumulating `Z` of the X-carrying instructions can only clear the
    /// flag. This is the rule the brief states backwards (addendum §1).
    #[test]
    fn accumulating_z_never_sets_a_clear_z() {
        assert!(accumulate_z(true, true), "zero limb, Z was set: stays set");
        assert!(
            !accumulate_z(true, false),
            "a zero result must NOT set a clear Z"
        );
        assert!(!accumulate_z(false, true), "a nonzero result clears Z");
        assert!(!accumulate_z(false, false));
    }

    #[test]
    fn logic_flags_clear_v_and_c() {
        let (n, z, v, c) = logic_flags(0xFFFF_FF80, Size::Byte);
        assert!(n && !z && !v && !c);
        let (n, z, ..) = logic_flags(0xFFFF_FF00, Size::Byte);
        assert!(!n && z, "only the low byte decides N and Z");
        let (n, z, ..) = logic_flags(0x0000_8000, Size::Word);
        assert!(n && !z);
    }
}
