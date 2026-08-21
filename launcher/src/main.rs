//! Demo disc boot menu.
//!
//! Reads the disc's table of contents ([`disc_toc`]), spins its contents round
//! a carousel, and chain-loads the selection. The look is a homage to the
//! PlayStation demo discs: glossy blue pills on a tilted ring, a ball of balls
//! turning above them, a starfield behind.
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

use carousel::{Bead, Placed, SPHERE_POINTS, TURN};
use disc_toc::{Entry, Header, MAX_ENTRIES, TOC_BYTES, TOC_LBA};
use psx_font::{
    fonts::{BASIC, SPLEEN_5X8},
    FontAtlas,
};
use psx_gpu::{self as gpu, framebuf::FrameBuffer, Resolution, VideoMode};
use psx_io::cdda::{CddaClock, CddaEndDetector, CddaStarter};
use psx_io::cdrom::{self, PlayPosition};
use psx_io::disc_base;
use psx_sfx::{Bank, OneShot, Player};
use psx_spu::{self as spu, Adsr, CdVolume, Pitch, SpuAddr, Voice, Volume};
use psx_pack::cd::{SectorReader, SECTOR_WORDS};
use psx_pad::{button, poll_port1, ButtonState};
use psx_rt::tty;
use psx_vram::{Clut, TexDepth, Tpage};

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

const FONT_TPAGE: Tpage = Tpage::new(320, 0, TexDepth::Bit4);
const FONT_CLUT: Clut = Clut::new(320, 256);

/// The header's own font: same eight-pixel height as BASIC but five wide, so
/// the music column fits beside a centred mark instead of running under it.
const SMALL_TPAGE: Tpage = Tpage::new(448, 0, TexDepth::Bit4);
const SMALL_CLUT: Clut = Clut::new(416, 256);

/// The banner sits in its own 4bpp page, clear of both framebuffers and of
/// the font. Tpage X must be a multiple of 64.
const BANNER_TPAGE: Tpage = Tpage::new(384, 0, TexDepth::Bit4);
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

/// Which mark leads the row, and the URL written out in full. Full because
/// the whole point of the card is that someone photographs or retypes these;
/// the first cut showed only the part after the slash and read as "which
/// site is this?".
const LINKS: [(i16, &str); 5] = [
    (0, "github.com/EBonura/PSoXide"),
    (1, "bonnie-studios.itch.io"),
    (2, "x.com/_bonniestudios"),
    (3, "instagram.com/izzy88izzy"),
    (4, "buymeacoffee.com/bonniestudios"),
];

const TITLE: (u8, u8, u8) = (255, 84, 62);
const HINT: (u8, u8, u8) = (168, 44, 40);
const NOW_PLAYING: (u8, u8, u8) = (172, 40, 34);
const TRACK_NAME: (u8, u8, u8) = (255, 88, 64);
const BLURB: (u8, u8, u8) = (255, 206, 196);
const LABEL: (u8, u8, u8) = (255, 255, 255);
const FAR_LABEL: (u8, u8, u8) = (215, 78, 62);
const ERROR: (u8, u8, u8) = (255, 214, 90);

/// The carousel entry that shows the credits instead of running something.
/// `split_title` breaks it at the space nearest the middle, so the pill
/// reads CREDITS over AND LINKS.
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

const STARS: u32 = 120;

/// Turns per frame the ring eases toward its target, as a fraction: the gap
/// closes by an eighth each frame, which settles in about half a second.
const EASE_SHIFT: i32 = 3;
/// What the ball of balls drifts at when nobody is touching the pad, before
/// the bar swings it.
const SPHERE_IDLE_SPIN: i32 = 5;
/// How much faster it turns at the top of a bar than at the end of one.
const SPHERE_BAR_SWING: i32 = 6;
/// How far the ball blows outward on a beat, in 256ths of its radius, on the
/// downbeat and on the other three.
const SWELL_DOWNBEAT: i32 = 44;
const SWELL_BEAT: i32 = 22;
/// How far a hard browse throws the beads apart, and the ceiling on it.
const SCATTER_PER_KICK: i32 = 4;
const SCATTER_MAX: i32 = 130;
/// The shove browsing gives it. It spins up with the carousel and coasts back
/// down to the idle drift, so the whole screen reacts rather than just the ring.
const SPHERE_KICK: i32 = 110;
/// How fast that shove bleeds off: a sixteenth of the excess per frame.
const SPHERE_DECAY_SHIFT: i32 = 4;

/// Widest line the description column fits at the 8-pixel font, with the
/// screenshot taking the rest of the panel's width.
const WRAP_CHARS: usize = disc_toc::DESC_COLUMNS;
/// Top of the description block, and the gap between its lines.
/// A black header strip across the whole screen, holding the mark and the
/// words under it. Full width rather than a box round the logo: centred is
/// where the mark belongs, and anything narrower collides with the music
/// panel, which is what pushed it off-centre before.
const HEADER_H: i16 = 44;
/// The music panel stacks down the header's left edge: label, title, meter,
/// hint. The mark sits below the title row rather than beside it, because a
/// title runs to 112 pixels and a centred mark starts at 90.
const MUSIC_TOP: i16 = 4;
const TRACK_TOP: i16 = 13;
const METER_BASE: i16 = 38;
/// The mark sits beside the column now rather than under it: at five pixels a
/// character the widest track title stops well short of a centred mark.
const BANNER_Y: i16 = 7;
/// Two boxes of the same size sit centred in the band between the header
/// and the carousel: the description on the left, the screenshot on the
/// right. The box is the screenshot plus its one-pixel border and nothing
/// else -- the image runs edge to edge.
const PANEL_W: i16 = disc_toc::SHOT_W as i16 + 2;
const PANEL_H: i16 = disc_toc::SHOT_H as i16 + 2;
/// Horizontally the pair splits its spare width into three equal gaps:
/// edge, middle, edge. Vertically it centres between the header's bottom
/// and the far pills' tops.
const PANEL_GAP: i16 = (320 - 2 * PANEL_W) / 3;
const PANEL_X: i16 = PANEL_GAP;
const PANEL_Y: i16 = HEADER_H + (168 - HEADER_H - PANEL_H) / 2;
/// The right box mirrors the left one across the screen's centre line.
const SHOT_BOX_X: i16 = 320 - PANEL_GAP - PANEL_W;
/// Where the text column blits inside its box, and the line pitch.
const TEXT_X: i16 = PANEL_X + (PANEL_W - paint::CACHE_W) / 2;
const TEXT_Y: i16 = PANEL_Y + (PANEL_H - paint::CACHE_H) / 2;
const DESC_LEADING: i16 = 9;
/// The screenshot, flush inside its border.
const SHOT_X: i16 = SHOT_BOX_X + 1;
const SHOT_Y: i16 = PANEL_Y + 1;

