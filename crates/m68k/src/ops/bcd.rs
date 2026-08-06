//! `ABCD`, `SBCD` and `NBCD` — packed binary-coded decimal.
//!
//! Two packed decimal digits per byte, with the `X` flag carrying between
//! chained operations. `C` is set on a decimal carry or borrow, and `X` always
//! copies `C`.
//!
//! # The manual says N and V are undefined. They are not.
//!
//! All three groups are exact on the result *and* on every flag including `N`
//! and `V`, at full corpus size — 7,500 cases with no residual, measured through
//! both the register and the memory path:
//!
//! ```text
//! ABCD 2500/2500      SBCD 2500/2500      NBCD 2500/2500
//! ```
//!
//! So do not implement `V = 0` (that fits about 65%) and do not leave `N` and
//! `V` preserved. `V` is *directional* and is an artifact of the decimal fixup
//! hardware:
//!
//! | group | `V` is set when the uncorrected MSB went | `V=1` share |
//! |-------|------------------------------------------|-------------|
//! | ABCD  | 0 → 1                                    | 360 / 1256  |
//! | SBCD  | 1 → 0                                    | 299 / 1230  |
//! | NBCD  | 1 → 0                                    | 165 / 403   |
//!
//! "Uncorrected" means the plain binary `a + b + x` / `a - b - x` truncated to
//! `u8`, before either decimal correction. A direction-agnostic "the MSB flipped"
//! rule scores 753/1256 on `ABCD`.
//!
//! # The two models are NOT mirror images
//!
//! `ABCD` needs a **nibble-wise** carry model and `SBCD`/`NBCD` need a
//! **byte-wise** borrow model. Each regresses the other family: byte-wise `ABCD`
//! scores 1240/1256, nibble-wise `SBCD` scores 1222/1230, and unifying them
//! behind a sign flag cost `NBCD` 391 of its 403 cases. They are written as two
//! functions on purpose.
//!
//! Both first attempts were off by exactly `0x60`, from gating the high fixup on
//! `sum > 0x99` — which fires when the *low* nibble's `+6` pushes the total past
//! `0x99` without the high digit itself exceeding 9. Generalising: a miss set
//! off by one constant offset is a correction-condition bug, not a failed model,
//! and the constant names the correction — `0x60` the decimal fixup, `0x10` the
//! carry width.
//!
//! # Z accumulates
//!
//! `Z_final = (result == 0) && Z_initial`: these three can only ever *clear* `Z`,
//! never set it. [`crate::flags::accumulate_z`] implements it. The rule is not vacuous
//! here — `ABCD` has 3 discriminating cases, `SBCD` 44 and `NBCD` 3, and the
//! own-result rule scores 0 on every one of them.
//!
//! # Provenance
//!
//! The suite was generated from MAME's microcode, so these are MAME-derived
//! rather than from silicon. That is a weaker claim than measurement against
//! hardware — but 7,500/7,500 on flags the manual calls undefined is much
//! stronger evidence than "matches whatever it reports".

use super::alu::{self, Plan};
use super::arith::valid_data_alterable;
use crate::cpu::{M68k, ADDR_MASK};
use crate::decode::Handler;
use crate::ea::{self, Ea, Size};
use crate::flags::accumulate_z;
use crate::Bus;

/// `ABCD`: `a + b + X`, decimal. Returns `(result, carry)`; `X` is the carry too.
///
/// **Nibble-wise**, and the low nibble's carry into the high one can be **2**,
/// not just 1: `0xF + 0xF + 1 = 0x1F`, and `+6` makes `0x25`. Taking `lo >> 4`
/// is what lets that fall out; masking with `-= 0x10` instead scores 1173/1256.
fn abcd(a: u8, b: u8, x: bool) -> (u8, bool) {
    let mut lo = (a & 0x0F) as u16 + (b & 0x0F) as u16 + x as u16;
    if lo > 9 {
        lo += 6;
    }
    let mut hi = (a >> 4) as u16 + (b >> 4) as u16 + (lo >> 4);
    if hi > 9 {
        hi += 6;
    }
    ((((hi & 0xF) << 4) | (lo & 0xF)) as u8, hi >= 0x10)
}

