//! Exception entry: vectors, stack frames, and interrupt dispatch.

use crate::cpu::{M68k, ADDR_MASK, SR_S, SR_T};
use crate::Bus;

pub const VEC_BUS_ERROR: u8 = 2;
pub const VEC_ADDRESS_ERROR: u8 = 3;
pub const VEC_ILLEGAL: u8 = 4;
pub const VEC_DIVIDE_BY_ZERO: u8 = 5;
pub const VEC_CHK: u8 = 6;
pub const VEC_TRAPV: u8 = 7;
pub const VEC_PRIVILEGE: u8 = 8;
pub const VEC_TRACE: u8 = 9;
pub const VEC_LINE_A: u8 = 10;
pub const VEC_LINE_F: u8 = 11;
/// TRAP #n uses vectors 32-47.
pub const VEC_TRAP_BASE: u8 = 32;
/// Autovectored interrupts use vectors 25-31 for levels 1-7.
pub const VEC_AUTOVECTOR_BASE: u8 = 24;

/// Pushes a word onto the active stack.
// Used by future instruction handlers (TRAP, bus-error frame, etc.).
#[allow(dead_code)]
pub(crate) fn push16(cpu: &mut M68k, bus: &mut dyn Bus, val: u16) {
    cpu.a[7] = cpu.a[7].wrapping_sub(2);
    bus.write16(cpu.a[7] & ADDR_MASK, val);
    if cpu.sr_s() {
        cpu.ssp = cpu.a[7];
    } else {
        cpu.usp = cpu.a[7];
    }
}

/// Where the next exception frame will be written.
///
/// **Always the supervisor stack, even from user mode.** Entry sets S *before*
/// pushing, so the frame lands on the SSP: measured 3,160/3,160 user-mode cases
/// across TRAP, TRAPV and RTE, with the USP form as a control at 0/3,160 and the
/// USP itself untouched. Reading the *active* SP instead scores ~50% — the
/// coin-flip signature of a predicate that is itself half true.
#[inline]
fn frame_base(cpu: &M68k) -> u32 {
    if cpu.sr_s() {
        cpu.a[7]
    } else {
        cpu.ssp
    }
}

/// The double bus fault: halts the CPU instead of entering an exception.
///
/// Called first by both [`take`] and [`address_error`], **before either commits
/// anything** — no SR change, no push, no vector fetch. Returns `true` if the
/// CPU halted, in which case the caller must not proceed.
///
/// Two ways in, and the second is the one that matters in practice:
///
/// 1. A fault raised while a frame is already being written (`in_exception`).
///    Structurally impossible today, since [`Bus`] is infallible and nothing
///    inside either frame writer can fault, but this is the guard that keeps a
///    future faulting bus from recursing until the *host* stack overflows.
/// 2. **An odd frame base.** Every stack offset in this architecture is even
///    (`±2`, `±4`, `±6`), so `parity(sp ± k) == parity(sp)`: if a stack access is
///    misaligned then the stack pointer is odd, and the frame this function
///    guards would fault on its own first push. That is the definition of a
///    double bus fault, and hardware halts rather than writing a frame.
///
/// This is why the handlers' own alignment checks (`link`, `pea`, `unlk`,
/// `movem`, `move_to_sr_ccr`, RTE's pops) stay exactly as they are and still
/// call [`address_error`]: they own the *address*, this function owns the
/// *consequence*. A frame-pushing operand fault with an even SSP is unaffected —
/// all 55,606 measured address errors are of that kind.
///
/// # Extrapolated: zero suite coverage
///
/// No case reaches this. `initial A7 odd: 0/317,500`, `frame base odd at fault
/// time: 0/55,606`, `odd SSP while in user mode: 0`, `bus-error (vector 2)
/// fetches: 0`. The control for "never odd" is the *stacked* fault address,
/// which is odd in 55,606/55,606 — so the query can see odd values in a frame
/// and the zero is a fact about the base. The reasoning above is arithmetic, not
/// measurement, which is why no suite result could confirm or refute it.
///
/// ⚠️ One place where the arithmetic does **not** reach: a user-mode push through
/// an odd USP. The frame goes to the SSP (see [`frame_base`]), which may well be
/// even, so that fault writes an ordinary vector-3 frame rather than halting.
/// Checking the frame base — rather than assuming every misaligned stack access
/// halts — is what gets both cases right without a case analysis. Also zero
/// coverage, and the supplement's theorem states the universal form; see the
/// task report.
fn double_bus_fault(cpu: &mut M68k) -> bool {
    if cpu.in_exception || frame_base(cpu) & 1 != 0 {
        cpu.halted = true;
        return true;
    }
    false
}

