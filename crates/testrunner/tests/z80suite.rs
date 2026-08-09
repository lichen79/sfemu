//! The Z80 vector suite: 1,604 files, 1,604,000 cases.
//!
//! Eight page tests, not 1,604 file tests. The 68000 suite in `suite.rs` hand-writes
//! 127 `groups!` entries; 1,604 is not viable, and generating the list from
//! `read_dir` at compile time would make the test set depend on what happens to be
//! on disk — a suite that silently shrinks when the fetch is incomplete.
//!
//! A failure still names its file, which is the actual requirement: every assertion
//! here carries the path, and [`z80runner::run_file`] returns the first five case
//! failures with the case index and the differing fields.
//!
//! # The three coverage tests are the substance
//!
//! **A page test over an empty directory iterates nothing and reports success.**
//! That is this project's recurring vacuous-pass shape, and it is the reason three
//! tests exist whose only job is to make emptiness and incompleteness fail. Their
//! counts are literals; a count from `read_dir().count()` would agree with whatever
//! is on disk, including nothing.
//!
//! Run with `--release`. 1,604,000 cases with a per-T-state bus trace is slow in a
//! debug build.

use std::path::PathBuf;
use testrunner::z80files::{self, Page};
use testrunner::z80runner;

/// Runs one file and asserts every case passed, naming the file if not.
///
/// Returns the case count so the callers can add it up: a page test that ran the
/// right files but zero cases each would otherwise pass, and that is precisely the
/// vacuous shape this file is built against.
fn run_one(name: &str) -> usize {
    let path = z80files::path_of(name);
    let r = z80runner::run_file(&path);
    assert!(
        r.failures.is_empty(),
        "{name} ({}): {}/{} cases passed\n{}",
        path.display(),
        r.passed,
        r.total,
        r.failures.join("\n")
    );
    assert_eq!(
        r.passed,
        r.total,
        "{name}: {} cases neither passed nor reported a failure -- \
         run_file's own accounting is broken",
        r.total - r.passed
    );
    r.total
}

/// Runs every file `filter` accepts, and asserts the case total.
///
/// `want_files` is a literal at every call site. Deriving it from the filtered list
/// would make this function compare a number against itself.
fn run_files(label: &str, want_files: usize, filter: impl Fn(&str) -> bool) {
    let names: Vec<String> = z80files::all_names()
        .into_iter()
        .filter(|n| filter(n))
        .collect();
    assert_eq!(
        names.len(),
        want_files,
        "{label} claims {} files, expected {want_files}",
        names.len()
    );
    let cases: usize = names.iter().map(|n| run_one(n)).sum();
    assert_eq!(
        cases,
        want_files * z80files::CASES_PER_FILE,
        "{label}: case total"
    );
}

/// The opcode byte a name ends with — the split key for the base page.
///
/// The name's *first* character cannot be used: `dd 80` starts with `d`, so a split
/// on the leading digit would put prefixed pages in arbitrary halves. Every name
/// ends in its opcode's two hex digits, prefixed or not.
fn opcode(name: &str) -> u8 {
    u8::from_str_radix(&name[name.len() - 2..], 16)
        .unwrap_or_else(|e| panic!("{name} does not end in a hex opcode: {e}"))
}

fn on(page: Page) -> impl Fn(&str) -> bool {
    move |n: &str| z80files::page_of(n) == Some(page)
}

// The base page splits at 0x80 only so the two halves run concurrently under cargo
// test's default thread pool. The boundary has no meaning beyond that. The halves are
// 128 and 124 rather than 126 each because all four prefix bytes — CB, DD, ED, FD —
// are above 0x80, so the whole gap falls in the upper half.

#[test]
fn base_00_7f() {
    run_files("base 00-7f", 128, |n| on(Page::Base)(n) && opcode(n) < 0x80);
}

#[test]
fn base_80_ff() {
    run_files("base 80-ff", 124, |n| {
        on(Page::Base)(n) && opcode(n) >= 0x80
    });
}

#[test]
fn cb_page() {
    run_files("cb", 256, on(Page::Cb));
}

#[test]
fn ed_page() {
    run_files("ed", 80, on(Page::Ed));
}

#[test]
fn dd_page() {
    run_files("dd", 252, on(Page::Dd));
}