/// `SBCD`: `a - b - X`, decimal. Returns `(result, borrow)`; `X` is the borrow.
///
/// **Byte-wise**, and not the mirror of [`abcd`]. Both corrections are decided
/// from the *uncorrected* subtraction and then applied to the byte; the
/// nibble-wise form fails because it propagates the low nibble's `-6` as a borrow
/// into the high nibble, which the hardware does not do.
///
/// Two details are load-bearing:
///
/// - `binary_borrow` comes from the **byte-wide** difference, not from the high
///   nibble's sign. The low nibble's `-6` can itself drive the high nibble
///   negative with no real byte borrow; gating on the nibble scores 1222/1230.
/// - `C` is `res < 0` *after* both fixups, not `binary_borrow` alone — the
///   `-0x60` can push a non-borrowing subtraction below zero.
fn sbcd(a: u8, b: u8, x: bool) -> (u8, bool) {
    let lo_borrow = ((a & 0x0F) as i16 - (b & 0x0F) as i16 - (x as i16)) < 0;
    let binary_borrow = (a as i16) - (b as i16) - (x as i16) < 0;
    let mut res = (a as i16) - (b as i16) - (x as i16);
    if lo_borrow {
        res -= 6;
    }
    if binary_borrow {
        res -= 0x60;
    }
    (res as u8, res < 0)
}

/// Sets the CCR for a completed BCD operation and returns the result.
///
/// `uncorrected` is the plain binary result before either decimal fixup, which is
/// what `V` is measured against. `msb_set_means_v` selects the direction: `ABCD`
/// looks for 0 → 1, `SBCD`/`NBCD` for 1 → 0.
fn finish(cpu: &mut M68k, res: u8, carry: bool, uncorrected: u8, adding: bool) -> u32 {
    let v = if adding {
        (!uncorrected & res) & 0x80 != 0
    } else {
        (uncorrected & !res) & 0x80 != 0
    };
    cpu.set_ccr(
        carry,
        res & 0x80 != 0,
        accumulate_z(res == 0, cpu.ccr_z()),
        v,
        carry,
    );
    res as u32
}

