# PSoXide Arcade

**[Download the latest version on itch.io](https://bonnie-studios.itch.io/psoxide-demo-disc)** (it ships on the PSoXide Demo Disc), or [play it in your browser](https://bonnie-studios.itch.io/psoxide).

> **Largely written with agentic coding.** I direct the agents and test their work in two places: PSoXide's emulator, which profiles every cycle, and a real PlayStation, which shows me where the emulator is wrong. Working between them is where the accuracy and the speed come from. [How PSoXide is built](https://ebonura.github.io/PSoXide/how-its-built/)

PSoXide Arcade is a native PlayStation collection containing Breakout, Space
Invaders, and Magikarp Pong. Its cabinet-style selector keeps all three games
visible on one CRT, with a dedicated control deck for music and credits. The
selector and each game are separate PS-X EXE programs, so only the selected
game occupies runtime memory.

The repository was extracted with filtered history from PSoXide. The
[SDK](https://github.com/EBonura/PSoXide),
[engine](https://github.com/EBonura/PSoXide-editor/tree/main/engine) and
[emulator](https://github.com/EBonura/PSoXide-emulator) have separate
repositories; the three games and their collection release live here.
`components.lock.json` pins the exact revisions a standalone build uses, while
the demo disc explicitly supplies its tested split components.

## Build

Install Rust through rustup, Make, Python 3 with Pillow, host C/C++ build
tools and `mipsel-none-elf-objdump` on `PATH`. The checked-in toolchain selects
the nightly and Rust components. Clone into a path without spaces: the
Makefile does not quote paths.

```sh
git clone https://github.com/EBonura/psoxide-arcade.git
cd psoxide-arcade
python3 -m venv .venv
. .venv/bin/activate
python -m pip install Pillow
make disc
make check
```

`make disc` downloads the pinned SDK, engine and emulator sources from their
public GitHub repositories into ignored `.psoxide/`, builds all
five guest executables, checks instruction hazards and packs the collection.

The standalone image (one data track; the music is an XA file inside it) is
written to:

```text
dist/psoxide-arcade.cue
dist/psoxide-arcade.bin
```

Keep both files together and open the CUE in a PlayStation emulator. To use
the separate PSoXide emulator:

```sh
/path/to/PSoXide-emulator/target/release/frontend launch --path dist/psoxide-arcade.cue
```

To copy the disc into a PSoXide games library, run `make install GAMES_DIR="DIR"`
(or `tools/mkdisc/target/release/mkdisc install --cue dist/psoxide-arcade.cue
--games-dir DIR` for the disc you already built). The library copy is named
`PSoXide Arcade (psoxide-arcade).bin` and `.cue` (library names are
`Game Name (repo)`) and replaces the previous install in place; a leftover
`psoxide-arcade.bin`/`.cue` pair from before the rename is removed if it is
identical to or older than the new disc, and a newer, different one stops the
install.

Use `PSOXIDE_FROM=/path/to/PSoXide-editor` with an already bootstrapped editor
checkout to build against split SDK/engine sources while keeping the standalone
pin unchanged. `make run FRONTEND=/path/to/frontend` builds the disc and
then runs the same `launch` command.

The carousel geometry and PSXDEMO1 catalog/checksum format are shared
engine crates (`psx-carousel` and `psx-disc-toc`) in the locked `.psoxide`
hydration. `make psoxide` verifies the imported source receipt; the launcher,
loader and host packer no longer maintain separate collection-support copies.

## Controls

- Left / Right: choose a game or Credits
- Cross or Start: launch
- Up / Down: switch English / Italian
- In each game: use its title and pause menus

The standalone image carries Magikarp Pong's Goncharov as an XA-ADPCM file
(`MUSIC1.XA`, 37.8 kHz stereo). The game finds it in the root directory, loops
it with the SDK's XA player, and times the spectrum bars from the drive's own
position in the song. Goncharov is by
[magikAAAAArp](https://www.youtube.com/@magikAAAAArp/videos) and is used with
the band's permission. `arcade-assets/audio/README.md` says how the file was
made and how to re-author it.

## Hardware status

The chain-loader is inherited from the hardware-proven demo-disc path, but the
nested route still needs one owner-console burn: selector, all three launches,
Magikarp audio (XA, tested only in the emulator so far), and reset back to the
outer menu.

## Recent changes

**0.1.1**: the six sound effects are cooked with the PSoXide SDK's shared
SPU-ADPCM encoder. See the [changelog](CHANGELOG.md) for the remaining changes
and published download versions.

## PSoXide source components

`components.lock.json` pins the SDK, engine/editor and emulator libraries separately.
`make psoxide` verifies and materializes them into the ignored `.psoxide` directory.
The demo disc can pass `PSOXIDE_FROM` with a verified composite editor checkout.
Pass `FRONTEND=/path/to/PSoXide-emulator/target/release/frontend` to player helpers.

## Licence

The code is GPL-2.0-or-later; see [LICENSE](LICENSE). The Goncharov audio,
the magikAAAAArp artwork and the other assets listed in
[THIRD_PARTY.md](THIRD_PARTY.md) are not covered by that licence. Breakout,
Pong, Space Invaders and Magikarp are trademarks of their respective owners;
this project is not affiliated with or endorsed by them.
