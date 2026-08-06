//! `BTST`, `BCHG`, `BCLR`, `BSET`, and `TAS`.
//!
//! Five instructions that share a shape: read one operand, look at one bit of
//! it, set `Z` from what was there *before* any modification, and write the
//! operand back changed. `TAS` is the degenerate case — its "bit" is always 7
//! and it always sets it.
//!
//! ```text
//!   0000 100 op mmm rrr   + one extension word   static:  bit number is #data
//!   0000 rrr 1 op mmm rrr                        dynamic: bit number is in Dn
//!   0100 1010 11 mmm rrr                         TAS
//! ```
//!
//! `op` is bits 7-6: `00` `BTST`, `01` `BCHG`, `10` `BCLR`, `11` `BSET`. The
//! dynamic form's mode `001` is **not** an address register — it is `MOVEP`,
//! which shares bit 8 with this family and belongs to a later task.
//!
//! # The bit number
//!
//! A data-register destination is 32 bits wide and takes the bit number **mod
//! 32**; every other operand is a **byte** and takes it **mod 8**. Both were
//! scored only on the cases where the two moduli predict different `Z`s, against
//! `mod 8` / `mod 16` / `mod 32` as rivals:
//!
//! ```text
//!                        register destination        memory destination
//!            all   disc.  mod32   mod8  mod16      all  disc.  mod8  mod32
//!   BTST     353     135    135      0      0      692    474   474      0
//!   BCHG     410     171    171      0      0      806    546   546      0
//!   BCLR     413     165    165      0      0      749    511   511      0
//!   BSET     389     137    137      0      0      795    526   526      0
//! ```
//!
//! `mod 8` also governs the operands that are *not* data-space memory, which the
//! table above cannot reach because it identifies the operand from a data-space
//! read: `d16(PC)` 19/19, `(d8,PC,Xn)` 17/17 and the immediate form 20/20, with
//! `mod 32` scoring 0 in each. So the rule is "byte operand ⇒ mod 8" and not
//! "memory operand ⇒ mod 8".
//!
//! # Only `Z` moves
//!
//! Across all four groups the union of SR bits that ever change between the
//! initial and final state is `0004` — `Z` alone. `N`, `V`, `C` and `X` are
//! *preserved*, not cleared, which is why none of these goes through
//! [`crate::flags::logic_flags`]. The measurement needs its control group to
//! mean anything, since a union of one bit is also what a broken census would
//! report: under the same code `AND.b` and `NOT.b` give `000F`, and `ASL.b` and
//! `ROXL.b` give `001F`. The absence is real.
//!
//! `TAS` is the exception, and behaves like a `TST` that writes: `N` and `Z`
//! come from the operand as read, `V` and `C` clear, `X` survives.
//!
//! # Cost
//!
//! The timing law (`4 * accesses + idle`) leaves only the idle to model. A
//! memory destination has none of its own; a data-register destination has:
//!
//! ```text
//!   idle = base + 2 * (op writes && bit % 32 >= 16)
//!   base = 4 for BCLR, 2 for BTST, BCHG and BSET
//! ```
//!
//! 1,565/1,565, and every one of the 16 (op × bit-half × static/dynamic) buckets
//! is populated. The threshold is genuinely 16: it beats 8 by 281/281, 15 by
//! 37/37, 17 by 39/39 and 24 by 317/317 on the cases where they disagree. The
//! high half costs an extra internal step because the 68000's shifter reaches a
//! bit above 15 the long way round — and `BTST`, which performs no write, is
//! exempt (176/176 against "`BTST` pays it too").
//!
//! `TAS`'s cost is `4` for a register destination and `4 * (de + 3) + 2` plus
//! [`alu`]'s own address-computation lead otherwise — 2,500/2,500. Note the
//! group is nonetheless a **known-bad** upstream group: the vectors' *ordered*
//! transactions put an idle between `TAS`'s read and its write, which the
//! harness cannot match, and the format's dedicated `Tas` transaction kind never
//! appears in them at all (5,540 `Read`, 2,904 `Idle`, 2,108 `Write`). Every
//! value this module computes for `TAS` is nevertheless confirmed against those
//! same vectors, so the group failing is a statement about the sequence and not
//! about the result.

