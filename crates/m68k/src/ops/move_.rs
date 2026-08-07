//! `MOVE`, `MOVEA` and `MOVEQ`.
//!
//! # Why this is not "resolve source, read, resolve destination, write"
//!
//! That structure emits every program fetch before any operand access. The real
//! chip interleaves them, and the harness compares the bus sequence *in order*,
//! so the obvious structure fails almost every memory-to-memory case.
//!
//! The measured schedule (addendum §9.1, reproducing all 272 rows of
//! `task-5-bus-shapes.md` exactly, `c=` and `dpc=` included):
//!
//! ```text
//! se = src ext words   de = dst ext words   N = 1 + se + de
//! nr = src is memory ? words-per-operand : 0
//! nw = dst is memory ? words-per-operand : 0
//!
//! if src mode is -(An), (d8,An,Xn) or (d8,PC,Xn):  2 idle cycles
//! b = src is memory ? min(de, 1) : de
//! c = N - se - b
//!   se program fetches              (the source's extension words; the
//!                                    opcode's own queue advance is counted in
//!                                    `c`, NOT here — `se`, not `se + 1`)
//!   nr source reads
//!   if dst mode is (d8,An,Xn):  2 idle cycles
//!   b program fetches
//!   if dst is -(An):  c program fetches, then nw writes
//!   else:             nw writes, then c program fetches
//!
//! cycles = 4 * (nr + nw + N) + idle        dpc = 2 * N
//! ```
//!
//! Note the fetch count is `N` — one per instruction word — because each is the
//! prefetch queue advancing by one. They land at `opcode_addr + 4 + 2i`, two
//! words ahead of the word being consumed, which is what makes it possible for
//! an operand access to precede the fetch that refills the queue behind it.
//! That is also why [`crate::ea::resolve`] takes its extension words as values
//! rather than fetching them.

use crate::cpu::M68k;
use crate::decode::Handler;
use crate::ea::{self, mode_is_mem, Ea, Size};
use crate::exception::{self, FaultKind, Space, ADDRESS_ERROR_TAIL_CYCLES};
use crate::Bus;

/// Both operands' addressing modes, decoded from the opcode.
///
/// MOVE packs the destination in bits 11-6 with its mode and register fields
/// **swapped** relative to the source's bits 5-0 — a decoding trap worth naming.
#[derive(Clone, Copy)]
struct Operands {
    src_mode: u16,
    src_reg: u16,
    dst_mode: u16,
    dst_reg: u16,
}

impl Operands {
    fn decode(op: u16) -> Self {
        Self {
            src_mode: (op >> 3) & 7,
            src_reg: op & 7,
            dst_mode: (op >> 6) & 7,
            dst_reg: (op >> 9) & 7,
        }
    }

    /// MOVEA's destination is an address register, encoded only by the register
    /// field — the mode field holds the size instead.
    fn decode_movea(op: u16) -> Self {
        Self {
            src_mode: (op >> 3) & 7,
            src_reg: op & 7,
            dst_mode: 1,
            dst_reg: (op >> 9) & 7,
        }
    }

    /// True when the source is not a memory operand — a register *or* an
    /// immediate. The write-fault CCR table keys on this rather than on a
    /// register-only test, because immediates follow the register rules in all
    /// four destination modes where they occur alongside a long write fault
    /// (counts in [`set_write_fault_flags`]).
    fn src_is_not_mem(&self) -> bool {
        self.src_mode <= 1 || (self.src_mode == 7 && self.src_reg == 4)
    }
}

/// Does resolving this source mode cost a leading 2-cycle idle? The
/// predecrement and both indexed modes need an internal step before the address
/// is ready.
fn src_leads_with_idle(mode: u16, reg: u16) -> bool {
    mode == 4 || mode == 6 || (mode == 7 && reg == 3)
}

/// Address-error alignment check. Byte accesses are never misaligned; word and
/// long accesses fault on an odd address. For a long only the base is checked —
/// a long at an even address has both its words even.
#[inline]
fn misaligned(addr: u32, size: Size) -> bool {
    size != Size::Byte && addr & 1 != 0
}

/// Words a memory operand of this size occupies on the bus.
#[inline]
fn operand_words(size: Size) -> u32 {
    if size == Size::Long {
        2
    } else {
        1
    }
}

