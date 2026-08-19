//! SF1's ADPCM sound board: everything Z80 #2 can address.
//!
//! Map and I/O map cited to MAME `mame0261`,
//! `src/mame/capcom/sf.cpp:217-232` (`sf_state::sound2_map`,
//! `sf_state::sound2_io_map`), with the bank configured at `:739` and written at
//! `:124-127`.
//!
//! # Why this is a third board
//!
//! ```text
//! sound2_map
//! 0x0000-0x7fff  rom
//! 0x8000-0xffff  bankr "audiobank"
//! 0x0000-0xffff  nopw()          /* Yes, _no_ ram */
//!
//! sound2_io_map, map.global_mask(0xff)
//! 0x00  w msm_w<0>
//! 0x01  w msm_w<1>
//! 0x01  r soundlatch
//! 0x02  w sound2_bank_w
//! ```
//!
//! [`crate::sound::SoundBoard`] and [`crate::sf1::FmBoard`] both have RAM, both decode a
//! program map only, and both have a chip that answers reads. This board has none
//! of those properties: its whole guest-visible interface is four I/O ports and a
//! 32 KB window that moves.
//!
//! # No RAM
//!
//! The third `sound2_map` entry overlays the first two for writes only, so every
//! write anywhere is discarded. A Z80 with no RAM has no usable stack — `push`,
//! `call` and `rst` write to nothing and `pop`/`ret` read back ROM. sfemu models
//! the discard rather than asserting on it: the guest is a sample player that
//! never needs a stack, and a panic here would turn a working board into a crash.
//! [`Adpcm2Trace::writes_discarded`] counts them, so "this ROM is using a stack"
//! is a question the overlay can answer.
//!
//! # The bank, and a documented divergence
//!
//! `machine_start` configures **256** entries of [`BANK_BYTES`] from
//! `base() + 0x8000` of a [`REGION_BYTES`] region, and `sound2_bank_w` is
//! `set_entry(data)` with the **full byte, no mask**. The region holds
//! [`BANK_WINDOWS`] windows: the first is the fixed `0x0000-0x7fff` ROM and the
//! other seven are entries 0 through [`MAX_BANK_IN_RANGE`]. Entries 7-255 point
//! past the end of the region — MAME's own configuration overruns.
//!
//! sfemu masks the offset into the region, which is a power of two, so an
//! out-of-range entry **aliases** with period [`BANK_WINDOWS`] rather than reading
//! foreign memory or panicking. ⚠️ That is a divergence from MAME's undefined
//! behaviour, not fidelity: the guest is not expected to select those banks, a
//! deterministic alias is the only defensible answer, and
//! [`Adpcm2Trace::bank_overruns`] is how a reader learns it happened.
//!
//! # Port `0x01` is both directions
//!
//! A write to MSM5205 #1 and a read of the sound latch, which [`z80::Bus`]'s split
//! `port_in`/`port_out` handles naturally. And `map.global_mask(0xff)` means only
//! the low eight bits decode.
//!
//! ⚠️ `z80::Bus::port_in`/`port_out` take a `u16`, because `in a,(c)` puts `B` on
//! A8-A15. Dropping the mask makes every `in a,(c)`/`out (c),a` with a non-zero
//! `B` miss the map — and a sample driver's inner loop is exactly that shape, so
//! the symptom is total silence from a board whose every other test passes.
//!
//! Both audio CPUs read the *same* `m_soundlatch`. This board holds its own copy
//! for [`crate::sf1::FmBoard`]'s reason, and [`Adpcm2Board::set_latch`] is the
//! only door.
//!
//! # This crate holds no ROM
//!
//! [`Adpcm2Board::new`] takes the assembled `audio2` region as a `Vec<u8>`;
//! assembling it from a user-supplied ROM set is `romset`'s job. Every test here
//! builds its region inline.

use crate::sf1::msm5205::Msm5205;
use crate::sf1::sound::UNMAPPED;

/// The `audio2` region's size: `ROM_REGION(0x40000, "audio2")` (`sf.cpp:841`).
///
/// A power of two, which is what lets [`Adpcm2Board::bank_base`] mask rather than
/// branch — asserted in `the_region_holds_eight_windows`.
pub const REGION_BYTES: usize = 0x4_0000;

/// One bank window: `configure_entries(0, 256, base() + 0x8000, 0x8000)`.
pub const BANK_BYTES: usize = 0x8000;

/// How many [`BANK_BYTES`] windows the region holds: 8.
///
/// One is the fixed `0x0000-0x7fff` ROM; the other seven are bank entries 0
/// through [`MAX_BANK_IN_RANGE`]. Also the aliasing period for an out-of-range
/// entry.
pub const BANK_WINDOWS: u8 = (REGION_BYTES / BANK_BYTES) as u8;

