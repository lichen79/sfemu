//! Run a ROM set on the board it came from, and report what that board saw.
//!
//! ```text
//! sfemu <path-to-rom-set> [frames] [--game <name>] [--ppm <path>]
//! sfemu <path-to-rom-set> --play [--game <name>] [--state <path>]
//! sfemu --demo [frames] [--play] [--ppm <path>] [--state <path>]
//! ```
//!
//! `<path-to-rom-set>` is a MAME-format zip or a directory of loose files that
//! **you supply**. This program contains no ROM data and no way to obtain any.
//!
//! `--demo` needs no files: it runs a CPS-1 image this workspace generates from
//! nothing (`crates/testrom`). That is the whole reason it exists — the emulator
//! must be runnable by someone who has no ROM set, and a black window is not a
//! demonstration that anything works.
//!
//! `--game` picks the hardware: `sf2` is Street Fighter II on CPS-1 and is the
//! default, `sf1` is Street Fighter on its own 1987 board. It is a choice and not a
//! guess — a set of files does not say what machine it came out of.
//!
//! # Why there is a report as well as a window
//!
//! `--play` opens a window; without it, the program runs a fixed number of frames and
//! prints counters instead. Both exist because a black window is indistinguishable
//! from a boot that hangs on the first instruction, whereas a count of vblanks,
//! acknowledges and video-register writes says which — and it says it in a form CI, a
//! bisect and a commit message can hold. `--ppm` writes the last frame out as a file,
//! so a headless run still produces a picture to look at.

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
    "usage: sfemu <path-to-rom-set> [frames] [--game <name>] [--ppm <path>]\n\
     \x20      sfemu <path-to-rom-set> --play [--game <name>] [--state <path>]\n\
     \x20      sfemu --demo [frames] [--play] [--ppm <path>] [--state <path>]\n\
     \n\
     <name> is `sf2` (Street Fighter II on CPS-1, the default) or `sf1`\n\
     (Street Fighter, on its own 1987 board). The board is not guessed from\n\
     the path: a set of files does not say what hardware it came from.\n\
     \n\
     `--demo` runs a CPS-1 image this program generates itself — scrolling\n\
     tilemaps, a sprite on a path, a frame counter and FM music — and needs\n\
     no files and no path. It is a homebrew demo and not any Capcom game.\n\
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
    /// Where the ROM image comes from.
    source: Source,
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

/// Where the ROM image comes from.
///
/// An enum and not a `path: String` beside a `demo: bool`, because those two fields
/// have two meaningless combinations — a demo with a path, and a path-less run that
/// is not the demo — and every one of them would need an error branch here and a
/// decision at each of the four places downstream that read a path. The enum makes
/// both unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// A ROM set the user supplies, and the name that says how to read it.
    ///
    /// `game` is a `String` and not a [`machine::BoardKind`] because it is also the
    /// name [`romset::games::by_name`] resolves — one name picking both the board and
    /// the files it expects is what keeps them from being chosen apart.
    Set { path: String, game: String },
    /// The demo image this workspace generates. No path, and CPS-1 by construction.
    Demo,
}

/// The stem the demo's state and screenshot files are named from.
///
/// A relative name, so they land in the working directory: there is no ROM set to
/// sit beside, and writing into a temporary directory would leave a user's F12
/// screenshot somewhere they will not find it.
const DEMO_STEM: &str = "sfemu-demo";

impl Source {
    /// The board this source runs on.
    fn board(&self) -> machine::BoardKind {
        match self {
            // `expect`: `parse_args` rejects a name with no board before an `Args`
            // exists, so this is unreachable rather than unhandled.
            Self::Set { game, .. } => {
                board_for(game).expect("parse_args rejects a name with no board")
            }
            // Not a lookup: the demo is a CPS-1 image, and [`demo_machine`] builds
            // one directly. A name here would be a second, silently divergent
            // statement of the same fact.
            Self::Demo => machine::BoardKind::Cps1,
        }
    }

