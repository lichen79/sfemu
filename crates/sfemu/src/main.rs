//! Run a CPS-1 ROM set and report what the board saw.
//!
//! ```text
//! sfemu <path-to-rom-set> [frames] [--ppm <path>]
//! ```
//!
//! `<path-to-rom-set>` is a MAME-format zip or a directory of loose files that
//! **you supply**. This program contains no ROM data and no way to obtain any.
//!
//! # Why a report and not a window
//!
//! There is still no window: opening one is sub-project E's. A black window would
//! be indistinguishable from a boot that hangs on the first instruction, whereas a
//! count of vblanks, acknowledges, and video-register writes tells you which — and
//! `--ppm` writes the last frame out as a file, which is a picture you can look at
//! without this program having to draw one.

mod audio;
#[cfg(test)]
mod confine;
mod display;
mod loop_;

use machine::video;
use machine::Trace;
use std::path::PathBuf;
use std::process::ExitCode;

/// What went wrong before the machine ever ran.
#[derive(Debug)]
enum Fault {
    /// No arguments: print the usage text.
    Usage,
    /// A message for stderr.
    Failed(String),
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(report) => {
            print!("{report}");
            ExitCode::SUCCESS
        }
        Err(Fault::Usage) => {
            eprint!("{}", usage());
            // 2, not 1: a usage error is not a failed run, and a script driving
            // this can tell them apart.
            ExitCode::from(2)
        }
        Err(Fault::Failed(msg)) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> String {
    // The legal note is in the usage text and not only in the README because this
    // is where someone who does not have a ROM set arrives.
    "usage: sfemu <path-to-sf2.zip-or-directory> [frames] [--ppm <path>]\n\
     \x20      sfemu <path-to-sf2.zip-or-directory> --play [--state <path>]\n\
     \n\
     Without `--play`, runs a fixed number of frames and reports what the\n\
     board saw. With `--play`, opens a window: arrow keys and ASDZXC for\n\
     player one, 5 to insert a coin, 1 to start, P to pause, . to step,\n\
     F3 to reset, F5 and F8 to save and load, F12 for a screenshot,\n\
     Escape to quit. A frame count is ignored with `--play`.\n\
     \n\
     The ROM set is yours to supply: this program neither bundles nor\n\
     downloads one. Legal sources include Capcom Arcade Stadium, Capcom\n\
     Fighting Collection, or a board you own and dumped.\n"
        .to_string()
}

/// What the command line asked for.
///
/// A struct rather than a tuple: three positional values of which two are strings
/// is a call whose arguments can be swapped without the compiler noticing, and the
/// next option would make it four.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    /// The ROM set to load.
    path: String,
    /// How many frames to run.
    frames: u64,
    /// Where to write the last frame as a binary PPM, if anywhere.
    ppm: Option<String>,
    /// Open a window and play, instead of running a fixed number of frames.
    play: bool,
    /// The save-state file, for F5 and F8. Only meaningful with [`Args::play`].
    ///
    /// Defaults to the ROM set's own path with its extension replaced by `.sfs`, so
    /// a state lands next to the game it belongs to and two games do not share one.
    state: PathBuf,
}

/// The save-state path implied by a ROM set's path.
///
/// `/a/b/sf2.zip` becomes `/a/b/sf2.sfs`. A directory of loose files — the other
/// thing `romset` accepts — has no extension to replace, so `/a/b/sf2` becomes
/// `/a/b/sf2.sfs` as well, which is the same rule and not a special case.
fn default_state_path(rom: &str) -> PathBuf {
    let mut p = PathBuf::from(rom);
    p.set_extension("sfs");
    p
}

/// Parses the command line.
///
/// Split out of [`run`] so it can be tested without a ROM set: everything after
/// this point needs one, so a default folded into `run` could only ever be checked
/// by a test that owns a ROM — which is to say, never.
fn parse_args(args: Vec<String>) -> Result<Args, Fault> {
    let mut rest = Vec::new();
    let mut ppm = None;
    let mut play = false;
    let mut state = None;
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        if a == "--play" {
            play = true;
        } else if a == "--state" {
            let p = args
                .next()
                .ok_or_else(|| Fault::Failed("`--state` needs a path".to_string()))?;
            if state.replace(p).is_some() {
                return Err(Fault::Failed("`--state` given twice".to_string()));
            }
        } else if a == "--ppm" {
            // An absent value is an error rather than a silent no-op: a run asked
            // to write a frame and quietly writing none is the failure this whole
            // program's strictness exists to avoid.
            let p = args
                .next()
                .ok_or_else(|| Fault::Failed("`--ppm` needs a path".to_string()))?;
            if ppm.replace(p).is_some() {
                return Err(Fault::Failed("`--ppm` given twice".to_string()));
            }
        } else {
            rest.push(a);
        }
    }

    let mut rest = rest.into_iter();
    let path = rest.next().ok_or(Fault::Usage)?;
    // Parsed strictly. `unwrap_or(60)` on a typo — `6O` with a letter O — would
    // run a different number of frames than asked for and say nothing, and the
    // whole point of this program is that its numbers mean what they say.
    let frames: u64 = match rest.next() {
        None => 60,
        Some(s) => s
            .parse()
            .map_err(|_| Fault::Failed(format!("`{s}` is not a frame count")))?,
    };
    if let Some(extra) = rest.next() {
        return Err(Fault::Failed(format!("unexpected argument `{extra}`")));
    }
    // `--state` without `--play` is a mistake worth naming rather than ignoring:
    // nothing but the window reads or writes a state, so the flag would have no
    // effect at all and a user would be left wondering where their file went.
    if state.is_some() && !play {
        return Err(Fault::Failed(
            "`--state` needs `--play`: only the window reads and writes states".to_string(),
        ));
    }
    let state = state.map_or_else(|| default_state_path(&path), PathBuf::from);
    Ok(Args {
        path,
        frames,
        ppm,
        play,
        state,
    })
}

