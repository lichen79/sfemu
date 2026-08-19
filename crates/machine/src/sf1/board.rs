//! SF1's 68000 board: everything the main CPU can address.
//!
//! # Why this is not [`crate::board::Board`]
//!
//! That board's I/O arm *is* the 0x800000 CPS-A/CPS-B block, and it owns a
//! [`crate::config::BoardConfig`] describing which CPS-B reads the board answers
//! itself. SF1 has no custom chips: the palette at 0xB00000 is plain RAM, the
//! tilemap data lives in a `tilerom` the video reads directly, and the I/O block
//! at 0xC00000 is decoded by address alone. There is no `BoardConfig` for SF1 and
//! no place to put one.
//!
//! What the two share is [`Lanes`], which models the 68000's UDS/LDS pins rather
//! than anything Capcom-specific.
//!
//! # Never panics on a guest address
//!
//! Every index is produced by masking or a nonzero remainder. See
//! `no_address_in_the_whole_24_bit_space_panics` at the bottom of this file.
//!
//! Map cited to `src/mame/capcom/sf.cpp:162-183` (`sf_state::sfus_map`) at tag
//! `mame0261`.

use crate::board::Lanes;
use crate::sf1::inputs::Sf1Inputs;
use crate::trace::Trace;
use m68k::Bus;

/// Main RAM, 0xFF8000-0xFFDFFF: 24 KB = 0x3000 words (`sf.cpp:181`).
pub const RAM_WORDS: usize = 0x3000;
/// objectram, 0xFFE000-0xFFFFFF: 8 KB = 0x1000 words (`sf.cpp:182`).
pub const OBJECTRAM_WORDS: usize = 0x1000;
/// videoram, 0x800000-0x800FFF: 4 KB = 0x800 words (`sf.cpp:166`).
pub const VIDEORAM_WORDS: usize = 0x800;
/// Palette RAM, 0xB00000-0xB007FF: 2 KB = 0x400 words (`sf.cpp:167`), which is
/// the 1,024 entries `PALETTE(config, m_palette)` declares (`sf.cpp:775`).
pub const PALETTE_WORDS: usize = 0x400;

/// Program ROM space, 0x000000-0x04FFFF (`sf.cpp:165`).
///
/// 320 KB, and unlike CPS-1's 4 MB window this is the exact size of the region —
/// so a read at 0x50000 is unmapped rather than an unpopulated socket.
const ROM_BYTES: usize = 0x5_0000;

/// First byte of main RAM.
pub(crate) const RAM_BASE: u32 = 0xFF_8000;
/// First byte of objectram.
pub(crate) const OBJECTRAM_BASE: u32 = 0xFF_E000;
/// First byte of videoram.
pub(crate) const VIDEORAM_BASE: u32 = 0x80_0000;
/// First byte of palette RAM.
pub(crate) const PALETTE_BASE: u32 = 0xB0_0000;

/// What an unmapped read returns.
///
/// `map.unmap_value_high()` (`sf.cpp:164`), and the same reasoning as
/// [`crate::board`]'s: the data bus floats high, and 0x0000 would decode as a
/// legal `ori.b #imm, d0` so a runaway PC would execute quietly instead of
/// quickly taking an exception.
const UNMAPPED: u16 = 0xFFFF;

/// Where the 68000 fetches its autovectored level-1 handler address.
///
/// ⚠️ **0x64, not CPS-1's 0x68.** `set_vblank_int("screen", irq1_line_hold)`
/// (`sf.cpp:755`) raises level 1, and `sf.cpp` never calls
/// `set_interrupt_mixer(false)`, so the default mixer is on and the level is the
/// autovector index: 24 + 1 = 25, at 25 × 4 = 0x64. CPS-1 is 0x68 because it
/// wires the IPL pins individually and drives level 2.
///
/// A board watching 0x68 never sees the acknowledge, so the interrupt is never
/// released: the game runs one frame and stops.
const VEC_AUTOVECTOR_1: u32 = 0x64;

