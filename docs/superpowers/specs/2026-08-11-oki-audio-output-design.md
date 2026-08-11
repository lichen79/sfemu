# Design: the OKI MSM6295, the mono mix, and host audio (sfemu sub-project D3)

Date: 2026-08-11
Scope: Sub-project D3 of the sfemu arcade emulator
Status: draft
Depends on: B (board and bus), C (video and the frame schedule), D1 (the Z80 core),
D2 (the YM2151 and the sound board), E1 (the frontend and its run loop)

**One sentence:** an MSM6295 ADPCM sample player in `crates/oki`, the CPS-1's mono
mix, a resampler onto the host's rate, and a host audio device — **this is the one
that ends "there is no sound."**

---

## Context

Eight sub-projects are complete: the 68000 core (A), the board and ROM loader (B),
CPS-1 video (C), the Z80 core (D1), the YM2151 and the sound board (D2), and the
frontend's three surfaces (E1 window and save states, E2 debugger, E3 graphics
viewers).

D2 stopped one step short of sound, deliberately. Its own scope note says what it
left: "the OKI MSM6295's ADPCM decoding, the host audio device, the mono mix CPS-1
actually sends to the cabinet speaker, resampling to the host's rate, and the
buffer-underrun policy." Everything up to the samples exists; the samples go into a
`Vec<(i16, i16)>` that nothing drains.

Four seams already exist and are traced, so D3 does not invent them:

- **The sample buffer.** `Cps1::samples() -> &[(i16, i16)]` and
  `Cps1::drain_samples()` (`cps1.rs:181,186`), with the accrual loop at
  `cps1.rs:373-379`. `a_frame_produces_nine_hundred_thirty_seven_or_eight_samples`
  pins the per-frame count.
- **The two OKI addresses.** `0xF002` is decoded on the Z80 bus, counted into
  `SoundTrace::oki_writes`, and reads back `0x00`; `0xF006` sets `oki_pin7`
  (`sound.rs:353,355`). Both are recorded stubs D2 put there "precisely so D3 has
  evidence about how the driver uses them."
- **The ROM region.** `romset` populates `oki` — 0x40000 bytes, two concatenated
  128 KB files, CRC-verified, with `oki_samples_are_concatenated_without_a_gap`
  at `games.rs:256`. Nothing consumes it: `grep '"oki"'` over
  `crates/machine/src` and `crates/sfemu/src` returns nothing. Wiring it up is
  D3's first job.
- **The save state.** `MachineState::oki_pin7` already exists, carried by D2 "so
  that a state written today is not silently missing it when D3 lands"
  (`snapshot.rs:88`).

## What this is not

- **Not stereo.** The CPS-1 is mono: `SPEAKER(config, "mono").front_center()`
  (`cps1.cpp:3935`), with both YM2151 channels and the OKI routed into that one
  node. See "The mix is mono, and that is a downgrade" below — this is the one
  design decision in D3 that makes the emulator's output *less* like what a modern
  player expects, and it is the correct one.
- **Not a resampling library.** No windowed-sinc, no polyphase filter bank, no
  crates.io DSP dependency. See "Resampling" — the rate ratio is fixed and known
  at compile time, and the chosen method is stated with its measured cost.
- **Not the MSM6585, the MSM9810, or `oki_adpcm2_state`.** MAME implements those
  beside the 6295 in the same files. CPS-1 has an MSM6295 (`cps1.cpp:3946`).
- **Not sound-driven timing.** The emulator's clock stays the frame pacer's. D3
  does not slew the machine to the audio device's crystal; see "Two clocks" for
  the measurement that says it does not need to.
- **Not the SF1 driver.** Sub-project F.

---

## Verification, and why D3's ground truth is a different problem from D2's

D2's answer was ymfm. **ymfm cannot serve D3.** It is Yamaha-only: its source tree
(`ymfm.h`, `ymfm_adpcm.*`, `ymfm_fm.*`, `ymfm_misc.*`, `ymfm_opl.*`, `ymfm_opm.*`,
`ymfm_opn.*`, `ymfm_opq.*`, `ymfm_opx.h`, `ymfm_opz.*`, `ymfm_pcm.*`, `ymfm_ssg.*`)
contains no MSM6295 — `grep -rn "6295\|6205\|MSM"` over its headers returns
nothing. Verified 2026-08-11 against the fetched tree.

And there is no published vector suite for the MSM6295, as there was none for the
YM2151. So D3 faces D2's central problem again, and answers it the same way:
**generate vectors from a reference implementation that compiles and runs on this
machine.**

### Ground truth: MAME's `okiadpcm`, compiled locally

`src/devices/sound/okiadpcm.cpp` and `.h` — **BSD-3-Clause**, copyright-holders
Andrew Gardner and Aaron Giles. This is the decoder MAME itself uses for the 6295,
and the algorithm is 32 lines.

**Verified 2026-08-11 by compiling and running it on this machine**, not assumed:

