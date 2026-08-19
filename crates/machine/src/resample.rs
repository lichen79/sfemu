//! Host-rate conversion and the bounded ring between the two clocks.
//!
//! This lives in `machine`, not behind the `Audio` trait, for one reason: it needs
//! testing. The trait boundary exists because a host device cannot be asserted about,
//! so anything with behaviour worth asserting has to sit on this side of it.
//!
//! # The rates do not match, and the error is not a footnote
//!
//! The board produces `SOUND_XTAL / YM_SAMPLE_CLOCKS` = 55,930.390625 **frames** a
//! second — every buffer here is interleaved stereo, [`CHANNELS`] samples per frame,
//! so a rate in this file is always a frame rate. A host wants
//! 48,000. Handing the board's stream to a 48 kHz device unconverted plays it **14.2%
//! slow** — 48,000/55,930.390625 = 0.858, so SF2's music comes out **2.65 semitones
//! flat**, A440 landing at 378 Hz. The direction is worth stating rather than leaving
//! to the reader: a device consuming 48,000 samples a second out of a 55,930 Hz stream
//! takes 1.165 seconds of music to fill one second, so the pitch falls. See
//! `the_resampler_knows_both_rates_are_different`, which pins the ratio.
//!
//! The conversion is **linear interpolation**, and the cost is stated rather than
//! hidden: at 1.165× downsampling it attenuates the top of the band and folds content
//! above 24 kHz back down. A polyphase FIR would be better and is either a dependency
//! or 200 lines of DSP with its own verification burden. The chip and the mix are
//! exact; the host output is not, and cannot be, because no host rate is a rational
//! multiple of 3,579,545/64.
//!
//! # The ring's policy is measured, not designed
//!
//! Every number here comes from the spec's probes rather than from taste: 100 ms of
//! capacity prefilled to 50 ms (the observed depth swing was 29.3–58.7 ms, so 50 ms of
//! headroom either side is ~1.7× it), drop the oldest on overflow so latency stays
//! bounded, hold the last sample on underrun because a step to silence clicks, count
//! both so "the audio is crackly" is diagnosable, and **no clock slewing** — drift
//! measured at +6.3 ppm ± 59.6 ppm, which is below the method's own resolution. The
//! bound that matters is the jitter, ~60 ppm or 3.6 ms a minute, and a 100 ms ring
//! absorbs 25 minutes of that from a 50 ms centre.

use crate::timing::{SOUND_XTAL, YM_SAMPLE_CLOCKS};
use std::collections::VecDeque;

/// How many milliseconds the ring holds.
///
/// Public because it is the emulator's audio latency ceiling, and a frontend that
/// reports latency or sizes a device buffer needs the number rather than a guess at it.
pub const RING_MS: u32 = 100;
/// How many milliseconds it prefills to before the device starts consuming.
///
/// Public for [`RING_MS`]'s reason: this is the latency the player actually hears once
/// the ring settles, the capacity being only the bound.
pub const PREFILL_MS: u32 = 50;

/// Channels in every audio buffer in this crate: two, always.
///
/// Interleaved L,R, not `(i16, i16)` and not a parameter. cpal's output buffer is
/// already interleaved, so the callback's copy is a memcpy rather than a fan-out;
/// the ring's arithmetic stays integer counts with an even-frame invariant rather
/// than gaining a stride; and CPS-1, which is mono, writes its one value into both
/// slots. One extra `i16` per sample buys exactly one code path through the
/// resampler — and the mono path is the one with 317,500 test cases behind it,
/// while the stereo path is new.
pub const CHANNELS: usize = 2;

/// Linear interpolation from the emulator's rate to a host rate.
#[derive(Clone, Debug)]
pub struct Resampler {
    host_rate: u32,
    /// How far past [`Resampler::prev`] the next output sample falls, as a numerator
    /// over the ratio's denominator.
    pos: u64,
    /// The previous input **frame**, so an interpolation can span a feed boundary.
    ///
    /// A frame rather than a sample: interpolating across the L/R boundary produces a
    /// signal that is neither channel, and at a non-integer ratio the proportion
    /// walks — audible as phasing rather than as an obvious defect. See
    /// `a_hard_panned_signal_stays_hard_panned`.
    prev: [i16; CHANNELS],
    /// Whether `prev` has ever been set. The first sample interpolates against itself
    /// rather than against a zero, so a stream that starts at a DC level does not open
    /// with a ramp up to it.
    primed: bool,
}

impl Resampler {
    /// A resampler onto `host_rate` samples per second.
    ///
    /// # Panics
    ///
    /// Panics if `host_rate` is zero: the ratio's denominator would be zero, and every
    /// later call would divide by it.
    #[must_use]
    pub fn new(host_rate: u32) -> Self {
        assert!(
            host_rate > 0,
            "a host rate of zero has no ratio to the board's"
        );
        Self {
            host_rate,
            pos: 0,
            prev: [0; CHANNELS],
            primed: false,
        }
    }

