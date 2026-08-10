//! The two timers, the status register, and the IRQ line.
//!
//! # A timer's period is measured in samples, and that is exact
//!
//! ymfm hands its host a duration in input clocks:
//! `period * OPERATORS * m_clock_prescale` (`ymfm_fm.ipp:1487`), which for the OPM is
//! `period * 32 * 2` — `period * 64`. One YM2151 sample is also exactly 64 input
//! clocks, so a period unit *is* a sample and this port counts samples directly. No
//! rounding is involved and no host scheduler is needed.
//!
//! The two formulas differ in shape, not just in width:
//! `A = 1024 - value` and `B = 16 * (256 - value)`.
//!
//! # Load and enable are different bits and they do different things
//!
//! `0x14` bits 0-1 **load** (run) the timers; bits 2-3 **enable** their status bits.
//! A loaded-but-not-enabled timer still counts, still overflows, and still reloads —
//! it simply does not touch the status register. That is what lets a driver use CSM
//! without ever taking an interrupt, and it is why the enable bit is read at overflow
//! time rather than gating the counter.
//!
//! # Timer B's first period is short on purpose
//!
//! `engine_mode_write` loads timer B with `-(m_total_clocks & 15)` added to its
//! period, because B's ×16 multiplier is free-running: the chip's divider does not
//! restart when the timer is loaded, so the first tick is a partial one. Dropping the
//! adjustment makes every timer-B interrupt up to 15 samples late — inaudible in a
//! single note and a drifting tempo over a song.
//!
//! # `write_mode` takes the registers, which the plan's signature did not
//!
//! The plan declared `write_mode(&mut self, val: u8)`. That cannot be implemented:
//! loading a timer requires its period, which lives in `0x10`-`0x12`, and [`Timers`]
//! owns no register file. The alternative — deferring the load to the next
//! [`Timers::clock`] — would sample `total_clocks & 15` one sample late and re-read
//! the period one sample late, diverging from `engine_mode_write`, which does both
//! synchronously. So `regs` is a parameter. The chip writes the register first and
//! then calls this, matching ymfm's order.
//!
//! # Two bits this port deliberately never sets
//!
//! `STATUS_BUSY` (bit 7) and `STATUS_IRQ`. The real chip holds BUSY for 32 clocks
//! after a data write; ymfm reports it only when the host's `ymfm_is_busy` says so,
//! and the default is `false` (`ymfm.h:539`). Nothing on the CPS-1 sound board polls
//! it. `STATUS_IRQ` is 0 for the OPM (`ymfm_opm.h:124`), so ymfm's set/clear of it is
//! a no-op — the IRQ line is [`Timers::irq`], not a status bit.

use crate::regs::Regs;

/// Timer A's status bit, `ymfm_opm.h`'s `STATUS_TIMERA`.
pub const STATUS_TIMER_A: u8 = 0x01;

/// Timer B's status bit.
pub const STATUS_TIMER_B: u8 = 0x02;

/// Which status bits can raise the IRQ line: both timers.
///
/// ymfm's `m_irq_mask`, initialised to `STATUS_TIMERA | STATUS_TIMERB` and never
/// changed for the OPM.
const IRQ_MASK: u8 = STATUS_TIMER_A | STATUS_TIMER_B;

/// What one sample's worth of timer clocking produced.
///
/// An overflow is reported whether or not the timer's enable bit is set, because CSM
/// is driven by the overflow and not by the status bit.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TimerEvents {
    /// Timer A reached the end of its period this sample.
    pub timer_a_overflow: bool,
    /// Timer B reached the end of its period this sample.
    pub timer_b_overflow: bool,
}

/// The two timers, the status register, and the IRQ state derived from it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Timers {
    /// Samples remaining in each timer's period.
    counter: [u32; 2],
    /// Whether each timer is loaded and counting.
    running: [bool; 2],
    /// The status register, minus BUSY and IRQ — see the module docs.
    status: u8,
    /// ymfm's `m_irq_state`, recomputed on every status change.
    irq: bool,
    /// Samples clocked since reset, for timer B's free-running divider.
    total_clocks: u32,
}

impl Default for Timers {
    fn default() -> Self {
        Self::new()
    }
}

