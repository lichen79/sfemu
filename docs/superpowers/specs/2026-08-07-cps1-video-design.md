# Design: CPS-1 Video — tiles, sprites, palette (sfemu sub-project C)

Date: 2026-08-07
Status: Approved (design calls made autonomously under a standing instruction to
proceed without check-ins; every call and its rationale is recorded inline)
Scope: Sub-project C of the sfemu arcade emulator

## Context

Sub-project A delivered `crates/m68k`, verified at 127/127 groups and
317,500/317,500 cases of the SingleStepTests/m68000 vector suite. Sub-project B
delivered `crates/romset` and `crates/machine`: a memory map, a ROM-set loader, a
scanline scheduler, and the vblank interrupt. B's deliverable was a *trace* — a
count of vblanks, acknowledges, and register writes — because B renders nothing.

C renders. It turns the state B already stores (gfxram, the CPS-A register file,
the CPS-B register file) plus the `gfx` ROM region B already loads into a
384×224 image.

### The ROM constraint, restated

**No ROM is bundled, fetched, downloaded, or committed, by any code in this
repository, for any purpose including diagnostics and test fixtures.** This is
unchanged from A and B and has no exemption for graphics data. Every automated test
in C runs on synthetic tile and palette data this repository generates. The one
test that needs a real set is opt-in on a user-supplied path.

### The reference

MAME `master` read 2026-08-07 from `/tmp/mameref/{cps1.cpp,cps1.h,cps1_v.cpp}`
(BSD-3-Clause, copyright-holders Paul Leaman). We read MAME as hardware
documentation and reimplement; we do not translate its code. See
`docs/hardware/cps1-notes.md` § "Getting the reference back" — the files live in a
scratch directory and the line numbers are pinned to that date.

---

## The verification problem, and the answer

**This is the defining constraint on C's design, and it is worth stating before the
architecture.**

A had 317,500 external vector cases. B had synthetic 68000 programs whose expected
cycle counts and register writes are computable by hand, plus MAME's source. **C has
neither an external corpus nor a hand-computable output** in the way B did: nobody
publishes "expected CPS-1 framebuffers", and the natural way to check a renderer —
look at it — is not a test. "It looks like Street Fighter" is precisely the
self-confirming judgement this branch has spent twenty-four tasks learning to
distrust.

Three things make C checkable anyway, and the plan is built on them:

1. **Hand-encoded synthetic tiles with hand-computed expected pixels.** The tile bit
   layout (below) is arithmetic. A test writes a tile's bytes as literals, states
   the 8×8 grid of pen values it must decode to as literals, and compares. Neither
   side is derived from the other.

   ⚠️ **The trap here is an encoder helper.** A test that calls `encode_tile(pixels)`
   and then asserts `decode_tile(encoded) == pixels` passes for *any* pair of
   mutually inverse functions, including two that are both wrong about where plane 3
   lives. Encoders are therefore banned from C's tests unless the encoder's own
   output is separately pinned against a literal byte array. This is the same defect
   as B's `assert_eq!(a / b, a / b)`, wearing a graphics costume.

2. **Each stage is separately observable.** The pipeline is
   gfx ROM → tile pixels → pen indices → RGB. Rendering to **pen indices** and
   keeping the palette a separate array means a tile-placement bug and a
   brightness-math bug cannot be confused for one another: they fail different
   tests. A renderer that only ever produced RGB would need every test to be right
   about both at once.

3. **Structural invariants that do not need a reference image.** The scan mappers
   must be bijections over their domains; the three layers must tile the screen
   without gaps; a full-screen opaque layer must leave no background pen visible; a
   layer that is disabled must change nothing. These are checkable exactly, and they
   are what catch the off-by-one that a reference image would show as "slightly
   wrong somewhere".

**What this does not give us.** It does not prove SF2 looks correct. Nothing
available to this repository can. The opt-in real-ROM test therefore grows one new
assertion — that a rendered attract-mode frame is not uniform and draws from
several palette pages — and the honest limit is written into
`docs/hardware/cps1-video-notes.md` rather than papered over. A framebuffer PPM dump
(§Deliverables) exists so a human *can* look, but no test asserts on what they see.

