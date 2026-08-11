//! The save-state file format.
//!
//! # The layout
//!
//! ```text
//! offset  size  field
//! 0       8     MAGIC          b"SFEMU\0\0\x02"; the last byte is the version
//! 8       4     board          little-endian; BOARD_SF2 = b"SF2\0"
//! 12      8     payload length little-endian
//! 20      len   payload
//! 20+len  4     CRC-32 of the payload, little-endian
//! ```
//!
//! # Version 2 added the sound board
//!
//! Version 1 predates it. A version-1 file has no Z80, no sound RAM, no YM2151, and
//! no sound schedule, so there is nothing to restore them from and no defensible
//! default: a machine whose 68000 is mid-frame and whose Z80 is at its reset vector
//! is not a state the hardware can be in. So a version-1 file is refused with
//! [`StateError::Version`], which is what that variant is for — it names the version
//! rather than calling the file damaged.
//!
//! The payload's field order is the list in [`encode`]'s source, read top to
//! bottom, and **every reader and writer follows that list**. Booleans are one
//! byte; a decoder accepts any non-zero as true, because the byte came from a file
//! and refusing a state over a padding byte of 2 is a rejection with no diagnostic
//! value.
//!
//! # Why hand-rolled and not `serde` + `bincode`
//!
//! Two more dependencies, and — the real reason — a format whose layout is
//! *implied* by struct definitions. Reorder two fields in a later refactor and the
//! file format silently changes while every round-trip test still passes, because
//! both sides moved together. That is this branch's characteristic defect wearing a
//! save-state costume: a test that cannot fail because its expectation is derived
//! from the thing under test. Writing the bytes out by hand means the reader and
//! the writer are two separate lists that a reordering breaks visibly, and
//! `tests::the_encoded_length_is_the_documented_size` pins the total.
//!
//! # Why the CRC is written again here
//!
//! `crates/romset/src/crc32.rs` already has this algorithm, and `frontend` may not
//! depend on `romset` — `romset` pulls in `miniz_oxide`, and this crate's manifest
//! is deliberately one dependency wide. So [`crc32`] is a second implementation,
//! and both are pinned against the CRC-32 specification's own check value rather
//! than against each other.

use machine::m68k::M68k;
use machine::sound::RAM_BYTES as SOUND_RAM_BYTES;
use machine::timing::{RationalAccumulator, Z80_T_DEN, Z80_T_NUM};
use machine::video::sprites::{ObjLatch, OBJ_WORDS};
use machine::ym2151::state::{
    StateReader as YmReader, StateWriter as YmWriter, STATE_BYTES as YM_BYTES,
};
use machine::ym2151::Ym2151;
use machine::z80::Z80;
use machine::{Inputs, MachineState, PlayerInput};

/// The first eight bytes of every save state. The last byte is [`VERSION`].
pub const MAGIC: [u8; 8] = *b"SFEMU\0\0\x02";

/// The format version, and the last byte of [`MAGIC`].
///
/// 2 since the sound board joined the state — see the module docs on why a
/// version-1 file is refused rather than filled in.
pub const VERSION: u8 = 2;

/// The board a state belongs to: `b"SF2\0"` big-endian, so it reads as ASCII in a
/// hex dump.
pub const BOARD_SF2: u32 = 0x5346_3200;

/// Bytes before the payload: magic, board, and the declared length.
const HEADER: usize = 8 + 4 + 8;

/// Main RAM, in words.
const RAM_WORDS: usize = 0x8000;
/// Tilemap, sprite, and palette RAM, in words.
const GFXRAM_WORDS: usize = 0x1_8000;
/// CPS-A and CPS-B each have this many registers.
const CPS_REGS: usize = 0x20;

/// The Z80's encoded size: 10 one-byte registers, 9 two-byte, then the flags.
///
/// **A hand count of the encoded fields, not `size_of::<Z80>()`**, which is 38 — the
/// struct has a byte of alignment padding, and a format taking its size from the
/// layout would change on a field reorder without any test noticing. That is this
/// module's whole argument against serde.
const Z80_BYTES: usize = 10          // a f b c d e h l i r
    + 9 * 2                          // ix iy sp pc wz af_ bc_ de_ hl_
    + 2                              // iff1 iff2
    + 4                              // im ei q p
    + 3; // halted irq nmi

/// The payload's exact size, written out term by term.
///
/// Stated here and checked against the encoder's output by
/// `tests::the_encoded_length_is_the_documented_size`, so the format is a format
/// rather than whatever the encoder happens to emit.
const PAYLOAD: usize = 8 * 4      // d[0..8]
    + 8 * 4                        // a[0..8]
    + 4                            // pc
    + 2                            // sr
    + 4 + 4                        // usp, ssp
    + 2 * 2                        // prefetch[0..2]
    + 5                            // halted, stopped, pending_irq, in_exception, trace_pending
    + RAM_WORDS * 2
    + GFXRAM_WORDS * 2
    + CPS_REGS * 2 * 2             // cps_a and cps_b
    + 2                            // sound_latch[0..2]
    + 2                            // coin_ctrl
    + 1                            // vblank_pending
    + 6                            // coin1, coin2, service, start1, start2, test
    + 2 * 10                       // p1 and p2: 4 stick, 3 punch, 3 kick
    + 3                            // dsw[0..3]
    + 8                            // total_cycles
    + 4                            // line
    + 8                            // carry
    + OBJ_WORDS * 2
    // ---- the sound board, new in version 2 ----
    + Z80_BYTES
    + SOUND_RAM_BYTES
    + 1                            // sound_bank
    + 1                            // oki_pin7
    + YM_BYTES
    + 1                            // ym_addr
    + 4 + 4 + 4                    // the Z80 accumulator: num, den, remainder
    + 8                            // z80_debt
    + 8                            // z80_total
    + 4; // sample_acc

/// Why a state was refused.
///
/// Each variant names **which check failed**, so a user with a bad file learns
/// whether it is the wrong game, the wrong version, or damaged — three different
/// things to do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    /// The magic does not match: this is not a save state at all.
    NotAState,
    /// A save state, but from a different version of the format.
    Version {
        /// The version byte the file carries.
        found: u8,
    },
    /// A save state for a different board.
    WrongBoard {
        /// The board the file names.
        found: u32,
        /// The board it was loaded into.
        expected: u32,
    },
    /// The file ends before the format says it should.
    Truncated {
        /// Bytes the format requires at this point.
        need: usize,
        /// Bytes actually present.
        got: usize,
    },
    /// The payload's CRC-32 does not match: the file is damaged.
    Corrupt {
        /// The CRC the file carries.
        found: u32,
        /// The CRC its payload actually has.
        computed: u32,
    },
    /// The state's sound clock ratio is not this board's.
    ///
    /// A separate variant from [`Self::WrongBoard`] because it is a different
    /// mistake: the file is for this game, and it is not damaged — its Z80's
    /// fractional cycle debt is measured against a denominator this build does not
    /// use, so resuming it would run the sound CPU at the wrong speed rather than
    /// fail. Silently re-basing the remainder onto the new denominator would be the
    /// decoder quietly changing the state it was given.
    WrongSoundClock {
        /// The ratio the file carries, as (numerator, denominator).
        found: (u32, u32),
        /// The ratio this build's board runs at.
        expected: (u32, u32),
    },
}

impl core::fmt::Display for StateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotAState => write!(f, "not a save state (wrong magic)"),
            Self::Version { found } => {
                write!(
                    f,
                    "save state version {found}, but this build reads {VERSION}"
                )
            }
            Self::WrongBoard { found, expected } => write!(
                f,
                "save state is for board {found:#010x}, not {expected:#010x}"
            ),
            Self::Truncated { need, got } => {
                write!(f, "save state is truncated: needs {need} bytes, has {got}")
            }
            Self::Corrupt { found, computed } => write!(
                f,
                "save state is damaged: CRC-32 {found:#010x}, computed {computed:#010x}"
            ),
            Self::WrongSoundClock {
                found: (fnum, fden),
                expected: (enum_, eden),
            } => write!(
                f,
                "save state's sound clock is {fnum}/{fden}, but this board runs {enum_}/{eden}"
            ),
        }
    }
}

