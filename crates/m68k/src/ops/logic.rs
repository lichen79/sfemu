//! `AND`, `OR`, `EOR`, `NOT`, the six `xxxI to CCR`/`to SR` forms, and `NOP`.
//!
//! The bitwise operations share [`alu`]'s schedule with [`super::arith`] and
//! differ only in their flag rule, which is uniform: **N and Z from the result,
//! V and C cleared, X preserved**. Preserving X rather than clearing it is
//! measured over 20,755 disagreeing cases, and clearing V and C rather than
//! preserving them over 24,738 — see [`crate::flags`].
//!
//! # Where `EOR` lives
//!
//! `AND` and `OR` occupy the `1100` and `1000` lines symmetrically. `EOR` does
//! not: it has no `<ea> op Dn` direction at all, and it lives in the **`1011`
//! (`CMP`) line** at opmode 4/5/6, sharing that space with `CMPM`:
//!
//! ```text
//!   1011 rrr 1ss mmm rrr    mode 001 -> CMPM (Ay)+,(Ax)+   (super::arith)
//!                           any other mode -> EOR Dn,<ea>
//! ```
//!
//! Mode 000 is therefore `EOR Dn,Dn`, a register-to-register form that a naive
//! "EOR writes memory" reading excludes. Excluding it costs 381 `EOR` cases per
//! size, and mis-assigning mode 001 costs 276 `CMP` cases per size.
//!
//! # `to CCR` and `to SR`
//!
//! Six opcodes with no addressing mode: `003C`/`007C` (`ORI`), `023C`/`027C`
//! (`ANDI`), `0A3C`/`0A7C` (`EORI`). All six take their operand from the word
//! already sitting in the prefetch queue, then **refill the queue from the word
//! after the opcode** — the same word is read twice, which is why the measured
//! shape is three program reads for a two-word instruction.
//!
//! The value rules, each scored on its discriminating subset:
//!
//! ```text
//!   to CCR:  sr = (sr & 0xFFE0) | ((sr op imm) & 0x1F)
//!   to SR:   sr = (sr op imm) & 0xA71F
//! ```
//!
//! `to CCR` touches the low **five** bits only. It preserves bits 5-7 — which
//! the 68000 has no register for, but which the vectors nonetheless carry
//! through unchanged — as well as the whole high byte. Scores: `EORItoCCR`
//! 2,189/2,189 against 0 for a full-byte write, `ORItoCCR` 2,176/2,176,
//! `EORItoSR` 1,256/1,256 against 0 for an unmasked store, `ORItoSR`
//! 1,185/1,185.
//!
//! **`ANDItoCCR` and `ANDItoSR` have zero discriminating cases.** `AND` can only
//! clear bits, so masking the result is indistinguishable from not masking it in
//! every case the suite contains. The masking there is inherited from the
//! `EORI`/`ORI` evidence, not independently confirmed.
//!
//! `to CCR` is **not privileged** — 3,653 user-mode cases across the three
//! `toCCR` groups execute it normally. `to SR` in user mode takes a privilege
//! violation stacking the address of the opcode itself (3,730/3,730 at
//! `opcode + 0`, over the three `toSR` groups), without advancing the queue.

use crate::cpu::M68k;
use crate::decode::Handler;
use crate::ea::{mode_is_mem, Size};
use crate::exception::{self, VEC_PRIVILEGE};
use crate::flags::logic_flags;
use crate::ops::alu::{self, Ops, Plan};
use crate::ops::arith::{Single, SIZES};
use crate::Bus;

/// The three bitwise operations.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    And,
    Or,
    Eor,
}

impl Op {
    #[inline]
    fn eval(self, a: u32, b: u32) -> u32 {
        match self {
            Op::And => a & b,
            Op::Or => a | b,
            Op::Eor => a ^ b,
        }
    }

    /// Applies the operation and sets the CCR, returning the masked result.
    fn apply(self, cpu: &mut M68k, a: u32, b: u32, size: Size) -> u32 {
        let r = self.eval(a, b) & size.mask();
        let (n, z, v, c) = logic_flags(r, size);
        cpu.set_ccr(cpu.ccr_x(), n, z, v, c);
        r
    }
}

/// `<ea> op Dn -> Dn` — `AND` and `OR` only; `EOR` has no such direction.
fn to_reg(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, op: Op, size: Size) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let dn = ((opcode >> 9) & 7) as usize;

    // Long forms pay 2 from memory and 4 from a register or an immediate.
    let idle = if size != Size::Long {
        0
    } else if mode_is_mem(mode, reg) {
        2
    } else {
        4
    };

    let plan = Plan::new(size, mode, reg).idle(idle);
    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        let r = op.apply(cpu, cpu.d[dn], ops.ea, size);
        let m = size.mask();
        cpu.d[dn] = (cpu.d[dn] & !m) | r;
        None
    })
}