- `okiadpcm.cpp` is **self-contained apart from `emu.h`**, which supplies only the
  standard headers it needs. Substituting `#include <cstdint>` and `#include
  <cmath>` for that one line makes it build with Apple clang under
  `c++ -std=c++17 -O2`, with nothing else from MAME.
- A 16-nibble probe produced real output: nibble `F` alone gives signal −48 with
  step 8; four consecutive `7`s ramp 15 → 151 → 444 → 1075; the negative clamp
  was confirmed reached at −2048.
- The algorithm: `s_index_shift[8] = {-1,-1,-1,-1,2,4,6,8}`, indexed by
  `nibble & 7`; a 49×16 `s_diff_lookup` computed as
  `floor(16.0 * pow(11.0/10.0, step))` decomposed through a sign/magnitude
  `nbl2bit` table; signal clamped to −2048..2047 and step clamped 0..48.
- `reset()` zeroes signal and step. There is no dither, no filter, and no
  per-voice interpolation: one nibble in, one 12-bit sample out.

`okim6295.cpp` — the chip *wrapper* — is BSD-3 as well but **does not compile
standalone**: it is a `device_t` and needs MAME's device layer. That is a real
difference from D2, where the whole reference built as-is, so it was verified
rather than assumed:

- **A standalone wrapper over the BSD-3 decoder was written and run** (2026-08-11).
  Its logic is transcribed from `okim6295.cpp`'s `write`, `read` and
  `generate_adpcm`; the decoder itself is MAME's file, unmodified apart from the
  include substitution. It produced: status `0xF0` idle → `0xF1` with voice 0
  playing → `0xF0` after the phrase ended; a 256-sample phrase from a 0x80-byte
  region, every sample non-zero, reaching full scale; and **zero energy over 50
  samples at volume index 9**, which is the silent-index claim measured rather
  than read off the table.

So the generator is a **thin wrapper the repository owns, over a reference decoder
it does not.** The wrapper is ~70 lines of transcribed protocol; the arithmetic
that would be hard to get right is MAME's. This is worth stating plainly because it
is weaker ground truth than D2 had: agreeing with the generator's wrapper is
agreeing with *my transcription* of the chip protocol, not with MAME's own. The
mitigation is that the protocol is the part the driver exercises constantly and the
real-ROM trace test can corroborate, while the arithmetic — the part no test could
plausibly reverse-engineer from behaviour — is the part taken from the reference
verbatim.

### The chip protocol, from `okim6295.cpp`

Four voices. Command writes to `0xF002` are a **two-byte sequence**, and a
partially-written command is a real state the chip holds:

| step | byte | effect |
|------|------|--------|
| 1 | `0x80 \| phrase` | latch `phrase` (7 bits); nothing plays yet |
| 2 | `vvvv gggg` | `vvvv` is the voice mask, `gggg` the volume index |
| — | `0nnnn000`, bit 7 clear | **stop**: mask is `command >> 3`, stops those voices |

On the second byte, for each masked voice **that is not already playing** (MAME's
comment: "fixes Got-cha and Steel Force"), the phrase table at `phrase * 8` is
read for a 3-byte start and a 3-byte stop, each masked to `0x3FFFF`; a phrase with
`start >= stop` is rejected and logged rather than played. Otherwise
`count = 2 * (stop - start + 1)` nibbles, the decoder is reset, and the volume is
`s_volume_table[gggg]`.

Nibble fetch, per output sample:
`read_byte(base + sample / 2) >> (((sample & 1) << 2) ^ 4)` — **high nibble first**.
That `^ 4` is the whole of the ordering, and getting it backwards produces audio
that is recognisably speech-like and completely wrong, which is exactly the class
of defect a listening test passes.

Status reads return `0xF0` with bit `n` set per playing voice — "naname expects
bits 4-7 to be 1" (`okim6295.cpp:216`). **D2's stub returns `0x00`**, which claims
"no voices playing" *and* clears the four high bits a real chip drives; D3 replaces
it.

The volume table is 16 entries and **indices 9 through 15 are exactly zero**:
`{0x20, 0x16, 0x10, 0x0b, 0x08, 0x06, 0x04, 0x03, 0x02, 0, 0, 0, 0, 0, 0, 0} / 0x20`.

### Integer volume, and why it is bit-exact rather than merely close

MAME scales in `float`: `signal * volume / 2048`. This project has no floating
point in a chip core and will not gain one. **Measured, not argued** (2026-08-11):
over all 16 volume indices × all 4,096 possible signal values, the maximum
difference between MAME's `f32` path and an integer `signal * numerator` path
scaled by 1/65536 is **exactly 0**. Every table entry `n/32` is representable in
`f32`, and the products are small enough that no rounding occurs. So the Rust
decoder is integer throughout and the equivalence is a fact rather than a
tolerance — and the test asserts the 65,536-case identity, not a sampled subset.

### The vector suite

