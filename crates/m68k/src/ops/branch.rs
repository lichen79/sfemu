//! Branches, jumps, subroutine calls, and conditional set/decrement.
//!
//! Nine instructions across three encoding lines, sharing one thing: they are
//! the first family whose *whole* effect is on the PC and the prefetch queue. So
//! this is where the queue model earns its keep, and where it fails loudly if it
//! is wrong — the harness compares both queue words on every case.
//!
//! # The displacement base
//!
//! `Bcc`, `BSR` and `DBcc` all compute
//!
//! ```text
//! target = opcode_addr + 2 + sign_extend(displacement)
//! ```
//!
//! With `OPCODE_PC_OFFSET = 4` the opcode's own address is `pc - 4`, so the base
//! is `pc - 2`: the address of the word *after* the opcode, which is the
//! displacement word's own address in the 16-bit form. Measured **674/674** on
//! the taken non-faulting `Bcc` cases and 1,229/1,229 on `BSR`, checked through
//! the suite's `target = final_pc - 4` identity (the queue is refilled at the
//! target, so the final PC sits 4 past it).
//!
//! A `Bcc` whose 8-bit displacement field is `0x00` takes a 16-bit displacement
//! from the following word instead. That word is **already in `prefetch[1]`** —
//! reading it costs no bus access, which is why a taken 16-bit branch has the
//! same two-access sequence as a taken 8-bit one.
//!
//! # Return-address push order
//!
//! `BSR` and `JSR` decrement SP by 4 and then write **ascending**: the high word
//! to `SP`, the low word to `SP+2`. 2,570/2,570 two-write cases high-word-first
//! (`BSR` 1,229, `JSR` 1,341), zero counterexamples, and every one of them
//! discriminates — no return address in the corpus has equal halves. This is *not*
//! the exception frame's order (`PC_lo`, `SR`, `PC_hi` at non-sequential offsets —
//! see [`exception::address_error`]), and the two must not be confused.
//!
//! The pushed value is the address of the next instruction:
//!
//! ```text
//! return = opcode_addr + 2 * (1 + extension words consumed)
//! ```
//!
//! which is `+2` for `BSR.b` and `JSR (An)`, `+4` for one extension word, and
//! `+6` for `JSR abs.L`. Derived mechanically from [`ea::ext_words`] rather than
//! from a per-mode table, and unanimous over all 2,570 two-write cases.
//!
//! # Cycle counts
//!
//! Per `timing-law.md`: `cycles = 4 * (non-idle bus accesses) + idle`. The access
//! sequence is forced by the harness, so idle is the only term to model, and for
//! this family it is a small unanimous table:
//!
//! ```text
//!                                        accesses  idle  cycles  cases
//! Bcc/BRA  8-bit,  not taken                    1     4       8   1147
//! Bcc/BRA  16-bit, not taken                    2     4      12      9
//! Bcc/BRA  either, taken                        2     2      10    674
//! BSR      either                               4     2      18   1229
//! DBcc     condition true, falls through        2     4      12   1257
//! DBcc     condition false, branches            2     2      10    611
//! DBcc     counter expired, falls through       2     6      14      0  <- untested
//! RTS                                           4     0      16   1263
//! RTR                                           5     0      20   1285
//! Scc      Dn, condition false                  1     0       4    200
//! Scc      Dn, condition true                   1     2       6    204
//! ```
//!
//! `JMP` and `JSR` share one idle table, keyed on the addressing mode alone —
//! `JSR` is `JMP` plus the two return-address writes, and the idle term does not
//! notice:
//!
//! ```text
//!                    JMP acc  JSR acc  idle    JMP    JSR    cases (JMP/JSR)
//! (An)                     2        4     0      8     16      388 / 406
//! d16(An)                  2        4     2     10     18      346 / 375
//! (d8,An,Xn)               2        4     6     14     22      343 / 386
//! abs.W                    2        4     2     10     18       57 /  44
//! abs.L                    3        5     0     12     20       49 /  40
//! d16(PC)                  2        4     2     10     18       46 /  44
//! (d8,PC,Xn)               2        4     6     14     22       43 /  46
//! ```
//!
//! `abs.L` is the only mode that pays for a program fetch — its *second*
//! extension word is not yet in the queue — and it is also the only one with no
//! idle, so the two effects cancel in the total but not in the sequence.
//!
//! # What faults, and what is committed first
//!
//! Every address error in all eight groups is a **program-space read** fault at
//! the branch or jump target — 7,412 of 7,412, with **zero** cases of any other
//! kind, zero write-address-errors, and zero odd initial stack pointers. So no
//! write-fault path exists in this family and none is written. (`Scc` has no
//! address-error cases at all; the other seven groups supply all 7,412.)
//!
//! What the instruction has already committed when the fault is taken differs per
//! instruction, and each row is a measured absence or presence rather than a
//! guess (user-mode cases, where the frame lands on the *other* stack and so the
//! delta is visible in isolation):
//!
//! ```text
//!         USP delta on a faulting case      meaning
//! BSR                 -4  (618/618)   pushed BEFORE the fault
//! JSR                  0  (585/585)   did NOT push
//! RTS                 +4  (598/598)   the pop is committed
//! RTR                 +6  (578/578)   the pop is committed, and the CCR restored
//! Bcc/DBcc/JMP         0               nothing to commit
//! ```
//!
//! `DBcc` additionally does **not** commit its decrement: the counter is
//! unchanged in 632/632 faulting cases, and since not one case starts with a low
//! word of 0, a decrement would have been visible in every one of them.
//!
//! The stacked PC, unanimous within each bucket:
//!
//! ```text
//! Bcc      opcode + 2                670       (both displacement widths)
//! DBcc     opcode + 4                632
//! RTS      opcode + 2               1237
//! RTR      opcode + 2               1215
//! JMP      opcode + 2               1228       every addressing mode
//! JSR      opcode + 2 * (1 + ext)   1159       i.e. its own return address
//! BSR      the branch target itself 1271       <- the one that is not an opcode offset
//! ```
//!
//! `BSR` stacking the *target* rather than an offset from the opcode looks like a
//! measurement error and is not: 1,271/1,271, with no case where the two
//! coincide.
//!
//! The frame's fault address is the full **unmasked 32-bit** target in every
//! group. Scored only on the cases where masking to 24 bits would differ, the
//! full-32 rule is 1,018/1,018 (`JMP`), 1,116/1,116 (`JSR`), 1,259/1,259 (`RTS`)
//! and 1,280/1,280 (`RTR`); the masked rival scores 0 on each of those subsets.
//! The final PC of a *non*-faulting jump is likewise the unmasked target plus 4.

