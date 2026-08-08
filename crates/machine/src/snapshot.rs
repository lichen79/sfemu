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
//! - **The ROM and the graphics ROM.** The user supplied them. A save state
//!   containing them would be a ROM file, which this project does not produce.
//! - **The palette and the framebuffer.** Recomputed by the next
//!   [`Cps1::render`](crate::Cps1::render).
//! - **The decoder table.** 512 KB, rebuilt in a constructor.
//! - **The [`Trace`](crate::Trace).** A record of the session, not state of the
//!   machine. Restoring it would also make a divergence test compare the first
//!   run's counters against a copy of themselves — the self-confirming shape this
//!   project exists to distrust.

use crate::board::{CPS_REGS, GFXRAM_WORDS, RAM_WORDS};
use crate::inputs::Inputs;
use m68k::M68k;
use video::sprites::ObjLatch;

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
}

/// Hand-written rather than derived, for the reason [`boxed_copy`] documents: the
/// derived `Clone` would route the two large arrays through `Box::clone` and
/// overflow the stack.
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
        d0: u32,
        d1: u32,
    }

    /// Runs `frames` frames and describes *what those frames did*.
    ///
    /// The trace counters are **deltas**, not absolutes. The trace is deliberately
    /// not restored — it records the session — so its absolute values necessarily
    /// differ between a first run and a replay, and comparing them would fail for
    /// the one reason that is correct behaviour. The delta is the interesting
    /// number: it says the replayed frames took and acknowledged the same
    /// interrupts and wrote gfxram the same number of times.
    fn advance_and_fingerprint(m: &mut Cps1, frames: u32) -> Fingerprint {
        let (v0, a0, w0) = (
            m.board.trace.vblanks,
            m.board.trace.acks,
            m.board.trace.gfxram_writes,
        );
        for _ in 0..frames {
            m.run_frame();
        }
        m.render();
        Fingerprint {
            pens: m.video.fb.pens.to_vec(),
            vblanks: m.board.trace.vblanks - v0,
            acks: m.board.trace.acks - a0,
            gfxram_writes: m.board.trace.gfxram_writes - w0,
            total_cycles: m.total_cycles,
            d0: m.cpu.d[0],
            d1: m.cpu.d[1],
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
    /// So: run 20 frames, snapshot, run 30 more and record what happened, restore,
    /// run the same 30, and require the framebuffer and the counters to match. A
    /// dropped `carry` shifts every later scanline boundary; a dropped
    /// `vblank_pending` doubles or misses an interrupt at the seam; a dropped object
    /// latch draws one frame of wrong sprites.
    #[test]
    fn a_restored_machine_runs_the_same_thirty_frames() {
        let mut m = machine();
        for _ in 0..20 {
            m.run_frame();
        }
        let s = m.snapshot();

        let first = advance_and_fingerprint(&mut m, 30);
        // The premise for comparing framebuffers at all: this one has more than one
        // pen in it. A uniform frame would make the `pens` half of the fingerprint
        // agree no matter what the sprite path did.
        assert!(
            first.pens.iter().any(|&p| p != first.pens[0]),
            "the fixture must draw something, or the pen comparison proves nothing"
        );

        m.restore(&s);
        let second = advance_and_fingerprint(&mut m, 30);

        assert_eq!(
            first, second,
            "a restored machine must run the same thirty frames"
        );
    }

    /// And the fingerprint can tell two runs apart.
    ///
    /// The test above is only meaningful if its comparison can fail. This runs a
    /// *different* number of frames and requires a different fingerprint — the
    /// control every "they matched" claim needs.
    #[test]
    fn the_fingerprint_distinguishes_different_runs() {
        let mut m = machine();
        for _ in 0..20 {
            m.run_frame();
        }
        let s = m.snapshot();
        let thirty = advance_and_fingerprint(&mut m, 30);
        m.restore(&s);
        let twenty_nine = advance_and_fingerprint(&mut m, 29);
        assert_ne!(
            thirty, twenty_nine,
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

    /// The scheduler's carry is part of the state.
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

    /// The pending vblank is part of the state.
    #[test]
    fn the_pending_vblank_is_part_of_the_state() {
        let mut m = machine();
        let mut s = m.snapshot();
        assert!(!s.vblank_pending, "a fresh machine has none");

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
