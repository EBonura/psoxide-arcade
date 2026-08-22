#!/usr/bin/env python3
"""Cook the three Arcade selector thumbnails into one PS1 4bpp strip.

Each card owns a 16-colour CLUT while sharing one texture page. This preserves
the very different source palettes at less than half the footprint of one
8bpp strip. Black is stored as opaque RGB555 black because 0x0000 is
transparent on PlayStation.
"""

import argparse
import struct
from pathlib import Path

from PIL import Image


CARD_W = 72
CARD_H = 54


def rgb555(r: int, g: int, b: int) -> int:
    value = (
        round(r * 31 / 255)
        | (round(g * 31 / 255) << 5)
        | (round(b * 31 / 255) << 10)
    )
    return value if value else 0x8000


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tex", type=Path, required=True)
    parser.add_argument("--clut", type=Path, required=True)
    parser.add_argument("--preview", type=Path, required=True)
    parser.add_argument("images", nargs=3, type=Path)
    args = parser.parse_args()

    preview = Image.new("RGB", (CARD_W * 3, CARD_H))
    card_indices: list[bytes] = []
    clut = bytearray(3 * 16 * 2)
    for slot, path in enumerate(args.images):
        image = Image.open(path).convert("RGB").resize((CARD_W, CARD_H), Image.Resampling.NEAREST)
        quantized = image.quantize(
            colors=16,
            method=Image.Quantize.MEDIANCUT,
            dither=Image.Dither.NONE,
        )
        card_indices.append(quantized.tobytes())
        preview.paste(quantized.convert("RGB"), (slot * CARD_W, 0))
        palette_bytes = quantized.getpalette()[: 16 * 3]
        palette = list(zip(palette_bytes[0::3], palette_bytes[1::3], palette_bytes[2::3]))
        for index, (r, g, b) in enumerate(palette):
            struct.pack_into("<H", clut, (slot * 16 + index) * 2, rgb555(r, g, b))

    # PS1 4bpp stores the left pixel in the low nibble and the right pixel in
    # the high nibble. Pack across the whole 216-pixel row so card boundaries
    # stay naturally aligned.
    pixels = bytearray(CARD_W * 3 * CARD_H // 2)
    out = 0
    for y in range(CARD_H):
        row = b"".join(
            indices[y * CARD_W : (y + 1) * CARD_W] for indices in card_indices
        )
        for x in range(0, len(row), 2):
            pixels[out] = row[x] | (row[x + 1] << 4)
            out += 1

    args.tex.parent.mkdir(parents=True, exist_ok=True)
    args.clut.parent.mkdir(parents=True, exist_ok=True)
    args.preview.parent.mkdir(parents=True, exist_ok=True)
    args.tex.write_bytes(pixels)
    args.clut.write_bytes(clut)
    preview.resize((CARD_W * 9, CARD_H * 3), Image.Resampling.NEAREST).save(args.preview)

    print(f"{args.tex}: three independent 16-colour cards, {len(pixels)} texture bytes")


if __name__ == "__main__":
    main()
