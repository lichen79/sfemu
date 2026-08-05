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
pub(crate) fn push16(cpu: &mut M68k, bus: &mut dyn Bus, val: u16) {
    cpu.a[7] = cpu.a[7].wrapping_sub(2);
    bus.write16(cpu.a[7] & ADDR_MASK, val);
}

/// Pushes a long, low word first, so the long reads back big-endian.
pub(crate) fn push32(cpu: &mut M68k, bus: &mut dyn Bus, val: u32) {
    push16(cpu, bus, (val & 0xFFFF) as u16);
    push16(cpu, bus, (val >> 16) as u16);
}

/// Takes a group 1/2 exception: save SR, enter supervisor mode, push the short
/// frame (PC then SR), then vector.
///
/// `pc_for_frame` is the PC value to stack, which differs per exception type,
/// so callers pass it explicitly rather than this function guessing from
/// `cpu.pc`.
pub fn take(cpu: &mut M68k, bus: &mut dyn Bus, vector: u8, pc_for_frame: u32) {
    let old_sr = cpu.sr;
    // Enter supervisor mode and clear trace before touching the stack, so the
    // frame lands on the supervisor stack and the handler does not trace.
    cpu.set_sr((old_sr | SR_S) & !SR_T);

    push32(cpu, bus, pc_for_frame);
    push16(cpu, bus, old_sr);

    let addr = (vector as u32) * 4;
    let hi = bus.read16(addr & ADDR_MASK) as u32;
    let lo = bus.read16((addr + 2) & ADDR_MASK) as u32;
    cpu.pc = (hi << 16) | lo;
    cpu.refill_prefetch_dyn(bus);
}

/// Byte distance from `cpu.pc` at handler entry back to the opcode word.
///
/// When `step_with` dispatches a handler, `cpu.pc` is 6 bytes ahead of the
/// opcode that was just fetched:
/// - 4 bytes: the prefetch queue holds two words beyond the instruction
/// - 2 bytes: `fetch_word` advanced `pc` by one more word to consume the opcode
///
/// **This offset is only valid for a handler that has consumed exactly the
/// opcode word and no extension words.** Any handler that calls `fetch_word`
/// or `fetch_long` to consume extension words will have advanced `pc` further;
/// those handlers must capture `cpu.pc` at entry (before any such fetch) and
/// pass their own adjusted value to `take`.
pub(crate) const OPCODE_PC_OFFSET: u32 = 6;

/// An unrecognised opcode. Line-A and Line-F opcodes have their own vectors,
/// which some software uses deliberately as a trap mechanism.
pub fn illegal_instruction(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let vector = match op >> 12 {
        0xA => VEC_LINE_A,
        0xF => VEC_LINE_F,
        _ => VEC_ILLEGAL,
    };
    // Stack the address of the offending instruction. See OPCODE_PC_OFFSET for
    // why this offset is 6 and why it must not be copied into other handlers.
    let pc = cpu.pc.wrapping_sub(OPCODE_PC_OFFSET);
    take(cpu, bus, vector, pc);
    34
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::FlatBus;
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

        // Short frame: PC (long) then SR (word), so SP drops by 6.
        assert_eq!(cpu.a[7], 0x2FFA);
        assert_eq!(bus.read16(0x2FFC), 0x0000);
        assert_eq!(bus.read16(0x2FFE), 0x1000, "stacked PC is the bad opcode");
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
}