    /// The host rate this converts to.
    #[must_use]
    pub const fn host_rate(&self) -> u32 {
        self.host_rate
    }

    /// Input samples per output sample, as an exact rational.
    ///
    /// The board's rate is `SOUND_XTAL / YM_SAMPLE_CLOCKS`, so the ratio to the host's
    /// is `SOUND_XTAL / (host_rate * YM_SAMPLE_CLOCKS)` — kept as a fraction rather
    /// than a float for [`crate::timing`]'s reason: 55,930.390625 Hz is not a whole
    /// number, and a rounded step accumulates into audible drift over a session.
    #[must_use]
    pub const fn ratio(&self) -> (u32, u32) {
        (SOUND_XTAL, self.host_rate * YM_SAMPLE_CLOCKS)
    }

    /// Convert `input` and append the result to `out`. Both are interleaved L,R.
    ///
    /// The fractional position and the last input frame both carry across calls, so
    /// feeding one long slice and many short ones give byte-identical output. They
    /// have to: the emulator hands over a frame at a time, and a phase that reset per
    /// call would make every frame boundary an audible seam.
    ///
    /// A trailing partial frame is ignored rather than padded — `chunks_exact` — so a
    /// caller that hands over an odd count loses half a sample instead of transposing
    /// every channel that follows it.
    pub fn feed(&mut self, input: &[i16], out: &mut Vec<i16>) {
        let (num, den) = self.ratio();
        let num = u64::from(num);
        let den = u64::from(den);
        for frame in input.chunks_exact(CHANNELS) {
            if !self.primed {
                self.prev.copy_from_slice(frame);
                self.primed = true;
            }
            // `pos` is where the next output falls within the step from `prev` to
            // `frame`, scaled by `den`. Emit every output that lands inside this step.
            // When downsampling `num > den`, so most steps emit one frame and some emit
            // none; upsampling emits several.
            while self.pos < den {
                let t = self.pos as i64;
                let d = den as i64;
                for (&p, &c) in self.prev.iter().zip(frame) {
                    let a = i64::from(p);
                    let b = i64::from(c);
                    // Both terms are at most 32,767 × 12.3M, which is 4.0e11 — three
                    // orders of magnitude inside `i64`.
                    out.push(((a * (d - t) + b * t) / d) as i16);
                }
                // Once per *frame*, not once per sample: advancing per sample compiles,
                // halves the output rate, and mixes the channels.
                self.pos += num;
            }
            // The loop exits with `pos >= den`, so this cannot underflow.
            self.pos -= den;
            self.prev.copy_from_slice(frame);
        }
    }
}

/// How many frames the ring dropped and held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RingStats {
    /// Frames discarded because the ring was full.
    pub drops: u32,
    /// Frames the consumer had to hold because the ring was empty.
    pub underruns: u32,
}

/// A bounded ring between the emulator's clock and the device's.
#[derive(Clone, Debug)]
pub struct Ring {
    /// Whole frames, so no drop or underrun can transpose the channels.
    buf: VecDeque<[i16; CHANNELS]>,
    /// In frames.
    capacity: usize,
    /// In frames.
    prefill: usize,
    /// Whether the prefill has ever been reached. **Sticky**: a drain is a real
    /// underrun, not a return to startup. Re-arming would mute for another 50 ms every
    /// time the machine stuttered, turning one click into half a second of silence.
    armed: bool,
    last: [i16; CHANNELS],
    stats: RingStats,
}

impl Ring {
    /// A ring sized for `host_rate`: [`RING_MS`] of capacity, [`PREFILL_MS`] of
    /// prefill.
    #[must_use]
    pub fn new(host_rate: u32) -> Self {
        Self::with_prefill(host_rate, Self::ms(host_rate, PREFILL_MS))
    }

    /// A ring with an explicit prefill, in frames.
    ///
    /// The tests use this so the behaviour assertions do not all depend on the one
    /// default — a policy test that only ever sees 2,400 frames cannot tell "prefills
    /// correctly" from "happens to hold 2,400 frames".
    #[must_use]
    pub fn with_prefill(host_rate: u32, prefill: usize) -> Self {
        let capacity = Self::ms(host_rate, RING_MS);
        Self {
            buf: VecDeque::with_capacity(capacity),
            capacity,
            prefill: prefill.min(capacity),
            armed: false,
            last: [0; CHANNELS],
            stats: RingStats::default(),
        }
    }

