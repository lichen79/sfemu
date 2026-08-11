//! The MSM6295's ADPCM decoder.
//!
//! Transcribed from MAME's `okiadpcm.cpp` (BSD-3, (C) Andrew Gardner and Aaron
//! Giles). The arithmetic is a signal accumulator clamped to `-2048..=2047` and
//! a step index clamped to `0..=48`; a nibble selects both a signed increment
//! and a step-index adjustment.

/// The step index clamps to `0..=48`, so the table holds 49 entries.
pub const STEPS: usize = 49;

/// `floor(16 * 1.1^step)` for `step` in `0..49`, MAME's `okiadpcm.cpp:129`.
///
/// A literal, not a computation: Rust has no `const fn` float `pow`, and the
/// exact integer form `16 * 11^48 / 10^48` needs 171 bits. See
/// `the_step_table_is_the_floor_of_a_ten_percent_geometric_series` for the
/// independent derivation that checks it, and for why the recurrence
/// `v += v / 10` is not an acceptable substitute.
pub const STEP_TABLE: [i16; STEPS] = [
    16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130,
    143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796,
    876, 963, 1060, 1166, 1282, 1411, 1552,
];

/// The signal accumulator's upper clamp (`okiadpcm.cpp:61`).
pub const SIGNAL_MAX: i16 = 2047;
/// The signal accumulator's lower clamp (`okiadpcm.cpp:63`).
pub const SIGNAL_MIN: i16 = -2048;

/// What each nibble does to the step index, MAME's `s_index_shift`
/// (`okiadpcm.cpp:34`). Indexed by `nibble & 7`, so the sign bit does not
/// affect the step.
const INDEX_SHIFT: [i8; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// The signed increment a nibble contributes at a given step index.
///
/// MAME precomputes this as a 784-entry table; the arithmetic is cheap enough
/// to do inline, and doing it inline keeps the 49 stepvals the only literal
/// data in the crate. Bit 3 of the nibble is a sign; bits 2, 1 and 0 weight
/// the step value at 1, 1/2 and 1/4; and there is always an unconditional
/// 1/8 term, which is why nibble 0 is not a no-op.
///
/// Each division truncates independently, exactly as MAME's does -- summing
/// first and dividing once would give different values.
///
/// # Panics
///
/// Panics if `step >= STEPS`. Callers inside this crate clamp first.
#[must_use]
pub fn diff(step: usize, nibble: u8) -> i16 {
    let sv = i32::from(STEP_TABLE[step]);
    let n = i32::from(nibble & 0x0F);
    let mut d = sv / 8;
    if n & 4 != 0 {
        d += sv;
    }
    if n & 2 != 0 {
        d += sv / 2;
    }
    if n & 1 != 0 {
        d += sv / 4;
    }
    if n & 8 != 0 {
        d = -d;
    }
    // The widest value is 2910; see `every_increment_leaves_the_sum_inside_i16`.
    d as i16
}

/// One ADPCM decoder: a signal accumulator and a step index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Adpcm {
    signal: i16,
    step: u8,
}

impl Adpcm {
    /// A decoder at rest: signal 0, step index 0.
    #[must_use]
    pub const fn new() -> Self {
        Self { signal: 0, step: 0 }
    }

    /// Rebuild a decoder from a saved signal and step index, clamping both.
    ///
    /// The clamps are not defensive habit: the values come from a save-state
    /// file, and an out-of-range step index would panic in [`diff`].
    #[must_use]
    pub const fn restore(signal: i16, step: u8) -> Self {
        let signal = if signal > SIGNAL_MAX {
            SIGNAL_MAX
        } else if signal < SIGNAL_MIN {
            SIGNAL_MIN
        } else {
            signal
        };
        let max = (STEPS - 1) as u8;
        Self {
            signal,
            step: if step > max { max } else { step },
        }
    }

    /// Return to the state [`Adpcm::new`] produces.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The current signal, without decoding anything.
    #[must_use]
    pub const fn signal(&self) -> i16 {
        self.signal
    }

    /// The current step index, in `0..=48`.
    #[must_use]
    pub const fn step(&self) -> u8 {
        self.step
    }

