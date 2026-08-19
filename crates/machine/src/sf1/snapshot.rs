//! SF1's save-state data.
//!
//! A sibling of [`crate::MachineState`] and not a variant of it: see the plan's
//! Task 19, or in short — the two payloads share no field, no length and no
//! version, so a single codec would be two disjoint functions behind a `match`.
//!
//! # What is not in here
//!
//! The three ROM sets and the five graphics regions (the user supplied them), the
//! framebuffer and the converted palette (rebuilt by the next
//! [`crate::Sf1::render`]), the decoded tile ROMs (built in
//! [`video::sf1::Sf1Video::new`]), [`video::sf1::LayerMask`] (a debugger's
//! subtraction, not machine state), the three traces (records of the session),
//! and the sample queue (output the host drains).
//!
//! ⚠️ **No object latch.** [`crate::MachineState::obj`] is CPS-1's one-frame
//! sprite delay; SF1's sprite walk reads `objectram` directly, so the RAM this
//! state already carries is the whole of it.
//!
//! ⚠️ **`active` carries screen flip.** Flip is [`video::sf1::ACTIVE_FLIP`], bit 2
//! of that byte, alongside the four layer enables — not a pair of
//! `flip_x`/`flip_y` fields. There is one flip bit on this hardware and no
//! vertical one.
//!
//! ⚠️ The five `ACTIVE_*` constants live in **`video::sf1`** (Task 7), not in
//! `crate::sf1::board`: they describe how the compositor reads the byte, and the
//! 68000 board only latches it. A doc link to `crate::sf1::board::ACTIVE_FLIP`
//! does not resolve, and neither does `crate::sf1::Sf1Video` — `sf1/mod.rs`
//! re-exports `Msm5205`, `FmBoard`, `Adpcm2Board`, `Sf1`, `MSM_TICKS_PER_LINE`,
//! `MsmState` and `Sf1State`, and nothing from `video`; `machine.rs`'s
//! `use video::sf1::Sf1Video` is a private import, not a re-export. The two links
//! above are therefore written `video::sf1::…`. `deny(rustdoc::private_intra_doc_links)` plus
//! `cargo doc` are what say so.

// ⚠️ No `crate::inputs::PlayerInput` import: [`Sf1State::inputs`] carries
// [`Sf1Inputs`] whole, which holds the two `PlayerInput`s itself, and no doc link
// here names the inner type. The plan's draft imported it; `-D warnings` makes an
// unused import a build failure, and widening a doc link to justify the import
// would be the tail wagging the dog.
use crate::sf1::adpcm2::CHIPS;
use crate::sf1::board::{OBJECTRAM_WORDS, PALETTE_WORDS, RAM_WORDS, VIDEORAM_WORDS};
use crate::sf1::inputs::Sf1Inputs;
use crate::sf1::sound::RAM_BYTES as SOUND_RAM_BYTES;
use m68k::M68k;
use ym2151::Ym2151;

/// One MSM5205's state: the decoder, the latched nibble, the two pins, and the
/// armed capture's countdown.
///
/// A struct rather than six loose fields on [`Sf1State`], because there are two
/// chips and `[MsmState; CHIPS]` is how the codec walks them in a fixed order. A
/// pair of six-field groups would be twelve fields whose chip a reader has to
/// infer from the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsmState {
    /// The decoder's signal, in `-2048..=2047`.
    ///
    /// The decoder, not just the position — a chip restored at signal 0 clicks and
    /// then plays the next few dozen samples at the wrong amplitude.
    pub signal: i16,
    /// The decoder's step index, in `0..=48`.
    pub step: u8,
    /// The nibble a capture will decode, already masked to 4 bits.
    pub data: u8,
    /// The VCK input's level, so the next write can be compared against it.
    pub vck: bool,
    /// The reset **pin**, sampled at each capture. Not a device reset.
    pub reset: bool,
    /// Master clocks until the armed capture fires; 0 when none is armed.
    ///
    /// A countdown and not a deadline, which is what makes it one small number
    /// here instead of an absolute time this format would have to define.
    pub pending: u8,
}

