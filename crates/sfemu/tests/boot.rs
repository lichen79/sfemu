//! The one test that needs a real ROM set, and the one `#[ignore]` in this
//! project.
//!
//! # Why this is `#[ignore]`d when the rest of the project forbids it
//!
//! The project rule is that missing test data **fails loudly**, naming the file
//! and the command that fetches it — no environment-variable escape hatch. That
//! rule exists because sub-project A's test data is legally fetchable and there
//! *is* a command to name.
//!
//! This data is not. SF2 is commercial Capcom code; there is no command we may put
//! in a failure message. A test that hard-fails on a machine which legally cannot
//! hold the file is a broken test, not a strict one. So it skips by default, and
//! CI's not running it is honest rather than hidden.
//!
//! ```text
//! SFEMU_ROMS=/path/to/sf2.zip cargo test -p sfemu --test boot -- --ignored
//! ```
//!
//! # Why this lives in `sfemu` and not `machine`
//!
//! It needs both `romset` (to load) and `machine` (to run), and `machine` must
//! never depend on `romset` — that would drag `miniz_oxide` and `std` into the
//! path sub-project A kept clean. `sfemu` is the crate that already joins them.

/// SF2 boots, services its vblank, programs the video hardware, and stays inside
/// the map for sixty frames.
///
/// # What each assertion would catch
///
/// - `vblanks == 60`, `acks >= 60`: the interrupt is asserted *and* serviced. A
///   game that never lowers its mask leaves `acks` at 0 while every cycle-counting
///   test in the project stays green.
/// - `cps_a_writes > 0`: the boot code reached the point of programming the video
///   registers, which is past the self-test and past RAM clearing.
/// - `gfxram_writes > 100_000`: and is *still* writing tile and palette data long
///   after the boot code's RAM clear. This is the assertion that catches a program
///   parked in a self-test failure loop, which every other assertion here passes;
///   see the note at the assertion itself for the two measured figures.
/// - `!halted`: no double bus fault. That is what a wrong memory map produces.
/// - the PC range: the program is somewhere it could legitimately be. A boot that
///   vectors into an all-0xFFFF region loops in an exception handler with every
///   counter above still plausible.
#[test]
#[ignore = "needs a user-supplied ROM set; set SFEMU_ROMS"]
fn sf2_boots_for_sixty_frames_without_wandering_off_the_map() {
    let Ok(path) = std::env::var("SFEMU_ROMS") else {
        panic!("set SFEMU_ROMS to your own sf2.zip or a directory of loose files");
    };
    // `identify`, not `load(&games::SF2, ..)`: which SF2 revision a user has is
    // theirs to have, and the CPS-B row differs between them. A test that assumed
    // rev G would, on any other revision, watch the program fail its ID check and
    // park in an idle loop — with `vblanks`, `acks` and `!halted` all still
    // satisfied, so every assertion below would pass on a machine drawing nothing.
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
    let mut m = machine::Cps1::new(prog, cfg, machine::Timing::cps1_10mhz());
    m.reset();
    // 4096 samples over 15,720 scanlines: enough to catch a program that spends
    // any sustained time off the map, and bounded.
    m.board.trace.pc_sample_cap = 4096;
    for _ in 0..60 {
        m.run_frame();
    }
    let t = &m.board.trace;
    assert_eq!(t.frames, 60);
    assert_eq!(t.vblanks, 60, "one vblank per frame");
    assert!(
        t.acks >= 60,
        "every vblank must be acknowledged: {} acks for {} vblanks",
        t.acks,
        t.vblanks
    );
    assert!(
        t.cps_a_writes > 0,
        "the game must program the video registers"
    );
    assert!(!m.cpu.halted, "a double bus fault means the map is wrong");

    // gfxram is in the allowed range because `cps1.cpp:592` records that SF2CE
    // executes code from there — a near neighbour of this set, so excluding it
    // would be wrong.
    for &pc in &t.pc_samples {
        assert!(
            pc < 0x10_0000 || (0x90_0000..=0x92_FFFF).contains(&pc) || pc >= 0xFF_0000,
            "PC {pc:#08x} is outside populated ROM, gfxram, or RAM — the program \
             has jumped somewhere the map does not answer"
        );
    }
    assert_eq!(
        t.pc_samples.len(),
        4096,
        "the sampler must have filled: 60 frames is 15,720 scanlines"
    );

    // And it is not parked after a failed self-test.
    //
    // ⚠️ Every assertion above was satisfied by a real run that drew nothing for
    // 1,200 frames. `sf2eb`'s program under rev G's CPS-B row fails its ID check at
    // 0x0004c2 and branches to `bra $6FC`, a loop on itself. Vblanks are asserted
    // and acknowledged from the interrupt handler, the video registers are
    // programmed *before* the check, and a deliberate loop is not a halt — so
    // `vblanks`, `acks`, `cps_a_writes`, `gfxram_writes > 0` and `!halted` all held.
    //
    // The distinct-PC count does **not** separate the two, which is worth recording
    // because it is the obvious thing to reach for and it fails: measured over these
    // same 60 frames, the parked run visits 26 distinct sampled addresses and the
    // working run 30. The parked program still runs its whole interrupt handler
    // every frame; it is only the main loop that is stuck.
    //
    // What separates them by an order of magnitude is how much tile and palette data
    // reaches the video RAM: 33,846 writes parked against 361,303 working, both
    // measured on the real set at 60 frames. The parked run's 33,846 are the boot
    // code's RAM clear, which happens before the check. 100,000 is between the two
    // with a wide margin either side, and it is a floor on *drawing something*,
    // which is the thing this test could not otherwise see.
    assert!(
        t.gfxram_writes > 100_000,
        "only {} gfxram writes in 60 frames — the boot code's clear is about 34,000 \
         and a game drawing is about 360,000, so this is a program that initialised \
         the hardware and then stopped. A self-test failure that branches to itself \
         satisfies every other assertion in this test",
        t.gfxram_writes
    );

    // Not an assertion — a report, so a failing run above prints the thing that
    // names the missing chip.
    println!("unmapped writes: {:?}", t.unmapped_writes.worst(8));
    println!("unmapped reads:  {:?}", t.unmapped_reads.worst(8));
}