use crate::cpu::M68k;
use crate::decode::Handler;
use crate::ea::{mode_is_mem, Size};
use crate::ops::alu::{self, Ops, Plan};
use crate::Bus;

/// Which of the four bit instructions, from bits 7-6.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Tst,
    Chg,
    Clr,
    Set,
}

impl Op {
    #[inline]
    fn from_bits(b: u16) -> Self {
        match b {
            0 => Op::Tst,
            1 => Op::Chg,
            2 => Op::Clr,
            _ => Op::Set,
        }
    }

    /// The one that only looks — and so pays no write, and no high-bit idle.
    #[inline]
    fn writes(self) -> bool {
        self != Op::Tst
    }

    /// The idle a data-register destination pays before the bit-position
    /// penalty.
    #[inline]
    fn base_idle(self) -> u32 {
        if self == Op::Clr {
            4
        } else {
            2
        }
    }

    #[inline]
    fn apply(self, val: u32, mask: u32) -> u32 {
        match self {
            Op::Tst => val,
            Op::Chg => val ^ mask,
            Op::Clr => val & !mask,
            Op::Set => val | mask,
        }
    }
}

/// `BTST`/`BCHG`/`BCLR`/`BSET`, both bit-number forms.
///
/// `static_form` selects where the bit number comes from: an extension word that
/// precedes the `<ea>`'s own (hence [`Plan::pre`]), or the data register named by
/// bits 11-9.
fn bit_op(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, op: Op, static_form: bool) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);

    // A data register is the operand's *own* width; everything else is a byte,
    // and the two take different moduli.
    let long = mode == 0;
    let size = if long { Size::Long } else { Size::Byte };

    let dyn_bit = cpu.d[((opcode >> 9) & 7) as usize];
    let mut plan = Plan::new(size, mode, reg);
    if static_form {
        plan = plan.pre(1);
    }
    // `Plan::writes` is dead state — it is assigned by every handler in the
    // crate and read by none. What actually decides the write is whether the
    // `compute` closure below returns `Some`, which for `BTST` it does not. Set
    // here only for symmetry with the existing handlers; a fix that removes the
    // field is queued separately.
    if op.writes() {
        plan = plan.writes();
    }

    if long {
        // The bit number is needed to price the instruction, and for the static
        // form it is still in the queue rather than in `Ops::imm` — the plan is
        // fixed before any fetch happens.
        let bit = if static_form {
            cpu.prefetch[1] as u32
        } else {
            dyn_bit
        } % 32;
        let hi = u32::from(op.writes() && bit >= 16) * 2;
        plan = plan.idle(op.base_idle() + hi);
    } else if mode == 7 && reg == 4 {
        // `BTST Dn,#imm`: no operand access at all, and a 2-cycle internal step
        // that the memory forms do not have.
        plan = plan.idle(2);
    }

    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        let number = if static_form { ops.imm } else { dyn_bit };
        let bit = number % if long { 32 } else { 8 };
        let mask = 1u32 << bit;

        // Z reflects the bit as it was *before* the modification, and nothing
        // else in the CCR moves.
        cpu.set_ccr(
            cpu.ccr_x(),
            cpu.ccr_n(),
            ops.ea & mask == 0,
            cpu.ccr_v(),
            cpu.ccr_c(),
        );

        if op.writes() {
            Some(op.apply(ops.ea, mask))
        } else {
            None
        }
    })
}

/// `TAS <ea>` — test a byte, then unconditionally set its high bit.
///
/// On real hardware this is an indivisible read-modify-write cycle, which is the
/// whole point of the instruction and the reason its vectors are known-bad; here
/// it is an ordinary byte RMW through [`alu::run`].
fn tas(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);

    let idle = if mode_is_mem(mode, reg) { 2 } else { 0 };
    let plan = Plan::new(Size::Byte, mode, reg).writes().idle(idle);

    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        let v = ops.ea & 0xFF;
        cpu.set_ccr(cpu.ccr_x(), v & 0x80 != 0, v == 0, false, false);
        Some(v | 0x80)
    })
}

