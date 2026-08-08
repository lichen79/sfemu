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
use crate::ops::{alu, bits, flow, load};
use crate::{Bus, Z80};

/// Executes a base-page opcode. Returns T-states.
///
/// Arms are added by later tasks; every one of the 256 is reachable, because the
/// Z80 has no undefined base-page opcode.
pub fn execute<B: Bus>(cpu: &mut Z80, bus: &mut B, op: u8) -> u32 {
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
        // LD r,r' and LD r,(HL) and LD (HL),r. 0x76 is HALT, handled above.
        0x40..=0x7F => {
            let (dst, src) = ((op >> 3) & 7, op & 7);
            let v = reg(cpu, bus, src);
            set_reg(cpu, bus, dst, v);
            cpu.q = 0;
            // 4 T-states, plus 3 for each (HL) touched.
            4 + 3 * u32::from(dst == 6 || src == 6)
        }
        // The eight ALU operations over the same source encoding.
        0x80..=0xBF => {
            let src = op & 7;
            let v = reg(cpu, bus, src);
            alu_op(cpu, (op >> 3) & 7, v);
            4 + 3 * u32::from(src == 6)
        }
        // LD r,n
        0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x36 | 0x3E => {
            let n = cpu.imm(bus);
            let dst = (op >> 3) & 7;
            set_reg(cpu, bus, dst, n);
            cpu.q = 0;
            if dst == 6 {
                10
            } else {
                7
            }
        }
        // ALU with an immediate operand.
        0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
            let n = cpu.imm(bus);
            alu_op(cpu, (op >> 3) & 7, n);
            7
        }
        // INC r and DEC r.
        0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x34 | 0x3C => {
            let i = (op >> 3) & 7;
            let v = reg(cpu, bus, i);
            let r = alu::inc8(cpu, v);
            set_reg(cpu, bus, i, r);
            if i == 6 {
                11
            } else {
                4
            }
        }
        0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x35 | 0x3D => {
            let i = (op >> 3) & 7;
            let v = reg(cpu, bus, i);
            let r = alu::dec8(cpu, v);
            set_reg(cpu, bus, i, r);
            if i == 6 {
                11
            } else {
                4
            }
        }
        // LD rr,nn
        0x01 | 0x11 | 0x21 | 0x31 => {
            let nn = cpu.imm16(bus);
            set_rp(cpu, (op >> 4) & 3, nn);
            cpu.q = 0;
            10
        }
        // INC rr and DEC rr: no flags at all, not even Z.
        0x03 | 0x13 | 0x23 | 0x33 => {
            let i = (op >> 4) & 3;
            let v = rp(cpu, i).wrapping_add(1);
            set_rp(cpu, i, v);
            cpu.q = 0;
            6
        }
        0x0B | 0x1B | 0x2B | 0x3B => {
            let i = (op >> 4) & 3;
            let v = rp(cpu, i).wrapping_sub(1);
            set_rp(cpu, i, v);
            cpu.q = 0;
            6
        }
        // ADD HL,rr
        0x09 | 0x19 | 0x29 | 0x39 => {
            let hl = cpu.hl();
            let v = rp(cpu, (op >> 4) & 3);
            let r = alu::add16(cpu, hl, v);
            cpu.set_hl(r);
            // The latch follows the addend as it enters the ALU, so it holds the
            // *old* HL plus one -- not the sum. See [`Z80::wz`].
            cpu.wz = hl.wrapping_add(1);
            11
        }
        // LD (BC)/(DE),A and LD A,(BC)/(DE)
        0x02 | 0x12 => {
            let addr = if op == 0x02 { cpu.bc() } else { cpu.de() };
            bus.write(addr, cpu.a);
            cpu.wz = wz_after_write(addr, cpu.a);
            cpu.q = 0;
            7
        }
        0x0A | 0x1A => {
            let addr = if op == 0x0A { cpu.bc() } else { cpu.de() };
            cpu.a = bus.read(addr);
            cpu.wz = addr.wrapping_add(1);
            cpu.q = 0;
            7
        }
        // LD (nn),A / LD A,(nn) / LD (nn),HL / LD HL,(nn)
        0x32 => {
            let nn = cpu.imm16(bus);
            bus.write(nn, cpu.a);
            cpu.wz = wz_after_write(nn, cpu.a);
            cpu.q = 0;
            13
        }
        0x3A => {
            let nn = cpu.imm16(bus);
            cpu.a = bus.read(nn);
            cpu.wz = nn.wrapping_add(1);
            cpu.q = 0;
            13
        }
        0x22 => {
            let nn = cpu.imm16(bus);
            bus.write(nn, cpu.l);
            bus.write(nn.wrapping_add(1), cpu.h);
            // `nn + 1`, not `nn + 2`, and no written byte in the high half: the
            // 16-bit forms increment the latch once and stop there. Measured on
            // both files' 1,000 cases -- `nn + 2` is wrong on every one.
            cpu.wz = nn.wrapping_add(1);
            cpu.q = 0;
            16
        }
        0x2A => {
            let nn = cpu.imm16(bus);
            cpu.l = bus.read(nn);
            cpu.h = bus.read(nn.wrapping_add(1));
            cpu.wz = nn.wrapping_add(1);
            cpu.q = 0;
            16
        }
        // PUSH rr / POP rr. The pair encoding here is BC DE HL AF, not ...SP.
        0xC5 | 0xD5 | 0xE5 | 0xF5 => {
            let v = match (op >> 4) & 3 {
                0 => cpu.bc(),
                1 => cpu.de(),
                2 => cpu.hl(),
                _ => cpu.af(),
            };
            load::push(cpu, bus, v);
            cpu.q = 0;
            11
        }
        0xC1 | 0xD1 | 0xE1 | 0xF1 => {
            let v = load::pop(cpu, bus);
            match (op >> 4) & 3 {
                0 => cpu.set_bc(v),
                1 => cpu.set_de(v),
                2 => cpu.set_hl(v),
                // POP AF writes the flag register wholesale, including F3/F5 and
                // the bits no instruction sets. Q clears: this is not a flag
                // computation.
                _ => cpu.set_af(v),
            }
            cpu.q = 0;
            10
        }
        0xEB => {
            load::ex_de_hl(cpu);
            cpu.q = 0;
            4
        }
        0x08 => {
            load::ex_af(cpu);
            cpu.q = 0;
            4
        }
        0xD9 => {
            load::exx(cpu);
            cpu.q = 0;
            4
        }
        0xF9 => {
            cpu.sp = cpu.hl();
            cpu.q = 0;
            6
        }
        // The four accumulator rotates.
        0x07 => {
            bits::rlca(cpu);
            4
        }
        0x0F => {
            bits::rrca(cpu);
            4
        }
        0x17 => {
            bits::rla(cpu);
            4
        }
        0x1F => {
            bits::rra(cpu);
            4
        }
        // JR d, and the four conditional forms. The condition index for
        // 0x20/0x28/0x30/0x38 is bits 4-3 of the opcode, which encode NZ Z NC C.
        0x18 => {
            let d = cpu.imm(bus);
            flow::jr(cpu, d);
            // Every jump that is *taken* latches its target; one that is not taken
            // leaves the latch alone. See [`Z80::wz`].
            cpu.wz = cpu.pc;
            cpu.q = 0;
            12
        }
        0x20 | 0x28 | 0x30 | 0x38 => {
            let d = cpu.imm(bus);
            cpu.q = 0;
            if flow::cond(cpu, (op >> 3) & 3) {
                flow::jr(cpu, d);
                cpu.wz = cpu.pc;
                12
            } else {
                7
            }
        }
        0x10 => {
            // DJNZ. B is decremented without flags -- this is not `dec8`, and
            // using it here would clobber a comparison the loop body depends on.
            let d = cpu.imm(bus);
            cpu.b = cpu.b.wrapping_sub(1);
            cpu.q = 0;
            if cpu.b != 0 {
                flow::jr(cpu, d);
                cpu.wz = cpu.pc;
                13
            } else {
                8
            }
        }
        0xC3 => {
            let nn = cpu.imm16(bus);
            cpu.pc = nn;
            cpu.wz = nn;
            cpu.q = 0;
            10
        }
        // JP cc,nn: 10 T-states either way -- the operand is read regardless, and
        // there is nothing else to charge for.
        0xC2 | 0xCA | 0xD2 | 0xDA | 0xE2 | 0xEA | 0xF2 | 0xFA => {
            let nn = cpu.imm16(bus);
            if flow::cond(cpu, (op >> 3) & 7) {
                cpu.pc = nn;
            }
            // The latch takes `nn` either way: unlike a relative jump, the absolute
            // forms compute their target in the latch before the condition is
            // consulted, so it is written even when the jump is not taken.
            cpu.wz = nn;
            cpu.q = 0;
            10
        }
        0xE9 => {
            // JP (HL): the target is HL itself. The parentheses are Zilog's, and
            // they are wrong. No operand is fetched, so nothing reaches the latch.
            cpu.pc = cpu.hl();
            cpu.q = 0;
            4
        }
        0xCD => {
            let nn = cpu.imm16(bus);
            flow::call(cpu, bus, nn);
            cpu.wz = nn;
            cpu.q = 0;
            17
        }
        0xC4 | 0xCC | 0xD4 | 0xDC | 0xE4 | 0xEC | 0xF4 | 0xFC => {
            let nn = cpu.imm16(bus);
            cpu.q = 0;
            let taken = flow::cond(cpu, (op >> 3) & 7);
            if taken {
                flow::call(cpu, bus, nn);
            }
            cpu.wz = nn;
            if taken {
                17
            } else {
                10
            }
        }
        0xC9 => {
            flow::ret(cpu, bus);
            cpu.wz = cpu.pc;
            cpu.q = 0;
            10
        }
        0xC0 | 0xC8 | 0xD0 | 0xD8 | 0xE0 | 0xE8 | 0xF0 | 0xF8 => {
            cpu.q = 0;
            if flow::cond(cpu, (op >> 3) & 7) {
                flow::ret(cpu, bus);
                cpu.wz = cpu.pc;
                11
            } else {
                5
            }
        }
        0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => {
            flow::rst(cpu, bus, (op >> 3) & 7);
            cpu.wz = cpu.pc;
            cpu.q = 0;
            11
        }
        // EX (SP),HL: the word at the top of the stack, swapped with HL.
        0xE3 => {
            let lo = bus.read(cpu.sp);
            let hi = bus.read(cpu.sp.wrapping_add(1));
            // `H` goes back first, at `SP + 1`, before `L` at `SP` -- the reverse
            // of the read order, and what the per-T-state trace records.
            bus.write(cpu.sp.wrapping_add(1), cpu.h);
            bus.write(cpu.sp, cpu.l);
            cpu.h = hi;
            cpu.l = lo;
            cpu.wz = cpu.hl();
            cpu.q = 0;
            19
        }
        // IN A,(n) and OUT (n),A. The port's high byte is A, not zero -- the
        // reason `Bus::port_in` takes 16 bits.
        0xDB => {
            let n = cpu.imm(bus);
            let port = u16::from(cpu.a) << 8 | u16::from(n);
            cpu.a = bus.port_in(port);
            cpu.wz = port.wrapping_add(1);
            cpu.q = 0;
            11
        }
        0xD3 => {
            let n = cpu.imm(bus);
            let port = u16::from(cpu.a) << 8 | u16::from(n);
            bus.port_out(port, cpu.a);
            // A port write follows the same rule as a memory write: the byte on the
            // data bus lands in the latch's high half. Here that byte is `A`, which
            // is also the port's high half, so the two are indistinguishable --
            // hence [`wz_after_write`] rather than a hand-written expression.
            cpu.wz = wz_after_write(port, cpu.a);
            cpu.q = 0;
            11
        }
        0xCB => cb_page(cpu, bus),
        // Only the four prefixes are left: ED, DD, FD, and the double-prefix
        // forms, which Tasks 10 through 12 fill in. Until then an unimplemented
        // opcode is a panic *in development only*: it is unreachable once the suite
        // is green, and a silent 4-T-state NOP here would make a missing instruction
        // look like a flag bug across a hundred vector files. Task 12 deletes this
        // arm and lets the compiler prove the match exhaustive.
        other => unimplemented!("base opcode {other:#04X}"),
    }
}

