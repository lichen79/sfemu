//! Dev-only harness driving this workspace's CPU cores against the
//! SingleStepTests vector suites: `m68k` against m68000, and `z80` against z80.
//!
//! Two suites, two formats. The m68000 one has an upstream binary rendering that
//! [`binfmt`] reads; the Z80 one is JSON only, so [`z80fmt`] defines a binary form
//! and the fetcher converts as it downloads. Neither suite's data is committed —
//! `testdata/` is gitignored, and a missing file fails loudly naming the fetch
//! command rather than skipping.

pub mod binfmt;
pub mod runner;
pub mod testbus;
pub mod z80files;
pub mod z80fmt;
pub mod z80json;
