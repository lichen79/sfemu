//! The instruction handlers, split by what they do to the machine.
//!
//! The one thing that lives here rather than in a submodule is the block
//! instructions' shared shape: sixteen opcodes across four families that all take
//! their direction and their repetition from the same two opcode bits, and all
//! repeat by the same mechanism. [`Block`] decodes those bits once and `repeat`
//! applies the repeat rules once, so the four decode arms cannot disagree.

pub mod alu;
pub mod bits;
pub mod flow;
pub mod io;
pub mod load;

use crate::flags::{F3, F5};
use crate::Z80;

/// The two parameters every block instruction on the `ED` page shares.
///
/// Bit 3 of the opcode is the direction and bit 4 the repetition, for all sixteen
/// block opcodes. Decoding both here means `LDI` and `LDIR` are one handler rather
/// than two copies that can drift.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Block {
    /// `HL` — and `DE`, for a transfer — steps up rather than down.
    pub inc: bool,
    /// One of the `B_` forms, which re-execute while their count lasts.
    pub repeating: bool,
}

impl Block {
    /// Decodes bits 4 and 3 of a block opcode.
    #[must_use]
    pub fn from_opcode(op: u8) -> Self {
        Block {
            inc: op & 0x08 == 0,
            repeating: op & 0x10 != 0,
        }
    }

    /// The value to `wrapping_add` to a pointer: `1` up, `0xFFFF` down.
    ///
    /// One addend either way rather than a branch between an add and a subtract, so
    /// the `HL`, `DE` and latch updates cannot drift apart, and so a block move at
    /// either end of memory wraps the way the hardware does.
    #[must_use]
    pub fn step(self) -> u16 {
        if self.inc {
            1
        } else {
            0xFFFF
        }
    }
}

/// Rewinds `PC` onto the prefix so the instruction re-executes, applies the two
/// state rules that come with repeating, and returns the repeating T-state cost.
///
/// **This is not a loop.** An `LDIR` over 65,536 bytes is 65,536 separate `step`
/// calls, because an interrupt can land between iterations on real hardware and
/// because the vectors are single instructions — a `while` inside a handler would
/// be both wrong and unverifiable.
///
/// The rules, measured over the 1,000 cases of each of the eight repeating files:
///
/// - `PC` goes back two, onto the `ED`, so the next M1 fetch re-reads the same
///   instruction. The vectors show the final `PC` equal to the initial one.
/// - The latch becomes `PC + 1`, pointing at the opcode byte after the prefix —
///   which is the one `wz` rule the whole `ED` page shares.
/// - **F3 and F5 come from the high byte of the rewound `PC`**, not from the byte
///   the non-repeating form derives them from. On `LDIR`, `LDDR`, `CPIR` and `CPDR`
///   this is the only difference from the non-repeating sibling; the sum-derived
///   rule is wrong on 721 to 762 of 1,000 cases there, and this one on none.
///
/// The four block-I/O forms need `io::block_io_repeat_adjust` as well, applied
/// before this — it moves H and P/V, which this leaves alone.
pub(crate) fn repeat(cpu: &mut Z80) -> u32 {
    cpu.pc = cpu.pc.wrapping_sub(2);
    cpu.wz = cpu.pc.wrapping_add(1);
    cpu.f = (cpu.f & !(F5 | F3)) | (((cpu.pc >> 8) as u8) & (F5 | F3));
    cpu.q = cpu.f;
    21
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::{C, H};

    /// Bit 3 is the direction and bit 4 the repetition, for all sixteen opcodes.
    ///
    /// Asserted over the whole family rather than on one opcode: the two bits are
    /// adjacent, and a decoder that swapped them would still get `LDI` (both clear)
    /// and `LDDR` (both set) right — half the family, and the half a spot check
    /// would pick.
    #[test]
    fn a_block_opcode_encodes_direction_in_bit_three_and_repetition_in_bit_four() {
        for (op, inc, repeating) in [
            (0xA0u8, true, false), // LDI
            (0xA8, false, false),  // LDD
            (0xB0, true, true),    // LDIR
            (0xB8, false, true),   // LDDR
            (0xA1, true, false),   // CPI
            (0xA9, false, false),  // CPD
            (0xB1, true, true),    // CPIR
            (0xB9, false, true),   // CPDR
            (0xA2, true, false),   // INI
            (0xAA, false, false),  // IND
            (0xB2, true, true),    // INIR
            (0xBA, false, true),   // INDR
            (0xA3, true, false),   // OUTI
            (0xAB, false, false),  // OUTD
            (0xB3, true, true),    // OTIR
            (0xBB, false, true),   // OTDR
        ] {
            let b = Block::from_opcode(op);
            assert_eq!(b.inc, inc, "ED {op:#04X} direction");
            assert_eq!(b.repeating, repeating, "ED {op:#04X} repetition");
        }
    }

    /// The step is `+1` or `-1` in sixteen bits, expressed as an addend.
    #[test]
    fn the_step_is_an_addend_either_way() {
        let up = Block {
            inc: true,
            repeating: false,
        };
        let down = Block {
            inc: false,
            repeating: false,
        };
        assert_eq!(0x1000u16.wrapping_add(up.step()), 0x1001);
        assert_eq!(0x1000u16.wrapping_add(down.step()), 0x0FFF);
        // And it wraps at both ends, which a block move at either end of memory
        // reaches and which a plain `+ 1` would panic on in a debug build.
        assert_eq!(0xFFFFu16.wrapping_add(up.step()), 0x0000);
        assert_eq!(0x0000u16.wrapping_add(down.step()), 0xFFFF);
    }

    /// Repeating rewinds two, latches `PC + 1`, and re-takes F3/F5 from `PC`'s high
    /// byte.
    ///
    /// The high byte is chosen with F5 set and F3 clear while the incoming flags
    /// have the reverse, so "took them from `PC`", "left them alone" and "cleared
    /// both" are three distinguishable outcomes rather than two.
    #[test]
    fn repeating_rewinds_the_program_counter_and_reflags_from_its_high_byte() {
        let mut c = Z80::new();
        c.pc = 0x2002; // past a two-byte instruction at 0x2000
        c.wz = 0x5EED;
        c.f = F3 | C | H;
        assert_eq!(repeat(&mut c), 21, "repeating costs 21");
        assert_eq!(c.pc, 0x2000, "back onto the ED prefix");
        assert_eq!(c.wz, 0x2001, "and the latch holds PC + 1");
        assert_eq!(c.f & F5, F5, "0x20's bit 5 is set, so F5 is");
        assert_eq!(c.f & F3, 0, "0x20's bit 3 is clear, so F3 is cleared");
        assert_eq!(c.f & (C | H), C | H, "and nothing else moves");
        assert_eq!(c.q, c.f);
    }
}
