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
        }
    }

    /// A board with no wired registers and no extra input port.
    ///
    /// Exists so a test can show that the wired-read behaviour comes from the
    /// config and not from the address: with this config 0x800172 is plain RAM.
    /// Without such a case, a hardcoded `0x32` would pass every `sf2()` test.
    pub const fn plain() -> Self {
        Self {
            cpsb_addr: None,
            cpsb_value: 0,
            in2_addr: None,
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
