//! The low-frequency oscillator: one counter, four waveforms, two outputs.
//!
//! # The waveform table is built, not sampled
//!
//! ymfm precomputes all four waveforms at construction (`ymfm_opm.cpp:57`) with AM
//! packed into the low 8 bits and PM into the upper 8, *signed*. This port keeps
//! that layout — [`WAVEFORM`] holds `(am, pm)` pairs — because the packing is where
//! the shapes come from: triangle's PM is its own AM conditionally inverted
//! (`pm = bitfield(index, 6) ? am : ~am`), which is not a relation you would write
//! if you generated the two outputs independently.
//!
//! Waveform 3 is noise and cannot be precomputed. It is written **one entry ahead**
//! of the current position each clock, which is what latches a stable value for a
//! full LFO step rather than re-reading the LFSR mid-step.
//!
//! # The rate is a 4.4 float, not a divider
//!
//! `counter += (0x10 | rate_low_4) << rate_high_4` — an implied leading 1 with the
//! low nibble as mantissa and the high nibble as exponent. The waveform position is
//! bits 22-29 of the counter. A plain divider is the natural reading and it gives
//! wrong rates everywhere except the sixteen values where the two happen to agree.
//!
//! # Reset holds the counter, and that is not the same as holding the output
//!
//! `0x01` bit 1 zeroes the counter every clock while set (`ymfm_opm.cpp:206`), so
//! the position is pinned at index 0 — but index 0's output is not zero. Measured:
//! AM at index 0 is 253 for saw, 253 for square, and 252 for triangle at full
//! depth. The observable property is that the output is **constant**, which is what
//! [`Lfo`]'s test asserts; the plan asserted zero and would have failed.

/// The AM and PM values for each of the three static waveforms, by position.
///
/// Index 0 is sawtooth, 1 square, 2 triangle. AM is unsigned 0-255 and PM is signed
/// −128..127, both before depth scaling. Waveform 3 lives in [`Lfo::noise_waveform`]
/// because it is filled in as the noise generator runs.
pub static WAVEFORM: [[(u8, i8); 256]; 3] = build_waveforms();

/// Build the three static waveforms exactly as `opm_registers`' constructor does.
const fn build_waveforms() -> [[(u8, i8); 256]; 3] {
    let mut waves = [[(0u8, 0i8); 256]; 3];
    let mut index = 0usize;
    while index < 256 {
        let i = index as u8;

        // Waveform 0 is a sawtooth: AM ramps down as PM ramps up.
        waves[0][index] = (i ^ 0xFF, i as i8);

        // Waveform 1 is a square wave. PM is AM with its top bit flipped, which is
        // what makes PM's two values symmetric about zero while AM's are not.
        let am = if i & 0x80 != 0 { 0 } else { 0xFF };
        waves[1][index] = (am, (am ^ 0x80) as i8);

        // Waveform 2 is a triangle. The `<< 1` is why its AM only takes even
        // values — 128 distinct rather than saw's 254 — and bit 6 selecting between
        // `am` and `!am` is what turns PM around twice per period.
        let am = if i & 0x80 != 0 {
            i.wrapping_shl(1)
        } else {
            (i ^ 0xFF).wrapping_shl(1)
        };
        let pm = if i & 0x40 != 0 { am } else { !am };
        waves[2][index] = (am, pm as i8);

        index += 1;
    }
    waves
}

/// The LFO's state: its phase accumulator and the noise waveform it fills in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Lfo {
    /// The phase accumulator. Bits 22-29 are the waveform position.
    pub counter: u32,
    /// Waveform 3, written one entry ahead of the position each clock.
    pub noise_waveform: [(u8, i8); 256],
    /// The AM value the last clock produced, for [`Lfo::am_offset`].
    pub am: u32,
}

impl Default for Lfo {
    fn default() -> Self {
        Self::new()
    }
}

