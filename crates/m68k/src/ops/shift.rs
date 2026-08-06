//! `ASL`, `ASR`, `LSL`, `LSR`, `ROL`, `ROR`, `ROXL`, `ROXR`.
//!
//! One line of the opcode map, `1110`, holding two structurally different
//! instructions that share it by size field:
//!
//! ```text
//!   1110 ccc d ss i tt yyy   ss != 11   shift Dy by a count, in place
//!   1110 0tt d 11  mmm rrr              shift a word in memory by one
//! ```
//!
//! `tt` is at bits 4-3 in the register form and bits 10-9 in the memory form:
//! `00` arithmetic, `01` logical, `10` rotate through X, `11` rotate. Bit 8 is
//! the direction (1 = left). In the register form `i` selects where the count
//! comes from — bit 5 set means `Dx` named by `ccc`, clear means `ccc` itself as
//! an immediate 1-8 with `000` meaning 8. Bit 11 of the memory form must be
//! clear; the encodings with it set are the 68020's bit-field instructions.
//!
//! Reading `tt` from bits 5-4 instead of 4-3 makes each sized group look like a
//! *mixture* of shift types rather than one type — `ASL.b` appearing to contain
//! `LSL`, `ROL` and `ROXL` cases. It is not: with `tt` read from bits 4-3 every
//! group is homogeneous, 2,500/2,500 in all 24 of them. The mixture is the
//! symptom of the off-by-one bit read, and worth naming because it invites
//! writing one handler that re-decodes the type it was already dispatched on.
//!
//! # The shift count and the cost
//!
//! A register count is used **`mod 64`** and no further: `ASL.b` by 40 is 40
//! shifts of a byte, not 40 mod 8. Distinguishable from `% bits` in 14,658
//! cases, 14,658/14,658. That is also why every step below is written as a
//! closed form over a possibly-out-of-range count rather than as a loop: the
//! loop is correct but 63 iterations per instruction, and the closed forms are
//! checked against exactly that loop (see the test module).
//!
//! Cost follows the timing law with a count-dependent idle:
//!
//! ```text
//!   idle = base + 2 * count        base = 4 for .l, 2 for .b/.w
//! ```
//!
//! Exact in all 24 groups with no exceptions, register and immediate forms
//! alike. A count of 0 still pays `base`. The memory form pays no idle of its
//! own — modes `-(An)` and `(d8,An,Xn)` show 2, which is `alu::run`'s own
//! address-computation lead and not a property of the shift.
//!
//! # The flag rules that are not obvious
//!
//! Each was scored only on the cases where it and a plausible alternative
//! predict different CCRs (task-6-addendum §3's method):
//!
//! - **A zero count leaves the operand and X alone, clears V, and sets C from X
//!   for `ROXL`/`ROXR` only** — 61/61 for `C = X` against `C = 0` on the `ROX`
//!   subset, 179/179 for `C = 0` against `C` preserved on the others. X
//!   preserved 238/238, V cleared 480/480, operand unchanged 480/480. The `ROX`
//!   case is not a special case in hardware: X sits *in* the rotate chain, so a
//!   zero-length rotation still reports it.
//! - **`ASL`'s V is a mid-shift predicate, not an endpoint comparison.** V is
//!   set if the sign bit changed at *any* step, which is not the same as the
//!   first and last signs differing — `0x80 ASL.b 2` returns to a clear sign
//!   having passed through a set one. Comparing endpoints scores ~25%; the
//!   mid-shift rule is 13,793/13,793 and wins 2,938/2,938 of the cases where
//!   the two disagree.
//! - **`ASR`, and every `LS`/`RO`/`ROX` form, clears V unconditionally** —
//!   3,477/3,477 for `ASR` against the endpoint rule, and 37,941/37,941 for the
//!   rest against `ASL`'s rule.
//! - **`ROL`/`ROR` leave X alone; everything else sets X from C.** 6,859/6,859
//!   and 6,646/6,646 (`ROX`) / 13,610/13,610 (`AS`/`LS`, restricted to a nonzero
//!   count — all 118 apparent counterexamples are zero-count cases, where X is
//!   preserved instead).
//! - **Z is this instruction's own, not accumulated into the incoming Z** the
//!   way `ADDX`'s is (4,861/4,861), and **N comes from the result**, not the
//!   operand (23,491/23,491). So [`crate::flags::accumulate_z`] is deliberately
//!   not used here.
//!
//! Bucket census over the 24 (type × direction × size) combinations: none
//! empty, 10-27 zero-count cases each, and the counts sum to 55,139 — the whole
//! register/immediate corpus. No rule above is untested for want of cases.

