//! The run loop, and the one trait that stands between it and a window.
//!
//! # Why a trait
//!
//! `cargo test` has no display, and "the right pixels reached the glass" is not
//! something a test can read back. But the loop's *decisions* — how many frames a
//! host tick owes, whether a pause holds, whether a step is exactly one frame,
//! whether a failed save stops everything — are the parts worth testing, and none
//! of them need a window. So the loop takes a [`Display`], the tests give it a
//! recording fake, and `sfemu`'s `display` module gives it a real window while
//! making no decisions of its own.
//!
//! # What lives here and what lives in `frontend`
//!
//! `frontend` decides *what* — how many frames this tick owes, which board input a
//! key is, what colour a pen is, what bytes a save state is. This module decides
//! *when*, in the sense of ordering: read the keys, then act on them, then run, then
//! present. It holds no arithmetic, which is why every constant it uses comes from
//! `frontend`.

use frontend::keys::{Actions, Controls, KeySet};
use frontend::{pens_to_argb, FramePacer};
use machine::Cps1;
use std::path::PathBuf;

/// A window, as far as the loop is concerned.
///
/// Five methods, none of which decides anything. An implementation translates: keys
/// to [`KeySet`], a clock to nanoseconds, a buffer to the glass.
pub trait Display {
    /// Shows a frame. `buf` is `WIDTH * HEIGHT` pixels of `0x00RRGGBB`.
    ///
    /// Fallible because the windowing library's update is: a window that has gone
    /// away mid-frame is a thing that happens, and the loop reports it rather than
    /// panicking in someone's game.
    fn present(&mut self, buf: &[u32]) -> Result<(), String>;

    /// The keys held right now.
    fn held_keys(&self) -> KeySet;

    /// Host nanoseconds since the previous call.
    ///
    /// `&mut self` because a real implementation resets its own last-tick mark, and
    /// a trait that took `&self` would push that state behind a cell for no reason.
    fn elapsed_ns(&mut self) -> u64;

    /// Whether the window is still there.
    fn is_open(&self) -> bool;

    /// Sets the title bar.
    fn set_title(&mut self, title: &str);
}

/// Where the loop writes the things it is asked to write.
#[derive(Debug, Clone)]
pub struct LoopOpts {
    /// The save-state file, for F5 and F8.
    pub state_path: PathBuf,
    /// The screenshot file, for F12.
    pub shot_path: PathBuf,
}

/// What a finished run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Frames the machine ran.
    pub frames: u64,
    /// Frames the pacer refused to catch up on.
    pub dropped: u64,
    /// Things that went wrong without being fatal, each reported once.
    ///
    /// A failed save is not a reason to lose the session — the loop keeps running
    /// and says so here. "Once" is deliberate: a notice pushed per frame would be
    /// sixty identical lines a second, which is a way of hiding the message inside
    /// the message.
    pub notices: Vec<String>,
}

/// The board this build's states belong to.
///
/// Only SF2 exists so far. When the SF1 driver lands, this becomes a field on
/// [`LoopOpts`] — the point of the tag is that loading one board's state into
/// another is refused, which needs the loop to know which board it is running.
const BOARD: u32 = frontend::BOARD_SF2;

