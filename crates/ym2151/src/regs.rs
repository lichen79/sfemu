//! The OPM register map and register file.
//!
//! # The one trap in this file
//!
//! A register address names an *operator index*, and that index is not the
//! operator's slot in the algorithm chain: indices 0, 1, 2, 3 are slots 1, 3, 2, 4.
//! Every test that writes all four of a channel's operators identically passes
//! under either map, so [`slot_of`] has its own test rather than relying on the
//! audio comparison to catch it.
//!
//! # The map
//!
//! Addresses below `0x40` are global (test, key on/off, noise, timers, mode, LFO),
//! `0x20`-`0x3F` is a four-family per-channel grid of 8 bytes each, and
//! `0x40`-`0xFF` is a six-family per-operator grid of 32 bytes each. Within a
//! per-operator family the channel is bits 0-2 and the register-operator index is
//! bits 3-4, so one *operator index* — the argument every `op_*` accessor takes —
//! is `channel + 8 * register_operator`, matching ymfm's `opoffs`.
//!
//! # Two places this crate deliberately differs from ymfm's accessors
//!
//! 1. ymfm has an internal fake register `1A` holding PM depth, because `0x19`
//!    carries both depths and its own array has nowhere else to put the second.
//!    `0x1A` is not an OPM address; this file keeps the two depths in named fields
//!    instead, so nothing can mistake the fake for real hardware.
//! 2. ymfm's `noise_frequency()` folds the `^ 0x1F` inversion into the accessor.
//!    Here [`Regs::noise_frequency`] returns the register field as written and
//!    [`Regs::noise_period`] returns the inverted value the noise counter compares
//!    against, so the inversion is named rather than hidden in a getter.

/// The six per-operator register families, `0x40`-`0xFF`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    /// `0x40`-`0x5F`: detune 1 and frequency multiple.
    Dt1Mul,
    /// `0x60`-`0x7F`: total level.
    Tl,
    /// `0x80`-`0x9F`: key scaling and attack rate.
    KsAr,
    /// `0xA0`-`0xBF`: AM enable and first decay rate.
    AmsD1r,
    /// `0xC0`-`0xDF`: detune 2 and second decay rate.
    Dt2D2r,
    /// `0xE0`-`0xFF`: first decay level and release rate.
    D1lRr,
}

/// The OPM's 32 operators: 8 channels of 4.
pub const OPERATOR_COUNT: u32 = 32;

/// The OPM's 8 FM channels.
pub const CHANNEL_COUNT: u32 = 8;

/// The key on/off register. A write here is the only register write with an effect
/// beyond storing a byte, which is why the chip watches for this address by name.
pub const REG_KEY_ON: u8 = 0x08;

/// The mode register: CSM, timer resets, enables, and loads.
pub const REG_MODE: u8 = 0x14;

/// The slot in the algorithm chain for a register-operator index.
///
/// Slots are numbered 0-3 here, meaning chain positions 1-4. The mapping is a
/// two-bit swap: `0 -> 0, 1 -> 2, 2 -> 1, 3 -> 3`.
#[must_use]
pub const fn slot_of(op_index: u32) -> u32 {
    ((op_index & 1) << 1) | ((op_index >> 1) & 1)
}

/// The operator index for a channel and a register-operator index.
///
/// This is the `0..32` value every `op_*` accessor takes, and it is *not* the
/// algorithm slot — see [`slot_of`].
#[must_use]
pub const fn op_index(ch: u32, reg_op: u32) -> u32 {
    (ch & 7) + 8 * (reg_op & 3)
}

/// Which family, channel, and register-operator index an address names.
///
/// `None` below `0x40`, where addresses are global or per-channel rather than
/// per-operator.
#[must_use]
pub fn decode(reg: u8) -> Option<(Family, u32, u32)> {
    if reg < 0x40 {
        return None;
    }
    let family = match reg >> 5 {
        2 => Family::Dt1Mul,
        3 => Family::Tl,
        4 => Family::KsAr,
        5 => Family::AmsD1r,
        6 => Family::Dt2D2r,
        _ => Family::D1lRr,
    };
    Some((family, u32::from(reg & 7), u32::from((reg >> 3) & 3)))
}

