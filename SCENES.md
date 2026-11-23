# PSoXide Arcade on the scene core

How the Arcade (a launcher, three bare EXEs and a chain-load blob) maps onto the SDK's scene system
(resident core at 0x801FD700..0x801FFF00, handoff block at 0x801FFF00..0x80200000, `SCENES.TBL` at
member LBA 22). The row format, kinds and link rules are the core lane's CORE-ABI.md (campaign
`scene-system-1123`, day 0, 11-23); everything here is provisional with it. Line numbers are at commit
10b16ae of this repo; SDK lines are at the pin 6bfe478ff0e0 (`components.lock.json`). Sizes are from the
10b16ae ship maps (`make disc BUILD=build/ship/work DIST=build/ship`) unless they say otherwise.

Rule this follows (Manny, 11-23): scene boundaries only where the player already waits today, and every
in-game screen stays resident in the playing scene. The Arcade's one wait is the launch (warp, fade, load
bar), so the boundaries are exactly today's program switches: launcher to each game.

## What the player sees

| What the player sees | In the program | Resident in |
|---|---|---|
| Boot: no splash (no psx-splash dependency in `launcher/Cargo.toml`); DualShock-in-analog check | launcher `main` (`launcher/src/main.rs:285`) | scene 0 (launcher) |
| Launcher menu: three game cards plus a credits card, Konami code for gated entries (`main.rs:127`, `:371-433`), EN/IT toggle, links, music panel | launcher menu loop; cards and art compiled in | scene 0 |
| Launch: 40-frame warp with the swoosh (`WARP_FRAMES`, `main.rs:264`), 14 fade frames (`FADE_FRAMES`, `main.rs:261`) | launcher `boot()` (`main.rs:786-829`) | scene 0, then the switch |
| Loading bar (today the blob's 160-pixel bar, `loader/src/main.rs:6-25`; red checklist on a failed stage) | the blob at 0x801F0000 today; the core's switch on the core (static frame left on screen, CORE-ABI 3.3; fatal screen on failure, 3.1) | the switch (core) |
| Breakout: title and settings, play, pause panel, results with best score | `psx_engine::microgame` `MicrogameScreen::{Title, Playing, Paused, Results}` (SDK `engine/crates/psx-engine/src/microgame.rs:20-29`), `Phase` (`games/breakout/src/main.rs:141-146`) | scene 1 |
| Space Invaders: the same four screens, three difficulty levels | `games/invaders/src/main.rs:168-172`, microgame shell | scene 2 |
| Magikaaaaarp Pong: the same four screens, Goncharov playing as XA with spectrum bars | `games/magikarp-pong/src/main.rs:1049` (title), `music.rs:48-58` | scene 3 |
| Back to the launcher | none today (reset button); none on the core (CORE-ABI return scene 0xFFFF, Manny 11-23 "no return-to-launcher") | |

## Scene table rows

| id | kind | What it is | Fields that matter | return scene |
|---|---|---|---|---|
| 0 | 0 Program (boot scene: header 0x10 = 0) | `PSX.EXE`, the launcher | `t_addr` 0x80010000, `t_size` 106,496 (52 sectors) plus the stored core image, SP 0x801FD700, entry `__core_boot`, name `ARCADE` | 0xFFFF |
| 1 | 0 Program | `BREAKO00.EXE` | `t_size` 225,280 (110 sectors), SP 0x801FD700, name `BREAKOUT` | 0xFFFF |
| 2 | 0 Program | `SPACEI01.EXE` | `t_size` 241,664 (118 sectors), SP 0x801FD700, name `SPACE INVADERS` | 0xFFFF |
| 3 | 0 Program | `MAGIKA02.EXE` | `t_size` 348,160 (170 sectors), SP 0x801FD700, name `MAGIKAAAAARP PONG` | 0xFFFF |
| 4 | 2 Data **(provisional kind)** | `DEMOTOC.BIN`, 4 sectors, the launcher's card text and per-game entries (`psx-disc-toc`, magic `PSXDEMO4`) | LBA and sectors; its entries name scene ids 1..3 instead of `exe_lba` / `payload_fnv` (PLAN 3.7: "DEMOTOC.BIN keeps its presentation and refers to scene ids") | n/a |
| 5 | 2 Data **(provisional kind)** | `MUSIC1.XA`, 17,560 sectors, Pong's music | LBA and sectors, so Pong can skip its PVD and root reads (`games/magikarp-pong/src/music.rs:234-262`) | n/a |

Not a row: `SHOTS.BIN` (36 sectors at LBA 26). Nothing reads it today (the cards are compiled in; no
reader of `shots_lba` in `launcher/src`), so it stays a file or leaves the disc; separate issue.

On the demo disc the Arcade today is one nested member image. Under the core its four Programs become
rows of the demo disc's table (the demo launcher switching straight to the Arcade launcher at row 0 of
the member), with the member's `lba_offset` as today. All four must be on the same core hash as the
demo disc (CORE-ABI 4.4: a mismatched row is refused, 0x23).

