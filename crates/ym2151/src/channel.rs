//! One FM channel: four operators, the eight algorithms, and the feedback loop.
//!
//! # The operators are stored by slot, and that is what makes the key-on mask work
//!
//! [`Channel::ops`] is indexed 0-3 by *chain position* — operator 1 through operator
//! 4 of the algorithm — not by register order. Two consequences, and both are load
//! bearing:
//!
//! 1. The cache for slot `s` comes from register operator [`slot_of`]`(s)`, because the
//!    register-to-slot swap is its own inverse (`0->0, 1->2, 2->1, 3->3`).
//! 2. A key-on mask bit index *is* a slot index. ymfm's `keyonoff` does
//!    `m_op[opnum]->keyonoff(bitfield(states, opnum))` over `opnum` 0-3, and `m_op`
//!    is slot-indexed, so register `0x08` bit 3 keys slot 0, bit 4 keys slot 1, and
//!    so on. Measured against ymfm: bits 3-6 reach register offsets `0x00`, `0x10`,
//!    `0x08`, `0x18` — which is `slot_of` again, not the identity.
//!
//! # The algorithm table is derived, not transcribed twice
//!
//! [`ALGORITHM_OPS`] is built by the private `algorithm`, which takes the same six
//! arguments in
//! the same order as ymfm's `ALGORITHM` macro (`ymfm_fm.ipp:1017`), so the eight rows
//! below are a character-for-character copy of `s_algorithm_ops`' first eight
//! entries. Rows 8-11 are OPL3's and are not reachable from a 3-bit OPM algorithm
//! field, so they are not here.
//!
//! The decoded row is eight columns indexed by slot: columns 0-3 name which `opout`
//! scratch slot feeds each operator's modulation input, and columns 4-7 whether each
//! operator's output enters the final sum. Column 0 is always 0 (operator 1's
//! modulation is the feedback loop, not an `opout` entry) and column 7 always 1
//! (every algorithm consumes operator 4), which is what lets one loop handle all
//! eight algorithms — see `the_table_is_indexable_by_slot`.
//!
//! # Three details that a plausible port gets wrong
//!
//! 1. **`opout` is 16-bit.** ymfm declares `int16_t opout[8]` and stores the sums
//!    `O1+O2`, `O1+O3`, `O2+O3` into it. Operator outputs are 14-bit signed, so the
//!    sums cannot actually exceed the range — but the truncation is reproduced with
//!    `wrapping_add` rather than assumed away.
//! 2. **The feedback is updated before the pan early-out.** ymfm computes operator
//!    1 and stores `m_feedback_in`, and only *then* returns early for a channel with
//!    both pan bits clear. A port that checked the pan first would leave the feedback
//!    loop frozen while the channel is muted, and the note would come back wrong when
//!    the driver re-enables it.
//! 3. **AM is gated per operator.** This crate's [`Operator::compute_volume`] takes
//!    an already-gated offset (see its module docs), so the gating lives here: an
//!    operator with `0xA0` bit 7 clear is passed 0, not the channel's AM offset.

use crate::operator::{OpCache, Operator, KEYON_CSM};
use crate::regs::{op_index, slot_of, Regs};

/// One decoded algorithm row, in the argument order of ymfm's `ALGORITHM` macro.
///
/// `opNin` is the `opout` index feeding operator N's modulation input; `opNout` is
/// whether operator N's output is added into the channel's result.
#[must_use]
const fn algorithm(op2in: u8, op3in: u8, op4in: u8, op1out: u8, op2out: u8, op3out: u8) -> [u8; 8] {
    [0, op2in, op3in, op4in, op1out, op2out, op3out, 1]
}