Same shape as D2's, and the same rules: generated by
`cargo run -q -p testrunner --release --bin genoki`, written to gitignored
`testdata/oki/`, **never committed**. A missing file fails loudly naming the file
and the generate command — no `#[ignore]`, no environment-variable escape hatch,
per this project's standing rule with no exemption for diagnostics.

**No ROM is involved.** The generator synthesises its own sample ROM: a phrase
table it writes plus deterministic pseudo-random nibble data. This is not a
concession — it is *better* coverage than a real ROM, which contains the handful of
phrases one game happens to use, while a synthetic region can walk the step-index
ladder to both clamps and back.

**1,000 cases of 512 samples**, the same shape as D2's suite, and the size is
arithmetic rather than a round number: 512 samples is 67.6 ms at pin7 high and
84.5 ms at pin7 low — long enough for a phrase to start, run and end naturally
inside one case, since a 512-sample phrase needs only 256 bytes of ADPCM data. The
record is 6 bytes per sample (`i16 + u8 + u8 + u16`), so the file is **3.07 MB** —
within 4% of D2's 2.97 MB, which is the practical reason to keep the same
dimensions. One shared 256 KB synthetic ROM serves every case: a 1,024-byte phrase
table for 128 phrases, then 1,020 distinct 256-byte phrase bodies, which is more
than 128 phrases can name and leaves room for the deliberate step-ladder segments.

A case is: a script of `(at_sample, byte)` writes to the command port, and a
per-sample record. The record carries **four fields, and each is there because the
others cannot catch a specific defect**:

| field | catches |
|-------|---------|
| `mono: i16` | the arithmetic — the decoder, the volume, the voice sum |
| `status: u8` | voice lifetime: a phrase that ends one sample early or late |
| `voices: u8` | which voices, not how many — a mask decoded as a count |
| `nibbles: u16` | the *fetch order*: a high/low swap that happens to sound plausible |

`nibbles` is the field that matters most and the one a "compare the audio" design
would omit. A `^ 4` dropped from the shift expression changes the audio
drastically, so `mono` catches it — but a *bank*-level addressing error that
returns the right nibbles in the right order from the wrong byte does not
necessarily, and this field states the claim directly.

### The premises, asserted as tests on the generated data

D2's most valuable finding was that a suite can pass vacuously, so the same
coverage tests exist here, each with its floor derived from a measured probe run
rather than chosen. The suite must contain: cases where all four voices sound at
once; cases using a silent volume index (which must produce exact zero); cases
where a phrase runs to its natural end and the voice stops; cases where a stop
command truncates a phrase; cases where a second command arrives for an
already-playing voice and is **ignored**; cases at both pin-7 divisors; and cases
whose step index reaches both 0 and 48. Cases must not all be the same case
(script hash and audio hash, as `ymsuite.rs` does).

**The step-index premise needs the generator's help, and measuring it is what
found that.** Run on the reference decoder 2026-08-11: 4,096 pseudo-random nibbles
drive the step index to **2..48** — it reaches the upper clamp easily and **never
reaches 0**. Random data has no long run of small nibbles, and each one only
decrements by 1 while a large one adds 8. So a generator filling its ROM with
pseudo-random data alone leaves the lower clamp untested, and a premise test
demanding 0 would fail on data that is otherwise perfectly good. The generator
therefore includes a deliberate segment — a run of large nibbles then a long run of
zeros, verified to walk 0 → 48 → 0 — and the floor for the random cases is stated
as 48-reached rather than both. Two further measured facts from the same probe,
both worth a case: nibble `4` saturates the step from reset in **24 clocks**, and
after saturating, 64 nibbles of `0` return the step to 0 while the **signal stays
pinned at 2047**, because nibble 0 still adds `stepval/8`. A decoder that let the
signal decay there would pass an "it makes sound" test and be wrong.

And the runner must be able to fail: `the_runner_reports_a_deliberately_corrupted_sample`'s
counterpart, corrupting one sample of each of the four fields in turn and asserting
the runner names that field and that sample. Without it, `N/N` means nothing.

---

## The chip's clock, and the one number that comes out exact

The OKI's clock is `XTAL(16'000'000)/4/4` (`cps1.cpp:3946`) — **1,000,000 Hz**,
divided again by 132 or 165 depending on pin 7:

| pin 7 | divisor | sample rate |
|-------|---------|-------------|
| high (`1`) | 132 | 1,000,000 / 132 = **7,575.7576 Hz** |
| low (`0`) | 165 | 1,000,000 / 165 = **6,060.6061 Hz** |

Both are inexact, and the ratio against the YM's 55,930.390625 Hz is ugly:
3,200,000 / 23,624,997 for pin7 high. A `RationalAccumulator` over YM samples would
work, but there is a much better denominator available, and finding it is the
scheduling result of D3:

