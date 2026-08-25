//! Downloads the Z80 vector suite and converts it as it goes.
//!
//! Upstream is 1.37 GB of JSON and this machine has under 3 GiB free, so nothing
//! is kept: each file is downloaded to a temporary path, converted to the
//! `Z80V` binary form (about 5.8x smaller), and the JSON deleted before the next
//! one starts. Peak extra disk is one JSON file plus the growing output — about
//! 236 MB when it finishes.
//!
//! Resumable, because 1,604 sequential downloads will be interrupted: a file whose
//! `.z80bin` already exists is skipped, and output is written to `.part` and
//! renamed only after a successful conversion, so an interrupted run leaves no
//! half-file for the suite to read.
//!
//! ```text
//! cargo run -q -p testrunner --release --bin fetchz80
//! ```
//!
//! Shells out to `curl` for the same reason [`fetch`](../fetch/index.html) does:
//! this runs once per checkout, and an empty dependency tree is worth more than
//! elegance.
//!
//! No ROM is involved. The vectors are MIT-licensed CPU test data from
//! SingleStepTests/z80; nothing here downloads game code.

// Each `bin` is its own crate root, so `lib.rs`'s attribute does not reach here.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::Command;
use testrunner::{z80files, z80fmt, z80json};

const BASE: &str = "https://raw.githubusercontent.com/SingleStepTests/z80/main/v1";

/// Cases per vector file, upstream. Asserted per file rather than trusted: a
/// truncated download that still parsed would otherwise silently shrink the suite.
const CASES: usize = 1000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Anchored to the manifest, not the working directory, so running from a
    // subdirectory writes to the same place rather than making a second suite.
    let out = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/z80"));
    std::fs::create_dir_all(&out)?;
    let names = z80files::all_names();
    assert_eq!(names.len(), z80files::EXPECTED, "inventory changed");
    println!("fetching {} files into {}", names.len(), out.display());

    let (mut done, mut fetched) = (0usize, 0usize);
    for (i, name) in names.iter().enumerate() {
        let stem = z80files::stem(name);
        let dest = out.join(format!("{stem}.z80bin"));
        if dest.exists() {
            done += 1;
            continue;
        }
        let tmp_json = out.join(format!("{stem}.json.part"));
        let tmp_bin = out.join(format!("{stem}.z80bin.part"));

        // The space is the only character here that needs encoding, and curl will
        // not do it for us. The `__` displacement marker is URL-safe as it stands.
        let url = format!("{BASE}/{}.json", name.replace(' ', "%20"));
        let st = Command::new("curl")
            .args(["-sfL", "--retry", "3", "-o"])
            .arg(&tmp_json)
            .arg(&url)
            .status()?;
        if !st.success() {
            // A failed transfer leaves a partial or empty file. Removing it here is
            // what makes the next run retry rather than parse the wreckage.
            let _ = std::fs::remove_file(&tmp_json);
            return Err(format!("curl failed for {name:?} ({url})").into());
        }

        let convert = || -> Result<(), Box<dyn std::error::Error>> {
            let text = std::fs::read_to_string(&tmp_json)?;
            let cases = z80json::parse(&text).map_err(|e| format!("parse {name:?}: {e}"))?;
            if cases.len() != CASES {
                return Err(format!("{name:?} has {} cases, expected {CASES}", cases.len()).into());
            }
            std::fs::write(&tmp_bin, z80fmt::write_file(&cases))?;
            std::fs::rename(&tmp_bin, &dest)?;
            Ok(())
        };
        let r = convert();
        // The JSON goes either way, and now rather than at the end: keeping 1,604 of
        // them is the thing this program exists to avoid.
        let _ = std::fs::remove_file(&tmp_json);
        if let Err(e) = r {
            let _ = std::fs::remove_file(&tmp_bin);
            return Err(e);
        }

        fetched += 1;
        if fetched % 50 == 0 {
            println!("[{}/{}] {} fetched", i + 1, names.len(), fetched);
        }
    }
    println!("z80 vectors: {fetched} fetched, {done} already present");
    Ok(())
}