## Shared code versus per scene

Nothing is shared at run time between the four rows: each is a whole program with its own copy of
psx-rt, psx-engine and the drivers, as today. Shared across rows: only the core (window) and the handoff
block. In source, the three games share the SDK's `psx_engine::microgame` shell and
`games/journey_state.rs`; the launcher shares `psx-disc-toc` with mkdisc.

Per scene: the launcher's art, fonts and SFX (all `include_bytes!`), each game's own code and assets.

## How the blob retires

Today `boot()` (`launcher/src/main.rs:786-829`) fades, copies `LOADER_BLOB` (14,040 B, `main.rs:36`)
to `LOADER_BASE` 0x801F0000 (`main.rs:46`), flushes the I-cache and jumps with
`(exe_lba, lba_offset, cdda_base, payload_fnv)` shifted by the launcher's `disc_base`. The blob
(`loader/`, the SDK's `psx-chainloader` `runtime::run` plus a 51-line `Presentation`) quiesces,
loads 64-sector chunks into 0x80010000, checks the FNV and enters the game with `PSXD` in `$a0`.

On the core, `boot()` keeps its fade and then calls `psx_core_switch(scene_id, args)` (CORE-ABI 3,
slot 0x801FD708). The core does the quiesce, chunked load, checksum and entry (CORE-ABI 3.1, 3.3). The
`loader/` crate, `LOADER_BLOB`, `LOADER_BASE`/`LOADER_LIMIT` and the launcher's `disc_base::shift_lba`
arithmetic go; mkdisc's `check_fits_below_loader` (`tools/mkdisc/src/main.rs:252`) becomes the scene
table's bound: `t_addr + t_size <= PAYLOAD_END` = 0x801FD300, the window minus the 1 KB switch stack the
core runs on at the top of the outgoing program's stack (CORE-ABI revision of 11-23, `psx-scene-abi`
`PAYLOAD_END` on SDK branch lane/disc-scenes-1123 50dfa6a7e). Pong, the largest, ends at 0x80065000.

## The relink (step 1, done)

`Makefile`: `CORE_RAM_SIZE := 0x1FD800` (one variable, marked provisional) is written to
`$(BUILD)/ram-size-$(CORE_RAM_SIZE).ld` and passed as `-Clink-arg=-T<that file>` before psoxide.ld in
`LINK_FLAGS`, which the launcher and all three games link with. The value is in the file name so a change
alters RUSTFLAGS and cargo relinks. The blob links by `loader/loader.ld` and is unchanged.

Measured on `make disc` against the 2046e80 builds kept by the 1122 lane
(`arcade-repin-1122/keep/2046e80a35e9/` exes and launcher map; game maps from
`arcade-repin-1122/repo/build/{breakout,invaders,pong}/`):

| Program | free RAM before | free RAM after | highest linked byte | header SP | exe bytes that differ |
|---|---|---|---|---|---|
| launcher | 1,884,792 | 1,874,552 | 0x8002BC88 | 0x801FFF00 to 0x801FD700 | 1 |
| breakout | 1,774,472 | 1,764,232 | 0x80046B78 | same | 1 |
| invaders | 1,757,764 | 1,747,524 | 0x8004ACBC | same | 1 |
| pong | 1,649,828 | 1,639,588 | 0x8006525C | same | 1 |

