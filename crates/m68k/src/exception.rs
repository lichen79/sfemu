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

/// Pushes a word onto the active stack: SP down 2, then write.
///
/// ⚠️ **Nothing calls this, and the reason is not "no handler needs it yet".** The
/// comment here used to read "used by future instruction handlers (TRAP, bus-error
/// frame, etc.)", which [`take`] fifty lines below directly contradicts: the 68000
/// writes the short frame in a **non-sequential** bus order that three sequential
/// `push16` calls cannot produce. `TRAP` was named as a future caller and is the
/// clearest case that can never be one. The other pushers — `push_return` in
/// `ops::branch`, `write_predec_long` in `ea` — likewise each have an order no shared
/// helper expresses; the core has four distinct 32-bit stack write orders.
///
/// So this is kept as documentation of the *simple* order, for comparison against the
/// three real ones, and not as a utility awaiting a caller. Removing its shadow-SP sync
/// kills no test in the workspace (measured, Task 14) — which says nothing about the
/// sync and everything about the function being unreachable. **A mutation score on dead
/// code measures only its deadness**; do not read that zero as evidence about stack
/// handling.
///
/// If you find yourself wanting this, check the required bus order first. If it is
/// sequential you are probably not modelling a real 68000 stack write.
#[allow(dead_code)]
pub(crate) fn push16(cpu: &mut M68k, bus: &mut dyn Bus, val: u16) {
    cpu.a[7] = cpu.a[7].wrapping_sub(2);
    bus.write16(cpu.a[7] & ADDR_MASK, val);
    // Kept coherent for the same debugging/save-state reason as `ops::system::sync_sp`;
    // see `M68k::a`'s docs for why the shadow is not an invariant.
    if cpu.sr_s() {
        cpu.ssp = cpu.a[7];
    } else {
        cpu.usp = cpu.a[7];
    }
}

/// Where the next exception frame will be written.
///
/// **Always the supervisor stack, even from user mode.** Entry sets S *before*
/// pushing, so the frame lands on the SSP: measured **43,483/43,483** user-mode
/// exception cases across all 127 groups, with the USP form as a control at
/// 0/43,483 and the USP itself untouched. Reading the *active* SP instead scores
/// ~50% — the coin-flip signature of a predicate that is itself half true.
///
/// The narrower 3,160/3,160 figure this doc used to quote is the same law measured
/// over TRAP, TRAPV and RTE alone; it was written without its denominator, which
/// made it read like a 14× disagreement with the census above rather than a subset
/// of it. State the scope with the count.
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
/// Called first by both [`take`] and [`address_error`], before either of *those
/// two functions* commits anything — no SR change, no push, no vector fetch.
/// Returns `true` if the CPU halted, in which case the caller must not proceed.
///
/// ⚠️ **This is a guarantee about `take`/`address_error`, not about the whole
/// boundary.** Two callers deliberately commit state before calling in, and both
/// are correct: [`check_interrupts`] and [`take_trace`] clear `stopped` and
/// refill the prefetch queue on the `STOP`-resume path, so a resumed entry that
/// then halts has already advanced the PC by 4 and logged 2 bus accesses.
/// Measured on this core: `halted=true acc=2 stopped true→false pc 0x1004→0x1008`
/// at both sites. That refill is real hardware behaviour — the queue is reloaded
/// when `STOP` is released, before the interrupt is acknowledged — and its 2
/// accesses are charged through [`entry_cycles`]'s lead term and asserted by
/// `a_halted_interrupt_entry_charges_its_refill_and_leaves_the_mask_alone` and
/// `a_halted_trace_entry_is_charged_for_its_resume_refill_and_no_more`.
///
/// So do not "restore" the stricter reading by moving those refills after the
/// halt check: it would contradict two passing tests and make the `resumed`
/// term dead. The invariant that actually holds, and the one worth preserving,
/// is that **no frame word is ever written and no vector ever fetched on a
/// halting entry** — `acc=2` is the resume refill alone, never frame traffic.
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

/// Idle cycles charged for the step in which the CPU double bus faults.
///
/// # Extrapolated: unmeasurable, not merely unmeasured
///
/// **0 of 317,500** cases halt — initial A7 is even in every one of them — so no
/// vector says how long hardware drives the bus before it gives up. The control
/// for that zero is the *stacked* fault address, odd in 55,606/55,606: odd
/// addresses are visible to the query, and the zero is a fact about A7. Nothing
/// measured can produce this number.
///
/// `4` is chosen rather than derived, for two statable reasons: it matches what
/// [`crate::cpu::M68k::step_with`] already charges for every *subsequent* step of
/// a halted CPU, and it keeps the halting step from returning 0, which would let a
/// cycle-budgeted driver loop make no progress. Replace it if hardware evidence
/// ever appears; the accompanying access count must not be replaced the same way,
/// because that part *is* derived — see [`entry_cycles`].
pub const HALTED_IDLE_CYCLES: u32 = 4;

