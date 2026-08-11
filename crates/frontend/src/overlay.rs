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
//! [`machine::cps1::Cps1::peek_word`], which takes `&self`, and `draw` takes `&Cps1`.
//! A debugger that could write would need to answer what a half-modified machine
//! means for a save state and for the 127/127 vector suite, and the answer is not
//! worth having.

use crate::font::{draw_text, fill_rect, ADVANCE, LINE};
use machine::video::{HEIGHT, WIDTH};
use machine::Cps1;

/// Panel background: dark, and opaque rather than blended.
///
/// Opaque because a blend would make the text's contrast depend on the game frame
/// underneath it — legible over a dark stage and not over a light one, which is a
/// bug that only appears on some screens.
const BG: u32 = 0x0000_0020;

/// Ordinary text.
const FG: u32 = 0x00D0_D0D0;

/// The executing instruction's line, and anything else the eye should go to first.
const HI: u32 = 0x0060_FF60;

/// A breakpoint marker.
const BP: u32 = 0x00FF_6060;

/// One pixel of padding inside a panel's box, so no glyph touches its edge.
const PAD: usize = 1;

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
pub const SND_Y: usize = DIS_Y;
/// `>8000 ld a,($f008)` at [`machine::z80::disasm::Text::CAP`] is 38 characters.
///
/// The cap, not a measured maximum: the widest text the Z80 disassembler can produce
/// is 32 characters, and a panel sized to the longest instruction *SF2's driver
/// happens to use* would clip on the first ROM that used a wider one.
const SND_COLS: usize = 38;
/// The fixed rows above the listing: registers, the interrupt state, the board, the
/// ADPCM chip, and the trace counters.
///
/// **Eleven is the ceiling here, not a round number.** The box starts at [`SND_Y`] and
/// its bottom is `SND_Y + SND_ROWS * LINE + 2 * PAD`, which at 11 header rows and
/// [`SND_DIS_ROWS`] listing rows is 211 — against [`STATUS_Y`]'s 214. A twelfth header
/// row would put the sound panel through the status line, so a new row has to take one
/// from the listing. `the_sound_panel_still_fits_below_its_last_row` is that arithmetic
/// as a test.
const SND_HEAD_ROWS: usize = 11;
/// How many Z80 instructions the sound listing shows.
///
/// Fewer than the 68000's eight: the Z80 is a supporting act here, and the rows are
/// what buys the header its space without pushing the box into the status line.
pub const SND_DIS_ROWS: usize = 6;
/// The whole box.
const SND_ROWS: usize = SND_HEAD_ROWS + SND_DIS_ROWS;

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
pub fn executing_pc(m: &Cps1) -> u32 {
    m.cpu.pc.wrapping_sub(4)
}

/// Draws the enabled panels into `buf`, over whatever the game rendered.
///
/// `disasm_at` is the address the listing starts at and `mem_at` the address the dump
/// starts at — both the caller's, so that `F4`'s "follow the PC" and `PageUp`'s
/// scrolling are decisions made in one place rather than remembered here. `bp` is the
/// breakpoint list, marked in the listing.
///
/// # Panics
///
/// If `buf` is not a `WIDTH × HEIGHT` frame, as [`draw_text`].
pub fn draw(buf: &mut [u32], m: &Cps1, p: Panels, disasm_at: u32, mem_at: u32, bp: &[u32]) {
    assert_eq!(buf.len(), WIDTH * HEIGHT, "not a frame");
    if p.regs {
        draw_regs(buf, m);
    }
    if p.disasm {
        draw_disasm(buf, m, disasm_at, bp);
    }
    if p.mem {
        draw_mem(buf, m, mem_at);
    }
    if p.sound {
        draw_sound(buf, m);
    }
    if p.status {
        draw_status(buf, m);
    }
}