Free RAM is `psoxide-size ram` (equal to the harness size stage's figure); each falls by 10,240. text,
data and bss are unchanged in all four. Nothing is linked at or above 0x801FD700, and no program uses a
heap (`alloc` off: the exes differ only in the SP byte).

## Conflicts with CORE-ABI.md

- **`DEMOTOC.BIN` sits at LBA 22**, the LBA CORE-ABI 1 gives `SCENES.TBL`. It is fixed by
  `disc_toc::TOC_LBA = 22` (used at `launcher/src/main.rs:21`, `:768`) and written there by mkdisc. One of
  them has to move (or `SCENES.TBL` absorbs DEMOTOC's presentation fields). The same holds for the demo
  disc launcher, which uses the same crate.
- **The launcher's `PSX.EXE` is at LBA 62**, after DEMOTOC and SHOTS.BIN, not at 23. CORE-ABI wants the
  boot exe at 23, so mkdisc's layout changes.
- **The blob region and the new stack reserve.** The blob is copied to 0x801F0000..0x801F36D8 while the
  launcher still runs on its own stack. After the relink the launcher's stack reserve is
  0x801F5700..0x801FD700 (was 0x801F7F00..0x801FFF00). The blob still clears it (0x801F36D8 is 8,232 B
  under the reserve), so the default build is safe, but the 32 KiB blob budget (`LOADER_LIMIT`,
  `main.rs:50`; loader.ld:17-18) now overlaps the reserve by 10,496 B (it was 256 B, the smell the audit
  noted), and `loader/loader.ld:12-14` ("0x801F8000 .. 0x80200000 stack") is now wrong. Not changed here,
  since the blob retires on the core; until then a blob over 22,272 B would reach the launcher's stack.
- **The window is never touched by the blob path today:** the blob enters each game with the game's
  header SP (now 0x801FD700) and loads only below 0x801F0000. On the core, a blob-launched game would be a
  cold entry (`PSXD` in `$a0`, CORE-ABI 4.3 step 2) and would re-copy its own core: correct, but the blob
  must be gone before the first landing so there is one CD owner.
- `__core_boot` then `_start`: fine for all four (psx-rt `_start` takes `$sp` from the header and reads
  `$a0..$a2`, SDK `psx-rt/src/lib.rs:164-172`, `:199`).
- The launcher masks nothing before copying the blob (audit section 7); `psx_core_switch` masks
  interrupts itself (CORE-ABI 3.1), so that difference disappears.

## What the core replaces (step 3): TODO, no code changed

| What | Where | Bytes (10b16ae maps) |
|---|---|---|
| the chain-load blob, embedded | `LOADER_BLOB` (`launcher/src/main.rs:36`), `.rodata.$anon...24` in the launcher's .data | 14,040 data |
| the blob's copy and jump | `launcher::boot` (`main.rs:786-829`; the fade stays) | 1,060 text (part) |
| the `loader/` crate | `loader/src/main.rs`, `loader/loader.ld` (built into the blob above) | in the 14,040 |
| launcher's CD reader | `psx_io::cd::reader::SectorReader` (`read_sector` 1,100, `send_command_waiting` 772, `ReaderPause` 704 + 612 + 492, `ack_unexpected` 472, `stop` 256) and `psx_io::cd` helpers 144; `launcher::READER` | 4,552 text + 2,064 data |
| the TOC read | `read_toc` (`main.rs:760-784`, inlined); `TOC_SECTOR` stays as the destination of a `psx_core_read` | 0 (buffer kept) |
| Pong's CD reader | `SectorReader` in `game-magikaaaaaarp-pong` | 6,456 text |
| Pong's PVD and root lookup | `find_song`, `read_one` (`games/magikarp-pong/src/music.rs:234-270`), with `psx_io::iso` | part of 4,176 text |
| Pong's XA player | `psx_io::cd::xa` | 2,740 text, **stays** (PLAN 3.1: the XA player stays in each program) |
| splash | none in any of the four | 0 |

Launcher total that leaves: about 20,656 B (14,040 + 4,552 + 2,064) plus part of `boot()`; Pong about
6,456 B plus part of `find_song`. None of it can go behind a `scene-core` feature before the core
exists: retiring the blob means calling `psx_core_switch`, and replacing the readers means calling
`psx_core_read`, and no `psx-core` crate, `psx-core-abi.ld` or core image exists to link (CORE-ABI 4.1).
A feature that only removed them would leave a launcher that cannot start a game. The edits for when
`psx-core` lands:

1. `Makefile`: add `-Clink-arg=-T$(PSOXIDE)/sdk/psoxide-core.ld` after the ram-size file in `LINK_FLAGS`;
   launcher and game `Cargo.toml`s depend on `psx-core` (feature `link`) behind `scene-core`, off by
   default; the `loader` target and `LOADER_BLOB` env drop out of the `scene-core` build.
2. `launcher/src/main.rs:786-829`: under `scene-core`, after the fade, `psx_core_switch(entry.scene, &[0; 4])`
   in place of the copy and jump; on a refusal code, show it (the launcher is still intact).
3. `launcher/src/main.rs:760-784` and `:280`: under `scene-core`, `read_toc` reads its 4 sectors with
   `psx_core_read` into `TOC_SECTOR`; `READER` goes.
4. `games/magikarp-pong/src/music.rs:234-270`: under `scene-core`, take `MUSIC1.XA`'s LBA from its Data row
   (or keep the root walk through `psx_core_read`); the `SectorReader` goes, the XA player stays.
5. `tools/mkdisc`: write `SCENES.TBL` at 22 and the launcher at 23, move DEMOTOC (conflict above), and put
   scene ids in DEMOTOC entries.
6. No return row: games keep no way back, as today.

## Load speed (Manny 11-23: every load the same speed or faster)

No code or data that is resident today becomes a disc read: the four programs are already separate
loads, and the switch replaces the blob's load with the core's.

| Transition | Disc read under the core | Measured today | Slower? |
|---|---|---|---|
| Boot to launcher menu | BIOS loads the launcher, 52 payload sectors today plus the stored core image (about 4 to 5 sectors at the 8,192 to 10,240 B cap, **estimate**), then DEMOTOC's 4 sectors | `launcher` checkpoint at frame 74 on PCSX-ReARMed, 657 on PCSX-Redux (2046e80 PASS json, `cross`); journey `carousel` 56 ticks (baseline). Boot to first frame alone: not measured | **yes, slightly (estimate):** about 5 more BIOS-load sectors (about 33 ms at double speed) and `__core_boot`'s copy and FNV. Kept smallest by the core's 8,192 B target |
| Launcher to Breakout / Invaders / Pong | the game's payload, 110 / 118 / 170 sectors today | launching to title 139 / 142 / 179 frames on ReARMed, 163 / 166 / 209 on Redux, including the 40 warp and 14 fade frames and the game's init (PASS json `cross`); journeys `launch-breakout` 454, `launch-invaders` 462, `launch-pong` 521 ticks (baseline); the load alone is not measured | **at risk, by about 5 sectors per switch:** each game's payload also stores its own copy of the core after `.data` (CORE-ABI 4.1), which a core entry never uses, and the scene-table writer on lane/disc-scenes-1123 sets a row's `t_size` to the whole payload (`crates/psx-iso/src/scenes.rs:329-353`) (it leaves the window alone, 4.3 step 2). If the row's `t_size` covers it, every switch reads about 5 more sectors than today. What keeps it equal: a row `t_size` that stops before the stored core image (place `.psx_core`'s LMA last and have mkdisc write the shorter size), so a core entry loads only what today's blob loads. Against that, the core's 256-sector chunks (PLAN load-speed gate) replace the blob's 64-sector chunks, and the blob copy (14,040 B) goes, which should make the switch faster; not measured |
| Pong music start | today PVD (LBA 16) and root reads, then XA streams | not measured | not slower; faster if the Data row gives the LBA (the two lookups go) |
| Every in-game screen (title, pause, results) | none | none (all in RAM) | no |

## Tests, bars and goldens that move

| Check | Moves? |
|---|---|
| `[[size]]` ship, breakout, invaders, pong (tolerance 0 on text and free) | yes, all four fail: free is 10,240 lower in each. text, data, bss unchanged. Needs a re-bless |
| `[maps]` hashes | yes, all four maps change (override line, new path) |
| disc and install facts (`psoxide-arcade.bin` sha, DEMOTOC `payload_fnv` per game) | yes: each game exe changes by one byte, so its FNV in DEMOTOC changes |
| journeys `carousel`, `launch-*`, `pad-refusal` | not expected: no symbol moves (the launch journeys read the launcher and game maps at fixed addresses). Not run |
| speed windows (three tapes from the launcher) | not expected: no code or data moves, the R3000 has no data cache. Not measured. The 1122 lane already found 2046e80 slower than the installed fce1 build (invaders +7.2%, pong +8.4% work per frame, `arcade-repin-1122/NOTES.md`), unrelated to this change |
| `[cross]` | not expected. Not run |
| goldens, pgo layouts | none (`layout = false`) |
| host tests (`psx-disc-toc`, mkdisc, check-mfc0) | unchanged by the relink; the step 3 edits change mkdisc's layout tests and DEMOTOC's format |
