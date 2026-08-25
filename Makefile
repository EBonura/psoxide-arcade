ROOT := $(CURDIR)
VERSION := 0.1.0
PSOXIDE := $(ROOT)/.psoxide
PSOXIDE_FROM ?=
BUILD := $(ROOT)/build
GUEST_BUILD := $(BUILD)/guest
OUT := $(GUEST_BUILD)/mipsel-sony-psx/release
DIST := $(ROOT)/dist
MKDISC := $(ROOT)/tools/mkdisc/target/release/mkdisc

PSX_TARGET := mipsel-sony-psx
PSX_FLAGS := --release --target $(PSX_TARGET) -Zjson-target-spec -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem
LINK_FLAGS := -Clink-arg=-T$(PSOXIDE)/sdk/psoxide.ld -Clink-arg=--oformat=binary

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

.PHONY: help psoxide loader launcher games mkdisc shots cards disc check run clean

help:
	@echo "make disc   - build the three games and standalone PSoXide Arcade disc"
	@echo "make check  - run host tests and build the full disc"
	@echo "make run    - boot the standalone collection in the PSoXide frontend"

psoxide:
	@if [ -n "$(PSOXIDE_FROM)" ]; then \
		cargo run -q --manifest-path "$(PSOXIDE_FROM)/tools/psoxide-link/Cargo.toml" -- \
			--from "$(PSOXIDE_FROM)" --into "$(PSOXIDE)"; \
	else \
		cargo run -q --manifest-path "$(ROOT)/psoxide-pin/Cargo.toml" -- "$(PSOXIDE)"; \
	fi

loader: psoxide
	cd loader && CARGO_TARGET_DIR=$(GUEST_BUILD) \
		RUSTFLAGS="-Clink-arg=-Tloader.ld -Clink-arg=--oformat=binary" \
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
		RUSTFLAGS="$(LINK_FLAGS)" cargo build $(PSX_FLAGS)

games: psoxide
	cd games/breakout && CARGO_TARGET_DIR=$(GUEST_BUILD) RUSTFLAGS="$(LINK_FLAGS)" cargo build $(PSX_FLAGS)
	cd games/invaders && CARGO_TARGET_DIR=$(GUEST_BUILD) RUSTFLAGS="$(LINK_FLAGS)" cargo build $(PSX_FLAGS)
	cd games/magikarp-pong && CARGO_TARGET_DIR=$(GUEST_BUILD) RUSTFLAGS="$(LINK_FLAGS)" cargo build $(PSX_FLAGS)

mkdisc: psoxide
	cd tools/mkdisc && cargo build --release

$(BUILD)/shots/%.shot: assets/shots/%.png tools/cook-shots.py
	@mkdir -p $(BUILD)/shots
	python3 tools/cook-shots.py "$<" "$@"

shots: $(SHOT_FILES)

disc: launcher games mkdisc shots
	@mkdir -p $(DIST)
	$(MKDISC) --launcher $(LAUNCHER_EXE) --out $(DIST)/psoxide-arcade.bin --volume PSXARCADE \
		--game "BREAKOUT=$(BREAKOUT_EXE)" \
		--game "SPACE INVADERS=$(INVADERS_EXE)" \
		--game "MAGIKAAAAARP PONG=$(MAGIKARP_EXE)" \
		--program-cdda "MAGIKAAAAARP PONG=$(ROOT)/arcade-assets/audio/goncharov.cdda" \
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
		--describe "MAGIKAAAAARP PONG=Pong with live CD audio and spectrum bars. Goncharov by magikAAAAArp.|Pong con audio CD e barre dello spettro. Goncharov dei magikAAAAArp."

check: psoxide
	cd disc-toc && cargo test
	cd carousel && cargo test
	cd tools/mkdisc && cargo test
	$(MAKE) disc

run: disc
	cd $(PSOXIDE)/emu && cargo run -p frontend --release -- launch --path $(DISC)

clean:
	rm -rf $(BUILD) $(DIST)
