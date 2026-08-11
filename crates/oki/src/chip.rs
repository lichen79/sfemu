//! The MSM6295's four voices, command protocol and volume table.
//!
//! Transcribed from MAME's `okim6295.cpp` (BSD-3, Mirko Buffoni and Aaron
//! Giles). The chip has no clock of its own here: [`Oki::step_2x`] produces one
//! output sample, and deciding when to call it is `machine`'s job.
//!
//! Output is in a **2x domain**: [`Oki::step_2x`] returns `sum(signal * T)`,
//! where `T` is the raw volume-table entry out of `0x20`. MAME's float
//! pipeline computes `signal * T / 32 / 2048`, and 4096 of the 65536 possible
//! products are half-integers -- so `signal * T` is the widest value that is
//! exactly representable as an integer, and dividing by two here would lose a
//! bit that the mix can otherwise keep.

use crate::adpcm::Adpcm;

/// The chip has four independent voices.
pub const VOICES: usize = 4;

/// The status byte with no voice playing (`okim6295.cpp` builds `0xF0` and
/// sets one bit per playing voice).
pub const STATUS_IDLE: u8 = 0xF0;

/// MAME's `s_volume_table`, as raw numerators over `0x20`.
///
/// Indices 9 through 15 are exactly zero: a voice set to one of them runs
/// silently. Stored as the numerator rather than a float so the voice product
/// stays exact; see the module docs.
pub const VOLUME_TABLE: [u8; 16] = [
    0x20, 0x16, 0x10, 0x0B, 0x08, 0x06, 0x04, 0x03, 0x02, 0, 0, 0, 0, 0, 0, 0,
];

/// The chip clamps its own summed stream to `+-1.0` before the speaker mix
/// (`okim6295.cpp:188`). In the 2x domain that is `2 * 32768`.
///
/// Reachable in normal play: one voice at volume index 0 and full signal is
/// 65504, but two are 131008, and 4826 of the 6561 four-voice volume
/// combinations exceed the clamp.
pub const CLAMP_2X: i32 = 65_536;

/// The sample ROM address bus is 18 bits (`device_rom_interface<18>`).
const ADDRESS_MASK: u32 = 0x3_FFFF;

/// One voice: a decoder, a position, and a volume.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Voice {
    adpcm: Adpcm,
    playing: bool,
    base: u32,
    sample: u32,
    count: u32,
    volume: u8,
}

impl Voice {
    /// Whether this voice is sounding.
    #[must_use]
    pub const fn playing(&self) -> bool {
        self.playing
    }

    /// The decoder's state, for a save file.
    #[must_use]
    pub const fn adpcm(&self) -> Adpcm {
        self.adpcm
    }

    /// Where in the ROM this phrase began.
    #[must_use]
    pub const fn base(&self) -> u32 {
        self.base
    }

    /// How many nibbles have been consumed.
    #[must_use]
    pub const fn sample(&self) -> u32 {
        self.sample
    }

    /// How many nibbles the phrase holds.
    #[must_use]
    pub const fn count(&self) -> u32 {
        self.count
    }

    /// This voice's volume-table numerator.
    #[must_use]
    pub const fn volume(&self) -> u8 {
        self.volume
    }

    /// Rebuild a voice from a save file. The decoder's own `restore` clamps
    /// the signal and step; a position past the end simply stops the voice.
    #[must_use]
    pub const fn restore(
        adpcm: Adpcm,
        playing: bool,
        base: u32,
        sample: u32,
        count: u32,
        volume: u8,
    ) -> Self {
        Self {
            adpcm,
            playing: playing && sample < count,
            base: base & ADDRESS_MASK,
            sample,
            count,
            volume,
        }
    }
}

/// The OKI MSM6295.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Oki {
    voices: [Voice; VOICES],
    /// The latched phrase number, or `None` when no command is half-delivered.
    command: Option<u8>,
}

