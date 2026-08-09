//! Interrupt acceptance: NMI, the three maskable modes, and the `EI` delay.
//!
//! **Nothing in the vector suite reaches this file.** Every one of the 1,604,000
//! cases is a single instruction with no interrupt, so the suite pins the `ei`,
//! `iff1`, `iff2` and `im` *fields* — a core that stores them wrongly is caught —
//! but never exercises acceptance. The tests in this module and the `z80int`
//! mutation set are the whole of this file's verification, which is why each rule
//! below names the failure it prevents.
//!
//! The one part of the delay that *is* pinned by the suite lives in
//! [`Z80::step`], not here: `ei` expires at the start of the next instruction and
//! must not be promoted into the flip-flops when it does. See the measurement in
//! that function.

use crate::{Bus, Z80};

impl Z80 {
    /// Services whichever request outranks the other, or neither.
    ///
    /// Returns the T-states consumed, `0` when nothing was accepted. NMI wins
    /// against a simultaneous maskable request, and the maskable one stays pending —
    /// it is level-sensitive, so the device is still holding the line.
    pub fn service<B: Bus>(&mut self, bus: &mut B) -> u32 {
        if let Some(t) = self.ack_nmi(bus) {
            return t;
        }
        self.ack_irq(bus).unwrap_or(0)
    }

    /// Accepts a non-maskable interrupt if one is pending.
    ///
    /// NMI ignores `IFF1` — that is what non-maskable means — and **saves `IFF1`
    /// into `IFF2`** before clearing it. `RETN` copies it back. Without that save an
    /// NMI would permanently disable maskable interrupts, so the machine would work
    /// until the first NMI and then go quiet.
    pub fn ack_nmi<B: Bus>(&mut self, bus: &mut B) -> Option<u32> {
        if !self.nmi {
            return None;
        }
        self.nmi = false; // edge-triggered, unlike `irq`
        self.leave_halt();
        self.iff2 = self.iff1;
        self.iff1 = false;
        // The handler starts with no arming outstanding. Not observable in the
        // vectors — nothing there accepts an interrupt — but leaving a stale mark
        // would make the handler's first instruction refuse a maskable request for
        // no reason a reader of the handler could see.
        self.ei = 0;
        crate::ops::load::push(self, bus, self.pc);
        self.pc = 0x0066;
        Some(11)
    }

    /// Accepts a maskable interrupt if one is pending and unmasked.
    ///
    /// Returns `None` without touching `self.irq` when the request is refused: the
    /// line is level-sensitive and the device holds it until acknowledged. Clearing
    /// it here would drop interrupts intermittently — a bug that presents as
    /// occasionally missing sound rather than as a failure.
    pub fn ack_irq<B: Bus>(&mut self, bus: &mut B) -> Option<u32> {
        // `ei` is the one-instruction arming delay: `EI` sets it, and the *next*
        // instruction clears it. While it is set the enable is one instruction old
        // and not yet in force, so `EI; RETI` cannot re-enter its own ISR.
        if !self.irq || !self.iff1 || self.ei != 0 {
            return None;
        }
        self.irq = false;
        self.leave_halt();
        self.iff1 = false;
        self.iff2 = false;
        let vector = bus.irq_ack();
        match self.im {
            0 => {
                // The bus byte is an opcode. In practice it is always an `RST`, and
                // that is the only form this implements — executing an arbitrary
                // opcode from the acknowledge cycle is a path no board uses and the
                // suite cannot check.
                let target = u16::from(vector & 0x38);
                crate::ops::load::push(self, bus, self.pc);
                self.pc = target;
                Some(13)
            }
            1 => {
                crate::ops::load::push(self, bus, self.pc);
                self.pc = 0x0038;
                Some(13)
            }
            _ => {
                // The table holds 16-bit entries, so the low bit is masked: an odd
                // bus value would read half of one vector and half of the next.
                let addr = u16::from(self.i) << 8 | u16::from(vector & 0xFE);
                crate::ops::load::push(self, bus, self.pc);
                let lo = bus.read(addr);
                let hi = bus.read(addr.wrapping_add(1));
                self.pc = u16::from(hi) << 8 | u16::from(lo);
                Some(19)
            }
        }
    }

    /// Leaves the halted state.
    ///
    /// `PC` is **already** past the `HALT`: the instruction advanced it and then
    /// stopped advancing. So there is nothing to adjust here, and adjusting would be
    /// the bug — the ISR would return into the `HALT` and the machine would freeze
    /// the first time an interrupt arrived in an idle loop, which is where a sound
    /// CPU spends most of its time.
    fn leave_halt(&mut self) {
        self.halted = false;
    }
}

