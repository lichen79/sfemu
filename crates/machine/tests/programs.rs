//! Hand-assembled 68000 programs run against the CPS-1 board.
//!
//! # What these replace
//!
//! Sub-project A had 317,500 external vector cases as its oracle. Sub-project B has
//! none: there is no public test suite for a Capcom board. These programs are the
//! standin, and this comment is explicit that they are a weaker one — they cover the
//! paths we thought of, no more.
//!
//! What they *do* guarantee is that no expectation is self-consistent with the code
//! under test: each program's expected outcome is a number written by hand from the
//! 68000 manual and the memory map, and each is mutation-checked.
//!
//! Every encoding below was verified against `m68k::disasm` on 2026-08-07 and the
//! disassembler's own rendering is quoted beside it. None was taken on trust.

use machine::{BoardConfig, Cps1, Timing};

/// Builds a ROM image: reset vector, an optional level-2 handler vector, the program
/// at 0x1000, and any extra blocks.
fn rom(prog: &[u16], vec2: Option<u32>, extra: &[(usize, &[u16])]) -> Vec<u8> {
    let mut r = vec![0u8; 0x4000];
    let put = |r: &mut Vec<u8>, at: usize, words: &[u16]| {
        for (i, w) in words.iter().enumerate() {
            let [h, l] = w.to_be_bytes();
            r[at + 2 * i] = h;
            r[at + 2 * i + 1] = l;
        }
    };
    // SSP = 0x00FF8000 (top of main RAM), PC = 0x00001000.
    //
    // ⚠️ The word split matters and is easy to get wrong: 0x00FF8000 is
    // `[0x00FF, 0x8000]`, **not** `[0x0000, 0xFF80]`. The latter is 0x0000FF80,
    // which is in ROM — the board swallows writes there, so every exception frame
    // would be discarded and every `rte` would pop ROM bytes as a return address.
    // Eight of the nine programs below still passed that way, because they never
    // return from anything. `a_stopped_cpu_is_woken_by_the_vblank_interrupt` is what
    // caught it; `the_stack_pointer_points_at_writable_ram` pins it directly.
    put(&mut r, 0, &[0x00FF, 0x8000, 0x0000, 0x1000]);
    if let Some(h) = vec2 {
        // Autovector level 2 = vector 24 + 2 = 26, at 26 * 4 = 0x68.
        put(&mut r, 0x68, &[(h >> 16) as u16, h as u16]);
    }
    put(&mut r, 0x1000, prog);
    for (at, words) in extra {
        put(&mut r, *at, words);
    }
    r
}

fn machine(rom: &[u8]) -> Cps1 {
    let mut m = Cps1::new(rom, BoardConfig::sf2(), Timing::cps1_10mhz());
    m.reset();
    m
}

/// The shape of every CPS-1 game's main loop: drop the mask, then spin.
///
/// ```text
/// 1000  46FC 2000   move #$2000,sr    supervisor, interrupt mask 0
/// 1004  60FE        bra  $1004        spin forever
/// ```
const MAIN_LOOP: &[u16] = &[0x46FC, 0x2000, 0x60FE];

/// A level-2 handler that counts itself in `ram[0]`, at 0x2000.
///
/// ```text
/// 2000  5279 00FF 0000   addq.w #1,$FF0000
/// 2006  4E73             rte
/// ```
const COUNTING_HANDLER: &[u16] = &[0x5279, 0x00FF, 0x0000, 0x4E73];

/// The reset vector puts the stack in writable RAM.
///
/// Every program below that takes an interrupt depends on this, and a stack pointing
/// into ROM fails *silently*: the board reports a ROM write as handled, so the frame
/// is discarded with no exception and no diagnostic. This asserts the address as a
/// literal and then shows a push actually lands.
#[test]
fn the_stack_pointer_points_at_writable_ram() {
    let mut m = machine(&rom(MAIN_LOOP, Some(0x2000), &[(0x2000, COUNTING_HANDLER)]));
    assert_eq!(m.cpu.a[7], 0x00FF_8000, "top of main RAM, not 0x0000FF80");

    // The level-2 frame is 6 bytes: SR at SSP-6, PC at SSP-4. After one frame the
    // handler has run once, so those words hold the interrupted context — nonzero,
    // and in RAM rather than nowhere.
    m.run_frame();
    let sp = 0x00FF_8000usize;
    let ram = |a: usize| m.board.ram[(a >> 1) & 0x7FFF];
    assert_eq!(ram(sp - 6), 0x2000, "the stacked SR: supervisor, mask 0");
    assert_eq!(ram(sp - 4), 0x0000, "the stacked PC, high word");
    assert_eq!(
        ram(sp - 2),
        0x1004,
        "the stacked PC, low word: the spin loop"
    );
}

