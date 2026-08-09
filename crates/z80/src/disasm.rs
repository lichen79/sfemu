//! One-instruction disassembly, for D2's debugger pane.
//!
//! # The property that matters
//!
//! Not the spelling — **the reported length must equal what [`Z80::step`]
//! consumes**. A pane whose disassembler is one byte short does not show one wrong
//! line; it shows one wrong line and then garbage forever, because every following
//! line starts inside the previous instruction.
//!
//! So the central test is not a table of expected lengths, which would be a second
//! guess capable of agreeing with a wrong first one. It is
//! `every_opcode_reports_the_length_the_core_consumes`, which runs all 256 opcodes
//! of each of the five pages through both this file and the core and compares —
//! 1,280 instructions, with the core as the authority.
//!
//! # No allocator
//!
//! [`Text`] is a fixed buffer rather than a `String`: `alloc` is not available here
//! and adding it for a debugger convenience would put an allocator requirement on
//! the WASM build. It truncates rather than panicking, because a debugger pane must
//! never take the emulator down.
//!
//! # Spelling conventions
//!
//! The vector suite carries no text, so it can say what an opcode *does* but not
//! how to spell it. These are house convention, chosen to match `m68k::disasm`:
//! lower case throughout, `$` hex with the operand's width (`$42`, `$1234`), and
//! **displacements signed and decimal** — `(ix-5)`, never `(ix+251)` and never
//! `(ix+fb)`. That last one is the only correctness claim in the list: `(ix+251)`
//! names a different address, and doing the two's-complement conversion by hand is
//! what a reader opened the pane to avoid.
//!
//! Two spellings say something the mnemonic alone cannot:
//!
//! - A relative jump prints its **resolved target** (`jr $0107`), computed from the
//!   end of the instruction. The classic error is measuring from its start.
//! - An index prefix that changes nothing prints as `[dd] nop`. The prefix was
//!   fetched and charged 4 T-states, and a pane that showed a bare `nop` would
//!   under-count the line by a byte — which is the whole failure this module is
//!   built to avoid.

use crate::Bus;
#[cfg(doc)]
use crate::Z80;
use core::fmt::Write as _;

/// A fixed-capacity string, so the disassembler needs no allocator.
///
/// 32 bytes is over twice the longest text this module produces (`[dd] ex (sp),hl`
/// is 15).
#[derive(Clone, Copy)]
pub struct Text {
    buf: [u8; Self::CAP],
    len: usize,
}

impl Text {
    /// The buffer's size in bytes. Text beyond it is dropped.
    pub const CAP: usize = 32;

    /// An empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: [0; Self::CAP],
            len: 0,
        }
    }

    /// Appends, **truncating** if full.
    ///
    /// A debugger pane must never take the emulator down, and the alternative to
    /// truncation is a panic on a path no test would otherwise reach.
    ///
    /// Truncation is byte-wise, so a multi-byte UTF-8 sequence could in principle be
    /// cut in half — [`Self::as_str`] handles that by returning a marker rather than
    /// panicking. Everything this module writes is ASCII, so neither path is reached
    /// in practice, which is exactly why both are tested directly.
    pub fn push_str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            if self.len == Self::CAP {
                return;
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
    }

    /// The text so far.
    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("<bad utf8>")
    }
}

impl Default for Text {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Display for Text {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl core::fmt::Debug for Text {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The string, not the 32-byte array with its tail of zeros: an
        // `assert_eq!` on a `Text` should print something a reader can act on.
        core::fmt::Debug::fmt(self.as_str(), f)
    }
}

/// Writes through [`Text::push_str`], so `write!` works and cannot fail.
///
/// This is what makes every number in the module go through `core`'s formatter
/// rather than a hand-rolled hex or decimal routine — `no_std` includes
/// `core::fmt::Write`, so there was never a reason to write one.
impl core::fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.push_str(s);
        Ok(())
    }
}

/// A read cursor over the instruction's bytes, counting how many it has taken.
///
/// A struct rather than a closure: `disasm` needs to append to a `Text` and advance
/// the cursor in the same expression, and a closure capturing the counter would hold
/// a borrow that conflicts with the one on the text.
struct Cur<F> {
    read: F,
    pc: u16,
    n: u16,
}

impl<F: FnMut(u16) -> u8> Cur<F> {
    fn byte(&mut self) -> u8 {
        let b = (self.read)(self.pc.wrapping_add(self.n));
        self.n = self.n.wrapping_add(1);
        b
    }

    fn word(&mut self) -> u16 {
        let lo = self.byte();
        let hi = self.byte();
        u16::from(hi) << 8 | u16::from(lo)
    }
}