use crate::cpu::M68k;
use crate::decode::Handler;
use crate::ea::Size;
use crate::ops::alu::{self, Ops, Plan};
use crate::Bus;

/// Which of the four families `tt` selects.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `ASL` / `ASR`: the only family with a V rule.
    Arith,
    /// `LSL` / `LSR`.
    Logic,
    /// `ROXL` / `ROXR`: a `bits + 1`-wide rotation with X as the extra bit.
    Rox,
    /// `ROL` / `ROR`: the only family that leaves X alone.
    Rot,
}

impl Kind {
    #[inline]
    fn from_bits(tt: u16) -> Self {
        match tt {
            0 => Kind::Arith,
            1 => Kind::Logic,
            2 => Kind::Rox,
            _ => Kind::Rot,
        }
    }
}

/// A shift's result and the CCR it produces.
struct Out {
    res: u32,
    x: bool,
    n: bool,
    z: bool,
    v: bool,
    c: bool,
}

/// Shifts `v0` by `count`, returning the result and every condition code.
///
/// `count` is the already-reduced `mod 64` value, so it may exceed the operand
/// width; each arm handles that explicitly rather than clamping, because the
/// two differ. Every shift below is on a `u32` by strictly less than 32, or on a
/// `u64` — a `u32` shifted by 32 or more panics in a debug build, and the
/// opcode-space test runs in debug precisely to catch it.
fn shift(kind: Kind, left: bool, v0: u32, count: u32, size: Size, x0: bool) -> Out {
    let m = size.mask();
    let msb = size.msb();
    let bits = 8 * size.bytes();
    let v = v0 & m;

    // A zero-length shift is not a no-op: it reports flags. The operand and X
    // survive, V clears, and C comes from X for a rotate-through-X because X is
    // part of that rotation.
    if count == 0 {
        return Out {
            res: v,
            x: x0,
            n: v & msb != 0,
            z: v == 0,
            v: false,
            c: kind == Kind::Rox && x0,
        };
    }

    let (res, x, c, v_flag) = match (kind, left) {
        (Kind::Arith, true) | (Kind::Logic, true) => {
            let res = if count >= bits { 0 } else { (v << count) & m };
            // The last bit shifted out of the top. A count past the width
            // shifts the operand away entirely, so nothing is left to carry.
            let c = count <= bits && (v >> (bits - count)) & 1 != 0;
            let v_flag = kind == Kind::Arith && asl_overflow(v, count, bits);
            (res, c, c, v_flag)
        }
        (Kind::Logic, false) => {
            let res = if count >= bits { 0 } else { v >> count };
            let c = count <= bits && (v >> (count - 1)) & 1 != 0;
            (res, c, c, false)
        }
        (Kind::Arith, false) => {
            // The sign bit is replicated, so a long enough shift saturates to
            // all-ones or all-zeroes rather than to zero.
            let neg = v & msb != 0;
            let res = if count >= bits {
                if neg {
                    m
                } else {
                    0
                }
            } else if neg {
                (v >> count) | (m & !(m >> count))
            } else {
                v >> count
            };
            let c = (v >> (count.min(bits) - 1)) & 1 != 0;
            (res, c, c, false)
        }
        (Kind::Rot, _) => {
            let r = count % bits;
            let res = if left {
                ((v << r) | (v >> ((bits - r) % bits))) & m
            } else {
                ((v >> r) | (v << ((bits - r) % bits))) & m
            };
            // The bit that wrapped around, which is the one that left the
            // operand: the new low bit going left, the new sign bit going right.
            let c = if left { res & 1 != 0 } else { res & msb != 0 };
            // X is untouched by a plain rotate — this is the one family where it
            // is, and the reason `x0` is threaded through at all.
            (res, x0, c, false)
        }
        (Kind::Rox, _) => {
            // X is bit `bits` of a `bits + 1`-wide value, and the whole thing
            // rotates. Written this way the count reduction is `mod bits + 1`,
            // which is what makes `ROXL.b` by 9 the identity.
            let n = bits + 1;
            let r = count % n;
            let chain = ((x0 as u64) << bits) | v as u64;
            let full = (1u64 << n) - 1;
            let rot = if left {
                ((chain << r) | (chain >> (n - r))) & full
            } else {
                ((chain >> r) | (chain << (n - r))) & full
            };
            let x = rot >> bits != 0;
            ((rot & m as u64) as u32, x, x, false)
        }
    };

    Out {
        res,
        x,
        n: res & msb != 0,
        z: res == 0,
        v: v_flag,
        c,
    }
}

