# Graphics Viewers Implementation Plan (sub-project E3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Look at what the video hardware did — the graphics ROM's tiles, a layer's tile table, the palette, and the layer stack — and subtract layers from the picture without changing the machine.

**Architecture:** A second overlay, cycled one view at a time by `F10`, drawn after E2's panels. `video` publishes four things a view must not re-derive (`layer_enabled`, `feeds_sprites`, `map_axis`, `Video::gfx`) and gains a subtractive `LayerMask` as a field on `Video`. State and decisions live in `crates/frontend/src/gfx.rs`, pixels in `crates/frontend/src/gfxpanels.rs` — the same split E2 has between `debug.rs` and `overlay.rs`. `KeySet`'s `bits` widens to `u64`. No new dependency in any crate.

**Tech Stack:** Rust 2021, rust-version 1.93. `video` reached as `machine::video`, never past `machine`.

**Spec:** `docs/superpowers/specs/2026-08-08-graphics-viewers-design.md`

## Global Constraints

- **No ROM is bundled, fetched, downloaded, or committed by any code in this repository, for any purpose — including diagnostics and test fixtures.** Every automated test builds its graphics ROM as a `Vec<u8>` the test itself writes. No URL to any ROM appears anywhere.
- **No new dependency in any crate.** `frontend` has exactly one dependency, `machine`, and must keep exactly one. It must **not** gain `video` in its manifest — `machine` does `pub use video`, and reaching past `machine` is what that re-export exists to prevent. `pixels.rs` already imports `machine::video::compose::Video`; every new import follows it.
- **`minifb` appears in exactly one file: `crates/sfemu/src/display.rs`.** `the_windowing_library_is_named_in_one_file` enforces it.
- **No logic behind the display boundary.** Every E3 decision — which view, where the cursor is, what a key does, what colour a swatch is — lives in `frontend`. `display.rs` gains only five new arms of the total `Key` match.
- **No clock access outside `display.rs`.**
- **The viewer must not change what the machine computes.** Every `frontend` entry point takes `&Cps1`. The `LayerMask` is computed in `frontend` and assigned by the loop; Task 6's `looking_at_the_video_does_not_change_the_machine` is the criterion.
- **The mask subtracts only.** A mask bit can never enable a layer the hardware has disabled. `render` combines them with `&&`, hardware first.
- **`LayerMask::all()` must be bit-identical to the tree before this change.** Task 1 proves it by leaving every existing `video` and `machine` test unmodified and green, and re-running the vector suite.
- **Expected values in tests are written as literals**, never derived by calling the code under test or its inverse. A view's output is read back **off the buffer** with `font::read_text`, or compared against a hand-written bitmap — never against the same `format!` or the same `tile_pen` call the view made.
- **`#![forbid(unsafe_code)]` and `#![warn(missing_docs)]`** hold in `frontend` and `video`; every new public item is documented.
- **rustdoc cannot resolve `cfg(test)` items.** Refer to tests with plain code spans (`` `tests::foo` ``), never `[`tests::foo`]`.
- **The gate before every commit:** `cargo fmt --all` (first), then `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo test --workspace --release`, `cargo doc --no-deps --workspace`. All clean. **Any task touching `video` or `machine` also runs `cargo run -q -p testrunner --release --bin report -- --test suite` and must report 127/127** — that is Task 1.
- **No test is `#[ignore]`d and no test reads an environment variable to decide whether to run.** The single existing exception is `crates/sfemu/tests/boot.rs`.
- **No test opens a window.**
- **Commit per task, not per session.** After compaction you cannot tell your own uncommitted work from a stranger's.
- **`scripts/mutate.py` edits files in place.** Commit before running it, and revert with `shutil.copy`, never `git checkout`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/video/src/compose.rs` | Modify. `LayerMask`; `Video::enable`, `Video::gfx`; `layer_enabled`, `feeds_sprites`, `DEPTHS` published. |
| `crates/video/src/layers.rs` | Modify. `map_axis` extracted from `draw_tilemap` and published. |
| `crates/frontend/src/keys.rs` | Modify. Five keys; `KeySet::bits` widens to `u64`. |
| `crates/frontend/src/font.rs` | Modify. `swatch`, a filled rectangle with a border. |
| `crates/frontend/src/gfxpanels.rs` | Create. The four views' pixels and their layout. |
| `crates/frontend/src/gfx.rs` | Create. `GfxViewer`: which view, the cursors, the mask. |
| `crates/frontend/src/lib.rs` | Modify. Two new modules. |
| `crates/sfemu/src/loop_.rs` | Modify. The loop drives the viewer and assigns the mask. |
| `crates/sfemu/src/display.rs` | Modify. Five new arms in `translate`. |
| `scripts/mutate.py` | Modify. Two new sets, `gfx` and `gfxpanels`; `keys`' control moves off bit 30. |
| `README.md` | Modify. E3 complete; the five keys; the two new user checks. |

**Task order and why:** `video` first (Task 1), because everything consumes it and it is the only part that can break the 127/127 suite — a break found in Task 1 is cheap and one found in Task 5 is a bisect. Then the keys (Task 2), which is the `u64` widening and is independent of every view. Then the pixels (Task 3) and the state (Task 4) — pixels first, because `gfx.rs`'s cursors are defined in terms of what a view can display. Then the loop (Task 5), the first task producing something a human can see. Then the non-disturbance criterion and the mutation pass (Task 6), then the README (Task 7).

---

### Task 1: `video` publishes what a viewer must not re-derive

**Files:**
- Modify: `crates/video/src/compose.rs` (`DEPTHS`, `Video`, `render`, `layer_enabled`)
- Modify: `crates/video/src/layers.rs` (`draw_tilemap`'s opening arithmetic)
- Test: both files' `mod tests`

**Interfaces:**
- Produces:
  ```rust
  // compose.rs
  pub const DEPTHS: usize = 4;                       // was private

  /// Which layers the caller permits to draw. Subtractive only.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct LayerMask {
      pub sprites: bool,
      pub scroll1: bool,
      pub scroll2: bool,
      pub scroll3: bool,
  }
  impl LayerMask {
      pub const fn all() -> Self;
      /// Whether this mask permits `Some(layer)`, or the sprites for `None`.
      pub const fn permits(&self, layer: Option<Layer>) -> bool;
  }
  impl Default for LayerMask { fn default() -> Self { Self::all() } }

  pub struct Video { pub enable: LayerMask, /* ...unchanged */ }
  impl Video { pub fn gfx(&self) -> &[u8]; }

  pub fn layer_enabled(cfg: &VideoConfig, layer: Layer,
                       layercontrol: u16, videocontrol: u16) -> bool;  // was private
  pub fn feeds_sprites(order: &[u8; DEPTHS], depth: usize) -> bool;

  // layers.rs
  /// `(tile index modulo the map's 64, offset within that tile)` for one axis.
  pub fn map_axis(edge: u32, raster: i32) -> (u32, u32);
  ```
- Consumes: nothing new.

⚠️ **This is the highest-risk task in the plan.** `render` is on the path every `video` composition test and every board-level video test runs through, and `machine` re-exports it. The default must be bit-identical.

- [ ] **Step 1: Write the failing test that pins the default, before touching `render`**

In `compose.rs`'s `mod tests`. This is the test that makes the whole task safe, so it comes first:

```rust
    /// The default mask changes not one pixel.
    ///
    /// The whole risk of this task in one assertion: `render` gains a condition, and
    /// a condition that was wrong in the default case would change every rendered
    /// frame in the workspace — the composition tests here, the board tests in
    /// `machine`, and the 127/127 vector suite, all at once and all in ways that look
    /// like a video bug rather than like this change.
    ///
    /// The two frames are rendered by the *same* code, so this cannot prove the
    /// default equals the old tree by itself; what proves that is every existing test
    /// in `video` and `machine` passing **unmodified**. This pins the property going
    /// forward: `LayerMask::all()` is the identity.
    #[test]
    fn the_default_mask_is_the_identity() {
        let (gfxram, cps_a, cps_b, gfx) = a_busy_frame();
        let mut a = a_video(gfx.clone());
        assert_eq!(a.enable, LayerMask::all(), "the default is everything");
        a.render(&gfxram, &cps_a, &cps_b);
        let want = a.fb.pens.clone();

        let mut b = a_video(gfx);
        b.enable = LayerMask::all();
        b.render(&gfxram, &cps_a, &cps_b);
        assert_eq!(&b.fb.pens[..], &want[..], "an explicit all() is the default");
        // And the premise: the frame has something in it, or this compares two
        // background-filled buffers.
        assert!(
            want.iter().any(|&p| p != crate::palette::BACKGROUND_PEN),
            "the premise: the fixture draws something"
        );
    }

    /// The mask subtracts and never adds.
    ///
    /// Over all sixteen masks: a layer the hardware has disabled must stay disabled
    /// whatever the mask says. A mask combined with `||` instead of `&&` would pass
    /// every other test in this file — it only differs where the hardware says no,
    /// and every other fixture here enables what it draws.
    #[test]
    fn the_mask_can_only_subtract() {
        // Scroll 2 disabled in hardware: its layer-control bit clear.
        let (gfxram, cps_a, mut cps_b, gfx) = a_busy_frame();
        cps_b[VideoConfig::sf2().layer_control] &= !0x10;

        for bits in 0u8..16 {
            let mut v = a_video(gfx.clone());
            v.enable = LayerMask {
                sprites: bits & 1 != 0,
                scroll1: bits & 2 != 0,
                scroll2: bits & 4 != 0,
                scroll3: bits & 8 != 0,
            };
            v.render(&gfxram, &cps_a, &cps_b);
            // Scroll 2's colour schemes are 0x40..=0x5F, so its pens are
            // 0x400..=0x5FF and no other layer can produce one.
            assert!(
                !v.fb.pens.iter().any(|&p| (0x400..=0x5FF).contains(&p)),
                "mask {bits:04b} drew a layer the hardware disabled"
            );
        }
    }
```

`a_busy_frame` and `a_video` are helpers this step also writes: a gfxram with a tile table and a palette, CPS registers enabling all three layers with a known `layer_order`, and a graphics ROM whose tiles are non-transparent. **Reuse the existing fixtures in this file if they already do this** — read `compose.rs`'s `mod tests` first and extend rather than duplicate.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p video the_default_mask_is_the_identity the_mask_can_only_subtract`
Expected: FAIL to compile — no `LayerMask`, no `Video::enable`.

- [ ] **Step 3: Add `LayerMask` and the field**

