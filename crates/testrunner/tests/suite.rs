//! One test per opcode group in the SingleStepTests/m68000 suite.
//!
//! Groups are enabled as the core learns to execute them. Run with `--release`:
//! 2500 cases per group is slow in a debug build.

use testrunner::runner::{assert_group, assert_known_bad};

macro_rules! group {
    ($fname:ident, $name:literal) => {
        #[test]
        fn $fname() {
            assert_group($name);
        }
    };
}

group!(illegal_line_a, "ILLEGAL_LINEA");
group!(illegal_line_f, "ILLEGAL_LINEF");

// Task 5: effective addressing and MOVE. MOVE.b and MOVEQ contain no
// address-error cases, so they exercise the EA layer and the bus schedule in
// isolation; the other four carry all 4,661 of them.
group!(move_b, "MOVE.b");
group!(move_w, "MOVE.w");
group!(move_l, "MOVE.l");
group!(move_q, "MOVE.q");
group!(movea_w, "MOVEA.w");
group!(movea_l, "MOVEA.l");

// Task 6: arithmetic, logic, and flags.
//
// The suite has no `ADDI`, `ADDQ` or `CMPI` groups: those encodings are folded
// into the sized groups below, so `ADD.b` alone covers `ADD`, `ADDI` and `ADDQ`
// at byte size (1,561 + 116 + 823 of its 2,500 cases). A group named for one
// instruction therefore exercises three handlers, and the `<ea> op Dn` /
// `Dn op <ea>` / immediate / quick schedules must all be right before it passes.
group!(add_b, "ADD.b");
group!(add_w, "ADD.w");
group!(add_l, "ADD.l");
group!(adda_w, "ADDA.w");
group!(adda_l, "ADDA.l");
group!(addx_b, "ADDX.b");
group!(addx_w, "ADDX.w");
group!(addx_l, "ADDX.l");
group!(sub_b, "SUB.b");
group!(sub_w, "SUB.w");
group!(sub_l, "SUB.l");
group!(suba_w, "SUBA.w");
group!(suba_l, "SUBA.l");
group!(subx_b, "SUBX.b");
group!(subx_w, "SUBX.w");
group!(subx_l, "SUBX.l");
// CMP.* also carry the CMPM cases; the CMP line's opmode 4/5/6 mode 1 is CMPM.
group!(cmp_b, "CMP.b");
group!(cmp_w, "CMP.w");
group!(cmp_l, "CMP.l");
group!(cmpa_w, "CMPA.w");
group!(cmpa_l, "CMPA.l");
group!(neg_b, "NEG.b");
group!(neg_w, "NEG.w");
group!(neg_l, "NEG.l");
group!(negx_b, "NEGX.b");
group!(negx_w, "NEGX.w");
group!(negx_l, "NEGX.l");
group!(clr_b, "CLR.b");
group!(clr_w, "CLR.w");
group!(clr_l, "CLR.l");
group!(tst_b, "TST.b");
group!(tst_w, "TST.w");
group!(tst_l, "TST.l");
group!(and_b, "AND.b");
group!(and_w, "AND.w");
group!(and_l, "AND.l");
group!(or_b, "OR.b");
group!(or_w, "OR.w");
group!(or_l, "OR.l");
// EOR.* live in the CMP line's opmode 4/5/6 at every mode except 1.
group!(eor_b, "EOR.b");
group!(eor_w, "EOR.w");
group!(eor_l, "EOR.l");
group!(not_b, "NOT.b");
group!(not_w, "NOT.w");
group!(not_l, "NOT.l");
// The to-CCR forms are unprivileged; the to-SR forms fault in user mode, so
// each of these groups is really two tests in one.
group!(andi_to_ccr, "ANDItoCCR");
group!(andi_to_sr, "ANDItoSR");
group!(eori_to_ccr, "EORItoCCR");
group!(eori_to_sr, "EORItoSR");
group!(ori_to_ccr, "ORItoCCR");
group!(ori_to_sr, "ORItoSR");
group!(nop, "NOP");

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
group!(asl_b, "ASL.b");
group!(asl_w, "ASL.w");
group!(asl_l, "ASL.l");
group!(asr_b, "ASR.b");
group!(asr_w, "ASR.w");
group!(asr_l, "ASR.l");
group!(lsl_b, "LSL.b");
group!(lsl_w, "LSL.w");
group!(lsl_l, "LSL.l");
group!(lsr_b, "LSR.b");
group!(lsr_w, "LSR.w");
group!(lsr_l, "LSR.l");
group!(rol_b, "ROL.b");
group!(rol_w, "ROL.w");
group!(rol_l, "ROL.l");
group!(ror_b, "ROR.b");
group!(ror_w, "ROR.w");
group!(ror_l, "ROR.l");
group!(roxl_b, "ROXL.b");
group!(roxl_w, "ROXL.w");
group!(roxl_l, "ROXL.l");
group!(roxr_b, "ROXR.b");
group!(roxr_w, "ROXR.w");
group!(roxr_l, "ROXR.l");
// One group per bit instruction, each holding both bit-number forms: the static
// form (bit number in an extension word) and the dynamic one (bit number in Dn).
group!(btst, "BTST");
group!(bset, "BSET");
group!(bclr, "BCLR");
group!(bchg, "BCHG");

// Task 8: branches, jumps, subroutine calls, and conditional set/decrement.
// `BRA` and `BSR` share the `Bcc` line's encoding but have their own groups; the
// `Bcc` group covers conditions 2-15 only.
group!(bcc, "Bcc");
group!(bsr, "BSR");
group!(dbcc, "DBcc");
group!(scc, "Scc");
group!(jmp, "JMP");
group!(jsr, "JSR");
group!(rts, "RTS");
group!(rtr, "RTR");

/// Known-bad upstream: `TAS`'s indivisible read-modify-write is not modelled by
/// the vector generator. Asserted as *partially* failing so an upstream fix
/// surfaces.
///
/// The failure is specifically in the *ordered* transactions — the vectors place
/// an idle between the read and the write, and never use the format's dedicated
/// `Tas` transaction kind at all. Every value the handler computes is confirmed
/// against these same vectors: the timing law holds 2,500/2,500, and so do the
/// predicted cycle count and the result-and-CCR prediction.
///
/// `assert_known_bad` rather than `#[should_panic]` around `assert_group`: the
/// latter passes when the vector file is missing, because the group's name appears
/// in that panic message too.
#[test]
fn tas_is_known_bad() {
    assert_known_bad(
        "TAS",
        "the generator models TAS's read-modify-write as read, idle, write and \
         never emits the format's Tas transaction kind",
    );
}
