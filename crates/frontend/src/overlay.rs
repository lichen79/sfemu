//! The debugger's panels, drawn into the framebuffer over the paused game.
//!
//! # Why in the framebuffer
//!
//! There is one window and no second surface. Drawing into the same `u32` frame the
//! game rendered into means the panels go through [`crate::pixels`] and the window
//! code unchanged, and it means this module is testable: a test renders a frame,
//! draws panels over it, and reads the characters back off the pixels with
//! `font::read_text`. Nothing here touches a window.
//!
//! (`read_text` is a plain code span rather than a rustdoc link: it is `#[cfg(test)]`,
//! so it does not exist in a doc build.)
//!
//! # Why the tests read the pixels back
//!
//! A panel test that compared against the same `format!` the panel used would assert
//! only that the formatter equals itself. Reading the glyphs back off the buffer
//! proves the formatting, the layout, the colour, and the clipping together. It
//! cannot prove the font table is right — the recogniser inverts that table — which
//! is what `font::tests::the_hex_digits_are_the_bitmaps_drawn_here` is for.
//!
//! # Nothing here is writable
//!
//! No register edit, no memory poke. Every read goes through
//! [`machine::cps1::Cps1::peek_word`], which takes `&self`, and `draw` takes a
//! `&Machine`.
//! A debugger that could write would need to answer what a half-modified machine
//! means for a save state and for the 127/127 vector suite, and the answer is not
//! worth having.

use crate::font::{draw_text, fill_rect, ADVANCE, LINE};
use machine::video::{HEIGHT, WIDTH};
use machine::{CpuView, Machine};

/// Panel background: dark, and opaque rather than blended.
///
/// Opaque because a blend would make the text's contrast depend on the game frame
/// underneath it — legible over a dark stage and not over a light one, which is a
/// bug that only appears on some screens.
const BG: u32 = 0x0000_0020;

/// Ordinary text.
///
/// `pub(crate)` for `sndpanel.rs`, which draws the one panel that forks per board.
/// Shared rather than copied: `gfxpanels.rs` holds its own `FG`, `HI` and `PAD`, and
/// its `PAD` is 2 where this one is 1 — a third copy would be the one that silently
/// misaligns.
pub(crate) const FG: u32 = 0x00D0_D0D0;

/// The executing instruction's line, and anything else the eye should go to first.
pub(crate) const HI: u32 = 0x0060_FF60;

/// A breakpoint marker.
const BP: u32 = 0x00FF_6060;

/// One pixel of padding inside a panel's box, so no glyph touches its edge.
pub(crate) const PAD: usize = 1;

/// Where the register panel's box starts.
pub const REGS_X: usize = 2;
/// Ditto.
pub const REGS_Y: usize = 2;
/// `Dn HHHHHHHH An HHHHHHHH` is 23 characters.
const REGS_COLS: usize = 23;
/// Eight register rows, plus PC, SR, and the cycle count.
const REGS_ROWS: usize = 11;

/// The taller of two row counts. `usize::max` is not `const` on this crate's
/// `rust-version`, and the layout constants must be `const` or they are not a layout.
const fn taller(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

/// How deep the top band is: registers on the left, memory on the right, and the
/// disassembly starts below **whichever is taller**.
///
/// Derived rather than written as a number. When it was `REGS_ROWS`, the memory
/// panel's extra row hung down into the disassembly's box and the two overlapped by
/// 165 pixels — caught by `all_four_panels_can_be_shown_at_once_without_overlapping`,
/// which is why that test compares claimed pixels rather than eyeballing the numbers.
const TOP_ROWS: usize = taller(REGS_ROWS, MEM_ROWS);

/// Where the disassembly panel's box starts: below the whole top band.
pub const DIS_X: usize = 2;
/// Ditto.
pub const DIS_Y: usize = REGS_Y + TOP_ROWS * LINE + 2 * PAD + 2;
/// `>*001000 move.w #$2000,sr` — 30 characters is enough for any of ours.
const DIS_COLS: usize = 30;
/// How many instructions the listing shows.
pub const DIS_ROWS: usize = 8;

/// Where the memory panel's box starts: to the right of the registers.
pub const MEM_X: usize = REGS_X + REGS_COLS * ADVANCE + 2 * PAD + 2;
/// Ditto.
pub const MEM_Y: usize = 2;
/// `FF0000 0000 0000 0000 0000` is 26 characters.
const MEM_COLS: usize = 26;
/// How many rows of four words the dump shows.
pub const MEM_ROWS: usize = 12;
/// Words per row of the dump.
pub const MEM_WORDS: usize = 4;

/// Where the sound panel's box starts: to the right of the disassembly.
///
/// The only free space of this shape. The top band is full — registers, then the
/// memory dump — and the bottom row is the status line, so the sound panel takes the
/// column beside the 68000 listing. Written as arithmetic on the disassembly's extent
/// rather than as `156`, so widening `DIS_COLS` moves this instead of silently
/// overlapping it.
pub const SND_X: usize = DIS_X + DIS_COLS * ADVANCE + 2 * PAD + 2;
/// Ditto: level with the disassembly, below the whole top band.
///
/// ⚠️ The sound panel's *widths* and *row counts* are not here: they are per-board and
/// live in [`crate::sndpanel`], which is where the panel itself does. Only the box's
/// origin is a layout decision this module owns, because it is the one the other four
/// panels' extents constrain.
pub const SND_Y: usize = DIS_Y;

/// Where the status line's box starts: the bottom of the screen.
pub const STATUS_X: usize = 2;
/// Ditto.
pub const STATUS_Y: usize = HEIGHT - LINE - 2 * PAD - 1;
/// Wide enough for the flags, the beam position, and `HALT`.
const STATUS_COLS: usize = 44;

/// Which panels are shown.
///
/// Five independent flags rather than a mode enum: the useful combinations are not a
/// sequence. Watching the beam position while stepping wants the status line and
/// nothing else; chasing a bad pointer wants registers and memory without the
/// disassembly taking half the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Panels {
    /// The 68000's registers, PC, SR, and the cycle count.
    pub regs: bool,
    /// A disassembly around the follow address.
    pub disasm: bool,
    /// A hex dump from the memory address.
    pub mem: bool,
    /// One line: flags, beam position, and whether the CPU is halted or stopped.
    pub status: bool,
    /// The sound board: the Z80, its listing, the latches, and the chip's key-on.
    pub sound: bool,
}

