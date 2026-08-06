//! Instruction handlers, one module per instruction family.

pub mod move_;

/// Installs every implemented handler into the dispatch table.
///
/// Opcodes left untouched keep the table's default `illegal_instruction`
/// handler, so a partially populated table is safe — an unimplemented opcode
/// raises an emulated illegal-instruction exception rather than misbehaving.
pub fn register_all(table: &mut [crate::decode::Handler; 65536]) {
    move_::register(table);
}
