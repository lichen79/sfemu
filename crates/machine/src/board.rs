//! The CPS-1 board: everything the 68000 can address.
//!
//! # Why this is a separate struct from the top-level machine
//!
//! `M68k::step_with(&dec, &mut bus)` borrows the CPU and the bus mutably at the
//! same time, so the CPU cannot live inside the thing it buses to. Splitting them
//! at the top level makes `self.cpu.step_with(&self.dec, &mut self.board)` legal
//! with no `RefCell`, no state swapping, and no `unsafe` — preserving
//! sub-project A's `forbid(unsafe_code)` posture through the board layer.
//!
//! # Never panics on a guest address
//!
//! Every index is produced by masking or a nonzero remainder, not by a
//! bounds-checked slice index on guest-supplied arithmetic. A mis-emulated jump
//! produces wild addresses as a matter of course, and an emulator that panics on
//! one has turned a guest fault into a host crash. See
//! `no_address_in_the_whole_24_bit_space_panics` at the bottom of this file.
//!
//! Map cited to MAME `master`, `src/mame/capcom/cps1.cpp:577-594`
//! (`cps_state::main_map`), read 2026-08-07.

use crate::config::BoardConfig;
use crate::inputs::Inputs;
use crate::trace::Trace;
use m68k::Bus;

/// Main RAM, 0xFF0000-0xFFFFFF: 64 KB = 32 K words (`cps1.cpp:593`).
pub(crate) const RAM_WORDS: usize = 0x8000;
/// gfxram, 0x900000-0x92FFFF: 192 KB = 96 K words (`cps1.cpp:592`).
pub(crate) const GFXRAM_WORDS: usize = 0x1_8000;
/// Program ROM space, 0x000000-0x3FFFFF (`CODE_SIZE`, `cps1.cpp:4063`).
const ROM_BYTES: usize = 0x40_0000;

/// First byte of gfxram.
const GFXRAM_BASE: u32 = 0x90_0000;

/// CPS-A register file base, 0x800100-0x80013F (`cps1.cpp:586`).
const CPS_A_BASE: u32 = 0x80_0100;
/// CPS-B register file base, 0x800140-0x80017F (`cps1.cpp:589`).
const CPS_B_BASE: u32 = 0x80_0140;
/// Both custom register files are 0x40 bytes = 32 words.
pub(crate) const CPS_REGS: usize = 0x20;

/// What an unmapped read returns.
///
/// The 68000's data bus floats high on an access no chip answers, and a board
/// with pull-up resistors reads it back as all ones. Zero would be the wrong
/// choice and a dangerous one: `0x0000` decodes as a legal `ori.b #imm, d0`,
/// so a runaway PC in unmapped space would execute quietly instead of quickly
/// taking an exception.
const UNMAPPED: u16 = 0xFFFF;

/// Where the 68000 fetches an autovectored level-2 handler address.
///
/// CPS-1 wires the IPL pins individually — `set_interrupt_mixer(false)`,
/// `cps1.cpp:3913` — so IPL1 is interrupt **level 2**, whose autovector is
/// 24 + 2 = 26, at 26 × 4 = 0x68.
const VEC_AUTOVECTOR_2: u32 = 0x68;

/// Everything on the 68000's bus: program ROM, main RAM, gfxram, and the I/O
/// block at 0x800000-0x80018F.
pub struct Board {
    /// The assembled `maincpu` region, zero-padded to the full ROM space.
    pub rom: Vec<u8>,
    /// Main RAM as words: the 68000 is big-endian and every CPS-1 access to it
    /// is word-oriented, so storing words keeps the byte-order conversion in one
    /// place ([`Bus::read8`] / [`Bus::write8`]) instead of at every use.
    pub ram: Box<[u16; RAM_WORDS]>,
    /// Tilemap/sprite/palette RAM. Sub-project C reads this; the board only
    /// stores it. SF2CE executes code from here, so it is readable as well as
    /// writable (`cps1.cpp:592`).
    pub gfxram: Box<[u16; GFXRAM_WORDS]>,
    /// CPS-A, 0x800100-0x80013F, indexed by **word**.
    ///
    /// Stored and not interpreted: sub-project C owns every meaning. `cps1.h:176-193`
    /// gives the layout, and note that MAME's constants there are already divided
    /// by two because its array is `uint16_t` — `CPS1_SCROLL1_SCROLLX = 0x0c / 2`.
    pub cps_a: [u16; CPS_REGS],
    /// CPS-B, 0x800140-0x80017F, indexed by word. Mostly RAM; see [`BoardConfig`]
    /// for the reads the board answers itself.
    pub cps_b: [u16; CPS_REGS],
    /// Controls and DIP switches. The frontend writes this between frames.
    pub inputs: Inputs,
    /// Sound command and fade latches, 0x800180 and 0x800188. Sub-project D reads
    /// them; here they only record what the 68000 wrote.
    pub sound_latch: [u8; 2],
    /// Coin counters and lockouts, 0x800030-0x800037. Recorded, not acted on.
    pub coin_ctrl: u16,
    /// Which CPS-B reads this board answers itself.
    pub cfg: BoardConfig,
    /// What the board saw. Sub-project B's whole observable surface.
    ///
    /// Counted here rather than in [`Cps1`](crate::Cps1) because the board is what
    /// decodes: only this file knows whether 0x810000 is a chip or a hole. Not
    /// cleared by [`Cps1::reset`](crate::Cps1::reset) — see [`Trace`].
    pub trace: Trace,
    /// Set while IPL1 is asserted and the 68000 has not yet fetched its vector.
    ///
    /// # Why there is no public deassertion API
    ///
    /// On hardware the line is cleared by the CPU's own autovector fetch: the 68000
    /// drives FC=7 with an address in 0xFFFFF2-0xFFFFFF and the board decodes that
    /// to drop both IPL1 and IPL2 (`irqack_r`, `cps1.cpp:407-422`, wired through
    /// `cpu_space_map` at `:419-422`). [`m68k::Bus`] carries no function code, so
    /// that cycle is invisible here — an autovector fetch of vector 26 and a
    /// `move.l $68,d0` are the same two `read16` calls.
    ///
    /// So the acknowledge is detected as a **read of the vector-26 longword** at
    /// 0x68/0x6A while an assertion is outstanding. Autovector level 2 is vector
    /// 24 + 2 = 26, at 26 × 4 = 0x68. On this board that inference is exact: the
    /// vector table is in ROM and no game reads its own vector 26 as data. If one
    /// did, the read would return the same value either way — only the deassertion
    /// would be early.
    ///
    /// The alternative considered and rejected was deasserting a scanline later,
    /// which is wrong in a way that hides: a handler slower than a line misses the
    /// next assertion, a faster one takes the same interrupt twice. Widening `Bus`
    /// with a function code is the correct fix and is deferred — it would break the
    /// trait that 317,500 verified vector cases run through, for one bit that one
    /// board needs.
    vblank_pending: bool,
}

/// Which halves of a word a bus cycle asserts.
///
/// The 68000 has no byte-wide bus: it drives UDS and LDS to select halves of a
/// 16-bit access. MAME passes the same information as `mem_mask`, and its I/O
/// handlers branch on it (`ACCESSING_BITS_0_7` at `cps1.cpp:300-313`), so a board
/// that models byte writes as read-modify-write gets write-only ports wrong — the
/// read half returns 0xFFFF from a port that does not read, and the "preserved"
/// neighbouring byte becomes 0xFF.
///
/// Shared with [`crate::sf1::board`]: the lane model is the 68000's bus, not
/// CPS-1's, and both boards branch on it for exactly the same reason — a
/// write-only port has no old word to merge with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lanes {
    /// A full word: both UDS and LDS.
    Word,
    /// The high byte only, at an even address.
    High,
    /// The low byte only, at an odd address.
    Low,
}

impl Board {
    /// `prog` is the assembled 68000 program region, big-endian, up to the 4 MB
    /// of ROM space (`CODE_SIZE`). Longer input is truncated; shorter is
    /// zero-padded, which is what an unpopulated socket reads as.
    ///
    /// Takes `&[u8]` and **not** a `romset::RomSet`: `machine` does not depend on
    /// `romset`, so this crate stays at one dependency and keeps working without
    /// `std`. Every test in this crate builds its program inline.
    pub fn new(prog: &[u8], cfg: BoardConfig) -> Self {
        let mut rom = vec![0u8; ROM_BYTES];
        let n = prog.len().min(ROM_BYTES);
        rom[..n].copy_from_slice(&prog[..n]);
        Self {
            rom,
            ram: Box::new([0u16; RAM_WORDS]),
            gfxram: Box::new([0u16; GFXRAM_WORDS]),
            cps_a: [0; CPS_REGS],
            cps_b: [0; CPS_REGS],
            inputs: Inputs::idle(),
            sound_latch: [0; 2],
            coin_ctrl: 0,
            cfg,
            trace: Trace::default(),
            vblank_pending: false,
        }
    }

    /// Asserts IPL1, as the beam reaching line 240 does (`cps1.cpp:394-396`).
    pub fn assert_vblank(&mut self) {
        self.vblank_pending = true;
        self.trace.vblanks += 1;
    }

    /// Whether IPL1 is still asserted — i.e. the 68000 has not yet acknowledged.
    ///
    /// See [`Board::assert_vblank`] and the `vblank_pending` field for how the
    /// acknowledge is detected.
    pub fn vblank_pending(&self) -> bool {
        self.vblank_pending
    }

    /// Sets the pending-interrupt line directly, for a save-state restore.
    ///
    /// ⚠️ **Not for the scheduler.** [`Board::assert_vblank`] is what a beam
    /// reaching line 240 calls, and it also counts the vblank in the trace. This
    /// sets the line without counting anything, which is right for a restore — the
    /// vblank being restored was counted when it originally happened — and wrong
    /// for everything else.
    pub fn set_vblank_pending(&mut self, pending: bool) {
        self.vblank_pending = pending;
    }

