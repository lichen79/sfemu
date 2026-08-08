//! The flag register's bits and the rules that fill them.
//!
//! Eight bits, two of them undocumented:
//!
//! | bit | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 |
//! |-----|---|---|---|---|---|---|---|---|
//! |     | S | Z | F5 | H | F3 | P/V | N | C |
//!
//! F5 and F3 hold copies of the result's bits 5 and 3 for nearly every
//! instruction. They are absent from the Zilog manual and present in `f`, which
//! the vectors compare on every one of 1,604,000 cases — so they are not optional
//! and they are not a curiosity.

/// Sign: a copy of the result's bit 7.
pub const S: u8 = 0x80;
/// Zero: set when the result is zero.
pub const Z: u8 = 0x40;
/// Undocumented: a copy of the result's bit 5.
pub const F5: u8 = 0x20;
/// Half carry: a carry out of bit 3.
pub const H: u8 = 0x10;
/// Undocumented: a copy of the result's bit 3.
pub const F3: u8 = 0x08;
/// Parity or overflow, depending on the instruction.
pub const PV: u8 = 0x04;
/// Add/subtract: set by subtractions, for `DAA`'s benefit.
pub const N: u8 = 0x02;
/// Carry.
pub const C: u8 = 0x01;

/// Even parity: `true` when `v` has an even number of set bits.
#[must_use]
pub fn parity(v: u8) -> bool {
    v.count_ones().is_multiple_of(2)
}

/// S, Z, and the two undocumented bits, taken from `v`.
///
/// H, N, C and P/V are deliberately untouched: they depend on the operation, not
/// on the result alone, so a helper that guessed at them would be wrong more
/// often than right.
#[must_use]
pub fn sz53(v: u8) -> u8 {
    (v & (S | F5 | F3)) | if v == 0 { Z } else { 0 }
}

/// [`sz53`] plus even parity in the P/V bit — the logical operations' pattern.
#[must_use]
pub fn sz53p(v: u8) -> u8 {
    sz53(v) | if parity(v) { PV } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parity is even parity: set when the number of set bits is even.
    ///
    /// Computed by hand at values chosen for their bit counts, not by comparing
    /// against a second implementation of the same idea.
    #[test]
    fn parity_is_set_when_the_bit_count_is_even() {
        assert!(parity(0x00), "zero bits set is even");
        assert!(!parity(0x01), "one bit");
        assert!(parity(0x03), "two bits");
        assert!(!parity(0x07), "three bits");
        assert!(parity(0xFF), "eight bits");
        assert!(!parity(0x7F), "seven bits");
        assert!(parity(0x81), "two bits, far apart");
        assert!(!parity(0x80), "one bit, high");
    }

    /// `sz53` takes S, Z, and the two undocumented bits from a result.
    ///
    /// The values are written out in binary so the four bits being claimed are
    /// visible in the test rather than implied by a mask.
    #[test]
    fn sz53_copies_sign_zero_and_bits_five_and_three() {
        assert_eq!(sz53(0x00), Z, "zero sets Z and nothing else");
        assert_eq!(sz53(0x80), S, "bit 7 is the sign");
        assert_eq!(sz53(0x28), F5 | F3, "bits 5 and 3 are copied straight");
        assert_eq!(sz53(0xA8), S | F5 | F3);
        assert_eq!(sz53(0x01), 0, "bit 0 touches no flag here");
        // The two undocumented bits are copied independently, which a value with
        // both set cannot show: 0x20 must give F5 alone and 0x08 F3 alone, or the
        // two are transposed and every assertion above still holds.
        assert_eq!(sz53(0x20), F5, "bit 5 alone sets F5 alone");
        assert_eq!(sz53(0x08), F3, "bit 3 alone sets F3 alone");
        // H, N, C and P/V are never set by sz53 -- they are the caller's business.
        for v in [0x00u8, 0x01, 0x7F, 0x80, 0xFF] {
            assert_eq!(
                sz53(v) & (H | N | C | PV),
                0,
                "sz53({v:#04X}) must not set H/N/C/PV"
            );
        }
    }

    /// `sz53p` is `sz53` plus even parity in the P/V bit.
    #[test]
    fn sz53p_adds_parity() {
        assert_eq!(sz53p(0x00), Z | PV, "zero is even parity");
        assert_eq!(sz53p(0x01), 0, "one bit: odd, no parity, no sign");
        assert_eq!(
            sz53p(0xFF),
            S | F5 | F3 | PV,
            "eight bits, and 5 and 3 are set"
        );
        assert_eq!(sz53p(0x80), S, "one bit, high: sign only");
    }

    /// The bit values themselves, because a transposed constant is invisible.
    ///
    /// `F5` at bit 5 and `F3` at bit 3 are the two that get swapped, and every
    /// other test in this file would pass with them swapped, since both are copied
    /// from bits of the same name.
    #[test]
    fn the_flag_bits_are_where_the_hardware_puts_them() {
        assert_eq!(S, 0x80);
        assert_eq!(Z, 0x40);
        assert_eq!(F5, 0x20);
        assert_eq!(H, 0x10);
        assert_eq!(F3, 0x08);
        assert_eq!(PV, 0x04);
        assert_eq!(N, 0x02);
        assert_eq!(C, 0x01);
    }
}