/// The channel and operator mask a write to [`REG_KEY_ON`] names.
///
/// Bits 3-6 of the value are operators 1-4 — in *register* order, so bit 3 is
/// register-operator 0 and needs [`slot_of`] before it means a chain position.
#[must_use]
pub const fn key_on_fields(val: u8) -> (u32, u32) {
    ((val & 7) as u32, ((val >> 3) & 0xF) as u32)
}

/// The OPM's 256-byte register file and the two depth registers behind `0x19`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Regs {
    /// Every written byte, by address.
    file: [u8; 0x100],
    /// `0x19` writes here when bit 7 is clear.
    am_depth: u8,
    /// `0x19` writes here when bit 7 is set. **Not** a register `0x1A` — ymfm's
    /// `1A` is an internal fake and is not an OPM address.
    pm_depth: u8,
}

impl Default for Regs {
    fn default() -> Self {
        Self::new()
    }
}

impl Regs {
    /// A register file in its post-reset state.
    #[must_use]
    pub fn new() -> Self {
        let mut regs = Self {
            file: [0; 0x100],
            am_depth: 0,
            pm_depth: 0,
        };
        regs.reset();
        regs
    }

    /// Clear every register, then restore the reset defaults.
    ///
    /// Reset is not all-zero: `opm_registers::reset` re-enables both pan bits on
    /// all eight channels (`0x20`-`0x27` = `0xC0`). A core that reset to zeros is
    /// silent until the driver writes `0x20`, which some do not.
    pub fn reset(&mut self) {
        self.file = [0; 0x100];
        self.am_depth = 0;
        self.pm_depth = 0;
        for ch in 0..CHANNEL_COUNT as usize {
            self.file[0x20 + ch] = 0xC0;
        }
    }

    /// Store one register write.
    ///
    /// Every address is writable and no value is invalid — the OPM has no error
    /// states, and the guest program can write anything. `0x19` routes to one of
    /// the two depth fields by its top bit; everything else lands in the file.
    pub fn write(&mut self, reg: u8, val: u8) {
        if reg == 0x19 {
            if val & 0x80 == 0 {
                self.am_depth = val & 0x7F;
            } else {
                self.pm_depth = val & 0x7F;
            }
        }
        self.file[reg as usize] = val;
    }

    /// The raw byte at an address, for the debugger's register dump.
    #[must_use]
    pub fn byte_at(&self, reg: u8) -> u8 {
        self.file[reg as usize]
    }

    /// `count` bits starting at `start` of the byte at `reg + extra`.
    fn bits(&self, reg: u8, start: u32, count: u32, extra: u32) -> u32 {
        let byte = u32::from(self.file[(u32::from(reg) + extra) as usize & 0xFF]);
        (byte >> start) & ((1 << count) - 1)
    }

    // ---- system-wide registers ----

    /// `0x01` bit 1: holds the LFO in reset while set.
    ///
    /// Officially undocumented; ymfm notes it was discovered rather than published.
    #[must_use]
    pub fn lfo_reset(&self) -> bool {
        self.bits(0x01, 1, 1, 0) != 0
    }

    /// `0x0F` bit 7: noise replaces channel 7's operator 4 output.
    #[must_use]
    pub fn noise_enable(&self) -> bool {
        self.bits(0x0F, 7, 1, 0) != 0
    }

    /// `0x0F` bits 0-4: the noise frequency field, as written.
    ///
    /// Higher values are *faster*. See [`Regs::noise_period`] for the value the
    /// noise counter actually compares against.
    #[must_use]
    pub fn noise_frequency(&self) -> u32 {
        self.bits(0x0F, 0, 5, 0)
    }

    /// The count the noise counter reaches before latching a new bit.
    ///
    /// This is `0x1F - noise_frequency()`, the inversion ymfm folds into its own
    /// `noise_frequency` accessor. Keeping it as its own named accessor is what
    /// stops the inversion from being dropped: without it, a plausible reading of
    /// the register map clocks the LFSR at every frequency but the right one.
    #[must_use]
    pub fn noise_period(&self) -> u32 {
        self.noise_frequency() ^ 0x1F
    }

