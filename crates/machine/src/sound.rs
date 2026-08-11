//! The CPS-1 sound board: everything the Z80 can address.
//!
//! A separate struct from [`crate::board::Board`] for the same reason that one is
//! separate from [`crate::cps1::Cps1`]: `Z80::step(&mut bus)` borrows the CPU and the
//! bus at once, so the CPU cannot live inside the thing it buses to.
//!
//! # Never panics on a guest address
//!
//! Every unmapped read returns [`UNMAPPED`] and every unmapped write is dropped.
//! Banked ROM reads go through `get()` rather than an index, so a user-supplied ROM
//! set with a smaller sound ROM than SF2's reads as unmapped instead of panicking.
//! See `the_whole_address_space_is_safe`.
//!
//! # This crate holds no ROM
//!
//! [`SoundBoard::new`] takes the assembled `audiocpu` region as a `Vec<u8>`;
//! assembling it from a user-supplied ROM set is `romset`'s job. Every test here
//! builds its ROM inline.
//!
//! Map cited to MAME `master`, `src/mame/capcom/cps1.cpp:631-642`
//! (`cps_state::sub_map`), read 2026-08-07.

use ym2151::Ym2151;

/// What an unmapped read returns.
///
/// The Z80's data bus floats high on an access no chip answers, and the board's
/// pull-ups read that back as all ones. 0x00 would be the wrong choice for the same
/// reason it is on the 68000 side: `0x00` is `NOP`, so a runaway PC in unmapped space
/// would run quietly forever instead of hitting `RST 38h` (0xFF) and being noticed.
pub const UNMAPPED: u8 = 0xFF;

/// Sound RAM, 0xD000-0xD7FF: 2 KB (`cps1.cpp:635`).
///
/// Public because [`crate::MachineState`] carries a copy of it, and a save-state
/// codec has to name the array's length.
pub const RAM_BYTES: usize = 0x800;
/// First address of sound RAM.
const RAM_BASE: u16 = 0xD000;

/// The fixed ROM window, 0x0000-0x7FFF (`cps1.cpp:633`).
const FIXED_END: u16 = 0x8000;
/// The banked window's base and size, 0x8000-0xBFFF (`cps1.cpp:634`).
const BANK_BASE: u16 = 0x8000;
/// One bank is 16 KB.
const BANK_SIZE: usize = 0x4000;
/// Where the banked region starts in the `audiocpu` region.
///
/// `MACHINE_START_MEMBER(cps_state,cps1)` configures the bank as
/// `(0, 2, memregion("audiocpu")->base() + 0x10000, 0x4000)`.
const BANK_ROM_BASE: usize = 0x1_0000;
/// How many banks that configuration provides. **Two, not six** — see
/// `the_banked_window_is_two_banks_ending_at_the_rom_end`.
const BANKS: u8 = 2;

/// What the sound board saw: five counters, none of them machine state.
///
/// [`crate::Trace`]'s counterpart for the Z80's side of the board, and the same
/// argument for being separate from the machine: it records the session rather than
/// the machine, so [`SoundBoard::restore`] leaves it alone and no save state carries
/// it. Putting it in one would make two otherwise-identical machines compare unequal.
///
/// It is the whole instrument the real-ROM trace test has — `tests/sound_boot.rs`
/// asserts these counters rather than an audio hash, because the question there is
/// whether the driver executes and reaches the chip, not whether it produces one
/// particular waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SoundTrace {
    /// Writes the guest made to the YM2151, address latches and data bytes alike.
    pub ym_writes: u32,
    /// Reads the guest made of either command latch.
    pub latch_reads: u32,
    /// Bytes the guest read from the `audiocpu` region, **as answered**.
    ///
    /// Opcode fetches plus immediate operands and any table the driver reads from its
    /// own ROM: `z80::Bus::read` carries no M1 flag, so the board cannot tell them
    /// apart, and the number's job is to answer "did the Z80 execute from `audiocpu`
    /// at all". A read the ROM did not answer is **not** counted — see
    /// this board's [`z80::Bus::read`] — so a machine built with no sound region reports 0
    /// rather than a large number describing a Z80 spinning on `RST 38h`.
    pub audiocpu_fetches: u32,
    /// Writes the guest made to the OKI's address, which is a stub until D3.
    pub oki_writes: u32,
    /// I/O port accesses the guest made. This board has no ports, so a non-zero count
    /// is a finding about the driver rather than a shrug.
    pub port_accesses: u32,
}