/// `Dn op <ea> -> <ea>` — `AND`, `OR` and `EOR`.
///
/// `EOR`'s mode 000 makes this the one place in the family where the destination
/// can be a data register; it pays the 4-cycle long-operation idle that a memory
/// destination does not.
fn to_ea(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, op: Op, size: Size) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let dn = ((opcode >> 9) & 7) as usize;

    let idle = if mode == 0 && size == Size::Long {
        4
    } else {
        0
    };
    let plan = Plan::new(size, mode, reg).writes().idle(idle);
    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        let src = cpu.d[dn];
        Some(op.apply(cpu, ops.ea, src, size))
    })
}

/// `ANDI`/`ORI`/`EORI #imm,<ea>`.
fn immediate(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, op: Op, size: Size) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let pre = if size == Size::Long { 2 } else { 1 };

    let mut plan = Plan::new(size, mode, reg).pre(pre).writes();
    if !mode_is_mem(mode, reg) && size == Size::Long {
        plan = plan.idle(4);
    }

    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        Some(op.apply(cpu, ops.ea, ops.imm, size))
    })
}

/// `ANDI`/`ORI`/`EORI to CCR` and `to SR`.
///
/// The bus shape is three program reads for a two-word instruction: the queue
/// advances past the immediate, then is refilled from the word *after the
/// opcode* — so that word is read twice. Reproducing it means rewinding `pc` by
/// one word before the refill, which looks wrong and is exactly what the vectors
/// show (3,847 supervisor and 3,653 user cases across the three `toCCR` groups,
/// 20 cycles each — 2,500/2,500 per group, so the split by mode does not matter
/// to the shape).
fn to_ccr_sr(cpu: &mut M68k, bus: &mut dyn Bus, op: Op, to_sr: bool) -> u32 {
    if to_sr && !cpu.sr_s() {
        // Privilege violation. The queue does not advance, so the stacked PC is
        // the opcode's own address — and no access precedes the frame, so a
        // double bus fault here leaves the bus log empty and owes 0 accesses.
        // The framed 34 is `4 * SHORT_FRAME_ACCESSES + 6` for seven accesses
        // this path would not have made. See `exception::entry_cycles`.
        let pc = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
        exception::take(cpu, bus, VEC_PRIVILEGE, pc);
        return exception::entry_cycles(cpu, 0, 4 * exception::SHORT_FRAME_ACCESSES + 6);
    }

    let imm = cpu.prefetch[1];
    cpu.consume_opcode_dyn(bus);

    if to_sr {
        // `set_sr` masks to the implemented bits and swaps the stack pointers if
        // the S bit changes — which `ANDI to SR` can do.
        cpu.set_sr(op.eval(cpu.sr as u32, imm as u32) as u16);
    } else {
        // The low five bits only: bits 5-7 and the entire high byte survive,
        // so this cannot go through `set_sr` (which would clear bits 5-7).
        let r = op.eval(cpu.sr as u32, imm as u32) as u16;
        cpu.sr = (cpu.sr & 0xFFE0) | (r & 0x1F);
    }

    // Rewind one word, then refill: the word after the opcode is read a second
    // time and the queue ends up holding it and its successor.
    cpu.pc = cpu.pc.wrapping_sub(2);
    cpu.refill_prefetch_dyn(bus);
    4 * 3 + 8
}

/// `NOP`. Advances the queue and does nothing else — notably it does *not* touch
/// the CCR (2,500/2,500).
fn nop(cpu: &mut M68k, bus: &mut dyn Bus, _opcode: u16) -> u32 {
    cpu.consume_opcode_dyn(bus);
    4
}

// --- Dispatch-table installation ------------------------------------------

macro_rules! handlers {
    ($($name:ident($op:expr, $size:expr, $body:path);)*) => {
        $(fn $name(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
            $body(cpu, bus, opcode, $op, $size)
        })*
    };
}

