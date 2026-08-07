//! The shared bus schedule behind every Task 6 arithmetic and logic handler.
//!
//! [`ops::arith`](super::arith) and [`ops::logic`](super::logic) differ only in
//! what they compute and which flags they set; the *schedule* — which bus cycle
//! happens when, and what the instruction costs — is one model across all 45
//! sized groups. This module owns it, and knows nothing about flags.
//!
//! # The schedule
//!
//! Measured against every non-address-error case in all 45 sized groups plus
//! `NOP` — **85,104/85,104 exact**, comparing the ordered access sequence
//! (direction and address space), the total cycle count, and the PC advance.
//!
//! ```text
//! pre  = extension words consumed BEFORE the <ea>'s own (the xxxI immediate)
//! de   = ext_words(mode, reg, size)          N = 1 + pre + de
//! nr   = operand words read      nw = operand words written
//!
//!   pre program fetches            (the immediate)
//!   de  program fetches            (the ea's own extension words)
//!   nr  operand reads
//!   1   program fetch              (the opcode's own queue advance)
//!   nw  operand writes
//!
//! cycles = 4 * (nr + nw + N) + idle        dpc = 2 * N
//! ```
//!
//! This is *not* MOVE's schedule ([`ops::move_`](super::move_)): there is only
//! one `<ea>`, and the single queue advance always sits between the reads and
//! the writes. The one structural exception is `ADDX.l`/`SUBX.l` into `-(Ax)`,
//! which splits its two writes around that fetch — see `Plan::pair`.
//!
//! Idle cycles are **not** ordered against the accesses here, only totalled.
//! The harness compares `Read`/`Write` transactions in sequence but matches
//! `Idle` ones only through the cycle count, so where an idle falls within an
//! instruction is unobservable and this module does not model it.
//!
//! # The trailing idle
//!
//! This is where the families diverge, and the divergence is not a function of
//! the addressing mode. Each row is set by the handler that builds the `Plan`;
//! collected here because the pattern is only visible side by side:
//!
//! ```text
//!   .b/.w, any family, any mode                                      0
//!   .l  <ea> op Dn -> Dn        register or immediate <ea>           4
//!                               memory <ea>                          2
//!   .l  CMP <ea>,Dn             any <ea>                             2
//!   .l  EOR Dn,Dn                                                    4
//!   .w  ADDA/SUBA               any <ea>                             4
//!   .l  ADDA/SUBA               register or immediate <ea>           4
//!                               memory <ea>                          2
//!   .w/.l CMPA                  any <ea>                             2
//!   .l  ADDX/SUBX Dy,Dx                                              4
//!   .l  xxxI #imm,Dn            (CMPI: 2)                            4
//!   .l  ADDQ/SUBQ #d,Dn                                              4
//!   .w/.l ADDQ/SUBQ #d,An                                            4
//!   .l  NEG/NEGX/CLR/NOT Dn                                          2
//!       TST anything                                                 0
//!   any read-modify-write memory destination                          0
//! ```
//!
//! Every row was cross-checked against the timing law (`cycles = 4 × non-idle
//! accesses + idle`), which is what catches a miscounted access before it turns
//! into a debugging session.
//!
//! # 32-bit access order
//!
//! Three orders coexist, all measured over the `.l` groups:
//!
//! - operand reads **ascend** (high word first) in every mode — the general case;
//! - `ADDX`/`SUBX`'s `-(Ay),-(Ax)` reads **descend**, both operands, which is why
//!   `Plan::desc_reads` exists. `CMPM`'s `(Ay)+,(Ax)+` reads ascend, so this is
//!   a property of the instruction and not of the memory-pair form;
//! - **every** long read-modify-write destination writes **descending**
//!   (low word first): 8,360/8,360 across all of `NEG`, `NEGX`, `NOT`, `CLR`,
//!   `EOR`, `ADD`/`SUB` to memory, `ADDQ`/`SUBQ`, `ADDX`/`SUBX` and the `xxxI`
//!   forms, in every addressing mode, with zero ascending counterexamples.
//!
//! That last one differs from MOVE, where only `-(An)` descends and every other
//! destination ascends. So this module routes *all* long destination writes
//! through [`ea::write_predec_long`] and never through [`ea::write`], despite
//! the name.

