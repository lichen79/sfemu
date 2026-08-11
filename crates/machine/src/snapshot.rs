//! Save-state data: everything that makes the machine's future what it is.
//!
//! # Why this is a struct and not public fields
//!
//! Three of the values a save state needs are private, and two of them are private
//! for reasons the code documents at length: [`Cps1`](crate::Cps1)'s `carry` is the
//! scheduler's sub-frame position, and `Video`'s object latch is the one-frame
//! sprite delay. Widening those fields so a codec could read them would let any
//! later caller write them, which is exactly what their documentation argues
//! against.
//!
//! So the machine hands out a copy and takes one back. [`MachineState`]'s own
//! fields are public — it is a data carrier, and a codec has to read it field by
//! field.
//!
//! # What is not in here, and why
//!
//! - **The ROM, the graphics ROM, and the OKI's sample ROM.** The user supplied
//!   them. A save state containing them would be a ROM file, which this project does
//!   not produce — and one carrying 256 KB of Capcom samples would also be a save
//!   state that could not be shared. The voices carry *positions* into that ROM; a
//!   state loaded against a different one plays the wrong sound rather than failing,
//!   which is the same bargain the 68000's PC has always made.
//! - **The palette and the framebuffer.** Recomputed by the next
//!   [`Cps1::render`](crate::Cps1::render).
//! - **The decoder table.** 512 KB, rebuilt in a constructor.
//! - **The [`Trace`](crate::Trace).** A record of the session, not state of the
//!   machine. Restoring it would also make a divergence test compare the first
//!   run's counters against a copy of themselves — the self-confirming shape this
//!   project exists to distrust.

use crate::board::{CPS_REGS, GFXRAM_WORDS, RAM_WORDS};
use crate::inputs::Inputs;
use crate::sound::RAM_BYTES as SOUND_RAM_BYTES;
use crate::timing::RationalAccumulator;
use m68k::M68k;
use video::sprites::ObjLatch;
use ym2151::Ym2151;

/// A complete save state.
///
/// No `PartialEq`: nothing compares two states. A save state is verified by
/// **divergence** — restore it, run, and require the same future — because
/// `snapshot == snapshot` passes for a codec that drops a field the comparison
/// also ignores.
#[derive(Debug)]
pub struct MachineState {
    /// The CPU, whole.
    pub cpu: M68k,
    /// Main RAM.
    pub ram: Box<[u16; RAM_WORDS]>,
    /// Tilemap, sprite, and palette RAM.
    pub gfxram: Box<[u16; GFXRAM_WORDS]>,
    /// CPS-A.
    pub cps_a: [u16; CPS_REGS],
    /// CPS-B.
    pub cps_b: [u16; CPS_REGS],
    /// The sound latches.
    pub sound_latch: [u8; 2],
    /// Coin counters and lockouts.
    pub coin_ctrl: u16,
    /// Whether IPL1 is asserted and unacknowledged.
    ///
    /// A state taken between the assertion and the guest's vector fetch — one
    /// scanline in 262, so it happens — restores wrong without this, and the guest
    /// then misses or doubles that interrupt.
    pub vblank_pending: bool,
    /// Controls and DIP switches.
    ///
    /// Cheap, and a state restored mid-move without it drops the held direction.
    pub inputs: Inputs,
    /// Cycles since reset.
    pub total_cycles: u64,
    /// The current scanline.
    pub line: u32,
    /// The scheduler's carried debt: where the machine is *within* a scanline.
    ///
    /// Always `<= 0`. Omitting it puts a restored machine up to one instruction out
    /// of step, every line, forever.
    pub carry: i64,
    /// The previous frame's object table — the one-frame sprite delay.
    pub obj: ObjLatch,

