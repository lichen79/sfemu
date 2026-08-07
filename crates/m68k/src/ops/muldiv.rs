//! `MULU`, `MULS`, `DIVU`, `DIVS` and `CHK`.
//!
//! Grouped because they share a shape the rest of the core does not: a word
//! `<ea>` source against a `Dn` destination, and a cycle count that depends on
//! the *data* rather than only on the addressing mode. Everything here rides
//! `alu::Plan`, so the bus schedule and the address-error behaviour are the
//! same model as Task 6's; only the arithmetic and the idle term are new.
//!
//! # Bus schedules
//!
//! ```text
//! MULU/MULS       [ea] P i          idle last
//! DIVU/DIVS       [ea] i P          idle before the queue advance
//! CHK, no trap    [ea] i P
//! CHK, trapping   [ea] i W W W R R P i P
//! ```
//!
//! Idle placement is unobservable — the harness matches accesses in order and
//! idles only through the total — so [`alu`] models the total alone and the
//! difference between rows 1 and 2 above costs nothing to ignore.
//!
//! # Timing
//!
//! Every formula here is closed-form and was verified against the suite over
//! **all** addressing modes, not just the register source. Each returns the
//! instruction's *own* idle; `alu::run_tail` adds the standard `+2` for modes
//! 4, 6 and (7,3) on top.
//!
//! ```text
//! MULU  1501/1501     MULS  1548/1548     DIVU  1546/1546
//! DIVS  1518/1518     CHK   1523/1523
//! ```
//!
//! The multiply base is **34**, not the manual's familiar 38: the published
//! number is a total including the opcode fetch, which the timing law already
//! charges 4 for. Every case misses by exactly 4 with the wrong base, which
//! under the law's diagnostic corollary reads as a miscounted *access* — worth
//! knowing, because that is what it would look like if the base were right and
//! the schedule wrong.
//!
//! # Divide by zero is NOT covered by the suite
//!
//! There is no divide-by-zero case in either group: 0 of 1,546 `DIVU` and 0 of
//! 1,518 `DIVS`, against a control that recovers 1,525 and 1,502 *distinct*
//! nonzero divisors from the same code — so the query can see a divisor, and the
//! absence is a property of the data. Nor is there a 3-write case anywhere in
//! either group to compare a frame against.
//!
//! So `divide_by_zero`'s vector, stacked PC and cycle count are
//! **extrapolated** from `CHK` (measured, same task) and `JSR` (measured, Task
//! 8), and the unit tests in this module are their only coverage. A green
//! `DIVU`/`DIVS` group says nothing about that path. See
//! `docs/hardware/68000-notes.md`.

use super::alu::{self, Ops, Plan, Tail};
use super::arith::valid_src;
use crate::cpu::M68k;
use crate::decode::Handler;
use crate::ea::Size;
use crate::exception::{self, VEC_CHK, VEC_DIVIDE_BY_ZERO};
use crate::Bus;

// --- Multiply --------------------------------------------------------------

/// `MULU`'s idle cycles: 34 plus 2 per set bit in the 16-bit source.
#[inline]
fn mulu_idle(src: u16) -> u32 {
    34 + 2 * src.count_ones()
}

/// `MULS`'s idle cycles: 34 plus 2 per `01`/`10` bit-pair in `src << 1`.
///
/// That is the count of adjacent-bit *differences*, which is what the Booth
/// recoder costs a cycle for. The `<< 1` supplies the implicit zero below bit 0,
/// so an odd source pays for the transition into it.
#[inline]
fn muls_idle(src: u16) -> u32 {
    let v = (src as u32) << 1;
    34 + 2 * ((v ^ (v >> 1)) & 0xFFFF).count_ones()
}

/// `MULU`/`MULS`: 16 × 16 → 32 into the whole of `Dn`.
///
/// `X` is untouched, `V` and `C` are cleared, and `N`/`Z` come from the full
/// 32-bit product — not from its low word.
fn mul(cpu: &mut M68k, bus: &mut dyn Bus, op: u16, signed: bool) -> u32 {
    let (mode, reg) = ((op >> 3) & 7, op & 7);
    let dn = ((op >> 9) & 7) as usize;
    // The idle term needs the source, which does not exist until the operand
    // read inside `run`; so the plan is built with a placeholder and the real
    // value is added to the returned total afterwards.
    let mut own_idle = 0;
    let plan = Plan::new(Size::Word, mode, reg);
    let cycles = alu::run(cpu, bus, &plan, &mut |cpu, ops| {
        let src = ops.ea as u16;
        let dst = cpu.d[dn] as u16;
        let result = if signed {
            ((src as i16 as i32) * (dst as i16 as i32)) as u32
        } else {
            (src as u32) * (dst as u32)
        };
        own_idle = if signed {
            muls_idle(src)
        } else {
            mulu_idle(src)
        };
        cpu.d[dn] = result;
        cpu.set_ccr(
            cpu.ccr_x(),
            result & 0x8000_0000 != 0,
            result == 0,
            false,
            false,
        );
        None
    });
    cycles + own_idle
}

