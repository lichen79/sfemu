//! The Z80's registers and its step entry point.

use crate::Bus;

/// A Z80 CPU.
///
/// Every field the vector suite's state block carries, and nothing else. The
/// suite compares 26 fields on each of 1,604,000 cases, so a field that exists
/// here but not there is unverified and a field there but not here is a test
/// that cannot pass.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Z80 {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    /// Interrupt vector base, the high byte of an IM 2 vector.
    pub i: u8,
    /// Memory refresh counter. Bit 7 is preserved across increments; the low 7
    /// bits advance once per M1 cycle, which is observable via `LD A,R`.
    pub r: u8,
    pub ix: u16,
    pub iy: u16,
    pub sp: u16,
    pub pc: u16,
    /// The internal address latch.
    ///
    /// Not architecturally visible and absent from most documentation, but it
    /// drives the address pins during some T-states, so the suite compares it.
    pub wz: u16,
    /// The shadow `AF`, as a packed pair — `EX AF,AF'` swaps it wholesale, so
    /// there is no reason to hold two more bytes.
    pub af_: u16,
    /// The shadow `BC`; see [`Z80::af_`].
    pub bc_: u16,
    /// The shadow `DE`; see [`Z80::af_`].
    pub de_: u16,
    /// The shadow `HL`; see [`Z80::af_`].
    pub hl_: u16,
    pub iff1: bool,
    pub iff2: bool,
    /// Interrupt mode: 0, 1, or 2.
    pub im: u8,
    /// Set when `EI` executed and the enable has not yet taken effect.
    ///
    /// `EI` does not enable interrupts until *after* the following instruction,
    /// so an `EI`/`RET` pair cannot be interrupted between them. This is that
    /// one-instruction delay, and the suite carries it as a state field.
    pub ei: u8,
    /// Whether the last instruction wrote the flags.
    ///
    /// `SCF` and `CCF` compute F3/F5 from `A` **or** from the previous flags
    /// depending on this. It is the most commonly omitted piece of Z80 state and
    /// the suite compares it on every case.
    pub q: u8,
    /// Whether the last instruction was `LD A,I` or `LD A,R`.
    pub p: u8,
    /// Whether a `HALT` is in effect.
    ///
    /// Not one of the suite's 26 fields: its cases are single instructions and
    /// none begins halted, so nothing in the vectors can observe this. It exists
    /// because D2 runs the core continuously, where a halted CPU that forgot it
    /// would execute whatever follows the `HALT` instead of waiting.
    pub halted: bool,
}

impl Z80 {
    /// A CPU in its power-on state.
    #[must_use]
    pub fn new() -> Self {
        let mut c = Self::default();
        c.reset();
        c
    }

    /// Reset, per the Zilog manual: PC, I and R cleared, interrupts disabled,
    /// mode 0, and **AF and SP all ones** — not zero.
    pub fn reset(&mut self) {
        self.pc = 0;
        self.i = 0;
        self.r = 0;
        self.iff1 = false;
        self.iff2 = false;
        self.im = 0;
        self.ei = 0;
        self.q = 0;
        self.p = 0;
        self.wz = 0;
        self.halted = false;
        self.set_af(0xFFFF);
        self.sp = 0xFFFF;
    }

    /// An M1 opcode fetch: read at `PC`, advance `PC`, bump `R`.
    ///
    /// Every opcode byte and every prefix byte goes through here, because `R`
    /// counts M1 cycles rather than instructions — a `DD CB __ 06` bumps it twice,
    /// once for each prefix, and the vectors check that.
    pub fn fetch<B: Bus>(&mut self, bus: &mut B) -> u8 {
        let op = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        self.bump_r();
        op
    }

    /// Reads the byte at `PC` and advances, without touching `R`.
    ///
    /// Operands are not M1 cycles. Using [`Self::fetch`] for a displacement byte
    /// is a bug the vectors catch on every indexed instruction.
    pub fn imm<B: Bus>(&mut self, bus: &mut B) -> u8 {
        let v = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    /// Reads a little-endian 16-bit operand.
    pub fn imm16<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.imm(bus);
        let hi = self.imm(bus);
        u16::from(hi) << 8 | u16::from(lo)
    }

