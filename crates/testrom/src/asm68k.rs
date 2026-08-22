//! Just enough 68000 assembler to write the demo's program.
//!
//! Not a general assembler: one method per instruction form the demo actually
//! uses, each emitting a hand-encoded opcode word. Anything more would be a
//! second project.
//!
//! # Why hand-encoded and then disassembled
//!
//! An emitter is only as good as its opcode constants, and a wrong constant
//! produces a *different valid instruction* rather than a crash — `move.w` where
//! `move.l` was meant runs fine and writes half the value. So every method here
//! is checked in `tests` by disassembling what it emitted with `m68k::disasm`
//! and comparing against the mnemonic text. That is an independent decoder: it
//! was written for the debugger, from the same manual, by a different path than
//! this file.
//!
//! # Labels
//!
//! Forward references are resolved by [`Asm::fixup`]: the emitter records where
//! a displacement word sits and what label it wants, and `finish` fills them in.
//! An unresolved label is a panic and not a zero displacement, because a zero
//! displacement is an infinite loop that looks like a hung emulator.

use std::collections::BTreeMap;

/// A 68000 data or address register number, 0-7.
pub type Reg = u16;

/// One pending forward reference: a word offset, and the label it needs.
#[derive(Debug, Clone)]
struct Fixup {
    /// Byte offset of the displacement word.
    at: usize,
    /// The label whose address goes there.
    label: String,
    /// The address the displacement is measured from — for a `Bcc`, the address
    /// of the instruction's *extension word*, which is the opcode address plus
    /// two.
    origin: usize,
}

/// An assembled 68000 program under construction.
#[derive(Debug, Default)]
pub struct Asm {
    code: Vec<u8>,
    labels: BTreeMap<String, usize>,
    fixups: Vec<Fixup>,
    /// Where in the final image `code` will be placed, so `label_addr` answers
    /// an absolute address rather than an offset.
    origin: u32,
}

impl Asm {
    /// A program that will be loaded at `origin`.
    pub fn new(origin: u32) -> Self {
        Self {
            code: Vec::new(),
            labels: BTreeMap::new(),
            fixups: Vec::new(),
            origin,
        }
    }

    /// The address the next emitted word will occupy.
    pub fn here(&self) -> u32 {
        self.origin + self.code.len() as u32
    }

    /// Records that `name` is at the current position.
    ///
    /// # Panics
    ///
    /// Panics on a duplicate label: two definitions mean one of the branches to
    /// it goes somewhere its author did not intend, and picking either silently
    /// is worse than stopping.
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
    /// Panics if the label is not defined yet. Callers use this for data
    /// addresses they have already emitted.
    pub fn label_addr(&self, name: &str) -> u32 {
        self.origin
            + *self
                .labels
                .get(name)
                .unwrap_or_else(|| panic!("no label `{name}`")) as u32
    }

    /// Emits one raw word.
    pub fn word(&mut self, w: u16) {
        self.code.extend_from_slice(&w.to_be_bytes());
    }

    /// Emits one raw longword.
    pub fn long(&mut self, l: u32) {
        self.code.extend_from_slice(&l.to_be_bytes());
    }

    // ---- Instructions -----------------------------------------------------
    //
    // Each comment gives the encoding as the manual writes it, because the
    // constant below is otherwise unreadable and unreviewable.

    /// `moveq #d,Dn` — `0111 rrr0 dddddddd`, `d` in −128..=127.
    pub fn moveq(&mut self, data: i8, dn: Reg) {
        self.word(0x7000 | (dn << 9) | (data as u8 as u16));
    }

    /// `move.w #imm,Dn` — `0011 rrr0 0011 1100`, immediate word follows.
    pub fn move_w_imm_dn(&mut self, imm: u16, dn: Reg) {
        self.word(0x303C | (dn << 9));
        self.word(imm);
    }

    /// `move.l #imm,Dn` — `0010 rrr0 0011 1100`, immediate long follows.
    pub fn move_l_imm_dn(&mut self, imm: u32, dn: Reg) {
        self.word(0x203C | (dn << 9));
        self.long(imm);
    }

    /// `movea.l #imm,An` — `0010 rrr0 0111 1100`.
    pub fn movea_l_imm_an(&mut self, imm: u32, an: Reg) {
        self.word(0x207C | (an << 9));
        self.long(imm);
    }

    /// `move.w Dn,(An)` — `0011 aaa0 1000 0rrr`.
    pub fn move_w_dn_ind(&mut self, dn: Reg, an: Reg) {
        self.word(0x3080 | (an << 9) | dn);
    }

