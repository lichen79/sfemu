//! System control, multi-register transfer, and the address-only instructions.
//!
//! Nineteen suite groups whose only shared property is that none of them fits
//! [`alu`](super::alu)'s single-`<ea>` schedule. Four schedules live here:
//!
//! ```text
//! 1. fixed-cost, no operand      SWAP EXT EXG STOP RESET MOVE USP
//! 2. address-only                LEA PEA LINK UNLK
//! 3. multi-word transfer         MOVEM.w/.l  MOVEP.w/.l
//! 4. rewind-and-refill           MOVEtoCCR MOVEtoSR   (and MOVEfromSR, which
//!                                                      is alu's schedule)
//! ```
//!
//! Everything below is measured against the SingleStepTests vectors, group by
//! group; the counts in each doc comment are the sample the rule rests on.
//!
//! # Privilege
//!
//! Privileged instructions cost **34 cycles** to trap —
//! [`exception::SHORT_FRAME_ENTRY_CYCLES`], `4 × 7 accesses + 6` idle, whose docs
//! name the two other things that reach 34 by a different split and the one
//! group-2 instruction that never reaches it at all. The stacked PC is the
//! **opcode address** with no bump, and the
//! queue is not advanced: a privilege violation aborts the instruction before
//! its own fetch. Measured 6,260/6,260 over the **five groups this module owns**
//! — `MOVEtoSR` 1290, `MOVEtoUSP` 1226, `MOVEfromUSP` 1207, `STOP` 1270,
//! `RESET` 1267.
//!
//! ⚠️ **6,260 is those five groups, not the whole privilege story.** An earlier
//! version of this paragraph called 34 "a singleton across all six" while
//! enumerating five and citing a count that covers exactly those five. Nine
//! suite groups fetch vector 8, totalling **11,276** cases: the five above, plus
//! `RTE` 1286 (Task 11's), plus `ORItoSR` 1301, `ANDItoSR` 1207 and `EORItoSR`
//! 1222, which live in [`logic`](super::logic). All nine cost 34 at 7 accesses
//! and 6 idle, so the *cost* claim generalises — but the count does not, and
//! "all six" was never a set anyone enumerated. Count privilege violations by the
//! **vector-8 fetch** (a read at `0x20` or `0x22`), never by an SR supervisor
//! transition: that predicate silently drops every violation raised while
//! already in supervisor mode.
//!
//! ⚠️ **`MOVEfromSR` and `MOVEtoCCR` are NOT privileged on the 68000** (they are
//! on the 68010). Measured: 0 of 747 user-mode `MOVEfromSR` cases and 0 of 756
//! user-mode `MOVEtoCCR` cases trap, against a control of 1290/1290 for
//! `MOVEtoSR`. Do not "fix" this by adding a check — the suite is the authority
//! and it is unambiguous.
//!
//! # Where the cycle counts come from
//!
//! Nothing here carries a hand-written cycle table. Every count below is
//! `4 * accesses + idle` under the timing law, with the idle stated as a
//! constant per mode, and each was cross-checked against the measured
//! per-`(mode, reg)` singleton buckets in the addendum. The two-cycle
//! address-formation lead for `-(An)`, `(d8,An,Xn)` and `(d8,PC,Xn)` is the same
//! `idle_lead` [`alu`](super::alu) charges, reproduced here as `idle_lead`
//! because this module does not route through `alu::run`.

use crate::cpu::{M68k, ADDR_MASK};
use crate::ea::{self, mode_is_mem, Ea, Size};
use crate::exception::{self, FaultKind, Space, ADDRESS_ERROR_TAIL_CYCLES, VEC_PRIVILEGE};
use crate::flags::logic_flags;
use crate::Bus;

/// Cycles a privilege violation costs: the short frame plus 6 idle.
///
/// Singleton at 34 across all **nine** suite groups that fetch vector 8,
/// 11,276 cases; 6,260 of those are this module's five. See the module docs.
const PRIVILEGE_CYCLES: u32 = exception::SHORT_FRAME_ENTRY_CYCLES;

/// Traps to vector 8 if the CPU is in user mode, returning the cycle cost.
///
/// The stacked PC is the opcode address itself and the prefetch queue is left
/// alone, because the instruction never reached its own fetch. Measured
/// **11,276/11,276 with zero bump** across all nine vector-8 groups — not just
/// the 6,260 this function serves, since [`exception::take`] stacks whatever PC
/// it is handed and `logic.rs`'s three `to SR` forms and `trap.rs`'s `RTE` pass
/// the same opcode address. 0 of 11,276 stack `+2`, so the rule is a whole
/// bucket over every group that can reach it, not over this module's share of
/// them.
fn privilege_check(cpu: &mut M68k, bus: &mut dyn Bus) -> Option<u32> {
    if cpu.sr_s() {
        return None;
    }
    let pc = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    exception::take(cpu, bus, VEC_PRIVILEGE, pc);
    // The queue is untouched before the frame, so a double bus fault here logged
    // nothing at all: 0 accesses. See `exception::entry_cycles`.
    Some(exception::entry_cycles(cpu, 0, PRIVILEGE_CYCLES))
}

/// Idle cycles spent forming the address, before any bus access.
///
/// The same rule as [`alu`](super::alu)'s: `-(An)`, `(d8,An,Xn)` and
/// `(d8,PC,Xn)` each cost a 2-cycle internal step.
///
/// ⚠️ **[`lea`] and [`pea`] charge this TWICE**, and that is not a fudge —
/// `LEA (d8,A0,D0),A1` logs `i2 P i2 P`, two separate 2-cycle idles bracketing
/// the two fetches, for 4 in total. The harness compares idle only in aggregate,
/// so the doubling is visible as a cycle count 2 over on exactly the three
/// indexed modes and nowhere else. Every other group here charges it once.
#[inline]
fn idle_lead(mode: u16, reg: u16) -> u32 {
    if mode == 4 || mode == 6 || (mode == 7 && reg == 3) {
        2
    } else {
        0
    }
}

/// Address-error check: word and long accesses fault on an odd address.
#[inline]
fn misaligned(addr: u32) -> bool {
    addr & 1 != 0
}

/// The SR value `MOVE <ea>,CCR` installs: bits **4..0** from the operand, every
/// bit above them kept.
///
/// ⚠️ **This exists as a named function so that the rule can be asserted
/// directly, and that is the entire point of extracting it.** Written inline as
/// `(sr & 0xFF00) | (val & 0x00FF)` — a *byte* mask, which is the weaker claim —
/// it behaves identically, because [`M68k::set_sr`] masks with
/// `SR_MASK = 0xA71F` and clears bits 7-5 a second time. So the weaker mask is
/// *recovered*, not correct.
///
/// The consequence is that **no test reading `cpu.sr` can tell the two apart**,
/// no matter what operand it chooses: the difference is destroyed by the mask
/// downstream of it. A test written that way looks discriminating and is not —
/// verified by mutating this function back to the byte form, under which all 206
/// unit tests and 127/127 suite groups still pass. The suite cannot help either:
/// 0 of 317,500 initial SRs have bits 5-7 set.
///
/// Asserting the returned value is the only way the rule is checked at all. This
/// is the same shape as `Plan::writes` in [`alu`](super::alu): an intent that
/// nothing read, made checkable by giving it a name.
#[inline]
fn move_to_ccr_value(sr: u16, val: u16) -> u16 {
    (sr & 0xFFE0) | (val & 0x001F)
}

/// Program space iff the mode is PC-relative; data space otherwise.
///
/// Confirmed from the address-error status word's function-code bits: MOVEM's
/// `(d16,PC)` and `(d8,PC,Xn)` faults carry `fc` 2 (user) or 6 (supervisor),
/// against 1/5 for every data-space mode.
#[inline]
fn operand_space(mode: u16, reg: u16) -> Space {
    if mode == 7 && (reg == 2 || reg == 3) {
        Space::Program
    } else {
        Space::Data
    }
}

/// Writes a long as two word accesses, low word first.
///
/// Used by the descending `MOVEM -(An)` transfer, where each long's two word
/// writes are logged low-word-first. The resulting memory image is ordinary
/// big-endian; only the transaction order differs, which is why getting this
/// wrong leaves RAM correct and fails only the access-sequence check.
fn write_long_desc(bus: &mut dyn Bus, addr: u32, val: u32) {
    let a = addr & ADDR_MASK;
    bus.write16(a.wrapping_add(2) & ADDR_MASK, val as u16);
    bus.write16(a, (val >> 16) as u16);
}

/// Writes a long as two word accesses, high word first.
fn write_long_asc(bus: &mut dyn Bus, addr: u32, val: u32) {
    let a = addr & ADDR_MASK;
    bus.write16(a, (val >> 16) as u16);
    bus.write16(a.wrapping_add(2) & ADDR_MASK, val as u16);
}

/// Keeps `usp`/`ssp` in step with `a[7]` after a handler moves the stack.
///
/// Belt-and-braces only — not load-bearing for harness correctness. The
/// harness reads the active pointer directly from `a[7]` and the inactive one
/// from `usp`/`ssp`; `set_sr` already saves the outgoing `a[7]` into the
/// right slot before loading the incoming one, so a handler that leaves the
/// shadow stale and then changes the S bit still ends up correct. This call
/// keeps the shadow coherent for debugging and save-state purposes only.
#[inline]
fn sync_sp(cpu: &mut M68k) {
    if cpu.sr_s() {
        cpu.ssp = cpu.a[7];
    } else {
        cpu.usp = cpu.a[7];
    }
}

// ---------------------------------------------------------------------------
// 1. Fixed-cost, no operand access
// ---------------------------------------------------------------------------

/// `SWAP Dn` — exchange the halves of a data register. 4 cycles, 2500/2500.
///
/// N and Z come from the **full 32-bit result**, not from either half; V and C
/// clear and X is preserved. `rotate_right(16)` is the whole operation.
fn swap(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let r = (opcode & 7) as usize;
    let v = cpu.d[r].rotate_right(16);
    cpu.d[r] = v;
    let (n, z, vf, c) = logic_flags(v, Size::Long);
    cpu.set_ccr(cpu.ccr_x(), n, z, vf, c);
    cpu.consume_opcode_dyn(bus);
    4
}