**The OKI's clock divides the scanline exactly.** 16 MHz / 2 = 8 MHz pixel clock,
/ 512 = 15,625 lines per second, and 1,000,000 / 15,625 = **64 OKI input clocks per
scanline, with no remainder.** Verified: 16/33 samples per line × 15,625 lines/s is
exactly 250,000/33 Hz, and 64/165 × 15,625 is exactly 200,000/33 Hz — the two rates
in the table, reproduced with no error at all. `timing.rs` gains
`OKI_CLOCKS_PER_LINE = 64` with that exactness asserted, beside the existing
assertion that the Z80's `715909/3125` is *not* exact.

**But the mix cannot happen per line, and the same arithmetic is what shows it.**
64/132 is 0.485 of a sample and 64/165 is 0.388 — **fewer than one OKI sample per
scanline in both cases.** So "generate this line's OKI samples" produces 0 or 1, and
a mix driven off the line boundary would have nothing to mix on most lines.

The resolution keeps the exact per-line rate and puts the interpolation where the
YM samples are produced. The OKI advances against the **Z80's T-states**, like the
YM does, using a ratio derived from the exact per-line figure: OKI samples per YM
sample is 3,200,000/23,624,997 at pin7 high and 2,560,000/23,624,997 at pin7 low
(both numerator and denominator fit `u32`, verified). One OKI sample is produced
every ~7.38 YM samples at pin7 high, and the accumulator's remainder *is* the
interpolation phase — the fraction between the last OKI sample and the next — so
the linear interpolation the resampling section specifies needs no separate state.

The per-line number is not discarded: it is the **derivation** of those ratios and
the thing `timing.rs` asserts, because 64-clocks-per-line is a checkable fact about
the board while 3,200,000/23,624,997 is a quotient nobody can eyeball. The test
asserts the ratio equals `64/132 ÷ (YM_SAMPLE_CLOCKS/Z80 T per line)` rather than
restating the literal — a constant asserted against a copy of itself is this
branch's characteristic defect.

**Pin 7 changes mid-stream.** `cps1.cpp:299` — `m_oki->set_pin7(BIT(data, 0))` —
and MAME's own comment on the config line says "pin 7 can be changed by the game
code, see f006 on z80." So the rate changes while the accumulator holds a
remainder — and here the two ratios cooperate: **3,200,000/23,624,997 and
2,560,000/23,624,997 share a denominator**, so a pin-7 write changes only the
numerator and the carried remainder keeps its units exactly. The remainder is
**kept**, not reset: it is a fraction of a sample period already elapsed, which is
physical and does not vanish because the divider changed.
`RationalAccumulator::with_remainder` already exists for exactly this kind of
reconstruction (`timing.rs:113`), and the shared denominator means it is a
numerator swap rather than a rebuild.

That the denominators match is not luck — both rates are 1,000,000/divisor against
the same YM rate, so the denominator is the YM's and only the divisor moves. Stated
because a reader who assumed a rebuild was needed would add state that then has to
be saved and restored.

### A D2 defect this uncovered

`SoundBoard::new` sets `oki_pin7: false` (`sound.rs:145`). **That is wrong.** MAME
constructs the device with `okim6295_device::PIN7_HIGH` (`cps1.cpp:3946`) and
`device_reset` stops the voices without touching `m_pin7_state`
(`okim6295.cpp:143-148`) — verified by reading both functions, 2026-08-11. So the
chip's rate before the driver writes `0xF006` is 7,575.76 Hz, not 6,060.61 Hz.

In D2 this was unobservable, because nothing read the flag. In D3 it is a 25%
pitch error on every sample the driver plays before its first pin-7 write. D3 fixes
the default to `true` in `SoundBoard::new` and `Cps1::reset`, and the test asserts
the reset value against the divisor it implies rather than against the boolean —
a test reading `oki_pin7 == true` would pass on a board that ignored the field.

---

## The mix is mono, and that is a downgrade

`cps1.cpp:3935-3946`:

```
SPEAKER(config, "mono").front_center();
ym2151.add_route(0, "mono", 0.35);
ym2151.add_route(1, "mono", 0.35);
OKIM6295(...).add_route(ALL_OUTPUTS, "mono", 0.30);
```

The YM2151's two channels are **summed into one node**, each at 0.35. The board has
one speaker. So the CPS-1's stereo FM is thrown away by the cabinet, and an
emulator that presented left and right separately would sound *better than the
hardware* while being wrong.

D3 mixes to mono, and the mono value is what the vector suite compares. The host
stream is opened with the device's default channel count and the mono sample is
written to every channel — which is what `front_center` on a two-speaker desk
means. This is stated in the spec rather than left to the implementer because it is
the one place where "more accurate" and "sounds nicer" point in opposite
directions, and someone will want to revisit it.

The weights are exact rationals, not floats: 35/100 and 30/100. Measured
2026-08-11, MAME's `f32` weights differ from exact 35/100 by at most
**0.00039 of one i16 LSB** on the YM pair, so the integer form is not an
approximation of MAME — it is MAME's intent without the `f32` representation error
(`0.35f` is actually 0.3499999940395355).

### Clipping, and why the headroom is not theoretical