/// The highest bank entry that lands inside the region.
///
/// Entry 6's window ends at exactly [`REGION_BYTES`]. Entries above this alias —
/// see the module doc.
pub const MAX_BANK_IN_RANGE: u8 = BANK_WINDOWS - 2;

/// How many MSM5205s this board drives (`sf.cpp:786-795`).
pub const CHIPS: usize = 2;

/// Where the bank window starts in the address space.
const BANK_BASE_ADDR: u16 = 0x8000;

/// What this board saw. Not machine state — an instrument, like every `Trace`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Adpcm2Trace {
    /// Port writes that reached each MSM5205, indexed by chip.
    ///
    /// Per chip rather than a total: the two chips are separate sample streams and
    /// "only one of them is being fed" is the failure this number exists to show.
    pub msm_writes: [u32; CHIPS],
    /// Reads the guest made of the sound latch at port `0x01`.
    pub latch_reads: u32,
    /// Writes the guest made to the bank register at port `0x02`.
    pub bank_writes: u32,
    /// Bank selections above [`MAX_BANK_IN_RANGE`], which alias.
    ///
    /// The divergence counter from the module doc. Non-zero means the guest went
    /// somewhere MAME's configuration allows and the region does not contain, and
    /// the bytes it read afterwards were sfemu's deterministic guess.
    pub bank_overruns: u32,
    /// Bytes read from the fixed `0x0000-0x7FFF` window, **as answered**.
    pub rom_fetches: u32,
    /// Bytes read from the bank window, **as answered**.
    ///
    /// Separate from [`Adpcm2Trace::rom_fetches`] because the interesting question
    /// is whether the guest reached its sample tables at all; a single total cannot
    /// say so.
    pub bank_fetches: u32,
    /// Writes the guest made anywhere in memory, all of them discarded.
    ///
    /// There is no writable memory on this CPU. A non-zero count is normal for a
    /// stray store and a large one means the ROM is trying to use a stack — see the
    /// module doc.
    pub writes_discarded: u32,
    /// Accesses to a port with no entry in this direction, including reads of
    /// `0x00` and `0x02`.
    ///
    /// This board's entire interface is ports, so an unmapped one is a finding
    /// about the driver rather than a shrug.
    pub unmapped_ports: u32,
}

/// Z80 #2's bus: 32 KB fixed, a 32 KB bank, four ports and two MSM5205s.
pub struct Adpcm2Board {
    /// The assembled `audio2` region.
    rom: Vec<u8>,
    /// The two ADPCM chips.
    msm: [Msm5205; CHIPS],
    /// The bank entry as written, unmasked — see [`Adpcm2Board::bank_base`].
    bank: u8,
    /// This board's copy of the machine's one sound latch.
    latch: u8,
    /// What the guest has done, counted.
    trace: Adpcm2Trace,
}

impl core::fmt::Debug for Adpcm2Board {
    /// Hand-written: a derived `Debug` would print 256 KB of ROM into a panic
    /// message.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Adpcm2Board")
            .field("rom_len", &self.rom.len())
            .field("bank", &self.bank)
            .field("latch", &self.latch)
            .field("msm", &self.msm)
            .field("trace", &self.trace)
            .finish_non_exhaustive()
    }
}

impl Adpcm2Board {
    /// A board holding `audio2`, at bank 0, with two reset chips and no latch.
    #[must_use]
    pub fn new(audio2: Vec<u8>) -> Self {
        Self {
            rom: audio2,
            msm: [Msm5205::new(); CHIPS],
            bank: 0,
            latch: 0,
            trace: Adpcm2Trace::default(),
        }
    }

    /// Put back what a save state carries: the bank entry and the latch.
    ///
    /// The chips come back through [`Msm5205::restore`] and the region through the
    /// ROM set, so neither is this method's business. The entry is stored as read
    /// from the file, unmasked — an out-of-range one aliases exactly as a bus write
    /// would, because a save state is not a trusted input.
    pub fn restore(&mut self, bank: u8, latch: u8) {
        self.bank = bank;
        self.latch = latch;
    }

    /// One of the two chips, for the debugger and the mix.
    ///
    /// # Panics
    ///
    /// If `chip >= CHIPS`. Not a guest address — the caller is sfemu's own code and
    /// there are exactly two chips, so an out-of-range index is a bug here rather
    /// than something a ROM can provoke.
    #[must_use]
    pub const fn msm(&self, chip: usize) -> &Msm5205 {
        &self.msm[chip]
    }

    /// One of the two chips, mutably, for the save-state codec.
    ///
    /// # Panics
    ///
    /// If `chip >= CHIPS`, for [`Adpcm2Board::msm`]'s reason.
    pub fn msm_mut(&mut self, chip: usize) -> &mut Msm5205 {
        &mut self.msm[chip]
    }

