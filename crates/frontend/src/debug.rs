//! The debugger's state: which panels, where they are looking, and where to stop.
//!
//! # Why the state is here and not in the loop
//!
//! The loop in `sfemu` is behind the display boundary in everything but name: it owns
//! a window, a clock, and a file system. Every decision the debugger makes — should
//! this frame run at all, has a breakpoint been hit, which panel does `PageDown`
//! move — is a decision testable without any of those, so it lives here. What the
//! loop does with a [`Debugger`] is three calls and no arithmetic.
//!
//! # `Option<u32>` is not `u32`
//!
//! [`Debugger::disasm_at`] is `None` for "follow the PC" rather than a `u32` kept
//! equal to it. Those are different states and the difference is visible: a listing
//! scrolled to an address that *happens* to be the PC must not start following it, or
//! stepping would yank the view away from what you were reading. Same for
//! [`Debugger::mem_at`] — except that a memory dump has no address to follow, so it
//! is a plain `u32` and `Home` returns it to the stack pointer, which is the address
//! you actually wanted when you pressed it.
//!
//! # Nothing here writes to the machine
//!
//! `should_break` and `draw` take `&Cps1`. `update` takes `&Cps1` — it reads the PC
//! to place a breakpoint and to reset the follow address, and that is all. Stepping
//! is the loop's to perform, because only the loop knows whether the machine is
//! paused.

use crate::keys::Actions;
use crate::overlay::{self, executing_pc, Panels, DIS_ROWS, MEM_ROWS, MEM_WORDS};
use machine::Cps1;

/// Which panel the scroll keys move.
///
/// Two variants, not four. `Panels` has four flags but only two of them scroll: the
/// registers and the status line have nowhere to go. A focus that cycled through all
/// four would spend half its presses on panels that ignore the keys, which reads as
/// the key being broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// `PageUp`/`PageDown` move the disassembly.
    #[default]
    Disasm,
    /// `PageUp`/`PageDown` move the memory dump.
    Mem,
}

impl Focus {
    /// The other one.
    pub const fn cycled(self) -> Self {
        match self {
            Focus::Disasm => Focus::Mem,
            Focus::Mem => Focus::Disasm,
        }
    }
}

/// How far `PageUp`/`PageDown` move the disassembly.
///
/// A listing is `DIS_ROWS` instructions, but instructions are 2 to 10 bytes and the
/// bytes are what an address is made of, so a page cannot be an exact screenful in
/// both directions. Two bytes per row is the honest compromise: scrolling forward
/// then back returns to the address you started at, which matters more than landing
/// exactly one screen away. Landing mid-instruction is recoverable — `Home` and the
/// `>` marker both tell you where the machine really is.
const DIS_PAGE: u32 = (DIS_ROWS * 2) as u32;

/// How far `PageUp`/`PageDown` move the memory dump: exactly one screenful.
///
/// Unlike the disassembly, a dump's rows are a fixed width, so a page is exact and
/// scrolling forward then back is exactly reversible.
const MEM_PAGE: u32 = (MEM_ROWS * MEM_WORDS * 2) as u32;

/// The debugger's whole state.
///
/// `Vec<u32>` for the breakpoints rather than a set: they are compared by scanning,
/// there are a handful at most, and the order is the order they were set — which is
/// the order a list of them should be shown in.
#[derive(Debug, Clone, Default)]
pub struct Debugger {
    /// Which panels are drawn.
    pub panels: Panels,
    /// Which panel the scroll keys move.
    pub focus: Focus,
    /// Where the listing starts, or `None` to follow the executing instruction.
    pub disasm_at: Option<u32>,
    /// Where the dump starts.
    pub mem_at: u32,
    /// Addresses to stop at, compared against the *executing* instruction.
    pub breakpoints: Vec<u32>,
    /// The machine's cycle count when a breakpoint last fired.
    ///
    /// Without something here, `should_break` is true on the instruction the machine
    /// is stopped *at*, so resuming stops again immediately, forever — a breakpoint
    /// you can set and never get past.
    ///
    /// ⚠️ **A cycle count, not the address.** The address was the obvious choice and it
    /// is wrong: it suppresses that address for the rest of the session, so a
    /// breakpoint inside a loop fires exactly once and then silently stops working —
    /// which is a debugger that lies to you about a loop, the thing you most often set
    /// a breakpoint in. Caught by `a_breakpoint_does_not_refire_where_it_stopped`'s
    /// second half, which runs the fixture's loop round and requires a second stop.
    ///
    /// The cycle count is exactly the identity of *this* stop: it advances on every
    /// instruction — a halted CPU still bills four cycles — so the suppression expires
    /// by itself the moment the machine moves, with no second call from the loop to
    /// clear it and no `&mut self` on a predicate.
    stopped_at: Option<u64>,
}

