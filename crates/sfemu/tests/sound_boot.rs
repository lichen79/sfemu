//! SF2's sound program runs, and reaches the YM2151.
//!
//! # Why this is `#[ignore]`d when the rest of the project forbids it
//!
//! The second and last test in this repository permitted to skip, for exactly the
//! reason `boot.rs` gives: the project rule is that missing test data **fails
//! loudly**, naming the file and the command that fetches it, and that rule exists
//! because the rest of this project's test data is legally fetchable and there *is* a
//! command to name. This data is not. SF2 is commercial Capcom code; there is no
//! command we may put in a failure message, and a test that hard-fails on a machine
//! which legally cannot hold the file is a broken test rather than a strict one.
//!
//! Supply a legally obtained set — Capcom Arcade Stadium, Capcom Fighting
//! Collection, or a board you own and dumped — at the path `boot.rs` already reads:
//!
//! ```text
//! SFEMU_ROMS=/path/to/sf2.zip cargo test -p sfemu --test sound_boot -- --ignored
//! ```
//!
//! The mechanism is `boot.rs`'s, deliberately unchanged: one variable, one panic
//! message, no second escape hatch anywhere.
//!
//! # Why the assertions are counters and not an audio hash
//!
//! The question here is whether the real driver *executes and reaches the chip*, not
//! whether it produces one particular waveform. `crates/ym2151` already checks the
//! waveform against ymfm, 1000/1000 vectors; a hash of this machine's output would be
//! a number nothing independent verifies, and it would change every time the
//! scheduler's interleave moved by a T-state without anything being wrong.

// An integration test is its own crate root, so the crate's `lib.rs` attribute
// does not reach here.
#![forbid(unsafe_code)]

/// SF2's sound program initialises the chip and plays its attract-mode music.
///
/// # Why the run is twenty seconds and not two
///
/// The attract music starts at **frame 916** on the real set — the boot self-test,
/// the RAM clear, the Capcom logo and the title screen all come first. A
/// 120-frame run from reset observes silence, and every floor here was originally
/// written against that run without ever having been executed against a ROM.
///
/// It also needs Demo Sounds *on*, which is not the default: see the assignment to
/// `dsw[2]` in the body.
///
/// # What each assertion would catch
///
/// Every figure below is from a run on the real set, quoted at the assertion. The
/// run is deterministic — two passes gave bit-identical counters — so each is a
/// floor under a known value rather than a guess.
///
/// - `audiocpu_fetches > 1_000_000`: the Z80 executed from the sound ROM. It is why
///   `SoundBoard` counts only bytes the ROM *answered*: an absent sound region reads
///   0xFF, which is `RST 38h`, so a machine built without `audiocpu` spins in a
///   tight loop and would clear a naive threshold while running no driver at all.
/// - `latch_reads > 100`: it read the command latch, so the two CPUs are connected.
/// - `ym_writes > 1_000`: and programmed the FM chip. Initialising one voice is
///   dozens of registers, so a driver that reached the chip clears this easily and
///   one that wrote a handful of bytes and gave up does not.
/// - half the samples non-zero: the chip's output actually left the chip. Every
///   counter above can be satisfied by a driver writing registers that produce
///   silence — a key-on that never lands, an envelope that never opens. A bare
///   `.any(|&s| s != 0)` is what this test asked before, and one non-zero sample in
///   450,000 passes it.
/// - `port_accesses == 0`: the board has no I/O ports. `sub_map` (`cps1.cpp:631-642`)
///   is program space only, so **a non-zero count here is a finding to report, not a
///   test to relax** — it would mean the map is not the whole story.
#[test]
#[ignore = "needs a user-supplied ROM set; set SFEMU_ROMS"]
fn the_sound_program_drives_the_ym2151() {
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
    // `with_sound`, not `with_gfx`: the sound region is the whole point here, and
    // `with_gfx` hands the board an empty one — a test built that way would assert a
    // fetch count produced entirely by `RST 38h`. `sfemu`'s own `main` uses
    // `with_sound` too, and has since sound landed; `with_gfx` survives there only in
    // a `#[cfg(test)]` fixture that renders and needs no audio.
    let audiocpu = set.region("audiocpu").expect("and an audiocpu region");
    // The ADPCM samples too, for the same reason: an absent sample ROM starts no
    // voice, so a driver that plays effects perfectly would still produce a mix with
    // the OKI term at zero and nothing here would say so.
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

    // Demo sounds on. `Inputs::idle` gives `dsw: [0xFF; 3]` — every switch off —
    // which is right as a default but leaves DSWC bit 0x20 set, and that bit is
    // Demo Sounds *off* (`cps1.cpp`, sf2's `INPUT_PORTS`, `SW(C):6`; MAME's own
    // default for it is 0x00, on). With the bit set this program plays nothing at
    // all in attract mode, which is correct hardware behaviour and would read here
    // as a driver that never reached the chip.
    m.board.inputs.dsw[2] &= !0x20;

    // The attract music does not start at reset: measured on the real set, the
    // first non-zero sample arrives at **frame 916**, after the boot self-test,
    // the RAM clear, the Capcom logo and the title screen. Two seconds from reset
    // — what this test used to run — is silence, and asserting on it asserted the
    // wrong thing.
    //
    // So: warm up past the onset, draining as we go so the buffer holds only the
    // window we measure, then keep 240 frames of it.
    for _ in 0..960 {
        m.run_frame();
        let _ = m.drain_samples();
    }
    for _ in 0..240 {
        m.run_frame();
    }
    let t = m.sound_trace();

    // Every floor below is derived from a run on the real set, printed beside it.
    // The run is deterministic — two passes gave bit-identical figures — so these
    // are floors under a known value rather than guesses.
    assert!(
        t.audiocpu_fetches > 1_000_000,
        "the Z80 ran: {} fetches (measured 14,942,240 over these 1,200 frames)",
        t.audiocpu_fetches
    );
    assert!(
        t.latch_reads > 100,
        "it read the command latch: {} (measured 5,022)",
        t.latch_reads
    );
    assert!(
        t.ym_writes > 1_000,
        "and wrote YM2151 registers: {} (measured 33,656)",
        t.ym_writes
    );

    // Not `.any(|&s| s != 0)`, which one non-zero sample in 450,000 satisfies —
    // and that is exactly what a driver whose key-on never lands produces. In the
    // measured window 450,044 of 450,164 samples carry signal, 99.97%, because a
    // music track is playing continuously by this point. Half is a wide margin
    // under that and still far above a click.
    let samples = m.samples();
    let nonzero = samples.iter().filter(|&&s| s != 0).count();
    assert!(
        nonzero * 2 > samples.len(),
        "the mix is mostly silence: {nonzero} of {} samples carry anything \
         (measured 450,044 of 450,164)",
        samples.len()
    );

    assert_eq!(t.port_accesses, 0, "the driver used an I/O port after all");

    // Not assertions — reports, so a failing run above prints the numbers that say
    // how far the driver got. `audio_boot.rs` is where the OKI count is asserted.
    eprintln!("OKI writes: {}", t.oki_writes);
    eprintln!("samples in the window: {}", samples.len());
    eprintln!("Z80 T-states: {}", m.z80_cycles());
    eprintln!("YM register writes: {}", t.ym_writes);
    eprintln!("latch reads: {}", t.latch_reads);
}
