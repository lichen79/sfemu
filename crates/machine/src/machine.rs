//! Which board is running: a two-arm enum, and the narrow view its panels share.
//!
//! # Why an enum rather than a trait
//!
//! The frontend has about forty signatures that name a board — `debug.rs` 10,
//! `gfx.rs` 11, `gfxpanels.rs` 12, `overlay.rs` 15, `state.rs` 5, `loop_.rs` 21,
//! `main.rs` 6. They do not want a machine-shaped interface; they want *fields*.
//! A trait wide enough to serve them is a trait with forty methods and no
//! abstraction, with a virtual call in each of the debugger's inner loops, and it
//! makes `size_of` invisible — which on this codebase is not an abstract concern
//! (see [`Machine`]'s own note).
//!
//! Generics fare no better: `Cps1<B: BoardLike>` pushes the same forty methods into
//! a type parameter, monomorphizes the frontend twice, and turns every `&Cps1` in
//! five files into a signature whose caller has to name a type.
//!
//! # What each type is for
//!
//! [`Machine`] is a **dispatcher**. Every method on it is a two-arm match that
//! forwards to the board, and it has a method only where both boards can answer the
//! same question with the same type.
//!
//! [`CpuView`] is a **narrowing**. The 68000, its cycle count, the beam position and
//! the board's [`Trace`] are identical on both boards, because both hold an
//! [`M68k`] and a [`Trace`]. Everything in `debug.rs` and the register, disassembly,
//! memory and status panels reads only those — so they take a `CpuView<'_>` and
//! never learn there is a second board.
//!
//! # What is deliberately absent
//!
//! - **No `as_cps1`/`as_sf1`.** A caller writing `if let Some(c) = m.as_cps1()`
//!   silently does nothing on the other board: a panel that goes blank with no
//!   error. The graphics panels match on `&Machine` and bind the box in their arm,
//!   so the compiler requires both.
//! - **No `snapshot`/`restore`.** The two save states are different types with
//!   different payload sizes and a board tag whose whole purpose is to refuse a
//!   cross-load. The save path matches on the enum where that tag is in scope.
//! - **No `sound_trace`.** CPS-1 has one Z80 and an OKI; SF1 has two Z80s and two
//!   MSM5205s. A shared accessor would have to return a union of two unrelated
//!   counter sets, and the sound panel forks anyway.
//! - **No `framebuffer`.** The two are different types with different palettes and
//!   different pen ranges.
//! - **No [`core::ops::Deref`].** It would make `m.cpu` compile for exactly one arm.

use crate::cps1::Cps1;
use crate::sf1::Sf1;
use crate::timing::Timing;
use crate::trace::Trace;
use m68k::M68k;

/// CPS-1's frame period in nanoseconds: 16.768 ms, or 59.637 Hz.
///
/// **Derived** from the dot clock, which is why it is not 60 Hz. `frontend::pace`
/// holds the same number for its own default pacer; this is the copy [`Machine`]
/// answers with, so a host never has to know which board it is pacing.
const CPS1_FRAME_NS: u64 = 16_768_000;

/// SF1's frame period in nanoseconds: 16.667 ms, or 60 Hz.
///
/// **Asserted**, not derived: `sf.cpp:766` is `set_refresh_hz(60)`. 1e9/60 is
/// 16,666,666.67, rounded **up** — a period a hair long, so a pacer would rather
/// drop a frame than run early.
///
/// ⚠️ Not [`CPS1_FRAME_NS`]. A host pacing SF1 at CPS-1's period runs it 0.6% slow,
/// which is inaudible, invisible and permanent.
const SF1_FRAME_NS: u64 = 16_666_667;

/// Which board, as a `Copy` tag.
///
/// For callers that need to branch without holding the machine — a save path
/// choosing a board tag to write, a panel router deciding which draw function to
/// call. [`Machine::board`] produces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoardKind {
    /// Street Fighter II on CPS-1.
    Cps1,
    /// Street Fighter on SF1's own board.
    Sf1,
}

