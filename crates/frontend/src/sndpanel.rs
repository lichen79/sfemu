//! The sound panel, which is the one panel that genuinely forks per board.
//!
//! # Why this is not one function
//!
//! CPS-1 has one Z80, one YM2151 and an OKI MSM6295 with a pin-7 divisor and a
//! pending-command register. SF1 has **two** Z80s on one crystal, one YM2151, two
//! MSM5205s with a signal, a step index and a reset pin each, and a 256-entry ROM
//! bank register on the second CPU's bus. Not one field on the two boards has both
//! the same name and the same meaning, so the content functions share nothing.
//!
//! What they share is the chrome — `box_at`, the `line` closure, the row and column
//! constants — and that is why both live in one file. `gfxpanels.rs` shows what the
//! alternative costs: it holds its own `FG`, `HI` and `PAD`, and its `PAD` is 2 where
//! `overlay.rs`'s is 1. A third copy would be the one that silently misaligns.
//!
//! # The row budget, measured
//!
//! [`crate::overlay::SND_Y`] is 90, `LINE` is 7, `PAD` is 1, and
//! [`crate::overlay::STATUS_Y`] is 214, so
//! `SND_Y + rows * LINE + 2 * PAD` is 211 at 17 rows and **218 at 18** — through the
//! status line. **17 is the ceiling and both boards spend all of it.** Adding a row
//! to either panel means taking one away, and
//! `seventeen_rows_is_the_ceiling_and_both_boards_use_it` is that fact as a test.
//!
//! SF1 pays for its second CPU out of the listing: 15 header rows and **2**
//! instructions against CPS-1's 11 and 6, which is enough to see the executing
//! instruction and the one after. It also drops the FM Z80's alternate-register row
//! that CPS-1 shows. Both losses are recorded here rather than silently taken.
//!
//! # The two box widths, and why every field is fixed-width
//!
//! CPS-1's box is 38 columns and SF1's is **45**, at the screen's right edge (156 +
//! 45 × 5 + 2 = 383 of 384).
//!
//! ⚠️ **Every counter on the SF1 panel is formatted to a fixed width** — `{:010}` for
//! a `u32`, `{:013}` for a `u64` — so no row's length depends on its value. This is
//! not decoration. `{:06}` is a *minimum* width, not a maximum: the first draft of
//! this panel used it, every row fit inside the box on a fresh machine and in every
//! test, and six of the fifteen rows overflowed once a counter passed 999,999 and
//! started printing ten digits. A fixed-width panel can be measured once; a
//! minimum-width one has to be measured at every value it can hold.
//!
//! At those widths the widest data row is `DSC` / `OVR` / `UNM` at a constant 44
//! characters, which is what 45 columns is for. `draw_text` clips rather than
//! panicking (`font.rs:218`), so an overflow would be a silently truncated number
//! rather than a crash — and, in a box not at the screen edge, ink printed over
//! whatever is to its right.
//!
//! ## The measured widths, per column
//!
//! Two of SF1's non-counter columns were measured wrong the first time, and neither
//! would have been caught by saturating the counters:
//!
//! | column | format | why |
//! |---|---|---|
//! | `OUT` | `{:+06}` | `Msm5205::output` is the *scaled* signal, `-16_384..=16_352` |
//! | `S` | `{:+05}` | `Msm5205::signal` is the raw 12-bit accumulator, `-2048..=2047` |
//! | `@` | `{:05X}` | `Adpcm2Board::bank_base` is masked into a 0x40000 region |
//! | any `u32` counter | `{:010}` | `u32::MAX` is ten digits |
//! | either T-state total | `{:013}` | twelve digits run out after 77.6 emulated hours |
//!
//! (`box_at`, `FG`, `HI` and `PAD` are plain code spans rather than links: they are
//! `pub(crate)` in `overlay.rs`, and `#![deny(rustdoc::private_intra_doc_links)]`
//! makes a link to one from this `pub` module a doc-build failure.)

use crate::font::{draw_text, LINE};
use crate::overlay::{box_at, FG, HI, SND_X, SND_Y};
use machine::{Machine, Sf1};

/// CPS-1's box width. The cap, not a measured maximum — see `overlay.rs`'s history:
/// the widest text the Z80 disassembler can produce is 32 characters.
pub const CPS1_COLS: usize = 38;
/// SF1's box width: one column wider than its widest *data* row (`DSC`/`OVR`/`UNM`,
/// a constant 44 characters) and one pixel short of the screen edge. The rule row is
/// generated at exactly this width. See this module's documentation.
pub const SF1_COLS: usize = 45;
/// CPS-1's fixed rows above the listing.
pub const CPS1_HEAD_ROWS: usize = 11;
/// CPS-1's listing rows.
pub const CPS1_DIS_ROWS: usize = 6;
/// SF1's fixed rows: five for the FM CPU and its YM, a rule, three for the ADPCM CPU
/// and its bank, two for the MSM5205s, and four counter rows.
pub const SF1_HEAD_ROWS: usize = 15;
/// SF1's listing rows. ⚠️ Two, not six — see this module's row budget.
pub const SF1_DIS_ROWS: usize = 2;
/// The box height, the same on both boards because 17 is the ceiling.
pub const SND_ROWS: usize = CPS1_HEAD_ROWS + CPS1_DIS_ROWS;