/// `b c d e h l (hl) a` — the same order and the same names as
/// [`crate::decode::reg`].
const R8: [&str; 8] = ["b", "c", "d", "e", "h", "l", "(hl)", "a"];
/// The eight ALU operations in encoding order, spelled with their destination.
///
/// `add a,` rather than `add`: the Z80's 8-bit ALU always targets `A`, and the
/// two-operand spelling is what an assembler accepts back.
const ALU: [&str; 8] = [
    "add a,", "adc a,", "sub ", "sbc a,", "and ", "xor ", "or ", "cp ",
];
/// The eight conditions in encoding order, matching [`crate::ops::flow::cond`].
const CC: [&str; 8] = ["nz", "z", "nc", "c", "po", "pe", "p", "m"];
/// The eight `CB`-page operations, matching [`crate::ops::bits::RotOp`]'s order.
///
/// `sll` is included and is not a Zilog mnemonic: the instruction is undocumented,
/// real, and shipped as sixteen vector files, so a pane that hid it would be lying
/// about the byte it just skipped.
const ROT: [&str; 8] = ["rlc", "rrc", "rl", "rr", "sla", "sra", "sll", "srl"];
/// The 16-bit pairs as the `LD rr,nn` family encodes them: `sp` in slot 3.
const RP: [&str; 4] = ["bc", "de", "hl", "sp"];
/// The 16-bit pairs as `PUSH`/`POP` encode them: `af` in slot 3, not `sp`.
const PP: [&str; 4] = ["bc", "de", "hl", "af"];

/// Disassembles one instruction at `pc`.
///
/// Returns the text and the instruction's **length in bytes**, which is the half
/// that matters: see the module docs.
///
/// `read` is a closure rather than a `&mut impl Bus` so the debugger can render a
/// listing without a mutable borrow of the machine, and without triggering the side
/// effects some bus reads have — a pane that advanced a hardware counter merely by
/// being displayed would be a debugger that changes what it is debugging.
///
/// Never panics, whatever the bytes. Every encoding on every page resolves to some
/// text; the undefined `ED` opcodes render as `db $ed,$xx`, which is both honest and
/// the right length.
pub fn disasm(read: impl FnMut(u16) -> u8, pc: u16) -> (Text, u16) {
    let mut c = Cur { read, pc, n: 0 };
    let mut t = Text::new();
    let op = c.byte();
    base(&mut t, &mut c, op);
    (t, c.n)
}

/// Disassembles the instruction `bus` holds at `pc`, for callers that have a bus.
///
/// A convenience over [`disasm`], and deliberately `&mut`: reading through [`Bus`]
/// can have side effects, so a caller that wants none must use `disasm` with a
/// closure over its own memory image. Naming that in the signature is better than
/// hiding it behind a shared reference this could not honestly take.
pub fn disasm_bus<B: Bus>(bus: &mut B, pc: u16) -> (Text, u16) {
    // The closure borrows `bus` for the call's duration only, which is why this can
    // be a thin wrapper rather than a duplicate of the dispatch.
    disasm(|a| bus.read(a), pc)
}

/// Appends the resolved target of a relative jump.
///
/// The displacement is relative to the **end** of the instruction, and `c.n` is
/// already that end because the displacement was the last byte read. Measuring from
/// the start instead is the classic error and would put every `jr` two bytes off —
/// or three, behind a prefix, which is why this reads the cursor rather than taking
/// a hardcoded 2.
fn push_target<F: FnMut(u16) -> u8>(t: &mut Text, c: &Cur<F>, d: i8) {
    let end = c.pc.wrapping_add(c.n);
    let _ = write!(t, "${:04x}", end.wrapping_add(d as u16));
}

/// Appends an index-register operand with its signed displacement: `(ix-5)`.
fn push_disp(t: &mut Text, idx: &str, d: i8) {
    let sign = if d < 0 { '-' } else { '+' };
    let _ = write!(t, "({idx}{sign}{})", d.unsigned_abs());
}

/// Appends register `n` with `h`/`l` rewritten to the index halves.
///
/// Index 6 never reaches here: every displaced form is handled by its own branch,
/// because it must read the displacement byte first. Mirrors the `ireg!` macro in
/// [`crate::decode::index_page`], and the two must agree — a divergence would mean
/// the pane naming a register the core does not touch.
fn push_ireg(t: &mut Text, idx: &str, n: u8) {
    match n {
        4 => {
            t.push_str(idx);
            t.push_str("h");
        }
        5 => {
            t.push_str(idx);
            t.push_str("l");
        }
        n => t.push_str(R8[n as usize]),
    }
}

