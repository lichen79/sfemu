//! The `AOKV` OKI vector-file codec.
//!
//! One file holds every case. A case is a synthesised sample ROM, a script of
//! command-byte writes, and the mono output the reference produced.
//!
//! ```text
//! file:   u32 magic 0x564B_4F41   u32 num_cases
//! case:   u32 seed, u8 pin7, u16 num_writes, u16 num_samples, u32 rom_len,
//!         write[num_writes], u8 rom[rom_len], sample[num_samples]
//! write:  u16 at_sample, u8 byte
//! sample: i32 mono_2x, u8 status, u8 voices, u16 nibbles
//! ```
//!
//! All little-endian.
//!
//! # Two fields that depart from the D3 spec
//!
//! 1. **`mono_2x` is `i32`, not the spec's `i16`.** The chip clamps its own
//!    summed stream to `+-65536` in the 2x domain (`okim6295.cpp:188`), and
//!    `+65536` does not fit an `i16`. Recording it as `i16` would fold the
//!    positive clamp onto `i16::MIN`, which is the one value a saturation bug
//!    would also produce. The record is 8 bytes rather than the spec's 6.
//! 2. **`nibbles` earns its two bytes.** When a case diverges on `mono_2x`
//!    alone, the cause is either the address walk fetching the wrong nibble or
//!    the decoder mishandling the right one — different bugs in different
//!    files. Recording what each voice consumed tells the report which. The
//!    reference already emits it (`target/okiref/wrap.cpp:50`).
//!
//! Case **names are dropped**, as in [`crate::z80fmt`] and [`crate::ymfmt`]: a
//! case is identified by its index and its seed, and the seed reproduces it.

/// `AOKV` in file order. As a little-endian `u32` that is `0x564B_4F41` —
/// writing the letters in reading order would give `0x414F_4B56`, a different
/// file.
pub const MAGIC: u32 = 0x564B_4F41;

/// One command-byte write, scheduled at a sample index.
///
/// Named with a trailing underscore because `Write` is [`std::io::Write`] and a
/// bare `Write` in this module would shadow it for anyone who glob-imports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Write_ {
    /// The byte lands before this sample is generated. Writes at the same
    /// sample apply in file order, which is the order the generator emitted
    /// them.
    pub at_sample: u16,
    /// The byte.
    pub byte: u8,
}

/// One expected output sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    /// The clamped 2x-domain mono sum. `i32` — see the module docs.
    pub mono_2x: i32,
    /// The status byte after this sample: `0xF0` plus one bit per playing voice.
    pub status: u8,
    /// The playing-voice mask after this sample, in the low four bits.
    pub voices: u8,
    /// The nibble each playing voice consumed, voice `v` in bits `4v..4v+3`.
    ///
    /// Non-playing voices contribute zero. This is what separates a wrong
    /// address walk from a wrong decoder when only `mono_2x` diverges.
    pub nibbles: u16,
}

/// One case.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Case {
    /// The generator's seed, equal to the case index.
    pub seed: u32,
    /// The pin-7 state this case was generated at.
    ///
    /// Recorded but not consumed by the decoder: pin 7 selects the sample rate,
    /// and the rate is `machine`'s business. It is here so a case that was
    /// generated at one rate cannot be silently re-read as the other.
    pub pin7: bool,
    /// The command script, in sample order.
    pub writes: Vec<Write_>,
    /// The synthesised sample ROM.
    pub rom: Vec<u8>,
    /// The expected output, one entry per generated sample.
    pub samples: Vec<Sample>,
}

/// Why a file did not parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// The first four bytes were not [`MAGIC`].
    BadMagic {
        /// What was there instead.
        found: u32,
    },
    /// A field or a count ran past the end of the buffer.
    Truncated {
        /// How many bytes were needed at that point.
        needed: usize,
        /// How many remained.
        had: usize,
    },
}