```rust
/// Which layers the caller permits to draw.
///
/// **Subtractive only.** A `false` hides a layer the hardware has enabled; a `true`
/// cannot show one the hardware has disabled. [`Video::render`] combines this with
/// [`layer_enabled`] using `&&`, hardware first, and the direction is not a
/// stylistic choice: forcing a layer on would draw a tilemap from a base register
/// the guest never set up, producing garbage indistinguishable from the
/// tile-decode bug a graphics viewer exists to rule out. "Show me only scroll 2" is
/// reached by clearing the other three.
///
/// This is a *view* setting and not machine state. Nothing the 68000 or the board
/// reads depends on the framebuffer, so a mask changes the picture and not the
/// machine — `sfemu`'s `looking_at_the_video_does_not_change_the_machine` is what
/// holds that line. It must not appear in a save state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerMask {
    /// Draw the sprites.
    pub sprites: bool,
    /// Draw scroll 1, the 8×8 layer.
    pub scroll1: bool,
    /// Draw scroll 2, the 16×16 layer.
    pub scroll2: bool,
    /// Draw scroll 3, the 32×32 layer.
    pub scroll3: bool,
}

impl LayerMask {
    /// Everything drawn: the default, and the identity.
    pub const fn all() -> Self {
        Self {
            sprites: true,
            scroll1: true,
            scroll2: true,
            scroll3: true,
        }
    }

    /// Whether this mask permits a layer — or the sprites, for `None`.
    ///
    /// `Option<Layer>` and not a fifth enum: `layer_order`'s vocabulary is already
    /// "the sprites or one of three layers", and a new enum would need converting at
    /// both call sites.
    pub const fn permits(&self, layer: Option<Layer>) -> bool {
        match layer {
            None => self.sprites,
            Some(Layer::Scroll1) => self.scroll1,
            Some(Layer::Scroll2) => self.scroll2,
            Some(Layer::Scroll3) => self.scroll3,
        }
    }
}

impl Default for LayerMask {
    /// [`LayerMask::all`] — **not** the derived all-`false`, which would render a
    /// blank screen for every caller that never heard of a mask.
    fn default() -> Self {
        Self::all()
    }
}
```

Add to `Video`, with the field documented, and initialise it in `Video::new` with `LayerMask::all()` — spelled out rather than `Default::default()`, so the value is visible at the one place it is set.

- [ ] **Step 4: Wire the mask into `render`, and publish the three functions**

In `render`'s depth loop, the sprite arm and the tilemap arm each gain one condition:

```rust
                0 => {
                    if !self.enable.permits(None) {
                        continue;
                    }
                    draw_sprites(/* unchanged */)
                }
                n => {
                    let layer = /* unchanged */;
                    // Hardware first, then the view's subtraction. `&&`, never
                    // `||`: see `LayerMask`.
                    if !layer_enabled(&self.cfg, layer, layercontrol, videocontrol)
                        || !self.enable.permits(Some(layer))
                    {
                        continue;
                    }
```

Then: `const DEPTHS` → `pub const DEPTHS` with a doc comment, `fn layer_enabled` → `pub fn layer_enabled` (its existing doc comment already explains it and needs one added sentence naming who else calls it), and `feeds_sprites` extracted:

```rust
/// Whether the layer at `depth` prepares the sprite occlusion mask.
///
/// True when the *next* depth is the sprites. `render_layers` calls
/// `cps1_render_high_layer(..., l0)` only `if (l1 == 0)`, and so on for each pair
/// (`cps1_v.cpp:2985-2996`), so a layer with no sprite pass behind it — including
/// the frontmost, which has no next depth at all — occludes nothing.
///
/// Published because a graphics viewer must display this and a second
/// `order.get(depth + 1) == Some(&0)` written elsewhere is a second answer.
pub fn feeds_sprites(order: &[u8; DEPTHS], depth: usize) -> bool {
    order.get(depth + 1) == Some(&0)
}
```

`render` calls it. And `Video::gfx`:

```rust
    /// The graphics ROM, for a viewer that decodes tiles straight out of it.
    ///
    /// `&[u8]` and not `&Vec<u8>`: every consumer wants a slice, and `tile_pen`
    /// takes one.
    pub fn gfx(&self) -> &[u8] {
        &self.gfx
    }
```

- [ ] **Step 5: Run the two new tests and the whole `video` suite**

Run: `cargo test -p video`
Expected: PASS, **with no existing test modified.** If an existing test needed a change, stop — that is the risk this task is about, and the change is not behaviour-preserving.

- [ ] **Step 6: Write the failing test for `feeds_sprites`**

```rust
    /// `feeds_sprites` is true exactly where the next depth is the sprites.
    ///
    /// Including the frontmost depth, which has no next depth: `order.get(3 + 1)` is
    /// `None`, not `Some(&0)`, and a `depth + 1 == 4` written as an index would panic
    /// there rather than answering false.
    #[test]
    fn only_the_layer_below_the_sprites_feeds_them() {
        // Sprites at depth 2: scroll1, scroll3, sprites, scroll2.
        let order = [1u8, 3, 0, 2];
        assert!(!feeds_sprites(&order, 0), "two depths above the sprites");
        assert!(feeds_sprites(&order, 1), "immediately below them");
        assert!(!feeds_sprites(&order, 2), "the sprites themselves");
        assert!(!feeds_sprites(&order, 3), "the frontmost, with no next depth");
        // Sprites frontmost: the layer at depth 2 feeds them.
        assert!(feeds_sprites(&[1, 2, 3, 0], 2));
        assert!(!feeds_sprites(&[1, 2, 3, 0], 3), "the sprites themselves");
    }
```

Run: `cargo test -p video only_the_layer_below_the_sprites_feeds_them` → PASS (the extraction is already in place from Step 4; this test pins it).

- [ ] **Step 7: Write the failing test for `map_axis`**

In `layers.rs`'s `mod tests`:

```rust
    /// `map_axis` is `draw_tilemap`'s own coordinate arithmetic, and the negative
    /// case is the point of it.
    ///
    /// Four decisions live here and a viewer that re-derived them would get one
    /// wrong: the wrap at [`MAP_TILES`], `div_euclid` rather than `/`, `rem_euclid`
    /// rather than `%`, and no bias of its own (the caller adds `VISIBLE_X` or
    /// `VISIBLE_Y`). Every expectation below is computed by hand.
    #[test]
    fn map_axis_is_euclidean_and_wraps_at_the_map_edge() {
        // 16-pixel tiles. Raster 0 is tile 0, pixel 0.
        assert_eq!(map_axis(16, 0), (0, 0));
        assert_eq!(map_axis(16, 15), (0, 15), "the last pixel of tile 0");
        assert_eq!(map_axis(16, 16), (1, 0), "the first pixel of tile 1");
        assert_eq!(map_axis(16, 40), (2, 8));

        // Negative: `/` truncates toward zero and would give tile 0 for −1, with
        // `%` giving pixel −1 — which as a `u32` is 4294967295. Euclidean gives
        // the last pixel of the tile *before* tile 0, which after the wrap is 63.
        assert_eq!(map_axis(16, -1), (63, 15));
        assert_eq!(map_axis(16, -16), (63, 0));
        assert_eq!(map_axis(16, -64), (60, 0), "SF2's bootleg scroll1xoff");

        // The wrap: 64 tiles of 16 pixels is 1024, and 1024 is tile 0 again.
        assert_eq!(map_axis(16, 1024), (0, 0));
        assert_eq!(map_axis(16, 1024 + 40), (2, 8), "the same as raster 40");

        // And the other two edges, whose spans differ.
        assert_eq!(map_axis(8, -1), (63, 7), "8-pixel tiles: 64 * 8 = 512");
        assert_eq!(map_axis(8, 512), (0, 0));
        assert_eq!(map_axis(32, -1), (63, 31), "32-pixel: 64 * 32 = 2048");
        assert_eq!(map_axis(32, 2048), (0, 0));
    }
```

Run: `cargo test -p video map_axis_is_euclidean` → FAIL, `map_axis` not found.

- [ ] **Step 8: Extract `map_axis` and call it from both axes of `draw_tilemap`**

```rust
/// One axis of a layer's map coordinate: `(tile, offset within the tile)`.
///
/// `edge` is the layer's tile edge in pixels and `raster` a **raster** position —
/// the caller has already added [`crate::VISIBLE_X`] or [`crate::VISIBLE_Y`] and the
/// scroll. Signed throughout, with Euclidean division, so a negative scroll is the
/// same arithmetic as a positive one rather than a branch: 0xFFC0 is −64, and
/// `-1 / 16` truncating to 0 would put the wrong tile on screen at the left edge.
///
/// The tile wraps at [`MAP_TILES`], because a layer's map is 64×64 and a scroll
/// past its span shows the map again.
///
/// Published because a graphics viewer must name the tile the renderer fetched, and
/// four decisions live in these two lines — the bias's absence, `div_euclid`,
/// `rem_euclid`, and the wrap. This crate had to correct the raster bias three
/// times; a viewer with a fourth reading of it would report a tile that was never
/// drawn, which is a diagnostic that lies exactly when it is being trusted.
pub fn map_axis(edge: u32, raster: i32) -> (u32, u32) {
    let step = edge as i32;
    let tile = raster.div_euclid(step).rem_euclid(MAP_TILES as i32) as u32;
    let offset = raster.rem_euclid(step) as u32;
    (tile, offset)
}
```

In `draw_tilemap`, replace the row computation with `let (row, ty) = map_axis(edge, map_y);` and the column with `let (col, tx0) = map_axis(edge, map_x);` — noting that `tx0` is currently an `i32` used in `step - tx0` and `tx0 + k`, so those two expressions become `step - tx0 as i32` and `tx0 + k as u32`. **Keep the existing comment about why the two `rem_euclid`s cannot be killed by a test**, moved onto `map_axis` where the arithmetic now lives.