/// Everything on SF1's 68000 bus.
pub struct Sf1Board {
    /// The `maincpu` region, zero-padded to 0x50000.
    pub rom: Vec<u8>,
    /// Main RAM, as words: every access to it is word-oriented and the 68000 is
    /// big-endian, so storing words keeps the byte-order conversion in
    /// [`Bus::read8`] / [`Bus::write8`] alone.
    pub ram: Box<[u16; RAM_WORDS]>,
    /// Sprite entries. The video reads this; the board only stores it.
    ///
    /// A separate array from [`Sf1Board::ram`] and not a window into it, because
    /// the map separates them (`sf.cpp:181-182`) and the video must not see the
    /// 68000's stack as sprites.
    pub objectram: Box<[u16; OBJECTRAM_WORDS]>,
    /// The text plane's tiles, one word each.
    pub videoram: Box<[u16; VIDEORAM_WORDS]>,
    /// Palette RAM, 1,024 flat 4-4-4 entries. Readable as well as writable:
    /// `.ram().w(palette_device::write16)` (`sf.cpp:167`).
    pub palette: Box<[u16; PALETTE_WORDS]>,
    /// Controls and DIP switches. The frontend writes this between frames.
    pub inputs: Sf1Inputs,
    /// `m_active` — `gfxctrl`'s latched byte (`sf.cpp:350`). The video reads it.
    pub active: u8,
    /// `m_bgscroll` (`sf.cpp:327`).
    pub bgscroll: u16,
    /// `m_fgscroll` (`sf.cpp:333`).
    pub fgscroll: u16,
    /// Coin counters and lockouts, 0xC00011. Recorded, not acted on — MAME's
    /// `coin_w` drives `machine().bookkeeping()`, which has no analogue here.
    pub coin_ctrl: u8,
    /// What the board saw.
    ///
    /// Not cleared by [`Sf1Board::reset`] — a trace is an instrument, not machine
    /// state.
    pub trace: Trace,
    /// The pending sound command, or `None` if the sound board has taken it.
    ///
    /// See [`Sf1Board::take_sound_command`] for why this is a take-once slot
    /// rather than a register.
    sound_latch: Option<u8>,
    /// Set while IPL1 is asserted and the 68000 has not yet fetched its vector.
    ///
    /// Detected as a read of the vector-25 longword at [`VEC_AUTOVECTOR_1`], for
    /// the reason [`crate::board::Board`]'s field documents at length: [`Bus`]
    /// carries no function code, so the autovector cycle is invisible and the
    /// vector fetch is the only observable proxy. Exact on this board too — the
    /// vector table is in ROM and no game reads its own vector 25 as data.
    vblank_pending: bool,
}

impl Sf1Board {
    /// `prog` is the assembled 68000 program region, big-endian, up to 0x50000
    /// bytes. Longer input is truncated; shorter is zero-padded.
    ///
    /// Takes `&[u8]` and not a `romset::RomSet` for the same reason
    /// [`crate::board::Board::new`] does: `machine` does not depend on `romset`.
    #[must_use]
    pub fn new(prog: &[u8]) -> Self {
        let mut rom = vec![0u8; ROM_BYTES];
        let n = prog.len().min(ROM_BYTES);
        rom[..n].copy_from_slice(&prog[..n]);
        Self {
            rom,
            ram: Box::new([0u16; RAM_WORDS]),
            objectram: Box::new([0u16; OBJECTRAM_WORDS]),
            videoram: Box::new([0u16; VIDEORAM_WORDS]),
            palette: Box::new([0u16; PALETTE_WORDS]),
            inputs: Sf1Inputs::idle(),
            active: 0,
            bgscroll: 0,
            fgscroll: 0,
            coin_ctrl: 0,
            trace: Trace::default(),
            sound_latch: None,
            vblank_pending: false,
        }
    }

    /// `machine_reset`, `sf.cpp:748-753`.
    ///
    /// Zeroes `m_active`, `m_bgscroll` and `m_fgscroll` — and nothing else. RAM,
    /// videoram, objectram and the palette survive, which is what a board with no
    /// RAM-clearing reset circuit does. (`m_prot_t0`, the fourth thing MAME
    /// clears, belongs to the `sfjp` sets' i8751 and has no field here.)
    ///
    /// Beyond MAME's list, this also drops the pending interrupt and any untaken
    /// sound command: the IPL line follows RESET on hardware, and a command the
    /// sound board never saw must not arrive after the reset meant to silence it.
    /// [`Sf1Board::trace`] is deliberately *not* cleared.
    pub fn reset(&mut self) {
        self.active = 0;
        self.bgscroll = 0;
        self.fgscroll = 0;
        self.vblank_pending = false;
        self.sound_latch = None;
    }

    /// Asserts IPL1, as the beam reaching vblank does (`sf.cpp:755`).
    pub fn assert_vblank(&mut self) {
        self.vblank_pending = true;
        self.trace.vblanks += 1;
    }

    /// Whether IPL1 is still asserted — i.e. the 68000 has not yet acknowledged.
    #[must_use]
    pub fn vblank_pending(&self) -> bool {
        self.vblank_pending
    }

