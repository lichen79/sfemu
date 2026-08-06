//! `ADD`, `SUB`, `CMP` and their variants, plus `NEG`, `NEGX`, `CLR` and `TST`.
//!
//! Every handler here builds an `alu::Plan` and hands a closure to `alu::run`,
//! which owns the bus schedule (see that module's docs). What is left in this
//! file is the arithmetic and the flag rules. (`alu` is `pub(super)`, so those
//! names are deliberately not intra-doc links — rustdoc cannot resolve a link
//! from a public module into a private one without `--document-private-items`.)
//!
//! # The encodings, censused from the vectors
//!
//! The `1001` (`SUB`) and `1101` (`ADD`) lines pack five different instructions,
//! selected by the 3-bit opmode:
//!
//! ```text
//!   opmode 0/1/2   <ea> op Dn -> Dn          modes 0, 2-6, 7r0-r4
//!                                            (opmode 1/2 also allow mode 1)
//!   opmode 3/7     ADDA/SUBA <ea>,An         modes 0-6, 7r0-r4
//!   opmode 4/5/6   mode 0: ADDX/SUBX Dy,Dx
//!                  mode 1: ADDX/SUBX -(Ay),-(Ax)
//!                  else:   Dn op <ea> -> <ea>   modes 2-6, 7r0, 7r1
//! ```
//!
//! `1011` (`CMP`) is the same shape with one substitution that is easy to get
//! wrong: in opmode 4/5/6, **mode 1 is `CMPM (Ay)+,(Ax)+` and every other mode
//! is `EOR Dn,<ea>`** — including mode 0, which is `EOR Dn,Dn`. Excluding mode 0
//! there costs 276 `CMP` and 381 `EOR` cases per size. `EOR` lives in
//! [`super::logic`]; only `CMPM` and `CMPA` are here.
//!
//! Byte-sized forms never accept mode 1: there is no byte of an address
//! register. `ADDQ`/`SUBQ` are the exception that proves the rule — `ADDQ.w #d,An`
//! and `ADDQ.l #d,An` exist and operate on all 32 bits, but `ADDQ.b #d,An` does
//! not.

use crate::cpu::M68k;
use crate::decode::Handler;
use crate::ea::{mode_is_mem, Size};
use crate::flags::{accumulate_z, add_flags, sub_flags};
use crate::ops::alu::{self, Ops, Plan};
use crate::Bus;

/// Which way an `<ea>`-and-`Dn` instruction moves its result.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    /// `<ea> op Dn -> Dn`
    ToReg,
    /// `Dn op <ea> -> <ea>`
    ToMem,
}

/// The three additive operations, sharing everything but their flag rules.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Op {
    Add,
    Sub,
    /// Subtract for flags only, discarding the result.
    Cmp,
}

impl Op {
    /// Applies the operation and sets the CCR, returning the result.
    ///
    /// `X` is *preserved* by `Cmp` and written by the other two. That is not an
    /// aesthetic choice: 20,755 cases distinguish it from deriving `X` from the
    /// carry, and all 20,755 prefer preservation.
    fn apply(self, cpu: &mut M68k, dst: u32, src: u32, size: Size) -> u32 {
        match self {
            Op::Add => {
                let (r, n, z, v, c) = add_flags(dst, src, false, size);
                cpu.set_ccr(c, n, z, v, c);
                r
            }
            Op::Sub => {
                let (r, n, z, v, c) = sub_flags(dst, src, false, size);
                cpu.set_ccr(c, n, z, v, c);
                r
            }
            Op::Cmp => {
                let (r, n, z, v, c) = sub_flags(dst, src, false, size);
                cpu.set_ccr(cpu.ccr_x(), n, z, v, c);
                r
            }
        }
    }
}

/// Trailing idle for `<ea> op Dn -> Dn`, which is the busiest row of the table
/// in [`alu`]'s module docs: `CMP` always pays 2, everything else pays 4 from a
/// register or immediate and 2 from memory. Byte and word forms pay nothing.
fn to_reg_idle(op: Op, size: Size, mode: u16, reg: u16) -> u32 {
    if size != Size::Long {
        0
    } else if op == Op::Cmp || mode_is_mem(mode, reg) {
        2
    } else {
        4
    }
}

