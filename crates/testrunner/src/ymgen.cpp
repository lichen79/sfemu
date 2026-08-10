// Generates the AYMV vector file from ymfm, the implementation MAME uses.
//
// This file is BUILT against ymfm; no ymfm source is vendored in this repository.
// `crates/testrunner/src/ymfm.rs` fetches the archive, verifies it, and invokes c++.
//
// No ROM is involved. ymfm is BSD-3 reference *code* (c) 2021 Aaron Giles; the only
// thing this program reads is a seed and the only thing it writes is samples.
//
// THE REGISTER SCRIPT IS STRUCTURED, NOT RANDOM. Three measurements forced this:
//   * A purely random script is silent: 0 of 500 cases produced one non-zero
//     sample. Every case must set up a playable patch and key on.
//   * A held note never exercises release rate: RR bit 0 was undetected in 0 of
//     200 cases until every case keys OFF at sample 256 of 512.
//   * Timer state is not audible: undetected in 0 of 200 until the record gained
//     a per-sample status byte.
//
// AND: CSM. The lazy prepare() gate consumes the CSM key-on flag, so a Rust port
// that prepares eagerly is wrong ONLY in CSM mode. Cases where `seed % 8 == 0`
// enable 0x14 bit 7 with timer A running, and the host below FIRES the timers.
// Without a firing host the whole comparison is vacuous — measured: CSM-on and
// CSM-off hashes came out identical until engine_timer_expired was actually
// called.

#include "ymfm_opm.h"

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace {

// One YM2151 sample is 32 operators x prescale 2 = 64 input clocks. ymfm hands the
// host timer durations in input clocks, so the host below works in those units and
// this is the conversion.
constexpr int64_t CLOCKS_PER_SAMPLE = 64;

// Samples per case, and the sample the key-off lands on. Both must match
// `ymfiles::SAMPLES_PER_CASE` and the runner's expectation.
constexpr int SAMPLES = 512;
constexpr int KEY_OFF_AT = 256;

struct Host : public ymfm::ymfm_interface {
    int64_t deadline[2] = {-1, -1};
    int64_t now = 0;
    bool irq = false;

    void ymfm_set_timer(uint32_t tnum, int32_t duration) override {
        deadline[tnum] = (duration < 0) ? -1 : now + duration;
    }
    void ymfm_update_irq(bool asserted) override { irq = asserted; }

    // Advances `clocks` input clocks, firing each timer that comes due inside the
    // window in chronological order. THIS is what makes CSM cases meaningful.
    void advance(int64_t clocks) {
        int64_t end = now + clocks;
        for (;;) {
            int best = -1;
            for (int t = 0; t < 2; t++)
                if (deadline[t] >= 0 && deadline[t] <= end &&
                    (best < 0 || deadline[t] < deadline[best]))
                    best = t;
            if (best < 0) break;
            now = deadline[best];
            deadline[best] = -1;
            m_engine->engine_timer_expired(best);
        }
        now = end;
    }
};

// xorshift64. The whole script is a function of the seed, so a failing case is
// reproducible from one integer and the file needs no script of its own.
struct Rng {
    uint64_t s;
    explicit Rng(uint32_t seed) : s(0x9E3779B97F4A7C15ULL ^ (uint64_t(seed) + 1)) {
        // Two warm-up rounds: xorshift64 seeded from a small integer correlates in
        // its first output, and case 0's algorithm choice would track its index.
        next();
        next();
    }
    uint64_t next() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        return s;
    }
    // Uniform in [0, n).
    uint32_t below(uint32_t n) { return uint32_t(next() % n); }
    // Inclusive range.
    uint32_t range(uint32_t lo, uint32_t hi) { return lo + below(hi - lo + 1); }
};

struct Write {
    uint16_t at_sample;
    uint8_t reg;
    uint8_t val;
};

struct Sample {
    int16_t left;
    int16_t right;
    uint8_t status;
};