impl Oki {
    /// A chip at rest: nothing playing, no command pending.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            voices: [Voice {
                adpcm: Adpcm::new(),
                playing: false,
                base: 0,
                sample: 0,
                count: 0,
                volume: 0,
            }; VOICES],
            command: None,
        }
    }

    /// Stop every voice and drop any pending command.
    ///
    /// Note what this does *not* do: MAME's `device_reset` leaves the pin-7
    /// state alone, so the sample rate survives a reset. That is `machine`'s
    /// concern, not this type's.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The status byte the Z80 reads: `0xF0` plus one bit per playing voice.
    #[must_use]
    pub fn status(&self) -> u8 {
        let mut s = STATUS_IDLE;
        for (i, v) in self.voices.iter().enumerate() {
            if v.playing {
                s |= 1 << i;
            }
        }
        s
    }

    /// A bitmask of the playing voices, for the debugger.
    #[must_use]
    pub fn voices_playing(&self) -> u8 {
        self.status() & 0x0F
    }

    /// The voices, for a save file and the debugger.
    #[must_use]
    pub const fn voices(&self) -> &[Voice; VOICES] {
        &self.voices
    }

    /// The half-delivered command, for a save file.
    #[must_use]
    pub const fn pending_command(&self) -> Option<u8> {
        self.command
    }

    /// Rebuild from a save file.
    ///
    /// An associated constructor rather than a method on `&mut self`: the
    /// save-state codec has no chip in hand to call it on, and a two-step
    /// `new` then `restore` is a shape in which forgetting the second step
    /// compiles.
    #[must_use]
    pub fn restore(voices: [Voice; VOICES], command: Option<u8>) -> Self {
        Self {
            voices,
            command: command.map(|c| c & 0x7F),
        }
    }

    /// Write one command byte.
    ///
    /// Three cases, in the order `okim6295.cpp:write` tests them: a pending
    /// command makes this byte `vvvv gggg`; otherwise bit 7 latches a phrase
    /// number; otherwise it is a stop whose mask is shifted by three.
    pub fn write(&mut self, command: u8, rom: &[u8]) {
        if let Some(phrase) = self.command.take() {
            let mask = command >> 4;
            let volume = VOLUME_TABLE[usize::from(command & 0x0F)];
            for (i, voice) in self.voices.iter_mut().enumerate() {
                if mask & (1 << i) == 0 {
                    continue;
                }
                // MAME skips a voice that is already playing; its comment
                // credits this to "fixes Got-cha and Steel Force".
                if voice.playing {
                    continue;
                }
                let base = u32::from(phrase) * 8;
                let start = read24(rom, base) & ADDRESS_MASK;
                let stop = read24(rom, base + 3) & ADDRESS_MASK;
                // MAME logs and refuses a phrase whose start is not below its
                // stop; `count` would otherwise be nonsense.
                if start < stop {
                    *voice = Voice {
                        adpcm: Adpcm::new(),
                        playing: true,
                        base: start,
                        sample: 0,
                        count: 2 * (stop - start + 1),
                        volume,
                    };
                }
            }
        } else if command & 0x80 != 0 {
            self.command = Some(command & 0x7F);
        } else {
            let mask = command >> 3;
            for (i, voice) in self.voices.iter_mut().enumerate() {
                if mask & (1 << i) != 0 {
                    voice.playing = false;
                }
            }
        }
    }

    /// Produce one output sample: the four voices' products summed in the 2x
    /// domain and clamped the way the chip clamps its own stream.
    ///
    /// Returns an `i16` because the clamp is `+-65536`, which saturates to
    /// `i16::MAX` -- the mix in `machine` calls [`Oki::step_2x`] instead, to
    /// keep the bit the saturation would lose.
    pub fn step(&mut self, rom: &[u8]) -> i16 {
        self.step_2x(rom)
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
    }

    /// Produce one output sample in the 2x domain, clamped to [`CLAMP_2X`].
    ///
    /// This is the value the mono mix consumes: `sum(signal * T)`, which is
    /// exactly twice MAME's float stream value.
    pub fn step_2x(&mut self, rom: &[u8]) -> i32 {
        self.step_all(rom).0
    }

    /// [`Oki::step_2x`], also reporting whether the clamp actually bit.
    ///
    /// The board counts clipping from this rather than from `sample.abs() ==
    /// CLAMP_2X`: a sum that lands on exactly `+-CLAMP_2X` was not clipped, and
    /// the output alone cannot distinguish the two.
    pub fn step_2x_clamped(&mut self, rom: &[u8]) -> (i32, bool) {
        let (sample, _, clamped) = self.step_all(rom);
        (sample, clamped)
    }

    /// [`Oki::step_2x`], also returning the nibble each voice consumed.
    ///
    /// Voice `v`'s nibble occupies bits `4v..4v+3`; a voice that is not playing
    /// contributes zero. The vector runner checks this **before** the sample,
    /// because a wrong nibble is an address-walk bug and a wrong sample from a
    /// right nibble is a decoder bug -- different files.
    pub fn step_2x_traced(&mut self, rom: &[u8]) -> (i32, u16) {
        let (sample, nibbles, _) = self.step_all(rom);
        (sample, nibbles)
    }

    /// The one implementation: the sample, the nibbles, and whether the clamp
    /// bit.
    ///
    /// The three public forms above are projections of this. One code path
    /// rather than three, so a traced or counted variant cannot drift from the
    /// plain one -- which is a real failure mode, because only the plain one is
    /// checked against the vector suite.
    fn step_all(&mut self, rom: &[u8]) -> (i32, u16, bool) {
        let mut sum = 0i32;
        let mut nibbles = 0u16;
        for (i, voice) in self.voices.iter_mut().enumerate() {
            if !voice.playing {
                continue;
            }
            let addr = voice.base.wrapping_add(voice.sample / 2) & ADDRESS_MASK;
            let byte = rom.get(addr as usize).copied().unwrap_or(0);
            // High nibble first: sample 0 takes bits 7..4.
            let shift = ((voice.sample & 1) << 2) ^ 4;
            let nibble = (byte >> shift) & 0x0F;
            nibbles |= u16::from(nibble) << (4 * i);
            let signal = voice.adpcm.clock(nibble);
            sum += i32::from(signal) * i32::from(voice.volume);
            voice.sample += 1;
            if voice.sample >= voice.count {
                voice.playing = false;
            }
        }
        let clamped = sum.clamp(-CLAMP_2X, CLAMP_2X);
        (clamped, nibbles, clamped != sum)
    }
}

