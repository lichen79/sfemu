//! SF1's machine: three CPUs, four clocks, and a scanline scheduler.
//!
//! # Why this is a sibling of [`crate::Cps1`] rather than a generalization
//!
//! The frontend's forty-odd signatures want *fields* — `m.cpu.d[i]`,
//! `m.total_cycles`, `m.board.trace.frames`, `m.video.palette()` — not a
//! machine-shaped interface. A trait wide enough to serve them is a trait with
//! forty methods and no abstraction, and every one of them costs a virtual call in
//! the debugger's inner loops.
//!
//! So this file copies [`crate::Cps1`]'s scheduler *shape* and none of its code:
//! the carry convention (positive is budget remaining, the overshoot carries
//! forward negative), the accumulators, the "budget lives in `step_instruction`"
//! placement, and the "68000's whole line, then the sound CPUs" order. Each of
//! those choices is restated where it applies, because a reader of this file has
//! not read that one.
//!
//! # Four clocks, and only one of them divides
//!
//! | Clock | Per line | Source |
//! |---|---|---|
//! | 68000 | 3,125/6 = 520.833 cycles | [`Timing::sf1_8mhz`] |
//! | both Z80s | 715,909/3,072 = 233.04 T | [`crate::timing::sf1_z80_t_per_line`] |
//! | ADPCM IRQ | 25/48 interrupts | [`crate::timing::sf1_adpcm_irq_per_line`] |
//! | MSM5205 | exactly 25 master clocks | [`MSM_TICKS_PER_LINE`] |
//!
//! ⚠️ The Z80 fraction shares CPS-1's **numerator** and not its denominator:
//! 715,909/3,072 at SF1's 15,360 Hz line rate against 715,909/3,125 at CPS-1's
//! 15,625 Hz — 233.04 T per line against 229.09. Two boards sharing a Z80 crystal
//! do not share a T-states-per-line count, and the shared numerator makes this the
//! easiest constant in the file to get wrong by copying.
//!
//! Every remainder is machine state. A restored machine that zeroed one would be a
//! fraction of a cycle out per line and permanently out of step, which is the
//! argument [`RationalAccumulator::with_remainder`]'s own doc makes.
//!
//! # The interrupts are three different shapes
//!
//! - **68000, vblank**: `set_vblank_int("screen", irq1_line_hold)` (`sf.cpp:755`)
//!   with no `set_interrupt_mixer` call, so the default mixer encodes level 1 as
//!   vector 25 = 0x64. ⚠️ CPS-1's is 0x68, at level 2, because it wires IPL
//!   individually; copying that gives a board whose interrupt is never
//!   acknowledged — a game that runs one frame and stops.
//! - **FM Z80, NMI**: `soundcmd_w` (`sf.cpp:118-122`) writes the one soundlatch and
//!   pulses this CPU's NMI. Edge-triggered, so the scheduler sets `nmi = true` once
//!   per command and the core's `ack_nmi` clears it.
//! - **FM Z80, IRQ**: the YM2151's line
//!   (`ymsnd.irq_handler().set_inputline(m_audiocpu, 0)`, `sf.cpp:781`), re-driven
//!   before every instruction because the chip holds it until the driver clears the
//!   status and `ack_irq` clears only the CPU's copy.
//! - **ADPCM Z80, IRQ**: `set_periodic_int(irq0_line_hold, from_hz(8000))`
//!   (`sf.cpp:763`) — MAME's own comment on that line is `// ?`, and the
//!   uncertainty is carried here rather than smoothed over. A level, held until
//!   acknowledged.
//!
//! ⚠️ The ADPCM Z80 gets **no NMI and no YM IRQ**. Wiring either would be a
//! plausible symmetry with no hardware behind it.
//!
//! # One deliberate group delay
//!
//! Samples are produced inside the FM Z80's run, and the 25 MSM clocks are spent
//! afterwards, so a line's audio uses the ADPCM levels as of the previous line —
//! a one-line (65 µs) group delay on the ADPCM channel only. Documented rather
//! than fixed: fixing it means interleaving both sound CPUs' T-states with the MSM
//! clock, three accumulators driving one loop, for a shift well under the chip's
//! own 15.6 µs capture delay.

use crate::sf1::adpcm2::Adpcm2Board;
use crate::sf1::board::Sf1Board;
use crate::sf1::mix::mix;
use crate::sf1::msm5205::{Msm5205, MASTER_HZ};
use crate::sf1::snapshot::{MsmState, Sf1State};
use crate::sf1::sound::FmBoard;
use crate::timing::{
    sf1_adpcm_irq_per_line, sf1_line_rate, sf1_z80_t_per_line, RationalAccumulator, Timing,
    YM_SAMPLE_CLOCKS,
};
use m68k::{decode::Decoder, M68k};
use video::sf1::Sf1Video;

/// MSM5205 master clocks per scanline: 384,000 / 15,360 = 25, exactly.
///
/// An integer, which is why this is not a fourth [`RationalAccumulator`] and why
/// [`crate::sf1::msm5205::Msm5205`]'s pending-capture countdown is a plain `u8`.
pub const MSM_TICKS_PER_LINE: u32 = MASTER_HZ / sf1_line_rate();

/// SF1's whole machine.
pub struct Sf1 {
    /// The 68000.
    pub cpu: M68k,
    /// Everything on the 68000's bus.
    ///
    /// Beside [`Sf1::cpu`] rather than inside it: `M68k::step_with` borrows both at
    /// once.
    pub board: Sf1Board,
    /// The video subsystem: five graphics regions and the frame.
    pub video: Sf1Video,
    /// The FM Z80 (MAME's `audiocpu`).
    pub fm_z80: z80::Z80,
    /// Everything on the FM Z80's bus, the YM2151 included.
    pub fm: FmBoard,
    /// The ADPCM Z80 (MAME's `audio2`).
    pub adpcm_z80: z80::Z80,
    /// Everything on the ADPCM Z80's bus, both MSM5205s included.
    pub adpcm: Adpcm2Board,
    /// The board's clocks and geometry.
    pub timing: Timing,
    /// Total 68000 cycles since the last [`Sf1::reset`].
    pub total_cycles: u64,
    /// The scanline the beam is on, `0..lines_per_frame`.
    pub line: u32,
    /// 68000 cycles per line: 3,125/6, which is not an integer.
    ///
    /// Its *remainder* is machine state — a copy restored without it runs a fraction
    /// of a cycle out per line and diverges permanently.
    line_cycles: RationalAccumulator,
    /// 68000 cycles granted and not yet spent, counting down.
    ///
    /// Positive is budget remaining; an instruction that overran the line boundary
    /// leaves it negative and the next line's grant absorbs the overshoot. It holds
    /// the *live* budget rather than only the overshoot because that is what makes
    /// [`Sf1::step_instruction`] possible: a budget in `run_scanline`'s stack frame
    /// cannot survive a return to a debugger.
    carry: i64,
    /// The FM Z80's T-states per line: 715,909/3,072.
    fm_carry: RationalAccumulator,
    /// FM T-states granted and not yet spent.
    fm_debt: i64,
    /// Total FM Z80 T-states since the last [`Sf1::reset`].
    fm_total: u64,
    /// The ADPCM Z80's T-states per line — the same fraction, its own phase.
    ///
    /// A second accumulator rather than one shared: the two CPUs spend their budgets
    /// independently, so one accumulator would grant a line's T-states once and
    /// whichever CPU drained it first would starve the other.
    adpcm_carry: RationalAccumulator,
    /// ADPCM T-states granted and not yet spent.
    adpcm_debt: i64,
    /// Total ADPCM Z80 T-states since the last [`Sf1::reset`].
    adpcm_total: u64,
    /// The ADPCM Z80's periodic interrupts per line: 25/48 (8 kHz).
    adpcm_irq: RationalAccumulator,
    /// Input clocks accrued toward the next YM2151 sample, `0..64`.
    ///
    /// Driven by FM T-states actually spent rather than by lines, so the sample rate
    /// stays locked to that CPU rather than drifting against it.
    sample_acc: u32,
    /// Samples produced and not yet drained, **interleaved stereo**.
    ///
    /// Output, not state: a save state carrying a frame of audio would grow every
    /// snapshot and make a divergence comparison depend on when it was taken.
    samples: Vec<i16>,
    /// How many times the scheduler raised each interrupt line.
    ///
    /// Instruments, not machine state: not in the save state and not cleared by
    /// [`Sf1::reset`]. They exist because a taken interrupt leaves no trace on the
    /// CPU once its handler returns, so "the 8 kHz periodic reaches `audio2` and only
    /// `audio2`" is otherwise not assertable at all.
    fm_irqs: u32,
    adpcm_irqs: u32,
    fm_nmis: u32,
    adpcm_nmis: u32,
    /// How many mixed frames saturated, from [`mix()`](fn@crate::sf1::mix)'s flag.
    ///
    /// The overlay's `CLP` column, which on CPS-1 shows the OKI chip's own clip
    /// count. SF1's MSM5205s have no output clamp of their own — Task 10's
    /// `Msm5205::output` scales a clamped internal signal, so it cannot clip — and the
    /// saturation happens in [`mix()`](fn@crate::sf1::mix) instead. Without this counter the
    /// column reads 0 forever and a distorted mix has no diagnostic at all. An
    /// instrument, like the four counters above.
    mix_clips: u32,
    /// Built once. `Decoder::new` fills a 65,536-entry table.
    ///
    /// # ⚠️ Boxed, and that is load-bearing
    ///
    /// A [`Decoder`] is 512 KB. Inline, it made `size_of::<Cps1>()` 529,360 bytes and
    /// eleven green tests in that file started aborting with
    /// `fatal runtime error: stack overflow` — a process abort rather than a test
    /// failure, so it names an arbitrary test and takes the binary down. An `Sf1`
    /// holds *more* than a `Cps1` does, so an inline decoder here fails harder in the
    /// same unhelpful way.
    dec: Box<Decoder>,
}

