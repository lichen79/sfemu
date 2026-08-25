//! Does SF1 boot on its own board?
//!
//! # Why this is `#[ignore]`d when the rest of the project forbids it
//!
//! The fourth such test, for `boot.rs`'s reason exactly: the project rule is that
//! missing test data **fails loudly**, naming the file and the command that fetches
//! it, and that rule holds because the rest of this project's test data is legally
//! fetchable and there *is* a command to name. This data is not. Street Fighter is
//! commercial Capcom code; there is no command we may put in a failure message.
//!
//! Supply a legally obtained set — Capcom Arcade Stadium, Capcom Fighting
//! Collection, or a board you own and dumped — at the path the other five read:
//!
//! ```text
//! SFEMU_ROMS=/path/to/sf.zip cargo test -p sfemu --test sf1_boot -- --ignored
//! ```
//!
//! ⚠️ **`SFEMU_ROMS` holds one path, and the six gated tests do not all want the
//! same one.** The three CPS-1 tests want an `sf2` set and these three want an `sf`
//! set, so a user who has both runs the suite twice with the variable pointing at
//! each in turn. That is deliberate: a second variable — `SFEMU_ROMS_SF1` — is the
//! second escape hatch this project's rule forbids by name, and the failure when the
//! variable points at the wrong set is a `romset::load` error naming the file it
//! could not find, which is loud.
//!
//! # ⚠️ These three have never been executed
//!
//! No SF1 set has been available to this project, so the three `sf1_*` gated tests
//! have never run against a ROM — while the three CPS-1 ones now have. **That run
//! falsified a premise in every one of them**, and these three still carry the same
//! premises, so expect the first real SF1 run to fail and read the failures as
//! findings about the tests before suspecting the drivers:
//!
//! - **`> 0` floors are usually on the wrong side of the gap.** `audio_boot.rs`'s
//!   `oki_writes > 0` passed on a machine that initialised the chip and played
//!   nothing: initialisation alone is 2 writes and playing is 91. Every `> 0` here
//!   — `latch_reads`, `bank_writes`, `bank_fetches`, `nibbles` — is the same shape.
//! - **120 frames from reset is before the music starts.** SF2's first non-zero
//!   sample arrives at frame 916, after the self-test, the RAM clear, the logo and
//!   the title screen. Both SF1 sound tests run 120 frames and assert on the result.
//! - **Attract-mode silence may be a DIP switch, not a fault.** SF2's Demo Sounds is
//!   DSWC bit 0x20 and the bit means *off*; `Inputs::idle` sets every switch off, so
//!   the default configuration correctly plays nothing at all. Check SF1's
//!   `INPUT_PORTS` for its equivalent before concluding the driver is silent.
//! - **A counter that rises is not a chip that plays.** Measure, then set a floor
//!   between the measured working figure and the measured broken one, and quote both
//!   at the assertion — see `sound_boot.rs` and `audio_boot.rs` for the shape.
//!
//! # Why this is not `boot.rs` with two names changed
//!
//! `boot.rs`'s two video assertions are `cps_a_writes > 0` and `gfxram_writes > 0`,
//! and neither can hold here: those are fields of the shared [`machine::Trace`] that
//! `Sf1Board` never writes, so both are structurally zero on this board. Copying them
//! would produce a test that fails on a working driver. SF1 keeps the same evidence
//! somewhere else — its palette and its text layer are plain RAM the video reads
//! directly — and that is what this file asserts on instead.
//!
//! The loading block below is repeated in `sf1_sound_boot.rs` and
//! `sf1_audio_boot.rs`, as `boot.rs`'s is in `sound_boot.rs` and `audio_boot.rs`.
//! That is a ruling rather than an oversight: a `tests/common/mod.rs` extracted for
//! these three would leave the crate with two conventions for the same fourteen
//! lines, and extracting all six means editing three tests nobody in CI can run.
//! Each gated test stays readable end to end by the one person who can run it.

// An integration test is its own crate root, so the crate's `lib.rs` attribute
// does not reach here.
#![forbid(unsafe_code)]

