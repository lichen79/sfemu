#!/usr/bin/env python3
"""Renders the caption bands burned into `docs/sfemu-linkedin.mp4`.

Why this exists rather than ffmpeg's `drawtext`: the ffmpeg on this machine is
built without freetype, so `drawtext` is not in `-filters`. Rather than rebuild
ffmpeg, the text is rendered once to two RGBA PNGs and composited by `overlay`,
which is available. That also makes the wording reviewable in a diff, and the
result reproducible without re-typing an escaped filtergraph.

Two bands, because the LinkedIn cut is a 1080x1350 portrait card with the
gameplay letterboxed in the middle:

  * `top.png` -- the headline, above the video.
  * `bottom.png` -- the credits line and the no-ROM statement, below it.

The no-ROM line is not decoration. The clip shows Street Fighter II running, and
a viewer's first question is where the game data came from; the answer belongs in
the frame rather than in a caption someone may not expand.

Usage:

    python3 scripts/caption.py /tmp/caption

writes `<dir>/top.png` and `<dir>/bottom.png`.
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

# The card is 1080x1350 — LinkedIn's feed-optimal 4:5 portrait. At full width the
# gameplay strip is 630 tall (1080 / (384/224), CPS-1's exact aspect), which leaves
# 720 for the two bands: 360 each.
WIDTH = 1080
TOP_H = 360
BOTTOM_H = 360

FONT_DIR = Path("/System/Library/Fonts/Supplemental")
BOLD = FONT_DIR / "Arial Bold.ttf"
REGULAR = FONT_DIR / "Arial.ttf"

# Matched to the encode's `pad` colour, so the bands and the letterbox are one
# surface rather than three shades of near-black.
BG = (11, 13, 18, 255)
WHITE = (245, 246, 250, 255)
DIM = (150, 156, 170, 255)
ACCENT = (255, 186, 73, 255)


def centred(draw, y, text, font, fill):
    """Draws `text` centred on the card, returning the y below it."""
    box = draw.textbbox((0, 0), text, font=font)
    draw.text(((WIDTH - (box[2] - box[0])) / 2 - box[0], y), text, font=font, fill=fill)
    return y + (box[3] - box[1])


def top_band():
    img = Image.new("RGBA", (WIDTH, TOP_H), BG)
    d = ImageDraw.Draw(img)
    y = centred(d, 62, "I built an arcade emulator", ImageFont.truetype(str(BOLD), 64), WHITE)
    y = centred(d, y + 30, "from the hardware up", ImageFont.truetype(str(BOLD), 64), ACCENT)
    y = centred(
        d,
        y + 46,
        "Street Fighter II on emulated CPS-1 hardware",
        ImageFont.truetype(str(REGULAR), 30),
        WHITE,
    )
    centred(
        d,
        y + 26,
        "68000  ·  Z80  ·  CPS-1 video  ·  YM2151 FM  ·  OKI ADPCM",
        ImageFont.truetype(str(REGULAR), 26),
        DIM,
    )
    return img


def bottom_band():
    img = Image.new("RGBA", (WIDTH, BOTTOM_H), BG)
    d = ImageDraw.Draw(img)
    # Written out rather than a speaker emoji: Arial has no glyph for U+1F50A and
    # PIL renders the miss as a tofu box, which is worse than the word. First
    # because LinkedIn autoplays muted, and this clip is half sound.
    y = centred(d, 40, "TURN SOUND ON", ImageFont.truetype(str(BOLD), 27), ACCENT)
    y = centred(
        d,
        y + 40,
        # Asserted by `the_video_caption_states_the_current_test_count` in
        # `crates/sfemu/src/main.rs`: this line is a claim about the repository burned
        # into a published video, and it had already drifted once (it said 1,880 at
        # 1,882). Changing the count means re-rendering and re-uploading the cut.
        # Drifted a second time at 1,884, when two tests guarding the README's demo GIF
        # took the suite to 1,886 — the guarding test compares two literals, so it saw
        # nothing. Re-read the real total from `cargo test --workspace` before trusting
        # this line.
        "Written in Rust. 1,886 tests. No unsafe code.",
        ImageFont.truetype(str(REGULAR), 35),
        WHITE,
    )
    y = centred(
        d,
        y + 26,
        "Both chips verified sample-for-sample against",
        ImageFont.truetype(str(REGULAR), 28),
        DIM,
    )
    y = centred(
        d,
        y + 22,
        "1,000 vector cases apiece.",
        ImageFont.truetype(str(REGULAR), 28),
        DIM,
    )
    y = centred(d, y + 34, "github.com/lichen79/sfemu", ImageFont.truetype(str(BOLD), 40), ACCENT)
    # The claim a viewer will want, in the frame rather than in the caption.
    centred(
        d,
        y + 30,
        "Ships no ROMs — you supply a set you own.",
        ImageFont.truetype(str(REGULAR), 25),
        DIM,
    )
    return img


def main():
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/caption")
    out.mkdir(parents=True, exist_ok=True)
    top_band().save(out / "top.png")
    bottom_band().save(out / "bottom.png")
    print(out / "top.png")
    print(out / "bottom.png")


if __name__ == "__main__":
    main()