impl Timers {
    /// Timers in their post-reset state: stopped, no status, no IRQ.
    #[must_use]
    pub fn new() -> Self {
        Self {
            counter: [0; 2],
            running: [false; 2],
            status: 0,
            irq: false,
            total_clocks: 0,
        }
    }

    /// Return to the post-reset state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The status register as the guest reads it.
    #[must_use]
    pub fn status(&self) -> u8 {
        self.status
    }

    /// Whether the IRQ line is asserted.
    #[must_use]
    pub fn irq(&self) -> bool {
        self.irq
    }

    /// One timer's period in samples, read from the registers now.
    ///
    /// `delta` is the free-running adjustment, which only timer B uses. The result is
    /// floored at 1: a zero-sample period would be a timer that never advances.
    fn period(tnum: usize, regs: &Regs, delta: i32) -> u32 {
        let period = if tnum == 0 {
            1024 - regs.timer_a_value()
        } else {
            16 * (256 - regs.timer_b_value())
        };
        (period as i32 + delta).max(1) as u32
    }

    /// Start or stop one timer, as ymfm's `update_timer`.
    ///
    /// **A timer that is already running is left alone.** That guard is why a driver
    /// re-writing `0x14` with the load bit still set does not restart the count — it
    /// only takes effect on the transition into running.
    fn update(&mut self, tnum: usize, enable: bool, regs: &Regs, delta: i32) {
        if enable {
            if !self.running[tnum] {
                self.counter[tnum] = Self::period(tnum, regs, delta);
                self.running[tnum] = true;
            }
        } else {
            self.running[tnum] = false;
        }
    }

    /// Recompute the IRQ line from the status register.
    fn check_interrupts(&mut self) {
        self.irq = self.status & IRQ_MASK != 0;
    }

    /// Handle a write to the mode register `0x14`.
    ///
    /// `val` is the byte written. The reset bits (4 and 5) are one-shots read from
    /// this value, and so are the load bits (0 and 1) — ymfm writes the register
    /// first and then reads them back, which is the same thing. The *enable* bits are
    /// not read here: they are read at overflow time, from `regs`.
    ///
    /// `regs` must already carry the new mode byte if the caller wants the timer
    /// values it reads to be current; the chip writes the register before calling
    /// this, matching `engine_mode_write`.
    pub fn write_mode(&mut self, val: u8, regs: &Regs) {
        let mut reset_mask = 0;
        if val & 0x20 != 0 {
            reset_mask |= STATUS_TIMER_B;
        }
        if val & 0x10 != 0 {
            reset_mask |= STATUS_TIMER_A;
        }
        self.status &= !reset_mask;
        self.check_interrupts();

        // Timer B first, and with the negative adjustment — see the module docs.
        let delta = -((self.total_clocks & 15) as i32);
        self.update(1, val & 0x02 != 0, regs, delta);
        self.update(0, val & 0x01 != 0, regs, 0);
    }