/// Takes a group 1/2 exception: save SR, enter supervisor mode, write the
/// short frame, then vector.
///
/// The 68000 writes the short frame in a non-sequential bus order that does
/// not match three simple push16 operations.  From the Users Manual and
/// confirmed by the SingleStepTests bus-transaction sequence:
///
///   1.  `PC[15:0]`  → old_SSP − 2   (highest address in the frame)
///   2.  SR        → old_SSP − 6   (lowest address; new SSP lands here)
///   3.  `PC[31:16]` → old_SSP − 4   (middle of the frame)
///
/// The result in memory (growing downward) is a canonical big-endian long PC
/// followed by the SR word, with new_SSP pointing at the SR.
///
/// `pc_for_frame` is the PC value to stack, which differs per exception type,
/// so callers pass it explicitly rather than this function guessing from
/// `cpu.pc`.
///
/// # The stacked SR is the SR *now*, not at instruction entry
///
/// `old_sr` below is read when the exception is taken, so an instruction that
/// updates flags and *then* traps stacks the **updated** flags. This is not
/// incidental — measured suite-wide over all 76,957 vector-taking cases, the
/// frame's low 5 bits match the post-instruction CCR in 76,369, against 72,848
/// for an instruction-entry snapshot. The 3,521-case gap is exactly the
/// flags-then-trap instructions: `CHK` 1,240, `RTR` 1,180, `MOVE.w` 604,
/// `MOVE.l` 471.
///
/// So a caller that traps must set the CCR **before** calling this, which is what
/// [`crate::ops::muldiv`]'s `chk` does. Do not add an entry-time SR parameter to
/// "fix" this: getting it wrong shows up as a diff confined to the low 5 bits of
/// one stacked word, which reads as a flag bug in the compare rather than a frame
/// bug.
///
/// Entry sets S and clears T, and leaves the interrupt mask **unchanged** —
/// `SR_MASK` keeps bits 10-8, so do not raise the IPL here (38,542/38,542 on each
/// of the three).
pub fn take(cpu: &mut M68k, bus: &mut dyn Bus, vector: u8, pc_for_frame: u32) {
    if double_bus_fault(cpu) {
        return;
    }
    cpu.in_exception = true;
    let old_sr = cpu.sr;
    // Enter supervisor mode and clear trace before touching the stack, so the
    // frame lands on the supervisor stack and the handler does not trace.
    cpu.set_sr((old_sr | SR_S) & !SR_T);

    // Hardware bus sequence: PC_low first, then SR, then PC_high.
    // The stack pointer ends up 6 bytes below where it started.
    let old_sp = cpu.a[7];
    bus.write16(
        (old_sp.wrapping_sub(2)) & ADDR_MASK,
        (pc_for_frame & 0xFFFF) as u16,
    );
    bus.write16((old_sp.wrapping_sub(6)) & ADDR_MASK, old_sr);
    bus.write16(
        (old_sp.wrapping_sub(4)) & ADDR_MASK,
        (pc_for_frame >> 16) as u16,
    );
    cpu.a[7] = old_sp.wrapping_sub(6);
    // Keep ssp in sync (we are always in supervisor mode at this point).
    cpu.ssp = cpu.a[7];

    let addr = (vector as u32) * 4;
    let hi = bus.read16(addr & ADDR_MASK) as u32;
    let lo = bus.read16((addr + 2) & ADDR_MASK) as u32;
    cpu.pc = (hi << 16) | lo;
    cpu.refill_prefetch_dyn(bus);
    cpu.in_exception = false;
}

/// Bus accesses [`take`] performs: 3 frame writes, 2 vector reads, 2 prefetch
/// refills.
///
/// Under the timing law (`cycles = 4 × non-idle accesses + idle`) a handler that
/// ends in `take` owes `4 * SHORT_FRAME_ACCESSES` on top of whatever ran before
/// it. Measured on `CHK`: `cycles - idle` buckets at 28 / 32 / 36 / 40 across the
/// addressing modes (305 / 590 / 408 / 23 cases), and 28 is `4 * 7` for the
/// register form, which performs no other access at all.
///
/// This is deliberately *not* the group-1/2 equivalent of
/// [`ADDRESS_ERROR_TAIL_CYCLES`]: that constant folds in a fixed 10 cycles of
/// idle and the aborted access, whereas a group-2 trap's idle is data-dependent
/// (`CHK`'s is 6, 10 or 12) and belongs to the instruction, not the frame.
pub const SHORT_FRAME_ACCESSES: u32 = 7;

