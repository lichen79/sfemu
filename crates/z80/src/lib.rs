//! A Zilog Z80 CPU core.
//!
//! The core is free of dependencies, host I/O, and clock access: all state lives
//! in [`Z80`], and every access goes through [`Bus`]. That keeps it
//! deterministic, snapshot-able, and usable from WASM — the same constraint
//! `m68k` honours.
//!
//! # Why this core has cycle costs and `m68k` does not
//!
//! `m68k` has no cycle table because the 68000 obeys a measured law: every bus
//! access is four cycles, so `cycles = 4 * accesses + idle`. **The Z80 obeys no
//! such law.** An opcode fetch is 4 T-states, a memory access 3, and the
//! internal cycles are per-instruction. So each handler here returns its own
//! T-state count, and those counts are taken from the vector suite rather than
//! from a table someone typed — every one of 1,604 files carries a `cycles`
//! array that is the authority.
//!
//! # No sound here
//!
//! This is sub-project D1. It emulates the CPU that *will* drive the YM2151 and
//! the OKI sample player, and it makes no sound of its own. D2 wires it to the
//! board; D3 produces audio.

#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![forbid(unsafe_code)]
// A public module's docs linking to a private item renders as a dead link and
// clippy cannot see it — four such bugs survived thirteen tasks in `m68k` and
// were each found only by someone running `cargo doc`. Denying it fails the
// build instead.
#![deny(rustdoc::private_intra_doc_links)]

pub mod bus;
pub mod cpu;

pub use bus::Bus;
pub use cpu::Z80;
