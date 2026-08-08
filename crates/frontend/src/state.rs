//! The save-state file format.
//!
//! # The layout
//!
//! ```text
//! offset  size  field
//! 0       8     MAGIC          b"SFEMU\0\0\x01"; the last byte is the version
//! 8       4     board          little-endian; BOARD_SF2 = b"SF2\0"
//! 12      8     payload length little-endian
//! 20      len   payload
//! 20+len  4     CRC-32 of the payload, little-endian
//! ```
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
use machine::video::sprites::{ObjLatch, OBJ_WORDS};
use machine::{Inputs, MachineState, PlayerInput};

/// The first eight bytes of every save state. The last byte is [`VERSION`].
pub const MAGIC: [u8; 8] = *b"SFEMU\0\0\x01";

/// The format version, and the last byte of [`MAGIC`].
pub const VERSION: u8 = 1;

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
    + OBJ_WORDS * 2;

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

#[cfg(test)]
mod tests {
    use super::*;
    use machine::{BoardConfig, Cps1, Timing};

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
        let mut m = Cps1::with_gfx(&rom, gfx, cfg, Timing::cps1_10mhz());
        m.reset();
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
        m.board.inputs.p1.down = true;
        m.board.inputs.p2.punch[2] = true;
        m.board.inputs.coin1 = true;
        m.board.inputs.dsw = [0x5A, 0xA5, 0x3C];
        m.board.sound_latch = [0x12, 0x34];
        m.board.coin_ctrl = 0xBEEF;
        m
    }

    /// What a run does, for the divergence comparison. Same shape as `machine`'s.
    #[derive(Debug, PartialEq, Eq)]
    struct Fingerprint {
        pens: Vec<u16>,
        vblanks: u64,
        acks: u64,
        total_cycles: u64,
        line: u32,
        d0: u32,
        d1: u32,
    }

    fn advance_and_fingerprint(m: &mut Cps1, lines: u32) -> Fingerprint {
        let (v0, a0) = (m.board.trace.vblanks, m.board.trace.acks);
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
        assert_eq!(s.inputs.dsw, [0x5A, 0xA5, 0x3C], "the DIPs are not 0xFF");
        assert_eq!(s.sound_latch, [0x12, 0x34], "the latches are not zero");
        assert_ne!(s.coin_ctrl, 0, "nor the coin control");
        assert!(s.ram.iter().any(|&w| w != 0), "the program wrote RAM");
        assert!(s.obj.words().iter().any(|&w| w != 0), "and sprites latched");
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
    ///                                                       ------
    /// payload                                               264461
    /// header  8 magic + 4 board + 8 length                 =     20
    /// crc                                                  =      4
    ///                                                       ------
    /// total                                                 264485
    /// ```
    #[test]
    fn the_encoded_length_is_the_documented_size() {
        assert_eq!(PAYLOAD, 264_461, "the payload, term by term");
        let bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        assert_eq!(bytes.len(), 264_485, "20 header + payload + 4 CRC");
        assert_eq!(HEADER, 20);
        // And the declared length in the header agrees with what follows it.
        let declared = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
        assert_eq!(declared as usize, PAYLOAD, "the header declares the truth");
    }

    /// The header's bytes are where the format says.
    #[test]
    fn the_header_is_laid_out_as_documented() {
        let bytes = encode(&a_machine().snapshot(), BOARD_SF2);
        assert_eq!(&bytes[0..8], b"SFEMU\0\0\x01", "magic, version in byte 7");
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
            &[0x00u8; 264_485][..],
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