/// The PC to stack in an address-error frame, per addendum §8 (4,661/4,661).
///
/// Deliberately not derived from `cpu.pc` at fault time — the two differ. The
/// measured rule is in terms of the opcode address plus a bump depending on
/// direction, source mode and size. Four things here each cost a failing bucket
/// to find, so none should be "simplified":
///
/// - Read faults do **not** add the source's extension words in general — only
///   the two absolute modes do. `d16(An)`, `(d8,An,Xn)`, `d16(PC)` and
///   `(d8,PC,Xn)` each consume an extension word yet still stack `opcode + 2`.
/// - `-(An)` as a source splits on **size**: `.w` stacks `+4`, `.l` stacks `+2`.
/// - Write faults **do** add `2 * src_ext` — the *source's* count. The
///   destination's extension words are fetched but do not move the stacked PC.
/// - An `abs.L` destination adds a further `+2`, but only when the source
///   performs no data-space read. See the note on the write arm: the immediate
///   half of that condition is reasoned, not measured.
fn stacked_pc(opcode_addr: u32, ops: &Operands, size: Size, kind: FaultKind, src_ext: u32) -> u32 {
    match kind {
        FaultKind::Read => {
            let bump = match (ops.src_mode, ops.src_reg) {
                (4, _) => {
                    if size == Size::Long {
                        0
                    } else {
                        2
                    }
                }
                (7, 0) => 2, // abs.W
                (7, 1) => 4, // abs.L
                _ => 0,
            };
            opcode_addr.wrapping_add(2).wrapping_add(bump)
        }
        FaultKind::Write => {
            // The `+2` keys on "the source performs no data-space read", which
            // is `src_is_not_mem` — the same predicate `set_write_fault_flags`
            // uses, and for the same reason: a register needs no fetch and an
            // immediate arrives through *program* space, so neither issues the
            // data read that a memory source does.
            //
            // UNMEASURED for immediates, and unmeasurable from this suite: a
            // register source always has `src_ext == 0` and an immediate always
            // has `src_ext >= 1`, so the two can never land in the same bucket.
            // Measured buckets are `(se=0, de=2, reg) -> +2` (14 cases) and
            // `(se=0..2, de=2, mem) -> +0` (13 cases); there is **no
            // `abs.L`-destination immediate-source write fault anywhere in
            // MOVE.w or MOVE.l**. Decided by analogy with §10.1, where
            // immediates provably follow the register column in all four
            // destination modes that do have cases. If a later group ever
            // produces such a case, trust it over this comment.
            let dst_is_abs_long = ops.dst_mode == 7 && ops.dst_reg == 1;
            let bonus = if dst_is_abs_long && ops.src_is_not_mem() {
                2
            } else {
                0
            };
            opcode_addr
                .wrapping_add(4)
                .wrapping_add(2 * src_ext)
                .wrapping_add(bonus)
        }
    }
}

/// Address space of a faulting source read: program space iff the source is
/// PC-relative. 177 such faults occur in the MOVE groups, and this rule is
/// 4,661/4,661 (addendum §9.4). A destination write is always data space.
fn src_space(mode: u16, reg: u16) -> Space {
    if mode == 7 && (reg == 2 || reg == 3) {
        Space::Program
    } else {
        Space::Data
    }
}

/// CCR for a completed MOVE: N and Z from the moved value, V and C cleared, X
/// preserved.
fn set_move_flags(cpu: &mut M68k, val: u32, size: Size) {
    let n = val & size.msb() != 0;
    let z = val & size.mask() == 0;
    cpu.set_ccr(cpu.ccr_x(), n, z, false, false);
}

