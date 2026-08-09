//! The OPM's four lookup tables, split by provenance.
//!
//! Two are exact closed forms and two are chip data, and treating them alike is how
//! a plausible wrong entry survives. Each is gated by a checksum measured against
//! ymfm on 2026-08-09, because 768 transcribed numbers cannot be proofread.
//!
//! - [`SIN`] and [`POW`] are formulas, verified at 0 of 256 mismatched each. They
//!   are transcribed rather than computed only because floating point is not
//!   available in a `const fn`; the closed-form test is what makes that safe.
//! - [`PHASE_STEP`] is David Viens' analysis of real chip data
//!   (`ymfm_fm.ipp:230`), which ymfm's own comment notes "differs in a few spots"
//!   from the computed table. No formula reproduces it: see this module's
//!   `the_phase_step_table_is_not_an_exponential` test.
//! - [`INCREMENT`] is 64 packed-nibble values (`ymfm_fm.ipp:147`).
//! - [`DETUNE`] is the DT1 table (`ymfm_fm.ipp:181`), chip data indexed by key code.

/// Sine as attenuation, 4.8 fixed point, one quarter period.
///
/// `round(-log2(sin((i + 0.5) * PI / 512)) * 256)`, exact at 0 of 256 mismatched.
/// The mirror and the sign live in [`abs_sin_attenuation`], not in the table.
pub static SIN: [u16; 256] = [
    0x859, 0x6c3, 0x607, 0x58b, 0x52e, 0x4e4, 0x4a6, 0x471, 0x443, 0x41a, 0x3f5, 0x3d3, 0x3b5,
    0x398, 0x37e, 0x365, 0x34e, 0x339, 0x324, 0x311, 0x2ff, 0x2ed, 0x2dc, 0x2cd, 0x2bd, 0x2af,
    0x2a0, 0x293, 0x286, 0x279, 0x26d, 0x261, 0x256, 0x24b, 0x240, 0x236, 0x22c, 0x222, 0x218,
    0x20f, 0x206, 0x1fd, 0x1f5, 0x1ec, 0x1e4, 0x1dc, 0x1d4, 0x1cd, 0x1c5, 0x1be, 0x1b7, 0x1b0,
    0x1a9, 0x1a2, 0x19b, 0x195, 0x18f, 0x188, 0x182, 0x17c, 0x177, 0x171, 0x16b, 0x166, 0x160,
    0x15b, 0x155, 0x150, 0x14b, 0x146, 0x141, 0x13c, 0x137, 0x133, 0x12e, 0x129, 0x125, 0x121,
    0x11c, 0x118, 0x114, 0x10f, 0x10b, 0x107, 0x103, 0x0ff, 0x0fb, 0x0f8, 0x0f4, 0x0f0, 0x0ec,
    0x0e9, 0x0e5, 0x0e2, 0x0de, 0x0db, 0x0d7, 0x0d4, 0x0d1, 0x0cd, 0x0ca, 0x0c7, 0x0c4, 0x0c1,
    0x0be, 0x0bb, 0x0b8, 0x0b5, 0x0b2, 0x0af, 0x0ac, 0x0a9, 0x0a7, 0x0a4, 0x0a1, 0x09f, 0x09c,
    0x099, 0x097, 0x094, 0x092, 0x08f, 0x08d, 0x08a, 0x088, 0x086, 0x083, 0x081, 0x07f, 0x07d,
    0x07a, 0x078, 0x076, 0x074, 0x072, 0x070, 0x06e, 0x06c, 0x06a, 0x068, 0x066, 0x064, 0x062,
    0x060, 0x05e, 0x05c, 0x05b, 0x059, 0x057, 0x055, 0x053, 0x052, 0x050, 0x04e, 0x04d, 0x04b,
    0x04a, 0x048, 0x046, 0x045, 0x043, 0x042, 0x040, 0x03f, 0x03e, 0x03c, 0x03b, 0x039, 0x038,
    0x037, 0x035, 0x034, 0x033, 0x031, 0x030, 0x02f, 0x02e, 0x02d, 0x02b, 0x02a, 0x029, 0x028,
    0x027, 0x026, 0x025, 0x024, 0x023, 0x022, 0x021, 0x020, 0x01f, 0x01e, 0x01d, 0x01c, 0x01b,
    0x01a, 0x019, 0x018, 0x017, 0x017, 0x016, 0x015, 0x014, 0x014, 0x013, 0x012, 0x011, 0x011,
    0x010, 0x00f, 0x00f, 0x00e, 0x00d, 0x00d, 0x00c, 0x00c, 0x00b, 0x00a, 0x00a, 0x009, 0x009,
    0x008, 0x008, 0x007, 0x007, 0x007, 0x006, 0x006, 0x005, 0x005, 0x005, 0x004, 0x004, 0x004,
    0x003, 0x003, 0x003, 0x002, 0x002, 0x002, 0x002, 0x001, 0x001, 0x001, 0x001, 0x001, 0x001,
    0x001, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000,
];