The naive worst case is 0.35 + 0.35 + 0.30 = 1.0, which looks like it cannot clip.
It can, easily:

- **The YM pair is not one signal.** Both channels reach ±32,767 independently, so
  the FM contribution alone is 0.70 of full scale — measured peak across the 1,000
  vector cases is **21,120** (64% of full scale), on a script with no attempt to
  maximise output.
- **The OKI's four voices sum without a limiter.** Each is ±2048 scaled by a
  volume of at most 1.0, and `generate_adpcm` does `stream.add_int(...)` per voice
  with no clamp. Four voices at full volume is **4× full scale** before the 0.30
  weight — 1.2 on its own.
- Measured worst case with the YM at its observed peak and the OKI at four full
  voices: **1.65× full scale.** With the YM at true full scale: **1.90×.**

So clipping is not an edge case, and D3 **clamps at the mix, saturating**, and
counts the clamps in the trace. The count is the point: a wrapping mix produces a
loud crack that a listener would blame on the ADPCM decoder, and a silently
clamping mix hides a scaling error that makes everything quiet. A visible clip
counter in the sound panel tells the user which.

Four voices at full volume is unlikely in practice — the driver mixes music and
effects — so the trace counter is expected to stay at or near zero on real content,
and a large count is a finding about D3's scaling, not about the game.

---

## Resampling, and the method chosen with its cost stated

Two inexact input rates onto one host device: YM at 55,930.390625 Hz, OKI at
7,575.7576 or 6,060.6061 Hz, host at whatever it says. **Measured on this machine
2026-08-11:** CoreAudio, "MacBook Pro Speakers", default 48,000 Hz, F32, 2
channels, supporting 44,100 / 48,000 / 88,200 / 96,000.

The chain is: OKI → up to the YM rate → mix → down to the host rate. The OKI is
resampled to the YM's rate rather than each to the host's, for two reasons: the mix
must happen at one rate and the reference mixes at its own internal rate, and it
means the vector suite compares the OKI at *its own* rate with no resampling in the
comparison path at all. A suite that compared post-resample audio would be testing
the resampler and the chip together, and a failure could not be attributed.

**Both stages are linear interpolation**, and the honest statement of what that
costs:

- **OKI → YM, 7.38× upsampling.** Linear interpolation here is not merely
  acceptable, it is arguably more faithful than a sharp filter: the real chip's
  output is a stepped 12-bit DAC at 7.6 kHz feeding an analogue path with no
  reconstruction filter to speak of, and the cabinet's speaker did the rest. A
  windowed-sinc would produce something cleaner than the arcade board.
- **Mix → host, 1.165× downsampling at 48 kHz.** Here linear interpolation is a
  real compromise: it attenuates the top of the band and aliases content above
  24 kHz back down. The YM's output above 20 kHz is low but not zero. The
  alternative is a polyphase FIR, which is a dependency or 200 lines of new
  DSP plus its own verification burden — and the whole of D3's verification budget
  is spent on the chip. **Decision: linear, and the limitation recorded here and in
  the README** so nobody reads "sample-exact" as extending past the mix. The chip
  and the mix are exact; the host output is not, and cannot be, because the host
  rate is not a rational multiple of anything on the board.

The resampler is **not** in the vector comparison path, which is what makes this
tradeoff affordable. `mono` is compared at the OKI's own rate.

---

## Two clocks, and the buffer policy — measured, not designed

The producer runs off the frame pacer's host monotonic clock; the consumer runs off
the audio device's crystal. These are different oscillators and they drift. This is
the part of D3 most likely to be got wrong by reasoning alone, so it was measured
before being specified.

**Probe 1 — callback cadence** (2026-08-11): 512 frames per callback, exactly,
every callback; gap median 10.67 ms, max 10.71 ms; first callback 143 ms after
`play()`. So the device asks for a fixed block and the emulator's ~16.7 ms frames
do not align with it — a burst-producer against a steady consumer.

**Probe 2 — a bounded ring at real cadences**, 10 s, 100 ms cap and 50 ms prefill:
**0 underruns, 0 drops**, depth ranging 29.3–58.7 ms. The audio thread's mutex wait
was **166 ns mean over 282 acquisitions**, which settles the lock question: a
plain `Mutex<VecDeque>` is not a problem at this block size, and a lock-free ring
is complexity D3 does not need.

**Probe 3 — the drift itself.** The first attempt measured +127 ppm, and that
number was discarded: it took the frame count inside the callback and the wall
time outside, so it carried the fixed "frames handed over but not yet played"
offset, worth ±178 ppm at 60 s — larger than the value being measured. Taking both
marks *inside* the callback cancels the offset. Re-measured over **179 s and
16,882 callbacks: +6.3 ppm ± 59.6 ppm** of callback jitter.

The honest reading of that: **the drift is below this method's resolution.** The
bound that matters is the jitter figure, not the point estimate — at worst about
60 ppm, or 3.6 ms of buffer per minute. A 100 ms ring absorbs 25 minutes of
worst-case one-way drift from a 50 ms centre.