    // ------------------------------------------------------------ the sound board
    /// The sound Z80, whole.
    pub z80: z80::Z80,
    /// Sound RAM.
    ///
    /// Boxed for the reason [`Self::ram`] is, at a twelfth of the size: a save-state
    /// codec assembles one of these from a file, and 2 KB on a test thread's stack is
    /// cheap to avoid.
    pub sound_ram: Box<[u8; SOUND_RAM_BYTES]>,
    /// The selected sound-ROM bank.
    pub sound_bank: u8,
    /// OKI pin 7, the MSM6295's rate divider select.
    ///
    /// Also the *rate* [`Self::oki_acc_rem`]'s fraction is measured against, which is
    /// why that field carries no numerator of its own: the ratio follows from this bit
    /// and the board's crystals through [`crate::timing::oki_per_ym`].
    pub oki_pin7: bool,
    /// The ADPCM chip's four voices: each one's decoder, position and volume.
    ///
    /// The decoder, not just the position. A voice restored at the right nibble with a
    /// reset decoder resumes at signal 0 and step index 0, which is a click and then a
    /// phrase at the wrong amplitude for the next few dozen samples — the same mistake
    /// [`Self::ym`] documents for the FM chip's envelopes.
    pub oki_voices: [oki::Voice; oki::VOICES],
    /// A phrase number latched by a `0x80`-prefixed byte, awaiting its voice mask.
    ///
    /// The OKI's start command is two bytes and the Z80 writes them as two
    /// instructions, so a state taken between them needs this. Without it the mask
    /// byte is read as a fresh command — `0x10` becomes a *stop* rather than the
    /// volume-and-voice half of a start — so the phrase never plays at all.
    pub oki_command: Option<u8>,
    /// The OKI sample accumulator's carried remainder.
    ///
    /// [`Self::sample_acc`]'s argument one chip along: at ~0.135 OKI samples per YM
    /// tick, dropping this puts a restored machine a fraction of an ADPCM sample out
    /// and the phrase drifts from there. Only the remainder, for the reason
    /// [`Self::oki_pin7`] gives.
    pub oki_acc_rem: u32,
    /// The chip's last output in the 2x domain, held between its own steps.
    ///
    /// Held, so it is state rather than scratch: most YM ticks step the chip zero
    /// times and the mix reuses this value, the way a sample-and-hold DAC does. A
    /// restore that zeroed it would put one silent sample into the middle of a phrase.
    pub oki_last: i32,
    /// The YM2151, whole: register file, envelopes, phases, LFO, noise, and timers.
    ///
    /// Not just the register file. A chip restored with its registers but not its
    /// envelope and phase counters sounds right for a few samples and then diverges,
    /// which is why `the_ym2151_envelope_and_phase_survive_a_save_state` compares
    /// produced samples rather than registers.
    pub ym: Ym2151,
    /// The address a write to 0xF000 latched, awaiting its data byte.
    ///
    /// The Z80 writes address and data as two instructions, so a state taken between
    /// them needs this or the next data byte lands in the wrong register.
    pub ym_addr: u8,
    /// The Z80's T-state accumulator, remainder included.
    ///
    /// **The field most easily forgotten.** Its absence is invisible for exactly one
    /// line, after which the two copies are one T-state apart and then diverge
    /// permanently — see `the_accumulator_remainder_survives_a_save_state`.
    pub z80_carry: RationalAccumulator,
    /// T-states granted to the current line and not yet spent.
    pub z80_debt: i64,
    /// Z80 T-states since reset.
    pub z80_total: u64,
    /// Input clocks accrued toward the next YM2151 sample.
    ///
    /// Omitting it puts every later sample up to 63 input clocks out of place, which
    /// is a click at the seam of every load.
    pub sample_acc: u32,
}

/// Hand-written rather than derived: the derived `Clone` would route the two large
/// arrays through `Box::clone`, which materialises the whole array as a temporary on
/// the stack before boxing it. For `gfxram` that is 192 KB and it overflows a test
/// thread's stack — an abort, not a failure. The private `boxed_copy` helper this
/// uses goes through the heap instead, and its own comment records the case that
/// found it.
///
/// The reason is spelled out here rather than linked, because the helper is
/// `pub(crate)`: a reader of these docs cannot follow the link.
impl Clone for MachineState {
    fn clone(&self) -> Self {
        Self {
            cpu: self.cpu.clone(),
            ram: boxed_copy(&self.ram),
            gfxram: boxed_copy(&self.gfxram),
            cps_a: self.cps_a,
            cps_b: self.cps_b,
            sound_latch: self.sound_latch,
            coin_ctrl: self.coin_ctrl,
            vblank_pending: self.vblank_pending,
            inputs: self.inputs,
            total_cycles: self.total_cycles,
            line: self.line,
            carry: self.carry,
            obj: self.obj.clone(),
            z80: self.z80.clone(),
            // 2 KB, so a stack temporary is harmless here — but written the same way
            // as the two above so that a reader does not have to work out which of
            // the three is which.
            sound_ram: Box::new(*self.sound_ram),
            sound_bank: self.sound_bank,
            oki_pin7: self.oki_pin7,
            oki_voices: self.oki_voices,
            oki_command: self.oki_command,
            oki_acc_rem: self.oki_acc_rem,
            oki_last: self.oki_last,
            ym: self.ym.clone(),
            ym_addr: self.ym_addr,
            z80_carry: self.z80_carry,
            z80_debt: self.z80_debt,
            z80_total: self.z80_total,
            sample_acc: self.sample_acc,
        }
    }
}

