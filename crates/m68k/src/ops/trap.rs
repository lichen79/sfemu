//! `TRAP`, `TRAPV` and `RTE` — the three instructions that drive exception
//! entry and exit from the program's own side.
//!
//! Everything here is measured against the SingleStepTests vectors, 7,500 cases
//! across the three groups, and every claim in the doc comments below is a whole
//! bucket rather than a majority. The three groups partition exactly:
//!
//! ```text
//! TRAP   2500  one shape, ".wwwrrr.r"  34 cycles
//! TRAPV  1250  V=0  "r"                 4      +  1250  V=1  "rwwwrrr.r"  34
//! RTE     600  clean "rrrrr"           20      +  1286  privilege ".wwwrrr.r"  34
//!                                              +   614  address error         70
//! ```
//!
//! # Three group-2 traps, three different stacked PCs
//!
//! `TRAP` and `TRAPV` stack `opcode + 2`; `RTE`'s privilege violation stacks
//! `opcode + 0`; `RTE`'s address error stacks `opcode + 2`. All three write the
//! same 3-word frame at the same 34 cycles with the same access shape, so the
//! shape tables give no warning — the distinction is semantic. A *completed*
//! instruction reports the PC past its own opcode; an instruction *aborted*
//! before execution reports the opcode's own address. That is why
//! [`crate::exception::take`] takes the stacked PC as a parameter and never
//! computes it.
//!
//! # TRAPV advances the queue before stacking; TRAP does not
//!
//! `TRAP`'s shape opens `.` (idle, then straight into the frame writes);
//! `TRAPV`'s opens `r`. So TRAPV performs one program read that TRAP does not,
//! and pays 4 fewer idle cycles for it — `4×8 + 2` against `4×7 + 6`, both 34.
//! TRAPV is the only outlier **among the paths that cost 34**: TRAPV
//! `Read at pc+0` 1250/1250, while TRAP, LINE-A, LINE-F and the STOP/RESET
//! privilege violations all open `Idle(4)` unanimously (2500, 2500, 2500, 1270,
//! 1267).
//!
//! ⚠️ It is **not** the only group-2 path that opens with a read. `CHK` opens
//! `Read` in 645 of its 1,326 trapping cases and `Idle(2)`/`Idle(8)`/`Idle(10)`
//! in the other 681 — and `CHK` costs 38-52, never 34. An earlier version of
//! this paragraph said "the sole outlier across every group-2 path in the
//! suite"; that quantifier ranges over a category containing `CHK`, which
//! refutes it twice over. The measured claim is about the 34-cycle paths only.
//!
//! ⚠️ **No mechanical explanation for that survives contact with the data.** The
//! attractive story — TRAPV must execute to know V, so its prefetch proceeds —
//! is refuted by TRAP, which also executes to completion, also stacks `+2`, and
//! still opens with idle. The shape is measured; the cause is open. Do not derive
//! either group's shape from the other, and never infer an access split from a
//! cycle total: two different splits reach 34 here.

use crate::cpu::{M68k, SR_MASK, SR_V};
use crate::decode::Handler;
use crate::exception::{
    self, FaultKind, Space, ADDRESS_ERROR_TAIL_CYCLES, SHORT_FRAME_ACCESSES, VEC_PRIVILEGE,
    VEC_TRAPV, VEC_TRAP_BASE,
};
use crate::Bus;