    /// Decode one nibble and return the new signal, in `-2048..=2047`.
    pub fn clock(&mut self, nibble: u8) -> i16 {
        let sum = i32::from(self.signal) + i32::from(diff(usize::from(self.step), nibble));
        self.signal = sum.clamp(i32::from(SIGNAL_MIN), i32::from(SIGNAL_MAX)) as i16;

        let step = i32::from(self.step) + i32::from(INDEX_SHIFT[usize::from(nibble & 7)]);
        self.step = step.clamp(0, (STEPS - 1) as i32) as u8;

        self.signal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MAME builds this table with `floor(16.0 * pow(11.0 / 10.0, step))`
    /// (`okiadpcm.cpp:129`). Rust has no `const fn` float `pow`, and the exact
    /// integer form `16 * 11^48 / 10^48` needs 171 bits, so the table is a
    /// literal -- and this test is the second, independent derivation that
    /// proves the literal right.
    ///
    /// The comparison is sound because the values sit clear of `floor()`
    /// boundaries: only step 0 is exactly integral, and the tightest
    /// non-integral margin (step 18, value 88, margin 0.0413) clears f64's
    /// error bound at that magnitude by a factor of 7e11.
    ///
    /// Do NOT replace the literal with the recurrence `v += v / 10`: it
    /// disagrees with MAME at 47 of the 49 entries, starting at step 2
    /// (18 against the correct 19).
    #[test]
    fn the_step_table_is_the_floor_of_a_ten_percent_geometric_series() {
        // Iterating the array rather than `0..STEPS` is clippy's requirement, and
        // it loses nothing: the table's type is `[i16; STEPS]`, so this visits
        // every entry, and the test below pins `STEP_TABLE.len() == STEPS`.
        for (step, &value) in STEP_TABLE.iter().enumerate() {
            let want = (16.0_f64 * (11.0_f64 / 10.0_f64).powi(step as i32)).floor();
            assert_eq!(
                f64::from(value),
                want,
                "step {step} disagrees with floor(16 * 1.1^{step})"
            );
        }
    }

    #[test]
    fn the_step_table_is_strictly_increasing_and_bounded() {
        assert_eq!(STEPS, 49, "the step index clamps to 0..=48, so 49 entries");
        assert_eq!(STEP_TABLE.len(), STEPS);
        assert_eq!(STEP_TABLE[0], 16);
        assert_eq!(STEP_TABLE[STEPS - 1], 1552);
        for step in 1..STEPS {
            assert!(
                STEP_TABLE[step] > STEP_TABLE[step - 1],
                "step {step} did not increase"
            );
        }
    }

    /// The measured first and last rows of MAME's 784-entry `s_diff_lookup`.
    /// Two independent derivations again: these rows come from running MAME's
    /// `compute_tables()` and printing it, not from `diff()`.
    #[test]
    fn the_diff_table_matches_the_rows_mame_computes() {
        let step0 = [
            2, 6, 10, 14, 18, 22, 26, 30, -2, -6, -10, -14, -18, -22, -26, -30,
        ];
        let step48 = [
            194, 582, 970, 1358, 1746, 2134, 2522, 2910, -194, -582, -970, -1358, -1746, -2134,
            -2522, -2910,
        ];
        for nibble in 0..16u8 {
            assert_eq!(
                diff(0, nibble),
                step0[nibble as usize],
                "step 0 nibble {nibble:X}"
            );
            assert_eq!(
                diff(48, nibble),
                step48[nibble as usize],
                "step 48 nibble {nibble:X}"
            );
        }
    }

    /// Bit 3 is a sign bit and nothing else, at every step.
    #[test]
    fn the_high_nibble_bit_negates_exactly() {
        for step in 0..STEPS {
            for nibble in 0..8u8 {
                assert_eq!(
                    diff(step, nibble + 8),
                    -diff(step, nibble),
                    "step {step} nibble {nibble:X}"
                );
            }
        }
    }

    /// The whole table fits i16 with room for the pre-clamp sum: the widest
    /// increment is 2910, and 2047 + 2910 = 4957.
    #[test]
    fn every_increment_leaves_the_sum_inside_i16() {
        let mut widest = 0i16;
        for step in 0..STEPS {
            for nibble in 0..16u8 {
                widest = widest.max(diff(step, nibble).abs());
            }
        }
        assert_eq!(widest, 2910, "the widest increment is step 48 nibble 7");
        assert!(i32::from(SIGNAL_MAX) + i32::from(widest) < i32::from(i16::MAX));
        assert!(i32::from(SIGNAL_MIN) - i32::from(widest) > i32::from(i16::MIN));
    }

    /// Measured from MAME: a single `F` from reset gives signal -30, step 8.
    ///
    /// The D3 spec claims -48 here. It does not reproduce; -30 is what MAME's
    /// own decoder returns, and `diff(0, 0xF)` is -30 by construction
    /// (-(16 + 8 + 4 + 2)).
    #[test]
    fn a_lone_f_from_reset_gives_minus_thirty() {
        let mut a = Adpcm::new();
        assert_eq!(a.clock(0xF), -30);
        assert_eq!(a.step(), 8, "nibble 7 of the shift table adds 8");
    }

    /// Measured from MAME: four consecutive `7`s ramp 30, 93, 229, 522 and
    /// leave the step index at 32. The spec's 15/151/444/1075 does not
    /// reproduce.
    #[test]
    fn four_sevens_ramp_the_way_mame_ramps() {
        let mut a = Adpcm::new();
        let got: Vec<i16> = (0..4).map(|_| a.clock(7)).collect();
        assert_eq!(got, vec![30, 93, 229, 522]);
        assert_eq!(a.step(), 32, "four steps of +8");
    }

    /// Both clamps, and the fact that the signal *stays* pinned: 64 zero
    /// nibbles after saturation drive the step index to 0 while the signal
    /// holds at 2047, because nibble 0 still carries the unconditional
    /// `stepval/8` term.
    #[test]
    fn the_signal_and_step_clamp_independently() {
        let mut a = Adpcm::new();
        for _ in 0..64 {
            a.clock(7);
        }
        assert_eq!((a.signal(), a.step()), (2047, 48), "saturated high");
        for _ in 0..64 {
            a.clock(0);
        }
        assert_eq!(
            (a.signal(), a.step()),
            (2047, 0),
            "the step index bottoms out but the signal stays pinned"
        );

        let mut b = Adpcm::new();
        let mut lowest = 0i16;
        for _ in 0..200 {
            lowest = lowest.min(b.clock(0xF));
        }
        assert_eq!((lowest, b.step()), (-2048, 48), "saturated low");
    }

    #[test]
    fn a_reset_returns_every_field_to_its_initial_value() {
        let mut a = Adpcm::new();
        for _ in 0..40 {
            a.clock(0xF);
        }
        assert_ne!((a.signal(), a.step()), (0, 0), "the test would be vacuous");
        a.reset();
        assert_eq!((a.signal(), a.step()), (0, 0));
        assert_eq!(a, Adpcm::new(), "reset and new must agree");
    }

    /// A restored decoder decodes identically to the one it was copied from --
    /// asserted through the samples it produces, not by comparing the fields
    /// that were just assigned.
    #[test]
    fn a_restored_decoder_produces_the_same_samples() {
        let mut a = Adpcm::new();
        for n in [3u8, 9, 1, 0xE, 7, 7, 2] {
            a.clock(n);
        }
        let mut b = Adpcm::restore(a.signal(), a.step());
        let feed = [5u8, 0xD, 0, 0xF, 8, 4, 4, 1];
        let from_a: Vec<i16> = feed.iter().map(|&n| a.clock(n)).collect();
        let from_b: Vec<i16> = feed.iter().map(|&n| b.clock(n)).collect();
        assert_eq!(from_a, from_b);
        assert!(
            from_a.iter().any(|&s| s != 0),
            "the comparison must be of something"
        );
    }

    /// A step index out of range cannot index the table; `restore` clamps.
    #[test]
    fn a_restore_clamps_a_step_index_the_file_got_wrong() {
        assert_eq!(Adpcm::restore(0, 200).step(), 48);
        assert_eq!(Adpcm::restore(9999, 0).signal(), SIGNAL_MAX);
        assert_eq!(Adpcm::restore(-9999, 0).signal(), SIGNAL_MIN);
    }
}
