//! Instruction handlers, one module per instruction family.

pub mod alu;
pub mod arith;
pub mod bits;
pub mod branch;
pub mod logic;
pub mod move_;
pub mod shift;

/// Installs every implemented handler into the dispatch table.
///
/// Opcodes left untouched keep the table's default `illegal_instruction`
/// handler, so a partially populated table is safe — an unimplemented opcode
/// raises an emulated illegal-instruction exception rather than misbehaving.
///
/// [`arith`] and [`logic`] share encoding space in three places — the `0000`
/// `xxxI` line, the `1011` line's opmode 4/5/6 (`CMPM` against `EOR`), and the
/// `0100` single-operand line — so each registers only the opcodes it owns and
/// the call order here does not matter. See their module docs for the split.
/// [`bits`] joins the first of those: the `0000` line holds `xxxI`, `MOVEP` and
/// the four bit instructions, split by bit 8 and by bits 11-9.
///
/// [`branch`] shares two more lines: the `0101` line, where it takes size `11`
/// (`Scc`/`DBcc`) and [`arith`] takes the rest (`ADDQ`/`SUBQ`); and the `0100`
/// line, where it takes only `4E75`, `4E77` and `4E80`-`4EFF`.
pub fn register_all(table: &mut [crate::decode::Handler; 65536]) {
    move_::register(table);
    arith::register(table);
    logic::register(table);
    shift::register(table);
    bits::register(table);
    branch::register(table);
}