// Builds one case's register script. Returns the writes in emission order, which is
// also `at_sample` order because everything but the key-off happens at sample 0.
std::vector<Write> build_script(uint32_t seed) {
    Rng rng(seed);
    std::vector<Write> w;
    auto put = [&](uint16_t at, uint8_t reg, uint8_t val) {
        w.push_back({at, reg, val});
    };

    // 1-3 channels, so the suite covers a single voice and the mixing of several.
    // Channel 7 is always among them when noise is on, since that is the only
    // channel the noise generator can reach.
    const bool noise = (seed % 7) == 0;
    const bool lfo = (seed % 5) == 0;
    const bool csm = (seed % 8) == 0;

    int nchan = int(rng.range(1, 3));
    uint8_t chans[3];
    for (int i = 0; i < nchan; i++) chans[i] = uint8_t(rng.below(8));
    if (noise) chans[0] = 7;

    for (int i = 0; i < nchan; i++) {
        const uint8_t ch = chans[i];
        const uint8_t alg = uint8_t(rng.below(8));
        const uint8_t fb = uint8_t(rng.below(8));
        // Both pans on: a channel with neither takes ymfm's early-out and
        // contributes nothing, which would waste the case.
        put(0, uint8_t(0x20 + ch), uint8_t(0xC0 | (fb << 3) | alg));
        // Key code and key fraction. The whole 8-octave range, so the phase-step
        // table and the detune-by-key-code path are both exercised.
        put(0, uint8_t(0x28 + ch), uint8_t(rng.below(0x80)));
        put(0, uint8_t(0x30 + ch), uint8_t(rng.below(0x40) << 2));
        // PM and AM sensitivity. Only meaningful with the LFO on, but written
        // either way so a port that reads them unconditionally is still compared.
        put(0, uint8_t(0x38 + ch), uint8_t((rng.below(8) << 4) | rng.below(4)));

        for (int op = 0; op < 4; op++) {
            const uint8_t off = uint8_t(op * 8 + ch);
            // Detune 1, multiple. Multiple 0 is a half-step and is included.
            put(0, uint8_t(0x40 + off), uint8_t((rng.below(8) << 4) | rng.below(16)));
            // Total level. Capped at 0x30 rather than 0x7F: past about 0x40 a
            // carrier is below the DAC's own quantisation and the case goes silent,
            // which is the measured failure this cap exists to avoid.
            put(0, uint8_t(0x60 + off), uint8_t(rng.below(0x31)));
            // Key scale and attack rate. AR is drawn from 20-31 so the envelope
            // reaches its peak well inside 256 samples; the measured alternative is
            // a case whose attack has not begun when the key-off arrives.
            put(0, uint8_t(0x80 + off), uint8_t((rng.below(4) << 6) | rng.range(20, 31)));
            // AM enable and decay rate 1.
            put(0, uint8_t(0xA0 + off), uint8_t((lfo ? 0x80 : 0x00) | rng.below(32)));
            // Detune 2 and decay rate 2.
            put(0, uint8_t(0xC0 + off), uint8_t((rng.below(4) << 6) | rng.below(32)));
            // Sustain level and RELEASE RATE. RR is 1-15, never 0: rate 0 is an
            // infinite release, so a zero here would make the key-off inaudible and
            // undo the measurement that put the key-off there.
            put(0, uint8_t(0xE0 + off), uint8_t((rng.below(16) << 4) | rng.range(1, 15)));
        }
    }

    if (noise) {
        // Noise enable plus a frequency. Frequency 0 is the slowest and is allowed.
        put(0, 0x0F, uint8_t(0x80 | rng.below(32)));
    }

    if (lfo) {
        put(0, 0x18, uint8_t(rng.below(256)));            // LFO rate
        put(0, 0x19, uint8_t(rng.range(1, 0x7F)));        // AM depth, never 0
        put(0, 0x19, uint8_t(0x80 | rng.range(1, 0x7F))); // PM depth, never 0
        put(0, 0x1B, uint8_t(rng.below(4)));              // waveform
    }

    // Timer values are written for every case, so the timer registers are compared
    // even when nothing is loaded. Timer A is drawn short enough to fire several
    // times inside 512 samples: period is 1024 - value in samples, so a value above
    // 900 gives at most 124 samples and at least 4 overflows.
    // uint16_t, not uint8_t: timer A's value is 10 bits, and truncating it to 8
    // silently maps 900-1023 onto 132-255, whose period of 769-892 samples never
    // overflows inside a 512-sample window. That was measured — it left CSM cases
    // with no key-on at all, the exact vacuity this file's header warns about.
    const uint16_t ta = uint16_t(rng.range(900, 1023));
    put(0, 0x10, uint8_t(ta >> 2));
    put(0, 0x11, uint8_t(ta & 3));
    // Timer B's period is 16 * (256 - value), so it needs a high value to fire at
    // all inside the window: 240 gives 256 samples, exactly one overflow.
    put(0, 0x12, uint8_t(rng.range(240, 255)));

    // The mode byte. CSM cases load and enable timer A; the rest load both timers
    // for a third of cases so the status trace is not always empty.
    uint8_t mode = 0;
    if (csm) {
        mode = 0x80 | 0x05; // CSM, load A, enable A
    } else if ((seed % 3) == 0) {
        mode = 0x0F; // load and enable both
    }
    put(0, 0x14, mode);

    // Key on every operator of every channel, then key OFF at sample 256. The
    // key-off is unconditional: it is the only thing that reaches release rate.
    for (int i = 0; i < nchan; i++) put(0, 0x08, uint8_t(0x78 | chans[i]));
    for (int i = 0; i < nchan; i++) put(KEY_OFF_AT, 0x08, uint8_t(chans[i]));

    return w;
}

void put_u16(std::vector<uint8_t> &o, uint16_t v) {
    o.push_back(uint8_t(v & 0xFF));
    o.push_back(uint8_t(v >> 8));
}

void put_u32(std::vector<uint8_t> &o, uint32_t v) {
    for (int i = 0; i < 4; i++) o.push_back(uint8_t((v >> (8 * i)) & 0xFF));
}

} // namespace

