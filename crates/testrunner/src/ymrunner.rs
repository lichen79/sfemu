//! Replays one YM2151 vector case against this workspace's core.
//!
//! One sample at a time, in lockstep with the generator: apply every write due at
//! this index, generate exactly one sample, read the status. That order is the
//! generator's own (`ymgen.cpp` applies the sample's writes, advances the host clock,
//! then calls `generate` and `read_status`), and it is not interchangeable — a
//! runner that applied a case's whole script up front would key off before the first
//! sample and render 512 samples of silence for every case.
//!
//! # It stops at the first divergence
//!
//! 512 samples times three fields is 1,536 comparisons per case, and once a core has
//! drifted every later sample differs too. The first one is the only informative
//! one, so [`run_case`] returns it and stops. `reportym --case N` prints the samples
//! around it.
//!
//! # Failures are values, not strings
//!
//! Unlike [`crate::z80runner`], whose diffs are prose read once by a person, a
//! YM2151 mismatch is three numbers and a field name, and the suite's corruption
//! test asserts on all four. A struct is what makes that assertion possible.

use crate::ymfmt::Case;
use ym2151::Ym2151;

/// Which of a sample's three compared fields diverged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    /// The left output.
    Left,
    /// The right output. Compared separately because the OPM's panning is
    /// per-channel, and a core that mixed both channels into one bus would still
    /// match on the left.
    Right,
    /// The status register read after the sample. Inaudible, and therefore the only
    /// thing that tests the timers at all — see [`crate::ymfmt`]'s module docs.
    Status,
}

impl core::fmt::Display for Field {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            Field::Left => "left",
            Field::Right => "right",
            Field::Status => "status",
        };
        f.write_str(s)
    }
}

/// The first sample and field at which the core and ymfm disagreed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mismatch {
    /// The sample index, 0-based.
    pub sample: usize,
    /// Which field.
    pub field: Field,
    /// What the vector file holds — ymfm's value.
    pub want: i32,
    /// What this workspace's core produced.
    pub got: i32,
}

impl core::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "sample {} {}: want {} got {}",
            self.sample, self.field, self.want, self.got
        )
    }
}

/// The outcome of one case.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CaseResult {
    /// Whether every sample of every field matched.
    pub ok: bool,
    /// The first divergence, present exactly when `ok` is false.
    pub first_mismatch: Option<Mismatch>,
}

