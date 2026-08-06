//! One test per opcode group in the SingleStepTests/m68000 suite.
//!
//! Groups are enabled as the core learns to execute them. Run with `--release`:
//! 2500 cases per group is slow in a debug build.

use testrunner::runner::assert_group;

/// Emits one `#[test]` per group plus `REGISTERED`, the list every group name is
/// read back from.
///
/// One plural invocation rather than 127 singular ones, because `macro_rules!`
/// cannot accumulate across invocations and `every_vector_file_has_a_registered_group`
/// needs a single source of truth. That test is the reason this is a list at all:
/// four groups (`RTE`, `TAS`, `TRAP`, `TRAPV`) were silently absent for several
/// tasks, and nothing could have noticed.
///
/// Plain `//` comments inside the invocation are fine — the lexer strips them —
/// but a `///` doc comment is a compile error (it expands to `#[doc = "…"]`,
/// which the matcher rejects), and so is a missing trailing comma on the last
/// entry.
macro_rules! groups {
    ($($fname:ident => $name:literal,)*) => {
        $( #[test] fn $fname() { assert_group($name); } )*
        /// Every group name registered above — the completeness test's source of truth.
        const REGISTERED: &[&str] = &[$($name),*];
    };
}

groups! {
    illegal_line_a => "ILLEGAL_LINEA",
    illegal_line_f => "ILLEGAL_LINEF",

    // Task 5: effective addressing and MOVE. MOVE.b and MOVEQ contain no
    // address-error cases, so they exercise the EA layer and the bus schedule in
    // isolation; the other four carry all 4,661 of them.
    move_b => "MOVE.b",
    move_w => "MOVE.w",
    move_l => "MOVE.l",
    move_q => "MOVE.q",
    movea_w => "MOVEA.w",
    movea_l => "MOVEA.l",

    // Task 6: arithmetic, logic, and flags.
    //
    // The suite has no `ADDI`, `ADDQ` or `CMPI` groups: those encodings are folded
    // into the sized groups below, so `ADD.b` alone covers `ADD`, `ADDI` and `ADDQ`
    // at byte size (1,561 + 116 + 823 of its 2,500 cases). A group named for one
    // instruction therefore exercises three handlers, and the `<ea> op Dn` /
    // `Dn op <ea>` / immediate / quick schedules must all be right before it passes.
    add_b => "ADD.b",
    add_w => "ADD.w",
    add_l => "ADD.l",
    adda_w => "ADDA.w",
    adda_l => "ADDA.l",
    addx_b => "ADDX.b",
    addx_w => "ADDX.w",
    addx_l => "ADDX.l",
    sub_b => "SUB.b",
    sub_w => "SUB.w",
    sub_l => "SUB.l",
    suba_w => "SUBA.w",
    suba_l => "SUBA.l",
    subx_b => "SUBX.b",
    subx_w => "SUBX.w",
    subx_l => "SUBX.l",
    // CMP.* also carry the CMPM cases; the CMP line's opmode 4/5/6 mode 1 is CMPM.
    cmp_b => "CMP.b",
    cmp_w => "CMP.w",
    cmp_l => "CMP.l",
    cmpa_w => "CMPA.w",
    cmpa_l => "CMPA.l",
    neg_b => "NEG.b",
    neg_w => "NEG.w",
    neg_l => "NEG.l",
    negx_b => "NEGX.b",
    negx_w => "NEGX.w",
    negx_l => "NEGX.l",
    clr_b => "CLR.b",
    clr_w => "CLR.w",
    clr_l => "CLR.l",
    tst_b => "TST.b",
    tst_w => "TST.w",
    tst_l => "TST.l",
    and_b => "AND.b",
    and_w => "AND.w",
    and_l => "AND.l",
    or_b => "OR.b",
    or_w => "OR.w",
    or_l => "OR.l",
    // EOR.* live in the CMP line's opmode 4/5/6 at every mode except 1.
    eor_b => "EOR.b",
    eor_w => "EOR.w",
    eor_l => "EOR.l",
    not_b => "NOT.b",
    not_w => "NOT.w",
    not_l => "NOT.l",
    // The to-CCR forms are unprivileged; the to-SR forms fault in user mode, so
    // each of these groups is really two tests in one.
    andi_to_ccr => "ANDItoCCR",
    andi_to_sr => "ANDItoSR",
    eori_to_ccr => "EORItoCCR",
    eori_to_sr => "EORItoSR",
    ori_to_ccr => "ORItoCCR",
    ori_to_sr => "ORItoSR",
    nop => "NOP",

    // Task 7: shifts, rotates, and bit operations.
    //
    // A group named for one shift type contains only that type — `ASL.b` is 2,500
    // `ASL` cases and no `LSL` — provided the type field is read from bits 4-3.
    // Read from 5-4 it appears to be a mixture, which is the symptom of that
    // off-by-one and not a property of the suite.
    //
    // The eight `.w` groups are the only ones with a memory form (size `11`), so
    // they carry every address error in the task: 2,248 read faults, and no write
    // faults anywhere in Task 7.
    asl_b => "ASL.b",
    asl_w => "ASL.w",
    asl_l => "ASL.l",
    asr_b => "ASR.b",
    asr_w => "ASR.w",
    asr_l => "ASR.l",
    lsl_b => "LSL.b",
    lsl_w => "LSL.w",
    lsl_l => "LSL.l",
    lsr_b => "LSR.b",
    lsr_w => "LSR.w",
    lsr_l => "LSR.l",
    rol_b => "ROL.b",
    rol_w => "ROL.w",
    rol_l => "ROL.l",
    ror_b => "ROR.b",
    ror_w => "ROR.w",
    ror_l => "ROR.l",
    roxl_b => "ROXL.b",
    roxl_w => "ROXL.w",
    roxl_l => "ROXL.l",
    roxr_b => "ROXR.b",
    roxr_w => "ROXR.w",
    roxr_l => "ROXR.l",
    // One group per bit instruction, each holding both bit-number forms: the static
    // form (bit number in an extension word) and the dynamic one (bit number in Dn).
    btst => "BTST",
    bset => "BSET",
    bclr => "BCLR",
    bchg => "BCHG",

    // Task 8: branches, jumps, subroutine calls, and conditional set/decrement.
    //
    // `BRA` has NO group of its own -- there is no `BRA.json.bin`. Its coverage lives
    // inside the `Bcc` group, whose condition-nibble census over all 2,500 cases is
    // {0: 166, 2: 181, 3: 169, 4: 158, 5: 149, 6: 161, 7: 160, 8: 154, 9: 176,
    //  10: 169, 11: 189, 12: 184, 13: 177, 14: 141, 15: 166} -- i.e. 166 condition-0
    // (`BRA`) cases. An earlier version of this comment claimed `BRA` had its own
    // group and that `Bcc` covered "conditions 2-15 only"; both halves were wrong.
    // Only the `BSR` split holds: condition 1 is 0/2500 here and 2500/2500 in the
    // `BSR` group, since that encoding IS `BSR`.
    bcc => "Bcc",
    bsr => "BSR",
    dbcc => "DBcc",
    scc => "Scc",
    jmp => "JMP",
    jsr => "JSR",
    rts => "RTS",
    rtr => "RTR",

    // Task 9: multiply, divide, CHK, and BCD.
    //
    // The five `<ea>`-source groups carry 4,864 address errors between them, all of
    // them *read* faults — so `alu`'s read-only fault arm covers the whole task and
    // the write-fault arm stays unwritten rather than untested. `ABCD` and `SBCD`
    // have no address errors at all: both forms are byte-sized, and a byte access
    // never misaligns.
    //
    // `DIVU`/`DIVS` contain **no divide-by-zero case** — 0 of 1,546 and 0 of 1,518,
    // against a control recovering 1,525 and 1,502 distinct nonzero divisors. These
    // two going green says nothing about vector 5; `ops::muldiv`'s unit tests are its
    // only coverage.
    mulu => "MULU",
    muls => "MULS",
    divu => "DIVU",
    divs => "DIVS",
    chk => "CHK",
    abcd => "ABCD",
    sbcd => "SBCD",
    nbcd => "NBCD",

    // Task 10: system control and multi-register transfer.
    //
    // `MOVEM.w`/`MOVEM.l` are the only *write*-fault groups outside `MOVE` (595 and
    // 585), which is what finally exercises `alu`'s write arm. Every one of those
    // faults is on the first operand access: `MOVEM` steps its address by 2 or 4, so
    // parity is invariant and a mid-transfer fault cannot occur — 0 completed operand
    // accesses before the abort in all 2,378 faulting cases, with both off-diagonals
    // of the odd/even × faults/clean table exactly zero. So these two going green
    // says nothing about partial-transfer rollback, and there is deliberately no
    // rollback code to say anything about.
    //
    // Five groups here are *mostly* privilege traps rather than executions —
    // `MOVEtoSR` 1290, `MOVEtoUSP` 1226, `MOVEfromUSP` 1207, `STOP` 1270, `RESET`
    // 1267 user-mode cases. They would go green on the trap alone if the supervisor
    // path were wrong, so `ops::system`'s unit tests carry the supervisor semantics.
    // `STOP` in particular has an *empty* access shape, which no cycle count can
    // distinguish from a wrong one.
    //
    // `LINK`, `PEA`, `LEA`, `MOVEP.w` and `MOVEP.l` have **zero** address-error
    // cases: `MOVEP` is byte-sized on the bus whatever its suffix says, and the other
    // three touch memory only through A7, which the generator always starts even. For
    // those three the alignment check is therefore unverified by the suite — it is
    // reachable on hardware, just not sampled here.
    movem_w => "MOVEM.w",
    movem_l => "MOVEM.l",
    movep_w => "MOVEP.w",
    movep_l => "MOVEP.l",
    link => "LINK",
    unlink => "UNLINK",
    exg => "EXG",
    swap => "SWAP",
    ext_w => "EXT.w",
    ext_l => "EXT.l",
    pea => "PEA",
    lea => "LEA",
    stop_op => "STOP",
    reset_op => "RESET",
    move_from_sr => "MOVEfromSR",
    move_to_sr => "MOVEtoSR",
    move_to_ccr => "MOVEtoCCR",
    move_to_usp => "MOVEtoUSP",
    move_from_usp => "MOVEfromUSP",

    // Task 11: exceptions taken and returned from. The last four groups, and the
    // first three of them were entirely unimplemented rather than buggy — there
    // were no `0x4E4x`, `0x4E73` or `0x4E76` table entries at all. `RTE` in
    // particular was the most heavily measured instruction in this project's notes
    // while scoring 0/2500, which is why the completeness test below now exists:
    // depth of measurement said nothing about whether any of it had been written.
    //
    // `TAS` was labelled **known-bad upstream** and asserted as *partially*
    // failing at 392/2500. That label is retracted. The measured facts behind it
    // stand — the generator does model TAS's indivisible read-modify-write as an
    // ordinary read, idle, write, and never emits the format's dedicated `Tas`
    // transaction kind (0 of 2,500 cases carry one) — but the inference that this
    // made the group unmatchable was wrong. The vectors are merely unanimous about
    // an unusual ordering: the queue advance follows the write instead of preceding
    // it, alone among this core's memory-write forms. The 392 that passed were
    // exactly the register-destination cases, which have no write to misplace.
    //
    // `RTE` is the only group whose cases split three ways: 600 clean returns at 20
    // cycles, 1,286 privilege violations at 34, and 614 address errors at 70 where
    // the popped PC is odd. `TRAPV` splits 1250/1250 on the V flag. Each of those
    // buckets is a single access shape at a single cycle count, so a residue in any
    // of them is a bug and not an unmeasured case.
    tas => "TAS",
    trap => "TRAP",
    trapv => "TRAPV",
    rte => "RTE",
}

/// Every vector file must have a `group!` entry.
///
/// This is the direction that failed: four data files sat in `testdata/` with no
/// test naming them, and 123 green groups looked like a complete suite. The
/// reverse direction is already covered — [`assert_group`] panics on a missing
/// path — so a `group!` naming a nonexistent file cannot hide either.
///
/// The failure message names the missing groups rather than reporting a count,
/// because "123 ≠ 127" is what the old arithmetic already said and it did not
/// identify which four.
#[test]
fn every_vector_file_has_a_registered_group() {
    let dir = testrunner::runner::testdata_dir();
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e} — run `cargo run -p testrunner --bin fetch`",
            dir.display()
        )
    });
    let mut found: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".json.bin"))
                .map(str::to_owned)
        })
        .collect();
    found.sort();
    // A missing testdata/ must fail loudly, not vacuously pass with an empty set.
    assert!(
        !found.is_empty(),
        "no vector files in {} — run `cargo run -p testrunner --bin fetch`",
        dir.display()
    );
    let registered: std::collections::BTreeSet<&str> = REGISTERED.iter().copied().collect();
    let missing: Vec<&str> = found
        .iter()
        .map(String::as_str)
        .filter(|n| !registered.contains(n))
        .collect();
    assert!(
        missing.is_empty(),
        "{} vector file(s) have no `group!` entry: {missing:?}",
        missing.len()
    );
}
