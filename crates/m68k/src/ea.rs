//! Effective addressing.
//!
//! The 68000 encodes an operand location in a 6-bit field: a 3-bit mode and a
//! 3-bit register. Twelve distinct addressing modes come out of that, seven of
//! them sharing `mode == 7` and distinguished by the register field.
//!
//! This module deliberately splits *resolution* from *access*. Resolution
//! computes where an operand lives and consumes any extension words; access
//! reads or writes it. Instructions need them separated because the bus
//! schedule interleaves program fetches with operand accesses — see
//! `ops::move_` for how the two halves are ordered — and because `LEA` and
//! `PEA` resolve an address without ever accessing it.

use crate::cpu::{M68k, ADDR_MASK};
use crate::Bus;

/// Operand width. The 68000's three operand sizes; `Size::Byte` accesses are
/// one bus cycle, `Size::Long` accesses are always two.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Size {
    Byte,
    Word,
    Long,
}

impl Size {
    /// Bytes occupied in memory. Note a byte operand still occupies a whole
    /// bus cycle; this is the operand's width, not its bus cost.
    #[inline]
    pub fn bytes(self) -> u32 {
        match self {
            Size::Byte => 1,
            Size::Word => 2,
            Size::Long => 4,
        }
    }

    /// How much `(An)+` / `-(An)` moves the address register.
    ///
    /// Byte operands through `A7` step by 2, not 1, so the stack pointer stays
    /// word-aligned.
    #[inline]
    pub fn step(self, reg: usize) -> u32 {
        if self == Size::Byte && reg == 7 {
            2
        } else {
            self.bytes()
        }
    }

    /// Mask selecting the bits this size occupies.
    #[inline]
    pub fn mask(self) -> u32 {
        match self {
            Size::Byte => 0xFF,
            Size::Word => 0xFFFF,
            Size::Long => 0xFFFF_FFFF,
        }
    }

    /// The sign bit for this size.
    #[inline]
    pub fn msb(self) -> u32 {
        match self {
            Size::Byte => 0x80,
            Size::Word => 0x8000,
            Size::Long => 0x8000_0000,
        }
    }

    /// Sign-extends a value of this size to 32 bits.
    #[inline]
    pub fn sign_extend(self, v: u32) -> u32 {
        match self {
            Size::Byte => v as u8 as i8 as i32 as u32,
            Size::Word => v as u16 as i16 as i32 as u32,
            Size::Long => v,
        }
    }
}

/// A resolved operand location.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ea {
    DataReg(usize),
    AddrReg(usize),
    /// A memory operand at an already-computed address. The address is **not**
    /// masked to 24 bits and **not** checked for alignment: the alignment check
    /// belongs to the access, so that the address error can stack the full
    /// 32-bit faulting address (addendum §9.6).
    Mem(u32),
    /// An immediate already fetched from the instruction stream.
    Imm(u32),
}

/// True if `(mode, reg)` names a memory operand.
///
/// Used by the schedule model, which must know this *before* resolving, to
/// decide how many program fetches precede the operand access. There is
/// deliberately no `Ea::is_mem` counterpart: every caller so far needs the
/// answer while planning the bus schedule, which is strictly earlier than
/// having an [`Ea`] in hand, and a post-resolve variant went unused.
#[inline]
pub fn mode_is_mem(mode: u16, reg: u16) -> bool {
    matches!(mode, 2..=6) || (mode == 7 && reg <= 3)
}

/// Addressing-mode category predicates from the 68000 PRM.
///
/// These are the canonical single-source definitions; both the disassembler
/// (`m68k::disasm`) and the opcode-space exhaustive tests (`testrunner`) import
/// from here so the two never drift apart. The predicates are thin — one
/// `match` each — but the three that differ by exactly one mode are a common
/// source of legality bugs.
pub mod modes {
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

    /// Data-alterable: alterable memory plus a data register. This is the
    /// destination set for most single-operand instructions and for
    /// `ADDQ`/`SUBQ` at byte size.
    pub fn data_alterable(mode: u16, reg: u16) -> bool {
        mode == 0 || mem_alterable(mode, reg)
    }

