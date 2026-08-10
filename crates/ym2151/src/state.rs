//! The chip's save-state byte layout.
//!
//! # Why the chip writes its own bytes
//!
//! Most of what a restored YM2151 needs is private: [`crate::regs::Regs`]'s register
//! file and the two depth registers behind `0x19`, [`crate::timer::Timers`]' counters
//! and IRQ state, and the chip's own envelope counter and prepare gate. A frontend
//! cannot reach any of them, and the alternative — a public setter per field — is a
//! far wider hole in the chip's interface than one byte layout, and one every later
//! caller could reach through.
//!
//! So each type encodes itself, next to the fields it encodes, and this module holds
//! the cursor both directions share plus the sizes. [`STATE_BYTES`] is written out
//! term by term and pinned against a literal by
//! `tests::the_state_is_the_documented_size`, so the layout is a layout rather than
//! whatever the writer happens to emit.
//!
//! # No allocation
//!
//! The writer fills a caller-supplied `&mut [u8]` instead of growing a `Vec`: this
//! crate is `no_std` without the `std` feature and has no `alloc` dependency, and
//! [`STATE_BYTES`] is a constant, so a fixed buffer costs nothing.
//!
//! # What is deliberately absent
//!
//! The two `#[cfg(any(test, feature = "internals"))]` diagnostic fields —
//! `force_eager_prepare` and `prepare_count`. They do not exist in a normal build, so
//! a format carrying them would change size with a feature flag. A chip decoded
//! inside this crate's own tests therefore has them at their defaults, which is why
//! `chip::tests::a_decoded_chip_produces_the_same_samples` compares produced samples
//! rather than whole chips.
//!
//! # This is not the frontend's format
//!
//! `frontend::state` writes the machine's save-state file and embeds [`STATE_BYTES`]
//! of this layout inside its payload. The two are separate lists on purpose, for the
//! reason that module's own docs give: a layout *implied* by struct definitions
//! changes silently on a field reorder while every round-trip test still passes.

use crate::regs::CHANNEL_COUNT;

/// [`crate::regs::Regs`]: the 256-byte file, then the two depths behind `0x19`.
pub const REGS_BYTES: usize = 0x100 + 1 + 1;

/// [`crate::operator::Operator`]: phase, attenuation, state, key, live key-ons.
pub const OPERATOR_BYTES: usize = 4 + 2 + 1 + 1 + 1;

/// [`crate::operator::OpCache`]: step, level, sustain, four rates, detune, multiple,
/// block/frequency.
pub const OP_CACHE_BYTES: usize = 4 + 2 + 2 + 4 + 4 + 4 + 4;

/// [`crate::channel::Channel`]: four operators, four caches, and the feedback trio.
pub const CHANNEL_BYTES: usize = 4 * OPERATOR_BYTES + 4 * OP_CACHE_BYTES + 2 + 2 + 2;

/// [`crate::lfo::Lfo`]: the counter, waveform 3's 256 entries, and the held AM.
///
/// The 512 bytes of waveform are most of the layout, and they are state: the LFO
/// writes one entry per clock from the noise generator, so a chip restored without
/// them plays waveform 3 out of a table of zeros until it has walked all 256
/// positions.
pub const LFO_BYTES: usize = 4 + 256 * 2 + 4;

/// [`crate::noise::Noise`]: the shift register, the counter, the latched bit.
pub const NOISE_BYTES: usize = 4 + 4 + 4;

/// [`crate::timer::Timers`]: two counters, two run flags, status, IRQ, total clocks.
pub const TIMERS_BYTES: usize = 4 + 4 + 1 + 1 + 1 + 1 + 4;

/// The chip's own fields: the envelope counter, the modified mask, the prepare
/// counter.
pub const CHIP_BYTES: usize = 4 + 1 + 4;