/// Whether a faulting access was a read or a write. Selects bit 4 of the
/// address-error status word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FaultKind {
    Read,
    Write,
}

/// Address space the faulting access was in. Selects bits 0-1 of `fc`.
///
/// Program space means the access was an instruction fetch or a PC-relative
/// operand read; everything else is data space. In the MOVE groups all 177
/// program-space faults are PC-relative source reads (addendum §9.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Space {
    Data,
    Program,
}

/// Fixed cycle cost of the address-error tail: the aborted access, the 7 frame
/// writes, the 2 vector reads, the 2 prefetch refills, and 10 cycles of idle.
///
/// Under the timing law (`cycles = 4 * non-idle accesses + idle`) that is
/// `4 * 12 + 10`. Measured at 58 in 4,661 of 4,661 cases (addendum §9.6). The
/// caller adds the cost of whatever schedule ran before the fault.
pub const ADDRESS_ERROR_TAIL_CYCLES: u32 = 58;

/// Takes an address error: vector 3 with a 7-word frame.
///
/// Raised when a **word or long** access is attempted at an odd address. Byte
/// accesses never raise it, and neither does `MOVEP`, which is byte-sized on the
/// bus whatever its suffix says.
///
/// **The faulting access must not have happened.** Callers check alignment
/// *before* touching the bus — the test harness asserts the access is absent
/// from the bus log, so a read-then-fault would fail even though the CPU state
/// came out right.
///
/// Arguments the caller must get right, each measured rather than guessed:
///
/// - `fault_addr` — the full **32-bit** un-masked address, odd bit included.
///   Do not mask to 24 bits: the frame stores both halves and the top byte is
///   frequently non-zero.
/// - `ir` — the instruction register at fault time. This is the opcode in every
///   case except a word-sized write fault into `-(An)`, where the pipeline has
///   advanced one word further (addendum §9.5c).
/// - `pc_for_frame` — **not** a fixed offset from the opcode, and not
///   `cpu.pc` at fault time either. It depends on the fault direction, the
///   addressing mode and the operand size; `ops::move_::stacked_pc` derives it
///   for the MOVE family and addendum §8 gives the general formula.
///
/// Bus write order, continuing the short frame's "low half, then skip back,
/// then fill in" pattern, verified 6,579/6,579:
///
/// | order | address     | contents      |
/// |-------|-------------|---------------|
/// | 1     | `base - 2`  | `PC[15:0]`    |
/// | 2     | `base - 6`  | SR            |
/// | 3     | `base - 4`  | `PC[31:16]`   |
/// | 4     | `base - 8`  | IR            |
/// | 5     | `base - 10` | `fault[15:0]` |
/// | 6     | `base - 14` | status word   |
/// | 7     | `base - 12` | `fault[31:16]`|
///
/// `base` is `cpu.a[7]` **as it stands now** — not the instruction's entry SP.
/// A faulting `-(A7)` or `(A7)+` has already moved it, and the frame lands
/// below the moved value in 31 of 4,661 cases (addendum §9.5b).
pub fn address_error(
    cpu: &mut M68k,
    bus: &mut dyn Bus,
    fault_addr: u32,
    kind: FaultKind,
    space: Space,
    ir: u16,
    pc_for_frame: u32,
) {
    if double_bus_fault(cpu) {
        return;
    }
    cpu.in_exception = true;
    // fc uses the S bit as it was BEFORE entry, and the space of the faulting
    // access. Bits 0-2 of the SR are C/V/Z, not a function code — compute this
    // from the access, never by reading the SR.
    let fc = (if cpu.sr_s() { 4u16 } else { 0 })
        | match space {
            Space::Program => 2,
            Space::Data => 1,
        };
    // The upper 11 bits are stale IR bits sharing the latch. That is real
    // hardware behaviour; do not clean them. Bit 3 is always 0 — there is no
    // "instruction/not" bit to set.
    let status = (ir & 0xFFE0)
        | match kind {
            FaultKind::Read => 1 << 4,
            FaultKind::Write => 0,
        }
        | fc;

    // The stacked SR is the SR at fault time, including any CCR update the
    // instruction already performed — not a pre-instruction snapshot
    // (addendum §9.5a). Capture it before entering supervisor mode.
    let old_sr = cpu.sr;
    cpu.set_sr((old_sr | SR_S) & !SR_T);

    let base = cpu.a[7];
    let w = |bus: &mut dyn Bus, off: u32, val: u16| {
        bus.write16(base.wrapping_sub(off) & ADDR_MASK, val);
    };
    w(bus, 2, pc_for_frame as u16);
    w(bus, 6, old_sr);
    w(bus, 4, (pc_for_frame >> 16) as u16);
    w(bus, 8, ir);
    w(bus, 10, fault_addr as u16);
    w(bus, 14, status);
    w(bus, 12, (fault_addr >> 16) as u16);

    cpu.a[7] = base.wrapping_sub(14);
    cpu.ssp = cpu.a[7];

    let vaddr = (VEC_ADDRESS_ERROR as u32) * 4;
    let hi = bus.read16(vaddr & ADDR_MASK) as u32;
    let lo = bus.read16((vaddr + 2) & ADDR_MASK) as u32;
    cpu.pc = (hi << 16) | lo;
    cpu.refill_prefetch_dyn(bus);
    cpu.in_exception = false;
}

