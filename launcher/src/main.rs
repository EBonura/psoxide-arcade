//! PSoXide Arcade boot menu.
//!
//! Reads the disc's table of contents ([`disc_toc`]), presents the collection
//! as a dedicated arcade cabinet, and chain-loads the selected game. The outer
//! PSoXide Demo Disc already owns the carousel metaphor; this nested launcher
//! deliberately uses its own CRT, cabinet, jukebox, and game-bank language.
//!
//! The chain-load itself cannot happen here: every PSoXide program links to
//! `0x80010000`, which is where this launcher is running, so streaming a game
//! in would overwrite the code doing the streaming. Instead the `loader` crate
//! is built separately at a high base address, embedded below as raw bytes,
//! copied up, and jumped to. See `loader/loader.ld`.

#![no_std]
#![no_main]

extern crate psx_rt;

mod paint;

use disc_toc::{Entry, Header, MAX_ENTRIES, TOC_BYTES, TOC_LBA};
use psx_font::{
    fonts::{BASIC, SPLEEN_5X8},
    FontAtlas,
};
use psx_gpu::{self as gpu, display::Resolution, display::VideoMode, framebuf::FrameBuffer};
use psx_io::disc_base;
use psx_pack::cd::{SectorReader, SECTOR_WORDS};
use psx_pad::{button, require_analog_port1, AnalogRequirement, ButtonState, PadMode, PadReader};
use psx_rt::tty;
use psx_sfx::{Bank, OneShot, Player};
use psx_spu::{Adsr, Pitch, SpuAddr, Voice, Volume};
use psx_vram::{Clut, TextureDepth, TexturePage};

/// The chain-load blob, linked at `LOADER_BASE` by `loader/loader.ld`.
const LOADER_BLOB: &[u8] = include_bytes!(env!("LOADER_BLOB"));

/// Which pressing this is: `git describe` at build time, on screen in the
/// header so every photo and recording says which disc is in the console.
const DISC_VERSION: &str = match option_env!("DISC_VERSION") {
    Some(v) => v,
    None => "DEV",
};

/// Where the blob expects to run. Must match `loader.ld`.
const LOADER_BASE: u32 = 0x801F_0000;

/// Blob budget, also from `loader.ld`. Overrunning it would walk into the
/// stack, so check rather than trust.
const LOADER_LIMIT: usize = 32 * 1024;

/// Frames between attempts to put a pad that is not in analog mode there.
/// Each attempt holds the CPU for a few frames (the commands are spaced the
/// way Sony's libpad spaces them), so not every frame.
const ANALOG_RECHECK_TICKS: u32 = 60;

const FONT_TPAGE: TexturePage = TexturePage::new(320, 0, TextureDepth::Bit4);
const FONT_CLUT: Clut = Clut::new(320, 256);

/// The header's own font: same eight-pixel height as BASIC but five wide, so
/// the music column fits beside a centred mark instead of running under it.
const SMALL_TPAGE: TexturePage = TexturePage::new(448, 0, TextureDepth::Bit4);
const SMALL_CLUT: Clut = Clut::new(416, 256);

/// The banner sits in its own 4bpp page, clear of both framebuffers and of
/// the font. Tpage X must be a multiple of 64.
const BANNER_TPAGE: TexturePage = TexturePage::new(384, 0, TextureDepth::Bit4);
const BANNER_CLUT: Clut = Clut::new(400, 256);
const BANNER_W: i16 = 120;
const BANNER_H: i16 = 19;
static BANNER_TEX: &[u8] = include_bytes!("../assets/banner.tex");
static BANNER_CLUT_DATA: &[u8] = include_bytes!("../assets/banner.clut");

/// The link marks ride the banner's page, below the mark: a 4bpp page is 256
/// rows deep and the banner uses nineteen. Their own CLUT, because the ramp
/// they need is grey and the banner's sixteen colours are the logo's.
const ICONS_V: u8 = 24;
const ICONS_CLUT: Clut = Clut::new(400, 257);
const ICONS_CELL: i16 = 12;
const ICONS_SIZE: i16 = 12;
/// Cells in the cooked strip. Not `LINKS.len()`: the strip carries every
/// mark tools/icons.py cooks (YouTube included, currently undrawn), and the
/// upload must match the file or `upload_bytes` panics at boot.
const ICONS_COUNT: i16 = 6;
static ICONS_TEX: &[u8] = include_bytes!("../assets/icons.tex");
static ICONS_CLUT_DATA: &[u8] = include_bytes!("../assets/icons.clut");

