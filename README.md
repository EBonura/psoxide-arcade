# PSoXide Arcade

PSoXide Arcade is a native PlayStation collection containing Breakout, Space
Invaders, and Magikarp Pong. The selector and each game are separate PS-X EXE
programs, so only the selected game occupies runtime memory.

The repository was extracted with filtered history from PSoXide. PSoXide is
still the pinned engine/SDK dependency; the concrete games and their release
disc live here.

## Build

```sh
make disc
```

The standalone mixed-mode image is written to:

```text
dist/psoxide-arcade.cue
dist/psoxide-arcade.bin
```

Use `PSOXIDE_FROM=/path/to/PSoXide` to build against a local checkout while
keeping the release pin unchanged.

## Controls

- Left / Right: choose a game
- Cross: launch
- Triangle: switch English / Italian descriptions
- In each game: use its title and pause menus

The standalone image owns Magikarp Pong's Goncharov CDDA track. On the full
PSoXide Demo Disc, GH-PSX is intentionally pointed at that same physical
track.

## Hardware status

The chain-loader and CDDA relocation are inherited from the hardware-proven
demo-disc path, but the new nested route still needs one owner-console burn:
selector, all three launches, Magikarp audio, GH-PSX borrowing, and reset back
to the outer menu.