    /// Advance both timers by one sample.
    pub fn clock(&mut self, regs: &Regs) -> TimerEvents {
        self.total_clocks = self.total_clocks.wrapping_add(1);
        let mut events = TimerEvents::default();

        for tnum in 0..2 {
            if !self.running[tnum] {
                continue;
            }
            self.counter[tnum] -= 1;
            if self.counter[tnum] != 0 {
                continue;
            }
            if tnum == 0 {
                events.timer_a_overflow = true;
            } else {
                events.timer_b_overflow = true;
            }

            // The enable bit gates the *status bit*, not the count.
            let enabled = if tnum == 0 {
                regs.enable_timer_a()
            } else {
                regs.enable_timer_b()
            };
            if enabled {
                self.status |= if tnum == 0 {
                    STATUS_TIMER_A
                } else {
                    STATUS_TIMER_B
                };
            }

            // ymfm reloads unconditionally: `m_timer_running = false; update_timer(t,
            // 1, 0)`. The period is re-read from the registers, so a value written
            // mid-period takes effect at the next overflow rather than immediately.
            self.running[tnum] = false;
            self.update(tnum, true, regs, 0);
        }

        if events.timer_a_overflow || events.timer_b_overflow {
            self.check_interrupts();
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regs::Regs;

    /// Timer A's period is `1024 - value` sample-times; timer B's is `16 * (256 - value)`.
    ///
    /// From `ymfm_fm.ipp:1481`, in units of 64 input clocks — one sample. Measured
    /// here by counting samples between overflows, which is what the suite's status
    /// trace observes.
    #[test]
    fn the_two_timer_periods_match_their_formulas() {
        for (value, want) in [(0u32, 1024u32), (1000, 24), (1023, 1)] {
            let mut r = Regs::new();
            let mut t = Timers::new();
            r.write(0x10, ((value >> 2) & 0xFF) as u8);
            r.write(0x11, (value & 3) as u8);
            r.write(0x14, 0x05); // load A, enable A
            t.write_mode(0x05, &r);
            let mut first = None;
            for n in 1..4096u32 {
                if t.clock(&r).timer_a_overflow {
                    first = Some(n);
                    break;
                }
            }
            assert_eq!(first, Some(want), "timer A at {value}");
        }

        for (value, want) in [(0u32, 4096u32), (255, 16)] {
            let mut r = Regs::new();
            let mut t = Timers::new();
            r.write(0x12, value as u8);
            r.write(0x14, 0x0A); // load B, enable B
            t.write_mode(0x0A, &r);
            let mut first = None;
            for n in 1..8192u32 {
                if t.clock(&r).timer_b_overflow {
                    first = Some(n);
                    break;
                }
            }
            assert_eq!(first, Some(want), "timer B at {value}");
        }
    }

    /// An enabled timer sets its status bit and raises IRQ; a disabled one does not.
    ///
    /// Enable is bits 2 and 3 of `0x14`; load is bits 0 and 1. **A loaded but not
    /// enabled timer still counts and still reloads — it just does not touch the
    /// status register.** A core that gated the counter on the enable bit drifts
    /// once a driver enables a running timer.
    #[test]
    fn a_loaded_but_disabled_timer_counts_without_signalling() {
        let mut r = Regs::new();
        let mut t = Timers::new();
        r.write(0x10, 0xFF);
        r.write(0x11, 0x03); // period 1
        r.write(0x14, 0x01); // load A, do NOT enable
        t.write_mode(0x01, &r);
        for _ in 0..64 {
            let ev = t.clock(&r);
            assert!(ev.timer_a_overflow, "still overflowing");
        }
        assert_eq!(t.status() & 0x01, 0, "but the status bit stays clear");
        assert!(!t.irq(), "and no IRQ");

        r.write(0x14, 0x05);
        t.write_mode(0x05, &r);
        t.clock(&r);
        assert_eq!(t.status() & 0x01, 0x01, "enabling it exposes the overflow");
        assert!(t.irq());
    }

    /// Writing the reset bit clears the status bit and drops IRQ.
    ///
    /// Bits 4 and 5 of `0x14` are one-shot resets for A and B. The IRQ line only
    /// drops when *both* status bits are clear — a core that dropped it on either
    /// reset would lose interrupts whenever both timers are in use, which is the
    /// normal CPS-1 configuration.
    #[test]
    fn irq_drops_only_when_both_status_bits_are_clear() {
        let mut r = Regs::new();
        let mut t = Timers::new();
        r.write(0x10, 0xFF);
        r.write(0x11, 0x03);
        r.write(0x12, 0xFF);
        r.write(0x14, 0x0F); // load and enable both
        t.write_mode(0x0F, &r);
        for _ in 0..64 {
            t.clock(&r);
        }
        assert_eq!(t.status() & 0x03, 0x03, "both overflowed");
        assert!(t.irq());

        t.write_mode(0x1F, &r); // reset A only
        assert_eq!(t.status() & 0x03, 0x02, "A cleared, B still set");
        assert!(t.irq(), "IRQ is still asserted for B");

        t.write_mode(0x2F, &r); // reset B
        assert_eq!(t.status() & 0x03, 0x00);
        assert!(!t.irq(), "now it drops");
    }

    /// CSM is bit 7 and fires on timer A regardless of timer A's enable bit.
    ///
    /// This is the premise the whole `prepare()` gate rests on (Task 9): CSM key-on
    /// is driven by timer A's *overflow*, not by its status bit, so a driver can use
    /// CSM without ever taking an interrupt.
    #[test]
    fn csm_fires_on_timer_a_overflow_even_with_the_irq_disabled() {
        let mut r = Regs::new();
        let mut t = Timers::new();
        r.write(0x10, 0xFF);
        r.write(0x11, 0x03);
        r.write(0x14, 0x81); // CSM on, load A, enable A OFF
        t.write_mode(0x81, &r);
        let ev = t.clock(&r);
        assert!(ev.timer_a_overflow, "the overflow happened");
        assert_eq!(t.status() & 0x01, 0, "with no status bit");
        assert!(!t.irq(), "and no interrupt");
    }

    /// Status bit 7 is BUSY and this core never reports busy.
    ///
    /// The real chip holds BUSY for 32 clocks after a data write. Nothing on the
    /// CPS-1 sound board polls it — the driver writes on a timer — so this core
    /// returns 0 and says so here, rather than leaving a future reader to wonder
    /// whether the omission was deliberate.
    #[test]
    fn busy_is_never_reported_and_that_is_deliberate() {
        let t = Timers::new();
        assert_eq!(t.status() & 0x80, 0);
    }

    /// Clearing the load bit stops a timer; setting it again restarts the period.
    ///
    /// The plan had no test for the stop path, and the natural bug — treating the
    /// load bit as write-once, since ymfm's `update_timer` returns early for an
    /// already-running timer — leaves a stopped timer counting forever. The second
    /// half is what makes the first non-vacuous: the restart is a *fresh* period, not
    /// a resumption of the old count.
    #[test]
    fn clearing_the_load_bit_stops_the_count_and_setting_it_starts_a_fresh_period() {
        let mut r = Regs::new();
        let mut t = Timers::new();
        r.write(0x10, 0xFF);
        r.write(0x11, 0x00); // value 1020, period 4
        r.write(0x14, 0x05);
        t.write_mode(0x05, &r);
        for _ in 0..3 {
            assert!(!t.clock(&r).timer_a_overflow, "three of four samples");
        }

        // One sample short of the overflow, stop it. It must not fire.
        r.write(0x14, 0x04);
        t.write_mode(0x04, &r);
        for _ in 0..16 {
            assert!(!t.clock(&r).timer_a_overflow, "stopped means stopped");
        }

        // Restarting gives a whole period, not the one sample that was left.
        r.write(0x14, 0x05);
        t.write_mode(0x05, &r);
        for n in 1..=4u32 {
            let fired = t.clock(&r).timer_a_overflow;
            assert_eq!(fired, n == 4, "a fresh period of four at sample {n}");
        }
    }

    /// A re-write of the mode register with the load bit still set does not restart.
    ///
    /// `update_timer`'s `enable && !m_timer_running` guard. Sound drivers rewrite
    /// `0x14` on every interrupt to clear the status bit, so a port that reloaded
    /// here would stretch every period and the music would run slow.
    #[test]
    fn re_writing_the_load_bit_does_not_restart_a_running_timer() {
        let mut r = Regs::new();
        let mut t = Timers::new();
        r.write(0x10, 0xFF);
        r.write(0x11, 0x00); // period 4
        r.write(0x14, 0x05);
        t.write_mode(0x05, &r);
        for n in 1..=8u32 {
            // Clear the status bit every sample, exactly as a driver's ISR would.
            t.write_mode(0x15, &r);
            let fired = t.clock(&r).timer_a_overflow;
            assert_eq!(fired, n % 4 == 0, "still every fourth sample, at {n}");
        }
    }

    /// Timer B's first period is short by the free-running divider's phase.
    ///
    /// `-(m_total_clocks & 15)`. Loading timer B 5 samples in gives a first period of
    /// `16 - 5`, and every period after that is a full 16 — which is the assertion
    /// that distinguishes a one-off adjustment from a permanently wrong period.
    #[test]
    fn timer_b_loses_the_divider_phase_on_its_first_period_only() {
        let mut r = Regs::new();
        let mut t = Timers::new();
        r.write(0x12, 0xFF); // period 16
        r.write(0x14, 0x0A);

        // Run five samples with nothing loaded so total_clocks advances.
        for _ in 0..5 {
            t.clock(&r);
        }
        t.write_mode(0x0A, &r);

        let mut fired_at = vec![];
        for n in 1..=40u32 {
            if t.clock(&r).timer_b_overflow {
                fired_at.push(n);
            }
        }
        assert_eq!(fired_at, vec![11, 27], "16 - 5, then a full 16");
    }
}
