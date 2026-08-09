//! Data movement: loads, exchanges, and the stack.
//!
//! The exchanges and the stack write no flags, so they touch no `cpu.q` — the
//! decode arms clear it, per the `Q` convention in [`crate::decode`]. The two block
//! transfers at the bottom are the exception: `LDI` and `CPI` both write flags, and
//! both set `q` themselves.

use crate::flags::{self, C, F3, F5, H, N, PV, S, Z};
use crate::ops::Block;
use crate::{Bus, Z80};

/// `EX DE,HL`. No flags.
pub fn ex_de_hl(cpu: &mut Z80) {
    let de = cpu.de();
    cpu.set_de(cpu.hl());
    cpu.set_hl(de);
}

/// `EX AF,AF'`. The only exchange that moves the flag register.
pub fn ex_af(cpu: &mut Z80) {
    let af = cpu.af();
    cpu.set_af(cpu.af_);
    cpu.af_ = af;
}

/// `EXX`: BC, DE and HL with their shadows. `AF` is not in the set.
pub fn exx(cpu: &mut Z80) {
    let (bc, de, hl) = (cpu.bc(), cpu.de(), cpu.hl());
    cpu.set_bc(cpu.bc_);
    cpu.set_de(cpu.de_);
    cpu.set_hl(cpu.hl_);
    cpu.bc_ = bc;
    cpu.de_ = de;
    cpu.hl_ = hl;
}

/// Pushes `v`: high byte to `SP-1`, low to `SP-2`, then `SP -= 2`.
///
/// The order is what the vectors record, and it matters to anything that reads
/// the stack as bytes — a bootloader reading its own return address, for one.
pub fn push<B: Bus>(cpu: &mut Z80, bus: &mut B, v: u16) {
    cpu.sp = cpu.sp.wrapping_sub(1);
    bus.write(cpu.sp, (v >> 8) as u8);
    cpu.sp = cpu.sp.wrapping_sub(1);
    bus.write(cpu.sp, v as u8);
}

/// Pops: low byte from `SP`, high from `SP+1`, then `SP += 2`.
#[must_use]
pub fn pop<B: Bus>(cpu: &mut Z80, bus: &mut B) -> u16 {
    let lo = bus.read(cpu.sp);
    cpu.sp = cpu.sp.wrapping_add(1);
    let hi = bus.read(cpu.sp);
    cpu.sp = cpu.sp.wrapping_add(1);
    u16::from(hi) << 8 | u16::from(lo)
}

/// `LDI` / `LDD`: move `(HL)` to `(DE)`, step both, decrement `BC`.
///
/// The flags are unlike anything else on the chip. P/V is **`BC != 0`** — a repeat
/// count rather than a parity or an overflow — and F3/F5 come from `A + the byte
/// moved`, bit 3 into F3 and **bit 1** into F5, a sum that appears in no register.
/// S, Z and carry are all preserved: this instruction cannot report what it moved.
///
/// The latch is untouched, which makes these two the only memory-writing
/// instructions on the chip that leave it alone. Measured 0 wrong over 1,000 cases
/// each of `ed_a0` and `ed_a8`.
pub fn ldi_ldd<B: Bus>(cpu: &mut Z80, bus: &mut B, block: Block) {
    let v = bus.read(cpu.hl());
    bus.write(cpu.de(), v);
    cpu.set_hl(cpu.hl().wrapping_add(block.step()));
    cpu.set_de(cpu.de().wrapping_add(block.step()));
    cpu.set_bc(cpu.bc().wrapping_sub(1));
    let n = cpu.a.wrapping_add(v);
    cpu.f = (cpu.f & (C | Z | S))
        | (n & F3)
        | if n & 0x02 != 0 { F5 } else { 0 }
        | if cpu.bc() != 0 { PV } else { 0 };
    cpu.q = cpu.f;
}

