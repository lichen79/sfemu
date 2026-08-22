//! The demo ROM: a whole CPS-1 image, assembled from nothing.
//!
//! [`build`] returns the four regions a CPS-1 machine is constructed from —
//! `maincpu`, `gfx`, `audiocpu`, `oki` — as `(name, bytes)` pairs. It needs no
//! files and reaches no network, so the emulator runs on a machine that has
//! never seen a commercial ROM set.
//!
//! # What it puts on screen
//!
//! Three scroll layers and a sprite, all moving:
//!
//! - **Scroll 3** (32×32 tiles) at the back, scrolling one pixel a frame, in
//!   framed tiles so its tile grid is visible.
//! - **Scroll 2** (16×16) over it, scrolling two pixels a frame the *other* way.
//!   Opposite directions at different rates is what makes the layers'
//!   independence visible; two moving together would look like one layer.
//! - **A sprite** — a disc drawn on the transparent pen — tracing a closed path
//!   from a table. The layers behind show through at its corners, which is the
//!   visible evidence that sprite transparency works.
//! - **Scroll 1** (8×8) in front, a four-digit frame counter. This is the piece
//!   that proves the 68000 is still executing: a frozen first frame and a running
//!   emulator look identical until a number changes.
//!
//! # What it plays
//!
//! The 68000 writes a sound command to the latch about once a second; the Z80
//! driver polls it and turns each change into an FM note on the YM2151 plus an
//! ADPCM phrase on the OKI. So the audio path is not merely initialised, it is
//! *driven from the other CPU* — a broken latch is audible rather than something
//! only a test can see.
//!
//! # This module hardcodes the formats
//!
//! No dependency on `video`, `machine` or `romset` — see the crate manifest. So
//! every register offset, boundary and table address below is a literal with its
//! source in the comment. A generator that imported those constants from the
//! consumer could not disagree with it, and so could not detect a wrong one.

use crate::asm68k::Asm as Asm68k;
use crate::asmz80::{Alu, Asm as AsmZ80, Cond, Pair, Reg8, Stack};
use crate::gfx::{self, Kind};

/// Builds the demo's four ROM regions, named as a `RomSet` names them.
///
/// Order is `maincpu`, `gfx`, `audiocpu`, `oki`, and each is exactly the size the
/// board maps, so a caller can insert them into a `RomSet` unchanged.
pub fn build() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("maincpu", maincpu()),
        ("gfx", graphics()),
        ("audiocpu", audiocpu()),
        ("oki", okirom()),
    ]
}

// ---- The 68000's view of the board ----------------------------------------

/// Where the program is assembled to run. `maincpu` is mapped at 0.
const ROM_ORIGIN: u32 = 0;

/// The `maincpu` region's size: 0x000000-0x3FFFFF.
const MAINCPU_BYTES: usize = 0x40_0000;

/// First byte of gfxram on the 68000's bus.
const GFXRAM: u32 = 0x90_0000;
/// First byte of the CPS-A register file.
const CPS_A: u32 = 0x80_0100;
/// First byte of the CPS-B register file.
const CPS_B: u32 = 0x80_0140;
/// The sound command latch, as the 68000 sees it.
const SOUND_LATCH: u32 = 0x80_0180;

/// Main RAM's first byte. RAM is 0xFF0000-0xFFFFFF.
const RAM: u32 = 0xFF_0000;
/// Word: frames since reset, wrapped at [`COUNTER_WRAP`].
const VAR_FRAME: u32 = RAM;
/// Word: scroll 2's accumulated horizontal scroll.
const VAR_SCROLL2: u32 = RAM + 2;
/// Word: scroll 3's.
const VAR_SCROLL3: u32 = RAM + 4;

/// The initial supervisor stack pointer.
///
/// Near the top of RAM but not past it. A stack pointer of 0x1000000 truncates
/// to 0 in the 68000's 24-bit space, and the first `jsr` would then write the
/// return address over the reset vectors — survivable right up until something
/// reads them again.
const STACK_TOP: u32 = 0xFF_FF00;

// CPS-A register byte offsets from `CPS_A`. Byte offsets because that is what a
// 68000 program writes; `video::regs` holds the same numbers halved into word
// indices.
/// Object (sprite) table base.
const CPS_A_OBJ_BASE: u32 = 0x00;
/// Scroll-1 tilemap base.
const CPS_A_SCROLL1_BASE: u32 = 0x02;
/// Scroll-2 tilemap base.
const CPS_A_SCROLL2_BASE: u32 = 0x04;
/// Scroll-3 tilemap base.
const CPS_A_SCROLL3_BASE: u32 = 0x06;
/// Palette base.
const CPS_A_PALETTE_BASE: u32 = 0x0A;
/// Scroll-1 horizontal scroll.
const CPS_A_SCROLL1_X: u32 = 0x0C;
/// Scroll-1 vertical scroll.
const CPS_A_SCROLL1_Y: u32 = 0x0E;
/// Scroll-2 horizontal scroll.
const CPS_A_SCROLL2_X: u32 = 0x10;
/// Scroll-2 vertical scroll.
const CPS_A_SCROLL2_Y: u32 = 0x12;
/// Scroll-3 horizontal scroll.
const CPS_A_SCROLL3_X: u32 = 0x14;
/// Scroll-3 vertical scroll.
const CPS_A_SCROLL3_Y: u32 = 0x16;
/// Video control: bit 0 row scroll, bits 2-3 the second enable for scrolls 2 and
/// 3, bit 15 screen flip.
const CPS_A_VIDEOCONTROL: u32 = 0x22;

// CPS-B register byte offsets for the layout `BoardConfig::sf2` configures.
/// Layer control: three enable bits, and four 2-bit depth fields from bit 6.
const CPS_B_LAYER_CONTROL: u32 = 0x26;
/// Palette page enable, one bit per 512-entry page.
const CPS_B_PALETTE_CONTROL: u32 = 0x30;
/// The board-ID register a CPS-1 program's boot self-test reads.
const CPS_B_ID: u32 = 0x32;
/// What that register is wired to answer on this board.
const CPS_B_ID_VALUE: u16 = 0x0401;

/// Byte offsets into gfxram for the four tables this demo lays out.
///
/// A base register holds `address / 256`, and the hardware truncates the result
/// *down* to the table's own alignment. So each address here must be a multiple
/// of 256 and of its own boundary — scroll tables 0x4000, the object table
/// 0x800, the palette 0x400. [`tests::every_table_satisfies_its_alignment`]
/// checks all of them, because a misaligned table is not rejected: it silently
/// moves down onto whatever sits below it, and the picture that results reads as
/// a tilemap bug.
mod at {
    /// Palette: 6 pages × 512 entries × 2 bytes = 0x1800.
    pub const PALETTE: u32 = 0x0_0000;
    /// Object table, 0x800 bytes.
    pub const OBJ: u32 = 0x0_2000;
    /// Scroll 1's tile table: 0x1000 cells of two words.
    pub const SCROLL1: u32 = 0x0_4000;
    /// Scroll 2's.
    pub const SCROLL2: u32 = 0x0_8000;
    /// Scroll 3's.
    pub const SCROLL3: u32 = 0x0_C000;
}

/// gfxram's size in bytes: 192 KB, 0x900000-0x92FFFF.
const GFXRAM_BYTES: u32 = 0x3_0000;

/// The last table must end inside gfxram.
///
/// A compile-time check and not a test: a table running past the end does not
/// fault, because the board takes the address modulo gfxram's length. The writes
/// wrap onto the palette, and the symptom is colours that change as the tile
/// maps are filled.
const _: () = assert!(at::SCROLL3 + MAP_CELLS * 4 <= GFXRAM_BYTES);

/// Cells in one axis of every layer's tile map.
const MAP_TILES: u32 = 64;
/// Cells in a whole map, hence entries a fill routine writes.
const MAP_CELLS: u32 = MAP_TILES * MAP_TILES;

/// The visible frame's origin in raster coordinates, horizontally.
const VISIBLE_X: u16 = 64;
/// And vertically.
const VISIBLE_Y: u16 = 16;
/// The visible frame's width.
const WIDTH: u16 = 384;
/// Its height.
const HEIGHT: u16 = 224;

/// Where the counter counts to before starting again.
///
/// Four digits, so the display never has to render a fifth. This keeps every
/// `divu` quotient below ten: dividing a five-digit frame by 1000 gives a
/// two-digit result, of which only the low digit is drawn, and the thousands
/// column would then count out of step with the rest — which looks like a font
/// bug rather than a range one.
const COUNTER_WRAP: u16 = 10_000;

/// The tile-map row the counter is drawn on.
///
/// `draw_tilemap` reads map row `(y + VISIBLE_Y + scroll_y) / 8` for visible line
/// `y`, so with scroll 1 at rest the top visible row is `VISIBLE_Y / 8` = 2.
/// Rows 0 and 1 are above the screen.
const COUNTER_ROW: u32 = VISIBLE_Y as u32 / 8;

