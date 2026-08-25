//! The YM2151 suite, and the premises that make it worth running.
//!
//! The comparison test is the point, but it is the *last* test in this file for a
//! reason: a suite of 1,000 silent cases passes it trivially. The coverage tests
//! assert the properties that were measured during the spec work — non-silence,
//! release-rate sensitivity, status variation, and CSM presence — so a regenerated
//! suite that loses discriminating power fails loudly instead of passing vacuously.

// An integration test is its own crate root, so the crate's `lib.rs` attribute
// does not reach here.
#![forbid(unsafe_code)]

use testrunner::{ymfiles, ymfmt, ymrunner};

/// Every case makes sound.
///
/// **Measured premise:** a purely random register script produced 0 non-zero samples
/// across 500 cases. If a future change to the generator's script reintroduces that,
/// this fails rather than the suite quietly becoming a 1,000-case test of silence.
///
/// The tolerance is the generator's own floor, not zero: 993 of 1,000 cases were
/// measured audible, and the seven that are not have a total level near the script's
/// cap with a slow decay, which lands the whole case under the DAC's quantisation.
/// Demanding all 1,000 would fail on real, correct data.
#[test]
fn every_case_is_audible() {
    let v = ymfiles::load().expect(ymfiles::FETCH_HINT);
    let silent: Vec<usize> = v
        .cases
        .iter()
        .enumerate()
        .filter(|(_, c)| c.samples.iter().all(|s| s.left == 0 && s.right == 0))
        .map(|(i, _)| i)
        .collect();
    assert!(
        silent.len() * 20 <= v.cases.len(),
        "{} of {} cases are silent: {:?}",
        silent.len(),
        v.cases.len(),
        &silent[..silent.len().min(20)]
    );
}

/// Every case releases, and the release is visible in the samples.
///
/// **Measured premise:** RR bit 0 was undetected in 0 of 200 cases until every case
/// keyed off. The generator keys off at sample 256; this asserts the *consequence* —
/// that the key-off changed what the second half of each case sounds like — rather
/// than re-reading the generator's intent.
///
/// Two floors, because they measure different things and the weaker one alone is
/// satisfiable by a suite with no release at all:
///
/// * **the peak changed** across the key-off — 977 of 1,000 measured. This is the
///   figure `genym`'s `MIN_RELEASE` floor is derived from, and it is what shows the
///   write took effect.
/// * **the peak went *down*** — 906 of 1,000 measured. Weaker as a count but
///   stronger as a claim: a case whose peak merely differs could have got louder.
///
/// Neither is 1,000, and the 94 that do not strictly decay are not a defect: 36 of
/// them run noise or CSM, both of which keep producing output after a key-off, and
/// the rest have a release rate slow enough that 64 samples of window is not enough
/// to fall below a peak set during the attack. The floors are 85%, which both clear.
#[test]
fn every_case_decays_after_the_key_off() {
    let v = ymfiles::load().expect(ymfiles::FETCH_HINT);
    let peak = |r: &[ymfmt::Sample]| {
        r.iter()
            .map(|s| i32::from(s.left).abs().max(i32::from(s.right).abs()))
            .max()
            .unwrap_or(0)
    };
    let mut changed = 0usize;
    let mut decayed = 0usize;
    let mut not_decaying = vec![];
    for (i, c) in v.cases.iter().enumerate() {
        let first = peak(&c.samples[..256]);
        let last = peak(&c.samples[448..]);
        if first != last {
            changed += 1;
        }
        if last < first {
            decayed += 1;
        } else {
            not_decaying.push(i);
        }
    }
    let floor = v.cases.len() * 85 / 100;
    assert!(
        changed >= floor,
        "only {changed} of {} cases changed peak across the key-off",
        v.cases.len()
    );
    assert!(
        decayed >= floor,
        "only {decayed} of {} cases decayed; {} did not: {:?}",
        v.cases.len(),
        not_decaying.len(),
        &not_decaying[..not_decaying.len().min(20)]
    );
}

