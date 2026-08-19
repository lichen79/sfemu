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

pub mod board;
pub mod inputs;
pub mod msm5205;

pub use msm5205::Msm5205;