/// The vblank counter increments exactly once per frame.
///
/// Not zero — the IRQ was never recognised. Not many — the line was never
/// acknowledged and the handler re-entered until the stack wrapped.
#[test]
fn vblank_increments_a_counter_once_per_frame() {
    let mut m = machine(&rom(MAIN_LOOP, Some(0x2000), &[(0x2000, COUNTING_HANDLER)]));
    for want in 1..=3u16 {
        m.run_frame();
        assert_eq!(
            m.board.ram[0], want,
            "frame {want}: the handler must run exactly once per frame"
        );
    }
}

/// The acknowledge is what makes the count 1 rather than hundreds.
///
/// ⚠️ This asserts the **observable artifact** — the handler's own increment — and
/// deliberately not `board.vblank_pending()`. A test that reads the flag the code
/// sets passes a half-done fix, and this project has produced that exact defect
/// before.
///
/// Ten frames rather than one, because the count and the frame count must stay equal
/// in *both* directions: an unacknowledged line re-enters (the mask only blocks it
/// during the handler, not after the `rte`), and a line acknowledged before the CPU
/// ever sampled it would count zero.
#[test]
fn the_handler_runs_once_per_frame_over_ten_frames_neither_dropped_nor_re_entered() {
    let mut m = machine(&rom(MAIN_LOOP, Some(0x2000), &[(0x2000, COUNTING_HANDLER)]));
    for _ in 0..10 {
        m.run_frame();
    }
    // A frame is 167,680 cycles and this handler costs on the order of 90, so an
    // unacknowledged level-2 line would re-enter on the order of a thousand times
    // per frame — the wrong answer here is not 11, it is four figures.
    assert_eq!(m.board.ram[0], 10, "ten frames, ten interrupts");
}

/// Vblank fires on line 240 and on no earlier line.
///
/// # Why this test exists
///
/// The per-frame count above cannot see a per-line phase error: a vblank wrongly
/// asserted on line 0 also fires exactly once per frame, so
/// `line == vblank_line` → `line == 0` survives every test that counts by the frame.
/// Verified — that mutant lived until this test existed.
///
/// `run_scanline` runs the line held in `self.line` and then advances it, and reset
/// leaves `line == 0`, so the call that runs line 240 is the **241st**. That literal
/// is the assertion; it is hand-derived from the counter's semantics and not read
/// back from `m.line`.
#[test]
fn the_vblank_interrupt_fires_on_line_240_and_not_before() {
    let mut m = machine(&rom(MAIN_LOOP, Some(0x2000), &[(0x2000, COUNTING_HANDLER)]));
    let mut first = None;
    for call in 1..=262u32 {
        m.run_scanline();
        if m.board.ram[0] != 0 && first.is_none() {
            first = Some(call);
        }
    }
    assert_eq!(
        first,
        Some(241),
        "line 240 is run by the 241st call after a reset, so that is the first call \
         by which the handler can have run"
    );
    assert_eq!(m.board.ram[0], 1, "and only once in the frame");
}

/// `STOP` parks the CPU; the vblank must wake it.
///
/// Zero vector cases cover this path — `STOP`'s access shape is empty and no vector
/// case runs a second step — so this program is the only evidence the resume works
/// at all.
///
/// ```text
/// 1000  46FC 2000        move #$2000,sr
/// 1004  4E72 2000        stop #$2000          stopped, mask 0, supervisor
/// 1008  5279 00FF 0002   addq.w #1,$FF0002    reached only after the handler returns
/// 100E  60FE             bra  $100E
/// ```
#[test]
fn a_stopped_cpu_is_woken_by_the_vblank_interrupt() {
    let mut m = machine(&rom(
        &[
            0x46FC, 0x2000, 0x4E72, 0x2000, 0x5279, 0x00FF, 0x0002, 0x60FE,
        ],
        Some(0x2000),
        &[(0x2000, COUNTING_HANDLER)],
    ));
    m.run_frame();
    assert_eq!(m.board.ram[0], 1, "the handler ran");
    assert_eq!(m.board.ram[1], 1, "and execution resumed past the STOP");
    assert!(!m.cpu.stopped, "the CPU is running again");
}