/// The eight OPM algorithms, as `[input slot; 4] ++ [in the sum?; 4]` per row.
///
/// The `opout` scratch slots these index are: `0` = nothing, `1` = O1, `2` = O2,
/// `3` = O3, `5` = O1+O2, `6` = O1+O3, `7` = O2+O3. Slot 4 is unused — ymfm lists it
/// as `(O4)` because operator 4's output goes straight to the result.
pub static ALGORITHM_OPS: [[u8; 8]; 8] = [
    algorithm(1, 2, 3, 0, 0, 0), // 0: O1 -> O2 -> O3 -> O4 -> out (O4)
    algorithm(0, 5, 3, 0, 0, 0), // 1: (O1 + O2) -> O3 -> O4 -> out (O4)
    algorithm(0, 2, 6, 0, 0, 0), // 2: (O1 + (O2 -> O3)) -> O4 -> out (O4)
    algorithm(1, 0, 7, 0, 0, 0), // 3: ((O1 -> O2) + O3) -> O4 -> out (O4)
    algorithm(1, 0, 3, 0, 1, 0), // 4: ((O1 -> O2) + (O3 -> O4)) -> out (O2+O4)
    algorithm(1, 1, 1, 0, 1, 1), // 5: ((O1 -> O2) + (O1 -> O3) + (O1 -> O4)) -> out
    algorithm(1, 0, 0, 0, 1, 1), // 6: ((O1 -> O2) + O3 + O4) -> out (O2+O3+O4)
    algorithm(0, 0, 0, 1, 1, 1), // 7: (O1 + O2 + O3 + O4) -> out
];

/// A cache that has never been filled in.
///
/// Every sample runs `prepare` before any output is computed, so no operator is ever
/// asked for a volume against this — it exists so [`Channel::new`] does not need a
/// register file to construct one.
const EMPTY_CACHE: OpCache = OpCache {
    phase_step: 0,
    total_level: 0,
    eg_sustain: 0,
    eg_rate: [0; 4],
    detune: 0,
    multiple: 0,
    block_freq: 0,
};

/// One channel: four operators by slot, their caches, and the feedback history.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Channel {
    /// The four operators, indexed by chain position rather than register order.
    pub ops: [Operator; 4],
    /// Each operator's cached register data, refreshed by [`Channel::prepare`].
    pub caches: [OpCache; 4],
    /// ymfm's `m_feedback`: operator 1's output from the two previous samples.
    pub feedback: [i16; 2],
    /// ymfm's `m_feedback_in`: operator 1's output from *this* sample.
    pub feedback_in: i16,
}

impl Default for Channel {
    fn default() -> Self {
        Self::new()
    }
}