---

## Scope

**In.** The palette and its brightness math; the three scroll layers (8×8, 16×16,
32×32) with scroll offsets, per-tile flip, and row scroll; sprites from the
one-frame-delayed object RAM, including blocked (multi-tile) sprites; layer ordering
and enable bits from CPS-B; the high-priority-tile-over-sprite mask; the gfx ROM
bank mapper; screen flip; a 384×224 framebuffer in pen indices and in RGB.

**Out, with reasons.**

- **Stars.** Two star layers exist (`CPS1_STARS1_SCROLLX` onward) and
  `cps1_render_stars` draws them, but only when a `stars` ROM region is present
  (`cps1_v.cpp:3045`, `if (m_region_stars)`). SF2's `ROM_START` has no such region,
  so on this board the code is unreachable. Deferred to F, which may add a game that
  uses them.
- **Bootleg kludges.** `bootleg_kludge` selects per-set scroll offsets and an
  alternate sprite end-marker. SF2's table row sets it to 0. Modelling paths no
  supported set takes would be untestable code.
- **The multiply protection.** `CPS_B_11` expands it to `__not_applicable__`
  (`cps1_v.cpp:491`), so SF2's board has none.
- **The window.** Sub-project E owns it. C's output is a buffer; nothing in C opens
  a display. This is deliberate: a renderer that can only be observed through a
  window can only be tested by looking.

---

## Architecture

A new crate, `crates/video`, **dependency-free** like `m68k` — no `std` requirement
beyond `alloc` for the framebuffer. `machine` depends on it; `video` depends on
nothing and knows nothing about `Board`, `Bus`, or the 68000.

That direction is the whole point. `video::render` takes borrowed slices —
`&[u16]` gfxram, `&[u16]` CPS-A, `&[u16]` CPS-B, `&[u8]` gfx ROM — so its tests
construct those four things directly, with no machine, no ROM set, and no CPU. A
renderer reachable only through `Cps1` would need a booted machine to test one tile.

```
crates/video/src/
  lib.rs        Video, render entry point, the framebuffer
  regs.rs       CPS-A register indices, cps1_base, VideoConfig (the CPS-B offsets)
  tiles.rs      the four gfx layouts: (code, x, y) -> pen
  bank.rs       gfxrom_bank_mapper and the STF29 range table
  palette.rs    gfxram -> pen -> RGB, and the brightness math
  layers.rs     the three tilemaps: scan mappers, tile fetch, scroll, flip
  sprites.rs    the object table, blocking, the one-frame delay
  compose.rs    layer order, enables, priority mask, the background fill
```

### The framebuffer

```rust
pub const WIDTH: usize = 384;
pub const HEIGHT: usize = 224;

pub struct Framebuffer {
    /// Pen indices, 0..0xC00. Row-major, `WIDTH * HEIGHT`.
    pub pens: Box<[u16]>,
    /// Per-pixel priority, used to let high-priority tile pens occlude sprites.
    pub prio: Box<[u8]>,
}
```

`pens` and not RGB, for the reason in §Verification. `Video::rgb()` converts on
demand for the PPM dump and for E's window.

### Wiring into `machine`

`Cps1` gains `pub video: Video`. `Cps1::new` keeps its current three-argument
signature and constructs a `Video` with an **empty** gfx region, so every one of
B's existing tests compiles and passes unchanged; `Cps1::with_gfx` adds the region.
A renderer with no gfx ROM draws the background pen and nothing else, which is the
correct behaviour for the synthetic-program tests B already has.

---

## The hardware facts C is built on

Every figure below was read from the cited line, not recalled.

### CPS-A registers (`cps1.h:175-193`)

Byte offsets from 0x800100. MAME's constants are **already divided by two** because
its array is `uint16_t` — `CPS1_SCROLL1_SCROLLX = 0x0c / 2`. The word index is what
indexes `cps_a`; the byte offset is what a 68000 program writes. Writing the `/2` at
every boundary is a rule inherited from B, where mixing them shifted the register
file by one entry and every value looked plausible in the wrong slot.

