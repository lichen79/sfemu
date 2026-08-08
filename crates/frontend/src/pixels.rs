//! The framebuffer as the pixels a window wants.
//!
//! The windowing library takes `0x00RRGGBB` per pixel; `video` produces palette
//! pens. This is the one-line bridge, and it is here rather than in the display
//! module because it is arithmetic, and arithmetic behind the display boundary
//! cannot be tested.
//!
//! # Why this calls `entry_to_rgb` but is not tested against it
//!
//! It calls it, and `tests::the_window_and_the_screenshot_cannot_disagree`
//! requires the two to agree over all 65,536 entries — a screenshot that differed
//! from the window would be a genuinely confusing bug. But the *format* is pinned
//! by hand-written literals, because a test that only compared this to the
//! function it wraps would pass with both wrong in the same direction.

use machine::video::compose::Video;
use machine::video::palette::entry_to_rgb;

/// One palette entry as `0x00RRGGBB`.
///
/// Red in bits 16-23, green in 8-15, blue in 0-7, and the top byte zero. The
/// windowing library ignores the top byte; leaving it zero rather than 0xFF is the
/// convention `minifb`'s own `from_u8_rgb` example uses.
///
/// `pub(crate)` for the graphics viewer's palette swatches: a swatch and the game's
/// own pixels must be the same colour, and that is guaranteed by calling this rather
/// than by writing the shift a second time.
pub(crate) fn argb(entry: u16) -> u32 {
    let [r, g, b] = entry_to_rgb(entry);
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

/// Converts the rendered frame into `out`, replacing its contents.
///
/// `out` is reused across frames: this runs sixty times a second on a 344 KB
/// buffer, and allocating one per frame is the kind of waste that shows up as a
/// stutter rather than as a slowdown.
///
/// Indexes the palette directly, as `Video::rgb` does. The renderer cannot produce
/// an out-of-range pen — the largest is `palette::BACKGROUND_PEN`, 0xBFF, and the
/// palette is `palette::PENS` = 0xC00 entries — so a `get` here would be dead
/// defensiveness that hid a renderer bug instead of crashing on it.
pub fn pens_to_argb(v: &Video, out: &mut Vec<u32>) {
    let pal = v.palette();
    out.clear();
    out.extend(v.fb.pens.iter().map(|&pen| argb(pal[usize::from(pen)])));
}

#[cfg(test)]
mod tests {
    use super::*;
    use machine::video::{palette, regs, HEIGHT, WIDTH};
    use machine::BoardConfig;

    /// A `Video` whose palette holds known entries, built through the real render
    /// path so what is under test is what the window will use.
    ///
    /// Uses `Video` directly rather than a booted machine: this function converts a
    /// framebuffer, and a CPU has nothing to do with it.
    fn video_with_palette(entries: &[(usize, u16)]) -> Video {
        let cfg = BoardConfig::sf2();
        let mut v = Video::new(cfg.video, cfg.mapper, Vec::new());
        // The palette is built from gfxram at render time, so write the entries
        // there and render once. Palette base register 0 resolves to word 0.
        let mut gfxram = vec![0u16; 0x1_8000];
        for &(pen, entry) in entries {
            gfxram[pen] = entry;
        }
        let mut cps_a = [0u16; 0x20];
        cps_a[regs::PALETTE_BASE] = 0;
        let mut cps_b = [0u16; 0x20];
        // All six palette pages enabled, so every entry written above is read.
        cps_b[cfg.video.palette_control] = 0x003F;
        v.render(&gfxram, &cps_a, &cps_b);
        v
    }

    /// The buffer is one `u32` per visible pixel, and the whole frame.
    #[test]
    fn the_buffer_is_one_word_per_pixel_of_the_visible_frame() {
        let v = video_with_palette(&[]);
        let mut out = Vec::new();
        pens_to_argb(&v, &mut out);
        assert_eq!(out.len(), 86_016, "384 * 224, as a literal");
        assert_eq!(out.len(), WIDTH * HEIGHT, "and that is the frame's size");
    }

    /// A pen becomes `0x00RRGGBB`, with the channels in that order.
    ///
    /// The literals are hand-computed from `video::palette::entry_to_rgb`'s
    /// documented arithmetic — `bright = 0x0F + ((e >> 12) << 1)`, each nibble
    /// scaled `* 0x11 * bright / 0x2D` — and written here rather than obtained by
    /// calling it. A conversion checked against the function it wraps agrees with
    /// itself whatever either does.
    ///
    /// Entry 0xFF00 is brightness 15 (unity: `0x0F + 30 = 0x2D`) with red 0x0F:
    /// `0x0F * 0x11 * 0x2D / 0x2D` = 0xFF. So pure red is 0x00FF0000, which is what
    /// pins the *order* — a red/blue swap gives 0x000000FF.
    #[test]
    fn a_pen_becomes_argb_with_red_in_the_high_byte() {
        let v = video_with_palette(&[
            (0, 0xFF00), // brightness 15, red 15, green 0, blue 0
            (1, 0xF0F0), // green 15
            (2, 0xF00F), // blue 15
            (3, 0xFFFF), // white
            (4, 0xF000), // black
        ]);
        let p = v.palette();
        assert_eq!(p[0], 0xFF00, "the palette really holds the entry");

        assert_eq!(argb(p[0]), 0x00FF_0000, "red is bits 16-23");
        assert_eq!(argb(p[1]), 0x0000_FF00, "green is bits 8-15");
        assert_eq!(argb(p[2]), 0x0000_00FF, "blue is bits 0-7");
        assert_eq!(argb(p[3]), 0x00FF_FFFF, "white");
        assert_eq!(argb(p[4]), 0x0000_0000, "black");
    }

    /// Brightness scales, and it truncates.
    ///
    /// `entry_to_rgb` records that entry 0x8777 gives 81 and not 82, which is the
    /// hardware's truncating division as MAME models it. Pinned here too, because a
    /// frontend that rounded instead would differ from the PPM dump by one in every
    /// channel — a difference nobody would ever see on screen and which would make
    /// the two outputs disagree forever.
    ///
    /// Hand-computed: brightness 8 gives `0x0F + 16 = 0x1F`; `7 * 0x11 * 0x1F /
    /// 0x2D` = `119 * 31 / 45` = `3689 / 45` = 81 (81.98 truncated) = 0x51.
    #[test]
    fn brightness_truncates_exactly_as_the_renderer_does() {
        assert_eq!(argb(0x8777), 0x0051_5151, "81 = 0x51 in all three channels");
    }

    /// The conversion agrees with the renderer's own, pen for pen.
    ///
    /// The literals above are what pin the format; this pins the *agreement*. The
    /// PPM writer in `sfemu` uses `entry_to_rgb` and the window uses this, and a
    /// channel swap in one of them would make a screenshot and the window disagree
    /// while both looked plausible alone. Checked over every reachable entry rather
    /// than a sample: 65,536 is cheap, and the failure could be in one brightness
    /// level.
    #[test]
    fn the_window_and_the_screenshot_cannot_disagree() {
        for e in 0..=0xFFFFu16 {
            let [r, g, b] = palette::entry_to_rgb(e);
            let want = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
            assert_eq!(argb(e), want, "entry {e:#06x}");
        }
    }

    /// Each pixel takes its own pen's colour, in row-major order.
    ///
    /// A conversion that wrote one colour everywhere would pass every test above.
    /// This puts two different pens in known places and reads them back at the
    /// matching offsets.
    ///
    /// Reaching into the framebuffer directly: the subject is the conversion, and
    /// drawing two specific pens through the tile path would test `video` again
    /// while saying less about this function.
    #[test]
    fn each_pixel_takes_its_own_pens_colour() {
        let mut v = video_with_palette(&[(0, 0xFF00), (1, 0xF00F)]);
        v.fb.pens[0] = 0;
        v.fb.pens[1] = 1;
        v.fb.pens[WIDTH] = 1; // the first pixel of row 1
        v.fb.pens[86_015] = 0; // the last pixel of the frame

        let mut out = Vec::new();
        pens_to_argb(&v, &mut out);
        assert_eq!(out[0], 0x00FF_0000, "pen 0 is red");
        assert_eq!(out[1], 0x0000_00FF, "pen 1 is blue");
        assert_eq!(out[WIDTH], 0x0000_00FF, "row-major: row 1 starts at WIDTH");
        assert_eq!(out[86_015], 0x00FF_0000, "and the last pixel is converted");
    }

    /// The background pen converts, so the largest pen the renderer emits is in
    /// range.
    ///
    /// `pens_to_argb` indexes the palette without a bounds check, on the grounds
    /// that `BACKGROUND_PEN` (0xBFF) is the largest pen and `PENS` is 0xC00. If a
    /// later change to either constant broke that, this panics here rather than in
    /// somebody's window.
    #[test]
    fn the_largest_pen_the_renderer_can_emit_is_in_range() {
        assert_eq!(palette::BACKGROUND_PEN, 0xBFF);
        assert_eq!(palette::PENS, 0xC00, "one past the largest pen");
        let mut v = video_with_palette(&[(0xBFF, 0xFF00)]);
        v.fb.pens[0] = palette::BACKGROUND_PEN;
        let mut out = Vec::new();
        pens_to_argb(&v, &mut out);
        assert_eq!(out[0], 0x00FF_0000, "the background pen has a colour");
    }

    /// The buffer is reused, not appended to.
    ///
    /// Called sixty times a second on a 344 KB buffer, so it takes `&mut Vec` — and
    /// a missing `clear` would grow it without bound while every length assertion
    /// above still passed on the first call.
    #[test]
    fn a_reused_buffer_does_not_grow() {
        let v = video_with_palette(&[]);
        let mut out = Vec::new();
        for _ in 0..3 {
            pens_to_argb(&v, &mut out);
            assert_eq!(out.len(), 86_016);
        }
    }
}