/// CCR for a MOVE whose **destination write** faulted.
///
/// A read fault leaves the CCR untouched — exact in both measured buckets,
/// 839/839 for `MOVE.w` and 869/869 for `MOVE.l` (addendum §7b) — so this is
/// only reached on a write fault, where the flag update has already happened
/// even though the write never commits. Three rules appear, all preserving X:
///
/// ```text
/// A = CCR fully preserved
/// B = N/Z set, V and C cleared        (the ordinary MOVE update)
/// C = N/Z set, V and C preserved
/// ```
///
/// and, for a long, **which half of the operand** N and Z describe. The flags on
/// a faulting long reflect one 16-bit word, never the whole 32 bits: the write is
/// two bus cycles and the CCR shows only the one that was in flight.
///
/// `MOVE.w` is rule B on the operand, unconditionally — all 648 write-fault
/// cases. `MOVE.l` needs a table keyed on the destination mode and on whether
/// the source reads memory — all 618 write-fault cases:
///
/// ```text
///   destination     register source    memory source
///   (An)            A                  B on the low word
///   (An)+           A                  B on the low word
///   -(An)           B on the high word B on the high word
///   d16(An)         C on the high word B on the high word
///   (d8,An,Xn)      C on the high word B on the high word
///   abs.W           B on the high word B on the high word
///   abs.L           B on the high word B on the low word
/// ```
///
/// Which word appears is *not* the word the faulting access carried — `-(An)`
/// writes descending, so its faulting access carries the low word, yet its flags
/// describe the high one. Verified on the 250 long cases where the two words
/// disagree in N or Z, so neither half is a coincidence.
///
/// Row support, counted (the 618 `MOVE.l` write faults sum exactly across the
/// 14 rows, so no row is silently unpopulated):
///
/// ```text
///                   non-mem source   memory source
///   (An)                        49              60
///   (An)+                       45              61
///   -(An)                       54              54
///   d16(An)                     61              60
///   (d8,An,Xn)                  54              88
///   abs.W                        4              12
///   abs.L                        8               8
/// ```
///
/// The two rule-A rows together rest on 94 cases. **The `abs` rows rest on 4 to
/// 12 cases each — `abs.W` with a non-memory source is the thinnest row in the
/// table at 4.** Every row is unanimous, but these four rows are the ones a
/// future suite disagreement is most likely to overturn, and it should be
/// believed over this table.
///
/// Splitting the non-memory column into register and immediate shows the
/// immediate sub-rows following the register column wherever they have cases:
/// `(An)` 1/1 and `(An)+` 4/4 fully preserve the CCR (rule A), `d16(An)` 2/2
/// never clear V or C (rule C), and `-(An)` 1/1 does clear them (rule B). Only
/// `abs.W` and `abs.L` have no immediate-source case at all, which is why the
/// predicate below is `src_is_not_mem` rather than a register-only test.
fn set_write_fault_flags(cpu: &mut M68k, val: u32, size: Size, ops: &Operands) {
    if size != Size::Long {
        set_move_flags(cpu, val, size);
        return;
    }
    if !ops.src_is_not_mem() {
        // A memory source shows the low word for (An), (An)+ and abs.L, the
        // high word everywhere else. Note abs.L differs between the two source
        // kinds, so this selection cannot be hoisted out of the branch.
        let dst_abs_long = ops.dst_mode == 7 && ops.dst_reg == 1;
        let word = if matches!(ops.dst_mode, 2 | 3) || dst_abs_long {
            val & 0xFFFF
        } else {
            val >> 16
        };
        cpu.set_ccr(cpu.ccr_x(), word & 0x8000 != 0, word == 0, false, false);
        return;
    }
    // A register or immediate source always shows the high word.
    let n = val & 0x8000_0000 != 0;
    let z = val >> 16 == 0;
    match ops.dst_mode {
        // (An) and (An)+ preserve the CCR entirely.
        2 | 3 => {}
        // d16(An) and (d8,An,Xn) set N/Z but leave V and C alone.
        5 | 6 => cpu.set_ccr(cpu.ccr_x(), n, z, cpu.ccr_v(), cpu.ccr_c()),
        _ => cpu.set_ccr(cpu.ccr_x(), n, z, false, false),
    }
}

/// Undoes the part of a postincrement/predecrement adjustment that a faulting
/// MOVE does **not** keep (addendum §9.3, 32,627/32,627 over A0-A6).
///
/// [`ea::resolve`] commits every adjustment as it forms the address, which is
/// what the vectors show for most buckets. Three buckets disagree, and they are
/// tabled here as measured constants — two of them (`(An)+ .l` on a read fault,
/// `-(An) .l` on a write fault) have no derivation, but each rests on 106-152
/// unanimous cases:
///
/// ```text
///                        read fault           write fault
/// source (An)+ .w        +2  (kept)           +size (kept)
/// source (An)+ .l         0  (rolled back)    +size (kept)
/// source -(An)           -size (kept)         -size (kept)
/// dest   (An)+           — not resolved        0  (rolled back)
/// dest   -(An) .w        — not resolved       -2  (kept)
/// dest   -(An) .l        — not resolved        0  (rolled back)
/// ```
fn roll_back_fault_adjustment(
    cpu: &mut M68k,
    ops: &Operands,
    size: Size,
    kind: FaultKind,
    is_movea: bool,
) {
    match kind {
        // Only the source has been resolved, so only it can need a rollback.
        FaultKind::Read => {
            if ops.src_mode == 3 && size == Size::Long {
                let r = ops.src_reg as usize;
                cpu.a[r] = cpu.a[r].wrapping_sub(4);
            }
        }
        // MOVEA's destination is a register, so it has no adjustment at all.
        FaultKind::Write if !is_movea => {
            let r = ops.dst_reg as usize;
            match ops.dst_mode {
                3 => cpu.a[r] = cpu.a[r].wrapping_sub(size.step(r)),
                4 if size == Size::Long => cpu.a[r] = cpu.a[r].wrapping_add(4),
                _ => {}
            }
        }
        FaultKind::Write => {}
    }
}

