//! Does SF2's sound program actually drive the OKI, and is the mix audible?
//!
//! # Why this is `#[ignore]`d when the rest of the project forbids it
//!
//! The third and last such test, for `boot.rs`'s and `sound_boot.rs`'s reason: the
//! project rule is that missing test data **fails loudly**, naming the file and the
//! command that fetches it, and that rule holds because the rest of this project's test
//! data is legally fetchable and there *is* a command to name. This data is not. SF2 is
//! commercial Capcom code; there is no command we may put in a failure message.
//!
//! Supply a legally obtained set — Capcom Arcade Stadium, Capcom Fighting Collection, or
//! a board you own and dumped — at the path the other two already read:
//!
//! ```text
//! SFEMU_ROMS=/path/to/sf2.zip cargo test -p sfemu --test audio_boot -- --ignored
//! ```
//!
//! The mechanism is `boot.rs`'s, deliberately unchanged: one variable, one panic
//! message, no second escape hatch anywhere.
//!
//! # What this adds over `sound_boot.rs`
//!
//! `sound_boot.rs` asks whether the driver reaches the *YM2151*, and reports
//! `oki_writes` without asserting on it. This asks the ADPCM question, which is a
//! different one and has a specific trap in it: with no sample ROM every phrase-table
//! entry reads `start == stop == 0`, the chip refuses every command, and `oki_writes`
//! climbs anyway — a rising counter over a silent chip. So the count is a premise here
//! and the assertions are about the samples that left the mix.
//!
//! Everything about the OKI *core* is already covered by the 1,000-case vector suite,
//! which runs unconditionally and needs no ROM. What only a real ROM can show is that
//! the driver talks to the chip at all: that the phrase table is where the code expects
//! it, and that what reaches the mix is not silence.

/// SF2's driver programs the OKI, and the mix carries audio.
///
/// # What each assertion would catch
///
/// - `oki_writes > 0`: the driver wrote to the chip at all. **A floor, not a measured
///   figure** — `sound_boot.rs` says outright that nobody has measured what SF2's OKI
///   write count should be over two seconds, and asserting a guessed threshold would be
///   inventing the number this test exists to discover. What is certain is that a driver
///   which never touched the chip wrote zero times. When a run prints a real figure,
///   raise this to a floor derived from it and record the measurement beside it.
/// - a non-empty sample buffer: the scheduler produced samples at all, which is the
///   premise everything below reads.
/// - a quarter of the samples non-zero: the mix is not silence with a click in it. A
///   `.any(|&s| s != 0)` — which is what `sound_boot.rs` asks, correctly, of a different
///   question — passes on one non-zero sample in 110,000, and that is what a driver
///   whose key-on never lands produces.
/// - `peak > 1000`: and it is loud enough to be music rather than a DC offset or the
///   bottom bit of an envelope that never opened. 1,000 of 32,767 is about −30 dBFS.
#[test]
#[ignore = "needs a user-supplied ROM set; set SFEMU_ROMS"]
fn sf2_drives_the_oki_and_the_mix_is_audible() {
    let Ok(path) = std::env::var("SFEMU_ROMS") else {
        panic!("set SFEMU_ROMS to your own sf2.zip or a directory of loose files");
    };
    let set = romset::load(&romset::games::SF2, std::path::Path::new(&path))
        .unwrap_or_else(|e| panic!("cannot load {path}: {e}"));
    let prog = set
        .region("maincpu")
        .expect("the sf2 spec has a maincpu region");
    let gfx = set.region("gfx").expect("and a gfx region");
    let audiocpu = set.region("audiocpu").expect("and an audiocpu region");
    // The one region this test cannot do without: see the module note on why an empty
    // one produces a rising `oki_writes` over a chip that never plays a byte.
    let okirom = set.region("oki").expect("and an oki region");
    let mut m = machine::Cps1::with_sound(
        prog,
        gfx.to_vec(),
        audiocpu.to_vec(),
        okirom.to_vec(),
        machine::BoardConfig::sf2(),
        machine::Timing::cps1_10mhz(),
    );
    m.reset();

    // 120 frames is two seconds: long enough for the driver to initialise the chip and
    // start a track, short enough to stay a test. The samples are never drained, so the
    // buffer below holds the whole run — about 110,000 mono samples, 220 KB.
    for _ in 0..120 {
        m.run_frame();
    }
    let t = m.sound_trace();
    eprintln!("OKI writes in 120 frames: {}", t.oki_writes);
    assert!(t.oki_writes > 0, "the driver never wrote to the OKI at all");

    let chip = m.sound.oki_ref();
    eprintln!(
        "OKI status {:02X}, voices {:04b}, divisor {}",
        chip.status(),
        chip.voices_playing(),
        m.sound.oki_divisor()
    );

    let samples = m.samples();
    assert!(!samples.is_empty(), "no samples at all");
    let nonzero = samples.iter().filter(|&&s| s != 0).count();
    let peak = samples
        .iter()
        .map(|&s| i32::from(s).abs())
        .max()
        .unwrap_or(0);
    eprintln!("{nonzero}/{} non-zero, peak {peak}", samples.len());
    assert!(
        nonzero * 4 > samples.len(),
        "the mix is mostly silence: {nonzero} of {} samples carry anything",
        samples.len()
    );
    assert!(
        peak > 1000,
        "the mix peaks at {peak} — too quiet to be music"
    );
}