/// The map column the counter's most significant digit sits at.
///
/// Same arithmetic on the other axis: the leftmost visible column is
/// `VISIBLE_X / 8` = 8. Columns 0-7 are off the left edge, and a counter written
/// there is correct, invisible, and indistinguishable from one that never
/// updates.
const COUNTER_COL: u32 = VISIBLE_X as u32 / 8;

// Tile codes. Each must fall inside a range STF29's mapper covers for its
// graphics type, or the tile has no ROM behind it and `draw_tilemap` skips it
// entirely. [`tests::every_tile_code_is_one_the_mapper_covers`] checks each,
// because "nothing drawn" looks exactly like a renderer that does not work —
// which is the diagnosis this whole crate exists to rule out.
/// Scroll 1's blank tile, the counter's background.
const T1_BLANK: u32 = 0x4000;
/// Scroll 1's ten digits, at `T1_DIGIT + n`.
const T1_DIGIT: u32 = 0x4010;
/// Scroll 2's framed tile.
const T2_FRAME: u32 = 0x2800;
/// Scroll 3's framed tile.
const T3_FRAME: u32 = 0x0400;
/// The sprite's disc.
const SPR_DISC: u32 = 0x0100;

/// The attribute word written beside scroll 1's codes.
///
/// `tile_info` computes the palette scheme as `colour_base + (attr & 0x1F)`, with
/// scroll 1's base at 0x20, so 0 here means scheme 0x20. Bits 5 and 6 are the
/// flips and bits 7-8 the priority group; all zero.
const SCROLL1_ATTR: u16 = 0x0000;
/// Scroll 2's, whose colour base is 0x40.
const SCROLL2_ATTR: u16 = 0x0000;
/// Scroll 3's, whose colour base is 0x60.
const SCROLL3_ATTR: u16 = 0x0000;

/// The palette scheme scroll 1's tiles resolve to.
const SCROLL1_SCHEME: usize = 0x20;
/// Scroll 2's.
const SCROLL2_SCHEME: usize = 0x40;
/// Scroll 3's.
const SCROLL3_SCHEME: usize = 0x60;
/// The sprite's. Sprites take `attr & 0x1F` with **no** base added, so they own
/// schemes 0x00-0x1F.
const SPRITE_SCHEME: usize = 0x00;

/// Pens per palette scheme.
const PEN_GRANULARITY: usize = 16;

// ---- The 68000 program ----------------------------------------------------

/// The 68000 program: vectors, setup, an idle loop, and a vblank handler.
///
/// Every frame's work happens in the vblank handler, which is where a real driver
/// puts it and which is what exercises the interrupt path. The main loop is a
/// branch to itself.
fn maincpu() -> Vec<u8> {
    let mut a = Asm68k::new(ROM_ORIGIN);

    // ---- Exception vectors ----
    // Reset takes the supervisor stack pointer from longword 0 and the PC from
    // longword 1.
    a.long(STACK_TOP);
    a.long_label("start");
    // Vectors 2 to 25 — 0x08 to 0x67 — point at a handler that returns, rather
    // than being left at zero. A vector of zero does not fault: it sends the
    // exception to address 0, and the 68000 executes the reset stack pointer as
    // instructions.
    while a.here() < VEC_AUTOVECTOR_2 {
        a.long_label("spurious");
    }
    assert_eq!(a.here(), VEC_AUTOVECTOR_2, "the level-2 autovector's slot");
    a.long_label("vblank");

    // ---- Entry ----
    a.label("start");
    // Supervisor, interrupt mask 7: no vblank arrives while setup is still
    // writing the tables the handler is about to read.
    a.move_to_sr(0x2700);
    a.jsr("setup");
    // Mask 0, so level 2 is taken.
    a.move_to_sr(0x2000);
    a.label("idle");
    a.bra("idle");

    // ---- The handler every unclaimed vector points at ----
    a.label("spurious");
    a.rte();

    setup(&mut a);
    vblank(&mut a);
    subroutines(&mut a);

    // ---- Data ----
    a.label("pal_data");
    for w in palette_words() {
        a.word(w);
    }
    a.label("obj_data");
    for w in obj_words() {
        a.word(w);
    }
    a.label("path_data");
    for w in path_words() {
        a.word(w);
    }

    let mut rom = a.finish();
    assert!(
        rom.len() <= MAINCPU_BYTES,
        "the program must fit the ROM space, not {} bytes",
        rom.len()
    );
    rom.resize(MAINCPU_BYTES, 0);
    rom
}

/// Where the 68000 fetches the level-2 autovector.
///
/// CPS-1 drives the IPL pins so that vblank is interrupt *level 2*, whose
/// autovector is 24 + 2 = 26, at 26 × 4 = 0x68. Reading this longword is also
/// what acknowledges the interrupt on this board, so a handler installed at any
/// other vector would leave the line asserted and take the same interrupt
/// forever.
const VEC_AUTOVECTOR_2: u32 = 0x68;

/// Programs the custom registers and lays out every table in gfxram.
fn setup(a: &mut Asm68k) {
    a.label("setup");

    // The board-ID register a boot self-test reads back. Writing it is what a
    // CPS-1 program always does first; a demo that skipped it would still run,
    // and would leave the one register every real driver touches untouched.
    a.move_w_imm_dn(CPS_B_ID_VALUE, 0);
    a.move_w_dn_abs(0, CPS_B + CPS_B_ID);

    // Table bases, each the byte address divided by 256.
    for (reg, addr) in [
        (CPS_A_PALETTE_BASE, at::PALETTE),
        (CPS_A_OBJ_BASE, at::OBJ),
        (CPS_A_SCROLL1_BASE, at::SCROLL1),
        (CPS_A_SCROLL2_BASE, at::SCROLL2),
        (CPS_A_SCROLL3_BASE, at::SCROLL3),
    ] {
        a.move_w_imm_dn((addr / 256) as u16, 0);
        a.move_w_dn_abs(0, CPS_A + reg);
    }

    // All six palette pages enabled. Three are used, and enabling every one keeps
    // the palette's compaction rule out of the picture: a *disabled* early page
    // shifts every later page's source down by 512 words, which would move every
    // colour on screen at once.
    a.move_w_imm_dn(0x003F, 0);
    a.move_w_dn_abs(0, CPS_B + CPS_B_PALETTE_CONTROL);

    a.move_w_imm_dn(LAYER_CONTROL, 0);
    a.move_w_dn_abs(0, CPS_B + CPS_B_LAYER_CONTROL);

    // Bits 2 and 3 are scroll 2's and scroll 3's *second* enable condition — a
    // layer is drawn only if its layer-control bit and this one are both set, and
    // only those two layers have one. Bit 0, row scroll, stays clear: this demo
    // scrolls whole layers, and a row-scroll table it never wrote would be read
    // out of whatever gfxram happens to hold.
    a.move_w_imm_dn(0x000C, 0);
    a.move_w_dn_abs(0, CPS_A + CPS_A_VIDEOCONTROL);

    // Every scroll register to zero. The two the handler drives are set again each
    // frame; these are the initial state, and nothing clears a custom register at
    // reset.
    for reg in [
        CPS_A_SCROLL1_X,
        CPS_A_SCROLL1_Y,
        CPS_A_SCROLL2_X,
        CPS_A_SCROLL2_Y,
        CPS_A_SCROLL3_X,
        CPS_A_SCROLL3_Y,
    ] {
        a.moveq(0, 0);
        a.move_w_dn_abs(0, CPS_A + reg);
    }

    // The palette and the sprite records are blocks of words in ROM, copied.
    for (src, dst, words) in [
        ("pal_data", GFXRAM + at::PALETTE, PALETTE_WORDS),
        ("obj_data", GFXRAM + at::OBJ, OBJ_INIT_WORDS),
    ] {
        a.movea_l_label_an(src, 2);
        a.movea_l_imm_an(dst, 1);
        a.move_w_imm_dn(words as u16 - 1, 0);
        a.jsr("blockcopy");
    }

    // The three tile maps are *filled* rather than copied: 0x1000 cells of one
    // repeated tile would be 8 KB of ROM per layer to store what four
    // instructions write.
    for (dst, code, attr) in [
        (at::SCROLL1, T1_BLANK, SCROLL1_ATTR),
        (at::SCROLL2, T2_FRAME, SCROLL2_ATTR),
        (at::SCROLL3, T3_FRAME, SCROLL3_ATTR),
    ] {
        a.movea_l_imm_an(GFXRAM + dst, 1);
        a.move_w_imm_dn(code as u16, 1);
        a.move_w_imm_dn(attr, 2);
        a.jsr("filltable");
    }

    // Clear the variables. RAM holds whatever it held at power-on, and a frame
    // counter starting from that would print a plausible number and count up from
    // it — which looks exactly like the counter working.
    for v in [VAR_FRAME, VAR_SCROLL2, VAR_SCROLL3] {
        a.moveq(0, 0);
        a.move_w_dn_abs(0, v);
    }
    a.rts();
}