impl Panels {
    /// Nothing shown, which is what `F1` toggles away to.
    ///
    /// `draw` with this must leave the frame untouched — an overlay that drew its
    /// background regardless would black out the game when switched off.
    pub const fn none() -> Self {
        Self {
            regs: false,
            disasm: false,
            mem: false,
            status: false,
            sound: false,
        }
    }

    /// Registers, disassembly, the sound board, and the status line: what `F1`
    /// switches on.
    ///
    /// Not the memory dump. It needs an address to be worth its width, and the
    /// address it would default to is a guess; `PageUp`/`PageDown` and `F6` are how
    /// you get it, and having asked for it is what makes it useful.
    ///
    /// The sound panel *is* in here, and the memory dump's argument does not apply to
    /// it: it needs no address, it always describes the machine, and it occupies space
    /// nothing else uses. There is also no key that shows it on its own, so leaving it
    /// out would make it unreachable — a panel nobody can display is a panel that
    /// silently rots.
    pub const fn on() -> Self {
        Self {
            regs: true,
            disasm: true,
            mem: false,
            status: true,
            sound: true,
        }
    }

    /// Whether any panel is shown.
    pub const fn any(self) -> bool {
        self.regs || self.disasm || self.mem || self.status || self.sound
    }
}

/// The address of the instruction about to execute.
///
/// ⚠️ **Not `cpu.pc`.** That field is always four bytes beyond the instruction being
/// executed, because of the two-word prefetch queue — `M68k::pc`'s own docs say so.
/// A disassembly marker comparing against `cpu.pc` would point two instructions
/// ahead, and a breakpoint comparing against it would fire late or, for a
/// three-word instruction, never.
///
/// Defined once, here, because the disassembly marker and Task 5's breakpoints must
/// agree about it. `wrapping_sub` because the arithmetic is the 68000's: a PC of 2
/// after a reset vector pointing near zero is not a case to panic on.
///
/// Takes the view rather than a board: this is the arithmetic every CPU panel needs
/// and no board is involved in it.
pub fn executing_pc(v: &CpuView<'_>) -> u32 {
    v.cpu.pc.wrapping_sub(4)
}

/// Draws the enabled panels into `buf`, over whatever the game rendered.
///
/// `disasm_at` is the address the listing starts at and `mem_at` the address the dump
/// starts at — both the caller's, so that `F4`'s "follow the PC" and `PageUp`'s
/// scrolling are decisions made in one place rather than remembered here. `bp` is the
/// breakpoint list, marked in the listing.
///
/// `peek` is how the memory and disassembly panels read the machine. A closure rather
/// than a field on [`CpuView`], because a `&dyn Fn` in the view would make it neither
/// `Copy` nor `Debug` and would hand a vtable to the twenty-odd functions that only
/// want registers. See `draw_mem`'s own note (private) for why it must be the
/// *side-effect-free* peek and not the CPU's read path.
///
/// ⚠️ `m: &Machine` is here for one panel: the sound panel is the one that forks per
/// board, and it dispatches on the variant itself. Every other panel below reads `v`.
///
/// # Panics
///
/// If `buf` is not a `WIDTH × HEIGHT` frame, as [`draw_text`].
// Eight parameters: the frame, the CPU view, the memory peek, the machine (for the
// sound panel, which is the one panel that forks per board), the panel flags, two
// scroll addresses and the breakpoint list. Every one is independent — bundling them
// into a struct would move the same eight names one level down and hand the caller a
// literal to fill in, which is not fewer decisions.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    buf: &mut [u32],
    v: &CpuView<'_>,
    peek: &dyn Fn(u32) -> Option<u16>,
    m: &Machine,
    p: Panels,
    disasm_at: u32,
    mem_at: u32,
    bp: &[u32],
) {
    assert_eq!(buf.len(), WIDTH * HEIGHT, "not a frame");
    if p.regs {
        draw_regs(buf, v);
    }
    if p.disasm {
        draw_disasm(buf, v, peek, disasm_at, bp);
    }
    if p.mem {
        draw_mem(buf, peek, mem_at);
    }
    if p.sound {
        crate::sndpanel::draw(buf, m);
    }
    if p.status {
        draw_status(buf, v);
    }
}

/// A panel's background, and the `(x, y)` of its first line of text.
///
/// `pub(crate)` for [`crate::sndpanel`], which draws its two boxes with this rather
/// than a copy: the chrome is what must not drift between panel modules.
pub(crate) fn box_at(
    buf: &mut [u32],
    x: usize,
    y: usize,
    cols: usize,
    rows: usize,
) -> (usize, usize) {
    fill_rect(
        buf,
        x,
        y,
        cols * ADVANCE + 2 * PAD,
        rows * LINE + 2 * PAD,
        BG,
    );
    (x + PAD, y + PAD)
}

