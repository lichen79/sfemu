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
    /// An implemented handler must own it.
    Mine,
    /// No 68000 instruction has this encoding — it must reach the illegal
    /// handler now and forever.
    Illegal,
    /// Deferred by this classifier. The name is now a historical one: with every
    /// task landed, **no** deferred opcode belongs to a later task. All 25,728 of
    /// them are either classified by a sibling test in this file or nonexistent —
    /// see [`deferred_opcodes_are_accounted_for`], which is what keeps the name
    /// from quietly becoming a lie.
    Later,
}

/// The addressing-mode categories from the 68000 manual. Canonical definitions
/// live in `m68k::ea::modes`; this re-exports them so the rest of this file
/// reads identically to before.
use m68k::ea::modes;

/// `MOVEM`'s `<ea>` rule, which is direction-dependent — the one addressing-mode
/// set in the instruction set that is not symmetric.
///
/// Control, plus whichever of the two stepping modes matches the transfer
/// direction: `-(An)` stores only and `(An)+` loads only, each being the mode that
/// walks *with* the transfer. PC-relative is likewise load-only, there being
/// nothing to store into the instruction stream.
fn movem_ea(mode: u16, reg: u16, to_regs: bool) -> bool {
    match mode {
        3 => to_regs,
        4 => !to_regs,
        7 => reg <= 1 || (reg <= 3 && to_regs),
        _ => modes::control(mode, reg),
    }
}

/// The bit instructions' destination rule, which differs between `BTST` and the
/// other three and again between the two bit-number forms.
///
/// `BTST` writes nothing, so its operand needs only to be *readable*: the two
/// PC-relative modes are legal for it and not for `BCHG`/`BCLR`/`BSET`. The
/// immediate operand `#data` is narrower still — legal in the **dynamic** form
/// only, which the vectors show directly: across `BTST`'s 2,500 cases the dynamic
/// form uses mode 7 reg 4 fifty-eight times and the static form never, while at
/// mode 5 (a control both forms reach) the split is 328 dynamic to 40 static.
fn bit_op(op: u16, dynamic: bool) -> Claim {
    let mode = (op >> 3) & 7;
    let reg = op & 7;
    let btst = (op >> 6) & 3 == 0;
    let ok = if btst {
        match mode {
            0 => true,
            1 => false,
            // Readable memory, plus the immediate in the dynamic form.
            7 => reg <= 3 || (reg == 4 && dynamic),
            _ => true,
        }
    } else {
        modes::data_alterable(mode, reg)
    };
    if ok {
        Claim::Mine
    } else {
        Claim::Illegal
    }
}