- [ ] **Step 9: Run the whole workspace and the vector suite**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace && cargo test --workspace --release
cargo doc --no-deps --workspace
cargo run -q -p testrunner --release --bin report -- --test suite
```

Expected: all clean, and **`groups: 127/127`**. Every existing test unmodified.

- [ ] **Step 10: Commit**

```bash
git add crates/video/src/compose.rs crates/video/src/layers.rs
git commit -m "feat(video): a subtractive layer mask, and publish what a viewer must not re-derive"
```

---

### Task 2: Five keys, and `KeySet` becomes a `u64`

**Files:**
- Modify: `crates/frontend/src/keys.rs`
- Modify: `crates/sfemu/src/display.rs` (`translate`, and its test's candidate list)
- Test: both files' `mod tests`

**Interfaces:**
- Produces: `Key::{GfxToggled, GfxView, BracketLeft, BracketRight, Enter}` on bits 29-33; `KeySet { bits: u64 }`; five new `Actions` fields.
- Consumes: nothing new.

- [ ] **Step 1: Write the failing test for the widening**

```rust
    /// Every key's bit fits the set, and the set is wide enough for the next one.
    ///
    /// `every_key_has_its_own_slot` proves the bits are distinct; it does not prove
    /// they are *reachable*. `1u32 << 33` is a shift overflow — a debug-build panic
    /// and a release-build wrap to bit 1, which would silently alias `GfxView` to
    /// `Down`. So the width is asserted against the highest bit any key uses.
    #[test]
    fn every_key_fits_the_set_with_room_left() {
        let highest = Key::ALL.iter().map(|k| k.bit()).max().expect("29+ keys");
        assert!(
            highest < u64::BITS,
            "key bit {highest} does not fit a u64 KeySet"
        );
        // Not merely "fits": every key must round-trip through a set, which a
        // wrapped shift would break by aliasing two keys onto one bit.
        for k in Key::ALL {
            let s = KeySet::from_keys(&[k]);
            assert!(s.contains(k), "{k:?} on bit {} does not round-trip", k.bit());
            let others = Key::ALL.iter().filter(|&&o| o != k).count();
            assert_eq!(
                Key::ALL.iter().filter(|&&o| s.contains(o)).count(),
                1,
                "{k:?} aliases one of the other {others} keys"
            );
        }
    }
```

`Key::bit` is currently private (`const fn bit`). This test needs it, so make it `pub(crate) const fn bit` — not `pub`: outside the crate a key's bit is not a fact anyone needs, and `mutate.py`'s control mutant edits the `match` arms in place, which `pub(crate)` does not affect.

Run: `cargo test -p frontend every_key_fits_the_set_with_room_left`
Expected: FAIL — with 29 keys the highest bit is 28, so this passes vacuously *before* the new keys exist. **Add the five keys first, in Step 2, and run this test between the enum change and the widening**, where it fails on the shift.

- [ ] **Step 2: Add the five keys, and see the test fail**

Three places, in this order:

```rust
    /// Show or hide the graphics viewer.
    GfxToggled,
    /// Cycle which graphics view is shown.
    GfxView,
    /// Page or move back within the graphics view.
    BracketLeft,
    /// Page or move forward within the graphics view.
    BracketRight,
    /// Act on the current graphics view.
    Enter,
```

`Key::ALL` becomes `[Key; 34]` with the five appended, and `bit` gains:

```rust
            Key::GfxToggled => 29,
            Key::GfxView => 30,
            Key::BracketLeft => 31,
            Key::BracketRight => 32,
            Key::Enter => 33,
```

Run: `cargo test -p frontend`
Expected: FAIL. `1u32 << 32` is a shift overflow — in a debug build, a panic naming it, which is exactly the failure the widening fixes.

- [ ] **Step 3: Widen `bits` to `u64`**

```rust
/// Which keys are held.
///
/// A bitmask rather than a `Vec`, so [`Controls`] can keep last frame's set by copy
/// and the edge detection is one `&`.
///
/// `u64` and not `u32`: 34 keys hold bits 0-33. It was a `u32` through E2's 29 keys,
/// and the alternative to widening was overloading `PageUp`/`PageDown`/`Home` to
/// mean something else while the graphics viewer is up — which would have reached 31
/// keys, leaving exactly one free bit, and `scripts/mutate.py`'s control mutant needs
/// a free bit to move `Escape` to. A `u64` is one field type and 30 bits to spare.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeySet {
    bits: u64,
}
```

and `press`/`contains` become `1u64 << k.bit()`. `bit` keeps returning `u32` — `<<` takes any integer on the right, and `u32` is what `u64::BITS` compares against.

Run: `cargo test -p frontend` → the new test passes; `sfemu` still fails to compile, because `translate` is a total match.

- [ ] **Step 4: Add the five `Actions` fields and their edges**

```rust
    /// Show or hide the graphics viewer.
    pub gfx_toggled: bool,
    /// Cycle to the next graphics view.
    pub gfx_view_cycled: bool,
    /// Move back within the graphics view.
    pub gfx_back: bool,
    /// Move forward within the graphics view.
    pub gfx_forward: bool,
    /// Act on the current graphics view — cycle its tile kind or layer, or toggle.
    pub gfx_act: bool,
```

In `update`, all five `edge(...)`, beside E2's. **Edge-triggered, every one**: a held `]` walking sixty pages a second is not a way to find a tile, which is the reasoning already written on `scroll_up`.

- [ ] **Step 5: Extend `every_control_action_is_edge_triggered`**

Read that test first — it is a table of `(Key, fn(&Actions) -> bool)` pairs. Add the five. **Do not write a new test for this**; the existing one is the enforcement point and a second one would be the copy that drifts.

Also extend `no_key_presses_a_player_two_control` and `the_dip_switches_are_never_touched` if they enumerate keys — they iterate `Key::ALL`, so they need no change, which is what `all_lists_every_key_exactly_once` is for.

- [ ] **Step 6: Add the five arms to `translate`, and to its candidate list**

```rust
        M::F9 => Key::GfxToggled,
        M::F10 => Key::GfxView,
        M::LeftBracket => Key::BracketLeft,
        M::RightBracket => Key::BracketRight,
        M::Enter => Key::Enter,
```

⚠️ **`every_frontend_key_can_be_produced_by_a_keypress` asserts `translate(M::F9) == None`** with the comment "including a neighbouring function key". That assertion is now false. Replace it with a key that is still unmapped — `M::F11` — and keep the comment's intent.

Add all five to that test's `candidates` array.

- [ ] **Step 7: Run the gate**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace && cargo test --workspace --release
cargo doc --no-deps --workspace
```

- [ ] **Step 8: Commit**

```bash
git add crates/frontend/src/keys.rs crates/sfemu/src/display.rs
git commit -m "feat(keys): five graphics-viewer keys, and KeySet widens to u64"
```

---

### Task 3: The four views' pixels

**Files:**
- Create: `crates/frontend/src/gfxpanels.rs`
- Modify: `crates/frontend/src/font.rs` (`swatch`)
- Modify: `crates/frontend/src/lib.rs` (`pub mod gfxpanels;`)
- Test: `crates/frontend/src/gfxpanels.rs` (`mod tests`), `crates/frontend/src/font.rs`

**Interfaces:**
- Produces:
  ```rust
  // font.rs
  /// A filled cell with a one-pixel border, for a colour swatch.
  pub fn swatch(buf: &mut [u32], x: usize, y: usize, w: usize, h: usize,
                fill: u32, border: u32);

  // gfxpanels.rs
  /// Which graphics view is shown.
  pub enum View { Tiles, Tilemap, Palette, Layers }
  impl View { pub const fn cycled(self) -> Self; pub const fn name(self) -> &'static str; }

  /// Everything a view needs that is not the machine.
  pub struct ViewState {
      pub view: View,
      pub kind: TileKind,       // the tile view's layout
      pub layer: Layer,         // the tilemap view's layer
      pub tile_at: u32,         // the tile view's first ROM index
      pub pal_at: usize,        // the palette view's cursor
      pub map_at: Option<(u32, u32)>,  // the tilemap cursor, None to follow the beam
      pub row: usize,           // the layers view's selected row
      pub mask: LayerMask,
  }

  /// Draws the current view over whatever is in `buf`.
  pub fn draw(buf: &mut [u32], m: &Cps1, s: &ViewState);

  /// The tile the renderer fetches at the visible top-left of `layer`.
  pub fn map_origin(m: &Cps1, layer: Layer) -> (u32, u32);
  ```
- Consumes: `font::{draw_text, fill_rect, swatch, ADVANCE, LINE}`; `machine::video::{tiles::{tile_pen, TileKind, TRANSPARENT_PEN}, layers::{Layer, map_axis, tile_info, MAP_TILES, PEN_GRANULARITY}, bank::GfxType, palette::{self, entry_to_rgb}, compose::{LayerMask, layer_enabled, layer_order, feeds_sprites, DEPTHS}, regs::{cps_a_base, SCROLL_BOUNDARY, VIDEOCONTROL, SCROLL1_BASE, SCROLL1_X, SCROLL1_Y, SCROLL2_BASE, SCROLL2_X, SCROLL2_Y, SCROLL3_BASE, SCROLL3_X, SCROLL3_Y}, WIDTH, HEIGHT, VISIBLE_X, VISIBLE_Y}`.

⚠️ **This is the largest task in the plan and it is one task, not four, because the four views share their layout constants, their box-drawing helper, and their test fixture.** Splitting it would put the shared parts in whichever view landed first.

- [ ] **Step 1: Add `swatch` to `font.rs`, test first**

```rust
    /// A swatch is its fill, inside its border.
    ///
    /// `fill_rect` alone is not enough: two adjacent swatches of similar colours are
    /// one indistinguishable block, and the palette view draws 3072 of them.
    #[test]
    fn a_swatch_is_a_fill_inside_a_border() {
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        swatch(&mut buf, 10, 20, 5, 4, 0x0011_2233, 0x00FF_FFFF);
        // The border is the outermost ring.
        assert_eq!(buf[20 * WIDTH + 10], 0x00FF_FFFF, "top-left corner");
        assert_eq!(buf[20 * WIDTH + 14], 0x00FF_FFFF, "top-right corner");
        assert_eq!(buf[23 * WIDTH + 10], 0x00FF_FFFF, "bottom-left corner");
        assert_eq!(buf[21 * WIDTH + 10], 0x00FF_FFFF, "left edge");
        assert_eq!(buf[20 * WIDTH + 12], 0x00FF_FFFF, "top edge");
        // The fill is what the border encloses.
        assert_eq!(buf[21 * WIDTH + 11], 0x0011_2233, "the interior");
        assert_eq!(buf[22 * WIDTH + 13], 0x0011_2233, "the interior");
        // And nothing outside.
        assert_eq!(buf[19 * WIDTH + 10], 0, "one row above");
        assert_eq!(buf[20 * WIDTH + 15], 0, "one column right");
        assert_eq!(buf[24 * WIDTH + 10], 0, "one row below");
    }

    /// A swatch too small for a border is all border, not a panic.
    ///
    /// The palette view's swatches are about 5×4 and a narrower window would make
    /// them 1×1. An interior computed as `w - 2` would underflow.
    #[test]
    fn a_swatch_smaller_than_its_border_is_all_border() {
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        swatch(&mut buf, 0, 0, 1, 1, 0x0011_2233, 0x00FF_FFFF);
        assert_eq!(buf[0], 0x00FF_FFFF, "a 1x1 swatch is its border");
        swatch(&mut buf, 0, 2, 2, 2, 0x0011_2233, 0x00FF_FFFF);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert_eq!(buf[(2 + dy) * WIDTH + dx], 0x00FF_FFFF, "2x2 is all border");
        }
    }
```

