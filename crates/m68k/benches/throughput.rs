//! Confirms the core is fast enough for CPS-1, which clocks its 68000 at
//! 10 MHz. A mixed instruction workload, not a NOP loop: the latter would
//! flatter the jump table and tell us nothing useful.
//!
//! # The `assert!` below is a liveness smoke test, not a performance gate
//!
//! It passes at a wide margin — **on the author's machine**, 719-820 MHz over
//! nine runs, a 72x-82x margin, where the spread within that band is host load
//! and the low end is reproducibly the first run after a build. So it will not
//! catch a 5x regression, or a 20x one. Quote the range rather than one sample —
//! and note that of the three figures printed below, only
//! `cycles/instruction` is stable, since it comes from the cycle model and not
//! from the wall clock.
//!
//! ⚠️ **719-820 MHz is a floor to read as "a large margin on one host", not a
//! band this bench holds anywhere.** This paragraph used to give the range with
//! no host caveat, while `README.md` correctly said "on the author's machine". A
//! reviewer's host measured **267.6 MHz, a 27x margin** — outside the band by 3x
//! — and the test **correctly still passed**, because the assertion is
//! `>= 10.0`, three orders of magnitude below either figure. That is the design
//! working: do not turn the assert into a margin threshold, because a threshold
//! anywhere inside the band would fail on a slower but perfectly adequate host.
//! The number to treat as durable is `cycles/instruction`.
//!
//! What it catches is "the core stopped executing" — and, with the
//! non-degeneracy census in [`assert_workload_is_mixed`], "the core is still
//! executing but no longer executing *this*". A throughput figure is
//! meaningless until the workload is known non-degenerate: the same MHz prints
//! just as happily for a one-instruction spin loop or an exception loop that
//! never leaves the vector. Read a green bench as evidence of liveness and of
//! the mix, never as a performance guarantee.

// A bench is its own crate root, so `lib.rs`'s attribute does not reach here — and a
// benchmark is exactly where a reach for `unsafe` to shave a nanosecond would land.
#![forbid(unsafe_code)]

use m68k::{decode::Decoder, Bus, M68k};
use std::collections::BTreeSet;
use std::time::Instant;

struct Ram(Vec<u8>);

impl Bus for Ram {
    fn read8(&mut self, a: u32) -> u8 {
        self.0[(a as usize) & 0xFFFF]
    }
    fn read16(&mut self, a: u32) -> u16 {
        let i = (a as usize) & 0xFFFE;
        u16::from_be_bytes([self.0[i], self.0[i + 1]])
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.0[(a as usize) & 0xFFFF] = v;
    }
    fn write16(&mut self, a: u32, v: u16) {
        let i = (a as usize) & 0xFFFE;
        let [h, l] = v.to_be_bytes();
        self.0[i] = h;
        self.0[i + 1] = l;
    }
}

/// The loop mixing register ops, memory access, and a taken branch.
///
/// The `bra` targets `0x1002`, **not** `0x1000`: a branch's displacement base is
/// `opcode_addr + 2`, so from the `bra` at `0x1010` a `disp8` of `-16` reaches
/// `0x1012 - 16 = 0x1002`. Reaching `0x1000` would need `-18`. So `moveq`
/// executes exactly once, as loop entry, and the steady-state body is six
/// instructions.
///
/// This is deliberately left as-is. The mix is a good one either way — register
/// ops, two memory accesses, a shift, and a taken branch — and changing the
/// displacement would invalidate every figure ever recorded from this bench.
const PROG: &[u16] = &[
    0x7001, // moveq #1,d0            0x1000, entered once
    0xD081, // add.l  d1,d0           0x1002, the branch target
    0x2200, // move.l d0,d1
    0x3140, 0x0100, // move.w d0,(0x100,a0)
    0x3228, 0x0100, // move.w (0x100,a0),d1
    0xE288, // lsr.l  #1,d0
    0x60F0, // bra    0x1002
];