impl core::fmt::Debug for Sf1 {
    /// Hand-written: the three boards' ROMs and the five graphics regions together
    /// are megabytes, and a derived `Debug` would print all of them into a panic
    /// message.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Sf1")
            .field("line", &self.line)
            .field("total_cycles", &self.total_cycles)
            .field("carry", &self.carry)
            .field("fm_total", &self.fm_total)
            .field("adpcm_total", &self.adpcm_total)
            .field("samples", &self.samples.len())
            .finish_non_exhaustive()
    }
}

impl Sf1 {
    /// A machine with `prog` on the 68000, `video`'s graphics, and the two sound
    /// programs.
    ///
    /// Call [`Sf1::reset`] before stepping: the 68000 takes SSP and PC from vectors 0
    /// and 1, which it reads through the bus.
    ///
    /// Empty sound programs are not an error. The FM Z80 then reads
    /// [`crate::sf1::sound::UNMAPPED`] on every fetch, which is `RST 38h`, so it
    /// spins deterministically rather than executing `NOP`s through the address
    /// space; the ADPCM Z80 does the same. That is what most of this crate's tests
    /// hand it.
    #[must_use]
    pub fn new(prog: &[u8], video: Sf1Video, audiocpu: Vec<u8>, audio2: Vec<u8>) -> Self {
        let timing = Timing::sf1_8mhz();
        let (line_num, line_den) = timing.line_cycles;
        let (t_num, t_den) = sf1_z80_t_per_line();
        let (irq_num, irq_den) = sf1_adpcm_irq_per_line();
        Self {
            cpu: M68k::new(),
            board: Sf1Board::new(prog),
            video,
            fm_z80: z80::Z80::new(),
            fm: FmBoard::new(audiocpu),
            adpcm_z80: z80::Z80::new(),
            adpcm: Adpcm2Board::new(audio2),
            timing,
            total_cycles: 0,
            line: 0,
            line_cycles: RationalAccumulator::new(line_num, line_den),
            carry: 0,
            fm_carry: RationalAccumulator::new(t_num, t_den),
            fm_debt: 0,
            fm_total: 0,
            adpcm_carry: RationalAccumulator::new(t_num, t_den),
            adpcm_debt: 0,
            adpcm_total: 0,
            adpcm_irq: RationalAccumulator::new(irq_num, irq_den),
            sample_acc: 0,
            samples: Vec::new(),
            fm_irqs: 0,
            adpcm_irqs: 0,
            fm_nmis: 0,
            adpcm_nmis: 0,
            mix_clips: 0,
            dec: Box::new(Decoder::new()),
        }
    }

    /// Power-up: the CPU takes SSP and PC from vectors 0 and 1, and every clock
    /// returns to phase zero.
    ///
    /// All four accumulators are rebuilt, not just zeroed in place — an accumulator
    /// surviving a reset would make two runs from reset produce samples at different
    /// instants, which is a per-run divergence no single run can detect.
    ///
    /// The three CPUs and all three chips are reset; sound RAM, the `audio2` bank and
    /// both latches are **not**. That is MAME's split: `machine_reset`
    /// (`sf.cpp:744-748`) zeroes four video scalars and propagates `device_reset` to
    /// the devices, while RAM contents and the bank selection are untouched.
    pub fn reset(&mut self) {
        self.cpu.reset(&mut self.board);
        self.board.reset();
        self.total_cycles = 0;
        self.line = 0;
        self.carry = 0;
        let (line_num, line_den) = self.timing.line_cycles;
        self.line_cycles = RationalAccumulator::new(line_num, line_den);
        self.fm_z80.reset();
        self.adpcm_z80.reset();
        self.fm.reset_ym();
        self.adpcm.reset_chips();
        let (t_num, t_den) = sf1_z80_t_per_line();
        self.fm_carry = RationalAccumulator::new(t_num, t_den);
        self.fm_debt = 0;
        self.fm_total = 0;
        self.adpcm_carry = RationalAccumulator::new(t_num, t_den);
        self.adpcm_debt = 0;
        self.adpcm_total = 0;
        let (irq_num, irq_den) = sf1_adpcm_irq_per_line();
        self.adpcm_irq = RationalAccumulator::new(irq_num, irq_den);
        self.sample_acc = 0;
        self.samples.clear();
        // The four interrupt counters survive: they are instruments, and a driver that
        // resets mid-run wants to keep what it has already observed.
    }

    /// Total FM Z80 T-states since the last [`Sf1::reset`].
    #[must_use]
    pub const fn z80_cycles(&self) -> u64 {
        self.fm_total
    }

    /// Total ADPCM Z80 T-states since the last [`Sf1::reset`].
    #[must_use]
    pub const fn adpcm_z80_cycles(&self) -> u64 {
        self.adpcm_total
    }

    /// The 68000's live cycle budget. Save-state field.
    #[must_use]
    pub const fn carry(&self) -> i64 {
        self.carry
    }

    /// The 68000 line accumulator's carried fraction, in sixths of a cycle.
    #[must_use]
    pub const fn line_cycles_remainder(&self) -> u32 {
        self.line_cycles.remainder()
    }

    /// The FM Z80's carried fraction of a T-state.
    #[must_use]
    pub const fn fm_carry_remainder(&self) -> u32 {
        self.fm_carry.remainder()
    }

    /// The ADPCM Z80's carried fraction of a T-state.
    #[must_use]
    pub const fn adpcm_carry_remainder(&self) -> u32 {
        self.adpcm_carry.remainder()
    }

    /// The ADPCM interrupt accumulator's carried fraction, in forty-eighths.
    #[must_use]
    pub const fn adpcm_irq_remainder(&self) -> u32 {
        self.adpcm_irq.remainder()
    }

    /// Input clocks accrued toward the next YM2151 sample.
    #[must_use]
    pub const fn sample_acc(&self) -> u32 {
        self.sample_acc
    }

    /// The FM Z80's unspent T-state budget.
    #[must_use]
    pub const fn fm_debt(&self) -> i64 {
        self.fm_debt
    }

    /// The ADPCM Z80's unspent T-state budget.
    #[must_use]
    pub const fn adpcm_debt(&self) -> i64 {
        self.adpcm_debt
    }

    /// How many times the scheduler raised the FM Z80's IRQ line. An instrument.
    #[must_use]
    pub const fn fm_irqs_raised(&self) -> u32 {
        self.fm_irqs
    }

    /// How many times the scheduler raised the ADPCM Z80's IRQ line. An instrument.
    #[must_use]
    pub const fn adpcm_irqs_raised(&self) -> u32 {
        self.adpcm_irqs
    }

    /// How many NMI pulses the FM Z80 received. An instrument.
    #[must_use]
    pub const fn fm_nmis_raised(&self) -> u32 {
        self.fm_nmis
    }

    /// How many NMI pulses the ADPCM Z80 received — always zero, and asserted.
    ///
    /// Nothing on the board is wired to it. The accessor exists so that fact is
    /// testable rather than merely true.
    #[must_use]
    pub const fn adpcm_nmis_raised(&self) -> u32 {
        self.adpcm_nmis
    }

    /// How many mixed frames saturated. The overlay's `CLP` column. An instrument.
    #[must_use]
    pub const fn mix_clips(&self) -> u32 {
        self.mix_clips
    }