/// The IR to stack, per addendum §9.5c (4,661/4,661).
///
/// IR is the opcode in every case **except** a word-sized write fault into
/// `-(An)`, where the pipeline has advanced `1 + src_ext` words further along
/// the instruction word stream. The words are
/// `[prefetch[0], prefetch[1], each program fetch in schedule order]` as they
/// stood at the start of the instruction.
///
/// An earlier hypothesis — that IR is simply `prefetch[0]` after the pipeline
/// advances — misses 2,484 of 4,661 cases. Do not retry it.
fn write_fault_ir(words: &[u16], ops: &Operands, size: Size, src_ext: u32) -> u16 {
    let d = if ops.dst_mode == 4 && size != Size::Long {
        1 + src_ext
    } else {
        0
    };
    words[d as usize]
}

/// Runs the shared MOVE/MOVEA schedule.
///
/// `is_movea` selects the destination decoding, suppresses the flag update, and
/// forces the destination write to be a full 32-bit address-register write.
fn move_common(cpu: &mut M68k, bus: &mut dyn Bus, op: u16, size: Size, is_movea: bool) -> u32 {
    let ops = if is_movea {
        Operands::decode_movea(op)
    } else {
        Operands::decode(op)
    };
    let opcode_addr = cpu.pc.wrapping_sub(exception::OPCODE_PC_OFFSET);

    // MOVEA's destination is An, so the *source* size governs the operand while
    // the destination always takes 32 bits.
    let se = ea::ext_words(ops.src_mode, ops.src_reg, size);
    let de = ea::ext_words(ops.dst_mode, ops.dst_reg, size);
    let n_words = 1 + se + de;
    let src_mem = mode_is_mem(ops.src_mode, ops.src_reg);
    let dst_mem = mode_is_mem(ops.dst_mode, ops.dst_reg);
    let nr = if src_mem { operand_words(size) } else { 0 };
    let nw = if dst_mem { operand_words(size) } else { 0 };

    // The instruction's whole word stream, in order. Slots 0 and 1 are already
    // in the queue; the rest arrive as the queue advances. Collecting them up
    // front would emit the fetches too early, so they are appended as each
    // fetch happens and `words` doubles as the IR lookup table (§9.5c).
    let mut words = [0u16; 6];
    words[0] = cpu.prefetch[0];
    words[1] = cpu.prefetch[1];
    let mut n_have = 2u32;

    // The two idles are tracked separately because they are reached at
    // different points: a source read that faults never incurs the
    // destination's idle.
    let src_idle = if src_leads_with_idle(ops.src_mode, ops.src_reg) {
        2
    } else {
        0
    };
    let dst_idle = if ops.dst_mode == 6 { 2 } else { 0 };

    // Advances the queue by one word, recording the word that arrives.
    let mut fetches_done = 0u32;
    macro_rules! fetch {
        ($n:expr) => {
            for _ in 0..$n {
                cpu.consume_opcode_dyn(bus);
                if n_have < words.len() as u32 {
                    words[n_have as usize] = cpu.prefetch[1];
                    n_have += 1;
                }
                fetches_done += 1;
            }
        };
    }

    // --- The source's extension words. ------------------------------------
    // Only `se` fetches happen here, not `1 + se`: the opcode's own queue
    // advance is one of the `c` fetches at the end. `MOVE.b (A0),(A1)` performs
    // its read and write before *any* program fetch, because both extension
    // words it needs are zero and the opcode is already out of the queue.
    fetch!(se);
    let src_ea = ea::resolve(
        cpu,
        ops.src_mode,
        ops.src_reg,
        size,
        &words[1..1 + se as usize],
        opcode_addr.wrapping_add(2),
    );

    // --- The source read, or a read fault. --------------------------------
    if let Ea::Mem(addr) = src_ea {
        if misaligned(addr, size) {
            // Abort before touching the bus: the harness asserts the faulting
            // access is absent from the log. The CCR is left untouched
            // (1,708/1,708) and the destination EA is never resolved, so its
            // address register keeps its initial value (§9.3).
            roll_back_fault_adjustment(cpu, &ops, size, FaultKind::Read, is_movea);
            exception::address_error(
                cpu,
                bus,
                addr,
                FaultKind::Read,
                src_space(ops.src_mode, ops.src_reg),
                words[0],
                stacked_pc(opcode_addr, &ops, size, FaultKind::Read, se),
            );
            // Only the tail collapses on a halt; the fetches and the source idle
            // already reached the bus. Measured: `MOVE.w (A0),D0` with an odd A0
            // and an odd SSP halts with an empty bus log.
            return 4 * fetches_done
                + src_idle
                + exception::entry_cycles(cpu, 0, ADDRESS_ERROR_TAIL_CYCLES);
        }
    }
    let src_val = ea::read(cpu, bus, src_ea, size);

    // --- The destination's extension words, split around the write. -------
    let b = if src_mem { de.min(1) } else { de };
    let c = n_words - se - b;
    let dst_ext_at = opcode_addr.wrapping_add(2).wrapping_add(2 * se);
    fetch!(b);

    // Resolution must precede the remaining fetches: `-(An)` commits its
    // decrement here, and a fault after `c` fetches still shows it (§9.3).
    let dst_ea = ea::resolve(
        cpu,
        ops.dst_mode,
        ops.dst_reg,
        size,
        &words[1 + se as usize..1 + se as usize + de as usize],
        dst_ext_at,
    );

    let predec_dst = ops.dst_mode == 4;
    if predec_dst {
        fetch!(c);
    }

    if let Ea::Mem(addr) = dst_ea {
        if misaligned(addr, size) {
            // The flag update is deliberately *not* done before this point: a
            // faulting write leaves the CCR in a state that is sometimes the
            // initial one untouched, which an unconditional update ahead of the
            // check would have already destroyed. MOVEA sets no flags at all,
            // faulting or not (1,687/1,687).
            if !is_movea {
                set_write_fault_flags(cpu, src_val, size, &ops);
            }
            roll_back_fault_adjustment(cpu, &ops, size, FaultKind::Write, is_movea);
            // A predecrementing long writes descending (§9.2), so the access
            // that faults is the *low* word and the frame stacks `addr + 2` —
            // 108/108, against `addr` in every ascending destination.
            let fault_addr = if predec_dst && size == Size::Long {
                addr.wrapping_add(2)
            } else {
                addr
            };
            exception::address_error(
                cpu,
                bus,
                fault_addr,
                FaultKind::Write,
                Space::Data,
                write_fault_ir(&words, &ops, size, se),
                stacked_pc(opcode_addr, &ops, size, FaultKind::Write, se),
            );
            // As the source-fault arm above: the lead is owed, the tail is not.
            // Measured: `MOVE.w D0,(A0)` with an odd A0 and an odd SSP halts with
            // an empty bus log.
            return 4 * (fetches_done + nr)
                + src_idle
                + dst_idle
                + exception::entry_cycles(cpu, 0, ADDRESS_ERROR_TAIL_CYCLES);
        }
        // `-(An)` is the one destination that writes a long descending
        // (§9.2, 147/147).
        if predec_dst && size == Size::Long {
            ea::write_predec_long(bus, addr, src_val);
        } else {
            ea::write(cpu, bus, dst_ea, size, src_val);
        }
    } else if is_movea {
        // MOVEA sign-extends a word source into the full 32-bit register.
        cpu.a[ops.dst_reg as usize] = size.sign_extend(src_val);
    } else {
        ea::write(cpu, bus, dst_ea, size, src_val);
    }

    if !is_movea {
        set_move_flags(cpu, src_val, size);
    }

    if !predec_dst {
        fetch!(c);
    }

    debug_assert_eq!(fetches_done, n_words, "schedule must issue N fetches");
    4 * (nr + nw + n_words) + src_idle + dst_idle
}

