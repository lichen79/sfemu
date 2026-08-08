//! Everything a frontend decides, with no window.
//!
//! # The display boundary
//!
//! A window cannot be asserted about: `cargo test` has no display, and "the right
//! pixels reached the glass" is not something a test can read back. So every
//! decision a frontend makes — how many frames this host tick owes, which board
//! input a key is, what colour a pen is, what bytes a save state is — lives here,
//! in a crate that has never heard of a window. The module that talks to the
//! windowing library lives in `sfemu` and makes no decisions at all.
//!
//! **The rule: no logic behind the display boundary.** A decision made inside the
//! module that calls the windowing library cannot be tested, so it must not be
//! made there.
//!
//! # No clock
//!
//! Nothing here reads a clock. [`FramePacer::tick`] is *given* the elapsed
//! nanoseconds, which is what lets a test drive it through a stalled host and
//! assert exactly how many frames it asks for. The one real clock read in the
//! project is in `sfemu`'s display module.
//!
//! # This crate holds no ROM
//!
//! No ROM is bundled, fetched, or committed, including as a test fixture. The
//! tests here build their machine from a program written inline.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![deny(rustdoc::private_intra_doc_links)]

pub mod font;
pub mod keys;
pub mod pace;
pub mod pixels;
pub mod state;

pub use keys::{Actions, Controls, Key, KeySet};
pub use pace::{FramePacer, FRAME_NS, MAX_CATCH_UP};
pub use pixels::pens_to_argb;
pub use state::{decode, encode, StateError, BOARD_SF2, MAGIC, VERSION};
