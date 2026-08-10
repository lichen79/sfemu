//! Fetches ymfm, builds [`ymgen.cpp`](../../src/testrunner/ymgen.cpp.html) against
//! it, and runs it.
//!
//! # ymfm is fetched, never vendored
//!
//! ymfm is BSD-3 reference *code*, © 2021 Aaron Giles — the FM implementation MAME
//! uses. It is downloaded at generate time, compiled, and left in a scratch
//! directory. Nothing from it is committed, and **no ROM is involved**: the only URL
//! in this module is a source archive.
//!
//! # It re-fetches every run rather than caching
//!
//! `/tmp` is not durable on macOS — the scratch copy used during the spec work was
//! deleted mid-session, which is why this is a design requirement rather than an
//! oversight. A cache that is sometimes there and sometimes not is worse than no
//! cache: it makes a generate run reproducible only by luck.
//!
//! # The line count is a tripwire, not a checksum
//!
//! Upstream `main` moves. A checksum would fail on any commit; the line count fails
//! only when the OPM path itself changes size, which is when the generated vectors
//! stop being comparable to the ones this repository was verified against. Either
//! way the driver **stops** — it does not regenerate silently.

use std::path::{Path, PathBuf};
use std::process::Command;

/// ymfm's source archive. BSD-3, © 2021 Aaron Giles — the FM implementation MAME
/// uses. Reference *code*, not game code: nothing here touches a ROM.
pub const YMFM_URL: &str = "https://github.com/aaronsgiles/ymfm/archive/refs/heads/main.zip";

/// The OPM path is five files and no build system, measured at 3,482 lines:
/// `ymfm.h`, `ymfm_fm.h`, `ymfm_fm.ipp`, `ymfm_opm.h`, `ymfm_opm.cpp`.
///
/// Asserted after every fetch. A count that moves means upstream changed and the
/// vectors may not be comparable to the ones this repository was verified against.
pub const YMFM_LINES: usize = 3_482;

/// The five files [`YMFM_LINES`] counts, in the order they are summed.
pub const YMFM_FILES: [&str; 5] = [
    "ymfm.h",
    "ymfm_fm.h",
    "ymfm_fm.ipp",
    "ymfm_opm.h",
    "ymfm_opm.cpp",
];

/// The generator's own source, beside this file in the repository.
///
/// `CARGO_MANIFEST_DIR` for the same reason the vector paths use it: the build runs
/// from wherever the developer is standing.
#[must_use]
pub fn generator_source() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ymgen.cpp"))
}

/// Everything the driver needs after a successful fetch and build.
#[derive(Clone, Debug)]
pub struct Built {
    /// The compiled generator.
    pub binary: PathBuf,
    /// The scratch directory holding the fetched tree and the binary.
    pub scratch: PathBuf,
    /// The measured line count, which equals [`YMFM_LINES`] or the fetch failed.
    pub lines: usize,
}

/// Runs `cmd`, returning its stdout, and failing with the command and stderr.
fn run(cmd: &mut Command) -> Result<String, Box<dyn std::error::Error>> {
    let shown = format!("{cmd:?}");
    let out = cmd.output().map_err(|e| format!("{shown}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{shown} failed ({}):\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Counts the lines of the five OPM files, failing if any is missing.
fn count_lines(src: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let mut total = 0;
    for name in YMFM_FILES {
        let p = src.join(name);
        let text = std::fs::read_to_string(&p)
            .map_err(|e| format!("{}: {e} — is the archive layout unchanged?", p.display()))?;
        total += text.lines().count();
    }
    Ok(total)
}

/// Fetches ymfm into `scratch`, verifies it, and builds the generator.
///
/// Shells out to `curl`, `unzip`, and `c++` for the reason the other fetchers do:
/// this runs once per checkout, and an empty dependency tree is worth more than
/// elegance.
///
/// # Errors
///
/// If any tool is missing or fails, if the archive layout changed, or if the
/// measured line count is not [`YMFM_LINES`] — in which case the message names both
/// numbers and nothing is generated.
pub fn fetch_and_build(scratch: &Path) -> Result<Built, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(scratch)?;
    let zip = scratch.join("ymfm.zip");
    println!("fetching {YMFM_URL}");
    run(Command::new("curl")
        .args(["-sfL", "--retry", "3", "-o"])
        .arg(&zip)
        .arg(YMFM_URL))?;
    run(Command::new("unzip")
        .arg("-oq")
        .arg(&zip)
        .arg("-d")
        .arg(scratch))?;

    let src = scratch.join("ymfm-main/src");
    let lines = count_lines(&src)?;
    if lines != YMFM_LINES {
        // Stop rather than regenerate against a moved upstream. The user can then
        // decide whether to pin a commit or accept the new count.
        return Err(format!(
            "ymfm's OPM path is {lines} lines, expected {YMFM_LINES}. Upstream moved; \
             the generated vectors may not be comparable to the ones this repository \
             was verified against. Pin a commit or update YMFM_LINES deliberately."
        )
        .into());
    }
    println!("ymfm verified at {lines} lines");

    let binary = scratch.join("ymgen");
    let gen_src = generator_source();
    println!("building {}", gen_src.display());
    run(Command::new("c++")
        .args(["-std=c++17", "-O2", "-I"])
        .arg(&src)
        .arg(&gen_src)
        .arg(src.join("ymfm_opm.cpp"))
        .arg("-o")
        .arg(&binary))?;

    Ok(Built {
        binary,
        scratch: scratch.to_path_buf(),
        lines,
    })
}

/// One `key value` line of the generator's statistics report.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Stat {
    pub key: String,
    pub value: i64,
}

/// Parses the generator's stdout into its statistics.
///
/// # Errors
///
/// If a line is not `key <integer>`. The generator's whole output is statistics, so
/// an unparseable line means it printed a diagnostic the driver should not swallow.
pub fn parse_stats(stdout: &str) -> Result<Vec<Stat>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let (key, value) = line
            .split_once(' ')
            .ok_or_else(|| format!("generator printed {line:?}, expected `key value`"))?;
        out.push(Stat {
            key: key.to_string(),
            value: value
                .trim()
                .parse()
                .map_err(|e| format!("generator printed {line:?}: {e}"))?,
        });
    }
    Ok(out)
}

