//! Generates the OKI MSM6295 vector suite.
//!
//! ```text
//! cargo run -q -p testrunner --release --bin genoki
//! ```
//!
//! Downloads MAME's `okiadpcm.{h,cpp}` (BSD-3 reference *code*, © Andrew Gardner
//! and Aaron Giles — not game code), writes the three-line `emu.h` shim that
//! `okiadpcm.cpp`'s one include needs, compiles
//! [`okigen.cpp`](../../src/testrunner/okigen.cpp.html) against them, runs it,
//! and then **re-parses the result rather than trusting the generator's word for
//! its own output**. `/testdata` is gitignored: this writes ~310 MB there and
//! none of it is committed. No ROM is involved anywhere in this program.
//!
//! # The floors are the point
//!
//! A generator that regressed to silence would produce a file the runner
//! compares sample-for-sample and passes on, because silence equals silence. So
//! the premises below are checked before the file is accepted, and the counted
//! ones come from a measured run. **If a later run reports lower figures, do not
//! lower the floor to match** — a suite that exercises less than this does not
//! test the clamp, and the fix is the generator.
//!
//! # Why several checks read the ROM back
//!
//! The tempting way to count refused phrases is `if i % 4 == 3 { refused += 1 }`,
//! which is what the plan proposed. That counts indices, not refusals: it passes
//! on a generator that stopped emitting refused phrases altogether. Every premise
//! here is instead read out of the case's own bytes — the phrase-table entry, the
//! command script, the recorded nibbles — so a generator that stopped producing
//! one cannot satisfy the check that it did.

use std::path::{Path, PathBuf};
use std::process::Command;
use testrunner::{okifiles, okifmt};

/// At least this fraction of cases must reach the chip's own `+-65536` bound.
/// Measured: two voices open at unity gain in every case, so the bound is
/// reached in all of them; the floor leaves room for the fixture to change
/// without becoming vacuous.
const MIN_CLAMPED: f64 = 0.90;

/// At least this fraction of samples across the suite must be non-zero.
const MIN_AUDIBLE: f64 = 0.80;

/// At least this many cases must carry a phrase the chip refuses (`start >=
/// stop`) **and** a command that tries to play it. Every fourth case is built
/// that way, so the design figure is 250.
const MIN_REFUSED_CASES: usize = 200;

/// MAME's sound sources, pinned to a release tag rather than `master`: the
/// vectors this repository was verified against came from this exact revision,
/// and `master` moving would silently change them.
const MAME: &str = "https://raw.githubusercontent.com/mamedev/mame/mame0261/src/devices/sound";

/// The two fetched files and their measured line counts.
///
/// A tripwire, not a checksum — but at a pinned tag the content cannot drift, so
/// what this really catches is a truncated download or a proxy's error page
/// landing in the file, which would otherwise fail later as a confusing compile
/// error. The arithmetic lines checked below are the substance the suite rests
/// on.
const FETCH: [(&str, usize); 2] = [("okiadpcm.h", 76), ("okiadpcm.cpp", 260)];

/// The two lines of `okiadpcm.cpp` that *are* the decoder. If either changed,
/// every vector in the suite changed with it.
const ARITHMETIC: [&str; 2] = [
    "const int8_t oki_adpcm_state::s_index_shift[8] = { -1, -1, -1, -1, 2, 4, 6, 8 };",
    "int stepval = floor(16.0 * pow(11.0 / 10.0, (double)step));",
];

/// The chip's own output bound, in the 2x domain. `oki::chip::CLAMP_2X`.
const CLAMP_2X: i32 = 65_536;