| Word | Byte | Name |
|---|---|---|
| 0 | 0x00 | `OBJ_BASE` |
| 1 | 0x02 | `SCROLL1_BASE` |
| 2 | 0x04 | `SCROLL2_BASE` |
| 3 | 0x06 | `SCROLL3_BASE` |
| 4 | 0x08 | `OTHER_BASE` (row scroll table) |
| 5 | 0x0A | `PALETTE_BASE` |
| 6, 7 | 0x0C, 0x0E | scroll1 X, Y |
| 8, 9 | 0x10, 0x12 | scroll2 X, Y |
| 10, 11 | 0x14, 0x16 | scroll3 X, Y |
| 12-15 | 0x18-0x1E | stars (out of scope) |
| 16 | 0x20 | `ROWSCROLL_OFFS` |
| 17 | 0x22 | `VIDEOCONTROL` — bit 0 row scroll, bit 2 scroll2 enable, bit 3 scroll3 enable, bit 15 flip screen |

### `cps1_base` (`cps1_v.cpp:2099-2113`)

```
base = cps_a[reg] * 256
base &= !(boundary - 1)      // "scroll RAM must start on a 0x4000 boundary"
word_index = (base & 0x3FFFF) / 2
```

Boundaries: scroll1/2/3 `0x4000`, obj `0x800`, other `0x800`, palette `0x400`
("minimum alignment is a single palette page (512 colors). Verified on pcb",
`cps1_v.cpp:2541`).

The `& !(boundary-1)` is not tidiness: MAME notes games that fail to align, naming
Captain Commando's continue screen.

⚠️ **The `& 0x3FFFF` does not keep the index inside gfxram.** It bounds the result
to a **256 KB** window, and gfxram is 192 KB — so a register above 0xDFFF resolves
to a word index between 0x18000 and 0x1FE00, past the end of the array. MAME has the
same gap: `cps1_base` returns a pointer into a `required_shared_ptr` with no bounds
check. Every read through one of these bases therefore wraps with `% gfxram.len()`,
and the alternative — clamping in `cps_a_base` — is rejected because it would
silently relocate a table the guest asked for. The plan's
`cps_a_base_can_point_past_gfxram_so_callers_must_wrap` sweeps all 65,536 register
values against all three boundaries and pins 0x1FE00 as the worst index, so a later
reader cannot delete a wrap believing it redundant.

Power-on defaults (`cps1_v.cpp:2565-2569`): OBJ 0x9200, SCROLL1 0x9000,
SCROLL2 0x9040, SCROLL3 0x9080, OTHER 0x9100.

### Tile pixel layout (`cps1.cpp:3837-3878`)

Four `gfx_layout`s, 4 bits per pixel, planes at bit offsets `{24, 16, 8, 0}` with
plane 0 the **most** significant bit of the pen. Bits are numbered MSB-first within
each byte, MAME's convention.

Unified rule. A tile lives in a storage frame `FW` pixels wide — 16 for the 8×8 and
16×16 layouts, 32 for 32×32 — and the bit index of pixel `(x, y)`, plane `p`, is:

```
bit(x, y, p) = y * (4 * FW)  +  32 * (x >> 3)  +  (x & 7)  +  plane_offset[p]
plane_offset = [24, 16, 8, 0]   // p = 0 is the pen's bit 3
```

which means each group of 8 horizontal pixels occupies 4 consecutive bytes, one per
plane, and a row of the frame is `FW / 2` bytes.

| Layout | Size | Frame | Bytes/tile | Used by | x base |
|---|---|---|---|---|---|
| `cps1_layout8x8` | 8×8 | 16 | 64 | scroll1, even columns | 0 |
| `cps1_layout8x8_2` | 8×8 | 16 | 64 | scroll1, odd columns | +32 bits |
| `cps1_layout16x16` | 16×16 | 16 | 128 | scroll2, sprites | 0 |
| `cps1_layout32x32` | 32×32 | 32 | 512 | scroll3 | 0 |