    /// Sets every instrument counter to its maximum, for a frontend test.
    ///
    /// ⚠️ **Public and not `#[cfg(test)]` on purpose.** `frontend` is a different
    /// crate, so a `#[cfg(test)]` helper here would be invisible to it — the same wall
    /// that puts `sf1::test_video()` out of reach. The alternative is for the panel's
    /// test to set fifteen private fields, which means making them public, which is a
    /// worse trade: this is one door with a name that says what it is for.
    ///
    /// The sound panel formats every counter to a fixed width, so no row's length
    /// depends on its value — but that is a property, and a property needs a test.
    /// `frontend`'s `no_sf1_row_overflows_its_box_with_every_counter_saturated`
    /// renders the panel with every counter at its maximum, and this is how it gets
    /// there. The draft that used `{:06}` — a minimum width, not a maximum — had six
    /// rows that fit at zero and overflowed here.
    ///
    /// ⚠️ `1 << 40` for the cycle counters, not `u64::MAX`: the panel prints them
    /// `{:013}`, and `u64::MAX` is 20 digits, so saturating to it would size the box
    /// for a case the hardware cannot reach. `1 << 40` T-states is 13 digits and 85
    /// hours of emulated Z80 time at 3.579545 MHz.
    ///
    /// ⚠️ Do not call this outside a test. The name is what says so at the call site.
    pub fn saturate_counters_for_test(&mut self) {
        // ⚠️ The **field** names, which are not the accessor names: `fm_irqs` behind
        // `fm_irqs_raised()`, `fm_total` behind `z80_cycles()`.
        self.fm_irqs = u32::MAX;
        self.adpcm_irqs = u32::MAX;
        self.fm_nmis = u32::MAX;
        self.adpcm_nmis = u32::MAX;
        self.mix_clips = u32::MAX;
        self.fm_total = 1 << 40;
        self.adpcm_total = 1 << 40;
        self.fm.saturate_trace_for_test();
        self.adpcm.saturate_trace_for_test();
    }

    /// Put the whole schedule back, remainders included.
    ///
    /// Twelve arguments because the schedule has twelve independent numbers, and a
    /// struct for them would be a second `Sf1State` (Task 19's) with a different
    /// field order. Every remainder goes through
    /// [`RationalAccumulator::with_remainder`], which is what that method is public
    /// for: a codec outside this crate has to be able to restore a phase.
    #[allow(clippy::too_many_arguments)]
    pub fn restore_schedule(
        &mut self,
        total_cycles: u64,
        line: u32,
        carry: i64,
        line_remainder: u32,
        fm_total: u64,
        fm_debt: i64,
        fm_remainder: u32,
        adpcm_total: u64,
        adpcm_debt: i64,
        adpcm_remainder: u32,
        adpcm_irq_remainder: u32,
        sample_acc: u32,
    ) {
        self.total_cycles = total_cycles;
        self.line = line % self.timing.lines_per_frame;
        self.carry = carry;
        let (line_num, line_den) = self.timing.line_cycles;
        self.line_cycles = RationalAccumulator::with_remainder(line_num, line_den, line_remainder);
        let (t_num, t_den) = sf1_z80_t_per_line();
        self.fm_total = fm_total;
        self.fm_debt = fm_debt;
        self.fm_carry = RationalAccumulator::with_remainder(t_num, t_den, fm_remainder);
        self.adpcm_total = adpcm_total;
        self.adpcm_debt = adpcm_debt;
        self.adpcm_carry = RationalAccumulator::with_remainder(t_num, t_den, adpcm_remainder);
        let (irq_num, irq_den) = sf1_adpcm_irq_per_line();
        self.adpcm_irq = RationalAccumulator::with_remainder(irq_num, irq_den, adpcm_irq_remainder);
        self.sample_acc = sample_acc;
    }

    /// The samples produced since the last [`Sf1::drain_samples`], interleaved L,R.
    ///
    /// One frame per YM2151 tick, at 55,930 Hz, so `samples().len()` is twice the tick
    /// count. Genuinely stereo, unlike CPS-1's: see [`mix()`](fn@crate::sf1::mix).
    #[must_use]
    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    /// Takes the produced samples, interleaved, which the host does once it has queued
    /// them.
    pub fn drain_samples(&mut self) -> Vec<i16> {
        core::mem::take(&mut self.samples)
    }

    /// The word at `addr` as a debugger sees it, or `None` if nothing decodes it.
    ///
    /// No side effects, which is why a debugger does not read through the CPU's own
    /// path. `&self` is what enforces it.
    #[must_use]
    pub fn peek_word(&self, addr: u32) -> Option<u16> {
        self.board.peek_word(addr)
    }

    /// Renders the current board state into [`Sf1::video`]'s framebuffer.
    pub fn render(&mut self) {
        self.video.render(
            &self.board.videoram[..],
            &self.board.objectram[..],
            &self.board.palette[..],
            self.board.active,
            self.board.bgscroll,
            self.board.fgscroll,
        );
    }

    /// Runs exactly one 68000 instruction, returning the cycles it consumed.
    ///
    /// The debugger's stepping primitive, and [`Sf1::run_scanline`] is a loop over
    /// it — **one code path deliberately.** A separate stepping path is a debugger
    /// that single-steps a machine subtly unlike the one that runs, and the IRQ sync
    /// below is the specific thing that gets left out of the second copy: the symptom
    /// is a machine that takes no interrupts under the debugger, which is what a
    /// debugger is most often opened to investigate.
    ///
    /// One instruction can overrun the line's budget — a `divs` costs 158 cycles and
    /// does not divide at a line boundary — so this may end a line, in which case it
    /// does everything the end of a line does.
    pub fn step_instruction(&mut self) -> u32 {
        // The start-of-line work, here rather than in `run_scanline` so that a caller
        // which only ever steps still gets it, guarded on the budget being spent so it
        // happens once per line.
        if self.carry <= 0 {
            self.carry += i64::from(self.line_cycles.advance());
            // Vblank on VBSTART, the first line past the visible area
            // (`set_vblank_int`, `sf.cpp:755`). SF1's vblank *period* is zero, so this
            // is a single edge rather than a level held for a span — which is what the
            // board's edge-plus-latch model already is.
            if self.line == self.timing.vblank_line {
                self.board.assert_vblank();
            }
        }
        // Re-drive the level from the board's own state before **every** step. The
        // acknowledge happens on the board, on the far side of the bus, so a level set
        // once per line would still read 1 after the handler's `rte` dropped the mask
        // and the handler would re-enter for the rest of the line.
        //
        // ⚠️ Level **1**, not CPS-1's 2: `irq1_line_hold` with the default interrupt
        // mixer, autovector 24 + 1 = 25 = 0x64.
        self.cpu
            .set_irq(if self.board.vblank_pending() { 1 } else { 0 });
        let c = self.cpu.step_with(&self.dec, &mut self.board);
        self.carry -= i64::from(c);
        self.total_cycles += u64::from(c);
        // `soundcmd_w` writes the one soundlatch and pulses the FM CPU's NMI. Checked
        // after every instruction rather than per line, so a command written mid-line
        // reaches the sound CPUs in the same line.
        if let Some(cmd) = self.board.take_sound_command() {
            // One hardware latch, two readers: each board holds a copy and this is the
            // only place either is written. See each board's
            // `no_bus_write_can_change_the_latch`.
            self.fm.set_latch(cmd);
            self.adpcm.set_latch(cmd);
            self.fm_z80.nmi = true;
            self.fm_nmis = self.fm_nmis.saturating_add(1);
        }
        if self.carry <= 0 {
            // **The 68000's whole line first, then the sound CPUs** — MAME's interleave
            // order at scanline granularity. Granting and spending here rather than at
            // the line's start is what makes that true for the whole line: granted at
            // the start and drained after the first 68000 instruction, the sound CPUs
            // would run their entire line before the 68000 had written anything.
            //
            // Neither loop can spin: every step spends at least four T-states against a
            // finite budget, a halted CPU included.
            self.fm_debt += i64::from(self.fm_carry.advance());
            while self.fm_debt > 0 {
                self.step_fm();
            }
            // The 8 kHz periodic, whose count for this line is 25/48 — so most lines
            // raise none and some raise one. Raised before the CPU runs, so an
            // interrupt due this line is taken this line.
            //
            // ⚠️ Set to `true` only. `z80::Z80::irq` is a *level* and the core's
            // `ack_irq` clears it, so writing `false` here would drop an interrupt the
            // guest had not reached yet — which sounds like ADPCM playing at a fraction
            // of its rate.
            for _ in 0..self.adpcm_irq.advance() {
                self.adpcm_z80.irq = true;
                self.adpcm_irqs = self.adpcm_irqs.saturating_add(1);
            }
            self.adpcm_debt += i64::from(self.adpcm_carry.advance());
            while self.adpcm_debt > 0 {
                self.step_adpcm();
            }
            // The MSM5205s' 25 master clocks, spent together at the line's end. Not
            // interleaved with the Z80s' T-states: a capture is armed by a port write
            // and lands six clocks later, so the placement only decides whether a
            // nibble written late in a line decodes in that line or the next — 65 µs
            // against the chip's own 15.6 µs delay. See the module doc's note on the
            // resulting one-line group delay.
            for _ in 0..MSM_TICKS_PER_LINE {
                self.adpcm.tick();
            }
            self.board.trace.sample_pc(self.cpu.pc);
            self.line = (self.line + 1) % self.timing.lines_per_frame;
            // Counted on the wrap, so a caller driving scanlines by hand counts the
            // same frames a `run_frame` caller does.
            if self.line == 0 {
                self.board.trace.frames += 1;
            }
        }
        c
    }

