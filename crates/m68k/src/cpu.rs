//! The 68000 CPU state and instruction stepping.

/// Status register bit positions.
pub const SR_C: u16 = 1 << 0;
pub const SR_V: u16 = 1 << 1;
pub const SR_Z: u16 = 1 << 2;
pub const SR_N: u16 = 1 << 3;
pub const SR_X: u16 = 1 << 4;
pub const SR_S: u16 = 1 << 13;
pub const SR_T: u16 = 1 << 15;

/// Bits of the SR that physically exist on a 68000. Writes to other bits are
/// dropped, which matters because the test suite asserts SR exactly.
pub const SR_MASK: u16 = 0xA71F;

/// The 68000's address bus is 24 bits wide.
pub const ADDR_MASK: u32 = 0x00FF_FFFF;

#[derive(Clone, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct M68k {
    pub d: [u32; 8],
    /// `a[7]` is the *active* stack pointer, mirroring `usp` or `ssp`
    /// depending on the S bit.
    pub a: [u32; 8],
    /// Always 4 bytes beyond the instruction word currently executing,
    /// because of the two-word prefetch queue.
    pub pc: u32,
    pub sr: u16,
    pub usp: u32,
    pub ssp: u32,
    /// Two-word prefetch queue. `prefetch[0]` is the instruction word being
    /// executed; `prefetch[1]` is the next word already fetched.
    pub prefetch: [u16; 2],
    /// Set by a double bus fault. The CPU is dead until reset.
    pub halted: bool,
    /// Set by STOP, cleared by an interrupt.
    pub stopped: bool,
    /// Pending interrupt priority level, 0 = none.
    pub pending_irq: u8,
    /// True while an exception is being entered. A second fault raised during
    /// that window is a double fault, which halts the CPU.
    pub in_exception: bool,
}

impl M68k {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn sr_s(&self) -> bool {
        self.sr & SR_S != 0
    }

    /// Writes the SR, swapping stack pointers if the S bit changes.
    pub fn set_sr(&mut self, val: u16) {
        let was_super = self.sr_s();
        let now_super = val & SR_S != 0;
        if was_super != now_super {
            // Save the active SP into its own slot, then load the other.
            if was_super {
                self.ssp = self.a[7];
                self.a[7] = self.usp;
            } else {
                self.usp = self.a[7];
                self.a[7] = self.ssp;
            }
        }
        self.sr = val & SR_MASK;
    }

    #[inline]
    pub fn ccr_x(&self) -> bool {
        self.sr & SR_X != 0
    }
    #[inline]
    pub fn ccr_n(&self) -> bool {
        self.sr & SR_N != 0
    }
    #[inline]
    pub fn ccr_z(&self) -> bool {
        self.sr & SR_Z != 0
    }
    #[inline]
    pub fn ccr_v(&self) -> bool {
        self.sr & SR_V != 0
    }
    #[inline]
    pub fn ccr_c(&self) -> bool {
        self.sr & SR_C != 0
    }

    pub fn set_ccr(&mut self, x: bool, n: bool, z: bool, v: bool, c: bool) {
        let mut sr = self.sr & !(SR_X | SR_N | SR_Z | SR_V | SR_C);
        if x {
            sr |= SR_X;
        }
        if n {
            sr |= SR_N;
        }
        if z {
            sr |= SR_Z;
        }
        if v {
            sr |= SR_V;
        }
        if c {
            sr |= SR_C;
        }
        self.sr = sr;
    }

    /// Shifts the queue one word and refills slot 1 from PC.
    ///
    /// Called by instruction handlers as their first bus operation: the opcode
    /// word at `prefetch[0]` was already peeked by `step_with`; this discards
    /// it, promotes slot 1, then fetches the next word into slot 1 from the
    /// current PC, advancing PC by 2.
    ///
    /// **Exception-aborting handlers must NOT call this.**  They return after
    /// `exception::take`, which refills both slots with `refill_prefetch_dyn`.
    pub(crate) fn consume_opcode_dyn(&mut self, bus: &mut dyn crate::Bus) {
        self.prefetch[0] = self.prefetch[1];
        self.prefetch[1] = bus.read16(self.pc & ADDR_MASK);
        self.pc = self.pc.wrapping_add(2);
    }

    #[inline]
    // Used by instruction handlers in Tasks 5+.
    #[allow(dead_code)]
    pub(crate) fn consume_opcode(&mut self, bus: &mut impl crate::Bus) {
        self.consume_opcode_dyn(bus);
    }

