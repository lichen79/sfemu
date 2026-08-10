//! Reports the YM2151 suite the way `reportz80` reports the Z80 one:
//! `cases: N/1000`, then a line per failure.
//!
//! Separate from `tests/ymsuite.rs` for the same reason `reportz80` is separate from
//! `tests/z80suite.rs`: a `#[test]` can only pass or fail, and the number a human
//! wants after touching the core is the count.
//!
//! ```text
//! cargo run -q -p testrunner --release --bin reportym
//! cargo run -q -p testrunner --release --bin reportym -- --case 137
//! ```
//!
//! `--case N` dumps one case's first divergence with the samples on either side,
//! which is how this sub-project's debugging is meant to be done — from the failure
//! diff, not from a standalone harness.

use testrunner::{ymfiles, ymfmt, ymrunner};

/// How many failing cases to name before the list stops being useful.
const SHOWN: usize = 20;

/// Samples of context on either side of a `--case` divergence.
const CONTEXT: usize = 4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let v = match ymfiles::load() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    match args.as_slice() {
        [] => summary(&v),
        [flag, n] if flag == "--case" => match n.parse::<usize>() {
            Ok(i) if i < v.cases.len() => one_case(&v, i),
            Ok(i) => {
                eprintln!("case {i} is out of range: the suite has {}", v.cases.len());
                std::process::exit(2);
            }
            Err(e) => {
                eprintln!("--case wants a number: {e}");
                std::process::exit(2);
            }
        },
        _ => {
            eprintln!("usage: reportym [--case N]");
            std::process::exit(2);
        }
    }
}

/// Prints the count, then the failing cases.
fn summary(v: &ymfmt::Vectors) {
    let mut passed = 0usize;
    let mut failed: Vec<(usize, ymrunner::Mismatch)> = Vec::new();
    // CSM cases are counted separately: until the lazy `prepare()` gate lands, they
    // are the predicted failure, and a bare total cannot tell that predicted set from
    // a general regression.
    let mut csm_failed = 0usize;

    for (i, case) in v.cases.iter().enumerate() {
        let r = ymrunner::run_case(case);
        if r.ok {
            passed += 1;
        } else {
            let m = r.first_mismatch.expect("a failure has a mismatch");
            if case
                .writes
                .iter()
                .any(|w| w.reg == 0x14 && w.val & 0x80 != 0)
            {
                csm_failed += 1;
            }
            failed.push((i, m));
        }
    }

    println!("cases: {passed}/{}", v.cases.len());
    if !failed.is_empty() {
        println!("\nfailed ({}, of which CSM: {csm_failed}):", failed.len());
        for (i, m) in failed.iter().take(SHOWN) {
            println!("  case {i} (seed {}): {m}", v.cases[*i].seed);
        }
        if failed.len() > SHOWN {
            println!("  ... and {} more", failed.len() - SHOWN);
        }
        println!("\nrun `--case N` for the samples around one case's divergence");
        std::process::exit(1);
    }

    // A green run over the wrong number of cases is the failure this binary exists to
    // make visible: a truncated file would print `cases: 3/3` and look like success.
    assert_eq!(
        passed,
        ymfiles::EXPECTED,
        "every case passed but the total is short -- the file and the inventory \
         disagree about how many cases the suite holds"
    );
}

/// Dumps one case: its script, its divergence, and the samples around it.
fn one_case(v: &ymfmt::Vectors, i: usize) {
    let case = &v.cases[i];
    println!(
        "case {i}: seed {}, {} writes, {} samples",
        case.seed,
        case.writes.len(),
        case.samples.len()
    );
    let csm = case
        .writes
        .iter()
        .any(|w| w.reg == 0x14 && w.val & 0x80 != 0);
    println!("  CSM: {csm}");

    let r = ymrunner::run_case(case);
    let Some(m) = r.first_mismatch else {
        println!("  PASS");
        return;
    };
    println!("  {m}");

    // Re-run to the divergence so the core's own samples are available for the
    // context window. Replayed rather than stored, because storing 512 samples per
    // case for 1,000 cases to print eight of them would be the wrong trade.
    let lo = m.sample.saturating_sub(CONTEXT);
    let hi = (m.sample + CONTEXT + 1).min(case.samples.len());
    let mut chip = ym2151::Ym2151::new();
    let mut buf = [(0i16, 0i16); 1];
    let mut w = 0usize;
    println!(
        "  {:>5}  {:>18}  {:>18}",
        "n", "want (ymfm)", "got (this core)"
    );
    for n in 0..hi {
        let at = u16::try_from(n).unwrap_or(u16::MAX);
        while w < case.writes.len() && case.writes[w].at_sample == at {
            if n >= lo {
                println!(
                    "  {:>5}  write {:02X}={:02X}",
                    n, case.writes[w].reg, case.writes[w].val
                );
            }
            chip.write(case.writes[w].reg, case.writes[w].val);
            w += 1;
        }
        chip.generate(&mut buf);
        let status = chip.read_status();
        if n >= lo {
            let e = &case.samples[n];
            let mark = if n == m.sample { " <--" } else { "" };
            println!(
                "  {n:>5}  {:>6} {:>6} {:02X}  {:>6} {:>6} {:02X}{mark}",
                e.left, e.right, e.status, buf[0].0, buf[0].1, status
            );
        }
    }
}