/// One chip's state, exactly.
///
/// ```text
/// regs      0x100 + 1 + 1            =   258
/// channels  8 * (4*9 + 4*24 + 6)     =  1104
/// lfo       4 + 256*2 + 4            =   520
/// noise     4 + 4 + 4                =    12
/// timers    4 + 4 + 1 + 1 + 1 + 1 + 4 =   16
/// chip      4 + 1 + 4                =     9
///                                      -----
///                                       1919
/// ```
pub const STATE_BYTES: usize = REGS_BYTES
    + CHANNEL_COUNT as usize * CHANNEL_BYTES
    + LFO_BYTES
    + NOISE_BYTES
    + TIMERS_BYTES
    + CHIP_BYTES;

/// Writes little-endian scalars into a caller's buffer.
///
/// A cursor rather than free functions so each type's `write_state` reads as one
/// sequence, which is what makes a reordering visible in a diff.
///
/// # Never panics
///
/// A put past the end of the buffer is dropped, and [`StateWriter::at`] keeps
/// counting either way. A buffer that is too short is a *layout* bug rather than
/// guest input — [`crate::Ym2151::write_state_bytes`] sizes it from
/// [`STATE_BYTES`] — and `tests::the_layout_fills_every_byte` is what stops such a
/// bug hiding: it requires the cursor to end exactly at the end.
pub struct StateWriter<'a> {
    out: &'a mut [u8],
    at: usize,
}

impl<'a> StateWriter<'a> {
    /// A writer positioned at the start of `out`.
    #[must_use]
    pub fn new(out: &'a mut [u8]) -> Self {
        Self { out, at: 0 }
    }

    /// How many bytes have been written, counting any dropped past the end.
    #[must_use]
    pub const fn at(&self) -> usize {
        self.at
    }

    /// Copies `src` in, dropping whatever falls past the end.
    fn put(&mut self, src: &[u8]) {
        if let Some(slot) = self.out.get_mut(self.at..self.at + src.len()) {
            slot.copy_from_slice(src);
        }
        self.at += src.len();
    }

    /// One byte.
    pub fn u8(&mut self, v: u8) {
        self.put(&[v]);
    }
    /// A signed byte, two's complement.
    pub fn i8(&mut self, v: i8) {
        self.put(&v.to_le_bytes());
    }
    /// One byte: 0 or 1.
    pub fn bool(&mut self, v: bool) {
        self.put(&[u8::from(v)]);
    }
    /// Two bytes, little-endian.
    pub fn u16(&mut self, v: u16) {
        self.put(&v.to_le_bytes());
    }
    /// Two bytes, little-endian, two's complement.
    pub fn i16(&mut self, v: i16) {
        self.put(&v.to_le_bytes());
    }
    /// Four bytes, little-endian.
    pub fn u32(&mut self, v: u32) {
        self.put(&v.to_le_bytes());
    }
    /// Four bytes, little-endian, two's complement.
    pub fn i32(&mut self, v: i32) {
        self.put(&v.to_le_bytes());
    }
}

