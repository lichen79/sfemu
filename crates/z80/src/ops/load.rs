//! Data movement: loads, exchanges, and the stack.
//!
//! Nothing here writes a flag, so nothing here touches `cpu.q` — the decode arms
//! clear it, per the `Q` convention in [`crate::decode`].

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
}