/// Loads the set, runs `frames` frames, and returns the report.
fn run(args: Vec<String>) -> Result<String, Fault> {
    let args = parse_args(args)?;

    let set = romset::load(&romset::games::SF2, std::path::Path::new(&args.path))
        .map_err(|e| Fault::Failed(e.to_string()))?;
    let prog = set.region("maincpu").ok_or_else(|| {
        Fault::Failed("internal: the sf2 spec has no `maincpu` region".to_string())
    })?;
    // Required, not optional: the spec always has this region, so `None` here would
    // be a bug in `romset::games` rather than something about the user's files — and
    // defaulting to an empty region would turn it into a blank frame with no
    // explanation, which is the one outcome the framebuffer line must never be
    // ambiguous about.
    let gfx = set
        .region("gfx")
        .ok_or_else(|| Fault::Failed("internal: the sf2 spec has no `gfx` region".to_string()))?
        .to_vec();
    // The sound region, on the same terms and for a sharper reason: an absent one is
    // not silence, it is 0xFF on every fetch, which is `RST 38h` — a Z80 spinning in a
    // deterministic loop that racks up a *larger* fetch count than a real driver. The
    // debugger's sound panel would show that loop, and it would look like a working
    // panel on a broken driver rather than a machine built without a sound ROM.
    let audiocpu = set
        .region("audiocpu")
        .ok_or_else(|| {
            Fault::Failed("internal: the sf2 spec has no `audiocpu` region".to_string())
        })?
        .to_vec();

    // The ADPCM samples, on the same terms again. An absent one is silence rather
    // than noise — every phrase header would read as `start == stop == 0`, which the
    // chip refuses — but it is silence with no explanation, and "no sound effects,
    // music fine" is the hardest symptom here to attribute to a missing region.
    let okirom = set
        .region("oki")
        .ok_or_else(|| Fault::Failed("internal: the sf2 spec has no `oki` region".to_string()))?
        .to_vec();

    let mut m = machine::Cps1::with_sound(
        prog,
        gfx,
        audiocpu,
        okirom,
        machine::BoardConfig::sf2(),
        machine::Timing::cps1_10mhz(),
    );
    m.reset();

    if args.play {
        // The window is opened *after* the ROM set loads, so a bad path reports the
        // load error rather than flashing a window onto a machine that never booted.
        let mut win = display::Window::open("sfemu").map_err(Fault::Failed)?;
        let mut sink = open_audio();
        let opts = loop_opts(&args);
        // The wrap is here and not at construction, because this branch returns and the
        // headless path below still reads the board directly — `m.board.trace`,
        // `m.video`, `Cpu::of(&m)`. Wrapping earlier would put a `match` in front of each
        // of those for no gain: `Machine` deliberately exposes no accessor that reaches
        // inside a variant.
        //
        // ⚠️ One arm until Task 21 adds the SF1 driver's, at which point the board comes
        // from the ROM set rather than a flag — a user who has sf2.zip should not have to
        // tell the emulator what sf2.zip is.
        let mut m = machine::Machine::Cps1(Box::new(m));
        let summary = loop_::run(&mut m, &mut win, sink.as_mut(), &opts);
        return Ok(play_report(&summary));
    }

    for _ in 0..args.frames {
        m.run_frame();
    }
    m.render();

    if let Some(path) = &args.ppm {
        std::fs::write(path, ppm(&m.video))
            .map_err(|e| Fault::Failed(format!("cannot write `{path}`: {e}")))?;
    }

    Ok(report(
        &m.board.trace,
        m.total_cycles,
        Cpu::of(&m),
        Frame::of(&m.video),
    ))
}

/// The audio sink, or a silent one and a line on stderr saying why.
///
/// A host with no output device, a device that refuses the default configuration, a
/// machine with no sound card at all: none of these is a reason to refuse to run SF2.
/// The notice goes to stderr rather than into [`loop_::Summary`] because it happens
/// before the loop exists, and it is printed rather than swallowed because "no sound"
/// with no explanation is a bug report nobody can act on.
///
/// Returns a `Box<dyn Audio>` and not a generic, which is why [`loop_::run`] takes
/// `&mut dyn Audio`: the choice between a device and silence is made here, at runtime.
fn open_audio() -> Box<dyn audio::Audio> {
    match audio::CpalAudio::open() {
        Ok(a) => {
            // Both rates, because the interesting number is the pair: the board's
            // 55,930.39 Hz is not any device's rate, and printing only the device's
            // would leave a reader with no reason the samples are being converted at
            // all. Three decimals because the board's rate is not an integer, which is
            // the whole reason `machine::resample` exists.
            eprintln!(
                "audio: device {} Hz, board {:.3} Hz",
                a.rate(),
                f64::from(audio::SAMPLE_RATE_NUM) / f64::from(audio::SAMPLE_RATE_DEN)
            );
            Box::new(a)
        }
        Err(e) => {
            eprintln!("notice: no audio ({e}); running silently");
            Box::new(audio::NullAudio::default())
        }
    }
}