/// `<ea> op Dn -> Dn` and `Dn op <ea> -> <ea>`.
fn alu_ea_dn(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, op: Op, size: Size, dir: Dir) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let dn = ((opcode >> 9) & 7) as usize;

    let mut plan = Plan::new(size, mode, reg);
    if dir == Dir::ToReg {
        plan = plan.idle(to_reg_idle(op, size, mode, reg));
    } else {
        plan = plan.writes();
        // The only register destination this direction has is `EOR Dn,Dn`,
        // handled in `super::logic`; an arithmetic `Dn op <ea>` is always memory
        // and always pays no trailing idle.
        debug_assert!(mode_is_mem(mode, reg), "Dn op <ea> writes memory");
    }

    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| match dir {
        Dir::ToReg => {
            let r = op.apply(cpu, cpu.d[dn], ops.ea, size);
            if op != Op::Cmp {
                let m = size.mask();
                cpu.d[dn] = (cpu.d[dn] & !m) | (r & m);
            }
            None
        }
        Dir::ToMem => {
            let src = cpu.d[dn];
            Some(op.apply(cpu, ops.ea, src, size))
        }
    })
}

/// `ADDA`/`SUBA`/`CMPA` — `<ea>` against a full 32-bit address register.
///
/// Two rules here, both measured:
///
/// - **`ADDA` and `SUBA` set no flags whatsoever.** Together with `ADDQ`/`SUBQ`
///   into `An` that is 6,450 disagreeing cases, 6,450/6,450 for "no flags"
///   (7,137 in the whole non-fault population, all CCR-unchanged).
/// - **a word-sized source is sign-extended, then compared against all 32 bits.**
///   At `.w`, 1,212 cases distinguish this from truncating the destination to a
///   word, and all 1,212 prefer sign extension. `CMPA.w #1,A0` with
///   `A0 = 0xFFFF0000` is the shape that shows it.
///
///   At `.l` the question is **not tested and cannot be**: sign-extending a long
///   source is the identity, so the two candidate rules predict the same result
///   in every case. `CMPA.l` therefore has zero discriminating cases. Long-size
///   sign extension here rests on `Size::sign_extend` being a no-op at `.l`
///   rather than on evidence — which is fine, but it is not a measurement, and
///   quoting one figure spanning both sizes would hide that.
fn alu_a(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, op: Op, size: Size) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let an = ((opcode >> 9) & 7) as usize;

    // CMPA always pays 2. ADDA/SUBA pay 4 for a word, and for a long pay 2 from
    // memory or 4 from a register or immediate.
    let idle = if op == Op::Cmp {
        2
    } else if size != Size::Long || !mode_is_mem(mode, reg) {
        4
    } else {
        2
    };

    let plan = Plan::new(size, mode, reg).idle(idle);
    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        let src = size.sign_extend(ops.ea);
        match op {
            Op::Cmp => {
                let (_, n, z, v, c) = sub_flags(cpu.a[an], src, false, Size::Long);
                cpu.set_ccr(cpu.ccr_x(), n, z, v, c);
            }
            Op::Add => cpu.a[an] = cpu.a[an].wrapping_add(src),
            Op::Sub => cpu.a[an] = cpu.a[an].wrapping_sub(src),
        }
        None
    })
}

/// `ADDX`/`SUBX` — add or subtract with the extend bit.
///
/// Two forms: `Dy,Dx` (mode 0) and `-(Ay),-(Ax)` (mode 1). The memory form's
/// source is resolved before its destination, so `ADDX.w -(A3),-(A3)` reads two
/// *different* addresses; [`alu::Plan::pair`] carries that.
///
/// # The `Z` rule
///
/// `Z_final = (result == 0) && Z_initial` — `Z` **accumulates**, so these
/// instructions can only ever clear it, never set it. This is what makes a chain
/// of `ADDX`s report "every limb was zero" for a multi-precision operand.
///
/// The task brief states this backwards. 146 cases distinguish the two readings;
/// the accumulating rule is 146/146 and the own-result rule 0/146. The control
/// group is the other half of the argument: `ADD.b`, `SUB.l`, `AND.b` and `NEG.b`
/// between them have 145 cases that go `Z=0 -> Z=1` (5, 10, 121 and 9), against
/// **zero** such cases in any size of `ADDX`, `SUBX` or `NEGX`. See
/// [`accumulate_z`] for why those counts are per-group populations, not
/// per-encoding.
fn addx_subx(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, op: Op, size: Size) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let x = ((opcode >> 9) & 7) as usize;

    let plan = if mode == 0 {
        // Dy,Dx: no memory operand at all.
        Plan::new(size, 0, reg).idle(if size == Size::Long { 4 } else { 0 })
    } else {
        // -(Ay),-(Ax): both operands are predecrementing memory, and both read
        // their long halves descending.
        Plan::new(size, 4, (opcode >> 9) & 7)
            .writes()
            .pair(4, reg, true)
    };
    let reg_form = mode == 0;

    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        let (dst, src) = if reg_form {
            (cpu.d[x], ops.ea)
        } else {
            (ops.ea, ops.src)
        };
        let xi = cpu.ccr_x();
        let zi = cpu.ccr_z();
        let (r, n, z, v, c) = if op == Op::Add {
            add_flags(dst, src, xi, size)
        } else {
            sub_flags(dst, src, xi, size)
        };
        cpu.set_ccr(c, n, accumulate_z(z, zi), v, c);
        if reg_form {
            let m = size.mask();
            cpu.d[x] = (cpu.d[x] & !m) | (r & m);
            None
        } else {
            Some(r)
        }
    })
}