/// Three always-resident 4bpp game thumbnails, packed into one texture page.
/// Each owns its own sixteen-colour palette, so Breakout, Invaders and Pong
/// retain their distinct original colours without runtime CLUT rewrites.
const CARDS_TPAGE: TexturePage = TexturePage::new(512, 256, TextureDepth::Bit4);
const CARDS_CLUTS: [Clut; 3] = [
    Clut::new(512, 508),
    Clut::new(528, 508),
    Clut::new(544, 508),
];
const CARD_ART_W: i16 = 72;
const CARD_ART_H: i16 = 54;
static CARDS_TEX: &[u8] = include_bytes!(env!("ARCADE_CARDS_TEX"));
static CARDS_CLUT_DATA: &[u8] = include_bytes!(env!("ARCADE_CARDS_CLUT"));

/// Which mark leads the row, and the URL written out in full. Full because
/// the whole point of the card is that someone photographs or retypes these;
/// the first cut showed only the part after the slash and read as "which
/// site is this?".
const LINKS: [(i16, &str); 6] = [
    (0, "github.com/EBonura/PSoXide"),
    (1, "bonnie-studios.itch.io"),
    (2, "x.com/_bonniestudios"),
    (3, "instagram.com/izzy88izzy"),
    (4, "buymeacoffee.com/bonniestudios"),
    (5, "youtube.com/@magikAAAAArp/videos"),
];

const TITLE: (u8, u8, u8) = (244, 244, 236);
const NOW_PLAYING: (u8, u8, u8) = (151, 174, 255);
const TRACK_NAME: (u8, u8, u8) = (255, 218, 66);
const BLURB: (u8, u8, u8) = (238, 240, 235);
const LABEL: (u8, u8, u8) = (248, 248, 240);
const FAR_LABEL: (u8, u8, u8) = (154, 169, 226);
const ERROR: (u8, u8, u8) = (255, 214, 90);

/// The control-deck entry that shows credits instead of running something.
const CREDITS_NAME: &str = "CREDITS AND LINKS";

/// The reveal sequence for gated entries: up, up, down, down, left, right,
/// left, right, circle, cross. Konami's, because a demo disc hiding builds
/// behind anything else would be missing the point.
const KONAMI: [u16; 10] = [
    button::UP,
    button::UP,
    button::DOWN,
    button::DOWN,
    button::LEFT,
    button::RIGHT,
    button::LEFT,
    button::RIGHT,
    button::CIRCLE,
    button::CROSS,
];

/// Buttons the sequence tracker watches. Anything else a player presses is
/// none of its business and does not reset progress.
const KONAMI_BUTTONS: [u16; 6] = [
    button::UP,
    button::DOWN,
    button::LEFT,
    button::RIGHT,
    button::CIRCLE,
    button::CROSS,
];

const DESC_LEADING: i16 = 9;

/// Capcom Arcade Stadium-inspired game-select screen. Three game cards remain
/// visible together inside the cabinet monitor, while credits and the jukebox
/// sit on the physical control deck below it.
const BANNER_Y: i16 = 3;
const DISC_VERSION_X: i16 = 7;
const DISC_VERSION_Y: i16 = 10;
const LANGUAGE_X: i16 = 303;
const LANGUAGE_Y: i16 = 10;
const GAME_SELECT_Y: i16 = 37;
const GAME_CARD_X: [i16; 3] = [38, 124, 210];
const GAME_CARD_Y: i16 = 49;
const GAME_CARD_W: i16 = 80;
const GAME_CARD_H: i16 = 84;
const SELECTED_TITLE_Y: i16 = 140;
const SELECTED_VERSION_Y: i16 = 153;
const JUKE_X: i16 = 30;
const JUKE_Y: i16 = 196;
const JUKE_W: i16 = 198;
const JUKE_H: i16 = 34;
const MUSIC_X: i16 = JUKE_X + 6;
const METER_X: i16 = JUKE_X + JUKE_W - 68;
const MUSIC_TOP: i16 = JUKE_Y + 4;
const TRACK_TOP: i16 = JUKE_Y + 13;
const METER_BASE: i16 = JUKE_Y + 29;
const CREDITS_KEY_X: i16 = 236;
const CREDITS_KEY_Y: i16 = JUKE_Y;
const CREDITS_KEY_W: i16 = 56;
const CREDITS_KEY_H: i16 = 30;

