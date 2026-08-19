//! SF1's stereo mix: one YM2151 panned across two speakers, two MSM5205s in
//! both.
//!
//! # Why this is not [`crate::cps1::mix`]
//!
//! Not the coefficients — the *saturation argument*. `cps1::mix`'s doc is
//! explicit that its lack of saturation is measured rather than assumed: the OKI
//! clamps its own sum to ±`oki::chip::CLAMP_2X` first, which bounds that
//! numerator at ±655,360 = 20 × 32,768. Neither the change of coefficients nor
//! the second ADPCM chip preserves that. Here the numerator reaches ±262,144,
//! which over [`MIX_DEN`] is ±52,428 — 60% past `i16` — so this mix saturates
//! explicitly and reports having done so.
//!
//! # The routes
//!
//! `sf.cpp:783-796` sends the YM's two channels to **opposite** speakers at 0.60
//! and both MSM5205s to **both** speakers at 1.0:
//!
//! ```text
//! left  = 0.60 * ym_l + 1.0 * msm0 + 1.0 * msm1
//! right = 0.60 * ym_r + 1.0 * msm0 + 1.0 * msm1
//! ```
//!
//! ⚠️ That asymmetry is what makes the board stereo. A mix that sent both YM
//! channels to both speakers, or either MSM to one side, would sound plausible
//! and would not be this board.
//!
//! # Where the clamp comes from
//!
//! `speaker_device::mix` (`speaker.cpp:89-146`) applies the pan and does **not**
//! clamp. The clamp is in the final downmix, `emusound.cpp:1598-1632`, which
//! clamps each side to ±1.0 independently and then multiplies by 32,767. So a
//! loud left channel cannot pull the right one down, and a mix that summed the
//! sides before clamping would cross-couple them.

/// The YM2151's route gain over [`MIX_DEN`]: 0.60 = 3/5, exactly.
pub const YM_NUM: i32 = 3;

/// Each MSM5205's route gain over [`MIX_DEN`]: 1.0 = 5/5, exactly.
///
/// ⚠️ Equal to [`MIX_DEN`] today and not the same constant. This is a gain; that
/// is a denominator. Changing the YM's gain moves the denominator and leaves this
/// tracking it.
pub const MSM_NUM: i32 = 5;

/// The common denominator, chosen so both route gains are exact integers over it.
///
/// The same device as `cps1::mix`'s 20, for the same reason: a float gain
/// transcribed as a float would put rounding inside the audio path.
pub const MIX_DEN: i32 = 5;

/// Mix one stereo sample, saturating each side, and report whether either
/// saturated.
///
/// `msm0` and `msm1` are [`crate::sf1::Msm5205::output`] values, already in the
/// `i16` full-scale domain; `ym_l` and `ym_r` are one pair from
/// `ym2151::Ym2151::generate`.
///
/// # The flag
///
/// One flag for both sides, feeding the overlay's single `CLP` column — where
/// CPS-1 shows `SoundTrace::oki_clamps`. SF1's ADPCM chips have no output clamp
/// of their own, so without this the column would read 0 forever and a distorted
/// mix would have no diagnostic. ⚠️ It means "the mix saturated", **not** "the
/// left channel saturated": a reader chasing one-sided distortion cannot get that
/// answer from this counter.
///
/// Reported from the arithmetic rather than by re-testing the output, for the
/// reason [`crate::sound::SoundBoard::oki_step_2x`] gives: a sample that
/// legitimately lands on exactly ±32,767 is not a clip, and the returned value
/// cannot distinguish the two. See
/// `a_sample_that_lands_on_the_rail_without_saturating_is_not_a_clip`.
///
/// # Accuracy
///
/// Within one LSB of MAME's float chain — the truncating divide and the
/// 32,767-against-32,768 full scale move the value in *opposite* directions, so
/// their difference rather than their sum is the deviation, and it reaches exactly
/// one only on the negative rail. That is why there is no dither or rounding term.
/// See `the_mix_is_mames_weights_within_one_lsb`.
#[must_use]
pub fn mix(ym_l: i16, ym_r: i16, msm0: i16, msm1: i16) -> ((i16, i16), bool) {
    // Both MSM terms are common to the two sides; only the YM term is panned.
    let adpcm = MSM_NUM * (i32::from(msm0) + i32::from(msm1));
    let (left, clip_l) = saturate((YM_NUM * i32::from(ym_l) + adpcm) / MIX_DEN);
    let (right, clip_r) = saturate((YM_NUM * i32::from(ym_r) + adpcm) / MIX_DEN);
    ((left, right), clip_l || clip_r)
}