/// CRC-32, reflected polynomial 0xEDB88320.
///
/// See the module documentation for why this is not `romset`'s.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Appends bytes in the payload's order.
///
/// A tiny struct rather than free functions so the writing side reads as one
/// sequence, which is what makes a reordering visible in a diff.
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn bool(&mut self, v: bool) {
        self.0.push(u8::from(v));
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn words(&mut self, ws: &[u16]) {
        for &w in ws {
            self.u16(w);
        }
    }
    fn bytes(&mut self, bs: &[u8]) {
        self.0.extend_from_slice(bs);
    }
    /// The sound Z80, field by field. See [`Z80_BYTES`].
    ///
    /// Written here rather than in `z80`, unlike the YM2151's: every field of a `Z80`
    /// is public, so there is nothing the codec cannot reach and no reason to put a
    /// byte layout inside a CPU core that has no other use for one.
    fn z80(&mut self, c: &Z80) {
        self.u8(c.a);
        self.u8(c.f);
        self.u8(c.b);
        self.u8(c.c);
        self.u8(c.d);
        self.u8(c.e);
        self.u8(c.h);
        self.u8(c.l);
        self.u8(c.i);
        self.u8(c.r);
        self.u16(c.ix);
        self.u16(c.iy);
        self.u16(c.sp);
        self.u16(c.pc);
        self.u16(c.wz);
        self.u16(c.af_);
        self.u16(c.bc_);
        self.u16(c.de_);
        self.u16(c.hl_);
        self.bool(c.iff1);
        self.bool(c.iff2);
        self.u8(c.im);
        self.u8(c.ei);
        self.u8(c.q);
        self.u8(c.p);
        self.bool(c.halted);
        self.bool(c.irq);
        self.bool(c.nmi);
    }
    /// The FM chip, in [`YM_BYTES`] bytes of its own layout.
    ///
    /// The chip writes its own bytes: its register file, timers, and envelope counter
    /// are private to it, and a setter per field would be a far wider hole in its
    /// interface than one layout. See `machine::ym2151::state`.
    fn ym(&mut self, chip: &Ym2151) {
        let at = self.0.len();
        self.0.resize(at + YM_BYTES, 0);
        let mut w = YmWriter::new(&mut self.0[at..]);
        chip.write_state(&mut w);
        debug_assert_eq!(w.at(), YM_BYTES, "the chip's own layout size");
    }
    fn player(&mut self, p: &PlayerInput) {
        self.bool(p.right);
        self.bool(p.left);
        self.bool(p.down);
        self.bool(p.up);
        for &b in &p.punch {
            self.bool(b);
        }
        for &b in &p.kick {
            self.bool(b);
        }
    }
}

/// Reads bytes in the payload's order.
///
/// Every getter is infallible because [`decode`] checks the payload's length once,
/// up front, against [`PAYLOAD`] — so by the time a reader runs, the bytes are
/// known to be there. `expect` on the slice conversions states that invariant
/// rather than defending against it.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let s = &self.bytes[self.at..self.at + n];
        self.at += n;
        s
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    /// Any non-zero byte is true. See the module documentation.
    fn bool(&mut self) -> bool {
        self.u8() != 0
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take(2).try_into().expect("two bytes"))
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().expect("four bytes"))
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take(8).try_into().expect("eight bytes"))
    }
    fn i64(&mut self) -> i64 {
        i64::from_le_bytes(self.take(8).try_into().expect("eight bytes"))
    }
    fn words(&mut self, out: &mut [u16]) {
        for w in out.iter_mut() {
            *w = self.u16();
        }
    }
    fn bytes(&mut self, out: &mut [u8]) {
        let n = out.len();
        out.copy_from_slice(self.take(n));
    }
    /// The sound Z80, in [`Writer::z80`]'s order.
    fn z80(&mut self) -> Z80 {
        // Field by field into a fresh CPU rather than a struct literal, so this reads
        // as the same sequence the writer does. `Z80::new` is only a way to name one:
        // every field below is overwritten.
        let mut c = Z80::new();
        c.a = self.u8();
        c.f = self.u8();
        c.b = self.u8();
        c.c = self.u8();
        c.d = self.u8();
        c.e = self.u8();
        c.h = self.u8();
        c.l = self.u8();
        c.i = self.u8();
        c.r = self.u8();
        c.ix = self.u16();
        c.iy = self.u16();
        c.sp = self.u16();
        c.pc = self.u16();
        c.wz = self.u16();
        c.af_ = self.u16();
        c.bc_ = self.u16();
        c.de_ = self.u16();
        c.hl_ = self.u16();
        c.iff1 = self.bool();
        c.iff2 = self.bool();
        c.im = self.u8();
        c.ei = self.u8();
        c.q = self.u8();
        c.p = self.u8();
        c.halted = self.bool();
        c.irq = self.bool();
        c.nmi = self.bool();
        c
    }
    /// The FM chip, from [`YM_BYTES`] bytes of its own layout.
    fn ym(&mut self) -> Ym2151 {
        let bytes = self.take(YM_BYTES);
        let mut r = YmReader::new(bytes);
        let chip = Ym2151::read_state_from(&mut r);
        debug_assert_eq!(r.at(), YM_BYTES, "the chip's own layout size");
        chip
    }
    fn player(&mut self) -> PlayerInput {
        PlayerInput {
            right: self.bool(),
            left: self.bool(),
            down: self.bool(),
            up: self.bool(),
            punch: [self.bool(), self.bool(), self.bool()],
            kick: [self.bool(), self.bool(), self.bool()],
        }
    }
}