/// `CMPM (Ay)+,(Ax)+`.
///
/// Postincrementing, so unlike `ADDX`/`SUBX` it has no leading idle and its long
/// reads ascend. The source still resolves first, so `CMPM.w (A3)+,(A3)+`
/// compares two consecutive words.
fn cmpm(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, size: Size) -> u32 {
    let plan = Plan::new(size, 3, (opcode >> 9) & 7).pair(3, opcode & 7, false);
    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        let (_, n, z, v, c) = sub_flags(ops.ea, ops.src, false, size);
        cpu.set_ccr(cpu.ccr_x(), n, z, v, c);
        None
    })
}

/// `ADDI`/`SUBI`/`CMPI` — an immediate against an `<ea>`.
///
/// The immediate occupies 1 word for `.b`/`.w` and 2 for `.l`, and it precedes
/// the `<ea>`'s own extension words in the instruction stream.
pub(super) fn immediate(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, op: Op, size: Size) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let pre = if size == Size::Long { 2 } else { 1 };

    let mut plan = Plan::new(size, mode, reg).pre(pre);
    if mode_is_mem(mode, reg) {
        if op != Op::Cmp {
            plan = plan.writes();
        }
    } else if size == Size::Long {
        plan = plan.idle(if op == Op::Cmp { 2 } else { 4 });
    }

    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        let r = op.apply(cpu, ops.ea, ops.imm, size);
        if op == Op::Cmp {
            None
        } else {
            Some(r)
        }
    })
}

/// `ADDQ`/`SUBQ #d,<ea>` — a 3-bit immediate held in the opcode, where 0 means 8.
///
/// An `An` destination is the special case: it takes all 32 bits regardless of
/// the size field, **sets no flags at all**, and pays 4 trailing idle cycles for
/// both `.w` and `.l`. Treating it as an ordinary destination costs ~2,000 cases.
fn quick(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, op: Op, size: Size) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let q = (opcode >> 9) & 7;
    let imm = if q == 0 { 8 } else { q as u32 };
    let an = mode == 1;

    let mut plan = Plan::new(size, mode, reg);
    if mode_is_mem(mode, reg) {
        plan = plan.writes();
    } else if an || size == Size::Long {
        // A register destination pays 4 for a long; an An destination pays 4 at
        // both its sizes, since it is a 32-bit operation whatever the size field
        // says.
        plan = plan.idle(4);
    }

    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        if an {
            // Full 32 bits, no flags.
            let r = reg as usize;
            cpu.a[r] = if op == Op::Add {
                cpu.a[r].wrapping_add(imm)
            } else {
                cpu.a[r].wrapping_sub(imm)
            };
            return None;
        }
        let r = op.apply(cpu, ops.ea, imm, size);
        Some(r)
    })
}

/// The four single-operand instructions in line 4, plus `NOT` from
/// [`super::logic`]'s point of view.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Single {
    Neg,
    Negx,
    Clr,
    Not,
    Tst,
}