fn move_byte(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    move_common(cpu, bus, op, Size::Byte, false)
}
fn move_word(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    move_common(cpu, bus, op, Size::Word, false)
}
fn move_long(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    move_common(cpu, bus, op, Size::Long, false)
}
fn movea_word(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    move_common(cpu, bus, op, Size::Word, true)
}
fn movea_long(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    move_common(cpu, bus, op, Size::Long, true)
}

/// `MOVEQ #d8,Dn` — sign-extends an 8-bit immediate held in the opcode itself,
/// so it needs no extension word and no operand access.
fn moveq(cpu: &mut M68k, bus: &mut dyn Bus, op: u16) -> u32 {
    let val = op as u8 as i8 as i32 as u32;
    cpu.d[((op >> 9) & 7) as usize] = val;
    set_move_flags(cpu, val, Size::Long);
    cpu.consume_opcode_dyn(bus);
    4
}

/// True if `(mode, reg)` is a legal source for a MOVE of this size.
///
/// Byte operations cannot touch an address register: there is no such thing as
/// a byte of `An`.
fn valid_src(mode: u16, reg: u16, size: Size) -> bool {
    match mode {
        1 => size != Size::Byte,
        7 => reg <= 4,
        _ => true,
    }
}

/// True if `(mode, reg)` is a legal destination for a MOVE.
///
/// Destinations exclude `An` (that encoding is MOVEA), the PC-relative modes and
/// immediates — none of which can be written.
fn valid_dst(mode: u16, reg: u16) -> bool {
    match mode {
        1 => false,
        7 => reg <= 1,
        _ => true,
    }
}