/// The two paths the loop writes to, from the parsed arguments.
///
/// Split out of [`run`] for the same reason [`parse_args`] is: everything else in
/// `run` needs a ROM set, so a `LoopOpts` built inline could only be checked by a
/// test that owns one. And it is worth checking — the struct has two same-typed
/// fields, so swapping them compiles, and the symptom is F5 overwriting your
/// screenshot with a save state.
fn loop_opts(args: &Args) -> loop_::LoopOpts {
    loop_::LoopOpts {
        state_path: args.state.clone(),
        shot_path: default_shot_path(&args.path),
    }
}

/// Where F12 writes.
///
/// Beside the ROM set, like the state file, and `.ppm` because that is what
/// [`ppm`] writes. One path rather than a numbered series: a screenshot you have to
/// hunt for is worse than one you have to move, and the alternative is this program
/// scanning a directory to find the next free number.
fn default_shot_path(rom: &str) -> PathBuf {
    let mut p = PathBuf::from(rom);
    p.set_extension("ppm");
    p
}

/// What a finished session printed on the way out.
///
/// Short, and printed rather than discarded because a session that dropped frames or
/// could not write a state should say so once at the end — the title bar said it at
/// the time, and the title bar is gone.
fn play_report(s: &loop_::Summary) -> String {
    let mut out = format!("frames        {}\ndropped       {}\n", s.frames, s.dropped);
    for n in &s.notices {
        out.push_str(&format!("notice        {n}\n"));
    }
    out
}

/// The last rendered frame as a binary PPM (`P6`).
///
/// Returns the bytes rather than writing them, so the format is testable without
/// touching the filesystem. `P6` and not the ASCII `P3` because a 384×224 frame is
/// 258,048 bytes binary and around a megabyte as text, and every image viewer
/// reads both.
///
/// The header is `P6\n384 224\n255\n`: the maxval is 255 because the body is one
/// byte per sample. Declaring 65535 would make a reader consume two bytes per
/// sample and produce a garbled half-height image out of a body that is perfectly
/// correct.
fn ppm(v: &video::compose::Video) -> Vec<u8> {
    let mut out = format!("P6\n{} {}\n255\n", video::WIDTH, video::HEIGHT).into_bytes();
    let rgb = v.rgb();
    assert_eq!(
        rgb.len(),
        video::WIDTH * video::HEIGHT * 3,
        "three bytes per pixel of the visible frame"
    );
    out.extend_from_slice(&rgb);
    out
}

/// The three CPU facts the report prints.
///
/// A small struct rather than a `&M68k` so this crate needs no `m68k` dependency
/// of its own: `machine` already owns that edge, and the report has no business
/// reaching into the core.
#[derive(Debug, Clone, Copy)]
struct Cpu {
    pc: u32,
    halted: bool,
    stopped: bool,
}

impl Cpu {
    fn of(m: &machine::Cps1) -> Self {
        Self {
            pc: m.cpu.pc,
            halted: m.cpu.halted,
            stopped: m.cpu.stopped,
        }
    }
}

/// The two facts the report prints about the rendered frame.
///
/// A struct computed by [`Frame::of`] rather than a `&Video` handed to [`report`],
/// for the same reason [`Cpu`] is: the summary is what the report is entitled to,
/// and a test can then state the numbers it wants without building a renderer.
///
/// # Why these two numbers
///
/// This is the line the real-ROM check will grow into, and it has to distinguish
/// three outcomes that a screenshot distinguishes at a glance and a cycle count
/// does not: nothing drew (`drawn` 0), something drew but out of one palette page
/// (a plausible attract-mode logo), and something drew out of several (a game
/// scene, which is what SF2 in play looks like). The page count is the second
/// number because a frame can be full of pens that all resolve to one page — a
/// renderer with a broken palette base does exactly that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame {
    /// Pixels whose pen is not the background pen.
    drawn: usize,
    /// How many of the six palette pages those pens fall in.
    pages: usize,
}

impl Frame {
    fn of(v: &video::compose::Video) -> Self {
        let mut pages = [false; video::palette::PAGES];
        let mut drawn = 0;
        for &pen in v.fb.pens.iter() {
            if pen == video::palette::BACKGROUND_PEN {
                continue;
            }
            drawn += 1;
            // A pen past the palette is not possible from this renderer — a tile
            // reaches 0x7FF and the star pens 0xBFF — but the report must not be
            // the thing that panics if one ever is.
            if let Some(p) = pages.get_mut(usize::from(pen) / video::palette::PAGE_ENTRIES) {
                *p = true;
            }
        }
        Self {
            drawn,
            pages: pages.iter().filter(|&&p| p).count(),
        }
    }
}

