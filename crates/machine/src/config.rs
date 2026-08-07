//! Per-game board configuration.
//!
//! CPS-B is not RAM: it answers some reads with values the board wires in rather
//! than what was written. MAME keeps these in a per-game table
//! (`cps1_v.cpp:1766-1900`); this is the same table with one row.

/// The CPS-B behaviours a game's board exhibits.
///
/// Offsets are **byte offsets from 0x800140**, matching MAME's table. The `/2` to
/// a word index is written at the point of use, never carried in the field —
/// mixing the two shifts the register file by one entry, and every value in it
/// looks plausible in the wrong slot.
#[derive(Debug, Clone, Copy)]
pub struct BoardConfig {
    /// Byte offset of the CPSB ID register, or `None` if the board has none.
    pub cpsb_addr: Option<u8>,
    /// The value that register reads back as, regardless of what was written.
    pub cpsb_value: u16,
    /// Byte offset of the extra-input port (`IN2`), or `None`.
    pub in2_addr: Option<u8>,
    /// Which CPS-B registers the video subsystem reads on this board.
    ///
    /// The same MAME table row supplies this and the fields above — `{"sf2",
    /// CPS_B_11, mapper_STF29, 0x36}` (`cps1_v.cpp:1838`) names the CPS-B variant
    /// once, and both halves of this struct come out of it. Keeping them together
    /// means a board cannot be configured with one game's registers and another's
    /// wired reads.
    pub video: video::regs::VideoConfig,
    /// How this board's graphics codes map onto its ROM banks.
    pub mapper: video::bank::BankMapper,
}

impl BoardConfig {
    /// Street Fighter II: The World Warrior, MAME set `sf2`.
    ///
    /// `cps1_v.cpp:1838` — `{"sf2", CPS_B_11, mapper_STF29, 0x36}` — and
    /// `cps1_v.cpp:491`, where `CPS_B_11` expands to `cpsb_addr 0x32`,
    /// `cpsb_value 0x0401`, with multiply protection `__not_applicable__`.
    ///
    /// The trailing `0x36` is `in2_addr`: **SF2's three kick buttons per player
    /// are read through the CPS-B space at 0x800176**, not through the 0x800000
    /// port block (`cps1_v.cpp:2155`). Both facts are boot-critical — the game
    /// reads 0x800172 and expects 0x0401, and a board that treats CPS-B as plain
    /// RAM returns the last value written and stops at a self-test failure.
    pub const fn sf2() -> Self {
        Self {
            cpsb_addr: Some(0x32),
            cpsb_value: 0x0401,
            in2_addr: Some(0x36),
            video: video::regs::VideoConfig::sf2(),
            mapper: video::bank::BankMapper::stf29(),
        }
    }

    /// A board with no wired registers and no extra input port.
    ///
    /// Exists so a test can show that the wired-read behaviour comes from the
    /// config and not from the address: with this config 0x800172 is plain RAM.
    /// Without such a case, a hardcoded `0x32` would pass every `sf2()` test.
    ///
    /// The video half is still SF2's. There is no second CPS-B variant in this
    /// workspace yet, and inventing a fake one here would put a register layout no
    /// board ever had in front of the renderer; what this config exists to vary is
    /// the wired reads above.
    pub const fn plain() -> Self {
        Self {
            cpsb_addr: None,
            cpsb_value: 0,
            in2_addr: None,
            video: video::regs::VideoConfig::sf2(),
            mapper: video::bank::BankMapper::stf29(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three values transcribed from MAME, as literals.
    #[test]
    fn sf2_matches_the_mame_table_row() {
        let c = BoardConfig::sf2();
        assert_eq!(c.cpsb_addr, Some(0x32), "CPS_B_11, cps1_v.cpp:491");
        assert_eq!(c.cpsb_value, 0x0401, "what the boot self-test expects");
        assert_eq!(c.in2_addr, Some(0x36), "cps1_v.cpp:1838, the kick buttons");
    }

    /// The video half of the row, also as literals.
    ///
    /// Written out rather than compared against `VideoConfig::sf2()`, which would
    /// be the same call this field already makes and so could not fail. Every test
    /// that renders through this config indexes `cps_b` *through* these fields, so
    /// a blanked video half moves the expectation with it and every one of them
    /// still passes; this is the only place that says what the indices are.
    ///
    /// The byte offsets are `CPS_B_11` at `cps1_v.cpp:491`, halved to word indices.
    #[test]
    fn the_video_half_of_the_row_has_sf2s_register_indices() {
        let v = BoardConfig::sf2().video;
        assert_eq!(v.layer_control, 0x26 / 2);
        assert_eq!(
            v.priority,
            [
                Some(0x28 / 2),
                Some(0x2A / 2),
                Some(0x2C / 2),
                Some(0x2E / 2)
            ],
        );
        assert_eq!(v.palette_control, 0x30 / 2);
        assert_eq!(v.layer_enable_mask, [0x08, 0x10, 0x20]);

        // And the mapper, by the one fact that is visible without re-deriving its
        // range table: three banks of 0x8000 8×8 units, and no fourth.
        assert_eq!(
            BoardConfig::sf2().mapper.bank_sizes,
            [0x8000, 0x8000, 0x8000, 0]
        );
        assert!(!BoardConfig::sf2().mapper.ranges.is_empty());
    }

    /// The offsets are byte offsets, so both are even and both land inside the
    /// 0x40-byte CPS-B window.
    #[test]
    fn the_offsets_are_even_byte_offsets_inside_the_cps_b_window() {
        let c = BoardConfig::sf2();
        for (name, off) in [("cpsb_addr", c.cpsb_addr), ("in2_addr", c.in2_addr)] {
            let off = off.expect("sf2 has both");
            assert_eq!(off % 2, 0, "{name} must be a word-aligned byte offset");
            assert!(off < 0x40, "{name} must be inside 0x800140-0x80017F");
        }
        assert_eq!(0x80_0140 + u32::from(c.cpsb_addr.unwrap()), 0x80_0172);
        assert_eq!(0x80_0176, 0x80_0140 + u32::from(c.in2_addr.unwrap()));
    }
}