impl Debugger {
    /// A debugger with nothing shown and no breakpoints.
    ///
    /// The overlay starts off: it covers most of the screen, and a debugger that
    /// appeared uninvited would make the emulator look broken on first run.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one frame's debugger keys. Returns whether the overlay is on.
    ///
    /// `m` is read, never written: the PC for `F7`'s breakpoint address and `Home`'s
    /// memory address, and nothing else.
    pub fn update(&mut self, a: &Actions, m: &Cps1) -> bool {
        if a.overlay_toggled {
            self.panels = if self.panels.any() {
                Panels::none()
            } else {
                Panels::on()
            };
        }
        if a.focus_cycled {
            self.focus = self.focus.cycled();
            // Focusing the memory dump shows it. Otherwise `F6` appears to do nothing
            // on the default panel set, which does not include the dump — and the key
            // that "does nothing" is the one you press to look at memory.
            if self.focus == Focus::Mem {
                self.panels.mem = true;
            }
        }
        if a.breakpoint_toggled {
            let at = executing_pc(m);
            if let Some(i) = self.breakpoints.iter().position(|&b| b == at) {
                self.breakpoints.remove(i);
            } else {
                self.breakpoints.push(at);
            }
        }
        if a.scroll_up {
            self.scroll(m, false);
        }
        if a.scroll_down {
            self.scroll(m, true);
        }
        if a.follow_reset {
            match self.focus {
                Focus::Disasm => self.disasm_at = None,
                // The stack pointer, from `a[7]` — never the `usp`/`ssp` shadows,
                // which are stale inside an exception handler, which is where you are
                // when you want to look at the stack.
                Focus::Mem => self.mem_at = m.cpu.a[7],
            }
        }
        self.panels.any()
    }

    /// Moves the focused panel one page.
    ///
    /// The disassembly's first scroll has to materialise an address: while it is
    /// `None` it is following the PC, and the page it moves from is the one on screen.
    fn scroll(&mut self, m: &Cps1, forward: bool) {
        match self.focus {
            Focus::Disasm => {
                let from = self.disasm_at.unwrap_or_else(|| executing_pc(m));
                self.disasm_at = Some(if forward {
                    from.wrapping_add(DIS_PAGE)
                } else {
                    from.wrapping_sub(DIS_PAGE)
                });
            }
            Focus::Mem => {
                self.mem_at = if forward {
                    self.mem_at.wrapping_add(MEM_PAGE)
                } else {
                    self.mem_at.wrapping_sub(MEM_PAGE)
                };
            }
        }
    }

    /// Where the listing starts: the follow address, or the executing instruction.
    pub fn disasm_from(&self, m: &Cps1) -> u32 {
        self.disasm_at.unwrap_or_else(|| executing_pc(m))
    }

    /// Whether the machine must stop before executing its next instruction.
    ///
    /// Compared against [`executing_pc`], **not** `cpu.pc`. The PC is four bytes past
    /// the instruction about to run, so `pc == addr` fires an instruction or two late,
    /// and for a multi-word instruction it fires at an address that is not an
    /// instruction boundary at all — a breakpoint that "works sometimes".
    pub fn should_break(&self, m: &Cps1) -> bool {
        self.stopped_at != Some(m.total_cycles) && self.breakpoints.contains(&executing_pc(m))
    }

    /// Records that the machine has stopped here, so resuming does not stop again.
    ///
    /// Called by the loop when a break fires. `should_break` alone cannot do this: it
    /// takes `&self` because it is also what a test and a status panel ask, and a
    /// predicate with a side effect would give a different answer the second time it
    /// was asked.
    pub fn note_stopped(&mut self, m: &Cps1) {
        self.stopped_at = Some(m.total_cycles);
    }

