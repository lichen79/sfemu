//! The `AYMV` vector format: YM2151 register scripts and the samples they produce.
//!
//! The OPM has no published vector suite, so this one is generated: `genym` builds a
//! C++ program against ymfm (the implementation MAME uses), runs a deterministic
//! register script per case, and writes what ymfm produced. This module is the
//! contract between that generator and the runner.
//!
//! ```text
//! file:   u32 magic 0x564D_5941   u32 num_cases
//! case:   u32 seed, u16 num_writes, write[num_writes],
//!         u16 num_samples, sample[num_samples], u8 final_status
//! write:  u16 at_sample, u8 reg, u8 val
//! sample: i16 left, i16 right, u8 status
//! ```
//!
//! All little-endian.
//!
//! # Three fields that exist because of a measurement
//!
//! 1. **`seed`.** The whole script is a function of it, so a failing case can be
//!    regenerated from one integer without shipping the script that produced it.
//! 2. **`at_sample` on every write, rather than one script replayed up front.** A
//!    key-off *during* the window is the only thing that reaches the release rate:
//!    the spec measured RR bit 0 as undetected in 0 of 200 cases until every case
//!    keyed off at sample 256.
//! 3. **`status` on every sample, not just at the end.** Timer state is inaudible —
//!    a run where the timers were mis-clocked produced byte-identical audio in 0 of
//!    200 cases until the record carried the status byte. `final_status` is kept as
//!    well, and is redundant with the last sample's by construction; the runner
//!    asserts they agree, which is how a writer that fills one and not the other is
//!    caught.
//!
//! Case **names are dropped**, as in [`crate::z80fmt`]: a case is identified by its
//! index and its seed, and both are recoverable.

/// `AYMV` in file order. As a little-endian `u32` that is `0x564D_5941` — writing
/// the letters in reading order would give `0x4159_4D56`, a different file.
pub const MAGIC: u32 = 0x564D_5941;

/// One register write, and the sample it happens before.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Write {
    /// The write lands before this sample is generated. Writes at the same sample
    /// apply in file order, which is the order the generator emitted them.
    pub at_sample: u16,
    /// The register address, `0x01`-`0xFF`.
    pub reg: u8,
    /// The byte written.
    pub val: u8,
}

/// One stereo sample and the status register as it read at that moment.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Sample {
    pub left: i16,
    pub right: i16,
    /// Bit 0 timer A, bit 1 timer B, bit 7 BUSY. Read through the chip's own status
    /// read, so a core that tracks timers but never exposes them still fails.
    pub status: u8,
}

/// One case: a register script and the samples ymfm produced from it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Case {
    /// The xorshift64 seed the script was drawn from.
    pub seed: u32,
    /// Every write, sorted by `at_sample`.
    pub writes: Vec<Write>,
    /// The samples, one per generated sample.
    pub samples: Vec<Sample>,
    /// The status after the last sample — see the module docs on why it is kept.
    pub final_status: u8,
}

/// A whole vector file.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Vectors {
    pub cases: Vec<Case>,
}

/// How many bytes a case with `writes` writes and `samples` samples occupies.
///
/// Named in lower case although the plan wrote `CASE_BYTES`: a `const fn` in
/// `SCREAMING_CASE` trips `non_snake_case`, which this workspace denies.
#[must_use]
pub const fn case_bytes(writes: usize, samples: usize) -> usize {
    // 4 seed + 2 write count + 2 sample count + 1 final status.
    9 + 4 * writes + 5 * samples
}

