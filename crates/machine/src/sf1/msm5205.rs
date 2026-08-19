//! The MSM5205, Street Fighter's ADPCM chip — a CPU-fed decoder, not a sample
//! player.
//!
//! Transcribed from MAME's `msm5205.cpp` (BSD-3, (C) Aaron Giles) at tag
//! `mame0261`, with `sf.cpp`'s `msm_w` (`:129-137`) as the write protocol.
//!
//! # Why this is not [`oki::chip`]
//!
//! The MSM6295 fetches its own samples over an address bus and mixes four voices
//! through a volume table. The MSM5205 has no address bus, no voices and no
//! volume: the CPU hands it one nibble per VCK edge. What the two share is
//! [`Adpcm`], whose arithmetic is identical in both chips' reference sources —
//! `compute_tables` (`msm5205.cpp:139-168`) and `okiadpcm.cpp`'s table are the
//! same `floor(16 * pow(11/10, step))` with the same nibble weights, the same
//! `index_shift`, and the same clamps. So `okiadpcm`'s 1,000 verified vector
//! cases cover this chip's arithmetic too, and the tests here cover only the
//! wrapper.
//!
//! # Why this is not in `oki`
//!
//! `oki` is the OKI MSM6295, it has zero runtime dependencies, and it builds for
//! `thumbv7em-none-eabihf`. A second chip's wrapper there would make that target
//! carry code it does not need. The shared part is already `oki`'s and is already
//! no-std, so the only thing crossing the crate edge is one `use`.
//!
//! # Slave clocking, and the delayed capture
//!
//! `set_prescaler_selector(SEX_4B)` (`sf.cpp:789`) is 7, so `m_s1` and `m_s2` are
//! both set and `get_prescaler()` returns 0 (`msm5205.cpp:262-268`);
//! `device_clock_changed` responds by setting the VCK timer to `attotime::never`
//! (`:338-342`). The chip never clocks itself — VCK is an input, and the Z80's
//! port writes clock it.
//!
//! A falling VCK edge does not decode. It arms a capture at
//! `attotime::from_hz(clock() / 6)` (`:236`) — [`CAPTURE_CLOCKS`] master clocks,
//! 15.625 µs at 384 kHz — and [`Msm5205::tick`] is what runs that down. The
//! nibble and the reset pin are both read when it fires, and arming while already
//! armed **restarts** the countdown, because MAME's `adjust` replaces the pending
//! timer rather than queueing behind it. That last one is reachable on the real
//! board: the delay is ~56 Z80 T-states and an `out (n),a` is 11, so a fast
//! writer drops nibbles, and this wrapper drops them in the same places.
//!
//! The countdown is a plain integer rather than a
//! [`RationalAccumulator`](crate::timing::RationalAccumulator) because it divides
//! out exactly — 384,000 / 15,360 = 25 master clocks per scanline, no remainder
//! and nothing fractional to carry.

use oki::adpcm::Adpcm;

/// The master clock SF1 wires (`sf.cpp:788`, `MSM5205(config, m_msm[0], 384000)`).
///
/// The chip is a VCK slave, so this is not a sample rate: it is the unit
/// [`Msm5205::tick`] counts, and 25 of them fit in one of SF1's scanlines exactly.
pub const MASTER_HZ: u32 = 384_000;

/// Master clocks from a falling VCK edge to the capture.
///
/// `attotime::from_hz(clock() / 6)` (`msm5205.cpp:236`), also spelled as
/// `adpcm_capture_divisor() == 6.0` (`msm5205.h:66`) — 15.625 µs at
/// [`MASTER_HZ`], which the datasheet quotes as "15.6 µsec at 384 kHz".
pub const CAPTURE_CLOCKS: u8 = 6;

/// What the 10-bit DAC throws away.
///
/// `msm5205_device` is constructed with `dac_bits = 10` (`msm5205.cpp:66`) and
/// `sound_stream_update` (`:358`) computes `(1 << (12 - dac_bits)) - 1` = 3. The
/// decoder's signal is 12-bit, so its two low bits are below the converter's
/// resolution and never reach the speaker.
pub const DAC_MASK: i16 = 3;

/// The 12-bit signal's scale in the `i16` full-scale domain: 32,768 / 4,096.
///
/// MAME's output is `(signal & ~3) / 4096.0`, a float. This board's mix is
/// integers, as [`crate::cps1::mix`] is, and the two conversions collapse to an
/// exact multiply — no divide, no rounding, no drift.
pub const DAC_TO_I16: i16 = 8;