/// The layer-control word.
///
/// Bits 3, 4 and 5 enable scrolls 1, 2 and 3. The four 2-bit fields from bit 6 up
/// name what is drawn at each depth, walked in order, so the field at bit 6 is
/// drawn *first* and ends up at the back. 0 selects the sprites; 1, 2 and 3
/// select scrolls 1, 2 and 3.
///
/// Back to front: scroll 3, scroll 2, sprites, scroll 1. Putting the sprite
/// between the two scrolling layers and the counter in front of everything is
/// what makes the depth order legible — four layers in any order look the same if
/// nothing overlaps.
const LAYER_CONTROL: u16 = 0x0038 | depth(3, 0) | depth(2, 1) | depth(0, 2) | depth(1, 3);

/// `layer` drawn at depth `slot`, in the bits the layer-control word wants it.
const fn depth(layer: u16, slot: u16) -> u16 {
    layer << (6 + 2 * slot)
}

/// The vblank handler: the counter, the scrolls, the sprite, and the sound.
fn vblank(a: &mut Asm68k) {
    a.label("vblank");

    // ---- The frame counter, wrapped so it stays four digits ----
    a.move_w_abs_dn(VAR_FRAME, 0);
    a.addq_w(1, 0);
    a.cmpi_w(COUNTER_WRAP, 0);
    a.bne("frame_stored");
    a.moveq(0, 0);
    a.label("frame_stored");
    a.move_w_dn_abs(0, VAR_FRAME);

    // ---- Scroll 3 one pixel a frame ----
    a.move_w_abs_dn(VAR_SCROLL3, 1);
    a.addq_w(1, 1);
    a.move_w_dn_abs(1, VAR_SCROLL3);
    a.move_w_dn_abs(1, CPS_A + CPS_A_SCROLL3_X);

    // ---- Scroll 2 two pixels a frame, the other way ----
    //
    // The accumulator counts up and the register gets its negation, so the two
    // layers run opposite ways at different rates. Negating after the store
    // rather than in place keeps the stored value monotonic, which leaves the
    // register's sign as the only thing this depends on.
    a.move_w_abs_dn(VAR_SCROLL2, 1);
    a.addq_w(2, 1);
    a.move_w_dn_abs(1, VAR_SCROLL2);
    a.neg_w(1);
    a.move_w_dn_abs(1, CPS_A + CPS_A_SCROLL2_X);

    // ---- The sprite's position, from the path table ----
    //
    // `frame & (PATH_STEPS - 1)` indexes the table, and each entry is two words,
    // so the byte offset is that times four. `adda.w` sign-extends the word
    // offset into the address register, and the largest offset is
    // 4 × (PATH_STEPS − 1) — well inside a positive word.
    a.move_w_abs_dn(VAR_FRAME, 0);
    a.andi_w(PATH_STEPS as u16 - 1, 0);
    a.add_w_dn_dn(0, 0);
    a.add_w_dn_dn(0, 0);
    a.movea_l_label_an("path_data", 2);
    a.adda_w_dn_an(0, 2);
    a.move_w_postinc_dn(2, 1);
    a.move_w_postinc_dn(2, 2);
    // Both drawable records get the same position. Record 1 exists only because
    // the end-marker rule answers `offset - 4` and so skips the record *before*
    // the marker; leaving it where setup put it would show a second, stationary
    // disc, which reads as a sprite the emulator failed to move.
    for rec in [0, SPRITE_RECORD_BYTES] {
        a.move_w_dn_abs(1, GFXRAM + at::OBJ + rec);
        a.move_w_dn_abs(2, GFXRAM + at::OBJ + rec + 2);
    }

    a.jsr("draw_counter");

    // ---- One sound command a second, cycling through eight ----
    //
    // The Z80 plays on each *change*, so a command that repeated would be silent
    // after the first frame. Eight distinct values means the driver is heard to
    // follow the latch rather than to have been triggered once.
    a.move_w_abs_dn(VAR_FRAME, 0);
    a.andi_w(SOUND_PERIOD - 1, 0);
    a.bne("no_sound");
    a.move_w_abs_dn(VAR_FRAME, 0);
    a.lsr_w_imm(6, 0);
    a.andi_w(0x0007, 0);
    // Commands 1-8: zero is what the latch reads before the first write, and the
    // driver treats it as "nothing to play".
    a.addq_w(1, 0);
    a.move_b_dn_abs(0, SOUND_LATCH);
    a.label("no_sound");
    a.rte();
}

/// Frames between sound commands, a power of two so the test is one `andi.w`.
///
/// 64 frames is about a second at CPS-1's ~60 Hz. It does not divide
/// [`COUNTER_WRAP`], so the note sequence skips a step every 10000 frames —
/// inaudible, and not worth a second variable to avoid.
const SOUND_PERIOD: u16 = 64;

/// Bytes per sprite record: four words — x, y, code, attr.
const SPRITE_RECORD_BYTES: u32 = 8;

/// The three subroutines the setup and the handler call.
fn subroutines(a: &mut Asm68k) {
    // ---- blockcopy: d0 + 1 words from (a2) to (a1) ----
    //
    // Entry and loop top are the same address, because `dbra` runs the body
    // `d0 + 1` times and the count is already in place.
    a.label("blockcopy");
    a.move_w_postinc_postinc(2, 1);
    a.dbra(0, "blockcopy");
    a.rts();

    // ---- filltable: a whole tile map at (a1), code in d1, attr in d2 ----
    //
    // Two words per cell, `MAP_CELLS` cells. `dbra` runs its body `d0 + 1` times,
    // so the count is one less than the cell total; a loop one cell short would
    // leave the map's last cell holding whatever gfxram held, which draws one
    // wrong tile in one corner.
    a.label("filltable");
    a.move_w_imm_dn(MAP_CELLS as u16 - 1, 0);
    a.label("filltable_loop");
    a.move_w_dn_postinc(1, 1);
    a.move_w_dn_postinc(2, 1);
    a.dbra(0, "filltable_loop");
    a.rts();

    // ---- draw_counter: four decimal digits of VAR_FRAME into scroll 1 ----
    //
    // `divu` divides the whole 32-bit register and leaves the quotient in the low
    // word with the remainder in the high word, so the dividend has to be cleared
    // to 32 bits first: `moveq #0` then `move.w`, because `move.w` alone leaves
    // the high half holding whatever was there.
    //
    // Between places the remainder comes back down through d2 rather than being
    // left in the high word. `swap` alone would put the previous *quotient* into
    // the high half of the next dividend, and the next division would be of a
    // number 65536 times too large — which still produces four digits, just not
    // the frame's.
    a.label("draw_counter");
    a.moveq(0, 0);
    a.move_w_abs_dn(VAR_FRAME, 0);
    for (i, place) in [1000u16, 100, 10, 1].into_iter().enumerate() {
        let cell = GFXRAM + at::SCROLL1 + scroll1_cell_bytes(COUNTER_COL + i as u32, COUNTER_ROW);
        a.move_w_imm_dn(place, 1);
        a.divu_dn_dn(1, 0);
        // The tile code for this digit, and the attribute beside it.
        a.move_w_imm_dn(T1_DIGIT as u16, 3);
        a.add_w_dn_dn(0, 3);
        a.move_w_dn_abs(3, cell);
        a.move_w_imm_dn(SCROLL1_ATTR, 3);
        a.move_w_dn_abs(3, cell + 2);
        // The remainder becomes the next dividend.
        a.swap(0);
        a.move_w_dn_dn(0, 2);
        a.moveq(0, 0);
        a.move_w_dn_dn(2, 0);
    }
    a.rts();
}

/// Byte offset into a scroll-1 table of the cell at `(col, row)`.
///
/// Scroll 1's scan mapper is `(row & 0x1F) + ((col & 0x3F) << 5) + ((row & 0x20)
/// << 6)`, and each cell is two words. So consecutive columns are 128 bytes
/// apart, which is why each digit is written to its own absolute address rather
/// than through a walking pointer.
const fn scroll1_cell_bytes(col: u32, row: u32) -> u32 {
    let scan = (row & 0x1F) + ((col & 0x3F) << 5) + ((row & 0x20) << 6);
    scan * 4
}

// ---- The palette ----------------------------------------------------------

/// The palette's size in words: 6 pages × 512 entries.
const PALETTE_WORDS: usize = 6 * 0x200;

/// A palette entry from 4-bit channels, at full brightness.
///
/// An entry holds blue at bits 0-3, green at 4-7, red at 8-11 and brightness at
/// 12-15, and the brightness field scales the whole entry — 15 is unity and 0 is
/// about a third. Every entry here is 15, because a demo a third as bright as
/// intended is easy to ship and hard to notice.
const fn rgb(r: u16, g: u16, b: u16) -> u16 {
    0xF000 | (r << 8) | (g << 4) | b
}

