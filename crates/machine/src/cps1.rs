//! The machine: a CPU, a board, and the schedule that interleaves them.

use crate::board::Board;
use crate::config::BoardConfig;
use crate::snapshot::MachineState;
use crate::timing::Timing;
use m68k::{decode::Decoder, M68k};
use video::compose::Video;

/// A CPS-1 machine: 68000, board, and frame schedule.
///
/// # Why the CPU and the board are separate fields
///
/// `M68k::step_with(&dec, &mut bus)` borrows both mutably at once, so the CPU
/// cannot live inside the thing it buses to. Holding them side by side here makes
/// `self.cpu.step_with(&self.dec, &mut self.board)` legal with no `RefCell` and no
/// `unsafe`.
pub struct Cps1 {
    /// The 68000.
    pub cpu: M68k,
    /// Everything on its bus.
    pub board: Board,
    /// The frame schedule.
    pub timing: Timing,
    /// Total 68000 cycles since the last [`Cps1::reset`].
    ///
    /// `u64` because 167,680 cycles per frame at 59.64 Hz overflows a `u32` in
    /// under twelve minutes of gameplay — long enough that a `u32` here would pass
    /// every test in this crate and then wrap during a real match.
    pub total_cycles: u64,
    /// The current scanline, `0..lines_per_frame`.
    pub line: u32,
    /// The current scanline's remaining cycle budget, counting down; `<= 0` between
    /// lines, where it is how far the last instruction overran.
    ///
    /// The 68000 cannot be stopped mid-instruction — a `divs` costs 158 cycles and
    /// does not divide at a scanline boundary — so overshoot is inherent. Carrying
    /// it forward means the *only* error at any moment is the current line's
    /// overshoot, never a sum of them. Dropping it would make every scanline
    /// slightly long and the frame rate slightly slow: music drifting against
    /// animation over a match, with nothing ever looking broken enough to
    /// investigate.
    ///
    /// It holds the *live* budget rather than only the overshoot because that is
    /// what makes [`Cps1::step_instruction`] possible: a budget living in
    /// `run_scanline`'s stack frame cannot survive a return to the caller, and a
    /// debugger stepping one instruction is exactly such a caller. The two readings
    /// coincide wherever anything observes this field — [`Cps1::run_scanline`] and
    /// [`Cps1::run_frame`] both return with a line finished — so a
    /// [`MachineState`] saved at a frame boundary carries the same value and means
    /// the same thing as before.
    carry: i64,
    /// The video subsystem: the graphics ROM, the object latch, and the frame.
    pub video: Video,
    /// Built once. `Decoder::new` fills a 65,536-entry table, so constructing one
    /// per step would dominate the run time.
    dec: Decoder,
}

impl Cps1 {
    /// A machine with `prog` in ROM space and no graphics ROM.
    ///
    /// Call [`Cps1::reset`] before stepping. With no graphics every tile decodes as
    /// absent, so [`Cps1::render`] produces a uniform background frame — which is
    /// what every test in this crate that does not care about pixels wants.
    pub fn new(prog: &[u8], cfg: BoardConfig, timing: Timing) -> Self {
        Self::with_gfx(prog, Vec::new(), cfg, timing)
    }

    /// A machine with `prog` in ROM space and `gfx` as its graphics ROM.
    ///
    /// `gfx` is the board's assembled graphics region, supplied by the caller. This
    /// crate holds no ROM.
    pub fn with_gfx(prog: &[u8], gfx: Vec<u8>, cfg: BoardConfig, timing: Timing) -> Self {
        Self {
            cpu: M68k::new(),
            board: Board::new(prog, cfg),
            timing,
            total_cycles: 0,
            line: 0,
            carry: 0,
            video: Video::new(cfg.video, cfg.mapper, gfx),
            dec: Decoder::new(),
        }
    }

    /// Renders the current board state into [`Cps1::video`]'s framebuffer.
    ///
    /// Uses the object table as [`Cps1::run_scanline`] last latched it, which is a
    /// frame behind — CPS-1 sprites are delayed one frame (`cps1_v.cpp:3067-3068`).
    pub fn render(&mut self) {
        self.video
            .render(&self.board.gfxram[..], &self.board.cps_a, &self.board.cps_b);
    }

    /// Power-up: the CPU takes SSP and PC from vectors 0 and 1, and the schedule
    /// returns to the top of a frame with no carried debt.
    pub fn reset(&mut self) {
        self.cpu.reset(&mut self.board);
        self.total_cycles = 0;
        self.line = 0;
        self.carry = 0;
    }

    /// The word at `addr` as a debugger sees it, or `None` if nothing decodes it.
    ///
    /// See [`Board::peek_word`]: no side effects, which is why a debugger does not
    /// read through the CPU's own path. `&self` is what enforces it.
    pub fn peek_word(&self, addr: u32) -> Option<u16> {
        self.board.peek_word(addr)
    }