/// A boxed copy of a large array, built on the heap.
///
/// **Not `Box::clone`.** `<Box<[u16; N]> as Clone>::clone` is `Box::new((**self)
/// .clone())`, which materialises the whole array as a temporary on the stack
/// before boxing it. For gfxram that is 192 KB, and it overflows a test thread's
/// stack — the first run of `tests::held_inputs_are_part_of_the_state` aborted with
/// `stack overflow` rather than failing. `to_vec` allocates on the heap and the
/// conversion back is a pointer cast.
pub(crate) fn boxed_copy<const N: usize>(src: &[u16; N]) -> Box<[u16; N]> {
    src.to_vec()
        .into_boxed_slice()
        .try_into()
        .expect("a Vec built from an [u16; N] has exactly N elements")
}

#[cfg(test)]
mod tests {
    use crate::{BoardConfig, Cps1, Timing};

    /// A program whose state diverges visibly if a restored machine is even
    /// slightly off.
    ///
    /// ```text
    /// 1000  46FC 2000        move #$2000,sr     supervisor, mask 0 -- take IRQs
    /// 1004  5240             addq.w #1,d0       a counter that never repeats
    /// 1006  33C0 00FF 0000   move.w d0,$FF0000  into RAM
    /// 100C  33C0 0090 0000   move.w d0,$900000  and into gfxram, which the
    ///                                           renderer reads
    /// 1012  60F0             bra $1004
    /// ```
    ///
    /// The vblank handler at 0x1100 counts interrupts in d1 and returns, so a
    /// restore that loses `vblank_pending` shows up as a different d1.
    ///
    /// ```text
    /// 1100  5241             addq.w #1,d1
    /// 1102  4E73             rte
    /// ```
    ///
    /// Every encoding above was verified with `m68k::disasm::disassemble` on
    /// 2026-08-08 rather than transcribed. The mask must be 0, not 7: the whole
    /// point is that the guest takes and acknowledges the vblank.
    fn diverging_program() -> Vec<u8> {
        let mut rom = vec![0u8; 0x2000];
        // Reset vector: SSP 0x00FF8000, PC 0x00001000.
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        // Autovector 26 (IPL1) at 0x68 -> 0x00001100.
        rom[0x68..0x6C].copy_from_slice(&[0x00, 0x00, 0x11, 0x00]);
        rom[0x1000..0x1014].copy_from_slice(&[
            0x46, 0xFC, 0x20, 0x00, // move #$2000,sr
            0x52, 0x40, // addq.w #1,d0
            0x33, 0xC0, 0x00, 0xFF, 0x00, 0x00, // move.w d0,$FF0000
            0x33, 0xC0, 0x00, 0x90, 0x00, 0x00, // move.w d0,$900000
            0x60, 0xF0, // bra $1004
        ]);
        rom[0x1100..0x1104].copy_from_slice(&[
            0x52, 0x41, // addq.w #1,d1
            0x4E, 0x73, // rte
        ]);
        rom
    }

    /// A 16×16 tile solid in pen 0x0A, so the renderer draws something.
    ///
    /// The same byte pattern `sfemu`'s `a_drawn_frame` uses: 0x0A is bits 1 and 3,
    /// so planes 1 and 3 are solid. With `Vec::new()` for gfx every tile decodes as
    /// absent and the frame is uniform — which would make the framebuffer half of
    /// the fingerprint blind.
    fn a_tile() -> Vec<u8> {
        let mut gfx = vec![0u8; 128];
        for row in 0..16 {
            for half in [0usize, 4] {
                gfx[row * 8 + half + 1] = 0xFF;
                gfx[row * 8 + half + 3] = 0xFF;
            }
        }
        gfx
    }

