//! Instruction handlers, one module per instruction family.

pub mod alu;
pub mod arith;
pub mod logic;
pub mod move_;

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
pub fn register_all(table: &mut [crate::decode::Handler; 65536]) {
    move_::register(table);
    arith::register(table);
    logic::register(table);
}