/// One side's quotient into `i16`, with a flag for having had to clamp it.
///
/// `i16::try_from` rather than a pair of comparisons: the fallible conversion is
/// the exact condition being reported, so the flag and the clamp cannot disagree.
fn saturate(value: i32) -> (i16, bool) {
    match i16::try_from(value) {
        Ok(v) => (v, false),
        Err(_) => (if value < 0 { i16::MIN } else { i16::MAX }, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sf1::msm5205::{Msm5205, DAC_MASK, DAC_TO_I16};

    /// The weights are MAME's route gains as exact ratios.
    ///
    /// Asserted as ratios rather than as the bare integers 3, 5 and 5: two of the
    /// three are the same number today, and a test that only pinned the integers
    /// would keep passing if the YM's numerator were swapped with the denominator.
    #[test]
    fn the_weights_are_mames_route_gains() {
        // `ymsnd.add_route(0, "lspeaker", 0.60)` (`sf.cpp:784`).
        assert_eq!(f64::from(YM_NUM) / f64::from(MIX_DEN), 0.60);
        // `m_msm[0]->add_route(ALL_OUTPUTS, "lspeaker", 1.0)` (`sf.cpp:790`).
        assert_eq!(f64::from(MSM_NUM) / f64::from(MIX_DEN), 1.0);
    }

    /// Silence in, silence out, on both sides.
    #[test]
    fn silence_mixes_to_silence() {
        assert_eq!(mix(0, 0, 0, 0), ((0, 0), false));
    }

    /// The YM's two channels go to opposite speakers.
    ///
    /// This is the asymmetry that makes the board stereo. A mix that sent both YM
    /// channels to both speakers would pass every level test below and destroy the
    /// stereo image.
    #[test]
    fn the_ym_channels_go_to_opposite_speakers() {
        let ((l, r), clipped) = mix(1000, 0, 0, 0);
        assert_eq!(l, 600, "0.60 of 1000");
        assert_eq!(r, 0, "channel 0 is left only");
        assert!(!clipped);
        let ((l, r), _) = mix(0, 1000, 0, 0);
        assert_eq!(l, 0, "channel 1 is right only");
        assert_eq!(r, 600);
    }

    /// Both MSM5205s go to both speakers, at unity.
    #[test]
    fn both_msms_go_to_both_speakers_at_unity() {
        assert_eq!(
            mix(0, 0, 1000, 0),
            ((1000, 1000), false),
            "chip 0, unity, both"
        );
        assert_eq!(
            mix(0, 0, 0, 1000),
            ((1000, 1000), false),
            "chip 1, unity, both"
        );
        assert_eq!(mix(0, 0, 1000, 500), ((1500, 1500), false), "and they sum");
    }

    /// One channel's YM level cannot move the other channel.
    ///
    /// `emusound.cpp:1598-1632` clamps each side independently, so a loud left
    /// cannot pull the right down. A mix that summed the sides before clamping —
    /// or shared one accumulator — fails here.
    #[test]
    fn the_two_sides_do_not_cross_couple() {
        // Left saturates; right is a quiet MSM-only signal and must be untouched.
        let ((l, r), clipped) = mix(i16::MAX, 0, 16_352, 16_352);
        assert_eq!(l, i16::MAX, "the left side saturated");
        assert!(clipped);
        assert_eq!(r, 32_704, "5 * 16352 * 2 / 5, with no YM term");
        // And the reverse.
        let ((l, r), _) = mix(0, i16::MAX, 16_352, 16_352);
        assert_eq!(l, 32_704);
        assert_eq!(r, i16::MAX);
    }

    /// The worst case is reachable and saturates rather than wrapping.
    ///
    /// The numerators and quotients are literals computed from the weights and the
    /// two chips' documented ranges, not from the code: a mix that wrapped would
    /// produce a *loud* wrong sample with the opposite sign, which is the audible
    /// failure this test exists to prevent.
    #[test]
    fn the_worst_case_saturates() {
        // Negative: 3 * -32768 + 5 * -16384 + 5 * -16384 = -262_144, / 5 = -52_428.
        assert_eq!(
            3 * -32_768 + 5 * -16_384 + 5 * -16_384,
            -262_144,
            "the numerator, from the weights"
        );
        assert_eq!(-262_144 / 5, -52_428, "past i16 by 60%");
        let ((l, r), clipped) = mix(i16::MIN, i16::MIN, -16_384, -16_384);
        assert_eq!((l, r), (i16::MIN, i16::MIN));
        assert!(clipped);

        // Positive: 3 * 32767 + 5 * 16352 + 5 * 16352 = 261_821, / 5 = 52_364.
        assert_eq!(3 * 32_767 + 5 * 16_352 + 5 * 16_352, 261_821);
        assert_eq!(261_821 / 5, 52_364);
        let ((l, r), clipped) = mix(i16::MAX, i16::MAX, 16_352, 16_352);
        assert_eq!((l, r), (i16::MAX, i16::MAX));
        assert!(clipped);
    }

    /// The chips' real extremes are what reach that worst case.
    ///
    /// The literals above are only worth trusting if a chip can actually produce
    /// them, so this derives both MSM rails from [`Msm5205`] itself rather than
    /// restating them. If the DAC scale ever changed, this test fails and the
    /// worst-case literals above stop being a claim about the hardware.
    #[test]
    fn the_msm_rails_are_the_chips_own() {
        let rail = |signal| Msm5205::restore(signal, 0, 0, false, false, 0).output();
        assert_eq!(rail(2047), 16_352, "and 2047 & !3 == 2044");
        assert_eq!(rail(-2048), -16_384);
        // Spelled out once, so the relationship is visible rather than coincidental.
        assert_eq!(i32::from((2047i16 & !DAC_MASK) * DAC_TO_I16), 16_352);
    }

    /// A sample that lands exactly on the rail without saturating is not a clip.
    ///
    /// [`crate::sound::SoundBoard::oki_step_2x`]'s argument, applied here: comparing
    /// the output against `i16::MAX` cannot tell a legitimate full-scale sample from
    /// a clipped one, so the flag comes from the arithmetic instead. The input is
    /// chosen so the quotient is exactly 32,767 — `3 * ym / 5 == 32_767` needs a
    /// numerator in `163_835..=163_839`, and `ym = 54_611` is out of `i16`, so the
    /// exact hit is built from the MSM terms instead.
    #[test]
    fn a_sample_that_lands_on_the_rail_without_saturating_is_not_a_clip() {
        // 5 * 16352 + 5 * 16352 = 163_520, plus 3 * ym. Want 163_835: 3 * ym = 315,
        // so ym = 105.
        let ((l, r), clipped) = mix(105, 105, 16_352, 16_352);
        assert_eq!(3 * 105 + 5 * 16_352 + 5 * 16_352, 163_835, "the numerator");
        assert_eq!(163_835 / 5, 32_767, "exactly the rail");
        assert_eq!((l, r), (i16::MAX, i16::MAX));
        assert!(!clipped, "on the rail is not over it");
        // One LSB more of numerator on the left only, and the left side clips.
        let ((l, r), clipped) = mix(107, 105, 16_352, 16_352);
        assert_eq!(3 * 107 + 163_520, 163_841, "past 163_839");
        assert_eq!((l, r), (i16::MAX, i16::MAX), "the same output");
        assert!(clipped, "but now it is a clip");
    }

    /// The flag is "the mix saturated", not "the left channel saturated".
    ///
    /// One flag for the overlay's one `CLP` column. Pinned because a reader
    /// debugging one-sided distortion needs to know which claim the counter makes,
    /// and because a per-side flag silently ORed into one is a different design that
    /// would pass every level test above.
    #[test]
    fn the_flag_covers_either_side() {
        let (_, left_only) = mix(i16::MAX, 0, 16_352, 16_352);
        assert!(left_only, "left saturated, right did not");
        let (_, right_only) = mix(0, i16::MAX, 16_352, 16_352);
        assert!(right_only, "right saturated, left did not");
        let (_, neither) = mix(100, 100, 100, 100);
        assert!(!neither);
    }

    /// The truncating divide is negative-symmetric in Rust's sense, and documented.
    ///
    /// Rust's `/` truncates towards zero, so a numerator of -1 gives 0 rather than
    /// -1. MAME's float chain rounds; the divergence is bounded by the test below,
    /// and this one pins the direction so a reader is not surprised by a
    /// zero-crossing that flattens.
    #[test]
    fn the_divide_truncates_towards_zero() {
        assert_eq!(
            mix(1, -1, 0, 0),
            ((0, 0), false),
            "3/5 and -3/5 both truncate"
        );
        assert_eq!(mix(2, -2, 0, 0), ((1, -1), false), "6/5 and -6/5");
    }

    /// The integer mix never deviates from MAME's float chain by more than one LSB,
    /// and reaches exactly one only on the negative rail.
    ///
    /// MAME computes `clamp(0.60 * ym/32768 + m0/32768 + m1/32768, ±1.0) * 32767`.
    /// The MSM terms divide by 32,768 rather than by MAME's 4,096 because
    /// [`Msm5205::output`] has already applied [`DAC_TO_I16`] — so this reference is
    /// MAME's arithmetic in this crate's units, not a restatement of it.
    ///
    /// Two terms separate the two forms and they do **not** add: the integer divide
    /// truncates towards zero by `frac`, and the 32,767-against-32,768 full scale
    /// scales down by `exact/32768`, so the deviation is their *difference* and
    /// stays under one LSB everywhere the mix does not saturate.
    ///
    /// The one place it reaches exactly 1.0 is the negative rail, where MAME's
    /// ±32,767 scale bottoms out at -32,767 and `i16` at -32,768. Asserted as an
    /// equality rather than a bound, because it is a fixed consequence of the two
    /// full scales rather than an accumulation — and because an inequality would
    /// keep passing if a rounding term were added and made the typical case worse.
    #[test]
    fn the_mix_is_mames_weights_within_one_lsb() {
        let reference = |ym: i16, m0: i16, m1: i16| -> f64 {
            let x = 0.60 * f64::from(ym) / 32_768.0
                + f64::from(m0) / 32_768.0
                + f64::from(m1) / 32_768.0;
            x.clamp(-1.0, 1.0) * 32_767.0
        };
        let mut worst = 0.0_f64;
        // 105 is in the grid deliberately: with both MSMs at their positive rail it
        // makes the quotient exactly 32,767, i.e. `frac == 0` with `exact/32768`
        // at its largest, which is the worst *unsaturated* case.
        for ym in [i16::MIN, -20_000, -3, -1, 0, 1, 3, 105, 20_000, i16::MAX] {
            for m0 in [-16_384i16, -4_000, 0, 4_000, 16_352] {
                for m1 in [-16_384i16, -777, 0, 777, 16_352] {
                    let ((l, r), _) = mix(ym, ym, m0, m1);
                    assert_eq!(l, r, "the same YM channel on both sides");
                    let dev = (f64::from(l) - reference(ym, m0, m1)).abs();
                    assert!(dev <= 1.0, "ym {ym} m0 {m0} m1 {m1}: off by {dev}");
                    if dev > worst {
                        worst = dev;
                    }
                }
            }
        }
        // Exactly 1.0, and exactly representable: -32768 minus -32767.0.
        assert_eq!(worst, 1.0, "the grid must reach the negative rail");
        let ((l, _), clipped) = mix(i16::MIN, i16::MIN, -16_384, -16_384);
        assert_eq!(l, i16::MIN);
        assert!(clipped);
        assert_eq!(f64::from(l) - reference(i16::MIN, -16_384, -16_384), -1.0);
    }

    /// A playing chip reaches the mix, weighted rather than passed through.
    ///
    /// The end-to-end check: a real [`Msm5205`] driven to a nonzero signal changes
    /// the mixed output on both sides. Without this, a mix that dropped an MSM term
    /// entirely would pass every test that supplies the terms as bare integers.
    #[test]
    fn a_decoding_chip_reaches_both_sides_of_the_mix() {
        let mut c = Msm5205::new();
        for _ in 0..8 {
            c.msm_w(0x07);
            for _ in 0..6 {
                c.tick();
            }
        }
        assert_ne!(c.output(), 0, "the premise: the chip is producing signal");
        let ((l, r), clipped) = mix(0, 0, c.output(), 0);
        assert_eq!(l, c.output(), "unity gain, and it is audible");
        assert_eq!(r, c.output());
        assert!(!clipped);
    }
}
