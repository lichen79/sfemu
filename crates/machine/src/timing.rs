//! Video and CPU timing.
//!
//! The primitives come from MAME `cps1.h:39-47`, which in turn credits Charles
//! MacDonald's measurements of a real board (`cps1.h:30-38`). Everything else here
//! is **derived** from them, and every derived figure is checked against a
//! hand-written literal in the tests below.
//!
//! # Why the literals matter more here than anywhere else
//!
//! A timing bug is not a crash. A frame that is 639 cycles per line instead of 640
//! runs 0.16% slow: music drifts against animation over a match, and nothing ever
//! looks broken enough to investigate. The derivation is three divisions and the
//! only defence against getting one of them wrong is a number written by hand from
//! the arithmetic — an `assert_eq!(a / b, a / b)` passes for every value of `a` and
//! `b`, including wrong ones.

/// `CPS_PIXEL_CLOCK` — `XTAL(16'000'000)/2`, `cps1.h:39`.
pub const PIXEL_CLOCK: u32 = 8_000_000;

/// `CPS_HTOTAL` — pixel clocks per scanline, `cps1.h:41`.
pub const HTOTAL: u32 = 512;
/// `CPS_HBEND` — the first visible pixel column, `cps1.h:42`.
pub const HBEND: u32 = 64;
/// `CPS_HBSTART` — one past the last visible pixel column, `cps1.h:43`.
pub const HBSTART: u32 = 448;

/// `CPS_VTOTAL` — scanlines per frame, `cps1.h:45`.
pub const VTOTAL: u32 = 262;
/// `CPS_VBEND` — the first visible scanline, `cps1.h:46`.
pub const VBEND: u32 = 16;
/// `CPS_VBSTART` — one past the last visible scanline, `cps1.h:47`.
pub const VBSTART: u32 = 240;

/// The 68000's clock: `XTAL(10'000'000)`, "verified on pcb", `cps1.cpp:3911`.
///
/// Some later CPS-1 games run at 12 MHz (`cps1_12MHz`), which is why the cycle
/// budget is a [`Timing`] field and not a constant.
pub const CPU_HZ_10M: u32 = 10_000_000;

/// The sound board's crystal, `XTAL(3'579'545)` — the NTSC colour subcarrier.
///
/// Both the Z80 and the YM2151 run from it, which is why one accumulator can drive
/// both. From `cps1.cpp`'s audio CPU and `ym2151` device clocks.
pub const SOUND_XTAL: u32 = 3_579_545;

/// The sound Z80's clock, in Hz. Same crystal, undivided.
pub const Z80_HZ: u32 = SOUND_XTAL;

/// T-states per scanline, as an exact fraction: 715,909 / 3,125.
///
/// **The first inexact division in this project.** 3,579,545 / 15,625 is
/// 229.09088 T per line; `the_z80_clock_does_not_divide_into_a_scanline` in this
/// module's tests pins the remainder that forbids a truncated constant. The
/// denominator is 5^5, so a `u32` accumulator is exact and never needs a float.
pub const Z80_T_NUM: u32 = 715_909;
/// The denominator of [`Z80_T_NUM`]. 3,125 = 5^5.
pub const Z80_T_DEN: u32 = 3_125;

/// Input clocks per YM2151 sample. Exactly 64: the chip divides by 2 then by 32.
///
/// This duplicates `ym2151::Ym2151::sample_clocks()`. The copy exists because this
/// module predates `machine`'s dependency on `ym2151`, and the two are held together
/// by `the_sample_rate_is_fractional_per_line_and_per_frame`, which asserts they
/// agree: a second literal that silently disagreed with the chip would put every
/// sample in the wrong place, and nothing else here would notice.
pub const YM_SAMPLE_CLOCKS: u32 = 64;

/// Hands out `num / den` units per step without drifting.
///
/// Integer only: the remainder carries between steps, so the total after `den` steps
/// is exactly `num`. Used for the Z80's T-states per line and the YM2151's samples
/// per T-state, both of which are fractional.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RationalAccumulator {
    num: u32,
    den: u32,
    rem: u32,
}

