// Generates the AOKV OKI vector suite.
//
// The ADPCM arithmetic is MAME's okiadpcm.cpp, unmodified -- this file is
// compiled against it, so the decoder here is not a transcription of anything.
//
// The four-voice protocol *is* a transcription, from okim6295.cpp, which cannot
// be compiled standalone because it is a device_t. That transcription is this
// suite's weakest link: it and crates/oki/src/chip.rs are the same reading of
// the same file, so a misreading would agree with itself and the suite would
// pass. The premise tests in crates/testrunner/tests/okisuite.rs are the check
// on that, and every deliberate fixture choice below is written down so a
// reader can tell a premise from an accident.
#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <vector>
#include "okiadpcm.h"

static const int VOLT[16] = {0x20, 0x16, 0x10, 0x0b, 0x08, 0x06, 0x04, 0x03, 0x02, 0, 0, 0, 0, 0, 0, 0};
static const long CLAMP2X = 65536;
static const int VOICES = 4;
static const size_t ROM_BYTES = 0x40000;
static const int SAMPLES = 512;
static const int CASES = 1000;

// Three phrases are reserved in every case, because each one pins down a
// premise that random data does not reach.
//
// Phrase 1 -- the step ladder. 16 bytes of nibble 7 then 48 of nibble 0, which
// is 32 sevens then 96 zeros. Measured against MAME's own decoder
// (target/okigen/ladderprobe.cpp): step 48 is first reached at sample 5 and
// held for 27 samples, step 0 at sample 79 and held for 49. Both step clamps
// are *held*, not merely touched, and the segment is audible -- total |signal|
// over the 128 samples is 253808, so it is not a silent ramp. Random nibbles
// reach step 1..48 only and never 0, so without this the lower step clamp is
// untested.
static const uint32_t LADDER_START = 0x400;
static const uint32_t LADDER_END = 0x440;  // exclusive; 64 bytes = 128 nibbles

// Phrase 3 -- the last 64 bytes of the ROM, so the top of the 18-bit address
// bus is walked in every case. A core that masked addresses with 0x1FFFF
// instead of 0x3FFFF would fold these reads onto a different byte and diverge
// here; nothing else in the fixture reaches this high. Started on voice 1 at
// sample 0 alongside the ladder, which also puts two voices at unity gain over
// the first 128 samples -- that is what drives the chip's own +-65536 clamp.
static const uint32_t TOP_START = 0x3FFC0;
static const uint32_t TOP_STOP = 0x3FFFF;

// Phrase 2 -- a normal phrase, except in every fourth case where its start and
// stop are swapped so that `start < stop` fails and the phrase is refused. Such
// a case also issues the refused start explicitly (see below) rather than
// hoping a random event picks phrase 2.
static const int REFUSED_PHRASE = 2;

struct Voice {
    oki_adpcm_state adpcm;
    bool playing = false;
    uint32_t base = 0, sample = 0, count = 0;
    int volume = 0;
};

struct Oki {
    std::vector<uint8_t> rom;
    Voice v[VOICES];
    int command = -1;

