//! SF1's FM sound board: everything Z80 #1 can address.
//!
//! A separate struct from the CPU for [`crate::sound`]'s reason —
//! `Z80::step(&mut bus)` borrows both at once, so the CPU cannot live inside the
//! thing it buses to.
//!
//! Map cited to MAME `mame0261`, `src/mame/capcom/sf.cpp:209-215`
//! (`sf_state::sound_map`).
//!
//! # Why this is not [`crate::sound::SoundBoard`]
//!
//! ```text
//! 0x0000-0x7fff  rom
//! 0xc000-0xc7ff  ram
//! 0xc800         r soundlatch
//! 0xe000-0xe001  rw ym2151
//! ```
//!
//! Four entries against CPS-1's eight, and **not one address matches**. There is
//! no ROM bank, no OKI, no pin-7 register, and one latch rather than two. Sharing
//! a type would mean a runtime-configured map, which is how a board becomes
//! correct by coincidence.
//!
//! What stays shared: the Z80 core, `Ym2151`, and [`UNMAPPED`]'s value and
//! argument — the bus is the same bus.
//!
//! ⚠️ **No ROM bank.** The window is `0x0000-0x7fff` and nothing else, so
//! `0x8000-0xBFFF` is unmapped here where CPS-1's board serves banked ROM.
//! Adapting that arm would serve bytes from past this region's 32 KB, and for any
//! larger region those are plausible data rather than [`UNMAPPED`] — so the guest
//! would execute garbage instead of hitting `RST 38h`.
//!
//! # One latch, two CPUs
//!
//! `GENERIC_LATCH_8(config, m_soundlatch)` (`sf.cpp:780`) is one device, read by
//! this board at `0xC800` and by `Adpcm2Board` at port `0x01`. Each
//! board holds its own copy and `Sf1` writes both from one place — not a
//! shared cell, which would be a borrow problem in exchange for nothing, since
//! `z80::Bus` is `&mut self` and the two CPUs are stepped in turn. What makes the
//! copies safe is that no bus path writes the latch: [`FmBoard::set_latch`] is
//! the only door. See `no_bus_write_can_change_the_latch`.
//!
//! Reading does not clear it — `generic_latch_8_device::read` returns
//! `m_latched_value` and only warns about a read-before-write
//! (`gen_latch.cpp:74-84`). The take-once discipline lives on the 68000 side,
//! where the NMI is: `Sf1Board::take_sound_command`.
//!
//! # Never panics on a guest address
//!
//! Every unmapped read returns [`UNMAPPED`] and every unmapped write is dropped.
//! ROM reads go through `get()` rather than an index, so a user-supplied ROM set
//! with a short sound ROM reads as unmapped instead of panicking. See
//! `the_whole_address_space_is_safe`.
//!
//! # This crate holds no ROM
//!
//! [`FmBoard::new`] takes the assembled `audiocpu` region as a `Vec<u8>`;
//! assembling it from a user-supplied ROM set is `romset`'s job. Every test here
//! builds its ROM inline.

use ym2151::Ym2151;

/// What an unmapped read returns.
///
/// [`crate::sound::UNMAPPED`]'s value and its argument, because the bus is the
/// same bus: the Z80's data bus floats high and the board's pull-ups read that
/// back as ones. `0x00` would be wrong for a reason beyond fidelity — it is `NOP`,
/// so a runaway PC in unmapped space would run quietly forever instead of hitting
/// `RST 38h` (0xFF) and being noticed.
pub const UNMAPPED: u8 = 0xFF;

/// Sound RAM, `0xC000-0xC7FF`: 2 KB (`sf.cpp:211`).
///
/// Public because the save state carries a copy and its codec has to name the
/// array's length.
pub const RAM_BYTES: usize = 0x800;