Then:

```rust
/// A filled cell with a one-pixel border, for a colour swatch.
///
/// Clipped like [`fill_rect`]. A swatch under 3 pixels in either axis is all border:
/// the interior is `w.saturating_sub(2)` wide, so a 1×1 swatch is one border pixel
/// rather than an underflow.
pub fn swatch(buf: &mut [u32], x: usize, y: usize, w: usize, h: usize, fill: u32, border: u32) {
    fill_rect(buf, x, y, w, h, border);
    fill_rect(
        buf,
        x + 1,
        y + 1,
        w.saturating_sub(2),
        h.saturating_sub(2),
        fill,
    );
}
```

Run: `cargo test -p frontend swatch` → PASS.

- [ ] **Step 2: Move `panel_contains` and `frame` into `font.rs`**

Every test below reads its result back off the buffer with `panel_contains`, and it currently lives in `overlay.rs`'s test module, private. Copying it into `gfxpanels.rs` would be the second copy of a glyph scanner — and `read_text`, which it is built on, is already `#[cfg(test)] pub(crate)` in `font.rs` precisely so it can be shared.

Move both, unchanged, beside `read_text`:

```rust
/// Whether `needle` appears on any glyph row of `buf`, in `fg`.
///
/// Scans every candidate baseline *and* every horizontal phase, so a test asserting
/// some text is present does not also have to know which row and column it landed on
/// — that is what the `read_text` assertions against exact coordinates are for.
/// `ADVANCE` phases is enough: a panel's columns are `x0 + i * ADVANCE`, so starting
/// the scan at `x0 % ADVANCE` reads exactly its cells, whatever `x0` is.
///
/// Lives here rather than in `overlay`'s tests because `gfxpanels` reads its views
/// back the same way, and a second glyph scanner is a second answer.
#[cfg(test)]
pub(crate) fn panel_contains(buf: &[u32], needle: &str, fg: u32) -> bool { /* unchanged */ }

/// An empty frame.
#[cfg(test)]
pub(crate) fn frame() -> Vec<u32> {
    vec![0u32; WIDTH * HEIGHT]
}
```

and in `overlay.rs`'s test module replace both definitions with `use crate::font::{frame, panel_contains};`. `cargo test -p frontend` must stay green with no other change — that is what makes this a move rather than a rewrite.

- [ ] **Step 3: Write the module's layout constants and the shared box helper**

The viewer owns the whole frame, unlike E2's corner panels. One box, full width, with a title line naming the view and what `Enter` does in it:

```rust
/// The viewer's box: the whole frame, inset by two pixels.
const VX: usize = 2;
/// Ditto.
const VY: usize = 2;
/// Ditto.
const VW: usize = WIDTH - 4;
/// Ditto.
const VH: usize = HEIGHT - 4;

/// Background: darker than E2's panels, because this box is the whole screen and
/// E2's sits on top of it — two identical backgrounds would make the boundary
/// between them invisible.
const BG: u32 = 0x0000_0010;
/// Ordinary text.
const FG: u32 = 0x00D0_D0D0;
/// A heading, and the cursored item.
const HI: u32 = 0x0060_FF60;
/// A value the hardware says no to: a disabled layer, an unmapped code.
const OFF: u32 = 0x00FF_6060;
/// A swatch's border.
const EDGE: u32 = 0x0080_8080;
/// Padding inside the box.
const PAD: usize = 2;
```

`draw` fills the box, draws the title line, then dispatches on `s.view`.

- [ ] **Step 4: Write the tile view's failing test**

The fixture is a **synthetic graphics ROM built by the test**, encoding a known pen per pixel — and this is where the expectation must be a literal rather than a `tile_pen` call:

```rust
    /// A graphics ROM in which tile `t`'s pixel `(x, y)` has pen `(x + y) & 0x0F`.
    ///
    /// Written by encoding the layout rule from `tiles.rs`'s module documentation
    /// *forwards*, from pen to bits — the opposite direction to `tile_pen`, which
    /// decodes. Two independent directions through one rule: a bug in either shows
    /// as a disagreement, where a fixture built by calling `tile_pen` could not
    /// disagree with it at all.
    ///
    /// ```text
    /// bit = y * (4 * FW) + 32 * (x >> 3) + (x & 7) + [24, 16, 8, 0][plane]
    /// ```
    /// Plane 0 supplies the pen's most significant bit.
    fn gfx_rom(kind: TileKind, tiles: u32) -> Vec<u8> {
        let bytes = kind.bytes();
        let mut rom = vec![0u8; bytes * tiles as usize];
        let fw = match kind {
            TileKind::Tile32x32 => 32u32,
            _ => 16,
        };
        let bias = match kind {
            TileKind::Tile8x8Odd => 32u32,
            _ => 0,
        };
        for t in 0..tiles as usize {
            for y in 0..kind.size() {
                for x in 0..kind.size() {
                    let pen = ((x + y) & 0x0F) as u8;
                    let base = y * 4 * fw + 32 * (x >> 3) + (x & 7) + bias;
                    for (p, off) in [24u32, 16, 8, 0].into_iter().enumerate() {
                        if pen & (0x08 >> p) != 0 {
                            let bit = base + off;
                            rom[t * bytes + (bit / 8) as usize] |= 0x80 >> (bit % 8);
                        }
                    }
                }
            }
        }
        rom
    }

    /// The tile view draws the ROM's pens as greyscale, at the cells the layout says.
    ///
    /// Pen 0 black, pen 15 white, and the ramp between — pinned as literals, because
    /// a greyscale mapping compared against the function that computes it passes with
    /// both wrong.
    #[test]
    fn a_browsed_tile_is_the_roms_pens_in_grey() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        let mut buf = frame();
        let s = ViewState {
            view: View::Tiles,
            kind: TileKind::Tile16x16,
            tile_at: 0,
            ..base_state()
        };
        draw(&mut buf, &m, &s);

        // Tile 0's cell origin, from the layout. Pixel (x, y) of it has pen
        // `(x + y) & 0x0F`, so the diagonal x + y == 0 is pen 0 and x + y == 15
        // is pen 15.
        let (ox, oy) = tile_cell(0);
        assert_eq!(buf[oy * WIDTH + ox], grey_literal(0), "pen 0 is black");
        assert_eq!(
            buf[oy * WIDTH + ox + 15],
            grey_literal(15),
            "pen 15 is white"
        );
        assert_eq!(buf[(oy + 8) * WIDTH + ox], grey_literal(8), "pen 8 is mid");
        assert_eq!(
            buf[(oy + 4) * WIDTH + ox + 4],
            grey_literal(8),
            "(4,4) is also pen 8"
        );
    }

    /// The greyscale ramp, as hand-written literals.
    ///
    /// Sixteen shades from black to white. Not `pen * 17` computed in the test —
    /// that is the implementation, and a test that recomputes it cannot fail.
    fn grey_literal(pen: u8) -> u32 {
        let v = [
            0x00u32, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
            0xEE, 0xFF,
        ][pen as usize];
        (v << 16) | (v << 8) | v
    }
```

⚠️ `grey_literal` is a table and the implementation must be the same table, not `pen * 17`. If the implementation computes it, this test is the same expression written twice. **Write the implementation as a `const GREYS: [u32; 16]` of hand-written ARGB literals** and the test's table as the sixteen `0x00`..`0xFF` values — then the two agree only if both are right.

- [ ] **Step 5: Write the tile view, and its `Enter` and paging**

Grid of `kind.size()`-pixel cells across the box's width, as many rows as fit, each labelled with its ROM index in hex every fourth column (a label per tile does not fit at 8 pixels). `tile_at` is the first index; `[`/`]` move by a screenful; `Enter` cycles `TileKind`.

The pens come from `tile_pen(m.video.gfx(), s.kind, index, x, y)` — the renderer's own call.

Run: `cargo test -p frontend a_browsed_tile` → PASS.

- [ ] **Step 6: Write the palette view's failing test**

```rust
    /// A palette swatch is the entry's colour, through the window's own conversion.
    ///
    /// Pinned against hand-written ARGB, for the reason `pixels.rs` documents about
    /// itself: compared only against `entry_to_rgb`, this would pass with both wrong
    /// in the same direction.
    #[test]
    fn a_palette_swatch_is_the_entrys_colour() {
        // Entry 0xFFFF is full brightness, full white; 0xF00F is full-brightness
        // blue; 0x0FFF is a third-brightness white. Every value hand-computed from
        // `bright = 0x0f + ((e >> 12) << 1)`, `c = nibble * 0x11 * bright / 0x2d`.
        let m = a_machine_with_palette(&[(0, 0xFFFF), (1, 0xF00F), (2, 0x0FFF)]);
        let mut buf = frame();
        draw(&mut buf, &m, &ViewState { view: View::Palette, ..base_state() });

        assert_eq!(swatch_fill(&buf, 0), 0x00FF_FFFF, "0xFFFF is white");
        assert_eq!(swatch_fill(&buf, 1), 0x0000_00FF, "0xF00F is blue");
        // Third brightness: bright = 0x0f, 0x0F * 0x11 * 0x0f / 0x2d = 0x55.
        assert_eq!(swatch_fill(&buf, 2), 0x0055_5555, "0x0FFF is a third white");
    }

    /// The background pen is marked, because "the screen is a colour I did not
    /// expect" is a palette question and 0xBFF is its answer.
    #[test]
    fn the_background_pen_is_marked() {
        let m = a_machine_with_palette(&[]);
        let mut buf = frame();
        draw(&mut buf, &m, &ViewState { view: View::Palette, ..base_state() });
        assert!(
            panel_contains(&buf, "BG 0BFF", HI),
            "the background pen is named"
        );
    }
