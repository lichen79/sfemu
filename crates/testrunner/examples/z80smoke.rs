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

/// The opcodes the core implements so far: as of Task 11, the 252 non-prefix
/// base-page opcodes, all 256 of the `CB` page, the 80 `ED` opcodes that have a
/// file, and the 252 plain stems of each index page.
///
/// Kept as a literal list rather than derived from the decoder: a default that
/// asked the decoder what it handled would report "all green" on exactly the
/// opcodes the decoder had wrongly claimed.
///
/// The `CB` stems are generated rather than listed, because that page is complete
/// and uniform — `cb_00` through `cb_ff` with no gaps, which neither of the other
/// two pages can say. See [`cb_stems`] and [`ED`].
const DEFAULT: &[&str] = &[
    "00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "0a", "0b", "0c", "0d", "0e", "0f",
    "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "1a", "1b", "1c", "1d", "1e", "1f",
    "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "2a", "2b", "2c", "2d", "2e", "2f",
    "30", "31", "32", "33", "34", "35", "36", "37", "38", "39", "3a", "3b", "3c", "3d", "3e", "3f",
    "40", "41", "42", "43", "44", "45", "46", "47", "48", "49", "4a", "4b", "4c", "4d", "4e", "4f",
    "50", "51", "52", "53", "54", "55", "56", "57", "58", "59", "5a", "5b", "5c", "5d", "5e", "5f",
    "60", "61", "62", "63", "64", "65", "66", "67", "68", "69", "6a", "6b", "6c", "6d", "6e", "6f",
    "70", "71", "72", "73", "74", "75", "76", "77", "78", "79", "7a", "7b", "7c", "7d", "7e", "7f",
    "80", "81", "82", "83", "84", "85", "86", "87", "88", "89", "8a", "8b", "8c", "8d", "8e", "8f",
    "90", "91", "92", "93", "94", "95", "96", "97", "98", "99", "9a", "9b", "9c", "9d", "9e", "9f",
    "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9", "aa", "ab", "ac", "ad", "ae", "af",
    "b0", "b1", "b2", "b3", "b4", "b5", "b6", "b7", "b8", "b9", "ba", "bb", "bc", "bd", "be", "bf",
    "c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9", "ca", "cc", "cd", "ce", "cf", "d0",
    "d1", "d2", "d3", "d4", "d5", "d6", "d7", "d8", "d9", "da", "db", "dc", "de", "df", "e0", "e1",
    "e2", "e3", "e4", "e5", "e6", "e7", "e8", "e9", "ea", "eb", "ec", "ee", "ef", "f0", "f1", "f2",
    "f3", "f4", "f5", "f6", "f7", "f8", "f9", "fa", "fb", "fc", "fe", "ff",
];

/// The 80 `ED` opcodes upstream ships a file for.
///
/// A literal list because the page is sparse: 176 of its 256 encodings do nothing
/// but consume two M1 cycles, and no file exists for any of them. Generating
/// `ed_00`–`ed_ff` would name 176 files that are not there, which — with missing
/// data failing loudly, as it must — would abort the run on the first of them.
///
/// The membership rule is `0x40..=0x7F` plus the sixteen block opcodes, and it is
/// written out rather than expressed as that rule for the same reason [`DEFAULT`]
/// is: a generated list agrees with whatever the generator believes, and what is
/// wanted here is agreement with the directory.
const ED: &[&str] = &[
    "ed_40", "ed_41", "ed_42", "ed_43", "ed_44", "ed_45", "ed_46", "ed_47", "ed_48", "ed_49",
    "ed_4a", "ed_4b", "ed_4c", "ed_4d", "ed_4e", "ed_4f", "ed_50", "ed_51", "ed_52", "ed_53",
    "ed_54", "ed_55", "ed_56", "ed_57", "ed_58", "ed_59", "ed_5a", "ed_5b", "ed_5c", "ed_5d",
    "ed_5e", "ed_5f", "ed_60", "ed_61", "ed_62", "ed_63", "ed_64", "ed_65", "ed_66", "ed_67",
    "ed_68", "ed_69", "ed_6a", "ed_6b", "ed_6c", "ed_6d", "ed_6e", "ed_6f", "ed_70", "ed_71",
    "ed_72", "ed_73", "ed_74", "ed_75", "ed_76", "ed_77", "ed_78", "ed_79", "ed_7a", "ed_7b",
    "ed_7c", "ed_7d", "ed_7e", "ed_7f", "ed_a0", "ed_a1", "ed_a2", "ed_a3", "ed_a8", "ed_a9",
    "ed_aa", "ed_ab", "ed_b0", "ed_b1", "ed_b2", "ed_b3", "ed_b8", "ed_b9", "ed_ba", "ed_bb",
];

/// `cb_00` through `cb_ff`.
///
/// Generated because the `CB` page is complete: all 256 opcodes exist and all 256
/// files do. The base page cannot be generated the same way — four of its 256 are
/// prefixes with no file of their own — which is why only this half is.
fn cb_stems() -> Vec<String> {
    (0..=255u8).map(|op| format!("cb_{op:02x}")).collect()
}

/// The 252 plain `dd_*` stems and the 252 `fd_*` ones.
///
/// Generated with one exclusion, and the exclusion is a rule rather than a list of
/// gaps: a prefix byte is not an opcode, so `dd_cb`, `dd_dd`, `dd_ed` and `dd_fd`
/// have no file of their own — the first is the double-prefix page, whose files are
/// named `dd_cb____NN`, and the other three restart the prefix. That is the same
/// reason [`DEFAULT`] omits those four stems from the base page.
///
/// So this page can be generated where [`ED`] could not: 252 of its 256 encodings
/// have a file, against 80 of 256 there. Confirmed against the directory — 252
/// two-digit stems per prefix, and the four names above are the only ones missing.
fn index_stems() -> Vec<String> {
    let mut v = Vec::with_capacity(504);
    for prefix in ["dd", "fd"] {
        for op in 0..=255u8 {
            if matches!(op, 0xCB | 0xDD | 0xED | 0xFD) {
                continue;
            }
            v.push(format!("{prefix}_{op:02x}"));
        }
    }
    v
}

fn main() {
    // Anchored to the manifest so the tool works from any directory, matching
    // `fetchz80`. A relative path would silently look in the wrong place.
    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/z80"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let generated = cb_stems();
    let index = index_stems();
    let stems: Vec<&str> = if args.is_empty() {
        DEFAULT
            .iter()
            .copied()
            .chain(generated.iter().map(String::as_str))
            .chain(ED.iter().copied())
            .chain(index.iter().map(String::as_str))
            .collect()
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