// --- Divide ----------------------------------------------------------------

/// `DIVU`'s idle cycles, by simulating the shift-subtract loop.
///
/// The early return is the overflow abort, and its predicate is the *pre-shift*
/// comparison `dividend >> 16 >= divisor`, which for unsigned division is
/// equivalent to `quotient > 0xFFFF` — 1546/1546 with zero disagreements
/// between the two. For `DIVS` the corresponding predicates are **not**
/// equivalent; see [`divs_idle`].
///
/// # Panics
///
/// Never called with `divisor == 0`: the caller routes that to vector 5 first.
/// It would not divide by zero here anyway — the loop only shifts and subtracts
/// — but the shortcut's `>= 0` would be trivially true and return a cycle count
/// for an instruction that does not run.
fn divu_idle(dividend: u32, divisor: u16) -> u32 {
    debug_assert_ne!(divisor, 0, "the zero divisor must be trapped before here");
    if (dividend >> 16) >= divisor as u32 {
        return 6;
    }
    let mut idle: u32 = 72;
    let mut dvd = dividend;
    let dvs = (divisor as u32) << 16;
    for _ in 0..15 {
        let old = dvd;
        dvd <<= 1;
        if (old as i32) < 0 {
            // The MSB was already set, so the subtraction is unconditional and
            // costs nothing — the trial comparison is not needed.
            dvd = dvd.wrapping_sub(dvs);
        } else {
            idle += 4;
            if dvd >= dvs {
                dvd = dvd.wrapping_sub(dvs);
                idle -= 2;
            }
        }
    }
    idle
}

/// `DIVS`'s idle cycles.
///
/// Two things here look wrong and are measured:
///
/// - **The overflow shortcut is keyed on the absolute values**, and it is *not*
///   the same predicate as the `V` flag's. `|dividend| >> 16 >= |divisor|` and
///   `quotient` out of `i16` range **disagree on 402 of 1,518 cases**, and the
///   disagreement is one-sided: the shortcut is *strictly weaker*, firing on 760
///   cases to `V`'s 1,162, with **zero** cases where it fires and `V` is clear.
///
///   So those 402 are a genuine **late overflow**: the division runs to
///   completion, the quotient turns out not to fit, and the instruction pays the
///   full slow cost while still setting `V` and leaving `Dn` alone. Measured
///   directly on the register-source subset — 99 such cases, `V` set 99/99, `Dn`
///   unchanged 99/99, cycles in the slow 126..146 range, never the fast 16/18.
///
///   Using the shortcut as the `V` flag loses 402 `V` flags; using `V` as the
///   shortcut charges 402 cases the fast cost and misses by ~120 cycles, which
///   reads as a broken constant rather than a wrong predicate. `DIVU`'s two
///   predicates *are* equivalent (0 disagreements of 1,546), which is what makes
///   computing one and reusing it pass `DIVU` completely.
/// - **The base is a function of the sign pair, not of the quotient's sign.** A
///   negative-quotient rule coincides with the truth on three of the four pairs
///   and scores 1240/1442 — 86%, which reads as "nearly right" and is the wrong
///   variable. Each pair has exactly one base: 116 / 118 / 120 / 122.
///
/// The iteration term is two cycles per **zero** bit in the low 15 of
/// `|quotient| >> 1` — the opposite polarity to `MULU`'s.
///
/// # Panics
///
/// Never called with `divisor == 0`; `dvd_abs / dvs_abs` would divide by zero
/// and panic in debug, which is why the caller must trap first.
fn divs_idle(dividend: u32, divisor: u16) -> u32 {
    debug_assert_ne!(divisor, 0, "the zero divisor must be trapped before here");
    let dvd_neg = (dividend as i32) < 0;
    // The divisor's sign bit is bit 15, not bit 31 — it is the word operand.
    // Reading it as i32 works whenever the high word happens to be zero, which
    // is what makes that a silent bug rather than a loud one.
    let dvs_neg = (divisor as i16) < 0;
    let dvd_abs = (dividend as i32).unsigned_abs();
    let dvs_abs = (divisor as i16).unsigned_abs() as u32;

    if (dvd_abs >> 16) >= dvs_abs {
        // Sign handling has already happened by the time the check fires, so an
        // overflowing DIVS is not one fixed cost the way DIVU's is.
        return 12 + if dvd_neg { 2 } else { 0 };
    }

    let mut idle: u32 = 116;
    if dvs_neg {
        idle += 2;
    }
    if dvd_neg {
        idle += 6;
    }
    if dvd_neg && dvs_neg {
        idle -= 4;
    }
    let qq = (dvd_abs / dvs_abs) >> 1;
    for i in 0..15 {
        if qq & (1 << i) == 0 {
            idle += 2;
        }
    }
    idle
}

