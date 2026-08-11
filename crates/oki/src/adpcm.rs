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
}
