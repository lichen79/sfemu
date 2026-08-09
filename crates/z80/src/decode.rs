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
use crate::ops::{alu, bits, flow, io, load, Block};
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
        0xED => ed_page(cpu, bus),
        0xDD => index_page(cpu, bus, false),
        0xFD => index_page(cpu, bus, true),
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

/// The `ED` page: 80 defined opcodes out of 256.
///
/// The undefined ones behave as two `NOP`s — the prefix having already cost its M1 —
/// and upstream ships no file for them, so they are reachable and unverifiable. That
/// is why the fallthrough arm is explicit about what it is doing rather than being a
/// `unimplemented!`.
///
/// # The latch on this page
///
/// None of these rules is documented anywhere; each was measured against the 80
/// `ed_*` files, 1,000 cases each, and the consolidated model is wrong on none.
///
/// | instruction | latch |
/// |---|---|
/// | `IN r,(C)`, `OUT (C),r` | `BC + 1` — *not* the base page's write rule |
/// | `ADC HL,rr`, `SBC HL,rr` | the **old** `HL` plus one, as `ADD HL,rr` |
/// | `LD (nn),rr`, `LD rr,(nn)` | `nn + 1` |
/// | `RETN`, `RETI` | the popped address |
/// | `RLD`, `RRD` | `HL + 1` |
/// | `LDI`, `LDD` | **unchanged** — the only memory writes on the chip that leave it |
/// | `CPI`, `CPD` | the incoming latch **plus or minus one** |
/// | `INI`, `IND` | `BC` stepped by direction, with `B` **before** the decrement |
/// | `OUTI`, `OUTD` | the same stepping, with `B` **after** it — `BC - 0x100` |
/// | any repeating form, while repeating | `PC + 1` after the rewind |
///
/// `NEG`, `IM n`, `LD I,A`, `LD R,A`, `LD A,I`, `LD A,R` and the two undefined stems
/// with files leave it alone.
pub fn ed_page<B: Bus>(cpu: &mut Z80, bus: &mut B) -> u32 {
    let op = cpu.fetch(bus);
    match op {
        // IN r,(C). 0x70 is `IN (C)`: it flags the byte and discards it, writing no
        // register and no memory -- confirmed over 1,000 cases of `ed_70`.
        0x40 | 0x48 | 0x50 | 0x58 | 0x60 | 0x68 | 0x70 | 0x78 => {
            let v = io::in_r_c(cpu, bus);
            let dst = (op >> 3) & 7;
            if dst != 6 {
                set_reg(cpu, bus, dst, v);
            }
            12
        }
        // OUT (C),r. 0x71 outputs **zero** on an NMOS Z80 -- 1,000 of 1,000 cases of
        // `ed_71` -- where a CMOS part outputs 0xFF. The suite is NMOS, and this is
        // the one bit of the page that is a hardware-revision choice rather than a
        // fact.
        0x41 | 0x49 | 0x51 | 0x59 | 0x61 | 0x69 | 0x71 | 0x79 => {
            let src = (op >> 3) & 7;
            let v = if src == 6 { 0 } else { reg(cpu, bus, src) };
            io::out_c_r(cpu, bus, v);
            12
        }
        // SBC HL,rr and ADC HL,rr. The latch takes the *old* HL plus one, which is
        // why it is read before the operation rather than after.
        0x42 | 0x52 | 0x62 | 0x72 => {
            cpu.wz = cpu.hl().wrapping_add(1);
            let r = alu::sbc16(cpu, cpu.hl(), rp(cpu, (op >> 4) & 3));
            cpu.set_hl(r);
            15
        }
        0x4A | 0x5A | 0x6A | 0x7A => {
            cpu.wz = cpu.hl().wrapping_add(1);
            let r = alu::adc16(cpu, cpu.hl(), rp(cpu, (op >> 4) & 3));
            cpu.set_hl(r);
            15
        }
        // LD (nn),rr and LD rr,(nn). One latch rule for both directions, unlike the
        // base page's single-byte forms, where a write puts the byte in the high half.
        0x43 | 0x53 | 0x63 | 0x73 => {
            let nn = cpu.imm16(bus);
            let v = rp(cpu, (op >> 4) & 3);
            bus.write(nn, v as u8);
            bus.write(nn.wrapping_add(1), (v >> 8) as u8);
            cpu.wz = nn.wrapping_add(1);
            cpu.q = 0;
            20
        }
        0x4B | 0x5B | 0x6B | 0x7B => {
            let nn = cpu.imm16(bus);
            let lo = bus.read(nn);
            let hi = bus.read(nn.wrapping_add(1));
            set_rp(cpu, (op >> 4) & 3, u16::from(hi) << 8 | u16::from(lo));
            cpu.wz = nn.wrapping_add(1);
            cpu.q = 0;
            20
        }
        0x44 | 0x4C | 0x54 | 0x5C | 0x64 | 0x6C | 0x74 | 0x7C => {
            alu::neg(cpu);
            8
        }
        // RETN and RETI: a return that copies IFF2 back into IFF1.
        //
        // The copy is the whole point -- an interrupt handler entered through mode 1
        // has IFF1 cleared and IFF2 holding the pre-interrupt state -- and it is
        // measured: of `ed_45`'s 1,000 cases, 498 have the two flip-flops disagreeing
        // on entry, and the final IFF1 equals the entry IFF2 on all 498. IFF2 itself
        // does not move.
        0x45 | 0x4D | 0x55 | 0x5D | 0x65 | 0x6D | 0x75 | 0x7D => {
            cpu.iff1 = cpu.iff2;
            flow::ret(cpu, bus);
            cpu.wz = cpu.pc;
            cpu.q = 0;
            14
        }
        // IM 0 / IM 1 / IM 2, each with duplicate encodings. The duplicates are not
        // guesswork: every one has its own vector file and its own final mode.
        0x46 | 0x4E | 0x66 | 0x6E => {
            cpu.im = 0;
            cpu.q = 0;
            8
        }
        0x56 | 0x76 => {
            cpu.im = 1;
            cpu.q = 0;
            8
        }
        0x5E | 0x7E => {
            cpu.im = 2;
            cpu.q = 0;
            8
        }
        0x47 => {
            cpu.i = cpu.a;
            cpu.q = 0;
            9
        }
        0x4F => {
            // LD R,A writes all eight bits, bit 7 included -- which is what makes
            // `R`'s bit 7 sticky under `bump_r`. It is the one ED stem whose `R` delta
            // is not 2.
            cpu.r = cpu.a;
            cpu.q = 0;
            9
        }
        // LD A,I and LD A,R: P/V leaks IFF2, so software can read the interrupt
        // enable, which is otherwise invisible. `p` marks the instruction because an
        // interrupt landing here corrupts that flag on real hardware.
        0x57 | 0x5F => {
            cpu.a = if op == 0x57 { cpu.i } else { cpu.r };
            cpu.f = (cpu.f & C) | flags::sz53(cpu.a) | if cpu.iff2 { PV } else { 0 };
            cpu.q = cpu.f;
            // `p` really is the constant 1 here, not the flag value `q` holds: over
            // 1,000 cases each of `ed_57` and `ed_5f` it is 1 every time, and equal
            // to `f` on 23 and 69 respectively -- those being the cases where `f`
            // happens to be 1.
            cpu.p = 1;
            9
        }
        0x67 => {
            bits::rrd(cpu, bus);
            18
        }
        0x6F => {
            bits::rld(cpu, bus);
            18
        }
        // The block instructions. See [`Block`] for the two opcode bits they share
        // and `crate::ops::repeat` for why the repeating forms are not loops.
        0xA0 | 0xA8 | 0xB0 | 0xB8 => {
            let block = Block::from_opcode(op);
            load::ldi_ldd(cpu, bus, block);
            if block.repeating && cpu.bc() != 0 {
                crate::ops::repeat(cpu)
            } else {
                16
            }
        }
        0xA1 | 0xA9 | 0xB1 | 0xB9 => {
            let block = Block::from_opcode(op);
            load::cpi_cpd(cpu, bus, block);
            // CPIR and CPDR stop on a match as well as on a count of zero, which the
            // transfer forms have no equivalent of. Measured against the vectors'
            // final `PC` on 1,000 cases each: 0 mismatches.
            if block.repeating && cpu.bc() != 0 && cpu.f & flags::Z == 0 {
                crate::ops::repeat(cpu)
            } else {
                16
            }
        }
        0xA2 | 0xAA | 0xB2 | 0xBA => {
            let block = Block::from_opcode(op);
            let v = io::ini_ind(cpu, bus, block);
            block_io_tail(cpu, block, v)
        }
        0xA3 | 0xAB | 0xB3 | 0xBB => {
            let block = Block::from_opcode(op);
            let v = io::outi_outd(cpu, bus, block);
            block_io_tail(cpu, block, v)
        }
        // The undefined opcodes: two NOPs' worth of time, and no state change beyond
        // the two M1 cycles `fetch` has already charged to `R`. Upstream ships no
        // file for any of them, so this arm is the one part of the page that rests on
        // documentation rather than on measurement.
        _ => {
            cpu.q = 0;
            8
        }
    }
}