    /// One unit of FM-board work: the interrupt, one Z80 instruction, and whatever
    /// samples its T-states paid for.
    ///
    /// **One copy of this body, deliberately** — [`Sf1::step_instruction`]'s reason. A
    /// second copy for the debugger is a debugger that steps a machine subtly unlike
    /// the one that runs.
    ///
    /// The order is not arbitrary: the IRQ level is re-driven before the instruction
    /// because the YM holds its line until the driver clears the status; `service` runs
    /// before `step` and its zero return means "nothing accepted", so a refused
    /// request costs nothing; and the samples come from T-states actually spent, so
    /// the sample rate is locked to this CPU rather than drifting against it.
    fn step_fm(&mut self) {
        self.fm_z80.irq = self.fm.ym_ref().irq();
        if self.fm_z80.irq {
            self.fm_irqs = self.fm_irqs.saturating_add(1);
        }
        let mut t = self.fm_z80.service(&mut self.fm);
        if t == 0 {
            t = self.fm_z80.step(&mut self.fm);
        }
        self.fm_debt -= i64::from(t);
        self.fm_total += u64::from(t);
        self.sample_acc += t;
        while self.sample_acc >= YM_SAMPLE_CLOCKS {
            self.sample_acc -= YM_SAMPLE_CLOCKS;
            let mut one = [(0i16, 0i16)];
            self.fm.ym().generate(&mut one);
            let (msm0, msm1) = self.adpcm.output();
            // The board's stereo mix, per side, with its own saturation — SF1's
            // weights reach 60% past full scale, unlike CPS-1's. The clip flag feeds
            // the overlay's CLP column.
            let ((l, r), clipped) = mix(one[0].0, one[0].1, msm0, msm1);
            if clipped {
                self.mix_clips = self.mix_clips.saturating_add(1);
            }
            // Interleaved, per `crate::resample::CHANNELS`.
            self.samples.push(l);
            self.samples.push(r);
        }
    }

    /// One unit of ADPCM-board work: one Z80 instruction, and nothing else.
    ///
    /// No sample production here — the YM's clock is the FM CPU's. No NMI and no YM
    /// IRQ either: nothing on the board is wired to this CPU but the 8 kHz periodic,
    /// which [`Sf1::step_instruction`] raises.
    fn step_adpcm(&mut self) {
        let mut t = self.adpcm_z80.service(&mut self.adpcm);
        if t == 0 {
            t = self.adpcm_z80.step(&mut self.adpcm);
        }
        self.adpcm_debt -= i64::from(t);
        self.adpcm_total += u64::from(t);
    }

    /// Runs exactly one FM Z80 instruction, for the debugger.
    ///
    /// A public door onto `step_fm` rather than a second copy of it. ⚠️ A plain code
    /// span, not a rustdoc link: `step_fm` is private and this crate sets
    /// `deny(rustdoc::private_intra_doc_links)` (`lib.rs:27`), so a link fails
    /// `cargo doc` — which is in the commit gate. The
    /// T-states are charged against the line's budget in there, not by the caller: a
    /// stepping path that spent them beside the budget would break the
    /// granted-equals-spent-plus-debt identity, and the symptom is a sound CPU that
    /// runs fast in proportion to how much the user stepped it.
    pub fn step_fm_instruction(&mut self) -> u32 {
        let before = self.fm_total;
        self.step_fm();
        u32::try_from(self.fm_total - before).unwrap_or(u32::MAX)
    }

    /// Runs exactly one ADPCM Z80 instruction, for the debugger.
    pub fn step_adpcm_instruction(&mut self) -> u32 {
        let before = self.adpcm_total;
        self.step_adpcm();
        u32::try_from(self.adpcm_total - before).unwrap_or(u32::MAX)
    }

