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
        // Neither constructor produces a zero period, and a zero would make this a
        // division by zero rather than a slow window — so it is worth one branch to
        // answer "no frames" instead of aborting the process.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame period is CPS-1's, as a literal.
    ///
    /// 8,000,000 / (512 × 262) = 59.6374 Hz, so one frame is
    /// 1,000,000,000 × 512 × 262 / 8,000,000 = 16,768,000 ns exactly. Written by
    /// hand rather than computed from `machine::Timing`'s constants: the point of
    /// the literal is that it disagrees loudly if the derivation is wrong, and a
    /// figure recomputed from the same three constants agrees with itself whatever
    /// they are.
    ///
    /// The division being exact is the reason this is a `u64` of nanoseconds and
    /// not a float.
    #[test]
    fn a_frame_is_16_768_000_nanoseconds() {
        assert_eq!(FRAME_NS, 16_768_000);
        // And the same number `machine`'s timing implies, computed the long way
        // round as an independent check of the literal above.
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
    /// The test above checks the alternation over four ticks; this checks the
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
    /// implemented one off would drop a frame on every ordinary four-frame hiccup
    /// and the count would be noise.
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

    /// The period and the cap come from the constructor, not from the constants.
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

    /// A zero period answers "no frames" instead of dividing by zero.
    ///
    /// Neither constructor produces one, so this is not a reachable state today —
    /// but the guard is in the code and an untested branch is a branch nobody knows
    /// the behaviour of. A panic here would be a frontend crash rather than a
    /// stalled window.
    #[test]
    fn a_zero_period_owes_nothing_rather_than_dividing_by_zero() {
        let mut p = FramePacer::new(0, 4);
        assert_eq!(p.tick(1_000_000), 0);
        assert_eq!(p.dropped(), 0);
    }
}