/// A complete SF1 save state.
///
/// No `PartialEq`, for [`crate::MachineState`]'s reason: a save state is verified
/// by **divergence** — restore it, run, require the same future — because
/// `snapshot == snapshot` passes for a codec that drops a field the comparison
/// also ignores.
#[derive(Debug)]
pub struct Sf1State {
    /// The 68000, whole.
    pub cpu: M68k,
    /// Main RAM, 0x3000 words.
    pub ram: Box<[u16; RAM_WORDS]>,
    /// Sprite entries, read directly by the video — SF1 has no object latch.
    pub objectram: Box<[u16; OBJECTRAM_WORDS]>,
    /// The text plane's tiles.
    pub videoram: Box<[u16; VIDEORAM_WORDS]>,
    /// Palette RAM, 1,024 entries. Carried, not the converted colours: the
    /// conversion happens at render time from exactly this.
    pub palette: Box<[u16; PALETTE_WORDS]>,
    /// `m_active`: the four layer enables **and** screen flip
    /// ([`video::sf1::ACTIVE_FLIP`], bit 2).
    pub active: u8,
    /// The background plane's X scroll.
    pub bgscroll: u16,
    /// The foreground plane's X scroll.
    pub fgscroll: u16,
    /// Coin counters and lockouts.
    pub coin_ctrl: u8,
    /// Whether IPL1 is asserted and the 68000 has not yet fetched its vector.
    pub vblank_pending: bool,
    /// A sound command the 68000 has written and the sound board has not taken.
    ///
    /// The scheduler polls once per scanline, so a command waits up to 520.833
    /// cycles. A state taken in that window and restored without this loses the
    /// command *and* Z80 #1's NMI: a missing effect after a load, with everything
    /// else correct.
    pub sound_command: Option<u8>,
    /// Controls and DIP switches. A state restored without them drops the held
    /// direction mid-move.
    pub inputs: Sf1Inputs,

    // ------------------------------------------------------------- the schedule
    /// 68000 cycles since reset.
    pub total_cycles: u64,
    /// The current scanline.
    pub line: u32,
    /// The 68000's carried debt: where the machine is *within* a scanline.
    pub carry: i64,
    /// The line accumulator's carried fraction, in sixths of a cycle.
    ///
    /// SF1's line is 3,125/6 cycles, so this is never zero for five lines in six.
    pub line_remainder: u32,

    // --------------------------------------------------------------- Z80 #1, FM
    /// The FM Z80, whole.
    pub fm_z80: z80::Z80,
    /// Sound RAM, 2 KB.
    pub fm_ram: Box<[u8; SOUND_RAM_BYTES]>,
    /// The YM2151, whole: registers, envelopes, phases, LFO, noise, timers.
    ///
    /// Not just the register file — a chip restored without its envelope and phase
    /// counters sounds right for a few samples and then diverges.
    pub ym: Ym2151,
    /// The register a `0xE001` write would reach.
    pub ym_addr: u8,
    /// Z80 #1's copy of the machine's one sound latch.
    pub fm_latch: u8,
    /// FM Z80 T-states since reset.
    pub fm_total: u64,
    /// T-states granted to the current line and not yet spent.
    pub fm_debt: i64,
    /// The FM Z80's carried fraction of a T-state.
    ///
    /// **The field most easily forgotten**, and its absence is invisible for
    /// exactly one line — after which the two copies are one T-state apart and
    /// then diverge permanently.
    pub fm_remainder: u32,

    // ------------------------------------------------------------ Z80 #2, ADPCM
    /// The ADPCM Z80, whole.
    ///
    /// This board has **no RAM**, so these registers plus [`Self::adpcm_bank`] are
    /// its entire mutable state — and the position within the phrase being played,
    /// which an MSM6295 would hold in a voice, is in here.
    pub adpcm_z80: z80::Z80,
    /// The bank entry as the guest wrote it, unmasked.
    pub adpcm_bank: u8,
    /// Z80 #2's copy of the machine's one sound latch.
    pub adpcm_latch: u8,
    /// Both MSM5205s, in chip order.
    pub msm: [MsmState; CHIPS],
    /// ADPCM Z80 T-states since reset.
    pub adpcm_total: u64,
    /// T-states granted to the current line and not yet spent.
    pub adpcm_debt: i64,
    /// The ADPCM Z80's carried fraction of a T-state.
    pub adpcm_remainder: u32,
    /// The 8 kHz periodic interrupt's carried fraction, in forty-eighths.
    ///
    /// Its absence puts every later ADPCM sample up to 1/8000 s out of place,
    /// permanently — `set_periodic_int` (`sf.cpp:763`) paces the whole stream.
    pub adpcm_irq_remainder: u32,