    /// Runs exactly one 68000 instruction, returning the cycles it consumed.
    ///
    /// The debugger's stepping primitive, and [`Cps1::run_scanline`] is a loop over
    /// it — **one code path deliberately.** A separate stepping path is a debugger
    /// that single-steps a machine subtly unlike the one that runs, and the IRQ sync
    /// below is the specific thing that would be left out of it: the symptom would be
    /// a machine that takes no interrupts under the debugger, which is the thing a
    /// debugger is most often opened to investigate.
    ///
    /// One instruction can overrun the scanline budget — a `divs` costs 158 cycles
    /// and does not divide at a line boundary — so this may end a line, in which case
    /// it does everything the end of a line does: sample the PC, advance the beam, and
    /// count a frame on the wrap. Arriving at [`Timing::vblank_line`] asserts vblank
    /// and latches the object table, once for the line however many instructions it
    /// takes.
    pub fn step_instruction(&mut self) -> u32 {
        // The start-of-line work. It lives here rather than in `run_scanline` so that
        // a caller which only ever steps still gets it, and it is guarded on the
        // budget being spent so it happens once per line rather than once per
        // instruction.
        if self.carry <= 0 {
            self.carry += i64::from(self.timing.cycles_per_line);
            // Vblank: IPL1 on the line the beam leaves the visible area
            // (`cps1.cpp:394-396`). CPS-1 wires the IPL pins individually —
            // `set_interrupt_mixer(false)`, `cps1.cpp:3913` — so IPL1 is level 2, not
            // an encoded priority.
            if self.line == self.timing.vblank_line {
                self.board.assert_vblank();
                // The object table is latched here, once per frame, at the same
                // instant vblank is asserted — `cps1_v.cpp:3060-3068`, where the
                // memcpy sits in `screen_vblank_cps1` under "CPS1 sprites have to be
                // delayed one frame". Taking it from the frame schedule rather than
                // simulating a delay is what makes the delay exactly one frame for a
                // caller driving scanlines — or instructions — by hand as much as for
                // a `run_frame` caller.
                self.video
                    .latch_objects(&self.board.gfxram[..], &self.board.cps_a);
            }
        }
        // Re-drive the level from the board's own state before **every** step, not
        // once per scanline.
        //
        // `M68k::pending_irq` is a level and nothing in the core clears it, while the
        // acknowledge happens on the board — on the far side of the bus. Set once per
        // line, the level would still read 2 after the handler's `rte` dropped the
        // mask, so the handler would re-enter for the rest of the line: the 640-cycle
        // budget fits about seven passes of a 90-cycle handler. Syncing per step is
        // what makes "the board owns deassertion" actually true, and it costs one
        // field write per instruction.
        self.cpu
            .set_irq(if self.board.vblank_pending() { 2 } else { 0 });
        // A halted CPU still burns time — the core returns 4 rather than 0 — so the
        // budget always decreases and a line can always end.
        let c = self.cpu.step_with(&self.dec, &mut self.board);
        self.carry -= i64::from(c);
        self.total_cycles += u64::from(c);
        if self.carry <= 0 {
            // One sample per scanline, taken after the line has run so the PC is where
            // the program got to rather than where it started.
            self.board.trace.sample_pc(self.cpu.pc);
            self.line = (self.line + 1) % self.timing.lines_per_frame;
            // Counted on the wrap rather than in `run_frame`, so a caller driving
            // scanlines by hand — the debugger, and every test in this crate — counts
            // the same frames a `run_frame` caller does.
            if self.line == 0 {
                self.board.trace.frames += 1;
            }
        }
        c
    }

    /// Runs one scanline's worth of CPU, returning the cycles actually consumed.
    ///
    /// Consumes at least `cycles_per_line + carry` cycles; the excess becomes the
    /// next line's carry, so the running total never drifts above
    /// `cycles_per_line × lines` by more than one instruction's cost.
    pub fn run_scanline(&mut self) -> u32 {
        let start = self.total_cycles;
        let line = self.line;
        // `step_instruction` advances `line` when the budget runs out, so this ends
        // when — and only when — the line it was called on has finished. It cannot
        // spin: every step spends at least four cycles against a finite budget.
        while self.line == line {
            self.step_instruction();
        }
        // A line is hundreds of cycles, so this cannot truncate; `try_from` rather
        // than `as` so that if it somehow could, it saturates visibly instead of
        // wrapping to a small number that looks like a fast line.
        u32::try_from(self.total_cycles - start).unwrap_or(u32::MAX)
    }

    /// Runs `lines_per_frame` scanlines.
    pub fn run_frame(&mut self) {
        for _ in 0..self.timing.lines_per_frame {
            self.run_scanline();
        }
    }

    /// Everything that decides the machine's future, copied out.
    ///
    /// Not the ROM, the graphics ROM, the decoder, or the [`Trace`](crate::Trace):
    /// see [`MachineState`] for why each is absent.
    pub fn snapshot(&self) -> MachineState {
        MachineState {
            cpu: self.cpu.clone(),
            // `boxed_copy` and not `.clone()`: see its documentation.
            ram: crate::snapshot::boxed_copy(&self.board.ram),
            gfxram: crate::snapshot::boxed_copy(&self.board.gfxram),
            cps_a: self.board.cps_a,
            cps_b: self.board.cps_b,
            sound_latch: self.board.sound_latch,
            coin_ctrl: self.board.coin_ctrl,
            vblank_pending: self.board.vblank_pending(),
            inputs: self.board.inputs,
            total_cycles: self.total_cycles,
            line: self.line,
            carry: self.carry,
            obj: self.video.obj_latch().clone(),
        }
    }