    /// Draws the enabled panels, if any.
    pub fn draw(&self, buf: &mut [u32], m: &Cps1) {
        if !self.panels.any() {
            return;
        }
        overlay::draw(
            buf,
            m,
            self.panels,
            self.disasm_from(m),
            self.mem_at,
            &self.breakpoints,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{Controls, Key, KeySet};
    use machine::config::BoardConfig;
    use machine::timing::Timing;

    /// A machine stopped at a multi-word instruction at 0x1000.
    ///
    /// The multi-word instruction is load-bearing, not decoration: with one-word
    /// instructions only, `pc - 4`, `pc - 2`, and a stale `pc` all land on
    /// instruction boundaries, so a breakpoint test cannot tell a correct
    /// implementation from any of three wrong ones.
    fn a_machine() -> Cps1 {
        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x1000..0x100E].copy_from_slice(&[
            0x33, 0xC0, 0x00, 0xFF, 0x00, 0x00, // move.w d0,$FF0000 (3 words)
            0x52, 0x40, // addq.w #1,d0    (1 word)
            0x52, 0x40, // addq.w #1,d0    (1 word)
            // `bra.s` back to 0x1000. The displacement is from the word *after* the
            // opcode: 0x100C - 0x1000 = 12, so -12 = 0xF4. 0xF8 lands on 0x1004, which
            // is the middle of the three-word instruction — worth spelling out, because
            // that is what it was, and the failure was an assertion about where the loop
            // returns to rather than anything about the debugger.
            0x60, 0xF4, // bra.s 0x1000
            0x4E, 0x71, // nop
        ]);
        let mut m = Cps1::new(&rom, BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        m
    }

    /// The actions one key press produces, through the real `Controls`.
    ///
    /// Built from a `KeySet` rather than by setting `Actions` fields directly, so
    /// these tests exercise the key map too: a debugger action wired to the wrong key
    /// would pass every test that constructed `Actions` by hand.
    fn pressing(k: Key) -> Actions {
        Controls::new().update(KeySet::from_keys(&[k]))
    }

    /// A breakpoint fires at the instruction, not at the PC.
    ///
    /// **The test this whole task turns on.** `cpu.pc` is four bytes beyond the
    /// executing instruction, so comparing against it fires late — or, for the
    /// three-word instruction this fixture starts with, at 0x1004, which is the middle
    /// of it and not an instruction boundary at all.
    #[test]
    fn a_breakpoint_fires_at_the_instruction_not_at_the_pc() {
        let mut m = a_machine();
        assert_eq!(m.cpu.pc, 0x1004, "the premise: the PC is four bytes ahead");
        let mut d = Debugger::new();
        d.breakpoints.push(0x1000);
        assert!(d.should_break(&m), "the breakpoint is at the instruction");

        let mut wrong = Debugger::new();
        wrong.breakpoints.push(0x1004);
        assert!(
            !wrong.should_break(&m),
            "and not at the PC, which here is mid-instruction"
        );

        // Step past it and it stops firing — and the instruction stepped over was
        // three words, so the next PC is not `pc + 2`.
        m.step_instruction();
        assert_eq!(
            m.cpu.pc, 0x100A,
            "the premise: a three-word instruction ran"
        );
        assert!(!d.should_break(&m), "the breakpoint is behind us now");
    }

    /// A breakpoint does not re-fire on the instruction it stopped at.
    ///
    /// Otherwise resuming from a breakpoint stops again immediately, forever: the
    /// breakpoint can be reached and never passed.
    #[test]
    fn a_breakpoint_does_not_refire_where_it_stopped() {
        let mut m = a_machine();
        let mut d = Debugger::new();
        d.breakpoints.push(0x1000);
        assert!(d.should_break(&m), "the premise: it fires once");
        d.note_stopped(&m);
        assert!(!d.should_break(&m), "and not again at the same instruction");

        // Run right round the loop and back to 0x1000: it must fire again, or a
        // breakpoint in a loop body works exactly once. This is the half that rejected
        // suppressing by *address*, which is the obvious implementation.
        for _ in 0..4 {
            m.step_instruction();
        }
        assert_eq!(executing_pc(&m), 0x1000, "the premise: back at the top");
        assert!(d.should_break(&m), "a second pass through must stop again");

        // And the suppression really is tied to this stop, not merely to time passing:
        // noting it again here silences it again.
        d.note_stopped(&m);
        assert!(!d.should_break(&m), "stopped here now");
    }

    /// The suppression expires when the machine moves, and not before.
    ///
    /// One instruction is enough: the cycle count changes, so the same address stops
    /// again. Both halves matter — a suppression keyed to nothing would never silence
    /// the stop, and one keyed to the address would never release it.
    #[test]
    fn the_suppression_lasts_exactly_one_stop() {
        let mut m = a_machine();
        let mut d = Debugger::new();
        // Every instruction in the loop is a breakpoint, so the address cannot be what
        // distinguishes the two answers below.
        d.breakpoints.extend([0x1000, 0x1006, 0x1008, 0x100A]);
        d.note_stopped(&m);
        assert!(!d.should_break(&m), "suppressed at this stop");
        let cycles = m.total_cycles;
        m.step_instruction();
        assert!(
            m.total_cycles > cycles,
            "the premise: an instruction costs cycles"
        );
        assert!(d.should_break(&m), "and the next instruction stops");
    }

    /// `should_break` has no side effect.
    ///
    /// It is asked by the loop, by the tests, and potentially by a status panel. A
    /// predicate that recorded `stopped_at` itself would answer differently the second
    /// time, so the loop's `note_stopped` is a separate call and this pins that.
    #[test]
    fn asking_whether_to_break_does_not_change_the_answer() {
        let m = a_machine();
        let mut d = Debugger::new();
        d.breakpoints.push(0x1000);
        assert!(d.should_break(&m));
        assert!(d.should_break(&m), "asking twice gives the same answer");
        assert!(d.should_break(&m), "and a third time");
    }

    /// `F7` sets a breakpoint at the current instruction, then clears it.
    #[test]
    fn f7_sets_then_clears_a_breakpoint_at_the_current_instruction() {
        let m = a_machine();
        let mut d = Debugger::new();
        d.update(&pressing(Key::F7), &m);
        assert_eq!(
            d.breakpoints,
            vec![0x1000],
            "set at the instruction, not the PC"
        );
        d.update(&pressing(Key::F7), &m);
        assert!(d.breakpoints.is_empty(), "and pressed again, cleared");
    }

    /// `F7` twice at different addresses keeps both.
    ///
    /// A toggle implemented as "clear the list, or set one" would pass the test above
    /// and lose every breakpoint but the last.
    #[test]
    fn two_breakpoints_at_different_addresses_both_stand() {
        let mut m = a_machine();
        let mut d = Debugger::new();
        d.update(&pressing(Key::F7), &m);
        m.step_instruction();
        d.update(&pressing(Key::F7), &m);
        assert_eq!(d.breakpoints, vec![0x1000, 0x1006], "both");
        // And clearing one leaves the other.
        d.update(&pressing(Key::F7), &m);
        assert_eq!(d.breakpoints, vec![0x1000], "the first survives");
    }

    /// `F1` toggles the whole overlay, and `F6` moves the focus.
    #[test]
    fn f1_toggles_the_overlay_and_f6_cycles_the_focus() {
        let m = a_machine();
        let mut d = Debugger::new();
        assert!(!d.panels.any(), "the overlay starts off");

        assert!(d.update(&pressing(Key::F1), &m), "F1 turns it on");
        assert!(d.panels.regs && d.panels.disasm && d.panels.status);
        assert!(!d.update(&pressing(Key::F1), &m), "and off again");
        assert!(!d.panels.any());

        d.update(&pressing(Key::F1), &m);
        assert_eq!(
            d.focus,
            Focus::Disasm,
            "the disassembly has the focus first"
        );
        d.update(&pressing(Key::F6), &m);
        assert_eq!(d.focus, Focus::Mem, "F6 moves it to memory");
        assert!(
            d.panels.mem,
            "and shows the dump, or F6 appears to do nothing"
        );
        d.update(&pressing(Key::F6), &m);
        assert_eq!(d.focus, Focus::Disasm, "and back: two states, not four");
    }

    /// Scrolling moves the focused panel and leaves the other alone.
    #[test]
    fn page_keys_scroll_only_the_focused_panel() {
        let m = a_machine();
        let mut d = Debugger::new();
        d.mem_at = 0x00FF_0000;

        // Focus is the disassembly: PageDown moves it, and the dump does not move.
        d.update(&pressing(Key::PageDown), &m);
        assert_eq!(
            d.disasm_at,
            Some(0x1000 + DIS_PAGE),
            "the listing moved from the PC"
        );
        assert_eq!(d.mem_at, 0x00FF_0000, "and the dump did not");

        // Focus the dump, and now the opposite.
        d.update(&pressing(Key::F6), &m);
        let listing = d.disasm_at;
        d.update(&pressing(Key::PageDown), &m);
        assert_eq!(d.mem_at, 0x00FF_0000 + MEM_PAGE, "the dump moved");
        assert_eq!(d.disasm_at, listing, "and the listing did not");
    }

    /// Scrolling back returns to where it started, both panels.
    ///
    /// A page forward and a page back must be the same distance. They are separate
    /// constants per panel and the natural bug is one of them being a screenful and
    /// the other a row.
    #[test]
    fn a_page_forward_and_a_page_back_cancel() {
        let m = a_machine();
        let mut d = Debugger::new();
        d.mem_at = 0x00FF_1000;
        for (focus, key) in [(Focus::Disasm, Key::PageUp), (Focus::Mem, Key::PageUp)] {
            d.focus = focus;
            d.disasm_at = Some(0x2000);
            d.mem_at = 0x00FF_1000;
            d.update(&pressing(key), &m);
            d.update(&pressing(Key::PageDown), &m);
            assert_eq!(d.disasm_at, Some(0x2000), "the listing, focus {focus:?}");
            assert_eq!(d.mem_at, 0x00FF_1000, "the dump, focus {focus:?}");
        }
    }

    /// `Home` makes the disassembly follow the PC again.
    ///
    /// `None` and "equal to the PC right now" are different states, and the difference
    /// shows on the next step: a panel scrolled to the PC's address must not start
    /// following it, or reading around the current instruction becomes impossible.
    #[test]
    fn home_makes_the_disassembly_follow_the_pc_again() {
        let mut m = a_machine();
        let mut d = Debugger::new();

        // Scrolled to exactly the PC, but not following it.
        d.disasm_at = Some(executing_pc(&m));
        m.step_instruction();
        assert_eq!(
            d.disasm_from(&m),
            0x1000,
            "a scrolled panel stays where it was put"
        );
        assert_ne!(
            executing_pc(&m),
            0x1000,
            "the premise: the machine moved on"
        );

        d.update(&pressing(Key::Home), &m);
        assert_eq!(d.disasm_at, None, "Home means follow, not `Some(pc)`");
        assert_eq!(
            d.disasm_from(&m),
            executing_pc(&m),
            "and following means it tracks"
        );
        // Which the `Some(pc)` version would fail on the very next step.
        m.step_instruction();
        assert_eq!(d.disasm_from(&m), executing_pc(&m), "still following");
    }

    /// `Home` sends the memory dump to the stack pointer.
    ///
    /// A dump has nothing to follow, so `Home` needs a destination, and the stack is
    /// the address you wanted: it is where the return addresses and the saved
    /// registers are. From `a[7]`, never the `usp`/`ssp` shadows, which are stale
    /// inside a handler.
    #[test]
    fn home_sends_the_memory_dump_to_the_stack_pointer() {
        let mut m = a_machine();
        m.cpu.a[7] = 0x00FF_7FF0;
        assert_ne!(m.cpu.a[7], m.cpu.ssp, "the premise: the shadow is stale");
        let mut d = Debugger::new();
        d.focus = Focus::Mem;
        d.mem_at = 0x1234;
        d.update(&pressing(Key::Home), &m);
        assert_eq!(d.mem_at, 0x00FF_7FF0, "the active stack pointer, from a[7]");
    }

    /// Scrolling past the end of the address space wraps rather than panicking.
    ///
    /// A debug build panics on overflow, so a dump scrolled to the top of memory would
    /// take the emulator down — and the top of memory is exactly where you scroll to
    /// when you are looking for the end of something.
    #[test]
    fn scrolling_past_the_end_of_the_address_space_wraps() {
        let m = a_machine();
        let mut d = Debugger::new();
        d.focus = Focus::Mem;
        d.mem_at = 0xFFFF_FFF0;
        d.update(&pressing(Key::PageDown), &m);
        assert_eq!(d.mem_at, 0xFFFF_FFF0u32.wrapping_add(MEM_PAGE));
        d.mem_at = 0;
        d.update(&pressing(Key::PageUp), &m);
        assert_eq!(d.mem_at, 0u32.wrapping_sub(MEM_PAGE), "and back off zero");

        d.focus = Focus::Disasm;
        d.disasm_at = Some(0);
        d.update(&pressing(Key::PageUp), &m);
        assert_eq!(
            d.disasm_at,
            Some(0u32.wrapping_sub(DIS_PAGE)),
            "the listing"
        );
    }

    /// A held key acts once, through the real `Controls`.
    ///
    /// `keys.rs` proves the edge detection; this proves the debugger is driven by it
    /// rather than by the level. A held `F4` stepping sixty instructions a second is
    /// unusable, and a held `F7` toggling a breakpoint every frame lands on whichever
    /// parity the frame count happens to have.
    #[test]
    fn a_held_debugger_key_acts_once() {
        let m = a_machine();
        let mut c = Controls::new();
        let mut d = Debugger::new();
        let held = KeySet::from_keys(&[Key::F7]);
        d.update(&c.update(held), &m);
        assert_eq!(d.breakpoints, vec![0x1000], "the press sets one");
        for _ in 0..8 {
            d.update(&c.update(held), &m);
        }
        assert_eq!(
            d.breakpoints,
            vec![0x1000],
            "and holding does not toggle it"
        );
    }

    /// Nothing pressed changes nothing.
    ///
    /// The frame-by-frame case, which is almost every frame. A `Debugger` that acted
    /// on a default `Actions` would scroll or toggle continuously.
    #[test]
    fn an_idle_frame_changes_nothing() {
        let m = a_machine();
        let mut d = Debugger::new();
        d.panels = Panels::on();
        d.disasm_at = Some(0x2000);
        d.mem_at = 0x00FF_1000;
        d.breakpoints.push(0x1000);
        let before = format!("{d:?}");
        for _ in 0..10 {
            assert!(d.update(&Actions::default(), &m), "the overlay stays on");
        }
        assert_eq!(format!("{d:?}"), before, "and nothing else moved");
    }

    /// `draw` draws when panels are on and nothing when they are off.
    ///
    /// The overlay's own tests cover what the panels contain. This covers the one
    /// decision `Debugger` makes about drawing: whether to.
    #[test]
    fn drawing_is_skipped_when_the_overlay_is_off() {
        let m = a_machine();
        let blank = vec![0x00AB_CDEF_u32; machine::video::WIDTH * machine::video::HEIGHT];
        let mut d = Debugger::new();

        let mut buf = blank.clone();
        d.draw(&mut buf, &m);
        assert_eq!(buf, blank, "off: not a pixel");

        d.panels = Panels::on();
        let mut buf = blank.clone();
        d.draw(&mut buf, &m);
        assert_ne!(buf, blank, "on: something was drawn");
    }

    /// The listing is drawn from the follow address, and the breakpoints reach it.
    ///
    /// `draw` passes four things to `overlay::draw` and passing the wrong one is
    /// invisible in a test that only asks whether *anything* was drawn — a listing
    /// stuck at address 0 looks like a plausible debugger until you notice it never
    /// moves.
    #[test]
    fn the_listing_is_drawn_where_the_debugger_is_looking() {
        let m = a_machine();
        let mut d = Debugger::new();
        d.panels = Panels::on();
        d.disasm_at = Some(0x1006);
        d.breakpoints.push(0x1006);

        let mut mine = vec![0u32; machine::video::WIDTH * machine::video::HEIGHT];
        d.draw(&mut mine, &m);

        // The same frame drawn by `overlay::draw` directly, with the arguments this
        // module is supposed to be passing.
        let mut expected = vec![0u32; machine::video::WIDTH * machine::video::HEIGHT];
        overlay::draw(&mut expected, &m, Panels::on(), 0x1006, 0, &[0x1006]);
        assert_eq!(mine, expected, "the same frame, so the arguments match");

        // And a *different* address gives a different frame, or the comparison above
        // would pass for a `draw` that ignored `disasm_at` entirely.
        let mut other = vec![0u32; machine::video::WIDTH * machine::video::HEIGHT];
        overlay::draw(&mut other, &m, Panels::on(), 0x1000, 0, &[0x1006]);
        assert_ne!(mine, other, "the premise: the address changes the frame");
    }
}