The two 8×8 layouts differ **only** in x base: `STEP8(0,1)` versus `STEP8(32,1)`.
`get_tile0_info` picks between them with `gfxset = BIT(tile_index, 5)`, and under
`tilemap0_scan` bit 5 of the tile index is the column's bit 0 — so scroll1's columns
alternate between the left and right half of each 16-pixel-wide storage block.
MAME's comment (`cps1_v.cpp:2462-2464`) records that this was found via a Final
Fight board with mixed-region ROMs.

The 64-byte 8×8 unit is why the bank mapper shifts codes per layer type: 16×16 is
two units (shift 1), 32×32 is eight (shift 3).

### The tilemaps (`cps1_v.cpp:2452-2510`, `:2433-2450`, `:2546-2548`)

All three are 64×64 tiles. Each entry is **two words** at `2 * tile_index`: a code
and an attribute.

```
attr & 0x001F   colour scheme, plus a per-layer base
attr & 0x0020   X flip        (TILE_FLIPYX((attr & 0x60) >> 5))
attr & 0x0040   Y flip
attr & 0x0180   priority group, >> 7
```

| Layer | Tile | Colour base | Code mask | Scan mapper: (col,row) → tile index |
|---|---|---|---|---|
| scroll1 | 8×8 | `+0x20` | — | `(row & 0x1F) + ((col & 0x3F) << 5) + ((row & 0x20) << 6)` |
| scroll2 | 16×16 | `+0x40` | — | `(row & 0x0F) + ((col & 0x3F) << 4) + ((row & 0x30) << 6)` |
| scroll3 | 32×32 | `+0x60` | `& 0x3FFF` | `(row & 0x07) + ((col & 0x3F) << 3) + ((row & 0x38) << 6)` |

A pen is `colour * 16 + pixel`, so the bases separate the layers: sprites take
schemes 0x00-0x1F, scroll1 0x20-0x3F, scroll2 0x40-0x5F, scroll3 0x60-0x7F.

**Why 16 and not 32.** The multiplier is MAME's `gfx_element` granularity, which
for a 4-bit-per-pixel layout is `1 << 4` = 16 — not the 32 that
`m_palette_size = CPS1_PALETTE_ENTRIES * 32` (`cps1_v.cpp:2542`) invites, that
32 being bytes per scheme in gfxram, two per entry. Four checks agree:

- `GFXDECODE_ENTRY(..., 0, 0x80)` (`cps1.cpp:3882-3885`) gives 0x80 colour codes,
  and 0x80 × 16 = 0x800 — exactly the four layers' 0x20 schemes each.
- The star layers write pens `0x800 + col` and `0xa00 + col`
  (`cps1_v.cpp:2900`, `:2926`), which sit immediately above that 0x800. At a
  granularity of 32 the tile pens would run to 0x1000 and swallow them.
- `set_entries(0xc00)` (`cps1.cpp:3932`) is the palette's real size, and
  0x800 tile pens + 0x400 star pens = 0xC00.
- [`BACKGROUND_PEN`] 0xBFF is the last of those 0xC00, in the star region rather
  than the tile region — which is why nothing a tilemap draws can equal it, and
  the "a solid layer hides the background" invariant is meaningful.

So the highest pen any tilemap can produce is `(0x1F + 0x60) * 16 + 15` = 0x7FF.

The mappers are column-major within a vertical strip, which is what makes them worth
testing as bijections rather than transcribing and hoping: each is a permutation of
`0..0xFFF`, and an off-by-one in a shift or a mask breaks that property loudly.

### Palette (`cps1_v.cpp:2612-2646`)

Six pages of 0x200 entries, copied from gfxram at `PALETTE_BASE`. Page `n` is copied
only if bit `n` of `cps_b[palette_control / 2]` is set, and **skipped pages compact**:
if page 0 is disabled, page 1's data comes from the *first* 0x200 words, not the
second — but only once at least one page has been copied. That asymmetry is in
MAME's source and its comment ("if the first palette pages are skipped, all the
following pages are scaled down"); it is exactly the sort of clause a
reimplementation drops silently, so it gets its own test.

