//! Replays one OKI vector case against `oki`.
//!
//! This is not a test of the core in the sense that matters: it replays a case
//! and reports the first place the core disagrees with the reference. The
//! expected values come from MAME's own decoder, recorded by `genoki` -- never
//! from the code under test. Expected values derived from the thing under test
//! cannot fail, and that pattern has shipped six times on this branch.
//!
//! # `voices` is read before the step, `status` after
//!
//! The two masks are not the same mask, and the difference is measured: they
//! disagree at 1,907 of the suite's 512,000 samples, first at case 0 sample 127.
//! [`crate::okifmt::Sample::voices`] is who *sounded during* the sample -- the
//! set the generator's step loop iterates -- and
//! [`crate::okifmt::Sample::status`] is who is *still playing after* it. A voice
//! whose phrase ends on this sample appears in the first and not the second.
//!
//! So this reads [`Oki::voices_playing`] **before** [`Oki::step_2x_traced`] and
//! [`Oki::status`] after. Comparing both against the post-step state, as this
//! plan's first draft did, fails on correct data at every phrase boundary.

use crate::okifmt::Case;
use oki::Oki;

/// Which field diverged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The mono output sample.
    Mono,
    /// The status byte, read after the sample.
    Status,
    /// The mask of voices that sounded during the sample, read before it.
    Voices,
    /// The packed nibbles the voices consumed.
    ///
    /// Checked **before** the mono value, because a wrong nibble explains a
    /// wrong sample and the reverse is not true: reporting the sample first
    /// would send a reader looking in the decoder for an address-walk bug.
    Nibbles,
    /// Not a field of a sample: the file scheduled writes past its last
    /// recorded sample, so the case is not the case it claims to be.
    UnreachedWrites,
}

impl core::fmt::Display for Field {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Mono => "mono",
            Self::Status => "status",
            Self::Voices => "voices",
            Self::Nibbles => "nibbles",
            Self::UnreachedWrites => "writes consumed",
        })
    }
}

/// Where and how a case diverged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mismatch {
    /// The sample index. Equal to the case's sample count for
    /// [`Field::UnreachedWrites`], which is past the last sample by definition.
    pub sample: usize,
    /// Which field.
    pub field: Field,
    /// What the reference recorded.
    pub want: i64,
    /// What the core produced.
    pub got: i64,
}

impl core::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "sample {}: {} want {} got {}",
            self.sample, self.field, self.want, self.got
        )
    }
}