impl Vectors {
    /// Serializes to the format the module documents.
    ///
    /// # Panics
    ///
    /// If a count exceeds its field: more than 65,535 writes or samples in a case,
    /// or more than `u32::MAX` cases. The generator asserts the same bounds before
    /// it gets here; a silent `as u16` is how a format grows a truncation bug when
    /// someone regenerates with different options.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(&MAGIC.to_le_bytes());
        o.extend_from_slice(
            &u32::try_from(self.cases.len())
                .expect("case count fits u32")
                .to_le_bytes(),
        );
        for c in &self.cases {
            o.extend_from_slice(&c.seed.to_le_bytes());
            o.extend_from_slice(
                &u16::try_from(c.writes.len())
                    .expect("write count fits u16")
                    .to_le_bytes(),
            );
            for w in &c.writes {
                o.extend_from_slice(&w.at_sample.to_le_bytes());
                o.push(w.reg);
                o.push(w.val);
            }
            o.extend_from_slice(
                &u16::try_from(c.samples.len())
                    .expect("sample count fits u16")
                    .to_le_bytes(),
            );
            for s in &c.samples {
                o.extend_from_slice(&s.left.to_le_bytes());
                o.extend_from_slice(&s.right.to_le_bytes());
                o.push(s.status);
            }
            o.push(c.final_status);
        }
        o
    }
}

/// What can be wrong with a vector file.
#[derive(Debug)]
pub enum FormatError {
    /// The first four bytes are not [`MAGIC`].
    BadMagic { want: u32, got: u32 },
    /// The file ended mid-record.
    Truncated { at: usize, need: usize },
}

