#!/usr/bin/env python3
"""Generates every visual asset the Microsoft Store package needs, from the
app's own icon geometry (deskwarden/assets/generate-icon.py). Nothing is
hand-drawn and nothing is downloaded: standard library only, like that script.

    python scripts/store/make-assets.py OUT_DIR

Writes, under OUT_DIR:

  Assets/                the MSIX tile and app-list logos, at every scale and
                         target size AppxManifest.xml's VisualElements needs
                         (referenced there by their un-suffixed base names)
  listing/               the Partner Center "Store logos" (1:1 300x300 and
                         1080x1080, 2:3 poster 720x1080) and the 71x71 icon

The mark is the quartered blue shield, transparent around it. Wide and
splash images carry the shield centred on a transparent field; the manifest
supplies the tile background colour. No Bitwarden artwork is used anywhere:
this project is unofficial and the Store listing must not imply otherwise
(docs/store/microsoft/listing.md, "Trademark").

Large canvases render the shield at a smaller size and place it, so the whole
set takes a couple of minutes of pure-Python rasterising rather than an hour.
"""

import importlib.util
import os
import struct
import sys
import zlib

sys.dont_write_bytecode = True  # importing the icon script must not leave a __pycache__ in the tree

HERE = os.path.dirname(os.path.abspath(__file__))
ICON_SCRIPT = os.path.join(HERE, "..", "..", "deskwarden", "assets", "generate-icon.py")

spec = importlib.util.spec_from_file_location("generate_icon", ICON_SCRIPT)
icon = importlib.util.module_from_spec(spec)
spec.loader.exec_module(icon)

_cache = {}


def shield(size):
    """The shield as size x size RGBA pixels (cached). Supersampling drops to
    2 for big renders; at that size the edge is smooth anyway."""
    if size not in _cache:
        icon.SUPERSAMPLE = 4 if size <= 256 else 2
        _cache[size] = icon.render(size)
    return _cache[size]


def png(width, height, pixels):
    raw = bytearray()
    for row in range(height):
        raw.append(0)
        for col in range(width):
            raw += bytes(pixels[row * width + col])

    def chunk(kind, data):
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def canvas(width, height, fraction):
    """A transparent width x height canvas with the shield centred on it, its
    HEIGHT being `fraction` of the shorter side (the mark is 24:28, taller
    than wide, so height is the limiting dimension)."""
    side = max(8, round(min(width, height) * fraction))
    mark = shield(side)
    out = [(0, 0, 0, 0)] * (width * height)
    ox, oy = (width - side) // 2, (height - side) // 2
    for y in range(side):
        base = (oy + y) * width + ox
        out[base : base + side] = mark[y * side : (y + 1) * side]
    return out


def write(path, width, height, fraction):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(png(width, height, canvas(width, height, fraction)))
    print(f"  {os.path.relpath(path, OUT)}  {width}x{height}")


SCALES = (100, 125, 150, 200, 400)


def scaled(base_w, base_h):
    return [(s, round(base_w * s / 100), round(base_h * s / 100)) for s in SCALES]


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    OUT = os.path.abspath(sys.argv[1])
    assets = os.path.join(OUT, "Assets")

    # name, base size, shield fraction of the shorter side
    for name, w, h, frac in (
        ("StoreLogo", 50, 50, 0.84),
        ("Square44x44Logo", 44, 44, 0.90),
        ("Square71x71Logo", 71, 71, 0.70),
        ("Square150x150Logo", 150, 150, 0.62),
        ("Wide310x150Logo", 310, 150, 0.70),
        ("Square310x310Logo", 310, 310, 0.60),
        ("SplashScreen", 620, 300, 0.50),
    ):
        # The un-suffixed base file is what makeappx validates the manifest
        # against; it is the scale-100 image.
        write(os.path.join(assets, f"{name}.png"), w, h, frac)
        for scale, sw, sh in scaled(w, h):
            write(os.path.join(assets, f"{name}.scale-{scale}.png"), sw, sh, frac)

    # The taskbar / Start list icon at fixed target sizes, in both the plated
    # and unplated forms (unplated: no tile-colour plate behind the glyph).
    for size in (16, 24, 32, 48, 256):
        write(os.path.join(assets, f"Square44x44Logo.targetsize-{size}.png"), size, size, 0.96)
        write(
            os.path.join(assets, f"Square44x44Logo.targetsize-{size}_altform-unplated.png"),
            size,
            size,
            0.96,
        )

    # Partner Center -> Store listing -> Store logos.
    listing = os.path.join(OUT, "listing")
    write(os.path.join(listing, "store-logo-1x1-300.png"), 300, 300, 0.80)
    write(os.path.join(listing, "store-logo-1x1-1080.png"), 1080, 1080, 0.55)
    write(os.path.join(listing, "store-logo-2x3-720x1080.png"), 720, 1080, 0.55)
    write(os.path.join(listing, "store-logo-71.png"), 71, 71, 0.80)
    print("done")