/// Reads little-endian scalars in the writer's order.
///
/// # Never panics, on any input
///
/// [`crate::Ym2151::read_state`] checks the slice is exactly [`STATE_BYTES`] long
/// before any getter runs, so a read past the end would be a layout bug rather than
/// bad input. Such a read yields zeros instead of panicking, so the bug cannot become
/// a crash in a frontend loading a user's file, and
/// `tests::the_layout_fills_every_byte` is what stops it hiding: it requires the
/// cursor to end exactly at the end.
pub struct StateReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> StateReader<'a> {
    /// A reader positioned at the start of `bytes`.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// How many bytes have been consumed, counting any read past the end.
    #[must_use]
    pub const fn at(&self) -> usize {
        self.at
    }

    /// The next `N` bytes, or zeros past the end. `N` is never more than 4.
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let from = self.at;
        self.at += N;
        self.bytes
            .get(from..from + N)
            .and_then(|s| s.try_into().ok())
            .unwrap_or([0; N])
    }

    /// One byte.
    pub fn u8(&mut self) -> u8 {
        self.take::<1>()[0]
    }
    /// A signed byte, two's complement.
    pub fn i8(&mut self) -> i8 {
        i8::from_le_bytes(self.take())
    }
    /// One byte; any non-zero is true, because a file's padding byte of 2 is not
    /// worth a rejection.
    pub fn bool(&mut self) -> bool {
        self.u8() != 0
    }
    /// Two bytes, little-endian.
    pub fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take())
    }
    /// Two bytes, little-endian, two's complement.
    pub fn i16(&mut self) -> i16 {
        i16::from_le_bytes(self.take())
    }
    /// Four bytes, little-endian.
    pub fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }
    /// Four bytes, little-endian, two's complement.
    pub fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ym2151;

    /// Every size against a hand-written literal.
    ///
    /// ⚠️ Each right-hand side is written out from the field list rather than
    /// recomputed from the left. `assert_eq!(a + b, a + b)` passes for every layout,
    /// including one that forgot a field.
    #[test]
    fn the_state_is_the_documented_size() {
        assert_eq!(REGS_BYTES, 258, "256 registers, two depths");
        assert_eq!(OPERATOR_BYTES, 9);
        assert_eq!(OP_CACHE_BYTES, 24);
        assert_eq!(CHANNEL_BYTES, 138, "4*9 + 4*24 + 6");
        assert_eq!(LFO_BYTES, 520, "and 512 of it is waveform 3");
        assert_eq!(NOISE_BYTES, 12);
        assert_eq!(TIMERS_BYTES, 16);
        assert_eq!(CHIP_BYTES, 9);
        assert_eq!(STATE_BYTES, 1_919, "258 + 1104 + 520 + 12 + 16 + 9");
        assert_eq!(CHANNEL_COUNT, 8, "the multiplier above");
    }

    /// The writer fills exactly [`STATE_BYTES`] and the reader consumes exactly that.
    ///
    /// Both halves. A writer and a reader that agree with each other but not with the
    /// constant would leave the frontend's payload the wrong length, and a reader
    /// stopping short of the end would read the *next* field's bytes as its own.
    #[test]
    fn the_layout_fills_every_byte() {
        let chip = Ym2151::new();
        let mut buf = [0u8; STATE_BYTES];
        let mut w = StateWriter::new(&mut buf);
        chip.write_state(&mut w);
        assert_eq!(w.at(), STATE_BYTES, "the writer's total");
        let mut r = StateReader::new(&buf);
        let _ = Ym2151::read_state_from(&mut r);
        assert_eq!(r.at(), STATE_BYTES, "and the reader's");
    }

    /// A slice of the wrong length is refused rather than decoded from zeros.
    #[test]
    fn a_wrong_sized_slice_is_refused() {
        let bytes = Ym2151::new().write_state_bytes();
        assert!(Ym2151::read_state(&bytes).is_some(), "the premise");
        assert!(Ym2151::read_state(&bytes[..STATE_BYTES - 1]).is_none());
        assert!(Ym2151::read_state(&[]).is_none());
        let mut long = [0u8; STATE_BYTES + 1];
        long[..STATE_BYTES].copy_from_slice(&bytes);
        assert!(Ym2151::read_state(&long).is_none(), "and one byte too many");
    }

    /// A short buffer drops the overflow instead of panicking, and says so.
    ///
    /// The frontend sizes its buffer from [`STATE_BYTES`], so this cannot happen from
    /// outside — but the writer is public, and a panic in a save path is worse than a
    /// wrong length a caller can check.
    #[test]
    fn a_short_buffer_is_truncated_rather_than_a_panic() {
        let chip = Ym2151::new();
        let mut buf = [0u8; 10];
        let mut w = StateWriter::new(&mut buf);
        chip.write_state(&mut w);
        assert_eq!(
            w.at(),
            STATE_BYTES,
            "the cursor still reports the full size"
        );
        // The first ten bytes are register file bytes 0-9, all zero after reset.
        assert_eq!(buf, [0u8; 10]);
    }
}
