//! Instruction-sequence diagnostics.
//!
//! The vector suite tests each opcode in isolation from a synthetic state. These
//! tests run real sequences, catching drift and nesting bugs that per-opcode
//! testing structurally cannot see.
//!
//! **Why this file exists and what it proves.** Three execution paths in this
//! core have zero coverage in all 317,500 vector cases:
//!
//! - **Interrupts** — every case starts with no pending IRQ; IPL is not in the
//!   state format.  `interrupt_mid_loop_resumes` is therefore the *first*
//!   end-to-end evidence that `exception::check_interrupts` works at all.
//!   Treat a failure there as a real finding about the core, not a bad encoding.
//!
//! - **STOP resume** — STOP's access shape is empty and no vector case runs a
//!   second step.
//!
//! - **Trace exception** — vector 9 is fetched **0** times in all 317,500 cases,
//!   because each case runs exactly one instruction and stops at the boundary the
//!   trace check lives on.  Note the *handler* is what is uncovered, not the
//!   boundary: 158,894 of 317,500 initial states have T **set**, across all 127
//!   groups, so a core that took the trace before the boundary would fail ~38% of
//!   the suite rather than passing it.
//!
//! Encodings are hand-assembled and were verified against the measured layout in
//! `task-12-addendum.md` and `task-12-addendum-supplement.md` (every displacement
//! is confirmed by execution, and three opcode words absent from the suite —
//! `0x7000`, `0x203C`, `0x343C` — are sampling gaps, not bad encodings).

// An integration test is its own crate root, so the crate's `lib.rs` attribute
// does not reach here.
#![forbid(unsafe_code)]

use m68k::{decode::Decoder, Bus, M68k};

struct Ram {
    mem: Vec<u8>,
}

impl Ram {
    fn new() -> Self {
        Self {
            mem: vec![0; 0x10000],
        }
    }

    fn load(&mut self, addr: u32, words: &[u16]) {
        for (i, w) in words.iter().enumerate() {
            let a = (addr as usize) + i * 2;
            let [hi, lo] = w.to_be_bytes();
            self.mem[a] = hi;
            self.mem[a + 1] = lo;
        }
    }
}

impl Bus for Ram {
    fn read8(&mut self, a: u32) -> u8 {
        self.mem[(a & 0xFFFF) as usize]
    }
    fn read16(&mut self, a: u32) -> u16 {
        let i = (a & 0xFFFE) as usize;
        u16::from_be_bytes([self.mem[i], self.mem[i + 1]])
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.mem[(a & 0xFFFF) as usize] = v;
    }
    fn write16(&mut self, a: u32, v: u16) {
        let i = (a & 0xFFFE) as usize;
        let [h, l] = v.to_be_bytes();
        self.mem[i] = h;
        self.mem[i + 1] = l;
    }
}

/// Boots at 0x1000 with SSP 0x8000 and runs until STOP, a halt, or the step
/// bound — so a runaway program fails the test instead of hanging it.
fn run(prog: &[u16], extra: &[(u32, &[u16])]) -> (M68k, Ram) {
    let mut ram = Ram::new();
    ram.load(0x0000, &[0x0000, 0x8000, 0x0000, 0x1000]); // SSP, PC
    ram.load(0x1000, prog);
    for (addr, words) in extra {
        ram.load(*addr, words);
    }

    let dec = Decoder::new();
    let mut cpu = M68k::new();
    cpu.reset(&mut ram);

    let mut steps = 0;
    while steps < 100_000 && !cpu.stopped && !cpu.halted {
        cpu.step_with(&dec, &mut ram);
        steps += 1;
    }
    assert!(
        cpu.stopped,
        "program did not reach STOP within {steps} steps"
    );
    (cpu, ram)
}

