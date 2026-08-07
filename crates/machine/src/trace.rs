//! What the board saw.
//!
//! Sub-project B renders nothing, so this is its entire observable surface — and
//! it is a better instrument for the question B actually answers ("is the boot
//! code progressing?") than a black window would be. "Does SF2 boot?" becomes
//! checkable: after N frames, is `vblanks == N`, did `cps_a_writes` happen, did
//! the game ask the Z80 for the attract music, and are the sampled PCs inside
//! populated ROM rather than looping in an exception handler?
//!
//! # Not machine state
//!
//! [`Cps1::reset`](crate::Cps1::reset) deliberately leaves the trace alone. It is
//! an instrument attached to the machine, not part of it: a driver that resets
//! mid-run wants to keep what it has already observed, and a caller that raised
//! [`Trace::pc_sample_cap`] before resetting would otherwise find it silently back
//! at zero.

use core::cmp::Ordering;

/// How many distinct addresses an [`UnmappedLog`] itemises before it stops
/// itemising.
///
/// A wild PC scanning memory produces millions of distinct unmapped addresses, and
/// a log with no bound is then a memory leak driven by guest behaviour — the exact
/// shape of failure this crate forbids everywhere else. It is also quadratic: the
/// sorted-`Vec` insert shifts the tail, so 200,000 distinct addresses is 200,000
/// insertions each memmoving ~1 MB. `crates/machine/src/board.rs`'s 24-bit sweep
/// test alone visits ~190,000 unmapped addresses.
///
/// 1024 is chosen because the diagnostic value is in the *worst* handful of
/// addresses (see [`UnmappedLog::worst`]) and because a board that is missing one
/// chip produces a handful, not thousands. Accesses past the cap are still counted
/// in [`UnmappedLog::total`] and reported separately by [`UnmappedLog::dropped`],
/// so the bound is visible rather than silent.
const DISTINCT_CAP: usize = 1024;

/// Per-address counter for accesses the board does not decode.
///
/// A `Vec` of pairs kept sorted rather than a `BTreeMap`, so `machine` needs no
/// `alloc`-flavoured collection beyond `Vec` and stays `no_std`-able later.
#[derive(Debug, Clone)]
pub struct UnmappedLog {
    /// Sorted by address. At most [`DISTINCT_CAP`] entries.
    entries: Vec<(u32, u64)>,
    total: u64,
    dropped: u64,
    cap: usize,
}

impl Default for UnmappedLog {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            total: 0,
            dropped: 0,
            cap: DISTINCT_CAP,
        }
    }
}

impl UnmappedLog {
    /// Counts one access to `addr`.
    pub fn record(&mut self, addr: u32) {
        self.total += 1;
        match self.entries.binary_search_by(|(a, _)| a.cmp(&addr)) {
            Ok(i) => self.entries[i].1 += 1,
            Err(i) if self.entries.len() < self.cap => self.entries.insert(i, (addr, 1)),
            // Past the cap: counted in `total`, reported by `dropped`, not itemised.
            Err(_) => self.dropped += 1,
        }
    }

    /// The itemised addresses and their counts, ascending by address.
    pub fn entries(&self) -> &[(u32, u64)] {
        &self.entries
    }

    /// Every access recorded, itemised or not.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Accesses to addresses past the distinct-address cap.
    ///
    /// Nonzero means [`UnmappedLog::entries`] is a sample rather than the whole
    /// story. A report that prints `total` without this reads as complete when it
    /// is not.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The addresses with the highest counts, worst first, ties by address.
    ///
    /// A boot that stalls with 40,000 unmapped writes to one address has just
    /// named the chip that is missing.
    pub fn worst(&self, n: usize) -> Vec<(u32, u64)> {
        let mut v = self.entries.clone();
        // ⚠️ **The tie-break is arithmetically dead today, and deliberately kept.**
        // `entries` is maintained ascending by address and `slice::sort_by` is
        // stable, so equal counts already come out ascending by address with the
        // tie-break replaced by `Ordering::Equal`. Mutation confirmed: no test in
        // this crate can kill it, and none was contorted to try.
        //
        // It stays because a total order is what makes two reports of the same run
        // diffable, and the equivalence rests on two facts elsewhere — that
        // `record` inserts in order, and that this `sort_by` is the stable one. A
        // switch to `sort_unstable_by`, or a `record` that appended, would make it
        // load-bearing with no test signalling that it had become so.
        v.sort_by(|a, b| match b.1.cmp(&a.1) {
            Ordering::Equal => a.0.cmp(&b.0),
            other => other,
        });
        v.truncate(n);
        v
    }
}

