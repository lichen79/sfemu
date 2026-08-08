//! Jumps, calls, returns, and the conditions that gate them.
//!
//! Nothing here writes a flag — not even `DJNZ`, which is the whole reason it is
//! not `DEC B` followed by `JR NZ`.

use crate::flags::{C, PV, S, Z};
use crate::ops::load;
use crate::{Bus, Z80};

/// Evaluates condition code `i`: `NZ Z NC C PO PE P M`.
///
/// Each pair is a flag and its negation, in that order — so the low bit selects
/// "flag set" and the upper two select which flag.
#[must_use]
pub fn cond(cpu: &Z80, i: u8) -> bool {
    let flag = match i >> 1 {
        0 => Z,
        1 => C,
        2 => PV,
        3 => S,
        _ => unreachable!("condition {i} is not three bits"),
    };
    (cpu.f & flag != 0) == (i & 1 != 0)
}

/// Applies a signed displacement to `PC`, which already points past the operand.
pub fn jr(cpu: &mut Z80, d: u8) {
    cpu.pc = cpu.pc.wrapping_add(i16::from(d as i8) as u16);
}

/// Pushes the return address and jumps.
pub fn call<B: Bus>(cpu: &mut Z80, bus: &mut B, target: u16) {
    let ret_addr = cpu.pc;
    load::push(cpu, bus, ret_addr);
    cpu.pc = target;
}

/// Pops the return address.
pub fn ret<B: Bus>(cpu: &mut Z80, bus: &mut B) {
    cpu.pc = load::pop(cpu, bus);
}