    /// Runs one scanline's worth of all three CPUs, returning the 68000 cycles spent.
    pub fn run_scanline(&mut self) -> u32 {
        let start = self.total_cycles;
        let line = self.line;
        // `step_instruction` advances `line` when the budget runs out, so this ends
        // when — and only when — the line it was called on has finished. It cannot
        // spin: every step spends at least four cycles against a finite budget.
        while self.line == line {
            self.step_instruction();
        }
        // A line is hundreds of cycles, so this cannot truncate; `try_from` rather than
        // `as` so that if it somehow could, it saturates visibly.
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
    /// Not the ROMs, the graphics regions, the decoded tiles, the framebuffer, the
    /// layer mask, the traces, or the sample queue: see [`Sf1State`] for why each
    /// is absent.
    #[must_use]
    pub fn snapshot(&self) -> Sf1State {
        Sf1State {
            cpu: self.cpu.clone(),
            // `boxed_copy` and not `.clone()`: see its documentation.
            ram: crate::snapshot::boxed_copy(&self.board.ram),
            objectram: crate::snapshot::boxed_copy(&self.board.objectram),
            videoram: crate::snapshot::boxed_copy(&self.board.videoram),
            palette: crate::snapshot::boxed_copy(&self.board.palette),
            active: self.board.active,
            bgscroll: self.board.bgscroll,
            fgscroll: self.board.fgscroll,
            coin_ctrl: self.board.coin_ctrl,
            vblank_pending: self.board.vblank_pending(),
            // ⚠️ `sound_command`, not `take_sound_command`: a save must not
            // consume the command it is saving. Task 19 added this door for it.
            sound_command: self.board.sound_command(),
            inputs: self.board.inputs,
            total_cycles: self.total_cycles,
            line: self.line,
            carry: self.carry(),
            line_remainder: self.line_cycles_remainder(),
            fm_z80: self.fm_z80.clone(),
            fm_ram: Box::new(*self.fm.ram()),
            ym: self.fm.ym_ref().clone(),
            ym_addr: self.fm.ym_addr(),
            fm_latch: self.fm.latch(),
            fm_total: self.z80_cycles(),
            fm_debt: self.fm_debt(),
            fm_remainder: self.fm_carry_remainder(),
            adpcm_z80: self.adpcm_z80.clone(),
            adpcm_bank: self.adpcm.bank(),
            adpcm_latch: self.adpcm.latch(),
            msm: core::array::from_fn(|i| {
                let c = self.adpcm.msm(i);
                MsmState {
                    signal: c.signal(),
                    step: c.step(),
                    data: c.data(),
                    vck: c.vck(),
                    // ⚠️ The accessor is `in_reset`, not `reset` — `reset` is the
                    // device reset and takes `&mut self`.
                    reset: c.in_reset(),
                    pending: c.pending(),
                }
            }),
            adpcm_total: self.adpcm_z80_cycles(),
            adpcm_debt: self.adpcm_debt(),
            adpcm_remainder: self.adpcm_carry_remainder(),
            adpcm_irq_remainder: self.adpcm_irq_remainder(),
            sample_acc: self.sample_acc(),
        }
    }

    /// Puts a snapshot back.
    ///
    /// The four boxed arrays are copied into the existing boxes rather than
    /// replacing them, so a load does not allocate 42 KB per press of the load
    /// key.
    ///
    /// Leaves the ROMs, the graphics regions, the decoded tiles, the layer mask,
    /// the three traces and the sample queue alone. The traces especially: they
    /// record the session rather than the machine, and rewinding them on every
    /// load would make a divergence test compare a run's counters against a copy
    /// of themselves.
    pub fn restore(&mut self, s: &Sf1State) {
        self.cpu = s.cpu.clone();
        self.board.ram.copy_from_slice(&s.ram[..]);
        self.board.objectram.copy_from_slice(&s.objectram[..]);
        self.board.videoram.copy_from_slice(&s.videoram[..]);
        self.board.palette.copy_from_slice(&s.palette[..]);
        self.board.active = s.active;
        self.board.bgscroll = s.bgscroll;
        self.board.fgscroll = s.fgscroll;
        self.board.coin_ctrl = s.coin_ctrl;
        self.board.set_vblank_pending(s.vblank_pending);
        self.board.set_sound_command(s.sound_command);
        self.board.inputs = s.inputs;
        self.fm_z80 = s.fm_z80.clone();
        self.fm.restore(*s.fm_ram, s.ym_addr, s.fm_latch);
        // The chip is rebuilt from the state's own YM2151, not from `Ym2151::new`.
        // `FmBoard::restore` deliberately does not take it — the chip has its own
        // codec — so this goes through the board's `&mut` door.
        *self.fm.ym() = s.ym.clone();
        self.adpcm_z80 = s.adpcm_z80.clone();
        self.adpcm.restore(s.adpcm_bank, s.adpcm_latch);
        for (i, m) in s.msm.iter().enumerate() {
            // `Msm5205::restore` is an associated function returning a chip, and it
            // clamps every field a file could corrupt — the step index especially,
            // which panics in `oki`'s `diff` if out of range.
            *self.adpcm.msm_mut(i) =
                Msm5205::restore(m.signal, m.step, m.data, m.vck, m.reset, m.pending);
        }
        // Every schedule number in one call, remainders included. Twelve
        // arguments in the order `restore_schedule` declares them — ⚠️ the FM
        // triple comes before the ADPCM one, and each triple is
        // (total, debt, remainder).
        self.restore_schedule(
            s.total_cycles,
            s.line,
            s.carry,
            s.line_remainder,
            s.fm_total,
            s.fm_debt,
            s.fm_remainder,
            s.adpcm_total,
            s.adpcm_debt,
            s.adpcm_remainder,
            s.adpcm_irq_remainder,
            s.sample_acc,
        );
        // `samples` is deliberately untouched: it is output the host drains, not
        // state, so a load must not retract audio already queued for playback.
        // `video.enable` likewise — a debugger's subtraction, not machine state.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 68000 program that spins: `bra.s *` at 0x0400, with SSP and PC vectors.
    ///
    /// Big-endian, because the 68000 is. Vector 0 is SSP and vector 1 is PC, which
    /// `M68k::reset` reads through the bus.
    fn spin_program() -> Vec<u8> {
        let mut rom = vec![0u8; 0x0800];
        // SSP = 0x00FF_F000, PC = 0x0000_0400.
        rom[0..4].copy_from_slice(&0x00FF_F000u32.to_be_bytes());
        rom[4..8].copy_from_slice(&0x0000_0400u32.to_be_bytes());
        // 0x60FE = bra.s -2, an infinite loop that costs 10 cycles a pass.
        rom[0x400..0x402].copy_from_slice(&0x60FEu16.to_be_bytes());
        rom
    }

    /// A Z80 program that spins: `jr -2` at 0, which is 12 T-states a pass.
    fn z80_spin() -> Vec<u8> {
        vec![0x18, 0xFE]
    }

    /// A machine with spinning programs on all three CPUs and no graphics.
    fn machine() -> Sf1 {
        let mut m = Sf1::new(
            &spin_program(),
            crate::sf1::test_video(),
            z80_spin(),
            z80_spin(),
        );
        m.reset();
        m
    }

    /// The MSM5205's master clock divides SF1's line rate exactly.
    ///
    /// 384,000 / 15,360 = 25, no remainder — which is why this is a plain integer
    /// and not a fourth [`RationalAccumulator`]. Task 10's `Msm5205::pending` being a
    /// `u8` rests on the same fact.
    #[test]
    fn the_msm_clock_divides_the_line_rate_exactly() {
        assert_eq!(MSM_TICKS_PER_LINE, 25);
        assert_eq!(
            crate::sf1::msm5205::MASTER_HZ % crate::timing::sf1_line_rate(),
            0,
            "a remainder here would need an accumulator"
        );
        assert_eq!(
            crate::sf1::msm5205::MASTER_HZ / crate::timing::sf1_line_rate(),
            MSM_TICKS_PER_LINE
        );
    }

    /// The three fractions are SF1's, and the Z80's is not CPS-1's.
    ///
    /// ⚠️ The numerator is shared and the denominator is not: 715,909/3,072 here
    /// against 715,909/3,125 on CPS-1, which is 233.04 T per line against 229.09.
    /// Copying CPS-1's constant compiles, runs, and makes the music 1.7% fast.
    #[test]
    fn the_three_fractions_are_sf1s() {
        let m = machine();
        assert_eq!(m.timing.line_cycles, (3125, 6), "68000: 520.83 per line");
        assert_eq!(
            crate::timing::sf1_z80_t_per_line(),
            (715_909, 3_072),
            "both Z80s"
        );
        assert_ne!(
            crate::timing::sf1_z80_t_per_line(),
            (crate::timing::Z80_T_NUM, crate::timing::Z80_T_DEN),
            "CPS-1's denominator is 3,125 and this is not CPS-1"
        );
        assert_eq!(crate::timing::sf1_adpcm_irq_per_line(), (25, 48), "8 kHz");
    }

    /// A scanline advances all three CPUs by their share of the line.
    ///
    /// Granted equals spent plus debt, for each sound CPU separately — the identity
    /// that catches a budget spent against the wrong counter, which is otherwise
    /// invisible until the two CPUs drift apart over minutes.
    #[test]
    fn a_scanline_advances_all_three_cpus() {
        let mut m = machine();
        let c0 = m.total_cycles;
        let fm0 = m.z80_cycles();
        let ad0 = m.adpcm_z80_cycles();
        m.run_scanline();
        let spent = m.total_cycles - c0;
        assert!(
            (500..=540).contains(&spent),
            "the 68000 ran {spent} cycles, not about 521"
        );
        for (name, spent, debt) in [
            ("fm", m.z80_cycles() - fm0, m.fm_debt()),
            ("adpcm", m.adpcm_z80_cycles() - ad0, m.adpcm_debt()),
        ] {
            // 233.04 T granted; spent plus the debt still owed must equal it.
            // `debt` is *added*, not subtracted: it is what remains of the grant, so
            // an instruction that overran leaves it negative and the sum comes back
            // down to the grant. `Cps1`'s
            // `a_scanline_advances_the_z80_by_its_share_of_the_line` states the same
            // identity the same way. Negating it here instead reads 247 for a 233-T
            // grant spent 240 — the overshoot counted twice.
            let granted = i64::try_from(spent).unwrap() + debt;
            assert!(
                (230..=236).contains(&granted),
                "{name}: granted {granted} T, not about 233"
            );
            assert!(spent > 0, "{name}: the CPU did not run");
        }
    }

    /// Both Z80s run, and they are not the same Z80.
    ///
    /// The failure this catches is a scheduler that steps `fm_z80` twice: the totals
    /// then match perfectly and the ADPCM board never sees an instruction, which
    /// sounds like a game with music and no voices.
    #[test]
    fn the_two_sound_cpus_are_independent() {
        // Different programs whose loop periods do not both divide the overshoot.
        //
        // ⚠️ The grant is the same for both CPUs, so the totals can only differ by the
        // overshoot on the last instruction of each line — a handful of T-states, not
        // a ratio. `jr -2` (12 T) against `nop; jr -3` (16 T) is *not* enough: both
        // divide the 240 T that 64 lines of a 233.04-T grant actually costs, and the
        // two CPUs tie at 14,916 exactly. So the second program's loop is 26 T
        // (`ld a,n; ld b,n; jr -6`), which lands at 14,924 against 14,916.
        //
        // The `assert_ne` is therefore the weaker half of this test. The fetch
        // counters below are the half that really catches "one CPU ran twice": a
        // scheduler stepping `fm_z80` in place of `adpcm_z80` leaves the ADPCM board
        // with zero fetches, whatever the totals say.
        let mut m = Sf1::new(
            &spin_program(),
            crate::sf1::test_video(),
            vec![0x18, 0xFE], // jr -2: 12 T
            // ld a,5; ld b,7; jr -6: 7 + 7 + 12 = 26 T. Register-only, so nothing
            // reaches the board's discard-everything write path.
            vec![0x3E, 0x05, 0x06, 0x07, 0x18, 0xFA],
        );
        m.reset();
        for _ in 0..64 {
            m.run_scanline();
        }
        assert!(m.z80_cycles() > 0 && m.adpcm_z80_cycles() > 0, "both ran");
        assert_ne!(
            m.z80_cycles(),
            m.adpcm_z80_cycles(),
            "identical totals from different programs means one CPU ran twice"
        );
        assert!(m.fm.trace().audiocpu_fetches > 0, "the FM board answered");
        assert!(
            m.adpcm.trace().rom_fetches > 0,
            "and so did the ADPCM board"
        );
    }

    /// The 8 kHz periodic interrupt reaches the ADPCM Z80 and only that one.
    ///
    /// 25/48 per line — so about one interrupt every two lines, and 8,000 a second.
    /// Asserted over a whole frame against the rate rather than counting one line,
    /// because a per-line count cannot tell 25/48 from 1/2.
    #[test]
    fn the_adpcm_cpu_takes_eight_thousand_interrupts_a_second() {
        let mut m = machine();
        // `im 1` and a handler that returns: the count is what this measures, so the
        // program has to be able to accept them.
        m.adpcm = Adpcm2Board::new(vec![
            0xED, 0x56, // im 1
            0xFB, // ei
            0x18, 0xFE, // jr -2
        ]);
        m.reset();
        for _ in 0..crate::timing::SF1_VTOTAL {
            m.run_scanline();
        }
        let taken = m.adpcm_irqs_raised();
        // 8,000 / 60 = 133.3 per frame.
        assert!(
            (130..=137).contains(&taken),
            "{taken} interrupts in a frame, not about 133"
        );
        assert_eq!(
            m.fm_irqs_raised(),
            0,
            "the periodic interrupt is audio2's alone (sf.cpp:763)"
        );
    }

    /// The FM Z80's IRQ is the YM2151's line, as on CPS-1.
    ///
    /// `ymsnd.irq_handler().set_inputline(m_audiocpu, 0)` (`sf.cpp:781`). Driven
    /// before every instruction rather than once per line, because the chip holds the
    /// line until the driver clears the status and `ack_irq` clears only the CPU's
    /// own copy.
    #[test]
    fn the_fm_cpus_irq_is_the_ym_line() {
        let mut m = machine();
        assert!(!m.fm.ym_ref().irq(), "a reset chip is quiet");
        // Timer A at its shortest, with its IRQ enabled. The period is
        // `1024 - value` sample-times, so the *shortest* is value 1,023 and not
        // value 0: register 0x10 holds the top eight bits and 0x11 the low two, so
        // 0xFF/0x03 is 1,023 and one sample-time. ⚠️ 0x00/0x00 is value 0, which is
        // the **longest** period — 1,024 sample-times — and a frame produces only 932
        // of them, so the timer would not have expired by the time this test looks.
        // 0x14 bit 0 starts the timer and bit 2 enables its IRQ.
        for (reg, val) in [(0x10u8, 0xFF), (0x11, 0x03), (0x14, 0x05)] {
            m.fm.ym().write(reg, val);
        }
        // Long enough for the timer to expire at 3.58 MHz.
        for _ in 0..crate::timing::SF1_VTOTAL {
            m.run_scanline();
        }
        assert!(
            m.fm.ym_ref().irq() || m.fm_irqs_raised() > 0,
            "the timer's IRQ never reached the CPU"
        );
    }

    /// A sound command sets both latches and pulses the FM CPU's NMI.
    ///
    /// `soundcmd_w` writes the one `m_soundlatch` and pulses NMI on
    /// `m_audiocpu` only (`sf.cpp:118-122`). Both boards hold a copy of that one
    /// latch, and this is the single place they are written — which is the invariant
    /// each board's `no_bus_write_can_change_the_latch` protects from the other side.
    #[test]
    fn a_sound_command_reaches_both_boards_and_pulses_one_nmi() {
        let mut m = machine();
        m.board.write_sound_command_for_test(0x42);
        m.step_instruction();
        assert_eq!(m.fm.latch(), 0x42, "the FM board's copy");
        assert_eq!(m.adpcm.latch(), 0x42, "and the ADPCM board's");
        assert!(m.fm_nmis_raised() > 0, "soundcmd_w pulses the FM CPU's NMI");
        assert_eq!(
            m.adpcm_nmis_raised(),
            0,
            "and not the ADPCM CPU's — nothing is wired to it"
        );
    }

    /// An FM ROM whose NMI handler leaves a mark in sound RAM.
    ///
    /// ```text
    /// 0000  31 FF C7    ld sp,$C7FF    the top of the 2 KB of sound RAM
    /// 0003  18 FE       jr $0003       the idle loop
    /// 0066  3A 01 C0    ld a,($C001)   the NMI handler: count the entries
    /// 0069  3C          inc a
    /// 006A  32 01 C0    ld ($C001),a
    /// 006D  ED 45       retn
    /// ```
    ///
    /// ⚠️ **The `ld sp` is load-bearing.** [`z80::Z80::ack_nmi`] *pushes* the return
    /// address, and `Z80::reset` leaves `sp` at 0xFFFF, which on this board is
    /// unmapped: the push would be discarded and the handler's `retn` would pop
    /// 0xFF 0xFF and land in unmapped space, which reads `RST 38h` and pushes again.
    /// So the two tests below run one scanline before writing anything, which is what
    /// gives the CPU time to execute the instruction at 0x0000.
    ///
    /// The mark is a **counter** rather than a constant so that "the handler ran"
    /// and "the handler ran twice" are different observations. It lives at 0xC001
    /// and not 0xC000 because the idle loops in this crate's other SF1 fixtures write
    /// 0xC000, and a mark a loop could also write proves nothing.
    fn fm_with_counting_handler() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0..5].copy_from_slice(&[0x31, 0xFF, 0xC7, 0x18, 0xFE]);
        rom[0x66..0x6F].copy_from_slice(&[0x3A, 0x01, 0xC0, 0x3C, 0x32, 0x01, 0xC0, 0xED, 0x45]);
        rom
    }