impl core::fmt::Display for FormatError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadMagic { found } => write!(
                f,
                "not an AOKV file: magic {found:#010X}, expected {MAGIC:#010X}"
            ),
            Self::Truncated { needed, had } => {
                write!(f, "truncated: needed {needed} more bytes, had {had}")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// How many bytes a case with these counts occupies, excluding the file header.
///
/// Named in lower case although a size constant would normally scream: a
/// `const fn` in `SCREAMING_CASE` trips `non_snake_case`, which this workspace
/// denies. [`crate::ymfmt::case_bytes`] made the same choice.
#[must_use]
pub const fn case_bytes(writes: usize, samples: usize, rom: usize) -> usize {
    // 4 seed + 1 pin7 + 2 write count + 2 sample count + 4 rom length.
    13 + writes * 3 + rom + samples * 8
}

/// A cursor that cannot read past its end. Every read returns `Result`, so a
/// bad length field in the file is an error rather than a panic.
struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let end = self.at.checked_add(n).ok_or(FormatError::Truncated {
            needed: n,
            had: self.left(),
        })?;
        if end > self.b.len() {
            return Err(FormatError::Truncated {
                needed: n,
                had: self.left(),
            });
        }
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, FormatError> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }

    fn u32(&mut self) -> Result<u32, FormatError> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn i32(&mut self) -> Result<i32, FormatError> {
        Ok(self.u32()? as i32)
    }

    fn left(&self) -> usize {
        self.b.len() - self.at
    }
}