/// Replay a case, returning the first divergence.
///
/// # Errors
///
/// Returns a [`Mismatch`] at the first field that disagrees. A case whose
/// writes are not all consumed by the end of its samples is a corrupt file,
/// reported as [`Field::UnreachedWrites`] so it cannot pass silently.
pub fn run_case(case: &Case) -> Result<(), Mismatch> {
    let mut chip = Oki::new();
    let mut wi = 0usize;
    for (n, want) in case.samples.iter().enumerate() {
        while let Some(w) = case.writes.get(wi) {
            if usize::from(w.at_sample) != n {
                break;
            }
            chip.write(w.byte, &case.rom);
            wi += 1;
        }
        // Before the step: these are the voices that will sound during it.
        let got_voices = chip.voices_playing();
        // The nibbles come out of the same call that produces the sample, so
        // the core reports both and the runner checks the cause first.
        let (got_mono, got_nibbles) = chip.step_2x_traced(&case.rom);
        let fail = |field, want: i64, got: i64| {
            Err(Mismatch {
                sample: n,
                field,
                want,
                got,
            })
        };
        if got_nibbles != want.nibbles {
            return fail(
                Field::Nibbles,
                i64::from(want.nibbles),
                i64::from(got_nibbles),
            );
        }
        if got_mono != want.mono_2x {
            return fail(Field::Mono, i64::from(want.mono_2x), i64::from(got_mono));
        }
        if got_voices != want.voices {
            return fail(Field::Voices, i64::from(want.voices), i64::from(got_voices));
        }
        let got_status = chip.status();
        if got_status != want.status {
            return fail(Field::Status, i64::from(want.status), i64::from(got_status));
        }
    }
    if wi != case.writes.len() {
        // Not a pass: the file scheduled writes past the recorded samples, so
        // the case is not the case it claims to be.
        return Err(Mismatch {
            sample: case.samples.len(),
            field: Field::UnreachedWrites,
            want: case.writes.len() as i64,
            got: wi as i64,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::okifmt::{Sample, Write_};

    /// A case the chip really does produce: one voice on a four-nibble phrase,
    /// five samples, recorded by running the core and then asserting nothing
    /// about the values themselves.
    ///
    /// **This is not a test of the core.** Recording from the thing under test
    /// is legitimate here and nowhere else -- the point is to build a case the
    /// runner accepts, so that corrupting it must make the runner reject it. The
    /// suite's own expectations come from MAME, via `genoki`.
    ///
    /// The phrase is deliberately *short*: it ends at sample 3, so sample 3 has
    /// `voices == 0x01` and `status == 0xF0` and sample 4 has both zero. Without
    /// a phrase boundary in the fixture the two masks are equal at every sample,
    /// and a runner that read the wrong one would pass every test here --
    /// which is the defect the plan's own five-line fixture could not see.
    fn passing_case() -> Case {
        let mut rom = vec![0u8; 0x4000];
        // Phrase 1: 0x1000..0x1001, so count = 2 * (0x1001 - 0x1000 + 1) = 4.
        for (i, b) in [0x00u8, 0x10, 0x00, 0x00, 0x10, 0x01]
            .into_iter()
            .enumerate()
        {
            rom[8 + i] = b;
        }
        let mut s: u64 = 0x1234_5678;
        for byte in &mut rom[0x1000..0x2000] {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            *byte = s as u8;
        }
        let writes = vec![
            Write_ {
                at_sample: 0,
                byte: 0x81,
            },
            Write_ {
                at_sample: 0,
                byte: 0x10,
            },
        ];
        let mut chip = Oki::new();
        chip.write(0x81, &rom);
        chip.write(0x10, &rom);
        let samples = (0..5)
            .map(|_| {
                let voices = chip.voices_playing();
                let (mono_2x, nibbles) = chip.step_2x_traced(&rom);
                Sample {
                    mono_2x,
                    status: chip.status(),
                    voices,
                    nibbles,
                }
            })
            .collect();
        Case {
            seed: 0,
            pin7: true,
            writes,
            rom,
            samples,
        }
    }

    #[test]
    fn a_case_the_core_reproduces_passes() {
        assert_eq!(run_case(&passing_case()), Ok(()));
    }

    /// The fixture distinguishes the two masks, which is what makes the
    /// [`Field::Voices`] and [`Field::Status`] comparisons independent.
    ///
    /// Measured on the real suite: `voices` and `status & 0x0F` disagree at
    /// 1,907 of 512,000 samples, always at a phrase boundary. A fixture with no
    /// boundary in it cannot tell a runner that reads the post-step mask for
    /// both from one that reads them correctly.
    #[test]
    fn the_fixture_contains_a_phrase_boundary() {
        let c = passing_case();
        assert_eq!(c.samples.len(), 5);
        for n in 0..4 {
            assert_eq!(c.samples[n].voices, 0x01, "sample {n} must sound");
        }
        assert_eq!(
            c.samples[3].status, 0xF0,
            "the phrase's last sample sounds but leaves nothing playing"
        );
        assert_eq!(c.samples[3].voices, 0x01);
        assert_ne!(
            c.samples[3].voices,
            c.samples[3].status & 0x0F,
            "without this the two masks are interchangeable"
        );
        assert_eq!(c.samples[4].voices, 0, "and then silence");
        assert_eq!(c.samples[4].mono_2x, 0);
    }

    /// The fixture is not accidentally uniform, which would make one corruption
    /// stand in for all of them.
    #[test]
    fn the_fixture_has_something_to_compare() {
        let case = passing_case();
        assert_ne!(
            case.samples[0].mono_2x, 0,
            "a silent fixture proves nothing"
        );
        assert_ne!(case.samples[0].nibbles, 0);
        assert_eq!(case.samples[0].voices, 0x01);
        assert_eq!(case.samples[0].status, 0xF1);
    }

    /// Every compared field, corrupted one at a time. A runner missing any one
    /// comparison passes the suite while ignoring that field entirely.
    ///
    /// Corrupted at sample 3, the phrase's last: there `voices` is `0x01` and
    /// `status & 0x0F` is `0`, so the two corruptions are distinguishable and
    /// each must be reported under its own name.
    #[test]
    fn corrupting_any_field_is_caught() {
        const AT: usize = 3;
        for field in [Field::Mono, Field::Status, Field::Voices, Field::Nibbles] {
            let mut case = passing_case();
            let s = &mut case.samples[AT];
            match field {
                Field::Mono => s.mono_2x = s.mono_2x.wrapping_add(1),
                Field::Status => s.status ^= 0x01,
                Field::Voices => s.voices ^= 0x01,
                Field::Nibbles => s.nibbles ^= 0x0F,
                Field::UnreachedWrites => unreachable!("not a sample field"),
            }
            let err = run_case(&case).expect_err(&format!("corrupt {field} passed"));
            assert_eq!(
                err.field, field,
                "corrupt {field} was reported as {}",
                err.field
            );
            assert_eq!(
                err.sample, AT,
                "corrupt {field} was reported at the wrong sample"
            );
        }
    }

    /// `nibbles` is compared before `mono`, so a wrong address walk is named as
    /// one rather than as a decoder bug.
    #[test]
    fn the_nibbles_are_compared_before_the_sample() {
        let mut case = passing_case();
        case.samples[1].nibbles ^= 0x0F;
        case.samples[1].mono_2x = case.samples[1].mono_2x.wrapping_add(1);
        let err = run_case(&case).expect_err("both fields are wrong");
        assert_eq!(err.field, Field::Nibbles);
    }

    /// The *first* divergence is the one reported, not the last or an arbitrary
    /// one: `reportoki --case N` prints the samples around it, and a report
    /// pointing downstream of the defect is worse than none.
    #[test]
    fn the_first_divergence_is_the_one_reported() {
        let mut case = passing_case();
        for n in [1, 2, 4] {
            case.samples[n].mono_2x = case.samples[n].mono_2x.wrapping_add(1);
        }
        assert_eq!(run_case(&case).expect_err("fails").sample, 1);
    }

    /// A write's timing is honoured, not just its presence.
    #[test]
    fn a_writes_timing_is_honoured() {
        let base = passing_case();

        let mut case = base.clone();
        case.writes.clear();
        assert!(
            run_case(&case).is_err(),
            "dropping the start command must be caught"
        );

        let mut case = base.clone();
        case.writes[1].at_sample = 2;
        let err = run_case(&case).expect_err("a late data byte must be caught");
        assert_eq!(err.sample, 0, "the voice does not sound at sample 0");
    }

    /// A write scheduled past the recorded samples is a corrupt file, not a
    /// pass: silently ignoring it would let a truncated case look clean.
    #[test]
    fn a_write_past_the_last_sample_is_a_divergence() {
        let mut case = passing_case();
        case.writes.push(Write_ {
            at_sample: 900,
            byte: 0x08,
        });
        let err = run_case(&case).expect_err("an unreachable write must not pass");
        assert_eq!(err.sample, case.samples.len());
        assert_eq!(err.field, Field::UnreachedWrites);
        assert_eq!(err.want, 3);
        assert_eq!(err.got, 2);
    }

    /// A mismatch prints as one line naming the sample, the field and both
    /// values, and every field has a distinct name.
    #[test]
    fn a_mismatch_prints_as_one_line() {
        let m = Mismatch {
            sample: 42,
            field: Field::Nibbles,
            want: 15,
            got: 0,
        };
        assert_eq!(format!("{m}"), "sample 42: nibbles want 15 got 0");
        let names = [
            Field::Mono,
            Field::Status,
            Field::Voices,
            Field::Nibbles,
            Field::UnreachedWrites,
        ]
        .map(|f| f.to_string());
        assert_eq!(
            names,
            ["mono", "status", "voices", "nibbles", "writes consumed"]
        );
    }
}