/// The base page. Mirrors [`crate::decode::execute`]'s arm order, deliberately: a
/// divergence between the two files is visible only if they can be read side by side.
fn base<F: FnMut(u16) -> u8>(t: &mut Text, c: &mut Cur<F>, op: u8) {
    match op {
        0x00 => t.push_str("nop"),
        0x07 => t.push_str("rlca"),
        0x08 => t.push_str("ex af,af'"),
        0x0F => t.push_str("rrca"),
        0x17 => t.push_str("rla"),
        0x1F => t.push_str("rra"),
        0x27 => t.push_str("daa"),
        0x2F => t.push_str("cpl"),
        0x37 => t.push_str("scf"),
        0x3F => t.push_str("ccf"),
        // Before the 0x40..=0x7F block, which is how that arm needs no guard and the
        // `match` stays exhaustive by construction — as in `decode::execute`.
        0x76 => t.push_str("halt"),
        0xC9 => t.push_str("ret"),
        0xD9 => t.push_str("exx"),
        0xE3 => t.push_str("ex (sp),hl"),
        0xE9 => t.push_str("jp (hl)"),
        0xEB => t.push_str("ex de,hl"),
        0xF3 => t.push_str("di"),
        0xF9 => t.push_str("ld sp,hl"),
        0xFB => t.push_str("ei"),
        0x10 => {
            let d = c.byte() as i8;
            t.push_str("djnz ");
            push_target(t, c, d);
        }
        0x18 => {
            let d = c.byte() as i8;
            t.push_str("jr ");
            push_target(t, c, d);
        }
        0x20 | 0x28 | 0x30 | 0x38 => {
            let d = c.byte() as i8;
            let _ = write!(t, "jr {},", CC[usize::from((op >> 3) & 3)]);
            push_target(t, c, d);
        }
        0x01 | 0x11 | 0x21 | 0x31 => {
            let nn = c.word();
            let _ = write!(t, "ld {},${nn:04x}", RP[usize::from((op >> 4) & 3)]);
        }
        0x03 | 0x13 | 0x23 | 0x33 => {
            let _ = write!(t, "inc {}", RP[usize::from((op >> 4) & 3)]);
        }
        0x0B | 0x1B | 0x2B | 0x3B => {
            let _ = write!(t, "dec {}", RP[usize::from((op >> 4) & 3)]);
        }
        0x09 | 0x19 | 0x29 | 0x39 => {
            let _ = write!(t, "add hl,{}", RP[usize::from((op >> 4) & 3)]);
        }
        0x02 => t.push_str("ld (bc),a"),
        0x12 => t.push_str("ld (de),a"),
        0x0A => t.push_str("ld a,(bc)"),
        0x1A => t.push_str("ld a,(de)"),
        0x22 => {
            let nn = c.word();
            let _ = write!(t, "ld (${nn:04x}),hl");
        }
        0x2A => {
            let nn = c.word();
            let _ = write!(t, "ld hl,(${nn:04x})");
        }
        0x32 => {
            let nn = c.word();
            let _ = write!(t, "ld (${nn:04x}),a");
        }
        0x3A => {
            let nn = c.word();
            let _ = write!(t, "ld a,(${nn:04x})");
        }
        0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x34 | 0x3C => {
            let _ = write!(t, "inc {}", R8[usize::from((op >> 3) & 7)]);
        }
        0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x35 | 0x3D => {
            let _ = write!(t, "dec {}", R8[usize::from((op >> 3) & 7)]);
        }
        0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x36 | 0x3E => {
            let n = c.byte();
            let _ = write!(t, "ld {},${n:02x}", R8[usize::from((op >> 3) & 7)]);
        }
        0x40..=0x7F => {
            let _ = write!(
                t,
                "ld {},{}",
                R8[usize::from((op >> 3) & 7)],
                R8[usize::from(op & 7)]
            );
        }
        0x80..=0xBF => {
            let _ = write!(
                t,
                "{}{}",
                ALU[usize::from((op >> 3) & 7)],
                R8[usize::from(op & 7)]
            );
        }
        0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
            let n = c.byte();
            let _ = write!(t, "{}${n:02x}", ALU[usize::from((op >> 3) & 7)]);
        }
        0xC0 | 0xC8 | 0xD0 | 0xD8 | 0xE0 | 0xE8 | 0xF0 | 0xF8 => {
            let _ = write!(t, "ret {}", CC[usize::from((op >> 3) & 7)]);
        }
        0xC1 | 0xD1 | 0xE1 | 0xF1 => {
            let _ = write!(t, "pop {}", PP[usize::from((op >> 4) & 3)]);
        }
        0xC5 | 0xD5 | 0xE5 | 0xF5 => {
            let _ = write!(t, "push {}", PP[usize::from((op >> 4) & 3)]);
        }
        0xC2 | 0xCA | 0xD2 | 0xDA | 0xE2 | 0xEA | 0xF2 | 0xFA => {
            let nn = c.word();
            let _ = write!(t, "jp {},${nn:04x}", CC[usize::from((op >> 3) & 7)]);
        }
        0xC4 | 0xCC | 0xD4 | 0xDC | 0xE4 | 0xEC | 0xF4 | 0xFC => {
            let nn = c.word();
            let _ = write!(t, "call {},${nn:04x}", CC[usize::from((op >> 3) & 7)]);
        }
        0xC3 => {
            let nn = c.word();
            let _ = write!(t, "jp ${nn:04x}");
        }
        0xCD => {
            let nn = c.word();
            let _ = write!(t, "call ${nn:04x}");
        }
        0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => {
            let _ = write!(t, "rst ${:02x}", ((op >> 3) & 7) * 8);
        }
        0xD3 => {
            let n = c.byte();
            let _ = write!(t, "out (${n:02x}),a");
        }
        0xDB => {
            let n = c.byte();
            let _ = write!(t, "in a,(${n:02x})");
        }
        0xCB => cb(t, c),
        0xED => ed(t, c),
        0xDD => index(t, c, "ix"),
        0xFD => index(t, c, "iy"),
    }
}

