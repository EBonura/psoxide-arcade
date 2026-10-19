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
PANIC_RUSTFLAGS = $(if $(filter-out 0,$(PSOXIDE_PANIC_MESSAGES)),,-Zunstable-options -Cpanic=immediate-abort)
LINK_FLAGS := -Cllvm-args=-disable-mips-df-backward-search -Clink-arg=-T$(PSOXIDE)/sdk/psoxide.ld -Clink-arg=--oformat=binary

LOADER_BLOB := $(OUT)/loader.exe
LAUNCHER_EXE := $(OUT)/launcher.exe
BREAKOUT_EXE := $(OUT)/game-breakout.exe
INVADERS_EXE := $(OUT)/game-invaders.exe
MAGIKARP_EXE := $(OUT)/game-magikaaaaaarp-pong.exe
DISC := $(DIST)/psoxide-arcade.cue

SHOT_NAMES := breakout breakout2 invaders invaders2 pong pong2
SHOT_FILES := $(foreach name,$(SHOT_NAMES),$(BUILD)/shots/$(name).shot)
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

.PHONY: help psoxide loader launcher games mkdisc shots cards music disc check run clean

help:
	@echo "make disc   - build the three games and standalone PSoXide Arcade disc"
	@echo "make check  - run host tests and build the full disc"
	@echo "make run    - boot the standalone collection in the PSoXide frontend"

# components.lock.json pins the SDK, editor/engine and emulator revisions.
# psoxide-components (built from the SDK's tools/psoxide-link) imports them
# into .psoxide so the path dependencies and the linker script resolve; an
# unchanged lock is verified against its receipt and not fetched again. The
# tool is installed once per revision under target/. COMPONENTS_REV is the
# lock's SDK revision (the psoxide-components binary first exists at e6ec267).
# PSOXIDE_FROM=/path/to/tree overrides the lock with a working tree.
COMPONENTS_REV := 3cf22179d8798e8ce5899743fdcf222fc9486a8a
COMPONENTS := $(ROOT)/target/psoxide-components/$(COMPONENTS_REV)
HAZARD_SCAN := cargo run -q --release --locked --manifest-path $(PSOXIDE)/Cargo.toml -p psoxide-hazard --bin hazard-scan --
# The SDK linker script leaves the flat file short of the sector boundary its
# header claims; hazard-patch (with the link map) appends those zeros and
# reroutes any load-delay hazard, so every EXE that goes on the disc passes it.
HAZARD_PATCH := cargo run -q --release --locked --manifest-path $(PSOXIDE)/Cargo.toml -p psoxide-hazard --bin hazard-patch --

psoxide:
	@if [ -n "$(PSOXIDE_FROM)" ]; then \
		cargo run -q --manifest-path "$(PSOXIDE_FROM)/tools/psoxide-link/Cargo.toml" -- \
			--from "$(PSOXIDE_FROM)" --into "$(PSOXIDE)"; \
	else \
		grep -q '$(COMPONENTS_REV)' "$(ROOT)/components.lock.json" || { echo "COMPONENTS_REV is not the lock's SDK revision"; exit 1; }; \
		[ -x "$(COMPONENTS)/bin/psoxide-components" ] || cargo install -q --locked \
			--git https://github.com/EBonura/PSoXide --rev $(COMPONENTS_REV) --root "$(COMPONENTS)" psoxide-link; \
		"$(COMPONENTS)/bin/psoxide-components" --root "$(PSOXIDE)" --lock "$(ROOT)/components.lock.json"; \
	fi

loader: psoxide
	cd loader && CARGO_TARGET_DIR=$(GUEST_BUILD) \
		RUSTFLAGS="$(PANIC_RUSTFLAGS) -Cllvm-args=-disable-mips-df-backward-search -Clink-arg=-Tloader.ld -Clink-arg=--oformat=binary" \
		cargo build $(PSX_FLAGS)

$(CARD_STAMP): $(CARD_SOURCES) tools/cook-arcade-cards.py
	@mkdir -p $(BUILD)/cards
	python3 tools/cook-arcade-cards.py --tex $(CARD_TEX) --clut $(CARD_CLUT) \
		--preview $(CARD_PREVIEW) $(CARD_SOURCES)
	@touch $(CARD_STAMP)