    /// Reset both chips, and nothing else on the board.
    ///
    /// A narrow door for `Sf1::reset`: `machine_reset` (`sf.cpp:744-748`)
    /// does not touch the bank, whose entry survives a soft reset because
    /// `machine_start` set it and nothing clears it.
    pub fn reset_chips(&mut self) {
        for chip in &mut self.msm {
            chip.reset();
        }
    }

    /// One MSM5205 master clock into both chips.
    ///
    /// Both, in one call, because the scheduler has one 384 kHz tick to spend and a
    /// caller that had to remember two would eventually tick one twice — see
    /// `ticking_advances_both_chips_independently`.
    pub fn tick(&mut self) {
        for chip in &mut self.msm {
            chip.tick();
        }
    }

    /// Both chips' current output, in chip order, for `crate::sf1::mix`.
    #[must_use]
    pub const fn output(&self) -> (i16, i16) {
        (self.msm[0].output(), self.msm[1].output())
    }

    /// The bank entry as the guest wrote it, unmasked.
    #[must_use]
    pub const fn bank(&self) -> u8 {
        self.bank
    }

    /// Where the bank window starts in the region.
    ///
    /// `base() + 0x8000 + entry * 0x8000`, masked into [`REGION_BYTES`]. The mask is
    /// the divergence documented in the module doc: it turns MAME's out-of-bounds
    /// entries into a deterministic alias with period [`BANK_WINDOWS`].
    #[must_use]
    pub const fn bank_base(&self) -> usize {
        (BANK_BYTES * (self.bank as usize + 1)) & (REGION_BYTES - 1)
    }

    /// Hand over the byte the 68000 wrote to `soundcmd_w`.
    ///
    /// The only door to the latch, and `Sf1` is its only caller — which is
    /// what keeps this board's copy equal to [`crate::sf1::FmBoard`]'s.
    pub fn set_latch(&mut self, val: u8) {
        self.latch = val;
    }

    /// This board's copy of the machine's one sound latch.
    #[must_use]
    pub const fn latch(&self) -> u8 {
        self.latch
    }

    /// What the guest has done, for the debug overlay.
    #[must_use]
    pub const fn trace(&self) -> Adpcm2Trace {
        self.trace
    }

    /// Zero the counters. Not a reset: no machine state moves.
    pub fn clear_trace(&mut self) {
        self.trace = Adpcm2Trace::default();
    }

    /// Sets every counter to its maximum, for a frontend panel-width test.
    ///
    /// See [`crate::sf1::Sf1::saturate_counters_for_test`], which is the only caller
    /// and which explains why this is `pub` rather than `#[cfg(test)]`. Assigns the
    /// whole struct rather than each field, so a counter added to [`Adpcm2Trace`] later
    /// is saturated by this without anyone remembering to: a literal missing a field
    /// fails the build, which is the property that makes that true.
    pub fn saturate_trace_for_test(&mut self) {
        self.trace = Adpcm2Trace {
            msm_writes: [u32::MAX; CHIPS],
            latch_reads: u32::MAX,
            bank_writes: u32::MAX,
            bank_overruns: u32::MAX,
            rom_fetches: u32::MAX,
            bank_fetches: u32::MAX,
            writes_discarded: u32::MAX,
            unmapped_ports: u32::MAX,
        };
    }

    /// Read a byte without moving a counter.
    ///
    /// Mirrors [`z80::Bus::read`]'s map arm for arm, and
    /// `peeking_agrees_with_the_bus_and_moves_no_counter` walks all 65,536 addresses
    /// at four bank entries to hold the two together — the bank is the one thing
    /// both maps *compute* rather than look up, so a `peek` that computed it
    /// differently would agree at entry 0 and nowhere else.
    ///
    /// `&self` is the enforcement rather than a preference, and it is what lets the
    /// overlay hold `&Sf1`.
    #[must_use]
    pub fn peek_byte(&self, addr: u16) -> u8 {
        self.rom
            .get(self.region_offset(addr))
            .copied()
            .unwrap_or(UNMAPPED)
    }

    /// Where a program address lands in the region.
    ///
    /// Below [`BANK_BASE_ADDR`] the region's first window, above it the bank's.
    const fn region_offset(&self, addr: u16) -> usize {
        if addr < BANK_BASE_ADDR {
            addr as usize
        } else {
            self.bank_base() + (addr as usize - BANK_BASE_ADDR as usize)
        }
    }
}

impl z80::Bus for Adpcm2Board {
    fn read(&mut self, addr: u16) -> u8 {
        let Some(byte) = self.rom.get(self.region_offset(addr)).copied() else {
            return UNMAPPED;
        };
        if addr < BANK_BASE_ADDR {
            self.trace.rom_fetches = self.trace.rom_fetches.saturating_add(1);
        } else {
            self.trace.bank_fetches = self.trace.bank_fetches.saturating_add(1);
        }
        byte
    }

    /// Every write is discarded: `map(0x0000, 0xffff).nopw()`, `/* Yes, _no_ ram */`.
    fn write(&mut self, _addr: u16, _val: u8) {
        self.trace.writes_discarded = self.trace.writes_discarded.saturating_add(1);
    }