/// Where `okigen.cpp` puts the step ladder, and the phrase that plays it.
const LADDER_PHRASE: usize = 1;
/// The phrase whose start and stop are swapped in every fourth case.
const REFUSED_PHRASE: usize = 2;
/// The phrase covering the top 64 bytes of the 18-bit address space.
const TOP_PHRASE: usize = 3;
const LADDER_START: u32 = 0x400;
const LADDER_STOP: u32 = 0x43F;
const TOP_START: u32 = 0x3_FFC0;
const TOP_STOP: u32 = 0x3_FFFF;
/// How many samples the ladder's leading run of nibble 7 covers.
const LADDER_SEVENS: usize = 32;
/// And how many samples the whole ladder covers.
const LADDER_SAMPLES: usize = 128;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = okifiles::dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    // Scratch under `target/`, not under `testdata/`: `testdata/` is the suite,
    // and a compiled binary and a copy of MAME's sources sitting in it turn any
    // later measurement of the suite's size or contents into a guess. `target/`
    // is already gitignored and is where `genym` puts its scratch too.
    let build = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/okigen"));
    std::fs::create_dir_all(&build).map_err(|e| format!("{}: {e}", build.display()))?;
    fetch(&build)?;

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/okigen.cpp");
    let exe = build.join("okigen");
    println!("compiling {}", src.display());
    run(Command::new("c++")
        .args(["-std=c++17", "-O2", "-I"])
        .arg(&build)
        .arg(&src)
        .arg(build.join("okiadpcm.cpp"))
        .arg("-o")
        .arg(&exe))?;

    let part = okifiles::path().with_extension("part");
    println!("generating {} cases", okifiles::EXPECTED);
    if let Err(e) = run(Command::new(&exe).arg(&part)) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }

    if let Err(e) = validate(&part) {
        // A file that failed its own premises must not be left where the runner
        // would find it and pass on it.
        let _ = std::fs::remove_file(&part);
        return Err(format!("the generator's output did not validate: {e}").into());
    }

    let dest = okifiles::path();
    std::fs::rename(&part, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let size = std::fs::metadata(&dest)?.len();
    println!("wrote {} ({size} bytes)", dest.display());
    Ok(())
}

/// Runs `cmd`, echoing its stderr, and failing with the command and that stderr.
fn run(cmd: &mut Command) -> Result<(), Box<dyn std::error::Error>> {
    let shown = format!("{cmd:?}");
    let out = cmd.output().map_err(|e| format!("{shown}: {e}"))?;
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        eprint!("{err}");
    }
    if !out.status.success() {
        return Err(format!("{shown} failed ({})", out.status).into());
    }
    Ok(())
}

/// Fetches MAME's decoder into `build` and writes the `emu.h` shim.
///
/// Re-fetches whenever the cached copy fails its checks rather than trusting
/// `exists()`: a half-written download would otherwise be reused forever.
fn fetch(build: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for (name, lines) in FETCH {
        let to = build.join(name);
        if check_source(&to, name, lines).is_err() {
            let url = format!("{MAME}/{name}");
            println!("fetching {url}");
            run(Command::new("curl")
                .args(["-sfL", "--retry", "3", "-o"])
                .arg(&to)
                .arg(&url))?;
        }
        check_source(&to, name, lines)?;
    }
    // MAME's okiadpcm.cpp includes emu.h and needs nothing from it but these two
    // headers. Written every run, so a stale or truncated shim cannot persist.
    std::fs::write(
        build.join("emu.h"),
        "#pragma once\n#include <cstdint>\n#include <cmath>\n",
    )
    .map_err(|e| format!("emu.h: {e}"))?;
    Ok(())
}

/// Checks one fetched file's line count, and `okiadpcm.cpp`'s arithmetic.
fn check_source(at: &Path, name: &str, lines: usize) -> Result<(), Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(at).map_err(|e| format!("{}: {e}", at.display()))?;
    let got = text.lines().count();
    if got != lines {
        return Err(format!("{}: {got} lines, expected {lines}", at.display()).into());
    }
    if name == "okiadpcm.cpp" {
        for want in ARITHMETIC {
            if !text.contains(want) {
                return Err(format!(
                    "{}: MAME's decoder no longer contains `{want}`. The suite's \
                     arithmetic changed; regenerating would silently replace every \
                     vector this repository was verified against.",
                    at.display()
                )
                .into());
            }
        }
    }
    Ok(())
}