/// `NEG`, `NEGX`, `CLR`, `NOT` and `TST`.
///
/// `CLR` still **reads its destination** before writing zero — a read-modify-write
/// even though the value read is discarded. That is visible on the bus and in the
/// cycle count, and it is also why a misaligned `CLR` raises a *read* fault.
///
/// `CLR` sets `Z` and clears `N`, which sounds too obvious to measure until you
/// consider that the alternative (deriving them from the pre-clear value) is what
/// a naive "compute flags from the operand" path produces: 5,494 cases
/// distinguish them, 5,494/5,494 for the constant rule. (`CLR`'s whole non-fault
/// population is 5,510; the 16-case gap is operands that were already zero and
/// non-negative, where the two rules agree and nothing is under test.)
pub(super) fn single(
    cpu: &mut M68k,
    bus: &mut dyn Bus,
    opcode: u16,
    which: Single,
    size: Size,
) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let writes = which != Single::Tst;

    let mut plan = Plan::new(size, mode, reg);
    if mode_is_mem(mode, reg) {
        if writes {
            plan = plan.writes();
        }
    } else if writes && size == Size::Long {
        plan = plan.idle(2);
    }

    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        let v = ops.ea;
        let r = match which {
            Single::Neg => {
                let (r, n, z, vf, c) = sub_flags(0, v, false, size);
                cpu.set_ccr(c, n, z, vf, c);
                r
            }
            Single::Negx => {
                let xi = cpu.ccr_x();
                let zi = cpu.ccr_z();
                let (r, n, z, vf, c) = sub_flags(0, v, xi, size);
                cpu.set_ccr(c, n, accumulate_z(z, zi), vf, c);
                r
            }
            Single::Clr => {
                cpu.set_ccr(cpu.ccr_x(), false, true, false, false);
                0
            }
            Single::Not => {
                let r = !v & size.mask();
                let (n, z, vf, c) = crate::flags::logic_flags(r, size);
                cpu.set_ccr(cpu.ccr_x(), n, z, vf, c);
                r
            }
            Single::Tst => {
                let (n, z, vf, c) = crate::flags::logic_flags(v, size);
                cpu.set_ccr(cpu.ccr_x(), n, z, vf, c);
                0
            }
        };
        if writes {
            Some(r)
        } else {
            None
        }
    })
}

// --- Dispatch-table installation ------------------------------------------

/// Sizes as encoded in bits 7-6, for the families that use that field.
pub(super) const SIZES: [(u16, Size); 3] = [(0, Size::Byte), (1, Size::Word), (2, Size::Long)];

/// True if `(mode, reg)` may be the `<ea>` of a *source* operand of this size:
/// any mode, but `An` only above byte size and mode 7 only up to the immediate.
pub(super) fn valid_src(mode: u16, reg: u16, size: Size) -> bool {
    match mode {
        1 => size != Size::Byte,
        7 => reg <= 4,
        _ => true,
    }
}

/// True if `(mode, reg)` is a writable memory `<ea>`: the alterable-memory set,
/// which excludes registers, the PC-relative modes and immediates.
pub(super) fn valid_mem_dst(mode: u16, reg: u16) -> bool {
    match mode {
        0 | 1 => false,
        7 => reg <= 1,
        _ => true,
    }
}

/// True if `(mode, reg)` is a data-alterable `<ea>`: writable memory *or* a data
/// register. This is `NEG`/`NEGX`/`CLR`/`NOT`/`TST`'s destination set and
/// `ADDQ`/`SUBQ`'s at byte size.
pub(super) fn valid_data_alterable(mode: u16, reg: u16) -> bool {
    mode == 0 || valid_mem_dst(mode, reg)
}

macro_rules! handlers {
    ($($name:ident($op:expr, $size:expr, $body:path $(, $extra:expr)*);)*) => {
        $(fn $name(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
            $body(cpu, bus, opcode, $op, $size $(, $extra)*)
        })*
    };
}

