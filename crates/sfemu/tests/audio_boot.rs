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
/// - `oki_writes > 20`: the driver played a phrase, rather than merely initialising
///   the chip. This was `> 0` and marked "a floor, not a measured figure" until a run
///   on the real set produced one: **91** writes over 1,200 frames, against **2** for
///   a machine that initialises the chip and plays nothing. The gap between those two
///   numbers is the whole assertion, and `> 0` sat on the wrong side of it.
/// - a non-empty sample buffer: the scheduler produced samples at all, which is the
///   premise everything below reads.
/// - a quarter of the samples non-zero: the mix is not silence with a click in it. A
///   `.any(|&s| s != 0)` passes on one non-zero sample in 450,000, and that is what a
///   driver whose key-on never lands produces. Measured: 99.97%.
/// - `peak > 1000`: and it is loud enough to be music rather than a DC offset or the
///   bottom bit of an envelope that never opened. 1,000 of 32,767 is about −30 dBFS;
///   measured 17,088.
#[test]
#[ignore = "needs a user-supplied ROM set; set SFEMU_ROMS"]
fn sf2_drives_the_oki_and_the_mix_is_audible() {
    let Ok(path) = std::env::var("SFEMU_ROMS") else {
        panic!("set SFEMU_ROMS to your own sf2.zip or a directory of loose files");
    };
    // `identify`, not `load(&games::SF2, ..)`: the user's revision is theirs, and
    // the CPS-B row differs between SF2 revisions. Under the wrong row the program
    // fails its ID check and parks in an idle loop — from which it never starts the
    // music, so every assertion here would fail with no hint as to why. See
    // `boot.rs` for the full account.
    let (spec, set) = romset::identify(std::path::Path::new(&path))
        .unwrap_or_else(|e| panic!("cannot load {path}: {e}"));
    eprintln!("identified {} at {path}", spec.name);
    let cfg = machine::BoardConfig::for_game(spec.name).unwrap_or_else(|| {
        panic!(
            "`{}` is not a CPS-1 game; point SFEMU_ROMS at an SF2 set",
            spec.name
        )
    });
    let prog = set
        .region("maincpu")
        .expect("every CPS-1 spec has a maincpu region");
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
        cfg,
        machine::Timing::cps1_10mhz(),
    );
    m.reset();

    // Demo sounds on: `Inputs::idle` leaves DSWC bit 0x20 set, which is Demo Sounds
    // *off*, and with it set this program plays nothing in attract mode at all. See
    // `sound_boot.rs`, which carries the same two lines and the full account.
    m.board.inputs.dsw[2] &= !0x20;

    // Warm up past the music onset at frame 916, draining so the buffer holds only
    // the window measured below. See `sound_boot.rs` for why this is not 120 frames.
    for _ in 0..960 {
        m.run_frame();
        let _ = m.drain_samples();
    }
    for _ in 0..240 {
        m.run_frame();
    }
    let t = m.sound_trace();
    eprintln!("OKI writes: {}", t.oki_writes);
    // Now a measured floor rather than the `> 0` this held before: 91 writes over
    // these 1,200 frames on the real set, with two voices seen playing during the
    // window. 20 is well under that and well over the 2 writes a run that only
    // initialises the chip produces.
    //
    // ⚠️ Do not reach for a coin instead of the DIP switch here. Inserting a coin
    // does produce a jingle — 157,410 non-zero samples, peak 7,033 — but with
    // **zero** OKI writes, so this assertion cannot pass that way. The ADPCM chip
    // carries the attract-mode demo's effects, not the coin sound.
    assert!(
        t.oki_writes > 20,
        "only {} OKI writes — the driver initialised the chip and never played a \
         phrase (measured 91; a chip merely initialised takes 2)",
        t.oki_writes
    );

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
        "the mix is mostly silence: {nonzero} of {} samples carry anything \
         (measured 450,044 of 450,164, 99.97%)",
        samples.len()
    );
    // 1,000 of 32,767 is about −30 dBFS. The measured peak is 17,088, so this stays
    // a floor on "audible at all" rather than a fingerprint of one mix — which is
    // what it should be, since the waveform itself is what the OKI and YM2151 vector
    // suites check, against ymfm, with no ROM involved.
    assert!(
        peak > 1000,
        "the mix peaks at {peak} — too quiet to be music (measured 17,088)"
    );
}