So the policy is deliberately dumb, and each part earns its place:

- **A bounded ring, 100 ms, prefilled to 50 ms.** Sized from the measurement:
  the observed depth swing is 29 ms, so 50 ms of headroom either side is ~1.7×
  the measured worst case.
- **On overflow, drop the oldest.** Latency stays bounded; a dropped sample at
  7.6 kHz is inaudible, and unbounded growth is not.
- **On underrun, hold the last sample** rather than emitting zeros. A zero is a
  step to silence and clicks; holding is a DC excursion that does not.
- **Both counted in the trace**, and shown in the sound panel. This is the
  measurement that tells a user their machine cannot keep up, and it is the number
  that makes "the audio is crackly" a diagnosable report instead of a shrug.
- **No clock slewing, no rate feedback.** The measurement does not justify it. If
  a future device drifts enough to matter, the counters are already there to show
  it, and this section is the baseline to compare against.

**Pausing.** A paused emulator produces nothing, so the ring drains and the
underrun counter would climb for as long as the pause lasts, reporting a fault
that is not one. The stream keeps running — tearing it down and rebuilding costs
143 ms — and the loop tells the audio side it is paused so underruns are not
counted, which is a distinction the trace has to draw or the counter is worthless.

---

## Architecture

```
crates/oki/            the MSM6295: four voices, ADPCM state, the phrase
                       table, the volume table, the two divisors. No
                       dependencies. Owns no ROM: `new` takes the region.
crates/machine/        SoundBoard gains the chip; 0xF002 read/write and
                       0xF006 become real. Cps1 mixes to mono and gains the
                       OKI's rate accumulator, advanced in `step_sound` beside
                       the YM's. `samples` becomes mono.
crates/frontend/       the ring buffer, the resampler, and the underrun and
                       clip counters. Decides *what*, as always — it has
                       never heard of an audio device.
crates/sfemu/          `audio.rs`: cpal, and nothing else. The only file
                       that names an audio library, exactly as `display.rs`
                       is the only file that names a windowing one.
crates/testrunner/     genoki, the OKI vector format, the runner, the suite.
```

`crates/oki` is dependency-free and `no_std`-shaped, like `m68k`, `z80` and
`ym2151` — and, per the correction D2's own documentation records, that claim gets
**verified by cross-building for `thumbv7em-none-eabihf`**, not by reading the
`cfg_attr`. A `std::` path compiles fine in a workspace whose default has `std`.

**The `samples` buffer changes shape**, from `Vec<(i16, i16)>` to `Vec<i16>`. This
is a breaking change to `Cps1`'s public API and it touches the overlay's `SMP`
readout and four tests in `cps1.rs`. It is the right change — the board is mono —
and doing it as part of D3 rather than leaving a stereo buffer holding two copies
of the same value is the difference between a mix and a pretence of one.

### The dependency: cpal

`cpal = "0.18"` in `crates/sfemu` only. Measured 2026-08-11: **15 transitive
crates** on macOS (`coreaudio-rs`, `objc2` and friends, `libc`, `bitflags`,
`dasp_sample`), versus `minifb`'s 1. That is a real cost and it is the largest
dependency this workspace has taken. The justification: CoreAudio directly means
`objc2` bindings written by hand and a Windows/Linux path that does not exist, and
`cpal` is the crate the Rust emulator ecosystem uses. It is confined to one file
behind a trait, exactly as `minifb` is, so the loop's decisions stay testable with
a recording fake and replacing it later touches one file.

**Verified working on this machine**, which is not a given for an audio crate: the
probes above opened the default device, enumerated its four supported
configurations, ran a real stream for 179 s, and produced audible output.

### The trait boundary

