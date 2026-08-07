//! The CPS-1 board: everything the 68000 can address.
//!
//! # Why this is a separate struct from the top-level machine
//!
//! `M68k::step_with(&dec, &mut bus)` borrows the CPU and the bus mutably at the
//! same time, so the CPU cannot live inside the thing it buses to. Splitting them
//! at the top level makes `self.cpu.step_with(&self.dec, &mut self.board)` legal
//! with no `RefCell`, no state swapping, and no `unsafe` — preserving
//! sub-project A's `forbid(unsafe_code)` posture through the board layer.
//!
//! # Never panics on a guest address
//!
//! Every index is produced by masking or a nonzero remainder, not by a
//! bounds-checked slice index on guest-supplied arithmetic. A mis-emulated jump
//! produces wild addresses as a matter of course, and an emulator that panics on
//! one has turned a guest fault into a host crash. See
//! `no_address_in_the_whole_24_bit_space_panics` at the bottom of this file.
//!
//! Map cited to MAME `master`, `src/mame/capcom/cps1.cpp:577-594`
//! (`cps_state::main_map`), read 2026-08-07.

use m68k::Bus;

/// Main RAM, 0xFF0000-0xFFFFFF: 64 KB = 32 K words (`cps1.cpp:593`).
const RAM_WORDS: usize = 0x8000;
/// gfxram, 0x900000-0x92FFFF: 192 KB = 96 K words (`cps1.cpp:592`).
const GFXRAM_WORDS: usize = 0x1_8000;
/// Program ROM space, 0x000000-0x3FFFFF (`CODE_SIZE`, `cps1.cpp:4063`).
const ROM_BYTES: usize = 0x40_0000;

/// First byte of gfxram.
const GFXRAM_BASE: u32 = 0x90_0000;

/// What an unmapped read returns.
///
/// The 68000's data bus floats high on an access no chip answers, and a board
/// with pull-up resistors reads it back as all ones. Zero would be the wrong
/// choice and a dangerous one: `0x0000` decodes as a legal `ori.b #imm, d0`,
/// so a runaway PC in unmapped space would execute quietly instead of quickly
/// taking an exception.
const UNMAPPED: u16 = 0xFFFF;

/// Everything on the 68000's bus: program ROM, main RAM, and gfxram.
///
/// The I/O block at 0x800000-0x80017F arrives in Task 6; until then those
/// addresses are unmapped and read as 0xFFFF.
pub struct Board {
    /// The assembled `maincpu` region, zero-padded to the full ROM space.
    pub rom: Vec<u8>,
    /// Main RAM as words: the 68000 is big-endian and every CPS-1 access to it
    /// is word-oriented, so storing words keeps the byte-order conversion in one
    /// place ([`Bus::read8`] / [`Bus::write8`]) instead of at every use.
    pub ram: Box<[u16; RAM_WORDS]>,
    /// Tilemap/sprite/palette RAM. Sub-project C reads this; the board only
    /// stores it. SF2CE executes code from here, so it is readable as well as
    /// writable (`cps1.cpp:592`).
    pub gfxram: Box<[u16; GFXRAM_WORDS]>,
}

impl Board {
    /// `prog` is the assembled 68000 program region, big-endian, up to the 4 MB
    /// of ROM space (`CODE_SIZE`). Longer input is truncated; shorter is
    /// zero-padded, which is what an unpopulated socket reads as.
    ///
    /// Takes `&[u8]` and **not** a `romset::RomSet`: `machine` does not depend on
    /// `romset`, so this crate stays at one dependency and keeps working without
    /// `std`. Every test in this crate builds its program inline.
    pub fn new(prog: &[u8]) -> Self {
        let mut rom = vec![0u8; ROM_BYTES];
        let n = prog.len().min(ROM_BYTES);
        rom[..n].copy_from_slice(&prog[..n]);
        Self {
            rom,
            ram: Box::new([0u16; RAM_WORDS]),
            gfxram: Box::new([0u16; GFXRAM_WORDS]),
        }
    }

    #[inline]
    fn ram_index(addr: u32) -> usize {
        ((addr >> 1) as usize) & (RAM_WORDS - 1)
    }