    /// Input clocks accrued toward the next YM2151 sample.
    pub sample_acc: u32,
}

/// Hand-written rather than derived, for [`crate::MachineState`]'s reason: the
/// derived `Clone` routes each boxed array through `Box::clone`, which
/// materialises the whole array as a stack temporary before boxing it. The
/// largest here is `ram` at 24 KB — survivable, unlike CPS-1's 192 KB gfxram, but
/// written the same way so that a reader does not have to work out which of the
/// four boxes is the dangerous one, and so that a later field that *is* large
/// inherits the safe shape.
impl Clone for Sf1State {
    fn clone(&self) -> Self {
        Self {
            cpu: self.cpu.clone(),
            ram: crate::snapshot::boxed_copy(&self.ram),
            objectram: crate::snapshot::boxed_copy(&self.objectram),
            videoram: crate::snapshot::boxed_copy(&self.videoram),
            palette: crate::snapshot::boxed_copy(&self.palette),
            active: self.active,
            bgscroll: self.bgscroll,
            fgscroll: self.fgscroll,
            coin_ctrl: self.coin_ctrl,
            vblank_pending: self.vblank_pending,
            sound_command: self.sound_command,
            inputs: self.inputs,
            total_cycles: self.total_cycles,
            line: self.line,
            carry: self.carry,
            line_remainder: self.line_remainder,
            fm_z80: self.fm_z80.clone(),
            // 2 KB: a stack temporary is harmless, and written this way to match
            // the four above.
            fm_ram: Box::new(*self.fm_ram),
            ym: self.ym.clone(),
            ym_addr: self.ym_addr,
            fm_latch: self.fm_latch,
            fm_total: self.fm_total,
            fm_debt: self.fm_debt,
            fm_remainder: self.fm_remainder,
            adpcm_z80: self.adpcm_z80.clone(),
            adpcm_bank: self.adpcm_bank,
            adpcm_latch: self.adpcm_latch,
            msm: self.msm,
            adpcm_total: self.adpcm_total,
            adpcm_debt: self.adpcm_debt,
            adpcm_remainder: self.adpcm_remainder,
            adpcm_irq_remainder: self.adpcm_irq_remainder,
            sample_acc: self.sample_acc,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::sf1::{test_video, Sf1};

    /// A 68000 program that diverges visibly if a restored machine is even
    /// slightly off, and that touches every RAM the state carries.
    ///
    /// ```text
    /// 0000  0005 0000        SSP  (the reset vector's high word is at 0)
    /// 0004  0000 1000        PC
    /// 1000  46FC 2000        move #$2000,sr      supervisor, mask 0 -- take IRQs
    /// 1004  5240             addq.w #1,d0        a counter that never repeats
    /// 1006  33C0 00C0 2000   move.w d0,$C02000   main RAM
    /// 100C  33C0 00C0 4000   move.w d0,$C04000   objectram
    /// 1012  33C0 00C0 8000   move.w d0,$C08000   videoram
    /// 1018  33C0 00C0 C000   move.w d0,$C0C000   palette RAM
    /// 101E  60E4             bra $1004
    /// ```
    ///
    /// ⚠️ The four addresses are **placeholders for this doc only** — write them
    /// from `board.rs`'s own `RAM_BASE`, `OBJECTRAM_BASE`, `VIDEORAM_BASE` and
    /// `PALETTE_BASE` constants, which Task 9 defines and which this test imports
    /// rather than re-typing. A hand-typed address that happens to land in an
    /// unmapped hole makes this test pass while writing nothing.
    ///
    /// The vblank handler at 0x1100 counts interrupts in d1 and returns, so a
    /// restore that loses `vblank_pending` shows up as a different d1:
    ///
    /// ```text
    /// 1100  5241             addq.w #1,d1
    /// 1102  4E73             rte
    /// ```
    fn program() -> Vec<u8> {
        use crate::sf1::board::{OBJECTRAM_BASE, PALETTE_BASE, RAM_BASE, VIDEORAM_BASE};
        let mut rom = vec![0u8; 0x2000];
        // The reset vector: SSP at 0, PC at 4, both big-endian longwords.
        rom[0..8].copy_from_slice(&[0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00]);
        // Vector 25 — autovector 1, this board's vblank — at 25 * 4 = 0x64.
        rom[0x64..0x68].copy_from_slice(&[0x00, 0x00, 0x11, 0x00]);

        // `move #$2000,sr` then `addq.w #1,d0`.
        let mut at = 0x1000;
        for op in [0x46FCu16, 0x2000, 0x5240] {
            rom[at..at + 2].copy_from_slice(&op.to_be_bytes());
            at += 2;
        }
        // Four `move.w d0,<abs.L>` — one per RAM the state carries. The addresses
        // come from `board.rs`'s own constants rather than being typed out: a
        // hand-typed address landing in an unmapped hole makes this whole test
        // pass while writing nothing.
        for base in [RAM_BASE, OBJECTRAM_BASE, VIDEORAM_BASE, PALETTE_BASE] {
            rom[at..at + 2].copy_from_slice(&0x33C0u16.to_be_bytes());
            at += 2;
            rom[at..at + 4].copy_from_slice(&base.to_be_bytes());
            at += 4;
        }
        // `bra` back to the `addq` at 0x1004. The displacement is relative to the
        // instruction's own address plus 2, and it is computed rather than written
        // so that adding a fifth store above cannot silently break the loop.
        let target = 0x1004i32;
        let disp = target - (at as i32 + 2);
        rom[at..at + 2].copy_from_slice(&(0x6000u16 | (disp as i8 as u8 as u16)).to_be_bytes());

        // The vblank handler: `addq.w #1,d1` then `rte`. A restore that loses
        // `vblank_pending` shows up as a different d1.
        rom[0x1100..0x1104].copy_from_slice(&[0x52, 0x41, 0x4E, 0x73]);
        rom
    }

    /// A Z80 #1 program that reads the YM2151 forever, so the FM Z80 has a pc and a
    /// T-state phase that both move — with a stack and an NMI handler.
    ///
    /// ```text
    /// 0000  31 FF C7    ld sp,$C7FF    the top of sound RAM
    /// 0003  3A 01 E0    ld a,($E001)   the YM2151's data port
    /// 0006  32 00 C0    ld ($C000),a   into the first byte of sound RAM
    /// 0009  00          nop
    /// 000A  18 F7       jr $0003
    /// 0066  ED 45       retn           the NMI handler
    /// ```
    ///
    /// ⚠️ **The stack pointer and the handler are both load-bearing**, and only
    /// `a_pending_sound_command_survives_and_raises_one_nmi` reaches them — which is
    /// exactly why they are easy to leave out. `soundcmd_w` pulses this CPU's NMI and
    /// [`z80::Z80::ack_nmi`] *pushes the return address and jumps to 0x0066*; it is
    /// not a signal the CPU can ignore. `Z80::reset` leaves `sp` at 0xFFFF
    /// (`cpu.rs:190`), which on this board is **unmapped** — the push is discarded
    /// and a `retn` would pop 0xFFFF and land in unmapped space, which reads 0xFF,
    /// `RST 38h`, and pushes again. Without both of these the CPU leaves its loop on
    /// the first command and never comes back, so `pc` wanders and every later
    /// assertion about this CPU stops meaning what it says.
    ///
    /// The `nop` is load-bearing for the same reason `frontend`'s `sound_rom`
    /// keeps one: it makes the loop's T-state count share no factor with a line's
    /// 233.04, so the snapshot lands on a different instruction each time rather
    /// than always the same one.
    fn audiocpu() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0..12].copy_from_slice(&[
            0x31, 0xFF, 0xC7, 0x3A, 0x01, 0xE0, 0x32, 0x00, 0xC0, 0x00, 0x18, 0xF7,
        ]);
        rom[0x66..0x68].copy_from_slice(&[0xED, 0x45]);
        rom
    }