/// `EXT.w Dn` — sign-extend byte to word. 4 cycles, 2500/2500.
///
/// The **upper 16 bits are left alone**: this is a word-sized result written
/// into a data register, so it obeys the ordinary partial-register rule. N and Z
/// come from the 16-bit result.
fn ext_w(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let r = (opcode & 7) as usize;
    let v = (cpu.d[r] as u8) as i8 as i16 as u16;
    cpu.d[r] = (cpu.d[r] & 0xFFFF_0000) | v as u32;
    let (n, z, vf, c) = logic_flags(v as u32, Size::Word);
    cpu.set_ccr(cpu.ccr_x(), n, z, vf, c);
    cpu.consume_opcode_dyn(bus);
    4
}

/// `EXT.l Dn` — sign-extend word to long. 4 cycles, 2500/2500.
fn ext_l(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let r = (opcode & 7) as usize;
    let v = (cpu.d[r] as u16) as i16 as i32 as u32;
    cpu.d[r] = v;
    let (n, z, vf, c) = logic_flags(v, Size::Long);
    cpu.set_ccr(cpu.ccr_x(), n, z, vf, c);
    cpu.consume_opcode_dyn(bus);
    4
}

/// `EXG Rx,Ry` — exchange two registers. 6 cycles (4 + 2 idle), 2500/2500.
///
/// The CCR is untouched. Three opmodes in bits 7-3, and the register *files* are
/// selected by the opmode rather than by an addressing mode — census 8×834,
/// 9×828, 17×838:
///
/// ```text
///   01000  Dx <-> Dy
///   01001  Ax <-> Ay
///   10001  Dx <-> Ay
/// ```
fn exg(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let rx = ((opcode >> 9) & 7) as usize;
    let ry = (opcode & 7) as usize;
    // A7 lives in a[7] with usp/ssp shadowing it, so read and write through
    // a[7] and re-sync afterwards rather than special-casing register 7.
    match (opcode >> 3) & 0x1F {
        0b01000 => cpu.d.swap(rx, ry),
        0b01001 => {
            cpu.a.swap(rx, ry);
            sync_sp(cpu);
        }
        _ => {
            // 10001: Dx <-> Ay. Across two arrays, so `slice::swap` does not
            // apply and `mem::swap` would need two disjoint borrows.
            //
            // `_` rather than `0b10001` because only three opmodes are installed
            // (see `install`: 0xC140, 0xC148, 0xC188), so no other value can arrive
            // — but the arm asserts rather than trusting that, for the same reason
            // as `branch::jump_idle`'s. A mis-installed opmode reaching here would
            // perform a *plausible* register exchange instead of an obviously wrong
            // one, turning a dispatch-table error into a silently corrupted
            // register file that no cycle count or fault would flag.
            debug_assert_eq!(
                (opcode >> 3) & 0x1F,
                0b10001,
                "exg reached opmode {:05b} (opcode {opcode:04X}): only 01000, 01001 \
                 and 10001 are installed, so the dispatch table is wrong",
                (opcode >> 3) & 0x1F
            );
            core::mem::swap(&mut cpu.d[rx], &mut cpu.a[ry]);
            sync_sp(cpu);
        }
    }
    cpu.consume_opcode_dyn(bus);
    4 + 2
}

/// `MOVE An,USP` — privileged, 4 cycles, 1274/1274.
///
/// Encoded at `4E60`-`4E67`, i.e. the `4E` selector's mode field 4. The USP is
/// the *inactive* pointer here (we are in supervisor mode), so this writes
/// `cpu.usp` and leaves `a[7]` alone.
fn move_to_usp(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    if let Some(c) = privilege_check(cpu, bus) {
        return c;
    }
    cpu.usp = cpu.a[(opcode & 7) as usize];
    cpu.consume_opcode_dyn(bus);
    4
}

/// `MOVE USP,An` — privileged, 4 cycles, 1293/1293. Encoded at `4E68`-`4E6F`.
fn move_from_usp(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    if let Some(c) = privilege_check(cpu, bus) {
        return c;
    }
    let r = (opcode & 7) as usize;
    cpu.a[r] = cpu.usp;
    // `MOVE USP,A7` would overwrite the active SP; keep the shadow consistent.
    sync_sp(cpu);
    cpu.consume_opcode_dyn(bus);
    4
}

/// `STOP #imm` — privileged, 4 cycles, and **zero bus accesses**, 1230/1230.
///
/// ⚠️ The immediate comes out of `prefetch[1]` **without a fetch**. The PC does
/// not move (`dpc == 0` in all 1,230 supervisor cases) and the prefetch queue is
/// unchanged, so the 4 cycles are 4 cycles of *idle*. Anything that calls
/// `fetch_word_dyn` here emits a bus read the hardware does not perform and
/// advances the PC by 2 — two diffs from one line.
///
/// This is consistent: `STOP` leaves the CPU waiting with the next instruction
/// still queued, and it is the *interrupt* that resumes and re-fetches. The 4
/// cycles are the cost of entering the state, not of being in it.
///
/// The new SR is the immediate through `SR_MASK` (1230/1230), which `set_sr`
/// applies — and which also performs the USP/SSP swap if the immediate clears S.
///
/// # An immediate that sets T traces instead of stopping
///
/// `STOP #$A700` is the one place a *loaded* SR can owe a trace that the
/// start-of-instruction latch in [`M68k::step_with`] cannot have seen: T was clear
/// when the `STOP` began. The manual's `STOP` entry makes the trace happen anyway,
/// and [`crate::exception::take_trace`] clears `stopped`, so the CPU vectors and
/// resumes rather than staying stopped.
///
/// This is raised here rather than by re-reading the final T at the boundary,
/// because reading the final T is wrong for every *other* instruction — see
/// [`M68k::trace_pending`]. It is an addition to the latch, never a replacement:
/// `|=`, so a `STOP` entered with T=1 whose immediate clears T still traces.
///
/// ⚠️ Without this, `STOP #$A700` wedges permanently on a core that clears
/// `stopped` only from the trace path: nothing owes the trace, so nothing resumes.
/// With the *previous* end-of-instruction sampling it wedged for the opposite
/// reason — the trace fired but left `stopped` set, leaving the CPU both in a
/// handler and stopped, a state with no hardware counterpart.
///
/// # Extrapolated: zero suite coverage
///
/// All 1,230 supervisor cases run one instruction and stop there, so no case
/// observes what the immediate's T bit does next; vector 9 is fetched 0 times in
/// all 317,500. The rule is the manual's, not a measurement.
fn stop(cpu: &mut M68k, bus: &mut dyn Bus, _opcode: u16) -> u32 {
    if let Some(c) = privilege_check(cpu, bus) {
        return c;
    }
    let imm = cpu.prefetch[1];
    cpu.set_sr(imm);
    cpu.trace_pending |= cpu.sr & crate::cpu::SR_T != 0;
    cpu.stopped = true;
    4
}

/// `RESET` — privileged, 132 cycles, 1233/1233.
///
/// One bus access (the queue advance) plus **128 idle** cycles: the instruction
/// holds RESET asserted for 124 clocks and this core models that as idle time
/// rather than signalling peripherals, since the vectors observe only the count.
/// The SR is unchanged (1233/1233).
fn reset(cpu: &mut M68k, bus: &mut dyn Bus, _opcode: u16) -> u32 {
    if let Some(c) = privilege_check(cpu, bus) {
        return c;
    }
    cpu.consume_opcode_dyn(bus);
    4 + 128
}

// ---------------------------------------------------------------------------
// 2. Address-only: LEA, PEA, LINK, UNLK
// ---------------------------------------------------------------------------

/// Fetches `de` extension words from the queue into `ext`, in instruction order.
///
/// Each word is taken from `prefetch[1]` and then the queue is advanced, which is
/// what puts these fetches at the right point in the access sequence. Reading
/// `prefetch[1]` *after* the advance would take the following instruction's word.
fn fetch_ext(cpu: &mut M68k, bus: &mut dyn Bus, de: u32, ext: &mut [u16; 2]) {
    for slot in ext.iter_mut().take(de as usize) {
        *slot = cpu.prefetch[1];
        cpu.consume_opcode_dyn(bus);
    }
}

/// Resolves an `<ea>` for its **address**, fetching extension words as it goes.
///
/// Returns the address and the number of extension words consumed. The
/// extension words are read from the prefetch queue in instruction order, which
/// is what puts the fetches at the right point in the access sequence.
fn resolve_address(cpu: &mut M68k, bus: &mut dyn Bus, mode: u16, reg: u16) -> (u32, u32) {
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let de = ea::ext_words(mode, reg, Size::Word);
    let mut ext = [0u16; 2];
    fetch_ext(cpu, bus, de, &mut ext);
    // The PC-relative base is the address of the first extension word, which for
    // these instructions is `opcode + 2`. MOVEM's is `opcode + 4` because its
    // mask word sits in between — see `movem`, which passes its own base.
    let ea = ea::resolve(
        cpu,
        mode,
        reg,
        Size::Word,
        &ext,
        opcode_addr.wrapping_add(2),
    );
    let Ea::Mem(addr) = ea else {
        unreachable!("LEA/PEA/MOVEM only accept memory addressing modes")
    };
    (addr, de)
}

/// `LEA <ea>,An` — load the effective address, 2500/2500.
///
/// ⚠️ **`LEA` must not read its operand.** The access log holds nothing but
/// program fetches in all 2,500 cases: `(An)` is a single fetch and 4 cycles.
/// Routing this through `ea::read` would add a data access the vectors do not
/// have, and would also make `LEA (A0),A1` able to address-error — which it
/// cannot, and does not in any case.
///
/// Cycles: `4 * (1 + ext) + idle_lead`, giving `(An)` 4, `(d16,An)` 8,
/// `(d8,An,Xn)` 12, `(xxx).w` 8, `(xxx).l` 12, `(d16,PC)` 8, `(d8,PC,Xn)` 12.
/// The CCR is untouched.
fn lea(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let dst = ((opcode >> 9) & 7) as usize;
    let (addr, de) = resolve_address(cpu, bus, mode, reg);
    cpu.a[dst] = addr;
    // LEA into A7 is legal; keep the shadow SP consistent.
    sync_sp(cpu);
    cpu.consume_opcode_dyn(bus);
    4 * (1 + de) + 2 * idle_lead(mode, reg)
}

