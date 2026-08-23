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

    /// Street Fighter II, MAME set `sf2eb` (World 910214).
    ///
    /// `cps1_v.cpp:1840` — `{"sf2eb", CPS_B_17, mapper_STF29, 0x36}`. Same
    /// graphics mapper and same kick-button port as [`Self::sf2`]; a **different
    /// CPS-B part**, which moves every register in the row.
    ///
    /// The ID register is the boot-critical one, and this set is the reason to be
    /// precise about it rather than treating one SF2 revision as standing for all
    /// of them. `sf2eb`'s program does this at 0x0004c2:
    ///
    /// ```text
    /// move.w $800148,d0     ; the ID register — offset 0x08, not 0x32
    /// andi.w #$FC3F,d0
    /// cmpi.w #$0407,d0      ; 0x0407, not 0x0401
    /// bne $6F0              ; and on a mismatch it parks in `bra $6FC` forever
    /// ```
    ///
    /// Run under [`Self::sf2`] it takes that branch: the machine boots, services
    /// every vblank, and draws nothing at all, because the failure path is an
    /// idle loop rather than a crash.
    pub const fn sf2eb() -> Self {
        Self {
            cpsb_addr: Some(0x08),
            cpsb_value: 0x0407,
            in2_addr: Some(0x36),
            video: video::regs::VideoConfig::cps_b_17(),
            mapper: video::bank::BankMapper::stf29(),
        }
    }

    /// The CPS-B row a MAME game name selects, or `None` for a name with no row.
    ///
    /// The name→row map lives here, in the crate that owns the hardware facts,
    /// rather than in whichever caller happens to need it. Two callers need it —
    /// the frontend that builds a machine from a user's set, and the gated tests
    /// that run a real ROM — and a second copy of this table is a second thing to
    /// be wrong, with the failure mode described on [`Self::sf2eb`]: a machine that
    /// boots, runs, takes every interrupt, and draws nothing.
    ///
    /// `None` rather than a default. There is no row that is safe to fall back to:
    /// the whole point of the ID register is that a board answers one address with
    /// one value, so a guess is a guess about which game this is.
    ///
    /// A `&str` and not an enum, and no dependency on `romset`: this crate must not
    /// gain one. The name is the same string `romset::games::by_name` resolves, and
    /// `sfemu`'s tests are what hold the two tables to the same set of names.
    #[must_use]
    pub fn for_game(name: &str) -> Option<Self> {
        match name {
            "sf2" => Some(Self::sf2()),
            "sf2eb" => Some(Self::sf2eb()),
            _ => None,
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

    /// `sf2eb`'s row, from `cps1_v.cpp:1840` and `CPS_B_17` at `cps1_v.cpp:497`.
    #[test]
    fn sf2eb_matches_the_mame_table_row() {
        let c = BoardConfig::sf2eb();
        assert_eq!(c.cpsb_addr, Some(0x08), "CPS_B_17, cps1_v.cpp:497");
        assert_eq!(c.cpsb_value, 0x0407, "what sf2eb's boot check demands");
        assert_eq!(c.in2_addr, Some(0x36), "the same kick buttons as sf2");
    }

    /// The address and the value both differ from `sf2`'s, and the ID lands where
    /// the program looks for it.
    ///
    /// The pair is what matters. Either field alone taken from the wrong row still
    /// fails the guest's check — it reads one address and compares one value — so
    /// asserting them together is what pins the row rather than a field of it.
    /// The absolute address is spelled out because that is what the disassembly
    /// shows, and a byte-versus-word slip in `cpsb_addr` would otherwise read as
    /// plausible.
    #[test]
    fn sf2ebs_id_register_is_at_the_address_its_program_reads() {
        let (a, b) = (BoardConfig::sf2(), BoardConfig::sf2eb());
        assert_ne!(a.cpsb_addr, b.cpsb_addr, "a different CPS-B part");
        assert_ne!(a.cpsb_value, b.cpsb_value);
        assert_eq!(
            0x80_0140 + u32::from(b.cpsb_addr.unwrap()),
            0x80_0148,
            "the address in `move.w $800148,d0` at 0x0004c2"
        );
        // And the value survives the mask the program applies before comparing.
        assert_eq!(b.cpsb_value & 0xFC3F, 0x0407, "andi.w #$FC3F then cmpi.w");
        assert_ne!(a.cpsb_value & 0xFC3F, 0x0407, "which sf2's value does not");
    }

    /// `for_game` returns each row under its own name, and nothing under any other.
    ///
    /// The negative case is the load-bearing one: a `_ => Some(Self::sf2())` arm
    /// would satisfy every positive assertion here and silently give a future
    /// revision rev G's registers, which is exactly the bug this whole row exists
    /// to fix. So an unknown name must be `None`, and `sf1` — a real game name, on
    /// hardware that has no CPS-B at all — is the case most likely to be wrongly
    /// admitted.
    #[test]
    fn for_game_maps_each_name_to_its_own_row_and_nothing_else() {
        assert_eq!(
            BoardConfig::for_game("sf2").map(|c| c.cpsb_addr),
            Some(Some(0x32))
        );
        assert_eq!(
            BoardConfig::for_game("sf2eb").map(|c| c.cpsb_addr),
            Some(Some(0x08))
        );
        for name in ["sf1", "sf2ce", "sf3", "", "SF2"] {
            assert!(
                BoardConfig::for_game(name).is_none(),
                "`{name}` has no CPS-B row and must not be given one"
            );
        }
    }

    /// Both SF2 rows use the same graphics mapper and the same kick-button port.
    ///
    /// Stated as a test because it is the half of the difference that is *not*
    /// there: a reader who sees two configs may reasonably assume the mapper moved
    /// too, and a future third revision copied from the wrong one would be caught
    /// by a mapper assertion only if some test says what the mapper should be.
    #[test]
    fn the_two_sf2_rows_share_the_mapper_and_the_kick_button_port() {
        let (a, b) = (BoardConfig::sf2(), BoardConfig::sf2eb());
        assert_eq!(a.in2_addr, b.in2_addr, "both mapper_STF29 rows end in 0x36");
        assert_eq!(a.mapper.bank_sizes, b.mapper.bank_sizes);
        assert_eq!(a.mapper.bank_sizes, [0x8000, 0x8000, 0x8000, 0]);
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
