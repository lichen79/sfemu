//! Exhaustive checks over the whole 65536-opcode space.
//!
//! The vector suite samples 2500 cases per group, so a legal-but-rare encoding
//! can be wrong or panicking without any group failing. These tests cover every
//! opcode instead of a sample. Run in debug too: overflow checks and
//! `debug_assert!`s (notably the MOVE schedule's fetch-count assertion) are the
//! point.

use m68k::decode::Decoder;
use m68k::{Bus, M68k};

/// 64 KB of RAM prefilled with `NOP`, so a fetch anywhere returns a legal word.
struct Ram(Vec<u8>);

impl Ram {
    fn new() -> Self {
        Self(vec![0x4E; 0x10000])
    }
}

impl Bus for Ram {
    fn read8(&mut self, a: u32) -> u8 {
        self.0[(a & 0xFFFF) as usize]
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.0[(a & 0xFFFF) as usize] = v;
    }
    fn read16(&mut self, a: u32) -> u16 {
        ((self.read8(a) as u16) << 8) | self.read8(a.wrapping_add(1)) as u16
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.write8(a, (v >> 8) as u8);
        self.write8(a.wrapping_add(1), v as u8);
    }
}

fn seeded(base: u32, sr: u16, op: u16) -> (M68k, Ram) {
    let mut cpu = M68k::new();
    for i in 0..8 {
        cpu.d[i] = base.wrapping_add(i as u32);
        cpu.a[i] = base.wrapping_add(i as u32);
    }
    cpu.ssp = 0x2000;
    cpu.usp = 0x3000;
    cpu.set_sr(sr);
    cpu.pc = 0x0504;
    cpu.prefetch = [op, 0x1235];
    (cpu, Ram::new())
}

/// Every opcode must execute without panicking, under seeds that hit odd and
/// even addresses, wrapping arithmetic, and both privilege modes. A guest fault
/// is an emulated 68000 exception, never a Rust panic.
#[test]
fn no_opcode_panics() {
    let dec = Decoder::new();
    // 0x1001 forces odd (address-error) operand addresses; 0xFFFFFFFF and 0
    // force wrapping adds and subtracts in address computation.
    let seeds: [(u32, u16); 4] = [
        (0x0000_1000, 0x2700),
        (0x0000_1001, 0x2700),
        (0xFFFF_FFFF, 0x0000),
        (0x0000_0000, 0x0000),
    ];
    for (base, sr) in seeds {
        for op in 0..=0xFFFFu32 {
            let (mut cpu, mut bus) = seeded(base, sr, op as u16);
            let _ = cpu.step_with(&dec, &mut bus);
        }
    }
}

/// Executing an opcode the illegal handler owns leaves PC at the vector-4
/// handler. Used to tell "this encoding is unclaimed" from "a MOVE handler
/// claimed it", without reaching into the decoder's private table.
fn is_illegal(dec: &Decoder, op: u16) -> bool {
    let (mut cpu, mut bus) = seeded(0x0000_1000, 0x2700, op);
    // Vector 4 (illegal instruction) is at 0x10; RAM is 0x4E4E throughout, so
    // an illegal opcode lands at 0x4E4E and nothing else does.
    bus.write16(0x10, 0x0000);
    bus.write16(0x12, 0x7000);
    let _ = cpu.step_with(dec, &mut bus);
    cpu.pc & 0xFFFF == 0x7000 + 4
}

/// The MOVE encodings that do not exist must reach the illegal handler, and
/// every encoding that does exist must not. Checked over all 65536 opcodes in
/// the four MOVE lines rather than the suite's sample.
#[test]
fn move_claims_exactly_the_legal_encodings() {
    let dec = Decoder::new();
    for op in 0x1000..0x4000u16 {
        let size_bits = op >> 12;
        let src_mode = (op >> 3) & 7;
        let src_reg = op & 7;
        let dst_mode = (op >> 6) & 7;
        let dst_reg = (op >> 9) & 7;

        // An address register is not a byte-sized source, mode 7 stops at
        // reg 4, a destination is never an immediate or PC-relative, and
        // MOVEA.b does not exist.
        let src_ok = match src_mode {
            1 => size_bits != 1,
            7 => src_reg <= 4,
            _ => true,
        };
        let dst_ok = match dst_mode {
            1 => size_bits != 1,
            7 => dst_reg <= 1,
            _ => true,
        };
        let legal = src_ok && dst_ok;
        assert_eq!(
            !is_illegal(&dec, op),
            legal,
            "opcode {op:04X}: size_bits={size_bits} src={src_mode}/{src_reg} \
             dst={dst_mode}/{dst_reg} — expected legal={legal}"
        );
    }
}

/// MOVEQ is `0111 rrr 0 dddddddd`: bit 8 must be clear. The 2048 opcodes with
/// bit 8 set are illegal, and none of them may reach the MOVEQ handler.
#[test]
fn moveq_requires_bit_8_clear() {
    let dec = Decoder::new();
    for op in 0x7000..0x8000u16 {
        let legal = op & 0x0100 == 0;
        assert_eq!(!is_illegal(&dec, op), legal, "opcode {op:04X}");
    }
}