/// `PEA <ea>` — push the effective address as a long, 2500/2500.
///
/// The long is written **high word first** at the final SP (2500/2500 for
/// lower-address-first), and like `LEA` the operand is never read.
///
/// ⚠️ The queue advance's position depends on the mode, and this is the one
/// place in the group where a uniform schedule is wrong. For the two absolute
/// modes the writes sit **between** the extension fetches and the queue advance,
/// so the advance comes last; for every other mode the advance already happened
/// as part of resolving the address and the writes are last:
///
/// ```text
///   (An)          P W W          12      (d8,An,Xn)  i2 P i2 P W W    20
///   (d16,An)      P P W W        16      (d16,PC)    P P W W          16
///   (xxx).w       P W W P        16      (d8,PC,Xn)  i2 P i2 P W W    20
///   (xxx).l       P P W W P      20
/// ```
///
/// Both orders produce identical memory and identical cycle counts, so this
/// fails *only* the access-sequence check — which is exactly why it is measured
/// rather than assumed. `(xxx).w`'s and `(xxx).l`'s trailing fetch is the
/// opcode's own advance being deferred past the push; the other modes spend
/// theirs on the extension word.
fn pea(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let absolute = mode == 7 && reg <= 1;
    // Capture the opcode address and IR before any queue advance.
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let ir = cpu.prefetch[0];
    let (addr, de) = resolve_address(cpu, bus, mode, reg);
    if !absolute {
        cpu.consume_opcode_dyn(bus);
    }
    let sp = cpu.a[7].wrapping_sub(4);
    if misaligned(sp) {
        // No vector has a PEA address error (0 in all 2,500 cases), so the
        // stacked PC is extrapolated from the neighbouring measured groups:
        // UNLK stacks `opcode + 4` (1115/1115) and LINK's shape is identical.
        // Using the same offset here; label this if hardware evidence changes it.
        //
        // ⚠️ `sp = a[7] - 4` and 4 is even, so misaligned(sp) ⟺ odd `a[7]`. In
        // supervisor mode that makes the frame's own base odd, which is a double
        // bus fault: `exception::double_bus_fault` sees it and halts instead of
        // stacking anything, so the stacked PC above is unreachable there. It is
        // still reachable from user mode, where the frame goes to a possibly-even
        // SSP. The consequence lives in one place; this check owns only the
        // address. Both it and the stacked PC are extrapolated (0 of 2,500).
        exception::address_error(
            cpu,
            bus,
            sp,
            FaultKind::Write,
            Space::Data,
            ir,
            opcode_addr.wrapping_add(4),
        );
        // The lead goes in the caller's own term, per `entry_cycles`' convention:
        // the accesses `resolve_address` and the queue advance already put on the
        // bus (`de` extension fetches, plus the advance for the non-absolute
        // modes), and the lead idle. Measured against the core's own log — `PEA
        // (A0)` with an odd SP logs exactly 1 access and 0 idle.
        //
        // ⚠️ Folding the lead into `entry_cycles`' first argument instead — which
        // is what this did — drops the idle on **both** arms, because
        // `entry_cycles` has no idle term, and drops `4 * made` from the framed
        // arm as well, because 58 is the tail alone. `(d8,An,Xn)` charges 4 of
        // lead idle (`idle_lead` twice, see its docs), so the two spellings differ
        // by 4 there and agree on `(An)` — which is why the mode-2 test could not
        // see it. Under the timing law the total is
        // `4 * (made + 12) + lead_idle + 10`, and the 58 is the `12`-and-`10` part.
        let made = de + u32::from(!absolute);
        return 4 * made
            + 2 * idle_lead(mode, reg)
            + exception::entry_cycles(cpu, 0, ADDRESS_ERROR_TAIL_CYCLES);
    }
    cpu.a[7] = sp;
    sync_sp(cpu);
    write_long_asc(bus, sp, addr);
    if absolute {
        cpu.consume_opcode_dyn(bus);
    }
    4 * (1 + de + 2) + 2 * idle_lead(mode, reg)
}

/// `LINK An,#d` — build a stack frame, 16 cycles, 2500/2500.
///
/// The sequence, and the order matters:
///
/// ```text
///   read An  ->  SP -= 4  ->  write An at SP  ->  An := SP  ->  SP += d
/// ```
///
/// Access shape `P W W P`: the displacement fetch, both writes (high word
/// first), then the queue advance.
///
/// ⚠️ **`LINK A7,#d` pushes the entry SP**, not `SP - 4` — 326/326 for the entry
/// value and 0/326 for the decremented one. Reading `An` *before* the decrement
/// is what produces that, so the A7 case needs no special-casing; writing this
/// as "decrement, then push `a[n]`" breaks 326 cases and nothing else.
///
/// `d` is a **signed** 16-bit displacement and the final addition wraps at 32
/// bits without masking (2174/2174 with wrapping; a first pass that compared
/// unsigned showed 5 phantom outliers).
fn link(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let n = (opcode & 7) as usize;
    // Capture the opcode address and IR before any queue advance.
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let ir = cpu.prefetch[0];
    let disp = cpu.prefetch[1] as i16 as i32 as u32;
    cpu.consume_opcode_dyn(bus);

    let pushed = cpu.a[n];
    let sp = cpu.a[7].wrapping_sub(4);
    if misaligned(sp) {
        // No vector has a LINK address error (0 in all 2,500 cases), so the
        // stacked PC is extrapolated from the neighbouring measured groups:
        // UNLK stacks `opcode + 4` (1115/1115), and LINK's shape is the same.
        // Label this if hardware evidence changes it.
        //
        // ⚠️ Same double-fault reasoning as `pea` above: `4` is even, so an
        // odd `sp` means an odd `a[7]`, and in supervisor mode that is an odd
        // frame base — `exception::double_bus_fault` halts instead of stacking.
        // The check stays here because it owns the address; the consequence is
        // central. Extrapolated (0 faults in 2,500).
        exception::address_error(
            cpu,
            bus,
            sp,
            FaultKind::Write,
            Space::Data,
            ir,
            opcode_addr.wrapping_add(4),
        );
        // The displacement fetch above is the one access already on the bus when a
        // halt aborts the rest; the frame and vector fetch the 58 pays for never
        // happen. See `exception::entry_cycles`.
        return exception::entry_cycles(cpu, 1, ADDRESS_ERROR_TAIL_CYCLES);
    }
    cpu.a[7] = sp;
    write_long_asc(bus, sp, pushed);
    cpu.a[n] = sp;
    cpu.a[7] = sp.wrapping_add(disp);
    sync_sp(cpu);

    cpu.consume_opcode_dyn(bus);
    4 * 4
}

/// `UNLK An` — tear down a stack frame, 12 cycles.
///
/// ```text
///   SP := An  ->  An := pop long  ->  SP += 4
/// ```
///
/// Access shape `R R P`: **both operand reads precede the queue advance**, which
/// is unusual — most single-operand instructions put the advance between the read
/// and the write, and `UNLK` has no write.
///
/// ⚠️ **`UNLK A7` ends with SP holding the popped value** (310/310), because
/// `SP := A7` is a no-op and the pop then overwrites SP outright. Sequencing it
/// as written gives that for free; a special case for A7 breaks it. For
/// `An != 7` the pop address is the old `An` and the final SP is `old An + 4`
/// (1075/1075).
///
/// A misaligned old `An` faults: the address error's fault address is the old
/// `An` in all 1,115 cases, and nothing is committed.
fn unlk(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let n = (opcode & 7) as usize;
    let addr = cpu.a[n];
    if misaligned(addr) {
        // The stacked PC is `opcode + 4` (1115/1115), and the queue has not
        // advanced, so nothing needs rolling back.
        let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
        exception::address_error(
            cpu,
            bus,
            addr,
            FaultKind::Read,
            Space::Data,
            cpu.prefetch[0],
            opcode_addr.wrapping_add(4),
        );
        // Nothing precedes the fault: the check is before the pop and before the
        // queue advance, so a halt logs zero accesses.
        return exception::entry_cycles(cpu, 0, ADDRESS_ERROR_TAIL_CYCLES);
    }
    let hi = bus.read16(addr & ADDR_MASK) as u32;
    let lo = bus.read16(addr.wrapping_add(2) & ADDR_MASK) as u32;
    // Move the SP first, then load `An`. For `An == 7` that ordering is what
    // leaves SP holding the popped value: the load lands last and wins.
    // Incrementing after the load instead gives `popped + 4`.
    cpu.a[7] = addr.wrapping_add(4);
    cpu.a[n] = (hi << 16) | lo;
    sync_sp(cpu);
    cpu.consume_opcode_dyn(bus);
    4 * 3
}

// ---------------------------------------------------------------------------
// 3. MOVEM
// ---------------------------------------------------------------------------

/// Registers a `MOVEM` mask selects, in transfer order.
///
/// For `-(An)` the mask is **reversed**: bit 0 names A7, bit 1 A6, … bit 15 D0.
/// For every other mode bit `b` names register `b` (D0-D7 then A0-A7). Measured
/// by pairing each transfer against the register whose value it carries, using
/// only cases where the selected registers hold distinct values:
///
/// ```text
///   -(An), bit i selects register i          (forward)      0/159   0/157
///   -(An), bit i selects register 15 - i     (REVERSED)   159/159 157/157
///   control: (An), forward mask                           154/154
/// ```
///
/// The control line is what makes this a measurement rather than a coin flip: a
/// probe that always answered "reversed" would have scored 0 on `(An)`.
///
/// ⚠️ This is a reversed **mask**, not a reversed **loop**. Walk the bits
/// ascending in both cases and the A7-first descending order falls out for
/// `-(An)` on its own. Reversing the iteration instead gets the register
/// *values* right and the transfer *order* wrong, which the cycle count cannot
/// see — it depends only on the popcount, and popcount is reversal-invariant.
fn mask_registers(mask: u16, predec: bool) -> impl Iterator<Item = usize> {
    (0..16u16).filter_map(move |b| {
        if mask & (1 << b) == 0 {
            return None;
        }
        Some(if predec { 15 - b as usize } else { b as usize })
    })
}