    /// A Z80 #2 program that feeds both MSM5205s, so the two decoders diverge from
    /// each other and from a reset chip.
    ///
    /// ```text
    /// 0000  3E 0F       ld a,$0F       reset pin clear, nibble F
    /// 0002  D3 00       out ($00),a    chip 0
    /// 0004  3E 02       ld a,$02       reset pin clear, nibble 2
    /// 0006  D3 01       out ($01),a    chip 1
    /// 0008  18 F6       jr $0000
    /// ```
    ///
    /// ⚠️ **Bit 7 of the byte is the reset pin, not part of the nibble.** Task 13's
    /// `msm_w` is `reset_w(val & 0x80 != 0)` and then `data_w(val)`, so any byte
    /// with the high bit set holds that chip in reset — and a chip captured under
    /// reset produces signal 0 and step 0 *without decoding*
    /// (`msm5205.cpp:194-198`). A byte like 0xF7 therefore leaves chip 1 identical
    /// to a reset chip, which is the one thing this fixture exists to avoid. Both
    /// bytes here are below 0x80.
    ///
    /// The two nibbles are chosen so the chips end up different in **every** field,
    /// which is what makes writing chip 0's state twice — or the two chips in the
    /// wrong order — visible in the round trip. Nibble 0xF is negative with the
    /// largest step and `0xF & 7 == 7` shifts the step index by +8, so chip 0 walks
    /// to step 48 and clamps at signal -2048; nibble 0x2 is positive and
    /// `0x2 & 7 == 2` shifts by **-1**, so chip 1 walks to step 0 and climbs to
    /// +2047. Two nibbles that both shifted the index upward would leave both chips
    /// at step 48 and the step byte would stop discriminating.
    ///
    /// Ports 0x00 and 0x01 are Task 13's, and its `port_out` decodes them as an
    /// identity onto the chip index (`sf.cpp:227-228`, `msm_w<0>` and `msm_w<1>`).
    ///
    /// This board has no RAM, so the program cannot use a stack: no `call`, no
    /// `push`. A `jr` loop is the whole vocabulary available.
    fn audio2() -> Vec<u8> {
        let mut rom = vec![0u8; 0x4_0000];
        rom[0..10].copy_from_slice(&[0x3E, 0x0F, 0xD3, 0x00, 0x3E, 0x02, 0xD3, 0x01, 0x18, 0xF6]);
        rom
    }