/// Everything on the sound Z80's bus.
///
/// `Clone` and `PartialEq` for the snapshot work in Task 11. `Debug` is written by
/// hand: deriving it would print 96 KB of ROM.
///
/// ⚠️ **The derived `PartialEq` compares [`SoundBoard::trace`] too**, which is not
/// machine state. Two boards that ran the same program from different starting
/// points — a restored save state against the original — hold the same state and
/// different counters. A divergence test comparing whole boards therefore calls
/// [`SoundBoard::clear_trace`] on both first, which is visible at the call site;
/// hand-writing an eq that skipped the counters would hide the exclusion inside `==`
/// and make a future field silently excluded too.
#[derive(Clone, PartialEq, Eq)]
pub struct SoundBoard {
    /// The assembled `audiocpu` region: fixed window, then the banks from 0x10000.
    rom: Vec<u8>,
    /// Sound RAM.
    ram: [u8; RAM_BYTES],
    /// The FM chip.
    ym: Ym2151,
    /// The address latched by a write to 0xF000, consumed by the next 0xF001 write.
    ym_addr: u8,
    /// The two bytes the 68000 hands over: command, then timer/fade.
    latches: [u8; 2],
    /// The selected ROM bank, already masked to bit 0.
    bank: u8,
    /// OKI pin 7, which selects the MSM6295's sample rate divider. D3 reads it.
    oki_pin7: bool,
    /// What the guest has done, counted. Not machine state — see [`SoundTrace`].
    trace: SoundTrace,
}

impl core::fmt::Debug for SoundBoard {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SoundBoard")
            .field("rom_len", &self.rom.len())
            .field("ym_addr", &self.ym_addr)
            .field("latches", &self.latches)
            .field("bank", &self.bank)
            .field("oki_pin7", &self.oki_pin7)
            .field("trace", &self.trace)
            .finish_non_exhaustive()
    }
}

impl SoundBoard {
    /// A board holding `audiocpu`, with a reset YM2151 and zeroed RAM.
    #[must_use]
    pub fn new(audiocpu: Vec<u8>) -> Self {
        Self {
            rom: audiocpu,
            ram: [0; RAM_BYTES],
            ym: Ym2151::new(),
            ym_addr: 0,
            latches: [0; 2],
            bank: 0,
            oki_pin7: false,
            trace: SoundTrace::default(),
        }
    }

    /// The FM chip, for the scheduler to clock and the debugger to inspect.
    pub fn ym(&mut self) -> &mut Ym2151 {
        &mut self.ym
    }

    /// The FM chip without borrowing the board mutably, for a snapshot.
    ///
    /// [`SoundBoard::ym`] cannot serve: `Cps1::snapshot` takes `&self`.
    #[must_use]
    pub const fn ym_ref(&self) -> &Ym2151 {
        &self.ym
    }

    /// Sound RAM, for a snapshot.
    #[must_use]
    pub const fn ram(&self) -> &[u8; RAM_BYTES] {
        &self.ram
    }

    /// The address a write to 0xF000 latched, for a snapshot.
    ///
    /// **Part of the state, and the plan's field list left it out.** The Z80 writes
    /// the address and the data as two separate instructions, so a state taken
    /// between them — one instruction in a handful, which happens constantly in a
    /// driver that writes the chip hundreds of times a frame — restores with the
    /// wrong latched address and puts the next data byte in the wrong register.
    /// `a_save_state_round_trips_the_sound_board` in `cps1.rs` is what would catch
    /// its absence.
    #[must_use]
    pub const fn ym_addr(&self) -> u8 {
        self.ym_addr
    }

    /// Puts the state a snapshot carries back, leaving the ROM and the [`SoundTrace`]
    /// alone.
    ///
    /// The counters are deliberately absent: they record the session rather than the
    /// machine, for the same reason [`crate::Trace`] is not restored. Restoring them
    /// would make a divergence test compare the first run's counters against a copy
    /// of themselves.
    pub fn restore(
        &mut self,
        ram: &[u8; RAM_BYTES],
        bank: u8,
        oki_pin7: bool,
        ym: &Ym2151,
        ym_addr: u8,
    ) {
        self.ram = *ram;
        self.bank = bank & (BANKS - 1);
        self.oki_pin7 = oki_pin7;
        self.ym = ym.clone();
        self.ym_addr = ym_addr;
    }

