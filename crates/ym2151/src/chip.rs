//! The whole chip: the register file, eight channels, the LFO, the noise generator,
//! the timers, and the per-sample loop that drives them.
//!
//! # One sample is exactly 64 input clocks
//!
//! ymfm's OPM has `OPERATORS = 32` and `DEFAULT_PRESCALE = 2`, and its sample rate is
//! `clock / (2 * 32)`. At CPS-1's 3.579545 MHz that is 55,930.39 Hz — the *rate* is
//! not an integer, but the *period* is: 64 clocks per sample, exactly. Task 8's
//! scheduler relies on that, which is why [`Ym2151::sample_clocks`] exists rather
//! than a sample-rate constant.
//!
//! # The per-sample order, which differs from the plan's listing
//!
//! The plan lists the noise and LFO first. `fm_engine_base::clock`
//! (`ymfm_fm.ipp:1280`) has them third, after the prepare gate and the envelope
//! counter. This module follows ymfm. The difference is unobservable in the output —
//! nothing before the channel clock reads the LFO, and `cache_operator_data` is
//! handed a PM of 0 either way — but the prepare gate's *timing* depends on running
//! first, and Task 9 measures that timing. Getting it right now means Task 9 changes
//! one condition rather than re-deriving the loop.
//!
//! Timers are clocked before the gate, because a timer A overflow in CSM mode sets a
//! key-on that the same sample's `prepare` must consume. ymfm reaches the same result
//! by a different route: its timers live in the host's scheduler and fire between
//! `clock` calls.
//!
//! # All eight channels are summed, unconditionally
//!
//! ymfm masks the sum by `m_active_channels`. That mask was measured as a pure
//! optimisation — deleting `chanmask &= m_active_channels` changed no sample over
//! 40,000, with CSM both on and off. Summing all eight is simpler and provably
//! identical, so [`Channel::prepare`]'s activity result feeds the debugger instead.
//!
//! # The DAC roundtrip is not optional
//!
//! CPS-1 pairs the YM2151 with a YM3012 DAC, which carries a 10-bit mantissa and a
//! 3-bit exponent. [`roundtrip_fp`] is that quantisation, and it is why the
//! reference's output fits `i16` at all. Skipping it leaves every sample above ±512
//! differing from the suite — the measured algorithm peaks (8176, 16352, 24512,
//! 32704) are all fixed points of it, which is why the channel tests' numbers are
//! those exact values and not powers of two.

use crate::channel::Channel;
use crate::lfo::Lfo;
use crate::noise::Noise;
use crate::operator::KEYON_CSM;
use crate::regs::{key_on_fields, Regs, CHANNEL_COUNT, REG_KEY_ON, REG_MODE};
use crate::timer::Timers;

/// The OPM's envelope clock divider: the envelope advances every third sample.
///
/// `ymfm_opm.h:121`. The counter is advanced so that values congruent to 3 mod 4 are
/// skipped, which makes `counter & 3 == 0` — the operator's envelope tick — land once
/// per three samples while still handing the operator a counter whose upper bits are a
/// plain tick count. See [`advance_env_counter`].
pub const EG_CLOCK_DIVIDER: u32 = 3;

/// Every channel marked stale: `ymfm_fm.h`'s `ALL_CHANNELS` for an eight-channel chip.
const ALL_CHANNELS: u8 = 0xFF;

/// Advance the envelope counter by one sample, skipping the unused phase.
///
/// ymfm: `else if (bitfield(++m_env_counter, 0, 2) == EG_CLOCK_DIVIDER) m_env_counter
/// += 4 - EG_CLOCK_DIVIDER;`. The sequence is 1, 2, 4, 5, 6, 8, … — every value
/// congruent to 3 mod 4 is stepped over, so one sample in three has the low two bits
/// clear. A port that simply divided by three would hand the operator a counter three
/// times too small and every envelope rate would be wrong by a factor of three.
#[must_use]
pub fn advance_env_counter(counter: u32) -> u32 {
    let next = counter.wrapping_add(1);
    if next & 3 == EG_CLOCK_DIVIDER {
        next.wrapping_add(4 - EG_CLOCK_DIVIDER)
    } else {
        next
    }
}

/// The YM3012 DAC's floating-point roundtrip, `ymfm.h:227`.
///
/// A 10-bit mantissa with a 3-bit exponent: the value is masked down to its top ten
/// significant bits, so quiet samples pass through untouched and loud ones lose their
/// low bits. Saturating rather than wrapping at both ends.
#[must_use]
pub fn roundtrip_fp(value: i32) -> i16 {
    if value < -32768 {
        return -32768;
    }
    if value > 32767 {
        return 32767;
    }

    // The magnitude, via the branchless absolute value ymfm uses: `x ^ (x >> 31)` is
    // `!x` for a negative x, which is `-x - 1` — one less than the true magnitude, and
    // that is deliberate. It is what makes -513 and 513 quantise to the same exponent.
    let scanvalue = value ^ (value >> 31);

    // The shift is done in `u32` because ymfm shifts an `int32_t` left into the sign
    // bit, which is undefined in C++ and wrapping here. `leading_zeros` of 0 is 32,
    // matching ymfm's `count_leading_zeros`.
    let exponent = 7 - ((scanvalue as u32) << 17).leading_zeros() as i32;
    let exponent = exponent.max(1) - 1;
    let mask = (1i32 << exponent) - 1;
    (value & !mask) as i16
}