    /// The whole machine's future survives a snapshot and restore.
    ///
    /// The shape of every test below: run the original on, run the restored copy
    /// on by the same amount, and require the two futures to agree. Comparing the
    /// two *states* instead would pass for a codec that drops a field the
    /// comparison also drops.
    #[test]
    fn a_restored_machine_has_the_same_future() {
        let mut original = Sf1::new(&program(), test_video(), audiocpu(), audio2());
        original.reset();
        // Long enough for all three CPUs to be mid-stream and the schedule's four
        // remainders to be nonzero. 400 lines is more than one frame, so the
        // vblank has fired and `line` has wrapped.
        for _ in 0..400 {
            original.run_scanline();
        }
        let state = original.snapshot();

        let mut restored = Sf1::new(&program(), test_video(), audiocpu(), audio2());
        restored.reset();
        restored.restore(&state);

        for _ in 0..200 {
            original.run_scanline();
            restored.run_scanline();
        }

        assert_eq!(restored.cpu.d, original.cpu.d, "the 68000's data registers");
        assert_eq!(restored.cpu.a, original.cpu.a, "and its address registers");
        assert_eq!(restored.cpu.pc, original.cpu.pc);
        assert_eq!(restored.total_cycles, original.total_cycles);
        assert_eq!(restored.line, original.line);
        assert_eq!(restored.carry(), original.carry());
        assert_eq!(restored.z80_cycles(), original.z80_cycles(), "Z80 #1");
        assert_eq!(
            restored.adpcm_z80_cycles(),
            original.adpcm_z80_cycles(),
            "Z80 #2"
        );
        assert_eq!(restored.fm_z80.pc, original.fm_z80.pc);
        assert_eq!(restored.adpcm_z80.pc, original.adpcm_z80.pc);
        assert_eq!(restored.board.ram[..], original.board.ram[..], "main RAM");
        assert_eq!(restored.board.objectram[..], original.board.objectram[..]);
        assert_eq!(restored.board.videoram[..], original.board.videoram[..]);
        assert_eq!(restored.board.palette[..], original.board.palette[..]);
    }