/// The `CB` page: four uniform quarters of 64, and no operand bytes.
fn cb<F: FnMut(u16) -> u8>(t: &mut Text, c: &mut Cur<F>) {
    let op = c.byte();
    let (slot, bit) = (usize::from(op & 7), (op >> 3) & 7);
    match op >> 6 {
        0 => {
            let _ = write!(t, "{} {}", ROT[usize::from(bit)], R8[slot]);
        }
        1 => {
            let _ = write!(t, "bit {bit},{}", R8[slot]);
        }
        2 => {
            let _ = write!(t, "res {bit},{}", R8[slot]);
        }
        _ => {
            let _ = write!(t, "set {bit},{}", R8[slot]);
        }
    }
}

/// The `ED` page: 80 defined opcodes, and `db` for the rest.
///
/// The undefined ones are two `NOP`s' worth of time on real hardware, and rendering
/// them as `nop` would hide the fact that the pane just consumed two bytes for
/// nothing. `db $ed,$xx` says what the bytes are, which is what a reader staring at
/// an undefined opcode needs.
fn ed<F: FnMut(u16) -> u8>(t: &mut Text, c: &mut Cur<F>) {
    let op = c.byte();
    match op {
        0x40 | 0x48 | 0x50 | 0x58 | 0x60 | 0x68 | 0x78 => {
            let _ = write!(t, "in {},(c)", R8[usize::from((op >> 3) & 7)]);
        }
        // `IN (C)`: it flags the byte and discards it. Naming no register is the
        // point — writing `in (hl),(c)` would name a destination the core does not
        // write.
        0x70 => t.push_str("in (c)"),
        0x41 | 0x49 | 0x51 | 0x59 | 0x61 | 0x69 | 0x79 => {
            let _ = write!(t, "out (c),{}", R8[usize::from((op >> 3) & 7)]);
        }
        // NMOS outputs zero here, which is what the vectors show and what this says.
        0x71 => t.push_str("out (c),0"),
        0x42 | 0x52 | 0x62 | 0x72 => {
            let _ = write!(t, "sbc hl,{}", RP[usize::from((op >> 4) & 3)]);
        }
        0x4A | 0x5A | 0x6A | 0x7A => {
            let _ = write!(t, "adc hl,{}", RP[usize::from((op >> 4) & 3)]);
        }
        0x43 | 0x53 | 0x63 | 0x73 => {
            let nn = c.word();
            let _ = write!(t, "ld (${nn:04x}),{}", RP[usize::from((op >> 4) & 3)]);
        }
        0x4B | 0x5B | 0x6B | 0x7B => {
            let nn = c.word();
            let _ = write!(t, "ld {},(${nn:04x})", RP[usize::from((op >> 4) & 3)]);
        }
        0x44 | 0x4C | 0x54 | 0x5C | 0x64 | 0x6C | 0x74 | 0x7C => t.push_str("neg"),
        // One `reti` and seven `retn`, which is the encoding: the two differ only in
        // the acknowledge cycle a daisy-chained peripheral sees, and this core does
        // the same thing for both. Spelling them apart is still right — a listing
        // showing `retn` where the programmer wrote `reti` sends the reader hunting.
        0x4D => t.push_str("reti"),
        0x45 | 0x55 | 0x5D | 0x65 | 0x6D | 0x75 | 0x7D => t.push_str("retn"),
        0x46 | 0x4E | 0x66 | 0x6E => t.push_str("im 0"),
        0x56 | 0x76 => t.push_str("im 1"),
        0x5E | 0x7E => t.push_str("im 2"),
        0x47 => t.push_str("ld i,a"),
        0x4F => t.push_str("ld r,a"),
        0x57 => t.push_str("ld a,i"),
        0x5F => t.push_str("ld a,r"),
        0x67 => t.push_str("rrd"),
        0x6F => t.push_str("rld"),
        0xA0 => t.push_str("ldi"),
        0xA1 => t.push_str("cpi"),
        0xA2 => t.push_str("ini"),
        0xA3 => t.push_str("outi"),
        0xA8 => t.push_str("ldd"),
        0xA9 => t.push_str("cpd"),
        0xAA => t.push_str("ind"),
        0xAB => t.push_str("outd"),
        0xB0 => t.push_str("ldir"),
        0xB1 => t.push_str("cpir"),
        0xB2 => t.push_str("inir"),
        0xB3 => t.push_str("otir"),
        0xB8 => t.push_str("lddr"),
        0xB9 => t.push_str("cpdr"),
        0xBA => t.push_str("indr"),
        0xBB => t.push_str("otdr"),
        _ => {
            let _ = write!(t, "db $ed,${op:02x}");
        }
    }
}