/// The status trace varies, both across cases and within them.
///
/// **Measured premise:** timer state is not audible at all — undetected in 0 of 200
/// cases until the record gained a per-sample status byte. This asserts that the byte
/// carries information: some cases must show a status edge, or the field is a column
/// of zeros and the timers are untested.
#[test]
fn the_status_trace_carries_information() {
    let v = ymfiles::load().expect(ymfiles::FETCH_HINT);
    let with_edge = v
        .cases
        .iter()
        .filter(|c| c.samples.windows(2).any(|w| w[0].status != w[1].status))
        .count();
    assert!(
        with_edge * 4 >= v.cases.len(),
        "only {with_edge} of {} cases have a status edge",
        v.cases.len()
    );

    // The plan asked for an IRQ bit here. There is no IRQ bit in the OPM's status
    // register — `STATUS_IRQ` is 0 for the OPM (`ymfm_opm.h:124`), so ymfm's
    // set/clear of it is a no-op and this byte can never have bit 7 set. Asserting
    // `with_irq > 0` would have failed on correct data. What the plan wanted is that
    // the timers are visibly firing, which is bits 0 and 1.
    let with_timer_a = v
        .cases
        .iter()
        .filter(|c| c.samples.iter().any(|s| s.status & 0x01 != 0))
        .count();
    let with_timer_b = v
        .cases
        .iter()
        .filter(|c| c.samples.iter().any(|s| s.status & 0x02 != 0))
        .count();
    assert!(with_timer_a > 0, "no case ever overflows timer A");
    assert!(with_timer_b > 0, "no case ever overflows timer B");
    // Both timers must fire in the same suite: a generator that loaded only timer A
    // would satisfy an "either one" check while leaving B's 16x divider untested.
    assert!(
        with_timer_a >= v.cases.len() / 4 && with_timer_b >= v.cases.len() / 8,
        "timer A in {with_timer_a}, timer B in {with_timer_b} of {} cases",
        v.cases.len()
    );
}

/// The suite contains CSM cases.
///
/// **This is Definition of Done item 5** (item 9 when this comment was written; the
/// list gained entries above it). The lazy-`prepare()` divergence is
/// invisible with CSM off — eager and lazy agree bit-for-bit over 40,000 samples —
/// and appears only with `0x14` bit 7 set and a host that fires timers. A suite with
/// no CSM case cannot distinguish the two readings, so a Rust port that prepares
/// eagerly would pass at 1,000/1,000 while being wrong.
#[test]
fn the_suite_contains_csm_cases_with_timer_a_running() {
    let v = ymfiles::load().expect(ymfiles::FETCH_HINT);
    let csm: Vec<usize> = v
        .cases
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            let has_csm = c.writes.iter().any(|w| w.reg == 0x14 && w.val & 0x80 != 0);
            let has_timer_a = c.writes.iter().any(|w| w.reg == 0x10 || w.reg == 0x11);
            has_csm && has_timer_a
        })
        .map(|(i, _)| i)
        .collect();
    assert!(
        csm.len() >= 100,
        "only {} CSM cases; the prepare() gate is untested",
        csm.len()
    );

    // A CSM case whose timer never fires proves nothing: CSM keys on from timer A's
    // *overflow*, so with no overflow the case is indistinguishable from CSM off.
    // This was a real defect in the generator — timer A's 10-bit value was held in a
    // uint8_t, giving every CSM case a period longer than the window — and the audio
    // looked healthy throughout.
    let mut without_overflow = vec![];
    let mut silent = vec![];
    for &i in &csm {
        let c = &v.cases[i];
        if !c.samples.iter().any(|s| s.status & 0x01 != 0) {
            without_overflow.push(i);
        }
        if !c.samples.iter().any(|s| s.left != 0 || s.right != 0) {
            silent.push(i);
        }
    }
    assert!(
        without_overflow.is_empty(),
        "{} CSM cases never overflow timer A: {:?}",
        without_overflow.len(),
        &without_overflow[..without_overflow.len().min(10)]
    );
    assert!(
        silent.len() * 20 <= csm.len(),
        "{} of {} CSM cases are silent: {:?}",
        silent.len(),
        csm.len(),
        &silent[..silent.len().min(10)]
    );
}

