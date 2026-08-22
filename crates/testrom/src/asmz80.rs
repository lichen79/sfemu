//! Just enough Z80 assembler to write the demo's sound driver.
//!
//! The same shape as [`crate::asm68k`] and for the same reason: one method per
//! instruction form the driver uses, hand-encoded, and every one checked in
//! `tests` by disassembling it back through `z80::disasm` and comparing the
//! mnemonic. A wrong opcode byte on a Z80 is a *different valid instruction* —
//! `0x3E` is `ld a,n` and `0x3D` is `dec a`, one bit apart, and the second one
//! leaves the immediate byte to be executed. Nothing about that looks like a
//! generator bug from the emulator's side.
//!
//! `z80::disasm` is an independent decoder — written for the debugger pane, and
//! itself checked against the core's own instruction lengths — so agreement
//! between it and this table is real evidence and not a decoder agreeing with
//! itself.
//!
//! # Two displacement forms
//!
//! `jr`/`djnz` take a signed byte measured from the *end* of the instruction;
//! `jp`/`call` take an absolute little-endian word. [`Asm::finish`] resolves
//! both, and panics rather than emitting a zero displacement for a label nobody
//! defined — a `jr` of 0 is a two-byte infinite loop that presents as a hung
//! sound chip.

use std::collections::BTreeMap;

/// An 8-bit operand, in the order every `r` field encodes them.
///
/// `MemHl` is slot 6 and is a memory operand, not a register — the Z80's `r`
/// field spends one of its eight slots on `(hl)`, which is why `ld (hl),(hl)`
/// has no encoding and slot 6 twice is `halt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg8 {
    /// `b`
    B = 0,
    /// `c`
    C = 1,
    /// `d`
    D = 2,
    /// `e`
    E = 3,
    /// `h`
    H = 4,
    /// `l`
    L = 5,
    /// `(hl)`
    MemHl = 6,
    /// `a`
    A = 7,
}

/// A 16-bit pair as the `ld rr,nn` and `inc rr` families encode them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pair {
    /// `bc`
    Bc = 0,
    /// `de`
    De = 1,
    /// `hl`
    Hl = 2,
    /// `sp`
    Sp = 3,
}

/// A 16-bit pair as `push` and `pop` encode them.
///
/// A separate type from [`Pair`] because slot 3 is a *different register*: `af`
/// here, `sp` there. One shared enum would make `push sp` spell `push af`, and
/// the program would run — with the flags where the stack pointer belonged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stack {
    /// `bc`
    Bc = 0,
    /// `de`
    De = 1,
    /// `hl`
    Hl = 2,
    /// `af`
    Af = 3,
}

/// A branch condition, in encoding order.
///
/// The first four are the only ones `jr` can take: the relative-jump block is
/// four opcodes wide, and `0x20 | (cond << 3)` for `Po` collides with `ld hl,nn`.
/// [`Asm::jr_cc`] asserts rather than emitting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    /// not zero
    Nz = 0,
    /// zero
    Z = 1,
    /// no carry
    Nc = 2,
    /// carry
    C = 3,
    /// parity odd
    Po = 4,
    /// parity even
    Pe = 5,
    /// sign positive
    P = 6,
    /// sign negative (minus)
    M = 7,
}

/// One of the eight 8-bit ALU operations, in encoding order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alu {
    /// `add a,`
    Add = 0,
    /// `adc a,`
    Adc = 1,
    /// `sub`
    Sub = 2,
    /// `sbc a,`
    Sbc = 3,
    /// `and`
    And = 4,
    /// `xor`
    Xor = 5,
    /// `or`
    Or = 6,
    /// `cp`
    Cp = 7,
}

/// What kind of hole a fixup fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A signed byte, measured from the end of the instruction.
    Rel8,
    /// An absolute little-endian word, including the origin.
    Abs16,
}

/// One pending reference to a label.
#[derive(Debug, Clone)]
struct Fixup {
    /// Byte offset of the hole.
    at: usize,
    /// The label whose address goes there.
    label: String,
    kind: Kind,
}

