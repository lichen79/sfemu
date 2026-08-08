# Frontend and Save States Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A window that runs SF2 at 59.63 Hz with a keyboard wired to the board's inputs, plus save states that restore a byte-identical machine.

**Architecture:** A new crate `crates/frontend` holding everything that decides *what* to show — the frame pacer, the key map, the pen-to-ARGB conversion, the save-state codec — depending on `machine` and nothing else. `crates/sfemu` gains the run loop behind a `Display` trait, and one module (`display.rs`) that names `minifb` and makes no decisions. The rule the whole design serves: **no logic behind the display boundary.**

**Tech Stack:** Rust 2021, rust-version 1.93. One new dependency, `minifb 0.28`, in `sfemu` only (two runtime crates: `raw-window-handle`, plus `cc` as a build dependency).

**Spec:** `docs/superpowers/specs/2026-08-08-frontend-savestates-design.md`

## Global Constraints

- **No ROM is bundled, fetched, downloaded, or committed by any code in this repository, for any purpose — including diagnostics and test fixtures.** Every automated test uses synthetic data the test itself writes. No URL to any ROM appears anywhere. The usage text must keep passing `!u.contains("http")`.
- **`crates/frontend` depends on `machine` and nothing else.** Not `minifb`, not `romset`, not `video` directly (it reaches `video` through `machine`'s `pub use video`), nothing from crates.io. A `use minifb::Key` anywhere in `frontend` puts the key map behind the display boundary and forfeits the whole testability argument.
- **`crates/machine` gains no new dependency.** It must never depend on `romset` or `frontend`.
- **`minifb` appears in exactly one file: `crates/sfemu/src/display.rs`.** Task 8's mutation pass includes a grep asserting this.
- **No logic behind the display boundary.** `display.rs` contains calls into `minifb` and one total `Key` match. No arithmetic, no state machine, no decisions.
- **No clock access outside `display.rs`.** `Instant::now`, `SystemTime`, and `Duration`-from-now appear nowhere else. The pacer is *fed* elapsed nanoseconds.
- **`#![forbid(unsafe_code)]` and `#![warn(missing_docs)]`** at the top of `crates/frontend/src/lib.rs`, matching `m68k`, `machine`, and `video`. Plus `#![deny(rustdoc::private_intra_doc_links)]`.
- **Expected values in tests are written as literals**, never derived by calling the code under test or its inverse. A save state is checked by **divergence** — restore and re-run, then compare the framebuffer and trace against the first run — never by `snapshot == snapshot`.
- **rustdoc cannot resolve `cfg(test)` items.** Refer to tests with plain code spans (`` `tests::foo` ``), never `[`tests::foo`]`.
- **The gate before every commit:** `cargo fmt --all` (first), then `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo test --workspace --release`. All clean. Sub-project A's 127/127 vector result must stay at 127/127 — re-run `cargo run -q -p testrunner --release --bin report -- --test suite` for any task touching `machine` (Tasks 4 and 5).
- **No test is `#[ignore]`d and no test reads an environment variable to decide whether to run.** The single existing exception is `crates/sfemu/tests/boot.rs`.
- **No test opens a window.** `cargo test` has no display. The loop's tests use a recording fake `Display`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/frontend/Cargo.toml` | Create. One dependency: `machine = { path = "../machine" }`. |
| `crates/frontend/src/lib.rs` | Create. The crate contract, the display-boundary rule, and re-exports. |
| `crates/frontend/src/pace.rs` | Create. `FramePacer`: elapsed nanoseconds → frames owed, with the catch-up cap. |
| `crates/frontend/src/keys.rs` | Create. `Key`, `KeySet`, `Controls`, `Actions`: the map and the edge/level asymmetry. |
| `crates/frontend/src/pixels.rs` | Create. `pens_to_argb`. |
| `crates/frontend/src/state.rs` | Create. The save-state format: `encode`, `decode`, `StateError`, and CRC-32. |
| `crates/machine/src/snapshot.rs` | Create. `MachineState`, `Cps1::snapshot`/`restore`, and the `Board`/`Video` delegates. |
| `crates/machine/src/cps1.rs` | Modify. `snapshot`/`restore` reaching the private `carry`. |
| `crates/machine/src/board.rs` | Modify. `snapshot`/`restore` reaching the private `vblank_pending`. |
| `crates/video/src/compose.rs` | Modify. `Video::snapshot`/`restore` reaching the private `obj`. |
| `crates/sfemu/Cargo.toml` | Modify. Add `frontend` and `minifb`. |
| `crates/sfemu/src/main.rs` | Modify. `--play`, `--state`, and dispatch to the loop. |
| `crates/sfemu/src/loop_.rs` | Create. The `Display` trait, the run loop, and the recording fake. |
| `crates/sfemu/src/display.rs` | Create. The only file naming `minifb`. |
| `Cargo.toml` | Modify. Add `crates/frontend` to `members`. |
| `README.md` | Modify (Task 8). Controls, `--play`, the measured frame cost, the roadmap row. |

Tests live in `#[cfg(test)] mod tests` at the foot of each module, as everywhere else in this workspace.

---

### Task 1: The crate, and the frame pacer

**Files:**
- Create: `crates/frontend/Cargo.toml`
- Create: `crates/frontend/src/lib.rs`
- Create: `crates/frontend/src/pace.rs`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  pub struct FramePacer { /* private */ }
  impl FramePacer {
      pub const fn cps1() -> Self;                      // FRAME_NS, MAX_CATCH_UP
      pub const fn new(frame_ns: u64, max_catch_up: u32) -> Self;
      pub fn tick(&mut self, elapsed_ns: u64) -> u32;
      pub fn dropped(&self) -> u64;
      pub fn reset(&mut self);
  }
  pub const FRAME_NS: u64 = 16_768_000;
  pub const MAX_CATCH_UP: u32 = 4;
  ```

- [ ] **Step 1: Create the manifest**

`crates/frontend/Cargo.toml`:

```toml
[package]
name = "frontend"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish = false
description = "Frame pacing, controls, and save states — everything a frontend decides, with no window"

# One dependency, and it must stay one. `minifb` belongs to `sfemu`: a windowing
# crate here would put the key map and the pacer behind a boundary no test can
# reach, which is the whole reason this crate is separate from `sfemu`. `romset`
# would drag in miniz_oxide. `video` arrives through `machine`'s `pub use video`.
[dependencies]
machine = { path = "../machine" }
```

Add `"crates/frontend",` to the `members` list in the workspace `Cargo.toml`, keeping the list alphabetical (after `crates/frontend` sorts before `crates/m68k`).

- [ ] **Step 2: Write `lib.rs`**

```rust
//! Everything a frontend decides, with no window.
//!
//! # The display boundary
//!
//! A window cannot be asserted about: `cargo test` has no display, and "the right
//! pixels reached the glass" is not something a test can read back. So every
//! decision a frontend makes — how many frames this host tick owes, which board
//! input a key is, what colour a pen is, what bytes a save state is — lives here,
//! in a crate that has never heard of a window. The module that talks to the
//! windowing library lives in `sfemu` and makes no decisions at all.
//!
//! **The rule: no logic behind the display boundary.** A decision made inside the
//! module that calls the windowing library cannot be tested, so it must not be
//! made there.
//!
//! # No clock
//!
//! Nothing here reads a clock. [`FramePacer::tick`] is *given* the elapsed
//! nanoseconds, which is what lets a test drive it through a stalled host and
//! assert exactly how many frames it asks for. The one real clock read in the
//! project is in `sfemu`'s display module.
//!
//! # This crate holds no ROM
//!
//! No ROM is bundled, fetched, or committed, including as a test fixture. The
//! save-state tests build their machine from a program written inline.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![deny(rustdoc::private_intra_doc_links)]

pub mod pace;

pub use pace::{FramePacer, FRAME_NS, MAX_CATCH_UP};
```

- [ ] **Step 3: Write the failing tests**

Create `crates/frontend/src/pace.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The frame period is CPS-1's, as a literal.
    ///
    /// 8,000,000 / (512 × 262) = 59.6374 Hz, so one frame is
    /// 1,000,000,000 × 512 × 262 / 8,000,000 = 16,768,000 ns exactly. Written by
    /// hand rather than computed from `machine::timing`'s constants: the point of
    /// the literal is that it disagrees loudly if the derivation is wrong, and a
    /// figure recomputed from the same three constants agrees with itself whatever
    /// they are.
    ///
    /// The division being exact is the reason this is a `u64` of nanoseconds and
    /// not a float. `machine::timing` asserts the two underlying divisions are
    /// exact; this asserts the consequence.
    #[test]
    fn a_frame_is_16_768_000_nanoseconds() {
        assert_eq!(FRAME_NS, 16_768_000);
        // And it is the same number `machine`'s timing implies, computed here the
        // long way round as an independent check of the literal above.
        let t = machine::Timing::cps1_10mhz();
        assert_eq!(
            1_000_000_000u64 * u64::from(t.lines_per_frame) * 512 / 8_000_000,
            FRAME_NS,
            "512 pixel clocks per line, 8 MHz pixel clock"
        );
        // 59.6374 Hz, to the milli-hertz, so a wrong period is visible as a rate.
        assert_eq!(1_000_000_000_000u64 / FRAME_NS, 59_637);
    }

    /// Exactly one frame's worth of host time owes exactly one frame.
    #[test]
    fn one_frame_of_elapsed_time_owes_one_frame() {
        let mut p = FramePacer::cps1();
        assert_eq!(p.tick(FRAME_NS), 1);
        assert_eq!(p.dropped(), 0);
    }

    /// Less than a frame owes none, and the remainder is kept for later.
    ///
    /// This is the property that makes the pacer a pacer rather than a divider: a
    /// host running at 120 Hz ticks every 8.384 ms, and must produce one frame on
    /// every *second* tick — not zero forever, and not one every tick.
    #[test]
    fn a_short_tick_owes_nothing_but_the_remainder_accumulates() {
        let mut p = FramePacer::cps1();
        let half = FRAME_NS / 2;
        assert_eq!(p.tick(half), 0, "half a frame is not a frame");
        assert_eq!(p.tick(half), 1, "and the two halves together are");
        assert_eq!(p.tick(half), 0);
        assert_eq!(p.tick(half), 1);
    }

    /// Sixty 120 Hz ticks produce thirty frames, not zero and not sixty.
    ///
    /// The loop above checks the alternation over four ticks; this checks the
    /// *rate* over sixty, which is what a dropped remainder would break in a way
    /// four ticks cannot show.
    #[test]
    fn a_fast_host_runs_at_the_right_rate_over_many_ticks() {
        let mut p = FramePacer::cps1();
        let mut frames = 0u32;
        for _ in 0..60 {
            frames += p.tick(FRAME_NS / 2);
        }
        assert_eq!(frames, 30, "sixty half-frames is thirty frames");
        assert_eq!(p.dropped(), 0, "and nothing was dropped");
    }

    /// A slow host owes several frames in one tick.
    #[test]
    fn a_slow_tick_owes_several_frames() {
        let mut p = FramePacer::cps1();
        assert_eq!(p.tick(FRAME_NS * 3), 3);
        assert_eq!(p.dropped(), 0, "three is within the catch-up cap");
    }

    /// The catch-up cap holds, and the debt it refuses is **discarded**.
    ///
    /// This is the test the whole struct exists for. A two-second stall — a
    /// breakpoint, a laptop lid, a slow disk — owes 119 frames. A pacer that
    /// carried that debt would then run flat out for two seconds of fast-forwarded
    /// game, which is the classic emulator bug where pausing the host makes the
    /// game sprint. So the cap both limits the answer *and* zeroes what it did not
    /// serve, and the difference is counted so the drop is visible rather than
    /// silent.
    ///
    /// Every number is a literal: 2,000,000,000 / 16,768,000 = 119.27, so 119
    /// whole frames are owed, 4 are served, and 115 are dropped.
    #[test]
    fn a_stalled_host_is_capped_and_the_refused_debt_is_dropped() {
        let mut p = FramePacer::cps1();
        assert_eq!(MAX_CATCH_UP, 4, "the cap, as a literal");
        assert_eq!(2_000_000_000 / FRAME_NS, 119, "the debt a 2 s stall owes");

        assert_eq!(p.tick(2_000_000_000), 4, "capped at MAX_CATCH_UP");
        assert_eq!(p.dropped(), 115, "119 owed, 4 served, 115 abandoned");

        // And the very next ordinary tick owes exactly one frame — not the 115 a
        // carried debt would still be holding. This is the assertion that
        // distinguishes discarding from carrying; the cap alone does not.
        assert_eq!(p.tick(FRAME_NS), 1, "no fast-forward after the stall");
        assert_eq!(p.dropped(), 115, "and nothing further was dropped");
    }

    /// A tick of exactly the cap's worth of time drops nothing.
    ///
    /// The boundary between "caught up" and "dropped": four frames of debt is
    /// served in full, and only the fifth is refused. Without this, a cap
    /// implemented as `>=` rather than `>` would drop a frame on every ordinary
    /// four-frame hiccup and the count would be noise.
    #[test]
    fn the_cap_is_inclusive_so_a_hiccup_of_exactly_the_cap_drops_nothing() {
        let mut p = FramePacer::cps1();
        assert_eq!(p.tick(FRAME_NS * 4), 4);
        assert_eq!(p.dropped(), 0, "four frames of debt is served, not capped");

        let mut p = FramePacer::cps1();
        assert_eq!(p.tick(FRAME_NS * 5), 4);
        assert_eq!(p.dropped(), 1, "the fifth is the first one refused");
    }

    /// A zero-length tick owes nothing and changes nothing.
    ///
    /// The first iteration of a real loop measures the time since the loop started,
    /// which can be zero. A pacer that owed a frame on a zero tick would run one
    /// frame too many at startup, and one that panicked would crash there.
    #[test]
    fn a_zero_tick_is_harmless() {
        let mut p = FramePacer::cps1();
        assert_eq!(p.tick(0), 0);
        assert_eq!(p.dropped(), 0);
        assert_eq!(p.tick(FRAME_NS), 1, "and the pacer still works after one");
    }

    /// `reset` clears the debt but **keeps** the drop count.
    ///
    /// The debt is where the machine is in the current frame, so unpausing must
    /// start fresh — otherwise the wall-clock time spent paused is owed as game
    /// time, which is the same fast-forward bug in a different disguise. The drop
    /// count is a record of the run and is not state to clear; a `reset` that
    /// cleared it would erase the evidence that the host cannot keep up.
    #[test]
    fn reset_clears_the_debt_and_keeps_the_record() {
        let mut p = FramePacer::cps1();
        p.tick(2_000_000_000);
        assert_eq!(p.dropped(), 115, "the premise");
        p.tick(FRAME_NS / 2); // half a frame of debt outstanding
        p.reset();
        assert_eq!(p.tick(FRAME_NS / 2), 0, "the outstanding half is gone");
        assert_eq!(p.dropped(), 115, "but the record of the stall is not");
    }

    /// The period and the cap come from the constructor, not from constants.
    ///
    /// Every test above uses `cps1()`, so all of them would pass with `frame_ns`
    /// and `max_catch_up` hard-wired to the CPS-1 values. Sub-project F adds a
    /// board, and a pacer that ignored its arguments would pace it at 59.63 Hz
    /// whatever its clocks were.
    #[test]
    fn a_different_period_and_cap_are_honoured() {
        let mut p = FramePacer::new(1_000, 2);
        assert_eq!(p.tick(1_000), 1, "one 1 µs frame");
        assert_eq!(p.tick(10_000), 2, "capped at 2, not 4");
        assert_eq!(p.dropped(), 8, "ten owed, two served");
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p frontend`
Expected: FAIL to compile — `FramePacer` and `FRAME_NS` do not exist.

- [ ] **Step 5: Write the implementation**

Above the test module in `crates/frontend/src/pace.rs`:

```rust
//! Turning host time into emulated frames.
//!
//! # Why the pacer is given the time instead of reading it
//!
//! A loop that calls `Instant::now()` inside itself cannot be tested: there is no
//! way to present it with a two-second stall, and no way to make two runs agree.
//! [`FramePacer::tick`] takes the elapsed nanoseconds as an argument, so the whole
//! of the interesting behaviour — the remainder, the cap, the discard — is an
//! ordinary function of an ordinary number.

/// Nanoseconds per CPS-1 frame: 16,768,000, exactly.
///
/// 512 pixel clocks per line × 262 lines = 134,144 pixel clocks per frame, at
/// 8 MHz. The division is exact, which is why this is an integer and there is no
/// fractional accumulator anywhere in the frontend.
pub const FRAME_NS: u64 = 16_768_000;

/// The most frames one host tick may ask for.
///
/// Four frames is a 67 ms hiccup, which is worth catching up smoothly. Longer than
/// that is better dropped than fast-forwarded: see [`FramePacer::tick`].
pub const MAX_CATCH_UP: u32 = 4;

/// Converts host time into a count of emulated frames.
///
/// Sleep-free and catch-up-bounded. The loop asks "how much time passed" and gets
/// "run this many frames"; holding the rate is the display's business, because the
/// windowing library is what can block on a vsync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePacer {
    frame_ns: u64,
    /// Host time accumulated but not yet paid out as a frame. Always `< frame_ns`
    /// after a `tick`, because whole frames are divided out and any excess past
    /// the cap is discarded.
    owed_ns: u64,
    max_catch_up: u32,
    dropped: u64,
}

impl Default for FramePacer {
    /// [`FramePacer::cps1`].
    fn default() -> Self {
        Self::cps1()
    }
}

impl FramePacer {
    /// A pacer for CPS-1: [`FRAME_NS`] and [`MAX_CATCH_UP`].
    pub const fn cps1() -> Self {
        Self::new(FRAME_NS, MAX_CATCH_UP)
    }

    /// A pacer with an explicit period and cap.
    ///
    /// `const` so a board table can hold one, the same reason
    /// `machine::Timing::cps1_10mhz` is const.
    pub const fn new(frame_ns: u64, max_catch_up: u32) -> Self {
        Self {
            frame_ns,
            owed_ns: 0,
            max_catch_up,
            dropped: 0,
        }
    }

    /// How many emulated frames `elapsed_ns` of host time owes.
    ///
    /// The remainder is kept, so a host ticking faster than the frame rate still
    /// produces frames at the right rate rather than none.
    ///
    /// # Why the excess is discarded rather than carried
    ///
    /// A two-second stall owes 119 frames. Serving four and *carrying* 115 means
    /// the next two seconds run at whatever rate the host can manage, flat out —
    /// the game sprints because the window was behind a breakpoint. So the debt
    /// past the cap is abandoned and counted in [`FramePacer::dropped`], which the
    /// window title reports: a dropped frame should be visible, not silent.
    pub fn tick(&mut self, elapsed_ns: u64) -> u32 {
        self.owed_ns = self.owed_ns.saturating_add(elapsed_ns);
        // `frame_ns` is never zero from either constructor, and a zero would make
        // this a division by zero rather than an infinite loop — so it is worth the
        // one branch to answer "no frames" instead of aborting the process.
        if self.frame_ns == 0 {
            return 0;
        }
        let owed = self.owed_ns / self.frame_ns;
        self.owed_ns %= self.frame_ns;
        let served = u64::from(self.max_catch_up).min(owed);
        self.dropped += owed - served;
        // The cap bounds `served` by `max_catch_up`, a `u32`, so this cannot lose
        // information.
        served as u32
    }

    /// Frames abandoned because the host fell further behind than it may catch up.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Forgets the outstanding debt, keeping the drop count.
    ///
    /// Called when resuming from a pause or a step: the wall-clock time spent
    /// paused is not game time the machine owes. The drop count is a record of the
    /// run rather than state of the pacer, so it survives.
    pub fn reset(&mut self) {
        self.owed_ns = 0;
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p frontend`
Expected: PASS, 10 tests.

- [ ] **Step 7: Run the gate**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --release
```

- [ ] **Step 8: Mutation pass**

Each mutant is one string replacement in `pace.rs`, applied with a `shutil.copy` backup and reverted the same way — **never `git checkout`**, which would destroy uncommitted work elsewhere in the tree. Assert the pattern occurs exactly once before replacing; a pattern that is absent or matches twice is a NO-OP, not a result.

| Mutant | Must |
|---|---|
| `self.dropped += owed - served;` → `self.dropped += 0;` | KILL |
| `let served = u64::from(self.max_catch_up).min(owed);` → `let served = owed;` | KILL |
| `self.owed_ns %= self.frame_ns;` → `self.owed_ns = 0;` | KILL |
| `self.owed_ns = self.owed_ns.saturating_add(elapsed_ns);` → `self.owed_ns = elapsed_ns;` | KILL |
| `pub const FRAME_NS: u64 = 16_768_000;` → `16_667_000` | KILL |
| `pub const MAX_CATCH_UP: u32 = 4;` → `5` | KILL |
| `self.owed_ns = 0;` in `reset` → `self.dropped = 0;` | KILL |
| **Control:** `#[derive(Debug, Clone, PartialEq, Eq)]` → `#[derive(Debug, Clone)]` | SURVIVE |

The control must survive: nothing compares two pacers, so removing the derive changes no behaviour. A pass in which every mutant dies is more likely broken than thorough.

- [ ] **Step 9: Commit**

```bash
git add crates/frontend Cargo.toml
git commit -m "feat(frontend): the frame pacer, and why it never reads a clock"
```

---

### Task 2: Keys, controls, and the edge/level asymmetry

**Files:**
- Create: `crates/frontend/src/keys.rs`
- Modify: `crates/frontend/src/lib.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Key {
      Up, Down, Left, Right,
      A, S, D,          // P1 punches
      Z, X, C,          // P1 kicks
      Num1, Num2, Num5, Num6,
      F2, F3, F5, F8, F12,
      P, Period, Escape,
  }
  impl Key { pub const ALL: [Key; 22]; }

  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
  pub struct KeySet { /* private bitmask */ }
  impl KeySet {
      pub const fn new() -> Self;
      pub fn press(&mut self, k: Key);
      pub fn contains(&self, k: Key) -> bool;
      pub fn from_keys(keys: &[Key]) -> Self;
  }

  #[derive(Debug, Clone, Copy, Default)]
  pub struct Actions {
      pub inputs: machine::Inputs,
      pub pause_toggled: bool,
      pub step: bool,
      pub reset: bool,
      pub save: bool,
      pub load: bool,
      pub screenshot: bool,
      pub quit: bool,
  }

  #[derive(Debug, Clone, Default)]
  pub struct Controls { /* private */ }
  impl Controls {
      pub fn new() -> Self;
      pub fn update(&mut self, now_held: KeySet) -> Actions;
  }
  ```
  `Actions` derives `Default` because `machine::Inputs` has one, hand-written to equal `Inputs::idle()` — the correct all-released value. It does **not** derive `PartialEq`, because `Inputs` does not, and nothing compares two `Actions`. Do not add `PartialEq` to `machine::Inputs` for the frontend's convenience: that widens `machine`'s API for a caller's ergonomics, and nothing in `machine` needs it.

- [ ] **Step 1: Write the failing tests**

Create `crates/frontend/src/keys.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing held is an idle board and no actions.
    #[test]
    fn nothing_held_is_an_idle_board() {
        let mut c = Controls::new();
        let a = c.update(KeySet::new());
        assert_eq!(a.inputs.in0(), 0xFF, "active low: nothing pressed");
        assert_eq!(a.inputs.in1(), 0xFFFF);
        assert_eq!(a.inputs.in2(), 0xFF);
        assert!(!a.pause_toggled && !a.step && !a.reset);
        assert!(!a.save && !a.load && !a.screenshot && !a.quit);
    }

    /// Each game key clears exactly its own port bit, with the expected values as
    /// literals.
    ///
    /// The literals are the point: a map checked by reading the same `Inputs` field
    /// it sets would pass with every key on the wrong button. These values are
    /// `machine::inputs`' own documented bits — `IN1` bits 4-6 for the punches,
    /// `IN2` bits 0-2 for the kicks — computed by hand.
    ///
    /// This also pins the punch/kick split, which is the one part of the map that
    /// is not a free choice: SF2's kicks are read through CPS-B (`IN2`) and its
    /// punches through the port block (`IN1`), so a frontend that put all six on
    /// one port would leave three buttons dead in-game while every test that looked
    /// only at "some bit changed" stayed green.
    #[test]
    fn each_game_key_clears_its_own_port_bit() {
        let one = |k: Key| {
            let mut c = Controls::new();
            c.update(KeySet::from_keys(&[k])).inputs
        };

        // The stick, IN1 bits 0-3.
        assert_eq!(one(Key::Right).in1(), 0xFFFE);
        assert_eq!(one(Key::Left).in1(), 0xFFFD);
        assert_eq!(one(Key::Down).in1(), 0xFFFB);
        assert_eq!(one(Key::Up).in1(), 0xFFF7);

        // Punches, IN1 bits 4-6, left to right on the top row.
        assert_eq!(one(Key::A).in1(), 0xFFEF, "jab");
        assert_eq!(one(Key::S).in1(), 0xFFDF, "strong");
        assert_eq!(one(Key::D).in1(), 0xFFBF, "fierce");
        assert_eq!(one(Key::A).in2(), 0xFF, "a punch is not a kick");

        // Kicks, IN2 bits 0-2, directly beneath.
        assert_eq!(one(Key::Z).in2(), 0xFE, "short");
        assert_eq!(one(Key::X).in2(), 0xFD, "forward");
        assert_eq!(one(Key::C).in2(), 0xFB, "roundhouse");
        assert_eq!(one(Key::Z).in1(), 0xFFFF, "a kick is not a punch");

        // Coins and starts, IN0. MAME's convention: 5 and 6 coin, 1 and 2 start.
        assert_eq!(one(Key::Num5).in0(), 0xFE, "coin 1");
        assert_eq!(one(Key::Num6).in0(), 0xFD, "coin 2");
        assert_eq!(one(Key::Num1).in0(), 0xEF, "start 1");
        assert_eq!(one(Key::Num2).in0(), 0xDF, "start 2");
        assert_eq!(one(Key::F2).in0(), 0xBF, "the test switch, IN0 bit 6");
    }

    /// Two keys at once clear two bits.
    ///
    /// Every case above holds one key, so all of them would pass with an
    /// implementation that overwrote `inputs` instead of accumulating into it —
    /// and holding down-and-punch is the first thing anyone does in a fighting
    /// game.
    #[test]
    fn several_keys_at_once_all_reach_the_board() {
        let mut c = Controls::new();
        let a = c.update(KeySet::from_keys(&[Key::Down, Key::A, Key::Z]));
        assert_eq!(a.inputs.in1(), 0xFFEB, "down (bit 2) and jab (bit 4)");
        assert_eq!(a.inputs.in2(), 0xFE, "and the kick, on its own port");
    }

    /// Player 2 is not mapped, deliberately.
    ///
    /// A default map cannot give P2 a second ten-key cluster on one keyboard, and a
    /// mapping nobody uses is a mapping nobody notices is wrong. The board's P2
    /// half must therefore read as idle no matter which key is held — which is what
    /// this asserts, over every key there is.
    #[test]
    fn no_key_presses_a_player_two_control() {
        for k in Key::ALL {
            let mut c = Controls::new();
            let i = c.update(KeySet::from_keys(&[k])).inputs;
            // P2 occupies IN1's high byte and IN2's bits 4-6.
            assert_eq!(i.in1() >> 8, 0xFF, "{k:?} moved P2's stick or punches");
            assert_eq!(i.in2() | 0x8F, 0x8F, "{k:?} pressed a P2 kick");
        }
    }

    /// The control keys are edge-triggered: held down, they act once.
    ///
    /// This is the whole substance of this module. A held `.` must not step sixty
    /// frames a second and a held F5 must not write sixty save states — while a
    /// held direction must absolutely keep pressing, because holding down is how
    /// you crouch. The asymmetry is deliberate and this test states both halves.
    #[test]
    fn control_keys_fire_once_per_press_and_game_keys_do_not() {
        let mut c = Controls::new();
        let held = KeySet::from_keys(&[Key::Period, Key::Down]);

        let a = c.update(held);
        assert!(a.step, "the first frame of the press steps");
        assert_eq!(a.inputs.in1(), 0xFFFB, "and down is pressed");

        let a = c.update(held);
        assert!(!a.step, "the second frame does not step again");
        assert_eq!(a.inputs.in1(), 0xFFFB, "but down is still pressed");

        let a = c.update(held);
        assert!(!a.step, "nor the third");

        // Release and press again: a second step.
        c.update(KeySet::new());
        let a = c.update(held);
        assert!(a.step, "a fresh press steps again");
    }

    /// Every control key is edge-triggered, not just the one above.
    ///
    /// Checked as a table over all seven, because the natural implementation is one
    /// `edge` helper per action and the natural mistake is to forget it on one of
    /// them — which then works exactly once out of seven, in whichever action the
    /// author tested by hand.
    #[test]
    fn all_seven_control_actions_are_edge_triggered() {
        let cases: [(Key, fn(&Actions) -> bool); 7] = [
            (Key::P, |a| a.pause_toggled),
            (Key::Period, |a| a.step),
            (Key::F3, |a| a.reset),
            (Key::F5, |a| a.save),
            (Key::F8, |a| a.load),
            (Key::F12, |a| a.screenshot),
            (Key::Escape, |a| a.quit),
        ];
        for (k, get) in cases {
            let mut c = Controls::new();
            let held = KeySet::from_keys(&[k]);
            assert!(get(&c.update(held)), "{k:?} must fire on the press");
            assert!(!get(&c.update(held)), "{k:?} must not fire while held");
            assert!(!get(&c.update(held)), "{k:?} still held");
            c.update(KeySet::new());
            assert!(get(&c.update(held)), "{k:?} must fire again after a release");
        }
    }

    /// Two control keys pressed on the same frame both fire.
    ///
    /// The edge tracking is per key, not one "something changed" flag.
    #[test]
    fn two_control_keys_pressed_together_both_fire() {
        let mut c = Controls::new();
        let a = c.update(KeySet::from_keys(&[Key::P, Key::F5]));
        assert!(a.pause_toggled && a.save);
    }

    /// Releasing one control key does not re-arm another.
    ///
    /// With a single "previous frame" set, releasing P while F5 stays held must not
    /// make F5 fire a second time. The natural bug is comparing set *sizes* or
    /// checking "any key released".
    #[test]
    fn releasing_one_key_does_not_refire_another() {
        let mut c = Controls::new();
        let a = c.update(KeySet::from_keys(&[Key::P, Key::F5]));
        assert!(a.pause_toggled && a.save, "the premise");
        let a = c.update(KeySet::from_keys(&[Key::F5]));
        assert!(!a.save, "F5 was held throughout and must not fire again");
        assert!(!a.pause_toggled);
    }

    /// A `KeySet` holds each key independently.
    ///
    /// The set is a bitmask, so the failure mode is two keys sharing a bit — which
    /// would make one key press the other's button and is invisible in any test
    /// that holds one key at a time. Checked over every pair.
    #[test]
    fn every_key_has_its_own_slot() {
        for a in Key::ALL {
            let s = KeySet::from_keys(&[a]);
            assert!(s.contains(a), "{a:?} is not in a set containing it");
            for b in Key::ALL {
                if a != b {
                    assert!(!s.contains(b), "{a:?} and {b:?} share a slot");
                }
            }
        }
        // And the full set contains everything, which a mask that overflowed its
        // width would fail.
        let all = KeySet::from_keys(&Key::ALL);
        for k in Key::ALL {
            assert!(all.contains(k), "{k:?} missing from the full set");
        }
    }

    /// `Key::ALL` lists every variant.
    ///
    /// Three tests above iterate `ALL` and would silently stop covering a key that
    /// was added to the enum and not to the list. The count is a literal, so adding
    /// a variant fails here — the one place that then tells you which tests to
    /// extend.
    #[test]
    fn all_lists_every_key_exactly_once() {
        assert_eq!(Key::ALL.len(), 22, "add new keys to ALL, and to this literal");
        for (i, a) in Key::ALL.iter().enumerate() {
            for b in &Key::ALL[i + 1..] {
                assert_ne!(a, b, "{a:?} appears twice");
            }
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p frontend`
Expected: FAIL to compile — `Key`, `KeySet`, `Controls` do not exist.

- [ ] **Step 3: Write the implementation**

Above the test module in `crates/frontend/src/keys.rs`:

```rust
//! The keyboard map, and the difference between holding a button and pressing a key.
//!
//! # Two kinds of input, and why they must not be treated alike
//!
//! A **game** input is level-triggered: the board reads what is held right now,
//! because holding down is how you crouch. A **control** — pause, step, save, load,
//! reset, screenshot, quit — is edge-triggered: it acts on the transition to
//! pressed. A held `.` that stepped every frame would run the game at full speed
//! while claiming to be paused, and a held F5 would write sixty save states a
//! second.
//!
//! That asymmetry is the reason [`Controls`] is a struct with a method rather than
//! a free function: the edge needs last frame's keys.
//!
//! # This crate does not know about polarity
//!
//! Every field of [`machine::Inputs`] is `true` for *pressed*, and `machine` does
//! the active-low conversion in one place. This module sets booleans and nothing
//! else. Computing port values here would duplicate the project's only piece of
//! polarity logic — and `machine::inputs`' own module comment records what getting
//! it backwards costs: a board that "boots with every button held, which looks like
//! a game bug rather than a bus bug and costs a day to find".
//!
//! # Player 2 is not mapped
//!
//! Two players on one keyboard needs a second ten-key cluster and every honest
//! option is bad. `Inputs` already carries P2 for a gamepad or netplay to fill in.
//! `tests::no_key_presses_a_player_two_control` pins the absence, so a later map
//! cannot half-add it.

use machine::Inputs;

/// A key this frontend understands.
///
/// The frontend's own enum, deliberately **not** the windowing library's. A
/// `minifb::Key` here would make this module — the key map, the thing most worth
/// testing — part of the display boundary. `sfemu`'s `display` module translates,
/// in a total match with no decisions in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// P1 stick up.
    Up,
    /// P1 stick down.
    Down,
    /// P1 stick left.
    Left,
    /// P1 stick right.
    Right,
    /// P1 jab.
    A,
    /// P1 strong.
    S,
    /// P1 fierce.
    D,
    /// P1 short kick.
    Z,
    /// P1 forward kick.
    X,
    /// P1 roundhouse kick.
    C,
    /// Start 1.
    Num1,
    /// Start 2.
    Num2,
    /// Coin 1.
    Num5,
    /// Coin 2.
    Num6,
    /// The test switch. Held at boot it enters the service menu.
    F2,
    /// Reset the machine.
    F3,
    /// Save state.
    F5,
    /// Load state.
    F8,
    /// Screenshot.
    F12,
    /// Pause / resume.
    P,
    /// Step one frame while paused.
    Period,
    /// Quit.
    Escape,
}

impl Key {
    /// Every variant, for the tests that must cover all of them.
    ///
    /// `tests::all_lists_every_key_exactly_once` fails if a variant is added and
    /// not listed here, which is what stops the tests that iterate this from
    /// quietly narrowing.
    pub const ALL: [Key; 22] = [
        Key::Up,
        Key::Down,
        Key::Left,
        Key::Right,
        Key::A,
        Key::S,
        Key::D,
        Key::Z,
        Key::X,
        Key::C,
        Key::Num1,
        Key::Num2,
        Key::Num5,
        Key::Num6,
        Key::F2,
        Key::F3,
        Key::F5,
        Key::F8,
        Key::F12,
        Key::P,
        Key::Period,
        Key::Escape,
    ];

    /// This key's bit in a [`KeySet`].
    ///
    /// A `match` and not `self as u32`: a cast makes the bit a function of
    /// declaration order, so reordering the enum for readability would silently
    /// remap every key. Written out, a reorder changes nothing.
    const fn bit(self) -> u32 {
        match self {
            Key::Up => 0,
            Key::Down => 1,
            Key::Left => 2,
            Key::Right => 3,
            Key::A => 4,
            Key::S => 5,
            Key::D => 6,
            Key::Z => 7,
            Key::X => 8,
            Key::C => 9,
            Key::Num1 => 10,
            Key::Num2 => 11,
            Key::Num5 => 12,
            Key::Num6 => 13,
            Key::F2 => 14,
            Key::F3 => 15,
            Key::F5 => 16,
            Key::F8 => 17,
            Key::F12 => 18,
            Key::P => 19,
            Key::Period => 20,
            Key::Escape => 21,
        }
    }
}

/// Which keys are held.
///
/// A bitmask rather than a `Vec`, so [`Controls`] can keep last frame's set by
/// copy and the edge detection is one `&`. Twenty-two keys fit a `u32` with room
/// to spare.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeySet {
    bits: u32,
}

impl KeySet {
    /// Nothing held.
    pub const fn new() -> Self {
        Self { bits: 0 }
    }

    /// Marks `k` held.
    pub fn press(&mut self, k: Key) {
        self.bits |= 1 << k.bit();
    }

    /// Whether `k` is held.
    pub fn contains(&self, k: Key) -> bool {
        self.bits & (1 << k.bit()) != 0
    }

    /// A set of exactly these keys.
    pub fn from_keys(keys: &[Key]) -> Self {
        let mut s = Self::new();
        for &k in keys {
            s.press(k);
        }
        s
    }
}

/// What the loop should do this frame.
/// No `PartialEq`: [`machine::Inputs`] has none, and nothing compares two of
/// these. `Default` works because `Inputs`' hand-written one is `Inputs::idle()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Actions {
    /// The board's inputs, level-triggered.
    pub inputs: Inputs,
    /// Pause was pressed this frame.
    pub pause_toggled: bool,
    /// Step one frame.
    pub step: bool,
    /// Reset the machine.
    pub reset: bool,
    /// Write a save state.
    pub save: bool,
    /// Read a save state.
    pub load: bool,
    /// Write a screenshot.
    pub screenshot: bool,
    /// Close the window.
    pub quit: bool,
}

/// The keyboard, frame to frame.
#[derive(Debug, Clone, Default)]
pub struct Controls {
    /// Last frame's held keys, for the edge detection.
    was: KeySet,
}

impl Controls {
    /// A keyboard with nothing held.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads this frame's held keys.
    pub fn update(&mut self, now: KeySet) -> Actions {
        // Pressed *this* frame: held now and not held before. Per key, so two
        // controls pressed on the same frame both fire and releasing one does not
        // re-arm another.
        let edge = |k: Key| now.contains(k) && !self.was.contains(k);

        let mut inputs = Inputs::idle();
        inputs.p1.up = now.contains(Key::Up);
        inputs.p1.down = now.contains(Key::Down);
        inputs.p1.left = now.contains(Key::Left);
        inputs.p1.right = now.contains(Key::Right);
        inputs.p1.punch = [
            now.contains(Key::A),
            now.contains(Key::S),
            now.contains(Key::D),
        ];
        inputs.p1.kick = [
            now.contains(Key::Z),
            now.contains(Key::X),
            now.contains(Key::C),
        ];
        inputs.coin1 = now.contains(Key::Num5);
        inputs.coin2 = now.contains(Key::Num6);
        inputs.start1 = now.contains(Key::Num1);
        inputs.start2 = now.contains(Key::Num2);
        // Level-triggered, unlike every other function key: the service menu is
        // entered by *holding* the test switch, which is what the switch does on a
        // real cabinet.
        inputs.test = now.contains(Key::F2);

        let actions = Actions {
            inputs,
            pause_toggled: edge(Key::P),
            step: edge(Key::Period),
            reset: edge(Key::F3),
            save: edge(Key::F5),
            load: edge(Key::F8),
            screenshot: edge(Key::F12),
            quit: edge(Key::Escape),
        };
        self.was = now;
        actions
    }
}
```

Add to `lib.rs`:

```rust
pub mod keys;

pub use keys::{Actions, Controls, Key, KeySet};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p frontend`
Expected: PASS.

Two details the code above gets right and are easy to get wrong when typing it out:

1. `Key::ALL` is `[Key; 22]` and includes `Key::Escape` — every variant. Three tests iterate it, and `no_key_presses_a_player_two_control` in particular must cover `Escape`; a list missing one key narrows all three silently, which is why `all_lists_every_key_exactly_once` pins the length as a literal.
2. `Actions` derives `Debug, Clone, Copy, Default` and **not** `PartialEq` — `machine::Inputs` derives only `Debug, Clone, Copy` (`crates/machine/src/inputs.rs:18`) with a hand-written `Default` equal to `idle()`. Nothing compares two `Actions`, so this costs nothing.

- [ ] **Step 5: Run the gate**

As Task 1 Step 7.

- [ ] **Step 6: Mutation pass**

| Mutant | Must |
|---|---|
| `let edge = \|k: Key\| now.contains(k) && !self.was.contains(k);` → `let edge = \|k: Key\| now.contains(k);` | KILL |
| `self.was = now;` → removed (`let _ = now;`) | KILL |
| `inputs.p1.kick = [` block's `Key::Z` → `Key::A` | KILL |
| `Key::D => 6,` → `Key::D => 5,` | KILL |
| `inputs.test = now.contains(Key::F2);` → `edge(Key::F2)` | KILL |
| `pause_toggled: edge(Key::P),` → `now.contains(Key::P)` | KILL |
| `inputs.coin1 = now.contains(Key::Num5);` → `Key::Num1` | KILL |
| **Control:** `Key::bit`'s `Key::Escape => 21` → `=> 25` | SURVIVE |

The control survives because bit 25 is still a free bit in the `u32` and still unique — the mask's *values* are arbitrary, only their distinctness matters, and `every_key_has_its_own_slot` tests exactly that. Worth having as a control precisely because it looks like it should fail.

- [ ] **Step 7: Commit**

```bash
git add crates/frontend
git commit -m "feat(frontend): the key map, and the edge/level asymmetry"
```

---

### Task 3: Pens to ARGB

**Files:**
- Create: `crates/frontend/src/pixels.rs`
- Modify: `crates/frontend/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  pub fn pens_to_argb(v: &machine::video::compose::Video, out: &mut Vec<u32>);
  ```
  Takes `&mut Vec<u32>` and clears it rather than returning a fresh one: this runs sixty times a second and the buffer is 344 KB.

- [ ] **Step 1: Write the failing tests**

Create `crates/frontend/src/pixels.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use machine::video::{compose::Video, palette, regs, HEIGHT, WIDTH};

    /// A `Video` whose palette has known entries, built through `machine` so the
    /// path under test is the real one.
    ///
    /// Uses `Video` directly rather than a booted machine: this function converts a
    /// framebuffer, and a CPU has nothing to do with it.
    fn video_with_palette(entries: &[(usize, u16)]) -> Video {
        let mut v = Video::new(
            machine::BoardConfig::sf2().video,
            machine::BoardConfig::sf2().mapper,
            Vec::new(),
        );
        // The palette is built from gfxram at render time, so write the entries
        // there and render once. Page 0 enabled, palette base 0 -> gfxram word 0.
        let mut gfxram = vec![0u16; 0x1_8000];
        for &(pen, entry) in entries {
            gfxram[pen] = entry;
        }
        let mut cps_a = [0u16; 0x20];
        cps_a[regs::PALETTE_BASE] = 0;
        let mut cps_b = [0u16; 0x20];
        cps_b[machine::BoardConfig::sf2().video.palette_control] = 0x003F;
        v.render(&gfxram, &cps_a, &cps_b);
        v
    }

    /// The buffer is one `u32` per visible pixel, and the whole frame.
    #[test]
    fn the_buffer_is_one_word_per_pixel_of_the_visible_frame() {
        let v = video_with_palette(&[]);
        let mut out = Vec::new();
        pens_to_argb(&v, &mut out);
        assert_eq!(out.len(), 86_016, "384 * 224, as a literal");
        assert_eq!(out.len(), WIDTH * HEIGHT, "and that is the frame's size");
    }

    /// A pen becomes `0x00RRGGBB`, with the channels in that order.
    ///
    /// The literals are hand-computed from `video::palette::entry_to_rgb`'s
    /// documented arithmetic — `bright = 0x0F + ((e >> 12) << 1)`, each nibble
    /// scaled `* 0x11 * bright / 0x2D` — and written here rather than obtained by
    /// calling it. A conversion checked against the function it wraps agrees with
    /// itself whatever either does.
    ///
    /// Entry 0xF000 is brightness 15 (unity: `0x0F + 30 = 0x2D`), red 0, green 0,
    /// blue 0 — black. 0xFF00 is brightness 15, red 0x0F: `0x0F * 0x11 * 0x2D /
    /// 0x2D` = 0xFF. So pure red is 0x00FF0000, which is what pins the *order*: a
    /// red/blue swap gives 0x000000FF.
    #[test]
    fn a_pen_becomes_argb_with_red_in_the_high_byte() {
        let v = video_with_palette(&[
            (0, 0xFF00), // brightness 15, red 15, green 0, blue 0
            (1, 0xF0F0), // green 15
            (2, 0xF00F), // blue 15
            (3, 0xFFFF), // white
            (4, 0xF000), // black
        ]);
        let p = v.palette();
        assert_eq!(p[0], 0xFF00, "the palette really holds the entry");

        assert_eq!(argb(p[0]), 0x00FF_0000, "red is bits 16-23");
        assert_eq!(argb(p[1]), 0x0000_FF00, "green is bits 8-15");
        assert_eq!(argb(p[2]), 0x0000_00FF, "blue is bits 0-7");
        assert_eq!(argb(p[3]), 0x00FF_FFFF, "white");
        assert_eq!(argb(p[4]), 0x0000_0000, "black");
    }

    /// Brightness scales, and it truncates.
    ///
    /// `entry_to_rgb`'s documentation records that entry 0x8777 gives 81 and not
    /// 82, which is the hardware's truncating division as MAME models it. Pinned
    /// here too, because a frontend that rounded instead would differ from the PPM
    /// dump by one in every channel — a difference nobody would ever see on screen
    /// and which would make the two outputs disagree forever.
    ///
    /// Hand-computed: brightness 8 gives `0x0F + 16 = 0x1F`; `7 * 0x11 * 0x1F /
    /// 0x2D` = `119 * 31 / 45` = `3689 / 45` = 81 (81.98 truncated).
    #[test]
    fn brightness_truncates_exactly_as_the_renderer_does() {
        assert_eq!(argb(0x8777), 0x0051_5151, "81 = 0x51 in all three channels");
    }

    /// The conversion agrees with the renderer's own, pen for pen.
    ///
    /// The literals above are what pin the format; this pins the *agreement*. The
    /// PPM writer in `sfemu` uses `entry_to_rgb` and this uses `argb`, and a
    /// channel swap in one of them would make a screenshot and the window disagree
    /// while both looked plausible on their own. Checked over every reachable
    /// entry rather than a sample: 65,536 is cheap and the failure could be in one
    /// brightness level.
    #[test]
    fn the_window_and_the_screenshot_cannot_disagree() {
        for e in 0..=0xFFFFu16 {
            let [r, g, b] = palette::entry_to_rgb(e);
            let want = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
            assert_eq!(argb(e), want, "entry {e:#06x}");
        }
    }

    /// The frame's pens are converted, in row-major order, and not a fill.
    ///
    /// A conversion that wrote the same pixel everywhere would pass every test
    /// above. This one puts two different pens in known places and reads them back
    /// at the matching offsets.
    #[test]
    fn each_pixel_takes_its_own_pens_colour() {
        let mut v = video_with_palette(&[(0, 0xFF00), (1, 0xF00F)]);
        // Reaching into the framebuffer directly: the subject is the conversion,
        // and drawing two specific pens through the tile path would test `video`
        // again while saying less about this function.
        v.fb.pens[0] = 0;
        v.fb.pens[1] = 1;
        v.fb.pens[WIDTH] = 1; // first pixel of row 1
        v.fb.pens[86_015] = 0; // last pixel

        let mut out = Vec::new();
        pens_to_argb(&v, &mut out);
        assert_eq!(out[0], 0x00FF_0000, "pen 0 is red");
        assert_eq!(out[1], 0x0000_00FF, "pen 1 is blue");
        assert_eq!(out[WIDTH], 0x0000_00FF, "row-major: row 1 starts at WIDTH");
        assert_eq!(out[86_015], 0x00FF_0000, "and the last pixel is converted");
    }

    /// The buffer is reused, not appended to.
    ///
    /// Called sixty times a second on a 344 KB buffer, so it takes `&mut Vec` — and
    /// a missing `clear` would grow it without bound while every length assertion
    /// above still passed on the first call.
    #[test]
    fn a_reused_buffer_does_not_grow() {
        let v = video_with_palette(&[]);
        let mut out = Vec::new();
        for _ in 0..3 {
            pens_to_argb(&v, &mut out);
            assert_eq!(out.len(), 86_016);
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p frontend`
Expected: FAIL to compile — `pens_to_argb` and `argb` do not exist.

- [ ] **Step 3: Write the implementation**

Above the test module in `crates/frontend/src/pixels.rs`:

```rust
//! The framebuffer as the pixels a window wants.
//!
//! The windowing library takes `0x00RRGGBB` per pixel; `video` produces palette
//! pens. This is the one-line bridge, and it is here rather than in the display
//! module because it is arithmetic, and arithmetic behind the display boundary
//! cannot be tested.
//!
//! # Why this does not call `entry_to_rgb`
//!
//! It does — `tests::the_window_and_the_screenshot_cannot_disagree` requires the
//! two to agree over all 65,536 entries, and a screenshot that differed from the
//! window would be a genuinely confusing bug. But the *format* is pinned by
//! hand-written literals, because a test that only compared the two would pass
//! with both wrong in the same direction.

use machine::video::compose::Video;
use machine::video::palette::entry_to_rgb;

/// One palette entry as `0x00RRGGBB`.
///
/// Red in bits 16-23, green in 8-15, blue in 0-7, and the top byte zero. The
/// windowing library ignores the top byte; leaving it zero rather than 0xFF is the
/// convention `minifb`'s own example uses.
fn argb(entry: u16) -> u32 {
    let [r, g, b] = entry_to_rgb(entry);
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

/// Converts the rendered frame into `out`, replacing its contents.
///
/// `out` is reused across frames: this runs sixty times a second on a 344 KB
/// buffer, and allocating one per frame is the kind of waste that shows up as a
/// stutter rather than as a slowdown.
pub fn pens_to_argb(v: &Video, out: &mut Vec<u32>) {
    let pal = v.palette();
    out.clear();
    out.reserve(v.fb.pens.len());
    out.extend(v.fb.pens.iter().map(|&pen| argb(pal[usize::from(pen)])));
}
```

Add to `lib.rs`:

```rust
pub mod pixels;

pub use pixels::pens_to_argb;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p frontend`
Expected: PASS.

⚠️ Expected first-run problem: `pal[usize::from(pen)]` panics if a pen exceeds the palette. `video`'s own `rgb()` indexes the same way and the renderer cannot produce an out-of-range pen (the maximum is `BACKGROUND_PEN`, 0xBFF, and the palette is 0xC00 entries) — so match `rgb()` and index directly rather than adding a `get`. If clippy objects to the `reserve` followed by `extend`, drop the `reserve`: `extend` over an `ExactSizeIterator`-backed map reserves anyway.

- [ ] **Step 5: Run the gate**

As Task 1 Step 7.

- [ ] **Step 6: Mutation pass**

| Mutant | Must |
|---|---|
| `(u32::from(r) << 16) \| (u32::from(g) << 8) \| u32::from(b)` → `r` and `b` swapped | KILL |
| `<< 16` → `<< 24` | KILL |
| `out.clear();` → removed | KILL |
| `v.fb.pens.iter()` → `v.fb.pens.iter().take(1000)` | KILL |
| **Control:** `out.reserve(v.fb.pens.len());` → removed | SURVIVE |

The control survives because `reserve` is an optimisation; removing it changes only how many allocations happen, which nothing observes.

- [ ] **Step 7: Commit**

```bash
git add crates/frontend
git commit -m "feat(frontend): pens to ARGB, pinned against the screenshot's colours"
```

---

### Task 4: Machine snapshots

**Files:**
- Create: `crates/machine/src/snapshot.rs`
- Modify: `crates/machine/src/lib.rs`
- Modify: `crates/machine/src/cps1.rs`
- Modify: `crates/machine/src/board.rs`
- Modify: `crates/video/src/compose.rs`
- Modify: `crates/video/src/sprites.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  // machine::snapshot
  #[derive(Debug, Clone)]
  pub struct MachineState {
      pub cpu: m68k::M68k,
      pub ram: Box<[u16; 0x8000]>,
      pub gfxram: Box<[u16; 0x1_8000]>,
      pub cps_a: [u16; 0x20],
      pub cps_b: [u16; 0x20],
      pub sound_latch: [u8; 2],
      pub coin_ctrl: u16,
      pub vblank_pending: bool,
      pub inputs: Inputs,
      pub total_cycles: u64,
      pub line: u32,
      pub carry: i64,
      pub obj: video::sprites::ObjLatch,
  }
  impl Cps1 {
      pub fn snapshot(&self) -> MachineState;
      pub fn restore(&mut self, s: &MachineState);
  }
  // video::compose
  impl Video {
      pub fn obj_latch(&self) -> &ObjLatch;
      pub fn set_obj_latch(&mut self, l: &ObjLatch);
  }
  ```
  `MachineState`'s fields are public because it is a plain data carrier the save-state codec in `frontend` has to read field by field. The *machine's* fields stay private; that is the point.

**Note:** this task touches `machine`, so the gate includes the vector suite: `cargo run -q -p testrunner --release --bin report -- --test suite`, expecting 127/127.

- [ ] **Step 1: Write the failing tests**

Create `crates/machine/src/snapshot.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoardConfig, Cps1, Timing};

    /// A program that writes a different value to RAM every frame and reads its
    /// vblank, so a restored machine that is even slightly off diverges visibly.
    ///
    /// ```text
    /// 1000  46FC 2000        move #$2000,sr        supervisor, mask 0 (take IRQs)
    /// 1004  5279            addq.w #1,$FF0000      count frames in RAM
    /// 100A  33C0 0090 0000   move.w d0,$900000     and write gfxram, which the
    ///                                              renderer reads
    /// 1010  60F2            bra $1004
    /// ```
    ///
    /// Encodings verified against `m68k::disasm` when this test was written.
    fn diverging_program() -> Vec<u8> {
        // ... (the implementer writes the bytes; see Step 3's note)
        unimplemented!()
    }

    /// A snapshot restores a machine that runs the same future.
    ///
    /// **This is the load-bearing test of the whole sub-project**, and it is a
    /// divergence test rather than a comparison. `snapshot == snapshot` passes for
    /// a serializer that drops a field the comparison also ignores — and three of
    /// the fields that must be in a state are private, so that is exactly the
    /// mistake available here.
    ///
    /// So: run 20 frames, snapshot, run 30 more and record what happened, restore,
    /// run the same 30, and require the *framebuffer and the trace counters* to
    /// match. A dropped `carry` shifts every later scanline boundary; a dropped
    /// `vblank_pending` doubles or misses an interrupt at the seam; a dropped
    /// object latch draws one frame of wrong sprites. Thirty frames is long enough
    /// for any of the three to become a different picture.
    #[test]
    fn a_restored_machine_runs_the_same_thirty_frames() {
        let mut m = machine();
        for _ in 0..20 {
            m.run_frame();
        }
        let s = m.snapshot();

        let first = advance_and_fingerprint(&mut m, 30);

        m.restore(&s);
        let second = advance_and_fingerprint(&mut m, 30);

        assert_eq!(
            first, second,
            "a restored machine must run the same thirty frames"
        );
    }

    /// And the fingerprint can tell two machines apart.
    ///
    /// The test above is only meaningful if its comparison can fail. This runs a
    /// *different* number of frames and requires a different fingerprint — the
    /// control every `0/N` claim in this project is required to have.
    #[test]
    fn the_fingerprint_distinguishes_different_runs() {
        let mut m = machine();
        for _ in 0..20 {
            m.run_frame();
        }
        let s = m.snapshot();
        let thirty = advance_and_fingerprint(&mut m, 30);
        m.restore(&s);
        let twenty_nine = advance_and_fingerprint(&mut m, 29);
        assert_ne!(
            thirty, twenty_nine,
            "if these matched, the divergence test above would prove nothing"
        );
    }

    /// Each of the three private fields is in the state, tested one at a time.
    ///
    /// The divergence test catches all three together, which means it says
    /// "something is missing" rather than which. These three say which. Each
    /// corrupts one field of a *restored* machine and requires the future to
    /// change — so a field that is restored but ignored fails here too.
    #[test]
    fn the_scheduler_carry_is_part_of_the_state() {
        let mut m = machine();
        m.run_scanline(); // leave a non-zero carry
        let mut s = m.snapshot();
        assert!(s.carry <= 0, "the carry is a debt, so never positive");
        let mut behind = s.clone();
        behind.carry -= 100;
        m.restore(&behind);
        let with = m.run_scanline();
        m.restore(&s);
        let without = m.run_scanline();
        assert_ne!(
            with, without,
            "the carry must reach the scheduler: a 100-cycle debt shortens a line"
        );
    }

    #[test]
    fn the_pending_vblank_is_part_of_the_state() {
        let mut m = machine();
        let mut s = m.snapshot();
        assert!(!s.vblank_pending, "a fresh machine has none");
        s.vblank_pending = true;
        m.restore(&s);
        assert!(
            m.board.vblank_pending(),
            "a state taken mid-interrupt must restore the pending line, or the \
             guest misses or doubles that interrupt"
        );
        s.vblank_pending = false;
        m.restore(&s);
        assert!(!m.board.vblank_pending());
    }

    #[test]
    fn the_object_latch_is_part_of_the_state() {
        let mut m = machine();
        // Put a sprite in gfxram and latch it, so the latch differs from a fresh
        // one. Object table at word 0, one record.
        m.board.gfxram[3] = 0x0001;
        m.video.latch_objects(&m.board.gfxram[..], &m.board.cps_a);
        let s = m.snapshot();
        assert_eq!(
            s.obj.words()[3], 0x0001,
            "the snapshot carries the latched table"
        );

        // A fresh machine's latch is all zero; restoring must overwrite it.
        let mut fresh = machine();
        assert_eq!(fresh.video.obj_latch().words()[3], 0x0000, "the premise");
        fresh.restore(&s);
        assert_eq!(
            fresh.video.obj_latch().words()[3],
            0x0001,
            "sprites are delayed one frame, so a state without the latch draws one \
             frame of the wrong sprites"
        );
    }

    /// A snapshot does not carry the ROM, the graphics ROM, or the trace.
    ///
    /// The ROM and gfx are the user's files: a save state that embedded them would
    /// be a ROM file this project must not produce. The trace is a record of the
    /// run rather than state of the machine — and a restored trace would make the
    /// divergence test compare a copy of the first run's counters against
    /// themselves, which is the self-confirming shape this project exists to
    /// distrust.
    ///
    /// Checked structurally: `MachineState` has no field for any of them, and its
    /// size is bounded well below a ROM's.
    #[test]
    fn a_snapshot_carries_no_rom_and_no_trace() {
        let m = machine();
        let s = m.snapshot();
        // RAM 0x8000 words + gfxram 0x18000 words = 0x20000 words = 256 KB, plus
        // the small fields. A state that had picked up the 4 MB ROM or the
        // graphics ROM would be an order of magnitude larger. The `Box`ed arrays
        // are behind pointers, so this measures the inline part.
        assert!(
            core::mem::size_of_val(&s) < 8 * 1024,
            "the large arrays are boxed and the ROM is absent: {} bytes inline",
            core::mem::size_of_val(&s)
        );
        // And the trace really is still the live one after a restore, not a copy.
        let mut m2 = machine();
        m2.run_frame();
        let before = m2.board.trace.frames;
        m2.restore(&s);
        assert_eq!(
            m2.board.trace.frames, before,
            "restoring must not rewind the trace: it records the session, not the \
             machine"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p machine`
Expected: FAIL to compile — `snapshot`, `restore`, `MachineState`, and `obj_latch` do not exist.

- [ ] **Step 3: Write the implementation**

The `diverging_program` and `advance_and_fingerprint` helpers are left for the implementer because they must be *verified*, not transcribed:

- `diverging_program`: assemble the four instructions above as bytes, and **verify each encoding with `m68k::disasm::disassemble`** before relying on it (`crates/machine/src/cps1.rs`'s `spin()` and its doc comment are the pattern to copy — it records the date it verified its three encodings). The reset vector is SSP 0x00FF8000, PC 0x00001000, as every other program in that file uses. The program must take interrupts (SR mask 0), because `vblank_pending` is only interesting if the guest acknowledges.
- `advance_and_fingerprint(m: &mut Cps1, frames: u32) -> (Vec<u16>, u64, u64, u64, u64)`: run `frames` frames, `render()`, and return the framebuffer pens plus the trace's `vblanks`, `acks`, `gfxram_writes`, and the machine's `total_cycles`. Pens **and** counters, because a picture alone would miss a missed interrupt that happened to draw the same and counters alone would miss a sprite drawn one frame late.
- `machine()`: `Cps1::with_gfx(&diverging_program(), gfx, BoardConfig::sf2(), Timing::cps1_10mhz())` with a small synthetic `gfx` (an opaque tile — see `crates/sfemu/src/main.rs`'s `a_drawn_frame` for the byte pattern) and `reset()` called. A `gfx` of `Vec::new()` renders a uniform frame, which would make the pen comparison blind.

Then `crates/machine/src/snapshot.rs`:

```rust
//! Save-state data: everything that makes the machine's future what it is.
//!
//! # Why this is a struct and not public fields
//!
//! Three of the values a save state needs are private, and two of them are private
//! for reasons the code documents at length: `Cps1::carry` is the scheduler's
//! sub-frame position and writing it without the invariant makes every later
//! scanline wrong, and `Video::obj` is the one-frame sprite delay. Widening those
//! fields so a codec could read them would let any later caller write them.
//!
//! So the machine hands out a copy and takes one back. [`MachineState`]'s own
//! fields are public — it is a data carrier, and `frontend`'s codec has to read it
//! field by field.
//!
//! # What is not in here, and why
//!
//! - **The ROM and the graphics ROM.** The user supplied them. A save state
//!   containing them would be a ROM file, which this project must not produce.
//! - **The palette and the framebuffer.** Recomputed by the next `render`.
//! - **The decoder table.** 512 KB, rebuilt in a constructor.
//! - **The [`Trace`](crate::Trace).** A record of the session, not state of the
//!   machine. Restoring it would also make a divergence test compare the first
//!   run's counters against a copy of themselves.

use crate::inputs::Inputs;
use m68k::M68k;
use video::sprites::ObjLatch;

/// A complete save state.
#[derive(Debug, Clone)]
pub struct MachineState {
    /// The CPU, whole.
    pub cpu: M68k,
    /// Main RAM.
    pub ram: Box<[u16; crate::board::RAM_WORDS]>,
    /// Tilemap, sprite, and palette RAM.
    pub gfxram: Box<[u16; crate::board::GFXRAM_WORDS]>,
    /// CPS-A.
    pub cps_a: [u16; crate::board::CPS_REGS],
    /// CPS-B.
    pub cps_b: [u16; crate::board::CPS_REGS],
    /// The sound latches.
    pub sound_latch: [u8; 2],
    /// Coin counters and lockouts.
    pub coin_ctrl: u16,
    /// Whether IPL1 is asserted and unacknowledged.
    ///
    /// A state taken between the assertion and the guest's vector fetch — one
    /// scanline in 262, so it happens — restores wrong without this, and the guest
    /// then misses or doubles that interrupt.
    pub vblank_pending: bool,
    /// Controls and DIP switches.
    ///
    /// Cheap, and a state restored mid-move without it drops the held direction.
    pub inputs: Inputs,
    /// Cycles since reset.
    pub total_cycles: u64,
    /// The current scanline.
    pub line: u32,
    /// The scheduler's carried debt: where the machine is *within* a scanline.
    ///
    /// Always `<= 0`. Omitting it puts a restored machine up to one instruction out
    /// of step, every line, forever.
    pub carry: i64,
    /// The previous frame's object table — the one-frame sprite delay.
    pub obj: ObjLatch,
}
```

`RAM_WORDS`, `GFXRAM_WORDS`, and `CPS_REGS` are currently private consts in `board.rs`; make them `pub(crate)` (not `pub` — nothing outside `machine` needs them, and `frontend` reads lengths off the arrays).

In `cps1.rs`:

```rust
    /// A complete save state.
    ///
    /// Clones the two large arrays, 256 KB in total. That is the cost of a save
    /// state and it happens on a keypress, not per frame.
    pub fn snapshot(&self) -> MachineState {
        MachineState {
            cpu: self.cpu.clone(),
            ram: self.board.ram.clone(),
            gfxram: self.board.gfxram.clone(),
            cps_a: self.board.cps_a,
            cps_b: self.board.cps_b,
            sound_latch: self.board.sound_latch,
            coin_ctrl: self.board.coin_ctrl,
            vblank_pending: self.board.vblank_pending(),
            inputs: self.board.inputs,
            total_cycles: self.total_cycles,
            line: self.line,
            carry: self.carry,
            obj: self.video.obj_latch().clone(),
        }
    }

    /// Restores a save state.
    ///
    /// Leaves the ROM, the graphics ROM, the decoder, and the trace alone: the
    /// first two are the user's files and the last is a record of the session
    /// rather than state of the machine.
    pub fn restore(&mut self, s: &MachineState) {
        self.cpu = s.cpu.clone();
        self.board.ram.copy_from_slice(&s.ram[..]);
        self.board.gfxram.copy_from_slice(&s.gfxram[..]);
        self.board.cps_a = s.cps_a;
        self.board.cps_b = s.cps_b;
        self.board.sound_latch = s.sound_latch;
        self.board.coin_ctrl = s.coin_ctrl;
        self.board.set_vblank_pending(s.vblank_pending);
        self.board.inputs = s.inputs;
        self.total_cycles = s.total_cycles;
        self.line = s.line;
        self.carry = s.carry;
        self.video.set_obj_latch(&s.obj);
    }
```

In `board.rs`, beside `assert_vblank`:

```rust
    /// Sets the pending-interrupt line directly, for a save-state restore.
    ///
    /// ⚠️ **Not for the scheduler.** [`Board::assert_vblank`] is what a beam
    /// reaching line 240 calls, and it also counts the vblank in the trace. This
    /// sets the line without counting anything, which is right for a restore — the
    /// vblank being restored was already counted when it happened — and wrong for
    /// everything else.
    pub fn set_vblank_pending(&mut self, pending: bool) {
        self.vblank_pending = pending;
    }
```

In `compose.rs`:

```rust
    /// The previous frame's object table, for a save state.
    pub fn obj_latch(&self) -> &ObjLatch {
        &self.obj
    }

    /// Restores the previous frame's object table.
    ///
    /// Sprites are delayed one frame (`cps1_v.cpp:3067-3068`), so this is state and
    /// not a cache: a machine restored without it draws one frame of the wrong
    /// sprites.
    pub fn set_obj_latch(&mut self, l: &ObjLatch) {
        self.obj = l.clone();
    }
```

`ObjLatch` already derives `Clone`. Add `pub mod snapshot;` and `pub use snapshot::MachineState;` to `machine`'s `lib.rs`, and `use crate::snapshot::MachineState;` to `cps1.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p machine`
Expected: PASS.

The carry test's two `restore`/`run_scanline` pairs must each bind their state to a local before calling `restore` — `m.restore(&m.snapshot())` does not borrow-check, since `&m.snapshot()` is an immutable borrow of `m` inside a mutable one.

- [ ] **Step 5: Run the gate, including the vector suite**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --release
cargo run -q -p testrunner --release --bin report -- --test suite   # expect 127/127
```

- [ ] **Step 6: Mutation pass**

| Mutant | Must |
|---|---|
| `carry: self.carry,` → `carry: 0,` | KILL |
| `self.carry = s.carry;` → removed | KILL |
| `vblank_pending: self.board.vblank_pending(),` → `false` | KILL |
| `self.video.set_obj_latch(&s.obj);` → removed | KILL |
| `self.board.gfxram.copy_from_slice(&s.gfxram[..]);` → removed | KILL |
| `line: self.line,` → `line: 0,` | KILL |
| `total_cycles: self.total_cycles,` → `0` | KILL |
| **Control:** `inputs: self.board.inputs,` → `Inputs::idle()` | ? |

The last one is a **probe, not a control**: if it survives, no test exercises restoring held inputs, and the right response is to add one (hold a direction, snapshot, restore into a fresh machine, and require the board to read that direction) rather than to accept the survivor. Add a separate genuine control: changing `MachineState`'s derive from `#[derive(Debug, Clone)]` to `#[derive(Clone)]` — nothing formats a state — which must SURVIVE.

- [ ] **Step 7: Commit**

```bash
git add crates/machine crates/video
git commit -m "feat(machine,video): save-state snapshots, without widening a private field"
```

---

### Task 5: The save-state format

**Files:**
- Create: `crates/frontend/src/state.rs`
- Modify: `crates/frontend/src/lib.rs`

**Interfaces:**
- Consumes: `machine::MachineState`, `Cps1::snapshot`/`restore` (Task 4).
- Produces:
  ```rust
  pub const MAGIC: [u8; 8] = *b"SFEMU\0\0\x01";
  pub const VERSION: u8 = 1;

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum StateError {
      NotAState,
      Version { found: u8 },
      WrongBoard { found: u32, expected: u32 },
      Truncated { need: usize, got: usize },
      Corrupt { found: u32, computed: u32 },
  }
  impl core::fmt::Display for StateError { /* names which check failed */ }

  pub fn encode(s: &MachineState, board: u32) -> Vec<u8>;
  pub fn decode(bytes: &[u8], board: u32) -> Result<MachineState, StateError>;
  pub const BOARD_SF2: u32 = 0x5346_3200;   // b"SF2\0"
  pub fn crc32(data: &[u8]) -> u32;
  ```

- [ ] **Step 1: Write the failing tests**

Create `crates/frontend/src/state.rs` with the tests first. The full set:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A machine with a distinctive state in every field, so a dropped field shows
    /// up as a changed value rather than as a coincidence.
    ///
    /// Built by *running* a machine rather than by assembling a `MachineState` by
    /// hand: a hand-built state could be internally impossible, and the codec would
    /// then be verified against something the machine cannot produce.
    fn a_state() -> (machine::Cps1, machine::MachineState) {
        // ... implementer: reuse the program pattern from machine's snapshot tests
        unimplemented!()
    }

    /// CRC-32 against the specification's own check vector.
    ///
    /// `"123456789"` → 0xCBF43926 is the CRC-32 spec's check value, and it is the
    /// same literal `romset`'s independent implementation is pinned against. Both
    /// are therefore checked against the standard rather than against each other,
    /// which is the point of not sharing the code: `frontend` may not depend on
    /// `romset`, because `romset` depends on `miniz_oxide`.
    #[test]
    fn crc32_matches_the_standard_check_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0x0000_0000, "the empty string");
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    }

    /// A round trip through bytes restores the same machine's future.
    ///
    /// Divergence again, not comparison: encode, decode into a *different* machine,
    /// and require thirty frames to match. Task 4 established that `snapshot` and
    /// `restore` carry everything; this establishes that the bytes do too, and a
    /// field the encoder forgot fails here even though Task 4's test passes.
    #[test]
    fn a_state_survives_a_round_trip_through_bytes() { /* ... */ }

    /// The five rejections, one test each, each naming its own check.
    #[test]
    fn a_file_that_is_not_a_state_is_refused() { /* bad magic */ }
    #[test]
    fn a_future_version_is_refused_by_version_and_not_by_crc() { /* ... */ }
    #[test]
    fn another_boards_state_is_refused() { /* ... */ }
    #[test]
    fn a_truncated_state_is_refused() { /* every prefix length */ }
    #[test]
    fn a_corrupted_payload_is_refused() { /* one flipped bit */ }

    /// The encoded length is a literal.
    #[test]
    fn the_encoded_length_is_the_documented_size() { /* ... */ }

    /// Decoding never panics, on any input.
    #[test]
    fn no_input_makes_the_decoder_panic() { /* ... */ }
}
```

The implementer writes each body. The requirements each must meet:

- **`a_truncated_state_is_refused`** must loop over **every** prefix length from 0 to `bytes.len() - 1` and require `Err` from all of them. One hand-picked length passes for a decoder that checks only the header.
- **`a_corrupted_payload_is_refused`** must flip one bit in the payload and require `Err(StateError::Corrupt { .. })` specifically — not just any error. And it must assert the *unmodified* bytes decode, so the test cannot pass because the fixture was broken all along.
- **`a_future_version_is_refused_by_version_and_not_by_crc`** must patch the version byte and then **fix up the CRC** so the file is otherwise valid. Without that fix-up the test passes on the CRC check and says nothing about version handling — the characteristic defect: an input that cannot exercise the property claimed.
- **`the_encoded_length_is_the_documented_size`** states the total as a hand-computed literal: 8 magic + 4 board + 8 length + payload + 4 CRC, with the payload's own arithmetic written out field by field in a comment. This is the test that makes the format a format rather than whatever the encoder happens to emit.
- **`no_input_makes_the_decoder_panic`** feeds: empty, one byte, the magic alone, the magic with a huge declared length (which must not attempt a 2^64 allocation — the decoder validates the length against the remaining bytes *before* using it), and a few thousand truncations and single-byte corruptions of a valid state. A frontend must never panic on a user's file.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p frontend`
Expected: FAIL to compile.

- [ ] **Step 3: Write the implementation**

`crates/frontend/src/state.rs`. The layout, which the module documentation must state as the authoritative order:

```
offset  size  field
0       8     MAGIC          b"SFEMU\0\0\x01"; the last byte is the version
8       4     board          little-endian; BOARD_SF2 = b"SF2\0"
12      8     payload length little-endian
20      len   payload
20+len  4     CRC-32 of the payload, little-endian
```

Payload order, and every reader and writer must follow this list top to bottom:

```
cpu: d[0..8], a[0..8], pc, sr, usp, ssp   (u32/u16, little-endian)
     prefetch[0..2], halted, stopped, pending_irq, in_exception, trace_pending
ram: 0x8000 u16
gfxram: 0x18000 u16
cps_a: 0x20 u16
cps_b: 0x20 u16
sound_latch: 2 u8
coin_ctrl: u16
vblank_pending: u8
inputs: coin1, coin2, service, start1, start2, test, then p1 and p2 each as
        right, left, down, up, punch[0..3], kick[0..3]; then dsw[0..3]
total_cycles: u64
line: u32
carry: i64
obj: 0x400 u16
```

Booleans are one byte, 0 or 1. **A decoder must accept any non-zero as true** rather than rejecting it: the value came from a file, and refusing a state because a padding byte is 2 is a rejection with no diagnostic value.

Why hand-rolled rather than `serde` + `bincode`: two more dependencies, and a format whose layout is *implied* by struct definitions — so a field reordered in a later refactor silently changes the format while every round-trip test still passes, because both sides moved together. That is this branch's characteristic defect in a save-state costume. Put that paragraph in the module docs.

`decode` validates in this order, and the error names which check failed: magic, version, board, declared length against the remaining bytes, CRC, then the payload's own length against what the fields need.

CRC-32 is twenty lines, reflected poly 0xEDB88320 — the same algorithm `crates/romset/src/crc32.rs` documents, written again here because `frontend` may not depend on `romset`. Say so in a comment, with the reason.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p frontend`
Expected: PASS.

- [ ] **Step 5: Run the gate** (as Task 1 Step 7; `machine` is untouched, so no vector suite)

- [ ] **Step 6: Mutation pass**

| Mutant | Must |
|---|---|
| The CRC check → always passes | KILL |
| The version check → always passes | KILL |
| The board check → always passes | KILL |
| The declared-length check → removed | KILL (and must not panic) |
| `carry` omitted from the payload | KILL |
| `obj` omitted from the payload | KILL |
| `vblank_pending` omitted | KILL |
| Two adjacent payload fields written in the other order | KILL |
| The CRC's `0xEDB88320` → `0x04C11DB7` | KILL |
| **Control:** `StateError`'s `Display` text reworded | SURVIVE |

The control survives because no test asserts the wording — deliberately, since a message's exact prose is not behaviour. What the tests do require is that the *variant* names the right check.

- [ ] **Step 7: Commit**

```bash
git add crates/frontend
git commit -m "feat(frontend): the save-state format, refused five ways"
```

---

### Task 6: The Display trait and the run loop

**Files:**
- Create: `crates/sfemu/src/loop_.rs`
- Modify: `crates/sfemu/src/main.rs`
- Modify: `crates/sfemu/Cargo.toml`

**Interfaces:**
- Consumes: `FramePacer`, `Controls`, `Actions`, `KeySet`, `pens_to_argb`, `state::{encode, decode}`.
- Produces:
  ```rust
  pub trait Display {
      fn present(&mut self, buf: &[u32]) -> Result<(), String>;
      fn held_keys(&self) -> KeySet;
      fn elapsed_ns(&mut self) -> u64;
      fn is_open(&self) -> bool;
      fn set_title(&mut self, title: &str);
  }
  pub struct LoopOpts { pub state_path: PathBuf, pub shot_path: PathBuf }
  pub struct Summary { pub frames: u64, pub dropped: u64, pub notices: Vec<String> }
  pub fn run(m: &mut Cps1, d: &mut impl Display, o: &LoopOpts) -> Summary;
  ```
  `elapsed_ns` takes `&mut self` because a real implementation resets its own last-tick mark. `present` is fallible because the windowing library's update is.

- [ ] **Step 1: Write the failing tests**

The recording fake is what makes this testable, and it is the part to get right:

```rust
    /// A `Display` that returns a script and records what it was shown.
    ///
    /// This is what lets the loop be tested at all: `cargo test` has no window, and
    /// the loop's decisions — how many frames a tick runs, whether a pause holds,
    /// whether a step is one frame — are exactly the decisions worth testing. A
    /// loop that could only be driven by a real window would be verified by
    /// looking at it.
    struct Fake {
        /// One entry per tick: the keys held and the host time since the last.
        script: Vec<(KeySet, u64)>,
        tick: usize,
        /// Every buffer length handed to `present`.
        presented: Vec<usize>,
        titles: Vec<String>,
    }
```

`is_open` returns `self.tick < self.script.len()`, so the loop ends when the script does. `elapsed_ns` returns the current entry's time and advances `tick`. `held_keys` returns the current entry's keys.

The tests, each with its scripted sequence:

- **`an_ordinary_tick_runs_one_frame`** — one tick of `FRAME_NS`, and `summary.frames == 1`.
- **`pause_stops_the_frames_and_resume_starts_them`** — press `P`, then two ordinary ticks (0 frames), press `P` again, then two more (2 frames). The assertion is on the frame count, not on an internal `paused` flag: a test reading the same flag the code sets passes a half-done fix.
- **`a_step_runs_exactly_one_frame_while_paused`** — pause, then three ticks holding `.` (one frame — the edge), then release and press again (a second frame).
- **`a_step_does_not_unpause`** — after a step, an ordinary tick still runs zero frames.
- **`a_stalled_host_runs_the_cap_and_not_the_debt`** — one tick of 2 s. `frames == 4`, `dropped == 115`.
- **`reset_returns_the_machine_to_power_on`** — run some frames, press `F3`, and require `total_cycles` back to 0 and the PC at the reset vector. **Not** the trace, which `reset` deliberately does not clear.
- **`escape_ends_the_loop_early`** — a ten-tick script with `Escape` on tick three; `presented.len() == 3`.
- **`every_tick_presents_a_full_frame`** — including while paused: the window must keep drawing or it goes black when you pause. Every entry of `presented` is 86,016.
- **`the_title_reports_dropped_frames`** — after a stall, some title contains the drop count. And after an ordinary run, none does — a title that always mentioned drops would be noise.
- **`a_save_and_load_round_trip_through_the_real_file`** — `F5`, run frames, `F8`, and require the machine's future to match. Uses a temp path under `std::env::temp_dir()` with the process id in the name, and removes it afterwards.
- **`a_failed_save_does_not_stop_the_loop`** — `state_path` pointing into a directory that does not exist. The loop continues, `notices` has one entry naming the path, and it is **one** entry rather than one per frame.
- **`a_corrupt_state_file_does_not_stop_the_loop`** — write garbage to the path, press `F8`, and require the loop to keep running with a notice.
- **`a_halted_cpu_is_reported_in_the_title_and_does_not_stop_the_loop`** — E2's debugger is what you want at that moment.

- [ ] **Step 2: Run the tests to verify they fail**

Expected: FAIL to compile — nothing in `loop_.rs` exists.

- [ ] **Step 3: Write the implementation**

The loop body, in this order per iteration:

1. `let elapsed = d.elapsed_ns();`
2. `let a = controls.update(d.held_keys());`
3. `if a.quit { break; }`
4. `m.board.inputs = a.inputs;`
5. `if a.reset { m.reset(); pacer.reset(); }`
6. `if a.pause_toggled { paused = !paused; pacer.reset(); }` — the pacer reset is what stops the paused wall-clock time from being owed as game time.
7. `if a.save { … }` / `if a.load { … }`, each pushing a notice on failure and never panicking.
8. The frame count: `if a.step { 1 } else if paused { 0 } else { pacer.tick(elapsed) }`.
9. Run that many frames, then `m.render()`.
10. `pens_to_argb(&m.video, &mut buf); d.present(&buf)`.
11. `if a.screenshot { … }`.
12. Update the title only when its content changes.

`m.render()` runs every iteration even at zero frames, so a paused window keeps drawing.

`main.rs` gains `mod loop_;` and, for now, no new flag — Task 7 adds `--play`. This task's deliverable is the loop and its tests; keeping the flag out means Task 6 commits with everything it added under test.

Add to `crates/sfemu/Cargo.toml`: `frontend = { path = "../frontend" }`. **Not `minifb` yet** — that is Task 7, and adding it here would put an untested dependency in a commit that does not use it.

- [ ] **Step 4: Run the tests to verify they pass**
- [ ] **Step 5: Run the gate** (as Task 1 Step 7)
- [ ] **Step 6: Mutation pass**

| Mutant | Must |
|---|---|
| `if a.step { 1 }` → `{ 2 }` | KILL |
| `else if paused { 0 }` → `pacer.tick(elapsed)` | KILL |
| `if a.pause_toggled { paused = !paused; }` → `paused = true;` | KILL |
| `pacer.reset()` on unpause → removed | KILL |
| `m.render()` moved inside the `frames > 0` branch | KILL |
| `m.board.inputs = a.inputs;` → removed | KILL |
| The save failure's notice → `panic!` | KILL |
| **Control:** the title's exact wording | SURVIVE |

- [ ] **Step 7: Commit**

```bash
git add crates/sfemu
git commit -m "feat(sfemu): the run loop, behind a Display a test can script"
```

---

### Task 7: The window

**Files:**
- Create: `crates/sfemu/src/display.rs`
- Modify: `crates/sfemu/src/main.rs`
- Modify: `crates/sfemu/Cargo.toml`

**Interfaces:**
- Consumes: the `Display` trait (Task 6).
- Produces: `Window::open(title) -> Result<Window, String>`, `impl Display for Window`, and `--play` / `--state` in the argument parser.

This is the only task whose deliverable a test cannot see, and it is deliberately last and deliberately thin.

- [ ] **Step 1: Add the dependency**

```toml
# The window, and the only crates.io dependency in this workspace besides
# romset's DEFLATE decoder. Reachable from `display.rs` alone: every decision a
# frontend makes lives in `crates/frontend`, which has never heard of a window.
# Verified 2026-08-08: two runtime crates (raw-window-handle, and cc at build
# time), builds in 9 s, opens a window on macOS.
minifb = "0.28"
```

- [ ] **Step 2: Write `display.rs`**

Its whole content: `Window::open`, the five trait methods, and `fn translate(k: minifb::Key) -> Option<Key>` — a total match. No arithmetic and no state beyond the last-tick `Instant` that `elapsed_ns` needs.

Window options, verified against `minifb 0.28` on 2026-08-08:

```rust
WindowOptions {
    resize: true,
    scale: Scale::X1,
    scale_mode: ScaleMode::AspectRatioStretch,
    ..Default::default()
}
```

opened at `WIDTH * 3` by `HEIGHT * 3`, and `set_target_fps(60)` so the library holds the rate. `update_with_buffer(buf, WIDTH, HEIGHT)` takes `0x00RRGGBB` — pinned by `pens_to_argb`'s tests, and confirmed against `minifb`'s own `from_u8_rgb` example.

The module's doc comment must state the boundary rule and why the file has no tests: *its content is calls into a library and one total match; there is nothing here a test could assert that would not be asserting about `minifb`.*

- [ ] **Step 3: The argument parser**

```
sfemu <rom-set> [frames] [--ppm <path>]        unchanged
sfemu <rom-set> --play [--state <path>]        the window
```

`--play` ignores a frame count rather than erroring: there is no reading of `sfemu set.zip 60 --play` under which the user wants a window that closes after one second. `--state` defaults to the ROM set's path with its extension replaced by `.sfs`.

Tests, in `main.rs`'s existing module: `--play` parses with and without `--state`; `--state` without `--play` is an error naming both flags; the default state path is derived from the ROM path (a literal expectation, e.g. `/a/b/sf2.zip` → `/a/b/sf2.sfs`); `--play` with a frame count parses and the count is ignored. Extend `the_usage_text_states_that_no_rom_is_supplied_or_fetched` to cover the new flags, keeping the `!u.contains("http")` assertion.

- [ ] **Step 4: Verify by hand, and say so**

`cargo test --workspace` cannot see this task's deliverable. The check is:

```bash
cargo run -p sfemu --release -- /path/to/your/sf2.zip --play
```

Record in the commit message that this was **not** verified against a real ROM set in this session, because no ROM may be committed or fetched. What *can* be verified without one, and must be: `cargo run -p sfemu --release -- /nonexistent --play` reports the load error and exits 1 rather than opening a window or panicking.

- [ ] **Step 5: Run the gate** (as Task 1 Step 7)
- [ ] **Step 6: Commit**

```bash
git add crates/sfemu
git commit -m "feat(sfemu): --play opens a window"
```

---

### Task 8: The mutation pass, and the boundary check

**Files:** any the pass finds a gap in.

- [ ] **Step 1: The boundary check**

```bash
grep -rln minifb crates/ | sort
```

Must print exactly `crates/sfemu/Cargo.toml` and `crates/sfemu/src/display.rs`. Anything else is the architecture's central constraint broken, and it is worth a test rather than a habit — add one to `crates/sfemu/src/display.rs`:

```rust
    /// `minifb` is named in this file and nowhere else.
    ///
    /// The whole testability argument rests on it: a `use minifb::Key` in
    /// `frontend` would put the key map behind the display boundary, and the
    /// compiler would not object. Checked by reading the source tree, which is
    /// unusual for a test and is the only way to assert the absence of a `use`.
    #[test]
    fn the_windowing_library_is_named_in_one_file() { /* ... */ }
```

It walks `../..` from `env!("CARGO_MANIFEST_DIR")` over `crates/**/*.rs` and `crates/**/Cargo.toml`, and requires the only matches to be this file and `sfemu`'s manifest. A path-walking test is unusual and justified: it is the only way to assert a `use` is *absent*.

- [ ] **Step 2: The whole-crate mutation pass**

Every mutant from Tasks 1-7 re-run in one harness over `crates/frontend` and the new `sfemu` modules, plus the mutants a whole-crate view suggests. Report: total, killed, survived, NO-OP, equivalent, control. Every survivor gets an explicit disposition — a real test gap, a documented equivalent mutant, or a NO-OP to redo. **No survivor passes as "probably fine."**

- [ ] **Step 3: `README.md`**

- The roadmap: A, B, C complete; **E1 complete**; and the row for E split into E1/E2/E3 with the reasoning from the spec's scope check. Fix the existing B-row wording, which lists "minimal window" under B when the window is E's.
- A "Playing it" section: the `--play` invocation, the controls table, and the save-state keys.
- The measured frame cost, **with its caveat**: 0.749 ms per frame for CPU + render + RGB against a 16.768 ms budget, on the author's machine, measured with synthetic worst-case content on 2026-08-08. Not a performance guarantee — the same language the 68000 bench section uses.
- The three checks only the user can make: does the window show SF2, does it run at the right speed, do the controls respond.

- [ ] **Step 4: Run the full gate**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --release
cargo doc --no-deps --workspace          # must be warning-free
cargo run -q -p testrunner --release --bin report -- --test suite   # 127/127
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs(readme): E1 — how to play it, and what only you can check"
```

---

## Self-Review

**Spec coverage.** Every section of the design maps to a task: the pacer to 1, controls to 2, pen conversion to 3, snapshots to 4, the format to 5, the loop to 6, the window and CLI to 7, the README and the boundary check to 8. The spec's five deliverables: `frontend` (1-3, 5), snapshot methods (4), `--play`/`--state`/`display.rs` (7), README (8), and the user-only checks (8, Step 3).

**Placeholders.** Tasks 5 and 6 give test *names and requirements* rather than full bodies, which is a deliberate exception to the no-placeholders rule and is marked as such: the bodies are mechanical once the requirement is stated, and the requirements are where the substance is (every prefix length; fix up the CRC before testing the version check; assert the frame count and not the `paused` flag). Task 4's two helpers are likewise left to the implementer **with the reason stated** — the instruction encodings must be verified against `m68k::disasm`, not transcribed from this plan, because a plan-supplied encoding is exactly the kind of unverified literal this project has learned to distrust.

**Type consistency.** `KeySet` is passed by value to `Controls::update` throughout (`Copy`). `Actions` is returned by value. `pens_to_argb` takes `&mut Vec<u32>` in the interface block, the implementation, and the loop. `MachineState` is produced by `Cps1::snapshot` and consumed by `restore`, `encode`, and `decode` with the same name in all four. `Display::present` returns `Result<(), String>` in the trait, the fake, and the real window.

**Three errors this review found and fixed inline.** `Key::ALL` was declared `[Key; 21]` while the enum has 22 variants and `Escape` was missing from the list — which would have silently narrowed the three tests that iterate it. `Actions` derived `PartialEq, Eq`, which cannot compile: `machine::Inputs` derives only `Debug, Clone, Copy` (verified at `crates/machine/src/inputs.rs:18`). And Task 4's carry test contained `m.restore(&m.snapshot())`, an immutable borrow inside a mutable one; it is now two explicit locals, which is also clearer about what it compares. The File Structure table said the README was Task 9 when there are eight tasks.

**Facts checked against the tree while reviewing, not assumed.** `Video::fb` and `Framebuffer::pens` are `pub` (`crates/video/src/compose.rs:100,40`), so Task 3's test can write pens directly. `Video::palette`, `render`, and `latch_objects` are `pub`. `ObjLatch` derives `Clone` and exposes `words()` (`crates/video/src/sprites.rs:47,80`), so Task 4's snapshot can clone it and its test can read it. `regs::PALETTE_BASE` is word index 5 and `VIDEOCONTROL` is 17, both `pub`.