/// The cases are not all the same case.
///
/// A generator bug that ignored its seed would produce 1,000 identical records, and
/// every test above would still pass.
///
/// Two hashes, because each catches what the other cannot. The **script** hash is the
/// direct test of the seed and must be 1,000 distinct. The **audio** hash is the test
/// that distinct scripts actually render differently, and its denominator is the 993
/// non-silent cases: the 7 silent ones are byte-identical to each other by necessity —
/// 512 zero samples are 512 zero samples — so demanding 1,000 distinct buffers would
/// fail on correct data. Measured: 993 non-silent cases, 993 distinct buffers.
#[test]
fn the_thousand_cases_are_a_thousand_different_cases() {
    let v = ymfiles::load().expect(ymfiles::FETCH_HINT);

    let mut scripts = std::collections::BTreeSet::new();
    for c in &v.cases {
        let flat: Vec<u16> = c
            .writes
            .iter()
            .flat_map(|w| [w.at_sample, u16::from(w.reg) << 8 | u16::from(w.val)])
            .collect();
        scripts.insert(ym2151::tables::fnv1a_u16(&flat));
    }
    assert_eq!(scripts.len(), v.cases.len(), "duplicate scripts");

    let mut audible = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    for c in &v.cases {
        if c.samples.iter().all(|s| s.left == 0 && s.right == 0) {
            continue;
        }
        audible += 1;
        let flat: Vec<u16> = c
            .samples
            .iter()
            .flat_map(|s| [s.left as u16, s.right as u16])
            .collect();
        seen.insert(ym2151::tables::fnv1a_u16(&flat));
    }
    assert_eq!(seen.len(), audible, "two audible cases render identically");
}

