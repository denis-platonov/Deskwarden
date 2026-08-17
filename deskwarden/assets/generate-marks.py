#!/usr/bin/env python3
"""Generates marks/<brand>.png, one network mark per card brand.

Generated rather than shipped, for the same reason the application icon is
(see generate-icon.py): there is no binary asset here whose provenance or
licensing anyone has to take on trust. Here there is a second reason, recorded
in docs/superpowers/specs/2026-08-17-card-art-design.md section 1 -- the
official network logos come with brand guidelines on colour, clear space and
minimum size that a 12-pixel corner badge breaks simply by existing.

**These are not imitations.** They are generic geometric glyphs in this app's
own palette that STAND FOR a network; none of them borrows a distinctive
element of the real mark (no interlocking circles, no centurion, no
wordmark). The honest cost is that a drawn mark is less immediately
recognisable than the real one; the badge is always accompanied by the brand's
name or its card, so it identifies rather than names.

The glyph and the background value are chosen TOGETHER, because seven marks
must be tellable apart at 12 px inside one blue palette. Three background
values carry pairs, and the two glyphs sharing a value differ in more than
outline: filled against striped, upright against diagonal, solid against
hollow.

Run from this directory:

    python generate-marks.py

Output: marks/*.png, 48x48 8-bit RGBA, written with the standard library only
-- no Pillow, no external tooling. 48 is the detail size; the app scales down
to the 12 px list badge.
"""

import math
import os
import struct
import zlib

# Same hex values as theme.rs's BLUE_DEEP / BLUE / BLUE_BRIGHT / BLUE_SOFT --
# kept in sync by hand (one is Rust, one is Python), exactly as
# generate-icon.py does.
BLUE_DEEP = (0x14, 0x30, 0x7A)
BLUE = (0x1B, 0x3F, 0xA0)
BLUE_BRIGHT = (0x3B, 0x74, 0xE8)
BLUE_SOFT = (0x7F, 0xA4, 0xEF)
WHITE = (0xFF, 0xFF, 0xFF)

SUPERSAMPLE = 4
SIZE = 48

# The geometry below is in a 24x24 viewbox.
VIEWBOX = 24.0
BADGE_RADIUS = 5.0


def _in_badge(x: float, y: float) -> bool:
    """Inside the rounded square the mark is drawn on."""
    r = BADGE_RADIUS
    cx = min(max(x, r), VIEWBOX - r)
    cy = min(max(y, r), VIEWBOX - r)
    dx, dy = x - cx, y - cy
    return dx * dx + dy * dy <= r * r


def _triangle(x: float, y: float) -> bool:
    """Right-pointing solid triangle: (7,5) (7,19) (18,12)."""
    if not 7.0 <= x <= 18.0:
        return False
    half = 7.0 * (18.0 - x) / 11.0
    return abs(y - 12.0) <= half


def _diamond(x: float, y: float) -> bool:
    return abs(x - 12.0) + abs(y - 12.0) <= 7.6


def _hbars(x: float, y: float) -> bool:
    """Two horizontal bars."""
    if not 4.6 <= x <= 19.4:
        return False
    return any(top <= y <= top + 3.6 for top in (6.6, 13.8))


def _disc(x: float, y: float) -> bool:
    return math.hypot(x - 12.0, y - 12.0) <= 6.9


def _ring(x: float, y: float) -> bool:
    d = math.hypot(x - 12.0, y - 12.0)
    return 3.4 <= d <= 7.4


def _bars(x: float, y: float) -> bool:
    """Three vertical bars."""
    if not 4.6 <= y <= 19.4:
        return False
    return any(left <= x <= left + 2.9 for left in (5.0, 10.55, 16.1))


def _slash(x: float, y: float) -> bool:
    """A thick diagonal band, bottom-left to top-right."""
    if not (4.0 <= x <= 20.0 and 4.0 <= y <= 20.0):
        return False
    return abs(x + y - 24.0) / math.sqrt(2.0) <= 2.7


# brand key -> (background, glyph colour, glyph predicate).
#
# The keys are the CardBrand variants' snake_case names; card_mark.rs pins
# that correspondence, driven from CARD_BRANDS, so a brand added later cannot
# ship markless.
MARKS = {
    # Deep: a solid wedge against three uprights.
    "visa": (BLUE_DEEP, WHITE, _triangle),
    "jcb": (BLUE_DEEP, WHITE, _bars),
    # Mid: a solid diamond against two horizontals.
    "mastercard": (BLUE, WHITE, _diamond),
    "american_express": (BLUE, WHITE, _hbars),
    # Bright: a diagonal band against the ring.
    "unionpay": (BLUE_BRIGHT, WHITE, _slash),
    "diners_club": (BLUE_BRIGHT, WHITE, _ring),
    # Soft: the only mark drawn dark-on-light.
    "discover": (BLUE_SOFT, BLUE_DEEP, _disc),
}

# **Exactly one glyph is hollow.** A first cut gave Amex a square frame and
# Diners a ring; at 12 px both collapsed to "a white shape with a dark dot in
# it" and were not tellable apart, which is the failure this whole size test
# exists to catch. Hollowness is now a distinguishing feature that only one
# mark spends, and the marks sharing a background value differ in filledness
# or in stroke direction rather than in outline alone.


def sample(key: str, x: float, y: float):
    """Colour of the viewbox-space point as (r, g, b, a)."""
    background, ink, glyph = MARKS[key]
    if not _in_badge(x, y):
        return (0, 0, 0, 0)
    return (*(ink if glyph(x, y) else background), 255)


def render(key: str, size: int) -> list:
    """`size`x`size` pixels as a flat list of (r, g, b, a) tuples."""
    pixels = []
    step = VIEWBOX / (size * SUPERSAMPLE)
    for py in range(size):
        for px in range(size):
            r = g = b = a = 0
            for sy in range(SUPERSAMPLE):
                for sx in range(SUPERSAMPLE):
                    x = (px * SUPERSAMPLE + sx + 0.5) * step
                    y = (py * SUPERSAMPLE + sy + 0.5) * step
                    sr, sg, sb, sa = sample(key, x, y)
                    # Premultiplied accumulation, so transparent samples don't
                    # drag the colour towards black along the edges.
                    r += sr * sa
                    g += sg * sa
                    b += sb * sa
                    a += sa
            samples = SUPERSAMPLE * SUPERSAMPLE
            if a == 0:
                pixels.append((0, 0, 0, 0))
            else:
                pixels.append(
                    (round(r / a), round(g / a), round(b / a), round(a / samples))
                )
    return pixels


def png_bytes(size: int, pixels: list) -> bytes:
    """An 8-bit RGBA PNG."""
    raw = bytearray()
    for row in range(size):
        raw.append(0)  # filter type: none
        for col in range(size):
            r, g, b, a = pixels[row * size + col]
            raw += bytes((r, g, b, a))

    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def main() -> None:
    os.makedirs("marks", exist_ok=True)
    for key in sorted(MARKS):
        data = png_bytes(SIZE, render(key, SIZE))
        with open(os.path.join("marks", f"{key}.png"), "wb") as handle:
            handle.write(data)
        print(f"wrote marks/{key}.png ({len(data)} bytes, {SIZE}px)")


if __name__ == "__main__":
    main()
