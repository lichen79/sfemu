//! SF1's FM sound program runs, and reaches the YM2151.
//!
//! # Why this is `#[ignore]`d when the rest of the project forbids it
//!
//! The fifth such test, for `sf1_boot.rs`'s reason, which is `boot.rs`'s. Supply a
//! legally obtained set at the same path:
//!
//! ```text
//! SFEMU_ROMS=/path/to/sf.zip cargo test -p sfemu --test sf1_sound_boot -- --ignored
//! ```
//!
//! ⚠️ That variable holds one path and the six gated tests want two different sets —
//! `sf1_boot.rs`'s module doc says why there is no second variable.
//!
//! # Why this is not `sound_boot.rs` with two names changed
//!
//! The counters live somewhere else and one of them is gone. CPS-1 keeps eight
//! numbers on one `SoundTrace`; SF1 has **two** sound boards, so the FM board's five
//! are on [`machine::sf1::sound::FmTrace`] behind `m.fm.trace()` and the ADPCM
//! board's eight are on `m.adpcm.trace()` — Step 43's file. `oki_writes` and
//! `oki_clamps` have no counterpart at all: that chip is not on this board.
//!
//! The samples differ too. `machine::Cps1::samples()` is mono; `machine::Sf1::samples()`
//! is **interleaved stereo**, because SF1 has two speakers and `sf1::mix` pans the FM
//! chip's two outputs across them. So `samples().len()` is twice the tick count, and
//! a test that read it as a tick count would be wrong by a factor of two in the
//! direction that makes everything look fine.
//!
//!
//! ⚠️ **Never executed against a ROM.** `sf1_boot.rs`'s module note lists the four
//! premises that a real run of the CPS-1 gated tests falsified — `> 0` floors, 120
//! frames from reset, attract-mode silence as a DIP setting, and a rising counter
//! over a silent chip. This file carries all of them. Read that note first.
//!
//! # Why the assertions are counters and not an audio hash
//!
//! `sound_boot.rs`'s reason, unchanged: `crates/ym2151` already checks the waveform
//! against ymfm over 1,000 vectors. A hash of this machine's output would be a number
//! nothing independent verifies, and it would move every time the scheduler's
//! interleave shifted by a T-state without anything being wrong.

/// SF1's FM driver initialises the chip and plays, over two seconds.
///
/// # What each assertion would catch
///
/// - `audiocpu_fetches > 100_000`: Z80 #1 executed from the `audiocpu` region. The
///   number is deliberately large, and it is why `FmBoard` counts only bytes the ROM
///   *answered*: an absent region reads `sound::UNMAPPED`, which is `RST 38h`, so a
///   machine built without `audiocpu` spins in a tight loop and would clear a naive
///   threshold while running no driver at all. Two seconds at 3.579545 MHz is about
///   7.2 million T-states, so a real driver clears this by an order of magnitude.
/// - `latch_reads > 0`: it read the command latch at 0xC800, so the 68000 and this
///   CPU are connected. The other end of the latch `sf1_boot.rs` only prints.
/// - `ym_writes > 100`: and programmed the FM chip. Initialising one voice is dozens
///   of registers, so a driver that reached the chip clears this easily and one that
///   wrote a handful of bytes and gave up does not.
/// - a non-silent sample: the chip's output actually left the chip. Every counter
///   above is satisfiable by a driver writing registers that produce silence — a
///   key-on that never lands, an envelope that never opens.
/// - `samples().len()` even: the mix is interleaved stereo, two `i16` per tick. An odd
///   count means the scheduler pushed one side of a pair, which every average and
///   peak below would then compute across a channel boundary without complaining.
/// - `port_accesses == 0`: this board has no I/O ports. `sound_map` (`sf.cpp:214-221`)
///   is program space only, so **a non-zero count is a finding to report, not a test
///   to relax** — it would mean the map is not the whole story.
#[test]
#[ignore = "needs a user-supplied ROM set; set SFEMU_ROMS"]
fn sf1s_fm_program_drives_the_ym2151() {
    let Ok(path) = std::env::var("SFEMU_ROMS") else {
        panic!("set SFEMU_ROMS to your own sf.zip or a directory of loose files");
    };
    let set = romset::load(&romset::games::SF1, std::path::Path::new(&path))
        .unwrap_or_else(|e| panic!("cannot load {path}: {e}"));
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
    // `audio2` too, though this test asserts nothing about it: the 68000 posts one
    // command to both boards, and a second CPU spinning on `RST 38h` is a different
    // machine from the one that ships.
    let mut m = machine::Sf1::new(&need("maincpu"), video, need("audiocpu"), need("audio2"));
    m.reset();

    // 120 frames is two seconds: long enough for the driver to initialise the chip and
    // start a track, short enough to stay a test. The samples are never drained, so the
    // buffer below holds the whole run — about 224,000 interleaved samples, 448 KB.
    for _ in 0..120 {
        m.run_frame();
    }
    let t = m.fm.trace();

    assert!(
        t.audiocpu_fetches > 100_000,
        "Z80 #1 ran: {}",
        t.audiocpu_fetches
    );
    assert!(t.latch_reads > 0, "it read the command latch");
    assert!(
        t.ym_writes > 100,
        "and wrote YM2151 registers: {}",
        t.ym_writes
    );
    let samples = m.samples();
    assert_eq!(
        samples.len() % 2,
        0,
        "the mix is interleaved stereo: {} samples is not a whole number of frames",
        samples.len()
    );
    assert!(
        samples.iter().any(|&s| s != 0),
        "and the chip produced non-silent samples out of {}",
        samples.len()
    );
    assert_eq!(t.port_accesses, 0, "the driver used an I/O port after all");

    // Not assertions — reports, so a failing run above prints how far the driver got.
    // `rom_writes` in particular is this board's own diagnostic: 2 KB of RAM against
    // 32 KB of ROM means a stray store is far more likely to land in ROM than
    // anywhere useful, and a large count here explains an otherwise silent driver.
    eprintln!("interleaved samples: {}", samples.len());
    eprintln!("Z80 #1 T-states: {}", m.z80_cycles());
    eprintln!("YM register writes: {}", t.ym_writes);
    eprintln!("latch reads: {}", t.latch_reads);
    eprintln!("writes into ROM: {}", t.rom_writes);
    eprintln!(
        "IRQs raised: FM {}, NMIs {}",
        m.fm_irqs_raised(),
        m.fm_nmis_raised()
    );
}
