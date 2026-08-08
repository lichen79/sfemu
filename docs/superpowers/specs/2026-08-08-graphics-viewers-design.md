# Design: Graphics viewers — looking at the video hardware (sfemu sub-project E3)

Date: 2026-08-08
Scope: Sub-project E3 of the sfemu arcade emulator
Status: approved
Depends on: B (board and bus), C (video), E1 (frontend and loop), E2 (debugger)

---

## The problem

E2 answered "which instruction wrote that". It cannot answer "what did the video
hardware do with it", and that is where a CPS-1 bug most often is: the 68000 wrote
a plausible word to gfxram and the picture is still wrong.

The failures E3 exists for all look identical on screen — a missing or wrong
sprite — and have unrelated causes:

- The tile code is right and the **graphics ROM decode** is wrong, so the tile is
  garbage. `tiles.rs` implements one bit-layout rule for four layouts; a wrong
  `frame_width` draws recognisable-but-sheared pixels.
- The tile code is right and the **bank mapper** rejects it, so
  `BankMapper::map` returns `None` and the tile is absent rather than wrong.
  `draw_tilemap` skips it silently, which is correct behaviour and invisible.
- The tile is right and the **palette** is wrong, so the sprite is drawn in the
  wrong colours or in the background colour.
- Everything is right and a **layer above it** is drawing over the top, or a
  priority mask is occluding it.

Nothing in the emulator today distinguishes these. `--screenshot` gives you the
composed frame, which is the sum of all four. E3 gives you each input separately:
the ROM's tiles, the layer's tile table, the palette, and the layer stack.

## What this is not

- **Not a graphics editor.** Nothing here writes gfxram, the palette, or the ROM.
  Same reasoning as E2's "why nothing is writable", and the same weight: the bug
  is in the emulator or the ROM, and a poke makes the state neither produced.
- **Not a re-implementation of the renderer.** Every view calls the same `video`
  function the renderer calls. A view that computed a tile's pixels its own way
  would agree with itself and disagree with the picture, which is this branch's
  characteristic defect with a new costume.
- **Not a frame-timing or beam view.** The scanline and the frame count are E2's
  status panel and stay there.
- **Not E2.** The line the E2 spec drew holds from the other side: E2 shows you
  the *CPU's* state, E3 shows you the *video's*. A palette is not a register.

## The surface: a second overlay, one view at a time

Same mechanism as E2 — drawn into the ARGB frame after `pens_to_argb`, composed
into a `Vec<u32>` a test reads back, no new dependency, nothing behind the display
boundary. The reasoning is E2's and is not re-argued.

**One view at a time, not four independent flags.** E2's `Panels` deliberately has
four flags because "the useful combinations are not a sequence" — and that
reasoning inverts here for a concrete reason, not a stylistic one: an E2 panel is a
corner of the screen, while a palette of 3072 swatches or a grid of 16×16 tiles is
most of a 384×224 frame. Two video views on screen at once would overlap, so they
*are* a sequence, and `F10` cycles them:

```
tiles → tilemap → palette → layers → tiles
```

**The video viewer draws last, and it is opaque.** It overlaps E2's panels where
they collide and wins, because a panel background that blended would make the
tiles' contrast depend on the register text underneath. There is no attempt to lay
the two out around each other: at this size it cannot be done, and `F9` is how you
put the registers back.

## The four views

### 1. Tiles — the graphics ROM, by ROM tile index

A grid of tiles decoded straight out of the graphics ROM, indexed by **ROM tile
index** rather than by a guest code. `Enter` cycles the `TileKind` — all four,
including `Tile8x8Odd`, because it is a real layout scroll 1 uses and a browser
that omitted it could not show the bug MAME found with a mixed-region Final Fight
board. `[` and `]` page the index.

**Drawn in greyscale by pen value, not in a palette colour scheme.** Sixteen
shades, pen 0 black to pen 15 white. This is the decision in this view worth
defending, because a colour scheme is the obvious thing to offer:

