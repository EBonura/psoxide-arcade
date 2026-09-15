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
[SDK](https://github.com/EBonura/PSoXide) and
[engine](https://github.com/EBonura/PSoXide-editor/tree/main/engine) now have
separate repositories; the three games and their collection release live here.
The standalone `psoxide-pin/` retains a reproducible historical dependency,
while the demo disc explicitly supplies its tested split components.

## Build

Install Rust through rustup, Make, Python 3 with Pillow, host C/C++ build
tools and `mipsel-none-elf-objdump` on `PATH`. The checked-in toolchain selects
the nightly and Rust components. Access to this repository is required while
it remains private.

```sh
git clone https://github.com/EBonura/psoxide-arcade.git
cd psoxide-arcade
python3 -m venv .venv
. .venv/bin/activate
python -m pip install Pillow
make disc
make check
```

`make disc` hydrates the pinned SDK/engine into ignored `.psoxide/`, builds all
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
pin unchanged. The legacy `make run` target builds the frontend from the
historical hydrated source; the explicit command above also works with split
source overrides.

## Controls

- Left / Right: choose a game or Credits
- Cross or Start: launch
- Up / Down: switch English / Italian
- L1 / R1: previous / next music track, including Music Off
- In each game: use its title and pause menus

The standalone image owns Magikarp Pong's Goncharov CDDA track. Goncharov is
by [magikAAAAArp](https://www.youtube.com/@magikAAAAArp/videos) and is used
with the band's permission. On the full PSoXide Demo Disc, GH-PSX is
intentionally pointed at that same physical track.

## Hardware status

The chain-loader and CDDA relocation are inherited from the hardware-proven
demo-disc path, but the new nested route still needs one owner-console burn:
selector, all three launches, Magikarp audio, GH-PSX borrowing, and reset back
to the outer menu.

## Recent changes

Source snapshot **2026.09.05**: Documented the separate SDK/engine dependencies and collection build.
See the [changelog](CHANGELOG.md) for the remaining changes and published download versions.

## PSoXide source components

`components.lock.json` pins the SDK, engine/editor and emulator libraries separately.
`make psoxide` verifies and materializes them into the ignored `.psoxide` directory.
The demo disc can pass `PSOXIDE_FROM` with a verified composite editor checkout.
Pass `FRONTEND=/path/to/PSoXide-emulator/target/release/frontend` to player helpers.