/// A stopped CPU stays stopped until the vblank arrives.
///
/// The test above would pass just as well if `STOP` were a no-op: the program would
/// fall straight through to the increment. This one shows the park is real by
/// checking that nothing past the `STOP` has run before line 240 — with `STOP`
/// ignored, `ram[1]` would be in the hundreds by the first frame's end.
#[test]
fn a_stopped_cpu_executes_nothing_until_the_interrupt_arrives() {
    let mut m = machine(&rom(
        &[
            0x46FC, 0x2000, 0x4E72, 0x2000, 0x5279, 0x00FF, 0x0002, 0x60FE,
        ],
        Some(0x2000),
        &[(0x2000, COUNTING_HANDLER)],
    ));
    for _ in 0..240 {
        m.run_scanline();
    }
    assert_eq!(m.board.ram[0], 0, "no interrupt yet");
    assert_eq!(
        m.board.ram[1], 0,
        "and the CPU has executed nothing since STOP"
    );
}

/// SF2's boot self-test in miniature: read the CPS-B ID register and branch on it.
///
/// ```text
/// 1000  3039 0080 0172        move.w $800172,d0
/// 1006  0C40 0401             cmpi.w #$0401,d0
/// 100A  6608                  bne    $1014            -> skip the pass marker
/// 100C  33FC 00A5 00FF 0000   move.w #$00A5,$FF0000
/// 1014  4E72 2000             stop   #$2000           both paths land here
/// ```
///
/// The `bne.s` displacement is relative to the instruction's own address + 2 =
/// 0x100C, so `+8` targets 0x1014 — confirmed by the disassembler rendering `6608`
/// at 0x100A as `bne $1014`.
const CPSB_ID_CHECK: &[u16] = &[
    0x3039, 0x0080, 0x0172, // move.w $800172,d0
    0x0C40, 0x0401, // cmpi.w #$0401,d0
    0x6608, // bne $1014
    0x33FC, 0x00A5, 0x00FF, 0x0000, // move.w #$00A5,$FF0000
    0x4E72, 0x2000, // stop #$2000
];

#[test]
fn the_cpsb_id_check_takes_the_pass_branch() {
    let mut m = machine(&rom(CPSB_ID_CHECK, None, &[]));
    m.run_frame();
    assert_eq!(
        m.board.ram[0], 0x00A5,
        "the board must answer 0x800172 with 0x0401, so the branch is not taken"
    );
}

/// The negative control, and the reason the test above is not vacuous: with the ID
/// register wrong, the same program must **fail**.
#[test]
fn the_cpsb_id_check_fails_when_the_board_answers_wrongly() {
    let wrong = BoardConfig {
        cpsb_value: 0x0000,
        ..BoardConfig::sf2()
    };
    let mut m = Cps1::new(&rom(CPSB_ID_CHECK, None, &[]), wrong, Timing::cps1_10mhz());
    m.reset();
    m.run_frame();
    assert_eq!(m.board.ram[0], 0x0000, "the pass branch must be skipped");
}

/// gfxram is byte-readable, big-endian, and distinct from main RAM.
///
/// ```text
/// 1000  33FC 1234 0090 0000   move.w #$1234,$900000
/// 1008  1639 0090 0000        move.b $900000,d3      -> 0x12, the high byte
/// 100E  13C3 00FF 0000        move.b d3,$FF0000
/// 1014  4E72 2000             stop #$2000
/// ```
#[test]
fn gfxram_word_writes_are_readable_as_big_endian_bytes() {
    let mut m = machine(&rom(
        &[
            0x33FC, 0x1234, 0x0090, 0x0000, //
            0x1639, 0x0090, 0x0000, //
            0x13C3, 0x00FF, 0x0000, //
            0x4E72, 0x2000,
        ],
        None,
        &[],
    ));
    m.run_frame();
    assert_eq!(m.board.gfxram[0], 0x1234, "the word landed in gfxram");
    assert_eq!(
        m.board.ram[0], 0x1200,
        "the byte read back is 0x12 — the high half of the word is at the even \
         address — and `move.b` to 0xFF0000 puts it in that word's high half too"
    );
}