/// Reads register `i` of the 0-15 `MOVEM` numbering: D0-D7 then A0-A7.
#[inline]
fn movem_read_reg(cpu: &M68k, i: usize) -> u32 {
    if i < 8 {
        cpu.d[i]
    } else {
        cpu.a[i - 8]
    }
}

/// Writes register `i` of the 0-15 `MOVEM` numbering.
///
/// `MOVEM.w` to registers **sign-extends each word to 32 bits** (698/698),
/// including into data registers — so unlike every other word-sized write into a
/// `Dn`, the upper half is *not* preserved. The caller sign-extends before
/// calling, so this is an unconditional 32-bit store.
#[inline]
fn movem_write_reg(cpu: &mut M68k, i: usize, val: u32) {
    if i < 8 {
        cpu.d[i] = val;
    } else {
        cpu.a[i - 8] = val;
        sync_sp(cpu);
    }
}

/// `MOVEM <list>,<ea>` and `MOVEM <ea>,<list>`, both sizes.
///
/// # The schedule
///
/// Measured per `(direction, mode, reg)` over both groups; the shape is uniform
/// once the trailing read is accounted for:
///
/// ```text
///   1        program fetch          (the mask word)
///   ext      program fetches        (the <ea>'s extension words)
///   k        operand transfers      k = popcount * (size/2)
///   [1]      one extra READ         to-registers direction only
///   1        program fetch          (the queue advance)
/// ```
///
/// So `cycles = 4 * (2 + ext + k + extra) + idle_lead`, and `dpc = 2 * (2 + ext)`.
/// That reproduces the addendum's measured base table exactly — `(An)` 8/12,
/// `(d16,An)` 12/16, `(d8,An,Xn)` 14/18, `(xxx).w` 12/16, `(xxx).l` 16/20,
/// `(d16,PC)` −/16, `(d8,PC,Xn)` −/18 — with the `+4`/`+8` per register falling
/// out of the transfer count rather than being added by hand.
///
/// ⚠️ **The to-registers direction always performs one extra discarded word read
/// at `base + count * size`** — 1,445/1,445 across every mode including the
/// PC-relative ones. It is the pipeline reading one word past the list and
/// throwing it away. `(An)+` still advances by only `count * size`, so the extra
/// read does *not* count toward the increment. Omitting it makes every
/// to-registers case short by one access and 4 cycles, and the `(want - got) % 4
/// == 0` diagnostic points at a missing access rather than a wrong constant.
///
/// # `-(An)`
///
/// Descending addresses with the reversed mask (see [`mask_registers`]), and
/// within one long the **low word is written first**.
///
/// ⚠️ **The predecrement register stores its INITIAL value**, not the decremented
/// one: 158/158 for the initial form and 0 for the updated one, and 127/127 with
/// A7 segregated. So compute the whole descending sequence from the entry `An`
/// and update `An` exactly once, at the end. Decrementing per register and
/// writing `a[n]` as you go stores the wrong value for the one case where the
/// register is in its own list — which is the case real code uses.
///
/// # Faults
///
/// A **mid-transfer address error is structurally impossible**: the address steps
/// by 2 or 4, so parity is invariant across the whole transfer. Either access #0
/// faults or none does. Measured as a 2×2 table with both off-diagonals exactly
/// zero (`MOVEM.w` 516 odd→fault / 639 even→clean; `MOVEM.l` 489 / 624), and
/// stated directly: 0 operand accesses complete before the abort in all 1,182 +
/// 1,196 faulting cases.
///
/// So the alignment check happens **once**, before anything is committed, and
/// there is deliberately no partial-commit or rollback logic here. Adding it
/// would be unreachable code that the suite could never contradict — `MOVEM`
/// would go green with an arbitrary mid-transfer model.
///
/// `MOVEM` is the only write-fault site outside `MOVE`: 595 (`.w`) + 585 (`.l`)
/// register-to-memory faults. The stacked PC is `opcode + 2 * (2 + ext)` — past
/// the whole instruction — in every bucket, for both directions, and the IR is
/// the opcode.
fn movem(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, size: Size) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let to_regs = opcode & 0x0400 != 0;
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let ir = cpu.prefetch[0];

    let mask = cpu.prefetch[1];
    cpu.consume_opcode_dyn(bus);

    let de = ea::ext_words(mode, reg, size);
    let mut ext = [0u16; 2];
    fetch_ext(cpu, bus, de, &mut ext);

    let step = size.bytes();
    let count = mask.count_ones();
    let predec = mode == 4;

    // ⚠️ `MOVEM -(An)` charges **no** address-formation idle, unlike every other
    // instruction with that mode. The measured base for `-(An)` is 8 — the same
    // as `(An)` — where `(d8,An,Xn)` is 14 = 4*3 + 2 and does charge it. MOVEM
    // computes the block base by subtracting `count * step` in one go rather than
    // stepping the address register, so there is no per-decrement step to pay
    // for. Using the shared `idle_lead` here puts exactly the `-(An)` cases 2
    // cycles over and nothing else.
    let idle = if predec { 0 } else { idle_lead(mode, reg) };

    // Resolve the base by hand rather than through `ea::resolve`: the
    // predecrement and postincrement adjustments here are `count * step`, not
    // the single-operand `step`, and `-(An)`'s base is the *bottom* of the block.
    let base = match mode {
        3 => cpu.a[reg as usize],
        4 => cpu.a[reg as usize].wrapping_sub(count * step),
        _ => {
            let ea = ea::resolve(cpu, mode, reg, size, &ext, opcode_addr.wrapping_add(4));
            let Ea::Mem(a) = ea else {
                unreachable!("MOVEM only accepts memory addressing modes")
            };
            a
        }
    };

    // One check, before any transfer or register update. The faulting access is
    // the first transfer: descending modes start at the top of the block, every
    // other mode at the bottom.
    let first = if predec {
        // The first write is the low word of the highest long, which is `An - 2`
        // at both sizes: `An - step` for `.w` and `An - step + 2` for `.l`. Same
        // parity as `base`, so the check above still decides it, but the frame
        // records this address rather than the block's base.
        cpu.a[reg as usize].wrapping_sub(2)
    } else {
        base
    };
    if misaligned(base) {
        let kind = if to_regs {
            FaultKind::Read
        } else {
            FaultKind::Write
        };
        // Stacked PC is `opcode + 6 + 2 * ext_words` — past the whole
        // instruction, in both directions and every mode. Note that the `6` is
        // *not* `2 * (2 + de)`: that would give `+4` for the extension-free
        // modes, and the frame's PC low word then reads 2 short.
        exception::address_error(
            cpu,
            bus,
            first,
            kind,
            operand_space(mode, reg),
            ir,
            opcode_addr.wrapping_add(6 + 2 * de),
        );
        // Only the fetches that actually happened: the mask word and the
        // extension words. The queue advance is charged on the clean path only,
        // because the fault aborts the instruction before reaching it.
        //
        // Those same fetches are all a halted entry keeps — the frame and vector
        // fetch inside the 58 never happen — so the tail collapses and the lead
        // does not.
        return 4 * (1 + de) + idle + exception::entry_cycles(cpu, 0, ADDRESS_ERROR_TAIL_CYCLES);
    }

    let mut extra = 0;
    if to_regs {
        let mut addr = base;
        for i in mask_registers(mask, false) {
            let v = match size {
                Size::Long => {
                    let hi = bus.read16(addr & ADDR_MASK) as u32;
                    let lo = bus.read16(addr.wrapping_add(2) & ADDR_MASK) as u32;
                    (hi << 16) | lo
                }
                // Sign-extended into all 32 bits, address and data registers
                // alike.
                _ => bus.read16(addr & ADDR_MASK) as i16 as i32 as u32,
            };
            movem_write_reg(cpu, i, v);
            addr = addr.wrapping_add(step);
        }
        // The trailing discarded read, at `base + count * step`.
        bus.read16(addr & ADDR_MASK);
        extra = 1;
        if mode == 3 {
            // `(An)+` advances by the transfers only — not by the extra read.
            cpu.a[reg as usize] = base.wrapping_add(count * step);
            sync_sp(cpu);
        }
    } else if predec {
        // Descending, from the entry An downward, reading every register in its
        // pre-instruction form.
        let mut addr = cpu.a[reg as usize];
        for i in mask_registers(mask, true) {
            addr = addr.wrapping_sub(step);
            let v = movem_read_reg(cpu, i);
            match size {
                Size::Long => write_long_desc(bus, addr, v),
                _ => bus.write16(addr & ADDR_MASK, v as u16),
            }
        }
        cpu.a[reg as usize] = addr;
        sync_sp(cpu);
    } else {
        let mut addr = base;
        for i in mask_registers(mask, false) {
            let v = movem_read_reg(cpu, i);
            match size {
                Size::Long => write_long_asc(bus, addr, v),
                _ => bus.write16(addr & ADDR_MASK, v as u16),
            }
            addr = addr.wrapping_add(step);
        }
    }

    cpu.consume_opcode_dyn(bus);
    let transfers = count * (step / 2);
    4 * (2 + de + transfers + extra) + idle
}

fn movem_w(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    movem(cpu, bus, opcode, Size::Word)
}

fn movem_l(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    movem(cpu, bus, opcode, Size::Long)
}

// ---------------------------------------------------------------------------
// 4. MOVEP
// ---------------------------------------------------------------------------