/// The `CB` page: rotates and shifts, then `BIT`, `RES` and `SET`.
///
/// Four uniform quarters of 64, with one asymmetry: `BIT b,(HL)` is 12 T-states
/// rather than 15, because it writes nothing back.
///
/// No opcode on this page touches [`Z80::wz`] — confirmed over all 256 files. That
/// matters for `BIT b,(HL)`, whose F3 and F5 come from the *incoming* latch's high
/// byte: they are the residue of whatever earlier instruction last wrote it, which
/// is why a core must carry `wz` between instructions rather than recompute it.
pub fn cb_page<B: Bus>(cpu: &mut Z80, bus: &mut B) -> u32 {
    let op = cpu.fetch(bus);
    let (slot, bit) = (op & 7, (op >> 3) & 7);
    let mem = slot == 6;
    match op {
        0x00..=0x3F => {
            let v = reg(cpu, bus, slot);
            let r = bits::rot(cpu, bits::RotOp::from_index(bit), v);
            set_reg(cpu, bus, slot, r);
            if mem {
                15
            } else {
                8
            }
        }
        0x40..=0x7F => {
            let v = reg(cpu, bus, slot);
            // The register forms take F3/F5 from the operand; `(HL)` takes them from
            // the latch's high byte. Measured: for `cb_46` the latch is right on all
            // 1,000 cases and `H` -- which the plan named -- is wrong on 760.
            let f35 = if mem { (cpu.wz >> 8) as u8 } else { v };
            bits::bit_test(cpu, bit, v, f35);
            if mem {
                12
            } else {
                8
            }
        }
        0x80..=0xBF => {
            let v = reg(cpu, bus, slot);
            set_reg(cpu, bus, slot, v & !(1 << bit));
            cpu.q = 0;
            if mem {
                15
            } else {
                8
            }
        }
        0xC0..=0xFF => {
            let v = reg(cpu, bus, slot);
            set_reg(cpu, bus, slot, v | (1 << bit));
            cpu.q = 0;
            if mem {
                15
            } else {
                8
            }
        }
    }
}

