//! Per-game board configuration.
//!
//! CPS-B is not RAM: it answers some reads with values the board wires in rather
//! than what was written. MAME keeps these in a per-game table
//! (`cps1_v.cpp:1766-1900`); this is the same table with one row.

/// Where a board's 16×16→32 multiplier reads its factors and answers with its
/// product.
///
/// Some CPS-B parts implement a copy-protection check as arithmetic the board can
/// do and a bootleg's discrete logic cannot: the program writes two 16-bit
/// factors to two registers and reads the 32-bit product back from two others
/// (`cps1_v.cpp:2143-2152`). MAME's comment dates the feature to `3wonders`
/// (CPSB ID 08xx).
///
/// All four are **byte offsets from 0x800140**, like [`BoardConfig::cpsb_addr`].
/// The factor offsets name ordinary registers — the program's writes land in the
/// register file as they would anywhere in the window — so what makes this a
/// protection device is only that reads of `result_lo`/`result_hi` answer with
/// the product instead of what was written there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultiplyPorts {
    /// Byte offset holding the first factor.
    pub factor1: u8,
    /// Byte offset holding the second factor.
    pub factor2: u8,
    /// Byte offset that reads back the product's low word.
    pub result_lo: u8,
    /// Byte offset that reads back the product's high word.
    pub result_hi: u8,
}

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
    /// The multiply-protection ports, or `None` on a board without them.
    pub multiply: Option<MultiplyPorts>,
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
            multiply: None, // CPS_B_11: __not_applicable__
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
            multiply: None, // CPS_B_17: __not_applicable__
            video: video::regs::VideoConfig::cps_b_17(),
            mapper: video::bank::BankMapper::stf29(),
        }
    }

    /// Street Fighter II: Champion Edition, MAME set `sf2ce`.
    ///
    /// `cps1_v.cpp:1905` — `{"sf2ce", CPS_B_21_DEF, mapper_S9263B, 0x36}` — and
    /// `cps1_v.cpp:499`, where `CPS_B_21_DEF` expands to `cpsb_addr 0x32`,
    /// `cpsb_value -1`, multiply protection at `0x00,0x02,0x04,0x06`.
    ///
    /// # `cpsb_value` is `-1`, which is not "no register"
    ///
    /// `cps1_cps_b_r` still intercepts offset 0x32 and returns `uint16_t(-1)`, so
    /// the register reads 0xFFFF and `cpsb_addr` stays `Some(0x32)`. Reading `-1`
    /// as "absent" and setting `cpsb_addr: None` would make 0x800172 plain RAM —
    /// a board that answers with the last value written, which is what a bootleg
    /// does and what the check exists to catch.
    ///
    /// It happens to be unobservable on this set: CE's program contains **zero**
    /// long operands equal to 0x800172, against `sf2eb`'s five for its own ID
    /// register. So this value is carried on MAME's authority rather than on a
    /// measurement, and no test here can distinguish it from a wrong one by
    /// running the game.
    ///
    /// # What CE checks instead
    ///
    /// The multiply ports, and it uses them heavily. Eight independent sites in
    /// the program write a factor to 0x800140 and another to 0x800142, then read
    /// the product's low word from 0x800144 — for instance at 0x003070:
    ///
    /// ```text
    /// move.w #$0004,$800140
    /// move.w d0,$800142
    /// move.w $800144,d0
    /// ```
    ///
    /// It never reads 0x800146, the high word. That is why [`MultiplyPorts`] still
    /// carries `result_hi` from the table rather than only the fields this game
    /// exercises: the field is the part's, not the program's.
    pub const fn sf2ce() -> Self {
        Self {
            cpsb_addr: Some(0x32),
            cpsb_value: 0xFFFF,
            in2_addr: Some(0x36),
            multiply: Some(MultiplyPorts {
                factor1: 0x00,
                factor2: 0x02,
                result_lo: 0x04,
                result_hi: 0x06,
            }),
            video: video::regs::VideoConfig::cps_b_21_def(),
            mapper: video::bank::BankMapper::s9263b(),
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
            "sf2ce" => Some(Self::sf2ce()),
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
            multiply: None,
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
    ///
    /// ⚠️ `sf2ce`'s `cpsb_addr` is 0x32, the **same** as `sf2`'s, so that field
    /// alone does not identify its row — a mis-wired `"sf2ce" => Some(Self::sf2())`
    /// would satisfy an address-only check. `multiply` is what separates them, so
    /// it is asserted here rather than only in the row's own test.
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
        let ce = BoardConfig::for_game("sf2ce").expect("sf2ce has a row");
        assert_eq!(ce.cpsb_addr, Some(0x32));
        assert_eq!(
            ce.multiply.map(|m| m.factor1),
            Some(0x00),
            "CE's row, not sf2's, which has no multiply ports at all"
        );
        assert!(BoardConfig::for_game("sf2").unwrap().multiply.is_none());
        for name in ["sf1", "sf3", "", "SF2", "sf2CE"] {
            assert!(
                BoardConfig::for_game(name).is_none(),
                "`{name}` has no CPS-B row and must not be given one"
            );
        }
    }

    /// `sf2ce`'s row, from `cps1_v.cpp:1905` and `CPS_B_21_DEF` at `cps1_v.cpp:499`.
    ///
    /// The multiply offsets are the four leading entries of that macro:
    /// `0x32, -1, 0x00, 0x02, 0x04, 0x06, ...` — `cpsb_addr`, `cpsb_value`, then
    /// `mult_factor1`, `mult_factor2`, `mult_result_lo`, `mult_result_hi`. The field
    /// order is `CPS1config`'s, from MAME's `cps1.h`, not inferred from the values:
    /// all four are small even numbers and any permutation of them reads as
    /// plausible.
    #[test]
    fn sf2ce_matches_the_mame_table_row() {
        let c = BoardConfig::sf2ce();
        assert_eq!(c.cpsb_addr, Some(0x32), "CPS_B_21_DEF, cps1_v.cpp:499");
        assert_eq!(c.cpsb_value, 0xFFFF, "uint16_t(-1), cps1_cps_b_r");
        assert_eq!(c.in2_addr, Some(0x36), "cps1_v.cpp:1905, the kick buttons");
        assert_eq!(
            c.multiply,
            Some(MultiplyPorts {
                factor1: 0x00,
                factor2: 0x02,
                result_lo: 0x04,
                result_hi: 0x06,
            })
        );
    }

    /// CE's multiply ports sit at the addresses its program actually uses.
    ///
    /// The offsets are transcribed from a table, and the check is that they land
    /// where the disassembly reads and writes. Eight sites in the loaded program do
    /// this — the first at 0x003070:
    ///
    /// ```text
    /// move.w #$0004,$800140   ; factor1
    /// move.w d0,$800142       ; factor2
    /// move.w $800144,d0       ; result_lo
    /// ```
    ///
    /// `result_hi` at 0x800146 is never read by this game; it is asserted anyway
    /// because it is the part's register, and a `result_hi` transcribed as 0x08
    /// (`unknown1`, which is what an earlier reading of the macro made it) would
    /// intercept a register the board answers from the file.
    #[test]
    fn sf2ces_multiply_ports_are_where_its_program_reads_and_writes() {
        let m = BoardConfig::sf2ce().multiply.expect("CE has them");
        let at = |off: u8| 0x80_0140 + u32::from(off);
        assert_eq!(at(m.factor1), 0x80_0140, "move.w #$0004,$800140");
        assert_eq!(at(m.factor2), 0x80_0142, "move.w d0,$800142");
        assert_eq!(at(m.result_lo), 0x80_0144, "move.w $800144,d0");
        assert_eq!(at(m.result_hi), 0x80_0146, "never read by this game");
    }

    /// The four multiply offsets are distinct, even, and inside the CPS-B window.
    ///
    /// Distinctness is the one that matters: `factor2 == factor1` squares one
    /// register and still produces a product, and `result_lo == result_hi` gives
    /// back the low word twice. Both look like working multipliers on any test that
    /// only checks the low word of a small product — which is every test CE's own
    /// program would motivate, since it never reads the high word.
    #[test]
    fn the_multiply_offsets_are_four_distinct_even_offsets_in_the_window() {
        let m = BoardConfig::sf2ce().multiply.expect("CE has them");
        let all = [
            ("factor1", m.factor1),
            ("factor2", m.factor2),
            ("result_lo", m.result_lo),
            ("result_hi", m.result_hi),
        ];
        for (name, off) in all {
            assert_eq!(off % 2, 0, "{name} must be a word-aligned byte offset");
            assert!(off < 0x40, "{name} must be inside 0x800140-0x80017F");
        }
        for (i, (a_name, a)) in all.iter().enumerate() {
            for (b_name, b) in &all[i + 1..] {
                assert_ne!(a, b, "{a_name} and {b_name} must be different registers");
            }
        }
        // And none of them collides with the two wired reads on the same row: an
        // intercept that shared an offset with the ID register or IN2 would be
        // shadowed by whichever arm the read path tries first.
        let c = BoardConfig::sf2ce();
        for (name, off) in all {
            assert_ne!(
                Some(off),
                c.cpsb_addr,
                "{name} collides with the ID register"
            );
            assert_ne!(Some(off), c.in2_addr, "{name} collides with IN2");
        }
    }

    /// CE shares `sf2`'s ID-register address and `sf2eb`'s nothing.
    ///
    /// Worth its own assertion because the shape of the CE/`sf2` difference is the
    /// **inverse** of the `sf2`/`sf2eb` one. `sf2eb` moved the ID register and kept
    /// the video registers' bits; CE keeps the ID register's address and moves the
    /// video registers' enable bits. A row assembled by analogy with `sf2eb` —
    /// "different part, so the ID register moves" — would put `cpsb_addr` somewhere
    /// else and fail here.
    #[test]
    fn sf2ce_shares_sf2s_id_address_and_differs_in_the_video_half() {
        let (a, c) = (BoardConfig::sf2(), BoardConfig::sf2ce());
        assert_eq!(a.cpsb_addr, c.cpsb_addr, "both CPS_B_*_DEF rows use 0x32");
        assert_ne!(a.cpsb_value, c.cpsb_value, "0x0401 against 0xFFFF");
        assert_eq!(a.in2_addr, c.in2_addr, "the same kick buttons");
        // Same video register addresses, different enable bits — `video::regs`
        // owns the detail; this is the row-level fact that the halves come from
        // different macros.
        assert_eq!(a.video.layer_control, c.video.layer_control);
        assert_ne!(a.video.layer_enable_mask, c.video.layer_enable_mask);
        assert_eq!(c.video.layer_enable_mask, [0x02, 0x04, 0x08]);
        // And the same range table, from a different PAL.
        assert_eq!(a.mapper.bank_sizes, c.mapper.bank_sizes);
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