    /// Consumes the next word from the prefetch queue, refilling from PC.
    pub(crate) fn fetch_word_dyn(&mut self, bus: &mut dyn crate::Bus) -> u16 {
        let w = self.prefetch[0];
        self.prefetch[0] = self.prefetch[1];
        self.prefetch[1] = bus.read16(self.pc & ADDR_MASK);
        self.pc = self.pc.wrapping_add(2);
        w
    }

    #[allow(dead_code)]
    pub(crate) fn fetch_long_dyn(&mut self, bus: &mut dyn crate::Bus) -> u32 {
        let hi = self.fetch_word_dyn(bus) as u32;
        let lo = self.fetch_word_dyn(bus) as u32;
        (hi << 16) | lo
    }

    /// Refills both queue slots from PC, as after a jump.
    pub(crate) fn refill_prefetch_dyn(&mut self, bus: &mut dyn crate::Bus) {
        self.prefetch[0] = bus.read16(self.pc & ADDR_MASK);
        self.prefetch[1] = bus.read16(self.pc.wrapping_add(2) & ADDR_MASK);
        self.pc = self.pc.wrapping_add(4);
    }

    #[inline]
    // Used by instruction handlers in Tasks 5+.
    #[allow(dead_code)]
    pub(crate) fn fetch_word(&mut self, bus: &mut impl crate::Bus) -> u16 {
        self.fetch_word_dyn(bus)
    }

    #[inline]
    #[allow(dead_code)]
    pub(crate) fn fetch_long(&mut self, bus: &mut impl crate::Bus) -> u32 {
        self.fetch_long_dyn(bus)
    }

    #[inline]
    // Used by branch/jump handlers in Task 3+; allow dead_code until then.
    #[allow(dead_code)]
    pub(crate) fn refill_prefetch(&mut self, bus: &mut impl crate::Bus) {
        self.refill_prefetch_dyn(bus);
    }

    /// Fills the prefetch queue from the current PC.
    ///
    /// Public because drivers and benchmarks that set PC directly, rather than
    /// going through [`M68k::reset`], must prime the queue before stepping.
    pub fn prime_prefetch(&mut self, bus: &mut impl crate::Bus) {
        self.refill_prefetch_dyn(bus);
    }

    /// Performs a CPU reset: supervisor mode, SSP and PC from vectors 0 and 1.
    pub fn reset(&mut self, bus: &mut impl crate::Bus) {
        self.sr = SR_S;
        let ssp = ((bus.read16(0) as u32) << 16) | bus.read16(2) as u32;
        let pc = ((bus.read16(4) as u32) << 16) | bus.read16(6) as u32;
        self.ssp = ssp;
        self.a[7] = ssp;
        self.pc = pc;
        self.halted = false;
        self.stopped = false;
        self.pending_irq = 0;
        self.in_exception = false;
        self.refill_prefetch_dyn(bus);
    }

    /// Raises an interrupt at `level` (0 clears, 1-7 are IPL).
    pub fn set_irq(&mut self, level: u8) {
        self.pending_irq = level & 7;
    }

    /// Executes one instruction, returning the cycles it consumed.
    pub fn step_with(&mut self, dec: &crate::decode::Decoder, bus: &mut impl crate::Bus) -> u32 {
        // A halted CPU is dead until reset, but still burns time.
        if self.halted {
            return 4;
        }
        if self.stopped {
            if self.pending_irq == 0 {
                return 4;
            }
            self.stopped = false;
        }
        // Peek at the opcode without touching the queue.
        // Real instruction handlers call `consume_opcode` as their first act
        // to shift the queue and refill slot 1 from PC; exception-aborting
        // instructions (illegal opcode, Line-A, Line-F, …) never do, so the
        // queue is left to be overwritten by `refill_prefetch_dyn` at vector
        // dispatch.  This matches the 68000 pipeline-abort behavior.
        let op = self.prefetch[0];
        dec.dispatch(op)(self, bus, op)
    }

    /// Convenience wrapper owning a lazily-built decoder. Requires `std`;
    /// `no_std` callers use [`M68k::step_with`] with their own `Decoder`.
    #[cfg(feature = "std")]
    pub fn step(&mut self, bus: &mut impl crate::Bus) -> u32 {
        use std::sync::OnceLock;
        static DEC: OnceLock<crate::decode::Decoder> = OnceLock::new();
        self.step_with(DEC.get_or_init(crate::decode::Decoder::new), bus)
    }
}

/// A flat 64 KB `Bus` for unit tests, shared by every module's tests.
///
/// Lives in the crate rather than a `tests/` file because the modules that need
/// it are testing crate-internal behavior.
#[cfg(test)]
pub(crate) mod tests_support {
    pub struct FlatBus {
        pub mem: Vec<u8>,
    }