/// First address of sound RAM.
const RAM_BASE: u16 = 0xC000;
/// The ROM window's end, exclusive (`sf.cpp:210`, `map(0x0000, 0x7fff).rom()`).
const ROM_END: u16 = 0x8000;
/// Where the one sound latch is read (`sf.cpp:212`).
const LATCH_ADDR: u16 = 0xC800;
/// The YM2151's address-latch port; `YM_ADDR + 1` is its data port (`sf.cpp:213`).
const YM_ADDR_PORT: u16 = 0xE000;

/// What this board saw: five counters, none of them machine state.
///
/// [`crate::sound::SoundTrace`]'s counterpart, and the same argument for being separate
/// from the machine — it records the session rather than the hardware, so it is
/// not in the save state and `clear_trace` is not a reset.
///
/// Five rather than eight: `oki_writes` and `oki_clamps` are about a chip this
/// board does not have, and the two host-ring counters live on `Sf1`,
/// because SF1's ring feeds a stereo mix that no single board owns. It gains
/// [`FmTrace::rom_writes`], which CPS-1's board drops silently.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FmTrace {
    /// Writes the guest made to the YM2151, address latches and data bytes alike.
    ///
    /// Both halves, for [`crate::sound::SoundTrace::ym_writes`]'s reason: the driver's cost
    /// is two instructions per register and the question is whether it reached the
    /// chip at all, so counting only the data half would report half.
    pub ym_writes: u32,
    /// Reads the guest made of the sound latch at `0xC800`.
    pub latch_reads: u32,
    /// Bytes the guest read from the `audiocpu` region, **as answered**.
    ///
    /// Opcode fetches plus immediates and any table the driver reads from its own
    /// ROM: `z80::Bus::read` carries no M1 flag, so the board cannot tell them
    /// apart, and the number's job is to answer "did the Z80 execute from
    /// `audiocpu` at all". A read the ROM did not answer is **not** counted — see
    /// `the_fetch_counter_counts_only_bytes_the_rom_answered` — so a machine built
    /// with no sound region reports 0 rather than a large number describing a Z80
    /// spinning on `RST 38h`.
    pub audiocpu_fetches: u32,
    /// Writes the guest made into the ROM window, all of them dropped.
    ///
    /// New against CPS-1's board, which drops them silently. This board has 2 KB of
    /// RAM against 32 KB of ROM, so a stray store is far more likely to land in ROM
    /// than anywhere useful, and the count is the diagnostic.
    pub rom_writes: u32,
    /// I/O port accesses the guest made. This board has no ports, so a non-zero
    /// count is a finding about the driver rather than a shrug.
    pub port_accesses: u32,
}

/// Z80 #1's bus: 32 KB of ROM, 2 KB of RAM, one latch and the FM chip.
pub struct FmBoard {
    /// The assembled `audiocpu` region. No bank: `0x0000-0x7FFF` and nothing else.
    rom: Vec<u8>,
    /// Sound RAM.
    ram: [u8; RAM_BYTES],
    /// The FM chip.
    ym: Ym2151,
    /// The register selected by a write to `0xE000`, consumed by the next `0xE001`.
    ym_addr: u8,
    /// This board's copy of the machine's one sound latch.
    ///
    /// Written only by [`FmBoard::set_latch`] — see the module doc.
    latch: u8,
    /// What the guest has done, counted. Not machine state — see [`FmTrace`].
    trace: FmTrace,
}

impl core::fmt::Debug for FmBoard {
    /// Hand-written for [`crate::sound::SoundBoard`]'s reason: a derived `Debug` on a
    /// struct holding a 32 KB `Vec` produces 32 KB of output in a panic message.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FmBoard")
            .field("rom_len", &self.rom.len())
            .field("ym_addr", &self.ym_addr)
            .field("latch", &self.latch)
            .field("trace", &self.trace)
            .finish_non_exhaustive()
    }
}