/// The `DD` and `FD` pages. Arm for arm the same shape as
/// [`crate::decode::index_page`], because the lengths have to agree exactly.
fn index<F: FnMut(u16) -> u8>(t: &mut Text, c: &mut Cur<F>, idx: &str) {
    let op = c.byte();

    // A prefix restarts the rule: `DD FD 21` loads IY. Recursing is what makes that
    // fall out here, exactly as re-dispatching does in the core.
    match op {
        0xDD => return index(t, c, "ix"),
        0xFD => return index(t, c, "iy"),
        0xCB => return index_cb(t, c, idx),
        _ => {}
    }

    match op {
        // The 8-bit loads. A displaced operand suppresses the h/l rewrite on the
        // other side, because one instruction cannot use the index as both a pointer
        // and a register half. 0x76 is HALT and falls through.
        0x40..=0x7F if op != 0x76 => {
            let (dst, src) = ((op >> 3) & 7, op & 7);
            if dst == 6 {
                let d = c.byte() as i8;
                t.push_str("ld ");
                push_disp(t, idx, d);
                let _ = write!(t, ",{}", R8[usize::from(src)]);
            } else if src == 6 {
                let d = c.byte() as i8;
                let _ = write!(t, "ld {},", R8[usize::from(dst)]);
                push_disp(t, idx, d);
            } else {
                t.push_str("ld ");
                push_ireg(t, idx, dst);
                t.push_str(",");
                push_ireg(t, idx, src);
            }
        }
        // LD (IX+d),n — the displacement comes before the immediate.
        0x36 => {
            let d = c.byte() as i8;
            let n = c.byte();
            t.push_str("ld ");
            push_disp(t, idx, d);
            let _ = write!(t, ",${n:02x}");
        }
        0x26 | 0x2E => {
            let n = c.byte();
            t.push_str("ld ");
            push_ireg(t, idx, (op >> 3) & 7);
            let _ = write!(t, ",${n:02x}");
        }
        0x80..=0xBF => {
            let src = op & 7;
            t.push_str(ALU[usize::from((op >> 3) & 7)]);
            if src == 6 {
                let d = c.byte() as i8;
                push_disp(t, idx, d);
            } else {
                push_ireg(t, idx, src);
            }
        }
        0x34 | 0x35 => {
            let d = c.byte() as i8;
            t.push_str(if op == 0x34 { "inc " } else { "dec " });
            push_disp(t, idx, d);
        }
        0x24 | 0x25 | 0x2C | 0x2D => {
            t.push_str(if op & 1 == 0 { "inc " } else { "dec " });
            push_ireg(t, idx, (op >> 3) & 7);
        }
        // ADD IX,rr — with pair 2 meaning the index itself, not HL.
        0x09 | 0x19 | 0x29 | 0x39 => {
            let which = (op >> 4) & 3;
            let _ = write!(t, "add {idx},");
            if which == 2 {
                t.push_str(idx);
            } else {
                t.push_str(RP[usize::from(which)]);
            }
        }
        0x21 => {
            let nn = c.word();
            let _ = write!(t, "ld {idx},${nn:04x}");
        }
        0x22 => {
            let nn = c.word();
            let _ = write!(t, "ld (${nn:04x}),{idx}");
        }
        0x2A => {
            let nn = c.word();
            let _ = write!(t, "ld {idx},(${nn:04x})");
        }
        0x23 => {
            let _ = write!(t, "inc {idx}");
        }
        0x2B => {
            let _ = write!(t, "dec {idx}");
        }
        0xE1 => {
            let _ = write!(t, "pop {idx}");
        }
        0xE5 => {
            let _ = write!(t, "push {idx}");
        }
        0xE3 => {
            let _ = write!(t, "ex (sp),{idx}");
        }
        0xE9 => {
            let _ = write!(t, "jp ({idx})");
        }
        0xF9 => {
            let _ = write!(t, "ld sp,{idx}");
        }
        // The prefix cost 4 T-states and changed nothing. Showing it is what keeps
        // the line's byte count honest: `dd 00` is two bytes, and a bare `nop` reads
        // as one.
        _ => {
            let _ = write!(t, "[{}] ", if idx == "ix" { "dd" } else { "fd" });
            base(t, c, op);
        }
    }
}

