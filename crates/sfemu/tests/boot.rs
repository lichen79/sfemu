//! The tests that need a real ROM set, all `#[ignore]`d.
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
//! # One variable, and each test skips the sets that are not its own
//!
//! `SFEMU_ROMS` names whatever set the user has, and `romset::identify` says which
//! game it is. A second variable per game — `SFEMU_ROMS_SF1`, `SFEMU_ROMS_SF2CE` —
//! is deliberately not a thing here: it multiplies as games are added and leaves
//! every test silently unrun when a name is misspelled.
//!
//! The cost is that each test must decide whether the set it was handed is one it
//! can speak for, and skip with a reason when it is not. Both tests below do, and
//! the reason matters in both directions: the frame counts and thresholds in each
//! are measurements of one specific program, not properties of CPS-1.
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
    if spec.name == "sf2ce" {
        // Champion Edition is a different program, and every frame count below is
        // World Warrior's. CE spends its first ~111 frames in a boot memory test
        // with the vblank masked, so `acks >= 60` at 60 frames is false for a
        // perfectly working run — this test fails on it, and did.
        //
        // Skipped rather than generalised. Loosening these constants to span both
        // programs would cost the sf2eb discriminator they exist for: `acks >= 60`
        // and `gfxram_writes > 100_000` are what catch a World Warrior set parked in
        // a self-test failure. CE gets its own test, with its own measured numbers.
        eprintln!("skipping: this test's frame counts are World Warrior's; see the CE test");
        return;
    }
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