impl RationalAccumulator {
    /// A new accumulator at zero remainder.
    ///
    /// # Panics
    ///
    /// If `den` is zero. This is an internal invariant, not guest input: the
    /// denominators here are compile-time constants, and a zero one would make
    /// `advance` divide by zero on the first step rather than at construction.
    #[must_use]
    pub const fn new(num: u32, den: u32) -> Self {
        assert!(den != 0, "a zero denominator is a programming error");
        Self { num, den, rem: 0 }
    }

    /// An accumulator resumed mid-fraction, for a save state.
    ///
    /// # Why this exists at all
    ///
    /// [`RationalAccumulator::new`] zeroes the remainder, and the remainder is state:
    /// dropping it puts a restored machine one T-state out within a line and then
    /// permanently out of step. A save-state codec lives outside this crate — it
    /// cannot reach the private field — so without this the only alternatives were
    /// public fields, which would let any caller move the fraction, or a save state
    /// that silently lost it.
    ///
    /// `rem` is taken modulo `den` rather than rejected: a remainder at or above the
    /// denominator is arithmetically a whole unit that [`advance`](Self::advance)
    /// would hand out on the next step anyway, and the value came from a file.
    ///
    /// # Panics
    ///
    /// If `den` is zero, for [`RationalAccumulator::new`]'s reason.
    #[must_use]
    pub const fn with_remainder(num: u32, den: u32, rem: u32) -> Self {
        assert!(den != 0, "a zero denominator is a programming error");
        Self {
            num,
            den,
            rem: rem % den,
        }
    }

    /// The ratio this accumulator hands out, as `(num, den)`.
    ///
    /// A save state carries the remainder, not the ratio — the ratio is the board's,
    /// fixed by its crystals — but a codec has to write *something* it can read back,
    /// and reconstructing from the board's constants while asserting these match is
    /// what makes a state written for one clock refuse to restore silently into
    /// another. See `frontend::state`'s Z80 accumulator field.
    #[must_use]
    pub const fn ratio(&self) -> (u32, u32) {
        (self.num, self.den)
    }

    /// The whole units for this step, carrying the fraction forward.
    pub fn advance(&mut self) -> u32 {
        let total = self.num + self.rem;
        self.rem = total % self.den;
        total / self.den
    }

    /// The carried fraction, in units of `1 / den`.
    #[must_use]
    pub const fn remainder(&self) -> u32 {
        self.rem
    }
}

/// How the CPU is interleaved with the beam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// 68000 clock in Hz.
    pub cpu_hz: u32,
    /// 68000 cycles the scheduler grants per scanline.
    pub cycles_per_line: u32,
    /// Scanlines in a frame, counting blanking.
    pub lines_per_frame: u32,
    /// The scanline on which IPL1 is asserted. `cps1.cpp:394-396` —
    /// `if (scanline == 240) set_input_line(M68K_IRQ_IPL1, ASSERT_LINE)`, which
    /// is `CPS_VBSTART`, the line the beam leaves the visible area on.
    pub vblank_line: u32,
}

impl Timing {
    /// The 10 MHz CPS-1 configuration — SF2's (`cps1.cpp:3909-3925`).
    ///
    /// # Why the integer division is exact
    ///
    /// 8 MHz / 512 = 15,625 lines per second with no remainder, and
    /// 10 MHz / 15,625 = 640 cycles per line, also with no remainder. **Both
    /// divisions are exact for this pair of clocks**, which removes accumulated
    /// fractional error from the scheduler entirely. The 12 MHz variant gives 768,
    /// exact as well. A board whose clocks did not divide evenly would need a
    /// fractional accumulator here, and
    /// `cps1_frame_geometry_is_384x224_at_59_63_hz` asserts the two remainders are
    /// zero so that a future board needing one cannot be added without noticing.
    pub const fn cps1_10mhz() -> Self {
        Self {
            cpu_hz: CPU_HZ_10M,
            cycles_per_line: 640,
            lines_per_frame: VTOTAL,
            vblank_line: VBSTART,
        }
    }