`loop_.rs`'s `Display` trait has a counterpart: `Audio`, with `queue(&mut self,
samples: &[i16])` and `underruns(&self) -> u64`. The tests get a recording fake and
assert the *decisions* — that a paused frame queues nothing, that a drained ring is
counted, that a reset clears the ring — none of which needs a speaker. This is
E1's own argument, quoted from `loop_.rs:1-19`: "'the right pixels reached the
glass' is not something a test can read back," and neither is "the right samples
reached the speaker."

---

## Save states

`MachineState` gains the OKI's whole state: four voices (playing, base offset,
sample index, count, volume index, and the ADPCM signal and step), the pending
command latch, and the rate accumulator's remainder. `oki_pin7` is already there.

The ADPCM signal and step are the interesting fields, and the reason is D2's
lesson about the YM's envelope: a chip restored with its phrase position but not
its predictor state resumes at the right *place* in the sample and the wrong
*amplitude*, then converges over a few dozen nibbles. That is a click and a wrong
attack, and it would present as a save-state bug rather than as a missing field.
Verified by divergence — restore, run, require the same samples — never by
`snapshot == snapshot`.

**The pending command is state.** A save taken between the two bytes of a command
restores with `command = -1`'s equivalent and the second byte is then interpreted
as a fresh command — plausibly a stop. One instruction in a handful, and it
happens constantly in a driver that plays samples.

**The ring buffer is not state.** It is output in flight, like the framebuffer.

---

## The debugger

The sound panel gains an OKI line: per-voice status, the pin-7 divisor as a *rate*
rather than a bit, the pending command, and the underrun and clip counters.
`SND_HEAD_ROWS` is `9` (`overlay.rs:119`) and is asserted against the rows actually
drawn (`overlay.rs:484`), so it becomes `10` — the panel's box grows with it.

Two rules the existing panel already establishes and this must not break:
`peek_byte`, never the bus, so the panel does not manufacture the fetch counts it
displays (`sound.rs:260-278`); and `draw` takes `&Cps1`, so
`drawing_the_sound_panel_does_not_move_the_counters` keeps holding.

The pin-7 line shows `7576 Hz` or `6061 Hz`, not `OKI7 1`. The bit is what the
board has; the rate is what the user is trying to find out, and it is the form in
which a wrong default is visible at a glance — which is how the `oki_pin7: false`
defect above would have been caught earlier had D2's panel shown it that way.

---

## Error handling

- **No audio device.** The emulator runs, silently, and says so once on stderr.
  A machine with no sound card must not fail to start a game. The `--play` path
  already reports a missing ROM as a `Fault`; a missing speaker is not one.
- **The stream dies mid-session** (device unplugged). `cpal`'s error callback
  records it; the loop keeps running and the summary reports it. Same posture as
  `Display::present` returning `Err`.
- **A phrase with `start >= stop`.** MAME logs and refuses to play. D3 counts it
  in the trace and refuses. This is a driver bug or a ROM problem, and counting it
  is how it becomes visible rather than inaudible.
- **A command for an already-playing voice.** Ignored, per MAME. Counted, because
  a large count means the driver's voice allocation is not doing what D3 thinks.
- **A phrase pointing past the ROM region.** `get()`, never an index — `romset`
  fills 0x40000 but a user's set could be short. Reads as `0x00`, like
  `SoundBoard::rom_byte`'s `None` path.
- **Missing vector data.** Fails loudly naming the file and
  `cargo run -q -p testrunner --release --bin genoki`. No `#[ignore]`, no env-var
  escape.

---

## Definition of done for D3

1. `cargo test --workspace` and `--release` green;
   `cargo clippy --all-targets --all-features -- -D warnings` clean;
   `cargo doc --no-deps --workspace` clean.
2. The OKI suite at **1,000/1,000 cases**, exact on all four fields — `mono`,
   `status`, `voices` and `nibbles` — reported as `cases: 1000/1000`.
3. The suite's discriminating power asserted as tests on the generated data: all
   four voices simultaneously, a silent volume index producing exact zero, natural
   end and truncated stop, an ignored command for a playing voice, both pin-7
   divisors, and the step index reaching both 0 and 48 — the latter from the
   deliberate segment, not from random data, for the measured reason above. Plus
   the runner's own failure test, corrupting each of the four fields in turn.
4. The three existing suites unmoved: **127/127** (317,500/317,500),
   **1,604/1,604** (1,604,000/1,604,000), **1,000/1,000**. D3 changes `machine`
   and `Cps1`'s public API; movement is a regression to investigate, not a
   tolerance.
5. The mutation pass at 100% as-expected, every survivor a declared control or a
   **proven** equivalent, with new sets for the ADPCM decoder, the command
   protocol, the mono mix, and the ring buffer. Run with `--all`, not one set:
   D2 found a control that a later task had silently turned into a real mutant,
   and only the full run catches that.
6. `crates/oki` has no dependencies, and **builds for
   `thumbv7em-none-eabihf`** — verified by building, not by reading the attribute.
   `cpal` appears in `crates/sfemu/Cargo.toml` and nowhere else, enforced by a
   source-walking test in the shape of
   `the_windowing_library_is_named_in_one_file` (`display.rs:192`), which asserts
   its own walk found the tree.
7. The integer volume path proven bit-exact against MAME's float path over all
   16 × 4,096 cases — the full identity, not a sample.
8. `oki_pin7` defaults to **high** in `SoundBoard::new` and after `Cps1::reset`,
   asserted through the divisor it implies rather than through the boolean.
9. The mix clamps rather than wraps, and the clamp count is in the trace and on
   the panel.
10. The ring's underrun and drop counts are in the trace and on the panel, and a
    paused emulator does not accrue underruns.
11. **Sound comes out.** With a user-supplied ROM set, `--play` produces audio:
    asserted as far as a test can — that samples were queued and the underrun
    count stayed bounded over a number of frames — in a test that fails loudly
    with the ROM absent, the third documented exception alongside
    `crates/sfemu/tests/boot.rs` and `sound_boot.rs`.

**Item 11 is the end of "there is still no sound."** Whether it sounds *right* is
item 8 of the README's "things only you can check" and needs a human with the ROM
set and a speaker — no test in this repository can make that claim, and none will
pretend to.

