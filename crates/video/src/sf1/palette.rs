//! The palette: 1,024 flat entries of 4-4-4, and no brightness.
//!
//! `PALETTE(config, m_palette).set_format(palette_device::xRGB_444, 1024)`
//! (`sf.cpp:775`). `emupal.h:213` declares `xRGB_444` as `xxxxRRRRGGGGBBBB`;
//! `emupal.cpp:171-175` resolves it to `standard_rgb_decoder<4,4,4, 8,4,0>`;
//! that decoder (`emupal.h:130-136`) is `palexpand<RedBits>(raw >> RedShift)`
//! per channel; and `palexpand<4>` (`palette.h:236`) is
//! `bits &= 0xf; return (bits << 4) | bits;`.
//!
//! # Why this is not [`crate::palette`]
//!
//! CPS-1's converter reads bits 12-15 as a brightness field and scales each
//! channel by `bright / 0x2d`. SF1's does not read them at all. The two produce
//! the same numbers only when CPS-1's brightness is at maximum, so sharing one
//! function would make SF1's colours depend on a CPS-1 default staying where it
//! is — a coupling no test on either side would catch. `crate::palette`'s
//! `PAGES`, `PAGE_ENTRIES`, `PENS` and `BACKGROUND_PEN` do not transfer either:
//! SF1 has one flat bank of 1,024 written directly by the 68000, no pages, no
//! gated copy out of gfxram, and no background pen — a disabled background
//! plane fills with pen **0** (`sf.cpp:456`).

/// Palette entries — `sf.cpp:775`.
///
/// Also palette RAM's word count: 0xb00000-0xb007ff is 0x800 bytes = 0x400
/// words = 1,024. The 68000 writes entries directly through
/// `palette_device::write16` (`emupal.cpp:405-409`), which is a store plus a
/// recalculation, so there is no separate copy step to model.
pub const ENTRIES: usize = 1024;

/// `palexpand<4>` (`palette.h:236`): a nibble scaled to a byte.
///
/// The mask is MAME's and it is load-bearing here: the caller shifts the entry
/// without masking, so the red channel arrives with bits 12-15 still above it.
#[must_use]
pub const fn expand4(nibble: u8) -> u8 {
    let n = nibble & 0x0F;
    (n << 4) | n
}

/// A palette entry's colour: `xxxxRRRRGGGGBBBB`, red first.
///
/// No brightness term — see the module documentation.
#[must_use]
pub const fn entry_to_rgb(entry: u16) -> [u8; 3] {
    [
        expand4((entry >> 8) as u8),
        expand4((entry >> 4) as u8),
        expand4(entry as u8),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1,024 entries — `sf.cpp:775`, `set_format(palette_device::xRGB_444, 1024)`.
    ///
    /// Also the word count of palette RAM: 0xb00000-0xb007ff is 0x800 bytes,
    /// which is 0x400 = 1,024 words. The two numbers agreeing is the check that
    /// the region and the palette describe the same thing.
    #[test]
    fn sf1_has_one_thousand_and_twenty_four_flat_entries() {
        assert_eq!(ENTRIES, 1024);
        assert_eq!(0x800 / 2, 1024, "palette RAM is 0x800 bytes of words");
    }

    /// `palexpand<4>` scales a nibble by 0x11, and nothing else.
    #[test]
    fn expand4_replicates_the_nibble_into_both_halves() {
        assert_eq!(expand4(0x0), 0x00);
        assert_eq!(expand4(0x1), 0x11);
        assert_eq!(expand4(0x7), 0x77);
        assert_eq!(expand4(0xF), 0xFF);
        // `palette.h:236` masks first, so a byte above a nibble does not leak.
        assert_eq!(expand4(0x35), 0x55, "the high nibble is masked off");
    }

    /// The whole conversion, hand-computed, including the asymmetric case.
    ///
    /// `emupal.h:213` declares `xRGB_444` as `xxxxRRRRGGGGBBBB`, so red is bits
    /// 8-11, green 4-7, blue 0-3, and bits 12-15 are **ignored**.
    #[test]
    fn an_entry_converts_to_rgb_with_no_brightness_term() {
        assert_eq!(entry_to_rgb(0x0000), [0x00, 0x00, 0x00]);
        assert_eq!(entry_to_rgb(0x0FFF), [0xFF, 0xFF, 0xFF]);
        assert_eq!(entry_to_rgb(0x0F00), [0xFF, 0x00, 0x00], "red is bits 8-11");
        assert_eq!(
            entry_to_rgb(0x00F0),
            [0x00, 0xFF, 0x00],
            "green is bits 4-7"
        );
        assert_eq!(entry_to_rgb(0x000F), [0x00, 0x00, 0xFF], "blue is bits 0-3");
        // 1 -> 0x11 = 17, 3 -> 0x33 = 51, 5 -> 0x55 = 85. Asymmetric on purpose:
        // a channel swap would pass every symmetric case above.
        assert_eq!(entry_to_rgb(0x0135), [17, 51, 85]);
    }

    /// The top four bits are not a brightness field on this board.
    ///
    /// This is the assertion that forbids reusing `video::palette::entry_to_rgb`.
    /// That function reads bits 12-15 as CPS-1's brightness and multiplies by
    /// `bright / 0x2d`; SF1's `standard_rgb_decoder<4,4,4, 8,4,0>` never looks at
    /// them. The two agree only where CPS-1's brightness happens to be maximum,
    /// so sharing one function would make SF1 correct by coincidence.
    #[test]
    fn the_top_nibble_is_ignored_and_this_is_not_cps1s_converter() {
        for high in 0u16..=0xF {
            assert_eq!(
                entry_to_rgb((high << 12) | 0x0FFF),
                [0xFF, 0xFF, 0xFF],
                "high nibble {high:#x} must not change the colour"
            );
        }
        // CPS-1's converter, on the same entry, gives a third of full scale.
        assert_eq!(crate::palette::entry_to_rgb(0x0FFF), [0x55, 0x55, 0x55]);
        assert_ne!(entry_to_rgb(0x0FFF), crate::palette::entry_to_rgb(0x0FFF));
    }
}