    /// Control: memory whose address does not depend on the access — no
    /// increment, no decrement, no immediate. `LEA`, `PEA`, `JMP`, and `JSR`
    /// use this set.
    pub fn control(mode: u16, reg: u16) -> bool {
        matches!(mode, 2 | 5 | 6) || (mode == 7 && reg <= 3)
    }

    /// `BTST`'s operand set: data-alterable, plus the two PC-relative modes
    /// because `BTST` does not write. The immediate operand `#data` is legal
    /// for the **dynamic** form only; the static `BTST #n,#data` does not exist.
    ///
    /// Measured: across 2,500 `BTST` cases the dynamic form uses mode 7 reg 4
    /// fifty-eight times; the static form never. The static form is well
    /// represented at mode 5 (328 dynamic / 40 static), so the static-form
    /// absence at mode 7 reg 4 is an encoding fact, not a thin sample.
    pub fn btst_src(mode: u16, reg: u16, immediate_ok: bool) -> bool {
        match mode {
            0 => true,
            1 => false,
            7 => reg <= 3 || (reg == 4 && immediate_ok),
            _ => true,
        }
    }
}

/// Number of extension words `(mode, reg)` consumes from the instruction
/// stream.
#[inline]
pub fn ext_words(mode: u16, reg: u16, size: Size) -> u32 {
    match mode {
        5 | 6 => 1,
        7 => match reg {
            0 | 2 | 3 => 1,
            1 => 2,
            4 => {
                if size == Size::Long {
                    2
                } else {
                    1
                }
            }
            _ => 0,
        },
        _ => 0,
    }
}

/// Computes the address for the extension-word-bearing brief format
/// `(d8, An, Xn)` / `(d8, PC, Xn)`.
///
/// `base` is the address register or PC value the displacement applies to.
fn indexed(cpu: &M68k, base: u32, ext: u16) -> u32 {
    let disp = ext as u8 as i8 as i32 as u32;
    let ireg = ((ext >> 12) & 7) as usize;
    let long = ext & 0x0800 != 0;
    let raw = if ext & 0x8000 != 0 {
        cpu.a[ireg]
    } else {
        cpu.d[ireg]
    };
    let index = if long {
        raw
    } else {
        raw as u16 as i16 as i32 as u32
    };
    base.wrapping_add(index).wrapping_add(disp)
}

/// Resolves an effective address from **already-available** extension words.
///
/// # Why this does not fetch
///
/// Extension-word *values* and the bus reads that supply them are separated in
/// time by the prefetch queue: the queue holds two words beyond the current
/// instruction word, so an operand's extension words are frequently readable
/// before the bus cycle that refills the queue behind them. `MOVE.b (An),abs.L`
/// is the clear case — it performs its destination write after only **one**
/// program fetch, yet needs both halves of a 32-bit absolute address, because
/// the second half is already sitting in `prefetch[1]`.
///
/// A `resolve` that called `fetch_word` per extension word would therefore emit
/// program reads at the wrong points in the bus sequence, and the harness
/// compares that sequence in order. So resolution is pure: the caller schedules
/// the queue advances and passes the words down (addendum §3, §4).
///
/// `ext` holds this operand's extension words in instruction order. `ext_base`
/// is the address of the first of them, used by the PC-relative modes — the
/// 68000 computes `d16(PC)` relative to the extension word's own address, not
/// to the opcode's and not to the live PC.
///
/// `(An)+` and `-(An)` update the address register here, as a side effect of
/// resolution. That is deliberate and observable: when a later access faults,
/// the adjustment is already committed, which is what the vectors record
/// (addendum §9.3).
///
/// # Panics
///
/// `ext.len()` **must** be at least `ext_words(mode, reg, size)`. That is the
/// contract: always size the slice with [`ext_words`] under the same
/// `(mode, reg, size)` rather than counting words by hand, because the two
/// functions are keyed identically and cannot then disagree. Passing a shorter
/// slice — for instance `&[]` for `d16(An)` because the handler fetched the
/// displacement into a local — panics on the index. A panic is a *host* bug;
/// guest faults are emulated 68000 exceptions and never unwind.
pub fn resolve(cpu: &mut M68k, mode: u16, reg: u16, size: Size, ext: &[u16], ext_base: u32) -> Ea {
    let r = reg as usize;
    match mode {
        0 => Ea::DataReg(r),
        1 => Ea::AddrReg(r),
        2 => Ea::Mem(cpu.a[r]),
        3 => {
            let addr = cpu.a[r];
            cpu.a[r] = addr.wrapping_add(size.step(r));
            Ea::Mem(addr)
        }
        4 => {
            // The decrement happens before the address is formed, so it is
            // committed even if the access faults.
            let addr = cpu.a[r].wrapping_sub(size.step(r));
            cpu.a[r] = addr;
            Ea::Mem(addr)
        }
        5 => Ea::Mem(cpu.a[r].wrapping_add(ext[0] as i16 as i32 as u32)),
        6 => Ea::Mem(indexed(cpu, cpu.a[r], ext[0])),
        7 => match reg {
            0 => Ea::Mem(ext[0] as i16 as i32 as u32),
            1 => Ea::Mem(((ext[0] as u32) << 16) | ext[1] as u32),
            2 => Ea::Mem(ext_base.wrapping_add(ext[0] as i16 as i32 as u32)),
            3 => Ea::Mem(indexed(cpu, ext_base, ext[0])),
            4 => Ea::Imm(match size {
                Size::Byte => ext[0] as u32 & 0xFF,
                Size::Word => ext[0] as u32,
                Size::Long => ((ext[0] as u32) << 16) | ext[1] as u32,
            }),
            _ => unreachable!("mode 7 reg {reg} is not a valid EA"),
        },
        _ => unreachable!("EA mode {mode} does not exist"),
    }
}