- It is what is actually in the ROM. A colour scheme is the palette's
  interpretation of it, and the palette has its own view.
- It keeps two failure modes in two views. `palette.rs` already states this
  doctrine for its own split — raw entries and RGB kept apart so that "a
  page-placement bug and a brightness-arithmetic bug then fail different tests
  instead of being indistinguishable in one RGB buffer". A tile browser tinted
  through a palette page makes a wrong decode and a wrong palette look the same.
- It removes an axis. With no colour scheme to choose there is no fifth key, and
  the view has one meaning.

Pen 15 is `TRANSPARENT_PEN` everywhere else in the renderer and is drawn here as
white rather than as a transparency pattern, because in the ROM it is a pen like
any other and the browser is showing the ROM.

### 2. Tilemap — a layer's tile table

For one scroll layer, selected by `Enter`:

- The table's **word base** in gfxram, from `cps_a_base` — the same call
  `render` makes.
- The layer's **scroll x and y**, read as `i16` and shown signed, because 0xFFC0
  is −64 and a viewer that showed 65472 would hide exactly the raster-offset class
  of bug the `video` crate had to fix three times.
- A window of **tile codes** as hex around a cursor, from `tile_info` — its
  `code`, already masked by `Layer::code_mask`.
- For the cursored tile: its **decoded attributes** (colour scheme, both flips,
  priority group), the **mapper's answer** for it, and the tile itself.

The mapper's answer is the point of this view. `BankMapper::map` returning `None`
is why a tile is absent rather than wrong, and `draw_tilemap` handles that case by
skipping the tile — correct, silent, and undiagnosable from the picture. Shown
here as `----` against a mapped index, it is one glance.

The cursor's default is the tile at the top-left of the *visible* screen, which is
a function of the scroll registers and of `VISIBLE_X`/`VISIBLE_Y` — the useful
default, and a fact worth displaying in its own right.

### 3. Palette — all 3072 pens

Six pages of 512 as coloured swatches, each labelled with its page number, plus the
raw 16-bit entry of the cursored pen in hex. All 3072 fit on one screen at this
size, so there is nothing to page and `[`/`]` move the cursor.

`BACKGROUND_PEN` (0xBFF) is marked, because "the screen is filled with a colour I
did not expect" is a palette question and that pen is the answer to it.

The colours come from `entry_to_rgb` through `pixels::argb` — the same two calls
the window uses. Not a third conversion.

### 4. Layers — what is drawn, and why not

Four rows, sprites and the three scroll layers, each showing:

| | what it says |
|---|---|
| hardware | whether the *guest* has this layer enabled |
| debug | whether the viewer is subtracting it |
| depth | its position in `layer_order`, front to back |
| feeds sprites | whether its priority mask is active this frame |

`[`/`]` select a row and `Enter` toggles the debug column.

The hardware column is why this view needs an API change rather than arithmetic:
it must come from `video`'s own `layer_enabled`, which is private today. A view
that re-derived "is scroll 2 enabled" from `layercontrol` and `videocontrol` would
be a second implementation of a two-condition rule, agreeing with itself and
capable of disagreeing with the renderer. This is the same resolution E2 reached
for `peek_word`/`read_word`: one implementation, published, and the duplication
never exists.

Sprites have no enable bit — they always draw — so their hardware cell reads
`always`. That is a fact about CPS-1 and it belongs on the screen, because "why
can I not turn the sprites off in hardware" is a question the table otherwise
invites.

## Layer toggles change the picture, not the machine

E2's central constraint was that reading must not disturb the machine. E3 breaks
the surface of that rule and not its substance, and the distinction has to be
exact because it is the one an implementation gets wrong.

A layer toggle changes **what is drawn**. It must not change **what the guest
computes**. On CPS-1 those are genuinely separable: the framebuffer is an output,
and nothing the 68000 or the board reads depends on it. `Video::render` reads
gfxram and the CPS registers and writes only `fb` and `pal`.