handlers! {
    and_to_reg_b(Op::And, Size::Byte, to_reg);
    and_to_reg_w(Op::And, Size::Word, to_reg);
    and_to_reg_l(Op::And, Size::Long, to_reg);
    and_to_ea_b(Op::And, Size::Byte, to_ea);
    and_to_ea_w(Op::And, Size::Word, to_ea);
    and_to_ea_l(Op::And, Size::Long, to_ea);
    or_to_reg_b(Op::Or, Size::Byte, to_reg);
    or_to_reg_w(Op::Or, Size::Word, to_reg);
    or_to_reg_l(Op::Or, Size::Long, to_reg);
    or_to_ea_b(Op::Or, Size::Byte, to_ea);
    or_to_ea_w(Op::Or, Size::Word, to_ea);
    or_to_ea_l(Op::Or, Size::Long, to_ea);
    eor_b(Op::Eor, Size::Byte, to_ea);
    eor_w(Op::Eor, Size::Word, to_ea);
    eor_l(Op::Eor, Size::Long, to_ea);

    andi_b(Op::And, Size::Byte, immediate);
    andi_w(Op::And, Size::Word, immediate);
    andi_l(Op::And, Size::Long, immediate);
    ori_b(Op::Or, Size::Byte, immediate);
    ori_w(Op::Or, Size::Word, immediate);
    ori_l(Op::Or, Size::Long, immediate);
    eori_b(Op::Eor, Size::Byte, immediate);
    eori_w(Op::Eor, Size::Word, immediate);
    eori_l(Op::Eor, Size::Long, immediate);

    not_b(Single::Not, Size::Byte, super::arith::single);
    not_w(Single::Not, Size::Word, super::arith::single);
    not_l(Single::Not, Size::Long, super::arith::single);
}

macro_rules! ccr_sr_handlers {
    ($($name:ident($op:expr, $to_sr:expr);)*) => {
        $(fn $name(cpu: &mut M68k, bus: &mut dyn Bus, _opcode: u16) -> u32 {
            to_ccr_sr(cpu, bus, $op, $to_sr)
        })*
    };
}

ccr_sr_handlers! {
    andi_to_ccr(Op::And, false);
    andi_to_sr(Op::And, true);
    ori_to_ccr(Op::Or, false);
    ori_to_sr(Op::Or, true);
    eori_to_ccr(Op::Eor, false);
    eori_to_sr(Op::Eor, true);
}

/// Installs one `0000 fff ss mmm rrr` immediate family across all three sizes.
///
/// Shared with [`super::arith`], which owns `SUBI`, `ADDI` and `CMPI` in the same
/// encoding space. The destination set is data-alterable — registers and writable
/// memory — and deliberately excludes mode 7 reg 4, which for families 0, 1 and 5
/// is the `to CCR`/`to SR` encoding and for the rest is not legal at all.
pub(super) fn register_immediate_family(
    table: &mut [Handler; 65536],
    family: u16,
    handlers: [Handler; 3],
) {
    for (sb, _size) in SIZES {
        for mode in 0..8u16 {
            for reg in 0..8u16 {
                if !super::arith::valid_data_alterable(mode, reg) {
                    continue;
                }
                let op = (family << 9) | (sb << 6) | (mode << 3) | reg;
                table[op as usize] = handlers[sb as usize];
            }
        }
    }
}