    /// A machine running [`diverging_program`], with sprites and a palette set up
    /// so the frame is not one flat colour.
    fn machine() -> Cps1 {
        let cfg = BoardConfig::sf2();
        let mut m = Cps1::with_gfx(&diverging_program(), a_tile(), cfg, Timing::cps1_10mhz());
        m.reset();
        // One sprite of colour 3 at the top-left of the visible area, with an end
        // marker behind it. Object table at word 0x2000 (register 0x40 × 256).
        m.board.cps_a[video::regs::OBJ_BASE] = 0x40;
        m.board.gfxram[0x2000] = video::VISIBLE_X as u16;
        m.board.gfxram[0x2001] = video::VISIBLE_Y as u16;
        m.board.gfxram[0x2002] = 0;
        m.board.gfxram[0x2003] = 3;
        m.board.gfxram[0x2007] = 0xFF00;
        // Palette page 0 enabled, and the sprite's pen given a colour.
        m.board.cps_b[cfg.video.palette_control] = 0x0001;
        m.board.gfxram[0x3A] = 0x0F0F;
        m.board.cps_a[video::regs::PALETTE_BASE] = 0;
        m
    }

    /// Everything about a run that a wrong restore would change.
    ///
    /// The framebuffer **and** the counters, because a picture alone would miss a
    /// missed interrupt that happened to draw the same, and counters alone would
    /// miss a sprite drawn one frame late.
    #[derive(Debug, PartialEq, Eq)]
    struct Fingerprint {
        pens: Vec<u16>,
        vblanks: u64,
        acks: u64,
        gfxram_writes: u64,
        total_cycles: u64,
        line: u32,
        d0: u32,
        d1: u32,
        ram_word: u16,
        gfx_word: u16,
    }

    /// Runs `lines` **scanlines** and describes what they did.
    ///
    /// Scanlines and not frames, and a count that is deliberately not a multiple of
    /// 262. A whole number of frames contains exactly one vblank per frame wherever
    /// it starts, so it cannot see a restored `line` being wrong — the mutant
    /// dropping `self.line = s.line` from `restore` survived a 30-frame version of
    /// this. A partial frame at each end makes the starting line observable.
    ///
    /// The trace counters are **deltas**, not absolutes. The trace is deliberately
    /// not restored — it records the session — so its absolute values necessarily
    /// differ between a first run and a replay, and comparing them would fail for
    /// the one reason that is correct behaviour. The delta is the interesting
    /// number: it says the replayed lines took and acknowledged the same interrupts
    /// and wrote gfxram the same number of times.
    fn advance_and_fingerprint(m: &mut Cps1, lines: u32) -> Fingerprint {
        let (v0, a0, w0) = (
            m.board.trace.vblanks,
            m.board.trace.acks,
            m.board.trace.gfxram_writes,
        );
        for _ in 0..lines {
            m.run_scanline();
        }
        m.render();
        Fingerprint {
            pens: m.video.fb.pens.to_vec(),
            vblanks: m.board.trace.vblanks - v0,
            acks: m.board.trace.acks - a0,
            gfxram_writes: m.board.trace.gfxram_writes - w0,
            total_cycles: m.total_cycles,
            line: m.line,
            d0: m.cpu.d[0],
            d1: m.cpu.d[1],
            // A word of each memory the program writes. The loop rewrites every word
            // it touches from `d0`, so identical memory follows from an identical
            // `d0` — which is why the single-field tests below, not this one, are
            // what pin RAM and gfxram being restored at all.
            ram_word: m.board.ram[0],
            gfx_word: m.board.gfxram[0],
        }
    }