So the invariant, stated as the test that proves it: **for any layer mask, running
N frames leaves the CPU, the board, `total_cycles`, `line`, and the trace
counters identical.** This is E3's analogue of E2's
`watching_the_machine_does_not_change_it`, and it is the criterion that matters
most here for the same reason: the tool that observes the bug must not be part of
it.

**The mask can only subtract, never add.** A mask bit cannot force on a layer the
hardware has disabled. This is not caution — forcing a layer on would draw a
tilemap from a base register the guest has not set up, producing garbage that looks
exactly like the tile-decode bug the viewer exists to rule out. "Show me only
scroll 2" is reached by subtracting the other three, which needs no such power.

Two consequences that must be written down rather than discovered:

- **A screenshot taken with a layer masked off is missing that layer.** `Video::rgb`
  reads the same `fb`. This is the right behaviour — you turned it off to look at
  it — and it is confusing if unstated.
- **The mask is not machine state.** It must not be in a save state, and loading
  one must not reset it, for the same reason `Trace` is excluded: it records the
  session, not the machine. `Cps1::restore` touches `Video` only through
  `set_obj_latch`, so this holds as the tree stands and gets a test to pin it.

## The API changes in `video`

Every one is a gap found by survey rather than designed, and each is the minimum:

```rust
// crates/video/src/compose.rs

/// Which layers the caller permits to draw. Subtractive only.
pub struct LayerMask { pub sprites: bool, pub scroll1: bool,
                       pub scroll2: bool, pub scroll3: bool }
impl LayerMask { pub const fn all() -> Self; }

pub struct Video {
    /// A debugger's subtraction. `all()` by default, so nothing changes.
    pub enable: LayerMask,
    // ...unchanged
}
impl Video {
    /// The graphics ROM, for a viewer that decodes tiles out of it.
    pub fn gfx(&self) -> &[u8];
}

/// Whether the *guest* has this layer enabled. Was private.
pub fn layer_enabled(cfg: &VideoConfig, layer: Layer,
                     layercontrol: u16, videocontrol: u16) -> bool;

/// Whether the layer at `depth` prepares the sprite mask: the next depth is
/// the sprites. Extracted from `render`, which now calls it.
pub fn feeds_sprites(order: &[u8; DEPTHS], depth: usize) -> bool;
```

`enable` is a public field on `Video` rather than a parameter to `render`, and the
choice is between one default-valued field and threading an argument through
`Cps1::render`, every test that renders, the screenshot path, and the test runner —
for a value that is `all()` at every one of those call sites. The field is honest
about what it is: a persistent view setting, like `Debugger::panels`.

**The viewer never writes it.** `gfx.rs` computes a `LayerMask` and the loop
assigns it, exactly as `Debugger` returns decisions and the loop performs them.
This keeps every `frontend` entry point on `&Cps1` and leaves one place where a
view setting reaches the machine.

`render` combines the hardware's answer with the mask using `&&`, in that order.
Sprites gain their first enable check, which the mask is the only thing that can
fail.

`feeds_sprites` is one line, and extracting it is the same argument as
`layer_enabled`'s at smaller scale: the layers view's "feeds sprites" column must
be the renderer's answer, and a second `order.get(depth + 1) == Some(&0)` written
in `frontend` is a second answer.

**E3 adds no dependency and no manifest change**, and reaches `video` the way
`frontend` already does — `machine::video`, never past `machine`. Verified against
`pixels.rs`, which imports `machine::video::compose::Video` today.

## The keys, and why `KeySet` becomes a `u64`

Five new keys, on top of E1's and E2's 29:

| Key | Does |
|---|---|
| `F9` | Toggle the video viewer |
| `F10` | Cycle the view: tiles → tilemap → palette → layers |
| `[` / `]` | Page or move within the current view |
| `Enter` | Act on the current view: cycle the tile kind, the layer, or toggle |