use crate::cpu::{M68k, ADDR_MASK};
use crate::ea::{self, mode_is_mem, Ea, Size};
use crate::exception::{self, FaultKind, Space, ADDRESS_ERROR_TAIL_CYCLES};
use crate::Bus;

/// Words a memory operand of this size occupies on the bus.
#[inline]
fn operand_words(size: Size) -> u32 {
    if size == Size::Long {
        2
    } else {
        1
    }
}

/// Address-error alignment check: word and long accesses fault on an odd
/// address, byte accesses never do.
#[inline]
fn misaligned(addr: u32, size: Size) -> bool {
    size != Size::Byte && addr & 1 != 0
}

/// Address space of an operand access: program space iff the mode is
/// PC-relative. Immediates are program-space too, but they arrive through the
/// prefetch queue rather than an operand access, so they never reach here.
#[inline]
fn operand_space(mode: u16, reg: u16) -> Space {
    if mode == 7 && (reg == 2 || reg == 3) {
        Space::Program
    } else {
        Space::Data
    }
}

/// The operands a handler's `compute` closure is called with.
pub(super) struct Ops {
    /// The `<ea>` operand: the source for `<ea> op Dn` and the destination for
    /// everything that writes back.
    pub ea: u32,
    /// A memory-pair form's source operand; 0 when there is no pair.
    pub src: u32,
    /// The `xxxI` immediate, already narrowed to the operand size; 0 when
    /// [`Plan::pre`] is 0.
    pub imm: u32,
    /// The PC to stack if the closure raises an exception:
    /// `opcode_addr + 2 * (1 + pre + ext_words)`, i.e. past the *whole*
    /// instruction. Measured 1326/1326 over every trapping `CHK` case, with
    /// each of the 11 addressing-mode buckets a singleton — the same rule Task 8
    /// measured for `JSR`.
    ///
    /// Supplied rather than left to the closure because `cpu.pc - 2` *happens*
    /// to equal this at the moment `compute` runs (the extension fetches are
    /// done, the queue advance is not) and so would pass the suite while
    /// encoding the wrong intent. A fixed `opcode_addr + 2` — which is what that
    /// expression means anywhere else in a handler — scores 851/1326.
    pub trap_pc: u32,
}

/// What a handler does once its operands are in hand.
///
/// Returning `None` means "write nothing back to the `<ea>`" — that covers both
/// the compare-and-discard instructions (`CMP`, `CMPI`, `CMPM`, `CMPA`, `TST`)
/// and the forms whose destination is a register the closure assigns itself
/// (`<ea> op Dn`, `ADDA`, `ADDQ #d,An`). The closure sets the CCR, because the
/// flag rules are exactly what differs between families.
pub(super) type Compute<'a> = &'a mut dyn FnMut(&mut M68k, Ops) -> Option<u32>;

/// How an instruction finishes, once its operands are in hand.
///
/// [`Compute`] cannot express the third arm, so the instructions that may trap
/// on their operand *value* — `CHK` and the divides — use [`ComputeTail`]
/// instead. The distinction is observable on the bus: a trapping instruction
/// never performs the queue advance, because the pipeline aborted before it.
///
/// A trapping `CHK Dn,Dn` logs exactly seven accesses — the three frame writes,
/// the two vector reads and the two prefetch refills — with no eighth for a
/// queue advance. Measured at `cycles - idle == 28` in all 305 of them.
pub(super) enum Tail {
    /// Write this value back to the `<ea>`, after the queue advance.
    Write(u32),
    /// Advance the queue and finish; nothing goes back to the `<ea>`.
    Done,
    /// The closure raised an exception itself, and has already run the frame's
    /// bus cycles. [`run_tail`] skips the queue advance and charges
    /// [`exception::SHORT_FRAME_ACCESSES`] for the frame.
    Trapped,
}

/// [`Compute`] plus the bus, so the closure can raise an exception.
///
/// The bus is threaded through rather than captured because [`run_tail`] holds
/// it for the duration of the schedule; a closure capturing it would alias.
pub(super) type ComputeTail<'a> = &'a mut dyn FnMut(&mut M68k, &mut dyn Bus, Ops) -> Tail;