    #[inline]
    fn gfx_index(addr: u32) -> usize {
        // 0x18000 is not a power of two, so this is a remainder rather than a
        // mask. `%` on a usize cannot panic for a nonzero divisor, and
        // `wrapping_sub` keeps it defined for a caller that reaches here with an
        // address below the base — which the match arms never do, but a future
        // caller might.
        //
        // The subtraction happens to be arithmetically dead today: 0x900000 >> 1
        // is 0x480000, exactly 48 × 0x18000, so the remainder is the same with or
        // without it. Mutation confirmed no test can kill its removal. It stays
        // because it is what makes the expression mean "an offset into gfxram",
        // and because that coincidence holds only for this base and this size —
        // a later change to either would make it load-bearing with no test
        // signalling that it had become so.
        ((addr.wrapping_sub(GFXRAM_BASE) >> 1) as usize) % GFXRAM_WORDS
    }

    /// The word at `addr`, or `None` if `addr` is in no mapped range.
    ///
    /// `&mut self` because Task 6's I/O arm mutates: reading a CPS-B register
    /// clears a latch. Splitting mapped from unmapped here rather than in
    /// [`Bus::read16`] is what lets Task 9's trace name the unmapped access.
    pub(crate) fn read_word(&mut self, addr: u32) -> Option<u16> {
        match addr {
            0x00_0000..=0x3F_FFFF => {
                let i = (addr & !1) as usize;
                Some(u16::from_be_bytes([self.rom[i], self.rom[i + 1]]))
            }
            0x90_0000..=0x92_FFFF => Some(self.gfxram[Self::gfx_index(addr)]),
            0xFF_0000..=0xFF_FFFF => Some(self.ram[Self::ram_index(addr)]),
            _ => None,
        }
    }

    /// Writes the word at `addr`; false if `addr` is in no writable range.
    pub(crate) fn write_word(&mut self, addr: u32, val: u16) -> bool {
        match addr {
            // ROM: the write reaches no chip that latches it. Discarded and
            // reported as handled — guest behaviour, not our bug, and not an
            // unmapped access either. A real board decodes this range.
            0x00_0000..=0x3F_FFFF => true,
            0x90_0000..=0x92_FFFF => {
                self.gfxram[Self::gfx_index(addr)] = val;
                true
            }
            0xFF_0000..=0xFF_FFFF => {
                self.ram[Self::ram_index(addr)] = val;
                true
            }
            _ => false,
        }
    }
}

impl Bus for Board {
    fn read16(&mut self, addr: u32) -> u16 {
        // Addresses arrive already masked to 24 bits by the core, but mask again:
        // this is also called directly by tests and by the frontend.
        let addr = addr & 0x00FF_FFFF;
        self.read_word(addr).unwrap_or(UNMAPPED)
    }

    fn read8(&mut self, addr: u32) -> u8 {
        let w = self.read16(addr & !1);
        if addr & 1 == 0 {
            (w >> 8) as u8
        } else {
            w as u8
        }
    }

    fn write16(&mut self, addr: u32, val: u16) {
        let addr = addr & 0x00FF_FFFF;
        let _ = self.write_word(addr, val);
    }