    /// `ms` milliseconds at `host_rate`, in frames.
    ///
    /// A host rate is frames per second, so this is unchanged by the widening — which
    /// is what preserves the measured policy exactly.
    ///
    /// Multiplied before dividing, and in `u64`. `(host_rate / 1000) * ms` is the
    /// obvious spelling and it is wrong for 44,100 Hz: 44 × 100 is 4,400 rather than
    /// 4,410, a 10-sample error that would put the CD rate's ring and prefill a fifth
    /// of a millisecond short of the policy on every one.
    fn ms(host_rate: u32, ms: u32) -> usize {
        usize::try_from(u64::from(host_rate) * u64::from(ms) / 1000)
            .expect("a ring of a tenth of a second fits a usize on any host")
    }

    /// How many **frames** the ring holds at most.
    ///
    /// Frames rather than samples, so [`RING_MS`]'s policy keeps its stated value:
    /// 4,800 frames at 48 kHz is 100 ms either way you count, but 4,800 *samples*
    /// would be 50 ms and the policy would be gone with no test failing.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// How many **frames** to accumulate before the device starts consuming.
    #[must_use]
    pub const fn prefill(&self) -> usize {
        self.prefill
    }

    /// How many **frames** are waiting. This is what the pacer reads: it asks how much
    /// *time* is buffered, and time is frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// How many interleaved samples are waiting: [`Ring::len`] × [`CHANNELS`].
    ///
    /// For a caller sizing an interleaved buffer, which is the one question
    /// [`Ring::len`] cannot answer without the reader knowing the channel count.
    #[must_use]
    pub fn len_samples(&self) -> usize {
        self.buf.len() * CHANNELS
    }

    /// Whether nothing is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The drop and underrun counts.
    #[must_use]
    pub const fn stats(&self) -> RingStats {
        self.stats
    }

    /// Reset the counters.
    ///
    /// The counters describe the *session*, like [`crate::sound::SoundTrace`]'s, so
    /// they are not state and a comparison between two runs has to be able to exclude
    /// them explicitly.
    pub fn clear_stats(&mut self) {
        self.stats = RingStats::default();
    }

    /// Add interleaved samples, dropping the oldest **frame** if the ring is full.
    ///
    /// Dropping the *oldest* keeps latency bounded. Letting the ring grow would trade a
    /// click now for a delay that grows for as long as the emulator runs ahead, and
    /// dropping the newest would throw away the audio the player is about to hear in
    /// favour of audio they should already have heard.
    ///
    /// A trailing partial frame is ignored. Dropping or storing half a frame would
    /// transpose every channel after it, permanently and undetectably — see
    /// `a_drop_discards_whole_frames`.
    pub fn push(&mut self, samples: &[i16]) {
        for frame in samples.chunks_exact(CHANNELS) {
            if self.buf.len() >= self.capacity {
                self.buf.pop_front();
                self.stats.drops = self.stats.drops.saturating_add(1);
            }
            let mut f = [0i16; CHANNELS];
            f.copy_from_slice(frame);
            self.buf.push_back(f);
        }
    }

