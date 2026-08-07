//! CRC-32 (IEEE 802.3, reflected, poly 0xEDB88320) — what zip and MAME both use.
//!
//! Hand-written rather than taken from a crate: it is 20 lines, and the whole
//! point of this crate's dependency budget is that `romset` adds a DEFLATE
//! decoder and nothing else.

/// CRC-32 of `data`.
pub fn of(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            // The reflected polynomial. A non-reflected 0x04C11DB7 here would
            // produce plausible-looking but wrong values for every input.
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::of;

    /// The three standard CRC-32 check values, written as literals.
    ///
    /// `"123456789"` → `0xCBF43926` is the CRC-32 spec's own check vector, so
    /// this pins the polynomial, the reflection, and both the init and final XOR
    /// of 0xFFFFFFFF. A table generated with the wrong polynomial fails here
    /// rather than four tasks later against a real ROM, where the only symptom
    /// would be "every CRC mismatches" — indistinguishable from a bad dump.
    #[test]
    fn matches_the_standard_check_vectors() {
        assert_eq!(of(b""), 0x0000_0000);
        assert_eq!(of(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            of(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn a_single_flipped_bit_changes_the_result() {
        let a = of(&[0x00; 64]);
        let mut b = [0u8; 64];
        b[63] = 0x01;
        assert_ne!(a, of(&b));
    }
}