/// Takes the divide-by-zero trap: vector 5.
///
/// **Extrapolated, not measured** — the suite contains no divide-by-zero case in
/// either group. Both choices are carried over from rules measured elsewhere in
/// this task and in Task 8:
///
/// - the stacked PC is `opcode_addr + 2 * (1 + ext_words)`, i.e. past the whole
///   instruction, which is `CHK`'s rule (1326/1326) and `JSR`'s (2570/2570). A
///   fixed `opcode_addr + 2` is measured *wrong* for both.
/// - the cycle count is the schedule so far plus the short frame's 7 accesses,
///   which is `CHK`'s trapping rule, plus 10 idle. `CHK`'s trapping idle is
///   **measured, and it is not a single value**: 10 in 204 cases and 12 in 101,
///   selected by [`chk_idle`]'s predicate on the operands. Borrowing 10 picks
///   one of two measured arms with no evidence for which arm a zero divisor
///   takes, so the uncertainty is 10-vs-12. Nothing in the repo can settle it.
///
/// The destination register is left completely unchanged.
fn divide_by_zero(cpu: &mut M68k, bus: &mut dyn Bus, ops: &Ops) -> Tail {
    exception::take(cpu, bus, VEC_DIVIDE_BY_ZERO, ops.trap_pc);
    Tail::Trapped
}

/// `DIVU`/`DIVS`: 32 ÷ 16, remainder in the high word and quotient in the low.
///
/// Three exits, and the suite covers only two of them:
///
/// | divisor | quotient fits | effect                                    |
/// |---------|---------------|-------------------------------------------|
/// | 0       | —             | vector 5; `Dn` unchanged (*extrapolated*) |
/// | nonzero | no            | `V` set, `Dn` unchanged, no exception     |
/// | nonzero | yes           | `Dn = (rem << 16) \| quot`                |
///
/// On the overflow path `N` is **set** and `Z` **clear**, unanimously in both
/// groups (791/791 and 1162/1162) — not preserved, which is what the brief's
/// sketch does. `C` is cleared and `X` kept on every path.
fn div(cpu: &mut M68k, bus: &mut dyn Bus, op: u16, signed: bool) -> u32 {
    let (mode, reg) = ((op >> 3) & 7, op & 7);
    let dn = ((op >> 9) & 7) as usize;
    let mut own_idle = 0;
    let plan = Plan::new(Size::Word, mode, reg);
    let cycles = alu::run_tail(cpu, bus, &plan, &mut |cpu, bus, ops| {
        let divisor = ops.ea as u16;
        let dividend = cpu.d[dn];
        if divisor == 0 {
            // EXTRAPOLATED, zero suite cases: the manual gives zero-divide 38
            // cycles total, and 38 - 4 * SHORT_FRAME_ACCESSES = 10 idle.
            //
            // 10 is NOT an outlier — it is measured, on CHK. Of the 7-access
            // short-frame cases (ssp -= 6), CHK contributes 204 at idle 10 and
            // 101 at idle 12, i.e. exactly 38 and 40 cycles; the twelve groups
            // that sit at idle 6 are the ones with no operand comparison to
            // make (ANDItoSR, EORItoSR, ILLEGAL_LINEA/F, MOVEfromUSP,
            // MOVEtoSR, MOVEtoUSP, ORItoSR, RESET, RTE, STOP, TRAP), 18,776
            // cases. So the family is not uniform at 6, and 38 is a value the
            // suite actually produces on the trap that DIV's rule is borrowed
            // from.
            //
            // The real uncertainty is 10 vs 12, not 10 vs 6: chk_idle picks
            // between them on a predicate about the operands, and nothing says
            // a zero divisor lands on the 10 arm. Do not "simplify" this to 6
            // for family consistency — that value belongs to the no-comparison
            // groups, not to this one.
            own_idle = 10;
            return divide_by_zero(cpu, bus, &ops);
        }
        own_idle = if signed {
            divs_idle(dividend, divisor)
        } else {
            divu_idle(dividend, divisor)
        };
        let (quot, rem, overflow) = if signed {
            // wrapping_*: -0x8000_0000 / -1 overflows i32, and a plain `/`
            // would panic in debug on guest data. That case overflows the
            // quotient anyway, so the wrapped value is never written.
            let d = dividend as i32;
            let s = divisor as i16 as i32;
            let q = d.wrapping_div(s);
            (
                q as u32,
                d.wrapping_rem(s) as u32,
                !(-32768..=32767).contains(&q),
            )
        } else {
            let q = dividend / divisor as u32;
            (q, dividend % divisor as u32, q > 0xFFFF)
        };
        if overflow {
            cpu.set_ccr(cpu.ccr_x(), true, false, true, false);
        } else {
            cpu.d[dn] = ((rem & 0xFFFF) << 16) | (quot & 0xFFFF);
            cpu.set_ccr(
                cpu.ccr_x(),
                quot & 0x8000 != 0,
                quot & 0xFFFF == 0,
                false,
                false,
            );
        }
        Tail::Done
    });
    cycles + own_idle
}

// --- CHK -------------------------------------------------------------------

