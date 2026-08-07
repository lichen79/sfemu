//! CPS-1 video: tiles, sprites, and the palette, rendered to a buffer.
//!
//! Zero dependencies. This crate knows nothing about the 68000, the bus, or a
//! ROM set: the entry point takes borrowed slices, so a test can construct a
//! screenful of state directly without booting a machine.
//!
//! # No window
//!
//! The output is a framebuffer of palette pens plus a converted RGB buffer.
//! Nothing here opens a display — that is sub-project E's. A renderer reachable
//! only through a window could only be checked by looking at it, and "it looks
//! right" is not a test.
//!
//! # This crate holds no ROM
//!
//! The graphics ROM arrives as a byte slice the caller supplies. No ROM is
//! bundled, fetched, or committed, including as a test fixture: every test here
//! writes the handful of tile bytes it needs.
//!
//! Hardware facts are cited to MAME `master`,
//! `src/mame/capcom/{cps1.h,cps1.cpp,cps1_v.cpp}` (BSD-3-Clause, Paul Leaman),
//! read 2026-08-07.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![deny(rustdoc::private_intra_doc_links)]

pub mod regs;
pub mod tiles;

/// Visible pixels per line (`cps1.h:41-43`: HBSTART 448 − HBEND 64).
pub const WIDTH: usize = 384;
/// Visible lines per frame (`cps1.h:45-47`: VBSTART 240 − VBEND 16).
pub const HEIGHT: usize = 224;
