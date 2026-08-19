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

/// SF1's 68000 crystal — `sf.cpp:751`, `M68000(config, m_maincpu, XTAL(8'000'000))`.
///
/// Note this is the same 8 MHz as CPS-1's *pixel* clock ([`PIXEL_CLOCK`]) and
/// serves a completely different purpose. SF1's 68000 is slower than CPS-1's
/// 10 MHz one.
pub const SF1_CPU_HZ: u32 = 8_000_000;

/// SF1's raster height — `sf.cpp:768`, `set_size(64*8, 32*8)`.
pub const SF1_VTOTAL: u32 = 256;

/// The scanline SF1 asserts its vblank interrupt on: the first line past the
/// visible area, from `set_visarea(8*8, (64-8)*8-1, 2*8, 30*8-1)`.
pub const SF1_VBSTART: u32 = 240;

/// SF1's nominal refresh — `sf.cpp:766`, `set_refresh_hz(60)`.
///
/// ⚠️ **Asserted, not derived.** `set_vblank_time(ATTOSECONDS_IN_USEC(0))` sets
/// `m_oldstyle_vblank_supplied` (`screen.h:272`), so `screen.cpp:1001-1005`
/// takes the vblank period as zero and the frame period as exactly this. A real
/// board's refresh comes from its dot clock; nobody measured this one. Every
/// fraction in [`Timing::sf1_8mhz`] follows from this number, so if it is wrong
/// they are all wrong together — the board would run uniformly fast rather than
/// drifting internally.
pub const SF1_REFRESH_HZ: u32 = 60;

/// The ADPCM Z80's periodic interrupt rate — `sf.cpp:761`,
/// `set_periodic_int(FUNC(sf_state::irq0_line_hold), attotime::from_hz(8000))`.
///
/// ⚠️ MAME's own comment on that line is `// ?`. This is what paces ADPCM
/// playback, and no test in this workspace can distinguish 8,000 from 8,192.
pub const SF1_ADPCM_IRQ_HZ: u32 = 8_000;

/// SF1's scanlines per second: the refresh times the line count.
///
/// Not the pixel clock over the raster width — that reading gives 61.035 Hz,
/// which is not the rate MAME configures. See [`SF1_REFRESH_HZ`].
#[must_use]
pub const fn sf1_line_rate() -> u32 {
    SF1_REFRESH_HZ * SF1_VTOTAL
}

/// SF1's Z80 T-states per scanline, as a reduced ratio.
///
/// ⚠️ **Not CPS-1's [`Z80_T_NUM`]/[`Z80_T_DEN`].** Both boards clock their Z80s
/// at [`SOUND_XTAL`], so the numerators match after reduction and copying the
/// wrong constant looks correct. The denominators differ because the line rates
/// do: 3,072 here against CPS-1's 3,125, a 1.7% difference.
#[must_use]
pub const fn sf1_z80_t_per_line() -> (u32, u32) {
    // 3_579_545 / 15_360, reduced by 5.
    (715_909, 3_072)
}

/// The ADPCM Z80's interrupts per scanline, as a reduced ratio.
///
/// 8,000 / 15,360, reduced by 320. See [`SF1_ADPCM_IRQ_HZ`] for the `// ?`.
#[must_use]
pub const fn sf1_adpcm_irq_per_line() -> (u32, u32) {
    (25, 48)
}

/// Input clocks per YM2151 sample. Exactly 64: the chip divides by 2 then by 32.
///
/// This duplicates `ym2151::Ym2151::sample_clocks()`. The copy exists because this
/// module predates `machine`'s dependency on `ym2151`, and the two are held together
/// by `the_sample_rate_is_fractional_per_line_and_per_frame`, which asserts they
/// agree: a second literal that silently disagreed with the chip would put every
/// sample in the wrong place, and nothing else here would notice.
pub const YM_SAMPLE_CLOCKS: u32 = 64;

/// The MSM6295's crystal on CPS-1: 1 MHz.
pub const OKI_XTAL: u32 = 1_000_000;

/// OKI input clocks in one scanline.
///
/// Unlike the Z80's, this divides exactly: `1_000_000 / 15_625 = 64` with no
/// remainder. See `the_oki_clock_divides_into_a_scanline_exactly`.
pub const OKI_CLOCKS_PER_LINE: u32 = 64;