All edge-triggered through E1's `Controls`, like every function key.

`Enter` having a different meaning per view is deliberate and is the alternative to
four more keys. It is one sentence to state — *`Enter` acts on the view you are
looking at* — and each meaning is the only useful action that view has.

⚠️ **`KeySet`'s `u32` does not fit this.** 29 keys hold bits 0-28; five more reach
bit 33. Two ways out and only one of them is honest:

- Overload `PageUp`/`PageDown`/`Home` to mean `[`/`]`/`Enter` while the viewer is
  up, adding only `F9` and `F10`. That reaches 31 keys on bits 0-30, leaving bit 31
  as the `u32`'s last free bit — and one of the things that must fit is
  `scripts/mutate.py`'s control mutant, which moves `Escape` to a free bit
  precisely to prove the bit assignment is tested. Spending the last bit on that
  control leaves the next key with nowhere to go, and the overload itself makes
  three keys' meaning depend on hidden state, which is what E2's `Focus` was
  written to avoid within a single view.
- Widen `bits` to `u64`. One field type, one doc line, `1u64 << k.bit()`. Nothing
  else in the crate cares: `KeySet` is frontend-only and appears in no save state.

Widen it. And the consequence must be actioned, not discovered: **the `keys`
control mutant moves off bit 30, which E3 now occupies.** It goes to bit 62, with
the denominator written into the comment the way the last such move should have
been — 34 keys hold bits 0-33, so everything above is free. A control that dies
because a later task took its bit is a finding, and this is the second time this
harness has produced it.

Three places change together per key — the enum, `Key::ALL`, and `Key::bit` —
which `all_lists_every_key_exactly_once` enforces, and `sfemu`'s `translate` is a
total match that will not compile until each is mapped. All five exist in
`minifb 0.28` as `F9`, `F10`, `LeftBracket`, `RightBracket`, `Enter`.

## Where the code goes

```
crates/video/src/compose.rs        LayerMask, Video::enable, Video::gfx,
                                   layer_enabled and feeds_sprites
                                   published (modified)
crates/frontend/src/gfx.rs         the viewer's state: which view, the cursors
crates/frontend/src/gfxpanels.rs   the pixels: four view drawers
crates/frontend/src/keys.rs        five keys, and KeySet becomes u64 (modified)
crates/frontend/src/font.rs        a greyscale swatch primitive (modified)
crates/sfemu/src/loop_.rs          the loop draws the viewer after E2's panels
crates/sfemu/src/display.rs        five key translations (modified)
```

The split mirrors E2's: `gfx.rs` is state and decisions, `gfxpanels.rs` is pixels,
exactly as `debug.rs` is to `overlay.rs`. Not folded into those two files because
`overlay.rs` is already 985 lines and the video panels are of a size with it.

`font.rs` gains one primitive — a filled swatch with a border — because the palette
and tile views both want it and `fill_rect` alone leaves adjacent swatches of
similar colours indistinguishable.

## The verification problem

Same answer as E1's and E2's: everything that decides anything is in `frontend`
and is asserted against a buffer. What is testable, and how:

- **A browsed tile's pixels are the ROM's.** Against a **synthetic graphics ROM
  built in the test** whose tile 3 encodes a known pen ramp — not against a second
  call to `tile_pen`, which would be the expectation derived from the thing under
  test. The greyscale mapping is pinned with literals for pen 0, 15, and one
  middle value.
- **A palette swatch is the entry's colour.** Read off the buffer, with a few
  entries pinned to hand-written ARGB literals, for the reason `pixels.rs` already
  documents: comparing this to the function it wraps passes with both wrong in the
  same direction.
- **The tilemap view's codes are `tile_info`'s**, over a gfxram written with known
  codes — so a wrong scan mapper or a wrong table base shows up as the wrong code
  in the wrong cell.
