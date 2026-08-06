//! A Motorola 68000 CPU core.
//!
//! The core is deliberately free of dependencies, host I/O, and clock access:
//! all state lives in [`M68k`], and every memory access goes through [`Bus`].
//! That keeps it deterministic, snapshot-able, and usable from WASM.
//!
//! # No `timing` module
//!
//! The plan called for one (`timing::ea_cycles`), and it is deliberately absent.
//! Cycle counts are not a function of the addressing mode alone: the measured
//! law is `cycles = 4 * (non-idle bus accesses) + (idle cycles)`, so the count
//! falls out of the access sequence a handler already has to schedule. A lookup
//! table keyed on the EA would have to be consulted *and* then corrected for
//! every fault path, faulting size, and write order — two sources of truth that
//! can disagree.
//!
//! Each handler therefore accumulates its own cycles alongside the bus schedule
//! it emits. See `ops::move_`'s module docs for the schedule model this follows.

#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![forbid(unsafe_code)]

pub mod bus;
pub mod cpu;
pub mod decode;
pub mod ea;
pub mod exception;
pub mod flags;
pub mod ops;

#[cfg(feature = "std")]
pub mod disasm;

pub use bus::Bus;
pub use cpu::M68k;
pub use ea::Size;