    fn port_in(&mut self, port: u16) -> u8 {
        // map.global_mask(0xff): only A0-A7 decode. `in a,(c)` puts B on A8-A15.
        match port & 0xFF {
            0x01 => {
                self.trace.latch_reads = self.trace.latch_reads.saturating_add(1);
                self.latch
            }
            _ => {
                self.trace.unmapped_ports = self.trace.unmapped_ports.saturating_add(1);
                UNMAPPED
            }
        }
    }

    fn port_out(&mut self, port: u16, val: u8) {
        match port & 0xFF {
            chip @ (0x00 | 0x01) => {
                let chip = chip as usize;
                self.trace.msm_writes[chip] = self.trace.msm_writes[chip].saturating_add(1);
                self.msm[chip].msm_w(val);
            }
            0x02 => {
                self.trace.bank_writes = self.trace.bank_writes.saturating_add(1);
                if val > MAX_BANK_IN_RANGE {
                    self.trace.bank_overruns = self.trace.bank_overruns.saturating_add(1);
                }
                self.bank = val;
            }
            _ => {
                self.trace.unmapped_ports = self.trace.unmapped_ports.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z80::Bus;

    /// A full 256 KB `audio2` region in which each byte names its 32 KB window.
    ///
    /// Eight distinct values, one per [`BANK_BYTES`] window, which is what makes a
    /// bank selection observable in one byte.
    fn region() -> Vec<u8> {
        (0..REGION_BYTES).map(|i| (i / BANK_BYTES) as u8).collect()
    }

    /// The region's shape, from `sf.cpp:739` and `ROM_REGION(0x40000, "audio2")`.
    #[test]
    fn the_region_holds_eight_windows() {
        assert_eq!(REGION_BYTES, 0x4_0000, "ROM_REGION(0x40000, \"audio2\")");
        assert_eq!(BANK_BYTES, 0x8000, "configure_entries(..., 0x8000)");
        assert_eq!(REGION_BYTES / BANK_BYTES, usize::from(BANK_WINDOWS));
        assert_eq!(BANK_WINDOWS, 8);
        // One window is the fixed 0x0000-0x7fff ROM; the other seven are banks 0-6.
        assert_eq!(
            MAX_BANK_IN_RANGE,
            BANK_WINDOWS - 2,
            "entry 6's window ends at exactly REGION_BYTES"
        );
        assert!(
            REGION_BYTES.is_power_of_two(),
            "the mask in bank_base needs this"
        );
    }

    /// The fixed window and the bank window, from `sf.cpp:219-220`.
    #[test]
    fn the_map_is_a_fixed_window_and_a_bank_window() {
        let mut b = Adpcm2Board::new(region());
        assert_eq!(b.read(0x0000), 0, "the fixed window is the region's first");
        assert_eq!(b.read(0x7FFF), 0);
        // Bank 0 is base() + 0x8000: the region's *second* window.
        assert_eq!(b.bank(), 0);
        assert_eq!(b.read(0x8000), 1);
        assert_eq!(b.read(0xFFFF), 1);
    }

    /// Every in-range entry selects its own window, and entry 0 is not the fixed one.
    ///
    /// `configure_entries(0, 256, base() + 0x8000, 0x8000)` — the `+ 0x8000` is the
    /// off-by-one-window that a reader adapting CPS-1's bank (which starts at its
    /// region's base) would drop.
    #[test]
    fn every_in_range_bank_selects_its_own_window() {
        let mut b = Adpcm2Board::new(region());
        for entry in 0..=MAX_BANK_IN_RANGE {
            b.port_out(0x02, entry);
            assert_eq!(b.bank(), entry);
            assert_eq!(
                b.read(0x8000),
                entry + 1,
                "entry {entry} must select window {}",
                entry + 1
            );
            assert_eq!(b.read(0xFFFF), entry + 1, "and all of it");
            assert_eq!(b.read(0x0000), 0, "the fixed window never moves");
        }
        assert_eq!(b.trace().bank_overruns, 0, "none of those was out of range");
        assert_eq!(b.trace().bank_writes, u32::from(MAX_BANK_IN_RANGE) + 1);
    }

    /// Entry 6's window ends exactly at the region's end.
    ///
    /// The boundary the mask has to get right: 0x8000 + 6 * 0x8000 + 0x8000 ==
    /// 0x40000. An off-by-one in `bank_base` shows up here as an unmapped read at
    /// 0xFFFF rather than anywhere earlier.
    #[test]
    fn the_last_in_range_window_ends_at_the_regions_end() {
        let mut b = Adpcm2Board::new(region());
        b.port_out(0x02, MAX_BANK_IN_RANGE);
        assert_eq!(b.bank_base(), REGION_BYTES - BANK_BYTES);
        assert_eq!(b.read(0xFFFF), (BANK_WINDOWS - 1), "the region's last byte");
    }

    /// An out-of-range entry aliases deterministically and is counted.
    ///
    /// MAME configures 256 entries from a region holding 8, so entries 7-255 point
    /// past the end. sfemu masks into the region: the aliasing period is
    /// [`BANK_WINDOWS`], so entry 7 lands on the region's first window — the same
    /// bytes the fixed 0x0000-0x7fff window serves — and entry 8 matches entry 0.
    ///
    /// ⚠️ This is a documented divergence from MAME's undefined behaviour, not
    /// fidelity. The counter is how a reader learns the guest went there.
    #[test]
    fn an_out_of_range_bank_aliases_deterministically_and_is_counted() {
        let mut b = Adpcm2Board::new(region());
        b.port_out(0x02, 7);
        assert_eq!(b.bank(), 7, "the entry is remembered as written");
        assert_eq!(b.bank_base(), 0, "aliased to the region's first window");
        assert_eq!(b.read(0x8000), 0);
        assert_eq!(b.trace().bank_overruns, 1);

        b.port_out(0x02, 8);
        assert_eq!(b.bank_base(), BANK_BYTES, "the same window entry 0 selects");
        assert_eq!(b.read(0x8000), 1);
        assert_eq!(b.trace().bank_overruns, 2);

        // The period is BANK_WINDOWS, all the way to the top of the byte.
        b.port_out(0x02, 255);
        assert_eq!(b.bank(), 255, "the full byte, no mask (sf.cpp:126)");
        // (255 + 1) % 8 == 0: the region's first window, same as entry 7.
        assert_eq!(b.bank_base(), 0);
        assert_eq!(b.trace().bank_overruns, 3);
    }

    /// The bank register takes the whole byte.
    ///
    /// `set_entry(data)` — no `& 0x07`, no `& 0x0f`. Masking on the way *in* would
    /// make the overrun counter unreachable and hide the divergence above.
    #[test]
    fn the_bank_register_takes_the_whole_byte() {
        let mut b = Adpcm2Board::new(region());
        for entry in [0u8, 6, 7, 0x0F, 0x80, 0xFF] {
            b.port_out(0x02, entry);
            assert_eq!(b.bank(), entry, "the register is not masked");
        }
    }

    /// There is no RAM: every write anywhere is discarded and counted.
    ///
    /// `map(0x0000, 0xffff).nopw()` overlaying both read entries — `/* Yes, _no_ ram
    /// */`. Modelled rather than asserted on: a Z80 with no stack is what this board
    /// is, and a panic would turn a working board into a crash.
    #[test]
    fn there_is_no_ram_and_every_write_is_discarded() {
        let mut b = Adpcm2Board::new(region());
        let mut n = 0u32;
        for addr in [0x0000u16, 0x4000, 0x7FFF, 0x8000, 0xC000, 0xFFFF] {
            b.write(addr, 0x5A);
            n += 1;
            assert_ne!(b.read(addr), 0x5A, "{addr:#06x} kept a written byte");
        }
        assert_eq!(b.trace().writes_discarded, n);
        // And the region itself is untouched: a second board reads the same.
        let mut fresh = Adpcm2Board::new(region());
        for addr in [0x0000u16, 0x8000, 0xFFFF] {
            assert_eq!(b.read(addr), fresh.read(addr));
        }
    }

    /// No memory write can move the bank or the latch.
    ///
    /// The bank moves only through port 0x02 and the latch only through
    /// [`Adpcm2Board::set_latch`]. A memory write that reached either would be a
    /// decode bug that `there_is_no_ram_...` cannot see, because it only checks that
    /// reads did not change.
    #[test]
    fn no_memory_write_can_move_the_bank_or_the_latch() {
        let mut b = Adpcm2Board::new(region());
        b.port_out(0x02, 3);
        b.set_latch(0x42);
        for addr in [0x0000u16, 0x0002, 0x0102, 0x8000, 0xFFFF] {
            b.write(addr, 0x05);
            assert_eq!(b.bank(), 3, "a write at {addr:#06x} moved the bank");
            assert_eq!(b.latch(), 0x42, "a write at {addr:#06x} moved the latch");
        }
    }

    /// Port 0x01 writes the second chip and reads the latch.
    ///
    /// `sf.cpp:229-230`: the same port number in both directions, which the split
    /// `port_in`/`port_out` handles naturally.
    #[test]
    fn port_one_writes_the_second_chip_and_reads_the_latch() {
        let mut b = Adpcm2Board::new(region());
        b.set_latch(0x42);
        assert_eq!(b.port_in(0x01), 0x42, "the read direction is the latch");
        b.port_out(0x01, 0x07);
        assert_eq!(b.msm(1).data(), 0x07, "the write direction is chip 1");
        assert_eq!(b.msm(0).data(), 0x00, "and not chip 0");
        assert_eq!(b.trace().latch_reads, 1);
        assert_eq!(b.trace().msm_writes, [0, 1]);
    }

    /// Port 0x00 reaches only the first chip, and reads nothing.
    #[test]
    fn port_zero_writes_the_first_chip_and_reads_unmapped() {
        let mut b = Adpcm2Board::new(region());
        b.port_out(0x00, 0x07);
        assert_eq!(b.msm(0).data(), 0x07);
        assert_eq!(b.msm(1).data(), 0x00);
        assert_eq!(b.trace().msm_writes, [1, 0]);
        // 0x00 has no read entry: only 0x01 does.
        assert_eq!(b.port_in(0x00), UNMAPPED);
        assert_eq!(b.trace().unmapped_ports, 1);
    }

    /// The I/O map decodes eight bits, because of `map.global_mask(0xff)`.
    ///
    /// ⚠️ `z80::Bus::port_in`/`port_out` take a `u16` because `in a,(c)` puts `B` on
    /// A8-A15. Without the mask, every `in a,(c)`/`out (c),a` with a non-zero `B`
    /// misses the map — and a sample driver's inner loop is exactly that shape, so
    /// the symptom would be total silence from a board whose every unit test passed.
    #[test]
    fn the_io_map_decodes_eight_bits() {
        let mut b = Adpcm2Board::new(region());
        b.set_latch(0x42);
        assert_eq!(b.port_in(0xFF01), 0x42, "0xFF01 is port 0x01");
        b.port_out(0x1200, 0x07);
        assert_eq!(b.msm(0).data(), 0x07, "0x1200 is port 0x00");
        b.port_out(0xAB02, 5);
        assert_eq!(b.bank(), 5, "0xAB02 is port 0x02");
        assert_eq!(b.trace().unmapped_ports, 0, "none of those was unmapped");
    }

    /// An unmapped port reads all ones and is counted, in either direction.
    #[test]
    fn an_unmapped_port_is_all_ones_and_counted() {
        let mut b = Adpcm2Board::new(region());
        for port in [0x03u16, 0x04, 0x80, 0xFF] {
            assert_eq!(b.port_in(port), UNMAPPED, "{port:#04x}");
            b.port_out(port, 0x5A);
        }
        assert_eq!(b.trace().unmapped_ports, 8);
        // 0x00 has no read entry either, and that is the count's whole job: an
        // unmapped access here is a driver bug rather than a shrug, because this
        // board's entire interface is ports.
        assert_eq!(b.port_in(0x00), UNMAPPED);
        assert_eq!(b.trace().unmapped_ports, 9);
    }

    /// Reading the latch does not clear it.
    ///
    /// `generic_latch_8_device::read` returns `m_latched_value` and only warns about
    /// a read-before-write (`gen_latch.cpp:74-84`). The take-once discipline is on
    /// the 68000 side, where the NMI is.
    #[test]
    fn reading_the_latch_does_not_clear_it() {
        let mut b = Adpcm2Board::new(region());
        b.set_latch(0x42);
        assert_eq!(b.port_in(0x01), 0x42);
        assert_eq!(b.port_in(0x01), 0x42, "still there");
        assert_eq!(b.latch(), 0x42);
        assert_eq!(b.trace().latch_reads, 2);
    }

    /// No bus path can change the latch.
    ///
    /// [`Adpcm2Board::set_latch`] is the only door, and `Sf1` is its only
    /// caller — which is what keeps this board's copy equal to
    /// [`crate::sf1::FmBoard`]'s. Port 0x01 in particular is a *write* to a chip, so
    /// a decode that also stored it would corrupt the shared value silently.
    #[test]
    fn no_bus_path_can_change_the_latch() {
        let mut b = Adpcm2Board::new(region());
        b.set_latch(0x42);
        for port in [0x00u16, 0x01, 0x02, 0x03, 0xFF01] {
            b.port_out(port, 0x99);
            assert_eq!(
                b.latch(),
                0x42,
                "a write to port {port:#06x} moved the latch"
            );
        }
        for addr in [0x0000u16, 0x8000, 0xFFFF] {
            b.write(addr, 0x99);
            assert_eq!(b.latch(), 0x42);
        }
    }

    /// A port write is the chip's whole `msm_w`, delay included.
    ///
    /// Asserted against a bare [`Msm5205`] driven the same way rather than against
    /// table values, so this test says "the board forwards" and Task 10's tests keep
    /// saying "the chip decodes". A hardcoded expectation here would fail for two
    /// unrelated reasons and be a worse signal for both.
    #[test]
    fn a_port_write_is_the_chips_whole_msm_w() {
        for (port, chip) in [(0x00u16, 0usize), (0x01, 1)] {
            let mut board = Adpcm2Board::new(region());
            let mut reference = Msm5205::new();
            board.port_out(port, 0x07);
            reference.msm_w(0x07);
            assert_eq!(board.msm(chip).pending(), reference.pending(), "armed");
            assert_eq!(board.msm(chip).signal(), 0, "nothing decoded yet");
            for _ in 0..reference.pending() {
                board.tick();
                reference.tick();
            }
            assert_eq!(board.msm(chip).signal(), reference.signal());
            assert_ne!(reference.signal(), 0, "the fixture nibble must be audible");
        }
    }

    /// `tick` advances both chips, and they are independent.
    ///
    /// Two chips at 384 kHz driven by one scheduler: the failure this catches is a
    /// loop that ticks `msm[0]` twice, which sounds like one chip playing at double
    /// speed and the other stuck — and no single-chip test can see it.
    #[test]
    fn ticking_advances_both_chips_independently() {
        let mut b = Adpcm2Board::new(region());
        b.port_out(0x00, 0x07); // chip 0: nibble 7
        b.port_out(0x01, 0x0F); // chip 1: nibble 15
        let pending = b.msm(0).pending();
        assert_eq!(
            b.msm(1).pending(),
            pending,
            "both armed by the same countdown"
        );
        for _ in 0..pending - 1 {
            b.tick();
        }
        assert_eq!(b.msm(0).signal(), 0, "not yet");
        assert_eq!(b.msm(1).signal(), 0);
        b.tick();
        assert_ne!(b.msm(0).signal(), 0, "chip 0 decoded");
        assert_ne!(b.msm(1).signal(), 0, "chip 1 decoded");
        assert_ne!(
            b.msm(0).signal(),
            b.msm(1).signal(),
            "different nibbles must give different signals, or one chip got both writes"
        );
    }

    /// `output` is the two chips side by side, in order.
    ///
    /// Task 14's mix takes them as `msm0, msm1`. Returning them swapped is invisible
    /// in a mono sum — and SF1's mix sends both to both speakers, so it would be
    /// invisible there too until a save state or the overlay disagreed.
    #[test]
    fn the_output_is_the_two_chips_in_order() {
        let mut b = Adpcm2Board::new(region());
        b.port_out(0x00, 0x07);
        b.port_out(0x01, 0x0F);
        for _ in 0..b.msm(0).pending() {
            b.tick();
        }
        assert_eq!(b.output(), (b.msm(0).output(), b.msm(1).output()));
        assert_ne!(
            b.output().0,
            b.output().1,
            "the fixture makes order observable"
        );
    }

    /// The fetch counters separate the fixed window from the bank.
    ///
    /// Two counters rather than one because the interesting question is whether the
    /// guest ever executed from a bank at all: a driver that never banks is a driver
    /// that never reached its sample tables, and a single total cannot say so.
    #[test]
    fn the_fetch_counters_separate_the_fixed_window_from_the_bank() {
        let mut b = Adpcm2Board::new(region());
        for addr in 0x0000u16..0x0010 {
            b.read(addr);
        }
        for addr in 0x8000u16..0x8020 {
            b.read(addr);
        }
        assert_eq!(b.trace().rom_fetches, 0x10);
        assert_eq!(b.trace().bank_fetches, 0x20);
    }

    /// A short region reads as unmapped rather than panicking, in both windows.
    ///
    /// A user-supplied ROM set may be incomplete. `bank_base` masks into the
    /// *declared* region size, so a short `Vec` leaves the top of the mask's range
    /// with nothing behind it — which must read [`UNMAPPED`], not index out of
    /// bounds.
    #[test]
    fn a_short_region_reads_as_unmapped() {
        let mut b = Adpcm2Board::new(vec![0x11, 0x22]);
        assert_eq!(b.read(0x0000), 0x11);
        assert_eq!(b.read(0x0001), 0x22);
        assert_eq!(b.read(0x0002), UNMAPPED);
        assert_eq!(b.read(0x8000), UNMAPPED, "bank 0 is past the end entirely");
        assert_eq!(b.trace().rom_fetches, 2, "only what was answered");
        assert_eq!(b.trace().bank_fetches, 0);
        let mut empty = Adpcm2Board::new(Vec::new());
        assert_eq!(empty.read(0x0000), UNMAPPED);
        assert_eq!(empty.read(0xFFFF), UNMAPPED);
    }

    /// Nothing in the 16-bit space panics, at any bank, on any region length.
    #[test]
    fn the_whole_address_space_is_safe() {
        for len in [0usize, 3, BANK_BYTES + 1, REGION_BYTES] {
            let mut b = Adpcm2Board::new(vec![0x5A; len]);
            for entry in [0u8, 6, 7, 0xFF] {
                b.port_out(0x02, entry);
                for addr in 0x0000u16..=0xFFFF {
                    let _ = b.read(addr);
                    b.write(addr, 0xA5);
                    let _ = b.peek_byte(addr);
                }
            }
        }
    }

    /// `peek_byte` agrees with the bus at every address and moves no counter.
    ///
    /// Walked at two banks, because the bank is the one thing the two maps compute
    /// rather than look up — and a `peek` that recomputed it differently would agree
    /// at entry 0 and nowhere else.
    #[test]
    fn peeking_agrees_with_the_bus_and_moves_no_counter() {
        let mut b = Adpcm2Board::new(region());
        b.set_latch(0x42);
        for entry in [0u8, 4, 7, 0xFF] {
            b.port_out(0x02, entry);
            b.clear_trace();
            let before = b.trace();
            for addr in 0x0000u16..=0xFFFF {
                let peeked = b.peek_byte(addr);
                assert_eq!(peeked, b.read(addr), "{addr:#06x} at entry {entry}");
            }
            let after = b.trace();
            assert_eq!(after.bank_writes, before.bank_writes, "peeking banked");
            // The reads above did move the fetch counters; the check that peeking
            // does not is a separate walk with no `read` in it.
            b.clear_trace();
            let quiet = b.trace();
            for addr in 0x0000u16..=0xFFFF {
                let _ = b.peek_byte(addr);
            }
            assert_eq!(b.trace(), quiet, "peeking moved a counter at entry {entry}");
        }
    }

    /// A fresh board is bank 0, an empty latch, two silent chips and no counts.
    #[test]
    fn a_fresh_board_is_at_rest() {
        let b = Adpcm2Board::new(region());
        assert_eq!(b.bank(), 0);
        assert_eq!(b.bank_base(), BANK_BYTES, "entry 0 is base() + 0x8000");
        assert_eq!(b.latch(), 0);
        assert_eq!(b.output(), (0, 0));
        assert_eq!(b.msm(0), b.msm(1), "both chips start identical");
        assert_eq!(b.trace(), Adpcm2Trace::default());
    }

    /// `reset_chips` resets both chips and leaves the board's own state alone.
    ///
    /// `machine_reset` (`sf.cpp:744-748`) does not touch the bank — MAME's bank entry
    /// survives a soft reset because `machine_start` set it and nothing clears it.
    /// So the door is narrow on purpose.
    #[test]
    fn reset_chips_leaves_the_bank_and_the_latch_alone() {
        let mut b = Adpcm2Board::new(region());
        b.port_out(0x02, 3);
        b.set_latch(0x42);
        b.port_out(0x00, 0x07);
        for _ in 0..b.msm(0).pending() {
            b.tick();
        }
        assert_ne!(b.msm(0).signal(), 0, "the premise");
        b.reset_chips();
        assert_eq!(b.output(), (0, 0), "both chips are silent");
        assert_eq!(b.msm(0).pending(), 0, "and disarmed");
        assert_eq!(b.bank(), 3, "the bank is the board's");
        assert_eq!(b.latch(), 0x42, "and so is the latch");
    }

    /// `restore` round-trips the board's own state.
    ///
    /// The chips are restored through [`Msm5205::restore`] by the save-state codec,
    /// so they are not this method's business — which is why it takes two bytes and
    /// not a chip.
    #[test]
    fn restore_round_trips_the_boards_state() {
        let mut b = Adpcm2Board::new(region());
        b.restore(5, 0x42);
        assert_eq!(b.bank(), 5);
        assert_eq!(b.bank_base(), BANK_BYTES * 6);
        assert_eq!(b.latch(), 0x42);
        assert_eq!(
            b.trace(),
            Adpcm2Trace::default(),
            "a restore is not a session"
        );
        // An out-of-range entry from a save state aliases, exactly as a bus write to
        // port 0x02 would — a save state is not a trusted input.
        b.restore(0xFF, 0);
        assert_eq!(b.bank(), 0xFF);
        // (255 + 1) % 8 == 0: the region's first window, same as entry 7.
        assert_eq!(b.bank_base(), 0);
        let _ = b.read(0x8000);
    }

    /// `clear_trace` zeroes the counters and touches nothing else.
    #[test]
    fn clearing_the_trace_moves_no_state() {
        let mut b = Adpcm2Board::new(region());
        b.port_out(0x02, 3);
        b.set_latch(0x42);
        b.port_out(0x00, 0x07);
        b.write(0x0000, 0x00);
        b.read(0x8000);
        assert_ne!(b.trace(), Adpcm2Trace::default(), "the premise");
        b.clear_trace();
        assert_eq!(b.trace(), Adpcm2Trace::default());
        assert_eq!(b.bank(), 3);
        assert_eq!(b.latch(), 0x42);
        assert_eq!(b.msm(0).data(), 0x07);
    }

    /// `Debug` names the region's length rather than printing it.
    #[test]
    fn debug_does_not_print_the_region() {
        let s = format!("{:?}", Adpcm2Board::new(region()));
        assert!(s.contains("rom_len: 262144"), "{s}");
        assert!(
            !s.contains("rom:"),
            "the region's bytes are in the output: {s}"
        );
    }
}