    /// Sets the pending-interrupt line directly, for a save-state restore.
    ///
    /// ⚠️ **Not for the scheduler.** [`Sf1Board::assert_vblank`] is what a beam
    /// reaching vblank calls, and it also counts the vblank. This counts nothing,
    /// which is right for a restore — the vblank being restored was counted when it
    /// happened — and wrong for everything else.
    pub fn set_vblank_pending(&mut self, pending: bool) {
        self.vblank_pending = pending;
    }

    /// Takes the pending sound command, if the 68000 has written one.
    ///
    /// A take-once slot rather than a register because `soundcmd_w`
    /// (`sf.cpp:118-122`) does two things: it writes the latch **and** pulses Z80
    /// #1's NMI. The NMI is per write, so the sound board must observe each command
    /// exactly once — a plain field would let a scheduler polling twice per command
    /// raise two NMIs, and one polling every other command drop one.
    ///
    /// A second write before a take overwrites the first, which is what a single
    /// 8-bit latch does. The trace counts both.
    pub fn take_sound_command(&mut self) -> Option<u8> {
        self.sound_latch.take()
    }

    /// The pending sound command without taking it, for a save state.
    ///
    /// ⚠️ **Not for the scheduler**, which must use
    /// [`Sf1Board::take_sound_command`]: the take is what makes each command
    /// raise exactly one NMI, and a scheduler reading through this door would
    /// re-raise the same NMI on every line until the next write.
    ///
    /// This exists because [`crate::Sf1::snapshot`] takes `&self` and a save that
    /// consumed the command would be a save that changed the machine.
    #[must_use]
    pub const fn sound_command(&self) -> Option<u8> {
        self.sound_latch
    }

    /// Put a pending command back, for a save-state restore.
    ///
    /// Counts nothing, for [`Sf1Board::set_vblank_pending`]'s reason: the write
    /// being restored was counted when the 68000 made it.
    pub fn set_sound_command(&mut self, cmd: Option<u8>) {
        self.sound_latch = cmd;
    }

    /// Fill the take-once slot as `soundcmd_w` does, for the scheduler's tests.
    ///
    /// The alternative is a 68000 program that writes 0xC0001D, which puts this
    /// file's address decode into `sf1::machine`'s failure surface — a scheduler test
    /// failing because a map arm moved is a test that names the wrong file.
    #[cfg(test)]
    pub(crate) fn write_sound_command_for_test(&mut self, val: u8) {
        self.sound_latch = Some(val);
        self.trace.sound_latch_writes += 1;
    }

    #[inline]
    fn ram_index(addr: u32) -> usize {
        // 0x3000 is not a power of two, so a remainder rather than a mask. `%` on a
        // usize cannot panic for a nonzero divisor. `wrapping_sub` keeps it defined
        // for a caller below the base, which the match arms never are.
        ((addr.wrapping_sub(RAM_BASE) >> 1) as usize) % RAM_WORDS
    }

    #[inline]
    fn objectram_index(addr: u32) -> usize {
        ((addr.wrapping_sub(OBJECTRAM_BASE) >> 1) as usize) & (OBJECTRAM_WORDS - 1)
    }

    #[inline]
    fn videoram_index(addr: u32) -> usize {
        ((addr.wrapping_sub(VIDEORAM_BASE) >> 1) as usize) & (VIDEORAM_WORDS - 1)
    }

    #[inline]
    fn palette_index(addr: u32) -> usize {
        ((addr.wrapping_sub(PALETTE_BASE) >> 1) as usize) & (PALETTE_WORDS - 1)
    }

    /// The 68000's autovector-25 fetch, which on this board is the acknowledge.
    #[inline]
    fn note_possible_ack(&mut self, addr: u32) {
        // `& !3` because the vector is a longword: a 16-bit bus fetches it as
        // 0x64 then 0x66, and either half is the same acknowledge cycle.
        if self.vblank_pending && (addr & !3) == VEC_AUTOVECTOR_1 {
            self.vblank_pending = false;
            self.trace.acks += 1;
        }
    }

    /// The word at `addr`, or `None` if `addr` is in no mapped range.
    ///
    /// This is the map plus the CPU's own bookkeeping: the acknowledge cycle and
    /// the trace's record of a read no chip answered. A debugger must use
    /// [`Sf1Board::peek_word`] instead.
    fn read_word(&mut self, addr: u32) -> Option<u16> {
        self.note_possible_ack(addr);
        let v = self.peek_word(addr);
        if v.is_none() {
            self.trace.unmapped_reads.record(addr);
        }
        v
    }