/// Attenuation to linear volume: 10-bit mantissas with an implied leading bit.
///
/// ymfm stores these pre-scaled as `(mantissa | 0x400) << 2` so its lookup is a
/// single shift. This table holds the **raw mantissas** instead, because that is
/// what the checksum in the spec was measured over, and
/// [`attenuation_to_volume`] applies the leading bit and the shift at use. The
/// mantissa is
/// `round(2^(-(i + 1) / 256) * 2048) - 1024`, exact at 0 of 256 — and **the
/// `i + 1` and the `- 1024` are both load-bearing**: the obvious
/// `2^(-i/256) * 2048` misses all 256 entries and truncating instead of rounding
/// misses 139.
pub static POW: [u16; 256] = [
    0x3fa, 0x3f5, 0x3ef, 0x3ea, 0x3e4, 0x3df, 0x3da, 0x3d4, 0x3cf, 0x3c9, 0x3c4, 0x3bf, 0x3b9,
    0x3b4, 0x3ae, 0x3a9, 0x3a4, 0x39f, 0x399, 0x394, 0x38f, 0x38a, 0x384, 0x37f, 0x37a, 0x375,
    0x370, 0x36a, 0x365, 0x360, 0x35b, 0x356, 0x351, 0x34c, 0x347, 0x342, 0x33d, 0x338, 0x333,
    0x32e, 0x329, 0x324, 0x31f, 0x31a, 0x315, 0x310, 0x30b, 0x306, 0x302, 0x2fd, 0x2f8, 0x2f3,
    0x2ee, 0x2e9, 0x2e5, 0x2e0, 0x2db, 0x2d6, 0x2d2, 0x2cd, 0x2c8, 0x2c4, 0x2bf, 0x2ba, 0x2b5,
    0x2b1, 0x2ac, 0x2a8, 0x2a3, 0x29e, 0x29a, 0x295, 0x291, 0x28c, 0x288, 0x283, 0x27f, 0x27a,
    0x276, 0x271, 0x26d, 0x268, 0x264, 0x25f, 0x25b, 0x257, 0x252, 0x24e, 0x249, 0x245, 0x241,
    0x23c, 0x238, 0x234, 0x230, 0x22b, 0x227, 0x223, 0x21e, 0x21a, 0x216, 0x212, 0x20e, 0x209,
    0x205, 0x201, 0x1fd, 0x1f9, 0x1f5, 0x1f0, 0x1ec, 0x1e8, 0x1e4, 0x1e0, 0x1dc, 0x1d8, 0x1d4,
    0x1d0, 0x1cc, 0x1c8, 0x1c4, 0x1c0, 0x1bc, 0x1b8, 0x1b4, 0x1b0, 0x1ac, 0x1a8, 0x1a4, 0x1a0,
    0x19c, 0x199, 0x195, 0x191, 0x18d, 0x189, 0x185, 0x181, 0x17e, 0x17a, 0x176, 0x172, 0x16f,
    0x16b, 0x167, 0x163, 0x160, 0x15c, 0x158, 0x154, 0x151, 0x14d, 0x149, 0x146, 0x142, 0x13e,
    0x13b, 0x137, 0x134, 0x130, 0x12c, 0x129, 0x125, 0x122, 0x11e, 0x11b, 0x117, 0x114, 0x110,
    0x10c, 0x109, 0x106, 0x102, 0x0ff, 0x0fb, 0x0f8, 0x0f4, 0x0f1, 0x0ed, 0x0ea, 0x0e7, 0x0e3,
    0x0e0, 0x0dc, 0x0d9, 0x0d6, 0x0d2, 0x0cf, 0x0cc, 0x0c8, 0x0c5, 0x0c2, 0x0be, 0x0bb, 0x0b8,
    0x0b5, 0x0b1, 0x0ae, 0x0ab, 0x0a8, 0x0a4, 0x0a1, 0x09e, 0x09b, 0x098, 0x094, 0x091, 0x08e,
    0x08b, 0x088, 0x085, 0x082, 0x07e, 0x07b, 0x078, 0x075, 0x072, 0x06f, 0x06c, 0x069, 0x066,
    0x063, 0x060, 0x05d, 0x05a, 0x057, 0x054, 0x051, 0x04e, 0x04b, 0x048, 0x045, 0x042, 0x03f,
    0x03c, 0x039, 0x036, 0x033, 0x030, 0x02d, 0x02a, 0x028, 0x025, 0x022, 0x01f, 0x01c, 0x019,
    0x016, 0x014, 0x011, 0x00e, 0x00b, 0x008, 0x006, 0x003, 0x000,
];