/// One instruction's schedule, fixed before any bus cycle happens.
pub(super) struct Plan {
    /// Operand size, governing the bus width and the alignment check.
    pub size: Size,
    /// The `<ea>`. For a memory-pair form this is the **destination**, already
    /// translated out of the opcode's `mode == 1` into the real mode (4 for
    /// `-(Ax)`, 3 for `(Ax)+`).
    pub mode: u16,
    pub reg: u16,
    /// Extension words consumed *before* the `<ea>`'s own: 1 or 2 for the `xxxI`
    /// immediate forms, 0 otherwise.
    ///
    /// This shifts where the `<ea>`'s own extension words sit in the instruction
    /// stream, and it is also a term in the address-error stacked PC — adding it
    /// there took the `xxxI` fault cases from 0/746 to 746/746.
    pub pre: u32,
    /// Does the result go back to a *memory* `<ea>`?
    ///
    /// Read only by [`run`]'s `debug_assert!`, which checks that a plan claiming
    /// a write-back gets one. It does **not** gate the write — `compute`
    /// returning `Some` decides that — and the assertion exists because for
    /// thirteen builder call sites there was no read at all, so the field looked
    /// causal while being inert. See the assertion for why the invariant is an
    /// implication rather than an equality.
    pub writes: bool,
    /// Trailing idle cycles; see the module docs' table.
    pub idle: u32,
    /// `ADDX`/`SUBX`/`CMPM`'s memory form, as `Some((src_mode, src_reg))`.
    ///
    /// A mode field of `001` in these encodings is **not** an address register:
    /// it selects `-(Ay),-(Ax)` for `ADDX`/`SUBX` and `(Ay)+,(Ax)+` for `CMPM`.
    /// Decoding it as register-direct leaves every memory-pair case wrong.
    ///
    /// The source resolves *first*, so its adjustment is committed before the
    /// destination address is formed. That is observable whenever both name the
    /// same register: `ADDX.w -(A3),-(A3)` reads its two operands from
    /// *different* addresses.
    ///
    /// **639 cases turn on it**, measured two independent ways that agree: a
    /// census of the encodings (915 same-register memory-pair cases, of which
    /// 276 fault before the second resolve, leaving 639), and a mutant that
    /// rolls the source's predecrement back — which fails exactly 639 cases,
    /// spread over all six groups (`ADDX.b` 158, `ADDX.w` 88, `ADDX.l` 82,
    /// `SUBX.b` 161, `SUBX.w` 76, `SUBX.l` 74). That is the count over *all*
    /// same-register non-faulting cases, `A7` included.
    ///
    /// ⚠️ Earlier revisions of this line said **514**, which reproduces under no
    /// predicate tried. `arith.rs`'s test doc says **526** and is *correct* — it
    /// states a narrower population, "non-faulting, excluding `A7`", and the 113
    /// `A7` cases are exactly the difference. The ledger recorded 514-vs-526 as
    /// one stale figure needing alignment; they are two scopes, and only the
    /// first was wrong. **State the scope beside the count**, or the next reader
    /// reconciles two right answers into one wrong one.
    pub pair: Option<(u16, u16)>,
    /// Read a long operand low word first. `ADDX`/`SUBX`'s memory form only.
    pub desc_reads: bool,
    /// Advance the prefetch queue *after* the write-back, not before it.
    ///
    /// Every other memory-write form on this core fetches then writes
    /// (`alu.rs`'s default arm). `TAS` is the sole exception: across all 2,108
    /// memory-form cases a program read *follows* the write, and the control
    /// "the write is the last non-idle transaction" scores 0/2108. The suite
    /// does not model TAS's bus-locked RMW as indivisible — it emits an ordinary
    /// read, idle, write — but it is unanimous about this ordering.
    pub fetch_last: bool,
}

impl Plan {
    /// A plan for an ordinary single-`<ea>` instruction.
    pub fn new(size: Size, mode: u16, reg: u16) -> Self {
        Self {
            size,
            mode,
            reg,
            pre: 0,
            writes: false,
            idle: 0,
            pair: None,
            desc_reads: false,
            fetch_last: false,
        }
    }

    pub fn writes(mut self) -> Self {
        self.writes = true;
        self
    }

