//! The YM2151 vector inventory.
//!
//! # Why these are literals
//!
//! `EXPECTED` is 1,000 because the generator is asked for 1,000 cases, not because
//! the file on disk happens to hold that many. Reading the count out of the data
//! would make a truncated or half-generated file into a smaller passing suite —
//! the same reasoning as [`crate::z80files`].

/// The number of cases the suite must contain.
pub const EXPECTED: usize = 1000;

/// Samples per case. 512 at 55,930 Hz is 9.2 ms — long enough for an attack, a
/// key-off at sample 256, and a measurable release tail.
pub const SAMPLES_PER_CASE: usize = 512;

/// What to tell the user when the vectors are missing.
///
/// Every loud failure quotes this one string, for the reason
/// [`crate::z80files::FETCH_HINT`] gives: duplicated across the harness, one copy
/// goes stale and sends a reader to a command that no longer exists.
pub const FETCH_HINT: &str = "run `cargo run -q -p testrunner --release --bin genym`";

/// Where the generated vectors live.
///
/// `CARGO_MANIFEST_DIR` rather than the working directory, so `cargo test` and
/// `cargo run` find the same suite from whatever directory they are invoked in.
#[must_use]
pub fn dir() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/ym2151"
    ))
}

/// The single vector file's path.
///
/// One file rather than one per case: 1,000 cases is 3.66 MB, and 1,000 files of
/// 3.6 KB each would spend more time in `open` than in comparison.
#[must_use]
pub fn path() -> std::path::PathBuf {
    dir().join("vectors.aymv")
}

/// Reads and parses the vector file.
///
/// # Errors
///
/// If the file is missing or unreadable, or does not parse. Both messages name the
/// path and [`FETCH_HINT`], because a bare `NotFound` from deep inside a test run
/// tells the reader nothing about how to fix it.
pub fn load() -> Result<crate::ymfmt::Vectors, Box<dyn std::error::Error>> {
    let p = path();
    let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e} — {FETCH_HINT}", p.display()))?;
    crate::ymfmt::parse(&bytes).map_err(|e| format!("{}: {e} — {FETCH_HINT}", p.display()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vectors are present, and their absence fails loudly by name.
    ///
    /// No `#[ignore]`, no env-var escape hatch: a missing suite is a failure with
    /// the path and the command in the message. The one documented exception in
    /// this repository is the real-ROM boot test.
    #[test]
    fn the_vectors_are_present() {
        let p = path();
        assert!(p.exists(), "missing {}: {FETCH_HINT}", p.display());
        let v = load().expect("the vectors parse");
        assert_eq!(v.cases.len(), EXPECTED, "case count in {}", p.display());
        for (i, c) in v.cases.iter().enumerate() {
            assert_eq!(c.samples.len(), SAMPLES_PER_CASE, "case {i}");
        }
    }
}
