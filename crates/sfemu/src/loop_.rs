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
//!
//! Audio arrives through a second such trait, [`crate::audio::Audio`], for the same
//! reason and with the same division: the rate conversion and the buffer policy are
//! [`machine::resample`]'s, and what happens here is only *when* a frame's samples are
//! handed over.

use crate::audio::Audio;
use frontend::debug::Debugger;
use frontend::gfx::GfxViewer;
use frontend::keys::{Actions, Controls, KeySet};
use frontend::{pens_to_argb, pens_to_argb_sf1, FramePacer, MAX_CATCH_UP};
use machine::Machine;
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
    /// The board tag this session's states carry, from [`state_tag`].
    ///
    /// # Why a field and not derived from the machine
    ///
    /// It could be — `state_tag(m.board())` is available everywhere it is read. It is
    /// a field because it is a *session* fact that `main` establishes when it picks a
    /// board, and because [`run`]'s `debug_assert_eq!` can then check `main`'s answer
    /// against the machine it was handed. Deriving it would make the two agree by
    /// construction and check nothing.
    ///
    /// ⚠️ **Read on the CPS-1 arm only.** [`frontend::encode_sf1`] and
    /// [`frontend::decode_sf1`] take no board argument: Task 19 baked `BOARD_SF1`
    /// into them, because SF1's payload length is validated in the same call and
    /// there is exactly one board that length belongs to. So this field is what
    /// CPS-1's `encode`/`decode` are given, and on an SF1 session it is checked by
    /// the assertion and otherwise unused. That is the reason the assertion exists.
    pub board: u32,
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