    /// A snapshot restores a machine that runs the same future.
    ///
    /// **This is the load-bearing test of the whole sub-project**, and it is a
    /// divergence test rather than a comparison. `snapshot == snapshot` passes for a
    /// codec that drops a field the comparison also ignores — and three of the
    /// fields that must be in a state are private, so that is exactly the mistake
    /// available here.
    ///
    /// So: run to a point **mid-frame**, snapshot, run 7,777 scanlines and record
    /// what happened, restore, run the same 7,777, and require the framebuffer and
    /// the counters to match. A dropped `carry` shifts every later scanline
    /// boundary; a dropped `vblank_pending` doubles or misses an interrupt at the
    /// seam; a dropped object latch draws one frame of wrong sprites.
    ///
    /// # Why the two odd numbers
    ///
    /// 5,241 lines is 20 frames and 1 line — so the snapshot is taken with a
    /// non-zero `line` and a non-zero `carry`, which a frame boundary would hide.
    /// 7,777 lines is 29 frames and 179 — a partial frame at each end, so the
    /// vblank count depends on where the run started. A whole number of frames sees
    /// exactly one vblank per frame from any starting line, and a 30-frame version
    /// of this test let the mutant dropping `self.line = s.line` survive.
    #[test]
    fn a_restored_machine_runs_the_same_seven_thousand_lines() {
        let mut m = machine();
        for _ in 0..5_241 {
            m.run_scanline();
        }
        let s = m.snapshot();
        assert_ne!(s.line, 0, "the premise: the snapshot is taken mid-frame");
        assert_ne!(s.carry, 0, "and mid-scanline, with debt carried");

        let first = advance_and_fingerprint(&mut m, 7_777);
        // The premise for comparing framebuffers at all: this one has more than one
        // pen in it. A uniform frame would make the `pens` half of the fingerprint
        // agree no matter what the sprite path did.
        assert!(
            first.pens.iter().any(|&p| p != first.pens[0]),
            "the fixture must draw something, or the pen comparison proves nothing"
        );

        m.restore(&s);
        let second = advance_and_fingerprint(&mut m, 7_777);

        assert_eq!(
            first, second,
            "a restored machine must run the same 7,777 scanlines"
        );
    }

    /// And the fingerprint can tell two runs apart.
    ///
    /// The test above is only meaningful if its comparison can fail. This runs one
    /// scanline fewer and requires a different fingerprint — the control every
    /// "they matched" claim needs. One line and not one frame, because one line is
    /// the smallest difference the test above could fail to notice.
    #[test]
    fn the_fingerprint_distinguishes_runs_one_scanline_apart() {
        let mut m = machine();
        for _ in 0..5_241 {
            m.run_scanline();
        }
        let s = m.snapshot();
        let long = advance_and_fingerprint(&mut m, 7_777);
        m.restore(&s);
        let short = advance_and_fingerprint(&mut m, 7_776);
        assert_ne!(
            long, short,
            "if these matched, the divergence test above would prove nothing"
        );
    }

    /// The program really does drive the interrupt path.
    ///
    /// Without this, a rom whose handler was never reached would make the
    /// `vblank_pending` half of the divergence test vacuous: no interrupt, nothing
    /// to lose at the seam. Every number here is a literal — 262 lines per frame,
    /// one vblank each.
    #[test]
    fn the_test_program_takes_and_acknowledges_its_interrupts() {
        let mut m = machine();
        m.run_frame();
        assert_eq!(m.board.trace.vblanks, 1, "one vblank per frame");
        assert_eq!(m.board.trace.acks, 1, "and the guest acknowledged it");
        assert_eq!(m.cpu.d[1], 1, "the handler ran exactly once");
        assert!(m.board.trace.gfxram_writes > 0, "and the loop wrote gfxram");
    }

    /// RAM and gfxram are part of the state.
    ///
    /// The divergence test cannot see this, and finding out why was worth the
    /// mutation pass: the test program rewrites every word it touches from `d0`, so
    /// restoring `d0` alone makes the memory converge within one loop iteration.
    /// Both mutants dropping a `copy_from_slice` survived it.
    ///
    /// So this writes a word each memory's *guest* never touches — the program
    /// writes RAM word 0 and gfxram word 0 — and requires it to come back. A word
    /// far from anything the fixture uses, so nothing else can be what restores it.
    #[test]
    fn ram_and_gfxram_are_part_of_the_state() {
        let mut m = machine();
        m.board.ram[0x1234] = 0xBEEF;
        m.board.gfxram[0x1_0000] = 0xCAFE;
        let s = m.snapshot();

        m.board.ram[0x1234] = 0x0000;
        m.board.gfxram[0x1_0000] = 0x0000;
        m.restore(&s);

        assert_eq!(m.board.ram[0x1234], 0xBEEF, "main RAM is restored");
        assert_eq!(m.board.gfxram[0x1_0000], 0xCAFE, "and so is gfxram");
    }

