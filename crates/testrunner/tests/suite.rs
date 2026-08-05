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
