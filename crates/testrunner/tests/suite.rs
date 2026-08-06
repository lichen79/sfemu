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