/// Credits take over the cabinet/info band but retain the same bank keys.
const CARD_W: i16 = 212;
const CARD_X: i16 = (320 - CARD_W) / 2;
/// Characters a card line holds at the 5-pixel font.
const CARD_COLS: usize = ((CARD_W - 8) / 5) as usize;
/// Rows the credits box holds at the description leading.
const CRED_ROWS: i16 = 5;
const CRED_Y: i16 = 38;
const CRED_H: i16 = CRED_ROWS * DESC_LEADING + 7;
const LINKS_Y: i16 = CRED_Y + CRED_H + 2;
/// A links row is a 12px mark with the URL's 8px font centred beside it.
const LINKS_PITCH: i16 = 12;
const LINKS_H: i16 = LINKS.len() as i16 * LINKS_PITCH + 6;
const LINKS_TEXT_DY: i16 = 2;
/// Rows of the cache page the two card caches render into: the description
/// cache owns 0..92, these sit below it, all inside the u8 V a sprite UV
/// can address.
const CRED_CACHE_V: u16 = 96;
const LINKS_CACHE_V: u16 = 152;

/// Display frames a second, which is what the sound effects are timed in.
const TICKS_HZ: u32 = 60;

/// How loud the two blips sit against the menu music, which plays at full
/// CD volume. The old 1/14 was about seven percent of full scale and was
/// nearly inaudible on a console: it had been set while the blips were
/// still keyed under `Adsr::sample()`, where the SB1 bug held them at full
/// envelope indefinitely, so a tiny gain still carried. `percussive()`
/// self-fades in ~150 ms, and at 1/14 that left almost nothing to hear.
/// Browse fires on every turn of the carousel so it stays the quieter of
/// the two; select is a one-off and can afford to land.
const BROWSE_GAIN: Volume = Volume::linear(1, 4);
const SELECT_GAIN: Volume = Volume::linear(1, 3);
/// The launch swoosh rides under the warp, so it carries the moment and is
/// the loudest of the three.
const LAUNCH_GAIN: Volume = Volume::linear(2, 5);
/// The swoosh is 0.56 s and the launch animation is 40 frames of warp
/// plus 14 of fade -- 0.9 s. Played at its own rate it finished a third
/// of a second before the screen did, which read as too quick. Pitching
/// it to 11/16 of unity stretches it to about 0.82 s, so it carries the
/// whole warp and deepens as it goes, which suits a launch.
const LAUNCH_STRETCH_NUM: u32 = 11;
const LAUNCH_STRETCH_DEN: u32 = 16;

/// A blip when the carousel turns and a heavier one when a program is
/// chosen. Two voices, well clear of the CD input.
const VOICE_BROWSE: Voice = Voice::V0;
const VOICE_SELECT: Voice = Voice::V1;
/// The launch swoosh gets its own voice so the confirm chirp underneath it
/// is not cut off by the round-robin.
const VOICE_LAUNCH: Voice = Voice::V2;
const SFX_BASE: SpuAddr = SpuAddr::new(0x1010);
/// A carousel tick, chosen by measuring every sound in the project.
///
/// ui_beep sat here first and was inaudible: 0.08 s is about thirty cycles of a
/// 400 Hz tone, well under the ~200 ms the ear integrates loudness over, so no
/// gain could rescue it. jump replaced it and was audible but wrong for the
/// job -- 0.25 s of boing, the longest and dullest short sample available, and
/// long enough that fast scrolling smears rather than ticks.
///
/// click is 0.10 s and the brightest short sound in the project at a 3097 Hz
/// centroid, which is what has to carry over CD music on a TV speaker. Its mean
/// amplitude looks low against jump's, but its peak is near full scale: that is
/// a sharp transient with a fast decay, which is exactly what a tick you hear a
/// hundred times a minute should be. At 0.10 s it can also retrigger three
/// times inside the window jump was still sounding in.
static SFX_BROWSE: &[u8] =
    include_bytes!("../../.psoxide/assets/audio/voxide-ui/psau/ui_click.psau");
static SFX_SELECT: &[u8] =
    include_bytes!("../../.psoxide/assets/audio/voxide-ui/psau/ui_confirm.psau");
/// 0.56 s of rushing air, which is just under the 40 frames of warp the
/// starfield accelerates through before the fade takes the screen.
static SFX_LAUNCH: &[u8] = include_bytes!("../../.psoxide/assets/audio/freesfx/psau/swoosh.psau");

