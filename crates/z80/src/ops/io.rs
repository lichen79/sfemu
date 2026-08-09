//! The I/O-space instructions.
//!
//! Two families with different flag behaviour: `IN A,(n)` and `OUT (n),A` on the
//! base page write no flags, while `IN r,(C)` and the block forms on the `ED` page
//! do. Sharing an implementation between them is the mistake this split prevents —
//! the base-page pair lives in [`crate::decode::execute`] and never calls in here.

use crate::flags::{self, C, H, N, PV};
use crate::ops::Block;
use crate::{Bus, Z80};

/// `IN r,(C)`: reads port `BC` and **flags the byte**.
///
/// Carry is the only incoming flag that survives. The latch becomes `BC + 1`, as it
/// does for every port form on this page — measured 0 wrong over the 1,000 cases of
/// each of the sixteen `IN`/`OUT` files.
#[must_use]
pub fn in_r_c<B: Bus>(cpu: &mut Z80, bus: &mut B) -> u8 {
    let v = bus.port_in(cpu.bc());
    cpu.wz = cpu.bc().wrapping_add(1);
    cpu.f = (cpu.f & C) | flags::sz53p(v);
    cpu.q = cpu.f;
    v
}

/// `OUT (C),r`: writes `v` to port `BC`. No flags.
///
/// The latch still becomes `BC + 1` — unlike `OUT (n),A` on the base page, whose
/// latch takes the written byte in its high half. Two `OUT`s, two rules.
pub fn out_c_r<B: Bus>(cpu: &mut Z80, bus: &mut B, v: u8) {
    bus.port_out(cpu.bc(), v);
    cpu.wz = cpu.bc().wrapping_add(1);
    cpu.q = 0;
}

/// `INI` / `IND` / `INIR` / `INDR`: read a port into `(HL)`, step `HL`, decrement `B`.
///
/// The port is read at the **old** `BC`, before `B` is decremented — 1,000 of 1,000
/// cases on each of the four files, and never `BC - 0x100`. That is the opposite of
/// [`outi_outd`], which is the single most surprising asymmetry on this page.
///
/// The latch is that same **old** `BC`, stepped: `INI` and `INIR` give `BC + 1`,
/// `IND` and `INDR` give `BC - 1`, with `B` as it was before the decrement. The
/// unifying rule across both families is that the latch follows the address that was
/// on the bus — which for these is the pre-decrement `BC` and for [`outi_outd`] is
/// the post-decrement one. Taking it after the decrement here is wrong by exactly
/// 0x100 on 1,000 of `ed_a2`'s 1,000 cases, which is how the ordering was pinned.
///
/// Returns the byte transferred, which the repeating forms need for
/// `block_io_repeat_adjust`.
pub fn ini_ind<B: Bus>(cpu: &mut Z80, bus: &mut B, block: Block) -> u8 {
    let v = bus.port_in(cpu.bc());
    bus.write(cpu.hl(), v);
    cpu.wz = cpu.bc().wrapping_add(block.step());
    cpu.b = cpu.b.wrapping_sub(1);
    cpu.set_hl(cpu.hl().wrapping_add(block.step()));
    // The addend is `C` stepped in the instruction's own direction.
    let addend = if block.inc {
        cpu.c.wrapping_add(1)
    } else {
        cpu.c.wrapping_sub(1)
    };
    block_io_flags(cpu, v, addend);
    v
}

/// `OUTI` / `OUTD` / `OTIR` / `OTDR`: write `(HL)` to a port, step `HL`, decrement `B`.
///
/// `B` is decremented **before** the port write, so the write lands on
/// `BC - 0x100` — 1,000 of 1,000 cases on each of the four files, and never `BC`.
/// The ordering is observable in the port log and is the mirror image of
/// [`ini_ind`], where the read happens first.
///
/// The flag addend is the **new** `L`, after `HL` has stepped, where the `IN` forms
/// use a stepped `C`. Two different registers for the same slot in the same formula.
///
/// Returns the byte transferred, which the repeating forms need for
/// `block_io_repeat_adjust`.
pub fn outi_outd<B: Bus>(cpu: &mut Z80, bus: &mut B, block: Block) -> u8 {
    let v = bus.read(cpu.hl());
    cpu.b = cpu.b.wrapping_sub(1);
    bus.port_out(cpu.bc(), v);
    cpu.wz = cpu.bc().wrapping_add(block.step());
    cpu.set_hl(cpu.hl().wrapping_add(block.step()));
    block_io_flags(cpu, v, cpu.l);
    v
}

