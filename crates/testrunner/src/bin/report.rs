//! Full-suite report: every file in `testdata/`, one row per group, then a tally.
//!
//! This is sub-project A's definition-of-done artifact. It walks the directory
//! rather than reading a registration list, so it cannot inherit a gap in one —
//! but for the same reason it cannot *detect* one either, and a truncated
//! `testdata/` would simply produce a shorter table. Two things guard that:
//! the group count is printed beside the tally so a short table is visible in
//! the artifact itself, and `suite.rs`'s `every_vector_file_has_a_registered_group`
//! covers the other direction (a file with no test naming it). That test is
//! deliberately **not** duplicated here.
//!
//! # Reading a failure
//!
//! For a cycle mismatch, check the delta first:
//!
//! ```text
//! (want - got) % 4 == 0  =>  a missing or extra bus ACCESS, not a wrong constant
//! ```
//!
//! Every bus access on this bus is exactly 4 cycles — the measured law is
//! `cycles = 4 * (non-Idle transactions) + (total Idle cycles)`, which holds
//! 317,500/317,500 across all 127 groups. So a delta divisible by 4 points at
//! the access schedule and a delta that is not is an idle-term error. The
//! most-missed clause: **the aborted access of an address error still counts as
//! a bus access**, even though the core must never put it on the bus.

use std::path::PathBuf;
use testrunner::runner::{run_group, testdata_dir};

/// Fails loudly, naming the missing directory and the command that fills it.
/// A missing vector set is a host fault, never a skip — there is no env-var
/// escape hatch anywhere in this project.
fn vector_files() -> Vec<PathBuf> {
    let dir = testdata_dir();
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\nrun `cargo run -p testrunner --bin fetch`",
            dir.display()
        )
    });
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".json.bin"))
        })
        .collect();
    assert!(
        !files.is_empty(),
        "no vector files in {}\nrun `cargo run -p testrunner --bin fetch`",
        dir.display()
    );
    files.sort();
    files
}

fn main() {
    let files = vector_files();

    println!(
        "{:<16} {:>6} {:>6}  first failing case",
        "group", "pass", "total"
    );
    println!("{}", "-".repeat(72));

    let mut groups_green = 0usize;
    let mut cases_passed = 0usize;
    let mut cases_total = 0usize;
    let mut red: Vec<String> = Vec::new();

    for path in &files {
        let r = run_group(path);
        // A file that parses to zero cases would otherwise report a vacuous
        // 0/0 "green" row.
        assert!(
            r.total > 0,
            "{}: parsed to zero cases — vector file may be corrupt",
            r.group
        );

        cases_passed += r.passed;
        cases_total += r.total;
        // Green is `passed == total`, not `failures.is_empty()`: `run_group`
        // keeps only the first five failures, so an empty failure list is a
        // consequence of that cap rather than the definition of a clean group.
        let first = if r.is_clean() {
            groups_green += 1;
            String::new()
        } else {
            red.push(r.group.clone());
            match r.failures.first() {
                Some(f) => format!(
                    "{}: {}",
                    f.name,
                    f.diffs.first().map_or("?", String::as_str)
                ),
                None => "(failed, no diff recorded)".to_string(),
            }
        };
        println!("{:<16} {:>6} {:>6}  {}", r.group, r.passed, r.total, first);
    }

    println!("{}", "-".repeat(72));
    println!(
        "groups: {groups_green}/{} green   cases: {cases_passed}/{cases_total}",
        files.len()
    );
    println!("(group count is the number of `.json.bin` files walked in testdata/)");

    if !red.is_empty() {
        println!();
        println!("FAILING GROUPS: {}", red.join(" "));
        println!(
            "for a `cycles: got N want M` diff, check (M - N) % 4 first: a multiple \
             of 4 is a\nmissing or extra bus ACCESS, not a wrong constant. The \
             aborted access of an\naddress error counts as an access."
        );
        std::process::exit(1);
    }
}