// --- Dispatch-table installation ------------------------------------------

macro_rules! handlers {
    ($($name:ident($op:expr, $static_form:expr);)*) => {
        $(fn $name(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
            bit_op(cpu, bus, opcode, $op, $static_form)
        })*
    };
}

handlers! {
    btst_static(Op::Tst, true);
    bchg_static(Op::Chg, true);
    bclr_static(Op::Clr, true);
    bset_static(Op::Set, true);
    btst_dynamic(Op::Tst, false);
    bchg_dynamic(Op::Chg, false);
    bclr_dynamic(Op::Clr, false);
    bset_dynamic(Op::Set, false);
}

/// `BTST`'s destination set: data-alterable, plus the two PC-relative modes
/// because it does not write.
///
/// The immediate operand `#data` is legal for the **dynamic** form only. That
/// asymmetry is not a guess: across 2,500 `BTST` cases the dynamic form uses mode
/// 7 reg 4 fifty-eight times and the static form never, while mode 5 — a control
/// both forms can reach — shows 328 dynamic against 40 static. So the static form
/// is well represented and its absence from mode 7 reg 4 is an encoding fact
/// rather than a thin sample.
fn valid_btst_src(mode: u16, reg: u16, immediate_ok: bool) -> bool {
    match mode {
        0 => true,
        1 => false,
        7 => reg <= 3 || (reg == 4 && immediate_ok),
        _ => true,
    }
}