/// `ABCD`/`SBCD`, both forms.
///
/// Bit 3 — **not** the mode field — selects the memory form: `Dy,Dx` when clear,
/// `-(Ay),-(Ax)` when set. These share opcode lines with `AND`/`OR`/`EXG`, so the
/// pattern must be pinned whole (`op & 0xF1F8 == 0xC100` / `0x8100`); reading
/// bits 5-3 as a mode field decodes `-(Ay),-(Ax)` as `An` direct.
///
/// The **source is read first**, then the destination. Reversing it scores `SBCD`
/// 15/1270; `ABCD` ties both ways because addition is commutative, so a green
/// `ABCD` is not evidence that the order is right.
fn bcd_two_operand(cpu: &mut M68k, bus: &mut dyn Bus, op: u16, adding: bool) -> u32 {
    let rx = ((op >> 9) & 7) as usize;
    let ry = (op & 7) as usize;
    let memory = op & 8 != 0;

    if !memory {
        let dst = cpu.d[rx] as u8;
        let src = cpu.d[ry] as u8;
        let (res, carry) = if adding {
            abcd(dst, src, cpu.ccr_x())
        } else {
            sbcd(dst, src, cpu.ccr_x())
        };
        let uncorrected = if adding {
            dst.wrapping_add(src).wrapping_add(cpu.ccr_x() as u8)
        } else {
            dst.wrapping_sub(src).wrapping_sub(cpu.ccr_x() as u8)
        };
        let res = finish(cpu, res, carry, uncorrected, adding);
        cpu.d[rx] = (cpu.d[rx] & !0xFF) | res;
        // Shape `P i`: one queue advance, 2 idle.
        cpu.consume_opcode_dyn(bus);
        return 4 + 2;
    }

    // Shape `i R R P W`. Both predecrements happen before either read, and the
    // source's lands first — observable when both name the same register, where
    // the two operands then come from different addresses.
    //
    // `ea::resolve` rather than a hand-rolled subtraction, so the byte-through-A7
    // step of 2 comes from one place.
    let Ea::Mem(src_addr) = ea::resolve(cpu, 4, ry as u16, Size::Byte, &[], 0) else {
        unreachable!("mode 4 always resolves to memory")
    };
    let Ea::Mem(dst_addr) = ea::resolve(cpu, 4, rx as u16, Size::Byte, &[], 0) else {
        unreachable!("mode 4 always resolves to memory")
    };

    let src = bus.read8(src_addr & ADDR_MASK);
    let dst = bus.read8(dst_addr & ADDR_MASK);
    let (res, carry) = if adding {
        abcd(dst, src, cpu.ccr_x())
    } else {
        sbcd(dst, src, cpu.ccr_x())
    };
    let uncorrected = if adding {
        dst.wrapping_add(src).wrapping_add(cpu.ccr_x() as u8)
    } else {
        dst.wrapping_sub(src).wrapping_sub(cpu.ccr_x() as u8)
    };
    let res = finish(cpu, res, carry, uncorrected, adding);
    cpu.consume_opcode_dyn(bus);
    bus.write8(dst_addr & ADDR_MASK, res as u8);
    // 2 operand reads + 1 queue advance + 1 write = 4 accesses, plus 2 idle.
    // The opcode's own fetch is not counted: the previous instruction's queue
    // advance performed it, which is why the register form above costs 4 and not
    // 8. Getting this wrong shows up as a uniform +4, the timing law's signature
    // for a miscounted access.
    4 * 4 + 2
}

/// `NBCD <ea>`: ten's complement negate, `0 - Dn - X`.
///
/// The operand order is **`(0, Dn)`**, not `(Dn, 0)`. Reversed it scores 40/2097
/// on the memory forms, and its `V` rule looks split rather than unanimous —
/// which was a measurement bug of the controller's, not a property of the
/// hardware.
///
/// Note the consequence: `NBCD` of 0 with `X` clear yields 0 with `C` **clear**,
/// and `Z` follows the accumulating rule like the others.
fn nbcd(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let (mode, reg) = ((op >> 3) & 7, op & 7);
    // The register form's idle is 2 and the memory forms' is 0, plus the
    // standard +2 that `alu` adds for modes 4, 6 and (7,3).
    let idle = if mode == 0 { 2 } else { 0 };
    let plan = Plan::new(Size::Byte, mode, reg).writes().idle(idle);
    alu::run(cpu, bus, &plan, &mut |cpu, ops| {
        let a = ops.ea as u8;
        let (res, carry) = sbcd(0, a, cpu.ccr_x());
        let uncorrected = 0u8.wrapping_sub(a).wrapping_sub(cpu.ccr_x() as u8);
        Some(finish(cpu, res, carry, uncorrected, false))
    })
}

// --- Dispatch-table installation ------------------------------------------

fn abcd_h(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    bcd_two_operand(cpu, bus, op, true)
}

fn sbcd_h(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    bcd_two_operand(cpu, bus, op, false)
}