/// Reads back what the generator wrote and checks it against the premises the
/// suite rests on.
#[allow(clippy::too_many_lines)]
fn validate(part: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(part).map_err(|e| format!("{}: {e}", part.display()))?;
    let cases = okifmt::parse(&bytes).map_err(|e| format!("{}: {e}", part.display()))?;
    if cases.len() != okifiles::EXPECTED {
        return Err(format!("{} cases, expected {}", cases.len(), okifiles::EXPECTED).into());
    }
    let mut clamped = 0usize;
    let mut refused = 0usize;
    let mut nonzero = 0usize;
    let mut total = 0usize;
    let mut four_voice_samples = 0usize;
    let mut silent_volume_starts = 0usize;
    for (i, c) in cases.iter().enumerate() {
        let bad = |m: String| -> Box<dyn std::error::Error> { format!("case {i}: {m}").into() };
        if c.seed as usize != i {
            return Err(bad(format!("carries seed {}", c.seed)));
        }
        if c.samples.len() != okifiles::SAMPLES_PER_CASE {
            return Err(bad(format!("has {} samples", c.samples.len())));
        }
        if c.rom.len() != okifiles::ROM_BYTES {
            return Err(bad(format!("has a {}-byte rom", c.rom.len())));
        }
        if c.writes.is_empty() {
            return Err(bad("writes nothing".into()));
        }
        if !c
            .writes
            .windows(2)
            .all(|w| w[0].at_sample <= w[1].at_sample)
        {
            return Err(bad("writes are not in sample order".into()));
        }
        if c.pin7 != (i % 2 == 1) {
            return Err(bad(format!("pin7 is {}", c.pin7)));
        }

        // The three reserved phrase-table entries, read back out of the ROM.
        let (ls, lp) = (phrase(&c.rom, LADDER_PHRASE), phrase(&c.rom, TOP_PHRASE));
        if ls != (LADDER_START, LADDER_STOP) {
            return Err(bad(format!("ladder phrase is {ls:#X?}")));
        }
        if lp != (TOP_START, TOP_STOP) {
            return Err(bad(format!("top-of-rom phrase is {lp:#X?}")));
        }

        // The ladder's nibbles, as the file recorded them. This is what makes the
        // step index cross its whole range: 32 sevens drive it to 48 and hold it,
        // then 96 zeros drive it to 0 and hold it. Random nibbles reach 1..48
        // only, so without this the lower step clamp is never exercised.
        for (n, s) in c.samples.iter().take(LADDER_SAMPLES).enumerate() {
            let want = u16::from(n < LADDER_SEVENS) * 7;
            if s.nibbles & 0x0F != want {
                return Err(bad(format!(
                    "sample {n}: voice 0 consumed {:#X}, the ladder says {want:#X}",
                    s.nibbles & 0x0F
                )));
            }
        }
        // And voice 1's first nibble must be the high nibble of the ROM's very
        // last 64 bytes — the top of the 18-bit bus, which nothing else reaches.
        // Checked against the ROM the file carries, so a generator whose address
        // walk drifted cannot agree with itself here.
        let want = u16::from(c.rom[TOP_START as usize] >> 4);
        if (c.samples[0].nibbles >> 4) & 0x0F != want {
            return Err(bad(format!(
                "voice 1's first nibble is {:#X}, the top of the rom says {want:#X}",
                (c.samples[0].nibbles >> 4) & 0x0F
            )));
        }

        // A refused phrase counted from the data: the table entry must actually
        // be invalid *and* the script must actually try to play it.
        let (rs, rp) = phrase(&c.rom, REFUSED_PHRASE);
        let (tried, silent_starts) = scan_script(&c.writes, REFUSED_PHRASE as u8);
        silent_volume_starts += silent_starts;
        if rs >= rp && tried {
            refused += 1;
            // The refusal must be what left the voice silent. Voice 3 is stopped
            // at sample 1 and asked for this phrase at sample 1, so it must not
            // be playing at sample 1.
            if c.samples[1].voices & 0b1000 != 0 {
                return Err(bad(
                    "voice 3 played a phrase whose start is not below its stop".into(),
                ));
            }
        }

        for (n, s) in c.samples.iter().enumerate() {
            if s.status != 0xF0 | (s.status & 0x0F) || s.voices & !0x0F != 0 {
                return Err(bad(format!("sample {n}: status {:#04X}", s.status)));
            }
            // `voices` is who sounded during the sample and `status` is who is
            // still playing after it, so the second is a subset of the first.
            if s.status & 0x0F & !s.voices != 0 {
                return Err(bad(format!(
                    "sample {n}: status {:#04X} claims a voice that did not sound ({:#04X})",
                    s.status, s.voices
                )));
            }
            if s.mono_2x.abs() > CLAMP_2X {
                return Err(bad(format!("sample {n}: {} exceeds the clamp", s.mono_2x)));
            }
            if s.mono_2x != 0 {
                nonzero += 1;
            }
            if s.voices == 0x0F {
                four_voice_samples += 1;
            }
        }
        total += c.samples.len();
        if c.samples.iter().any(|s| s.mono_2x.abs() == CLAMP_2X) {
            clamped += 1;
        }
    }

    let clamp_frac = clamped as f64 / cases.len() as f64;
    let audible = nonzero as f64 / total as f64;
    println!("  cases reaching the +-{CLAMP_2X} bound: {clamped}/{} = {clamp_frac:.3} (floor {MIN_CLAMPED:.2})", cases.len());
    println!("  samples non-zero: {nonzero}/{total} = {audible:.3} (floor {MIN_AUDIBLE:.2})");
    println!("  cases carrying a refused phrase and a command for it: {refused} (floor {MIN_REFUSED_CASES})");
    println!("  samples with all four voices sounding: {four_voice_samples}");
    println!("  starts at a silent volume index (9..15): {silent_volume_starts}");
    if clamp_frac < MIN_CLAMPED {
        return Err(format!(
            "only {clamp_frac:.3} of cases reach the clamp, floor {MIN_CLAMPED}. The \
             fixture has lost the two unity-gain voices it opens with — fix the \
             generator, not the floor."
        )
        .into());
    }
    if audible < MIN_AUDIBLE {
        return Err(format!(
            "only {audible:.3} of samples are non-zero, floor {MIN_AUDIBLE}. A suite \
             of silence is one the runner passes on for free."
        )
        .into());
    }
    if refused < MIN_REFUSED_CASES {
        return Err(format!(
            "only {refused} cases carry a refused phrase and a command for it, floor \
             {MIN_REFUSED_CASES}"
        )
        .into());
    }
    if four_voice_samples == 0 {
        return Err("no sample had all four voices sounding".into());
    }
    if silent_volume_starts == 0 {
        return Err("no start used a silent volume index (9..15)".into());
    }
    Ok(())
}