impl Lfo {
    /// An LFO in its post-reset state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            counter: 0,
            noise_waveform: [(0, 0); 256],
            am: 0,
        }
    }

    /// Return to the post-reset state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The waveform position, bits 22-29 of the counter.
    #[must_use]
    pub fn position(&self) -> u32 {
        (self.counter >> 22) & 0xFF
    }

    /// Advance one sample and return `(am, pm)` after depth scaling.
    ///
    /// `noise_byte` is [`crate::noise::Noise::lfo_byte`] — waveform 3's sample,
    /// which must be the value from the *same* clock as this call, so the caller
    /// clocks the noise generator first.
    ///
    /// The registers are re-read every call rather than cached because a driver can
    /// change the waveform or a depth mid-period and the reference does not latch
    /// them.
    pub fn clock(&mut self, regs: &crate::regs::Regs, noise_byte: u32) -> (u32, i32) {
        let rate = regs.lfo_rate();
        let step = (0x10 | (rate & 0xF)) << ((rate >> 4) & 0xF);
        self.counter = self.counter.wrapping_add(step);

        // The reset bit zeroes the counter *after* the step, every clock it is set,
        // so holding it pins the position at 0 rather than freezing wherever the
        // counter happened to be.
        if regs.lfo_reset() {
            self.counter = 0;
        }

        let position = self.position();

        // Fill waveform 3 one entry ahead so the current value stays stable for a
        // full LFO step. AM and PM get the same byte, so waveform 3's PM is its AM
        // reinterpreted as signed.
        let byte = (noise_byte & 0xFF) as u8;
        self.noise_waveform[((position + 1) & 0xFF) as usize] = (byte, byte as i8);

        let (am, pm) = if regs.lfo_waveform() == 3 {
            self.noise_waveform[position as usize]
        } else {
            WAVEFORM[regs.lfo_waveform() as usize][position as usize]
        };

        self.am = (u32::from(am) * regs.lfo_am_depth()) >> 7;
        let pm = (i32::from(pm) * i32::try_from(regs.lfo_pm_depth()).unwrap_or(0)) >> 7;
        (self.am, pm)
    }

    /// The AM attenuation this channel sees, given its AM sensitivity.
    ///
    /// `ymfm_opm.cpp:236`. Sensitivity 0 is no AM at all; 1, 2, and 3 shift the
    /// stored AM by 0, 1, and 2. The **`sens - 1`** is why sensitivity is not simply
    /// a shift amount: a port that shifted by `sens` would make every setting one
    /// step too loud and setting 0 identical to setting 1.
    #[must_use]
    pub fn am_offset(&self, regs: &crate::regs::Regs, ch: u32) -> u32 {
        let sens = regs.ch_lfo_am_sens(ch);
        if sens == 0 {
            0
        } else {
            self.am << (sens - 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regs::Regs;

    /// Full rate, full depths, and one selected waveform.
    fn regs_for(wave: u8) -> Regs {
        let mut r = Regs::new();
        r.write(0x18, 0xFF); // fastest rate
        r.write(0x19, 0x7F); // AM depth 127 (bit 7 clear)
        r.write(0x19, 0xFF); // PM depth 127 (bit 7 set)
        r.write(0x1B, wave);
        r
    }

    /// Collect `n` samples of `(am, pm)`.
    fn run(regs: &Regs, n: usize) -> (Vec<u32>, Vec<i32>) {
        let mut lfo = Lfo::new();
        let mut ams = Vec::with_capacity(n);
        let mut pms = Vec::with_capacity(n);
        for _ in 0..n {
            let (am, pm) = lfo.clock(regs, 0);
            ams.push(am);
            pms.push(pm);
        }
        (ams, pms)
    }

    fn distinct<T: Ord + Copy>(v: &[T]) -> usize {
        let mut s = v.to_vec();
        s.sort_unstable();
        s.dedup();
        s.len()
    }

    /// The four waveforms have the shapes their names claim.
    ///
    /// **The plan's discriminator does not work.** It counted direction changes in
    /// PM and asserted triangle turns more often than saw; measured over 2,048
    /// samples they are 8 and 8 — indistinguishable, because saw's wrap counts as a
    /// turn just as triangle's peak does. These are the three properties that do
    /// separate them, each measured before being written down:
    ///
    /// - **Direction bias.** Saw's PM rises in 490 of its 492 changes (0.996); the
    ///   two exceptions are its wraps. Triangle's rises in 238 of 494 (0.482) — it
    ///   spends half its time falling. This is the property "turns more often" was
    ///   reaching for.
    /// - **AM resolution.** Triangle's AM is built with a `<< 1` so it takes only
    ///   even values: 128 distinct against saw's 254.
    /// - **Square takes two values** in both outputs, and its PM's are symmetric
    ///   about zero (±126 at full depth) while its AM's are not.
    #[test]
    fn the_four_waveforms_have_distinguishable_shapes() {
        let (saw_am, saw_pm) = run(&regs_for(0), 2048);
        let (sq_am, sq_pm) = run(&regs_for(1), 2048);
        let (tri_am, tri_pm) = run(&regs_for(2), 2048);

        assert_eq!(distinct(&sq_pm), 2, "square PM takes exactly two values");
        assert_eq!(distinct(&sq_am), 2, "and so does its AM");
        assert_eq!(sq_pm.iter().min().unwrap(), &-127);
        assert_eq!(sq_pm.iter().max().unwrap(), &126);

        assert_eq!(distinct(&saw_pm), 254, "saw sweeps the full PM range");
        assert_eq!(distinct(&tri_pm), 254, "and so does triangle");
        assert_eq!(distinct(&saw_am), 254, "saw's AM has full resolution");
        assert_eq!(distinct(&tri_am), 128, "triangle's AM is even-valued only");

        let up_fraction = |v: &[i32]| {
            let changes: Vec<i32> = v
                .windows(2)
                .map(|w| w[1] - w[0])
                .filter(|&d| d != 0)
                .collect();
            let up = changes.iter().filter(|&&d| d > 0).count();
            (up, changes.len())
        };
        let (saw_up, saw_changes) = up_fraction(&saw_pm);
        let (tri_up, tri_changes) = up_fraction(&tri_pm);
        assert!(
            saw_up * 100 > saw_changes * 95,
            "saw PM only ever rises, bar the wrap: {saw_up}/{saw_changes}"
        );
        assert!(
            tri_up * 100 > tri_changes * 40 && tri_up * 100 < tri_changes * 60,
            "triangle PM falls as often as it rises: {tri_up}/{tri_changes}"
        );
    }

    /// Waveform 3 follows the noise byte and is not one of the static three.
    ///
    /// The plan had no test for it, and the natural bug — indexing [`WAVEFORM`] with
    /// 3 and reading past the end, or falling back to waveform 0 — is invisible to a
    /// test that only checks that the value changes. This feeds a counter as the
    /// noise byte so the expected output is known, and it also pins the
    /// *one-ahead* write: the value read at a position is the byte supplied on the
    /// clock before the position advanced to it.
    #[test]
    fn the_noise_waveform_is_written_one_entry_ahead() {
        let regs = regs_for(3);
        let mut lfo = Lfo::new();
        let mut supplied: Vec<u32> = vec![];
        let mut positions = vec![];
        let mut ams = vec![];
        for i in 0..512u32 {
            let byte = i & 0xFF;
            supplied.push(byte);
            let (am, _) = lfo.clock(&regs, byte);
            positions.push(lfo.position());
            ams.push(am);
        }
        // At full AM depth, `am == (byte * 127) >> 7`, i.e. byte - 1 for byte > 0.
        // Find a clock where the position advanced and check it reads the byte
        // written one clock earlier — not the byte supplied on this clock.
        let mut checked = 0;
        for i in 1..512 {
            if positions[i] != positions[i - 1] {
                let expected = (supplied[i - 1] * 127) >> 7;
                assert_eq!(
                    ams[i], expected,
                    "position {} reads the previous clock's byte",
                    positions[i]
                );
                checked += 1;
            }
        }
        assert!(checked > 8, "the position advanced enough times: {checked}");
    }

    /// AM is unsigned and PM is signed — they are not one value scaled.
    ///
    /// A single "LFO output" reused for both is the natural simplification and it is
    /// wrong: AM only attenuates (0 upward) while PM detunes both ways.
    #[test]
    fn am_is_unsigned_and_pm_is_signed() {
        let (ams, pms) = run(&regs_for(2), 4096);
        assert!(*pms.iter().min().unwrap() < 0, "PM goes negative");
        assert!(*pms.iter().max().unwrap() > 0, "and positive");
        assert_eq!(
            *ams.iter().min().unwrap(),
            0,
            "AM bottoms out at no attenuation"
        );
        assert!(*ams.iter().max().unwrap() > 0, "and rises from there");
    }

    /// Zero depth means zero modulation, for both AM and PM.
    #[test]
    fn zero_depth_produces_no_modulation() {
        let mut r = Regs::new();
        r.write(0x18, 0xFF);
        r.write(0x1B, 2);
        r.write(0x19, 0x00); // AM depth 0
        r.write(0x19, 0x80); // PM depth 0
        let mut lfo = Lfo::new();
        for _ in 0..4096 {
            assert_eq!(lfo.clock(&r, 0), (0, 0));
        }
    }

    /// `0x01` bit 1 holds the LFO's *position*, so the output goes constant.
    ///
    /// **The plan asserted the output goes to zero**, which is wrong for every
    /// waveform: with the counter pinned at 0 the measured AM at full depth is 253
    /// (saw), 253 (square), 252 (triangle) — the counter is held, not the output.
    /// Asserting zero would have failed on the first run; asserting "constant" is
    /// the observable property, and it still separates a hold from a one-shot edge,
    /// which is the bug the test exists to catch.
    #[test]
    fn the_lfo_reset_bit_holds_rather_than_pulsing() {
        for wave in 0..3u8 {
            let mut r = regs_for(wave);
            let mut lfo = Lfo::new();
            // 2,048 clocks, not the plan's 500. At the fastest rate the counter
            // steps 1,015,808, so a position lasts 4 clocks and a full period is
            // 1,057 — and 500 clocks only reaches position 145, which is inside
            // *one* half of the square wave. Square's output would be constant over
            // that window and this premise would pass vacuously for two waveforms
            // and fail for the third, which is exactly what it did.
            let mut running = vec![];
            for _ in 0..2048 {
                running.push(lfo.clock(&r, 0));
            }
            assert!(
                distinct(&running) > 1,
                "waveform {wave} is moving before the hold"
            );

            r.write(0x01, 0x02);
            let held = lfo.clock(&r, 0);
            assert_eq!(lfo.position(), 0, "the position is pinned at zero");
            for _ in 0..500 {
                assert_eq!(lfo.clock(&r, 0), held, "constant while the bit is set");
            }
            assert_ne!(held, (0, 0), "and constant is not silent: {held:?}");

            r.write(0x01, 0x00);
            let mut moved = false;
            for _ in 0..2048 {
                if lfo.clock(&r, 0) != held {
                    moved = true;
                    break;
                }
            }
            assert!(moved, "waveform {wave} runs again once cleared");
        }
    }

    /// The rate is a 4.4 float: the mantissa scales and the exponent doubles.
    ///
    /// A plain divider agrees with this at some rates and not others, so a test that
    /// only checked "a higher rate is faster" would pass on the wrong reading. These
    /// two relations pin the decode: raising the exponent nibble by one exactly
    /// doubles the counter step, and the mantissa contributes with an implied
    /// leading 1 — so rate `0x00` steps 16, not 0.
    #[test]
    fn the_rate_is_a_four_point_four_step_with_an_implied_leading_one() {
        let step_for = |rate: u8| {
            let mut r = Regs::new();
            r.write(0x18, rate);
            let mut lfo = Lfo::new();
            lfo.clock(&r, 0);
            lfo.counter
        };
        assert_eq!(
            step_for(0x00),
            0x10,
            "rate 0 still advances: implied leading 1"
        );
        assert_eq!(step_for(0x0F), 0x1F, "the low nibble is the mantissa");
        for exponent in 0..15u8 {
            let lo = step_for(exponent << 4);
            let hi = step_for((exponent + 1) << 4);
            assert_eq!(
                hi,
                lo * 2,
                "exponent {exponent} to {} doubles",
                exponent + 1
            );
        }
    }

    /// AM sensitivity shifts by `sens - 1`, and 0 means no AM at all.
    ///
    /// The off-by-one is the whole point: a port shifting by `sens` makes settings 0
    /// and 1 identical and every other setting one step too loud. Sensitivity 0 has
    /// to be a separate case rather than a shift of −1.
    #[test]
    fn am_sensitivity_zero_is_off_and_the_rest_shift_by_one_less() {
        let mut r = regs_for(0);
        let mut lfo = Lfo::new();
        for _ in 0..37 {
            lfo.clock(&r, 0);
        }
        let raw = lfo.am;
        assert!(raw > 0, "there is an AM value to scale");
        for sens in 0..4u8 {
            r.write(0x38, sens);
            let got = lfo.am_offset(&r, 0);
            let want = if sens == 0 { 0 } else { raw << (sens - 1) };
            assert_eq!(got, want, "sensitivity {sens}");
        }
        r.write(0x38, 0);
        assert_eq!(lfo.am_offset(&r, 0), 0, "sensitivity 0 is silent AM");
        r.write(0x38, 1);
        assert_eq!(lfo.am_offset(&r, 0), raw, "sensitivity 1 is unshifted");
    }
}