    /// The 68000's autovector-26 fetch, which on this board is the acknowledge
    /// cycle.
    ///
    /// Split out of [`Board::read_word`]'s ROM arm so the reasoning lives next to
    /// the address test rather than inside a hot path.
    #[inline]
    fn note_possible_ack(&mut self, addr: u32) {
        // `& !3` because the vector is a longword: a 68000 with a 16-bit bus fetches
        // it as two `read16` calls, 0x68 then 0x6A, and either half is the same
        // acknowledge cycle.
        //
        // ⚠️ **Arithmetically dead today, and deliberately kept.** `m68k`'s
        // `exception::take` reads the high half first (`exception.rs:371-372`), so
        // 0x68 always arrives before 0x6A and `addr == VEC_AUTOVECTOR_2` would behave
        // identically. Mutation confirmed: no test in this crate can kill dropping
        // the mask, and none was contorted to try. It stays because it encodes "the
        // acknowledge is the *longword* fetch", which is the hardware fact, and
        // because the equivalence rests on one core's fetch order — a core that read
        // the low half first, or a future `read32` fast path, would make it
        // load-bearing with no test signalling that it had become so.
        if self.vblank_pending && (addr & !3) == VEC_AUTOVECTOR_2 {
            self.vblank_pending = false;
            self.trace.acks += 1;
        }
    }

    #[inline]
    fn ram_index(addr: u32) -> usize {
        ((addr >> 1) as usize) & (RAM_WORDS - 1)
    }

    #[inline]
    fn gfx_index(addr: u32) -> usize {
        // 0x18000 is not a power of two, so this is a remainder rather than a
        // mask. `%` on a usize cannot panic for a nonzero divisor, and
        // `wrapping_sub` keeps it defined for a caller that reaches here with an
        // address below the base — which the match arms never do, but a future
        // caller might.
        //
        // The subtraction happens to be arithmetically dead today: 0x900000 >> 1
        // is 0x480000, exactly 48 × 0x18000, so the remainder is the same with or
        // without it. Mutation confirmed no test can kill its removal. It stays
        // because it is what makes the expression mean "an offset into gfxram",
        // and because that coincidence holds only for this base and this size —
        // a later change to either would make it load-bearing with no test
        // signalling that it had become so.
        ((addr.wrapping_sub(GFXRAM_BASE) >> 1) as usize) % GFXRAM_WORDS
    }

    /// The multiply protection's answer for CPS-B byte offset `off`, if `off` is one
    /// of the two result registers on this board.
    ///
    /// `cps1_v.cpp:2145-2152`. The board holds a 16×16→32 multiplier: the program
    /// writes two factors to two registers and reads the product back from two
    /// others. A bootleg's discrete logic cannot do it, which is the point.
    ///
    /// The product is computed **on each read**, from whatever the factor registers
    /// currently hold, rather than latched when a factor is written. That is MAME's
    /// structure and it is observable: a program that writes one factor, reads the
    /// result, writes the other factor and reads again gets two different answers
    /// from one write each. Latching on write would make the second read stale.
    ///
    /// `u32::from` on both factors before multiplying. `u16 * u16` in Rust is a `u16`
    /// multiply that **panics on overflow in debug builds** and wraps in release — and
    /// `result_hi` is precisely the half that a `u16` multiply throws away, so a
    /// 32-bit product is not an optimisation here but the whole feature. The
    /// widening also means this function cannot panic on any guest input, which the
    /// bus contract requires.
    ///
    /// `None` on a board with no `multiply` row, which is every board but Champion
    /// Edition's so far; the caller then treats the offset as an ordinary register.
    fn multiply_at(&self, off: u8) -> Option<u16> {
        let p = self.cfg.multiply?;
        let factors = || {
            let a = u32::from(self.cps_b[usize::from(p.factor1 >> 1) & (CPS_REGS - 1)]);
            let b = u32::from(self.cps_b[usize::from(p.factor2 >> 1) & (CPS_REGS - 1)]);
            a * b
        };
        if off == p.result_lo {
            // `& 0xffff`, spelled as the truncating cast it is.
            Some(factors() as u16)
        } else if off == p.result_hi {
            Some((factors() >> 16) as u16)
        } else {
            None
        }
    }

    /// The word at `addr`, or `None` if `addr` is in no mapped range.
    ///
    /// `&mut self` because [`Bus::read16`] is, and because a later CPS-1 read
    /// handler may mutate — MAME's own `cps1_cps_b_r` is non-const for the raster
    /// counters. Splitting mapped from unmapped in this function rather than in
    /// [`Bus::read16`] is what lets Task 9's trace name the unmapped access.
    ///
    /// The address map itself lives in [`Board::peek_word`]. This is the map **plus
    /// the CPU's own bookkeeping**: the acknowledge cycle, and the trace's record of
    /// a read no chip answered. Both are why a debugger must not read through here.
    pub(crate) fn read_word(&mut self, addr: u32) -> Option<u16> {
        // Unconditional rather than inside a ROM-range arm, as it was when this was
        // one function: the address it tests for is 0x68, which is in ROM space, so
        // the range guard could only ever be redundant with the test inside.
        self.note_possible_ack(addr);
        let v = self.peek_word(addr);
        if v.is_none() {
            self.trace.unmapped_reads.record(addr);
        }
        v
    }

    /// The word at `addr`, or `None` if `addr` is in no mapped range — **with no
    /// side effects.**
    ///
    /// For a debugger, a memory viewer, or anything else that reads the machine
    /// without being part of it. `read_word` is what the CPU uses and it is
    /// not side-effect-free: it acknowledges a pending interrupt on a read of the
    /// vector-26 longword, and records unmapped addresses in the trace. A memory
    /// panel built on it would clear the interrupt it was opened to investigate, and
    /// one parked on unmapped space would fill the counter it displays with its own
    /// reads.
    ///
    /// `&self` is the enforcement, not a preference: a `&mut self` version could
    /// acknowledge the interrupt and the compiler would not object.
    ///
    /// (`read_word` and `note_possible_ack` are named in plain code spans rather than
    /// rustdoc links: both are private, and rustdoc rejects a public item linking to
    /// a private one.)
    ///
    /// This holds the whole address map and `read_word` delegates to it, so the two
    /// **cannot** disagree. That was not a given — the design for this expected to
    /// duplicate the map and hold the copies together with an agreement test, on the
    /// grounds that the I/O ranges compute rather than store. They do compute, but
    /// every one of those computations reads only [`Board::inputs`] and
    /// [`Board::cfg`], so all of it is `&self`-safe and there is one map after all.
    pub fn peek_word(&self, addr: u32) -> Option<u16> {
        match addr {
            0x00_0000..=0x3F_FFFF => {
                let i = (addr & !1) as usize;
                Some(u16::from_be_bytes([self.rom[i], self.rom[i + 1]]))
            }
            // ---- The I/O block. Ranges and handlers from `cps1.cpp:577-594`. ----
            //
            // Only the ranges MAME gives a *read* handler appear here. CPS-A, the
            // coin control, and both sound latches are `.w(...)` — write-only — so
            // a read of them decodes nothing and floats high, which is what
            // returning `None` here produces one layer up.
            0x80_0000..=0x80_0007 => Some(self.inputs.in1()),
            0x80_0018..=0x80_001F => {
                // `cps1_dsw_r`, `cps1.cpp:257-272`: four word offsets select IN0,
                // DSWA, DSWB, DSWC, and the byte lands in the **high** half with
                // 0xFF below it.
                //
                // Not `cps1_hack_dsw_r` (`cps1.cpp:274`), which is the same
                // function with `| in` instead of `| 0xff`. The two are adjacent in
                // the file and differ in one token; sf2 gets `main_map` from
                // `cps1_10MHz` (`cps1.cpp:3909`, `GAME(1991, sf2, …)` at 15024),
                // and `main_map` wires `cps1_dsw_r`.
                let sel = ((addr - 0x80_0018) >> 1) & 3;
                let byte = match sel {
                    0 => self.inputs.in0(),
                    n => self.inputs.dsw[(n - 1) as usize],
                };
                Some((u16::from(byte) << 8) | 0x00FF)
            }
            // `nopr()`, `cps1.cpp:583`: decoded as a read that returns nothing in
            // particular. MAME's `nopr` yields the unmapped value; ours is 0xFFFF
            // for the same reason the default is.
            0x80_0020..=0x80_0021 => Some(UNMAPPED),
            0x80_0140..=0x80_017F => {
                let off = ((addr - CPS_B_BASE) as u8) & !1;
                if self.cfg.cpsb_addr == Some(off) {
                    // The boot self-test. `cps1_v.cpp:2139-2140`.
                    Some(self.cfg.cpsb_value)
                } else if let Some(p) = self.multiply_at(off) {
                    // The multiply protection. `cps1_v.cpp:2145-2152`, tested
                    // before IN2 there and here for the same reason: the arms are
                    // an if-chain, so the order is the precedence, and a board
                    // whose table put a factor port at `in2_addr` would answer
                    // with whichever came first. No row in this workspace does —
                    // `config.rs` asserts it for `sf2ce` — but the order is
                    // MAME's, not ours to pick.
                    Some(p)
                } else if self.cfg.in2_addr == Some(off) {
                    // SF2's six kick buttons, on the C-board. `cps1_v.cpp:2155-2156`.
                    // An 8-bit port read into a 16-bit space: 0x00 above the byte.
                    Some(u16::from(self.inputs.in2()))
                } else {
                    Some(self.cps_b[(off >> 1) as usize])
                }
            }
            0x90_0000..=0x92_FFFF => Some(self.gfxram[Self::gfx_index(addr)]),
            0xFF_0000..=0xFF_FFFF => Some(self.ram[Self::ram_index(addr)]),
            _ => None,
        }
    }

    /// Writes the word at `addr`; false if `addr` is in no writable range.
    pub(crate) fn write_word(&mut self, addr: u32, val: u16) -> bool {
        self.write_lanes(addr, val, Lanes::Word)
    }