/// The palette the demo writes: four schemes of a few pens each.
///
/// Each layer's colour base puts its scheme 0 somewhere different — scroll 1 at
/// 0x20, scroll 2 at 0x40, scroll 3 at 0x60, sprites at 0x00 with no base — so
/// this is four small blocks at four widely separated offsets, with everything
/// else left black.
fn palette_words() -> Vec<u16> {
    let mut pal = vec![0u16; PALETTE_WORDS];
    let mut set = |scheme: usize, pen: u8, entry: u16| {
        pal[scheme * PEN_GRANULARITY + usize::from(pen)] = entry;
    };
    // The sprite's disc.
    set(SPRITE_SCHEME, SPRITE_PEN, rgb(15, 15, 2));
    // The counter's digits.
    set(SCROLL1_SCHEME, DIGIT_PEN, rgb(15, 15, 15));
    // Scroll 2's border. It has no fill entry, because its interior is the
    // transparent pen — see [`graphics`]. A colour given here for a pen the tile
    // never uses would read as the fill's colour and be unreachable.
    set(SCROLL2_SCHEME, S2_EDGE_PEN, rgb(2, 14, 6));
    // Scroll 3's, which is the layer that shows through scroll 2.
    set(SCROLL3_SCHEME, S3_EDGE_PEN, rgb(6, 6, 14));
    set(SCROLL3_SCHEME, S3_FILL_PEN, rgb(1, 1, 5));
    pal
}

/// The pen the sprite's disc is drawn on.
const SPRITE_PEN: u8 = 0x05;
/// The pen the counter's digits are drawn on.
const DIGIT_PEN: u8 = 0x06;
/// Scroll 2's tile border. Its interior is [`gfx::TRANSPARENT_PEN`], so scroll 3
/// shows through — a fill pen here would hide the layer behind it entirely.
const S2_EDGE_PEN: u8 = 0x03;
/// Scroll 3's tile border.
const S3_EDGE_PEN: u8 = 0x01;
/// Scroll 3's tile interior.
const S3_FILL_PEN: u8 = 0x02;

// ---- The object table and the sprite's path -------------------------------

/// How many words of the object table setup copies from ROM.
const OBJ_INIT_WORDS: usize = 12;

/// The object table's initial contents: two drawable records and an end marker.
///
/// Four words per record — x, y, code, attr — and the marker is a record whose
/// attribute satisfies `attr & 0xFF00 == 0xFF00`. The scan answers `offset - 4`,
/// so **the record before the marker is skipped as well**: the marker has to be
/// in record 2 for record 0 to draw at all. Record 1 is that sacrificial record,
/// and it holds a copy rather than blanks so a change in the rule shows up as a
/// doubled sprite instead of as nothing.
fn obj_words() -> Vec<u16> {
    let (x, y) = path_step(0);
    // Scheme 0, no flips, a single 16×16 tile: `attr & 0x0F00` and `attr & 0xF000`
    // are the block's dimensions minus one, so zero is 1×1.
    let record = [x, y, SPR_DISC as u16, 0x0000];
    let mut w = Vec::new();
    w.extend_from_slice(&record);
    w.extend_from_slice(&record);
    w.extend_from_slice(&[0, 0, 0, 0xFF00]);
    w
}

/// Positions in the sprite's path. A power of two, so the 68000 indexes it with
/// one `andi.w` instead of a division.
const PATH_STEPS: u32 = 64;

/// The path table: [`PATH_STEPS`] `(x, y)` raster positions.
fn path_words() -> Vec<u16> {
    let mut w = Vec::new();
    for i in 0..PATH_STEPS {
        let (x, y) = path_step(i);
        w.push(x);
        w.push(y);
    }
    w
}

/// Step `i` of the sprite's path, as a raster `(x, y)`.
///
/// Two triangle waves at different rates, which traces a closed figure rather
/// than a line. Both axes must vary independently: a straight diagonal would be
/// produced just as well by a bug that wrote one coordinate into both registers.
/// Triangles rather than sines because there is no floating point here, and the
/// shape is a closed path either way.
///
/// `tri` reduces its own phase, so the vertical axis passes `3 * i` unreduced.
/// Writing `3 * i % PATH_STEPS` here would read as the guard that keeps the path
/// closed while doing nothing at all — and a mutation that removed it could not
/// be caught by any test.
fn path_step(i: u32) -> (u16, u16) {
    let x = tri(i, PATH_STEPS, u32::from(WIDTH) - 2 * SPRITE_EDGE);
    let y = tri(3 * i, PATH_STEPS, u32::from(HEIGHT) - 2 * SPRITE_EDGE);
    (
        (x + u32::from(VISIBLE_X) + SPRITE_EDGE) as u16,
        (y + u32::from(VISIBLE_Y) + SPRITE_EDGE) as u16,
    )
}

/// A sprite's edge in pixels, which is also how far the path keeps it from the
/// edge of the frame.
///
/// One sprite's width of margin, so the disc is wholly on screen at every step.
/// The blitter clips rather than wraps, so a sprite past the edge is silently cut
/// in half — visible, but as a rendering artefact rather than as a path that goes
/// too far.
const SPRITE_EDGE: u32 = 16;

/// A triangle wave: 0 up to `amp` over the first half of `period`, back down over
/// the second.
fn tri(i: u32, period: u32, amp: u32) -> u32 {
    let half = period / 2;
    let phase = i % period;
    if phase < half {
        phase * amp / half
    } else {
        (period - phase) * amp / half
    }
}

// ---- The graphics region --------------------------------------------------

/// The graphics region's size in bytes.
///
/// STF29 maps three 0x8000-unit banks, so a mapped code can reach 0x18000 8×8
/// units of 64 bytes. The region covers all of it: a shorter one would leave some
/// mapped codes reading past the end, and a tile that is not wholly inside the
/// ROM decodes as the transparent pen — an invisible tile, which is the failure
/// this size rules out.
const GFX_BYTES: usize = 0x1_8000 * 64;

/// The demo's tiles, drawn into a region sized for the whole mapped space.
///
/// The codes are placed through the mapper's own arithmetic ([`map_code`]): a
/// code's ROM offset is `(bank_base + ((code << shift) & (size - 1))) >> shift`,
/// in tiles of the code's own size. Drawing at the raw code instead would put
/// every tile somewhere the renderer does not look — and for scroll 1, in the
/// wrong bank entirely.
fn graphics() -> Vec<u8> {
    let mut rom = vec![0u8; GFX_BYTES];

    // Scroll 3's big framed tile, at the back.
    gfx::framed(
        &mut rom,
        Kind::Tile32x32,
        map_code(Gfx::Scroll3, T3_FRAME),
        S3_FILL_PEN,
        S3_EDGE_PEN,
    );

    // Scroll 2's smaller one over it, in different pens so the two layers are told
    // apart on screen and not only in a test — and with a **transparent** interior,
    // which is the whole reason scroll 3 is visible at all.
    //
    // An opaque fill here is not a subtle wrong colour: scroll 2's grid covers every
    // pixel of the screen, so scroll 3 would be drawn, scrolled, and completely
    // invisible. Every counter a headless run prints would be unchanged, and the
    // picture would look like a deliberate one-layer demo. `sfemu`'s
    // `the_demo_runs_and_draws_and_talks_to_the_sound_board` counts *four* palette
    // pages for this reason.
    gfx::framed(
        &mut rom,
        Kind::Tile16x16,
        map_code(Gfx::Scroll2, T2_FRAME),
        gfx::TRANSPARENT_PEN,
        S2_EDGE_PEN,
    );

    // The sprite: a disc on the transparent pen.
    gfx::disc(
        &mut rom,
        Kind::Tile16x16,
        map_code(Gfx::Sprite, SPR_DISC),
        SPRITE_PEN,
    );

    // Scroll 1's background, wholly transparent — and `solid` with the
    // transparent pen, not a zeroed tile: pen 0 is a *colour*, so a blank left as
    // zeros would draw an opaque block over everything behind it.
    for kind in [Kind::Tile8x8, Kind::Tile8x8Odd] {
        gfx::solid(
            &mut rom,
            kind,
            map_code(Gfx::Scroll1, T1_BLANK),
            gfx::TRANSPARENT_PEN,
        );
    }
    for d in 0..10u8 {
        gfx::digit(
            &mut rom,
            map_code(Gfx::Scroll1, T1_DIGIT + u32::from(d)),
            d,
            DIGIT_PEN,
        );
    }

    rom
}

/// Which set of banks a code is fetched through.
///
/// A transcription of `video`'s graphics types, deliberately — see the module
/// documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gfx {
    Sprite,
    Scroll1,
    Scroll2,
    Scroll3,
}

impl Gfx {
    /// How far a code shifts to become an 8×8-unit offset.
    const fn shift(self) -> u32 {
        match self {
            Self::Scroll1 => 0,
            Self::Sprite | Self::Scroll2 => 1,
            Self::Scroll3 => 3,
        }
    }
}