    /// Advances the refresh counter: seven bits, with bit 7 held.
    ///
    /// The Z80's `R` is a 7-bit counter and bit 7 is whatever was last written by
    /// `LD R,A`. A plain increment would clear it once every 128 instructions.
    pub(crate) fn bump_r(&mut self) {
        self.r = (self.r & 0x80) | (self.r.wrapping_add(1) & 0x7F);
    }

    /// Executes one instruction and returns the T-states it took.
    ///
    /// The count is not derived from a table: each handler returns its own, and
    /// the vectors' `cycles` array is the authority those numbers were taken from.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        if self.halted {
            // A halted CPU runs NOPs: it burns T-states and bumps R (the refresh
            // cycles continue, which is what kept DRAM alive) but does not fetch.
            self.bump_r();
            return 4;
        }
        // `EI`'s one-instruction delay expires at the *start* of the next
        // instruction, so the pending mark is cleared before it runs rather than
        // after — an interrupt check between the two would otherwise see the wrong
        // state.
        let was_pending = self.ei;
        let op = self.fetch(bus);
        let t = crate::decode::execute(self, bus, op);
        if was_pending != 0 && self.ei == was_pending {
            self.ei = 0;
        }
        t
    }

    /// `BC` as one 16-bit value: `B` high, `C` low.
    #[must_use]
    pub fn bc(&self) -> u16 {
        u16::from(self.b) << 8 | u16::from(self.c)
    }

    /// `DE`: `D` high, `E` low.
    #[must_use]
    pub fn de(&self) -> u16 {
        u16::from(self.d) << 8 | u16::from(self.e)
    }

    /// `HL`: `H` high, `L` low.
    #[must_use]
    pub fn hl(&self) -> u16 {
        u16::from(self.h) << 8 | u16::from(self.l)
    }

    /// `AF`: `A` high, `F` low.
    #[must_use]
    pub fn af(&self) -> u16 {
        u16::from(self.a) << 8 | u16::from(self.f)
    }

    /// Splits `v` into `B` and `C`.
    pub fn set_bc(&mut self, v: u16) {
        self.b = (v >> 8) as u8;
        self.c = v as u8;
    }

    /// Splits `v` into `D` and `E`.
    pub fn set_de(&mut self, v: u16) {
        self.d = (v >> 8) as u8;
        self.e = v as u8;
    }

    /// Splits `v` into `H` and `L`.
    pub fn set_hl(&mut self, v: u16) {
        self.h = (v >> 8) as u8;
        self.l = v as u8;
    }

    /// Splits `v` into `A` and `F`.
    pub fn set_af(&mut self, v: u16) {
        self.a = (v >> 8) as u8;
        self.f = v as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags;

    /// A 64 KiB bus for the core's own tests.
    ///
    /// Deliberately not the test harness's recording bus: that lives in
    /// `testrunner`, which depends on this crate. These tests must run under
    /// `cargo test -p z80` with nothing else built.
    struct Mem {
        ram: [u8; 0x1_0000],
        ports_out: Vec<(u16, u8)>,
        port_in_value: u8,
    }

    impl Mem {
        fn at(pc: u16, prog: &[u8]) -> Self {
            let mut m = Mem {
                ram: [0; 0x1_0000],
                ports_out: Vec::new(),
                port_in_value: 0xFF,
            };
            for (i, b) in prog.iter().enumerate() {
                m.ram[usize::from(pc) + i] = *b;
            }
            m
        }
    }

    impl Bus for Mem {
        fn read(&mut self, addr: u16) -> u8 {
            self.ram[usize::from(addr)]
        }
        fn write(&mut self, addr: u16, val: u8) {
            self.ram[usize::from(addr)] = val;
        }
        fn port_in(&mut self, _port: u16) -> u8 {
            self.port_in_value
        }
        fn port_out(&mut self, port: u16, val: u8) {
            self.ports_out.push((port, val));
        }
    }

    /// The 16-bit pairs are the 8-bit registers, high byte first.
    ///
    /// Hand-written from the architecture, not read back from a setter: `B` is the
    /// high half of `BC` on a Z80, and a core that swapped them would still be
    /// self-consistent under `set_bc(bc())` — which is why that identity is not
    /// what is asserted here.
    #[test]
    fn a_register_pair_is_its_two_halves_high_byte_first() {
        let mut c = Z80::new();
        c.b = 0x12;
        c.c = 0x34;
        assert_eq!(c.bc(), 0x1234);
        c.d = 0xAB;
        c.e = 0xCD;
        assert_eq!(c.de(), 0xABCD);
        c.h = 0x00;
        c.l = 0xFF;
        assert_eq!(c.hl(), 0x00FF);
        c.a = 0x7F;
        c.f = 0x80;
        assert_eq!(c.af(), 0x7F80, "A is the high half of AF, F the low");

        // And the setters land in the halves, in the same order.
        c.set_bc(0xDEAD);
        assert_eq!((c.b, c.c), (0xDE, 0xAD));
        c.set_hl(0xBEEF);
        assert_eq!((c.h, c.l), (0xBE, 0xEF));
        c.set_af(0x1234);
        assert_eq!((c.a, c.f), (0x12, 0x34));
    }

    /// Reset, per the Zilog manual: PC and I and R cleared, interrupts disabled,
    /// interrupt mode 0, and **AF and SP set to 0xFFFF**.
    ///
    /// The 0xFFFF is the detail worth a test: it is not zero, and a core that
    /// zeroed everything would boot differently on the first `PUSH`.
    #[test]
    fn reset_clears_the_program_counter_and_leaves_af_and_sp_all_ones() {
        let mut c = Z80::new();
        c.pc = 0x1234;
        c.i = 0x55;
        c.r = 0x7F;
        c.iff1 = true;
        c.iff2 = true;
        c.im = 2;
        c.reset();
        assert_eq!(c.pc, 0, "PC clear");
        assert_eq!(c.i, 0, "I clear");
        assert_eq!(c.r, 0, "R clear");
        assert!(!c.iff1, "interrupts disabled");
        assert!(!c.iff2);
        assert_eq!(c.im, 0, "interrupt mode 0");
        assert_eq!(c.af(), 0xFFFF, "AF is all ones after reset, not zero");
        assert_eq!(c.sp, 0xFFFF, "and so is SP");
    }

    /// `Q` and `P` are real state, not derived.
    ///
    /// Both are fields of the suite's state block, so they are compared on every
    /// one of 1,604,000 cases. `Q` records whether the last instruction wrote the
    /// flags (which `SCF`/`CCF` need for F3/F5) and `P` whether `LD A,I`/`LD A,R`
    /// was last. A core without them cannot pass `37.json` (`SCF`).
    #[test]
    fn the_undocumented_state_exists_and_starts_clear() {
        let c = Z80::new();
        assert_eq!(c.q, 0, "no instruction has run, so no flags were written");
        assert_eq!(c.p, 0);
        assert_eq!(c.wz, 0, "the internal address latch");
    }

    /// `NOP` is four T-states and moves only `PC` and `R`.
    ///
    /// The `R` increment is the part worth asserting: it is observable through
    /// `LD A,R`, the suite compares it on every case, and a core that forgot it
    /// would pass any test that only looked at `PC`.
    #[test]
    fn nop_takes_four_t_states_and_bumps_the_refresh_counter() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.r = 0x7E;
        let mut m = Mem::at(0x100, &[0x00]);
        assert_eq!(c.step(&mut m), 4, "NOP is 4 T-states");
        assert_eq!(c.pc, 0x101);
        assert_eq!(c.r, 0x7F, "one M1 cycle, one refresh increment");
    }

    /// `R`'s bit 7 survives the increment; only the low seven bits count.
    ///
    /// A 7-bit counter with a sticky top bit, per the Zilog manual. `LD A,R` after
    /// 128 instructions is how software sees the difference, and a plain
    /// `wrapping_add(1)` would clear bit 7 exactly once every 128 instructions —
    /// rare enough to look like a mystery later.
    #[test]
    fn the_refresh_counter_wraps_in_seven_bits_and_keeps_bit_seven() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.r = 0xFF;
        let mut m = Mem::at(0x100, &[0x00]);
        c.step(&mut m);
        assert_eq!(c.r, 0x80, "0x7F wrapped to 0x00, bit 7 untouched");
        c.r = 0x7F;
        c.pc = 0x100;
        c.step(&mut m);
        assert_eq!(c.r, 0x00, "and with bit 7 clear it stays clear");
    }

    /// `HALT` advances `PC` past its own opcode, then stops advancing.
    ///
    /// Both halves matter, and they are easy to conflate. The M1 fetch advances `PC`
    /// to `0x101` like any other opcode — the vectors show that. What `halted` changes
    /// is every step *after*: the CPU burns 4 T-states and `PC` stays put, rather than
    /// running into whatever follows the `HALT`.
    ///
    /// `PC` sitting past the opcode is also what makes interrupt acceptance correct
    /// (Task 13): the pushed return address is `0x101`, so `RETI` resumes after the
    /// `HALT` instead of halting again forever. `76.json` is the vector file for this.
    #[test]
    fn halt_stays_on_its_own_instruction() {
        let mut c = Z80::new();
        c.pc = 0x100;
        // A byte that is *not* a NOP follows, so a CPU that ran on would be caught
        // by the T-state count as well as by PC.
        let mut m = Mem::at(0x100, &[0x76, 0x37]);
        assert_eq!(c.step(&mut m), 4);
        assert_eq!(c.pc, 0x101, "the fetch advanced PC past the opcode");
        assert!(c.halted, "and the CPU is now halted");
        // The next step re-executes the HALT: PC does not run away.
        let before = c.pc;
        let r_before = c.r;
        assert_eq!(c.step(&mut m), 4, "a halted CPU still burns T-states");
        assert_eq!(c.pc, before, "and does not advance");
        assert_ne!(c.r, r_before, "but the refresh cycles keep running");
    }

    /// `SCF` with `Q` set takes F3/F5 from `A`.
    ///
    /// Hand-computed: the previous instruction wrote the flags, so F3/F5 come from
    /// `A` alone. `A = 0x28` has bits 5 and 3 set, so both appear; `A = 0x00` has
    /// neither, so both clear even though the old `f` had them.
    #[test]
    fn scf_after_a_flag_writing_instruction_takes_f3_and_f5_from_a() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x28;
        c.f = 0x00;
        c.q = 1;
        let mut m = Mem::at(0x100, &[0x37]);
        c.step(&mut m);
        assert_eq!(c.f & flags::C, flags::C, "SCF sets carry");
        assert_eq!(c.f & (flags::H | flags::N), 0, "and clears H and N");
        assert_eq!(
            c.f & (flags::F5 | flags::F3),
            flags::F5 | flags::F3,
            "from A"
        );

        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x00;
        c.f = flags::F5 | flags::F3;
        c.q = 1;
        c.step(&mut m);
        assert_eq!(
            c.f & (flags::F5 | flags::F3),
            0,
            "A has neither, so neither survives"
        );
    }

    /// `SCF` with `Q` clear ORs F3/F5 with the previous flags.
    ///
    /// The trap. The previous instruction did not write the flags, so the old F3/F5
    /// stay set even though `A` has neither bit — a core that always read `A` would
    /// clear them and fail `37.json` on exactly the cases where the preceding
    /// instruction was itself an `SCF`.
    #[test]
    fn scf_after_a_non_flag_writing_instruction_ors_with_the_old_flags() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x00;
        c.f = flags::F5 | flags::F3;
        c.q = 0;
        let mut m = Mem::at(0x100, &[0x37]);
        c.step(&mut m);
        assert_eq!(
            c.f & (flags::F5 | flags::F3),
            flags::F5 | flags::F3,
            "Q clear means the old bits are ORed in, not replaced"
        );
        assert_eq!(c.f & flags::C, flags::C);
    }

    /// `SCF` and `CCF` leave S, Z and P/V alone.
    ///
    /// From the Zilog manual: of the eight bits, `SCF` defines C, H and N and leaves
    /// the rest — so a mask that rebuilt `F` from S and Z only would silently clear
    /// P/V. That is invisible in every test above, because they all start with P/V
    /// clear.
    #[test]
    fn scf_and_ccf_preserve_sign_zero_and_parity() {
        for op in [0x37u8, 0x3F] {
            let mut c = Z80::new();
            c.pc = 0x100;
            c.a = 0;
            c.f = flags::S | flags::Z | flags::PV;
            c.q = 1;
            let mut m = Mem::at(0x100, &[op]);
            c.step(&mut m);
            assert_eq!(
                c.f & (flags::S | flags::Z | flags::PV),
                flags::S | flags::Z | flags::PV,
                "{op:#04X} must not disturb S, Z or P/V"
            );
        }
    }

    /// `CCF` complements carry and puts the old carry in H.
    ///
    /// From the manual: H receives the carry's previous value, N is cleared, and
    /// F3/F5 follow the same `Q` rule as `SCF`.
    #[test]
    fn ccf_complements_carry_and_saves_the_old_one_in_h() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0;
        c.f = flags::C;
        c.q = 1;
        let mut m = Mem::at(0x100, &[0x3F]);
        c.step(&mut m);
        assert_eq!(c.f & flags::C, 0, "carry was set, now clear");
        assert_eq!(c.f & flags::H, flags::H, "and H holds the old carry");
        assert_eq!(c.f & flags::N, 0);

        let mut c = Z80::new();
        c.pc = 0x100;
        c.f = 0;
        c.q = 1;
        c.step(&mut m);
        assert_eq!(c.f & flags::C, flags::C, "carry was clear, now set");
        assert_eq!(
            c.f & flags::H,
            0,
            "and H holds the old carry, which was clear"
        );
    }

    /// `CPL` inverts `A` and sets H and N, leaving carry alone.
    #[test]
    fn cpl_inverts_a_and_sets_h_and_n() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x3B;
        c.f = flags::C;
        let mut m = Mem::at(0x100, &[0x2F]);
        c.step(&mut m);
        assert_eq!(c.a, 0xC4, "0x3B inverted");
        assert_eq!(c.f & (flags::H | flags::N), flags::H | flags::N);
        assert_eq!(c.f & flags::C, flags::C, "carry is untouched");

        // F5 and F3 come from the *result*, and 0xC4 has neither bit — so that case
        // alone cannot tell a copy from a clear. These two can, one bit each.
        for (a, want) in [(0xDFu8, flags::F5), (0xF7, flags::F3)] {
            let mut c = Z80::new();
            c.pc = 0x100;
            c.a = a;
            c.f = 0;
            c.step(&mut m);
            assert_eq!(
                c.f & (flags::F5 | flags::F3),
                want,
                "CPL of {a:#04X} gives {:#04X}, whose F5/F3 are {want:#04X}",
                !a
            );
        }
    }

    /// `CPL` leaves S, Z and P/V alone.
    ///
    /// The manual defines `CPL` as setting H and N and touching nothing else. Every
    /// other `CPL` assertion holds with P/V wrongly cleared.
    #[test]
    fn cpl_preserves_sign_zero_and_parity() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x3B;
        c.f = flags::S | flags::Z | flags::PV;
        let mut m = Mem::at(0x100, &[0x2F]);
        c.step(&mut m);
        assert_eq!(
            c.f & (flags::S | flags::Z | flags::PV),
            flags::S | flags::Z | flags::PV
        );
    }

    /// `DAA` after an addition, computed by hand from the manual's table.
    ///
    /// 0x09 + 0x08 = 0x11 in binary; as BCD it should be 0x17. The low nibble is
    /// 1 with H set from the ALU, so DAA adds 0x06: 0x11 + 0x06 = 0x17.
    #[test]
    fn daa_corrects_a_bcd_addition() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x11;
        c.f = flags::H; // as ADD 0x09,0x08 would have left it
        let mut m = Mem::at(0x100, &[0x27]);
        c.step(&mut m);
        assert_eq!(c.a, 0x17, "0x09 + 0x08 is 0x17 in BCD");
        assert_eq!(c.f & flags::C, 0, "no decimal carry out");
        assert_eq!(c.f & flags::N, 0, "N is preserved, and it was clear");
    }

    /// `DAA` after a subtraction subtracts instead of adding.
    ///
    /// N distinguishes them, which is the only reason N exists. 0x10 - 0x01 is
    /// 0x0F in binary with H set; as BCD the answer is 0x09, so DAA subtracts 0x06.
    #[test]
    fn daa_corrects_a_bcd_subtraction_because_n_is_set() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x0F;
        c.f = flags::N | flags::H;
        let mut m = Mem::at(0x100, &[0x27]);
        c.step(&mut m);
        assert_eq!(c.a, 0x09, "0x10 - 0x01 is 0x09 in BCD");
        assert_eq!(c.f & flags::N, flags::N, "N survives DAA");
    }

    /// `DAA` carries out of the high nibble when `A` exceeds 0x99.
    #[test]
    fn daa_sets_carry_when_the_high_nibble_overflows() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x9A;
        c.f = 0;
        let mut m = Mem::at(0x100, &[0x27]);
        c.step(&mut m);
        assert_eq!(c.a, 0x00, "0x9A corrects to 0x100, truncated");
        assert_eq!(c.f & flags::C, flags::C, "with a carry out");
        assert_eq!(c.f & flags::Z, flags::Z, "and the result is zero");
    }

    /// `DAA` adds 0x60 when carry is set even though `A` is in range.
    ///
    /// The carry-in case, which the three tests above do not reach: `A = 0x11` with
    /// C set is the low byte of a two-digit-plus-carry addition, and BCD correction
    /// must bring it to 0x71 with the carry still out.
    #[test]
    fn daa_honours_an_incoming_carry() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x11;
        c.f = flags::C;
        let mut m = Mem::at(0x100, &[0x27]);
        c.step(&mut m);
        assert_eq!(c.a, 0x71, "carry in means the high nibble is corrected");
        assert_eq!(c.f & flags::C, flags::C, "and the carry stays out");
    }

    /// `EI` does not enable interrupts until after the next instruction.
    ///
    /// The `ei` field is that pending state, and an `EI`/`RET` pair being
    /// uninterruptible is what it protects.
    #[test]
    fn ei_defers_the_enable_by_one_instruction() {
        let mut c = Z80::new();
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xFB, 0x00]);
        c.step(&mut m);
        assert!(c.iff1, "EI sets IFF1 immediately");
        assert_eq!(c.ei, 1, "and marks the enable as still pending");
        c.step(&mut m);
        assert_eq!(c.ei, 0, "the next instruction clears the pending mark");
    }

    /// `DI` clears both interrupt flip-flops at once.
    #[test]
    fn di_clears_both_flip_flops() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.iff1 = true;
        c.iff2 = true;
        let mut m = Mem::at(0x100, &[0xF3]);
        c.step(&mut m);
        assert!(!c.iff1 && !c.iff2);
    }

    /// `Q` is set by instructions that write flags and cleared by those that do not.
    ///
    /// The protocol the whole `SCF`/`CCF` rule depends on. `NOP` writes no flags,
    /// `SCF` does — so `Q` must differ after them, and a core that set `Q`
    /// unconditionally would pass the `SCF` tests above and fail the suite.
    #[test]
    fn q_records_whether_the_instruction_wrote_the_flags() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.q = 1;
        let mut m = Mem::at(0x100, &[0x00, 0x37]);
        c.step(&mut m);
        assert_eq!(c.q, 0, "NOP writes no flags");
        c.step(&mut m);
        assert_eq!(c.q, 1, "SCF does");
    }

    /// An operand read is not an M1 cycle, so it does not bump `R`.
    ///
    /// [`Z80::fetch`] and [`Z80::imm`] differ in exactly this, and using the wrong
    /// one for a displacement byte is a bug the vectors catch on every indexed
    /// instruction — which is why it is worth pinning before those tasks arrive.
    #[test]
    fn an_immediate_operand_does_not_bump_the_refresh_counter() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.r = 0x10;
        let mut m = Mem::at(0x100, &[0xAA, 0x34, 0x12]);
        assert_eq!(c.fetch(&mut m), 0xAA, "an opcode fetch reads and advances");
        assert_eq!(c.r, 0x11, "and counts as an M1 cycle");
        assert_eq!(c.imm16(&mut m), 0x1234, "little-endian: low byte first");
        assert_eq!(c.pc, 0x103);
        assert_eq!(c.r, 0x11, "two operand bytes, no M1 cycles");
    }
}