/// What is expected of an opcode in a line an implemented task shares with a
/// future one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Claim {
    /// A Task 6 handler must own it.
    Mine,
    /// No 68000 instruction has this encoding — it must reach the illegal
    /// handler now and forever.
    Illegal,
    /// A real instruction belonging to a later task. Unasserted, because it is
    /// illegal today and must not be once that task lands.
    Later,
}

/// The addressing-mode categories from the 68000 manual, spelled out because
/// three of them differ by exactly one mode and the difference is load-bearing.
mod modes {
    /// Any source operand of this size. `An` is not a byte-sized source (there
    /// is no byte of an address register) and mode 7 stops at the immediate.
    pub fn src(mode: u16, reg: u16, byte: bool) -> bool {
        match mode {
            1 => !byte,
            7 => reg <= 4,
            _ => true,
        }
    }
    /// Alterable memory: no registers, no PC-relative, no immediate.
    pub fn mem_alterable(mode: u16, reg: u16) -> bool {
        match mode {
            0 | 1 => false,
            7 => reg <= 1,
            _ => true,
        }
    }
    /// Data-alterable: alterable memory plus a data register.
    pub fn data_alterable(mode: u16, reg: u16) -> bool {
        mode == 0 || mem_alterable(mode, reg)
    }
}

/// Classifies every opcode in the eight lines Task 6 touches.
///
/// Written as one function over the whole space rather than per family, because
/// the interesting cases are the collisions: `EOR` and `CMPM` sharing an opmode,
/// `ADDQ`'s `An` destination appearing only above byte size, and the `to CCR` /
/// `to SR` opcodes occupying an `<ea>` slot that is otherwise illegal.
fn claim(op: u16) -> Claim {
    let mode = (op >> 3) & 7;
    let reg = op & 7;
    let opmode = (op >> 6) & 7;
    let size_bits = (op >> 6) & 3;
    let mem = modes::mem_alterable(mode, reg);
    let data = modes::data_alterable(mode, reg);

    match op >> 12 {
        // 0000: the xxxI family, sharing the line with MOVEP and the bit
        // instructions.
        0x0 => {
            if op & 0x0100 != 0 {
                // MOVEP (mode 001) and the dynamic bit instructions.
                return Claim::Later;
            }
            match (op >> 9) & 7 {
                // Static BTST/BCHG/BCLR/BSET.
                4 => Claim::Later,
                // MOVES: a 68010 instruction, so illegal here.
                7 => Claim::Illegal,
                family => {
                    // Size 11 is CMP2/CHK2 on the 68020 and nothing at all here.
                    if size_bits == 3 {
                        Claim::Illegal
                    } else if mode == 7 && reg == 4 {
                        // ORI/ANDI/EORI to CCR (byte) and to SR (word). The
                        // long-sized encodings of the same slot do not exist,
                        // and neither do SUBI/ADDI/CMPI versions.
                        let to_ccr_sr = matches!(family, 0 | 1 | 5) && size_bits != 2;
                        if to_ccr_sr {
                            Claim::Mine
                        } else {
                            Claim::Illegal
                        }
                    } else if data {
                        Claim::Mine
                    } else {
                        Claim::Illegal
                    }
                }
            }
        }
        // 0100: NEGX/CLR/NEG/NOT/TST and NOP, among a crowd of later work.
        0x4 => {
            if op == 0x4E71 {
                return Claim::Mine;
            }
            let sel = (op >> 8) & 0xF;
            // Size 11 of these selectors is MOVE from SR / to CCR / to SR / TAS.
            if !matches!(sel, 0x0 | 0x2 | 0x4 | 0x6 | 0xA) || size_bits == 3 {
                Claim::Later
            } else if data {
                Claim::Mine
            } else {
                Claim::Illegal
            }
        }
        // 0101: ADDQ/SUBQ below size 11, Scc/DBcc/TRAPcc at it.
        0x5 => {
            if size_bits == 3 {
                Claim::Later
            } else if mode == 1 {
                // ADDQ/SUBQ #d,An exists at word and long size, operating on all
                // 32 bits; there is no byte form.
                if size_bits == 0 {
                    Claim::Illegal
                } else {
                    Claim::Mine
                }
            } else if data {
                Claim::Mine
            } else {
                Claim::Illegal
            }
        }
        // 1000 OR / 1100 AND: opmode 3 and 7 are the divides, and opmode 4/5/6 at
        // modes 000/001 are the BCD instructions and EXG.
        //
        // **`AND` and `OR` have no address-register source at any size** — unlike
        // `ADD`, `SUB` and `CMP`, whose word and long forms do. There is no
        // meaningful bitwise operation on an address register, and the vectors
        // agree: mode 001 never appears in lines 8 or C. Allowing it here (by
        // reusing the `ADD` rule) is what this test caught.
        0x8 | 0xC => match opmode {
            0..=2 => {
                if mode != 1 && modes::src(mode, reg, true) {
                    Claim::Mine
                } else {
                    Claim::Illegal
                }
            }
            3 | 7 => Claim::Later,
            _ if mode <= 1 => match (op >> 12, opmode, mode) {
                // SBCD and ABCD, both forms: Task 9.
                (_, 4, _) => Claim::Later,
                // EXG Dx,Dy / Ax,Ay (opmode 5) and Dx,Ay (opmode 6 mode 1):
                // Task 10.
                (0xC, 5, _) | (0xC, 6, 1) => Claim::Later,
                // Everything else in this corner is PACK/UNPK, which arrived with
                // the 68020, or simply nothing at all.
                _ => Claim::Illegal,
            },
            _ => {
                if mem {
                    Claim::Mine
                } else {
                    Claim::Illegal
                }
            }
        },
        // 1001 SUB / 1101 ADD: opmode 3/7 are SUBA/ADDA, and opmode 4/5/6 at
        // modes 000/001 are ADDX/SUBX rather than a memory destination.
        0x9 | 0xD => match opmode {
            0..=2 => {
                if modes::src(mode, reg, opmode == 0) {
                    Claim::Mine
                } else {
                    Claim::Illegal
                }
            }
            3 | 7 => {
                // SUBA/ADDA take any source at both sizes, An included.
                if modes::src(mode, reg, false) {
                    Claim::Mine
                } else {
                    Claim::Illegal
                }
            }
            _ => {
                // mode 000 = ADDX/SUBX Dy,Dx; mode 001 = ADDX/SUBX -(Ay),-(Ax),
                // which is the one place a `001` mode field is not An.
                if mode <= 1 || mem {
                    Claim::Mine
                } else {
                    Claim::Illegal
                }
            }
        },
        // 1011: CMP, CMPA, and the EOR/CMPM opmode.
        0xB => match opmode {
            0 => {
                if modes::src(mode, reg, true) {
                    Claim::Mine
                } else {
                    Claim::Illegal
                }
            }
            1 | 2 | 3 | 7 => {
                if modes::src(mode, reg, false) {
                    Claim::Mine
                } else {
                    Claim::Illegal
                }
            }
            _ => {
                // mode 001 is CMPM (Ay)+,(Ax)+; every other mode is EOR Dn,<ea>,
                // *including* mode 000, which is EOR Dn,Dn. Dropping mode 000
                // here would silently lose 381 EOR cases per size.
                if mode == 1 || data {
                    Claim::Mine
                } else {
                    Claim::Illegal
                }
            }
        },
        _ => Claim::Later,
    }
}

