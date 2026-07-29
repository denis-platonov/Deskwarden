#!/usr/bin/env python3
"""Generates deskwarden.ico, the application/tray icon.

The icon is generated rather than drawn by hand so it is reproducible and
reviewable as source: there is no binary asset here whose provenance or
licensing anyone has to take on trust. It is deliberately a simple placeholder
-- a shield with a keyhole, in a neutral indigo -- and is *not* derived from
Bitwarden's branding (this project is unofficial and unaffiliated; borrowing
their mark would imply an endorsement that does not exist).

Run from this directory:

    python generate-icon.py

Output: deskwarden.ico, containing 16/32/48/64 px uncompressed BGRA entries
plus a PNG-compressed 256 px entry (supported by Windows Vista and later,
which is well below this app's Windows 10 floor). Everything is written with
the standard library only -- no Pillow, no external tooling.
"""

import math
import struct
import zlib

# Shield fill and glyph colors, as (r, g, b).
SHIELD_RGB = (59, 91, 219)
GLYPH_RGB = (255, 255, 255)

# Supersampling factor per axis. 4 means each output pixel is the average of
# 16 samples, which is what keeps the curved shield edge from looking ragged
# at 16 px.
SUPERSAMPLE = 4

SIZES = [16, 32, 48, 64]
PNG_SIZE = 256


def inside_shield(x: float, y: float) -> bool:
    """True if the normalized point (0..1, 0..1) is inside the shield body."""
    if not (0.10 <= y <= 0.94):
        return False

    # Straight-sided upper section with rounded top corners.
    left, right = 0.16, 0.84
    if y <= 0.52:
        radius = 0.10
        if y < 0.10 + radius:
            for corner_x in (left + radius, right - radius):
                if (x < left + radius and corner_x == left + radius) or (
                    x > right - radius and corner_x == right - radius
                ):
                    dx = x - corner_x
                    dy = y - (0.10 + radius)
                    return dx * dx + dy * dy <= radius * radius
        return left <= x <= right

    # Lower section tapering to a point at the bottom center, on an elliptical
    # profile so the sides curve in rather than forming a flat wedge.
    t = (y - 0.52) / (0.94 - 0.52)
    half_width = 0.34 * math.sqrt(max(0.0, 1.0 - t * t))
    return abs(x - 0.5) <= half_width


def inside_keyhole(x: float, y: float) -> bool:
    """True if the normalized point falls in the keyhole cut out of the shield."""
    # Bow of the keyhole.
    dx, dy = x - 0.5, y - 0.42
    if dx * dx + dy * dy <= 0.115 * 0.115:
        return True
    # Stem, widening slightly towards the bottom.
    if 0.42 <= y <= 0.70:
        t = (y - 0.42) / (0.70 - 0.42)
        half_width = 0.045 + 0.035 * t
        return abs(x - 0.5) <= half_width
    return False


def sample(x: float, y: float):
    """Color of the normalized point as (r, g, b, a)."""
    if not inside_shield(x, y):
        return (0, 0, 0, 0)
    if inside_keyhole(x, y):
        return (*GLYPH_RGB, 255)
    return (*SHIELD_RGB, 255)


def render(size: int) -> list:
    """Renders `size`x`size` pixels as a flat list of (r, g, b, a) tuples."""
    pixels = []
    step = 1.0 / (size * SUPERSAMPLE)
    for py in range(size):
        for px in range(size):
            r = g = b = a = 0
            for sy in range(SUPERSAMPLE):
                for sx in range(SUPERSAMPLE):
                    x = (px * SUPERSAMPLE + sx + 0.5) * step
                    y = (py * SUPERSAMPLE + sy + 0.5) * step
                    sr, sg, sb, sa = sample(x, y)
                    # Premultiplied accumulation, so transparent samples don't
                    # drag the color towards black along the edges.
                    r += sr * sa
                    g += sg * sa
                    b += sb * sa
                    a += sa
            samples = SUPERSAMPLE * SUPERSAMPLE
            if a == 0:
                pixels.append((0, 0, 0, 0))
            else:
                pixels.append((round(r / a), round(g / a), round(b / a), round(a / samples)))
    return pixels


def bmp_entry(size: int, pixels: list) -> bytes:
    """An uncompressed 32-bit BGRA icon image (BITMAPINFOHEADER + rows + mask)."""
    header = struct.pack(
        "<IiiHHIIiiII",
        40,          # biSize
        size,        # biWidth
        size * 2,    # biHeight: doubled, per the ICO format's XOR+AND convention
        1,           # biPlanes
        32,          # biBitCount
        0,           # biCompression (BI_RGB)
        0,           # biSizeImage
        0, 0,        # pixels-per-meter
        0, 0,        # palette
    )
    body = bytearray()
    for row in range(size - 1, -1, -1):  # bottom-up
        for col in range(size):
            r, g, b, a = pixels[row * size + col]
            body += bytes((b, g, r, a))
    # AND mask: unused for 32-bit icons (the alpha channel carries
    # transparency) but still required to be present, one bit per pixel with
    # rows padded to 4 bytes.
    mask_row_bytes = ((size + 31) // 32) * 4
    body += bytes(mask_row_bytes * size)
    return header + bytes(body)


def png_entry(size: int, pixels: list) -> bytes:
    """A PNG-compressed icon image, for the 256 px entry."""
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

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # 8-bit RGBA
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def main() -> None:
    images = [(size, bmp_entry(size, render(size))) for size in SIZES]
    images.append((PNG_SIZE, png_entry(PNG_SIZE, render(PNG_SIZE))))

    out = bytearray(struct.pack("<HHH", 0, 1, len(images)))  # ICONDIR
    offset = 6 + 16 * len(images)
    for size, data in images:
        out += struct.pack(
            "<BBBBHHII",
            size if size < 256 else 0,  # 0 means 256 in the ICO format
            size if size < 256 else 0,
            0,            # colors in palette
            0,            # reserved
            1,            # planes
            32,           # bits per pixel
            len(data),
            offset,
        )
        offset += len(data)
    for _, data in images:
        out += data

    with open("deskwarden.ico", "wb") as handle:
        handle.write(bytes(out))
    print(f"wrote deskwarden.ico ({len(out)} bytes, {len(images)} sizes)")


if __name__ == "__main__":
    main()