/// Cycles an autovectored interrupt costs.
///
/// # Extrapolated: zero suite coverage
///
/// Interrupts (vectors 24-31) are fetched **0** times in all 317,500 cases, so no
/// part of this number is measured. What *is* measured is the decomposition
/// method: the manual writes exception costs as `total(reads/writes)`, and that
/// notation reproduces the vectors exactly on every path that does have coverage
/// — `(4/3)` idle 6 for TRAP, LINE-A, LINE-F and the privilege violations, and
/// `(4/7)` idle 10 for the 55,606 address errors, both split confirmed
/// access-by-access suite-wide across 10,287 cases and both frame sizes.
///
/// The manual's interrupt row is `44(5/3)`, which under the timing law
/// (`4 × accesses + idle`) decomposes as the same seven accesses TRAP performs —
/// 3 frame writes, 2 vector reads, 2 prefetch refills — **plus one more read,
/// the interrupt-acknowledge cycle** — and `44 − 4 × 8 = 12` idle. The IACK
/// access and the idle split are the two parts that remain genuinely
/// unverifiable here.
///
/// ⚠️ Do not read TRAPV's `(5/3)` idle 2 as licence to infer this split from the
/// total: two different decompositions reach 34 on measured paths, so a total
/// never determines a split on this core.
pub const INTERRUPT_CYCLES: u32 = 4 * (SHORT_FRAME_ACCESSES + 1) + 12;

/// Takes a pending interrupt if one outranks the SR's mask, returning its cost.
///
/// Called at the instruction boundary in [`crate::cpu::M68k::step_with`], which
/// is where hardware samples IPL — not inside any handler.
///
/// The mask rule is the standard one: level 7 is non-maskable, and levels 1-6
/// are taken only when strictly above `(sr >> 8) & 7`. The vector is
/// [`VEC_AUTOVECTOR_BASE`] `+ level`; a vectored-interrupt controller supplying
/// its own vector number is not modelled, because nothing in this project has one
/// (CPS-1's 68000 is autovectored).
///
/// Entry raises the mask to the interrupt's own level, which is the one place a
/// vector-taking path *does* touch bits 10-8 — [`take`] deliberately leaves them
/// alone (38,542/38,542), so the raise happens here, after the frame is stacked
/// with the old mask.
///
/// # Extrapolated: zero suite coverage
///
/// See [`INTERRUPT_CYCLES`]. Every claim in this function is unmeasured; the unit
/// tests in this module are its only coverage, and a green suite says nothing
/// about it.
pub fn check_interrupts(cpu: &mut M68k, bus: &mut dyn Bus) -> Option<u32> {
    let level = cpu.pending_irq & 7;
    if level == 0 {
        return None;
    }
    let mask = (cpu.sr >> 8) & 7;
    if level < 7 && u16::from(level) <= mask {
        return None;
    }

    // Resuming from STOP: the stopped state froze the PC and both queue words
    // with the STOP opcode still in slot 0 (measured 1230/1230), so the queue
    // must be re-primed here or dispatch would re-run the STOP forever. `pc`
    // already points at the instruction *after* STOP and `refill_prefetch_dyn`
    // advances it by 4 itself, so this is a bare call: no arithmetic around it.
    // Rewinding first resumes into the STOP again; adding `pc += 4` after skips
    // an instruction.
    //
    // The stacked PC is therefore `pc - 4` on both paths — mid-stream the queue
    // holds the *next* instruction's two words, and after the refill it holds
    // the resumed instruction's. Two accesses (8 cycles) that no vector
    // confirms hardware pays on the resume path; the cost returned below does
    // not include them, which is itself extrapolated.
    if cpu.stopped {
        cpu.stopped = false;
        cpu.refill_prefetch_dyn(bus);
    }

    let pc = cpu.pc.wrapping_sub(OPCODE_PC_OFFSET);
    take(cpu, bus, VEC_AUTOVECTOR_BASE + level, pc);
    // Mask the level being serviced, after the old SR is safely on the stack.
    cpu.set_sr((cpu.sr & !0x0700) | (u16::from(level) << 8));
    Some(INTERRUPT_CYCLES)
}

