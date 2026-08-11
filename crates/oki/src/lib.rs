//! The OKI MSM6295 ADPCM sound chip, the sample player on CPS-1's sound board.
//!
//! Two layers, neither of which knows anything about a host or a clock:
//! [`adpcm`] is the per-nibble decoder, and (from Task 3) `chip` is the
//! four-voice chip with its command protocol and volume table. Driving them at
//! the right rate is `machine`'s job.
//!
//! # What verifies this
//!
//! Like [`ym2151`] and unlike [`m68k`], the MSM6295 has no published vector
//! suite, so the ground truth is *generated*: `testrunner` links MAME's own
//! `okiadpcm.cpp` (BSD-3, (C) Andrew Gardner and Aaron Giles), runs a
//! deterministic command script against a synthetic sample ROM, and records 512
//! samples per case for this crate to reproduce exactly.
//!
//! The step table is the exception, and deliberately so. It is a literal checked
//! against an independent closed form rather than against the reference, because
//! the reference's own construction (`floor(16 * 1.1^step)`) is reproducible here
//! while the obvious integer shortcut is not -- see [`adpcm::STEP_TABLE`].
//!
//! [`m68k`]: ../m68k/index.html
//! [`ym2151`]: ../ym2151/index.html

#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// A public module's docs linking to a private item renders as a dead link and
// clippy cannot see it -- four such bugs survived thirteen tasks in `m68k` and were
// each found only by someone running `cargo doc`. Denying it fails the build.
#![deny(rustdoc::private_intra_doc_links)]

pub mod adpcm;
pub mod chip;

pub use adpcm::Adpcm;
pub use chip::{Oki, Voice, VOICES};
