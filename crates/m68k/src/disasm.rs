//! Motorola 68000 disassembler.
//!
//! Decodes one instruction from a stream of 16-bit words and returns its
//! text and length. The output format follows MAME-style lowercase mnemonics.
//!
//! # Feature gate
//!
//! This module requires the `std` feature because [`Insn`] holds a [`String`].
//! Gate it: `#[cfg(feature = "std")] pub mod disasm;` in `lib.rs`.
//!
//! # Unknown opcodes
//!
//! Any word that does not match a known 68000 encoding renders as `dc.w $XXXX`.
//! The function never panics regardless of the input.

/// The result of disassembling one instruction.
pub struct Insn {
    /// MAME-style lowercase mnemonic with operands, e.g. `"move.l $123456,d0"`.
    pub text: String,
    /// Byte length of the instruction, including extension words.
    pub len: u32,
}

/// Disassembles one instruction.
///
/// `read` is called with a **byte address** (always even) and must return the
/// 16-bit word at that address. The first call is always with `addr`, which
/// fetches the opcode word. Extension words are fetched at `addr + 2`,
/// `addr + 4`, etc.
///
/// The result's `len` is always at least 2 (the opcode word itself).
pub fn disassemble(mut read: impl FnMut(u32) -> u16, addr: u32) -> Insn {
    let op = read(addr);
    let mut d = Dis {
        read: &mut read,
        addr,
        words_read: 1,
    };
    let text = d.decode(op);
    Insn {
        text,
        len: d.words_read * 2,
    }
}

// ---------------------------------------------------------------------------
// Internal decoder state
// ---------------------------------------------------------------------------

struct Dis<'a> {
    read: &'a mut dyn FnMut(u32) -> u16,
    addr: u32,
    words_read: u32,
}

impl<'a> Dis<'a> {
    /// Reads the next extension word, advancing the position counter.
    fn next_word(&mut self) -> u16 {
        let w = (self.read)(self.addr + self.words_read * 2);
        self.words_read += 1;
        w
    }

    /// Decodes `op` and returns its text.
    fn decode(&mut self, op: u16) -> String {
        let nibble = op >> 12;
        match nibble {
            0x0 => self.decode_nibble0(op),
            0x1..=0x3 => self.decode_move(op),
            0x4 => self.decode_nibble4(op),
            0x5 => self.decode_nibble5(op),
            0x6 => self.decode_branch(op),
            0x7 => self.decode_moveq(op),
            0x8 => self.decode_or_muldiv_bcd(op),
            0x9 => self.decode_sub_family(op),
            0xA => self.illegal_word(op),
            0xB => self.decode_cmp_eor(op),
            0xC => self.decode_and_muldiv_bcd(op),
            0xD => self.decode_add_family(op),
            0xE => self.decode_shift(op),
            0xF => self.illegal_word(op),
            _ => self.illegal_word(op),
        }
    }

    // -----------------------------------------------------------------------
    // Nibble 0: ORI/ANDI/SUBI/ADDI/EORI/CMPI, MOVEP, bit operations
    // -----------------------------------------------------------------------

    fn decode_nibble0(&mut self, op: u16) -> String {
        if op & 0x0100 != 0 {
            // Bit 8 set: MOVEP or dynamic bit ops.
            let mode = (op >> 3) & 7;
            if mode == 1 {
                // MOVEP
                return self.decode_movep(op);
            }
            // Dynamic bit op: bit number in Dn.
            return self.decode_bit_op(op, true);
        }
        // Bit 8 clear.
        let family = (op >> 9) & 7;
        if family == 4 {
            // Static bit op: bit number in immediate.
            return self.decode_bit_op(op, false);
        }
        // Immediate operations: ORI, ANDI, SUBI, ADDI, EORI, CMPI.
        self.decode_immediate_op(op)
    }

    fn decode_movep(&mut self, op: u16) -> String {
        let dn = (op >> 9) & 7;
        let an = op & 7;
        let opmode = (op >> 6) & 7;
        let disp = self.next_word() as i16;
        let ea_str = format_disp_an(disp, an);
        match opmode {
            4 => format!("movep.w {ea_str},d{dn}"),
            5 => format!("movep.l {ea_str},d{dn}"),
            6 => format!("movep.w d{dn},{ea_str}"),
            7 => format!("movep.l d{dn},{ea_str}"),
            _ => self.illegal_word(op),
        }
    }

    fn decode_bit_op(&mut self, op: u16, dynamic: bool) -> String {
        let bit_type = (op >> 6) & 3;
        let mode = (op >> 3) & 7;
        let reg = op & 7;
        let mnemonic = match bit_type {
            0 => "btst",
            1 => "bchg",
            2 => "bclr",
            _ => "bset",
        };
        let bit_str = if dynamic {
            let dn = (op >> 9) & 7;
            format!("d{dn}")
        } else {
            // Static: bit number in next word's low byte.
            let imm = self.next_word() & 0xFF;
            format!("#{imm}")
        };
        let ea_str = self.format_ea(mode, reg, EaSize::Byte);
        format!("{mnemonic} {bit_str},{ea_str}")
    }