/// Nested subroutines must unwind to the right depth. A stack that drifts by
/// even one word per call returns to garbage after enough nesting.
///
/// Layout (each entry is address / word / comment):
/// ```text
/// 1000  7000  moveq #0,d0
/// 1002  6100  bsr.w
/// 1004  0006    disp          ext@1004 + 6 = 100A  -> outer
/// 1006  4E72  stop
/// 1008  2700    #$2700
/// 100A  5240  addq.w #1,d0    <- outer
/// 100C  6100  bsr.w
/// 100E  0004    disp          ext@100E + 4 = 1012  -> inner
/// 1010  4E75  rts             <- outer's rts
/// 1012  5240  addq.w #1,d0    <- inner
/// 1014  4E75  rts             <- inner's rts
/// ```
#[test]
fn nested_bsr_returns_correctly() {
    let (cpu, _) = run(
        &[
            0x7000, // moveq #0,d0
            0x6100, 0x0006, // bsr.w  outer  (ext@0x1004 + 6 = to 0x100A)
            0x4E72, 0x2700, // stop   #$2700
            // outer @ 0x100A:
            0x5240, // addq.w #1,d0
            0x6100, 0x0004, // bsr.w  inner  (ext@0x100E + 4 = to 0x1012)
            0x4E75, // rts
            // inner @ 0x1012:
            0x5240, // addq.w #1,d0
            0x4E75, // rts
        ],
        &[],
    );
    assert_eq!(cpu.d[0], 2, "both subroutine bodies must run exactly once");
    assert_eq!(cpu.a[7], 0x8000, "stack must be fully unwound");
}

/// MOVEM must round-trip every register through the stack unchanged.
///
/// `0x48E7 / 0xFFFE`: MOVEM.l D0-D7/A0-A6,-(A7).
///   `-(An)` uses a reversed mask; bit 0 = A7.  `0xFFFE` clears bit 0 (A7 excluded
///   — correct, you must not push the stack pointer you're pushing through).
///
/// `0x4CDF / 0x7FFF`: MOVEM.l (A7)+,D0-D7/A0-A6.
///   Normal mask; bit 0 = D0.  `0x7FFF` sets bits 0-14, i.e. D0-D7/A0-A6.
///
/// The two masks are not bit-reversals of each other and are not supposed to be;
/// each is correct for its own addressing mode.
#[test]
fn movem_roundtrip_preserves_all_registers() {
    let (cpu, _) = run(
        &[
            0x203C, 0x1234, 0x5678, // move.l #$12345678,d0
            0x227C, 0x0000, 0x4000, // movea.l #$4000,a1
            0x48E7, 0xFFFE, // movem.l d0-d7/a0-a6,-(a7)
            0x7000, // moveq  #0,d0
            0x93C9, // suba.l a1,a1
            0x4CDF, 0x7FFF, // movem.l (a7)+,d0-d7/a0-a6
            0x4E72, 0x2700, // stop   #$2700
        ],
        &[],
    );
    assert_eq!(cpu.d[0], 0x1234_5678, "d0 must be restored");
    assert_eq!(cpu.a[1], 0x0000_4000, "a1 must be restored");
    assert_eq!(cpu.a[7], 0x8000, "stack must be balanced");
}

/// A TRAP handler must return to the instruction after the TRAP.
///
/// TRAP #0 → vector 32, at address 0x80 (32 × 4 = 0x80).
/// The stacked PC is `opcode_addr + 2`, so RTE resumes at the `addq` after TRAP.
/// If d0 == 1: RTE resumed at TRAP itself (infinite-loop-turned-STOP).
/// If d0 == 3: something ran twice.
#[test]
fn trap_and_rte_resume_correctly() {
    let mut vectors = [0u16; 2];
    vectors[0] = 0x0000;
    vectors[1] = 0x2000;
    let (cpu, _) = run(
        &[
            0x7000, // moveq #0,d0
            0x4E40, // trap  #0
            0x5240, // addq.w #1,d0   <- must execute after RTE
            0x4E72, 0x2700, // stop  #$2700
        ],
        &[
            // TRAP #0 is vector 32, at address 0x80.
            (0x0080, &vectors),
            (0x2000, &[0x5240, 0x4E73]), // addq.w #1,d0 ; rte
        ],
    );
    assert_eq!(
        cpu.d[0], 2,
        "handler and the instruction after TRAP must both run"
    );
    assert_eq!(cpu.a[7], 0x8000, "the exception frame must be fully popped");
}