/// A panel's background, and the `(x, y)` of its first line of text.
fn box_at(buf: &mut [u32], x: usize, y: usize, cols: usize, rows: usize) -> (usize, usize) {
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
fn draw_regs(buf: &mut [u32], m: &Cps1) {
    let (x, y) = box_at(buf, REGS_X, REGS_Y, REGS_COLS, REGS_ROWS);
    for i in 0..8 {
        // A7 comes from `a[7]`, never from `usp`/`ssp`: those are shadows written for
        // legibility and the active pointer is the one in `a[7]`. A panel reading the
        // shadow would show a plausible number that is wrong in exactly the situation
        // — inside an exception handler — where you are most likely reading it.
        let line = format!("D{i} {:08X} A{i} {:08X}", m.cpu.d[i], m.cpu.a[i]);
        draw_text(buf, x, y + i * LINE, &line, FG);
    }
    draw_text(
        buf,
        x,
        y + 8 * LINE,
        &format!("PC {:08X} SR {:04X}", executing_pc(m), m.cpu.sr),
        HI,
    );
    draw_text(
        buf,
        x,
        y + 9 * LINE,
        &format!("CYC {:012}", m.total_cycles),
        FG,
    );
    draw_text(
        buf,
        x,
        y + 10 * LINE,
        &format!("FRM {:010}", m.board.trace.frames),
        FG,
    );
}

/// The listing, marking the executing line and any breakpoint.
fn draw_disasm(buf: &mut [u32], m: &Cps1, at: u32, bp: &[u32]) {
    let (x, y) = box_at(buf, DIS_X, DIS_Y, DIS_COLS, DIS_ROWS);
    let pc = executing_pc(m);
    let mut a = at;
    for row in 0..DIS_ROWS {
        // `peek_word`, not the bus: a disassembly panel scrolled over $68 must not
        // acknowledge the interrupt it is there to explain. An undecoded address
        // reads as all ones, which the disassembler renders `dc.w $FFFF` — the
        // honest answer for a listing pointed at nothing.
        let insn = machine::m68k::disasm::disassemble(|w| m.peek_word(w).unwrap_or(0xFFFF), a);
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
fn draw_mem(buf: &mut [u32], m: &Cps1, at: u32) {
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
            match m.peek_word(a) {
                Some(v) => line.push_str(&format!(" {v:04X}")),
                None => line.push_str("   --"),
            }
        }
        draw_text(buf, x, y + row * LINE, &line, FG);
    }
}

/// The sound board: the Z80, its listing, the latches, and the chip.
///
/// # The Z80's PC is not the 68000's
///
/// There is no [`executing_pc`] here, and that is not an oversight. The 68000's `pc`
/// is four bytes past the instruction about to run because of the prefetch queue;
/// `z80::Z80::pc` is the address of the *next fetch*, so it already is the executing
/// instruction's. Subtracting anything would put the listing's marker in the middle of
/// whatever ran last.
///
/// # The listing always follows the PC
///
/// No scroll address. `Focus` has two variants because only two panels scroll, and a
/// sound listing you could park somewhere would need a third — for a 16-bit space you
/// can read in one screenful of pages. Following is what you want from a panel you
/// opened to watch a driver run.
fn draw_sound(buf: &mut [u32], m: &Cps1) {
    let (x, y) = box_at(buf, SND_X, SND_Y, SND_COLS, SND_ROWS);
    let z = &m.z80;
    let mut row = 0usize;
    let line = |buf: &mut [u32], row: &mut usize, s: &str, fg: u32| {
        draw_text(buf, x, y + *row * LINE, s, fg);
        *row += 1;
    };

    line(
        buf,
        &mut row,
        &format!(
            "AF {:04X} BC {:04X} DE {:04X} HL {:04X}",
            z.af(),
            z.bc(),
            z.de(),
            z.hl()
        ),
        FG,
    );
    line(
        buf,
        &mut row,
        &format!(
            "IX {:04X} IY {:04X} SP {:04X} PC {:04X}",
            z.ix, z.iy, z.sp, z.pc
        ),
        HI,
    );
    // The alternate set, which `exx` and `ex af,af'` swap in. A driver's interrupt
    // handler is where they are used, and a panel without them shows a Z80 whose
    // registers appear to change for no reason.
    line(
        buf,
        &mut row,
        &format!(
            "AF'{:04X} BC'{:04X} DE'{:04X} HL'{:04X}",
            z.af_, z.bc_, z.de_, z.hl_
        ),
        FG,
    );
    // `HALT` is a real state on this CPU and not a fault: the driver halts waiting for
    // the YM2151's timer interrupt, so a halted Z80 with `EI` set is normal and one
    // with `DI` set is hung forever. Both bits are shown for that reason.
    let run = if z.halted { "HALT" } else { "RUN " };
    line(
        buf,
        &mut row,
        &format!(
            "I {:02X} R {:02X} IM{} IFF{}{} {run}",
            z.i,
            z.r,
            z.im,
            u8::from(z.iff1),
            u8::from(z.iff2)
        ),
        HI,
    );

    // The 68000's side of the two latches beside the Z80's, because they are different
    // bytes: the board's are what the 68000 last wrote, and the board copies them into
    // the sound board's at the start of each Z80 instruction. Showing only one pair
    // hides a command in flight, which is the moment you opened this panel to see.
    line(
        buf,
        &mut row,
        &format!(
            "LATCH {:02X} {:02X} < {:02X} {:02X}  BANK {}  OKI7 {}",
            m.sound.latch(0),
            m.sound.latch(1),
            m.board.sound_latch[0],
            m.board.sound_latch[1],
            m.sound.bank(),
            u8::from(m.sound.oki_pin7()),
        ),
        FG,
    );

    let ym = m.sound.ym_ref();
    line(
        buf,
        &mut row,
        &format!(
            "YM REG {:02X} STAT {:02X} IRQ {}",
            m.sound.ym_addr(),
            ym.read_status(),
            if ym.irq() { "Y" } else { "-" }
        ),
        FG,
    );
    // One character per channel: its own number when any of its four operators is
    // keyed, a dot when none is. Read off `keyon_live` rather than register 0x08,
    // which is write-only and holds only the *last* write — a driver keying one
    // channel at a time would show one voice however many were sounding.
    let keys: String = ym
        .channels
        .iter()
        .enumerate()
        .map(|(c, ch)| {
            if ch.ops.iter().any(|op| op.keyon_live != 0) {
                char::from(b'0' + c as u8)
            } else {
                '.'
            }
        })
        .collect();
    line(buf, &mut row, &format!("KEYON {keys}"), HI);

    // The ADPCM chip. `STAT` is the byte the driver reads back from 0xF002 — bit 3..0 per
    // voice plus the idle bit — and `V` spells out which voices are sounding the way
    // `KEYON` does above, because the status byte alone is four bits you have to decode
    // in your head while a phrase is playing.
    //
    // `DIV` is the pin-7 divisor rather than the pin: the pin is one bit already on the
    // `LATCH` row, and the divisor is the number that tells you the sample rate — 132 or
    // 165, which at 1 MHz is 7.576 kHz or 6.061 kHz.
    let oki = m.sound.oki_ref();
    let playing: String = oki
        .voices()
        .iter()
        .enumerate()
        .map(|(i, v)| {
            if v.playing() {
                char::from(b'0' + i as u8)
            } else {
                '.'
            }
        })
        .collect();
    line(
        buf,
        &mut row,
        &format!(
            "OKI {:02X} V {playing} DIV {:3} CMD {}",
            oki.status(),
            m.sound.oki_divisor(),
            // The half-delivered start command, which is the state a phrase that never
            // plays is stuck in: a driver that latched a phrase and then took an
            // interrupt shows a number here that never clears.
            match oki.pending_command() {
                Some(p) => format!("{p:02X}"),
                None => "--".to_string(),
            }
        ),
        HI,
    );
    // The three counters that answer "why does it sound wrong", in the order the audio
    // path produces them: the chip clamped its own sum, the ring was full, the ring was
    // empty. `DRP` and `UND` come from the host through `set_audio_stats` and stay 0
    // until Task 12 wires the device — 0 is the honest reading for a machine with no
    // audio device, not a placeholder.
    line(
        buf,
        &mut row,
        &format!(
            "CLP {:06} DRP {:06} UND {:06}",
            m.sound_trace().oki_clamps,
            m.sound_trace().audio_drops,
            m.sound_trace().audio_underruns
        ),
        FG,
    );

    line(
        buf,
        &mut row,
        &format!("TSC {:012} SMP {:06}", m.z80_cycles(), m.samples().len()),
        FG,
    );
    let t = m.sound_trace();
    line(
        buf,
        &mut row,
        &format!(
            "FET {:09} YM {:06} LAT {:06}",
            t.audiocpu_fetches, t.ym_writes, t.latch_reads
        ),
        FG,
    );
    debug_assert_eq!(
        row, SND_HEAD_ROWS,
        "the header is what the box was sized for"
    );

    // The listing. `peek_byte`, not the bus: reading through `z80::Bus` would add this
    // panel's own reads to the `FET` count directly above it, so the number would be
    // mostly the panel. `disasm_bus` is unusable here for the same reason — it takes
    // `&mut B`, and this module holds `&Cps1`.
    let mut a = z.pc;
    for i in 0..SND_DIS_ROWS {
        let (text, len) = machine::z80::disasm::disasm(|w| m.sound.peek_byte(w), a);
        let marker = if a == z.pc { '>' } else { ' ' };
        let fg = if a == z.pc { HI } else { FG };
        draw_text(
            buf,
            x,
            y + (SND_HEAD_ROWS + i) * LINE,
            &format!("{marker}{a:04X} {}", text.as_str()),
            fg,
        );
        // `len` is at least 1, so the listing always advances and cannot loop.
        a = a.wrapping_add(len);
    }
}

/// Flags, the beam, and whether the CPU is running at all.
fn draw_status(buf: &mut [u32], m: &Cps1) {
    let (x, y) = box_at(buf, STATUS_X, STATUS_Y, STATUS_COLS, 1);
    let f = |bit: u16, c: char| if m.cpu.sr & bit != 0 { c } else { '-' };
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
    let run = if m.cpu.halted {
        "HALT"
    } else if m.cpu.stopped {
        "STOP"
    } else {
        "RUN "
    };
    let ipl = (m.cpu.sr >> 8) & 7;
    let irq = if m.board.vblank_pending() {
        "IRQ"
    } else {
        "   "
    };
    draw_text(
        buf,
        x,
        y,
        &format!(
            "{run} {flags} IPL{ipl} {irq} LINE {:03} S{}",
            m.line,
            u8::from(m.cpu.sr & 0x2000 != 0)
        ),
        HI,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::{frame, panel_contains, read_text};
    use machine::config::BoardConfig;
    use machine::timing::Timing;
    // The Z80's bus, for writing the board's ports the way a driver does rather than
    // reaching into the chips directly: `0xF002` is how a phrase is started, and a test
    // that called `Oki::write` would bypass the port decode the panel's rows describe.
    use machine::z80::Bus as _;

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

    fn a_machine() -> Cps1 {
        let rom = prog();
        let mut m = Cps1::new(&rom, BoardConfig::sf2(), Timing::cps1_10mhz());
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

    /// A machine with a sound program the Z80 actually executes.
    ///
    /// The driver is `machine`'s own `sound_spin` loop — read the command latch, store
    /// it to sound RAM, jump back — and it is padded to the full 0x18000 so both ROM
    /// banks decode. A machine built with `Cps1::new` has an *empty* sound region,
    /// where every fetch reads 0xFF: the Z80 spins on `RST 38h`, `keyon_live` is zero
    /// for all eight channels, and a listing shows six identical lines. That fixture
    /// cannot tell a working panel from one that reads the wrong fields, so the panel
    /// tests use this one.
    fn a_sound_machine() -> Cps1 {
        // `ld a,($f008)` / `ld ($d000),a` / `jr -9`, from `machine`'s `sound_spin`.
        let mut audiocpu = vec![0u8; 0x1_8000];
        audiocpu[..9].copy_from_slice(&[0x3A, 0x08, 0xF0, 0x32, 0x00, 0xD0, 0x00, 0x18, 0xF7]);
        let rom = prog();
        let mut m = Cps1::with_sound(
            &rom,
            Vec::new(),
            audiocpu,
            a_sample_rom(),
            BoardConfig::sf2(),
            Timing::cps1_10mhz(),
        );
        m.reset();
        m
    }

    /// A sample ROM with two phrases, so the panel's OKI rows have voices to show.
    ///
    /// A board built with `Vec::new()` refuses every phrase — `start == stop == 0` — so
    /// its status byte, its voice string, and its clamp count would all read as an idle
    /// chip whatever the panel did with them. `0x77` is the largest positive step
    /// repeated, which ramps to near full scale and makes the OKI clamp against its own
    /// ±65536 output limit once two voices are sounding: that is what gives `CLP`
    /// something to count.
    fn a_sample_rom() -> Vec<u8> {
        let mut r = vec![0u8; 0x8000];
        // Phrase headers at `phrase * 8`: start and last byte, 24-bit big-endian.
        r[8..14].copy_from_slice(&[0x00, 0x10, 0x00, 0x00, 0x30, 0x00]);
        r[16..22].copy_from_slice(&[0x00, 0x40, 0x00, 0x00, 0x60, 0x00]);
        r[0x1000..0x3001].fill(0x77);
        r[0x4000..0x6001].fill(0x77);
        r
    }

    /// The register panel shows the registers, read back off the pixels.
    ///
    /// Not compared against the same `format!` the renderer used — that would assert
    /// the formatter equals itself. `read_text` recovers the characters from the
    /// buffer, so this proves the formatting, the layout, and the glyphs together.
    #[test]
    fn the_register_panel_shows_the_registers() {
        let m = a_machine();
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
            &m,
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
        let m = a_machine();
        assert_eq!(
            m.cpu.pc, 0x1004,
            "the premise: PC is prefetched past 0x1000"
        );
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
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
        let m = a_machine();
        let at = executing_pc(&m);
        assert_eq!(at, m.cpu.pc - 4, "the premise of this whole panel");
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
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
        let m = a_machine();
        let at = executing_pc(&m);
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
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
        let mut m = a_machine();
        m.board.ram[0] = 0xBEEF;
        m.board.ram[1] = 0xCAFE;
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
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
            &m,
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
        let m = a_machine();
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
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
        let mut m = a_machine();
        m.board.assert_vblank();
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
                &m,
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
        let m = a_machine();
        let before = vec![0x00AB_CDEF_u32; WIDTH * HEIGHT];
        let mut after = before.clone();
        draw(
            &mut after,
            &m,
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
            ("sound", SND_X, SND_Y, SND_COLS, SND_ROWS),
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
        let m = a_machine();
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
            draw(&mut buf, &m, p, 0x1000, 0xFF_0000, &[]);
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
        draw(&mut buf, &m, all, 0x1000, 0xFF_0000, &[]);
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
        let m = a_machine();
        let before = vec![0x00AB_CDEF_u32; WIDTH * HEIGHT];
        let mut after = before.clone();
        draw(&mut after, &m, Panels::none(), 0, 0, &[]);
        assert_eq!(before, after);
        assert!(!Panels::none().any(), "and `any` agrees");
        assert!(Panels::on().any(), "while the default-on set does not");
    }

    /// A breakpoint is marked in the disassembly, in its own colour.
    #[test]
    fn a_breakpoint_is_marked() {
        let m = a_machine();
        let at = executing_pc(&m);
        // On the *second* line, not the current one: a marker only ever drawn on the
        // executing line would be indistinguishable from the `>` marker.
        let brk = at + 4;
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
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
        let mut m = a_machine();
        let p = Panels {
            status: true,
            ..Panels::none()
        };
        let mut buf = frame();
        draw(&mut buf, &m, p, 0, 0, &[]);
        assert!(panel_contains(&buf, "RUN", HI), "a running CPU says so");

        m.cpu.stopped = true;
        let mut buf = frame();
        draw(&mut buf, &m, p, 0, 0, &[]);
        assert!(panel_contains(&buf, "STOP", HI), "stopped");
        assert!(!panel_contains(&buf, "HALT", HI), "and not halted");

        m.cpu.halted = true;
        let mut buf = frame();
        draw(&mut buf, &m, p, 0, 0, &[]);
        assert!(panel_contains(&buf, "HALT", HI), "a dead CPU is not paused");
    }

    /// The status line shows the flags that are set and dashes for the rest.
    #[test]
    fn the_status_line_shows_the_flags_and_the_beam() {
        let mut m = a_machine();
        // Z and C set, X/N/V clear, supervisor, IPL mask 7.
        m.cpu.set_sr(0x2705);
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            Panels {
                status: true,
                ..Panels::none()
            },
            0,
            0,
            &[],
        );
        assert_eq!(
            read_text(&buf, STATUS_X + PAD, STATUS_Y + PAD, 24, HI),
            "RUN  --Z-C IPL7     LINE",
            "run state, flags, mask, and no interrupt outstanding"
        );
        assert!(panel_contains(&buf, "S1", HI), "supervisor");

        m.cpu.set_sr(0x0018);
        m.board.assert_vblank();
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            Panels {
                status: true,
                ..Panels::none()
            },
            0,
            0,
            &[],
        );
        assert_eq!(
            read_text(&buf, STATUS_X + PAD, STATUS_Y + PAD, 19, HI),
            "RUN  XN--- IPL0 IRQ",
            "the other flags, and an outstanding interrupt"
        );
        assert!(panel_contains(&buf, "S0", HI), "and user mode");
    }

    /// The sound panel shows the Z80, the board, and the chip.
    ///
    /// Read back off the pixels, like the 68000 panel's, for the same reason: comparing
    /// against the same `format!` the panel used would assert the formatter equals
    /// itself.
    #[test]
    fn the_sound_panel_shows_the_z80_the_latches_and_the_chip() {
        let mut m = a_sound_machine();
        // A command in flight: the 68000 has written the board's latches, and the Z80's
        // copy is a scanline behind until `step_sound` refreshes it.
        m.board.sound_latch = [0x5C, 0xA3];
        // A voice sounding, so `KEYON` has something to show. Channel 5, not 0: the
        // low three bits of 0x08 select the channel, and a panel keying the wrong one
        // would still light a voice.
        m.sound.ym().write(0x08, 0x78 | 5);
        m.run_scanline();

        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            Panels {
                sound: true,
                ..Panels::none()
            },
            0,
            0,
            &[],
        );

        // The PC row, at the exact pixels the layout says. SP is 0xFFFF and AF is all
        // ones after a Z80 reset — real hardware, not a placeholder, and asserted here
        // because a panel showing 0000 for SP would look like a plausible fresh
        // machine while actually reading the wrong field. The driver's PC is one of
        // 0x0000, 0x0003, or 0x0006 depending on where the line's budget ran out.
        let pcs = read_text(&buf, SND_X + PAD, SND_Y + PAD + LINE, 38, HI);
        assert!(
            pcs.starts_with("IX 0000 IY 0000 SP FFFF PC 000"),
            "the index, stack, and program registers: {pcs:?}"
        );
        // The alternate set is shown, and this driver never touches it — no `exx`, no
        // `ex af,af'` — so it is still the all-ones a Z80 reset leaves. The main `AF`
        // cannot be asserted here: `ld a,($f008)` has loaded the latch into A, and
        // whether it has run yet depends on where the scanline's budget ran out.
        assert_eq!(
            read_text(&buf, SND_X + PAD, SND_Y + PAD + 2 * LINE, 31, FG),
            "AF'0000 BC'0000 DE'0000 HL'0000",
            "the shadow set, which this driver never swaps in — and which a panel \
             printing the main set twice would show as the main set"
        );
        // The latches, both pairs. The Z80's copy is what `step_sound` refreshed, and
        // the board's is what the 68000 wrote — the same bytes here, because a
        // scanline ran, and the point is that both are shown.
        assert!(
            panel_contains(&buf, "LATCH 5C A3 < 5C A3", FG),
            "the Z80's latches and the 68000's, side by side"
        );
        assert!(panel_contains(&buf, "BANK 0", FG), "and the ROM bank");
        // The chip. Channel 5 keyed and nothing else: `.....5..`, which is the
        // assertion a panel reading register 0x08 instead of `keyon_live` fails once
        // the driver keys a second voice.
        assert!(
            panel_contains(&buf, "KEYON .....5..", HI),
            "one character per channel, and only channel 5 is keyed"
        );
        // The trace counters, from the machine rather than from a second tally.
        let t = m.sound_trace();
        assert!(t.audiocpu_fetches > 0, "the premise: the Z80 fetched");
        assert!(
            panel_contains(&buf, &format!("FET {:09}", t.audiocpu_fetches), FG),
            "the fetch count is the machine's"
        );
    }

    /// The sound panel shows the ADPCM chip: status, voices, divisor, and the counters.
    ///
    /// Read back off the pixels for the rest of the panel's reason, and with values
    /// chosen so each field can fail on its own:
    ///
    /// - **Voices 1 and 2, not 0.** The mask nibble is the high one, so a panel reading
    ///   `command & 0x0F` as the mask would light voice 0 instead. `.12.` also fails for a
    ///   panel that printed the status byte's bits in the wrong order.
    /// - **Pin 7 low, divisor 165.** The default is high, 132, so a panel that ignored the
    ///   pin — or printed the pin instead of the divisor — reads 132 or `1`.
    /// - **A phrase latched and left.** `CMD 03` is the half-delivered command; a panel
    ///   with no such row is how a phrase that never plays stays invisible.
    /// - **`CLP` non-zero.** Two voices at volume index 0 exceed the chip's own ±65536
    ///   clamp, which is what makes the counter a number rather than a permanent 0.
    #[test]
    fn the_sound_panel_shows_the_adpcm_chip() {
        let mut m = a_sound_machine();
        m.sound.write(0xF006, 0x00); // pin 7 low: the slower divisor
                                     // Two voices, both at volume index 0 so their sum clips.
        m.sound.write(0xF002, 0x81);
        m.sound.write(0xF002, 0x20); // mask 2 -> voice 1
        m.sound.write(0xF002, 0x82);
        m.sound.write(0xF002, 0x40); // mask 4 -> voice 2
        for _ in 0..64 {
            m.run_scanline();
        }
        // A third phrase latched with no mask byte, last so it stays pending.
        m.sound.write(0xF002, 0x83);

        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            Panels {
                sound: true,
                ..Panels::none()
            },
            0,
            0,
            &[],
        );

        // The status byte the driver itself would read. `0xF6`, not `0x06`: the chip
        // builds `0xF0` and sets one bit per playing voice, so the high nibble is always
        // set — the panel shows the byte the guest sees, not a cleaned-up version of it.
        assert_eq!(
            m.sound.oki_ref().status(),
            0xF6,
            "the premise: the chip reports voices 1 and 2"
        );
        assert!(
            panel_contains(&buf, "OKI F6 V .12. DIV 165 CMD 03", HI),
            "the chip's status, its voices, its rate, and the pending command"
        );

        let t = m.sound_trace();
        assert!(
            t.oki_clamps > 0,
            "the premise: two loud voices clipped, so CLP is a real count ({})",
            t.oki_clamps
        );
        assert!(
            panel_contains(&buf, &format!("CLP {:06}", t.oki_clamps), FG),
            "the clamp count is the machine's, not a second tally"
        );
        // Both host counters are 0 until Task 12 wires a device, and shown anyway: the
        // panel's job is to say "the ring is fine", which a missing row cannot.
        assert!(
            panel_contains(&buf, "DRP 000000 UND 000000", FG),
            "the ring's counters, zero with no audio device attached"
        );
    }

    /// The sound panel still ends above the status line, with no room for a twelfth row.
    ///
    /// `every_panel_fits_inside_the_frame` only checks the frame's edge, and the status
    /// line is not the frame's edge — a sound panel two rows taller would fit the window
    /// and draw straight through the status line's box, which `fill_rect` would happily
    /// do. This is the arithmetic in [`SND_HEAD_ROWS`]'s documentation as an assertion,
    /// including the part that says a new header row has to take one from the listing.
    #[test]
    fn the_sound_panel_still_fits_below_its_last_row() {
        let bottom = SND_Y + SND_ROWS * LINE + 2 * PAD;
        assert_eq!(bottom, 211, "the sound box ends here");
        assert!(
            bottom < STATUS_Y,
            "the sound panel ends at {bottom}, at or past the status line's {STATUS_Y}"
        );
        let with_one_more = bottom + LINE;
        assert!(
            with_one_more > STATUS_Y,
            "and there is no room for a twelfth header row ({with_one_more} against \
             {STATUS_Y}), so the next row added has to come out of SND_DIS_ROWS — this \
             is the assertion that says so rather than leaving it to a comment"
        );
    }

    /// The sound listing follows the Z80's PC and marks it, with no prefetch offset.
    ///
    /// **The Z80's `pc` is already the executing instruction's**, unlike the 68000's.
    /// A panel that borrowed [`executing_pc`]'s `- 4` here would point four bytes
    /// behind, into the middle of whatever ran last — and the driver's three
    /// instructions are 3, 3, and 2 bytes, so a fixed offset lands mid-instruction and
    /// disassembles to something that was never executed.
    #[test]
    fn the_sound_listing_starts_at_the_z80s_pc_and_marks_it() {
        let m = a_sound_machine();
        assert_eq!(m.z80.pc, 0, "the premise: a freshly reset Z80 is at 0x0000");
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            Panels {
                sound: true,
                ..Panels::none()
            },
            0,
            0,
            &[],
        );
        let head = SND_Y + PAD + SND_HEAD_ROWS * LINE;
        assert_eq!(
            read_text(&buf, SND_X + PAD, head, 20, HI),
            ">0000 ld a,($f008)  ",
            "the first line: marked, addressed, and disassembled"
        );
        // The second line proves the instruction *length* was used: a listing advancing
        // by a fixed 1 or 2 would show 0001 or 0002, both mid-instruction.
        assert!(
            panel_contains(&buf, "0003 ld ($d000),a", FG),
            "the next instruction is at 0003, not 0001"
        );
        // And exactly one line is marked, counted across both colours — reading only
        // `HI` cannot fail, because a panel marking every line still draws the others
        // in `FG`.
        let markers = (0..SND_DIS_ROWS)
            .filter(|row| {
                let y = head + row * LINE;
                [HI, FG]
                    .iter()
                    .any(|&fg| read_text(&buf, SND_X + PAD, y, 1, fg) == ">")
            })
            .count();
        assert_eq!(markers, 1, "exactly one line carries the marker");
    }

    /// Drawing the sound panel does not move the trace counters it displays.
    ///
    /// **The claim that makes the numbers mean anything.** A listing read through
    /// `z80::Bus` would add six instructions' worth of fetches per frame — 360 a second
    /// — to the count printed one row above it, so `FET` would be mostly the panel and
    /// `tests/sound_boot.rs`'s `audiocpu_fetches > 100_000` would be satisfiable by a
    /// machine that never ran the driver at all.
    #[test]
    fn drawing_the_sound_panel_does_not_move_the_counters() {
        let mut m = a_sound_machine();
        m.run_scanline();
        let before = m.sound_trace();
        assert!(before.audiocpu_fetches > 0, "the premise: there is a count");
        let mut buf = frame();
        for _ in 0..8 {
            draw(
                &mut buf,
                &m,
                Panels {
                    sound: true,
                    ..Panels::none()
                },
                0,
                0,
                &[],
            );
        }
        assert_eq!(
            m.sound_trace(),
            before,
            "eight frames of panel added nothing to the counters"
        );
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
        for mut m in [a_sound_machine(), a_machine()] {
            let mut buf = frame();
            for _ in 0..4 {
                draw(
                    &mut buf,
                    &m,
                    Panels {
                        sound: true,
                        ..Panels::none()
                    },
                    0,
                    0,
                    &[],
                );
                for _ in 0..97 {
                    m.step_sound_instruction();
                }
            }
        }
    }

    /// A sound listing that runs off the top of the address space wraps.
    ///
    /// A Z80 PC near 0xFFFF is reachable — a driver bug, or `RST 38h` in unmapped
    /// space walking upward — and the listing's six rows then cross the wrap. Debug
    /// builds panic on overflow, so this is the difference between a debugger and a
    /// crash at the moment you most need one.
    #[test]
    fn a_sound_listing_at_the_top_of_the_space_wraps() {
        let mut m = a_sound_machine();
        m.z80.pc = 0xFFFD;
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
            Panels {
                sound: true,
                ..Panels::none()
            },
            0,
            0,
            &[],
        );
        assert!(
            panel_contains(&buf, "FFFD", HI),
            "the listing starts where the PC is"
        );
    }

    /// A listing pointed at nothing renders `dc.w`, not a panic and not a blank.
    ///
    /// A debugger is most often opened *because* the PC has gone somewhere it should
    /// not be, so the case of a listing over undecoded space is the normal case, not
    /// an edge one.
    #[test]
    fn a_listing_over_undecoded_space_says_so() {
        let m = a_machine();
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
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
        let m = a_machine();
        let mut buf = frame();
        draw(
            &mut buf,
            &m,
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
}
