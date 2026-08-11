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

/// SF2's sound program initialises the chip and plays, over two seconds.
///
/// # What each assertion would catch
///
/// - `audiocpu_fetches > 100_000`: the Z80 executed from the sound ROM. The number is
///   deliberately large, and it is why `SoundBoard` counts only bytes the ROM
///   *answered*: an absent sound region reads 0xFF, which is `RST 38h`, so a machine
///   built without `audiocpu` spins in a tight loop and would clear a naive threshold
///   while running no driver at all.
/// - `latch_reads > 0`: it read the command latch, so the two CPUs are connected.
/// - `ym_writes > 100`: and programmed the FM chip. Initialising one voice is dozens
///   of registers, so a driver that got as far as the chip clears this easily and one
///   that wrote a handful of bytes and gave up does not.
/// - a non-silent sample: the chip's output actually left the chip. Every counter
///   above can be satisfied by a driver writing registers that produce silence — a
///   key-on that never lands, an envelope that never opens.
/// - `port_accesses == 0`: the board has no I/O ports. `sub_map` (`cps1.cpp:631-642`)
///   is program space only, so **a non-zero count here is a finding to report, not a
///   test to relax** — it would mean the map is not the whole story.
#[test]
#[ignore = "needs a user-supplied ROM set; set SFEMU_ROMS"]
fn the_sound_program_drives_the_ym2151() {
    let Ok(path) = std::env::var("SFEMU_ROMS") else {
        panic!("set SFEMU_ROMS to your own sf2.zip or a directory of loose files");
    };
    let set = romset::load(&romset::games::SF2, std::path::Path::new(&path))
        .unwrap_or_else(|e| panic!("cannot load {path}: {e}"));
    let prog = set
        .region("maincpu")
        .expect("the sf2 spec has a maincpu region");
    let gfx = set.region("gfx").expect("and a gfx region");
    // `with_sound`, not `with_gfx`: the sound region is the whole point here, and
    // `with_gfx` hands the board an empty one. That is a real hazard rather than a
    // theoretical one — `sfemu`'s own main loop still builds with `with_gfx`, so a
    // test copied from it would assert a fetch count produced entirely by `RST 38h`.
    let audiocpu = set.region("audiocpu").expect("and an audiocpu region");
    let mut m = machine::Cps1::with_sound(
        prog,
        gfx.to_vec(),
        audiocpu.to_vec(),
        machine::BoardConfig::sf2(),
        machine::Timing::cps1_10mhz(),
    );
    m.reset();

    // A bounded run: 120 frames is two seconds, long enough for the driver to
    // initialise the chip and start a track, short enough to stay a unit test. The
    // samples are never drained, so `samples()` below holds the whole run — about
    // 110,000 stereo pairs, which is 440 KB and fine for a test.
    for _ in 0..120 {
        m.run_frame();
    }
    let t = m.sound_trace();

    assert!(
        t.audiocpu_fetches > 100_000,
        "the Z80 ran: {}",
        t.audiocpu_fetches
    );
    assert!(t.latch_reads > 0, "it read the command latch");
    assert!(
        t.ym_writes > 100,
        "and wrote YM2151 registers: {}",
        t.ym_writes
    );
    assert!(
        m.samples().iter().any(|&(l, r)| l != 0 || r != 0),
        "and the chip produced non-silent samples out of {}",
        m.samples().len()
    );

    assert_eq!(t.port_accesses, 0, "the driver used an I/O port after all");

    // Not assertions — reports, so a failing run above prints the numbers that say
    // how far the driver got. The OKI count in particular is D3's evidence: nobody has
    // measured what it should be yet, so asserting a value would be inventing one.
    eprintln!("OKI writes in 120 frames: {}", t.oki_writes);
    eprintln!("samples: {}", m.samples().len());
    eprintln!("Z80 T-states: {}", m.z80_cycles());
    eprintln!("YM register writes: {}", t.ym_writes);
    eprintln!("latch reads: {}", t.latch_reads);
}