/// Walks a command script the way the chip's own state machine does, returning
/// whether `phrase` was ever asked for and how many starts used a silent volume
/// index (9..15, whose table entries are exactly zero).
///
/// A state machine rather than a `windows(2)` pattern match, because which bytes
/// are data bytes depends on what came before them: `0x80 | phrase` followed by
/// `0xF3` is a start, but the same `0xF3` on its own is a latch. Matching pairs
/// positionally would miscount both figures, and in a fixture the generator
/// controls it would miscount them *consistently* — which is exactly the kind of
/// agreement that reads as a passing check.
fn scan_script(writes: &[okifmt::Write_], phrase: u8) -> (bool, usize) {
    let mut pending: Option<u8> = None;
    let mut tried = false;
    let mut silent = 0;
    for w in writes {
        if let Some(latched) = pending.take() {
            if w.byte >> 4 != 0 {
                if latched == phrase {
                    tried = true;
                }
                if w.byte & 0x0F >= 9 {
                    silent += 1;
                }
            }
        } else if w.byte & 0x80 != 0 {
            pending = Some(w.byte & 0x7F);
        }
    }
    (tried, silent)
}

/// One phrase-table entry: 3-byte big-endian start then stop, at `phrase * 8`.
fn phrase(rom: &[u8], phrase: usize) -> (u32, u32) {
    let at =
        |a: usize| u32::from(rom[a]) << 16 | u32::from(rom[a + 1]) << 8 | u32::from(rom[a + 2]);
    (at(phrase * 8), at(phrase * 8 + 3))
}