/// Reads a resolved operand.
///
/// Long reads are two word accesses, high word first — 32-bit source operands
/// are ascending by address in every measured case (addendum §7e, §9.2).
pub fn read(cpu: &M68k, bus: &mut dyn Bus, ea: Ea, size: Size) -> u32 {
    match ea {
        Ea::DataReg(r) => cpu.d[r] & size.mask(),
        Ea::AddrReg(r) => cpu.a[r] & size.mask(),
        Ea::Imm(v) => v,
        Ea::Mem(addr) => {
            let a = addr & ADDR_MASK;
            match size {
                Size::Byte => bus.read8(a) as u32,
                Size::Word => bus.read16(a) as u32,
                Size::Long => {
                    let hi = bus.read16(a) as u32;
                    let lo = bus.read16(a.wrapping_add(2) & ADDR_MASK) as u32;
                    (hi << 16) | lo
                }
            }
        }
    }
}

/// Writes a resolved operand.
///
/// A `Size::Byte` or `Size::Word` write into a register leaves the register's
/// upper bits alone. An `AddrReg` write instead takes the full 32 bits and
/// **sign-extends** a word-sized value: every instruction that writes an address
/// register affects all 32 bits, and the word-sized forms (`MOVEA.w`, `ADDA.w`,
/// `SUBA.w`) get there by sign extension. Truncating instead would store
/// `0x0000_8001` where `0xFFFF_8001` is required. The extension is a no-op for
/// `Size::Long`, so the long-sized forms are unaffected.
///
/// Long writes are two word accesses, **high word first**. The one exception —
/// `-(An)`, which writes the low word first — is handled by
/// [`write_predec_long`], because the ordering is a property of the addressing
/// mode rather than of the address (addendum §9.2).
pub fn write(cpu: &mut M68k, bus: &mut dyn Bus, ea: Ea, size: Size, val: u32) {
    match ea {
        Ea::DataReg(r) => {
            let m = size.mask();
            cpu.d[r] = (cpu.d[r] & !m) | (val & m);
        }
        Ea::AddrReg(r) => cpu.a[r] = size.sign_extend(val),
        Ea::Imm(_) => unreachable!("cannot write to an immediate operand"),
        Ea::Mem(addr) => {
            let a = addr & ADDR_MASK;
            match size {
                Size::Byte => bus.write8(a, val as u8),
                Size::Word => bus.write16(a, val as u16),
                Size::Long => {
                    bus.write16(a, (val >> 16) as u16);
                    bus.write16(a.wrapping_add(2) & ADDR_MASK, val as u16);
                }
            }
        }
    }
}