    /// The 68000 hands a byte to the Z80. `which` is 0 for the command byte and 1 for
    /// the timer/fade byte; any other index is ignored.
    pub fn set_latch(&mut self, which: usize, val: u8) {
        if let Some(slot) = self.latches.get_mut(which) {
            *slot = val;
        }
    }

    /// What a latch currently holds, or 0 for an out-of-range index.
    #[must_use]
    pub fn latch(&self, which: usize) -> u8 {
        self.latches.get(which).copied().unwrap_or(0)
    }

    /// The selected ROM bank: 0 or 1.
    #[must_use]
    pub const fn bank(&self) -> u8 {
        self.bank
    }

    /// OKI pin 7, as the Z80 last set it.
    #[must_use]
    pub const fn oki_pin7(&self) -> bool {
        self.oki_pin7
    }

    /// How many writes the guest has made to the OKI's address.
    #[must_use]
    pub const fn oki_writes(&self) -> u32 {
        self.trace.oki_writes
    }

    /// How many I/O port accesses the guest has made. Expected to stay 0.
    #[must_use]
    pub const fn port_accesses(&self) -> u32 {
        self.trace.port_accesses
    }

    /// Every counter at once, which is what a debugger and the boot test read.
    #[must_use]
    pub const fn trace(&self) -> SoundTrace {
        self.trace
    }

    /// Zeroes the counters, leaving the machine alone.
    ///
    /// For a divergence test comparing two whole boards: the derived `PartialEq`
    /// covers the counters, and a restored board's counters are the restoring
    /// machine's rather than the original's. Calling this on both is how that
    /// exclusion stays visible at the call site — see [`SoundBoard`]'s own note.
    pub fn clear_trace(&mut self) {
        self.trace = SoundTrace::default();
    }

    /// The byte at `addr` as a debugger sees it — **with no side effects.**
    ///
    /// [`crate::board::Board::peek_word`]'s counterpart on this side of the machine,
    /// and here for a sharper reason than symmetry: reading through [`z80::Bus::read`]
    /// moves `audiocpu_fetches` and `latch_reads`, which are the two numbers
    /// `tests/sound_boot.rs` reads to claim the driver ran. A disassembly panel drawn
    /// once a frame would add 60 × its window to the fetch count, so the panel would
    /// manufacture the evidence for the assertion — and the number a user reads off
    /// the panel would be mostly the panel.
    ///
    /// `&self` is the enforcement rather than a preference: a `&mut self` version
    /// could bump a counter and the compiler would not object. It is also what lets
    /// the overlay hold `&Cps1`, which is that module's stated invariant —
    /// [`z80::disasm::disasm_bus`] takes `&mut B` and so cannot be used there at all.
    ///
    /// This mirrors `read`'s map arm for arm, and
    /// `peeking_agrees_with_the_bus_and_moves_no_counter` walks all 65,536 addresses
    /// to hold the two together. Two maps that can disagree is what that test exists
    /// to prevent; unlike `Board`, the counters mean the map genuinely has to be
    /// written twice.
    #[must_use]
    pub fn peek_byte(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0xBFFF => self.rom_byte(addr).unwrap_or(UNMAPPED),
            0xD000..=0xD7FF => self.ram[usize::from(addr - RAM_BASE)],
            0xF000 | 0xF001 => self.ym.read_status(),
            0xF002 => 0x00,
            0xF008 => self.latches[0],
            0xF00A => self.latches[1],
            _ => UNMAPPED,
        }
    }

    /// The ROM byte behind a program address, or `None` if the ROM is too short.
    fn rom_byte(&self, addr: u16) -> Option<u8> {
        let index = if addr < FIXED_END {
            usize::from(addr)
        } else {
            BANK_ROM_BASE + usize::from(self.bank) * BANK_SIZE + usize::from(addr - BANK_BASE)
        };
        self.rom.get(index).copied()
    }
}