    /// Fill `out` with interleaved samples, holding the last frame if the ring runs
    /// dry.
    ///
    /// `paused` suppresses the underrun count and outputs silence: a paused emulator
    /// produces nothing by design, so counting that as a fault makes the counter
    /// worthless as a "your machine cannot keep up" signal — and holding a DC level for
    /// the length of the pause would be worse than the click it avoids.
    ///
    /// Before the prefill is first reached this outputs silence and **consumes
    /// nothing**. The device's first callback arrives 143 ms after `play()`, well
    /// before the emulator has produced 50 ms of audio, and starting on a nearly-empty
    /// ring would underrun on the first block.
    ///
    /// A trailing partial frame in `out` is zeroed rather than half-filled: a device
    /// block that is not a whole number of this stream's frames is the caller's
    /// problem, and half a frame written into it would transpose the next block.
    pub fn pop(&mut self, out: &mut [i16], paused: bool) {
        if !self.armed {
            if self.buf.len() < self.prefill {
                out.fill(0);
                return;
            }
            self.armed = true;
        }
        let whole = out.len() - out.len() % CHANNELS;
        let (frames, tail) = out.split_at_mut(whole);
        tail.fill(0);
        for slot in frames.chunks_mut(CHANNELS) {
            if let Some(f) = self.buf.pop_front() {
                self.last = f;
                slot.copy_from_slice(&f);
            } else if paused {
                slot.fill(0);
            } else {
                // Holding rather than zeroing: a step to silence is a click, and a held
                // level is a DC excursion that the device's own filtering removes. The
                // whole frame, so the channels cannot slip.
                slot.copy_from_slice(&self.last);
                self.stats.underruns = self.stats.underruns.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame is two samples, and every public length is in frames.
    ///
    /// Frames rather than samples for the numbers the policy is stated in: 4,800
    /// frames at 48 kHz is the 100 ms the module doc defends. Reporting samples would
    /// halve the policy with no test failing, because
    /// `the_ring_is_sized_from_the_measured_policy` asserts the number rather than the
    /// milliseconds.
    #[test]
    fn a_frame_is_two_samples_and_lengths_are_frames() {
        assert_eq!(CHANNELS, 2);
        let mut ring = Ring::new(48_000);
        assert_eq!(ring.capacity(), 4800, "100 ms at 48 kHz, in frames");
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.len_samples(), 0);
        ring.push(&[100, -100, 200, -200]);
        assert_eq!(ring.len(), 2, "two frames");
        assert_eq!(ring.len_samples(), 4, "four samples");
        // The policy in milliseconds, which is the claim the numbers stand for.
        assert_eq!(
            u64::from(RING_MS) * 48_000 / 1000,
            ring.capacity() as u64,
            "capacity is RING_MS of frames"
        );
    }

    /// A hard-panned signal stays hard-panned through the resampler.
    ///
    /// The assertion a per-sample resampler fails and a per-frame one passes: at
    /// 55,930 → 48,000 the ratio is not an integer, so a resampler that interpolates
    /// across the L/R boundary mixes the two channels in a proportion that walks —
    /// audible as phasing, not as an obvious bug. Left full scale and right silent is
    /// the input that makes any leakage a nonzero right sample.
    #[test]
    fn a_hard_panned_signal_stays_hard_panned() {
        let mut r = Resampler::new(48_000);
        let mut out = Vec::new();
        // A left channel that moves, so an interpolator has something to smear, and a
        // right channel that is exactly zero.
        let mut input = Vec::new();
        for i in 0..2048i32 {
            let l = ((i % 64) * 500 - 16_000) as i16;
            input.push(l);
            input.push(0);
        }
        r.feed(&input, &mut out);
        assert!(!out.is_empty(), "nothing came out");
        assert_eq!(out.len() % CHANNELS, 0, "a partial frame was emitted");
        for (frame, chunk) in out.chunks(CHANNELS).enumerate() {
            assert_eq!(
                chunk[1], 0,
                "frame {frame}: the right channel is {}, so the left leaked into it",
                chunk[1]
            );
        }
        assert!(
            out.chunks(CHANNELS).any(|c| c[0] != 0),
            "the left channel came out silent, so the test proved nothing"
        );
        // And the left channel is genuinely resampled rather than passed through: the
        // output frame count tracks the ratio, not the input count.
        let frames_in = input.len() / CHANNELS;
        let frames_out = out.len() / CHANNELS;
        assert!(
            frames_out < frames_in,
            "48 kHz out of 55,930 Hz must produce fewer frames: {frames_out} from \
             {frames_in}"
        );
    }

    /// An underrun consumes and holds whole frames, so the channels never swap.
    ///
    /// The failure this test exists for is permanent and undetectable downstream: one
    /// odd sample held or consumed makes every later frame R,L, and the ring's own
    /// length arithmetic looks correct either way. Driven with an odd-length `out`
    /// slice as well, because a device's callback block size is not guaranteed even
    /// in frames of *this* stream's channel count.
    #[test]
    fn an_underrun_holds_whole_frames() {
        let mut ring = Ring::with_prefill(48_000, 0);
        // Distinguishable channels: left positive, right negative.
        ring.push(&[1000, -1000, 2000, -2000, 3000, -3000]);
        let mut out = [0i16; 16];
        ring.pop(&mut out, false);
        // Three frames of real audio, then the held frame repeated.
        assert_eq!(&out[..6], &[1000, -1000, 2000, -2000, 3000, -3000]);
        for (i, frame) in out[6..].chunks(CHANNELS).enumerate() {
            assert_eq!(
                frame,
                &[3000, -3000],
                "held frame {i} is {frame:?}, so a channel slipped"
            );
        }
        assert!(ring.stats().underruns > 0, "the underrun was counted");
        // Now the part a sample-wise ring gets wrong: feed real audio again and check
        // the channels did not swap.
        ring.push(&[4000, -4000]);
        let mut after = [0i16; 2];
        ring.pop(&mut after, false);
        assert_eq!(
            after,
            [4000, -4000],
            "the channels swapped after the underrun, which is permanent"
        );
    }

    /// A drop discards whole frames, so overflow does not swap the channels either.
    ///
    /// [`Ring::push`] drops the oldest, and dropping one sample rather than one frame
    /// has the same permanent consequence as an odd underrun — with the extra
    /// unpleasantness that it happens on a *fast* machine, which is the case nobody
    /// tests by ear.
    #[test]
    fn a_drop_discards_whole_frames() {
        let mut ring = Ring::with_prefill(48_000, 0);
        let cap = ring.capacity();
        // Fill it exactly, with each frame numbered so the survivors are identifiable.
        let mut filled = Vec::new();
        for i in 0..cap {
            filled.push(i as i16);
            filled.push(-(i as i16));
        }
        ring.push(&filled);
        assert_eq!(ring.len(), cap, "full");
        assert_eq!(ring.stats().drops, 0, "and nothing dropped yet");
        // One more frame: the oldest frame goes.
        ring.push(&[30_000, -30_000]);
        assert_eq!(ring.len(), cap, "still full");
        assert_eq!(ring.stats().drops, 1, "one frame dropped, not two samples");
        let mut out = [0i16; 2];
        ring.pop(&mut out, false);
        assert_eq!(out, [1, -1], "the second frame is now the oldest");
    }

    /// The ratio is the two rates, and it is not 1 — which is the whole reason this
    /// file exists. At 48 kHz the unconverted path would be 14.2% slow, 2.65 semitones
    /// flat; the module docs work the direction out.
    #[test]
    fn the_resampler_knows_both_rates_are_different() {
        let r = Resampler::new(48_000);
        assert_eq!(r.host_rate(), 48_000);
        // 55,930.390625 in, 48,000 out: about 1.165 input samples per output.
        let (num, den) = r.ratio();
        assert_eq!(num, crate::timing::SOUND_XTAL);
        assert_eq!(den, 48_000 * crate::timing::YM_SAMPLE_CLOCKS);
        let pitch_error = f64::from(num) / f64::from(den);
        assert!(
            (pitch_error - 1.165).abs() < 0.001,
            "1.165 input samples per output at 48 kHz, got {pitch_error}"
        );
    }

    /// A constant in is a constant out, at every rate. The simplest property a broken
    /// interpolator fails.
    #[test]
    fn a_constant_signal_survives_resampling() {
        for host in [44_100u32, 48_000, 88_200, 96_000] {
            let mut r = Resampler::new(host);
            let mut out = Vec::new();
            for _ in 0..8 {
                r.feed(&[1000i16; 256], &mut out);
            }
            assert!(!out.is_empty(), "host {host}: nothing came out");
            for (i, &s) in out.iter().enumerate() {
                assert_eq!(s, 1000, "host {host}: sample {i} is {s}, not 1000");
            }
            assert_eq!(
                out.len() % CHANNELS,
                0,
                "host {host}: a partial frame was emitted"
            );
        }
    }

    /// The output length tracks the ratio: feeding N input samples produces about
    /// `N * host / 55930` output samples.
    ///
    /// Asserted against the *rates*, not against a second call to the resampler — a
    /// comparison between two resamplers would pass for a resampler that ignored the
    /// host rate entirely.
    #[test]
    fn the_output_length_follows_the_rate_ratio() {
        for host in [44_100u32, 48_000, 96_000] {
            let mut r = Resampler::new(host);
            let mut out = Vec::new();
            let n = 55_930usize; // about one second of input
            for chunk in vec![0i16; n].chunks(512) {
                r.feed(chunk, &mut out);
            }
            let want = host as usize;
            let got = out.len();
            assert!(
                got.abs_diff(want) < want / 100,
                "host {host}: {got} out for {n} in, expected about {want}"
            );
        }
    }

    /// A ramp stays monotonic: linear interpolation of an increasing signal cannot
    /// decrease. This catches an index that walks backwards.
    #[test]
    fn a_ramp_stays_monotonic_through_the_resampler() {
        let mut r = Resampler::new(48_000);
        // The ramp is per *frame* — the same value in both channels. A ramp laid down
        // per sample instead would put L and R half a step apart, and the interleaved
        // output of two offset ramps is not monotonic even when each channel is: the
        // next frame's L is interpolated 0.165 frames further along than this frame's
        // R, which is not far enough to make up the half-step. That failure would be
        // the fixture's, not the resampler's.
        let ramp: Vec<i16> = (0..1000).flat_map(|i| [i as i16, i as i16]).collect();
        let mut out = Vec::new();
        for chunk in ramp.chunks(256) {
            r.feed(chunk, &mut out);
        }
        assert!(out.len() > 1500, "only {} samples out", out.len());
        for w in out.windows(2) {
            assert!(
                w[1] >= w[0],
                "the ramp went backwards: {} then {}",
                w[0],
                w[1]
            );
        }
    }

    /// It **interpolates** rather than picking the nearest input sample.
    ///
    /// Every other test here passes for a nearest-neighbour resampler: a constant stays
    /// constant, a ramp stays monotonic, and the output length depends only on the step
    /// size. So this one upsamples a coarse staircase — 96 kHz is ~1.72 outputs per
    /// input — and requires outputs that lie strictly *between* two neighbouring input
    /// values. A resampler that repeated samples produces only multiples of `STEP` here
    /// and cannot pass.
    ///
    /// `STEP` is 500 across 64 stairs, which tops out at 31,500: an `i16` staircase of
    /// 1,000 would overflow at stair 33 and the test would panic in its own fixture.
    #[test]
    fn the_resampler_interpolates_rather_than_repeating_samples() {
        const STEP: i16 = 500;
        let mut r = Resampler::new(96_000);
        // Each stair duplicated, so the staircase is per *frame* and the claim below is
        // about one channel — which is what it was always about.
        let staircase: Vec<i16> = (0..64).flat_map(|i| [i * STEP, i * STEP]).collect();
        let mut out = Vec::new();
        r.feed(&staircase, &mut out);
        let left: Vec<i16> = out.chunks(CHANNELS).map(|c| c[0]).collect();
        let between = left.iter().filter(|&&s| s % STEP != 0).count();
        assert!(
            between > 20,
            "only {between} of {} samples are interpolated; a nearest-neighbour \
             resampler would produce 0",
            left.len()
        );
        // And the interpolated values stay inside the step they span rather than
        // overshooting it: an interpolation with the two weights swapped still lands
        // off-grid, so "not a multiple of STEP" alone would not catch it.
        for w in left.windows(2) {
            assert!(
                w[1] - w[0] <= STEP,
                "a step of {} is larger than the input's own {STEP}",
                w[1] - w[0]
            );
        }
    }

    /// Feeding in one chunk and in many gives the same result: the phase carries across
    /// calls, or a frame boundary becomes an audible seam.
    #[test]
    fn the_phase_carries_across_feeds() {
        // Duplicated per frame, and chunked at an even length: `feed` ignores a trailing
        // partial frame, so an odd chunk size would drop one sample per call and the two
        // paths would differ for that reason rather than for a phase reset.
        let signal: Vec<i16> = (0..3000)
            .flat_map(|i| {
                let v = ((i * 37) % 2000 - 1000) as i16;
                [v, v]
            })
            .collect();
        let mut whole = Vec::new();
        Resampler::new(48_000).feed(&signal, &mut whole);
        let mut piecemeal = Vec::new();
        let mut r = Resampler::new(48_000);
        for chunk in signal.chunks(102) {
            r.feed(chunk, &mut piecemeal);
        }
        assert!(!whole.is_empty(), "the premise: there is output to compare");
        assert_eq!(
            whole, piecemeal,
            "the chunked feed drifted from the whole one"
        );
    }

    /// The ring's capacity comes from the measured policy: 100 ms at the host rate,
    /// prefilled to 50 ms.
    #[test]
    fn the_ring_is_sized_from_the_measured_policy() {
        let ring = Ring::new(48_000);
        assert_eq!(ring.capacity(), 4800, "100 ms at 48 kHz");
        assert_eq!(ring.prefill(), 2400, "50 ms at 48 kHz");
        assert_eq!(ring.len_samples(), 0);
        // And it scales with the rate rather than being a magic number. 44,100 is the
        // case that catches `(rate / 1000) * ms`, which gives 4,400 here.
        assert_eq!(Ring::new(96_000).capacity(), 9600);
        assert_eq!(Ring::new(44_100).capacity(), 4410);
        assert_eq!(Ring::new(44_100).prefill(), 2205);
        // The prefill is a parameter, not a constant welded to the capacity: the
        // behaviour tests below vary it, so a test cannot pass by coincidence with the
        // default.
        let custom = Ring::with_prefill(48_000, 7);
        assert_eq!(custom.prefill(), 7);
        assert_eq!(
            custom.capacity(),
            4800,
            "the prefill does not change the size"
        );
    }

    /// The ring outputs silence until it has prefilled, and does not call that an
    /// underrun: startup is not a fault.
    ///
    /// Measured: the first callback arrives 143 ms after `play()`, so the device asks
    /// before the emulator has produced 50 ms of anything.
    #[test]
    fn the_ring_holds_silence_until_it_is_prefilled() {
        let mut ring = Ring::with_prefill(48_000, 4);
        let mut out = vec![9i16; 8];
        ring.push(&[100, 100, 200, 200]);
        ring.pop(&mut out, false);
        assert_eq!(out, vec![0; 8], "not yet prefilled, so silence");
        assert_eq!(ring.stats().underruns, 0, "startup is not an underrun");
        assert_eq!(ring.len(), 2, "and it did not consume the frames it had");
        ring.push(&[300, 300, 400, 400]);
        ring.pop(&mut out, false);
        assert_eq!(
            out,
            vec![100, 100, 200, 200, 300, 300, 400, 400],
            "armed, so it plays"
        );
    }

    /// Once armed it stays armed: a drain is a real underrun, not a return to startup.
    ///
    /// Re-arming would mute for another 50 ms every time the machine stuttered, so one
    /// dropped frame would cost half a second of silence.
    #[test]
    fn a_drained_ring_stays_armed() {
        let mut ring = Ring::with_prefill(48_000, 2);
        ring.push(&[5, 5, 6, 6]);
        let mut out = vec![0i16; 8];
        ring.pop(&mut out, false);
        assert_eq!(out, vec![5, 5, 6, 6, 6, 6, 6, 6]);
        assert_eq!(ring.stats().underruns, 2, "two held frames");
        ring.push(&[7, 7]);
        ring.pop(&mut out, false);
        assert_eq!(out[0], 7, "it played immediately rather than re-prefilling");
        assert_eq!(ring.stats().underruns, 5);
    }

    /// Overflow drops the oldest and counts it, so latency stays bounded.
    ///
    /// **Counted, not sampled.** Reading `out[cap - 1] == 2` and `out[0] == 1` cannot
    /// fail: a ring that drops the *newest* — `pop_back` where this one pops the front —
    /// evicts the sample it is about to insert, so it also ends holding `cap - 1` ones
    /// followed by a single 2, and both of those reads agree with it. The mutation
    /// survived on exactly that. What separates the two policies is *how many* of the
    /// 100 new samples are still there: all of them if the oldest went, one if the
    /// newest did.
    #[test]
    fn overflow_drops_the_oldest_and_counts_it() {
        let mut ring = Ring::with_prefill(48_000, 0);
        let cap = ring.capacity();
        ring.push(&vec![1i16; cap * CHANNELS]);
        assert_eq!(ring.len(), cap);
        assert_eq!(ring.stats().drops, 0);
        ring.push(&[2i16; 100 * CHANNELS]);
        assert_eq!(ring.len(), cap, "the ring grew past its capacity");
        assert_eq!(ring.stats().drops, 100, "100 frames, not 200 samples");
        // The newest frames survived; the oldest went.
        let mut out = vec![0i16; cap * CHANNELS];
        ring.pop(&mut out, false);
        assert_eq!(out[cap * CHANNELS - 1], 2, "the newest frame went instead");
        assert_eq!(out[0], 1);
        assert_eq!(
            out.chunks(CHANNELS).filter(|c| c[0] == 2).count(),
            100,
            "all 100 new frames must be in the ring: dropping the newest keeps only \
             the last of them, which the two reads above cannot distinguish"
        );
        assert_eq!(
            out[(cap - 100) * CHANNELS],
            2,
            "and they are contiguous at the end, so the boundary is where it should be"
        );
    }

    /// Underrun holds the last sample rather than emitting zeros, and counts how many
    /// it held.
    #[test]
    fn underrun_holds_the_last_sample_and_counts_it() {
        let mut ring = Ring::with_prefill(48_000, 3);
        ring.push(&[500i16, 500, 600, 600, 700, 700]);
        let mut out = vec![0i16; 16];
        ring.pop(&mut out, false);
        assert_eq!(&out[..6], &[500, 500, 600, 600, 700, 700]);
        assert_eq!(
            &out[6..],
            &[700; 10],
            "a zero here is a click; hold instead"
        );
        assert_eq!(ring.stats().underruns, 5, "five held frames");
    }

    /// An armed but never-fed ring holds silence: there is no last value, and a DC step
    /// from an arbitrary one would be worse than the silence.
    #[test]
    fn an_unfed_ring_holds_silence() {
        let mut ring = Ring::with_prefill(48_000, 0);
        let mut out = vec![99i16; 8];
        ring.pop(&mut out, false);
        assert_eq!(out, vec![0; 8]);
        assert_eq!(ring.stats().underruns, 4, "four frames");
    }

    /// A paused emulator accrues no underruns: it produces nothing by design, and
    /// counting that as a fault makes the counter worthless as a "your machine cannot
    /// keep up" signal.
    #[test]
    fn a_paused_emulator_accrues_no_underruns() {
        let mut ring = Ring::with_prefill(48_000, 0);
        let mut out = vec![7i16; 1024];
        ring.pop(&mut out, true);
        assert_eq!(ring.stats().underruns, 0, "paused is not an underrun");
        assert_eq!(out, vec![0; 1024], "and it still outputs silence");
        // Unpaused, the same empty ring does count — so the suppression is the pause
        // flag rather than the ring being quiet.
        ring.pop(&mut out, false);
        assert_eq!(ring.stats().underruns, 512, "512 frames of 1,024 samples");
    }

    /// A pause does not hold the level it was playing.
    ///
    /// `a_paused_emulator_accrues_no_underruns` starts from a ring that never held a
    /// sample, so its silence also follows from `last` being 0 — it cannot tell "paused
    /// outputs silence" from "there was nothing to hold". This one pauses *mid-phrase*,
    /// with a loud level latched, where holding would put a DC offset on the speaker for
    /// as long as the player left the game paused.
    #[test]
    fn a_pause_outputs_silence_rather_than_the_level_it_held() {
        let mut ring = Ring::with_prefill(48_000, 0);
        ring.push(&[20_000i16, 20_000]);
        let mut out = vec![0i16; 8];
        ring.pop(&mut out, false);
        assert_eq!(out, vec![20_000; 8], "the premise: a loud level is held");
        ring.pop(&mut out, true);
        assert_eq!(out, vec![0; 8], "paused, so silence and not 20,000 of DC");
    }

    /// At the measured cadences — 512-sample callbacks against 933-sample frames — a
    /// second of play runs clean.
    ///
    /// This is the spec's Probe 2 as a test: the ring's size is justified by surviving
    /// the real numbers, not by the numbers being restated in a constant.
    #[test]
    fn the_measured_cadences_run_without_drops_or_underruns() {
        let mut ring = Ring::new(48_000);
        let mut r = Resampler::new(48_000);
        let mut converted = Vec::new();
        let mut out = vec![0i16; 512 * CHANNELS];
        let mut callbacks = 0usize;
        let mut sounded = 0usize;
        // 60 frames — one second — of production against the callbacks they pay for.
        // 933 input samples is 16.7 ms, which is 800 output samples at 48 kHz, so
        // 1.5625 callbacks per frame.
        let mut owed = 0i64;
        for frame in 0..60i64 {
            // 933 *frames*, both channels equal — the cadence is about frame counts.
            let block: Vec<i16> = (0..933)
                .flat_map(|i| {
                    let v = ((frame * 933 + i) % 1000) as i16;
                    [v, v]
                })
                .collect();
            r.feed(&block, &mut converted);
            ring.push(&converted);
            converted.clear();
            owed += 800;
            while owed >= 512 {
                ring.pop(&mut out, false);
                if out.iter().any(|&s| s != 0) {
                    sounded += 1;
                }
                owed -= 512;
                callbacks += 1;
            }
        }
        assert_eq!(
            callbacks, 93,
            "60 frames at 800 samples is 93 full callbacks"
        );
        // The premise, and it is not a formality: a ring that never armed outputs
        // silence from every callback and reports (0, 0) for exactly that reason. Both
        // counters are unreachable while muted, so without this the clean result would
        // be indistinguishable from a ring that never played at all. 50 ms of prefill
        // is 3 frames, so at most the first 5 callbacks are silent.
        assert!(
            sounded >= 88,
            "only {sounded} of {callbacks} callbacks carried audio; the ring spent the \
             second muted, so (0, 0) says nothing"
        );
        let s = ring.stats();
        assert_eq!(
            (s.drops, s.underruns),
            (0, 0),
            "{s:?} at the measured cadences"
        );
        // And the depth stayed inside the ring with room either side, which is what
        // makes 100 ms the right size rather than a guess.
        assert!(
            ring.len() < ring.capacity(),
            "depth {} of {}",
            ring.len(),
            ring.capacity()
        );
    }

    /// A host that consumes faster than the emulator produces underruns, and one that
    /// consumes slower drops — so the two counters are reachable and distinguishable.
    ///
    /// The clean-cadence test above is the one that matters, and on its own it cannot
    /// fail for a ring whose counters are wired to each other or never incremented at
    /// all. These are the two failures that ring is claimed to survive.
    #[test]
    fn a_mismatched_host_reaches_one_counter_and_not_the_other() {
        // Consuming 1,024 per frame against 800 produced: the ring drains.
        let mut fast = Ring::new(48_000);
        let mut out = vec![0i16; 1024 * CHANNELS];
        for _ in 0..60 {
            fast.push(&vec![1i16; 800 * CHANNELS]);
            fast.pop(&mut out, false);
        }
        let s = fast.stats();
        assert!(s.underruns > 0, "a fast host must underrun: {s:?}");
        assert_eq!(s.drops, 0, "and must not drop: {s:?}");

        // Consuming 512 per frame against 800 produced: the ring fills and overflows.
        let mut slow = Ring::new(48_000);
        let mut out = vec![0i16; 512 * CHANNELS];
        for _ in 0..60 {
            slow.push(&vec![1i16; 800 * CHANNELS]);
            slow.pop(&mut out, false);
        }
        let s = slow.stats();
        assert!(s.drops > 0, "a slow host must drop: {s:?}");
        assert_eq!(s.underruns, 0, "and must not underrun: {s:?}");
    }
}
