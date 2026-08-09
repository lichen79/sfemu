//! One FM operator: its phase, its four-state envelope, and its output.
//!
//! # Four states, not six
//!
//! ymfm's envelope has six states because it serves six chip families: `EG_DEPRESS`
//! is OPLL's, `EG_REVERB` is OPQ/OPZ's, and the whole SSG-EG inversion machinery is
//! OPN's. All three are guarded behind `RegisterType::EG_HAS_*`, and **the OPM has
//! none of them**. This operator has exactly four states, no `ssg_inverted` field,
//! and no depress or reverb branch — porting them would add code no OPM register can
//! reach and no test could distinguish.
//!
//! # Two places the port deliberately differs from ymfm
//!
//! 1. **AM enable is the caller's decision.** ymfm's `envelope_attenuation` reads
//!    `m_regs.op_lfo_am_enable(m_opoffs)` and adds `am_offset` only when set. Here
//!    the caller passes 0 for an operator with AM disabled, which is identical
//!    arithmetic and keeps [`OpCache`] free of a copy of one register bit.
//! 2. **No `eg_shift`.** ymfm's cache carries one, but only `ymfm_opz.cpp` and
//!    `ymfm_opx.h` ever write it, from registers both files mark "fake". OPM always
//!    leaves it 0, so a ported `>> eg_shift` would be a branch no test could ever
//!    make fire.
//!
//! # The attack formula's width
//!
//! ymfm's attack step is `m_env_attenuation += (~m_env_attenuation * increment) >> 4`
//! on a `uint16_t` field, where `~` promotes to 32 bits and the multiply is unsigned
//! because `increment` is a `uint32_t`. The result is then truncated back into 16
//! bits. This port masks to 10 bits instead, which is *not* obviously the same
//! thing — so `the_attack_step_matches_the_reference_truncation` checks the two
//! forms against each other over every reachable attenuation and increment.

use crate::tables;

/// The attenuation past which an operator contributes nothing.
///
/// `ymfm_fm.h:168`. **Not `0x200`** — measured, a `0x200` gate wrongly silences
/// 263,380 of the (attenuation, phase) pairs above it, whose peak magnitude is 31.
/// Quiet, but the difference between a decaying note and one that stops mid-decay.
///
/// Above `0x33F` the power table yields 0 at every phase, so the gate's exact
/// position anywhere in `0x340..=0x3FF` is not observable in the output. It is
/// observable in [`Operator::is_active`], which the debugger reports.
pub const EG_QUIET: u16 = 0x380;

/// A key-on written by the guest through register `0x08`.
pub const KEYON_NORMAL: u8 = 0;

/// A key-on synthesised by CSM mode on a timer A overflow.
///
/// The distinction matters because CSM's key-on is *consumed* by `prepare()`, which
/// is why the lazy `prepare()` gate is semantics rather than an optimisation.
pub const KEYON_CSM: u8 = 2;

/// The envelope's four states, in the order [`OpCache::eg_rate`] indexes them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnvState {
    /// Attenuation falls towards 0.
    Attack,
    /// Attenuation rises towards the sustain level.
    Decay,
    /// Attenuation rises from the sustain level towards silence.
    Sustain,
    /// Attenuation rises towards silence after a key-off.
    Release,
}

/// The per-operator values `prepare()` computes once and the sample loop reads.
///
/// This is ymfm's `opdata_cache` minus the fields the OPM cannot use. Task 4 fills
/// it from the registers; Task 3 only reads it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpCache {
    /// The 10.10 phase increment per sample, or [`OpCache::PHASE_STEP_DYNAMIC`].
    pub phase_step: u32,
    /// Total level, already scaled by 8 into envelope units.
    pub total_level: u16,
    /// The sustain level, already shifted into envelope units.
    pub eg_sustain: u16,
    /// The effective 6-bit rate for each [`EnvState`], KSR included.
    pub eg_rate: [u8; 4],
    /// The detune-1 adjustment, signed, in phase-table units.
    pub detune: i32,
    /// The frequency multiple as an x.1 value: 1 means a half.
    pub multiple: u32,
    /// The raw 13-bit `BBBCCCCFFFFFF` word this cache was computed from.
    pub block_freq: u32,
}