/// `CHK`'s own idle cycles.
///
/// The trapping cost is **not** "12 if the value is negative else 10" — that
/// scores 225/305, 74%, which is the same wrong-variable signature as `DIVS`'s
/// base. The discriminating variable is whether `value - bound` overflows `i16`,
/// the only one of eight candidates that bucketed unanimously. Six buckets, all
/// singletons, totalling the corpus.
#[inline]
fn chk_idle(value: i32, bound: i32) -> u32 {
    let neg = value < 0;
    let gt = value > bound;
    if !neg && !gt {
        return 6;
    }
    // i64: `value` spans the whole of `i32` and `bound` is sign-extended from a
    // word, so `0x7FFF_FFFF - -32768` overflows `i32` and would panic in debug on
    // guest data. Widening keeps the comparison exact rather than wrapping it.
    let diff = value as i64 - bound as i64;
    let overflows_word = !(-32768..=32767).contains(&diff);
    if neg && !gt && !overflows_word {
        12
    } else {
        10
    }
}

/// `CHK.w Dn,<ea>`: trap through vector 6 if `Dn` is negative or exceeds the
/// bound.
///
/// Both operands are **signed** words: `value < 0 || value > bound` scores
/// 1523/1523 against an unsigned reading's 232/376 on the register form, and on
/// the 144 discriminating cases the signed rule takes all 144 and the unsigned
/// rule none.
///
/// One CCR rule covers both paths — `N` and `Z` from the *tested value*, `V` and
/// `C` cleared, `X` kept. On the trapping path that is what the frame's SR
/// holds (1523/1523 read from the stacked word). 85 trapping cases show an SR
/// equal to the initial one, but those are exactly the cases where the two
/// readings coincide, so they are not evidence for "untouched".
fn chk(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let (mode, reg) = ((op >> 3) & 7, op & 7);
    let dn = ((op >> 9) & 7) as usize;
    let mut own_idle = 0;
    let plan = Plan::new(Size::Word, mode, reg);
    alu::run_tail(cpu, bus, &plan, &mut |cpu, bus, ops| {
        let bound = ops.ea as u16 as i16 as i32;
        let value = cpu.d[dn] as u16 as i16 as i32;
        own_idle = chk_idle(value, bound);
        // The CCR is set before the frame is written, so the stacked SR carries
        // it. Ordering these the other way passes the non-trapping path and
        // fails every trapping one.
        cpu.set_ccr(cpu.ccr_x(), value < 0, value == 0, false, false);
        if value < 0 || value > bound {
            exception::take(cpu, bus, VEC_CHK, ops.trap_pc);
            Tail::Trapped
        } else {
            Tail::Done
        }
    }) + own_idle
}

// --- Dispatch-table installation ------------------------------------------

macro_rules! handlers {
    ($($name:ident = $body:path $(, $arg:expr)*;)*) => {
        $(fn $name(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
            $body(cpu, bus, op $(, $arg)*)
        })*
    };
}

handlers! {
    mulu = mul, false;
    muls = mul, true;
    divu = div, false;
    divs = div, true;
}