- **An unmapped code renders as `----`, not as tile 0.** The one case the picture
  cannot show.
- **The mask can only subtract.** Over every one of the sixteen masks, a layer the
  hardware has disabled stays disabled.
- **The mask changes no machine state.** N frames under every mask, comparing the
  CPU, the board, the cycle count, and the trace. E3's central criterion.
- **The mask is absent from a save state, and survives a load.**
- **The layers view agrees with the renderer**, because it calls
  `layer_enabled` — pinned by a test that disables a layer through the registers
  and requires both the view's cell and the drawn frame to change together.
- **Each view leaves the rest of the frame alone**, by bounding box, as
  `overlay`'s `a_panel_leaves_the_rest_of_the_frame_alone` does.
- **The labels read back as text**, through E2's glyph recogniser.
- **The viewer draws over E2's panels and not under them.**

What is not testable here, and is therefore stated as a user check alongside E1's
three and E2's fourth:

- **Is a tile recognisable in the browser?** A test can prove the pixels are the
  ROM's pens in the right cells. Whether you can look at a 16×16 greyscale tile in
  a 384-wide window and say "that is Ryu's fist" is not a property of a buffer.
- **Are the palette swatches distinguishable?** 3072 swatches on a 384×224 frame
  is about 5×4 pixels each. A test can prove each holds the right colour; it
  cannot prove two adjacent near-identical entries look different to you.

## Risks

1. **The `render` change alters a rendered frame by default.** Highest severity,
   because `video` carries C's tests and the composed-frame assertions, and the
   127/127 vector suite gates anything under `machine`. Mitigated by the mask
   defaulting to `all()` and combining with `&&`, which makes the default path
   bit-identical — and stated as a test: a frame rendered with `LayerMask::all()`
   must equal one rendered by the tree before this change, pinned by the existing
   composition tests passing unmodified.
2. **The `u64` widening touches every key path.** Low severity and broad blast
   radius: `press`, `contains`, and `bit`'s shift. The existing
   `every_key_has_its_own_slot` and `all_lists_every_key_exactly_once` tests
   already cover exactly this, and the mutation set for `keys` is 16 mutants deep.
3. **A view re-derives what the renderer computes.** The defect this branch keeps
   producing. Mitigated structurally rather than by review: the tile view calls
   `tile_pen`, the tilemap view `tile_info` and `cps_a_base` and `BankMapper::map`,
   the palette view `entry_to_rgb`, and the layers view `layer_enabled` and
   `layer_order` — every one of them the renderer's own call. Publishing
   `layer_enabled` exists to make the last of these possible.
4. **The palette view is illegible at 384×224.** Cannot be settled here. If 5×4
   swatches turn out unusable the fix is one page of 512 at a time, which is local
   to `gfxpanels.rs` and costs a use of `[`/`]` that already exists.
5. **`Enter`'s per-view meaning is confusing.** Real, and accepted over five more
   keys. Mitigated by each view labelling what its own `Enter` does, on screen,
   which costs one line per view and removes the need to remember anything.

## Success criteria

E3 is done when, against a ROM set the user supplies:

- `F9` then `F10` reaches four views, each showing SF2's real data: recognisable
  tiles out of the graphics ROM, scroll 2's tile codes and its signed scroll, the
  game's palette, and the four layers with their live enables.
- Turning off scroll 1 in the layers view makes the HUD disappear and the stage
  behind it visible, and turning it back on restores it exactly.
- The tilemap view shows a mapped ROM index for a normal tile and `----` for a
  code no bank range covers.
- The vector suite is 127/127, both test profiles are green, and the mutation pass
  is fully accounted for — every survivor a declared control or a declared
  equivalent.
- **A fixed number of frames run with every view open and every mask combination
  reaches the same machine state as one run with the viewer off.** As in E2, the
  last criterion is the one that matters most and the one a viewer most easily
  fails.