/// The `DD` and `FD` pages: 252 opcodes each.
///
/// A prefix is a rule for the *next* instruction, not 252 new instructions:
/// wherever it would use `HL`, use `IX`; wherever `(HL)`, use `(IX+d)` with a
/// signed displacement read after the opcode. `FD` is the same rule with `IY`.
///
/// Two rules fall out of that and are the ones a core gets wrong:
///
/// - `H` and `L` become `IXH` and `IXL`, so `DD 44` is `LD B,IXH` — a real
///   instruction with a real vector file. The prefix rewrites register operands,
///   not just memory ones.
/// - **Except** when the other operand is `(IX+d)`: `DD 66` is `LD H,(IX+d)` with
///   a plain `H`, because one instruction cannot use `IX` as both a pointer and a
///   register half.
///
/// `EX DE,HL` is untouched — it reaches `HL` by a path the encoding does not route
/// through this substitution. And an opcode with no `HL` in it simply costs 4 extra
/// T-states, which is a tested path (`DD 00`), not an error: every cost on this
/// page is `4 + the base-page cost`, confirmed per stem over all 504 files.
///
/// # The latch on this page
///
/// 81 of the 252 stems write [`Z80::wz`] and 171 leave it alone. 34 of the 81 are
/// fallthroughs, which take the base page's rules unchanged; the 47 that belong to
/// this page follow three rules, each measured 0-wrong over 1,000 cases per file on
/// both prefixes:
///
/// | instruction | latch |
/// |---|---|
/// | any `(IX+d)` form — loads, **stores**, ALU, `INC`/`DEC` | the computed `IX+d` |
/// | `ADD IX,rr` | the **old** `IX` plus one, as `ADD HL,rr` |
/// | `EX (SP),IX` | the **new** `IX` — the word that came off the stack |
/// | `LD (nn),IX`, `LD IX,(nn)` | `nn + 1` |
///
/// The stores are the trap: `LD (IX+d),r` writes a byte to memory, and every other
/// single-byte write on the chip puts that byte in the latch's high half — see
/// `wz_after_write`. These do not. The address wins, on all 7,000 cases of the
/// seven store stems.
pub fn index_page<B: Bus>(cpu: &mut Z80, bus: &mut B, use_iy: bool) -> u32 {
    // A prefix resets `Q`, because a prefix writes no flags — see [`Z80::q`]. Every
    // arm below overwrites it, so this line is invisible to all of them but one:
    // `SCF`/`CCF` reached through the fallthrough, whose F3/F5 come from
    // `A | (f & !q)`. Clearing here degrades that to `A | f`, which is what the
    // vectors show: on `dd_37`, `dd_3f` and `fd_37` the carried-over `Q` is wrong on
    // 239, 230 and 223 of 1,000 cases and `A | f` on none. It is the one place the
    // prefix changes an instruction's *result* rather than only its cost.
    cpu.q = 0;
    let op = cpu.fetch(bus);

    // A prefix restarts the rule: DD FD 21 loads IY. Re-dispatching here is what
    // makes that fall out rather than needing a special case. `ED` is not in this
    // list: `DD ED` is not a page switch but two wasted M1 cycles followed by the
    // `ED` page reached through the fallthrough, which is the same thing by a
    // shorter route.
    match op {
        0xDD => return 4 + index_page(cpu, bus, false),
        0xFD => return 4 + index_page(cpu, bus, true),
        // `DD CB` is the double-prefix page, whose operand order is unique on the
        // chip -- displacement *before* the opcode -- so it cannot fall through to
        // `cb_page`, which would read the displacement as its opcode.
        // No `4 +`: unlike the prefix-restart arms above, the double-prefix page's
        // costs are the whole instruction's, measured as 23 and 20 rather than
        // derived from a base-page figure.
        0xCB => return index_cb_page(cpu, bus, use_iy),
        _ => {}
    }

    let idx = if use_iy { cpu.iy } else { cpu.ix };

    /// Reads register `n` with `H`/`L` rewritten to the index halves.
    ///
    /// Index 6 never reaches here: every displaced form is handled by its own
    /// branch, because it must read the displacement byte first.
    macro_rules! ireg {
        ($n:expr) => {
            match $n {
                4 => (idx >> 8) as u8,
                5 => idx as u8,
                n => reg(cpu, bus, n),
            }
        };
    }

    /// Reads the displacement and returns the address it points at.
    macro_rules! displaced {
        () => {{
            let d = cpu.disp(bus);
            idx.wrapping_add(d as u16)
        }};
    }

    match op {
        // The 8-bit loads. Destination 6 or source 6 means displaced memory, and
        // that form suppresses the H/L rewrite on the other side. 0x76 is HALT,
        // which sits in the middle of the block and falls through.
        0x40..=0x7F if op != 0x76 => {
            let (dst, src) = ((op >> 3) & 7, op & 7);
            if dst == 6 {
                let addr = displaced!();
                // Plain `reg`: the source keeps H and L, not IXH and IXL.
                let v = reg(cpu, bus, src);
                bus.write(addr, v);
                // The address, not `wz_after_write`'s byte-in-the-high-half rule
                // that every other single-byte write on the chip follows.
                cpu.wz = addr;
                cpu.q = 0;
                19
            } else if src == 6 {
                let addr = displaced!();
                let v = bus.read(addr);
                // Plain `set_reg`, for the same reason.
                set_reg(cpu, bus, dst, v);
                cpu.wz = addr;
                cpu.q = 0;
                19
            } else {
                let v = ireg!(src);
                match dst {
                    4 => set_index(cpu, use_iy, (idx & 0x00FF) | u16::from(v) << 8),
                    5 => set_index(cpu, use_iy, (idx & 0xFF00) | u16::from(v)),
                    n => set_reg(cpu, bus, n, v),
                }
                cpu.q = 0;
                8
            }
        }
        // LD (IX+d),n -- two operands, and the displacement comes first.
        0x36 => {
            let addr = displaced!();
            let n = cpu.imm(bus);
            bus.write(addr, n);
            cpu.wz = addr;
            cpu.q = 0;
            19
        }
        // LD IXH,n and LD IXL,n. Without these the halves would fall through to the
        // base page and load plain `H`/`L` -- and, `LD r,n` writing no flags and no
        // latch, the only visible difference would be which register moved.
        0x26 | 0x2E => {
            let n = cpu.imm(bus);
            let v = if op == 0x26 {
                (idx & 0x00FF) | u16::from(n) << 8
            } else {
                (idx & 0xFF00) | u16::from(n)
            };
            set_index(cpu, use_iy, v);
            cpu.q = 0;
            11
        }
        // The 8-bit ALU against a register half or displaced memory.
        0x80..=0xBF => {
            let src = op & 7;
            let (v, t) = if src == 6 {
                let addr = displaced!();
                cpu.wz = addr;
                (bus.read(addr), 19)
            } else {
                (ireg!(src), 8)
            };
            alu_op(cpu, (op >> 3) & 7, v);
            t
        }
        // INC/DEC of displaced memory: a read-modify-write, carry preserved.
        0x34 | 0x35 => {
            let addr = displaced!();
            let v = bus.read(addr);
            let r = if op == 0x34 {
                alu::inc8(cpu, v)
            } else {
                alu::dec8(cpu, v)
            };
            bus.write(addr, r);
            cpu.wz = addr;
            23
        }
        // INC/DEC of a half. `alu::inc8` sets Q; the latch is untouched.
        0x24 | 0x25 | 0x2C | 0x2D => {
            let high = op < 0x2C;
            let v = if high { (idx >> 8) as u8 } else { idx as u8 };
            let r = if op & 1 == 0 {
                alu::inc8(cpu, v)
            } else {
                alu::dec8(cpu, v)
            };
            set_index(
                cpu,
                use_iy,
                if high {
                    (idx & 0x00FF) | u16::from(r) << 8
                } else {
                    (idx & 0xFF00) | u16::from(r)
                },
            );
            8
        }
        // The 16-bit forms: ADD IX,rr, with rr = 2 meaning IX itself rather than HL.
        0x09 | 0x19 | 0x29 | 0x39 => {
            let which = (op >> 4) & 3;
            let operand = if which == 2 { idx } else { rp(cpu, which) };
            let r = alu::add16(cpu, idx, operand);
            set_index(cpu, use_iy, r);
            // The old index plus one, as on the base page: the latch follows the
            // addend into the ALU rather than the sum out of it.
            cpu.wz = idx.wrapping_add(1);
            15
        }
        0x21 => {
            let nn = cpu.imm16(bus);
            set_index(cpu, use_iy, nn);
            cpu.q = 0;
            14
        }
        0x22 => {
            let nn = cpu.imm16(bus);
            bus.write(nn, idx as u8);
            bus.write(nn.wrapping_add(1), (idx >> 8) as u8);
            cpu.wz = nn.wrapping_add(1);
            cpu.q = 0;
            20
        }
        0x2A => {
            let nn = cpu.imm16(bus);
            let lo = bus.read(nn);
            let hi = bus.read(nn.wrapping_add(1));
            set_index(cpu, use_iy, u16::from(hi) << 8 | u16::from(lo));
            cpu.wz = nn.wrapping_add(1);
            cpu.q = 0;
            20
        }
        0x23 => {
            set_index(cpu, use_iy, idx.wrapping_add(1));
            cpu.q = 0;
            10
        }
        0x2B => {
            set_index(cpu, use_iy, idx.wrapping_sub(1));
            cpu.q = 0;
            10
        }
        0xE1 => {
            let v = load::pop(cpu, bus);
            set_index(cpu, use_iy, v);
            cpu.q = 0;
            14
        }
        0xE5 => {
            load::push(cpu, bus, idx);
            cpu.q = 0;
            15
        }
        0xE3 => {
            let v = load::pop(cpu, bus);
            load::push(cpu, bus, idx);
            set_index(cpu, use_iy, v);
            // The *new* index -- the word that came off the stack. The one latch rule
            // on the page that reads a result rather than an operand.
            cpu.wz = v;
            cpu.q = 0;
            23
        }
        0xE9 => {
            // JP (IX). No operand is fetched, so nothing reaches the latch -- as on
            // the base page's `JP (HL)`.
            cpu.pc = idx;
            cpu.q = 0;
            8
        }
        0xF9 => {
            cpu.sp = idx;
            cpu.q = 0;
            10
        }
        // Everything else: the prefix cost 4 T-states and changed nothing. A real,
        // tested path -- `dd_00` exists, and so does `dd_76`, which is HALT after a
        // wasted prefix -- not an error case.
        _ => 4 + execute(cpu, bus, op),
    }
}

