//! The opcode pages and their dispatch.
//!
//! One `match` per page, arms delegating to helpers. Written as a flat `match`
//! rather than a table of function pointers: the compiler turns a dense `match`
//! into a jump table anyway, and a table would need 256 entries written out with
//! no help from the type system if one were missed.
//!
//! # The `Q` convention
//!
//! Every arm ends by setting `cpu.q` to the flags it wrote, or to zero if it wrote
//! none — see [`Z80::q`]. `cpu.p` and `cpu.ei` are cleared by [`Z80::step`] before
//! dispatch, so an arm only ever sets those.

use crate::flags::{self, C, F3, F5, H, N, PV, S, Z};
use crate::{Bus, Z80};

/// Executes a base-page opcode. Returns T-states.
///
/// Arms are added by later tasks; every one of the 256 is reachable, because the
/// Z80 has no undefined base-page opcode.
pub fn execute<B: Bus>(cpu: &mut Z80, bus: &mut B, op: u8) -> u32 {
    let _ = bus;
    match op {
        0x00 => {
            // NOP. Writes no flags, so Q clears.
            cpu.q = 0;
            4
        }
        0x27 => {
            daa(cpu);
            4
        }
        0x2F => {
            // CPL: invert A, set H and N. C, S, Z and P/V are untouched per the
            // manual; F3/F5 come from the result.
            cpu.a = !cpu.a;
            cpu.f = (cpu.f & (C | Z | S | PV)) | H | N | (cpu.a & (F5 | F3));
            cpu.q = cpu.f;
            4
        }
        0x37 => {
            scf_ccf(cpu, false);
            4
        }
        0x3F => {
            scf_ccf(cpu, true);
            4
        }
        0x76 => {
            // HALT. `PC` stays past the opcode -- the vectors show it advancing --
            // and `halted` is what makes the *next* step re-execute instead of
            // running into whatever follows.
            cpu.halted = true;
            cpu.q = 0;
            4
        }
        0xF3 => {
            // DI: both flip-flops, immediately, no delay.
            cpu.iff1 = false;
            cpu.iff2 = false;
            cpu.q = 0;
            4
        }
        0xFB => {
            // EI: flip-flops set now, but the enable does not take effect until
            // after the next instruction. `ei` carries that pending state, and the
            // next `step` clears it before running anything.
            cpu.iff1 = true;
            cpu.iff2 = true;
            cpu.ei = 1;
            cpu.q = 0;
            4
        }
        // Tasks 7 through 9 fill the rest. Until then an unimplemented opcode is
        // a panic *in development only*: it is unreachable once the suite is
        // green, and a silent 4-T-state NOP here would make a missing instruction
        // look like a flag bug across a hundred vector files. Task 12 deletes this
        // arm and lets the compiler prove the match exhaustive.
        other => unimplemented!("base opcode {other:#04X}"),
    }
}

/// `SCF` (`ccf = false`) and `CCF` (`ccf = true`).
///
/// The F3/F5 rule is the reason `q` exists, and the reason it holds a *value*. The
/// two undocumented bits are taken from `A` ORed with the bits of `F` that the
/// previous instruction did not write — `f & !q`. So after a flag writer they come
/// from `A` alone (every bit of `F` was just written), and after a `NOP` they carry
/// whatever `F` already held.
///
/// Measured against `37.json` and `3f.json`: 0 of 2,000 cases wrong with this rule,
/// and 229 and 219 wrong respectively with `A` alone.
///
/// S, Z and P/V are preserved: the manual defines these two instructions as
/// affecting C, H and N and nothing else.
fn scf_ccf(cpu: &mut Z80, ccf: bool) {
    let old_c = cpu.f & C;
    let f35 = (cpu.a | (cpu.f & !cpu.q)) & (F5 | F3);
    let carry = if ccf { old_c ^ C } else { C };
    let h = if ccf { old_c << 4 } else { 0 };
    cpu.f = (cpu.f & (S | Z | PV)) | f35 | h | carry;
    cpu.q = cpu.f;
}

/// `DAA`: corrects `A` after a BCD add or subtract.
///
/// N selects the direction — that is the only thing N is for. The adjustment is
/// 0x06 per nibble that is out of range or carried, exactly as the Zilog manual's
/// table states, and the manual's table is where these conditions came from.
fn daa(cpu: &mut Z80) {
    let a = cpu.a;
    let mut adjust = 0u8;
    let mut carry = cpu.f & C != 0;
    if cpu.f & H != 0 || (a & 0x0F) > 9 {
        adjust |= 0x06;
    }
    if carry || a > 0x99 {
        adjust |= 0x60;
        carry = true;
    }
    let result = if cpu.f & N != 0 {
        a.wrapping_sub(adjust)
    } else {
        a.wrapping_add(adjust)
    };
    // H after DAA: the manual defines it as the half-carry of the adjustment,
    // which for a subtraction means "was there a borrow from bit 4".
    let h = if cpu.f & N != 0 {
        cpu.f & H != 0 && (a & 0x0F) < 6
    } else {
        (a & 0x0F) > 9
    };
    cpu.a = result;
    cpu.f = flags::sz53p(result) | (cpu.f & N) | if h { H } else { 0 } | if carry { C } else { 0 };
    cpu.q = cpu.f;
}