/// Cycles a group-2 trap costs: the short frame's 7 accesses plus 6 idle.
///
/// A singleton at 34 across the **twelve groups that enter at 7 accesses and 6
/// idle** — `TRAP` 2500, LINE-A 2500, LINE-F 2500, and the nine groups whose
/// privilege violations total 11,276 — which is 18,776 cases. `TRAPV`'s 1,250
/// reach the same 34 at `4×8 + 2`; see the module docs. So 20,026 cases sit at
/// 34, in **two** decompositions, and a doc comment citing 20,026 against
/// `4×7 + 6` would be describing only 18,776 of them.
///
/// ⚠️ **Not every group-2 path costs 34.** `CHK` is group 2 and never is: its
/// 1,326 trapping cases run 38/40/42/44/46/48/50/52 across ten `(accesses,
/// idle)` shapes, because it pays for an operand comparison these paths do not
/// make. Selecting on **the vector address a case reads** is what makes that
/// visible — vector `v` at `4v` and `4v+2`, `TRAP` at `0x80..=0xBF`. Do not
/// select trapping cases by an SR supervisor transition: that drops every
/// exception taken from supervisor mode (about half of each group) and admits
/// address errors, which is how `CHK` stayed hidden behind this sentence.
///
/// ⚠️ **`final.ssp == initial.ssp - 6` is not a synonym for "took a short frame",
/// and `CHK` is where the two predicates part.** 1,326 `CHK` cases fetch vector
/// 6 but only 1,281 satisfy the SSP predicate. The missing **45** are all
/// supervisor-mode cases whose `<ea>` names A7 with a side effect — 25 at
/// `-(A7)` and 20 at `(A7)+` — so the EA's own adjustment composes with the
/// frame's −6 and the net delta reads −8 or −4. They took the frame; the
/// arithmetic just does not show it. Every other group is unaffected because
/// none of the 34-cycle paths has an `<ea>` at all. Prefer the vector fetch:
/// it survives an operand that touches the stack pointer.
const GROUP2_CYCLES: u32 = 4 * SHORT_FRAME_ACCESSES + 6;

/// `TRAP #n` — an unconditional trap through vectors 32-47.
///
/// One shape and one cost across all 2,500 cases, with every field a whole
/// bucket:
///
/// ```text
/// cycles / shape                     34, ".wwwrrr.r"   2500/2500
/// vector == 32 + (opcode & 0xF)                        2500/2500  (all 16 used,
///                                                                 127-168 each)
/// stacked PC == opcode address + 2                     2500/2500
/// stacked SR == entry SR                               2500/2500
/// frame base == entry SSP − 6                          2500/2500
/// final PC == UNMASKED vector target + 4               2500/2500  (masked: 0/2500)
/// final SR == (entry SR | S) & !T                      2500/2500
/// ```
///
/// Entry mode splits 1,251 user / 1,249 supervisor and **the shape is identical
/// in both**, so there is no mode-dependent branch: the frame always goes to the
/// SSP, and the 1,251 user cases confirm it with the USP form at 0/1,251 and the
/// USP itself untouched.
///
/// TRAP is unprivileged — that is the whole point of a supervisor call gate — so
/// there is no [`exception::take`]-preceding check here.
///
/// ⚠️ **No queue advance.** The 3 frame writes are the *first* accesses; the
/// stacked `+2` comes from the arithmetic on `opcode_addr`, not from a fetch. An
/// implementation that advances the queue first produces 8 accesses and 38
/// cycles, which the timing law's `(want − got) % 4 == 0` diagnostic identifies
/// as an extra access rather than a wrong constant.
fn trap(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let vector = VEC_TRAP_BASE + (opcode & 0xF) as u8;
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    exception::take(cpu, bus, vector, opcode_addr.wrapping_add(2));
    // No queue advance precedes the frame (see above), so a halted entry logged
    // nothing: 0 accesses.
    exception::entry_cycles(cpu, 0, GROUP2_CYCLES)
}