impl z80::Bus for SoundBoard {
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0xBFFF => match self.rom_byte(addr) {
                // Counted only when the ROM answered. A machine built with no sound
                // region reads UNMAPPED here on every fetch, and counting those would
                // report a Z80 executing from `audiocpu` when there is no `audiocpu`
                // — which is exactly the claim `tests/sound_boot.rs` makes with the
                // number. `the_fetch_counter_counts_only_bytes_the_rom_answered` is
                // what holds the distinction.
                Some(b) => {
                    self.trace.audiocpu_fetches = self.trace.audiocpu_fetches.saturating_add(1);
                    b
                }
                None => UNMAPPED,
            },
            0xD000..=0xD7FF => self.ram[usize::from(addr - RAM_BASE)],
            // Both YM2151 addresses read the status register. The chip has one status
            // port and the board does not decode A0 for reads.
            0xF000 | 0xF001 => self.ym.read_status(),
            // The OKI's status is "not busy" until D3 implements it.
            0xF002 => 0x00,
            0xF008 | 0xF00A => {
                self.trace.latch_reads = self.trace.latch_reads.saturating_add(1);
                if addr == 0xF008 {
                    self.latches[0]
                } else {
                    self.latches[1]
                }
            }
            _ => UNMAPPED,
        }
    }

    fn write(&mut self, addr: u16, val: u8) {
        match addr {
            // ROM. A write here is what a driver bug looks like, not a crash.
            0x0000..=0xBFFF => {}
            0xD000..=0xD7FF => self.ram[usize::from(addr - RAM_BASE)] = val,
            // Both halves of a chip write are counted: the driver's cost is two
            // instructions per register and the question the count answers is whether
            // it reached the chip at all. Counting only 0xF001 would report half.
            0xF000 => {
                self.trace.ym_writes = self.trace.ym_writes.saturating_add(1);
                self.ym_addr = val;
            }
            0xF001 => {
                self.trace.ym_writes = self.trace.ym_writes.saturating_add(1);
                self.ym.write(self.ym_addr, val);
            }
            0xF002 => self.trace.oki_writes = self.trace.oki_writes.saturating_add(1),
            0xF004 => self.bank = val & (BANKS - 1),
            0xF006 => self.oki_pin7 = val & 0x01 != 0,
            _ => {}
        }
    }

    fn port_in(&mut self, _port: u16) -> u8 {
        self.trace.port_accesses = self.trace.port_accesses.saturating_add(1);
        UNMAPPED
    }

    fn port_out(&mut self, _port: u16, _val: u8) {
        self.trace.port_accesses = self.trace.port_accesses.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z80::Bus;

    /// A recognisable ROM: each byte names the 4 KB page it lives in.
    ///
    /// **`>> 12`, not the plan's `>> 8`.** A byte holding `i >> 8` truncates to 0 at
    /// index 0x10000 — exactly where the banked region starts — so the whole banked
    /// window would read 0x00 through 0x07 and the plan's own expected values (0x10,
    /// 0x13, 0x14, 0x17, which are `>> 12`) could not appear. The expectations are
    /// the bank arithmetic and are right; the helper was not.
    fn rom() -> Vec<u8> {
        let mut r = vec![0u8; 0x18000];
        for (i, b) in r.iter_mut().enumerate() {
            *b = (i >> 12) as u8;
        }
        r
    }

    /// The banked window is 2 banks, and bank 1 ends exactly at the end of the ROM.
    ///
    /// **`MACHINE_START_MEMBER(cps_state,cps1)` configures `(0, 2, base + 0x10000,
    /// 0x4000)` — two entries, not six.** The 6-entry form is QSound's, on a
    /// different board with a larger sound ROM. `audiocpu` here is 0x18000 bytes, so
    /// two 0x4000 banks from 0x10000 land exactly at 0x18000: the arithmetic
    /// confirms the 2, and a 6-bank reading would run 0x10000 bytes off the end.
    #[test]
    fn the_banked_window_is_two_banks_ending_at_the_rom_end() {
        assert_eq!(0x10000 + 2 * 0x4000, 0x18000, "two banks exactly fill it");
        let mut b = SoundBoard::new(rom());
        b.write(0xF004, 0);
        assert_eq!(b.read(0x8000), 0x10, "bank 0 starts at 0x10000");
        assert_eq!(b.read(0xBFFF), 0x13, "and runs to 0x13FFF");
        b.write(0xF004, 1);
        assert_eq!(b.read(0x8000), 0x14, "bank 1 starts at 0x14000");
        assert_eq!(
            b.read(0xBFFF),
            0x17,
            "and ends at 0x17FFF, the last ROM byte"
        );
    }

    /// Only bit 0 of the bank register selects; the rest is ignored.
    ///
    /// A core that used the whole byte would index past the ROM on the first driver
    /// write that sets a stray bit.
    #[test]
    fn only_bit_zero_of_the_bank_register_selects() {
        let mut b = SoundBoard::new(rom());
        b.write(0xF004, 0xFE);
        assert_eq!(b.bank(), 0, "even values are bank 0");
        assert_eq!(b.read(0x8000), 0x10);
        b.write(0xF004, 0xFF);
        assert_eq!(b.bank(), 1);
        assert_eq!(b.read(0x8000), 0x14);
    }

    /// The fixed window is fixed: banking does not move it.
    #[test]
    fn banking_does_not_move_the_fixed_window() {
        let mut b = SoundBoard::new(rom());
        let before: Vec<u8> = (0..0x8000u16).step_by(0x100).map(|a| b.read(a)).collect();
        b.write(0xF004, 1);
        let after: Vec<u8> = (0..0x8000u16).step_by(0x100).map(|a| b.read(a)).collect();
        assert_eq!(before, after);
    }

    /// ROM is read-only: a write into it changes nothing.
    #[test]
    fn writing_to_rom_is_ignored() {
        let mut b = SoundBoard::new(rom());
        let was = b.read(0x1234);
        b.write(0x1234, !was);
        assert_eq!(b.read(0x1234), was);
        // And in the banked window too, which a match arm covering only the fixed
        // half would let through into `rom`.
        let was = b.read(0x9000);
        b.write(0x9000, !was);
        assert_eq!(b.read(0x9000), was);
    }

    /// Sound RAM is 2 KB at 0xD000 and reads back what was written.
    #[test]
    fn sound_ram_is_two_kilobytes_at_d000() {
        let mut b = SoundBoard::new(rom());
        for a in 0xD000..0xD800u16 {
            b.write(a, (a & 0xFF) as u8);
        }
        for a in 0xD000..0xD800u16 {
            assert_eq!(b.read(a), (a & 0xFF) as u8);
        }
    }

    /// The 68000's latches are read-only to the Z80 and do not alias each other.
    ///
    /// Latch 0 is the command byte and latch 1 the timer/fade byte. A core that
    /// mapped both to the same storage would make every command also a fade value.
    #[test]
    fn the_two_latches_are_independent_and_read_only_to_the_z80() {
        let mut b = SoundBoard::new(rom());
        b.set_latch(0, 0xA5);
        b.set_latch(1, 0x5A);
        assert_eq!(b.read(0xF008), 0xA5);
        assert_eq!(b.read(0xF00A), 0x5A);
        b.write(0xF008, 0x00);
        b.write(0xF00A, 0x00);
        assert_eq!(b.read(0xF008), 0xA5, "the Z80 cannot clear the latch");
        assert_eq!(b.read(0xF00A), 0x5A);
        // And `latch()` reports the same bytes the Z80 sees, which is what the
        // debugger overlay reads.
        assert_eq!((b.latch(0), b.latch(1)), (0xA5, 0x5A));
        // An out-of-range index is ignored rather than panicking: the 68000 side
        // computes `which` from an address.
        b.set_latch(2, 0xFF);
        assert_eq!(b.latch(2), 0);
        assert_eq!((b.latch(0), b.latch(1)), (0xA5, 0x5A), "and nothing moved");
    }

    /// A YM2151 write reaches the chip; a read returns its status.
    #[test]
    fn the_ym2151_is_addressable_at_f000_and_f001() {
        let mut b = SoundBoard::new(rom());
        // Write the mode register: load and enable both timers with a short period.
        b.write(0xF000, 0x10);
        b.write(0xF001, 0xFF);
        b.write(0xF000, 0x11);
        b.write(0xF001, 0x03);
        b.write(0xF000, 0x14);
        b.write(0xF001, 0x05);
        let mut buf = [(0i16, 0i16); 64];
        b.ym().generate(&mut buf);
        assert_ne!(
            b.read(0xF001) & 0x01,
            0,
            "timer A overflowed into the status"
        );
        assert_eq!(b.read(0xF001), b.read(0xF000), "both addresses read status");
    }

    /// The latched address is what the data write uses, and it persists.
    ///
    /// The board latches 0xF000 and applies it on the next 0xF001 write, so a driver
    /// that writes two data bytes to one address hits the same register twice. A
    /// board that instead wrote `(addr, val)` straight through, or reset the latch
    /// after each data byte, would pass the test above and get real drivers wrong.
    #[test]
    fn the_ym_address_latch_persists_across_data_writes() {
        let mut b = SoundBoard::new(rom());
        // 0x08 is key-on. Two data writes to the one latched address.
        b.write(0xF000, 0x08);
        b.write(0xF001, 0x78); // all four operators of channel 0 on
        assert!(b.ym().channels[0].ops.iter().all(|op| op.keyon_live != 0));
        b.write(0xF001, 0x00); // and off again, without re-writing the address
        assert!(b.ym().channels[0].ops.iter().all(|op| op.keyon_live == 0));
    }

    /// The OKI is a counted stub, and the count is the finding.
    ///
    /// D3 implements the MSM6295. Until then a write here is recorded rather than
    /// ignored: a non-zero count after booting SF2 says the driver really does use
    /// it, which is the evidence D3's spec needs. A silent ignore would leave that
    /// unmeasured.
    #[test]
    fn the_oki_is_a_counted_stub() {
        let mut b = SoundBoard::new(rom());
        assert_eq!(b.oki_writes(), 0);
        b.write(0xF002, 0x80);
        b.write(0xF002, 0x00);
        assert_eq!(b.oki_writes(), 2);
        assert_eq!(b.read(0xF002), 0x00, "and it reads as not-busy");
    }

    /// OKI pin 7 is a latched bit the Z80 sets; D3 reads it.
    #[test]
    fn oki_pin_seven_latches_bit_zero() {
        let mut b = SoundBoard::new(rom());
        assert!(!b.oki_pin7());
        b.write(0xF006, 0x01);
        assert!(b.oki_pin7());
        b.write(0xF006, 0xFE);
        assert!(!b.oki_pin7(), "bit 0 only");
    }

    /// Unmapped addresses read 0xFF and swallow writes without panicking.
    ///
    /// Sweeps the whole 16-bit space. The guest can address anything; nothing here
    /// may panic, and nothing may index out of bounds.
    #[test]
    fn the_whole_address_space_is_safe() {
        let mut b = SoundBoard::new(rom());
        for a in 0..=0xFFFFu16 {
            let _ = b.read(a);
            b.write(a, 0x5A);
        }
        assert_eq!(b.read(0xC000), 0xFF, "the gap below sound RAM");
        assert_eq!(b.read(0xD800), 0xFF, "and above it");
        assert_eq!(b.read(0xE000), 0xFF);
        // The sweep wrote 0x5A to 0xF004, whose bit 0 is 0, so the bank is 0 and the
        // fixed ROM is untouched. Asserting it makes the sweep more than a
        // "does not panic" check.
        assert_eq!(b.bank(), 0);
        assert_eq!(b.read(0x1000), 0x01, "and ROM still reads as itself");
    }

    /// This board has no I/O ports, and the counter proves nothing uses them.
    ///
    /// `cps_state::sub_map` is `AM_RANGE`s in program space only — there is no
    /// `sub_io_map`. So `IN`/`OUT` are recorded no-ops returning 0xFF, and a non-zero
    /// count after booting a real ROM would be a genuine finding about the driver,
    /// not a shrug.
    #[test]
    fn there_are_no_io_ports_and_accesses_are_counted() {
        let mut b = SoundBoard::new(rom());
        assert_eq!(b.port_accesses(), 0);
        assert_eq!(b.port_in(0x00), 0xFF);
        b.port_out(0x00, 0x12);
        assert_eq!(
            b.port_accesses(),
            2,
            "counted, so the boot test can check it"
        );
        // A port write must not reach memory or a device: the count is all it does.
        let mut plain = SoundBoard::new(rom());
        plain.port_in(0x00);
        plain.port_out(0x00, 0x12);
        assert_eq!(plain.ram, b.ram);
        assert_eq!(plain.bank(), 0);
        assert_eq!(plain.oki_writes(), 0);
    }

    /// The fetch counter counts only bytes the ROM actually answered.
    ///
    /// **The distinction the number's whole meaning rests on.** `tests/sound_boot.rs`
    /// reads it to claim "the Z80 executed from `audiocpu`", and a machine built with
    /// no sound region reads [`UNMAPPED`] on every fetch — which is `RST 38h`, so the
    /// Z80 spins in a tight loop and racks up a *larger* count than a real driver
    /// would. A counter that counted those would make the boot test's assertion
    /// satisfiable by a machine with no sound ROM at all.
    #[test]
    fn the_fetch_counter_counts_only_bytes_the_rom_answered() {
        let mut b = SoundBoard::new(rom());
        assert_eq!(b.trace().audiocpu_fetches, 0);
        let _ = b.read(0x0000);
        let _ = b.read(0x8000);
        assert_eq!(
            b.trace().audiocpu_fetches,
            2,
            "fixed and banked windows both"
        );
        // Reads outside the ROM window are not fetches, however they read.
        let _ = b.read(0xD000);
        let _ = b.read(0xF008);
        let _ = b.read(0xE000);
        assert_eq!(
            b.trace().audiocpu_fetches,
            2,
            "RAM, a latch, and a gap are not"
        );

        // And an absent region answers nothing, so nothing is counted.
        let mut none = SoundBoard::new(Vec::new());
        for a in 0..=0xBFFFu16 {
            assert_eq!(none.read(a), UNMAPPED, "at {a:04X}");
        }
        assert_eq!(
            none.trace().audiocpu_fetches,
            0,
            "a board with no sound ROM has executed nothing, however much it read"
        );
    }

    /// Both halves of a YM2151 write are counted, and a read of the chip is not.
    ///
    /// Two per register is what a driver costs — the address latch and the data byte
    /// are separate instructions — and the count's job is to answer whether the driver
    /// reached the chip at all, so counting only the data byte would report half. A
    /// status *read* is not a write: a driver polling the status in a tight loop would
    /// otherwise look like one programming the chip furiously.
    #[test]
    fn both_halves_of_a_chip_write_are_counted_and_a_status_read_is_not() {
        let mut b = SoundBoard::new(rom());
        b.write(0xF000, 0x20);
        b.write(0xF001, 0xC7);
        assert_eq!(b.trace().ym_writes, 2);
        b.write(0xF001, 0xC0);
        assert_eq!(b.trace().ym_writes, 3, "a second data byte, same address");
        let _ = b.read(0xF000);
        let _ = b.read(0xF001);
        assert_eq!(
            b.trace().ym_writes,
            3,
            "and polling the status is not a write"
        );
        // The bank and OKI-pin registers live in the same page and are not chip writes.
        b.write(0xF004, 0x01);
        b.write(0xF006, 0x01);
        assert_eq!(b.trace().ym_writes, 3);
    }

    /// Either command latch counts as a latch read, and a write to one does not.
    #[test]
    fn reading_either_latch_is_counted() {
        let mut b = SoundBoard::new(rom());
        b.set_latch(0, 0xA5);
        b.set_latch(1, 0x5A);
        assert_eq!(b.read(0xF008), 0xA5, "and the byte is still the right one");
        assert_eq!(b.read(0xF00A), 0x5A);
        assert_eq!(b.trace().latch_reads, 2);
        // `set_latch` is the 68000's side of the board, not the guest's.
        b.set_latch(0, 0x00);
        // And the Z80 cannot write a latch, so the attempt is not a read either.
        b.write(0xF008, 0xFF);
        assert_eq!(b.trace().latch_reads, 2);
    }

    /// Clearing the trace zeroes every counter and touches nothing else.
    ///
    /// A divergence test comparing two whole boards calls this on both, so what it
    /// must *not* do is disturb the state that test is comparing. Asserted by
    /// comparing against a board that never had the counters moved: if `clear_trace`
    /// touched anything else, the two would differ.
    #[test]
    fn clearing_the_trace_zeroes_the_counters_and_nothing_else() {
        let mut worked = SoundBoard::new(rom());
        let mut untouched = SoundBoard::new(rom());
        for b in [&mut worked, &mut untouched] {
            b.write(0xD100, 0xA5);
            b.write(0xF004, 0x01);
            b.write(0xF006, 0x01);
            b.write(0xF000, 0x08);
            b.write(0xF001, 0x78);
        }
        // Only `worked` reads, so only its counters move.
        for _ in 0..7 {
            let _ = worked.read(0x0000);
            let _ = worked.read(0xF008);
        }
        worked.write(0xF002, 0x80);
        worked.port_out(0x00, 0x00);
        assert_ne!(
            worked.trace(),
            untouched.trace(),
            "the premise: the counters differ"
        );
        assert_ne!(worked, untouched, "and the derived eq sees that difference");

        worked.clear_trace();
        untouched.clear_trace();
        assert_eq!(
            worked.trace(),
            SoundTrace::default(),
            "every counter zeroed"
        );
        assert_eq!(
            worked, untouched,
            "and the boards are otherwise identical, so nothing else was touched"
        );
    }

    /// A restore leaves the counters where they were.
    ///
    /// They record the session rather than the machine, so a save state does not carry
    /// them and `restore` must not zero them either: a debugger that reset its own
    /// instrument on every state load would lose the history it was opened to read.
    #[test]
    fn a_restore_does_not_touch_the_counters() {
        let mut b = SoundBoard::new(rom());
        for _ in 0..5 {
            let _ = b.read(0x0000);
        }
        b.write(0xF002, 0x80);
        let before = b.trace();
        assert_eq!((before.audiocpu_fetches, before.oki_writes), (5, 1));
        let ram = [0x5Au8; RAM_BYTES];
        b.restore(&ram, 1, true, &Ym2151::new(), 0x08);
        assert_eq!(b.trace(), before, "the instrument survives the state load");
        assert_eq!(b.bank(), 1, "and the state really was restored");
    }

    /// `peek_byte` returns what the bus would, everywhere, and moves no counter.
    ///
    /// **The whole 16-bit space, not a sample.** `peek_byte` writes the address map a
    /// second time — it must, because the point of it is not bumping the counters
    /// `read` bumps — so nothing structural stops the two drifting. A test spot-checking
    /// a few addresses would miss exactly the arm someone forgets, and the arm someone
    /// forgets is where a debugger's disassembly silently shows 0xFF for a byte the
    /// Z80 reads as something else.
    ///
    /// The counter half is the other claim: a panel that peeked its way through a
    /// disassembly window once a frame would add its own reads to the numbers
    /// `tests/sound_boot.rs` asserts.
    #[test]
    fn peeking_agrees_with_the_bus_and_moves_no_counter() {
        for bank in [0u8, 1] {
            let mut b = SoundBoard::new(rom());
            b.write(0xF004, bank);
            b.set_latch(0, 0xA5);
            b.set_latch(1, 0x5A);
            b.write(0xD000, 0x11);
            b.write(0xD7FF, 0x22);
            // Something in the chip's status, so the 0xF000/0xF001 arms are not both
            // trivially zero.
            b.write(0xF000, 0x14);
            b.write(0xF001, 0x0F);
            b.clear_trace();

            for a in 0..=0xFFFFu16 {
                // Peek first: if it had side effects, the bus read below would see
                // them and the two could still agree while both being wrong.
                let peeked = b.peek_byte(a);
                assert_eq!(
                    b.trace(),
                    SoundTrace::default(),
                    "peeking {a:04X} moved a counter"
                );
                assert_eq!(peeked, b.read(a), "at {a:04X}, bank {bank}");
                b.clear_trace();
            }
        }
    }

    /// A short ROM does not panic; it reads 0xFF where the data is missing.
    ///
    /// A ROM set the user supplies may be a variant with a smaller sound ROM. That
    /// is a load-time report, not a crash.
    #[test]
    fn a_short_rom_reads_as_unmapped_rather_than_panicking() {
        let mut b = SoundBoard::new(vec![0xAAu8; 0x8000]);
        assert_eq!(b.read(0x0000), 0xAA);
        assert_eq!(b.read(0x8000), 0xFF, "no banked region present");
        b.write(0xF004, 1);
        assert_eq!(b.read(0xBFFF), 0xFF);
        // An empty ROM too, which is what a missing region looks like.
        let mut none = SoundBoard::new(Vec::new());
        for a in 0..=0xBFFFu16 {
            assert_eq!(none.read(a), 0xFF, "at {a:04X}");
        }
    }
}