/// Phase step per effective frequency, 12 notes x 64 key fractions.
///
/// **Chip data — do not replace with a formula.** See the module docs.
pub static PHASE_STEP: [u32; 768] = [
    41568, 41600, 41632, 41664, 41696, 41728, 41760, 41792, 41856, 41888, 41920, 41952, 42016,
    42048, 42080, 42112, 42176, 42208, 42240, 42272, 42304, 42336, 42368, 42400, 42464, 42496,
    42528, 42560, 42624, 42656, 42688, 42720, 42784, 42816, 42848, 42880, 42912, 42944, 42976,
    43008, 43072, 43104, 43136, 43168, 43232, 43264, 43296, 43328, 43392, 43424, 43456, 43488,
    43552, 43584, 43616, 43648, 43712, 43744, 43776, 43808, 43872, 43904, 43936, 43968, 44032,
    44064, 44096, 44128, 44192, 44224, 44256, 44288, 44352, 44384, 44416, 44448, 44512, 44544,
    44576, 44608, 44672, 44704, 44736, 44768, 44832, 44864, 44896, 44928, 44992, 45024, 45056,
    45088, 45152, 45184, 45216, 45248, 45312, 45344, 45376, 45408, 45472, 45504, 45536, 45568,
    45632, 45664, 45728, 45760, 45792, 45824, 45888, 45920, 45984, 46016, 46048, 46080, 46144,
    46176, 46208, 46240, 46304, 46336, 46368, 46400, 46464, 46496, 46528, 46560, 46656, 46688,
    46720, 46752, 46816, 46848, 46880, 46912, 46976, 47008, 47072, 47104, 47136, 47168, 47232,
    47264, 47328, 47360, 47392, 47424, 47488, 47520, 47552, 47584, 47648, 47680, 47744, 47776,
    47808, 47840, 47904, 47936, 48032, 48064, 48096, 48128, 48192, 48224, 48288, 48320, 48384,
    48416, 48448, 48480, 48544, 48576, 48640, 48672, 48736, 48768, 48800, 48832, 48896, 48928,
    48992, 49024, 49088, 49120, 49152, 49184, 49248, 49280, 49344, 49376, 49440, 49472, 49504,
    49536, 49600, 49632, 49696, 49728, 49792, 49824, 49856, 49888, 49952, 49984, 50048, 50080,
    50144, 50176, 50208, 50240, 50304, 50336, 50400, 50432, 50496, 50528, 50560, 50592, 50656,
    50688, 50752, 50784, 50880, 50912, 50944, 50976, 51040, 51072, 51136, 51168, 51232, 51264,
    51328, 51360, 51424, 51456, 51488, 51520, 51616, 51648, 51680, 51712, 51776, 51808, 51872,
    51904, 51968, 52000, 52064, 52096, 52160, 52192, 52224, 52256, 52384, 52416, 52448, 52480,
    52544, 52576, 52640, 52672, 52736, 52768, 52832, 52864, 52928, 52960, 52992, 53024, 53120,
    53152, 53216, 53248, 53312, 53344, 53408, 53440, 53504, 53536, 53600, 53632, 53696, 53728,
    53792, 53824, 53920, 53952, 54016, 54048, 54112, 54144, 54208, 54240, 54304, 54336, 54400,
    54432, 54496, 54528, 54592, 54624, 54688, 54720, 54784, 54816, 54880, 54912, 54976, 55008,
    55072, 55104, 55168, 55200, 55264, 55296, 55360, 55392, 55488, 55520, 55584, 55616, 55680,
    55712, 55776, 55808, 55872, 55936, 55968, 56032, 56064, 56128, 56160, 56224, 56288, 56320,
    56384, 56416, 56480, 56512, 56576, 56608, 56672, 56736, 56768, 56832, 56864, 56928, 56960,
    57024, 57120, 57152, 57216, 57248, 57312, 57376, 57408, 57472, 57536, 57568, 57632, 57664,
    57728, 57792, 57824, 57888, 57952, 57984, 58048, 58080, 58144, 58208, 58240, 58304, 58368,
    58400, 58464, 58496, 58560, 58624, 58656, 58720, 58784, 58816, 58880, 58912, 58976, 59040,
    59072, 59136, 59200, 59232, 59296, 59328, 59392, 59456, 59488, 59552, 59648, 59680, 59744,
    59776, 59840, 59904, 59936, 60000, 60064, 60128, 60160, 60224, 60288, 60320, 60384, 60416,
    60512, 60544, 60608, 60640, 60704, 60768, 60800, 60864, 60928, 60992, 61024, 61088, 61152,
    61184, 61248, 61280, 61376, 61408, 61472, 61536, 61600, 61632, 61696, 61760, 61824, 61856,
    61920, 61984, 62048, 62080, 62144, 62208, 62272, 62304, 62368, 62432, 62496, 62528, 62592,
    62656, 62720, 62752, 62816, 62880, 62944, 62976, 63040, 63104, 63200, 63232, 63296, 63360,
    63424, 63456, 63520, 63584, 63648, 63680, 63744, 63808, 63872, 63904, 63968, 64032, 64096,
    64128, 64192, 64256, 64320, 64352, 64416, 64480, 64544, 64608, 64672, 64704, 64768, 64832,
    64896, 64928, 65024, 65056, 65120, 65184, 65248, 65312, 65376, 65408, 65504, 65536, 65600,
    65664, 65728, 65792, 65856, 65888, 65984, 66016, 66080, 66144, 66208, 66272, 66336, 66368,
    66464, 66496, 66560, 66624, 66688, 66752, 66816, 66848, 66944, 66976, 67040, 67104, 67168,
    67232, 67296, 67328, 67424, 67456, 67520, 67584, 67648, 67712, 67776, 67808, 67904, 67936,
    68000, 68064, 68128, 68192, 68256, 68288, 68384, 68448, 68512, 68544, 68640, 68672, 68736,
    68800, 68896, 68928, 68992, 69056, 69120, 69184, 69248, 69280, 69376, 69440, 69504, 69536,
    69632, 69664, 69728, 69792, 69920, 69952, 70016, 70080, 70144, 70208, 70272, 70304, 70400,
    70464, 70528, 70560, 70656, 70688, 70752, 70816, 70912, 70976, 71040, 71104, 71136, 71232,
    71264, 71360, 71424, 71488, 71552, 71616, 71648, 71744, 71776, 71872, 71968, 72032, 72096,
    72160, 72192, 72288, 72320, 72416, 72480, 72544, 72608, 72672, 72704, 72800, 72832, 72928,
    72992, 73056, 73120, 73184, 73216, 73312, 73344, 73440, 73504, 73568, 73632, 73696, 73728,
    73824, 73856, 73952, 74080, 74144, 74208, 74272, 74304, 74400, 74432, 74528, 74592, 74656,
    74720, 74784, 74816, 74912, 74944, 75040, 75136, 75200, 75264, 75328, 75360, 75456, 75488,
    75584, 75648, 75712, 75776, 75840, 75872, 75968, 76000, 76096, 76224, 76288, 76352, 76416,
    76448, 76544, 76576, 76672, 76736, 76800, 76864, 76928, 77024, 77120, 77152, 77248, 77344,
    77408, 77472, 77536, 77568, 77664, 77696, 77792, 77856, 77920, 77984, 78048, 78144, 78240,
    78272, 78368, 78464, 78528, 78592, 78656, 78688, 78784, 78816, 78912, 78976, 79040, 79104,
    79168, 79264, 79360, 79392, 79488, 79616, 79680, 79744, 79808, 79840, 79936, 79968, 80064,
    80128, 80192, 80256, 80320, 80416, 80512, 80544, 80640, 80768, 80832, 80896, 80960, 80992,
    81088, 81120, 81216, 81280, 81344, 81408, 81472, 81568, 81664, 81696, 81792, 81952, 82016,
    82080, 82144, 82176, 82272, 82304, 82400, 82464, 82528, 82592, 82656, 82752, 82848, 82880,
    82976,
];