/// Champion Edition boots, finishes its memory test, and draws its attract mode
/// **with its own CPS-B row and not another's**.
///
/// ```text
/// SFEMU_ROMS=/path/to/sf2ce.zip cargo test -p sfemu --test boot -- --ignored
/// ```
///
/// # Why this is a separate test and not the one above with a bigger frame count
///
/// Every number in `sf2_boots_for_sixty_frames_without_wandering_off_the_map` is
/// World Warrior's, and two of them are false for CE:
///
/// - **`acks == 60` at 60 frames.** CE's first acknowledged interrupt is at frame
///   **111**. Before that it is in a boot memory test — a write / read-back /
///   restore loop at 0x0006f2 over the pattern table at 0x00071e
///   (`0000/5555/aaaa/ffff`) — with the interrupt mask at 6, which masks the
///   level-2 vblank. Measured: 0 acks at 60 frames, 10 at 120, 490 at 600. So the
///   run has to be long enough to get past it, and `acks == frames` is never true
///   for this set.
/// - **`gfxram_writes > 100_000` separates a wrong row.** For `sf2eb` it does:
///   33,846 parked against 920,306 running. For CE it cannot, because **CE never
///   reads its ID register at all** — 0 long operands equal to 0x800172 in the whole
///   program — so a wrong row does not park it. Measured at 1200 frames, CE writes
///   the identical 966,668 words under all three rows.
///
/// # What does separate CE's row
///
/// The rendered palette. CE's row and `sf2`'s share every CPS-B register *address*
/// and no layer-enable *bit* — the inverse of the `sf2`/`sf2eb` difference — so a
/// CE board given `sf2`'s registers reads valid registers and tests the wrong bits.
/// The scroll-2 background is disabled and the sprites and crowd still draw. That is
/// a screen that looks like a working emulator with one layer missing, not a black
/// one, which is why the counters above cannot see it.
///
/// So this test runs both rows and compares. Measured distinct pens in the composed
/// frame, own row against `sf2`'s: 184/123 at 900, 1000 and 1100 frames, 172/111 at
/// 1200. The **ratio** is what is asserted rather than either number: the exact
/// count tracks whichever attract-mode scene the frame lands in, and at 1800 frames
/// the two rows happen to agree at 32 apiece — so a threshold on the absolute count
/// would be vacuous at some frame counts and brittle at all of them.
#[test]
#[ignore = "needs a user-supplied ROM set; set SFEMU_ROMS"]
fn sf2ce_draws_its_attract_mode_and_only_under_its_own_cps_b_row() {
    let Ok(path) = std::env::var("SFEMU_ROMS") else {
        panic!("set SFEMU_ROMS to your own sf2ce.zip or a directory of loose files");
    };
    let (spec, set) = romset::identify(std::path::Path::new(&path))
        .unwrap_or_else(|e| panic!("cannot load {path}: {e}"));
    if spec.name != "sf2ce" {
        // Skipped rather than failed. `SFEMU_ROMS` is one variable for every gated
        // test in this project by design — a second variable per game is exactly
        // what this project does not do — so a user running the whole `--ignored`
        // set with a World Warrior zip should see this pass by, not fail.
        eprintln!("skipping: SFEMU_ROMS is `{}`, not sf2ce", spec.name);
        return;
    }

    // 1100 frames: past the memory test with a wide margin, and on the plateau where
    // the own-row and wrong-row pen counts were measured 184 against 123.
    const FRAMES: u32 = 1100;
    let prog = set.region("maincpu").expect("sf2ce has a maincpu region");
    let gfx = set.region("gfx").expect("sf2ce has a gfx region");

    // Both rows, on the same files. `gfx` is cloned per machine because `with_gfx`
    // takes it by value.
    let run = |cfg: machine::BoardConfig| {
        let mut m = machine::Cps1::with_gfx(prog, gfx.to_vec(), cfg, machine::Timing::cps1_10mhz());
        m.reset();
        m.board.trace.pc_sample_cap = 4096;
        for _ in 0..FRAMES {
            m.run_frame();
        }
        m.render();
        m
    };
    let own = run(machine::BoardConfig::sf2ce());
    let wrong = run(machine::BoardConfig::sf2());

    let pens = |m: &machine::Cps1| {
        let mut v: Vec<u16> = m.video.fb.pens.to_vec();
        v.sort_unstable();
        v.dedup();
        v.len()
    };
    let (a, b) = (pens(&own), pens(&wrong));
    println!("distinct pens: own row {a}, sf2's row {b}");

    // The machine is alive at all, on the same terms as the test above.
    let t = &own.board.trace;
    println!(
        "acks {}, gfxram writes {}, cps-a writes {}",
        t.acks, t.gfxram_writes, t.cps_a_writes
    );
    assert_eq!(t.frames, u64::from(FRAMES));
    assert_eq!(t.vblanks, u64::from(FRAMES), "one vblank per frame");
    assert!(!own.cpu.halted, "a double bus fault means the map is wrong");
    // 1100 - 111 = 989, and 990 is what a real run gives: every frame after the
    // memory test is serviced. 900 is that with slack for the exact frame the mask
    // drops, and it is far above the 0 a run still parked in the test would give.
    assert!(
        t.acks > 900,
        "only {} acknowledged interrupts in {FRAMES} frames — the memory test ends \
         around frame 111 and every frame after it is serviced, so a figure near \
         zero is a program still parked in it",
        t.acks
    );
    // Not a discriminator between rows — see the doc — but still a floor on *any*
    // program getting past boot. Measured 883,876 here; the boot clear alone is
    // about 34,000.
    assert!(
        t.gfxram_writes > 700_000,
        "only {} gfxram writes in {FRAMES} frames — measured 883,876, against about \
         34,000 for the boot code's clear on its own",
        t.gfxram_writes
    );
    for &pc in &t.pc_samples {
        assert!(
            pc < 0x10_0000 || (0x90_0000..=0x92_FFFF).contains(&pc) || pc >= 0xFF_0000,
            "PC {pc:#08x} is outside populated ROM, gfxram, or RAM"
        );
    }
    // Without this the loop above is vacuous: an empty sampler passes it.
    assert_eq!(
        t.pc_samples.len(),
        4096,
        "the sampler must have filled: {FRAMES} frames is over 280,000 scanlines"
    );

    // And it is drawing something substantial, so the comparison below is between
    // two rendered screens rather than two nearly-empty ones.
    assert!(
        a > 100,
        "only {a} distinct pens in the composed frame — measured 184 at {FRAMES} \
         frames. A CE that reached its attract mode draws a stage, a crowd and two \
         fighters"
    );

    // The row is what produced them. `sf2`'s registers disable CE's scroll-2 layer,
    // which is the background: the sprites and the crowd still draw, so this is a
    // frame that looks plausible and is missing a layer.
    //
    // ⚠️ A ratio and not a threshold on `b`. Measured 184/123 at 900-1100 frames and
    // 172/111 at 1200 — but at 1800 both rows give 32, because the attract mode has
    // moved to a scene whose background the wrong row happens not to suppress. An
    // `assert!(b < 130)` would pass there while comparing nothing.
    assert!(
        b < a,
        "the wrong CPS-B row rendered {b} pens against {a} — a CE board given sf2's \
         row reads valid registers and tests the wrong layer-enable bits, so it must \
         draw strictly less"
    );
    assert!(
        a * 100 / b.max(1) >= 130,
        "own row {a} pens against sf2's {b} — measured 184 against 123, a ratio of \
         1.49. Anything under 1.3 means the layer the wrong row disables is barely \
         contributing to this frame, and the comparison is not testing the row"
    );

    println!("unmapped writes: {:?}", t.unmapped_writes.worst(8));
    println!("unmapped reads:  {:?}", t.unmapped_reads.worst(8));
}