    impl Default for FlatBus {
        fn default() -> Self {
            Self::new()
        }
    }

    impl FlatBus {
        pub fn new() -> Self {
            Self {
                mem: vec![0; 0x10000],
            }
        }

        pub fn put16(&mut self, addr: u32, val: u16) {
            let [hi, lo] = val.to_be_bytes();
            let a = (addr & 0xFFFF) as usize;
            self.mem[a] = hi;
            self.mem[a + 1] = lo;
        }

        /// Loads consecutive words starting at `addr`, for hand-assembled
        /// programs.
        pub fn load(&mut self, addr: u32, words: &[u16]) {
            for (i, w) in words.iter().enumerate() {
                self.put16(addr + (i as u32) * 2, *w);
            }
        }
    }

    impl crate::Bus for FlatBus {
        fn read8(&mut self, addr: u32) -> u8 {
            self.mem[(addr & 0xFFFF) as usize]
        }
        fn read16(&mut self, addr: u32) -> u16 {
            let a = (addr & 0xFFFE) as usize;
            u16::from_be_bytes([self.mem[a], self.mem[a + 1]])
        }
        fn write8(&mut self, addr: u32, val: u8) {
            self.mem[(addr & 0xFFFF) as usize] = val;
        }
        fn write16(&mut self, addr: u32, val: u16) {
            let [hi, lo] = val.to_be_bytes();
            let a = (addr & 0xFFFE) as usize;
            self.mem[a] = hi;
            self.mem[a + 1] = lo;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::FlatBus;
    use super::*;

    #[test]
    fn set_sr_swaps_stack_pointers_when_leaving_supervisor() {
        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x0010_0000; // active SSP
        cpu.usp = 0x0020_0000;

        cpu.set_sr(0); // drop to user mode

        assert_eq!(cpu.ssp, 0x0010_0000, "old SSP must be saved");
        assert_eq!(cpu.a[7], 0x0020_0000, "USP must become active");
    }

    #[test]
    fn set_sr_masks_nonexistent_bits() {
        let mut cpu = M68k::new();
        cpu.set_sr(0xFFFF);
        assert_eq!(cpu.sr, SR_MASK);
    }

    #[test]
    fn ccr_roundtrip() {
        let mut cpu = M68k::new();
        cpu.set_ccr(true, false, true, false, true);
        assert!(cpu.ccr_x() && cpu.ccr_z() && cpu.ccr_c());
        assert!(!cpu.ccr_n() && !cpu.ccr_v());
    }

    #[test]
    fn fetch_word_drains_and_refills_the_queue() {
        let mut bus = FlatBus::new();
        bus.put16(0x1004, 0x3333);
        bus.put16(0x1006, 0x4444);

        let mut cpu = M68k::new();
        cpu.pc = 0x1004;
        cpu.prefetch = [0x1111, 0x2222];

        assert_eq!(cpu.fetch_word(&mut bus), 0x1111);
        assert_eq!(cpu.prefetch, [0x2222, 0x3333]);
        assert_eq!(cpu.pc, 0x1006);

        assert_eq!(cpu.fetch_word(&mut bus), 0x2222);
        assert_eq!(cpu.prefetch, [0x3333, 0x4444]);
        assert_eq!(cpu.pc, 0x1008);
    }

    #[test]
    fn fetch_long_takes_two_words_big_endian() {
        let mut bus = FlatBus::new();
        bus.load(0x1004, &[0x3333, 0x4444]);
        let mut cpu = M68k::new();
        cpu.pc = 0x1004;
        cpu.prefetch = [0x1111, 0x2222];
        assert_eq!(cpu.fetch_long(&mut bus), 0x1111_2222);
    }

    #[test]
    fn reset_loads_ssp_and_pc_from_vectors_and_fills_prefetch() {
        let mut bus = FlatBus::new();
        bus.load(0x0000, &[0x0000, 0x2000, 0x0000, 0x1000]); // SSP, then PC
        bus.load(0x1000, &[0x4E71, 0x4E71]); // two NOPs

        let mut cpu = M68k::new();
        cpu.reset(&mut bus);

        assert_eq!(cpu.a[7], 0x2000);
        assert_eq!(cpu.ssp, 0x2000);
        assert!(cpu.sr_s(), "reset enters supervisor mode");
        assert_eq!(cpu.prefetch, [0x4E71, 0x4E71]);
        assert_eq!(cpu.pc, 0x1004, "pc sits 4 past the first instruction");
    }
}
