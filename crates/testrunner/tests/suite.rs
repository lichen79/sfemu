//! One test per opcode group in the SingleStepTests/m68000 suite.
//!
//! Groups are enabled as the core learns to execute them. Run with `--release`:
//! 2500 cases per group is slow in a debug build.

use testrunner::runner::assert_group;

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