/// Runs one case and reports the first divergence, if any.
///
/// The chip is fresh: [`Ym2151::new`] is post-reset, which is the state ymfm's
/// constructor leaves its own chip in.
#[must_use]
pub fn run_case(case: &Case) -> CaseResult {
    let mut chip = Ym2151::new();
    let mut buf = [(0i16, 0i16); 1];
    // A cursor rather than a filter per sample: the writes are sorted by `at_sample`,
    // so this is one pass over both sequences and it preserves file order among the
    // writes sharing an index — which matters, because a case can write a register
    // and then the key-on that acts on it at the same sample.
    let mut w = 0usize;

    for (i, want) in case.samples.iter().enumerate() {
        let at = u16::try_from(i).unwrap_or(u16::MAX);
        while w < case.writes.len() && case.writes[w].at_sample == at {
            chip.write(case.writes[w].reg, case.writes[w].val);
            w += 1;
        }
        chip.generate(&mut buf);
        let (left, right) = buf[0];
        let status = chip.read_status();

        let bad = if left != want.left {
            Some((Field::Left, i32::from(want.left), i32::from(left)))
        } else if right != want.right {
            Some((Field::Right, i32::from(want.right), i32::from(right)))
        } else if status != want.status {
            Some((Field::Status, i32::from(want.status), i32::from(status)))
        } else {
            None
        };
        if let Some((field, want, got)) = bad {
            return CaseResult {
                ok: false,
                first_mismatch: Some(Mismatch {
                    sample: i,
                    field,
                    want,
                    got,
                }),
            };
        }
    }

    // A write the replay never reached means the case declares a write past its last
    // sample. That is a corrupt file rather than a core defect, but silently ignoring
    // it would let a generator bug shorten every case's script unnoticed.
    if w < case.writes.len() {
        return CaseResult {
            ok: false,
            first_mismatch: Some(Mismatch {
                sample: case.samples.len(),
                field: Field::Status,
                want: i32::try_from(case.writes.len()).unwrap_or(i32::MAX),
                got: i32::try_from(w).unwrap_or(i32::MAX),
            }),
        };
    }

    CaseResult {
        ok: true,
        first_mismatch: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ymfmt::{Sample, Write};

    /// Builds a case by recording what this workspace's core does with a script.
    ///
    /// **This is not a test of the core.** Expected values derived from the thing
    /// under test cannot fail, and that pattern has shipped six times on this branch.
    /// What it tests is the *runner*: the write cursor, the per-sample ordering, and
    /// the three comparisons. The core's agreement with ymfm is tested in
    /// `tests/ymsuite.rs` against the generated suite, and by
    /// `four_patches_match_ymfm_sample_for_sample` in `ym2151::chip` — both against
    /// values ymfm produced. Every assertion below is about the runner reacting to a
    /// change in the *file*, with the core held fixed.
    fn record(writes: Vec<Write>, samples: usize) -> Case {
        let mut chip = Ym2151::new();
        let mut buf = [(0i16, 0i16); 1];
        let mut out = Vec::with_capacity(samples);
        let mut w = 0usize;
        for i in 0..samples {
            let at = u16::try_from(i).expect("small");
            while w < writes.len() && writes[w].at_sample == at {
                chip.write(writes[w].reg, writes[w].val);
                w += 1;
            }
            chip.generate(&mut buf);
            out.push(Sample {
                left: buf[0].0,
                right: buf[0].1,
                status: chip.read_status(),
            });
        }
        let final_status = out.last().expect("samples").status;
        Case {
            seed: 0,
            writes,
            samples: out,
            final_status,
        }
    }

    /// A patch that makes noise, keys off partway, and runs a timer.
    fn script() -> Vec<Write> {
        let w = |at: u16, reg: u8, val: u8| Write {
            at_sample: at,
            reg,
            val,
        };
        vec![
            // Timer A near the top of its 10-bit range so it overflows inside 64
            // samples, and enabled, so the status byte is not a column of zeros.
            w(0, 0x10, 0xF8),
            w(0, 0x11, 0x03),
            w(0, 0x14, 0x05),
            // Channel 0, algorithm 7 (all four operators to the output), both pans.
            w(0, 0x20, 0xC7),
            w(0, 0x28, 0x4A),
            w(0, 0x30, 0x00),
            w(0, 0x40, 0x01),
            w(0, 0x48, 0x01),
            w(0, 0x50, 0x01),
            w(0, 0x58, 0x01),
            w(0, 0x60, 0x10),
            w(0, 0x68, 0x10),
            w(0, 0x70, 0x10),
            w(0, 0x78, 0x10),
            w(0, 0x80, 0x1F),
            w(0, 0x88, 0x1F),
            w(0, 0x90, 0x1F),
            w(0, 0x98, 0x1F),
            w(0, 0xE0, 0x0A),
            w(0, 0xE8, 0x0A),
            w(0, 0xF0, 0x0A),
            w(0, 0xF8, 0x0A),
            w(0, 0x08, 0x78),
            // The key-off, mid-window.
            w(32, 0x08, 0x00),
        ]
    }

    /// A recorded case replays exactly.
    #[test]
    fn a_recorded_case_replays() {
        let case = record(script(), 64);
        // The premise the rest of this module rests on: the case is audible and its
        // status byte is not constant. Without both, every corruption below would
        // be flipping a zero to a one in a field nothing reads.
        assert!(
            case.samples.iter().any(|s| s.left != 0 && s.right != 0),
            "the recorded case must be audible on both channels"
        );
        assert!(
            case.samples
                .iter()
                .any(|s| s.status != case.samples[0].status),
            "and its status must change"
        );
        let r = run_case(&case);
        assert!(r.ok, "{:?}", r.first_mismatch);
        assert_eq!(r.first_mismatch, None);
    }

    /// Each of the three fields is compared, at the sample that was changed.
    #[test]
    fn each_field_is_compared_and_the_sample_is_named() {
        let base = record(script(), 64);
        let audible = base
            .samples
            .iter()
            .position(|s| s.left != 0 && s.right != 0)
            .expect("audible");

        let mut c = base.clone();
        c.samples[audible].left = c.samples[audible].left.wrapping_add(1);
        let m = run_case(&c).first_mismatch.expect("left is compared");
        assert_eq!(m.field, Field::Left);
        assert_eq!(m.sample, audible);
        assert_eq!(m.want, i32::from(c.samples[audible].left));
        assert_eq!(m.got, i32::from(base.samples[audible].left));

        let mut c = base.clone();
        c.samples[audible].right = c.samples[audible].right.wrapping_add(1);
        let m = run_case(&c).first_mismatch.expect("right is compared");
        assert_eq!(m.field, Field::Right);
        assert_eq!(m.sample, audible);

        let mut c = base.clone();
        c.samples[audible].status ^= 0x01;
        let m = run_case(&c).first_mismatch.expect("status is compared");
        assert_eq!(m.field, Field::Status);
        assert_eq!(m.sample, audible);
    }

    /// The *first* divergence is the one reported.
    ///
    /// A runner that returned the last mismatch, or an arbitrary one, would make
    /// `reportym --case N` point at a sample hundreds of samples downstream of the
    /// defect — which is the whole reason this returns early.
    #[test]
    fn the_first_divergence_is_the_one_reported() {
        let mut c = record(script(), 64);
        let a = c.samples.iter().position(|s| s.left != 0).expect("audible");
        c.samples[a].left = c.samples[a].left.wrapping_add(1);
        c.samples[a + 1].left = c.samples[a + 1].left.wrapping_add(1);
        c.samples[63].left = c.samples[63].left.wrapping_add(1);
        assert_eq!(run_case(&c).first_mismatch.expect("fails").sample, a);
    }

    /// Left is compared before right, and both before status.
    ///
    /// The order is what makes a report legible: a core with a broken DAC diverges on
    /// every field at once, and naming the audio rather than the status byte is the
    /// difference between a one-line diagnosis and a wrong one.
    #[test]
    fn the_fields_are_compared_in_order() {
        let base = record(script(), 64);
        let i = base
            .samples
            .iter()
            .position(|s| s.left != 0 && s.right != 0)
            .expect("audible");
        let mut c = base.clone();
        c.samples[i].left = c.samples[i].left.wrapping_add(1);
        c.samples[i].right = c.samples[i].right.wrapping_add(1);
        c.samples[i].status ^= 0x01;
        assert_eq!(
            run_case(&c).first_mismatch.expect("fails").field,
            Field::Left
        );

        let mut c = base.clone();
        c.samples[i].right = c.samples[i].right.wrapping_add(1);
        c.samples[i].status ^= 0x01;
        assert_eq!(
            run_case(&c).first_mismatch.expect("fails").field,
            Field::Right
        );
    }

    /// A write's timing is honoured, not just its presence.
    ///
    /// Three mutations, each of which a runner that ignored `at_sample` would accept:
    /// dropping the timed key-off, moving it earlier, and moving it later. This is
    /// the only test here that exercises the write cursor rather than the comparison.
    #[test]
    fn a_writes_timing_is_honoured() {
        let base = record(script(), 64);

        let mut c = base.clone();
        c.writes.retain(|w| w.at_sample == 0);
        assert!(!run_case(&c).ok, "dropping the key-off must be caught");

        let mut c = base.clone();
        let last = c.writes.len() - 1;
        assert_eq!(c.writes[last].at_sample, 32, "the key-off is last");
        c.writes[last].at_sample = 16;
        assert!(!run_case(&c).ok, "an early key-off must be caught");

        let mut c = base.clone();
        c.writes[last].at_sample = 48;
        assert!(!run_case(&c).ok, "a late key-off must be caught");
    }

    /// A write past the last sample is a corrupt file, not a pass.
    #[test]
    fn a_write_that_is_never_reached_fails() {
        let mut c = record(script(), 64);
        c.writes.push(Write {
            at_sample: 999,
            reg: 0x08,
            val: 0x78,
        });
        let m = run_case(&c).first_mismatch.expect("must fail");
        assert_eq!(m.sample, c.samples.len(), "reported past the end");
        assert_eq!(m.want, i32::try_from(c.writes.len()).expect("small"));
    }

    /// A mismatch prints as one line naming the sample, the field and both values.
    #[test]
    fn a_mismatch_prints_as_one_line() {
        let m = Mismatch {
            sample: 42,
            field: Field::Right,
            want: -1234,
            got: 5678,
        };
        assert_eq!(format!("{m}"), "sample 42 right: want -1234 got 5678");
        assert_eq!(format!("{}", Field::Left), "left");
        assert_eq!(format!("{}", Field::Status), "status");
    }
}
