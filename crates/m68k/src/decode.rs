//! Opcode dispatch.
//!
//! The 68000's opcode is a single 16-bit word, so dispatch is a flat table of
//! 65536 function pointers built once up front. One array index replaces a tree
//! of nested matches, and "is this opcode legal" stops being logic scattered
//! across handlers: illegal entries simply point at the illegal-instruction
//! handler.

use crate::cpu::M68k;
use crate::Bus;

/// A handler executes one instruction and returns the cycles it consumed.
/// The opcode word has already been consumed from the prefetch queue.
pub type Handler = fn(&mut M68k, &mut dyn Bus, u16) -> u32;

/// Every opcode not claimed by a real handler lands here, so an unimplemented
/// instruction fails identifiably rather than silently doing nothing.
fn unimplemented_op(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    crate::exception::illegal_instruction(cpu, bus, op)
}

/// The dispatch table, owned by the caller.
///
/// Built once and shared. Keeping it out of a global avoids `std`
/// synchronisation primitives in a `no_std`-friendly crate and keeps the core
/// free of hidden state.
///
/// The table is heap-allocated (`Box`) because 65536 function pointers (512 KB)
/// would overflow the stack if constructed locally.
pub struct Decoder {
    table: Box<[Handler; 65536]>,
}

impl Decoder {
    pub fn new() -> Self {
        // Allocate on the heap to avoid a 512 KB stack frame.
        let mut v: Vec<Handler> = vec![unimplemented_op as Handler; 65536];
        register_all(&mut v);
        let boxed: Box<[Handler; 65536]> = v
            .into_boxed_slice()
            .try_into()
            .expect("vec has exactly 65536 elements");
        Self { table: boxed }
    }

    #[inline]
    pub fn dispatch(&self, op: u16) -> Handler {
        self.table[op as usize]
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Each instruction module contributes its opcodes here. Later tasks extend it.
fn register_all(_t: &mut [Handler]) {
    // Task 5 onward fills this in.
}