/// What a handler charges for an exception entry that may have double bus
/// faulted.
///
/// Every constant in this crate that pays for an exception entry —
/// [`SHORT_FRAME_ENTRY_CYCLES`], [`ADDRESS_ERROR_TAIL_CYCLES`],
/// [`INTERRUPT_CYCLES`] — is `4 × accesses + idle` for accesses that
/// `double_bus_fault` never performs: no frame, no vector fetch, no refill.
/// Returning one of them after a halt contradicts the core's own bus log, and the
/// timing law is not optional. So the entry's cost collapses:
///
/// - `accesses_made` — accesses the handler **already put on the bus** before the
///   entry, which are still owed because they really happened: `TRAPV`'s leading
///   read, `RTE`'s three pops. This term is *derived*, by reading the core's own
///   bus log on each path (`RTE`/odd-SSP logs 0; `PEA`/odd-SP logs 1).
/// - [`HALTED_IDLE_CYCLES`] — the idle before the halt, which is extrapolated.
///
/// `framed` is the unchanged cost of the path where the frame *was* written.
///
/// ⚠️ **Pass 0 when the caller already adds its lead outside this call.** Several
/// sites are written `4 * lead + idle + ADDRESS_ERROR_TAIL_CYCLES`, with the
/// preceding accesses in the caller's own term; wrapping only the tail keeps that
/// arithmetic visible and must not double-count. Sites that fold their lead into
/// the constant instead — `TRAPV`'s `4 × (7 + 1)`, `RTE`'s `4 × 3 + tail` — pass
/// the lead here. Either way the total obeys the law; what must not happen is
/// counting a lead twice or not at all.
///
/// Taking `cpu` rather than a bare `bool` is deliberate: a site that has to spell
/// out `cpu.halted` is a site that can forget to. The halt tests assert the
/// cycle count precisely because its absence is how a stale constant survived
/// here once already.
///
/// # This is now the single chokepoint, and that is a checked claim
///
/// Task 11 left this doc saying "not yet a single chokepoint", naming
/// `ops::{alu, branch, move_, muldiv, logic}` as having fault sites that still
/// returned their framed constant unconditionally. All of them do route through
/// here now. The six that were fixed in Task 14 were each *measured* first, by
/// stepping the core into the halt and reading its own bus log — not argued from
/// the shape of the code:
///
/// ```text
///   alu::run fault arm     ADD.w D0,(A0)   odd A0, odd SSP   58 claimed, 0 acc
///   alu::run_tail Trapped  CHK.w D1,D0     trapping, odd SSP 40 claimed, 0 acc
///   branch::target_error   JMP (A0)        odd target/SSP    58 claimed, 0 acc
///   move_ source fault     MOVE.w (A0),D0  odd A0, odd SSP   58 claimed, 0 acc
///   move_ dest fault       MOVE.w D0,(A0)  odd A0, odd SSP   58 claimed, 0 acc
///   logic::to_ccr_sr       ORI #1,SR       user, odd SSP     34 claimed, 0 acc
/// ```
///
/// Each is pinned by a test named `a_halted_*`, and each of those tests was
/// confirmed to fail — alone — with its fix reverted. Two of them needed a second
/// case with a **nonzero lead** to be meaningful at all: the zero-lead form scores
/// the same whether the lead is charged or dropped into `accesses_made`, which is
/// how `pea`'s missing lead idle survived a test that looked like it covered it.
///
/// ⚠️ So do not add a fault site that returns a framed constant directly. There is
/// no longer a precedent for it in this crate, and the class is invisible to both
/// `clippy` and the vector suite: 0 of 317,500 cases halt, so **every one of these
/// six was wrong through thirteen tasks of a fully green suite.**
#[inline]
pub fn entry_cycles(cpu: &M68k, accesses_made: u32, framed: u32) -> u32 {
    if cpu.halted {
        4 * accesses_made + HALTED_IDLE_CYCLES
    } else {
        framed
    }
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

/// Cycles an exception entry costs when the frame is the *first* thing it does:
/// `4 * SHORT_FRAME_ACCESSES + 6`, which is **34**.
///
/// Measured 18,776/18,776 at this decomposition — `TRAP` 2,500, LINE-A 2,500,
/// LINE-F 2,500, and 11,276 privilege violations across nine groups. Those are
/// the four code sites that shared the arithmetic and now share this name.
///
/// # Three things at 34 that are not this
///
/// The total 34 is *not* the claim; the split is. A handler that reaches 34 by a
/// different route must keep its own spelling, because consolidating on the
/// total would assert a bus shape the vectors contradict:
///
/// - **`TRAPV`'s 1,250 trapping cases are `4×8 + 2`.** One more access — the
///   queue advance it performs before checking V — and 4 fewer idle. Same total,
///   different shape; see `ops::trap::trapv`.
/// - **The trace exception is extrapolated.** Vector 9 is fetched 0 times in
///   317,500 cases, so its 34 rests on the frame shape alone, and its resume
///   path has a lead of 2. See [`take_trace`].
/// - **`CHK` is group 2 and never costs 34.** Its 1,326 trapping cases run
///   38/40/42/44/46/48/50/52 across ten `(accesses, idle)` shapes, because it
///   pays for an operand comparison first. No statement of the form "group 2
///   costs 34" is true — the true scope is "entries whose frame is their first
///   access", which is what this constant's name says.
///
/// So this replaces four identical expressions, not seven occurrences of the
/// number 34. Two of the remaining three are deliberate; the third does not
/// exist, and asserting 20,026 cases against `4×7 + 6` would be describing only
/// 18,776 of them.
pub const SHORT_FRAME_ENTRY_CYCLES: u32 = 4 * SHORT_FRAME_ACCESSES + 6;

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
    // An aborted instruction owes no trace, and this is the only place that knows an
    // abort happened. The PRM conditions the trace on the instruction being
    // *completed*; a group-0 fault stops it mid-way, with the operand unstored. So
    // the trace latched at this instruction's start is withdrawn here.
    //
    // ⚠️ Deliberately **not** done in `take`, which is the group-1/2 path. An
    // instruction trap is a completion — `TRAP`, `TRAPV`, `CHK`, divide-by-zero,
    // illegal and privilege all did exactly what they are defined to do — so those
    // still owe their trace, taken before the handler's first instruction. The two
    // cases are opposite, which is why the withdrawal lives at the group-0
    // chokepoint rather than at a shared `!halted` test.
    //
    // Both directions are asserted by
    // `tests::an_aborted_instruction_owes_no_trace_but_a_completed_trap_does`, and
    // both are extrapolated: vector 9 is fetched 0 times in 317,500 cases. The
    // 38,542/38,542 "entry clears T" census cannot adjudicate this — entry clears T
    // on both paths, so it does not distinguish them.
    cpu.trace_pending = false;
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
/// The manual's interrupt row is `44(5/3)`, whose eighth access is the
/// interrupt-acknowledge cycle.
///
/// ⚠️ **This core does not emit an IACK access, and the constant is spelled to
/// say so.** [`Bus`] carries no function code, so an IACK is not representable
/// distinctly from an ordinary read; inventing one would add a transaction to a
/// group shape that is otherwise measured access-for-access. So the decomposition
/// this core actually produces is
///
/// ```text
///   4 × 7 accesses + 16 idle = 44        <- what check_interrupts emits
///   4 × 8 accesses + 12 idle = 44        <- the manual's, with the IACK modelled
/// ```
///
/// — the IACK's 4 cycles are **spent as idle** rather than on the bus. The total
/// is the manual's either way; the split is not, and a core that ever gains
/// function codes should move those 4 cycles back onto the bus.
///
/// ⚠️ Do not read TRAPV's `(5/3)` idle 2 as licence to infer a split from a
/// total: two different decompositions reach 34 on *measured* paths, so a total
/// never determines a split on this core — and this constant is a third example.
pub const INTERRUPT_CYCLES: u32 = 4 * SHORT_FRAME_ACCESSES + IACK_AS_IDLE_CYCLES + 12;

/// The interrupt-acknowledge cycle's 4 cycles, spent as idle.
///
/// Named rather than folded into [`INTERRUPT_CYCLES`]'s idle term so that the one
/// unmodelled access in the interrupt path is visible in the arithmetic instead of
/// hiding inside a `16`. See [`INTERRUPT_CYCLES`] for why it is not a bus access.
pub const IACK_AS_IDLE_CYCLES: u32 = 4;

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
/// # The caller owns deassertion — and level 7 has no substitute for it
///
/// [`M68k::pending_irq`] is a **level**, not an edge, and nothing in this core
/// clears it. Entry raises the mask, which is what stops levels 1-6 from
/// re-entering their own handler; **level 7 is non-maskable, so no mask value can
/// block it.** A level-7 line left asserted therefore re-enters the handler at
/// every boundary without the handler executing a single instruction, marching the
/// stack pointer down 6 bytes per step until it wraps. That is a livelock, and it
/// is the caller's to prevent: the device model must drop the line — [`M68k::set_irq`]
/// with 0 — once the handler has acknowledged it, exactly as
/// `testrunner/tests/integration_asm.rs` does for its level-4 handler.
///
/// This is documented rather than fixed by modelling level 7 as edge-triggered,
/// because edge sensitivity would change the interrupt contract that existing
/// integration tests are written against. See [`M68k::set_irq`].
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
    let resumed = cpu.stopped;
    if resumed {
        cpu.stopped = false;
        cpu.refill_prefetch_dyn(bus);
    }

    let pc = cpu.pc.wrapping_sub(OPCODE_PC_OFFSET);
    take(cpu, bus, VEC_AUTOVECTOR_BASE + level, pc);
    if !cpu.halted {
        // Mask the level being serviced, after the old SR is safely on the stack.
        // Skipped on the halt path: no frame holds the old mask, and there is no
        // handler to run at the new one — `take` leaves the SR alone there too.
        cpu.set_sr((cpu.sr & !0x0700) | (u16::from(level) << 8));
    }
    // The resume refill is the only thing that precedes the entry, so a halted
    // entry is charged for those 2 accesses and nothing else.
    Some(entry_cycles(
        cpu,
        if resumed { 2 } else { 0 },
        INTERRUPT_CYCLES,
    ))
}

