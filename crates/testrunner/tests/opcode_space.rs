//! Exhaustive checks over the whole 65536-opcode space.
//!
//! The vector suite samples 2500 cases per group, so a legal-but-rare encoding
//! can be wrong or panicking without any group failing. These tests cover every
//! opcode instead of a sample. Run in debug too: overflow checks and
//! `debug_assert!`s (notably the MOVE schedule's fetch-count assertion) are the
//! point.

use m68k::decode::Decoder;
use m68k::{Bus, M68k};

/// 64 KB of RAM prefilled with `NOP`, so a fetch anywhere returns a legal word.
struct Ram(Vec<u8>);

impl Ram {
    fn new() -> Self {
        Self(vec![0x4E; 0x10000])
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

fn seeded(base: u32, sr: u16, op: u16) -> (M68k, Ram) {
    let mut cpu = M68k::new();
    for i in 0..8 {
        cpu.d[i] = base.wrapping_add(i as u32);
        cpu.a[i] = base.wrapping_add(i as u32);
    }
    cpu.ssp = 0x2000;
    cpu.usp = 0x3000;
    cpu.set_sr(sr);
    cpu.pc = 0x0504;
    cpu.prefetch = [op, 0x1235];
    (cpu, Ram::new())
}

/// Every opcode must execute without panicking, under seeds that hit odd and
/// even addresses, wrapping arithmetic, and both privilege modes. A guest fault
/// is an emulated 68000 exception, never a Rust panic.
#[test]
fn no_opcode_panics() {
    let dec = Decoder::new();
    // 0x1001 forces odd (address-error) operand addresses; 0xFFFFFFFF and 0
    // force wrapping adds and subtracts in address computation.
    let seeds: [(u32, u16); 4] = [
        (0x0000_1000, 0x2700),
        (0x0000_1001, 0x2700),
        (0xFFFF_FFFF, 0x0000),
        (0x0000_0000, 0x0000),
    ];
    for (base, sr) in seeds {
        for op in 0..=0xFFFFu32 {
            let (mut cpu, mut bus) = seeded(base, sr, op as u16);
            let _ = cpu.step_with(&dec, &mut bus);
        }
    }
}

/// Executing an opcode the illegal handler owns leaves PC at the vector-4
/// handler. Used to tell "this encoding is unclaimed" from "a MOVE handler
/// claimed it", without reaching into the decoder's private table.
fn is_illegal(dec: &Decoder, op: u16) -> bool {
    let (mut cpu, mut bus) = seeded(0x0000_1000, 0x2700, op);
    // Vector 4 (illegal instruction) is at 0x10; RAM is 0x4E4E throughout, so
    // an illegal opcode lands at 0x4E4E and nothing else does.
    bus.write16(0x10, 0x0000);
    bus.write16(0x12, 0x7000);
    let _ = cpu.step_with(dec, &mut bus);
    cpu.pc & 0xFFFF == 0x7000 + 4
}

/// The MOVE encodings that do not exist must reach the illegal handler, and
/// every encoding that does exist must not. Checked over all 65536 opcodes in
/// the four MOVE lines rather than the suite's sample.
#[test]
fn move_claims_exactly_the_legal_encodings() {
    let dec = Decoder::new();
    for op in 0x1000..0x4000u16 {
        let size_bits = op >> 12;
        let src_mode = (op >> 3) & 7;
        let src_reg = op & 7;
        let dst_mode = (op >> 6) & 7;
        let dst_reg = (op >> 9) & 7;

        // An address register is not a byte-sized source, mode 7 stops at
        // reg 4, a destination is never an immediate or PC-relative, and
        // MOVEA.b does not exist.
        let src_ok = match src_mode {
            1 => size_bits != 1,
            7 => src_reg <= 4,
            _ => true,
        };
        let dst_ok = match dst_mode {
            1 => size_bits != 1,
            7 => dst_reg <= 1,
            _ => true,
        };
        let legal = src_ok && dst_ok;
        assert_eq!(
            !is_illegal(&dec, op),
            legal,
            "opcode {op:04X}: size_bits={size_bits} src={src_mode}/{src_reg} \
             dst={dst_mode}/{dst_reg} — expected legal={legal}"
        );
    }
}

/// MOVEQ is `0111 rrr 0 dddddddd`: bit 8 must be clear. The 2048 opcodes with
/// bit 8 set are illegal, and none of them may reach the MOVEQ handler.
#[test]
fn moveq_requires_bit_8_clear() {
    let dec = Decoder::new();
    for op in 0x7000..0x8000u16 {
        let legal = op & 0x0100 == 0;
        assert_eq!(!is_illegal(&dec, op), legal, "opcode {op:04X}");
    }
}