/// Runs until the display closes or the user quits.
///
/// The order inside an iteration is the whole design, and it is this:
///
/// 1. read the elapsed time — before anything, so a slow save is not charged to the
///    game as owed frames;
/// 2. read the keys and turn them into [`Actions`];
/// 3. quit, if asked;
/// 4. hand the board its inputs — *level*-triggered, so this happens whether or not
///    a frame runs;
/// 5. reset, pause, save, load;
/// 6. run the frames this tick owes;
/// 7. render and present — **every** iteration, including a paused one, or the
///    window goes black the moment you pause;
/// 8. screenshot, then the title.
pub fn run(m: &mut Cps1, d: &mut impl Display, o: &LoopOpts) -> Summary {
    let mut pacer = FramePacer::cps1();
    let mut controls = Controls::new();
    let mut buf: Vec<u32> = Vec::new();
    let mut paused = false;
    let mut summary = Summary::default();
    let mut title = String::new();

    while d.is_open() {
        let elapsed = d.elapsed_ns();
        let a: Actions = controls.update(d.held_keys());
        if a.quit {
            break;
        }

        m.board.inputs = a.inputs;

        if a.reset {
            m.reset();
            // The pacer too: the wall-clock time spent deciding to press F3 is not
            // game time the fresh machine owes.
            pacer.reset();
        }
        if a.pause_toggled {
            paused = !paused;
            // Whichever way it went. Pausing discards the debt accrued before the
            // pause; unpausing discards the pause itself, which is the one that
            // matters — without it, a minute paused is a minute owed, and the
            // machine sprints through four frames and drops 3,575.
            pacer.reset();
        }

        if a.save {
            save(m, o, &mut summary);
        }
        if a.load {
            load(m, o, &mut summary);
        }

        // A step is one frame regardless of the clock, which is what makes it a
        // step. Checked before `paused` because stepping only means anything while
        // paused, and checking `paused` first would make the branch unreachable.
        let frames = if a.step {
            1
        } else if paused {
            0
        } else {
            pacer.tick(elapsed)
        };
        for _ in 0..frames {
            m.run_frame();
        }
        summary.frames += u64::from(frames);

        // Outside the loop above: a paused iteration renders too. The frame does not
        // change, but the window is redrawn, and a windowing library that is not
        // given a buffer shows an undefined one.
        m.render();
        pens_to_argb(&m.video, &mut buf);
        if let Err(e) = d.present(&buf) {
            note(&mut summary, format!("cannot present a frame: {e}"));
            break;
        }

        if a.screenshot {
            screenshot(m, o, &mut summary);
        }

        summary.dropped = pacer.dropped();
        let want = title_for(&summary, m, paused);
        if want != title {
            d.set_title(&want);
            title = want;
        }
    }

    summary
}

/// The title bar.
///
/// Only the states worth interrupting someone's game to mention: paused, because
/// the picture stopped and you want to know why; dropped frames, because the host
/// cannot keep up and that is not the emulator's bug; halted, because a 68000 that
/// double bus faulted will never execute another instruction and the window would
/// otherwise just freeze.
fn title_for(s: &Summary, m: &Cps1, paused: bool) -> String {
    let mut t = String::from("sfemu");
    if paused {
        t.push_str(" [paused]");
    }
    if m.cpu.halted {
        t.push_str(" [CPU halted]");
    }
    if s.dropped > 0 {
        t.push_str(&format!(" [{} dropped]", s.dropped));
    }
    t
}

/// Records a notice, once.
///
/// Deduplicated by exact text: the same failure on every frame is one problem, and
/// sixty lines a second of it is how a log stops being read.
fn note(s: &mut Summary, msg: String) {
    if !s.notices.contains(&msg) {
        s.notices.push(msg);
    }
}

fn save(m: &Cps1, o: &LoopOpts, s: &mut Summary) {
    let bytes = frontend::encode(&m.snapshot(), BOARD);
    match std::fs::write(&o.state_path, &bytes) {
        Ok(()) => {}
        Err(e) => note(s, format!("cannot write `{}`: {e}", o.state_path.display())),
    }
}

fn load(m: &mut Cps1, o: &LoopOpts, s: &mut Summary) {
    let bytes = match std::fs::read(&o.state_path) {
        Ok(b) => b,
        Err(e) => {
            note(s, format!("cannot read `{}`: {e}", o.state_path.display()));
            return;
        }
    };
    // The machine is left untouched on any failure. A partial restore would be a
    // machine that is neither the saved one nor the running one, and the loop would
    // carry on running it.
    match frontend::decode(&bytes, BOARD) {
        Ok(state) => m.restore(&state),
        Err(e) => note(s, format!("cannot load `{}`: {e}", o.state_path.display())),
    }
}