/// The latch after a single-byte write through an address: `addr + 1` in the low
/// half, **the byte written** in the high half.
///
/// The asymmetry with a read is the whole reason this helper exists. On a read the
/// latch is just `addr + 1`; on a write the low half increments but the high half
/// is overwritten by whatever was on the data bus. Nothing documents it — it is a
/// measured fact, confirmed on all 4,000 cases of `02`, `12` and `32`, where
/// `addr + 1` alone is wrong on every case whose written byte differs from the
/// address's own high byte.
fn wz_after_write(addr: u16, val: u8) -> u16 {
    u16::from(val) << 8 | u16::from(addr.wrapping_add(1) as u8)
}

/// The `B C D E H L (HL) A` encoding, as used by bits 2–0 and 5–3 everywhere.
///
/// Index 6 is `(HL)` — a memory access, not a register — which is why this takes
/// a bus and why the T-state costs differ by three for that one index.
pub fn reg<B: Bus>(cpu: &mut Z80, bus: &mut B, i: u8) -> u8 {
    match i {
        0 => cpu.b,
        1 => cpu.c,
        2 => cpu.d,
        3 => cpu.e,
        4 => cpu.h,
        5 => cpu.l,
        6 => bus.read(cpu.hl()),
        7 => cpu.a,
        _ => unreachable!("register index {i} is not three bits"),
    }
}