cards: $(CARD_STAMP)

launcher: loader cards
	cd launcher && CARGO_TARGET_DIR=$(GUEST_BUILD) \
		LOADER_BLOB=$(LOADER_BLOB) DISC_VERSION=v$(VERSION) \
		ARCADE_CARDS_TEX=$(CARD_TEX) ARCADE_CARDS_CLUT=$(CARD_CLUT) \
		RUSTFLAGS="$(PANIC_RUSTFLAGS) $(LINK_FLAGS) -Clink-arg=-Map=$(OUT)/launcher.map" cargo build $(PSX_FLAGS)

games: psoxide
	cd games/breakout && CARGO_TARGET_DIR=$(GUEST_BUILD) RUSTFLAGS="$(PANIC_RUSTFLAGS) $(LINK_FLAGS) -Clink-arg=-Map=$(OUT)/game-breakout.map" cargo build $(PSX_FLAGS)
	cd games/invaders && CARGO_TARGET_DIR=$(GUEST_BUILD) RUSTFLAGS="$(PANIC_RUSTFLAGS) $(LINK_FLAGS) -Clink-arg=-Map=$(OUT)/game-invaders.map" cargo build $(PSX_FLAGS)
	cd games/magikarp-pong && CARGO_TARGET_DIR=$(GUEST_BUILD) RUSTFLAGS="$(PANIC_RUSTFLAGS) $(LINK_FLAGS) -Clink-arg=-Map=$(OUT)/game-magikaaaaaarp-pong.map" cargo build $(PSX_FLAGS)

mkdisc: psoxide
	cd tools/mkdisc && cargo build --release

$(BUILD)/shots/%.shot: assets/shots/%.png tools/cook-shots.py
	@mkdir -p $(BUILD)/shots
	python3 tools/cook-shots.py "$<" "$@"

shots: $(SHOT_FILES)

disc: launcher games mkdisc shots
	@for exe in $(LAUNCHER_EXE) $(BREAKOUT_EXE) $(INVADERS_EXE) $(MAGIKARP_EXE); do $(HAZARD_PATCH) "$$exe" --map "$${exe%.exe}.map" || exit 1; done
	@for exe in $(LOADER_BLOB) $(LAUNCHER_EXE) $(BREAKOUT_EXE) $(INVADERS_EXE) $(MAGIKARP_EXE); do $(HAZARD_SCAN) "$$exe" || exit 1; done
	@mkdir -p $(DIST)
	$(MKDISC) --launcher $(LAUNCHER_EXE) --out $(DIST)/psoxide-arcade.bin --volume PSXARCADE \
		--game "BREAKOUT=$(BREAKOUT_EXE)" \
		--game "SPACE INVADERS=$(INVADERS_EXE)" \
		--game "MAGIKAAAAARP PONG=$(MAGIKARP_EXE)" \
		--xa $(MUSIC_XA) \
		--shot "BREAKOUT=$(BUILD)/shots/breakout.shot" \
		--shot "BREAKOUT=$(BUILD)/shots/breakout2.shot" \
		--shot "SPACE INVADERS=$(BUILD)/shots/invaders.shot" \
		--shot "SPACE INVADERS=$(BUILD)/shots/invaders2.shot" \
		--shot "MAGIKAAAAARP PONG=$(BUILD)/shots/pong.shot" \
		--shot "MAGIKAAAAARP PONG=$(BUILD)/shots/pong2.shot" \
		--version-of "BREAKOUT=0.1.0" \
		--version-of "SPACE INVADERS=0.1.0" \
		--version-of "MAGIKAAAAARP PONG=0.1.0" \
		--describe "BREAKOUT=A complete Breakout clone with responsive controls, collision, sound, scoring and a persistent high score.|Un clone completo di Breakout con controlli reattivi, collisioni, audio, punti e record persistente." \
		--describe "SPACE INVADERS=A complete Space Invaders clone with marching formations, shields, enemy fire, scoring and three difficulty levels.|Un clone completo di Space Invaders con formazioni, scudi, fuoco nemico, punti e tre livelli di difficolta." \
		--describe "MAGIKAAAAARP PONG=Pong with XA music and spectrum bars. Goncharov by magikAAAAArp.|Pong con musica XA e barre dello spettro. Goncharov dei magikAAAAArp."

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
