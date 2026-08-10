//! The noise generator: a 17-bit LFSR and the divider that samples it.
//!
//! # The LFSR is not where you would put it
//!
//! ymfm holds the shift register in **bits 8-24** of a 32-bit word, not the low 17
//! (`ymfm_opm.cpp:186`: "the low 8 bits are the most recent 8 bits of history while
//! bits 8-24 contain the 17 bit LFSR state"). The taps are read at bits 17 and 14,
//! which are bits 9 and 6 *of the register*. Those low 8 bits of history are not
//! decoration: `clock_noise_and_lfo` reads eight of them at once
//! (`bitfield(m_noise_lfsr, 17, 8)`) as LFO waveform 3's sample, so a port that
//! stored a bare 17-bit register would have no bits to hand the LFO. This is why
//! [`Noise::lfsr`] is a `u32` and [`Noise::register`] does the extraction.
//!
//! # Two shifts per clock, and the counter is not a divider
//!
//! One call to [`Noise::clock`] performs **two** shifts, because the noise clock is
//! measured at twice the FM rate (`ymfm_opm.cpp:176`). The counter compares
//! `counter++ >= period` rather than counting down, so `period == 0` latches on
//! every shift — the *fastest* setting. And `period` is the register field
//! **inverted** ([`crate::regs::Regs::noise_period`]), so the fastest setting is
//! frequency field 31, not 0. Both inversions have to be right for a note to sit at
//! the right pitch; getting one right and one wrong gives a plausible-sounding
//! wrong answer.

/// The noise generator's state: the shift register and the sampling counter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Noise {
    /// ymfm's `m_noise_lfsr`: the 17-bit register in bits 8-24, history in bits 0-7.
    pub lfsr: u32,
    /// Shifts since the last latch, compared against the period.
    pub counter: u32,
    /// The latched output bit the operator reads.
    pub state: u32,
}

impl Default for Noise {
    fn default() -> Self {
        Self::new()
    }
}

