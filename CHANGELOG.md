# Changelog

## Unreleased

- Magikarp Pong's Goncharov is an XA-ADPCM file inside the data track instead of
  a CD-DA track, looped by the SDK's XA player, with the spectrum bars timed
  from the drive's position in the song (the table is baked from the full
  track, 7017 frames). The image is one data track and no audio tracks.
- The selector and the host packer drop their CD-DA code (the standalone
  collection never had a menu track).
- Repinned the SDK, engine and emulator to revisions on their mains (SDK
  4e97cf3; the old pin, cc668b56b, was never pushed), ported the three games
  to the current engine drawing API, and imported the locked components with
  `psoxide-components` instead of the Python script.

## 0.1.1

- The six sound effects are cooked with the PSoXide SDK's shared SPU-ADPCM
  encoder (SDK a156a2f), each within 0.6 dB of the level it shipped at.

## Source 2026.09.05

This source snapshot is tagged `source-2026.09.05`. Download versions are
listed separately below; source cleanup does not replace an already published disc.

- Documented the separate SDK/engine dependencies and collection build.
- Protected all five guest executables against R3000 load-delay hazards.
- Cleaned selector/loader warnings and declared loader handoff register ownership.

## split.20260905 | 2026-09-05

Standalone collection published on itch.io.

- Published the cabinet selector and Breakout, Space Invaders and Magikarp Pong with CD audio.
