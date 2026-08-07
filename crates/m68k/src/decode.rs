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
/// # ⚠️ This is a 512 KB value and [`Decoder::new`] builds it on the stack
///
/// `size_of::<Decoder>()` is **524,288 bytes** — 65,536 function pointers at 8
/// bytes each — against `size_of::<M68k>()` of **88**. That asymmetry is the
/// whole design and it is a good trade (O(1) dispatch, and "is this opcode legal"
/// needs no logic at all because illegal entries point at the illegal-instruction
/// handler). The cost is only that construction has a stack requirement, which
/// callers have to know about:
///
/// - **`Decoder::new()` needs a stack of at least 1 MB.** Measured on a spawned
///   thread with `stack_size`: 512, 640, 768 and 896 KB all abort; 1024 KB and
///   above succeed.
/// - **`Box::new(Decoder::new())` does not help.** The temporary is built on the
///   stack and *then* moved into the allocation, so the boxed form aborts at
///   exactly the same four sizes. `Box::default()` is the same. A constructor that
///   avoided this would have to fill the table *through* the `Box` rather than
///   move a finished value into it; there is no such constructor here because no
///   caller has needed one.
/// - **The failure is not a catchable panic.** A Rust stack overflow is
///   `fatal runtime error: stack overflow, aborting` — the process dies, and
///   `catch_unwind` and `JoinHandle::join` never see it. So this cannot be probed
///   defensively at runtime; it has to be arranged for.
/// - [`M68k::step`](crate::M68k::step) inherits the requirement, since its
///   `OnceLock` calls `Decoder::new` on whichever thread reaches it first.
///
/// **Recommended: construct once on the main thread, before spawning**, and pass
/// `&Decoder` to [`M68k::step_with`](crate::M68k::step_with). The default main
/// thread gets 8 MB, which is why this never bites in testing. It bites on a
/// thread spawned with a custom `stack_size`, and on `wasm32`, whose default stack
/// is 1 MB — comfortable enough to succeed, but with the whole margin spent here.
pub struct Decoder {
    table: [Handler; 65536],
}

impl Decoder {
    /// Builds the table. **Needs ≥1 MB of stack** — see the type's doc; `Box` does
    /// not avoid it, and overflow aborts the process rather than panicking.
    pub fn new() -> Self {
        let mut table = [unimplemented_op as Handler; 65536];
        register_all(&mut table);
        Self { table }
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
fn register_all(t: &mut [Handler; 65536]) {
    crate::ops::register_all(t);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the two sizes [`Decoder`]'s doc quotes, so the stack paragraph cannot
    /// go stale silently.
    ///
    /// Both are **literals**. Writing the expectation as
    /// `65536 * size_of::<Handler>()` would restate the definition and pass for
    /// any table size, which is the shape that lets a documented figure drift away
    /// from the code describing it. Gated to 64-bit because a 32-bit host halves
    /// the pointer and so the figure — the doc's ≥1 MB advice is about the 64-bit
    /// build the project actually ships.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_decoder_is_512_kb_and_the_cpu_is_not() {
        assert_eq!(
            core::mem::size_of::<Decoder>(),
            524_288,
            "the doc's 512 KB / ≥1 MB stack paragraph is keyed to this figure"
        );
        assert_eq!(core::mem::size_of::<M68k>(), 88);
    }
}