/// A 3-byte big-endian pointer out of the phrase table, zero past the end.
fn read24(rom: &[u8], at: u32) -> u32 {
    let byte = |i: u32| u32::from(rom.get((at + i) as usize).copied().unwrap_or(0));
    byte(0) << 16 | byte(1) << 8 | byte(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a phrase-table entry: a 3-byte big-endian start and stop at
    /// `phrase * 8`.
    ///
    /// A helper rather than the plan's per-test closure because the plan spelt
    /// the address as `1 * 8`, which clippy's `identity_op` rejects under
    /// `-D warnings`. The bytes are identical either way.
    fn put_phrase(rom: &mut [u8], phrase: usize, start: u32, stop: u32) {
        let a = phrase * 8;
        rom[a] = (start >> 16) as u8;
        rom[a + 1] = (start >> 8) as u8;
        rom[a + 2] = start as u8;
        rom[a + 3] = (stop >> 16) as u8;
        rom[a + 4] = (stop >> 8) as u8;
        rom[a + 5] = stop as u8;
    }

    /// A ROM with three phrase-table entries and pseudorandom sample data,
    /// matching the fixture the reference probe used.
    fn rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x4_0000];
        put_phrase(&mut rom, 1, 0x1000, 0x107F); // 0x80 bytes = 256 nibbles
        put_phrase(&mut rom, 2, 0x2000, 0x203F);
        put_phrase(&mut rom, 3, 0x3000, 0x2FFF); // start > stop: invalid
                                                 // start == stop, the *boundary* of the refusal and a separate case from the
                                                 // one above. `if start <= stop` — accepting a degenerate phrase — is refused
                                                 // by phrase 3 either way, so without this entry the comparison's boundary is
                                                 // untested and the mutation survives; see the test below.
        put_phrase(&mut rom, 4, 0x1800, 0x1800);
        // Iterating the slice rather than the index range is clippy's
        // requirement under `-D warnings`, and the bytes are identical: the
        // xorshift advances once per address in the same order.
        let mut s: u64 = 0xdead_beef;
        for byte in &mut rom[0x1000..0x4000] {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            *byte = s as u8;
        }
        rom
    }

    /// Measured: idle F0, one voice F1, four voices FF.
    #[test]
    fn the_status_byte_reports_the_playing_voices_in_its_low_nibble() {
        let rom = rom();
        let mut o = Oki::new();
        assert_eq!(o.status(), 0xF0, "idle");
        o.write(0x81, &rom);
        o.write(0x10, &rom);
        assert_eq!(o.status(), 0xF1, "voice 0");
        o.write(0x81, &rom);
        o.write(0x20, &rom);
        o.write(0x82, &rom);
        o.write(0x40, &rom);
        o.write(0x82, &rom);
        o.write(0x80, &rom);
        assert_eq!(o.status(), 0xFF, "all four");
        assert_eq!(o.voices_playing(), 0x0F);
    }

    /// A phrase runs for `2 * (stop - start + 1)` samples and then stops
    /// itself. Measured: phrase 1 is 256 samples, all 256 non-zero, and the
    /// chip returns to F0 on its own.
    #[test]
    fn a_phrase_runs_its_measured_length_and_then_stops_itself() {
        let rom = rom();
        let mut o = Oki::new();
        o.write(0x81, &rom);
        o.write(0x10, &rom);
        let mut samples = 0;
        let mut nonzero = 0;
        while o.status() != STATUS_IDLE && samples < 1000 {
            let s = o.step(&rom);
            samples += 1;
            if s != 0 {
                nonzero += 1;
            }
        }
        assert_eq!(samples, 256, "2 * (0x107F - 0x1000 + 1)");
        assert_eq!(nonzero, 256, "every sample of this phrase is audible");
        assert_eq!(o.status(), STATUS_IDLE);
    }

    /// The high nibble comes first. Asserted against the ROM byte, not against
    /// a second call to the fetch: voice 0's first decoded sample must be
    /// `diff(0, rom[0x1000] >> 4)`.
    #[test]
    fn the_high_nibble_is_decoded_first() {
        let rom = rom();
        let byte = rom[0x1000];
        assert_ne!(byte >> 4, byte & 0x0F, "the fixture must distinguish them");
        let mut o = Oki::new();
        o.write(0x81, &rom);
        o.write(0x10, &rom); // volume index 0 = unity
        let got = o.step(&rom);
        let want = i32::from(crate::adpcm::diff(0, byte >> 4)) * i32::from(VOLUME_TABLE[0]);
        assert_eq!(i32::from(got), want);
    }

    /// Measured: with four voices playing -- voices 0 and 1 on phrase 1, voices
    /// 2 and 3 on phrase 2 -- one step draws nibbles 5, 5, 6, 6 in voice order
    /// and sums to 3072 in the 2x domain.
    #[test]
    fn four_voices_sum_the_way_the_reference_sums() {
        let rom = rom();
        let mut o = Oki::new();
        o.write(0x81, &rom);
        o.write(0x10, &rom);
        o.write(0x81, &rom);
        o.write(0x20, &rom);
        o.write(0x82, &rom);
        o.write(0x40, &rom);
        o.write(0x82, &rom);
        o.write(0x80, &rom);
        assert_eq!(o.step(&rom), 3072);
    }

    /// The traced form reports the same sample as the plain one and packs voice
    /// 0 in the low bits. The measured draw is 5, 5, 6, 6 in voice order, so the
    /// packed word is `0x6655` -- asserting the whole word is what proves the
    /// shift is 4 bits per voice and that voice 0 is the low field, which a
    /// per-nibble assertion on equal values could not.
    #[test]
    fn the_traced_step_packs_the_nibbles_voice_zero_first() {
        let rom = rom();
        let mut o = Oki::new();
        for byte in [0x81, 0x10, 0x81, 0x20, 0x82, 0x40, 0x82, 0x80] {
            o.write(byte, &rom);
        }
        let (mono, nibbles) = o.step_2x_traced(&rom);
        assert_eq!(mono, 3072);
        assert_eq!(nibbles, 0x6655, "5, 5, 6, 6 in voice order, voice 0 lowest");

        // A single voice fills only its own field, so an idle voice reads zero
        // rather than leaving stale bits.
        let mut p = Oki::new();
        p.write(0x81, &rom);
        p.write(0x40, &rom); // voice 2 alone
        let (_, only) = p.step_2x_traced(&rom);
        assert_eq!(only & 0xF0FF, 0, "voices 0, 1 and 3 are silent");
        assert_ne!(only & 0x0F00, 0, "and voice 2's nibble is where it belongs");
    }

    /// An idle chip traces nothing, which is what makes `nibbles == 0` in a
    /// vector case mean "no voice played" rather than "the field is unset".
    #[test]
    fn an_idle_chip_traces_no_nibbles() {
        let rom = rom();
        let mut o = Oki::new();
        assert_eq!(o.step_2x_traced(&rom), (0, 0));
    }

    /// The clamp report is about whether the clamp *bit*, not about the value
    /// landing on the boundary. A sum of exactly `+-CLAMP_2X` reports `false`.
    #[test]
    fn the_clamp_report_distinguishes_reaching_the_bound_from_exceeding_it() {
        let rom = rom();
        // Four voices on the same saturating ramp: measured to peak at 262016
        // unclamped, so it clips hard.
        let mut clipping = vec![0u8; 0x4000];
        put_phrase(&mut clipping, 1, 0x1000, 0x107F);
        clipping[0x1000..0x1080].fill(0x77);
        let mut o = Oki::new();
        for byte in [0x81, 0x10, 0x81, 0x20, 0x81, 0x40, 0x81, 0x80] {
            o.write(byte, &clipping);
        }
        let clipped = (0..64).filter(|_| o.step_2x_clamped(&clipping).1).count();
        assert_eq!(clipped, 61, "measured against MAME's decoder: 61 of 64");

        // And an idle chip -- a sum of zero, nowhere near the bound -- reports
        // false, so the flag is not simply always set.
        let mut q = Oki::new();
        assert_eq!(q.step_2x_clamped(&rom), (0, false));
    }

    /// All three step forms agree, so the traced and counted projections cannot
    /// drift from the one the vector suite checks.
    #[test]
    fn the_three_step_forms_agree() {
        let rom = rom();
        let start = |o: &mut Oki| {
            o.write(0x81, &rom);
            o.write(0x10, &rom);
        };
        let mut plain = Oki::new();
        let mut traced = Oki::new();
        let mut counted = Oki::new();
        start(&mut plain);
        start(&mut traced);
        start(&mut counted);
        let mut audible = false;
        for n in 0..64 {
            let p = plain.step_2x(&rom);
            audible |= p != 0;
            assert_eq!(traced.step_2x_traced(&rom).0, p, "traced diverged at {n}");
            assert_eq!(
                counted.step_2x_clamped(&rom).0,
                p,
                "counted diverged at {n}"
            );
        }
        assert!(audible, "the comparison must be of something");
    }

    /// Volume indices 9 through 15 are exactly zero, so a voice at one of them
    /// plays silently -- it keeps running and keeps advancing, it just
    /// contributes nothing.
    #[test]
    fn the_silent_volume_indices_produce_exactly_no_energy() {
        let rom = rom();
        for index in 9..16u8 {
            assert_eq!(VOLUME_TABLE[usize::from(index)], 0, "index {index}");
        }
        let mut o = Oki::new();
        o.write(0x81, &rom);
        o.write(0x10 | 9, &rom);
        let mut energy: i64 = 0;
        for _ in 0..50 {
            energy += i64::from(o.step(&rom).abs());
        }
        assert_eq!(energy, 0, "50 samples at volume index 9");
        assert_eq!(o.status(), 0xF1, "and it is still playing");
    }

    /// A byte with bit 7 clear stops voices, its mask shifted by 3 rather
    /// than 4.
    #[test]
    fn a_stop_command_shifts_its_mask_by_three() {
        let rom = rom();
        let mut o = Oki::new();
        o.write(0x81, &rom);
        o.write(0x10, &rom);
        assert_eq!(o.status(), 0xF1);
        o.write(0x08, &rom); // 8 >> 3 == 1: voice 0
        assert_eq!(o.status(), STATUS_IDLE);

        // And 0x10 >> 3 == 2 stops voice 1, not voice 0.
        let mut p = Oki::new();
        p.write(0x81, &rom);
        p.write(0x30, &rom); // voices 0 and 1
        assert_eq!(p.status(), 0xF3);
        p.write(0x10, &rom);
        assert_eq!(p.status(), 0xF1, "voice 1 stopped, voice 0 still playing");
    }

    /// `start >= stop` is refused, and refusing it leaves the chip idle
    /// rather than playing a phrase of absurd length.
    ///
    /// **Both sides of the comparison**, because `>` and `>=` are a different rule and
    /// only one fixture separates them. `start > stop` (phrase 3) is refused by
    /// `start < stop` and by `start <= stop` alike, so on its own it says nothing about
    /// which of the two is implemented — the mutation to `<=` survived the first draft
    /// of this test. Phrase 4 is `start == stop`, which the real chip refuses and `<=`
    /// would accept as a two-nibble phrase.
    #[test]
    fn a_phrase_whose_start_is_not_below_its_stop_is_refused() {
        let rom = rom();
        let mut o = Oki::new();
        o.write(0x83, &rom); // start 0x3000, stop 0x2FFF
        o.write(0x10, &rom);
        assert_eq!(o.status(), STATUS_IDLE, "nothing started");

        let mut o = Oki::new();
        o.write(0x84, &rom); // start == stop == 0x1800
        o.write(0x10, &rom);
        assert_eq!(
            o.status(),
            STATUS_IDLE,
            "a degenerate phrase is refused too: the comparison is `<`, not `<=`"
        );
        // And the premise, so a fixture that lost phrase 4 fails here rather than
        // passing vacuously: the entry really is start == stop.
        assert_eq!(
            (read24(&rom, 4 * 8), read24(&rom, 4 * 8 + 3)),
            (0x1800, 0x1800),
            "the fixture's degenerate phrase"
        );
    }

    /// MAME skips a voice that is already playing ("fixes Got-cha and Steel
    /// Force"). Asserted through the audio: the voice must keep decoding its
    /// original phrase, not restart.
    #[test]
    fn a_command_for_a_playing_voice_is_ignored() {
        let rom = rom();
        let mut reference = Oki::new();
        reference.write(0x81, &rom);
        reference.write(0x10, &rom);
        let want: Vec<i16> = (0..8).map(|_| reference.step(&rom)).collect();

        let mut o = Oki::new();
        o.write(0x81, &rom);
        o.write(0x10, &rom);
        let mut got = Vec::new();
        for i in 0..8 {
            if i == 4 {
                o.write(0x82, &rom); // a different phrase, same voice
                o.write(0x10, &rom);
            }
            got.push(o.step(&rom));
        }
        assert_eq!(got, want, "the interrupting command changed the audio");
    }

    /// The chip clamps its own summed output before anything downstream sees
    /// it (`okim6295.cpp:188` clamps the stream to +-1.0, which is +-65536 in
    /// the 2x domain). Two voices at volume index 0 and full signal already
    /// exceed it, so this is reachable in normal play, not a corner.
    #[test]
    fn the_chip_clamps_its_own_sum() {
        assert_eq!(
            CLAMP_2X, 65_536,
            "+-1.0 stream == +-32768 i16 == +-65536 at 2x"
        );
        // One voice at full scale fits; two do not.
        let one = i32::from(crate::adpcm::SIGNAL_MAX) * i32::from(VOLUME_TABLE[0]);
        assert_eq!(one, 65_504, "one voice fits under the clamp");
        assert!(2 * one > CLAMP_2X, "two voices exceed it");

        // Drive all four voices to saturation on the same ramp and confirm the
        // step output stops at the clamp.
        let mut rom = vec![0u8; 0x4_0000];
        put_phrase(&mut rom, 1, 0x1000, 0x10FF);
        rom[0x1000..=0x10FF].fill(0x77); // every nibble a 7: ramp up hard
        let mut o = Oki::new();
        for voice in 0..4u8 {
            o.write(0x81, &rom);
            // `vvvv gggg` with volume index 0: the voice bit alone.
            o.write(1 << (voice + 4), &rom);
        }
        let mut peak = 0i16;
        for _ in 0..64 {
            peak = peak.max(o.step(&rom));
        }
        assert_eq!(
            i32::from(peak),
            CLAMP_2X.min(i32::from(i16::MAX)),
            "the sum must saturate, and at i16 the clamp is i16::MAX"
        );
    }

    /// A reset stops every voice and clears any half-delivered command, so the
    /// byte after a reset is read as a fresh command rather than as the second
    /// half of the one that was pending.
    #[test]
    fn a_reset_stops_every_voice_and_drops_a_pending_command() {
        let rom = rom();
        let mut o = Oki::new();
        o.write(0x81, &rom);
        o.write(0xF0, &rom); // all four voices
        assert_eq!(o.status(), 0xFF);
        o.write(0x81, &rom); // latch a command, do not complete it
        o.reset();
        assert_eq!(o.status(), STATUS_IDLE);
        // If the pending command had survived, this byte would be read as
        // `vvvv gggg` and start voice 0.
        o.write(0x10, &rom);
        assert_eq!(
            o.status(),
            STATUS_IDLE,
            "0x10 with no command pending is a stop"
        );
    }

    /// A phrase pointing past the end of a short ROM reads as zero rather
    /// than panicking: the ROM's size is the user's file's business.
    ///
    /// The plan expected sample 9 to be 0 here, "signal pinned at 0". That is
    /// wrong, and wrong in a way the decoder documents: nibble 0 carries the
    /// unconditional `stepval / 8` term, so a ROM of zeroes is not silence. The
    /// step index is pinned at 0 (nibble 0 shifts it by -1), so every sample
    /// adds `diff(0, 0) == 2`; nine samples in the signal is 18 and the output
    /// is `18 * 0x20 == 576`.
    #[test]
    fn a_short_rom_reads_as_zero_rather_than_panicking() {
        let mut rom = vec![0u8; 0x40];
        rom[8] = 0x03; // phrase 1 start 0x030000
        rom[9] = 0x00;
        rom[10] = 0x00;
        rom[11] = 0x03; // stop 0x030010
        rom[12] = 0x00;
        rom[13] = 0x10;
        let mut o = Oki::new();
        o.write(0x81, &rom);
        o.write(0x10, &rom);
        assert_eq!(o.status(), 0xF1, "the phrase is well-formed, just unbacked");
        let mut last = 0;
        for _ in 0..9 {
            last = o.step(&rom);
        }
        assert_eq!(
            last, 576,
            "9 * diff(0, 0) * 0x20, not a panic and not silence"
        );
    }

    /// A restored chip plays on identically -- asserted through the samples it
    /// produces, not by comparing the fields that were just assigned.
    #[test]
    fn a_restored_chip_plays_on_from_where_it_was_saved() {
        let rom = rom();
        let mut saved = Oki::new();
        saved.write(0x81, &rom);
        saved.write(0x10, &rom);
        saved.write(0x82, &rom);
        saved.write(0x40, &rom);
        for _ in 0..20 {
            saved.step_2x(&rom);
        }
        saved.write(0x83, &rom); // latch a command and leave it pending
        let mut rebuilt = Oki::restore(*saved.voices(), saved.pending_command());
        assert_eq!(rebuilt.pending_command(), Some(0x03));

        let from_saved: Vec<i32> = (0..16).map(|_| saved.step_2x(&rom)).collect();
        let from_rebuilt: Vec<i32> = (0..16).map(|_| rebuilt.step_2x(&rom)).collect();
        assert_eq!(from_saved, from_rebuilt);
        assert!(
            from_saved.iter().any(|&s| s != 0),
            "the comparison must be of something"
        );
    }

    /// What `restore` refuses: a position past the end of its own phrase, a
    /// base wider than the 18-bit bus, and a command byte with bit 7 still set.
    #[test]
    fn a_restore_repairs_the_fields_a_save_file_could_get_wrong() {
        let dead = Voice::restore(Adpcm::new(), true, 0x1000, 40, 40, 0x20);
        assert!(
            !dead.playing(),
            "a voice saved past its own phrase is stopped"
        );
        let live = Voice::restore(Adpcm::new(), true, 0x1000, 39, 40, 0x20);
        assert!(live.playing());
        assert_eq!(
            Voice::restore(Adpcm::new(), true, 0x4_1234, 0, 4, 0x20).base(),
            0x1234,
            "the base is masked to the 18-bit bus"
        );

        let o = Oki::restore([Voice::default(); VOICES], Some(0xFF));
        assert_eq!(
            o.pending_command(),
            Some(0x7F),
            "bit 7 is not part of the phrase number"
        );
        assert_eq!(o.status(), STATUS_IDLE);
    }

    #[test]
    fn there_are_four_voices_and_sixteen_volume_levels() {
        assert_eq!(VOICES, 4);
        assert_eq!(VOLUME_TABLE.len(), 16);
        assert_eq!(VOLUME_TABLE[0], 0x20, "index 0 is unity: 0x20/0x20");
    }
}