impl FmBoard {
    /// A board holding `audiocpu`, with a reset YM2151, zeroed RAM and an empty
    /// latch.
    #[must_use]
    pub fn new(audiocpu: Vec<u8>) -> Self {
        Self {
            rom: audiocpu,
            ram: [0; RAM_BYTES],
            ym: Ym2151::new(),
            ym_addr: 0,
            latch: 0,
            trace: FmTrace::default(),
        }
    }

    /// Put back what a save state carries: RAM, the YM address latch, the latch.
    ///
    /// The chip itself is restored through `Ym2151`'s own codec, and the ROM comes
    /// from the ROM set — neither is this method's business. The trace is not
    /// touched, because a restore is not a session.
    pub fn restore(&mut self, ram: [u8; RAM_BYTES], ym_addr: u8, latch: u8) {
        self.ram = ram;
        self.ym_addr = ym_addr;
        self.latch = latch;
    }

    /// The FM chip, for the scheduler to clock and the debugger to inspect.
    pub fn ym(&mut self) -> &mut Ym2151 {
        &mut self.ym
    }

    /// The FM chip without borrowing the board mutably, for a snapshot.
    ///
    /// [`FmBoard::ym`] cannot serve: `Sf1::snapshot` takes `&self`.
    #[must_use]
    pub const fn ym_ref(&self) -> &Ym2151 {
        &self.ym
    }

    /// The chip's own reset, and nothing else on the board.
    ///
    /// A narrow door rather than relying on [`FmBoard::ym`]: `Sf1::reset`
    /// must reach the chip because MAME's machine reset propagates `device_reset` to
    /// it, but it has no business moving the address latch or the RAM — which
    /// `machine_reset` (`sf.cpp:748-753`) does not touch.
    pub fn reset_ym(&mut self) {
        self.ym.reset();
    }

    /// Sound RAM, for a snapshot.
    #[must_use]
    pub const fn ram(&self) -> &[u8; RAM_BYTES] {
        &self.ram
    }

    /// The register a `0xE001` write would reach.
    #[must_use]
    pub const fn ym_addr(&self) -> u8 {
        self.ym_addr
    }

    /// Hand over the byte the 68000 wrote to `soundcmd_w`.
    ///
    /// The only door to the latch, and `Sf1` is its only caller — which is
    /// what keeps this board's copy and the ADPCM board's copy equal.
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
    pub const fn trace(&self) -> FmTrace {
        self.trace
    }

    /// Zero the counters. Not a reset: no machine state moves.
    pub fn clear_trace(&mut self) {
        self.trace = FmTrace::default();
    }

    /// Sets every counter to its maximum, for a frontend panel-width test.
    ///
    /// See [`crate::sf1::Sf1::saturate_counters_for_test`], which is the only caller
    /// and which explains why this is `pub` rather than `#[cfg(test)]`. Assigns the
    /// whole struct rather than each field, so a counter added to [`FmTrace`] later is
    /// saturated by this without anyone remembering to: a literal missing a field
    /// fails the build, which is the property that makes that true.
    pub fn saturate_trace_for_test(&mut self) {
        self.trace = FmTrace {
            ym_writes: u32::MAX,
            latch_reads: u32::MAX,
            audiocpu_fetches: u32::MAX,
            rom_writes: u32::MAX,
            port_accesses: u32::MAX,
        };
    }

    /// Read a byte without acknowledging anything or moving a counter.
    ///
    /// Mirrors [`z80::Bus::read`]'s map arm for arm, and
    /// `peeking_agrees_with_the_bus_and_moves_no_counter` walks all 65,536 addresses
    /// to hold the two together — two maps that can disagree is what that test
    /// exists to prevent, and the counters mean the map genuinely has to be written
    /// twice.
    ///
    /// `&self` is the enforcement rather than a preference: a `&mut self` version
    /// could bump a counter and the compiler would not object. It is also what lets
    /// the overlay hold `&Sf1` — [`z80::disasm::disasm_bus`] takes `&mut B` and so
    /// cannot be used there at all.
    #[must_use]
    pub fn peek_byte(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.rom_byte(addr).unwrap_or(UNMAPPED),
            RAM_BASE..=0xC7FF => self.ram[usize::from(addr - RAM_BASE)],
            LATCH_ADDR => self.latch,
            YM_ADDR_PORT | 0xE001 => self.ym.read_status(),
            _ => UNMAPPED,
        }
    }

    /// The ROM byte behind a program address, or `None` if the ROM is too short.
    ///
    /// `get()` rather than an index: a user-supplied ROM set with a short sound ROM
    /// must read as unmapped, not panic.
    fn rom_byte(&self, addr: u16) -> Option<u8> {
        debug_assert!(addr < ROM_END, "only the ROM window reaches here");
        self.rom.get(usize::from(addr)).copied()
    }
}