/// A masked interrupt stays pending until the guest lowers the mask, and the
/// pending line does not survive the acknowledge.
///
/// # What this pins that the tests above cannot
///
/// Every test above lets the interrupt in immediately, so it never observes the line
/// while it is *outstanding*. Two mutants survived them:
///
/// - **`vblank_pending = false` unconditionally on every ROM read.** With the mask
///   at 0, the acknowledge fetch happens in the same instruction that recognised the
///   interrupt, so clearing on any ROM read is indistinguishable from clearing on the
///   vector fetch: both land within one step. Only a program that runs from ROM for
///   thousands of cycles *while the line is asserted* separates them.
/// - **`if self.line == 240` in place of `self.timing.vblank_line`.** Both are 240
///   for `cps1_10mhz()`, so no test that uses that `Timing` can tell them apart.
///   This test supplies a 20-line frame with `vblank_line: 10`.
///
/// ```text
/// 1000  46FC 2700   move #$2700,sr    supervisor, interrupt mask 7 — locked out
/// 1004  323C 031F   move.w #$031F,d1  799
/// 1008  51C9 FFFE   dbra  d1,$1008    ~8,000 cycles of ROM fetches
/// 100C  46FC 2000   move #$2000,sr    mask 0 — the interrupt lands here
/// 1010  60FE        bra   $1010
/// ```
///
/// The frame is deliberately 20 lines of 640 so the whole run is 12,800 cycles: long
/// enough for the delay loop to finish and short enough to state every line's state
/// as a literal.
#[test]
fn a_masked_interrupt_stays_pending_across_scanlines_and_is_cleared_by_the_fetch() {
    let short_frame = Timing {
        cpu_hz: 10_000_000,
        line_cycles: (640, 1),
        lines_per_frame: 20,
        vblank_line: 10,
    };
    let r = rom(
        &[
            0x46FC, 0x2700, // move #$2700,sr
            0x323C, 0x031F, // move.w #$031F,d1
            0x51C9, 0xFFFE, // dbra d1,$1008
            0x46FC, 0x2000, // move #$2000,sr
            0x60FE, // bra $1010
        ],
        Some(0x2000),
        &[(0x2000, COUNTING_HANDLER)],
    );
    let mut m = Cps1::new(&r, BoardConfig::sf2(), short_frame);
    m.reset();

    // Lines 0-9: before the vblank line. Nothing asserted, nothing run.
    for line in 0..10 {
        m.run_scanline();
        assert!(
            !m.board.vblank_pending(),
            "line {line} is before vblank_line 10"
        );
        assert_eq!(m.board.ram[0], 0);
    }

    // Lines 10 and 11: asserted and *masked*. The CPU is executing the `dbra` loop
    // out of ROM the whole time — thousands of ROM reads, none of them the vector
    // fetch — so a line cleared by any ROM read would drop here.
    for line in 10..12 {
        m.run_scanline();
        assert!(
            m.board.vblank_pending(),
            "line {line}: asserted, masked, and not yet acknowledged"
        );
        assert_eq!(
            m.board.ram[0], 0,
            "line {line}: the handler cannot have run"
        );
    }

    // Line 12: the `dbra` finishes, `move #$2000,sr` lowers the mask, the interrupt
    // is taken, the vector fetch drops the line, and the handler runs once.
    m.run_scanline();
    assert!(!m.board.vblank_pending(), "the fetch acknowledged it");
    assert_eq!(m.board.ram[0], 1, "and the handler ran exactly once");

    // The rest of the frame: the guest spins with the mask at 0 and nothing is
    // asserted, so the count must not move.
    for _ in 13..20 {
        m.run_scanline();
    }
    assert_eq!(m.board.ram[0], 1, "one interrupt in the frame, not two");
}