/// A complete YM2151.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ym2151 {
    /// The 256-byte register file.
    pub regs: Regs,
    /// The eight FM channels, four operators each.
    pub channels: [Channel; CHANNEL_COUNT as usize],
    /// The low-frequency oscillator.
    pub lfo: Lfo,
    /// The noise generator.
    pub noise: Noise,
    /// The two timers and the status register.
    pub timers: Timers,
    /// The envelope counter, advanced by [`advance_env_counter`] each sample.
    env_counter: u32,
    /// One bit per channel whose cached data is stale. Post-reset it is all eight —
    /// `ymfm_fm.ipp:1190` constructs `m_modified_channels(ALL_CHANNELS)`, so the
    /// first sample always prepares.
    modified_channels: u8,
    /// Samples since the last `prepare`, for the every-4,096 half of the gate.
    ///
    /// Only advanced on samples where nothing was modified: ymfm's `++` sits on the
    /// right of a `||`, so a chip being written to every sample never increments it.
    /// See [`Ym2151::clock_sample`].
    prepare_counter: u32,
    /// Test-only: prepare every sample, the way Task 5 did.
    ///
    /// The measured divergence cannot be asserted from inside this crate any other
    /// way, and the alternative — asserting ymfm's hashes — would make the test depend
    /// on a register script instead of on the property. Not compiled outside tests.
    #[cfg(any(test, feature = "internals"))]
    force_eager_prepare: bool,
    /// Test-only: how many times the gate has opened.
    #[cfg(any(test, feature = "internals"))]
    prepare_count: u32,
}

impl Default for Ym2151 {
    fn default() -> Self {
        Self::new()
    }
}

impl Ym2151 {
    /// A chip in its post-reset state.
    ///
    /// Constructed from the field defaults and then [`Ym2151::reset`]. Building it
    /// this way is what keeps `reset_restores_the_constructed_state_exactly` from
    /// becoming a claim that cannot fail: a field the literal initialises but `reset`
    /// forgets still shows up as a difference after the chip has been used.
    #[must_use]
    pub fn new() -> Self {
        let mut chip = Self {
            regs: Regs::new(),
            channels: [Channel::new(); CHANNEL_COUNT as usize],
            lfo: Lfo::new(),
            noise: Noise::new(),
            timers: Timers::new(),
            env_counter: 0,
            modified_channels: ALL_CHANNELS,
            prepare_counter: 0,
            #[cfg(any(test, feature = "internals"))]
            force_eager_prepare: false,
            #[cfg(any(test, feature = "internals"))]
            prepare_count: 0,
        };
        chip.reset();
        chip
    }

    /// Test-only: prepare every sample, as Task 5's core did.
    ///
    /// Exists to assert the divergence the gate causes — see
    /// `with_csm_on_eager_and_lazy_diverge`. The suite runner must never call this:
    /// `testrunner` runs the real chip.
    #[cfg(any(test, feature = "internals"))]
    pub fn set_force_eager_prepare_for_test(&mut self, eager: bool) {
        self.force_eager_prepare = eager;
    }

    /// Test-only: how many times the prepare gate has opened.
    #[cfg(any(test, feature = "internals"))]
    #[must_use]
    pub fn prepare_count_for_test(&self) -> u32 {
        self.prepare_count
    }

    /// How many input clocks one sample takes: `OPERATORS * DEFAULT_PRESCALE`.
    #[must_use]
    pub fn sample_clocks() -> u32 {
        32 * 2
    }

    /// Return the chip to its post-reset state.
    ///
    /// The write to [`REG_MODE`] is explicit and is not redundant with clearing the
    /// register file: ymfm's comment is "explicitly write to the mode register since
    /// it has side-effects" — it is what cancels any running timer.
    pub fn reset(&mut self) {
        self.timers.reset();
        self.regs.reset();
        self.write(REG_MODE, 0);
        for ch in &mut self.channels {
            ch.reset();
        }
        self.lfo.reset();
        self.noise.reset();
        self.env_counter = 0;
        // Every channel is stale again, matching the constructor: the first sample
        // after a reset must prepare, or a channel written before the reset would keep
        // a cache the register file no longer backs.
        self.modified_channels = ALL_CHANNELS;
        self.prepare_counter = 0;
        // The diagnostic counter too, so `reset_restores_the_constructed_state_exactly`
        // compares the whole struct rather than the whole struct minus an exception.
        #[cfg(any(test, feature = "internals"))]
        {
            self.prepare_count = 0;
        }
    }

    /// The status register as the guest reads it.
    ///
    /// BUSY is never set — see [`crate::timer`]'s module docs — and the OPM's
    /// `STATUS_IRQ` is 0, so the IRQ line is [`Ym2151::irq`] rather than a bit here.
    #[must_use]
    pub fn read_status(&self) -> u8 {
        self.timers.status()
    }