/// Looks up one statistic, failing if the generator did not report it.
///
/// # Errors
///
/// If `key` is absent — which means the generator and this driver disagree about
/// what was measured, and the driver's assertions would silently pass.
pub fn stat(stats: &[Stat], key: &str) -> Result<i64, Box<dyn std::error::Error>> {
    stats
        .iter()
        .find(|s| s.key == key)
        .map(|s| s.value)
        .ok_or_else(|| format!("generator reported no {key:?}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The URL is a source archive and names no ROM.
    ///
    /// This repository never fetches game code. The check is mechanical rather than
    /// rhetorical: the one URL here must be ymfm's own repository over HTTPS.
    #[test]
    fn the_only_url_is_ymfms_own_source() {
        assert!(YMFM_URL.starts_with("https://github.com/aaronsgiles/ymfm/"));
        assert!(YMFM_URL.ends_with(".zip"));
    }

    /// The line count is the sum over exactly the five OPM files.
    ///
    /// Both halves are asserted because either alone is satisfiable by a wrong
    /// value: a list of five names says nothing about the total, and a total says
    /// nothing about which files it covers. The OPM path needs `ymfm.h` for the
    /// interface, `ymfm_fm.h`/`.ipp` for the engine, and the two `opm` files — the
    /// OPL, OPN, PCM, SSG and ADPCM sources are not compiled and not counted.
    #[test]
    fn the_line_count_covers_the_five_opm_files() {
        assert_eq!(YMFM_FILES.len(), 5);
        assert_eq!(YMFM_LINES, 3_482);
        for f in YMFM_FILES {
            assert!(f.starts_with("ymfm"), "{f}");
        }
        for absent in [
            "ymfm_opl.cpp",
            "ymfm_opn.cpp",
            "ymfm_pcm.cpp",
            "ymfm_ssg.cpp",
        ] {
            assert!(!YMFM_FILES.contains(&absent), "{absent} is not compiled");
        }
    }

    /// The statistics parser reads the generator's report and rejects anything else.
    #[test]
    fn the_statistics_parser_reads_key_value_lines() {
        let s = parse_stats("cases 1000\nbytes 2965009\ncases_with_sound 991\n").expect("parses");
        assert_eq!(s.len(), 3);
        assert_eq!(stat(&s, "cases").expect("present"), 1000);
        assert_eq!(stat(&s, "cases_with_sound").expect("present"), 991);
        // A missing key must be an error, not a zero: a driver that read a missing
        // `cases_with_sound` as 0 would then compare 0 against its floor and the
        // assertion would be the one thing it was written to prevent.
        assert!(stat(&s, "cases_with_noise").is_err(), "absent is an error");
        // And a diagnostic the generator wrote to stdout must not be swallowed.
        assert!(parse_stats("something went wrong\n").is_err());
        assert!(parse_stats("cases lots\n").is_err());
        // Blank lines are ignored, since the report ends with a newline.
        assert_eq!(parse_stats("cases 1\n\n").expect("parses").len(), 1);
    }

    /// The generator's source is beside this module and is the file that is built.
    #[test]
    fn the_generator_source_is_in_the_repository() {
        let p = generator_source();
        assert!(p.exists(), "missing {}", p.display());
        let text = std::fs::read_to_string(&p).expect("readable");
        // The three measurements that shaped the register script must stay recorded
        // where the next person to edit the script will see them.
        assert!(
            text.contains("STRUCTURED, NOT RANDOM"),
            "the silence measurement"
        );
        assert!(text.contains("release rate"), "the key-off measurement");
        assert!(text.contains("status byte"), "the timer measurement");
        // And it must not have grown a URL of its own — the one URL lives here.
        assert!(!text.contains("http"), "the generator fetches nothing");
    }
}