    fn decode_immediate_op(&mut self, op: u16) -> String {
        let family = (op >> 9) & 7;
        let size_bits = (op >> 6) & 3;
        let mode = (op >> 3) & 7;
        let reg = op & 7;

        let sz = match size_bits {
            0 => EaSize::Byte,
            1 => EaSize::Word,
            2 => EaSize::Long,
            _ => return self.illegal_word(op),
        };

        // CCR/SR forms: mode 7 reg 4, byte and word only.
        if mode == 7 && reg == 4 {
            let (mnemonic, dest_str) = match (family, size_bits) {
                (0, 0) => ("ori", "#imm,ccr"),  // ORI to CCR
                (0, 1) => ("ori", "#imm,sr"),   // ORI to SR
                (1, 0) => ("andi", "#imm,ccr"), // ANDI to CCR
                (1, 1) => ("andi", "#imm,sr"),  // ANDI to SR
                (5, 0) => ("eori", "#imm,ccr"), // EORI to CCR
                (5, 1) => ("eori", "#imm,sr"),  // EORI to SR
                _ => return self.illegal_word(op),
            };
            let imm = self.read_immediate(sz);
            return format!("{mnemonic} #{imm},{}", &dest_str[5..]);
        }

        let mnemonic = match family {
            0 => "ori",
            1 => "andi",
            2 => "subi",
            3 => "addi",
            5 => "eori",
            6 => "cmpi",
            _ => return self.illegal_word(op),
        };
        let size_suffix = size_suffix(sz);
        let imm = self.read_immediate(sz);
        let ea_str = self.format_ea(mode, reg, sz);
        format!("{mnemonic}.{size_suffix} #{imm},{ea_str}")
    }

    fn read_immediate(&mut self, sz: EaSize) -> u32 {
        match sz {
            EaSize::Byte => {
                let w = self.next_word();
                (w & 0xFF) as u32
            }
            EaSize::Word => self.next_word() as u32,
            EaSize::Long => {
                let hi = self.next_word() as u32;
                let lo = self.next_word() as u32;
                (hi << 16) | lo
            }
        }
    }

    // -----------------------------------------------------------------------
    // Lines 0001/0010/0011: MOVE
    // -----------------------------------------------------------------------

    fn decode_move(&mut self, op: u16) -> String {
        let size_bits = op >> 12;
        let sz = match size_bits {
            1 => EaSize::Byte,
            3 => EaSize::Word,
            2 => EaSize::Long,
            _ => return self.illegal_word(op),
        };

        let dst_mode = (op >> 6) & 7;
        let dst_reg = (op >> 9) & 7;
        let src_mode = (op >> 3) & 7;
        let src_reg = op & 7;

        // MOVEA: destination mode 001 (address register).
        if dst_mode == 1 {
            if sz == EaSize::Byte {
                return self.illegal_word(op);
            }
            let size_suffix = match sz {
                EaSize::Word => "w",
                EaSize::Long => "l",
                _ => return self.illegal_word(op),
            };
            let src_str = self.format_ea(src_mode, src_reg, sz);
            return format!("movea.{size_suffix} {src_str},a{dst_reg}");
        }

        let size_suffix = size_suffix(sz);
        // Source is decoded first (it comes first in the instruction stream).
        let src_str = self.format_ea(src_mode, src_reg, sz);
        let dst_str = self.format_ea_write(dst_mode, dst_reg, sz);
        format!("move.{size_suffix} {src_str},{dst_str}")
    }

    // -----------------------------------------------------------------------
    // Nibble 4: single-operand and miscellaneous instructions
    // -----------------------------------------------------------------------