/// Takes the trace exception owed by the instruction that just finished,
/// returning its cost.
///
/// Called only when [`M68k::trace_pending`] is set, which is where the decision
/// lives: **T is sampled at the start of the instruction**, not read here at the
/// end. This function does not consult `cpu.sr & SR_T` at all, and must not —
/// exception entry has already cleared T by the time a forced exception reaches
/// this boundary, and an instruction that loaded the whole SR has changed T
/// arbitrarily. See [`M68k::trace_pending`] for the two cases that discriminate
/// start-sampling from end-sampling and why end-sampling breaks single-stepping.
///
/// # Fired at the boundary *after* the instruction — this part is measured
///
/// Each vector case runs exactly one instruction and stops at this boundary, so
/// `TRACE` (vector 9) is fetched **0** times in all 317,500 cases, against a
/// control of 2,500 TRAP fetches recovered by the same code, while 158,894 cases
/// *enter* with T=1 across all 127 groups. An implementation that traced inside
/// the instruction's own step would fail ~38% of the suite — that is what the
/// measurement settles, and it settles nothing else. In particular it cannot
/// discriminate *when T is sampled*, because no case ever reaches the second
/// boundary; see the "Extrapolated" note below.
///
/// # A trace exits the stopped state
///
/// `STOP #$A700` loads an SR with T set, so it is stopped *and* owes a trace, and
/// hardware has no state for "inside a handler while stopped": the trace is taken
/// and the CPU resumes. Clearing `stopped` here is what makes that true — without
/// it the CPU wedges permanently, with the handler's first instruction never run,
/// and only an interrupt escapes (into that same un-started handler).
///
/// The resume is [`check_interrupts`]'s, verbatim and for the same reason: the
/// stopped state froze `pc` and both queue words with the `STOP` opcode in slot 0,
/// and `pc - OPCODE_PC_OFFSET` is the `STOP`'s **own** address until the refill
/// moves `pc` on. Skipping the refill stacks the `STOP` and re-executes it after
/// the handler returns. Those 2 accesses are not in the returned cost, which is the
/// same extrapolation [`check_interrupts`] documents.
///
/// # Extrapolated: zero suite coverage
///
/// Both the cost and the **sampling point** are extrapolated, for the same reason:
/// no case takes a trace exception, so no measurement here can be a count.
///
/// - *Cost:* [`SHORT_FRAME_ENTRY_CYCLES`], the 34 measured at `4×7 + 6` over
///   18,776 cases in twelve groups. ⚠️ It is **not** "the same 34 five other
///   group-2 paths measure", as this line used to say: the number 34 is reached by
///   two different splits, `TRAPV` uses the other one, and `CHK` is group 2 and
///   never reaches 34 at all. Borrowing the 7-access arm is a choice between two
///   shapes, and nothing verifies either for vector 9 specifically.
/// - *Sampling point:* start-of-instruction, from the 68000 User's Manual's
///   definition of T. ⚠️ The suite's census of which instructions can *clear* T
///   (`ANDItoSR`, `EORItoSR`, `STOP`, `MOVEtoSR`, `RTE` — 1,277 clean cases, with
///   `ORItoSR` at 0/591 as the control) is accurate and lives at
///   [`M68k::trace_pending`], but it is a census of instructions, not of sampling
///   points, and it cannot support this choice. It is named there as the set of
///   cases where the two rules *differ*, which is what it can speak to.
pub fn take_trace(cpu: &mut M68k, bus: &mut dyn Bus) -> u32 {
    let resumed = cpu.stopped;
    if resumed {
        cpu.stopped = false;
        cpu.refill_prefetch_dyn(bus);
    }
    let pc = cpu.pc.wrapping_sub(OPCODE_PC_OFFSET);
    take(cpu, bus, VEC_TRACE, pc);
    // Only the resume refill can precede the frame; without it a halt here logged
    // nothing at all.
    // ⚠️ Extrapolated: vector 9 is fetched 0 times in 317,500 cases, so this
    // shares `SHORT_FRAME_ENTRY_CYCLES`' arithmetic without sharing its evidence.
    // The name is the shape claim — frame first, 7 accesses, 6 idle — and that
    // much is structural; the number is unmeasured for *this* vector.
    entry_cycles(cpu, if resumed { 2 } else { 0 }, SHORT_FRAME_ENTRY_CYCLES)
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
    // No access precedes the frame — the queue is never advanced on this path.
    entry_cycles(cpu, 0, SHORT_FRAME_ENTRY_CYCLES)
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
    ///
    /// ⚠️ The vector number below is an **independent literal**, not
    /// [`VEC_LINE_F`] / [`VEC_ILLEGAL`]. This test used to derive the handler
    /// address from the constant it was testing, which made the assertion
    /// self-consistent for every constant value: the handler moved with the
    /// mutation and the test still passed. Measured — `VEC_LINE_F = 12` survived
    /// all 198 unit tests in that form and is killed here now, along with `= 10`
    /// (which aliases Line-A) and `= 5`. `VEC_ILLEGAL` had independent coverage
    /// either way: its mutants fail two tests, not one.
    ///
    /// Keep the literals. A test that installs the handler wherever the constant
    /// points is checking self-consistency, never the vector number.
    #[test]
    fn line_f_and_plain_illegal_use_their_own_vectors() {
        for (opcode, vector) in [(0xF000u16, 11u32), (0x4AFC, 4u32)] {
            let mut bus = FlatBus::new();
            bus.load(0x1000, &[opcode, 0x4E71]);
            let vaddr = vector * 4;
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

        // The access count is asserted alongside the total, because the total
        // alone would pass for the manual's 8-access split too — and this core
        // emits 7. Never assert one without the other on this path.
        assert_eq!(
            bus.log.len(),
            SHORT_FRAME_ACCESSES as usize,
            "3 frame writes + 2 vector reads + 2 refills, and NO IACK access: \
             `Bus` carries no function code, so this core spends the IACK's 4 \
             cycles as idle"
        );
        assert_eq!(
            cycles, 44,
            "4 × 7 accesses + 4 (the IACK, as idle) + 12 idle — the manual's \
             total, reached by this core's own decomposition and not by its"
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
    /// group-2 paths that *are* measured at 34 cycles: the **twelve groups** at
    /// `4 × 7 + 6` — TRAP 2500, LINE-A 2500, LINE-F 2500 and the nine privilege
    /// groups' 11,276, so 18,776 cases — plus TRAPV's 1250 reaching 34 at
    /// `4 × 8 + 2`. Borrowing the 7-access arm is a choice between those two
    /// shapes, not a reading off a uniform family, and `CHK` shows the category
    /// is not uniform at all: it is group 2 and costs 38-52, never 34.
    ///
    /// Vectors **4** (illegal instruction) and **5** (divide-by-zero) are also
    /// fetched 0 times, so this is one of *three* unmeasured entry paths, not a
    /// lone gap. `ILLEGAL_LINEA`/`ILLEGAL_LINEF` measure vectors 10 and 11 — the
    /// group names say ILLEGAL but neither exercises vector 4.
    ///
    /// The *timing* — after, not during — is the measured part: any
    /// implementation that traced within the instruction's own step would fail
    /// 38% of the suite.
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

    /// The frame base in **user** mode is the SSP, not the active `a[7]`.
    ///
    /// This is the only test that can tell those two apart: `frame_base` is used
    /// solely as a *parity* predicate, so a case where `a[7]` and `ssp` agree in
    /// parity cannot discriminate — and every other test, plus all 317,500
    /// vector cases, has them agreeing. Replacing the body with `cpu.a[7]`
    /// passes the entire suite and every other unit test.
    ///
    /// # Extrapolated
    ///
    /// Zero suite coverage: initial A7 is even in 317,500/317,500 cases. The
    /// rule is that entry sets S *before* pushing, so the frame lands on the
    /// supervisor stack; measured 43,483/43,483 user-mode exception cases put
    /// the frame on the SSP, with the USP form as a control at 0/43,483.
    #[test]
    fn in_user_mode_the_frame_base_is_the_ssp_not_the_active_sp() {
        // An odd USP with an even SSP must NOT halt: the frame goes to the SSP.
        let mut bus = RecordingBus::new();
        bus.put16(0x000C, 0x0000); // vector 3, so a frame would be visible
        bus.put16(0x000E, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = 0x0000; // user mode
        cpu.a[7] = 0x7FFF; // odd USP
        cpu.ssp = 0x3000; // even SSP
        cpu.pc = 0x1004;
        bus.log.clear();

        take(&mut cpu, &mut bus, VEC_ILLEGAL, 0x1000);

        assert!(
            !cpu.halted,
            "an odd USP is irrelevant: the frame is on the SSP"
        );
        assert_eq!(bus.writes().len(), 3, "the short frame was written");
        assert_eq!(
            cpu.a[7], 0x2FFA,
            "a[7] is now the supervisor stack, base - 6"
        );
        assert_eq!(cpu.usp, 0x7FFF, "the odd USP is preserved untouched");

        // The converse: an even USP with an odd SSP must halt.
        let mut bus = RecordingBus::new();
        bus.put16(0x000C, 0x0000);
        bus.put16(0x000E, 0x2000);

        let mut cpu = M68k::new();
        cpu.sr = 0x0000;
        cpu.a[7] = 0x8000; // even USP
        cpu.ssp = 0x2FFF; // odd SSP
        cpu.pc = 0x1004;
        bus.log.clear();

        take(&mut cpu, &mut bus, VEC_ILLEGAL, 0x1000);

        assert!(
            cpu.halted,
            "an odd SSP is the double bus fault, from user mode too"
        );
        assert_eq!(bus.log, vec![], "no frame, no vector fetch");
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

    /// Single-stepping makes progress: a trace handler ending in `RTE` runs one
    /// traced instruction per handler entry, forever.
    ///
    /// **This is the test for the sampling point**, and it is the whole reason T
    /// must be latched at instruction *start*. The shape is the canonical debugger
    /// one: T set in the traced program, handler = a bare `RTE`, and the popped SR
    /// restores T. `RTE` is *entered* with T clear — exception entry cleared it —
    /// so hardware owes no trace for the `RTE` itself; the resumed instruction runs
    /// and then traces. A core that samples T at the *end* owes a trace for the
    /// `RTE`, whose popped SR has T set, and so re-enters the handler having
    /// executed nothing: measured at 1 traced instruction in 200 steps against the
    /// 4 below, with the PC pinned and the SP walking down 6 bytes per pair.
    ///
    /// Four NOPs at `0x2000`, so a livelock is distinguishable from termination:
    /// the assertion is on how many *distinct* traced-program PCs were reached, not
    /// merely that the handler ran.
    ///
    /// # Extrapolated
    ///
    /// Zero suite coverage — vector 9 is fetched 0 times in 317,500 cases, against
    /// 158,894 that enter with T=1 (the control showing T is present in the corpus
    /// and only the *second boundary* is missing). So this asserts a mechanism, not
    /// a measured literal: no cycle counts appear below.
    #[test]
    fn single_stepping_with_an_rte_handler_advances_one_instruction_per_trace() {
        let mut bus = RecordingBus::new();
        // The traced program: four NOPs at 0x2000.
        bus.load(0x2000, &[0x4E71, 0x4E71, 0x4E71, 0x4E71]);
        // Vector 9 -> the handler at 0x3000, which is a bare RTE.
        bus.put16(0x0024, 0x0000);
        bus.put16(0x0026, 0x3000);
        bus.load(0x3000, &[0x4E73, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | SR_T | 0x0700;
        cpu.a[7] = 0x4000;
        cpu.pc = 0x2000;
        cpu.prime_prefetch(&mut bus);

        let dec = Decoder::new();
        // Each traced instruction takes 3 steps: the instruction, the trace entry,
        // the handler's RTE. 12 steps is exactly 4 NOPs' worth.
        let mut reached = Vec::new();
        for _ in 0..12 {
            // Record the traced-program PCs, i.e. the ones outside the handler.
            if cpu.pc < 0x3000 && !cpu.trace_pending {
                reached.push(cpu.pc);
            }
            cpu.step_with(&dec, &mut bus);
            assert!(!cpu.halted, "single-stepping must not halt");
        }

        assert_eq!(
            reached,
            vec![0x2004, 0x2006, 0x2008, 0x200A],
            "one distinct traced instruction per handler entry: an \
             end-of-instruction sample re-enters the handler forever and reaches \
             only the first"
        );
        assert_eq!(
            cpu.a[7], 0x4000,
            "each RTE unwinds its own frame; the stack does not drift"
        );
    }

    /// The two cases that discriminate start-sampling from end-sampling directly,
    /// without the handler: an instruction that clears T is still traced, and one
    /// that sets T is not.
    ///
    /// The control for the test above, and sharper: that one shows the *consequence*
    /// (a livelock), this one shows the *rule* on the single instruction pair where
    /// the two rules disagree. `ANDI #$7FFF,SR` and `ORI #$8000,SR` differ only in
    /// which way they move T.
    ///
    /// # Extrapolated
    ///
    /// The sampling point is from the manual; the suite cannot reach it (0 vector-9
    /// fetches in 317,500). What the suite *does* establish is that these are the
    /// right instructions to test with: `ANDItoSR` is among the 1,277 clean cases
    /// that end with T clear, with `ORItoSR` at 0/591 as the control.
    ///
    /// ⚠️ **This asserts the vector-9 frame, never `trace_pending`.** The sample
    /// point is gated twice — once latching the flag in
    /// [`crate::cpu::M68k::step_with`], once deciding whether to actually vector in
    /// [`take_trace`] — and a flag assertion only sees the first. Restoring the
    /// second gate's old `if cpu.sr & SR_T == 0 { return }`, with the latch left
    /// correct, yields `trace_pending == true` and **still no trace**, because that
    /// read sees the SR `ANDI` just cleared. Verified: that mutation passes all 195
    /// tests when this test reads the flag, and fails here as written. The flag is
    /// the code's own intermediate value; the frame is the behaviour.
    #[test]
    fn t_is_sampled_at_instruction_start_not_at_its_end() {
        // Direction 1 — `ANDI #$7FFF,SR` clears T, entered with T=1: still traced.
        // This is the direction end-sampling gets wrong, and 1,277 real suite cases.
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x027C, 0x7FFF, 0x4E71, 0x4E71, 0x4E71]);
        bus.put16(0x0024, 0x0000); // vector 9 -> the handler at 0x2000
        bus.put16(0x0026, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | SR_T | 0x0700;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        let dec = Decoder::new();
        bus.log.clear();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(
            bus.writes(),
            vec![],
            "no frame within the traced instruction"
        );
        assert_eq!(cpu.sr & SR_T, 0, "the ANDI really did clear T");

        bus.log.clear();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(
            bus.writes(),
            vec![(0x2FFE, 0x1004), (0x2FFA, SR_S | 0x0700), (0x2FFC, 0x0000)],
            "T was set at entry, so the trace is owed even though the \
             instruction cleared it; the stacked SR is the cleared one"
        );
        assert_eq!(cpu.pc, 0x2004, "vectored through 9");

        // Direction 2 — `ORI #$8000,SR` sets T, entered with T=0: that instruction
        // is NOT traced, and the one after it is. 1,286 real suite cases, and the
        // control: without it, a core that always traces passes the block above.
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x007C, 0x8000, 0x4E71, 0x4E71, 0x4E71]);
        bus.put16(0x0024, 0x0000);
        bus.put16(0x0026, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | 0x0700;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        bus.log.clear();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.sr & SR_T, SR_T, "the ORI really did set T");
        assert_eq!(
            bus.writes(),
            vec![],
            "setting T does not trace the instruction that set it"
        );

        // The NOP after it: T was set at *its* start, so it is the traced one.
        bus.log.clear();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(bus.writes(), vec![], "still inside the traced instruction");

        bus.log.clear();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(
            bus.writes(),
            vec![
                (0x2FFE, 0x1006),
                (0x2FFA, SR_S | SR_T | 0x0700),
                (0x2FFC, 0x0000)
            ],
            "tracing begins with the instruction after the one that set T"
        );
        assert_eq!(cpu.pc, 0x2004, "vectored through 9");
    }

    /// An instruction that *aborts* on an address error owes **no** trace; one that
    /// completes into an instruction trap still does.
    ///
    /// The PRM conditions the trace on the instruction being **completed**:
    ///
    /// > "If the T bit is set at the beginning of the execution of an instruction, a
    /// > trace exception is generated after the instruction is completed."
    ///
    /// Group 0 faults (address error, bus error) abort mid-instruction — the write
    /// never happens and the operand is never stored — so the condition is not met
    /// and no trace is owed. Group 1/2 instruction traps (`TRAP`, `TRAPV`, `CHK`,
    /// divide-by-zero, illegal, privilege) *are* completions: the instruction did
    /// exactly what it is defined to do, so the trace is still owed and is taken
    /// before the handler's first instruction.
    ///
    /// So the two are opposite, and a single `!halted` test cannot express both. This
    /// distinction was previously implemented in the coarse form — every latched
    /// trace survived any non-halting exception — which owed a trace after an
    /// aborted instruction. Probed on that form: the faulting `MOVE.W D0,(A0)`
    /// vectored to 3, and the *next* step fetched vector 9 and entered the trace
    /// handler at `0x3004`.
    ///
    /// # Extrapolated
    ///
    /// Both halves. Vector 9 is fetched 0 times in 317,500 cases, so nothing here is
    /// measured; the control for that zero is the 158,894 cases that enter with T set,
    /// which shows T is well represented and only the second boundary is missing.
    /// The 38,542/38,542 figure sometimes cited nearby says only that exception entry
    /// clears T — it cannot distinguish these two cases, because entry clears T in
    /// both.
    #[test]
    fn an_aborted_instruction_owes_no_trace_but_a_completed_trap_does() {
        // Vectors and handlers shared by both halves: 3 -> 0x2000, 9 -> 0x3000,
        // TRAP #0 (vector 32) -> 0x4000.
        let vectors = |bus: &mut RecordingBus| {
            bus.put16(0x000C, 0x0000);
            bus.put16(0x000E, 0x2000);
            bus.load(0x2000, &[0x4E71, 0x4E71]);
            bus.put16(0x0024, 0x0000);
            bus.put16(0x0026, 0x3000);
            bus.load(0x3000, &[0x4E71, 0x4E71]);
            bus.put16(0x0080, 0x0000);
            bus.put16(0x0082, 0x4000);
            bus.load(0x4000, &[0x4E71, 0x4E71]);
        };

        // Half 1 — `MOVE.W D0,(A0)` with A0 odd. The write aborts, so the
        // instruction never completed and no trace is owed.
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x3080, 0x4E71, 0x4E71]);
        vectors(&mut bus);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | SR_T | 0x0700;
        cpu.a[7] = 0x5000;
        cpu.a[0] = 0x6001; // odd: the destination faults
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert!(
            !cpu.halted,
            "an odd operand address faults, it does not halt"
        );
        assert_eq!(cpu.pc, 0x2004, "vectored through 3");

        bus.log.clear();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(
            bus.writes(),
            vec![],
            "no trace frame: the aborted instruction never completed, so the PRM's \
             condition was never met"
        );
        assert_eq!(
            cpu.pc, 0x2006,
            "execution continues in the vector-3 handler, not in a trace handler"
        );

        // Half 2 — the control, and the direction that must NOT change: `TRAP #0`
        // completes, so its trace is still owed and is taken before the trap
        // handler's first instruction.
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E40, 0x4E71, 0x4E71]); // TRAP #0
        vectors(&mut bus);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | SR_T | 0x0700;
        cpu.a[7] = 0x5000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.pc, 0x4004, "vectored through 32");

        bus.log.clear();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(
            cpu.pc, 0x3004,
            "the TRAP completed, so its trace fires before the trap handler runs"
        );
        assert_eq!(
            bus.writes().len(),
            3,
            "and it is a real frame, stacked on top of the trap's"
        );
    }

    /// `STOP #$A700` — an immediate that sets T — traces and **resumes**, rather
    /// than leaving the CPU both stopped and inside a handler.
    ///
    /// That combined state has no hardware counterpart, and a core that reaches it
    /// is permanently wedged: every later step falls through the `stopped` gate and
    /// returns 4 with no bus activity, the handler's first instruction never run.
    /// Measured on the defect: `pc` pinned at the vector target and 0 accesses over
    /// 20 further steps. The two assertions that bite are `!cpu.stopped` and the
    /// handler's NOP actually executing.
    ///
    /// # Extrapolated
    ///
    /// Zero coverage twice over: no case runs a second step after `STOP` (its
    /// access shape is empty), and vector 9 is fetched 0 times in 317,500. The
    /// trace-on-a-T-setting-immediate rule is the manual's `STOP` entry.
    #[test]
    fn stop_with_t_set_traces_and_resumes_instead_of_wedging() {
        let mut bus = RecordingBus::new();
        // 0x1000: STOP #$A700, 0x1004: NOP
        bus.load(0x1000, &[0x4E72, 0xA700, 0x4E71, 0x4E71]);
        bus.put16(0x0024, 0x0000); // vector 9 -> 0x2000
        bus.put16(0x0026, 0x2000);
        // The handler: ADDQ #1,D0 then NOP, so "the handler ran" is observable.
        bus.load(0x2000, &[0x5240, 0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | 0x0700;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        let dec = Decoder::new();
        assert_eq!(cpu.step_with(&dec, &mut bus), 4, "STOP still costs 4");
        assert!(cpu.stopped);
        assert!(cpu.trace_pending, "the immediate set T, so a trace is owed");

        // The trace fires, and it must clear `stopped` on its way.
        let trace_cycles = cpu.step_with(&dec, &mut bus);
        assert_eq!(trace_cycles, 34, "the group-2 short frame");
        assert!(
            !cpu.stopped,
            "the trace resumed the CPU: nothing is both stopped and in a handler"
        );
        assert_eq!(cpu.pc, 0x2004, "the handler was entered");
        assert_eq!(
            bus.writes(),
            vec![(0x2FFE, 0x1004), (0x2FFA, SR_S | 0xA700), (0x2FFC, 0x0000)],
            "the frame stacks the instruction after the STOP, and an SR with T set"
        );

        // And the handler makes progress, which is the half a wedged core fails.
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0], 1, "the handler's first instruction executed");
    }

    /// The `stopped` state survives a step that neither traces nor takes an
    /// interrupt — the control for the test above.
    ///
    /// Without this, clearing `stopped` unconditionally at the boundary would pass
    /// that test while breaking `STOP` outright.
    #[test]
    fn stop_without_t_stays_stopped() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E72, 0x2700, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | 0x0700;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert!(cpu.stopped);
        assert!(!cpu.trace_pending, "the immediate left T clear");

        let pc = cpu.pc;
        bus.log.clear();
        for _ in 0..5 {
            assert_eq!(cpu.step_with(&dec, &mut bus), 4);
        }
        assert!(cpu.stopped, "still stopped with no interrupt and no trace");
        assert_eq!(cpu.pc, pc, "the PC stayed frozen");
        assert_eq!(bus.log, vec![], "and nothing reached the bus");
    }

    /// A halted exception entry is charged for the accesses it actually made,
    /// which on this path is none of them.
    ///
    /// The **cycle** assertion is the point. `an_odd_frame_base_halts_without_writing_a_frame`
    /// above asserts state and an empty bus log but not the cost, which is how
    /// `ADDRESS_ERROR_TAIL_CYCLES`'s 58 — `4 × 12 + 10`, paying for an aborted
    /// access, 7 frame writes, 2 vector reads and 2 refills — survived onto a path
    /// that performs zero accesses. Under the timing law that is a 58-cycle lie
    /// about the core's own bus log.
    ///
    /// # Extrapolated
    ///
    /// The access count is *derived* from the bus log asserted alongside it, so it
    /// is not extrapolated. [`HALTED_IDLE_CYCLES`] is: 0 of 317,500 cases halt, with
    /// the stacked fault address (odd in 55,606/55,606) as the control for that
    /// zero. If that constant changes, this literal changes with it — deliberately,
    /// via `HALTED_IDLE_CYCLES` rather than a bare number.
    #[test]
    fn a_halted_entry_costs_only_the_accesses_it_made() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4AFC, 0x4E71]); // an illegal opcode
        bus.put16(0x0010, 0x0000); // vector 4, so a frame would be visible
        bus.put16(0x0012, 0x2000);

        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x2FFF; // odd: the frame's own base, so entry halts
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert!(cpu.halted);
        assert_eq!(bus.log, vec![], "zero accesses: no frame, no vector fetch");
        assert_eq!(
            cycles, HALTED_IDLE_CYCLES,
            "4 × 0 accesses + the extrapolated halt idle — not the framed 34"
        );

        // The two *properties* [`HALTED_IDLE_CYCLES`]'s value was chosen for, as
        // opposed to the value itself. Asserting `4` as a literal would claim a
        // measurement nobody has; these two are consequences the choice is answerable
        // for, and they are what a change to the constant has to preserve.
        assert!(
            cycles > 0,
            "a halting step that costs nothing lets a cycle-budgeted driver loop \
             forever without advancing its clock"
        );
        assert_eq!(
            cycles,
            cpu.step_with(&Decoder::new(), &mut bus),
            "the halting step costs what every subsequent halted step costs: the \
             halt is not a discontinuity in the cycle stream"
        );
    }

    /// The control for the test above: the same illegal opcode with an **even**
    /// frame base pays the full framed cost, so the collapse is conditional rather
    /// than a blanket reduction.
    #[test]
    fn an_unhalted_entry_still_costs_the_full_frame() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4AFC, 0x4E71]);
        bus.put16(0x0010, 0x0000);
        bus.put16(0x0012, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000; // even
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert!(!cpu.halted);
        assert_eq!(bus.log.len(), 7, "the full short-frame access count");
        assert_eq!(cycles, 4 * SHORT_FRAME_ACCESSES + 6, "4 × 7 + 6 idle");
    }

    /// The same collapse at [`take_trace`]'s own site, in both of its shapes.
    ///
    /// This is a second site of the defect above rather than a repeat of it: the
    /// trace entry is the one path whose access count on a halt is **not** zero, so
    /// the halted cost is not a constant and a test asserting only
    /// [`HALTED_IDLE_CYCLES`] elsewhere cannot reach it.
    ///
    /// | entered from | accesses before the halt | cost |
    /// |---|---|---|
    /// | a traced instruction | 0 | `HALTED_IDLE_CYCLES` |
    /// | a `STOP` being resumed | 2, the resume refill | `4 × 2 + HALTED_IDLE_CYCLES` |
    ///
    /// The second row is what pins [`take_trace`]'s `resumed` term: it is the only
    /// assertion in the crate that can tell `2` from `0` there, because on every
    /// non-halting path the term is unobservable — the framed constant is returned
    /// instead and the refill's 2 accesses are already inside it.
    ///
    /// # Extrapolated
    ///
    /// Both rows. Vector 9 is fetched 0 times in 317,500 cases and 0 cases halt; the
    /// controls for those two zeros are the 158,894 cases that enter with T=1 and the
    /// 55,606/55,606 odd stacked fault addresses respectively. The *access counts*
    /// are derived from the asserted bus log; only the idle term is extrapolated.
    #[test]
    fn a_halted_trace_entry_is_charged_for_its_resume_refill_and_no_more() {
        // Row 1: a NOP executed with T set, then the trace entry halts.
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E71, 0x4E71]);
        bus.put16(0x0024, 0x0000); // vector 9, so a frame would be visible
        bus.put16(0x0026, 0x2000);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | SR_T | 0x0700;
        cpu.a[7] = 0x2FFF; // odd: the frame base, so the trace entry halts
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert!(cpu.trace_pending, "the NOP ran with T set");
        bus.log.clear();

        let cycles = cpu.step_with(&dec, &mut bus);
        assert!(cpu.halted);
        assert_eq!(bus.log, vec![], "zero accesses: no frame, no vector fetch");
        assert_eq!(
            cycles, HALTED_IDLE_CYCLES,
            "4 × 0 accesses + the halt idle — not the framed 34"
        );

        // Row 2: the same halt reached through a STOP resume, which does access the
        // bus before the entry.
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E72, 0xA700, 0x4E71]); // STOP #$A700: sets T
        bus.put16(0x0024, 0x0000);
        bus.put16(0x0026, 0x2000);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | 0x0700;
        cpu.a[7] = 0x2FFF;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        cpu.step_with(&dec, &mut bus);
        assert!(cpu.stopped && cpu.trace_pending);
        bus.log.clear();

        let cycles = cpu.step_with(&dec, &mut bus);
        assert!(cpu.halted);
        assert_eq!(
            bus.log.len(),
            2,
            "the resume refill happened; the frame and vector did not"
        );
        assert!(
            bus.writes().is_empty(),
            "a halt writes no frame, so the refill is all of it"
        );
        assert_eq!(
            cycles,
            4 * 2 + HALTED_IDLE_CYCLES,
            "the 2 refill accesses are charged and nothing else is"
        );
    }

    /// [`check_interrupts`]'s halt path, the third site of the same defect — and the
    /// one where the cost and the *state* both need pinning.
    ///
    /// The interrupt entry does two things `take` cannot undo: it clears `stopped`
    /// and refills the queue (2 accesses) *before* the entry, and it raises the SR
    /// mask *after*. On a halt the accesses have happened and must be charged; the
    /// mask raise must not happen at all, because no frame holds the old mask and
    /// there is no handler to run at the new one.
    ///
    /// # Extrapolated
    ///
    /// The halt itself: 0 of 317,500 cases halt, with the odd stacked fault address
    /// (55,606/55,606) as the control for that zero. The access count is derived from
    /// the asserted bus log; [`HALTED_IDLE_CYCLES`] is the extrapolated term.
    #[test]
    fn a_halted_interrupt_entry_charges_its_refill_and_leaves_the_mask_alone() {
        let mut bus = RecordingBus::new();
        // STOP #$2100: mask 1, deliberately *below* the level raised next, so that
        // "raise the mask to the serviced level" is an observable change rather than
        // a no-op. A mask of 7 would hide the mutant.
        bus.load(0x1000, &[0x4E72, 0x2100, 0x4E71]);
        bus.put16(0x0074, 0x0000); // vector 29 (level 5), so a frame would be visible
        bus.put16(0x0076, 0x2000);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | 0x0700;
        cpu.a[7] = 0x2FFF; // odd: the frame base, so the entry halts
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert!(cpu.stopped);
        assert_eq!(
            cpu.sr & 0x0700,
            0x0100,
            "the STOP immediate lowered the mask"
        );
        cpu.set_irq(5);
        let sr_before = cpu.sr;
        bus.log.clear();

        let cycles = cpu.step_with(&dec, &mut bus);

        assert!(cpu.halted);
        assert_eq!(
            bus.log.len(),
            2,
            "the resume refill happened; the frame and vector did not"
        );
        assert!(bus.writes().is_empty(), "a halt writes no frame");
        assert_eq!(
            cycles,
            4 * 2 + HALTED_IDLE_CYCLES,
            "the 2 refill accesses are charged — not the framed 44"
        );
        assert_eq!(
            cpu.sr, sr_before,
            "the serviced level is not masked on a halt: nothing stacked the old one"
        );
    }
}
