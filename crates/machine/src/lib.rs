//! The CPS-1 arcade board: memory map, frame schedule, and vblank interrupt.
//!
//! Zero dependencies beyond [`m68k`] and [`video`], both dependency-free crates
//! of this workspace. No `std` requirement in the simulation path, no host I/O,
//! no clock access — the same constraints sub-project A honoured, for the same
//! reason: WASM and rollback netplay stay nearly free.
//!
//! # This crate holds no ROM
//!
//! [`Board::new`] takes a byte slice. Assembling that slice from a user-supplied
//! ROM set is `romset`'s job, and `machine` does not depend on `romset`. No ROM is
//! bundled, fetched, or committed — including as a test fixture. Every test here
//! builds its program inline.
//!
//! Board facts are cited to MAME `master`,
//! `src/mame/capcom/{cps1.h,cps1.cpp,cps1_v.cpp}` (BSD-3-Clause, Paul Leaman),
//! read 2026-08-07.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![deny(rustdoc::private_intra_doc_links)]

pub mod board;
pub mod config;
pub mod cps1;
pub mod inputs;
pub mod snapshot;
pub mod sound;
pub mod timing;
pub mod trace;

/// The video subsystem, re-exported.
///
/// A host that drives a [`Cps1`] reads its framebuffer, and reading it means
/// naming [`video::palette::BACKGROUND_PEN`] and [`video::WIDTH`]. Re-exporting is
/// how `sfemu` does that without taking a second dependency edge on the same
/// crate — the same reasoning that keeps `m68k` out of `sfemu`'s manifest.
pub use video;

/// The CPU crate, re-exported, for the same reason [`video`] is.
///
/// A save-state codec has to name [`m68k::M68k`] — [`MachineState::cpu`] is one —
/// and it has to construct one to fill from a file. Re-exporting is how `frontend`
/// does that while keeping its manifest one dependency wide: a second edge on
/// `m68k` would let `frontend` reach past `machine` into the core, which is the
/// coupling the boundary exists to prevent.
pub use m68k;

pub use board::Board;
pub use config::BoardConfig;
pub use cps1::Cps1;
pub use inputs::{Inputs, PlayerInput};
pub use snapshot::MachineState;
pub use timing::Timing;
pub use trace::{Trace, UnmappedLog};
