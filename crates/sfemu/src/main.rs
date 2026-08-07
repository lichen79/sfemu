//! Run a CPS-1 ROM set and report what the board saw.
//!
//! ```text
//! sfemu <path-to-rom-set> [frames]
//! ```
//!
//! `<path-to-rom-set>` is a MAME-format zip or a directory of loose files that
//! **you supply**. This program contains no ROM data and no way to obtain any.
//!
//! # Why a report and not a window
//!
//! Sub-project B models the 68000 side of the board and nothing that draws. A
//! black window would be indistinguishable from a boot that hangs on the first
//! instruction; a count of vblanks, acknowledges, and video-register writes tells
//! you which. The window arrives with sub-projects C and E.

use machine::Trace;
use std::process::ExitCode;

/// What went wrong before the machine ever ran.
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
    "usage: sfemu <path-to-sf2.zip-or-directory> [frames]\n\
     \n\
     The ROM set is yours to supply: this program neither bundles nor\n\
     downloads one. Legal sources include Capcom Arcade Stadium, Capcom\n\
     Fighting Collection, or a board you own and dumped.\n"
        .to_string()
}

/// The ROM-set path and the frame count.
///
/// Split out of [`run`] so the frame count can be tested without a ROM set:
/// everything after this point needs one, so a default folded into `run` could
/// only ever be checked by a test that owns a ROM — which is to say, never.
fn parse_args(args: Vec<String>) -> Result<(String, u64), Fault> {
    let mut args = args.into_iter();
    let path = args.next().ok_or(Fault::Usage)?;
    // Parsed strictly. `unwrap_or(60)` on a typo — `6O` with a letter O — would
    // run a different number of frames than asked for and say nothing, and the
    // whole point of this program is that its numbers mean what they say.
    let frames: u64 = match args.next() {
        None => 60,
        Some(s) => s
            .parse()
            .map_err(|_| Fault::Failed(format!("`{s}` is not a frame count")))?,
    };
    if let Some(extra) = args.next() {
        return Err(Fault::Failed(format!("unexpected argument `{extra}`")));
    }
    Ok((path, frames))
}

/// Loads the set, runs `frames` frames, and returns the report.
fn run(args: Vec<String>) -> Result<String, Fault> {
    let (path, frames) = parse_args(args)?;

    let set = romset::load(&romset::games::SF2, std::path::Path::new(&path))
        .map_err(|e| Fault::Failed(e.to_string()))?;
    let prog = set.region("maincpu").ok_or_else(|| {
        Fault::Failed("internal: the sf2 spec has no `maincpu` region".to_string())
    })?;

    let mut m = machine::Cps1::new(
        prog,
        machine::BoardConfig::sf2(),
        machine::Timing::cps1_10mhz(),
    );
    m.reset();
    for _ in 0..frames {
        m.run_frame();
    }
    Ok(report(&m.board.trace, m.total_cycles, Cpu::of(&m)))
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

/// Formats a run's trace.
///
/// Separate from [`run`] so it can be tested against a machine this crate builds
/// itself — the loader path needs a ROM set and therefore cannot be tested here at
/// all, but the report is pure and gets literals like everything else.
fn report(t: &Trace, cycles: u64, cpu: Cpu) -> String {
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
            report(&m.board.trace, m.total_cycles, Cpu::of(&m)),
            format!(
                "frames        1\n\
                 vblanks       1  acks 0\n\
                 cycles        {}\n\
                 cpu           pc 0x001014  stopped (waiting for an interrupt)\n\
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
        let r = report(&m.board.trace, m.total_cycles, Cpu::of(&m));
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
        let r = report(&m.board.trace, m.total_cycles, Cpu::of(&m));
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
        let r = report(&t, 0, IDLE);
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
        let r = report(&t, 0, IDLE);
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
        let r = report(&t, 0, IDLE);
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
        assert_eq!(
            parse_args(vec!["/some/sf2.zip".into()]).ok(),
            Some(("/some/sf2.zip".to_string(), 60))
        );
        assert_eq!(
            parse_args(vec!["/some/sf2.zip".into(), "7".into()]).ok(),
            Some(("/some/sf2.zip".to_string(), 7)),
            "and an explicit count is taken as given"
        );
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
}