/// Installs MOVE, MOVEA and MOVEQ into the dispatch table.
///
/// Opcode layout: bits 15-14 are `00`, bits 13-12 select the size
/// (`01` = byte, `11` = word, `10` = long), then the destination and source EA
/// fields. A word- or long-sized MOVE whose destination mode field reads `001`
/// is MOVEA; there is no byte MOVEA.
pub fn register(table: &mut [Handler; 65536]) {
    for (size_bits, size) in [(1u16, Size::Byte), (3, Size::Word), (2, Size::Long)] {
        for dst_reg in 0..8u16 {
            for dst_mode in 0..8u16 {
                for src_mode in 0..8u16 {
                    for src_reg in 0..8u16 {
                        if !valid_src(src_mode, src_reg, size) {
                            continue;
                        }
                        let op = (size_bits << 12)
                            | (dst_reg << 9)
                            | (dst_mode << 6)
                            | (src_mode << 3)
                            | src_reg;
                        let handler: Handler = if dst_mode == 1 {
                            match size {
                                // MOVEA.b does not exist.
                                Size::Byte => continue,
                                Size::Word => movea_word,
                                Size::Long => movea_long,
                            }
                        } else if !valid_dst(dst_mode, dst_reg) {
                            continue;
                        } else {
                            match size {
                                Size::Byte => move_byte,
                                Size::Word => move_word,
                                Size::Long => move_long,
                            }
                        };
                        table[op as usize] = handler;
                    }
                }
            }
        }
    }

    // MOVEQ: 0111 rrr 0 dddddddd. Bit 8 must be 0.
    for reg in 0..8u16 {
        for imm in 0..256u16 {
            table[(0x7000 | (reg << 9) | imm) as usize] = moveq;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::tests_support::{FlatBus, RecordingBus};
    use crate::decode::Decoder;

    /// Builds a CPU sitting at 0x1000 with the queue primed, in supervisor mode.
    /// The instruction words come from `bus.load` at the call site, so this only
    /// primes the queue from whatever is already there.
    fn at(bus: &mut impl Bus) -> M68k {
        let mut cpu = M68k::new();
        cpu.sr = crate::cpu::SR_S;
        cpu.a[7] = 0x3000;
        cpu.pc = 0x1000;
        cpu.prime_prefetch(bus);
        cpu
    }

    /// A halted MOVE fault is charged for its fetches and nothing else.
    ///
    /// Both fault arms — source and destination — returned
    /// [`ADDRESS_ERROR_TAIL_CYCLES`] unconditionally, so a double bus fault was
    /// charged 58 cycles for twelve accesses it never made. The fetches stay
    /// outside [`exception::entry_cycles`] because they did happen.
    ///
    /// Both arms are covered, because they are separate returns and fixing one
    /// leaves the other wrong: `MOVE.w (A0),D0` faults on the source read,
    /// `MOVE.w D0,(A0)` on the destination write. The `(A0)` forms have **no**
    /// lead — the alignment check precedes every fetch, and the queue advance is
    /// at the tail — so the `(d16,A0)` forms are included to give a nonzero one.
    /// Without them the lead term is unobservable and dropping it into
    /// `entry_cycles`' first argument would pass, which is exactly the mistake
    /// `pea`'s halt arm made.
    ///
    /// Extrapolated: 0 of 317,500 cases halt.
    #[test]
    fn a_halted_move_fault_costs_only_its_fetches() {
        for (label, prog, lead) in [
            // No lead at all: the alignment check precedes every fetch.
            ("source", &[0x3010u16, 0x4E71, 0x4E71][..], 0),
            ("destination", &[0x3080, 0x4E71, 0x4E71][..], 0),
            // `(d16,A0)` fetches its displacement first, so the lead is 1 and the
            // two spellings differ by 4. Without this row the test pins only the
            // tail collapse and a lead dropped into `entry_cycles` would pass.
            ("source+ext", &[0x3028, 0x0000, 0x4E71][..], 1),
            ("destination+ext", &[0x3140, 0x0000, 0x4E71][..], 1),
        ] {
            let mut bus = RecordingBus::new();
            bus.load(0x1000, prog);
            bus.put16(0x000C, 0x0000); // vector 3, so a frame would be visible
            bus.put16(0x000E, 0x2000);
            let mut cpu = M68k::new();
            cpu.sr = crate::cpu::SR_S;
            cpu.a[0] = 0x4001; // odd operand address
            cpu.a[7] = 0x3001; // odd frame base
            cpu.ssp = 0x3001;
            cpu.pc = 0x1000;
            cpu.prime_prefetch(&mut bus);
            bus.log.clear();

            let cycles = cpu.step_with(&Decoder::new(), &mut bus);

            assert!(cpu.halted, "{label}: an odd frame base halts");
            assert_eq!(bus.writes(), vec![], "{label}: no frame was written");
            assert_eq!(bus.log.len(), lead as usize, "{label}: lead accesses only");
            assert_eq!(
                cycles,
                4 * lead + exception::HALTED_IDLE_CYCLES,
                "{label}: 4 × lead + the halt idle, not the framed 58"
            );
        }
    }

    #[test]
    fn moveq_sign_extends_and_sets_flags() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x70FF, 0x4E71]); // MOVEQ #-1,D0
        let mut cpu = at(&mut bus);
        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.d[0], 0xFFFF_FFFF);
        assert!(cpu.ccr_n() && !cpu.ccr_z() && !cpu.ccr_v() && !cpu.ccr_c());
        assert_eq!(cycles, 4);
        assert_eq!(cpu.pc, 0x1006, "one word consumed, queue stays 4 ahead");
    }

    /// `MOVE.b (A0),(A1)` — the schedule is `r w p0`: the operand accesses come
    /// first and the queue advance last. A resolve-read-write implementation
    /// would emit the fetch first and fail here.
    #[test]
    fn move_byte_memory_to_memory_emits_read_write_then_fetch() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x1290, 0x4E71, 0x4E71]); // MOVE.b (A0),(A1)
        bus.put16(0x2000, 0xAB00);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2000;
        cpu.a[1] = 0x2100;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.log,
            vec![
                (false, 0x2000, 0xAB),   // the source read
                (true, 0x2100, 0xAB),    // the destination write
                (false, 0x1004, 0x4E71), // the queue advancing
            ],
            "bus order must be r w p0"
        );
        assert_eq!(cycles, 12, "3 accesses, no idle");
    }

    /// `MOVE.w D0,-(A1)` — a predecrementing destination writes *after* the
    /// final program fetch, the reverse of every other mode.
    #[test]
    fn predecrement_destination_writes_after_the_final_fetch() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x3300, 0x4E71, 0x4E71]); // MOVE.w D0,-(A1)
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x1234;
        cpu.a[1] = 0x2100;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.log,
            vec![(false, 0x1004, 0x4E71), (true, 0x20FE, 0x1234)],
            "bus order must be p0 w"
        );
        assert_eq!(cpu.a[1], 0x20FE);
        assert_eq!(cycles, 8);
    }

    /// A `MOVE.l` into `-(An)` writes the low word first — the one descending
    /// 32-bit write among the MOVE destinations (addendum §9.2).
    #[test]
    fn move_long_into_predecrement_writes_low_word_first() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x2300, 0x4E71, 0x4E71]); // MOVE.l D0,-(A1)
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0xAAAA_BBBB;
        cpu.a[1] = 0x2100;
        bus.log.clear();

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.log,
            vec![
                (false, 0x1004, 0x4E71),
                (true, 0x20FE, 0xBBBB),
                (true, 0x20FC, 0xAAAA),
            ],
            "low word first, then high"
        );
    }

    /// An odd word source aborts before the read reaches the bus, and stacks a
    /// 7-word frame through vector 3.
    #[test]
    fn odd_source_raises_an_address_error_without_reading() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x3010, 0x4E71]); // MOVE.w (A0),D0
        bus.put16(0x000C, 0x0000); // vector 3
        bus.put16(0x000E, 0x4000);
        bus.load(0x4000, &[0x4E71, 0x4E71]); // the handler
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2001; // odd
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert!(
            !bus.reads().iter().any(|&(a, _)| a == 0x2000 || a == 0x2001),
            "the faulting access must never reach the bus"
        );
        assert_eq!(cpu.a[7], 0x3000 - 14, "7-word frame");
        assert_eq!(cpu.pc, 0x4004, "vectored through 3, then refilled");
        // frame, low address to high:
        //   status, fault_hi, fault_lo, IR, SR, PC_hi, PC_lo
        assert_eq!(bus.read16(0x2FF4), 0x0000, "fault address high");
        assert_eq!(bus.read16(0x2FF6), 0x2001, "fault address low");
        assert_eq!(bus.read16(0x2FF8), 0x3010, "IR is the opcode");
        assert_eq!(bus.read16(0x2FFE), 0x1002, "stacked PC = opcode + 2");
        // status = (IR & 0xFFE0) | read | fc, fc = supervisor|data = 5
        assert_eq!(bus.read16(0x2FF2), (0x3010 & 0xFFE0) | 0x10 | 5);
        assert_eq!(
            cycles, ADDRESS_ERROR_TAIL_CYCLES,
            "no fetch precedes the fault, so only the tail"
        );
    }

    /// A read fault leaves the CCR untouched; a write fault does not.
    #[test]
    fn read_fault_preserves_the_ccr() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x3010, 0x4E71]); // MOVE.w (A0),D0
        bus.put16(0x000E, 0x2000);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2001;
        cpu.sr |= crate::cpu::SR_C | crate::cpu::SR_V;
        let before = cpu.sr & 0x1F;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);
        assert_eq!(cpu.sr & 0x1F, before, "CCR untouched by a read fault");
    }

    /// MOVEA sets no flags, and sign-extends a word source into all 32 bits.
    #[test]
    fn movea_word_sign_extends_and_leaves_flags_alone() {
        let mut bus = FlatBus::new();
        bus.load(0x1000, &[0x3240, 0x4E71]); // MOVEA.w D0,A1
        let mut cpu = at(&mut bus);
        cpu.d[0] = 0x0000_8001;
        cpu.a[1] = 0x1111_1111;
        cpu.sr |= crate::cpu::SR_C;
        let before = cpu.sr & 0x1F;

        let dec = Decoder::new();
        cpu.step_with(&dec, &mut bus);

        assert_eq!(cpu.a[1], 0xFFFF_8001, "sign-extended");
        assert_eq!(cpu.sr & 0x1F, before, "MOVEA never touches the CCR");
    }

    /// `MOVE.b (A0),abs.L` needs both halves of the 32-bit address but performs
    /// only one program fetch before the write — the second half is already in
    /// the prefetch queue. This is the case that forces `resolve` to take its
    /// extension words as values rather than fetching them.
    #[test]
    fn absolute_long_destination_uses_a_queued_extension_word() {
        let mut bus = RecordingBus::new();
        bus.load(0x1000, &[0x13D0, 0x0000, 0x2100, 0x4E71, 0x4E71]); // MOVE.b (A0),$2100
        bus.put16(0x2000, 0xCD00);
        let mut cpu = at(&mut bus);
        cpu.a[0] = 0x2000;
        bus.log.clear();

        let dec = Decoder::new();
        let cycles = cpu.step_with(&dec, &mut bus);

        assert_eq!(
            bus.log,
            vec![
                (false, 0x2000, 0xCD),   // r
                (false, 0x1004, 0x2100), // p0
                (true, 0x2100, 0xCD),    // w
                (false, 0x1006, 0x4E71), // p2
                (false, 0x1008, 0x4E71), // p4
            ],
            "schedule r p0 w p2 p4"
        );
        assert_eq!(cycles, 20);
        assert_eq!(cpu.pc, 0x100A, "dpc = 6, plus the 4-byte queue lead");
    }
}
