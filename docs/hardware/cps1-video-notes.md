# CPS-1 video notes

How the Capcom Play System 1 turns gfxram and two register files into a 384×224
picture. Companion to `cps1-notes.md`, which covers the bus, the frame schedule,
and the interrupt; this page covers everything downstream of a register write.

Same discipline as its companion: every claim is backed by a test in this
workspace, a line of MAME source read and cited, or an explicit measurement —
never by recollection. Where a claim rests on MAME rather than on a test, it says
so and names the file and line. MAME `master` was read on 2026-08-07;
`src/mame/capcom/cps1.cpp`, `cps1.h`, and `cps1_v.cpp`, BSD-3-Clause,
copyright-holders Paul Leaman.

⚠️ **No ROM data appears in this repository, in this file, or in any test.** SF1
and SF2 are still-commercial Capcom code. Every fixture on this page is a tile
this workspace synthesises byte by byte. The loader takes a path to a MAME-format
set the user supplies; legal sources are Capcom Arcade Stadium, Capcom Fighting
Collection, or a board you own and dumped.

**⚠️ Read [What the tests cannot see](#what-the-tests-cannot-see) before trusting
any of this.** There is no oracle for a CPS-1 frame. That section is not a
caveat at the end; it is the calibration for everything above it.

---

## Coordinates: raster, not visible frame

Two coordinate systems, and every position on this page is in the first:

- **Raster space**, 512×262. What the registers hold. Sprite x/y, scroll
  registers, and the row-scroll table are all in it.
- **Visible frame**, 384×224. What `Framebuffer::pens` holds, row-major.

The visible window is the subrectangle at `(VISIBLE_X, VISIBLE_Y)` = **(64, 16)**
— `CPS_HBEND` and `CPS_VBEND` from `cps1.h:42`, `:46`. So a sprite written at
raster (64, 16) lands at visible (0, 0), and a fixture that wants a sprite in the
top-left corner must add the offset.

This is one subtraction in two places (`sprites.rs`'s `blit`, `layers.rs`'s
`draw_tilemap`) and it is the single most common way to get a plausible-looking
frame that is wrong by 64 pixels. Both directions are pinned:
`the_visible_window_is_the_raster_subrectangle_at_sixty_four_sixteen` and
`a_layers_origin_is_the_visible_window_not_the_raster_origin`.

Mutation confirms it: dropping `- VISIBLE_X` or `- VISIBLE_Y` from either the
sprite blit or the tilemap walk is killed.

---

## The tile bit layout: four `gfx_layout`s are one rule

MAME declares four layouts (`cps1.cpp:3837-3878`) — 8×8, 8×8 odd, 16×16, 32×32.
They are one formula. A tile occupies a storage *frame* `FW` pixels wide (16 for
the 8×8 and 16×16 kinds, 32 for 32×32), and the bit index of pixel `(x, y)` in
plane `p`, from the tile's first byte with bits numbered MSB-first, is

```
y * (4 * FW)  +  32 * (x >> 3)  +  (x & 7)  +  [24, 16, 8, 0][p]
```

Consequences, each of which the formula makes obvious and a four-layout
transcription does not:

| Fact | Why |
|---|---|
| Eight horizontal pixels occupy four consecutive bytes, one per plane | the `32 * (x >> 3)` term is 4 bytes × 8 bits |
| A frame row is `FW / 2` bytes | `4 * FW` bits |
| Plane 0 sits at bit offset 24 and supplies the pen's **most** significant bit | the `[24, 16, 8, 0]` order with `pen \|= 0x08 >> p` |
| 4 bits per pixel, hence 16 pens per colour scheme | four planes |

**The two 8×8 kinds share a 64-byte frame.** `Tile8x8` is `STEP8(0, 1)` and
`Tile8x8Odd` is `STEP8(32, 1)` (`cps1.cpp:3843`, `:3854`): one block holds two
8-pixel tiles side by side, and a code indexes the *frame*, not the half-frame —
which is why `TileKind::bytes()` answers 64 for both. `get_tile0_info` picks
between them with `BIT(tile_index, 5)` (`cps1_v.cpp:2461`), which under scroll 1's
scan mapper is the column's low bit. MAME's comment records that this was found
with a Final Fight board carrying mixed-region ROMs.

Tests: `a_tile_kinds_size_and_byte_count_are_the_layouts`,
`a_code_indexes_by_the_tile_byte_size`,
`scroll_ones_odd_columns_read_the_frames_second_half`. Every term of the formula
is mutation-checked — the row stride, both x terms, the within-group mask, the
plane order, the MSB-first bit selection, the pen bit order, and the odd-tile
bias are all killed when changed.

**Out of range is transparent, not tile 0.** A code whose tile is not wholly
inside the ROM decodes to `TRANSPARENT_PEN` (0x0F), matching MAME's
`m_empty_tile` fill at `cps1_v.cpp:2551`. Returning 0 would paint colour-index 0
wherever a tile is missing, which reads as a tilemap bug and sends the reader to
the wrong file. `a_code_past_the_end_of_the_rom_is_transparent`,
`an_enormous_code_is_transparent_rather_than_an_overflow` (the arithmetic
saturates rather than overflowing), `the_transparent_pen_is_fifteen`.

---

## Pen arithmetic: granularity is 16, and the 32 is a trap

A drawn pixel's palette pen is `colour * 16 + pixel`. The 16 is `gfx_element`'s
granularity for a 4bpp layout, and four independent readings agree:

- `GFXDECODE_ENTRY(..., 0, 0x80)` (`cps1.cpp:3882-3885`) gives 0x80 colour
  schemes, and 0x80 × 16 = 0x800 — exactly the four users' 0x20 schemes each.
- The star layers write pens `0x800 + col` and `0xa00 + col` (`cps1_v.cpp:2900`,
  `:2926`), immediately above that 0x800. At a granularity of 32 the tile pens
  would reach 0x1000 and swallow them.
- `set_entries(0xc00)` (`cps1.cpp:3932`) is 0x800 tile pens plus 0x400 star pens.
- `BACKGROUND_PEN` 0xBFF then falls in the star region, unreachable from a
  tilemap.

**The trap:** `m_palette_size = CPS1_PALETTE_ENTRIES * 32` (`cps1_v.cpp:2542`).
That 32 is *bytes per scheme in gfxram*, two per entry — not pens per scheme.

The colour bases partition the palette (`cps1_v.cpp:2466`, `:2485`, `:2502`):

| User | Schemes | Base |
|---|---|---|
| Sprites | 0x00-0x1F | none added |
| Scroll 1 | 0x20-0x3F | 0x20 |
| Scroll 2 | 0x40-0x5F | 0x40 |
| Scroll 3 | 0x60-0x7F | 0x60 |

The highest pen any tilemap can produce is `(0x1F + 0x60) * 16 + 15 = 0x7FF`,
below `BACKGROUND_PEN` — which is what makes "a solid layer hides the background"
a real invariant rather than a coincidence. `the_layer_colour_bases_partition_the_palette`
asserts that inequality directly, and `a_solid_layer_leaves_no_pixel_undrawn`
and `a_full_screen_opaque_layer_hides_the_background` are the two halves of the
claim. Sprites having no base is
`sprite_colours_come_from_the_low_five_bits_with_no_base`; mutation kills a
version that adds one.

---

## The palette: six pages, and one asymmetric rule

`cps1_build_palette` (`cps1_v.cpp:2611-2645`) copies pages out of gfxram at the
CPS-A palette base, gated by a CPS-B register: bit `n` enables page `n`. Six
pages of 0x200 entries = 3072 pens, and `BACKGROUND_PEN` 0xBFF being the last of
those 3072 is the corroboration that the loop's extent is right.

This crate writes raw 16-bit entries and converts separately. That is a testing
decision: a page-placement bug and a brightness-arithmetic bug then fail
different tests instead of being indistinguishable in one RGB buffer.

### The compaction asymmetry

The source pointer advances past a *skipped* page only once some page has already
been copied (`cps1_v.cpp:2638-2643`). So:

- A disabled page **2** leaves pages 3-5 reading their own source words.
- A disabled page **0** shifts every later page's source down by one.

MAME's comment: "if the first palette pages are skipped, all the following pages
are scaled down". It is an odd rule and it is the hardware's. Both halves are
pinned — `a_disabled_middle_page_leaves_later_pages_in_place` and
`leading_disabled_pages_compact_the_source` — and each half kills a different
mutant: making the pointer always advance kills the first, never advance the
second. One test alone would leave the asymmetry unverified in one direction.

### Brightness

`cps1_v.cpp:2628-2634`. Blue is bits 0-3, green 4-7, red 8-11, brightness 12-15:

```
bright  = 0x0f + ((entry >> 12) << 1)          max 0x2d
channel = nibble * 0x11 * bright / 0x2d
```

`0x11` scales a nibble to a byte; brightness 15 is unity and brightness 0 is
about a third, which MAME reads off the schematics. **The division truncates, and
that is the arithmetic as modelled**: entry 0x8777 gives 81, not 82.
`the_brightness_formula_scales_each_nibble` pins that value as a literal.

Mutation kills the `<< 1` weight in either direction, the `0x11`, the channel
order, and `BRIGHT_MAX`.

---

## CPS-A base registers can point past gfxram

`cps1_v.cpp:2099-2110`: a base register is scaled by 256, truncated to the
table's alignment, and wrapped into 256 KB. The truncation is hardware, not
tidiness — MAME's comment names Captain Commando's continue screen as a game that
fails to align its tables.

```
base = reg * 256
base &= !(boundary - 1)          0x4000 scrolls, 0x800 obj/rowscroll, 0x400 palette
index = (base & 0x3FFFF) / 2     a WORD index
```

**`& 0x3FFFF` bounds the index to 256 KB and gfxram is 192 KB** (`cps1.cpp:592`),
so some registers resolve to word indices from 0x18000 to 0x1FE00 — outside the
array. That is the hardware's arithmetic and MAME's too: `cps1_base` returns a
pointer into a `required_shared_ptr` with no bounds check.

**Which registers overflow is counterintuitive.** `* 256` followed by `& 0x3FFFF`
keeps only bits 8-17 of the product, so the index depends on nothing but
`reg & 0x3FF`, and it leaves gfxram exactly when `(reg & 0x3FF) >= 0x300`. That is
a quarter of all register values and it includes small ones: **0x0300 resolves to
0x18000**, the first word past the end, while **0xE000 resolves to 0**. "Only
large registers overflow" is the wrong intuition, and the test pins the predicate
over all 65,536 registers at all three boundaries rather than only the maximum —
a maximum alone is satisfied by an implementation that overflows for the wrong
inputs.

Every caller therefore wraps with `% gfxram.len()`. Clamping in `cps_a_base`
would silently relocate a table the guest asked for.
`cps_a_base_can_point_past_gfxram_so_callers_must_wrap` exists so a later reader
does not remove a wrap believing it redundant, and each wrapping caller has its
own case: `a_base_past_the_end_of_gfxram_wraps_rather_than_panicking`,
`a_page_straddling_the_end_of_gfxram_wraps_mid_page`,
`the_table_base_offsets_the_layer_and_wraps`,
`a_base_past_the_end_of_gfxram_wraps` (sprites).

Word indices, not byte offsets, everywhere in `regs.rs`. Mixing the two shifts
the whole register file by one slot and every value then reads as plausible in
the wrong place. `the_cps_a_indices_are_byte_offsets_divided_by_two` pairs two
independently written tables rather than restating one expression.

---

## The bank mapper: a miss is nothing, not tile 0

CPS-1 boards route tile codes through a PAL, and MAME models one function per PAL
(`cps1_v.cpp:1109` for SF2's `mapper_STF29`). A code is shifted into 8×8 ROM
units, matched against a table of ranges, masked into its bank, and shifted back
(`cps1_v.cpp:2385-2424`).

The shifts are the tile sizes in 8×8 units — a 16×16 tile is two of them, a 32×32
is eight — so scroll 1 shifts 0, sprites and scroll 2 shift 1, scroll 3 shifts 3
(`cps1_v.cpp:2392-2397`).

**A code no range covers has no ROM behind it.** MAME returns −1 and
`cps1_v.cpp:2474` substitutes the empty tile; this crate returns `None` and the
caller draws nothing. The wrong answer here is *plausible*: a mapper answering 0
on a miss draws tile 0 across the layer, which reads as a tilemap bug.
`a_code_outside_every_range_maps_to_nothing`, plus
`a_tile_the_mapper_rejects_draws_nothing` and
`a_blocked_sprite_the_mapper_rejects_draws_no_tile_at_all` at the two call sites.

Three properties of STF29's table are checked on **synthetic** mappers rather
than on SF2's, because SF2's own table cannot distinguish them:

| Property | Why SF2's table cannot see it | Test |
|---|---|---|
| The bank mask wraps within the bank | every STF29 range is narrower than a bank, so nothing aliases | `the_bank_mask_wraps_within_the_bank` |
| A bank's base is the sum of the banks before it | STF29's three banks are all 0x8000, where a running sum and `bank * 0x8000` agree | `a_banks_base_is_the_sum_of_the_banks_before_it` |
| A zero-sized bank maps to nothing | no STF29 range uses bank 3, the absent one | `a_zero_sized_bank_maps_to_nothing` |

The third is not merely untested-by-SF2 but actively dangerous: `size - 1` on a
zero size is `0xFFFF_FFFF`, an unmasked pass-through in release and a panic in
debug.

`BankRange` carries one `GfxType` where MAME's `gfx_range::type` is a bitmask
(`cps1_v.cpp:597`, `:915`). Every row of STF29's table names exactly one type. A
board whose table shares rows between types needs that field widened — which is a
visible change, not a silent misread.

---

## The tilemaps: 64×64, and the mappers are bijections

Each layer is a 64×64 grid of tiles held in gfxram as two words per tile, a code
and an attribute. A *scan mapper* turns `(col, row)` into the tile's index
(`cps1_v.cpp:2433-2450`):

```
scroll1:  (row & 0x1F) + ((col & 0x3F) << 5) + ((row & 0x20) << 6)
scroll2:  (row & 0x0F) + ((col & 0x3F) << 4) + ((row & 0x30) << 6)
scroll3:  (row & 0x07) + ((col & 0x3F) << 3) + ((row & 0x38) << 6)
```

**Each is a permutation of `0..0x1000`, and that is the load-bearing test.**
`every_scan_mapper_is_a_bijection_over_the_tile_grid` enumerates all 4096
`(col, row)` pairs and requires 4096 distinct indices. Transcribed arithmetic
with a wrong shift still draws a plausible picture — but it cannot stay a
bijection, because a collision means two tiles share a slot and some slot is
unreachable. `the_scan_mappers_are_the_ones_mame_documents` pins individual
values beside the citation as well; the bijection is the property, the values are
the transcription.

The attribute word:

```
attr & 0x001F   colour scheme, added to the layer's base
attr & 0x0020   X flip
attr & 0x0040   Y flip
attr & 0x0180   priority group 0-3
```

Only scroll 3 masks its code, `& 0x3fff` (`cps1_v.cpp:2495`); scrolls 1 and 2
take the whole word (`:2453`, `:2477`). `a_tile_entry_is_a_code_and_an_attribute`
checks each field in isolation *and at its upper edge* — colour 0x1F and group 3
from `attr = 0x0380` — because every drawing fixture in this crate uses small
values and a mask only fails where it stops. Those two cases exist because the
whole-crate mutation pass found them missing: `0x1F → 0x0F` and `0x0180 → 0x0380`
both survived until they were added.

The group is two bits, and that bound is structural: `hi_pens` is `[u16; 4]`, so
a widened mask indexes past it.

### Scroll and wrap

Signed throughout, with `rem_euclid` for the wrap, so a negative scroll is the
same arithmetic as a positive one rather than a branch. The registers are
unsigned words holding signed scrolls, read as `i16` before widening: 0xFFC0 is
−64, not 65472.

**That `as i16` cannot be killed by a test, and this is provable rather than a
gap.** The two readings differ by 65536, a whole multiple of every layer's map
span (64 tiles × 8, 16, or 32 pixels = 512, 1024, 2048 — and 65536 is divisible
by each), so on screen they are indistinguishable.
`the_unsigned_scroll_reading_is_an_equivalent_mutant` states that argument and
**fails if it ever stops being true**, so a future layer whose span does not
divide 65536 turns the equivalence back into a real bug.
`a_negative_scroll_register_moves_the_layer_right` pins the direction and
distance regardless.

The two `rem_euclid(tiles)` calls in `draw_tilemap` are the same shape: `col` and
`row` reach nothing but `Layer::scan`, whose masks already cover exactly six
bits, and for a power-of-two modulus the low bits of a two's-complement `as u32`
*are* the mathematical remainder. They stay because they make `col` and `row` the
in-range values their names claim rather than leaving that to a mask two
functions away. `the_map_wraps_at_sixty_four_tiles` pins the wrap itself.

### Row scroll is scroll 2's alone

`tilemap[1]` is the one MAME calls `set_scroll_rows(1024)` on
(`cps1_v.cpp:3022`); scrolls 1 and 3 take a single `set_scrollx` (`:3015`,
`:3035`). It applies only when `videocontrol` bit 0 is set — `if (BIT(videocontrol, 0))
// linescroll enable` (`cps1_v.cpp:3018`); with it clear MAME calls
`set_scroll_rows(1)` and one flat `set_scrollx` (`:3030-3032`).

The table index is

```
x[y] = scroll_x + other[(y + VISIBLE_Y + ROWSCROLL_OFFS) & 0x3FF]
```

with **no `scroll_y` term**: the table is indexed by *raster row*, and visible row
`y` is raster row `y + VISIBLE_Y`. The obvious reading of MAME's line suggests a
`scroll_y` term and is wrong; the derivation is written out on
`row_scroll_reads_a_per_line_offset_independent_of_the_vertical_scroll`, which is
the test that would fail if it were added.

Without a test on the *selector*, a correctly computed row-scroll table that was
never selected would look exactly like a working one — hence
`videocontrol_bit_zero_selects_row_scroll_for_scroll_two` and
`row_scroll_does_not_apply_to_scrolls_one_and_three`. `ScrollRows` carries the
per-row array for all three layers so `draw_tilemap` has one code path instead of
a flag it could get the wrong way round.

---

## Sprites

Four words per record — `x`, `y`, `code`, `attr` (`cps1_v.cpp:2652-2680`) — in a
0x400-word table, drawn as one or more 16×16 tiles.

```
attr & 0x001F   colour scheme (no base: sprites own 0x00-0x1F)
attr & 0x0020   X flip
attr & 0x0040   Y flip
attr & 0x0F00   X block size - 1, in 16-pixel tiles
attr & 0xF000   Y block size - 1
```

Positions are masked `& 0x1ff` (`cps1_v.cpp:2777`) and are raster coordinates, so
a sprite near an edge is clipped, not wrapped (`a_sprite_straddling_the_edge_is_clipped`).

### The one-frame latch

`cps1_objram_latch` memcpys 0x800 bytes out of gfxram at vblank
(`cps1_v.cpp:3068`), under the comment "CPS1 sprites have to be delayed one
frame". **A renderer reading live objram puts every sprite one frame ahead of its
layers, which on screen reads as jitter rather than as a bug** — so the copy is
the behaviour, not an optimisation.

`machine` takes the delay from the frame schedule rather than simulating it:
`latch_objects` sits inside `run_scanline`'s vblank branch beside
`assert_vblank`, mirroring where `screen_vblank_cps1` puts the memcpy. That makes
the delay exactly one frame for a caller stepping scanlines by hand as much as
for a `run_frame` caller. `the_latch_delays_the_table_by_a_frame`,
`render_draws_the_latched_object_table_and_not_live_objram`, and — for the
schedule — `machine`'s `objects_are_latched_once_per_frame_at_vblank`, which
asserts through the drawn frame rather than by reading the latch, because a test
that read the latch would pass on a renderer that ignored it.

### The end marker skips two records

`find_last_sprite` (`cps1_v.cpp:2684`) walks forward in steps of four for an
attribute with `(attr & 0xFF00) == 0xFF00` and answers `offset - 4`, so **the
record immediately before the marker is skipped too**: a marker in record 2
leaves records 0 and 1 drawable. With no marker the whole table is used (`:2716`,
"Sprites must use full sprite RAM").

The test is on the high byte, not the whole word — 0xFF01 is a marker too.

A marker in record 0 gives −4. MAME holds that in a signed `int` and its `i >= 0`
loop draws nothing; this crate returns `None`. A `saturating_sub` to `Some(0)`
would draw the very record the marker declares is not a sprite —
`a_marker_in_record_zero_draws_no_sprites` kills exactly that.

### Table order is forwards

Records draw **forwards**, so a later entry lands on top.
`cps1_v.cpp:2754` reads `for (int i = m_last_sprite_offset; i >= 0; i -= 4)`,
which looks like a backwards walk and is not: the record pointer is a separate
variable advancing `base += baseadd` (`:2836`) with `baseadd = 4` (`:2751`), and
`i` only counts how many records remain. The genuinely downward variant — `base`
starting at `m_last_sprite_offset` with `baseadd = -4` (`:2746`) — is reached only
under `bootleg_kludge` bit 6, commented "some sf2 hacks draw the sprites in
reverse order". `sprites_draw_in_table_order_so_a_later_entry_lands_on_top`.

### Blocked sprites wrap within the low nibble

The mapper runs **once, on the base code**, before any block arithmetic
(`cps1_v.cpp:2764-2766`) — so a rejected code drops the whole sprite rather than
individual tiles, and the block codes are derived from the *mapped* value. Then
(`cps1_v.cpp:2789` onward):

```
tile = (code & !0x0F) + (cx & 0x0F) + 0x10 * cy
```

The x term wraps within the low nibble, so **a block crossing a 16-code boundary
repeats rather than running on into the next sixteen codes**; rows step by 0x10.
Under flip the x codes count down from the far end and the y codes reverse.
`a_blocked_sprite_tiles_its_codes_within_the_low_nibble`,
`a_blocked_sprite_counts_down_from_the_far_end_under_flip`,
`a_blocks_size_is_the_attribute_nibble_plus_one`.

A lone sprite has `attr & 0xFF00 == 0`, giving nx = ny = 1, and takes the same
path — no separate branch, so no untested branch.

---

## Layer order and the priority mask

`render_layers` (`cps1_v.cpp:2971-2999`) reads four 2-bit fields out of the
layer-control register, from bit 6 up, and draws them in field order. So **`l0` is
the back and `l3` the front**. Each field's value selects what goes at that depth:
0 for the sprites, 1, 2, 3 for the corresponding scroll layer. A value can
repeat, and SF2's own 0x1B40 does repeat 1 — which is why
`the_layer_order_is_four_two_bit_fields` also pins a value with four distinct
fields.

A scroll layer draws only if two conditions hold (`cps1_v.cpp:2331-2333`): its bit
in the layer-control register, and — for scrolls 2 and 3 only — a bit of
`videocontrol`, bit 2 for scroll 2 and bit 3 for scroll 3. Scroll 1 has no second
condition. `a_layer_absent_from_the_layer_control_is_not_drawn`,
`videocontrol_bits_two_and_three_gate_scroll_two_and_three`.

Screen flip is `flip_screen_set(BIT(videocontrol, 15))` (`cps1_v.cpp:3044`),
applied here as **one pass over the finished buffer**. That is equivalent to
MAME's per-primitive flip because the visible window is symmetric within the
raster pivots 511 and 255: mirroring the crop is the same picture as cropping the
mirror. `prio` is mirrored too — it describes the same pixels, and mutation kills
a version that flips only the pens.

### The double inversion

`cps1_update_transmasks` (`cps1_v.cpp:2515-2531`) computes `mask =
m_cps_b_regs[priority[i]/2] ^ 0xffff` and passes it to `set_transmask` as the pens
that are **transparent in the high-priority pass**. Read that twice: a register
bit that is *set* leaves its pen opaque there, and an opaque high-priority pen is
what marks the sprite mask. So this crate stores the register uninverted and a set
bit means "this pen of this group hides a sprite behind it".

Pen 15 corroborates the direction: it is the transparent pen everywhere else, and
`set_transmask`'s second argument keeps bit 15 transparent in the low pass
regardless.

A board with no register for a group occludes nothing there — `mask = 0xffff`,
every pen transparent in the high pass ("completely transparent if priority masks
not defined (qad)", `:2526`), which is a **zero** mask in this crate's uninverted
form. `a_board_with_no_priority_register_occludes_nothing` and
`a_priority_register_of_zero_occludes_nothing` are separate tests because they
are separate claims that happen to agree: one is about `None`, one about 0.

Four registers, one per tile group, and `each_tile_group_reads_its_own_priority_register`
is what prevents them collapsing to one — mutation kills `hi_pens[0]` in place of
`hi_pens[group]`.

### Only the layer immediately below the sprites masks them

`render_layers` calls `cps1_render_high_layer(..., l0)` only `if (l1 == 0)`, and so
on for each pair (`cps1_v.cpp:2985-2996`). A layer with no sprite pass behind it —
including the frontmost, which has no next depth at all — occludes nothing,
because there is nothing there to occlude.
`a_layer_masks_sprites_only_when_the_next_depth_is_the_sprites`.

This is also why `prio` is only ever set, never cleared, within a frame: no two
layers can disagree about a pixel.

---

## What the tests cannot see

**No test in this repository proves that SF2 renders correctly.**

There is no external corpus of expected CPS-1 frames, none is going to exist, and
"it looks right" is not a test. Sub-project A had 317,500 vector cases as its
oracle; this subsystem has MAME's source read line by line, synthetic tile
fixtures, and a mutation pass. That is a **weaker oracle** and this section says
where.

### What the tests do prove

- **Every arithmetic rule on this page, against hand-computed literals.** 99 of
  99 mutants over `crates/video` are killed (see Method below), including every
  term of the tile formula, every scan mapper, every mask and shift of both
  attribute decodes, the palette compaction in both directions, the brightness
  arithmetic, the block-code wrap, the end marker, the latch, the layer order,
  and the priority mask's sense.
- **Structural invariants that a plausible-looking wrong answer cannot satisfy:**
  the scan mappers are bijections over all 4096 tiles; no tilemap pen can equal
  the background pen; a solid layer leaves no pixel undrawn.
- **That fixtures decode as intended before any render test relies on them** —
  `solid_bytes_are_solid`, `fixture_tiles_decode_as_intended`,
  `the_fixture_mapper_is_the_identity_on_small_codes`,
  `the_fixture_board_sets_every_base_it_relies_on`. A bad fixture would
  otherwise make a render test assert a wrong pen and pass.

### What no test proves

- **That the composition of all of it produces SF2's actual picture.** Every
  piece is checked against MAME's description of that piece. Nothing checks the
  whole against a real frame. A misreading of MAME that is *self-consistent*
  across the pieces would pass everything here.
- **What a real ROM's registers actually contain.** Every fixture writes the
  registers this crate expects. If SF2 uses a register combination no fixture
  covers, no test here notices.
- **Anything about timing within a frame.** The renderer draws a whole frame at
  once. A game that changes a register mid-frame — a raster effect — gets the
  value that happens to be there at `render()` time. Whether SF2 does this is
  unknown, and no test would tell us.
- **Colour accuracy against a real board.** The brightness formula is MAME's
  reading of the schematics, checked against MAME's arithmetic. Neither has been
  compared to a photograph.

**What is left to a human's eye:** `sfemu <path-to-your-own-sf2.zip> --ppm out.ppm`
writes the last frame as a binary PPM. Looking at that file is the only check in
existence that this subsystem draws Street Fighter II. The report's `framebuffer`
line — drawn-pixel count and distinct palette pages — is the machine-readable
part of that check, and the page count is there because a broken palette base
produces exactly one page on a real frame, which a pixel count alone cannot
distinguish from a plausible single-page logo.

### Deliberately unimplemented

Each of these exists on some CPS-1 board and is unreachable on an SF2 one. They
are absent by decision, not oversight:

| Feature | Why it is unreachable on an SF2 board |
|---|---|
| **The two star layers** (`cps1_v.cpp:2880-2930`) | They need a `stars` graphics region, and SF2's `ROM_START` has none. Corroborating: SF2's `CPS_B_11` layer-enable mask has 0 in both star positions (`cps1_v.cpp:491`), which is why `VideoConfig::layer_enable_mask` carries three entries where MAME's has five. The star pens 0x800-0xBFF are still accounted for — `BACKGROUND_PEN` is one of them. |
| **`bootleg_kludge`** (reverse sprite order, `cps1_v.cpp:2746`) | Gated on a per-set flag whose comment reads "some sf2 hacks draw the sprites in reverse order". The genuine `sf2` set does not set it. Implementing it would add a branch no legal ROM set reaches. |
| **CPS-B multiply protection** | `CPS_B_11` expands with multiply protection `__not_applicable__` (`cps1_v.cpp:491`). Later boards use it; this one has no such register. |

The honest form of these three is "not implemented, with a citation", not "handled".

---

## Method: the mutation pass

**99 mutants over `crates/video`, 99 killed, 0 NO-OP, 1 documented equivalent
mutant, 1 control.** Sampling every `if`, mask, and shift in the crate: the tile
bit formula, both attribute decodes, all three scan mappers, the palette pages and
brightness, `cps_a_base`'s three stages, the bank mapper's ranges and masks, the
sprite marker and block arithmetic, both clip bounds, the layer order and enables,
the row-scroll selector, and the priority mask's sense and grouping.

The harness, and why each part of it is there:

- Back up with `shutil.copy`, restore with `shutil.copy`. **Never `git checkout`**
  — the working tree may hold uncommitted work in the same file, and reverting a
  hand-written mutant that way destroys it.
- Assert `src.count(old) == 1` before replacing. A pattern that is absent, or
  matches more than once, is a **NO-OP** — not a result. Counting a NO-OP as
  "survived" invents a test gap; counting it as "killed" invents coverage.
- KILLED/SURVIVED by exit code of `cargo test -q -p video`.
- One deliberate **control** mutant that must survive (a dead statement). A
  harness whose every mutant dies is more likely broken than thorough.

Two survivors were real gaps and are now closed, both in `tile_info` and both the
same shape — **a mask checked only below its top bit**. `attr & 0x1F → 0x0F`
survived because every fixture's colour scheme was 5; `attr & 0x0180 → 0x0380`
survived because no fixture set bit 9. This is the branch's characteristic defect
in miniature: not a wrong expectation, but an *input* that cannot exercise the
property claimed.

One survivor is an **equivalent mutant** provable unkillable by arithmetic (the
`as i16` scroll reading, above), documented at the line with a named test that
fails if the proof stops holding.

---

## How to check that a claim in this file is still checked

Three procedures, in increasing cost.

**For a constant or an arithmetic term:** mutate it by hand, run
`cargo test --workspace --release`, record which tests died, revert, and confirm
the clean tree **before the next mutant**. A mutant that kills nothing means the
constant is a comment, not a check. `/tmp/mut10.py`-style harnesses are
disposable; the discipline above is not.

**For a claim sourced from MAME rather than from a test:** the check is
**re-reading the cited line**. Every such claim here names its file and line for
that reason. A claim citing `cps1_v.cpp:2638-2643` is checkable in ten seconds; a
claim saying "MAME does X" is not, and is the shape this file avoids.

**For the one claim neither procedure reaches** — that this draws Street Fighter
II — run `sfemu <your-rom-set> --ppm out.ppm` and look at it. Nothing automated
can replace this, and no amount of green tests above should be read as having
done so.

### Getting the reference back

The three MAME files are **not in this repository** — MAME is BSD-3-Clause and
vendoring someone else's tree into ours is not our call to make. Restoring them:

```sh
mkdir -p /tmp/mameref && cd /tmp/mameref
for f in cps1.cpp cps1.h cps1_v.cpp; do
  curl -sO "https://raw.githubusercontent.com/mamedev/mame/master/src/mame/capcom/$f"
done
```

⚠️ **The line numbers on this page are pinned to `master` as of 2026-08-07, and
`master` moves.** A citation that has drifted looks exactly like a citation that
was always wrong. If a line does not say what this page claims, check whether the
*content* still exists elsewhere in the file before concluding the claim is false
— and if it moved, update the number here. Pinning to a commit hash instead of
`master` would fix this and is worth doing the next time these files are fetched.