/// D0-D7 beside A0-A7, then PC, SR, and the cycle count.
fn draw_regs(buf: &mut [u32], v: &CpuView<'_>) {
    let (x, y) = box_at(buf, REGS_X, REGS_Y, REGS_COLS, REGS_ROWS);
    for i in 0..8 {
        // A7 comes from `a[7]`, never from `usp`/`ssp`: those are shadows written for
        // legibility and the active pointer is the one in `a[7]`. A panel reading the
        // shadow would show a plausible number that is wrong in exactly the situation
        // — inside an exception handler — where you are most likely reading it.
        let line = format!("D{i} {:08X} A{i} {:08X}", v.cpu.d[i], v.cpu.a[i]);
        draw_text(buf, x, y + i * LINE, &line, FG);
    }
    draw_text(
        buf,
        x,
        y + 8 * LINE,
        &format!("PC {:08X} SR {:04X}", executing_pc(v), v.cpu.sr),
        HI,
    );
    draw_text(
        buf,
        x,
        y + 9 * LINE,
        &format!("CYC {:012}", v.total_cycles),
        FG,
    );
    draw_text(
        buf,
        x,
        y + 10 * LINE,
        &format!("FRM {:010}", v.trace.frames),
        FG,
    );
}

/// The listing, marking the executing line and any breakpoint.
fn draw_disasm(
    buf: &mut [u32],
    v: &CpuView<'_>,
    peek: &dyn Fn(u32) -> Option<u16>,
    at: u32,
    bp: &[u32],
) {
    let (x, y) = box_at(buf, DIS_X, DIS_Y, DIS_COLS, DIS_ROWS);
    let pc = executing_pc(v);
    let mut a = at;
    for row in 0..DIS_ROWS {
        // `peek`, not the bus: a disassembly panel scrolled over $68 must not
        // acknowledge the interrupt it is there to explain. An undecoded address
        // reads as all ones, which the disassembler renders `dc.w $FFFF` — the
        // honest answer for a listing pointed at nothing.
        let insn = machine::m68k::disasm::disassemble(|w| peek(w).unwrap_or(0xFFFF), a);
        let marker = if a == pc { '>' } else { ' ' };
        let brk = if bp.contains(&a) { '*' } else { ' ' };
        let fg = if a == pc { HI } else { FG };
        draw_text(
            buf,
            x,
            y + row * LINE,
            &format!("{marker}{brk}{a:06X} {}", insn.text),
            fg,
        );
        // A breakpoint marker in its own colour, over the space just drawn, so it is
        // visible on a line that is not the current one.
        if brk == '*' {
            draw_text(buf, x + ADVANCE, y + row * LINE, "*", BP);
        }
        // `len` is at least 2, so this always advances and the listing cannot loop.
        a = a.wrapping_add(insn.len);
    }
}

/// Four words per row, `--` for anything undecoded.
fn draw_mem(buf: &mut [u32], peek: &dyn Fn(u32) -> Option<u16>, at: u32) {
    let (x, y) = box_at(buf, MEM_X, MEM_Y, MEM_COLS, MEM_ROWS);
    for row in 0..MEM_ROWS {
        let base = at.wrapping_add((row * MEM_WORDS * 2) as u32);
        let mut line = format!("{base:06X}");
        for w in 0..MEM_WORDS {
            let a = base.wrapping_add((w * 2) as u32);
            // `--` rather than `FFFF`: "nothing decodes here" and "this decodes and
            // reads as all ones" are different facts, and a panel that conflated them
            // would send you looking for a chip that is not there. $800020 genuinely
            // reads 0xFFFF and is decoded, which is what makes this distinction real
            // rather than theoretical.
            match peek(a) {
                Some(v) => line.push_str(&format!(" {v:04X}")),
                None => line.push_str("   --"),
            }
        }
        draw_text(buf, x, y + row * LINE, &line, FG);
    }
}