/// Parse a whole file.
///
/// # Errors
///
/// [`FormatError::BadMagic`] if the header is wrong, [`FormatError::Truncated`]
/// if any field or count runs past the end.
pub fn parse(bytes: &[u8]) -> Result<Vec<Case>, FormatError> {
    let mut r = Rd { b: bytes, at: 0 };
    let magic = r.u32()?;
    if magic != MAGIC {
        return Err(FormatError::BadMagic { found: magic });
    }
    let count = r.u32()? as usize;
    // Not `with_capacity(count)`: `count` comes from the file, so a corrupt
    // header would ask for an arbitrary allocation before a single record is
    // validated.
    let mut cases = Vec::new();
    for _ in 0..count {
        let seed = r.u32()?;
        let pin7 = r.u8()? != 0;
        let writes_len = usize::from(r.u16()?);
        let samples_len = usize::from(r.u16()?);
        let rom_len = r.u32()? as usize;
        // Check the whole body fits before reading any of it, so an absurd
        // length field is one error rather than a long loop that ends in one.
        // The three products cannot overflow: `writes_len` and `samples_len`
        // came from a `u16` and `rom_len` from a `u32`.
        let need = writes_len * 3 + rom_len + samples_len * 8;
        if need > r.left() {
            return Err(FormatError::Truncated {
                needed: need,
                had: r.left(),
            });
        }
        let mut writes = Vec::with_capacity(writes_len);
        for _ in 0..writes_len {
            let at_sample = r.u16()?;
            let byte = r.u8()?;
            writes.push(Write_ { at_sample, byte });
        }
        let rom = r.take(rom_len)?.to_vec();
        let mut samples = Vec::with_capacity(samples_len);
        for _ in 0..samples_len {
            let mono_2x = r.i32()?;
            let status = r.u8()?;
            let voices = r.u8()?;
            let nibbles = r.u16()?;
            samples.push(Sample {
                mono_2x,
                status,
                voices,
                nibbles,
            });
        }
        cases.push(Case {
            seed,
            pin7,
            writes,
            rom,
            samples,
        });
    }
    Ok(cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_case() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&MAGIC.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes()); // case count
                                                  // case 0
        b.extend_from_slice(&7u32.to_le_bytes()); // seed
        b.push(1); // pin7
        b.extend_from_slice(&2u16.to_le_bytes()); // writes
        b.extend_from_slice(&3u16.to_le_bytes()); // samples
        b.extend_from_slice(&4u32.to_le_bytes()); // rom bytes
        for (at, byte) in [(0u16, 0x81u8), (0u16, 0x10u8)] {
            b.extend_from_slice(&at.to_le_bytes());
            b.push(byte);
        }
        b.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        for (mono, status, voices, nibbles) in [
            (-65_536i32, 0xF1u8, 1u8, 0x000Fu16),
            (0, 0xF1, 1, 0x0000),
            (65_536, 0xF0, 0, 0xFEDC),
        ] {
            b.extend_from_slice(&mono.to_le_bytes());
            b.push(status);
            b.push(voices);
            b.extend_from_slice(&nibbles.to_le_bytes());
        }
        b
    }

    /// The magic is 'A','O','K','V' in file order.
    ///
    /// Written as a little-endian `u32`, so the constant reads backwards from
    /// the file bytes. Spelling it out both ways is what stops a byte-order
    /// slip from silently rejecting every file the generator writes.
    #[test]
    fn the_magic_is_aokv_in_file_order() {
        assert_eq!(MAGIC, 0x564B_4F41);
        assert_eq!(MAGIC.to_le_bytes(), [b'A', b'O', b'K', b'V']);
    }

    #[test]
    fn a_case_round_trips_every_field() {
        let cases = parse(&one_case()).expect("the fixture must parse");
        assert_eq!(cases.len(), 1);
        let c = &cases[0];
        assert_eq!(c.seed, 7);
        assert!(c.pin7);
        assert_eq!(c.rom, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(c.writes.len(), 2);
        assert_eq!((c.writes[0].at_sample, c.writes[0].byte), (0, 0x81));
        assert_eq!((c.writes[1].at_sample, c.writes[1].byte), (0, 0x10));
        assert_eq!(c.samples.len(), 3);
        assert_eq!(c.samples[0].mono_2x, -65_536);
        assert_eq!(c.samples[0].nibbles, 0x000F);
        assert_eq!(
            c.samples[1].nibbles, 0x0000,
            "no voice playing packs to zero"
        );
        assert_eq!(c.samples[2].mono_2x, 65_536, "the clamp does not fit i16");
        assert_eq!(c.samples[2].status, 0xF0);
        assert_eq!(c.samples[2].voices, 0);
        assert_eq!(
            c.samples[2].nibbles, 0xFEDC,
            "all four nibbles survive, so the field is read as u16 not u8"
        );
    }

    /// `pin7` is a byte in the file and a `bool` here, and any non-zero byte
    /// means true. Reading it as `== 1` would turn a generator that writes
    /// `0xFF` into a silently-false case.
    #[test]
    fn any_nonzero_pin7_byte_reads_as_high() {
        let mut b = one_case();
        b[12] = 0xFF; // 8 file header + 4 seed
        assert!(parse(&b).unwrap()[0].pin7);
        b[12] = 0;
        assert!(!parse(&b).unwrap()[0].pin7);
    }

    #[test]
    fn the_wrong_magic_is_not_a_truncation() {
        let mut b = one_case();
        b[0] ^= 0xFF;
        assert!(matches!(parse(&b), Err(FormatError::BadMagic { .. })));
    }

    /// Every cut point, not a representative one: an off-by-one in a length
    /// field must not read past the buffer.
    #[test]
    fn every_truncation_is_an_error() {
        let full = one_case();
        for cut in 0..full.len() {
            assert!(
                parse(&full[..cut]).is_err(),
                "a {cut}-byte prefix parsed as a whole file"
            );
        }
        assert!(parse(&full).is_ok(), "the uncut file must still parse");
    }

    /// The size arithmetic is a function of the counts, so a reader can check a
    /// file's length before allocating anything from it.
    #[test]
    fn the_case_size_is_the_sum_of_its_parts() {
        assert_eq!(case_bytes(0, 0, 0), 13, "the header alone");
        assert_eq!(case_bytes(2, 3, 4), 13 + 6 + 4 + 24);
        // And the fixture's own body length agrees.
        let parsed = &parse(&one_case()).unwrap()[0];
        assert_eq!(
            case_bytes(parsed.writes.len(), parsed.samples.len(), parsed.rom.len()),
            one_case().len() - 8,
            "8 bytes of file header: magic and count"
        );
    }

    /// A count field larger than the file must not become a huge allocation.
    #[test]
    fn an_absurd_count_is_rejected_before_it_is_allocated() {
        let mut b = one_case();
        b[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(parse(&b), Err(FormatError::Truncated { .. })));
    }

    /// And an absurd *`rom_len`* is caught by the body check rather than by
    /// `to_vec` on a 4 GB slice: the sizes are validated before anything is
    /// read, which is the whole point of that check existing separately from
    /// the per-field bounds test.
    #[test]
    fn an_absurd_rom_length_is_rejected_before_it_is_allocated() {
        let mut b = one_case();
        b[17..21].copy_from_slice(&u32::MAX.to_le_bytes()); // 8 + 4 + 1 + 2 + 2
        match parse(&b) {
            Err(FormatError::Truncated { needed, had }) => {
                assert_eq!(
                    needed,
                    2 * 3 + u32::MAX as usize + 3 * 8,
                    "the whole body length is reported, not one field's"
                );
                assert!(had < needed);
            }
            other => panic!("expected a truncation, got {other:?}"),
        }
    }
}