/// `TRAPV` — trap through vector 7 if V is set, otherwise a no-op.
///
/// The split is on the V flag and it is clean, 1250/1250 each way with no third
/// bucket:
///
/// ```text
/// V=0   1250   4 cycles   shape "r"          the queue advance, and nothing else
/// V=1   1250  34 cycles   shape "rwwwrrr.r"  that same advance, then vector 7
/// ```
///
/// **Both paths perform the advance, before the check.** That is what makes the
/// V=0 path cost exactly 4 (`4×1 + 0` idle) and the V=1 path open with a read
/// where every other group-2 path opens with 4 idle cycles. Writing this as
/// "check V, then fall through to a shared trap-entry helper" gets the trapping
/// path wrong; the two paths share the read and differ only in what follows it.
///
/// On the trapping path, every field is a whole bucket over the 1,250 cases:
/// vector 7 (address `0x1C`) 1250/1250, stacked PC == opcode + 2, stacked SR ==
/// entry SR (**keeping T set** in all 595 T=1 entries, while the final SR clears
/// it), frame base == entry SSP − 6 including the 623 user-mode entries (USP
/// form 0/623, USP untouched), and final PC == unmasked vector target + 4
/// (masked target: 0/1250).
fn trapv(cpu: &mut M68k, bus: &mut dyn Bus, _opcode: u16) -> u32 {
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let trapped = cpu.sr & SR_V != 0;
    // Unconditional, and before the check: the V=0 path is exactly this read.
    cpu.consume_opcode_dyn(bus);
    if !trapped {
        return 4;
    }
    exception::take(cpu, bus, VEC_TRAPV, opcode_addr.wrapping_add(2));
    // One access more than the other group-2 paths, 4 idle cycles fewer:
    // 4*8 + 2 == 4*7 + 6 == 34. See the module docs.
    //
    // TRAPV is the one group-2 path with an access *before* the frame, so a halted
    // entry still owes that leading read: 1 access, not 0.
    exception::entry_cycles(cpu, 1, 4 * (SHORT_FRAME_ACCESSES + 1) + 2)
}