/// An assembled Z80 program under construction.
#[derive(Debug, Default)]
pub struct Asm {
    code: Vec<u8>,
    labels: BTreeMap<String, usize>,
    fixups: Vec<Fixup>,
    /// Where in the Z80's address space `code` will sit, so `jp` to a label is an
    /// address and not an offset.
    origin: u16,
}

impl Asm {
    /// A program that will be loaded at `origin`.
    pub fn new(origin: u16) -> Self {
        Self {
            code: Vec::new(),
            labels: BTreeMap::new(),
            fixups: Vec::new(),
            origin,
        }
    }

    /// The address the next emitted byte will occupy.
    pub fn here(&self) -> u16 {
        self.origin.wrapping_add(self.code.len() as u16)
    }

    /// Records that `name` is at the current position.
    ///
    /// # Panics
    ///
    /// Panics on a duplicate label, for the reason
    /// [`crate::asm68k::Asm::label`] gives.
    pub fn label(&mut self, name: &str) {
        let at = self.code.len();
        assert!(
            self.labels.insert(name.to_string(), at).is_none(),
            "label `{name}` is defined twice"
        );
    }

    /// The absolute address of a defined label.
    ///
    /// # Panics
    ///
    /// Panics if the label is not defined yet.
    pub fn label_addr(&self, name: &str) -> u16 {
        self.origin.wrapping_add(
            *self
                .labels
                .get(name)
                .unwrap_or_else(|| panic!("no label `{name}`")) as u16,
        )
    }

    /// Emits one raw byte.
    pub fn byte(&mut self, b: u8) {
        self.code.push(b);
    }

    /// Emits one raw word, little-endian as every Z80 operand is.
    pub fn word(&mut self, w: u16) {
        self.code.extend_from_slice(&w.to_le_bytes());
    }

    // ---- Instructions -----------------------------------------------------
    //
    // Each doc comment gives the encoding in the bit form the manual uses,
    // because the constant on the line below is otherwise unreviewable.

    /// `nop` — `0x00`.
    pub fn nop(&mut self) {
        self.byte(0x00);
    }

    /// `di` — `0xF3`.
    pub fn di(&mut self) {
        self.byte(0xF3);
    }

    /// `ei` — `0xFB`.
    pub fn ei(&mut self) {
        self.byte(0xFB);
    }

    /// `halt` — `0x76`.
    pub fn halt(&mut self) {
        self.byte(0x76);
    }

    /// `ret` — `0xC9`.
    pub fn ret(&mut self) {
        self.byte(0xC9);
    }

    /// `reti` — `0xED 0x4D`.
    ///
    /// Not interchangeable with `ret` even though the CPS-1 sound board has no
    /// daisy chain to notify: the vector suite distinguishes them and a driver
    /// that returned from a maskable interrupt with plain `ret` would be one
    /// byte shorter and one behaviour off.
    pub fn reti(&mut self) {
        self.byte(0xED);
        self.byte(0x4D);
    }

    /// `im 1` — `0xED 0x56`.
    pub fn im1(&mut self) {
        self.byte(0xED);
        self.byte(0x56);
    }

    /// `ld r,n` — `00 rrr 110`, immediate byte follows.
    pub fn ld_r_imm(&mut self, r: Reg8, n: u8) {
        self.byte(0x06 | ((r as u8) << 3));
        self.byte(n);
    }

    /// `ld r,r'` — `01 ddd sss`.
    ///
    /// # Panics
    ///
    /// Panics on `ld (hl),(hl)`: that encoding is `halt`, and silently emitting
    /// it would stop the sound CPU where a copy was meant.
    pub fn ld_r_r(&mut self, dst: Reg8, src: Reg8) {
        assert!(
            !(dst == Reg8::MemHl && src == Reg8::MemHl),
            "`ld (hl),(hl)` has no encoding — that opcode is `halt`"
        );
        self.byte(0x40 | ((dst as u8) << 3) | (src as u8));
    }

    /// `ld rr,nn` — `00 rr0 001`, immediate word follows.
    pub fn ld_pair_imm(&mut self, rr: Pair, nn: u16) {
        self.byte(0x01 | ((rr as u8) << 4));
        self.word(nn);
    }