/// Takes the trace exception if the T bit is set, returning its cost.
///
/// Called at the instruction boundary, **after** the instruction completes —
/// that is what the T bit means, and it is why the vector suite never sees one:
/// each case runs exactly one instruction and stops at the boundary this check
/// lives on. `TRACE` (vector 9) is fetched **0** times in all 317,500 cases,
/// against a control of 2,500 TRAP fetches recovered by the same code, while
/// 158,894 cases *enter* with T=1 across all 127 groups. So an implementation
/// that traces inside a handler, or before the boundary, fails 38% of the suite.
///
/// The T bit read here is the SR the instruction *left behind*: an instruction
/// that loads the whole SR changes its own trace state, which is the operand's
/// doing rather than the CPU's (`ANDItoSR`, `EORItoSR`, `STOP`, `MOVEtoSR` and
/// `RTE` account for all 1,277 clean cases that end with T cleared, with
/// `ORItoSR` at 0/591 as the control — `OR` cannot clear a bit).
///
/// # Extrapolated: zero suite coverage
///
/// The cost is the group-2 short frame, i.e. the same `34` every other group-2
/// path measures. Nothing verifies that for vector 9 specifically.
pub fn check_trace(cpu: &mut M68k, bus: &mut dyn Bus) -> Option<u32> {
    if cpu.sr & SR_T == 0 {
        return None;
    }
    let pc = cpu.pc.wrapping_sub(OPCODE_PC_OFFSET);
    take(cpu, bus, VEC_TRACE, pc);
    Some(4 * SHORT_FRAME_ACCESSES + 6)
}

/// Byte distance from `cpu.pc` at handler entry back to the opcode word.
///
/// When `step_with` dispatches a handler, `cpu.pc` is 4 bytes ahead of the
/// opcode being executed:
/// - 4 bytes: the prefetch queue holds two words beyond the instruction;
///   `step_with` peeks at `prefetch[0]` without shifting the queue or
///   advancing `pc`.
///
/// **This offset is only valid for a handler that has consumed exactly the
/// opcode word and no extension words.** Any handler that calls `fetch_word`
/// or `fetch_long` to consume extension words will have advanced `pc` further;
/// those handlers must capture `cpu.pc` at entry (before any such fetch) and
/// pass their own adjusted value to `take`.
pub(crate) const OPCODE_PC_OFFSET: u32 = 4;