const PROG_BASE: u32 = 0x1000;
const PROG_LAST: u32 = PROG_BASE + (PROG.len() as u32 - 1) * 2;

/// The control the throughput number needs: proof that the workload is mixed.
///
/// Censuses the PCs visited and the per-instruction cycle costs over a couple of
/// laps. A spin loop shows one PC and one cost; a runaway exception shows a PC
/// outside the program. Both would leave the MHz figure looking perfectly
/// healthy, which is why this is a separate check and not a comment.
///
/// **Call this before the warm-up loop, not after.** The expected PC count
/// includes `moveq`'s single loop-entry visit, which is only reachable from a
/// freshly primed queue.
fn assert_workload_is_mixed(cpu: &mut M68k, dec: &Decoder, ram: &mut Ram) {
    let mut pcs = BTreeSet::new();
    let mut costs = BTreeSet::new();
    for _ in 0..40 {
        let pc = cpu.pc - 4; // pc runs 4 bytes ahead of the executing word
        assert!(
            (PROG_BASE..=PROG_LAST).contains(&pc),
            "left the program at {pc:06X}: the workload is not the one being measured"
        );
        pcs.insert(pc);
        costs.insert(cpu.step_with(dec, ram));
    }
    assert_eq!(
        pcs.len(),
        PROG.len() - 2, // two of the nine words are extension words, so 7 instructions
        "expected every instruction in the loop to be reached, got {pcs:02X?}"
    );
    assert!(
        costs.len() > 1,
        "one cycle cost across the whole loop means a degenerate workload: {costs:?}"
    );
    println!(
        "control: {} distinct PCs, {} distinct cycle costs {:?}",
        pcs.len(),
        costs.len(),
        costs
    );
}

fn main() {
    let mut ram = Ram(vec![0; 0x10000]);
    for (i, w) in PROG.iter().enumerate() {
        let [h, l] = w.to_be_bytes();
        ram.0[PROG_BASE as usize + i * 2] = h;
        ram.0[PROG_BASE as usize + i * 2 + 1] = l;
    }

    let dec = Decoder::new();
    let mut cpu = M68k::new();
    cpu.pc = PROG_BASE;
    cpu.prime_prefetch(&mut ram);

    assert_workload_is_mixed(&mut cpu, &dec, &mut ram);

    // Warm up, then measure simulated cycles per wall-clock second.
    for _ in 0..1_000_000 {
        cpu.step_with(&dec, &mut ram);
    }
    // A halted or stopped CPU still steps and still returns a cycle count, so
    // both would benchmark at a plausible-looking rate while executing nothing.
    assert!(!cpu.halted, "the CPU halted during warm-up");
    assert!(!cpu.stopped, "the CPU stopped during warm-up");

    let iters = 20_000_000u64;
    let start = Instant::now();
    let mut cycles = 0u64;
    for _ in 0..iters {
        cycles += cpu.step_with(&dec, &mut ram) as u64;
    }
    let secs = start.elapsed().as_secs_f64();
    let mhz = cycles as f64 / secs / 1e6;

    assert!(!cpu.halted, "the CPU halted during the measured run");
    assert!(!cpu.stopped, "the CPU stopped during the measured run");
    // A single-instruction spin would make the total an exact multiple of the
    // instruction count. The real loop averages 9.33 cycles/instruction.
    assert!(
        !cycles.is_multiple_of(iters),
        "{cycles} cycles over {iters} instructions is an exact multiple — \
         the loop has collapsed to one instruction"
    );

    println!("{iters} instructions, {cycles} simulated cycles in {secs:.3}s");
    println!("=> {mhz:.1} MHz simulated (CPS-1 needs 10.0)");
    println!(
        "   margin {:.0}x; mean {:.2} cycles/instruction",
        mhz / 10.0,
        cycles as f64 / iters as f64
    );
    assert!(mhz >= 10.0, "too slow for CPS-1: {mhz:.1} MHz");
}