    /// `0x10` (high 8) and `0x11` (low 2): timer A's 10-bit value.
    #[must_use]
    pub fn timer_a_value(&self) -> u32 {
        (self.bits(0x10, 0, 8, 0) << 2) | self.bits(0x11, 0, 2, 0)
    }

    /// `0x12`: timer B's 8-bit value.
    #[must_use]
    pub fn timer_b_value(&self) -> u32 {
        self.bits(0x12, 0, 8, 0)
    }

    /// `0x14` bit 7: CSM mode, which key-ons every channel on each timer A overflow.
    #[must_use]
    pub fn csm(&self) -> bool {
        self.bits(REG_MODE, 7, 1, 0) != 0
    }

    /// `0x14` bit 5: a one-shot reset of timer B's status bit.
    #[must_use]
    pub fn reset_timer_b(&self) -> bool {
        self.bits(REG_MODE, 5, 1, 0) != 0
    }

    /// `0x14` bit 4: a one-shot reset of timer A's status bit.
    #[must_use]
    pub fn reset_timer_a(&self) -> bool {
        self.bits(REG_MODE, 4, 1, 0) != 0
    }

    /// `0x14` bit 3: timer B's overflow reaches the status register.
    #[must_use]
    pub fn enable_timer_b(&self) -> bool {
        self.bits(REG_MODE, 3, 1, 0) != 0
    }

    /// `0x14` bit 2: timer A's overflow reaches the status register.
    #[must_use]
    pub fn enable_timer_a(&self) -> bool {
        self.bits(REG_MODE, 2, 1, 0) != 0
    }

    /// `0x14` bit 1: timer B runs.
    #[must_use]
    pub fn load_timer_b(&self) -> bool {
        self.bits(REG_MODE, 1, 1, 0) != 0
    }

    /// `0x14` bit 0: timer A runs.
    #[must_use]
    pub fn load_timer_a(&self) -> bool {
        self.bits(REG_MODE, 0, 1, 0) != 0
    }

    /// `0x18`: the LFO rate, read as a 4.4 step with an implied leading 1.
    #[must_use]
    pub fn lfo_rate(&self) -> u32 {
        self.bits(0x18, 0, 8, 0)
    }

    /// `0x19` with bit 7 clear: AM depth, 0-127.
    #[must_use]
    pub fn lfo_am_depth(&self) -> u32 {
        u32::from(self.am_depth)
    }

    /// `0x19` with bit 7 set: PM depth, 0-127.
    #[must_use]
    pub fn lfo_pm_depth(&self) -> u32 {
        u32::from(self.pm_depth)
    }

    /// `0x1B` bits 0-1: which of the four LFO waveforms is selected.
    #[must_use]
    pub fn lfo_waveform(&self) -> u32 {
        self.bits(0x1B, 0, 2, 0)
    }

    // ---- per-channel registers ----

    /// `0x20`-`0x27` bits 6 and 7: whether this channel reaches (left, right).
    ///
    /// Bit 6 is left, bit 7 is right — the register map's "pan left" and "pan
    /// right", and ymfm's `ch_output_0` and `ch_output_1` in that order.
    #[must_use]
    pub fn ch_pan(&self, ch: u32) -> (bool, bool) {
        (
            self.bits(0x20, 6, 1, ch & 7) != 0,
            self.bits(0x20, 7, 1, ch & 7) != 0,
        )
    }

    /// `0x20`-`0x27` bits 3-5: operator 1's self-feedback level, 0-7.
    #[must_use]
    pub fn ch_feedback(&self, ch: u32) -> u32 {
        self.bits(0x20, 3, 3, ch & 7)
    }

    /// `0x20`-`0x27` bits 0-2: the operator connection algorithm, 0-7.
    #[must_use]
    pub fn ch_algorithm(&self, ch: u32) -> u32 {
        self.bits(0x20, 0, 3, ch & 7)
    }

    /// The 13-bit `BBBCCCCFFFFFF` block/key-code/fraction word.
    ///
    /// `0x28` supplies the top 7 bits (octave and note) and `0x30` the low 6.
    /// **`0x30`'s low two bits are not used**: the register holds the fraction in
    /// bits 2-7, so the assembly is `(kc << 6) | (kf >> 2)`. Reading all 8 bits of
    /// `0x30` is what made the spec's first sensitivity measurement report a false
    /// 0% — two of the thirteen bits were being fed from nothing.
    #[must_use]
    pub fn ch_block_freq(&self, ch: u32) -> u32 {
        (self.bits(0x28, 0, 7, ch & 7) << 6) | self.bits(0x30, 2, 6, ch & 7)
    }