handlers! {
    add_ea_dn_b(Op::Add, Size::Byte, alu_ea_dn, Dir::ToReg);
    add_ea_dn_w(Op::Add, Size::Word, alu_ea_dn, Dir::ToReg);
    add_ea_dn_l(Op::Add, Size::Long, alu_ea_dn, Dir::ToReg);
    add_dn_ea_b(Op::Add, Size::Byte, alu_ea_dn, Dir::ToMem);
    add_dn_ea_w(Op::Add, Size::Word, alu_ea_dn, Dir::ToMem);
    add_dn_ea_l(Op::Add, Size::Long, alu_ea_dn, Dir::ToMem);
    sub_ea_dn_b(Op::Sub, Size::Byte, alu_ea_dn, Dir::ToReg);
    sub_ea_dn_w(Op::Sub, Size::Word, alu_ea_dn, Dir::ToReg);
    sub_ea_dn_l(Op::Sub, Size::Long, alu_ea_dn, Dir::ToReg);
    sub_dn_ea_b(Op::Sub, Size::Byte, alu_ea_dn, Dir::ToMem);
    sub_dn_ea_w(Op::Sub, Size::Word, alu_ea_dn, Dir::ToMem);
    sub_dn_ea_l(Op::Sub, Size::Long, alu_ea_dn, Dir::ToMem);
    cmp_b(Op::Cmp, Size::Byte, alu_ea_dn, Dir::ToReg);
    cmp_w(Op::Cmp, Size::Word, alu_ea_dn, Dir::ToReg);
    cmp_l(Op::Cmp, Size::Long, alu_ea_dn, Dir::ToReg);

    adda_w(Op::Add, Size::Word, alu_a);
    adda_l(Op::Add, Size::Long, alu_a);
    suba_w(Op::Sub, Size::Word, alu_a);
    suba_l(Op::Sub, Size::Long, alu_a);
    cmpa_w(Op::Cmp, Size::Word, alu_a);
    cmpa_l(Op::Cmp, Size::Long, alu_a);

    addx_b(Op::Add, Size::Byte, addx_subx);
    addx_w(Op::Add, Size::Word, addx_subx);
    addx_l(Op::Add, Size::Long, addx_subx);
    subx_b(Op::Sub, Size::Byte, addx_subx);
    subx_w(Op::Sub, Size::Word, addx_subx);
    subx_l(Op::Sub, Size::Long, addx_subx);

    addi_b(Op::Add, Size::Byte, immediate);
    addi_w(Op::Add, Size::Word, immediate);
    addi_l(Op::Add, Size::Long, immediate);
    subi_b(Op::Sub, Size::Byte, immediate);
    subi_w(Op::Sub, Size::Word, immediate);
    subi_l(Op::Sub, Size::Long, immediate);
    cmpi_b(Op::Cmp, Size::Byte, immediate);
    cmpi_w(Op::Cmp, Size::Word, immediate);
    cmpi_l(Op::Cmp, Size::Long, immediate);

    addq_b(Op::Add, Size::Byte, quick);
    addq_w(Op::Add, Size::Word, quick);
    addq_l(Op::Add, Size::Long, quick);
    subq_b(Op::Sub, Size::Byte, quick);
    subq_w(Op::Sub, Size::Word, quick);
    subq_l(Op::Sub, Size::Long, quick);

    neg_b(Single::Neg, Size::Byte, single);
    neg_w(Single::Neg, Size::Word, single);
    neg_l(Single::Neg, Size::Long, single);
    negx_b(Single::Negx, Size::Byte, single);
    negx_w(Single::Negx, Size::Word, single);
    negx_l(Single::Negx, Size::Long, single);
    clr_b(Single::Clr, Size::Byte, single);
    clr_w(Single::Clr, Size::Word, single);
    clr_l(Single::Clr, Size::Long, single);
    tst_b(Single::Tst, Size::Byte, single);
    tst_w(Single::Tst, Size::Word, single);
    tst_l(Single::Tst, Size::Long, single);
}

fn cmpm_b(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    cmpm(cpu, bus, opcode, Size::Byte)
}
fn cmpm_w(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    cmpm(cpu, bus, opcode, Size::Word)
}
fn cmpm_l(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    cmpm(cpu, bus, opcode, Size::Long)
}

/// Installs one `1xxx` arithmetic line: `ADD`/`SUB` shape, or `CMP` shape when
/// `x_is_cmpm` (in which case opmode 4/5/6 mode 1 is `CMPM` rather than `ADDX`,
/// and the other modes belong to `EOR` and are left to [`super::logic`]).
///
/// Shared because `1001`/`1101` and `1011` differ in exactly two places, and a
/// second copy of this loop is a second place for the opmode table to rot.
#[allow(clippy::too_many_arguments)]
fn register_line(
    table: &mut [Handler; 65536],
    line: u16,
    to_reg: [Handler; 3],
    to_mem: Option<[Handler; 3]>,
    to_a: [Handler; 2],
    x_form: [Handler; 3],
    x_is_cmpm: bool,
) {
    for dn in 0..8u16 {
        for opmode in 0..8u16 {
            for mode in 0..8u16 {
                for reg in 0..8u16 {
                    let op = (line << 12) | (dn << 9) | (opmode << 6) | (mode << 3) | reg;
                    let h: Handler = match opmode {
                        // <ea> op Dn -> Dn. An is a legal source only above byte
                        // size; CMP additionally allows it at word and long.
                        0..=2 => {
                            let size = [Size::Byte, Size::Word, Size::Long][opmode as usize];
                            if !valid_src(mode, reg, size) {
                                continue;
                            }
                            to_reg[opmode as usize]
                        }
                        // ADDA/SUBA/CMPA: any source, including An at both sizes.
                        3 | 7 => {
                            if !valid_src(mode, reg, Size::Word) {
                                continue;
                            }
                            to_a[usize::from(opmode == 7)]
                        }
                        // 4/5/6: ADDX/SUBX or CMPM at modes 0 and 1, memory
                        // destination otherwise.
                        _ => {
                            let i = (opmode - 4) as usize;
                            if x_is_cmpm {
                                // CMP line: mode 1 is CMPM, everything else EOR.
                                if mode != 1 {
                                    continue;
                                }
                                x_form[i]
                            } else if mode == 0 || mode == 1 {
                                x_form[i]
                            } else {
                                let Some(to_mem) = to_mem else { continue };
                                if !valid_mem_dst(mode, reg) {
                                    continue;
                                }
                                to_mem[i]
                            }
                        }
                    };
                    table[op as usize] = h;
                }
            }
        }
    }
}