/// The credits card stacks two centred boxes in the band instead of the twin
/// description/screenshot pair: CREDITS on top, LINKS below. Wide enough that
/// the Just Music credit and every full URL fit on one line.
const CARD_W: i16 = 212;
const CARD_X: i16 = (320 - CARD_W) / 2;
/// Characters a card line holds at the 5-pixel font.
const CARD_COLS: usize = ((CARD_W - 8) / 5) as usize;
/// Rows the credits box holds at the description leading.
const CRED_ROWS: i16 = 5;
const CRED_Y: i16 = HEADER_H + 2;
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

/// Ticks between drive-status polls while the menu track plays. Often enough
/// to restart the loop without a gap anyone notices, rare enough that the
/// polling does not fight the audio.
const CDDA_POLL_TICKS: u32 = 30;
/// Consecutive idle polls that mean the track really has ended. At 30 ticks
/// a poll this is four seconds of silence, and it has to be that long: the
/// menu tracks sit at the far end of the disc, and around the cross-disc
/// seek after Play a real drive spends one to two seconds reporting neither
/// playing nor seeking. At 3 polls (1.5 s) that window read as track-over
/// and the menu skipped through all four tracks before playback stuck --
/// the 2026-08-01 19:10 console recording, music arriving in blips. A
/// genuine end of track now takes four quiet seconds to advance, which the
/// inter-track pregap absorbs.
const CDDA_IDLE_POLLS_TO_ADVANCE: u8 = 8;
/// Spin budget per CD command. Silicon wants more than an emulator does.
const CDDA_SPINS: u32 = 0x10_0000;
/// Ticks after an accepted Play by which the drive must have been seen
/// actually playing, or the start is retried. Covers a spin-up plus a
/// worst-case seek with room to spare.
const PLAY_CONFIRM_TICKS: u32 = 480;
/// How close to a track's known length "quiet" still counts as the song
/// finishing rather than the drive losing its place mid-track.
const END_MARGIN_MS: u32 = 4000;
/// Display frames a second, which is what the CD clock counts in.
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
static SFX_LAUNCH: &[u8] =
    include_bytes!("../../.psoxide/assets/audio/freesfx/psau/swoosh.psau");

/// Idle frames before the menu clears itself down to the ball turning over
/// the carousel. A demo disc spends most of its life unattended.
const ATTRACT_AFTER: u32 = 60 * 20;
/// Frames the launch fade takes. Long enough to read as deliberate, short
/// enough that nobody waits for it.
const FADE_FRAMES: i32 = 14;

/// Frames the launch warp runs before that fade: the starfield accelerates
/// into streaks and the ball blows apart, so leaving the menu is an event
/// rather than a cut.
const WARP_FRAMES: i32 = 40;
/// Warp travel per frame is the frame count squared over this, which reads as
/// acceleration rather than a jump to speed.
const WARP_RAMP: i32 = 3;
/// Cap on that. The field is 1360 deep; at 280 a star crosses it in five
/// frames, which is as fast as streaks read as motion rather than noise.
const WARP_TOP_SPEED: i32 = 280;
/// How much a warp frame adds to the ball's swell, in 256ths of its radius.
/// By the fade the beads are three screens wide and gone past the camera.
const WARP_SWELL: i32 = 18;

// The reader owns a one-sector bounce buffer; keep it off the 32 KiB stack.
static mut READER: SectorReader = SectorReader::new();
static mut TOC_SECTOR: [u32; SECTOR_WORDS * disc_toc::TOC_SECTORS as usize] =
    [0; SECTOR_WORDS * disc_toc::TOC_SECTORS as usize];

/// Room for every menu track's level-meter data. Read once at boot rather
/// than per track, so skipping stays as quick as the drive allows.
const SPECTRUM_MAX_SECTORS: usize = 192;
static mut SPECTRUM: [u32; SECTOR_WORDS * SPECTRUM_MAX_SECTORS] =
    [0; SECTOR_WORDS * SPECTRUM_MAX_SECTORS];

/// Every screenshot on the disc, cached whole at boot for the same reason
/// the spectrum is: once the menu music has the drive, a disc read means a
/// seek away from the audio and back, audible every time. Swapping a shot
/// from RAM is a moment of DMA instead. ~200 KiB of .bss.
const SHOT_SLOT_WORDS: usize = disc_toc::SHOT_SECTORS as usize * SECTOR_WORDS;
static mut SHOTS: [u32; SHOT_SLOT_WORDS * disc_toc::MAX_SHOTS] =
    [0; SHOT_SLOT_WORDS * disc_toc::MAX_SHOTS];