    uint8_t rd(uint32_t a) const { return a < rom.size() ? rom[a] : 0; }
    uint32_t rd24(uint32_t a) const {
        return ((uint32_t)rd(a) << 16 | (uint32_t)rd(a + 1) << 8 | rd(a + 2)) & 0x3ffff;
    }
    uint8_t status() const {
        uint8_t r = 0xf0;
        for (int i = 0; i < VOICES; i++)
            if (v[i].playing) r |= 1u << i;
        return r;
    }
    void write(uint8_t c) {
        if (command != -1) {
            int mask = c >> 4;
            for (int i = 0; i < VOICES; i++, mask >>= 1) {
                if (!(mask & 1)) continue;
                Voice &vo = v[i];
                if (vo.playing) continue;  // "fixes Got-cha and Steel Force"
                uint32_t base = (uint32_t)command * 8;
                uint32_t start = rd24(base), stop = rd24(base + 3);
                if (start < stop) {
                    vo.playing = true;
                    vo.base = start;
                    vo.sample = 0;
                    vo.count = 2 * (stop - start + 1);
                    vo.adpcm.reset();
                    vo.volume = VOLT[c & 0x0f];
                }
            }
            command = -1;
        } else if (c & 0x80) {
            command = c & 0x7f;
        } else {
            int mask = c >> 3;
            for (int i = 0; i < VOICES; i++, mask >>= 1)
                if (mask & 1) v[i].playing = false;
        }
    }
    long step(uint8_t &voices, uint16_t &nibbles) {
        long sum = 0;
        voices = 0;
        nibbles = 0;
        for (int i = 0; i < VOICES; i++) {
            Voice &vo = v[i];
            if (!vo.playing) continue;
            voices |= 1u << i;
            uint32_t a = (vo.base + vo.sample / 2) & 0x3ffff;
            int nib = (rd(a) >> (((vo.sample & 1) << 2) ^ 4)) & 0x0f;
            nibbles |= (uint16_t)nib << (4 * i);
            sum += (long)vo.adpcm.clock((uint8_t)nib) * vo.volume;
            if (++vo.sample >= vo.count) vo.playing = false;
        }
        return std::max(-CLAMP2X, std::min(CLAMP2X, sum));
    }
};

static uint64_t rng_state;
static uint32_t rnd() {
    rng_state ^= rng_state << 13;
    rng_state ^= rng_state >> 7;
    rng_state ^= rng_state << 17;
    return (uint32_t)(rng_state >> 11);
}

static void put8(std::vector<uint8_t> &o, uint8_t v) { o.push_back(v); }
static void put16(std::vector<uint8_t> &o, uint16_t v) {
    o.push_back(v & 0xff);
    o.push_back(v >> 8);
}
static void put32(std::vector<uint8_t> &o, uint32_t v) {
    for (int i = 0; i < 4; i++) o.push_back((v >> (8 * i)) & 0xff);
}

