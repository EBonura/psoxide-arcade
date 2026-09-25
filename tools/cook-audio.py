#!/usr/bin/env python3
"""Cook assets/audio/freesfx/psau/*.psau from the Kronbits Free SFX archive (CC0).

usage: tools/cook-audio.py /path/to/FreeSFX.zip

Each clip is encoded at 44.1 kHz, peak 0.9, by the SDK's shared SPU-ADPCM
cooker (psx-audio-cook, built from the pinned .psoxide by tools/psx-audio-cook;
run `make psoxide` first). The archive is the one PSoXide's freesfx pack was
cooked from; its digest is checked so the source is the one the README names.
"""

import hashlib
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "audio" / "freesfx" / "psau"
COOK = ROOT / "tools" / "psx-audio-cook"
ARCHIVE_SHA256 = "3378feb1912e0081a7e3d07b7dd378e7e0a4ec4696c25d18b0edd4aae1330ac1"
RATE = 44_100
SOUNDS = {
    "ui_beep": "FreeSFX/GameSFX/Alarms Blip Beeps/Retro Beeep 06.wav",
    "hit_punch": "FreeSFX/GameSFX/Impact/Retro Impact Punch 07.wav",
    "hit_metal": "FreeSFX/GameSFX/Impact/Retro Impact Metal 36.wav",
    "pickup_coin": "FreeSFX/GameSFX/PickUp/Retro PickUp Coin 04.wav",
    "explosion_short": "FreeSFX/GameSFX/Explosion/Retro Explosion Short 01.wav",
    "swoosh": "FreeSFX/GameSFX/Swoosh/Retro Swooosh 02.wav",
}


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    archive = Path(sys.argv[1])
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if digest != ARCHIVE_SHA256:
        sys.exit(f"{archive}: sha256 {digest}, expected {ARCHIVE_SHA256}")
    subprocess.run(
        ["cargo", "build", "-q", "--release", "--manifest-path", str(COOK / "Cargo.toml")],
        check=True,
    )
    cook = COOK / "target" / "release" / "psx-audio-cook"
    with zipfile.ZipFile(archive) as zf, tempfile.TemporaryDirectory() as tmp:
        for name, member in SOUNDS.items():
            wav = Path(tmp) / f"{name}.wav"
            wav.write_bytes(zf.read(member))
            out = OUT / f"{name}.psau"
            result = subprocess.run(
                [str(cook), "encode", str(wav), str(out), "--rate", str(RATE), "--format", "psau"],
                check=True,
                capture_output=True,
                text=True,
            )
            print(f"{name}: {result.stdout.strip()}")


if __name__ == "__main__":
    main()
