//! Street Fighter's board — a pre-CPS design, not a CPS-1 one.
//!
//! Driver: MAME `src/mame/capcom/sf.cpp` (© Olivier Galibert), read at tag
//! `mame0261`. Not `cps1.cpp`: there is no CPS-A, no CPS-B, no gfxram and no
//! bank mapper, so [`crate::config::BoardConfig`] has no analogue here at all.
//! The palette is plain RAM at 0xB00000 and the I/O block is plain address
//! decoding at 0xC00000.
//!
//! Only the parent set `sf` is emulated: `GAME(1987, sf, 0, sfus, sfus, …)`
//! (`sf.cpp:1421`), the one set with neither an i8751 protection MCU nor
//! pneumatic buttons.

pub mod adpcm2;
pub mod board;
pub mod inputs;
pub mod machine;
pub mod mix;
pub mod msm5205;
pub mod sound;

pub use adpcm2::{Adpcm2Board, Adpcm2Trace};
pub use machine::{Sf1, MSM_TICKS_PER_LINE};
pub use mix::mix;
pub use msm5205::Msm5205;
pub use sound::{FmBoard, FmTrace};

/// A video subsystem with no graphics, for this crate's tests.
///
/// Every SF1 test in `machine` drives the schedule rather than the pixels, and a
/// fixture that built graphics would make those files' runtime decoding.
#[cfg(test)]
pub(crate) fn test_video() -> video::sf1::Sf1Video {
    video::sf1::Sf1Video::new(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new())
}
