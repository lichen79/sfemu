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
///
/// The discriminants are part of the save-state format — [`EnvState::from_u8`] —
/// so reordering these four variants changes the file format as well as the rate
/// indexing.
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

impl EnvState {
    /// The state a save state's byte names.
    ///
    /// The enum has no `Default`, deliberately — an envelope has no neutral state —
    /// so a save state's byte is mapped explicitly. An out-of-range byte becomes
    /// [`EnvState::Release`], the post-reset state: the four values this ever writes
    /// are 0-3, so a fifth can only come from a damaged file, and the frontend's
    /// CRC-32 has already refused those. Release is silent, which is the failure a
    /// user can hear least.
    #[must_use]
    pub const fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Attack,
            1 => Self::Decay,
            2 => Self::Sustain,
            _ => Self::Release,
        }
    }
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

    /// Fill a cache from the registers, as ymfm's `cache_operator_data` does.
    ///
    /// `lfo_pm` is the LFO's raw PM output for this sample. It only reaches the
    /// result through `phase_step`, and only when PM is inactive: when both the PM
    /// depth and the channel's PM sensitivity are non-zero, `phase_step` is set to
    /// [`OpCache::PHASE_STEP_DYNAMIC`] and the sample loop calls
    /// [`OpCache::phase_step_with_pm`] every sample instead. Passing a non-zero
    /// `lfo_pm` here is therefore only meaningful for the non-PM path, which is why
    /// the caller passes 0 outside of tests.
    #[must_use]
    pub fn compute(regs: &crate::regs::Regs, ch: u32, op: u32, lfo_pm: i32) -> Self {
        let block_freq = regs.ch_block_freq(ch);

        // The 5-bit keycode is the top 5 bits of `BBBCCCCFFFFFF` — block plus the
        // top *two* bits of the 4-bit key code. Not the whole key code: a port that
        // used bits 6-10 would index the detune and KSR tables an octave off.
        let key_code = (block_freq >> 8) & 0x1F;

        let multiple = match regs.op_multiple(op) {
            0 => 1,
            mul => mul * 2,
        };

        // The sustain level is 4 bits, but 15 must mean full silence rather than
        // 15/16 of it. `sl |= (sl + 1) & 0x10` sets bit 4 only when sl == 15,
        // widening it to 31 before the shift. Dropping this line makes the loudest
        // patches audibly wrong only at one setting, which is exactly the kind of
        // bug a suite finds and reasoning does not.
        let mut eg_sustain = regs.op_sustain_level(op);
        eg_sustain |= (eg_sustain + 1) & 0x10;
        eg_sustain <<= 5;

        // KSR selects how much of the key code scales the rates: `keycode >> (ksr ^
        // 3)`, so ksr 3 uses all five bits and ksr 0 uses two. The `^ 3` is an
        // inversion, not a subtraction — ksr 0 is the *least* scaling.
        let ksrval = key_code >> (regs.op_ksr(op) ^ 3);

        let mut cache = Self {
            phase_step: 0,
            total_level: u16::try_from(regs.op_total_level(op) << 3).unwrap_or(u16::MAX),
            eg_sustain: u16::try_from(eg_sustain).unwrap_or(u16::MAX),
            eg_rate: [
                effective_rate(regs.op_attack_rate(op) * 2, ksrval),
                effective_rate(regs.op_decay_rate(op) * 2, ksrval),
                effective_rate(regs.op_sustain_rate(op) * 2, ksrval),
                // Release rate is 4 bits, not 5, so it is scaled by 4 and offset by
                // 2 — a released note never has rate 0, which would never finish.
                effective_rate(regs.op_release_rate(op) * 4 + 2, ksrval),
            ],
            detune: tables::detune_adjustment(regs.op_detune(op), key_code),
            multiple,
            block_freq,
        };

        cache.phase_step = if regs.lfo_pm_depth() == 0 || regs.ch_lfo_pm_sens(ch) == 0 {
            cache.phase_step_with_pm(regs, ch, op, lfo_pm)
        } else {
            Self::PHASE_STEP_DYNAMIC
        };
        cache
    }

    /// The phase step for this sample, ymfm's `compute_phase_step`.
    ///
    /// The order of operations is load-bearing and two of the four steps are easy to
    /// transpose without any *shape* test noticing, because both are proportional:
    ///
    /// - **DT2 and PM are added before the table lookup**, in units of 1/768 octave,
    ///   so they move the key code.
    /// - **DT1 is added after the block shift**, in phase-step units, so its size in
    ///   cents shrinks as the octave rises. That asymmetry is what makes DT1 a
    ///   detune and DT2 a transpose.
    ///
    /// Swapping them keeps every ratio test passing and only the vector suite
    /// catches it.
    #[must_use]
    pub fn phase_step_with_pm(
        &self,
        regs: &crate::regs::Regs,
        ch: u32,
        op: u32,
        lfo_pm: i32,
    ) -> u32 {
        // The manual gives DT2 in cents; ymfm converts to 1/64ths of a semitone,
        // which is the table's unit. Written as the same rounding expression rather
        // than the four results so the provenance stays visible.
        const DETUNE2_DELTA: [i32; 4] = [
            0,
            (600 * 64 + 50) / 100,
            (781 * 64 + 50) / 100,
            (950 * 64 + 50) / 100,
        ];
        let mut delta = DETUNE2_DELTA[(regs.op_detune2(op) & 3) as usize];

        // PM sensitivity is a shift of the ±200-cent raw value, and it changes
        // *direction* at 6: settings 0-5 shift right by `6 - sens` and 6-7 shift
        // left by `sens - 5`. A single-direction reading gives the two loudest
        // vibrato settings as the two quietest.
        let pm_sens = regs.ch_lfo_pm_sens(ch);
        if pm_sens != 0 {
            delta += if pm_sens < 6 {
                lfo_pm >> (6 - pm_sens)
            } else {
                lfo_pm << (pm_sens - 5)
            };
        }

        let step = key_code_to_phase_step(self.block_freq, delta);
        // Wrapping because `detune` is signed and ymfm's `uint32_t += int32_t` wraps
        // here too. Reachable only at the very bottom of the range, where the table
        // entry is smaller than a downward detune.
        let step = step.wrapping_add(self.detune as u32);
        (step.wrapping_mul(self.multiple)) >> 1
    }

    /// Appends this cache to a save state, in [`crate::state::OP_CACHE_BYTES`] bytes.
    ///
    /// The cache is derived data — [`OpCache::compute`] rebuilds it from the
    /// registers — but it is not *only* derived: the chip's prepare gate means a
    /// restored chip may run up to 4,096 samples before recomputing it, and until
    /// then it plays whatever the cache holds. Carrying it is what makes a restored
    /// chip's next sample identical rather than merely eventually identical.
    pub fn write_state(&self, w: &mut crate::state::StateWriter<'_>) {
        w.u32(self.phase_step);
        w.u16(self.total_level);
        w.u16(self.eg_sustain);
        for &rate in &self.eg_rate {
            w.u8(rate);
        }
        w.i32(self.detune);
        w.u32(self.multiple);
        w.u32(self.block_freq);
    }

    /// A cache read back from a save state. See [`OpCache::write_state`].
    #[must_use]
    pub fn read_state(r: &mut crate::state::StateReader<'_>) -> Self {
        Self {
            phase_step: r.u32(),
            total_level: r.u16(),
            eg_sustain: r.u16(),
            eg_rate: [r.u8(), r.u8(), r.u8(), r.u8()],
            detune: r.i32(),
            multiple: r.u32(),
            block_freq: r.u32(),
        }
    }
}

