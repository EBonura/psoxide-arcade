#!/usr/bin/env python3
"""Cook a screenshot into the disc's menu-panel format.

One shot is a 512-byte CLUT (256 RGB555 entries, little-endian) followed by
120x90 pixel indices, which is what `mkdisc --shot` presses and the
launcher uploads straight into VRAM to fill the box beside the description. Colours are
quantized to 256: exactly, when the downscaled frame already fits, and by
median cut with dithering when it does not.

Black maps to 0x8000 rather than 0x0000, because on the PlayStation a texel
of 0x0000 is transparent and a screenshot with holes in it reads as a bug.

Usage: cook-shots.py <in.png> <out.shot>
"""

import struct
import sys

from PIL import Image

W, H = 120, 90


def rgb555(r: int, g: int, b: int) -> int:
    v = (round(r * 31 / 255)) | (round(g * 31 / 255) << 5) | (round(b * 31 / 255) << 10)
    # 0x0000 is transparent on the console; force opaque black instead.
    return v if v else 0x8000


def main():
    src, out = sys.argv[1], sys.argv[2]
    im = Image.open(src).convert("RGB")
    if im.size != (W, H):
        if im.width % W == 0 and im.height % H == 0:
            im = im.resize((W, H), Image.NEAREST)
        else:
            im = im.resize((W, H), Image.LANCZOS)

    colors = im.getcolors(maxcolors=256)
    if colors:
        # Fits as-is: an exact palette, no dithering to muddy clean pixels.
        palette = [c for _, c in colors]
        lookup = {c: i for i, c in enumerate(palette)}
        indices = bytes(lookup[p] for p in im.getdata())
    else:
        q = im.convert("P", palette=Image.ADAPTIVE, colors=256)
        flat = q.getpalette()[: 256 * 3]
        palette = list(zip(flat[0::3], flat[1::3], flat[2::3]))
        indices = q.tobytes()

    clut = bytearray(512)
    for i, (r, g, b) in enumerate(palette):
        struct.pack_into("<H", clut, i * 2, rgb555(r, g, b))

    assert len(indices) == W * H
    with open(out, "wb") as f:
        f.write(clut)
        f.write(indices)
    print(f"{out}: {len(set(indices))} colours used")


if __name__ == "__main__":
    main()