/// Writes through the same encoding.
pub fn set_reg<B: Bus>(cpu: &mut Z80, bus: &mut B, i: u8, v: u8) {
    match i {
        0 => cpu.b = v,
        1 => cpu.c = v,
        2 => cpu.d = v,
        3 => cpu.e = v,
        4 => cpu.h = v,
        5 => cpu.l = v,
        6 => bus.write(cpu.hl(), v),
        7 => cpu.a = v,
        _ => unreachable!("register index {i} is not three bits"),
    }
}

/// The `BC DE HL SP` encoding for bits 5–4.
fn rp(cpu: &Z80, i: u8) -> u16 {
    match i {
        0 => cpu.bc(),
        1 => cpu.de(),
        2 => cpu.hl(),
        3 => cpu.sp,
        _ => unreachable!("pair index {i} is not two bits"),
    }
}

fn set_rp(cpu: &mut Z80, i: u8, v: u16) {
    match i {
        0 => cpu.set_bc(v),
        1 => cpu.set_de(v),
        2 => cpu.set_hl(v),
        3 => cpu.sp = v,
        _ => unreachable!("pair index {i} is not two bits"),
    }
}

/// The eight ALU operations, in their encoded order.
///
/// `ADD ADC SUB SBC AND XOR OR CP` — note that `XOR` precedes `OR`, which is not
/// the order they are usually listed in and is the order the opcodes use.
fn alu_op(cpu: &mut Z80, which: u8, v: u8) {
    match which {
        0 => alu::add(cpu, v, false),
        1 => alu::add(cpu, v, true),
        2 => alu::sub(cpu, v, false),
        3 => alu::sub(cpu, v, true),
        4 => alu::and(cpu, v),
        5 => alu::xor(cpu, v),
        6 => alu::or(cpu, v),
        7 => alu::cp(cpu, v),
        _ => unreachable!("ALU index {which} is not three bits"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testbus::Mem;

    /// Runs one instruction at 0x100 and returns its T-states.
    fn run(c: &mut Z80, prog: &[u8]) -> u32 {
        c.pc = 0x100;
        let mut m = Mem::at(0x100, prog);
        c.step(&mut m)
    }

    /// The register field is `B C D E H L (HL) A`, in that order.
    ///
    /// The order is the substance of the whole `0x40..=0x7F` range: get index 6 and
    /// 7 the wrong way round and 30 opcodes read a register where the hardware
    /// reads memory. Each `LD A,r` is loaded from a distinct value so a transposed
    /// pair cannot pass.
    #[test]
    fn the_register_field_orders_the_registers_with_memory_at_six() {
        let mut m = Mem::new();
        let mut c = Z80::new();
        c.b = 0x10;
        c.c = 0x11;
        c.d = 0x12;
        c.e = 0x13;
        c.h = 0x40; // so HL is 0x4014 -- inside RAM, and not equal to any register
        c.l = 0x14;
        c.a = 0x17;
        m.ram[0x4014] = 0x16;
        for (i, want) in [0x10u8, 0x11, 0x12, 0x13, 0x40, 0x14, 0x16, 0x17]
            .into_iter()
            .enumerate()
        {
            let i = u8::try_from(i).expect("eight indices fit a byte");
            assert_eq!(
                reg(&mut c, &mut m, i),
                want,
                "index {i} is not the right register"
            );
        }
        // And writes go to the same eight places. Index 6 must land in memory, not
        // in a register: that is what the read side above cannot prove on its own.
        set_reg(&mut c, &mut m, 6, 0x99);
        assert_eq!(m.ram[0x4014], 0x99, "index 6 writes through (HL)");
        set_reg(&mut c, &mut m, 4, 0x40);
        assert_eq!(c.h, 0x40, "index 4 is H");
    }

    /// `LD r,r'` decodes destination from bits 5–3 and source from bits 2–0.
    ///
    /// Getting these two fields the wrong way round is the single likeliest bug in
    /// the range, and it is invisible on any `LD r,r` where the two are equal.
    /// `0x47` is `LD B,A`: after it, `B` holds `A`'s value and `A` is unchanged.
    #[test]
    fn ld_r_r_takes_the_destination_from_the_high_field() {
        let mut c = Z80::new();
        c.a = 0x5A;
        c.b = 0x00;
        assert_eq!(run(&mut c, &[0x47]), 4, "LD B,A is 4 T-states");
        assert_eq!(c.b, 0x5A, "0x47 is LD B,A, not LD A,B");
        assert_eq!(c.a, 0x5A);
        assert_eq!(c.q, 0, "a load writes no flags");

        // And the reverse encoding moves the other way.
        let mut c = Z80::new();
        c.a = 0x00;
        c.b = 0x5A;
        run(&mut c, &[0x78]);
        assert_eq!(c.a, 0x5A, "0x78 is LD A,B");
    }

    /// Touching `(HL)` costs three more T-states, at either end.
    #[test]
    fn a_memory_operand_costs_three_extra_t_states() {
        let mut c = Z80::new();
        c.set_hl(0x4000);
        assert_eq!(run(&mut c, &[0x7E]), 7, "LD A,(HL)");
        let mut c = Z80::new();
        c.set_hl(0x4000);
        assert_eq!(run(&mut c, &[0x77]), 7, "LD (HL),A");
        let mut c = Z80::new();
        assert_eq!(run(&mut c, &[0x78]), 4, "LD A,B touches no memory");
    }

    /// The eight ALU operations are `ADD ADC SUB SBC AND XOR OR CP`.
    ///
    /// `XOR` before `OR` is the order the opcodes use and not the order they are
    /// usually listed in, so the two are easy to swap — and swapping them is
    /// invisible on any operand where the two agree. `A = 0x0F` against `0x30`
    /// distinguishes them: `XOR` gives 0x3F and `OR` gives 0x3F as well, so the
    /// operand here is 0x03, where `XOR` gives 0x0C and `OR` gives 0x0F.
    #[test]
    fn the_alu_field_orders_the_operations_with_xor_before_or() {
        // 0xA8..0xAF is XOR r, 0xB0..0xB7 is OR r. Both against B = 0x03.
        let mut c = Z80::new();
        c.a = 0x0F;
        c.b = 0x03;
        run(&mut c, &[0xA8]);
        assert_eq!(c.a, 0x0C, "0xA8 is XOR B");

        let mut c = Z80::new();
        c.a = 0x0F;
        c.b = 0x03;
        run(&mut c, &[0xB0]);
        assert_eq!(c.a, 0x0F, "0xB0 is OR B");

        // And the first four, which differ in whether they carry and which way.
        for (op, name, want) in [
            (0x80u8, "ADD B", 0x13u8),
            (0x88, "ADC B", 0x14),
            (0x90, "SUB B", 0x0D),
            (0x98, "SBC B", 0x0C),
        ] {
            let mut c = Z80::new();
            c.a = 0x10;
            c.b = 0x03;
            c.f = C;
            run(&mut c, &[op]);
            assert_eq!(c.a, want, "{name}");
        }

        // CP leaves A alone; a mis-decode to SUB would not.
        let mut c = Z80::new();
        c.a = 0x10;
        c.b = 0x03;
        run(&mut c, &[0xB8]);
        assert_eq!(c.a, 0x10, "0xB8 is CP B");
        assert_eq!(c.f & C, 0, "0x10 - 0x03 does not borrow");
    }

    /// `LD (rr),A` leaves the **written byte** in the latch's high half.
    ///
    /// The rule that cost 1,000 of 1,000 cases on `02.z80bin` before it was found.
    /// `A` is chosen to differ from the address's own high byte, which is the only
    /// way to tell this rule from a plain `addr + 1`.
    #[test]
    fn a_write_puts_the_written_byte_in_the_latch_high_half() {
        let mut c = Z80::new();
        c.set_bc(0x4012);
        c.a = 0x99;
        run(&mut c, &[0x02]);
        assert_eq!(c.wz, 0x9913, "0x99 over the low byte of 0x4013");

        // A read is the plain increment, and that difference is the point.
        let mut c = Z80::new();
        c.set_bc(0x4012);
        run(&mut c, &[0x0A]);
        assert_eq!(c.wz, 0x4013, "a read increments and keeps the high byte");
    }

    /// The 16-bit `(nn)` forms increment the latch once, not twice.
    #[test]
    fn the_sixteen_bit_memory_forms_increment_the_latch_once() {
        let mut c = Z80::new();
        c.set_hl(0x1234);
        run(&mut c, &[0x22, 0x00, 0x40]);
        assert_eq!(c.wz, 0x4001, "LD (nn),HL leaves nn + 1");

        let mut c = Z80::new();
        run(&mut c, &[0x2A, 0x00, 0x40]);
        assert_eq!(c.wz, 0x4001, "and so does LD HL,(nn)");
    }

    /// `ADD HL,rr` latches the **old** `HL` plus one, not the sum.
    #[test]
    fn add_hl_latches_the_old_hl_plus_one() {
        let mut c = Z80::new();
        c.set_hl(0x1000);
        c.set_de(0x0234);
        run(&mut c, &[0x19]);
        assert_eq!(c.hl(), 0x1234, "the sum lands in HL");
        assert_eq!(c.wz, 0x1001, "and the latch holds the old HL plus one");
    }

    /// `LD (nn),HL` writes `L` first, at `nn`.
    ///
    /// Little-endian, and a core that wrote `H` first would still round-trip
    /// through its own `LD HL,(nn)` — so the bytes are inspected in memory.
    #[test]
    fn ld_nn_hl_writes_the_low_byte_first() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x1234);
        let mut m = Mem::at(0x100, &[0x22, 0x00, 0x40]);
        assert_eq!(c.step(&mut m), 16);
        assert_eq!(m.ram[0x4000], 0x34, "L at nn");
        assert_eq!(m.ram[0x4001], 0x12, "H at nn + 1");
    }

    /// `INC rr` and `DEC rr` write no flags at all — not even `Z`.
    ///
    /// The 16-bit increments go through the address adder, not the ALU, which is
    /// why. A core that reused `inc8` here would set `Z` on a wrap to zero and fail
    /// every case of `23.z80bin` whose `HL` was 0xFFFF.
    #[test]
    fn the_sixteen_bit_increments_write_no_flags() {
        let mut c = Z80::new();
        c.set_hl(0xFFFF);
        c.f = 0x00;
        assert_eq!(run(&mut c, &[0x23]), 6);
        assert_eq!(c.hl(), 0x0000, "and it wraps");
        assert_eq!(c.f, 0x00, "no flag was written, though the result is zero");
        assert_eq!(c.q, 0);

        let mut c = Z80::new();
        c.set_bc(0x0000);
        c.f = 0xFF;
        run(&mut c, &[0x0B]);
        assert_eq!(c.bc(), 0xFFFF);
        assert_eq!(c.f, 0xFF, "nor is any flag cleared");
    }

    /// The pair field is `BC DE HL SP` for `LD rr,nn` and `BC DE HL AF` for `PUSH`.
    ///
    /// The two encodings differ in index 3 alone, and that one difference is the
    /// reason `PUSH`/`POP` cannot share `rp`. A `PUSH AF` that pushed `SP` would
    /// look plausible in isolation.
    #[test]
    fn index_three_is_sp_for_loads_and_af_for_the_stack() {
        let mut c = Z80::new();
        run(&mut c, &[0x31, 0x34, 0x12]);
        assert_eq!(c.sp, 0x1234, "0x31 is LD SP,nn");

        let mut c = Z80::new();
        c.pc = 0x100;
        c.sp = 0x4000;
        c.a = 0xAB;
        c.f = 0xCD;
        let mut m = Mem::at(0x100, &[0xF5]);
        assert_eq!(c.step(&mut m), 11);
        assert_eq!(c.sp, 0x3FFE);
        assert_eq!(m.ram[0x3FFF], 0xAB, "0xF5 pushes A, not SP's high byte");
        assert_eq!(m.ram[0x3FFE], 0xCD, "and F");
    }

    /// `POP AF` writes the flag register wholesale, and clears `Q`.
    ///
    /// Both halves matter. The flags are not computed — every bit comes from memory,
    /// including the two undocumented ones and the bits no instruction sets — and
    /// because they were not computed, `Q` is zero, so a following `SCF` reads them
    /// through `f & !q`. A `POP AF` that set `q = f` would corrupt the *next*
    /// instruction.
    #[test]
    fn pop_af_loads_every_flag_bit_and_leaves_q_clear() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.sp = 0x4000;
        c.q = 0xFF;
        let mut m = Mem::at(0x100, &[0xF1]);
        m.ram[0x4000] = 0xFF;
        m.ram[0x4001] = 0x12;
        assert_eq!(c.step(&mut m), 10);
        assert_eq!(c.a, 0x12);
        assert_eq!(c.f, 0xFF, "every bit, including the ones nothing sets");
        assert_eq!(c.sp, 0x4002);
        assert_eq!(c.q, 0, "the flags were loaded, not computed");
    }

    /// `LD (HL),n` costs 10 T-states; `LD r,n` costs 7.
    #[test]
    fn the_immediate_load_costs_three_more_through_memory() {
        let mut c = Z80::new();
        assert_eq!(run(&mut c, &[0x3E, 0x5A]), 7, "LD A,n");
        assert_eq!(c.a, 0x5A);

        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x4000);
        let mut m = Mem::at(0x100, &[0x36, 0x5A]);
        assert_eq!(c.step(&mut m), 10, "LD (HL),n");
        assert_eq!(m.ram[0x4000], 0x5A);
    }

    /// `INC (HL)` reads, increments and writes back, for 11 T-states.
    #[test]
    fn inc_through_memory_writes_the_result_back() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x4000);
        let mut m = Mem::at(0x100, &[0x34]);
        m.ram[0x4000] = 0x0F;
        assert_eq!(c.step(&mut m), 11);
        assert_eq!(m.ram[0x4000], 0x10, "the incremented byte went back");
        assert_eq!(c.f & H, H, "and 0x0F + 1 half-carries");
    }

    /// `LD SP,HL` is 6 T-states and copies in the one direction.
    #[test]
    fn ld_sp_hl_copies_hl_into_sp() {
        let mut c = Z80::new();
        c.set_hl(0x1234);
        c.sp = 0xFFFF;
        assert_eq!(run(&mut c, &[0xF9]), 6);
        assert_eq!(c.sp, 0x1234);
        assert_eq!(c.hl(), 0x1234, "HL is unchanged");
    }

    /// A value no instruction here could produce, so "the latch was left alone" and
    /// "the latch was written with the right answer" cannot be confused.
    const SENTINEL: u16 = 0x5EED;

    /// A **taken** relative jump latches its target; one not taken leaves the latch.
    ///
    /// Three rules are distinguishable here and two of them are wrong: `pc + 2`
    /// (the instruction after) and `pc + 2 + d` computed unconditionally. Both were
    /// measured at 100% wrong on every not-taken case of `20.z80bin` through
    /// `38.z80bin`, which is what established that the latch is untouched.
    #[test]
    fn a_relative_jump_latches_its_target_only_when_taken() {
        // JR NZ,+4 with Z clear: taken.
        let mut c = Z80::new();
        c.wz = SENTINEL;
        c.f = 0;
        assert_eq!(run(&mut c, &[0x20, 0x04]), 12);
        assert_eq!(c.pc, 0x106, "0x102 + 4");
        assert_eq!(c.wz, 0x106, "the latch holds the target");

        // The same instruction with Z set: not taken, and the latch is untouched --
        // not 0x102, which is where PC ends up.
        let mut c = Z80::new();
        c.wz = SENTINEL;
        c.f = Z;
        assert_eq!(run(&mut c, &[0x20, 0x04]), 7);
        assert_eq!(c.pc, 0x102);
        assert_eq!(c.wz, SENTINEL, "a jump not taken writes no latch");

        // DJNZ follows the same rule, on B rather than a flag.
        let mut c = Z80::new();
        c.wz = SENTINEL;
        c.b = 1; // decrements to zero: not taken
        assert_eq!(run(&mut c, &[0x10, 0x04]), 8);
        assert_eq!(c.wz, SENTINEL);
        let mut c = Z80::new();
        c.wz = SENTINEL;
        c.b = 2;
        assert_eq!(run(&mut c, &[0x10, 0x04]), 13);
        assert_eq!(c.wz, 0x106);
    }

    /// An **absolute** jump or call latches `nn` whether or not it is taken.
    ///
    /// This is the rule that makes the relative and absolute forms different, and
    /// carrying the relative rule over to `JP cc` would fail every not-taken case
    /// of eight vector files. The target is chosen to differ from both `PC` values.
    #[test]
    fn an_absolute_jump_latches_its_operand_even_when_not_taken() {
        for (op, name) in [(0xC2u8, "JP NZ,nn"), (0xC4, "CALL NZ,nn")] {
            let mut c = Z80::new();
            c.wz = SENTINEL;
            c.sp = 0x8000;
            c.f = Z; // Z set: NZ is false, so not taken
            run(&mut c, &[op, 0x78, 0x56]);
            assert_eq!(c.pc, 0x103, "{name} was not taken");
            assert_eq!(c.wz, 0x5678, "{name} latches nn regardless");
        }

        // And the unconditional forms, which have nothing to be regardless of.
        let mut c = Z80::new();
        c.wz = SENTINEL;
        run(&mut c, &[0xC3, 0x78, 0x56]);
        assert_eq!(c.wz, 0x5678, "JP nn");

        // JP (HL) fetches no operand, so nothing reaches the latch. A core that
        // wrote HL here would look right in a debugger and fail `e9.z80bin`.
        let mut c = Z80::new();
        c.wz = SENTINEL;
        c.set_hl(0x4000);
        assert_eq!(run(&mut c, &[0xE9]), 4);
        assert_eq!(c.pc, 0x4000);
        assert_eq!(
            c.wz, SENTINEL,
            "JP (HL) reads no operand and latches nothing"
        );
    }

    /// `RET` latches the address it popped; `RET cc` not taken latches nothing.
    #[test]
    fn a_return_latches_the_popped_address_only_when_taken() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.wz = SENTINEL;
        c.sp = 0x8000;
        let mut m = Mem::at(0x100, &[0xC9]);
        m.ram[0x8000] = 0x78;
        m.ram[0x8001] = 0x56;
        assert_eq!(c.step(&mut m), 10);
        assert_eq!(c.pc, 0x5678);
        assert_eq!(c.wz, 0x5678);

        // RET NZ with Z set: nothing is popped, and nothing is latched.
        let mut c = Z80::new();
        c.wz = SENTINEL;
        c.sp = 0x8000;
        c.f = Z;
        assert_eq!(run(&mut c, &[0xC0]), 5);
        assert_eq!(c.sp, 0x8000, "the stack was not touched");
        assert_eq!(c.wz, SENTINEL);
    }

    /// `RST n` latches its fixed target, and pushes the address after itself.
    #[test]
    fn rst_latches_its_fixed_target() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.wz = SENTINEL;
        c.sp = 0x8000;
        let mut m = Mem::at(0x100, &[0xEF]); // RST 28h
        assert_eq!(c.step(&mut m), 11);
        assert_eq!(c.pc, 0x28);
        assert_eq!(c.wz, 0x28);
        assert_eq!(m.ram[0x7FFF], 0x01, "0x101 was pushed, high byte first");
        assert_eq!(m.ram[0x7FFE], 0x01);
    }

    /// `IN A,(n)` latches `port + 1` across all 16 bits; `OUT (n),A` truncates.
    ///
    /// `n = 0xFF` is the one operand that separates them. Reading, the increment
    /// carries into the high half: `0x12FF + 1` is `0x1300`. Writing, only the low
    /// half increments and the high half takes the byte on the data bus — which for
    /// `OUT (n),A` is `A` — so the answer is `0x1200`, not `0x1300`. Any other `n`
    /// makes the two rules agree.
    #[test]
    fn the_port_instructions_differ_in_the_latch_on_a_carry() {
        let mut c = Z80::new();
        c.wz = SENTINEL;
        c.a = 0x12;
        assert_eq!(run(&mut c, &[0xDB, 0xFF]), 11, "IN A,(n)");
        assert_eq!(c.wz, 0x1300, "a read carries into the high half");

        let mut c = Z80::new();
        c.wz = SENTINEL;
        c.a = 0x12;
        assert_eq!(run(&mut c, &[0xD3, 0xFF]), 11, "OUT (n),A");
        assert_eq!(c.wz, 0x1200, "a write does not, and A is the high half");
    }

    /// `SET` and `RES` touch no flags whatsoever, and clear `Q`.
    #[test]
    fn set_and_res_write_no_flags() {
        let mut c = Z80::new();
        c.b = 0x00;
        c.f = 0x5A;
        c.q = 0x5A;
        assert_eq!(run(&mut c, &[0xCB, 0xC0]), 8, "SET 0,B");
        assert_eq!(c.b, 0x01);
        assert_eq!(c.f, 0x5A, "no flags");
        assert_eq!(c.q, 0, "and Q clears, because none were written");

        assert_eq!(run(&mut c, &[0xCB, 0x80]), 8, "RES 0,B");
        assert_eq!(c.b, 0x00);
        assert_eq!(c.f, 0x5A);
    }

    /// The bit field is bits 5–3, and it selects a bit rather than an operation.
    ///
    /// `SET 5,B` and `SET 3,B` differ only in that field. A decoder that read the
    /// register field for the bit number would set bit 0 of every register here and
    /// pass any test whose bit number happened to be zero.
    #[test]
    fn the_bit_field_is_bits_five_to_three() {
        for (op, want) in [(0xC0u8, 0x01u8), (0xC8, 0x02), (0xE8, 0x20), (0xFF, 0x80)] {
            let mut c = Z80::new();
            c.b = 0;
            c.a = 0;
            run(&mut c, &[0xCB, op]);
            // 0xFF is SET 7,A; the rest are SET n,B.
            let got = if op == 0xFF { c.a } else { c.b };
            assert_eq!(got, want, "CB {op:#04X}");
        }
    }

    /// The page's T-state costs: 8 for a register, 15 for `(HL)`, and **12 for
    /// `BIT b,(HL)`** — which writes nothing back and so is three cheaper.
    #[test]
    fn the_cb_page_costs_eight_fifteen_or_twelve() {
        let mut c = Z80::new();
        c.set_hl(0x2000);
        assert_eq!(run(&mut c, &[0xCB, 0x00]), 8, "RLC B: a register form");
        assert_eq!(
            run(&mut c, &[0xCB, 0x06]),
            15,
            "RLC (HL): read, modify, write"
        );
        assert_eq!(
            run(&mut c, &[0xCB, 0x46]),
            12,
            "BIT 0,(HL) writes nothing back"
        );
        assert_eq!(run(&mut c, &[0xCB, 0x86]), 15, "RES 0,(HL) does");
    }

    /// `BIT b,(HL)` reads the byte but writes nothing back.
    ///
    /// The T-state count above is the cheap half of this; the expensive half is that
    /// a `cb_page` reusing the rotate quarter's `set_reg` call would write the
    /// operand back unchanged. RAM would be identical, so only the write log shows
    /// it — and the vectors compare the bus.
    #[test]
    fn bit_through_memory_writes_nothing() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x2000);
        let mut m = Mem::at(0x100, &[0xCB, 0x46]);
        m.ram[0x2000] = 0x5A;
        c.step(&mut m);
        assert!(m.writes.is_empty(), "BIT (HL) is read-only");
    }

    /// `BIT b,(HL)` takes F3 and F5 from the latch; the register forms do not.
    ///
    /// Two runs of the same bit number on the same value, differing only in the
    /// addressing mode, with the latch's high byte set to the complement of the
    /// operand's F3/F5 — so a decoder that passed the operand for both forms, or
    /// the latch for both, fails one of the two assertions.
    #[test]
    fn bit_through_memory_takes_the_undocumented_flags_from_the_latch() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x2000);
        c.wz = u16::from(F5 | F3) << 8; // both set in the latch
        let mut m = Mem::at(0x100, &[0xCB, 0x46]);
        m.ram[0x2000] = 0x01; // bit 0 set, and neither F3 nor F5
        c.step(&mut m);
        assert_eq!(c.f & (F5 | F3), F5 | F3, "from the latch, not the operand");
        assert_eq!(
            c.wz,
            u16::from(F5 | F3) << 8,
            "and the page leaves wz alone"
        );

        // The register form, same bit and same operand, takes them from the operand.
        let mut c = Z80::new();
        c.b = 0x01;
        c.wz = u16::from(F5 | F3) << 8;
        run(&mut c, &[0xCB, 0x40]); // BIT 0,B
        assert_eq!(c.f & (F5 | F3), 0, "the operand has neither");
    }

    /// `EX (SP),HL` latches the **new** `HL` — the word it took off the stack.
    #[test]
    fn ex_sp_hl_latches_the_word_it_loaded() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.wz = SENTINEL;
        c.sp = 0x8000;
        c.set_hl(0x1234);
        let mut m = Mem::at(0x100, &[0xE3]);
        m.ram[0x8000] = 0x78;
        m.ram[0x8001] = 0x56;
        assert_eq!(c.step(&mut m), 19);
        assert_eq!(c.wz, 0x5678, "the new HL, not the old one");
    }
}