/// Installs every instruction this module owns.
pub fn register(table: &mut [Handler; 65536]) {
    // 1100 AND / 1000 OR: opmode 0/1/2 to a data register, 4/5/6 to an <ea>.
    // Opmode 3 and 7 are MULx/DIVx/ABCD/SBCD/EXG, which belong to later tasks.
    for (line, to_reg_h, to_ea_h) in [
        (
            0xCu16,
            [and_to_reg_b, and_to_reg_w, and_to_reg_l],
            [and_to_ea_b, and_to_ea_w, and_to_ea_l],
        ),
        (
            0x8,
            [or_to_reg_b, or_to_reg_w, or_to_reg_l],
            [or_to_ea_b, or_to_ea_w, or_to_ea_l],
        ),
    ] {
        for dn in 0..8u16 {
            for i in 0..3usize {
                for mode in 0..8u16 {
                    for reg in 0..8u16 {
                        let base = (line << 12) | (dn << 9) | (mode << 3) | reg;
                        // <ea> op Dn: any source but an address register.
                        if mode != 1 && (mode != 7 || reg <= 4) {
                            table[(base | ((i as u16) << 6)) as usize] = to_reg_h[i];
                        }
                        // Dn op <ea>: writable memory only.
                        if super::arith::valid_mem_dst(mode, reg) {
                            table[(base | ((i as u16 + 4) << 6)) as usize] = to_ea_h[i];
                        }
                    }
                }
            }
        }
    }

    // 1011 opmode 4/5/6: EOR Dn,<ea> in every mode except 001, which is CMPM
    // and belongs to `super::arith`. Mode 000 is `EOR Dn,Dn`.
    for dn in 0..8u16 {
        for (i, h) in [eor_b, eor_w, eor_l].into_iter().enumerate() {
            for mode in 0..8u16 {
                for reg in 0..8u16 {
                    if mode == 1 || !super::arith::valid_data_alterable(mode, reg) {
                        continue;
                    }
                    let op = 0xB000 | (dn << 9) | ((i as u16 + 4) << 6) | (mode << 3) | reg;
                    table[op as usize] = h;
                }
            }
        }
    }

    // 0000 xxxI: ORI = 0, ANDI = 1, EORI = 5.
    register_immediate_family(table, 0, [ori_b, ori_w, ori_l]);
    register_immediate_family(table, 1, [andi_b, andi_w, andi_l]);
    register_immediate_family(table, 5, [eori_b, eori_w, eori_l]);

    // The six fixed to-CCR / to-SR opcodes, which occupy mode 7 reg 4 of those
    // same three families at byte and word size.
    table[0x003C] = ori_to_ccr;
    table[0x007C] = ori_to_sr;
    table[0x023C] = andi_to_ccr;
    table[0x027C] = andi_to_sr;
    table[0x0A3C] = eori_to_ccr;
    table[0x0A7C] = eori_to_sr;

    // 0100 0110 ss mmm rrr: NOT.
    for (sb, _size) in SIZES {
        for mode in 0..8u16 {
            for reg in 0..8u16 {
                if !super::arith::valid_data_alterable(mode, reg) {
                    continue;
                }
                table[(0x4600 | (sb << 6) | (mode << 3) | reg) as usize] =
                    [not_b, not_w, not_l][sb as usize];
            }
        }
    }

    table[0x4E71] = nop;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::{FlatBus, RecordingBus};
    use crate::cpu::{SR_C, SR_S, SR_V, SR_X};
    use crate::decode::Decoder;

    fn at(bus: &mut impl Bus) -> M68k {
        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(bus);
        cpu
    }

    /// The logical ops clear V and C but leave X alone. Getting either half
    /// wrong is invisible in isolation and wrong in tens of thousands of cases.
    #[test]
    fn logic_clears_v_and_c_and_preserves_x() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xC001, 0x4E71]); // AND.b D1,D0
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_X | SR_V | SR_C;
        cpu.d[0] = 0xFF;
        cpu.d[1] = 0xF0;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0] & 0xFF, 0xF0);
        assert!(cpu.ccr_x(), "X survives a logical op");
        assert!(!cpu.ccr_v() && !cpu.ccr_c());
        assert!(cpu.ccr_n(), "0xF0 is negative as a byte");
    }

    /// `EOR Dn,Dn` — mode 000 of the `1011` line, which shares its opmode with
    /// `CMPM`. If this decodes as anything else the EOR groups collapse.
    #[test]
    fn eor_into_a_data_register() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xB380, 0x4E71]); // EOR.l D1,D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0xFFFF_0000;
        cpu.d[1] = 0x00FF_00FF;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 0xFF00_00FF);
        assert_eq!(cycles, 8, "one fetch plus the 4-cycle long-op idle");
    }

    /// Mode 001 of the same opmode is `CMPM`, not `EOR` — the complement of the
    /// test above, and the reason both live in one encoding table.
    #[test]
    fn mode_one_of_the_eor_opmode_is_cmpm() {
        let dec = Decoder::new();
        // EOR would write A1; CMPM only compares and increments.
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xB389, 0x4E71]); // CMPM.l (A1)+,(A1)+
        let mut cpu = at(&mut bus);
        cpu.a[1] = 0x2000;
        cpu.d[1] = 0xDEAD_BEEF;
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.a[1], 0x2008, "two long postincrements");
        assert_eq!(cpu.d[1], 0xDEAD_BEEF, "CMPM writes nothing");
    }

    /// `NOT` inverts and reports N/Z from the result.
    #[test]
    fn not_inverts_within_the_operand_size() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4640, 0x4E71]); // NOT.w D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x1234_FFFF;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 0x1234_0000, "upper word untouched");
        assert!(cpu.ccr_z() && !cpu.ccr_n());
    }

    /// `ORI to CCR` writes the low five bits and preserves everything above
    /// them — including bits 5-7, which no documented register contains.
    #[test]
    fn ori_to_ccr_touches_only_the_low_five_bits() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x003C, 0x00FF, 0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr = SR_S | 0x0700 | 0x00E0; // bits 5-7 set

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.sr, SR_S | 0x0700 | 0x00E0 | 0x1F);
        assert_eq!(cycles, 20);
        assert_eq!(cpu.pc, 0x1008, "PC advances 4 from its primed value");
    }

    /// The queue refill re-reads the word after the opcode, so a two-word
    /// instruction makes three program reads.
    #[test]
    fn to_ccr_rereads_the_word_after_the_opcode() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x003C, 0x0000, 0x4E71, 0x4E72]);
        let mut cpu = at(&mut bus);
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.log,
            vec![
                (false, 0x1004, 0x4E71),
                (false, 0x1004, 0x4E71),
                (false, 0x1006, 0x4E72),
            ]
        );
        assert_eq!(cpu.prefetch, [0x4E71, 0x4E72]);
    }

    /// `to CCR` is not privileged; `to SR` is.
    #[test]
    fn to_ccr_is_legal_in_user_mode_but_to_sr_is_not() {
        let dec = Decoder::new();

        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x003C, 0x0001, 0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr = 0; // user mode
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.sr, 1, "ORI to CCR runs in user mode");

        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x007C, 0x0001, 0x4E71, 0x4E71]);
        bus.put16(0x0022, 0x2000); // vector 8 -> 0x2000
        bus.load(0x2000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr = 0;
        cpu.usp = 0x3000;
        cpu.ssp = 0x4000;
        cpu.a[7] = 0x3000;
        let cycles = cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.pc, 0x2004, "vectored through 8");
        assert_eq!(cycles, 34);
        // The stacked PC is the opcode's own address, not the next instruction.
        assert_eq!(bus.read16(0x3FFE), 0x1000);
        assert_eq!(cpu.sr & 0x2000, 0x2000, "supervisor on entry");
    }

    /// A privilege violation that double bus faults costs the halt idle, not 34.
    ///
    /// `to_ccr_sr` is the sixth site of this defect; the other five were fixed in
    /// Task 11. The 34 is `4 * SHORT_FRAME_ACCESSES + 6` — it pays for seven frame
    /// and vector accesses that a halted entry never performs, and this path
    /// performs none of its own either, so its bus log is empty and it owes 0.
    ///
    /// Reaching it needs an odd **frame base**, which in user mode is the SSP, not
    /// `a[7]` — an odd `a[7]` here would be an odd USP and the frame would still
    /// go to an even SSP and be written normally. `to_ccr_is_legal_in_user_mode..`
    /// above is the even-SSP control: same instruction, same mode, 34 cycles and a
    /// real frame.
    ///
    /// Extrapolated, like every halt path: 0 of 317,500 cases halt.
    #[test]
    fn to_sr_privilege_violation_onto_an_odd_ssp_halts() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x007C, 0x0001, 0x4E71]); // ORI #1,SR
        bus.put16(0x0022, 0x2000); // vector 8, so a frame would be visible
        let mut cpu = at(&mut bus);
        cpu.sr = 0; // user mode: the frame base is the SSP
        cpu.a[7] = 0x3000;
        cpu.usp = 0x3000;
        cpu.ssp = 0x4001; // odd
        bus.log.clear();

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert!(cpu.halted, "an odd frame base is a double bus fault");
        assert_eq!(bus.writes(), vec![], "no frame was written");
        assert_eq!(bus.reads(), vec![], "not even the vector was fetched");
        assert_eq!(
            cycles,
            exception::HALTED_IDLE_CYCLES,
            "4 × 0 accesses + the halt idle; the framed 34 pays for seven \
             accesses this step's bus log does not contain"
        );
    }

    /// `ANDI to SR` can clear the S bit, which swaps the active stack pointer.
    #[test]
    fn andi_to_sr_dropping_supervisor_swaps_the_stack_pointer() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x027C, 0xDFFF, 0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.a[7] = 0x4000;
        cpu.usp = 0x2000;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.sr & 0x2000, 0, "S cleared");
        assert_eq!(cpu.a[7], 0x2000, "USP is now active");
        assert_eq!(cpu.ssp, 0x4000, "SSP saved");
    }

    /// `to SR` masks the unimplemented bits away.
    #[test]
    fn to_sr_masks_the_result() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x007C, 0xFFFF, 0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.sr, 0xA71F);
    }

    /// `NOP` advances the queue, costs 4 cycles, and leaves the CCR alone.
    #[test]
    fn nop_does_nothing_but_advance() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_X | SR_V | SR_C;
        let before = cpu.sr;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(cpu.sr, before);
        assert_eq!(cpu.pc, 0x1006);
    }
}