/// The MSM6295 divides its clock by 132 with pin 7 high (`okim6295.cpp`).
pub const OKI_DIV_PIN7_HIGH: u32 = 132;

/// ...and by 165 with pin 7 low.
pub const OKI_DIV_PIN7_LOW: u32 = 165;

/// The denominator both pin-7 ratios share.
///
/// A shared denominator makes a pin-7 write a numerator swap: the remainder
/// already carried keeps its units, so the phase does not jump.
pub const OKI_PER_YM_DEN: u32 = 23_624_997;

/// OKI samples per YM sample with pin 7 high.
pub const OKI_PER_YM_NUM_PIN7_HIGH: u32 = 3_200_000;

/// OKI samples per YM sample with pin 7 low.
pub const OKI_PER_YM_NUM_PIN7_LOW: u32 = 2_560_000;

/// The OKI-samples-per-YM-sample ratio for a pin-7 state.
///
/// The mix is driven off the YM tick because both rates are **under one sample
/// per scanline** — 16/33 and 64/165 — so a per-line loop could not place them.
/// See `there_is_less_than_one_oki_sample_per_scanline`.
#[must_use]
pub const fn oki_per_ym(pin7: bool) -> (u32, u32) {
    let num = if pin7 {
        OKI_PER_YM_NUM_PIN7_HIGH
    } else {
        OKI_PER_YM_NUM_PIN7_LOW
    };
    (num, OKI_PER_YM_DEN)
}

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
    /// 68000 cycles per scanline, as a reduced `(numerator, denominator)`.
    ///
    /// # Why a ratio and not a count
    ///
    /// CPS-1 derives its refresh from the pixel clock, so 10 MHz over 15,625
    /// lines per second is exactly 640 and the denominator is 1. SF1 asserts a
    /// round 60 Hz refresh (`sf.cpp:766`), so 8 MHz over 15,360 lines per second
    /// is 3125/6 — and rounding that to 520 or 521 is a 0.16% error, which is
    /// the drift this module's header describes: audible over a match, never
    /// broken enough to investigate.
    ///
    /// The **remainder is not here.** [`Timing`] is `Copy` configuration, and a
    /// moving remainder inside it would let two copies of the same board's
    /// timing disagree about the future. It lives in the machine, beside its
    /// other fractional clocks — see [`RationalAccumulator::ratio`], which
    /// makes the same distinction.
    pub line_cycles: (u32, u32),
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
    /// # Why the division is exact
    ///
    /// 8 MHz / 512 = 15,625 lines per second with no remainder, and
    /// 10 MHz / 15,625 = 640 cycles per line, also with no remainder — so
    /// [`Timing::line_cycles`]'s denominator is 1 and the accumulator never
    /// carries. The 12 MHz variant gives 768, exact as well.
    ///
    /// SF1 is the board this doc used to warn about: see [`Timing::sf1_8mhz`].
    pub const fn cps1_10mhz() -> Self {
        Self {
            cpu_hz: CPU_HZ_10M,
            line_cycles: (640, 1),
            lines_per_frame: VTOTAL,
            vblank_line: VBSTART,
        }
    }

    /// The 8 MHz Street Fighter 1 configuration (`sf.cpp:751-771`).
    ///
    /// 8 MHz over [`sf1_line_rate`]'s 15,360 lines per second is **3125/6**,
    /// which is not an integer. The number 512 is the raster *width*; using it
    /// as a cycle count would silently assume a 61.035 Hz refresh.
    pub const fn sf1_8mhz() -> Self {
        Self {
            cpu_hz: SF1_CPU_HZ,
            // 8_000_000 / 15_360, reduced by 2_560.
            line_cycles: (3125, 6),
            lines_per_frame: SF1_VTOTAL,
            vblank_line: SF1_VBSTART,
        }
    }

    /// 68000 cycles in one frame, floored.
    ///
    /// Exact for a board whose denominator is 1 (CPS-1: 167,680). For SF1 the
    /// true value is 400000/3 and this returns 133,333; the third of a cycle is
    /// carried by the machine's accumulator, not lost — this function is a
    /// reporting figure, and its one caller is an inequality.
    #[must_use]
    pub const fn cycles_per_frame(&self) -> u32 {
        let (num, den) = self.line_cycles;
        num * self.lines_per_frame / den
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

    /// SF1's line rate is the **frame** rate times the line count, not the
    /// pixel clock divided by the raster width.
    ///
    /// MAME asserts 60 Hz (`sf.cpp:766`) and 256 lines (`set_size(64*8, 32*8)`),
    /// so 60 × 256 = 15,360 lines per second. The dot-clock reading would give
    /// 8,000,000 / (512 × 256) = 61.035 Hz, which is not what MAME configures.
    #[test]
    fn sf1_line_rate_is_the_refresh_times_the_line_count() {
        assert_eq!(SF1_REFRESH_HZ, 60, "sf.cpp:766 set_refresh_hz(60)");
        assert_eq!(SF1_VTOTAL, 256, "sf.cpp:768 set_size(64*8, 32*8)");
        assert_eq!(sf1_line_rate(), 15_360, "and the literal");
        assert_ne!(sf1_line_rate(), 15_625, "that is CPS-1's, at 262 lines");
    }

    /// 8 MHz over 15,360 lines/s is 3125/6, and it is not an integer.
    ///
    /// Three independent statements of the same number: the reduced pair as
    /// literals, the unreduced division, and the sum over a whole frame. A
    /// per-line assertion alone cannot catch a wrong reduction.
    #[test]
    fn sf1_line_cycles_are_3125_over_6() {
        let t = Timing::sf1_8mhz();
        assert_eq!(t.cpu_hz, 8_000_000, "sf.cpp:751 XTAL(8'000'000)");
        assert_eq!(t.line_cycles, (3125, 6), "the reduced ratio, as literals");

        let (num, den) = t.line_cycles;
        assert_eq!(num * 15_360, 8_000_000 * den, "same fraction as 8MHz/15360");
        assert_ne!(den, 1, "SF1 does not divide evenly; see cps1_10mhz's doc");

        let mut acc = RationalAccumulator::new(num, den);
        let total: u32 = (0..256).map(|_| acc.advance()).sum();
        assert_eq!(total, 133_333, "floor(8_000_000 / 60)");
        assert_eq!(acc.remainder(), 2, "2/6 = 1/3 of a cycle carried");
    }

    /// CPS-1 keeps its exact 640, now expressed as a ratio with denominator 1.
    #[test]
    fn cps1_line_cycles_are_640_over_1() {
        let t = Timing::cps1_10mhz();
        assert_eq!(t.line_cycles, (640, 1), "exact, so the denominator is 1");
        let mut acc = RationalAccumulator::new(640, 1);
        assert_eq!(acc.advance(), 640);
        assert_eq!(acc.remainder(), 0, "and never carries");
    }

    /// SF1's geometry: 384×224 visible at (64,16), 512×256 raster, vblank at 240.
    ///
    /// The visible window is identical to CPS-1's; the raster is six lines
    /// shorter (256 against 262).
    #[test]
    fn sf1_frame_geometry_is_384x224_at_60_hz() {
        let t = Timing::sf1_8mhz();
        assert_eq!(t.lines_per_frame, 256);
        assert_eq!(
            t.vblank_line, 240,
            "VBSTART, sf.cpp:769 set_visarea 2*8..30*8-1"
        );
        assert_eq!(SF1_VBSTART, 240);
        // The visible window, from set_visarea(8*8, (64-8)*8-1, 2*8, 30*8-1):
        assert_eq!(
            (8 * 8, (64 - 8) * 8 - 1, 2 * 8, 30 * 8 - 1),
            (64, 447, 16, 239)
        );
        assert_eq!(447 - 64 + 1, 384, "visible width");
        assert_eq!(239 - 16 + 1, 224, "visible height");
        // draw_common's extents, which the screen-flip pivots depend on:
        assert_eq!(447 + 64 + 1, 512, "xextent = the raster width exactly");
        assert_eq!(239 + 16 + 1, 256, "yextent = the raster height exactly");
    }

    /// `cycles_per_frame` floors, and both boards' values are literals.
    #[test]
    fn cycles_per_frame_handles_both_denominators() {
        assert_eq!(
            Timing::cps1_10mhz().cycles_per_frame(),
            167_680,
            "640 × 262"
        );
        assert_eq!(
            Timing::sf1_8mhz().cycles_per_frame(),
            133_333,
            "floor(3125 × 256 / 6) = floor(400000/3)"
        );
    }

    /// SF1's Z80 fraction shares CPS-1's numerator and **not** its denominator.
    ///
    /// Both boards clock their Z80s at 3.579545 MHz, so copying CPS-1's
    /// `715_909 / 3_125` looks right and is 1.7% wrong. SF1's line rate is
    /// 15,360, giving 715_909 / 3_072.
    #[test]
    fn sf1_z80_t_states_per_line_is_not_cps1s() {
        assert_eq!(sf1_z80_t_per_line(), (715_909, 3_072));
        assert_eq!(
            (Z80_T_NUM, Z80_T_DEN),
            (715_909, 3_125),
            "CPS-1's, for contrast"
        );

        let (num, den) = sf1_z80_t_per_line();
        // `u64`: 715_909 × 15_360 is 10,996,362,240, which does not fit a `u32` —
        // and `rustc` panics rather than wrapping, which is how this was caught.
        // Same reason as the milli-hertz assertion in
        // `cps1_frame_geometry_is_384x224_at_59_63_hz`.
        assert_eq!(
            u64::from(num) * 15_360,
            u64::from(SOUND_XTAL) * u64::from(den),
            "same fraction as 3579545/15360"
        );
        let mut acc = RationalAccumulator::new(num, den);
        let total: u32 = (0..256).map(|_| acc.advance()).sum();
        assert_eq!(total, 59_659, "floor(3_579_545 / 60)");
    }

    /// The ADPCM Z80's 8 kHz periodic IRQ, as interrupts per scanline.
    ///
    /// `sf.cpp:761` — `set_periodic_int(irq0_line_hold, from_hz(8000)); // ?`.
    /// MAME's own comment records that the rate is a guess; ours does too.
    #[test]
    fn sf1_adpcm_irq_is_25_over_48_per_line() {
        assert_eq!(SF1_ADPCM_IRQ_HZ, 8_000);
        assert_eq!(sf1_adpcm_irq_per_line(), (25, 48));
        let (num, den) = sf1_adpcm_irq_per_line();
        assert_eq!(num * 15_360, SF1_ADPCM_IRQ_HZ * den);
        let mut acc = RationalAccumulator::new(num, den);
        let total: u32 = (0..256).map(|_| acc.advance()).sum();
        assert_eq!(total, 133, "floor(8000 / 60)");
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
        assert_eq!(
            t.line_cycles.0 / t.line_cycles.1,
            CPU_HZ_10M / (PIXEL_CLOCK / HTOTAL)
        );
        assert_eq!(t.line_cycles, (640, 1), "and the literal, both ways");
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
            line_cycles: (768, 1),
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
        // satisfied by `line_cycles.0 * 262` — verified: that mutant survived
        // until this third case existed. A `lines_per_frame` no board uses is what
        // makes the second operand load-bearing.
        let odd = Timing {
            cpu_hz: 10_000_000,
            line_cycles: (640, 1),
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
            t.cycles_per_frame() - 224 * t.line_cycles.0 / t.line_cycles.1,
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

    /// The OKI's clock *does* divide into a scanline, exactly -- unlike the
    /// Z80's. Asserted from the pixel clock and the horizontal total, not from
    /// a restatement of the constant.
    #[test]
    fn the_oki_clock_divides_into_a_scanline_exactly() {
        let lines_per_second = PIXEL_CLOCK / HTOTAL;
        assert_eq!(lines_per_second, 15_625, "8 MHz over 512 pixels");
        assert_eq!(OKI_XTAL % lines_per_second, 0, "no remainder to carry");
        assert_eq!(OKI_XTAL / lines_per_second, OKI_CLOCKS_PER_LINE);
    }

    /// The sample-rate ratio, asserted through its derivation.
    ///
    /// `assert_eq!(NUM / DEN, NUM / DEN)` passes for every value including
    /// wrong ones, so instead: cross-multiply the ratio against the two
    /// independently-derived quantities that define it -- the OKI divisor and
    /// the Z80's own T-states-per-line ratio. Both sides come to
    /// 302,399,961,600,000 for each divisor, and the two divisors reaching the
    /// same total is not a coincidence to be tidied away: the ratios differ by
    /// exactly 165:132, so the products cannot differ.
    #[test]
    fn the_oki_to_ym_ratio_is_what_the_two_clocks_imply() {
        for (pin7, div) in [(true, OKI_DIV_PIN7_HIGH), (false, OKI_DIV_PIN7_LOW)] {
            let (num, den) = oki_per_ym(pin7);
            let lhs = u64::from(num) * u64::from(div) * u64::from(Z80_T_NUM);
            let rhs = u64::from(den)
                * u64::from(OKI_CLOCKS_PER_LINE)
                * u64::from(Z80_T_DEN)
                * u64::from(YM_SAMPLE_CLOCKS);
            assert_eq!(
                lhs, rhs,
                "pin7 {pin7}: the ratio does not follow from the clocks"
            );
            assert_eq!(lhs, 302_399_961_600_000, "pin7 {pin7}");
        }
    }

    /// The ratio also follows from the crystals alone, with no board constant
    /// on the expected side.
    ///
    /// The test above cross-multiplies against `OKI_CLOCKS_PER_LINE` and the
    /// Z80's T-state ratio, both of which are this module's own constants. This
    /// one goes back to the two crystals and the two divisors: the OKI's sample
    /// rate is `OKI_XTAL / div` and the YM's is `SOUND_XTAL /
    /// YM_SAMPLE_CLOCKS`, so `num / den` must equal
    /// `OKI_XTAL * YM_SAMPLE_CLOCKS / (div * SOUND_XTAL)`. A ratio that was
    /// internally consistent with a wrong `OKI_CLOCKS_PER_LINE` fails here.
    #[test]
    fn the_ratio_follows_from_the_crystals_alone() {
        for (pin7, div) in [(true, OKI_DIV_PIN7_HIGH), (false, OKI_DIV_PIN7_LOW)] {
            let (num, den) = oki_per_ym(pin7);
            assert_eq!(
                u64::from(num) * u64::from(div) * u64::from(SOUND_XTAL),
                u64::from(den) * u64::from(OKI_XTAL) * u64::from(YM_SAMPLE_CLOCKS),
                "pin7 {pin7}"
            );
            // And the fraction is in lowest terms, so the accumulator's period
            // is as short as the arithmetic allows.
            let gcd = |mut a: u32, mut b: u32| {
                while b != 0 {
                    (a, b) = (b, a % b);
                }
                a
            };
            assert_eq!(gcd(num, den), 1, "pin7 {pin7}: {num}/{den} is reducible");
        }
    }

    /// The two ratios share a denominator, which is what makes a pin-7 write a
    /// numerator swap that keeps the carried remainder's units.
    #[test]
    fn both_pin_seven_states_share_a_denominator() {
        let (hi_num, hi_den) = oki_per_ym(true);
        let (lo_num, lo_den) = oki_per_ym(false);
        assert_eq!(hi_den, lo_den);
        assert_eq!(hi_den, OKI_PER_YM_DEN);
        // Pin 7 high is the faster rate, by exactly the divisor ratio 165:132.
        assert_eq!(
            u64::from(hi_num) * u64::from(OKI_DIV_PIN7_HIGH),
            u64::from(lo_num) * u64::from(OKI_DIV_PIN7_LOW)
        );
        assert!(hi_num > lo_num);
    }

    /// The display rates, derived rather than restated.
    #[test]
    fn the_sample_rates_round_to_the_documented_hertz() {
        // (OKI_XTAL + div/2) / div: round-half-up in integers.
        let round = |div: u32| (OKI_XTAL + div / 2) / div;
        assert_eq!(round(OKI_DIV_PIN7_HIGH), 7576);
        assert_eq!(round(OKI_DIV_PIN7_LOW), 6061);
        // And the exact rates are 250000/33 and 200000/33.
        assert_eq!(OKI_XTAL * 33, 250_000 * OKI_DIV_PIN7_HIGH);
        assert_eq!(OKI_XTAL * 33, 200_000 * OKI_DIV_PIN7_LOW);
    }

    /// Fewer than one OKI sample per scanline at either rate. This is the fact
    /// that rules out mixing per line.
    ///
    /// The exact fractions are 16/33 and 64/165 samples per line, asserted
    /// against the crystal and the line rate rather than against
    /// `OKI_CLOCKS_PER_LINE`. The form `CPL * 165 == 64 * DIV_LOW` also fails
    /// when either constant moves, so it is not vacuous -- but its literals 64
    /// and 165 *are* the two constants, so it reads as a restatement, and it
    /// cannot distinguish a wrong `OKI_CLOCKS_PER_LINE` from a wrong divisor.
    /// Going back to `OKI_XTAL` and the line rate leaves the scanline division
    /// to the test that owns it, above.
    #[test]
    fn there_is_less_than_one_oki_sample_per_scanline() {
        let lines_per_second = PIXEL_CLOCK / HTOTAL;
        for div in [OKI_DIV_PIN7_HIGH, OKI_DIV_PIN7_LOW] {
            let rate = OKI_XTAL / div; // truncating, so a floor
            assert!(
                rate < lines_per_second,
                "div {div}: {rate} >= {lines_per_second}"
            );
        }
        // samples/line = OKI_XTAL / (div * lines_per_second), cross-multiplied
        // against the literal fractions.
        assert_eq!(
            u64::from(OKI_XTAL) * 33,
            16 * u64::from(OKI_DIV_PIN7_HIGH) * u64::from(lines_per_second)
        );
        assert_eq!(
            u64::from(OKI_XTAL) * 165,
            64 * u64::from(OKI_DIV_PIN7_LOW) * u64::from(lines_per_second)
        );
    }

    /// The accumulator at the OKI's ratio emits the count the crystals imply.
    ///
    /// # The expectation comes from the clocks, not from the ratio under test
    ///
    /// The obvious form of this test is `want = num * STEPS / den` for the same
    /// `(num, den)` the accumulator was given. That cannot fail: measured, it
    /// passes for a doubled numerator, a halved one, a numerator off by 1,000,
    /// and the two pin-7 numerators swapped -- every mutation this test exists
    /// to catch, because both sides move together.
    ///
    /// So `want` is computed from the two crystals and the divisor:
    /// `STEPS * OKI_XTAL * YM_SAMPLE_CLOCKS / (div * SOUND_XTAL)`. Measured
    /// against that expectation, the swapped numerators are out by 2,709 and
    /// the off-by-1,000 numerator by 5.
    ///
    /// The tolerance is one sample, for the truncation at each end of a window
    /// that is not a whole period -- a full period is 23,624,997 steps.
    #[test]
    fn the_accumulator_emits_the_ratio_over_its_full_period() {
        const STEPS: u32 = 100_000;
        for (pin7, div) in [(true, OKI_DIV_PIN7_HIGH), (false, OKI_DIV_PIN7_LOW)] {
            let (num, den) = oki_per_ym(pin7);
            let mut acc = RationalAccumulator::new(num, den);
            let emitted: u64 = (0..STEPS).map(|_| u64::from(acc.advance())).sum();
            let want = u64::from(STEPS) * u64::from(OKI_XTAL) * u64::from(YM_SAMPLE_CLOCKS)
                / (u64::from(div) * u64::from(SOUND_XTAL));
            assert!(
                emitted.abs_diff(want) <= 1,
                "pin7 {pin7}: {emitted} emitted over {STEPS}, the clocks imply {want}"
            );
        }
    }

    /// The fraction is *carried*, not dropped: the remainder after a window is
    /// the exact one the arithmetic implies.
    ///
    /// The count test above tolerates one sample, which is what lets it accept
    /// an accumulator that truncates the last partial step. This does not: the
    /// remainder is the accumulated fraction itself, and an implementation that
    /// zeroed it, saturated it, or carried it in the wrong direction lands on a
    /// different value even when the emitted count is within tolerance.
    ///
    /// The expected remainders -- 23,040,632 and 23,157,505 -- are
    /// `num * STEPS % den`, measured. Both are close to `den`, so this window
    /// also happens to end one step short of an emission, which is the state a
    /// save/restore has to preserve exactly.
    #[test]
    fn the_accumulator_carries_the_okis_fraction_rather_than_dropping_it() {
        const STEPS: u32 = 100_000;
        for (pin7, want_rem) in [(true, 23_040_632u32), (false, 23_157_505)] {
            let (num, den) = oki_per_ym(pin7);
            let mut acc = RationalAccumulator::new(num, den);
            for _ in 0..STEPS {
                acc.advance();
            }
            assert_eq!(
                u64::from(want_rem),
                u64::from(num) * u64::from(STEPS) % u64::from(den),
                "pin7 {pin7}: the expected remainder is not what the ratio implies"
            );
            assert_eq!(acc.remainder(), want_rem, "pin7 {pin7}");
            assert!(
                want_rem < den,
                "a remainder at or above den is a whole unit"
            );
        }
    }
}