## Risks

- **The wrapper is my transcription, not MAME's code.** D2 compared against a
  reference that compiled whole; here only the decoder does. The protocol logic in
  the generator is transcribed from `okim6295.cpp` and could be wrong in the same
  way in both the generator and the Rust chip, in which case the suite passes at
  1,000/1,000 and the emulator is wrong. This is the weakest link in D3 and is stated as
  such. Partial mitigations: the protocol is small and quoted line-by-line above;
  the two implementations are written in different languages from the same source
  rather than one being a port of the other; and the real-ROM trace test
  corroborates the parts the driver exercises. It is not eliminated.
- **The mono downgrade will look like a bug.** Someone will hear centre-panned
  audio and file it. Hence the explicit section, the README note, and the
  `cps1.cpp:3935` citation at the mix site.
- **Linear resampling to the host rate is a real compromise**, unlike the
  upsampling stage. Recorded rather than hidden: the chip and the mix are exact,
  the host output is not.
- **The drift measurement is a bound, not a value.** +6.3 ppm ± 59.6 ppm on one
  machine, one device, over 179 s. Another machine could be worse, and the ring is
  sized against the jitter bound rather than the point estimate for that reason.
  The counters exist so a user's report is diagnosable.
- **The `samples` API change touches four tests and the overlay.** Mechanical, but
  it is the kind of change where a test gets adjusted to match new behaviour
  instead of being asked whether the new behaviour is right. Each adjusted
  assertion gets a note saying why mono is the correct expectation.
- **Audio makes the emulator's timing audible for the first time.** Frame-pacing
  imperfections that were invisible become clicks. This is not a new defect; it is
  a new instrument, and it may well find something in E1's pacer. That is a good
  outcome and possibly a follow-up sub-project.

## Sources

- MAME `src/devices/sound/okiadpcm.cpp` and `okiadpcm.h` (BSD-3-Clause,
  copyright-holders Andrew Gardner, Aaron Giles) — **fetched, compiled and run
  standalone on this machine 2026-08-11**. `s_index_shift` at `okiadpcm.cpp:14`;
  `compute_tables` with the `nbl2bit` decomposition and
  `floor(16.0 * pow(11.0/10.0, step))`; `clock()` with the ±2047/−2048 signal clamp
  and the 0..48 step clamp; `reset()` zeroing signal and step. The step-index
  reachability figures in "The vector suite" are from a probe against this file.
- MAME `src/devices/sound/okim6295.cpp` and `okim6295.h` (BSD-3-Clause) — read for
  the protocol, **not compilable standalone** (it is a `device_t`). Citations used
  above: `s_volume_table` at `:59-77`; `read()` at `:214-224` with the `0xf0` base;
  `write()` at `:228-284` — the two-byte command, the `phrase * 8` table, the
  `0x3ffff` masks, `count = 2 * (stop - start + 1)`, the already-playing skip, the
  `start >= stop` rejection, and the `>> 3` stop mask; `generate_adpcm()` at
  `:334-357` with the `>> (((m_sample & 1) << 2) ^ 4)` nibble fetch and the
  unclamped four-voice `add_int`; `device_reset()` at `:143-148` and
  `device_clock_changed()` at `:166-170` for the `132`/`165` divisors and the fact
  that a reset does **not** clear pin 7.
- MAME `src/mame/capcom/cps1.cpp` (BSD-3-Clause, copyright-holders Paul Leaman):
  `SPEAKER(config, "mono").front_center()` at 3935; the YM's two 0.35 routes at
  3942-3943; `OKIM6295(config, m_oki, XTAL(16'000'000)/4/4,
  okim6295_device::PIN7_HIGH).add_route(ALL_OUTPUTS, "mono", 0.30)` at 3946 with
  its "pin 7 can be changed by the game code" comment; `cps1_oki_pin7_w` at
  297-300; `sub_map` at 631-642 for `0xF002` being read/write.
- ymfm (`https://github.com/aaronsgiles/ymfm`), BSD-3-Clause, © 2021 Aaron Giles —
  checked 2026-08-11 and **confirmed to contain no MSM6295**, which is why D2's
  ground truth cannot serve D3.
- This machine, 2026-08-11: `cpal` 0.18.1 on CoreAudio — device enumeration, the
  512-frame callback block, the 179-second drift measurement, and the ring-buffer
  probe. Numbers quoted in "Two clocks" and "Resampling" are from those runs.
- `crates/machine/src/timing.rs` for `RationalAccumulator` and the exactness
  assertions the OKI's 64-clocks-per-line result is asserted beside;
  `crates/sfemu/src/loop_.rs:1-19` for the trait-boundary argument the `Audio`
  trait reuses; `crates/sfemu/src/display.rs:192` for the source-walking test
  shape that confines `cpal` to one file.
- Oki Semiconductor MSM6295 datasheet for the pin-7 divisors, the four-voice
  architecture and the 4-bit ADPCM format.