    /// Puts a snapshot back.
    ///
    /// The two large arrays are copied into the existing boxes rather than replacing
    /// them, so loading a state does not allocate 208 KB per press of the load key.
    ///
    /// Leaves the ROM, the graphics ROM, the decoder, and the trace alone. The trace
    /// especially: it records the session rather than the machine, and rewinding it
    /// on every load would make a divergence test compare a run's counters against a
    /// copy of themselves.
    pub fn restore(&mut self, s: &MachineState) {
        self.cpu = s.cpu.clone();
        self.board.ram.copy_from_slice(&s.ram[..]);
        self.board.gfxram.copy_from_slice(&s.gfxram[..]);
        self.board.cps_a = s.cps_a;
        self.board.cps_b = s.cps_b;
        self.board.sound_latch = s.sound_latch;
        self.board.coin_ctrl = s.coin_ctrl;
        self.board.set_vblank_pending(s.vblank_pending);
        self.board.inputs = s.inputs;
        self.total_cycles = s.total_cycles;
        self.line = s.line;
        self.carry = s.carry;
        self.video.set_obj_latch(&s.obj);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The longest instruction in [`spin`], in cycles.
    ///
    /// Measured on this core, not assumed: `move #imm,sr` is 16, `bra.s` is 10, `nop`
    /// is 4. Every bound below is `640 × lines + SLOWEST` and this is the only place
    /// the number appears, so a wrong value shows up as a test that no longer
    /// discriminates rather than as a test that passes wrongly — and the mutation
    /// pass is what confirms it still discriminates.
    const SLOWEST: u64 = 16;

    /// A masking prologue and then a two-instruction loop that never terminates.
    ///
    /// ```text
    /// 1000  46FC 2700   move #$2700,sr   supervisor, interrupt mask 7
    /// 1004  4E71        nop
    /// 1006  60FC        bra  $1004
    /// ```
    ///
    /// # Why the mask
    ///
    /// `M68k::reset` leaves SR at 0x2000 — supervisor, mask **0** — so from Task 8 on
    /// the scheduler asserts IPL1 at line 240 and this program would vector through
    /// an all-zero vector table into ROM and stop being a loop. Masking to 7 keeps
    /// these tests measuring the schedule and nothing else; the interrupt itself is
    /// covered by `tests/programs.rs`.
    ///
    /// # Why not the obvious `bra.s -2`
    ///
    /// A lone `bra.s -2` costs 10 cycles and **640 / 10 = 64 exactly**, so every
    /// scanline would end precisely on its budget and the carry would be zero for the
    /// whole run. A test built on it cannot tell a working carry from
    /// `self.carry = 0`: the mutant would survive with the suite green, which is the
    /// project's characteristic defect wearing a scheduler costume.
    ///
    /// `nop` + `bra.s` is 14 cycles, and 640 = 45 × 14 + 10, so the loop straddles
    /// nearly every scanline boundary and the carry is exercised on nearly every
    /// line.
    ///
    /// Encodings verified against `m68k::disasm` on 2026-08-07: `0x46FC 0x2700`
    /// renders `move #$2700,sr`, `0x4E71` renders `nop`, and `0x60FC` at 0x1006
    /// renders `bra $1004`.
    fn spin() -> Vec<u8> {
        let mut rom = vec![0u8; 0x2000];
        // Reset vector: SSP 0x00FF8000 (top of main RAM), PC 0x00001000.
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x1000..0x1008].copy_from_slice(&[0x46, 0xFC, 0x27, 0x00, 0x4E, 0x71, 0x60, 0xFC]);
        rom
    }

