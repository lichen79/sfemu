//! A cycle-accurate Yamaha YM2151 (OPM), the FM chip on CPS-1's sound board.
//!
//! The core is free of dependencies, host I/O, and clock access: all state lives in
//! one struct and samples are written into a caller-supplied slice. That keeps it
//! deterministic, snapshot-able, and usable from WASM — the same constraint `m68k`
//! and `z80` honour.
//!
//! # What verifies this
//!
//! Unlike [`m68k`] and [`z80`], the OPM has **no published vector suite**: FM
//! synthesis has no per-instruction granularity to enumerate, and no equivalent of
//! SingleStepTests exists. So this crate's ground truth is *generated* —
//! `testrunner` links ymfm (BSD-3, © 2021 Aaron Giles, the implementation MAME
//! uses), runs a deterministic register script, and records 512 samples per case
//! for this crate to reproduce sample-for-sample.
//!
//! That shifts the risk: a generated suite can produce cases that cannot fail. Three
//! measurements shaped it, each recorded in the spec — a random register script is
//! silent (0 non-zero samples in 500 cases), a held note never exercises release
//! rate (undetected in 0 of 200 until every case keys off), and timer state is not
//! audible at all (0 of 200 until the record gained a per-sample status byte). The
//! suite re-asserts all three as premises, so a regeneration that loses
//! discriminating power fails rather than passing vacuously.
//!
//! # No sound
//!
//! This is sub-project D2. `generate` writes samples into a caller-supplied slice;
//! this crate owns no audio device, no buffer, and no thread. D3 produces audio.
//!
//! [`m68k`]: ../m68k/index.html
//! [`z80`]: ../z80/index.html

#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![forbid(unsafe_code)]
// A public module's docs linking to a private item renders as a dead link and
// clippy cannot see it — four such bugs survived thirteen tasks in `m68k` and were
// each found only by someone running `cargo doc`. Denying it fails the build.
#![deny(rustdoc::private_intra_doc_links)]

pub mod operator;
pub mod regs;
pub mod tables;