    /// Whether the chip is asserting its IRQ line.
    #[must_use]
    pub fn irq(&self) -> bool {
        self.timers.irq()
    }

    /// Write one register.
    ///
    /// Every address is writable and no value is invalid: the OPM has no error
    /// states. Two addresses do more than store a byte — [`REG_MODE`] reloads the
    /// timers and [`REG_KEY_ON`] records a key edge.
    pub fn write(&mut self, addr: u8, val: u8) {
        self.regs.write(addr, val);

        // "for now just mark all channels as modified" — `ymfm_fm.ipp:1412`, and the
        // mode-register path at `:1563` does the same. Every write, not only the ones
        // that change a cached field: a per-register map would be an optimisation, and
        // the gate is semantics, so a wrong map would silently eat CSM key-ons.
        self.modified_channels = ALL_CHANNELS;

        if addr == REG_MODE {
            self.timers.write_mode(val, &self.regs);
        } else if addr == REG_KEY_ON {
            let (ch, mask) = key_on_fields(val);
            for slot in 0..4 {
                // The mask bit index is a *slot* index, not a register-operator
                // index — see `channel`'s module docs. Bits 3-6 of the written byte
                // therefore reach register offsets 0x00, 0x10, 0x08, 0x18.
                let on = mask & (1 << slot) != 0;
                self.channels[ch as usize].ops[slot].set_keyon(on, crate::operator::KEYON_NORMAL);
            }
        }
    }

    /// Render samples into a caller-supplied slice.
    ///
    /// The chip owns no buffer and no audio device; an empty slice is a no-op that
    /// does not advance any state.
    pub fn generate(&mut self, out: &mut [(i16, i16)]) {
        for sample in out.iter_mut() {
            *sample = self.clock_sample();
        }
    }

    /// Whether the test-only eager override is on. Always false in a release build.
    fn force_eager_prepare(&self) -> bool {
        #[cfg(any(test, feature = "internals"))]
        {
            self.force_eager_prepare
        }
        #[cfg(not(any(test, feature = "internals")))]
        {
            false
        }
    }