/// Draws whichever board's sound panel.
///
/// Matches on [`Machine`] rather than taking an `as_sf1() -> Option<&Sf1>`: an
/// accessor would let this function compile with one arm doing nothing, which is a
/// panel that goes blank on one board with no error. `the_two_panels_are_different`
/// is the test for that; this signature is what makes it hard to get wrong.
///
/// # Panics
///
/// If `buf` is not a `WIDTH × HEIGHT` frame, as [`crate::font::draw_text`].
pub fn draw(buf: &mut [u32], m: &Machine) {
    match m {
        Machine::Cps1(c) => draw_cps1(buf, c),
        Machine::Sf1(s) => draw_sf1(buf, s),
    }
}

/// The sound board: the Z80, its listing, the latches, and the chip.
///
/// ⚠️ **Moved verbatim from `overlay.rs`'s `draw_sound`.** Not rewritten: every row,
/// format string, colour and comment came across unchanged, and only
/// `SND_COLS`/`SND_HEAD_ROWS`/`SND_DIS_ROWS` were renamed to their `CPS1_` forms, at
/// the same values. The panel's existing tests assert its pixels.
///
/// # The Z80's PC is not the 68000's
///
/// There is no [`crate::overlay::executing_pc`] here, and that is not an oversight.
/// The 68000's `pc` is four bytes past the instruction about to run because of the
/// prefetch queue; `z80::Z80::pc` is the address of the *next fetch*, so it already is
/// the executing instruction's. Subtracting anything would put the listing's marker in
/// the middle of whatever ran last.
///
/// # The listing always follows the PC
///
/// No scroll address. `Focus` has two variants because only two panels scroll, and a
/// sound listing you could park somewhere would need a third — for a 16-bit space you
/// can read in one screenful of pages. Following is what you want from a panel you
/// opened to watch a driver run.
fn draw_cps1(buf: &mut [u32], m: &machine::Cps1) {
    let (x, y) = box_at(buf, SND_X, SND_Y, CPS1_COLS, SND_ROWS);
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
    // empty. `DRP` and `UND` come from the host through `set_audio_stats`, which the
    // frontend loop calls once a tick from the device's ring; they read 0 on a machine
    // with no audio device, which is the honest reading rather than a placeholder.
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
        &format!(
            "TSC {:012} SMP {:06}",
            m.z80_cycles(),
            m.samples().len() / machine::resample::CHANNELS
        ),
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
        row, CPS1_HEAD_ROWS,
        "the header is what the box was sized for"
    );

    // The listing. `peek_byte`, not the bus: reading through `z80::Bus` would add this
    // panel's own reads to the `FET` count directly above it, so the number would be
    // mostly the panel. `disasm_bus` is unusable here for the same reason — it takes
    // `&mut B`, and this module holds `&Cps1`.
    let mut a = z.pc;
    for i in 0..CPS1_DIS_ROWS {
        let (text, len) = machine::z80::disasm::disasm(|w| m.sound.peek_byte(w), a);
        let marker = if a == z.pc { '>' } else { ' ' };
        let fg = if a == z.pc { HI } else { FG };
        draw_text(
            buf,
            x,
            y + (CPS1_HEAD_ROWS + i) * LINE,
            &format!("{marker}{a:04X} {}", text.as_str()),
            fg,
        );
        // `len` is at least 1, so the listing always advances and cannot loop.
        a = a.wrapping_add(len);
    }
}