fn screenshot(m: &Cps1, o: &LoopOpts, s: &mut Summary) {
    let ppm = crate::ppm(&m.video);
    if let Err(e) = std::fs::write(&o.shot_path, &ppm) {
        note(s, format!("cannot write `{}`: {e}", o.shot_path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontend::keys::Key;
    use frontend::FRAME_NS;
    use machine::video::{HEIGHT, WIDTH};
    use machine::{BoardConfig, Timing};

    /// A `Display` that returns a script and records what it was shown.
    ///
    /// This is what lets the loop be tested at all: `cargo test` has no window, and
    /// the loop's decisions — how many frames a tick runs, whether a pause holds,
    /// whether a step is one frame — are exactly the decisions worth testing. A
    /// loop that could only be driven by a real window would be verified by looking
    /// at it.
    struct Fake {
        /// One entry per tick: the keys held and the host time since the last.
        script: Vec<(KeySet, u64)>,
        tick: usize,
        /// Every buffer length handed to `present`.
        presented: Vec<usize>,
        titles: Vec<String>,
    }

    impl Fake {
        /// A script from `(keys, elapsed)` pairs.
        fn new(script: Vec<(KeySet, u64)>) -> Self {
            Self {
                script,
                tick: 0,
                presented: Vec::new(),
                titles: Vec::new(),
            }
        }

        /// `n` ticks of one frame's time with nothing held.
        fn idle(n: usize) -> Vec<(KeySet, u64)> {
            vec![(KeySet::new(), FRAME_NS); n]
        }

        /// One tick with `keys` held for one frame's time.
        fn held(keys: &[Key]) -> (KeySet, u64) {
            (KeySet::from_keys(keys), FRAME_NS)
        }
    }

    impl Display for Fake {
        fn present(&mut self, buf: &[u32]) -> Result<(), String> {
            self.presented.push(buf.len());
            Ok(())
        }
        fn held_keys(&self) -> KeySet {
            // `tick` has already been advanced by `elapsed_ns`, which the loop calls
            // first — so this reads the entry that call consumed.
            self.script[self.tick - 1].0
        }
        fn elapsed_ns(&mut self) -> u64 {
            let ns = self.script[self.tick].1;
            self.tick += 1;
            ns
        }
        fn is_open(&self) -> bool {
            self.tick < self.script.len()
        }
        fn set_title(&mut self, title: &str) {
            self.titles.push(title.to_string());
        }
    }

    /// A machine running a program written inline. No ROM, here or anywhere.
    ///
    /// The same diverging fixture the save-state tests use: it counts in `d0`, writes
    /// every word it touches, and counts its interrupts in `d1`. That makes two runs
    /// of different lengths distinguishable, which is what the save/load test needs.
    ///
    /// ```text
    /// 1000  46FC 2000        move #$2000,sr
    /// 1004  5240             addq.w #1,d0
    /// 1006  33C0 00FF 0000   move.w d0,$FF0000
    /// 100C  60F6             bra $1004
    /// 1100  5241             addq.w #1,d1
    /// 1102  4E73             rte
    /// ```
    fn machine() -> Cps1 {
        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x68..0x6C].copy_from_slice(&[0x00, 0x00, 0x11, 0x00]);
        rom[0x1000..0x100E].copy_from_slice(&[
            0x46, 0xFC, 0x20, 0x00, 0x52, 0x40, 0x33, 0xC0, 0x00, 0xFF, 0x00, 0x00, 0x60, 0xF6,
        ]);
        rom[0x1100..0x1104].copy_from_slice(&[0x52, 0x41, 0x4E, 0x73]);
        let mut m = Cps1::new(&rom, BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        m
    }

    /// A unique temp path, removed when the guard drops.
    ///
    /// The process id keeps two `cargo test` runs from colliding, and the name keeps
    /// two tests in the same run apart. `Drop` and not a call at the end of the test
    /// so a failing assertion still cleans up.
    struct TempPath(PathBuf);

    impl TempPath {
        fn new(name: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!("sfemu-loop-{}-{}.bin", std::process::id(), name));
            let _ = std::fs::remove_file(&p);
            Self(p)
        }
    }

    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Options writing into temp files named after the test.
    fn opts(name: &str) -> (LoopOpts, TempPath, TempPath) {
        let state = TempPath::new(&format!("{name}-state"));
        let shot = TempPath::new(&format!("{name}-shot"));
        let o = LoopOpts {
            state_path: state.0.clone(),
            shot_path: shot.0.clone(),
        };
        (o, state, shot)
    }

    /// One tick of one frame's time runs one frame.
    #[test]
    fn an_ordinary_tick_runs_one_frame() {
        let (o, _s, _p) = opts("ordinary");
        let mut m = machine();
        let mut d = Fake::new(Fake::idle(1));
        let s = run(&mut m, &mut d, &o);
        assert_eq!(s.frames, 1);
        assert_eq!(s.dropped, 0);
        assert!(s.notices.is_empty(), "{:?}", s.notices);
    }

    /// Pause stops the frames, and resume starts them.
    ///
    /// Asserted on the frame count and not on an internal `paused` flag: a test
    /// reading the same flag the code sets passes a half-done fix that flips the
    /// flag and keeps running frames.
    #[test]
    fn pause_stops_the_frames_and_resume_starts_them() {
        let (o, _s, _p) = opts("pause");
        let mut script = vec![Fake::held(&[Key::P])];
        script.extend(Fake::idle(2));
        script.push(Fake::held(&[Key::P]));
        script.extend(Fake::idle(2));
        let mut d = Fake::new(script);
        let s = run(&mut machine(), &mut d, &o);
        // Six ticks, three frames. The pause tick and its two successors run none;
        // the resume tick and the two after it run one each — the pacer's reset
        // discards the time spent paused, but this tick's own elapsed is still
        // charged, which is the one frame of host time that really did pass.
        //
        // Three and not six is the whole assertion: a pause that did nothing would
        // run one frame per tick.
        assert_eq!(s.frames, 3, "the resume tick and the two after it");
    }

    /// A step is exactly one frame, on the edge.
    #[test]
    fn a_step_runs_exactly_one_frame_while_paused() {
        let (o, _s, _p) = opts("step");
        let mut script = vec![Fake::held(&[Key::P])];
        // Held for three ticks: one frame, because a step is an edge and not a
        // level. Then released and pressed again: a second edge, a second frame.
        script.extend(vec![Fake::held(&[Key::Period]); 3]);
        script.extend(Fake::idle(1));
        script.push(Fake::held(&[Key::Period]));
        let mut d = Fake::new(script);
        let s = run(&mut machine(), &mut d, &o);
        assert_eq!(s.frames, 2, "one frame per press, not per tick held");
    }

    /// A step does not unpause.
    #[test]
    fn a_step_does_not_unpause() {
        let (o, _s, _p) = opts("step-pause");
        let mut script = vec![Fake::held(&[Key::P]), Fake::held(&[Key::Period])];
        script.extend(Fake::idle(3));
        let mut d = Fake::new(script);
        let s = run(&mut machine(), &mut d, &o);
        assert_eq!(s.frames, 1, "the step, and nothing from the three after it");
    }

    /// A stalled host runs the cap, not the debt.
    ///
    /// Two seconds owes 119 frames. The pacer serves four and records the rest as
    /// dropped, which is what stops a hitch from becoming a burst of fast-forward.
    /// The literals are hand-computed: 2e9 / 16_768_000 = 119 whole frames.
    #[test]
    fn a_stalled_host_runs_the_cap_and_not_the_debt() {
        let (o, _s, _p) = opts("stall");
        let mut d = Fake::new(vec![(KeySet::new(), 2_000_000_000)]);
        let s = run(&mut machine(), &mut d, &o);
        assert_eq!(s.frames, 4, "the catch-up cap");
        assert_eq!(s.dropped, 115, "119 owed less the 4 served");
    }

    /// F3 returns the machine to power-on.
    #[test]
    fn reset_returns_the_machine_to_power_on() {
        let (o, _s, _p) = opts("reset");
        let mut m = machine();
        // Recorded from *this* machine before it runs, rather than from a second one
        // built afterwards: a `Cps1` is 525 KB on the stack, and two live in one test
        // thread overflows it. Which also makes the check stronger — the comparison
        // is against this machine's own power-on state.
        let fresh = (m.cpu.pc, m.cpu.prefetch);
        let mut script = Fake::idle(3);
        // Zero elapsed on the F3 tick: the reset happens before the frame count is
        // decided, so a tick that owed a frame would run one *after* resetting and
        // `total_cycles` would be one frame rather than zero. Zero host time is what
        // isolates the reset from the frame that follows it.
        script.push((KeySet::from_keys(&[Key::F3]), 0));
        let mut d = Fake::new(script);
        run(&mut m, &mut d, &o);
        assert_eq!(m.total_cycles, 0, "the cycle count restarts");
        // 0x1004 and not 0x1000: `M68k::reset` refills the prefetch queue, which
        // advances the PC past the two words it read. Compared against a freshly
        // reset machine rather than against a literal, so this states "F3 is a power
        // cycle" rather than restating the core's prefetch convention — which is
        // `m68k`'s to change.
        assert_eq!(m.cpu.pc, fresh.0, "the PC is where power-on leaves it");
        assert_eq!(m.cpu.prefetch, fresh.1, "and so is the queue");
        assert_eq!(m.line, 0, "and the beam is at the top of a frame");
        // Not the trace: `reset` deliberately does not clear it, because the trace
        // records the session and a reset is part of the session.
        assert!(
            m.board.trace.vblanks > 0,
            "the trace keeps the whole session"
        );
    }

    /// Escape ends the loop before the script does.
    #[test]
    fn escape_ends_the_loop_early() {
        let (o, _s, _p) = opts("escape");
        let mut script = Fake::idle(10);
        script[3] = Fake::held(&[Key::Escape]);
        let mut d = Fake::new(script);
        run(&mut machine(), &mut d, &o);
        // Three, not four: the quit tick breaks *before* presenting. Nothing about
        // the frame changed on that tick, and drawing into a window that is closing
        // is at best wasted and at worst a use of a surface that has gone away.
        assert_eq!(d.presented.len(), 3, "the three ticks before the quit");
        assert_eq!(d.tick, 4, "and it read the quit tick and stopped");
        assert!(d.tick < 10, "well short of the script's end");
    }

    /// Every tick presents a full frame, paused ones included.
    ///
    /// A window that is not given a buffer while paused shows an undefined one — in
    /// practice, black. Pausing is not supposed to blank the screen.
    #[test]
    fn every_tick_presents_a_full_frame() {
        let (o, _s, _p) = opts("present");
        let mut script = Fake::idle(2);
        script.push(Fake::held(&[Key::P]));
        script.extend(Fake::idle(3));
        let ticks = script.len();
        let mut d = Fake::new(script);
        run(&mut machine(), &mut d, &o);
        assert_eq!(d.presented.len(), ticks, "one present per tick");
        assert!(
            d.presented.iter().all(|&n| n == WIDTH * HEIGHT),
            "every frame is {} pixels, got {:?}",
            WIDTH * HEIGHT,
            d.presented
        );
        assert_eq!(WIDTH * HEIGHT, 86_016, "384 x 224");
    }

    /// The title reports dropped frames — and only when there are some.
    #[test]
    fn the_title_reports_dropped_frames() {
        let (o, _s, _p) = opts("title-drop");
        let mut d = Fake::new(vec![(KeySet::new(), 2_000_000_000)]);
        run(&mut machine(), &mut d, &o);
        assert!(
            d.titles.iter().any(|t| t.contains("115 dropped")),
            "a stall must say so: {:?}",
            d.titles
        );

        // And an ordinary run does not. A title that always mentioned drops would
        // be noise, and a test that only checked the stall case would pass for one.
        let (o, _s, _p) = opts("title-quiet");
        let mut d = Fake::new(Fake::idle(4));
        run(&mut machine(), &mut d, &o);
        assert!(
            !d.titles.iter().any(|t| t.contains("dropped")),
            "a healthy run must not: {:?}",
            d.titles
        );
    }

    /// Pausing says so in the title.
    #[test]
    fn the_title_reports_the_pause() {
        let (o, _s, _p) = opts("title-pause");
        let mut script = vec![Fake::held(&[Key::P])];
        script.extend(Fake::idle(1));
        let mut d = Fake::new(script);
        run(&mut machine(), &mut d, &o);
        assert!(
            d.titles.iter().any(|t| t.contains("paused")),
            "{:?}",
            d.titles
        );
    }

    /// F5 then F8 restores the machine's future, through a real file.
    ///
    /// Divergence, not comparison: save, run on, load, run the same number of
    /// frames, and require the machine to arrive at the same place. The file is a
    /// real one on disk — this is the test that would catch a loop that encoded to a
    /// buffer and forgot to write it.
    #[test]
    fn a_save_and_load_round_trip_through_the_real_file() {
        let (o, _s, _p) = opts("roundtrip");
        let mut m = machine();

        // Save on tick one, then run four frames.
        let mut script = vec![Fake::held(&[Key::F5])];
        script.extend(Fake::idle(4));
        let mut d = Fake::new(script);
        let s = run(&mut m, &mut d, &o);
        assert!(
            s.notices.is_empty(),
            "the save must succeed: {:?}",
            s.notices
        );
        assert!(o.state_path.exists(), "and leave a file behind");
        let after = (m.total_cycles, m.cpu.d[0], m.cpu.d[1], m.line);
        assert_ne!(after.0, 0, "the premise: the machine moved");

        // Run four more, then load and run the same four again.
        let mut d = Fake::new(Fake::idle(4));
        run(&mut m, &mut d, &o);
        let mut script = vec![Fake::held(&[Key::F8])];
        script.extend(Fake::idle(4));
        let mut d = Fake::new(script);
        let s = run(&mut m, &mut d, &o);
        assert!(
            s.notices.is_empty(),
            "the load must succeed: {:?}",
            s.notices
        );
        assert_eq!(
            (m.total_cycles, m.cpu.d[0], m.cpu.d[1], m.line),
            after,
            "a loaded machine must run the same four frames"
        );
    }

    /// A failed save does not stop the loop, and says so once.
    #[test]
    fn a_failed_save_does_not_stop_the_loop() {
        let mut bad = std::env::temp_dir();
        bad.push(format!(
            "sfemu-no-such-dir-{}/state.bin",
            std::process::id()
        ));
        let (_o, _s, shot) = opts("save-fail");
        let o = LoopOpts {
            state_path: bad.clone(),
            shot_path: shot.0.clone(),
        };
        assert!(!bad.exists(), "the premise: the directory is really absent");

        // Pressed on three separate edges, so a per-frame notice would be three.
        let script = vec![
            Fake::held(&[Key::F5]),
            Fake::held(&[]),
            Fake::held(&[Key::F5]),
            Fake::held(&[]),
            Fake::held(&[Key::F5]),
        ];
        let ticks = script.len();
        let mut d = Fake::new(script);
        let s = run(&mut machine(), &mut d, &o);
        assert_eq!(d.presented.len(), ticks, "the loop ran to the end");
        assert_eq!(s.notices.len(), 1, "one notice, not one per press");
        assert!(
            s.notices[0].contains(&bad.display().to_string()),
            "and it names the path: {}",
            s.notices[0]
        );
    }

    /// A corrupt state file does not stop the loop.
    #[test]
    fn a_corrupt_state_file_does_not_stop_the_loop() {
        let (o, _s, _p) = opts("corrupt");
        std::fs::write(&o.state_path, b"this is not a save state").expect("temp dir is writable");
        let mut m = machine();
        let mut script = Fake::idle(2);
        script.push(Fake::held(&[Key::F8]));
        script.extend(Fake::idle(2));
        let ticks = script.len();
        let mut d = Fake::new(script);
        let s = run(&mut m, &mut d, &o);
        assert_eq!(d.presented.len(), ticks, "the loop ran to the end");
        assert_eq!(s.notices.len(), 1, "{:?}", s.notices);
        assert!(
            s.notices[0].contains("not a save state"),
            "the notice carries the codec's reason: {}",
            s.notices[0]
        );
        // And the machine kept running rather than being half-restored. Five, not
        // four: the F8 tick owes a frame of its own like any other.
        assert_eq!(s.frames, 5, "one per tick, the failed load included");
    }

    /// A load that fails leaves the machine exactly as it was.
    #[test]
    fn a_failed_load_does_not_disturb_the_machine() {
        let (o, _s, _p) = opts("load-fail");
        // A valid state with one payload bit flipped: it passes the magic, version,
        // and board checks and fails the CRC, which is the closest a bad file gets
        // to being applied.
        let mut m = machine();
        m.run_frame();
        let mut bytes = frontend::encode(&m.snapshot(), BOARD);
        bytes[100_000] ^= 0x01;
        std::fs::write(&o.state_path, &bytes).expect("temp dir is writable");

        let before = (m.total_cycles, m.cpu.d[0], m.line);
        // Zero elapsed, so the tick owes no frame: what this test asserts is that
        // the *load* left the machine alone, and a frame run afterwards would move
        // it for a reason that has nothing to do with the load.
        let mut d = Fake::new(vec![(KeySet::from_keys(&[Key::F8]), 0)]);
        let s = run(&mut m, &mut d, &o);
        assert_eq!(s.notices.len(), 1, "{:?}", s.notices);
        assert_eq!(s.frames, 0, "the load tick owed nothing");
        assert_eq!(
            (m.total_cycles, m.cpu.d[0], m.line),
            before,
            "a refused load must not partially apply"
        );
    }

    /// F12 writes a screenshot.
    #[test]
    fn a_screenshot_is_written_as_a_ppm() {
        let (o, _s, shot) = opts("shot");
        let mut d = Fake::new(vec![Fake::held(&[Key::F12])]);
        let s = run(&mut machine(), &mut d, &o);
        assert!(s.notices.is_empty(), "{:?}", s.notices);
        let bytes = std::fs::read(&shot.0).expect("F12 must write the file");
        assert_eq!(&bytes[..2], b"P6", "a binary PPM");
        assert_eq!(
            bytes.len(),
            format!("P6\n{WIDTH} {HEIGHT}\n255\n").len() + WIDTH * HEIGHT * 3,
            "header plus three bytes a pixel"
        );
    }

    /// A halted CPU is reported and does not stop the loop.
    ///
    /// E2's debugger is what you want at that moment, and a window that froze with
    /// no explanation is what you would otherwise get. The machine here takes an
    /// interrupt through an odd SSP, which double bus faults on the frame push.
    #[test]
    fn a_halted_cpu_is_reported_in_the_title_and_does_not_stop_the_loop() {
        let (o, _s, _p) = opts("halt");
        let mut rom = vec![0u8; 0x2000];
        // An odd SSP. The reset vector fetch itself is fine; the first exception's
        // frame push is not, and `double_bus_fault` halts on an odd frame base.
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x01, 0x00, 0x00, 0x10, 0x00]);
        rom[0x68..0x6C].copy_from_slice(&[0x00, 0x00, 0x11, 0x00]);
        // move #$2000,sr to unmask, then spin.
        rom[0x1000..0x1006].copy_from_slice(&[0x46, 0xFC, 0x20, 0x00, 0x60, 0xFE]);
        let mut m = Cps1::new(&rom, BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();

        let mut d = Fake::new(Fake::idle(3));
        let s = run(&mut m, &mut d, &o);
        assert!(m.cpu.halted, "the premise: this program really halts");
        assert_eq!(d.presented.len(), 3, "the loop ran to the end anyway");
        assert_eq!(s.frames, 3, "and kept running frames");
        assert!(
            d.titles.iter().any(|t| t.contains("halted")),
            "the title must say so: {:?}",
            d.titles
        );
    }

    /// Held keys reach the board even on a tick that runs no frames.
    ///
    /// The board's inputs are level-triggered, so they are set before the frame
    /// count is decided. A loop that set them inside the frame loop would drop the
    /// coin you inserted while paused.
    #[test]
    fn inputs_reach_the_board_on_a_tick_with_no_frames() {
        let (o, _s, _p) = opts("inputs");
        let mut m = machine();
        let mut d = Fake::new(vec![
            Fake::held(&[Key::P]),
            Fake::held(&[Key::P, Key::Num5, Key::Down]),
        ]);
        let s = run(&mut m, &mut d, &o);
        // P is held on both ticks, so there is no second edge and no unpause.
        assert_eq!(s.frames, 0, "the premise: no frame ran");
        assert!(m.board.inputs.coin1, "the coin reached the board anyway");
        assert_ne!(m.board.inputs.in1(), 0xFFFF, "and so did the stick");
    }

    /// A closed display runs nothing.
    #[test]
    fn a_display_that_never_opens_runs_nothing() {
        let (o, _s, _p) = opts("closed");
        let mut d = Fake::new(Vec::new());
        let s = run(&mut machine(), &mut d, &o);
        assert_eq!(s, Summary::default());
        assert!(d.presented.is_empty());
    }
}