/// Formats a run's trace and its last frame.
///
/// Separate from [`run`] so it can be tested against a machine this crate builds
/// itself — the loader path needs a ROM set and therefore cannot be tested here at
/// all, but the report is pure and gets literals like everything else.
///
fn report(t: &Trace, cycles: u64, cpu: Cpu, frame: Frame) -> String {
    let mut s = String::new();
    let line = |s: &mut String, k: &str, v: String| {
        s.push_str(&format!("{k:<14}{v}\n"));
    };
    line(&mut s, "frames", t.frames.to_string());
    line(&mut s, "vblanks", format!("{}  acks {}", t.vblanks, t.acks));
    line(&mut s, "cycles", cycles.to_string());
    line(
        &mut s,
        "cpu",
        format!(
            "pc {:#08x}  {}",
            cpu.pc,
            match (cpu.halted, cpu.stopped) {
                // Halted before stopped, because the core can be both and the fault
                // is the fact worth reporting: the 68000 halts on a fault taken
                // while already taking one, which is what a wrong map produces.
                (true, _) => "HALTED (double bus fault)",
                (_, true) => "stopped (waiting for an interrupt)",
                _ => "running",
            }
        ),
    );
    line(
        &mut s,
        "framebuffer",
        format!(
            "{} of {} pixels drawn, {} palette page(s)",
            frame.drawn,
            video::WIDTH * video::HEIGHT,
            frame.pages
        ),
    );
    line(&mut s, "cps-a writes", t.cps_a_writes.to_string());
    line(&mut s, "cps-b writes", t.cps_b_writes.to_string());
    line(&mut s, "gfxram writes", t.gfxram_writes.to_string());
    line(&mut s, "sound latch", t.sound_latch_writes.to_string());
    line(&mut s, "rom writes", t.rom_writes.to_string());
    line(
        &mut s,
        "unmapped",
        format!(
            "{} reads, {} writes",
            t.unmapped_reads.total(),
            t.unmapped_writes.total()
        ),
    );
    for (tag, log) in [("W", &t.unmapped_writes), ("R", &t.unmapped_reads)] {
        for (a, n) in log.worst(8) {
            s.push_str(&format!("  {tag} {a:#08x}  {n}\n"));
        }
        // Printing `total` without this would read as a complete list when the
        // distinct-address cap has silently made it a sample.
        if log.dropped() > 0 {
            s.push_str(&format!(
                "  {tag} …{} more accesses to addresses past the 1024-address cap\n",
                log.dropped()
            ));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CPU doing nothing in particular, for the tests that only exercise the
    /// trace half of the report.
    const IDLE: Cpu = Cpu {
        pc: 0,
        halted: false,
        stopped: false,
    };

    /// A frame in which nothing drew, for those same tests.
    const BLANK: Frame = Frame { drawn: 0, pages: 0 };

    /// A machine running a program this test writes, so the report has real
    /// numbers in it without a ROM set.
    ///
    /// ```text
    /// 1000  33FC 0040 0080 010C   move.w #$0040,$80010C   CPS-A
    /// 1008  33FC FFFF 0081 0000   move.w #$FFFF,$810000   unmapped
    /// 1010  4E72 2700             stop #$2700
    /// ```
    fn one_frame() -> machine::Cps1 {
        let mut rom = vec![0u8; 0x2000];
        let words: &[u16] = &[
            0x00FF, 0x8000, 0x0000, 0x1000, // SSP, PC
        ];
        for (i, w) in words.iter().enumerate() {
            rom[2 * i..2 * i + 2].copy_from_slice(&w.to_be_bytes());
        }
        let prog: &[u16] = &[
            0x33FC, 0x0040, 0x0080, 0x010C, //
            0x33FC, 0xFFFF, 0x0081, 0x0000, //
            0x4E72, 0x2700,
        ];
        for (i, w) in prog.iter().enumerate() {
            rom[0x1000 + 2 * i..0x1002 + 2 * i].copy_from_slice(&w.to_be_bytes());
        }
        let mut m = machine::Cps1::new(
            &rom,
            machine::BoardConfig::sf2(),
            machine::Timing::cps1_10mhz(),
        );
        m.reset();
        m.run_frame();
        m
    }

    /// The report names every counter and prints the worst unmapped addresses.
    ///
    /// Compared against the whole expected text rather than with `contains`: a
    /// `contains` check passes when a line is missing, and a missing line is the
    /// failure mode — this report exists to be read, and a counter that silently
    /// stops being printed is a diagnostic that silently stops working.
    ///
    /// The PC is 0x001014: `stop #$2700` occupies 0x1010-0x1013, and `STOP` loads
    /// SR and then leaves the PC past its own extension word.
    #[test]
    fn the_report_prints_every_counter_with_its_value() {
        let m = one_frame();
        assert_eq!(
            report(&m.board.trace, m.total_cycles, Cpu::of(&m), BLANK),
            format!(
                "frames        1\n\
                 vblanks       1  acks 0\n\
                 cycles        {}\n\
                 cpu           pc 0x001014  stopped (waiting for an interrupt)\n\
                 framebuffer   0 of 86016 pixels drawn, 0 palette page(s)\n\
                 cps-a writes  1\n\
                 cps-b writes  0\n\
                 gfxram writes 0\n\
                 sound latch   0\n\
                 rom writes    0\n\
                 unmapped      0 reads, 1 writes\n\
                 \x20 W 0x810000  1\n",
                m.total_cycles
            ),
        );
    }

    /// The cycle count is the machine's, and it is a frame's worth.
    ///
    /// The test above interpolates `m.total_cycles` into its expectation, which
    /// proves the number is printed but not that it is right. This pins it against
    /// the hand-written 167,680 — 640 × 262 — plus at most one instruction, the
    /// same bound `cps1.rs` uses.
    #[test]
    fn the_reported_cycle_count_is_one_frames_worth() {
        let m = one_frame();
        assert!(
            (167_680..167_680 + 16).contains(&m.total_cycles),
            "got {}",
            m.total_cycles
        );
        let r = report(&m.board.trace, m.total_cycles, Cpu::of(&m), BLANK);
        // The exact total depends on where the `stop` lands relative to the budget,
        // so the printed digits are checked against the machine's own value — but
        // the *bound* above is the hand-written 640 × 262, so this pair pins both
        // that the number is right and that it reaches the page.
        assert!(
            r.contains(&format!("cycles        {}\n", m.total_cycles)),
            "the cycle line is missing or reformatted: {r}"
        );
    }

    /// A halted CPU is called out, because that is the one state that means our
    /// memory map is wrong rather than the game's code being unfinished.
    #[test]
    fn a_halted_cpu_is_reported_as_a_double_bus_fault() {
        let mut m = one_frame();
        m.cpu.halted = true;
        let r = report(&m.board.trace, m.total_cycles, Cpu::of(&m), BLANK);
        assert!(
            r.contains("HALTED (double bus fault)"),
            "the report must say so plainly: {r}"
        );
        assert!(
            !r.contains("running"),
            "and must not also claim it is running"
        );
    }

    /// A run with nothing unmapped prints no per-address lines at all.
    ///
    /// The eight-worst loop over an empty log must produce nothing, not a header
    /// with no rows under it.
    #[test]
    fn a_clean_run_prints_no_per_address_lines() {
        let t = Trace::default();
        let r = report(&t, 0, IDLE, BLANK);
        assert!(r.contains("unmapped      0 reads, 0 writes\n"));
        assert!(!r.contains("  W "), "no write rows: {r}");
        assert!(!r.contains("  R "), "no read rows");
        assert!(!r.contains("cap"), "and no truncation note");
    }

    /// The report lists more than one unmapped address.
    ///
    /// The single-address case cannot tell `worst(8)` from `worst(1)`, and one row
    /// is the wrong output for the situation this report is for: a board missing a
    /// chip is usually missing a *range*, and a list truncated to its worst entry
    /// hides that the neighbouring addresses are being hit too. Nine addresses, so
    /// the eighth is printed and the ninth is not.
    ///
    /// The counts descend with the address so the expected order is unambiguous:
    /// 0x810000 is hit nine times, 0x810002 eight, down to 0x810010 once.
    #[test]
    fn the_report_lists_up_to_eight_unmapped_addresses() {
        let mut t = Trace::default();
        for i in 0..9u32 {
            for _ in 0..(9 - i) {
                t.unmapped_writes.record(0x81_0000 + i * 2);
            }
        }
        let r = report(&t, 0, IDLE, BLANK);
        for i in 0..8u32 {
            let row = format!("  W {:#08x}  {}\n", 0x81_0000 + i * 2, 9 - i);
            assert!(r.contains(&row), "row {i} missing: {r}");
        }
        assert!(
            !r.contains("0x810010"),
            "the ninth-worst address is past the eight rows: {r}"
        );
        assert!(
            r.contains("unmapped      0 reads, 45 writes\n"),
            "and the total still counts all 45: {r}"
        );
    }

    /// Past the distinct-address cap the report says so.
    ///
    /// Without this line the `unmapped` total and an eight-row list read as the
    /// whole story when the list is a sample of 1024 addresses out of more.
    #[test]
    fn the_report_admits_when_the_address_list_is_truncated() {
        let mut t = Trace::default();
        for i in 0..1030u32 {
            t.unmapped_writes.record(0x40_0000 + i * 2);
        }
        let r = report(&t, 0, IDLE, BLANK);
        assert!(
            r.contains("  W …6 more accesses to addresses past the 1024-address cap\n"),
            "1030 - 1024 = 6: {r}"
        );
    }

    /// Argument handling: no path is a usage error, a bad frame count is an error
    /// naming the argument, and a stray third argument is rejected.
    ///
    /// A silently-defaulted frame count is the failure this guards: a run asked for
    /// 600 frames and given 60 would report a stalled boot as a healthy one.
    /// With no count given, the run is 60 frames — one second of hardware time.
    ///
    /// A literal, and load-bearing: 60 frames is long enough for SF2's boot
    /// self-test to finish and the attract mode to start writing video registers,
    /// which is the whole signal the report carries. A default of 1 would report
    /// a healthy boot as a stalled one, and every other test here passes an
    /// explicit count and so cannot see it.
    #[test]
    fn the_default_frame_count_is_sixty() {
        let args = |v: Vec<&str>| parse_args(v.into_iter().map(String::from).collect()).ok();
        assert_eq!(
            args(vec!["/some/sf2.zip"]),
            Some(Args {
                path: "/some/sf2.zip".to_string(),
                frames: 60,
                ppm: None,
                play: false,
                state: PathBuf::from("/some/sf2.sfs"),
            })
        );
        assert_eq!(
            args(vec!["/some/sf2.zip", "7"]),
            Some(Args {
                path: "/some/sf2.zip".to_string(),
                frames: 7,
                ppm: None,
                play: false,
                state: PathBuf::from("/some/sf2.sfs"),
            }),
            "and an explicit count is taken as given"
        );
    }

    /// `--ppm` takes a path, is optional, and does not consume a positional slot.
    ///
    /// The last case is the one worth having: with `--ppm` between the path and the
    /// frame count, a parser that walked positionally would read `/tmp/f.ppm` as the
    /// frame count and fail, or worse read `--ppm` as the path.
    #[test]
    fn the_ppm_option_is_parsed_out_of_the_positional_arguments() {
        let args = |v: Vec<&str>| parse_args(v.into_iter().map(String::from).collect());
        let want = Args {
            path: "/some/sf2.zip".to_string(),
            frames: 7,
            ppm: Some("/tmp/f.ppm".to_string()),
            play: false,
            state: PathBuf::from("/some/sf2.sfs"),
        };
        assert_eq!(
            args(vec!["/some/sf2.zip", "7", "--ppm", "/tmp/f.ppm"]).ok(),
            Some(want.clone())
        );
        assert_eq!(
            args(vec!["--ppm", "/tmp/f.ppm", "/some/sf2.zip", "7"]).ok(),
            Some(want.clone()),
            "leading"
        );
        assert_eq!(
            args(vec!["/some/sf2.zip", "--ppm", "/tmp/f.ppm", "7"]).ok(),
            Some(want),
            "and in the middle, where a positional walk would misread it"
        );

        // A missing value is an error, not a silently ignored flag.
        match args(vec!["/some/sf2.zip", "--ppm"]) {
            Err(Fault::Failed(m)) => assert_eq!(m, "`--ppm` needs a path"),
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }
        match args(vec!["/some/sf2.zip", "--ppm", "a", "--ppm", "b"]) {
            Err(Fault::Failed(m)) => assert_eq!(m, "`--ppm` given twice"),
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }

        // An option this program does not know is a loud error, not a dropped
        // argument. `--ppm` is the only option there is, so the walk above could
        // just as easily have skipped everything else beginning with a dash — and
        // then `sfemu set.zip --pmm out.ppm` would run sixty frames, write no file,
        // and print a successful-looking report.
        match args(vec!["/some/sf2.zip", "--pmm"]) {
            Err(Fault::Failed(m)) => assert_eq!(m, "`--pmm` is not a frame count"),
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }
    }

    #[test]
    fn arguments_are_parsed_strictly() {
        assert!(matches!(run(vec![]), Err(Fault::Usage)));
        match run(vec!["/nonexistent".into(), "6O".into()]) {
            Err(Fault::Failed(m)) => assert_eq!(m, "`6O` is not a frame count"),
            other => panic!("expected a frame-count error, got {:?}", other.is_ok()),
        }
        match run(vec!["/nonexistent".into(), "60".into(), "extra".into()]) {
            Err(Fault::Failed(m)) => assert_eq!(m, "unexpected argument `extra`"),
            other => panic!("expected an extra-argument error, got {:?}", other.is_ok()),
        }
        // And a path that does not exist is a load error, not a panic. The message
        // comes from `romset`, which names the path.
        match run(vec!["/nonexistent-rom-set".into()]) {
            Err(Fault::Failed(m)) => assert!(
                m.contains("/nonexistent-rom-set"),
                "the message must name the path: {m}"
            ),
            other => panic!("expected a load error, got {:?}", other.is_ok()),
        }
    }

    /// A renderer with one solid tile on screen, so the frame tests have pens.
    ///
    /// Built through `machine`, not by hand: what these tests are for is the wiring,
    /// and a `Video` constructed here directly would test `video` a second time
    /// while proving nothing about this crate's path to it.
    fn a_drawn_frame() -> machine::Cps1 {
        // A 16x16 tile solid in pen 0x0A: plane bytes all 0x00 or all 0xFF, four
        // per group, two groups per row of a 16-wide tile.
        let mut gfx = vec![0u8; 128];
        for row in 0..16 {
            for half in [0usize, 4] {
                // 0x0A is bits 1 and 3, so planes 1 and 3 are solid and 0 and 2 are
                // empty.
                gfx[row * 8 + half + 1] = 0xFF;
                gfx[row * 8 + half + 3] = 0xFF;
            }
        }
        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        // `move #$2700,sr` then `stop #$2700`, so the frame runs without vectoring.
        rom[0x1000..0x1008].copy_from_slice(&[0x46, 0xFC, 0x27, 0x00, 0x4E, 0x72, 0x27, 0x00]);
        let mut m = machine::Cps1::with_gfx(
            &rom,
            gfx,
            machine::BoardConfig::sf2(),
            machine::Timing::cps1_10mhz(),
        );
        m.reset();
        // Object table at word 0x2000, one sprite of colour 3 at visible (0, 0),
        // with an end marker behind it.
        m.board.cps_a[video::regs::OBJ_BASE] = 0x40;
        for (i, w) in [
            video::VISIBLE_X as u16,
            video::VISIBLE_Y as u16,
            0,
            3, // colour 3
        ]
        .into_iter()
        .enumerate()
        {
            m.board.gfxram[0x2000 + i] = w;
        }
        m.board.gfxram[0x2007] = 0xFF00;
        // Palette page 0 enabled, and pen 0x3A given a value, so the PPM body is
        // not all one colour by accident.
        m.board.cps_b[machine::BoardConfig::sf2().video.palette_control] = 0x0001;
        m.board.gfxram[0x3A] = 0x0F00;
        m.run_frame();
        m.render();
        m
    }

    /// The PPM is a binary P6 with the frame's exact dimensions and one byte per
    /// sample.
    ///
    /// Every number is a literal: 384, 224, 255, and 384 × 224 × 3 = 258,048. A
    /// header interpolated from the same constants the body is sized from would
    /// agree with itself whatever those constants were.
    #[test]
    fn the_ppm_header_is_a_binary_p6_of_the_right_size() {
        let m = a_drawn_frame();
        let bytes = ppm(&m.video);
        let header = b"P6\n384 224\n255\n";
        assert_eq!(&bytes[..header.len()], header, "the exact header");
        assert_eq!(
            bytes.len() - header.len(),
            258_048,
            "384 * 224 * 3 bytes of body"
        );
        assert_eq!(bytes.len(), 258_063, "and nothing after it");

        // The body is the frame and not a fill: the sprite's 16x16 corner is the
        // colour its palette entry gives, and the background around it is not.
        let pixel = |x: usize, y: usize| {
            let i = header.len() + (y * 384 + x) * 3;
            [bytes[i], bytes[i + 1], bytes[i + 2]]
        };
        let sprite = pixel(0, 0);
        assert_ne!(sprite, pixel(200, 200), "the sprite is not the background");
        assert_eq!(sprite, pixel(15, 15), "and it is a whole 16x16 tile");
    }

    /// The report names the framebuffer, with the drawn count and the page count.
    ///
    /// This is the line the real-ROM check grows into, so it is asserted against a
    /// frame whose content is known exactly: one 16×16 sprite is 256 pixels out of
    /// 86,016, from one palette page.
    #[test]
    fn the_report_names_the_framebuffer() {
        let m = a_drawn_frame();
        let f = Frame::of(&m.video);
        assert_eq!(f.drawn, 256, "one 16x16 sprite");
        assert_eq!(f.pages, 1, "pen 0x3A is in page 0");
        let r = report(&m.board.trace, m.total_cycles, Cpu::of(&m), f);
        assert!(
            r.contains("framebuffer   256 of 86016 pixels drawn, 1 palette page(s)\n"),
            "384 * 224 = 86016: {r}"
        );

        // A blank frame says so rather than omitting the line, which is the state a
        // stalled boot leaves and the one this line has to be able to report.
        let r = report(&Trace::default(), 0, IDLE, BLANK);
        assert!(
            r.contains("framebuffer   0 of 86016 pixels drawn, 0 palette page(s)\n"),
            "{r}"
        );
    }

    /// A pen in a second palette page is counted as a second page.
    ///
    /// [`the_report_names_the_framebuffer`]'s frame has one page, so on its own it
    /// cannot tell the page count from the constant 1 — and 1 is exactly what a
    /// broken palette base produces on a real frame, which is the case this number
    /// exists to expose.
    #[test]
    fn the_page_count_counts_distinct_pages() {
        assert_eq!(video::palette::PAGE_ENTRIES, 0x200, "512 pens per page");
        let mut m = a_drawn_frame();
        assert_eq!(Frame::of(&m.video).pages, 1, "the premise");

        // Move one of the sprite's pixels into page 1 by hand. Reaching into the
        // framebuffer is legitimate here: the subject is the *counting*, and a
        // second page from a second sprite would need a second colour scheme and
        // tell us less clearly.
        m.video.fb.pens[0] = 0x200;
        assert_eq!(Frame::of(&m.video).pages, 2, "0x200 is the start of page 1");
        assert_eq!(Frame::of(&m.video).drawn, 256, "still 256 drawn pixels");

        m.video.fb.pens[1] = 0xBFE;
        assert_eq!(Frame::of(&m.video).pages, 3, "0xBFE is in page 5, the last");
    }

    /// The usage text names the legal sources and promises no download.
    ///
    /// This is a project-wide constraint, not a cosmetic one: this text is where
    /// someone without a ROM set arrives, and it must not read as an invitation to
    /// go and find one.
    #[test]
    fn the_usage_text_states_that_no_rom_is_supplied_or_fetched() {
        let u = usage();
        assert!(u.contains("neither bundles nor"));
        assert!(u.contains("downloads one"));
        assert!(u.contains("Capcom Arcade Stadium"));
        assert!(
            !u.contains("http"),
            "no URL of any kind belongs in this program"
        );
    }

    /// `--play` parses, with and without `--state`.
    #[test]
    fn the_play_flag_parses_with_and_without_a_state_path() {
        let args = |v: Vec<&str>| parse_args(v.into_iter().map(String::from).collect());
        assert_eq!(
            args(vec!["/some/sf2.zip", "--play"]).ok(),
            Some(Args {
                path: "/some/sf2.zip".to_string(),
                frames: 60,
                ppm: None,
                play: true,
                state: PathBuf::from("/some/sf2.sfs"),
            }),
        );
        assert_eq!(
            args(vec!["/some/sf2.zip", "--play", "--state", "/tmp/mine.sfs"]).ok(),
            Some(Args {
                path: "/some/sf2.zip".to_string(),
                frames: 60,
                ppm: None,
                play: true,
                state: PathBuf::from("/tmp/mine.sfs"),
            }),
            "an explicit state path wins over the derived one"
        );
        // Order-independent, like `--ppm`: a positional walk would misread this.
        assert_eq!(
            args(vec!["--play", "/some/sf2.zip"]).ok().map(|a| a.play),
            Some(true),
            "leading"
        );
    }

    /// The default state path is derived from the ROM path.
    ///
    /// Literal expectations, not a re-derivation: computing the answer the same way
    /// the code does would pass for any rule at all.
    #[test]
    fn the_default_state_path_sits_beside_the_rom_set() {
        assert_eq!(
            default_state_path("/a/b/sf2.zip"),
            PathBuf::from("/a/b/sf2.sfs")
        );
        assert_eq!(
            default_state_path("/a/b/sf2"),
            PathBuf::from("/a/b/sf2.sfs"),
            "a directory of loose files has no extension to replace"
        );
        assert_eq!(
            default_state_path("sf2.zip"),
            PathBuf::from("sf2.sfs"),
            "a bare filename stays bare"
        );
        assert_eq!(
            default_state_path("/a/b.c/sf2.zip"),
            PathBuf::from("/a/b.c/sf2.sfs"),
            "a dot in a directory name is not the extension"
        );
        assert_eq!(
            default_shot_path("/a/b/sf2.zip"),
            PathBuf::from("/a/b/sf2.ppm"),
            "and the screenshot lands beside it too"
        );
    }

    /// The loop is handed the state path as the state path.
    ///
    /// `LoopOpts` has two `PathBuf` fields, so swapping them compiles and the
    /// symptom is F5 writing a save state over your screenshot. Distinct extensions
    /// on purpose: with both derived from the same stem, a swap would be invisible.
    #[test]
    fn the_loop_is_given_the_state_path_and_the_shot_path_the_right_way_round() {
        let args = Args {
            path: "/a/b/sf2.zip".to_string(),
            frames: 60,
            ppm: None,
            play: true,
            state: PathBuf::from("/tmp/mine.sfs"),
        };
        let o = loop_opts(&args);
        assert_eq!(o.state_path, PathBuf::from("/tmp/mine.sfs"));
        assert_eq!(o.shot_path, PathBuf::from("/a/b/sf2.ppm"));
    }

    /// `--state` without `--play` is an error naming both flags.
    #[test]
    fn a_state_path_without_play_is_an_error() {
        let args = |v: Vec<&str>| parse_args(v.into_iter().map(String::from).collect());
        match args(vec!["/some/sf2.zip", "--state", "/tmp/s.sfs"]) {
            Err(Fault::Failed(m)) => {
                assert!(m.contains("--state"), "names the flag given: {m}");
                assert!(m.contains("--play"), "and the one missing: {m}");
            }
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }
        // And a missing value is an error rather than a silently ignored flag.
        match args(vec!["/some/sf2.zip", "--play", "--state"]) {
            Err(Fault::Failed(m)) => assert_eq!(m, "`--state` needs a path"),
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }
        match args(vec![
            "/some/sf2.zip",
            "--play",
            "--state",
            "/a",
            "--state",
            "/b",
        ]) {
            Err(Fault::Failed(m)) => assert_eq!(m, "`--state` given twice"),
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }
    }

    /// A frame count with `--play` parses, and is ignored.
    ///
    /// Not an error: there is no reading of `sfemu set.zip 60 --play` under which the
    /// user wants a window that closes after one second.
    #[test]
    fn a_frame_count_is_accepted_and_ignored_with_play() {
        let args = |v: Vec<&str>| parse_args(v.into_iter().map(String::from).collect());
        let a = args(vec!["/some/sf2.zip", "600", "--play"]).expect("this must parse");
        assert!(a.play);
        assert_eq!(
            a.frames, 600,
            "parsed, and `run` does not read it when playing"
        );
    }

    /// A bad ROM path with `--play` reports the load error and does not open a window.
    ///
    /// The one thing about this feature a test without a display can check, and the
    /// reason `Window::open` is called after `romset::load` rather than before.
    #[test]
    fn a_bad_rom_path_with_play_reports_the_load_error() {
        match run(vec!["/nonexistent-rom-set".into(), "--play".into()]) {
            Err(Fault::Failed(m)) => {
                assert!(m.contains("/nonexistent-rom-set"), "names the path: {m}");
                assert!(
                    !m.contains("window"),
                    "and it is the load that failed, not the window: {m}"
                );
            }
            other => panic!("expected a load error, got {:?}", other.is_ok()),
        }
    }

    /// The session report prints what the loop returned.
    #[test]
    fn the_play_report_prints_the_counts_and_every_notice() {
        let s = loop_::Summary {
            frames: 3_600,
            dropped: 12,
            notices: vec!["cannot write `/x/y.sfs`: nope".to_string()],
        };
        let r = play_report(&s);
        assert!(r.contains("frames        3600"), "{r}");
        assert!(r.contains("dropped       12"), "{r}");
        assert!(
            r.contains("notice        cannot write `/x/y.sfs`: nope"),
            "{r}"
        );
        // A clean session says nothing extra.
        let clean = play_report(&loop_::Summary::default());
        assert!(!clean.contains("notice"), "{clean}");
    }
}