/// Task 6 must claim every encoding that exists in its lines and none that does
/// not — checked over all 65536 opcodes, because the suite samples 2500 cases per
/// group and a rare mode can be missing from the table without any group failing.
///
/// Encodings belonging to later tasks are skipped rather than asserted illegal:
/// they are illegal *today*, so asserting it would bake in a fact that Task 7
/// through 10 must then unbake.
#[test]
fn task6_claims_exactly_the_legal_encodings() {
    let dec = Decoder::new();
    let mut mine = 0;
    let mut illegal = 0;
    for op in 0..=0xFFFFu32 {
        let op = op as u16;
        let claimed = !is_illegal(&dec, op);
        match claim(op) {
            Claim::Mine => {
                assert!(
                    claimed,
                    "opcode {op:04X} is a legal Task 6 encoding but reaches the \
                     illegal handler"
                );
                mine += 1;
            }
            Claim::Illegal => {
                assert!(
                    !claimed,
                    "opcode {op:04X} does not exist on the 68000 but a handler \
                     claims it"
                );
                illegal += 1;
            }
            Claim::Later => {}
        }
    }
    // A guard against the classifier itself going vacuous: if a refactor made
    // `claim` return `Later` everywhere the assertions above would all pass.
    assert!(mine > 20_000, "only {mine} opcodes classified as Task 6's");
    assert!(illegal > 1_000, "only {illegal} classified as nonexistent");
}

/// The `to CCR` and `to SR` forms are six specific opcodes, and the long-sized
/// versions of the same slot do not exist. Called out separately because they are
/// the one place in Task 6 where a single opcode, not a pattern, is the encoding.
#[test]
fn to_ccr_and_to_sr_are_six_opcodes_and_no_more() {
    let dec = Decoder::new();
    for op in [0x003Cu16, 0x007C, 0x023C, 0x027C, 0x0A3C, 0x0A7C] {
        assert!(!is_illegal(&dec, op), "{op:04X} must be claimed");
    }
    // The long-sized slot (bits 7-6 = 10, so mode 7 reg 4 reads as `BC`), and the
    // same slot in the families that have no CCR/SR form: SUBI, ADDI and CMPI.
    // Note that `ORI.l #imm,<mode 7 reg 4>` would otherwise be a plausible
    // "immediate to immediate", which is why the illegal half is worth asserting.
    for op in [
        0x00BCu16, 0x02BC, 0x0ABC, // ORI.l / ANDI.l / EORI.l to <mode 7 reg 4>
        0x043C, 0x047C, // SUBI to CCR / to SR
        0x063C, 0x067C, // ADDI
        0x0C3C, 0x0C7C, // CMPI
    ] {
        assert!(is_illegal(&dec, op), "{op:04X} must not exist");
    }
}