/// Installs `ABCD`, `SBCD` and `NBCD`.
///
/// ```text
/// 1100 xxx1 0000 Myyy   ABCD   line C, opmode 4, M = bit 3
/// 1000 xxx1 0000 Myyy   SBCD   line 8, opmode 4
/// 0100 1000 00mm mrrr   NBCD   line 4, selector 8
/// ```
///
/// `ABCD`/`SBCD` occupy only bits 5-4 == `00` of their opmode-4 slot; bits 5-4 of
/// `01`, `10` and `11` there are `EXG` and belong to another task, so this
/// installs 16 opcodes per instruction and leaves the rest alone.
///
/// `NBCD` is data-alterable: no `An`, no PC-relative, no immediate.
pub(super) fn register(table: &mut [Handler; 65536]) {
    for x in 0..8u16 {
        for y in 0..8u16 {
            for m in 0..2u16 {
                let slot = (x << 9) | (1 << 8) | (m << 3) | y;
                table[(0xC000 | slot) as usize] = abcd_h;
                table[(0x8000 | slot) as usize] = sbcd_h;
            }
        }
    }
    for mode in 0..8u16 {
        for reg in 0..8u16 {
            if !valid_data_alterable(mode, reg) {
                continue;
            }
            table[(0x4800 | (mode << 3) | reg) as usize] = nbcd;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::{FlatBus, RecordingBus};
    use crate::cpu::{SR_S, SR_X, SR_Z};
    use crate::decode::Decoder;

    fn run_one(cpu: &mut M68k, bus: &mut impl Bus, words: &[u16], at: u32) -> u32 {
        for (i, w) in words.iter().enumerate() {
            bus.write16(at + 2 * i as u32, *w);
        }
        cpu.pc = at;
        cpu.prime_prefetch(bus);
        let dec = Decoder::new();
        cpu.step_with(&dec, bus)
    }

    fn cpu_at(sr: u16) -> M68k {
        let mut cpu = M68k::new();
        cpu.sr = sr;
        cpu.a[7] = 0x3000;
        cpu.ssp = 0x3000;
        cpu
    }

    #[test]
    fn abcd_adds_decimal_digits_with_a_carry_between_them() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S | SR_Z);
        cpu.d[0] = 0x28;
        cpu.d[1] = 0x34;
        // ABCD D1,D0 => 0x62
        let cycles = run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x62);
        assert!(!cpu.ccr_c() && !cpu.ccr_x());
        assert_eq!(cycles, 4 + 2);
    }

    #[test]
    fn abcd_sets_c_and_x_on_a_decimal_carry_out() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x99;
        cpu.d[1] = 0x01;
        run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x00);
        assert!(cpu.ccr_c() && cpu.ccr_x(), "carry out of the high digit");
    }

    /// The low nibble's carry into the high one can be 2: `0xF + 0xF + 1 = 0x1F`,
    /// `+6 = 0x25`. A model that masks with `-= 0x10` gets `0x15` here.
    #[test]
    fn abcd_propagates_a_carry_of_two_out_of_the_low_nibble() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S | SR_X);
        cpu.d[0] = 0x0F;
        cpu.d[1] = 0x0F;
        run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        // lo = 0xF+0xF+1 = 0x1F, +6 = 0x25 -> digit 5, carry 2.
        // hi = 0 + 0 + 2 = 2.
        assert_eq!(cpu.d[0], 0x25);
    }

    /// `V` is set exactly when the decimal correction drove bit 7 from 0 to 1 —
    /// a fact the manual calls undefined.
    #[test]
    fn abcd_sets_v_when_the_fixup_drives_bit_7_high() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        // 0x79 + 0x09: binary 0x82 already has bit 7, so no 0 -> 1 transition.
        cpu.d[0] = 0x79;
        cpu.d[1] = 0x09;
        run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x88);
        assert!(!cpu.ccr_v(), "uncorrected 0x82 already had bit 7 set");

        // 0x40 + 0x40: binary 0x80 has bit 7; corrected 0x80 too. Still no
        // 0 -> 1 by the fixup.
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x74;
        cpu.d[1] = 0x12;
        run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        // binary 0x86, corrected 0x86; bit 7 set in both.
        assert_eq!(cpu.d[0], 0x86);
        assert!(!cpu.ccr_v());

        // 0x49 + 0x29: binary 0x72 (bit 7 clear), decimal 0x78 — still clear.
        // 0x49 + 0x39: binary 0x82, decimal 0x88.
        // The 0 -> 1 case needs the +0x60 to push past 0x7F:
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x30;
        cpu.d[1] = 0x40;
        run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x70);
        assert!(!cpu.ccr_v());

        // 0x45 + 0x35 = binary 0x7A (bit 7 clear), decimal 0x80 (bit 7 set):
        // fixup drove bit 7 from 0 -> 1 => V must be set.
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x45;
        cpu.d[1] = 0x35;
        run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x80);
        assert!(cpu.ccr_v(), "fixup drove bit 7 from 0 to 1: V must be set");
    }

    #[test]
    fn sbcd_subtracts_decimal_digits() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x62;
        cpu.d[1] = 0x34;
        // SBCD D1,D0 => 0x28
        run_one(&mut cpu, &mut bus, &[0x8101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x28);
        assert!(!cpu.ccr_c() && !cpu.ccr_x());
    }

    #[test]
    fn sbcd_borrows_across_the_byte() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x00;
        cpu.d[1] = 0x01;
        run_one(&mut cpu, &mut bus, &[0x8101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x99);
        assert!(cpu.ccr_c() && cpu.ccr_x(), "borrow out of the byte");
    }

    /// The byte-wide borrow, not the high nibble's sign, decides the `-0x60`.
    /// `0x10 - 0x01` needs the low fixup, and that fixup alone drives the high
    /// nibble negative without a real byte borrow.
    #[test]
    fn sbcd_decides_the_high_fixup_from_the_byte_not_the_nibble() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x10;
        cpu.d[1] = 0x01;
        run_one(&mut cpu, &mut bus, &[0x8101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x09, "no -0x60: the byte did not borrow");
        assert!(!cpu.ccr_c());
    }

    /// `Z` can only be cleared. Starting from `Z` clear, a zero result leaves it
    /// clear — the opposite of every non-BCD instruction.
    #[test]
    fn bcd_z_accumulates_and_never_sets() {
        let mut bus = FlatBus::new();
        // Z initially clear, result 0: Z stays clear.
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x00;
        cpu.d[1] = 0x00;
        run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0);
        assert!(!cpu.ccr_z(), "a zero result must NOT set Z");

        // Z initially set, result 0: Z stays set.
        let mut cpu = cpu_at(SR_S | SR_Z);
        cpu.d[0] = 0x00;
        cpu.d[1] = 0x00;
        run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        assert!(cpu.ccr_z());

        // Z initially set, nonzero result: cleared.
        let mut cpu = cpu_at(SR_S | SR_Z);
        cpu.d[0] = 0x01;
        cpu.d[1] = 0x00;
        run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        assert!(!cpu.ccr_z());
    }

    #[test]
    fn abcd_and_sbcd_include_x_as_the_incoming_carry() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S | SR_X);
        cpu.d[0] = 0x10;
        cpu.d[1] = 0x10;
        run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x21, "X added in");

        let mut cpu = cpu_at(SR_S | SR_X);
        cpu.d[0] = 0x20;
        cpu.d[1] = 0x10;
        run_one(&mut cpu, &mut bus, &[0x8101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x09, "X borrowed out");
    }

    /// The memory form predecrements **both** registers before either read, with
    /// the source's decrement first. When both name the same register the two
    /// operands come from different addresses, and the order is what decides
    /// which.
    #[test]
    fn bcd_memory_form_predecrements_both_before_reading() {
        let mut bus = RecordingBus::new();
        bus.put16(0x2000, 0x1234);
        let mut cpu = cpu_at(SR_S);
        cpu.a[3] = 0x2002;
        // SBCD -(A3),-(A3): source at 0x2001, destination at 0x2000.
        cpu.prime_prefetch(&mut bus);
        run_one(&mut cpu, &mut bus, &[0x870B, 0x4E71], 0x1000);
        assert_eq!(cpu.a[3], 0x2000, "two decrements");
        let reads: Vec<u32> = bus
            .log
            .iter()
            .filter(|(write, addr, _)| !*write && (0x2000..0x2010).contains(addr))
            .map(|(_, addr, _)| *addr)
            .collect();
        assert_eq!(
            reads,
            vec![0x2001, 0x2000],
            "source first, then destination"
        );
    }

    #[test]
    fn bcd_memory_form_costs_four_accesses_and_two_idle() {
        let mut bus = FlatBus::new();
        bus.put16(0x2000, 0x2828);
        let mut cpu = cpu_at(SR_S);
        cpu.a[1] = 0x2001;
        cpu.a[2] = 0x2002;
        // ABCD -(A1),-(A2)
        let cycles = run_one(&mut cpu, &mut bus, &[0xC509, 0x4E71], 0x1000);
        assert_eq!(cycles, 4 * 4 + 2);
        assert_eq!(bus.read8(0x2001), 0x56, "0x28 + 0x28");
    }

    /// `NBCD` is `0 - Dn - X`, not `Dn - 0 - X`.
    #[test]
    fn nbcd_negates_in_tens_complement() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x01;
        let cycles = run_one(&mut cpu, &mut bus, &[0x4800, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x99, "ten's complement of 1");
        assert!(cpu.ccr_c() && cpu.ccr_x());
        assert_eq!(cycles, 4 + 2);
    }

    /// `NBCD` of zero with `X` clear is zero with `C` **clear** — the one case
    /// where nothing borrows.
    #[test]
    fn nbcd_of_zero_clears_c() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S | SR_Z);
        cpu.d[0] = 0x00;
        run_one(&mut cpu, &mut bus, &[0x4800, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x00);
        assert!(!cpu.ccr_c() && !cpu.ccr_x(), "nothing borrowed");
        assert!(cpu.ccr_z(), "Z was already set and the result is zero");
    }

    #[test]
    fn nbcd_with_x_set_borrows_one_more() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S | SR_X);
        cpu.d[0] = 0x00;
        run_one(&mut cpu, &mut bus, &[0x4800, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x99);
        assert!(cpu.ccr_c());
    }

    #[test]
    fn nbcd_writes_a_memory_operand_back() {
        let mut bus = FlatBus::new();
        bus.put16(0x2000, 0x2500);
        let mut cpu = cpu_at(SR_S);
        cpu.a[1] = 0x2000;
        // NBCD (A1)
        let cycles = run_one(&mut cpu, &mut bus, &[0x4811, 0x4E71], 0x1000);
        assert_eq!(bus.read8(0x2000), 0x75, "100 - 25");
        // 1 operand read + 1 queue advance + 1 write, no idle.
        assert_eq!(cycles, 4 * 3);
    }

    /// `NBCD` preserves the bits above the byte in a data register.
    #[test]
    fn nbcd_touches_only_the_low_byte() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0xDEAD_BE01;
        run_one(&mut cpu, &mut bus, &[0x4800, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0xDEAD_BE99);
    }

    /// The `-(An)` forms must not be decoded through the mode field: bit 3
    /// selects them, and bits 5-3 read as a mode would say `An` direct.
    #[test]
    fn bit_3_not_the_mode_field_selects_the_memory_form() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x11;
        cpu.d[1] = 0x11;
        cpu.a[1] = 0x2010;
        // 0xC101 is the register form; 0xC109 the memory one. Same mode bits.
        run_one(&mut cpu, &mut bus, &[0xC101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0], 0x22);
        assert_eq!(cpu.a[1], 0x2010, "the register form touches no An");
    }

    /// `V`'s direction is opposite between add and subtract; a
    /// direction-agnostic "MSB flipped" rule fits neither family.
    ///
    /// Both a positive (V=1) and negative (V=0) case for each family, so
    /// replacing `finish`'s `v` with `false` or swapping the direction between
    /// add and subtract will each break at least one assertion.
    #[test]
    fn v_is_directional_between_abcd_and_sbcd() {
        // ABCD V=1: 0x45 + 0x35 — binary 0x7A (bit 7 clear), decimal 0x80
        // (bit 7 set). The fixup drove bit 7 from 0 to 1.
        let (res, _) = abcd(0x45, 0x35, false);
        let bin = 0x45u8.wrapping_add(0x35);
        assert_eq!(res, 0x80);
        assert_eq!(bin, 0x7A, "uncorrected bit 7 clear");
        assert!(
            (!bin & res) & 0x80 != 0,
            "0 -> 1 transition: ABCD V must be set"
        );

        // ABCD V=0: 0x79 + 0x09 — both uncorrected (0x82) and corrected (0x88)
        // have bit 7 set: no 0 -> 1 transition.
        let (res2, _) = abcd(0x79, 0x09, false);
        let bin2 = 0x79u8.wrapping_add(0x09);
        assert_eq!(res2, 0x88);
        assert_eq!(bin2, 0x82);
        assert!(
            (!bin2 & res2) & 0x80 == 0,
            "both have bit 7: not a 0 -> 1 transition"
        );

        // SBCD V=1: 0x00 - 0x80 — binary 0x80 (bit 7 set after wrapping),
        // corrected 0x20 (the -0x60 byte-borrow correction drives bit 7 clear).
        // The fixup drove bit 7 from 1 to 0.
        let (sbcd_res, _) = sbcd(0x00, 0x80, false);
        let sbcd_bin = 0x00u8.wrapping_sub(0x80);
        assert_eq!(sbcd_bin, 0x80, "uncorrected bit 7 set");
        assert_eq!(sbcd_res, 0x20, "fixup cleared bit 7");
        assert!(
            (sbcd_bin & !sbcd_res) & 0x80 != 0,
            "1 -> 0 transition: SBCD V must be set"
        );

        // SBCD: 0x00 - 0x81 — binary 0x7F (bit 7 clear), corrected also clear.
        // Uncorrected bit 7 is clear: no 1 -> 0 transition => V clear.
        let (res, _) = sbcd(0x00, 0x81, false);
        let bin = 0x00u8.wrapping_sub(0x81);
        assert_eq!(bin, 0x7F, "uncorrected bit 7 clear");
        assert!(
            (bin & !res) & 0x80 == 0,
            "no 1 -> 0 transition: V must be clear"
        );
    }

    /// End-to-end pin for SBCD V=1 through the full CPU pipeline (i.e. through
    /// `finish`). 0x00 - 0x80: binary 0x80 (bit 7 set), corrected 0x20 (bit 7
    /// cleared by the -0x60 fixup) => V must be set.
    #[test]
    fn sbcd_sets_v_when_the_fixup_drives_bit_7_low() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x00;
        cpu.d[1] = 0x80;
        // SBCD D1,D0
        run_one(&mut cpu, &mut bus, &[0x8101, 0x4E71], 0x1000);
        assert_eq!(cpu.d[0] & 0xFF, 0x20, "0x00 - 0x80 decimal = 0x20");
        assert!(cpu.ccr_v(), "fixup drove bit 7 from 1 to 0: V must be set");
    }

    /// The two models are not interchangeable: `sbcd` is byte-wise and the
    /// nibble-wise mirror of `abcd` gives a different answer for this operand
    /// pair. Pinned so a later "unification" fails loudly here rather than 8
    /// cases into a suite group.
    #[test]
    fn abcd_and_sbcd_are_structurally_different_models() {
        // Nibble-wise borrow would propagate the low -6 into the high nibble.
        assert_eq!(sbcd(0x10, 0x01, false).0, 0x09);
        // The commuted addition is the same either way, which is why ABCD alone
        // cannot discriminate operand order.
        assert_eq!(abcd(0x28, 0x34, false), abcd(0x34, 0x28, false));
        // Subtraction can: it is not commutative.
        assert_ne!(sbcd(0x62, 0x34, false), sbcd(0x34, 0x62, false));
    }
}