/// `RST n`: a one-byte call to `n * 8`.
pub fn rst<B: Bus>(cpu: &mut Z80, bus: &mut B, n: u8) {
    call(cpu, bus, u16::from(n) * 8);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testbus::Mem;

    /// The eight condition codes, each read from the flag it names.
    ///
    /// Written as a table of (index, flag, expected-when-set) so a transposed pair
    /// — NZ against Z, or PO against PE — shows up as a failure rather than as two
    /// mutually consistent bugs.
    #[test]
    fn the_eight_conditions_read_the_flags_they_name() {
        let mut c = Z80::new();
        for (i, flag, when_set) in [
            (0u8, Z, false), // NZ
            (1, Z, true),    // Z
            (2, C, false),   // NC
            (3, C, true),    // C
            (4, PV, false),  // PO, parity odd
            (5, PV, true),   // PE, parity even
            (6, S, false),   // P, sign positive
            (7, S, true),    // M, sign negative
        ] {
            c.f = 0;
            assert_eq!(cond(&c, i), !when_set, "condition {i} with the flag clear");
            c.f = flag;
            assert_eq!(cond(&c, i), when_set, "condition {i} with the flag set");
        }
    }

    /// Each condition reads its own flag and no other.
    ///
    /// The table above sets one flag at a time, so a `cond` that ORed two flags
    /// together — or ignored the index's upper bits entirely and always read `Z` —
    /// still passes it for four of the eight. Here every *other* flag is set and
    /// the named one is not: the answer must be the same as with `f = 0`.
    #[test]
    fn a_condition_ignores_the_flags_it_does_not_name() {
        let mut c = Z80::new();
        for (i, flag) in [
            (0u8, Z),
            (1, Z),
            (2, C),
            (3, C),
            (4, PV),
            (5, PV),
            (6, S),
            (7, S),
        ] {
            c.f = 0;
            let with_none = cond(&c, i);
            // Every flag but this one.
            c.f = !flag;
            assert_eq!(
                cond(&c, i),
                with_none,
                "condition {i} changed when a flag other than {flag:#04X} was set"
            );
        }
    }

    /// `JR` adds a **signed** displacement to the address after the instruction.
    ///
    /// The sign is the point: a backwards jump is how every loop on this chip is
    /// written, and an unsigned reading would send it 254 bytes forward instead of
    /// two back.
    #[test]
    fn jr_displacement_is_signed_and_relative_to_the_next_instruction() {
        let mut c = Z80::new();
        c.pc = 0x100;
        // 0x18 0xFE is "jump to yourself": PC is 0x102 after the operand, and
        // -2 lands back on the 0x18.
        let mut m = Mem::at(0x100, &[0x18, 0xFE]);
        assert_eq!(c.step(&mut m), 12, "JR is always taken and always 12");
        assert_eq!(c.pc, 0x100, "a displacement of -2 is an infinite loop");

        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0x18, 0x05]);
        c.step(&mut m);
        assert_eq!(c.pc, 0x107, "0x102 + 5");
    }

    /// A conditional jump costs different T-states taken and not taken.
    ///
    /// The pairs are the whole reason this needs testing: a core that returned 12
    /// for both would pass every register assertion and fail half of `20.z80bin`.
    #[test]
    fn a_conditional_jump_costs_more_when_taken() {
        // JR NZ,+5 with Z clear: taken.
        let mut c = Z80::new();
        c.pc = 0x100;
        c.f = 0;
        let mut m = Mem::at(0x100, &[0x20, 0x05]);
        assert_eq!(c.step(&mut m), 12, "taken");
        assert_eq!(c.pc, 0x107);

        // With Z set: not taken, and cheaper.
        let mut c = Z80::new();
        c.pc = 0x100;
        c.f = Z;
        assert_eq!(c.step(&mut m), 7, "not taken");
        assert_eq!(c.pc, 0x102, "and PC only passed the operand");
    }

    /// `CALL` pushes the return address and `RET` pops it.
    #[test]
    fn call_pushes_the_return_address_and_ret_pops_it() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.sp = 0x8000;
        let mut m = Mem::at(0x100, &[0xCD, 0x34, 0x12]);
        assert_eq!(c.step(&mut m), 17);
        assert_eq!(c.pc, 0x1234);
        assert_eq!(c.sp, 0x7FFE);
        assert_eq!(m.ram[0x7FFF], 0x01, "the return address 0x0103, high byte");
        assert_eq!(m.ram[0x7FFE], 0x03, "and low");

        m.ram[0x1234] = 0xC9; // RET
        assert_eq!(c.step(&mut m), 10, "an unconditional RET is 10");
        assert_eq!(c.pc, 0x0103, "back after the CALL");
        assert_eq!(c.sp, 0x8000);
    }

    /// A conditional `RET` is 11 taken and 5 not.
    ///
    /// Not 10 and 4: the condition test itself costs a T-state, and it is charged
    /// on both paths.
    #[test]
    fn a_conditional_ret_costs_eleven_or_five() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.sp = 0x7FFE;
        c.f = 0;
        let mut m = Mem::at(0x100, &[0xC0]); // RET NZ
        m.ram[0x7FFE] = 0x03;
        m.ram[0x7FFF] = 0x01;
        assert_eq!(c.step(&mut m), 11, "taken");
        assert_eq!(c.pc, 0x0103);

        let mut c = Z80::new();
        c.pc = 0x100;
        c.sp = 0x7FFE;
        c.f = Z;
        assert_eq!(c.step(&mut m), 5, "not taken");
        assert_eq!(c.pc, 0x101);
        assert_eq!(c.sp, 0x7FFE, "and the stack is untouched");
    }

    /// `DJNZ` decrements `B`, jumps while it is non-zero, and touches no flags.
    ///
    /// The no-flags part is what separates it from `DEC B` plus `JR NZ`: a loop
    /// using `DJNZ` can carry a comparison result across its whole body.
    #[test]
    fn djnz_decrements_b_without_flags_and_costs_thirteen_or_eight() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.b = 2;
        c.f = 0xFF;
        let mut m = Mem::at(0x100, &[0x10, 0xFE]);
        assert_eq!(c.step(&mut m), 13, "B became 1, so the jump was taken");
        assert_eq!(c.b, 1);
        assert_eq!(c.pc, 0x100);
        assert_eq!(c.f, 0xFF, "DJNZ writes no flags at all");

        assert_eq!(c.step(&mut m), 8, "B became 0: not taken, and cheaper");
        assert_eq!(c.b, 0);
        assert_eq!(c.pc, 0x102);
    }

    /// `RST n` pushes and jumps to `n * 8`, in 11 T-states.
    #[test]
    fn rst_jumps_to_a_multiple_of_eight() {
        for (op, target) in [(0xC7u8, 0x00u16), (0xCF, 0x08), (0xDF, 0x18), (0xFF, 0x38)] {
            let mut c = Z80::new();
            c.pc = 0x100;
            c.sp = 0x8000;
            let mut m = Mem::at(0x100, &[op]);
            assert_eq!(c.step(&mut m), 11, "RST is 11");
            assert_eq!(c.pc, target, "RST {op:#04X}");
            assert_eq!(c.sp, 0x7FFE);
            assert_eq!(m.ram[0x7FFF], 0x01, "and the return address is pushed");
            assert_eq!(m.ram[0x7FFE], 0x01);
        }
    }

    /// `JP (HL)` is four T-states and reads no memory.
    ///
    /// The parentheses in the mnemonic are a lie inherited from Zilog: the target
    /// is `HL` itself, not the word at `HL`. A core that dereferenced would jump
    /// somewhere plausible and wrong.
    #[test]
    fn jp_hl_uses_hl_and_does_not_dereference_it() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x1234);
        let mut m = Mem::at(0x100, &[0xE9]);
        m.ram[0x1234] = 0x99;
        m.ram[0x1235] = 0x88;
        assert_eq!(c.step(&mut m), 4);
        assert_eq!(c.pc, 0x1234, "HL, not the word at HL");
    }

    /// `EX (SP),HL` swaps `HL` with the word on top of the stack.
    ///
    /// Both directions in one instruction, and `SP` does not move. The bytes are
    /// checked in memory rather than by swapping twice, because a core that got the
    /// halves the wrong way round would be its own inverse.
    #[test]
    fn ex_sp_hl_swaps_hl_with_the_top_of_the_stack() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.sp = 0x8000;
        c.set_hl(0x1234);
        let mut m = Mem::at(0x100, &[0xE3]);
        m.ram[0x8000] = 0xCD;
        m.ram[0x8001] = 0xAB;
        assert_eq!(c.step(&mut m), 19);
        assert_eq!(c.hl(), 0xABCD, "HL took the word from the stack");
        assert_eq!(m.ram[0x8000], 0x34, "and L went to SP");
        assert_eq!(m.ram[0x8001], 0x12, "with H at SP + 1");
        assert_eq!(c.sp, 0x8000, "SP does not move");
        // The two bytes go back in the reverse of the order they were read: `H` at
        // `SP + 1` first. RAM cannot tell the two orders apart, and the vectors
        // compare the bus cycle by cycle, so `e3.z80bin` failed all 1,000 of its
        // cases on this alone while every register above was already right.
        assert_eq!(
            m.writes,
            vec![(0x8001, 0x12), (0x8000, 0x34)],
            "the high byte is written first"
        );
    }

    /// `IN A,(n)` and `OUT (n),A` put `A` on the port's **high** byte.
    ///
    /// Which is why [`crate::Bus`]'s port methods take 16 bits. A core that passed
    /// `n` alone would talk to the right device on a board that ignores the high
    /// half and to the wrong one everywhere else.
    #[test]
    fn the_accumulator_port_instructions_use_a_as_the_high_address_byte() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x12;
        let mut m = Mem::at(0x100, &[0xD3, 0x34]);
        assert_eq!(c.step(&mut m), 11);
        assert_eq!(m.ports_out, vec![(0x1234, 0x12)], "port 0x1234, value A");

        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x12;
        let mut m = Mem::at(0x100, &[0xDB, 0x34]);
        m.port_in_value = 0x5A;
        assert_eq!(c.step(&mut m), 11);
        assert_eq!(
            m.ports_in,
            vec![0x1234],
            "the port is A's old value over n, read before A is overwritten"
        );
        assert_eq!(c.a, 0x5A, "and IN lands the device's byte in A");
    }
}