/// An unrecognised opcode. Line-A and Line-F opcodes have their own vectors,
/// which some software uses deliberately as a trap mechanism.
pub fn illegal_instruction(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let vector = match op >> 12 {
        0xA => VEC_LINE_A,
        0xF => VEC_LINE_F,
        _ => VEC_ILLEGAL,
    };
    // Stack the address of the offending instruction. See OPCODE_PC_OFFSET for
    // the rationale; do not copy this calculation into other handlers.
    let pc = cpu.pc.wrapping_sub(OPCODE_PC_OFFSET);
    take(cpu, bus, vector, pc);
    34
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::{FlatBus, RecordingBus};
    use crate::decode::Decoder;

    /// A Line-A opcode must vector through 10, stacking the address of the
    /// offending instruction — which the prefetch queue puts at `pc - 4`.
    #[test]
    fn line_a_opcode_takes_vector_10_with_a_short_frame() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xA000, 0x4E71]);
        bus.put16(0x0028, 0x0000); // vector 10 = address 40
        bus.put16(0x002A, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]); // the handler

        let mut cpu = M68k::new();
        cpu.sr = SR_S | 0x0700; // supervisor, all interrupts masked
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        assert_eq!(cpu.pc, 0x1004);

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        // Short frame: SR (word) at new_SSP, PC_high above that, PC_low at top.
        // SP drops by 6 total.
        assert_eq!(cpu.a[7], 0x2FFA);
        // Memory layout (low addr → high addr): [SR][PC_hi][PC_lo]
        //   0x2FFA: SR,      0x2FFC: PC[31:16] = 0x0000,  0x2FFE: PC[15:0] = 0x1000
        assert_eq!(
            bus.read16(0x2FFE),
            0x1000,
            "stacked PC[15:0] is the bad opcode"
        );
        assert_eq!(bus.read16(0x2FFC), 0x0000, "stacked PC[31:16]");
        assert_eq!(
            bus.read16(0x2FFA),
            SR_S | 0x0700,
            "stacked SR is the old SR"
        );

        assert_eq!(cpu.pc, 0x2004, "vectored, then refilled the queue");
        assert_eq!(cpu.prefetch, [0x4E71, 0x4E71]);
        assert!(cpu.sr_s());
    }

    /// A Line-F opcode uses vector 11, and a plain illegal opcode uses 4.
    #[test]
    fn line_f_and_plain_illegal_use_their_own_vectors() {
        for (opcode, vector) in [(0xF000u16, VEC_LINE_F), (0x4AFC, VEC_ILLEGAL)] {
            let mut bus = FlatBus::new();
            bus.load(0x1000, &[opcode, 0x4E71]);
            let vaddr = (vector as u32) * 4;
            bus.put16(vaddr, 0x0000);
            bus.put16(vaddr + 2, 0x2000);
            bus.load(0x2000, &[0x4E71, 0x4E71]);

            let mut cpu = M68k::new();
            cpu.sr = SR_S;
            cpu.a[7] = 0x3000;
            cpu.pc = 0x1000;
            cpu.prime_prefetch(&mut bus);

            let dec = Decoder::new();
            cpu.step_with(&dec, &mut bus);

            assert_eq!(
                cpu.pc, 0x2004,
                "opcode {opcode:04X} must use vector {vector}"
            );
        }
    }

    /// Entering an exception clears the trace bit, or the handler would trace.
    #[test]
    fn exception_entry_clears_trace() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xA000, 0x4E71]);
        bus.put16(0x002A, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | SR_T;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.sr & SR_T, 0);
    }

    /// The hardware writes the short frame in a non-sequential order:
    ///   1. PC[15:0]  at old_SSP − 2
    ///   2. SR        at old_SSP − 6
    ///   3. PC[31:16] at old_SSP − 4
    ///
    /// Asserting final memory contents cannot catch a revert to sequential
    /// push16 order (which produces identical contents with different bus
    /// timing).  This test records every write in order and asserts the
    /// sequence, so a regression is immediately visible.
    #[test]
    fn short_frame_write_order_is_pc_low_sr_pc_high() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0xA000, 0x4E71]);
        bus.put16(0x0028, 0x0000); // vector 10 → 0x2000
        bus.put16(0x002A, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | 0x0700;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        // prime_prefetch issues reads; clear the log before stepping so we
        // only see the instruction's bus activity.
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        let writes = bus.writes();
        // The first three writes are the frame; the vector reads follow.
        assert!(writes.len() >= 3, "expected at least 3 frame writes");
        // Write 0: PC[15:0] at SP−2 = 0x2FFE
        assert_eq!(
            writes[0],
            (0x2FFE, 0x1000),
            "write 0 must be PC[15:0] at SP-2"
        );
        // Write 1: SR at SP−6 = 0x2FFA
        assert_eq!(
            writes[1],
            (0x2FFA, SR_S | 0x0700),
            "write 1 must be SR at SP-6"
        );
        // Write 2: PC[31:16] at SP−4 = 0x2FFC
        assert_eq!(
            writes[2],
            (0x2FFC, 0x0000),
            "write 2 must be PC[31:16] at SP-4"
        );
    }

    /// An interrupt above the mask vectors through `24 + level` and stacks the
    /// PC of the instruction it interrupted.
    ///
    /// # Extrapolated
    ///
    /// Interrupt vectors 24-31 are fetched **0** times in all 317,500 suite
    /// cases, so nothing about this path is measured — including the 44 cycles.
    /// The literals below are extrapolated from the manual's `44(5/3)` row,
    /// whose read/write notation *is* verified against the vectors on every
    /// exception path that has coverage (10,287 cases, both frame sizes). The
    /// stacked-PC rule is the measured one for a mid-stream boundary: PC leads
    /// the queue by 4 (1230/1230 in STOP, 1233/1233 in RESET).
    #[test]
    fn an_interrupt_above_the_mask_vectors_and_stacks_the_next_instruction() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E71, 0x4E71, 0x4E71]);
        bus.put16(0x0074, 0x0000); // vector 29 = level 5, address 0x74
        bus.put16(0x0076, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | 0x0200; // mask level 2
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        assert_eq!(cpu.pc, 0x1004);
        cpu.set_irq(5);
        bus.log.clear();

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert_eq!(
            cycles, 44,
            "3 writes + 2 vector + 2 refill + IACK + 12 idle"
        );
        assert_eq!(
            bus.writes(),
            vec![(0x2FFE, 0x1000), (0x2FFA, SR_S | 0x0200), (0x2FFC, 0x0000)],
            "the frame stacks pc - 4 and the OLD mask"
        );
        assert_eq!(cpu.sr, SR_S | 0x0500, "entry raises the mask to level 5");
        assert_eq!(cpu.pc, 0x2004, "vectored through 29, queue refilled");
    }

    /// An interrupt at or below the mask is not taken; level 7 always is.
    ///
    /// The control for the test above: without it, an implementation that takes
    /// every pending interrupt would pass that one.
    #[test]
    fn interrupts_at_or_below_the_mask_are_ignored_but_level_7_is_not() {
        for (level, mask, expect_taken) in [
            (2u8, 0x0200u16, false),
            (3, 0x0200, true),
            (7, 0x0700, true),
            (6, 0x0700, false),
        ] {
            let mut bus = RecordingBus::new();
            bus.load(0x1000, &[0x4E71, 0x4E71, 0x4E71]);
            bus.put16(0x2000, 0x4E71);

            let mut cpu = M68k::new();
            cpu.sr = SR_S | mask;
            cpu.a[7] = 0x3000;
            cpu.pc = 0x1000;
            cpu.prime_prefetch(&mut bus);
            cpu.set_irq(level);
            bus.log.clear();

            cpu.step_with(&Decoder::new(), &mut bus);

            let taken = !bus.writes().is_empty();
            assert_eq!(
                taken, expect_taken,
                "level {level} against mask {mask:#06X}"
            );
        }
    }

    /// Resuming from `STOP` must run the instruction *after* the STOP, not the
    /// STOP again, and must stack that instruction's address.
    ///
    /// # Extrapolated
    ///
    /// No vector case resumes from `stopped` — the suite snapshots the machine
    /// *while* stopped — so the literals here are extrapolated. What they rest on
    /// is measured: STOP leaves the PC and both queue words frozen with its own
    /// opcode `0x4E72` in slot 0 (1230/1230, RESET as a control at 0/1233 on
    /// every row), and `RAM[pc - 4] == prefetch[0]` (1230/1230). So `pc` is the
    /// address of the instruction after the STOP, and `0x1006` below is that
    /// literal address — the NOP following `STOP #imm` at `0x1000`. Asserting it
    /// against a recomputed `cpu.pc` would pass for any implementation.
    #[test]
    fn resuming_from_stop_runs_the_instruction_after_it() {
        let mut bus = RecordingBus::new();
        // 0x1000: STOP #$2000, 0x1004: NOP, 0x1006: NOP
        bus.load(0x1000, &[0x4E72, 0x2000, 0x4E71, 0x4E71, 0x4E71]);
        bus.put16(0x0074, 0x0000); // vector 29 = level 5
        bus.put16(0x0076, 0x3000);
        bus.load(0x3000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x4000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        let dec = Decoder::new();
        assert_eq!(cpu.step_with(&dec, &mut bus), 4, "STOP costs 4");
        assert!(cpu.stopped);
        assert_eq!(cpu.prefetch[0], 0x4E72, "the queue still holds the STOP");

        // Stopped with no interrupt: 4 cycles, and nothing moves.
        let pc_while_stopped = cpu.pc;
        assert_eq!(cpu.step_with(&dec, &mut bus), 4);
        assert_eq!(cpu.pc, pc_while_stopped, "the PC stays frozen");
        assert!(cpu.stopped);

        cpu.set_irq(5);
        bus.log.clear();
        cpu.step_with(&dec, &mut bus);

        assert!(!cpu.stopped, "the interrupt resumed the CPU");
        assert_eq!(
            bus.writes(),
            vec![(0x3FFE, 0x1004), (0x3FFA, 0x2000), (0x3FFC, 0x0000)],
            "the stacked PC is the NOP after the STOP, not the STOP itself"
        );
        assert_eq!(cpu.pc, 0x3004, "the handler is entered");
        assert_eq!(
            cpu.prefetch,
            [0x4E71, 0x4E71],
            "the handler's words, not the STOP's"
        );
    }

    /// A trace exception fires at the boundary *after* the traced instruction —
    /// which is why the one-instruction-per-case suite never sees one.
    ///
    /// # Extrapolated
    ///
    /// Vector 9 is fetched **0** times in all 317,500 cases, against a control of
    /// 2,500 TRAP fetches recovered by the same code, while 158,894 cases enter
    /// with T=1. So the cost and the stacked PC below are extrapolated from the
    /// other group-2 paths (34 cycles, `4 × 7 + 6`, a singleton across five
    /// measured paths). The *timing* — after, not during — is the measured part:
    /// any implementation that traced within the instruction's own step would
    /// fail 38% of the suite.
    #[test]
    fn trace_fires_on_the_boundary_after_the_instruction() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E71, 0x4E71, 0x4E71]);
        bus.put16(0x0024, 0x0000); // vector 9 = address 0x24
        bus.put16(0x0026, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | SR_T;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        let dec = Decoder::new();
        // Step 1: the NOP itself. No frame, and T survives.
        let nop_cycles = cpu.step_with(&dec, &mut bus);
        assert_eq!(nop_cycles, 4, "a NOP is a NOP even with T set");
        assert_eq!(
            bus.writes(),
            vec![],
            "no trace frame within the instruction"
        );
        assert_eq!(cpu.sr, SR_S | SR_T, "T survives a clean instruction");

        // Step 2: the boundary. Now the trace is taken.
        let trace_cycles = cpu.step_with(&dec, &mut bus);
        assert_eq!(trace_cycles, 34);
        assert_eq!(
            bus.writes(),
            vec![(0x2FFE, 0x1002), (0x2FFA, SR_S | SR_T), (0x2FFC, 0x0000)],
            "the stacked PC is the instruction after the traced NOP, and the \
             stacked SR keeps T set"
        );
        assert_eq!(cpu.sr & SR_T, 0, "the final SR clears T");
        assert_eq!(cpu.pc, 0x2004);
    }

    /// An odd frame base halts instead of writing a frame, and the frame's own
    /// writes never reach the bus.
    ///
    /// # Extrapolated
    ///
    /// Zero coverage: `initial A7 odd: 0/317,500`, `frame base odd at fault time:
    /// 0/55,606`, `bus-error (vector 2) fetches: 0`. The control for that zero is
    /// the *stacked* fault address, odd in 55,606/55,606 — so odd values are
    /// visible in a frame and the zero is a fact about the base. The reasoning is
    /// arithmetic (every stack offset is even), not measurement. This asserts only
    /// what the contract gives: halted, nothing on the bus, nothing committed.
    /// There is no frame, so there is no stacked PC to assert.
    #[test]
    fn an_odd_frame_base_halts_without_writing_a_frame() {
        let mut bus = RecordingBus::new();
        bus.put16(0x000C, 0x0000); // vector 3, so a frame would be visible
        bus.put16(0x000E, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x2FFF;
        cpu.pc = 0x1004;
        bus.log.clear();

        address_error(
            &mut cpu,
            &mut bus,
            0x1235,
            FaultKind::Read,
            Space::Data,
            0x4E71,
            0x1000,
        );

        assert!(cpu.halted, "odd frame base is a double bus fault");
        assert_eq!(bus.log, vec![], "no frame, no vector fetch");
        assert_eq!(cpu.a[7], 0x2FFF, "nothing committed");
        assert_eq!(cpu.sr, SR_S, "the SR is untouched");
        assert_eq!(cpu.pc, 0x1004, "the PC is untouched");
    }

    /// The control for the test above: an **even** frame base at the same odd
    /// fault address writes the ordinary seven-word frame.
    ///
    /// Without this, "no writes" would also hold for an `address_error` that was
    /// broken outright.
    #[test]
    fn an_even_frame_base_still_writes_the_long_frame() {
        let mut bus = RecordingBus::new();
        bus.put16(0x000C, 0x0000);
        bus.put16(0x000E, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1004;
        bus.log.clear();

        address_error(
            &mut cpu,
            &mut bus,
            0x1235,
            FaultKind::Read,
            Space::Data,
            0x4E71,
            0x1000,
        );

        assert!(!cpu.halted);
        assert_eq!(bus.writes().len(), 7, "the 7-word frame");
        assert_eq!(cpu.a[7], 0x2FF2, "base - 14");
    }

    /// A second fault raised while a frame is being written halts rather than
    /// recursing.
    ///
    /// # Extrapolated
    ///
    /// Unreachable today — [`Bus`] is infallible, so nothing inside either frame
    /// writer can fault — which is precisely why it is asserted directly here
    /// rather than through an instruction. Its job is to keep a future faulting
    /// bus from recursing until the *host* stack overflows.
    #[test]
    fn a_fault_during_exception_entry_halts_instead_of_recursing() {
        let mut bus = RecordingBus::new();
        bus.put16(0x000C, 0x0000);
        bus.put16(0x000E, 0x2000);

        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000; // even, so only `in_exception` can trigger the halt
        cpu.pc = 0x1004;
        cpu.in_exception = true;
        bus.log.clear();

        take(&mut cpu, &mut bus, VEC_ILLEGAL, 0x1000);

        assert!(cpu.halted);
        assert_eq!(bus.log, vec![], "no second frame");
        assert_eq!(cpu.a[7], 0x3000, "nothing committed");
    }
}
