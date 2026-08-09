//! Reports the Z80 suite the way `report --test suite` reports the 68000 one:
//! `files: N/N green   cases: M/M`.
//!
//! Separate from `tests/z80suite.rs` because a `#[test]` cannot print a summary —
//! it can only pass or fail — and the one number a human wants after touching the
//! core is the count, not eleven lines of `ok`.
//!
//! # No `catch_unwind`
//!
//! The plan for this task proposed wrapping each file in `catch_unwind` so one
//! mismatch would not stop the count. That is unnecessary here and would have been
//! the wrong shape: [`z80runner::run_file`] already reports case failures as
//! *values* — `FileResult { total, passed, failures }` — and only panics when the
//! data itself is missing or corrupt. Those two are not failures to count past:
//! a missing file means the fetch is incomplete, and continuing would print a
//! confident partial total. So this binary lets them abort, and counts everything
//! else.
//!
//! Run with `--release`: 1,604,000 cases with a per-T-state bus trace is slow in a
//! debug build.

use testrunner::z80files;
use testrunner::z80runner;

/// How many failing files to name before the list stops being useful.
const SHOWN: usize = 20;

fn main() {
    let names = z80files::all_names();
    let mut green = 0usize;
    let mut cases = 0usize;
    let mut failed: Vec<(String, usize, usize, Vec<String>)> = Vec::new();

    for name in &names {
        let r = z80runner::run_file(&z80files::path_of(name));
        cases += r.passed;
        if r.failures.is_empty() && r.passed == r.total {
            green += 1;
        } else {
            failed.push((name.clone(), r.passed, r.total, r.failures));
        }
    }

    let want_cases = names.len() * z80files::CASES_PER_FILE;
    println!(
        "files: {green}/{}   cases: {cases}/{want_cases}",
        names.len()
    );

    if !failed.is_empty() {
        println!("\nfailed ({}):", failed.len());
        for (name, passed, total, why) in failed.iter().take(SHOWN) {
            println!("  {name}: {passed}/{total}");
            // One case's diff per file. The first names the same defect as the rest,
            // and five diffs times 1,604 files would bury the summary line the
            // reader came for.
            if let Some(first) = why.first() {
                println!("      {first}");
            }
        }
        if failed.len() > SHOWN {
            println!("  ... and {} more", failed.len() - SHOWN);
        }
        std::process::exit(1);
    }

    // A green run with the wrong total is the failure this binary exists to make
    // visible: 1,604 files that each pass zero cases would print `files: 1604/1604`
    // and look like success.
    assert_eq!(
        cases, want_cases,
        "every file passed but the case total is short -- the inventory and the data \
         disagree about how many cases a file holds"
    );
}