/// Writes a long operand to a predecrementing destination: **low word first**,
/// then the high word.
///
/// `MOVE.l` into `-(An)` is the only MOVE destination that writes descending —
/// measured 147/147, against high-word-first in every other mode (addendum
/// §9.2). The core now has four distinct 32-bit write orders and no shared
/// helper can express all of them; this is the second.
///
/// ⚠️ **This does not decrement anything.** `addr` is the *already-decremented*
/// destination — the low end of the long, `An - 4` — and the caller owns the
/// register update. The name describes which addressing mode's write order this is,
/// not what it does to `An`, and the distinction matters because the plausible
/// misreading ("it handles the predecrement") would leave `An` unmodified while the
/// write still lands in the right place, so memory contents would be correct and
/// only the register wrong. Both callers (`ops::alu` and `ops::move_`) compute the
/// address themselves; see them before adding a third.
pub fn write_predec_long(bus: &mut dyn Bus, addr: u32, val: u32) {
    let a = addr & ADDR_MASK;
    bus.write16(a.wrapping_add(2) & ADDR_MASK, val as u16);
    bus.write16(a, (val >> 16) as u16);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::FlatBus;

    /// `modes::control` enumerated over all 64 `(mode, reg)` pairs.
    ///
    /// Four sites used to state this rule: this function, a private `is_control`
    /// in `ops/branch.rs`, a hand-inlined copy in `opcode_space.rs`'s JMP/JSR
    /// arm, and `jump_idle`'s seven match arms. The first three were consolidated
    /// onto this one in Task 14 after checking they agreed on 64/64 pairs.
    ///
    /// ⚠️ **That consolidation is why this test exists.** Before it, a wrong
    /// `control` was caught by two copies disagreeing with it. Now every caller
    /// asks the same function, so a wrong answer here is consistent everywhere
    /// and nothing downstream contradicts it. This is the cost of removing
    /// duplication: the accidental cross-check goes too, and it has to be
    /// replaced by a deliberate one. Enumerating the set by hand — rather than
    /// re-deriving it from `matches!` — is what makes this a check and not a
    /// restatement.
    ///
    /// The set is the 68000 PRM's: `(An)`, `(d16,An)`, `(d8,An,Xn)`, `(xxx).W`,
    /// `(xxx).L`, `(d16,PC)`, `(d8,PC,Xn)`. Not `Dn`, not `An`, not `(An)+` or
    /// `-(An)` (no operand size to step by), not `#imm` (no address at all).
    #[test]
    fn control_modes_are_the_seven_addressable_without_stepping() {
        // Hand-written truth table: (mode, reg) pairs that ARE control.
        let want: &[(u16, u16)] = &[
            (2, 0),
            (2, 1),
            (2, 2),
            (2, 3),
            (2, 4),
            (2, 5),
            (2, 6),
            (2, 7), // (An)
            (5, 0),
            (5, 1),
            (5, 2),
            (5, 3),
            (5, 4),
            (5, 5),
            (5, 6),
            (5, 7), // (d16,An)
            (6, 0),
            (6, 1),
            (6, 2),
            (6, 3),
            (6, 4),
            (6, 5),
            (6, 6),
            (6, 7), // (d8,An,Xn)
            (7, 0), // (xxx).W
            (7, 1), // (xxx).L
            (7, 2), // (d16,PC)
            (7, 3), // (d8,PC,Xn)
        ];
        for mode in 0u16..8 {
            for reg in 0u16..8 {
                let expected = want.contains(&(mode, reg));
                assert_eq!(
                    modes::control(mode, reg),
                    expected,
                    "control({mode},{reg}) should be {expected}"
                );
            }
        }
        // The size of the set, stated separately so that a truth table edited to
        // match a wrong implementation still fails.
        assert_eq!(want.len(), 28);
        // And the four exclusions most likely to be wrongly admitted, named:
        assert!(!modes::control(0, 0), "Dn is not control");
        assert!(!modes::control(1, 0), "An is not control");
        assert!(!modes::control(3, 0), "(An)+ is not control");
        assert!(!modes::control(4, 0), "-(An) is not control");
        assert!(!modes::control(7, 4), "#imm is not control");
    }

    #[test]
    fn byte_step_through_a7_is_two() {
        assert_eq!(Size::Byte.step(0), 1);
        assert_eq!(Size::Byte.step(7), 2);
        assert_eq!(Size::Word.step(7), 2);
        assert_eq!(Size::Long.step(7), 4);
    }

    #[test]
    fn predecrement_commits_before_the_address_is_used() {
        // Addendum §9.3: the decrement is visible even when the access faults,
        // because resolution performs it.
        let mut cpu = M68k::new();
        cpu.a[3] = 0x1000;
        let ea = resolve(&mut cpu, 4, 3, Size::Long, &[], 0);
        assert_eq!(ea, Ea::Mem(0x0FFC));
        assert_eq!(cpu.a[3], 0x0FFC);
    }

    #[test]
    fn postincrement_commits_and_yields_the_old_address() {
        let mut cpu = M68k::new();
        cpu.a[3] = 0x1000;
        let ea = resolve(&mut cpu, 3, 3, Size::Word, &[], 0);
        assert_eq!(ea, Ea::Mem(0x1000));
        assert_eq!(cpu.a[3], 0x1002);
    }

    #[test]
    fn pc_relative_displacement_is_from_the_extension_word() {
        // d16(PC) is relative to the address of the displacement word itself,
        // which for a MOVE source is opcode + 2.
        let mut cpu = M68k::new();
        let ea = resolve(&mut cpu, 7, 2, Size::Word, &[0x0010], 0x1002);
        assert_eq!(ea, Ea::Mem(0x1012));
    }

    #[test]
    fn absolute_long_joins_two_extension_words() {
        let mut cpu = M68k::new();
        let ea = resolve(&mut cpu, 7, 1, Size::Word, &[0x00FF, 0x1234], 0);
        assert_eq!(ea, Ea::Mem(0x00FF_1234));
    }

    #[test]
    fn indexed_sign_extends_a_word_index() {
        let mut cpu = M68k::new();
        cpu.a[1] = 0x2000;
        cpu.d[2] = 0x0000_FFFE; // -2 as a word
                                // ext: Dn index, reg 2, word size, displacement +4
        let ea = resolve(&mut cpu, 6, 1, Size::Word, &[0x2004], 0);
        assert_eq!(ea, Ea::Mem(0x2002), "0x2000 - 2 + 4");
    }

    #[test]
    fn long_write_is_high_word_first_except_predecrement() {
        use crate::cpu::tests_support::RecordingBus;
        let mut cpu = M68k::new();

        let mut bus = RecordingBus::new();
        write(&mut cpu, &mut bus, Ea::Mem(0x2000), Size::Long, 0xAAAA_BBBB);
        assert_eq!(bus.writes(), vec![(0x2000, 0xAAAA), (0x2002, 0xBBBB)]);

        let mut bus = RecordingBus::new();
        write_predec_long(&mut bus, 0x2000, 0xAAAA_BBBB);
        assert_eq!(bus.writes(), vec![(0x2002, 0xBBBB), (0x2000, 0xAAAA)]);
    }

    #[test]
    fn byte_write_to_a_data_register_preserves_upper_bits() {
        let mut cpu = M68k::new();
        let mut bus = FlatBus::new();
        cpu.d[0] = 0x1234_5678;
        write(&mut cpu, &mut bus, Ea::DataReg(0), Size::Byte, 0xAB);
        assert_eq!(cpu.d[0], 0x1234_56AB);
        write(&mut cpu, &mut bus, Ea::DataReg(0), Size::Word, 0xCDEF);
        assert_eq!(cpu.d[0], 0x1234_CDEF);
    }

    #[test]
    fn ext_words_per_mode() {
        assert_eq!(ext_words(2, 0, Size::Word), 0);
        assert_eq!(ext_words(5, 0, Size::Word), 1);
        assert_eq!(ext_words(6, 0, Size::Word), 1);
        assert_eq!(ext_words(7, 0, Size::Word), 1); // abs.W
        assert_eq!(ext_words(7, 1, Size::Word), 2); // abs.L
        assert_eq!(ext_words(7, 4, Size::Byte), 1); // #imm.b
        assert_eq!(ext_words(7, 4, Size::Long), 2); // #imm.l
    }
}