// A phrase-table entry: 3-byte big-endian start then stop, at phrase * 8.
static void put_phrase(std::vector<uint8_t> &rom, int phrase, uint32_t start, uint32_t stop) {
    uint32_t a = (uint32_t)phrase * 8;
    rom[a + 0] = start >> 16;
    rom[a + 1] = start >> 8;
    rom[a + 2] = start;
    rom[a + 3] = stop >> 16;
    rom[a + 4] = stop >> 8;
    rom[a + 5] = stop;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: okigen <out-file>\n");
        return 2;
    }
    std::vector<uint8_t> out;
    put32(out, 0x564b4f41);  // AOKV
    put32(out, CASES);

    for (int c = 0; c < CASES; c++) {
        rng_state = 0x9e3779b97f4a7c15ull ^ (uint64_t)(c + 1) * 0x100000001b3ull;
        Oki o;
        o.rom.assign(ROM_BYTES, 0);

        // A phrase table of up to 32 entries. Phrases 4..phrases are random, so
        // short and long phrases both appear; 1, 2 and 3 are the reserved ones
        // described at the top of this file.
        int phrases = 8 + (int)(rnd() % 25);
        for (int p = 4; p <= phrases; p++) {
            uint32_t start = LADDER_END + (rnd() % 0x39000);
            uint32_t len = 1 + (rnd() % 0x300);
            put_phrase(o.rom, p, start, start + len);
        }
        {
            uint32_t start = LADDER_END + (rnd() % 0x39000);
            uint32_t stop = start + 1 + (rnd() % 0x300);
            if (c % 4 == 3) std::swap(start, stop);
            put_phrase(o.rom, REFUSED_PHRASE, start, stop);
        }
        put_phrase(o.rom, 1, LADDER_START, LADDER_END - 1);
        put_phrase(o.rom, 3, TOP_START, TOP_STOP);

        // Sample data. The fill starts past the ladder so the ladder's own bytes
        // stay deliberate; the phrase table lives below LADDER_START and is not
        // touched either.
        for (size_t a = LADDER_END; a < ROM_BYTES; a++) o.rom[a] = (uint8_t)rnd();
        for (uint32_t a = LADDER_START; a < LADDER_START + 16; a++) o.rom[a] = 0x77;
        for (uint32_t a = LADDER_START + 16; a < LADDER_END; a++) o.rom[a] = 0x00;

        struct W {
            uint16_t at;
            uint8_t byte;
        };
        std::vector<W> writes;
        auto start_voice = [&](uint16_t at, int voice, int phrase, int vol) {
            writes.push_back({at, (uint8_t)(0x80 | phrase)});
            // Note that for voice 3 this data byte has bit 7 set: a chip that
            // tested bit 7 before the pending command would read it as a latch.
            writes.push_back({at, (uint8_t)((1 << (voice + 4)) | vol)});
        };
        auto stop_voice = [&](uint16_t at, int voice) {
            writes.push_back({at, (uint8_t)(1u << (voice + 3))});
        };

        // Sample 0: the ladder on voice 0 and the top-of-ROM phrase on voice 1,
        // both at unity gain. No case is silent, every case walks the whole step
        // range, every case reads the top of the address bus, and two voices at
        // unity is what reaches the chip's clamp.
        start_voice(0, 0, 1, 0);
        start_voice(0, 1, 3, 0);
        // Every fourth case exercises the refused phrase deliberately: stop
        // voice 3 first, so the refusal is what leaves it silent rather than the
        // already-playing skip.
        if (c % 4 == 3) {
            stop_voice(1, 3);
            start_voice(1, 3, REFUSED_PHRASE, 0);
        }
        int events = 6 + (int)(rnd() % 12);
        for (int e = 0; e < events; e++) {
            // Samples 0 and 1 are reserved for the deterministic openers and the
            // refusal above, so a random event cannot land on top of them and
            // undo a premise the validator then checks for.
            uint16_t at = (uint16_t)(2 + rnd() % (SAMPLES - 2));
            uint32_t kind = rnd() % 10;
            int voice = (int)(rnd() % VOICES);
            if (kind < 6) {
                // Volume index 0..15, so the silent indices 9..15 appear too.
                start_voice(at, voice, 1 + (int)(rnd() % phrases), (int)(rnd() % 16));
            } else if (kind < 8) {
                // Never voice 0: it runs the ladder, and stopping it early would
                // truncate the one segment that reaches step 0.
                stop_voice(at, 1 + (int)(rnd() % 3));
            } else {
                // All four at once, at a loud volume index: the other way the
                // clamp is reached, and the only thing that starts four voices
                // from one command byte.
                writes.push_back({at, (uint8_t)(0x80 | (1 + (rnd() % phrases)))});
                writes.push_back({at, (uint8_t)(0xF0 | (rnd() % 3))});
            }
        }
        // Stable, so the two bytes of a start stay adjacent and in order.
        std::stable_sort(writes.begin(), writes.end(),
                         [](const W &a, const W &b) { return a.at < b.at; });

        put32(out, (uint32_t)c);        // seed == index
        put8(out, (uint8_t)(c % 2));    // pin7 alternates
        put16(out, (uint16_t)writes.size());
        put16(out, (uint16_t)SAMPLES);
        put32(out, (uint32_t)ROM_BYTES);
        for (const W &w : writes) {
            put16(out, w.at);
            put8(out, w.byte);
        }
        out.insert(out.end(), o.rom.begin(), o.rom.end());

        size_t wi = 0;
        for (int s = 0; s < SAMPLES; s++) {
            while (wi < writes.size() && writes[wi].at == (uint16_t)s) o.write(writes[wi++].byte);
            uint8_t voices;
            uint16_t nibbles;
            long mono = o.step(voices, nibbles);
            put32(out, (uint32_t)(int32_t)mono);
            put8(out, o.status());
            put8(out, voices);
            put16(out, nibbles);
        }
    }

    FILE *f = fopen(argv[1], "wb");
    if (!f) {
        fprintf(stderr, "cannot write %s\n", argv[1]);
        return 1;
    }
    size_t wrote = fwrite(out.data(), 1, out.size(), f);
    if (wrote != out.size() || fclose(f) != 0) {
        fprintf(stderr, "short write to %s\n", argv[1]);
        return 1;
    }
    fprintf(stderr, "wrote %zu bytes, %d cases\n", out.size(), CASES);
    return 0;
}