/// Envelope attenuation increments, four packed nibbles per rate.
pub static INCREMENT: [u32; 64] = [
    0x00000000, 0x00000000, 0x10101010, 0x10101010, 0x10101010, 0x10101010, 0x11101110, 0x11101110,
    0x10101010, 0x10111010, 0x11101110, 0x11111110, 0x10101010, 0x10111010, 0x11101110, 0x11111110,
    0x10101010, 0x10111010, 0x11101110, 0x11111110, 0x10101010, 0x10111010, 0x11101110, 0x11111110,
    0x10101010, 0x10111010, 0x11101110, 0x11111110, 0x10101010, 0x10111010, 0x11101110, 0x11111110,
    0x10101010, 0x10111010, 0x11101110, 0x11111110, 0x10101010, 0x10111010, 0x11101110, 0x11111110,
    0x10101010, 0x10111010, 0x11101110, 0x11111110, 0x10101010, 0x10111010, 0x11101110, 0x11111110,
    0x11111111, 0x21112111, 0x21212121, 0x22212221, 0x22222222, 0x42224222, 0x42424242, 0x44424442,
    0x44444444, 0x84448444, 0x84848484, 0x88848884, 0x88888888, 0x88888888, 0x88888888, 0x88888888,
];

/// DT1 detune magnitudes, indexed by `[key_code][detune & 3]`.
///
/// Chip data. The magnitude depends on the key code, which is what makes DT1 a
/// detune rather than a transpose — asserted by `detune_scales_with_the_key_code`.
pub static DETUNE: [[u8; 4]; 32] = [
    [0, 0, 1, 2],
    [0, 0, 1, 2],
    [0, 0, 1, 2],
    [0, 0, 1, 2],
    [0, 1, 2, 2],
    [0, 1, 2, 3],
    [0, 1, 2, 3],
    [0, 1, 2, 3],
    [0, 1, 2, 4],
    [0, 1, 3, 4],
    [0, 1, 3, 4],
    [0, 1, 3, 5],
    [0, 2, 4, 5],
    [0, 2, 4, 6],
    [0, 2, 4, 6],
    [0, 2, 5, 7],
    [0, 2, 5, 8],
    [0, 3, 6, 8],
    [0, 3, 6, 9],
    [0, 3, 7, 10],
    [0, 4, 8, 11],
    [0, 4, 8, 12],
    [0, 4, 9, 13],
    [0, 5, 10, 14],
    [0, 5, 11, 16],
    [0, 6, 12, 17],
    [0, 6, 13, 19],
    [0, 7, 14, 20],
    [0, 8, 16, 22],
    [0, 8, 16, 22],
    [0, 8, 16, 22],
    [0, 8, 16, 22],
];