/// The block-I/O flag set, shared by all eight forms.
///
/// Undocumented by Zilog and reconstructed by the community from hardware; the
/// eight `ed_a2`-family files are what pins it here. S, Z, F3 and F5 come from the
/// decremented `B`, N from bit 7 of the byte transferred, H and C together from
/// whether `v + addend` carries out of eight bits, and P/V from the parity of the
/// low three bits of that sum XORed with `B`.
///
/// Measured 0 wrong over 1,000 cases on each of the four non-repeating files. The
/// repeating forms need this **plus** `block_io_repeat_adjust`.
fn block_io_flags(cpu: &mut Z80, v: u8, addend: u8) {
    let sum = u16::from(v) + u16::from(addend);
    cpu.f = flags::sz53(cpu.b)
        | if v & 0x80 != 0 { N } else { 0 }
        | if sum > 0xFF { H | C } else { 0 }
        | if flags::parity(((sum & 7) as u8) ^ cpu.b) {
            PV
        } else {
            0
        };
    cpu.q = cpu.f;
}

/// The extra H and P/V rules the four **repeating** block-I/O forms need.
///
/// Community-documented as the "Boo-boo" adjustment. When the carry out of
/// `v + addend` is set, H and P/V are recomputed from `B` stepped one further in the
/// direction bit 7 of the transferred byte selects; when it is clear, P/V is
/// recomputed from `B` alone and H is left as [`block_io_flags`] wrote it.
///
/// Two facts about where this applies, both measured:
///
/// - It is **only** for the repeating forms. Applying it to `INI` breaks 727 of
///   `ed_a2`'s 1,000 cases, where the plain flag set is wrong on none.
/// - It is not sufficient alone. With this and F3/F5 from the rewound `PC`'s high
///   byte — `super::repeat` — the four repeating files are wrong on 0 of 995, 0 of
///   996, 0 of 999 and 0 of 1,000 repeating cases. Testing `PC`'s high byte *without*
///   this adjustment is what made the rule look wrong on roughly a quarter of cases.
///
/// Call it before `super::repeat`, which then overwrites F3 and F5. The `b` this
/// reads is the already-decremented one.
pub(crate) fn block_io_repeat_adjust(cpu: &mut Z80, v: u8) {
    let b = cpu.b;
    let (pv_source, h) = if cpu.f & C != 0 {
        if v & 0x80 != 0 {
            (b.wrapping_sub(1), if b & 0x0F == 0x00 { H } else { 0 })
        } else {
            (b.wrapping_add(1), if b & 0x0F == 0x0F { H } else { 0 })
        }
    } else {
        (b, cpu.f & H)
    };
    // The adjustment XORs a parity into P/V rather than replacing it: the value
    // being corrected is `parity((sum & 7) ^ B)`, and XORing in the parity of the
    // stepped `B`'s low three bits swaps that `B` term for the stepped one. The
    // trailing `^ PV` is the sign of the swap, and dropping it inverts P/V on every
    // repeating case.
    let stepped_parity = if flags::parity(pv_source & 7) { PV } else { 0 };
    let pv = (cpu.f & PV) ^ stepped_parity ^ PV;
    cpu.f = (cpu.f & !(PV | H)) | pv | h;
    cpu.q = cpu.f;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::{F3, F5, S, Z};
    use crate::testbus::Mem;

    /// `IN r,(C)` uses all sixteen bits of `BC` as the port and latches `BC + 1`.
    ///
    /// `B` on the high half is what a core that used `C` alone gets wrong, and a
    /// port of 0x1234 distinguishes the two where a `B` of zero could not.
    #[test]
    fn in_from_c_addresses_the_port_with_all_of_bc_and_latches_bc_plus_one() {
        let mut c = Z80::new();
        c.b = 0x12;
        c.c = 0x34;
        c.wz = 0x5EED;
        let mut m = Mem::new();
        m.port_in_value = 0x5A;
        assert_eq!(in_r_c(&mut c, &mut m), 0x5A);
        assert_eq!(m.ports_in, vec![0x1234], "BC, not C alone");
        assert_eq!(c.wz, 0x1235, "and the latch is BC + 1");
    }

    /// `IN r,(C)` flags the byte it read and preserves only carry.
    #[test]
    fn in_from_c_flags_the_byte_and_preserves_only_carry() {
        let mut c = Z80::new();
        c.f = 0xFF;
        let mut m = Mem::new();
        m.port_in_value = 0x00;
        assert_eq!(in_r_c(&mut c, &mut m), 0x00);
        assert_eq!(c.f, Z | PV | C, "zero: Z, even parity, and carry survives");

        c.f = 0;
        m.port_in_value = 0x80;
        assert_eq!(in_r_c(&mut c, &mut m), 0x80);
        assert_eq!(c.f, S, "0x80: sign, odd parity, no carry to keep");
        assert_eq!(c.q, c.f);
    }

    /// `OUT (C),r` latches `BC + 1` — not the written byte, as `OUT (n),A` does.
    ///
    /// The written byte is 0x99 while `BC`'s high half is 0x12, so a core sharing the
    /// base page's `wz_after_write` rule gives 0x9935 here instead of 0x1235.
    #[test]
    fn out_to_c_latches_bc_plus_one_rather_than_the_written_byte() {
        let mut c = Z80::new();
        c.b = 0x12;
        c.c = 0x34;
        c.f = 0x5A;
        let mut m = Mem::new();
        out_c_r(&mut c, &mut m, 0x99);
        assert_eq!(m.ports_out, vec![(0x1234, 0x99)]);
        assert_eq!(c.wz, 0x1235, "BC + 1, not 0x9935");
        assert_eq!(c.f, 0x5A, "and no flags");
        assert_eq!(c.q, 0);
    }

    /// `INI` reads the port at the **old** `BC` and writes to the old `HL`.
    ///
    /// `B` is 0x12 going in, so an implementation that decremented first would read
    /// port 0x1134. The vectors show the old `BC` on 1,000 of 1,000 cases of each
    /// `IN` block file, and the decremented one on none.
    #[test]
    fn ini_reads_the_port_before_decrementing_b() {
        let mut c = Z80::new();
        c.b = 0x12;
        c.c = 0x34;
        c.set_hl(0x2000);
        let mut m = Mem::new();
        m.port_in_value = 0x5A;
        ini_ind(&mut c, &mut m, Block::from_opcode(0xA2));
        assert_eq!(m.ports_in, vec![0x1234], "the old BC, with B undecremented");
        assert_eq!(m.writes, vec![(0x2000, 0x5A)], "written at the old HL");
        assert_eq!(c.b, 0x11, "and only then is B decremented");
        assert_eq!(c.hl(), 0x2001);
        assert_eq!(
            c.wz, 0x1235,
            "the latch follows the address on the bus, so the old BC plus one"
        );
    }

    /// `IND` steps `HL` and the latch down where `INI` steps them up.
    #[test]
    fn ind_steps_downwards() {
        let mut c = Z80::new();
        c.b = 0x12;
        c.c = 0x34;
        c.set_hl(0x2000);
        let mut m = Mem::new();
        ini_ind(&mut c, &mut m, Block::from_opcode(0xAA));
        assert_eq!(c.hl(), 0x1FFF);
        assert_eq!(c.wz, 0x1233, "the old BC, minus one");
    }

    /// The two families latch different `B`s, and that is the whole asymmetry.
    ///
    /// Same registers into both: `INI` latches from `BC` with `B` at 0x12 and `OUTI`
    /// from `BC` with `B` at 0x11, because one reads its port before the decrement and
    /// the other writes after. A core that used one ordering for both would agree with
    /// exactly one of these two lines — which is why they are asserted together
    /// rather than in the per-family tests above.
    #[test]
    fn the_two_block_io_families_latch_from_different_sides_of_the_decrement() {
        let mut c = Z80::new();
        c.b = 0x12;
        c.c = 0x34;
        c.set_hl(0x2000);
        let mut m = Mem::new();
        ini_ind(&mut c, &mut m, Block::from_opcode(0xA2));
        assert_eq!(c.wz, 0x1235, "INI: before the decrement");

        let mut c = Z80::new();
        c.b = 0x12;
        c.c = 0x34;
        c.set_hl(0x2000);
        let mut m = Mem::new();
        outi_outd(&mut c, &mut m, Block::from_opcode(0xA3));
        assert_eq!(c.wz, 0x1135, "OUTI: after it");
    }

    /// `OUTI` writes the port at `BC - 0x100`, after reading `(HL)`.
    ///
    /// The mirror of [`ini_ind`]: here `B` is decremented *before* the port access,
    /// so the address carries 0x11 and not 0x12. Both orderings pass a test that only
    /// checks the byte, which is why the address is what is asserted.
    #[test]
    fn outi_writes_the_port_after_decrementing_b() {
        let mut c = Z80::new();
        c.b = 0x12;
        c.c = 0x34;
        c.set_hl(0x2000);
        let mut m = Mem::new();
        m.ram[0x2000] = 0x5A;
        outi_outd(&mut c, &mut m, Block::from_opcode(0xA3));
        assert_eq!(
            m.ports_out,
            vec![(0x1134, 0x5A)],
            "BC - 0x100: B was decremented first, unlike INI"
        );
        assert_eq!(c.b, 0x11);
        assert_eq!(c.hl(), 0x2001);
        assert_eq!(c.wz, 0x1135);
    }

    /// The block-I/O flag set: `B` supplies S/Z/F3/F5, the byte supplies N, and the
    /// sum supplies H and C together.
    ///
    /// H and C are one condition here, which no other instruction on the chip does —
    /// so a core that computed them separately, as every other arm must, would set
    /// one without the other.
    #[test]
    fn the_block_io_flags_take_h_and_c_together_from_the_sum() {
        let mut c = Z80::new();
        c.b = 0x28;
        block_io_flags(&mut c, 0x00, 0x00);
        assert_eq!(c.f & (F5 | F3), F5 | F3, "0x28's bits 5 and 3, from B");
        assert_eq!(c.f & (S | Z), 0, "B is neither zero nor negative");
        assert_eq!(c.f & N, 0, "the byte's bit 7 is clear");
        assert_eq!(c.f & (H | C), 0, "0 + 0 does not carry");

        // 0xFF + 0x01 carries out of eight bits: H and C both.
        c.b = 0x00;
        block_io_flags(&mut c, 0xFF, 0x01);
        assert_eq!(c.f & (H | C), H | C, "one condition, two flags");
        assert_eq!(c.f & N, N, "0xFF's bit 7 is set");
        assert_eq!(c.f & Z, Z, "B is zero");
        assert_eq!(c.q, c.f);
    }

    /// The repeat adjustment moves H and P/V and touches nothing else.
    ///
    /// With carry clear it recomputes P/V from `B` alone and leaves H; with carry set
    /// it steps `B` by the direction bit 7 of the byte selects, and derives H from
    /// `B`'s low nibble. The two branches are asserted separately because the
    /// carry-clear branch is a no-op on P/V whenever the sum's low bits were zero,
    /// which is most of the interesting cases.
    #[test]
    fn the_repeat_adjustment_only_moves_h_and_parity() {
        // Carry clear: P/V becomes the parity of B's low three bits, H survives.
        let mut c = Z80::new();
        c.b = 0x07; // three bits set in the low three: odd parity, so P/V clears
        c.f = S | Z | H | PV | N;
        block_io_repeat_adjust(&mut c, 0x00);
        assert_eq!(c.f & PV, 0, "0x07's low three bits are odd");
        assert_eq!(c.f & H, H, "and H is untouched with carry clear");
        assert_eq!(c.f & (S | Z | N), S | Z | N, "nothing else moves");

        // Carry set, byte positive: B steps up, and H comes from B's low nibble
        // being 0x0F.
        let mut c = Z80::new();
        c.b = 0x0F;
        c.f = C | PV;
        block_io_repeat_adjust(&mut c, 0x00);
        assert_eq!(
            c.f & H,
            H,
            "B's low nibble is 0x0F and the byte is positive"
        );
        assert_eq!(c.f & C, C, "carry itself is not part of the adjustment");

        // Carry set, byte negative: B steps down, and H comes from a low nibble of 0.
        let mut c = Z80::new();
        c.b = 0x10;
        c.f = C;
        block_io_repeat_adjust(&mut c, 0x80);
        assert_eq!(c.f & H, H, "B's low nibble is 0 and the byte is negative");

        let mut c = Z80::new();
        c.b = 0x11;
        c.f = C | H;
        block_io_repeat_adjust(&mut c, 0x80);
        assert_eq!(
            c.f & H,
            0,
            "and H is cleared when the nibble does not match"
        );
        assert_eq!(c.q, c.f);
    }
}