use crate::cpu::{M68k, ADDR_MASK};
use crate::decode::Handler;
use crate::ea::{self, Size};
use crate::exception::{self, FaultKind, Space, ADDRESS_ERROR_TAIL_CYCLES};
use crate::ops::alu::{self, Ops, Plan};
use crate::Bus;

/// Evaluates one of the 16 condition codes.
pub fn test_condition(cpu: &M68k, cond: u8) -> bool {
    let (n, z, v, c) = (cpu.ccr_n(), cpu.ccr_z(), cpu.ccr_v(), cpu.ccr_c());
    match cond & 0xF {
        0x0 => true,           // T
        0x1 => false,          // F
        0x2 => !c && !z,       // HI
        0x3 => c || z,         // LS
        0x4 => !c,             // CC / HS
        0x5 => c,              // CS / LO
        0x6 => !z,             // NE
        0x7 => z,              // EQ
        0x8 => !v,             // VC
        0x9 => v,              // VS
        0xA => !n,             // PL
        0xB => n,              // MI
        0xC => n == v,         // GE
        0xD => n != v,         // LT
        0xE => !z && (n == v), // GT
        _ => z || (n != v),    // LE
    }
}

/// Idle cycles of a taken `Bcc`/`BRA`/`DBcc` branch, and of `BSR`.
const TAKEN_IDLE: u32 = 2;
/// Idle cycles of a `Bcc` that falls through, and of a condition-true `DBcc`.
const NOT_TAKEN_IDLE: u32 = 4;
/// Idle cycles of the `DBcc` counter-expiry fall-through.
///
/// **Both this constant and the fetch count below are unverified — neither is
/// backed by a suite case or a physical timing table.**
///
/// The 68000 manual's row for cc-false, counter-expired is `14(3/0)` — three
/// read cycles, i.e. `4*3 + 2` — as reported by review; that table could not be
/// verified against a physical copy. This implementation emits **2** fetches,
/// not 3: a fall-through must advance the queue past the opcode and the
/// displacement word, which is exactly two program reads, and three would leave
/// the queue inconsistent. `EXPIRY_IDLE = 6` is then the value that makes the
/// total agree with the manual's 14 (`4*2 + 6 = 14`). The suite confirms the
/// total for the measured rows (12 and 10), but zero cases reach expiry, so
/// neither the access count nor the idle split has any vector coverage.
///
/// **Consequence:** a future vector or hardware trace that reaches expiry may
/// match on total cycles and still fail on access sequence. If that happens, the
/// fetch count is the first thing to re-examine.
const EXPIRY_IDLE: u32 = 6;

/// Is the target of a jump or branch misaligned?
///
/// An instruction fetch is a word access, so an odd target always faults. There
/// is no size to consider: unlike an operand, a target is never byte-wide.
#[inline]
fn target_faults(target: u32) -> bool {
    target & 1 != 0
}

/// Raises the target address error every instruction in this module shares, and
/// returns the case's total cycle count.
///
/// `acc` and `idle` are what the instruction spent *before* the fault; the tail
/// is fixed at [`ADDRESS_ERROR_TAIL_CYCLES`], which already accounts for the
/// aborted access. The fault is always a program-space read — see the module
/// docs' 7,412/7,412.
fn target_error(
    cpu: &mut M68k,
    bus: &mut dyn Bus,
    target: u32,
    ir: u16,
    pc_for_frame: u32,
    acc: u32,
    idle: u32,
) -> u32 {
    exception::address_error(
        cpu,
        bus,
        target,
        FaultKind::Read,
        Space::Program,
        ir,
        pc_for_frame,
    );
    // `acc` and `idle` happened; the tail has not, if the entry halted. Measured:
    // `JMP (A0)` to an odd target with an odd SSP halts with an empty bus log.
    4 * acc + idle + exception::entry_cycles(cpu, 0, ADDRESS_ERROR_TAIL_CYCLES)
}

/// Sets the PC and refills both queue slots. Two program reads.
#[inline]
fn jump_to(cpu: &mut M68k, bus: &mut dyn Bus, target: u32) {
    cpu.pc = target;
    cpu.refill_prefetch_dyn(bus);
}

/// Pushes a 32-bit return address: SP down 4, then **high word first**, both
/// writes ascending. See the module docs.
///
/// Writes `a[7]` and deliberately **not** the `usp`/`ssp` shadow, so after a `BSR` the
/// shadow is stale — measured, in supervisor mode `a[7]` reads `0x2FFC` while `ssp`
/// still reads `0x3000`. That is correct under the invariant `M68k::a` documents: the
/// active pointer is authoritative only in `a[7]`, and `set_sr` saves it into the right
/// slot before any S-bit change can expose the stale one. The harness agrees — it reads
/// the active SP from `a[7]` — which is why 1,229/1,229 `BSR` cases pass without the
/// sync.
fn push_return(cpu: &mut M68k, bus: &mut dyn Bus, ret: u32) {
    let sp = cpu.a[7].wrapping_sub(4);
    cpu.a[7] = sp;
    bus.write16(sp & ADDR_MASK, (ret >> 16) as u16);
    bus.write16(sp.wrapping_add(2) & ADDR_MASK, ret as u16);
}

// --- Line 0110: Bcc, BRA, BSR ---------------------------------------------

