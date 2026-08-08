//! Runs named vector files through `z80runner::run_file` and prints pass counts.
//!
//! Task 15 builds the real reporter, which gates the build on a target. This is
//! the developer's inner loop until then: while the base page is being filled in,
//! most files fail by construction, so a pass/fail gate would be noise and a way
//! to ask "how does `80.z80bin` look now" is what is actually needed.
//!
//! ```text
//! cargo run -q -p testrunner --release --example z80smoke -- 00 37 3f
//! ```
//!
//! With no arguments it runs the opcodes the core implements so far.
//!
//! No ROM is involved: these are MIT-licensed CPU vectors from SingleStepTests/z80,
//! fetched by `fetchz80` into gitignored `testdata/`.

use std::path::PathBuf;

/// The base-page opcodes the core implements so far, as of Task 7.
///
/// Kept as a literal list rather than derived from the decoder: a default that
/// asked the decoder what it handled would report "all green" on exactly the
/// opcodes the decoder had wrongly claimed.
const DEFAULT: &[&str] = &[
    "00", "01", "02", "03", "04", "05", "06", "08", "09", "0a", "0b", "0c", "0d", "0e", "11", "12",
    "13", "14", "15", "16", "19", "1a", "1b", "1c", "1d", "1e", "21", "22", "23", "24", "25", "26",
    "27", "29", "2a", "2b", "2c", "2d", "2e", "2f", "31", "32", "33", "34", "35", "36", "37", "39",
    "3a", "3b", "3c", "3d", "3e", "3f", "40", "41", "42", "43", "44", "45", "46", "47", "48", "49",
    "4a", "4b", "4c", "4d", "4e", "4f", "50", "51", "52", "53", "54", "55", "56", "57", "58", "59",
    "5a", "5b", "5c", "5d", "5e", "5f", "60", "61", "62", "63", "64", "65", "66", "67", "68", "69",
    "6a", "6b", "6c", "6d", "6e", "6f", "70", "71", "72", "73", "74", "75", "76", "77", "78", "79",
    "7a", "7b", "7c", "7d", "7e", "7f", "80", "81", "82", "83", "84", "85", "86", "87", "88", "89",
    "8a", "8b", "8c", "8d", "8e", "8f", "90", "91", "92", "93", "94", "95", "96", "97", "98", "99",
    "9a", "9b", "9c", "9d", "9e", "9f", "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9",
    "aa", "ab", "ac", "ad", "ae", "af", "b0", "b1", "b2", "b3", "b4", "b5", "b6", "b7", "b8", "b9",
    "ba", "bb", "bc", "bd", "be", "bf", "c1", "c5", "c6", "ce", "d1", "d5", "d6", "d9", "de", "e1",
    "e5", "e6", "eb", "ee", "f1", "f3", "f5", "f6", "f9", "fb", "fe",
];

fn main() {
    // Anchored to the manifest so the tool works from any directory, matching
    // `fetchz80`. A relative path would silently look in the wrong place.
    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/z80"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stems: Vec<&str> = if args.is_empty() {
        DEFAULT.to_vec()
    } else {
        args.iter().map(String::as_str).collect()
    };

    let (mut passed, mut total) = (0usize, 0usize);
    for stem in stems {
        let r = testrunner::z80runner::run_file(&dir.join(format!("{stem}.z80bin")));
        passed += r.passed;
        total += r.total;
        println!("{stem}: {}/{}", r.passed, r.total);
        for f in r.failures.iter().take(2) {
            println!("    {f}");
        }
    }
    println!("total: {passed}/{total}");
}