/// `ASL`'s V: did the sign bit change at any point during the shift?
///
/// Shifting left by `count` moves `count + 1` bits through the sign position in
/// turn, so V is set unless those top `count + 1` bits are all equal. Computed
/// in `u64` because `count + 1` reaches 32 for a long operand, and `1u32 << 32`
/// panics in a debug build.
#[inline]
fn asl_overflow(v: u32, count: u32, bits: u32) -> bool {
    if count >= bits {
        // Everything is shifted out, so the sign passed through every bit of
        // the operand: V is set unless the operand was zero.
        return v != 0;
    }
    let k = count + 1;
    let top = (v as u64) >> (bits - k);
    !(top == 0 || top == (1u64 << k) - 1)
}

/// `1110 ccc d ss i tt yyy` — shift a data register by a register or immediate
/// count.
fn register_form(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16, size: Size) -> u32 {
    let kind = Kind::from_bits((opcode >> 3) & 3);
    let left = opcode & 0x0100 != 0;
    let ccc = ((opcode >> 9) & 7) as usize;

    // Bit 5 set: the count is in Dx, taken mod 64. Clear: `ccc` is the count
    // itself, with 000 meaning 8 rather than 0 — there is no encoding for a
    // zero-length immediate shift, which is why the zero-count flag rules are
    // reachable only through a register count.
    let count = if opcode & 0x0020 != 0 {
        cpu.d[ccc] & 63
    } else if ccc == 0 {
        8
    } else {
        ccc as u32
    };

    let base = if size == Size::Long { 4 } else { 2 };
    let plan = Plan::new(size, 0, opcode & 7).idle(base + 2 * count);
    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        let o = shift(kind, left, ops.ea, count, size, cpu.ccr_x());
        cpu.set_ccr(o.x, o.n, o.z, o.v, o.c);
        Some(o.res)
    })
}

/// `1110 0tt d 11 mmm rrr` — shift a word in memory by one.
fn memory_form(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    let kind = Kind::from_bits((opcode >> 9) & 3);
    let left = opcode & 0x0100 != 0;
    let (mode, reg) = ((opcode >> 3) & 7, opcode & 7);

    let plan = Plan::new(Size::Word, mode, reg).writes();
    alu::run(cpu, bus, &plan, &mut |cpu, ops: Ops| {
        let o = shift(kind, left, ops.ea, 1, Size::Word, cpu.ccr_x());
        cpu.set_ccr(o.x, o.n, o.z, o.v, o.c);
        Some(o.res)
    })
}

// --- Dispatch-table installation ------------------------------------------

fn shift_b(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    register_form(cpu, bus, opcode, Size::Byte)
}
fn shift_w(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    register_form(cpu, bus, opcode, Size::Word)
}
fn shift_l(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    register_form(cpu, bus, opcode, Size::Long)
}
fn shift_mem(cpu: &mut M68k, bus: &mut dyn Bus, opcode: u16) -> u32 {
    memory_form(cpu, bus, opcode)
}