    /// `move.w Dn,(An)+` — `0011 aaa0 1100 0rrr`.
    pub fn move_w_dn_postinc(&mut self, dn: Reg, an: Reg) {
        self.word(0x30C0 | (an << 9) | dn);
    }

    /// `move.w Dn,d(An)` — `0011 aaa1 0100 0rrr`, displacement word follows.
    pub fn move_w_dn_disp(&mut self, dn: Reg, an: Reg, disp: i16) {
        self.word(0x3140 | (an << 9) | dn);
        self.word(disp as u16);
    }

    /// `move.w Dn,(xxx).l` — `0011 0011 1100 0rrr`, absolute long follows.
    ///
    /// ⚠️ **Destination register 1, not 0.** A `MOVE`'s destination is mode in
    /// bits 8-6 and register in bits 11-9, and mode 7 selects between the two
    /// absolute forms by that register: 0 is absolute *short* — one extension
    /// word, sign-extended — and 1 is absolute long. `0x31C0` here instead of
    /// `0x33C0` assembles `move.w d0,$80` and leaves the low half of the address
    /// to be executed as the next opcode.
    pub fn move_w_dn_abs(&mut self, dn: Reg, addr: u32) {
        self.word(0x33C0 | dn);
        self.long(addr);
    }

    /// `move.w (xxx).l,Dn` — `0011 rrr0 0011 1001`.
    pub fn move_w_abs_dn(&mut self, addr: u32, dn: Reg) {
        self.word(0x3039 | (dn << 9));
        self.long(addr);
    }

    /// `move.b Dn,(xxx).l` — `0001 0011 1100 0rrr`. Register 1 for the same
    /// reason as [`Asm::move_w_dn_abs`].
    pub fn move_b_dn_abs(&mut self, dn: Reg, addr: u32) {
        self.word(0x13C0 | dn);
        self.long(addr);
    }

    /// `move.w Dm,Dn` — `0011 nnn0 0000 0mmm`.
    pub fn move_w_dn_dn(&mut self, from: Reg, to: Reg) {
        self.word(0x3000 | (to << 9) | from);
    }

    /// `addq.w #n,Dn` — `0101 nnn0 0100 0rrr`, `n` in 1..=8 encoded as 0 for 8.
    pub fn addq_w(&mut self, n: u16, dn: Reg) {
        assert!((1..=8).contains(&n), "addq counts run 1 to 8, not {n}");
        self.word(0x5040 | ((n & 7) << 9) | dn);
    }

    /// `add.w Dm,Dn` — `1101 nnn0 0100 0mmm`.
    pub fn add_w_dn_dn(&mut self, from: Reg, to: Reg) {
        self.word(0xD040 | (to << 9) | from);
    }

    /// `andi.w #imm,Dn` — `0000 0010 0100 0rrr`.
    pub fn andi_w(&mut self, imm: u16, dn: Reg) {
        self.word(0x0240 | dn);
        self.word(imm);
    }

    /// `lsr.w #n,Dn` — `1110 nnn0 0100 1rrr`, `n` in 1..=8 encoded as 0 for 8.
    pub fn lsr_w_imm(&mut self, n: u16, dn: Reg) {
        assert!((1..=8).contains(&n), "shift counts run 1 to 8, not {n}");
        self.word(0xE048 | ((n & 7) << 9) | dn);
    }

    /// `cmpi.w #imm,Dn` — `0000 1100 0100 0rrr`.
    pub fn cmpi_w(&mut self, imm: u16, dn: Reg) {
        self.word(0x0C40 | dn);
        self.word(imm);
    }

    /// `dbra Dn,label` — `0101 0001 1100 1rrr`, 16-bit displacement.
    pub fn dbra(&mut self, dn: Reg, label: &str) {
        self.word(0x51C8 | dn);
        self.branch_word(label);
    }

    /// `bra label`, 16-bit displacement form — `0110 0000 0000 0000`.
    pub fn bra(&mut self, label: &str) {
        self.word(0x6000);
        self.branch_word(label);
    }

    /// `bne label`, 16-bit displacement form.
    pub fn bne(&mut self, label: &str) {
        self.word(0x6600);
        self.branch_word(label);
    }

    /// `beq label`, 16-bit displacement form.
    pub fn beq(&mut self, label: &str) {
        self.word(0x6700);
        self.branch_word(label);
    }

    /// `jsr (xxx).l` — `0100 1110 1011 1001`.
    pub fn jsr_abs(&mut self, addr: u32) {
        self.word(0x4EB9);
        self.long(addr);
    }

    /// `jsr label`, resolved at `finish`.
    pub fn jsr(&mut self, label: &str) {
        self.word(0x4EB9);
        let at = self.code.len();
        self.long(0);
        self.fixups.push(Fixup {
            at,
            label: label.to_string(),
            // `usize::MAX` marks an absolute fixup: see `finish`.
            origin: usize::MAX,
        });
    }