/// `CPI` / `CPD`: compare `A` with `(HL)`, step `HL`, decrement `BC`.
///
/// `A` is not written and carry is preserved — the two differences from `CP`. F3/F5
/// come from `A - value - H` by the same trick as [`ldi_ldd`], bit 3 to F3 and bit 1
/// to F5, and P/V is again `BC != 0` rather than an overflow.
///
/// The latch **steps**, unlike [`ldi_ldd`]'s: `CPI` leaves the incoming `wz` plus
/// one and `CPD` leaves it minus one. It is the only instruction on the chip whose
/// latch is a function of its own previous value, so a core that recomputed `wz`
/// from scratch each instruction cannot express it. Measured 0 wrong over 1,000
/// cases each of `ed_a1` and `ed_a9`.
pub fn cpi_cpd<B: Bus>(cpu: &mut Z80, bus: &mut B, block: Block) {
    let v = bus.read(cpu.hl());
    let diff = cpu.a.wrapping_sub(v);
    let h = (cpu.a & 0x0F) < (v & 0x0F);
    cpu.set_hl(cpu.hl().wrapping_add(block.step()));
    cpu.set_bc(cpu.bc().wrapping_sub(1));
    cpu.wz = cpu.wz.wrapping_add(block.step());
    let n = diff.wrapping_sub(u8::from(h));
    cpu.f = (cpu.f & C)
        | N
        | (flags::sz53(diff) & (S | Z))
        | if h { H } else { 0 }
        | (n & F3)
        | if n & 0x02 != 0 { F5 } else { 0 }
        | if cpu.bc() != 0 { PV } else { 0 };
    cpu.q = cpu.f;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testbus::Mem;

    /// `EX DE,HL` swaps the pairs and touches no flags.
    #[test]
    fn ex_de_hl_swaps_the_pairs_and_no_flags() {
        let mut c = Z80::new();
        c.set_de(0x1234);
        c.set_hl(0xABCD);
        c.f = 0x5A;
        ex_de_hl(&mut c);
        assert_eq!(c.de(), 0xABCD);
        assert_eq!(c.hl(), 0x1234);
        assert_eq!(c.f, 0x5A, "no flags, and F is not part of the swap");
    }

    /// `EX AF,AF'` swaps `AF` with the shadow — **including `F`**.
    ///
    /// The one exchange that moves the flag register, which is why it cannot go
    /// through the same helper as `EXX`.
    #[test]
    fn ex_af_swaps_the_flag_register_too() {
        let mut c = Z80::new();
        c.a = 0x12;
        c.f = 0x34;
        c.af_ = 0xABCD;
        ex_af(&mut c);
        assert_eq!((c.a, c.f), (0xAB, 0xCD));
        assert_eq!(c.af_, 0x1234);
    }

    /// `EXX` swaps BC, DE and HL with their shadows and leaves `AF` alone.
    #[test]
    fn exx_swaps_three_pairs_and_spares_af() {
        let mut c = Z80::new();
        c.set_bc(0x1111);
        c.set_de(0x2222);
        c.set_hl(0x3333);
        c.bc_ = 0xAAAA;
        c.de_ = 0xBBBB;
        c.hl_ = 0xCCCC;
        c.a = 0x77;
        c.f = 0x88;
        exx(&mut c);
        assert_eq!((c.bc(), c.de(), c.hl()), (0xAAAA, 0xBBBB, 0xCCCC));
        assert_eq!((c.bc_, c.de_, c.hl_), (0x1111, 0x2222, 0x3333));
        assert_eq!((c.a, c.f), (0x77, 0x88), "AF is not in the EXX set");
    }

    /// `PUSH` writes the high byte first, at `SP-1`.
    ///
    /// The order is observable — a `PUSH` followed by a byte read at `SP` must see
    /// the low half — and a core that wrote low-first would still round-trip
    /// through its own `POP`, which is why the memory is inspected directly.
    #[test]
    fn push_writes_high_byte_first_and_predecrements() {
        let mut c = Z80::new();
        c.sp = 0x8000;
        let mut m = Mem::new();
        push(&mut c, &mut m, 0x1234);
        assert_eq!(c.sp, 0x7FFE);
        assert_eq!(m.ram[0x7FFF], 0x12, "the high byte went to SP-1");
        assert_eq!(m.ram[0x7FFE], 0x34, "the low byte to SP-2");
        // In that order, which the final RAM contents cannot show.
        assert_eq!(m.writes, vec![(0x7FFF, 0x12), (0x7FFE, 0x34)]);
    }

    /// `POP` reads low then high and post-increments.
    #[test]
    fn pop_reads_low_then_high() {
        let mut c = Z80::new();
        c.sp = 0x7FFE;
        let mut m = Mem::new();
        m.ram[0x7FFE] = 0x34;
        m.ram[0x7FFF] = 0x12;
        assert_eq!(pop(&mut c, &mut m), 0x1234);
        assert_eq!(c.sp, 0x8000);
    }

    /// The stack wraps in 16 bits at both ends.
    ///
    /// `PUSH` with `SP = 0x0000` writes at 0xFFFF and 0xFFFE, and `POP` with
    /// `SP = 0xFFFF` reads the second byte from 0x0000. Real Z80 code hits this on
    /// any board that puts its stack at the top of RAM, and `wrapping_sub` is not
    /// exercised by any test that starts mid-memory.
    #[test]
    fn the_stack_pointer_wraps_at_both_ends_of_memory() {
        let mut c = Z80::new();
        c.sp = 0x0000;
        let mut m = Mem::new();
        push(&mut c, &mut m, 0x1234);
        assert_eq!(c.sp, 0xFFFE);
        assert_eq!(m.ram[0xFFFF], 0x12);
        assert_eq!(m.ram[0xFFFE], 0x34);

        let mut c = Z80::new();
        c.sp = 0xFFFF;
        let mut m = Mem::new();
        m.ram[0xFFFF] = 0x34;
        m.ram[0x0000] = 0x12;
        assert_eq!(
            pop(&mut c, &mut m),
            0x1234,
            "the high byte came from 0x0000"
        );
        assert_eq!(c.sp, 0x0001);
    }

    /// `LDI` leaves the latch alone; `CPI` and `CPD` step it by one.
    ///
    /// The three are asserted together because the interesting claim is the
    /// difference between them: a core with one block-transfer latch rule gets two of
    /// the three wrong, and `CPD`'s downward step is invisible to any test that only
    /// checks `CPI`. `SENTINEL` is a value no rule here could produce, so "left
    /// alone" is distinguishable from "written correctly by accident".
    #[test]
    fn the_block_transfers_disagree_about_the_latch() {
        const SENTINEL: u16 = 0x5EED;

        let mut c = Z80::new();
        c.set_hl(0x2000);
        c.set_de(0x3000);
        c.set_bc(1);
        c.wz = SENTINEL;
        let mut m = Mem::new();
        ldi_ldd(&mut c, &mut m, Block::from_opcode(0xA0));
        assert_eq!(
            c.wz, SENTINEL,
            "LDI writes memory and still leaves the latch alone"
        );

        let mut c = Z80::new();
        c.set_hl(0x2000);
        c.set_bc(1);
        c.wz = SENTINEL;
        cpi_cpd(&mut c, &mut m, Block::from_opcode(0xA1));
        assert_eq!(c.wz, 0x5EEE, "CPI adds one to whatever was there");

        let mut c = Z80::new();
        c.set_hl(0x2000);
        c.set_bc(1);
        c.wz = SENTINEL;
        cpi_cpd(&mut c, &mut m, Block::from_opcode(0xA9));
        assert_eq!(c.wz, 0x5EEC, "and CPD subtracts one");
    }

    /// `CPD` steps `HL` down and does not touch `DE`.
    ///
    /// `LDD`'s downward step covers both pairs; `CPD` has only `HL`, and a handler
    /// that stepped `DE` too would be invisible in the vectors' final state only if
    /// `DE` happened to be ignored — which it is not.
    #[test]
    fn cpd_steps_hl_downwards_and_leaves_de_alone() {
        let mut c = Z80::new();
        c.a = 0x5A;
        c.set_hl(0x2000);
        c.set_de(0x3000);
        c.set_bc(2);
        let mut m = Mem::new();
        m.ram[0x2000] = 0x5A;
        cpi_cpd(&mut c, &mut m, Block::from_opcode(0xA9));
        assert_eq!(c.hl(), 0x1FFF);
        assert_eq!(c.de(), 0x3000, "CPD has no DE");
        assert_eq!(c.bc(), 1);
        assert_eq!(m.writes, vec![], "and it writes no memory");
    }

    /// `LDD` steps both pairs down.
    #[test]
    fn ldd_steps_both_pairs_downwards() {
        let mut c = Z80::new();
        c.set_hl(0x2000);
        c.set_de(0x3000);
        c.set_bc(1);
        let mut m = Mem::new();
        m.ram[0x2000] = 0x77;
        ldi_ldd(&mut c, &mut m, Block::from_opcode(0xA8));
        assert_eq!(m.ram[0x3000], 0x77);
        assert_eq!(c.hl(), 0x1FFF);
        assert_eq!(c.de(), 0x2FFF);
    }

    /// `LDI` preserves S, Z and carry, and writes only P/V, F3 and F5.
    ///
    /// Three preserved flags is unusual enough that a core is likely to compute S and
    /// Z from something — the byte moved, or `BC` — and be right by coincidence on
    /// half the cases. Setting all three on entry with a byte and a count that would
    /// each clear them makes the preservation the only way to pass.
    #[test]
    fn ldi_preserves_sign_zero_and_carry() {
        let mut c = Z80::new();
        c.a = 0x00;
        c.set_hl(0x2000);
        c.set_de(0x3000);
        c.set_bc(1);
        c.f = S | Z | C | H | N;
        let mut m = Mem::new();
        // A byte of 0x04: the sum's bits 3 and 1 are clear, so F3/F5 clear.
        m.ram[0x2000] = 0x04;
        ldi_ldd(&mut c, &mut m, Block::from_opcode(0xA0));
        assert_eq!(c.f & (S | Z | C), S | Z | C, "all three survive");
        assert_eq!(c.f & (H | N), 0, "H and N are always cleared");
        assert_eq!(c.f & PV, 0, "BC reached zero");
        assert_eq!(c.f & (F3 | F5), 0, "and the sum 0x04 has neither bit");
        assert_eq!(c.q, c.f);
    }
}
