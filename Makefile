ROOT := $(CURDIR)
VERSION := 0.1.1
PSOXIDE := $(ROOT)/.psoxide
FRONTEND ?= frontend
PSOXIDE_FROM ?=
BUILD := $(ROOT)/build
GUEST_BUILD := $(BUILD)/guest
OUT := $(GUEST_BUILD)/mipsel-sony-psx/release
DIST := $(ROOT)/dist
MKDISC := $(ROOT)/tools/mkdisc/target/release/mkdisc

PSX_TARGET := mipsel-sony-psx
PSX_FLAGS := --release --target $(PSX_TARGET) -Zjson-target-spec -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem
# A panic is a BREAK at the faulting instruction (-Cpanic=immediate-abort): core::fmt, the panic Location records and
# their file names stay out of the image, and psx-rt's exception handler records Cause/EPC/BadVAddr
# (__psx_rt_fault_*; `psoxide-size faults` reads them from a RAM dump). PSOXIDE_PANIC_MESSAGES=1 restores the
# formatted panic.
PANIC_RUSTFLAGS = $(if $(filter-out 0,$(PSOXIDE_PANIC_MESSAGES)),,"-Zunstable-options", "-Cpanic=immediate-abort",)

# Every program runs on the SDK's scene core (SCENES.md, CORE-ABI 4.1): linked with the RAM top below the
# core's window (0x1FE000: STACK_INIT 0x801FDF00, the window 0x801FDF00..0x801FFF00, the handoff block
# above it), then psoxide-core.ld (the core's stored image and `__core_boot`), the launcher also
# psoxide-boot.ld (the boot splash in boot-only memory), then psoxide.ld. Sections run apart from where
# they are stored, so each link is an ELF and the SDK's build driver writes the PS-X EXE from it by load
# address. The scripts go in a cargo config file each program's build reads (`psoxide-pgo apply` refuses
# a RUSTFLAGS environment; config rustflags lists join).
CORE_RAM_SIZE := 0x1FE000
RAM_SIZE_LD := $(BUILD)/ram-size-$(CORE_RAM_SIZE).ld
LINK_SCRIPTS = "-Clink-arg=-T$(RAM_SIZE_LD)", "-Clink-arg=-T$(PSOXIDE)/sdk/psoxide-core.ld",$(1) "-Clink-arg=-T$(PSOXIDE)/sdk/psoxide.ld"
GUEST_RUSTFLAGS = [$(PANIC_RUSTFLAGS) "-Cllvm-args=-disable-mips-df-backward-search", $(call LINK_SCRIPTS,$(1))]
BOOT_SCRIPT = "-Clink-arg=-T$(PSOXIDE)/sdk/psoxide-boot.ld",
GAME_CONFIG := $(BUILD)/game-link.toml
LAUNCHER_CONFIG := $(BUILD)/launcher-link.toml

# FEATURES: cargo features of the launcher and the three games (`journey-state` is the test disc's
# read-only observation; the shipping build has none). Each variant wants its own BUILD directory
# (psoxide-test's two builds pass BUILD and DIST).
FEATURES ?=
FEATURE_FLAGS = $(if $(strip $(FEATURES)),--features $(strip $(FEATURES)))

# The build goes through the SDK's shared driver (psoxide-pgo apply, variant off: no profile and no
# layout): it compiles under its own psoxide-rustc (the checkout directory stays out of the crate
# hashes), writes the PS-X EXE from the ELF, runs the hazard patcher and scanner, and stamps the
# architecture gate's receipt (psx-gate.toml) into the image, which a collection disc's press checks.
# Each program builds in its own target directory, so the newest map in its psoxide-pgo-maps is its
# own; the EXE and map are copied to OUT under the names the journeys and the harness read.
PGO := $(PSOXIDE)/target/release/psoxide-pgo
# $(call APPLY,<crate dir>,<cargo bin name>,<config>,<extra env>)
define APPLY
	cd $(1) && $(4) CARGO_TARGET_DIR=$(GUEST_BUILD)/$(2) PSOXIDE=$(PSOXIDE) $(PGO) apply --crate . --variant off -- \
		build $(PSX_FLAGS) --config $(3) $(FEATURE_FLAGS)
	@mkdir -p $(OUT)
	cp $(GUEST_BUILD)/$(2)/$(PSX_TARGET)/release/$(2).exe $(OUT)/$(2).exe
	cp "$$(/bin/ls -t $(GUEST_BUILD)/$(2)/$(PSX_TARGET)/psoxide-pgo-maps/*.map | head -1)" $(OUT)/$(2).map
endef