/// `RTE` — return from exception: pop the 3-word frame and resume.
///
/// Three paths, partitioning all 2,500 cases with no residue. The discriminator
/// is the **entry** S bit, whose census is `{false: 1286, true: 1214}` against
/// `600 + 614 = 1214`:
///
/// ```text
/// entry user        1286   34 cycles   privilege violation, vector 8
/// popped PC odd      614   70 cycles   address error at the refill, vector 3
/// otherwise          600   20 cycles   clean return
/// ```
///
/// # The clean path, 600/600 on every row
///
/// ```text
/// pops at {+0, +2, +4} ascending from the entry SSP  (SR lowest)
/// SSP += 6
/// install popped SR & 0xA71F  via  set_sr
/// PC = popped PC, then refill both queue words
/// 20 cycles = 4×5 + 0 idle   (3 pops + 2 refills, no writes, NO idle term)
/// ```
///
/// Three things measured rather than assumed, each with a control:
///
/// - **The pops ascend**, `{+0, +2, +4}` — the *opposite* of the frame write
///   order (`+4, +0, +2`). Reading and writing a frame are not mirror images.
/// - **The SR goes through [`SR_MASK`]**, same as `STOP` and `MOVEtoSR`.
///   Installing the raw word fails 594 of 600; the 6 that pass are the cases
///   where the popped word was already masked. RTE's address-error bucket
///   discriminates this harder — there the SR comes from unconstrained memory,
///   control 0/614.
/// - **[`M68k::set_sr`], not an assignment to `cpu.sr`.** The S bit follows the
///   *popped* SR, and 331 of the fault cases return to user mode; a direct
///   assignment leaves `a[7]` pointing at the supervisor stack.
/// - **Both refills follow all three pops**, 600/600, and read the popped PC and
///   PC+2.
///
/// # Install the SR before the refill — confirmed six independent ways
///
/// The ordering is forced by the 614-case fault bucket: that fault's status word
/// carries an `fc` derived from the **popped** S bit (`{2: 331, 6: 283}`), so the
/// SR is already installed when the faulting access happens. Deriving it from the
/// entry SR gives `fc=6` for all 614 and is wrong 331 times. The same ordering
/// shows up in the frame's pushed T bit (popped T 614/614, entry T 314/614 — the
/// coincidental agreements), the pushed low bits, the status word, and §21's
/// refill-after-pop. An implementation that refills first and installs the SR
/// afterwards passes all 600 clean cases and fails 331 of the 614.
///
/// # The two address errors are opposite, and only one is measured
///
/// | | odd **entry SSP** | odd **popped PC** |
/// |---|---|---|
/// | coverage | 0 cases — extrapolated | **614 — measured** |
/// | what faults | the first pop | the refill after the pops |
/// | pops completed | **none** | **all three** |
/// | `fc` source | entry S bit | the **popped** S bit |
/// | outcome | **HALT, no frame** | vector 3, ordinary frame |
///
/// The first column is `exception::double_bus_fault`'s territory: an odd SSP
/// makes the frame's own base odd, so stacking would fault again. That function
/// holds the consequence for every stack fault on this core; the halt is not
/// repeated here.
fn rte(cpu: &mut M68k, bus: &mut dyn Bus, _opcode: u16) -> u32 {
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let ir = cpu.prefetch[0];

    // Privileged, and the check is *first*: the shape opens with the frame
    // writes and no preceding read, so RTE never touches the stack before
    // discovering it is unprivileged. The stacked PC is the opcode's own address
    // with no bump (1286/1286, `+2` control 0/1286) — the instruction aborted.
    if !cpu.sr_s() {
        exception::take(cpu, bus, VEC_PRIVILEGE, opcode_addr);
        // No access precedes the frame on this path, so an odd frame base leaves
        // the bus log empty and the 34 collapses. See `exception::entry_cycles`.
        return exception::entry_cycles(cpu, 0, GROUP2_CYCLES);
    }

    let sp = cpu.a[7];
    // An odd SSP would fault on the first pop with nothing committed — and per
    // the stack-fault theorem it halts rather than framing, because the frame's
    // own base would be this same odd SSP. Zero suite coverage (odd active SP is
    // 0/317,500); routing it through `address_error` keeps the check here (which
    // owns the address) and the consequence in one place.
    if sp & 1 != 0 {
        exception::address_error(
            cpu,
            bus,
            sp,
            FaultKind::Read,
            Space::Data,
            ir,
            opcode_addr.wrapping_add(2),
        );
        // An odd SSP is *always* a double bus fault in supervisor mode — this arm
        // is unreachable with a written frame, since the frame base is this same
        // odd `a[7]`. So the halt branch is the live one and it logs **zero**
        // accesses: no pop happened (the check is before the bus), no frame, no
        // vector fetch. Returning the full 58 here contradicted that log by 58
        // cycles. `ADDRESS_ERROR_TAIL_CYCLES` is kept as the framed arm rather than
        // deleted because it becomes reachable the moment `frame_base` can differ
        // in parity from `sp`, which is exactly the user-mode case.
        return exception::entry_cycles(cpu, 0, ADDRESS_ERROR_TAIL_CYCLES);
    }

    let new_sr = bus.read16(sp & crate::cpu::ADDR_MASK);
    let hi = bus.read16(sp.wrapping_add(2) & crate::cpu::ADDR_MASK) as u32;
    let lo = bus.read16(sp.wrapping_add(4) & crate::cpu::ADDR_MASK) as u32;
    let new_pc = (hi << 16) | lo;

    // Commit the pops before installing the SR: `set_sr` may swap `a[7]` out to
    // the USP, and the `+6` belongs to the supervisor stack it was popped from.
    cpu.a[7] = sp.wrapping_add(6);
    cpu.ssp = cpu.a[7];
    cpu.set_sr(new_sr & SR_MASK);
    cpu.pc = new_pc;

    // The refill faults on an odd PC. `Space::Program` unconditionally — RTE is
    // in the control-flow family, which is 100% program space — and the `fc`'s S
    // bit now comes from the freshly installed SR, which is the whole reason the
    // ordering above is forced.
    if new_pc & 1 != 0 {
        // The stacked fault address is the RAW popped PC, top byte and all; the
        // bus address is that value masked to 24 bits with bit 0 cleared, which
        // `address_error` does at the `write16` boundary. Do not mask here.
        exception::address_error(
            cpu,
            bus,
            new_pc,
            FaultKind::Read,
            Space::Program,
            ir,
            opcode_addr.wrapping_add(2),
        );
        // 70 = 4×15 + 10: the 3 pops, the aborted access, the 7 frame writes,
        // the 2 vector reads, the 2 refills, and 10 idle. The aborted access
        // counts — 614/614 under the timing law.
        //
        // A halt here needs an odd frame base, which is the post-pop SSP: reachable
        // only from user mode with an odd SSP, since supervisor entry with an odd
        // SSP was already caught above. The 3 pops did reach the bus, so they are
        // still owed.
        return exception::entry_cycles(cpu, 3, 4 * 3 + ADDRESS_ERROR_TAIL_CYCLES);
    }

    cpu.refill_prefetch_dyn(bus);
    // 3 pops + 2 refills, no idle term at all — one of the few instructions
    // where a wrong access count cannot hide behind an idle adjustment.
    4 * 5
}