    /// See [`Plan::fetch_last`]. `TAS`'s memory form is the only user.
    pub fn fetch_last(mut self) -> Self {
        self.fetch_last = true;
        self
    }

    pub fn pre(mut self, words: u32) -> Self {
        self.pre = words;
        self
    }

    pub fn idle(mut self, cycles: u32) -> Self {
        self.idle = cycles;
        self
    }

    /// Marks this as a memory-pair form with the given source mode/register.
    pub fn pair(mut self, src_mode: u16, src_reg: u16, desc_reads: bool) -> Self {
        self.pair = Some((src_mode, src_reg));
        self.desc_reads = desc_reads;
        self
    }
}

/// Reads a memory operand, honouring [`Plan::desc_reads`].
fn read_operand(cpu: &M68k, bus: &mut dyn Bus, ea: Ea, size: Size, desc: bool) -> u32 {
    match ea {
        Ea::Mem(addr) if desc && size == Size::Long => {
            let a = addr & ADDR_MASK;
            let lo = bus.read16(a.wrapping_add(2) & ADDR_MASK) as u32;
            let hi = bus.read16(a) as u32;
            (hi << 16) | lo
        }
        _ => ea::read(cpu, bus, ea, size),
    }
}

/// Address of the first word a read of `addr` touches — which is the word that
/// faults, since a misaligned operand faults on its first access.
#[inline]
fn first_access(addr: u32, size: Size, desc: bool) -> u32 {
    if desc && size == Size::Long {
        addr.wrapping_add(2)
    } else {
        addr
    }
}

/// Runs a plan: emits the bus schedule, calls `compute` at the right point, and
/// returns the cycle count. Raises an address error instead if an operand access
/// would be misaligned.
pub(super) fn run(cpu: &mut M68k, bus: &mut dyn Bus, plan: &Plan, compute: Compute) -> u32 {
    run_tail(
        cpu,
        bus,
        plan,
        &mut |cpu, _bus, ops| match compute(cpu, ops) {
            Some(v) => Tail::Write(v),
            None => Tail::Done,
        },
    )
}