/// `0110 cccc dddddddd` — conditional branch, with condition 0 meaning `BRA`
/// and condition 1 meaning `BSR` rather than "never".
///
/// A displacement field of `0x00` selects the 16-bit form, whose displacement
/// word is already in `prefetch[1]`.
fn bcc(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let cond = ((op >> 8) & 0xF) as u8;
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let disp8 = (op & 0xFF) as u8;
    let wide = disp8 == 0;

    let disp = if wide {
        cpu.prefetch[1] as i16 as i32 as u32
    } else {
        disp8 as i8 as i32 as u32
    };
    let base = opcode_addr.wrapping_add(2);
    let target = base.wrapping_add(disp);
    // Words this instruction occupies: the opcode, plus a displacement word in
    // the 16-bit form.
    let words = if wide { 2 } else { 1 };

    if cond == 1 {
        // BSR. The return address is the next instruction's, so it is past the
        // displacement word when there is one.
        let ret = opcode_addr.wrapping_add(2 * words);
        push_return(cpu, bus, ret);
        if target_faults(target) {
            // Pushed before faulting, and the frame stacks the *target*.
            return target_error(cpu, bus, target, op, target, 2, TAKEN_IDLE);
        }
        jump_to(cpu, bus, target);
        return 4 * 4 + TAKEN_IDLE;
    }

    if cond == 0 || test_condition(cpu, cond) {
        if target_faults(target) {
            return target_error(cpu, bus, target, op, base, 0, TAKEN_IDLE);
        }
        jump_to(cpu, bus, target);
        return 4 * 2 + TAKEN_IDLE;
    }

    // Falling through: advance the queue past the opcode and the displacement
    // word, and do NOT refill — the queue is still valid, and refilling would
    // fail all 1,147 of the 8-cycle cases.
    for _ in 0..words {
        cpu.consume_opcode_dyn(bus);
    }
    4 * words + NOT_TAKEN_IDLE
}

// --- Line 0101 at size 11: DBcc and Scc ------------------------------------

/// `0101 cccc 11001 rrr` — decrement and branch while the condition is false.
///
/// # The expiry path has no vector coverage
///
/// All 2,500 cases start with a non-zero counter low word, so the suite can
/// never reach expiry and a green `DBcc` group says nothing about loop
/// termination. Two things therefore have to be right by construction, and are
/// covered by unit tests instead:
///
/// - the test is against `-1` **after** decrementing, not against `0` before, so
///   an initial `n` takes `n` branches — a do-while loop whose body therefore
///   runs `n + 1` times, which is why an initial `0xFFFF` runs it 65,536 times
///   and an initial `0` runs it exactly once;
/// - the decrement is 16-bit and wraps **within the low word** — a 32-bit
///   `wrapping_sub` would corrupt the high word exactly when the low word is 0,
///   which is precisely the case no vector exercises.
fn dbcc(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let cond = ((op >> 8) & 0xF) as u8;
    let reg = (op & 7) as usize;
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let target = opcode_addr
        .wrapping_add(2)
        .wrapping_add(cpu.prefetch[1] as i16 as i32 as u32);

    // Condition true: no decrement at all, fall through past the displacement.
    if test_condition(cpu, cond) {
        cpu.consume_opcode_dyn(bus);
        cpu.consume_opcode_dyn(bus);
        return 4 * 2 + NOT_TAKEN_IDLE;
    }

    let counter = (cpu.d[reg] as u16).wrapping_sub(1);
    let decremented = (cpu.d[reg] & 0xFFFF_0000) | counter as u32;

    if counter == 0xFFFF {
        // Expired: commit the counter and fall through. Zero suite cases.
        cpu.d[reg] = decremented;
        cpu.consume_opcode_dyn(bus);
        cpu.consume_opcode_dyn(bus);
        return 4 * 2 + EXPIRY_IDLE;
    }

    if target_faults(target) {
        // The decrement is NOT committed: 632/632 faulting cases show the
        // counter unchanged.
        return target_error(
            cpu,
            bus,
            target,
            op,
            opcode_addr.wrapping_add(4),
            0,
            TAKEN_IDLE,
        );
    }
    cpu.d[reg] = decremented;
    jump_to(cpu, bus, target);
    4 * 2 + TAKEN_IDLE
}

/// `0101 cccc 11 mmmrrr` — set a byte to `0xFF` or `0x00` on a condition.
///
/// This is the one instruction in the family that fits [`alu::run`] unchanged:
/// its measured sequence is `read <ea>`, queue advance, `write <ea>` — the
/// standard single-`<ea>` read-modify-write schedule — even though the value
/// written does not depend on the value read. The read is real and must happen
/// (404/404 of the `(An)` cases show it), so leaving it out to "optimise" a
/// write-only operation would fail the ordered comparison.
///
/// `Scc Dn` is the exception in two ways, both handled by the plan rather than by
/// a separate code path: it performs **no bus write** (0 writes in 404/404
/// cases), which falls out of [`ea::write`] on a register, and it is the one
/// place in the core where a *flag* changes the cycle count — a true condition
/// costs 2 extra idle cycles, split 204 true / 200 false with no overlap. The
/// memory modes charge 0 idle whatever the condition, so the extra 2 is keyed on
/// mode 0.
///
/// Modes 4 and 6 pay a further 2 idle for forming their address; that comes from
/// [`alu::run`]'s own `idle_lead` and is not repeated here.
fn scc(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let cond = ((op >> 8) & 0xF) as u8;
    let (mode, reg) = ((op >> 3) & 7, op & 7);
    let set = test_condition(cpu, cond);

    let plan = Plan::new(Size::Byte, mode, reg)
        .writes()
        .idle(if mode == 0 && set { 2 } else { 0 });
    alu::run(cpu, bus, &plan, &mut |_cpu, _ops: Ops| {
        Some(if set { 0xFF } else { 0x00 })
    })
}

// --- 0100 1110: JMP, JSR, RTS, RTR ----------------------------------------

