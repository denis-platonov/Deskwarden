#!/usr/bin/env python3
"""Generates deskwarden.ico, the application/tray icon.

The icon is generated rather than drawn by hand so it is reproducible and
reviewable as source: there is no binary asset here whose provenance or
licensing anyone has to take on trust. The mark is the quartered shield from
the design document committed at docs/design/Deskwarden.dc.html (section 3g): four
quadrants in four values of one blue -- one quarter per vault kind (logins,
passkeys, cards, notes) -- reading as a single blue shield at icon size. It
is *not* derived from Bitwarden's branding (this project is unofficial and
unaffiliated; borrowing their mark would imply an endorsement that does not
exist).

Run from this directory:

    python generate-icon.py

Output: deskwarden.ico, containing 16/32/48/64 px uncompressed BGRA entries
plus a PNG-compressed 256 px entry (supported by Windows Vista and later,
which is well below this app's Windows 10 floor). Everything is written with
the standard library only -- no Pillow, no external tooling.
"""

import struct
import zlib

# Quadrant fills, as (r, g, b): top-left, top-right, bottom-left,
# bottom-right. Same hex values as theme.rs's BLUE_DEEP / BLUE / BLUE_BRIGHT
# / BLUE_SOFT -- kept in sync by hand (one is Rust, one is Python).
QUADRANT_RGB = {
    "tl": (0x14, 0x30, 0x7A),
    "tr": (0x1B, 0x3F, 0xA0),
    "bl": (0x3B, 0x74, 0xE8),
    "br": (0x7F, 0xA4, 0xEF),
}

# Supersampling factor per axis. 4 means each output pixel is the average of
# 16 samples, which is what keeps the curved shield edge from looking ragged
# at 16 px.
SUPERSAMPLE = 4

SIZES = [16, 32, 48, 64]
PNG_SIZE = 256


# The design's SVG viewbox is 24 wide by 28 tall; all the geometry below is
# in that coordinate space. Shield body: x in [2, 22], top edge at y=2 with
# rounded corners of radius 2.4, straight sides down to y=14, then two
# mirrored cubic curves meeting at the bottom point (12, 26).
CORNER_RADIUS = 2.4

# The bottom-left edge, from the design's `C6.6 23.2, 3.2 19.4, 2 14` cubic:
# P0=(12,26) -> P3=(2,14) with control points (6.6,23.2) and (3.2,19.4). The
# bottom-right edge is its mirror around x=12.
_BOTTOM_CUBIC = ((12.0, 26.0), (6.6, 23.2), (3.2, 19.4), (2.0, 14.0))


def _cubic_point(t: float):
    """Point on the bottom-left cubic at parameter t (0 = bottom tip)."""
    (x0, y0), (x1, y1), (x2, y2), (x3, y3) = _BOTTOM_CUBIC
    u = 1.0 - t
    x = u * u * u * x0 + 3 * u * u * t * x1 + 3 * u * t * t * x2 + t * t * t * x3
    y = u * u * u * y0 + 3 * u * u * t * y1 + 3 * u * t * t * y2 + t * t * t * y3
    return x, y


def _left_edge_at(y: float) -> float:
    """X of the shield's left edge at height y in the curved lower section.

    y(t) on the cubic is monotonic (26 down to 14), so plain bisection on t
    converges; 40 iterations is far below a supersampled pixel at 256 px.
    """
    lo, hi = 0.0, 1.0
    for _ in range(40):
        mid = (lo + hi) / 2.0
        _, my = _cubic_point(mid)
        if my > y:
            lo = mid
        else:
            hi = mid
    x, _ = _cubic_point((lo + hi) / 2.0)
    return x


def inside_shield(sx: float, sy: float) -> bool:
    """True if the viewbox-space point is inside the quartered shield body."""
    if not (2.0 <= sy <= 26.0):
        return False

    # Upper section: a rectangle with two rounded top corners.
    if sy <= 14.0:
        if not (2.0 <= sx <= 22.0):
            return False
        if sy < 2.0 + CORNER_RADIUS:
            for cx in (2.0 + CORNER_RADIUS, 22.0 - CORNER_RADIUS):
                near_left = sx < 2.0 + CORNER_RADIUS and cx < 12.0
                near_right = sx > 22.0 - CORNER_RADIUS and cx > 12.0
                if near_left or near_right:
                    dx, dy = sx - cx, sy - (2.0 + CORNER_RADIUS)
                    return dx * dx + dy * dy <= CORNER_RADIUS * CORNER_RADIUS
        return True

    # Lower section: between the two mirrored cubics.
    left = _left_edge_at(sy)
    return left <= sx <= 24.0 - left


def sample(x: float, y: float):
    """Color of the normalized (0..1, 0..1) point as (r, g, b, a)."""
    sx, sy = x * 24.0, y * 28.0
    if not inside_shield(sx, sy):
        return (0, 0, 0, 0)
    key = ("t" if sy < 14.0 else "b") + ("l" if sx < 12.0 else "r")
    return (*QUADRANT_RGB[key], 255)


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