#[cfg(test)]
mod tests {
    use crate::testbus::Mem;
    use crate::Z80;

    /// `EI` arms interrupts for *after* the next instruction.
    ///
    /// So `EI; RET` cannot be interrupted between the two — which is why an ISR can
    /// end `EI; RETI` without re-entering itself. Getting this wrong produces a stack
    /// overflow under load and nothing at all when idle.
    #[test]
    fn ei_does_not_let_an_interrupt_in_until_after_the_next_instruction() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.sp = 0x3000;
        c.im = 1;
        let mut m = Mem::at(0x100, &[0xFB, 0x00, 0x00]); // EI; NOP; NOP
        c.step(&mut m); // EI
        assert_eq!(c.ei, 1, "armed, not enabled");
        c.irq = true;
        assert_eq!(
            c.ack_irq(&mut m),
            None,
            "the instruction after EI is protected"
        );
        c.step(&mut m); // NOP
        assert_eq!(c.ei, 0, "the arming is consumed");
        assert_eq!(c.ack_irq(&mut m), Some(13), "and now the interrupt lands");
        assert_eq!(c.pc, 0x0038, "mode 1 vectors to 0x38");
        assert_eq!(m.ram[0x2FFF], 0x01, "the return address is the second NOP");
        assert_eq!(m.ram[0x2FFE], 0x02);
    }

    /// The expiring arming must not be promoted into the flip-flops.
    ///
    /// `EI; DI` is the case that tells the two readings apart, and neither the test
    /// above nor any other in this module can: a core that treated "`ei` was set on
    /// entry" as "now enable interrupts" would re-enable them *after* the `DI` had
    /// just turned them off, and an ISR ending `DI; RET` would be re-entered.
    ///
    /// The suite pins this too, which is how the error was caught: `f3` ends with
    /// both flip-flops clear on 1,000 of 1,000 cases, 544 of which begin with `ei`
    /// set. Across all 1,518,000 cases outside the `EI`/`DI`/`RETN` pages, promoting
    /// disagrees with the recorded final state on 569,245 of the 759,299 that begin
    /// armed.
    #[test]
    fn the_expiring_ei_arming_does_not_re_enable_interrupts_behind_a_di() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.im = 1;
        let mut m = Mem::at(0x100, &[0xFB, 0xF3]); // EI; DI
        c.step(&mut m); // EI: both flip-flops set now, arming pending
        assert!(c.iff1 && c.iff2);
        c.step(&mut m); // DI, with the arming expiring on this very instruction
        assert_eq!(c.ei, 0, "the arming expired");
        assert!(
            !c.iff1,
            "and DI won -- the arming carried no enable of its own"
        );
        assert!(!c.iff2);
        c.irq = true;
        assert_eq!(c.ack_irq(&mut m), None, "so nothing gets in");
    }

    /// A second `EI` re-arms, and does not consume its own arming.
    ///
    /// `EI; EI; NOP` must still protect exactly one instruction after the last `EI`.
    /// This is what fixes the order of the clear in [`Z80::step`]: clearing `ei`
    /// *after* dispatch instead of before would swallow the arming the second `EI`
    /// had just set, and interrupts would never come on at all.
    #[test]
    fn a_second_ei_re_arms_rather_than_consuming_its_own_arming() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.sp = 0x3000;
        c.im = 1;
        let mut m = Mem::at(0x100, &[0xFB, 0xFB, 0x00]); // EI; EI; NOP
        c.step(&mut m);
        c.step(&mut m);
        assert_eq!(c.ei, 1, "the second EI left its own arming standing");
        c.irq = true;
        assert_eq!(
            c.ack_irq(&mut m),
            None,
            "which still protects one instruction"
        );
        c.step(&mut m); // NOP
        assert_eq!(c.ack_irq(&mut m), Some(13));
    }

    /// `DI` takes effect immediately — the asymmetry with `EI` is deliberate.
    #[test]
    fn di_disables_immediately_where_ei_defers() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.iff1 = true;
        c.iff2 = true;
        let mut m = Mem::at(0x100, &[0xF3]);
        c.step(&mut m);
        assert!(!c.iff1);
        assert!(!c.iff2);
        c.irq = true;
        assert_eq!(c.ack_irq(&mut m), None, "no deferral on the way down");
    }

    /// A maskable interrupt clears both flip-flops.
    #[test]
    fn accepting_an_interrupt_clears_both_interrupt_flip_flops() {
        let mut c = Z80::new();
        c.pc = 0x200;
        c.sp = 0x3000;
        c.im = 1;
        c.iff1 = true;
        c.iff2 = true;
        c.irq = true;
        let mut m = Mem::at(0x200, &[0x00]);
        assert_eq!(c.ack_irq(&mut m), Some(13));
        assert!(!c.iff1, "the ISR runs with interrupts off");
        assert!(!c.iff2);
    }

    /// NMI ignores `IFF1`, saves it into `IFF2`, and vectors to 0x66.
    ///
    /// The save is the whole mechanism by which an NMI does not permanently disable
    /// interrupts: `RETN` copies it back. A core that skipped the copy would run
    /// correctly until the first NMI and then never take another maskable interrupt.
    #[test]
    fn nmi_ignores_iff1_but_saves_it_for_retn() {
        let mut c = Z80::new();
        c.pc = 0x200;
        c.sp = 0x3000;
        c.iff1 = true;
        c.iff2 = false;
        c.nmi = true;
        let mut m = Mem::at(0x200, &[0x00]);
        assert_eq!(c.ack_nmi(&mut m), Some(11));
        assert_eq!(c.pc, 0x0066);
        assert!(!c.iff1, "cleared on entry");
        assert!(c.iff2, "and the old value saved here");
        assert!(!c.nmi, "the edge is consumed");

        // RETN puts it back.
        let mut m2 = Mem::at(0x0066, &[0xED, 0x45]);
        m2.ram[0x2FFF] = 0x02;
        m2.ram[0x2FFE] = 0x00;
        c.step(&mut m2);
        assert!(c.iff1, "RETN restores IFF1 from IFF2");
        assert_eq!(c.pc, 0x0200);
    }

    /// NMI outranks a simultaneous maskable interrupt.
    #[test]
    fn an_nmi_wins_against_a_simultaneous_maskable_interrupt() {
        let mut c = Z80::new();
        c.pc = 0x200;
        c.sp = 0x3000;
        c.im = 1;
        c.iff1 = true;
        c.nmi = true;
        c.irq = true;
        let mut m = Mem::at(0x200, &[0x00]);
        assert_eq!(c.service(&mut m), 11);
        assert_eq!(c.pc, 0x0066, "0x66, not 0x38");
        assert!(c.irq, "and the maskable request is still pending");
    }

    /// With nothing asserted, `service` costs nothing and changes nothing.
    ///
    /// The caller runs it every instruction boundary, so a `service` that pushed or
    /// vectored on an idle bus would corrupt the stack on the first idle step rather
    /// than fail visibly. `0` is also how D2 will know not to charge the scheduler.
    #[test]
    fn service_on_an_idle_bus_is_free_and_inert() {
        let mut c = Z80::new();
        c.pc = 0x200;
        c.sp = 0x3000;
        c.im = 1;
        c.iff1 = true;
        let before = c.clone();
        let mut m = Mem::at(0x200, &[0x00]);
        assert_eq!(c.service(&mut m), 0);
        assert_eq!(c, before, "no register moved");
        assert!(m.writes.is_empty(), "and nothing was pushed");
    }

    /// Acceptance leaves `HALT`, and `PC` is past the `HALT` before the push.
    ///
    /// The consequential off-by-one of this whole task: push the `HALT`'s own address
    /// and the ISR returns into it, halting again forever. The machine would boot,
    /// run, and then freeze the first time an interrupt arrived during an idle loop —
    /// which is where a sound CPU spends most of its time.
    #[test]
    fn an_interrupt_leaves_halt_with_pc_past_the_halt_instruction() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.sp = 0x3000;
        c.im = 1;
        c.iff1 = true;
        let mut m = Mem::at(0x100, &[0x76]); // HALT
        c.step(&mut m);
        assert!(c.halted);
        assert_eq!(c.pc, 0x101, "HALT itself advances PC and then holds");
        // A halted CPU keeps consuming T-states.
        assert_eq!(c.step(&mut m), 4, "and keeps ticking while halted");
        assert_eq!(c.pc, 0x101, "without advancing");

        c.irq = true;
        c.ack_irq(&mut m);
        assert!(!c.halted, "acceptance leaves the halt");
        assert_eq!(m.ram[0x2FFF], 0x01, "and pushes 0x101 -- past the HALT");
        assert_eq!(m.ram[0x2FFE], 0x01);
        assert_eq!(c.pc, 0x0038);
    }

    /// An NMI leaves `HALT` too, and by the same rule.
    ///
    /// The two acceptance paths are separate functions, so a fix applied to one and
    /// not the other would leave the machine wedging on NMI only — and CPS-1's sound
    /// board is exactly a CPU that idles in `HALT`.
    #[test]
    fn an_nmi_also_leaves_halt_with_pc_past_the_halt_instruction() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.sp = 0x3000;
        let mut m = Mem::at(0x100, &[0x76]);
        c.step(&mut m);
        assert!(c.halted);
        c.nmi = true;
        assert_eq!(c.ack_nmi(&mut m), Some(11));
        assert!(!c.halted);
        assert_eq!(m.ram[0x2FFF], 0x01);
        assert_eq!(m.ram[0x2FFE], 0x01);
        assert_eq!(c.pc, 0x0066);
    }

    /// Mode 2 masks the vector's low bit.
    ///
    /// The table holds 16-bit entries, so an odd bus value would read a misaligned
    /// pair — half of one vector and half of the next. Devices routinely put an odd
    /// value on the bus, so the mask is load-bearing rather than defensive.
    #[test]
    fn mode_two_masks_the_low_bit_of_the_vector() {
        let mut c = Z80::new();
        c.pc = 0x200;
        c.sp = 0x3000;
        c.im = 2;
        c.i = 0x40;
        c.iff1 = true;
        c.irq = true;
        let mut m = Mem::at(0x200, &[0x00]);
        m.irq_vector = 0x0F; // odd
        m.ram[0x400E] = 0x34; // the entry at 0x400E, not 0x400F
        m.ram[0x400F] = 0x12;
        assert_eq!(c.ack_irq(&mut m), Some(19));
        assert_eq!(c.pc, 0x1234, "read from I<<8 | 0x0E");
    }

    /// Mode 0 executes the byte on the bus; mode 1 is `RST 38h` and ignores it.
    #[test]
    fn mode_zero_executes_the_bus_byte_and_mode_one_ignores_it() {
        // Mode 1: the bus byte is irrelevant.
        let mut c = Z80::new();
        c.pc = 0x200;
        c.sp = 0x3000;
        c.im = 1;
        c.iff1 = true;
        c.irq = true;
        let mut m = Mem::at(0x200, &[0x00]);
        m.irq_vector = 0xC7; // RST 00h, which mode 1 must not honour
        c.ack_irq(&mut m);
        assert_eq!(c.pc, 0x0038, "always 0x38");

        // Mode 0: the byte is an opcode, and CPS-1's board puts an RST there.
        let mut c = Z80::new();
        c.pc = 0x200;
        c.sp = 0x3000;
        c.im = 0;
        c.iff1 = true;
        c.irq = true;
        let mut m = Mem::at(0x200, &[0x00]);
        m.irq_vector = 0xD7; // RST 10h
        c.ack_irq(&mut m);
        assert_eq!(c.pc, 0x0010, "the bus byte chose the vector");
    }

    /// A request with `IFF1` clear is held, not dropped.
    ///
    /// The line is level-sensitive: the device holds it until acknowledged. Clearing
    /// `irq` on a rejected check would lose the interrupt, which is a class of bug
    /// that presents as occasional missing sound rather than as a failure.
    #[test]
    fn a_rejected_request_stays_pending() {
        let mut c = Z80::new();
        c.pc = 0x200;
        c.sp = 0x3000;
        c.im = 1;
        c.iff1 = false;
        c.irq = true;
        let mut m = Mem::at(0x200, &[0x00]);
        assert_eq!(c.ack_irq(&mut m), None);
        assert!(c.irq, "still asserted -- the device holds the line");
        c.iff1 = true;
        assert_eq!(c.ack_irq(&mut m), Some(13), "and it lands when unmasked");
    }

    /// `reset` does not clear the two request lines.
    ///
    /// They are input pins, not state: a request asserted while the CPU is held in
    /// reset is still asserted when it comes out. Clearing them in `reset` would drop
    /// the first interrupt after every reset — including the one D2's sound board
    /// raises to hand over its first command.
    #[test]
    fn reset_leaves_the_request_lines_alone() {
        let mut c = Z80::new();
        c.irq = true;
        c.nmi = true;
        c.reset();
        assert!(c.irq, "still asserted across a reset");
        assert!(c.nmi);
        // And a fresh CPU has neither, so the fields start where the pins do.
        let fresh = Z80::new();
        assert!(!fresh.irq);
        assert!(!fresh.nmi);
    }
}