    /// The scanline counter is part of the state.
    ///
    /// The divergence test's earlier 30-*frame* form could not see this: a whole
    /// number of frames contains one vblank per frame from any starting line. It now
    /// runs 7,777 lines, which does see it — and this says which field, directly.
    #[test]
    fn the_scanline_counter_is_part_of_the_state() {
        let mut m = machine();
        for _ in 0..100 {
            m.run_scanline();
        }
        let s = m.snapshot();
        assert_eq!(s.line, 100, "100 lines run, 100 lines counted");

        for _ in 0..50 {
            m.run_scanline();
        }
        assert_eq!(m.line, 150, "the premise: the machine has moved on");

        m.restore(&s);
        assert_eq!(
            m.line, 100,
            "the beam goes back where the state left it, or the next vblank lands \
             in the wrong place"
        );
    }

    /// The scheduler's carry reaches the scheduler.
    ///
    /// The divergence test catches all three private fields together, which means it
    /// says "something is missing" rather than which. This says which: it restores
    /// two states differing only in `carry` and requires the next scanline to cost
    /// something different. A field restored but ignored fails here too.
    #[test]
    fn the_scheduler_carry_is_part_of_the_state() {
        let mut m = machine();
        m.run_scanline(); // leave a non-zero carry
        let s = m.snapshot();
        assert!(s.carry <= 0, "the carry is a debt, so never positive");

        let mut behind = s.clone();
        behind.carry -= 100;
        m.restore(&behind);
        let with = m.run_scanline();

        m.restore(&s);
        let without = m.run_scanline();

        assert_ne!(
            with, without,
            "the carry must reach the scheduler: a 100-cycle debt lengthens a line"
        );
    }

    /// And the carry is *captured*, not just restorable.
    ///
    /// A `snapshot` hard-coding `carry: 0` passes the test above — that one supplies
    /// its own carry values — and it also survives the divergence test, which was
    /// the surprise the mutation pass turned up. The reason is arithmetic: a
    /// scanline spends `640 + carry_in - carry_out` cycles, so over a run the carried
    /// terms cancel at both ends and a wrong starting carry changes no cycle total
    /// by more than one instruction's worth. The observable is the carry itself.
    ///
    /// So: reach a non-zero carry, snapshot, run on, restore, and require the same
    /// carry — read back through a second snapshot, which is the only way out.
    #[test]
    fn the_carry_is_captured_and_not_just_restorable() {
        let mut m = machine();
        m.run_scanline();
        let s = m.snapshot();
        let carry = s.carry;
        assert_ne!(
            carry, 0,
            "the premise: this fixture straddles line boundaries"
        );

        for _ in 0..7 {
            m.run_scanline();
        }
        assert_ne!(
            m.snapshot().carry,
            carry,
            "the premise: seven more lines moved the carry"
        );

        m.restore(&s);
        assert_eq!(
            m.snapshot().carry,
            carry,
            "a state that did not capture the carry restores the machine \
             mid-instruction out of step, every line, forever"
        );
    }

    /// The pending vblank is part of the state.
    #[test]
    fn the_pending_vblank_is_part_of_the_state() {
        let mut m = machine();
        let mut s = m.snapshot();
        assert!(!s.vblank_pending, "a fresh machine has none");

        // Captured, and not just restored. Asserted through the beam's own path
        // rather than the restore setter, so this cannot pass by the setter agreeing
        // with itself. A snapshot hard-coding `false` survives every restore
        // assertion below and fails here.
        m.board.assert_vblank();
        assert!(
            m.snapshot().vblank_pending,
            "a state taken with the line asserted must say so"
        );
        m.board.set_vblank_pending(false);

        s.vblank_pending = true;
        m.restore(&s);
        assert!(
            m.board.vblank_pending(),
            "a state taken mid-interrupt must restore the pending line, or the \
             guest misses or doubles that interrupt"
        );

        s.vblank_pending = false;
        m.restore(&s);
        assert!(!m.board.vblank_pending(), "and it restores false too");
    }

    /// Restoring a pending vblank does not count a second one.
    ///
    /// `Board::assert_vblank` counts the vblank in the trace, so a `restore` written
    /// in terms of it would inflate the count on every load — and the trace is what
    /// the divergence test reads. This is why `set_vblank_pending` exists separately.
    #[test]
    fn restoring_a_pending_vblank_does_not_count_one() {
        let mut m = machine();
        m.run_frame();
        let before = m.board.trace.vblanks;
        let mut s = m.snapshot();
        s.vblank_pending = true;
        m.restore(&s);
        assert_eq!(
            m.board.trace.vblanks, before,
            "a restore replays state, it does not re-assert an interrupt"
        );
    }