/// Flags, the beam, and whether the CPU is running at all.
fn draw_status(buf: &mut [u32], v: &CpuView<'_>) {
    let (x, y) = box_at(buf, STATUS_X, STATUS_Y, STATUS_COLS, 1);
    let f = |bit: u16, c: char| if v.cpu.sr & bit != 0 { c } else { '-' };
    let flags: String = [
        f(0x0010, 'X'),
        f(0x0008, 'N'),
        f(0x0004, 'Z'),
        f(0x0002, 'V'),
        f(0x0001, 'C'),
    ]
    .into_iter()
    .collect();
    // `HALT` and `STOP` are different machines: a halted 68000 took a double bus
    // fault and will never run again, while a stopped one is waiting for an
    // interrupt and will. Showing one for the other sends you to the wrong question.
    let run = if v.cpu.halted {
        "HALT"
    } else if v.cpu.stopped {
        "STOP"
    } else {
        "RUN "
    };
    let ipl = (v.cpu.sr >> 8) & 7;
    // A *field*, not a call: `Machine::cpu_view` filled it by asking the board. A
    // mechanical `m.` → `v.` would leave `v.vblank_pending()` here, which does not
    // compile — which is the right way for that to fail.
    let irq = if v.vblank_pending { "IRQ" } else { "   " };
    draw_text(
        buf,
        x,
        y,
        &format!(
            "{run} {flags} IPL{ipl} {irq} LINE {:03} S{}",
            v.line,
            u8::from(v.cpu.sr & 0x2000 != 0)
        ),
        HI,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::{frame, panel_contains, read_text};
    // The sound machine moved out with the panel it was written for, and this module's
    // `the_sound_panel_renders_without_panicking` still draws that panel through
    // `draw`. Shared rather than copied: a second copy here would go on claiming to
    // run a driver after the real one's opcodes changed.
    use crate::sndpanel::a_sound_machine;
    use machine::config::BoardConfig;
    use machine::timing::Timing;

    /// A machine stopped part-way through a program, with something in every field a
    /// panel reads.
    ///
    /// The program at 0x1000 is deliberately mixed-width — `move #imm,sr` is two
    /// words and `move.w d0,$FF0000` is three — because a listing of one-word
    /// instructions cannot tell a correct `insn.len` from a hardcoded 2, and
    /// `executing_pc` cannot be told from `pc` itself unless the instruction it
    /// points at is longer than one word.
    /// The 68000 program both fixtures run.
    fn prog() -> Vec<u8> {
        let mut rom = vec![0u8; 0x2000];
        // SSP 0x00FF8000, PC 0x1000.
        rom[0..8].copy_from_slice(&[0x00, 0xFF, 0x80, 0x00, 0x00, 0x00, 0x10, 0x00]);
        rom[0x1000..0x100E].copy_from_slice(&[
            0x46, 0xFC, 0x20, 0x00, // move #$2000,sr   (2 words)
            0x33, 0xC0, 0x00, 0xFF, 0x00, 0x00, // move.w d0,$FF0000 (3 words)
            0x52, 0x40, // addq.w #1,d0    (1 word)
            0x60, 0xFA, // bra.s back      (1 word)
        ]);
        rom
    }

    /// The board inside a [`Machine`], and the machine around a board.
    ///
    /// `draw`'s fourth argument is a `&Machine` now, because the sound panel forks per
    /// board — but these tests still need the `Cps1`: they set `cpu.stopped`, they poke
    /// `board.ram`, and they read `board.trace` back afterwards to assert the panel
    /// disturbed nothing. Both at once is what these two give, and reading the board
    /// *out of the machine that was drawn* is the point: a decoy `Machine` beside the
    /// board would let `drawing_the_memory_panel_does_not_disturb_the_machine` assert
    /// about a machine no panel ever saw.
    fn wrap(m: machine::Cps1) -> Machine {
        Machine::Cps1(Box::new(m))
    }

    /// See [`wrap`]. The `unreachable!` is honest: every caller wrapped a `Cps1`.
    fn as_cps1(m: &Machine) -> &machine::Cps1 {
        match m {
            Machine::Cps1(c) => c,
            Machine::Sf1(_) => unreachable!("wrapped from a_machine"),
        }
    }

    /// See [`wrap`], for the tests that change the CPU between two draws.
    fn as_cps1_mut(m: &mut Machine) -> &mut machine::Cps1 {
        match m {
            Machine::Cps1(c) => c,
            Machine::Sf1(_) => unreachable!("wrapped from a_machine"),
        }
    }

    fn a_machine() -> machine::Cps1 {
        let rom = prog();
        let mut m = machine::Cps1::new(&rom, BoardConfig::sf2(), Timing::cps1_10mhz());
        m.reset();
        m.cpu.d[0] = 0x1234_ABCD;
        m.cpu.d[7] = 0x0000_0001;
        m.cpu.a[0] = 0x00FF_1000;
        // A7 moved off the `ssp` shadow, as it is after any push. Load-bearing: a
        // mutant reading A7 from `ssp` instead of `a[7]` SURVIVED while this fixture
        // left them equal — `reset` sets both to 0x00FF8000, so the panel showed the
        // right number by coincidence and the assertion could not fail. The shadow is
        // stale exactly here, inside a handler, which is when you read it.
        m.cpu.a[7] = 0x00FF_7FF0;
        m
    }

    /// The register panel shows the registers, read back off the pixels.
    ///
    /// Not compared against the same `format!` the renderer used — that would assert
    /// the formatter equals itself. `read_text` recovers the characters from the
    /// buffer, so this proves the formatting, the layout, and the glyphs together.
    #[test]
    fn the_register_panel_shows_the_registers() {
        let mach = wrap(a_machine());
        let m = as_cps1(&mach);
        // A7 is taken from the fixture, not set here, because the fixture deliberately
        // leaves `a[7]` and `ssp` *different*. Setting it to `ssp`'s value would put
        // the coincidence back and this test's A7 assertion could no longer fail.
        assert_ne!(
            m.cpu.a[7], m.cpu.ssp,
            "the premise: the shadow is stale, so reading it would be visibly wrong"
        );
        let mut buf = frame();
        draw(
            &mut buf,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels {
                regs: true,
                ..Panels::none()
            },
            0,
            0,
            &[],
        );
        assert_eq!(
            read_text(&buf, REGS_X + PAD, REGS_Y + PAD, 23, FG),
            "D0 1234ABCD A0 00FF1000",
            "the first row, at the exact pixels the layout says"
        );
        // A7 is the row worth checking beyond D0: it shadows USP/SSP, so a panel that
        // read the wrong one would show a plausible number.
        assert!(
            panel_contains(&buf, "A7 00FF7FF0", FG),
            "the stack pointer must be the active one, from a[7]"
        );
        assert!(
            !panel_contains(&buf, "A7 00FF8000", FG),
            "and never the ssp shadow"
        );
        // And a register that is *not* the first one, so the loop is a loop.
        assert!(panel_contains(&buf, "D7 00000001", FG), "D7");
    }

    /// The PC shown is the executing instruction's, not `cpu.pc`.
    ///
    /// Separated from the panel's other rows because it is the one number here that
    /// is not a field read straight off the CPU, and the failure is silent: a panel
    /// showing 001004 for an instruction at 001000 looks entirely plausible.
    #[test]
    fn the_register_panel_shows_the_executing_pc_not_the_prefetched_one() {
        let mach = wrap(a_machine());
        let m = as_cps1(&mach);
        assert_eq!(
            m.cpu.pc, 0x1004,
            "the premise: PC is prefetched past 0x1000"
        );
        let mut buf = frame();
        draw(
            &mut buf,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels {
                regs: true,
                ..Panels::none()
            },
            0,
            0,
            &[],
        );
        assert!(panel_contains(&buf, "PC 00001000", HI), "the executing PC");
        assert!(
            !panel_contains(&buf, "PC 00001004", HI),
            "and not the prefetched one"
        );
    }

    /// The disassembly panel disassembles from the follow address and marks the PC.
    #[test]
    fn the_disassembly_panel_marks_the_executing_instruction() {
        let mach = wrap(a_machine());
        let m = as_cps1(&mach);
        let at = executing_pc(&mach.cpu_view());
        assert_eq!(at, m.cpu.pc - 4, "the premise of this whole panel");
        let mut buf = frame();
        draw(
            &mut buf,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels {
                disasm: true,
                ..Panels::none()
            },
            at,
            0,
            &[],
        );
        assert_eq!(
            read_text(&buf, DIS_X + PAD, DIS_Y + PAD, 25, HI),
            "> 001000 move #$2000,sr  ",
            "the first line: marked, addressed, and disassembled"
        );
        // The second line proves `insn.len` was used: a listing advancing by a fixed
        // 2 would show 001002, which is the middle of the first instruction.
        assert!(
            panel_contains(&buf, "001004 move.w d0,$FF0000", FG),
            "the next instruction is at 001004, not 001002"
        );
        // And the third, which follows a *three*-word instruction.
        assert!(
            panel_contains(&buf, "00100A addq.w #1,d0", FG),
            "then 00100A"
        );
    }

    /// Only the executing line is marked and highlighted.
    ///
    /// A panel that marked every line, or highlighted the whole listing, would tell
    /// you nothing about where the machine is.
    #[test]
    fn only_the_executing_line_is_marked() {
        let mach = wrap(a_machine());
        let m = as_cps1(&mach);
        let at = executing_pc(&mach.cpu_view());
        let mut buf = frame();
        draw(
            &mut buf,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels {
                disasm: true,
                ..Panels::none()
            },
            at,
            0,
            &[],
        );
        // Counted across *both* colours. Reading only `HI` cannot fail: a mutant that
        // marked every line still draws the non-PC lines in `FG`, so a `HI`-only scan
        // sees the one marker it expected and the assertion is vacuous. That mutant
        // SURVIVED until this loop looked at `FG` too.
        let markers = (0..DIS_ROWS)
            .filter(|row| {
                let y = DIS_Y + PAD + row * LINE;
                [HI, FG]
                    .iter()
                    .any(|&fg| read_text(&buf, DIS_X + PAD, y, 1, fg) == ">")
            })
            .count();
        assert_eq!(markers, 1, "exactly one line carries the marker");
        assert_eq!(
            read_text(&buf, DIS_X + PAD, DIS_Y + PAD + LINE, 1, FG),
            " ",
            "the second line's marker cell is blank"
        );
        assert_eq!(
            read_text(&buf, DIS_X + PAD, DIS_Y + PAD + LINE, 1, HI),
            " ",
            "and it is not highlighted either"
        );
    }

    /// The memory panel shows what `peek_word` returns, and `--` for what it does not.
    #[test]
    fn the_memory_panel_shows_words_and_gaps() {
        let mut mach = wrap(a_machine());
        let b = as_cps1_mut(&mut mach);
        b.board.ram[0] = 0xBEEF;
        b.board.ram[1] = 0xCAFE;
        let m = as_cps1(&mach);
        let mut buf = frame();
        draw(
            &mut buf,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels {
                mem: true,
                ..Panels::none()
            },
            0,
            0xFF_0000,
            &[],
        );
        assert_eq!(
            read_text(&buf, MEM_X + PAD, MEM_Y + PAD, 16, FG),
            "FF0000 BEEF CAFE",
            "the address and the first two words"
        );

        let mut buf = frame();
        draw(
            &mut buf,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels {
                mem: true,
                ..Panels::none()
            },
            0,
            0x40_0000,
            &[],
        );
        assert_eq!(
            read_text(&buf, MEM_X + PAD, MEM_Y + PAD, 16, FG),
            "400000   --   --",
            "an undecoded address is `--`, not FFFF: different facts"
        );
        assert!(
            !panel_contains(&buf, "FFFF", FG),
            "and must not read as all ones"
        );
    }

    /// A decoded address that genuinely reads all ones is shown as FFFF.
    ///
    /// The other half of the `--` claim, and the reason it is a real distinction
    /// rather than a stylistic one: $800020 is MAME's `nopr`, decoded and reading
    /// 0xFFFF. A panel rendering both as `--` loses the fact that a chip answered.
    #[test]
    fn a_decoded_all_ones_is_not_shown_as_a_gap() {
        let mach = wrap(a_machine());
        let m = as_cps1(&mach);
        let mut buf = frame();
        draw(
            &mut buf,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels {
                mem: true,
                ..Panels::none()
            },
            0,
            0x80_0020,
            &[],
        );
        assert!(
            panel_contains(&buf, "800020 FFFF", FG),
            "nopr reads all ones"
        );
    }

    /// The memory panel does not disturb the machine.
    ///
    /// `machine`'s own tests prove `peek_word` has no side effects. This proves the
    /// *panel* uses it: a dump pointed at the vector table must not acknowledge the
    /// interrupt it is there to explain, and one pointed at a gap must not fill the
    /// unmapped-read counter the status panel could show.
    #[test]
    fn drawing_the_memory_panel_does_not_disturb_the_machine() {
        let mut mach = wrap(a_machine());
        as_cps1_mut(&mut mach).board.assert_vblank();
        let m = as_cps1(&mach);
        assert!(
            m.board.vblank_pending(),
            "the premise: an interrupt is outstanding"
        );
        let unmapped = m.board.trace.unmapped_reads.total();
        let acks = m.board.trace.acks;
        let mut buf = frame();
        // 0x68 is in the first row of a dump starting at 0x60, and 0x400000 is a gap.
        for at in [0x60, 0x40_0000] {
            draw(
                &mut buf,
                &mach.cpu_view(),
                &|a| m.peek_word(a),
                &mach,
                Panels {
                    mem: true,
                    disasm: true,
                    ..Panels::none()
                },
                at,
                at,
                &[],
            );
        }
        assert!(
            m.board.vblank_pending(),
            "the interrupt is still outstanding"
        );
        assert_eq!(m.board.trace.acks, acks, "no acknowledge was invented");
        assert_eq!(
            m.board.trace.unmapped_reads.total(),
            unmapped,
            "and the panel's own reads are not in the trace"
        );
    }

    /// Drawing a panel does not touch a pixel outside it.
    ///
    /// The overlay covers the game where it is drawn and nowhere else. A panel
    /// writing one pixel past its box would corrupt the frame in a way that looks
    /// like a video bug.
    #[test]
    fn a_panel_leaves_the_rest_of_the_frame_alone() {
        let mach = wrap(a_machine());
        let m = as_cps1(&mach);
        let before = vec![0x00AB_CDEF_u32; WIDTH * HEIGHT];
        let mut after = before.clone();
        draw(
            &mut after,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels {
                regs: true,
                ..Panels::none()
            },
            0,
            0,
            &[],
        );
        let changed_rows = (0..HEIGHT)
            .filter(|&y| (0..WIDTH).any(|x| before[y * WIDTH + x] != after[y * WIDTH + x]))
            .count();
        assert!(changed_rows > 0, "the premise: the panel drew something");
        assert!(
            changed_rows < HEIGHT,
            "but not everywhere: {changed_rows} of {HEIGHT} rows"
        );
        // The box's own extent, exactly: one row above it and one column to its right
        // are untouched.
        let box_h = REGS_ROWS * LINE + 2 * PAD;
        let box_w = REGS_COLS * ADVANCE + 2 * PAD;
        for x in 0..WIDTH {
            assert_eq!(
                after[(REGS_Y - 1) * WIDTH + x],
                before[(REGS_Y - 1) * WIDTH + x],
                "the row above the box, column {x}"
            );
        }
        for y in 0..HEIGHT {
            assert_eq!(
                after[y * WIDTH + REGS_X + box_w],
                before[y * WIDTH + REGS_X + box_w],
                "the column right of the box, row {y}"
            );
        }
        let below = REGS_Y + box_h;
        for x in 0..WIDTH {
            assert_eq!(
                after[below * WIDTH + x],
                before[below * WIDTH + x],
                "the row below the box, column {x}"
            );
        }
    }

    /// Every panel fits inside the frame.
    ///
    /// `fill_rect` and `draw_text` clip, which is what stops a bad layout panicking —
    /// and is also what makes a bad layout silent. A box running off the bottom loses
    /// its last rows with nothing to show that it did, so the extents are asserted
    /// here rather than left to the eye.
    #[test]
    fn every_panel_fits_inside_the_frame() {
        for (name, x, y, cols, rows) in [
            ("regs", REGS_X, REGS_Y, REGS_COLS, REGS_ROWS),
            ("disasm", DIS_X, DIS_Y, DIS_COLS, DIS_ROWS),
            ("mem", MEM_X, MEM_Y, MEM_COLS, MEM_ROWS),
            ("status", STATUS_X, STATUS_Y, STATUS_COLS, 1),
            // The sound box's width and height are per-board and live in `sndpanel`;
            // only its origin is this module's. `SND_ROWS` is shared by both boards,
            // and SF1's wider box has its own extent assertion over there, where the
            // width it depends on is defined.
            (
                "sound",
                SND_X,
                SND_Y,
                crate::sndpanel::CPS1_COLS,
                crate::sndpanel::SND_ROWS,
            ),
        ] {
            let right = x + cols * ADVANCE + 2 * PAD;
            let bottom = y + rows * LINE + 2 * PAD;
            assert!(right <= WIDTH, "{name} is {right} wide, past {WIDTH}");
            assert!(bottom <= HEIGHT, "{name} ends at {bottom}, past {HEIGHT}");
        }
    }

    /// The panels do not overlap each other.
    ///
    /// Every panel can be on at once, and a layout that put two boxes on the same
    /// pixels would hide one behind the other — legible in a test that draws only one
    /// panel, and useless in the window.
    #[test]
    fn all_five_panels_can_be_shown_at_once_without_overlapping() {
        let mach = wrap(a_machine());
        let m = as_cps1(&mach);
        let all = Panels {
            regs: true,
            disasm: true,
            mem: true,
            status: true,
            sound: true,
        };
        // Each panel drawn alone, and the pixels it changed recorded. Two panels
        // claiming the same pixel is the failure.
        let blank = vec![0x00AB_CDEF_u32; WIDTH * HEIGHT];
        let mut claimed = vec![0u8; WIDTH * HEIGHT];
        for p in [
            Panels {
                regs: true,
                ..Panels::none()
            },
            Panels {
                disasm: true,
                ..Panels::none()
            },
            Panels {
                mem: true,
                ..Panels::none()
            },
            Panels {
                status: true,
                ..Panels::none()
            },
            Panels {
                sound: true,
                ..Panels::none()
            },
        ] {
            let mut buf = blank.clone();
            draw(
                &mut buf,
                &mach.cpu_view(),
                &|a| m.peek_word(a),
                &mach,
                p,
                0x1000,
                0xFF_0000,
                &[],
            );
            let mut touched = 0usize;
            for i in 0..buf.len() {
                if buf[i] != blank[i] {
                    claimed[i] += 1;
                    touched += 1;
                }
            }
            assert!(touched > 0, "every panel draws something: {p:?}");
        }
        assert!(
            claimed.iter().all(|&n| n <= 1),
            "{} pixels are claimed by two panels",
            claimed.iter().filter(|&&n| n > 1).count()
        );
        // And all five together really is all five: the same pixel count.
        let mut buf = blank.clone();
        draw(
            &mut buf,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            all,
            0x1000,
            0xFF_0000,
            &[],
        );
        let together = (0..buf.len()).filter(|&i| buf[i] != blank[i]).count();
        let separate = claimed.iter().filter(|&&n| n > 0).count();
        assert_eq!(together, separate, "all five drawn together cover all five");
        // `all` really is every flag: a field added to `Panels` and left out of the
        // literal above would make this whole test blind to it. The exhaustive
        // destructuring is what fails to compile if a sixth panel appears.
        let Panels {
            regs,
            disasm,
            mem,
            status,
            sound,
        } = all;
        assert!(
            regs && disasm && mem && status && sound,
            "every flag is set, so every panel was compared"
        );
    }

    /// No panels enabled draws nothing at all.
    ///
    /// The `F1`-off case, and worth a test: an overlay that always drew its
    /// background would black out the game when disabled.
    #[test]
    fn nothing_enabled_draws_nothing() {
        let mach = wrap(a_machine());
        let m = as_cps1(&mach);
        let before = vec![0x00AB_CDEF_u32; WIDTH * HEIGHT];
        let mut after = before.clone();
        draw(
            &mut after,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels::none(),
            0,
            0,
            &[],
        );
        assert_eq!(before, after);
        assert!(!Panels::none().any(), "and `any` agrees");
        assert!(Panels::on().any(), "while the default-on set does not");
    }

    /// A breakpoint is marked in the disassembly, in its own colour.
    #[test]
    fn a_breakpoint_is_marked() {
        let mach = wrap(a_machine());
        let m = as_cps1(&mach);
        let at = executing_pc(&mach.cpu_view());
        // On the *second* line, not the current one: a marker only ever drawn on the
        // executing line would be indistinguishable from the `>` marker.
        let brk = at + 4;
        let mut buf = frame();
        draw(
            &mut buf,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels {
                disasm: true,
                ..Panels::none()
            },
            at,
            0,
            &[brk],
        );
        assert_eq!(
            read_text(&buf, DIS_X + PAD + ADVANCE, DIS_Y + PAD + LINE, 1, BP),
            "*",
            "the breakpoint's line is marked, in the breakpoint colour"
        );
        assert_eq!(
            read_text(&buf, DIS_X + PAD + ADVANCE, DIS_Y + PAD, 1, BP),
            " ",
            "and a line without one is not"
        );
    }

    /// A halted CPU says so, and is not confused with a stopped one.
    ///
    /// `[CPU halted]` in the window title is where you find out today; the panel is
    /// where you find out what state it is in. HALT and STOP are different machines:
    /// a halted 68000 took a double bus fault and will never run again, a stopped one
    /// is waiting for an interrupt and will.
    #[test]
    fn a_halted_cpu_is_shown_as_halted_and_a_stopped_one_as_stopped() {
        let mut mach = wrap(a_machine());
        let p = Panels {
            status: true,
            ..Panels::none()
        };
        let mut buf = frame();
        {
            let m = as_cps1(&mach);
            draw(
                &mut buf,
                &mach.cpu_view(),
                &|a| m.peek_word(a),
                &mach,
                p,
                0,
                0,
                &[],
            );
        }
        assert!(panel_contains(&buf, "RUN", HI), "a running CPU says so");

        as_cps1_mut(&mut mach).cpu.stopped = true;
        let mut buf = frame();
        {
            let m = as_cps1(&mach);
            draw(
                &mut buf,
                &mach.cpu_view(),
                &|a| m.peek_word(a),
                &mach,
                p,
                0,
                0,
                &[],
            );
        }
        assert!(panel_contains(&buf, "STOP", HI), "stopped");
        assert!(!panel_contains(&buf, "HALT", HI), "and not halted");

        as_cps1_mut(&mut mach).cpu.halted = true;
        let mut buf = frame();
        {
            let m = as_cps1(&mach);
            draw(
                &mut buf,
                &mach.cpu_view(),
                &|a| m.peek_word(a),
                &mach,
                p,
                0,
                0,
                &[],
            );
        }
        assert!(panel_contains(&buf, "HALT", HI), "a dead CPU is not paused");
    }

    /// The status line shows the flags that are set and dashes for the rest.
    #[test]
    fn the_status_line_shows_the_flags_and_the_beam() {
        let mut mach = wrap(a_machine());
        // Z and C set, X/N/V clear, supervisor, IPL mask 7.
        as_cps1_mut(&mut mach).cpu.set_sr(0x2705);
        let mut buf = frame();
        {
            let m = as_cps1(&mach);
            draw(
                &mut buf,
                &mach.cpu_view(),
                &|a| m.peek_word(a),
                &mach,
                Panels {
                    status: true,
                    ..Panels::none()
                },
                0,
                0,
                &[],
            );
        }
        assert_eq!(
            read_text(&buf, STATUS_X + PAD, STATUS_Y + PAD, 24, HI),
            "RUN  --Z-C IPL7     LINE",
            "run state, flags, mask, and no interrupt outstanding"
        );
        assert!(panel_contains(&buf, "S1", HI), "supervisor");

        let b = as_cps1_mut(&mut mach);
        b.cpu.set_sr(0x0018);
        b.board.assert_vblank();
        let mut buf = frame();
        {
            let m = as_cps1(&mach);
            draw(
                &mut buf,
                &mach.cpu_view(),
                &|a| m.peek_word(a),
                &mach,
                Panels {
                    status: true,
                    ..Panels::none()
                },
                0,
                0,
                &[],
            );
        }
        assert_eq!(
            read_text(&buf, STATUS_X + PAD, STATUS_Y + PAD, 19, HI),
            "RUN  XN--- IPL0 IRQ",
            "the other flags, and an outstanding interrupt"
        );
        assert!(panel_contains(&buf, "S0", HI), "and user mode");
    }

    /// The sound panel renders for a machine in any state.
    ///
    /// Not a legibility test — that one is the user's, with a real ROM and `F1`. This is
    /// the crash test: a panel that indexes a register array or a disassembly window out
    /// of bounds takes the whole frontend down, and it would do it at whatever moment
    /// the user pressed the key rather than in CI.
    ///
    /// Both fixtures, because they exercise different code: the sound machine runs a
    /// real driver, and the ROM-less one spins on `RST 38h` with an empty region, where
    /// every `peek_byte` misses the ROM entirely. 97 instructions between draws is
    /// deliberately coprime with the driver's 3-instruction loop, so the panel is drawn
    /// at every point in it rather than always at the same one.
    #[test]
    fn the_sound_panel_renders_without_panicking() {
        for board in [a_sound_machine(), a_machine()] {
            let mut mach = wrap(board);
            let mut buf = frame();
            for _ in 0..4 {
                {
                    let m = as_cps1(&mach);
                    draw(
                        &mut buf,
                        &mach.cpu_view(),
                        &|a| m.peek_word(a),
                        &mach,
                        Panels {
                            sound: true,
                            ..Panels::none()
                        },
                        0,
                        0,
                        &[],
                    );
                }
                for _ in 0..97 {
                    as_cps1_mut(&mut mach).step_sound_instruction();
                }
            }
        }
    }

    /// A listing pointed at nothing renders `dc.w`, not a panic and not a blank.
    ///
    /// A debugger is most often opened *because* the PC has gone somewhere it should
    /// not be, so the case of a listing over undecoded space is the normal case, not
    /// an edge one.
    #[test]
    fn a_listing_over_undecoded_space_says_so() {
        let mach = wrap(a_machine());
        let m = as_cps1(&mach);
        let mut buf = frame();
        draw(
            &mut buf,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels {
                disasm: true,
                ..Panels::none()
            },
            0x40_0000,
            0,
            &[],
        );
        assert!(panel_contains(&buf, "400000 dc.w $FFFF", FG));
    }

    /// Drawing at the far end of the address space does not panic.
    ///
    /// `PageDown` held at the bottom of memory is how you get here, and a wrap is
    /// the right answer — the 68000's own address arithmetic wraps.
    #[test]
    fn addresses_at_the_end_of_the_space_wrap_rather_than_panic() {
        let mach = wrap(a_machine());
        let m = as_cps1(&mach);
        let mut buf = frame();
        draw(
            &mut buf,
            &mach.cpu_view(),
            &|a| m.peek_word(a),
            &mach,
            Panels {
                regs: true,
                disasm: true,
                mem: true,
                status: true,
                sound: true,
            },
            0xFFFF_FFFE,
            0xFFFF_FFF0,
            &[0xFFFF_FFFE],
        );
        assert!(panel_contains(&buf, "FFFFFFF0", FG), "the dump wrapped");
    }

    /// The executing PC is computed from the view, and it is four bytes behind.
    ///
    /// The whole reason `executing_pc` exists: a 68000's `pc` after a prefetching
    /// core's step points four bytes past the instruction that just ran, so a
    /// breakpoint or a listing keyed on `pc` is one or two instructions late. Moving
    /// to `CpuView` must not lose the subtraction.
    #[test]
    fn the_executing_pc_is_four_behind_the_views_pc() {
        let mut m = wrap(a_machine());
        m.step_instruction();
        let v = m.cpu_view();
        assert_eq!(
            executing_pc(&v),
            v.cpu.pc.wrapping_sub(4),
            "the same arithmetic, now off the view"
        );
        assert_ne!(executing_pc(&v), v.cpu.pc, "and it is not just the PC");
    }

    /// The memory panel reads through the closure it was given, and nothing else.
    ///
    /// ⚠️ This is the test that would catch `peek` being wired to the wrong board
    /// after Task 18 makes there be two. The closure counts its calls and answers a
    /// constant, so the panel's output is a function of the closure alone — if a
    /// future edit reached past it to a machine, the count would be wrong and the
    /// digits would not be 0xBEEF.
    #[test]
    fn the_memory_panel_reads_only_through_the_closure() {
        use core::cell::Cell;
        let mach = wrap(a_machine());
        let v = mach.cpu_view();
        let calls = Cell::new(0usize);
        let peek = |_addr: u32| -> Option<u16> {
            calls.set(calls.get() + 1);
            Some(0xBEEF)
        };
        let mut buf = frame();
        draw(
            &mut buf,
            &v,
            &peek,
            &mach,
            Panels {
                mem: true,
                ..Panels::none()
            },
            0,
            0x00FF_0000,
            &[],
        );
        assert_eq!(
            calls.get(),
            MEM_ROWS * MEM_WORDS,
            "one read per word shown, and no others"
        );
        // 0xBEEF's glyphs are on screen: the panel drew the closure's answer.
        assert!(
            panel_contains(&buf, "BEEF BEEF BEEF BEEF", FG),
            "the panel drew the closure's answer and nothing else"
        );
    }

    /// An undecoded word still prints as dashes, through the closure.
    ///
    /// `None` and `Some(0xFFFF)` are different facts and the panel prints them
    /// differently. The migration must not flatten them — a closure returning `None`
    /// has to reach the same `--` branch a board's `peek_word` did.
    #[test]
    fn a_none_from_the_closure_still_prints_dashes() {
        let mach = wrap(a_machine());
        let v = mach.cpu_view();
        let mut all_none = frame();
        draw(
            &mut all_none,
            &v,
            &|_| None,
            &mach,
            Panels {
                mem: true,
                ..Panels::none()
            },
            0,
            0,
            &[],
        );
        let mut all_ffff = frame();
        draw(
            &mut all_ffff,
            &v,
            &|_| Some(0xFFFF),
            &mach,
            Panels {
                mem: true,
                ..Panels::none()
            },
            0,
            0,
            &[],
        );
        assert_ne!(
            all_none, all_ffff,
            "`--` and `FFFF` are different pixels; the closure's None was not \
             flattened to a value"
        );
    }
}