```

`swatch_fill` reads the interior pixel of swatch `n` from the buffer, using the same layout the implementation publishes as `pal_cell(n) -> (usize, usize)`. That is a shared layout function, not a duplicated one — the same relationship `overlay`'s tests have to `REGS_X`.

- [ ] **Step 7: Write the palette view**

3072 swatches. At `VW = 380` and 512 per page, six rows of 512 does not fit one swatch per column; the layout is 64 columns × 48 rows of 5×4 cells, page-labelled down the left edge. The cursored entry's raw hex and its page are on the title line. `[`/`]` move `pal_at`.

Colours come from `entry_to_rgb(m.video.palette()[n])` composed into ARGB the same way `pixels::argb` does — **and `argb` is private to `pixels.rs`.** Make it `pub(crate) fn argb` and call it, rather than writing the shift a second time: one conversion, as the spec requires.

- [ ] **Step 8: Write the tilemap view's failing tests**

```rust
    /// The tilemap view shows `tile_info`'s codes at the cells the map says.
    #[test]
    fn the_tilemap_view_shows_the_tables_codes() {
        // gfxram with scroll 2's table at word 0 and a known code at map (3, 1).
        let mut m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        let i = 2 * Layer::Scroll2.scan(3, 1);
        m.board.gfxram[i] = 0x0123;
        m.board.gfxram[i + 1] = 0x0045; // colour 5, no flip, group 0
        let mut buf = frame();
        draw(&mut buf, &m, &ViewState {
            view: View::Tilemap,
            layer: Layer::Scroll2,
            map_at: Some((3, 1)),
            ..base_state()
        });
        assert!(panel_contains(&buf, "0123", HI), "the cursored code");
        assert!(panel_contains(&buf, "COL 45", FG), "and its colour scheme");
    }

    /// A code no bank range covers reads `----`, not tile 0.
    ///
    /// The one failure the picture cannot show: `draw_tilemap` skips an unmapped
    /// tile silently, which is correct and undiagnosable. A viewer that showed the
    /// mapper's `None` as 0 would send you looking at tile 0.
    #[test]
    fn an_unmapped_code_is_not_shown_as_tile_zero() {
        let mut m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        // Scroll 2 has no bank range in `STF29_RANGES` at all, so every scroll-2
        // code is unmapped on this board — which is itself worth asserting, since
        // it is why SF2's scroll 2 draws from the sprite ROM ranges.
        let i = 2 * Layer::Scroll2.scan(0, 0);
        m.board.gfxram[i] = 0xFFFF;
        let mut buf = frame();
        draw(&mut buf, &m, &ViewState {
            view: View::Tilemap,
            layer: Layer::Scroll2,
            map_at: Some((0, 0)),
            ..base_state()
        });
        assert!(panel_contains(&buf, "ROM ----", OFF), "an unmapped code");
        assert!(
            !panel_contains(&buf, "ROM 0000", OFF),
            "and not shown as tile 0"
        );
    }

    /// The cursor's default is the tile the renderer draws at the top-left pixel.
    ///
    /// **Not asserted by calling `map_axis` twice** — that would compare the view to
    /// itself. A distinctive tile is placed at a known map position, the scroll set
    /// so the renderer fetches *that* tile for visible pixel (0, 0), and the cursor
    /// required to name it. The scroll is negative, which is the case that separates
    /// `div_euclid` from `/`: with truncating division the answer is tile 0.
    #[test]
    fn the_cursor_follows_the_tile_at_the_visible_top_left() {
        let mut m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        // Scroll 2 x = -80, y = -32. Visible pixel (0, 0) is raster
        // (VISIBLE_X, VISIBLE_Y) = (64, 16), so the map position is
        // (64 - 80, 16 - 32) = (-16, -16) — one tile left and one tile up of the
        // origin, which after the wrap at 64 is map tile (63, 63).
        m.board.cps_a[machine::video::regs::SCROLL2_X] = (-80i16) as u16;
        m.board.cps_a[machine::video::regs::SCROLL2_Y] = (-32i16) as u16;
        assert_eq!(
            map_origin(&m, Layer::Scroll2),
            (63, 63),
            "one tile left and up of the origin, wrapped"
        );
        // And the truncating answer, which is what a re-derived viewer produces.
        assert_ne!(map_origin(&m, Layer::Scroll2), (0, 0));
    }
```

- [ ] **Step 9: Write the tilemap view and `map_origin`**

```rust
/// The map tile the renderer fetches for the visible top-left pixel of `layer`.
///
/// The tilemap view's cursor default. `map_axis` is called rather than re-derived,
/// because the raster bias, the Euclidean division, and the wrap at 64 are four
/// decisions the renderer has already made — see `video`'s `map_axis`.
pub fn map_origin(m: &Cps1, layer: Layer) -> (u32, u32) {
    let (sx, sy) = match layer {
        Layer::Scroll1 => (SCROLL1_X, SCROLL1_Y),
        Layer::Scroll2 => (SCROLL2_X, SCROLL2_Y),
        Layer::Scroll3 => (SCROLL3_X, SCROLL3_Y),
    };
    // `as i16` before widening: 0xFFC0 is −64, not 65472. The registers are
    // unsigned words holding signed scrolls, which is the trap `compose.rs`
    // documents at length.
    let x = VISIBLE_X + i32::from(m.board.cps_a[sx] as i16);
    let y = VISIBLE_Y + i32::from(m.board.cps_a[sy] as i16);
    let edge = layer.tile_edge();
    (map_axis(edge, x).0, map_axis(edge, y).0)
}
```

The view itself: the table's word base from `cps_a_base`, the signed scrolls, a window of codes around the cursor from `tile_info`, and for the cursored tile its colour, flips, group, the mapper's answer (`----` in `OFF` when `None`), and the tile drawn in greyscale beside it. `Enter` cycles `layer`, which resets `map_at` to `None`.

- [ ] **Step 10: Write the layers view's failing test**

```rust
    /// The layers view's enable column is the renderer's answer, not its own.
    ///
    /// Disabling scroll 1 through the registers must change both the view's cell and
    /// the drawn frame. A view that re-derived "is scroll 1 enabled" could pass the
    /// first half and fail the second, which is the whole reason `layer_enabled` is
    /// public.
    #[test]
    fn the_layers_view_agrees_with_the_renderer() {
        let mut m = a_machine(gfx_rom(TileKind::Tile8x8, 8));
        let mut buf = frame();
        draw(&mut buf, &m, &ViewState { view: View::Layers, ..base_state() });
        assert!(panel_contains(&buf, "S1 ON", FG), "enabled in hardware");

        // Scroll 1's layer-control bit is `layer_enable_mask[0]` = 0x08 on SF2.
        m.board.cps_b[machine::video::regs::VideoConfig::sf2().layer_control] &= !0x08;
        let mut buf = frame();
        draw(&mut buf, &m, &ViewState { view: View::Layers, ..base_state() });
        assert!(panel_contains(&buf, "S1 OFF", OFF), "and now disabled");
    }

    /// Sprites read `ALWAYS`, because CPS-1 has no sprite enable bit.
    ///
    /// A fact about the hardware, on the screen, because "why can I not turn the
    /// sprites off in hardware" is the question the table otherwise invites.
    #[test]
    fn the_sprites_have_no_hardware_enable() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 8));
        let mut buf = frame();
        draw(&mut buf, &m, &ViewState { view: View::Layers, ..base_state() });
        assert!(panel_contains(&buf, "OB ALWAYS", FG));
    }
```

- [ ] **Step 11: Write the layers view**

Four rows — `OB`, `S1`, `S2`, `S3` — each with the hardware enable (from `layer_enabled`, or `ALWAYS` for sprites), the mask's bit, the depth from `layer_order`, and whether it feeds the sprites (from `feeds_sprites`). The selected row in `HI`.

- [ ] **Step 12: The bounding-box test, and the two frame-integrity tests**

```rust
    /// Every view stays inside the frame and inside its box.
    ///
    /// The same claim `overlay`'s `a_panel_leaves_the_rest_of_the_frame_alone`
    /// makes, and for the same reason: a view that ran one pixel past its box would
    /// look like a rendering bug in the game.
    #[test]
    fn every_view_stays_inside_its_box() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 64));
        for view in [View::Tiles, View::Tilemap, View::Palette, View::Layers] {
            let mut buf = vec![0x00FF_00FFu32; WIDTH * HEIGHT];
            draw(&mut buf, &m, &ViewState { view, ..base_state() });
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let inside = x >= VX && x < VX + VW && y >= VY && y < VY + VH;
                    if !inside {
                        assert_eq!(
                            buf[y * WIDTH + x],
                            0x00FF_00FF,
                            "{view:?} touched ({x}, {y}), outside its box"
                        );
                    }
                }
            }
        }
    }

    /// A view draws something. The premise every assertion above rests on.
    #[test]
    fn every_view_draws_something() {
        let m = a_machine(gfx_rom(TileKind::Tile16x16, 64));
        for view in [View::Tiles, View::Tilemap, View::Palette, View::Layers] {
            let mut buf = frame();
            draw(&mut buf, &m, &ViewState { view, ..base_state() });
            assert!(
                buf.iter().any(|&p| p != 0),
                "{view:?} drew nothing at all"
            );
            assert!(
                panel_contains(&buf, view.name(), HI),
                "{view:?} names itself on its title line"
            );
        }
    }

    /// Drawing a view does not disturb the machine.
    ///
    /// Every entry point takes `&Cps1`, which the compiler enforces — and the
    /// tilemap view reads memory, so the behavioural claim is worth making too:
    /// `peek_word`'s trap is that a `&mut self` read would acknowledge an interrupt,
    /// and a view that reached for the bus instead of gfxram would do the same.
    #[test]
    fn drawing_a_view_does_not_disturb_the_machine() {
        let mut m = a_machine(gfx_rom(TileKind::Tile16x16, 64));
        let before = (
            m.total_cycles,
            m.board.trace.acks,
            m.board.trace.unmapped_reads.total(),
            m.cpu.pc,
        );
        let mut buf = frame();
        for view in [View::Tiles, View::Tilemap, View::Palette, View::Layers] {
            draw(&mut buf, &m, &ViewState { view, ..base_state() });
        }
        assert_eq!(
            before,
            (
                m.total_cycles,
                m.board.trace.acks,
                m.board.trace.unmapped_reads.total(),
                m.cpu.pc,
            ),
            "a view read the machine through something with side effects"
        );
    }
```

`unmapped_reads` is an `UnmappedLog`, whose count is `total()` — not `len()`. It also has `entries()` and `dropped()`; `total()` is the one that cannot be flattened by the log's own capacity cap.

- [ ] **Step 13: Run the gate and commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace && cargo test --workspace --release
cargo doc --no-deps --workspace
git add crates/frontend/src/gfxpanels.rs crates/frontend/src/font.rs \
        crates/frontend/src/pixels.rs crates/frontend/src/lib.rs
git commit -m "feat(frontend): the four graphics views, drawn from the renderer's own calls"
```

---

### Task 4: The viewer's state

**Files:**
- Create: `crates/frontend/src/gfx.rs`
- Modify: `crates/frontend/src/lib.rs` (`pub mod gfx;`)
- Test: `crates/frontend/src/gfx.rs` (`mod tests`)

