//! A Motorola 68000 CPU core.
//!
//! The core is deliberately free of dependencies, host I/O, and clock access:
//! all state lives in [`M68k`], and every memory access goes through [`Bus`].
//! That keeps it deterministic, snapshot-able, and usable from WASM.

#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![forbid(unsafe_code)]

pub mod bus;
pub mod cpu;
pub mod decode;
pub mod exception;

pub use bus::Bus;
pub use cpu::M68k;
