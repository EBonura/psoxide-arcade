# Third-party material

The source code in this repository is licensed under GPL-2.0-or-later (see
`LICENSE`). The files below are not. They belong to their respective rights
holders and are included only so the collection can be built. They are not
licensed for reuse under the GPL.

## Goncharov (magikAAAAArp)

Goncharov is by [magikAAAAArp](https://www.youtube.com/@magikAAAAArp/videos)
and is used with the band's permission.

- `arcade-assets/audio/goncharov.cdda`: the CD-DA audio track.
- `games/magikarp-pong/assets/goncharov_spectrum_16x30hz.bin`: a spectrum
  table computed from that track.

## magikAAAAArp artwork and Magikarp Pong images

- `games/magikarp-pong/vendor/magikaaaaaarp_album.jpg` and its cooked form
  `games/magikarp-pong/assets/magikaaaaaarp_album.psxt`: magikAAAAArp album
  artwork.
- `games/magikarp-pong/vendor/score_flyby.png` and its cooked form
  `games/magikarp-pong/assets/score_flyby.psxt`.

## Sound effects

`assets/audio/freesfx/psau/*.psau` are cooked from the Kronbits Free SFX
archive (sha256 `3378feb1912e0081a7e3d07b7dd378e7e0a4ec4696c25d18b0edd4aae1330ac1`),
released as CC0. Each clip is encoded at 44.1 kHz, peak 0.9, with
`tools/psx-audio-cook encode <clip.wav> <out.psau> --rate 44100 --format psau`
(the SDK's shared SPU-ADPCM cooker). The source clip for each file, by path
inside the archive:

| file | clip |
|---|---|
| `ui_beep.psau` | `FreeSFX/GameSFX/Alarms Blip Beeps/Retro Beeep 06.wav` |
| `hit_punch.psau` | `FreeSFX/GameSFX/Impact/Retro Impact Punch 07.wav` |
| `hit_metal.psau` | `FreeSFX/GameSFX/Impact/Retro Impact Metal 36.wav` |
| `pickup_coin.psau` | `FreeSFX/GameSFX/PickUp/Retro PickUp Coin 04.wav` |
| `explosion_short.psau` | `FreeSFX/GameSFX/Explosion/Retro Explosion Short 01.wav` |
| `swoosh.psau` | `FreeSFX/GameSFX/Swoosh/Retro Swooosh 02.wav` |

## Names

Breakout, Pong and Space Invaders are trademarks of their respective owners.
Magikarp is a trademark of its respective owners. magikAAAAArp is the band's
name. PlayStation is a trademark of Sony Interactive Entertainment. This
project is not affiliated with or endorsed by any of them, and the names are
used only to describe the games.