int main(int argc, char **argv) {
    if (argc != 3) {
        std::fprintf(stderr, "usage: ymgen <num_cases> <out.aymv>\n");
        return 2;
    }
    const int num_cases = std::atoi(argv[1]);
    if (num_cases <= 0) {
        std::fprintf(stderr, "num_cases must be positive, got %s\n", argv[1]);
        return 2;
    }

    std::vector<uint8_t> out;
    put_u32(out, 0x564D5941u); // 'A','Y','M','V' in file order
    put_u32(out, uint32_t(num_cases));

    // Statistics, printed at the end. The Rust driver asserts on them: a generator
    // that silently regressed to silence must not produce a passing suite.
    int64_t nonzero_samples = 0;
    int64_t status_set_samples = 0;
    int cases_with_sound = 0;
    int cases_with_status = 0;
    int cases_with_release_change = 0;
    size_t max_writes = 0;
    int64_t total_writes = 0;

    for (int ci = 0; ci < num_cases; ci++) {
        const uint32_t seed = uint32_t(ci);
        std::vector<Write> script = build_script(seed);
        if (script.size() > 0xFFFF) {
            std::fprintf(stderr, "case %d has %zu writes, over u16\n", ci, script.size());
            return 1;
        }
        max_writes = script.size() > max_writes ? script.size() : max_writes;
        total_writes += int64_t(script.size());

        Host host;
        ymfm::ym2151 chip(host);
        chip.reset();

        std::vector<Sample> samples;
        samples.reserve(SAMPLES);
        size_t next_write = 0;
        ymfm::ym2151::output_data frame;
        bool any_sound = false;
        bool any_status = false;
        // Peak over the two halves separately: a release that changed nothing means
        // the key-off did not take, which is the measurement the whole layout rests
        // on. Compared per case rather than in aggregate so one loud case cannot
        // hide 999 silent ones.
        int peak_before = 0, peak_after = 0;

        for (int i = 0; i < SAMPLES; i++) {
            while (next_write < script.size() && script[next_write].at_sample == i) {
                chip.write_address(script[next_write].reg);
                chip.write_data(script[next_write].val);
                next_write++;
            }
            // The timers advance across the sample the chip is about to generate, so
            // a CSM key-on lands before the operators are prepared for it.
            host.advance(CLOCKS_PER_SAMPLE);
            chip.generate(&frame, 1);
            const uint8_t status = chip.read_status();
            samples.push_back({int16_t(frame.data[0]), int16_t(frame.data[1]), status});

            const int mag = frame.data[0] < 0 ? -frame.data[0] : frame.data[0];
            if (frame.data[0] != 0 || frame.data[1] != 0) {
                nonzero_samples++;
                any_sound = true;
            }
            if (status != 0) {
                status_set_samples++;
                any_status = true;
            }
            if (i < KEY_OFF_AT) {
                peak_before = mag > peak_before ? mag : peak_before;
            } else {
                peak_after = mag > peak_after ? mag : peak_after;
            }
        }
        if (next_write != script.size()) {
            std::fprintf(stderr, "case %d: %zu writes never applied\n", ci,
                         script.size() - next_write);
            return 1;
        }

        if (any_sound) cases_with_sound++;
        if (any_status) cases_with_status++;
        if (peak_before != peak_after) cases_with_release_change++;

        put_u32(out, seed);
        put_u16(out, uint16_t(script.size()));
        for (const Write &w : script) {
            put_u16(out, w.at_sample);
            out.push_back(w.reg);
            out.push_back(w.val);
        }
        put_u16(out, uint16_t(samples.size()));
        for (const Sample &s : samples) {
            put_u16(out, uint16_t(s.left));
            put_u16(out, uint16_t(s.right));
            out.push_back(s.status);
        }
        out.push_back(samples.empty() ? 0 : samples.back().status);
    }

    std::FILE *f = std::fopen(argv[2], "wb");
    if (!f) {
        std::fprintf(stderr, "cannot write %s\n", argv[2]);
        return 1;
    }
    if (std::fwrite(out.data(), 1, out.size(), f) != out.size()) {
        std::fprintf(stderr, "short write to %s\n", argv[2]);
        std::fclose(f);
        return 1;
    }
    std::fclose(f);

    // One machine-readable line per statistic, for the Rust driver to assert on.
    std::printf("cases %d\n", num_cases);
    std::printf("bytes %zu\n", out.size());
    std::printf("max_writes %zu\n", max_writes);
    std::printf("mean_writes %lld\n", (long long)(total_writes / num_cases));
    std::printf("cases_with_sound %d\n", cases_with_sound);
    std::printf("cases_with_status %d\n", cases_with_status);
    std::printf("cases_with_release_change %d\n", cases_with_release_change);
    std::printf("nonzero_samples %lld\n", (long long)nonzero_samples);
    std::printf("status_set_samples %lld\n", (long long)status_set_samples);
    return 0;
}