    /// `ld (nn),a` — `0x32`, address follows.
    pub fn ld_abs_a(&mut self, addr: u16) {
        self.byte(0x32);
        self.word(addr);
    }

    /// `ld a,(nn)` — `0x3A`, address follows.
    pub fn ld_a_abs(&mut self, addr: u16) {
        self.byte(0x3A);
        self.word(addr);
    }

    /// `inc r` — `00 rrr 100`.
    pub fn inc_r(&mut self, r: Reg8) {
        self.byte(0x04 | ((r as u8) << 3));
    }

    /// `dec r` — `00 rrr 101`.
    pub fn dec_r(&mut self, r: Reg8) {
        self.byte(0x05 | ((r as u8) << 3));
    }

    /// `inc rr` — `00 rr0 011`.
    pub fn inc_pair(&mut self, rr: Pair) {
        self.byte(0x03 | ((rr as u8) << 4));
    }

    /// `dec rr` — `00 rr1 011`.
    pub fn dec_pair(&mut self, rr: Pair) {
        self.byte(0x0B | ((rr as u8) << 4));
    }

    /// An ALU operation against an immediate — `11 ooo 110`, byte follows.
    pub fn alu_imm(&mut self, op: Alu, n: u8) {
        self.byte(0xC6 | ((op as u8) << 3));
        self.byte(n);
    }

    /// An ALU operation against a register or `(hl)` — `10 ooo rrr`.
    pub fn alu_r(&mut self, op: Alu, r: Reg8) {
        self.byte(0x80 | ((op as u8) << 3) | (r as u8));
    }

    /// `push rr` — `11 rr0 101`.
    pub fn push(&mut self, rr: Stack) {
        self.byte(0xC5 | ((rr as u8) << 4));
    }

    /// `pop rr` — `11 rr0 001`.
    pub fn pop(&mut self, rr: Stack) {
        self.byte(0xC1 | ((rr as u8) << 4));
    }

    /// `jp nn` — `0xC3`, resolved at `finish`.
    pub fn jp(&mut self, label: &str) {
        self.byte(0xC3);
        self.abs_word(label);
    }

    /// `jp cc,nn` — `11 ccc 010`.
    pub fn jp_cc(&mut self, cc: Cond, label: &str) {
        self.byte(0xC2 | ((cc as u8) << 3));
        self.abs_word(label);
    }

    /// `call nn` — `0xCD`.
    pub fn call(&mut self, label: &str) {
        self.byte(0xCD);
        self.abs_word(label);
    }

    /// `jr d` — `0x18`, signed displacement byte.
    pub fn jr(&mut self, label: &str) {
        self.byte(0x18);
        self.rel_byte(label);
    }

    /// `jr cc,d` — `001 cc 000`, and only the first four conditions exist.
    ///
    /// # Panics
    ///
    /// Panics on `Po` and above. `0x20 | (Po << 3)` is `0x40` — `ld b,b` — so
    /// the assert is the difference between a diagnostic and a driver that
    /// falls through every branch it thought it took.
    pub fn jr_cc(&mut self, cc: Cond, label: &str) {
        assert!(
            matches!(cc, Cond::Nz | Cond::Z | Cond::Nc | Cond::C),
            "`jr` has no {cc:?} form — only nz, z, nc and c"
        );
        self.byte(0x20 | ((cc as u8) << 3));
        self.rel_byte(label);
    }

    /// `djnz d` — `0x10`, signed displacement byte.
    pub fn djnz(&mut self, label: &str) {
        self.byte(0x10);
        self.rel_byte(label);
    }

    /// `rst n` — `11 nnn 111`, for `n` a multiple of 8 below 0x40.
    ///
    /// # Panics
    ///
    /// Panics on an address that is not one of the eight, because the encoding
    /// has no room for it and masking would silently pick a neighbour.
    pub fn rst(&mut self, addr: u8) {
        assert!(
            addr < 0x40 && addr.is_multiple_of(8),
            "rst targets are $00 to $38 in steps of 8, not ${addr:02x}"
        );
        self.byte(0xC7 | addr);
    }

