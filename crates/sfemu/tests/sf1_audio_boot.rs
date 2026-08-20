//! Does SF1's ADPCM board actually stream, and is the stereo mix audible?
//!
//! # Why this is `#[ignore]`d when the rest of the project forbids it
//!
//! The sixth and last such test, for `sf1_boot.rs`'s reason, which is `boot.rs`'s.
//!
//! ```text
//! SFEMU_ROMS=/path/to/sf.zip cargo test -p sfemu --test sf1_audio_boot -- --ignored
//! ```
//!
//! ⚠️ One variable, two sets — `sf1_boot.rs`'s module doc says why there is no second
//! one.
//!
//! # What this adds over `sf1_sound_boot.rs`
//!
//! That file asks whether the driver reaches the *YM2151*, on Z80 #1. This asks the
//! ADPCM question, on Z80 #2, and the two CPUs share nothing but the latch the 68000
//! writes — so neither answer implies the other.
//!
//! Everything about the MSM5205 *core* is covered by `crates/machine`'s unit tests
//! against `msm5205.cpp`, which run unconditionally and need no ROM. What only a real
//! ROM can show is that the driver talks to the chips at all: that the bank register
//! reaches a window the samples are actually in, that nibbles get written, and that
//! what leaves `sf1::mix` is not silence.
//!
//! # Why the bank counters are assertions and not reports
//!
//! `audio2` is 256 KB in eight 32 KB windows, of which the low one is Z80 #2's
//! program and the other seven are banked at 0x8000. `bank_w` takes a whole byte, so
//! a driver can select entry 7 or entry 200, and those alias — `Adpcm2Trace::bank_overruns`
//! is how a reader learns it happened. An aliasing bank does not fail: it streams
//! 32 KB of the wrong thing, which sounds like noise or like nothing, and every other
//! counter in this file stays healthy through it.

/// SF1's ADPCM driver streams from the banked ROM, and the stereo mix carries audio.
///
/// # What each assertion would catch
///
/// - `rom_fetches > 100_000`: Z80 #2 executed from `audio2`'s low window. The same
///   "as answered" property as the FM board's fetch counter, for the same reason: an
///   absent region reads `RST 38h` and would clear a naive threshold while running no
///   driver.
/// - `latch_reads > 0`: it read port 0x01, so the 68000's command reached this CPU
///   too. ⚠️ Port 0x01 is the latch on the way **in** and MSM chip 1 on the way
///   **out**; a board that had those crossed would fail here or at `msm_writes`.
/// - `bank_writes > 0`: it selected a sample window. Every sample past the first
///   32 KB needs one, and the region is 256 KB.
/// - `bank_fetches > 0`: and read through the banked window, which is the sample data
///   itself. This is the assertion that separates "the driver initialised the chips"
///   from "the driver played something".
/// - the sum of `msm_writes` non-zero: nibbles reached at least one chip.
/// - `bank_overruns == 0`: and the window it selected was one that exists. See the
///   module note — **a non-zero count is a finding to report, not a test to relax.**
/// - `unmapped_ports == 0`: the driver used only ports 0x00, 0x01 and 0x02. Unlike
///   the FM board's `port_accesses`, this is a claim about the *driver* rather than
///   about the map — this board does have ports — so a non-zero count is a finding
///   about which port, and the count is what to report.
/// - a non-empty, even-length sample buffer: the scheduler produced whole interleaved
///   frames, which is the premise everything below reads.
/// - a quarter of the samples non-zero: the mix is not silence with a click in it. A
///   bare `.any(|&s| s != 0)` — which is the right question in `sf1_sound_boot.rs`,
///   about a different thing — passes on one non-zero sample in 224,000, and that is
///   what a driver whose key-on never lands produces.
/// - `peak > 1000`: and it is loud enough to be audio rather than a DC offset or the
///   bottom bit of an envelope that never opened. 1,000 of 32,767 is about −30 dBFS.
#[test]
#[ignore = "needs a user-supplied ROM set; set SFEMU_ROMS"]
fn sf1_streams_adpcm_and_the_stereo_mix_is_audible() {
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
    // The one region this test cannot do without is `audio2`: it is both Z80 #2's
    // program and every sample byte the chips will ever see. `audiocpu` matters too,
    // because the FM term is two thirds of the numerator `sf1::mix` divides.
    let mut m = machine::Sf1::new(&need("maincpu"), video, need("audiocpu"), need("audio2"));
    m.reset();

    for _ in 0..120 {
        m.run_frame();
    }
    let t = m.adpcm.trace();
    eprintln!(
        "ADPCM in 120 frames: chip0 {} writes, chip1 {}, banks {}, bank fetches {}",
        t.msm_writes[0], t.msm_writes[1], t.bank_writes, t.bank_fetches
    );

    assert!(t.rom_fetches > 100_000, "Z80 #2 ran: {}", t.rom_fetches);
    assert!(t.latch_reads > 0, "it read the command latch on port 0x01");
    assert!(t.bank_writes > 0, "it selected a sample window");
    assert!(
        t.bank_fetches > 0,
        "and read sample data through it — a driver that programmed the chips but \
         streamed nothing gets this far with everything else green"
    );
    let nibbles: u32 = t.msm_writes.iter().sum();
    assert!(nibbles > 0, "and wrote nibbles to at least one MSM5205");
    assert_eq!(
        t.bank_overruns, 0,
        "the driver selected a bank above the seven that exist, {} times — it is \
         streaming an aliased window, which sounds like noise",
        t.bank_overruns
    );
    assert_eq!(
        t.unmapped_ports, 0,
        "the driver touched a port this board does not decode, {} times",
        t.unmapped_ports
    );

    let samples = m.samples();
    assert!(!samples.is_empty(), "no samples at all");
    assert_eq!(
        samples.len() % 2,
        0,
        "the mix is interleaved stereo: {} is not a whole number of frames",
        samples.len()
    );
    let nonzero = samples.iter().filter(|&&s| s != 0).count();
    let peak = samples
        .iter()
        .map(|&s| i32::from(s).abs())
        .max()
        .unwrap_or(0);
    let panned = samples.chunks_exact(2).filter(|f| f[0] != f[1]).count();
    eprintln!(
        "{nonzero}/{} non-zero, peak {peak}, {panned} frames panned, {} clips",
        samples.len(),
        m.mix_clips()
    );
    assert!(
        nonzero * 4 > samples.len(),
        "the mix is mostly silence: {nonzero} of {} samples carry anything",
        samples.len()
    );
    assert!(
        peak > 1000,
        "the mix peaks at {peak} — too quiet to be audio"
    );

    // Not assertions — reports.
    //
    // `panned` is deliberately not asserted: `sf1::mix` gives both sides the same
    // ADPCM term by construction, so only the YM's two outputs can differ, and a
    // driver that pans every voice centre produces identical sides legitimately. The
    // number is here because a mix that is *never* panned over two seconds is worth
    // a reader knowing about, not because it is wrong.
    //
    // `writes_discarded` counts stores Z80 #2 made into an address space with no RAM
    // in it at all — MAME's own comment on that map is `/* Yes, _no_ ram */`. A large
    // count means the driver is using a stack this board does not have, which is the
    // kind of thing that would make the map worth re-reading.
    eprintln!("writes discarded (no RAM): {}", t.writes_discarded);
    eprintln!("Z80 #2 T-states: {}", m.adpcm_z80_cycles());
    eprintln!(
        "ADPCM IRQs raised: {}, NMIs {}",
        m.adpcm_irqs_raised(),
        m.adpcm_nmis_raised()
    );
}