/// STF29's range table, in 8×8 units: `(type, first, last, bank)`.
///
/// The first matching range wins, which is why the sprite ranges come before the
/// layer ranges they overlap in unit space.
const STF29: [(Gfx, u32, u32, usize); 6] = [
    (Gfx::Sprite, 0x0_0000, 0x0_7FFF, 0),
    (Gfx::Sprite, 0x0_8000, 0x0_FFFF, 1),
    (Gfx::Sprite, 0x1_0000, 0x1_1FFF, 2),
    (Gfx::Scroll3, 0x0_2000, 0x0_3FFF, 2),
    (Gfx::Scroll1, 0x0_4000, 0x0_4FFF, 2),
    (Gfx::Scroll2, 0x0_5000, 0x0_7FFF, 2),
];

/// The three bank sizes in 8×8 units. Bank 3 is absent on this board, and a
/// zero-size bank rejects every code that reaches it.
const BANK_SIZES: [u32; 4] = [0x8000, 0x8000, 0x8000, 0];

/// The ROM offset, in tiles of the code's own size, that `code` fetches from.
///
/// `None` when no range covers the code, which for a generator means the caller
/// picked a code the board cannot address.
fn try_map_code(kind: Gfx, code: u32) -> Option<u32> {
    let shift = kind.shift();
    let unit = code << shift;
    let &(_, _, _, bank) = STF29
        .iter()
        .find(|&&(k, first, last, _)| k == kind && (first..=last).contains(&unit))?;
    let size = BANK_SIZES[bank];
    if size == 0 {
        return None;
    }
    let base: u32 = BANK_SIZES[..bank].iter().sum();
    Some((base + (unit & (size - 1))) >> shift)
}

/// [`try_map_code`], panicking on a code the board cannot reach.
///
/// # Panics
///
/// Panics when no range covers the code. Drawing nothing instead would ship a ROM
/// with a blank layer, and a blank layer reads as an emulator fault — which is
/// the one diagnosis this whole crate exists to rule out.
fn map_code(kind: Gfx, code: u32) -> u32 {
    try_map_code(kind, code)
        .unwrap_or_else(|| panic!("no STF29 range covers {kind:?} code {code:#x}"))
}

// ---- The Z80 sound driver -------------------------------------------------

/// The `audiocpu` region's size: 0x8000 fixed, plus two 0x4000 banks at 0x10000.
///
/// The driver fits in the fixed region and the banked window stays zero. A bank
/// switch is not part of what this demo demonstrates, and a driver that switched
/// into empty ROM would execute 0xFF — `rst 38h`, straight back into itself.
const AUDIOCPU_BYTES: usize = 0x1_8000;

/// Where the driver keeps the last command it saw. Sound RAM is 0xD000-0xD7FF.
const Z80_LAST_CMD: u16 = 0xD000;
/// Sound RAM's first byte past the end, which is where the stack starts. A push
/// decrements before it writes, so the first byte written is inside the RAM.
const Z80_STACK_TOP: u16 = 0xD800;

/// The YM2151's address port. A write here latches a register number, applied by
/// the next write to [`Z80_YM_DATA`].
const Z80_YM_ADDR: u16 = 0xF000;
/// The YM2151's data port.
const Z80_YM_DATA: u16 = 0xF001;
/// The OKI's command port.
const Z80_OKI: u16 = 0xF002;
/// The sound command latch, as the Z80 sees it.
const Z80_LATCH: u16 = 0xF008;

/// The Z80 sound driver.
///
/// A poll and not an interrupt, deliberately: the YM2151's IRQ is a timer the
/// driver would first have to configure, and the claim this demo makes is about
/// the *latch* — the 68000 writing 0x800180 and the Z80 seeing it at 0xF008.
fn audiocpu() -> Vec<u8> {
    let mut a = AsmZ80::new(0);

    // No interrupt source is configured, and a stray one would vector to 0x38 —
    // which on this board is inside the driver's own code.
    a.di();
    a.im1();
    a.ld_pair_imm(Pair::Sp, Z80_STACK_TOP);

    a.call("ym_init");

    // Seed the last-command byte from the latch rather than from zero: the driver
    // plays on a *change*, and if the 68000 has already written by the time this
    // runs, a hardcoded zero would play that first command twice.
    a.ld_a_abs(Z80_LATCH);
    a.ld_pair_imm(Pair::Hl, Z80_LAST_CMD);
    a.ld_r_r(Reg8::MemHl, Reg8::A);

    a.label("poll");
    a.ld_a_abs(Z80_LATCH);
    a.alu_r(Alu::Cp, Reg8::MemHl);
    a.jr_cc(Cond::Z, "poll");
    a.ld_r_r(Reg8::MemHl, Reg8::A);
    // Command 0 is "nothing to play", which is what the latch reads before the
    // 68000 has written to it.
    a.alu_imm(Alu::Cp, 0);
    a.jr_cc(Cond::Z, "poll");
    a.call("play");
    a.jr("poll");

    play(&mut a);
    ym_init(&mut a);
    ym_write(&mut a);

    let mut rom = a.finish();
    assert!(
        rom.len() <= FIXED_ROM_BYTES,
        "the driver must fit the fixed region, not {} bytes",
        rom.len()
    );
    rom.resize(AUDIOCPU_BYTES, 0);
    rom
}

/// How much of `audiocpu` the Z80 sees without switching a bank.
const FIXED_ROM_BYTES: usize = 0x8000;

/// `play`: the command is in A.
fn play(a: &mut AsmZ80) {
    a.label("play");
    a.push(Stack::Af);
    // Key off first, so a note already sounding is released rather than
    // retriggered part way through its envelope. Register 0x08 with the channel in
    // bits 2-0 and no operator bits set.
    a.ld_r_imm(Reg8::B, YM_KEY);
    a.ld_r_imm(Reg8::C, 0x00);
    a.call("ym_write");
    a.pop(Stack::Af);

    // The key code. Register 0x28 holds the octave in bits 6-4 and the note in
    // bits 3-0, and the YM2151 uses only twelve of the sixteen note codes — the
    // four with both low bits set are not notes. Doubling a 0-7 command gives
    // 0, 2, 4 … 14, which skips every one of them; masking the command into the
    // nibble directly would land on them and produce a pitch nobody chose.
    a.alu_imm(Alu::And, 0x07);
    a.alu_r(Alu::Add, Reg8::A);
    a.alu_imm(Alu::Add, YM_OCTAVE);
    a.ld_r_r(Reg8::C, Reg8::A);
    a.ld_r_imm(Reg8::B, YM_KEYCODE);
    a.call("ym_write");

    // Key on: all four operators of channel 0, bits 3-6 of register 0x08.
    a.ld_r_imm(Reg8::B, YM_KEY);
    a.ld_r_imm(Reg8::C, YM_ALL_OPERATORS);
    a.call("ym_write");

    // And an ADPCM phrase, so the OKI is driven too. Two writes: the phrase with
    // bit 7 set, then a byte holding the voice mask in the high nibble and a
    // volume *index* in the low one. Index 0 is the loudest and indices 9 to 15
    // are silent, so the low nibble is not a volume in the direction it looks
    // like.
    a.ld_r_imm(Reg8::A, 0x80 | DEMO_PHRASE);
    a.ld_abs_a(Z80_OKI);
    a.ld_r_imm(Reg8::A, OKI_VOICE_0_LOUDEST);
    a.ld_abs_a(Z80_OKI);
    a.ret();
}

/// The YM2151 key-on/off register: channel in bits 2-0, operators in bits 3-6.
const YM_KEY: u8 = 0x08;
/// All four operators, in the position register 0x08 wants them.
const YM_ALL_OPERATORS: u8 = 0x78;
/// The key-code register for channel 0: octave in bits 6-4, note in bits 3-0.
const YM_KEYCODE: u8 = 0x28;
/// Octave 4, in the position register 0x28 wants it.
const YM_OCTAVE: u8 = 4 << 4;

/// The OKI's second command byte: voice 0 in the high nibble, volume index 0 —
/// the loudest — in the low one.
const OKI_VOICE_0_LOUDEST: u8 = 0x10;