    /// Writes `lanes` of the word at `addr`; false if `addr` is in no writable
    /// range.
    ///
    /// `val` is positioned as the 68000 drives it: for [`Lanes::High`] the byte
    /// sits in bits 15-8 and bits 7-0 are ignored, for [`Lanes::Low`] the reverse.
    /// This is exactly MAME's `(data, mem_mask)` pair, and it is why this takes
    /// lanes at all rather than doing a read-modify-write in [`Bus::write8`]: the
    /// sound latch and the coin control do not read back, so there is no old word
    /// to merge with, and `cps1_coinctrl_w` ignores a low-half access entirely.
    pub(crate) fn write_lanes(&mut self, addr: u32, val: u16, lanes: Lanes) -> bool {
        match addr {
            // ROM: the write reaches no chip that latches it. Discarded and
            // reported as handled — guest behaviour, not our bug, and not an
            // unmapped access either. A real board decodes this range.
            0x00_0000..=0x3F_FFFF => {
                self.trace.rom_writes += 1;
                true
            }
            // `cps1_coinctrl_w`, `cps1.cpp:316-327`. Every bit it uses is in the
            // high half and the handler is wrapped in `ACCESSING_BITS_8_15`, so a
            // low-byte-only write is decoded and then does nothing.
            0x80_0030..=0x80_0037 => {
                if lanes != Lanes::Low {
                    self.coin_ctrl = lanes.merge(self.coin_ctrl, val);
                }
                true
            }
            // `cps1_cps_a_w` / `cps1_cps_b_w` (`cps1_v.cpp:2115`, `:2183`) both
            // begin with `COMBINE_DATA`, so a byte write merges into the register
            // and leaves its other half alone.
            0x80_0100..=0x80_013F => {
                let i = (((addr - CPS_A_BASE) >> 1) as usize) & (CPS_REGS - 1);
                self.cps_a[i] = lanes.merge(self.cps_a[i], val);
                self.trace.cps_a_writes += 1;
                true
            }
            0x80_0140..=0x80_017F => {
                // The write lands even at `cpsb_addr`: the ID register is
                // readable-as-wired, not write-protected. MAME's `COMBINE_DATA`
                // runs before `cps1_cps_b_r` ever intercepts the read.
                let i = (((addr - CPS_B_BASE) >> 1) as usize) & (CPS_REGS - 1);
                self.cps_b[i] = lanes.merge(self.cps_b[i], val);
                self.trace.cps_b_writes += 1;
                true
            }
            // `cps1_soundlatch_w`, `cps1.cpp:300-306`: the low byte when the low
            // lane is asserted, otherwise the high byte. A word write asserts both,
            // so `ACCESSING_BITS_0_7` holds and the low byte wins.
            0x80_0180..=0x80_0187 => {
                self.sound_latch[0] = lanes.byte_written(val);
                self.trace.sound_latch_writes += 1;
                true
            }
            // `cps1_soundlatch2_w`, `cps1.cpp:308-312`: the low-lane branch only.
            // A high-byte-only write is decoded and discarded.
            0x80_0188..=0x80_018F => {
                if lanes != Lanes::High {
                    self.sound_latch[1] = val as u8;
                }
                // Counted even when the lane check discards the byte: the counter
                // records that the 68000 *addressed* the sound block, which is the
                // progress signal, and a write MAME decodes and drops is not an
                // unmapped access.
                self.trace.sound_latch_writes += 1;
                true
            }
            0x90_0000..=0x92_FFFF => {
                let i = Self::gfx_index(addr);
                self.gfxram[i] = lanes.merge(self.gfxram[i], val);
                self.trace.gfxram_writes += 1;
                true
            }
            0xFF_0000..=0xFF_FFFF => {
                let i = Self::ram_index(addr);
                self.ram[i] = lanes.merge(self.ram[i], val);
                true
            }
            _ => {
                self.trace.unmapped_writes.record(addr);
                false
            }
        }
    }
}

impl Lanes {
    /// The lanes a byte access at `addr` asserts.
    #[inline]
    pub(crate) fn of_byte(addr: u32) -> Self {
        if addr & 1 == 0 {
            Self::High
        } else {
            Self::Low
        }
    }

    /// `val` positioned on these lanes, as the 68000 drives it.
    ///
    /// The unasserted half is left zero. Nothing may read it — every consumer here
    /// is gated on the lanes — and zero rather than a duplicate of the byte is what
    /// makes a consumer that *isn't* gated fail a test instead of coincidentally
    /// working.
    #[inline]
    pub(crate) fn place(self, val: u8) -> u16 {
        match self {
            Self::Word | Self::Low => u16::from(val),
            Self::High => u16::from(val) << 8,
        }
    }

    /// `old` with the asserted lanes of `new` written over it — MAME's
    /// `COMBINE_DATA`.
    #[inline]
    pub(crate) fn merge(self, old: u16, new: u16) -> u16 {
        match self {
            Self::Word => new,
            Self::High => (new & 0xFF00) | (old & 0x00FF),
            Self::Low => (old & 0xFF00) | (new & 0x00FF),
        }
    }

    /// The byte an 8-bit write-only port latches from a `val` on these lanes.
    ///
    /// MAME's idiom, verbatim: `if (ACCESSING_BITS_0_7) write(data & 0xff); else
    /// write(data >> 8);`. A full word therefore latches the **low** byte —
    /// counter-intuitive for a big-endian CPU, and what the hardware does.
    #[inline]
    pub(crate) fn byte_written(self, val: u16) -> u8 {
        match self {
            Self::Word | Self::Low => val as u8,
            Self::High => (val >> 8) as u8,
        }
    }
}

impl Bus for Board {
    fn read16(&mut self, addr: u32) -> u16 {
        // Addresses arrive already masked to 24 bits by the core, but mask again:
        // this is also called directly by tests and by the frontend.
        let addr = addr & 0x00FF_FFFF;
        self.read_word(addr).unwrap_or(UNMAPPED)
    }

    fn read8(&mut self, addr: u32) -> u8 {
        let w = self.read16(addr & !1);
        if addr & 1 == 0 {
            (w >> 8) as u8
        } else {
            w as u8
        }
    }

    fn write16(&mut self, addr: u32, val: u16) {
        let addr = addr & 0x00FF_FFFF;
        let _ = self.write_word(addr, val);
    }