    /// 68000 cycles in one frame.
    pub const fn cycles_per_frame(&self) -> u32 {
        self.cycles_per_line * self.lines_per_frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every derived figure against a literal.
    ///
    /// ⚠️ Each right-hand side is written by hand from the arithmetic, not
    /// recomputed from the left.
    #[test]
    fn cps1_frame_geometry_is_384x224_at_59_63_hz() {
        assert_eq!(HBSTART - HBEND, 384, "visible width");
        assert_eq!(VBSTART - VBEND, 224, "visible height");
        assert_eq!(PIXEL_CLOCK / HTOTAL, 15_625, "lines per second");
        assert_eq!(PIXEL_CLOCK % HTOTAL, 0, "and that division is exact");
        assert_eq!(
            CPU_HZ_10M / (PIXEL_CLOCK / HTOTAL),
            640,
            "CPU cycles per line"
        );
        assert_eq!(
            CPU_HZ_10M % (PIXEL_CLOCK / HTOTAL),
            0,
            "and that one is exact too"
        );
        assert_eq!(640 * VTOTAL, 167_680, "CPU cycles per frame");
        // 8,000,000 / (512 × 262) = 59.6374...; assert the milli-hertz so the
        // figure is pinned without a float comparison. MAME's own comment at
        // cps1.h:36 says "Refresh rate: 59.63 MHz" — the unit is a typo there, the
        // number is not.
        // `u64` because 8,000,000 × 1000 overflows a `u32` — and `rustc` refuses
        // to compile the `u32` form rather than wrapping it, which is how this was
        // caught.
        assert_eq!(
            u64::from(PIXEL_CLOCK) * 1000 / u64::from(HTOTAL * VTOTAL),
            59_637
        );
    }

    /// `cps1_10mhz()`'s hard-coded 640 is checked against the derivation rather
    /// than merely asserted to equal itself.
    ///
    /// This is the assertion that catches a hand-edited `cycles_per_line`: the
    /// geometry test above would still pass, because it never reads the struct.
    #[test]
    fn the_default_timing_matches_the_derivation() {
        let t = Timing::cps1_10mhz();
        assert_eq!(t.cpu_hz, 10_000_000);
        assert_eq!(t.cycles_per_line, CPU_HZ_10M / (PIXEL_CLOCK / HTOTAL));
        assert_eq!(t.cycles_per_line, 640, "and the literal, both ways");
        assert_eq!(t.lines_per_frame, 262);
        assert_eq!(t.cycles_per_frame(), 167_680);
        assert_eq!(t.vblank_line, 240);
    }

    /// `cycles_per_frame` multiplies the two fields it names.
    ///
    /// A `Timing` nobody constructs from `cps1_10mhz()` — the 12 MHz variant, or a
    /// unit test's — must get the same arithmetic. Without this, `cycles_per_frame`
    /// returning the literal 167,680 would pass every test above.
    #[test]
    fn cycles_per_frame_is_the_product_and_not_a_constant() {
        let t = Timing {
            cpu_hz: 12_000_000,
            cycles_per_line: 768,
            lines_per_frame: 262,
            vblank_line: 240,
        };
        assert_eq!(t.cycles_per_frame(), 201_216, "768 × 262");
        assert_eq!(
            12_000_000 / (PIXEL_CLOCK / HTOTAL),
            768,
            "12 MHz is exact too"
        );
        assert_eq!(12_000_000 % (PIXEL_CLOCK / HTOTAL), 0);

        // Both real CPS-1 variants have 262 lines, so the two cases above are also
        // satisfied by `cycles_per_line * 262` — verified: that mutant survived
        // until this third case existed. A `lines_per_frame` no board uses is what
        // makes the second operand load-bearing.
        let odd = Timing {
            cpu_hz: 10_000_000,
            cycles_per_line: 640,
            lines_per_frame: 100,
            vblank_line: 90,
        };
        assert_eq!(odd.cycles_per_frame(), 64_000, "640 × 100");
    }

    #[test]
    fn vblank_is_inside_the_frame_and_right_after_the_visible_area() {
        let t = Timing::cps1_10mhz();
        assert!(t.vblank_line < t.lines_per_frame);
        assert_eq!(t.vblank_line, VBSTART, "the beam leaves the visible area");
        assert_eq!(
            t.vblank_line - VBEND,
            224,
            "and it has drawn all 224 visible lines by then"
        );
    }

    /// The blanking budget: 262 lines total, 224 visible.
    ///
    /// 38 lines of vertical blanking is 38 × 640 = 24,320 cycles, which is how much
    /// 68000 time a game's vblank handler has before the beam is back in the
    /// visible area. Sub-project C will need this figure; pinning it here means a
    /// wrong `VTOTAL` or `VBEND` shows up as a wrong budget rather than as a
    /// mysteriously slow game.
    #[test]
    fn vertical_blanking_is_38_lines_and_24320_cycles() {
        assert_eq!(VTOTAL - (VBSTART - VBEND), 38);
        assert_eq!(38 * 640, 24_320);
        let t = Timing::cps1_10mhz();
        assert_eq!(
            t.cycles_per_frame() - 224 * t.cycles_per_line,
            24_320,
            "the frame minus the visible lines"
        );
    }

    /// The Z80's clock does not divide evenly into a scanline.
    ///
    /// **This is the first inexact division in this project.** The 68000's 10 MHz
    /// over 15,625 lines/s is exactly 640 T/line and the pixel clock is exactly 512
    /// per line — both asserted above. The sound Z80 runs at XTAL(3'579'545), and
    /// 3,579,545 / 15,625 = 229.09088 T/line, which is 715,909/3,125.
    ///
    /// Truncating to 229 loses 1,420 T per second — 396.7 ppm, about 71 ms of drift
    /// across a three-minute match. So the scheduler carries a rational accumulator
    /// instead, and this test pins the remainder that makes it necessary.
    #[test]
    fn the_z80_clock_does_not_divide_into_a_scanline() {
        assert_eq!(Z80_HZ, 3_579_545);
        assert_eq!(SOUND_XTAL, 3_579_545);
        let lines_per_second = PIXEL_CLOCK / HTOTAL; // 15,625
        assert_eq!(lines_per_second, 15_625);
        assert_eq!(
            Z80_HZ % lines_per_second,
            1_420,
            "the remainder that forbids a truncated constant"
        );
        assert_eq!(Z80_HZ / lines_per_second, 229, "and the truncated value");
        assert_eq!((Z80_T_NUM, Z80_T_DEN), (715_909, 3_125));
        // 3,125 is 5^5, so the accumulator is exact after 3,125 lines and never
        // accumulates floating-point error — there is no float here at all.
        assert_eq!(5u32.pow(5), 3_125);
    }

    /// The accumulator is exact over its period: 3,125 lines is exactly 715,909 T.
    ///
    /// The property that makes this correct rather than merely better than
    /// truncating. A truncated 229 gives 715,625 over the same span — 284 short.
    #[test]
    fn the_rational_accumulator_is_exact_over_its_period() {
        let mut acc = RationalAccumulator::new(Z80_T_NUM, Z80_T_DEN);
        let total: u64 = (0..Z80_T_DEN).map(|_| u64::from(acc.advance())).sum();
        assert_eq!(total, u64::from(Z80_T_NUM), "exact after one period");
        assert_eq!(acc.remainder(), 0, "and back where it started");
        assert_eq!(
            229u64 * u64::from(Z80_T_DEN),
            715_625,
            "what truncating costs"
        );
    }

    /// Each line gets 229 or 230 T-states, never anything else.
    ///
    /// A scheduler that handed out a burst to catch up would put the Z80's writes in
    /// the wrong scanline. The accumulator spreads the remainder one T at a time.
    ///
    /// **284, not 1,420.** The plan wrote 1,420 here, which is
    /// `Z80_HZ % 15_625` — the remainder over a *second*, whose denominator is
    /// 15,625 lines. The accumulator's period is 3,125 lines, and
    /// `Z80_T_NUM % Z80_T_DEN` is 284. The two are the same fraction
    /// (1,420/15,625 = 284/3,125 = 0.09088) reduced by 5, and the count asserted
    /// here has to use the accumulator's own denominator or it is off by exactly
    /// that factor. Both figures appear below so the relationship is visible rather
    /// than looking like one of them is a typo.
    #[test]
    fn each_line_gets_two_hundred_twenty_nine_or_thirty() {
        let mut acc = RationalAccumulator::new(Z80_T_NUM, Z80_T_DEN);
        let mut counts = std::collections::BTreeMap::new();
        for _ in 0..Z80_T_DEN * 3 {
            *counts.entry(acc.advance()).or_insert(0u32) += 1;
        }
        assert_eq!(counts.keys().copied().collect::<Vec<_>>(), vec![229, 230]);
        // 284 of every 3,125 lines get the extra T.
        assert_eq!(Z80_T_NUM % Z80_T_DEN, 284);
        assert_eq!(counts[&230], 284 * 3);
        assert_eq!(counts[&229], (3_125 - 284) * 3);
        // And the per-second figure the plan quoted is the same fraction over the
        // 15,625 lines in a second, which is 5x this period.
        assert_eq!(1_420 / 5, 284);
        assert_eq!(15_625 / 5, Z80_T_DEN);
    }

    /// A frame is 60,021.81056 T, and the accumulator does not round it away.
    ///
    /// 262 lines at 715,909/3,125 each. The fractional part is why a per-frame
    /// integer constant is also wrong, not just a per-line one.
    #[test]
    fn a_frame_is_not_a_whole_number_of_t_states_either() {
        let mut acc = RationalAccumulator::new(Z80_T_NUM, Z80_T_DEN);
        let frame: u32 = (0..VTOTAL).map(|_| acc.advance()).sum();
        // The first frame gets 60,021 or 60,022 depending on the phase; over 3,125
        // lines the average is exact, which the period test above proves.
        assert!(frame == 60_021 || frame == 60_022, "got {frame}");
        assert_ne!(acc.remainder(), 0, "a frame is not a whole period");
    }

    /// The YM2151 samples per line and per frame are also fractional.
    ///
    /// 3,579,545 / 64 = 55,930.39 Hz, so 3.579545 samples per line and 937.84 per
    /// frame. The sample accumulator runs over T-states actually spent rather than
    /// over lines, so it stays locked to the Z80 rather than drifting against it.
    #[test]
    fn the_sample_rate_is_fractional_per_line_and_per_frame() {
        assert_eq!(YM_SAMPLE_CLOCKS, 64);
        // **The assertion [`YM_SAMPLE_CLOCKS`]'s own doc says this module owes.** The
        // constant is a second copy of the chip's figure, written before `machine`
        // depended on `ym2151`; a copy that silently disagreed would put every sample
        // in the wrong place and nothing else here would notice, because every other
        // test in this file reads the copy rather than the chip.
        assert_eq!(
            YM_SAMPLE_CLOCKS,
            ym2151::Ym2151::sample_clocks(),
            "the constant and the chip must agree about the sample period"
        );
        assert_eq!(
            Z80_HZ % YM_SAMPLE_CLOCKS,
            25,
            "not a whole sample rate either"
        );
        // Per line: 715,909 / 3,125 / 64 T per sample.
        let per_frame_num = u64::from(Z80_T_NUM) * u64::from(VTOTAL);
        let per_frame_den = u64::from(Z80_T_DEN) * u64::from(YM_SAMPLE_CLOCKS);
        assert_eq!(per_frame_num / per_frame_den, 937, "937 whole samples");
        assert_ne!(per_frame_num % per_frame_den, 0, "and a fraction left over");
    }

    /// The accumulator is general, not a hand-tuned Z80 line counter.
    ///
    /// Every test above drives it with the single ratio 715,909/3,125, so an
    /// implementation that ignored its arguments and returned 229 or 230 from a
    /// hardcoded table would pass all of them. These ratios are chosen so the
    /// expected sequences are short enough to write out by hand:
    ///
    /// * 1/3 — two zero steps then a one, which is the case an implementation that
    ///   returned `max(1, ...)` or added the remainder in the wrong direction gets
    ///   wrong.
    /// * 7/2 — 3, 4, 3, 4: the whole part is greater than one and the fraction is
    ///   exactly a half.
    /// * 5/1 — an exact ratio: every step is 5 and the remainder is always 0.
    #[test]
    fn the_accumulator_is_not_specific_to_the_z80s_ratio() {
        let run = |num, den, steps| {
            let mut acc = RationalAccumulator::new(num, den);
            (0..steps).map(|_| acc.advance()).collect::<Vec<u32>>()
        };
        assert_eq!(run(1, 3, 7), vec![0, 0, 1, 0, 0, 1, 0]);
        assert_eq!(run(7, 2, 4), vec![3, 4, 3, 4]);
        assert_eq!(run(5, 1, 3), vec![5, 5, 5]);

        // An exact ratio never carries anything, and a fractional one carries the
        // numerator's remainder on its first step.
        let mut exact = RationalAccumulator::new(5, 1);
        exact.advance();
        assert_eq!(exact.remainder(), 0);
        let mut third = RationalAccumulator::new(1, 3);
        assert_eq!(third.remainder(), 0, "a fresh accumulator carries nothing");
        third.advance();
        assert_eq!(third.remainder(), 1, "and then carries the fraction");
    }

    /// A zero denominator panics at construction, not on the first division.
    #[test]
    #[should_panic(expected = "a zero denominator")]
    fn a_zero_denominator_is_rejected() {
        let _ = RationalAccumulator::new(1, 0);
    }

    /// And `with_remainder` rejects one too, for the same reason.
    #[test]
    #[should_panic(expected = "a zero denominator")]
    fn a_zero_denominator_is_rejected_with_a_remainder_too() {
        let _ = RationalAccumulator::with_remainder(1, 0, 0);
    }

    /// A resumed accumulator continues the sequence rather than restarting it.
    ///
    /// The save state's requirement, asserted as behaviour and not as a getter
    /// agreeing with a setter: run an accumulator part-way, rebuild one from its
    /// reported remainder, and require the *next* steps to match. A
    /// `with_remainder` that ignored `rem` passes a `remainder()` comparison — the
    /// field is read back through the same accessor — and fails here.
    #[test]
    fn an_accumulator_resumes_where_the_remainder_says() {
        let mut original = RationalAccumulator::new(Z80_T_NUM, Z80_T_DEN);
        // 1,000 steps is not a multiple of the 3,125-step period, so the remainder
        // here is not zero and the resumed copy has something to carry.
        for _ in 0..1_000 {
            original.advance();
        }
        let rem = original.remainder();
        assert_ne!(rem, 0, "the premise: the fraction is mid-carry");

        let mut resumed = RationalAccumulator::with_remainder(Z80_T_NUM, Z80_T_DEN, rem);
        let want: Vec<u32> = (0..2_125).map(|_| original.advance()).collect();
        let got: Vec<u32> = (0..2_125).map(|_| resumed.advance()).collect();
        assert_eq!(got, want, "the same 2,125 steps to the end of the period");
        // And the totals: 1,000 + 2,125 steps is one full period, so a resumed
        // accumulator that had lost the fraction would be short by the carry.
        assert_eq!(resumed.remainder(), 0, "back at the period boundary");

        // The control: the same accumulator built with `new` diverges, which is what
        // makes the remainder load-bearing rather than decorative.
        let mut fresh = RationalAccumulator::new(Z80_T_NUM, Z80_T_DEN);
        let plain: Vec<u32> = (0..2_125).map(|_| fresh.advance()).collect();
        assert_ne!(plain, want, "dropping the remainder changes the sequence");
    }

    /// A remainder at or above the denominator folds rather than being rejected.
    ///
    /// The value comes from a file. `den + 7` is `7` — the same accumulator, one whole
    /// unit already handed out — so folding it is arithmetic and not a guess.
    #[test]
    fn an_out_of_range_remainder_folds_into_the_period() {
        let a = RationalAccumulator::with_remainder(7, 10, 13);
        assert_eq!(a.remainder(), 3, "13 mod 10");
        let b = RationalAccumulator::with_remainder(7, 10, 3);
        assert_eq!(a, b, "and it is the same accumulator");
        let exact = RationalAccumulator::with_remainder(7, 10, 10);
        assert_eq!(exact.remainder(), 0);
    }

    /// `ratio` reports the ratio the accumulator was built with.
    ///
    /// The codec reconstructs from the board's constants and compares against this,
    /// so a state written for one sound clock cannot restore into a board with
    /// another.
    #[test]
    fn the_ratio_is_reported_as_it_was_given() {
        assert_eq!(
            RationalAccumulator::new(Z80_T_NUM, Z80_T_DEN).ratio(),
            (715_909, 3_125)
        );
        // Not the Z80's, so a `ratio` returning the board's constants fails.
        assert_eq!(RationalAccumulator::new(7, 2).ratio(), (7, 2));
        assert_eq!(
            RationalAccumulator::with_remainder(7, 2, 1).ratio(),
            (7, 2),
            "and a carried remainder does not change it"
        );
    }
}
