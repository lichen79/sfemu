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
    /// `a[7]` is the *active* stack pointer. The inactive one lives in [`Self::usp`]
    /// or [`Self::ssp`] depending on the S bit.
    ///
    /// ⚠️ **`a[7]` is the only authoritative copy, and the matching shadow slot goes
    /// stale.** This doc used to say `a[7]` "mirrors" `usp`/`ssp`, which reads as an
    /// invariant and is not one. Measured: after `BSR` in supervisor mode `a[7]` is
    /// `0x2FFC` while `ssp` still reads `0x3000`, and after `BSR` in user mode `a[7]`
    /// is `0x7FFC` while `usp` still reads `0x8000`. Most handlers that move the stack
    /// do not write the shadow.
    ///
    /// That is sound because [`Self::set_sr`] saves the outgoing `a[7]` into the right
    /// slot *before* loading the incoming one, so the stale value is always overwritten
    /// before it can be read as the active pointer. The invariant is therefore the
    /// weaker one:
    ///
    /// > The **inactive** pointer is valid in its slot; the **active** pointer is valid
    /// > only in `a[7]`.
    ///
    /// So read the active SP from `a[7]` and never from `usp`/`ssp` — `exception.rs`'s
    /// `frame_base` is the pattern to copy. `ops::system::sync_sp` writes the shadow
    /// anyway for debugging and save-state legibility, and is pinned by exactly one
    /// assertion: making it a no-op kills only
    /// `ops::system::tests::link_a7_pushes_the_entry_stack_pointer` and no suite group
    /// (measured, Task 14). One test is the right amount for a claim that is explicitly
    /// not load-bearing — but it does mean the shadow's coherence rests on that single
    /// assertion, not on the 317,500 cases, because the harness reads the active
    /// pointer out of `a[7]` and so cannot see a stale shadow at all.
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
    /// T was set when the last instruction **began**, so a trace exception is
    /// owed at the next instruction boundary.
    ///
    /// Two independent facts are packed into this flag. They have different
    /// evidence behind them, and conflating the two is a mistake this comment
    /// previously made:
    ///
    /// - **Fired at the next boundary, not this one — measured.**
    ///   [`M68k::step_with`] must return the traced instruction's own result first.
    ///   The suite runs one instruction per case, so vector 9 is fetched 0 times in
    ///   317,500 cases while 158,894 *enter* with T=1: a core that traced within the
    ///   instruction's own step fails ~38% of the suite. That is what makes this a
    ///   flag rather than a check inside the same step.
    /// - **Sampled at instruction start — extrapolated.** Once latched, the trace is
    ///   owed even if the instruction goes on to clear T itself. From the User's
    ///   Manual's definition of T, and **not** measurable here: no case takes a
    ///   trace, so no count can discriminate start-sampling from end-sampling.
    ///
    /// ⚠️ The suite census of which instructions can *clear* T — `ANDItoSR`,
    /// `EORItoSR`, `STOP`, `MOVEtoSR` and `RTE`, the 1,277 clean T=1 cases that end
    /// with T clear, with `ORItoSR` at 0/591 as the control since `OR` cannot clear
    /// a bit — is accurate, and it is what identifies **where the two rules
    /// disagree**. It cannot say which rule is right; it is a census of
    /// instructions, not of sampling points. It was once cited as measured support
    /// for reading the final T, which the number cannot reach.
    ///
    /// What settles it is behaviour rather than a count. The two discriminating
    /// cases, asserted by
    /// `exception::tests::t_is_sampled_at_instruction_start_not_at_its_end`:
    ///
    /// | case | owed? |
    /// |---|---|
    /// | `ANDI #$7FFF,SR` clears T, entered with T=1 | **yes** |
    /// | `ORI #$8000,SR` sets T, entered with T=0 | **no** (the next one is) |
    ///
    /// End-sampling inverts both, and inverts the single-step mechanism with them: a
    /// trace handler ends in `RTE`, whose popped SR restores T, so an end-sampling
    /// core owes a trace for the `RTE` and re-enters the handler having executed
    /// nothing —
    /// `single_stepping_with_an_rte_handler_advances_one_instruction_per_trace`
    /// measures 1 traced instruction in 200 steps that way, against 4 in 12 here.
    ///
    /// Nothing consults the live T bit once this is latched; see
    /// [`crate::exception::take_trace`]. Two places move the flag afterwards, and
    /// both are rules rather than conveniences:
    ///
    /// - `ops::system::stop` may *raise* it, for the manual's rule about a `STOP`
    ///   immediate that sets T — the start-of-instruction latch cannot see that,
    ///   because T was clear when the `STOP` began.
    /// - [`crate::exception::address_error`] *withdraws* it. The PRM owes the trace
    ///   only once the instruction has **completed**, and a group-0 fault aborts it.
    ///   An instruction *trap* is a completion and keeps its trace, so the two kinds
    ///   of exception are opposite here and a shared `!halted` test cannot express
    ///   both.
    pub trace_pending: bool,
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

    /// Advances the pipeline by one word: promotes slot 1 to slot 0, reads the
    /// next word from PC into slot 1, and increments PC by 2.
    ///
    /// This is the same bus operation as `fetch_word_dyn` — both bodies are
    /// identical except that `fetch_word_dyn` also returns the consumed word.
    /// Use whichever makes intent clearest: `consume_opcode` when the handler
    /// already has the opcode from `step_with`'s peek and the advance is purely
    /// for pipeline bookkeeping; `fetch_word` when the handler needs the value
    /// of the word it is consuming (extension words, immediates, displacements).
    ///
    /// **Placement within a handler's bus sequence is per-instruction** and must
    /// be derived from the vector data.  Many instructions read operands from
    /// memory *before* the pipeline-advance read; calling this first would emit
    /// the reads out of order.
    ///
    /// **Warning — OPCODE_PC_OFFSET:** calling this followed by `fetch_word` for
    /// one extension word leaves `cpu.pc` 8 bytes past the opcode, not 4.
    /// `exception::OPCODE_PC_OFFSET` is valid only for a handler that consumed
    /// exactly the opcode word; multi-word handlers must capture `cpu.pc` before
    /// any fetches and compute their own stacked-PC value.
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
        self.trace_pending = false;
        self.refill_prefetch_dyn(bus);
    }

    /// Raises an interrupt at `level` (0 clears, 1-7 are IPL).
    ///
    /// # The caller owns deassertion
    ///
    /// [`M68k::pending_irq`] is a **level**, exactly like the 68000's IPL pins, and
    /// nothing inside the core clears it. A device model must call this with `0`
    /// once its handler has acknowledged the interrupt — see
    /// `testrunner/tests/integration_asm.rs`, whose level-4 handler does — or the
    /// core will keep taking the same interrupt.
    ///
    /// ⚠️ **Levels 1-6 are self-limiting; level 7 is not.** Exception entry raises
    /// the SR's mask to the level being serviced, so a held level 1-6 is blocked
    /// while its own handler runs. Level 7 is non-maskable, so **no mask value can
    /// block it**: a level-7 line left asserted re-enters the handler at every
    /// instruction boundary without the handler executing a single instruction,
    /// pushing a 6-byte frame each time until the stack pointer wraps. That is a
    /// livelock, not a modelling artefact of this core — real hardware makes level
    /// 7 transition-sensitive for precisely this reason.
    ///
    /// This core does **not** model that transition sensitivity, deliberately: a
    /// level-triggered `pending_irq` is the contract the existing integration tests
    /// are written against, and edge sensitivity would silently change what
    /// "assert the line and step" means for every caller. The requirement is
    /// documented here instead. See [`crate::exception::check_interrupts`].
    ///
    /// # Panics
    ///
    /// In debug builds, if `level > 7`. The three IPL pins encode 0..=7 and nothing
    /// else, so a larger value is a caller bug rather than an input to clamp.
    ///
    /// ⚠️ **The `& 7` is a mask, not a clamp, and the difference is a silent wrong
    /// answer.** `set_irq(8)` masks to **0** — a *deassertion* — so a caller that
    /// computed a level off by one would silently disable interrupts rather than
    /// request a high-priority one. `set_irq(9)` becomes level 1, the lowest priority.
    /// Neither reports anything. The `debug_assert` turns both into a named failure in
    /// tests while leaving release builds unchanged; the mask stays so release
    /// behaviour is still defined.
    pub fn set_irq(&mut self, level: u8) {
        debug_assert!(
            level <= 7,
            "IRQ level {level} is out of range: the IPL pins encode 0..=7, and masking \
             would turn 8 into a deassertion rather than a high-priority request"
        );
        self.pending_irq = level & 7;
    }

    /// Executes one instruction, returning the cycles it consumed.
    ///
    /// # The instruction boundary
    ///
    /// Two exceptions fire at a *boundary* rather than inside an instruction, and
    /// both live here rather than in any handler:
    ///
    /// - **Trace**, owed by the instruction that just finished, because T was set
    ///   when that instruction *began*. It is taken as its own step at the *next*
    ///   call, which is what "the boundary after the instruction" means and why the
    ///   vector suite — one instruction per case — sees zero trace exceptions
    ///   despite 158,894 cases entering with T=1. Taking it before returning from
    ///   the same call would fail 38% of the suite. Trace outranks interrupts, so
    ///   it is checked first.
    /// - **Interrupts**, sampled before the instruction runs. This is also a path
    ///   out of `stopped`: [`crate::exception::check_interrupts`] re-primes the
    ///   queue, because STOP leaves the PC and both queue words frozen with its
    ///   own opcode still in slot 0. Clearing `stopped` here and dispatching would
    ///   re-execute the STOP forever.
    ///
    /// # `stopped` is checked after both, and both can clear it
    ///
    /// `STOP #$A700` loads an SR with T set, so it is stopped *and* owes a trace.
    /// Hardware has no state corresponding to "inside a handler while stopped", so
    /// the trace must win and must resume the CPU — which is why
    /// [`crate::exception::take_trace`] clears `stopped` rather than this ordering
    /// merely tolerating it. Leaving `stopped` set there wedges the CPU
    /// permanently: every later step falls through to the `return 4` below with the
    /// handler's first instruction never run, and only an interrupt escapes, into
    /// that same un-started handler.
    ///
    /// So the order here is deliberate and each leg is load-bearing: trace (clears
    /// `stopped`), then interrupts (clears `stopped`), then the `stopped` gate for
    /// the case where neither fired.
    ///
    /// Both exceptions are zero-coverage paths; see
    /// [`crate::exception::check_interrupts`] and
    /// [`crate::exception::take_trace`].
    pub fn step_with(&mut self, dec: &crate::decode::Decoder, bus: &mut impl crate::Bus) -> u32 {
        // A halted CPU is dead until reset, but still burns time.
        if self.halted {
            return 4;
        }
        if self.trace_pending {
            self.trace_pending = false;
            return crate::exception::take_trace(self, bus);
        }
        if let Some(c) = crate::exception::check_interrupts(self, bus) {
            return c;
        }
        // Neither fired, so a stopped CPU stays stopped: burn 4 cycles without
        // touching the PC or the queue (STOP's own measured shape).
        if self.stopped {
            return 4;
        }
        // Latch T **now**, before the instruction runs, and do not re-read it
        // afterwards. The 68000 samples T at instruction start, so the trace is
        // owed on the strength of the entry SR even if the instruction goes on to
        // change T itself.
        //
        // ⚠️ **The sampling point is extrapolated, not measured.** It comes from the
        // User's Manual's definition of T; no suite case can reach it, since vector
        // 9 is fetched 0 times in 317,500 cases (control: 158,894 of those cases
        // *enter* with T=1, so T is well represented in the corpus — what is absent
        // is any trace being taken, not the bit). The suite census of instructions
        // that can *clear* T — `ANDItoSR`, `EORItoSR`, `STOP`, `MOVEtoSR` and `RTE`,
        // the 1,277 clean cases that end with T clear, `ORItoSR` at 0/591 as the
        // control since `OR` cannot clear a bit — identifies **where start-sampling
        // and end-sampling disagree**, and nothing more; it is a census of
        // instructions, not of sampling points, and it was previously mis-cited here
        // as evidence for reading the final T. What actually rules end-sampling out
        // is that it breaks single-stepping: the handler ends in an `RTE` whose
        // popped SR restores T, so the `RTE` itself owes a trace and the handler
        // re-enters having executed nothing. See `trace_pending` for the two
        // discriminating cases, both asserted.
        //
        // ⚠️ Nothing below re-reads T, but the latch is not the whole rule: the PRM
        // owes the trace only once the instruction has **completed**, and that splits
        // the exception cases in two rather than treating them alike.
        //
        // - An instruction **trap** is a completion. `TRAP`, `TRAPV`, `CHK`,
        //   divide-by-zero, illegal and privilege all did what they are defined to do,
        //   so the trace is still owed: a traced `TRAP` stacks the trap frame and then
        //   the trace frame, and the trace handler's `RTE` returns into the trap
        //   handler. That is this line's business, and it is why nothing here
        //   withdraws the latch.
        // - An **address or bus error aborts**. The instruction stopped mid-way with
        //   its operand unstored, so the PRM's condition is never met and no trace is
        //   owed. `exception::address_error` withdraws the latch, because it is the
        //   only place that knows an abort happened.
        //
        // Both directions are extrapolated (0 vector-9 fetches in 317,500) and both
        // are asserted by
        // `exception::tests::an_aborted_instruction_owes_no_trace_but_a_completed_trap_does`.
        // ⚠️ The 38,542/38,542 "exception entry clears T" census cannot decide this
        // and must not be cited for it: entry clears T on *both* paths, so it does not
        // distinguish them. A coarse `!halted` test does not either — that form owed a
        // trace after an aborted instruction, probed as a vector-9 entry on the step
        // following a faulting `MOVE.W D0,(A0)`.
        self.trace_pending = self.sr & SR_T != 0;
        // Peek at the opcode without touching the queue.
        // Instruction handlers call `consume_opcode` (or `fetch_word` for the
        // same effect with a return value) at the point dictated by their bus
        // sequence — not necessarily first.  Exception-aborting instructions
        // (illegal opcode, Line-A, Line-F, …) never call it; they return after
        // `exception::take`, which overwrites both slots via `refill_prefetch_dyn`.
        // This matches the 68000 pipeline-abort behavior.
        let op = self.prefetch[0];
        let cycles = dec.dispatch(op)(self, bus, op);

        // A halted CPU owes nothing. The gate at the top of this function already
        // makes the flag unreachable after a halt; clearing it keeps the state
        // readable for a debugger or a save-state.
        if self.halted {
            self.trace_pending = false;
        }
        cycles
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

/// Test buses and helpers shared by every module's unit tests.
///
/// Lives in the crate (not `tests/`) because the modules that need it are
/// testing crate-internal behavior.
#[cfg(test)]
pub(crate) mod tests_support {
    /// Flat 64 KB memory bus.  No access log; use `RecordingBus` when the bus
    /// sequence matters.
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

    /// A `Bus` that **enforces** [`super::ADDR_MASK`] instead of re-applying it.
    ///
    /// ⚠️ Every other bus in this workspace masks the incoming address a second
    /// time — `FlatBus` and `RecordingBus` with `& 0xFFFF`/`& 0xFFFE`, and the
    /// harness's `TestBus`, `integration_asm`, `opcode_space` and `throughput`
    /// with `& 0x00FF_FFFF`. So none of them can observe whether the core
    /// truncated to 24 bits at all: `ADDR_MASK` widened to `0xFFFF_FFFF`
    /// survived the entire workspace, 366 tests and 317,500 suite cases, at 0
    /// failed. The control is that narrowing it to `0x000F_FFFF` fails 127
    /// tests, so the harness demonstrably *can* see the constant change; what it
    /// cannot see is the core forgetting to apply it.
    ///
    /// This bus panics on an address the core should have masked, which is what
    /// makes the widening mutant fail. `Bus`'s contract paragraph — "addresses
    /// are already masked to 24 bits by the core" — is a promise to every
    /// implementor, and a memory map decoded by address range would route an
    /// unmasked address to a different device or to nothing.
    ///
    /// Leave the *other* buses' masks alone: a 24-bit bus is what the hardware
    /// has, and modelling it is correct. They just must not be the only thing
    /// enforcing the invariant.
    pub struct StrictBus {
        inner: FlatBus,
        /// Every address exactly as the core presented it, in order. Tests
        /// assert against written-out literals here rather than recomputing the
        /// expected value from `ADDR_MASK`.
        pub seen: Vec<u32>,
    }

    impl StrictBus {
        pub fn new() -> Self {
            Self {
                inner: FlatBus::new(),
                seen: Vec::new(),
            }
        }

        pub fn put16(&mut self, addr: u32, val: u16) {
            self.inner.put16(addr, val);
        }

        pub fn load(&mut self, addr: u32, words: &[u16]) {
            self.inner.load(addr, words);
        }

        /// Reads memory back without going through the trait, so a test's own
        /// assertions do not appear in [`Self::seen`].
        pub fn peek16(&self, addr: u32) -> u16 {
            let a = (addr & 0xFFFE) as usize;
            u16::from_be_bytes([self.inner.mem[a], self.inner.mem[a + 1]])
        }

        fn check(&mut self, addr: u32) {
            assert!(
                addr <= super::ADDR_MASK,
                "the core presented {addr:#010X} to the bus, above the 24-bit \
                 address bus: `Bus`'s contract says the core has already masked"
            );
            self.seen.push(addr);
        }
    }

    impl crate::Bus for StrictBus {
        fn read8(&mut self, addr: u32) -> u8 {
            self.check(addr);
            self.inner.read8(addr)
        }
        fn read16(&mut self, addr: u32) -> u16 {
            self.check(addr);
            self.inner.read16(addr)
        }
        fn write8(&mut self, addr: u32, val: u8) {
            self.check(addr);
            self.inner.write8(addr, val);
        }
        fn write16(&mut self, addr: u32, val: u16) {
            self.check(addr);
            self.inner.write16(addr, val);
        }
    }

    /// A `Bus` that records every word-level access in order, on top of a flat
    /// memory image.  Used by tests that need to assert bus *sequence*, not just
    /// final memory state.
    pub struct RecordingBus {
        pub mem: Vec<u8>,
        /// Each entry is `(is_write, addr, val)`.
        pub log: Vec<(bool, u32, u16)>,
    }

    impl RecordingBus {
        pub fn new() -> Self {
            Self {
                mem: vec![0; 0x10000],
                log: Vec::new(),
            }
        }

        pub fn put16(&mut self, addr: u32, val: u16) {
            let [hi, lo] = val.to_be_bytes();
            let a = (addr & 0xFFFF) as usize;
            self.mem[a] = hi;
            self.mem[a + 1] = lo;
        }

        pub fn load(&mut self, addr: u32, words: &[u16]) {
            for (i, w) in words.iter().enumerate() {
                self.put16(addr + (i as u32) * 2, *w);
            }
        }

        /// Returns the subsequence of write entries: `(addr, val)`.
        pub fn writes(&self) -> Vec<(u32, u16)> {
            self.log
                .iter()
                .filter(|(w, _, _)| *w)
                .map(|(_, a, v)| (*a, *v))
                .collect()
        }

        /// Returns the subsequence of read entries: `(addr, val)`.
        // Used by bus-error frame tests in Task 11+.
        #[allow(dead_code)]
        pub fn reads(&self) -> Vec<(u32, u16)> {
            self.log
                .iter()
                .filter(|(w, _, _)| !*w)
                .map(|(_, a, v)| (*a, *v))
                .collect()
        }
    }

    impl crate::Bus for RecordingBus {
        fn read8(&mut self, addr: u32) -> u8 {
            let v = self.mem[(addr & 0xFFFF) as usize];
            // Record as a word-level entry with just the byte; callers that
            // care about byte vs word granularity should use the full log.
            self.log.push((false, addr & 0xFFFF, v as u16));
            v
        }
        fn read16(&mut self, addr: u32) -> u16 {
            let a = (addr & 0xFFFE) as usize;
            let v = u16::from_be_bytes([self.mem[a], self.mem[a + 1]]);
            self.log.push((false, addr & 0xFFFF, v));
            v
        }
        fn write8(&mut self, addr: u32, val: u8) {
            self.mem[(addr & 0xFFFF) as usize] = val;
            self.log.push((true, addr & 0xFFFF, val as u16));
        }
        fn write16(&mut self, addr: u32, val: u16) {
            let [hi, lo] = val.to_be_bytes();
            let a = (addr & 0xFFFE) as usize;
            self.mem[a] = hi;
            self.mem[a + 1] = lo;
            self.log.push((true, addr & 0xFFFF, val));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::{FlatBus, RecordingBus};
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

    /// Every address-mode family presents a 24-bit address to the `Bus`.
    ///
    /// ⚠️ **This is the only test in the workspace that can see `ADDR_MASK` being
    /// applied**, as opposed to seeing its value. Widening it to `0xFFFF_FFFF`
    /// left all 366 workspace tests and all 317,500 suite cases green, because
    /// every other bus masks the address a second time on the way in — see
    /// [`tests_support::StrictBus`]. Each row below drives an `A0`, `PC` or `SP`
    /// above `0x00FF_FFFF` through one family's `& ADDR_MASK` site.
    ///
    /// The expected addresses are **written out as literals**, never computed
    /// from `ADDR_MASK`, so the assertions cannot follow the constant if it
    /// moves. Verified per site by deleting each `& ADDR_MASK` individually,
    /// not just by widening the constant: the four `wrapping_add(2) & ADDR_MASK`
    /// sites in particular are only reachable from an `A0` at the *top* of the
    /// bus, which is why the wrapping rows below exist as well as the high ones.
    #[test]
    fn every_address_mode_family_presents_a_24_bit_address() {
        use super::tests_support::StrictBus;
        use crate::decode::Decoder;

        let dec = Decoder::new();

        // A high address register through each memory destination mode. The
        // program itself sits at a normal PC; only the operand address is high.
        // `want` is the operand address the bus must see, written out.
        let modes: &[(&str, &[u16], u32, u32)] = &[
            ("(A0)", &[0x3080], 0xFF00_2000, 0x0000_2000),
            ("(A0)+", &[0x30C0], 0xFF00_2000, 0x0000_2000),
            ("-(A0)", &[0x3100], 0xFF00_2002, 0x0000_2000),
            ("(d16,A0)", &[0x3140, 0x0010], 0xFF00_2000, 0x0000_2010),
            ("(d8,A0,D1)", &[0x3180, 0x1004], 0xFF00_2000, 0x0000_2004),
            ("(xxx).L", &[0x33C0, 0xFF00, 0x2000], 0, 0x0000_2000),
        ];
        for (name, prog, a0, want) in modes {
            let mut bus = StrictBus::new();
            bus.load(0x1000, prog);
            let mut cpu = M68k::new();
            cpu.sr = SR_S;
            cpu.a[0] = *a0;
            cpu.d[0] = 0xBEEF;
            cpu.d[1] = 0; // index register for the (d8,An,Xn) row
            cpu.pc = 0x1000;
            cpu.prime_prefetch(&mut bus);
            cpu.step_with(&dec, &mut bus);
            assert!(
                bus.seen.contains(want),
                "{name}: expected the masked operand address {want:#010X} on the \
                 bus, saw {:#010X?}",
                bus.seen
            );
            assert_eq!(bus.peek16(*want), 0xBEEF, "{name}: wrote the wrong place");
        }

        // A high PC: the prefetch refill, the pipeline advance, and the
        // PC-relative EA all mask.  `MOVE.W (d16,PC),D0` at 0xFF001000, with the
        // displacement word at 0xFF001002 and d16 = +0x10.
        let mut bus = StrictBus::new();
        bus.load(0x1000, &[0x303A, 0x0010, 0x4E71]);
        bus.put16(0x1012, 0xCAFE);
        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.pc = 0xFF00_1000;
        cpu.prime_prefetch(&mut bus);
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0] & 0xFFFF, 0xCAFE, "d16(PC) read the wrong place");
        assert!(
            bus.seen.contains(&0x0000_1012),
            "d16(PC): expected 0x00001012 on the bus, saw {:#010X?}",
            bus.seen
        );
        // The queue advance issued by the handler's own `fetch_word`, at the PC
        // *after* priming: 0xFF001004 masked.
        assert!(
            bus.seen.contains(&0x0000_1004),
            "the pipeline advance must mask too, saw {:#010X?}",
            bus.seen
        );

        // Long accesses whose *second* word crosses the top of the address bus.
        // At A0 = 0x00FFFFFE the high word sits at 0x00FFFFFE and the low word
        // wraps to 0 — the `a.wrapping_add(2) & ADDR_MASK` sites, which a merely
        // *high* A0 cannot reach because masking the base already fixes them.
        for (name, prog) in [
            ("MOVE.L D0,(A0)", &[0x2080u16][..]),
            ("MOVE.L (A0),D1", &[0x2210][..]),
        ] {
            let mut bus = StrictBus::new();
            bus.load(0x1000, prog);
            bus.put16(0x0000, 0x3344);
            let mut cpu = M68k::new();
            cpu.sr = SR_S;
            cpu.a[0] = 0x00FF_FFFE;
            cpu.d[0] = 0x1122_3344;
            cpu.pc = 0x1000;
            cpu.prime_prefetch(&mut bus);
            cpu.step_with(&dec, &mut bus);
            assert!(
                bus.seen.contains(&0x0000_0000),
                "{name}: the second word must wrap to 0x00000000, saw {:#010X?}",
                bus.seen
            );
        }

        // A high supervisor stack pointer: exception entry's frame pushes and
        // the vector fetch both mask.  `TRAP #0` is vector 32 at 0x80.
        let mut bus = StrictBus::new();
        bus.load(0x1000, &[0x4E40]);
        bus.load(0x0080, &[0x0000, 0x3000]);
        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0xFF00_2000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        cpu.step_with(&dec, &mut bus);
        assert!(
            bus.seen.contains(&0x0000_1FFA),
            "the frame's lowest push must land at 0x00001FFA, saw {:#010X?}",
            bus.seen
        );
        assert!(
            bus.seen.contains(&0x0000_0080),
            "the vector fetch must land at 0x00000080, saw {:#010X?}",
            bus.seen
        );

        // MOVEM and MOVEP carry their own masking sites, away from `ea`.
        // `MOVEM.w (A0),D0-D3` and `MOVEP.w D0,(d16,A0)`.
        for (name, prog, want) in [
            (
                "MOVEM.w (A0),D0-D3",
                &[0x4C90u16, 0x000F][..],
                0x0000_2000u32,
            ),
            ("MOVEP.w D0,(d16,A0)", &[0x0188, 0x0000][..], 0x0000_2000),
        ] {
            let mut bus = StrictBus::new();
            bus.load(0x1000, prog);
            let mut cpu = M68k::new();
            cpu.sr = SR_S;
            cpu.a[0] = 0xFF00_2000;
            cpu.pc = 0x1000;
            cpu.prime_prefetch(&mut bus);
            cpu.step_with(&dec, &mut bus);
            assert!(
                bus.seen.contains(&want),
                "{name}: expected {want:#010X} on the bus, saw {:#010X?}",
                bus.seen
            );
        }
    }

    /// `consume_opcode` shifts the queue, issues exactly one `read16` at the
    /// pre-call PC, and advances PC by 2.  The opcode value is NOT returned;
    /// the caller already has it from `step_with`'s peek.
    #[test]
    fn consume_opcode_shifts_queue_and_refills_slot_1() {
        let mut bus = RecordingBus::new();
        bus.put16(0x1004, 0x3333); // the word that will fill slot 1

        let mut cpu = M68k::new();
        cpu.pc = 0x1004;
        cpu.prefetch = [0x1111, 0x2222];

        cpu.consume_opcode(&mut bus);

        // slot 0 takes the old slot 1
        assert_eq!(cpu.prefetch[0], 0x2222, "prefetch[0] must be old slot 1");
        // slot 1 refilled by read16 at the pre-call PC (0x1004)
        assert_eq!(cpu.prefetch[1], 0x3333, "prefetch[1] must be refilled");
        // PC advanced by 2
        assert_eq!(cpu.pc, 0x1006, "PC must advance by 2");
        // exactly one bus access: the read16 at the pre-call PC
        assert_eq!(bus.log.len(), 1, "must issue exactly one bus access");
        assert_eq!(
            bus.log[0],
            (false, 0x1004, 0x3333),
            "must be read16 at pre-call PC"
        );
    }
}