/// Installs `TRAP #0`-`#15` (`4E40`-`4E4F`), `RTE` (`4E73`) and `TRAPV`
/// (`4E76`).
///
/// These are the last three unclaimed rows of the `0100 1110` line;
/// [`super::system`] owns `4E70`/`4E72` (RESET/STOP) and [`super::branch`] owns
/// `4E75`/`4E77` (RTS/RTR) and `4E80`-`4EFF` (JSR/JMP).
pub fn register(table: &mut [Handler; 65536]) {
    for n in 0..16u16 {
        table[(0x4E40 + n) as usize] = trap;
    }
    table[0x4E73] = rte;
    table[0x4E76] = trapv;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::RecordingBus;
    use crate::cpu::SR_S;
    use crate::decode::Decoder;

    /// `RTE` returning to user mode must install the popped SR through
    /// `set_sr`, so `a[7]` becomes the USP.
    ///
    /// The suite covers this (331 of the 614 fault cases return to user mode),
    /// but only indirectly through a fault's function code; asserting the
    /// register swap directly is cheap and names the mechanism.
    #[test]
    fn rte_to_user_mode_swaps_in_the_usp() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E73, 0x4E71]);
        // Frame on the supervisor stack: SR (user, all flags clear), then PC.
        bus.put16(0x3000, 0x0000);
        bus.put16(0x3002, 0x0000);
        bus.put16(0x3004, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000;
        cpu.usp = 0x8000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert_eq!(cycles, 20);
        assert_eq!(cpu.sr, 0x0000, "popped SR installed, S cleared");
        assert_eq!(cpu.a[7], 0x8000, "a[7] is now the USP");
        assert_eq!(cpu.ssp, 0x3006, "the SSP kept the +6 from the pops");
        assert_eq!(cpu.pc, 0x2004, "refilled at the popped PC");
    }

    /// An odd **entry SSP** halts instead of stacking a frame, and the odd
    /// access never reaches the bus.
    ///
    /// # Extrapolated
    ///
    /// Zero suite cases: no case has an odd active stack pointer (0/317,500), so
    /// nothing here is measured. The expectation is extrapolated from the
    /// stack-fault theorem — every stack offset is even, so an odd SSP makes the
    /// address-error frame's own base odd, and stacking it would fault again.
    /// That is a double bus fault, which halts. The values asserted below are
    /// what the *contract* gives (`exception.rs`'s "the faulting access must not
    /// have happened", and "no frame"), not an extrapolated frame layout — there
    /// is no frame to have one.
    ///
    /// **The cycle count is part of the contract**, and its earlier absence here
    /// is how this path kept returning `ADDRESS_ERROR_TAIL_CYCLES` — 58, `4 × 12 +
    /// 10`, paying for an aborted access, 7 frame writes, 2 vector reads and 2
    /// refills — while performing none of them. The access count below is derived
    /// from the bus log asserted next to it; only the idle term is extrapolated,
    /// via [`exception::HALTED_IDLE_CYCLES`].
    #[test]
    fn rte_with_an_odd_ssp_halts_and_writes_nothing() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E73, 0x4E71]);
        bus.put16(0x000C, 0x0000); // vector 3, so a frame would be visible
        bus.put16(0x000E, 0x2000);

        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3001;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert!(cpu.halted, "an odd SSP is a double bus fault");
        assert_eq!(bus.writes(), vec![], "no frame was written");
        assert_eq!(bus.reads(), vec![], "the odd pop never reached the bus");
        assert_eq!(
            cycles,
            exception::HALTED_IDLE_CYCLES,
            "4 × 0 accesses + the halt idle. The full 58 would be a 58-cycle \
             claim about a step with an empty bus log"
        );
        assert_eq!(cpu.a[7], 0x3001, "nothing was committed");
        assert_eq!(cpu.sr, SR_S, "the SR is untouched");
        assert_eq!(cpu.pc, 0x1004, "the PC is untouched");
    }

    /// `TRAPV` with V clear costs 4 cycles and one program read — the queue
    /// advance, and nothing else.
    #[test]
    fn trapv_with_v_clear_advances_the_queue_only() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E76, 0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert_eq!(cycles, 4);
        assert_eq!(bus.reads(), vec![(0x1004, 0x4E71)], "one read, at pc");
        assert_eq!(bus.writes(), vec![], "no frame");
        assert_eq!(cpu.a[7], 0x3000, "the stack is untouched");
    }

    /// `TRAPV` with V set reads *before* it stacks — the one group-2 path that
    /// does, and the reason it cannot share TRAP's entry sequence.
    #[test]
    fn trapv_with_v_set_reads_before_stacking() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E76, 0x4E71, 0x4E71]);
        bus.put16(0x001C, 0x0000); // vector 7
        bus.put16(0x001E, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S | SR_V;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert_eq!(cycles, 34);
        // (is_write, addr, val) in order: the read comes first.
        assert!(!bus.log[0].0, "the first transaction is a read");
        assert_eq!(bus.log[0].1, 0x1004, "…at the entry PC");
        assert_eq!(
            bus.writes(),
            vec![(0x2FFE, 0x1002), (0x2FFA, SR_S | SR_V), (0x2FFC, 0x0000)],
            "then the frame: PC.lo, SR, PC.hi — stacked PC is opcode + 2"
        );
        assert_eq!(cpu.pc, 0x2004);
    }

    /// `TRAP #n` uses vector `32 + n` and stacks `opcode + 2`, with no leading
    /// read.
    ///
    /// ⚠️ The base `32` below is an **independent literal**, not [`VEC_TRAP_BASE`].
    /// Deriving the handler address from the constant under test makes the
    /// assertion self-consistent for any constant value — the handler moves with
    /// the mutation and the test still passes — which is how a wrong
    /// `VEC_TRAP_BASE` stayed invisible to the test named for it. Measured:
    /// `VEC_TRAP_BASE = 33` is killed here now; in the derived form it was killed
    /// only incidentally, by an `exception.rs` trace test that happens to hard-code
    /// `0x4004` for an unrelated reason.
    ///
    /// Keep the literal. See `exception::tests::line_f_and_plain_illegal_use_their_own_vectors`,
    /// which had the same defect for `VEC_LINE_F`.
    #[test]
    fn trap_uses_vector_32_plus_n_and_stacks_opcode_plus_2() {
        for n in [0u16, 7, 15] {
            let mut bus = RecordingBus::new();
            bus.load(0x1000, &[0x4E40 + n, 0x4E71]);
            let vaddr = (32 + n as u32) * 4;
            bus.put16(vaddr, 0x0000);
            bus.put16(vaddr + 2, 0x2000);
            bus.load(0x2000, &[0x4E71, 0x4E71]);

            let mut cpu = M68k::new();
            cpu.sr = SR_S;
            cpu.a[7] = 0x3000;
            cpu.pc = 0x1000;
            cpu.prime_prefetch(&mut bus);
            bus.log.clear();

            let cycles = cpu.step_with(&Decoder::new(), &mut bus);

            assert_eq!(cycles, 34, "TRAP #{n}");
            assert!(bus.log[0].0, "TRAP #{n}: the first transaction is a write");
            assert_eq!(
                bus.writes(),
                vec![(0x2FFE, 0x1002), (0x2FFA, SR_S), (0x2FFC, 0x0000)],
                "TRAP #{n}: stacked PC is opcode + 2"
            );
            assert_eq!(cpu.pc, 0x2004, "TRAP #{n}: vector {}", 32 + n);
        }
    }

    /// A user-mode `TRAP` puts its frame on the SSP and leaves the USP alone.
    #[test]
    fn trap_from_user_mode_frames_on_the_ssp() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E4F, 0x4E71]);
        bus.put16(0x00BC, 0x0000); // vector 47
        bus.put16(0x00BE, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = 0x0000;
        cpu.a[7] = 0x8000; // the USP, since S is clear
        cpu.ssp = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        cpu.step_with(&Decoder::new(), &mut bus);

        assert_eq!(
            bus.writes(),
            vec![(0x2FFE, 0x1002), (0x2FFA, 0x0000), (0x2FFC, 0x0000)],
            "the frame is on the SSP, not the USP"
        );
        assert_eq!(cpu.usp, 0x8000, "the USP is untouched");
        assert_eq!(cpu.a[7], 0x2FFA, "a[7] is now the supervisor stack");
    }

    /// `RTE` from user mode is a privilege violation stacking the opcode's own
    /// address — `+0`, where TRAP and TRAPV stack `+2`.
    #[test]
    fn rte_from_user_mode_stacks_the_opcode_address_itself() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E73, 0x4E71]);
        bus.put16(0x0020, 0x0000); // vector 8
        bus.put16(0x0022, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = 0x0000;
        cpu.a[7] = 0x8000;
        cpu.ssp = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert_eq!(cycles, 34);
        assert!(bus.log[0].0, "no read precedes the frame");
        assert_eq!(
            bus.writes(),
            vec![(0x2FFE, 0x1000), (0x2FFA, 0x0000), (0x2FFC, 0x0000)],
            "stacked PC is the opcode address, with no bump"
        );
    }

    /// `RTE` to an odd PC faults on the refill *after* all three pops, with the
    /// function code taken from the popped S bit.
    #[test]
    fn rte_to_an_odd_pc_faults_after_the_pops() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E73, 0x4E71]);
        bus.put16(0x3000, 0x0000); // popped SR: user mode
        bus.put16(0x3002, 0x0000);
        bus.put16(0x3004, 0x2001); // odd PC
        bus.put16(0x000C, 0x0000); // vector 3
        bus.put16(0x000E, 0x4000);
        bus.load(0x4000, &[0x4E71, 0x4E71]);

        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000;
        cpu.usp = 0x8000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert_eq!(cycles, 70, "3 pops + the aborted access + the long frame");
        // The frame base is the post-pop SSP, 0x3006, so the 7 writes run from
        // 0x3004 down to 0x2FF8.
        let writes = bus.writes();
        assert_eq!(writes.len(), 7, "the 7-word frame");
        // Status word: IR 0x4E73 & 0xFFE0, read, program space, popped S clear.
        assert_eq!(
            writes[5],
            (0x2FF8, 0x4E60 | 0x10 | 0x2),
            "fc comes from the POPPED S bit: program space, user"
        );
        // The stacked fault address is the raw popped PC.
        assert_eq!(writes[4], (0x2FFC, 0x2001), "fault address low word");
        assert_eq!(writes[6], (0x2FFA, 0x0000), "fault address high word");
        // The aborted access is absent from the log.
        assert!(
            !bus.reads().iter().any(|(a, _)| *a == 0x2000),
            "the faulting refill never reached the bus"
        );
        assert_eq!(cpu.pc, 0x4004);
    }
}