/// One MSM5205, in VCK-slave 4-bit mode.
///
/// No serde derives, deliberately: `machine` keeps `oki`'s `serde` feature off so
/// that serde stays out of this crate, and SF1's save state is hand-rolled in
/// `frontend::state` exactly as CPS-1's is. The six arguments of
/// [`Msm5205::restore`] are that codec's field list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Msm5205 {
    /// The shared decoder: signal and step index.
    adpcm: Adpcm,
    /// `m_data` — the nibble a capture will decode, already masked to 4 bits.
    data: u8,
    /// `m_vck` — the VCK input's level, so the next write can be compared to it.
    vck: bool,
    /// `m_reset` — the reset **pin**, sampled at each capture.
    ///
    /// Not a device reset: see [`Msm5205::reset_w`].
    reset: bool,
    /// Master clocks until the armed capture fires; 0 when none is armed.
    ///
    /// MAME's `m_capture_timer`. Held as a countdown rather than a deadline so
    /// that a save state carries one small number and no absolute time.
    pending: u8,
}

impl Msm5205 {
    /// A chip at rest — `device_reset`, `msm5205.cpp:117-125`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            adpcm: Adpcm::new(),
            data: 0,
            vck: false,
            reset: false,
            pending: 0,
        }
    }

    /// Return to the state [`Msm5205::new`] produces, disarming any capture.
    ///
    /// ⚠️ Not [`Msm5205::reset_w`], which sets a **pin**. A machine reset that left
    /// a capture armed would decode a stale nibble six clocks into the new run.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Rebuild a chip from a save state, clamping every field a file could corrupt.
    ///
    /// The signal and step clamps are [`Adpcm::restore`]'s: an out-of-range step
    /// index panics in `oki`'s `diff`. `data` is masked the way [`Msm5205::data_w`]
    /// masks it, so a restored nibble cannot decode differently from a written one.
    /// `pending` clamps to [`CAPTURE_CLOCKS`], which is the longest delay the
    /// hardware can be in the middle of.
    #[must_use]
    pub const fn restore(
        signal: i16,
        step: u8,
        data: u8,
        vck: bool,
        reset: bool,
        pending: u8,
    ) -> Self {
        Self {
            adpcm: Adpcm::restore(signal, step),
            data: data & 0x0F,
            vck,
            reset,
            pending: if pending > CAPTURE_CLOCKS {
                CAPTURE_CLOCKS
            } else {
                pending
            },
        }
    }

    /// The reset **pin** (`msm5205.cpp:245-248`).
    ///
    /// ⚠️ Stores the pin and nothing else. The clearing happens at the next capture,
    /// where a set pin produces signal 0 and step 0 *without decoding the nibble*
    /// (`:194-198`). A wrapper that cleared the decoder here would be wrong for a
    /// driver that raises and lowers the pin between captures — and
    /// [`Msm5205::msm_w`] writes the pin on **every** byte, from bit 7, so that is
    /// the normal case rather than an edge case.
    pub fn reset_w(&mut self, state: bool) {
        self.reset = state;
    }

    /// Latch the nibble a capture will decode (`msm5205.cpp:254-260`).
    ///
    /// `m_bitwidth` is 4 on this board (`SEX_4B`), so this keeps `data & 0x0f`. The
    /// 3-bit branch — `(data & 0x07) << 1`, which MAME itself marks
    /// `/* unknown */` — is unreachable from `sf.cpp` and is not implemented:
    /// adding it would be a mode this emulator can never enter.
    pub fn data_w(&mut self, data: u8) {
        self.data = data & 0x0F;
    }

    /// Drive the VCK input. A **falling** edge arms a capture.
    ///
    /// `msm5205.cpp:229-239` arms only when `m_vck && !state`, then stores the new
    /// level unconditionally. So `vclk_w(true)` from rest arms nothing, and two
    /// `vclk_w(false)` calls in a row arm once.
    ///
    /// Arming while already armed restarts the countdown, which is `adjust`'s
    /// behaviour and drops the earlier nibble.
    pub fn vclk_w(&mut self, state: bool) {
        let falling = self.vck && !state;
        self.vck = state;
        if falling {
            self.pending = CAPTURE_CLOCKS;
        }
    }

    /// One port write, as `sf.cpp:129-137` performs it.
    ///
    /// ```text
    /// m_msm[Chip]->reset_w(BIT(data, 7));
    /// /* ?? bit 6?? */
    /// m_msm[Chip]->data_w(data);
    /// m_msm[Chip]->vclk_w(1);
    /// m_msm[Chip]->vclk_w(0);
    /// ```
    ///
    /// Bit 7 is the reset pin and bits 0-3 are the nibble. Bits 4-6 are wired to
    /// nothing — MAME's own `?? bit 6??` is an open question about the hardware, and
    /// the honest model is the one it ships.
    ///
    /// This lives on the chip rather than in the sound board because both chips take
    /// it identically, and a copy per chip is a copy that can drift.
    pub fn msm_w(&mut self, data: u8) {
        self.reset_w(data & 0x80 != 0);
        self.data_w(data);
        self.vclk_w(true);
        self.vclk_w(false);
    }

    /// Advance one master clock, capturing if the countdown reaches zero.
    ///
    /// Called [`MASTER_HZ`] / 15,360 = 25 times per scanline per chip. Idle is the
    /// common case and costs a comparison.
    pub fn tick(&mut self) {
        if self.pending == 0 {
            return;
        }
        self.pending -= 1;
        if self.pending == 0 {
            self.capture();
        }
    }

    /// `update_adpcm`, `msm5205.cpp:184-221`.
    ///
    /// The reset branch does not decode: it produces signal 0 and step 0 directly,
    /// leaving the decoder as if the captures under reset had never happened.
    ///
    /// MAME's "update only when the signal changed" (`:216-220`) is a stream
    /// optimisation, not behaviour — the assignment it guards is the same value —
    /// and has no counterpart here.
    fn capture(&mut self) {
        if self.reset {
            self.adpcm.reset();
        } else {
            self.adpcm.clock(self.data);
        }
    }

    /// The current 12-bit signal, in `-2048..=2047`.
    #[must_use]
    pub const fn signal(&self) -> i16 {
        self.adpcm.signal()
    }

    /// The current step index, in `0..=48`.
    #[must_use]
    pub const fn step(&self) -> u8 {
        self.adpcm.step()
    }

    /// The latched nibble, in `0..=15`.
    #[must_use]
    pub const fn data(&self) -> u8 {
        self.data
    }

    /// The VCK input's current level.
    #[must_use]
    pub const fn vck(&self) -> bool {
        self.vck
    }

    /// Whether the reset pin is currently held.
    #[must_use]
    pub const fn in_reset(&self) -> bool {
        self.reset
    }

    /// Master clocks until the armed capture fires; 0 when none is armed.
    #[must_use]
    pub const fn pending(&self) -> u8 {
        self.pending
    }

    /// The DAC's output in the `i16` full-scale domain, in `-16_384..=16_352`.
    ///
    /// `sound_stream_update`, `msm5205.cpp:350-364`: `(m_signal & ~dac_mask) /
    /// 4096.0`, here scaled by [`DAC_TO_I16`] instead of divided. MAME's
    /// `if (m_signal)` guard around it is arithmetically dead — `0 & !3` is 0 — and
    /// is dropped rather than transcribed; see
    /// `the_reference_implementations_zero_branch_is_redundant`.
    ///
    /// ⚠️ The mask acts on the two's-complement bit pattern, so it does **not**
    /// truncate towards zero: -1 masks to -4, further from zero, while -2048 is
    /// already aligned and is unchanged.
    #[must_use]
    pub const fn output(&self) -> i16 {
        (self.signal() & !DAC_MASK) * DAC_TO_I16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deliver one nibble the way the hardware does: arm, then wait out the delay.
    ///
    /// Every test that just wants a decoded sample goes through this, so the
    /// six-tick wait appears once rather than in a dozen places — and the tests
    /// that are *about* the delay drive `vclk_w` and `tick` themselves.
    fn deliver(chip: &mut Msm5205, nibble: u8) {
        chip.data_w(nibble);
        chip.vclk_w(true);
        chip.vclk_w(false);
        for _ in 0..CAPTURE_CLOCKS {
            chip.tick();
        }
    }

    /// The four published constants, each from its own citation.
    #[test]
    fn the_constants_are_the_chips() {
        // `MSM5205(config, m_msm[0], 384000)` (`sf.cpp:788`).
        assert_eq!(MASTER_HZ, 384_000);
        // `attotime::from_hz(clock() / 6)` (`msm5205.cpp:236`), which
        // `adpcm_capture_divisor()` also gives as 6.0 (`msm5205.h:66`).
        assert_eq!(CAPTURE_CLOCKS, 6);
        // `(1 << (12 - dac_bits)) - 1` with `dac_bits == 10` (`msm5205.cpp:66,358`).
        assert_eq!(DAC_MASK, 3);
        // The 12-bit signal in the i16 domain: 32768 / 4096, exactly.
        assert_eq!(DAC_TO_I16, 8);
        assert_eq!(32_768 / 4_096, i32::from(DAC_TO_I16));
    }

    /// The scheduler's tick rate divides SF1's line rate exactly.
    ///
    /// This is why the countdown is an integer and not a
    /// [`crate::timing::RationalAccumulator`]: 25 master clocks per line, no
    /// remainder, nothing to save. If a later change to [`MASTER_HZ`] or to
    /// [`crate::timing::sf1_line_rate`] broke the divisibility, the countdown would
    /// silently drift — so the divisibility is asserted rather than assumed.
    ///
    /// ⚠️ Task 2's [`crate::timing::sf1_line_rate`], not CPS-1's
    /// `PIXEL_CLOCK / HTOTAL`, which is 15,625 and would make this ratio
    /// fractional. The two boards' line rates differ.
    #[test]
    fn the_master_clock_divides_the_line_rate_exactly() {
        let line_hz = crate::timing::sf1_line_rate();
        assert_eq!(line_hz, 15_360, "SF1's line rate");
        assert_eq!(MASTER_HZ % line_hz, 0, "no fractional tick per line");
        assert_eq!(MASTER_HZ / line_hz, 25);
    }

    /// A fresh chip is silent, idle, and at the state `device_reset` leaves.
    ///
    /// `device_reset` (`msm5205.cpp:117-125`) zeroes `m_data`, `m_vck`, `m_reset`,
    /// `m_signal` and `m_step` — and **not** the selector fields, which sfemu fixes
    /// at construction and so has no field for.
    #[test]
    fn a_fresh_chip_is_silent() {
        let c = Msm5205::new();
        assert_eq!(c.signal(), 0);
        assert_eq!(c.step(), 0);
        assert_eq!(c.data(), 0);
        assert!(!c.vck());
        assert!(!c.in_reset());
        assert_eq!(c.pending(), 0, "no capture armed");
        assert_eq!(c.output(), 0);
        assert_eq!(Msm5205::default(), c, "default is new");
    }

    /// Ticking an idle chip does nothing at all.
    ///
    /// The scheduler ticks both chips 25 times a line whether or not the Z80 has
    /// written a port, so the idle path is the common one. A `tick` that decoded
    /// with `pending == 0` would play the last nibble forever.
    #[test]
    fn ticking_an_idle_chip_is_a_no_op() {
        let mut c = Msm5205::new();
        deliver(&mut c, 0x07);
        let after = c;
        for _ in 0..1_000 {
            c.tick();
        }
        assert_eq!(c, after, "1,000 idle ticks changed nothing");
    }

    /// Only a falling VCK edge arms the capture.
    ///
    /// `msm5205.cpp:235`: `if (m_vck && !state)`. So `vclk_w(true)` from rest arms
    /// nothing, `vclk_w(false)` after it arms, and a second `vclk_w(false)` with no
    /// rise between is not an edge.
    #[test]
    fn only_a_falling_vck_edge_arms_the_capture() {
        let mut c = Msm5205::new();
        c.data_w(0x04);
        c.vclk_w(true);
        assert_eq!(c.pending(), 0, "a rising edge arms nothing");
        assert!(c.vck(), "but it does set the level");
        c.vclk_w(false);
        assert_eq!(c.pending(), CAPTURE_CLOCKS, "the falling edge armed");
        assert!(!c.vck());
        // Run it out, then try the same write again.
        for _ in 0..CAPTURE_CLOCKS {
            c.tick();
        }
        let after = c;
        c.vclk_w(false);
        assert_eq!(c.pending(), 0, "vck was already low: not an edge");
        assert_eq!(c, after);
    }

    /// The capture lands on the sixth tick, not the first and not the seventh.
    ///
    /// The exact tick matters: this is what puts a reset click on the right sample,
    /// and an off-by-one here is inaudible in isolation but shifts every ADPCM
    /// sample against the FM by one 384 kHz clock.
    #[test]
    fn the_capture_lands_on_the_sixth_tick() {
        let mut c = Msm5205::new();
        c.data_w(0x07);
        c.vclk_w(true);
        c.vclk_w(false);
        for tick in 1..CAPTURE_CLOCKS {
            c.tick();
            assert_eq!(c.signal(), 0, "decoded early, on tick {tick}");
            assert_eq!(c.pending(), CAPTURE_CLOCKS - tick);
        }
        c.tick();
        assert_ne!(c.signal(), 0, "the sixth tick decodes");
        assert_eq!(c.pending(), 0, "and disarms");
    }

    /// The decode is [`Adpcm`]'s, nibble for nibble.
    ///
    /// Not a re-derivation of the arithmetic — that is `oki`'s, verified against
    /// MAME's own `okiadpcm.cpp` by 1,000 vector cases. This asserts the wrapper
    /// feeds the shared decoder and nothing else, so a wrapper that mis-shifted or
    /// pre-sign-extended the nibble diverges on the first sample.
    #[test]
    fn the_decode_is_the_shared_decoders() {
        let mut c = Msm5205::new();
        let mut reference = Adpcm::new();
        for nibble in [0x4u8, 0x5, 0xC, 0x0, 0xF, 0x7, 0x8, 0x1] {
            deliver(&mut c, nibble);
            let want = reference.clock(nibble);
            assert_eq!(c.signal(), want, "nibble {nibble:#x}");
            assert_eq!(c.step(), reference.step());
        }
    }

    /// `data_w` keeps the low nibble and drops the rest.
    ///
    /// `m_bitwidth` is 4 on this board, so `m_data = data & 0x0f`
    /// (`msm5205.cpp:254-260`). The 3-bit branch — `(data & 0x07) << 1`, which MAME
    /// itself marks `/* unknown */` — is unreachable from `sf.cpp` and is not
    /// implemented: it would be a mode this emulator can never enter.
    #[test]
    fn data_w_keeps_only_the_low_nibble() {
        let mut c = Msm5205::new();
        c.data_w(0xF4);
        assert_eq!(c.data(), 0x04, "masked on the way in, not at the capture");
        let mut plain = Msm5205::new();
        plain.data_w(0x04);
        assert_eq!(c, plain);
        deliver(&mut c, 0xB4);
        deliver(&mut plain, 0x04);
        assert_eq!(c.signal(), plain.signal());
        assert_ne!(c.signal(), 0, "and 0x4 really did decode");
    }

    /// The nibble is read at the capture, not at the edge.
    ///
    /// MAME's timer callback reads `m_data` when it fires (`msm5205.cpp:203`,
    /// `val = m_data;`), so a `data_w` inside the delay window changes what decodes.
    /// A wrapper that snapshotted the nibble when arming would pass every test above
    /// and fail here.
    #[test]
    fn the_nibble_is_read_at_the_capture_and_not_at_the_edge() {
        let mut c = Msm5205::new();
        c.data_w(0x01);
        c.vclk_w(true);
        c.vclk_w(false);
        c.tick();
        c.tick();
        c.data_w(0x07); // still inside the window
        for _ in 0..(CAPTURE_CLOCKS - 2) {
            c.tick();
        }
        let mut reference = Adpcm::new();
        assert_eq!(c.signal(), reference.clock(0x07), "the late nibble won");
    }

    /// A second edge inside the delay window drops the first nibble.
    ///
    /// `m_capture_timer->adjust` (`msm5205.cpp:236`) replaces the pending timer
    /// rather than queueing behind it, so the first nibble never decodes. This is
    /// reachable on the real board — the delay is ~56 Z80 T-states and an
    /// `out (n),a` is 11 — so a wrapper that queued would produce audibly different
    /// audio, not merely a different internal state.
    #[test]
    fn a_second_edge_inside_the_window_drops_the_first_nibble() {
        let mut c = Msm5205::new();
        c.data_w(0x01);
        c.vclk_w(true);
        c.vclk_w(false);
        c.tick();
        c.tick();
        c.tick();
        // A second write, three clocks in.
        c.data_w(0x07);
        c.vclk_w(true);
        c.vclk_w(false);
        assert_eq!(c.pending(), CAPTURE_CLOCKS, "the countdown restarted");
        // The first window's remaining three clocks pass with nothing decoded.
        for _ in 0..3 {
            c.tick();
        }
        assert_eq!(c.signal(), 0, "the dropped nibble never decoded");
        for _ in 0..3 {
            c.tick();
        }
        let mut reference = Adpcm::new();
        assert_eq!(
            c.signal(),
            reference.clock(0x07),
            "exactly one decode happened"
        );
        assert_eq!(
            c.step(),
            reference.step(),
            "and the step advanced only once"
        );
    }

    /// The reset pin is sampled at the capture, not applied when written.
    ///
    /// `reset_w` (`msm5205.cpp:245-248`) stores the pin and nothing else;
    /// `update_adpcm` (`:194-198`) is where a set pin forces signal 0 and step 0.
    /// This matters here rather than being a nicety: [`Msm5205::msm_w`] writes the
    /// pin from bit 7 on **every** byte, so raising and lowering it between captures
    /// is the normal case, and a wrapper that cleared the decoder inside `reset_w`
    /// would silence the chip between samples.
    #[test]
    fn the_reset_pin_is_sampled_at_the_capture() {
        let mut c = Msm5205::new();
        for _ in 0..4 {
            deliver(&mut c, 0x07);
        }
        let loud = c.signal();
        assert_ne!(loud, 0);
        // Raising and lowering the pin with no capture in between changes nothing.
        c.reset_w(true);
        assert_eq!(c.signal(), loud, "reset_w alone does not clear the signal");
        c.reset_w(false);
        assert_eq!(c.signal(), loud);
        // Held across a capture, it clears both signal and step.
        c.reset_w(true);
        deliver(&mut c, 0x07);
        assert_eq!(c.signal(), 0, "the capture saw the pin set");
        assert_eq!(c.step(), 0);
        assert!(c.in_reset(), "and the pin is still held");
    }

    /// The pin is read at the capture too, so it can be raised inside the window.
    #[test]
    fn the_reset_pin_is_read_at_the_capture_and_not_at_the_edge() {
        let mut c = Msm5205::new();
        for _ in 0..4 {
            deliver(&mut c, 0x07);
        }
        assert_ne!(c.signal(), 0);
        c.data_w(0x07);
        c.vclk_w(true);
        c.vclk_w(false);
        c.tick();
        c.reset_w(true); // inside the window
        for _ in 0..(CAPTURE_CLOCKS - 1) {
            c.tick();
        }
        assert_eq!(c.signal(), 0, "the capture saw the late pin");
    }

    /// A capture under reset does not decode the nibble at all.
    ///
    /// `update_adpcm`'s reset branch skips the `m_diff_lookup` add *and* the
    /// `index_shift`: the step is never consulted, not advanced and then cleared.
    /// Both leave the same state, so this is asserted through the *next* capture —
    /// a decoder that had advanced would produce a different signal on the sample
    /// after the reset ends.
    #[test]
    fn a_capture_under_reset_does_not_advance_the_decoder() {
        let mut held = Msm5205::new();
        held.reset_w(true);
        for _ in 0..3 {
            deliver(&mut held, 0x07);
        }
        held.reset_w(false);
        deliver(&mut held, 0x04);

        let mut fresh = Msm5205::new();
        deliver(&mut fresh, 0x04);

        assert_eq!(
            held.signal(),
            fresh.signal(),
            "the held captures left no trace"
        );
        assert_eq!(held.step(), fresh.step());
    }

    /// `msm_w` is one port write, one armed nibble, and the pin from bit 7.
    ///
    /// `sf.cpp:129-137` in one call: `reset_w(BIT(data,7))`, `data_w(data)`,
    /// `vclk_w(1)`, `vclk_w(0)`.
    #[test]
    fn msm_w_is_one_port_write_and_one_armed_nibble() {
        let mut byte = Msm5205::new();
        byte.msm_w(0x07);
        assert_eq!(byte.data(), 0x07);
        assert!(!byte.in_reset(), "bit 7 clear");
        assert!(!byte.vck(), "left low, after the high-then-low toggle");
        assert_eq!(byte.pending(), CAPTURE_CLOCKS, "exactly one capture armed");

        let mut spelled = Msm5205::new();
        spelled.reset_w(false);
        spelled.data_w(0x07);
        spelled.vclk_w(true);
        spelled.vclk_w(false);
        assert_eq!(byte, spelled, "msm_w is those four calls and nothing more");
    }

    /// `msm_w` takes the reset pin from bit 7 and ignores bits 4-6.
    ///
    /// Bit 6 is MAME's own open question — `/* ?? bit 6?? */` — and the honest
    /// model is the one MAME ships: bit 6 does nothing. Bits 4 and 5 likewise
    /// reach neither the pin (bit 7 only) nor the nibble (`& 0x0f`).
    #[test]
    fn msm_w_takes_the_reset_pin_from_bit_seven_only() {
        let mut c = Msm5205::new();
        c.msm_w(0x87);
        assert!(c.in_reset(), "bit 7 set");
        assert_eq!(c.data(), 0x07, "and the nibble is still the low four bits");
        // Bits 4-6 change nothing at all, including bit 6.
        let mut plain = Msm5205::new();
        plain.msm_w(0x07);
        for bit in [0x10u8, 0x20, 0x40] {
            let mut c = Msm5205::new();
            c.msm_w(0x07 | bit);
            assert_eq!(c, plain, "bit {bit:#x} is not wired");
        }
    }

    /// The output masks the signal's two low bits off and scales to `i16`.
    ///
    /// `sound_stream_update` (`msm5205.cpp:350-364`): `(m_signal & ~3) / 4096.0`.
    /// Here the same value lands in the `i16` full-scale domain, so the divide by
    /// 4,096 and the multiply by 32,768 collapse to `* 8`. The expectations are
    /// written as literals computed from that formula rather than from the code, so
    /// a wrapper that shifted by 4 or masked with 1 fails instead of agreeing with
    /// its own derivation.
    #[test]
    fn the_output_is_the_signal_masked_to_ten_bits_scaled_to_i16() {
        let out = |signal| Msm5205::restore(signal, 0, 0, false, false, 0).output();
        assert_eq!(out(0), 0);
        assert_eq!(out(4), 32, "4 & !3 == 4, times 8");
        // 1, 2 and 3 all mask to 0: the two low bits are below the DAC.
        for signal in [1i16, 2, 3] {
            assert_eq!(out(signal), 0, "signal {signal} is below the DAC");
        }
        assert_eq!(out(7), 32, "7 & !3 == 4");
        assert_eq!(out(2047), 16_352, "2047 & !3 == 2044, times 8");
        // ⚠️ The mask acts on the two's-complement bit pattern, so it does **not**
        // truncate towards zero. -2048 is 0xF800, already a multiple of 4, and is
        // unchanged; -1 is 0xFFFF and masks to -4, which is *further* from zero.
        // That is MAME's arithmetic, and a wrapper that used `signal / 4 * 4` or
        // `abs`-then-mask would differ on every odd negative sample.
        assert_eq!(out(-2048), -16_384);
        assert_eq!(out(-1), -32);
        assert_eq!(out(-5), -64, "-5 & !3 == -8");
    }

    /// MAME's explicit zero branch is arithmetically dead.
    ///
    /// `if (m_signal) … else output.fill(0)`. `0 & !3` is 0, so the branch changes
    /// nothing — and this test is what lets the implementation drop it without a
    /// reader having to re-derive that.
    ///
    /// The zero comes from a chip's own `signal()` rather than a `0i16` literal:
    /// `clippy::erasing_op` rejects `0 & x` as always-zero, which is precisely the
    /// fact under test, so the value has to reach the expression at run time.
    #[test]
    fn the_reference_implementations_zero_branch_is_redundant() {
        let zero = Msm5205::new().signal();
        assert_eq!(zero, 0, "a fresh chip's signal");
        assert_eq!(zero & !DAC_MASK, 0, "the mask does not lift it off zero");
        assert_eq!(
            (zero & !DAC_MASK) * DAC_TO_I16,
            0,
            "and neither does the scale"
        );
    }

    /// The output never leaves half of `i16`'s range.
    ///
    /// The signal clamps to `-2048..=2047` and the scale is 8, so the range is
    /// `-16_384..=16_352`. Task 11's mix sums this with the YM2151's full-scale
    /// output, and these two bounds are what make its saturation argument checkable
    /// — so both rails are driven here rather than assumed from the decoder's.
    #[test]
    fn the_output_stays_inside_half_of_full_scale() {
        let mut c = Msm5205::new();
        // 0x7 is the largest positive nibble and 0xF the largest negative; 64
        // captures at a climbing step index is far more than enough to saturate.
        for _ in 0..64 {
            deliver(&mut c, 0x07);
        }
        assert_eq!(c.signal(), 2047, "the positive rail");
        assert_eq!(c.output(), 16_352);
        for _ in 0..64 {
            deliver(&mut c, 0x0F);
        }
        assert_eq!(c.signal(), -2048, "the negative rail");
        assert_eq!(c.output(), -16_384);
        assert_eq!(
            i32::from(c.output()),
            -(1 << 14),
            "exactly -0.5 of full scale"
        );
    }

    /// `reset` returns the chip to `new`, unlike the reset **pin**.
    ///
    /// It must also disarm a pending capture: a machine reset that left one armed
    /// would decode a stale nibble six clocks into the new run.
    #[test]
    fn reset_is_a_device_reset_and_not_the_pin() {
        let mut c = Msm5205::new();
        c.msm_w(0x8F);
        assert_eq!(c.pending(), CAPTURE_CLOCKS);
        c.reset();
        assert_eq!(c, Msm5205::new(), "every field, including vck and the pin");
        assert_eq!(c.pending(), 0, "and no capture survives");
        assert!(!c.in_reset());
    }

    /// `restore` round-trips and clamps everything a corrupt save state could hold.
    ///
    /// The signal and step clamps are [`Adpcm::restore`]'s, and they are not
    /// defensive habit: an out-of-range step index panics in `oki`'s `diff`. The
    /// pending count needs the same treatment for its own reason — a count above
    /// [`CAPTURE_CLOCKS`] would delay the capture past what the hardware can, and
    /// the value comes from a file.
    #[test]
    fn restore_round_trips_and_clamps() {
        let c = Msm5205::restore(-1000, 20, 0x0A, true, true, 3);
        assert_eq!(c.signal(), -1000);
        assert_eq!(c.step(), 20);
        assert_eq!(c.data(), 0x0A);
        assert!(c.vck());
        assert!(c.in_reset());
        assert_eq!(c.pending(), 3);

        // Out of range, from a corrupt or hand-edited file.
        let mut c = Msm5205::restore(30_000, 200, 0xFF, false, false, 99);
        assert_eq!(c.signal(), 2047);
        assert_eq!(c.step(), 48);
        assert_eq!(c.data(), 0x0F, "masked exactly as data_w masks");
        assert_eq!(c.pending(), CAPTURE_CLOCKS);
        // And the restored chip decodes rather than panicking — a step index of 48
        // is the last row of `oki`'s table, and one past it is the panic.
        //
        // ⚠️ The clamped nibble is 0x0F, whose bit 3 is the **sign**, so this is the
        // largest *negative* increment rather than a hold at the rail: step 48's
        // value is 1552, the 1 + 1/2 + 1/4 + 1/8 weights sum to 2910, and
        // 2047 - 2910 = -863, which is inside the clamp and so lands exactly.
        for _ in 0..CAPTURE_CLOCKS {
            c.tick();
        }
        assert_eq!(
            c.signal(),
            -863,
            "the full negative swing from the top rail"
        );
        assert_eq!(c.step(), 48, "and the step index stays clamped");
    }

    /// A restored pending count still fires, with the restored nibble.
    ///
    /// `m_data` is saved state (`msm5205.cpp:103`) and so is the pending capture, so
    /// a save taken between a port write and its capture must reproduce both. A
    /// restore that dropped either would substitute a different sample — silently,
    /// because the state is otherwise identical.
    #[test]
    fn a_restored_pending_capture_fires_with_the_restored_nibble() {
        let mut c = Msm5205::restore(0, 0, 0x07, false, false, 2);
        c.tick();
        assert_eq!(c.signal(), 0, "one clock left");
        c.tick();
        let mut reference = Adpcm::new();
        assert_eq!(c.signal(), reference.clock(0x07));
        assert_eq!(c.step(), reference.step());
    }

    /// A restored VCK level decides whether the next write is an edge.
    #[test]
    fn a_restored_vck_level_decides_the_next_edge() {
        // Saved with VCK already high: the next `vclk_w(false)` is a falling edge.
        let mut high = Msm5205::restore(0, 0, 0x07, true, false, 0);
        high.vclk_w(false);
        assert_eq!(high.pending(), CAPTURE_CLOCKS, "the edge landed");
        // Saved with VCK low: the same write is not an edge.
        let mut low = Msm5205::restore(0, 0, 0x07, false, false, 0);
        low.vclk_w(false);
        assert_eq!(low.pending(), 0, "no edge, no capture");
    }
}