/// [`run`], but the closure may raise an exception instead of producing a value.
///
/// The only difference in the schedule is at the very end: a [`Tail::Trapped`]
/// closure skips the queue advance, because the pipeline aborted before it, and
/// is charged for the short frame's own seven accesses rather than for one
/// fetch. Everything before `compute` — the extension fetches, the alignment
/// check, the operand read — is byte-for-byte the same code, which is the point
/// of routing `CHK` and the divides through here rather than hand-rolling them.
pub(super) fn run_tail(
    cpu: &mut M68k,
    bus: &mut dyn Bus,
    plan: &Plan,
    compute: ComputeTail,
) -> u32 {
    let size = plan.size;
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let ir = cpu.prefetch[0];

    let de = ea::ext_words(plan.mode, plan.reg, size);
    let n_words = 1 + plan.pre + de;
    let wper = operand_words(size);

    // The instruction's word stream. Slots 0 and 1 are already in the queue;
    // later words arrive as it advances, so they are recorded as each fetch
    // happens rather than collected up front — collecting would emit the reads
    // at the wrong point in the sequence.
    let mut words = [0u16; 6];
    words[0] = cpu.prefetch[0];
    words[1] = cpu.prefetch[1];
    let mut n_have = 2usize;

    // Bus accesses committed so far, and idle cycles incurred so far. Both are
    // needed by the address-error paths, which charge only what ran before the
    // fault plus a fixed tail.
    let mut acc = 0u32;
    macro_rules! fetch {
        ($n:expr) => {
            for _ in 0..$n {
                cpu.consume_opcode_dyn(bus);
                if n_have < words.len() {
                    words[n_have] = cpu.prefetch[1];
                    n_have += 1;
                }
                acc += 1;
            }
        };
    }

    // Resolving `-(An)`, `(d8,An,Xn)` or `(d8,PC,Xn)` costs a 2-cycle internal
    // step before the address is ready. A memory-pair `ADDX`/`SUBX` picks this
    // up automatically through its translated `-(Ax)` destination mode, and
    // `CMPM`'s `(Ax)+` correctly does not.
    let idle_lead = if plan.mode == 4 || plan.mode == 6 || (plan.mode == 7 && plan.reg == 3) {
        2
    } else {
        0
    };

    // --- The xxxI immediate. -----------------------------------------------
    fetch!(plan.pre);
    let imm = match plan.pre {
        0 => 0,
        1 => words[1] as u32 & size.mask(),
        _ => ((words[1] as u32) << 16) | words[2] as u32,
    };

    // --- Resolve and read, or fault. ---------------------------------------
    // Every address error in Task 6 is a *read* fault (29,896/29,896): a
    // read-modify-write reads its destination before writing it, so a misaligned
    // destination faults on the read and the write is never reached. The CCR is
    // untouched in all of them, so `compute` must not have run yet.
    //
    // The 29,896 denominator is the **32 sized data-alu groups**: the twelve
    // binary forms, the ten unary (`CLR`/`NEG`/`NEGX`/`NOT`/`TST`), and the ten
    // `ADDA`/`CMPA`/`SUBA`/`ADDX`/`SUBX`. Re-measured in Task 14 and it
    // reproduces exactly. ⚠️ Naming the set matters more than it looks: an
    // *approximation* of "Task 6's groups" scores 21,563 on the same query,
    // which reads exactly like a stale figure and is a wrong denominator. Check
    // the group list before concluding this number has drifted.
    //
    // The control for the zero is in `68000-notes.md`: write faults occur
    // 2,446 times, in `MOVE.w/.l` and `MOVEM.w/.l` and no other group, so the
    // query can see one. The zero is a property of the instruction class.
    macro_rules! fault {
        ($addr:expr, $mode:expr, $reg:expr, $faulted_src:expr) => {{
            adjust_on_fault(cpu, plan, size, $faulted_src);
            exception::address_error(
                cpu,
                bus,
                $addr,
                FaultKind::Read,
                operand_space($mode, $reg),
                ir,
                stacked_pc(opcode_addr, plan, size),
            );
            // The lead — `acc` accesses and `idle_lead` — is in this term
            // because it really reached the bus; only the tail collapses on a
            // halt. Measured on this core: `ADD.w D0,(A0)` with an odd A0 and an
            // odd SSP halts with an **empty** bus log, where the bare 58 was a
            // 58-cycle claim about 0 accesses. See `exception::entry_cycles`.
            return 4 * acc
                + idle_lead
                + exception::entry_cycles(cpu, 0, ADDRESS_ERROR_TAIL_CYCLES);
        }};
    }

    // A pair form's source resolves before the destination exists, so a source
    // fault must leave the destination register *entirely* untouched — not
    // adjusted and rolled back. Resolving it lazily here is what guarantees
    // that, and it is measured: 855 `ADDX.l` source faults show a destination
    // register still holding its initial value.
    let mut src_val = 0;
    if let Some((sm, sr)) = plan.pair {
        let src_ea = ea::resolve(cpu, sm, sr, size, &[], 0);
        let Ea::Mem(sa) = src_ea else {
            unreachable!("a pair form's source is always a memory operand")
        };
        if misaligned(sa, size) {
            fault!(first_access(sa, size, plan.desc_reads), sm, sr, true);
        }
        src_val = read_operand(cpu, bus, src_ea, size, plan.desc_reads);
        acc += wper;
    }

    fetch!(de);
    let ext_at = 1 + plan.pre as usize;
    let dst_ea = ea::resolve(
        cpu,
        plan.mode,
        plan.reg,
        size,
        &words[ext_at..ext_at + de as usize],
        opcode_addr.wrapping_add(2).wrapping_add(2 * plan.pre),
    );
    if let Ea::Mem(addr) = dst_ea {
        if misaligned(addr, size) {
            fault!(
                first_access(addr, size, plan.desc_reads),
                plan.mode,
                plan.reg,
                false
            );
        }
    }

    let ea_val = read_operand(cpu, bus, dst_ea, size, plan.desc_reads);
    let nr = if mode_is_mem(plan.mode, plan.reg) {
        wper
    } else {
        0
    };
    acc += nr;

    // --- Compute, advance the queue, write back. ---------------------------
    let result = compute(
        cpu,
        bus,
        Ops {
            ea: ea_val,
            src: src_val,
            imm,
            trap_pc: opcode_addr
                .wrapping_add(2)
                .wrapping_add(2 * plan.pre)
                .wrapping_add(2 * de),
        },
    );

    // A trap has already run the frame on the bus and left the queue alone. Its
    // accesses are counted, not re-emitted.
    let result = match result {
        Tail::Trapped => {
            // A halted entry writes no frame and fetches no vector, so
            // `SHORT_FRAME_ACCESSES` is not owed; `acc` and the idle are, having
            // already happened. Measured: `CHK D0,D1` trapping with an odd SSP
            // halts with an empty bus log, where the unconditional form claimed
            // 40 cycles for it.
            return 4 * acc
                + idle_lead
                + plan.idle
                + exception::entry_cycles(cpu, 0, 4 * exception::SHORT_FRAME_ACCESSES);
        }
        Tail::Write(v) => Some(v),
        Tail::Done => None,
    };

    // `Plan::writes` records the handler's *intent* to write back; whether a write
    // actually happens is decided by `compute` returning `Some`. Those must agree,
    // and until this assertion existed nothing checked that they did — the field
    // had 13 builder call sites and 0 reads, so it read as though it gated the
    // write while being inert. Asserting is better than deleting: the intent is
    // worth recording, and `Plan` demonstrably carries both live flags
    // (`fetch_last`) and formerly-inert ones with identical shape, which a reader
    // cannot tell apart without grepping.
    // `Plan::writes` records that the handler intends the result to go back to
    // the `<ea>`, and until this assertion existed nothing checked it: the field
    // had 13 builder call sites and **0** reads, so it read as though it gated
    // the write while being inert. The write is actually decided by `compute`
    // returning `Some`.
    //
    // ⚠️ The invariant is an **implication, not an equality**, and the difference
    // is measured. Over the full suite, `writes == result.is_some()` fails
    // 82,380 times (`writes` false while a write happens — the register
    // destinations), and `writes == (result.is_some() && dst is memory)` fails
    // 7,918 times in the *opposite* direction (`BSET`/`BCLR`/`BCHG` on a
    // register set `writes` legitimately). Neither equality is the rule. What
    // never occurs is `writes` set while nothing is written — 0 cases — so that
    // is what is asserted, and asserting either equality would be a false claim
    // that happens to be shaped like a check.
    debug_assert!(
        !plan.writes || result.is_some(),
        "Plan::writes was set but compute returned None: an intended write-back never happened"
    );

    let mut nw = 0;
    match (result, dst_ea) {
        (Some(val), Ea::Mem(addr)) if size == Size::Long && plan.pair.is_some() => {
            // ADDX.l/SUBX.l into -(Ax) splits its descending write around the
            // queue advance: low word, fetch, high word.
            let a = addr & ADDR_MASK;
            bus.write16(a.wrapping_add(2) & ADDR_MASK, val as u16);
            fetch!(1);
            bus.write16(a, (val >> 16) as u16);
            nw = 2;
        }
        (Some(val), Ea::Mem(addr)) => {
            if !plan.fetch_last {
                fetch!(1);
            }
            if size == Size::Long {
                // Descending, in every mode — see the module docs.
                ea::write_predec_long(bus, addr, val);
            } else {
                ea::write(cpu, bus, dst_ea, size, val);
            }
            if plan.fetch_last {
                fetch!(1);
            }
            nw = wper;
        }
        (Some(val), _) => {
            fetch!(1);
            ea::write(cpu, bus, dst_ea, size, val);
        }
        (None, _) => fetch!(1),
    }
    acc += nw;

    debug_assert_eq!(
        acc,
        n_words + nr + nw + if plan.pair.is_some() { wper } else { 0 },
        "the schedule's access count must match its own plan"
    );
    4 * acc + idle_lead + plan.idle
}