    fn write8(&mut self, addr: u32, val: u8) {
        // A byte write asserts one strobe, and the board is told which. It is
        // **not** a read-modify-write of the containing word: for a write-only
        // port there is nothing to read back, so `read16` would return 0xFFFF and
        // the "preserved" neighbouring byte would be latched as 0xFF. The RAM-like
        // ranges get their read-modify-write from `Lanes::merge`, at the arm that
        // owns the storage.
        let addr = addr & 0x00FF_FFFF;
        let lanes = Lanes::of_byte(addr);
        let _ = self.write_lanes(addr & !1, lanes.place(val), lanes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use m68k::Bus;

    fn board() -> Board {
        Board::new(&[], BoardConfig::sf2())
    }

    #[test]
    fn ram_stores_words_and_bytes_big_endian() {
        let mut b = board();
        b.write16(0xFF_0000, 0x1234);
        assert_eq!(b.read16(0xFF_0000), 0x1234);
        assert_eq!(b.read8(0xFF_0000), 0x12, "high byte at the even address");
        assert_eq!(b.read8(0xFF_0001), 0x34, "low byte at the odd address");
        b.write8(0xFF_0001, 0xAB);
        assert_eq!(
            b.read16(0xFF_0000),
            0x12AB,
            "a byte write must not disturb its neighbour"
        );
        b.write8(0xFF_0000, 0xCD);
        assert_eq!(
            b.read16(0xFF_0000),
            0xCDAB,
            "and the same in the other half"
        );
    }

    #[test]
    fn ram_is_64k_and_wraps_within_its_window() {
        // 0xFF0000-0xFFFFFF is 64 KB, and the 68000's address bus is 24 bits, so
        // there is nothing above it to alias into.
        let mut b = board();
        b.write16(0xFF_FFFE, 0xBEEF);
        assert_eq!(b.read16(0xFF_FFFE), 0xBEEF);
        assert_eq!(
            b.read16(0xFF_0000),
            0x0000,
            "the top word is not the bottom word"
        );
    }

    #[test]
    fn gfxram_is_192k_and_distinct_from_main_ram() {
        let mut b = board();
        b.write16(0x90_0000, 0xAAAA);
        b.write16(0x92_FFFE, 0x5555);
        assert_eq!(b.read16(0x90_0000), 0xAAAA);
        assert_eq!(b.read16(0x92_FFFE), 0x5555);
        assert_eq!(
            b.read16(0xFF_0000),
            0x0000,
            "gfxram must not alias main RAM"
        );
    }

    /// Every mapped gfxram address gets its own storage slot.
    ///
    /// This checks `gfx_index` directly rather than through `write16`, because a
    /// value-pattern sweep cannot do the job: 0x18000 words do not fit in the
    /// 0x10000 distinct values a `u16` holds, so *no* pattern is injective over
    /// the region, and a sweep that reuses a value can be aliased without
    /// noticing. My first attempt at this test did exactly that, and its own
    /// injectivity check is what caught it.
    ///
    /// Both surviving mutants — masking with 0xFFFF, and a `GFXRAM_WORDS` of
    /// 0x10000 — collapse two addresses onto one slot, which is precisely what a
    /// bijection test rejects.
    #[test]
    fn gfx_index_maps_the_mapped_range_one_to_one_onto_the_array() {
        let mut hit = vec![false; GFXRAM_WORDS];
        for i in 0..GFXRAM_WORDS {
            let addr = GFXRAM_BASE + (i as u32) * 2;
            let slot = Board::gfx_index(addr);
            assert!(
                !hit[slot],
                "{addr:#08x} aliases an earlier address onto slot {slot}"
            );
            hit[slot] = true;
        }
        assert!(hit.iter().all(|&h| h), "some slot is unreachable");
    }

    /// And the same property observed through the bus, at the addresses where a
    /// 64 K alias would show.
    ///
    /// `gfx_index_maps_the_mapped_range_one_to_one_onto_the_array` reads the index
    /// function; this reads the artifact. A pair 0x10000 words apart is what a
    /// 0xFFFF mask collapses, and 0x8000 words apart is what a smaller region
    /// would.
    #[test]
    fn words_that_a_wrong_mask_would_alias_stay_independent() {
        let mut b = board();
        for stride in [0x8000u32, 0x1_0000] {
            for i in [0u32, 1, 0x1234, 0x7FFF] {
                let lo = GFXRAM_BASE + i * 2;
                let hi = GFXRAM_BASE + (i + stride) * 2;
                b.write16(lo, 0xA1A1);
                b.write16(hi, 0xB2B2);
                assert_eq!(
                    b.read16(lo),
                    0xA1A1,
                    "words {i} and {} (stride {stride:#x}) share a slot",
                    i + stride
                );
                assert_eq!(b.read16(hi), 0xB2B2);
            }
        }
    }

    /// gfxram is 96 K words, pinned as a literal.
    ///
    /// `every_word_of_gfxram_is_independently_addressable` iterates
    /// `0..GFXRAM_WORDS`, so a shrunken constant would shrink the sweep with it
    /// and stay green — a self-consistent test. This is the literal that stops
    /// that, and 0x92FFFE below is the address the region's size implies.
    #[test]
    fn gfxram_holds_exactly_96k_words() {
        assert_eq!(GFXRAM_WORDS, 0x1_8000, "192 KB at cps1.cpp:592");
        let b = board();
        assert_eq!(b.gfxram.len(), 0x1_8000);
        assert_eq!(
            GFXRAM_BASE + (GFXRAM_WORDS as u32) * 2 - 2,
            0x92_FFFE,
            "the last word of the mapped range"
        );
    }

    #[test]
    fn rom_reads_the_program_and_ignores_writes() {
        let mut b = Board::new(&[0x12, 0x34, 0x56, 0x78], BoardConfig::sf2());
        assert_eq!(b.read16(0x00_0000), 0x1234);
        assert_eq!(b.read16(0x00_0002), 0x5678);
        b.write16(0x00_0000, 0xFFFF);
        assert_eq!(
            b.read16(0x00_0000),
            0x1234,
            "ROM is read-only; the write is discarded"
        );
        b.write8(0x00_0001, 0xFF);
        assert_eq!(b.read16(0x00_0000), 0x1234, "and a byte write too");
    }

    #[test]
    fn rom_beyond_the_program_reads_zero_not_out_of_bounds() {
        let mut b = Board::new(&[0x12, 0x34], BoardConfig::sf2());
        assert_eq!(b.read16(0x3F_FFFE), 0x0000, "unpopulated ROM space");
        assert_eq!(b.read16(0x00_0002), 0x0000, "just past the program");
    }

    /// An odd ROM address reads the word containing it.
    ///
    /// Found by mutation: dropping `& !1` from the ROM arm's index survived every
    /// other test, because none of them read an odd ROM address. The consequence
    /// is not cosmetic — an unmasked index at 0x3FFFFF reads `rom[0x400000]`,
    /// one past a 4 MB `Vec`, which is a host panic on a guest address.
    #[test]
    fn an_odd_rom_address_reads_the_containing_word_and_the_last_byte_does_not_panic() {
        let mut b = Board::new(&[0x12, 0x34, 0x56, 0x78], BoardConfig::sf2());
        assert_eq!(b.read16(0x00_0001), 0x1234, "not 0x3456");
        assert_eq!(b.read8(0x00_0003), 0x78);
        assert_eq!(
            b.read16(0x3F_FFFF),
            0x0000,
            "the last odd address in ROM space must not index past the Vec"
        );
    }

    #[test]
    fn a_program_longer_than_the_rom_space_is_truncated_not_a_panic() {
        // A wrong spec table, or a future set with a larger region, must not
        // panic the loader. Truncation is the documented behaviour.
        let mut prog = vec![0u8; ROM_BYTES + 4];
        prog[ROM_BYTES - 2] = 0xC0;
        prog[ROM_BYTES - 1] = 0xDE;
        let mut b = Board::new(&prog, BoardConfig::sf2());
        assert_eq!(b.rom.len(), ROM_BYTES);
        assert_eq!(
            b.read16(0x3F_FFFE),
            0xC0DE,
            "the last in-range word survives"
        );
    }

    /// An unmapped read returns all ones, not zero.
    ///
    /// 0x0000 is a legal opcode (`ori.b #imm, d0`), so a runaway PC in unmapped
    /// space would execute quietly for thousands of instructions before anything
    /// looked wrong. 0xFFFF is illegal, which is both what the floating bus
    /// actually reads as and the failure that surfaces immediately.
    #[test]
    fn unmapped_space_reads_all_ones() {
        let mut b = board();
        assert_eq!(b.read16(0x40_0000), 0xFFFF, "just above ROM");
        assert_eq!(b.read16(0x81_0000), 0xFFFF, "just above the I/O block");
        assert_eq!(b.read16(0x93_0000), 0xFFFF, "just above gfxram");
        assert_eq!(b.read16(0xFE_FFFE), 0xFFFF, "just below main RAM");
        assert_eq!(b.read8(0x40_0000), 0xFF, "and a byte read of the same");
        assert_eq!(b.read8(0x40_0001), 0xFF);
    }

    /// `write_word` reports whether the board decoded the address.
    ///
    /// Found by mutation: flipping the unmapped arm to `true` survived the whole
    /// suite, because every test checked only that the write changed nothing —
    /// which a discarded-but-claimed-handled write also satisfies. This bool is
    /// Task 9's trace signal for "the guest wrote somewhere no chip answers", the
    /// single most useful line of output when a driver is wrong about the map, and
    /// an arm that always returns true silently empties that report.
    ///
    /// ROM is deliberately `true`: a real board decodes 0x000000-0x3FFFFF, so a
    /// write there is a guest bug worth no trace line, not an unmapped access.
    #[test]
    fn write_word_reports_unmapped_addresses_as_undecoded() {
        let mut b = board();
        assert!(b.write_word(0xFF_0000, 0x1234), "main RAM");
        assert!(b.write_word(0x90_0000, 0x1234), "gfxram");
        assert!(
            b.write_word(0x00_0000, 0x1234),
            "ROM is decoded, just read-only"
        );
        assert!(!b.write_word(0x40_0000, 0x1234), "just above ROM");
        assert!(
            !b.write_word(0x80_0000, 0x1234),
            "IN1 is read-only: cps1.cpp:580 gives it no write handler"
        );
        assert!(!b.write_word(0x81_0000, 0x1234), "just above the I/O block");
        assert!(!b.write_word(0x93_0000, 0x1234), "just above gfxram");
        assert!(!b.write_word(0xFE_FFFE, 0x1234), "just below main RAM");
    }

    /// And the same for reads: `read_word` returns `None` exactly where the board
    /// decodes nothing, which is what `read16` turns into 0xFFFF one layer up.
    #[test]
    fn read_word_reports_unmapped_addresses_as_none() {
        let mut b = board();
        assert!(b.read_word(0xFF_0000).is_some());
        assert!(b.read_word(0x90_0000).is_some());
        assert!(b.read_word(0x00_0000).is_some());
        assert!(b.read_word(0x40_0000).is_none());
        assert!(b.read_word(0x80_0000).is_some(), "IN1");
        assert!(
            b.read_word(0x80_0100).is_none(),
            "CPS-A is write-only: cps1.cpp:586 gives it no read handler"
        );
        assert!(b.read_word(0x81_0000).is_none());
        assert!(b.read_word(0x93_0000).is_none());
        assert!(b.read_word(0xFE_FFFE).is_none());
    }

    /// Peeking never disturbs the machine.
    ///
    /// [`Board::read_word`] acknowledges a pending interrupt on a read of the
    /// vector-26 longword and records unmapped reads in the trace. A debugger's
    /// memory panel scrolled over 0x68 must not acknowledge the interrupt it was
    /// opened to investigate, and one parked on unmapped space must not fill the
    /// counter it displays with its own reads.
    ///
    /// Swept over the whole vector table *and* unmapped space, with an interrupt
    /// outstanding — the state in which every one of those side effects is live.
    #[test]
    fn peeking_does_not_disturb_the_machine() {
        let mut b = board();
        b.assert_vblank();
        assert!(
            b.vblank_pending(),
            "the premise: an interrupt is outstanding"
        );
        let acks = b.trace.acks;
        let unmapped = b.trace.unmapped_reads.total();

        for addr in (0..0x400).step_by(2) {
            b.peek_word(addr);
        }
        for addr in (0x40_0000..0x40_0100).step_by(2) {
            b.peek_word(addr);
        }

        assert!(
            b.vblank_pending(),
            "the interrupt must still be outstanding"
        );
        assert_eq!(b.trace.acks, acks, "no acknowledge was invented");
        assert_eq!(
            b.trace.unmapped_reads.total(),
            unmapped,
            "and the debugger's own reads are not in the counter"
        );

        // The control: the same reads through `read_word` *do* disturb it. Without
        // this the test passes for a machine that was never disturbable in the first
        // place — which is exactly what a future refactor moving the acknowledge
        // elsewhere would produce, silently.
        assert!(b.read_word(0x68).is_some());
        assert_eq!(
            b.trace.acks,
            acks + 1,
            "read_word acknowledges, which is why peek exists"
        );
        assert!(!b.vblank_pending(), "and drops the line");
        b.read_word(0x40_0000);
        assert!(
            b.trace.unmapped_reads.total() > unmapped,
            "and records an unmapped read"
        );
    }

    /// Peek and read agree everywhere.
    ///
    /// Otherwise the debugger shows a different machine than the one running.
    /// `read_word` delegates to `peek_word` today, so this is a *pin* rather than a
    /// discovery: it fails if the two are ever split into separate maps that drift.
    /// Walked over both sides of every boundary in the map, in a state where
    /// `read_word`'s side effects are inert — no interrupt pending — so the
    /// comparison is legal.
    #[test]
    fn peek_and_read_agree_across_the_address_map() {
        let mut b = board();
        assert!(!b.vblank_pending(), "the premise: no side effect is live");
        let edges = [
            0x00_0000u32,
            0x3F_FFFE,
            0x40_0000,
            0x7F_FFFE,
            0x80_0000,
            0x80_0006,
            0x80_0008,
            0x80_0016,
            0x80_0018,
            0x80_001E,
            0x80_0020,
            0x80_0022,
            0x80_0100,
            0x80_013E,
            0x80_0140,
            0x80_017E,
            0x80_0180,
            0x8F_FFFE,
            0x90_0000,
            0x92_FFFE,
            0x93_0000,
            0xFE_FFFE,
            0xFF_0000,
            0xFF_FFFE,
        ];
        for addr in edges {
            let peeked = b.peek_word(addr);
            assert_eq!(
                peeked,
                b.read_word(addr),
                "peek and read disagree at {addr:#08X}"
            );
        }
        // The premise: that loop compared something. Without these it passes for a
        // `peek_word` that returns `None` everywhere and a `read_word` that agrees.
        assert_eq!(b.peek_word(0xFF_0000), Some(0), "RAM decodes");
        assert_eq!(b.peek_word(0x40_0000), None, "and a gap does not");
        assert!(
            b.peek_word(0x80_0018).is_some(),
            "and so does a computed I/O range, which is the interesting case"
        );
    }

    /// Undecoded is `None`, not `0xFFFF`.
    ///
    /// "Nothing decodes here" and "this decodes and reads as all ones" are different
    /// facts, and a debugger that renders both as `FFFF` sends you looking in the
    /// wrong place. `0x800020` is the one address that genuinely reads all ones while
    /// being decoded — MAME's `nopr` — which is what makes the distinction testable
    /// rather than theoretical.
    #[test]
    fn undecoded_and_all_ones_are_different_answers() {
        let b = board();
        assert_eq!(b.peek_word(0x40_0000), None, "a gap decodes nothing");
        assert_eq!(
            b.peek_word(0x80_0020),
            Some(0xFFFF),
            "nopr decodes, to all ones"
        );
    }

    /// Each write counts against its own region's counter and no other's.
    ///
    /// The zeroes are the assertions that matter. A counter incremented in the
    /// wrong arm still produces a plausible-looking report — a boot that touched
    /// only main RAM would appear to have programmed the video hardware, which is
    /// the one thing this trace exists to tell you it did *not* do. Found by
    /// mutation: adding `gfxram_writes += 1` to the main-RAM arm survived every
    /// other test in the crate, because they all check a counter's own region and
    /// none checks that the neighbours stayed at zero.
    #[test]
    fn a_write_counts_against_its_own_region_and_no_other() {
        let mut b = board();
        b.write16(0xFF_0000, 0x1234);
        b.write16(0xFF_0002, 0x5678);
        assert_eq!(b.trace.gfxram_writes, 0, "main RAM is not gfxram");
        assert_eq!(b.trace.cps_a_writes, 0);
        assert_eq!(b.trace.cps_b_writes, 0);
        assert_eq!(b.trace.sound_latch_writes, 0);
        assert_eq!(b.trace.rom_writes, 0);
        assert_eq!(b.trace.unmapped_writes.total(), 0, "main RAM is mapped");

        b.write16(0x90_0000, 0x1234);
        assert_eq!(b.trace.gfxram_writes, 1);
        assert_eq!(b.trace.cps_a_writes, 0, "gfxram is not a CPS-A register");

        b.write16(0x80_0100, 0x1234);
        assert_eq!(b.trace.cps_a_writes, 1);
        assert_eq!(b.trace.cps_b_writes, 0);
        assert_eq!(b.trace.gfxram_writes, 1, "and gfxram did not move");
    }

    /// The unmapped log names the address it was given, not a rounded one.
    ///
    /// [`Board::read_word`] and [`Board::write_lanes`] are the crate's own entry
    /// points and take the address verbatim: [`Bus::read8`]/[`Bus::write8`] align
    /// before calling them, but the debugger's memory panes in sub-project E will
    /// not, and a 68000 word access to an odd address is an address error the core
    /// reports rather than something this board ever sees. Found by mutation:
    /// masking the recorded address with `& !1` survived, and a report that folds
    /// 0x810001 into 0x810000 sends the reader looking at the wrong register.
    #[test]
    fn the_unmapped_log_records_the_exact_address_including_odd_ones() {
        let mut b = board();
        assert!(!b.write_word(0x81_0001, 0x1234));
        assert!(b.read_word(0x40_0003).is_none());
        assert_eq!(
            b.trace.unmapped_writes.entries(),
            &[(0x81_0001, 1)],
            "the odd address, not 0x810000"
        );
        assert_eq!(b.trace.unmapped_reads.entries(), &[(0x40_0003, 1)]);
    }

    #[test]
    fn a_write_to_unmapped_space_is_discarded_and_changes_nothing() {
        let mut b = board();
        b.write16(0xFF_0000, 0x1234);
        b.write16(0x40_0000, 0xDEAD);
        b.write8(0x93_0001, 0xBE);
        assert_eq!(b.read16(0xFF_0000), 0x1234, "RAM is untouched");
        assert_eq!(b.read16(0x90_0000), 0x0000, "gfxram is untouched");
    }

    /// Region boundaries are exact.
    ///
    /// Every off-by-one in a match arm shows up here and nowhere else: an
    /// inclusive bound written exclusive silently unmaps the last word of a
    /// region, which the CPS-1 uses (the stack lives at the top of main RAM).
    #[test]
    fn each_regions_first_and_last_word_is_mapped_and_its_neighbours_are_not() {
        let mut b = board();
        for (name, first, last) in [
            ("rom", 0x00_0000u32, 0x3F_FFFEu32),
            ("gfxram", 0x90_0000, 0x92_FFFE),
            ("ram", 0xFF_0000, 0xFF_FFFE),
        ] {
            assert!(
                b.read_word(first).is_some(),
                "{name}'s first word {first:#08x} must be mapped"
            );
            assert!(
                b.read_word(last).is_some(),
                "{name}'s last word {last:#08x} must be mapped"
            );
            if first >= 2 {
                assert!(
                    b.read_word(first - 2).is_none(),
                    "{name}'s first word has a mapped neighbour below it"
                );
            }
            // Main RAM's last word is the top of the 24-bit space; there is
            // nothing above it.
            if last < 0xFF_FFFE {
                assert!(
                    b.read_word(last + 2).is_none(),
                    "{name}'s last word has a mapped neighbour above it"
                );
            }
        }
    }

    /// A word access to an odd address is the 68000 core's business, not the
    /// board's.
    ///
    /// The core raises the address-error exception before the bus cycle happens,
    /// so `read16` here never sees an odd address from the CPU. The board still
    /// must not panic on one — the debugger and the frontend call `read16`
    /// directly — so it truncates to the containing word.
    #[test]
    fn an_odd_word_address_truncates_rather_than_panicking() {
        let mut b = board();
        b.write16(0xFF_0000, 0x1234);
        assert_eq!(b.read16(0xFF_0001), 0x1234);
        b.write16(0xFF_0001, 0x5678);
        assert_eq!(
            b.read16(0xFF_0000),
            0x5678,
            "the containing word was written"
        );
    }

    /// The invariant inherited verbatim from sub-project A: no guest address may
    /// panic. A mis-emulated jump produces exactly these accesses.
    #[test]
    fn no_address_in_the_whole_24_bit_space_panics() {
        let mut b = board();
        let mut addr = 0u32;
        while addr < 0x100_0000 {
            let _ = b.read16(addr);
            let _ = b.read8(addr);
            b.write16(addr, 0xDEAD);
            b.write8(addr, 0xBE);
            // Step by a prime so the sweep hits odd addresses and every region
            // boundary neighbourhood without taking 16M iterations.
            addr += 0x3B;
        }
    }

    /// And no address above 24 bits either.
    ///
    /// `Bus` takes a `u32`. The core masks to 24 bits, but a debugger or a
    /// frontend need not, and `rom[i + 1]` with an unmasked address would index
    /// past a 4 MB `Vec`.
    #[test]
    fn an_address_above_24_bits_is_masked_not_indexed() {
        let mut b = Board::new(&[0x12, 0x34], BoardConfig::sf2());
        assert_eq!(b.read16(0xFF00_0000), 0x1234, "wraps to 0x000000");
        assert_eq!(b.read16(0xFFFF_FFFE), b.read16(0xFF_FFFE), "wraps into RAM");
        b.write16(0x01FF_0000, 0x9999);
        assert_eq!(
            b.read16(0xFF_0000),
            0x9999,
            "and a write wraps the same way"
        );
    }

    // ---------------------------------------------------------------------------
    // The I/O block, 0x800000-0x80018F.
    // ---------------------------------------------------------------------------

    /// Active low, everywhere, at boot.
    ///
    /// This is the assertion a model that returns 0 for "nothing pressed" fails,
    /// and it is the difference between a game that boots and a game that thinks
    /// every button is held from the first frame.
    #[test]
    fn an_idle_board_reads_all_ones_across_the_port_block() {
        let mut b = board();
        assert_eq!(b.read16(0x80_0000), 0xFFFF, "IN1");
        assert_eq!(
            b.read16(0x80_0018),
            0xFFFF,
            "IN0 in the high byte, 0xFF in the low"
        );
        assert_eq!(b.read16(0x80_001A), 0xFFFF, "DSWA");
        assert_eq!(b.read16(0x80_001C), 0xFFFF, "DSWB");
        assert_eq!(b.read16(0x80_001E), 0xFFFF, "DSWC");
        assert_eq!(b.read16(0x80_0176), 0x00FF, "IN2 through CPS-B");
    }

    /// `cps1_dsw_r` puts the byte in the **high** half. `cps1.cpp:257-272`.
    ///
    /// A model that returns the byte in the low half passes every "is it 0xFF"
    /// check above and then fails every actual DIP-switch read. The literals here
    /// are asymmetric on purpose: 0x12FF and 0xFF12 are distinguishable, which
    /// 0xFFFF and 0xFFFF are not.
    #[test]
    fn dsw_reads_put_the_selected_bank_in_the_high_byte() {
        let mut b = board();
        b.inputs.dsw = [0x12, 0x34, 0x56];
        assert_eq!(b.read16(0x80_0018), 0xFFFF, "offset 0 is IN0, not DSWA");
        assert_eq!(b.read16(0x80_001A), 0x12FF, "DSWA");
        assert_eq!(b.read16(0x80_001C), 0x34FF, "DSWB");
        assert_eq!(b.read16(0x80_001E), 0x56FF, "DSWC");
    }

    /// Offset 0 of the DSW window is IN0, and it is not one of the DIP banks.
    ///
    /// `cps1_dsw_r`'s switch has four cases and the first is a different port. A
    /// model that computed `dsw[sel]` would shift all three banks down by one and
    /// read DSWA where the game expects the coin inputs — and with the banks all
    /// equal, as they are at boot, nothing would look wrong.
    #[test]
    fn the_dsw_window_selects_in0_then_the_three_dip_banks() {
        let mut b = board();
        b.inputs.dsw = [0x12, 0x34, 0x56];
        b.inputs.coin1 = true;
        assert_eq!(b.read16(0x80_0018), 0xFEFF, "IN0 with coin 1 in");
        assert_eq!(b.read16(0x80_001A), 0x12FF, "and DSWA is still DSWA");
    }

    /// `IN1` carries P1 in the low byte and P2 in the high. `cps1.cpp:840-856`.
    #[test]
    fn in1_carries_p1_in_the_low_byte_and_p2_in_the_high() {
        let mut b = board();
        b.inputs.p1.punch[0] = true; // bit 4
        b.inputs.p2.right = true; // bit 8
        assert_eq!(b.read16(0x80_0000), 0xFEEF);
    }

    /// 0x800000-0x800007 is one 16-bit port (`cps1.cpp:580`), so all four word
    /// addresses read the same value.
    #[test]
    fn in1_is_one_word_mirrored_across_its_eight_bytes() {
        let mut b = board();
        b.inputs.p1.up = true;
        let v = b.read16(0x80_0000);
        assert_eq!(v, 0xFFF7);
        for a in [0x80_0002u32, 0x80_0004, 0x80_0006] {
            assert_eq!(b.read16(a), v, "{a:#x} mirrors IN1");
        }
        assert_eq!(
            b.read16(0x80_0008),
            0xFFFF,
            "0x800008 is past the port; unmapped, which also reads 0xFFFF"
        );
        assert!(
            b.read_word(0x80_0008).is_none(),
            "and that 0xFFFF is the floating bus, not the port"
        );
    }

    /// The DSW window starts at 0x800018 and the gap below it is unmapped.
    ///
    /// `cps1.cpp:581` notes that a handful of games read 0x800010 as a
    /// development leftover; sf2's map decodes nothing there.
    #[test]
    fn the_gap_between_in1_and_the_dsw_window_is_unmapped() {
        let mut b = board();
        b.inputs.dsw = [0x12, 0x34, 0x56];
        assert!(b.read_word(0x80_0010).is_none());
        assert!(b.read_word(0x80_0016).is_none());
        assert!(b.read_word(0x80_0018).is_some(), "the window starts here");
        assert!(b.read_word(0x80_001E).is_some(), "and ends here");
        assert!(b.read_word(0x80_0022).is_none(), "past nopr()");
    }

    /// SF2's boot self-test: read 0x800140 + 0x32 and expect 0x0401.
    ///
    /// `cps1_v.cpp:491` (`CPS_B_11`) and `cps1_v.cpp:2139-2140`. The write must
    /// still land in the register file — MAME's `COMBINE_DATA` runs before the read
    /// interception — which is why this reads 0x0401 back *and*
    /// `a_write_to_the_cpsb_id_register_still_lands_in_the_file` sees 0xDEAD.
    #[test]
    fn the_cpsb_id_register_reads_its_wired_value_not_what_was_written() {
        let mut b = board();
        assert_eq!(b.read16(0x80_0172), 0x0401);
        b.write16(0x80_0172, 0xDEAD);
        assert_eq!(
            b.read16(0x80_0172),
            0x0401,
            "the ID register is wired, not RAM"
        );
    }

    /// The write is not swallowed, only the read is intercepted.
    ///
    /// `cps1_cps_b_w` (`cps1_v.cpp:2183-2185`) starts with `COMBINE_DATA`
    /// unconditionally. Sub-project C reads `cps_b` directly for the layer-enable
    /// and priority registers, so an arm that skipped the store at `cpsb_addr`
    /// would lose a write the video hardware needs — and no read through the bus
    /// could ever show it, because that read returns the wired value.
    #[test]
    fn a_write_to_the_cpsb_id_register_still_lands_in_the_file() {
        let mut b = board();
        b.write16(0x80_0172, 0xDEAD);
        assert_eq!(b.cps_b[0x32 / 2], 0xDEAD, "word index 0x19");
        assert_eq!(b.read16(0x80_0172), 0x0401, "but the read is still wired");
    }

    #[test]
    fn other_cps_b_registers_are_read_write() {
        let mut b = board();
        b.write16(0x80_0140, 0x1111);
        b.write16(0x80_017E, 0x2222);
        assert_eq!(b.read16(0x80_0140), 0x1111, "the first register");
        assert_eq!(b.read16(0x80_017E), 0x2222, "and the last");
        assert_eq!(
            b.read16(0x80_0172),
            0x0401,
            "and the ID register still is not"
        );
    }

    /// The kicks are read through CPS-B, not the 0x800000 block.
    /// 0x800140 + 0x36 = 0x800176 (`cps1_v.cpp:1838`, `:2155-2156`).
    #[test]
    fn in2_is_read_through_cps_b_at_in2_addr() {
        let mut b = board();
        assert_eq!(b.read16(0x80_0176), 0x00FF, "idle: 0xFF in the low byte");
        b.inputs.p1.kick[2] = true; // bit 2
        assert_eq!(b.read16(0x80_0176), 0x00FB);
        b.inputs.p2.kick[0] = true; // bit 4
        assert_eq!(b.read16(0x80_0176), 0x00EB);
    }

    /// The wired reads come from the config, not from the address.
    ///
    /// With no `cpsb_addr` and no `in2_addr`, both of those addresses are plain
    /// registers. Without this case a hardcoded `0x32` and `0x36` would pass every
    /// `sf2()` test above.
    ///
    /// Not SF1: that board has no CPS-B at all — no register file, plain palette
    /// RAM at 0xB00000 and plain I/O at 0xC00000 — so it needs no [`BoardConfig`]
    /// and will not exercise this. What will is a second CPS-1 title, every one of
    /// which has its own row in `cps1_v.cpp`'s table with its own address pair.
    #[test]
    fn with_a_plain_config_the_wired_addresses_are_ordinary_registers() {
        let mut b = Board::new(&[], BoardConfig::plain());
        assert_eq!(b.read16(0x80_0172), 0x0000, "not 0x0401");
        b.write16(0x80_0172, 0xDEAD);
        assert_eq!(b.read16(0x80_0172), 0xDEAD, "plain RAM");
        b.inputs.p1.kick = [true; 3];
        assert_eq!(b.read16(0x80_0176), 0x0000, "and no IN2 here");
    }

    /// The multiply ports answer with the product of the two factor registers.
    ///
    /// Champion Edition's protection check (`cps1_v.cpp:2145-2152`). Both halves of
    /// a product that does not fit in 16 bits, so `result_hi` is not zero and a
    /// truncating `u16` multiply could not produce these numbers.
    ///
    /// The expectations are hand-computed and written as literals: 0x1234 × 0x0100 =
    /// 0x0012_3400, so lo is 0x3400 and hi is 0x0012. Deriving them in the test with
    /// the same expression the code uses would pass under any factor pairing at all.
    #[test]
    fn the_multiply_ports_return_the_product_of_the_two_factors() {
        let mut b = Board::new(&[], BoardConfig::sf2ce());
        b.write16(0x80_0140, 0x1234); // factor1
        b.write16(0x80_0142, 0x0100); // factor2
        assert_eq!(b.read16(0x80_0144), 0x3400, "low word of 0x00123400");
        assert_eq!(b.read16(0x80_0146), 0x0012, "high word of 0x00123400");
    }

    /// Each factor register is also an ordinary read/write register.
    ///
    /// A write to 0x800140 reads back at 0x800140. That is what makes the pair
    /// *ports* rather than a hidden device: only the two result offsets are
    /// intercepted, so a row that put `result_lo` at a factor's offset would answer
    /// this read with a product instead of the value written.
    ///
    /// ⚠️ This does **not** distinguish `factor1` from `factor2`. Nothing can:
    /// multiplication commutes and both registers are read the same way, so
    /// exchanging the two — in this function or in `sf2ce`'s row — is an exact
    /// equivalence. Mutation confirmed the swap survives the whole suite. Recording
    /// that here rather than leaving a later reader to discover it and assume the
    /// tests are weak: they are as strong as the behaviour allows. What *is* caught
    /// is a wrong pairing — reading one register twice, which squares it — and
    /// `the_product_is_recomputed_on_each_read` fails on that mutant because its two
    /// factors differ.
    #[test]
    fn the_factor_registers_stay_ordinary_read_write_registers() {
        let mut b = Board::new(&[], BoardConfig::sf2ce());
        b.write16(0x80_0140, 0x1234);
        b.write16(0x80_0142, 0x0100);
        assert_eq!(b.read16(0x80_0140), 0x1234, "factor1 reads back as written");
        assert_eq!(b.read16(0x80_0142), 0x0100, "and so does factor2");
        // Word indices, written plainly: clippy rejects `0x00 / 2` (`erasing_op`)
        // and `0x02 / 2` (`eq_op`), so the byte offsets they come from — CE's
        // `factor1` at 0x00 and `factor2` at 0x02 — are named here instead.
        assert_eq!(b.cps_b[0], 0x1234, "factor1, byte offset 0x00");
        assert_eq!(b.cps_b[1], 0x0100, "factor2, byte offset 0x02");
    }

    /// A write to a result register lands in the file and is never read back.
    ///
    /// `cps1_cps_b_w`'s `COMBINE_DATA` runs before any read intercept, exactly as at
    /// the ID register. The store is checked in the file directly, because a bus read
    /// of these two addresses cannot show it: they answer with the product.
    ///
    /// Sub-project C reads `cps_b` directly for the layer and priority registers, so
    /// an arm that skipped the store here would be a lost write with no bus read able
    /// to reveal it — the same trap as
    /// `a_write_to_the_cpsb_id_register_still_lands_in_the_file`.
    #[test]
    fn a_write_to_a_multiply_result_register_still_lands_in_the_file() {
        let mut b = Board::new(&[], BoardConfig::sf2ce());
        b.write16(0x80_0144, 0xDEAD);
        b.write16(0x80_0146, 0xBEEF);
        assert_eq!(b.cps_b[0x04 / 2], 0xDEAD, "word index 2");
        assert_eq!(b.cps_b[0x06 / 2], 0xBEEF, "word index 3");
        assert_eq!(b.read16(0x80_0144), 0x0000, "but the read is 0 × 0");
        assert_eq!(b.read16(0x80_0146), 0x0000);
    }

    /// The product is recomputed on every read, not latched when a factor is written.
    ///
    /// MAME computes it inside the read handler. The difference is observable with
    /// one write between two reads: a latching implementation would answer the second
    /// read with the first read's product.
    ///
    /// Written with a **second factor that changes the answer**, and each expectation
    /// hand-computed: 0x0003 × 0x0005 = 0x0F, then 0x0003 × 0x0007 = 0x15. A test
    /// that re-read the same product twice would pass under a latch.
    #[test]
    fn the_product_is_recomputed_on_each_read() {
        let mut b = Board::new(&[], BoardConfig::sf2ce());
        b.write16(0x80_0140, 0x0003);
        b.write16(0x80_0142, 0x0005);
        assert_eq!(b.read16(0x80_0144), 0x000F);
        b.write16(0x80_0142, 0x0007);
        assert_eq!(b.read16(0x80_0144), 0x0015, "0x3 × 0x7, not the stale 0xF");
    }

    /// The largest product the ports can produce does not panic or wrap.
    ///
    /// 0xFFFF × 0xFFFF = 0xFFFE_0001, which overflows a `u16` multiply — a debug
    /// build panics on it and a release build silently answers 0x0001 for both
    /// halves. The bus contract is that no guest access panics, and the guest reaches
    /// this by writing two words it is entirely entitled to write.
    #[test]
    fn the_widest_product_neither_panics_nor_wraps() {
        let mut b = Board::new(&[], BoardConfig::sf2ce());
        b.write16(0x80_0140, 0xFFFF);
        b.write16(0x80_0142, 0xFFFF);
        assert_eq!(b.read16(0x80_0144), 0x0001, "low word of 0xFFFE0001");
        assert_eq!(b.read16(0x80_0146), 0xFFFE, "high word — not 0x0001 again");
    }

    /// A factor offset outside the CPS-B window indexes in range instead of panicking.
    ///
    /// `CPS_REGS` is 0x20 words, so a byte offset must be under 0x40 for `off >> 1`
    /// to index the file — and every real row's is: `config.rs` asserts it for
    /// `sf2ce`, and MAME's table has no row that violates it. So `& (CPS_REGS - 1)`
    /// in [`Board::multiply_at`] is dead on every configuration this workspace ships,
    /// and mutation confirmed that removing it fails no other test.
    ///
    /// It is not dead on every configuration that *compiles*. [`BoardConfig`]'s
    /// fields are public, so a caller — a future MAME row transcribed with a slip, a
    /// fuzz harness, a test — can hand the board an offset of 0x80. Unmasked, that is
    /// `0x40` into a 32-word array: a panic, on a guest read of an address the guest
    /// is entitled to read. The bus contract is that no guest address panics, and
    /// this is the assertion that keeps the mask honest rather than a comment saying
    /// it is probably needed.
    ///
    /// The row here puts the **factors** out of window and leaves `result_lo` inside
    /// it, which is the only shape that reaches the indexing at all: a `result_lo` of
    /// 0x80 is never compared equal to an `off` that is always under 0x40, so the
    /// product would never be computed.
    #[test]
    fn an_out_of_window_factor_offset_wraps_rather_than_panicking() {
        let mut cfg = BoardConfig::sf2ce();
        cfg.multiply = Some(crate::config::MultiplyPorts {
            factor1: 0x80,
            factor2: 0x82,
            result_lo: 0x04,
            result_hi: 0x06,
        });
        let mut b = Board::new(&[], cfg);
        // 0x80 >> 1 = 0x40, masked to word index 0; 0x82 >> 1 = 0x41, to index 1.
        // Those are the registers at byte offsets 0x00 and 0x02.
        b.write16(0x80_0140, 0x0003);
        b.write16(0x80_0142, 0x0005);
        assert_eq!(b.read16(0x80_0144), 0x000F, "wrapped, and did not panic");
    }

    /// The multiply behaviour comes from the config, not from the addresses.
    ///
    /// Under `sf2()` — a row whose `multiply` is `None` — all four of CE's offsets
    /// are plain registers. Without this case a hardcoded `0x04`/`0x06` would pass
    /// every test above, and every SF2 board in the workspace would answer 0x800144
    /// with a product it never computed.
    ///
    /// The existing `other_cps_b_registers_are_read_write` already writes 0x1111 to
    /// 0x800140 and reads it back, and passes for exactly this reason; it is asserted
    /// here explicitly rather than left as a coincidence of that test's choice of
    /// address.
    #[test]
    fn without_a_multiply_row_the_result_offsets_are_ordinary_registers() {
        let mut b = board(); // sf2: multiply is None
        b.write16(0x80_0140, 0x0003);
        b.write16(0x80_0142, 0x0005);
        b.write16(0x80_0144, 0xDEAD);
        b.write16(0x80_0146, 0xBEEF);
        assert_eq!(b.read16(0x80_0144), 0xDEAD, "not 0x000F");
        assert_eq!(b.read16(0x80_0146), 0xBEEF, "not 0x0000");
        // And on the plain row too, which shares no field with sf2's.
        let mut p = Board::new(&[], BoardConfig::plain());
        p.write16(0x80_0140, 0x0003);
        p.write16(0x80_0142, 0x0005);
        assert_eq!(p.read16(0x80_0144), 0x0000, "untouched, not a product");
    }

    /// CE's other two wired reads still work alongside the multiply ports.
    ///
    /// The read path is an if-chain, so an intercept inserted in the wrong place can
    /// shadow a later arm. This checks the whole row at once: the ID register, IN2,
    /// and a register that is none of the five.
    ///
    /// CE's `cpsb_value` is 0xFFFF — `uint16_t(-1)` — which is also this bus's
    /// unmapped-read value, so the assertion is paired with a write that would show
    /// through if the address were falling out of the CPS-B arm entirely.
    #[test]
    fn ces_id_register_and_in2_read_alongside_the_multiply_ports() {
        let mut b = Board::new(&[], BoardConfig::sf2ce());
        b.write16(0x80_0172, 0x1234);
        assert_eq!(b.read16(0x80_0172), 0xFFFF, "wired, not the 0x1234 written");
        assert_eq!(b.cps_b[0x32 / 2], 0x1234, "which did land in the file");

        assert_eq!(b.read16(0x80_0176), 0x00FF, "IN2, idle");
        b.inputs.p1.kick[1] = true;
        assert_eq!(b.read16(0x80_0176), 0x00FD, "bit 1 low");

        // A register in the window that is none of the five.
        b.write16(0x80_0150, 0xABCD);
        assert_eq!(b.read16(0x80_0150), 0xABCD);
    }

    /// CPS-A is indexed by word, and this is the boundary the plan warns about.
    ///
    /// 0x800100 is word index 0; 0x80010C is `CPS1_SCROLL1_SCROLLX`, which
    /// `cps1.h:182` writes as `0x0c / 2` = 6. An index of 0x0C here would put
    /// scroll-1's X where scroll-2's Y belongs — one register off, and every value
    /// in the file looks plausible in the wrong slot.
    #[test]
    fn cps_a_writes_land_in_the_register_file_by_word_index() {
        let mut b = board();
        b.write16(0x80_010C, 0x0040);
        assert_eq!(b.cps_a[6], 0x0040, "CPS1_SCROLL1_SCROLLX, cps1.h:182");
        assert_eq!(b.cps_a[0x0C], 0x0000, "not indexed by the byte offset");
        b.write16(0x80_0100, 0x9000);
        b.write16(0x80_013E, 0x0001);
        assert_eq!(b.cps_a[0], 0x9000, "the first register");
        assert_eq!(b.cps_a[0x1F], 0x0001, "and the last");
    }

    /// CPS-B is indexed by word too, and its file is separate from CPS-A's.
    #[test]
    fn cps_b_writes_land_in_their_own_file_by_word_index() {
        let mut b = board();
        b.write16(0x80_0146, 0x0055);
        assert_eq!(b.cps_b[3], 0x0055);
        assert_eq!(b.cps_a[3], 0x0000, "the two files are not the same array");
        b.write16(0x80_0106, 0x00AA);
        assert_eq!(b.cps_a[3], 0x00AA);
        assert_eq!(b.cps_b[3], 0x0055, "and CPS-A's write did not reach CPS-B");
    }

    /// A word write to a sound latch latches the **low** byte.
    ///
    /// `cps1_soundlatch_w` (`cps1.cpp:300-306`) is
    /// `if (ACCESSING_BITS_0_7) write(data & 0xff); else write(data >> 8);` — a
    /// word write asserts both lanes, so the first branch holds. The literals are
    /// asymmetric so a model that took `>> 8` fails.
    #[test]
    fn the_sound_latches_take_the_low_byte_of_a_word_write() {
        let mut b = board();
        b.write16(0x80_0180, 0x12AB);
        assert_eq!(b.sound_latch[0], 0xAB, "not 0x12");
        b.write16(0x80_0188, 0x34CD);
        assert_eq!(b.sound_latch[1], 0xCD, "not 0x34");
    }

    /// Both latches are mirrored across their eight bytes, and they are distinct.
    #[test]
    fn the_two_sound_latches_are_separate_and_mirrored() {
        let mut b = board();
        for a in [0x80_0180u32, 0x80_0182, 0x80_0184, 0x80_0186] {
            b.write16(a, 0x0011);
            assert_eq!(b.sound_latch[0], 0x11, "{a:#x} mirrors the command latch");
            assert_eq!(b.sound_latch[1], 0x00, "and does not touch the fade latch");
        }
        for a in [0x80_0188u32, 0x80_018A, 0x80_018C, 0x80_018E] {
            b.write16(a, 0x0022);
            assert_eq!(b.sound_latch[1], 0x22, "{a:#x} mirrors the fade latch");
            assert_eq!(
                b.sound_latch[0], 0x11,
                "and does not touch the command latch"
            );
        }
        assert!(
            !b.write_word(0x80_0190, 0x0033),
            "0x800190 is past the block"
        );
        assert!(b.read_word(0x80_0180).is_none(), "the latches do not read");
    }

    /// A byte write to a write-only port latches the byte on its own lane.
    ///
    /// This is what the read-modify-write `write8` of Task 5 got wrong. Reading
    /// 0x800180 back returns 0xFFFF — nothing there reads — so merging a low byte
    /// into it and writing the merged word would latch 0xFF for a high-byte write
    /// and, worse, would have latched the *low* byte 0xFF even for a high write,
    /// because a word write takes the low half.
    #[test]
    fn a_byte_write_to_a_sound_latch_uses_the_addressed_lane_not_a_merge() {
        let mut b = board();
        b.write8(0x80_0181, 0xAB);
        assert_eq!(b.sound_latch[0], 0xAB, "the odd address is the low lane");
        b.write8(0x80_0180, 0xCD);
        assert_eq!(
            b.sound_latch[0], 0xCD,
            "the even address is the high lane, and cps1_soundlatch_w's else \
             branch latches data >> 8 — which is this byte, not 0xFF"
        );
        // soundlatch2 has no high-lane branch at all (cps1.cpp:308-312).
        b.write8(0x80_0189, 0x11);
        assert_eq!(b.sound_latch[1], 0x11, "low lane writes");
        b.write8(0x80_0188, 0x22);
        assert_eq!(
            b.sound_latch[1], 0x11,
            "a high-lane write to soundlatch2 is decoded and discarded"
        );
    }

    /// `cps1_coinctrl_w` uses only the high half. `cps1.cpp:316-327`.
    #[test]
    fn the_coin_control_records_high_half_writes_and_ignores_low_half_ones() {
        let mut b = board();
        b.write16(0x80_0030, 0x0100);
        assert_eq!(b.coin_ctrl, 0x0100, "coin counter 1 is bit 8");
        b.write8(0x80_0037, 0x03);
        assert_eq!(
            b.coin_ctrl, 0x0100,
            "a low-lane write is inside ACCESSING_BITS_8_15's else, so nothing happens"
        );
        b.write8(0x80_0030, 0x02);
        assert_eq!(b.coin_ctrl, 0x0200, "and a high-lane write merges");
        assert!(
            b.read_word(0x80_0030).is_none(),
            "the coin control does not read"
        );
    }

    /// A byte write to a register file merges rather than clobbering.
    ///
    /// `COMBINE_DATA` (`cps1_v.cpp:2117`, `:2185`). Distinguishing this from the
    /// write-only ports above is the whole reason [`Lanes`] exists rather than a
    /// bool.
    #[test]
    fn a_byte_write_to_a_register_file_merges_with_the_other_half() {
        let mut b = board();
        b.write16(0x80_0140, 0x1234);
        b.write8(0x80_0140, 0xAB);
        assert_eq!(b.cps_b[0], 0xAB34, "the high lane alone");
        b.write8(0x80_0141, 0xCD);
        assert_eq!(b.cps_b[0], 0xABCD, "and the low lane alone");
        b.write16(0x80_0100, 0x1234);
        b.write8(0x80_0101, 0x99);
        assert_eq!(b.cps_a[0], 0x1299, "and the same for CPS-A");
    }

    /// The whole I/O block, one address at a time: which words decode a read and
    /// which decode a write.
    ///
    /// The individual tests above each pin one port. This one pins the *map* — the
    /// thing an off-by-one in any arm's bounds breaks, and the thing no
    /// single-port test can see. Every range is a literal transcribed from
    /// `cps1.cpp:579-591`, not derived from the constants the arms use.
    #[test]
    fn the_io_block_decodes_exactly_the_ranges_mame_maps() {
        // (first, last, reads, writes) — inclusive byte bounds, word-aligned.
        let ports: &[(u32, u32, bool, bool)] = &[
            (0x80_0000, 0x80_0006, true, false),  // portr("IN1")
            (0x80_0008, 0x80_0016, false, false), // the development-leftover gap
            (0x80_0018, 0x80_001E, true, false),  // cps1_dsw_r
            (0x80_0020, 0x80_0020, true, false),  // nopr()
            (0x80_0022, 0x80_002E, false, false),
            (0x80_0030, 0x80_0036, false, true), // cps1_coinctrl_w
            (0x80_0038, 0x80_00FE, false, false),
            (0x80_0100, 0x80_013E, false, true), // cps1_cps_a_w
            (0x80_0140, 0x80_017E, true, true),  // cps1_cps_b_r / _w
            (0x80_0180, 0x80_0186, false, true), // cps1_soundlatch_w
            (0x80_0188, 0x80_018E, false, true), // cps1_soundlatch2_w
            (0x80_0190, 0x80_01FE, false, false),
        ];
        let mut b = board();
        for &(first, last, reads, writes) in ports {
            let mut a = first;
            while a <= last {
                assert_eq!(
                    b.read_word(a).is_some(),
                    reads,
                    "{a:#08x}: read decode should be {reads}"
                );
                assert_eq!(
                    b.write_word(a, 0),
                    writes,
                    "{a:#08x}: write decode should be {writes}"
                );
                a += 2;
            }
        }
        // The listed ranges must actually tile 0x800000-0x8001FF with no gap and
        // no overlap — otherwise a hole in this table would look like coverage.
        assert_eq!(ports[0].0, 0x80_0000);
        assert_eq!(ports[ports.len() - 1].1, 0x80_01FE);
        for w in ports.windows(2) {
            assert_eq!(
                w[1].0,
                w[0].1 + 2,
                "{:#08x}..{:#08x} does not abut the next range",
                w[0].0,
                w[0].1
            );
        }
    }

    /// Sanity: the I/O block never reaches RAM or ROM storage.
    ///
    /// A missing `0x80_...` arm would fall through to `_ => None`, which the
    /// decode test catches. A *misplaced* arm — a range written 0x00_0140 instead
    /// of 0x80_0140 — would instead corrupt ROM-space reads, which nothing above
    /// would notice.
    #[test]
    fn io_writes_do_not_disturb_ram_gfxram_or_the_rom_image() {
        let mut b = Board::new(&[0x12, 0x34], BoardConfig::sf2());
        b.write16(0xFF_0000, 0x1111);
        b.write16(0x90_0000, 0x2222);
        let mut a = 0x80_0000u32;
        while a < 0x80_0200 {
            b.write16(a, 0xDEAD);
            a += 2;
        }
        assert_eq!(b.read16(0x00_0000), 0x1234, "the ROM image is intact");
        assert_eq!(b.read16(0xFF_0000), 0x1111, "main RAM is intact");
        assert_eq!(b.read16(0x90_0000), 0x2222, "gfxram is intact");
    }

    /// An odd word address in the I/O block reads the word containing it.
    ///
    /// Found by mutation: dropping `& !1` from the CPS-B arm's offset survived
    /// every test above, because none of them read an odd I/O address. The
    /// consequence is a real behaviour change and not a cosmetic one — with an odd
    /// offset, 0x800173 misses the `cpsb_addr` comparison and returns the register
    /// file's contents instead of the wired 0x0401, contradicting
    /// `an_odd_word_address_truncates_rather_than_panicking`, which is the
    /// board-wide contract the debugger relies on.
    #[test]
    fn an_odd_address_in_the_io_block_reads_the_containing_word() {
        let mut b = board();
        b.inputs.dsw = [0x12, 0x34, 0x56];
        b.inputs.p1.up = true;
        b.write16(0x80_0172, 0xDEAD);
        b.write16(0x80_0140, 0x1111);
        assert_eq!(
            b.read16(0x80_0173),
            0x0401,
            "the wired ID register, not 0xDEAD"
        );
        assert_eq!(b.read16(0x80_0177), 0x00FF, "IN2");
        assert_eq!(b.read16(0x80_0141), 0x1111, "an ordinary CPS-B register");
        assert_eq!(b.read16(0x80_0001), 0xFFF7, "IN1");
        assert_eq!(b.read16(0x80_001B), 0x12FF, "DSWA, not DSWB");
    }

    /// `Lanes` is the file's one polarity-and-position decision, so it gets its
    /// own literals.
    #[test]
    fn lanes_place_merge_and_select_the_documented_halves() {
        assert_eq!(Lanes::of_byte(0x80_0180), Lanes::High, "even = UDS");
        assert_eq!(Lanes::of_byte(0x80_0181), Lanes::Low, "odd = LDS");

        assert_eq!(Lanes::Word.place(0xAB), 0x00AB);
        assert_eq!(Lanes::High.place(0xAB), 0xAB00);
        assert_eq!(Lanes::Low.place(0xAB), 0x00AB);

        assert_eq!(Lanes::Word.merge(0x1234, 0xABCD), 0xABCD);
        assert_eq!(Lanes::High.merge(0x1234, 0xABCD), 0xAB34);
        assert_eq!(Lanes::Low.merge(0x1234, 0xABCD), 0x12CD);

        assert_eq!(Lanes::Word.byte_written(0x12AB), 0xAB, "the low byte wins");
        assert_eq!(Lanes::High.byte_written(0x12AB), 0x12);
        assert_eq!(Lanes::Low.byte_written(0x12AB), 0xAB);
    }
}