/// The trace counts what the program actually did, and nothing it did not.
///
/// Every count is a literal derived from the program, one write per port. A
/// counter placed on the wrong arm shows up as one of these being 0 and another
/// being 2 — which is why they are asserted together rather than one per test.
///
/// ```text
/// 1000  33FC 0040 0080 010C   move.w #$0040,$80010C   CPS-A
/// 1008  33FC 1234 0090 0000   move.w #$1234,$900000   gfxram
/// 1010  33FC 00AB 0080 0180   move.w #$00AB,$800180   sound latch
/// 1018  33FC FFFF 0081 0000   move.w #$FFFF,$810000   unmapped
/// 1020  33FC 5555 0080 0146   move.w #$5555,$800146   CPS-B
/// 1028  4E72 2700             stop #$2700
/// ```
///
/// `stop #$2700` and not the mask-0 form: with the mask down the vblank at line
/// 240 vectors through an all-zero table and the resulting garbage adds writes of
/// its own to every counter below.
#[test]
fn the_trace_counts_what_the_program_actually_did() {
    let mut m = machine(&rom(
        &[
            0x33FC, 0x0040, 0x0080, 0x010C, //
            0x33FC, 0x1234, 0x0090, 0x0000, //
            0x33FC, 0x00AB, 0x0080, 0x0180, //
            0x33FC, 0xFFFF, 0x0081, 0x0000, //
            0x33FC, 0x5555, 0x0080, 0x0146, //
            0x4E72, 0x2700,
        ],
        None,
        &[],
    ));
    m.run_frame();
    let t = &m.board.trace;
    assert_eq!(t.cps_a_writes, 1, "0x80010C");
    assert_eq!(t.cps_b_writes, 1, "0x800146");
    assert_eq!(t.gfxram_writes, 1, "0x900000");
    assert_eq!(t.sound_latch_writes, 1, "0x800180");
    assert_eq!(t.rom_writes, 0, "this program writes no ROM");
    assert_eq!(t.unmapped_writes.total(), 1);
    assert_eq!(t.unmapped_writes.entries(), &[(0x81_0000, 1)]);
    assert_eq!(t.unmapped_writes.dropped(), 0);
    assert_eq!(
        t.unmapped_reads.total(),
        0,
        "every read this program makes is a ROM fetch"
    );
    assert_eq!(t.frames, 1);
    assert_eq!(t.vblanks, 1, "line 240 asserted once");
    assert_eq!(t.acks, 0, "and the mask kept it from ever being taken");
}

/// A ROM write is counted apart from an unmapped one.
///
/// A real CPS-1 decodes 0x000000-0x3FFFFF, so a write there is a guest bug, not
/// evidence our map is missing a chip. Folding the two together would put the
/// program's own ROM address at the top of the "worst unmapped" report and send
/// the reader looking for a chip that exists.
///
/// ```text
/// 1000  33FC DEAD 0000 2000   move.w #$DEAD,$2000     ROM
/// 1008  33FC BEEF 0040 0000   move.w #$BEEF,$400000   unmapped
/// 1010  4E72 2700             stop #$2700
/// ```
#[test]
fn a_rom_write_is_counted_separately_from_an_unmapped_one() {
    let mut m = machine(&rom(
        &[
            0x33FC, 0xDEAD, 0x0000, 0x2000, //
            0x33FC, 0xBEEF, 0x0040, 0x0000, //
            0x4E72, 0x2700,
        ],
        None,
        &[],
    ));
    m.run_frame();
    let t = &m.board.trace;
    assert_eq!(t.rom_writes, 1);
    assert_eq!(t.unmapped_writes.total(), 1);
    assert_eq!(t.unmapped_writes.entries(), &[(0x40_0000, 1)]);
}

/// An unmapped *read* is counted and named too.
///
/// ```text
/// 1000  3039 0040 0000   move.w $400000,d0    unmapped read
/// 1006  4E72 2700        stop #$2700
/// ```
///
/// The value read is 0xFFFF (the floating bus), which the counter does not care
/// about — what matters is that the address is on the report, because a boot that
/// polls a chip we have not modelled shows up here and nowhere else.
#[test]
fn an_unmapped_read_is_counted_and_named() {
    let mut m = machine(&rom(&[0x3039, 0x0040, 0x0000, 0x4E72, 0x2700], None, &[]));
    m.run_frame();
    let t = &m.board.trace;
    assert_eq!(t.unmapped_reads.total(), 1);
    assert_eq!(t.unmapped_reads.entries(), &[(0x40_0000, 1)]);
    assert_eq!(t.unmapped_writes.total(), 0, "a read is not a write");
    assert_eq!(m.cpu.d[0] & 0xFFFF, 0xFFFF, "and it read the floating bus");
}

