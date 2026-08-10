//! Generates the YM2151 vector suite from ymfm.
//!
//! ```text
//! cargo run -q -p testrunner --release --bin genym
//! ```
//!
//! Fetches ymfm (BSD-3 reference code, not game code), builds
//! `crates/testrunner/src/ymgen.cpp` against it, runs it, and asserts the suite it
//! produced still discriminates. **No ROM is involved anywhere in this program.**
//!
//! # The floors are the point
//!
//! A generator that regressed to silence would produce a file the runner compares
//! sample-for-sample and passes on, because silence equals silence. So the four
//! floors below are checked before the file is accepted, and each comes from a
//! measured run rather than a guess:
//!
//! * **sound** — 198 of 200 probe cases produced a non-zero sample. Two did not:
//!   with total level near the cap and a slow decay, a patch can land under the
//!   DAC's own quantisation. The floor is 95%, which those two do not breach.
//! * **status** — the timer registers are written for every case but loaded for
//!   about 42% (one case in three plus the CSM eighth). Measured 83 of 200. The
//!   floor is 25%, well below the design ratio and well above zero.
//! * **release** — 191 of 200 cases had a different peak before and after the
//!   key-off, which is what shows the key-off took effect. The floor is 85%.
//! * **CSM** — one case in eight enables it, and the timer must actually fire.
//!   Checked separately below, because the status floor alone is satisfiable by the
//!   non-CSM cases and that is exactly the vacuity the format's docs warn about.
//!
//! `/testdata` is gitignored: this writes 2.97 MB there and none of it is committed.

use std::path::PathBuf;
use std::process::Command;
use testrunner::{ymfiles, ymfm, ymfmt};

/// Fraction of cases that must produce audible output.
const MIN_SOUND: f64 = 0.95;

/// Fraction that must set a status bit at some point.
const MIN_STATUS: f64 = 0.25;

/// Fraction whose peak differs across the key-off.
const MIN_RELEASE: f64 = 0.85;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = ymfiles::dir();
    std::fs::create_dir_all(&out_dir)?;
    let dest = ymfiles::path();

    // Scratch under the target directory rather than /tmp: the fetched tree is 1 MB
    // and target/ is already gitignored, so it neither pollutes the repo nor depends
    // on /tmp surviving, which on this machine it did not.
    let scratch = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/ymfm"));
    let built = ymfm::fetch_and_build(&scratch)?;

    let tmp = out_dir.join("vectors.aymv.part");
    println!("generating {} cases", ymfiles::EXPECTED);
    let out = Command::new(&built.binary)
        .arg(ymfiles::EXPECTED.to_string())
        .arg(&tmp)
        .output()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "ymgen failed ({}):\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    print!("{stdout}");
    let stats = ymfm::parse_stats(&stdout)?;

    let check = || -> Result<(), Box<dyn std::error::Error>> {
        let cases = ymfm::stat(&stats, "cases")?;
        if cases != ymfiles::EXPECTED as i64 {
            return Err(format!("generated {cases} cases, expected {}", ymfiles::EXPECTED).into());
        }
        let n = cases as f64;
        for (key, floor) in [
            ("cases_with_sound", MIN_SOUND),
            ("cases_with_status", MIN_STATUS),
            ("cases_with_release_change", MIN_RELEASE),
        ] {
            let got = ymfm::stat(&stats, key)?;
            let frac = got as f64 / n;
            println!("  {key}: {got}/{cases} = {frac:.3} (floor {floor:.2})");
            if frac < floor {
                return Err(format!(
                    "{key} is {got}/{cases} = {frac:.3}, below the floor of {floor:.2}. \
                     The register script has lost discriminating power — see the \
                     measurements in ymgen.cpp before changing this."
                )
                .into());
            }
        }

        // Parse what was written and check the file against the format's own
        // invariants, rather than trusting the generator's word for its own output.
        let bytes = std::fs::read(&tmp)?;
        let v = ymfmt::parse(&bytes)?;
        if v.cases.len() != ymfiles::EXPECTED {
            return Err(format!("parsed {} cases", v.cases.len()).into());
        }
        let mut csm_with_timer = 0;
        for (i, c) in v.cases.iter().enumerate() {
            if c.samples.len() != ymfiles::SAMPLES_PER_CASE {
                return Err(format!("case {i} has {} samples", c.samples.len()).into());
            }
            if c.seed != i as u32 {
                return Err(format!("case {i} has seed {}", c.seed).into());
            }
            let last = c.samples.last().expect("512 samples").status;
            if c.final_status != last {
                return Err(format!("case {i}: final_status {} vs {last}", c.final_status).into());
            }
            // Every case keys off at 256 — the measurement the layout rests on.
            let off = c
                .writes
                .iter()
                .filter(|w| w.reg == 0x08 && w.val & 0x78 == 0)
                .count();
            if off == 0 {
                return Err(format!("case {i} never keys off").into());
            }
            if c.writes.iter().any(|w| w.reg == 0x08 && w.at_sample == 256) {
                // fine: that is the key-off
            } else {
                return Err(format!("case {i} has no write at sample 256").into());
            }
            // CSM cases must show a timer A overflow, or the CSM comparison is
            // vacuous. Measured: a truncated timer value left every CSM case with no
            // overflow at all and the suite still looked healthy.
            let csm = c
                .writes
                .iter()
                .any(|w| w.reg == 0x14 && w.val & 0x80 != 0 && w.val & 0x01 != 0);
            if csm && c.samples.iter().any(|s| s.status & 0x01 != 0) {
                csm_with_timer += 1;
            }
        }
        let csm_expected = ymfiles::EXPECTED / 8;
        println!("  csm cases with a timer A overflow: {csm_with_timer}");
        if csm_with_timer < csm_expected {
            return Err(format!(
                "only {csm_with_timer} of {csm_expected} CSM cases saw timer A overflow. \
                 Without a firing timer the CSM comparison is vacuous — the lazy \
                 prepare() gate consumes the CSM key-on flag, so this is the only \
                 thing that distinguishes it from eager preparation."
            )
            .into());
        }
        Ok(())
    };

    if let Err(e) = check() {
        // A file that failed its own floors must not be left where the runner would
        // find it and pass on it.
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    std::fs::rename(&tmp, &dest)?;
    let size = std::fs::metadata(&dest)?.len();
    println!("wrote {} ({size} bytes)", dest.display());
    Ok(())
}