    /// The word at `addr`, or `None` if `addr` is in no mapped range — **with no
    /// side effects.**
    ///
    /// For a debugger or a memory panel. `read_word` acknowledges a pending
    /// interrupt and records unmapped addresses, so a panel built on it would clear
    /// the interrupt it was opened to investigate and fill the counter it displays
    /// with its own reads. `&self` is the enforcement.
    ///
    /// This holds the whole read map and `read_word` delegates to it, so the two
    /// cannot disagree.
    #[must_use]
    pub fn peek_word(&self, addr: u32) -> Option<u16> {
        match addr {
            0x00_0000..=0x04_FFFF => {
                let i = (addr & !1) as usize;
                Some(u16::from_be_bytes([self.rom[i], self.rom[i + 1]]))
            }
            0x80_0000..=0x80_0FFF => Some(self.videoram[Self::videoram_index(addr)]),
            0xB0_0000..=0xB0_07FF => Some(self.palette[Self::palette_index(addr)]),
            0xC0_0000..=0xC0_0001 => Some(self.inputs.in0()),
            0xC0_0002..=0xC0_0003 => Some(self.inputs.in1()),
            // `nopr()`, `sf.cpp:172-173` and `:176`: decoded, returns nothing in
            // particular. Distinguished from the default arm only by the trace,
            // which is the point — the boot code reads these.
            0xC0_0004..=0xC0_0007 | 0xC0_000E..=0xC0_000F => Some(UNMAPPED),
            0xC0_0008..=0xC0_0009 => Some(self.inputs.dsw[0]),
            0xC0_000A..=0xC0_000B => Some(self.inputs.dsw[1]),
            0xC0_000C..=0xC0_000D => Some(self.inputs.system()),
            0xFF_8000..=0xFF_DFFF => Some(self.ram[Self::ram_index(addr)]),
            0xFF_E000..=0xFF_FFFF => Some(self.objectram[Self::objectram_index(addr)]),
            // Everything else, including every write-only port in the 0xC00000
            // block: `.w(...)` with no `.ram()` drives nothing on a read.
            _ => None,
        }
    }

    /// Writes `lanes` of the word at `addr`; false if `addr` is in no writable
    /// range.
    ///
    /// `val` is positioned as the 68000 drives it, which is exactly MAME's
    /// `(data, mem_mask)` pair. See [`Lanes`] for why this takes lanes rather than
    /// doing a read-modify-write in [`Bus::write8`].
    fn write_lanes(&mut self, addr: u32, val: u16, lanes: Lanes) -> bool {
        match addr {
            // ROM: the write reaches no chip that latches it. Discarded and
            // reported as handled — a real board decodes this range.
            0x00_0000..=0x04_FFFF => {
                self.trace.rom_writes += 1;
                true
            }
            // `videoram_w`, `sf.cpp:319-323`: `COMBINE_DATA` then a tilemap dirty
            // mark, which has no analogue here — this renderer reads videoram each
            // frame rather than caching decoded tiles.
            0x80_0000..=0x80_0FFF => {
                let i = Self::videoram_index(addr);
                self.videoram[i] = lanes.merge(self.videoram[i], val);
                true
            }
            // `palette_device::write16`, which is `COMBINE_DATA` plus an immediate
            // recalculation. The recalculation is the video's job here.
            0xB0_0000..=0xB0_07FF => {
                let i = Self::palette_index(addr);
                self.palette[i] = lanes.merge(self.palette[i], val);
                true
            }
            // `coin_w`, `sf.cpp:109-116`, at the **odd** byte 0xC00011
            // (`sf.cpp:177`). Only the low lane reaches it; a byte write to
            // 0xC00010 is decoded and does nothing.
            0xC0_0010..=0xC0_0011 => {
                if lanes != Lanes::High {
                    self.coin_ctrl = val as u8;
                }
                true
            }
            // `fg_scroll_w`, `sf.cpp:331-335`: `COMBINE_DATA`.
            0xC0_0014..=0xC0_0015 => {
                self.fgscroll = lanes.merge(self.fgscroll, val);
                true
            }
            // `bg_scroll_w`, `sf.cpp:325-329`.
            0xC0_0018..=0xC0_0019 => {
                self.bgscroll = lanes.merge(self.bgscroll, val);
                true
            }
            // `gfxctrl_w`, `sf.cpp:337-355`. The whole handler is inside
            // `if (ACCESSING_BITS_0_7)`, so a high-lane-only write is decoded and
            // changes nothing — not a read-modify-write.
            0xC0_001A..=0xC0_001B => {
                if lanes != Lanes::High {
                    self.active = val as u8;
                }
                true
            }
            // `soundcmd_w`, `sf.cpp:118-122`, at the odd byte 0xC0001D
            // (`sf.cpp:180`).
            0xC0_001C..=0xC0_001D => {
                if lanes != Lanes::High {
                    self.sound_latch = Some(val as u8);
                    // Counted inside the lane check, unlike CPS-1's, because here
                    // the port *is* the odd byte: a high-lane write is a different
                    // address that MAME does not map at all, and counting it would
                    // report a sound command the hardware never saw.
                    self.trace.sound_latch_writes += 1;
                }
                true
            }
            0xFF_8000..=0xFF_DFFF => {
                let i = Self::ram_index(addr);
                self.ram[i] = lanes.merge(self.ram[i], val);
                true
            }
            0xFF_E000..=0xFF_FFFF => {
                let i = Self::objectram_index(addr);
                self.objectram[i] = lanes.merge(self.objectram[i], val);
                true
            }
            _ => {
                self.trace.unmapped_writes.record(addr);
                false
            }
        }
    }
}