/// The `DD CB` and `FD CB` pages: 256 opcodes each, all of them on displaced memory.
///
/// The one place the Z80's encoding stops being systematic:
///
/// - **The displacement precedes the opcode** — prefix, `CB`, displacement, opcode —
///   so the operand's address is known before the operation is. Every other
///   multi-byte instruction on the chip reads its opcode first.
/// - **The register field does not select the operand.** All eight encodings act on
///   `(idx+d)`; seven of them *also* copy the result into the named register, and
///   fields 4 and 5 copy into plain `H` and `L` rather than the index halves. Those
///   seven are undocumented and upstream ships a file for each, which is why this
///   page is 256 forms rather than the 32 Zilog lists.
/// - **`BIT` produces no value**, so its eight encodings are genuinely identical,
///   none writes a register, and it costs 20 rather than 23 — there is no write
///   cycle.
/// - `R` advances **twice** across the four bytes. Only the prefix and the `CB` are
///   M1 cycles.
///
/// Measured over all 512 files: 23 T-states everywhere but `BIT`'s 20, `R` advancing
/// 2 and `PC` 4 on every case, and the latch holding `idx+d` on every case of every
/// quarter — this page has no stem that leaves `wz` alone, unlike the plain `CB` page
/// where none writes it.
pub fn index_cb_page<B: Bus>(cpu: &mut Z80, bus: &mut B, use_iy: bool) -> u32 {
    let d = cpu.disp(bus);
    // `imm`, not `fetch`: the final byte is not an M1 cycle and must not bump `R`.
    // Two M1 cycles across four bytes, and the suite compares `R`.
    let op = cpu.imm(bus);

    let idx = if use_iy { cpu.iy } else { cpu.ix };
    let addr = idx.wrapping_add(d as u16);
    cpu.wz = addr;

    let v = bus.read(addr);
    let field = op & 7;
    let bit = (op >> 3) & 7;

    match op >> 6 {
        // The rotates and shifts, `SLL` included.
        0 => {
            let r = bits::rot(cpu, bits::RotOp::from_index(bit), v);
            bus.write(addr, r);
            copy_to_field(cpu, field, r);
            23
        }
        // BIT: no result, no write, no register copy, and 20 T-states.
        1 => {
            // F3/F5 from the address latch's *high* byte, so F3 is address bit 11 and
            // F5 is bit 13. The low byte is wrong on 740-762 of 1,000 cases per file,
            // and the tested byte on 743-764 — the two rules a core would try first.
            bits::bit_test(cpu, bit, v, (cpu.wz >> 8) as u8);
            20
        }
        // RES and SET.
        _ => {
            let r = if op >> 6 == 2 {
                v & !(1 << bit)
            } else {
                v | (1 << bit)
            };
            bus.write(addr, r);
            copy_to_field(cpu, field, r);
            cpu.q = 0;
            23
        }
    }
}

/// Copies a double-prefix result into the register the field names, if any.
///
/// Field 6 names `(HL)` in every other use of this encoding, and here it means
/// "memory only" — the documented form. The other seven are the undocumented copies.
///
/// Deliberately **not** [`set_reg`]: that function's index 6 writes through `HL`,
/// which would issue a second, stray memory write at an address this instruction
/// never computed. Fields 4 and 5 are plain `H` and `L` for the same reason
/// `LD H,(IX+d)` loads plain `H` — a displaced operand suppresses the index
/// rewrite on the other side.
fn copy_to_field(cpu: &mut Z80, field: u8, v: u8) {
    match field {
        0 => cpu.b = v,
        1 => cpu.c = v,
        2 => cpu.d = v,
        3 => cpu.e = v,
        4 => cpu.h = v,
        5 => cpu.l = v,
        6 => {} // memory only: the documented form
        _ => cpu.a = v,
    }
}

/// Writes `v` to `IX` or `IY`.
///
/// A function rather than a `&mut u16` taken at the top of [`index_page`]: the
/// borrow would conflict with every `cpu` use in between, and threading the choice
/// through as a bool keeps the two prefixes provably one code path.
fn set_index(cpu: &mut Z80, use_iy: bool, v: u16) {
    if use_iy {
        cpu.iy = v;
    } else {
        cpu.ix = v;
    }
}