/// Encodes a state as a file.
///
/// The field order here **is** the format. [`decode`] reads the same list in the
/// same order.
pub fn encode(s: &MachineState, board: u32) -> Vec<u8> {
    let mut w = Writer(Vec::with_capacity(PAYLOAD));

    // The CPU.
    for &v in &s.cpu.d {
        w.u32(v);
    }
    for &v in &s.cpu.a {
        w.u32(v);
    }
    w.u32(s.cpu.pc);
    w.u16(s.cpu.sr);
    w.u32(s.cpu.usp);
    w.u32(s.cpu.ssp);
    for &v in &s.cpu.prefetch {
        w.u16(v);
    }
    w.bool(s.cpu.halted);
    w.bool(s.cpu.stopped);
    w.u8(s.cpu.pending_irq);
    w.bool(s.cpu.in_exception);
    w.bool(s.cpu.trace_pending);

    // Memory.
    w.words(&s.ram[..]);
    w.words(&s.gfxram[..]);
    w.words(&s.cps_a);
    w.words(&s.cps_b);

    // The rest of the board.
    w.u8(s.sound_latch[0]);
    w.u8(s.sound_latch[1]);
    w.u16(s.coin_ctrl);
    w.bool(s.vblank_pending);

    // Controls.
    w.bool(s.inputs.coin1);
    w.bool(s.inputs.coin2);
    w.bool(s.inputs.service);
    w.bool(s.inputs.start1);
    w.bool(s.inputs.start2);
    w.bool(s.inputs.test);
    w.player(&s.inputs.p1);
    w.player(&s.inputs.p2);
    for &v in &s.inputs.dsw {
        w.u8(v);
    }

    // The schedule, and the sprite delay.
    w.u64(s.total_cycles);
    w.u32(s.line);
    w.i64(s.carry);
    w.words(s.obj.words());

    // The sound board.
    w.z80(&s.z80);
    w.bytes(&s.sound_ram[..]);
    w.u8(s.sound_bank);
    w.bool(s.oki_pin7);
    w.ym(&s.ym);
    w.u8(s.ym_addr);
    // The accumulator's ratio as well as its remainder. The ratio is the board's,
    // fixed by its crystals, so it is not information the state adds — it is what
    // lets `decode` refuse a file written on a board with a different sound clock
    // instead of resuming its fraction against the wrong denominator.
    let (num, den) = s.z80_carry.ratio();
    w.u32(num);
    w.u32(den);
    w.u32(s.z80_carry.remainder());
    w.i64(s.z80_debt);
    w.u64(s.z80_total);
    w.u32(s.sample_acc);

    let payload = w.0;
    let mut out = Vec::with_capacity(HEADER + payload.len() + 4);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&board.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    let crc = crc32(&payload);
    out.extend_from_slice(&payload);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

/// Decodes a file into a state, or says which check refused it.
///
/// Validation order — magic, version, board, declared length, CRC, then the
/// payload's length against what the fields need — is chosen so the error names the
/// most useful thing. Checking the CRC before the version would report a
/// next-version file as "damaged".
///
/// **Never panics, on any input.** The declared length is checked against the bytes
/// actually present *before* it is used for anything, so a file claiming a payload
/// of 2^64 is `Truncated` and not an allocation failure.
pub fn decode(bytes: &[u8], board: u32) -> Result<MachineState, StateError> {
    if bytes.len() < MAGIC.len() {
        return Err(StateError::Truncated {
            need: MAGIC.len(),
            got: bytes.len(),
        });
    }
    // The version lives in the magic's last byte, so compare the first seven and
    // then the version separately — otherwise every version mismatch would report
    // `NotAState` and a user with an old state would be told it is not a state.
    if bytes[..7] != MAGIC[..7] {
        return Err(StateError::NotAState);
    }
    if bytes[7] != VERSION {
        return Err(StateError::Version { found: bytes[7] });
    }
    if bytes.len() < HEADER {
        return Err(StateError::Truncated {
            need: HEADER,
            got: bytes.len(),
        });
    }
    let found_board = u32::from_le_bytes(bytes[8..12].try_into().expect("four bytes"));
    if found_board != board {
        return Err(StateError::WrongBoard {
            found: found_board,
            expected: board,
        });
    }
    let declared = u64::from_le_bytes(bytes[12..20].try_into().expect("eight bytes"));
    // `try_into` and not `as usize`: on a 32-bit host a declared length of 2^32
    // truncates to 0, which would pass the check below and then read a payload that
    // is not there.
    let declared: usize = declared.try_into().unwrap_or(usize::MAX);
    let need = HEADER.saturating_add(declared).saturating_add(4);
    if bytes.len() < need {
        return Err(StateError::Truncated {
            need,
            got: bytes.len(),
        });
    }
    let payload = &bytes[HEADER..HEADER + declared];
    let found_crc = u32::from_le_bytes(
        bytes[HEADER + declared..HEADER + declared + 4]
            .try_into()
            .expect("four bytes"),
    );
    let computed = crc32(payload);
    if found_crc != computed {
        return Err(StateError::Corrupt {
            found: found_crc,
            computed,
        });
    }
    // A payload that passes the CRC but is the wrong length is a well-formed file
    // this build cannot read — same version, same board, different field set. It
    // should not be possible, and it must not be a panic if it is.
    if payload.len() != PAYLOAD {
        return Err(StateError::Truncated {
            need: HEADER + PAYLOAD + 4,
            got: bytes.len(),
        });
    }

    let mut r = Reader {
        bytes: payload,
        at: 0,
    };
    // Every field below is overwritten from the file. `M68k::new` is the starting
    // point only because there is no other way to name one, and `sr` is assigned
    // directly rather than through `set_sr` on purpose: `set_sr` swaps the stack
    // pointers, and this is restoring a CPU whose `a[7]` the file already carries.
    let mut cpu = M68k::new();

    // The CPU, in the same order `encode` wrote it.
    for v in cpu.d.iter_mut() {
        *v = r.u32();
    }
    for v in cpu.a.iter_mut() {
        *v = r.u32();
    }
    cpu.pc = r.u32();
    cpu.sr = r.u16();
    cpu.usp = r.u32();
    cpu.ssp = r.u32();
    for v in cpu.prefetch.iter_mut() {
        *v = r.u16();
    }
    cpu.halted = r.bool();
    cpu.stopped = r.bool();
    cpu.pending_irq = r.u8();
    cpu.in_exception = r.bool();
    cpu.trace_pending = r.bool();

    let mut ram = vec![0u16; RAM_WORDS];
    r.words(&mut ram);
    let mut gfxram = vec![0u16; GFXRAM_WORDS];
    r.words(&mut gfxram);
    let mut cps_a = [0u16; CPS_REGS];
    r.words(&mut cps_a);
    let mut cps_b = [0u16; CPS_REGS];
    r.words(&mut cps_b);

    let sound_latch = [r.u8(), r.u8()];
    let coin_ctrl = r.u16();
    let vblank_pending = r.bool();

    let inputs = Inputs {
        coin1: r.bool(),
        coin2: r.bool(),
        service: r.bool(),
        start1: r.bool(),
        start2: r.bool(),
        test: r.bool(),
        p1: r.player(),
        p2: r.player(),
        dsw: [r.u8(), r.u8(), r.u8()],
    };

    let total_cycles = r.u64();
    let line = r.u32();
    let carry = r.i64();
    let mut obj = ObjLatch::new();
    r.words(obj.words_mut());

    // The sound board, in `encode`'s order.
    let z80 = r.z80();
    let mut sound_ram = vec![0u8; SOUND_RAM_BYTES];
    r.bytes(&mut sound_ram);
    let sound_bank = r.u8();
    let oki_pin7 = r.bool();
    let ym = r.ym();
    let ym_addr = r.u8();
    // The ratio is checked, not adopted: a remainder is only meaningful against the
    // denominator it was taken modulo. See `StateError::WrongSoundClock`.
    let found_ratio = (r.u32(), r.u32());
    let z80_rem = r.u32();
    if found_ratio != (Z80_T_NUM, Z80_T_DEN) {
        return Err(StateError::WrongSoundClock {
            found: found_ratio,
            expected: (Z80_T_NUM, Z80_T_DEN),
        });
    }
    let z80_carry = RationalAccumulator::with_remainder(Z80_T_NUM, Z80_T_DEN, z80_rem);
    let z80_debt = r.i64();
    let z80_total = r.u64();
    let sample_acc = r.u32();

    Ok(MachineState {
        cpu,
        ram: boxed(ram),
        gfxram: boxed(gfxram),
        cps_a,
        cps_b,
        sound_latch,
        coin_ctrl,
        vblank_pending,
        inputs,
        total_cycles,
        line,
        carry,
        obj,
        z80,
        sound_ram: boxed_bytes(sound_ram),
        sound_bank,
        oki_pin7,
        ym,
        ym_addr,
        z80_carry,
        z80_debt,
        z80_total,
        sample_acc,
    })
}

/// A `Vec` of exactly `N` words as a boxed array, without a stack temporary.
///
/// The same reasoning as `machine::snapshot::boxed_copy`: `Box::new([0u16; N])`
/// builds 192 KB on the stack for gfxram, which overflows a test thread.
fn boxed<const N: usize>(v: Vec<u16>) -> Box<[u16; N]> {
    v.into_boxed_slice()
        .try_into()
        .expect("the reader filled exactly N words")
}

/// [`boxed`] for the sound board's byte-wide RAM.
fn boxed_bytes<const N: usize>(v: Vec<u8>) -> Box<[u8; N]> {
    v.into_boxed_slice()
        .try_into()
        .expect("the reader filled exactly N bytes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use machine::z80::Bus;
    use machine::{BoardConfig, Cps1, Timing};

    /// A Z80 program that copies the 68000's command latch into sound RAM, forever.
    ///
    /// ```text
    /// 0000  3A 08 F0    ld a,($F008)   the command latch
    /// 0003  32 00 D0    ld ($D000),a   into the first byte of sound RAM
    /// 0006  00          nop
    /// 0007  18 F7       jr $0000
    /// ```
    ///
    /// The same program `machine`'s `cps1::tests::sound_spin` uses, reproduced rather
    /// than shared for the reason [`a_machine`]'s program is: a test module of another
    /// crate is not reachable from here. Its `nop` is load-bearing there — it makes
    /// the loop 42 T-states, whose remainder against a line's 229 walks the whole loop
    /// — and it is kept here so the fixture's Z80 stops on each of the four
    /// instructions in turn rather than on a fixed one, which is what makes the
    /// restored `pc` and `z80_debt` take many values across the run.
    ///
    /// Padded to the full 0x18000 `audiocpu` region so a bank switch has a bank to
    /// switch to.
    fn sound_rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x1_8000];
        rom[0..9].copy_from_slice(&[0x3A, 0x08, 0xF0, 0x32, 0x00, 0xD0, 0x00, 0x18, 0xF7]);
        // A byte only reachable through bank 1, so `sound_bank` is observable: the
        // fixture selects bank 1, and a codec that dropped the bank would restore a
        // machine reading 0x00 here instead.
        rom[0x1_4000] = 0x5A;
        rom
    }

    /// Puts the FM chip in a state no default can imitate: mid-note, LFO running,
    /// both timers loaded and enabled.
    ///
    /// Written through the board's bus, as a driver does, so the address latch at
    /// 0xF000 is exercised rather than bypassed — and the register file, the envelope
    /// counter, the LFO waveform, and the timers all end up somewhere a reset chip is
    /// not.
    ///
    /// **All four operators.** Algorithm 4's carriers are the operators at register
    /// offsets 0x10 and 0x18, so leaving their attack rates at 0 — "never attack" —
    /// makes the patch silent, and `machine`'s `ym2151` tests record that a silent
    /// patch is what turns every sound assertion vacuous.
    fn patch_the_chip(m: &mut Cps1) {
        let mut w = |addr: u8, val: u8| {
            m.sound.write(0xF000, addr);
            m.sound.write(0xF001, val);
        };
        w(0x20, 0xC4); // algorithm 4, both outputs
        w(0x28, 0x4A); // key code
        for op in 0..4u8 {
            let off = op * 8;
            w(0x40 + off, 0x01); // detune 0, multiple 1
            w(0x80 + off, 0x1F); // attack rate 31: immediate
        }
        w(0x18, 0x40); // LFO frequency
        w(0x19, 0x7F); // AM depth (bit 7 clear)
        w(0x19, 0xFF); // and PM depth (bit 7 set), the same register address
        w(0x1B, 0x03); // LFO waveform 3: noise, which fills the 256-entry table
        w(0x0F, 0x94); // noise enable, frequency 0x14
        w(0x10, 0x40); // timer A high
        w(0x11, 0x02); // timer A low
        w(0x12, 0x37); // timer B
        w(0x14, 0x0F); // load and enable both timers, IRQ on both
        w(0x08, 0x78); // key on, all four operators of channel 0
    }

    /// A machine whose state is distinctive in every field, and the state itself.
    ///
    /// Built by *running* a machine rather than by assembling a `MachineState` by
    /// hand. A hand-built state could be internally impossible — a `carry` above
    /// zero, a `line` past the frame — and the codec would then be verified against
    /// something the machine cannot produce. It also means the fixture exercises
    /// the fields that only a running machine sets: the prefetch queue, the
    /// interrupt level, the object latch.
    ///
    /// The program is the one `machine`'s snapshot tests use, reproduced rather than
    /// shared: `frontend`'s tests may not reach into another crate's test module,
    /// and a fixture that drifted would be a fixture that stopped diverging.
    ///
    /// ```text
    /// 1000  46FC 2000        move #$2000,sr     supervisor, mask 0 -- take IRQs
    /// 1004  5240             addq.w #1,d0
    /// 1006  33C0 00FF 0000   move.w d0,$FF0000
    /// 100C  33C0 0090 0000   move.w d0,$900000
    /// 1012  60F0             bra $1004
    /// 1100  5241             addq.w #1,d1       the vblank handler
    /// 1102  4E73             rte
    /// ```
    ///
    /// # It has a sound program, since version 2
    ///
    /// The machine is built with [`sound_spin`] rather than through `with_gfx`. A
    /// board with no sound ROM reads [`machine::sound::UNMAPPED`] — 0xFF, `RST 38h` —
    /// so its Z80 spins in a reset loop that writes nothing: sound RAM stays zero, the
    /// stack is the only thing that moves, and every sound field a codec dropped
    /// would restore to a value indistinguishable from the one it lost.
    fn a_machine() -> Cps1 {
        let mut rom = vec![0u8; 0x2000];
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x68..0x6C].copy_from_slice(&[0x00, 0x00, 0x11, 0x00]);
        rom[0x1000..0x1014].copy_from_slice(&[
            0x46, 0xFC, 0x20, 0x00, 0x52, 0x40, 0x33, 0xC0, 0x00, 0xFF, 0x00, 0x00, 0x33, 0xC0,
            0x00, 0x90, 0x00, 0x00, 0x60, 0xF0,
        ]);
        rom[0x1100..0x1104].copy_from_slice(&[0x52, 0x41, 0x4E, 0x73]);

        // A 16x16 tile solid in pen 0x0A, so the renderer draws something and the
        // framebuffer comparison below is not a comparison of one flat colour.
        let mut gfx = vec![0u8; 128];
        for row in 0..16 {
            for half in [0usize, 4] {
                gfx[row * 8 + half + 1] = 0xFF;
                gfx[row * 8 + half + 3] = 0xFF;
            }
        }

        let cfg = BoardConfig::sf2();
        // No sample ROM. **Task 10 must revisit this**: it puts the OKI's voices into
        // `MachineState`, and a round trip over a chip with nothing playing would be a
        // round trip over four stopped voices — trivially preserved. Until then a
        // restore rebuilds the chip at power-up, so a voice playing here would make the
        // fixture diverge for a reason the codec is not yet meant to fix.
        let mut m = Cps1::with_sound(
            &rom,
            gfx,
            sound_rom(),
            Vec::new(),
            cfg,
            Timing::cps1_10mhz(),
        );
        m.reset();
        // The sound board, before the run: the patch and the bank are what the Z80's
        // 5,241 lines of copying then interleave with.
        patch_the_chip(&mut m);
        m.sound.write(0xF004, 0x01); // bank 1
        m.sound.write(0xF006, 0x01); // OKI pin 7 high
        m.sound.write(0xF000, 0x30); // an address latched with no data byte yet
                                     // A byte for the Z80's copy loop to find, and a pattern across sound RAM. The
                                     // loop only ever writes 0xD000, so without the pattern 2,047 of the 2,048
                                     // bytes would be zero and a codec that wrote the region short would restore an
                                     // identical machine.
        m.board.sound_latch[0] = 0x5C;
        for i in 0..SOUND_RAM_BYTES as u16 {
            m.sound.write(0xD000 + i, (i as u8) ^ 0xA5);
        }
        m.board.cps_a[machine::video::regs::OBJ_BASE] = 0x40;
        m.board.gfxram[0x2000] = machine::video::VISIBLE_X as u16;
        m.board.gfxram[0x2001] = machine::video::VISIBLE_Y as u16;
        m.board.gfxram[0x2002] = 0;
        m.board.gfxram[0x2003] = 3;
        m.board.gfxram[0x2007] = 0xFF00;
        m.board.cps_b[cfg.video.palette_control] = 0x0001;
        m.board.gfxram[0x3A] = 0x0F0F;
        m.board.cps_a[machine::video::regs::PALETTE_BASE] = 0;
        // 5,241 lines is 20 frames and 1: a non-zero `line` and a non-zero `carry`,
        // which a frame boundary would hide. Held inputs and a coin counter too, so
        // the fields a quiet machine leaves at their defaults are non-default here.
        for _ in 0..5_241 {
            m.run_scanline();
        }
        // Every control is set to the *opposite* of the one encoded next to it, and
        // p2 is p1's complement. The payload is a run of one-byte booleans, so two
        // adjacent fields written in the wrong order is invisible unless the two
        // disagree — and the guest never reads a control, so no divergence test can
        // see it either. An alternating fixture makes any adjacent swap show up in
        // `every_field_survives_the_round_trip`.
        let inputs = &mut m.board.inputs;
        inputs.coin1 = true;
        inputs.coin2 = false;
        inputs.service = true;
        inputs.start1 = false;
        inputs.start2 = true;
        inputs.test = false;
        inputs.p1.right = true;
        inputs.p1.left = false;
        inputs.p1.down = true;
        inputs.p1.up = false;
        inputs.p1.punch = [true, false, true];
        inputs.p1.kick = [false, true, false];
        inputs.p2.right = false;
        inputs.p2.left = true;
        inputs.p2.down = false;
        inputs.p2.up = true;
        inputs.p2.punch = [false, true, false];
        inputs.p2.kick = [true, false, true];
        inputs.dsw = [0x5A, 0xA5, 0x3C];
        m.board.sound_latch = [0x12, 0x34];
        m.board.coin_ctrl = 0xBEEF;
        // A vblank the CPU has not acknowledged yet. The program acknowledges
        // promptly, so a snapshot taken at any quiet moment has this clear — and a
        // codec that wrote a constant `false` would then be indistinguishable. This
        // is the last thing the fixture does so the flag is still set.
        m.board.assert_vblank();
        m
    }

    /// What a run does, for the divergence comparison. Same shape as `machine`'s.
    ///
    /// # The sound half
    ///
    /// `samples`, `z80_cycles`, `z80_pc`, `ym_status`, and `sound_ram` are here because
    /// the video half cannot see the sound board at all: the 68000 never reads it, so
    /// a save state that dropped the whole Z80 would still reproduce every pen. The
    /// samples are the load-bearing one — they are a function of the register file, the
    /// envelope phase, the LFO, and the noise LFSR together — and `ym_status` is what
    /// makes the timers observable, since a timer's only visible effect is a status bit
    /// and an IRQ.
    #[derive(Debug, PartialEq, Eq)]
    struct Fingerprint {
        pens: Vec<u16>,
        vblanks: u64,
        acks: u64,
        total_cycles: u64,
        line: u32,
        d0: u32,
        d1: u32,
        samples: Vec<i16>,
        z80_cycles: u64,
        z80_pc: u16,
        ym_status: u8,
        sound_ram: Vec<u8>,
    }

    fn advance_and_fingerprint(m: &mut Cps1, lines: u32) -> Fingerprint {
        let (v0, a0) = (m.board.trace.vblanks, m.board.trace.acks);
        let c0 = m.z80_cycles();
        m.drain_samples();
        for _ in 0..lines {
            m.run_scanline();
        }
        m.render();
        Fingerprint {
            pens: m.video.fb.pens.to_vec(),
            vblanks: m.board.trace.vblanks - v0,
            acks: m.board.trace.acks - a0,
            total_cycles: m.total_cycles,
            line: m.line,
            d0: m.cpu.d[0],
            d1: m.cpu.d[1],
            // Every 71st sample: a stride co-prime with the 128-sample timer periods
            // and with a frame's sample count, so the kept samples are not one phase of
            // a repeating envelope. The whole run is ~44,000 samples and comparing all
            // of them makes the failure message unreadable.
            samples: m.samples().iter().copied().step_by(71).collect(),
            z80_cycles: m.z80_cycles() - c0,
            z80_pc: m.z80.pc,
            ym_status: m.sound.ym_ref().read_status(),
            sound_ram: m.sound.ram().to_vec(),
        }
    }

    /// Why a `decode` failed.
    ///
    /// `MachineState` has no `PartialEq` — see `machine::snapshot` for why — so a
    /// whole `Result` cannot be compared. This projects out the error, and panics
    /// with a message rather than a 264 KB `Debug` dump when a decode unexpectedly
    /// succeeds.
    fn err(bytes: &[u8], board: u32) -> StateError {
        match decode(bytes, board) {
            Ok(_) => panic!("expected a refusal, but {} bytes decoded", bytes.len()),
            Err(e) => e,
        }
    }

    /// CRC-32 against the specification's own check vector.
    ///
    /// `"123456789"` -> 0xCBF43926 is the CRC-32 spec's check value, and it is the
    /// same literal `romset`'s independent implementation is pinned against. Both
    /// are therefore checked against the standard rather than against each other,
    /// which is the point of not sharing the code.
    #[test]
    fn crc32_matches_the_standard_check_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0x0000_0000, "the empty string");
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    }

    /// A round trip through bytes restores the same machine's future.
    ///
    /// Divergence again, not comparison: encode, decode, restore, and require 7,777
    /// scanlines to match. Task 4 established that `snapshot` and `restore` carry
    /// everything; this establishes that the *bytes* do, and a field the encoder
    /// forgot fails here even though Task 4's tests pass.
    ///
    /// 7,777 lines is 29 frames and 179 — a partial frame at each end, so where the
    /// run started is observable. A whole number of frames sees one vblank per frame
    /// from any starting line, which is how a dropped `line` hides.
    #[test]
    fn a_state_survives_a_round_trip_through_bytes() {
        let mut m = a_machine();
        let bytes = encode(&m.snapshot(), BOARD_SF2);

        let first = advance_and_fingerprint(&mut m, 7_777);
        assert!(
            first.pens.iter().any(|&p| p != first.pens[0]),
            "the fixture must draw something, or the pen comparison proves nothing"
        );

        let decoded = decode(&bytes, BOARD_SF2).expect("a state this crate just wrote");
        m.restore(&decoded);
        let second = advance_and_fingerprint(&mut m, 7_777);

        assert_eq!(
            first, second,
            "a state that went through bytes must run the same 7,777 scanlines"
        );
    }

    /// And the comparison can fail.
    ///
    /// One scanline fewer must give a different fingerprint — otherwise the round
    /// trip above proves nothing. One line and not one frame, because one line is
    /// the smallest difference it could fail to notice.
    #[test]
    fn the_fingerprint_distinguishes_runs_one_scanline_apart() {
        let mut m = a_machine();
        let bytes = encode(&m.snapshot(), BOARD_SF2);
        let long = advance_and_fingerprint(&mut m, 7_777);
        m.restore(&decode(&bytes, BOARD_SF2).expect("valid"));
        let short = advance_and_fingerprint(&mut m, 7_776);
        assert_ne!(
            long, short,
            "if these matched, the round-trip test would prove nothing"
        );
    }

    /// The decoded state carries the individual fields, field by field.
    ///
    /// The round trip proves the *machine's future* survives, which is what matters
    /// — but it reports "something is wrong" rather than which field. This names
    /// them, and it is what catches a pair of adjacent same-width fields written in
    /// the other order: swapping two `bool`s the program never reads is invisible to
    /// a divergence test.
    #[test]
    fn every_field_survives_the_round_trip() {
        let m = a_machine();
        let s = m.snapshot();
        let d = decode(&encode(&s, BOARD_SF2), BOARD_SF2).expect("valid");

        assert_eq!(d.cpu.d, s.cpu.d, "data registers");
        assert_eq!(d.cpu.a, s.cpu.a, "address registers");
        assert_eq!(d.cpu.pc, s.cpu.pc, "pc");
        assert_eq!(d.cpu.sr, s.cpu.sr, "sr");
        assert_eq!(d.cpu.usp, s.cpu.usp, "usp");
        assert_eq!(d.cpu.ssp, s.cpu.ssp, "ssp");
        assert_eq!(d.cpu.prefetch, s.cpu.prefetch, "the prefetch queue");
        assert_eq!(d.cpu.halted, s.cpu.halted, "halted");
        assert_eq!(d.cpu.stopped, s.cpu.stopped, "stopped");
        assert_eq!(d.cpu.pending_irq, s.cpu.pending_irq, "pending_irq");
        assert_eq!(d.cpu.in_exception, s.cpu.in_exception, "in_exception");
        assert_eq!(d.cpu.trace_pending, s.cpu.trace_pending, "trace_pending");
        assert_eq!(&d.ram[..], &s.ram[..], "main RAM");
        assert_eq!(&d.gfxram[..], &s.gfxram[..], "gfxram");
        assert_eq!(d.cps_a, s.cps_a, "CPS-A");
        assert_eq!(d.cps_b, s.cps_b, "CPS-B");
        assert_eq!(d.sound_latch, s.sound_latch, "the sound latches");
        assert_eq!(d.coin_ctrl, s.coin_ctrl, "the coin control");
        assert_eq!(d.vblank_pending, s.vblank_pending, "the pending vblank");
        assert_eq!(d.total_cycles, s.total_cycles, "total cycles");
        assert_eq!(d.line, s.line, "the scanline");
        assert_eq!(d.carry, s.carry, "the scheduler's carry");
        assert_eq!(d.obj.words(), s.obj.words(), "the object latch");

        // `Inputs` has no `PartialEq`, so compare through the bus words the board
        // reads — which is what the guest can actually observe — plus the DIPs.
        assert_eq!(d.inputs.in0(), s.inputs.in0(), "IN0");
        assert_eq!(d.inputs.in1(), s.inputs.in1(), "IN1");
        assert_eq!(d.inputs.in2(), s.inputs.in2(), "IN2");
        assert_eq!(d.inputs.dsw, s.inputs.dsw, "the DIP switches");
        assert_eq!(d.inputs.start1, s.inputs.start1, "start1 specifically");
        assert_eq!(d.inputs.start2, s.inputs.start2, "and start2");
    }

    /// The fixture's fields are actually distinctive.
    ///
    /// Every assertion above compares decoded against original, so all of them pass
    /// for a fixture whose fields are all zero *and* a codec that writes zeros. This
    /// is the premise: the values are non-default, so the comparisons discriminate.
    #[test]
    fn the_fixture_is_not_a_quiet_machine() {
        let s = a_machine().snapshot();
        assert_ne!(s.line, 0, "mid-frame");
        assert_ne!(s.carry, 0, "and mid-scanline");
        assert_ne!(s.total_cycles, 0, "the machine ran");
        assert_ne!(s.cpu.d[0], 0, "the loop counted");
        assert_ne!(s.cpu.d[1], 0, "and the handler ran");
        assert_ne!(s.cpu.prefetch, [0, 0], "the prefetch queue is primed");
        assert_ne!(s.inputs.in0(), 0xFF, "a coin is held");
        assert_ne!(s.inputs.in1(), 0xFFFF, "and a stick and a punch");
        assert!(s.vblank_pending, "an unacknowledged vblank");
        // Adjacent controls disagree, which is what makes a swapped pair visible.
        assert_ne!(s.inputs.start1, s.inputs.start2, "start1 and start2");
        assert_ne!(s.inputs.coin1, s.inputs.coin2, "coin1 and coin2");
        assert_ne!(s.inputs.p1.right, s.inputs.p1.left, "p1 right and left");
        assert_ne!(s.inputs.p1.down, s.inputs.p1.up, "p1 down and up");
        assert_ne!(s.inputs.p1.punch[0], s.inputs.p1.punch[1], "p1 punches");
        assert_ne!(s.inputs.p1.kick[0], s.inputs.p1.kick[1], "p1 kicks");
        assert_ne!(s.inputs.p1.right, s.inputs.p2.right, "p1 against p2");
        assert_eq!(s.inputs.dsw, [0x5A, 0xA5, 0x3C], "the DIPs are not 0xFF");
        assert_eq!(s.sound_latch, [0x12, 0x34], "the latches are not zero");
        assert_ne!(s.coin_ctrl, 0, "nor the coin control");
        assert!(s.ram.iter().any(|&w| w != 0), "the program wrote RAM");
        assert!(s.obj.words().iter().any(|&w| w != 0), "and sprites latched");
    }

    /// The fixture's *sound* fields are distinctive too.
    ///
    /// The same premise as above, for the half the video assertions cannot see. Every
    /// sound comparison in `every_sound_field_survives_the_round_trip` is
    /// decoded-against-original, so all of them pass for a reset sound board and a
    /// codec that writes a reset one. These are the values that make them discriminate,
    /// and each one names what a codec dropping it would restore instead.
    #[test]
    fn the_fixture_is_not_a_quiet_sound_board() {
        let mut m = a_machine();
        let s = m.snapshot();

        assert_ne!(s.z80.pc, 0, "the Z80 is mid-loop, not at its reset vector");
        assert_ne!(s.z80.sp, 0, "and its stack pointer has moved");
        assert_ne!(s.z80_total, 0, "the sound CPU ran");
        assert_ne!(
            s.z80_debt, 0,
            "and it is mid-line: a debt is owed or overspent"
        );
        assert_eq!(
            s.sound_bank, 1,
            "bank 1, so a dropped bank reads other bytes"
        );
        assert!(s.oki_pin7, "pin 7 high, so a dropped `false` is visible");
        assert_eq!(s.ym_addr, 0x30, "an address latched with no data byte yet");
        // The copy loop ran: sound RAM's first byte is the latch the fixture set
        // before the run, not the 0xA5 the pattern put there. The latch itself is
        // 0x12 by snapshot time — `a_machine` sets it last, after the run — so this
        // also says the byte came from the Z80 rather than from a codec copying the
        // 68000's latch into the sound side.
        assert_eq!(
            s.sound_ram[0], 0x5C,
            "the Z80 copied the latch it was given"
        );
        assert_eq!(
            s.sound_ram[1],
            0x01 ^ 0xA5,
            "and left the pattern beside it"
        );
        assert!(
            s.sound_ram.iter().filter(|&&b| b != 0).count() > 2_000,
            "sound RAM is patterned, not one written byte in 2,048 zeros"
        );

        // The chip. Its interior is private, so it is measured the way a driver sees
        // it: its bytes against a reset chip's, and the sound it makes.
        let reset = machine::ym2151::Ym2151::new();
        assert_ne!(
            s.ym.write_state_bytes().as_slice(),
            reset.write_state_bytes().as_slice(),
            "the chip is not a reset chip"
        );
        assert_ne!(s.sample_acc, 0, "and it is mid-sample");
        m.drain_samples();
        for _ in 0..64 {
            m.run_scanline();
        }
        let sound = m.samples();
        assert!(!sound.is_empty(), "64 lines produce samples");
        assert!(
            sound.iter().any(|&s| s != 0),
            "and the patch makes sound, or every sample comparison is a comparison \
             of silence against silence"
        );
        assert!(
            sound.iter().any(|&s| s != sound[0]),
            "sound that changes, not one held level"
        );
    }

    /// The sound board's fields survive the bytes, field by field.
    ///
    /// The round trip proves the machine's *future* survives, which is what matters;
    /// this names which sound field is wrong when it does not. It is also what catches
    /// the two adjacent one-byte fields the guest never reads back — `sound_bank` and
    /// `oki_pin7` — whose swap no divergence test can see, since D2 has no OKI for pin
    /// 7 to reach.
    #[test]
    fn every_sound_field_survives_the_round_trip() {
        let s = a_machine().snapshot();
        let d = decode(&encode(&s, BOARD_SF2), BOARD_SF2).expect("valid");

        // `Z80` has `PartialEq`, so the whole CPU is one comparison — but name the
        // registers a swap would hide: the eight one-byte registers are written as a
        // run, and `assert_eq!` on the struct reports "not equal" without saying which.
        assert_eq!(d.z80.a, s.z80.a, "a");
        assert_eq!(d.z80.f, s.z80.f, "f");
        assert_eq!((d.z80.b, d.z80.c), (s.z80.b, s.z80.c), "bc");
        assert_eq!((d.z80.d, d.z80.e), (s.z80.d, s.z80.e), "de");
        assert_eq!((d.z80.h, d.z80.l), (s.z80.h, s.z80.l), "hl");
        assert_eq!((d.z80.i, d.z80.r), (s.z80.i, s.z80.r), "i and r");
        assert_eq!((d.z80.ix, d.z80.iy), (s.z80.ix, s.z80.iy), "ix and iy");
        assert_eq!(d.z80.sp, s.z80.sp, "sp");
        assert_eq!(d.z80.pc, s.z80.pc, "pc");
        assert_eq!(d.z80.wz, s.z80.wz, "wz");
        assert_eq!(d.z80.af_, s.z80.af_, "af'");
        assert_eq!(d.z80.bc_, s.z80.bc_, "bc'");
        assert_eq!(d.z80.de_, s.z80.de_, "de'");
        assert_eq!(d.z80.hl_, s.z80.hl_, "hl'");
        assert_eq!(d.z80, s.z80, "and the whole CPU, flags included");

        assert_eq!(&d.sound_ram[..], &s.sound_ram[..], "sound RAM");
        assert_eq!(d.sound_bank, s.sound_bank, "the ROM bank");
        assert_eq!(d.oki_pin7, s.oki_pin7, "OKI pin 7");
        assert_eq!(d.ym_addr, s.ym_addr, "the latched YM2151 address");
        // The chip through its own bytes: it has no `PartialEq` reachable from here
        // that would cover its private interior, and its layout is what a save state
        // actually carries.
        assert_eq!(
            d.ym.write_state_bytes().as_slice(),
            s.ym.write_state_bytes().as_slice(),
            "the FM chip"
        );
        assert_eq!(d.z80_carry.ratio(), s.z80_carry.ratio(), "the clock ratio");
        assert_eq!(
            d.z80_carry.remainder(),
            s.z80_carry.remainder(),
            "and the fraction it had carried"
        );
        assert_eq!(d.z80_debt, s.z80_debt, "the T-state debt");
        assert_eq!(d.z80_total, s.z80_total, "the T-state total");
        assert_eq!(d.sample_acc, s.sample_acc, "the sample accumulator");
    }

    /// The sound half is load-bearing in the fingerprint.
    ///
    /// `a_state_survives_a_round_trip_through_bytes` compares two fingerprints, and a
    /// fingerprint that only reflected the video half would let a save state drop the
    /// whole sound board and still pass. This resets the chip on the decoded state and
    /// requires the run to differ — so the comparison is one that can fail for a sound
    /// reason and not only for a video one.
    #[test]
    fn resetting_the_decoded_chip_changes_the_run() {
        let mut m = a_machine();
        let bytes = encode(&m.snapshot(), BOARD_SF2);

        m.restore(&decode(&bytes, BOARD_SF2).expect("valid"));
        let carried = advance_and_fingerprint(&mut m, 1_000);

        let mut broken = decode(&bytes, BOARD_SF2).expect("valid");
        broken.ym = machine::ym2151::Ym2151::new();
        m.restore(&broken);
        let reset = advance_and_fingerprint(&mut m, 1_000);

        assert_ne!(
            carried, reset,
            "a state whose chip came back reset must run differently"
        );
        assert_eq!(
            carried.pens, reset.pens,
            "and the difference is entirely in the sound half: the 68000 never reads \
             the sound board, so a fingerprint that noticed this in the pens would be \
             noticing something else"
        );
    }

    /// A version-1 file is refused, and told which version it is.
    ///
    /// Version 1 predates the sound board, so its payload has no Z80, no sound RAM,
    /// and no chip — and there is no defensible default for them. The file is
    /// well-formed and its CRC is valid; only the version byte differs, so this
    /// exercises the version check rather than the CRC.
    #[test]
    fn a_version_one_file_is_refused() {
        let mut bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        bytes[7] = 1;
        assert_eq!(
            crc32(&bytes[HEADER..HEADER + PAYLOAD]),
            u32::from_le_bytes(bytes[HEADER + PAYLOAD..].try_into().unwrap()),
            "the CRC covers only the payload, so it is still valid"
        );
        assert_eq!(
            err(&bytes, BOARD_SF2),
            StateError::Version { found: 1 },
            "a version-1 state must be told it is a version-1 state"
        );
    }

    /// A state written against a different sound clock is refused.
    ///
    /// The remainder is a fraction of the denominator it was taken modulo. A decoder
    /// that adopted the file's numbers would run the sound CPU at the file's clock; one
    /// that ignored them would resume a 1/2 remainder as 1/3,125. Both are wrong in a
    /// way nothing else in the format would catch, so the ratio is checked.
    ///
    /// The offset is *found* rather than hand-computed, so a later field reordering
    /// moves the test with the format instead of silently patching a `line` byte.
    #[test]
    fn a_state_from_another_sound_clock_is_refused() {
        let bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        let mut wanted = Z80_T_NUM.to_le_bytes().to_vec();
        wanted.extend_from_slice(&Z80_T_DEN.to_le_bytes());
        let payload = &bytes[HEADER..HEADER + PAYLOAD];
        let hits: Vec<usize> = payload
            .windows(8)
            .enumerate()
            .filter(|(_, w)| *w == &wanted[..])
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "the ratio appears exactly once in the payload"
        );

        let mut bad = bytes.clone();
        let at = HEADER + hits[0];
        bad[at + 4..at + 8].copy_from_slice(&(Z80_T_DEN * 2).to_le_bytes());
        let crc = crc32(&bad[HEADER..HEADER + PAYLOAD]);
        bad[HEADER + PAYLOAD..].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(
            err(&bad, BOARD_SF2),
            StateError::WrongSoundClock {
                found: (Z80_T_NUM, Z80_T_DEN * 2),
                expected: (Z80_T_NUM, Z80_T_DEN),
            },
            "a valid file with another board's sound clock is a clock error, not \
             a corrupt file"
        );
        // And the unpatched original still loads, so the check is not refusing
        // everything.
        assert!(decode(&bytes, BOARD_SF2).is_ok());
    }

    /// Encoding a decoded state reproduces the bytes exactly.
    ///
    /// This is what catches a field the writer writes and the reader never reads back:
    /// the decoded state carries `Z80::new()`'s or `Ym2151::new()`'s value for it, and
    /// re-encoding puts that default where the original byte was. It covers every byte
    /// of the payload at once, which a per-byte mutation test cannot afford to —
    /// 268,500 decode-and-re-CRC round trips of a 262 KB file.
    ///
    /// It rests on `the_fixture_is_not_a_quiet_machine` and
    /// `the_fixture_is_not_a_quiet_sound_board`: a field whose fixture value *is* the
    /// default would round-trip byte-identically while being dropped. Those two tests
    /// are why that is not the case here.
    #[test]
    fn re_encoding_a_decoded_state_reproduces_the_bytes() {
        let first = encode(&a_machine().snapshot(), BOARD_SF2);
        let decoded = decode(&first, BOARD_SF2).expect("valid");
        let second = encode(&decoded, BOARD_SF2);
        // Report the offset rather than dumping 262 KB twice.
        let differs = (0..first.len().min(second.len())).find(|&i| first[i] != second[i]);
        assert_eq!(
            differs, None,
            "byte {differs:?} changed: a field the encoder writes and the decoder \
             does not read back"
        );
        assert_eq!(first.len(), second.len());
    }

    /// A flipped bit anywhere in the sound half changes the decoded state.
    ///
    /// The complement of the test above: that one proves the reader reads every byte;
    /// this proves each byte it reads reaches a field the encoder writes again — a
    /// reader that consumed a byte and discarded it passes the idempotence test above
    /// and fails here.
    ///
    /// **Every 13th byte of the sound region, not all 4,039.** Each iteration decodes
    /// and re-encodes a 262 KB file and re-runs two CRCs over it, so full coverage is
    /// ~35× this test's cost for no new failure mode; 13 is co-prime with every field
    /// width in the region, so the sampled offsets land inside fields of all sizes
    /// rather than on 4-byte boundaries. The YM2151's own
    /// `every_byte_of_the_layout_changes_the_decoded_chip` covers its 1,919 bytes
    /// exhaustively.
    #[test]
    fn a_flipped_bit_in_the_sound_half_changes_the_decoded_state() {
        let good = encode(&a_machine().snapshot(), BOARD_SF2);
        // The sound half is the payload's tail: the video half's size is the version-1
        // payload, so the region starts where that ended.
        let sound_bytes = Z80_BYTES + SOUND_RAM_BYTES + 2 + YM_BYTES + 1 + 12 + 20;
        let start = HEADER + PAYLOAD - sound_bytes;
        assert_eq!(sound_bytes, 4_039, "the sound half's size, hand-counted");

        let mut checked = 0;
        for at in (start..HEADER + PAYLOAD).step_by(13) {
            let mut bad = good.clone();
            bad[at] ^= 0x01;
            let crc = crc32(&bad[HEADER..HEADER + PAYLOAD]);
            bad[HEADER + PAYLOAD..].copy_from_slice(&crc.to_le_bytes());
            // A flipped ratio byte is refused rather than decoded, which is that
            // field's own test above; every other byte must decode and differ.
            if let Ok(d) = decode(&bad, BOARD_SF2) {
                assert_ne!(
                    encode(&d, BOARD_SF2),
                    good,
                    "payload byte {} decoded to a state indistinguishable from the \
                     original: the decoder read that byte and threw it away",
                    at - HEADER
                );
                checked += 1;
            }
        }
        assert!(
            checked > 300,
            "and it checked {checked} bytes, not a handful"
        );
    }

    /// The CPU's flag bytes survive individually.
    ///
    /// A running machine has `halted`, `stopped`, `in_exception`, and
    /// `trace_pending` all false, so the fixture cannot distinguish them from each
    /// other and a swapped pair among them would be invisible. The state is still
    /// machine-produced — these are set on a real snapshot rather than on a
    /// hand-built one — and the four are given distinct values.
    #[test]
    fn the_cpu_flag_bytes_are_distinguishable() {
        let mut s = a_machine().snapshot();
        s.cpu.halted = true;
        s.cpu.stopped = false;
        s.cpu.in_exception = true;
        s.cpu.trace_pending = false;
        s.cpu.pending_irq = 5;
        let d = decode(&encode(&s, BOARD_SF2), BOARD_SF2).expect("valid");
        assert!(d.cpu.halted, "halted");
        assert!(!d.cpu.stopped, "stopped");
        assert!(d.cpu.in_exception, "in_exception");
        assert!(!d.cpu.trace_pending, "trace_pending");
        assert_eq!(d.cpu.pending_irq, 5, "and the pending level is a level");
    }

    /// The encoded length is the documented size.
    ///
    /// Hand-computed, so the format is a format rather than whatever the encoder
    /// happens to emit:
    ///
    /// ```text
    /// cpu     8*4 + 8*4 + 4 + 2 + 4 + 4 + 2*2 + 5        =     87
    /// ram     0x8000 * 2                                  =  65536
    /// gfxram  0x18000 * 2                                 = 196608
    /// cps_a   0x20 * 2                                    =     64
    /// cps_b   0x20 * 2                                    =     64
    /// board   2 latches + 2 coin_ctrl + 1 vblank          =      5
    /// inputs  6 + 2*10 + 3                                =     29
    /// sched   8 total_cycles + 4 line + 8 carry           =     20
    /// obj     0x400 * 2                                   =   2048
    /// ---- the sound board, new in version 2 ----
    /// z80     10*1 + 9*2 + 2 iff + 4 (im ei q p) + 3      =     37
    /// sndram  0x800                                       =   2048
    /// bank    1 sound_bank + 1 oki_pin7                   =      2
    /// ym      the chip's own layout                       =   1919
    /// ymaddr  1                                           =      1
    /// z80acc  4 num + 4 den + 4 remainder                 =     12
    /// z80sch  8 debt + 8 total + 4 sample_acc             =     20
    ///                                                       ------
    /// payload                                               268500
    /// header  8 magic + 4 board + 8 length                 =     20
    /// crc                                                  =      4
    ///                                                       ------
    /// total                                                 268524
    /// ```
    #[test]
    fn the_encoded_length_is_the_documented_size() {
        assert_eq!(PAYLOAD, 268_500, "the payload, term by term");
        let bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        assert_eq!(bytes.len(), 268_524, "20 header + payload + 4 CRC");
        assert_eq!(HEADER, 20);
        // The sound half is 4,039 of those bytes, and the YM2151 is most of it. Named
        // here so a change to the chip's private layout shows up as a save-state size
        // change rather than only as a mismatch inside `ym2151`.
        assert_eq!(Z80_BYTES, 37, "a hand count, not size_of::<Z80>() = 38");
        assert_eq!(SOUND_RAM_BYTES, 0x800);
        assert_eq!(YM_BYTES, 1_919, "the chip's own documented size");
        // And the declared length in the header agrees with what follows it.
        let declared = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
        assert_eq!(declared as usize, PAYLOAD, "the header declares the truth");
    }

    /// The header's bytes are where the format says.
    #[test]
    fn the_header_is_laid_out_as_documented() {
        let bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        assert_eq!(&bytes[0..8], b"SFEMU\0\0\x02", "magic, version in byte 7");
        assert_eq!(MAGIC[7], VERSION, "the version *is* the magic's last byte");
        assert_eq!(
            &bytes[8..12],
            b"\x00\x32\x46\x53",
            "b\"SF2\\0\" little-endian"
        );
        assert_eq!(BOARD_SF2, 0x5346_3200, "which reads as ASCII in a dump");
        let crc = u32::from_le_bytes(bytes[HEADER + PAYLOAD..].try_into().unwrap());
        assert_eq!(
            crc,
            crc32(&bytes[HEADER..HEADER + PAYLOAD]),
            "the CRC covers the payload"
        );
    }

    /// A file that is not a state is refused, and says so.
    #[test]
    fn a_file_that_is_not_a_state_is_refused() {
        let mut bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        assert!(decode(&bytes, BOARD_SF2).is_ok(), "the premise");
        bytes[0] = b'X';
        // Compared as errors and not as `Result`s: `MachineState` has no `PartialEq`,
        // deliberately, so that a save-state test cannot be written as
        // `snapshot == snapshot` — the comparison that passes for a codec dropping a
        // field the comparison also ignores.
        assert_eq!(err(&bytes, BOARD_SF2), StateError::NotAState);
        assert_eq!(
            err(b"not a save state at all, just some bytes", BOARD_SF2),
            StateError::NotAState,
        );
    }

    /// A future version is refused **by the version check**, not by the CRC.
    ///
    /// The version byte is patched *and the CRC left valid* — the payload is
    /// untouched, so it still matches. Without that, the test would pass on the CRC
    /// check and say nothing about version handling: an input that cannot exercise
    /// the property claimed, which is this branch's characteristic defect.
    #[test]
    fn a_future_version_is_refused_by_version_and_not_by_crc() {
        let mut bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        bytes[7] = VERSION + 1;
        // The CRC covers only the payload, so patching the header cannot invalidate
        // it. Asserted, because if the CRC ever covered the header this test would
        // silently start proving the wrong thing.
        let crc = u32::from_le_bytes(bytes[HEADER + PAYLOAD..].try_into().unwrap());
        assert_eq!(
            crc,
            crc32(&bytes[HEADER..HEADER + PAYLOAD]),
            "the CRC is still valid"
        );
        assert_eq!(
            err(&bytes, BOARD_SF2),
            StateError::Version { found: VERSION + 1 },
            "a next-version file must be told it is a next-version file"
        );
    }

    /// Another board's state is refused.
    #[test]
    fn another_boards_state_is_refused() {
        let bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        let sf1 = 0x5346_3100; // b"SF1\0"
        assert_eq!(
            err(&bytes, sf1),
            StateError::WrongBoard {
                found: BOARD_SF2,
                expected: sf1
            },
        );
        // And the state a board wrote loads into that board.
        assert!(decode(&bytes, BOARD_SF2).is_ok());
    }

    /// A truncated state is refused, at **every** prefix length.
    ///
    /// Every length from 0 to one byte short, not a hand-picked one: a single length
    /// passes for a decoder that checks only the header. 264,476 decodes of a 264 KB
    /// buffer, which is why this is a release-profile-friendly loop and nothing more
    /// clever.
    #[test]
    fn a_truncated_state_is_refused() {
        let bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        for n in 0..bytes.len() {
            assert!(
                decode(&bytes[..n], BOARD_SF2).is_err(),
                "a {n}-byte prefix of a {}-byte state must not decode",
                bytes.len()
            );
        }
        assert!(
            decode(&bytes, BOARD_SF2).is_ok(),
            "and the whole thing does"
        );
    }

    /// A corrupted payload is refused as **corrupt**, specifically.
    #[test]
    fn a_corrupted_payload_is_refused() {
        let mut bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        assert!(
            decode(&bytes, BOARD_SF2).is_ok(),
            "the premise: the fixture was not broken all along"
        );
        // One bit, in the middle of the payload.
        bytes[100_000] ^= 0x01;
        match decode(&bytes, BOARD_SF2) {
            Err(StateError::Corrupt { found, computed }) => {
                assert_ne!(found, computed, "and the two CRCs differ")
            }
            other => panic!("one flipped bit must be Corrupt, got {other:?}"),
        }
    }

    /// A declared length larger than the file is refused, without allocating.
    ///
    /// The length comes from a user's file. A decoder that trusted it would try to
    /// take a slice of 2^64 bytes and panic — so the check happens before the length
    /// is used for anything.
    #[test]
    fn an_impossible_declared_length_is_refused() {
        let mut bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        bytes[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
        match decode(&bytes, BOARD_SF2) {
            Err(StateError::Truncated { .. }) => {}
            other => panic!("a 2^64 payload must be Truncated, got {other:?}"),
        }
        // And a length one byte too long, which is the realistic version.
        let mut bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        bytes[12..20].copy_from_slice(&((PAYLOAD + 1) as u64).to_le_bytes());
        match decode(&bytes, BOARD_SF2) {
            Err(StateError::Truncated { .. }) => {}
            other => panic!("one byte too long must be Truncated, got {other:?}"),
        }
    }

    /// A payload that is short but internally consistent is refused too.
    ///
    /// Same magic, same board, valid CRC, wrong field set — a file a different build
    /// of this format could have written. It must be an error rather than a panic in
    /// the reader, which is what the length check after the CRC is for.
    #[test]
    fn a_consistent_but_wrong_sized_payload_is_refused() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&BOARD_SF2.to_le_bytes());
        let payload = vec![0xABu8; 64];
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        let crc = crc32(&payload);
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&crc.to_le_bytes());
        match decode(&bytes, BOARD_SF2) {
            Err(StateError::Truncated { need, .. }) => {
                assert_eq!(need, HEADER + PAYLOAD + 4, "and it names the real size")
            }
            other => panic!("a 64-byte payload must be Truncated, got {other:?}"),
        }
    }

    /// Any non-zero byte decodes as true.
    ///
    /// The byte came from a file. Refusing a state because a boolean is 2 is a
    /// rejection with no diagnostic value, so the decoder is permissive here — and
    /// this pins that, since `!= 0` and `== 1` are one character apart.
    #[test]
    fn any_non_zero_byte_is_true() {
        let mut m = a_machine();
        m.board.inputs.p1.up = false;
        let bytes = encode(&m.snapshot(), BOARD_SF2);
        // p1.up is the fourth boolean of p1's block. Find it by encoding both ways
        // and diffing, rather than by hand-computing an offset that a later field
        // reordering would silently invalidate.
        m.board.inputs.p1.up = true;
        let other = encode(&m.snapshot(), BOARD_SF2);
        let at = (0..bytes.len())
            .find(|&i| bytes[i] != other[i])
            .expect("p1.up must change the encoding");
        assert_eq!(bytes[at], 0, "false");
        assert_eq!(other[at], 1, "and true, so the encoder writes 0 and 1");

        let mut odd = bytes.clone();
        odd[at] = 0x7F;
        let payload_crc = crc32(&odd[HEADER..HEADER + PAYLOAD]);
        odd[HEADER + PAYLOAD..].copy_from_slice(&payload_crc.to_le_bytes());
        let d = decode(&odd, BOARD_SF2).expect("a valid file with an odd boolean");
        assert_eq!(
            d.inputs.in1(),
            m.board.inputs.in1(),
            "0x7F must read as true, the same as 1"
        );
    }

    /// No input makes the decoder panic.
    ///
    /// A frontend must never crash on a user's file, whatever is in it. Empty, one
    /// byte, the magic alone, a huge declared length, and a few thousand
    /// truncations and single-byte corruptions of a valid state.
    #[test]
    fn no_input_makes_the_decoder_panic() {
        for bad in [
            &b""[..],
            &b"S"[..],
            &MAGIC[..],
            &MAGIC[..7],
            &[0xFFu8; 32][..],
            &[0x00u8; 268_524][..],
        ] {
            let _ = decode(bad, BOARD_SF2);
        }
        // A huge declared length with nothing behind it.
        let mut header = MAGIC.to_vec();
        header.extend_from_slice(&BOARD_SF2.to_le_bytes());
        header.extend_from_slice(&u64::MAX.to_le_bytes());
        let _ = decode(&header, BOARD_SF2);

        let good = encode(&a_machine().snapshot(), BOARD_SF2);
        // Every 97th truncation: a stride co-prime with the field widths, so the cuts
        // land inside fields of every size rather than on aligned boundaries.
        for n in (0..good.len()).step_by(97) {
            let _ = decode(&good[..n], BOARD_SF2);
        }
        // And single-byte corruptions across the whole file, header included.
        for i in (0..good.len()).step_by(89) {
            let mut b = good.clone();
            b[i] = !b[i];
            let _ = decode(&b, BOARD_SF2);
        }
    }
}