/// SF1's panel: two Z80s, one YM, two MSM5205s and a bank.
fn draw_sf1(buf: &mut [u32], m: &Sf1) {
    let (x, y) = box_at(buf, SND_X, SND_Y, SF1_COLS, SND_ROWS);
    let mut row = 0usize;
    // Not `let mut line` — the closure mutates only through its `&mut` parameters, and
    // `unused_mut` is an error here.
    let line = |buf: &mut [u32], row: &mut usize, s: &str, fg: u32| {
        draw_text(buf, x, y + *row * LINE, s, fg);
        *row += 1;
    };

    // ---- The FM Z80, which is where a music problem is diagnosed. ----
    let z = &m.fm_z80;
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
    // `HALT` with `EI` is normal, as on CPS-1: the driver halts waiting for the YM's
    // timer. `NMI` is how many sound commands the 68000 has sent, which is the number
    // that says whether the game is asking for music at all — and it has no other home,
    // because a taken NMI leaves no trace on the CPU once its handler returns.
    line(
        buf,
        &mut row,
        &format!(
            "I {:02X} R {:02X} IM{} IFF{}{} {} NMI {:010}",
            z.i,
            z.r,
            z.im,
            u8::from(z.iff1),
            u8::from(z.iff2),
            if z.halted { "HALT" } else { "RUN " },
            m.fm_nmis_raised()
        ),
        HI,
    );
    // One latch, not CPS-1's two: `soundcmd_w` (`sf.cpp:118-122`) writes a single byte,
    // so there is no second half in flight to show.
    let ym = m.fm.ym_ref();
    line(
        buf,
        &mut row,
        &format!(
            "LATCH {:02X}  YM REG {:02X} ST {:02X} IRQ {}",
            m.fm.latch(),
            m.fm.ym_addr(),
            ym.read_status(),
            if ym.irq() { "Y" } else { "-" }
        ),
        FG,
    );
    // Read off `keyon_live` rather than register 0x08, which is write-only and holds
    // only the last write — a driver keying one channel at a time would show one voice
    // however many were sounding. The same code and the same reason as CPS-1's.
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
    // `UND` rides along on the key-on row, which at 14 columns is the panel's widest
    // slack; its partner `DRP` is on row 14. See the ⚠️ above row 12 for why the pair is
    // split across two rows rather than sharing one.
    line(
        buf,
        &mut row,
        &format!("KEYON {keys} UND {:010}", m.audio_underruns()),
        HI,
    );

    // ---- A rule, then the second CPU. ----
    //
    // ⚠️ A whole row spent on a separator, on a panel at its row ceiling. It earns it:
    // the two register blocks are identically formatted, so a reader glancing at the
    // panel while a voice is missing must not read the FM CPU's PC as the ADPCM CPU's.
    // Without the rule the two blocks are indistinguishable.
    //
    // Generated to span the box rather than written as a literal, so it cannot be the
    // row that overflows when someone changes `SF1_COLS`.
    line(
        buf,
        &mut row,
        &format!("---- ADPCM Z80 {}", "-".repeat(SF1_COLS - 15)),
        FG,
    );

    let a = &m.adpcm_z80;
    // Four registers, not eight. This CPU reads a latch, picks a phrase and writes
    // nibbles to two chips; DE and HL are where it does that, but AF/BC/SP/PC are what
    // say whether it is doing it at all, and there is one row.
    line(
        buf,
        &mut row,
        &format!(
            "AF {:04X} BC {:04X} SP {:04X} PC {:04X}",
            a.af(),
            a.bc(),
            a.sp,
            a.pc
        ),
        FG,
    );
    // ⚠️ `IRQ` here is the **8 kHz periodic**, and it is the most diagnostic number on
    // this half of the panel: `set_periodic_int(irq0_line_hold, from_hz(8000))`
    // (`sf.cpp:763`) paces every sample the board plays, so a count not climbing at
    // 8,000 a second is ADPCM at the wrong rate or none at all. This CPU takes no NMI
    // and no YM IRQ, which is why there is no NMI column here and there is one above.
    line(
        buf,
        &mut row,
        &format!(
            "I {:02X} IM{} IFF{}{} {} IRQ {:010}",
            a.i,
            a.im,
            u8::from(a.iff1),
            u8::from(a.iff2),
            if a.halted { "HALT" } else { "RUN " },
            m.adpcm_irqs_raised()
        ),
        HI,
    );
    // The bank, **resolved to the byte offset it actually reads**. `set_entry(data)`
    // takes a full byte against a `REGION_BYTES` (0x40000) region of `BANK_BYTES`
    // (0x8000) windows, so entries alias with period 8 and a register above 7 is a
    // driver bug the hardware would have answered with garbage — `OVR`, two rows down,
    // is how that becomes visible. `{:3}` on the entry so the `@` does not shuffle
    // sideways as the bank changes, and `{:05X}` because `bank_base()` is masked into
    // the region and so cannot exceed 0x3FFFF — five hex digits, not six.
    line(
        buf,
        &mut row,
        &format!(
            "LATCH {:02X} BANK {:3} @ {:05X}",
            m.adpcm.latch(),
            m.adpcm.bank(),
            m.adpcm.bank_base()
        ),
        FG,
    );

    // ---- The two chips, one row each. ----
    //
    // `S` is the 12-bit accumulator, signed, which says a phrase is progressing; `ST`
    // is the step index, which says it is progressing *coherently* — a step stuck at 48
    // is a decoder out of headroom, which sounds like clipping. `D` is the latched
    // nibble and `RST` the reset pin, where `*` is held in reset: a chip in reset with a
    // nonzero latch is a driver that queued a phrase and never released the pin, which
    // is silence with everything else looking right.
    //
    // ⚠️ `OUT` is `{:+06}`, one wider than `S`'s `{:+05}`. `output()` is the *scaled*
    // signal — `(signal & !DAC_MASK) * DAC_TO_I16`, so `-2048..=2044` times 8, which
    // is `-16_384..=16_352` and five digits with the sign. `{:+05}` would print six
    // characters anyway, one past where the column was measured.
    for chip in 0..2 {
        let c = m.adpcm.msm(chip);
        line(
            buf,
            &mut row,
            &format!(
                "M{chip} S {:+05} ST {:02} D {:02X} RST{} OUT {:+06}",
                c.signal(),
                c.step(),
                c.data(),
                if c.in_reset() { '*' } else { '-' },
                c.output()
            ),
            FG,
        );
    }

    // ---- Four counter rows. ----
    //
    // ⚠️ Every counter is `{:010}` for a `u32` and `{:013}` for a `u64`, so a row's
    // width never depends on its value. `{:06}` is a *minimum* width: the first draft of
    // this panel used it, fit inside the box at zero, and overflowed after nineteen
    // minutes of play. Four rows rather than three is what the fixed widths cost, and
    // the listing pays for it.
    //
    // `CLP` is the mix saturating, which on this board is the only place saturation can
    // happen: two MSM5205s at their rails sum to exactly `i16::MIN`, in range, so it
    // takes the YM's share on top to clip. `MSM` is the per-chip write counts.
    let at = m.adpcm.trace();
    line(
        buf,
        &mut row,
        &format!(
            "CLP {:010} MSM {:010}/{:010}",
            m.mix_clips(),
            at.msm_writes[0],
            at.msm_writes[1]
        ),
        FG,
    );
    // The three "the driver is doing something the board cannot honour" counters.
    // `DSC` is `writes_discarded`, which exists because Z80 #2 has **no RAM at all**
    // (`sf.cpp:217-223`, MAME's own comment is `/* Yes, _no_ ram */`) — a driver that
    // assumed a scratchpad shows a climbing number here and nothing else wrong. `OVR`
    // is a bank entry past the region. `UNM` is an unmapped port.
    //
    // ⚠️ **`DRP`/`UND` are not on this row, unlike CPS-1's, and they are not on the same
    // row as each other.** Those two are the *host's* ring statistics, arriving through
    // `Sf1::set_audio_stats`. They are split because this row is already 44 of the box's
    // 45 columns: a fourth field does not fit, an eighteenth panel row would land at 218
    // against a status line at 214, and a 46-column box is 388 pixels of a 384-pixel
    // window. So `UND` goes on row 4 (`KEYON`, 14 columns → 29) and `DRP` on row 14
    // (`FET`/`YM`, 28 columns → 43).
    line(
        buf,
        &mut row,
        &format!(
            "DSC {:010} OVR {:010} UNM {:010}",
            at.writes_discarded, at.bank_overruns, at.unmapped_ports
        ),
        FG,
    );
    // Both CPUs' T-states. The whole point of two accumulators is that they can
    // diverge, and a pair not advancing together is a scheduler bug.
    //
    // ⚠️ `{:013}`, not `{:012}`: twelve digits overflow after 77.6 hours of emulated
    // Z80 time, which a save state carried across sessions reaches. Thirteen lasts 776.
    line(
        buf,
        &mut row,
        &format!("TSC {:013}/{:013}", m.z80_cycles(), m.adpcm_z80_cycles()),
        FG,
    );
    let ft = m.fm.trace();
    line(
        buf,
        &mut row,
        &format!(
            "FET {:010} YM {:010} DRP {:010}",
            ft.audiocpu_fetches,
            ft.ym_writes,
            m.audio_drops()
        ),
        FG,
    );

    debug_assert_eq!(
        row, SF1_HEAD_ROWS,
        "the header is what the box was sized for"
    );

    // The listing, two rows, of the **FM** CPU. One of the two had to be chosen and this
    // is the one whose code you read: it has the RAM, it services the YM, and it is what
    // a music problem is traced through. The ADPCM CPU's PC is on its own row above,
    // which is enough to see it advancing.
    //
    // `peek_byte`, not the bus: reading through `z80::Bus` would add this panel's own
    // reads to the `FET` count two rows above, and
    // `drawing_the_sf1_panel_does_not_move_the_counters` is what holds that.
    let mut pc = z.pc;
    for i in 0..SF1_DIS_ROWS {
        let (text, len) = machine::z80::disasm::disasm(|w| m.fm.peek_byte(w), pc);
        let marker = if pc == z.pc { '>' } else { ' ' };
        let fg = if pc == z.pc { HI } else { FG };
        draw_text(
            buf,
            x,
            y + (SF1_HEAD_ROWS + i) * LINE,
            &format!("{marker}{pc:04X} {}", text.as_str()),
            fg,
        );
        // `len` is at least 1, so the listing always advances and cannot loop.
        pc = pc.wrapping_add(len);
    }
}

