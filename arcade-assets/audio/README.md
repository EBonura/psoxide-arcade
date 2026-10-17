# Goncharov (XA)

`xa/MUSIC1.XA` is Goncharov, the song Magikarp Pong plays, as an XA-ADPCM file:
37.8 kHz stereo, single speed, 17,560 sectors (the song's 233.92 s plus the
SDK's end guard). Goncharov is by
[magikAAAAArp](https://www.youtube.com/@magikAAAAArp/videos) and is used with
the band's permission. `xa/songs.rs` is the table the game compiles in (file
name, number, sectors, the song's channel and length) and `xa/music.json` is the
encoder's report. Both are written by the encoder, not by hand.

The source is the raw 44.1 kHz stereo track also kept as PSoXide-editor's
`assets/audio/cdda/GONCHAROV.track02.cdda` (sha1
`6601dd8e87d11089db5b1466df785459b7a25944`, the bytes this repository carried as
`goncharov.cdda` until the XA move). To re-author:

```sh
ffmpeg -f s16le -ar 44100 -ac 2 -i GONCHAROV.track02.cdda goncharov.wav
make music GONCHAROV_WAV=goncharov.wav XA_COOK=/path/to/psx-audio-cook
```

`XA_COOK` is a `psx-audio-cook` built with the `xa-pack` subcommand (on the SDK's
main since 2026-10-16). This file was packed with a build of SDK 27ed9a4f, whose
encoder core is the same, and re-packing from the editor copy of the track with
the SDK at 5219afbe gives the same bytes (checked 2026-10-17: `MUSIC1.XA`,
`music.json` and `songs.rs` identical). `make music` also bakes the spectrum
table from the same WAV, and `make music-check` fails if any committed byte
changes. A fresh clone and the disc build use the committed files and need
neither the WAV nor the encoder. The encoder scores the file at 40.3 dB (left)
and 40.6 dB (right) SNR against the source.

The game reads the disc once, before the first note: the drive is busy while a
song plays, so nothing else is read from the disc during the game.