/// Everything the CPU, disassembly, memory and status panels read.
///
/// The two big fields are **borrowed** and the three small ones **copied**. A
/// [`Trace`] owns a `Vec<u32>` of PC samples and two unmapped-access logs, so a
/// by-value view would clone that vector once per panel per frame; an [`M68k`] is
/// around 200 bytes and `draw_regs` reads sixteen registers out of it. The three
/// scalars are copied because a `&u64` in a struct a panel formats is a pointer
/// chase for eight bytes.
///
/// ⚠️ Borrowed rather than snapshotted is also a correctness property: a view built
/// by value shows the registers as of when it was made, and a panel drawn once a
/// frame from a view made earlier in that frame lags — which reads as the emulator
/// being slow rather than the panel being stale.
#[derive(Debug, Clone, Copy)]
pub struct CpuView<'a> {
    /// The 68000, live.
    pub cpu: &'a M68k,
    /// The board's instrument counters.
    pub trace: &'a Trace,
    /// 68000 cycles since the last reset.
    pub total_cycles: u64,
    /// The scanline the beam is on.
    pub line: u32,
    /// Whether a vblank interrupt is waiting to be acknowledged.
    ///
    /// A `bool` rather than the IPL level, because that is what the status panel
    /// prints and both boards answer it the same way — even though the levels differ
    /// (CPS-1 drives level 2 at autovector 0x68, SF1 level 1 at 0x64).
    pub vblank_pending: bool,
}

/// The board that is running.
///
/// # ⚠️ Both arms are boxed, and a test asserts the size
///
/// A [`Cps1`] measures **5,232 bytes** today — `cps1.rs:120` recorded 5,088 when it
/// was written and the struct has grown since — and an [`Sf1`] is larger. Unboxed, this enum is the
/// bigger board plus a tag **by value** in every `Machine` anywhere — the
/// `Option<Machine>` a caller keeps, the temporary a `match` moves, the local a test
/// builds. `cps1.rs:127` records what that costs: an inline [`m68k::decode::Decoder`] made
/// `size_of::<Cps1>()` 529,360 bytes and eleven passing tests began aborting with
/// `fatal runtime error: stack overflow`, which is a process abort naming an
/// arbitrary test rather than a failure naming the cause. The boxes shut that door,
/// and `the_enum_is_a_pointer_and_a_tag` keeps it shut.
///
/// # Why [`core::fmt::Debug`] is hand-written
///
/// ⚠️ **[`Cps1`] implements [`Debug`](core::fmt::Debug) neither by derive nor by
/// hand.** Verified against the tree: `cps1.rs` contains no `derive(Debug)` on the
/// struct and no `impl Debug for Cps1`. A `#[derive(Debug)]` on this enum therefore
/// **does not compile** — and the fix is not to derive it on `Cps1`, because a
/// derived one prints a 4 MB ROM `Vec`, a 512 KB decoder and a graphics-RAM array
/// into whatever panic message asked for it. [`Sf1`]'s is hand-written for the same
/// reason. This one prints the arm and the four numbers a reader of a panic actually
/// wants.
pub enum Machine {
    /// Street Fighter II on CPS-1.
    Cps1(Box<Cps1>),
    /// Street Fighter on SF1's own board.
    Sf1(Box<Sf1>),
}

impl core::fmt::Debug for Machine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let v = self.cpu_view();
        f.debug_struct("Machine")
            .field("board", &self.board())
            .field("line", &v.line)
            .field("total_cycles", &v.total_cycles)
            .field("frames", &v.trace.frames)
            .field("pc", &format_args!("{:#08x}", v.cpu.pc))
            .finish_non_exhaustive()
    }
}

impl Machine {
    /// Which board this is.
    #[must_use]
    pub const fn board(&self) -> BoardKind {
        match self {
            Machine::Cps1(_) => BoardKind::Cps1,
            Machine::Sf1(_) => BoardKind::Sf1,
        }
    }