/// An interrupt raised mid-loop must be taken, then the loop resumed.
///
/// This test is the *sole* end-to-end evidence that `exception::check_interrupts`
/// works.  The 317,500-case vector suite has zero autovector cases — IPL is not
/// part of the state format — so a failure here is a real finding about the core.
///
/// The clear-on-`d1==1` logic fires after every step, so if the handler's `addq`
/// and its `rte` are separate steps the IRQ line is cleared between them.  That
/// is fine: the raise to mask-level-4 during the handler body already prevents
/// re-entry; clearing the line here just ensures a second look after `rte` also
/// does nothing.
///
/// **Which assertion catches which class of bug.**  A masking bug and a nesting
/// bug are indistinguishable from `d1` alone, so the two assertions split them:
///
/// | observation | what failed |
/// |---|---|
/// | `d1 > 1` | **masking**: entry did not raise the mask to the serviced level, so the interrupt was re-taken before the line was cleared |
/// | `d1 == 0` | **recognition**: `check_interrupts` never fired at all |
/// | `a7 != 0x8000` | **nesting**: the frame was not stacked or not fully popped, independent of how many times the handler ran |
///
/// The test is sensitive to all three: disabling interrupt recognition and
/// removing the mask raise each fail it, on the `d1` and `a7` assertions
/// respectively.  That is why both are asserted rather than `d1` alone.
///
/// Layout:
/// ```text
/// 1000  343C  move.w #100,d2   d1 counts handler entries
/// 1002  0064    immediate 100
/// 1004  5342  subq.w #1,d2     <- loop target
/// 1006  66FC  bne.s -4         base=0x1006+2=0x1008; 0x1008+(-4)=0x1004. Correct.
/// 1008  4E72  stop
/// 100A  2700    #$2700
/// ```
#[test]
fn interrupt_mid_loop_resumes() {
    let mut ram = Ram::new();
    ram.load(0x0000, &[0x0000, 0x8000, 0x0000, 0x1000]);
    // Level 4: VEC_AUTOVECTOR_BASE (24) + 4 = vector 28, at address 28 * 4 = 0x70.
    ram.load(0x0070, &[0x0000, 0x2000]);
    ram.load(0x2000, &[0x5241, 0x4E73]); // addq.w #1,d1 ; rte
    ram.load(
        0x1000,
        &[
            0x343C, 0x0064, // move.w #100,d2   (d1 counts handler entries)
            0x5342, // subq.w #1,d2       <- loop target @ 0x1004
            0x66FC, // bne.s -4           base=0x1008; targets 0x1004. Correct.
            0x4E72, 0x2700, // stop #$2700
        ],
    );

    let dec = Decoder::new();
    let mut cpu = M68k::new();
    cpu.reset(&mut ram);
    cpu.d[1] = 0;
    // Supervisor with mask 0, so a level-4 IRQ is taken.
    // Route through set_sr so the USP/SSP swap logic runs if needed
    // (the S bit is already set after reset so no swap occurs here).
    cpu.set_sr(cpu.sr & !0x0700);

    let mut steps = 0;
    let mut fired = false;
    // `!cpu.halted` matches the shared `run()` helper: a double bus fault would
    // otherwise spin to the step cap and fail on `stopped` with a misleading message
    // instead of revealing the halt.
    while steps < 100_000 && !cpu.stopped && !cpu.halted {
        // Raise the IRQ once, partway through the loop.
        if steps == 20 && !fired {
            cpu.set_irq(4);
            fired = true;
        }
        cpu.step_with(&dec, &mut ram);
        // The handler runs once; clear the line so it is not re-taken.
        if fired && cpu.d[1] == 1 {
            cpu.set_irq(0);
        }
        steps += 1;
    }
    assert!(cpu.stopped, "loop must still terminate after the interrupt");
    assert_eq!(cpu.d[1], 1, "the handler must have run exactly once");
    assert_eq!(cpu.a[7], 0x8000, "the interrupt frame must be fully popped");
}

/// DBcc must iterate exactly count+1 times: it exits when the counter reaches
/// -1, not 0, which is the classic off-by-one in a 68000 core.
///
/// Layout:
/// ```text
/// 1000  7000  moveq #0,d0
/// 1002  323C  move.w #9,d1
/// 1004  0009    immediate 9
/// 1006  5240  addq.w #1,d0     <- loop body
/// 1008  51C9  dbra d1
/// 100A  FFFC    disp -4         base=0x100A+(-4)=0x1006 (the addq body). Correct.
/// 100C  4E72  stop
/// 100E  2700    #$2700
/// ```
#[test]
fn dbcc_loop_iterates_exactly() {
    let (cpu, _) = run(
        &[
            0x7000, // moveq #0,d0
            0x323C, 0x0009, // move.w #9,d1
            0x5240, // addq.w #1,d0     <- loop body @ 0x1006
            0x51C9, 0xFFFC, // dbra   d1,-4  (ext@0x100A; targets 0x1006)
            0x4E72, 0x2700, // stop   #$2700
        ],
        &[],
    );
    assert_eq!(cpu.d[0], 10, "dbra with #9 must run the body 10 times");
    assert_eq!(cpu.d[1] as u16, 0xFFFF, "the counter must end at -1");
}