**Interfaces:**
- Produces:
  ```rust
  pub struct GfxViewer { /* private: on, and a ViewState */ }
  impl GfxViewer {
      pub fn new() -> Self;
      /// Applies one frame's viewer keys. Returns whether the viewer is shown.
      pub fn update(&mut self, a: &Actions, m: &Cps1) -> bool;
      /// The mask the loop must give `Video`.
      pub fn mask(&self) -> LayerMask;
      /// Draws the current view, if shown.
      pub fn draw(&self, buf: &mut [u32], m: &Cps1);
      pub fn shown(&self) -> bool;
  }
  ```
- Consumes: Task 2's five `Actions` fields; Task 3's `ViewState`, `View`, `draw`, `map_origin`.

This is `debug.rs`'s shape: state and decisions, no pixels, everything on `&Cps1`.

- [ ] **Step 1: Write the failing tests**

```rust
    /// `F9` shows and hides it, and it starts hidden.
    ///
    /// Hidden because it covers the whole screen, and a viewer that appeared
    /// uninvited would make the emulator look broken on first run — the same
    /// reasoning `Debugger::new` carries.
    #[test]
    fn the_viewer_starts_hidden_and_f9_toggles_it() {
        let m = a_machine();
        let mut g = GfxViewer::new();
        assert!(!g.shown(), "hidden on first run");
        assert!(g.update(&act(|a| a.gfx_toggled = true), &m));
        assert!(g.shown());
        assert!(!g.update(&act(|a| a.gfx_toggled = true), &m));
    }

    /// `F10` cycles all four views and returns to the first.
    #[test]
    fn f10_cycles_the_four_views_and_comes_back() {
        let m = a_machine();
        let mut g = GfxViewer::new();
        let mut seen = vec![g.view()];
        for _ in 0..4 {
            g.update(&act(|a| a.gfx_view_cycled = true), &m);
            seen.push(g.view());
        }
        assert_eq!(seen[4], seen[0], "four cycles return to the start");
        let distinct: std::collections::HashSet<_> = seen[..4].iter().collect();
        assert_eq!(distinct.len(), 4, "and the four are distinct: {seen:?}");
    }

    /// `Enter` acts on the view you are looking at, and on no other.
    ///
    /// The one key with four meanings. A dispatch that acted on the wrong view would
    /// cycle a tile kind while you were looking at the palette, which reads as the
    /// key doing nothing.
    #[test]
    fn enter_acts_on_the_current_view_only() {
        let m = a_machine();
        let mut g = GfxViewer::new();
        // On Tiles, Enter cycles the tile kind and touches nothing else.
        let before = (g.state().layer, g.state().row, g.state().mask);
        g.update(&act(|a| a.gfx_act = true), &m);
        assert_ne!(g.state().kind, TileKind::Tile16x16, "the kind moved");
        assert_eq!(
            (g.state().layer, g.state().row, g.state().mask),
            before,
            "and nothing else did"
        );
        // On Layers, Enter toggles the selected row's mask bit.
        while g.view() != View::Layers {
            g.update(&act(|a| a.gfx_view_cycled = true), &m);
        }
        let kind = g.state().kind;
        g.update(&act(|a| a.gfx_act = true), &m);
        assert_ne!(g.mask(), LayerMask::all(), "a mask bit toggled");
        assert_eq!(g.state().kind, kind, "and the tile kind did not move");
    }

    /// The mask only ever subtracts, whatever the keys do.
    ///
    /// `GfxViewer` cannot produce a mask that enables something: `all()` is the
    /// start and every toggle clears or restores a bit. Asserted by exhausting the
    /// four rows and requiring the result to be a subset of `all()`, which is what
    /// "subtractive" means at this layer.
    #[test]
    fn the_viewer_can_only_subtract() {
        let m = a_machine();
        let mut g = GfxViewer::new();
        while g.view() != View::Layers {
            g.update(&act(|a| a.gfx_view_cycled = true), &m);
        }
        for _ in 0..4 {
            g.update(&act(|a| a.gfx_act = true), &m);
            g.update(&act(|a| a.gfx_forward = true), &m);
        }
        assert_eq!(
            g.mask(),
            LayerMask {
                sprites: false,
                scroll1: false,
                scroll2: false,
                scroll3: false
            },
            "all four subtracted"
        );
        // And toggling back restores exactly, with nothing else changed.
        for _ in 0..4 {
            g.update(&act(|a| a.gfx_act = true), &m);
            g.update(&act(|a| a.gfx_forward = true), &m);
        }
        assert_eq!(g.mask(), LayerMask::all(), "and restored");
    }

    /// The tilemap cursor follows the beam until you move it, then stops.
    ///
    /// `Option<(u32, u32)>` and not a pair kept equal to the origin: the same
    /// distinction `Debugger::disasm_at` documents. A cursor moved to a cell that
    /// happens to be the origin must not resume following, or scrolling the layer
    /// would yank the view away from the tile you were reading.
    #[test]
    fn the_tilemap_cursor_stops_following_once_moved() {
        let m = a_machine();
        let mut g = GfxViewer::new();
        while g.view() != View::Tilemap {
            g.update(&act(|a| a.gfx_view_cycled = true), &m);
        }
        assert_eq!(g.state().map_at, None, "following the beam");
        g.update(&act(|a| a.gfx_forward = true), &m);
        assert!(g.state().map_at.is_some(), "and now pinned");
    }

    /// Cycling the tilemap's layer returns the cursor to following.
    ///
    /// A cursor kept across a layer change would point at a cell of a map with a
    /// different tile size, which is a coordinate that means nothing.
    #[test]
    fn changing_the_tilemap_layer_returns_the_cursor_to_the_beam() {
        let m = a_machine();
        let mut g = GfxViewer::new();
        while g.view() != View::Tilemap {
            g.update(&act(|a| a.gfx_view_cycled = true), &m);
        }
        g.update(&act(|a| a.gfx_forward = true), &m);
        assert!(g.state().map_at.is_some());
        g.update(&act(|a| a.gfx_act = true), &m);
        assert_eq!(g.state().map_at, None, "a new layer, a new default");
    }

    /// A hidden viewer ignores its own keys but keeps its mask.
    ///
    /// Two separate claims and both are deliberate. The keys are ignored so `]` does
    /// not silently page a view you cannot see. The mask persists because turning the
    /// *viewer* off is not turning the *layers* back on — you asked to look at the
    /// game with scroll 1 subtracted, and hiding the box is how you do that.
    #[test]
    fn a_hidden_viewer_ignores_keys_but_keeps_its_mask() {
        let m = a_machine();
        let mut g = GfxViewer::new();
        g.update(&act(|a| a.gfx_toggled = true), &m);
        while g.view() != View::Layers {
            g.update(&act(|a| a.gfx_view_cycled = true), &m);
        }
        g.update(&act(|a| a.gfx_act = true), &m);
        let masked = g.mask();
        assert_ne!(masked, LayerMask::all(), "the premise: a bit is subtracted");

        g.update(&act(|a| a.gfx_toggled = true), &m);
        assert!(!g.shown());
        let view = g.view();
        g.update(&act(|a| a.gfx_view_cycled = true), &m);
        assert_eq!(g.view(), view, "a hidden viewer does not cycle");
        assert_eq!(g.mask(), masked, "but it does not forget its mask");
    }

    /// A hidden viewer draws nothing.
    #[test]
    fn a_hidden_viewer_draws_nothing() {
        let m = a_machine();
        let g = GfxViewer::new();
        let mut buf = vec![0u32; WIDTH * HEIGHT];
        g.draw(&mut buf, &m);
        assert!(buf.iter().all(|&p| p == 0), "nothing shown, nothing drawn");
    }
```

`act` is a helper building an `Actions` with one field set: `fn act(f: impl FnOnce(&mut Actions)) -> Actions`. `g.view()` and `g.state()` are accessors this task adds — `state()` is `pub(crate)` or `#[doc(hidden)]`, because outside the crate the `ViewState` is Task 3's business; make it `pub fn state(&self) -> &ViewState` with a doc line saying it is for the loop's `draw` and for tests.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p frontend gfx::` → FAIL to compile.

- [ ] **Step 3: Write `GfxViewer`**

The dispatch, which is the module's substance:

```rust
        if a.gfx_act {
            match self.state.view {
                View::Tiles => self.state.kind = next_kind(self.state.kind),
                View::Tilemap => {
                    self.state.layer = next_layer(self.state.layer);
                    // A new layer's tiles are a different size, so a cell index from
                    // the old one means nothing. Back to following the beam.
                    self.state.map_at = None;
                }
                // The palette's cursor is moved by `[`/`]` and there is nothing else
                // to act on — the view has one axis. Deliberately nothing, and named
                // so, because a silent `_ => {}` reads as an oversight.
                View::Palette => {}
                View::Layers => self.toggle_row(),
            }
        }
```

`next_kind` cycles all four `TileKind`s including `Tile8x8Odd`, and `next_layer` the three. Both written out as `match`es, not as `as`-casts, for the reason `Key::bit` gives.

- [ ] **Step 4: Run the tests, then the gate, then commit**

```bash
cargo test -p frontend
cargo fmt --all && cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace && cargo test --workspace --release
cargo doc --no-deps --workspace
git add crates/frontend/src/gfx.rs crates/frontend/src/lib.rs
git commit -m "feat(frontend): the graphics viewer's state, and one Enter with four meanings"
```

---

### Task 5: The loop drives the viewer

**Files:**
- Modify: `crates/sfemu/src/loop_.rs`
- Test: `crates/sfemu/src/loop_.rs` (`mod tests`)

**Interfaces:**
- Consumes: `frontend::gfx::GfxViewer`.
- Produces: nothing public.

- [ ] **Step 1: Give `Fake` a `last` frame**

`Fake` records `first` only, with the comment that "86,016 pixels a tick is a lot to keep, and one is enough to ask whether it was rendered." E2's tests are all satisfied by the first frame, because E2's panels appear on the same tick their key is pressed. E3's do not: reaching the layers view and subtracting a layer takes five ticks, and the frame that proves it is the last one.

```rust
        /// The first buffer, in full. Only the first: 86,016 pixels a tick is a lot
        /// to keep, and one is enough to ask whether it was rendered.
        first: Option<Vec<u32>>,
        /// And the last, for a claim that takes several ticks to set up: reaching a
        /// graphics view and subtracting a layer is five key presses, and the frame
        /// that shows it is the one after them. Two frames, not sixty.
        last: Option<Vec<u32>>,
