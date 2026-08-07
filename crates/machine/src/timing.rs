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
}
