//! Distributing a source file's bytes into its region.
//!
//! Pure arithmetic over slices: no I/O, no archive, no allocation beyond the
//! caller's `dest`. Separated from the loader for exactly that reason — the
//! interleave is the part with an off-by-one in it, and it is testable against a
//! synthetic pattern with nothing else in the loop.

use crate::spec::{LoadKind, RomEntry};
use crate::RomError;

/// The last byte index this entry writes, exclusive.
///
/// Used for the bounds check in [`place`] and by the region-size tests in the
/// game tables, which is where a transcription slip shows up first.
pub fn end_of(entry: &RomEntry) -> usize {
    match entry.load {
        LoadKind::Byte => entry.offset + entry.len,
        // The final byte is at offset + 2*(len-1), so the exclusive end is one
        // past it.
        LoadKind::Word16Byte => entry.offset + 2 * entry.len.saturating_sub(1) + 1,
        LoadKind::Word64Word => {
            let words = entry.len / 2;
            entry.offset + 8 * words.saturating_sub(1) + 2
        }
        LoadKind::Continue { split, cont_at } => {
            (entry.offset + split).max(cont_at + entry.len.saturating_sub(split))
        }
    }
}

/// Writes `src` into `dest` according to `entry.load`.
///
/// `region` is used only to name the region in [`RomError::SpecOverflow`].
///
/// # Errors
///
/// [`RomError::SpecOverflow`] if the entry would write past the end of `dest`.
/// That is our table being wrong, never the user's file.
pub fn place(
    dest: &mut [u8],
    src: &[u8],
    entry: &RomEntry,
    region: &'static str,
) -> Result<(), RomError> {
    let end = end_of(entry);
    if end > dest.len() {
        return Err(RomError::SpecOverflow {
            region,
            name: entry.name,
            end,
            size: dest.len(),
        });
    }
    match entry.load {
        LoadKind::Byte => dest[entry.offset..entry.offset + src.len()].copy_from_slice(src),
        LoadKind::Word16Byte => {
            for (i, &b) in src.iter().enumerate() {
                dest[entry.offset + 2 * i] = b;
            }
        }
        LoadKind::Word64Word => {
            for (i, pair) in src.chunks_exact(2).enumerate() {
                let at = entry.offset + 8 * i;
                dest[at] = pair[0];
                dest[at + 1] = pair[1];
            }
        }
        LoadKind::Continue { split, cont_at } => {
            let (a, b) = src.split_at(split.min(src.len()));
            dest[entry.offset..entry.offset + a.len()].copy_from_slice(a);
            dest[cont_at..cont_at + b.len()].copy_from_slice(b);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{LoadKind, RomEntry};

    /// The synthetic pattern is chosen so that a wrong interleave is visible at
    /// byte 1.
    ///
    /// ⚠️ A zero-filled or constant source would make `Byte` and `Word16Byte`
    /// produce **identical** output, so the test would pass with the interleave
    /// completely wrong. That is the same shape as this project's crossed-widths
    /// defect: the input has to *discriminate*, not merely be present. `0xA0 |`
    /// and `0xB0 |` make the two source files distinguishable in every byte.
    fn pat(tag: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| tag | (i as u8 & 0x0F)).collect()
    }

    fn entry(offset: usize, len: usize, load: LoadKind) -> RomEntry {
        RomEntry {
            name: "t",
            offset,
            len,
            crc32: 0,
            load,
        }
    }

    #[test]
    fn word16_byte_interleaves_even_file_into_the_high_byte() {
        let mut dest = vec![0u8; 16];
        place(
            &mut dest,
            &pat(0xA0, 4),
            &entry(0, 4, LoadKind::Word16Byte),
            "r",
        )
        .unwrap();
        place(
            &mut dest,
            &pat(0xB0, 4),
            &entry(1, 4, LoadKind::Word16Byte),
            "r",
        )
        .unwrap();
        assert_eq!(
            &dest[..8],
            &[0xA0, 0xB0, 0xA1, 0xB1, 0xA2, 0xB2, 0xA3, 0xB3],
            "even entry supplies the high byte of each big-endian word"
        );
        // The first 68000 word of this region is therefore 0xA0B0, not 0xB0A0.
        assert_eq!(u16::from_be_bytes([dest[0], dest[1]]), 0xA0B0);
    }

    #[test]
    fn byte_kind_is_a_straight_copy_and_differs_from_word16() {
        let mut dest = vec![0u8; 16];
        place(&mut dest, &pat(0xA0, 4), &entry(0, 4, LoadKind::Byte), "r").unwrap();
        assert_eq!(&dest[..4], &[0xA0, 0xA1, 0xA2, 0xA3]);
        // The discrimination this test exists for: the same source under the two
        // kinds must not agree.
        let mut other = vec![0u8; 16];
        place(
            &mut other,
            &pat(0xA0, 4),
            &entry(0, 4, LoadKind::Word16Byte),
            "r",
        )
        .unwrap();
        assert_ne!(dest, other, "Byte and Word16Byte must be distinguishable");
    }

    #[test]
    fn word64_word_strides_two_bytes_every_eight() {
        let mut dest = vec![0u8; 32];
        place(
            &mut dest,
            &pat(0xA0, 4),
            &entry(0, 4, LoadKind::Word64Word),
            "r",
        )
        .unwrap();
        assert_eq!(&dest[0..2], &[0xA0, 0xA1]);
        assert_eq!(&dest[8..10], &[0xA2, 0xA3]);
        assert_eq!(&dest[2..8], &[0, 0, 0, 0, 0, 0], "the gap stays untouched");
    }

    #[test]
    fn continue_splits_the_file_across_two_offsets() {
        let mut dest = vec![0u8; 0x20];
        let e = entry(
            0x00,
            0x10,
            LoadKind::Continue {
                split: 0x08,
                cont_at: 0x10,
            },
        );
        place(&mut dest, &pat(0xA0, 0x10), &e, "r").unwrap();
        assert_eq!(dest[0x00], 0xA0, "first half at offset");
        assert_eq!(dest[0x07], 0xA7);
        assert_eq!(dest[0x08], 0x00, "nothing between the halves");
        assert_eq!(dest[0x10], 0xA8, "second half at cont_at");
        assert_eq!(dest[0x17], 0xAF);
    }

    /// `end_of` must report the exact minimum region size for every kind.
    ///
    /// Found by mutation: dropping the `+ 1` from the `Word16Byte` arm survived
    /// the whole suite, because the overflow test below uses `Byte`, whose end is
    /// trivially `offset + len`. An interleaved kind's end is the part with the
    /// arithmetic in it, and under-reporting it turns a table error into an
    /// index-out-of-bounds panic inside `place` — the bounds check exists
    /// precisely so that cannot happen.
    ///
    /// Each pair below asserts the minimum both ways: one byte short must fail,
    /// exactly enough must succeed.
    #[test]
    fn end_of_reports_the_exact_minimum_region_size_for_each_kind() {
        // Word16Byte, 4 bytes at odd offset 1: last write is dest[1 + 2*3] = dest[7],
        // so 8 bytes are needed.
        let e = entry(1, 4, LoadKind::Word16Byte);
        assert_eq!(end_of(&e), 8);
        assert!(place(&mut [0u8; 7], &pat(0xA0, 4), &e, "r").is_err());
        assert!(place(&mut [0u8; 8], &pat(0xA0, 4), &e, "r").is_ok());

        // Word64Word, 4 bytes = 2 words: last write is dest[8..10], so 10 bytes.
        let e = entry(0, 4, LoadKind::Word64Word);
        assert_eq!(end_of(&e), 10);
        assert!(place(&mut [0u8; 9], &pat(0xA0, 4), &e, "r").is_err());
        assert!(place(&mut [0u8; 10], &pat(0xA0, 4), &e, "r").is_ok());

        // Continue with cont_at BELOW offset: the second half is what reaches
        // furthest here, so taking only the first term under-reports the end.
        let e = entry(
            0x20,
            0x10,
            LoadKind::Continue {
                split: 0x04,
                cont_at: 0x00,
            },
        );
        assert_eq!(end_of(&e), 0x24, "offset + split, the higher of the two");
        // And with cont_at above, the second term wins.
        let e = entry(
            0x00,
            0x10,
            LoadKind::Continue {
                split: 0x04,
                cont_at: 0x20,
            },
        );
        assert_eq!(end_of(&e), 0x2C, "cont_at + (len - split)");
        assert!(place(&mut [0u8; 0x2B], &pat(0xA0, 0x10), &e, "r").is_err());
        assert!(place(&mut [0u8; 0x2C], &pat(0xA0, 0x10), &e, "r").is_ok());

        // Byte, for completeness.
        assert_eq!(end_of(&entry(3, 5, LoadKind::Byte)), 8);
    }

    #[test]
    fn an_entry_past_the_end_of_its_region_is_our_bug_and_says_so() {
        let mut dest = vec![0u8; 4];
        let err = place(
            &mut dest,
            &pat(0xA0, 4),
            &entry(2, 4, LoadKind::Byte),
            "maincpu",
        )
        .unwrap_err();
        assert_eq!(
            err,
            crate::RomError::SpecOverflow {
                region: "maincpu",
                name: "t",
                end: 6,
                size: 4
            }
        );
    }
}
