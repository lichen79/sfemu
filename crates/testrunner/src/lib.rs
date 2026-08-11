//! Dev-only harness driving this workspace's emulated chips against vector
//! suites: `m68k` and `z80` against SingleStepTests, `ym2151` against ymfm, and
//! `oki` against MAME's own ADPCM decoder.
//!
//! Four suites, four formats, and two provenances. The CPU suites are
//! *published*: the m68000 one has an upstream binary rendering that [`binfmt`]
//! reads, and the Z80 one is JSON only, so [`z80fmt`] defines a binary form and
//! the fetcher converts as it downloads. The two sound chips have no published
//! suite, so theirs are *generated* against the reference implementation —
//! [`ymfmt`] holds that contract for the OPM and [`okifmt`] for the MSM6295.
//!
//! No suite's data is committed — `testdata/` is gitignored, and a missing file
//! fails loudly naming the fetch or generate command rather than skipping.

pub mod binfmt;
pub mod okifiles;
pub mod okifmt;
pub mod runner;
pub mod testbus;
pub mod ymfiles;
pub mod ymfm;
pub mod ymfmt;
pub mod ymrunner;
pub mod z80bus;
pub mod z80files;
pub mod z80fmt;
pub mod z80json;
pub mod z80runner;
