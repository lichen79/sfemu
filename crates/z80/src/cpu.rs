//! The Z80's registers and its step entry point.

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
        self.set_af(0xFFFF);
        self.sp = 0xFFFF;
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
}