    /// The CPU state every panel shares.
    #[must_use]
    pub fn cpu_view(&self) -> CpuView<'_> {
        match self {
            Machine::Cps1(m) => CpuView {
                cpu: &m.cpu,
                trace: &m.board.trace,
                total_cycles: m.total_cycles,
                line: m.line,
                vblank_pending: m.board.vblank_pending(),
            },
            Machine::Sf1(m) => CpuView {
                cpu: &m.cpu,
                trace: &m.board.trace,
                total_cycles: m.total_cycles,
                line: m.line,
                vblank_pending: m.board.vblank_pending(),
            },
        }
    }

    /// The board's clocks and geometry.
    ///
    /// [`Timing`] is `Copy`, so this returns it by value: it is five small integers
    /// and a ratio, and a borrow would tie a caller's lifetime to the machine for
    /// data it is about to divide.
    #[must_use]
    pub fn timing(&self) -> Timing {
        match self {
            Machine::Cps1(m) => m.timing,
            Machine::Sf1(m) => m.timing,
        }
    }

    /// The board's frame period in nanoseconds, for the host's pacer.
    ///
    /// ⚠️ Not one constant. See this module's private `CPS1_FRAME_NS` and
    /// `SF1_FRAME_NS` — a public doc comment cannot link a private item: one is
    /// derived from a dot clock and the other is asserted at 60 Hz, and pacing either
    /// board at the other's period is a 0.6% speed error that nothing surfaces.
    #[must_use]
    pub const fn frame_ns(&self) -> u64 {
        match self {
            Machine::Cps1(_) => CPS1_FRAME_NS,
            Machine::Sf1(_) => SF1_FRAME_NS,
        }
    }

    /// The word at `addr` as a debugger sees it, or `None` if nothing decodes it.
    ///
    /// `None` and `Some(0xFFFF)` stay distinct all the way through: "no chip here"
    /// and "a chip that reads as all ones" are different facts, and the memory panel
    /// prints `--` for the first. Flattening them would make the panel claim a chip
    /// exists — CPS-1's 0x800020 genuinely reads 0xFFFF and *is* decoded, which is
    /// what makes the distinction real rather than theoretical.
    #[must_use]
    pub fn peek_word(&self, addr: u32) -> Option<u16> {
        match self {
            Machine::Cps1(m) => m.peek_word(addr),
            Machine::Sf1(m) => m.peek_word(addr),
        }
    }

    /// Runs one 68000 instruction, returning the cycles it consumed.
    pub fn step_instruction(&mut self) -> u32 {
        match self {
            Machine::Cps1(m) => m.step_instruction(),
            Machine::Sf1(m) => m.step_instruction(),
        }
    }

    /// Runs one scanline, returning the 68000 cycles spent.
    pub fn run_scanline(&mut self) -> u32 {
        match self {
            Machine::Cps1(m) => m.run_scanline(),
            Machine::Sf1(m) => m.run_scanline(),
        }
    }

    /// Runs a whole frame.
    pub fn run_frame(&mut self) {
        match self {
            Machine::Cps1(m) => m.run_frame(),
            Machine::Sf1(m) => m.run_frame(),
        }
    }

    /// Renders the current state into the board's framebuffer.
    pub fn render(&mut self) {
        match self {
            Machine::Cps1(m) => m.render(),
            Machine::Sf1(m) => m.render(),
        }
    }

    /// Power-up.
    pub fn reset(&mut self) {
        match self {
            Machine::Cps1(m) => m.reset(),
            Machine::Sf1(m) => m.reset(),
        }
    }

    /// The samples produced since the last drain, interleaved L,R.
    ///
    /// Two channels on both boards after Task 14: CPS-1's mono mix is written into
    /// both slots, so a host has one buffer layout to handle rather than a layout
    /// that depends on which game was loaded.
    #[must_use]
    pub fn samples(&self) -> &[i16] {
        match self {
            Machine::Cps1(m) => m.samples(),
            Machine::Sf1(m) => m.samples(),
        }
    }

    /// Takes the produced samples.
    pub fn drain_samples(&mut self) -> Vec<i16> {
        match self {
            Machine::Cps1(m) => m.drain_samples(),
            Machine::Sf1(m) => m.drain_samples(),
        }
    }

    /// Frames completed since the last reset.
    ///
    /// From the board's [`Trace`], which is the one place either board counts them —
    /// so this and `cpu_view().trace.frames` cannot disagree.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.cpu_view().trace.frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BoardConfig;
    use crate::timing::Timing;

    /// A 68000 program that spins at 0x400, for both boards.
    ///
    /// Big-endian: vector 0 is SSP and vector 1 is PC, both read through the bus by
    /// `M68k::reset`. `0x60FE` is `bra.s -2`.
    fn prog() -> Vec<u8> {
        let mut rom = vec![0u8; 0x0800];
        rom[0..4].copy_from_slice(&0x00FF_F000u32.to_be_bytes());
        rom[4..8].copy_from_slice(&0x0000_0400u32.to_be_bytes());
        rom[0x400..0x402].copy_from_slice(&0x60FEu16.to_be_bytes());
        rom
    }

    /// The same program with eight `nop`s ahead of the spin.
    ///
    /// `0x4E71` is `nop`. For `a_fresh_view_shows_the_current_registers`, which
    /// cannot use a program whose PC stands still.
    fn nop_sled() -> Vec<u8> {
        let mut rom = prog();
        for i in 0..8 {
            rom[0x400 + i * 2..0x402 + i * 2].copy_from_slice(&0x4E71u16.to_be_bytes());
        }
        rom[0x410..0x412].copy_from_slice(&0x60FEu16.to_be_bytes());
        rom
    }

    fn a_cps1() -> Machine {
        let mut m = Cps1::new(&prog(), BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        Machine::Cps1(Box::new(m))
    }

    fn an_sf1() -> Machine {
        let mut m = Sf1::new(
            &prog(),
            crate::sf1::test_video(),
            vec![0x18, 0xFE],
            vec![0x18, 0xFE],
        );
        m.reset();
        Machine::Sf1(Box::new(m))
    }

    /// Both machines, so every test below runs on both without saying so twice.
    fn both() -> [Machine; 2] {
        [a_cps1(), an_sf1()]
    }

    /// A `Machine` is a pointer and a tag, not a board by value.
    ///
    /// ⚠️ Asserted against a literal for the reason `cps1.rs:127` records its two
    /// numbers: unboxed, this enum is the larger board plus a tag in every `Machine`
    /// anywhere, and the failure is `fatal runtime error: stack overflow` — a process
    /// abort naming an arbitrary test.
    #[test]
    fn the_enum_is_a_pointer_and_a_tag() {
        assert_eq!(
            size_of::<Machine>(),
            16,
            "two boxed arms plus a discriminant; an unboxed arm is thousands"
        );
        assert!(
            size_of::<Cps1>() > 4_000 && size_of::<Sf1>() > 4_000,
            "the premise: both boards are large by value, so the boxes are load-bearing"
        );
    }

    /// The tag says which board, and it is not derived from anything else.
    #[test]
    fn the_tag_names_the_board() {
        assert_eq!(a_cps1().board(), BoardKind::Cps1);
        assert_eq!(an_sf1().board(), BoardKind::Sf1);
        assert_ne!(BoardKind::Cps1, BoardKind::Sf1);
    }

    /// The CPU view reaches the same fields on both boards.
    ///
    /// The whole point of the type: five fields, identical shape, no board named.
    #[test]
    fn the_cpu_view_is_the_same_shape_on_both_boards() {
        for mut m in both() {
            let board = m.board();
            {
                let v = m.cpu_view();
                assert_eq!(v.total_cycles, 0, "{board:?}: fresh");
                assert_eq!(v.line, 0);
                assert!(!v.vblank_pending);
                assert_eq!(v.trace.frames, 0);
                assert_ne!(v.cpu.pc, 0, "{board:?}: reset loaded a PC from the vector");
            }
            m.run_scanline();
            let v = m.cpu_view();
            assert!(
                v.total_cycles > 0,
                "{board:?}: the view follows the machine"
            );
        }
    }

    /// A fresh view shows the registers as they are now, not as they were.
    ///
    /// ⚠️ **This does not test that `CpuView` borrows rather than copies, and no
    /// runtime test can.** A borrowed view cannot be held across a `&mut self` call
    /// at all — the borrow checker rejects it — so "the view is live" is enforced by
    /// the signature at compile time, and a by-value `CpuView` would satisfy every
    /// assertion a test could write. What is checkable, and what this checks, is that
    /// each freshly-taken view reflects the machine's current state. The borrow's
    /// purpose is the cost argument on [`CpuView`] (a `Trace` owns a `Vec` of PC
    /// samples), not a behaviour.
    ///
    /// The program is a NOP sled rather than the spin loop, because a spin loop's PC
    /// is *constant*: asserting it equals its earlier value passes whether the view
    /// tracks the machine or not. Measured — eight `step_instruction` calls on the
    /// spin program leave `pc` at 0x404 every time.
    #[test]
    fn a_fresh_view_shows_the_current_registers() {
        let mut s = Sf1::new(
            &nop_sled(),
            crate::sf1::test_video(),
            vec![0x18, 0xFE],
            vec![0x18, 0xFE],
        );
        s.reset();
        let mut m = Machine::Sf1(Box::new(s));
        let pc_then = m.cpu_view().cpu.pc;
        m.step_instruction();
        let pc_now = m.cpu_view().cpu.pc;
        assert_eq!(pc_now, pc_then + 2, "one `nop` advanced the PC by one word");
    }

    /// Stepping, running a line, and running a frame all reach both boards.
    #[test]
    fn the_scheduler_forwards_on_both_boards() {
        for mut m in both() {
            let board = m.board();
            let c = m.step_instruction();
            assert!(c > 0, "{board:?}: an instruction costs cycles");
            let line = m.run_scanline();
            assert!(line > 0, "{board:?}: a line costs cycles");
            let before = m.cpu_view().total_cycles;
            m.run_frame();
            assert!(
                m.cpu_view().total_cycles > before,
                "{board:?}: a frame costs cycles"
            );
            assert!(m.frames() > 0, "{board:?}: and completed at least one");
        }
    }

    /// `reset` reaches both boards and puts the cycle count back.
    #[test]
    fn reset_forwards_on_both_boards() {
        for mut m in both() {
            let board = m.board();
            m.run_frame();
            assert_ne!(m.cpu_view().total_cycles, 0, "{board:?}: the premise");
            m.reset();
            assert_eq!(m.cpu_view().total_cycles, 0, "{board:?}");
            assert_eq!(m.cpu_view().line, 0);
            assert!(m.samples().is_empty(), "{board:?}: audio is dropped");
        }
    }

    /// Peeking reaches the board's own decode, and an undecoded address is `None`.
    ///
    /// `None` and `Some(0xFFFF)` are different facts — the memory panel prints `--`
    /// for one and `FFFF` for the other — so a forward that flattened them would make
    /// the panel lie about which chips exist.
    #[test]
    fn peeking_forwards_and_keeps_the_undecoded_case() {
        for m in both() {
            let board = m.board();
            assert_eq!(
                m.peek_word(0),
                Some(0x00FF),
                "{board:?}: the SSP's high word"
            );
            assert_eq!(
                m.peek_word(0x40_0000),
                None,
                "{board:?}: one past CPS-1's 0x3FFFFF ROM top, far past SF1's 0x4FFFF"
            );
            // ⚠️ Not 0xFFFFFE. That address is decoded on *both* boards — CPS-1 maps
            // RAM at 0xFF0000-0xFFFFFF (`board.rs:336`) and SF1 maps object RAM at
            // 0xFFE000-0xFFFFFF. Measured: CPS-1's `peek_word(0x00FF_FFFE)` returns
            // `Some(0x0000)`. "The top of the address space is obviously unmapped" is
            // the wrong instinct on a 68000 board, because the top of the space is
            // where the RAM is.
        }
    }

    /// Each board's timing is its own, and SF1's is not CPS-1's.
    #[test]
    fn the_timing_is_the_boards_own() {
        let cps1 = a_cps1().timing();
        let sf1 = an_sf1().timing();
        assert_eq!(cps1.line_cycles, (640, 1), "CPS-1: exactly 640");
        assert_eq!(sf1.line_cycles, (3125, 6), "SF1: 520.83");
        assert_eq!(cps1.lines_per_frame, 262);
        assert_eq!(sf1.lines_per_frame, 256);
        assert_eq!(cps1.vblank_line, 240, "both are 240, by coincidence");
        assert_eq!(sf1.vblank_line, 240);
    }

    /// The frame period comes from the board, and the two differ.
    ///
    /// ⚠️ CPS-1's 16,768,000 ns is 59.637 Hz *derived* from its dot clock; SF1's is a
    /// nominal 60 Hz. A host pacing SF1 at CPS-1's period runs it 0.6% slow —
    /// inaudible, invisible, permanent — which is why this is on the machine rather
    /// than a constant in the pacer.
    #[test]
    fn the_frame_period_is_the_boards_own() {
        assert_eq!(
            a_cps1().frame_ns(),
            16_768_000,
            "59.637 Hz, CPS-1's derived rate"
        );
        assert_eq!(
            an_sf1().frame_ns(),
            16_666_667,
            "60 Hz, SF1's asserted rate"
        );
        assert_ne!(a_cps1().frame_ns(), an_sf1().frame_ns());
        // The rounding direction is stated rather than left to the reader: 1e9/60 is
        // 16,666,666.67, and rounding up is a period a hair long — a pacer that would
        // rather drop a frame than run early.
        assert_eq!(
            1_000_000_000 / an_sf1().frame_ns(),
            59,
            "59.9999 Hz, rounded down"
        );
    }

    /// Audio drains through the enum, in whole stereo frames on both boards.
    ///
    /// Task 14 made CPS-1's mono value fan out into two slots, so "interleaved, always
    /// two channels" is a property of both boards and this is where it is checked
    /// through the type the host actually holds.
    ///
    /// ⚠️ **This test depends on Task 14 and fails without it.** Measured on the tree
    /// before the widening, one CPS-1 frame of the spin program produces **937** samples
    /// — an *odd*
    /// number, because each is one mono value. So `n % CHANNELS == 0` is a real check
    /// that Task 14's widening reached all the way through `Cps1::samples`, not a
    /// tautology. If it fails, the widening stopped short; do not relax the modulus.
    #[test]
    fn audio_drains_in_whole_frames_on_both_boards() {
        for mut m in both() {
            let board = m.board();
            m.run_frame();
            let n = m.samples().len();
            assert!(n > 0, "{board:?}: a frame produced audio");
            assert_eq!(
                n % crate::resample::CHANNELS,
                0,
                "{board:?}: {n} samples is not a whole number of frames"
            );
            let taken = m.drain_samples();
            assert_eq!(taken.len(), n, "{board:?}: the drain took all of it");
            assert!(m.samples().is_empty(), "{board:?}: and left none");
        }
    }

    /// Rendering forwards, and each board draws into its own framebuffer.
    ///
    /// Checked through the arm rather than through `Machine`, because the two
    /// framebuffers are different types with different palettes — which is exactly why
    /// there is no `Machine::framebuffer()`.
    #[test]
    fn rendering_forwards_to_each_boards_own_framebuffer() {
        for mut m in both() {
            let board = m.board();
            m.render();
            match &m {
                Machine::Cps1(c) => assert_eq!(
                    c.video.fb.pens.len(),
                    video::WIDTH * video::HEIGHT,
                    "{board:?}"
                ),
                Machine::Sf1(s) => assert_eq!(
                    s.video.fb.pens.len(),
                    video::WIDTH * video::HEIGHT,
                    "{board:?}"
                ),
            }
        }
    }

    /// The frame counter is the board's trace, on both.
    #[test]
    fn the_frame_count_is_the_traces() {
        for mut m in both() {
            let board = m.board();
            assert_eq!(m.frames(), 0, "{board:?}");
            m.run_frame();
            assert_eq!(m.frames(), 1, "{board:?}");
            assert_eq!(
                m.frames(),
                m.cpu_view().trace.frames,
                "{board:?}: one source"
            );
        }
    }

    /// Every `Machine` method reaches both arms.
    ///
    /// ⚠️ The failure this exists for: a forwarding method written with one arm
    /// implemented and the other `todo!()` or copied from its neighbour compiles, and
    /// every CPS-1 test passes. The panel that calls it panics on SF1 only, in a
    /// release build the user is running. Every method above is called on `an_sf1()`
    /// somewhere in this module; this test is the reminder to keep that true when a
    /// method is added.
    #[test]
    fn every_method_is_exercised_on_the_sf1_arm() {
        let mut m = an_sf1();
        let _ = m.board();
        let _ = m.timing();
        let _ = m.frame_ns();
        let _ = m.peek_word(0);
        let _ = m.frames();
        let _ = m.cpu_view();
        let _ = m.step_instruction();
        let _ = m.run_scanline();
        m.run_frame();
        m.render();
        let _ = m.samples();
        let _ = m.drain_samples();
        m.reset();
    }
}