    fn write8(&mut self, addr: u32, val: u8) {
        // A byte write is a read-modify-write of the containing word because the
        // storage is word-wide. On the real bus it is UDS or LDS alone; the
        // observable result is the same, and the neighbouring byte must survive.
        let base = addr & !1;
        let old = self.read16(base);
        let new = if addr & 1 == 0 {
            (u16::from(val) << 8) | (old & 0x00FF)
        } else {
            (old & 0xFF00) | u16::from(val)
        };
        self.write16(base, new);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use m68k::Bus;

    fn board() -> Board {
        Board::new(&[])
    }

    #[test]
    fn ram_stores_words_and_bytes_big_endian() {
        let mut b = board();
        b.write16(0xFF_0000, 0x1234);
        assert_eq!(b.read16(0xFF_0000), 0x1234);
        assert_eq!(b.read8(0xFF_0000), 0x12, "high byte at the even address");
        assert_eq!(b.read8(0xFF_0001), 0x34, "low byte at the odd address");
        b.write8(0xFF_0001, 0xAB);
        assert_eq!(
            b.read16(0xFF_0000),
            0x12AB,
            "a byte write must not disturb its neighbour"
        );
        b.write8(0xFF_0000, 0xCD);
        assert_eq!(
            b.read16(0xFF_0000),
            0xCDAB,
            "and the same in the other half"
        );
    }

    #[test]
    fn ram_is_64k_and_wraps_within_its_window() {
        // 0xFF0000-0xFFFFFF is 64 KB, and the 68000's address bus is 24 bits, so
        // there is nothing above it to alias into.
        let mut b = board();
        b.write16(0xFF_FFFE, 0xBEEF);
        assert_eq!(b.read16(0xFF_FFFE), 0xBEEF);
        assert_eq!(
            b.read16(0xFF_0000),
            0x0000,
            "the top word is not the bottom word"
        );
    }

    #[test]
    fn gfxram_is_192k_and_distinct_from_main_ram() {
        let mut b = board();
        b.write16(0x90_0000, 0xAAAA);
        b.write16(0x92_FFFE, 0x5555);
        assert_eq!(b.read16(0x90_0000), 0xAAAA);
        assert_eq!(b.read16(0x92_FFFE), 0x5555);
        assert_eq!(
            b.read16(0xFF_0000),
            0x0000,
            "gfxram must not alias main RAM"
        );
    }

    /// Every mapped gfxram address gets its own storage slot.
    ///
    /// This checks `gfx_index` directly rather than through `write16`, because a
    /// value-pattern sweep cannot do the job: 0x18000 words do not fit in the
    /// 0x10000 distinct values a `u16` holds, so *no* pattern is injective over
    /// the region, and a sweep that reuses a value can be aliased without
    /// noticing. My first attempt at this test did exactly that, and its own
    /// injectivity check is what caught it.
    ///
    /// Both surviving mutants — masking with 0xFFFF, and a `GFXRAM_WORDS` of
    /// 0x10000 — collapse two addresses onto one slot, which is precisely what a
    /// bijection test rejects.
    #[test]
    fn gfx_index_maps_the_mapped_range_one_to_one_onto_the_array() {
        let mut hit = vec![false; GFXRAM_WORDS];
        for i in 0..GFXRAM_WORDS {
            let addr = GFXRAM_BASE + (i as u32) * 2;
            let slot = Board::gfx_index(addr);
            assert!(
                !hit[slot],
                "{addr:#08x} aliases an earlier address onto slot {slot}"
            );
            hit[slot] = true;
        }
        assert!(hit.iter().all(|&h| h), "some slot is unreachable");
    }

    /// And the same property observed through the bus, at the addresses where a
    /// 64 K alias would show.
    ///
    /// `gfx_index_maps_the_mapped_range_one_to_one_onto_the_array` reads the index
    /// function; this reads the artifact. A pair 0x10000 words apart is what a
    /// 0xFFFF mask collapses, and 0x8000 words apart is what a smaller region
    /// would.
    #[test]
    fn words_that_a_wrong_mask_would_alias_stay_independent() {
        let mut b = board();
        for stride in [0x8000u32, 0x1_0000] {
            for i in [0u32, 1, 0x1234, 0x7FFF] {
                let lo = GFXRAM_BASE + i * 2;
                let hi = GFXRAM_BASE + (i + stride) * 2;
                b.write16(lo, 0xA1A1);
                b.write16(hi, 0xB2B2);
                assert_eq!(
                    b.read16(lo),
                    0xA1A1,
                    "words {i} and {} (stride {stride:#x}) share a slot",
                    i + stride
                );
                assert_eq!(b.read16(hi), 0xB2B2);
            }
        }
    }

    /// gfxram is 96 K words, pinned as a literal.
    ///
    /// `every_word_of_gfxram_is_independently_addressable` iterates
    /// `0..GFXRAM_WORDS`, so a shrunken constant would shrink the sweep with it
    /// and stay green — a self-consistent test. This is the literal that stops
    /// that, and 0x92FFFE below is the address the region's size implies.
    #[test]
    fn gfxram_holds_exactly_96k_words() {
        assert_eq!(GFXRAM_WORDS, 0x1_8000, "192 KB at cps1.cpp:592");
        let b = board();
        assert_eq!(b.gfxram.len(), 0x1_8000);
        assert_eq!(
            GFXRAM_BASE + (GFXRAM_WORDS as u32) * 2 - 2,
            0x92_FFFE,
            "the last word of the mapped range"
        );
    }

    #[test]
    fn rom_reads_the_program_and_ignores_writes() {
        let mut b = Board::new(&[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(b.read16(0x00_0000), 0x1234);
        assert_eq!(b.read16(0x00_0002), 0x5678);
        b.write16(0x00_0000, 0xFFFF);
        assert_eq!(
            b.read16(0x00_0000),
            0x1234,
            "ROM is read-only; the write is discarded"
        );
        b.write8(0x00_0001, 0xFF);
        assert_eq!(b.read16(0x00_0000), 0x1234, "and a byte write too");
    }

    #[test]
    fn rom_beyond_the_program_reads_zero_not_out_of_bounds() {
        let mut b = Board::new(&[0x12, 0x34]);
        assert_eq!(b.read16(0x3F_FFFE), 0x0000, "unpopulated ROM space");
        assert_eq!(b.read16(0x00_0002), 0x0000, "just past the program");
    }

    /// An odd ROM address reads the word containing it.
    ///
    /// Found by mutation: dropping `& !1` from the ROM arm's index survived every
    /// other test, because none of them read an odd ROM address. The consequence
    /// is not cosmetic — an unmasked index at 0x3FFFFF reads `rom[0x400000]`,
    /// one past a 4 MB `Vec`, which is a host panic on a guest address.
    #[test]
    fn an_odd_rom_address_reads_the_containing_word_and_the_last_byte_does_not_panic() {
        let mut b = Board::new(&[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(b.read16(0x00_0001), 0x1234, "not 0x3456");
        assert_eq!(b.read8(0x00_0003), 0x78);
        assert_eq!(
            b.read16(0x3F_FFFF),
            0x0000,
            "the last odd address in ROM space must not index past the Vec"
        );
    }

    #[test]
    fn a_program_longer_than_the_rom_space_is_truncated_not_a_panic() {
        // A wrong spec table, or a future set with a larger region, must not
        // panic the loader. Truncation is the documented behaviour.
        let mut prog = vec![0u8; ROM_BYTES + 4];
        prog[ROM_BYTES - 2] = 0xC0;
        prog[ROM_BYTES - 1] = 0xDE;
        let mut b = Board::new(&prog);
        assert_eq!(b.rom.len(), ROM_BYTES);
        assert_eq!(
            b.read16(0x3F_FFFE),
            0xC0DE,
            "the last in-range word survives"
        );
    }

    /// An unmapped read returns all ones, not zero.
    ///
    /// 0x0000 is a legal opcode (`ori.b #imm, d0`), so a runaway PC in unmapped
    /// space would execute quietly for thousands of instructions before anything
    /// looked wrong. 0xFFFF is illegal, which is both what the floating bus
    /// actually reads as and the failure that surfaces immediately.
    #[test]
    fn unmapped_space_reads_all_ones() {
        let mut b = board();
        assert_eq!(b.read16(0x40_0000), 0xFFFF, "just above ROM");
        assert_eq!(b.read16(0x80_0000), 0xFFFF, "the I/O block, until Task 6");
        assert_eq!(b.read16(0x93_0000), 0xFFFF, "just above gfxram");
        assert_eq!(b.read16(0xFE_FFFE), 0xFFFF, "just below main RAM");
        assert_eq!(b.read8(0x40_0000), 0xFF, "and a byte read of the same");
        assert_eq!(b.read8(0x40_0001), 0xFF);
    }

    /// `write_word` reports whether the board decoded the address.
    ///
    /// Found by mutation: flipping the unmapped arm to `true` survived the whole
    /// suite, because every test checked only that the write changed nothing —
    /// which a discarded-but-claimed-handled write also satisfies. This bool is
    /// Task 9's trace signal for "the guest wrote somewhere no chip answers", the
    /// single most useful line of output when a driver is wrong about the map, and
    /// an arm that always returns true silently empties that report.
    ///
    /// ROM is deliberately `true`: a real board decodes 0x000000-0x3FFFFF, so a
    /// write there is a guest bug worth no trace line, not an unmapped access.
    #[test]
    fn write_word_reports_unmapped_addresses_as_undecoded() {
        let mut b = board();
        assert!(b.write_word(0xFF_0000, 0x1234), "main RAM");
        assert!(b.write_word(0x90_0000, 0x1234), "gfxram");
        assert!(
            b.write_word(0x00_0000, 0x1234),
            "ROM is decoded, just read-only"
        );
        assert!(!b.write_word(0x40_0000, 0x1234), "just above ROM");
        assert!(
            !b.write_word(0x80_0000, 0x1234),
            "the I/O block, until Task 6"
        );
        assert!(!b.write_word(0x93_0000, 0x1234), "just above gfxram");
        assert!(!b.write_word(0xFE_FFFE, 0x1234), "just below main RAM");
    }

    /// And the same for reads: `read_word` returns `None` exactly where the board
    /// decodes nothing, which is what `read16` turns into 0xFFFF one layer up.
    #[test]
    fn read_word_reports_unmapped_addresses_as_none() {
        let mut b = board();
        assert!(b.read_word(0xFF_0000).is_some());
        assert!(b.read_word(0x90_0000).is_some());
        assert!(b.read_word(0x00_0000).is_some());
        assert!(b.read_word(0x40_0000).is_none());
        assert!(b.read_word(0x80_0000).is_none());
        assert!(b.read_word(0x93_0000).is_none());
        assert!(b.read_word(0xFE_FFFE).is_none());
    }

    #[test]
    fn a_write_to_unmapped_space_is_discarded_and_changes_nothing() {
        let mut b = board();
        b.write16(0xFF_0000, 0x1234);
        b.write16(0x40_0000, 0xDEAD);
        b.write8(0x93_0001, 0xBE);
        assert_eq!(b.read16(0xFF_0000), 0x1234, "RAM is untouched");
        assert_eq!(b.read16(0x90_0000), 0x0000, "gfxram is untouched");
    }

    /// Region boundaries are exact.
    ///
    /// Every off-by-one in a match arm shows up here and nowhere else: an
    /// inclusive bound written exclusive silently unmaps the last word of a
    /// region, which the CPS-1 uses (the stack lives at the top of main RAM).
    #[test]
    fn each_regions_first_and_last_word_is_mapped_and_its_neighbours_are_not() {
        let mut b = board();
        for (name, first, last) in [
            ("rom", 0x00_0000u32, 0x3F_FFFEu32),
            ("gfxram", 0x90_0000, 0x92_FFFE),
            ("ram", 0xFF_0000, 0xFF_FFFE),
        ] {
            assert!(
                b.read_word(first).is_some(),
                "{name}'s first word {first:#08x} must be mapped"
            );
            assert!(
                b.read_word(last).is_some(),
                "{name}'s last word {last:#08x} must be mapped"
            );
            if first >= 2 {
                assert!(
                    b.read_word(first - 2).is_none(),
                    "{name}'s first word has a mapped neighbour below it"
                );
            }
            // Main RAM's last word is the top of the 24-bit space; there is
            // nothing above it.
            if last < 0xFF_FFFE {
                assert!(
                    b.read_word(last + 2).is_none(),
                    "{name}'s last word has a mapped neighbour above it"
                );
            }
        }
    }

    /// A word access to an odd address is the 68000 core's business, not the
    /// board's.
    ///
    /// The core raises the address-error exception before the bus cycle happens,
    /// so `read16` here never sees an odd address from the CPU. The board still
    /// must not panic on one — the debugger and the frontend call `read16`
    /// directly — so it truncates to the containing word.
    #[test]
    fn an_odd_word_address_truncates_rather_than_panicking() {
        let mut b = board();
        b.write16(0xFF_0000, 0x1234);
        assert_eq!(b.read16(0xFF_0001), 0x1234);
        b.write16(0xFF_0001, 0x5678);
        assert_eq!(
            b.read16(0xFF_0000),
            0x5678,
            "the containing word was written"
        );
    }

    /// The invariant inherited verbatim from sub-project A: no guest address may
    /// panic. A mis-emulated jump produces exactly these accesses.
    #[test]
    fn no_address_in_the_whole_24_bit_space_panics() {
        let mut b = board();
        let mut addr = 0u32;
        while addr < 0x100_0000 {
            let _ = b.read16(addr);
            let _ = b.read8(addr);
            b.write16(addr, 0xDEAD);
            b.write8(addr, 0xBE);
            // Step by a prime so the sweep hits odd addresses and every region
            // boundary neighbourhood without taking 16M iterations.
            addr += 0x3B;
        }
    }

    /// And no address above 24 bits either.
    ///
    /// `Bus` takes a `u32`. The core masks to 24 bits, but a debugger or a
    /// frontend need not, and `rom[i + 1]` with an unmasked address would index
    /// past a 4 MB `Vec`.
    #[test]
    fn an_address_above_24_bits_is_masked_not_indexed() {
        let mut b = Board::new(&[0x12, 0x34]);
        assert_eq!(b.read16(0xFF00_0000), 0x1234, "wraps to 0x000000");
        assert_eq!(b.read16(0xFFFF_FFFE), b.read16(0xFF_FFFE), "wraps into RAM");
        b.write16(0x01FF_0000, 0x9999);
        assert_eq!(
            b.read16(0xFF_0000),
            0x9999,
            "and a write wraps the same way"
        );
    }
}