```

with `self.last = Some(buf.to_vec());` beside the `first` assignment in `present`, and `last: None` in `new`.

- [ ] **Step 2: Write the failing tests**

```rust
    /// The viewer draws over E2's panels, not under them.
    ///
    /// Both overlays are opaque and they overlap; the order decides which you can
    /// read. The video viewer wins, because it is the whole screen and E2's panels
    /// are corners of it — the other order would leave the viewer with a register
    /// panel punched out of its top-left, which is where its own labels are.
    #[test]
    fn the_video_viewer_draws_over_the_debugger() {
        let (o, _s, _p) = opts("viewer-over-debugger");
        let mut m = machine_that_draws();
        // One tick, both overlays on. A tick reads its keys before it renders, so
        // the frame this tick presents already has both.
        let mut d = Fake::new(vec![Fake::held(&[Key::F1, Key::GfxToggled])]);
        run(&mut m, &mut d, &o);
        let shown = d.first.expect("a tick presents");

        // E2's register panel starts one pixel inside (REGS_X, REGS_Y) and the
        // viewer's box covers it. What must be there is the viewer's background,
        // not the debugger's.
        let px = shown[(frontend::overlay::REGS_Y + 1) * WIDTH + frontend::overlay::REGS_X + 1];
        assert_ne!(px, 0x0000_0020, "the debugger's background is on top");

        // And the premise, or a viewer that drew nothing would pass: with the
        // viewer off, that pixel *is* the debugger's background.
        let mut m = machine_that_draws();
        let mut d = Fake::new(vec![Fake::held(&[Key::F1])]);
        run(&mut m, &mut d, &o);
        let e2 = d.first.expect("a tick presents");
        assert_eq!(
            e2[(frontend::overlay::REGS_Y + 1) * WIDTH + frontend::overlay::REGS_X + 1],
            0x0000_0020,
            "the premise: E2 draws there when the viewer does not"
        );
    }

    /// The mask reaches `Video`, and the frame changes.
    ///
    /// The end-to-end claim of this task: a key press in `sfemu` subtracts a layer
    /// from the pixels the window is given. Everything else in this module tests a
    /// decision; this tests the wire.
    #[test]
    fn subtracting_a_layer_changes_the_presented_frame() {
        let (o, _s, _p) = opts("mask-wired");
        let mut m = machine_that_draws();
        let mut d = Fake::new(Fake::idle(6));
        run(&mut m, &mut d, &o);
        let full = d.last.expect("a tick presents");

        // Now: show the viewer, cycle to the layers view, subtract the selected
        // row, and hide the viewer again — the box would otherwise cover the whole
        // frame and every pixel would differ for the wrong reason. Hiding it also
        // exercises "a hidden viewer keeps its mask".
        //
        // Three `GfxView` presses because the cycle is tiles → tilemap → palette →
        // layers. If Task 3 chose another order this count is wrong, and the
        // assertion below is what says so.
        let mut m = machine_that_draws();
        let mut d = Fake::new(vec![
            Fake::held(&[Key::GfxToggled]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[Key::Enter]),
            Fake::held(&[Key::GfxToggled]),
        ]);
        run(&mut m, &mut d, &o);
        let masked = d.last.expect("a tick presents");
        assert_ne!(
            m.video.enable,
            machine::video::compose::LayerMask::all(),
            "the presses reached the layers view and subtracted a row"
        );
        assert_ne!(masked, full, "subtracting a layer changed the picture");
    }
```

Both scripts are six ticks, so both runs render the same number of frames — the fake advances one frame per tick whether keys are held or not, which is why E2's `watching_the_machine_does_not_change_it` can compare a `held` tick against an `idle` one.

- [ ] **Step 3: Wire it**

Three lines beside E2's three, in the documented order:

```rust
    let mut gfx = GfxViewer::new();
    // ...
        gfx.update(&a, m);
        // The mask is a view setting the loop applies, not something `frontend`
        // reaches into the machine to set: every `frontend` entry point takes
        // `&Cps1`. Before `render`, so this tick's frame is the masked one.
        m.video.enable = gfx.mask();
    // ...
        dbg.draw(&mut buf, m);
        // Over the debugger, not under it: both are opaque, and this one is the
        // whole screen while E2's are corners of it.
        gfx.draw(&mut buf, m);
```

Update the module's numbered ordering comment — it is the design, and a step added without it becomes undocumented.

- [ ] **Step 4: Run the gate and commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace && cargo test --workspace --release
cargo doc --no-deps --workspace
git add crates/sfemu/src/loop_.rs
git commit -m "feat(sfemu): the loop drives the graphics viewer and applies its mask"
```

---

### Task 6: The criterion, and the mutation pass

**Files:**
- Modify: `crates/sfemu/src/loop_.rs` (the non-disturbance test)
- Modify: `crates/machine/src/cps1.rs` (`mod tests`, the save-state pin)
- Modify: `scripts/mutate.py`
- Test: as above

- [ ] **Step 1: Write E3's central test**

Modelled on `watching_the_machine_does_not_change_it`, which you must read first — it restores one machine between two runs because two do not fit on a test thread's stack, and it compares trace figures as deltas:

```rust
    /// **The criterion that matters most:** looking at the video does not change the
    /// machine.
    ///
    /// E2's `watching_the_machine_does_not_change_it` proves the debugger is inert.
    /// E3 changes the *picture* on purpose, which makes the same claim harder and
    /// more important: the mask must reach `Video` and nothing else. Nothing the
    /// 68000 or the board reads depends on the framebuffer, so this is provable
    /// rather than merely intended.
    ///
    /// Compared field by field rather than through `snapshot()`, because a snapshot
    /// leaves out the trace and the trace is where a stray acknowledge would show.
    #[test]
    fn looking_at_the_video_does_not_change_the_machine() {
        let (o, _s, _p) = opts("looking");
        let mut m = machine();
        let start = m.snapshot();

        // Every view, and a subtracted layer. Eight ticks, so the comparison run's
        // eight idle ticks ask for the same frames — the fake advances one frame per
        // tick whether keys are held or not.
        let base = (m.board.trace.acks, m.board.trace.vblanks);
        let mut script = vec![
            Fake::held(&[Key::GfxToggled]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[Key::GfxView]),
            Fake::held(&[Key::Enter]),
        ];
        script.extend(Fake::idle(3));
        let mut d = Fake::new(script);
        let s_on = run(&mut m, &mut d, &o);
        let on = (
            m.total_cycles,
            m.cpu.d,
            m.cpu.a,
            m.cpu.pc,
            m.line,
            m.board.trace.acks - base.0,
            m.board.trace.vblanks - base.1,
        );
        let ram_on = m.board.ram.clone();
        assert_ne!(on.0, 0, "the premise: the machine ran");
        assert_ne!(
            m.video.enable,
            machine::video::compose::LayerMask::all(),
            "the premise: a layer really was subtracted"
        );

        m.restore(&start);
        // `restore` leaves `enable` alone — it is not machine state — so the
        // comparison run must clear it by hand. That this is necessary is itself
        // the point of `the_layer_mask_is_not_machine_state`.
        m.video.enable = machine::video::compose::LayerMask::all();
        let base = (m.board.trace.acks, m.board.trace.vblanks);
        let mut d = Fake::new(Fake::idle(8));
        let s_off = run(&mut m, &mut d, &o);

        assert_eq!(s_on.frames, s_off.frames, "the same frames were asked for");
        assert_eq!(s_on.frames, 8, "and there were some");
        assert_eq!(
            on,
            (
                m.total_cycles,
                m.cpu.d,
                m.cpu.a,
                m.cpu.pc,
                m.line,
                m.board.trace.acks - base.0,
                m.board.trace.vblanks - base.1,
            ),
            "the viewer must not move the machine"
        );
        assert_eq!(ram_on, m.board.ram, "nor write a word of its memory");
    }
```

The `machine()` fixture, not `machine_that_draws()`: this test compares CPU and RAM state, and `machine()` is the diverging program E2's version uses, whose `d0` and RAM move every frame — so two runs that differed at all would differ visibly.

- [ ] **Step 2: Pin the mask out of the save state**

In `crates/machine/src/cps1.rs`'s `mod tests`:

```rust
    /// The layer mask is not machine state.
    ///
    /// It records how you are *looking* at the machine, like the `Trace` records the
    /// session — so a snapshot must not carry it and a restore must not clear it. A
    /// mask that round-tripped through a save state would come back with someone
    /// else's layers subtracted, and one that a load reset would silently undo the
    /// thing you were in the middle of looking at.
    #[test]
    fn the_layer_mask_is_not_machine_state() {
        // `machine()` — this module's fixture is not called `a_machine`.
        let mut m = machine();
        let s = m.snapshot();
        m.video.enable = video::compose::LayerMask {
            sprites: false,
            ..video::compose::LayerMask::all()
        };
        m.restore(&s);
        assert!(
            !m.video.enable.sprites,
            "a restore must not reset the view's mask"
        );
    }
```

⚠️ `MachineState` has no `PartialEq`, so this cannot be asserted by comparing snapshots. The field's absence is enforced by `restore` not touching it, which is what this asserts.

- [ ] **Step 3: Move `keys`' control mutant off bit 30**

E3 assigned bit 30 to `Key::GfxView`, so the control that moves `Escape` there is now a two-keys-share-a-bit mutant the suite will kill. **This is the second time this has happened**; write the denominator down this time:

```python
            # A control: `Escape`'s bit is arbitrary as long as it is unique, so
            # moving it to a free one must not fail anything.
            #
            # It has moved twice, both times because a later task took the bit it
            # was parked on: E2 took 25 for `F7`, and E3 took 30 for `GfxView`.
            # 34 keys hold bits 0-33, and `KeySet` is a `u64`, so everything from
            # 34 up is free — 62 leaves room above and below. This control will die
            # again if a key is ever given bit 62, and that death is the signal it
            # exists for, not a mutant to re-expect.
            (
                "CONTROL-escape-moves-to-another-free-bit",
                "Key::Escape => 21,",
                "Key::Escape => 62,",
                "SURVIVE",
            ),
```

- [ ] **Step 4: Add the `gfx` and `gfxpanels` mutant sets**

Every mutant must be a **one-line exact-match replacement whose pattern occurs exactly once**, and every set needs a control that must SURVIVE. Write them against the code as implemented — read `gfx.rs` and `gfxpanels.rs` before choosing patterns, and run a uniqueness pre-flight over every pattern before running the pass:

```python
import pathlib
for name, (path, muts) in SETS.items():
    src = pathlib.Path(path).read_text()
    for mname, old, _new, _exp in muts:
        n = src.count(old)
        if n != 1:
            print(f"{name}/{mname}: pattern occurs {n} times")
```