/// A 68000 spin program, for both boards.
///
/// `overlay.rs`'s copy is `#[cfg(test)]` in a sibling module and so out of reach — and
/// these tests draw a *sound* panel, which reads none of it: all it has to do is give
/// the 68000 a stack pointer and a reset vector that lands on a branch to itself.
#[cfg(test)]
fn prog() -> Vec<u8> {
    let mut rom = vec![0u8; 0x0800];
    rom[0..4].copy_from_slice(&0x00FF_F000u32.to_be_bytes());
    rom[4..8].copy_from_slice(&0x0000_0400u32.to_be_bytes());
    rom[0x400..0x402].copy_from_slice(&0x60FEu16.to_be_bytes());
    rom
}

/// A machine with a sound program the Z80 actually executes.
///
/// The driver is `machine`'s own `sound_spin` loop — read the command latch, store it
/// to sound RAM, jump back — and it is padded to the full 0x18000 so both ROM banks
/// decode. A machine built with `Cps1::new` has an *empty* sound region, where every
/// fetch reads 0xFF: the Z80 spins on `RST 38h`, `keyon_live` is zero for all eight
/// channels, and a listing shows six identical lines. That fixture cannot tell a
/// working panel from one that reads the wrong fields, so the panel tests use this one.
///
/// `pub(crate)` and outside `mod tests`, the way [`crate::font::frame`] is: the panel
/// moved here from `overlay.rs` but `overlay`'s own
/// `the_sound_panel_renders_without_panicking` draws *every* panel through
/// `overlay::draw` and still needs this machine. One fixture two modules share beats
/// two copies that drift — a copy in `overlay.rs` would go on claiming to run a driver
/// after this one's opcodes changed.
#[cfg(test)]
pub(crate) fn a_sound_machine() -> machine::Cps1 {
    // `ld a,($f008)` / `ld ($d000),a` / `jr -9`, from `machine`'s `sound_spin`.
    let mut audiocpu = vec![0u8; 0x1_8000];
    audiocpu[..9].copy_from_slice(&[0x3A, 0x08, 0xF0, 0x32, 0x00, 0xD0, 0x00, 0x18, 0xF7]);
    let rom = prog();
    let mut m = machine::Cps1::with_sound(
        &rom,
        Vec::new(),
        audiocpu,
        a_sample_rom(),
        machine::config::BoardConfig::sf2(),
        machine::timing::Timing::cps1_10mhz(),
    );
    m.reset();
    m
}