/// The `DD CB` and `FD CB` pages: always four bytes, always on displaced memory.
///
/// The displacement precedes the opcode, which is why this cannot be `cb` with a
/// different register name — that would read the displacement as the opcode.
///
/// The register field is a **second destination**, not the operand: all eight
/// encodings act on `(idx+d)`, and seven of them also copy the result into the named
/// register. Those seven are undocumented, and the second operand is how a reader
/// tells `dd cb 05 00` from `dd cb 05 06` — which the core does distinguish.
fn index_cb<F: FnMut(u16) -> u8>(t: &mut Text, c: &mut Cur<F>, idx: &str) {
    let d = c.byte() as i8;
    let op = c.byte();
    let field = op & 7;
    let bit = (op >> 3) & 7;
    match op >> 6 {
        // BIT copies nothing, so its eight encodings really are identical and naming
        // a register would invent a destination.
        1 => {
            let _ = write!(t, "bit {bit},");
            push_disp(t, idx, d);
        }
        q => {
            match q {
                0 => t.push_str(ROT[usize::from(bit)]),
                2 => {
                    let _ = write!(t, "res {bit}");
                }
                _ => {
                    let _ = write!(t, "set {bit}");
                }
            }
            t.push_str(if q == 0 { " " } else { "," });
            push_disp(t, idx, d);
            if field != 6 {
                let _ = write!(t, ",{}", R8[usize::from(field)]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testbus::Mem;
    use crate::Z80;

    /// The number of bytes an instruction *fetched*, from the bus's read log.
    ///
    /// The fetch bytes are the maximal run of reads at `pc`, `pc+1`, `pc+2` … : every
    /// page reads all of its operand bytes before any data byte, so the run ends
    /// exactly where the instruction does.
    ///
    /// `PC`-delta cannot be used instead, and that is the whole reason this helper
    /// exists: a branch sets `PC` to its target, a repeating `LDIR` rewinds `PC` to
    /// where it started, and a `HALT` holds it. All three would report a length the
    /// instruction does not have — so a length test built on `PC` would have to skip
    /// precisely the opcodes most likely to be wrong.
    fn fetched(m: &Mem, pc: u16) -> u16 {
        let mut n = 0u16;
        for &a in &m.reads {
            if a == pc.wrapping_add(n) {
                n += 1;
            } else {
                break;
            }
        }
        n
    }

    /// A CPU whose every pointer aims far away from the program.
    ///
    /// Required by [`fetched`]: with `HL`, `SP`, `BC`, `DE` and both index registers
    /// pointing near 0x4000 and 0x3000, and every operand byte 0x40, no data access
    /// can land at `pc + n` and be mistaken for a fetch.
    fn far_from(pc: u16) -> Z80 {
        let mut c = Z80::new();
        c.pc = pc;
        c.sp = 0x3000;
        c.set_hl(0x4000);
        c.set_bc(0x4040);
        c.set_de(0x4080);
        c.ix = 0x4000;
        c.iy = 0x4000;
        c
    }

    /// The reported length equals what the core consumes, on all five pages entire.
    ///
    /// 1,280 instructions: every one of the 256 base opcodes, the 256 `CB`, the 256
    /// `ED`, the 256 `DD` and the 256 `DD CB`. The core is the authority — checking
    /// against a table of expected lengths would be a second guess able to agree with
    /// a wrong first one, and it is the *rare* encodings that get lengths wrong, so a
    /// hand-picked list is exactly the wrong instrument.
    ///
    /// The filler byte is 0x40 for a reason: every operand it forms points at
    /// 0x4040-ish, far from the program, so [`fetched`] cannot mistake a data read
    /// for an operand read. It also makes `B` non-zero, so `DJNZ` takes its branch,
    /// and `BC` non-zero, so the repeating block forms repeat — both being the arms
    /// with the unusual `PC`.
    #[test]
    fn every_opcode_reports_the_length_the_core_consumes() {
        const F: u8 = 0x40;
        let mut checked = 0usize;
        for op in 0..=255u8 {
            let programs: [&[u8]; 5] = [
                &[op, F, F, F, F],
                &[0xCB, op],
                &[0xED, op, F, F],
                &[0xDD, op, F, F, F],
                &[0xDD, 0xCB, F, op],
            ];
            for bytes in programs {
                let mut m = Mem::at(0x100, bytes);
                let (text, len) = disasm(|a| m.ram[usize::from(a)], 0x100);
                assert!(!text.as_str().is_empty(), "no text for {bytes:02X?}");
                let mut c = far_from(0x100);
                c.step(&mut m);
                assert_eq!(
                    len,
                    fetched(&m, 0x100),
                    "{bytes:02X?} disassembled as `{text}`"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 1280, "five pages of 256");
    }

    /// The same, for `FD`, which shares its code path with `DD` through one argument.
    ///
    /// Cheap to add and it pins the argument: a `disasm` that hardcoded `ix` would
    /// pass every test above and print the wrong register for half the real
    /// instructions on the page.
    #[test]
    fn the_iy_pages_report_the_same_lengths_and_name_iy() {
        const F: u8 = 0x40;
        for op in 0..=255u8 {
            for bytes in [&[0xFD, op, F, F, F][..], &[0xFD, 0xCB, F, op][..]] {
                let mut m = Mem::at(0x100, bytes);
                let (text, len) = disasm(|a| m.ram[usize::from(a)], 0x100);
                let mut c = far_from(0x100);
                c.step(&mut m);
                assert_eq!(
                    len,
                    fetched(&m, 0x100),
                    "{bytes:02X?} disassembled as `{text}`"
                );
                assert!(
                    !text.as_str().contains("ix"),
                    "{bytes:02X?} named ix: `{text}`"
                );
            }
        }
    }

    /// A branch's reported length is its encoding's length, not its target.
    #[test]
    fn a_branch_reports_its_encoded_length() {
        for (bytes, want) in [
            (&[0xC3u8, 0x00, 0x20][..], 3u16),
            (&[0x18, 0xFE][..], 2),
            (&[0xC9][..], 1),
            (&[0xCD, 0x00, 0x20][..], 3),
            (&[0xC7][..], 1),
            (&[0xE9][..], 1),
            (&[0xDD, 0xE9][..], 2),
        ] {
            let m = Mem::at(0x100, bytes);
            let (text, len) = disasm(|a| m.ram[usize::from(a)], 0x100);
            assert_eq!(len, want, "{bytes:02X?} disassembled as `{text}`");
        }
    }

    /// The text is readable, and the displacement is shown signed.
    ///
    /// `(ix-5)`, not `(ix+251)` and not `(ix+fb)`. A debugger that shows the raw byte
    /// makes the reader do the two's-complement conversion, which is exactly the
    /// conversion they opened the debugger to avoid.
    #[test]
    fn the_text_reads_like_assembly_with_a_signed_displacement() {
        for (bytes, want) in [
            (&[0x00u8][..], "nop"),
            (&[0x3E, 0x42][..], "ld a,$42"),
            (&[0x21, 0x34, 0x12][..], "ld hl,$1234"),
            (&[0xCB, 0x06][..], "rlc (hl)"),
            (&[0xCB, 0x30][..], "sll b"),
            (&[0xED, 0xB0][..], "ldir"),
            (&[0xED, 0x43, 0x00, 0x20][..], "ld ($2000),bc"),
            (&[0xDD, 0x7E, 0x05][..], "ld a,(ix+5)"),
            (&[0xDD, 0x7E, 0xFB][..], "ld a,(ix-5)"),
            (&[0xDD, 0x7E, 0x80][..], "ld a,(ix-128)"),
            (&[0xDD, 0x36, 0x05, 0x99][..], "ld (ix+5),$99"),
            (&[0xDD, 0x44][..], "ld b,ixh"),
            (&[0xDD, 0x66, 0x05][..], "ld h,(ix+5)"),
            (&[0xDD, 0x29][..], "add ix,ix"),
            (&[0xFD, 0xCB, 0xFB, 0x46][..], "bit 0,(iy-5)"),
            (&[0x18, 0xFE][..], "jr $0100"),
            (&[0x86][..], "add a,(hl)"),
            (&[0xC7][..], "rst $00"),
            (&[0xFF][..], "rst $38"),
        ] {
            let m = Mem::at(0x100, bytes);
            let (text, _) = disasm(|a| m.ram[usize::from(a)], 0x100);
            assert_eq!(text.as_str(), want, "on {bytes:02X?}");
        }
    }

    /// `jr` shows its resolved target, not its displacement.
    ///
    /// `18 FE` is a jump to itself: the displacement is relative to the *end* of the
    /// instruction, so −2 from 0x102 is 0x100. Showing `jr -2` would make the reader
    /// do that arithmetic, and getting the base wrong by one is the classic error.
    #[test]
    fn jr_resolves_its_target_from_the_end_of_the_instruction() {
        let m = Mem::at(0x100, &[0x18, 0x05]);
        let (text, len) = disasm(|a| m.ram[usize::from(a)], 0x100);
        assert_eq!(len, 2);
        assert_eq!(text.as_str(), "jr $0107", "0x102 + 5, not 0x100 + 5");
    }

    /// The resolved target agrees with where the core actually lands.
    ///
    /// The two conditional relative forms and `DJNZ` all resolve the same way, and a
    /// base-off-by-one would be invisible against a hand-written expectation that
    /// made the same slip. So the address in the text is compared against the `PC` the
    /// core arrives at — including through a prefix, where the base is 3 bytes on and
    /// a hardcoded `pc + 2` would be wrong.
    #[test]
    fn a_resolved_target_agrees_with_where_the_core_lands() {
        for bytes in [
            &[0x18u8, 0x05][..],     // JR +5
            &[0x18, 0xFB][..],       // JR -5
            &[0x28, 0x05][..],       // JR Z (taken: Z is set below)
            &[0x10, 0x05][..],       // DJNZ, with B non-zero
            &[0xDD, 0x18, 0x05][..], // JR behind a wasted prefix: base is pc + 3
        ] {
            let mut m = Mem::at(0x100, bytes);
            let (text, _) = disasm(|a| m.ram[usize::from(a)], 0x100);
            let mut c = far_from(0x100);
            c.f = crate::flags::Z;
            c.step(&mut m);
            let mut want = Text::new();
            let _ = write!(want, "${:04x}", c.pc);
            assert!(
                text.as_str().ends_with(want.as_str()),
                "`{text}` does not end at the core's {}",
                want.as_str()
            );
        }
    }

    /// A wasted prefix is shown, because it was paid for.
    ///
    /// `dd 00` is two bytes and 8 T-states. A pane rendering it as a bare `nop` would
    /// be describing a one-byte instruction, and the next line would start inside
    /// this one.
    #[test]
    fn a_prefix_that_changes_nothing_is_still_shown() {
        for (bytes, want) in [
            (&[0xDDu8, 0x00][..], "[dd] nop"),
            (&[0xFD, 0x00][..], "[fd] nop"),
            (&[0xDD, 0xEB][..], "[dd] ex de,hl"),
            (&[0xDD, 0x76][..], "[dd] halt"),
        ] {
            let m = Mem::at(0x100, bytes);
            let (text, len) = disasm(|a| m.ram[usize::from(a)], 0x100);
            assert_eq!(text.as_str(), want, "on {bytes:02X?}");
            assert_eq!(len, 2, "and both bytes counted");
        }
    }

    /// The last prefix before the opcode is the one that applies.
    ///
    /// `DD FD 21` loads `IY`, and the reported length counts both prefixes. A
    /// disassembler that stopped at the first prefix would name the wrong register
    /// *and* report 3 where the core consumes 4.
    #[test]
    fn the_last_prefix_wins_and_every_prefix_is_counted() {
        let m = Mem::at(0x100, &[0xDD, 0xFD, 0x21, 0x34, 0x12]);
        let (text, len) = disasm(|a| m.ram[usize::from(a)], 0x100);
        assert_eq!(text.as_str(), "ld iy,$1234");
        assert_eq!(len, 5, "two prefixes, an opcode and two operand bytes");
    }

    /// The double-prefix page names its second destination, and `BIT` names none.
    ///
    /// The register field is not the operand: `dd cb 05 00` and `dd cb 05 06` both
    /// rotate `(ix+5)`, and the first also copies the result to `B`. A pane that
    /// printed them identically would hide a register the core writes.
    #[test]
    fn the_double_prefix_page_shows_the_extra_destination() {
        for (bytes, want) in [
            (&[0xDDu8, 0xCB, 0x05, 0x06][..], "rlc (ix+5)"),
            (&[0xDD, 0xCB, 0x05, 0x00][..], "rlc (ix+5),b"),
            (&[0xDD, 0xCB, 0x05, 0x34][..], "sll (ix+5),h"),
            (&[0xDD, 0xCB, 0x05, 0x86][..], "res 0,(ix+5)"),
            (&[0xDD, 0xCB, 0x05, 0x80][..], "res 0,(ix+5),b"),
            (&[0xDD, 0xCB, 0x05, 0xFF][..], "set 7,(ix+5),a"),
            // BIT copies nothing, so all eight fields read the same.
            (&[0xDD, 0xCB, 0x05, 0x46][..], "bit 0,(ix+5)"),
            (&[0xDD, 0xCB, 0x05, 0x40][..], "bit 0,(ix+5)"),
        ] {
            let m = Mem::at(0x100, bytes);
            let (text, len) = disasm(|a| m.ram[usize::from(a)], 0x100);
            assert_eq!(text.as_str(), want, "on {bytes:02X?}");
            assert_eq!(len, 4, "always four bytes");
        }
    }

    /// An undefined `ED` opcode shows its bytes rather than a plausible mnemonic.
    #[test]
    fn an_undefined_ed_opcode_is_rendered_as_data() {
        let m = Mem::at(0x100, &[0xED, 0x00]);
        let (text, len) = disasm(|a| m.ram[usize::from(a)], 0x100);
        assert_eq!(text.as_str(), "db $ed,$00");
        assert_eq!(len, 2, "and it still cost two bytes");
    }

    /// `Text` truncates rather than panicking.
    ///
    /// A debugger pane must never take the emulator down. The longest real
    /// instruction text is well inside the buffer, so truncation is unreachable in
    /// practice — which is exactly why it must be tested directly rather than
    /// trusted.
    #[test]
    fn the_text_buffer_truncates_instead_of_panicking() {
        let mut t = Text::new();
        for _ in 0..200 {
            t.push_str("xxxxxxxxxx");
        }
        assert_eq!(t.as_str().len(), Text::CAP);
        // And through the `Write` impl, which is the path every number takes.
        let mut u = Text::new();
        for _ in 0..200 {
            let _ = write!(u, "{:x}", 0xDEAD_BEEFu32);
        }
        assert_eq!(u.as_str().len(), Text::CAP);
    }

    /// A byte-wise truncation that split a multi-byte character says so.
    ///
    /// Nothing this module writes is non-ASCII, so this is unreachable through
    /// `disasm` — and `Text` is public, so it is reachable through a caller. The
    /// alternative to the marker is a panic inside a debugger pane.
    #[test]
    fn a_split_character_does_not_panic() {
        let mut t = Text::new();
        // 31 bytes, then a two-byte character with room for only its first byte.
        t.push_str("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        t.push_str("é");
        assert_eq!(t.as_str(), "<bad utf8>");
    }

    /// `disasm_bus` agrees with `disasm` over the same bytes.
    #[test]
    fn the_bus_wrapper_reads_the_same_instruction() {
        let mut m = Mem::at(0x100, &[0xDD, 0xCB, 0xFB, 0x46]);
        let (want, want_len) = disasm(|a| m.ram[usize::from(a)], 0x100);
        let (got, got_len) = disasm_bus(&mut m, 0x100);
        assert_eq!(got.as_str(), want.as_str());
        assert_eq!(got_len, want_len);
    }
}