/// The suite: every case, sample-exact on both channels and on the status trace.
///
/// The gate for this sub-project is 1,000 of 1,000. A failure names the case, the
/// sample, the field, and both values — enough to reproduce with `reportym --case N`
/// without re-reading the file by hand.
#[test]
fn the_suite_passes() {
    let v = ymfiles::load().expect(ymfiles::FETCH_HINT);
    assert_eq!(v.cases.len(), ymfiles::EXPECTED);

    let mut failures = vec![];
    for (i, case) in v.cases.iter().enumerate() {
        let r = ymrunner::run_case(case);
        if !r.ok {
            failures.push((i, r.first_mismatch.expect("a failure has a mismatch")));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases failed; first five: {:?}",
        failures.len(),
        v.cases.len(),
        &failures[..failures.len().min(5)]
    );
}

/// The runner can fail.
///
/// **The most important test in this file.** A runner whose comparison is broken —
/// wrong slice, swallowed error, `ok` hardcoded true — passes the suite above at
/// 1,000/1,000 and tells us nothing. This corrupts one sample of a real case and
/// asserts the runner reports exactly that sample, so the 1,000/1,000 above means
/// something. This branch has shipped six variants of "a claim that cannot fail";
/// this is the test that refuses the seventh.
///
/// The plan wrote `v.cases[0]` here. **Case 0 is a CSM case** — the generator enables
/// CSM when `seed % 8 == 0` — and until the lazy `prepare()` gate lands it does not
/// pass pristine, so the plan's version fails on its first assertion for a reason
/// that has nothing to do with the runner. Rather than hardcode a different index,
/// this picks the first case that passes and carries both channels and a timed write,
/// which keeps the test's subject the runner in either era.
#[test]
fn the_runner_reports_a_deliberately_corrupted_sample() {
    let v = ymfiles::load().expect(ymfiles::FETCH_HINT);
    let (at, pristine) = v
        .cases
        .iter()
        .enumerate()
        .find(|(_, c)| {
            ymrunner::run_case(c).ok
                && c.samples.iter().any(|s| s.left != 0)
                && c.samples.iter().any(|s| s.right != 0)
                && c.writes.iter().any(|w| w.at_sample != 0)
        })
        .map(|(i, c)| (i, c.clone()))
        .expect("some case passes, is audible on both channels, and has a timed write");

    let mut case = pristine.clone();
    let idx = case
        .samples
        .iter()
        .position(|s| s.left != 0)
        .expect("audible");
    let original = case.samples[idx].left;
    case.samples[idx].left = original.wrapping_add(1);
    let r = ymrunner::run_case(&case);
    assert!(!r.ok, "the corrupted case must fail (case {at})");
    let m = r.first_mismatch.expect("with a mismatch");
    assert_eq!(m.sample, idx, "at the sample that was corrupted");
    assert_eq!(m.field, ymrunner::Field::Left);
    // And it names both values, so a report line is enough to act on.
    assert_eq!(m.want, i32::from(original.wrapping_add(1)));
    assert_eq!(m.got, i32::from(original));

    // And the status field is compared too, not just the audio.
    let mut case = pristine.clone();
    case.samples[10].status ^= 0x01;
    let r = ymrunner::run_case(&case);
    assert!(!r.ok, "a corrupted status byte must fail");
    let m = r.first_mismatch.expect("with a mismatch");
    assert_eq!(m.field, ymrunner::Field::Status);
    assert_eq!(m.sample, 10);

    // And the right channel, which a runner that compared `left` twice would miss.
    let mut case = pristine.clone();
    let idx = case
        .samples
        .iter()
        .position(|s| s.right != 0)
        .expect("has a right channel");
    case.samples[idx].right = case.samples[idx].right.wrapping_add(1);
    let r = ymrunner::run_case(&case);
    assert!(!r.ok, "a corrupted right sample must fail");
    let m = r.first_mismatch.expect("with a mismatch");
    assert_eq!(m.field, ymrunner::Field::Right);
    assert_eq!(m.sample, idx);

    // A dropped write must fail too. This is the one corruption that tests the
    // runner's *script replay* rather than its comparison: a runner that applied
    // every write up front, or ignored `at_sample`, would still pass all three
    // corruptions above while getting every real key-off wrong.
    let mut case = pristine.clone();
    case.writes.retain(|w| w.at_sample == 0);
    assert!(
        case.writes.len() < pristine.writes.len(),
        "the case has a timed write to drop"
    );
    assert!(
        !ymrunner::run_case(&case).ok,
        "dropping the key-off must change the samples"
    );
}

/// The two existing suites still pass, in the same test run.
///
/// D2 modifies `machine`. This is the regression gate: 127/127 for the 68000 and
/// 1,604/1,604 for the Z80 are not tolerances.
///
/// The plan wrote `testrunner::files::EXPECTED` for the 68000 count. There is no such
/// constant — the 68000 side discovers its groups by walking `testdata/`, and 127
/// appears only as a literal inside a `binfmt` test. Asserting a constant equals its
/// own literal would be a claim that cannot fail anyway, so this counts the files on
/// disk instead: that verifies the data is present *and* that both inventories still
/// describe it.
#[test]
fn the_existing_suites_have_not_moved() {
    let m68k_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata"));
    let count = |dir: &std::path::Path, ext: &str| {
        std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .flatten()
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some(ext))
            .count()
    };
    assert_eq!(count(m68k_dir, "bin"), 127, "the 68000 suite is 127 groups");
    assert_eq!(
        count(&testrunner::z80files::dir(), "z80bin"),
        testrunner::z80files::EXPECTED,
        "the Z80 suite is 1,604 files"
    );
    assert_eq!(testrunner::z80files::EXPECTED, 1604);
}