/// A sample ROM with two phrases, so the panel's OKI rows have voices to show.
///
/// A board built with `Vec::new()` refuses every phrase — `start == stop == 0` — so its
/// status byte, its voice string, and its clamp count would all read as an idle chip
/// whatever the panel did with them. `0x77` is the largest positive step repeated,
/// which ramps to near full scale and makes the OKI clamp against its own ±65536 output
/// limit once two voices are sounding: that is what gives `CLP` something to count.
#[cfg(test)]
fn a_sample_rom() -> Vec<u8> {
    let mut r = vec![0u8; 0x8000];
    // Phrase headers at `phrase * 8`: start and last byte, 24-bit big-endian.
    r[8..14].copy_from_slice(&[0x00, 0x10, 0x00, 0x00, 0x30, 0x00]);
    r[16..22].copy_from_slice(&[0x00, 0x40, 0x00, 0x00, 0x60, 0x00]);
    r[0x1000..0x3001].fill(0x77);
    r[0x4000..0x6001].fill(0x77);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::{frame, panel_contains, read_text};
    // The chrome the row-budget assertions are arithmetic over. Not imported by the
    // module itself: `box_at` applies the padding, so the panel never names it.
    use crate::font::ADVANCE;
    use crate::overlay::{PAD, STATUS_Y};
    use machine::video::{HEIGHT, WIDTH};
    use machine::Cps1;
    // The Z80's bus, for writing the board's ports the way a driver does rather than
    // reaching into the chips directly: `0xF002` is how a phrase is started, and a test
    // that called `Oki::write` would bypass the port decode the panel's rows describe.
    use machine::z80::Bus as _;

    /// The CPS-1 board inside a [`Machine`], for the assertions that read it back.
    ///
    /// `draw` takes a `&Machine`, so these tests wrap their fixture — and then need the
    /// board again to compare a panel's numbers against the machine's own. The
    /// `unreachable!` is honest: every caller here wrapped a `Cps1` two lines above.
    fn as_cps1(m: &Machine) -> &Cps1 {
        match m {
            Machine::Cps1(c) => c,
            Machine::Sf1(_) => unreachable!("wrapped from a_sound_machine"),
        }
    }

    /// CPS-1, wrapped. [`a_sound_machine`] is the fixture the CPS-1 tests use; this is
    /// it inside a `Machine` for the two tests that compare the boards.
    fn a_cps1_machine() -> Machine {
        Machine::Cps1(Box::new(a_sound_machine()))
    }

    /// SF1, with a **different** Z80 program on each sound CPU.
    ///
    /// ⚠️ The two differ on purpose — `jr -2` at 0x0000 on the FM CPU, a `nop` then
    /// `jr -2` on the ADPCM one — so `the_adpcm_row_reads_the_adpcm_cpu` has two
    /// distinguishable PCs to work with. Identical programs would let that test pass
    /// under a panel that read one CPU twice.
    ///
    /// ⚠️ `machine::sf1::test_video()` is `pub(crate)` so this crate cannot call it. The
    /// five empty regions below are exactly what it passes, and `Sf1Video::new` is
    /// `pub`. Empty is legal: the decoder returns pen 0 for a ROM it cannot index, and
    /// this panel draws no framebuffer pixels.
    fn an_sf1_machine() -> Machine {
        let mut m = machine::Sf1::new(
            &prog(),
            machine::video::sf1::Sf1Video::new(
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            vec![0x18, 0xFE],
            vec![0x00, 0x18, 0xFE],
        );
        m.reset();
        Machine::Sf1(Box::new(m))
    }

    /// The SF1 machine out of its wrapper, for a test that pokes the hardware.
    fn as_sf1(m: &mut Machine) -> &mut machine::Sf1 {
        match m {
            Machine::Sf1(s) => s,
            Machine::Cps1(_) => unreachable!("built by an_sf1_machine"),
        }
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
        let m = Machine::Cps1(Box::new(m));

        let mut buf = frame();
        draw(&mut buf, &m);

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
        let t = as_cps1(&m).sound_trace();
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
        let m = Machine::Cps1(Box::new(m));

        let mut buf = frame();
        draw(&mut buf, &m);

        // The status byte the driver itself would read. `0xF6`, not `0x06`: the chip
        // builds `0xF0` and sets one bit per playing voice, so the high nibble is always
        // set — the panel shows the byte the guest sees, not a cleaned-up version of it.
        assert_eq!(
            as_cps1(&m).sound.oki_ref().status(),
            0xF6,
            "the premise: the chip reports voices 1 and 2"
        );
        assert!(
            panel_contains(&buf, "OKI F6 V .12. DIV 165 CMD 03", HI),
            "the chip's status, its voices, its rate, and the pending command"
        );

        let t = as_cps1(&m).sound_trace();
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
    /// do. This is the arithmetic in [`CPS1_HEAD_ROWS`]'s documentation as an assertion,
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
             {STATUS_Y}), so the next row added has to come out of CPS1_DIS_ROWS — this \
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
        draw(&mut buf, &Machine::Cps1(Box::new(m)));
        let head = SND_Y + PAD + CPS1_HEAD_ROWS * LINE;
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
        let markers = (0..CPS1_DIS_ROWS)
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
        let m = Machine::Cps1(Box::new(m));
        let mut buf = frame();
        for _ in 0..8 {
            draw(&mut buf, &m);
        }
        assert_eq!(
            as_cps1(&m).sound_trace(),
            before,
            "eight frames of panel added nothing to the counters"
        );
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
        draw(&mut buf, &Machine::Cps1(Box::new(m)));
        assert!(
            panel_contains(&buf, "FFFD", HI),
            "the listing starts where the PC is"
        );
    }

    /// Seventeen rows is the ceiling, and both boards spend all of it.
    ///
    /// ⚠️ **Measured, not chosen.** `SND_Y` 90, `LINE` 7, `PAD` 1, `STATUS_Y` 214, so
    /// 17 rows ends at 211 and 18 ends at 218 — through the status line. This is the
    /// assertion that stops a row being added to either panel without one being taken
    /// away.
    #[test]
    fn seventeen_rows_is_the_ceiling_and_both_boards_use_it() {
        assert_eq!(CPS1_HEAD_ROWS + CPS1_DIS_ROWS, SND_ROWS);
        assert_eq!(SF1_HEAD_ROWS + SF1_DIS_ROWS, SND_ROWS);
        let bottom = SND_Y + SND_ROWS * LINE + 2 * PAD;
        assert_eq!(bottom, 211, "where the box ends");
        assert!(bottom < STATUS_Y, "{bottom} against {STATUS_Y}");
        // Bound to a local first, not asserted inline: every term is a `const`, and
        // clippy's `assertions_on_constants` rejects an assertion it can fold — which
        // `-D warnings` makes a build failure. `overlay.rs`'s own layout assertions at
        // HEAD are written this way for the same reason.
        let with_one_more = bottom + LINE;
        assert!(
            with_one_more > STATUS_Y,
            "an eighteenth row would cross the status line ({with_one_more} against \
             {STATUS_Y})"
        );
    }

    /// SF1 pays for its second CPU out of the listing, and the trade balances.
    #[test]
    fn sf1_pays_for_its_second_cpu_out_of_the_listing() {
        assert_eq!(CPS1_DIS_ROWS, 6, "one CPU, six instructions");
        assert_eq!(SF1_DIS_ROWS, 2, "two CPUs, two instructions");
        assert_eq!(
            SF1_HEAD_ROWS - CPS1_HEAD_ROWS,
            CPS1_DIS_ROWS - SF1_DIS_ROWS,
            "every header row SF1 gained came out of the listing"
        );
    }

    /// The rule row spans the box exactly, at whatever `SF1_COLS` becomes.
    ///
    /// ⚠️ The one row on this panel that is 45 characters and not 44 is the rule, and
    /// it is the one row that cannot overflow: it is *generated* from `SF1_COLS`
    /// rather than written as a literal. This test is what keeps it that way — a
    /// future hand-written `"---- ADPCM Z80 ------"` would fit today and be the wrong
    /// length the moment the box changes width, which is a rule that stops short of
    /// the edge or ink outside the box.
    #[test]
    fn the_rule_row_spans_the_box_exactly() {
        let rule = format!("---- ADPCM Z80 {}", "-".repeat(SF1_COLS - 15));
        assert_eq!(rule.len(), SF1_COLS, "{rule}");
        assert!(rule.starts_with("---- ADPCM Z80 "), "the label survives");
        assert!(rule.ends_with('-'), "and it runs to the last column");
    }

    /// Both boxes stay on screen, and SF1's wider one reaches the right edge.
    ///
    /// ⚠️ **The reason SF1's box is 45 columns and sits at the edge.** Its widest
    /// data row — `DSC {:010} OVR {:010} UNM {:010}` — is 44 characters at every
    /// value, so 45 is the first width that holds it. `draw_text` clips rather than
    /// panicking, so an overflow would be a silently truncated number rather than a
    /// crash, and in a 38-column box it would also print over whatever is to the
    /// box's right.
    #[test]
    fn both_boxes_stay_on_screen_and_sf1_reaches_the_edge() {
        assert_eq!(SND_X + CPS1_COLS * ADVANCE + 2 * PAD, 348);
        let sf1_right = SND_X + SF1_COLS * ADVANCE + 2 * PAD;
        assert_eq!(sf1_right, 383, "one pixel of margin");
        assert!(sf1_right <= WIDTH, "{sf1_right} against {WIDTH}");
        // A local, for the reason given in `seventeen_rows_is_the_ceiling_and_both_
        // boards_use_it`: clippy folds an all-`const` assertion and `-D warnings` then
        // fails the build.
        let one_wider = sf1_right + ADVANCE;
        assert!(
            one_wider > WIDTH,
            "and a forty-sixth column would run off the screen ({one_wider} against \
             {WIDTH})"
        );
    }

    /// Every SF1 row fits its box at the widest value it can hold.
    ///
    /// ⚠️ This is the test the first draft of this panel needed and did not have.
    /// That draft formatted counters `{:06}`, which is a *minimum* width and not a
    /// maximum: six of its fifteen rows fit at zero and overflowed at saturation.
    /// The fix was fixed-width fields (`{:010}` per `u32`, `{:013}` per `u64`) so a
    /// row's length cannot depend on its value — and this test is what holds that
    /// property when someone adds the sixteenth field.
    ///
    /// ⚠️ **What this test does not cover.** It saturates the counters, which are the
    /// fields whose width the first draft got wrong. The two non-counter columns whose
    /// widths were *also* wrong — `OUT`, which is `output()` scaled into
    /// `-16_384..=16_352` and needs `{:+06}`, and `@`, which is `bank_base()` masked
    /// into a 0x40000 region and so needs only `{:05X}` — cannot be reached from here:
    /// `output()` follows from an ADPCM stream and `bank_base()` from a bank register.
    /// Both are bounded by their format specifiers instead, and both rows sit 7
    /// characters below the box at those specifiers. The module doc's table is where
    /// they are recorded.
    #[test]
    fn no_sf1_row_overflows_its_box_with_every_counter_saturated() {
        let mut m = an_sf1_machine();
        {
            let s = as_sf1(&mut m);
            // A CPU state that maxes every hex field, and cycle counters at 2^40 —
            // 1.1e12 T-states, 85 hours of emulated Z80 time at 3.579545 MHz, which
            // is 13 digits and so exactly the width the panel is formatted for.
            s.fm_z80.pc = 0xFFFF;
            s.fm_z80.sp = 0xFFFF;
            s.adpcm_z80.pc = 0xFFFF;
            s.saturate_counters_for_test();
        }
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        draw(&mut buf, &m);
        // Nothing drew in the column strip past the box, and nothing below it.
        let right = SND_X + SF1_COLS * ADVANCE + 2 * PAD;
        let bottom = SND_Y + SND_ROWS * LINE + 2 * PAD;
        for y in SND_Y..bottom {
            for x in right..WIDTH {
                assert_eq!(buf[y * WIDTH + x], 0, "ink at ({x},{y}), past {right}");
            }
        }
        for y in bottom..HEIGHT {
            for x in SND_X..WIDTH {
                assert_eq!(buf[y * WIDTH + x], 0, "ink at ({x},{y}), below {bottom}");
            }
        }
    }

    /// Both boards draw something, and neither draws below its box.
    #[test]
    fn both_boards_draw_inside_the_box() {
        for m in [a_cps1_machine(), an_sf1_machine()] {
            let board = m.board();
            let mut buf = vec![0u32; WIDTH * HEIGHT];
            draw(&mut buf, &m);
            let bottom = SND_Y + SND_ROWS * LINE + 2 * PAD;
            for y in bottom..HEIGHT {
                for x in SND_X..WIDTH {
                    assert_eq!(buf[y * WIDTH + x], 0, "{board:?}: ink at ({x},{y})");
                }
            }
            assert!(buf.iter().any(|&p| p != 0), "{board:?}: drew nothing");
        }
    }

    /// The two boards' panels are not the same pixels.
    ///
    /// ⚠️ The failure this catches: `draw` matching on `Machine` with both arms
    /// calling `draw_cps1`. That compiles and shows a plausible panel on the wrong
    /// board.
    #[test]
    fn the_two_panels_are_different() {
        let mut a = vec![0u32; WIDTH * HEIGHT];
        let mut b = vec![0u32; WIDTH * HEIGHT];
        draw(&mut a, &a_cps1_machine());
        draw(&mut b, &an_sf1_machine());
        assert_ne!(a, b, "one board's panel was drawn for both");
    }

    /// The ADPCM CPU's row reads the ADPCM CPU.
    ///
    /// ⚠️ Checked by moving *only* the second Z80. A panel that read `fm_z80` twice
    /// would draw identical pixels, and every other test here would still pass.
    #[test]
    fn the_adpcm_row_reads_the_adpcm_cpu() {
        let mut m = an_sf1_machine();
        let mut before = vec![0u32; WIDTH * HEIGHT];
        draw(&mut before, &m);
        as_sf1(&mut m).adpcm_z80.pc = 0x0123;
        let mut after = vec![0u32; WIDTH * HEIGHT];
        draw(&mut after, &m);
        assert_ne!(before, after, "the ADPCM Z80's PC is not on the panel");
    }

    /// Each MSM5205 row reads its own chip.
    ///
    /// Same argument one level down: a panel that indexed `msm(0)` twice would show a
    /// board where one voice can never be diagnosed.
    ///
    /// ⚠️ `msm_w` is the door, not `data_w`: `data_w` latches a nibble but only
    /// `vclk_w`'s falling edge arms a capture, so `data_w` plus ticks moves nothing.
    /// `msm_w` does reset, data, and both edges.
    #[test]
    fn each_msm_row_reads_its_own_chip() {
        let mut m = an_sf1_machine();
        let mut quiet = vec![0u32; WIDTH * HEIGHT];
        draw(&mut quiet, &m);
        {
            let s = as_sf1(&mut m);
            s.adpcm.msm_mut(1).msm_w(0x0F);
            // ⚠️ `machine::sf1::msm5205`, not `machine::oki::msm5205`. `oki` is the
            // reused ADPCM *decoder* crate; the MSM5205 wrapper with the capture
            // timing is `machine`'s own, and `CAPTURE_CLOCKS` is 6.
            for _ in 0..machine::sf1::msm5205::CAPTURE_CLOCKS {
                s.adpcm.msm_mut(1).tick();
            }
            assert_ne!(s.adpcm.msm(1).signal(), 0, "chip 1 moved");
            assert_eq!(s.adpcm.msm(0).signal(), 0, "chip 0 did not");
        }
        let mut moved = vec![0u32; WIDTH * HEIGHT];
        draw(&mut moved, &m);
        assert_ne!(quiet, moved, "chip 1's row does not read chip 1");
    }

    /// Drawing the SF1 panel does not move the machine's counters.
    ///
    /// The same property CPS-1's `drawing_the_sound_panel_does_not_move_the_counters`
    /// asserts, and for the same reason: this panel reads the ADPCM CPU's ROM through
    /// `peek_byte`, and reading it through the Z80 bus instead would add the panel's
    /// own fetches to the `FET` count three rows above it.
    #[test]
    fn drawing_the_sf1_panel_does_not_move_the_counters() {
        let mut m = an_sf1_machine();
        m.run_frame();
        let before = {
            let s = as_sf1(&mut m);
            (
                s.fm.trace(),
                s.adpcm.trace(),
                s.z80_cycles(),
                s.adpcm_z80_cycles(),
            )
        };
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        draw(&mut buf, &m);
        draw(&mut buf, &m);
        let after = {
            let s = as_sf1(&mut m);
            (
                s.fm.trace(),
                s.adpcm.trace(),
                s.z80_cycles(),
                s.adpcm_z80_cycles(),
            )
        };
        assert_eq!(
            before.0.audiocpu_fetches, after.0.audiocpu_fetches,
            "FM fetches"
        );
        assert_eq!(before.1.rom_fetches, after.1.rom_fetches, "ADPCM fetches");
        assert_eq!(before.1.bank_fetches, after.1.bank_fetches, "bank fetches");
        assert_eq!(before.2, after.2, "FM T-states");
        assert_eq!(before.3, after.3, "ADPCM T-states");
    }

    /// A listing at the top of the Z80's space wraps rather than panicking.
    ///
    /// ⚠️ The 68000 panel's equivalent case, and the reason it is a test on both
    /// boards: debug builds panic on overflow, and a debugger is most often opened
    /// *because* a PC has gone somewhere it should not be.
    #[test]
    fn an_sf1_listing_at_the_top_of_the_space_wraps() {
        let mut m = an_sf1_machine();
        as_sf1(&mut m).fm_z80.pc = 0xFFFD;
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        draw(&mut buf, &m);
        assert!(buf.iter().any(|&p| p != 0), "it drew");
    }

    /// The SF1 panel prints the host ring's counters, and prints the machine's.
    ///
    /// # Why the glyphs and not a call count
    ///
    /// The failure this catches is two hardcoded zeroes — the thing Task 18's ⚠️ in
    /// `draw_sf1` forbade until the door existed. A test that asserted `draw_sf1` reads
    /// `m.audio_drops()` would pass on a panel that read it and printed a literal. So
    /// the numbers go in through `set_audio_stats` and come back out of the pixels.
    ///
    /// # Why the two are on different rows
    ///
    /// Not tidy; it is what the box allows. Row 12 (`DSC`/`OVR`/`UNM`) is 44 of 45
    /// columns, `SND_ROWS` is 17 and an eighteenth row would be 218 against a status
    /// line at 214, and a 46-column box is 388 pixels of a 384-pixel window. Row 14
    /// (`FET`/`YM`) is 28 columns, so one `DRP {:010}` fits at 43 and two do not at 57;
    /// row 4 (`KEYON 01234567`) is the panel's widest slack at 14, taking `UND` to 29.
    ///
    /// ⚠️ 1,234 and 56, not one number twice: swapped arguments would render
    /// identically if both fields carried the same value.
    #[test]
    fn the_sf1_panel_shows_the_host_rings_counters() {
        let mut m = an_sf1_machine();
        as_sf1(&mut m).set_audio_stats(1_234, 56);
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        draw(&mut buf, &m);
        assert!(
            panel_contains(&buf, "DRP 0000001234", FG),
            "the fetch row must carry the drop count"
        );
        assert!(
            panel_contains(&buf, "UND 0000000056", HI),
            "and the key-on row the underrun count"
        );
        // Neither number is anywhere else, so a panel that printed the pair twice — or
        // printed `drops` into both fields — fails.
        assert!(!panel_contains(&buf, "UND 0000001234", HI));
        assert!(!panel_contains(&buf, "DRP 0000000056", FG));
    }

    /// And only those two rows respond to the host's counters.
    ///
    /// The pixel-difference half: a segment added to the wrong row would satisfy the
    /// text assertions above — `panel_contains` scans the whole frame — while pushing
    /// some other row's numbers sideways.
    #[test]
    fn only_two_sf1_rows_move_when_the_host_rings_counters_change() {
        let mut m = an_sf1_machine();
        let mut a = vec![0u32; WIDTH * HEIGHT];
        let mut b = vec![0u32; WIDTH * HEIGHT];
        as_sf1(&mut m).set_audio_stats(0, 0);
        draw(&mut a, &m);
        as_sf1(&mut m).set_audio_stats(1_234, 56);
        draw(&mut b, &m);
        assert_ne!(
            a, b,
            "the panel does not read the host ring's counters at all — two hardcoded \
             zeroes render identically whatever the machine says"
        );
        let moved: Vec<usize> = (0..SND_ROWS)
            .filter(|&r| {
                let y0 = SND_Y + PAD + r * LINE;
                (y0..y0 + LINE).any(|y| (0..WIDTH).any(|x| a[y * WIDTH + x] != b[y * WIDTH + x]))
            })
            .collect();
        assert_eq!(moved, vec![4, 14], "row 4 carries UND and row 14 DRP");
    }
}