    /// A machine whose FM CPU can be caught in its handler, and whose ADPCM CPU spins.
    fn machine_with_fm_handler() -> Sf1 {
        let mut m = Sf1::new(
            &spin_program(),
            crate::sf1::test_video(),
            fm_with_counting_handler(),
            z80_spin(),
        );
        m.reset();
        // One line so `ld sp,$C7FF` has run — see [`fm_with_counting_handler`].
        m.run_scanline();
        m
    }

    /// A sound command lands the FM Z80 at 0x0066 and it comes back.
    ///
    /// The artifact is the byte the handler wrote, not `fm_z80.pc`: a test reading
    /// `pc` would have to catch the CPU inside four instructions, and `run_scanline`
    /// grants 233 T-states at a time. So the handler leaves a mark, and the mark
    /// proves the CPU reached 0x0066 — which is where [`z80::Z80::ack_nmi`] sends it
    /// and the only address on this ROM that writes 0xC001.
    ///
    /// `pc` back at 0x0003 is the second half of the claim: the `retn` popped what
    /// the acknowledge pushed, so the stack survived the round trip. That is
    /// deterministic here because the idle loop is one two-byte instruction jumping
    /// to itself, so every instruction boundary outside the handler has `pc` at
    /// 0x0003.
    #[test]
    fn a_sound_command_lands_the_fm_cpu_in_its_handler_at_0x0066() {
        let mut m = machine_with_fm_handler();
        assert_eq!(m.fm.ram()[1], 0, "the premise: the handler has not run");
        m.board.write_sound_command_for_test(0x42);
        // Finish this line: the scheduler takes the command mid-line and runs the
        // sound CPUs at the line's end.
        m.run_scanline();
        assert_eq!(
            m.fm.ram()[1],
            1,
            "the NMI handler at 0x0066 ran exactly once"
        );
        assert_eq!(m.fm_z80.pc, 0x0003, "and `retn` returned to the idle loop");
        assert_eq!(m.fm.latch(), 0x42, "with the command available to read");
    }