/// Idle cycles `JMP` and `JSR` spend forming the target address, per addressing
/// mode. Unanimous within every bucket; see the module docs for the counts.
///
/// `abs.L` idles 0 because it spends a real program fetch instead — its second
/// extension word is the only one not already sitting in the prefetch queue.
///
/// The seven arms are exactly the control modes, so the fallthrough is
/// unreachable: `register` installs these handlers only where
/// [`ea::modes::control`] holds, and that predicate admits modes 2/5/6 and mode 7
/// regs 0..=3 and nothing else.
///
/// ⚠️ **The fallthrough asserts rather than returning 0.** It previously returned
/// 0, which is a *plausible* idle count — so if a future `register` widened the
/// installed set, every newly reachable mode would be charged 4 accesses and no
/// idle, and the suite would report a cycle count wrong by a small multiple of 2
/// rather than an obviously broken one. Returning a defensible-looking number
/// from an unreachable arm is how a dispatch-table change turns into a timing
/// bug that reads as a constant being off. `debug_assert` keeps release builds
/// free of the branch while making the debug-mode opcode-space sweep — which
/// executes all 65,536 encodings — the thing that catches it.
fn jump_idle(mode: u16, reg: u16) -> u32 {
    match (mode, reg) {
        (2, _) => 0, // (An)
        (5, _) => 2, // d16(An)
        (6, _) => 6, // (d8,An,Xn)
        (7, 0) => 2, // abs.W
        (7, 1) => 0, // abs.L
        (7, 2) => 2, // d16(PC)
        (7, 3) => 6, // (d8,PC,Xn)
        _ => {
            debug_assert!(
                false,
                "jump_idle reached a non-control mode {mode}/{reg}: JMP/JSR were \
                 installed somewhere ea::modes::control does not hold"
            );
            0
        }
    }
}

/// Resolves a `JMP`/`JSR` target, emitting the one program fetch `abs.L` needs.
///
/// Returns the target and the accesses spent. The first extension word is
/// already in `prefetch[1]`; a second one requires advancing the queue, after
/// which it is in `prefetch[1]` in turn. PC-relative modes are relative to the
/// extension word's own address, `opcode_addr + 2` — 46/46 and 44/44 for `JMP`
/// and `JSR`'s two PC modes.
fn jump_target(cpu: &mut M68k, bus: &mut dyn Bus, mode: u16, reg: u16) -> (u32, u32) {
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let de = ea::ext_words(mode, reg, Size::Long);
    let mut ext = [0u16; 2];
    let mut acc = 0;
    if de >= 1 {
        ext[0] = cpu.prefetch[1];
    }
    if de >= 2 {
        cpu.consume_opcode_dyn(bus);
        ext[1] = cpu.prefetch[1];
        acc += 1;
    }
    let ea = ea::resolve(
        cpu,
        mode,
        reg,
        Size::Long,
        &ext[..de as usize],
        opcode_addr.wrapping_add(2),
    );
    let ea::Ea::Mem(target) = ea else {
        unreachable!("only control addressing modes are registered for JMP/JSR")
    };
    (target, acc)
}

/// `0100 1110 11 mmmrrr` — jump.
fn jmp(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let (mode, reg) = ((op >> 3) & 7, op & 7);
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let idle = jump_idle(mode, reg);
    let (target, mut acc) = jump_target(cpu, bus, mode, reg);

    if target_faults(target) {
        return target_error(cpu, bus, target, op, opcode_addr.wrapping_add(2), acc, idle);
    }
    jump_to(cpu, bus, target);
    acc += 2;
    4 * acc + idle
}

/// `0100 1110 10 mmmrrr` — jump to subroutine.
///
/// The two return-address writes sit **between** the two target refill reads:
/// `read target`, `write SP`, `write SP+2`, `read target+2`. That interleaving is
/// why this cannot go through [`alu::run`] — no single-`<ea>` schedule splits a
/// refill around a push — and it is unanimous across all 1,341 non-faulting
/// cases and every addressing mode.
///
/// Unlike `BSR`, `JSR` does **not** push before faulting: 0 of 1,159 faulting
/// cases moved the stack pointer.
fn jsr(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let (mode, reg) = ((op >> 3) & 7, op & 7);
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let idle = jump_idle(mode, reg);
    let de = ea::ext_words(mode, reg, Size::Long);
    // The next instruction's address: past the opcode and every extension word.
    let ret = opcode_addr.wrapping_add(2 * (1 + de));

    let (target, mut acc) = jump_target(cpu, bus, mode, reg);
    if target_faults(target) {
        // Nothing pushed. The stacked PC is the return address it would have
        // pushed, which is also `opcode + 2 * (1 + ext)`.
        return target_error(cpu, bus, target, op, ret, acc, idle);
    }

    // The refill, split around the push.
    cpu.pc = target;
    cpu.prefetch[0] = bus.read16(target & ADDR_MASK);
    push_return(cpu, bus, ret);
    cpu.prefetch[1] = bus.read16(target.wrapping_add(2) & ADDR_MASK);
    cpu.pc = target.wrapping_add(4);
    acc += 4;
    4 * acc + idle
}

/// `0100 1110 0111 0101` (`4E75`) — return from subroutine.
fn rts(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let sp = cpu.a[7];
    let hi = bus.read16(sp & ADDR_MASK) as u32;
    let lo = bus.read16(sp.wrapping_add(2) & ADDR_MASK) as u32;
    let target = (hi << 16) | lo;
    cpu.a[7] = sp.wrapping_add(4);

    if target_faults(target) {
        // The pop is committed: +4 in 598/598 faulting user-mode cases.
        return target_error(cpu, bus, target, op, opcode_addr.wrapping_add(2), 2, 0);
    }
    jump_to(cpu, bus, target);
    4 * 4
}

/// `0100 1110 0111 0111` (`4E77`) — return and restore the CCR.
///
/// Pops **six** bytes: a CCR word at `SP`, then the longword PC above it, read in
/// ascending order (`SP`, `SP+2`, `SP+4` — unanimous over 1,285 cases). Only the
/// low five CCR bits are restored; the SR's system byte is untouched, so `RTR` is
/// unprivileged.
///
/// The rule `sr = (sr & 0xFF00) | (word & 0x1F)` scores 1,285/1,285, and each
/// rival scores **0** on the subset where it disagrees: `(sr & 0xFF00) | (word &
/// 0xA71F)` 0/966, `(sr & 0xFF00) | (word & 0xFF)` 0/1,132, and taking the whole
/// word `word & 0xA71F` 0/1,245.
///
/// The CCR is restored **before** the target alignment check: on a faulting case
/// the stacked SR equals the restored SR 1,215/1,215, and equals the entry SR in
/// 0 of the 1,180 cases where those two differ.
fn rtr(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);
    let sp = cpu.a[7];
    let ccr = bus.read16(sp & ADDR_MASK);
    let hi = bus.read16(sp.wrapping_add(2) & ADDR_MASK) as u32;
    let lo = bus.read16(sp.wrapping_add(4) & ADDR_MASK) as u32;
    let target = (hi << 16) | lo;
    cpu.a[7] = sp.wrapping_add(6);
    // Direct assignment, not `set_sr`: the S bit cannot change here, so there is
    // no stack-pointer swap to perform, and the mask is narrower than SR_MASK.
    cpu.sr = (cpu.sr & 0xFF00) | (ccr & 0x1F);

    if target_faults(target) {
        return target_error(cpu, bus, target, op, opcode_addr.wrapping_add(2), 3, 0);
    }
    jump_to(cpu, bus, target);
    4 * 5
}