#[test]
fn fd_page() {
    run_files("fd", 252, on(Page::Fd));
}

#[test]
fn ddcb_page() {
    run_files("dd cb", 256, on(Page::DdCb));
}

#[test]
fn fdcb_page() {
    run_files("fd cb", 256, on(Page::FdCb));
}

/// Every file on disk is a file the inventory knows, and vice versa.
///
/// Without this, a file the fetcher wrote under a name `all_names` does not produce
/// would sit in `testdata/z80` untested forever while the suite reported full
/// coverage. The comparison runs both directions for that reason: a missing file and
/// a stray file are different bugs and both are silent.
#[test]
fn every_vector_file_is_covered() {
    let dir = z80files::dir();
    let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("missing {}: {e}\n{}", dir.display(), z80files::FETCH_HINT))
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".z80bin").map(str::to_owned))
        .collect();
    on_disk.sort();

    let mut expected: Vec<String> = z80files::all_names()
        .iter()
        .map(|n| z80files::stem(n))
        .collect();
    expected.sort();

    // Named differences rather than a 1,604-element `assert_eq!`, whose output a
    // reader cannot use.
    let missing: Vec<&String> = expected.iter().filter(|n| !on_disk.contains(n)).collect();
    let stray: Vec<&String> = on_disk.iter().filter(|n| !expected.contains(n)).collect();
    assert!(
        missing.is_empty(),
        "{} files the inventory expects are not on disk, e.g. {:?}\n{}",
        missing.len(),
        &missing[..missing.len().min(5)],
        z80files::FETCH_HINT
    );
    assert!(
        stray.is_empty(),
        "{} files on disk that no page claims, e.g. {:?} -- \
         they are untested and the suite would report full coverage",
        stray.len(),
        &stray[..stray.len().min(5)]
    );
    assert_eq!(on_disk.len(), z80files::EXPECTED);
}

/// Each page's file count is a **literal** here, never `read_dir().count()`.
///
/// A count read from disk agrees with whatever is on disk, including nothing — which
/// is precisely the failure the eight page tests cannot see, because a loop over an
/// empty list passes. The sum is asserted too: seven predicates that each claimed
/// nothing would satisfy seven zero-expectations, and only the total catches it.
#[test]
fn every_page_has_its_expected_file_count() {
    let names = z80files::all_names();
    let counts = z80files::page_counts(&names);
    for (page, want) in [
        (Page::Base, 252usize),
        (Page::Cb, 256),
        (Page::Ed, 80),
        (Page::Dd, 252),
        (Page::Fd, 252),
        (Page::DdCb, 256),
        (Page::FdCb, 256),
    ] {
        assert_eq!(counts[page as usize], want, "{page:?}");
    }
    assert_eq!(names.len(), z80files::EXPECTED, "and 1,604 in total");
    assert_eq!(
        counts.iter().sum::<usize>(),
        z80files::EXPECTED,
        "every name claimed by exactly one page"
    );
    // The two halves of the base page must also account for all of it, or a split
    // that dropped files would leave both halves individually plausible.
    let lo = names
        .iter()
        .filter(|n| on(Page::Base)(n) && opcode(n) < 0x80)
        .count();
    let hi = names
        .iter()
        .filter(|n| on(Page::Base)(n) && opcode(n) >= 0x80)
        .count();
    assert_eq!((lo, hi), (128, 124), "the base page's halves");
    assert_eq!(lo + hi, 252, "and they are the whole page");
}

/// The cheapest statement of the failure the other two are shaped around.
///
/// Kept separate so a wholly absent `testdata/z80` fails once, clearly, rather than
/// eight times with eight different case-count mismatches. This is the test that
/// proves the loud-failure path works: with no data it names a path and the fetch
/// command, and there is no skip path and no environment variable that turns it off.
#[test]
fn no_page_is_empty() {
    let names = z80files::all_names();
    for page in Page::ALL {
        let files: Vec<&String> = names.iter().filter(|n| on(page)(n)).collect();
        assert!(!files.is_empty(), "{page:?} claims no files");
        let first: PathBuf = z80files::path_of(files[0]);
        assert!(
            first.exists(),
            "{} is missing\n{}",
            first.display(),
            z80files::FETCH_HINT
        );
    }
}