LAUNCHER_EXE := $(OUT)/launcher.exe
BREAKOUT_EXE := $(OUT)/game-breakout.exe
INVADERS_EXE := $(OUT)/game-invaders.exe
MAGIKARP_EXE := $(OUT)/game-magikaaaaaarp-pong.exe
DISC := $(DIST)/psoxide-arcade.cue
# The launcher's table of contents (`mkdisc toc`), compiled into it.
TOC_BIN := $(BUILD)/DEMOTOC.BIN

CARD_TEX := $(BUILD)/cards/arcade-cards.tex
CARD_CLUT := $(BUILD)/cards/arcade-cards.clut
CARD_PREVIEW := $(BUILD)/cards/arcade-cards.png
CARD_STAMP := $(BUILD)/cards/.cooked
CARD_SOURCES := assets/shots/breakout.png assets/shots/invaders.png assets/shots/pong.png

# Goncharov, packed as XA (see arcade-assets/audio/README.md). The file and the
# table the game compiles in are committed; `make music` re-authors them.
MUSIC_DIR := $(ROOT)/arcade-assets/audio/xa
MUSIC_XA := $(MUSIC_DIR)/MUSIC1.XA
SPECTRUM_BIN := $(ROOT)/games/magikarp-pong/assets/goncharov_spectrum_16x30hz.bin

.PHONY: help psoxide sdk-tools link-configs launcher games mkdisc toc cards music disc install check run clean

help:
	@echo "make disc   - build the three games and standalone PSoXide Arcade disc"
	@echo "make check  - run host tests and build the full disc"
	@echo "make install GAMES_DIR=DIR - build the disc and install it into DIR as 'PSoXide Arcade (psoxide-arcade)'"
	@echo "make run    - boot the standalone collection in the PSoXide frontend"

# components.lock.json pins the SDK, editor/engine and emulator revisions.
# psoxide-components (built from the SDK's tools/psoxide-link) imports them
# into .psoxide so the path dependencies and the linker script resolve; an
# unchanged lock is verified against its receipt and not fetched again. The
# tool is installed once per revision under target/. COMPONENTS_REV is the
# lock's SDK revision (the psoxide-components binary first exists at e6ec267).
# PSOXIDE_FROM=/path/to/tree overrides the lock with a working tree.
COMPONENTS_REV := 5688e778aa6708c054203eb5d2fc544dcbd6ef3d
COMPONENTS := $(ROOT)/target/psoxide-components/$(COMPONENTS_REV)
# The SDK's build driver and its repro wrapper, built once per pin under .psoxide/target.
sdk-tools: psoxide
	cargo build -q --release --locked --manifest-path $(PSOXIDE)/Cargo.toml -p psoxide-pgo --bin psoxide-pgo --bin psoxide-rustc

$(CARD_STAMP): $(CARD_SOURCES) tools/cook-arcade-cards.py
	@mkdir -p $(BUILD)/cards
	python3 tools/cook-arcade-cards.py --tex $(CARD_TEX) --clut $(CARD_CLUT) \
		--preview $(CARD_PREVIEW) $(CARD_SOURCES)
	@touch $(CARD_STAMP)

cards: $(CARD_STAMP)

$(RAM_SIZE_LD):
	@mkdir -p $(BUILD)
	printf 'RAM_SIZE_OVERRIDE = %s;\n' $(CORE_RAM_SIZE) > $@

# Rewritten on every build: the paths and the panic policy are inputs (cargo reruns when a flag changes).
link-configs: $(RAM_SIZE_LD)
	printf '[target.%s]\nrustflags = %s\n' $(PSX_TARGET) '$(call GUEST_RUSTFLAGS,)' > $(GAME_CONFIG)
	printf '[target.%s]\nrustflags = %s\n' $(PSX_TARGET) '$(call GUEST_RUSTFLAGS,$(BOOT_SCRIPT))' > $(LAUNCHER_CONFIG)

mkdisc: psoxide
	cd tools/mkdisc && cargo build --release

# The launcher's card text: one entry per game, in scene order (scenes.manifest: game i is scene i + 1).
toc: mkdisc
	@mkdir -p $(BUILD)
	$(MKDISC) toc --out $(TOC_BIN) \
		--game "BREAKOUT" \
		--game "SPACE INVADERS" \
		--game "MAGIKAAAAARP PONG" \
		--version-of "BREAKOUT=0.1.0" \
		--version-of "SPACE INVADERS=0.1.0" \
		--version-of "MAGIKAAAAARP PONG=0.1.0" \
		--describe "BREAKOUT=A complete Breakout clone with responsive controls, collision, sound, scoring and a persistent high score.|Un clone completo di Breakout con controlli reattivi, collisioni, audio, punti e record persistente." \
		--describe "SPACE INVADERS=A complete Space Invaders clone with marching formations, shields, enemy fire, scoring and three difficulty levels.|Un clone completo di Space Invaders con formazioni, scudi, fuoco nemico, punti e tre livelli di difficolta." \
		--describe "MAGIKAAAAARP PONG=Pong with XA music and spectrum bars. Goncharov by magikAAAAArp.|Pong con musica XA e barre dello spettro. Goncharov dei magikAAAAArp."