/// Frames the launch fade takes. Long enough to read as deliberate, short
/// enough that nobody waits for it.
const FADE_FRAMES: i32 = 14;

/// Frames the launch shutter runs before the hand-off.
const WARP_FRAMES: i32 = 40;

// The reader owns a one-sector bounce buffer; keep it off the 32 KiB stack.
static mut READER: SectorReader = SectorReader::new();
static mut TOC_SECTOR: [u32; SECTOR_WORDS * disc_toc::TOC_SECTORS as usize] =
    [0; SECTOR_WORDS * disc_toc::TOC_SECTORS as usize];

#[no_mangle]
fn main() {
    tty::println("launcher: booted");

    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);
    let font = FontAtlas::upload(&BASIC, FONT_TPAGE, FONT_CLUT);
    let small = FontAtlas::upload(&SPLEEN_5X8, SMALL_TPAGE, SMALL_CLUT);
    let banner = paint::Banner::upload(
        BANNER_TEX,
        BANNER_CLUT_DATA,
        BANNER_W,
        BANNER_H,
        BANNER_TPAGE,
        BANNER_CLUT,
    );
    let icons = paint::Icons::upload(
        ICONS_TEX,
        ICONS_CLUT_DATA,
        ICONS_CELL * ICONS_COUNT,
        ICONS_SIZE,
        BANNER_TPAGE,
        ICONS_V,
        ICONS_CLUT,
        ICONS_CELL,
    );
    let cards = paint::ArcadeCards::upload(
        CARDS_TEX,
        CARDS_CLUT_DATA,
        CARD_ART_W,
        CARD_ART_H,
        CARDS_TPAGE,
        CARDS_CLUTS,
    );

    let mut all_entries = [Entry::new("", 0, 0, 0); MAX_ENTRIES];
    let header = read_toc(&mut all_entries);
    let all_count = header.map_or(0, |h| h.count);
    // What the carousel actually shows. Gated entries stay out of it until
    // the cheat code below rebuilds it with `unlocked` set; the credits ride
    // at the end like everything else, with no program behind them (a zero
    // LBA is what marks an entry as nothing to boot).
    let mut unlocked = false;
    let mut entries = [Entry::new("", 0, 0, 0); MAX_ENTRIES];
    let mut count = visible_entries(&all_entries, all_count, unlocked, &mut entries);
    if count == 0 {
        tty::println("launcher: no table of contents on this disc");
    }

    let mut tick: u32 = 0;
    // The blips.
    //
    // psx-sfx owns what used to sit here: the sequential upload, the key-on
    // that writes a repeat address, and the cutoff that stops a one-shot on a
    // clock because END will not. Its Player also scales the launch swoosh's
    // cutoff by its own pitch, which this had to do by hand.
    let mut bank = Bank::new(SFX_BASE);
    // OneShot defaults to default_tone. percussive self-fades in about 150 ms,
    // which suits the select chirp and would throw away most of a 0.25 s blip
    // or a 0.56 s swoosh.
    let sfx_browse = OneShot::new(bank.upload(SFX_BROWSE), BROWSE_GAIN);
    let sfx_select =
        OneShot::new(bank.upload(SFX_SELECT), SELECT_GAIN).with_adsr(Adsr::percussive());
    // 0x1000 is unity; scaling it down stretches the sample over the warp.
    let sfx_launch = OneShot::new(bank.upload(SFX_LAUNCH), LAUNCH_GAIN).with_pitch(Pitch::raw(
        (0x1000 * LAUNCH_STRETCH_NUM / LAUNCH_STRETCH_DEN) as u16,
    ));
    let mut sfx: Player<3> = Player::new([VOICE_BROWSE, VOICE_SELECT, VOICE_LAUNCH], TICKS_HZ);
    /// Slots into `sfx`, in the order its voices were handed over.
    const SFX_BROWSE_SLOT: usize = 0;
    const SFX_SELECT_SLOT: usize = 1;
    const SFX_LAUNCH_SLOT: usize = 2;

    let mut selected: i32 = 0;
    // How far into KONAMI the pad has come.
    let mut konami: u8 = 0;
    // Frames into the launch warp, or -1 while the menu is just a menu.
    let mut warp: i32 = -1;
    let mut launch_index = 0usize;
    let mut italian = false;
    let mut prev_held = ButtonState::default();
    // The collection needs a DualShock in analog mode. Lock it there now, so
    // the Analog button cannot drop it back to digital mid-menu, and read it
    // through a PadReader so a packet that fails validation never turns into
    // a phantom press. A pad that is not in analog mode (a digital pad, one
    // plugged in later, an empty port) holds the menu on a notice and is
    // asked again every ANALOG_RECHECK_TICKS.
    let mut pad_reader = PadReader::port1();
    let mut analog_ok = require_analog_port1() == AnalogRequirement::Analog;
    let mut next_analog_check: u32 = ANALOG_RECHECK_TICKS;
    // The credits card's two boxes never change, so these render once and
    // are blitted for the rest of the run. Key 0 is as good as any.
    let mut credits_cache = paint::TextCache::at(CRED_CACHE_V, CARD_W - 2, CRED_H - 2);
    let mut links_cache = paint::TextCache::at(LINKS_CACHE_V, CARD_W - 2, LINKS_H - 2);
    if let Some(ref header) = header {
        credits_cache.begin(0);
        render_credits(&small, header);
        credits_cache.end(&fb);
        links_cache.begin(0);
        render_links(&small, &icons);
        links_cache.end(&fb);
        gpu::draw_sync();
    }
    loop {
        tick = tick.wrapping_add(1);
        // Stop any SFX voice that has outlived its own sample. Volume, not
        // key_off: immediate, and owing nothing to envelope behaviour.
        sfx.tick(tick);

        let state = pad_reader.poll();
        if state.mode == PadMode::Analog {
            analog_ok = true;
        } else if warp < 0 && tick.wrapping_sub(next_analog_check) < u32::MAX / 2 {
            analog_ok = require_analog_port1() == AnalogRequirement::Analog;
            next_analog_check = tick.wrapping_add(ANALOG_RECHECK_TICKS);
        }
        let pad = state.buttons;
        // Buttons still track while the notice is up, so one held through
        // it does not fire when the pad is accepted.
        let pressed = |b: u16| analog_ok && pad.is_held(b) && !prev_held.is_held(b);
        // The oldest trick in the book, on press edges: a wrong button starts
        // the sequence over (or counts as its first UP). The final CROSS is
        // swallowed below so completing the code cannot double as "launch
        // whatever the carousel happens to be resting on". The side effects
        // along the way -- four language flips, four browse chirps -- cancel
        // out or read as the console noticing something is up.
        let mut code_cross = false;
        if !unlocked && warp < 0 {
            for &b in &KONAMI_BUTTONS {
                if pressed(b) {
                    konami = if b == KONAMI[konami as usize] {
                        konami + 1
                    } else if b == KONAMI[0] {
                        1
                    } else {
                        0
                    };
                    if konami as usize == KONAMI.len() {
                        unlocked = true;
                        count = visible_entries(&all_entries, all_count, unlocked, &mut entries);
                        selected = 0;
                        code_cross = true;
                        sfx.play_on(SFX_SELECT_SLOT, &sfx_select, tick);
                    }
                }
            }
        }
        // The pad goes quiet once a launch is under way: the warp is short,
        // and half a browse queued behind it would land in the game.
        if warp < 0 {
            if pressed(button::UP) || pressed(button::DOWN) {
                italian = !italian;
            }
            if count > 0 {
                // The game-bank keys are laid out left to right, so the pad
                // moves through them in the same direction.
                let browse = pressed(button::RIGHT) as i32 - pressed(button::LEFT) as i32;
                if browse != 0 {
                    selected += browse;
                    sfx.play_on(SFX_BROWSE_SLOT, &sfx_browse, tick);
                }
                if (pressed(button::CROSS) && !code_cross) || pressed(button::START) {
                    let index = selected.rem_euclid(count as i32) as usize;
                    // Nothing behind the credits entry to chain-load.
                    if entries[index].exe_lba != 0 {
                        // Confirm chirp and launch swoosh together: the chirp
                        // answers the button, the swoosh carries the warp.
                        // The swoosh's cutoff stretches with its pitch on its
                        // own now, which this used to work out by hand.
                        sfx.play_on(SFX_SELECT_SLOT, &sfx_select, tick);
                        sfx.play_on(SFX_LAUNCH_SLOT, &sfx_launch, tick);
                        launch_index = index;
                        warp = 0;
                    }
                }
            }
        }
        prev_held = pad;

        // The warp itself: count it forward, and hand over to the fade and
        // the chain-load once it has run its course.
        if warp >= 0 {
            warp += 1;
            if warp >= WARP_FRAMES {
                // Never returns when the disc is readable.
                boot(&entries[launch_index], &mut fb);
            }
        }
        // No menu track on this disc, so there is no beat to answer.
        let beat = carousel::Beat::default();
        fb.clear(0, 0, 6);
        paint::selection_cabinet(beat.pulse);
        banner.draw(160 - BANNER_W / 2, BANNER_Y);
        small.draw_text(DISC_VERSION_X, DISC_VERSION_Y, DISC_VERSION, NOW_PLAYING);
        small.draw_text(
            LANGUAGE_X,
            LANGUAGE_Y,
            if italian { "IT" } else { "EN" },
            NOW_PLAYING,
        );

        if count == 0 {
            centred(&font, 106, "DISC TABLE OF CONTENTS UNREADABLE", ERROR);
        } else {
            let index = selected.rem_euclid(count as i32) as usize;
            let hide_text = warp >= 0 || !analog_ok;
            if !hide_text {
                if entries[index].exe_lba == 0 {
                    paint::text_panel(CARD_X, CRED_Y, CARD_W, CRED_H);
                    credits_cache.draw(CARD_X + 1, CRED_Y + 1);
                    paint::text_panel(CARD_X, LINKS_Y, CARD_W, LINKS_H);
                    links_cache.draw(CARD_X + 1, LINKS_Y + 1);
                } else {
                    centred(&small, GAME_SELECT_Y, "GAME SELECT", TITLE);
                    draw_game_cards(&font, &small, &cards, &entries[..count], index, beat.pulse);
                }
                paint::arcade_jukebox(JUKE_X, JUKE_Y, JUKE_W, JUKE_H, beat.pulse);
                draw_music_panel(&small);
                draw_credits_key(&small, entries[index].exe_lba == 0, beat.pulse);
            }
            if warp >= 0 {
                paint::arcade_launch_shutter(warp, WARP_FRAMES);
            }
            if !analog_ok && warp < 0 {
                draw_controller_notice(&small);
            }
        }

        gpu::draw_sync();
        psx_rt::interrupts::wait_vblank();
        fb.swap();
    }
}