impl core::fmt::Display for FormatError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FormatError::BadMagic { want, got } => {
                write!(f, "bad magic: want {want:08X}, got {got:08X}")
            }
            FormatError::Truncated { at, need } => {
                write!(f, "truncated at byte {at}: need {need} more")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// A cursor that cannot read past its end.
struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let end = self.at.checked_add(n).ok_or(FormatError::Truncated {
            at: self.at,
            need: n,
        })?;
        if end > self.b.len() {
            return Err(FormatError::Truncated {
                at: self.at,
                need: end - self.b.len(),
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
}

/// Parses a whole vector file.
///
/// # Errors
///
/// [`FormatError::BadMagic`] if the header is not an `AYMV` file, or
/// [`FormatError::Truncated`] if it ends mid-record.
pub fn parse(bytes: &[u8]) -> Result<Vectors, FormatError> {
    let mut r = Rd { b: bytes, at: 0 };
    let magic = r.u32()?;
    if magic != MAGIC {
        return Err(FormatError::BadMagic {
            want: MAGIC,
            got: magic,
        });
    }
    let n = r.u32()? as usize;
    // Not `with_capacity(n)`: `n` comes from the file, so a corrupt header would ask
    // for an arbitrary allocation before a single record is validated.
    let mut cases = Vec::new();
    for _ in 0..n {
        let seed = r.u32()?;
        let nw = r.u16()? as usize;
        let mut writes = Vec::with_capacity(nw);
        for _ in 0..nw {
            writes.push(Write {
                at_sample: r.u16()?,
                reg: r.u8()?,
                val: r.u8()?,
            });
        }
        let ns = r.u16()? as usize;
        let mut samples = Vec::with_capacity(ns);
        for _ in 0..ns {
            samples.push(Sample {
                left: r.u16()? as i16,
                right: r.u16()? as i16,
                status: r.u8()?,
            });
        }
        cases.push(Case {
            seed,
            writes,
            samples,
            final_status: r.u8()?,
        });
    }
    Ok(Vectors { cases })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The magic is 'A','Y','M','V' in file order.
    ///
    /// Written as a little-endian u32, so the constant reads backwards from the file
    /// bytes. Spelling it out both ways here is what stops a byte-order slip from
    /// silently rejecting every file the generator writes.
    #[test]
    fn the_magic_is_aymv_in_file_order() {
        assert_eq!(MAGIC, 0x564D_5941);
        assert_eq!(MAGIC.to_le_bytes(), [b'A', b'Y', b'M', b'V']);
    }

    /// A case round-trips through the encoder and the parser unchanged.
    #[test]
    fn a_case_round_trips() {
        let case = Case {
            seed: 0xDEAD_BEEF,
            writes: vec![
                Write {
                    at_sample: 0,
                    reg: 0x20,
                    val: 0xC7,
                },
                Write {
                    at_sample: 256,
                    reg: 0x08,
                    val: 0x00,
                },
            ],
            samples: vec![
                Sample {
                    left: -1234,
                    right: 5678,
                    status: 0x00,
                },
                Sample {
                    left: 0,
                    right: 0,
                    status: 0x81,
                },
            ],
            final_status: 0x81,
        };
        let vectors = Vectors {
            cases: vec![case.clone()],
        };
        let bytes = vectors.encode();
        let back = parse(&bytes).expect("round trip");
        assert_eq!(back.cases.len(), 1);
        assert_eq!(back.cases[0], case);
    }

    /// A 512-sample, 272-write case is exactly 3,657 bytes.
    ///
    /// 5 bytes per sample x 512, plus 4 bytes per write x 272, plus 9 header and
    /// trailer bytes: a u32 seed, two u16 counts, and a u8 final status. A format
    /// change that grows the record fails here rather than surprising anyone at 10x
    /// the disk.
    ///
    /// 272 is a worst case, not the measured mean. The plan projected 3.657 MB from
    /// it, which assumed every case writes all eight channels; the script writes 1-3,
    /// so the generated suite measured **63 writes mean, 99 max, 2,822,420 bytes**
    /// for 1,000 cases. The 272 figure is kept as the assertion's anchor because a
    /// bound above the maximum is the right thing to pin a size formula at.
    #[test]
    fn the_measured_case_size_is_three_thousand_six_hundred_and_fifty_seven_bytes() {
        assert_eq!(case_bytes(272, 512), 3_657);
        assert_eq!(5 * 512 + 4 * 272 + 9, 3_657);

        // The two lines above are the same arithmetic written twice, so on their own
        // they hold even if `encode` lays out something else entirely. What ties the
        // figure to the format is measuring a real encoded case: 8 header bytes for
        // the file, and the rest is the case.
        let case = Case {
            seed: 7,
            writes: vec![Write::default(); 272],
            samples: vec![Sample::default(); 512],
            final_status: 0,
        };
        let bytes = Vectors {
            cases: vec![case.clone()],
        }
        .encode();
        assert_eq!(
            bytes.len(),
            8 + case_bytes(272, 512),
            "one case plus header"
        );

        // And it is linear in both counts, which is what makes the 3.657 MB estimate
        // for 1,000 cases a prediction rather than a coincidence.
        let two = Vectors {
            cases: vec![case; 2],
        }
        .encode();
        assert_eq!(two.len(), 8 + 2 * case_bytes(272, 512));
    }

    /// A truncated file is rejected, not silently short-read.
    ///
    /// A parser that returns the cases it managed to read turns a corrupt download
    /// into a smaller passing suite. Every truncation point must be an error.
    #[test]
    fn every_truncation_is_an_error() {
        let vectors = Vectors {
            cases: vec![Case {
                seed: 1,
                writes: vec![Write {
                    at_sample: 0,
                    reg: 0x20,
                    val: 0xC7,
                }],
                samples: vec![Sample {
                    left: 1,
                    right: 2,
                    status: 3,
                }],
                final_status: 0,
            }],
        };
        let bytes = vectors.encode();
        for cut in 0..bytes.len() {
            assert!(
                parse(&bytes[..cut]).is_err(),
                "truncated at {cut} must fail"
            );
        }
        assert!(parse(&bytes).is_ok(), "and the whole thing must not");
    }

    /// A wrong magic is rejected with a message that names what was found.
    #[test]
    fn a_wrong_magic_is_rejected_by_name() {
        let mut bytes = Vectors { cases: vec![] }.encode();
        bytes[0] = b'X';
        let err = parse(&bytes).expect_err("must reject");
        assert!(
            format!("{err}").contains("magic"),
            "says what is wrong: {err}"
        );
    }
}