/// A raw rate plus its KSR adjustment, saturated at 63.
///
/// `ymfm_fm.h`'s `effective_rate`. **Rate 0 stays 0** rather than becoming `ksr` —
/// that is what makes "attack rate 0" mean "never attack" rather than "attack
/// slowly", and it is the branch that keeps a silent operator silent.
#[must_use]
pub fn effective_rate(raw: u32, ksr: u32) -> u8 {
    if raw == 0 {
        0
    } else {
        u8::try_from((raw + ksr).min(63)).unwrap_or(63)
    }
}

/// An OPM block/key-code/fraction word plus a delta, as a phase step.
///
/// `opm_key_code_to_phase_step` from `ymfm_fm.ipp:206`. Two subtleties:
///
/// 1. **The key code is gappy.** Its 4 bits hold twelve notes across sixteen codes,
///    with note 3 of each group of four unused. `adjusted_code = code - (code >> 2)`
///    multiplies by 3/4 to close the gaps before the table lookup. A port using the
///    raw nibble gets four wrong notes per octave.
/// 2. **Over- and underflow adjust the block, not the index.** The `uint32_t` compare
///    catches both signs at once: a negative `eff_freq` wraps to a huge unsigned
///    value, so one comparison handles the ±768 and ±1536 cases below.
#[must_use]
pub fn key_code_to_phase_step(block_freq: u32, delta: i32) -> u32 {
    let mut block = (block_freq >> 10) & 7;
    let adjusted_code = ((block_freq >> 6) & 0xF).wrapping_sub((block_freq >> 8) & 3);
    let mut eff_freq = ((adjusted_code << 6) | (block_freq & 0x3F)) as i32 + delta;

    if (eff_freq as u32) >= 768 {
        if eff_freq < 0 {
            // The minimum delta is -512 (PM alone), so at most one octave down.
            eff_freq += 768;
            if block == 0 {
                return tables::PHASE_STEP[0] >> 7;
            }
            block -= 1;
        } else {
            // The maximum is +512 + 608 (PM plus DT2), so up to two octaves up.
            eff_freq -= 768;
            if eff_freq >= 768 {
                block += 1;
                eff_freq -= 768;
            }
            if block >= 7 {
                return tables::PHASE_STEP[767];
            }
            block += 1;
        }
    }

    // Not masked or clamped. The index is in 0..=767 for every one of the 8,192
    // block/freq words crossed with every reachable delta (−508..=1112, i.e. DT2 plus
    // PM at every sensitivity and depth) — verified exhaustively, and asserted by
    // `the_phase_table_index_never_leaves_the_table`. A mask here would turn a
    // future arithmetic error into a wrong note instead of a panic.
    tables::PHASE_STEP[eff_freq as usize] >> (block ^ 7)
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

    /// Appends this operator to a save state, in
    /// [`crate::state::OPERATOR_BYTES`] bytes.
    pub fn write_state(&self, w: &mut crate::state::StateWriter<'_>) {
        w.u32(self.phase);
        w.u16(self.env_attenuation);
        w.u8(self.env_state as u8);
        w.bool(self.key_state);
        w.u8(self.keyon_live);
    }

    /// An operator read back from a save state.
    #[must_use]
    pub fn read_state(r: &mut crate::state::StateReader<'_>) -> Self {
        Self {
            phase: r.u32(),
            env_attenuation: r.u16(),
            env_state: EnvState::from_u8(r.u8()),
            key_state: r.bool(),
            keyon_live: r.u8(),
        }
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

    // ---- OpCache::compute and the phase step ----

    /// Registers with one channel's block/note set and one operator configured.
    fn phase_regs(kc: u8, dt1_mul: u8, dt2: u8) -> crate::regs::Regs {
        let mut r = crate::regs::Regs::new();
        r.write(0x28, kc);
        r.write(0x40, dt1_mul);
        r.write(0xC0, dt2 << 6);
        r
    }

    fn step(kc: u8, dt1_mul: u8, dt2: u8) -> u32 {
        OpCache::compute(&phase_regs(kc, dt1_mul, dt2), 0, 0, 0).phase_step
    }

    /// Each octave doubles the phase step — but not below block 3.
    ///
    /// **The plan asserted exact doubling across all seven blocks and it does not
    /// hold.** The block is applied as `>> (block ^ 7)`, so a low block is a *large*
    /// right shift and truncation loses the low bits. Measured, note 0 with MUL = 1:
    ///
    /// ```text
    /// block:  0    1    2     3     4     5      6
    /// step: 324  649 1299  2598  5196 10392  20784
    /// ```
    ///
    /// 649 is not 2 × 324 and 1299 is not 2 × 649 — each low block rounds down by
    /// half a bit. From block 3 up the shift is small enough that doubling is exact,
    /// which is what this test asserts, together with the truncation bound (each step
    /// is within 1 of twice the last) for the blocks below it. Asserting the bound
    /// everywhere would be weaker; asserting exactness everywhere is false.
    #[test]
    fn each_octave_doubles_the_phase_step_above_the_truncation_floor() {
        let steps: Vec<u32> = (0..7u8).map(|block| step(block << 4, 0x01, 0)).collect();
        assert_eq!(steps, vec![324, 649, 1299, 2598, 5196, 10392, 20784]);
        for (block, w) in steps.windows(2).enumerate() {
            if block >= 3 {
                assert_eq!(w[1], w[0] * 2, "block {block} to {} is exact", block + 1);
            } else {
                let doubled = w[0] * 2;
                assert!(
                    w[1] == doubled || w[1] == doubled + 1,
                    "block {block}: {} vs {doubled}, truncation is at most one",
                    w[1]
                );
            }
        }
    }

    /// The frequency multiple scales the step, and MUL = 0 means one half.
    ///
    /// The field is 4 bits and the multiplier is `2 * mul` except that 0 means 1 —
    /// the step is `base * mul` with `mul = 0` behaving as 0.5. A core that treated 0
    /// as 0 would silence every operator with MUL = 0, a common patch setting.
    #[test]
    fn multiple_zero_is_a_half_not_a_zero() {
        let half = step(0x40, 0x00, 0);
        let one = step(0x40, 0x01, 0);
        let two = step(0x40, 0x02, 0);
        assert_eq!((half, one, two), (2598, 5196, 10392));
        assert!(half > 0, "MUL = 0 is audible");
        assert_eq!(one, half * 2);
        assert_eq!(two, one * 2);
    }

    /// DT2 shifts the key code by a fixed amount, in units of 1/768 octave.
    ///
    /// The four settings add 0, 384, 500, and 608 before the table lookup, so their
    /// effect is a *ratio* — asserted as one here so the test does not depend on the
    /// particular table entry the base note lands on. **The plan's comment was
    /// wrong**: 608/768 is 0.79 of an octave, not "a little under an octave and a
    /// half". The numeric tolerance it chose is fine; the description was not.
    #[test]
    fn detune_two_shifts_the_key_code_by_fixed_amounts() {
        let steps: Vec<u32> = (0..4u8).map(|dt2| step(0x40, 0x01, dt2)).collect();
        assert_eq!(steps, vec![5196, 7348, 8156, 8996]);
        for w in steps.windows(2) {
            assert!(w[1] > w[0], "each DT2 setting raises the pitch: {steps:?}");
        }
        let ratio = f64::from(steps[3]) / f64::from(steps[0]);
        let want = 2f64.powf(608.0 / 768.0);
        assert!(
            (ratio - want).abs() < 0.002,
            "DT2 = 3 is 608/768 of an octave ({want}), got {ratio}"
        );
        assert!(ratio < 2.0, "608/768 of an octave is less than one octave");
    }

    /// DT1 settings 0 and 4 are both no-ops: magnitude 0 with the sign bit set.
    ///
    /// The DT1 field is magnitude in bits 0-1 and sign in bit 2. A core that read all
    /// three bits as a magnitude would make setting 4 a real detune, and one that
    /// dropped the sign would make 6 detune upward.
    #[test]
    fn detune_one_settings_zero_and_four_are_both_no_ops() {
        let zero = step(0x4A, 0x01, 0);
        let four = step(0x4A, 0x01 | (4 << 4), 0);
        assert_eq!(zero, four, "magnitude 0 with the sign set is still 0");

        let up = step(0x4A, 0x01 | (2 << 4), 0);
        let down = step(0x4A, 0x01 | (6 << 4), 0);
        assert!(up > zero, "DT1 = 2 detunes up");
        assert!(down < zero, "DT1 = 6 is the same magnitude down");
        assert_eq!(up - zero, zero - down, "symmetric about the base");
        assert_eq!((down, zero, up), (8242, 8248, 8254));
    }

    /// The DT1 amount depends on the key code — that is what makes it a *detune*.
    ///
    /// A constant offset would be a transpose. The same setting measured an octave
    /// apart gives 2 and 9 phase units, so the offset grows with pitch: a fixed
    /// fraction of the note rather than a fixed frequency.
    #[test]
    fn detune_one_scales_with_the_key_code() {
        let deltas: Vec<u32> = [0x08u8, 0x48]
            .iter()
            .map(|&kc| step(kc, 0x01 | (3 << 4), 0) - step(kc, 0x01, 0))
            .collect();
        assert_eq!(deltas, vec![2, 9], "a detune, not a transpose");
    }

    /// The key code skips note 3 of every group of four.
    ///
    /// `0x28`'s note field takes 0,1,2,4,5,6,8,9,10,12,13,14 — twelve values across
    /// sixteen codes — and the `code - (code >> 2)` correction closes the gaps. A
    /// core using the raw nibble produces four wrong notes per octave.
    #[test]
    fn note_three_of_each_group_is_not_a_note() {
        let steps: Vec<u32> = [0u8, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14]
            .iter()
            .map(|&note| step(0x40 | note, 0x01, 0))
            .collect();
        for w in steps.windows(2) {
            assert!(w[1] > w[0], "twelve ascending semitones: {steps:?}");
        }
        // Eleven semitones is a little under a doubling. This is the assertion that
        // fails if note 3 leaks in: an extra semitone per group pushes the span
        // past an octave.
        let ratio = f64::from(*steps.last().unwrap()) / f64::from(steps[0]);
        assert!(
            ratio > 1.85 && ratio < 1.90,
            "eleven semitones up from the root, got {ratio}"
        );
        // Note 3 is not skipped so much as *aliased*: `code - (code >> 2)` maps both
        // 3 and 4 to adjusted code 3, so writing note 3 sounds note 4. Measured for
        // all four groups. This is the assertion that distinguishes the 3/4
        // correction from a table that simply omits the fourth entry — and it is why
        // code 15 "bleeds into the next octave", as ymfm's comment says: 15 maps to
        // 12, one past the eleven semitones.
        for group in 0..3u8 {
            let three = 4 * group + 3;
            assert_eq!(
                step(0x40 | three, 0x01, 0),
                step(0x40 | (three + 1), 0x01, 0),
                "note {three} aliases note {}",
                three + 1
            );
        }
        assert_eq!(
            step(0x4F, 0x01, 0),
            step(0x50, 0x01, 0),
            "code 15 bleeds into the next octave's root"
        );
    }

    /// The phase table index stays inside the table for every reachable input.
    ///
    /// [`key_code_to_phase_step`] indexes without a mask, which is only safe because
    /// the block adjustment brings every over- and underflow back into 0..=768. This
    /// walks all 8,192 block/freq words against the delta extremes — DT2 = 608 plus
    /// PM at every sensitivity, and the most negative PM — so an arithmetic slip
    /// panics here rather than reading a neighbouring note in the suite.
    #[test]
    fn the_phase_table_index_never_leaves_the_table() {
        for block_freq in 0..(1u32 << 13) {
            for delta in [-508, -256, -1, 0, 1, 384, 608, 1112] {
                let step = key_code_to_phase_step(block_freq, delta);
                assert!(step > 0, "no register setting silences the phase");
            }
        }
    }

    /// DT1 is applied after the block shift and DT2 before the table lookup.
    ///
    /// Both are proportional, so every ratio test above passes with them swapped.
    /// The asymmetry that distinguishes them: DT2 moves the key code, so its effect
    /// in *cents* is the same in every octave, while DT1 adds a constant number of
    /// phase units after the shift, so its effect in cents halves each octave up.
    /// Measured across five octaves, DT2's ratio is constant and DT1's delta is
    /// nearly so — which is the opposite of what a swap produces.
    #[test]
    fn detune_one_is_absolute_and_detune_two_is_proportional() {
        let mut dt2_ratios = vec![];
        let mut dt1_relative = vec![];
        for block in 2..7u8 {
            let kc = (block << 4) | 4;
            let base = f64::from(step(kc, 0x01, 0));
            dt2_ratios.push(f64::from(step(kc, 0x01, 2)) / base);
            dt1_relative.push(f64::from(step(kc, 0x01 | (3 << 4), 0)) / base);
        }
        // DT2's ratio is *identical* across all five octaves — measured, the spread
        // is exactly 0, because it moves the table index and the block shift applies
        // afterwards to both. DT1's shrinks monotonically: 0.259% of the note at
        // block 2 down to 0.069% at block 6, a factor of 3.76. Asserting a threshold
        // on that spread was my first attempt and it fails — the differences are
        // fractions of a percent. Monotonicity plus the end-to-end factor is what
        // actually holds, and a swap of the two additions inverts both.
        for w in dt2_ratios.windows(2) {
            assert_eq!(
                w[0].to_bits(),
                w[1].to_bits(),
                "DT2's ratio is octave-independent to the bit: {dt2_ratios:?}"
            );
        }
        for w in dt1_relative.windows(2) {
            assert!(
                w[1] < w[0],
                "DT1's relative size shrinks each octave: {dt1_relative:?}"
            );
        }
        let shrinkage = (dt1_relative[0] - 1.0) / (dt1_relative[4] - 1.0);
        assert!(
            shrinkage > 3.5 && shrinkage < 4.0,
            "five octaves shrink DT1 by about 3.76x, got {shrinkage}"
        );
    }

    /// Rate 0 stays 0 through KSR, and everything else saturates at 63.
    ///
    /// `effective_rate`'s zero branch is what makes "attack rate 0" mean never
    /// rather than slowly. Without it a key scale of 8 would turn a deliberately
    /// silent operator into an audible one.
    #[test]
    fn rate_zero_is_not_scaled_and_the_rest_saturate() {
        assert_eq!(effective_rate(0, 0), 0);
        assert_eq!(effective_rate(0, 31), 0, "KSR does not revive rate 0");
        assert_eq!(effective_rate(2, 0), 2);
        assert_eq!(effective_rate(2, 7), 9);
        assert_eq!(effective_rate(62, 7), 63, "saturates rather than wrapping");
        assert_eq!(effective_rate(62, 31), 63);
    }

    /// A sustain level of 15 means silence, not fifteen sixteenths of it.
    ///
    /// `sl |= (sl + 1) & 0x10` widens 15 to 31 before the shift, so the decay target
    /// is `0x3E0` rather than `0x1E0`. Dropping the line leaves the loudest fifteen
    /// settings correct and only the sixteenth wrong.
    #[test]
    fn sustain_level_fifteen_is_widened_to_silence() {
        let mut r = crate::regs::Regs::new();
        let mut levels = vec![];
        for sl in 0..16u8 {
            r.write(0xE0, sl << 4);
            levels.push(OpCache::compute(&r, 0, 0, 0).eg_sustain);
        }
        assert_eq!(levels[0], 0, "level 0 does not decay at all");
        assert_eq!(levels[1], 0x20);
        assert_eq!(levels[14], 14 * 0x20);
        assert_eq!(levels[15], 0x3E0, "15 means 31, not 15");
        assert_ne!(levels[15], 15 * 0x20, "the widening is not a no-op");
    }

    /// KSR inverts: setting 0 uses the fewest key-code bits, 3 the most.
    ///
    /// `keycode >> (ksr ^ 3)`. A port reading `>> ksr` gets the ordering backwards,
    /// so high notes decay slower than low ones instead of faster — audible, and
    /// invisible to any test that only varies KSR at one pitch.
    #[test]
    fn key_scaling_zero_is_the_least_scaling_not_the_most() {
        let mut r = crate::regs::Regs::new();
        r.write(0x28, 0x64); // a high key code, so there are bits to shift out
        r.write(0x80, 0x02); // attack rate 2, low enough that KSR is visible
        let rates: Vec<u8> = (0..4u8)
            .map(|ksr| {
                r.write(0x80, 0x02 | (ksr << 6));
                OpCache::compute(&r, 0, 0, 0).eg_rate[EnvState::Attack as usize]
            })
            .collect();
        for w in rates.windows(2) {
            assert!(w[1] > w[0], "higher KSR scales more: {rates:?}");
        }
        // Key code 25 for this block/note, so the four shifts are `25 >> 3`, `>> 2`,
        // `>> 1`, `>> 0` = 3, 6, 12, 25, on top of an attack rate of 2 * 2 = 4.
        assert_eq!(rates, vec![7, 10, 16, 29], "shifts of 3, 2, 1, 0");
    }

    /// PM active makes the step dynamic; either depth or sensitivity at zero pins it.
    ///
    /// The dynamic marker is what tells the sample loop to recompute, so a port that
    /// cached a step while PM was running would produce a vibrato-free note. Both
    /// gates have to be checked: ymfm requires *both* non-zero.
    #[test]
    fn pm_makes_the_phase_step_dynamic_only_when_both_gates_are_open() {
        let mut r = crate::regs::Regs::new();
        r.write(0x28, 0x44);
        r.write(0x40, 0x01);
        assert_ne!(
            OpCache::compute(&r, 0, 0, 0).phase_step,
            OpCache::PHASE_STEP_DYNAMIC,
            "no PM at all"
        );
        r.write(0x19, 0xFF); // PM depth 127
        assert_ne!(
            OpCache::compute(&r, 0, 0, 0).phase_step,
            OpCache::PHASE_STEP_DYNAMIC,
            "depth alone is not enough, sensitivity is 0"
        );
        r.write(0x38, 0x70); // PM sensitivity 7
        assert_eq!(
            OpCache::compute(&r, 0, 0, 0).phase_step,
            OpCache::PHASE_STEP_DYNAMIC,
            "both gates open"
        );
        r.write(0x19, 0x80); // PM depth 0, sensitivity still 7
        assert_ne!(
            OpCache::compute(&r, 0, 0, 0).phase_step,
            OpCache::PHASE_STEP_DYNAMIC,
            "sensitivity alone is not enough either"
        );
    }

    /// PM sensitivity changes shift direction at 6, and 0 is no modulation.
    ///
    /// Settings 0-5 shift the raw value right by `6 - sens`; 6 and 7 shift *left* by
    /// `sens - 5`. A single-direction reading turns the two widest vibrato settings
    /// into the two narrowest, which sounds plausible in isolation.
    #[test]
    fn pm_sensitivity_reverses_shift_direction_at_six() {
        let mut r = crate::regs::Regs::new();
        r.write(0x28, 0x44);
        r.write(0x40, 0x01);
        let cache = OpCache::compute(&r, 0, 0, 0);
        let base = cache.phase_step_with_pm(&r, 0, 0, 0);

        let mut deviations = vec![];
        for sens in 0..8u8 {
            r.write(0x38, sens << 4);
            let up = cache.phase_step_with_pm(&r, 0, 0, 127);
            deviations.push(i64::from(up) - i64::from(base));
        }
        assert_eq!(deviations[0], 0, "sensitivity 0 is no PM");
        for w in deviations.windows(2) {
            assert!(w[1] >= w[0], "wider with each setting: {deviations:?}");
        }
        assert!(
            deviations[7] > deviations[5] * 3,
            "6 and 7 shift left, so they jump: {deviations:?}"
        );

        // And PM is signed at the phase step too, not just at the LFO.
        r.write(0x38, 0x50);
        let down = cache.phase_step_with_pm(&r, 0, 0, -127);
        assert!(down < base, "negative PM lowers the pitch");
    }
}