/// Installs the bit instructions and `TAS`.
pub fn register(table: &mut [Handler; 65536]) {
    for opbits in 0..4u16 {
        let op = Op::from_bits(opbits);
        let sh: Handler = [btst_static, bchg_static, bclr_static, bset_static][opbits as usize];
        let dh: Handler = [btst_dynamic, bchg_dynamic, bclr_dynamic, bset_dynamic][opbits as usize];

        for mode in 0..8u16 {
            for reg in 0..8u16 {
                let ok = |immediate_ok| {
                    if op == Op::Tst {
                        valid_btst_src(mode, reg, immediate_ok)
                    } else {
                        super::arith::valid_data_alterable(mode, reg)
                    }
                };

                // Static: 0000 1000 op mmm rrr. No immediate operand in this
                // form, for BTST or anything else.
                if ok(false) {
                    table[(0x0800 | (opbits << 6) | (mode << 3) | reg) as usize] = sh;
                }
                // Dynamic: 0000 rrr 1 op mmm rrr. Mode 001 is MOVEP, which is
                // excluded by every destination rule above anyway.
                if ok(true) {
                    for dn in 0..8u16 {
                        table[(0x0100 | (dn << 9) | (opbits << 6) | (mode << 3) | reg) as usize] =
                            dh;
                    }
                }
            }
        }
    }

    // 0100 1010 11 mmm rrr: TAS. Mode 7 reg 4 of this pattern is the `ILLEGAL`
    // instruction, whose entire effect is the illegal-instruction trap, so
    // leaving it unclaimed is not merely acceptable — it is correct.
    for mode in 0..8u16 {
        for reg in 0..8u16 {
            if super::arith::valid_data_alterable(mode, reg) {
                table[(0x4AC0 | (mode << 3) | reg) as usize] = tas;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::{FlatBus, RecordingBus};
    use crate::cpu::{SR_C, SR_N, SR_S, SR_V, SR_X};
    use crate::decode::Decoder;

    fn at(bus: &mut impl Bus) -> M68k {
        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(bus);
        cpu
    }

    /// A data-register destination takes the bit number mod 32, so bit 33 is
    /// bit 1 and not bit 1-of-a-byte-nor-nothing.
    #[test]
    fn a_register_destination_takes_the_bit_number_mod_32() {
        let dec = Decoder::new();
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x0800, 0x0021, 0x4E71]); // BTST #33,D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x0000_0002;
        cpu.step_with(&dec, &mut bus);
        assert!(!cpu.ccr_z(), "bit 33 mod 32 = bit 1, which is set");

        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x0800, 0x0014, 0x4E71]); // BTST #20,D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x0010_0000;
        cpu.step_with(&dec, &mut bus);
        assert!(!cpu.ccr_z(), "bit 20 of the register, not of its low byte");
    }

    /// A memory destination is one byte wide and takes the bit number mod 8.
    #[test]
    fn a_memory_destination_takes_the_bit_number_mod_8() {
        let dec = Decoder::new();
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x0810, 0x000C, 0x4E71]); // BTST #12,(A0)
        bus.mem[0x2000] = 0x10;
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2000;
        cpu.step_with(&dec, &mut bus);
        assert!(!cpu.ccr_z(), "12 mod 8 = 4, and bit 4 of 0x10 is set");

        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x0810, 0x0004, 0x4E71]); // BTST #4,(A0)
        bus.mem[0x2000] = 0x08;
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2000;
        cpu.step_with(&dec, &mut bus);
        assert!(cpu.ccr_z(), "bit 4 of 0x08 is clear");
    }

    /// `Z` comes from the bit as it was before the write, which is the whole
    /// difference between `BSET` and a plain `OR`.
    #[test]
    fn z_reports_the_bit_before_the_modification() {
        let dec = Decoder::new();
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x08D0, 0x0003, 0x4E71]); // BSET #3,(A0)
        bus.mem[0x2000] = 0x00;
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2000;
        cpu.step_with(&dec, &mut bus);
        assert_eq!(bus.mem[0x2000], 0x08, "the bit is set afterwards");
        assert!(cpu.ccr_z(), "but Z says it had been clear");
    }

    /// Nothing but `Z` moves — not even to be cleared.
    #[test]
    fn only_z_changes() {
        let dec = Decoder::new();
        for op in [0x0800u16, 0x0840, 0x0880, 0x08C0] {
            for preset in [0u16, SR_X | SR_N | SR_V | SR_C] {
                let mut bus = FlatBus::new();
                bus.load(0x1000, &[op, 0x0001, 0x4E71]);
                let mut cpu = at(&mut bus);
                cpu.sr |= preset;
                cpu.d[0] = 0x0000_00FF;
                cpu.step_with(&dec, &mut bus);
                assert_eq!(
                    cpu.sr & (SR_X | SR_N | SR_V | SR_C),
                    preset,
                    "opcode {op:04X} must leave X, N, V and C alone"
                );
            }
        }
    }

    /// Each of the three writing ops does the right thing to the bit, and
    /// `BTST` does nothing at all.
    #[test]
    fn each_op_changes_the_bit_its_own_way() {
        let dec = Decoder::new();
        // Static form, bit 1 of D0 = 0x0000_00FF.
        for (op, want, name) in [
            (0x0800u16, 0xFFu32, "BTST"),
            (0x0840, 0xFD, "BCHG"),
            (0x0880, 0xFD, "BCLR"),
            (0x08C0, 0xFF, "BSET"),
        ] {
            let mut bus = FlatBus::new();
            bus.load(0x1000, &[op, 0x0001, 0x4E71]);
            let mut cpu = at(&mut bus);
            cpu.d[0] = 0xFF;
            cpu.step_with(&dec, &mut bus);
            assert_eq!(cpu.d[0], want, "{name} #1,D0");
            assert!(!cpu.ccr_z(), "{name}: the bit had been set");
        }
    }

    /// `BTST` reads and never writes; the other three are byte
    /// read-modify-writes with the queue advance in between.
    #[test]
    fn btst_never_writes_and_the_others_always_do() {
        let dec = Decoder::new();

        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x0110, 0x4E71, 0x4E71]); // BTST D0,(A0)
        bus.put16(0x2000, 0x0100);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2000;
        cpu.d[0] = 0;
        bus.log.clear();
        let cycles = cpu.step_with(&dec, &mut bus);
        assert!(bus.writes().is_empty(), "BTST never writes");
        assert_eq!(cycles, 8, "one operand read and one queue advance");

        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x01D0, 0x4E71, 0x4E71]); // BSET D0,(A0)
        bus.put16(0x2000, 0x0100);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2000;
        cpu.d[0] = 0;
        bus.log.clear();
        let cycles = cpu.step_with(&dec, &mut bus);
        assert_eq!(bus.writes(), vec![(0x2000, 0x01)]);
        assert_eq!(cycles, 12, "read, advance, write");
        // The write lands after the queue advance, not before it.
        assert_eq!(
            bus.log,
            vec![
                (false, 0x2000, 0x01),
                (false, 0x1004, 0x4E71),
                (true, 0x2000, 0x01),
            ]
        );
    }

    /// The high half of a register destination costs two extra cycles, and
    /// `BTST` is exempt because it does not write.
    #[test]
    fn a_high_bit_number_costs_a_writing_op_two_more_cycles() {
        let dec = Decoder::new();
        // Dynamic form: one access (the queue advance) plus the idle.
        for (op, base, name) in [
            (0x0100u16, 2u32, "BTST"),
            (0x0140, 2, "BCHG"),
            (0x0180, 4, "BCLR"),
            (0x01C0, 2, "BSET"),
        ] {
            for bit in [3u32, 20] {
                let mut bus = FlatBus::new();
                bus.load(0x1000, &[op, 0x4E71]);
                let mut cpu = at(&mut bus);
                cpu.d[0] = bit;
                let cycles = cpu.step_with(&dec, &mut bus);
                let hi = u32::from(op != 0x0100 && bit >= 16) * 2;
                assert_eq!(cycles, 4 + base + hi, "{name} #{bit},D0");
            }
        }
    }

    /// `BTST Dn,#imm` exists — the one operand that is neither a register nor
    /// memory — and only in the dynamic form.
    #[test]
    fn btst_against_an_immediate_operand() {
        let dec = Decoder::new();
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x013C, 0x0080, 0x4E71]); // BTST D0,#0x80
        let mut cpu = at(&mut bus);
        cpu.d[0] = 7;
        bus.log.clear();
        let cycles = cpu.step_with(&dec, &mut bus);
        assert!(!cpu.ccr_z(), "bit 7 of 0x80 is set");
        assert!(bus.writes().is_empty());
        assert_eq!(cycles, 10, "two program fetches and a 2-cycle step");
    }

    /// `BTST #n,#imm` does not exist: the immediate operand belongs to the
    /// dynamic form only. Checked by dispatching to the illegal handler, which
    /// pushes a frame — the encoding half of this lives in the `opcode_space`
    /// test.
    #[test]
    fn the_static_form_has_no_immediate_operand() {
        let dec = Decoder::new();
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x083C, 0x0001, 0x4E71]);
        bus.put16(0x0010, 0x0000);
        bus.put16(0x0012, 0x5000);
        let mut cpu = at(&mut bus);
        cpu.step_with(&dec, &mut bus);
        assert_eq!(
            cpu.pc & 0xFFFF,
            0x5004,
            "BTST #n,#imm must reach the illegal-instruction handler"
        );
    }

    /// `TAS` reports the byte it found and leaves bit 7 set.
    #[test]
    fn tas_tests_then_sets_the_high_bit() {
        let dec = Decoder::new();

        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4AD0, 0x4E71, 0x4E71]); // TAS (A0)
        bus.put16(0x2000, 0x0000);
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_X | SR_V | SR_C;
        cpu.a[0] = 0x2000;
        bus.log.clear();
        let cycles = cpu.step_with(&dec, &mut bus);
        assert_eq!(bus.writes(), vec![(0x2000, 0x80)]);
        assert!(cpu.ccr_z() && !cpu.ccr_n(), "the byte read was zero");
        assert!(!cpu.ccr_v() && !cpu.ccr_c(), "V and C clear");
        assert!(cpu.ccr_x(), "X survives");
        assert_eq!(cycles, 14, "three accesses plus the 2-cycle step");

        // A register destination touches only its low byte, and costs one fetch.
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4AC0, 0x4E71]); // TAS D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x1234_5601;
        let cycles = cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0], 0x1234_5681);
        assert!(!cpu.ccr_z() && !cpu.ccr_n());
        assert_eq!(cycles, 4);
    }
}
