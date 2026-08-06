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
pub fn take(cpu: &mut M68k, bus: &mut dyn Bus, vector: u8, pc_for_frame: u32) {
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
}
