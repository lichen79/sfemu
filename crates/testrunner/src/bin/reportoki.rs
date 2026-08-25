//! Reports the OKI MSM6295 suite the way `reportym` reports the OPM one:
//! `cases: N/1000`, then a line per failure.
//!
//! Separate from `tests/okisuite.rs` for the same reason: a `#[test]` can only
//! pass or fail, and the number a human wants after touching the core is the
//! count.
//!
//! ```text
//! cargo run -q -p testrunner --release --bin reportoki -- --test suite
//! cargo run -q -p testrunner --release --bin reportoki -- --case 137
//! ```
//!
//! `--case N` dumps one case's first divergence with the samples on either side
//! and both the reference's values and the core's, which is how this
//! sub-project's debugging is meant to be done — from the failure diff, not from
//! a standalone harness.
//!
//! # `--test suite` is accepted, and so is no argument at all
//!
//! `reportym` takes only the bare form, and the gate for it was written as
//! `reportym -- --test suite` — which printed usage and **exited 0**, a hole that
//! went unnoticed. Both spellings are accepted here so neither habit silently
//! runs nothing, and an unrecognised argument exits 2.

// Each `bin` is its own crate root, so `lib.rs`'s attribute does not reach here.
#![forbid(unsafe_code)]

use testrunner::{okifiles, okifmt, okirunner};

/// How many failing cases to name before the list stops being useful.
const SHOWN: usize = 20;

/// Samples of context on either side of a `--case` divergence.
const CONTEXT: usize = 4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cases = match okifiles::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["--test", "suite"] => summary(&cases),
        ["--case", n] => match n.parse::<usize>() {
            Ok(i) if i < cases.len() => one_case(&cases[i], i),
            Ok(i) => {
                eprintln!("case {i} is out of range: the suite has {}", cases.len());
                std::process::exit(2);
            }
            Err(e) => {
                eprintln!("--case wants a number: {e}");
                std::process::exit(2);
            }
        },
        _ => {
            eprintln!("usage: reportoki [--test suite | --case N]");
            std::process::exit(2);
        }
    }
}

/// Prints the count, then the failing cases.
fn summary(cases: &[okifmt::Case]) {
    let mut passed = 0usize;
    let mut failed: Vec<(usize, okirunner::Mismatch)> = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        match okirunner::run_case(case) {
            Ok(()) => passed += 1,
            Err(m) => failed.push((i, m)),
        }
    }

    println!("cases: {passed}/{}", cases.len());
    if !failed.is_empty() {
        // Which field diverged first, over the whole run. A core with a wrong
        // address walk fails on `nibbles` and one with a wrong decoder on `mono`,
        // and the split says which file to open before any case is read.
        let mut by_field: Vec<(String, usize)> = Vec::new();
        for (_, m) in &failed {
            let name = m.field.to_string();
            match by_field.iter_mut().find(|(n, _)| *n == name) {
                Some((_, c)) => *c += 1,
                None => by_field.push((name, 1)),
            }
        }
        by_field.sort_by(|a, b| b.1.cmp(&a.1));
        println!("\nfailed ({}):", failed.len());
        for (name, count) in &by_field {
            println!("  first divergence on {name}: {count}");
        }
        for (i, m) in failed.iter().take(SHOWN) {
            println!("  case {i} (seed {}): {m}", cases[*i].seed);
        }
        if failed.len() > SHOWN {
            println!("  ... and {} more", failed.len() - SHOWN);
        }
        println!("\nrun `--case N` for the samples around one case's divergence");
        std::process::exit(1);
    }

    // A green run over the wrong number of cases is the failure this binary
    // exists to make visible: a truncated file would print `cases: 3/3` and look
    // like success. `okifiles::load` checks this too; the assertion stays because
    // this binary is the gate, and a gate that trusts one check upstream of it is
    // a gate that stops working when that check moves.
    assert_eq!(
        passed,
        okifiles::EXPECTED,
        "every case passed but the total is short -- the file and the inventory \
         disagree about how many cases the suite holds"
    );
}

/// Dumps one case: its shape, its divergence, and the samples around it with
/// both sides' values.
fn one_case(case: &okifmt::Case, i: usize) {
    println!(
        "case {i}: seed {}, pin7 {}, {} writes, {} samples, {}-byte rom",
        case.seed,
        case.pin7,
        case.writes.len(),
        case.samples.len(),
        case.rom.len()
    );

    let Err(m) = okirunner::run_case(case) else {
        println!("  PASS");
        return;
    };
    println!("  {m}");

    // Replay to the divergence so the core's own values are available for the
    // context window, rather than storing every sample of every case to print
    // nine of them.
    let lo = m.sample.saturating_sub(CONTEXT);
    let hi = (m.sample + CONTEXT + 1).min(case.samples.len());
    let mut chip = oki::Oki::new();
    let mut wi = 0usize;
    println!(
        "  {:>5}  {:>26}  {:>26}",
        "n", "want (MAME)", "got (this core)"
    );
    println!(
        "  {:>5}  {:>10} {:>4} {:>4} {:>5}  {:>10} {:>4} {:>4} {:>5}",
        "", "mono", "stat", "voic", "nibs", "mono", "stat", "voic", "nibs"
    );
    for n in 0..hi {
        while let Some(w) = case.writes.get(wi) {
            if usize::from(w.at_sample) != n {
                break;
            }
            if n >= lo {
                println!("  {n:>5}  write {:02X}", w.byte);
            }
            chip.write(w.byte, &case.rom);
            wi += 1;
        }
        let voices = chip.voices_playing();
        let (mono, nibbles) = chip.step_2x_traced(&case.rom);
        let status = chip.status();
        if n >= lo {
            let e = case.samples[n];
            let mark = if n == m.sample { " <--" } else { "" };
            println!(
                "  {n:>5}  {:>10} {:>4X} {:>4X} {:>5X}  {:>10} {:>4X} {:>4X} {:>5X}{mark}",
                e.mono_2x, e.status, e.voices, e.nibbles, mono, status, voices, nibbles
            );
        }
    }
}
