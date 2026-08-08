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

/// The opcodes Task 5 implements — the useful default before Task 7 lands.
const DEFAULT: &[&str] = &["00", "27", "2f", "37", "3f", "76", "f3", "fb"];

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