/// What the menu shows while port 1 holds no DualShock in analog mode.
fn draw_controller_notice(small: &FontAtlas) {
    const LINES: [&str; 5] = [
        "This disc needs an analog controller",
        "(DualShock) in port 1.",
        "",
        "Questo disco richiede un controller",
        "analogico (DualShock) nella porta 1.",
    ];
    let h = LINES.len() as i16 * 12 + 14;
    let y = GAME_CARD_Y;
    paint::text_panel(CARD_X, y, CARD_W, h);
    for (row, line) in LINES.iter().enumerate() {
        let x = CARD_X + (CARD_W - line.len() as i16 * 5) / 2;
        small.draw_text(x, y + 8 + row as i16 * 12, line, BLURB);
    }
}

fn centred(font: &FontAtlas, y: i16, text: &str, tint: (u8, u8, u8)) {
    if text.is_empty() {
        return;
    }
    font.draw_text(160 - (font.text_width(text) as i16) / 2, y, text, tint);
}

/// Break `text` at the last space that fits, so a long blurb reads as two
/// tidy lines rather than one cut mid-word.
fn wrap(text: &str, max: usize) -> (&str, &str) {
    if text.len() <= max {
        return (text, "");
    }
    match text[..max].rfind(' ') {
        Some(at) => (&text[..at], text[at + 1..].trim_start()),
        None => (&text[..max], text[max..].trim_start()),
    }
}