/// Installs every instruction this module owns.
pub fn register(table: &mut [Handler; 65536]) {
    // 1101 ADD / 1001 SUB, and 1011 CMP (whose opmode 4/5/6 is CMPM + EOR).
    register_line(
        table,
        0xD,
        [add_ea_dn_b, add_ea_dn_w, add_ea_dn_l],
        Some([add_dn_ea_b, add_dn_ea_w, add_dn_ea_l]),
        [adda_w, adda_l],
        [addx_b, addx_w, addx_l],
        false,
    );
    register_line(
        table,
        0x9,
        [sub_ea_dn_b, sub_ea_dn_w, sub_ea_dn_l],
        Some([sub_dn_ea_b, sub_dn_ea_w, sub_dn_ea_l]),
        [suba_w, suba_l],
        [subx_b, subx_w, subx_l],
        false,
    );
    register_line(
        table,
        0xB,
        [cmp_b, cmp_w, cmp_l],
        None,
        [cmpa_w, cmpa_l],
        [cmpm_b, cmpm_w, cmpm_l],
        true,
    );

    // 0000 xxxI: bits 11-9 select the operation, 7-6 the size. ORI/ANDI/EORI
    // are registered by `super::logic`, which owns the same encoding space.
    for (family, hs) in [
        (2u16, [subi_b, subi_w, subi_l]),
        (3, [addi_b, addi_w, addi_l]),
        (6, [cmpi_b, cmpi_w, cmpi_l]),
    ] {
        super::logic::register_immediate_family(table, family, hs);
    }

    // 0101 ADDQ/SUBQ: bit 8 selects the direction, 11-9 the immediate, 7-6 the
    // size. Bits 7-6 == 3 is Scc/DBcc, which belongs to a later task.
    for (bit8, hs) in [
        (0u16, [addq_b, addq_w, addq_l]),
        (1, [subq_b, subq_w, subq_l]),
    ] {
        for q in 0..8u16 {
            for (sb, size) in SIZES {
                for mode in 0..8u16 {
                    for reg in 0..8u16 {
                        // An is a destination at word and long size only.
                        let ok = if mode == 1 {
                            size != Size::Byte
                        } else {
                            valid_data_alterable(mode, reg)
                        };
                        if !ok {
                            continue;
                        }
                        let op = 0x5000 | (q << 9) | (bit8 << 8) | (sb << 6) | (mode << 3) | reg;
                        table[op as usize] = hs[sb as usize];
                    }
                }
            }
        }
    }

    // 0100 single-operand: bits 11-8 select the instruction, 7-6 the size.
    for (sel, hs) in [
        (0x0u16, [negx_b, negx_w, negx_l]),
        (0x2, [clr_b, clr_w, clr_l]),
        (0x4, [neg_b, neg_w, neg_l]),
        (0xA, [tst_b, tst_w, tst_l]),
    ] {
        for (sb, _size) in SIZES {
            for mode in 0..8u16 {
                for reg in 0..8u16 {
                    if !valid_data_alterable(mode, reg) {
                        continue;
                    }
                    let op = 0x4000 | (sel << 8) | (sb << 6) | (mode << 3) | reg;
                    table[op as usize] = hs[sb as usize];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::{FlatBus, RecordingBus};
    use crate::cpu::{SR_S, SR_X, SR_Z};
    use crate::decode::Decoder;

    fn at(bus: &mut impl Bus) -> M68k {
        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(bus);
        cpu
    }

    /// `ADD.b D1,D0` — the simplest form, checking flags and the byte-width
    /// write into a register.
    #[test]
    fn add_byte_register_to_register() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xD001, 0x4E71]); // ADD.b D1,D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x1234_5680;
        cpu.d[1] = 0x0000_0080;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 0x1234_5600, "upper bits untouched");
        assert!(cpu.ccr_z() && cpu.ccr_v() && cpu.ccr_c() && cpu.ccr_x());
        assert_eq!(cycles, 4);
    }

    /// `ADDX` accumulates `Z`: a zero result must not *set* a clear `Z`.
    /// This is the rule the brief states backwards.
    #[test]
    fn addx_accumulates_z_and_never_sets_it() {
        let dec = Decoder::new();

        // Z initially clear: a zero result leaves it clear.
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xD101, 0x4E71]); // ADDX.b D1,D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0;
        cpu.d[1] = 0;
        cpu.step_with(&dec, &mut bus);
        assert!(!cpu.ccr_z(), "a zero result must not set a clear Z");

        // Z initially set: a zero result keeps it set.
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xD101, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_Z;
        cpu.d[0] = 0;
        cpu.d[1] = 0;
        cpu.step_with(&dec, &mut bus);
        assert!(cpu.ccr_z(), "zero limb with Z set stays set");

        // A nonzero result clears an initially-set Z.
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xD101, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_Z;
        cpu.d[0] = 1;
        cpu.d[1] = 1;
        cpu.step_with(&dec, &mut bus);
        assert!(!cpu.ccr_z());
    }

    /// `ADDX` consumes `X` as a carry-in.
    #[test]
    fn addx_uses_the_extend_bit() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xD101, 0x4E71]); // ADDX.b D1,D0
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_X | SR_Z;
        cpu.d[0] = 1;
        cpu.d[1] = 1;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0] & 0xFF, 3, "1 + 1 + X");
    }

    /// `ADDX.w -(A3),-(A3)`: the source decrement commits before the
    /// destination address is formed, so the two operands come from different
    /// addresses. 526 suite cases carry this encoding — `ADDX`/`SUBX -(Ay),-(Ax)`
    /// with `Ay == Ax`, non-faulting, excluding `A7` — but the bus log this test
    /// asserts is the real evidence, since a population count says only that the
    /// shape occurs, not that the addresses differ.
    #[test]
    fn addx_with_the_same_register_reads_two_addresses() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0xD74B, 0x4E71, 0x4E71]); // ADDX.w -(A3),-(A3)
        bus.put16(0x2000, 0x0003); // the destination word
        bus.put16(0x1FFE, 0x0004); // the source word
        let mut cpu = at(&mut bus);
        cpu.a[3] = 0x2002;
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.a[3], 0x1FFE, "two decrements");
        assert_eq!(
            bus.log,
            vec![
                (false, 0x2000, 0x0003), // source at 0x2000
                (false, 0x1FFE, 0x0004), // destination at 0x1FFE
                (false, 0x1004, 0x4E71), // the queue advance
                (true, 0x1FFE, 0x0007),  // the result
            ]
        );
    }

    /// `ADDX.l -(A1),-(A2)` reads both long operands **descending** and splits
    /// its descending write around the queue advance. `CMPM.l` ascends, so this
    /// is per-instruction, not per-addressing-mode.
    #[test]
    fn addx_long_memory_form_reads_and_writes_descending() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0xD589, 0x4E71, 0x4E71]); // ADDX.l -(A1),-(A2)
        bus.put16(0x2000, 0x0000);
        bus.put16(0x2002, 0x0001); // source = 1
        bus.put16(0x2100, 0x0000);
        bus.put16(0x2102, 0x0002); // destination = 2
        let mut cpu = at(&mut bus);
        cpu.a[1] = 0x2004;
        cpu.a[2] = 0x2104;
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.log,
            vec![
                (false, 0x2002, 0x0001), // source low word first
                (false, 0x2000, 0x0000),
                (false, 0x2102, 0x0002), // destination low word first
                (false, 0x2100, 0x0000),
                (true, 0x2102, 0x0003),  // result low word
                (false, 0x1004, 0x4E71), // the queue advance, mid-write
                (true, 0x2100, 0x0000),  // result high word
            ]
        );
    }

    /// `CMPM.l (A1)+,(A2)+` reads ascending and writes nothing.
    #[test]
    fn cmpm_long_reads_ascending() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0xB589, 0x4E71, 0x4E71]); // CMPM.l (A1)+,(A2)+
        let mut cpu = at(&mut bus);
        cpu.a[1] = 0x2000;
        cpu.a[2] = 0x2100;
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.log,
            vec![
                (false, 0x2000, 0x0000), // source high word first
                (false, 0x2002, 0x0000),
                (false, 0x2100, 0x0000), // destination high word first
                (false, 0x2102, 0x0000),
                (false, 0x1004, 0x4E71),
            ]
        );
        assert_eq!(cpu.a[1], 0x2004);
        assert_eq!(cpu.a[2], 0x2104);
    }

    /// `CMPA.w` sign-extends its source and compares all 32 bits. Truncating
    /// the destination to a word instead would report equality here.
    #[test]
    fn cmpa_word_sign_extends_the_source() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xB0C1, 0x4E71]); // CMPA.w D1,A0
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0xFFFF_0001;
        cpu.d[1] = 0x0000_0001;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert!(
            !cpu.ccr_z(),
            "0xFFFF0001 != 0x00000001 across the full 32 bits"
        );
        assert!(cpu.ccr_n(), "the difference is negative");
    }

    /// `ADDA` and `ADDQ #d,An` set no flags at all.
    #[test]
    fn address_register_arithmetic_sets_no_flags() {
        let dec = Decoder::new();

        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xD0C1, 0x4E71]); // ADDA.w D1,A0
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_X | SR_Z;
        cpu.a[0] = 0;
        cpu.d[1] = 0x8000;
        let before = cpu.sr & 0x1F;
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.a[0], 0xFFFF_8000, "word source sign-extended");
        assert_eq!(cpu.sr & 0x1F, before, "ADDA touches no flag");

        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x5248, 0x4E71]); // ADDQ.w #1,A0
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_X | SR_Z;
        cpu.a[0] = 0xFFFF_FFFF;
        let before = cpu.sr & 0x1F;
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.a[0], 0, "full 32 bits, wrapping");
        assert_eq!(cpu.sr & 0x1F, before, "ADDQ to An touches no flag");
    }

    /// `CLR` reads its destination before writing zero — a discarded read that
    /// is nonetheless visible on the bus.
    #[test]
    fn clr_still_reads_its_destination() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4252, 0x4E71, 0x4E71]); // CLR.w (A2)
        bus.put16(0x2000, 0xABCD);
        let mut cpu = at(&mut bus);
        cpu.a[2] = 0x2000;
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.log,
            vec![
                (false, 0x2000, 0xABCD), // the discarded read
                (false, 0x1004, 0x4E71),
                (true, 0x2000, 0x0000),
            ]
        );
        assert!(cpu.ccr_z() && !cpu.ccr_n());
    }

    /// A long read-modify-write writes its destination **descending**, unlike
    /// MOVE where only `-(An)` does.
    #[test]
    fn long_read_modify_write_writes_descending() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4492, 0x4E71, 0x4E71]); // NEG.l (A2)
        bus.put16(0x2000, 0x0000);
        bus.put16(0x2002, 0x0001);
        let mut cpu = at(&mut bus);
        cpu.a[2] = 0x2000;
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.writes(),
            vec![(0x2002, 0xFFFF), (0x2000, 0xFFFF)],
            "low word first"
        );
    }

    /// A misaligned read-modify-write destination faults on the **read**, so no
    /// write ever happens and the CCR is untouched.
    #[test]
    fn misaligned_rmw_destination_raises_a_read_fault() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4452, 0x4E71]); // NEG.w (A2)
        bus.put16(0x000C, 0x0000); // vector 3
        bus.put16(0x000E, 0x4000);
        bus.load(0x4000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.a[2] = 0x2001;
        cpu.sr |= SR_X | SR_Z;
        let before = cpu.sr & 0x1F;
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert!(
            !bus.log
                .iter()
                .any(|&(w, a, _)| w && (a == 0x2000 || a == 0x2001)),
            "the faulting access must not reach the bus"
        );
        assert_eq!(cpu.sr & 0x1F, before, "a read fault leaves the CCR alone");
        assert_eq!(bus.read16(0x2FF8), 0x4452, "IR is the opcode");
        // status = (IR & 0xFFE0) | read | fc, fc = supervisor|data = 5
        assert_eq!(bus.read16(0x2FF2), (0x4452 & 0xFFE0) | 0x10 | 5);
    }

    /// `TST` writes nothing, so a misaligned `TST` is still a read fault and a
    /// legal one performs a single read.
    #[test]
    fn tst_reads_without_writing() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4A52, 0x4E71, 0x4E71]); // TST.w (A2)
        bus.put16(0x2000, 0x8000);
        let mut cpu = at(&mut bus);
        cpu.a[2] = 0x2000;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert!(bus.writes().is_empty(), "TST never writes");
        assert!(cpu.ccr_n() && !cpu.ccr_z());
        assert_eq!(cycles, 8, "one read, one fetch, no idle");
    }

    /// `SUBQ #8,Dn` — a quick immediate of 0 encodes 8.
    #[test]
    fn quick_immediate_zero_means_eight() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x5100, 0x4E71]); // SUBQ.b #8,D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 10;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0], 2);
    }
}
