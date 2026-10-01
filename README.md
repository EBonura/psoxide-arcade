# PSoXide Arcade

Start with the [PSoXide Demo Disc](https://bonnie-studios.itch.io/psoxide-demo-disc): it includes PSoXide Arcade
and the other Bonnie Studios PlayStation demos. Standalone downloads are available
for testing just this collection.

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

The standalone mixed-mode image is written to:

```text
dist/psoxide-arcade.cue
dist/psoxide-arcade.bin
```

Keep both files together and open the CUE in a PlayStation emulator. To use
the separate PSoXide emulator:

```sh
/path/to/PSoXide-emulator/target/release/frontend launch --path dist/psoxide-arcade.cue
```

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
- L1 / R1: previous / next music track, including Music Off
- In each game: use its title and pause menus

The standalone image owns Magikarp Pong's Goncharov CDDA track. Goncharov is
by [magikAAAAArp](https://www.youtube.com/@magikAAAAArp/videos) and is used
with the band's permission.

## Hardware status

The chain-loader and CDDA relocation are inherited from the hardware-proven
demo-disc path, but the new nested route still needs one owner-console burn:
selector, all three launches, Magikarp audio, and reset back to the outer
menu.

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