impl OpCache {
    /// A `phase_step` of 1 means "recompute every sample" — PM is active.
    ///
    /// A real step of 1 is unreachable: the smallest phase table entry is 41,568
    /// (`PHASE_STEP[0]`) and the multiple is at least 1, so no register setting
    /// produces a step this small. ymfm relies on the same fact.
    pub const PHASE_STEP_DYNAMIC: u32 = 1;
}

/// One operator's mutable state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Operator {
    /// The 10.10 phase accumulator. The top 10 bits index the sine table.
    pub phase: u32,
    /// The envelope's 4.6 attenuation, 0 loudest and `0x3FF` silent.
    pub env_attenuation: u16,
    /// Which of the four states the envelope is in.
    pub env_state: EnvState,
    /// The key state the last `prepare()` observed.
    pub key_state: bool,
    /// One bit per key-on source: [`KEYON_NORMAL`] and [`KEYON_CSM`].
    pub keyon_live: u8,
}

impl Default for Operator {
    fn default() -> Self {
        Self::new()
    }
}

impl Operator {
    /// An operator in its post-reset state: silent, released, phase zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: 0,
            env_attenuation: 0x3FF,
            env_state: EnvState::Release,
            key_state: false,
            keyon_live: 0,
        }
    }

    /// Return to the post-reset state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The top 10 bits of the phase — the sine table index a channel modulates.
    #[must_use]
    pub fn phase_index(&self) -> u32 {
        self.phase >> 10
    }

    /// Record a key-on or key-off from one source, without acting on it.
    ///
    /// The envelope does not move until [`Operator::clock_keystate`] runs, which is
    /// what makes a key-on written and cleared between two `prepare()` calls a
    /// no-op on the real chip.
    pub fn set_keyon(&mut self, on: bool, kind: u8) {
        let bit = 1 << kind;
        if on {
            self.keyon_live |= bit;
        } else {
            self.keyon_live &= !bit;
        }
    }

    /// Whether this operator still contributes: not silently released.
    ///
    /// ymfm's `prepare()` returns this to build its active-channel mask. The spec
    /// measured that mask as a pure optimisation — deleting it changed no sample
    /// over 40,000 — so this is reported for the debugger, not to gate the mix.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.env_state != EnvState::Release || self.env_attenuation < EG_QUIET
    }

    /// Begin the attack, unless one is already in progress.
    ///
    /// A key-on for an already-attacking operator changes nothing — in particular
    /// it does **not** reset the phase, which is why a driver's redundant key-on
    /// does not restart the note.
    pub fn start_attack(&mut self, cache: &OpCache) {
        if self.env_state == EnvState::Attack {
            return;
        }
        self.env_state = EnvState::Attack;
        self.phase = 0;
        // An attack rate of 62 or 63 arrives instantly. Once here, the documented
        // 62/63 glitch keeps `clock_envelope` from moving it again.
        if cache.eg_rate[EnvState::Attack as usize] >= 62 {
            self.env_attenuation = 0;
        }
    }

    /// Begin the release from wherever the envelope currently is.
    pub fn start_release(&mut self) {
        if self.env_state == EnvState::Release {
            return;
        }
        self.env_state = EnvState::Release;
    }

    /// Apply an edge in the key state, starting an attack or a release.
    pub fn clock_keystate(&mut self, keystate: bool, cache: &OpCache) {
        if keystate == self.key_state {
            return;
        }
        self.key_state = keystate;
        if keystate {
            self.start_attack(cache);
        } else {
            self.start_release();
        }
    }

    /// Advance the envelope, if this counter value is a tick for its rate.
    ///
    /// `env_counter` is the engine's sample counter already divided by 4: the caller
    /// clocks envelopes only on every fourth engine count, matching ymfm's
    /// `bitfield(env_counter, 0, 2) == 0` guard before `clock_envelope(counter >> 2)`.
    ///
    /// The rate's top four bits are a shift, which is how one counter drives rates
    /// spanning four orders of magnitude: a rate of `4n` ticks every `2^(11-n)`
    /// counts, and the low three bits of what is left pick one of eight increments
    /// from a packed nibble — that is what makes rate 20 alternate 0, 1, 0, 1 rather
    /// than moving by a half.
    pub fn clock_envelope(&mut self, env_counter: u32, cache: &OpCache) {
        if self.env_state == EnvState::Attack && self.env_attenuation == 0 {
            self.env_state = EnvState::Decay;
        }
        // Immediately after, in the same clock: a sustain level of 0 means decay has
        // nothing to do and sustain starts now. ymfm cites shinobi's cymbals.
        if self.env_state == EnvState::Decay && self.env_attenuation >= cache.eg_sustain {
            self.env_state = EnvState::Sustain;
        }

        let rate = u32::from(cache.eg_rate[self.env_state as usize]);
        let rate_shift = rate >> 2;
        let counter = env_counter << rate_shift;
        if counter & 0x7FF != 0 {
            return;
        }
        let pick = if rate_shift <= 11 { 11 } else { rate_shift };
        let increment = tables::attenuation_increment(rate, counter >> pick);

        if self.env_state == EnvState::Attack {
            // Rates 62 and 63 are the documented glitch: having been snapped to 0
            // by `start_attack`, they do not increment again. nukeykt confirmed it
            // on OPM, OPN, and OPL/OPLL. A core that incremented here would climb
            // away from zero and fade the note in backwards.
            if rate < 62 {
                let a = u32::from(self.env_attenuation);
                let delta = (!a).wrapping_mul(increment) >> 4;
                self.env_attenuation = ((a + delta) & 0x3FF) as u16;
            }
        } else {
            self.env_attenuation += increment as u16;
            if self.env_attenuation >= 0x400 {
                self.env_attenuation = 0x3FF;
            }
        }
        debug_assert!(self.env_attenuation <= 0x3FF, "the field is 10 bits");
    }

    /// Advance the 10.10 phase by one sample's step.
    pub fn clock_phase(&mut self, step: u32) {
        self.phase = self.phase.wrapping_add(step);
    }

    /// The envelope's attenuation with AM and total level folded in, clamped.
    ///
    /// `am_offset` is already gated by this operator's AM enable bit — see the
    /// module docs. Total level is stored pre-scaled, so this is one add.
    #[must_use]
    pub fn envelope_attenuation(&self, am_offset: u32, cache: &OpCache) -> u32 {
        let sum = u32::from(self.env_attenuation) + am_offset + u32::from(cache.total_level);
        sum.min(0x3FF)
    }

    /// This operator's 14-bit signed output for a given phase.
    ///
    /// The early-out above [`EG_QUIET`] is not an optimisation: it is what makes a
    /// decayed note *silent* rather than very quiet, and the suite's non-silence
    /// premise depends on it being the only thing that silences a live note.
    #[must_use]
    pub fn compute_volume(&self, phase: u32, am_offset: u32, cache: &OpCache) -> i32 {
        if self.env_attenuation > EG_QUIET {
            return 0;
        }
        // A 4.8 sine attenuation with the sign in bit 15, plus the envelope's 4.6
        // shifted up to 4.8, makes a 5.8 attenuation to convert to linear volume.
        let sin_attenuation = u32::from(tables::abs_sin_attenuation(phase));
        let env = self.envelope_attenuation(am_offset, cache) << 2;
        let volume = tables::attenuation_to_volume((sin_attenuation & 0x7FFF) + env) as i32;
        if sin_attenuation & 0x8000 != 0 {
            -volume
        } else {
            volume
        }
    }

    /// The noise channel's output: the raw envelope, inverted, without the log curve.
    ///
    /// The application manual says the logarithmic transform is not applied to the
    /// noise path, so this is an 11-bit linear value signed by the LFSR bit rather
    /// than a table lookup.
    #[must_use]
    pub fn compute_noise_volume(&self, am_offset: u32, noise_state: u32, cache: &OpCache) -> i32 {
        let result = ((self.envelope_attenuation(am_offset, cache) ^ 0x3FF) << 1) as i32;
        if noise_state & 1 != 0 {
            -result
        } else {
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(ar: u8, d1r: u8, d2r: u8, rr: u8, d1l: u16, tl: u16) -> OpCache {
        OpCache {
            phase_step: 0,
            total_level: tl << 3,
            eg_sustain: d1l,
            eg_rate: [ar, d1r, d2r, rr],
            detune: 0,
            multiple: 2,
            block_freq: 0x1280,
        }
    }

    /// Attack drives attenuation down to zero, then decay takes over.
    ///
    /// The transition is on `attenuation == 0`, checked at the top of the clock —
    /// not after the increment. A core that transitioned after would spend one
    /// extra envelope tick in attack, which is inaudible in a note and cumulative
    /// over a song.
    #[test]
    fn attack_falls_to_zero_and_hands_over_to_decay() {
        let c = cache(31 * 2, 0, 0, 0, 0x3E0, 0);
        let mut op = Operator::new();
        op.start_attack(&c);
        assert_eq!(op.env_state, EnvState::Attack);
        assert_eq!(op.env_attenuation, 0, "AR >= 62 snaps straight to full");

        // A slower attack takes many ticks and never overshoots.
        let c = cache(20, 0, 0, 0, 0x3E0, 0);
        let mut op = Operator::new();
        op.env_attenuation = 0x3FF;
        op.env_state = EnvState::Attack;
        let mut ticks = 0;
        while op.env_state == EnvState::Attack && ticks < 100_000 {
            ticks += 1;
            op.clock_envelope(ticks, &c);
        }
        assert!(ticks < 100_000, "attack terminated");
        assert_eq!(op.env_state, EnvState::Decay);
        assert_eq!(op.env_attenuation, 0, "and it arrived at zero, not past it");
    }

    /// The masked attack step is the reference's 16-bit truncation, exactly.
    ///
    /// ymfm adds a 32-bit unsigned product into a `uint16_t`; this port masks to 10
    /// bits. That the two agree is a claim about C's promotion rules, so it is
    /// checked over every reachable input rather than reasoned about: attenuation 1
    /// through `0x3FF` (0 is unreachable, since the state machine leaves attack the
    /// moment it hits 0) against every increment the packed table can yield.
    #[test]
    fn the_attack_step_matches_the_reference_truncation() {
        let max_increment = (0..64)
            .flat_map(|rate| (0..8).map(move |i| tables::attenuation_increment(rate, i)))
            .max()
            .expect("the table is not empty");
        assert_eq!(max_increment, 8, "the packed nibbles top out at 8");

        for a in 1..=0x3FFu32 {
            for increment in 0..=max_increment {
                let delta = (!a).wrapping_mul(increment) >> 4;
                let reference = (a + delta) & 0xFFFF; // ymfm: += into a uint16_t
                let ported = (a + delta) & 0x3FF;
                assert_eq!(
                    reference, ported,
                    "attenuation {a:#05x} increment {increment}"
                );
                assert!(ported <= a, "attack only ever falls");
            }
        }
    }

    /// Attack rates 62 and 63 do not increment after the initial key-on.
    ///
    /// A documented chip glitch (ymfm cites nukeykt, confirmed on OPM/OPN/OPL).
    /// `start_attack` snaps to 0 for those rates, and `clock_envelope` must then
    /// leave them alone — a core that incremented would climb *away* from zero and
    /// the note would fade in backwards.
    #[test]
    fn attack_rates_sixty_two_and_above_do_not_increment_when_clocked() {
        for rate in [62u8, 63] {
            let c = cache(rate, 0, 0, 0, 0x3E0, 0);
            let mut op = Operator::new();
            op.env_state = EnvState::Attack;
            op.env_attenuation = 0x100; // deliberately not 0
            op.clock_envelope(4, &c);
            assert_eq!(
                op.env_attenuation, 0x100,
                "rate {rate} is the glitch case and must not move"
            );
        }
        // Rate 61 is the highest that still moves, which is what makes the two
        // assertions above about the glitch rather than about a dead code path.
        let c = cache(61, 0, 0, 0, 0x3E0, 0);
        let mut op = Operator::new();
        op.env_state = EnvState::Attack;
        op.env_attenuation = 0x100;
        op.clock_envelope(4, &c);
        assert!(op.env_attenuation < 0x100, "rate 61 still attacks");
    }

    /// Decay ends at the sustain level, and a zero sustain level skips decay.
    ///
    /// The decay-to-sustain check runs immediately after attack-to-decay in the
    /// same clock, so a D1L of 0 goes attack -> decay -> sustain without a single
    /// decay tick. ymfm cites shinobi's cymbals as the audible case.
    #[test]
    fn a_zero_sustain_level_skips_decay_entirely() {
        let c = cache(62, 10, 10, 10, 0, 0);
        let mut op = Operator::new();
        op.start_attack(&c);
        assert_eq!(op.env_attenuation, 0);
        op.clock_envelope(4, &c);
        assert_eq!(
            op.env_state,
            EnvState::Sustain,
            "attack -> decay -> sustain in one clock"
        );

        // With a real sustain level it stops in decay instead, which is what makes
        // the assertion above about the zero case and not about the ordering alone.
        let c = cache(62, 10, 10, 10, 0x3E0, 0);
        let mut op = Operator::new();
        op.start_attack(&c);
        op.clock_envelope(4, &c);
        assert_eq!(op.env_state, EnvState::Decay, "0x3E0 is a long way down");
    }

    /// Release clamps at 0x3FF and stops.
    ///
    /// `0x400` would wrap a 10-bit field back to silence-becomes-loud, which is the
    /// classic click at the end of a note.
    #[test]
    fn release_clamps_rather_than_wrapping() {
        let c = cache(62, 0, 0, 31 * 4 + 2, 0, 0);
        let mut op = Operator::new();
        op.env_state = EnvState::Release;
        op.env_attenuation = 0x3F0;
        for t in 1..200u32 {
            op.clock_envelope(t * 4, &c);
        }
        assert_eq!(op.env_attenuation, 0x3FF, "clamped, not wrapped");
    }

    /// A key-off during attack goes to release from wherever the envelope got to.
    ///
    /// **This is the case the spec measured as invisible.** RR was undetected in
    /// 0 of 200 generated cases until every case keyed off, because a held note
    /// never enters release. The suite fixed that with a key-off at sample 256;
    /// this test is the unit-level counterpart.
    #[test]
    fn key_off_mid_attack_releases_from_the_current_attenuation() {
        let c = cache(20, 0, 0, 20, 0x3E0, 0);
        let mut op = Operator::new();
        op.clock_keystate(true, &c);
        assert_eq!(op.env_state, EnvState::Attack);
        for t in 1..50u32 {
            op.clock_envelope(t * 4, &c);
        }
        let mid = op.env_attenuation;
        assert!(mid > 0 && mid < 0x3FF, "partway through attack: {mid}");
        op.clock_keystate(false, &c);
        assert_eq!(op.env_state, EnvState::Release);
        assert_eq!(
            op.env_attenuation, mid,
            "release starts where attack stopped"
        );
    }

    /// Key-on is idempotent: re-keying an already-attacking operator changes nothing.
    ///
    /// `start_attack` returns early when already in attack, so the phase is *not*
    /// reset. A core that reset it would restart every note that got a redundant
    /// key-on, which sound drivers emit routinely.
    #[test]
    fn a_redundant_key_on_does_not_restart_the_phase() {
        let c = cache(20, 0, 0, 20, 0x3E0, 0);
        let mut op = Operator::new();
        op.clock_keystate(true, &c);
        op.phase = 0x1234;
        op.clock_keystate(true, &c);
        assert_eq!(op.phase, 0x1234, "still attacking, nothing restarted");
        assert_eq!(op.env_state, EnvState::Attack);
    }

    /// Key-on resets the phase; that is what makes a note start at zero crossing.
    #[test]
    fn a_fresh_key_on_resets_the_phase() {
        let c = cache(20, 0, 0, 20, 0x3E0, 0);
        let mut op = Operator::new();
        op.phase = 0x9999;
        op.clock_keystate(true, &c);
        assert_eq!(op.phase, 0, "a new note starts at phase zero");
    }

    /// Total level and the envelope add in the attenuation domain, and clamp.
    #[test]
    fn total_level_adds_to_the_envelope_and_the_sum_clamps() {
        let c = cache(31 * 2, 0, 0, 0, 0x3E0, 100);
        let mut op = Operator::new();
        op.env_attenuation = 0x300;
        // TL is stored << 3, so 100 becomes 800; 0x300 is 768; the sum exceeds
        // 0x3FF and must clamp rather than wrap.
        assert_eq!(op.envelope_attenuation(0, &c), 0x3FF);

        // Below the clamp the three terms genuinely add — a clamp that swallowed
        // everything would pass the assertion above on its own.
        let c = cache(31 * 2, 0, 0, 0, 0x3E0, 8);
        op.env_attenuation = 0x40;
        assert_eq!(op.envelope_attenuation(0x10, &c), 0x40 + 0x10 + 64);
    }

    /// An operator past EG_QUIET contributes exactly zero.
    ///
    /// The early-out is not an optimisation: it is what makes a decayed note
    /// silent rather than very quiet, and the suite's non-silence premise depends
    /// on it being the *only* thing that silences a live note.
    #[test]
    fn a_fully_decayed_operator_outputs_silence() {
        let c = cache(62, 0, 0, 0, 0, 0);
        let mut op = Operator::new();
        op.env_attenuation = EG_QUIET + 1;
        assert_eq!(op.compute_volume(0x100, 0, &c), 0);

        // Without the assertions below, the gate could sit anywhere at or below
        // EG_QUIET and the one above would still pass — including at 0x200, which
        // is what the plan's sketch said and which wrongly silences 263,380
        // (attenuation, phase) pairs. `0x33F` is the loudest of them: measured as
        // the highest attenuation the power table still yields a non-zero volume
        // for at any phase, so it is exactly the case a 0x200 gate gets wrong.
        for attenuation in [0x201, 0x300, 0x33F] {
            op.env_attenuation = attenuation;
            assert_ne!(
                op.compute_volume(0x100, 0, &c),
                0,
                "attenuation {attenuation:#05x} is above 0x200 and still audible"
            );
        }
        // And past 0x33F the table itself returns 0, which is why the gate's exact
        // position between 0x340 and 0x3FF is not observable in the output.
        op.env_attenuation = 0x340;
        assert_eq!(op.compute_volume(0x100, 0, &c), 0, "the table has run out");
    }

    /// The output is signed by the sine's half, and symmetric across it.
    ///
    /// Phases `0x000`-`0x1FF` are the positive half and `0x200`-`0x3FF` the
    /// negative one. A core that dropped the sign bit would output a rectified
    /// wave: audible, and the suite would only report it as a whole-buffer
    /// mismatch.
    #[test]
    fn the_two_halves_of_the_sine_have_opposite_signs() {
        let c = cache(62, 0, 0, 0, 0, 0);
        let mut op = Operator::new();
        op.env_attenuation = 0;
        for phase in 0..0x200u32 {
            let positive = op.compute_volume(phase, 0, &c);
            let negative = op.compute_volume(phase + 0x200, 0, &c);
            assert_eq!(positive, -negative, "phase {phase:#05x}");
        }
        assert!(
            op.compute_volume(0x080, 0, &c) > 0,
            "the first quarter is +"
        );
        assert!(
            op.compute_volume(0x280, 0, &c) < 0,
            "the third quarter is -"
        );
    }

    /// A released, silent operator is inactive; anything else is active.
    #[test]
    fn only_a_silently_released_operator_is_inactive() {
        let mut op = Operator::new();
        assert!(!op.is_active(), "a reset operator is released and silent");
        op.env_attenuation = EG_QUIET - 1;
        assert!(op.is_active(), "still audible in release");
        op.env_attenuation = 0x3FF;
        op.env_state = EnvState::Sustain;
        assert!(op.is_active(), "silent but not released: still active");
    }

    /// Key-on sources are independent bits, and CSM is bit 2.
    #[test]
    fn the_two_key_on_sources_do_not_clobber_each_other() {
        let mut op = Operator::new();
        op.set_keyon(true, KEYON_NORMAL);
        op.set_keyon(true, KEYON_CSM);
        assert_eq!(op.keyon_live, 0b101);
        op.set_keyon(false, KEYON_CSM);
        assert_eq!(op.keyon_live, 0b001, "the guest's key-on survives");
        op.set_keyon(false, KEYON_NORMAL);
        assert_eq!(op.keyon_live, 0);
    }

    /// The noise path is linear, not logarithmic, and the LFSR bit signs it.
    #[test]
    fn the_noise_output_is_linear_and_signed_by_the_lfsr() {
        let c = cache(62, 0, 0, 0, 0, 0);
        let mut op = Operator::new();
        op.env_attenuation = 0;
        assert_eq!(op.compute_noise_volume(0, 0, &c), 0x3FF << 1, "loudest");
        assert_eq!(op.compute_noise_volume(0, 1, &c), -(0x3FF << 1));
        op.env_attenuation = 0x3FF;
        assert_eq!(op.compute_noise_volume(0, 0, &c), 0, "fully attenuated");
    }
}