fn draw_game_cards(
    font: &FontAtlas,
    small: &FontAtlas,
    cards: &paint::ArcadeCards,
    entries: &[Entry],
    selected: usize,
    pulse: u8,
) {
    for (slot, entry) in entries.iter().enumerate().take(3) {
        let x = GAME_CARD_X[slot];
        let active = slot == selected;
        paint::selection_card(x, GAME_CARD_Y, GAME_CARD_W, GAME_CARD_H, active, pulse);
        let label = match entry.name_str() {
            "SPACE INVADERS" => "INVADERS",
            "MAGIKAAAAARP PONG" => "MAGIKARP",
            other => other,
        };
        let tint = if active { LABEL } else { FAR_LABEL };
        let tx = x + GAME_CARD_W / 2 - small.text_width(label) as i16 / 2;
        small.draw_text(tx, GAME_CARD_Y + 5, label, tint);
        cards.draw(slot, x + 4, GAME_CARD_Y + 16, if active { 128 } else { 92 });
    }

    let entry = &entries[selected.min(2)];
    centred(font, SELECTED_TITLE_Y, entry.name_str(), LABEL);
    let version = entry.version_str();
    if !version.is_empty() {
        let width = small.text_width(version) as i16 + 5;
        let x = 160 - width / 2;
        small.draw_text(x, SELECTED_VERSION_Y, "v", NOW_PLAYING);
        small.draw_text(x + 5, SELECTED_VERSION_Y, version, TRACK_NAME);
    }
}