launcher: sdk-tools cards toc link-configs
	$(call APPLY,launcher,launcher,$(LAUNCHER_CONFIG),DISC_VERSION=v$(VERSION) ARCADE_TOC=$(TOC_BIN) ARCADE_CARDS_TEX=$(CARD_TEX) ARCADE_CARDS_CLUT=$(CARD_CLUT))

games: sdk-tools link-configs
	$(call APPLY,games/breakout,game-breakout,$(GAME_CONFIG),)
	$(call APPLY,games/invaders,game-invaders,$(GAME_CONFIG),)
	$(call APPLY,games/magikarp-pong,game-magikaaaaaarp-pong,$(GAME_CONFIG),)

# The scene disc, pressed by the SDK's mkisopsx (scenes.manifest): SCENES.TBL at LBA 22, the launcher
# (image 0, scene 0) at 23, the three games as Programs of the same table, MUSIC1.XA as a root file
# Magikaaaaarp Pong finds by name. Every program is checked against the SDK's prebuilt core image,
# and the launcher's gate receipt against psx-gate.toml.
CORE_IMAGE := $(PSOXIDE)/sdk/crates/psx-core/prebuilt/psx-core.bin
disc: launcher games
	cp scenes.manifest $(OUT)/scenes.manifest
	@mkdir -p $(DIST)
	cd $(PSOXIDE)/tools/mkisopsx && cargo run -q --release -- \
		--exe $(LAUNCHER_EXE) --out $(DIST)/psoxide-arcade.bin --volume PSXARCADE \
		--gate-file $(ROOT)/psx-gate.toml \
		--scenes $(OUT)/scenes.manifest --core-image $(CORE_IMAGE) \
		--xa-file $(MUSIC_XA)

# Install into a PSoXide games library under its library name, replacing the previous install in
# place. Quoted: the library name has spaces and parentheses. `mkdisc install --cue dist/psoxide-arcade.cue
# --games-dir DIR` does the same without rebuilding.
install: disc
	@test -n "$(strip $(GAMES_DIR))" || { echo "Set GAMES_DIR to the games library directory."; exit 1; }
	$(MKDISC) install --cue "$(DISC)" --games-dir "$(GAMES_DIR)"

# Re-author the music from the Goncharov WAV (44.1 kHz stereo, 16-bit):
#   make music GONCHAROV_WAV=/path/goncharov.wav XA_COOK=/path/psx-audio-cook
# XA_COOK is a psx-audio-cook with `xa-pack` (on the SDK's main since
# 2026-10-16; a build of any later psx-audio-cook gives the same bytes).
# The spectrum table is baked from the same WAV by psoxide-dev.
# `make music-check` (same arguments) cooks again and fails if any committed
# byte changes. A fresh clone and the disc build use the committed files and
# need neither the WAV nor XA_COOK.
music: psoxide
	@test -f "$(GONCHAROV_WAV)" -a -x "$(XA_COOK)" || { echo "music: set GONCHAROV_WAV (the song) and XA_COOK (a psx-audio-cook with xa-pack)"; exit 1; }
	$(XA_COOK) xa-pack "$(MUSIC_DIR)" MUSIC "$(GONCHAROV_WAV)" --manifest "$(MUSIC_DIR)/music.json" --rust "$(MUSIC_DIR)/songs.rs"
	cargo run -q --release --locked --manifest-path $(PSOXIDE)/Cargo.toml -p psoxide-dev --bin psoxide-dev -- \
		bake-spectrum "$(GONCHAROV_WAV)" -o "$(SPECTRUM_BIN)" --fps 30 --bands 16

.PHONY: music-check
music-check: music
	@git diff --exit-code -- "$(MUSIC_DIR)" "$(SPECTRUM_BIN)" && echo "music-check: the cook is byte-identical to the committed music"

check: psoxide
	cargo test --manifest-path .psoxide/engine/Cargo.toml -p psx-disc-toc
	cargo test --manifest-path .psoxide/engine/Cargo.toml -p psx-carousel
	cd tools/mkdisc && cargo test
	$(MAKE) disc

run: disc
	"$(FRONTEND)" launch --path $(DISC)

clean:
	rm -rf $(BUILD) $(DIST)