/// SF1 boots, services its vblank, fills its palette and text layer, and stays
/// inside the map for sixty frames.
///
/// # What each assertion would catch
///
/// - `frames == 60`, `vblanks == 60`: the scanline counter wrapped when it should
///   and the beam reached vblank once per frame.
/// - `acks >= 60`: the interrupt was asserted *and* acknowledged. This is the
///   assertion that catches the vector number being wrong — SF1 autovectors level 1
///   at **0x64**, not CPS-1's 0x68, and a board watching the wrong longword never
///   sees the acknowledge, so the interrupt is never released and the game runs one
///   frame and stops. Every cycle-counting test in `machine` stays green through
///   that.
/// - `!halted`: no double bus fault, which is what a wrong memory map produces.
/// - a non-zero palette word: the boot code got as far as writing colours. This is
///   SF1's `cps_a_writes` — the palette is 1,024 words of plain RAM at 0xB00000 that
///   the video reads directly, so there is no register write to count and the RAM's
///   contents are the evidence. `Sf1Board::new` zeroes it and `reset` does not clear
///   it, so a non-zero word can only be the guest's.
/// - a non-zero videoram word: and wrote text-layer tiles. SF1's `gfxram_writes`,
///   for the same reason: 2,048 words at 0x800000, exactly `tilemap::TX.tiles()`.
/// - the three CPS-1 counters at zero: they are fields of the shared `Trace` that
///   this board does not have the chips for, and `main.rs`'s report omits their
///   lines on SF1 on exactly that ground. If this ever fires, the omission is hiding
///   a real number and the report is wrong, not this test.
/// - the PC range: the program is somewhere it could legitimately be. Two arms, not
///   `boot.rs`'s three — SF1 has no executable graphics RAM and nothing decodes
///   between 0x050000 and 0x7FFFFF, so a boot that vectors into the gap loops in an
///   exception handler with every counter above still plausible.
/// - `pc_samples.len() == 4096`: the sampler filled, so the range above was checked
///   against a full run rather than a handful of lines.
#[test]
#[ignore = "needs a user-supplied ROM set; set SFEMU_ROMS"]
fn sf1_boots_for_sixty_frames_without_wandering_off_the_map() {
    let Ok(path) = std::env::var("SFEMU_ROMS") else {
        panic!("set SFEMU_ROMS to your own sf.zip or a directory of loose files");
    };
    let set = romset::load(&romset::games::SF1, std::path::Path::new(&path))
        .unwrap_or_else(|e| panic!("cannot load {path}: {e}"));
    // Every region, not just `maincpu`: `romset::load` has already read and CRC-checked
    // the whole set, so handing the machine less than all of it would only invent a
    // second way for the boot to stall.
    let need = |name: &str| -> Vec<u8> {
        set.region(name)
            .unwrap_or_else(|| panic!("the sf1 spec has a `{name}` region"))
            .to_vec()
    };
    let video = machine::video::sf1::Sf1Video::new(
        need("gfx1"),
        need("gfx2"),
        need("gfx3"),
        need("gfx4"),
        need("tilerom"),
    );
    let mut m = machine::Sf1::new(&need("maincpu"), video, need("audiocpu"), need("audio2"));
    m.reset();
    // 4,096 samples over 15,360 scanlines: enough to catch a program that spends any
    // sustained time off the map, and bounded.
    m.board.trace.pc_sample_cap = 4096;
    for _ in 0..60 {
        m.run_frame();
    }
    let t = &m.board.trace;
    assert_eq!(t.frames, 60);
    assert_eq!(t.vblanks, 60, "one vblank per frame");
    assert!(
        t.acks >= 60,
        "every vblank must be acknowledged: {} acks for {} vblanks — the level-1 \
         autovector is at 0x64 on this board, not CPS-1's 0x68",
        t.acks,
        t.vblanks
    );
    assert!(!m.cpu.halted, "a double bus fault means the map is wrong");
    assert!(
        m.board.palette.iter().any(|&w| w != 0),
        "the game must write colours: all 1024 palette words are still zero"
    );
    assert!(
        m.board.videoram.iter().any(|&w| w != 0),
        "and text-layer tiles: all 2048 videoram words are still zero"
    );
    assert_eq!(
        (t.cps_a_writes, t.cps_b_writes, t.gfxram_writes),
        (0, 0, 0),
        "this board has no CPS-A, no CPS-B and no gfxram, and `report` omits all \
         three lines on that ground — a non-zero count means the omission is hiding \
         something"
    );

    for &pc in &t.pc_samples {
        // `!(0x05_0000..0xFF_8000).contains(&pc)` and not `pc < 0x05_0000 || pc >=
        // 0xFF_8000`: the two are the same predicate and clippy's
        // `manual_range_contains` rejects the second under `-D warnings`. `boot.rs`
        // keeps the comparison form because its middle arm makes three, which clippy
        // does not flag; SF1's map has only the gap.
        //
        // ⚠️ The bound is `0x05_0000`, not `0x05_0004`. The core's `pc` runs two
        // prefetched words ahead, so code in the last two words of the program region
        // would sample a PC just past it — but no code lives there, the region's top
        // is padding in every SF1 set. If this fires with a PC of `0x0500_0x`, that is
        // a finding to report, **not** a bound to widen by four: widening hides the
        // case it was written for, a `jmp` through a corrupted pointer into the
        // 0x050000-0x7FFFFF gap.
        assert!(
            !(0x05_0000..0xFF_8000).contains(&pc),
            "PC {pc:#08x} is outside the 320 KB program region and outside RAM — the \
             program has jumped somewhere the map does not answer"
        );
    }
    assert_eq!(
        t.pc_samples.len(),
        4096,
        "the sampler must have filled: 60 frames is 15,360 scanlines"
    );

    // Not assertions — reports, so a failing run above prints the things that name
    // what went wrong. `active`, the two scrolls and the coin latch are the four
    // scalars `gfxctrl_w`, `bg_scroll_w`, `fg_scroll_w` and `coin_w` own, and all four
    // being zero after sixty frames is the signature of a boot that never reached its
    // display setup.
    println!("unmapped writes: {:?}", t.unmapped_writes.worst(8));
    println!("unmapped reads:  {:?}", t.unmapped_reads.worst(8));
    println!(
        "active {:#04X}  bgscroll {:#06X}  fgscroll {:#06X}  coin {:#04X}",
        m.board.active, m.board.bgscroll, m.board.fgscroll, m.board.coin_ctrl
    );
    println!("sound commands posted: {}", t.sound_latch_writes);
    println!("writes into ROM: {}", t.rom_writes);
}