/// FNV-1a over a `u16` table's little-endian bytes.
///
/// New to this repository — `grep` found no existing FNV in `crates/` or
/// `scripts/` — so this is the only implementation, and the vector generator's
/// determinism check reuses it rather than growing a second one.
#[must_use]
pub fn fnv1a_u16(vals: &[u16]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in vals {
        for b in v.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

/// FNV-1a over a `u32` table's little-endian bytes.
#[must_use]
pub fn fnv1a_u32(vals: &[u32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in vals {
        for b in v.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

/// The absolute sine attenuation for a phase, as a 4.8 value with the sign in bit 15.
///
/// [`SIN`] covers one quarter period. Bit 8 of the phase mirrors it — ymfm inverts
/// the whole input with `~` and then masks, which is the same as `0xFF - index` for
/// the low byte — and bit 9 is the sign of the half-cycle.
///
/// **The sign is not in the table.** ymfm builds an OPM waveform array at reset
/// (`ymfm_opm.cpp:55`: `abs_sin_attenuation(index) | (bitfield(index, 9) << 15)`)
/// and `compute_volume` reads bit 15 back out of it. This function folds those two
/// steps together, which is why its return value is not simply a table entry.
#[must_use]
pub fn abs_sin_attenuation(phase: u32) -> u16 {
    let index = if phase & 0x100 != 0 { !phase } else { phase };
    SIN[(index & 0xFF) as usize] | (u16::try_from((phase >> 9) & 1).unwrap_or(0) << 15)
}

/// A 5.8 attenuation converted to linear volume.
///
/// From `ymfm_fm.ipp:133`. ymfm pre-scales its table by `(a | 0x400) << 2` so this
/// is one shift; [`POW`] holds the raw mantissas (the form the spec's checksum
/// covers), so the leading bit and the scale are applied here instead.
#[must_use]
pub fn attenuation_to_volume(atten: u32) -> u32 {
    let mantissa = (u32::from(POW[(atten & 0xFF) as usize]) | 0x400) << 2;
    mantissa >> (atten >> 8)
}

/// A 4-bit envelope increment for a 6-bit rate and a 3-bit counter position.
///
/// From `ymfm_fm.ipp:166`.
#[must_use]
pub fn attenuation_increment(rate: u32, index: u32) -> u32 {
    (INCREMENT[(rate & 0x3F) as usize] >> (4 * (index & 7))) & 0xF
}

/// The DT1 phase adjustment for a key code and a detune setting.
///
/// Magnitude from bits 0-1 of the register field, sign from bit 2. From
/// `ymfm_fm.ipp:192`.
#[must_use]
pub fn detune_adjustment(detune: u32, key_code: u32) -> i32 {
    let magnitude = i32::from(DETUNE[(key_code & 0x1F) as usize][(detune & 3) as usize]);
    if detune & 4 != 0 {
        -magnitude
    } else {
        magnitude
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four tables, pinned by checksum rather than by proofreading.
    ///
    /// 768 hand-transcribed numbers cannot be verified by reading them. These
    /// figures were measured against ymfm's own tables on 2026-08-09 and are
    /// recorded in the spec; a table that is subtly wrong fails here rather than on
    /// one obscure note in one case of the suite.
    #[test]
    fn the_four_tables_match_their_measured_checksums() {
        assert_eq!(SIN.len(), 256);
        assert_eq!(SIN.iter().map(|&v| u32::from(v)).sum::<u32>(), 65_406);
        assert_eq!(fnv1a_u16(&SIN), 0x6901_6697_2613_166b);

        assert_eq!(POW.len(), 256);
        assert_eq!(POW.iter().map(|&v| u32::from(v)).sum::<u32>(), 115_543);
        assert_eq!(fnv1a_u16(&POW), 0xc284_cffe_0e13_3896);

        assert_eq!(PHASE_STEP.len(), 768);
        assert_eq!(
            PHASE_STEP.iter().map(|&v| u64::from(v)).sum::<u64>(),
            46_015_744
        );
        assert_eq!(PHASE_STEP[0], 41_568);
        assert_eq!(PHASE_STEP[767], 82_976);
        assert_eq!(fnv1a_u32(&PHASE_STEP), 0x3b0f_96a4_7792_bdb3);

        assert_eq!(INCREMENT.len(), 64);
        assert_eq!(
            INCREMENT.iter().map(|&v| u64::from(v)).sum::<u64>(),
            35_716_092_092
        );
        assert_eq!(fnv1a_u32(&INCREMENT), 0x7e8e_a956_6fb1_810d);
    }

    /// The phase-step table is chip data, not a formula — asserted, not assumed.
    ///
    /// ymfm's own comment says the computed table "differs in a few spots from the
    /// data verified from an actual chip" (David Viens' analysis). Measured: it
    /// diverges from a pure exponential by up to 106 units, its consecutive deltas
    /// are quantised to exactly {32, 64, 96, 128, 160}, and its span is 1.99615
    /// rather than the formula's 1.99820. This test is why the table is transcribed
    /// rather than computed: a future reader who "simplifies" it fails here.
    #[test]
    fn the_phase_step_table_is_not_an_exponential() {
        let mut deltas: Vec<u32> = PHASE_STEP.windows(2).map(|w| w[1] - w[0]).collect();
        deltas.sort_unstable();
        deltas.dedup();
        assert_eq!(
            deltas,
            vec![32, 64, 96, 128, 160],
            "quantised chip data, not a smooth curve"
        );

        let base = f64::from(PHASE_STEP[0]);
        let worst = PHASE_STEP
            .iter()
            .enumerate()
            .map(|(i, &g)| {
                #[allow(clippy::cast_precision_loss)]
                let want = (base * 2f64.powf(i as f64 / 768.0)).round();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let d = (want - f64::from(g)).abs() as u32;
                d
            })
            .max()
            .unwrap();
        assert_eq!(worst, 106, "no formula reproduces this table");

        let span = f64::from(PHASE_STEP[767]) / base;
        assert!(
            (span - 1.996_150_885).abs() < 1e-6,
            "measured span, not the formula's 1.99820: {span}"
        );
    }

    /// The two computed tables match their closed forms exactly.
    ///
    /// Both fits were measured at 0 of 256 mismatched. The near-misses are the
    /// reason this test exists: for the power table the obvious
    /// `2^(-i/256) * 2048` misses all 256 entries, and truncating instead of
    /// rounding misses 139. The tables are transcribed only because floating point
    /// is unavailable in a `const fn`; this test is what makes that safe.
    #[test]
    fn the_sine_and_power_tables_match_their_closed_forms() {
        for i in 0..256u32 {
            let want = -((f64::from(i) + 0.5) * core::f64::consts::PI / 512.0)
                .sin()
                .log2()
                * 256.0;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let want = want.round() as u16;
            assert_eq!(SIN[i as usize], want, "sine at {i}");
        }
        for i in 0..256u32 {
            let want = (2f64.powf(-(f64::from(i) + 1.0) / 256.0) * 2048.0).round() - 1024.0;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let want = want as u16;
            assert_eq!(POW[i as usize], want, "power at {i}");
        }
    }

    /// The near-misses are real, so the formulas above are not the only fit tried.
    ///
    /// Without this, a future reader has no way to know that the `i + 1` and the
    /// rounding in the power formula were *measured* rather than guessed, and the
    /// natural "simplification" looks free.
    #[test]
    fn the_obvious_power_formulas_are_wrong() {
        let naive = (0..256u32)
            .filter(|&i| {
                let w = (2f64.powf(-f64::from(i) / 256.0) * 2048.0).round();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let w = w as u16;
                POW[i as usize] != w
            })
            .count();
        assert_eq!(naive, 256, "2^(-i/256)*2048 misses every entry");

        let truncated = (0..256u32)
            .filter(|&i| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let w = ((2f64.powf(-(f64::from(i) + 1.0) / 256.0) * 2048.0) as u32 - 1024) as u16;
                POW[i as usize] != w
            })
            .count();
        assert_eq!(truncated, 139, "truncating instead of rounding misses 139");
    }

    /// The sine lookup mirrors on bit 8 and takes its sign from bit 9.
    ///
    /// The fold and the sign are in [`abs_sin_attenuation`], not in [`SIN`], because
    /// ymfm bakes them into a waveform array at reset. Checked at the seams: the
    /// two quarter-periods either side of the mirror must agree, and the two halves
    /// must differ only in bit 15.
    #[test]
    fn the_sine_lookup_mirrors_and_signs_correctly() {
        for i in 0..256u32 {
            assert_eq!(
                abs_sin_attenuation(i) & 0x7FFF,
                abs_sin_attenuation(0x1FF - i) & 0x7FFF,
                "the first half mirrors about phase 0x100 at {i}"
            );
        }
        for phase in 0..512u32 {
            assert_eq!(
                abs_sin_attenuation(phase) & 0x8000,
                0,
                "first half positive"
            );
            assert_eq!(
                abs_sin_attenuation(phase + 512) & 0x8000,
                0x8000,
                "second half negative"
            );
            assert_eq!(
                abs_sin_attenuation(phase) & 0x7FFF,
                abs_sin_attenuation(phase + 512) & 0x7FFF,
                "and the magnitudes match"
            );
        }
        // Phase 0 is the quietest point of the quarter and 0x100 the loudest, in
        // attenuation terms: 0x859 down to 0x000.
        assert_eq!(abs_sin_attenuation(0) & 0x7FFF, 0x859);
        assert_eq!(abs_sin_attenuation(0xFF) & 0x7FFF, 0x000);
    }

    /// Volume falls monotonically as attenuation rises, and full attenuation is silent.
    ///
    /// The property that says the pre-scale and the shift were reassembled
    /// correctly after moving them out of the table. A wrong shift shows up as a
    /// non-monotonic step at each 0x100 boundary.
    #[test]
    fn volume_falls_monotonically_with_attenuation() {
        let mut prev = attenuation_to_volume(0);
        // 8,168, not 8,192: POW[0] is 0x3fa, so (0x3fa | 0x400) << 2 = 8,168.
        // The table starts one step below full scale because it is a 2^-x curve
        // sampled at (i + 1) / 256 rather than at i / 256.
        assert_eq!(prev, 8_168, "zero attenuation is the loudest");
        for atten in 1..0x1000u32 {
            let v = attenuation_to_volume(atten);
            assert!(v <= prev, "monotonic at {atten}: {v} > {prev}");
            prev = v;
        }
        assert_eq!(
            attenuation_to_volume(0xFFF),
            0,
            "and full attenuation is silent"
        );
    }

    /// The increment lookup unpacks four nibbles per rate, and rate 0 never moves.
    #[test]
    fn the_increment_lookup_unpacks_four_nibbles() {
        for index in 0..4u32 {
            assert_eq!(
                attenuation_increment(0, index),
                0,
                "rate 0 holds the envelope still"
            );
        }
        // Rate 63's nibbles are the largest steps in the table.
        let fastest: Vec<u32> = (0..4).map(|i| attenuation_increment(63, i)).collect();
        assert!(
            fastest.iter().all(|&v| v >= 8),
            "the fastest rate takes big steps: {fastest:?}"
        );
    }

    /// DT1 settings 0 and 4 are both no-ops, and 1-3 mirror 5-7.
    ///
    /// Magnitude is bits 0-1 and sign is bit 2, so setting 4 is "zero, negated" —
    /// still zero. A core that folded the sign bit into the magnitude would make
    /// setting 4 a real detune and setting 7 read past the row.
    #[test]
    fn detune_settings_zero_and_four_are_both_no_ops() {
        for kc in 0..32u32 {
            assert_eq!(detune_adjustment(0, kc), 0, "setting 0 at key code {kc}");
            assert_eq!(detune_adjustment(4, kc), 0, "and setting 4");
            for mag in 1..4u32 {
                assert_eq!(
                    detune_adjustment(mag, kc),
                    -detune_adjustment(mag | 4, kc),
                    "settings {mag} and {} are symmetric at key code {kc}",
                    mag | 4
                );
            }
        }
    }

    /// DT1 depends on the key code — that is what makes it a detune, not a transpose.
    #[test]
    fn detune_scales_with_the_key_code() {
        assert_ne!(
            detune_adjustment(3, 0),
            detune_adjustment(3, 31),
            "a constant offset would be a transpose"
        );
        // Monotonic non-decreasing across key codes for a fixed magnitude.
        for mag in 1..4u32 {
            let mut prev = detune_adjustment(mag, 0);
            for kc in 1..32u32 {
                let v = detune_adjustment(mag, kc);
                assert!(v >= prev, "magnitude {mag} rises with key code {kc}");
                prev = v;
            }
        }
    }
}
