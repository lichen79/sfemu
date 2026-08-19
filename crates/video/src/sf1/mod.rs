//! Street Fighter 1 video: a pre-CPS board, and almost nothing is shared.
//!
//! SF1 has no CPS-A and no CPS-B, so there is no register file to consult, no
//! bank mapper, no layer-priority word and no gfxram — the tilemaps for the two
//! scrolling planes live in a **ROM** region, the palette is plain 4-4-4 RAM,
//! and the four graphics layouts are two distinct `gfx_layout`s rather than one
//! parameterized by tile size.
//!
//! What *is* shared is the geometry: [`crate::WIDTH`], [`crate::HEIGHT`],
//! [`crate::VISIBLE_X`] and [`crate::VISIBLE_Y`] are the same 384×224 window at
//! (64, 16) inside a raster whose origin is in blanking, so this module inherits
//! the crate documentation's coordinate rule unchanged. SF1's raster is 512×256
//! where CPS-1's is 512×262, which changes nothing about the offset.
//!
//! Hardware facts are cited to MAME `mame0261`, `src/mame/capcom/sf.cpp`
//! (BSD-3-Clause, Olivier Galibert), and to `src/emu/{drawgfx,digfx,tilemap,
//! emupal}.cpp` for the framework behaviour SF1 relies on. Read 2026-08-17.

pub mod gfx;
pub mod palette;
pub mod tilemap;
