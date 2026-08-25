//! Biconditional legality test for the disassembler.
//!
//! For every one of the 65,536 opcode words, asserts:
//!
//! ```text
//! disassembler renders dc.w  <=>  CPU reaches the illegal-instruction handler
//! ```
//!
//! The two sides are independent implementations of the same fact. The
//! disassembler decodes structurally from the encoding; the CPU executes the
//! word and the result is observed through the vector table. Structural
//! agreement of two independent implementations is a stronger check than
//! either alone.
//!
//! **Both directions are asserted.** The `dc.w`-but-CPU-legal direction is
//! currently 0, and keeping it asserted is what stops an over-broad fix from
//! silently suppressing real instructions.
//!
//! The test runs in a few seconds in release mode. The CPU execution uses the
//! same setup as `opcode_space.rs::is_illegal`: vector 4 is redirected to a
//! known address, and the PC after one step is checked.

// An integration test is its own crate root, so the crate's `lib.rs` attribute
// does not reach here.
#![forbid(unsafe_code)]

use m68k::decode::Decoder;
use m68k::disasm::disassemble;
use m68k::{Bus, M68k};

struct Ram(Vec<u8>);

impl Ram {
    fn new() -> Self {
        // Fill with NOP (0x4E71 = 0x4E, 0x71) so prefetch anywhere is legal.
        let mut v = vec![0u8; 0x10000];
        for i in (0..0x10000).step_by(2) {
            v[i] = 0x4E;
            v[i + 1] = 0x71;
        }
        Self(v)
    }
}

impl Bus for Ram {
    fn read8(&mut self, a: u32) -> u8 {
        self.0[(a & 0xFFFF) as usize]
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.0[(a & 0xFFFF) as usize] = v;
    }
    fn read16(&mut self, a: u32) -> u16 {
        ((self.read8(a) as u16) << 8) | self.read8(a.wrapping_add(1)) as u16
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.write8(a, (v >> 8) as u8);
        self.write8(a.wrapping_add(1), v as u8);
    }
}

fn seeded(op: u16) -> (M68k, Ram) {
    let mut cpu = M68k::new();
    for i in 0..8 {
        cpu.d[i] = 0x1000u32.wrapping_add(i as u32);
        cpu.a[i] = 0x1000u32.wrapping_add(i as u32);
    }
    cpu.ssp = 0x2000;
    cpu.usp = 0x3000;
    cpu.set_sr(0x2700); // supervisor, interrupts masked
    cpu.pc = 0x0504;
    cpu.prefetch = [op, 0x1235];
    (cpu, Ram::new())
}

/// Returns true if executing `op` causes the CPU to reach one of the three
/// "not a real instruction" handlers: illegal-instruction (vector 4, address
/// 0x10), line-A emulator (vector 10, address 0x28), or line-F emulator
/// (vector 11, address 0x2C).
///
/// All three are redirected to the same handler at 0x7000 before the step;
/// any of the three landing there counts as "CPU says illegal". The disasm
/// test considers all three equivalent because none corresponds to a
/// real 68000 instruction, which is what the biconditional is checking.
fn cpu_is_illegal(dec: &Decoder, op: u16) -> bool {
    let (mut cpu, mut bus) = seeded(op);
    // Vector 4 (illegal): address 0x10-0x13
    bus.write16(0x10, 0x0000);
    bus.write16(0x12, 0x7000);
    // Vector 10 (line-A): address 0x28-0x2B
    bus.write16(0x28, 0x0000);
    bus.write16(0x2A, 0x7000);
    // Vector 11 (line-F): address 0x2C-0x2F
    bus.write16(0x2C, 0x0000);
    bus.write16(0x2E, 0x7000);
    let _ = cpu.step_with(dec, &mut bus);
    cpu.pc & 0xFFFF == 0x7000 + 4
}

/// Returns true if the disassembler renders `op` as `dc.w`.
fn disasm_is_illegal(op: u16) -> bool {
    // Extension words as 0 — the biconditional check is on the opcode word
    // only; extension words do not affect legality on the 68000.
    let text = disassemble(|a| if a == 0 { op } else { 0 }, 0).text;
    text.starts_with("dc.w")
}

/// Biconditional: `disasm renders dc.w` ⟺ `CPU reaches illegal handler`.
///
/// Both directions are asserted so neither a missed-illegal (false legal) nor
/// an over-broad fix (false illegal) can go unnoticed.
#[test]
fn disasm_illegal_iff_cpu_illegal() {
    let dec = Decoder::new();

    let mut false_legal = Vec::new(); // disasm says legal, CPU says illegal
    let mut false_illegal = Vec::new(); // disasm says dc.w, CPU says legal

    for op in 0u32..0x10000 {
        let op = op as u16;
        let dis_illegal = disasm_is_illegal(op);
        let cpu_illegal = cpu_is_illegal(&dec, op);

        if !dis_illegal && cpu_illegal {
            false_legal.push(op);
        } else if dis_illegal && !cpu_illegal {
            false_illegal.push(op);
        }
    }

    let mut msg = String::new();

    if !false_legal.is_empty() {
        msg.push_str(&format!(
            "\n{} opcodes rendered as instructions but CPU says illegal:\n",
            false_legal.len()
        ));
        for op in false_legal.iter().take(20) {
            let text = disassemble(|a| if a == 0 { *op } else { 0 }, 0).text;
            msg.push_str(&format!("  {op:04X} -> {text}\n"));
        }
        if false_legal.len() > 20 {
            msg.push_str(&format!("  ... and {} more\n", false_legal.len() - 20));
        }
    }

    if !false_illegal.is_empty() {
        msg.push_str(&format!(
            "\n{} opcodes rendered as dc.w but CPU has a handler:\n",
            false_illegal.len()
        ));
        for op in false_illegal.iter().take(20) {
            msg.push_str(&format!("  {op:04X}\n"));
        }
        if false_illegal.len() > 20 {
            msg.push_str(&format!("  ... and {} more\n", false_illegal.len() - 20));
        }
    }

    assert!(
        false_legal.is_empty() && false_illegal.is_empty(),
        "disasm/CPU legality mismatch:{msg}"
    );
}