    /// The audio the two machines produce is identical, sample for sample.
    ///
    /// A separate assertion from the registers above, and the stronger one: the
    /// YM2151's envelopes and both ADPCM decoders are invisible in a register
    /// comparison and audible in this one. `drain_samples` first, so each machine
    /// contributes only what it produced *after* the restore.
    #[test]
    fn the_restored_machine_produces_the_same_samples() {
        let mut original = Sf1::new(&program(), test_video(), audiocpu(), audio2());
        original.reset();
        for _ in 0..400 {
            original.run_scanline();
        }
        let state = original.snapshot();
        let _ = original.drain_samples();

        let mut restored = Sf1::new(&program(), test_video(), audiocpu(), audio2());
        restored.reset();
        restored.restore(&state);
        let _ = restored.drain_samples();

        for _ in 0..300 {
            original.run_scanline();
            restored.run_scanline();
        }
        let a = original.drain_samples();
        let b = restored.drain_samples();
        assert!(!a.is_empty(), "the premise: the board produced audio");
        assert_eq!(a, b, "every sample, both channels");
    }

    /// A pending sound command survives, and raises exactly one NMI.
    ///
    /// ⚠️ **Not** `assert_eq!(board.sound_command(), Some(0x42))` after a restore —
    /// that reads the same field the restore wrote. The artifact is the NMI: the
    /// command exists in order to pulse Z80 #1, so the test requires the restored
    /// machine to raise the same number of NMIs as the original.
    #[test]
    fn a_pending_sound_command_survives_and_raises_one_nmi() {
        let mut original = Sf1::new(&program(), test_video(), audiocpu(), audio2());
        original.reset();
        // One line first, so the FM Z80's `ld sp,$C7FF` has run: the NMI this test is
        // about *pushes*, and a push with `sp` still at its reset 0xFFFF goes into
        // unmapped space. See [`audiocpu`]. It does not change what this test
        // measures — the counter is incremented by the scheduler, not by the CPU —
        // but it keeps the machine the assertions run on a sane one.
        original.run_scanline();
        // Put a command in the latch without letting the scheduler poll it: write
        // it through the bus and snapshot before the next `run_scanline`.
        original.board.write_sound_command_for_test(0x42);
        assert!(
            original.board.sound_command().is_some(),
            "the premise: a command is pending"
        );
        let state = original.snapshot();
        assert_eq!(
            original.board.sound_command(),
            Some(0x42),
            "snapshot must not consume the command"
        );

        let mut restored = Sf1::new(&program(), test_video(), audiocpu(), audio2());
        restored.reset();
        restored.restore(&state);

        let before = restored.fm_nmis_raised();
        restored.run_scanline();
        assert_eq!(
            restored.fm_nmis_raised(),
            before + 1,
            "the restored command pulsed Z80 #1's NMI exactly once"
        );
        restored.run_scanline();
        assert_eq!(
            restored.fm_nmis_raised(),
            before + 1,
            "and not again: the latch is take-once"
        );
    }