    /// The path the state and screenshot files are derived from.
    fn stem(&self) -> &str {
        match self {
            Self::Set { path, .. } => path,
            Self::Demo => DEMO_STEM,
        }
    }
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

/// The board a game name names.
///
/// Separate from [`romset::games::by_name`] and checked against it by a test: the
/// name selects both the ROM spec and the hardware, and a crossed pair — SF2's files
/// on SF1's bus — is a machine whose every symptom is downstream of a choice made
/// here.
fn board_for(game: &str) -> Option<machine::BoardKind> {
    match game {
        // Both SF2 revisions are CPS-1 boards. They are *not* the same board:
        // their CPS-B parts differ, which [`cps_b_config_for`] resolves. This
        // enum names the bus and the video subsystem, which they do share.
        "sf2" | "sf2eb" => Some(machine::BoardKind::Cps1),
        "sf1" => Some(machine::BoardKind::Sf1),
        _ => None,
    }
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
    let mut demo = false;
    let mut state = None;
    let mut game = None;
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        if a == "--play" {
            play = true;
        } else if a == "--demo" {
            demo = true;
        } else if a == "--state" {
            let p = args
                .next()
                .ok_or_else(|| Fault::Failed("`--state` needs a path".to_string()))?;
            if state.replace(p).is_some() {
                return Err(Fault::Failed("`--state` given twice".to_string()));
            }
        } else if a == "--game" {
            let g = args
                .next()
                .ok_or_else(|| Fault::Failed("`--game` needs a name".to_string()))?;
            if game.replace(g).is_some() {
                return Err(Fault::Failed("`--game` given twice".to_string()));
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
    // Under `--demo` there is no path to give, so the first positional is the frame
    // count.
    let path = if demo {
        None
    } else {
        Some(rest.next().ok_or(Fault::Usage)?)
    };
    // Parsed strictly. `unwrap_or(60)` on a typo — `6O` with a letter O — would
    // run a different number of frames than asked for and say nothing, and the
    // whole point of this program is that its numbers mean what they say.
    let frames: u64 = match rest.next() {
        None => 60,
        Some(s) => s.parse().map_err(|_| {
            // A path handed to `--demo` lands in this slot, and the message says so
            // rather than only that it is not a number: the demo opens no files, so
            // `sfemu --demo mysf2.zip` is a user expecting their set to be read.
            Fault::Failed(if demo {
                format!("`--demo` takes no ROM set path, and `{s}` is not a frame count")
            } else {
                format!("`{s}` is not a frame count")
            })
        })?,
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
    let source = match path {
        Some(path) => {
            // `sf2` and not "whatever the path looks like": the board is a stated
            // choice. See [`usage`], and the spec's "the selection is explicit, not
            // inferred from a filename".
            let game = game.unwrap_or_else(|| "sf2".to_string());
            // Rejected here rather than at load time. `romset::load` would never be
            // reached with an unknown name — there is no spec to hand it — and a user
            // who typed a name this program does not know needs the names it does.
            if board_for(&game).is_none() {
                return Err(Fault::Failed(format!(
                    "`{game}` is not a game this program knows: try `sf1` or `sf2`"
                )));
            }
            Source::Set { path, game }
        }
        // `--game` with `--demo` is an error and not an ignored flag, on the same
        // terms as `--state` without `--play`: the demo is one specific image, so
        // `--demo --game sf1` would run CPS-1 code on a request for SF1 hardware and
        // say nothing about having refused.
        None if game.is_some() => {
            return Err(Fault::Failed(
                "`--game` and `--demo` cannot both be given: the demo is its own image".to_string(),
            ));
        }
        None => Source::Demo,
    };
    let state = state.map_or_else(|| default_state_path(source.stem()), PathBuf::from);
    Ok(Args {
        source,
        frames,
        ppm,
        play,
        state,
    })
}

/// The CPS-B configuration a CPS-1 game name selects.
///
/// A third consequence of the name, alongside the ROM spec and the board kind, and
/// the one with the least forgiving failure mode. CPS-B is a family of parts, and
/// the row a game needs is not derivable from its hardware being "CPS-1": `sf2`
/// has `CPS_B_11` and `sf2eb` has `CPS_B_17`, which agree on no register at all.
///
/// A wrong row here does not produce a crash or a garbled screen. The program
/// reads the ID register, finds the wrong value, and branches to an idle loop —
/// so the machine boots, runs, takes every interrupt, and draws nothing, with no
/// unmapped access and no fault to report. That is why an unknown name is an
/// error: there is no config that is safe to fall back to.
///
/// # Errors
///
/// [`Fault::Failed`] if the name has no row.
/// The table itself is [`machine::BoardConfig::for_game`], in the crate that owns
/// the hardware facts; this is where the `None` becomes a message. A name reaching
/// here without a row means [`board_for`] called it CPS-1 and `machine` has no
/// registers for it, which is a gap in this workspace and not in the user's files.
fn cps_b_config_for(game: &str) -> Result<machine::BoardConfig, Fault> {
    machine::BoardConfig::for_game(game).ok_or_else(|| {
        Fault::Failed(format!(
            "internal: `{game}` is a CPS-1 game with no CPS-B configuration. \
             Add its row to `machine::BoardConfig::for_game` — there is no \
             default, because a wrong row boots and draws nothing."
        ))
    })
}

/// SF2 on CPS-1, from a loaded set.
///
/// The four region lookups are `expect`-free and `ok_or_else`-loud for the reason
/// each message states: every one of these regions is in `games::SF2`, so a `None`
/// is a bug in `romset::games` rather than anything about the user's files, and
/// defaulting to an empty region turns each into a different silent wrong answer.
///
/// `game` selects the CPS-B row through [`cps_b_config_for`]; see there for why it
/// cannot be hardcoded.
fn build_cps1(game: &str, set: &romset::RomSet) -> Result<machine::Cps1, Fault> {
    let prog = set.region("maincpu").ok_or_else(|| {
        Fault::Failed("internal: the sf2 spec has no `maincpu` region".to_string())
    })?;
    let gfx = set
        .region("gfx")
        .ok_or_else(|| Fault::Failed("internal: the sf2 spec has no `gfx` region".to_string()))?
        .to_vec();
    // An absent sound region is not silence, it is 0xFF on every fetch, which is
    // `RST 38h` — a Z80 spinning in a deterministic loop that racks up a *larger*
    // fetch count than a real driver. The debugger's sound panel would show that loop
    // and look like a working panel on a broken driver.
    let audiocpu = set
        .region("audiocpu")
        .ok_or_else(|| {
            Fault::Failed("internal: the sf2 spec has no `audiocpu` region".to_string())
        })?
        .to_vec();
    // The ADPCM samples, on the same terms again. An absent one *is* silence — every
    // phrase header reads `start == stop == 0`, which the chip refuses — but silence
    // with no explanation, and "no sound effects, music fine" is the hardest symptom
    // here to attribute to a missing region.
    let okirom = set
        .region("oki")
        .ok_or_else(|| Fault::Failed("internal: the sf2 spec has no `oki` region".to_string()))?
        .to_vec();
    let mut m = machine::Cps1::with_sound(
        prog,
        gfx,
        audiocpu,
        okirom,
        cps_b_config_for(game)?,
        machine::Timing::cps1_10mhz(),
    );
    m.reset();
    Ok(m)
}

/// SF1 on its own board, from a loaded set.
///
/// Eight regions against CPS-1's four, and the messages are as loud for the same
/// reason. Two of the eight deserve their own note:
///
/// - `tilerom` is not graphics, it is the **tile maps** — SF1's background and
///   foreground layouts live in ROM rather than in guest RAM, which is the single
///   biggest way this board differs from CPS-1. An absent one draws a screen of
///   tile 0 in colour 0, which looks exactly like a renderer that is not working.
/// - `audio2` is the second sound CPU, the one that drives the two MSM5205s. An
///   absent one is 0xFF on every fetch, which is `RST 38h`: see [`build_cps1`].
///
/// ⚠️ **No `proms` region.** `games::SF1` deliberately has none — SF1's PROMs are
/// priority and timing logic this emulator does not model — so there is nothing to
/// look up and nothing to fail loudly about.
fn build_sf1(set: &romset::RomSet) -> Result<machine::Sf1, Fault> {
    let need = |name: &'static str| -> Result<Vec<u8>, Fault> {
        set.region(name)
            .ok_or_else(|| Fault::Failed(format!("internal: the sf1 spec has no `{name}` region")))
            .map(<[u8]>::to_vec)
    };
    let prog = need("maincpu")?;
    let video = machine::video::sf1::Sf1Video::new(
        need("gfx1")?,
        need("gfx2")?,
        need("gfx3")?,
        need("gfx4")?,
        need("tilerom")?,
    );
    let mut m = machine::Sf1::new(&prog, video, need("audiocpu")?, need("audio2")?);
    m.reset();
    Ok(m)
}

/// The machine a game name and a loaded set make.
///
/// One name, two consequences, chosen here together: [`romset::games::by_name`] says
/// which files and [`board_for`] says which hardware.
/// `each_game_name_selects_its_own_board_and_its_own_rom_spec` is what holds them to
/// the same name.
///
/// Split out of [`run`] rather than written inline, so a test can hand it a
/// [`romset::RomSet`] it builds itself: the struct is a map of region name to bytes,
/// which this crate can fill with synthetic data and does. Inline, the fork would be
/// reachable only from a path that needs a real ROM set, which is to say from no test
/// at all.
///
/// # Errors
///
/// Whatever [`build_cps1`] or [`build_sf1`] returns: a region the spec names and the
/// set has not got.
fn build_machine(game: &str, set: &romset::RomSet) -> Result<machine::Machine, Fault> {
    Ok(
        match board_for(game).expect("parse_args rejects a name with no board") {
            machine::BoardKind::Cps1 => machine::Machine::Cps1(Box::new(build_cps1(game, set)?)),
            machine::BoardKind::Sf1 => machine::Machine::Sf1(Box::new(build_sf1(set)?)),
        },
    )
}

/// The report for a finished headless run.
///
/// The frame summary is the one thing that cannot come through [`machine::Machine`]:
/// it counts pens against a board-specific palette. Split out with the board line for
/// the same reason [`build_machine`] is — a test builds the machine and reads the
/// string, where inline both would need a ROM set.
fn summary(m: &machine::Machine) -> String {
    let frame = match m {
        machine::Machine::Cps1(c) => Frame::of(&c.video),
        machine::Machine::Sf1(f) => Frame::of_sf1(&f.video),
    };
    let v = m.cpu_view();
    report(m.board(), v.trace, v.total_cycles, Cpu::of(&v), frame)
}

/// The PPM bytes for whichever board this is.
///
/// The second fork that cannot go through `Machine`: [`ppm`] reads
/// [`video::compose::Video`] and [`ppm_sf1`] reads [`video::sf1::Sf1Video`], and the
/// two palettes reach the DAC by different rules.
fn screenshot(m: &machine::Machine) -> Vec<u8> {
    match m {
        machine::Machine::Cps1(c) => ppm(&c.video),
        machine::Machine::Sf1(f) => ppm_sf1(&f.video),
    }
}

/// The demo machine: a CPS-1 built from the image [`testrom::demo`] generates.
///
/// Goes through [`romset::RomSet`] and [`build_machine`] rather than calling
/// [`machine::Cps1::with_sound`] directly, so the demo boots down the *same* path a
/// real set does — the region names, the four lookups, the board config and the
/// timing. A second construction site would let the two drift, and the one that
/// nobody can test is the one that matters.
///
/// # Errors
///
/// Whatever [`build_cps1`] returns. `testrom::demo::build` answers all four regions
/// CPS-1 needs, so a failure here is a mismatch between the generator's names and
/// `build_cps1`'s — which is exactly what
/// `the_demo_image_names_the_regions_the_cps1_builder_asks_for` pins.
fn demo_machine() -> Result<machine::Machine, Fault> {
    let set = romset::RomSet {
        regions: testrom::demo::build()
            .into_iter()
            .map(|(name, bytes)| (name.to_string(), bytes))
            .collect(),
    };
    build_machine("sf2", &set)
}

/// Loads the set, runs `frames` frames, and returns the report.
fn run(args: Vec<String>) -> Result<String, Fault> {
    let args = parse_args(args)?;

    let mut machine = match &args.source {
        Source::Set { path, game } => {
            let spec =
                romset::games::by_name(game).expect("parse_args rejects a name that has no spec");
            let set = romset::load(spec, std::path::Path::new(path))
                .map_err(|e| Fault::Failed(e.to_string()))?;
            build_machine(game, &set)?
        }
        Source::Demo => demo_machine()?,
    };

    if args.play {
        // The window is opened *after* the ROM set loads, so a bad path reports the
        // load error rather than flashing a window onto a machine that never booted.
        let mut win = display::Window::open("sfemu").map_err(Fault::Failed)?;
        let mut sink = open_audio();
        let opts = loop_opts(&args);
        let summary = loop_::run(&mut machine, &mut win, sink.as_mut(), &opts);
        return Ok(play_report(&summary));
    }

    for _ in 0..args.frames {
        machine.run_frame();
    }
    machine.render();

    if let Some(path) = &args.ppm {
        std::fs::write(path, screenshot(&machine))
            .map_err(|e| Fault::Failed(format!("cannot write `{path}`: {e}")))?;
    }

    Ok(summary(&machine))
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

/// The two paths and the state tag the loop is given.
///
/// Split out of [`run`] for the same reason [`parse_args`] is: everything else in
/// `run` needs a ROM set, so a `LoopOpts` built inline could only be checked by a
/// test that owns one. And it is worth checking — the struct has two same-typed path
/// fields, so swapping them compiles and the symptom is F5 overwriting your
/// screenshot; and the board tag is the second of two choices `main` makes about the
/// board, which [`loop_::run`]'s assertion checks against the first.
fn loop_opts(args: &Args) -> loop_::LoopOpts {
    loop_::LoopOpts {
        state_path: args.state.clone(),
        shot_path: default_shot_path(args.source.stem()),
        board: loop_::state_tag(args.source.board()),
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

/// An SF1 frame as a binary PPM.
///
/// The same `P6\n384 224\n255\n` header as [`ppm`] — a screenshot of either board is
/// the same file format, and a viewer must not have to know which machine wrote it.
///
/// Separate from `ppm` and not a parameter, because there is no shared type to take:
/// `Video::rgb` returns an assembled `Vec<u8>` and `Sf1Video` publishes a per-pen
/// `rgb` instead. Assembling the body here rather than adding a frame-wide `rgb` to
/// `Sf1Video` keeps the allocation in the crate that is about to write a file, which
/// is the crate that already owns a filesystem.
fn ppm_sf1(v: &video::sf1::Sf1Video) -> Vec<u8> {
    let mut out = format!("P6\n{} {}\n255\n", video::WIDTH, video::HEIGHT).into_bytes();
    assert_eq!(
        v.fb.pens.len(),
        video::WIDTH * video::HEIGHT,
        "one pen per pixel of the visible frame"
    );
    for &pen in v.fb.pens.iter() {
        // `rgb` and not a palette index: a never-rendered frame holds CPS-1's
        // `BACKGROUND_PEN`, which is past SF1's 1,024 entries, and `rgb` answers
        // black for it where an index would be out of bounds.
        out.extend_from_slice(&v.rgb(pen));
    }
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
    /// From the shared view, so one implementation serves both boards.
    ///
    /// [`machine::CpuView::cpu`] is a `&M68k` on either board — the same core, the
    /// same three fields — which is the whole argument for `CpuView` existing. A
    /// `match` here would be a match on a difference that does not exist.
    fn of(v: &machine::CpuView<'_>) -> Self {
        Self {
            pc: v.cpu.pc,
            halted: v.cpu.halted,
            stopped: v.cpu.stopped,
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

    /// The same two facts about an SF1 frame.
    ///
    /// Separate from [`Frame::of`] and not a parameter, for the reason [`ppm_sf1`] is
    /// separate from [`ppm`]: there is no shared type to take, and the three things
    /// this has to know are all different.
    ///
    /// # The three differences, and the trap in the first
    ///
    /// 1. **The blank pen is 0**, from `Sf1Video::render`'s `self.fb.pens.fill(0)`,
    ///    where CPS-1's is `BACKGROUND_PEN` (0xBFF). ⚠️ But `Framebuffer::new` fills
    ///    with 0xBFF on **both** boards, so a never-rendered SF1 frame is 86,016 pens
    ///    of 0xBFF — and a bare `pen != 0` test would report a machine that never drew
    ///    a frame as having drawn every pixel of one. Hence the upper bound as well as
    ///    the lower: a pen is drawn when it is neither the blank pen nor past the
    ///    palette.
    /// 2. **1,024 entries**, not 3,072.
    /// 3. **Four 256-entry blocks**, one per [`video::sf1::Plane`]'s `colour_base`, not
    ///    six 512-entry `PAGE_ENTRIES` pages. Nothing selects between them, which is
    ///    why [`report`] calls them colour blocks rather than pages.
    ///
    /// ⚠️ `drawn` is a **lower bound on SF1**, and knowingly so. BG has no transparent
    /// pen (`MapKind::Bg`'s `transparent_pen()` is `None`), so a rendered BG covers the
    /// frame — and any of its pixels that legitimately resolve to pen 0 are counted as
    /// not drawn. The number is still the one worth printing: what it distinguishes is
    /// nothing-drew from something-drew, and nothing-drew is exactly 0.
    fn of_sf1(v: &video::sf1::Sf1Video) -> Self {
        const BLOCK: usize = 256;
        let mut blocks = [false; video::sf1::palette::ENTRIES / BLOCK];
        let mut drawn = 0;
        for &pen in v.fb.pens.iter() {
            let pen = usize::from(pen);
            if pen == 0 || pen >= video::sf1::palette::ENTRIES {
                continue;
            }
            drawn += 1;
            // `get_mut` for `Frame::of`'s reason: the report must not be the thing
            // that panics on a pen it did not expect. The bound above makes this
            // unreachable, and it stays because the bound is one edit from moving.
            if let Some(b) = blocks.get_mut(pen / BLOCK) {
                *b = true;
            }
        }
        Self {
            drawn,
            pages: blocks.iter().filter(|&&b| b).count(),
        }
    }
}

/// Formats a run's trace and its last frame.
///
/// Separate from [`run`] so it can be tested against a machine this crate builds
/// itself — the loader path needs a ROM set and therefore cannot be tested here at
/// all, but the report is pure and gets literals like everything else.
///
fn report(board: machine::BoardKind, t: &Trace, cycles: u64, cpu: Cpu, frame: Frame) -> String {
    let mut s = String::new();
    let line = |s: &mut String, k: &str, v: String| {
        s.push_str(&format!("{k:<14}{v}\n"));
    };
    // First, because two later decisions are consequences of it: three counters are
    // omitted on SF1 and the framebuffer line's noun changes. Without this line a
    // reader cannot tell a board that has no CPS-A registers from a report that
    // stopped printing them.
    line(
        &mut s,
        "board",
        match board {
            machine::BoardKind::Cps1 => "CPS-1",
            machine::BoardKind::Sf1 => "SF1",
        }
        .to_string(),
    );
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
            "{} of {} pixels drawn, {} {}",
            frame.drawn,
            video::WIDTH * video::HEIGHT,
            frame.pages,
            // SF1's 1,024 entries divide into four `colour_base` blocks that nothing
            // selects between; CPS-1's 3,072 into six pages that `palette_control`
            // does. Calling both "pages" names a register SF1 has not got.
            match board {
                machine::BoardKind::Cps1 => "palette page(s)",
                machine::BoardKind::Sf1 => "colour block(s)",
            }
        ),
    );
    // The three chips only CPS-1 has. See this function's `board` line: on SF1 these
    // are fields of the shared `Trace` that nothing writes, and `cps-a writes  0`
    // reads as a finding about a driver rather than a fact about a board.
    if board == machine::BoardKind::Cps1 {
        line(&mut s, "cps-a writes", t.cps_a_writes.to_string());
        line(&mut s, "cps-b writes", t.cps_b_writes.to_string());
        line(&mut s, "gfxram writes", t.gfxram_writes.to_string());
    }
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

    /// The CPS-1 board inside a test's machine.
    ///
    /// These tests are about this crate's report and its PPM, not about the enum:
    /// matching on `Machine` at each of a dozen field reads would make every one of
    /// them a test of `Machine`'s shape. `panic` because every fixture here builds
    /// the CPS-1 arm, and a test that wired up the wrong one should fail loudly on
    /// the first read rather than skip its assertions.
    fn cps1(m: &machine::Machine) -> &machine::Cps1 {
        match m {
            machine::Machine::Cps1(c) => c,
            machine::Machine::Sf1(_) => panic!("this fixture builds a CPS-1"),
        }
    }

    fn cps1_mut(m: &mut machine::Machine) -> &mut machine::Cps1 {
        match m {
            machine::Machine::Cps1(c) => c,
            machine::Machine::Sf1(_) => panic!("this fixture builds a CPS-1"),
        }
    }

    /// A machine running a program this test writes, so the report has real
    /// numbers in it without a ROM set.
    ///
    /// Returns a `Machine` and not a `Cps1` because `Cpu::of` takes a `CpuView` and
    /// [`machine::Machine::cpu_view`] is the only thing that makes one — Task 16
    /// deliberately did **not** add a `cpu_view` to `Cps1`. The board-specific reads
    /// below go through `cps1`, like the loop's tests.
    ///
    /// ```text
    /// 1000  33FC 0040 0080 010C   move.w #$0040,$80010C   CPS-A
    /// 1008  33FC FFFF 0081 0000   move.w #$FFFF,$810000   unmapped
    /// 1010  4E72 2700             stop #$2700
    /// ```
    fn one_frame() -> machine::Machine {
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
        machine::Machine::Cps1(Box::new(m))
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
            report(
                machine::BoardKind::Cps1,
                &cps1(&m).board.trace,
                cps1(&m).total_cycles,
                Cpu::of(&m.cpu_view()),
                BLANK,
            ),
            format!(
                "board         CPS-1\n\
                 frames        1\n\
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
                cps1(&m).total_cycles
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
            (167_680..167_680 + 16).contains(&cps1(&m).total_cycles),
            "got {}",
            cps1(&m).total_cycles
        );
        let r = report(
            machine::BoardKind::Cps1,
            &cps1(&m).board.trace,
            cps1(&m).total_cycles,
            Cpu::of(&m.cpu_view()),
            BLANK,
        );
        // The exact total depends on where the `stop` lands relative to the budget,
        // so the printed digits are checked against the machine's own value — but
        // the *bound* above is the hand-written 640 × 262, so this pair pins both
        // that the number is right and that it reaches the page.
        assert!(
            r.contains(&format!("cycles        {}\n", cps1(&m).total_cycles)),
            "the cycle line is missing or reformatted: {r}"
        );
    }

    /// A halted CPU is called out, because that is the one state that means our
    /// memory map is wrong rather than the game's code being unfinished.
    #[test]
    fn a_halted_cpu_is_reported_as_a_double_bus_fault() {
        let mut m = one_frame();
        cps1_mut(&mut m).cpu.halted = true;
        let r = report(
            machine::BoardKind::Cps1,
            &cps1(&m).board.trace,
            cps1(&m).total_cycles,
            Cpu::of(&m.cpu_view()),
            BLANK,
        );
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
        let r = report(machine::BoardKind::Cps1, &t, 0, IDLE, BLANK);
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
        let r = report(machine::BoardKind::Cps1, &t, 0, IDLE, BLANK);
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
        let r = report(machine::BoardKind::Cps1, &t, 0, IDLE, BLANK);
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
                source: Source::Set {
                    path: "/some/sf2.zip".to_string(),
                    game: "sf2".to_string(),
                },
                frames: 60,
                ppm: None,
                play: false,
                state: PathBuf::from("/some/sf2.sfs"),
            })
        );
        assert_eq!(
            args(vec!["/some/sf2.zip", "7"]),
            Some(Args {
                source: Source::Set {
                    path: "/some/sf2.zip".to_string(),
                    game: "sf2".to_string(),
                },
                frames: 7,
                ppm: None,
                play: false,
                state: PathBuf::from("/some/sf2.sfs"),
            }),
            "and an explicit count is taken as given"
        );
        // And `--demo`'s first positional is the frame count, because it has no path
        // to occupy the slot. A parser that kept a path slot under `--demo` would read
        // `7` as the path and then run the default 60 frames.
        assert_eq!(
            args(vec!["--demo", "7"]),
            Some(Args {
                source: Source::Demo,
                frames: 7,
                ppm: None,
                play: false,
                state: PathBuf::from("sfemu-demo.sfs"),
            })
        );
        assert_eq!(
            args(vec!["--demo"]),
            Some(Args {
                source: Source::Demo,
                frames: 60,
                ppm: None,
                play: false,
                state: PathBuf::from("sfemu-demo.sfs"),
            }),
            "and the demo takes the same default"
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
            source: Source::Set {
                path: "/some/sf2.zip".to_string(),
                game: "sf2".to_string(),
            },
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
    fn a_drawn_frame() -> machine::Machine {
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
        machine::Machine::Cps1(Box::new(m))
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
        let bytes = ppm(&cps1(&m).video);
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

    /// An `Sf1Video` with one palette entry set and one frame rendered.
    ///
    /// No ROM: the four graphics regions are empty, so `render` clears every pen to 0
    /// and the whole frame is entry 0. That is enough to test the format, which is
    /// what this tests.
    fn an_sf1_frame() -> video::sf1::Sf1Video {
        let mut v =
            video::sf1::Sf1Video::new(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut palette = vec![0u16; video::sf1::palette::ENTRIES];
        palette[0] = 0x0135;
        let videoram = vec![0u16; 0x800];
        let objectram = vec![0u16; 0x1000];
        v.render(&videoram, &objectram, &palette, 0, 0, 0);
        v
    }

    /// SF1's screenshot is the same file format as CPS-1's.
    ///
    /// Every number a literal, for the reason
    /// `the_ppm_header_is_a_binary_p6_of_the_right_size` gives: a header interpolated
    /// from the constants the body is sized from agrees with itself whatever they are.
    #[test]
    fn the_sf1_ppm_header_is_a_binary_p6_of_the_right_size() {
        let v = an_sf1_frame();
        let bytes = ppm_sf1(&v);
        let header = b"P6\n384 224\n255\n";
        assert_eq!(&bytes[..header.len()], header, "the exact header");
        assert_eq!(
            bytes.len() - header.len(),
            258_048,
            "384 * 224 * 3 bytes of body"
        );
        assert_eq!(bytes.len(), 258_063, "and nothing after it");
    }

    /// And its colours are SF1's, not CPS-1's.
    #[test]
    fn the_sf1_screenshot_uses_sf1s_dac_rule_and_not_cps1s() {
        let v = an_sf1_frame();
        let bytes = ppm_sf1(&v);
        let header = 15;
        // Entry 0x0135 through SF1's `(n << 4) | n` is (0x11, 0x33, 0x55). CPS-1's
        // converter would give (0x08, 0x18, 0x28) and the screenshot would be dark.
        assert_eq!(&bytes[header..header + 3], &[0x11, 0x33, 0x55]);
        assert!(bytes[header..].chunks(3).all(|p| p == [0x11, 0x33, 0x55]));
    }

    /// A never-rendered frame screenshots black rather than panicking.
    #[test]
    fn a_never_rendered_sf1_screenshots_black_rather_than_panicking() {
        // `Framebuffer::new` fills every pen with CPS-1's 0xBFF, past SF1's 1,024
        // entries. `Sf1Video::rgb` answers black for it; indexing would not.
        let v =
            video::sf1::Sf1Video::new(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let bytes = ppm_sf1(&v);
        assert_eq!(bytes.len(), 258_063);
        assert!(bytes[15..].iter().all(|&b| b == 0));
    }

    /// The report names the framebuffer, with the drawn count and the page count.
    ///
    /// This is the line the real-ROM check grows into, so it is asserted against a
    /// frame whose content is known exactly: one 16×16 sprite is 256 pixels out of
    /// 86,016, from one palette page.
    #[test]
    fn the_report_names_the_framebuffer() {
        let m = a_drawn_frame();
        let f = Frame::of(&cps1(&m).video);
        assert_eq!(f.drawn, 256, "one 16x16 sprite");
        assert_eq!(f.pages, 1, "pen 0x3A is in page 0");
        let r = report(
            machine::BoardKind::Cps1,
            &cps1(&m).board.trace,
            cps1(&m).total_cycles,
            Cpu::of(&m.cpu_view()),
            f,
        );
        assert!(
            r.contains("framebuffer   256 of 86016 pixels drawn, 1 palette page(s)\n"),
            "384 * 224 = 86016: {r}"
        );

        // A blank frame says so rather than omitting the line, which is the state a
        // stalled boot leaves and the one this line has to be able to report.
        let r = report(machine::BoardKind::Cps1, &Trace::default(), 0, IDLE, BLANK);
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
        assert_eq!(Frame::of(&cps1(&m).video).pages, 1, "the premise");

        // Move one of the sprite's pixels into page 1 by hand. Reaching into the
        // framebuffer is legitimate here: the subject is the *counting*, and a
        // second page from a second sprite would need a second colour scheme and
        // tell us less clearly.
        cps1_mut(&mut m).video.fb.pens[0] = 0x200;
        assert_eq!(
            Frame::of(&cps1(&m).video).pages,
            2,
            "0x200 is the start of page 1"
        );
        assert_eq!(
            Frame::of(&cps1(&m).video).drawn,
            256,
            "still 256 drawn pixels"
        );

        cps1_mut(&mut m).video.fb.pens[1] = 0xBFE;
        assert_eq!(
            Frame::of(&cps1(&m).video).pages,
            3,
            "0xBFE is in page 5, the last"
        );
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
                source: Source::Set {
                    path: "/some/sf2.zip".to_string(),
                    game: "sf2".to_string(),
                },
                frames: 60,
                ppm: None,
                play: true,
                state: PathBuf::from("/some/sf2.sfs"),
            }),
        );
        assert_eq!(
            args(vec!["/some/sf2.zip", "--play", "--state", "/tmp/mine.sfs"]).ok(),
            Some(Args {
                source: Source::Set {
                    path: "/some/sf2.zip".to_string(),
                    game: "sf2".to_string(),
                },
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
            source: Source::Set {
                path: "/a/b/sf2.zip".to_string(),
                game: "sf2".to_string(),
            },
            frames: 60,
            ppm: None,
            play: true,
            state: PathBuf::from("/tmp/mine.sfs"),
        };
        let o = loop_opts(&args);
        assert_eq!(o.state_path, PathBuf::from("/tmp/mine.sfs"));
        assert_eq!(o.shot_path, PathBuf::from("/a/b/sf2.ppm"));

        // The demo has no ROM set to sit beside, so both files come off its own
        // stem — and they still differ by extension, which is what makes a swap of
        // the two `PathBuf` fields visible here too.
        let demo = Args {
            source: Source::Demo,
            frames: 60,
            ppm: None,
            play: true,
            state: PathBuf::from("sfemu-demo.sfs"),
        };
        let o = loop_opts(&demo);
        assert_eq!(o.state_path, PathBuf::from("sfemu-demo.sfs"));
        assert_eq!(o.shot_path, PathBuf::from("sfemu-demo.ppm"));
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

    /// A CPU summary comes from the shared view, so one implementation serves both.
    #[test]
    fn the_cpu_summary_comes_from_the_shared_view() {
        let m = one_frame();
        let c = Cpu::of(&m.cpu_view());
        // Literals: `stop #$2700` is at 0x1010 and `STOP` leaves the PC past its own
        // extension word.
        assert_eq!(c.pc, 0x0000_1014);
        assert!(!c.halted);
        assert!(c.stopped, "the program ends in `stop`");
    }

    /// An SF1 report names the board and prints no CPS-A, CPS-B or gfxram line.
    ///
    /// Both halves matter. The absent lines are the ruling; the `board` line is what
    /// makes their absence mean "this board has no such chip" rather than "this
    /// report lost three counters".
    #[test]
    fn an_sf1_report_omits_the_chips_sf1_does_not_have() {
        // A trace with all eleven counters non-zero, including the three CPS ones —
        // which SF1's board never writes, but this test must not depend on that to
        // prove the *report* drops them. A zero would pass against a report that
        // printed the line.
        //
        // A struct literal and not eight assignments to a `default()`: clippy's
        // `field_reassign_with_default` rejects the latter, and the values are the
        // same either way.
        let t = Trace {
            frames: 3,
            vblanks: 3,
            acks: 2,
            cps_a_writes: 11,
            cps_b_writes: 12,
            gfxram_writes: 13,
            sound_latch_writes: 4,
            rom_writes: 5,
            ..Trace::default()
        };
        let r = report(machine::BoardKind::Sf1, &t, 100, IDLE, BLANK);
        assert!(
            r.starts_with("board         SF1\n"),
            "names the board first: {r}"
        );
        for absent in ["cps-a", "cps-b", "gfxram", "11", "12", "13"] {
            assert!(
                !r.contains(absent),
                "`{absent}` names a chip SF1 does not have: {r}"
            );
        }
        // And every counter both boards do have is still there.
        assert!(r.contains("frames        3\n"), "{r}");
        assert!(r.contains("vblanks       3  acks 2\n"), "{r}");
        assert!(r.contains("sound latch   4\n"), "{r}");
        assert!(r.contains("rom writes    5\n"), "{r}");
        assert!(r.contains("unmapped      0 reads, 0 writes\n"), "{r}");
        // The noun follows the board.
        assert!(
            r.contains("framebuffer   0 of 86016 pixels drawn, 0 colour block(s)\n"),
            "SF1's palette has no pages: {r}"
        );

        // The same trace on CPS-1 prints all three, so the assertions above are
        // about the board and not about the counters being unreachable.
        let r = report(machine::BoardKind::Cps1, &t, 100, IDLE, BLANK);
        assert!(r.starts_with("board         CPS-1\n"), "{r}");
        assert!(r.contains("cps-a writes  11\n"), "{r}");
        assert!(r.contains("cps-b writes  12\n"), "{r}");
        assert!(r.contains("gfxram writes 13\n"), "{r}");
        assert!(
            r.contains("framebuffer   0 of 86016 pixels drawn, 0 palette page(s)\n"),
            "{r}"
        );
    }

    /// SF1's frame summary counts its own blank pen and its own palette division.
    ///
    /// Three separate traps, one per assertion block:
    ///
    /// 1. A **never-rendered** SF1 frame is not zeroes. `Framebuffer::new` fills every
    ///    pen with CPS-1's `BACKGROUND_PEN` (0xBFF), which is past SF1's 1,024
    ///    entries — so a `pen != 0` test would report a stalled boot as **86,016 of
    ///    86,016 pixels drawn**, which is the exact opposite of the truth and the one
    ///    reading this line exists to make impossible.
    /// 2. A **rendered** frame's blank pen is 0, from `render`'s `self.fb.pens.fill(0)`.
    /// 3. The division is four 256-entry blocks, one per `Plane::colour_base`, not six
    ///    512-entry pages.
    #[test]
    fn an_sf1_frame_summary_uses_sf1s_blank_pen_and_palette_division() {
        let mut v = machine::video::sf1::Sf1Video::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        // 1. Never rendered: every pen is 0xBFF, past 1,024.
        assert_eq!(
            Frame::of_sf1(&v),
            Frame { drawn: 0, pages: 0 },
            "a frame that was never rendered drew nothing"
        );

        // 2. Rendered and blank. `render` with a zero `active` byte draws no layer and
        // fills with 0.
        v.render(&[], &[], &[], 0, 0, 0);
        assert_eq!(Frame::of_sf1(&v), Frame { drawn: 0, pages: 0 });

        // 3. Four blocks at 256 apart, and the fourth is the last.
        v.fb.pens[0] = 1;
        assert_eq!(Frame::of_sf1(&v), Frame { drawn: 1, pages: 1 });
        v.fb.pens[1] = 256;
        assert_eq!(Frame::of_sf1(&v), Frame { drawn: 2, pages: 2 }, "FG's base");
        v.fb.pens[2] = 512;
        assert_eq!(Frame::of_sf1(&v), Frame { drawn: 3, pages: 3 }, "OB's base");
        v.fb.pens[3] = 1023;
        assert_eq!(
            Frame::of_sf1(&v),
            Frame { drawn: 4, pages: 4 },
            "1023 is the last entry, in TX's block"
        );
        // A pen past the palette is counted in neither, and does not panic.
        v.fb.pens[4] = 1024;
        assert_eq!(
            Frame::of_sf1(&v),
            Frame { drawn: 4, pages: 4 },
            "1024 is past the palette: not drawn, and no fifth block"
        );
    }
    /// `--game` selects the board, and the default is stated rather than sniffed.
    #[test]
    fn the_game_option_selects_the_board_and_defaults_to_sf2() {
        let game = |v: Vec<&str>| match parse_args(v.into_iter().map(String::from).collect()) {
            Ok(Args {
                source: Source::Set { game, .. },
                ..
            }) => Some(game),
            _ => None,
        };
        assert_eq!(
            game(vec!["/some/sf2.zip"]),
            Some("sf2".to_string()),
            "the default is a constant in this program"
        );
        assert_eq!(
            game(vec!["/wherever/it/is.zip", "--game", "sf1"]),
            Some("sf1".to_string()),
            "and a path that says nothing about the board is fine, because the \
             board does not come from the path"
        );
        // Order-independent, like `--ppm` and `--state`.
        assert_eq!(
            game(vec!["--game", "sf1", "/x.zip", "7"]),
            Some("sf1".to_string())
        );
    }

    /// A game this program does not know is a usage error, not a load error.
    ///
    /// The distinction is the point: `romset::load` on an unknown spec cannot happen
    /// — there is no spec — and a user who typed `sf3` needs to be told which names
    /// exist, not handed a missing-region message about files they got right.
    #[test]
    fn an_unknown_game_name_is_rejected_with_the_names_that_exist() {
        let args = |v: Vec<&str>| parse_args(v.into_iter().map(String::from).collect());
        match args(vec!["/some/set.zip", "--game", "sf3"]) {
            Err(Fault::Failed(m)) => {
                assert!(m.contains("sf3"), "names what was asked for: {m}");
                assert!(m.contains("sf1"), "and both names that exist: {m}");
                assert!(m.contains("sf2"), "{m}");
            }
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }
        // A missing value is an error rather than a silently ignored flag, like the
        // other two options.
        match args(vec!["/some/set.zip", "--game"]) {
            Err(Fault::Failed(m)) => assert_eq!(m, "`--game` needs a name"),
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }
        match args(vec!["/x.zip", "--game", "sf1", "--game", "sf2"]) {
            Err(Fault::Failed(m)) => assert_eq!(m, "`--game` given twice"),
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }
    }

    /// Each name maps to its own board, and to the spec of the same name.
    ///
    /// Two halves, because the failure that matters is a crossed pair: `--game sf1`
    /// loading SF2's ROM spec into SF1's board is a machine that fetches CPS-1 code
    /// through an SF1 bus, and every symptom of that is downstream and confusing.
    #[test]
    fn each_game_name_selects_its_own_board_and_its_own_rom_spec() {
        assert_eq!(board_for("sf2"), Some(machine::BoardKind::Cps1));
        assert_eq!(board_for("sf2eb"), Some(machine::BoardKind::Cps1));
        assert_eq!(board_for("sf1"), Some(machine::BoardKind::Sf1));
        assert_eq!(board_for("sf3"), None);
        // And the name that picks the board is the name that picks the files.
        assert_eq!(romset::games::by_name("sf2").map(|g| g.name), Some("sf2"));
        assert_eq!(
            romset::games::by_name("sf2eb").map(|g| g.name),
            Some("sf2eb")
        );
        assert_eq!(romset::games::by_name("sf1").map(|g| g.name), Some("sf1"));
        // Every name with a board has a spec, and no name has one without the other.
        for g in romset::games::ALL {
            assert!(board_for(g.name).is_some(), "{} has no board", g.name);
        }
    }

    /// The name selects the CPS-B row, and the two SF2 revisions get different ones.
    ///
    /// The check the guest performs is spelled out here rather than only in
    /// `machine`, because this is the function that chooses which value the guest
    /// will read: `sf2eb`'s program reads offset 0x08 and needs 0x0407, and under
    /// `sf2`'s row it would read a plain CPS-B register instead and branch to an
    /// idle loop. Asserting the ID pair — not just that the two configs differ —
    /// is what ties this mapping to the reason it exists.
    #[test]
    fn the_game_name_selects_the_cps_b_row_and_the_two_sf2_revisions_differ() {
        let a = cps_b_config_for("sf2").expect("sf2 has a row");
        let b = cps_b_config_for("sf2eb").expect("sf2eb has a row");
        assert_eq!(a.cpsb_addr, Some(0x32), "CPS_B_11");
        assert_eq!(a.cpsb_value, 0x0401);
        assert_eq!(b.cpsb_addr, Some(0x08), "CPS_B_17");
        assert_eq!(b.cpsb_value, 0x0407);
        assert_ne!(a.video, b.video, "and the video registers move with it");
    }

    /// A CPS-1 machine built for `sf2eb` answers the address its program reads.
    ///
    /// The artifact, not the mapping: [`build_cps1`] is called with the name and the
    /// resulting machine's bus is read at 0x800148, through `peek_word`, exactly as
    /// the guest's `move.w $800148,d0` does. A `build_cps1` that took the config
    /// from anywhere but its `game` argument fails here while the mapping test above
    /// still passes.
    #[test]
    fn a_machine_built_for_sf2eb_answers_the_id_read_its_program_makes() {
        // Synthetic regions: this needs a bus, not a game.
        let set = a_synthetic_set(&[
            ("maincpu", 0x11),
            ("gfx", 0x22),
            ("audiocpu", 0x33),
            ("oki", 0x44),
        ]);
        let eb = build_cps1("sf2eb", &set).expect("every region is present");
        let g = build_cps1("sf2", &set).expect("every region is present");

        // `andi.w #$FC3F` then `cmpi.w #$0407`, at 0x0004c2.
        let read = |m: &machine::Cps1| m.board.peek_word(0x80_0148).expect("CPS-B decodes");
        assert_eq!(read(&eb) & 0xFC3F, 0x0407, "sf2eb's check passes");
        assert_ne!(read(&g) & 0xFC3F, 0x0407, "and rev G's board fails it");

        // The converse address, so this is about the row and not about 0x800148
        // happening to hold 0x0407 on every board.
        assert_eq!(
            g.board.peek_word(0x80_0172).expect("CPS-B decodes"),
            0x0401,
            "rev G answers its own ID address"
        );
    }

    /// The loop's board tag follows the selected game.
    ///
    /// `loop_opts` is where `main`'s two independent choices — the board and the tag
    /// — are made together, and `loop_::run`'s `debug_assert_eq!` is what checks it.
    /// This is the test on this side of that assertion.
    #[test]
    fn the_loops_board_tag_follows_the_selected_game() {
        let with = |source: Source| Args {
            source,
            frames: 60,
            ppm: None,
            play: true,
            state: PathBuf::from("/tmp/mine.sfs"),
        };
        let named = |game: &str| {
            with(Source::Set {
                path: "/a/b/set.zip".to_string(),
                game: game.to_string(),
            })
        };
        // Literals, not `state_tag(..)`: this asserts what reaches the loop, and
        // calling the function under test to produce the expectation would pass for
        // any mapping at all. Big-endian ASCII `SF2\0` and `SF1\0`.
        assert_eq!(loop_opts(&named("sf2")).board, 0x5346_3200);
        assert_eq!(loop_opts(&named("sf1")).board, 0x5346_3100);
        // The demo is CPS-1, so it carries SF2's tag — which is what stops a state
        // saved from the demo from loading into an SF1 machine, and vice versa.
        assert_eq!(loop_opts(&with(Source::Demo)).board, 0x5346_3200);
    }
    /// The demo builds a CPS-1 out of the regions the generator answers.
    ///
    /// The seam this guards is a pair of string lists that never meet at compile
    /// time: `testrom::demo::build` names four regions and `build_cps1` looks four
    /// up. A rename on either side is a `Fault::Failed` at run time and nothing at
    /// build time — and the whole point of `--demo` is that it works for someone who
    /// cannot check it against a real set.
    #[test]
    fn the_demo_image_names_the_regions_the_cps1_builder_asks_for() {
        let m = demo_machine().expect("the generator answers every region CPS-1 needs");
        assert_eq!(m.board(), machine::BoardKind::Cps1);
        assert!(matches!(m, machine::Machine::Cps1(_)), "and the Cps1 arm");
    }

    /// The demo boots, draws through the real renderer, and keeps running.
    ///
    /// This is the test that closes the loop `crates/testrom` deliberately leaves
    /// open: `gfx`'s own tests read tiles back through a *second transcription* of
    /// the tile format, so a shared error there would cancel out. Here the pens come
    /// out of `video` itself.
    ///
    /// 70 frames, and the number is load-bearing: the demo's sound command goes out
    /// every 64th frame, so a shorter run would assert a silent latch and pass on a
    /// driver that never talks to the Z80.
    #[test]
    fn the_demo_runs_and_draws_and_talks_to_the_sound_board() {
        let mut m = demo_machine().expect("the demo builds");
        for _ in 0..70 {
            m.run_frame();
        }
        m.render();

        let t = m.cpu_view().trace;
        assert_eq!(t.frames, 70, "seventy frames ran");
        // One short of the vblanks: the last one is asserted at the end of the frame
        // and acknowledged in the next, which has not run.
        assert_eq!(
            (t.vblanks, t.acks),
            (70, 69),
            "and the handler serviced them"
        );
        assert_eq!(
            (t.unmapped_reads.total(), t.unmapped_writes.total()),
            (0, 0),
            "the demo touches nothing this board does not decode"
        );
        assert_eq!(t.sound_latch_writes, 1, "one command by frame 64");

        // The picture: a `Frame` count through `video`'s own palette rule. Four
        // pages, one per layer, because the four colour bases are far enough apart to
        // land in four different 512-pen pages: sprites at scheme 0x00, scroll 1 at
        // 0x20, scroll 2 at 0x40 and scroll 3 at 0x60, which is pen 0x600 and so
        // page 3.
        //
        // The count is load-bearing and not decorative. A layer wholly hidden behind
        // an opaque one still draws every register write, every gfxram write and a
        // full screen of pens — so the counters above and a `drawn > 0` cannot see
        // it, and the picture looks deliberate. Three pages here is exactly the
        // symptom of scroll 2 covering scroll 3.
        let f = Frame::of(&cps1(&m).video);
        assert!(f.drawn > 0, "something drew: {f:?}");
        assert_eq!(f.pages, 4, "all four layers reach the screen: {f:?}");

        // And it is still executing rather than halted on a bad vector — which a
        // drawn frame alone does not rule out, because the tables were written
        // before the fault.
        let v = m.cpu_view();
        assert!(!v.cpu.halted, "not halted");
        assert!(!v.cpu.stopped, "and not stopped");
    }

    /// The demo's picture changes from frame to frame.
    ///
    /// The assertion a moving demo actually needs. Every check above is satisfied by
    /// a machine that drew one frame and then wedged: the tables are in gfxram by the
    /// end of `setup`, so a vblank handler that never ran would still leave a full,
    /// plausible screen. Two renders at different frame counts, compared pen by pen,
    /// is what says the 68000 is still doing work.
    #[test]
    fn the_demo_picture_moves() {
        let pens = |frames: u32| {
            let mut m = demo_machine().expect("the demo builds");
            for _ in 0..frames {
                m.run_frame();
            }
            m.render();
            cps1(&m).video.fb.pens.to_vec()
        };
        // Ten frames apart: scroll 3 moves a pixel a frame and scroll 2 two the
        // other way, so a single frame would also differ — ten is simply past any
        // question of an off-by-one in the scroll registers.
        assert_ne!(pens(5), pens(15), "the screen is not the same picture");
    }

    /// `--demo` takes no path, and a path given with it is a loud error.
    ///
    /// The error matters more than the parse: the demo opens no files, so
    /// `sfemu --demo mysf2.zip` under a lenient parser would run the generated image
    /// and print a healthy report about a set it never looked at.
    #[test]
    fn the_demo_flag_replaces_the_rom_set_path() {
        let args = |v: Vec<&str>| parse_args(v.into_iter().map(String::from).collect());
        assert_eq!(
            args(vec!["--demo"]).ok().map(|a| a.source),
            Some(Source::Demo)
        );
        match args(vec!["--demo", "/some/sf2.zip"]) {
            Err(Fault::Failed(m)) => {
                assert!(m.contains("--demo"), "names the flag: {m}");
                assert!(m.contains("no ROM set path"), "and why: {m}");
                assert!(m.contains("/some/sf2.zip"), "and what was given: {m}");
            }
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }
        // `--game` with `--demo` is an error too: the demo is one specific CPS-1
        // image, so `--demo --game sf1` is a request this program cannot honour and
        // must not silently ignore.
        match args(vec!["--demo", "--game", "sf1"]) {
            Err(Fault::Failed(m)) => {
                assert!(m.contains("--game"), "names both flags: {m}");
                assert!(m.contains("--demo"), "{m}");
            }
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }
        // And no arguments at all is still the usage error it was: `--demo` is a
        // stated choice, not what happens when you forget the path.
        assert!(matches!(args(vec![]), Err(Fault::Usage)));
    }

    /// The usage text tells a reader without a ROM set that `--demo` exists.
    ///
    /// This text is where someone who has no set arrives — the same argument
    /// `the_usage_text_states_that_no_rom_is_supplied_or_fetched` makes about the
    /// legal note. An option nobody can find is an option nobody has.
    #[test]
    fn the_usage_text_offers_the_demo_to_a_reader_with_no_rom_set() {
        let u = usage();
        assert!(u.contains("sfemu --demo"), "the invocation: {u}");
        assert!(
            u.contains("no files and no path"),
            "and that it needs none of what the rest of this text asks for: {u}"
        );
        assert!(
            u.contains("not any Capcom game"),
            "and that it is homebrew, so nobody reads it as a bundled ROM: {u}"
        );
    }

    /// A synthetic ROM set: one distinguishable byte pattern per region.
    ///
    /// Not a ROM and not a `romset::load` call — `RomSet` is a map of region name to
    /// bytes, so this crate can fill it with data it invents. Each region's first
    /// byte is a distinct tag, which is what lets the tests below tell a crossed pair
    /// of regions from a correct one.
    ///
    /// The sizes are the specs' own, from `romset::games`: SF1's `maincpu` is
    /// 0x60000 and its `gfx4` 0x4000, and a `Vec` shorter than the board's ROM window
    /// is copied in and zero-filled, which is fine — the subject here is which bytes
    /// land where, not how many.
    fn a_synthetic_set(names: &[(&str, u8)]) -> romset::RomSet {
        romset::RomSet {
            regions: names
                .iter()
                .map(|&(name, tag)| (name.to_string(), vec![tag; 0x100]))
                .collect(),
        }
    }

    /// The SF1 regions `build_sf1` asks for, each tagged with its own byte.
    fn an_sf1_set() -> romset::RomSet {
        a_synthetic_set(&[
            ("maincpu", 0x11),
            ("audiocpu", 0x22),
            ("audio2", 0x33),
            ("gfx1", 0x44),
            ("gfx2", 0x55),
            ("gfx3", 0x66),
            ("gfx4", 0x77),
            ("tilerom", 0x88),
        ])
    }

    /// The game name picks the board the machine is built on.
    ///
    /// `build_machine` is the one place `main` turns a name into hardware, and the
    /// failure it guards is a machine built on the wrong board out of a set that
    /// loaded cleanly: SF2's files on SF1's bus fetch garbage and every symptom is
    /// thousands of instructions downstream.
    #[test]
    fn the_game_name_picks_the_board_the_machine_is_built_on() {
        let sf1 = build_machine("sf1", &an_sf1_set()).expect("every sf1 region is present");
        assert_eq!(sf1.board(), machine::BoardKind::Sf1);
        assert!(matches!(sf1, machine::Machine::Sf1(_)), "and the Sf1 arm");

        let sf2 = build_machine(
            "sf2",
            &a_synthetic_set(&[("maincpu", 1), ("gfx", 2), ("audiocpu", 3), ("oki", 4)]),
        )
        .expect("every sf2 region is present");
        assert_eq!(sf2.board(), machine::BoardKind::Cps1);
        assert!(matches!(sf2, machine::Machine::Cps1(_)), "and the Cps1 arm");
    }

    /// Each of SF1's five graphics regions reaches the plane that reads it.
    ///
    /// Eight regions arrive as eight `Vec<u8>`s of the same type, so any two can be
    /// swapped without the compiler noticing — and a swap of `gfx1` and `gfx2` draws
    /// the foreground's tiles in the background and vice versa, which looks like a
    /// renderer bug in a renderer that is correct. The tags are literals, one per
    /// region, and this asserts which tag arrives where.
    #[test]
    fn each_of_sf1s_graphics_regions_reaches_its_own_plane() {
        let m = build_machine("sf1", &an_sf1_set()).expect("every sf1 region is present");
        let machine::Machine::Sf1(f) = &m else {
            panic!("`sf1` builds the Sf1 arm")
        };
        use machine::video::sf1::Plane;
        assert_eq!(f.video.region(Plane::Bg)[0], 0x44, "gfx1 is the background");
        assert_eq!(f.video.region(Plane::Fg)[0], 0x55, "gfx2 the foreground");
        assert_eq!(f.video.region(Plane::Sprites)[0], 0x66, "gfx3 the sprites");
        assert_eq!(f.video.region(Plane::Tx)[0], 0x77, "gfx4 the text plane");
        assert_eq!(f.video.tilerom()[0], 0x88, "and the tilerom is its own");
    }

    /// A missing SF1 region is a loud error naming the region.
    ///
    /// Eight `ok_or_else` messages behind one closure, and the closure is what this
    /// checks: a message that named the wrong region, or a builder that defaulted to
    /// an empty one, would leave a machine that boots into garbage with nothing said.
    #[test]
    fn a_missing_sf1_region_is_named_in_the_error() {
        let mut set = an_sf1_set();
        set.regions.remove("tilerom");
        match build_machine("sf1", &set) {
            Err(Fault::Failed(m)) => {
                assert!(m.contains("tilerom"), "names the region: {m}");
                assert!(m.contains("sf1"), "and the spec: {m}");
            }
            other => panic!("expected an error, got {:?}", other.is_ok()),
        }
    }

    /// A headless run's report names the board the machine actually is.
    ///
    /// `summary` is the seam between the machine and the text, and the failure it
    /// guards is a report that hardcodes one board: an SF1 run headed `CPS-1` would
    /// then print three counters about chips that are not there, which is the whole
    /// thing `an_sf1_report_omits_the_chips_sf1_does_not_have` rules out one level
    /// down.
    #[test]
    fn a_headless_runs_report_names_the_board_it_ran() {
        let r = summary(&one_frame());
        assert!(r.starts_with("board         CPS-1\n"), "{r}");

        let sf1 = build_machine("sf1", &an_sf1_set()).expect("every sf1 region is present");
        let r = summary(&sf1);
        assert!(r.starts_with("board         SF1\n"), "{r}");
        assert!(!r.contains("cps-a"), "and no CPS-A line: {r}");
        // The frame summary is SF1's too: nothing rendered, so nothing drew — where
        // CPS-1's blank pen rule would count every one of the 86,016 pens of 0xBFF
        // that `Framebuffer::new` leaves behind.
        assert!(
            r.contains("framebuffer   0 of 86016 pixels drawn, 0 colour block(s)\n"),
            "{r}"
        );

        // And a drawn SF1 frame is counted, not assumed empty: a fork that returned a
        // zeroed `Frame` for the Sf1 arm would satisfy every assertion above, because
        // an unrendered frame's honest answer is also zero. Two pens in two different
        // 256-entry colour blocks, written straight into the framebuffer — the subject
        // is which counter reads it, not how the renderer filled it.
        let mut sf1 = sf1;
        let machine::Machine::Sf1(f) = &mut sf1 else {
            panic!("`sf1` builds the Sf1 arm")
        };
        f.video.fb.pens[0] = 1;
        f.video.fb.pens[1] = 300;
        let r = summary(&sf1);
        assert!(
            r.contains("framebuffer   2 of 86016 pixels drawn, 2 colour block(s)\n"),
            "{r}"
        );
    }

    /// SF1's two sound ROMs reach the two Z80s that run them.
    ///
    /// `Sf1::new` takes `audiocpu` and `audio2` as adjacent `Vec<u8>` parameters of
    /// the same type, so swapping them compiles: the FM Z80 would then execute the
    /// ADPCM program and neither chip would be driven. The tags are literals, and
    /// `peek_byte` is the read that does not disturb the trace counters.
    #[test]
    fn sf1s_two_sound_roms_reach_their_own_processors() {
        let m = build_machine("sf1", &an_sf1_set()).expect("every sf1 region is present");
        let machine::Machine::Sf1(f) = &m else {
            panic!("`sf1` builds the Sf1 arm")
        };
        assert_eq!(f.fm.peek_byte(0), 0x22, "audiocpu drives the YM2151 board");
        assert_eq!(f.adpcm.peek_byte(0), 0x33, "audio2 the MSM5205 pair");
    }

    /// A screenshot is taken with the board's own DAC rule.
    ///
    /// One byte tells the two apart: SF1 doubles each nibble, so palette entry
    /// 0x0135 is (0x11, 0x33, 0x55), where CPS-1's shift gives (0x08, 0x18, 0x28).
    /// A `screenshot` that always called `ppm` would not compile for an `Sf1`, but
    /// one that reached for the wrong palette rule would — and would write a picture
    /// in the wrong colours.
    #[test]
    fn a_screenshot_uses_the_boards_own_dac_rule() {
        let mut sf1 = build_machine("sf1", &an_sf1_set()).expect("every sf1 region is present");
        let machine::Machine::Sf1(f) = &mut sf1 else {
            panic!("`sf1` builds the Sf1 arm")
        };
        f.board.palette[0] = 0x0135;
        f.render();
        let bytes = screenshot(&sf1);
        assert_eq!(&bytes[..15], b"P6\n384 224\n255\n", "the exact header");
        assert_eq!(
            &bytes[15..18],
            &[0x11, 0x33, 0x55],
            "SF1 doubles each nibble; CPS-1's shift would give 08 18 28"
        );

        let cps1 = a_drawn_frame();
        let bytes = screenshot(&cps1);
        assert_eq!(&bytes[..15], b"P6\n384 224\n255\n");
        assert_eq!(bytes.len(), 258_063, "and a full CPS-1 frame behind it");
    }

    /// The usage text names both games and says the board is not guessed.
    ///
    /// `--game` with no usage line is an option nobody can find, and the sentence
    /// about not guessing is the spec's ruling stated where a user reads it.
    #[test]
    fn the_usage_text_names_both_games() {
        let u = usage();
        assert!(u.contains("--game <name>"), "the option: {u}");
        assert!(u.contains("`sf2`"), "and sf2: {u}");
        assert!(u.contains("`sf1`"), "and sf1: {u}");
        assert!(
            u.contains("not guessed from"),
            "and that the board is a stated choice: {u}"
        );
    }
}