Each entry is `bbbb` in bits 0-3, `gggg` in 4-7, `rrrr` in 8-11, brightness in
12-15:

```
bright = 0x0F + ((entry >> 12) << 1)
r = ((entry >> 8) & 0x0F) * 0x11 * bright / 0x2D
g = ((entry >> 4) & 0x0F) * 0x11 * bright / 0x2D
b = ((entry >> 0) & 0x0F) * 0x11 * bright / 0x2D
```

Integer division, truncating. `0x11` scales a nibble to 8 bits (0x0F → 0xFF) and
`bright / 0x2D` scales by brightness, where 0x2D = 45 = the maximum `bright`
(0x0F + 15×2). So brightness 15 is unity and brightness 0 gives `0x0F/0x2D` ≈ 1/3 —
MAME's comment reads "from my understanding of the schematics, when the 'brightness'
component is set to 0 it should reduce brightness to 1/3".

Total pens `6 * 0x200 = 0xC00`. The background fill is pen **0xBFF**, the last pen
of the last page (`cps1_v.cpp:3041`): "Games use pen 0xbff as background color".

### Sprites (`cps1_v.cpp:2724-2861`, table format at `:2652-2680`)

Eight bytes per sprite: `x, y, code, attr` as four words.

```
attr & 0x001F   colour
attr & 0x0020   X flip
attr & 0x0040   Y flip
attr & 0x0F00   X block size - 1, in 16-pixel sprites
attr & 0xF000   Y block size - 1
```

Positions mask to `& 0x1FF`. Pen 15 is transparent (`prio_transpen(..., 15)`).

Two facts a naive reimplementation misses:

- **The object table is delayed one frame.** `cps1_objram_latch` copies 0x800 bytes
  out of gfxram at vblank (`cps1_v.cpp:3063-3070`, "CPS1 sprites have to be delayed
  one frame"). Rendering live objram puts every sprite one frame ahead of its
  layers, which reads as jitter rather than as a bug.
- **The table is scanned to an end marker, backwards.** `find_last_sprite` walks
  forward for an attribute word with `(attr & 0xFF00) == 0xFF00` and sets the last
  offset to **four words before** it; the render loop then walks *downward* from
  there, so later table entries draw first and earlier ones on top. With no marker
  found, the whole 0x800-byte table is used.

  Two consequences of that `offset - 4` that are easy to get wrong. A marker in
  record 2 leaves records **1 and 0** drawable, not records 0-1 plus the record
  before the marker — the record immediately preceding the marker is skipped. And a
  marker in record **0** gives −4, which MAME holds in a signed `int` so its
  `i >= 0` loop draws nothing at all; the reimplementation returns
  `Option<usize>` and `None` for that case, because a `saturating_sub` to 0 would
  draw the very record the marker declares is not a sprite.

Blocked sprites tile a `nx × ny` grid of 16×16 tiles from a base code, and the code
arithmetic wraps within the low nibble: `(code & ~0xF) + ((code + nxs) & 0xF) +
0x10 * nys`, with the x term counting down as `(code + (nx - 1) - nxs)` under X flip
and rows as `0x10 * (ny - 1 - nys)` under Y flip. That wrap is why a block crossing a
16-code boundary repeats rather than running on.

⚠️ **The bank mapper runs once on the base code, before the block arithmetic.**
`cps1_v.cpp:2764` maps the code and `:2766` gates the whole record on
`code != -1`; every tile of the block is then derived from the *mapped* value. A
reimplementation that mapped each block tile separately would produce different
codes wherever a range boundary falls inside a block, and would drop individual
tiles rather than the whole sprite.

### Row scroll, and screen flip (`cps1_v.cpp:3017-3033`, `:3005`)

Two facts here had to be **derived** rather than transcribed, because MAME states
both in coordinate systems this design does not use. Each is recorded with its
derivation because in both cases the obvious reading of MAME's line is wrong.

**Row scroll does not shift with the vertical scroll.** With `VIDEOCONTROL` bit 0
set, MAME writes

```c
for (int i = 0; i < 256; i++)
    tilemap[1]->set_scrollx((i - scrly) & 0x3ff,
                            scrollx[1] + other[(i + otheroffs) & 0x3ff]);
```

where `scrly = -scrolly[1]`. That row index is a **tilemap** row. A tilemap
scrolled by `scrolly` shows tilemap row `y + scrolly` at screen row `y`, so screen
row `y` reads the entry the loop wrote at `t = y + scrolly = i - scrly = i + scrolly`
— which gives `i = y`, and in screen coordinates

```
x[y] = scrollx[1] + other[(y + otheroffs) & 0x3FF]
```

with **no `scrolly` term**. The `- scrly` exists precisely to cancel the tilemap's
own vertical scroll. Writing `(y + scrolly + otheroffs)` — the obvious translation —
would shear every row-scrolled layer as it scrolled vertically. Row scroll applies to
**scroll 2 only**: `tilemap[1]` is the 16×16 map (`cps1_v.cpp:2547`), and scrolls 1
and 3 take a single `set_scrollx` at `:3015` and `:3035`.

**One mirror of the finished frame is equivalent to MAME's per-primitive flip.**
MAME sets a global flip flag (`cps1_v.cpp:3005`, `VIDEOCONTROL` bit 15) and each
sprite blit then uses `512 - 16 - sx`, `256 - 16 - sy` with its own flip bits
inverted. Those are the same transform: a 16-pixel sprite at `[sx, sx+15]` mirrored
about `511 - p` spans `[496 - sx, 511 - sx]`, whose left edge is `512 - 16 - sx`, and
vertically `[sy, sy+15]` about `255 - p` gives `256 - 16 - sy`. The 512 and 256 are
mirror pivots plus one, not screen dimensions.

The mirror commutes with the crop because the visible window is symmetric within
those pivots: HBEND 64 and HBSTART−1 447 sum to 511, VBEND 16 and VBSTART−1 239 sum
to 255 (`cps1.h:41-47`). So this design flips the finished 384×224 buffer in one pass
rather than threading a flip flag through every blit.

### Layer order and priority (`cps1_v.cpp:2970-2999`, `:2515-2531`)

`layercontrol = cps_b[layer_control / 2]`, and four 2-bit fields select what is drawn
at each of four depths, back to front:

```
l0 = (layercontrol >> 6)  & 3
l1 = (layercontrol >> 8)  & 3
l2 = (layercontrol >> 10) & 3
l3 = (layercontrol >> 12) & 3
```

Value 0 means sprites, 1-3 mean scroll1-3. Enables come from
`layer_enable_mask[0..2]` ANDed with `layercontrol`, and scroll2/scroll3
additionally require `VIDEOCONTROL` bits 2 and 3.

Above each sprite layer, tile pens flagged high-priority occlude sprites. For
priority group `i`, the pens that occlude are those set in
`cps_b[priority[i] / 2]` — MAME computes `mask = reg ^ 0xFFFF` and hands it to
`set_transmask` as the pens transparent in the foreground pass, which is the same
statement inverted twice. C models it directly as "pen `p` of group `i` occludes
sprites iff bit `p` of `cps_b[priority[i]/2]` is set", and the test writes the
register as a literal.

### The bank mapper (`cps1_v.cpp:2385-2424`, table at `:1109-1127`)

The gfx ROM is banked, and a tile code is mapped through a per-board PAL. Codes are
first shifted into the common 8×8 unit — scroll1 0, sprites 1, scroll2 1, scroll3 3
— then looked up in a range table; the result is `bank_base + (code & (bank_size -
1))`, shifted back. **An out-of-range code returns −1 and the tile is drawn fully
transparent** (`m_empty_tile`, filled with 0x0F, `cps1_v.cpp:2551`).

SF2 uses `mapper_STF29` with `bank_sizes = { 0x8000, 0x8000, 0x8000, 0 }`, verified
by MAME from a PAL dump:

| Type | Start | End | Bank |
|---|---|---|---|
| sprites | 0x00000 | 0x07FFF | 0 |
| sprites | 0x08000 | 0x0FFFF | 1 |
| sprites | 0x10000 | 0x11FFF | 2 |
| scroll3 | 0x02000 | 0x03FFF | 2 |
| scroll1 | 0x04000 | 0x04FFF | 2 |
| scroll2 | 0x05000 | 0x07FFF | 2 |

The −1 path is the interesting one to test, because a mapper that returned 0 instead
would draw tile 0 all over the screen — which looks like a plausible bug in the
*tilemap* code and would send the reader to the wrong file.

### SF2's video configuration (`cps1_v.cpp:491`, `:1838`, `:1109`)

`{"sf2", CPS_B_11, mapper_STF29, 0x36}` where `CPS_B_11` is, in the field order of
the header comment at `cps1_v.cpp:487`:

| Field | Value |
|---|---|
| `cpsb_addr` | 0x32 |
| `cpsb_value` | 0x0401 |
| multiply protection | `__not_applicable__` (all −1) |
| `layer_control` | 0x26 |
| `priority[4]` | 0x28, 0x2A, 0x2C, 0x2E |
| `palette_control` | 0x30 |
| `layer_enable_mask[5]` | 0x08, 0x10, 0x20, 0x00, 0x00 |

The two trailing zeros are the star layers, absent on this board — consistent with
SF2 having no `stars` ROM region, and a second reason stars are out of scope.

`BoardConfig` in `machine` already carries `cpsb_addr`, `cpsb_value`, and
`in2_addr`. The video fields are C's, and go in `video::VideoConfig` rather than
being bolted onto `BoardConfig`: `machine` has no use for a layer-enable mask, and
`video` has none for `in2_addr`.

---

## Deliverables

1. `crates/video`, dependency-free, with the eight modules above.
2. `Cps1::with_gfx` and a `pub video` field; `Cps1::new` unchanged.
3. `sfemu --ppm <path>` writing a rendered frame as a binary PPM (P6). Chosen over
   PNG because PPM needs no dependency and no compressor: a 384×224 P6 file is a
   15-byte header and 258,048 bytes of RGB. Every image viewer reads it.
4. `docs/hardware/cps1-video-notes.md`, same discipline as the other two: every
   claim backed by a test, a cited line, or a measurement — and an explicit section
   on what C's tests **cannot** see.
5. One new assertion in the opt-in real-ROM test: a rendered attract-mode frame is
   not uniform and draws on more than one palette page.

## Task decomposition

Ten tasks, each ending with a green `cargo test --workspace` and a commit:

1. `video` crate scaffold, CPS-A register indices, `cps1_base`, `VideoConfig`.
2. Tile decoding: the four layouts, `(code, x, y) -> pen`, hand-encoded literals.
3. The bank mapper and the STF29 table, including the −1 transparent path.
4. The palette: page gating, the compaction asymmetry, the brightness math.
5. Tilemap rendering: scan mappers as bijections, tile fetch, scroll, per-tile flip.
6. Sprites: the object table, the end marker, blocking, the one-frame latch.
7. Composition: layer order, enables, the background pen, screen flip.
8. The priority mask: high-priority tile pens occluding sprites.
9. Wiring into `machine` and `sfemu`, the PPM dump, the real-ROM assertion.
10. `docs/hardware/cps1-video-notes.md` and a mutation pass over the new crate.

## Risks

**The one that will actually bite.** The scan mappers and `cps1_base` are pure
arithmetic transcribed from C, and a wrong shift produces an image that is *nearly*
right — layers offset by a few tiles, or wrapping at the wrong column. No test that
renders a single tile catches it. Task 5 therefore tests the mappers as bijections
over their whole domain before rendering anything, which turns "nearly right" into a
hard failure.

**The one that will look like a different bug.** An out-of-range tile code drawing
tile 0 instead of nothing (Task 3) manifests as garbage in the *tilemap*, sending the
reader to `layers.rs` when the fault is in `bank.rs`. The −1 path gets an explicit
test for that reason.

**The one we cannot close.** None of this proves SF2 renders correctly. The PPM dump
lets a human check, and Task 10 writes the limit down. If the attract mode renders
visibly wrong, the debugging tool is a diff against MAME's own behaviour on the same
register writes — which is what B's trace was built to make possible.