    fn machine() -> Cps1 {
        let mut m = Cps1::new(&spin(), BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        m
    }

    /// [`spin`], but with the mask down and a level-2 handler that counts itself.
    ///
    /// ```text
    /// 0068  0000 2000        the level-2 autovector -> 0x2000
    /// 1000  46FC 2000   move  #$2000,sr        supervisor, interrupt mask 0
    /// 1004  4E71        nop
    /// 1006  60FC        bra   $1004
    /// 2000  5279 00FF 0000   addq.w #1,$FF0000
    /// 2006  4E73        rte
    /// ```
    ///
    /// The fixture for everything about [`Cps1::step_instruction`], and it is this
    /// program rather than [`spin`] for three reasons, each of which a weaker fixture
    /// would silently drop from the claim:
    ///
    /// - **The interrupt is taken and acknowledged.** `spin` masks to 7, so
    ///   `trace.acks` stays 0 and a stepping path that forgot to re-drive the IRQ
    ///   level would compare equal to a running one. Here the handler's own
    ///   increment in `ram[0]` is the artifact.
    /// - **The loop straddles scanline boundaries.** `nop` + `bra.s` is 14 cycles
    ///   against a 640-cycle budget, so the carry is non-zero on nearly every line
    ///   — see [`spin`]'s own note on why a bare `bra.s -2` cannot show that.
    /// - **It contains multi-word instructions.** `move #imm,sr` is two words and
    ///   `addq.w #1,$FF0000` is three, so a claim about an instruction's address is
    ///   not satisfiable by a fixture where every instruction is one word.
    ///
    /// Encodings are [`spin`]'s and `tests/programs.rs`'s, both verified against
    /// `m68k::disasm` and quoted there.
    fn a_running_machine() -> Cps1 {
        let mut rom = vec![0u8; 0x4000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        // Autovector level 2 = vector 24 + 2 = 26, at 26 * 4 = 0x68.
        rom[0x68..0x6C].copy_from_slice(&[0x00, 0x00, 0x20, 0x00]);
        rom[0x1000..0x1008].copy_from_slice(&[0x46, 0xFC, 0x20, 0x00, 0x4E, 0x71, 0x60, 0xFC]);
        rom[0x2000..0x2008].copy_from_slice(&[0x52, 0x79, 0x00, 0xFF, 0x00, 0x00, 0x4E, 0x73]);
        let mut m = Cps1::new(&rom, BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        m
    }

    /// Runs `f` on a thread with an 8 MB stack.
    ///
    /// A `Cps1` is 525 KB, almost all of it [`Decoder`]'s 512 KB dispatch table, and
    /// `Decoder::new` builds that table *on the stack* before it lands in the struct
    /// — `m68k::decode`'s own note measures the floor at 1 MB and records that
    /// `Box::new` does not avoid it. So two live machines do not fit a test thread's
    /// 2 MB, and elsewhere in this crate the answer is "one machine, not two".
    ///
    /// A divergence test cannot take that answer: comparing two ways of running the
    /// *same* program is the entire claim, and it needs both machines alive at once.
    /// Hence an explicit stack rather than a contorted single-machine version. The
    /// overflow it avoids is a process abort rather than a test failure, so getting
    /// this wrong does not look like a bug in the thing under test.
    fn on_a_big_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn(f)
            .expect("spawn")
            .join()
            .expect("the divergence test panicked; its own message is above");
    }

    /// One frame by scanlines and one frame by `run_frame` reach the same machine.
    ///
    /// A baseline, not a tautology: it passes trivially as written, because
    /// `run_frame` *is* a loop over `run_scanline`. It exists to pin current
    /// behaviour before [`Cps1::step_instruction`] is extracted out from under both,
    /// so that a refactor which changes what a frame does fails here — and can only
    /// fail here for that reason.
    #[test]
    fn a_frame_of_scanlines_equals_a_frame() {
        on_a_big_stack(|| {
            let mut a = a_running_machine();
            let mut b = a_running_machine();
            for _ in 0..a.timing.lines_per_frame {
                a.run_scanline();
            }
            b.run_frame();
            assert_eq!(a.total_cycles, b.total_cycles, "cycles");
            assert_eq!(a.line, b.line, "beam");
            assert_eq!(a.carry, b.carry, "and the schedule's debt");
            assert_eq!(a.cpu, b.cpu, "the whole CPU, every field");
            assert_eq!(&a.board.ram[..], &b.board.ram[..], "RAM");
            assert_eq!(a.board.trace.frames, b.board.trace.frames, "frames");
            assert_eq!(a.board.trace.vblanks, b.board.trace.vblanks, "vblanks");
            assert_eq!(a.board.trace.acks, b.board.trace.acks, "acks");

            // The premises. Without these the test passes for two machines that both did
            // nothing, and every claim above is vacuous.
            assert_ne!(a.total_cycles, 0, "the machines ran");
            assert_eq!(a.board.trace.vblanks, 1, "a frame contains one vblank");
            assert_eq!(a.board.trace.acks, 1, "which was acknowledged");
            assert_eq!(a.board.ram[0], 1, "and the handler ran, once");
        });
    }

    /// N instructions equal one frame, for an N the machine reports itself.
    ///
    /// **The test the refactor exists to satisfy**, and a divergence rather than a
    /// comparison: the count comes from stepping until the frame wraps, then running
    /// the same program on a fresh machine with `run_frame`. A literal N would be a
    /// number to re-derive every time the fixture changed, and would be wrong in a
    /// way that looked like a refactor bug.
    #[test]
    fn instructions_add_up_to_a_frame() {
        on_a_big_stack(|| {
            let mut a = a_running_machine();
            // Sampling is off by default (cap 0), so the `pc_samples` comparison below
            // would otherwise be `[] == []` — a claim about where each line ended that
            // holds for a stepping path which ended lines anywhere at all.
            a.board.trace.pc_sample_cap = 512;
            let mut n = 0u64;
            // Step until the machine says a frame has passed. `line == 0` would be the
            // obvious condition and is wrong: the machine *starts* on line 0, so it
            // holds after the first step and the test would compare one instruction
            // against a frame. `trace.frames` is incremented on the wrap, by the same
            // code `run_frame` reaches, and is 0 until then.
            while a.board.trace.frames == 0 {
                a.step_instruction();
                n += 1;
                assert!(n < 1_000_000, "a frame cannot be a million instructions");
            }

            let mut b = a_running_machine();
            b.board.trace.pc_sample_cap = 512;
            b.run_frame();
            assert_eq!(a.total_cycles, b.total_cycles, "cycles");
            assert_eq!(a.line, b.line, "beam");
            assert_eq!(a.carry, b.carry, "the schedule's debt");
            assert_eq!(a.cpu, b.cpu, "the whole CPU, every field");
            assert_eq!(&a.board.ram[..], &b.board.ram[..], "RAM");
            assert_eq!(a.board.trace.frames, b.board.trace.frames, "frames");
            assert_eq!(a.board.trace.vblanks, b.board.trace.vblanks, "vblanks");
            assert_eq!(a.board.trace.acks, b.board.trace.acks, "acks");
            assert_eq!(
                a.board.trace.pc_samples, b.board.trace.pc_samples,
                "and the same per-scanline PC samples, which is where a stepping path \
                 that ended lines at the wrong instruction would show up"
            );

            // The premises.
            assert!(n > 100, "a frame is many instructions, got {n}");
            assert_eq!(
                a.board.trace.pc_samples.len(),
                262,
                "one sample per line of the frame, so the comparison above is not \
                 two empty vectors"
            );
            assert_eq!(a.board.trace.vblanks, 1, "a frame contains one vblank");
            assert_eq!(a.board.trace.acks, 1, "which was acknowledged");
            assert_eq!(a.board.ram[0], 1, "and the handler ran, once");
        });
    }

    /// A single step advances the machine by exactly one instruction's cycles.
    #[test]
    fn one_step_is_one_instruction() {
        let mut m = a_running_machine();
        let pc0 = m.cpu.pc;
        let c = m.step_instruction();
        assert!(c >= 4, "every 68000 instruction costs at least four cycles");
        assert_eq!(m.total_cycles, u64::from(c), "cycles are accrued, once");
        assert_ne!(m.cpu.pc, pc0, "and the PC moved");
        // 16 rather than any four-cycle instruction: the fixture's first instruction
        // is `move #$2000,sr`, which this crate's `SLOWEST` also pins at 16. A step
        // that ran two instructions would report 20 or more.
        assert_eq!(c, 16, "`move #imm,sr` is 16 cycles, and only one ran");
    }

    /// Stepping advances the beam, which is what makes a debugger's video update.
    ///
    /// A `step_instruction` that left the budget alone would single-step forever on
    /// scanline 0: the game would never draw, and the bug would present as a video
    /// bug rather than a scheduling one.
    #[test]
    fn stepping_advances_the_beam() {
        let mut m = a_running_machine();
        let mut steps = 0;
        while m.line == 0 {
            m.step_instruction();
            steps += 1;
            assert!(
                steps < 10_000,
                "the beam must move; the budget is not being charged"
            );
        }
        assert_eq!(m.line, 1, "one line at a time, not a jump");
        assert!(
            m.total_cycles >= 640,
            "and a line's worth of cycles was spent: {}",
            m.total_cycles
        );
    }

    /// Stepping across the vblank line asserts vblank exactly once.
    ///
    /// The per-line work is not per-instruction work. A `step_instruction` that ran
    /// the start-of-line block on every call would assert vblank tens of times on
    /// line 240 — and `assert_vblank` counts, so the trace would show it while the
    /// game merely took one interrupt and looked fine.
    #[test]
    fn stepping_asserts_vblank_once_per_frame_not_once_per_instruction() {
        let mut m = a_running_machine();
        while m.line != m.timing.vblank_line {
            m.step_instruction();
        }
        assert_eq!(m.board.trace.vblanks, 0, "the premise: not yet asserted");
        while m.line == m.timing.vblank_line {
            m.step_instruction();
        }
        assert_eq!(
            m.board.trace.vblanks, 1,
            "one vblank for the line, however many instructions it took"
        );
    }

    /// The machine can be read without being disturbed, mid-run.
    ///
    /// `Board`'s own tests cover the address map and the side effects. This is the
    /// claim the debugger actually rests on: peeking a *running* machine, over the
    /// vector table with an interrupt outstanding, changes nothing about what it
    /// does next. Asserted by continuing the run afterwards and comparing against a
    /// machine that was never peeked.
    #[test]
    fn peeking_a_running_machine_does_not_change_where_it_goes() {
        on_a_big_stack(|| {
            let mut a = a_running_machine();
            let mut b = a_running_machine();
            // Stop mid-frame, on the vblank line, so the interrupt is outstanding and
            // `note_possible_ack`'s address is live.
            while a.line != a.timing.vblank_line {
                a.run_scanline();
                b.run_scanline();
            }
            a.run_scanline();
            b.run_scanline();
            assert!(
                a.board.trace.vblanks > 0,
                "the premise: a vblank has been asserted"
            );

            // Everything a memory panel would read on arriving here: the vector table,
            // the stack, the code, and a gap.
            for addr in (0..0x400).step_by(2) {
                a.peek_word(addr);
            }
            for addr in (0x00FF_7F00..0x00FF_8000).step_by(2) {
                a.peek_word(addr);
            }
            for addr in (0x40_0000..0x40_0080).step_by(2) {
                a.peek_word(addr);
            }
            assert_eq!(a.peek_word(0xFF_0000), Some(a.board.ram[0]), "and it read");

            // Then run both on, a whole frame, and require identical machines.
            for _ in 0..a.timing.lines_per_frame {
                a.run_scanline();
                b.run_scanline();
            }
            assert_eq!(a.total_cycles, b.total_cycles, "cycles");
            assert_eq!(a.carry, b.carry, "the schedule's debt");
            assert_eq!(a.cpu, b.cpu, "the whole CPU");
            assert_eq!(&a.board.ram[..], &b.board.ram[..], "RAM");
            assert_eq!(a.board.trace.acks, b.board.trace.acks, "acks");
            assert_eq!(
                a.board.trace.unmapped_reads.total(),
                b.board.trace.unmapped_reads.total(),
                "and the debugger's reads are not in the trace"
            );
            assert!(a.board.ram[0] >= 1, "the premise: the handler ran");
        });
    }

    /// Reset takes SSP and PC from the vectors, as the hardware does.
    #[test]
    fn reset_loads_the_stack_pointer_and_program_counter_from_the_vectors() {
        let m = machine();
        assert_eq!(m.cpu.a[7], 0x00FF_8000, "SSP from vector 0");
        assert_eq!(
            m.cpu.pc, 0x0000_1004,
            "PC from vector 1, plus the two prefetched words"
        );
        assert_eq!(m.total_cycles, 0);
        assert_eq!(m.line, 0);
    }

    /// A scanline runs its whole budget and overshoots by less than one
    /// instruction.
    #[test]
    fn a_scanline_runs_its_budget_and_overshoots_by_at_most_one_instruction() {
        let mut m = machine();
        let ran = u64::from(m.run_scanline());
        assert!(ran >= 640, "a scanline must run its full budget, ran {ran}");
        assert!(
            ran < 640 + SLOWEST,
            "and must not overrun by more than one instruction: {ran}"
        );
    }

    /// A frame is 167,680 cycles plus at most one instruction's overshoot.
    ///
    /// 167,680 is `640 × 262`, written here as a literal rather than read from
    /// `Timing::cycles_per_frame` — a scheduler checked against the same field it
    /// schedules from proves nothing about either.
    #[test]
    fn a_frame_costs_167680_cycles_plus_at_most_one_instruction() {
        let mut m = machine();
        m.run_frame();
        assert!(
            (167_680..167_680 + SLOWEST).contains(&m.total_cycles),
            "got {}",
            m.total_cycles
        );
        assert_eq!(m.line, 0, "and the beam is back at the top");
    }

    /// The overshoot does not accumulate across frames.
    ///
    /// This is what the carry exists for, and the assertion is `+ SLOWEST` rather
    /// than `+ 10 × SLOWEST`: after ten frames the error must still be one
    /// instruction, not ten. Ten frames of `nop`/`bra.s` against a 640-cycle budget
    /// is 2,620 scanline boundaries, so a dropped carry lands roughly 2,620 × 5
    /// cycles high — about 8% of a whole frame, and unmissable.
    #[test]
    fn ten_frames_do_not_drift() {
        let mut m = machine();
        for _ in 0..10 {
            m.run_frame();
        }
        assert!(
            (1_676_800..1_676_800 + SLOWEST).contains(&m.total_cycles),
            "ten frames is 1,676,800 plus one instruction, not plus ten. got {}",
            m.total_cycles
        );
    }

    /// And the same property line by line, which is where a drift begins.
    ///
    /// A frame-level bound can be satisfied by a scheduler that runs some lines
    /// short and others long. This checks the invariant after *every* one of the
    /// first 262 lines: the running total is never below the budget owed and never
    /// more than one instruction above it.
    #[test]
    fn the_running_total_stays_within_one_instruction_of_the_budget_every_line() {
        let mut m = machine();
        for n in 1..=262u64 {
            m.run_scanline();
            let owed = 640 * n;
            assert!(
                (owed..owed + SLOWEST).contains(&m.total_cycles),
                "after line {n} the total is {} but must be in {owed}..{}",
                m.total_cycles,
                owed + SLOWEST
            );
        }
    }

    /// The carry is actually exercised by this program.
    ///
    /// If every line happened to end exactly on its budget, every test above would
    /// pass with the carry hard-wired to zero — which is precisely what `bra.s -2`
    /// would have done, since 640 / 10 = 64. This asserts the premise those tests
    /// rest on.
    ///
    /// The discriminating count is lines that spend **fewer than 640 cycles**. With
    /// a working carry a line's budget is `640 + carry` with `carry <= 0`, so a
    /// short line is only possible when a debt was carried in. With `carry` hard
    /// wired to zero every line's budget is exactly 640 and no line can ever come
    /// in short — so a non-zero count here is direct evidence the carry is live,
    /// and no arbitrary threshold is needed to say so.
    #[test]
    fn the_test_program_straddles_scanline_boundaries() {
        let mut m = machine();
        let mut short = 0;
        let mut exact = 0;
        for _ in 0..262 {
            match m.run_scanline() {
                n if n < 640 => short += 1,
                640 => exact += 1,
                _ => {}
            }
        }
        assert!(
            short > 0,
            "no line of the frame came in under 640 cycles, which is only possible \
             if no debt is ever carried — the drift tests above would then prove \
             nothing"
        );
        assert!(
            exact < 262,
            "every line landed exactly on its budget, so this program cannot \
             exercise the carry"
        );
    }

    /// `line` counts scanlines and wraps at the frame boundary.
    #[test]
    fn the_scanline_counter_advances_and_wraps_at_the_frame_boundary() {
        let mut m = machine();
        for expected in 1..=5u32 {
            m.run_scanline();
            assert_eq!(m.line, expected);
        }
        for _ in 5..262 {
            m.run_scanline();
        }
        assert_eq!(m.line, 0, "262 lines is one frame");
        m.run_scanline();
        assert_eq!(m.line, 1, "and the next line is line 1, not 263");
    }

    /// The program really is running on the board, not stepping in a vacuum.
    ///
    /// Every test above counts cycles, and a `run_scanline` that stepped a CPU
    /// with a broken bus would count them just as happily. This one watches the
    /// guest write to main RAM: `move.w #$1234,$FF0000` is `0x33FC 0x1234 0x00FF
    /// 0x0000` (verified against `m68k::disasm`, which renders it
    /// `move.w #$1234,$FF0000`), then `bra.s` back to itself.
    #[test]
    fn the_guest_program_reaches_the_board() {
        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x1000..0x100A].copy_from_slice(&[
            0x33, 0xFC, 0x12, 0x34, 0x00, 0xFF, 0x00, 0x00, // move.w #$1234,$FF0000
            0x60, 0xF6, // bra.s -10 -> 0x1000
        ]);
        let mut m = Cps1::new(&rom, BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        assert_eq!(m.board.ram[0], 0x0000, "before the first line");
        m.run_scanline();
        assert_eq!(m.board.ram[0], 0x1234, "the guest wrote main RAM");
    }

    /// A frame's length follows `Timing`, not a constant.
    ///
    /// With a two-line frame of 100 cycles each, a `run_frame` hard-wired to 262
    /// lines or 640 cycles gives a wildly different total. The bound is the same
    /// one-instruction rule, hand-computed: 2 × 100 = 200.
    #[test]
    fn a_different_timing_gives_a_different_frame() {
        let t = Timing {
            cpu_hz: 10_000_000,
            cycles_per_line: 100,
            lines_per_frame: 2,
            vblank_line: 1,
        };
        let mut m = Cps1::new(&spin(), BoardConfig::sf2(), t);
        m.reset();
        m.run_frame();
        assert!(
            (200..200 + SLOWEST).contains(&m.total_cycles),
            "got {}",
            m.total_cycles
        );
        assert_eq!(m.line, 0);
    }

    // ------------------------------------------------------------------ video

    /// The pen the video fixture's sprite tile is solid in.
    const SOLID_PEN: u8 = 0x0A;
    /// The colour scheme its record asks for.
    const SPRITE_COLOUR: u16 = 3;
    /// The pen that combination lands in the framebuffer: `colour * 16 + pen`.
    const SPRITE_PEN: u16 = SPRITE_COLOUR * 16 + SOLID_PEN as u16;

    /// Word index in gfxram of the object table the fixture uses.
    ///
    /// Not zero: at zero the table would sit on top of the tilemaps, and these
    /// tests could not tell a sprite record from a map entry.
    const OBJ_WORD: usize = 0x2000;
    /// The object-base register value that resolves there — 0x40 × 256 = 0x4000
    /// bytes, already aligned to the table's 0x800 boundary, which is word 0x2000.
    const OBJ_BASE_REG: u16 = 0x40;

    /// A 16×16 graphics tile every pixel of which is `pen`.
    ///
    /// Written from the plane *byte* structure and not from `tile_pen`'s within-byte
    /// arithmetic: a solid tile's plane bytes are all 0x00 or all 0xFF, a group's
    /// four bytes are pen bits 0-3 in memory order, and a 16×16 row is two such
    /// groups. [`the_sprite_fixture_tile_is_solid`] decodes it back through
    /// `video`'s own reader, so a wrong transcription here fails there rather than
    /// quietly making a render test assert the wrong pen.
    fn solid_sprite_tile(pen: u8) -> Vec<u8> {
        let byte_for = |bit: u8| if pen & (1 << bit) != 0 { 0xFFu8 } else { 0x00 };
        let group = [byte_for(0), byte_for(1), byte_for(2), byte_for(3)];
        let mut rom = vec![0u8; 128];
        for row in 0..16 {
            for half in [0usize, 4] {
                rom[row * 8 + half..][..4].copy_from_slice(&group);
            }
        }
        rom
    }

    /// The fixture's tile really is solid in [`SOLID_PEN`].
    #[test]
    fn the_sprite_fixture_tile_is_solid() {
        let rom = solid_sprite_tile(SOLID_PEN);
        assert_eq!(rom.len(), 128, "a 16x16 4bpp tile is 128 bytes");
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(
                    video::tiles::tile_pen(&rom, video::tiles::TileKind::Tile16x16, 0, x, y),
                    SOLID_PEN,
                    "({x}, {y})"
                );
            }
        }
    }