/// Fixes up the `(An)+` / `-(An)` adjustment that a faulting instruction does
/// not keep.
///
/// [`ea::resolve`] commits every adjustment as it forms the address, which is
/// what the vectors show for most buckets. The exceptions are tabled here as
/// measured constants — none has a derivation, and each rests on the case count
/// shown (A7 excluded throughout, since the exception frame's own push would
/// otherwise masquerade as an operand adjustment):
///
/// ```text
///                                  source reg        destination reg
/// single <ea>  (An)+ .w             — (kept, +step)   n/a       893+449+405+…
/// single <ea>  (An)+ .l             — (rolled back)   n/a       887+430+418+…
/// single <ea>  -(An) .w/.l          — (kept, -step)   n/a       910+874+450+…
/// ADDX/SUBX    src fault .w         kept  (-step)     untouched         846
/// ADDX/SUBX    src fault .l         rolled back       untouched         855
/// ADDX/SUBX    dst fault .w         kept  (-step)     kept (-step)      410
/// ADDX/SUBX    dst fault .l         kept  (-step)     rolled back       396
/// CMPM         src fault .w         +2                untouched          95
/// CMPM         src fault .l         +2  (not +4)      untouched          91
/// CMPM         dst fault .w/.l      kept  (+step)     rolled back    37+39
/// ```
///
/// Two rows deserve naming because they look like mistakes and are not:
///
/// - **`CMPM.l` commits only `+2` on a source fault**, not the full `+4`. The
///   postincrement appears to advance per word accessed, and the first word is
///   the one that faults. `CMPM.w` also shows `+2`, which is its full step, so
///   the two sizes agree on the *number* and disagree on whether it is complete.
/// - **`ADDX`/`SUBX` roll back on a long fault but keep on a word fault**, which
///   is the reverse of the intuition that a wider access has "got further".
///
/// Where a fault leaves a register *untouched* rather than adjusted-then-rolled-
/// back, that is not implemented here at all: [`run`] resolves the destination
/// lazily, so on a source fault there is nothing to undo.
fn adjust_on_fault(cpu: &mut M68k, plan: &Plan, size: Size, faulted_src: bool) {
    match plan.pair {
        Some((src_mode, src_reg)) if faulted_src => {
            let r = src_reg as usize;
            match src_mode {
                // -(Ay): a long rolls the decrement back, a word keeps it.
                4 if size == Size::Long => cpu.a[r] = cpu.a[r].wrapping_add(size.step(r)),
                // (Ay)+: lands on initial + 2 whatever the size.
                3 => cpu.a[r] = cpu.a[r].wrapping_sub(size.step(r)).wrapping_add(2),
                _ => {}
            }
        }
        Some(_) => {
            // The source read completed, so it keeps its full adjustment; only
            // the destination is fixed up.
            let r = plan.reg as usize;
            match plan.mode {
                4 if size == Size::Long => cpu.a[r] = cpu.a[r].wrapping_add(size.step(r)),
                3 => cpu.a[r] = cpu.a[r].wrapping_sub(size.step(r)),
                _ => {}
            }
        }
        None => {
            if plan.mode == 3 && size == Size::Long {
                let r = plan.reg as usize;
                cpu.a[r] = cpu.a[r].wrapping_sub(size.step(r));
            }
        }
    }
}