/// Installs `MULU`, `MULS`, `DIVU`, `DIVS` and `CHK`.
///
/// All five are `<ea>`-source-to-`Dn` forms selected by opmode (bits 8-6) on
/// lines `0x8`, `0xC` and `0x4`:
///
/// ```text
/// 1100 dddd 011m mmrr   MULU   opmode 3 on line C
/// 1100 dddd 111m mmrr   MULS   opmode 7
/// 1000 dddd 011m mmrr   DIVU   opmode 3 on line 8
/// 1000 dddd 111m mmrr   DIVS   opmode 7
/// 0100 dddd 110m mmrr   CHK    opmode 6 on line 4
/// ```
///
/// Opmode alone is not enough on lines 8 and C: `mode == 0` or `mode == 1` at
/// **opmode 4** is `SBCD`/`ABCD` and at opmodes 5-7 is `EXG`, both of which live
/// in [`super::bcd`] and [`super::move_`]. Those opmodes are simply not touched
/// here, so the split needs no coordination — but `MULU`/`DIVU` at opmode 3 do
/// overlap nothing, and the two `Later` arms in `opcode_space.rs` that used to
/// cover this are now `Mine`.
///
/// The `<ea>` is a source in all five, so the `An` mode is excluded (it is
/// `EXG`'s encoding here, and word-sized `An` source would be legal otherwise)
/// and mode 7 stops at the immediate. `CHK` shares its exclusion with the other
/// four despite living on a different line.
pub(super) fn register(table: &mut [Handler; 65536]) {
    for dn in 0..8u16 {
        for mode in 0..8u16 {
            for reg in 0..8u16 {
                // Word-sized `valid_src` rejects mode 1 (`An`) only at byte
                // size, so the `An` exclusion is spelt out rather than
                // delegated: none of these five accepts it.
                if mode == 1 || !valid_src(mode, reg, Size::Word) {
                    continue;
                }
                let ea = (mode << 3) | reg;
                let slot = (dn << 9) | ea;
                table[(0xC000 | slot | (3 << 6)) as usize] = mulu;
                table[(0xC000 | slot | (7 << 6)) as usize] = muls;
                table[(0x8000 | slot | (3 << 6)) as usize] = divu;
                table[(0x8000 | slot | (7 << 6)) as usize] = divs;
                table[(0x4000 | slot | (6 << 6)) as usize] = chk;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::{FlatBus, RecordingBus};
    use crate::cpu::{SR_C, SR_N, SR_S, SR_V, SR_X, SR_Z};
    use crate::decode::Decoder;

    /// Runs one instruction from `0x1000` and returns the cycle count.
    fn run_one(cpu: &mut M68k, bus: &mut FlatBus, words: &[u16]) -> u32 {
        bus.load(0x1000, words);
        cpu.pc = 0x1000;
        cpu.prime_prefetch(bus);
        let dec = Decoder::new();
        cpu.step_with(&dec, bus)
    }

    /// A halted `CHK` trap is charged for its lead and nothing else.
    ///
    /// This is `alu::run_tail`'s [`Tail::Trapped`] arm, and the one site in the
    /// family whose framed constant is a *frame* rather than a fault tail: it
    /// added `4 * SHORT_FRAME_ACCESSES` unconditionally, so a double bus fault was
    /// charged 28 cycles for a frame and vector fetch it never performed. `acc`
    /// and the idle stay outside [`exception::entry_cycles`] because they did
    /// happen; `SHORT_FRAME_ACCESSES` does not.
    ///
    /// The register form is the zero-access case, and its `chk_idle` is 12 here —
    /// value negative, not greater than the bound, and the difference fits a word
    /// — so the expected cost is `12 + 4`. Taking `chk_idle`'s value from the
    /// condition rather than assuming its most common 10 matters: the framed cost
    /// would be 40, and 40 − 28 is 12, so a wrong idle would have looked like a
    /// correct fix.
    ///
    /// Extrapolated: 0 of 317,500 cases halt. Note this arm is reachable *only*
    /// through a trap, so it is the one halt path in the crate that is not an
    /// address error.
    #[test]
    fn a_halted_chk_trap_costs_only_its_lead() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x4181, 0x4E71]); // CHK.w D1,D0 — bound D1, value D0
        bus.put16(0x0018, 0x0000); // vector 6, so a frame would be visible
        bus.put16(0x001A, 0x2000);
        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.d[0] = 0xFFFF_FFFF; // value: negative, so it traps
        cpu.d[1] = 1; // bound
        cpu.a[7] = 0x3001; // odd frame base
        cpu.ssp = 0x3001;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(&mut bus);
        bus.log.clear();

        let cycles = cpu.step_with(&Decoder::new(), &mut bus);

        assert!(cpu.halted, "an odd frame base is a double bus fault");
        assert_eq!(bus.log.len(), 0, "no frame and no vector fetch");
        assert_eq!(
            cycles,
            12 + crate::exception::HALTED_IDLE_CYCLES,
            "chk_idle's 12 + the halt idle. The 28 for the frame is not owed"
        );
    }

    fn cpu_at(sr: u16) -> M68k {
        let mut cpu = M68k::new();
        cpu.sr = sr;
        cpu.a[7] = 0x3000;
        cpu.ssp = 0x3000;
        cpu
    }

    #[test]
    fn mulu_multiplies_into_the_whole_register() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x1234_8000;
        // MULU #0x0003,D0 — only the low word of D0 takes part.
        let cycles = run_one(&mut cpu, &mut bus, &[0xC0FC, 0x0003, 0x4E71]);
        assert_eq!(cpu.d[0], 0x8000 * 3);
        assert!(!cpu.ccr_n() && !cpu.ccr_z() && !cpu.ccr_v() && !cpu.ccr_c());
        // 2 accesses — the immediate fetch and the queue advance — plus
        // 34 + 2*popcount(3) = 38 idle. The opcode's own fetch belongs to the
        // *previous* instruction's queue advance and is not charged here.
        assert_eq!(cycles, 4 * 2 + 38);
    }

    /// A product with bit 31 set must set `N`, which reading `N` from the low
    /// word would miss.
    #[test]
    fn mulu_sets_n_from_the_full_32_bit_product() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0xFFFF;
        let cycles = run_one(&mut cpu, &mut bus, &[0xC0FC, 0xFFFF, 0x4E71]);
        assert_eq!(cpu.d[0], 0xFFFE_0001);
        assert!(cpu.ccr_n(), "bit 31 of the product is set");
        assert_eq!(cycles, 4 * 2 + 34 + 2 * 16);
    }

    #[test]
    fn muls_sign_extends_both_operands() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0xFFFF; // -1 as a word
                           // MULS #-2,D0 => +2
        let cycles = run_one(&mut cpu, &mut bus, &[0xC1FC, 0xFFFE, 0x4E71]);
        assert_eq!(cpu.d[0], 2);
        assert!(!cpu.ccr_n() && !cpu.ccr_z());
        // src << 1 = 0x1FFFC; difference map 0x1FFFC ^ 0xFFFE = 0x10002,
        // & 0xFFFF = 0x0002 — has 1 set bit => idle = 34 + 2*1 = 36.
        assert_eq!(cycles, 4 * 2 + 36);
    }

    #[test]
    fn mul_leaves_x_alone_and_clears_v_and_c() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S | SR_X | SR_V | SR_C);
        cpu.d[0] = 0;
        run_one(&mut cpu, &mut bus, &[0xC0FC, 0x0000, 0x4E71]);
        assert!(cpu.ccr_x(), "X is untouched by a multiply");
        assert!(cpu.ccr_z() && !cpu.ccr_v() && !cpu.ccr_c());
    }

    #[test]
    fn divu_packs_remainder_high_and_quotient_low() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 100;
        // DIVU #7,D0 => quotient 14, remainder 2.
        run_one(&mut cpu, &mut bus, &[0x80FC, 0x0007, 0x4E71]);
        assert_eq!(cpu.d[0], (2 << 16) | 14);
        assert!(!cpu.ccr_v() && !cpu.ccr_c() && !cpu.ccr_n() && !cpu.ccr_z());
    }

    #[test]
    fn divs_truncates_toward_zero() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = (-100i32) as u32;
        // DIVS #7,D0 => -14 remainder -2, truncating toward zero.
        run_one(&mut cpu, &mut bus, &[0x81FC, 0x0007, 0x4E71]);
        assert_eq!(cpu.d[0] & 0xFFFF, (-14i32) as u32 & 0xFFFF);
        assert_eq!(cpu.d[0] >> 16, (-2i32) as u32 & 0xFFFF);
        assert!(cpu.ccr_n(), "the quotient's word sign bit is set");
        assert!(!cpu.ccr_v());
    }

    /// Overflow sets `V`, leaves the destination alone, and raises nothing —
    /// and sets `N` while clearing `Z`, which is the part the brief's sketch
    /// gets wrong by preserving them.
    #[test]
    fn div_overflow_sets_n_and_v_and_preserves_the_destination() {
        for (op, dividend, divisor) in [
            (0x80FCu16, 0x0001_0000u32, 1u16), // DIVU: quotient 0x10000
            (0x81FC, 0x0000_8000, 1),          // DIVS: quotient 0x8000 > i16::MAX
        ] {
            let mut bus = FlatBus::new();
            let mut cpu = cpu_at(SR_S | SR_X | SR_Z);
            cpu.d[0] = dividend;
            run_one(&mut cpu, &mut bus, &[op, divisor, 0x4E71]);
            assert_eq!(cpu.d[0], dividend, "{op:04X}: destination unchanged");
            assert!(cpu.ccr_v(), "{op:04X}: V set");
            assert!(cpu.ccr_n(), "{op:04X}: N set on the overflow path");
            assert!(!cpu.ccr_z(), "{op:04X}: Z cleared on the overflow path");
            assert!(!cpu.ccr_c(), "{op:04X}: C cleared");
            assert!(cpu.ccr_x(), "{op:04X}: X kept");
            assert_eq!(cpu.pc, 0x1008, "no exception was raised");
        }
    }

    /// `DIVS` overflows on the *signed* range, so a quotient of 0x8000 — which
    /// fits unsigned — must still set `V`. The unsigned predicate would fail
    /// roughly 371 suite cases here.
    #[test]
    fn divs_overflow_uses_the_signed_range() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x8000; // / 1 = 0x8000, in unsigned range, out of signed
        run_one(&mut cpu, &mut bus, &[0x81FC, 0x0001, 0x4E71]);
        assert!(cpu.ccr_v());
        // -0x8000 fits, so the very next value down does not overflow.
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = (-0x8000i32) as u32;
        run_one(&mut cpu, &mut bus, &[0x81FC, 0x0001, 0x4E71]);
        assert!(!cpu.ccr_v(), "-0x8000 is representable");
    }

    /// `DIVS`'s **late overflow**: the timing shortcut does not fire, so the
    /// division runs to completion and the full slow cost is paid, and only then
    /// does the quotient turn out not to fit.
    ///
    /// `0x462A588A / 0x5925` — true quotient 51583, past `i16` but comfortably
    /// inside the shortcut's window, since `0x462A >> 0 = 17962 < 0x5925 = 22821`.
    /// One of 402 such cases in the suite. Guards against the natural
    /// simplification of reusing the `V` predicate as the timing shortcut, which
    /// would charge this the fast 12 and miss by over 100 cycles.
    #[test]
    fn divs_late_overflow_pays_the_full_slow_cost() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x462A_588A;
        let cycles = run_one(&mut cpu, &mut bus, &[0x81FC, 0x5925, 0x4E71]);
        assert!(cpu.ccr_v(), "51583 does not fit in i16");
        assert_eq!(cpu.d[0], 0x462A_588A, "the destination is left alone");
        // The shortcut is silent, so this is the loop cost, not 12-or-14.
        let fast = 12 + 2;
        assert!(
            cycles > fast + 100,
            "late overflow must pay the slow cost, got {cycles}"
        );
        assert_eq!(cycles, 4 * 2 + divs_idle(0x462A_588A, 0x5925));
    }

    /// `DIVS.w #-1` of `i32::MIN` overflows i32 division itself. A plain `/`
    /// panics in debug on that, which would be a host panic on guest data.
    #[test]
    fn divs_of_i32_min_by_minus_one_does_not_panic() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 0x8000_0000;
        run_one(&mut cpu, &mut bus, &[0x81FC, 0xFFFF, 0x4E71]);
        assert!(cpu.ccr_v(), "the quotient does not fit in 16 bits");
        assert_eq!(cpu.d[0], 0x8000_0000, "destination untouched");
    }

    /// Divide by zero. **This test and the two below it are the only coverage
    /// of this path** — the vector suite contains no divide-by-zero case in
    /// either group, so the values asserted here are extrapolated from `CHK`
    /// and `JSR` rather than measured. See the module docs.
    #[test]
    fn divide_by_zero_takes_vector_5_with_the_destination_untouched() {
        for op in [0x80FCu16, 0x81FC] {
            let mut bus = FlatBus::new();
            bus.put16(0x0014, 0x0000); // vector 5 = address 20
            bus.put16(0x0016, 0x2000);
            bus.load(0x2000, &[0x4E71, 0x4E71]);
            let mut cpu = cpu_at(SR_S | SR_X);
            cpu.d[0] = 0x1234_5678;
            let cycles = run_one(&mut cpu, &mut bus, &[op, 0x0000, 0x4E71]);

            assert_eq!(cpu.d[0], 0x1234_5678, "{op:04X}: destination untouched");
            assert_eq!(cpu.pc, 0x2004, "{op:04X}: vectored through 5");
            assert_eq!(cpu.a[7], 0x2FFA, "{op:04X}: a 6-byte short frame");
            // Stacked PC is past the whole instruction: opcode + 2 + 2 ext.
            assert_eq!(bus.read16(0x2FFE), 0x1004, "{op:04X}: stacked PC low");
            assert_eq!(bus.read16(0x2FFC), 0x0000, "{op:04X}: stacked PC high");
            assert!(cpu.ccr_x(), "{op:04X}: X preserved into the handler");
            // EXTRAPOLATED: manual gives zero-divide 38 cycles. 38 - 4*7 = 10 idle.
            // The immediate-source form costs 1 extra access (the immediate fetch),
            // so its total is 42 = 4*(1+7) + 10.
            assert_eq!(
                cycles, 42,
                "{op:04X}: 4*(1+7) + 10 idle (manual, extrapolated)"
            );
        }
    }

    /// The zero divisor must be detected *before* either timing formula runs:
    /// `divs_idle` divides by `dvs_abs` and would panic in debug. A register
    /// source as well as an immediate, since the two take different paths
    /// through the schedule.
    #[test]
    fn divide_by_zero_with_a_register_divisor_does_not_panic() {
        for op in [0x80C1u16, 0x81C1] {
            let mut bus = FlatBus::new();
            bus.put16(0x0016, 0x2000);
            bus.load(0x2000, &[0x4E71, 0x4E71]);
            let mut cpu = cpu_at(SR_S);
            cpu.d[0] = 0xFFFF_FFFF; // negative dividend: exercises divs_idle's
            cpu.d[1] = 0; //           sign handling before the guard
            run_one(&mut cpu, &mut bus, &[op, 0x4E71]);
            assert_eq!(cpu.pc, 0x2004, "{op:04X}: vectored through 5");
            assert_eq!(cpu.d[0], 0xFFFF_FFFF);
        }
    }

    #[test]
    fn chk_within_bounds_does_not_trap() {
        let mut bus = FlatBus::new();
        let mut cpu = cpu_at(SR_S | SR_X | SR_N | SR_V | SR_C);
        cpu.d[0] = 5;
        // CHK #10,D0
        let cycles = run_one(&mut cpu, &mut bus, &[0x41BC, 0x000A, 0x4E71]);
        assert_eq!(cpu.pc, 0x1008, "no trap");
        assert!(!cpu.ccr_n() && !cpu.ccr_z(), "N/Z from the tested value");
        assert!(!cpu.ccr_v() && !cpu.ccr_c(), "V and C cleared");
        assert!(cpu.ccr_x(), "X preserved");
        assert_eq!(cycles, 4 * 2 + 6);
    }

    #[test]
    fn chk_above_the_bound_traps_through_vector_6() {
        let mut bus = FlatBus::new();
        bus.put16(0x001A, 0x2000); // vector 6 = address 24
        bus.load(0x2000, &[0x4E71, 0x4E71]);
        let mut cpu = cpu_at(SR_S);
        cpu.d[0] = 11;
        run_one(&mut cpu, &mut bus, &[0x41BC, 0x000A, 0x4E71]);
        assert_eq!(cpu.pc, 0x2004);
        // Past the whole instruction — opcode 0x1000 plus one extension word.
        assert_eq!(bus.read16(0x2FFE), 0x1004, "stacked PC");
        assert_eq!(
            bus.read16(0x2FFA) & 0x1F,
            0,
            "N/Z clear for a positive value"
        );
    }

    /// A negative value traps whatever the bound, because the comparison is
    /// signed. Read unsigned, `-1` would exceed no bound at all.
    #[test]
    fn chk_traps_on_a_negative_value_and_stacks_n() {
        let mut bus = FlatBus::new();
        bus.put16(0x001A, 0x2000);
        bus.load(0x2000, &[0x4E71, 0x4E71]);
        let mut cpu = cpu_at(SR_S | SR_X);
        cpu.d[0] = 0xFFFF; // -1 as a word
        let cycles = run_one(&mut cpu, &mut bus, &[0x41BC, 0x7FFF, 0x4E71]);
        assert_eq!(cpu.pc, 0x2004, "trapped despite the huge bound");
        let stacked_sr = bus.read16(0x2FFA);
        assert_eq!(
            stacked_sr & (SR_N | SR_Z | SR_V | SR_C),
            SR_N,
            "the frame carries N set from the tested value"
        );
        assert_eq!(stacked_sr & SR_X, SR_X, "X preserved into the frame");
        // value - bound = -1 - 32767 = -32768, which still fits i16, so the
        // idle is 12 — the one bucket the naive "12 if negative" rule gets right.
        assert_eq!(cycles, 4 * (1 + exception::SHORT_FRAME_ACCESSES) + 12);
    }

    /// The `i16` overflow of `value - bound` is what picks 12 over 10, not the
    /// sign of the value: both cases here have a negative value and differ only
    /// in whether the difference fits.
    #[test]
    fn chk_idle_is_keyed_on_the_difference_overflowing_a_word() {
        // -1 vs bound -2: negative, not greater, difference 1 fits => 12.
        assert_eq!(chk_idle(-1, -2), 10, "value > bound wins");
        assert_eq!(
            chk_idle(-3, -2),
            12,
            "negative, not greater, difference fits"
        );
        assert_eq!(chk_idle(-2, 32767), 10, "difference overflows i16");
        assert_eq!(chk_idle(5, 10), 6, "no trap");
        assert_eq!(chk_idle(11, 10), 10, "greater than the bound");
    }

    /// `MULU`'s idle counts set bits; `DIVS`'s iteration term counts *clear*
    /// ones. Pinned because the two are easy to transpose and a transposition
    /// is invisible on any single value.
    #[test]
    fn timing_formula_polarities() {
        assert_eq!(mulu_idle(0), 34, "no set bits");
        assert_eq!(mulu_idle(0xFFFF), 34 + 32, "all 16 set");

        // MULS counts Booth *transitions*, not set bits — the two formulas
        // disagree on these two inputs. 0x5555 has popcount 8 (=> 50 under
        // the wrong rule) but 16 transitions (=> 66 under the right one).
        // 0xFF00 has popcount 8 (=> 50 under wrong rule) but only 1 transition
        // (one contiguous run of set bits -- one rising edge in the <<1 window).
        assert_eq!(
            muls_idle(0x5555),
            34 + 2 * 16,
            "Booth counts transitions, not set bits"
        );
        // 34 + 2*1, written without the `* 1` to keep clippy::identity_op quiet.
        assert_eq!(muls_idle(0xFF00), 34 + 2, "eight set bits, one transition");

        // |q| >> 1 == 0, so all 15 bits are clear: 116 + 30.
        assert_eq!(divs_idle(1, 1), 116 + 30);
        // Overflow shortcut, positive dividend.
        assert_eq!(divs_idle(0x1_0000, 1), 12);
        assert_eq!(divs_idle(0xFFFF_0000, 1), 14, "+2 for a negative dividend");
        assert_eq!(divu_idle(0x1_0000, 1), 6, "DIVU's overflow is one cost");

        // DIVU 100 / 7 = 14 rem 2: nonzero quotient, loop exercises all 15
        // iterations. 16 iterations would produce 130; 15 gives 126.
        assert_eq!(divu_idle(100, 7), 126);

        // DIVS (-,+) pair: base 116 + 6 (dvd_neg) = 122. Quotient -100/7 =>
        // |q|=14, |q|>>1=7 (binary 0b111), 15 bits: 12 zero, 3 set => 12*2=24
        // extra idle. Total: 122 + 24 = 146.
        let neg_100 = (-100i32) as u32;
        assert_eq!(divs_idle(neg_100, 7), 122 + 24, "(-,+) sign pair, base 122");

        // DIVS (+,-) pair: base 116 + 2 (dvs_neg) = 118. Quotient 100/-7 =>
        // |q|=14, same iteration term 24. Total: 118 + 24 = 142.
        let neg_7 = (-7i16) as u16;
        assert_eq!(divs_idle(100, neg_7), 118 + 24, "(+,-) sign pair, base 118");
    }
}