// --- Dispatch-table installation ------------------------------------------

/// Installs the whole of line `0110`, the size-`11` half of line `0101`, and the
/// four `0100 1110` encodings this task owns.
pub fn register(table: &mut [Handler; 65536]) {
    // Every one of line 0110's 4,096 encodings exists: 16 conditions (with 0 =
    // BRA and 1 = BSR) times 256 displacements, the `0x00` displacement being
    // the 16-bit form rather than a hole.
    for op in 0x6000..=0x6FFFu16 {
        table[op as usize] = bcc;
    }

    // Line 0101 at size 11. Mode 001 is DBcc — the one place in this line where
    // a `001` mode field is not an address register — and every other mode is
    // Scc over the data-alterable set. Mode 7 reg 2/3/4 would be TRAPcc on a
    // 68020 and is nothing at all here.
    for op in 0x5000..=0x5FFFu16 {
        if (op >> 6) & 3 != 3 {
            continue;
        }
        let (mode, reg) = ((op >> 3) & 7, op & 7);
        if mode == 1 {
            table[op as usize] = dbcc;
        } else if super::arith::valid_data_alterable(mode, reg) {
            table[op as usize] = scc;
        }
    }

    // 0100 1110 10/11 mmmrrr: JSR and JMP, split by bit 6.
    //
    // The operand set is *control* addressing — a memory operand whose address
    // does not depend on the access itself, so no register direct and neither
    // `(An)+` nor `-(An)`, a jump having no operand size to step by. That rule
    // comes from `ea::modes::control` rather than a local copy: a private
    // `is_control` here duplicated it exactly (verified 64/64 over every
    // `(mode, reg)` pair, against a control predicate that disagreed on 8), and a
    // duplicate that currently agrees is the shape a future divergence takes.
    // `jump_idle` below enumerates the same 28 pairs a third time; see its docs.
    for op in 0x4E80..=0x4EFFu16 {
        let (mode, reg) = ((op >> 3) & 7, op & 7);
        if ea::modes::control(mode, reg) {
            table[op as usize] = if op & 0x0040 != 0 { jmp } else { jsr };
        }
    }
    table[0x4E75] = rts;
    table[0x4E77] = rtr;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::{FlatBus, RecordingBus};
    use crate::cpu::{M68k, SR_C, SR_S, SR_V, SR_X, SR_Z};
    use crate::decode::Decoder;

    fn cpu_with(n: bool, z: bool, v: bool, c: bool) -> M68k {
        let mut cpu = M68k::new();
        cpu.set_ccr(false, n, z, v, c);
        cpu
    }

    /// A halted branch-target fault is charged for its lead and nothing else.
    ///
    /// `target_error`'s tail is [`ADDRESS_ERROR_TAIL_CYCLES`] — 58, `4 × 12 + 10`
    /// for an aborted access, 7 frame writes, 2 vector reads and 2 refills. A
    /// double bus fault performs none of the twelve, so returning it
    /// unconditionally claimed 58 cycles for a step whose bus log is empty. `acc`
    /// and `idle` stay outside the call because they really happened.
    ///
    /// `JMP (A0)` is the shape with no lead at all — 0 accesses, 0 idle — which
    /// makes the whole return value the tail and the defect maximal. The even-SSP
    /// control is `jmp_to_an_odd_address_faults` above: same instruction, 58
    /// cycles, 11 accesses, a real frame.
    ///
    /// Extrapolated: 0 of 317,500 cases halt.
    #[test]
    fn a_halted_branch_target_fault_costs_only_its_lead() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4ED0, 0x4E71]); // JMP (A0)
        bus.put16(0x000C, 0x0000); // vector 3, so a frame would be visible
        bus.put16(0x000E, 0x2000);
        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[0] = 0x4001; // odd target
        cpu.a[7] = 0x3001; // odd frame base
        cpu.ssp = 0x3001;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert!(cpu.halted, "an odd frame base is a double bus fault");
        assert_eq!(bus.log.len(), 0, "nothing reached the bus");
        assert_eq!(
            cycles,
            exception::HALTED_IDLE_CYCLES,
            "JMP (A0) has no lead, so the halt idle is the whole cost"
        );
    }

    #[test]
    fn conditions_match_the_manual() {
        // T, F
        assert!(test_condition(&cpu_with(false, false, false, false), 0));
        assert!(!test_condition(&cpu_with(false, false, false, false), 1));
        // HI = !C && !Z
        assert!(test_condition(&cpu_with(false, false, false, false), 2));
        assert!(!test_condition(&cpu_with(false, true, false, false), 2));
        // LS = C || Z
        assert!(test_condition(&cpu_with(false, true, false, false), 3));
        // CC/HS = !C, CS/LO = C
        assert!(test_condition(&cpu_with(false, false, false, false), 4));
        assert!(test_condition(&cpu_with(false, false, false, true), 5));
        // NE, EQ
        assert!(test_condition(&cpu_with(false, false, false, false), 6));
        assert!(test_condition(&cpu_with(false, true, false, false), 7));
        // VC, VS
        assert!(test_condition(&cpu_with(false, false, false, false), 8));
        assert!(test_condition(&cpu_with(false, false, true, false), 9));
        // PL, MI
        assert!(test_condition(&cpu_with(false, false, false, false), 10));
        assert!(test_condition(&cpu_with(true, false, false, false), 11));
        // GE = N == V
        assert!(test_condition(&cpu_with(true, false, true, false), 12));
        assert!(!test_condition(&cpu_with(true, false, false, false), 12));
        // LT = N != V
        assert!(test_condition(&cpu_with(true, false, false, false), 13));
        // GT = !Z && (N == V)
        assert!(test_condition(&cpu_with(true, false, true, false), 14));
        assert!(!test_condition(&cpu_with(true, true, true, false), 14));
        // LE = Z || (N != V)
        assert!(test_condition(&cpu_with(false, true, false, false), 15));
    }

    /// A CPU sitting at 0x1000 with its queue primed, supervisor, SP at 0x3000.
    fn at(bus: &mut impl Bus) -> M68k {
        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000;
        cpu.ssp = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(bus);
        cpu
    }

    // --- DBcc: the paths no vector reaches --------------------------------

    /// The expiry transition is `0 -> 0xFFFF` and it **falls through**. This is
    /// the bucket with zero suite cases, so it exists only here.
    #[test]
    fn dbcc_expiry_wraps_the_low_word_and_falls_through() {
        let mut bus = FlatBus::new();
        // DBF D0,-8  (0x51C8, displacement 0xFFF8)
        bus.load(0x1000, &[0x51C8, 0xFFF8, 0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x1234_0000;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(
            cpu.d[0], 0x1234_FFFF,
            "the decrement must wrap within the low word, leaving the high word \
             untouched — a 32-bit wrapping_sub gives 0x1233FFFF"
        );
        assert_eq!(
            cpu.pc, 0x1008,
            "expiry falls through to the next instruction"
        );
        assert_eq!(
            cpu.prefetch,
            [0x4E71, 0x4E71],
            "queue advanced, not refilled"
        );
        assert_eq!(cycles, 4 * 2 + EXPIRY_IDLE);
    }

    /// A counter of 1 still branches: the test is against `-1` after
    /// decrementing, not against `0` before it. Testing `== 0` first would end
    /// the loop one iteration early.
    #[test]
    fn dbcc_counter_of_one_still_branches() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x51C8, 0xFFF8, 0x4E71]);
        bus.load(0x0FF8, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x00FF_0001;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 0x00FF_0000);
        assert_eq!(
            cpu.pc, 0x0FFE,
            "branched to 0x1002 - 8 = 0xFFA, then refilled"
        );
        assert_eq!(cycles, 4 * 2 + TAKEN_IDLE);
    }

    /// An initial counter of `0xFFFF` therefore takes 65,535 branches, running the
    /// loop body 65,536 times. Driven to completion rather than asserted about,
    /// because the count is the whole point and an off-by-one in either direction
    /// is invisible in a single step.
    #[test]
    fn dbcc_from_ffff_iterates_65536_times() {
        let mut bus = FlatBus::new();
        // DBF D0,-2 — branches back to its own displacement word, i.e. loops on
        // itself; the loop body is the DBcc alone.
        bus.load(0x1000, &[0x51C8, 0xFFFE, 0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0xFFFF;

        let dec = Decoder::new();
        let mut iterations = 0u32;
        loop {
            cpu.step_with(&dec, &mut bus);
            iterations += 1;
            assert!(iterations <= 70_000, "the loop is not terminating");
            // A taken branch lands at 0x1000 and refills, so the PC is 0x1004
            // for as long as the loop is still going round.
            if cpu.pc != 0x1004 {
                break;
            }
        }
        assert_eq!(iterations, 65_536);
        assert_eq!(cpu.d[0], 0xFFFF, "expiry leaves the counter at 0xFFFF");
        assert_eq!(cpu.pc, 0x1008, "and falls through");
    }

    /// A true condition does nothing at all — no decrement, and the fall-through
    /// costs the same two fetches.
    #[test]
    fn dbcc_condition_true_leaves_the_counter_alone() {
        let mut bus = FlatBus::new();
        // DBT D0,-8 (0x50C8)
        bus.load(0x1000, &[0x50C8, 0xFFF8, 0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x0000_0005;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 5, "condition true must not decrement");
        assert_eq!(cpu.pc, 0x1008);
        assert_eq!(cycles, 4 * 2 + NOT_TAKEN_IDLE);
    }

    // --- Bcc's 16-bit displacement, 14 cases in 1,830 ---------------------

    /// `BRA.w`: a displacement field of 0 takes the following word, relative to
    /// that word's own address (`opcode + 2`).
    #[test]
    fn bra_word_displacement_is_relative_to_the_extension_word() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x6000, 0x0100]); // BRA.w +0x100
        bus.load(0x1102, &[0xAAAA, 0xBBBB]);
        let mut cpu = at(&mut bus);
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.pc, 0x1106, "target 0x1002 + 0x100, then refilled");
        assert_eq!(cpu.prefetch, [0xAAAA, 0xBBBB]);
        assert_eq!(
            bus.reads(),
            vec![(0x1102, 0xAAAA), (0x1104, 0xBBBB)],
            "the displacement word is read from the queue, not the bus"
        );
        assert_eq!(cycles, 4 * 2 + TAKEN_IDLE);
    }

    /// A not-taken `Bcc.w` still consumes the displacement word: two queue
    /// advances, no refill, 12 cycles. The 9-case bucket.
    #[test]
    fn bcc_word_not_taken_consumes_the_displacement() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x6700, 0x0100, 0x4E71, 0x4E71]); // BEQ.w, Z clear
        let mut cpu = at(&mut bus);
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.pc, 0x1008);
        assert_eq!(cpu.prefetch, [0x4E71, 0x4E71]);
        assert_eq!(bus.reads(), vec![(0x1004, 0x4E71), (0x1006, 0x4E71)]);
        assert_eq!(cycles, 4 * 2 + NOT_TAKEN_IDLE);
    }

    /// A not-taken 8-bit `Bcc` advances the queue once and must NOT refill.
    #[test]
    fn bcc_byte_not_taken_advances_the_queue_once() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x6704, 0x4E71, 0x4E71]); // BEQ.b +4, Z clear
        let mut cpu = at(&mut bus);
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.pc, 0x1006);
        assert_eq!(bus.reads().len(), 1, "one fetch, and no refill");
        assert_eq!(cycles, 4 + NOT_TAKEN_IDLE);
    }

    /// A backward 8-bit displacement is signed.
    #[test]
    fn bcc_byte_displacement_is_signed() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x60F0]); // BRA.b -16
        bus.load(0x0FF2, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.pc, 0x0FF6, "0x1002 - 16 = 0x0FF2, plus the refill");
    }

    // --- Push order and the JSR interleave -------------------------------

    /// `BSR` writes the high word to `SP` and the low word to `SP+2` — ascending,
    /// unlike the exception frame — and the pushed value is the next
    /// instruction's address.
    #[test]
    fn bsr_pushes_the_high_word_first_at_the_new_sp() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x6110]); // BSR.b +0x10
        bus.load(0x1012, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.a[7] = 0x0002_3000;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.a[7], 0x0002_2FFC);
        assert_eq!(
            bus.writes(),
            vec![(0x2FFC, 0x0000), (0x2FFE, 0x1002)],
            "return address 0x00001002: high word at SP, low word at SP+2, in \
             that order, and both bus addresses masked to 24 bits"
        );
        assert_eq!(cpu.pc, 0x1016);
        assert_eq!(cycles, 4 * 4 + TAKEN_IDLE);
    }

    /// `BSR.w` pushes `opcode + 4`, not `opcode + 2` — the return address is past
    /// the displacement word.
    #[test]
    fn bsr_word_pushes_past_the_displacement_word() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x6100, 0x0010]);
        bus.load(0x1012, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.a[7], 0x2FFC);
        assert_eq!(bus.read16(0x2FFE), 0x1004, "return address is opcode + 4");
        assert_eq!(cpu.pc, 0x1016);
    }

    /// `JSR`'s two writes sit **between** the two target reads. Asserting final
    /// memory cannot catch a push-then-refill ordering, so this asserts the
    /// interleaved sequence.
    #[test]
    fn jsr_splits_the_refill_around_the_push() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E92]); // JSR (A2)
        bus.load(0x2000, &[0xAAAA, 0xBBBB]);
        let mut cpu = at(&mut bus);
        cpu.a[2] = 0x2000;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.log,
            vec![
                (false, 0x2000, 0xAAAA),
                (true, 0x2FFC, 0x0000),
                (true, 0x2FFE, 0x1002),
                (false, 0x2002, 0xBBBB),
            ]
        );
        assert_eq!(cpu.pc, 0x2004);
        assert_eq!(cpu.prefetch, [0xAAAA, 0xBBBB]);
        assert_eq!(cycles, 4 * 4);
    }

    /// `JSR abs.L` spends one program fetch on its second extension word, and
    /// pushes `opcode + 6`.
    #[test]
    fn jsr_absolute_long_fetches_its_second_extension_word() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4EB9, 0x0000, 0x2000]);
        bus.load(0x2000, &[0xAAAA, 0xBBBB]);
        let mut cpu = at(&mut bus);
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.pc, 0x2004);
        assert_eq!(bus.read16(0x2FFE), 0x1006, "return address is opcode + 6");
        assert_eq!(cycles, 4 * 5, "5 accesses, no idle");
    }

    /// `JMP d16(PC)` is relative to the extension word's own address.
    #[test]
    fn jmp_pc_relative_is_relative_to_the_extension_word() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4EFA, 0x0020]); // JMP 0x20(PC)
        bus.load(0x1022, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.pc, 0x1026, "0x1002 + 0x20, then the refill");
        assert_eq!(cycles, 4 * 2 + 2);
    }

    // --- Returns ----------------------------------------------------------

    #[test]
    fn rts_pops_a_longword_and_refills() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E75]);
        bus.load(0x3000, &[0x0000, 0x2000]);
        bus.load(0x2000, &[0xAAAA, 0xBBBB]);
        let mut cpu = at(&mut bus);
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.a[7], 0x3004);
        assert_eq!(cpu.pc, 0x2004);
        assert_eq!(cpu.prefetch, [0xAAAA, 0xBBBB]);
        assert_eq!(
            bus.reads(),
            vec![
                (0x3000, 0x0000),
                (0x3002, 0x2000),
                (0x2000, 0xAAAA),
                (0x2002, 0xBBBB),
            ]
        );
        assert_eq!(cycles, 4 * 4);
    }

    /// `RTR` restores only the low five CCR bits and leaves the system byte —
    /// including S — alone, so it does not escalate privilege.
    #[test]
    fn rtr_restores_five_ccr_bits_and_nothing_else() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4E77]);
        // A stacked word with every bit set: only 0x1F may survive.
        bus.load(0x3000, &[0xFFFF, 0x0000, 0x2000]);
        bus.load(0x2000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr = SR_S | 0x0700;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(
            cpu.sr,
            SR_S | 0x0700 | SR_X | SR_Z | SR_V | SR_C | 0x08,
            "the system byte is preserved and only the low five bits restored"
        );
        assert_eq!(cpu.a[7], 0x3006, "RTR pops six bytes");
        assert_eq!(cpu.pc, 0x2004);
        assert_eq!(cycles, 4 * 5);
    }

    /// The CCR word is at `SP` and the PC longword above it — not the other way
    /// round, which would still produce a plausible-looking pop.
    #[test]
    fn rtr_reads_the_ccr_word_first() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4E77]);
        bus.load(0x3000, &[0x0000, 0x0000, 0x2000]);
        bus.load(0x2000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        let reads = bus.reads();
        assert_eq!(
            &reads[..3],
            &[(0x3000, 0x0000), (0x3002, 0x0000), (0x3004, 0x2000)]
        );
    }

    // --- Scc --------------------------------------------------------------

    /// `Scc Dn` writes no bus cycle at all, and a true condition costs 2 extra
    /// cycles — the one place a flag changes timing.
    #[test]
    fn scc_on_a_register_never_touches_the_bus() {
        for (sr_z, want_byte, want_cycles) in [(true, 0xFFu32, 6u32), (false, 0x00, 4)] {
            let mut bus = RecordingBus::new();
            bus.load(0x1000, &[0x57C0, 0x4E71]); // SEQ D0
            let mut cpu = at(&mut bus);
            cpu.d[0] = 0x1234_5678;
            if sr_z {
                cpu.sr |= SR_Z;
            }
            bus.log.clear();

            let dec = Decoder::new();
            let cycles = cpu.step_with(&dec, &mut bus);

            assert_eq!(cpu.d[0], 0x1234_5600 | want_byte, "byte-sized write");
            assert!(bus.writes().is_empty(), "Scc Dn performs no bus write");
            assert_eq!(cycles, want_cycles);
        }
    }

    /// `Scc <mem>` reads its destination before writing it, with the queue
    /// advance in between.
    #[test]
    fn scc_on_memory_reads_before_writing() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x56D2, 0x4E71]); // SNE (A2)
        let mut cpu = at(&mut bus);
        cpu.a[2] = 0x2000;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.log,
            vec![
                (false, 0x2000, 0x0000), // read the destination byte
                (false, 0x1004, 0x0000), // the queue advance
                (true, 0x2000, 0x00FF),  // write it back
            ]
        );
        assert_eq!(cycles, 4 * 3);
    }

    // --- Wrapping and faults ---------------------------------------------

    /// A target that wraps past 0 or past 0xFFFFFF must not panic, and the bus
    /// address must be masked to 24 bits while the PC keeps all 32.
    #[test]
    fn branch_targets_wrap_without_panicking() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x6000, 0x8000]); // BRA.w -0x8000
        let mut cpu = at(&mut bus);
        cpu.pc = 0x1004;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.pc, 0xFFFF_9006, "0x1002 - 0x8000 wraps below zero");

        // And a jump straight to the top of the address space.
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4EF9, 0xFFFF, 0xFFFE]); // JMP 0xFFFFFFFE
        let mut cpu = at(&mut bus);
        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.pc, 0x0000_0002, "target + 4 wraps");
    }

    /// An odd target raises a program-space read address error, and the aborted
    /// fetch must not reach the bus.
    #[test]
    fn an_odd_target_faults_without_fetching_it() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4EFA, 0x0021]); // JMP 0x21(PC) -> odd
        bus.load(0x000C, &[0x0000, 0x4000]); // vector 3
        bus.load(0x4000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.pc, 0x4004, "vectored through 3");
        assert!(
            !bus.log.iter().any(|(_, a, _)| *a == 0x1023 || *a == 0x1022),
            "the faulting fetch must never reach the bus"
        );
        // status word: stale IR bits, read (0x10), supervisor program space (0x6).
        assert_eq!(bus.read16(0x2FF2), (0x4EFA & 0xFFE0) | 0x16);
        assert_eq!(bus.read16(0x2FFE), 0x1002, "stacked PC is opcode + 2");
        assert_eq!(cycles, 2 + ADDRESS_ERROR_TAIL_CYCLES);
    }

    /// `BSR` pushes before it faults; `JSR` does not. The two differ, and the
    /// stack pointer is where it shows.
    #[test]
    fn bsr_pushes_before_faulting_and_jsr_does_not() {
        for (words, want_sp) in [
            ([0x6111u16, 0x0000, 0x0000], 0x2FFCu32), // BSR.b +0x11 -> odd
            ([0x4EBA, 0x0011, 0x0000], 0x3000),       // JSR 0x11(PC) -> odd
        ] {
            let mut bus = FlatBus::new();
            bus.load(0x1000, &words);
            bus.load(0x000C, &[0x0000, 0x4000]);
            bus.load(0x4000, &[0x4E71, 0x4E71]);
            let mut cpu = at(&mut bus);

            let dec = Decoder::new();
            cpu.step_with(&dec, &mut bus);

            // The frame is 14 bytes below wherever the SP stood after the push.
            assert_eq!(
                cpu.a[7],
                want_sp - 14,
                "opcode {:04X}: the frame must land below the pushed return address",
                words[0]
            );
        }
    }

    /// `RTS` and `RTR` commit their pop before checking the target, and `RTR`
    /// restores the CCR first — so the frame stacks the *restored* SR.
    #[test]
    fn rtr_restores_the_ccr_before_faulting() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x4E77]);
        bus.load(0x3000, &[0x001F, 0x0000, 0x2001]); // all CCR bits, odd PC
        bus.load(0x000C, &[0x0000, 0x4000]);
        bus.load(0x4000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr = SR_S | 0x0700;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        // The pop is committed (SP +6), then the frame lands 14 below that.
        assert_eq!(cpu.a[7], 0x3006 - 14);
        assert_eq!(
            bus.read16(0x3006 - 6),
            SR_S | 0x0700 | 0x1F,
            "the stacked SR includes the restored CCR"
        );
        assert_eq!(cycles, 4 * 3 + ADDRESS_ERROR_TAIL_CYCLES);
    }

    /// A faulting `DBcc` must not commit its decrement.
    #[test]
    fn dbcc_does_not_commit_its_decrement_when_the_target_faults() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x51C8, 0x0011]); // DBF D0, +0x11 -> odd target
        bus.load(0x000C, &[0x0000, 0x4000]);
        bus.load(0x4000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x0000_0005;

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 5, "the counter must be untouched");
        assert_eq!(bus.read16(0x2FFE), 0x1004, "stacked PC is opcode + 4");
        assert_eq!(cycles, TAKEN_IDLE + ADDRESS_ERROR_TAIL_CYCLES);
    }

    /// `BSR`'s frame stacks the branch *target*, not an offset from the opcode.
    #[test]
    fn bsr_stacks_the_branch_target_on_a_fault() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x6111]); // BSR.b +0x11 -> target 0x1013
        bus.load(0x000C, &[0x0000, 0x4000]);
        bus.load(0x4000, &[0x4E71, 0x4E71]);
        let mut cpu = at(&mut bus);

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        let base = 0x2FFCu32; // SP after the push
        assert_eq!(bus.read16(base - 2), 0x1013, "stacked PC is the target");
        assert_eq!(bus.read16(base - 10), 0x1013, "and so is the fault address");
        assert_eq!(cycles, 4 * 2 + TAKEN_IDLE + ADDRESS_ERROR_TAIL_CYCLES);
    }
}