impl Noise {
    /// A noise generator in its post-reset state.
    ///
    /// `lfsr` starts at 1, matching `opm_registers`' constructor
    /// (`ymfm_opm.cpp:46`). Any non-zero seed enters the same 131,071-state cycle,
    /// but the *phase* within it is what the vector suite pins, so the seed is not
    /// a free choice.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lfsr: 1,
            counter: 0,
            state: 0,
        }
    }

    /// Return to the post-reset state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Appends the noise generator to a save state, in
    /// [`crate::state::NOISE_BYTES`] bytes.
    pub fn write_state(&self, w: &mut crate::state::StateWriter<'_>) {
        w.u32(self.lfsr);
        w.u32(self.counter);
        w.u32(self.state);
    }

    /// A noise generator read back from a save state.
    ///
    /// The LFSR is restored as written, including a zero. Zero is a state the real
    /// register cannot reach — it is a lock-up — but it can only arrive here from a
    /// damaged file the frontend's CRC-32 has already refused, and substituting the
    /// seed would be a decoder quietly changing the state it was given.
    #[must_use]
    pub fn read_state(r: &mut crate::state::StateReader<'_>) -> Self {
        Self {
            lfsr: r.u32(),
            counter: r.u32(),
            state: r.u32(),
        }
    }

    /// The 17-bit shift register, extracted from its window in [`Noise::lfsr`].
    #[must_use]
    pub fn register(&self) -> u32 {
        (self.lfsr >> 8) & 0x1_FFFF
    }

    /// The latched output bit, 0 or 1.
    #[must_use]
    pub fn state(&self) -> u32 {
        self.state
    }

    /// The eight most recent output bits, LFO waveform 3's sample.
    ///
    /// `bitfield(m_noise_lfsr, 17, 8)` — the same bit position the tap is read
    /// from, taken eight wide. See [`crate::lfo::Lfo::clock`], which writes this one
    /// entry ahead of the LFO's position.
    #[must_use]
    pub fn lfo_byte(&self) -> u32 {
        (self.lfsr >> 17) & 0xFF
    }

    /// Advance one FM sample: two shifts, latching when the counter reaches `period`.
    ///
    /// `period` is [`crate::regs::Regs::noise_period`] — the *inverted* frequency
    /// field. Returns the latched state, which may be unchanged.
    pub fn clock(&mut self, period: u32) -> u32 {
        for _ in 0..2 {
            // The feedback bit is forced to 1 when the taps agree, which is what
            // keeps the all-zero state out of the cycle: `x ^ x ^ 1 == 1`.
            self.lfsr <<= 1;
            self.lfsr |= ((self.lfsr >> 17) & 1) ^ ((self.lfsr >> 14) & 1) ^ 1;
            if self.counter >= period {
                self.counter = 0;
                self.state = (self.lfsr >> 17) & 1;
            } else {
                self.counter += 1;
            }
        }
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The LFSR's period is 2^17 - 1, measured rather than asserted from the code.
    ///
    /// A 16-bit or 18-bit tap set gives a different period and a broken tap gives a
    /// short one, so this number is what pins the tap positions. Two details the
    /// plan got wrong and this test has to respect:
    ///
    /// 1. **The register is bits 8-24**, so the state must be compared through
    ///    [`Noise::register`]. Comparing `lfsr` whole compares 15 bits of shift
    ///    history too, and the loop then reports a period of 1 — the low bits
    ///    return to their start value almost immediately.
    /// 2. **The window has to be warmed up first.** From the seed `lfsr == 1` the
    ///    register reads 0 and stays 0 for the first eight shifts, so a measurement
    ///    that starts at the seed terminates on the second clock. 32 clocks of
    ///    warm-up puts a real state in the window.
    ///
    /// The period is in *clocks* rather than shifts only because a clock is two
    /// shifts and 131,071 is odd: 131,071 clocks is 262,142 shifts, two full cycles
    /// of the register.
    #[test]
    fn the_lfsr_period_is_one_hundred_thirty_one_thousand_and_seventy_one() {
        let mut n = Noise::new();
        // Frequency field 31 gives period 0, which is the *fastest*: the counter
        // compares `>=`, so 0 latches on every shift.
        for _ in 0..32 {
            n.clock(0);
        }
        let start = n.register();
        assert_ne!(start, 0, "the warm-up put a real state in the window");
        let mut steps = 0u32;
        loop {
            n.clock(0);
            steps += 1;
            if n.register() == start || steps > 200_000 {
                break;
            }
        }
        assert_eq!(steps, 131_071, "2^17 - 1");
    }

    /// The LFSR reaches all-zero and shifts out of it; all-ones is the lock-up.
    ///
    /// The plan asserted the opposite — that all-zero locks up — which is the OPN
    /// and OPL convention. This LFSR's feedback is `tap ^ tap ^ 1`, so agreeing taps
    /// force a 1 in: from all-zero the next state is 1, and it is **all-ones** that
    /// is the fixed point. Both halves are asserted here because "never reaches the
    /// lock-up state" is only meaningful alongside a demonstration of which state
    /// that is.
    #[test]
    fn the_lock_up_state_is_all_ones_and_the_run_never_reaches_it() {
        let mut n = Noise::new();
        let mut saw_zero = false;
        for _ in 0..200_000 {
            n.clock(0);
            assert_ne!(n.register(), 0x1_FFFF, "all-ones would never shift again");
            saw_zero |= n.register() == 0;
        }
        assert!(saw_zero, "all-zero is inside the cycle, not outside it");

        // And all-ones really is the fixed point, so the assertion above is not
        // guarding a state that could not have occurred anyway.
        let mut stuck = Noise {
            lfsr: (0x1_FFFF << 8) | 0xFF,
            counter: 0,
            state: 0,
        };
        stuck.clock(0);
        assert_eq!(stuck.register(), 0x1_FFFF, "all-ones is a fixed point");
    }

    /// A shorter period latches more often, and the counter is not a countdown.
    ///
    /// Measured by counting *latched output* changes over a fixed window, not by
    /// reading the divider. The register shifts on every clock regardless of period
    /// — that is the "clocked continually and just sampled" behaviour ymfm's comment
    /// describes — so counting register changes would give the same number for
    /// every setting and could not fail. This test asserts both halves: the latch
    /// rate varies and the shift rate does not.
    #[test]
    fn the_period_divides_the_latch_rate_but_not_the_shift_rate() {
        let mut latches = vec![];
        let mut shifts = vec![];
        for period in [31u32, 15, 0] {
            let mut n = Noise::new();
            let (mut prev_state, mut prev_reg) = (n.state(), n.register());
            let (mut latched, mut shifted) = (0, 0);
            for _ in 0..4096 {
                n.clock(period);
                if n.state() != prev_state {
                    latched += 1;
                    prev_state = n.state();
                }
                if n.register() != prev_reg {
                    shifted += 1;
                    prev_reg = n.register();
                }
            }
            latches.push(latched);
            shifts.push(shifted);
        }
        assert!(
            latches[0] < latches[1] && latches[1] < latches[2],
            "a shorter period latches more often: {latches:?}"
        );
        assert_eq!(
            shifts[0], shifts[2],
            "the register shifts at the same rate regardless: {shifts:?}"
        );
    }

    /// One clock is two shifts, because the noise clock runs at twice the FM rate.
    ///
    /// A port that shifted once per clock would halve the noise pitch. The register
    /// advances two positions, which this test detects by comparing against a
    /// single-shift reference built from the same feedback expression.
    #[test]
    fn one_clock_advances_the_register_by_two_shifts() {
        let mut n = Noise::new();
        for _ in 0..32 {
            n.clock(0);
        }
        let before = n.lfsr;
        n.clock(0);
        let one_shift = {
            let mut l = before << 1;
            l |= ((l >> 17) & 1) ^ ((l >> 14) & 1) ^ 1;
            l
        };
        let two_shifts = {
            let mut l = one_shift << 1;
            l |= ((l >> 17) & 1) ^ ((l >> 14) & 1) ^ 1;
            l
        };
        assert_ne!(n.lfsr, one_shift, "not one shift per clock");
        assert_eq!(n.lfsr, two_shifts, "two shifts per clock");
    }

    /// The LFO's noise byte is the tap window widened, so it tracks the output.
    ///
    /// It is read at the same bit position the tap is, taken eight wide, which makes
    /// two relations checkable without re-deriving the LFSR: at the fastest setting
    /// the latched state is the byte's bit 0, and one clock slides the byte by
    /// exactly two bits. A port that handed the LFO the register's low byte, or the
    /// register itself, satisfies neither.
    #[test]
    fn the_lfo_byte_is_the_tap_window_widened() {
        let mut n = Noise::new();
        for _ in 0..32 {
            n.clock(0);
        }
        let mut seen = 0u32;
        for _ in 0..4096 {
            let before = n.lfo_byte();
            let state = n.clock(0);
            let after = n.lfo_byte();
            assert_eq!(state, after & 1, "period 0 latches the byte's newest bit");
            assert_eq!(
                (after >> 2) & 0x3F,
                before & 0x3F,
                "one clock slides the byte two bits"
            );
            seen |= 1 << (after & 1);
        }
        assert_eq!(
            seen, 0b11,
            "both output values occur, so bit 0 is not constant"
        );
    }
}