    fn decode_nibble4(&mut self, op: u16) -> String {
        let mode = (op >> 3) & 7;
        let reg = op & 7;
        let opmode = (op >> 6) & 7;
        let size_bits = opmode & 3; // bits 7-6

        // CHK: opmode 110, all selectors.
        if opmode == 6 {
            let dn = (op >> 9) & 7;
            let ea_str = self.format_ea(mode, reg, EaSize::Word);
            return format!("chk {ea_str},d{dn}");
        }

        // LEA: opmode 111.
        if opmode == 7 {
            let an = (op >> 9) & 7;
            let ea_str = self.format_ea(mode, reg, EaSize::Long);
            return format!("lea {ea_str},a{an}");
        }

        let sel = (op >> 8) & 0xF;

        // NBCD: selector 8, size bits 00.
        if sel == 0x8 && size_bits == 0 {
            let ea_str = self.format_ea(mode, reg, EaSize::Byte);
            return format!("nbcd {ea_str}");
        }

        // Selector 8, size bits 01: SWAP (mode 000) or PEA (control).
        if sel == 0x8 && size_bits == 1 {
            if mode == 0 {
                return format!("swap d{reg}");
            }
            let ea_str = self.format_ea(mode, reg, EaSize::Long);
            return format!("pea {ea_str}");
        }

        // Selector 8, size bits 10: EXT.w (mode 000) or MOVEM.w reg->mem.
        if sel == 0x8 && size_bits == 2 {
            if mode == 0 {
                return format!("ext.w d{reg}");
            }
            return self.decode_movem(op, false, EaSize::Word);
        }

        // Selector 8, size bits 11: EXT.l (mode 000) or MOVEM.l reg->mem.
        if sel == 0x8 && size_bits == 3 {
            if mode == 0 {
                return format!("ext.l d{reg}");
            }
            return self.decode_movem(op, false, EaSize::Long);
        }

        // Selector C, size bits 10: MOVEM.w mem->reg.
        if sel == 0xC && size_bits == 2 {
            return self.decode_movem(op, true, EaSize::Word);
        }

        // Selector C, size bits 11: MOVEM.l mem->reg.
        if sel == 0xC && size_bits == 3 {
            return self.decode_movem(op, true, EaSize::Long);
        }

        // SR/CCR moves: size bits 11 at selectors 0, 4, 6.
        if size_bits == 3 {
            return match sel {
                0x0 => {
                    // MOVEfromSR: MOVE SR,<ea>
                    let ea_str = self.format_ea(mode, reg, EaSize::Word);
                    format!("move sr,{ea_str}")
                }
                0x4 => {
                    // MOVEtoCCR: MOVE <ea>,ccr
                    let ea_str = self.format_ea(mode, reg, EaSize::Word);
                    format!("move {ea_str},ccr")
                }
                0x6 => {
                    // MOVEtoSR: MOVE <ea>,sr
                    let ea_str = self.format_ea(mode, reg, EaSize::Word);
                    format!("move {ea_str},sr")
                }
                0xA => {
                    // TAS: selector A, size 11.
                    if mode == 7 && reg == 4 {
                        // 0x4AFC is the ILLEGAL instruction encoding.
                        return self.illegal_word(op);
                    }
                    let ea_str = self.format_ea(mode, reg, EaSize::Byte);
                    format!("tas {ea_str}")
                }
                _ => self.decode_nibble4_4e(op),
            };
        }

        // Selectors 0, 2, 4, 6 at size bits 00/01/10: NEGX/CLR/NEG/NOT/TST.
        if matches!(sel, 0x0 | 0x2 | 0x4 | 0x6) {
            let sz = match size_bits {
                0 => EaSize::Byte,
                1 => EaSize::Word,
                2 => EaSize::Long,
                _ => return self.illegal_word(op),
            };
            let mnemonic = match sel {
                0x0 => "negx",
                0x2 => "clr",
                0x4 => "neg",
                0x6 => "not",
                _ => return self.illegal_word(op),
            };
            let ea_str = self.format_ea(mode, reg, sz);
            return format!("{mnemonic}.{} {ea_str}", size_suffix(sz));
        }

        // Selector A at size 00/01/10: TST.
        if sel == 0xA && size_bits < 3 {
            let sz = match size_bits {
                0 => EaSize::Byte,
                1 => EaSize::Word,
                2 => EaSize::Long,
                _ => return self.illegal_word(op),
            };
            let ea_str = self.format_ea(mode, reg, sz);
            return format!("tst.{} {ea_str}", size_suffix(sz));
        }

        // Selector E: 0100 1110 — the 0x4Exx space.
        if sel == 0xE {
            return self.decode_nibble4_4e(op);
        }

        self.illegal_word(op)
    }

    fn decode_movem(&mut self, op: u16, to_regs: bool, sz: EaSize) -> String {
        let mask = self.next_word();
        let mode = (op >> 3) & 7;
        let reg = op & 7;
        let sz_suffix = match sz {
            EaSize::Word => "w",
            EaSize::Long => "l",
            _ => "?",
        };
        let reg_list = if to_regs || mode == 4 {
            // Pre-decrement (to memory) uses reversed mask.
            format_movem_mask(mask)
        } else {
            format_movem_mask(mask)
        };
        let ea_str = self.format_ea(mode, reg, sz);
        if to_regs {
            format!("movem.{sz_suffix} {ea_str},{reg_list}")
        } else {
            format!("movem.{sz_suffix} {reg_list},{ea_str}")
        }
    }