fn draw_credits_key(font: &FontAtlas, active: bool, pulse: u8) {
    paint::credits_key(
        CREDITS_KEY_X,
        CREDITS_KEY_Y,
        CREDITS_KEY_W,
        CREDITS_KEY_H,
        active,
        pulse,
    );
    let label = "CREDITS";
    let x = CREDITS_KEY_X + CREDITS_KEY_W / 2 - font.text_width(label) as i16 / 2;
    font.draw_text(
        x,
        CREDITS_KEY_Y + 11,
        label,
        if active { LABEL } else { FAR_LABEL },
    );
}

/// One centred line of the credits card's wide box. Coordinates are local
/// to its cache.
fn card_line(font: &FontAtlas, line: i16, text: &str, tint: (u8, u8, u8)) {
    if text.is_empty() {
        return;
    }
    let x = (CARD_W - 2) / 2 - (font.text_width(text) as i16) / 2;
    font.draw_text(x, line * DESC_LEADING + 2, text, tint);
}

/// Draw the credits into their cache. The card is wide enough that the
/// artist credit sits on one line, with the four track titles under it.
fn render_credits(font: &FontAtlas, header: &Header) {
    let mut lines = 0i16;
    let mut rest = header.credit_str();
    while !rest.is_empty() && lines < 2 {
        let (_, tail) = wrap(rest, CARD_COLS);
        lines += 1;
        rest = tail;
    }
    let titled = (0..header.menu_track_count as usize)
        .filter(|&t| !header.title(t).is_empty())
        .count() as i16;
    if lines == 0 && titled == 0 {
        card_line(font, 0, "MUSIC", NOW_PLAYING);
        card_line(font, 1, "GONCHAROV", TRACK_NAME);
        card_line(font, 2, "MAGIKAAAAARP", BLURB);
        card_line(font, 3, "USED WITH PERMISSION", BLURB);
        return;
    }
    let total = (lines + titled).min(CRED_ROWS);
    let mut line = (CRED_ROWS - total) / 2;
    let mut rest = header.credit_str();
    let credit_end = line + lines;
    while !rest.is_empty() && line < credit_end {
        let (head, tail) = wrap(rest, CARD_COLS);
        card_line(font, line, head, BLURB);
        line += 1;
        rest = tail;
    }
    for track in 0..header.menu_track_count as usize {
        if line >= CRED_ROWS {
            break;
        }
        let title = header.title(track);
        if !title.is_empty() {
            card_line(font, line, title, TRACK_NAME);
            line += 1;
        }
    }
}

/// Draw the links into their cache. Coordinates are local to it.
///
/// One mark and one full URL each. The rows left-align as a block, and the
/// block centres on the widest URL, so the domains line up under each other
/// the way a list should. Nothing here depends on the disc, so the cache
/// never needs rebuilding after the first time.
fn render_links(font: &FontAtlas, icons: &paint::Icons) {
    let widest = LINKS
        .iter()
        .map(|(_, url)| font.text_width(url) as i16)
        .max()
        .unwrap_or(0);
    let x0 = ((CARD_W - 2) - (ICONS_SIZE + 2 + widest)) / 2;
    for (row, (slot, url)) in LINKS.iter().enumerate() {
        let y = 2 + row as i16 * LINKS_PITCH;
        icons.draw(*slot, x0, y, BLURB);
        font.draw_text(x0 + ICONS_SIZE + 2, y + LINKS_TEXT_DY, url, BLURB);
    }
}