    /// Advance one sample and return it.
    fn clock_sample(&mut self) -> (i16, i16) {
        // 1. The timers. A timer A overflow in CSM mode key-ons every channel, and
        //    that key-on must be consumed by this sample's `prepare`.
        let events = self.timers.clock(&self.regs);
        if events.timer_a_overflow && self.regs.csm() {
            // `CSM_TRIGGER_MASK` is `ALL_CHANNELS` for the OPM (`ymfm_opm.h:119`), so
            // every channel is keyed on and every channel is marked modified —
            // `ymfm_fm.ipp:1516-1522`. The mark is what opens the gate below on this
            // sample, which is the entire mechanism: without it the key-on sits in
            // `keyon_live` until some later write happens to open the gate, by which
            // time the trigger it represented is long past.
            for ch in &mut self.channels {
                for op in &mut ch.ops {
                    op.set_keyon(true, KEYON_CSM);
                }
            }
            self.modified_channels = ALL_CHANNELS;
        }

        // 2. The gate. This is *semantics*, not an optimisation, because
        //    `Operator::prepare` consumes the CSM key-on bit (`ymfm_fm.ipp:434`):
        //    preparing every sample clears the flag one sample after the timer set it,
        //    before the key-state clock ever sees it. Measured over 40,000 samples —
        //    with CSM off, eager and lazy agree bit for bit (fnv `bfc97b4fa40cfcf1`,
        //    575 non-silent both ways); with CSM on they diverge, stock ymfm giving
        //    `322d488e3f59bdb5` with 39,737 non-silent samples against eager's
        //    `ffbdd8b77349c3d5` with 15,775.
        //
        //    The counter is only advanced when nothing was modified. ymfm writes
        //    `m_modified_channels != 0 || m_prepare_count++ >= 4096`, and `||`
        //    short-circuits: a chip written to every sample never increments it. That
        //    is why this is not `prepare_counter % 4_096 == 0` on a counter advanced
        //    unconditionally — the two rules differ on exactly the traffic a real
        //    driver produces.
        let eager = self.force_eager_prepare();
        let due = if self.modified_channels != 0 {
            true
        } else {
            let count = self.prepare_counter;
            self.prepare_counter = count.wrapping_add(1);
            count >= 4_096
        };
        if eager || due {
            for ch in 0..CHANNEL_COUNT {
                self.channels[ch as usize].prepare(&self.regs, ch);
            }
            self.modified_channels = 0;
            self.prepare_counter = 0;
            #[cfg(any(test, feature = "internals"))]
            {
                self.prepare_count = self.prepare_count.wrapping_add(1);
            }
        }

        // 3. The envelope counter. Every operator checks its low two bits, so this
        //    is what makes the envelope advance once per three samples.
        self.env_counter = advance_env_counter(self.env_counter);

        // 4. The noise generator, then the LFO — in that order, because the LFO's
        //    waveform 3 is fed the byte the LFSR holds *after* this sample's shifts.
        let noise_state = self.noise.clock(self.regs.noise_period());
        let noise_byte = self.noise.lfo_byte();
        let (_am, lfo_pm) = self.lfo.clock(&self.regs, noise_byte);

        // 5. Every channel's operators: envelope where due, phase every sample.
        for ch in 0..CHANNEL_COUNT {
            self.channels[ch as usize].clock(&self.regs, ch, self.env_counter, lfo_pm);
        }

        // 6. Sum all eight channels — not only the active ones.
        let mut left = 0i32;
        let mut right = 0i32;
        for ch in 0..CHANNEL_COUNT {
            let am_offset = self.lfo.am_offset(&self.regs, ch);
            let (l, r) = self.channels[ch as usize].output(&self.regs, ch, am_offset, noise_state);
            left += l;
            right += r;
        }

        // 7. The DAC roundtrip, which also saturates into `i16`.
        (roundtrip_fp(left), roundtrip_fp(right))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One sample is exactly 64 input clocks.
    ///
    /// `sample_rate(3'579'545)` in ymfm is 3,579,545 / (2 * 32) = 55,930, and the
    /// division is exact. Task 8's scheduler depends on this being an integer.
    #[test]
    fn one_sample_is_sixty_four_input_clocks() {
        assert_eq!(Ym2151::sample_clocks(), 64);
        assert_eq!(3_579_545 / 64, 55_930);
        assert_eq!(3_579_545 % 64, 25, "the sample rate itself is not exact");
    }

    /// A freshly reset chip is silent, and every register write is accepted.
    ///
    /// The OPM has no invalid addresses and no error states: this sweeps all 256 of
    /// them with all 256 values and asserts only that nothing panics. It is a
    /// robustness test, not a behaviour test — the guest can write anything.
    #[test]
    fn every_address_and_value_is_accepted_without_panicking() {
        let mut chip = Ym2151::new();
        let mut buf = [(0i16, 0i16); 4];
        for addr in 0..=255u8 {
            for val in 0..=255u8 {
                chip.write(addr, val);
                chip.generate(&mut buf);
            }
        }
    }

    /// A reset chip with no writes produces silence.
    #[test]
    fn silence_before_any_write() {
        let mut chip = Ym2151::new();
        let mut buf = [(0i16, 0i16); 1024];
        chip.generate(&mut buf);
        assert!(buf.iter().all(|&(l, r)| l == 0 && r == 0));
    }

    /// `reset` returns the chip to exactly its constructed state.
    ///
    /// `PartialEq` over the whole struct, so a field added later without a reset is
    /// caught here rather than by a save-state divergence three sub-projects later.
    #[test]
    fn reset_restores_the_constructed_state_exactly() {
        let fresh = Ym2151::new();
        let mut chip = Ym2151::new();
        chip.write(0x20, 0xC7);
        chip.write(0x28, 0x4A);
        chip.write(0x08, 0x78);
        let mut buf = [(0i16, 0i16); 256];
        chip.generate(&mut buf);
        assert_ne!(chip, fresh, "it did something");
        chip.reset();
        assert_eq!(chip, fresh, "and reset undid all of it");
    }

    /// `generate` is a pure function of state: two chips in the same state agree.
    ///
    /// The determinism premise the whole emulator rests on, asserted at the chip
    /// boundary where a stray global or an uninitialised field would show up.
    ///
    /// **The plan's register patch made this test's last assertion unsatisfiable.**
    /// It wrote MUL and AR for register-operator 0 only, leaving the other three at
    /// attack rate 0 — "never attack" — and algorithm 4's carriers are the operators
    /// at offsets 0x10 and 0x18, both of which were therefore silent. Measured
    /// against ymfm: 0 non-zero samples of 2,048, peak 0, so `any(|l| l != 0)` could
    /// not hold. Configuring all four operators measures 2,048 non-zero samples and a
    /// peak of 16,352.
    #[test]
    fn two_chips_in_the_same_state_generate_the_same_samples() {
        let mut a = Ym2151::new();
        a.write(0x20, 0xC4);
        a.write(0x28, 0x4A);
        for op in 0..4u8 {
            let off = op * 8;
            a.write(0x40 + off, 0x01);
            a.write(0x80 + off, 0x1F);
        }
        a.write(0x08, 0x78);

        let mut b = a.clone();
        let mut ba = [(0i16, 0i16); 2048];
        let mut bb = [(0i16, 0i16); 2048];
        a.generate(&mut ba);
        b.generate(&mut bb);
        assert_eq!(ba, bb);
        assert_eq!(a, b, "and their states still match afterwards");
        assert!(ba.iter().any(|&(l, _)| l != 0), "and it was not silence");
    }

    /// `generate` into an empty slice is a no-op that does not advance the chip.
    #[test]
    fn generating_zero_samples_does_not_advance_the_chip() {
        let mut chip = Ym2151::new();
        chip.write(0x20, 0xC7);
        chip.write(0x08, 0x78);
        let before = chip.clone();
        chip.generate(&mut []);
        assert_eq!(chip, before);
    }

    /// The envelope counter skips one phase in four, so envelopes tick every third
    /// sample.
    ///
    /// The plan had no test for the divider, and the whole envelope's speed rides on
    /// it: a port that clocked the envelope on every sample runs every note three
    /// times too fast, and one that divided the counter by three instead of skipping
    /// hands the operator a counter three times too small, which shifts every rate.
    /// Both are audible and neither is visible in any per-algorithm test.
    #[test]
    fn the_envelope_counter_skips_one_phase_in_four() {
        let mut counter = 0u32;
        let mut seen = vec![];
        for _ in 0..12 {
            counter = advance_env_counter(counter);
            seen.push(counter);
        }
        assert_eq!(seen, vec![1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, 16]);

        // One sample in three has the low two bits clear — that is the operator's
        // envelope tick — and the tick number it derives is a plain 1, 2, 3, ...
        let ticks: Vec<u32> = seen
            .iter()
            .filter(|c| *c & 3 == 0)
            .map(|c| c >> 2)
            .collect();
        assert_eq!(ticks, vec![1, 2, 3, 4], "four ticks in twelve samples");
        assert!(
            seen.iter().all(|c| c & 3 != EG_CLOCK_DIVIDER),
            "the skipped phase never appears: {seen:?}"
        );
    }

    /// The DAC roundtrip is an identity below ±513 and quantises above it.
    ///
    /// Every one of these is measured against ymfm's `roundtrip_fp`, and the boundary
    /// is the assertion that matters: a port that always masked would quantise quiet
    /// samples the reference passes through, and one that never masked would leave
    /// every loud sample differing from the suite. The four algorithm peaks are fixed
    /// points, which is why the channel tests assert those exact numbers.
    #[test]
    fn the_dac_roundtrip_quantises_only_above_five_hundred_and_twelve() {
        for v in -512..=512i32 {
            assert_eq!(roundtrip_fp(v), v as i16, "identity at {v}");
        }
        assert_eq!(roundtrip_fp(513), 512, "the first value that loses a bit");
        assert_eq!(roundtrip_fp(-513), -514, "and its negative counterpart");
        assert_eq!(roundtrip_fp(1023), 1022);
        assert_eq!(roundtrip_fp(16383), 16352);
        assert_eq!(roundtrip_fp(32767), 32704);
        assert_eq!(roundtrip_fp(40000), 32767, "saturates rather than wrapping");
        assert_eq!(roundtrip_fp(-40000), -32768);

        for peak in [8176i32, 16352, 24512, 32704] {
            assert_eq!(roundtrip_fp(peak), peak as i16, "{peak} is a fixed point");
        }
    }

    /// Four patches, hashed, against figures measured from real ymfm.
    ///
    /// Every test above this one asserts a *relation* — this peak exceeds that one,
    /// these eight hashes differ. Relations are what make a failure legible, but a
    /// port can satisfy all of them and still be wrong in a way that only a
    /// sample-for-sample comparison sees. These four hashes were produced by a C++
    /// program linking ymfm (the implementation MAME uses) and rendering the same
    /// register scripts, so they pin 3,072 samples of exact agreement.
    ///
    /// The four were chosen for coverage rather than variety: between them they
    /// exercise the algorithm table and the slot map (1), feedback with LFO AM and PM
    /// and asymmetric operators (2), the noise generator on channel 7 with a second
    /// channel sounding at once (3), and both timers running with a key-off partway
    /// so the release rate is reached (4). Patch 2's peak is 473, below the DAC's
    /// quantisation threshold, so it also pins the *identity* half of
    /// [`roundtrip_fp`] while the other three pin the masking half.
    ///
    /// Task 7's generated suite supersedes this in breadth. It is kept because it
    /// needs no fetched artefact and no generator, so it fails on the same `cargo
    /// test` run that a Task 9 mistake would otherwise pass.
    #[test]
    fn four_patches_match_ymfm_sample_for_sample() {
        // Two measurements that shaped patch 2, recorded because both were surprises:
        // AM depth `0x6F` at AM sensitivity 3 clamps the carrier to silence for the
        // whole 1,024-sample window — 0 non-zero samples — so the depth is `0x18` and
        // the sensitivity 1. A patch that renders silence pins nothing.
        let cases = [
            YmfmCase {
                name: "alg4-all",
                writes: patch_alg4(),
                samples: 512,
                hash: 0x2c37_9c97_d95e_0229,
                nonzero: 512,
                peak: 16352,
            },
            YmfmCase {
                name: "alg0-lfo-fb",
                writes: patch_lfo_feedback(),
                samples: 1024,
                hash: 0x702f_37f1_dc07_eb6d,
                nonzero: 1022,
                peak: 473,
            },
            YmfmCase {
                name: "noise-ch7",
                writes: patch_noise(),
                samples: 1024,
                hash: 0x787b_7180_1d8c_4ebd,
                nonzero: 1024,
                peak: 17600,
            },
            YmfmCase {
                name: "timers-release",
                writes: patch_timers(),
                samples: 512,
                hash: 0xa8de_4b73_d244_b295,
                nonzero: 512,
                peak: 32704,
            },
        ];

        for case in cases {
            let mut chip = Ym2151::new();
            for (addr, val) in case.writes {
                chip.write(addr, val);
            }
            let mut flat = Vec::with_capacity(case.samples * 2);
            let mut buf = [(0i16, 0i16); 1];
            for i in 0..case.samples {
                // Patch 4 keys off halfway, which is the only way the release rate is
                // reached at all: the spec measured RR as undetected in 0 of 200
                // generated cases until every case keyed off.
                if case.name == "timers-release" && i == 256 {
                    chip.write(0x08, 0x00);
                }
                chip.generate(&mut buf);
                flat.push(buf[0].0 as u16);
                flat.push(buf[0].1 as u16);
            }

            // The non-silence and peak premises come first: a hash comparison against
            // an all-zero buffer is a claim that cannot fail, and three of the four
            // scripts above were rejected during development for rendering silence.
            let nonzero = flat.chunks(2).filter(|c| c[0] != 0).count();
            let peak = flat
                .chunks(2)
                .map(|c| i32::from(c[0] as i16).abs())
                .max()
                .unwrap();
            let name = case.name;
            assert_eq!(nonzero, case.nonzero, "{name}: non-silent sample count");
            assert_eq!(peak, case.peak, "{name}: peak");
            assert_eq!(
                crate::tables::fnv1a_u16(&flat),
                case.hash,
                "{name}: every sample matches ymfm"
            );
        }
    }

    /// One patch and the four figures real ymfm produced for it.
    struct YmfmCase {
        /// For the assertion messages, and to select the key-off case.
        name: &'static str,
        /// `(address, value)` in the order the script writes them.
        writes: Vec<(u8, u8)>,
        /// How many stereo samples to render.
        samples: usize,
        /// FNV-1a over every sample's little-endian bytes, left then right.
        hash: u64,
        /// Non-silent left samples — the premise that stops the hash being vacuous.
        nonzero: usize,
        /// Largest absolute left sample, which pins the DAC's quantisation.
        peak: i32,
    }

    /// Algorithm 4 with all four operators at MUL 1 and the fastest attack.
    fn patch_alg4() -> Vec<(u8, u8)> {
        let mut w = vec![(0x20u8, 0xC4u8), (0x28, 0x4A)];
        for op in 0..4u8 {
            let off = op * 8;
            w.extend([
                (0x40 + off, 0x01),
                (0x60 + off, 0x00),
                (0x80 + off, 31),
                (0xA0 + off, 0),
                (0xC0 + off, 0),
                (0xE0 + off, 0),
            ]);
        }
        w.push((0x08, 0x78));
        w
    }

    /// Algorithm 0 with feedback 5, four different operators, and the LFO running.
    fn patch_lfo_feedback() -> Vec<(u8, u8)> {
        let mut w = vec![(0x20u8, 0xC0 | (5 << 3)), (0x28, 0x52), (0x30, 0x40)];
        for op in 0..4u8 {
            let off = op * 8;
            w.extend([
                (0x40 + off, 0x11 * (op + 1)),
                (0x60 + off, op * 9),
                (0x80 + off, 0x1F - op),
                (0xA0 + off, 0x80 | (op * 3)), // AM enable set on every operator
                (0xC0 + off, (op << 6) | op),
                (0xE0 + off, (op << 4) | (15 - op)),
            ]);
        }
        w.extend([
            (0x18, 0x9A), // LFO rate
            (0x19, 0x18), // AM depth — see the test's comment on why not 0x6F
            (0x19, 0xF0), // PM depth 0x70
            (0x1B, 0x02), // triangle
            (0x38, 0x71), // PM sensitivity 7, AM sensitivity 1
            (0x08, 0x78),
        ]);
        w
    }

    /// Channels 5 and 7 sounding together with the noise generator on.
    fn patch_noise() -> Vec<(u8, u8)> {
        let mut w = vec![
            (0x0Fu8, 0x94u8), // noise enable, frequency 0x14
            (0x27, 0xC7),
            (0x2F, 0x40),
            (0x25, 0xC2),
            (0x28 + 7, 0x4A),
            (0x28 + 5, 0x3C),
        ];
        for op in 0..4u8 {
            for ch in [5u8, 7] {
                let off = op * 8 + ch;
                w.extend([
                    (0x40 + off, 0x01 + op),
                    (0x60 + off, op * 5),
                    (0x80 + off, 0x1F),
                    (0xA0 + off, op * 2),
                    (0xC0 + off, 0),
                    (0xE0 + off, 0x08),
                ]);
            }
        }
        w.extend([(0x08, 0x7D), (0x08, 0x7F)]);
        w
    }

    /// Both timers loaded and enabled, with a decay and release worth reaching.
    fn patch_timers() -> Vec<(u8, u8)> {
        let mut w = vec![(0x20u8, 0xC7u8), (0x28, 0x4A)];
        for op in 0..4u8 {
            let off = op * 8;
            w.extend([
                (0x40 + off, 0x01),
                (0x60 + off, 0x00),
                (0x80 + off, 0x1F),
                (0xA0 + off, 0x05),
                (0xC0 + off, 0x03),
                (0xE0 + off, 0x24),
            ]);
        }
        w.extend([
            (0x10, 0x30),
            (0x11, 0x02),
            (0x12, 0xF0),
            (0x14, 0x0F), // load and enable both timers
            (0x08, 0x78),
        ]);
        w
    }

    /// A key-on write reaches the four slots in slot order, not register order.
    ///
    /// Measured against ymfm: bits 3, 4, 5, 6 of `0x08` reach register offsets 0x00,
    /// 0x10, 0x08, 0x18. `keyonoff` indexes its mask by `opnum` over the
    /// slot-ordered operator array, so the mask bit *is* a slot — a port that treated
    /// it as a register-operator index keys the wrong two of the four operators, and
    /// every test that keys all four at once passes anyway.
    #[test]
    fn a_key_on_mask_bit_names_a_slot_not_a_register_operator() {
        for (bit, want_slot) in [(3u8, 0usize), (4, 1), (5, 2), (6, 3)] {
            let mut chip = Ym2151::new();
            chip.write(0x08, 1 << bit);
            for slot in 0..4 {
                let live = chip.channels[0].ops[slot].keyon_live != 0;
                assert_eq!(live, slot == want_slot, "bit {bit} keys slot {want_slot}");
            }
        }
        // And the channel field is the low three bits, so the same mask on channel 5
        // leaves channel 0 alone.
        let mut chip = Ym2151::new();
        chip.write(0x08, 0x78 | 5);
        assert!(chip.channels[5].ops.iter().all(|op| op.keyon_live != 0));
        assert!(chip.channels[0].ops.iter().all(|op| op.keyon_live == 0));
    }

    /// A patch whose only note source is CSM: no key-on write at all.
    ///
    /// Decay and sustain rates fast enough that the note dies well inside timer A's
    /// period, so the two readings of the gate differ in *how much* sound comes out
    /// rather than only in its phase. With the CSM bit clear this patch is silent by
    /// construction — nothing ever keys it on — which is what makes it a clean probe
    /// of the CSM path and why `render_csm(_, false)` is not usable as a control.
    fn csm_patch(chip: &mut Ym2151, csm: bool) {
        chip.write(0x20, 0xC7); // channel 0, algorithm 7 — all four to the output
        for op in 0..4u8 {
            let off = op * 8;
            chip.write(0x40 + off, 0x01); // detune/multiple
            chip.write(0x60 + off, 0); // total level: full volume
            chip.write(0x80 + off, 31); // attack rate: instant
            chip.write(0xA0 + off, 20); // first decay
            chip.write(0xC0 + off, 20); // second decay
            chip.write(0xE0 + off, 0xFF); // D1L max, release fast
        }
        chip.write(0x28, 0x4A); // key code
                                // Timer A value 1000 — period 24 samples. Written as the hardware sees it:
                                // the top 8 bits at 0x10 and the low 2 at 0x11.
        chip.write(0x10, (1000 >> 2) as u8);
        chip.write(0x11, (1000 & 3) as u8);
        chip.write(0x14, if csm { 0x81 } else { 0x01 });
    }

    /// Renders a CSM patch and returns (non-silent samples, fnv of the whole buffer).
    fn render_csm(force_eager: bool, csm: bool) -> (usize, u64) {
        let mut chip = Ym2151::new();
        chip.set_force_eager_prepare_for_test(force_eager);
        csm_patch(&mut chip, csm);
        let mut buf = vec![(0i16, 0i16); 40_000];
        chip.generate(&mut buf);
        let flat: Vec<u16> = buf
            .iter()
            .flat_map(|&(l, r)| [l as u16, r as u16])
            .collect();
        (
            buf.iter().filter(|&&(l, r)| l != 0 || r != 0).count(),
            crate::tables::fnv1a_u16(&flat),
        )
    }

    /// A CSM trigger reaches the key state, so a CSM-only patch makes sound at all.
    ///
    /// `prepare()` is what carries a CSM key-on into the envelope: `fm_operator::prepare`
    /// calls `clock_keystate(m_keyon_live != 0)` and only *then* clears the CSM bit
    /// (`ymfm_fm.ipp:425-436`), and `engine_timer_expired` marks every channel modified
    /// alongside setting the bit (`:1516-1522`) so the gate opens on the trigger's own
    /// sample. A core that keyed on without marking the channel would leave the flag
    /// sitting in `keyon_live` until some unrelated write opened the gate, and this
    /// patch — which never writes `0x08` at all — would be silent.
    ///
    /// **Measured, and corroborated by the suite at 1,000/1,000 including 125 CSM
    /// cases:** sound starts at sample 23, which is timer A's first overflow for the
    /// period this patch loads, and 6,148 of 40,000 samples are non-silent.
    #[test]
    fn csm_key_on_survives_to_be_acted_on() {
        let (non_silent, _) = render_csm(false, true);
        assert!(
            non_silent > 1_000,
            "a CSM-only patch must sound: {non_silent} of 40,000 non-silent"
        );
        // And the same patch with the CSM bit clear is silent, which is what makes the
        // sound above attributable to CSM rather than to a stray key-on.
        assert_eq!(
            render_csm(false, false).0,
            0,
            "without the CSM bit nothing ever keys this patch on"
        );
    }

    /// With CSM off, the gate makes no difference — which is why it is easy to miss.
    ///
    /// The contrast that makes the test above meaningful. If a core is silently eager,
    /// this test still passes and only the CSM tests fail; that asymmetry is exactly
    /// why the generated suite needs CSM cases.
    #[test]
    fn without_csm_the_gate_is_invisible() {
        let render = |force_eager: bool| {
            let mut chip = Ym2151::new();
            chip.set_force_eager_prepare_for_test(force_eager);
            chip.write(0x20, 0xC7);
            for op in 0..4u8 {
                let off = op * 8;
                chip.write(0x40 + off, 0x01);
                chip.write(0x60 + off, 0);
                chip.write(0x80 + off, 31);
                chip.write(0xA0 + off, 8);
                chip.write(0xC0 + off, 8);
                chip.write(0xE0 + off, 0x0F);
            }
            chip.write(0x28, 0x4A);
            chip.write(0x08, 0x78);
            let mut buf = vec![(0i16, 0i16); 40_000];
            chip.generate(&mut buf);
            let flat: Vec<u16> = buf
                .iter()
                .flat_map(|&(l, r)| [l as u16, r as u16])
                .collect();
            (
                crate::tables::fnv1a_u16(&flat),
                buf.iter().filter(|&&(l, r)| l != 0 || r != 0).count(),
            )
        };
        let lazy = render(false);
        assert_eq!(lazy, render(true), "identical with CSM off");
        // And not identically silent: two silent renders agree trivially, which would
        // make the equality above a claim that cannot fail.
        assert!(lazy.1 > 0, "the patch must make sound: {lazy:?}");
    }

    /// And with CSM on, the two readings diverge — which is the whole point of the gate.
    ///
    /// **The direction here is the opposite of what the plan predicted, and the suite
    /// settled it.** The plan expected eager preparation to *eat* CSM triggers and so
    /// produce less sound. Measured, and confirmed by the vector suite passing
    /// 1,000/1,000 with the lazy reading, it is the reverse: 6,148 non-silent samples
    /// lazily against 39,977 eagerly.
    ///
    /// The reason is the *key-off* half. `prepare()` clocks the key state from
    /// `keyon_live` and then clears the CSM bit, so an eager chip sees `true` on the
    /// trigger's sample and `false` on the next — every trigger is a complete key
    /// cycle, and `start_attack` re-zeroes the phase 1,666 times over this window,
    /// giving continuous sound. Under the gate the chip only prepares when a channel is
    /// marked, which for this patch is only on trigger samples; the key state therefore
    /// goes true once and stays true, `clock_keystate` returns early, and the note
    /// attacks once and then decays to silence between triggers. Eager is louder, and
    /// wrong.
    ///
    /// **A core where this test passes trivially — where both readings agree — has no
    /// gate**, which is the failure mode this whole task exists to prevent. Both the
    /// count and the hash are compared, since a core could match the count by rendering
    /// different audio of the same density.
    #[test]
    fn with_csm_on_eager_and_lazy_diverge() {
        let lazy = render_csm(false, true);
        let eager = render_csm(true, true);
        assert_ne!(lazy.1, eager.1, "the two readings must not agree");
        assert!(
            eager.0 > lazy.0 * 2,
            "eager re-attacks on every trigger: lazy {lazy:?}, eager {eager:?}"
        );
    }

    /// `prepare()` runs at least every 4,096 samples even with nothing modified.
    ///
    /// The second half of the gate condition, and it is not decorative: it is what
    /// re-caches operator data on a chip nobody is writing to. A core with only the
    /// modified-channels half never re-caches at all.
    #[test]
    fn prepare_runs_at_least_every_four_thousand_ninety_six_samples() {
        let mut chip = Ym2151::new();
        chip.write(0x20, 0xC7);
        for op in 0..4u8 {
            let off = op * 8;
            chip.write(0x40 + off, 0x01);
            chip.write(0x80 + off, 31);
            chip.write(0xE0 + off, 0x0F);
        }
        chip.write(0x28, 0x4A);
        chip.write(0x08, 0x78);
        // The writes above leave one channel marked, so sample 1 prepares. From there
        // nothing is modified, so the periodic half must fire: 4,096 idle samples
        // after that first prepare.
        let mut buf = [(0i16, 0i16); 4_098];
        chip.generate(&mut buf);
        assert_eq!(
            chip.prepare_count_for_test(),
            2,
            "the write's prepare, then the periodic one"
        );
    }

    /// The gate is a gate: a quiet chip does not prepare every sample.
    ///
    /// Without this, a core that ignored `modified_channels` and prepared
    /// unconditionally would pass `prepare_runs_at_least_every_...` — which only
    /// asserts a floor — and every CSM test would then fail for a reason the counter
    /// could have named directly.
    #[test]
    fn an_unwritten_chip_does_not_prepare_every_sample() {
        let mut chip = Ym2151::new();
        let mut buf = [(0i16, 0i16); 1_000];
        chip.generate(&mut buf);
        assert_eq!(
            chip.prepare_count_for_test(),
            1,
            "only the post-reset prepare in 1,000 samples"
        );

        // And a write re-opens it exactly once.
        chip.write(0x20, 0xC7);
        chip.generate(&mut buf);
        assert_eq!(chip.prepare_count_for_test(), 2, "the write's prepare");
    }
}