    fn decode_nibble4_4e(&mut self, op: u16) -> String {
        // The composite key for the 0x4Exx space.
        let lo = op & 0xFF;
        let mode = (op >> 3) & 7;
        let reg = op & 7;
        let size_bits = (op >> 6) & 3;

        match lo {
            0x40..=0x4F => {
                // TRAP #0-#15.
                let n = lo & 0xF;
                format!("trap #{n}")
            }
            0x50..=0x57 => {
                // LINK An, #disp
                let disp = self.next_word() as i16;
                let an = lo & 7;
                format!("link a{an},#{disp}")
            }
            0x58..=0x5F => {
                // UNLK An
                let an = lo & 7;
                format!("unlk a{an}")
            }
            0x60..=0x67 => {
                // MOVE An,USP
                let an = lo & 7;
                format!("move a{an},usp")
            }
            0x68..=0x6F => {
                // MOVE USP,An
                let an = lo & 7;
                format!("move usp,a{an}")
            }
            0x70 => "reset".to_string(),
            0x71 => "nop".to_string(),
            0x72 => {
                let imm = self.next_word();
                format!("stop #${imm:04X}")
            }
            0x73 => "rte".to_string(),
            0x75 => "rts".to_string(),
            0x76 => "trapv".to_string(),
            0x77 => "rtr".to_string(),
            _ => {
                // JSR and JMP: size bits 10 = JSR, 11 = JMP.
                if size_bits == 2 {
                    let ea_str = self.format_ea(mode, reg, EaSize::Long);
                    format!("jsr {ea_str}")
                } else if size_bits == 3 {
                    let ea_str = self.format_ea(mode, reg, EaSize::Long);
                    format!("jmp {ea_str}")
                } else {
                    self.illegal_word(op)
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Nibble 5: ADDQ/SUBQ, Scc, DBcc
    // -----------------------------------------------------------------------

    fn decode_nibble5(&mut self, op: u16) -> String {
        let size_bits = (op >> 6) & 3;
        let mode = (op >> 3) & 7;
        let reg = op & 7;
        let cond = ((op >> 8) & 0xF) as u8;

        if size_bits == 3 {
            // DBcc or Scc.
            if mode == 1 {
                // DBcc Dn, disp
                let disp = self.next_word() as i16;
                let target = (self.addr as i64 + 2 + disp as i64) as u32;
                let cc = cc_name(cond, DbccStyle::Dbcc);
                return format!("db{cc} d{reg},${target:X}");
            }
            // Scc <ea>
            let cc = cc_name(cond, DbccStyle::Scc);
            let ea_str = self.format_ea(mode, reg, EaSize::Byte);
            return format!("s{cc} {ea_str}");
        }

        // ADDQ or SUBQ.
        let sz = match size_bits {
            0 => EaSize::Byte,
            1 => EaSize::Word,
            2 => EaSize::Long,
            _ => unreachable!(),
        };
        let data = ((op >> 9) & 7) as u32;
        let data = if data == 0 { 8 } else { data };
        let mnemonic = if op & 0x0100 != 0 { "subq" } else { "addq" };
        let ea_str = self.format_ea(mode, reg, sz);
        format!("{mnemonic}.{} #{data},{ea_str}", size_suffix(sz))
    }

    // -----------------------------------------------------------------------
    // Nibble 6: Bcc, BRA, BSR
    // -----------------------------------------------------------------------

    fn decode_branch(&mut self, op: u16) -> String {
        let cond = ((op >> 8) & 0xF) as u8;
        let disp8 = (op & 0xFF) as u8;
        let (disp, wide) = if disp8 == 0 {
            (self.next_word() as i16 as i32, true)
        } else {
            (disp8 as i8 as i32, false)
        };
        // Base is opcode_addr + 2, regardless of displacement width.
        let base = self.addr.wrapping_add(2);
        let target = (base as i64 + disp as i64) as u32;
        let _ = wide;

        let mnemonic = match cond {
            0 => "bra".to_string(),
            1 => "bsr".to_string(),
            c => format!("b{}", cc_name(c, DbccStyle::Bcc)),
        };
        format!("{mnemonic} ${target:X}")
    }

    // -----------------------------------------------------------------------
    // Nibble 7: MOVEQ
    // -----------------------------------------------------------------------

    fn decode_moveq(&mut self, op: u16) -> String {
        if op & 0x0100 != 0 {
            return self.illegal_word(op);
        }
        let dn = (op >> 9) & 7;
        let data = (op & 0xFF) as i8;
        format!("moveq #{data},d{dn}")
    }

    // -----------------------------------------------------------------------
    // Nibble 8: OR, DIVU/DIVS, SBCD
    // -----------------------------------------------------------------------

    fn decode_or_muldiv_bcd(&mut self, op: u16) -> String {
        let dn = (op >> 9) & 7;
        let opmode = (op >> 6) & 7;
        let mode = (op >> 3) & 7;
        let reg = op & 7;

        // DIVU: opmode 011.
        if opmode == 3 {
            let ea_str = self.format_ea(mode, reg, EaSize::Word);
            return format!("divu {ea_str},d{dn}");
        }
        // DIVS: opmode 111.
        if opmode == 7 {
            let ea_str = self.format_ea(mode, reg, EaSize::Word);
            return format!("divs {ea_str},d{dn}");
        }
        // SBCD: opmode 4/5/6 at mode 000 or 001.
        if matches!(opmode, 4..=6) && mode <= 1 {
            let rx = dn;
            let ry = reg;
            if mode == 0 {
                return format!("sbcd d{ry},d{rx}");
            } else {
                return format!("sbcd -(a{ry}),-(a{rx})");
            }
        }
        // OR: opmodes 0-2 (ea->Dn) and 4-6 (Dn->ea).
        let sz = match opmode & 3 {
            0 => EaSize::Byte,
            1 => EaSize::Word,
            2 => EaSize::Long,
            _ => return self.illegal_word(op),
        };
        let to_dn = opmode < 4;
        let ea_str = self.format_ea(mode, reg, sz);
        if to_dn {
            format!("or.{} {ea_str},d{dn}", size_suffix(sz))
        } else {
            format!("or.{} d{dn},{ea_str}", size_suffix(sz))
        }
    }

    // -----------------------------------------------------------------------
    // Nibble 9: SUB family (SUB, SUBA, SUBX, SUBI, SUBQ)
    // -----------------------------------------------------------------------

    fn decode_sub_family(&mut self, op: u16) -> String {
        let dn = (op >> 9) & 7;
        let opmode = (op >> 6) & 7;
        let mode = (op >> 3) & 7;
        let reg = op & 7;

        // SUBA: opmode 011 (word) or 111 (long).
        if opmode == 3 {
            let ea_str = self.format_ea(mode, reg, EaSize::Word);
            return format!("suba.w {ea_str},a{dn}");
        }
        if opmode == 7 {
            let ea_str = self.format_ea(mode, reg, EaSize::Long);
            return format!("suba.l {ea_str},a{dn}");
        }
        // SUBX: opmode 4/5/6 at mode 000 or 001.
        if matches!(opmode, 4..=6) && mode <= 1 {
            let sz = match opmode {
                4 => EaSize::Byte,
                5 => EaSize::Word,
                _ => EaSize::Long,
            };
            let rx = dn;
            let ry = reg;
            if mode == 0 {
                return format!("subx.{} d{ry},d{rx}", size_suffix(sz));
            } else {
                return format!("subx.{} -(a{ry}),-(a{rx})", size_suffix(sz));
            }
        }
        // SUB: opmodes 0-2 (ea->Dn) and 4-6 (Dn->ea).
        let sz = match opmode & 3 {
            0 => EaSize::Byte,
            1 => EaSize::Word,
            2 => EaSize::Long,
            _ => return self.illegal_word(op),
        };
        let to_dn = opmode < 4;
        let ea_str = self.format_ea(mode, reg, sz);
        if to_dn {
            format!("sub.{} {ea_str},d{dn}", size_suffix(sz))
        } else {
            format!("sub.{} d{dn},{ea_str}", size_suffix(sz))
        }
    }

    // -----------------------------------------------------------------------
    // Nibble B: CMP, CMPA, CMPM, EOR
    // -----------------------------------------------------------------------

    fn decode_cmp_eor(&mut self, op: u16) -> String {
        let dn = (op >> 9) & 7;
        let opmode = (op >> 6) & 7;
        let mode = (op >> 3) & 7;
        let reg = op & 7;

        // CMPA: opmode 011 (word) or 111 (long).
        if opmode == 3 {
            let ea_str = self.format_ea(mode, reg, EaSize::Word);
            return format!("cmpa.w {ea_str},a{dn}");
        }
        if opmode == 7 {
            let ea_str = self.format_ea(mode, reg, EaSize::Long);
            return format!("cmpa.l {ea_str},a{dn}");
        }
        // CMP: opmodes 0-2 (ea->Dn, always CMP).
        if matches!(opmode, 0..=2) {
            let sz = match opmode {
                0 => EaSize::Byte,
                1 => EaSize::Word,
                _ => EaSize::Long,
            };
            let ea_str = self.format_ea(mode, reg, sz);
            return format!("cmp.{} {ea_str},d{dn}", size_suffix(sz));
        }
        // Opmodes 4/5/6: CMPM (mode 001) vs EOR (everything else).
        if matches!(opmode, 4..=6) {
            let sz = match opmode {
                4 => EaSize::Byte,
                5 => EaSize::Word,
                _ => EaSize::Long,
            };
            if mode == 1 {
                // CMPM (Ay)+,(Ax)+
                let ax = dn;
                let ay = reg;
                return format!("cmpm.{} (a{ay})+,(a{ax})+", size_suffix(sz));
            }
            // EOR Dn,<ea>
            let ea_str = self.format_ea(mode, reg, sz);
            return format!("eor.{} d{dn},{ea_str}", size_suffix(sz));
        }
        self.illegal_word(op)
    }

    // -----------------------------------------------------------------------
    // Nibble C: AND, MULU/MULS, ABCD, EXG
    // -----------------------------------------------------------------------

    fn decode_and_muldiv_bcd(&mut self, op: u16) -> String {
        let dn = (op >> 9) & 7;
        let opmode = (op >> 6) & 7;
        let mode = (op >> 3) & 7;
        let reg = op & 7;

        // MULU: opmode 011.
        if opmode == 3 {
            let ea_str = self.format_ea(mode, reg, EaSize::Word);
            return format!("mulu {ea_str},d{dn}");
        }
        // MULS: opmode 111.
        if opmode == 7 {
            let ea_str = self.format_ea(mode, reg, EaSize::Word);
            return format!("muls {ea_str},d{dn}");
        }
        // ABCD: opmode 4 at mode 000 or 001.
        if opmode == 4 && mode <= 1 {
            let rx = dn;
            let ry = reg;
            if mode == 0 {
                return format!("abcd d{ry},d{rx}");
            } else {
                return format!("abcd -(a{ry}),-(a{rx})");
            }
        }
        // EXG: opmode 5 (Dx,Dy or Ax,Ay) and opmode 6 at mode 001 (Dx,Ay).
        if opmode == 5 {
            // bits 3-0 select the other register; bit 3 selects type if mode < 2.
            // Actually: opmode 5 is EXG Dx,Dy (mode 000) or EXG Ax,Ay (mode 001).
            // Both mode values here are used.
            if mode == 0 {
                return format!("exg d{dn},d{reg}");
            }
            if mode == 1 {
                return format!("exg a{dn},a{reg}");
            }
        }
        if opmode == 6 && mode == 1 {
            return format!("exg d{dn},a{reg}");
        }
        // AND: opmodes 0-2 (ea->Dn) and 4-6 (Dn->ea, excluding BCD slots).
        let sz = match opmode & 3 {
            0 => EaSize::Byte,
            1 => EaSize::Word,
            2 => EaSize::Long,
            _ => return self.illegal_word(op),
        };
        let to_dn = opmode < 4;
        let ea_str = self.format_ea(mode, reg, sz);
        if to_dn {
            format!("and.{} {ea_str},d{dn}", size_suffix(sz))
        } else {
            format!("and.{} d{dn},{ea_str}", size_suffix(sz))
        }
    }

    // -----------------------------------------------------------------------
    // Nibble D: ADD family (ADD, ADDA, ADDX, ADDI, ADDQ)
    // -----------------------------------------------------------------------

    fn decode_add_family(&mut self, op: u16) -> String {
        let dn = (op >> 9) & 7;
        let opmode = (op >> 6) & 7;
        let mode = (op >> 3) & 7;
        let reg = op & 7;

        // ADDA: opmode 011 (word) or 111 (long).
        if opmode == 3 {
            let ea_str = self.format_ea(mode, reg, EaSize::Word);
            return format!("adda.w {ea_str},a{dn}");
        }
        if opmode == 7 {
            let ea_str = self.format_ea(mode, reg, EaSize::Long);
            return format!("adda.l {ea_str},a{dn}");
        }
        // ADDX: opmode 4/5/6 at mode 000 or 001.
        if matches!(opmode, 4..=6) && mode <= 1 {
            let sz = match opmode {
                4 => EaSize::Byte,
                5 => EaSize::Word,
                _ => EaSize::Long,
            };
            let rx = dn;
            let ry = reg;
            if mode == 0 {
                return format!("addx.{} d{ry},d{rx}", size_suffix(sz));
            } else {
                return format!("addx.{} -(a{ry}),-(a{rx})", size_suffix(sz));
            }
        }
        // ADD: opmodes 0-2 (ea->Dn) and 4-6 (Dn->ea).
        let sz = match opmode & 3 {
            0 => EaSize::Byte,
            1 => EaSize::Word,
            2 => EaSize::Long,
            _ => return self.illegal_word(op),
        };
        let to_dn = opmode < 4;
        let ea_str = self.format_ea(mode, reg, sz);
        if to_dn {
            format!("add.{} {ea_str},d{dn}", size_suffix(sz))
        } else {
            format!("add.{} d{dn},{ea_str}", size_suffix(sz))
        }
    }

    // -----------------------------------------------------------------------
    // Nibble E: shifts and rotates
    // -----------------------------------------------------------------------

    fn decode_shift(&mut self, op: u16) -> String {
        let size_bits = (op >> 6) & 3;

        if size_bits == 3 {
            // Memory form: 1110 0tt d 11 mmm rrr
            // Type is in bits 10-9, not bits 4-3.
            if op & 0x0800 != 0 {
                return self.illegal_word(op);
            }
            let shift_type = (op >> 9) & 3;
            let dir = (op >> 8) & 1;
            let mode = (op >> 3) & 7;
            let reg = op & 7;
            let mnemonic = shift_mnemonic(shift_type, dir);
            let ea_str = self.format_ea(mode, reg, EaSize::Word);
            return format!("{mnemonic}.w {ea_str}");
        }

        // Register/immediate form: 1110 ccc d ss i tt yyy
        let sz = match size_bits {
            0 => EaSize::Byte,
            1 => EaSize::Word,
            2 => EaSize::Long,
            _ => unreachable!(),
        };
        let dir = (op >> 8) & 1;
        let count_in_reg = (op >> 5) & 1 != 0;
        let shift_type = (op >> 3) & 3;
        let dn = op & 7;
        let count_field = (op >> 9) & 7;

        let mnemonic = shift_mnemonic(shift_type, dir);
        let count_str = if count_in_reg {
            format!("d{count_field}")
        } else {
            let count = if count_field == 0 { 8 } else { count_field };
            format!("#{count}")
        };
        format!("{mnemonic}.{} {count_str},d{dn}", size_suffix(sz))
    }

    // -----------------------------------------------------------------------
    // Effective address formatting
    // -----------------------------------------------------------------------

    /// Format a source EA, reading any extension words.
    fn format_ea(&mut self, mode: u16, reg: u16, sz: EaSize) -> String {
        self.format_ea_inner(mode, reg, sz)
    }

    /// Format a destination EA (same as source for disassembly).
    fn format_ea_write(&mut self, mode: u16, reg: u16, sz: EaSize) -> String {
        self.format_ea_inner(mode, reg, sz)
    }

    fn format_ea_inner(&mut self, mode: u16, reg: u16, sz: EaSize) -> String {
        let r = reg as usize;
        match mode {
            0 => format!("d{r}"),
            1 => format!("a{r}"),
            2 => format!("(a{r})"),
            3 => format!("(a{r})+"),
            4 => format!("-(a{r})"),
            5 => {
                let disp = self.next_word() as i16;
                format_disp_an(disp, reg)
            }
            6 => {
                let ext = self.next_word();
                format_index_ea(ext, r)
            }
            7 => match reg {
                0 => {
                    let w = self.next_word() as i16;
                    format!("${:X}", w as i32)
                }
                1 => {
                    let hi = self.next_word() as u32;
                    let lo = self.next_word() as u32;
                    let addr = (hi << 16) | lo;
                    format!("${addr:X}")
                }
                2 => {
                    let disp = self.next_word() as i16;
                    let base = self.addr + (self.words_read - 1) * 2;
                    let target = (base as i64 + disp as i64) as u32;
                    format!("(${target:X},pc)")
                }
                3 => {
                    let ext = self.next_word();
                    format_index_pc_ea(ext, self.addr + (self.words_read - 1) * 2)
                }
                4 => {
                    let imm = self.read_immediate(sz);
                    match sz {
                        EaSize::Byte | EaSize::Word => format!("#{imm}"),
                        EaSize::Long => format!("#{imm}"),
                    }
                }
                _ => "?".to_string(),
            },
            _ => "?".to_string(),
        }
    }

    fn illegal_word(&mut self, op: u16) -> String {
        format!("dc.w ${op:04X}")
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum EaSize {
    Byte,
    Word,
    Long,
}

fn size_suffix(sz: EaSize) -> &'static str {
    match sz {
        EaSize::Byte => "b",
        EaSize::Word => "w",
        EaSize::Long => "l",
    }
}

fn format_disp_an(disp: i16, an: u16) -> String {
    if disp == 0 {
        format!("(a{an})")
    } else {
        format!("(${:X},a{an})", disp)
    }
}

fn format_index_ea(ext: u16, an: usize) -> String {
    let disp = ext as i8;
    let ireg = (ext >> 12) & 7;
    let ilong = ext & 0x0800 != 0;
    let iaddr = ext & 0x8000 != 0;
    let ireg_str = if iaddr {
        format!("a{ireg}")
    } else {
        format!("d{ireg}")
    };
    let isz = if ilong { ".l" } else { ".w" };
    format!("(${disp:02X},a{an},{ireg_str}{isz})")
}

fn format_index_pc_ea(ext: u16, base: u32) -> String {
    let disp = ext as i8 as i32;
    let ireg = (ext >> 12) & 7;
    let ilong = ext & 0x0800 != 0;
    let iaddr = ext & 0x8000 != 0;
    let ireg_str = if iaddr {
        format!("a{ireg}")
    } else {
        format!("d{ireg}")
    };
    let isz = if ilong { ".l" } else { ".w" };
    let target = (base as i64 + disp as i64) as u32;
    format!("(${target:X},pc,{ireg_str}{isz})")
}

fn format_movem_mask(mask: u16) -> String {
    let mut parts = Vec::new();
    for i in 0..8 {
        if mask & (1 << i) != 0 {
            parts.push(format!("d{i}"));
        }
    }
    for i in 0..8 {
        if mask & (1 << (i + 8)) != 0 {
            parts.push(format!("a{i}"));
        }
    }
    if parts.is_empty() {
        "#0".to_string()
    } else {
        parts.join("/")
    }
}

#[derive(Clone, Copy)]
enum DbccStyle {
    Bcc,
    Scc,
    Dbcc,
}

/// Returns the condition-code suffix for a condition number.
///
/// For Bcc/BSR: condition 0 is `ra` (bra), condition 1 is `sr` (bsr).
/// For Scc: condition 0 is `t`, condition 1 is `f`.
/// For DBcc: condition 0 is `t`, condition 1 is `ra` (dbra, the alias for dbf).
fn cc_name(cond: u8, style: DbccStyle) -> &'static str {
    match style {
        DbccStyle::Bcc => match cond & 0xF {
            0x0 => "ra",
            0x1 => "sr",
            0x2 => "hi",
            0x3 => "ls",
            0x4 => "cc",
            0x5 => "cs",
            0x6 => "ne",
            0x7 => "eq",
            0x8 => "vc",
            0x9 => "vs",
            0xA => "pl",
            0xB => "mi",
            0xC => "ge",
            0xD => "lt",
            0xE => "gt",
            _ => "le",
        },
        DbccStyle::Scc => match cond & 0xF {
            0x0 => "t",
            0x1 => "f",
            0x2 => "hi",
            0x3 => "ls",
            0x4 => "cc",
            0x5 => "cs",
            0x6 => "ne",
            0x7 => "eq",
            0x8 => "vc",
            0x9 => "vs",
            0xA => "pl",
            0xB => "mi",
            0xC => "ge",
            0xD => "lt",
            0xE => "gt",
            _ => "le",
        },
        DbccStyle::Dbcc => match cond & 0xF {
            0x0 => "t",
            // DBcc condition 1: universally aliased as `dbra` in real 68000 source.
            // This disassembler uses `dbra` (the alias) rather than `dbf` — a
            // formatting decision, not a measured one. See task-13-report.md.
            0x1 => "ra",
            0x2 => "hi",
            0x3 => "ls",
            0x4 => "cc",
            0x5 => "cs",
            0x6 => "ne",
            0x7 => "eq",
            0x8 => "vc",
            0x9 => "vs",
            0xA => "pl",
            0xB => "mi",
            0xC => "ge",
            0xD => "lt",
            0xE => "gt",
            _ => "le",
        },
    }
}

fn shift_mnemonic(shift_type: u16, dir: u16) -> &'static str {
    match (shift_type & 3, dir & 1) {
        (0, 0) => "asr",
        (0, 1) => "asl",
        (1, 0) => "lsr",
        (1, 1) => "lsl",
        (2, 0) => "roxr",
        (2, 1) => "roxl",
        (3, 0) => "ror",
        _ => "rol",
    }
}

// ---------------------------------------------------------------------------
// Inline tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unused_mut)]
    fn dis(words: &[u16]) -> String {
        let mut w = words.to_vec();
        disassemble(|a| w.get((a / 2) as usize).copied().unwrap_or(0), 0).text
    }

    #[test]
    fn disassembles_common_forms() {
        assert_eq!(dis(&[0x4E71]), "nop");
        assert_eq!(dis(&[0x4E75]), "rts");
        assert_eq!(dis(&[0x7003]), "moveq #3,d0");
        assert_eq!(dis(&[0x3001]), "move.w d1,d0");
        assert_eq!(dis(&[0x2039, 0x0012, 0x3456]), "move.l $123456,d0");
        assert_eq!(dis(&[0x6000, 0x0010]), "bra $12");
        assert_eq!(dis(&[0xD041]), "add.w d1,d0");
    }

    #[test]
    #[allow(unused_mut, clippy::useless_vec)]
    fn reports_length_including_extension_words() {
        let mut w = vec![0x2039u16, 0x0012, 0x3456];
        let i = disassemble(|a| w.get((a / 2) as usize).copied().unwrap_or(0), 0);
        assert_eq!(i.len, 6);
    }

    #[test]
    fn never_panics_on_arbitrary_words() {
        // Every possible opcode must produce *something* without panicking.
        for op in 0u32..0x10000 {
            let w = [op as u16, 0x1234, 0x5678, 0x9ABC];
            let i = disassemble(|a| w.get((a / 2) as usize).copied().unwrap_or(0), 0);
            assert!(!i.text.is_empty());
            assert!(i.len >= 2);
        }
    }
}