/// `ym_init`: channel 0, algorithm 7, four operators with a fast envelope.
///
/// Algorithm 7 routes all four operators to the output, so every one is audible
/// and a single wrong operator register is *heard* rather than masked by a
/// modulator that never reaches the mixer.
fn ym_init(a: &mut AsmZ80) {
    a.label("ym_init");
    a.ld_r_imm(Reg8::B, 0x20);
    // Right and left output enabled, feedback 0, algorithm 7.
    a.ld_r_imm(Reg8::C, 0xC7);
    a.call("ym_write");

    // The four operators, whose register blocks are eight apart. E holds the
    // offset; `ym_write` clobbers only A, so E survives the calls.
    a.ld_r_imm(Reg8::E, 0x00);
    a.label("ym_op");
    for (reg, val) in [
        (0x40u8, 0x01u8), // detune and multiple
        (0x60, 0x00),     // total level 0: full volume
        (0x80, 0x1F),     // attack rate 31: instant
        (0xA0, 0x0A),     // first decay
        (0xC0, 0x0A),     // second decay
        (0xE0, 0xFF),     // first-decay level max, release fast
    ] {
        a.ld_r_imm(Reg8::A, reg);
        a.alu_r(Alu::Add, Reg8::E);
        a.ld_r_r(Reg8::B, Reg8::A);
        a.ld_r_imm(Reg8::C, val);
        a.call("ym_write");
    }
    a.ld_r_r(Reg8::A, Reg8::E);
    a.alu_imm(Alu::Add, 0x08);
    a.ld_r_r(Reg8::E, Reg8::A);
    a.alu_imm(Alu::Cp, 0x20);
    a.jr_cc(Cond::Nz, "ym_op");
    a.ret();
}

/// `ym_write`: register number in B, value in C. Clobbers A only.
fn ym_write(a: &mut AsmZ80) {
    // ⚠️ **Two ports, and the order is the whole protocol.** A write to 0xF000
    // latches the register *address*; the next write to 0xF001 applies it.
    // Reversed, the value is latched as an address and the address written as a
    // value — configuring a register nobody chose, and the chip does not
    // complain.
    a.label("ym_write");
    a.ld_r_r(Reg8::A, Reg8::B);
    a.ld_abs_a(Z80_YM_ADDR);
    a.ld_r_r(Reg8::A, Reg8::C);
    a.ld_abs_a(Z80_YM_DATA);
    a.ret();
}

// ---- The OKI sample ROM ---------------------------------------------------

/// The `oki` region's size: 256 KB, the chip's whole address space.
const OKI_BYTES: usize = 0x4_0000;

/// The phrase the driver asks for.
///
/// Phrase 1 rather than 0: an all-zero ROM decodes as a request for phrase 0 with
/// a zero-length header, so starting at 1 keeps "the driver asked for a phrase"
/// distinguishable from "the driver asked for nothing".
const DEMO_PHRASE: u8 = 1;

/// The phrase table's size: 128 entries of 8 bytes.
const PHRASE_TABLE_BYTES: u32 = 128 * 8;

/// Where the demo's sample data starts, clear of the phrase table.
const SAMPLE_START: u32 = 0x1000;

/// The sample's length in bytes: two nibbles each, so twice this many samples.
const SAMPLE_BYTES: u32 = 0x0800;