    /// The object latch is part of the state.
    ///
    /// One machine, not two: a `Cps1` is half a megabyte of decoder table built on
    /// the stack, and two live ones overflow a test thread — the first version of
    /// this file aborted rather than failed. Restoring over a *different* latched
    /// table is the stronger check anyway, since it requires `restore` to overwrite
    /// rather than merely fill in a zero.
    #[test]
    fn the_object_latch_is_part_of_the_state() {
        let mut m = machine();
        // The table lives at gfxram word 0x2000 — `cps_a[OBJ_BASE] = 0x40`, and
        // `cps_a_base` is `reg * 256` bytes — so latched word 3 is the first
        // record's colour, which `machine()` set to 3.
        m.board.gfxram[0x2003] = 0x0001;
        m.video.latch_objects(&m.board.gfxram[..], &m.board.cps_a);
        // Now change the table the guest sees *without* latching, which is the whole
        // point of the delay: the latch and gfxram disagree by one frame. A snapshot
        // taken here holds 1 in the latch and 2 in gfxram, so a `restore` that
        // re-latched from restored gfxram — a plausible wrong implementation — gives
        // 2 and fails, where a latch carried as state gives 1.
        m.board.gfxram[0x2003] = 0x0002;
        let s = m.snapshot();
        assert_eq!(
            s.obj.words()[3],
            0x0001,
            "the snapshot carries the latched table, not the table the guest wrote"
        );

        // Latch again, so the restore has something to undo.
        m.video.latch_objects(&m.board.gfxram[..], &m.board.cps_a);
        assert_eq!(m.video.obj_latch().words()[3], 0x0002, "the premise");

        m.restore(&s);
        assert_eq!(
            m.video.obj_latch().words()[3],
            0x0001,
            "sprites are delayed one frame, so a state without the latch draws one \
             frame of the wrong sprites"
        );
        assert_eq!(
            m.board.gfxram[0x2003], 0x0002,
            "and the latch is state in its own right: gfxram still says 2"
        );
    }

    /// Held inputs are part of the state.
    ///
    /// A state saved mid-move restores with the direction still held. Cheap to
    /// carry, and its absence would look like the stick dropping on every load.
    #[test]
    fn held_inputs_are_part_of_the_state() {
        let mut m = machine();
        m.board.inputs.p1.down = true;
        let s = m.snapshot();

        m.board.inputs.p1.down = false;
        assert_eq!(m.board.inputs.in1(), 0xFFFF, "the premise: nothing held");
        m.restore(&s);
        assert_eq!(
            m.board.inputs.in1(),
            0xFFFB,
            "down (IN1 bit 2) survives the restore"
        );
    }

    /// A snapshot carries no ROM and does not rewind the trace.
    ///
    /// The ROM and gfx are the user's files: a save state that embedded them would
    /// be a ROM file this project does not produce. The trace is a record of the
    /// session rather than state of the machine — and a restored trace would make
    /// the divergence test compare the first run's counters against a copy of
    /// themselves.
    #[test]
    fn a_snapshot_carries_no_rom_and_does_not_rewind_the_trace() {
        let mut m = machine();
        let s = m.snapshot();
        // The two large arrays are boxed, so this measures the inline part. A state
        // that had picked up the program ROM or the graphics ROM by value would be
        // orders of magnitude larger. 8 KB is loose on purpose: the point is the
        // order of magnitude, not the exact layout.
        assert!(
            core::mem::size_of_val(&s) < 8 * 1024,
            "{} bytes inline: the ROM is absent and the arrays are boxed",
            core::mem::size_of_val(&s)
        );

        m.run_frame();
        let frames = m.board.trace.frames;
        assert!(frames > 0, "the premise: the trace has counted something");
        m.restore(&s);
        assert_eq!(
            m.board.trace.frames, frames,
            "restoring must not rewind the trace: it records the session, not the \
             machine"
        );
        assert!(!m.board.rom.is_empty(), "and the ROM is still loaded");
    }
}