    /// `rts` — `0100 1110 0111 0101`.
    pub fn rts(&mut self) {
        self.word(0x4E75);
    }

    /// `rte` — `0100 1110 0111 0011`.
    pub fn rte(&mut self) {
        self.word(0x4E73);
    }

    /// `nop` — `0100 1110 0111 0001`.
    pub fn nop(&mut self) {
        self.word(0x4E71);
    }

    /// `move.w #imm,SR` — `0100 0110 0111 1100`.
    pub fn move_to_sr(&mut self, imm: u16) {
        self.word(0x46FC);
        self.word(imm);
    }

    /// Emits a 16-bit branch displacement referring to `label`.
    fn branch_word(&mut self, label: &str) {
        let at = self.code.len();
        // The displacement is measured from the extension word's own address,
        // which is where we are about to write.
        self.word(0);
        self.fixups.push(Fixup {
            at,
            label: label.to_string(),
            origin: at,
        });
    }

    /// Resolves every fixup and returns the bytes.
    ///
    /// # Panics
    ///
    /// Panics on an undefined label, or on a branch too far for a 16-bit
    /// displacement. Both are silent wrong-jump bugs otherwise.
    pub fn finish(mut self) -> Vec<u8> {
        let fixups = std::mem::take(&mut self.fixups);
        for f in fixups {
            let target = *self
                .labels
                .get(&f.label)
                .unwrap_or_else(|| panic!("branch to undefined label `{}`", f.label));
            if f.origin == usize::MAX {
                let abs = self.origin + target as u32;
                self.code[f.at..f.at + 4].copy_from_slice(&abs.to_be_bytes());
                continue;
            }
            let disp = target as i64 - f.origin as i64;
            let disp = i16::try_from(disp)
                .unwrap_or_else(|_| panic!("branch to `{}` is {disp} bytes, too far", f.label));
            self.code[f.at..f.at + 2].copy_from_slice(&disp.to_be_bytes());
        }
        self.code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Disassembles the one instruction at the start of `code`.
    fn dis(code: &[u8]) -> String {
        let insn = m68k::disasm::disassemble(
            |addr| {
                let i = addr as usize;
                u16::from_be_bytes([code[i], code[i + 1]])
            },
            0,
        );
        assert_eq!(
            insn.len as usize,
            code.len(),
            "the disassembler read a different number of bytes than were emitted: {}",
            insn.text
        );
        insn.text
    }

    /// Assembles one instruction with `f` and disassembles it back.
    fn round(f: impl FnOnce(&mut Asm)) -> String {
        let mut a = Asm::new(0);
        f(&mut a);
        let code = a.finish();
        let insn = m68k::disasm::disassemble(
            |addr| {
                let i = addr as usize;
                u16::from_be_bytes([code[i], code[i + 1]])
            },
            0,
        );
        assert_eq!(
            insn.len as usize,
            code.len(),
            "emitted {} bytes, the disassembler read {}: {}",
            code.len(),
            insn.len,
            insn.text
        );
        insn.text
    }

    /// Every instruction form the demo uses disassembles to what it claims.
    ///
    /// This is the whole justification for hand-encoding: a wrong opcode
    /// constant is a *different valid instruction*, and the emulator would run
    /// it without complaint. `m68k::disasm` is an independently written decoder,
    /// so agreement between it and this table is real evidence.
    ///
    /// The expected strings are literals in MAME's syntax, which is what
    /// `disasm` produces.
    #[test]
    fn every_emitted_instruction_disassembles_to_its_own_mnemonic() {
        assert_eq!(round(|a| a.moveq(5, 3)), "moveq #5,d3");
        assert_eq!(round(|a| a.moveq(-1, 0)), "moveq #-1,d0");
        assert_eq!(round(|a| a.move_w_imm_dn(0x1234, 2)), "move.w #$1234,d2");
        assert_eq!(
            round(|a| a.move_l_imm_dn(0x1234_5678, 7)),
            "move.l #$12345678,d7"
        );
        assert_eq!(
            round(|a| a.movea_l_imm_an(0x90_0000, 1)),
            "movea.l #$00900000,a1"
        );
        assert_eq!(round(|a| a.move_w_dn_ind(2, 1)), "move.w d2,(a1)");
        assert_eq!(round(|a| a.move_w_dn_postinc(0, 3)), "move.w d0,(a3)+");
        assert_eq!(
            round(|a| a.move_w_dn_disp(4, 2, 0x10)),
            "move.w d4,($10,a2)"
        );
        assert_eq!(
            round(|a| a.move_w_dn_abs(1, 0x80_0100)),
            "move.w d1,$800100"
        );
        assert_eq!(
            round(|a| a.move_w_abs_dn(0xFF_0000, 5)),
            "move.w $FF0000,d5"
        );
        assert_eq!(
            round(|a| a.move_b_dn_abs(0, 0x80_0180)),
            "move.b d0,$800180"
        );
        assert_eq!(round(|a| a.move_w_dn_dn(6, 1)), "move.w d6,d1");
        assert_eq!(round(|a| a.addq_w(1, 0)), "addq.w #1,d0");
        assert_eq!(round(|a| a.addq_w(8, 2)), "addq.w #8,d2");
        assert_eq!(round(|a| a.add_w_dn_dn(1, 2)), "add.w d1,d2");
        assert_eq!(round(|a| a.andi_w(0x00FF, 3)), "andi.w #$00FF,d3");
        assert_eq!(round(|a| a.lsr_w_imm(4, 1)), "lsr.w #4,d1");
        assert_eq!(round(|a| a.lsr_w_imm(8, 1)), "lsr.w #8,d1");
        assert_eq!(round(|a| a.cmpi_w(0x0040, 0)), "cmpi.w #$0040,d0");
        assert_eq!(round(|a| a.jsr_abs(0x00_1000)), "jsr $1000");
        assert_eq!(round(|a| a.rts()), "rts");
        assert_eq!(round(|a| a.rte()), "rte");
        assert_eq!(round(|a| a.nop()), "nop");
        assert_eq!(round(|a| a.move_to_sr(0x2000)), "move #$2000,sr");
    }

    /// A backward branch resolves to the label's address.
    ///
    /// The displacement is measured from the extension word, so a loop of one
    /// `nop` branches by −4 and not −2 or −6. Every off-by-one here is an
    /// instruction boundary miss, which on a 68000 means executing an operand as
    /// an opcode.
    #[test]
    fn a_backward_branch_lands_on_its_label() {
        let mut a = Asm::new(0x1000);
        a.label("top");
        a.nop();
        a.bra("top");
        let code = a.finish();
        assert_eq!(code, vec![0x4E, 0x71, 0x60, 0x00, 0xFF, 0xFC]);
        assert_eq!(dis(&code[2..]), "bra $FFFFFFFE");
    }

    /// A forward branch resolves too, and `dbra` uses the same rule.
    #[test]
    fn a_forward_branch_and_a_dbra_resolve_to_the_same_place() {
        let mut a = Asm::new(0);
        a.bne("done");
        a.nop();
        a.label("done");
        a.rts();
        let code = a.finish();
        // The displacement word sits at offset 2 and `done` is at offset 6, so
        // the displacement is 4.
        assert_eq!(u16::from_be_bytes([code[2], code[3]]), 4);

        let mut a = Asm::new(0);
        a.label("loop");
        a.nop();
        a.dbra(0, "loop");
        let code = a.finish();
        assert_eq!(dis(&code[2..]), "dbra d0,$FFFFFFFE");
        // −4 for the same reason as `bra`.
        assert_eq!(i16::from_be_bytes([code[4], code[5]]), -4);
    }

    /// `jsr label` resolves to an absolute address including the origin.
    ///
    /// The origin is the trap: a program assembled for 0x1000 whose `jsr`
    /// resolved to the *offset* 0x20 would call into the vector table, and the
    /// first thing there is the reset SSP.
    #[test]
    fn a_jsr_to_a_label_is_absolute_and_includes_the_origin() {
        let mut a = Asm::new(0x40_0000);
        a.jsr("sub");
        a.rts();
        a.label("sub");
        a.rts();
        let code = a.finish();
        let target = u32::from_be_bytes([code[2], code[3], code[4], code[5]]);
        assert_eq!(target, 0x40_0008, "origin + the label's offset");
        assert_eq!(dis(&code[..6]), "jsr $400008");
    }

    /// `label_addr` answers an absolute address.
    #[test]
    fn label_addr_includes_the_origin() {
        let mut a = Asm::new(0x1234);
        a.nop();
        a.label("data");
        assert_eq!(a.label_addr("data"), 0x1236);
    }

    #[test]
    #[should_panic(expected = "branch to undefined label `nowhere`")]
    fn an_undefined_label_panics_rather_than_branching_to_itself() {
        let mut a = Asm::new(0);
        a.bra("nowhere");
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
    #[should_panic(expected = "addq counts run 1 to 8")]
    fn an_addq_of_nine_panics_rather_than_wrapping_to_one() {
        Asm::new(0).addq_w(9, 0);
    }
}