    /// Reserves a two-byte absolute address referring to `label`.
    fn abs_word(&mut self, label: &str) {
        let at = self.code.len();
        self.word(0);
        self.fixups.push(Fixup {
            at,
            label: label.to_string(),
            kind: Kind::Abs16,
        });
    }

    /// Reserves a one-byte signed displacement referring to `label`.
    fn rel_byte(&mut self, label: &str) {
        let at = self.code.len();
        self.byte(0);
        self.fixups.push(Fixup {
            at,
            label: label.to_string(),
            kind: Kind::Rel8,
        });
    }

    /// Resolves every fixup and returns the bytes.
    ///
    /// # Panics
    ///
    /// Panics on an undefined label, or on a `jr`/`djnz` past the reach of a
    /// signed byte. Both are silent wrong-jump bugs otherwise, and a Z80 that
    /// jumps into the middle of an instruction produces sound, just not the
    /// sound anyone wrote.
    pub fn finish(mut self) -> Vec<u8> {
        let fixups = std::mem::take(&mut self.fixups);
        for f in fixups {
            let target = *self
                .labels
                .get(&f.label)
                .unwrap_or_else(|| panic!("branch to undefined label `{}`", f.label));
            match f.kind {
                Kind::Abs16 => {
                    let abs = self.origin.wrapping_add(target as u16);
                    self.code[f.at..f.at + 2].copy_from_slice(&abs.to_le_bytes());
                }
                Kind::Rel8 => {
                    // Measured from the end of the instruction, which is one
                    // past the displacement byte itself.
                    let disp = target as i64 - (f.at as i64 + 1);
                    let disp = i8::try_from(disp).unwrap_or_else(|_| {
                        panic!(
                            "`jr` to `{}` is {disp} bytes, too far for one byte",
                            f.label
                        )
                    });
                    self.code[f.at] = disp as u8;
                }
            }
        }
        self.code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assembles with `f` at `origin` and disassembles the first instruction.
    ///
    /// Asserts the decoder consumed exactly what was emitted: a length mismatch
    /// means the byte after this instruction is about to be read as an operand,
    /// or an operand read as an opcode, which is the failure mode this whole
    /// module exists to rule out.
    fn round_at(origin: u16, f: impl FnOnce(&mut Asm)) -> String {
        let mut a = Asm::new(origin);
        f(&mut a);
        let code = a.finish();
        let (text, len) = z80::disasm::disasm(|addr| code[usize::from(addr - origin)], origin);
        assert_eq!(
            usize::from(len),
            code.len(),
            "emitted {} bytes, the disassembler read {len}: {}",
            code.len(),
            text.as_str()
        );
        text.as_str().to_string()
    }

    /// [`round_at`] with the origin at 0, which is where the sound ROM starts.
    fn round(f: impl FnOnce(&mut Asm)) -> String {
        round_at(0, f)
    }

    /// Every instruction form the driver uses disassembles to what it claims.
    ///
    /// The justification for hand-encoding: `ld a,n` and `dec a` are one bit
    /// apart, and the emulator would run either without complaint. The expected
    /// strings are literals in `z80::disasm`'s spelling — lower case, `$` hex at
    /// the operand's width.
    #[test]
    fn every_emitted_instruction_disassembles_to_its_own_mnemonic() {
        assert_eq!(round(|a| a.nop()), "nop");
        assert_eq!(round(|a| a.di()), "di");
        assert_eq!(round(|a| a.ei()), "ei");
        assert_eq!(round(|a| a.halt()), "halt");
        assert_eq!(round(|a| a.ret()), "ret");
        assert_eq!(round(|a| a.reti()), "reti");
        assert_eq!(round(|a| a.im1()), "im 1");

        assert_eq!(round(|a| a.ld_r_imm(Reg8::A, 0x2A)), "ld a,$2a");
        assert_eq!(round(|a| a.ld_r_imm(Reg8::B, 0x08)), "ld b,$08");
        assert_eq!(round(|a| a.ld_r_imm(Reg8::MemHl, 0xFF)), "ld (hl),$ff");
        assert_eq!(round(|a| a.ld_r_r(Reg8::A, Reg8::B)), "ld a,b");
        assert_eq!(round(|a| a.ld_r_r(Reg8::MemHl, Reg8::A)), "ld (hl),a");
        assert_eq!(round(|a| a.ld_r_r(Reg8::E, Reg8::MemHl)), "ld e,(hl)");
        assert_eq!(round(|a| a.ld_r_r(Reg8::L, Reg8::H)), "ld l,h");

        assert_eq!(round(|a| a.ld_pair_imm(Pair::Sp, 0xD7FF)), "ld sp,$d7ff");
        assert_eq!(round(|a| a.ld_pair_imm(Pair::Hl, 0xF000)), "ld hl,$f000");
        assert_eq!(round(|a| a.ld_pair_imm(Pair::Bc, 0x0100)), "ld bc,$0100");
        assert_eq!(round(|a| a.ld_pair_imm(Pair::De, 0x8000)), "ld de,$8000");
        assert_eq!(round(|a| a.ld_abs_a(0xF002)), "ld ($f002),a");
        assert_eq!(round(|a| a.ld_a_abs(0xF008)), "ld a,($f008)");

        assert_eq!(round(|a| a.inc_r(Reg8::A)), "inc a");
        assert_eq!(round(|a| a.dec_r(Reg8::B)), "dec b");
        assert_eq!(round(|a| a.inc_r(Reg8::MemHl)), "inc (hl)");
        assert_eq!(round(|a| a.inc_pair(Pair::Hl)), "inc hl");
        assert_eq!(round(|a| a.dec_pair(Pair::De)), "dec de");

        assert_eq!(round(|a| a.alu_imm(Alu::Add, 0x10)), "add a,$10");
        assert_eq!(round(|a| a.alu_imm(Alu::Adc, 0x10)), "adc a,$10");
        assert_eq!(round(|a| a.alu_imm(Alu::Sub, 0x01)), "sub $01");
        assert_eq!(round(|a| a.alu_imm(Alu::Sbc, 0x01)), "sbc a,$01");
        assert_eq!(round(|a| a.alu_imm(Alu::And, 0x7F)), "and $7f");
        assert_eq!(round(|a| a.alu_imm(Alu::Xor, 0xFF)), "xor $ff");
        assert_eq!(round(|a| a.alu_imm(Alu::Or, 0x80)), "or $80");
        assert_eq!(round(|a| a.alu_imm(Alu::Cp, 0x0A)), "cp $0a");
        assert_eq!(round(|a| a.alu_r(Alu::Or, Reg8::A)), "or a");
        assert_eq!(round(|a| a.alu_r(Alu::Cp, Reg8::MemHl)), "cp (hl)");
        assert_eq!(round(|a| a.alu_r(Alu::Add, Reg8::C)), "add a,c");

        assert_eq!(round(|a| a.push(Stack::Af)), "push af");
        assert_eq!(round(|a| a.push(Stack::Hl)), "push hl");
        assert_eq!(round(|a| a.pop(Stack::Af)), "pop af");
        assert_eq!(round(|a| a.pop(Stack::Bc)), "pop bc");

        assert_eq!(round(|a| a.rst(0x38)), "rst $38");
        assert_eq!(round(|a| a.rst(0x00)), "rst $00");
    }

    /// `jp`, `jp cc` and `call` resolve to an absolute address that includes the
    /// origin.
    ///
    /// The origin is the trap: a driver assembled for 0x8000 whose `jp`
    /// resolved to the *offset* 0x10 would jump into the fixed ROM's reset
    /// vector.
    #[test]
    fn absolute_jumps_include_the_origin() {
        let target = |f: fn(&mut Asm, &str)| -> u16 {
            let mut a = Asm::new(0x8000);
            f(&mut a, "there");
            a.nop();
            a.label("there");
            a.ret();
            let code = a.finish();
            u16::from_le_bytes([code[1], code[2]])
        };
        // Three bytes of jump plus one `nop`, so the label is at offset 4.
        assert_eq!(target(|a, l| a.jp(l)), 0x8004);
        assert_eq!(target(|a, l| a.jp_cc(Cond::Nz, l)), 0x8004);
        assert_eq!(target(|a, l| a.call(l)), 0x8004);

        // And the text, which is where a swapped-endian word would show: a `jp`
        // to its own address assembles to the origin, and `$0080` rather than
        // `$8000` is exactly what a big-endian `word` would print.
        assert_eq!(
            round_at(0x8000, |a| {
                a.label("top");
                a.jp("top");
            }),
            "jp $8000"
        );
    }

    /// A backward `jr` and `djnz` resolve to the label, measured from the end of
    /// the instruction.
    ///
    /// A displacement measured from the instruction's *start* instead is off by
    /// two, which on a Z80 means landing inside the previous instruction. The
    /// literal byte is asserted because the disassembler's resolved target would
    /// agree with a compensating error in both.
    #[test]
    fn a_backward_jr_lands_on_its_label() {
        let mut a = Asm::new(0x0100);
        a.label("top");
        a.nop();
        a.jr("top");
        let code = a.finish();
        // `nop`, then `jr` with a displacement of −3: the instruction ends at
        // offset 3 and the label is at 0.
        assert_eq!(code, vec![0x00, 0x18, 0xFD]);
        // And the decoder resolves it back to the label's address, which is what
        // catches a displacement written from the right base with the wrong sign.
        let (text, len) = z80::disasm::disasm(|addr| code[usize::from(addr - 0x0100)], 0x0101);
        assert_eq!(len, 2);
        assert_eq!(text.as_str(), "jr $0100");

        let mut a = Asm::new(0x0100);
        a.label("loop");
        a.nop();
        a.djnz("loop");
        assert_eq!(a.finish(), vec![0x00, 0x10, 0xFD]);
    }

    /// A forward `jr cc` resolves too, and the disassembler agrees on the target.
    #[test]
    fn a_forward_jr_cc_resolves_to_the_label() {
        let mut a = Asm::new(0x0200);
        a.jr_cc(Cond::Z, "done");
        a.nop();
        a.nop();
        a.label("done");
        a.ret();
        let code = a.finish();
        // The instruction ends at offset 2 and `done` is at offset 4.
        assert_eq!(code[..2], [0x28, 0x02]);
        let (text, len) = z80::disasm::disasm(|addr| code[usize::from(addr - 0x200)], 0x0200);
        assert_eq!(len, 2);
        assert_eq!(text.as_str(), "jr z,$0204");
    }

    /// A `jr` beyond a signed byte panics rather than wrapping.
    #[test]
    #[should_panic(expected = "too far for one byte")]
    fn a_jr_past_the_reach_of_a_byte_panics() {
        let mut a = Asm::new(0);
        a.jr("far");
        for _ in 0..200 {
            a.nop();
        }
        a.label("far");
        let _ = a.finish();
    }

    #[test]
    #[should_panic(expected = "branch to undefined label `nowhere`")]
    fn an_undefined_label_panics_rather_than_jumping_to_itself() {
        let mut a = Asm::new(0);
        a.jp("nowhere");
        let _ = a.finish();
    }

    #[test]
    #[should_panic(expected = "defined twice")]
    fn a_duplicate_label_panics() {
        let mut a = Asm::new(0);
        a.label("x");
        a.label("x");
    }

    #[test]
    #[should_panic(expected = "only nz, z, nc and c")]
    fn a_jr_on_a_parity_condition_panics_rather_than_emitting_ld_b_b() {
        let mut a = Asm::new(0);
        a.jr_cc(Cond::Pe, "somewhere");
    }

    #[test]
    #[should_panic(expected = "that opcode is `halt`")]
    fn ld_memhl_memhl_panics_rather_than_halting() {
        Asm::new(0).ld_r_r(Reg8::MemHl, Reg8::MemHl);
    }

    #[test]
    #[should_panic(expected = "rst targets are")]
    fn an_unaligned_rst_panics() {
        Asm::new(0).rst(0x30 + 1);
    }

    /// `label_addr` and `here` answer absolute addresses.
    #[test]
    fn addresses_include_the_origin() {
        let mut a = Asm::new(0x1234);
        assert_eq!(a.here(), 0x1234);
        a.nop();
        a.label("data");
        assert_eq!(a.label_addr("data"), 0x1235);
        assert_eq!(a.here(), 0x1235);
    }
}