impl z80::Bus for FmBoard {
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => match self.rom_byte(addr) {
                Some(b) => {
                    self.trace.audiocpu_fetches = self.trace.audiocpu_fetches.saturating_add(1);
                    b
                }
                None => UNMAPPED,
            },
            RAM_BASE..=0xC7FF => self.ram[usize::from(addr - RAM_BASE)],
            LATCH_ADDR => {
                self.trace.latch_reads = self.trace.latch_reads.saturating_add(1);
                self.latch
            }
            // Both YM2151 addresses read the status register: the chip has one status
            // port and `map(0xe000, 0xe001).rw(...)` does not decode A0 for reads.
            YM_ADDR_PORT | 0xE001 => self.ym.read_status(),
            _ => UNMAPPED,
        }
    }

    fn write(&mut self, addr: u16, val: u8) {
        match addr {
            0x0000..=0x7FFF => {
                self.trace.rom_writes = self.trace.rom_writes.saturating_add(1);
            }
            RAM_BASE..=0xC7FF => self.ram[usize::from(addr - RAM_BASE)] = val,
            YM_ADDR_PORT => {
                self.trace.ym_writes = self.trace.ym_writes.saturating_add(1);
                self.ym_addr = val;
            }
            0xE001 => {
                self.trace.ym_writes = self.trace.ym_writes.saturating_add(1);
                self.ym.write(self.ym_addr, val);
            }
            // Everything else, including the latch at 0xC800, which is read-only to
            // this CPU. See the module doc: `set_latch` is the only door.
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

    /// A 32 KB ROM in which each byte names the 4 KB page it lives in.
    ///
    /// `>> 12` for the reason `crate::sound`'s fixture records: a byte holding
    /// `i >> 8` truncates and stops discriminating. 32 KB is the region's real size
    /// — `sound_map`'s ROM window is `0x0000-0x7fff` and there is no bank — so a
    /// fixture larger than that would hide the unmapped window above it.
    fn rom_pages() -> Vec<u8> {
        (0..0x8000usize).map(|i| (i >> 12) as u8).collect()
    }

    /// The map's four regions, from `sf.cpp:209-215`.
    #[test]
    fn the_map_is_rom_ram_latch_and_the_ym() {
        let mut b = FmBoard::new(rom_pages());
        assert_eq!(b.read(0x0000), 0x00, "rom, first page");
        assert_eq!(b.read(0x7FFF), 0x07, "rom, last page");
        b.write(0xC000, 0x5A);
        assert_eq!(b.read(0xC000), 0x5A, "ram");
        b.write(0xC7FF, 0xA5);
        assert_eq!(b.read(0xC7FF), 0xA5, "ram, last byte");
        b.set_latch(0x42);
        assert_eq!(b.read(0xC800), 0x42, "the latch");
        // The YM's status register: bit 7 is busy, bits 0-1 are the timers, and a
        // reset chip reads 0. Asserted as a value rather than "not UNMAPPED" so an
        // arm that fell through to 0xFF cannot pass.
        assert_eq!(b.read(0xE000), 0x00, "the ym status");
        assert_eq!(b.read(0xE001), 0x00);
    }

    /// The RAM is 2 KB at 0xC000, and it is the only writable memory.
    ///
    /// `map(0xc000, 0xc7ff).ram()` — 0x800 bytes. The address just past it must not
    /// alias into it, which is the failure a `& 0x7FF` on too wide a range produces.
    #[test]
    fn the_ram_is_two_kilobytes_at_0xc000() {
        assert_eq!(RAM_BYTES, 0x800, "0xc7ff - 0xc000 + 1");
        let mut b = FmBoard::new(rom_pages());
        for addr in [0xC000u16, 0xC400, 0xC7FF] {
            b.write(addr, 0x11);
            assert_eq!(b.read(addr), 0x11);
        }
        // 0xC800 is the latch, not RAM, and a write there must not reach RAM.
        b.write(0xC000, 0x22);
        b.write(0xC800, 0x33);
        assert_eq!(
            b.read(0xC000),
            0x22,
            "the latch write did not alias into RAM"
        );
        // And no aliasing 2 KB further up either.
        b.write(0xC800 + 0x400, 0x44);
        assert_eq!(b.read(0xC400), 0x11, "still what was written at 0xC400");
    }

    /// There is no ROM bank: everything above 0x7FFF that is not decoded is unmapped.
    ///
    /// ⚠️ The arm most easily got wrong by adapting `crate::sound`, whose `read`
    /// answers `0x0000..=0xBFFF` from ROM. Copying that arm would serve bytes from
    /// past this region's 32 KB — plausible data rather than 0xFF for any larger
    /// ROM — so the guest would execute garbage instead of hitting `RST 38h`. The
    /// fixture here is deliberately 64 KB so that a copied arm *finds* something.
    #[test]
    fn there_is_no_rom_bank_above_the_fixed_window() {
        let oversized: Vec<u8> = (0..0x1_0000usize).map(|i| (i >> 12) as u8).collect();
        let mut b = FmBoard::new(oversized);
        assert_eq!(b.read(0x7FFF), 0x07, "the fixed window ends here");
        for addr in [0x8000u16, 0x9000, 0xA000, 0xBFFF] {
            assert_eq!(
                b.read(addr),
                UNMAPPED,
                "{addr:#06x} is unmapped, not banked ROM"
            );
        }
        assert_eq!(
            b.trace().audiocpu_fetches,
            1,
            "only the 0x7FFF read was a fetch"
        );
    }

    /// The gaps between the decoded regions are unmapped.
    #[test]
    fn the_gaps_read_as_unmapped() {
        let mut b = FmBoard::new(rom_pages());
        for addr in [0x8000u16, 0xBFFF, 0xC801, 0xCFFF, 0xDFFF, 0xE002, 0xFFFF] {
            assert_eq!(b.read(addr), UNMAPPED, "{addr:#06x}");
        }
    }

    /// Unmapped reads answer 0xFF, and the argument for that value.
    ///
    /// The Z80's data bus floats high and the pull-ups read it back as ones. 0x00
    /// would be `NOP`, so a runaway PC in unmapped space would run quietly forever
    /// instead of hitting `RST 38h` and being noticed — `crate::sound::UNMAPPED`'s
    /// argument, unchanged, because the bus is the same bus.
    #[test]
    fn unmapped_is_all_ones_and_not_a_nop() {
        assert_eq!(UNMAPPED, 0xFF);
        assert_ne!(UNMAPPED, 0x00, "0x00 is NOP and would hide a runaway PC");
    }

    /// A write to ROM is dropped and counted.
    ///
    /// A driver bug, not a crash — the board has 2 KB of RAM and 32 KB of ROM, so a
    /// stray store is far more likely to land in ROM than anywhere useful. CPS-1's
    /// board drops it silently; this one counts it, because on a board this small
    /// the count is diagnostic.
    #[test]
    fn a_rom_write_is_dropped_and_counted() {
        let mut b = FmBoard::new(rom_pages());
        b.write(0x0000, 0xFF);
        b.write(0x7FFF, 0xFF);
        assert_eq!(b.read(0x0000), 0x00, "the ROM did not change");
        assert_eq!(b.read(0x7FFF), 0x07);
        assert_eq!(b.trace().rom_writes, 2);
    }

    /// The YM's address latch and data write decode A0; reads do not.
    ///
    /// `map(0xe000, 0xe001).rw(...)` hands both addresses to the device, and the
    /// device's `read` ignores the offset — it has one status port. Writes do split:
    /// 0xE000 latches the register number, 0xE001 writes the value.
    #[test]
    fn the_ym_decodes_a_zero_for_writes_and_not_for_reads() {
        let mut b = FmBoard::new(rom_pages());
        // Register 0x14 is the timer control; setting bit 0 starts timer A, which the
        // chip reports in status bit 0 once it expires. Here the observable is
        // narrower and sufficient: the address latch is visible directly.
        b.write(0xE000, 0x14);
        assert_eq!(b.ym_addr(), 0x14, "0xE000 latched the address");
        assert_eq!(b.trace().ym_writes, 1);
        b.write(0xE001, 0x00);
        assert_eq!(b.ym_addr(), 0x14, "the data write left the latch alone");
        assert_eq!(b.trace().ym_writes, 2, "both halves are counted");
        // Reads: the same value from both addresses.
        assert_eq!(b.read(0xE000), b.read(0xE001));
    }

    /// A write reaches the chip, not just the board's latch.
    ///
    /// The end-to-end check the latch-only assertions above cannot make: key on a
    /// channel through the bus and the chip produces a nonzero sample. Without it, a
    /// board that latched the address and dropped the data would pass every test
    /// above.
    #[test]
    fn a_bus_write_reaches_the_chip() {
        let mut b = FmBoard::new(rom_pages());
        // A minimal audible voice on channel 0: total level 0 on the carrier, a
        // release rate, then key-on with all four slots.
        for (reg, val) in [
            (0x60u8, 0x00), // TL, slot 1 (M1)
            (0x68, 0x00),   // TL, slot 2
            (0x70, 0x00),   // TL, slot 3
            (0x78, 0x00),   // TL, slot 4
            (0x80, 0x1F),   // AR/KS, slot 1
            (0x88, 0x1F),
            (0x90, 0x1F),
            (0x98, 0x1F),
            (0x28, 0x4A), // key code
            (0x08, 0x78), // key on, channel 0, all slots
        ] {
            b.write(0xE000, reg);
            b.write(0xE001, val);
        }
        let mut out = [(0i16, 0i16); 256];
        b.ym().generate(&mut out);
        assert!(
            out.iter().any(|&(l, r)| l != 0 || r != 0),
            "the chip produced no sample, so the data writes did not reach it"
        );
    }

    /// The latch is a register on this side: reading does not clear it.
    ///
    /// `generic_latch_8_device::read` returns `m_latched_value` and only warns about
    /// a read-before-write (`gen_latch.cpp:74-84`). The take-once discipline belongs
    /// on the 68000 side, where the NMI is — Task 9's `take_sound_command`.
    #[test]
    fn reading_the_latch_does_not_clear_it() {
        let mut b = FmBoard::new(rom_pages());
        b.set_latch(0x42);
        assert_eq!(b.read(0xC800), 0x42);
        assert_eq!(b.read(0xC800), 0x42, "still there");
        assert_eq!(b.latch(), 0x42);
        assert_eq!(b.trace().latch_reads, 2);
    }

    /// The bus cannot write the latch.
    ///
    /// [`FmBoard::set_latch`] is the only door, and Task 15's `Sf1` is its only
    /// caller. That is what makes the two boards' separate copies of the one
    /// hardware latch safe — so a bus path that wrote it would break the invariant
    /// silently, by making the two copies diverge.
    #[test]
    fn no_bus_write_can_change_the_latch() {
        let mut b = FmBoard::new(rom_pages());
        b.set_latch(0x42);
        for addr in [0x0000u16, 0xC000, 0xC7FF, 0xC800, 0xE000, 0xE001, 0xFFFF] {
            b.write(addr, 0x99);
            assert_eq!(b.latch(), 0x42, "a write at {addr:#06x} moved the latch");
        }
    }

    /// The fetch counter counts only bytes the ROM answered.
    ///
    /// `crate::sound`'s discipline, and the reason is the same: a machine built with
    /// no sound region reads UNMAPPED on every fetch, and counting those would
    /// report a Z80 executing from `audiocpu` when there is no `audiocpu` — which is
    /// exactly the claim a boot test makes with the number.
    #[test]
    fn the_fetch_counter_counts_only_bytes_the_rom_answered() {
        let mut empty = FmBoard::new(Vec::new());
        for addr in 0x0000u16..0x0100 {
            assert_eq!(empty.read(addr), UNMAPPED);
        }
        assert_eq!(empty.trace().audiocpu_fetches, 0, "no ROM, no fetches");
        let mut short = FmBoard::new(vec![0xC9; 0x10]);
        for addr in 0x0000u16..0x0020 {
            short.read(addr);
        }
        assert_eq!(
            short.trace().audiocpu_fetches,
            0x10,
            "exactly what it answered"
        );
    }

    /// A ROM shorter than the window reads as unmapped rather than panicking.
    #[test]
    fn a_short_rom_reads_as_unmapped() {
        let mut b = FmBoard::new(vec![0x11, 0x22]);
        assert_eq!(b.read(0x0000), 0x11);
        assert_eq!(b.read(0x0001), 0x22);
        assert_eq!(b.read(0x0002), UNMAPPED);
        assert_eq!(b.read(0x7FFF), UNMAPPED);
    }

    /// Nothing in the 16-bit space panics, on either a full or an empty ROM.
    #[test]
    fn the_whole_address_space_is_safe() {
        for rom in [Vec::new(), vec![0u8; 3], rom_pages()] {
            let mut b = FmBoard::new(rom);
            for addr in 0x0000u16..=0xFFFF {
                let _ = b.read(addr);
                b.write(addr, 0x5A);
                let _ = b.peek_byte(addr);
            }
        }
    }

    /// This board has no I/O ports, so any port access is a finding.
    ///
    /// `sound_map` is a program map with no `AS_IO` counterpart — unlike Task 13's
    /// board, which is nearly all ports. A driver that reaches for one here has
    /// mistaken which CPU it is on, and the counter is how a reader learns that
    /// rather than hearing silence.
    #[test]
    fn a_port_access_is_unmapped_and_counted() {
        let mut b = FmBoard::new(rom_pages());
        assert_eq!(b.port_in(0x00), UNMAPPED);
        b.port_out(0x00, 0x5A);
        assert_eq!(b.trace().port_accesses, 2);
    }

    /// `peek_byte` agrees with the bus at every address and moves no counter.
    ///
    /// Two maps that can disagree is what this prevents; the counters mean the map
    /// genuinely has to be written twice, so it is walked in full. `&self` is the
    /// enforcement rather than a preference — a `&mut self` version could bump a
    /// counter and the compiler would not object — and it is what lets the overlay
    /// hold `&Sf1`.
    #[test]
    fn peeking_agrees_with_the_bus_and_moves_no_counter() {
        let mut b = FmBoard::new(rom_pages());
        b.set_latch(0x42);
        for addr in 0xC000u16..=0xC7FF {
            b.write(addr, (addr & 0xFF) as u8);
        }
        b.clear_trace();
        let before = b.trace();
        for addr in 0x0000u16..=0xFFFF {
            assert_eq!(
                b.peek_byte(addr),
                b.peek_byte(addr),
                "{addr:#06x} is stable"
            );
        }
        assert_eq!(b.trace(), before, "peeking moved a counter");
        // Now against the bus, which does move counters — so compare first, then
        // check the values rather than the counts.
        let mut fresh = FmBoard::new(rom_pages());
        fresh.set_latch(0x42);
        for addr in 0xC000u16..=0xC7FF {
            fresh.write(addr, (addr & 0xFF) as u8);
        }
        for addr in 0x0000u16..=0xFFFF {
            let peeked = fresh.peek_byte(addr);
            assert_eq!(peeked, fresh.read(addr), "{addr:#06x} disagrees");
        }
    }

    /// A fresh board is a reset chip, zeroed RAM, and an empty latch.
    #[test]
    fn a_fresh_board_is_at_rest() {
        let b = FmBoard::new(rom_pages());
        assert_eq!(b.ram(), &[0u8; RAM_BYTES]);
        assert_eq!(b.ym_addr(), 0);
        assert_eq!(b.latch(), 0);
        assert!(!b.ym_ref().irq(), "no pending FM interrupt");
        assert_eq!(b.trace(), FmTrace::default());
    }

    /// `reset_ym` is a narrow door, and it leaves the board's own state alone.
    ///
    /// A `&mut Ym2151` accessor exists for the scheduler, which has to clock the
    /// chip; this is for `Sf1::reset`, which must reach the chip's reset
    /// without also being licensed to move the address latch or the RAM. MAME's
    /// machine reset propagates `device_reset` to the chip, which is why the machine
    /// needs a door at all.
    #[test]
    fn reset_ym_leaves_the_boards_own_state_alone() {
        let mut b = FmBoard::new(rom_pages());
        b.write(0xE000, 0x14);
        b.write(0xC000, 0x5A);
        b.set_latch(0x42);
        b.reset_ym();
        assert_eq!(b.ym_addr(), 0x14, "the board's latch is the board's");
        assert_eq!(b.ram()[0], 0x5A, "and so is its RAM");
        assert_eq!(b.latch(), 0x42);
        assert_eq!(b.read(0xE000), 0x00, "but the chip is reset");
    }

    /// `restore` round-trips everything the save state carries.
    #[test]
    fn restore_round_trips_the_boards_state() {
        let mut ram = [0u8; RAM_BYTES];
        ram[0] = 0x11;
        ram[RAM_BYTES - 1] = 0x22;
        let mut b = FmBoard::new(rom_pages());
        b.restore(ram, 0x14, 0x42);
        assert_eq!(b.ram()[0], 0x11);
        assert_eq!(b.ram()[RAM_BYTES - 1], 0x22);
        assert_eq!(b.ym_addr(), 0x14);
        assert_eq!(b.latch(), 0x42);
        assert_eq!(b.trace(), FmTrace::default(), "a restore is not a session");
    }

    /// `clear_trace` zeroes the counters and touches nothing else.
    #[test]
    fn clearing_the_trace_moves_no_state() {
        let mut b = FmBoard::new(rom_pages());
        b.write(0xC000, 0x5A);
        b.write(0xE000, 0x14);
        b.set_latch(0x42);
        b.read(0x0000);
        assert_ne!(b.trace(), FmTrace::default(), "the premise");
        b.clear_trace();
        assert_eq!(b.trace(), FmTrace::default());
        assert_eq!(b.ram()[0], 0x5A);
        assert_eq!(b.ym_addr(), 0x14);
        assert_eq!(b.latch(), 0x42);
    }

    /// `Debug` names the ROM's length rather than printing it.
    ///
    /// `crate::SoundBoard`'s reason, unchanged: a derived `Debug` on a struct holding
    /// a 32 KB `Vec` produces 32 KB of output in a panic message.
    #[test]
    fn debug_does_not_print_the_rom() {
        let s = format!("{:?}", FmBoard::new(rom_pages()));
        assert!(s.contains("rom_len: 32768"), "{s}");
        assert!(
            !s.contains("rom:"),
            "the ROM's bytes are in the output: {s}"
        );
    }
}