    /// A second command written before the first is serviced loses neither edge.
    ///
    /// Two things this catches, both of them plausible drafts of
    /// [`Sf1::step_instruction`]:
    ///
    /// - `self.fm_z80.nmi = self.board.take_sound_command().is_some()` — an
    ///   assignment rather than a set. Every instruction with nothing pending would
    ///   clear an edge raised by an earlier one, and the handler would run only when
    ///   a command happened to land on the line's last instruction. The three idle
    ///   `step_instruction` calls between the two writes are what make that visible.
    /// - Taking the command once per *line* instead of once per instruction, which
    ///   would drop the first of two commands written in the same line.
    ///
    /// ⚠️ **Two pulses are one serviced interrupt, and that is deliberate.**
    /// `z80::Z80::nmi` is a single `bool` — an edge-triggered pin, not a queue — so a
    /// second pulse on an unserviced pin coalesces. Giving the core a queue is a core
    /// change the spec rules out, and a real NMI pin coalesces the same way. So this
    /// test asserts the counter saw **both** pulses and the handler ran **at least**
    /// once; it does not assert two handler entries, and the fix for a failure is not
    /// to add a queue.
    #[test]
    fn a_second_sound_command_does_not_lose_the_nmi_edge() {
        let mut m = machine_with_fm_handler();
        let nmis = m.fm_nmis_raised();
        m.board.write_sound_command_for_test(0x11);
        m.step_instruction();
        assert_eq!(
            m.fm_nmis_raised(),
            nmis + 1,
            "the premise: the first command pulsed the NMI"
        );
        // Three instructions with nothing pending. `spin_program` is `bra.s -2` at
        // 10 cycles against a 520.83-cycle line, so these cannot end the line and
        // the FM CPU has not run yet — the edge is still unserviced.
        for _ in 0..3 {
            m.step_instruction();
        }
        m.board.write_sound_command_for_test(0x22);
        m.step_instruction();
        assert_eq!(m.fm_nmis_raised(), nmis + 2, "and so did the second");
        assert_eq!(m.fm.ram()[1], 0, "the premise: the FM CPU has not run yet");
        // Now let the line end and the FM CPU take its slice.
        m.run_scanline();
        assert!(
            m.fm.ram()[1] >= 1,
            "the handler never ran: an edge raised four instructions ago was lost"
        );
        assert_eq!(
            m.fm.latch(),
            0x22,
            "and the second command's value is the one there"
        );
    }

    /// A frame produces the right number of stereo frames, interleaved.
    ///
    /// 3,579,545 / 64 = 55,930.4 samples a second, over a 60 Hz frame: 932.2 frames.
    /// Asserted as a range because the count must vary — a frame that always produced
    /// the same number would be running the sample clock off the line count rather
    /// than off T-states spent.
    #[test]
    fn a_frame_produces_about_nine_hundred_thirty_two_stereo_frames() {
        let mut m = machine();
        // One frame to settle the accumulators, then measure three.
        m.run_frame();
        for _ in 0..3 {
            m.drain_samples();
            m.run_frame();
            let n = m.samples().len();
            assert_eq!(
                n % crate::resample::CHANNELS,
                0,
                "{n} samples is not a whole number of frames"
            );
            let frames = n / crate::resample::CHANNELS;
            assert!(
                (925..=940).contains(&frames),
                "one frame's audio, not a burst or a gap: {frames} stereo frames"
            );
        }
    }

    /// The samples are the mix's, so the YM reaches both sides and the MSMs reach both.
    ///
    /// End to end: a keyed YM voice and a fed MSM5205 both have to appear. Without
    /// this, a scheduler that produced silent frames at exactly the right rate would
    /// pass every count assertion above.
    #[test]
    fn a_playing_board_is_audible_in_the_samples() {
        let mut m = machine();
        // A YM voice, keyed, on channel 0 — the same minimal patch Task 12 uses.
        for (reg, val) in [
            (0x60u8, 0x00),
            (0x68, 0x00),
            (0x70, 0x00),
            (0x78, 0x00),
            (0x80, 0x1F),
            (0x88, 0x1F),
            (0x90, 0x1F),
            (0x98, 0x1F),
            (0x28, 0x4A),
            (0x08, 0x78),
        ] {
            m.fm.ym().write(reg, val);
        }
        m.drain_samples();
        for _ in 0..32 {
            m.run_scanline();
        }
        let ym_only = m.drain_samples();
        assert!(
            ym_only.iter().any(|&s| s != 0),
            "the keyed YM voice is not in the samples"
        );
        // Now an MSM5205 as well, fed a large nibble repeatedly so its level walks
        // away from zero.
        for _ in 0..16 {
            m.adpcm.msm_mut(0).msm_w(0x07);
            for _ in 0..8 {
                m.adpcm.tick();
            }
        }
        assert_ne!(m.adpcm.msm(0).output(), 0, "the chip is at a level");
        for _ in 0..32 {
            m.run_scanline();
        }
        let both = m.drain_samples();
        assert!(both.iter().any(|&s| s != 0), "still audible");
    }

    /// `reset` puts the schedule back exactly, on all three CPUs.
    ///
    /// Two runs from reset must produce samples at the same instants; an accumulator
    /// or a debt surviving would make them differ, and the difference is a per-run
    /// divergence that no single run can detect. [`Cps1::reset`]'s argument, three
    /// times over.
    #[test]
    fn reset_restores_the_whole_schedule() {
        let mut m = machine();
        for _ in 0..37 {
            m.run_scanline();
        }
        assert_ne!(m.total_cycles, 0, "the premise");
        m.reset();
        assert_eq!(m.total_cycles, 0);
        assert_eq!(m.line, 0);
        assert_eq!(m.carry(), 0);
        assert_eq!(m.line_cycles_remainder(), 0);
        assert_eq!(m.z80_cycles(), 0);
        assert_eq!(m.adpcm_z80_cycles(), 0);
        assert_eq!(m.fm_debt(), 0);
        assert_eq!(m.adpcm_debt(), 0);
        assert_eq!(m.fm_carry_remainder(), 0);
        assert_eq!(m.adpcm_carry_remainder(), 0);
        assert_eq!(m.adpcm_irq_remainder(), 0);
        assert_eq!(m.sample_acc(), 0);
        assert!(m.samples().is_empty(), "a reset drops undrained audio");
        // And both chips are silent, while the bank and the latches are not cleared —
        // `machine_reset` (`sf.cpp:744-748`) touches neither.
        assert_eq!(m.adpcm.output(), (0, 0));
    }

    /// Two machines run the same way produce identical everything.
    ///
    /// The determinism claim the save state rests on, made across three CPUs and four
    /// accumulators. Compared as whole values at the end rather than sample by sample,
    /// so a divergence anywhere is one failure rather than nine hundred.
    #[test]
    fn two_machines_run_the_same_way_agree() {
        let mut a = machine();
        let mut b = machine();
        for _ in 0..2 {
            a.run_frame();
            b.run_frame();
        }
        assert_eq!(a.total_cycles, b.total_cycles);
        assert_eq!(a.z80_cycles(), b.z80_cycles());
        assert_eq!(a.adpcm_z80_cycles(), b.adpcm_z80_cycles());
        assert_eq!(a.samples(), b.samples());
        assert!(!a.samples().is_empty(), "the run produced audio to compare");
        assert_eq!(a.carry(), b.carry());
        assert_eq!(a.fm_debt(), b.fm_debt());
        assert_eq!(a.adpcm_debt(), b.adpcm_debt());
        assert_eq!(a.adpcm.trace(), b.adpcm.trace());
        assert_eq!(a.fm.trace(), b.fm.trace());
    }

    /// Stepping and running produce the same samples.
    ///
    /// One code path, deliberately: [`Cps1::step_instruction`]'s note — "a separate
    /// stepping path is a debugger that single-steps a machine subtly unlike the one
    /// that runs" — and this is the test that would catch the second copy.
    #[test]
    fn stepping_and_running_produce_the_same_samples() {
        let mut stepped = machine();
        let mut run = machine();
        run.run_scanline();
        while stepped.line == 0 {
            stepped.step_instruction();
        }
        assert_eq!(stepped.total_cycles, run.total_cycles);
        assert_eq!(stepped.z80_cycles(), run.z80_cycles());
        assert_eq!(stepped.adpcm_z80_cycles(), run.adpcm_z80_cycles());
        assert_eq!(stepped.samples(), run.samples());
        assert_eq!(stepped.adpcm.msm(0).pending(), run.adpcm.msm(0).pending());
    }

    /// The MSM5205s advance 25 master clocks per line, both of them.
    ///
    /// Measured through the chip's own countdown rather than a counter this file
    /// keeps: arm a capture and check it lands within the line it was armed in, which
    /// is what 25 ticks against a 6-tick delay means.
    #[test]
    fn the_msms_advance_twenty_five_clocks_a_line() {
        let mut m = machine();
        m.adpcm.msm_mut(0).msm_w(0x07);
        m.adpcm.msm_mut(1).msm_w(0x07);
        assert_eq!(
            m.adpcm.msm(0).pending(),
            crate::sf1::msm5205::CAPTURE_CLOCKS
        );
        m.run_scanline();
        assert_eq!(
            m.adpcm.msm(0).pending(),
            0,
            "the capture fired within the line"
        );
        assert_ne!(m.adpcm.msm(0).signal(), 0, "and decoded");
        assert_eq!(
            m.adpcm.msm(0).signal(),
            m.adpcm.msm(1).signal(),
            "both chips got the same 25 ticks"
        );
    }