/// Classifies every opcode in the lines Tasks 6 through 9 touch.
///
/// Written as one function over the whole space rather than per family, because
/// the interesting cases are the collisions: `EOR` and `CMPM` sharing an opmode,
/// `ADDQ`'s `An` destination appearing only above byte size, the `to CCR` /
/// `to SR` opcodes occupying an `<ea>` slot that is otherwise illegal, `DBcc`
/// taking the mode-`001` slot that `Scc` would otherwise fill, and `CHK` cutting
/// across all sixteen selectors of line `0100` because its `Dn` field sits where
/// every other instruction in that line keeps its selector.
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
                // The dynamic bit instructions — and, at mode 001 only, MOVEP.
                // Every bit-op destination rule excludes mode 001 anyway, so the
                // two do not overlap.
                //
                // All four opmodes at mode 001 are MOVEP: bit 8 set means
                // opmode 4-7, which are `.w` and `.l` in each direction. Nothing
                // in this corner is illegal, so the whole of mode 001 is claimed.
                return if mode == 1 {
                    Claim::Mine
                } else {
                    bit_op(op, true)
                };
            }
            match (op >> 9) & 7 {
                // Static BTST/BCHG/BCLR/BSET.
                4 => bit_op(op, false),
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
            // CHK cuts across every selector: it is opmode 6 (bits 8-6 = 110)
            // with bits 11-9 naming the destination `Dn`, so it appears at all
            // sixteen selector values and must be tested before any `sel` arm.
            // A word-sized source `<ea>`, no `An`.
            if opmode == 6 {
                return if mode != 1 && modes::src(mode, reg, false) {
                    Claim::Mine
                } else {
                    Claim::Illegal
                };
            }
            // LEA has the same cross-cutting shape as CHK: opmode 7 with bits
            // 11-9 naming the destination `An`, so it too appears at all sixteen
            // selector values and must be tested before any `sel` arm. Nothing
            // else in line 0100 uses opmode 7. Control modes only — an address
            // with no operand access has no reason to step a register.
            if opmode == 7 {
                return if modes::control(mode, reg) {
                    Claim::Mine
                } else {
                    Claim::Illegal
                };
            }
            // NBCD: selector 8 at size bits 00, over the data-alterable set.
            if sel == 0x8 && size_bits == 0 {
                return if data { Claim::Mine } else { Claim::Illegal };
            }
            // The rest of selector 8, and all of selector C, is Task 10's:
            //
            //   4840  01  SWAP (mode 000) and PEA (control modes)
            //   4880  10  EXT.w (mode 000) and MOVEM.w reg->mem
            //   48C0  11  EXT.l (mode 000) and MOVEM.l reg->mem
            //   4C80  10  MOVEM.w mem->reg
            //   4CC0  11  MOVEM.l mem->reg
            //
            // Mode 000 is a *different instruction* in selector 8 rather than an
            // invalid MOVEM operand, which is why the mode-0 case is pulled out
            // before the MOVEM mode test rather than being folded into it.
            if sel == 0x8 && size_bits == 1 {
                return if mode == 0 || modes::control(mode, reg) {
                    Claim::Mine
                } else {
                    Claim::Illegal
                };
            }
            if sel == 0x8 && size_bits >= 2 {
                return if mode == 0 || movem_ea(mode, reg, false) {
                    Claim::Mine
                } else {
                    Claim::Illegal
                };
            }
            if sel == 0xC && size_bits >= 2 {
                return if movem_ea(mode, reg, true) {
                    Claim::Mine
                } else {
                    Claim::Illegal
                };
            }
            // Selectors 0/4/6 at size bits 11: the three SR/CCR moves. Selector 2
            // is `MOVE from CCR`, which arrived with the 68010 — the asymmetry is
            // real, and the 68000 has a `MOVE to CCR` with no matching read.
            if size_bits == 3 && matches!(sel, 0x0 | 0x2 | 0x4 | 0x6) {
                return match sel {
                    0x0 => {
                        if data {
                            Claim::Mine
                        } else {
                            Claim::Illegal
                        }
                    }
                    0x4 | 0x6 => {
                        if mode != 1 && modes::src(mode, reg, false) {
                            Claim::Mine
                        } else {
                            Claim::Illegal
                        }
                    }
                    _ => Claim::Illegal,
                };
            }
            // Size 11 of selector A is TAS; of the others it is MOVE from SR /
            // to CCR / to SR, which belong to Task 10.
            if sel == 0xA && size_bits == 3 {
                if mode == 7 && reg == 4 {
                    // 0x4AFC is `ILLEGAL`, whose entire effect *is* the
                    // illegal-instruction trap. Classifying it here is exact
                    // behaviourally — a dedicated handler for it would land at
                    // the same vector — even though the encoding exists.
                    Claim::Illegal
                } else if data {
                    Claim::Mine
                } else {
                    Claim::Illegal
                }
            } else if sel == 0xE {
                // 0100 1110: the four Task 8 encodings among a crowd of later
                // work. `JMP` (size 11) and `JSR` (size 10) take the *control*
                // modes — memory operands whose address does not depend on the
                // access, so neither `(An)+` nor `-(An)`, a jump having no
                // operand size to step by.
                if op == 0x4E75 || op == 0x4E77 {
                    // RTS and RTR, two single opcodes rather than patterns.
                    Claim::Mine
                } else if size_bits >= 2 {
                    // `modes::control`, not a hand-inlined copy of it. The LEA and
                    // PEA arms above already consult it, so the copy that used to
                    // sit here made one arm of this function disagree in form with
                    // two others about the same rule.
                    if modes::control(mode, reg) {
                        Claim::Mine
                    } else {
                        Claim::Illegal
                    }
                } else {
                    // The rest of 0100 1110 0xxx, one 16-opcode row at a time.
                    // These are single opcodes and 8-opcode runs rather than
                    // `<ea>` patterns, so this arm enumerates rather than
                    // consulting `modes::`.
                    match op {
                        // 4E40-4E4F: TRAP #0-#15.
                        0x4E40..=0x4E4F => Claim::Mine,
                        // 4E50-4E57 LINK, 4E58-4E5F UNLK.
                        0x4E50..=0x4E5F => Claim::Mine,
                        // 4E60-4E67 MOVE An,USP; 4E68-4E6F MOVE USP,An.
                        0x4E60..=0x4E6F => Claim::Mine,
                        0x4E70 => Claim::Mine, // RESET
                        0x4E72 => Claim::Mine, // STOP
                        0x4E73 => Claim::Mine, // RTE
                        0x4E76 => Claim::Mine, // TRAPV
                        // 4E71 NOP and 4E75/4E77 RTS/RTR are handled above.
                        // 4E74 is RTD (68010) and 4E7A/4E7B are MOVEC (68010),
                        // so this row's remaining encodings do not exist here.
                        0x4E71 | 0x4E75 | 0x4E77 => Claim::Mine,
                        _ => Claim::Illegal,
                    }
                }
            } else if !matches!(sel, 0x0 | 0x2 | 0x4 | 0x6 | 0xA) || size_bits == 3 {
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
                // Size 11: `DBcc` at mode 001 — the one place in this line where
                // a `001` mode field is not `An` — and `Scc` over the
                // data-alterable set everywhere else. Mode 7 reg 2/3/4 would be
                // `TRAPcc` on a 68020 and is nothing at all on a 68000; the
                // vectors agree, with zero mode-7-above-reg-1 cases in `Scc`'s
                // 2,500.
                if mode == 1 || data {
                    Claim::Mine
                } else {
                    Claim::Illegal
                }
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
            // MULU/MULS (line C) and DIVU/DIVS (line 8), all four `<ea>` sources
            // to `Dn`. Word-sized, and with no `An` source: mode 001 in this
            // opmode is nothing at all, the same exclusion `AND`/`OR` have above.
            3 | 7 => {
                if mode != 1 && modes::src(mode, reg, false) {
                    Claim::Mine
                } else {
                    Claim::Illegal
                }
            }
            _ if mode <= 1 => match (op >> 12, opmode, mode) {
                // SBCD and ABCD. Bit 3, not the mode field, picks the form: mode
                // 000 is `Dy,Dx` and 001 is `-(Ay),-(Ax)`, so both values of
                // `mode` here are legal and neither is an address-register
                // operand. That is why this arm does not consult `modes::`.
                (_, 4, _) => Claim::Mine,
                // EXG Dx,Dy / Ax,Ay (opmode 5) and Dx,Ay (opmode 6 mode 1).
                // As with ABCD above, `mode` here is a form selector rather than
                // an addressing mode, so `modes::` does not apply.
                (0xC, 5, _) | (0xC, 6, 1) => Claim::Mine,
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
        // 1110: the shifts and rotates. Two encodings in one line, split by the
        // size field.
        0xE => {
            if size_bits != 3 {
                // Register/immediate form: every combination of count, count
                // source, direction, type and data register exists, so all 3,072
                // of these are legal without further conditions.
                Claim::Mine
            } else if op & 0x0800 != 0 {
                // Bit 11 is not part of the memory form's encoding. With it set
                // these are the 68020's bit-field instructions — BFTST and its
                // relatives — and nothing at all on a 68000.
                Claim::Illegal
            } else if mem {
                Claim::Mine
            } else {
                Claim::Illegal
            }
        }
        // 0110: Bcc, BRA and BSR. All 4,096 encodings exist — 16 conditions
        // (condition 0 being BRA and condition 1 BSR, not "never") times 256
        // displacements, with displacement 0x00 selecting the 16-bit form rather
        // than being a hole. There is nothing to classify as illegal.
        0x6 => Claim::Mine,
        _ => Claim::Later,
    }
}

/// The implemented tasks must claim every encoding that exists in their lines and
/// none that does not — checked over all 65536 opcodes, because the suite samples
/// 2500 cases per group and a rare mode can be missing from the table without any
/// group failing.
///
/// Encodings this classifier defers are skipped rather than asserted illegal.
/// Through Tasks 5-10 that was because they were illegal *today* and must not be
/// once a later task landed. Every task has now landed and the skip remains, for
/// a different reason: whole lines are classified by the sibling tests above
/// rather than by `claim`, so asserting anything here would duplicate them. The
/// count is asserted instead — see [`deferred_opcodes_are_accounted_for`].
#[test]
fn implemented_tasks_claim_exactly_the_legal_encodings() {
    let dec = Decoder::new();
    let mut mine = 0;
    let mut illegal = 0;
    let mut later = 0;
    for op in 0..=0xFFFFu32 {
        let op = op as u16;
        let claimed = !is_illegal(&dec, op);
        match claim(op) {
            Claim::Mine => {
                assert!(
                    claimed,
                    "opcode {op:04X} is a legal encoding of an implemented \
                     instruction but reaches the illegal handler"
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
            Claim::Later => later += 1,
        }
    }
    // The deferred count is asserted exactly, not as a floor. See
    // `deferred_opcodes_are_accounted_for` for where each of these lands; the
    // point here is that `mine`, `illegal` and `later` partition the space, so
    // an encoding cannot silently move between the three.
    assert_eq!(
        mine + illegal + later,
        0x10000,
        "the three claims must partition the opcode space"
    );
    assert_eq!(later, 25_728, "deferred opcode census changed");
    // A guard against the classifier itself going vacuous: if a refactor made
    // `claim` return `Later` everywhere the assertions above would all pass.
    //
    // The thresholds are floors, not measurements. The history: Task 6 left them
    // at 20,177 / 3,632, Task 7 at 25,461 / 4,556, Task 8 at 30,543 / 4,724,
    // Task 9 at 32,969 / 5,178, Task 10 at 34,023 / 5,767, and Task 11 at
    // **34,041 / 5,767**.
    //
    // Task 11 moved `mine` by exactly 18 and `illegal` by 0, which is the whole
    // of its opcode footprint: the sixteen `TRAP #n`, plus `RTE` and `TRAPV`.
    // Those eighteen were `Claim::Later` before and are now `Claim::Mine`. No
    // encoding changed legality, so `illegal` is unchanged at 5,767 — but its
    // floor still sat at 5,700 from Task 9, so it is tightened here too.
    //
    // Task 9 raised both, having left them alone through two tasks: at 20,000 the
    // `mine` floor sat at 61% of the true count and the `illegal` one at 19%, and
    // a floor that far below the value it guards would not notice a whole line
    // reverting to `Later`. They are set just under the current counts, which
    // costs one line of maintenance per task and buys a guard that actually
    // bites — the earlier note's reasoning (a guard adjusted every task is one
    // nobody trusts) is true of a floor tracked *upward for its own sake*, not of
    // one that has drifted an order of magnitude loose.
    //
    // Both floors are re-confirmed non-vacuous by raising them until they fail:
    // the failure messages report the counts above, so the classifier really is
    // reaching the new lines and not returning `Later` across them.
    assert!(
        mine > 34_040,
        "only {mine} opcodes classified as implemented"
    );
    assert!(illegal > 5_766, "only {illegal} classified as nonexistent");
}

/// Every deferred opcode is either classified elsewhere in this file or does not
/// exist — no encoding is unaccounted for now that all fourteen tasks have landed.
///
/// `claim` returns [`Claim::Later`] for 25,728 opcodes, which through Task 10 was
/// read as "a later task will own these". That reading is now retracted: nothing
/// is left to land, so each of them must already be settled. Where:
///
/// | line | count | who settles it |
/// |------|-------|----------------|
/// | 0001, 0010, 0011 | 12,288 | `move_claims_exactly_the_legal_encodings` |
/// | 0100 | 1,152 | nobody — 68020 encodings, asserted illegal below |
/// | 0111 | 4,096 | `moveq_requires_bit_8_clear` |
/// | 1010, 1111 | 8,192 | the `ILLEGAL_LINEA`/`ILLEGAL_LINEF` suite groups |
///
/// Line 0100's 1,152 are the only ones no other test touches, so they are the
/// only ones this test executes. They decompose as 1,024 at the odd selectors
/// (1, 3, 5, 7, 9, B, D, F) with opmode 4 or 5 — `MOVE from CCR` is selector 2
/// and the odd selectors have no instruction at those opmodes at all — plus 128
/// at selector C size bits 00 and 01, which is the 68020's 32-bit `MULU`/`DIVU`
/// and nothing on a 68000.
///
/// The reason this is a separate test rather than more `Claim::Illegal` arms in
/// `claim` is that `claim`'s line-0100 arm is already the longest in the file and
/// these encodings share no structure with the instructions around them.
#[test]
fn deferred_opcodes_are_accounted_for() {
    let dec = Decoder::new();
    let mut checked = 0;
    for op in 0x4000..0x5000u16 {
        if claim(op) != Claim::Later {
            continue;
        }
        let sel = (op >> 8) & 0xF;
        let opmode = (op >> 6) & 7;
        let size_bits = (op >> 6) & 3;
        // The classification this test asserts, stated independently of `claim`.
        let known = ((sel & 1) == 1 && matches!(opmode, 4 | 5))
            || (sel == 0xC && matches!(size_bits, 0 | 1));
        assert!(
            known,
            "opcode {op:04X} (selector {sel:X}, opmode {opmode}) is deferred by \
             `claim` and matches no known 68020-only encoding"
        );
        assert!(
            is_illegal(&dec, op),
            "opcode {op:04X} does not exist on the 68000 but a handler claims it"
        );
        checked += 1;
    }
    assert_eq!(checked, 1_152, "line 0100's deferred census changed");
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
