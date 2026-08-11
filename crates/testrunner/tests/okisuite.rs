//! Premises about what the OKI suite exercises.
//!
//! The generator's protocol logic and `crates/oki/src/chip.rs` are the same
//! reading of the same MAME file, so a misreading would agree with itself and
//! the whole suite would pass. These tests are the check on that: they assert
//! properties of the *recorded data* that a wrong reading would not produce.
//!
//! # Every figure here is measured, and none is a tolerance
//!
//! The numbers come from the suite `genoki` committed, and `genoki`'s own
//! validator checks the same premises before it will promote a file. Restated
//! here so a suite regenerated with a quieter script fails loudly rather than
//! testing less — and so a `cargo test --workspace` run catches it without
//! anyone remembering to run the report.
//!
//! # What these tests deliberately avoid
//!
//! They do not recompute a sample from the core and compare it to itself. Where
//! a premise needs arithmetic — the step index, for instance — the arithmetic is
//! written out here from MAME's published table rather than called out of `oki`,
//! so a core whose table was wrong cannot make a premise about the data agree
//! with it.

use testrunner::okifiles;

/// MAME's `s_index_shift`, indexed `nibble & 7`, from `okiadpcm.cpp`.
/// Transcribed here rather than imported: see the module docs.
const SHIFT: [i8; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// The generator opens voice 0 on the ladder phrase at sample 0 of every case,
/// and the phrase is 128 nibbles long.
const LADDER: usize = 128;
/// Its leading run of nibble 7, which drives the step index to 48.
const LADDER_SEVENS: usize = 32;

#[test]
fn every_case_is_audible() {
    let cases = okifiles::load().expect("the suite must be present");
    let silent: Vec<usize> = cases
        .iter()
        .enumerate()
        .filter(|(_, c)| c.samples.iter().all(|s| s.mono_2x == 0))
        .map(|(i, _)| i)
        .collect();
    assert!(
        silent.is_empty(),
        "{} cases are entirely silent: {:?}",
        silent.len(),
        &silent[..silent.len().min(10)]
    );
}

/// The chip's own clamp is reachable and reached. If this fails, the suite does
/// not test the clamp — and the clamp is the thing the D3 spec omitted entirely.
///
/// Measured: 998 of 1,000. Two voices open at unity gain in every case, so the
/// bound is reached in nearly all of them; the floor of 0.90 is `genoki`'s.
#[test]
fn the_suite_reaches_the_chips_own_clamp() {
    let cases = okifiles::load().expect("the suite must be present");
    let clamped = cases
        .iter()
        .filter(|c| c.samples.iter().any(|s| s.mono_2x.abs() == 65_536))
        .count();
    let frac = clamped as f64 / cases.len() as f64;
    assert!(
        frac >= 0.90,
        "only {clamped}/{} cases reach +-65536 ({frac:.3}); the clamp is untested",
        cases.len()
    );
    // And nothing exceeds it, which is the other half of the claim: a suite
    // recorded without the clamp would satisfy the floor above and still be
    // wrong.
    for (i, c) in cases.iter().enumerate() {
        for (n, s) in c.samples.iter().enumerate() {
            assert!(
                s.mono_2x.abs() <= 65_536,
                "case {i} sample {n}: {} exceeds the clamp",
                s.mono_2x
            );
        }
    }
}

/// Four voices must actually play at once somewhere, since that is the only way
/// the sum exceeds one voice's range. Measured: 933 of 1,000 cases.
#[test]
fn the_suite_plays_all_four_voices_at_once() {
    let cases = okifiles::load().expect("the suite must be present");
    let all_four = cases
        .iter()
        .filter(|c| c.samples.iter().any(|s| s.voices == 0x0F))
        .count();
    assert!(all_four > 0, "no case ever has all four voices playing");
    let per_voice: Vec<usize> = (0..4)
        .map(|v| {
            cases
                .iter()
                .filter(|c| c.samples.iter().any(|s| s.voices & (1 << v) != 0))
                .count()
        })
        .collect();
    assert!(
        per_voice.iter().all(|&n| n > 0),
        "some voice never plays: {per_voice:?}"
    );
}

/// Voices must both start and stop, or the suite never tests the stop command or
/// the end-of-phrase condition. Measured: all 1,000 cases do both.
#[test]
fn voices_both_start_and_stop_within_a_case() {
    let cases = okifiles::load().expect("the suite must be present");
    let mut started = 0usize;
    let mut stopped = 0usize;
    for c in &cases {
        let transitions: Vec<u8> = c.samples.iter().map(|s| s.voices).collect();
        if transitions.windows(2).any(|w| w[1] & !w[0] != 0) {
            started += 1;
        }
        if transitions.windows(2).any(|w| w[0] & !w[1] != 0) {
            stopped += 1;
        }
    }
    assert!(
        started > cases.len() / 2,
        "only {started} cases ever start a voice mid-case"
    );
    assert!(
        stopped > cases.len() / 2,
        "only {stopped} cases ever stop a voice"
    );
}

/// The status byte's high nibble is always F, and its low nibble is a **subset**
/// of the voices that sounded — not equal to it.
///
/// The plan asserted equality. That is wrong on correct data, and measurably so:
/// `voices` is who sounded *during* the sample and `status` is who is still
/// playing *after* it, so a voice whose phrase ends on this sample appears in the
/// first and not the second. They differ at 1,907 of the suite's 512,000
/// samples, first at case 0 sample 127 — where the ladder ends. Both halves are
/// checked here because the subset relation alone is satisfied by a status byte
/// that is always `0xF0`.
#[test]
fn the_status_is_a_subset_of_the_voices_that_sounded() {
    let cases = okifiles::load().expect("the suite must be present");
    let mut differ = 0usize;
    for (i, c) in cases.iter().enumerate() {
        for (n, s) in c.samples.iter().enumerate() {
            assert_eq!(s.status & 0xF0, 0xF0, "case {i} sample {n}");
            assert_eq!(
                s.voices & 0xF0,
                0,
                "case {i} sample {n}: voices has high bits"
            );
            assert_eq!(
                s.status & 0x0F & !s.voices,
                0,
                "case {i} sample {n}: status {:#04X} claims a voice that did not sound ({:#04X})",
                s.status,
                s.voices
            );
            if s.status & 0x0F != s.voices {
                differ += 1;
            }
        }
    }
    assert!(
        differ > 0,
        "the two masks never differ, so the suite contains no phrase that ends \
         while it is playing -- and the runner could read either mask for both"
    );
}

/// The step index reaches **both** clamps, 0 and 48, in every case.
///
/// Measured against MAME's decoder: pseudorandom nibbles drive the index over
/// 1..48 and **never** reach 0, so a random script would leave the lower clamp
/// untested. The generator therefore reserves phrase 1 in every case for a
/// deliberate ladder — 32 nibbles of 7 then 96 of 0 — and this recomputes the
/// index from the recorded nibbles to confirm it.
///
/// Recomputing rather than recording the index is the point: the file holds
/// nibbles, and the index is derived from them by MAME's rule written out at the
/// top of this file, so agreement here is a claim about the *data* — that it
/// contains a segment which crosses the range — and not about the core.
#[test]
fn the_suite_drives_the_step_index_to_both_clamps() {
    let cases = okifiles::load().expect("the suite must be present");
    // The ladder occupies voice 0 for exactly the first 128 samples of every
    // case: it starts at sample 0, the generator never stops voice 0, and a
    // running voice cannot be restarted. Confining the walk to that window is
    // what makes it sound -- past sample 127 voice 0 may restart on a random
    // phrase, and a restart on the sample right after the phrase ends leaves no
    // gap in the `voices` mask to detect, so the decoder reset would be missed.
    let mut at_zero = 0usize;
    let mut at_max = 0usize;
    for (i, c) in cases.iter().enumerate() {
        assert!(
            c.samples.len() >= LADDER,
            "case {i} is shorter than the ladder"
        );
        let mut step: i8 = 0;
        let mut saw_zero = false;
        let mut saw_max = false;
        for (n, s) in c.samples.iter().take(LADDER).enumerate() {
            assert!(s.voices & 1 != 0, "case {i}: voice 0 stopped at sample {n}");
            let nibble = usize::from(s.nibbles & 0x0F);
            step = (step + SHIFT[nibble & 7]).clamp(0, 48);
            saw_zero |= step == 0;
            saw_max |= step == 48;
        }
        at_zero += usize::from(saw_zero);
        at_max += usize::from(saw_max);
    }
    assert_eq!(
        (at_zero, at_max),
        (cases.len(), cases.len()),
        "the ladder is in every case, so every case must reach both clamps: \
         {at_zero} reach 0 and {at_max} reach 48 of {}",
        cases.len()
    );
}

/// The ladder phrase is intact in every case: exactly 32 nibbles of 7 then 96 of
/// 0 on voice 0.
///
/// The step-clamp test above would still pass on a ladder the random fill had
/// partly overwritten — some other nibble sequence crossing the range by luck —
/// so this checks the shape itself. Asserted on the nibbles rather than on a
/// decoded total, so no part of it depends on the core.
#[test]
fn the_ladder_phrase_is_intact_in_every_case() {
    let cases = okifiles::load().expect("the suite must be present");
    for (i, c) in cases.iter().enumerate() {
        for (n, s) in c.samples.iter().take(LADDER).enumerate() {
            let want = if n < LADDER_SEVENS { 7 } else { 0 };
            assert_eq!(
                s.nibbles & 0x0F,
                want,
                "case {i} sample {n}: voice 0 consumed {:#X}, the ladder says {want:#X}",
                s.nibbles & 0x0F
            );
        }
    }
}

/// The suite reads the top of the 18-bit address bus, in every case.
///
/// The generator opens voice 1 on a phrase covering the ROM's last 64 bytes.
/// Nothing else in the fixture reaches this high, and a core masking addresses
/// with `0x1FFFF` instead of `0x3FFFF` folds these reads onto a different byte.
/// Verified: that mutation fails all 1,000 cases at sample 0, reported as a
/// `nibbles` divergence.
///
/// Checked against the ROM the file carries rather than against a constant, so a
/// generator whose own address walk drifted cannot agree with itself here.
#[test]
fn the_suite_reads_the_top_of_the_address_bus() {
    let cases = okifiles::load().expect("the suite must be present");
    for (i, c) in cases.iter().enumerate() {
        let top = c.rom.len() - 64;
        let want = u16::from(c.rom[top] >> 4);
        assert_eq!(
            (c.samples[0].nibbles >> 4) & 0x0F,
            want,
            "case {i}: voice 1's first nibble is not the high nibble of rom[{top:#X}]"
        );
        assert!(
            c.samples[0].voices & 0b10 != 0,
            "case {i}: voice 1 does not sound at sample 0"
        );
    }
}

/// Some voice is set to a silent volume index (9..15, whose table entries are
/// exactly zero) and keeps advancing while contributing nothing.
///
/// Measured: 3,026 such starts across the suite. Without one, a core that read
/// the volume table one entry short — or treated index 9 as index 8 — would pass
/// every case.
#[test]
fn the_suite_uses_the_silent_volume_indices() {
    let cases = okifiles::load().expect("the suite must be present");
    // Walk each script the way the chip's state machine does: whether a byte is
    // a data byte depends on what preceded it, so a positional pair match
    // miscounts.
    let mut silent_starts = 0usize;
    for c in &cases {
        let mut pending = false;
        for w in &c.writes {
            if pending {
                pending = false;
                if w.byte >> 4 != 0 && w.byte & 0x0F >= 9 {
                    silent_starts += 1;
                }
            } else if w.byte & 0x80 != 0 {
                pending = true;
            }
        }
    }
    assert!(
        silent_starts > 0,
        "no start uses a silent volume index, so the zero entries are untested"
    );
}

/// Both pin-7 states appear. The decoder does not care, but the record is what
/// `machine`'s rate test reads, and a suite of one polarity would hide a
/// numerator swap.
#[test]
fn both_pin_seven_states_appear() {
    let cases = okifiles::load().expect("the suite must be present");
    assert!(cases.iter().any(|c| c.pin7), "no case records pin 7 high");
    assert!(cases.iter().any(|c| !c.pin7), "no case records pin 7 low");
}

/// The whole suite passes. This is the gate the report command runs; having it
/// as a test too means `cargo test --workspace` catches a regression without
/// anyone remembering to run the report.
#[test]
fn every_case_passes() {
    let cases = okifiles::load().expect("the suite must be present");
    let failures: Vec<String> = cases
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            testrunner::okirunner::run_case(c)
                .err()
                .map(|m| format!("case {i}: {m}"))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} cases diverge:\n{}",
        failures.len(),
        cases.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}