/// Full brightness for the shot, in GPU tint units.
const SHOT_FULL: i32 = 128;
/// Tint steps per frame: out faster than in, so a browse feels like the menu
/// answering rather than the old image lingering under the new pill. The
/// single VRAM slot only swaps at black, which is what makes the upload
/// invisible.
const SHOT_FADE_IN: i32 = 10;
const SHOT_FADE_OUT: i32 = 16;
/// Frames a shot rests before a multi-shot entry moves to its next one.
/// Seven seconds read as a stall on the console and three still lingered:
/// the fade carries the transition, so the hold only has to be long enough
/// to take the image in.
const SHOT_SLIDE_FRAMES: u32 = 90;

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

    let mut all_entries = [Entry::new("", 0, 0, 0); MAX_ENTRIES];
    // Read the table before a note of music plays: a data read while the
    // drive is playing CD-DA is the one thing this hardware is worst at.
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

    // Before the music starts, for the same reason the table is: reading the
    // disc while it plays CD-DA is what this hardware is worst at.
    let spectrum_frames = read_spectrum(header.as_ref());
    // The whole region, gated entries included: unlocking must not need the
    // drive back.
    let shot_total = read_shots(header.as_ref(), &all_entries, all_count);

    let menu_track = header.map_or(0, |h| h.menu_track) as u8;
    let menu_track_count = header.map_or(0, |h| h.menu_track_count).max(1) as u8;
    // Which of the run is playing. Cycling beats looping one track when the
    // disc might sit on the menu for a whole presentation.
    let mut menu_track_index: u8 = 0;
    let mut music = CddaStarter::new().with_spins(CDDA_SPINS);
    let mut clock = CddaClock::new(TICKS_HZ);
    let mut tick: u32 = 0;
    let mut next_music_poll = CDDA_POLL_TICKS;
    // End-of-track detection lives in the SDK now: the detector only arms
    // once the drive has been SEEN playing the current track, which is what
    // stops the 0x00 stop/spin-up window a real drive reports from reading
    // as track-over (the first debug burn's phantom skip).
    let mut track_end = CddaEndDetector::new(CDDA_IDLE_POLLS_TO_ADVANCE);
    // --- CD debug overlay state (SELECT toggles the display; capture is
    // always on so the counters are honest from boot). Built after the first
    // silicon burn: tracks double-advance and chain-loads fail red, and the
    // emulator reproduces neither, so the console screen is the debugger.
    let mut debug = false;
    let mut last_stat: u8 = 0xEE; // 0xEE = no reading yet, 0xDD = timeout
    let mut stat_hist = [0xEEu8; 10]; // newest first
    let mut stat_timeouts: u16 = 0;
    let mut adv_idle: u16 = 0;
    let mut adv_btn: u16 = 0;
    // Same-track restarts by the playback watchdog: a start that was never
    // seen playing, or quiet in the middle of a track.
    let mut stalls: u16 = 0;
    let mut confirm_by: u32 = 0;
    let mut stop_timeouts: u16 = 0;
    // Where the drive says its head is, polled only while the overlay is
    // up. The v0.4 footage showed WHAT the drive was doing (seeking,
    // forever) but not WHERE; this is the missing column.
    let mut last_pos: Option<PlayPosition> = None;
    let mut next_pos_poll: u32 = CDDA_POLL_TICKS + 15;
    let mut hsk_begin: u32 = 0;
    let mut hsk_ticks: u32 = 0; // last completed Play handshake, in ticks
    let mut events = [0u8; 10]; // bit7 = idle advance, bit6 = button; low bits = track index
    if menu_track != 0 {
        // The CD controller playing is only half of it: the SPU's CD input
        // comes up silent, so without this the drive spins a track nobody
        // hears.
        spu::init();
        spu::set_main_volume(Volume::MAX, Volume::MAX);
        spu::set_cd_volume(CdVolume::MAX, CdVolume::MAX);
        spu::enable_cd_audio(true);
        music.begin(tick);
    }
    // Independent of the music: the blips play whether or not the disc
    // carries a menu track.
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
    let mut ring = 0i32;
    // Two axes. A browse shoves it in some direction, and whatever tumble
    // that leaves is what it keeps until the damping bleeds it off.
    let mut yaw = 0i32;
    let mut pitch = 0i32;
    let mut yaw_rate = SPHERE_IDLE_SPIN;
    let mut pitch_rate = 0i32;
    let mut shoves: u32 = 0;
    // How far the camera has flown into the starfield.
    let mut travel: i32 = 0;
    let mut italian = false;
    let mut prev_held = ButtonState::default();
    /// Frames since the pad last did anything.
    let mut idle: u32 = 0;
    let mut order = [0usize; MAX_ENTRIES];
    let mut beads = [Bead::default(); SPHERE_POINTS];
    let mut text_cache = paint::TextCache::new();
    // The credits card's two boxes never change, so these render once and
    // are blitted for the rest of the run. Key 0 is as good as any.
    let mut credits_cache = paint::TextCache::at(CRED_CACHE_V, CARD_W - 2, CRED_H - 2);
    let mut links_cache = paint::TextCache::at(LINKS_CACHE_V, CARD_W - 2, LINKS_H - 2);
    // The backdrop: which cached shot is in VRAM, how bright it is drawn,
    // and the slideshow clock. -1 means the single VRAM slot holds nothing
    // worth showing.
    let mut shot_shown: i32 = -1;
    let mut shot_level: i32 = 0;
    let mut shot_cycle: u8 = 0;
    let mut shot_dwell: u32 = 0;
    let mut shot_entry: usize = usize::MAX;

    loop {
        tick = tick.wrapping_add(1);
        // One slot past the last track is MUSIC OFF: the L1/R1 cycle
        // walks through every song and then silence, so muting needs no
        // button of its own and the panel can say which state it is in.
        let muted = menu_track != 0 && menu_track_index >= menu_track_count;
        // One clock read a frame, up here so the music watchdog below and
        // the beat code further down agree on where the song is.
        let song_ms = if !muted && clock.playing() {
            clock.tick(tick)
        } else {
            0
        };
        if menu_track != 0 && !muted {
            // Debug-only, and issued BEFORE this frame's GetStat: command
            // then status-drain is the order the controller tolerates (see
            // CddaStarter's note; the reverse has wedged it on silicon).
            if debug
                && music.started()
                && tick.wrapping_sub(next_pos_poll) < u32::MAX / 2
            {
                next_pos_poll = tick.wrapping_add(CDDA_POLL_TICKS);
                if let Some(r) = cdrom::try_get_loc_p(CDDA_SPINS) {
                    if let Some(p) = PlayPosition::parse(&r) {
                        last_pos = Some(p);
                    }
                }
            }
            if music.tick(tick, menu_track + menu_track_index) {
                clock.start(tick);
                hsk_ticks = tick.wrapping_sub(hsk_begin);
                // The drive accepted Play; now it must be SEEN playing
                // before this deadline, or the start gets retried. The
                // v0.2 console run proved a Play can be accepted and then
                // sink without trace: the end detector never arms on a
                // track that never starts, which left the menu silent
                // forever ("other songs don't work at all").
                confirm_by = tick.wrapping_add(PLAY_CONFIRM_TICKS);
            }
            if music.started() && tick.wrapping_sub(next_music_poll) < u32::MAX / 2 {
                next_music_poll = tick.wrapping_add(CDDA_POLL_TICKS);
                let status = cdrom::try_get_stat(CDDA_SPINS);
                last_stat = match &status {
                    Some(r) => r.bytes().first().copied().unwrap_or(0xEF),
                    None => {
                        stat_timeouts = stat_timeouts.saturating_add(1);
                        0xDD
                    }
                };
                stat_hist.copy_within(0..9, 1);
                stat_hist[0] = last_stat;
                let status_byte = status.and_then(|r| r.bytes().first().copied());
                // Quiet long enough to mean the audio is over -- but over
                // can mean two things, and the spectrum data already on the
                // disc tells them apart: quiet NEAR the track's known end
                // is the song finishing (advance), quiet in the middle is
                // the drive losing the track (retry the same one). Without
                // an analysis for the track, quiet advances as before.
                if track_end.poll(status_byte) {
                    let near_end = header
                        .and_then(|h| h.spectrum_span(menu_track_index as usize))
                        .map_or(true, |(_, frames)| {
                            let duration_ms =
                                frames * 1000 / disc_toc::SPECTRUM_FRAME_RATE;
                            song_ms + END_MARGIN_MS >= duration_ms
                        });
                    if near_end {
                        menu_track_index = (menu_track_index + 1) % menu_track_count;
                        adv_idle = adv_idle.saturating_add(1);
                        push_event(&mut events, 0x80 | menu_track_index);
                    } else {
                        stalls = stalls.saturating_add(1);
                        push_event(&mut events, 0x20 | menu_track_index);
                    }
                    hsk_begin = tick;
                    music.begin(tick);
                } else if !track_end.armed()
                    && tick.wrapping_sub(confirm_by) < u32::MAX / 2
                {
                    // Accepted but never seen playing: re-run the start
                    // from Stop, same track, however long it takes. Silence
                    // first, as the manual skip does -- re-Playing over a
                    // wedged drive is how it stayed wedged.
                    if cdrom::try_stop(CDDA_SPINS).is_none() {
                        stop_timeouts = stop_timeouts.saturating_add(1);
                    }
                    stalls = stalls.saturating_add(1);
                    push_event(&mut events, 0x20 | menu_track_index);
                    track_end.rearm();
                    hsk_begin = tick;
                    music.begin(tick);
                }
            }
        }

        // Stop any SFX voice that has outlived its own sample. Volume, not
        // key_off: immediate, and owing nothing to envelope behaviour.
        sfx.tick(tick);

        let pad = poll_port1().buttons;
        let pressed = |b: u16| pad.is_held(b) && !prev_held.is_held(b);
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
        let touched = [
            button::LEFT,
            button::RIGHT,
            button::UP,
            button::DOWN,
            button::L1,
            button::R1,
            button::CROSS,
            button::START,
        ]
        .iter()
        .any(|b| pressed(*b));
        idle = if touched { 0 } else { idle.saturating_add(1) };
        let attract = idle > ATTRACT_AFTER;

        let loading = menu_track != 0 && !music.started();
        // The pad goes quiet once a launch is under way: the warp is short,
        // and half a browse queued behind it would land in the game.
        if warp < 0 {
            if pressed(button::UP) || pressed(button::DOWN) {
                italian = !italian;
            }
            if pressed(button::SELECT) {
                debug = !debug;
            }
            // Skipping tracks by hand, through every song and then the
            // MUSIC OFF slot. The drive takes the better part of a second
            // to pick up a new track; ignore further presses until it has,
            // or the handshake gets re-armed from the start each time and
            // never finishes.
            if menu_track != 0 && !loading {
                let slots = menu_track_count + 1; // the last one is silence
                let skip = if pressed(button::R1) {
                    1
                } else if pressed(button::L1) {
                    slots - 1 // one back, without going negative
                } else {
                    0
                };
                if skip != 0 {
                    menu_track_index = (menu_track_index + skip) % slots;
                    // Silence first: the handshake re-issues Play, and leaving the
                    // old track running under it is how the drive got wedged.
                    // Entering the off slot, this stop IS the feature.
                    if cdrom::try_stop(CDDA_SPINS).is_none() {
                        stop_timeouts = stop_timeouts.saturating_add(1);
                    }
                    track_end.rearm();
                    adv_btn = adv_btn.saturating_add(1);
                    push_event(&mut events, 0x40 | menu_track_index);
                    if menu_track_index < menu_track_count {
                        hsk_begin = tick;
                        music.begin(tick);
                    }
                }
            }
            if count > 0 {
                // RIGHT advances the carousel, LEFT goes back. The ring
                // turns the other way underneath, so pressing right moves the
                // selection toward you rather than sending the ring that way,
                // which is the reading people expect from a list.
                let browse = pressed(button::RIGHT) as i32 - pressed(button::LEFT) as i32;
                if browse != 0 {
                    selected -= browse;
                    // The shove points somewhere unpredictable rather than along
                    // one axis, so the ball tumbles instead of spinning on the
                    // spot. Which way the carousel went only sets the sign.
                    shoves = shoves.wrapping_add(1);
                    let (dx, dy) = carousel::impulse(shoves);
                    yaw_rate -= browse * ((SPHERE_KICK * dx) >> 12);
                    pitch_rate -= browse * ((SPHERE_KICK * dy) >> 12);
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
        let warp_speed = if warp >= 0 {
            (warp * warp / WARP_RAMP).min(WARP_TOP_SPEED)
        } else {
            0
        };

        // Ease toward the selection instead of snapping. `target` is allowed
        // to run past a full turn so the ring keeps spinning the short way
        // rather than unwinding.
        let step = TURN / count.max(1) as i32;
        let target = -selected * step;
        ring += (target - ring) >> EASE_SHIFT;

        // Everything visual answers the beat. The grid was measured off the
        // audio and shipped in the table, so this stays in step for the whole
        // length of a track rather than drifting out of it. `song_ms` was
        // read once at the top of the loop, shared with the watchdog.
        let beat = match header.and_then(|h| h.beat(menu_track_index as usize)) {
            Some((beat_ms, phase_ms)) if clock.playing() => {
                carousel::beat_at(song_ms, beat_ms, phase_ms)
            }
            _ => carousel::Beat::default(),
        };
        // The downbeat gets the bigger shove. A hard browse adds to the same
        // term, so the ball blows apart and re-forms as the kick decays.
        // The low bands drive the swell when the disc carries an analysis:
        // the ball then answers what the track is actually doing rather than
        // a grid laid over it. The beat envelope is the fallback.
        let levels = header
            .as_ref()
            .and_then(|h| spectrum_frame(h, menu_track_index, song_ms, spectrum_frames));
        let bass = levels.map(|l| (l[0] as i32 + l[1] as i32 + l[2] as i32) / 3);
        let swell_beat = match bass {
            Some(level) => level * SWELL_DOWNBEAT / 255,
            None => {
                (beat.pulse as i32
                    * if beat.is_downbeat() {
                        SWELL_DOWNBEAT
                    } else {
                        SWELL_BEAT
                    })
                    / 255
            }
        };
        let shake = yaw_rate.abs().max(pitch_rate.abs());
        let scatter = ((shake - SPHERE_IDLE_SPIN).max(0) * SCATTER_PER_KICK / 5)
            .min(SCATTER_MAX);
        // The warp term dwarfs the other two: the ball does not pulse its way
        // out, it detonates.
        let swell = swell_beat + scatter + warp.max(0) * WARP_SWELL;

        // Coast the ball back to its idle drift, which is itself riding the
        // bar: quickest just after the downbeat, slowest going into the next.
        // Yaw settles back to the drift that rides the bar; pitch settles
        // back to nothing, so the ball ends level however it was shoved.
        let idle = SPHERE_IDLE_SPIN + (beat.bar as i32 * SPHERE_BAR_SWING) / 255;
        yaw_rate = carousel::ease_spin(yaw_rate, idle, SPHERE_DECAY_SHIFT);
        pitch_rate = carousel::ease_spin(pitch_rate, 0, SPHERE_DECAY_SHIFT);
        yaw = (yaw + yaw_rate) & (TURN - 1);
        pitch = (pitch + pitch_rate) & (TURN - 1);
        // Flying forward the whole time, and a browse shoves the camera along
        // with the ball.
        travel = travel.wrapping_add(yaw_rate.abs().max(1) + warp_speed);

        fb.clear(26, 0, 4);
        draw_starfield(travel, beat.offbeat, warp_speed);
        draw_sphere(yaw, pitch, swell, &mut beads);

        // A launch clears the header and the description away entirely:
        // nothing between the warp and the eye but the ring it is leaving,
        // and the chosen title on the front pill.
        if warp < 0 {
            paint::header_strip(HEADER_H);
            banner.draw(160 - BANNER_W / 2, BANNER_Y);
            centred(&font, BANNER_Y + BANNER_H + 2, "ARCADE", TITLE);
            centred(&small, HEADER_H - 8, DISC_VERSION, HINT);
            if let Some(header) = header {
                draw_music_panel(
                    &small,
                    &header,
                    menu_track_index,
                    &beat,
                    muted,
                    loading,
                    spectrum_frame(&header, menu_track_index, song_ms, spectrum_frames),
                );
            }
        }

        if count == 0 {
            centred(&font, 106, "DISC TABLE OF CONTENTS UNREADABLE", ERROR);
        } else {
            let index = selected.rem_euclid(count as i32) as usize;
            // The block only changes when the selection or the language does,
            // so it is rendered off-screen on those frames and blitted on the
            // rest. A glyph at a time cost about a whole vblank. The unlock
            // bit matters too: the cheat code rebuilds the list with Cortex
            // at the index the old selection held, and a key without it kept
            // showing the old entry's text under the new entry's pill.
            let key = (index as u32) << 2 | (unlocked as u32) << 1 | italian as u32;
            let hide_text = attract || warp >= 0 || debug;
            if hide_text {
                // Nothing but the ball turning over the carousel.
            } else if entries[index].exe_lba == 0 {
                if !credits_cache.holds(0) {
                    credits_cache.begin(0);
                    render_credits(&small, &header.expect("count came from it"));
                    credits_cache.end(&fb);
                }
                if !links_cache.holds(0) {
                    links_cache.begin(0);
                    render_links(&small, &icons);
                    links_cache.end(&fb);
                }
            } else if !text_cache.holds(key) {
                text_cache.begin(key);
                render_description(&small, &entries[index], italian);
                text_cache.end(&fb);
            }
            // The screenshot rides the panel's visibility. Its one VRAM slot
            // only changes at black: ease the old image out, swap while
            // nothing shows, ease the new one in. A browse, the slideshow
            // and the launch warp all ride the same three steps.
            let shot_desired: i32 = if shot_total == 0 || hide_text {
                -1
            } else {
                let entry = &entries[index];
                if shot_entry != index {
                    shot_entry = index;
                    shot_cycle = 0;
                    shot_dwell = 0;
                }
                if entry.shot_count == 0
                    || (entry.shot_first as u32 + entry.shot_count as u32) > shot_total
                {
                    -1
                } else {
                    if entry.shot_count > 1 && shot_dwell >= SHOT_SLIDE_FRAMES {
                        shot_cycle = (shot_cycle + 1) % entry.shot_count;
                        shot_dwell = 0;
                    }
                    (entry.shot_first + shot_cycle) as i32
                }
            };
            if shot_desired != shot_shown {
                shot_level -= SHOT_FADE_OUT;
                if shot_level <= 0 {
                    shot_level = 0;
                    if shot_desired >= 0 {
                        paint::upload_shot(shot_bytes(shot_desired as u8));
                    }
                    shot_shown = shot_desired;
                }
            } else if shot_shown >= 0 {
                shot_level = (shot_level + SHOT_FADE_IN).min(SHOT_FULL);
                shot_dwell += 1;
            }
            if !hide_text {
                if entries[index].exe_lba == 0 {
                    // The credits card: two stacked centred boxes where the
                    // description/screenshot pair normally sits.
                    paint::text_panel(CARD_X, CRED_Y, CARD_W, CRED_H);
                    credits_cache.draw(CARD_X + 1, CRED_Y + 1);
                    paint::text_panel(CARD_X, LINKS_Y, CARD_W, LINKS_H);
                    links_cache.draw(CARD_X + 1, LINKS_Y + 1);
                    if italian {
                        paint::flag_it(320 - paint::FLAG_W - 5, 4);
                    } else {
                        paint::flag_uk(320 - paint::FLAG_W - 5, 4);
                    }
                } else {
                    draw_text_block(&font, &text_cache, italian);
                    if shot_level > 0 && shot_shown >= 0 {
                        paint::text_panel(SHOT_BOX_X, PANEL_Y, PANEL_W, PANEL_H);
                        paint::draw_shot(SHOT_X, SHOT_Y, shot_level as u8);
                    }
                }
                // The program's own version, right-aligned just below the
                // screenshot's box. The header already says which pressing
                // this is; this says which build of the thing you are about
                // to run, which is the question when one game looks wrong
                // and ten others do not.
                let version = entries[index].version_str();
                if !version.is_empty() {
                    let w = 1 + version.len() as i16;
                    let vx = SHOT_BOX_X + PANEL_W - w * 5;
                    let vy = PANEL_Y + PANEL_H + 3;
                    small.draw_text(vx, vy, "v", NOW_PLAYING);
                    small.draw_text(vx + 5, vy, version, TRACK_NAME);
                }
            }
            draw_ring(&font, &entries[..count], ring, step, &beat, &mut order);
            if debug {
                let dur_ms = header
                    .and_then(|h| h.spectrum_span(menu_track_index as usize))
                    .map_or(0, |(_, frames)| {
                        frames * 1000 / disc_toc::SPECTRUM_FRAME_RATE
                    });
                draw_cd_debug(
                    &small,
                    menu_track + menu_track_index,
                    music.step_code(),
                    track_end.armed(),
                    track_end.quiet_polls(),
                    hsk_ticks,
                    &stat_hist,
                    adv_idle,
                    adv_btn,
                    stalls,
                    stat_timeouts,
                    stop_timeouts,
                    &events,
                    last_pos.as_ref(),
                    song_ms,
                    dur_ms,
                );
            }
        }

        gpu::draw_sync();
        psx_rt::interrupts::wait_vblank();
        fb.swap();
    }
}

/// One line inside the text cache, centred on the cache's own width.
fn cached_line(font: &FontAtlas, line: i16, text: &str, tint: (u8, u8, u8)) {
    if text.is_empty() {
        return;
    }
    let x = paint::CACHE_W / 2 - (font.text_width(text) as i16) / 2;
    font.draw_text(x, line * DESC_LEADING + 2, text, tint);
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

/// Split a title at the space nearest its middle, so the two lines on a pill
/// come out roughly even. Titles with no space stay on one line.
fn split_title(name: &str) -> (&str, &str) {
    let middle = name.len() / 2;
    let mut best: Option<usize> = None;
    for (at, byte) in name.bytes().enumerate() {
        if byte == b' ' && best.is_none_or(|b| at.abs_diff(middle) < b.abs_diff(middle)) {
            best = Some(at);
        }
    }
    match best {
        Some(at) => (&name[..at], &name[at + 1..]),
        None => (name, ""),
    }
}

/// Stars flying past the camera. `offbeat` is 255 between two beats and 0 on
/// them, so the sky twinkles in the gaps the ball and the pills leave.
///
/// `streak` is how far the camera moved this frame. During a launch that is
/// hundreds of units, and each star trails a line back to where it was a
/// frame ago, dim at the far end: the classic warp.
fn draw_starfield(travel: i32, offbeat: u8, streak: i32) {
    for i in 0..STARS {
        let star = carousel::star(i, travel);
        if !star.visible {
            continue;
        }
        let twinkler = i % 7 == 0;
        let lift = if twinkler { offbeat / 3 } else { offbeat / 8 };
        let b = star.bright.saturating_add(lift);
        let size = star.size + u16::from(twinkler && offbeat > 190);
        // Widen before scaling: `b` is a byte, and `b * 13` overflows one
        // for any brightness above 19. That wrapped silently in release and
        // collapsed green and blue to almost nothing, which is why the field
        // was a scatter of near-black red rather than stars.
        let shade = |numerator: u16| ((b as u16 * numerator) / 16) as u8;
        let head = (b, shade(13), shade(11));
        if streak > 0 {
            if let Some((tx, ty)) = carousel::streak_tail(i, travel, streak) {
                let tail = (b / 4, shade(13) / 4, shade(11) / 4);
                gpu::draw_line_gouraud(tx, ty, tail, star.x, star.y, head);
            }
        }
        // Barely tinted rather than deeply red: against a red field a red
        // star disappears, and these are meant to read as flying past.
        gpu::draw_rect_flat(star.x, star.y, size, size, head.0, head.1, head.2);
    }
}

/// `swell` pushes the beads outward from the centre, so the ball itself grows
/// and snaps back rather than each bead getting fatter in place.
fn draw_sphere(yaw: i32, pitch: i32, swell: i32, beads: &mut [Bead; SPHERE_POINTS]) {
    let n = carousel::sphere(yaw, pitch, swell, beads);
    carousel::sort_by_depth(&mut beads[..n], |b| b.z);
    let lift = (swell.clamp(0, 255) / 3) as u8;
    for bead in &beads[..n] {
        paint::bead(&Bead {
            lit: bead.lit.saturating_add(lift),
            ..*bead
        });
    }
}

/// Who made what is on the disc. The music is here by permission, and an
/// attribution that only exists in a README is not an attribution, so this is
/// the page that discharges it: artist first, then every track by name.
/// The panel, the flag, and the cached block of text on top of them.
// --- CD debug overlay ---------------------------------------------------

fn push_event(events: &mut [u8; 10], ev: u8) {
    events.copy_within(0..9, 1);
    events[0] = ev;
}

fn put_hex2(buf: &mut [u8], at: usize, v: u8) -> usize {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    buf[at] = HEX[(v >> 4) as usize];
    buf[at + 1] = HEX[(v & 0xF) as usize];
    at + 2
}

fn put_dec(buf: &mut [u8], at: usize, v: u32) -> usize {
    let mut digits = [0u8; 10];
    let mut n = 0;
    let mut v = v;
    loop {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
        if v == 0 {
            break;
        }
    }
    for i in 0..n {
        buf[at + i] = digits[n - 1 - i];
    }
    at + n
}

fn put_str(buf: &mut [u8], at: usize, s: &str) -> usize {
    buf[at..at + s.len()].copy_from_slice(s.as_bytes());
    at + s.len()
}

/// The SELECT overlay, drawn over the description block: the requested
/// track and handshake state, the raw GetStat bytes the idle detector saw
/// (newest first), and who advanced the track and why. Everything the
/// silicon track-skip needs, photographable from the couch.
#[allow(clippy::too_many_arguments)]
fn draw_cd_debug(
    small: &FontAtlas,
    requested_track: u8,
    step: u8,
    armed: bool,
    idle_polls: u8,
    hsk_ticks: u32,
    stat_hist: &[u8; 10],
    adv_idle: u16,
    adv_btn: u16,
    stalls: u16,
    stat_timeouts: u16,
    stop_timeouts: u16,
    events: &[u8; 10],
    pos: Option<&PlayPosition>,
    song_ms: u32,
    dur_ms: u32,
) {
    let x = 10;
    // Where the description panel's text used to start; the overlay keeps
    // the spot now that the panel spans the band above it too.
    let mut y = 102;
    let mut buf = [0u8; 56];
    let mut emit = |small: &FontAtlas, y: &mut i16, buf: &[u8], n: usize| {
        // SAFETY: every byte written above is ASCII.
        small.draw_text(x, *y, unsafe { core::str::from_utf8_unchecked(&buf[..n]) }, LABEL);
        *y += 9;
    };

    let mut n = put_str(&mut buf, 0, "REQ ");
    n = put_dec(&mut buf, n, requested_track as u32);
    n = put_str(&mut buf, n, " STEP ");
    n = put_dec(&mut buf, n, step as u32);
    n = put_str(&mut buf, n, " ARM ");
    n = put_dec(&mut buf, n, armed as u32);
    n = put_str(&mut buf, n, " IDLE ");
    n = put_dec(&mut buf, n, idle_polls as u32);
    buf[n] = b'/';
    n = put_dec(&mut buf, n + 1, CDDA_IDLE_POLLS_TO_ADVANCE as u32);
    n = put_str(&mut buf, n, " HSK ");
    n = put_dec(&mut buf, n, hsk_ticks);
    emit(small, &mut y, &buf, n);

    let mut n = put_str(&mut buf, 0, "STAT ");
    for (i, s) in stat_hist.iter().enumerate() {
        n = put_hex2(&mut buf, n, *s);
        if i < stat_hist.len() - 1 {
            buf[n] = b' ';
            n += 1;
        }
    }
    emit(small, &mut y, &buf, n);

    let mut n = put_str(&mut buf, 0, "ADV IDLE ");
    n = put_dec(&mut buf, n, adv_idle as u32);
    n = put_str(&mut buf, n, " BTN ");
    n = put_dec(&mut buf, n, adv_btn as u32);
    n = put_str(&mut buf, n, " STALL ");
    n = put_dec(&mut buf, n, stalls as u32);
    n = put_str(&mut buf, n, " STTO ");
    n = put_dec(&mut buf, n, stat_timeouts as u32);
    n = put_str(&mut buf, n, " SPTO ");
    n = put_dec(&mut buf, n, stop_timeouts as u32);
    emit(small, &mut y, &buf, n);

    // Where the drive says its head is (GetlocP), and where the watchdog
    // thinks the song is against its known length. "POS ?" until the
    // first successful position read.
    let mut put_msf = |buf: &mut [u8; 56], at: usize, m: u8, s: u8, f: u8| {
        let mut n = put_dec(buf, at, m as u32);
        buf[n] = b':';
        n = put_dec(buf, n + 1, s as u32);
        buf[n] = b':';
        put_dec(buf, n + 1, f as u32)
    };
    let mut n = put_str(&mut buf, 0, "POS ");
    match pos {
        Some(p) => {
            buf[n] = b'T';
            n = put_dec(&mut buf, n + 1, p.track as u32);
            buf[n] = b' ';
            buf[n + 1] = b'I';
            n = put_dec(&mut buf, n + 2, p.index as u32);
            buf[n] = b' ';
            buf[n + 1] = b'R';
            n = put_msf(&mut buf, n + 2, p.relative_min, p.relative_sec, p.relative_frame);
            buf[n] = b' ';
            buf[n + 1] = b'A';
            n = put_msf(&mut buf, n + 2, p.absolute_min, p.absolute_sec, p.absolute_frame);
        }
        None => n = put_str(&mut buf, n, "?"),
    }
    n = put_str(&mut buf, n, " MS ");
    n = put_dec(&mut buf, n, song_ms);
    buf[n] = b'/';
    n = put_dec(&mut buf, n + 1, dur_ms);
    emit(small, &mut y, &buf, n);

    let mut n = put_str(&mut buf, 0, "EV ");
    for ev in events {
        let cause = if ev & 0x80 != 0 {
            b'I'
        } else if ev & 0x40 != 0 {
            b'B'
        } else if ev & 0x20 != 0 {
            b'S' // watchdog same-track restart
        } else {
            b'-'
        };
        buf[n] = cause;
        buf[n + 1] = b'0' + (ev & 0x0F);
        buf[n + 2] = b' ';
        n += 3;
    }
    emit(small, &mut y, &buf, n);
}

fn draw_text_block(font: &FontAtlas, cache: &paint::TextCache, italian: bool) {
    let _ = font;
    paint::text_panel(PANEL_X, PANEL_Y, PANEL_W, PANEL_H);
    // Top-right, inside the header.
    if italian {
        paint::flag_it(320 - paint::FLAG_W - 5, 4);
    } else {
        paint::flag_uk(320 - paint::FLAG_W - 5, 4);
    }
    cache.draw(TEXT_X, TEXT_Y);
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

/// Top-left: what is playing, a level meter that dances on the beat, and the
/// one control worth labelling. Nothing about the screen suggests the
/// shoulder buttons do anything, so that one is spelled out.
fn draw_music_panel(
    font: &FontAtlas,
    header: &Header,
    track: u8,
    beat: &carousel::Beat,
    muted: bool,
    loading: bool,
    levels: Option<&[u8]>,
) {
    // The off slot past the last track: say so where the title would be,
    // and keep the one control that gets the music back on screen.
    if muted {
        font.draw_text(4, MUSIC_TOP, "MUSIC | L1/R1", NOW_PLAYING);
        font.draw_text(6, TRACK_TOP, "OFF", TRACK_NAME);
        return;
    }
    let title = header.title(track as usize);
    if title.is_empty() {
        return;
    }
    // All of it down the header's left edge, in reading order.
    if loading {
        // The drive takes a moment to pick a track up, and a menu that just
        // goes quiet reads as broken. Say what it is doing, where the label
        // that it is playing would be.
        font.draw_text(4, MUSIC_TOP, "LOADING", NOW_PLAYING);
        font.draw_text(6, TRACK_TOP, title, TRACK_NAME);
        return;
    }
    // Label and control share a row. Spelling out the one control worth
    // labelling costs nothing here and saves a row of header. Fifteen
    // characters, so it stops at x=79 and leaves the centred mark alone.
    // Always skippable now: the cycle holds every track plus MUSIC OFF.
    font.draw_text(4, MUSIC_TOP, "PLAYING | L1/R1", NOW_PLAYING);
    // The title brightens on the beat, so the words themselves keep time.
    let lift = beat.pulse / 4;
    let tint = (
        TRACK_NAME.0.saturating_add(lift),
        TRACK_NAME.1.saturating_add(lift),
        TRACK_NAME.2.saturating_add(lift),
    );
    font.draw_text(6, TRACK_TOP, title, tint);
    match levels {
        Some(levels) => paint::level_meter(6, METER_BASE, levels),
        // No analysis for this track: keep time off the beat instead of
        // leaving a dead space where the meter should be.
        None => paint::level_meter_beat(6, METER_BASE, beat.pulse),
    }
}

/// The selected game's blurb in one language, under the flag of whichever
/// one it is. Up or down swaps.
/// Draw a description into the cache. Coordinates are local to it.
fn render_description(font: &FontAtlas, entry: &Entry, italian: bool) {
    let text = if italian {
        entry.desc_it_str()
    } else {
        entry.desc_en_str()
    };
    // Count the lines first, so a short blurb sits centred in its box the
    // way the screenshot does in the one beside it.
    let mut lines = 0i16;
    let mut rest = text;
    while !rest.is_empty() && lines < disc_toc::DESC_LINES as i16 {
        let (_, tail) = wrap(rest, WRAP_CHARS);
        lines += 1;
        rest = tail;
    }
    let start = (disc_toc::DESC_LINES as i16 - lines) / 2;
    // Greedy wrap, a line at a time. mkdisc has already checked the text fits
    // in DESC_LINES of them, so nothing is dropped here.
    let mut rest = text;
    for line in 0..lines {
        let (head, tail) = wrap(rest, WRAP_CHARS);
        cached_line(font, start + line, head, BLURB);
        rest = tail;
    }
}

/// The carousel: place every entry on the ring, draw back to front, and label
/// each pill.
fn draw_ring(
    font: &FontAtlas,
    entries: &[Entry],
    ring: i32,
    step: i32,
    beat: &carousel::Beat,
    order: &mut [usize; MAX_ENTRIES],
) {
    let count = entries.len();
    for (slot, item) in order.iter_mut().enumerate().take(count) {
        *item = slot;
    }
    let placed = |slot: usize| -> Placed { carousel::place(ring + slot as i32 * step) };
    carousel::sort_by_depth(&mut order[..count], |slot| placed(*slot).z);

    // Reflections first, all of them, so a nearer pill's reflection cannot
    // draw over a nearer pill.
    for &slot in &order[..count] {
        let item = placed(slot);
        paint::pill_reflection(&item, carousel::reflect_y(item.y));
    }

    // Only the downbeat flashes the pills. Lifting them every beat left
    // nothing for the downbeat to be.
    let flash = if beat.is_downbeat() { beat.pulse } else { 0 };
    for &slot in &order[..count] {
        let item = placed(slot);
        paint::pill(&item, flash);

        // Titles wider than their pill are left to overhang, the way the demo
        // discs did it. The ones round the back are dropped instead: at that
        // size they are unreadable and only add clutter.
        if item.front > 96 {
            let tint = if item.front > 200 { LABEL } else { FAR_LABEL };
            let (top, bottom) = split_title(entries[slot].name_str());
            let line = |y: i16, text: &str| {
                if !text.is_empty() {
                    let width = font.text_width(text) as i16;
                    font.draw_text(item.x - width / 2, y, text, tint);
                }
            };
            if bottom.is_empty() {
                line(item.y - 4, top);
            } else {
                line(item.y - 9, top);
                line(item.y + 1, bottom);
            }
        }
    }
}

/// Read the level-meter data for every menu track. Returns how many frames
/// landed in the buffer, which is 0 when the disc carries none or when it
/// carries more than there is room for.
/// Pull every screenshot on the disc into the RAM cache, while the drive is
/// still free. Returns how many shots landed; 0 keeps the backdrop off.
fn read_shots(header: Option<&Header>, entries: &[Entry], count: usize) -> u32 {
    let Some(header) = header else { return 0 };
    if header.shots_lba == 0 {
        return 0;
    }
    // The region's extent comes from the entries: shots sit end to end, so
    // the furthest-reaching claim is the read length.
    let total = entries[..count]
        .iter()
        .map(|e| e.shot_first as u32 + e.shot_count as u32)
        .max()
        .unwrap_or(0);
    if total == 0 || total as usize > disc_toc::MAX_SHOTS {
        return 0;
    }
    // SAFETY: single-threaded, polled; only `main` reaches these statics.
    let reader = unsafe { &mut *core::ptr::addr_of_mut!(READER) };
    let buffer = unsafe { &mut *core::ptr::addr_of_mut!(SHOTS) };
    let sectors = total as usize * disc_toc::SHOT_SECTORS as usize;
    let mut ok = unsafe {
        reader.prepare() && reader.start_read(disc_base::shift_lba(header.shots_lba))
    };
    for chunk in buffer.chunks_exact_mut(SECTOR_WORDS).take(sectors) {
        let slot: &mut [u32; SECTOR_WORDS] = chunk.try_into().expect("exact chunks");
        ok = ok && unsafe { reader.read_sector(slot) };
    }
    unsafe { reader.stop() };
    if ok {
        total
    } else {
        tty::println("launcher: screenshot read failed, backdrops off");
        0
    }
}

/// Shot `index` of the RAM cache, as the bytes `paint::upload_shot`
/// wants. The cache is `u32` for the sector reader's sake; the reinterpret
/// down to bytes is always aligned.
fn shot_bytes(index: u8) -> &'static [u8] {
    // SAFETY: single-threaded; read_shots finished with the buffer at boot.
    let buffer = unsafe { &*core::ptr::addr_of!(SHOTS) };
    let words = &buffer[index as usize * SHOT_SLOT_WORDS..];
    // SAFETY: u32 -> u8 loosens alignment; SHOT_BYTES fits inside a slot.
    unsafe {
        core::slice::from_raw_parts(words.as_ptr() as *const u8, disc_toc::SHOT_BYTES)
    }
}

fn read_spectrum(header: Option<&Header>) -> u32 {
    let Some(header) = header else { return 0 };
    if header.spectrum_lba == 0 {
        return 0;
    }
    let total: u32 = header.spectrum_frames.iter().sum();
    let bytes = total as usize * disc_toc::SPECTRUM_BANDS;
    let sectors = bytes.div_ceil(SECTOR_WORDS * 4);
    if sectors > SPECTRUM_MAX_SECTORS {
        tty::println("launcher: spectrum too large for the buffer, meter off");
        return 0;
    }
    // SAFETY: single-threaded, polled; only `main` reaches these statics.
    let reader = unsafe { &mut *core::ptr::addr_of_mut!(READER) };
    let buffer = unsafe { &mut *core::ptr::addr_of_mut!(SPECTRUM) };

    let mut ok = unsafe {
        reader.prepare() && reader.start_read(disc_base::shift_lba(header.spectrum_lba))
    };
    for chunk in buffer.chunks_exact_mut(SECTOR_WORDS).take(sectors) {
        let slot: &mut [u32; SECTOR_WORDS] = chunk.try_into().expect("exact chunks");
        ok = ok && unsafe { reader.read_sector(slot) };
    }
    unsafe { reader.stop() };
    if ok {
        total
    } else {
        0
    }
}

/// The band levels to draw right now, or `None` when this track has no
/// analysis on the disc.
fn spectrum_frame(header: &Header, track: u8, song_ms: u32, loaded_frames: u32) -> Option<&[u8]> {
    if loaded_frames == 0 {
        return None;
    }
    let (offset, frames) = header.spectrum_span(track as usize)?;
    let frame = song_ms * disc_toc::SPECTRUM_FRAME_RATE / 1000;
    // Hold the last frame rather than wrapping: the clock can run a little
    // past the end of a track while the drive notices it has finished.
    let at = (offset + frame.min(frames.saturating_sub(1))) as usize;
    let start = at * disc_toc::SPECTRUM_BANDS;
    // SAFETY: as `read_spectrum`; read-only here.
    let buffer = unsafe { &*core::ptr::addr_of!(SPECTRUM) };
    let bytes = unsafe {
        core::slice::from_raw_parts(buffer.as_ptr() as *const u8, buffer.len() * 4)
    };
    bytes.get(start..start + disc_toc::SPECTRUM_BANDS)
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
    let mut ok = unsafe { reader.prepare() && reader.start_read(disc_base::shift_lba(TOC_LBA)) };
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
    // still on screen, and get the drive off CD-DA before the blob starts
    // reading sectors with it.
    gpu::draw_sync();
    // Stop only gets ACCEPTED promptly; on silicon the drive then spins
    // down for a good fraction of a second, and chain-load reads issued
    // into that window fail (the debug burn's stage-1 prepare panel). The
    // SDK helper waits, bounded, until the drive is genuinely quiet.
    let _ = cdrom::stop_and_settle(CDDA_SPINS, 240);

    // SAFETY: `LOADER_BASE` is above every game's payload (mkdisc enforces
    // that) and below the stack, so nothing live is being overwritten. The
    // blob is position-dependent and linked for exactly this address.
    unsafe {
        core::ptr::copy_nonoverlapping(
            LOADER_BLOB.as_ptr(),
            LOADER_BASE as *mut u8,
            LOADER_BLOB.len(),
        );
        psx_rt::cache::flush_i_cache();
        let blob: unsafe extern "C" fn(u32, u32, u32, u32) -> ! =
            core::mem::transmute(LOADER_BASE as usize);
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