    /// `0x38`-`0x3F` bits 4-6: LFO phase-modulation sensitivity, 0-7.
    #[must_use]
    pub fn ch_lfo_pm_sens(&self, ch: u32) -> u32 {
        self.bits(0x38, 4, 3, ch & 7)
    }

    /// `0x38`-`0x3F` bits 0-1: LFO amplitude-modulation shift, 0-3.
    #[must_use]
    pub fn ch_lfo_am_sens(&self, ch: u32) -> u32 {
        self.bits(0x38, 0, 2, ch & 7)
    }

    // ---- per-operator registers ----
    //
    // `op` is an operator index in `0..32`: `channel + 8 * register_operator`.
    // See [`op_index`], and [`slot_of`] for the chain position it is *not*.

    /// `0x40`-`0x5F` bits 4-6: detune 1, a key-code-dependent fine detune.
    #[must_use]
    pub fn op_detune(&self, op: u32) -> u32 {
        self.bits(0x40, 4, 3, op % OPERATOR_COUNT)
    }

    /// `0x40`-`0x5F` bits 0-3: the frequency multiple, 0-15, where 0 means a half.
    #[must_use]
    pub fn op_multiple(&self, op: u32) -> u32 {
        self.bits(0x40, 0, 4, op % OPERATOR_COUNT)
    }

    /// `0x60`-`0x7F` bits 0-6: total level, 0 loudest and 127 silent.
    #[must_use]
    pub fn op_total_level(&self, op: u32) -> u32 {
        self.bits(0x60, 0, 7, op % OPERATOR_COUNT)
    }

    /// `0x80`-`0x9F` bits 6-7: key scale rate, 0-3.
    #[must_use]
    pub fn op_ksr(&self, op: u32) -> u32 {
        self.bits(0x80, 6, 2, op % OPERATOR_COUNT)
    }

    /// `0x80`-`0x9F` bits 0-4: attack rate, 0-31.
    #[must_use]
    pub fn op_attack_rate(&self, op: u32) -> u32 {
        self.bits(0x80, 0, 5, op % OPERATOR_COUNT)
    }

    /// `0xA0`-`0xBF` bit 7: whether the LFO's AM reaches this operator.
    #[must_use]
    pub fn op_lfo_am_enable(&self, op: u32) -> bool {
        self.bits(0xA0, 7, 1, op % OPERATOR_COUNT) != 0
    }

    /// `0xA0`-`0xBF` bits 0-4: first decay rate, 0-31.
    #[must_use]
    pub fn op_decay_rate(&self, op: u32) -> u32 {
        self.bits(0xA0, 0, 5, op % OPERATOR_COUNT)
    }

    /// `0xC0`-`0xDF` bits 6-7: detune 2, a coarse pitch offset, 0-3.
    #[must_use]
    pub fn op_detune2(&self, op: u32) -> u32 {
        self.bits(0xC0, 6, 2, op % OPERATOR_COUNT)
    }

    /// `0xC0`-`0xDF` bits 0-4: second decay (sustain) rate, 0-31.
    #[must_use]
    pub fn op_sustain_rate(&self, op: u32) -> u32 {
        self.bits(0xC0, 0, 5, op % OPERATOR_COUNT)
    }

    /// `0xE0`-`0xFF` bits 4-7: first decay level, the attenuation decay stops at.
    #[must_use]
    pub fn op_sustain_level(&self, op: u32) -> u32 {
        self.bits(0xE0, 4, 4, op % OPERATOR_COUNT)
    }