impl Channel {
    /// A channel in its post-reset state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ops: [Operator::new(); 4],
            caches: [EMPTY_CACHE; 4],
            feedback: [0; 2],
            feedback_in: 0,
        }
    }

    /// Return to the post-reset state, feedback history included.
    ///
    /// `fm_channel::reset` clears all three feedback fields; leaving them would let
    /// one note's tail modulate the first sample of the next.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The register operator index backing a slot, for `ch`.
    ///
    /// [`slot_of`] is an involution, so it converts in both directions: slot 1 is
    /// register operator 2, and register operator 2 is slot 1.
    #[must_use]
    pub const fn operator_index(ch: u32, slot: usize) -> u32 {
        op_index(ch, slot_of(slot as u32))
    }

    /// Re-cache every operator and act on any pending key-on edge.
    ///
    /// Returns whether any operator is still contributing — ymfm's `prepare` builds
    /// its active-channel mask from this. The mask was measured as a pure
    /// optimisation (no sample differs over 40,000 with it deleted), so the return
    /// value feeds the debugger rather than the mix.
    pub fn prepare(&mut self, regs: &Regs, ch: u32) -> bool {
        let mut active = false;
        for slot in 0..4 {
            let op_index = Self::operator_index(ch, slot);
            self.caches[slot] = OpCache::compute(regs, ch, op_index, 0);
            let cache = self.caches[slot];
            let op = &mut self.ops[slot];
            op.clock_keystate(op.keyon_live != 0, &cache);
            // `fm_operator::prepare` consumes CSM's key-on bit (`ymfm_fm.ipp:434`).
            // That is why calling `prepare` on every sample is wrong rather than
            // merely slow — see Task 9 and `chip.rs`'s TODO(task-9).
            op.set_keyon(false, KEYON_CSM);
            active |= op.is_active();
        }
        active
    }

    /// Advance the feedback history, then every operator's envelope and phase.
    ///
    /// The envelope only moves when `env_counter`'s low two bits are clear, matching
    /// `fm_operator::clock`; the phase moves every sample.
    pub fn clock(&mut self, regs: &Regs, ch: u32, env_counter: u32, lfo_pm: i32) {
        self.feedback[0] = self.feedback[1];
        self.feedback[1] = self.feedback_in;

        for slot in 0..4 {
            let cache = self.caches[slot];
            if env_counter & 3 == 0 {
                self.ops[slot].clock_envelope(env_counter >> 2, &cache);
            }
            let step = if cache.phase_step == OpCache::PHASE_STEP_DYNAMIC {
                cache.phase_step_with_pm(regs, ch, Self::operator_index(ch, slot), lfo_pm)
            } else {
                cache.phase_step
            };
            self.ops[slot].clock_phase(step);
        }
    }

    /// This channel's contribution to (left, right), as ymfm's `output_4op`.
    ///
    /// `am_offset` is the channel's AM offset before per-operator gating, and
    /// `noise_state` the noise generator's latched bit for this sample — used only by
    /// channel 7 with `0x0F` bit 7 set.
    pub fn output(&mut self, regs: &Regs, ch: u32, am_offset: u32, noise_state: u32) -> (i32, i32) {
        let ops = ALGORITHM_OPS[regs.ch_algorithm(ch) as usize];

        // Operator 1's optional self-feedback, from the two previous samples. The
        // shift is `10 - feedback`, so feedback 7 is the strongest and 0 means none —
        // a port shifting by `feedback` inverts the whole control.
        let feedback = regs.ch_feedback(ch);
        let opmod = if feedback == 0 {
            0
        } else {
            (i32::from(self.feedback[0]) + i32::from(self.feedback[1])) >> (10 - feedback)
        };

        let op1value = self.slot_volume(regs, ch, 0, opmod, am_offset);
        self.feedback_in = op1value as i16;

        // Only now, with the feedback updated, is a muted channel finished.
        let (left, right) = regs.ch_pan(ch);
        if !left && !right {
            return (0, 0);
        }

        // The `opout` scratch table. The sums use `wrapping_add` because ymfm's array
        // is `int16_t`; 14-bit operator outputs cannot actually reach the wrap.
        let mut opout = [0i16; 8];
        opout[1] = op1value as i16;

        let opmod = i32::from(opout[usize::from(ops[1])]) >> 1;
        opout[2] = self.slot_volume(regs, ch, 1, opmod, am_offset) as i16;
        opout[5] = opout[1].wrapping_add(opout[2]);

        let opmod = i32::from(opout[usize::from(ops[2])]) >> 1;
        opout[3] = self.slot_volume(regs, ch, 2, opmod, am_offset) as i16;
        opout[6] = opout[1].wrapping_add(opout[3]);
        opout[7] = opout[2].wrapping_add(opout[3]);

        // Operator 4 is the one the noise generator can replace, and only on channel
        // 7. Its output is the result rather than an `opout` entry, which is why
        // column 7 of the algorithm row is not read below.
        let mut result = if regs.noise_enable() && ch == 7 {
            let slot = 3;
            self.ops[slot].compute_noise_volume(
                self.am_for(regs, ch, slot, am_offset),
                noise_state,
                &self.caches[slot],
            )
        } else {
            let opmod = i32::from(opout[usize::from(ops[3])]) >> 1;
            self.slot_volume(regs, ch, 3, opmod, am_offset)
        };

        // OPM passes `rshift = 0` and `clipmax = 32767`, so each optional add clips at
        // the 16-bit bounds. The clip is per-add, not once at the end: three
        // operators summing to more than 32767 saturate on the way.
        for slot in 0..3 {
            if ops[4 + slot] != 0 {
                result = (result + i32::from(opout[slot + 1])).clamp(-32768, 32767);
            }
        }

        // Each pan bit gates one output independently — `add_to_output` tests them
        // separately, so a channel can be heard on one side only.
        (
            if left { result } else { 0 },
            if right { result } else { 0 },
        )
    }

    /// The AM offset one operator sees: the channel's, or 0 if its AM bit is clear.
    fn am_for(&self, regs: &Regs, ch: u32, slot: usize, am_offset: u32) -> u32 {
        if regs.op_lfo_am_enable(Self::operator_index(ch, slot)) {
            am_offset
        } else {
            0
        }
    }

    /// One operator's output at its current phase plus a modulation offset.
    fn slot_volume(&self, regs: &Regs, ch: u32, slot: usize, opmod: i32, am_offset: u32) -> i32 {
        let phase = self.ops[slot].phase_index().wrapping_add(opmod as u32);
        self.ops[slot].compute_volume(
            phase,
            self.am_for(regs, ch, slot, am_offset),
            &self.caches[slot],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A helper: set up one channel with a given algorithm, all four operators
    /// audible, and one operator silenced by total level.
    fn peak_with_one_silenced(algorithm: u8, silenced: Option<u8>) -> i32 {
        let mut chip = crate::Ym2151::new();
        chip.write(0x20, 0xC0 | algorithm); // channel 0: both pans, ALG
        for op in 0..4u8 {
            let off = op * 8;
            chip.write(0x40 + off, 0x01); // MUL = 1
            chip.write(0x60 + off, if Some(off) == silenced { 127 } else { 0 });
            chip.write(0x80 + off, 31); // AR = 31, fastest
            chip.write(0xA0 + off, 0); // D1R = 0, hold
            chip.write(0xC0 + off, 0);
            chip.write(0xE0 + off, 0); // D1L = 0, RR = 0
        }
        chip.write(0x28, 0x4A);
        chip.write(0x08, 0x78); // key on all four slots of channel 0
        let mut buf = [(0i16, 0i16); 512];
        chip.generate(&mut buf);
        buf.iter().map(|&(l, _)| i32::from(l).abs()).max().unwrap()
    }

    /// Algorithm 4's two carriers are at register offsets 0x10 and 0x18.
    ///
    /// **This is the experiment that discriminates the operator slot map**, and it
    /// is why this test exists rather than one that writes all four operators the
    /// same — such a test passes under both maps.
    ///
    /// Algorithm 4 is two independent 2-operator chains, so exactly two of the four
    /// operators are carriers and reach the output. Measured against ymfm: silencing
    /// register offsets 0x10 or 0x18 halves the peak, while 0x00 and 0x08 leave it
    /// unchanged. The naive 0,8,16,24 map predicts carriers at 0x08 and 0x18 and
    /// fails on 0x10.
    #[test]
    fn algorithm_four_carriers_are_at_offsets_ten_and_eighteen() {
        let full = peak_with_one_silenced(4, None);
        assert!(full > 0, "the reference peak is non-zero: {full}");

        let s00 = peak_with_one_silenced(4, Some(0x00));
        let s08 = peak_with_one_silenced(4, Some(0x08));
        let s10 = peak_with_one_silenced(4, Some(0x10));
        let s18 = peak_with_one_silenced(4, Some(0x18));

        assert_eq!(
            s00, full,
            "0x00 is a modulator: silencing it changes the timbre, not the peak"
        );
        assert_eq!(s08, full, "0x08 is a modulator too");
        assert!(s10 < full, "0x10 is a carrier: {s10} < {full}");
        assert!(s18 < full, "0x18 is a carrier: {s18} < {full}");
    }

    /// Algorithm 0 is a pure chain: only the last operator reaches the output.
    ///
    /// Silencing register offset 0x18's operator — slot 4 — takes the peak to
    /// exactly zero. Measured against ymfm at peak 0 versus 8176.
    #[test]
    fn algorithm_zero_has_exactly_one_carrier() {
        let full = peak_with_one_silenced(0, None);
        assert!(full > 4000, "a pure chain still makes sound: {full}");
        assert_eq!(
            peak_with_one_silenced(0, Some(0x18)),
            0,
            "slot 4 is the only carrier in algorithm 0"
        );
        for off in [0x00u8, 0x08, 0x10] {
            assert!(
                peak_with_one_silenced(0, Some(off)) > 0,
                "offset {off:#04x} is a modulator: the chain still sounds"
            );
        }
    }

    /// Algorithm 7 is four parallel carriers: every operator reaches the output.
    #[test]
    fn algorithm_seven_has_four_carriers() {
        let full = peak_with_one_silenced(7, None);
        for off in [0x00u8, 0x08, 0x10, 0x18] {
            let s = peak_with_one_silenced(7, Some(off));
            assert!(s < full, "offset {off:#04x} is a carrier in algorithm 7");
        }
    }

    /// The eight algorithms are eight distinct sounds.
    ///
    /// Without this, a table with two rows accidentally identical passes every
    /// per-algorithm test above. Compared by FNV over the whole 512-sample buffer.
    #[test]
    fn the_eight_algorithms_produce_eight_distinct_outputs() {
        let mut hashes = std::collections::BTreeSet::new();
        for alg in 0..8u8 {
            let mut chip = crate::Ym2151::new();
            chip.write(0x20, 0xC0 | alg);
            for op in 0..4u8 {
                let off = op * 8;
                // Deliberately asymmetric: identical operators make several
                // algorithms coincide and this test would pass vacuously.
                chip.write(0x40 + off, op + 1);
                chip.write(0x60 + off, op * 8);
                chip.write(0x80 + off, 31);
                chip.write(0xA0 + off, 0);
                chip.write(0xC0 + off, 0);
                chip.write(0xE0 + off, 0);
            }
            chip.write(0x28, 0x4A);
            chip.write(0x08, 0x78);
            let mut buf = [(0i16, 0i16); 512];
            chip.generate(&mut buf);
            let flat: Vec<u16> = buf
                .iter()
                .flat_map(|&(l, r)| [l as u16, r as u16])
                .collect();
            hashes.insert(crate::tables::fnv1a_u16(&flat));
        }
        assert_eq!(hashes.len(), 8, "eight algorithms, eight sounds");
    }

    /// Feedback changes the sound, and feedback 0 means none.
    #[test]
    fn feedback_changes_the_output_and_zero_means_none() {
        let render = |fb: u8| {
            let mut chip = crate::Ym2151::new();
            chip.write(0x20, 0xC0 | (fb << 3) | 7); // algorithm 7, all carriers
            for op in 0..4u8 {
                let off = op * 8;
                chip.write(0x40 + off, 0x01);
                chip.write(0x60 + off, 0);
                chip.write(0x80 + off, 31);
                chip.write(0xA0 + off, 0);
                chip.write(0xC0 + off, 0);
                chip.write(0xE0 + off, 0);
            }
            chip.write(0x28, 0x4A);
            chip.write(0x08, 0x78);
            let mut buf = [(0i16, 0i16); 512];
            chip.generate(&mut buf);
            let flat: Vec<u16> = buf
                .iter()
                .flat_map(|&(l, r)| [l as u16, r as u16])
                .collect();
            crate::tables::fnv1a_u16(&flat)
        };
        let none = render(0);
        let mut distinct = std::collections::BTreeSet::new();
        for fb in 1..8u8 {
            let h = render(fb);
            assert_ne!(h, none, "feedback {fb} is audible");
            distinct.insert(h);
        }
        assert_eq!(distinct.len(), 7, "seven distinct non-zero feedback levels");
    }

    /// Pan bits gate each channel to each output independently.
    ///
    /// A core that ORed them, or that applied the same bit to both, passes a test
    /// that only checks "there is sound". This checks all four combinations.
    ///
    /// The plan asked for the `0x40`-is-left reading to be confirmed against
    /// `ymfm_opm.h`'s `ch_output_0` before implementing. It is confirmed twice over:
    /// `ymfm_opm.h:195-199` gives `ch_output_0 = byte(0x20, 6, 1)` and
    /// `add_to_output` writes `data[0]` — the left channel — from it, and the four
    /// combinations below were measured against ymfm as `(f,f)`, `(t,f)`, `(f,t)`,
    /// `(t,t)`. No flip.
    #[test]
    fn the_pan_bits_gate_each_output_independently() {
        for (bits, want_l, want_r) in [
            (0x00u8, false, false),
            (0x40, true, false),
            (0x80, false, true),
            (0xC0, true, true),
        ] {
            let mut chip = crate::Ym2151::new();
            chip.write(0x20, bits | 7);
            for op in 0..4u8 {
                let off = op * 8;
                chip.write(0x40 + off, 0x01);
                chip.write(0x60 + off, 0);
                chip.write(0x80 + off, 31);
                chip.write(0xA0 + off, 0);
                chip.write(0xC0 + off, 0);
                chip.write(0xE0 + off, 0);
            }
            chip.write(0x28, 0x4A);
            chip.write(0x08, 0x78);
            let mut buf = [(0i16, 0i16); 256];
            chip.generate(&mut buf);
            let l = buf.iter().any(|&(l, _)| l != 0);
            let r = buf.iter().any(|&(_, r)| r != 0);
            assert_eq!(l, want_l, "left for pan {bits:#04x}");
            assert_eq!(r, want_r, "right for pan {bits:#04x}");
        }
    }

    /// The decoded table is uniform enough for one loop to run all eight algorithms.
    ///
    /// Column 0 is structurally 0 and column 7 structurally 1. Both are asserted
    /// because [`Channel::output`] relies on them: it never reads column 0 (operator
    /// 1's modulation is the feedback path) and never reads column 7 (operator 4's
    /// output *is* the result). A row that broke either would be silently mis-decoded
    /// rather than caught.
    #[test]
    fn the_table_is_indexable_by_slot() {
        for (alg, row) in ALGORITHM_OPS.iter().enumerate() {
            assert_eq!(
                row[0], 0,
                "algorithm {alg} has no opout input for operator 1"
            );
            assert_eq!(row[7], 1, "algorithm {alg} consumes operator 4");
            for (slot, &input) in row.iter().enumerate().take(4).skip(1) {
                assert!(
                    input != 4 && input < 8,
                    "algorithm {alg} slot {slot} names a real opout entry: {input}"
                );
            }
        }
        // Algorithm 0 is the pure chain and 7 the four-way parallel: the two extremes
        // of the sum mask, which is what makes the assertions above non-vacuous.
        assert_eq!(ALGORITHM_OPS[0], [0, 1, 2, 3, 0, 0, 0, 1]);
        assert_eq!(ALGORITHM_OPS[7], [0, 0, 0, 0, 1, 1, 1, 1]);
    }

    /// A slot's register operator is `slot_of` applied twice — that is, itself.
    ///
    /// The map has to work in both directions: register operator to slot when a
    /// key-on mask arrives, slot to register operator when a cache is filled. It is
    /// the same function only because the two-bit swap is an involution, which is
    /// asserted here rather than assumed.
    #[test]
    fn the_slot_to_register_map_is_its_own_inverse() {
        for slot in 0..4u32 {
            assert_eq!(slot_of(slot_of(slot)), slot, "slot {slot} round-trips");
        }
        let ch0: Vec<u32> = (0..4)
            .map(|slot| Channel::operator_index(0, slot))
            .collect();
        assert_eq!(
            ch0,
            vec![0, 16, 8, 24],
            "ymfm's operator_list(0, 16, 8, 24)"
        );
        let ch5: Vec<u32> = (0..4)
            .map(|slot| Channel::operator_index(5, slot))
            .collect();
        assert_eq!(ch5, vec![5, 21, 13, 29], "offset by the channel index");
    }
}