/// The ADPCM sample ROM: one phrase header and a decaying tone.
///
/// A header is a 3-byte big-endian start then stop at `phrase * 8`, and the chip
/// refuses a phrase unless `start < stop`. The nibble count it plays is
/// `2 * (stop - start + 1)`.
fn okirom() -> Vec<u8> {
    let mut rom = vec![0u8; OKI_BYTES];
    let start = SAMPLE_START;
    let stop = SAMPLE_START + SAMPLE_BYTES - 1;
    assert!(
        start < stop,
        "the chip refuses a header that does not ascend"
    );
    // A sample overlapping its own header would play the header as audio — a
    // recognisable buzz and an unrecognisable bug.
    assert!(
        start >= PHRASE_TABLE_BYTES,
        "the sample must start clear of the phrase table"
    );
    let base = usize::from(DEMO_PHRASE) * 8;
    rom[base] = (start >> 16) as u8;
    rom[base + 1] = (start >> 8) as u8;
    rom[base + 2] = start as u8;
    rom[base + 3] = (stop >> 16) as u8;
    rom[base + 4] = (stop >> 8) as u8;
    rom[base + 5] = stop as u8;

    // ADPCM encodes a *delta*, so a constant nibble is a ramp and an alternating
    // pair is a tone: this alternates one positive and one negative step, with the
    // step shrinking across the sample so the note decays instead of ending in a
    // click. Bit 3 of a nibble is the sign and bits 2-0 the weight.
    for i in 0..SAMPLE_BYTES {
        let weight = 7 - (i * 8 / SAMPLE_BYTES) as u8;
        // High nibble first: the chip decodes bits 7-4 of a byte before bits 3-0,
        // so the rising step is the one in the high nibble.
        rom[(start + i) as usize] = (weight << 4) | 0x08 | weight;
    }
    rom
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every table the demo lays out satisfies its own alignment.
    ///
    /// A base register is `value * 256` truncated *down* to the table's boundary,
    /// so a misaligned table is not rejected — it moves onto whatever sits below
    /// it, and the picture reads as a tilemap bug rather than an address one.
    #[test]
    fn every_table_satisfies_its_alignment() {
        for (name, addr, boundary) in [
            ("palette", at::PALETTE, 0x400u32),
            ("obj", at::OBJ, 0x800),
            ("scroll1", at::SCROLL1, 0x4000),
            ("scroll2", at::SCROLL2, 0x4000),
            ("scroll3", at::SCROLL3, 0x4000),
        ] {
            assert_eq!(addr % boundary, 0, "{name} is not {boundary:#x}-aligned");
            assert_eq!(
                addr % 256,
                0,
                "{name} is not expressible in a base register"
            );
        }
    }

    /// The tables do not overlap, and all of them fit inside gfxram.
    ///
    /// Overlap is the failure worth catching: two tables sharing words draws one
    /// layer's codes as another's attributes, which is a mess that looks like a
    /// renderer bug.
    #[test]
    fn the_tables_do_not_overlap_and_fit_in_gfxram() {
        let spans = [
            ("palette", at::PALETTE, PALETTE_WORDS as u32 * 2),
            ("obj", at::OBJ, 0x800),
            ("scroll1", at::SCROLL1, MAP_CELLS * 4),
            ("scroll2", at::SCROLL2, MAP_CELLS * 4),
            ("scroll3", at::SCROLL3, MAP_CELLS * 4),
        ];
        for (name, addr, len) in spans {
            assert!(
                addr + len <= GFXRAM_BYTES,
                "{name} ends at {:#x}, past gfxram",
                addr + len
            );
        }
        for (i, &(an, aa, al)) in spans.iter().enumerate() {
            for &(bn, ba, bl) in &spans[i + 1..] {
                assert!(
                    ba >= aa + al || aa >= ba + bl,
                    "{an} at {aa:#x}+{al:#x} overlaps {bn} at {ba:#x}+{bl:#x}"
                );
            }
        }
    }

    /// Every tile code the demo uses is one STF29 can address.
    ///
    /// A code no range covers has no ROM behind it, and the tile is skipped. That
    /// failure is invisible: a blank layer looks exactly like a renderer that does
    /// not work.
    #[test]
    fn every_tile_code_is_one_the_mapper_covers() {
        assert!(
            try_map_code(Gfx::Scroll1, T1_BLANK).is_some(),
            "scroll 1's blank"
        );
        for d in 0..10 {
            assert!(
                try_map_code(Gfx::Scroll1, T1_DIGIT + d).is_some(),
                "digit {d}"
            );
        }
        assert!(try_map_code(Gfx::Scroll2, T2_FRAME).is_some(), "scroll 2");
        assert!(try_map_code(Gfx::Scroll3, T3_FRAME).is_some(), "scroll 3");
        assert!(try_map_code(Gfx::Sprite, SPR_DISC).is_some(), "the sprite");

        // And two codes outside every range, so the checks above are not vacuous:
        // a `try_map_code` that answered `Some` for everything would pass them
        // all.
        assert!(
            try_map_code(Gfx::Scroll1, 0x0000).is_none(),
            "scroll 1's range starts at 0x4000"
        );
        assert!(
            try_map_code(Gfx::Scroll3, 0x0000).is_none(),
            "scroll 3's starts at unit 0x2000"
        );
    }

    /// Every mapped tile lies wholly inside the graphics region.
    ///
    /// A tile that runs past the end of the ROM decodes as the transparent pen, so
    /// a region one tile short draws an invisible sprite rather than producing a
    /// diagnostic.
    #[test]
    fn every_mapped_tile_fits_the_graphics_region() {
        for (kind, code, bytes) in [
            (Gfx::Scroll1, T1_BLANK, 64usize),
            (Gfx::Scroll1, T1_DIGIT + 9, 64),
            (Gfx::Scroll2, T2_FRAME, 128),
            (Gfx::Scroll3, T3_FRAME, 512),
            (Gfx::Sprite, SPR_DISC, 128),
        ] {
            let tile = map_code(kind, code) as usize;
            assert!(
                (tile + 1) * bytes <= GFX_BYTES,
                "{kind:?} code {code:#x} maps to tile {tile}, ending past {GFX_BYTES:#x}"
            );
        }
    }

    /// The graphics region has non-zero bytes at every tile the demo draws.
    ///
    /// The assertion `graphics()` most needs. `map_code` and `gfx::framed` could
    /// each be right while disagreeing about where a tile goes, and the result is
    /// a region full of pixel data with nothing at the offsets the renderer reads.
    #[test]
    fn the_graphics_region_is_non_empty_at_every_tile_the_demo_draws() {
        let rom = graphics();
        assert_eq!(rom.len(), GFX_BYTES);
        for (name, kind, code, bytes) in [
            ("scroll 2's frame", Gfx::Scroll2, T2_FRAME, 128usize),
            ("scroll 3's frame", Gfx::Scroll3, T3_FRAME, 512),
            ("the sprite disc", Gfx::Sprite, SPR_DISC, 128),
            ("scroll 1's blank", Gfx::Scroll1, T1_BLANK, 64),
            ("digit 7", Gfx::Scroll1, T1_DIGIT + 7, 64),
        ] {
            let at = map_code(kind, code) as usize * bytes;
            assert!(
                rom[at..at + bytes].iter().any(|&b| b != 0),
                "{name} is blank at {at:#x}"
            );
        }
    }

    /// Scroll 2's tile is hollow and scroll 3's is filled, so both layers reach
    /// the screen.
    ///
    /// This pair of pens is the whole difference between a four-layer demo and a
    /// three-layer one. Scroll 2's grid covers every pixel of the frame, so an
    /// opaque interior there hides scroll 3 completely — and hides it in a way no
    /// counter can see: the register writes, the gfxram writes and the pixel count
    /// of a run with scroll 3 buried are identical to a correct one, and the
    /// picture reads as a deliberate single-layer background.
    ///
    /// The other half is asserted too. A transparent *fill* on scroll 3 would
    /// leave the back layer as a grid of hairlines on black, which is the same
    /// mistake pointed the other way.
    ///
    /// Pens are read back through [`gfx::read_pen`] — `video::tiles`' own rule,
    /// transcribed independently of the writer — rather than by inspecting the
    /// bytes, so this fails if the pens land on the wrong pixels as well as if
    /// they are the wrong pens.
    #[test]
    fn scroll_2_is_hollow_and_scroll_3_is_filled() {
        let rom = graphics();
        let s2 = map_code(Gfx::Scroll2, T2_FRAME);
        let s3 = map_code(Gfx::Scroll3, T3_FRAME);

        assert_eq!(
            gfx::read_pen(&rom, Kind::Tile16x16, s2, 8, 8),
            gfx::TRANSPARENT_PEN,
            "scroll 2's interior lets scroll 3 through"
        );
        assert_eq!(
            gfx::read_pen(&rom, Kind::Tile16x16, s2, 0, 0),
            S2_EDGE_PEN,
            "and its border is still drawn"
        );
        assert_eq!(
            gfx::read_pen(&rom, Kind::Tile16x16, s2, 15, 15),
            S2_EDGE_PEN,
            "on the far corner as well as the near one"
        );

        assert_eq!(
            gfx::read_pen(&rom, Kind::Tile32x32, s3, 16, 16),
            S3_FILL_PEN,
            "scroll 3's interior is opaque, so there is something to see through scroll 2"
        );
        assert_ne!(
            S3_FILL_PEN,
            gfx::TRANSPARENT_PEN,
            "and the pen it is filled with is not the transparent one"
        );
        assert_eq!(
            gfx::read_pen(&rom, Kind::Tile32x32, s3, 0, 0),
            S3_EDGE_PEN,
            "with its own border in its own pen"
        );
    }

    /// The ten digits land in ten different places in the ROM.
    ///
    /// Scroll 1 has the only mapper shift of zero, so its codes index frames
    /// one-for-one. If a wrong shift folded ten codes onto fewer frames the
    /// counter would still draw digits — the same digit, in every column.
    #[test]
    fn the_ten_digits_occupy_ten_distinct_frames() {
        let mut seen = Vec::new();
        for d in 0..10u32 {
            let tile = map_code(Gfx::Scroll1, T1_DIGIT + d);
            assert!(!seen.contains(&tile), "digit {d} shares a frame");
            seen.push(tile);
        }
        assert!(
            !seen.contains(&map_code(Gfx::Scroll1, T1_BLANK)),
            "the blank tile shares a frame with a digit"
        );
    }

    /// The palette has a colour at every scheme the demo's tiles resolve to.
    ///
    /// Four schemes at four widely separated offsets, and all four are asserted: a
    /// palette written at one base only would light one layer and leave the rest
    /// black, which reads as three broken layers.
    #[test]
    fn the_palette_has_an_entry_for_every_layer() {
        let pal = palette_words();
        assert_eq!(pal.len(), PALETTE_WORDS);
        for (name, scheme, pen) in [
            ("the sprite", SPRITE_SCHEME, SPRITE_PEN),
            ("scroll 1's digits", SCROLL1_SCHEME, DIGIT_PEN),
            ("scroll 2's border", SCROLL2_SCHEME, S2_EDGE_PEN),
            ("scroll 3's border", SCROLL3_SCHEME, S3_EDGE_PEN),
            ("scroll 3's fill", SCROLL3_SCHEME, S3_FILL_PEN),
        ] {
            let entry = pal[scheme * PEN_GRANULARITY + usize::from(pen)];
            assert_ne!(entry, 0, "{name} has no colour");
            // Full brightness on every one: bits 12-15 at 15 is unity and 0 is
            // about a third, which is easy to ship and hard to notice.
            assert_eq!(entry >> 12, 0x0F, "{name} is not at full brightness");
        }
        // And nothing was written at the transparent pen of any scheme, which
        // would be a colour nobody can see and a sign the pen numbering slipped.
        for scheme in [
            SPRITE_SCHEME,
            SCROLL1_SCHEME,
            SCROLL2_SCHEME,
            SCROLL3_SCHEME,
        ] {
            assert_eq!(
                pal[scheme * PEN_GRANULARITY + usize::from(gfx::TRANSPARENT_PEN)],
                0,
                "scheme {scheme:#x} has a colour at the transparent pen"
            );
        }
    }

    /// The object table's marker sits in record 2, so record 0 draws.
    ///
    /// The scan answers `offset - 4`, which skips the record *before* the marker as
    /// well. A marker in record 1 — the intuitive place for it — draws nothing at
    /// all.
    #[test]
    fn the_sprite_records_survive_the_end_marker_rule() {
        let w = obj_words();
        assert_eq!(w.len(), OBJ_INIT_WORDS, "setup copies exactly this many");
        assert_eq!(w[3] & 0xFF00, 0, "record 0 is not itself a marker");
        assert_eq!(w[7] & 0xFF00, 0, "nor is record 1");
        assert_eq!(w[11] & 0xFF00, 0xFF00, "record 2 is the marker");
        assert_eq!(w[2], SPR_DISC as u16, "record 0 draws the disc");
        assert_eq!(w[6], SPR_DISC as u16, "and so does record 1");
        // The block-dimension fields are the same bits as the end marker — 0x0F00
        // and 0xF000 together are 0xFF00 — so the three assertions above are also
        // the assertion that each drawable record is a single 16×16 tile. A
        // multi-tile record would ask for a grid of codes the ROM does not have,
        // and one of them *is* the marker value.
        assert_eq!(w[3], 0x0000, "record 0 is one tile, no flips, scheme 0");
    }

    /// The sprite's path moves in both axes and stays wholly on screen.
    ///
    /// Both halves matter. A path that moved in one axis only would be produced
    /// equally well by a bug that wrote one coordinate into both registers, and a
    /// path that left the frame would look like a sprite that vanishes — the
    /// blitter clips, it does not wrap.
    #[test]
    fn the_sprite_path_moves_in_both_axes_and_stays_visible() {
        let w = path_words();
        assert_eq!(w.len() as u32, PATH_STEPS * 2);
        let xs: Vec<u16> = w.iter().step_by(2).copied().collect();
        let ys: Vec<u16> = w.iter().skip(1).step_by(2).copied().collect();
        assert!(xs.iter().any(|&x| x != xs[0]), "x varies along the path");
        assert!(ys.iter().any(|&y| y != ys[0]), "y varies along the path");
        // And the two axes are not the same sequence, which is what one coordinate
        // written twice would look like.
        assert_ne!(xs, ys, "the axes are independent");
        let edge = SPRITE_EDGE as u16;
        for (i, (&x, &y)) in xs.iter().zip(&ys).enumerate() {
            assert!(
                x >= VISIBLE_X && x + edge <= VISIBLE_X + WIDTH,
                "step {i}: x {x} puts the disc off screen"
            );
            assert!(
                y >= VISIBLE_Y && y + edge <= VISIBLE_Y + HEIGHT,
                "step {i}: y {y} puts the disc off screen"
            );
        }
        // The path is closed: the step from the last entry back to the first is no
        // longer than the longest step within it, so the sprite does not jump when
        // the index wraps. Comparing `path_step(0)` with `path_step(PATH_STEPS)`
        // would assert nothing — `tri` takes its phase modulo the period, so those
        // two are the same call.
        let step = |a: usize, b: usize| {
            let dx = i32::from(xs[a]) - i32::from(xs[b]);
            let dy = i32::from(ys[a]) - i32::from(ys[b]);
            dx * dx + dy * dy
        };
        let last = xs.len() - 1;
        let longest = (1..=last).map(|i| step(i, i - 1)).max().expect("a path");
        assert!(
            step(0, last) <= longest,
            "the wrap from step {last} to step 0 is a jump"
        );
    }

    /// The counter's cells are on screen, and each digit gets its own.
    ///
    /// Scroll 1's map is 64 columns of 8 pixels and the visible frame starts at
    /// raster (64, 16), so the top-left visible cell is (8, 2). A counter written
    /// at column 0 is correct, invisible, and indistinguishable from one that
    /// never updates.
    #[test]
    fn the_counters_cells_are_inside_the_visible_frame() {
        let mut seen = Vec::new();
        assert!(
            COUNTER_ROW >= u32::from(VISIBLE_Y) / 8,
            "row {COUNTER_ROW} is above the frame"
        );
        assert!(
            COUNTER_ROW < (u32::from(VISIBLE_Y) + u32::from(HEIGHT)) / 8,
            "row {COUNTER_ROW} is below the frame"
        );
        for i in 0..4u32 {
            let col = COUNTER_COL + i;
            assert!(
                col >= u32::from(VISIBLE_X) / 8,
                "column {col} is off the left"
            );
            assert!(
                col < (u32::from(VISIBLE_X) + u32::from(WIDTH)) / 8,
                "column {col} is off the right"
            );
            let at = scroll1_cell_bytes(col, COUNTER_ROW);
            assert!(!seen.contains(&at), "column {col} reuses another's cell");
            assert!(at + 4 <= MAP_CELLS * 4, "cell {at:#x} is past the table");
            seen.push(at);
        }
        // Consecutive columns are a whole 128 bytes apart under scroll 1's scan
        // mapper — the column is the *outer* index. A generator that assumed
        // adjacent cells were adjacent words would write all four digits into one
        // column's neighbourhood.
        assert_eq!(seen[1] - seen[0], 128, "scroll 1's column stride");
    }

    /// The counter wraps before it needs a fifth digit.
    ///
    /// Not cosmetic: the handler divides by 1000 first, and a five-digit frame
    /// gives a two-digit quotient there, of which only the low digit is drawn. The
    /// thousands column would count 0-9 out of step with the rest, which looks
    /// like a font or tile-code bug.
    #[test]
    fn the_counter_wraps_before_it_needs_a_fifth_digit() {
        assert_eq!(COUNTER_WRAP, 10_000);
        for frame in [0u16, 1, 999, COUNTER_WRAP - 1] {
            assert!(frame / 1000 < 10, "{frame} needs a fifth column");
        }
    }

    /// The layer-control word enables three layers and orders four depths.
    ///
    /// Every field is asserted separately, because the fields are adjacent 2-bit
    /// slots: an off-by-one shift moves the sprites behind everything and enables
    /// a layer at the same time, and the result is a picture that is merely wrong
    /// rather than absent.
    #[test]
    fn the_layer_control_word_orders_the_four_depths() {
        assert_eq!(LAYER_CONTROL & 0x38, 0x38, "scrolls 1, 2 and 3 all enabled");
        let field = |bit: u32| (LAYER_CONTROL >> bit) & 3;
        // Back to front.
        assert_eq!(field(6), 3, "scroll 3 at the back");
        assert_eq!(field(8), 2, "then scroll 2");
        assert_eq!(field(10), 0, "then the sprites");
        assert_eq!(field(12), 1, "scroll 1 in front");
        // And the four depths name four different layers, so nothing is drawn
        // twice while another layer is never drawn at all.
        let mut drawn: Vec<u16> = [6, 8, 10, 12].iter().map(|&b| field(b)).collect();
        drawn.sort_unstable();
        assert_eq!(drawn, [0, 1, 2, 3], "each depth draws a different layer");
    }

    /// The program fits its ROM space, the vblank vector is in the right slot, and
    /// no vector points at zero.
    ///
    /// The vector table is the part where a mistake does not fault. A vector left
    /// at zero sends its exception to address 0, which on this board holds the
    /// reset stack pointer, and the 68000 executes it.
    #[test]
    fn the_vector_table_is_filled_and_the_vblank_vector_is_at_0x68() {
        let rom = maincpu();
        assert_eq!(rom.len(), MAINCPU_BYTES);
        let long = |at: usize| u32::from_be_bytes([rom[at], rom[at + 1], rom[at + 2], rom[at + 3]]);
        assert_eq!(long(0), STACK_TOP, "the reset stack pointer");
        let start = long(4);
        assert!(start > VEC_AUTOVECTOR_2, "the reset PC is past the vectors");
        assert_eq!(start % 2, 0, "an odd PC is an address error at reset");
        for at in (8..VEC_AUTOVECTOR_2 as usize).step_by(4) {
            assert_ne!(long(at), 0, "vector at {at:#x} would execute address 0");
        }
        let vblank = long(VEC_AUTOVECTOR_2 as usize);
        assert_ne!(vblank, 0, "the vblank vector");
        assert_ne!(vblank, long(8), "and it is not the spurious handler");
        // The program itself, and then zeros. `finish` would have panicked on an
        // unresolved label before this ran.
        let used = rom.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        assert!(used > VEC_AUTOVECTOR_2 as usize, "there is a program");
        assert!(used < MAINCPU_BYTES, "and it fits with room to spare");
    }

    /// The sound driver fits the fixed region, and the region is the size the
    /// board maps.
    ///
    /// Reaching past 0x8000 would put code in the banked window, which this driver
    /// never switches — so the Z80 would run into 0x00 bytes and execute `nop`
    /// until it wrapped.
    #[test]
    fn the_sound_driver_fits_the_fixed_region() {
        let rom = audiocpu();
        assert_eq!(rom.len(), AUDIOCPU_BYTES);
        let used = rom.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        assert!(used > 0, "the driver is not empty");
        assert!(
            used <= FIXED_ROM_BYTES,
            "{used} bytes reaches the bank window"
        );
        // Nothing in the banked window, which is what makes the claim above worth
        // asserting: a driver that had put data there would still pass on length.
        assert!(
            rom[FIXED_ROM_BYTES..].iter().all(|&b| b == 0),
            "the banked window is not empty"
        );
    }

    /// The OKI phrase is one the chip accepts, and its samples are not silence.
    ///
    /// The chip refuses `start >= stop`, and it plays silence for a phrase whose
    /// nibbles are all zero — two different silences with the same symptom, so
    /// both are asserted.
    #[test]
    fn the_oki_phrase_is_playable_and_its_samples_are_not_silence() {
        let rom = okirom();
        assert_eq!(rom.len(), OKI_BYTES);
        let base = usize::from(DEMO_PHRASE) * 8;
        let read24 = |a: usize| {
            (u32::from(rom[a]) << 16) | (u32::from(rom[a + 1]) << 8) | u32::from(rom[a + 2])
        };
        let start = read24(base);
        let stop = read24(base + 3);
        assert_eq!(start, SAMPLE_START);
        assert_eq!(stop, SAMPLE_START + SAMPLE_BYTES - 1);
        assert!(start < stop, "the chip's own acceptance test");
        assert_eq!(
            2 * (stop - start + 1),
            2 * SAMPLE_BYTES,
            "the nibble count the chip will play"
        );
        assert!(
            start >= PHRASE_TABLE_BYTES,
            "the sample is clear of the phrase table"
        );
        let data = &rom[start as usize..=stop as usize];
        assert!(data.iter().any(|&b| b != 0), "the phrase has audio in it");
        // And the steps alternate sign: bit 3 of a nibble is the sign, so a byte
        // whose two nibbles share it is a ramp, not a tone.
        for (i, &b) in data.iter().enumerate() {
            assert_ne!(b >> 4 & 0x08, b & 0x08, "byte {i} steps the same way twice");
        }
        // Phrase 0's header is untouched, so a driver that asked for the wrong
        // phrase gets silence rather than this sample under another number.
        assert!(rom[..8].iter().all(|&b| b == 0), "phrase 0 has a header");
    }

    /// `build` answers the four regions a CPS-1 machine needs, in order and each
    /// at the size the board maps.
    #[test]
    fn build_answers_the_four_regions_at_the_sizes_the_board_maps() {
        let regions = build();
        let names: Vec<&str> = regions.iter().map(|&(n, _)| n).collect();
        assert_eq!(names, ["maincpu", "gfx", "audiocpu", "oki"]);
        let sizes: Vec<usize> = regions.iter().map(|(_, b)| b.len()).collect();
        assert_eq!(
            sizes,
            [MAINCPU_BYTES, GFX_BYTES, AUDIOCPU_BYTES, OKI_BYTES],
            "each region is exactly the mapped size"
        );
    }
}