    /// A machine whose graphics hold that one tile, with the object base set.
    fn video_machine() -> Cps1 {
        let mut m = Cps1::with_gfx(
            &spin(),
            solid_sprite_tile(SOLID_PEN),
            BoardConfig::sf2(),
            Timing::cps1_10mhz(),
        );
        m.reset();
        m.board.cps_a[video::regs::OBJ_BASE] = OBJ_BASE_REG;
        assert_eq!(
            video::regs::cps_a_base(&m.board.cps_a, video::regs::OBJ_BASE, 0x800),
            OBJ_WORD,
            "the fixture's object table is where the tests write it"
        );
        // Every layer-control field left at zero puts the sprites at all four
        // depths and enables no tilemap, so the sprites are the only thing drawn.
        assert_eq!(m.board.cps_b[BoardConfig::sf2().video.layer_control], 0);
        m
    }

    /// Writes sprite record 0 at visible (`x`, `y`), with an end marker behind it.
    ///
    /// The register holds a **raster** position, so the visible offset is added
    /// here — `video`'s own tests pin that offset at (64, 16) against literals.
    fn write_obj(m: &mut Cps1, x: i32, y: i32) {
        let rec = [
            (x + video::VISIBLE_X) as u16,
            (y + video::VISIBLE_Y) as u16,
            0, // code 0, the fixture's solid tile
            SPRITE_COLOUR,
        ];
        for (i, w) in rec.into_iter().enumerate() {
            m.board.gfxram[OBJ_WORD + i] = w;
        }
        m.board.gfxram[OBJ_WORD + 7] = 0xFF00;
    }