/// The save-state tag a board's states carry.
///
/// One function with two callers — [`run`]'s assertion and `main`'s `loop_opts` — so
/// that the tag in a [`LoopOpts`] and the board in the [`Machine`] beside it are
/// checkably paired rather than conventionally paired.
///
/// The tags themselves live in `frontend`, next to the codec that writes and checks
/// them. This is only the mapping from a board to its own.
#[must_use]
pub(crate) const fn state_tag(k: machine::BoardKind) -> u32 {
    match k {
        machine::BoardKind::Cps1 => frontend::BOARD_SF2,
        machine::BoardKind::Sf1 => frontend::BOARD_SF1,
    }
}

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
/// 6. apply the debugger's keys — before the frames, so a breakpoint set this tick is
///    honoured by this tick's frames rather than the next one's;
/// 7. apply the graphics viewer's keys, and hand its layer mask to `Video` — before
///    the render below, so this tick's frame is the masked one;
/// 8. run the frames this tick owes, **stopping mid-frame at a breakpoint**, and step
///    one instruction if `F4` asked;
/// 9. hand the samples those frames produced to the host, and tell it whether the
///    emulator is paused — after the frames, so the buffer this tick queues is the one
///    this tick made, and every iteration, so a pause that is held stays reported;
/// 10. render and present — **every** iteration, including a paused one, or the
///     window goes black the moment you pause. The overlays are drawn *after*
///     the pen-to-ARGB conversion, because they are ARGB and the pens are not, and the
///     graphics viewer goes over the debugger rather than under it;
/// 11. screenshot, then the title.
///
/// Nine of those steps fork on the board — the pacer's period (through
/// [`machine::Machine::frame_ns`], so not a match), the inputs, the layer mask, the
/// host ring's counters, the pen conversion, save, load, screenshot and the state tag.
/// The rest is board-agnostic because [`machine::Machine`] dispatches it.
///
/// `audio` is `&mut dyn` where `d` is `impl`: `main` picks its sink at runtime — a real
/// device or [`crate::audio::NullAudio`] when one cannot be opened — so it holds a
/// `Box<dyn Audio>`, and a generic parameter would make that the caller's problem.
pub fn run(m: &mut Machine, d: &mut impl Display, audio: &mut dyn Audio, o: &LoopOpts) -> Summary {
    // `main` picks the board and the tag independently — one from the ROM set, one
    // from `state_tag` — and a `LoopOpts` carrying SF2's tag beside an `Sf1` compiles,
    // runs, and writes states the same machine will then refuse to load. This is the
    // check that they were picked together.
    //
    // `debug_assert` and not `assert`: a mismatch is a bug in `main` that no user
    // input can cause, and a panic here takes someone's window down mid-game.
    debug_assert_eq!(
        o.board,
        state_tag(m.board()),
        "the options' board tag does not match the machine's board"
    );

    // `frame_ns()` and not `FramePacer::cps1()`: the two boards' frame times differ —
    // SF1's 60.0 Hz against CPS-1's 59.63 — and the machine is the one place that
    // knows which. `cps1()` stays in `frontend` for its own tests.
    let mut pacer = FramePacer::new(m.frame_ns(), MAX_CATCH_UP);
    let mut controls = Controls::new();
    // The DIP switches the machine was built with, carried into every frame's
    // `Inputs`. Read from the machine rather than named here: `Controls::update`
    // returns a whole `Inputs` that the loop assigns over the board's own, so
    // whatever `main` configured at construction would be gone after one frame.
    //
    // Taken from the machine and not from a constant because the boards differ —
    // `Sf1Inputs::from_shared` maps a different port block — so which switches are
    // right is `main`'s decision, made once, per game.
    if let Machine::Cps1(c) = &*m {
        controls.set_dsw(c.board.inputs.dsw);
    }
    let mut buf: Vec<u32> = Vec::new();
    let mut paused = false;
    let mut summary = Summary::default();
    let mut title = String::new();
    let mut dbg = Debugger::new();
    let mut gfx = GfxViewer::new();

    while d.is_open() {
        let elapsed = d.elapsed_ns();
        let a: Actions = controls.update(d.held_keys());
        if a.quit {
            break;
        }

        // Level-triggered, so this happens whether or not a frame runs. The two
        // boards read different structs — SF1 puts starts and service on SYSTEM and
        // two of the six attack buttons on IN0 — and `Sf1Inputs::from_shared` is
        // where that knowledge lives. See `machine::sf1::inputs`.
        match &mut *m {
            Machine::Cps1(c) => c.board.inputs = a.inputs,
            Machine::Sf1(f) => {
                f.board.inputs = machine::sf1::inputs::Sf1Inputs::from_shared(&a.inputs);
            }
        }

        if a.reset {
            m.reset();
            // The pacer too: the wall-clock time spent deciding to press F3 is not
            // game time the fresh machine owes.
            pacer.reset();
        }
        if a.pause_toggled {
            paused = !paused;
            // Whichever way it went, so a resume starts from a clean slate.
            //
            // What this discards is the sub-frame *remainder* held from before the
            // pause — at most one frame's worth. The time spent paused is not owed
            // and never was: `tick` is not called on a paused iteration, so the
            // debt cannot accrue in the first place. Worth stating, because "a
            // minute paused would be a minute owed" is the plausible reason for
            // this line and it is not the real one.
            pacer.reset();
        }

        if a.save {
            save(m, o, &mut summary);
        }
        if a.load {
            load(m, o, &mut summary);
        }

        // Before the frames: a breakpoint set on this tick must be honoured by this
        // tick's frames, not by the next tick's. `dbg` reads the machine and never
        // writes it — see `frontend::debug`.
        dbg.update(&a, &m.cpu_view());

        gfx.update(&a, m);
        // The mask is a view setting the loop applies, not something `frontend`
        // reaches into the machine to set. Before `render`, so this tick's frame is
        // the masked one. Two mask types because the boards have different layers:
        // CPS-1 has three scroll planes and a star field, SF1 has BG, FG, sprites and
        // a text layer. `GfxViewer` keeps both and hands over whichever is asked for.
        match &mut *m {
            Machine::Cps1(c) => c.video.enable = gfx.mask(),
            Machine::Sf1(f) => f.video.enable = gfx.sf1_mask(),
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
        // Counted separately from `frames`, which is what this tick *owed*. A frame the
        // breakpoint cut in half is not a frame the machine ran, and a summary that said
        // otherwise would make the frame count disagree with `total_cycles`.
        let mut ran = 0u32;
        for _ in 0..frames {
            if run_frame_to_breakpoint(m, &mut dbg) {
                // A breakpoint stopped this frame part-way through. Pause, so the next
                // tick does not immediately run on past it, and abandon the frames this
                // tick still owed: they are host time the user is no longer watching.
                paused = true;
                // And the debt with them, for the same reason the pause key resets it:
                // otherwise resuming with `P` replays whatever this tick had left.
                pacer.reset();
                break;
            }
            ran += 1;
        }
        summary.frames += u64::from(ran);

        // One instruction, on F4's edge. Not inside the loop above and not conditional
        // on `paused`: stepping while running is meaningless but harmless, and a guard
        // would be a second place for the pause state to be interpreted.
        if a.step_instruction {
            m.step_instruction();
            // The step moved the machine, so the breakpoint at the instruction just
            // *arrived* at must not fire on the next tick as if it were fresh — you
            // asked to be here.
            dbg.note_stopped(&m.cpu_view());
        }

        // The pause reaches the sink every tick it is in effect, not on its edge: the
        // device callback reads the flag once per callback, and a pause reported once and
        // then forgotten would go on counting a deliberately empty ring as underruns.
        audio.set_paused(paused);
        // After the frames, so this is the audio this tick produced, and drained rather
        // than read, so the machine's buffer does not grow for the whole session. Empty
        // when no frame ran, and an empty queue is not a call: a sink cannot tell "the
        // emulator is paused" from "the emulator produced silence" if both arrive as an
        // empty slice.
        //
        // A failure is a notice, not a stop — `note`, so four hundred dead-device frames
        // are one line. A dropped buffer is a click, and ending someone's session over a
        // click would be the worse failure.
        let samples = m.drain_samples();
        if !samples.is_empty() {
            if let Err(e) = audio.queue(&samples) {
                note(&mut summary, format!("audio: {e}"));
            }
        }
        // The ring's counters, handed to the machine so `F1`'s sound panel can show
        // them beside the chip's own clip count. They are the host's, not the board's —
        // the ring is sized from a sample rate `machine` has no business knowing — which
        // is why they arrive through a setter rather than being counted there.
        //
        // After the queue and before the render, so the panel drawn this tick shows the
        // drops this tick's push caused rather than last tick's. Every iteration, paused
        // included: a panel opened while paused would otherwise read zero and look like
        // a clean run.
        let stats = audio.stats();
        match &mut *m {
            Machine::Cps1(c) => c.sound.set_audio_stats(stats.drops, stats.underruns),
            // Not `f.fm.set_audio_stats(..)`: SF1 has two sound boards and the host's
            // ring is downstream of the mix, so it belongs to neither and lives on the
            // machine. CPS-1 has one board to hang it on, which is why that arm is
            // the one that looks different from this file's other matches.
            Machine::Sf1(f) => f.set_audio_stats(stats.drops, stats.underruns),
        }

        // Outside the loop above: a paused iteration renders too. The frame does not
        // change, but the window is redrawn, and a windowing library that is not
        // given a buffer shows an undefined one.
        m.render();
        // Two converters and not one generic: the palettes are different sizes and
        // different formats — CPS-1's 3,072 entries against SF1's 1,024, and SF1
        // expands each nibble to `(n << 4) | n` where CPS-1 does not.
        match &*m {
            Machine::Cps1(c) => pens_to_argb(&c.video, &mut buf),
            Machine::Sf1(f) => pens_to_argb_sf1(&f.video, &mut buf),
        }
        // After the conversion, never before: the overlay's pixels are already
        // `0x00RRGGBB`, while the video's are pens. Drawn into the pen buffer they
        // would be run through the palette and come out as whatever colours those
        // indices happen to name.
        dbg.draw(&mut buf, &m.cpu_view(), &|a| m.peek_word(a), m);
        // Over the debugger, not under it: both are opaque, and this one is the
        // whole screen while E2's are corners of it.
        gfx.draw(&mut buf, m);
        if let Err(e) = d.present(&buf) {
            note(&mut summary, format!("cannot present a frame: {e}"));
            break;
        }

        if a.screenshot {
            screenshot(m, o, &mut summary);
        }

        summary.dropped = pacer.dropped();
        let want = title_for(&summary, &m.cpu_view(), paused, audio.is_running());
        if want != title {
            d.set_title(&want);
            title = want;
        }
    }

    summary
}

/// Runs one frame, or stops early at a breakpoint. Returns whether it stopped.
///
/// Instruction by instruction rather than `Machine::run_frame`, because a breakpoint that
/// only stopped at frame boundaries would be the `.` key with extra steps: the whole
/// point is to stop *at* the instruction, 167,680 cycles into the middle of a frame if
/// that is where it is.
///
/// The cost is real and worth naming: this is a `should_break` call per instruction
/// against a `Vec` of breakpoints, where `run_frame` is a tight loop. With no
/// breakpoints set the check is a scan of an empty `Vec`, which is why the fast path is
/// still a fast path — and `watching_the_machine_does_not_change_it` is what proves
/// this path and `run_frame` reach the same machine.
fn run_frame_to_breakpoint(m: &mut Machine, dbg: &mut Debugger) -> bool {
    let start_frames = m.frames();
    // `line` alone cannot say when a frame is done: a breakpoint on the very first
    // instruction of line 0 leaves `line == 0`, which is where the frame started, and a
    // loop watching for the wrap would run a whole extra frame. The frame counter moves
    // exactly once per wrap.
    //
    // `m.frames()` and not a board's `trace.frames`: `Machine::frames` reads exactly
    // that field, on whichever board, so this and `m.cpu_view().trace.frames` cannot
    // disagree.
    while m.frames() == start_frames {
        if dbg.should_break(&m.cpu_view()) {
            dbg.note_stopped(&m.cpu_view());
            return true;
        }
        m.step_instruction();
    }
    false
}

/// The title bar.
///
/// Only the states worth interrupting someone's game to mention: paused, because
/// the picture stopped and you want to know why; dropped frames, because the host
/// cannot keep up and that is not the emulator's bug; halted, because a 68000 that
/// double bus faulted will never execute another instruction and the window would
/// otherwise just freeze; and no audio device, because otherwise "I hear nothing" is
/// unattributable between a device that would not open, a game that is silent, and a
/// mix that is broken. The `eprintln!` in `main` names the reason and is gone from the
/// scrollback five minutes later; this stays.
/// ⚠️ A [`machine::CpuView`] and not a `&Machine`: this reads exactly one field, and
/// `CpuView` is the narrow type Task 16 built for that. A `&Machine` parameter would
/// let a later edit reach the whole board from the title bar.
fn title_for(s: &Summary, v: &machine::CpuView<'_>, paused: bool, audio: bool) -> String {
    let mut t = String::from("sfemu");
    if paused {
        t.push_str(" [paused]");
    }
    if v.cpu.halted {
        t.push_str(" [CPU halted]");
    }
    if !audio {
        t.push_str(" [no audio]");
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

/// Writes the machine's state, or notes why it could not.
///
/// # Why the two boards' codecs have different signatures
///
/// CPS-1's [`frontend::encode`] takes the board tag; SF1's [`frontend::encode_sf1`]
/// does not. That is Task 19's decision and it is right: SF1's payload has one
/// hand-counted length, `unframe` validates the tag and the length in the same call,
/// and there is exactly one board that length belongs to. A `board` parameter on
/// `encode_sf1` could only ever be `BOARD_SF1`.
///
/// The consequence is here: `o.board` is read on one arm. [`run`]'s `debug_assert_eq!`
/// is what keeps it honest on the other.
fn save(m: &Machine, o: &LoopOpts, s: &mut Summary) {
    let bytes = match m {
        Machine::Cps1(c) => frontend::encode(&c.snapshot(), o.board),
        Machine::Sf1(f) => frontend::encode_sf1(&f.snapshot()),
    };
    match std::fs::write(&o.state_path, &bytes) {
        Ok(()) => {}
        Err(e) => note(s, format!("cannot write `{}`: {e}", o.state_path.display())),
    }
}

/// Restores the machine's state, or notes why it could not.
///
/// The machine is left untouched on any failure. A partial restore would be a machine
/// that is neither the saved one nor the running one, and the loop would carry on
/// running it.
fn load(m: &mut Machine, o: &LoopOpts, s: &mut Summary) {
    let bytes = match std::fs::read(&o.state_path) {
        Ok(b) => b,
        Err(e) => {
            note(s, format!("cannot read `{}`: {e}", o.state_path.display()));
            return;
        }
    };
    // The decode and the restore are inside one arm each, not a decode followed by a
    // shared restore: the two codecs return different types — `MachineState` and
    // `Sf1State` — and there is no supertype, deliberately. A state's shape is the
    // board's, and a common one would be a struct with two thirds of its fields unused
    // on whichever board was running.
    //
    // The notice text is identical on both arms and duplicated on purpose: it names the
    // path and carries the codec's own reason, which is what
    // `a_corrupt_state_file_does_not_stop_the_loop` asserts on. A closure over `s`
    // would borrow it mutably across both arms while `note` also wants it, and the
    // reader's question at this line — "what happens when a load fails" — is answered
    // by two visible arms.
    match m {
        Machine::Cps1(c) => match frontend::decode(&bytes, o.board) {
            Ok(state) => c.restore(&state),
            Err(e) => note(s, format!("cannot load `{}`: {e}", o.state_path.display())),
        },
        Machine::Sf1(f) => match frontend::decode_sf1(&bytes) {
            Ok(state) => f.restore(&state),
            Err(e) => note(s, format!("cannot load `{}`: {e}", o.state_path.display())),
        },
    }
}

fn screenshot(m: &Machine, o: &LoopOpts, s: &mut Summary) {
    // Two writers, not one: CPS-1's `ppm` asks `Video::rgb()` for a whole RGB buffer
    // and SF1's `ppm_sf1` asks `Sf1Video::rgb(pen)` per pen. Different shapes, because
    // a never-rendered SF1 frame holds CPS-1's `BACKGROUND_PEN` — past SF1's 1,024
    // entries — and the per-pen form answers black for it where an index would be out
    // of bounds. `ppm_sf1`'s own doc says so.
    let ppm = match m {
        Machine::Cps1(c) => crate::ppm(&c.video),
        Machine::Sf1(f) => crate::ppm_sf1(&f.video),
    };
    if let Err(e) = std::fs::write(&o.shot_path, &ppm) {
        note(s, format!("cannot write `{}`: {e}", o.shot_path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The tests that predate audio pass a sink that discards: what they assert is the
    // loop's frame, pause and save behaviour, and a recording fake in all forty of them
    // would say nothing they check. The audio tests below use `FakeAudio`.
    use crate::audio::NullAudio;
    use frontend::keys::Key;
    // Under `#[cfg(test)]` because production code in this file no longer names the
    // type: the fixtures build a `Cps1` and `cps1`/`cps1_mut` hand it back, but every
    // one of `run`'s own board decisions is a match on `Machine`.
    use frontend::FRAME_NS;
    use machine::video::{HEIGHT, WIDTH};
    use machine::Cps1;
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
        /// The first buffer, in full. Only the first: 86,016 pixels a tick is a lot
        /// to keep, and one is enough to ask whether it was rendered.
        first: Option<Vec<u32>>,
        /// And the last, for a claim that takes several ticks to set up: reaching a
        /// graphics view and subtracting a layer is five key presses, and the frame
        /// that shows it is the one after them. Two frames, not sixty.
        last: Option<Vec<u32>>,
        titles: Vec<String>,
    }

    impl Fake {
        /// A script from `(keys, elapsed)` pairs.
        fn new(script: Vec<(KeySet, u64)>) -> Self {
            Self {
                script,
                tick: 0,
                presented: Vec::new(),
                first: None,
                last: None,
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
            if self.first.is_none() {
                self.first = Some(buf.to_vec());
            }
            self.last = Some(buf.to_vec());
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
    fn machine() -> Machine {
        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x68..0x6C].copy_from_slice(&[0x00, 0x00, 0x11, 0x00]);
        rom[0x1000..0x100E].copy_from_slice(&[
            0x46, 0xFC, 0x20, 0x00, 0x52, 0x40, 0x33, 0xC0, 0x00, 0xFF, 0x00, 0x00, 0x60, 0xF6,
        ]);
        rom[0x1100..0x1104].copy_from_slice(&[0x52, 0x41, 0x4E, 0x73]);
        let mut m = Cps1::new(&rom, BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        wrap(m)
    }

    /// A machine whose rendered frame is not one flat colour.
    ///
    /// The plain fixture draws nothing, so its frame is uniform and a presented
    /// buffer cannot say whether it was rendered or is the framebuffer's power-on
    /// fill. This one has a sprite and a palette entry, so a rendered frame and an
    /// unrendered one are distinguishable — which is what
    /// `a_paused_tick_presents_a_rendered_frame` needs.
    ///
    /// The program is the plain fixture's, unchanged: what is under test is the
    /// loop's ordering, not the guest.
    fn machine_that_draws() -> Machine {
        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x68..0x6C].copy_from_slice(&[0x00, 0x00, 0x11, 0x00]);
        rom[0x1000..0x100E].copy_from_slice(&[
            0x46, 0xFC, 0x20, 0x00, 0x52, 0x40, 0x33, 0xC0, 0x00, 0xFF, 0x00, 0x00, 0x60, 0xF6,
        ]);
        rom[0x1100..0x1104].copy_from_slice(&[0x52, 0x41, 0x4E, 0x73]);

        // A 16x16 tile solid in pen 0x0A.
        let mut gfx = vec![0u8; 128];
        for row in 0..16 {
            for half in [0usize, 4] {
                gfx[row * 8 + half + 1] = 0xFF;
                gfx[row * 8 + half + 3] = 0xFF;
            }
        }

        let cfg = BoardConfig::sf2();
        let mut m = Cps1::with_gfx(&rom, gfx, cfg, Timing::cps1_10mhz());
        m.reset();
        m.board.cps_a[machine::video::regs::OBJ_BASE] = 0x40;
        m.board.cps_a[machine::video::regs::PALETTE_BASE] = 0;
        m.board.gfxram[0x2000] = WIDTH as u16 / 2;
        m.board.gfxram[0x2001] = HEIGHT as u16 / 2;
        m.board.gfxram[0x2002] = 0;
        m.board.gfxram[0x2003] = 3;
        m.board.gfxram[0x2007] = 0xFF00;
        m.board.cps_b[cfg.video.palette_control] = 0x0001;
        m.board.gfxram[0x3A] = 0x0F0F;
        // The sprite table is latched at vblank, so one frame must pass before the
        // renderer has anything to draw. Run it here rather than in the loop: the
        // test's point is a *paused* tick, which runs no frames at all.
        m.run_frame();
        wrap(m)
    }

    /// The address the breakpoint tests stop at: the top of the fixture's loop.
    ///
    /// It is also where `reset` leaves the machine, which is what makes `F7` — the only
    /// way to reach the loop's own `Debugger` — able to set a breakpoint there.
    const TARGET: u32 = 0x1000;

    /// A machine whose loop body costs more than a scanline.
    ///
    /// Load-bearing for `a_breakpoint_stops_the_loop_mid_frame`: with a short loop the
    /// machine returns to [`TARGET`] within a few dozen cycles, the beam is still on line
    /// 0, and "it stopped mid-frame" cannot be told from "it stopped at a frame
    /// boundary". The `dbra` delay loop is ~2,000 cycles, which is three scanlines.
    ///
    /// Interrupts stay masked — `reset` leaves the SR at supervisor level 7 and nothing
    /// here unmasks them — so the only thing that moves the PC is this program.
    ///
    /// ```text
    /// 1000  5240        addq.w #1,d0     <- TARGET
    /// 1002  323C 00C8   move.w #200,d1
    /// 1006  51C9 FFFE   dbra d1,$1006
    /// 100A  60F4        bra $1000
    /// ```
    fn machine_with_a_long_loop() -> Machine {
        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        // Both displacements are from the word *after* the opcode: the `dbra` at 0x1006
        // counts from 0x1008 back to 0x1006, which is -2 = 0xFFFE; the `bra` at 0x100A
        // counts from 0x100C back to 0x1000, which is -12 = 0xF4.
        rom[0x1000..0x100C].copy_from_slice(&[
            0x52, 0x40, // addq.w #1,d0
            0x32, 0x3C, 0x00, 0xC8, // move.w #200,d1
            0x51, 0xC9, 0xFF, 0xFE, // dbra d1,$1006
            0x60, 0xF4, // bra $1000
        ]);
        let mut m = Cps1::new(&rom, BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        wrap(m)
    }

    /// A board inside a [`Machine`], which is what `run` takes.
    ///
    /// The fixtures build a `Cps1` because that is what has a ROM, a `gfxram` and a
    /// `cps_a` to set up; `run` takes a `Machine`. The tests then read the board back out
    /// with [`cps1`] and [`cps1_mut`].
    fn wrap(m: Cps1) -> Machine {
        Machine::Cps1(Box::new(m))
    }

    /// The CPS-1 board inside a fixture's machine.
    ///
    /// ⚠️ **Not the `cps1`/`cps1_ref` pair Task 18 put in production code** — this task
    /// deletes those. This is a test-module helper over fixtures that build exactly one
    /// arm, and it exists so that ~130 field reads across ~55 tests stay field reads. A
    /// test that matched on [`Machine`] in a hundred places to reach
    /// `m.board.trace.frames` would be a test of the enum.
    ///
    /// `frontend`'s `gfx.rs` has the same pair for the same reason.
    fn cps1(m: &Machine) -> &Cps1 {
        match m {
            Machine::Cps1(c) => c,
            Machine::Sf1(_) => unreachable!("this fixture builds a Cps1"),
        }
    }

    /// The same, mutable. See [`cps1`].
    fn cps1_mut(m: &mut Machine) -> &mut Cps1 {
        match m {
            Machine::Cps1(c) => c,
            Machine::Sf1(_) => unreachable!("this fixture builds a Cps1"),
        }
    }

    /// An SF1 with just enough program to run a frame.
    ///
    /// Deliberately Task 18's `an_sf1_machine`, byte for byte in the parts that matter
    /// for a machine that must merely run: a two-instruction 68000 program, `jr -2` on
    /// the FM Z80, a `nop` and `jr -2` on the ADPCM one, and five empty graphics
    /// regions. Two fixtures that differed would leave a failure here and a pass there
    /// with nothing to compare.
    ///
    /// No ROM: the program is written inline and every region is `Vec::new()`.
    ///
    /// ⚠️ **The `move.w` target is 0xFF8000, not the CPS-1 fixture's 0xFF0000.** SF1's
    /// map starts RAM at 0xFF8000, so CPS-1's address is this board's *unmapped* space,
    /// and a fixture that quietly logged an unmapped write per frame would be a trap for
    /// the next test written against it. `0xFFE000` in the SSP for the same reason: that
    /// is `objectram`'s base, which is the top of RAM, which is where a stack goes.
    fn an_sf1_machine() -> Machine {
        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0xE0, 0x00, 0x00, 0x00, 0x10, 0x00]);
        // Vector 0x64 and not 0x68: SF1's vblank is level 1. `sf1/board.rs` says so.
        rom[0x64..0x68].copy_from_slice(&[0x00, 0x00, 0x11, 0x00]);
        rom[0x1000..0x100E].copy_from_slice(&[
            0x46, 0xFC, 0x20, 0x00, 0x52, 0x40, 0x33, 0xC0, 0x00, 0xFF, 0x80, 0x00, 0x60, 0xF6,
        ]);
        rom[0x1100..0x1104].copy_from_slice(&[0x52, 0x41, 0x4E, 0x73]);
        let mut m = machine::Sf1::new(
            &rom,
            machine::video::sf1::Sf1Video::new(
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            vec![0x18, 0xFE],
            vec![0x00, 0x18, 0xFE],
        );
        m.reset();
        Machine::Sf1(Box::new(m))
    }

    /// An SF1 whose rendered frame is not one flat colour.
    ///
    /// [`an_sf1_machine`]'s counterpart to [`machine_that_draws`], and needed for the
    /// same reason: a frame of pen 0 cannot distinguish "converted" from "zeroed", or
    /// "masked" from "unmasked".
    ///
    /// The text plane, because it is the cheapest of the four to make draw: its tile
    /// map lives in guest `videoram` rather than in a tilerom, so one word there and a
    /// 2-plane 8×8 tile ROM are the whole setup. `video::sf1`'s own render tests use
    /// this cell and this entry.
    ///
    /// - `active = ACTIVE_TX` — the hardware's own enable, which the mask ANDs with.
    /// - `videoram[136] = 0x2001` — TX cell 136 is the screen origin; colour 2, code 1.
    /// - `palette[776] = 0x0135` — colour 2 × granularity 4 + base 768. `Sf1Video::rgb`
    ///   returns black for an unwritten entry, so a drawn pixel with no palette entry
    ///   is indistinguishable from an undrawn one.
    fn an_sf1_machine_that_draws() -> Machine {
        use machine::video::sf1::Plane;

        // The same 8×8 2-plane tile ROM shape `video::sf1`'s tests use: `pen(x, y)` is
        // `(x + y) & 3`, so pixel (0,0) of any code is pen 0 and (1,0) is pen 1. Two
        // codes, because cell 136's entry names code 1.
        let mut tx = vec![0u8; 2 * 128 / 8 * 2];
        let layout = Plane::Tx.layout();
        for code in 0..2u32 {
            for y in 0..layout.height {
                for x in 0..layout.width {
                    let pen = (x + y) & 3;
                    for p in 0..layout.planes {
                        if pen & (1 << (layout.planes - 1 - p)) == 0 {
                            continue;
                        }
                        let off = (code * layout.char_increment
                            + layout.plane_offsets[p as usize].bits
                            + y * layout.y_step
                            + layout.x_offsets[x as usize])
                            as usize;
                        tx[off / 8] |= 0x80 >> (off % 8);
                    }
                }
            }
        }

        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0xE0, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x64..0x68].copy_from_slice(&[0x00, 0x00, 0x11, 0x00]);
        rom[0x1000..0x100E].copy_from_slice(&[
            0x46, 0xFC, 0x20, 0x00, 0x52, 0x40, 0x33, 0xC0, 0x00, 0xFF, 0x80, 0x00, 0x60, 0xF6,
        ]);
        rom[0x1100..0x1104].copy_from_slice(&[0x52, 0x41, 0x4E, 0x73]);
        let mut m = machine::Sf1::new(
            &rom,
            machine::video::sf1::Sf1Video::new(Vec::new(), Vec::new(), Vec::new(), tx, Vec::new()),
            vec![0x18, 0xFE],
            vec![0x00, 0x18, 0xFE],
        );
        m.reset();
        m.board.active = machine::video::sf1::ACTIVE_TX;
        m.board.videoram[136] = 0x2001;
        m.board.palette[776] = 0x0135;
        Machine::Sf1(Box::new(m))
    }

    /// The SF1 board inside [`an_sf1_machine`]'s machine.
    ///
    /// The mirror of [`cps1`], for the same reason: the SF1-arm tests read fields —
    /// `board.inputs.in0()`, `audio_drops()` — and the fixture builds exactly one arm.
    fn sf1(m: &Machine) -> &machine::Sf1 {
        match m {
            Machine::Sf1(f) => f,
            Machine::Cps1(_) => unreachable!("this fixture builds an Sf1"),
        }
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
            // Every fixture here builds a CPS-1 except `an_sf1_machine`, whose one test
            // overwrites this field. `run`'s assertion checks the pairing either way.
            board: state_tag(machine::BoardKind::Cps1),
        };
        (o, state, shot)
    }

    /// One tick of one frame's time runs one frame.
    #[test]
    fn an_ordinary_tick_runs_one_frame() {
        let (o, _s, _p) = opts("ordinary");
        let mut m = machine();
        let mut d = Fake::new(Fake::idle(1));
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
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
        let mut script = vec![Fake::held(&[Key::F11])];
        script.extend(Fake::idle(2));
        script.push(Fake::held(&[Key::F11]));
        script.extend(Fake::idle(2));
        let mut d = Fake::new(script);
        let s = run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
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
        let mut script = vec![Fake::held(&[Key::F11])];
        // Held for three ticks: one frame, because a step is an edge and not a
        // level. Then released and pressed again: a second edge, a second frame.
        script.extend(vec![Fake::held(&[Key::Period]); 3]);
        script.extend(Fake::idle(1));
        script.push(Fake::held(&[Key::Period]));
        let mut d = Fake::new(script);
        let s = run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
        assert_eq!(s.frames, 2, "one frame per press, not per tick held");
    }

    /// A step does not unpause.
    #[test]
    fn a_step_does_not_unpause() {
        let (o, _s, _p) = opts("step-pause");
        let mut script = vec![Fake::held(&[Key::F11]), Fake::held(&[Key::Period])];
        script.extend(Fake::idle(3));
        let mut d = Fake::new(script);
        let s = run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
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
        let s = run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
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
        let fresh = (cps1(&m).cpu.pc, cps1(&m).cpu.prefetch);
        let mut script = Fake::idle(3);
        // Zero elapsed on the F3 tick: the reset happens before the frame count is
        // decided, so a tick that owed a frame would run one *after* resetting and
        // `total_cycles` would be one frame rather than zero. Zero host time is what
        // isolates the reset from the frame that follows it.
        script.push((KeySet::from_keys(&[Key::F3]), 0));
        let mut d = Fake::new(script);
        run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert_eq!(cps1(&m).total_cycles, 0, "the cycle count restarts");
        // 0x1004 and not 0x1000: `M68k::reset` refills the prefetch queue, which
        // advances the PC past the two words it read. Compared against a freshly
        // reset machine rather than against a literal, so this states "F3 is a power
        // cycle" rather than restating the core's prefetch convention — which is
        // `m68k`'s to change.
        assert_eq!(
            cps1(&m).cpu.pc,
            fresh.0,
            "the PC is where power-on leaves it"
        );
        assert_eq!(cps1(&m).cpu.prefetch, fresh.1, "and so is the queue");
        assert_eq!(cps1(&m).line, 0, "and the beam is at the top of a frame");
        // Not the trace: `reset` deliberately does not clear it, because the trace
        // records the session and a reset is part of the session.
        assert!(
            cps1(&m).board.trace.vblanks > 0,
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
        run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
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
        script.push(Fake::held(&[Key::F11]));
        script.extend(Fake::idle(3));
        let ticks = script.len();
        let mut d = Fake::new(script);
        run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
        assert_eq!(d.presented.len(), ticks, "one present per tick");
        assert!(
            d.presented.iter().all(|&n| n == WIDTH * HEIGHT),
            "every frame is {} pixels, got {:?}",
            WIDTH * HEIGHT,
            d.presented
        );
        assert_eq!(WIDTH * HEIGHT, 86_016, "384 x 224");
    }

    /// A paused tick presents a *rendered* frame, not a blank one.
    ///
    /// `presented.len()` cannot see this: a loop that rendered only when frames ran
    /// still presents a buffer every tick — it just presents the same stale one. The
    /// pixels are the artifact, and this fixture's frame is not uniform, so a
    /// rendered frame and an unrendered one differ.
    #[test]
    fn a_paused_tick_presents_a_rendered_frame() {
        let (o, _s, _p) = opts("paused-render");
        let mut m = machine_that_draws();
        // The very first tick pauses, so no frame runs inside the loop at all.
        let mut d = Fake::new(vec![Fake::held(&[Key::F11])]);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert_eq!(s.frames, 0, "the premise: the loop ran no frames");
        let first = d.first.expect("a paused tick still presents");
        assert!(
            first.iter().any(|&px| px != first[0]),
            "a rendered frame is not one flat colour"
        );
    }

    /// Unpausing discards the frame debt held from before the pause.
    ///
    /// A sub-frame remainder is the observable. Before pausing, a tick of
    /// `FRAME_NS - 1` runs no frame and leaves that much owed; after resuming, a tick
    /// of 1 ns completes it. With the pacer reset, the remainder is gone and no frame
    /// runs; without it, the two add to exactly one frame's worth and one does.
    ///
    /// One frame, not a burst, because `tick` keeps only the remainder — which is
    /// also why this is the *only* way to see the reset. A long pause is already
    /// discarded by never calling `tick` while paused.
    #[test]
    fn unpausing_discards_the_debt_from_before_the_pause() {
        let (o, _s, _p) = opts("pause-debt");
        let mut d = Fake::new(vec![
            (KeySet::new(), FRAME_NS - 1),
            Fake::held(&[Key::F11]),
            Fake::held(&[]),
            (KeySet::from_keys(&[Key::F11]), 1),
        ]);
        let s = run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
        assert_eq!(
            s.frames, 0,
            "the pre-pause remainder must not complete a frame after the resume"
        );
    }

    /// And that remainder really would complete a frame without the pause.
    ///
    /// The premise for the test above: if `FRAME_NS - 1` then `1` did not add to a
    /// frame in the first place, that test would pass for a loop that never resets
    /// the pacer at all.
    #[test]
    fn the_remainder_completes_a_frame_when_nothing_is_paused() {
        let (o, _s, _p) = opts("pace-remainder");
        let mut d = Fake::new(vec![(KeySet::new(), FRAME_NS - 1), (KeySet::new(), 1)]);
        let s = run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
        assert_eq!(s.frames, 1, "the two ticks are one frame between them");
    }

    /// The title reports dropped frames — and only when there are some.
    #[test]
    fn the_title_reports_dropped_frames() {
        let (o, _s, _p) = opts("title-drop");
        let mut d = Fake::new(vec![(KeySet::new(), 2_000_000_000)]);
        run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
        assert!(
            d.titles.iter().any(|t| t.contains("115 dropped")),
            "a stall must say so: {:?}",
            d.titles
        );

        // And an ordinary run does not. A title that always mentioned drops would
        // be noise, and a test that only checked the stall case would pass for one.
        let (o, _s, _p) = opts("title-quiet");
        let mut d = Fake::new(Fake::idle(4));
        run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
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
        let mut script = vec![Fake::held(&[Key::F11])];
        script.extend(Fake::idle(1));
        let mut d = Fake::new(script);
        run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
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
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert!(
            s.notices.is_empty(),
            "the save must succeed: {:?}",
            s.notices
        );
        assert!(o.state_path.exists(), "and leave a file behind");
        let after = (
            cps1(&m).total_cycles,
            cps1(&m).cpu.d[0],
            cps1(&m).cpu.d[1],
            cps1(&m).line,
        );
        assert_ne!(after.0, 0, "the premise: the machine moved");

        // Run four more, then load and run the same four again.
        let mut d = Fake::new(Fake::idle(4));
        run(&mut m, &mut d, &mut NullAudio::default(), &o);
        let mut script = vec![Fake::held(&[Key::F8])];
        script.extend(Fake::idle(4));
        let mut d = Fake::new(script);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert!(
            s.notices.is_empty(),
            "the load must succeed: {:?}",
            s.notices
        );
        assert_eq!(
            (
                cps1(&m).total_cycles,
                cps1(&m).cpu.d[0],
                cps1(&m).cpu.d[1],
                cps1(&m).line
            ),
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
            board: state_tag(machine::BoardKind::Cps1),
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
        let s = run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
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
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
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
        let mut bytes = frontend::encode(&cps1(&m).snapshot(), o.board);
        bytes[100_000] ^= 0x01;
        std::fs::write(&o.state_path, &bytes).expect("temp dir is writable");

        let before = (cps1(&m).total_cycles, cps1(&m).cpu.d[0], cps1(&m).line);
        // Zero elapsed, so the tick owes no frame: what this test asserts is that
        // the *load* left the machine alone, and a frame run afterwards would move
        // it for a reason that has nothing to do with the load.
        let mut d = Fake::new(vec![(KeySet::from_keys(&[Key::F8]), 0)]);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert_eq!(s.notices.len(), 1, "{:?}", s.notices);
        assert_eq!(s.frames, 0, "the load tick owed nothing");
        assert_eq!(
            (cps1(&m).total_cycles, cps1(&m).cpu.d[0], cps1(&m).line),
            before,
            "a refused load must not partially apply"
        );
    }

    /// A saved state names the board it came from.
    ///
    /// Nothing else here reads the tag off disk. `save` and `load` both use
    /// `o.board`, so a round trip agrees with itself whatever it holds — the file could
    /// be tagged `SF1\0` and every other test in this module would still pass. The tag
    /// exists so that loading one board's state into another build is *refused*,
    /// which is a claim about the bytes on disk and has to be read off the bytes.
    ///
    /// Read at the documented offset rather than by searching the file: the board
    /// field's position is the format's, and `frontend`'s own
    /// `the_header_is_laid_out_as_documented` is what pins the offset to 8.
    #[test]
    fn a_saved_state_is_tagged_with_this_build_s_board() {
        let (o, _s, _p) = opts("board-tag");
        let mut d = Fake::new(vec![Fake::held(&[Key::F5])]);
        let s = run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
        assert!(s.notices.is_empty(), "{:?}", s.notices);

        let bytes = std::fs::read(&o.state_path).expect("F5 must write the file");
        let tag = u32::from_le_bytes(bytes[8..12].try_into().expect("four bytes"));
        assert_eq!(
            tag,
            frontend::BOARD_SF2,
            "the file must name the board this build runs, not {tag:#010x}"
        );
        // And in the form the format documents: big-endian ASCII, so a hex dump of
        // the file reads `SF2\0` rather than four unrelated bytes.
        assert_eq!(&tag.to_be_bytes(), b"SF2\0");
    }

    /// A state tagged for another board is refused, and the machine keeps running.
    ///
    /// The other half of the same claim: the tag is not decoration, it is a check
    /// `load` applies. Written by hand rather than by encoding under a different
    /// constant, because building a second board to produce one byte pattern would test
    /// `Sf1`'s codec rather than this loop's refusal.
    #[test]
    fn another_boards_state_is_refused_by_the_loop() {
        let (o, _s, _p) = opts("board-refuse");
        let mut m = machine();
        m.run_frame();
        let mut bytes = frontend::encode(&cps1(&m).snapshot(), o.board);
        // `SF1\0`, written by hand rather than by building an SF1 and encoding it:
        // what is under test is that *this* machine refuses a foreign tag, and the
        // sibling below is the same claim from SF1's side.
        bytes[8..12].copy_from_slice(&0x5346_3100_u32.to_le_bytes());
        std::fs::write(&o.state_path, &bytes).expect("temp dir is writable");

        let before = (cps1(&m).total_cycles, cps1(&m).cpu.d[0], cps1(&m).line);
        let mut d = Fake::new(vec![(KeySet::from_keys(&[Key::F8]), 0)]);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert_eq!(s.notices.len(), 1, "{:?}", s.notices);
        assert!(
            s.notices[0].contains("board"),
            "the notice must say it is the board: {}",
            s.notices[0]
        );
        assert_eq!(
            (cps1(&m).total_cycles, cps1(&m).cpu.d[0], cps1(&m).line),
            before,
            "and the running machine is untouched"
        );
    }

    /// The SF1 loop refuses a CPS-1 state, and keeps running.
    ///
    /// The mirror of [`another_boards_state_is_refused_by_the_loop`]. Both directions,
    /// because a codec that checked the tag in only one is a codec that lets exactly
    /// one of the two dangerous loads through — and which one depends on which board
    /// the user happens to be playing.
    ///
    /// # Why the notice's text is not asserted
    ///
    /// `decode_sf1` refuses this file, but *which* `StateError` depends on whether the
    /// tag or the length is checked first, and CPS-1's payload and SF1's are not the
    /// same size — so this file may well be `Truncated` rather than `WrongBoard`. One
    /// notice and an untouched machine is the claim that matters and the one that does
    /// not depend on that ordering. The `WrongBoard`-specifically assertion is in
    /// `frontend`'s codec tests; the sibling above can assert on `"board"` because
    /// there both payloads are the same length.
    #[test]
    fn a_cps1_state_is_refused_by_an_sf1_loop() {
        let (mut o, _s, _p) = opts("sf1-board-refuse");
        o.board = state_tag(machine::BoardKind::Sf1);
        let mut m = an_sf1_machine();
        m.run_frame();

        // A well-formed CPS-1 state, produced by CPS-1's own codec so that the only
        // thing wrong with it is the board it belongs to. A hand-mangled tag would
        // also be caught by the length check, which is a different check.
        let mut other = machine();
        other.run_frame();
        let bytes = frontend::encode(&cps1(&other).snapshot(), frontend::BOARD_SF2);
        std::fs::write(&o.state_path, &bytes).expect("temp dir is writable");

        // Destructured and dropped before `run`: a `CpuView` borrows the machine, and
        // holding one across `run(&mut m, ..)` is `E0502`.
        let before = {
            let v = m.cpu_view();
            (v.total_cycles, v.cpu.pc, v.line)
        };
        let mut d = Fake::new(vec![(KeySet::from_keys(&[Key::F8]), 0)]);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert_eq!(s.notices.len(), 1, "{:?}", s.notices);
        let v = m.cpu_view();
        assert_eq!(
            (v.total_cycles, v.cpu.pc, v.line),
            before,
            "the running machine must be untouched by a refused load"
        );
    }

    /// Each board's tag is the one its codec writes.
    ///
    /// Every value is a literal here, not `frontend::BOARD_SF2`: this is the mapping
    /// under test and reading the constant the function returns would prove nothing.
    /// The bytes are the format's documented form — big-endian ASCII, so a hex dump
    /// of a state file reads `SF2\0` or `SF1\0`.
    #[test]
    fn each_board_has_its_own_state_tag() {
        assert_eq!(state_tag(machine::BoardKind::Cps1), 0x5346_3200);
        assert_eq!(state_tag(machine::BoardKind::Sf1), 0x5346_3100);
        assert_eq!(&state_tag(machine::BoardKind::Cps1).to_be_bytes(), b"SF2\0");
        assert_eq!(&state_tag(machine::BoardKind::Sf1).to_be_bytes(), b"SF1\0");
        assert_ne!(
            state_tag(machine::BoardKind::Cps1),
            state_tag(machine::BoardKind::Sf1),
            "two boards sharing a tag is the whole failure the tag exists to stop"
        );
    }

    /// The tag `state_tag` answers is the tag the codec checks.
    ///
    /// # Why this is not the same test as the one above
    ///
    /// The one above pins `state_tag` against two literals. This pins it against
    /// `frontend`'s codec, which is the thing that has to agree: a `state_tag` that
    /// returned the right-looking number while `frontend::BOARD_SF2` was something
    /// else would pass up there and fail here.
    #[test]
    fn the_state_tag_is_the_one_the_codec_accepts() {
        let mut m = machine();
        m.run_frame();
        let bytes = frontend::encode(&cps1(&m).snapshot(), state_tag(machine::BoardKind::Cps1));
        assert!(
            frontend::decode(&bytes, state_tag(machine::BoardKind::Cps1)).is_ok(),
            "a state written with this tag must decode under it"
        );
        assert!(
            matches!(
                frontend::decode(&bytes, state_tag(machine::BoardKind::Sf1)),
                Err(frontend::StateError::WrongBoard { .. })
            ),
            "and must be refused under the other board's"
        );
    }

    /// F12 writes a screenshot.
    #[test]
    fn a_screenshot_is_written_as_a_ppm() {
        let (o, _s, shot) = opts("shot");
        let mut d = Fake::new(vec![Fake::held(&[Key::F12])]);
        let s = run(&mut machine(), &mut d, &mut NullAudio::default(), &o);
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
        let mut m = wrap(m);

        let mut d = Fake::new(Fake::idle(3));
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert!(
            cps1(&m).cpu.halted,
            "the premise: this program really halts"
        );
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
            Fake::held(&[Key::F11]),
            Fake::held(&[Key::F11, Key::Num5, Key::S]),
        ]);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        // F11 is held on both ticks, so there is no second edge and no unpause.
        assert_eq!(s.frames, 0, "the premise: no frame ran");
        assert!(
            cps1(&m).board.inputs.coin1,
            "the coin reached the board anyway"
        );
        assert_ne!(cps1(&m).board.inputs.in1(), 0xFFFF, "and so did the stick");
    }

    /// On an SF1 the same held keys reach the board *through the conversion*.
    ///
    /// The sibling above would pass on this board with the loop's SF1 arm assigning
    /// `Sf1Inputs::idle()` — nothing there reads an SF1 port. The three ports are
    /// asserted whole, as literals, because that is the only form in which "the
    /// conversion ran" and "a constant was written" differ:
    ///
    /// | port | idle | with these three keys |
    /// |---|---|---|
    /// | `IN0` 0xC00000 | 0xFFFF | 0xFFFE — coin 1 at bit 0 |
    /// | `IN1` 0xC00002 | 0xFFFF | 0xFFFD — P1 left at bit 1 |
    /// | `SYSTEM` 0xC0000C | 0xFF7F | 0xFF7E — start 1 at bit 0 |
    ///
    /// One key per port, deliberately: a conversion that dropped `start1` while
    /// carrying `coin1` is the failure a single-port assertion would miss, and the
    /// three ports do not overlap. `SYSTEM`'s 0xFF7F baseline is bit 7, the board's one
    /// active-high line — see `machine::sf1::inputs`.
    #[test]
    fn sf1_inputs_reach_the_board_through_the_conversion() {
        let (mut o, _s, _p) = opts("sf1-inputs");
        o.board = state_tag(machine::BoardKind::Sf1);
        let mut m = an_sf1_machine();
        let mut d = Fake::new(vec![Fake::held(&[Key::Num5, Key::Num1, Key::Q])]);
        run(&mut m, &mut d, &mut NullAudio::default(), &o);

        let i = &sf1(&m).board.inputs;
        assert_eq!(i.in0(), 0xFFFE, "coin 1 reached IN0");
        assert_eq!(i.in1(), 0xFFFD, "P1's left reached IN1");
        assert_eq!(i.system(), 0xFF7E, "and start 1 reached SYSTEM");
    }

    /// An SF1's frame reaches the window through SF1's pen conversion.
    ///
    /// The end-to-end claim for the pen fork: CPS-1's converter reads a 3,072-entry
    /// palette and shifts each 5-bit component; SF1's asks `Sf1Video::rgb` per pen, and
    /// that expands each nibble to `(n << 4) | n`. Entry 0x0135 is (0x11, 0x33, 0x55)
    /// under SF1's rule — the literal `main`'s `ppm_sf1` test uses — and would be
    /// (0x08, 0x18, 0x28) under CPS-1's, so a loop that ran the wrong converter, or
    /// none, presents a different number here.
    ///
    /// The pixel is the screen origin, which is TX cell 136's tile pixel (0,0): pen 0
    /// of colour 2, palette entry 776. Pen 0 is not the text layer's transparent pen
    /// (that is 3), so it draws.
    #[test]
    fn an_sf1_frame_reaches_the_window_through_sf1s_pen_conversion() {
        let (mut o, _s, _p) = opts("sf1-pens");
        o.board = state_tag(machine::BoardKind::Sf1);
        let mut m = an_sf1_machine_that_draws();
        let mut d = Fake::new(Fake::idle(2));
        run(&mut m, &mut d, &mut NullAudio::default(), &o);
        let shown = d.last.expect("a tick presents");
        assert_eq!(
            shown.len(),
            384 * 224,
            "one ARGB word per pixel of SF1's screen"
        );
        assert_eq!(
            shown[0], 0x0011_3355,
            "SF1's `(n << 4) | n` on entry 0x0135, not CPS-1's shift"
        );
    }

    /// Subtracting SF1's text layer changes the frame the window is given.
    ///
    /// The mirror of [`subtracting_a_layer_changes_the_presented_frame`], and the only
    /// test of the SF1 mask fork: that one runs a CPS-1, where `gfx.mask()` is read and
    /// `sf1_mask()` never is. A loop assigning `LayerMask::all()` on the SF1 arm — or
    /// CPS-1's mask, which does not typecheck, or nothing — passes it and fails here.
    ///
    /// The key script is that test's, with the selection walked to row 3 first. SF1's
    /// layers view has rows BG, FG, sprites, TX and the selection starts at 0, so
    /// reaching TX means three `]` presses — and this fixture draws only the text plane,
    /// because that is the plane whose map lives in guest RAM. `Enter` on row 0 would
    /// subtract the background, which draws nothing here, and the frame would not
    /// change.
    ///
    /// ⚠️ **A released tick between every press, including between the three `]`s.**
    /// They are edge-triggered, so three consecutive held ticks are one press: written
    /// that way first, the selection stopped at row 1 and the frame did not change.
    #[test]
    fn subtracting_an_sf1_layer_changes_the_presented_frame() {
        let (mut o, _s, _p) = opts("sf1-mask-wired");
        o.board = state_tag(machine::BoardKind::Sf1);

        // Scoped, so the first machine is gone before the second is built.
        let full = {
            let mut m = an_sf1_machine_that_draws();
            let mut d = Fake::new(Fake::idle(20));
            run(&mut m, &mut d, &mut NullAudio::default(), &o);
            d.last.expect("a tick presents")
        };
        assert_ne!(
            full[0], 0,
            "the premise: the unmasked frame draws at the origin"
        );

        // Show the viewer, cycle to the layers view, walk the selection to row 3, act,
        // and hide the viewer again — the box would otherwise cover the frame and every
        // pixel would differ for the wrong reason.
        //
        // Twenty ticks, matching the run above tick for tick: both must render the same
        // number of frames, or the pictures differ because the machines are at different
        // points and the mask is not what the comparison sees.
        let mut m = an_sf1_machine_that_draws();
        let mut d = Fake::new(vec![
            Fake::held(&[Key::GfxToggled]),
            Fake::held(&[]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[]),
            Fake::held(&[Key::BracketRight]),
            Fake::held(&[]),
            Fake::held(&[Key::BracketRight]),
            Fake::held(&[]),
            Fake::held(&[Key::BracketRight]),
            Fake::held(&[]),
            Fake::held(&[Key::Enter]),
            Fake::held(&[]),
            Fake::held(&[Key::GfxToggled]),
            Fake::held(&[]),
            Fake::held(&[]),
            Fake::held(&[]),
        ]);
        run(&mut m, &mut d, &mut NullAudio::default(), &o);
        let masked = d.last.expect("a tick presents");
        assert_ne!(
            full, masked,
            "subtracting the text layer must change the picture"
        );
        assert_eq!(
            masked[0], 0,
            "and leave the origin at the cleared pen, whose colour is black"
        );
    }

    /// F5 then F8 round-trips an SF1 through a real file.
    ///
    /// The mirror of [`a_save_and_load_round_trip_through_the_real_file`], which runs a
    /// CPS-1 and so exercises `encode`/`decode` only. This is the one test of
    /// `encode_sf1`/`decode_sf1` from inside the loop: a save arm that wrote an empty
    /// `Vec` leaves a file the load arm then refuses, so the notice assertions are what
    /// catch it, and the divergence is what catches a load that decoded and dropped the
    /// state.
    ///
    /// Divergence, not comparison — the sibling's argument: save, run on, load, run the
    /// same number of frames, and require the machine to arrive at the same place.
    #[test]
    fn an_sf1_save_and_load_round_trip_through_the_real_file() {
        let (mut o, _s, _p) = opts("sf1-roundtrip");
        o.board = state_tag(machine::BoardKind::Sf1);
        let mut m = an_sf1_machine();

        let mut script = vec![Fake::held(&[Key::F5])];
        script.extend(Fake::idle(4));
        let mut d = Fake::new(script);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert!(
            s.notices.is_empty(),
            "the save must succeed: {:?}",
            s.notices
        );
        assert!(o.state_path.exists(), "and leave a file behind");
        let after = {
            let v = m.cpu_view();
            (v.total_cycles, v.cpu.d[0], v.cpu.d[1], v.line)
        };
        assert_ne!(after.0, 0, "the premise: the machine moved");

        // Run four more, then load and run the same four again.
        let mut d = Fake::new(Fake::idle(4));
        run(&mut m, &mut d, &mut NullAudio::default(), &o);
        let mut script = vec![Fake::held(&[Key::F8])];
        script.extend(Fake::idle(4));
        let mut d = Fake::new(script);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert!(
            s.notices.is_empty(),
            "the load must succeed: {:?}",
            s.notices
        );
        let v = m.cpu_view();
        assert_eq!(
            (v.total_cycles, v.cpu.d[0], v.cpu.d[1], v.line),
            after,
            "the loaded machine must reach the same place"
        );
    }

    /// F12 writes an SF1 screenshot, and it is SF1's PPM.
    ///
    /// The screenshot fork's only test from inside the loop. `main`'s
    /// `the_sf1_screenshot_uses_sf1s_dac_rule_and_not_cps1s` pins `ppm_sf1` itself; this
    /// pins that the loop calls it on this arm — a fork writing an empty `Vec`, or
    /// CPS-1's `ppm`, leaves a different file.
    ///
    /// The literals are `main`'s: a 15-byte `P6\n384 224\n255\n` header and 258,048
    /// bytes of body. The first pixel is entry 0x0135 through SF1's rule, which is the
    /// byte triple CPS-1's converter cannot produce from the same entry.
    #[test]
    fn f12_writes_an_sf1_screenshot() {
        let (mut o, _s, _p) = opts("sf1-shot");
        o.board = state_tag(machine::BoardKind::Sf1);
        let mut m = an_sf1_machine_that_draws();
        let mut d = Fake::new(vec![Fake::held(&[Key::F12]), Fake::held(&[])]);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert!(
            s.notices.is_empty(),
            "the screenshot must succeed: {:?}",
            s.notices
        );
        let bytes = std::fs::read(&o.shot_path).expect("F12 must write the file");
        assert_eq!(&bytes[..15], b"P6\n384 224\n255\n", "the exact header");
        assert_eq!(bytes.len(), 258_063, "and 384 * 224 * 3 bytes after it");
        assert_eq!(
            &bytes[15..18],
            &[0x11, 0x33, 0x55],
            "SF1's DAC rule on entry 0x0135, not CPS-1's"
        );
    }

    /// The pacer runs each board at its own frame period.
    ///
    /// SF1's is 16,666,667 ns (60.0 Hz) against CPS-1's 16,768,000 (59.63) — the
    /// shorter one, which is what makes one tick able to tell them apart. A tick of
    /// exactly SF1's period owes a whole frame on SF1 and falls 101,333 ns short on
    /// CPS-1, so:
    ///
    /// | board | tick of 16,666,667 ns |
    /// |---|---|
    /// | SF1 | 1 frame |
    /// | CPS-1 | 0 frames |
    ///
    /// Both halves, because only the pair distinguishes "the period follows the board"
    /// from "the period is a constant that happens to be SF1's". A loop hardcoding
    /// `FramePacer::cps1()` fails the first row; one hardcoding SF1's fails the second.
    ///
    /// The literals are the periods themselves, not `m.frame_ns()`: reading the number
    /// the code under test reads would assert nothing. `machine`'s own tests pin the two
    /// constants against their refresh rates.
    #[test]
    fn each_boards_pacer_runs_at_that_boards_frame_period() {
        /// SF1's frame period in nanoseconds. See `machine`'s `SF1_FRAME_NS`.
        const SF1_NS: u64 = 16_666_667;

        let (mut o, _s, _p) = opts("sf1-pace");
        o.board = state_tag(machine::BoardKind::Sf1);
        let mut m = an_sf1_machine();
        let mut d = Fake::new(vec![(KeySet::new(), SF1_NS)]);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert_eq!(s.frames, 1, "SF1 owes a frame at its own period");

        let (o2, _s2, _p2) = opts("cps1-pace");
        let mut m2 = machine();
        let mut d2 = Fake::new(vec![(KeySet::new(), SF1_NS)]);
        let s2 = run(&mut m2, &mut d2, &mut NullAudio::default(), &o2);
        assert_eq!(
            s2.frames, 0,
            "and CPS-1's period is longer, so the same tick owes nothing"
        );
    }

    /// A `LoopOpts` whose tag does not match its machine is a panic in debug.
    ///
    /// The pairing `main` makes by hand — a board from the ROM set, a tag from
    /// [`state_tag`] — is the thing this checks. Get it wrong and a session writes
    /// states its own machine then refuses to load, which is silent until someone
    /// presses `F8`.
    ///
    /// `#[cfg(debug_assertions)]`, because that is the build the assertion exists in: a
    /// `panic!` inside `run` in a release build takes a window down mid-game, and the
    /// mismatch is a bug in `main` that no user input can cause. `crates/m68k`'s
    /// `set_irq_panics_on_an_out_of_range_level_in_debug` is the same shape.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "does not match the machine's board")]
    fn a_mismatched_board_tag_panics_in_debug() {
        // SF2's tag, an SF1 machine — the pairing that compiles, runs, and is wrong.
        let (o, _s, _p) = opts("board-mismatch");
        run(
            &mut an_sf1_machine(),
            &mut Fake::new(Fake::idle(1)),
            &mut NullAudio::default(),
            &o,
        );
    }

    /// `F4` steps one instruction per press, and no frames.
    ///
    /// The literals are measured, not derived: `move #$2000,sr` at 0x1000 costs 16
    /// cycles and the `addq.w` at 0x1004 costs 4. Written out rather than compared
    /// against a second machine stepped the same way, which would be asserting
    /// `step_instruction` equals itself.
    #[test]
    fn f4_steps_one_instruction() {
        let (o, _s, _p) = opts("step-insn");
        let mut m = machine();
        // Paused throughout: what is under test is that `F4` moves the machine by an
        // *instruction*, and a running loop would move it by a frame at the same time.
        let script = vec![
            Fake::held(&[Key::F11]),
            Fake::held(&[Key::F11, Key::F4]),
            // Held for a second tick: one instruction per press, not per tick.
            Fake::held(&[Key::F11, Key::F4]),
            Fake::held(&[Key::F11]),
            Fake::held(&[Key::F11, Key::F4]),
        ];
        let mut d = Fake::new(script);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert_eq!(s.frames, 0, "the premise: no frame ran");
        assert_eq!(
            cps1(&m).total_cycles,
            20,
            "16 for the `move`, 4 for the `addq`"
        );
        assert_eq!(
            frontend::overlay::executing_pc(&m.cpu_view()),
            0x1006,
            "and it is sitting at the third instruction"
        );
    }

    /// A breakpoint stops the loop part-way through a frame, and says so in the title.
    ///
    /// Mid-frame is the whole point: a breakpoint that only stopped at frame boundaries
    /// would be the `.` key with extra steps. The discriminator is the cycle count — a
    /// loop that ran the frame and *then* stopped would be 167,680 cycles in, at line 0.
    ///
    /// The breakpoint is set through `F7`, which is the only way in: the loop owns its
    /// `Debugger` and a test cannot reach inside it. So the machine is positioned on the
    /// target instruction first, `F7` marks it, `F4` steps off it, and the frame that
    /// follows comes back round to it.
    #[test]
    fn a_breakpoint_stops_the_loop_mid_frame() {
        let (o, _s, _p) = opts("bp-midframe");
        let mut m = machine_with_a_long_loop();
        assert_eq!(
            frontend::overlay::executing_pc(&m.cpu_view()),
            TARGET,
            "the premise: reset leaves the machine on the instruction F7 will mark"
        );

        let script = vec![
            // Zero elapsed on the two setting-up ticks, so neither owes a frame and the
            // only frame in this test is the one the breakpoint cuts.
            (KeySet::from_keys(&[Key::F7]), 0),
            (KeySet::from_keys(&[Key::F4]), 0),
            (KeySet::new(), FRAME_NS),
        ];
        let mut d = Fake::new(script);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);

        assert_eq!(
            frontend::overlay::executing_pc(&m.cpu_view()),
            TARGET,
            "it stopped at the breakpoint"
        );
        assert_ne!(
            cps1(&m).line,
            0,
            "mid-frame: the beam is not at a frame boundary"
        );
        assert!(
            cps1(&m).total_cycles < u64::from(cps1(&m).timing.cycles_per_frame()),
            "and well short of a whole frame: {} cycles",
            cps1(&m).total_cycles
        );
        assert_eq!(s.frames, 0, "a frame cut in half is not a frame that ran");
        assert!(
            d.titles.iter().any(|t| t.contains("paused")),
            "and the loop is paused, which the title must say: {:?}",
            d.titles
        );
    }

    /// Resuming from a breakpoint makes progress, and not resuming makes none.
    ///
    /// Two runs of the same script, one with a `P` in it. The comparison is what makes
    /// both halves testable at once: a loop that never paused would run its three
    /// trailing ticks in the first script too, so the first total would be the *larger*
    /// of the two. A loop that paused and could not resume would leave them equal.
    #[test]
    fn resuming_from_a_breakpoint_makes_progress() {
        let (o, _s, _p) = opts("bp-resume");
        let mut m = machine_with_a_long_loop();
        // One machine, snapshotted and put back, rather than two: a `Cps1` is 525 KB on
        // the stack and two live in one test thread overflows it.
        let start = cps1(&m).snapshot();

        // Stop, then sit there for two more ticks.
        let mut script = vec![
            (KeySet::from_keys(&[Key::F7]), 0),
            (KeySet::from_keys(&[Key::F4]), 0),
        ];
        script.extend(Fake::idle(3));
        let mut d = Fake::new(script.clone());
        run(&mut m, &mut d, &mut NullAudio::default(), &o);
        let stopped = cps1(&m).total_cycles;
        assert!(stopped > 0, "the premise: the machine ran up to the stop");

        // The same, but the third of those ticks presses pause.
        cps1_mut(&mut m).restore(&start);
        script[3] = Fake::held(&[Key::F11]);
        let mut d = Fake::new(script);
        run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert!(
            cps1(&m).total_cycles > stopped,
            "resuming must run on: {} cycles against {stopped} stopped",
            cps1(&m).total_cycles
        );
    }

    /// Stepping *onto* a breakpoint and then resuming makes progress.
    ///
    /// `F4` arrives at an instruction that has a breakpoint on it. You asked to be
    /// there, so resuming must run on — the loop notes the step as a stop for exactly
    /// this reason. Without that note the resume tick runs **zero** instructions and
    /// pauses again, which is a step key that can walk you into a place you cannot
    /// leave without deleting the breakpoint.
    ///
    /// Found by mutation, not by reading: deleting the `note_stopped` after `F4`'s step
    /// left every other test in this module green.
    ///
    /// The fixture is a two-instruction loop, so `F4` can walk right round it and land
    /// back on the marked instruction in two presses:
    ///
    /// ```text
    /// 1000  5240   addq.w #1,d0     4 cycles
    /// 1002  60FC   bra $1000       10 cycles   (0x1004 - 4)
    /// ```
    ///
    /// One `run` call, not two: the loop owns its `Debugger`, so the breakpoint `F7`
    /// sets does not outlive the call that set it. Written as two calls first, where the
    /// second ran with no breakpoints at all and the assertion failed for a reason that
    /// had nothing to do with the claim.
    #[test]
    fn stepping_onto_a_breakpoint_and_resuming_makes_progress() {
        let (o, _s, _p) = opts("step-onto-bp");
        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x1000..0x1004].copy_from_slice(&[0x52, 0x40, 0x60, 0xFC]);
        let mut m = Cps1::new(&rom, BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        let mut m = wrap(m);

        // Zero elapsed on every set-up tick, so the only thing that moves the machine
        // before the last one is `F4`. Three steps: 4 + 10 + 4 = 18 cycles, arriving
        // back at 0x1002 with a breakpoint on it.
        let mut d = Fake::new(vec![
            (KeySet::from_keys(&[Key::F4]), 0),
            // 0x1002 now, and `F7` marks it.
            (KeySet::from_keys(&[Key::F7]), 0),
            // Round the loop: 0x1002 -> 0x1000, then 0x1000 -> 0x1002, onto the mark.
            (KeySet::from_keys(&[Key::F4]), 0),
            (KeySet::new(), 0),
            (KeySet::from_keys(&[Key::F4]), 0),
            // And a tick that owes a frame. It must run.
            (KeySet::new(), FRAME_NS),
        ]);
        run(&mut m, &mut d, &mut NullAudio::default(), &o);

        // 32, not 18: the resume ran the `bra` and the `addq` and stopped when it came
        // back round. 18 is exactly the mutant's answer — stepped onto the breakpoint
        // and stuck there.
        assert_eq!(
            cps1(&m).total_cycles,
            32,
            "the resume must leave the instruction it was stepped onto"
        );
        assert_eq!(
            frontend::overlay::executing_pc(&m.cpu_view()),
            0x1002,
            "and stop at the breakpoint again on the way round, not run the frame out"
        );
    }

    /// The overlay reaches the presented buffer, drawn over the game's pixels.
    ///
    /// The presented buffer is the artifact — this is the only test that covers the
    /// whole path: frame rendered, pens converted, overlay composed, handed to the
    /// display.
    ///
    /// `frontend`'s own panel tests read the characters back off the pixels with
    /// `font::read_text`, which this crate cannot: it is `#[cfg(test)]` and
    /// crate-private to `frontend`. The equality against `overlay::draw` is the
    /// available equivalent, and for *this* claim it is the better one — what Task 6
    /// owns is the composition, so what is asserted is that the presented frame is
    /// exactly the game's pixels with the overlay over them, drawn with the arguments
    /// the loop is supposed to pass.
    #[test]
    fn the_overlay_reaches_the_presented_buffer() {
        let (o, _s, _p) = opts("overlay-shown");
        let mut m = machine_that_draws();
        // Zero elapsed and one tick, so the machine does not move: the state it is in
        // afterwards is the state the overlay was drawn from.
        let mut d = Fake::new(vec![(KeySet::from_keys(&[Key::F1]), 0)]);
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert_eq!(s.frames, 0, "the premise: the machine did not move");
        let shown = d.first.expect("a tick presents");

        let mut game: Vec<u32> = Vec::new();
        pens_to_argb(&cps1(&m).video, &mut game);
        assert_ne!(
            shown, game,
            "the premise: the overlay changed the presented frame"
        );

        let mut expected = game.clone();
        frontend::overlay::draw(
            &mut expected,
            &m.cpu_view(),
            &|a| m.peek_word(a),
            &m,
            frontend::overlay::Panels::on(),
            frontend::overlay::executing_pc(&m.cpu_view()),
            0,
            &[],
        );
        assert_eq!(
            shown, expected,
            "the game's pixels with the overlay over them, at the follow address"
        );
        // And the game really is underneath: this fixture's frame is not uniform, so an
        // overlay that had replaced the frame rather than composed over it would fail
        // the equality above — but only if the game's pixels are actually there.
        assert!(
            game.iter().any(|&px| px != game[0]),
            "the premise: the game's frame is not one flat colour"
        );
    }

    /// With the overlay off, the presented frame is the game's pixels exactly.
    ///
    /// The other half of the claim above, and the one that matters when the debugger is
    /// not in use: an overlay that drew its background unconditionally would put a dark
    /// box over the game of everyone who never presses `F1`.
    #[test]
    fn the_overlay_off_presents_an_unmodified_frame() {
        let (o, _s, _p) = opts("overlay-off");
        let mut m = machine_that_draws();
        let mut d = Fake::new(vec![(KeySet::new(), 0)]);
        run(&mut m, &mut d, &mut NullAudio::default(), &o);
        let shown = d.first.expect("a tick presents");
        let mut game: Vec<u32> = Vec::new();
        pens_to_argb(&cps1(&m).video, &mut game);
        assert_eq!(shown, game, "not a pixel of the game is disturbed");
    }

    /// The viewer draws over E2's panels, not under them.
    ///
    /// Both overlays are opaque and they overlap; the order decides which you can
    /// read. The video viewer wins, because it is the whole screen and E2's panels
    /// are corners of it — the other order would leave the viewer with a register
    /// panel punched out of its top-left, which is where its own labels are.
    #[test]
    fn the_video_viewer_draws_over_the_debugger() {
        let (o, _s, _p) = opts("viewer-over-debugger");
        // The pixel both halves read: the top-left corner of E2's register panel,
        // which the viewer's box covers. The corner itself and not a pixel inside it
        // — `overlay::PAD` is one pixel, so the first glyph of `D0 ...` starts at
        // `(REGS_X + 1, REGS_Y + 1)` and that pixel is 0x00D0D0D0 text, not the
        // panel's background at all.
        let at = frontend::overlay::REGS_Y * WIDTH + frontend::overlay::REGS_X;

        // Each run in its own scope, so its machine is dropped before the next one is
        // built: a `Cps1` is 525 KB on the stack, and two live at once overflows a
        // test thread. A shadowing `let` would *not* do it — the first binding lives
        // to the end of the block either way.
        let shown = {
            // One tick, both overlays on. A tick reads its keys before it renders, so
            // the frame this tick presents already has both.
            let mut m = machine_that_draws();
            let mut d = Fake::new(vec![Fake::held(&[Key::F1, Key::GfxToggled])]);
            run(&mut m, &mut d, &mut NullAudio::default(), &o);
            d.first.expect("a tick presents")
        };
        // What must be there is the viewer's background, not the debugger's.
        assert_ne!(
            shown[at], 0x0000_0020,
            "the debugger's background is on top"
        );

        // And the premise, or a viewer that drew nothing would pass: with the
        // viewer off, that pixel *is* the debugger's background.
        let e2 = {
            let mut m = machine_that_draws();
            let mut d = Fake::new(vec![Fake::held(&[Key::F1])]);
            run(&mut m, &mut d, &mut NullAudio::default(), &o);
            d.first.expect("a tick presents")
        };
        assert_eq!(
            e2[at], 0x0000_0020,
            "the premise: E2 draws there when the viewer does not"
        );
    }

    /// The mask reaches `Video`, and the frame changes.
    ///
    /// The end-to-end claim of this task: a key press in `sfemu` subtracts a layer
    /// from the pixels the window is given. Everything else in this module tests a
    /// decision; this tests the wire.
    #[test]
    fn subtracting_a_layer_changes_the_presented_frame() {
        let (o, _s, _p) = opts("mask-wired");
        // Scoped, so this machine is gone before the second is built: 525 KB each and
        // a test thread's stack is 2 MB.
        //
        // Twelve ticks, matching the script below tick for tick: both runs must render
        // the same number of frames, or the two pictures differ because the machines
        // are at different points and the mask is not what the comparison sees.
        let full = {
            let mut m = machine_that_draws();
            let mut d = Fake::new(Fake::idle(12));
            run(&mut m, &mut d, &mut NullAudio::default(), &o);
            d.last.expect("a tick presents")
        };

        // Now: show the viewer, cycle to the layers view, subtract the selected
        // row, and hide the viewer again — the box would otherwise cover the whole
        // frame and every pixel would differ for the wrong reason. Hiding it also
        // exercises "a hidden viewer keeps its mask".
        //
        // Three `GfxView` presses because the cycle is tiles → tilemap → palette →
        // layers. If Task 3 chose another order this count is wrong, and the
        // assertion below is what says so.
        //
        // A released tick between each: every one of these keys is edge-triggered, so
        // three consecutive held ticks are one press and would leave the viewer on the
        // tilemap view, where `Enter` changes the layer being *browsed* and masks
        // nothing. Written without them first, and that is exactly what happened.
        let mut m = machine_that_draws();
        let mut d = Fake::new(vec![
            Fake::held(&[Key::GfxToggled]),
            Fake::held(&[]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[]),
            Fake::held(&[Key::Enter]),
            Fake::held(&[]),
            Fake::held(&[Key::GfxToggled]),
            Fake::held(&[]),
        ]);
        run(&mut m, &mut d, &mut NullAudio::default(), &o);
        let masked = d.last.expect("a tick presents");
        assert_ne!(
            cps1(&m).video.enable,
            machine::video::compose::LayerMask::all(),
            "the presses reached the layers view and subtracted a row"
        );
        assert_ne!(masked, full, "subtracting a layer changed the picture");
    }

    /// **The criterion that matters most:** looking at the video does not change the
    /// machine.
    ///
    /// `watching_the_machine_does_not_change_it` proves the debugger is inert. This
    /// changes the *picture* on purpose, which makes the same claim harder and more
    /// important: the mask must reach `Video` and nothing else. Nothing the 68000 or
    /// the board reads depends on the framebuffer, so this is provable rather than
    /// merely intended.
    ///
    /// The `machine()` fixture and not `machine_that_draws()`: what is compared is CPU
    /// and RAM state, and `machine()`'s program moves `d0` and a word of RAM every
    /// frame — so two runs that differed at all would differ visibly.
    ///
    /// Compared field by field rather than through `snapshot()`, because a snapshot
    /// leaves out the trace and the trace is where a stray acknowledge would show.
    #[test]
    fn looking_at_the_video_does_not_change_the_machine() {
        let (o, _s, _p) = opts("looking");
        let mut m = machine();
        // One machine, restored between the two runs — 525 KB on the stack means two
        // do not fit in a test thread.
        let start = cps1(&m).snapshot();

        // Every view, and a subtracted layer. Ten ticks, so the comparison run's ten
        // idle ticks ask for the same frames — the fake advances one frame per tick
        // whether keys are held or not.
        //
        // A released tick after each press: these keys are all edge-triggered, so
        // consecutive held ticks are one press and the script would stop short of the
        // layers view.
        let base = (cps1(&m).board.trace.acks, cps1(&m).board.trace.vblanks);
        let mut d = Fake::new(vec![
            Fake::held(&[Key::GfxToggled]),
            Fake::held(&[]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[]),
            Fake::held(&[Key::Enter]),
            Fake::held(&[]),
        ]);
        let s_on = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        let b = cps1(&m);
        let on = (
            b.total_cycles,
            b.cpu.d,
            b.cpu.a,
            b.cpu.pc,
            b.line,
            b.board.trace.acks - base.0,
            b.board.trace.vblanks - base.1,
        );
        let ram_on = cps1(&m).board.ram.clone();
        assert_ne!(on.0, 0, "the premise: the machine ran");
        assert_ne!(
            cps1(&m).video.enable,
            machine::video::compose::LayerMask::all(),
            "the premise: a layer really was subtracted"
        );

        cps1_mut(&mut m).restore(&start);
        // `restore` leaves `enable` alone — it is not machine state — so the
        // comparison run must clear it by hand. That this is necessary is itself the
        // point of `machine`'s own `the_layer_mask_is_not_machine_state`.
        cps1_mut(&mut m).video.enable = machine::video::compose::LayerMask::all();
        let base = (cps1(&m).board.trace.acks, cps1(&m).board.trace.vblanks);
        let mut d = Fake::new(Fake::idle(10));
        let s_off = run(&mut m, &mut d, &mut NullAudio::default(), &o);

        assert_eq!(s_on.frames, s_off.frames, "the same frames were asked for");
        assert_eq!(s_on.frames, 10, "and there were some");
        let b = cps1(&m);
        assert_eq!(
            on,
            (
                b.total_cycles,
                b.cpu.d,
                b.cpu.a,
                b.cpu.pc,
                b.line,
                b.board.trace.acks - base.0,
                b.board.trace.vblanks - base.1,
            ),
            "the viewer must not move the machine"
        );
        assert_eq!(ram_on, cps1(&m).board.ram, "nor write a word of its memory");
    }

    /// And the same claim for **all sixteen** mask combinations, not just one.
    ///
    /// E3's spec asks for every combination, and one subtracted layer is not that: a
    /// mask wired to something with a side effect could be inert for the sprites and
    /// not for scroll 2, and the test above would pass.
    ///
    /// Driven through the keys rather than by setting `m.video.enable`, which would be
    /// vacuous — the loop assigns `gfx.mask()` every tick and would overwrite it before
    /// the first frame. So the script walks the layers view: `Enter` on a row to
    /// subtract it, `]` to move on. **Every run is the same 24 ticks whatever the
    /// subset**, because a row that is not being subtracted spends its press on a
    /// released tick instead of on `Enter` — a comparison between runs of different
    /// lengths would be comparing frame counts, not inertness.
    #[test]
    fn no_mask_combination_changes_the_machine() {
        let (o, _s, _p) = opts("everymask");
        let mut m = machine();
        let start = cps1(&m).snapshot();

        /// The 24-tick script that subtracts the rows in `bits`, one bit per row.
        ///
        /// Released ticks throughout: every key here is edge-triggered, so two
        /// consecutive held ticks are one press.
        fn script(bits: u8) -> Vec<(KeySet, u64)> {
            let mut s = vec![Fake::held(&[Key::GfxToggled]), Fake::held(&[])];
            // Tiles → Tilemap → Palette → Layers.
            for _ in 0..3 {
                s.push(Fake::held(&[Key::GfxView]));
                s.push(Fake::held(&[]));
            }
            for row in 0..4 {
                // The press, or a released tick standing in for it, so the length
                // does not depend on `bits`.
                s.push(if bits & (1 << row) != 0 {
                    Fake::held(&[Key::Enter])
                } else {
                    Fake::held(&[])
                });
                s.push(Fake::held(&[]));
                s.push(Fake::held(&[Key::BracketRight]));
                s.push(Fake::held(&[]));
            }
            s
        }

        // The baseline: the same tick count with nothing subtracted, which `bits == 0`
        // is exactly — the script still walks to the layers view and moves the row
        // selection, so the two runs differ in the mask and in nothing else.
        let base = (cps1(&m).board.trace.acks, cps1(&m).board.trace.vblanks);
        let mut d = Fake::new(script(0));
        let s_off = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        let b = cps1(&m);
        let want = (
            b.total_cycles,
            b.cpu.d,
            b.cpu.a,
            b.cpu.pc,
            b.line,
            b.board.trace.acks - base.0,
            b.board.trace.vblanks - base.1,
        );
        let want_ram = cps1(&m).board.ram.clone();
        assert_ne!(want.0, 0, "the premise: the machine ran");
        assert_eq!(s_off.frames, 24, "and the script is 24 ticks long");
        assert_eq!(
            cps1(&m).video.enable,
            machine::video::compose::LayerMask::all(),
            "the premise: the baseline subtracted nothing"
        );

        for bits in 1u8..16 {
            cps1_mut(&mut m).restore(&start);
            // `restore` leaves `enable` alone — it is not machine state — so each run
            // starts from the identity by hand.
            cps1_mut(&mut m).video.enable = machine::video::compose::LayerMask::all();
            let base = (cps1(&m).board.trace.acks, cps1(&m).board.trace.vblanks);
            let mut d = Fake::new(script(bits));
            let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
            // Row 0 is the sprites, then the three scrolls in order.
            assert_eq!(
                cps1(&m).video.enable,
                machine::video::compose::LayerMask {
                    sprites: bits & 1 == 0,
                    scroll1: bits & 2 == 0,
                    scroll2: bits & 4 == 0,
                    scroll3: bits & 8 == 0,
                },
                "the premise: {bits:04b} reached the mask it names"
            );
            assert_eq!(s.frames, s_off.frames, "the same frames were asked for");
            let b = cps1(&m);
            assert_eq!(
                want,
                (
                    b.total_cycles,
                    b.cpu.d,
                    b.cpu.a,
                    b.cpu.pc,
                    b.line,
                    b.board.trace.acks - base.0,
                    b.board.trace.vblanks - base.1,
                ),
                "mask {bits:04b} moved the machine"
            );
            assert_eq!(
                want_ram,
                cps1(&m).board.ram,
                "mask {bits:04b} wrote its memory"
            );
        }
    }

    /// **The criterion that matters most:** watching the machine does not change it.
    ///
    /// A tool that observes the bug must not be part of it. `peek_word` taking `&self`
    /// is necessary and not sufficient — the *loop* could still differ with the overlay
    /// on, by drawing before the conversion, by consulting the machine differently, or
    /// by spending a frame's worth of anything.
    ///
    /// Compared field by field rather than through `snapshot()`: a snapshot leaves out
    /// the trace, and the trace is where a stray interrupt acknowledge would show.
    #[test]
    fn watching_the_machine_does_not_change_it() {
        let (o, _s, _p) = opts("watching");
        let mut m = machine();
        // One machine, put back between the two runs — 525 KB on the stack means two do
        // not fit in a test thread. `restore` leaves the trace alone deliberately, so
        // the trace figures are compared as deltas from each run's own baseline.
        let start = cps1(&m).snapshot();
        let base = (cps1(&m).board.trace.acks, cps1(&m).board.trace.vblanks);

        let mut script = vec![Fake::held(&[Key::F1])];
        script.extend(Fake::idle(3));
        let mut d = Fake::new(script);
        let s_on = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        let b = cps1(&m);
        let on = (
            b.total_cycles,
            b.cpu.d,
            b.cpu.a,
            b.cpu.pc,
            b.line,
            b.board.trace.acks - base.0,
            b.board.trace.vblanks - base.1,
        );
        let ram_on = cps1(&m).board.ram.clone();
        assert_ne!(on.0, 0, "the premise: the machine ran");
        // And the overlay really was on, or this compares two identical runs.
        let first = d.first.expect("a tick presents");
        assert!(
            first.iter().any(|&px| px != first[0]),
            "the premise: the overlay was drawn"
        );

        cps1_mut(&mut m).restore(&start);
        let base = (cps1(&m).board.trace.acks, cps1(&m).board.trace.vblanks);
        let mut d = Fake::new(Fake::idle(4));
        let s_off = run(&mut m, &mut d, &mut NullAudio::default(), &o);

        assert_eq!(s_on.frames, s_off.frames, "the same frames were asked for");
        assert_eq!(s_on.frames, 4, "and there were some");
        let b = cps1(&m);
        assert_eq!(
            on,
            (
                b.total_cycles,
                b.cpu.d,
                b.cpu.a,
                b.cpu.pc,
                b.line,
                b.board.trace.acks - base.0,
                b.board.trace.vblanks - base.1,
            ),
            "the overlay must not move the machine"
        );
        assert_eq!(ram_on, cps1(&m).board.ram, "nor write a word of its memory");
    }

    /// The debugger's stepping path reaches the same machine as `run_frame`.
    ///
    /// Every tick now runs its frames through [`run_frame_to_breakpoint`], one
    /// instruction at a time, where before it called `Cps1::run_frame`. That is the
    /// substitution this task made, and nothing else tests it: the two paths differing
    /// would show as an emulator that runs one way under the debugger and another way
    /// without it, with every other test in this module green either way.
    #[test]
    fn the_stepping_path_reaches_the_same_machine_as_run_frame() {
        let (o, _s, _p) = opts("stepping-path");
        let mut m = machine();
        let start = cps1(&m).snapshot();
        // `restore` deliberately leaves the trace alone — a reset or a load is part of
        // the session the trace records — so the trace figures are compared as deltas
        // from each run's own baseline rather than as absolutes.
        let b = cps1(&m);
        let base = (
            b.board.trace.frames,
            b.board.trace.acks,
            b.board.trace.vblanks,
        );

        let mut d = Fake::new(Fake::idle(4));
        let s = run(&mut m, &mut d, &mut NullAudio::default(), &o);
        assert_eq!(s.frames, 4, "the premise: four frames ran");
        let b = cps1(&m);
        let stepped = (
            b.total_cycles,
            b.cpu.d,
            b.cpu.a,
            b.cpu.pc,
            b.line,
            b.board.trace.frames - base.0,
            b.board.trace.acks - base.1,
            b.board.trace.vblanks - base.2,
        );
        let ram_stepped = cps1(&m).board.ram.clone();
        assert_ne!(stepped.0, 0, "and the machine moved");

        cps1_mut(&mut m).restore(&start);
        let b = cps1(&m);
        let base = (
            b.board.trace.frames,
            b.board.trace.acks,
            b.board.trace.vblanks,
        );
        for _ in 0..4 {
            m.run_frame();
        }
        let b = cps1(&m);
        assert_eq!(
            stepped,
            (
                b.total_cycles,
                b.cpu.d,
                b.cpu.a,
                b.cpu.pc,
                b.line,
                b.board.trace.frames - base.0,
                b.board.trace.acks - base.1,
                b.board.trace.vblanks - base.2,
            ),
            "instruction by instruction must reach where `run_frame` reaches"
        );
        assert_eq!(
            ram_stepped,
            cps1(&m).board.ram,
            "down to the last word of memory"
        );
    }

    /// A closed display runs nothing.
    #[test]
    fn a_display_that_never_opens_runs_nothing() {
        let (o, _s, _p) = opts("closed");
        let mut d = Fake::new(Vec::new());
        let mut a = FakeAudio::default();
        let s = run(&mut machine(), &mut d, &mut a, &o);
        assert_eq!(s, Summary::default());
        assert!(d.presented.is_empty());
        assert_eq!(a.calls, 0, "and queued nothing");
    }

    /// Records what the loop queued, so a test can assert on the audio without a device.
    ///
    /// The counterpart of [`Fake`], and for the same reason: a real device's buffer
    /// cannot be read back, so what the loop *decides* about audio — how often it
    /// queues, what it does with a failure, whether a pause is reported — could only be
    /// checked by listening. `calls` is separate from `queued.len()` because "queued the
    /// samples twice" and "queued twice as many samples" are different bugs.
    #[derive(Debug)]
    struct FakeAudio {
        queued: Vec<i16>,
        calls: usize,
        paused: Vec<bool>,
        /// When set, every [`Audio::queue`] fails with this message.
        fail: Option<String>,
        /// What [`Audio::stats`] reports, so a test can check the numbers reach the
        /// board's sound panel rather than being read and dropped.
        stats: machine::resample::RingStats,
        /// What [`Audio::is_running`] reports. A field rather than a constant because
        /// the title bar branches on it, and a constant would leave one arm of that
        /// branch unreachable from any test.
        running: bool,
    }

    /// `running: true` by default — a sink that works is the ordinary case, and the
    /// tests that care about a dead one say so.
    impl Default for FakeAudio {
        fn default() -> Self {
            Self {
                queued: Vec::new(),
                calls: 0,
                paused: Vec::new(),
                fail: None,
                stats: machine::resample::RingStats::default(),
                running: true,
            }
        }
    }

    impl Audio for FakeAudio {
        fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
            self.calls += 1;
            if let Some(e) = &self.fail {
                return Err(e.clone());
            }
            self.queued.extend_from_slice(samples);
            Ok(())
        }
        fn queued(&self) -> usize {
            self.queued.len()
        }
        fn stats(&self) -> machine::resample::RingStats {
            self.stats
        }
        fn set_paused(&mut self, paused: bool) {
            self.paused.push(paused);
        }
        fn is_running(&self) -> bool {
            self.running
        }
    }

    /// Every frame's samples are queued, once, and the machine's buffer is drained — so
    /// a long run does not grow a `Vec` forever.
    ///
    /// The sample count is bounded on both sides against the rate, not against a second
    /// call to the loop: about 937 *frames* per frame at 55,930 Hz over 59.63 fps, so
    /// three frames is 2,800 give or take one frame's fractional remainder. A loop that
    /// queued the same buffer twice would land at 5,600 and a loop that never drained
    /// would grow quadratically. The buffer is interleaved stereo, so the queued length
    /// is that count times [`machine::resample::CHANNELS`].
    #[test]
    fn each_frame_queues_its_samples_exactly_once() {
        let (o, _s, _p) = opts("queue-once");
        let mut m = machine();
        let mut d = Fake::new(Fake::idle(3));
        let mut a = FakeAudio::default();
        let s = run(&mut m, &mut d, &mut a, &o);
        assert_eq!(s.frames, 3);
        assert_eq!(
            a.calls, 3,
            "one queue call per frame, not per tick or per two"
        );
        let frames = a.queued.len() / machine::resample::CHANNELS;
        assert_eq!(
            a.queued.len() % machine::resample::CHANNELS,
            0,
            "whole frames only: {} samples",
            a.queued.len()
        );
        assert!(
            (2_700..3_000).contains(&frames),
            "{frames} frames for 3 frames, expected about 2,811"
        );
        assert!(
            m.samples().is_empty(),
            "the machine's buffer must be drained, or it grows for the whole session: \
             {} left",
            m.samples().len()
        );
        assert!(s.notices.is_empty(), "{:?}", s.notices);
    }

    /// A paused frame still presents, but queues nothing new — because the machine did
    /// not run — and the sink is told about the pause so a drained ring is not counted as
    /// an underrun.
    #[test]
    fn a_paused_tick_queues_nothing_and_reports_the_pause() {
        let (o, _s, _p) = opts("queue-paused");
        let mut script = vec![Fake::held(&[Key::F11])];
        script.extend(Fake::idle(2));
        let mut d = Fake::new(script);
        let mut a = FakeAudio::default();
        let s = run(&mut machine(), &mut d, &mut a, &o);
        assert_eq!(s.frames, 0, "the premise: no frame ran");
        assert!(
            a.queued.is_empty(),
            "queued {} samples while paused",
            a.queued.len()
        );
        assert_eq!(
            a.calls, 0,
            "and did not call `queue` with an empty slice either"
        );
        // The pause reached the sink on every tick it was in effect, not just its edge:
        // `Ring::pop` reads the flag per callback, so a pause reported once and then
        // forgotten would resume counting underruns while still paused.
        assert_eq!(
            a.paused,
            vec![true, true, true],
            "the pause must be reported every tick it holds"
        );
        // And presented anyway, or the window goes black the moment you pause.
        assert_eq!(d.presented.len(), 3);
    }

    /// Resuming tells the sink the pause is over, and the samples flow again.
    ///
    /// Without this, a `set_paused(true)` that was never undone would silence the
    /// underrun counter for the rest of the session — the counter would read zero
    /// through a genuinely struggling host, which is the one thing it exists to say.
    #[test]
    fn resuming_tells_the_sink_the_pause_is_over() {
        let (o, _s, _p) = opts("queue-resume");
        let mut script = vec![Fake::held(&[Key::F11])];
        script.extend(Fake::idle(1));
        script.push(Fake::held(&[Key::F11]));
        script.extend(Fake::idle(1));
        let mut d = Fake::new(script);
        let mut a = FakeAudio::default();
        let s = run(&mut machine(), &mut d, &mut a, &o);
        assert_eq!(s.frames, 2, "the resume tick and the one after it");
        assert_eq!(
            a.paused,
            vec![true, true, false, false],
            "paused for two ticks, then running for two"
        );
        assert_eq!(a.calls, 2, "and the frames that ran queued their samples");
        assert!(!a.queued.is_empty());
    }

    /// A device that cannot take the samples is a notice, once, and the loop runs on.
    ///
    /// The same policy as a failed save, for a sharper reason: a dropped buffer is a
    /// click, and stopping someone's game over a click would be the worse failure. Once,
    /// because sixty identical lines a second is how a message gets hidden inside itself.
    #[test]
    fn a_failed_queue_is_one_notice_and_does_not_stop_the_loop() {
        let (o, _s, _p) = opts("queue-fails");
        let mut d = Fake::new(Fake::idle(4));
        let mut a = FakeAudio {
            fail: Some("the device went away".to_string()),
            ..FakeAudio::default()
        };
        let s = run(&mut machine(), &mut d, &mut a, &o);
        assert_eq!(s.frames, 4, "the loop ran every frame it owed");
        assert_eq!(a.calls, 4, "and kept trying rather than giving up");
        assert_eq!(
            s.notices,
            vec!["audio: the device went away".to_string()],
            "one notice for four identical failures"
        );
        assert_eq!(d.presented.len(), 4, "and the picture kept going");
    }

    /// A sink with no device behind it says so in the title, and a working one does not.
    ///
    /// Both halves, because the tag is only useful if its absence means something. `main`
    /// also prints the reason on `stderr`, but that line is gone from the scrollback five
    /// minutes later, and "I hear nothing" then has three explanations — no device, a
    /// silent game, a broken mix — with nothing on screen to choose between them.
    #[test]
    fn the_title_says_when_there_is_no_audio_device() {
        let (o, _s, _p) = opts("title-no-audio");
        let mut d = Fake::new(Fake::idle(2));
        let mut dead = FakeAudio {
            running: false,
            ..FakeAudio::default()
        };
        run(&mut machine(), &mut d, &mut dead, &o);
        assert!(
            d.titles.iter().any(|t| t.contains("[no audio]")),
            "a sink that is not running must say so: {:?}",
            d.titles
        );

        let (o2, _s2, _p2) = opts("title-has-audio");
        let mut d2 = Fake::new(Fake::idle(2));
        let mut live = FakeAudio::default();
        run(&mut machine(), &mut d2, &mut live, &o2);
        assert!(
            !d2.titles.iter().any(|t| t.contains("no audio")),
            "and a working device must not: {:?}",
            d2.titles
        );
    }

    /// The ring's drop and underrun counts reach the board, where the sound panel reads
    /// them.
    ///
    /// They are the host's numbers, not the board's — the ring is sized from a device
    /// sample rate `machine` has no business knowing — so they arrive through a setter,
    /// and a loop that read [`Audio::stats`] and dropped the result would leave the
    /// panel's `DRP` and `UND` columns reading zero through a session of clicks. The
    /// second run is the premise: without it, a `set_audio_stats(0, 0)` that ignored the
    /// sink would pass the first half.
    #[test]
    fn the_rings_counters_reach_the_sound_panel() {
        let (o, _s, _p) = opts("audio-stats");
        let mut m = machine();
        let mut d = Fake::new(Fake::idle(2));
        let mut a = FakeAudio {
            stats: machine::resample::RingStats {
                drops: 17,
                underruns: 4,
            },
            ..FakeAudio::default()
        };
        run(&mut m, &mut d, &mut a, &o);
        let t = cps1(&m).sound.trace();
        assert_eq!(t.audio_drops, 17, "drops is the first argument");
        assert_eq!(t.audio_underruns, 4, "and underruns the second");

        let (o2, _s2, _p2) = opts("audio-stats-quiet");
        let mut m2 = machine();
        let mut d2 = Fake::new(Fake::idle(2));
        run(&mut m2, &mut d2, &mut FakeAudio::default(), &o2);
        assert_eq!(
            (
                cps1(&m2).sound.trace().audio_drops,
                cps1(&m2).sound.trace().audio_underruns
            ),
            (0, 0),
            "a healthy sink leaves them at zero"
        );
    }

    /// The same counters, on an SF1, where they land on the machine and not a board.
    ///
    /// ⚠️ **This is the asymmetric fork.** CPS-1 hangs the pair on its one sound board;
    /// SF1 has two and the host ring is downstream of the mix, so `Sf1::set_audio_stats`
    /// takes them. The sibling above cannot cover this arm, and without this test the
    /// SF1 arm's two arguments could be swapped — 17 clicks reported as 17 starvations —
    /// with nothing to say so. `17` and `4` are the sibling's numbers on purpose: two
    /// distinct values, so a swap is visible, and the same two so the two tests read as
    /// one pair.
    #[test]
    fn the_rings_counters_reach_an_sf1_machine() {
        let (mut o, _s, _p) = opts("sf1-audio-stats");
        o.board = state_tag(machine::BoardKind::Sf1);
        let mut m = an_sf1_machine();
        let mut d = Fake::new(Fake::idle(2));
        let mut a = FakeAudio {
            stats: machine::resample::RingStats {
                drops: 17,
                underruns: 4,
            },
            ..FakeAudio::default()
        };
        run(&mut m, &mut d, &mut a, &o);
        assert_eq!(sf1(&m).audio_drops(), 17, "drops is the first argument");
        assert_eq!(sf1(&m).audio_underruns(), 4, "and underruns the second");

        let (mut o2, _s2, _p2) = opts("sf1-audio-stats-quiet");
        o2.board = state_tag(machine::BoardKind::Sf1);
        let mut m2 = an_sf1_machine();
        let mut d2 = Fake::new(Fake::idle(2));
        run(&mut m2, &mut d2, &mut FakeAudio::default(), &o2);
        assert_eq!(
            (sf1(&m2).audio_drops(), sf1(&m2).audio_underruns()),
            (0, 0),
            "a healthy sink leaves them at zero here too"
        );
    }

    /// Queuing does not change the machine.
    ///
    /// The audio is drained out of the machine, which is a mutation — so unlike the
    /// viewer's read-only paths this cannot be asserted as "nothing changed". What is
    /// asserted instead is that draining changes *only* the sample buffer: two runs of
    /// the same length, one with a sink that fails and one with a sink that succeeds,
    /// must reach the same machine. A failure path that skipped the drain would leave the
    /// buffer full and diverge.
    #[test]
    fn a_failing_sink_reaches_the_same_machine_as_a_working_one() {
        let (o, _s, _p) = opts("queue-same-a");
        let mut working = machine();
        let mut d = Fake::new(Fake::idle(3));
        let mut a = FakeAudio::default();
        run(&mut working, &mut d, &mut a, &o);

        let (o2, _s2, _p2) = opts("queue-same-b");
        let mut failing = machine();
        let mut d2 = Fake::new(Fake::idle(3));
        let mut a2 = FakeAudio {
            fail: Some("no device".to_string()),
            ..FakeAudio::default()
        };
        run(&mut failing, &mut d2, &mut a2, &o2);

        assert_eq!(
            cps1(&working).board.ram,
            cps1(&failing).board.ram,
            "the same memory"
        );
        assert_eq!(
            cps1(&working).total_cycles,
            cps1(&failing).total_cycles,
            "the same cycle count"
        );
        assert!(
            working.samples().is_empty() && failing.samples().is_empty(),
            "both drained: {} and {}",
            working.samples().len(),
            failing.samples().len()
        );
    }
}
