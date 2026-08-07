//! The machine: a CPU, a board, and the schedule that interleaves them.

use crate::board::Board;
use crate::config::BoardConfig;
use crate::timing::Timing;
use m68k::{decode::Decoder, M68k};

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
    /// How far the last instruction overran its scanline budget, as a value `<= 0`
    /// carried into the next line.
    ///
    /// The 68000 cannot be stopped mid-instruction — a `divs` costs 158 cycles and
    /// does not divide at a scanline boundary — so overshoot is inherent. Carrying
    /// it forward means the *only* error at any moment is the current line's
    /// overshoot, never a sum of them. Dropping it would make every scanline
    /// slightly long and the frame rate slightly slow: music drifting against
    /// animation over a match, with nothing ever looking broken enough to
    /// investigate.
    carry: i64,
    /// Built once. `Decoder::new` fills a 65,536-entry table, so constructing one
    /// per step would dominate the run time.
    dec: Decoder,
}

impl Cps1 {
    /// A machine with `prog` in ROM space. Call [`Cps1::reset`] before stepping.
    pub fn new(prog: &[u8], cfg: BoardConfig, timing: Timing) -> Self {
        Self {
            cpu: M68k::new(),
            board: Board::new(prog, cfg),
            timing,
            total_cycles: 0,
            line: 0,
            carry: 0,
            dec: Decoder::new(),
        }
    }

    /// Power-up: the CPU takes SSP and PC from vectors 0 and 1, and the schedule
    /// returns to the top of a frame with no carried debt.
    pub fn reset(&mut self) {
        self.cpu.reset(&mut self.board);
        self.total_cycles = 0;
        self.line = 0;
        self.carry = 0;
    }

    /// Runs one scanline's worth of CPU, returning the cycles actually consumed.
    ///
    /// Consumes at least `cycles_per_line + carry` cycles; the excess becomes the
    /// next line's carry, so the running total never drifts above
    /// `cycles_per_line × lines` by more than one instruction's cost.
    pub fn run_scanline(&mut self) -> u32 {
        // Vblank: IPL1 on the line the beam leaves the visible area
        // (`cps1.cpp:394-396`). CPS-1 wires the IPL pins individually —
        // `set_interrupt_mixer(false)`, `cps1.cpp:3913` — so IPL1 is level 2, not an
        // encoded priority.
        if self.line == self.timing.vblank_line {
            self.board.assert_vblank();
        }
        let mut budget = i64::from(self.timing.cycles_per_line) + self.carry;
        let mut spent = 0u32;
        while budget > 0 {
            // Re-drive the level from the board's own state before **every** step,
            // not once per scanline.
            //
            // `M68k::pending_irq` is a level and nothing in the core clears it, while
            // the acknowledge happens on the board — on the far side of the bus. Set
            // once per line, the level would still read 2 after the handler's `rte`
            // dropped the mask, so the handler would re-enter for the rest of the
            // line: the 640-cycle budget fits about seven passes of a 90-cycle
            // handler. Syncing per step is what makes "the board owns deassertion"
            // actually true, and it costs one field write per instruction.
            self.cpu
                .set_irq(if self.board.vblank_pending() { 2 } else { 0 });
            // A halted CPU still burns time — the core returns 4 rather than 0 —
            // so `budget` always decreases and this cannot spin forever.
            let c = self.cpu.step_with(&self.dec, &mut self.board);
            budget -= i64::from(c);
            spent += c;
        }
        self.carry = budget; // <= 0
        self.total_cycles += u64::from(spent);
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
        spent
    }

    /// Runs `lines_per_frame` scanlines.
    pub fn run_frame(&mut self) {
        for _ in 0..self.timing.lines_per_frame {
            self.run_scanline();
        }
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