/// `MOVEP` — alternate-byte transfer for 8-bit peripherals on a 16-bit bus.
///
/// Fully measured, 2500/2500 on each rule and on each size:
///
/// ```text
///   addressing mode       d16(An) only, base = An + sign_extend(d16)   2500/2500
///   accesses              BYTE, at base, +2, +4, +6                   2500/2500
///   byte order            most-significant FIRST                      2500/2500
///                                                        (LSB-first: 7/2500 .w)
///   strobes               uniform across the transfer                 2500/2500
///   CCR                   untouched
///   cycles                .w 16, .l 24, zero idle
///   shape                 P (the d16 fetch), all transfers, P
/// ```
///
/// ⚠️ **`MOVEP` never address-errors**, whatever its suffix says: every access is
/// a byte, and bytes have no alignment requirement. The stride of 2 preserves
/// parity, so the strobe follows the parity of the *first* address and then stays
/// fixed — which is why the harness sees a uniform strobe. Doing word accesses
/// instead produces transactions the vectors do not have *and* invents faults.
///
/// ⚠️ **`mem -> reg` at word size MERGES into `Dn`**: the upper 16 bits survive
/// (1257/1257 merge, 0 clobber). The long form writes all four bytes, so there is
/// nothing to preserve.
fn movep(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, size: Size) -> u32 {
    let dn = ((opcode >> 9) & 7) as usize;
    let an = (opcode & 7) as usize;
    let to_mem = opcode & 0x0080 != 0;
    let n = size.bytes();

    let disp = cpu.prefetch[1] as i16 as i32 as u32;
    cpu.consume_opcode_dyn(bus);
    let base = cpu.a[an].wrapping_add(disp);

    if to_mem {
        for i in 0..n {
            let shift = 8 * (n - 1 - i);
            bus.write8(
                base.wrapping_add(2 * i) & ADDR_MASK,
                (cpu.d[dn] >> shift) as u8,
            );
        }
    } else {
        let mut v = 0u32;
        for i in 0..n {
            v = (v << 8) | bus.read8(base.wrapping_add(2 * i) & ADDR_MASK) as u32;
        }
        cpu.d[dn] = match size {
            Size::Long => v,
            _ => (cpu.d[dn] & 0xFFFF_0000) | v,
        };
    }

    cpu.consume_opcode_dyn(bus);
    4 * (2 + n)
}

fn movep_w(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    movep(cpu, bus, opcode, Size::Word)
}

fn movep_l(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    movep(cpu, bus, opcode, Size::Long)
}

// ---------------------------------------------------------------------------
// 5. The SR/CCR moves
// ---------------------------------------------------------------------------

/// `MOVE SR,<ea>` — store the status register, word-sized.
///
/// ⚠️ **Not privileged on the 68000.** 0 of 747 user-mode cases trap.
///
/// ⚠️ It transfers the **full 16-bit SR**, not just the CCR: 404/404 for the full
/// word against 16/404 for CCR-only, on 388 discriminating cases. Into a `Dn`
/// that is an ordinary word write, so the upper half of the register survives
/// (404/404).
///
/// The schedule is `alu`'s read-modify-write: the destination is **read before it
/// is written**, even though the value read is discarded. Shapes `P` (mode 0,
/// 6 cycles = 4 + 2 idle), `R P W` (12), `i2 R P W` (14 for `-(An)`),
/// `P R P W` (16), `i2 P R P W` (18) — which is why this routes through
/// [`alu::run`](super::alu) rather than being hand-written, and why its ladder is
/// identical to `NBCD`'s: 6, 12, 12, 14, 16, 18, 16, 20.
fn move_from_sr(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let idle = if mode == 0 { 2 } else { 0 };
    let plan = super::alu::Plan::new(Size::Word, mode, reg)
        .writes()
        .idle(idle);
    super::alu::run(cpu, bus, &plan, &mut |cpu, _ops| Some(cpu.sr as u32))
}

/// `MOVE <ea>,CCR` and `MOVE <ea>,SR` — load the low or the whole word.
///
/// Both share one cycle ladder — 12, 16, 16, 18, 20, 22, 20, 24, 16 — and one
/// schedule, which is **not** `alu`'s. The tail rewinds the PC by 2 and refills
/// the queue, exactly like `logic`'s `ANDItoSR` family:
///
/// ```text
///   mode 0        i4 P-2 P+0                12   dpc +2
///   (An),(An)+    R i4 P-2 P+0              16   dpc +2
///   -(An)         i2 R i4 P-2 P+0           18   dpc +2
///   (d16,An)      P+0 R i4 P+0 P+2          20   dpc +4
///   (d8,An,Xn)    i2 P+0 R i4 P+0 P+2       22   dpc +4
///   (xxx).w       P+0 R i4 P+0 P+2          20   dpc +4
///   (xxx).l       P+0 P+2 R i4 P+2 P+4      24   dpc +6
///   (d16,PC)      P+0 p i4 P+0 P+2          20   dpc +4
///   (d8,PC,Xn)    i2 P+0 p i4 P+0 P+2       22   dpc +4
///   #imm          P+0 i4 P+0 P+2            16   dpc +4
/// ```
///
/// Read that as: fetch the extension words, read the operand, then **re-fetch
/// two words at the PC**. The two trailing fetches are a refill, not two queue
/// advances — for a register operand the first of them lands at `pc - 2`,
/// *before* the instruction's own end, which no forward-only schedule can
/// produce. The 4 idle cycles are the pipeline restart.
///
/// `MOVEtoCCR` writes only the low byte: the upper byte of the SR survives
/// (1504/1504) and bits 5-7 always read back zero (1504/1504), which `SR_MASK`
/// already enforces. `MOVEtoSR` is privileged (1290/1290) and its `set_sr` may
/// swap the stack pointers.
fn move_to_sr_ccr(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, to_sr: bool) -> u32 {
    if to_sr {
        if let Some(c) = privilege_check(cpu, bus) {
            return c;
        }
    }
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    // Capture the IR before any fetch: after the extension words are consumed,
    // `prefetch[0]` holds the *next* instruction's word, and the frame then
    // stacks a status word built from the wrong opcode.
    let ir = cpu.prefetch[0];

    let de = ea::ext_words(mode, reg, Size::Word);
    let mut ext = [0u16; 2];
    fetch_ext(cpu, bus, de, &mut ext);
    let ea = ea::resolve(
        cpu,
        mode,
        reg,
        Size::Word,
        &ext,
        opcode_addr.wrapping_add(2),
    );

    let mut acc = de;
    if let Ea::Mem(addr) = ea {
        if misaligned(addr) {
            // Every fault in both groups is a read fault, and the stacked PC is
            // `opcode + 2` for the register-indirect modes, `+4` for `-(An)` and
            // `(xxx).w`, `+6` for `(xxx).l` — i.e. the same rule `alu` measured,
            // reproduced here because this schedule is not `alu`'s.
            let bump = match (mode, reg) {
                (4, _) => 4,
                (7, 0) => 4,
                (7, 1) => 6,
                _ => 2,
            };
            exception::address_error(
                cpu,
                bus,
                addr,
                FaultKind::Read,
                operand_space(mode, reg),
                ir,
                opcode_addr.wrapping_add(bump),
            );
            // The extension fetches happened; the frame and vector fetch inside the
            // 58 do not, if the entry halted.
            return 4 * de
                + idle_lead(mode, reg)
                + exception::entry_cycles(cpu, 0, ADDRESS_ERROR_TAIL_CYCLES);
        }
    }
    let val = ea::read(cpu, bus, ea, Size::Word) as u16;
    if mode_is_mem(mode, reg) {
        acc += 1;
    }

    if to_sr {
        cpu.set_sr(val);
    } else {
        cpu.set_sr(move_to_ccr_value(cpu.sr, val));
    }

    // Rewind and refill: the two trailing fetches replace the queue rather than
    // advancing past the instruction. `refill_prefetch_dyn` advances the PC by 4,
    // so backing up 2 first lands it exactly 2 past the instruction's last word.
    cpu.pc = cpu.pc.wrapping_sub(2);
    cpu.refill_prefetch_dyn(bus);

    // No separate queue-advance term: the two refill fetches *are* this
    // instruction's whole program traffic beyond its extension words. Mode 0 is
    // the case that pins that down — `i4 P-2 P+0` is two accesses and 12 cycles,
    // so an extra `+1` here would put every mode 4 cycles over.
    4 * (acc + 2) + idle_lead(mode, reg) + 4
}

fn move_to_ccr(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    move_to_sr_ccr(cpu, bus, opcode, false)
}