/// Counters and samples describing a run.
///
/// Every field is public and read directly; nothing here feeds back into the
/// simulation, so there is no invariant for a setter to protect.
#[derive(Debug, Default, Clone)]
pub struct Trace {
    /// Frames completed — counted when the scanline counter wraps, so a caller
    /// driving [`Cps1::run_scanline`](crate::Cps1::run_scanline) by hand counts the
    /// same frames a [`Cps1::run_frame`](crate::Cps1::run_frame) caller does.
    pub frames: u64,
    /// Times IPL1 was asserted at the top of vertical blanking.
    pub vblanks: u64,
    /// Autovector-26 fetches — the interrupt acknowledges.
    ///
    /// `acks` short of `vblanks` means the game is not servicing the interrupt:
    /// either the mask never drops or the handler never returns.
    pub acks: u64,
    /// Word-equivalent writes decoded by the CPS-A register file.
    pub cps_a_writes: u64,
    /// Word-equivalent writes decoded by the CPS-B register file.
    pub cps_b_writes: u64,
    /// Writes decoded by gfxram.
    pub gfxram_writes: u64,
    /// Writes decoded by either sound latch.
    ///
    /// Both latches share a counter because the question this answers is "has the
    /// 68000 started talking to the Z80 at all?", and sub-project D will read the
    /// latches themselves for anything finer.
    pub sound_latch_writes: u64,
    /// Writes to ROM space: decoded by a real board, latched by nothing.
    ///
    /// Counted separately from unmapped writes because a real CPS-1 decodes
    /// 0x000000-0x3FFFFF. A game writing there is a guest bug or a deliberate
    /// discard, not evidence that our map is missing a chip.
    pub rom_writes: u64,
    /// Reads no chip answered.
    pub unmapped_reads: UnmappedLog,
    /// Writes no chip latched.
    pub unmapped_writes: UnmappedLog,
    /// One PC per scanline, up to [`Trace::pc_sample_cap`].
    ///
    /// The value is the core's `pc`, which runs ahead of the executing instruction
    /// by the two prefetched words. That is fine for the question these answer —
    /// "is the program somewhere it could legitimately be?" — and would not be for
    /// a breakpoint.
    pub pc_samples: Vec<u32>,
    /// How many PCs to keep. **Zero disables sampling**, which is the default: a
    /// 60-frame run is 15,720 scanlines and a frontend running for an hour is
    /// 56 million, so opting in is the only safe default.
    pub pc_sample_cap: usize,
}

impl Trace {
    /// Records `pc` if there is room under [`Trace::pc_sample_cap`].
    pub(crate) fn sample_pc(&mut self, pc: u32) {
        if self.pc_samples.len() < self.pc_sample_cap {
            self.pc_samples.push(pc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_counts_per_address_and_keeps_entries_sorted() {
        let mut l = UnmappedLog::default();
        for a in [0x81_0000u32, 0x40_0000, 0x81_0000, 0x93_0000, 0x81_0000] {
            l.record(a);
        }
        assert_eq!(l.total(), 5);
        assert_eq!(l.dropped(), 0);
        assert_eq!(
            l.entries(),
            &[(0x40_0000, 1), (0x81_0000, 3), (0x93_0000, 1)],
            "ascending by address, with per-address counts"
        );
    }

    /// `worst` orders by count, and breaks ties by address so the report is stable.
    ///
    /// The tie case is the point: a `sort_by` on the count alone is not a total
    /// order, so two addresses with equal counts could swap between runs and a
    /// diff of two reports would show noise.
    #[test]
    fn worst_ranks_by_count_and_breaks_ties_by_address() {
        let mut l = UnmappedLog::default();
        for _ in 0..5 {
            l.record(0x93_0000);
        }
        for _ in 0..9 {
            l.record(0x81_0000);
        }
        l.record(0x40_0002);
        l.record(0x40_0000);
        assert_eq!(
            l.worst(3),
            vec![(0x81_0000, 9), (0x93_0000, 5), (0x40_0000, 1)],
            "0x400000 before 0x400002 on the tie at one access each"
        );
        assert_eq!(l.worst(1), vec![(0x81_0000, 9)]);
        assert_eq!(
            l.worst(99).len(),
            4,
            "asking for more than there are is fine"
        );
    }

    /// The distinct-address cap holds, and what it drops is still counted.
    ///
    /// The literals are hand-derived from `DISTINCT_CAP = 1024`: 1030 distinct
    /// addresses itemises 1024 and drops 6, while `total` sees all 1030 plus the
    /// 40 repeats of an address that *is* itemised.
    #[test]
    fn the_distinct_address_cap_bounds_the_entries_but_not_the_total() {
        let mut l = UnmappedLog::default();
        for i in 0..1030u32 {
            l.record(0x40_0000 + i * 2);
        }
        assert_eq!(l.entries().len(), 1024, "DISTINCT_CAP");
        assert_eq!(l.dropped(), 6, "1030 - 1024");
        assert_eq!(l.total(), 1030);

        // An address already itemised keeps counting past the cap: the bound is on
        // how many addresses are named, not on how many accesses are seen.
        for _ in 0..40 {
            l.record(0x40_0000);
        }
        assert_eq!(l.total(), 1070);
        assert_eq!(l.dropped(), 6, "a hit is not a drop");
        assert_eq!(l.entries()[0], (0x40_0000, 41));
    }

    /// Sampling is off until a cap is set, and then it stops at the cap.
    #[test]
    fn pc_sampling_is_opt_in_and_bounded() {
        let mut t = Trace::default();
        assert_eq!(t.pc_sample_cap, 0, "off by default");
        for _ in 0..10 {
            t.sample_pc(0x1234);
        }
        assert!(t.pc_samples.is_empty(), "a zero cap records nothing");

        t.pc_sample_cap = 3;
        for pc in [1u32, 2, 3, 4, 5] {
            t.sample_pc(pc);
        }
        assert_eq!(t.pc_samples, vec![1, 2, 3], "the first three, then nothing");
    }
}