impl Bus for Sf1Board {
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
        self.write_lanes(addr, val, Lanes::Word);
    }

    fn write8(&mut self, addr: u32, val: u8) {
        let addr = addr & 0x00FF_FFFF;
        let lanes = Lanes::of_byte(addr);
        self.write_lanes(addr & !1, lanes.place(val), lanes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A board with a program at 0 and nothing else.
    fn board() -> Sf1Board {
        Sf1Board::new(&[0x00, 0x11, 0x22, 0x33])
    }

    /// The four RAM regions are the sizes the map gives.
    #[test]
    fn the_region_sizes_are_the_maps() {
        assert_eq!(RAM_WORDS, 0x3000, "0xFF8000-0xFFDFFF, 24 KB");
        assert_eq!(OBJECTRAM_WORDS, 0x1000, "0xFFE000-0xFFFFFF, 8 KB");
        assert_eq!(VIDEORAM_WORDS, 0x800, "0x800000-0x800FFF, 4 KB");
        assert_eq!(PALETTE_WORDS, 0x400, "0xB00000-0xB007FF, 2 KB");
    }

    /// Program ROM reads big-endian from offset 0, and 0x50000 is the top.
    #[test]
    fn program_rom_reads_big_endian_and_ends_at_0x50000() {
        let mut b = board();
        assert_eq!(b.read16(0), 0x0011);
        assert_eq!(b.read16(2), 0x2233);
        assert_eq!(
            b.read16(4),
            0x0000,
            "past the supplied program, inside the region"
        );
        assert!(b.peek_word(0x4_FFFE).is_some(), "the last ROM word");
        assert!(b.peek_word(0x5_0000).is_none(), "one word past the region");
    }

    /// Main RAM and objectram are contiguous but separate.
    ///
    /// The map splits 0xFF8000-0xFFFFFF at 0xFFE000 so the video can read
    /// objectram without seeing the 68000's stack. A single 0x4000-word array with
    /// a shared index would pass every read/write test and give the video 24 KB of
    /// scratch as sprite entries.
    #[test]
    fn main_ram_and_objectram_are_separate_arrays() {
        let mut b = board();
        b.write16(0xFF_8000, 0x1234);
        b.write16(0xFF_E000, 0x5678);
        assert_eq!(b.read16(0xFF_8000), 0x1234);
        assert_eq!(b.read16(0xFF_E000), 0x5678);
        assert_eq!(b.ram[0], 0x1234);
        assert_eq!(b.objectram[0], 0x5678, "the video's view");
        assert_eq!(b.ram[0x2FFF], 0, "the last main-RAM word is untouched");
        // The boundary, from both sides.
        b.write16(0xFF_DFFE, 0xAAAA);
        b.write16(0xFF_FFFE, 0xBBBB);
        assert_eq!(b.ram[RAM_WORDS - 1], 0xAAAA);
        assert_eq!(b.objectram[OBJECTRAM_WORDS - 1], 0xBBBB);
    }

    /// Neither RAM region mirrors: the map covers each range exactly once.
    #[test]
    fn the_ram_regions_do_not_mirror() {
        let mut b = board();
        b.write16(0xFF_8000, 0x1111);
        assert_eq!(b.read16(0xFF_C000), 0x0000, "not a mirror of 0xFF8000");
        assert!(b.peek_word(0xFF_0000).is_none(), "below the RAM window");
        assert!(b.peek_word(0xFF_7FFE).is_none(), "one word below main RAM");
    }

    /// videoram and the palette are word RAM at their own addresses.
    #[test]
    fn videoram_and_palette_are_plain_word_ram() {
        let mut b = board();
        b.write16(0x80_0000, 0x1234);
        b.write16(0x80_0FFE, 0x5678);
        assert_eq!(b.read16(0x80_0000), 0x1234);
        assert_eq!(b.videoram[VIDEORAM_WORDS - 1], 0x5678);
        assert!(b.peek_word(0x80_1000).is_none(), "one word past videoram");
        b.write16(0xB0_0000, 0x0FFF);
        b.write16(0xB0_07FE, 0x0123);
        assert_eq!(b.read16(0xB0_0000), 0x0FFF);
        assert_eq!(b.palette[PALETTE_WORDS - 1], 0x0123);
        assert!(
            b.peek_word(0xB0_0800).is_none(),
            "one word past the palette"
        );
    }

    /// The palette is readable.
    ///
    /// `map(0xb00000, 0xb007ff).ram().w(palette_device::write16)` — `.ram()` first,
    /// so the range reads back. A write-only palette would break the boot self-test,
    /// which reads back what it writes.
    #[test]
    fn the_palette_reads_back() {
        let mut b = board();
        b.write16(0xB0_0010, 0x0ABC);
        assert_eq!(b.read16(0xB0_0010), 0x0ABC);
    }

    /// Each input port is at its own address, and `sfus` has no `IN2`.
    #[test]
    fn each_input_port_answers_at_its_own_address() {
        let mut b = board();
        b.inputs.coin1 = true;
        b.inputs.p1.right = true;
        b.inputs.start1 = true;
        assert_eq!(b.read16(0xC0_0000), 0xFFFE, "IN0");
        assert_eq!(b.read16(0xC0_0002), 0xFFFE, "IN1");
        assert_eq!(b.read16(0xC0_0008), 0xDFFF, "DSW1");
        assert_eq!(b.read16(0xC0_000A), 0xFFFF, "DSW2");
        assert_eq!(b.read16(0xC0_000C), 0xFF7E, "SYSTEM");
    }

    /// The two `nopr()` holes are decoded, not unmapped.
    ///
    /// `sf.cpp:172-173` and `:176`. They return 0xFFFF like an unmapped read, and
    /// the difference is the trace: a decoded read is not counted. The boot code
    /// reads them, so folding them into the default arm makes the unmapped counter
    /// tick every frame and hides a real unmapped access under the noise.
    #[test]
    fn the_nopr_holes_are_decoded_and_not_counted_as_unmapped() {
        let mut b = board();
        for addr in [0xC0_0004u32, 0xC0_0006, 0xC0_000E] {
            assert_eq!(
                b.read16(addr),
                0xFFFF,
                "{addr:#x} returns the unmapped value"
            );
        }
        assert_eq!(b.trace.unmapped_reads.total(), 0, "but none was counted");
        // A genuinely unmapped address, for contrast.
        assert_eq!(b.read16(0xC0_0010), 0xFFFF);
        assert_eq!(b.trace.unmapped_reads.total(), 1);
    }

    /// The write-only ports do not read back.
    ///
    /// `map(...).w(...)` with no `.ram()`: nothing drives the bus on a read, so it
    /// floats high and is an unmapped read. A board that let `gfxctrl` read back
    /// would let the self-test verify a register the hardware cannot verify.
    #[test]
    fn the_write_only_ports_read_as_unmapped() {
        let mut b = board();
        b.write16(0xC0_001A, 0x00FF);
        assert_eq!(b.read16(0xC0_001A), 0xFFFF);
        assert_eq!(b.trace.unmapped_reads.total(), 1);
        assert!(b.peek_word(0xC0_0014).is_none(), "fg scroll");
        assert!(b.peek_word(0xC0_0018).is_none(), "bg scroll");
        assert!(b.peek_word(0xC0_0010).is_none(), "coin");
        assert!(b.peek_word(0xC0_001C).is_none(), "sound command");
    }

    /// Both scroll registers merge byte writes and are independent.
    #[test]
    fn the_scrolls_combine_data_and_are_independent() {
        let mut b = board();
        b.write16(0xC0_0018, 0x1234);
        assert_eq!(b.bgscroll, 0x1234);
        assert_eq!(b.fgscroll, 0, "untouched");
        b.write16(0xC0_0014, 0x5678);
        assert_eq!(b.fgscroll, 0x5678);
        assert_eq!(b.bgscroll, 0x1234);
        // COMBINE_DATA: a byte write leaves the other half alone.
        b.write8(0xC0_0018, 0xAB);
        assert_eq!(b.bgscroll, 0xAB34, "high lane only");
        b.write8(0xC0_0019, 0xCD);
        assert_eq!(b.bgscroll, 0xABCD, "low lane only");
    }

    /// `gfxctrl` latches the low byte and ignores a high-lane-only write.
    ///
    /// `sf.cpp:348-355` wraps the whole handler in `if (ACCESSING_BITS_0_7)`, so a
    /// byte write to the even address 0xC0001A reaches a decoded port that does
    /// nothing at all. A read-modify-write model would latch 0x00 there instead.
    #[test]
    fn gfxctrl_takes_the_low_byte_and_drops_a_high_only_write() {
        let mut b = board();
        b.write16(0xC0_001A, 0x12AB);
        assert_eq!(b.active, 0xAB, "the low byte of a word write");
        b.write8(0xC0_001A, 0xCD);
        assert_eq!(b.active, 0xAB, "the high lane alone changes nothing");
        b.write8(0xC0_001B, 0xEF);
        assert_eq!(b.active, 0xEF, "the low lane latches");
    }

    /// The coin control is one byte at an **odd** address.
    #[test]
    fn the_coin_control_is_a_byte_at_an_odd_address() {
        let mut b = board();
        b.write8(0xC0_0011, 0x33);
        assert_eq!(b.coin_ctrl, 0x33);
        b.write8(0xC0_0010, 0x44);
        assert_eq!(b.coin_ctrl, 0x33, "the even byte is not the port");
        // A word write asserts both lanes, so its low half reaches 0xC00011.
        b.write16(0xC0_0010, 0x5566);
        assert_eq!(b.coin_ctrl, 0x66, "the low byte of the word");
    }

    /// The sound command is one byte at an odd address, and reading it clears it.
    ///
    /// `soundcmd_w` (`sf.cpp:118-122`) writes the latch **and** pulses Z80 #1's NMI.
    /// The pulse is the reason this is a take-once queue rather than a register: the
    /// sound board must see each write exactly once, and a plain field would let a
    /// scheduler that polls twice per command NMI twice.
    #[test]
    fn the_sound_command_is_taken_once() {
        let mut b = board();
        assert_eq!(b.take_sound_command(), None, "nothing written yet");
        b.write8(0xC0_001D, 0x42);
        assert_eq!(b.take_sound_command(), Some(0x42));
        assert_eq!(b.take_sound_command(), None, "taken once");
        // The even byte is not the port.
        b.write8(0xC0_001C, 0x99);
        assert_eq!(b.take_sound_command(), None);
        // A word write's low half reaches it.
        b.write16(0xC0_001C, 0x1177);
        assert_eq!(b.take_sound_command(), Some(0x77));
        // A second write before a take overwrites: one latch, not a queue.
        b.write8(0xC0_001D, 0x01);
        b.write8(0xC0_001D, 0x02);
        assert_eq!(b.take_sound_command(), Some(0x02));
        assert_eq!(b.take_sound_command(), None);
    }

    /// Every sound-command write is counted, including one that is overwritten.
    #[test]
    fn the_trace_counts_every_sound_command_write() {
        let mut b = board();
        b.write8(0xC0_001D, 0x01);
        b.write8(0xC0_001D, 0x02);
        assert_eq!(b.trace.sound_latch_writes, 2);
    }

    /// A ROM write is discarded, counted, and reported as handled.
    #[test]
    fn a_rom_write_is_discarded_and_counted() {
        let mut b = board();
        b.write16(0, 0xFFFF);
        assert_eq!(b.read16(0), 0x0011, "ROM is unchanged");
        assert_eq!(b.trace.rom_writes, 1);
        assert_eq!(b.trace.unmapped_writes.total(), 0, "decoded, not unmapped");
    }

    /// The interrupt vector is 0x64, not CPS-1's 0x68.
    ///
    /// `irq1_line_hold` (`sf.cpp:755`) with the default interrupt mixer on — `sf.cpp`
    /// never calls `set_interrupt_mixer` — gives level 1, autovector 24 + 1 = 25, at
    /// 25 × 4 = 0x64. A board watching 0x68 never sees the acknowledge, so the
    /// interrupt stays asserted forever: the game runs one frame and stops.
    #[test]
    fn the_acknowledge_is_the_vector_25_fetch_at_0x64() {
        let mut b = board();
        assert!(!b.vblank_pending());
        b.assert_vblank();
        assert!(b.vblank_pending());
        assert_eq!(b.trace.vblanks, 1);
        // CPS-1's vector is not this board's.
        b.read16(0x68);
        assert!(b.vblank_pending(), "0x68 is not the acknowledge here");
        b.read16(0x64);
        assert!(!b.vblank_pending());
        assert_eq!(b.trace.acks, 1);
    }

    /// Either half of the vector longword is the acknowledge.
    #[test]
    fn both_halves_of_the_vector_longword_acknowledge() {
        let mut b = board();
        b.assert_vblank();
        b.read16(0x66);
        assert!(!b.vblank_pending(), "the low half of the 0x64 longword");
        // And an acknowledge with nothing pending counts nothing.
        b.read16(0x64);
        assert_eq!(b.trace.acks, 1);
    }

    /// `peek_word` has no side effects — it neither acknowledges nor traces.
    #[test]
    fn peek_word_neither_acknowledges_nor_traces() {
        let mut b = board();
        b.assert_vblank();
        assert!(b.peek_word(0x64).is_some());
        assert!(b.vblank_pending(), "peek did not acknowledge");
        assert!(b.peek_word(0x00DE_ADBE).is_none());
        assert_eq!(b.trace.unmapped_reads.total(), 0, "peek did not trace");
    }

    /// `set_vblank_pending` restores the line without counting a vblank.
    #[test]
    fn set_vblank_pending_does_not_count_a_vblank() {
        let mut b = board();
        b.set_vblank_pending(true);
        assert!(b.vblank_pending());
        assert_eq!(b.trace.vblanks, 0, "a restore is not a new vblank");
    }

    /// `reset` zeroes exactly what `machine_reset` does.
    ///
    /// `sf.cpp:748-753` sets `m_active`, `m_bgscroll`, `m_fgscroll` and `m_prot_t0`
    /// to 0 — and nothing else. RAM, videoram, the palette and objectram survive a
    /// reset on this board, and the trace is an instrument rather than machine state.
    /// (`m_prot_t0` has no field here: it belongs to the i8751 of the `sfjp` sets.)
    #[test]
    fn reset_zeroes_only_what_machine_reset_does() {
        let mut b = board();
        b.active = 0xFF;
        b.bgscroll = 0x1234;
        b.fgscroll = 0x5678;
        b.ram[0] = 0xAAAA;
        b.videoram[0] = 0xBBBB;
        b.palette[0] = 0xCCCC;
        b.objectram[0] = 0xDDDD;
        b.coin_ctrl = 0x0F;
        b.write8(0xC0_001D, 0x42);
        b.assert_vblank();
        b.reset();
        assert_eq!(b.active, 0);
        assert_eq!(b.bgscroll, 0);
        assert_eq!(b.fgscroll, 0);
        assert_eq!(b.ram[0], 0xAAAA, "RAM survives");
        assert_eq!(b.videoram[0], 0xBBBB);
        assert_eq!(b.palette[0], 0xCCCC);
        assert_eq!(b.objectram[0], 0xDDDD);
        assert_eq!(b.coin_ctrl, 0x0F, "machine_reset does not touch it");
        assert_eq!(b.trace.vblanks, 1, "the trace is an instrument, not state");
        // ⚠️ The pending interrupt and the sound latch are cleared: a reset drops
        // the IPL line, and a command the sound board never saw must not arrive
        // after the reset that was supposed to silence it.
        assert!(!b.vblank_pending());
        assert_eq!(b.take_sound_command(), None);
    }

    /// Byte reads split words big-endian.
    #[test]
    fn byte_reads_are_big_endian() {
        let mut b = board();
        b.write16(0xFF_8000, 0x1234);
        assert_eq!(b.read8(0xFF_8000), 0x12, "even is the high byte");
        assert_eq!(b.read8(0xFF_8001), 0x34);
    }

    /// No address in the whole 24-bit space panics, reading or writing.
    ///
    /// A mis-emulated jump produces wild addresses as a matter of course, and an
    /// emulator that panics on one has turned a guest fault into a host crash.
    /// Stepped by 2 for the words and offset by 1 for the odd bytes, so both lanes
    /// of every word are exercised.
    #[test]
    fn no_address_in_the_whole_24_bit_space_panics() {
        let mut b = board();
        for addr in (0..=0x00FF_FFFEu32).step_by(2) {
            let _ = b.read16(addr);
            let _ = b.read8(addr);
            let _ = b.read8(addr | 1);
            b.write16(addr, 0xA5A5);
            b.write8(addr, 0x5A);
            b.write8(addr | 1, 0x5A);
        }
    }

    /// An address above 24 bits is masked, not indexed.
    ///
    /// The 68000 has 24 address pins. The core masks, and this masks again because
    /// tests and the frontend call the bus directly.
    #[test]
    fn addresses_above_24_bits_are_masked() {
        let mut b = board();
        b.write16(0xFF_8000, 0x1234);
        assert_eq!(b.read16(0xFFFF_8000), 0x1234, "the high byte is ignored");
    }
}
