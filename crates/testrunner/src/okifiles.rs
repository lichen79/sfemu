//! The OKI MSM6295 vector inventory.
//!
//! # Why these are literals
//!
//! `EXPECTED` is 1,000 because the generator is asked for 1,000 cases, not
//! because the file on disk happens to hold that many. Reading the count out of
//! the data would make a truncated or half-generated file into a smaller
//! passing suite — the same reasoning as [`crate::ymfiles`] and
//! [`crate::z80files`].

/// The number of cases the suite must contain.
pub const EXPECTED: usize = 1000;

/// Samples per case. 512 at 7,576 Hz is 68 ms — long enough for a phrase to
/// start, saturate, be interrupted and end.
pub const SAMPLES_PER_CASE: usize = 512;

/// How large a synthesised sample ROM is.
///
/// The real chip's address bus is 18 bits (`device_rom_interface<18>`), and
/// SF2's `oki` region is exactly this size. Kept at the true size rather than a
/// smaller window so that a case whose address walk runs off the end wraps
/// through the 18-bit mask, which is what the hardware does — with a short ROM
/// it would instead fall into the read-past-the-end path, a different branch.
///
/// 1,000 cases at 256 KB each is ~256 MB of `testdata/`. That is the cost of
/// checking the mask at its real boundary.
pub const ROM_BYTES: usize = 0x4_0000;

/// What to tell the user when the vectors are missing.
///
/// Every loud failure quotes this one string, for the reason
/// [`crate::z80files::FETCH_HINT`] gives: duplicated across the harness, one
/// copy goes stale and sends a reader to a command that no longer exists.
pub const FETCH_HINT: &str = "run `cargo run -q -p testrunner --release --bin genoki`";

/// Where the generated vectors live.
///
/// `CARGO_MANIFEST_DIR` rather than the working directory, so `cargo test` and
/// `cargo run` find the same suite from whatever directory they are invoked in.
#[must_use]
pub fn dir() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/oki"))
}

/// The single vector file's path.
#[must_use]
pub fn path() -> std::path::PathBuf {
    dir().join("vectors.aokv")
}

/// Reads and parses the vector file.
///
/// # Errors
///
/// If the file is missing, unreadable, does not parse, or does not hold
/// [`EXPECTED`] cases. Every message names the path and [`FETCH_HINT`]: a bare
/// `NotFound` from deep inside a test run tells the reader nothing about how to
/// fix it. There is no environment variable that turns any of this into a skip —
/// a suite that silently does not run is a suite that silently does not catch
/// anything.
pub fn load() -> Result<Vec<crate::okifmt::Case>, Box<dyn std::error::Error>> {
    let p = path();
    let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e} — {FETCH_HINT}", p.display()))?;
    let cases =
        crate::okifmt::parse(&bytes).map_err(|e| format!("{}: {e} — {FETCH_HINT}", p.display()))?;
    if cases.len() != EXPECTED {
        return Err(format!(
            "{}: holds {} cases, expected {EXPECTED} — {FETCH_HINT}",
            p.display(),
            cases.len()
        )
        .into());
    }
    Ok(cases)
}