/// Every vblank the guest services is counted as an acknowledge.
///
/// This is the trace's headline health check — `acks` short of `vblanks` means the
/// game is not servicing the interrupt — so it gets its own test with a handler
/// that returns, over ten frames.
#[test]
fn ten_serviced_frames_give_ten_vblanks_and_ten_acks() {
    let mut m = machine(&rom(MAIN_LOOP, Some(0x2000), &[(0x2000, COUNTING_HANDLER)]));
    for _ in 0..10 {
        m.run_frame();
    }
    let t = &m.board.trace;
    assert_eq!(t.frames, 10);
    assert_eq!(t.vblanks, 10);
    assert_eq!(t.acks, 10, "one acknowledge per vblank");
    assert_eq!(m.board.ram[0], 10, "and the handler ran once per vblank");
}

/// `frames` counts wraps of the scanline counter, not calls to `run_frame`.
///
/// A counter incremented inside `run_frame` would report 0 for a debugger stepping
/// scanline by scanline — the one caller most likely to be reading the trace.
#[test]
fn frames_are_counted_by_scanline_wraps_not_by_run_frame_calls() {
    let mut m = machine(&rom(&[0x46FC, 0x2700, 0x60FE], None, &[]));
    for _ in 0..261 {
        m.run_scanline();
    }
    assert_eq!(m.board.trace.frames, 0, "line 261 has not wrapped yet");
    m.run_scanline();
    assert_eq!(m.board.trace.frames, 1, "the 262nd line wraps");
    m.run_frame();
    assert_eq!(m.board.trace.frames, 2, "and a whole frame adds one");
}

/// PC samples are capped rather than growing without bound.
///
/// Ten frames is 2,620 scanlines, so an uncapped sampler would hold 2,620 entries.
/// The cap is 100 and the assertion is the literal 100.
#[test]
fn pc_samples_are_capped_rather_than_growing_without_bound() {
    let mut m = machine(&rom(&[0x46FC, 0x2700, 0x60FE], None, &[]));
    m.board.trace.pc_sample_cap = 100;
    for _ in 0..10 {
        m.run_frame();
    }
    assert_eq!(m.board.trace.pc_samples.len(), 100);
    // The program is a two-instruction loop at 0x1004, so every sample is one of
    // the two PCs inside it — evidence the samples are the guest's and not zeroes.
    for &pc in &m.board.trace.pc_samples {
        assert!(
            (0x1004..=0x1008).contains(&pc),
            "sampled PC {pc:#06x} is outside the loop"
        );
    }
}

/// Sampling is off unless the caller asks for it.
///
/// A frontend running for an hour is 56 million scanlines. A sampler on by default
/// is a memory leak that only shows up in the one run nobody profiles.
#[test]
fn pc_sampling_is_off_by_default() {
    let mut m = machine(&rom(&[0x46FC, 0x2700, 0x60FE], None, &[]));
    m.run_frame();
    assert!(m.board.trace.pc_samples.is_empty());
}

/// `reset` clears the machine's schedule and leaves the instrument alone.
///
/// The trace is attached to the machine, not part of it: a caller that raised
/// `pc_sample_cap` and then reset would otherwise find sampling silently off, and
/// a driver resetting mid-run would lose everything it had observed.
#[test]
fn a_reset_clears_the_schedule_but_not_the_trace() {
    let mut m = machine(&rom(MAIN_LOOP, Some(0x2000), &[(0x2000, COUNTING_HANDLER)]));
    m.board.trace.pc_sample_cap = 4;
    m.run_frame();
    assert_eq!(m.board.trace.frames, 1);
    assert_eq!(m.board.trace.pc_samples.len(), 4);

    m.reset();
    assert_eq!(m.total_cycles, 0, "the schedule is cleared");
    assert_eq!(m.line, 0);
    assert_eq!(m.board.trace.frames, 1, "and the trace is not");
    assert_eq!(m.board.trace.vblanks, 1);
    assert_eq!(m.board.trace.pc_sample_cap, 4, "the cap the caller set");
}

/// An idle board reads all ones through the DIP-switch port.
///
/// ```text
/// 1000  3239 0080 001A   move.w $80001A,d1     DSWA
/// 1006  33C1 00FF 0000   move.w d1,$FF0000
/// 100C  4E72 2000        stop #$2000
/// ```
#[test]
fn an_unpressed_board_reads_all_ones_through_the_dip_port() {
    let mut m = machine(&rom(
        &[
            0x3239, 0x0080, 0x001A, 0x33C1, 0x00FF, 0x0000, 0x4E72, 0x2000,
        ],
        None,
        &[],
    ));
    m.run_frame();
    assert_eq!(m.board.ram[0], 0xFFFF, "active low, every switch off");
}