    /// The pen at visible (`x`, `y`), or [`None`] where the background shows.
    fn px(m: &Cps1, x: usize, y: usize) -> Option<u16> {
        match m.video.fb.pens[y * video::WIDTH + x] {
            video::palette::BACKGROUND_PEN => None,
            p => Some(p),
        }
    }

    /// How many pixels of the frame are not the background.
    fn drawn(m: &Cps1) -> usize {
        m.video
            .fb
            .pens
            .iter()
            .filter(|&&p| p != video::palette::BACKGROUND_PEN)
            .count()
    }

    /// `Cps1::new` still takes three arguments, and its frame is pure background.
    ///
    /// Every other test in this crate proves the signature by compiling. This one
    /// exercises the empty-graphics path rather than merely linking it: with no
    /// graphics ROM every tile decodes as absent, so a rendered frame must be
    /// uniformly the background pen — which is also the premise the tests below
    /// rest on when they assert that *something* drew.
    #[test]
    fn cps1_new_still_takes_three_arguments() {
        let mut m = Cps1::new(&spin(), BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        m.run_frame();
        m.render();
        assert_eq!(
            m.video.fb.pens.len(),
            video::WIDTH * video::HEIGHT,
            "384 x 224"
        );
        assert_eq!(drawn(&m), 0, "no graphics, so nothing can be drawn");
        assert_eq!(px(&m, 0, 0), None);
    }

    /// The graphics region handed to `with_gfx` is the one the renderer draws from.
    #[test]
    fn with_gfx_carries_the_region_into_the_renderer() {
        let mut m = video_machine();
        write_obj(&mut m, 0, 0);
        m.run_frame();
        m.render();
        assert_eq!(px(&m, 0, 0), Some(SPRITE_PEN), "colour 3, pen 0x0A");
        assert_eq!(
            px(&m, 15, 15),
            Some(SPRITE_PEN),
            "and the tile's far corner"
        );
        assert_eq!(drawn(&m), 16 * 16, "one 16x16 tile and nothing else");
        assert_eq!(px(&m, 16, 0), None, "the pixel past its right edge");
    }

    /// The object table is latched once per frame, at vblank.
    ///
    /// Three assertions, and each kills a different way of getting this wrong:
    /// a frame's worth of scanlines latches (so the table reaches the renderer at
    /// all); a change made mid-frame does *not* take effect (so the latch is not
    /// run every scanline, and not at line 0); and the next vblank picks it up (so
    /// it is a one-frame delay rather than a one-shot read).
    ///
    /// The delay is asserted through the drawn frame rather than by reading the
    /// latch, which `video` keeps private: the frame is the artifact, and a test
    /// that read the latch would pass on a renderer that ignored it.
    #[test]
    fn objects_are_latched_once_per_frame_at_vblank() {
        let mut m = video_machine();
        write_obj(&mut m, 0, 0);
        m.run_frame();
        m.render();
        assert_eq!(px(&m, 0, 0), Some(SPRITE_PEN), "the first frame's table");

        // Move the sprite, then run ten scanlines — lines 0 to 9, none of them the
        // vblank line 240. The frame the renderer draws must still be the old one.
        write_obj(&mut m, 32, 48);
        for _ in 0..10 {
            m.run_scanline();
        }
        assert!(m.line < m.timing.vblank_line, "the premise: no vblank yet");
        m.render();
        assert_eq!(
            px(&m, 0, 0),
            Some(SPRITE_PEN),
            "a table written mid-frame must not appear until the next vblank"
        );
        assert_eq!(
            px(&m, 32, 48),
            None,
            "and the new position must not be drawn"
        );

        // A full frame from here crosses line 240 exactly once.
        for _ in 0..262 {
            m.run_scanline();
        }
        m.render();
        assert_eq!(
            px(&m, 32, 48),
            Some(SPRITE_PEN),
            "the next vblank latched it"
        );
        assert_eq!(px(&m, 0, 0), None, "and the old position is gone");
        assert_eq!(drawn(&m), 16 * 16, "still exactly one tile");
    }

    /// `reset` clears the schedule as well as the CPU.
    ///
    /// A carry surviving a reset would make the first scanline of a new run short
    /// by up to an instruction — a per-run difference in a system whose whole value
    /// is determinism, and exactly what makes a rollback-netplay resynchronise
    /// wrongly.
    ///
    /// # Why this compares two runs rather than bounding one line
    ///
    /// A `640..650` bound on the first line does **not** catch a leaked carry: the
    /// spend is quantised to whole instructions, so a budget of 636 and a budget of
    /// 640 both spend 644. Verified — the bounded version left the
    /// `self.carry = 0`-removed mutant alive.
    ///
    /// So this asserts the property that actually matters: two runs from reset are
    /// identical, line for line. The reset is taken after **eight** lines, where the
    /// carry is non-zero — the pattern has period three (see the literals below), so
    /// resetting after a multiple of three would find the carry already at zero and
    /// prove nothing.
    #[test]
    fn reset_restores_the_schedule_exactly() {
        let mut m = machine();
        let first: Vec<u32> = (0..8).map(|_| m.run_scanline()).collect();

        // Hand-computed from the instruction costs, not copied from a run.
        //
        // Line 1 pays the 16-cycle `move #$2700,sr` prologue and then runs the
        // 14-cycle `nop`+`bra.s` pair: 16 + 44 × 14 = 632, leaving 8 of the budget,
        // so a `nop` takes it to 636 and the `bra.s` to **646** — a 6-cycle debt.
        // Line 2 has 634 to spend and the next instruction is a `nop`: 45 × 14 = 630,
        // then a `nop` lands on **634** exactly, debt 0.
        // Line 3 has the full 640 and now begins mid-pair on a `bra.s`: 45 pairs is
        // 630, then a `bra.s` lands on **640** exactly, debt 0 again.
        assert_eq!(
            &first[..3],
            &[646, 634, 640],
            "the first three lines: a 16-cycle prologue then a 14-cycle loop against \
             a 640-cycle budget"
        );

        assert!(m.total_cycles > 0);
        assert_ne!(m.line, 0);
        m.reset();
        assert_eq!(m.total_cycles, 0);
        assert_eq!(m.line, 0);

        let second: Vec<u32> = (0..8).map(|_| m.run_scanline()).collect();
        assert_eq!(
            first, second,
            "a reset machine must run the same schedule as a fresh one; a leftover \
             carry shows up as a different first line"
        );
    }
}