    /// Each of the four schedule remainders matters on its own.
    ///
    /// A single "the future matches" test can pass while one remainder is dropped,
    /// if the drop happens to be invisible over the lines the test runs. So each
    /// is checked as a value that is nonzero at snapshot time and equal after —
    /// which is the one place in this module where comparing numbers rather than
    /// futures is the right test, because the *claim* is that the codec carries
    /// four specific numbers.
    #[test]
    fn all_four_schedule_remainders_are_carried() {
        let mut m = Sf1::new(&program(), test_video(), audiocpu(), audio2());
        m.reset();
        // 401 lines, not 400: an odd count leaves the 3,125/6 line accumulator
        // mid-fraction. Any count not a multiple of 6 does.
        for _ in 0..401 {
            m.run_scanline();
        }
        let s = m.snapshot();
        assert_ne!(s.line_remainder, 0, "the premise for line_cycles");
        assert_ne!(s.fm_remainder, 0, "the premise for Z80 #1");
        assert_ne!(s.adpcm_remainder, 0, "the premise for Z80 #2");
        assert_ne!(s.adpcm_irq_remainder, 0, "the premise for the 8 kHz IRQ");

        let mut r = Sf1::new(&program(), test_video(), audiocpu(), audio2());
        r.reset();
        r.restore(&s);
        assert_eq!(r.line_cycles_remainder(), s.line_remainder);
        assert_eq!(r.fm_carry_remainder(), s.fm_remainder);
        assert_eq!(r.adpcm_carry_remainder(), s.adpcm_remainder);
        assert_eq!(r.adpcm_irq_remainder(), s.adpcm_irq_remainder);
    }

    /// A restore does not reset the traces.
    ///
    /// A trace is an instrument, not machine state — and a restore that rewound
    /// the counters would make every divergence test above compare a run's
    /// counters against a copy of themselves.
    #[test]
    fn a_restore_leaves_the_traces_alone() {
        let mut m = Sf1::new(&program(), test_video(), audiocpu(), audio2());
        m.reset();
        for _ in 0..100 {
            m.run_scanline();
        }
        let s = m.snapshot();
        // ⚠️ `crate::Trace` is `Debug, Default, Clone` and **not** `PartialEq`
        // (`trace.rs:123`), so the whole struct cannot be compared. `FmTrace` and
        // `Adpcm2Trace` *are* `PartialEq`. So compare those two whole and the
        // 68000 board's by the one counter this test is about — and do not add
        // `PartialEq` to `Trace` for a test's convenience, which would widen a
        // shared type on CPS-1's side for SF1's reason.
        let before = (
            m.board.trace.vblanks,
            m.board.trace.sound_latch_writes,
            m.fm.trace(),
            m.adpcm.trace(),
        );
        m.restore(&s);
        assert_eq!(
            (
                m.board.trace.vblanks,
                m.board.trace.sound_latch_writes,
                m.fm.trace(),
                m.adpcm.trace()
            ),
            before,
            "a restore is not a session"
        );
    }

    /// A restore does not retract audio already queued for playback.
    #[test]
    fn a_restore_leaves_the_sample_queue_alone() {
        let mut m = Sf1::new(&program(), test_video(), audiocpu(), audio2());
        m.reset();
        for _ in 0..100 {
            m.run_scanline();
        }
        let s = m.snapshot();
        let queued = m.samples().len();
        assert!(queued > 0, "the premise");
        m.restore(&s);
        assert_eq!(m.samples().len(), queued, "output, not state");
    }

    /// `LayerMask` is a debugger's subtraction and is not restored.
    ///
    /// ⚠️ SF1's `LayerMask` has `all()` and **no `none()`** — Task 7 gives it four
    /// fields (`bg`, `fg`, `sprites`, `tx`) and one constructor. So the mask that
    /// is not the default is written as a struct literal, and it must not be
    /// `video::LayerMask`: that is CPS-1's four-field type with `scroll1..3`, a
    /// different struct that happens to share a name.
    #[test]
    fn a_restore_leaves_the_layer_mask_alone() {
        use video::sf1::LayerMask;
        let mut m = Sf1::new(&program(), test_video(), audiocpu(), audio2());
        m.reset();
        let s = m.snapshot();
        let inspecting = LayerMask {
            bg: false,
            ..LayerMask::all()
        };
        m.video.enable = inspecting;
        m.restore(&s);
        assert_eq!(
            m.video.enable, inspecting,
            "the person who loaded was mid-inspection"
        );
    }
}