/// Installs the whole `1110` line.
///
/// Every register-form encoding exists: all eight count values, both
/// directions, all four types and all eight destination registers are legal, so
/// the 3,072 opcodes below size `11` are claimed unconditionally. Only the
/// memory form has holes.
pub fn register(table: &mut [Handler; 65536]) {
    for op in 0xE000..=0xEFFFu16 {
        let size_bits = (op >> 6) & 3;
        if size_bits != 3 {
            table[op as usize] = [shift_b, shift_w, shift_l][size_bits as usize];
            continue;
        }
        // Memory form: bit 11 is not part of the encoding and must be clear,
        // and the destination must be writable memory.
        let (mode, reg) = ((op >> 3) & 7, op & 7);
        if op & 0x0800 == 0 && super::arith::valid_mem_dst(mode, reg) {
            table[op as usize] = shift_mem;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::{FlatBus, RecordingBus};
    use crate::cpu::{SR_C, SR_S, SR_V, SR_X};
    use crate::decode::Decoder;

    fn at(bus: &mut impl Bus) -> M68k {
        let mut cpu = M68k::new();
        cpu.sr = SR_S;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(bus);
        cpu
    }

    /// One hardware step at a time: the definition the closed forms above are
    /// derived from, kept here so they can be checked against it rather than
    /// against a restatement of themselves.
    fn one_step_at_a_time(
        kind: Kind,
        left: bool,
        v0: u32,
        count: u32,
        size: Size,
        x0: bool,
    ) -> Out {
        let m = size.mask();
        let msb = size.msb();
        let mut v = v0 & m;
        let mut x = x0;
        let mut c = false;
        let mut v_flag = false;

        if count == 0 {
            return Out {
                res: v,
                x,
                n: v & msb != 0,
                z: v == 0,
                v: false,
                c: kind == Kind::Rox && x,
            };
        }

        for _ in 0..count {
            let sign_before = v & msb != 0;
            match (kind, left) {
                (Kind::Arith, true) | (Kind::Logic, true) => {
                    c = v & msb != 0;
                    v = (v << 1) & m;
                    x = c;
                }
                (Kind::Arith, false) => {
                    c = v & 1 != 0;
                    v = (v >> 1) | (v & msb);
                    x = c;
                }
                (Kind::Logic, false) => {
                    c = v & 1 != 0;
                    v >>= 1;
                    x = c;
                }
                (Kind::Rox, true) => {
                    let x_in = x;
                    c = v & msb != 0;
                    x = c;
                    v = ((v << 1) & m) | x_in as u32;
                }
                (Kind::Rox, false) => {
                    let x_in = x;
                    c = v & 1 != 0;
                    x = c;
                    v = (v >> 1) | if x_in { msb } else { 0 };
                }
                (Kind::Rot, true) => {
                    c = v & msb != 0;
                    v = ((v << 1) & m) | c as u32;
                }
                (Kind::Rot, false) => {
                    c = v & 1 != 0;
                    v = (v >> 1) | if c { msb } else { 0 };
                }
            }
            if kind == Kind::Arith && (v & msb != 0) != sign_before {
                v_flag = true;
            }
        }

        Out {
            res: v,
            x,
            n: v & msb != 0,
            z: v == 0,
            v: v_flag,
            c,
        }
    }

    /// The closed forms must agree with the per-step definition everywhere,
    /// including every count from 0 to 63 and both X inputs. This is what makes
    /// the closed forms safe to trust past the sampled vectors.
    #[test]
    fn closed_forms_match_a_per_step_shift() {
        for size in [Size::Byte, Size::Word, Size::Long] {
            let m = size.mask();
            let bits = 8 * size.bytes();
            let values: [u32; 13] = [
                0,
                1,
                2,
                3,
                m,
                m - 1,
                1 << (bits - 1),
                (1 << (bits - 1)) - 1,
                0x55 & m,
                0xAA & m,
                0x1234_5678 & m,
                0x8000_0001 & m,
                0x7FFF_FFFF & m,
            ];
            for tt in 0..4u16 {
                let kind = Kind::from_bits(tt);
                for left in [true, false] {
                    for count in 0..64u32 {
                        for &v in &values {
                            for x0 in [false, true] {
                                let a = one_step_at_a_time(kind, left, v, count, size, x0);
                                let b = shift(kind, left, v, count, size, x0);
                                assert!(
                                    a.res == b.res
                                        && a.x == b.x
                                        && a.n == b.n
                                        && a.z == b.z
                                        && a.v == b.v
                                        && a.c == b.c,
                                    "tt={tt} left={left} size={size:?} v={v:08X} \
                                     count={count} x={x0}: step-by-step gives \
                                     ({:08X},x{},n{},z{},v{},c{}), closed form gives \
                                     ({:08X},x{},n{},z{},v{},c{})",
                                    a.res,
                                    a.x as u8,
                                    a.n as u8,
                                    a.z as u8,
                                    a.v as u8,
                                    a.c as u8,
                                    b.res,
                                    b.x as u8,
                                    b.n as u8,
                                    b.z as u8,
                                    b.v as u8,
                                    b.c as u8,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// A zero count reports flags rather than doing nothing, and `ROXL` alone
    /// sets C from X there.
    #[test]
    fn zero_count_preserves_the_operand_and_reports_x_only_for_rox() {
        let dec = Decoder::new();
        // ROXL.b D1,D0 with D1 = 0: C must come from X.
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xE330, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_X | SR_V;
        cpu.d[0] = 0x1234_5678;
        cpu.d[1] = 0;
        let cycles = cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0], 0x1234_5678, "the operand is untouched");
        assert!(cpu.ccr_c(), "a zero-length ROX reports X in C");
        assert!(cpu.ccr_x(), "X itself survives");
        assert!(!cpu.ccr_v(), "V is cleared even by a zero-length shift");
        assert_eq!(cycles, 6, "one fetch plus the 2-cycle byte base idle");

        // LSL.b D1,D0 with D1 = 0: C must be clear despite X being set.
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xE328, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_X | SR_C;
        cpu.d[0] = 0x1234_5678;
        cpu.d[1] = 0;
        cpu.step_with(&dec, &mut bus);
        assert!(!cpu.ccr_c(), "a zero-length non-ROX shift clears C");
        assert!(cpu.ccr_x(), "X still survives");
    }

    /// `ASL`'s V is set by a sign change *during* the shift, which endpoint
    /// comparison misses: 0x80 shifted left twice as a byte ends at 0x00, so
    /// both endpoints have a clear sign and V must still be set.
    #[test]
    fn asl_overflow_is_a_mid_shift_predicate() {
        let dec = Decoder::new();
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xE500, 0x4E71]); // ASL.b #2,D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x80;
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0], 0x00);
        assert!(cpu.ccr_z() && !cpu.ccr_n(), "both endpoints look positive");
        assert!(cpu.ccr_v(), "but the sign changed on the way");

        // The count >= bits branch: everything is shifted out, and V is set
        // for any nonzero operand.
        assert!(asl_overflow(0x01, 8, 8));
        assert!(asl_overflow(0x01, 40, 8));
        assert!(!asl_overflow(0x00, 40, 8));
        // A single left shift of 0x40 sets the sign, so V is set; of 0x20 it
        // does not.
        assert!(asl_overflow(0x40, 1, 8));
        assert!(!asl_overflow(0x20, 1, 8));
    }

    /// `ROXL` carries X through the operand: bit `bits` of the rotation.
    #[test]
    fn roxl_rotates_through_x() {
        let dec = Decoder::new();
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xE310, 0x4E71]); // ROXL.b #1,D0
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_X;
        cpu.d[0] = 0x80;
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0] & 0xFF, 0x01, "X came in at the bottom");
        assert!(cpu.ccr_c() && cpu.ccr_x(), "the old sign bit went out to X");

        // Nine ROXL.b steps are the identity, because the chain is 9 bits wide.
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xE330, 0x4E71]); // ROXL.b D1,D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0xA5;
        cpu.d[1] = 9;
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0] & 0xFF, 0xA5);
        assert!(!cpu.ccr_x(), "X round-trips too, from its cleared start");
    }

    /// A count of 32 or more must not shift a `u32` by 32 — that panics in a
    /// debug build. Exercised through the widest operand, where the boundary
    /// is reachable.
    #[test]
    fn a_count_past_the_operand_width_does_not_panic() {
        let dec = Decoder::new();
        for (op, name) in [
            (0xE3A0u16, "ASL.l D1,D0"),
            (0xE2A0, "ASR.l D1,D0"),
            (0xE3A8, "LSL.l D1,D0"),
            (0xE2A8, "LSR.l D1,D0"),
            (0xE3B0, "ROXL.l D1,D0"),
            (0xE3B8, "ROL.l D1,D0"),
        ] {
            for count in [31u32, 32, 33, 63, 64, 0xFFFF_FFFF] {
                let mut bus = FlatBus::new();
                bus.load(0x1000, &[op, 0x4E71]);
                let mut cpu = at(&mut bus);
                cpu.d[0] = 0x8765_4321;
                cpu.d[1] = count;
                let cycles = cpu.step_with(&dec, &mut bus);
                // The count is `mod 64`, so it also bounds the cost.
                assert_eq!(
                    cycles,
                    4 + 4 + 2 * (count & 63),
                    "{name} with count {count}"
                );
            }
        }
    }

    /// `ASR` replicates the sign, so a long shift saturates rather than
    /// clearing.
    #[test]
    fn asr_saturates_to_the_sign() {
        let dec = Decoder::new();
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xE220, 0x4E71]); // ASR.b D1,D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x80;
        cpu.d[1] = 20;
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0] & 0xFF, 0xFF, "a negative byte saturates to -1");
        assert!(cpu.ccr_c(), "the last bit out was a one");
        assert!(!cpu.ccr_v(), "ASR never sets V");
    }

    /// `ROR` leaves X alone; `LSR` sets it from C. The one-line difference
    /// between the families that is easiest to get wrong.
    #[test]
    fn a_plain_rotate_leaves_x_alone() {
        let dec = Decoder::new();

        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xE218, 0x4E71]); // ROR.b #1,D0
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_X;
        cpu.d[0] = 0x02;
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0] & 0xFF, 0x01);
        assert!(!cpu.ccr_c(), "bit 0 was clear");
        assert!(cpu.ccr_x(), "ROR does not touch X");

        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xE208, 0x4E71]); // LSR.b #1,D0
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_X;
        cpu.d[0] = 0x02;
        cpu.step_with(&dec, &mut bus);
        assert!(!cpu.ccr_x(), "LSR sets X from C, which is clear here");
    }

    /// The memory form shifts a word by one and writes it back, reading before
    /// writing with the queue advance between.
    #[test]
    fn the_memory_form_is_a_word_read_modify_write() {
        let dec = Decoder::new();
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0xE1D0, 0x4E71, 0x4E71]); // ASL.w (A0)
        bus.put16(0x2000, 0x4001);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2000;
        bus.log.clear();

        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cycles, 12, "three accesses, no idle");
        assert!(cpu.ccr_n() && cpu.ccr_v(), "0x4001 -> 0x8002 changes sign");
        assert!(!cpu.ccr_c(), "bit 15 of 0x4001 is clear");
        // Read, queue advance, write — in that order. Asserted before reading
        // memory back, since a read through the bus would appear in the log.
        assert_eq!(
            bus.log,
            vec![
                (false, 0x2000, 0x4001),
                (false, 0x1004, 0x4E71),
                (true, 0x2000, 0x8002),
            ]
        );
        assert_eq!(bus.read16(0x2000), 0x8002);
    }

    /// The immediate count encodes 8 as `000`, so there is no zero-length
    /// immediate shift and `ASL.b #8,D0` costs the full eight steps.
    #[test]
    fn an_immediate_count_of_zero_means_eight() {
        let dec = Decoder::new();
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xE100, 0x4E71]); // ASL.b #8,D0
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x0000_00FF;
        let cycles = cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.d[0], 0x0000_0000, "shifted out entirely");
        assert!(cpu.ccr_c(), "the last bit out was a one");
        assert_eq!(cycles, 4 + 2 + 2 * 8);
    }

    /// The shift type comes from bits 4-3, not 5-4. Two opcodes that differ
    /// only in bit 5 are the same *type* with different count sources; two that
    /// differ in bit 3 are different types.
    #[test]
    fn the_type_field_is_bits_four_and_three() {
        let dec = Decoder::new();

        // 0xE320 = ASL.b D1,D0 (tt=00, i=1); 0xE328 = LSL.b D1,D0 (tt=01).
        // ASL sets V on a sign change, LSL never does.
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xE320, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x40;
        cpu.d[1] = 1;
        cpu.step_with(&dec, &mut bus);
        assert!(cpu.ccr_v(), "0xE320 must be ASL");

        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0xE328, 0x4E71]);
        let mut cpu = at(&mut bus);
        cpu.sr |= SR_V;
        cpu.d[0] = 0x40;
        cpu.d[1] = 1;
        cpu.step_with(&dec, &mut bus);
        assert!(!cpu.ccr_v(), "0xE328 must be LSL, which clears V");
    }
}