    /// `0xE0`-`0xFF` bits 0-3: release rate, 0-15.
    #[must_use]
    pub fn op_release_rate(&self, op: u32) -> u32 {
        self.bits(0xE0, 0, 4, op % OPERATOR_COUNT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Register-operator index maps to slots 1, 3, 2, 4 — not 1, 2, 3, 4.
    ///
    /// `ymfm_opm.cpp:117-138` states it as "the channel index order is 0,2,1,3, so
    /// we bitswap the index", with `operator_list(0, 16, 8, 24)`. The register map
    /// order is carrier 1, carrier 2, modulator 1, modulator 2 while the natural
    /// wiring order is carrier 1, modulator 1, carrier 2, modulator 2.
    ///
    /// **Measured, not transcribed.** Under algorithm 4 — two independent 2-op
    /// chains, carriers on slots 2 and 4 — silencing register offsets 0x10 and 0x18
    /// halves the output while 0x00 and 0x08 leave it unchanged, which refutes the
    /// naive 0,8,16,24 map. Task 7's audio test is that experiment; this one pins
    /// the arithmetic it depends on.
    #[test]
    fn the_register_operator_index_is_not_the_slot_number() {
        assert_eq!(slot_of(0), 0, "register-operator 0 is slot 1");
        assert_eq!(slot_of(1), 2, "register-operator 1 is slot 3");
        assert_eq!(slot_of(2), 1, "register-operator 2 is slot 2");
        assert_eq!(slot_of(3), 3, "register-operator 3 is slot 4");
        // The naive map is a bitswap away, and this is the assertion that refuses
        // it: an identity `slot_of` passes any test that writes all four operators
        // the same, which is most of them.
        assert_ne!(slot_of(1), 1, "the naive identity map is wrong");
        assert_ne!(slot_of(2), 2);

        // And the operator index the accessors take is `ch + 8 * reg_op`, which is
        // ymfm's `operator_list(0, 16, 8, 24)` read back out for channel 0.
        let ch0: Vec<u32> = (0..4)
            .map(|slot| {
                let reg_op = (0..4).find(|&r| slot_of(r) == slot).unwrap();
                op_index(0, reg_op)
            })
            .collect();
        assert_eq!(ch0, vec![0, 16, 8, 24], "operator_list(0, 16, 8, 24)");
    }

    /// A register address decodes to its family, channel, and operator.
    ///
    /// Written from the OPM register map (`ymfm_opm.h:47-101`), not read off the
    /// implementation. `0x40`-`0xFF` is a 6-family grid: each family is 32 bytes,
    /// `channel = reg & 7`, `operator = (reg >> 3) & 3`.
    #[test]
    fn addresses_decode_to_family_channel_and_operator() {
        assert_eq!(decode(0x40), Some((Family::Dt1Mul, 0, 0)));
        assert_eq!(decode(0x47), Some((Family::Dt1Mul, 7, 0)));
        assert_eq!(decode(0x48), Some((Family::Dt1Mul, 0, 1)));
        assert_eq!(decode(0x5F), Some((Family::Dt1Mul, 7, 3)));
        assert_eq!(decode(0x60), Some((Family::Tl, 0, 0)));
        assert_eq!(decode(0x80), Some((Family::KsAr, 0, 0)));
        assert_eq!(decode(0xA0), Some((Family::AmsD1r, 0, 0)));
        assert_eq!(decode(0xC0), Some((Family::Dt2D2r, 0, 0)));
        assert_eq!(decode(0xE0), Some((Family::D1lRr, 0, 0)));
        assert_eq!(decode(0xFF), Some((Family::D1lRr, 7, 3)));
        // Below 0x40 the addresses are global, not per-operator.
        assert_eq!(decode(0x20), None);
        assert_eq!(decode(0x08), None);
    }

    /// `0x19` selects AM or PM depth by its top bit; there is no register `0x1A`.
    ///
    /// ymfm has an internal fake register `1A` for PM depth. It is not a real OPM
    /// address, and a reader transcribing the two maps must not copy it. Writing
    /// `0x1A` here lands in the register file and affects nothing.
    #[test]
    fn one_nine_carries_both_depths_and_there_is_no_register_one_a() {
        let mut r = Regs::new();
        r.write(0x19, 0x7F); // top bit clear: AM depth
        assert_eq!(r.lfo_am_depth(), 0x7F);
        assert_eq!(r.lfo_pm_depth(), 0, "untouched");
        r.write(0x19, 0xC5); // top bit set: PM depth
        assert_eq!(r.lfo_pm_depth(), 0x45);
        assert_eq!(r.lfo_am_depth(), 0x7F, "AM depth is not clobbered");
        // And the fake address is inert: it is not where PM depth lives.
        r.write(0x1A, 0x7F);
        assert_eq!(r.lfo_pm_depth(), 0x45, "0x1A is not an OPM register");
    }

    /// `0x0F` is noise enable in bit 7 and a 5-bit frequency in the low bits.
    #[test]
    fn noise_enable_and_frequency_share_one_register() {
        let mut r = Regs::new();
        r.write(0x0F, 0x00);
        assert!(!r.noise_enable());
        r.write(0x0F, 0x87);
        assert!(r.noise_enable());
        assert_eq!(r.noise_frequency(), 7);
        r.write(0x0F, 0x1F);
        assert!(!r.noise_enable(), "bit 7 only");
        assert_eq!(r.noise_frequency(), 0x1F);
    }

    /// The noise *period* is the frequency field inverted, and the two are not equal.
    ///
    /// ymfm's `noise_frequency()` returns `field ^ 0x1F` — the counter limit, not
    /// the field. This crate splits them, so this test is what stops the inversion
    /// from being silently dropped: `assert_ne!` on the midpoints fails for any
    /// implementation where `noise_period` is the identity, and the endpoints pin
    /// the direction (a higher register value is a *faster* noise).
    #[test]
    fn the_noise_period_is_the_frequency_field_inverted() {
        let mut r = Regs::new();
        r.write(0x0F, 0x00);
        assert_eq!(r.noise_period(), 0x1F, "field 0 is the slowest noise");
        r.write(0x0F, 0x1F);
        assert_eq!(r.noise_period(), 0, "field 0x1F is the fastest");
        for field in 1..0x1Fu8 {
            r.write(0x0F, field);
            assert_eq!(r.noise_period(), u32::from(field) ^ 0x1F);
            if field != 0x0F && field != 0x10 {
                assert_ne!(
                    r.noise_period(),
                    r.noise_frequency(),
                    "the period is not the field at {field:#04x}"
                );
            }
        }
    }

    /// `0x14` is the mode register: timer loads, enables, resets, and CSM.
    ///
    /// CSM is bit 7 and is the bit the whole `prepare()` gate turns on — see
    /// Task 9. `enable_timer_a` is bit 2, `enable_timer_b` bit 3.
    #[test]
    fn the_mode_register_decodes_csm_and_the_timer_enables() {
        let mut r = Regs::new();
        r.write(0x14, 0x00);
        assert!(!r.csm());
        assert!(!r.enable_timer_a());
        assert!(!r.enable_timer_b());
        r.write(0x14, 0x3F);
        assert!(!r.csm(), "bit 7 clear");
        assert!(r.enable_timer_a());
        assert!(r.enable_timer_b());
        assert!(r.load_timer_a());
        assert!(r.load_timer_b());
        assert!(r.reset_timer_a());
        assert!(r.reset_timer_b());
        r.write(0x14, 0xBF);
        assert!(r.csm(), "and bit 7 set turns CSM on");
    }

    /// Timer A is 10 bits split across two registers; timer B is 8 in one.
    #[test]
    fn timer_a_spans_two_registers_and_timer_b_one() {
        let mut r = Regs::new();
        r.write(0x10, 0xFF); // high 8 bits
        r.write(0x11, 0x03); // low 2 bits
        assert_eq!(r.timer_a_value(), 0x3FF);
        r.write(0x10, 0x30);
        r.write(0x11, 0x01);
        assert_eq!(r.timer_a_value(), (0x30 << 2) | 1);
        r.write(0x12, 0x40);
        assert_eq!(r.timer_b_value(), 0x40);
    }

    /// Key code is 7 bits, and note 3 of each octave does not exist.
    ///
    /// `0x28` holds octave in bits 4-6 and note in bits 0-3, but the note field
    /// only uses values 0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14 — the OPM skips
    /// every fourth. The register file stores what was written; interpretation is
    /// the phase calculation's job (Task 4).
    #[test]
    fn the_key_code_register_stores_seven_bits() {
        let mut r = Regs::new();
        r.write(0x28, 0x4A);
        assert_eq!(
            r.ch_block_freq(0) >> 6,
            0x4A,
            "octave and note in the top bits"
        );
        r.write(0x30, 0xFC);
        assert_eq!(r.ch_block_freq(0) & 0x3F, 0x3F, "key fraction in the low 6");
        // `0x30`'s low two bits are not part of the word: setting them changes
        // nothing. This is the assertion that fails on a naive 8-bit read.
        r.write(0x30, 0xFF);
        assert_eq!(
            r.ch_block_freq(0) & 0x3F,
            0x3F,
            "bits 0-1 of 0x30 are unused"
        );
        assert_eq!(r.ch_block_freq(0), (0x4A << 6) | 0x3F);
    }

    /// Reset re-enables both pan bits rather than zeroing everything.
    ///
    /// `opm_registers::reset` writes `0xC0` to `0x20`-`0x27`. A core that reset to
    /// all zeros is silent on every channel until the driver writes `0x20`, and the
    /// suite would report it as a whole-buffer mismatch saying nothing about why.
    #[test]
    fn reset_leaves_both_pans_enabled_on_every_channel() {
        let mut r = Regs::new();
        for ch in 0..CHANNEL_COUNT {
            assert_eq!(r.ch_pan(ch), (true, true), "channel {ch} after new()");
            assert_eq!(r.ch_algorithm(ch), 0);
            assert_eq!(r.ch_feedback(ch), 0);
        }
        for reg in 0..=0xFFu8 {
            r.write(reg, 0x5A);
        }
        r.reset();
        assert_eq!(r, Regs::new(), "and reset undoes a full sweep of writes");
    }

    /// The per-operator accessors read the family they name, at `ch + 8 * reg_op`.
    ///
    /// One distinct value per field, so a copy-paste that points two accessors at
    /// the same register or the same bit range fails here rather than showing up as
    /// an audio mismatch 3,000 lines later.
    #[test]
    fn each_per_operator_accessor_reads_its_own_field() {
        let mut r = Regs::new();
        let op = op_index(5, 2); // channel 5, register-operator 2 -> index 21
        assert_eq!(op, 21);
        r.write(0x40 + op as u8, 0x35); // DT1 = 3, MUL = 5
        r.write(0x60 + op as u8, 0x51); // TL = 0x51
        r.write(0x80 + op as u8, 0x87); // KS = 2, AR = 7
        r.write(0xA0 + op as u8, 0x89); // AM enable, D1R = 9
        r.write(0xC0 + op as u8, 0x4B); // DT2 = 1, D2R = 11
        r.write(0xE0 + op as u8, 0x6D); // D1L = 6, RR = 13
        assert_eq!(r.op_detune(op), 3);
        assert_eq!(r.op_multiple(op), 5);
        assert_eq!(r.op_total_level(op), 0x51);
        assert_eq!(r.op_ksr(op), 2);
        assert_eq!(r.op_attack_rate(op), 7);
        assert!(r.op_lfo_am_enable(op));
        assert_eq!(r.op_decay_rate(op), 9);
        assert_eq!(r.op_detune2(op), 1);
        assert_eq!(r.op_sustain_rate(op), 11);
        assert_eq!(r.op_sustain_level(op), 6);
        assert_eq!(r.op_release_rate(op), 13);
        // A neighbouring operator index is a different byte, not the same one.
        assert_eq!(r.op_multiple(op + 1), 0, "operator 22 is untouched");
        assert_eq!(r.op_multiple(op - 8), 0, "register-operator 1 is untouched");
    }

    /// A key-on write names a channel in bits 0-2 and four operators in bits 3-6.
    #[test]
    fn a_key_on_write_names_a_channel_and_four_register_operators() {
        assert_eq!(key_on_fields(0x78), (0, 0xF), "all four of channel 0");
        assert_eq!(key_on_fields(0x0F), (7, 0x1), "operator 1 of channel 7");
        assert_eq!(key_on_fields(0x00), (0, 0x0), "key off");
        assert_eq!(key_on_fields(0x22), (2, 0x4), "register-operator 2");
        // The mask is in register order, so slot_of stands between it and a chain
        // position: bit 3 + 1 is register-operator 1, which is *slot 3*.
        assert_eq!(slot_of(1), 2);
    }
}