fn move_to_sr(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    move_to_sr_ccr(cpu, bus, opcode, true)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// A `MOVEM` `<ea>` is control-alterable for the store direction and
/// control-plus-postincrement for the load direction.
///
/// Register direct is excluded (`MOVEM D0,D1` is meaningless), and the two
/// increment modes split by direction: `-(An)` stores only, `(An)+` and the
/// PC-relative modes load only.
fn valid_movem(mode: u16, reg: u16, to_regs: bool) -> bool {
    match mode {
        2 => true,
        3 => to_regs,
        4 => !to_regs,
        5 | 6 => true,
        7 => match reg {
            0 | 1 => true,
            2 | 3 => to_regs,
            _ => false,
        },
        _ => false,
    }
}

/// Installs every handler in this module.
///
/// The `0100` line is crowded: `arith`, `bcd`, `bits`, `branch` and `muldiv` all
/// have tenants there, so each of the selectors below is claimed narrowly.
/// `NOP` (`4E71`) is deliberately absent — it already lives in
/// [`logic`](super::logic) and its group is green.
pub fn register(table: &mut [crate::decode::Handler; 65536]) {
    for mode in 0..8u16 {
        for reg in 0..8u16 {
            let ea = (mode << 3) | reg;

            // 0100 1000 1s mmm rrr / 0100 1100 1s mmm rrr: MOVEM.
            for (bit, to_regs) in [(0x0000u16, false), (0x0400, true)] {
                if valid_movem(mode, reg, to_regs) {
                    table[(0x4880 | bit | ea) as usize] = movem_w;
                    table[(0x48C0 | bit | ea) as usize] = movem_l;
                }
            }

            // 0100 1000 01 mmm rrr: PEA. Control modes only.
            if valid_control(mode, reg) {
                table[(0x4840 | ea) as usize] = pea;
                // 0100 rrr 111 mmm rrr: LEA, same <ea> set.
                for dst in 0..8u16 {
                    table[(0x41C0 | (dst << 9) | ea) as usize] = lea;
                }
            }

            // 0100 0000 11 mmm rrr: MOVE from SR. Data-alterable.
            if super::arith::valid_data_alterable(mode, reg) {
                table[(0x40C0 | ea) as usize] = move_from_sr;
            }

            // 0100 0100 11 mmm rrr: MOVE to CCR.
            // 0100 0110 11 mmm rrr: MOVE to SR. Both take any data <ea>.
            if valid_data_src(mode, reg) {
                table[(0x44C0 | ea) as usize] = move_to_ccr;
                table[(0x46C0 | ea) as usize] = move_to_sr;
            }
        }
    }

    // 0100 1000 01 000 rrr: SWAP shares PEA's selector at mode 0, which PEA
    // does not accept — hence the separate loop rather than an arm above.
    for reg in 0..8u16 {
        table[(0x4840 | reg) as usize] = swap;
        table[(0x4880 | reg) as usize] = ext_w;
        table[(0x48C0 | reg) as usize] = ext_l;
        table[(0x4E50 | reg) as usize] = link;
        table[(0x4E58 | reg) as usize] = unlk;
        table[(0x4E60 | reg) as usize] = move_to_usp;
        table[(0x4E68 | reg) as usize] = move_from_usp;
    }

    table[0x4E70] = reset;
    table[0x4E72] = stop;

    // 0000 ddd 1oo 001 aaa: MOVEP. Mode field 001 is not an address register
    // here — it selects MOVEP, and the opmode picks the size and direction.
    for dn in 0..8u16 {
        for an in 0..8u16 {
            let base = 0x0008 | (dn << 9) | an;
            table[(base | 0x0100) as usize] = movep_w; // opmode 100: mem -> reg .w
            table[(base | 0x0140) as usize] = movep_l; // opmode 101: mem -> reg .l
            table[(base | 0x0180) as usize] = movep_w; // opmode 110: reg -> mem .w
            table[(base | 0x01C0) as usize] = movep_l; // opmode 111: reg -> mem .l
        }
    }

    // 1100 xxx 1oo 00y yyy: EXG, three opmodes.
    for rx in 0..8u16 {
        for ry in 0..8u16 {
            let d = (rx << 9) | ry;
            table[(0xC140 | d) as usize] = exg; // 01000: Dx <-> Dy
            table[(0xC148 | d) as usize] = exg; // 01001: Ax <-> Ay
            table[(0xC188 | d) as usize] = exg; // 10001: Dx <-> Ay
        }
    }
}

/// Control addressing modes: memory, but no increment/decrement and no
/// immediate. `LEA` and `PEA`'s `<ea>` set.
fn valid_control(mode: u16, reg: u16) -> bool {
    match mode {
        2 | 5 | 6 => true,
        7 => reg <= 3,
        _ => false,
    }
}

/// Any data `<ea>`, including immediate: `MOVE to CCR`/`SR`'s source set.
fn valid_data_src(mode: u16, reg: u16) -> bool {
    match mode {
        1 => false,
        7 => reg <= 4,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::{FlatBus, RecordingBus};
    use crate::cpu::{SR_N, SR_S};
    use crate::decode::Decoder;

    fn at(bus: &mut impl Bus) -> M68k {
        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(bus);
        cpu
    }

    /// `MOVEM.l D0-D7/A0-A7,-(A7)` writes descending, and the A7 it stores is
    /// the value *before* the instruction moved it.
    ///
    /// Both facts are invisible to the cycle count (which depends only on the
    /// popcount) and to the final memory image (which is correctly big-endian
    /// either way for the word order). Only the ordered write log catches them.
    #[test]
    fn movem_l_predecrement_descends_and_stores_the_entry_a7() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x48E7, 0xFFFF, 0x4E71]); // MOVEM.l D0-D7/A0-A7,-(A7)
        let mut cpu = at(&mut bus);
        for i in 0..8 {
            cpu.d[i] = 0xD000_0000 | i as u32;
            cpu.a[i] = 0xA000_0000 | i as u32;
        }
        cpu.a[7] = 0x3000;
        cpu.ssp = 0x3000;
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        // 16 registers x 4 bytes below the entry SP.
        assert_eq!(cpu.a[7], 0x3000 - 64, "A7 drops by 16 longs");

        // The stored A7 is the ENTRY value, at the top of the block.
        assert_eq!(
            bus.read16(0x3000 - 4),
            0x0000,
            "stacked A7 high word is the entry SP"
        );
        assert_eq!(
            bus.read16(0x3000 - 2),
            0x3000,
            "stacked A7 low word is the entry SP, not SP-4"
        );
        // D0 lands at the bottom: the reversed mask puts A7 first.
        assert_eq!(bus.read16(0x3000 - 64), 0xD000);
        assert_eq!(bus.read16(0x3000 - 62), 0x0000);

        // Write order: descending addresses, and low word before high within
        // each long.
        let writes = bus.writes();
        assert_eq!(writes.len(), 32, "16 longs = 32 word writes");
        assert_eq!(
            writes[0],
            (0x3000 - 2, 0x3000),
            "first write is the LOW word of the highest long"
        );
        assert_eq!(
            writes[1],
            (0x3000 - 4, 0x0000),
            "then its high word, at the lower address"
        );
        assert_eq!(writes[2].0, 0x3000 - 6, "the next long descends");
    }

    /// `MOVEM.w (A0),D0-D3` restores ascending, sign-extends each word into all
    /// 32 bits, and performs one extra discarded read past the list.
    #[test]
    fn movem_w_to_registers_ascends_sign_extends_and_reads_one_extra() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4C90, 0x000F, 0x4E71]); // MOVEM.w (A0),D0-D3
        bus.load(0x2000, &[0x0001, 0x8002, 0x0003, 0xFFFC, 0xDEAD]);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2000;
        for i in 0..4 {
            cpu.d[i] = 0x1111_0000;
        }
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 0x0000_0001, "ascending: D0 takes the first word");
        assert_eq!(
            cpu.d[1], 0xFFFF_8002,
            "each word is SIGN-EXTENDED to 32 bits"
        );
        assert_eq!(cpu.d[2], 0x0000_0003);
        assert_eq!(cpu.d[3], 0xFFFF_FFFC);

        let reads: Vec<u32> = bus.reads().iter().map(|r| r.0).collect();
        assert!(
            reads.contains(&0x2008),
            "the to-registers direction reads one extra word past the list, \
             at base + count*size; reads were {reads:02X?}"
        );
        // 4 transfers + 1 extra + 2 program words + the queue advance.
        assert_eq!(cycles, 4 * (2 + 4 + 1), "12 cycles of base plus 4 per word");
    }

    /// `MOVEM.w (A0)+,D0-D2` advances `A0` by `count * size` — **not** by the
    /// extra trailing read, even though that read touches `base + count * size`.
    #[test]
    fn movem_w_postincrement_advances_by_the_transfers_only() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4C98, 0x0007, 0x4E71]); // MOVEM.w (A0)+,D0-D2
        bus.load(0x2000, &[0x1234, 0x5678, 0x9ABC, 0xDEAD]);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2000;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 0x0000_1234);
        assert_eq!(cpu.d[2], 0xFFFF_9ABC);
        assert_eq!(
            cpu.a[0], 0x2006,
            "3 words = +6; the discarded read at 0x2006 does not advance A0"
        );
    }

    /// `MOVEP.w D0,d16(A0)` hits alternate bytes, most-significant first.
    #[test]
    fn movep_w_writes_alternate_bytes_most_significant_first() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x0188, 0x0010, 0x4E71]); // MOVEP.w D0,16(A0)
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2000;
        cpu.d[0] = 0x1234_ABCD;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        let writes = bus.writes();
        assert_eq!(
            writes.len(),
            2,
            "MOVEP.w is two BYTE accesses, not one word"
        );
        assert_eq!(writes[0], (0x2010, 0xAB), "most-significant byte first");
        assert_eq!(writes[1], (0x2012, 0xCD), "then +2, not +1");
        assert_eq!(cycles, 16);
    }

    /// `MOVEP.l d16(A0),D0` reads four alternate bytes and assembles them
    /// most-significant first.
    #[test]
    fn movep_l_reads_four_alternate_bytes() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x0149, 0x0002, 0x4E71]); // MOVEP.l 2(A1),D0
        bus.put16(0x2002, 0xA8FF);
        bus.put16(0x2004, 0x4FFF);
        bus.put16(0x2006, 0xFCFF);
        bus.put16(0x2008, 0xBCFF);
        let mut cpu = at(&mut bus);
        cpu.a[1] = 0x2000;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 0xA84F_FCBC, "bytes at +0,+2,+4,+6, MSB first");
        assert_eq!(cycles, 24);
    }

    /// `MOVEP.w` into a register merges: the upper half of `Dn` survives.
    #[test]
    fn movep_w_to_register_merges_into_the_low_half() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x0109, 0x0000, 0x4E71]); // MOVEP.w 0(A1),D0
        bus.put16(0x2000, 0x11FF);
        bus.put16(0x2002, 0x22FF);
        let mut cpu = at(&mut bus);
        cpu.a[1] = 0x2000;
        cpu.d[0] = 0xAAAA_5555;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 0xAAAA_1122, "the upper 16 bits are preserved");
    }

    /// `LINK A7,#d` pushes the SP as it was on entry, not `SP - 4`.
    #[test]
    fn link_a7_pushes_the_entry_stack_pointer() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4E57, 0xFFF0, 0x4E71]); // LINK A7,#-16
        let mut cpu = at(&mut bus);
        cpu.a[7] = 0x3000;
        cpu.ssp = 0x3000;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.read16(0x2FFE),
            0x3000,
            "LINK A7 pushes the ENTRY SP, not SP-4"
        );
        // A7 := SP-4 = 0x2FFC, then += -16.
        assert_eq!(cpu.a[7], 0x2FFC - 16);
        // This pins the sync_sp invariant (shadow coherence), not observable
        // harness behaviour — the harness reads the active pointer from a[7].
        assert_eq!(cpu.ssp, cpu.a[7], "shadow SSP is kept coherent by sync_sp");
        assert_eq!(cycles, 16);
    }

    /// `UNLK A7` ends with SP holding the popped value: `SP := A7` is a no-op
    /// and the pop then overwrites SP outright.
    #[test]
    fn unlk_a7_leaves_sp_holding_the_popped_value() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4E5F, 0x4E71]); // UNLK A7
        bus.put16(0x3000, 0x0001);
        bus.put16(0x3002, 0x2340);
        let mut cpu = at(&mut bus);
        cpu.a[7] = 0x3000;
        cpu.ssp = 0x3000;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(
            cpu.a[7], 0x0001_2340,
            "UNLK A7 ends with the popped value, not old_A7 + 4"
        );
        assert_eq!(cycles, 12);
    }

    /// `UNLK An` for a normal register: SP ends at `old An + 4`.
    #[test]
    fn unlk_an_pops_into_an_and_leaves_sp_above_it() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4E5A, 0x4E71]); // UNLK A2
        bus.put16(0x2800, 0x0000);
        bus.put16(0x2802, 0x4444);
        let mut cpu = at(&mut bus);
        cpu.a[2] = 0x2800;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.a[2], 0x0000_4444, "A2 takes the popped long");
        assert_eq!(cpu.a[7], 0x2804, "SP ends at old A2 + 4");
    }

    /// `LEA` must not read its operand — the access log holds program fetches
    /// only.
    #[test]
    fn lea_performs_no_data_access() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x43D0, 0x4E71]); // LEA (A0),A1
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2001; // odd, and still must not fault
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.a[1], 0x2001);
        assert!(
            bus.reads().iter().all(|r| r.0 >= 0x1000 && r.0 < 0x1010),
            "LEA reads only the instruction stream; got {:04X?}",
            bus.reads()
        );
        assert_eq!(cycles, 4);
        assert_eq!(cpu.sr, SR_S, "LEA does not touch the CCR");
    }

    /// `PEA` pushes the address, high word first, at the final SP.
    #[test]
    fn pea_pushes_the_address_high_word_first() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4850, 0x4E71]); // PEA (A0)
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x0012_3456;
        cpu.a[7] = 0x3000;
        cpu.ssp = 0x3000;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.a[7], 0x2FFC);
        let writes = bus.writes();
        assert_eq!(
            writes[0],
            (0x2FFC, 0x0012),
            "high word at the lower address"
        );
        assert_eq!(writes[1], (0x2FFE, 0x3456));
        assert_eq!(cycles, 12);
    }

    /// `PEA (xxx).w` defers its queue advance past the push, unlike every other
    /// mode. Identical memory and cycles either way, so only the ordered log
    /// distinguishes them.
    #[test]
    fn pea_absolute_short_advances_the_queue_after_the_push() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4878, 0x2000, 0x4E71]); // PEA (0x2000).w
        let mut cpu = at(&mut bus);
        cpu.a[7] = 0x3000;
        cpu.ssp = 0x3000;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        // P W W P: one fetch, both writes, then the deferred advance.
        let log: Vec<(bool, u32)> = bus.log.iter().map(|e| (e.0, e.1)).collect();
        assert!(!log[0].0, "access 0 is a program fetch");
        assert_eq!(log[1], (true, 0x2FFC), "the writes come next");
        assert_eq!(log[2], (true, 0x2FFE));
        assert!(!log[3].0, "the queue advance is LAST for (xxx).w");
        assert_eq!(cycles, 16);
    }

    /// `STOP` performs **no bus access at all** and does not move the PC.
    #[test]
    fn stop_makes_no_bus_access_and_leaves_the_pc_alone() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E72, 0x2700, 0x4E71]); // STOP #$2700
        let mut cpu = at(&mut bus);
        let pc_before = cpu.pc;
        let queue_before = cpu.prefetch;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert!(
            bus.log.is_empty(),
            "STOP takes its immediate from the queue, with no fetch; got {:04X?}",
            bus.log
        );
        assert_eq!(cpu.pc, pc_before, "the PC does not advance");
        assert_eq!(cpu.prefetch, queue_before, "the queue is unchanged");
        assert_eq!(cpu.sr, 0x2700);
        assert!(cpu.stopped);
        assert_eq!(cycles, 4);
    }

    /// `RESET` costs 132 cycles — one access and 128 idle — and leaves the SR
    /// alone.
    #[test]
    fn reset_costs_132_cycles_and_preserves_the_sr() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4E70, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr = SR_S | 0x0705;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cycles, 132);
        assert_eq!(cpu.sr, SR_S | 0x0705);
        assert_eq!(cpu.pc, 0x1006, "the queue advanced by one word");
    }

    /// Each privileged instruction traps to vector 8 for 34 cycles, stacking the
    /// opcode's own address.
    #[test]
    fn the_privileged_instructions_trap_to_vector_8_in_user_mode() {
        for (opcode, name) in [
            (0x46C0u16, "MOVE D0,SR"),
            (0x4E60, "MOVE A0,USP"),
            (0x4E68, "MOVE USP,A0"),
            (0x4E70, "RESET"),
            (0x4E72, "STOP"),
        ] {
            let mut bus = FlatBus::new();
            bus.load(0x1000, &[opcode, 0x2700, 0x4E71]);
            bus.put16(0x0020, 0x0000); // vector 8 -> 0x4000
            bus.put16(0x0022, 0x4000);
            bus.load(0x4000, &[0x4E71, 0x4E71]);

            let mut cpu = M68k::new();
            cpu.sr = 0; // user mode
            cpu.usp = 0x2000;
            cpu.ssp = 0x3000;
            cpu.a[7] = 0x2000;
            cpu.pc = 0x1000;
            cpu.prime_prefetch(&mut bus);

            let dec = Decoder::new();
            let cycles = cpu.step_with(&dec, &mut bus);

            assert_eq!(cycles, 34, "{name} must cost 34 cycles to trap");
            assert_eq!(cpu.pc, 0x4004, "{name} must vector through 8");
            assert!(cpu.sr_s(), "{name} enters supervisor mode");
            assert_eq!(
                bus.read16(0x3000 - 2),
                0x1000,
                "{name} stacks the OPCODE address, with no bump"
            );
            assert_eq!(
                cpu.a[7],
                0x3000 - 6,
                "{name} frames on the supervisor stack"
            );
        }
    }

    /// `MOVE SR,Dn` is **not** privileged and transfers all 16 bits.
    #[test]
    fn move_from_sr_is_unprivileged_and_moves_the_whole_word() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x40C0, 0x4E71]); // MOVE SR,D0
        let mut cpu = M68k::new();
        cpu.sr = 0x0015; // user mode, some flags set
        cpu.usp = 0x2000;
        cpu.a[7] = 0x2000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        cpu.d[0] = 0xFFFF_FFFF;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.pc, 0x1006, "no trap: the instruction completed");
        assert_eq!(
            cpu.d[0], 0xFFFF_0015,
            "the full SR word, with Dn's upper half preserved"
        );
        assert_eq!(cycles, 6);
    }

    /// `MOVE <ea>,CCR` writes bits 4..0 and keeps every bit above them.
    ///
    /// Asserts [`move_to_ccr_value`]'s return rather than the resulting `cpu.sr`,
    /// because `set_sr`'s `SR_MASK` clears bits 7-5 anyway and so hides the
    /// difference between this rule and a byte mask. An earlier attempt at this
    /// test drove a full `step_with` with bits 5-7 set in the source and asserted
    /// `cpu.sr`; it passed under both masks, which is how the weaker spelling
    /// survived in the first place.
    #[test]
    fn move_to_ccr_writes_only_the_low_five_bits() {
        // Bits 7,6,5 set in the source alongside the five CCR bits.
        assert_eq!(
            move_to_ccr_value(0x0700, 0x00FF),
            0x071F,
            "bits 7-5 of the operand are not CCR bits and must not be taken"
        );
        // Every bit above 4 comes from the old SR, including ones SR_MASK would
        // clear — so this also pins that the function itself does not mask.
        assert_eq!(
            move_to_ccr_value(0xFFE0, 0x0000),
            0xFFE0,
            "the whole system half is preserved, bit for bit"
        );
        assert_eq!(
            move_to_ccr_value(0x0000, 0x001F),
            0x001F,
            "all five CCR bits are writable"
        );
        assert_eq!(
            move_to_ccr_value(0xFFFF, 0x0000),
            0xFFE0,
            "and all five are clearable"
        );
    }

    /// `MOVE <ea>,CCR` is unprivileged, writes only the low byte, and rewinds
    /// the PC before refilling — so a register operand's first refill fetch
    /// lands *before* the end of the instruction.
    #[test]
    fn move_to_ccr_is_unprivileged_and_rewinds_before_refilling() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x44C0, 0x4E71, 0x4E71]); // MOVE D0,CCR
        let mut cpu = M68k::new();
        cpu.sr = 0x0700; // user mode, interrupt mask set
        cpu.usp = 0x2000;
        cpu.a[7] = 0x2000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        cpu.d[0] = 0xFFFF_FF1F;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(
            cpu.sr, 0x071F,
            "only the CCR half changes; bits 5-7 stay clear via SR_MASK"
        );
        assert_eq!(cycles, 12);
        let reads: Vec<u32> = bus.reads().iter().map(|r| r.0).collect();
        assert_eq!(
            reads,
            vec![0x1002, 0x1004],
            "the tail refills at pc-2 then pc, rather than advancing twice"
        );
        assert_eq!(cpu.pc, 0x1006);
    }

    /// `MOVE <ea>,SR` is privileged, and its `set_sr` swaps the stack pointers
    /// when it drops out of supervisor mode.
    #[test]
    fn move_to_sr_can_leave_supervisor_mode_and_swaps_the_stack_pointers() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x46C0, 0x4E71, 0x4E71]); // MOVE D0,SR
        let mut cpu = at(&mut bus);
        cpu.a[7] = 0x3000;
        cpu.ssp = 0x3000;
        cpu.usp = 0x2000;
        cpu.d[0] = 0x0000; // clears S

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert!(!cpu.sr_s());
        assert_eq!(cpu.a[7], 0x2000, "a[7] switches to the USP");
        assert_eq!(cpu.ssp, 0x3000, "the SSP is saved");
    }

    /// `SWAP` takes N and Z from the full 32-bit result, not from either half.
    #[test]
    fn swap_sets_n_from_the_swapped_long() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4840, 0x4E71]); // SWAP D0
        let mut cpu = at(&mut bus);
        // Low half has the high bit set, so after the swap N is set.
        cpu.d[0] = 0x0001_8000;
        cpu.sr = SR_S | 0x0010; // X set, and it must survive

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 0x8000_0001);
        assert_eq!(cpu.sr, SR_S | 0x0010 | SR_N, "N from bit 31, X preserved");
        assert_eq!(cycles, 4);
    }

    /// `EXT.w` leaves the upper 16 bits of `Dn` untouched; `EXT.l` does not.
    #[test]
    fn ext_w_preserves_the_upper_half_and_ext_l_does_not() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4880, 0x4E71]); // EXT.w D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0xAAAA_AA80;
        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0], 0xAAAA_FF80, "EXT.w is a word-sized write");
        assert_eq!(cpu.sr & SR_N, SR_N);

        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x48C0, 0x4E71]); // EXT.l D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0xAAAA_8000;
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0], 0xFFFF_8000, "EXT.l rewrites all 32 bits");
    }

    /// `EXG` exchanges across register files and does not touch the CCR.
    #[test]
    fn exg_exchanges_data_and_address_registers() {
        let dec = Decoder::new();
        // EXG D0,A1 -> opmode 10001
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xC189, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x1111_1111;
        cpu.a[1] = 0x2222_2222;
        cpu.sr = SR_S | 0x001F;

        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 0x2222_2222);
        assert_eq!(cpu.a[1], 0x1111_1111);
        assert_eq!(cpu.sr, SR_S | 0x001F, "EXG leaves the CCR alone");
        assert_eq!(cycles, 6);
    }

    /// `MOVE USP,An` and `MOVE An,USP` reach the *inactive* stack pointer.
    #[test]
    fn the_usp_moves_reach_the_inactive_stack_pointer() {
        let dec = Decoder::new();
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4E68, 0x4E71]); // MOVE USP,A0
        let mut cpu = at(&mut bus);
        cpu.usp = 0x1234_5678;
        cpu.a[7] = 0x3000;
        cpu.ssp = 0x3000;

        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.a[0], 0x1234_5678);
        assert_eq!(cpu.a[7], 0x3000, "the active SP is untouched");

        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4E61, 0x4E71]); // MOVE A1,USP
        let mut cpu = at(&mut bus);
        cpu.a[1] = 0x00AB_CDEF;
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.usp, 0x00AB_CDEF);
    }

    /// A misaligned `MOVEM` base faults on access #0 with nothing committed —
    /// no register loaded and `An` not updated.
    #[test]
    fn a_misaligned_movem_faults_before_transferring_anything() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4C98, 0x000F, 0x4E71]); // MOVEM.w (A0)+,D0-D3
        bus.put16(0x000C, 0x0000); // vector 3 -> 0x5000
        bus.put16(0x000E, 0x5000);
        bus.load(0x5000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2001; // odd
        for i in 0..4 {
            cpu.d[i] = 0x9999_9999;
        }
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.pc, 0x5004, "vectored through 3");
        assert_eq!(cpu.a[0], 0x2001, "An is NOT updated on a fault");
        for i in 0..4 {
            assert_eq!(cpu.d[i], 0x9999_9999, "no register is loaded");
        }
        // No data-space access happened before the frame: the first write is the
        // frame's own.
        assert!(
            bus.reads()
                .iter()
                .all(|r| r.0 < 0x1010 || r.0 >= 0x5000 || r.0 < 0x0010),
            "the aborted access must not appear on the bus; got {:04X?}",
            bus.reads()
        );
    }

    /// `PEA` with an odd SP halts without writing the operand.
    ///
    /// The stacked PC is not asserted here — there are 0 PEA address-error cases
    /// in the vector suite, so any expected offset is extrapolated rather than
    /// measured. **Nor is a frame asserted:** an odd `a[7]` in supervisor mode is
    /// an odd frame base, so `exception::double_bus_fault` halts instead of
    /// stacking. This test originally expected vector 3; that expectation was
    /// retracted, not the check that produces it. The check is what keeps the odd
    /// write off the bus, and without it there is neither a halt nor a fault —
    /// just a silently split word.
    ///
    /// The operand write being absent is asserted by value, not by address: `A0`
    /// is loaded with `0xABCD_0000`, whose halves (`0xABCD`, `0x0000`) cannot
    /// appear in the exception frame, so the absence of `0xABCD` in all write
    /// values confirms the push never happened — which is the contract at
    /// `exception.rs:148-151`.
    ///
    /// **The cycle count is asserted**, and its earlier absence is how this path
    /// kept returning `ADDRESS_ERROR_TAIL_CYCLES` — 58, `4 × 12 + 10` for an
    /// aborted access, 7 frame writes, 2 vector reads and 2 refills — on a step
    /// whose bus log holds exactly one access. The count below is *derived* from
    /// that log; only [`exception::HALTED_IDLE_CYCLES`] is extrapolated.
    #[test]
    fn pea_with_odd_sp_faults_without_writing() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4850, 0x4E71]); // PEA (A0)
        bus.put16(0x000C, 0x0000); // vector 3 -> 0x5000
        bus.put16(0x000E, 0x5000);
        bus.load(0x5000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0xABCD_0000; // distinctive high word; must not appear in writes
        cpu.a[7] = 0x2FFF; // odd: SP - 4 = 0x2FFB, also odd
        cpu.ssp = 0x2FFF;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert!(cpu.halted, "an odd frame base is a double bus fault");
        assert_eq!(bus.writes(), vec![], "no frame and no operand push");
        assert!(
            !bus.log.iter().any(|&(w, _, v)| w && v == 0xABCD),
            "the operand push must not have happened; 0xABCD appeared in writes: {:04X?}",
            bus.writes()
        );
        // `PEA (A0)` has no extension word, so the one access is the queue
        // advance — and it really happened, so it is still owed.
        assert_eq!(bus.log.len(), 1, "just the queue advance");
        assert_eq!(
            cycles,
            4 + exception::HALTED_IDLE_CYCLES,
            "4 × 1 access + the halt idle, not the framed 58"
        );
    }

    /// The same halt, from an **indexed** mode, which is the only shape that can
    /// see the lead idle.
    ///
    /// `pea_with_odd_sp_faults_without_writing` uses `PEA (A0)`, where
    /// `idle_lead` is 0 — so it scores the same whether the halt arm charges the
    /// lead idle or drops it, and it could not see that the arm *did* drop it.
    /// `(d8,A0,D0)` is one of the three modes where `idle_lead` is nonzero, and
    /// `pea` charges it twice (see `idle_lead`'s docs), so the two spellings
    /// differ by 4 here.
    ///
    /// This is the control the mode-2 test needed: a case where the term under
    /// test is guaranteed present by construction. Deleting `2 * idle_lead(..)`
    /// from `pea`'s halt arm fails this and nothing else.
    #[test]
    fn pea_indexed_with_odd_sp_charges_its_lead_idle() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4870, 0x0000, 0x4E71]); // PEA (0,A0,D0.w)
        bus.put16(0x000C, 0x0000); // vector 3 -> 0x5000
        bus.put16(0x000E, 0x5000);
        bus.load(0x5000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0xABCD_0000;
        cpu.d[0] = 0;
        cpu.a[7] = 0x2FFF; // odd: SP - 4 = 0x2FFB, also odd
        cpu.ssp = 0x2FFF;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert!(cpu.halted, "an odd frame base is a double bus fault");
        assert_eq!(bus.writes(), vec![], "no frame and no operand push");
        // The extension-word fetch and the queue advance, and nothing else: the
        // aborted push must not reach the bus.
        assert_eq!(
            bus.log.len(),
            2,
            "the extension fetch and the queue advance"
        );
        assert_eq!(
            cycles,
            4 * 2 + 4 + exception::HALTED_IDLE_CYCLES,
            "4 × 2 accesses + 4 of lead idle + the halt idle"
        );
    }

    /// `LINK` with an odd SP halts without writing the operand.
    ///
    /// The stacked PC is not asserted here — there are 0 LINK address-error cases
    /// in the vector suite, so any expected offset is extrapolated rather than
    /// measured. See `pea_with_odd_sp_faults_without_writing` for why the outcome
    /// is a halt rather than a vector-3 frame.
    ///
    /// Uses `LINK A0,#0` (not A7) so the pushed value is `A0` and is independent
    /// of SP. `A0` is loaded with `0xDEAD_0000`; the absence of `0xDEAD` in all
    /// write values confirms the push never happened.
    ///
    /// The cycle count is asserted for the reason
    /// `pea_with_odd_sp_faults_without_writing` gives: the framed 58 accounts for
    /// twelve accesses this step does not make.
    #[test]
    fn link_with_odd_sp_faults_without_writing() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E50, 0x0000, 0x4E71]); // LINK A0,#0
        bus.put16(0x000C, 0x0000); // vector 3 -> 0x5000
        bus.put16(0x000E, 0x5000);
        bus.load(0x5000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0xDEAD_0000; // distinctive high word; must not appear in writes
        cpu.a[7] = 0x2FFF; // odd: SP - 4 = 0x2FFB, also odd
        cpu.ssp = 0x2FFF;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert!(cpu.halted, "an odd frame base is a double bus fault");
        assert_eq!(bus.writes(), vec![], "no frame and no operand push");
        assert!(
            !bus.log.iter().any(|&(w, _, v)| w && v == 0xDEAD),
            "the operand push must not have happened; 0xDEAD appeared in writes: {:04X?}",
            bus.writes()
        );
        // The displacement fetch is the one access that reached the bus.
        assert_eq!(bus.log.len(), 1, "just the displacement fetch");
        assert_eq!(
            cycles,
            4 + exception::HALTED_IDLE_CYCLES,
            "4 × 1 access + the halt idle, not the framed 58"
        );
    }
}