    /// A vblank on line 240 raises the 68000's interrupt, at autovector 0x64.
    ///
    /// `set_vblank_int("screen", irq1_line_hold)` (`sf.cpp:755`) with no
    /// `set_interrupt_mixer` call, so the default mixer encodes level 1 → vector
    /// 24 + 1 = 25 → 0x64. ⚠️ CPS-1's is 0x68 because it wires IPL individually at
    /// level 2, and copying that gives a board whose interrupt is never
    /// acknowledged — a game that runs one frame and stops.
    #[test]
    fn vblank_is_level_one_on_line_two_hundred_forty() {
        let mut m = machine();
        assert_eq!(crate::timing::SF1_VBSTART, 240);
        assert!(!m.board.vblank_pending(), "not yet");
        let before = m.board.trace.vblanks;
        // Up to the start of line 240, not through it: the grant-and-assert block is
        // the *first* thing `step_instruction` does on a line, so nothing has happened
        // yet while `line` reads 240 and no instruction has stepped.
        while m.line < crate::timing::SF1_VBSTART {
            m.run_scanline();
        }
        assert_eq!(m.line, crate::timing::SF1_VBSTART);
        assert_eq!(m.board.trace.vblanks, before, "the premise: not raised yet");
        // One instruction into line 240 raises it, and the 68000 takes it in the same
        // step.
        m.step_instruction();
        assert_eq!(m.board.trace.vblanks, before + 1, "raised on line 240");
        assert_eq!(m.cpu.pending_irq, 1, "level 1, not CPS-1's 2");
        // ⚠️ And `vblank_pending` is already back to false, because the acknowledge is
        // the *vector fetch*: `Sf1Board::read16` clears the flag when the CPU reads
        // 0x64 (`board.rs:245-246`). So `vblank_pending` is not the observable after
        // the step — a test asserting it there would fail on a correct machine, which
        // is what the counter is for.
        assert!(
            !m.board.vblank_pending(),
            "the vector fetch is the acknowledge"
        );
    }

    /// A frame is `SF1_VTOTAL` lines and the wrap counts one.
    #[test]
    fn a_frame_is_two_hundred_fifty_six_lines() {
        let mut m = machine();
        assert_eq!(m.timing.lines_per_frame, crate::timing::SF1_VTOTAL);
        let f0 = m.board.trace.frames;
        m.run_frame();
        assert_eq!(m.line, 0, "back at the top");
        assert_eq!(m.board.trace.frames, f0 + 1, "counted on the wrap");
    }

    /// `restore_schedule` round-trips every scheduler field.
    ///
    /// All four remainders included. A restored machine that zeroed one would be a
    /// fraction of a cycle out per line and permanently out of step — the argument
    /// [`RationalAccumulator::with_remainder`]'s own doc makes, and the reason that
    /// method is public.
    #[test]
    fn restore_schedule_round_trips_every_field() {
        let mut m = machine();
        // An odd number of lines plus a few instructions, so no accumulator is at a
        // round value.
        for _ in 0..37 {
            m.run_scanline();
        }
        m.step_instruction();
        m.step_instruction();
        let saved = (
            m.total_cycles,
            m.line,
            m.carry(),
            m.line_cycles_remainder(),
            m.z80_cycles(),
            m.fm_debt(),
            m.fm_carry_remainder(),
            m.adpcm_z80_cycles(),
            m.adpcm_debt(),
            m.adpcm_carry_remainder(),
            m.adpcm_irq_remainder(),
            m.sample_acc(),
        );
        assert_ne!(saved.3, 0, "the 68000's remainder is mid-fraction");
        let mut fresh = machine();
        fresh.restore_schedule(
            saved.0, saved.1, saved.2, saved.3, saved.4, saved.5, saved.6, saved.7, saved.8,
            saved.9, saved.10, saved.11,
        );
        assert_eq!(
            (
                fresh.total_cycles,
                fresh.line,
                fresh.carry(),
                fresh.line_cycles_remainder(),
                fresh.z80_cycles(),
                fresh.fm_debt(),
                fresh.fm_carry_remainder(),
                fresh.adpcm_z80_cycles(),
                fresh.adpcm_debt(),
                fresh.adpcm_carry_remainder(),
                fresh.adpcm_irq_remainder(),
                fresh.sample_acc(),
            ),
            saved
        );
    }

    /// A restored schedule runs on identically from where it was saved.
    ///
    /// Stronger than the field-by-field round trip and the reason that one is not
    /// enough: a remainder restored into the wrong accumulator would round-trip
    /// perfectly and diverge on the next line.
    #[test]
    fn a_restored_schedule_continues_identically() {
        let mut original = machine();
        for _ in 0..37 {
            original.run_scanline();
        }
        let mut restored = machine();
        restored.restore_schedule(
            original.total_cycles,
            original.line,
            original.carry(),
            original.line_cycles_remainder(),
            original.z80_cycles(),
            original.fm_debt(),
            original.fm_carry_remainder(),
            original.adpcm_z80_cycles(),
            original.adpcm_debt(),
            original.adpcm_carry_remainder(),
            original.adpcm_irq_remainder(),
            original.sample_acc(),
        );
        // The CPUs and memory are not restored here — only the schedule — so compare
        // the schedule's own progression rather than the samples.
        let before = (original.total_cycles, original.z80_cycles());
        original.run_scanline();
        let advanced_by = (
            original.total_cycles - before.0,
            original.z80_cycles() - before.1,
        );
        let r0 = (restored.total_cycles, restored.z80_cycles());
        restored.run_scanline();
        assert_eq!(
            (restored.total_cycles - r0.0, restored.z80_cycles() - r0.1),
            advanced_by,
            "the restored machine's next line is a different length"
        );
    }

    /// The audio path is interleaved stereo, and CPS-1's mono adapter is not used here.
    ///
    /// SF1's two sides genuinely differ — the YM's channels go to opposite speakers —
    /// so a frame whose two slots are always equal means [`mix()`](fn@crate::sf1::mix) was
    /// bypassed or fed one YM channel twice.
    #[test]
    fn the_two_sides_differ_when_the_ym_is_panned() {
        let mut m = machine();
        // A voice on channel 0 with the panning register (0x20 + ch) set to left only:
        // bit 6 is left enable, bit 7 is right.
        for (reg, val) in [
            (0x20u8, 0x40), // RL/FB/CONN: left only
            (0x60, 0x00),
            (0x68, 0x00),
            (0x70, 0x00),
            (0x78, 0x00),
            (0x80, 0x1F),
            (0x88, 0x1F),
            (0x90, 0x1F),
            (0x98, 0x1F),
            (0x28, 0x4A),
            (0x08, 0x78),
        ] {
            m.fm.ym().write(reg, val);
        }
        m.drain_samples();
        for _ in 0..64 {
            m.run_scanline();
        }
        let s = m.samples();
        assert!(!s.is_empty());
        assert!(
            s.chunks(crate::resample::CHANNELS).any(|f| f[0] != f[1]),
            "every frame has equal sides, so the panning never reached the mix"
        );
    }

    /// The mix's clip flag reaches a counter.
    ///
    /// Both chips at their rail sum to exactly -32,768 — `i16::MIN`, in range — so
    /// the saturation comes from the YM's 3/5 on top of that. Any negative `ym_l`
    /// sample takes the left side past the rail, and a keyed voice's waveform is
    /// negative for about half of its samples. Task 11's `the_worst_case_saturates`
    /// proves the arithmetic; this proves the flag is wired to something.
    #[test]
    fn a_saturating_mix_reaches_the_clip_counter() {
        let mut m = machine();
        // Both chips driven to the negative rail: nibble 0x0F is the largest negative
        // step and its `index_shift` of +8 climbs the step index, so a run of them
        // walks the internal signal to its -2048 clamp, which is -16,384 out.
        for _ in 0..64 {
            for chip in 0..crate::sf1::adpcm2::CHIPS {
                m.adpcm.msm_mut(chip).msm_w(0x0F);
            }
            for _ in 0..8 {
                m.adpcm.tick();
            }
        }
        assert_eq!(
            m.adpcm.output(),
            (-16_384, -16_384),
            "both chips are at the rail, so the MSM term is exactly i16::MIN"
        );
        // And a YM voice, so there is something to push it past.
        for (reg, val) in [
            (0x60u8, 0x00),
            (0x68, 0x00),
            (0x70, 0x00),
            (0x78, 0x00),
            (0x80, 0x1F),
            (0x88, 0x1F),
            (0x90, 0x1F),
            (0x98, 0x1F),
            (0x28, 0x4A),
            (0x08, 0x78),
        ] {
            m.fm.ym().write(reg, val);
        }
        let before = m.mix_clips();
        for _ in 0..64 {
            m.run_scanline();
        }
        assert!(
            m.mix_clips() > before,
            "a mix past the rails did not report a clip"
        );
    }
}