/// Jukebox readout. The standalone collection owns no menu track: Goncharov
/// belongs to Magikarp Pong and starts only after that game launches, so the
/// strip says where the music is and keeps the meter moving on its own.
fn draw_music_panel(font: &FontAtlas) {
    font.draw_text(MUSIC_X, MUSIC_TOP, "MUSIC IN MAGIKARP", NOW_PLAYING);
    font.draw_text(MUSIC_X, TRACK_TOP, "GONCHAROV", TRACK_NAME);
    paint::level_meter_beat(METER_X, METER_BASE, 72);
}

/// Read the table of contents. `None` if the disc has none.
/// Copy the entries the carousel should show into `out` and append the
/// credits card: everything when `unlocked`, everything but the gated ones
/// before then. Returns how many cards that is.
fn visible_entries(
    all: &[Entry; MAX_ENTRIES],
    all_count: usize,
    unlocked: bool,
    out: &mut [Entry; MAX_ENTRIES],
) -> usize {
    let mut n = 0;
    for entry in all.iter().take(all_count) {
        if (unlocked || !entry.is_hidden()) && n < MAX_ENTRIES {
            out[n] = *entry;
            n += 1;
        }
    }
    if n > 0 && n < MAX_ENTRIES {
        out[n] = Entry::new(CREDITS_NAME, 0, 0, 0);
        n += 1;
    }
    n
}

fn read_toc(entries: &mut [Entry; MAX_ENTRIES]) -> Option<Header> {
    // SAFETY: single-threaded, polled; `main` runs once and nothing else
    // touches these statics.
    let reader = unsafe { &mut *core::ptr::addr_of_mut!(READER) };
    let sector = unsafe { &mut *core::ptr::addr_of_mut!(TOC_SECTOR) };

    // The table spans more than one sector now; `read_sector` walks the same
    // ReadN stream, so they arrive back to back.
    let mut ok = unsafe { reader.prepare() && reader.start_read(TOC_LBA) };
    for chunk in sector.chunks_exact_mut(SECTOR_WORDS) {
        let slot: &mut [u32; SECTOR_WORDS] = chunk.try_into().expect("exact chunks");
        ok = ok && unsafe { reader.read_sector(slot) };
    }
    unsafe { reader.stop() };
    if !ok {
        return None;
    }
    // SAFETY: a [u32; 512] is 2048 bytes; the target is little-endian, so the
    // word buffer and the on-disc byte order agree.
    let bytes: &[u8; TOC_BYTES] =
        unsafe { &*(sector.as_ptr() as *const u8 as *const [u8; TOC_BYTES]) };
    disc_toc::decode(bytes, entries)
}

/// Copy the chain-load blob high and jump to it. Never returns while the disc
/// is readable; the blob paints the screen red and stops if it is not.
fn boot(entry: &Entry, fb: &mut FrameBuffer) -> ! {
    // Fade rather than cut. Averaging black over a buffer halves it, and each
    // buffer comes round every other frame, so seven passes each takes the
    // picture to a hundred and twenty-eighth before the loader takes over.
    for _ in 0..FADE_FRAMES {
        paint::fade_step();
        gpu::draw_sync();
        psx_rt::interrupts::wait_vblank();
        fb.swap();
    }

    assert!(LOADER_BLOB.len() <= LOADER_LIMIT, "loader blob too large");
    tty::println("launcher: chain-loading");

    // Let the GPU finish before the blob resets it out from under whatever is
    // still on screen.
    gpu::draw_sync();

    // SAFETY: `LOADER_BASE` is above every game's payload (mkdisc enforces
    // that) and below the stack, so nothing live is being overwritten. The
    // blob is position-dependent and linked for exactly this address.
    unsafe {
        core::ptr::copy_nonoverlapping(
            LOADER_BLOB.as_ptr(),
            LOADER_BASE as *mut u8,
            LOADER_BLOB.len(),
        );
        psx_rt::cache::flush_instruction_cache();
        let blob: unsafe extern "C" fn(u32, u32, u32, u32) -> ! =
            core::mem::transmute(LOADER_BASE as usize);
        // The copied loader is its own linked image, so its SectorReader has
        // no access to this launcher's installed disc base. Resolve the EXE
        // LBA here; pass the combined data/audio bases separately for the
        // child runtime to install at _start.
        blob(
            disc_base::shift_lba(entry.exe_lba),
            disc_base::shift_lba(entry.lba_offset),
            entry
                .cdda_track_base
                .wrapping_add(disc_base::cdda_track_base() as u32),
            entry.payload_fnv,
        )
    }
}
