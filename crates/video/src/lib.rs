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
//! # Positions are raster coordinates, not visible-frame coordinates
//!
//! Every position the hardware hands this crate — a sprite's x and y from the
//! object table, a layer's scroll registers, a row-scroll table entry — is
//! measured in the **full raster**, 512×262, whose origin lies inside the
//! blanking region. The visible 384×224 window is the sub-rectangle starting at
//! ([`VISIBLE_X`], [`VISIBLE_Y`]), so a pen at raster (`rx`, `ry`) belongs at
//! framebuffer pixel (`rx - VISIBLE_X`, `ry - VISIBLE_Y`) and is dropped when
//! that falls outside the frame.
//!
//! MAME gets this for free and so never states it: `set_raw(..., CPS_HTOTAL,
//! CPS_HBEND, CPS_HBSTART, CPS_VTOTAL, CPS_VBEND, CPS_VBSTART)`
//! (`cps1.cpp:3925`) makes the screen bitmap the whole raster, every primitive
//! is drawn at its raw coordinate, and `cliprect` — `[HBEND, HBSTART-1] ×
//! [VBEND, VBSTART-1]` — does the cropping. Treating those raw coordinates as
//! visible-frame coordinates puts every layer and every sprite 64 pixels right
//! and 16 pixels down of where the hardware puts it.
//!
//! Three independent readings of the reference fix the offset at exactly
//! (64, 16):
//!
//! - **The flip pivots.** A flipped sprite is drawn at `512 - 16 - sx`,
//!   `256 - 16 - sy` (`cps1_v.cpp:2730`) — a mirror about 511 and 255. Those are
//!   raster pivots: HBEND 64 + HBSTART−1 447 = 511, and VBEND 16 + VBSTART−1 239
//!   = 255. In visible-frame coordinates the pivots would have to be 383 and 223.
//! - **The stars.** `cps1_render_stars` builds a raster position, masks it to
//!   `& 0x1ff` / `& 0xff`, then tests `cliprect.contains(sx, sy)`
//!   (`cps1_v.cpp:2899`, `:2925`). A star at x = 40 is discarded as off-screen,
//!   which is only true if 40 is a raster column inside left blanking.
//! - **The bootleg offsets.** `scroll1xoff = 0xffc0` (`cps1_v.cpp:2284`) is −64,
//!   the correction a board needs when it does *not* already sit in this
//!   coordinate space. A standard board's offset is 0 because the raster origin
//!   is the coordinate system the hardware natively uses.
//!
//! The screen flip is still a single mirror of the finished visible frame: the
//! visible window is symmetric within the raster pivots, so mirroring the crop
//! equals cropping the mirror.
//!
//! Hardware facts are cited to MAME `master`,
//! `src/mame/capcom/{cps1.h,cps1.cpp,cps1_v.cpp}` (BSD-3-Clause, Paul Leaman),
//! read 2026-08-07.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![deny(rustdoc::private_intra_doc_links)]

pub mod bank;
pub mod layers;
pub mod palette;
pub mod regs;
pub mod sprites;
pub mod tiles;

/// Visible pixels per line (`cps1.h:41-43`: HBSTART 448 − HBEND 64).
pub const WIDTH: usize = 384;
/// Visible lines per frame (`cps1.h:45-47`: VBSTART 240 − VBEND 16).
pub const HEIGHT: usize = 224;

/// Raster column of the visible window's left edge (`cps1.h:42`: HBEND).
pub const VISIBLE_X: i32 = 64;
/// Raster row of the visible window's top edge (`cps1.h:46`: VBEND).
pub const VISIBLE_Y: i32 = 16;