/// The repeat decision the four block-I/O families share.
///
/// All four repeat on `B != 0` — the transfer and compare forms use `BC` — and all
/// four need `io::block_io_repeat_adjust` before the rewind, which the other two
/// families do not. Kept as one function so the `IN` and `OUT` arms cannot come to
/// disagree about the order of the two.
fn block_io_tail(cpu: &mut Z80, block: Block, v: u8) -> u32 {
    if block.repeating && cpu.b != 0 {
        io::block_io_repeat_adjust(cpu, v);
        crate::ops::repeat(cpu)
    } else {
        16
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

    /// `LDI` moves a byte, advances both pointers, and puts `BC != 0` in P/V.
    ///
    /// The P/V rule is a repeat count, not a parity and not an overflow. Every other
    /// instruction on the chip that writes P/V writes one of those two, so this arm is
    /// the only place the third meaning appears — and the second `step` below is what
    /// distinguishes "count remaining" from "always set".
    #[test]
    fn ldi_moves_a_byte_and_puts_the_remaining_count_in_parity() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x2000);
        c.set_de(0x3000);
        c.set_bc(2);
        let mut m = Mem::at(0x100, &[0xED, 0xA0]);
        m.ram[0x2000] = 0x5A;
        assert_eq!(c.step(&mut m), 16);
        assert_eq!(m.ram[0x3000], 0x5A);
        assert_eq!(c.hl(), 0x2001);
        assert_eq!(c.de(), 0x3001);
        assert_eq!(c.bc(), 1);
        assert_eq!(c.f & PV, PV, "BC is 1, so P/V is set");
        assert_eq!(c.f & N, 0, "N and H are cleared");
        assert_eq!(c.f & H, 0);
        assert_eq!(c.pc, 0x102, "LDI does not repeat");

        // One more, and BC reaches zero.
        c.pc = 0x100;
        c.step(&mut m);
        assert_eq!(c.bc(), 0);
        assert_eq!(c.f & PV, 0, "BC is 0, so P/V is clear");
    }

    /// `LDI`'s F3/F5 come from `A + the byte moved`, bit 3 and bit **1**.
    ///
    /// Bit 1 into F5, not bit 5. The undocumented bits are not copies here — they are
    /// two bits of a sum that appears in no register — and this is the detail the
    /// `ed_a0` file exists to catch.
    #[test]
    fn ldi_derives_the_undocumented_flags_from_a_plus_the_moved_byte() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x2000);
        c.set_de(0x3000);
        c.set_bc(1);
        c.a = 0x00;
        let mut m = Mem::at(0x100, &[0xED, 0xA0]);
        // 0x00 + 0x0A = 0x0A: bit 3 set, bit 1 set.
        m.ram[0x2000] = 0x0A;
        c.step(&mut m);
        assert_eq!(c.f & F3, F3, "bit 3 of the sum goes to F3");
        assert_eq!(c.f & F5, F5, "and bit 1 of the sum goes to F5");

        // 0x00 + 0x20 = 0x20: bit 5 is set and bits 3 and 1 are not, so a core that
        // copied bit 5 into F5 -- as every other instruction does -- passes the case
        // above and fails this one.
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x2000);
        c.set_de(0x3000);
        c.set_bc(1);
        c.a = 0x00;
        m.ram[0x2000] = 0x20;
        c.step(&mut m);
        assert_eq!(c.f & (F3 | F5), 0, "bit 5 of the sum reaches no flag");

        // And the sum is `A + v`, not `v`: the same byte with a different `A` gives
        // different flags.
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x2000);
        c.set_de(0x3000);
        c.set_bc(1);
        c.a = 0x0A;
        m.ram[0x2000] = 0x00;
        c.step(&mut m);
        assert_eq!(c.f & (F3 | F5), F3 | F5, "A is part of the sum");
    }

    /// `LDIR` re-executes by rewinding `PC`, and costs 21 while it repeats.
    ///
    /// Not a loop: each iteration is one `step`, so an interrupt can land between them
    /// as it can on the real chip, and each is one vector case. A handler that looped
    /// internally would move 65,536 bytes in one uninterruptible `step`.
    #[test]
    fn ldir_rewinds_pc_to_repeat_rather_than_looping_internally() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x2000);
        c.set_de(0x3000);
        c.set_bc(2);
        let mut m = Mem::at(0x100, &[0xED, 0xB0]);
        m.ram[0x2000] = 0x11;
        m.ram[0x2001] = 0x22;
        assert_eq!(c.step(&mut m), 21, "repeating costs 21");
        assert_eq!(c.pc, 0x100, "and PC is back on the ED, ready to re-fetch");
        assert_eq!(c.bc(), 1);
        assert_eq!(m.ram[0x3000], 0x11, "one byte moved, not both");
        assert_eq!(m.ram[0x3001], 0x00);
        assert_eq!(c.step(&mut m), 16, "the last iteration costs 16");
        assert_eq!(c.pc, 0x102, "and PC finally moves past the instruction");
        assert_eq!(c.bc(), 0);
        assert_eq!(m.ram[0x3001], 0x22);
    }

    /// While repeating, F3 and F5 come from the high byte of the rewound `PC`.
    ///
    /// The one rule the plan does not mention and the hardest on the page. Placing the
    /// instruction at 0x2800 gives a `PC` high byte of 0x28 — both bits set — while the
    /// byte moved and `A` are chosen so the ordinary `A + v` rule would set neither.
    /// The final iteration, at the same address, must go back to the sum rule, which
    /// is what separates "from `PC`" from "always both".
    #[test]
    fn a_repeating_transfer_takes_the_undocumented_flags_from_the_program_counter() {
        let mut c = Z80::new();
        c.pc = 0x2800;
        c.a = 0x00;
        c.set_hl(0x2000);
        c.set_de(0x3000);
        c.set_bc(2);
        let mut m = Mem::at(0x2800, &[0xED, 0xB0]);
        // 0x00 + 0x04: bits 3 and 1 both clear, so the non-repeating rule gives 0.
        m.ram[0x2000] = 0x04;
        m.ram[0x2001] = 0x04;
        c.step(&mut m);
        assert_eq!(c.pc, 0x2800);
        assert_eq!(
            c.f & (F5 | F3),
            F5 | F3,
            "0x28's bits 5 and 3, from PC -- not the sum's bits 3 and 1"
        );
        assert_eq!(c.wz, 0x2801, "and the latch is PC + 1 while repeating");

        // The last iteration is not repeating, so the sum rule applies again.
        c.step(&mut m);
        assert_eq!(c.pc, 0x2802);
        assert_eq!(c.f & (F5 | F3), 0, "back to the sum, which has neither bit");
    }

    /// `CPI` compares without writing `A`, and its carry is untouched.
    #[test]
    fn cpi_compares_and_leaves_carry_alone() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x5A;
        c.set_hl(0x2000);
        c.set_bc(1);
        c.f = C;
        let mut m = Mem::at(0x100, &[0xED, 0xA1]);
        m.ram[0x2000] = 0x5A;
        assert_eq!(c.step(&mut m), 16);
        assert_eq!(c.a, 0x5A, "A is not written");
        assert_eq!(c.f & Z, Z, "the bytes matched");
        assert_eq!(c.f & N, N, "CPI is a subtraction");
        assert_eq!(c.f & C, C, "and carry is preserved, unlike CP");
        assert_eq!(c.hl(), 0x2001);
        assert_eq!(c.bc(), 0);
        assert_eq!(c.f & PV, 0, "BC reached zero");
    }

    /// `CPIR` stops on a match as well as on a count of zero.
    ///
    /// The exit condition `LDIR` has no equivalent of: `BC` is still 1 after the match,
    /// so a core that only checked the count would repeat. Both cases are run from the
    /// same setup, differing only in the byte at `(HL)`.
    #[test]
    fn cpir_stops_early_on_a_match_where_ldir_would_continue() {
        // A match with the count still non-zero: stop anyway.
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x5A;
        c.set_hl(0x2000);
        c.set_bc(2);
        let mut m = Mem::at(0x100, &[0xED, 0xB1]);
        m.ram[0x2000] = 0x5A;
        assert_eq!(c.step(&mut m), 16, "a match ends the search");
        assert_eq!(c.pc, 0x102, "so PC moves past the instruction");
        assert_eq!(c.bc(), 1, "with the count still non-zero");
        assert_eq!(c.f & Z, Z);

        // No match, same count: repeat.
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x5A;
        c.set_hl(0x2000);
        c.set_bc(2);
        m.ram[0x2000] = 0x99;
        assert_eq!(c.step(&mut m), 21);
        assert_eq!(c.pc, 0x100, "no match and a count left: search on");
    }

    /// The four undefined-`IM` encodings set the mode their documented twins do.
    #[test]
    fn the_im_instructions_set_the_interrupt_mode() {
        // Every encoding, documented and duplicate, because each has its own vector
        // file and a decoder that handled only 0x46/0x56/0x5E would fall through to
        // the NOP arm on five of the eight.
        for (op, mode) in [
            (0x46u8, 0u8),
            (0x4E, 0),
            (0x66, 0),
            (0x6E, 0),
            (0x56, 1),
            (0x76, 1),
            (0x5E, 2),
            (0x7E, 2),
        ] {
            let mut c = Z80::new();
            c.pc = 0x100;
            c.im = 3; // an impossible value, so a no-op would show
            c.wz = SENTINEL;
            let mut m = Mem::at(0x100, &[0xED, op]);
            assert_eq!(c.step(&mut m), 8, "ED {op:#04X} is 8 T-states");
            assert_eq!(c.im, mode, "ED {op:#04X}");
            assert_eq!(c.wz, SENTINEL, "and IM leaves the latch alone");
        }
    }

    /// `IN r,(C)` sets flags; `IN A,(n)` does not.
    ///
    /// Same mnemonic family, different pages, different behaviour. A core that shared
    /// one implementation would fail one of the two and look correct on the other.
    #[test]
    fn in_from_c_sets_flags_but_in_a_from_n_does_not() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.b = 0x12;
        c.c = 0x34;
        c.f = 0xFF;
        let mut m = Mem::at(0x100, &[0xED, 0x40]); // IN B,(C)
        m.port_in_value = 0x00;
        assert_eq!(c.step(&mut m), 12);
        assert_eq!(c.b, 0x00);
        assert_eq!(c.f & Z, Z, "IN r,(C) flags the byte it read");
        assert_eq!(c.f & PV, PV, "with parity");
        assert_eq!(c.f & (H | N), 0);

        // IN A,(n) on the base page: flags untouched.
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x12;
        c.f = 0x5A;
        let mut m = Mem::at(0x100, &[0xDB, 0x34]);
        m.port_in_value = 0x00;
        c.step(&mut m);
        assert_eq!(c.a, 0x00);
        assert_eq!(c.f, 0x5A, "and this form writes no flags at all");
    }

    /// `ED 70` is `IN (C)`: it flags the byte and writes it nowhere.
    ///
    /// Register slot 6 means `(HL)` everywhere else in this encoding, so the natural
    /// mistake is a memory write. The vectors show none on any of `ed_70`'s 1,000
    /// cases, and no register changing either.
    #[test]
    fn in_with_no_destination_flags_the_byte_and_stores_it_nowhere() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.b = 0x20;
        c.c = 0x00; // HL is 0x2000 too, so a stray (HL) write would land in RAM
        c.set_hl(0x2000);
        c.f = 0xFF;
        let mut m = Mem::at(0x100, &[0xED, 0x70]);
        m.port_in_value = 0x80;
        assert_eq!(c.step(&mut m), 12);
        assert_eq!(m.writes, vec![], "no memory write");
        assert_eq!(c.hl(), 0x2000, "and no register written");
        assert_eq!(c.f & S, S, "but the byte is still flagged");
        assert_eq!(c.f & Z, 0);
    }

    /// `ED 71` is `OUT (C),0`: it writes the byte zero on an NMOS Z80.
    ///
    /// The zero is a hardware-revision fact, not a decode convention — a CMOS part
    /// writes 0xFF — and the suite is NMOS on all 1,000 cases. A core reusing slot 6's
    /// `(HL)` meaning would write whatever RAM held.
    #[test]
    fn out_with_no_source_writes_zero_on_an_nmos_part() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.b = 0x12;
        c.c = 0x34;
        c.set_hl(0x2000);
        let mut m = Mem::at(0x100, &[0xED, 0x71]);
        m.ram[0x2000] = 0x99; // what an `(HL)` reading would send instead
        assert_eq!(c.step(&mut m), 12);
        assert_eq!(m.ports_out, vec![(0x1234, 0x00)], "zero, not (HL)");
    }

    /// `IN r,(C)` and `OUT (C),r` use `BC` as the port, `B` on the high half.
    #[test]
    fn the_c_port_forms_use_bc_as_a_sixteen_bit_address() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.b = 0x12;
        c.c = 0x34;
        c.d = 0x99;
        let mut m = Mem::at(0x100, &[0xED, 0x51]); // OUT (C),D
        assert_eq!(c.step(&mut m), 12);
        assert_eq!(m.ports_out, vec![(0x1234, 0x99)], "BC, not just C");
    }

    /// `RLD` rotates a nibble through `A` and `(HL)`; `RRD` is its mirror.
    ///
    /// Hand-computed: `A = 0x7A`, `(HL) = 0x31` gives `A = 0x73`, `(HL) = 0x1A` for
    /// `RLD`, and `A = 0x84`, `(HL) = 0x20` gives `A = 0x80`, `(HL) = 0x42` for `RRD`.
    /// The two opcodes are adjacent and transposable, so both are decoded here.
    #[test]
    fn the_nibble_rotations_are_decoded_the_right_way_round() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x7A;
        c.set_hl(0x2000);
        let mut m = Mem::at(0x100, &[0xED, 0x6F]);
        m.ram[0x2000] = 0x31;
        assert_eq!(c.step(&mut m), 18);
        assert_eq!(c.a, 0x73, "ED 6F is RLD");
        assert_eq!(m.ram[0x2000], 0x1A);
        assert_eq!(c.f & (H | N), 0);
        assert_eq!(c.wz, 0x2001, "and it latches HL + 1");

        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0x84;
        c.set_hl(0x2000);
        let mut m = Mem::at(0x100, &[0xED, 0x67]);
        m.ram[0x2000] = 0x20;
        assert_eq!(c.step(&mut m), 18);
        assert_eq!(c.a, 0x80, "ED 67 is RRD");
        assert_eq!(m.ram[0x2000], 0x42);
    }

    /// `LD A,I` sets `P` to **1**, and copies IFF2 into P/V.
    ///
    /// The interrupt-state leak: P/V holds IFF2 so software can read the interrupt
    /// enable, which is otherwise invisible. `p` records that this instruction ran,
    /// because an interrupt arriving here corrupts the flag on real hardware — and
    /// unlike `q`, which holds a flag value, `p` is the literal 1. Measured: `p == 1`
    /// on all 1,000 cases of `ed_57`, and `p == f` on 23 of them, those being the
    /// cases where `f` happens to be 1. The flags here are 0xB4, so the two differ.
    #[test]
    fn ld_a_i_copies_iff2_into_parity_and_sets_p_to_one() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.i = 0xA0;
        c.iff2 = true;
        c.f = C;
        c.wz = SENTINEL;
        let mut m = Mem::at(0x100, &[0xED, 0x57]);
        assert_eq!(c.step(&mut m), 9);
        assert_eq!(c.a, 0xA0);
        assert_eq!(c.f, S | F5 | PV | C, "IFF2 in P/V, and carry preserved");
        assert_eq!(c.p, 1, "P is the literal 1");
        assert_ne!(c.p, c.f, "which is not the flag value, as Q is");
        assert_eq!(c.q, c.f, "Q still is");
        assert_eq!(c.wz, SENTINEL, "and the latch is untouched");

        c.pc = 0x100;
        c.iff2 = false;
        c.step(&mut m);
        assert_eq!(c.f & PV, 0);
    }

    /// `RETN` copies IFF2 back into IFF1 and returns to the popped address.
    ///
    /// The copy is the instruction's reason to exist, and it is invisible whenever the
    /// two flip-flops already agree — which they do in most states, so the test starts
    /// them disagreeing in both directions. IFF2 itself does not move.
    #[test]
    fn retn_copies_iff2_into_iff1_in_both_directions() {
        for (iff1, iff2) in [(false, true), (true, false)] {
            let mut c = Z80::new();
            c.pc = 0x100;
            c.sp = 0x8000;
            c.iff1 = iff1;
            c.iff2 = iff2;
            c.f = 0x5A;
            let mut m = Mem::at(0x100, &[0xED, 0x45]);
            m.ram[0x8000] = 0x34;
            m.ram[0x8001] = 0x12;
            assert_eq!(c.step(&mut m), 14);
            assert_eq!(c.pc, 0x1234, "the popped address");
            assert_eq!(c.sp, 0x8002);
            assert_eq!(c.iff1, iff2, "IFF1 takes IFF2's value");
            assert_eq!(c.iff2, iff2, "and IFF2 does not move");
            assert_eq!(c.f, 0x5A, "RETN writes no flags");
            assert_eq!(c.wz, 0x1234, "the latch takes the popped address");
        }
    }

    /// All eight `RETN`/`RETI` encodings return; five are undocumented duplicates.
    #[test]
    fn every_retn_encoding_returns() {
        for op in [0x45u8, 0x4D, 0x55, 0x5D, 0x65, 0x6D, 0x75, 0x7D] {
            let mut c = Z80::new();
            c.pc = 0x100;
            c.sp = 0x8000;
            let mut m = Mem::at(0x100, &[0xED, op]);
            m.ram[0x8000] = 0x34;
            m.ram[0x8001] = 0x12;
            assert_eq!(c.step(&mut m), 14, "ED {op:#04X}");
            assert_eq!(c.pc, 0x1234, "ED {op:#04X} must return");
        }
    }

    /// `ADC HL,rr` and `SBC HL,rr` latch the **old** `HL` plus one, not the result.
    ///
    /// The same rule as `ADD HL,rr` on the base page — the addend's path through the
    /// ALU is what the latch follows — and the two are distinguishable here because
    /// the sum differs from the operand.
    #[test]
    fn the_sixteen_bit_adc_and_sbc_latch_the_old_hl() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x2000);
        c.set_de(0x0800);
        c.f = 0; // a reset Z80 has every flag set, carry included
        c.wz = SENTINEL;
        let mut m = Mem::at(0x100, &[0xED, 0x5A]); // ADC HL,DE
        assert_eq!(c.step(&mut m), 15);
        assert_eq!(c.hl(), 0x2800);
        assert_eq!(c.wz, 0x2001, "the old HL plus one, not 0x2801");

        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_hl(0x2000);
        c.set_de(0x0800);
        c.f = 0;
        let mut m = Mem::at(0x100, &[0xED, 0x52]); // SBC HL,DE
        assert_eq!(c.step(&mut m), 15);
        assert_eq!(c.hl(), 0x1800);
        assert_eq!(c.f & N, N, "SBC sets N where ADC clears it");
        assert_eq!(c.wz, 0x2001);
    }

    /// `LD (nn),rr` and `LD rr,(nn)` both latch `nn + 1`.
    ///
    /// One rule for both directions, unlike the base page's single-byte forms where a
    /// write puts the byte on the latch's high half. The written low byte here is 0x78
    /// while `nn`'s high byte is 0x20, so the two rules give different answers.
    #[test]
    fn the_sixteen_bit_absolute_loads_latch_nn_plus_one_in_both_directions() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.set_de(0x5678);
        let mut m = Mem::at(0x100, &[0xED, 0x53, 0x00, 0x20]); // LD (0x2000),DE
        assert_eq!(c.step(&mut m), 20);
        assert_eq!(m.ram[0x2000], 0x78, "low byte first");
        assert_eq!(m.ram[0x2001], 0x56);
        assert_eq!(
            c.wz, 0x2001,
            "nn + 1, not the written byte in the high half"
        );
        assert_eq!(c.q, 0, "and no flags");

        let mut c = Z80::new();
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xED, 0x5B, 0x00, 0x20]); // LD DE,(0x2000)
        m.ram[0x2000] = 0x78;
        m.ram[0x2001] = 0x56;
        assert_eq!(c.step(&mut m), 20);
        assert_eq!(c.de(), 0x5678);
        assert_eq!(c.wz, 0x2001);
    }

    /// `LD R,A` writes all eight bits, bit 7 included.
    ///
    /// `bump_r` holds bit 7 across a fetch, so a `LD R,A` that masked to seven bits
    /// would be indistinguishable on any value under 0x80. It is also the one `ED`
    /// stem whose `R` delta is not 2.
    #[test]
    fn ld_r_from_a_writes_all_eight_bits() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.a = 0xFF;
        c.r = 0x00;
        let mut m = Mem::at(0x100, &[0xED, 0x4F]);
        assert_eq!(c.step(&mut m), 9);
        assert_eq!(
            c.r, 0xFF,
            "bit 7 included, and no fetch bump after the write"
        );

        // And `LD A,R` reads it back including bit 7 -- after the two M1 bumps this
        // instruction's own fetches caused.
        let mut c = Z80::new();
        c.pc = 0x100;
        c.r = 0x80;
        let mut m = Mem::at(0x100, &[0xED, 0x5F]);
        c.step(&mut m);
        assert_eq!(c.a, 0x82, "0x80 with two M1 bumps in the low seven bits");
        assert_eq!(c.f & S, S, "and the flags describe what was read");
    }

    /// `NEG` on all eight encodings, and P/V only at 0x80.
    #[test]
    fn every_neg_encoding_negates_and_overflows_only_at_the_sign_bit() {
        for op in [0x44u8, 0x4C, 0x54, 0x5C, 0x64, 0x6C, 0x74, 0x7C] {
            let mut c = Z80::new();
            c.pc = 0x100;
            c.a = 0x01;
            c.wz = SENTINEL;
            let mut m = Mem::at(0x100, &[0xED, op]);
            assert_eq!(c.step(&mut m), 8, "ED {op:#04X}");
            assert_eq!(c.a, 0xFF, "ED {op:#04X} must negate");
            assert_eq!(c.f & (N | C), N | C, "ED {op:#04X}");
            assert_eq!(c.f & PV, 0, "ED {op:#04X}: 1 does not overflow");
            assert_eq!(c.wz, SENTINEL, "ED {op:#04X} leaves the latch alone");
        }
    }

    /// `INI` writes to `(HL)` and latches the port address it used, stepped.
    ///
    /// Three claims in one instruction that a core can get individually wrong: the port
    /// address carries the *un*decremented `B`, the memory write lands on the *old*
    /// `HL`, and the latch takes that same pre-decrement `BC` stepped by the direction.
    /// Values are chosen so all three differ.
    #[test]
    fn ini_reads_the_old_port_writes_the_old_address_and_latches_the_bus_address() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.b = 0x12;
        c.c = 0x34;
        c.set_hl(0x2000);
        let mut m = Mem::at(0x100, &[0xED, 0xA2]);
        m.port_in_value = 0x5A;
        assert_eq!(c.step(&mut m), 16);
        assert_eq!(m.ports_in, vec![0x1234], "the port before the decrement");
        assert_eq!(m.ram[0x2000], 0x5A, "written at the old HL");
        assert_eq!(c.b, 0x11);
        assert_eq!(c.hl(), 0x2001);
        assert_eq!(
            c.wz, 0x1235,
            "the old BC, plus one -- the address on the bus"
        );
    }

    /// `OUTI` writes the port at `BC - 0x100`: `B` is decremented first.
    ///
    /// The exact opposite ordering to `INI`, which reads its port before decrementing.
    /// Both orderings produce the same final register state, so the port *address* is
    /// the only thing that can tell them apart.
    #[test]
    fn outi_writes_its_port_with_the_decremented_b() {
        let mut c = Z80::new();
        c.pc = 0x100;
        c.b = 0x12;
        c.c = 0x34;
        c.set_hl(0x2000);
        let mut m = Mem::at(0x100, &[0xED, 0xA3]);
        m.ram[0x2000] = 0x5A;
        assert_eq!(c.step(&mut m), 16);
        assert_eq!(
            m.ports_out,
            vec![(0x1134, 0x5A)],
            "BC - 0x100, where INI would have used 0x1234"
        );
        assert_eq!(c.hl(), 0x2001);
    }

    /// The four repeating block-I/O forms repeat on `B`, not on `BC`.
    ///
    /// `C` is left non-zero and `B` reaches zero, so a core that tested `BC` — as the
    /// transfer and compare forms correctly do — would repeat forever here. Run over
    /// all four opcodes because each has its own arm's worth of chances to use the
    /// wrong register.
    #[test]
    fn the_repeating_block_io_forms_count_down_b_alone() {
        for op in [0xB2u8, 0xBA, 0xB3, 0xBB] {
            let mut c = Z80::new();
            c.pc = 0x100;
            c.b = 0x01; // one iteration left
            c.c = 0x34; // and a non-zero C, which must not keep it going
            c.set_hl(0x2000);
            let mut m = Mem::at(0x100, &[0xED, op]);
            assert_eq!(c.step(&mut m), 16, "ED {op:#04X} does not repeat at B = 1");
            assert_eq!(c.b, 0x00);
            assert_eq!(c.pc, 0x102, "ED {op:#04X}: PC past the instruction");

            let mut c = Z80::new();
            c.pc = 0x100;
            c.b = 0x02;
            c.c = 0x34;
            c.set_hl(0x2000);
            assert_eq!(c.step(&mut m), 21, "ED {op:#04X} repeats at B = 2");
            assert_eq!(c.pc, 0x100, "ED {op:#04X}: PC back on the prefix");
        }
    }

    /// The `ED` page's undefined opcodes are two `NOP`s, and bump `R` twice.
    ///
    /// 176 of the 256 are undefined and upstream ships no file for any of them, so this
    /// is the one part of the page resting on documentation. What is testable is that
    /// they are reachable rather than a panic, and that both M1 cycles were charged.
    #[test]
    fn the_undefined_ed_opcodes_are_two_nops() {
        for op in [0x00u8, 0x3F, 0x80, 0xA4, 0xBC, 0xFF] {
            let mut c = Z80::new();
            c.pc = 0x100;
            c.r = 0x00;
            c.f = 0x5A;
            c.wz = SENTINEL;
            let mut m = Mem::at(0x100, &[0xED, op]);
            assert_eq!(c.step(&mut m), 8, "ED {op:#04X}");
            assert_eq!(c.pc, 0x102, "ED {op:#04X}");
            assert_eq!(c.r, 0x02, "ED {op:#04X}: two M1 cycles, prefix included");
            assert_eq!(c.f, 0x5A, "ED {op:#04X} writes no flags");
            assert_eq!(c.q, 0, "ED {op:#04X}");
            assert_eq!(c.wz, SENTINEL, "ED {op:#04X}");
        }
    }

    /// `DD` makes `H` and `L` the halves of `IX`.
    ///
    /// `DD 44` is `LD B,IXH`. The prefix rewrites *register* operands, not only
    /// memory ones — the part a core that treats `DD` as "add a displacement"
    /// misses entirely. `H` is loaded with a third value so that "took IXH",
    /// "took H" and "took IXL" are three distinguishable outcomes.
    #[test]
    fn a_dd_prefix_turns_h_and_l_into_the_halves_of_ix() {
        let mut c = Z80::new();
        c.ix = 0x1234;
        c.h = 0x99;
        c.l = 0x88;
        assert_eq!(run(&mut c, &[0xDD, 0x44]), 8); // LD B,IXH
        assert_eq!(c.b, 0x12, "IXH, not H and not IXL");

        let mut c = Z80::new();
        c.iy = 0x1234;
        c.l = 0x88;
        run(&mut c, &[0xFD, 0x4D]); // LD C,IYL
        assert_eq!(c.c, 0x34, "and FD reaches IY's halves");

        // Both halves at once: LD IXH,IXL leaves plain HL alone entirely.
        let mut c = Z80::new();
        c.ix = 0x1234;
        c.h = 0x99;
        c.l = 0x88;
        run(&mut c, &[0xDD, 0x65]); // LD IXH,IXL
        assert_eq!(c.ix, 0x3434, "the low half copied into the high one");
        assert_eq!((c.h, c.l), (0x99, 0x88), "and HL untouched");
    }

    /// But a displaced-memory operand suppresses the rewrite on the other side.
    ///
    /// `DD 66` is `LD H,(IX+d)` — plain `H`, not `IXH`. One instruction cannot use
    /// `IX` as both a pointer and a register half, so the memory form wins and the
    /// register stays unprefixed. A core that rewrote both would load `IXH` here and
    /// pass every non-displaced test.
    #[test]
    fn a_displaced_operand_leaves_the_other_register_unprefixed() {
        let mut c = Z80::new();
        c.ix = 0x2000;
        c.h = 0x11;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0x66, 0x05]); // LD H,(IX+5)
        m.ram[0x2005] = 0x7E;
        assert_eq!(c.step(&mut m), 19);
        assert_eq!(c.h, 0x7E, "H, not IXH");
        assert_eq!(c.ix, 0x2000, "and IX is untouched");

        // And the store direction: LD (IX+d),H writes plain H.
        let mut c = Z80::new();
        c.ix = 0x2000;
        c.h = 0x42;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0x74, 0x03]);
        assert_eq!(c.step(&mut m), 19);
        assert_eq!(m.ram[0x2003], 0x42, "H, not IXH");
    }

    /// The displacement is **signed**.
    ///
    /// `0xFB` is −5, so `(IX-5)`. An unsigned read would address `IX+251` — off by
    /// 256, which lands inside a nearby structure rather than crashing, and is
    /// therefore the kind of bug that shows up as garbled sprites. Both candidate
    /// addresses are seeded, so a wrong core reads a plausible byte rather than a
    /// zero.
    #[test]
    fn the_displacement_is_a_signed_byte() {
        let mut c = Z80::new();
        c.ix = 0x2010;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0x7E, 0xFB]); // LD A,(IX-5)
        m.ram[0x200B] = 0xAB;
        m.ram[0x210B] = 0xCD; // where an unsigned read would land
        assert_eq!(c.step(&mut m), 19);
        assert_eq!(c.a, 0xAB, "IX-5, not IX+251");
    }

    /// `EX DE,HL` under a prefix still exchanges `HL`.
    ///
    /// The exception that shows the rule is about encoding rather than mnemonics:
    /// `DD EB` reaches `HL` by a path the prefix does not intercept. A core that
    /// substituted on the mnemonic would exchange `DE` with `IX` and be wrong in a
    /// way no non-prefixed test can see.
    #[test]
    fn ex_de_hl_under_a_prefix_still_touches_hl() {
        let mut c = Z80::new();
        c.set_de(0xAAAA);
        c.set_hl(0xBBBB);
        c.ix = 0xCCCC;
        run(&mut c, &[0xDD, 0xEB]);
        assert_eq!(c.de(), 0xBBBB);
        assert_eq!(c.hl(), 0xAAAA);
        assert_eq!(c.ix, 0xCCCC, "IX is not involved");
    }

    /// A prefix on an opcode with no `HL` costs 4 extra T-states and does nothing.
    ///
    /// `DD 00` is a tested opcode with its own vector file, not an error case. So the
    /// fallthrough is a real code path and must charge the prefix's M1 — which it
    /// does by construction, since the prefix fetch already bumped `R`.
    ///
    /// `DD 76` is the same path reached the other way: `HALT` sits inside the
    /// `0x40..=0x7F` load block, so only the `op != 0x76` guard sends it here. Its
    /// 8 T-states are measured — `dd_76` shows 8 on all 1,000 cases — and without
    /// the guard it would decode as `LD (IX+d),(IX+d)` and read a displacement byte
    /// that is not there, advancing `PC` one too far.
    #[test]
    fn a_prefix_on_an_unaffected_opcode_only_costs_time() {
        let mut c = Z80::new();
        c.r = 0;
        assert_eq!(run(&mut c, &[0xDD, 0x00]), 8, "4 for the prefix, 4 for NOP");
        assert_eq!(c.pc, 0x102);
        assert_eq!(c.r, 2, "two M1 cycles, so R advanced twice");

        let mut c = Z80::new();
        c.r = 0;
        assert_eq!(run(&mut c, &[0xDD, 0x76]), 8, "HALT, after a wasted prefix");
        assert_eq!(
            c.pc, 0x102,
            "past the prefix and the opcode, and no further"
        );
        assert!(c.halted);
        assert_eq!(c.r, 2);
    }

    /// `INC (IX+d)` reads, modifies, writes — and preserves carry, as `INC` does.
    #[test]
    fn inc_of_displaced_memory_is_a_read_modify_write_at_twenty_three_tstates() {
        let mut c = Z80::new();
        c.ix = 0x2000;
        c.f = C;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0x34, 0x02]);
        m.ram[0x2002] = 0x7F;
        assert_eq!(c.step(&mut m), 23);
        assert_eq!(m.ram[0x2002], 0x80);
        assert_eq!(c.f & PV, PV, "0x7F to 0x80 overflows");
        assert_eq!(c.f & C, C, "and INC never touches carry");
    }

    /// `INC IXH` moves one half and leaves the other, and both `HL` bytes, alone.
    #[test]
    fn inc_of_an_index_half_moves_only_that_half() {
        let mut c = Z80::new();
        c.ix = 0x12FF;
        c.h = 0x77;
        c.l = 0x88;
        assert_eq!(run(&mut c, &[0xDD, 0x24]), 8); // INC IXH
        assert_eq!(c.ix, 0x13FF, "the low half does not carry into anything");
        assert_eq!((c.h, c.l), (0x77, 0x88));

        // And the low half wraps within itself rather than into the high one.
        let mut c = Z80::new();
        c.ix = 0x12FF;
        run(&mut c, &[0xDD, 0x2C]); // INC IXL
        assert_eq!(c.ix, 0x1200, "0xFF + 1 wraps to 0x00, no carry out");
    }

    /// `ADD IX,IX` doubles it, and `ADD IX,rr` uses the prefixed register.
    #[test]
    fn add_ix_uses_ix_on_both_sides_where_the_encoding_says_hl() {
        let mut c = Z80::new();
        c.ix = 0x1111;
        c.set_hl(0x9999);
        assert_eq!(run(&mut c, &[0xDD, 0x29]), 15); // ADD IX,IX
        assert_eq!(c.ix, 0x2222, "IX + IX, not IX + HL");
        assert_eq!(c.hl(), 0x9999);

        let mut c = Z80::new();
        c.ix = 0x0001;
        c.set_bc(0x0002);
        run(&mut c, &[0xDD, 0x09]); // ADD IX,BC
        assert_eq!(
            c.ix, 0x0003,
            "BC stays BC; only the destination is prefixed"
        );
    }

    /// `LD SP,IX` and `JP (IX)` and `PUSH IX`.
    ///
    /// Three instructions that read `HL` in the unprefixed encoding and must all
    /// follow the substitution. `LD SP,HL` is the one most often missed, because it
    /// is the only 16-bit register-to-register move on the chip.
    #[test]
    fn the_sixteen_bit_hl_instructions_all_substitute() {
        let mut c = Z80::new();
        c.ix = 0x1234;
        c.set_hl(0x9999);
        assert_eq!(run(&mut c, &[0xDD, 0xF9]), 10); // LD SP,IX
        assert_eq!(c.sp, 0x1234);

        let mut c = Z80::new();
        c.ix = 0x4321;
        c.set_hl(0x9999);
        assert_eq!(run(&mut c, &[0xDD, 0xE9]), 8); // JP (IX)
        assert_eq!(c.pc, 0x4321);

        let mut c = Z80::new();
        c.sp = 0x3000;
        c.ix = 0xBEEF;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0xE5]); // PUSH IX
        assert_eq!(c.step(&mut m), 15);
        assert_eq!(c.sp, 0x2FFE);
        assert_eq!(m.ram[0x2FFF], 0xBE, "the high byte goes first");
        assert_eq!(m.ram[0x2FFE], 0xEF);
    }

    /// `LD IXH,n` and `LD IXL,n` write a half — the arm the plan omits.
    ///
    /// Without it these fall through to the base page and load plain `H`/`L`. Since
    /// `LD r,n` writes no flags and no latch, *which register moved* is the only
    /// visible difference, so both halves and both plain registers are seeded
    /// distinctly. The cost is 11, not the base page's 7: `dd_26` shows 11 on all
    /// 1,000 cases.
    #[test]
    fn a_prefixed_immediate_load_reaches_an_index_half() {
        let mut c = Z80::new();
        c.ix = 0x1234;
        c.h = 0x77;
        c.l = 0x88;
        assert_eq!(run(&mut c, &[0xDD, 0x26, 0xAB]), 11); // LD IXH,0xAB
        assert_eq!(c.ix, 0xAB34, "the high half only");
        assert_eq!((c.h, c.l), (0x77, 0x88), "HL untouched");

        let mut c = Z80::new();
        c.iy = 0x1234;
        c.l = 0x88;
        assert_eq!(run(&mut c, &[0xFD, 0x2E, 0xCD]), 11); // LD IYL,0xCD
        assert_eq!(c.iy, 0x12CD);
        assert_eq!(c.l, 0x88);
    }

    /// A prefix followed by another prefix: the second one wins.
    ///
    /// `DD FD 21 nn nn` loads `IY`, not `IX`. Each prefix restarts the rule, and the
    /// natural implementation — re-dispatching on the prefix — gets this for free;
    /// a match arm that assumed the next byte is an opcode does not.
    #[test]
    fn the_last_prefix_before_the_opcode_is_the_one_that_applies() {
        let mut c = Z80::new();
        c.r = 0;
        let t = run(&mut c, &[0xDD, 0xFD, 0x21, 0x34, 0x12]);
        assert_eq!(c.iy, 0x1234, "the FD won");
        assert_eq!(c.ix, 0x0000, "and the DD did nothing but cost time");
        assert_eq!(t, 18, "4 for the discarded DD, then a 14-T-state LD IY,nn");
        assert_eq!(c.r, 3, "three M1 cycles: DD, FD, 21");
    }

    /// Every displaced form latches the address it computed — the stores included.
    ///
    /// The stores are the trap. `LD (IX+d),r` writes one byte to memory, and every
    /// other single-byte write on the chip puts that byte into the latch's high half
    /// ([`wz_after_write`]); these put the address there instead. The written byte
    /// and the address's high byte are chosen to differ, so the two rules give
    /// different answers — with a matching pair they are indistinguishable, which is
    /// how a core ships this bug.
    #[test]
    fn a_displaced_store_latches_its_address_not_the_byte_it_wrote() {
        let mut c = Z80::new();
        c.ix = 0x2040;
        c.b = 0x99; // differs from 0x20, so the two candidate rules disagree
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0x70, 0x02]); // LD (IX+2),B
        assert_eq!(c.step(&mut m), 19);
        assert_eq!(m.ram[0x2042], 0x99);
        assert_eq!(c.wz, 0x2042, "the address, not 0x9943");

        // The load direction, the immediate form, and the read-modify-write form.
        let mut c = Z80::new();
        c.ix = 0x2040;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0x7E, 0x02]); // LD A,(IX+2)
        c.step(&mut m);
        assert_eq!(c.wz, 0x2042);

        let mut c = Z80::new();
        c.ix = 0x2040;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0x36, 0x02, 0x99]); // LD (IX+2),0x99
        c.step(&mut m);
        assert_eq!(c.wz, 0x2042, "the address, not 0x9943");

        let mut c = Z80::new();
        c.ix = 0x2040;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0x86, 0x02]); // ADD A,(IX+2)
        c.step(&mut m);
        assert_eq!(c.wz, 0x2042);

        let mut c = Z80::new();
        c.ix = 0x2040;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0x35, 0x02]); // DEC (IX+2)
        c.step(&mut m);
        assert_eq!(c.wz, 0x2042);
    }

    /// The three 16-bit latch rules, each of which reads a different thing.
    ///
    /// `ADD IX,rr` takes the **old** index plus one, so the sum must differ from the
    /// operand. `EX (SP),IX` takes the **new** index — the only rule on the page that
    /// reads a result rather than an operand — so the two indices must differ.
    /// `LD (nn),IX` takes `nn + 1` rather than the base page's byte-in-the-high-half
    /// rule, so the stored bytes must differ from `nn`'s.
    #[test]
    fn the_sixteen_bit_index_forms_each_latch_a_different_operand() {
        let mut c = Z80::new();
        c.ix = 0x2800;
        c.set_bc(0x0100);
        c.f = 0;
        assert_eq!(run(&mut c, &[0xDD, 0x09]), 15); // ADD IX,BC
        assert_eq!(c.ix, 0x2900);
        assert_eq!(c.wz, 0x2801, "the old IX plus one, not the sum plus one");

        let mut c = Z80::new();
        c.sp = 0x3000;
        c.ix = 0x1111;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0xE3]); // EX (SP),IX
        m.ram[0x3000] = 0x22;
        m.ram[0x3001] = 0x44;
        assert_eq!(c.step(&mut m), 23);
        assert_eq!(c.ix, 0x4422);
        assert_eq!(m.ram[0x3001], 0x11, "the old high byte goes back first");
        assert_eq!(m.ram[0x3000], 0x11);
        assert_eq!(c.wz, 0x4422, "the new IX, not the old one");

        let mut c = Z80::new();
        c.ix = 0x9988; // both bytes differ from 0x30 and 0x00
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0x22, 0x00, 0x30]); // LD (0x3000),IX
        assert_eq!(c.step(&mut m), 20);
        assert_eq!((m.ram[0x3000], m.ram[0x3001]), (0x88, 0x99));
        assert_eq!(c.wz, 0x3001, "nn + 1: not nn + 2, and not 0x9901");

        let mut c = Z80::new();
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0x2A, 0x00, 0x30]); // LD IX,(0x3000)
        m.ram[0x3000] = 0x88;
        m.ram[0x3001] = 0x99;
        assert_eq!(c.step(&mut m), 20);
        assert_eq!(c.ix, 0x9988);
        assert_eq!(c.wz, 0x3001);
    }

    /// The 171 stems that leave the latch alone, spot-checked across its shapes.
    ///
    /// `wz` persists between instructions — `BIT b,(HL)` reads it — so an arm that
    /// writes it spuriously is a bug that surfaces two instructions later. The
    /// sentinel makes "left alone" a positive assertion rather than the absence of
    /// one, and these five are the forms most likely to write it by analogy with a
    /// neighbour that does.
    #[test]
    fn the_register_only_index_forms_write_no_latch() {
        for (prog, what) in [
            (&[0xDD, 0x44u8] as &[u8], "LD B,IXH"),
            (&[0xDD, 0x21, 0x34, 0x12], "LD IX,nn"),
            (&[0xDD, 0x23], "INC IX"),
            (&[0xDD, 0x24], "INC IXH"),
            (&[0xDD, 0xE9], "JP (IX)"),
            (&[0xDD, 0xF9], "LD SP,IX"),
            (&[0xDD, 0xE5], "PUSH IX"),
            (&[0xDD, 0x00], "NOP after a prefix"),
        ] {
            let mut c = Z80::new();
            c.sp = 0x3000;
            c.ix = 0x2040;
            c.wz = SENTINEL;
            run(&mut c, prog);
            assert_eq!(c.wz, SENTINEL, "{what} writes no latch");
        }
    }

    /// `Q` after an index instruction: the flag writers set it, the movers clear it.
    ///
    /// Measured over all 252 stems of both prefixes: `q == f` on exactly the 100
    /// flag-writing stems and `q == 0` on the other 152, with no stem mixed. A core
    /// that set `q = 1` on the writers — the shape the plan's text suggests — would
    /// agree with the suite only where `f` happens to be 1, and `Q` is what the next
    /// `SCF`/`CCF` reads, so the error surfaces on a later instruction.
    #[test]
    fn q_holds_the_flags_an_index_instruction_wrote_or_zero() {
        // A flag writer: ADD IX,BC writes C, H, F3 and F5.
        let mut c = Z80::new();
        c.ix = 0x28FF;
        c.set_bc(0x0001);
        c.f = 0;
        run(&mut c, &[0xDD, 0x09]);
        assert_ne!(c.f, 0, "ADD IX,rr wrote flags");
        assert_eq!(c.q, c.f, "so Q holds them -- a value, not a 1");

        // And a mover: LD B,IXH writes none.
        let mut c = Z80::new();
        c.ix = 0x1234;
        c.f = 0x5A;
        run(&mut c, &[0xDD, 0x44]);
        assert_eq!(c.f, 0x5A, "no flags written");
        assert_eq!(c.q, 0);
    }

    /// A prefix resets `Q`, which changes what the `SCF` behind it computes.
    ///
    /// The only place on the page where the prefix alters a *result* rather than
    /// only a cost. `SCF` takes F3/F5 from `A | (f & !q)` — see [`scf_ccf`] — so a
    /// prefix that left `Q` alone would suppress the `f` term. The setup makes the
    /// two answers differ: `f`'s F5 and F3 are set, `A`'s are clear, and `q == f`,
    /// as it would be after any flag-writing instruction.
    ///
    /// Measured: on `dd_37`, `dd_3f` and `fd_37` a carried-over `Q` is wrong on 239,
    /// 230 and 223 of 1,000 cases, and clearing it on none. The unprefixed `37` and
    /// `3f` are unaffected either way, which is why 2,000 green cases on the base
    /// page said nothing about this.
    #[test]
    fn a_prefix_resets_q_so_the_scf_behind_it_sees_no_previous_writer() {
        for (prog, what) in [
            (&[0xDD, 0x37u8] as &[u8], "SCF"),
            (&[0xDD, 0x3F], "CCF"),
            (&[0xFD, 0x37], "SCF under FD"),
        ] {
            let mut c = Z80::new();
            c.a = 0x00; // contributes neither undocumented bit
            c.f = F5 | F3;
            c.q = c.f; // as after any flag writer
            run(&mut c, prog);
            assert_eq!(
                c.f & (F5 | F3),
                F5 | F3,
                "{what}: the prefix cleared Q, so F's own bits still count"
            );
        }

        // Unprefixed, the same setup gives the opposite answer -- which is what makes
        // the assertion above a distinction rather than a restatement.
        let mut c = Z80::new();
        c.a = 0x00;
        c.f = F5 | F3;
        c.q = c.f;
        run(&mut c, &[0x37]);
        assert_eq!(c.f & (F5 | F3), 0, "no prefix, so Q still masks F");
    }

    /// The displacement precedes the opcode.
    ///
    /// prefix, `CB`, **displacement**, opcode — the operand is read before the
    /// instruction is known. Every other multi-byte instruction reads its opcode
    /// first, so this is the one place the natural decode order is wrong. Swapping
    /// the two reads would rotate the byte at `IX+6` and consume the same four
    /// bytes, so `PC` alone cannot catch it: the *address* is the witness.
    #[test]
    fn the_displacement_comes_before_the_opcode_byte() {
        let mut c = Z80::new();
        c.ix = 0x2000;
        c.f = 0;
        c.pc = 0x100;
        // DD CB 05 06 = RLC (IX+5)
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0x05, 0x06]);
        m.ram[0x2005] = 0x81;
        m.ram[0x2006] = 0x00; // where a swapped decode would work instead
        assert_eq!(c.step(&mut m), 23);
        assert_eq!(m.ram[0x2005], 0x03, "0x81 rotated left is 0x03");
        assert_eq!(m.ram[0x2006], 0x00, "and IX+6 is untouched");
        assert_eq!(c.f & C, C, "with bit 7 into carry");
        assert_eq!(c.pc, 0x104, "and all four bytes consumed");
    }

    /// The register field addresses no operand and names a second destination.
    ///
    /// `DD CB 05 00` is `RLC (IX+5)` **and** `LD B,result`. All eight encodings
    /// address the same memory; seven also write a register. Undocumented, and
    /// upstream ships all 256 files — which is why this page is 256 and not 32.
    ///
    /// Fields 4 and 5 copy to **plain** `H` and `L`, not the index halves: measured
    /// 0-wrong on `dd_cb____05`, and a core that used [`set_index`] there would
    /// corrupt the pointer it was just told to read through.
    #[test]
    fn the_register_field_names_an_extra_destination_not_the_operand() {
        let mut c = Z80::new();
        c.ix = 0x2000;
        c.b = 0xFF;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0x05, 0x00]);
        m.ram[0x2005] = 0x81;
        assert_eq!(c.step(&mut m), 23);
        assert_eq!(m.ram[0x2005], 0x03, "memory still gets the result");
        assert_eq!(c.b, 0x03, "and so does B");

        // The documented form (field 6) writes memory only -- and writes it once. A
        // `set_reg` here would take the `(HL)` path and write through HL as well.
        let mut c = Z80::new();
        c.ix = 0x2000;
        c.b = 0xFF;
        c.set_hl(0x3000);
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0x05, 0x06]);
        m.ram[0x2005] = 0x81;
        c.step(&mut m);
        assert_eq!(c.b, 0xFF, "field 6 leaves the registers alone");
        assert_eq!(m.ram[0x3000], 0x00, "and issues no stray write through HL");

        // Field 5 is plain L, and the index is not disturbed.
        let mut c = Z80::new();
        c.ix = 0x2000;
        c.l = 0xFF;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0x05, 0x05]);
        m.ram[0x2005] = 0x81;
        c.step(&mut m);
        assert_eq!(c.l, 0x03, "plain L, not IXL");
        assert_eq!(c.ix, 0x2000, "and IX is intact");
    }

    /// `SET` and `RES` copy to the register too; `BIT` has nothing to copy.
    ///
    /// `BIT` produces no value, so its eight encodings really are identical and none
    /// writes a register. A core that applied the copy uniformly would clobber a
    /// register on every `BIT (IX+d)` — and `BIT` is the one form software uses in
    /// hot loops.
    #[test]
    fn set_and_res_copy_to_the_register_but_bit_writes_nothing() {
        let mut c = Z80::new();
        c.ix = 0x2000;
        c.d = 0x00;
        c.pc = 0x100;
        // DD CB 00 C2 = SET 0,(IX+0), also into D
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0x00, 0xC2]);
        m.ram[0x2000] = 0x00;
        assert_eq!(c.step(&mut m), 23);
        assert_eq!(m.ram[0x2000], 0x01);
        assert_eq!(c.d, 0x01, "SET copies to the named register");

        let mut c = Z80::new();
        c.ix = 0x2000;
        c.d = 0x77;
        c.pc = 0x100;
        // DD CB 00 42 = BIT 0,(IX+0) -- the field is 2 (D) and must be ignored
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0x00, 0x42]);
        m.ram[0x2000] = 0x01;
        assert_eq!(c.step(&mut m), 20, "BIT is 20, not 23 -- no write cycle");
        assert_eq!(c.d, 0x77, "and D is untouched");
        assert_eq!(m.ram[0x2000], 0x01, "memory is not rewritten either");
        assert_eq!(c.f & Z, 0, "bit 0 was set");
    }

    /// `R` advances twice: `DD` and `CB` are M1 cycles, the other two bytes are not.
    ///
    /// Four bytes, two M1 cycles. A core that bumped `R` per byte would be off by
    /// two — and the suite compares `R`, so this is caught rather than mysterious.
    #[test]
    fn the_double_prefix_bumps_the_refresh_counter_exactly_twice() {
        let mut c = Z80::new();
        c.r = 0;
        c.ix = 0x2000;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0x00, 0x06]);
        c.step(&mut m);
        assert_eq!(c.r, 2, "two M1 cycles across four bytes");
    }

    /// `BIT b,(IX+d)`'s undocumented flags come from the **address**, not the byte.
    ///
    /// F3/F5 are bits 11 and 13 of `IX+d` — the internal address latch leaking onto
    /// the flag register.
    ///
    /// The two addresses are chosen to disagree so a swapped pair of bits fails
    /// rather than passing twice: `0x0800` has bit 11 set and bit 13 clear (F3 set,
    /// F5 clear), and `0x2000` is its mirror. An address like `0x2800` would be
    /// useless — it has *both* bits set, so it cannot tell the two flags apart. The
    /// tested byte is `0xFF` in both, contributing both bits, so "took them from the
    /// byte" is a third distinguishable outcome: measured wrong on 743–764 of 1,000
    /// cases per file, as is the address's low byte.
    #[test]
    fn bit_of_displaced_memory_takes_f3_and_f5_from_the_address() {
        let mut c = Z80::new();
        c.ix = 0x0800;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0x00, 0x46]);
        m.ram[0x0800] = 0xFF;
        c.step(&mut m);
        assert_eq!(c.wz, 0x0800, "the latch holds the displaced address");
        assert_eq!(c.f & F3, F3, "bit 11 of 0x0800 is set");
        assert_eq!(c.f & F5, 0, "bit 13 is clear");

        // 0x2000: bit 13 set, bit 11 clear -- the mirror case.
        let mut c = Z80::new();
        c.ix = 0x2000;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0x00, 0x46]);
        m.ram[0x2000] = 0xFF;
        c.step(&mut m);
        assert_eq!(c.f & F5, F5);
        assert_eq!(c.f & F3, 0);
    }

    /// `SLL (IX+d)` exists and shifts a 1 into bit 0.
    #[test]
    fn sll_of_displaced_memory_shifts_a_one_in() {
        let mut c = Z80::new();
        c.ix = 0x2000;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0x00, 0x36]);
        m.ram[0x2000] = 0x00;
        c.step(&mut m);
        assert_eq!(
            m.ram[0x2000], 0x01,
            "a 1, not a 0 -- that is what makes it SLL"
        );
    }

    /// The double-prefix displacement is signed, and `FD CB` reaches `IY`.
    ///
    /// Two rules that the four-byte form makes easy to lose: the displacement is
    /// read by [`Z80::disp`] and so is signed like every other one, and the index
    /// choice has to survive being threaded through two dispatch levels.
    #[test]
    fn the_double_prefix_form_is_signed_and_honours_the_second_prefix() {
        let mut c = Z80::new();
        c.iy = 0x2010;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xFD, 0xCB, 0xFB, 0xC6]); // SET 0,(IY-5)
        m.ram[0x200B] = 0x00;
        m.ram[0x210B] = 0x00; // where an unsigned read would land
        assert_eq!(c.step(&mut m), 23);
        assert_eq!(m.ram[0x200B], 0x01, "IY-5, not IY+251");
        assert_eq!(m.ram[0x210B], 0x00);
        assert_eq!(c.wz, 0x200B);

        // And DD CB with IY loaded is unaffected: the prefix, not the register, picks.
        let mut c = Z80::new();
        c.ix = 0x2000;
        c.iy = 0x4000;
        c.pc = 0x100;
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0x00, 0xC6]);
        c.step(&mut m);
        assert_eq!(m.ram[0x2000], 0x01, "DD chose IX");
        assert_eq!(m.ram[0x4000], 0x00);
    }
}