`gfx` — at least these, each killed by a named test from Task 4:

| mutant | kill |
|---|---|
| `enter-always-acts-on-the-tile-view` (`match self.state.view` → `match View::Tiles`) | `enter_acts_on_the_current_view_only` |
| `the-view-does-not-cycle-back` (`View::Layers => View::Tiles` → `View::Layers => View::Layers`) | `f10_cycles_the_four_views_and_comes_back` |
| `a-hidden-viewer-still-takes-keys` (the `if !self.on { ... }` guard removed) | `a_hidden_viewer_ignores_keys_but_keeps_its_mask` |
| `toggling-the-viewer-clears-the-mask` (add `self.state.mask = LayerMask::all();`) | same |
| `the-cursor-keeps-following-after-a-move` (`map_at = Some(..)` → `map_at = None`) | `the_tilemap_cursor_stops_following_once_moved` |
| `a-new-layer-keeps-the-old-cursor` (`map_at = None` removed from the layer arm) | `changing_the_tilemap_layer_returns_the_cursor_to_the_beam` |
| `the-viewer-starts-shown` (`on: false` → `on: true`) | `the_viewer_starts_hidden_and_f9_toggles_it` |
| **CONTROL** `the-initial-tile-index-is-arbitrary` (`tile_at: 0` → `tile_at: 0x40`) | must SURVIVE |

`gfxpanels` — at least these:

| mutant | kill |
|---|---|
| `the-tile-pens-are-not-the-roms` (`tile_pen(...)` → `0`) | `a_browsed_tile_is_the_roms_pens_in_grey` |
| `the-greys-are-inverted` (`GREYS[pen]` → `GREYS[15 - pen]`) | same |
| `an-unmapped-code-shows-as-tile-zero` (`.unwrap_or(...)` in place of the `None` arm) | `an_unmapped_code_is_not_shown_as_tile_zero` |
| `the-cursor-ignores-the-scroll` (`map_origin`'s scroll term dropped) | `the_cursor_follows_the_tile_at_the_visible_top_left` |
| `the-cursor-reads-the-scroll-unsigned` (`as i16` dropped) | same |
| `the-cursor-forgets-the-visible-origin` (`VISIBLE_X +` dropped) | same |
| `the-layers-view-derives-its-own-enable` (`layer_enabled(...)` → `layercontrol & 0x08 != 0`) | `the_layers_view_agrees_with_the_renderer` |
| `a-view-runs-past-its-box` (`VW` → `VW + 4`) | `every_view_stays_inside_its_box` |
| **CONTROL** `the-swatch-border-colour-is-arbitrary` (`EDGE` → a different grey) | must SURVIVE |

⚠️ **A mutant must be honest.** Before adding one, ask whether it fails for the reason claimed: a mutant that panics on an assertion unrelated to the property is not a test of that property, and a mutant that is masked by a later line is a no-op dressed as a kill. This is what the `loop` set's overlay-ordering mutant needed three attempts to get right.

- [ ] **Step 5: Commit, then run the whole pass**

Commit first — `mutate.py` edits files in place.

```bash
git add scripts/mutate.py crates/sfemu/src/loop_.rs crates/machine/src/cps1.rs
git commit -m "test: E3's criterion, the mask kept out of save states, and two mutant sets"
python3 scripts/mutate.py --all
```

**`--all` takes about 20 minutes**, exceeds the 600 s foreground limit, and buffers its stdout until exit — so run it in the background and do other work rather than polling, and do not read its output file mid-run.

Expected: **every mutant as expected.** Every survivor is a declared control or a declared equivalent. A control that died is a **harness finding to diagnose**, never a mutant to re-expect — see the memory `run-all-mutant-sets-not-one`.

- [ ] **Step 6: Fix whatever the pass found, and re-run the affected sets**

If a KILL-expected mutant survived, the suite has a gap: write the missing test, do not weaken the expectation. If a control died, diagnose it as a harness finding.

- [ ] **Step 7: Run the full gate and commit any fixes**

Including `cargo run -q -p testrunner --release --bin report -- --test suite` → **127/127**, because Task 1 touched `video`.

---

### Task 7: The README

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the counts and the status**

- "Five sub-projects are complete" → **"Six"**, naming E3 with `F9`/`F10`.
- Remaining: D (Z80 + audio) and F (the SF1 driver) — and keep "**there is no sound.**"
- The mutant count, in both the Getting-started block and the layout block. **Count it, do not guess**: `python3 -c "import ast..."` or simply sum the sets.
- The roadmap: E3 complete, D or F next.

- [ ] **Step 2: Add the five keys to the controls table**

| Key | Does |
|---|---|
| `F9` | Show or hide the graphics viewer |
| `F10` | Cycle the view: tiles, tilemap, palette, layers |
| `[` / `]` | Move within the view |
| `Enter` | Act on the view — cycle its tile layout or layer, or toggle a layer |

- [ ] **Step 3: Write a "Looking at the graphics" section**

The four views and what each answers, and the three facts a reader needs that the screen alone does not give:

- **Tiles are greyscale on purpose.** A colour scheme is the palette's reading of the ROM, and the palette has its own view; tinting the browser would make a wrong decode and a wrong palette look the same.
- **`----` in the tilemap view is the bank mapper saying no.** The tile is *absent*, not wrong, and the composed picture cannot show the difference — `draw_tilemap` skips it silently.
- **Turning a layer off changes the picture, not the machine**, and a screenshot taken with a layer off is missing that layer. Name `looking_at_the_video_does_not_change_the_machine`.

And the asymmetry worth stating: **you cannot turn a layer *on*.** The mask subtracts only, because forcing a layer on would draw a tilemap from a base register the game never set up — garbage that looks exactly like the tile-decode bug the viewer is there to rule out.

- [ ] **Step 4: Extend "Four things only you can check" to six**

- **Is a tile recognisable in the browser?** A test can prove the pixels are the ROM's pens in the right cells. Whether a 16×16 greyscale tile in a 384-wide window reads as a character's fist is not a property of a buffer.
- **Are the palette swatches distinguishable?** 3072 swatches on a 384×224 frame is about 5×4 pixels each. A test can prove each holds the right colour; it cannot prove two adjacent near-identical entries look different to you.

- [ ] **Step 5: Update the layout block**

`crates/frontend/` gains "the graphics viewer's state and its four views"; `crates/video/` gains a mention of the subtractive layer mask.

- [ ] **Step 6: Run the gate and commit**

The README is not code, but `cargo test --workspace` covers the usage-text tests that read it, and `!u.contains("http")` must still pass.

```bash
git add README.md
git commit -m "docs(readme): E3 — the graphics viewers, and the two things only you can see"
```

---

## Self-Review

**Spec coverage.** Walked the spec section by section:

| Spec section | Task |
|---|---|
| Tiles view, greyscale, all four `TileKind`s | 3 (Steps 4-5) |
| Tilemap view: base, signed scroll, codes, attributes, mapper | 3 (Steps 8-9) |
| Palette view: 3072 swatches, raw hex, `BACKGROUND_PEN` marked | 3 (Steps 6-7) |
| Layers view: hardware, debug, depth, feeds-sprites | 3 (Steps 10-11) |
| One view at a time, `F10` cycling | 4 |
| Opaque, drawn over E2 | 5 (Step 2) |
| Layer toggles change the picture, not the machine | 6 (Step 1) |
| The mask subtracts only | 1 (Step 1), 4 (Step 1) |
| The mask is not machine state | 6 (Step 2) |
| `LayerMask`, `Video::enable`, `Video::gfx` | 1 |
| `layer_enabled`, `feeds_sprites`, `DEPTHS`, `map_axis` published | 1 |
| Five keys, `KeySet` → `u64`, the control's bit | 2, 6 (Step 3) |
| Every verification bullet | 1, 3, 4, 5, 6 |
| Two new user checks | 7 (Step 4) |

No spec section is unassigned.

**Placeholder scan.** Every code step carries the code. One ⚠️ remains that is not a placeholder but an instruction whose whole content is "the value here depends on code you must read first": `mutate.py`'s patterns must be written against the implemented source (Task 6 Step 4), with a uniqueness pre-flight given in full. Two others were resolved by reading the tree rather than left as instructions — see below. Task 3 Step 7 names a real edit outside its file list, `pixels::argb` becoming `pub(crate)`, and Step 13's `git add` includes it.

**Type consistency.** `LayerMask` is constructed in five places with the same four field names. `ViewState`'s fields are named identically in Task 3's interface block, Task 3's tests, and Task 4's tests. `map_axis(edge: u32, raster: i32) -> (u32, u32)` has one signature throughout; `draw_tilemap` calls it for both axes and `map_origin` takes `.0` of each. `View` has four variants in one order, and Task 5's keypress-counting test asserts that order rather than assuming it. `Key::bit` becomes `pub(crate)`, which Task 2 Step 1 states and no later task contradicts.

**Five things checked against the tree rather than assumed**, because a plan that names an identifier wrongly costs an implementer a debugging session for nothing:

- `Trace::unmapped_reads` is an `UnmappedLog` whose count is **`total()`**, not `len()`. Task 3's non-disturbance test now says so.
- `Fake` records **`first` only**, and its comment says why. E3's mask claim needs the *last* frame, five key presses in, so Task 5 gained a step that adds `last` beside it with the reason — rather than a ⚠️ telling the implementer to work it out.
- **A `Fake` tick runs one frame whether keys are held or not**, which is what lets E2's `watching_the_machine_does_not_change_it` compare a `held` script against an `idle` one. So Task 5's two scripts are both six ticks and Task 6's are both eight, and both assert the frame count rather than trusting it.
- **`cps1.rs`'s fixture is `machine()`, not `a_machine()`** — `a_machine` is `overlay.rs`'s. Task 6 Step 2 was corrected.
- **`panel_contains` and `frame` are private to `overlay.rs`'s test module**, so `gfxpanels.rs` cannot reach them and every test in Task 3 depends on them. Task 3 gained Step 2, moving both into `font.rs` beside `read_text`, which is already `#[cfg(test)] pub(crate)` for exactly this reason. A move, not a rewrite: `cargo test -p frontend` must stay green with no other change.

**One gap found and closed inline:** Task 4's `enter_acts_on_the_current_view_only` asserts `g.state().kind != TileKind::Tile16x16` after one `Enter`, which requires the initial `kind` to *be* `Tile16x16`. That is now stated: `ViewState`'s default `kind` is `TileKind::Tile16x16` — sprites' and scroll 2's layout, the most common one on this board — and `gfx`'s control mutant moves `tile_at`, not `kind`, so the default is not something a mutant quietly relies on.