/// The PC to stack in an address-error frame.
///
/// Every Task 6 fault is a read fault, so only the read arm of the general rule
/// is reachable; the write arm is deliberately absent rather than written
/// untested. Three terms, each of which cost a failing bucket to find:
///
/// - `2 * pre` — the `xxxI` immediate precedes the `<ea>`'s extension words, so
///   it shifts the stacked PC. Without it the `xxxI` faults score 0/746.
/// - the memory-pair forms stack a **constant** `opcode + 4`, not split by size
///   the way `-(An)` is. Applying the `-(An)` rule to them left `.l` at 0/1,656.
/// - `-(An)` splits on size (`.w` +2, `.l` +0) and the two absolute modes add
///   their extension words. No other mode adds anything, despite `d16(An)`,
///   `(d8,An,Xn)`, `d16(PC)` and `(d8,PC,Xn)` all consuming an extension word.
fn stacked_pc(opcode_addr: u32, plan: &Plan, size: Size) -> u32 {
    if plan.pair.is_some() {
        return opcode_addr.wrapping_add(4);
    }
    let bump = match (plan.mode, plan.reg) {
        (4, _) => {
            if size == Size::Long {
                0
            } else {
                2
            }
        }
        (7, 0) => 2, // abs.W
        (7, 1) => 4, // abs.L
        _ => 0,
    };
    opcode_addr
        .wrapping_add(2)
        .wrapping_add(2 * plan.pre)
        .wrapping_add(bump)
}
