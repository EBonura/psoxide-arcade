//! Goncharov as an XA song: found by name in the disc's root directory, looped
//! by the SDK's `Player`, and timed by the drive's own head position so the
//! spectrum bars follow the sound instead of a frame count.
//!
//! The file is `MUSIC1.XA`, packed by `psx-audio-cook xa-pack` (see
//! `arcade-assets/audio/README.md`); the table it wrote sits next to it and is
//! compiled in below. The drive decodes the audio itself, so the CPU's whole
//! share is one position read every few frames. The drive is busy while the
//! song plays, so the directory is read once, before the first note, and
//! nothing reads the disc afterwards.

use psx_fmv::iso;
use psx_io::cd::reader::{SectorReader, SECTOR_WORDS};
use psx_io::cd::xa::{DriveSpeed, Event, File, Player, Song};
use psx_io::periph::Cd;

/// The table `xa-pack` wrote: file name, number, sector count and speed, and
/// per song its file, channel and length in milliseconds.
#[allow(dead_code)]
mod pack {
    include!("../../../arcade-assets/audio/xa/songs.rs");
}

/// Frames the song waits after the game starts before the first note, so the
/// first picture is up before the drive spins.
const START_DELAY_TICKS: u32 = 30;
/// Ticks before a start the drive refused is tried again.
const RETRY_TICKS: u32 = 60;
/// A start not seen streaming by then is redone from a stop. Covers a
/// spin-up and a long seek with room to spare.
const START_TIMEOUT_TICKS: u32 = 300;
/// Position reads a second while the song plays (every tenth of a second keeps
/// the clock within a sector or two).
const POLLS_PER_SECOND: u32 = 10;
/// Position reads every tick this close to the end of the song, so the loop
/// restart is noticed within a frame and not within a tenth of a second.
const END_WATCH_MS: u32 = 1_500;
/// How far the drive's head runs ahead of the sound, in milliseconds. The
/// head sits a sector or two past what is audible and the drive decodes in
/// four-sector groups. An estimate from the SDK notes, not a console
/// measurement; it only shifts when the bars land against the audio.
const HEAD_LEAD_MS: u32 = 40;
/// A reading further than this from the running clock is a seek or a restart,
/// not jitter: take it whole.
const SNAP_MS: u32 = 80;

#[derive(Copy, Clone, PartialEq, Eq)]
enum State {
    /// No song on this disc, or the player has none to run.
    Silent,
    /// Waiting for the first start at this tick.
    Waiting(u32),
    /// A start was made at this tick and the head has not been seen yet.
    Starting(u32),
    /// The drive refused a start; try again at this tick.
    Retry(u32),
    /// The head is inside the song.
    Playing,
}

pub struct Music {
    player: Player,
    song: Option<Song>,
    duration_ms: u32,
    state: State,
    next_poll: u32,
    // The song clock: the last head reading and the tick it was taken at.
    anchor_ms: u32,
    anchor_tick: u32,
    running: bool,
    /// Times the song reached its end and started again.
    pub loops: u16,
    /// Starts redone because the head never showed up.
    pub stalls: u16,
    /// Starts the drive refused outright.
    pub refusals: u16,
}

impl Music {
    /// Find the song and hold the drive. Reads the root directory, so it has
    /// to run before any note plays. A disc without the file (or with one that
    /// does not match the table this game was built with) has no music.
    pub fn locate() -> Music {
        // SAFETY: nothing else in this program drives the CD controller; the
        // reader and then the player are its only owners, one after the other.
        let mut reader = SectorReader::with_cd(unsafe { Cd::steal() });
        let found = find_song(&mut reader);
        let cd = reader.release();
        let duration_ms = pack::XA_SONGS[0].2;
        let mut player = Player::new(cd);
        // The drive's own mixer has to pass the song to the SPU; 0x80 is unity.
        player.set_volume(0x80, 0x80);
        Music {
            player,
            song: found,
            duration_ms,
            state: if found.is_some() {
                State::Waiting(START_DELAY_TICKS)
            } else {
                State::Silent
            },
            next_poll: 0,
            anchor_ms: 0,
            anchor_tick: 0,
            running: false,
            loops: 0,
            stalls: 0,
            refusals: 0,
        }
    }

    /// Whether the clock has a reading to run from.
    pub fn playing(&self) -> bool {
        self.running
    }

    /// Song time in milliseconds at tick `tick` (`hz` ticks a second); 0
    /// while nothing is audible. Held at the song's length until the loop
    /// restart is seen.
    pub fn song_ms(&self, tick: u32, hz: u32) -> u32 {
        if !self.running {
            return 0;
        }
        let ticks = tick.wrapping_sub(self.anchor_tick) as u64;
        let ms = self.anchor_ms as u64 + ticks * 1000 / hz.max(1) as u64;
        (ms as u32).min(self.duration_ms)
    }

    /// Once a tick: start the song, read the head, notice a loop, redo a start
    /// that did not take.
    pub fn update(&mut self, tick: u32, hz: u32) {
        match self.state {
            State::Silent => {}
            State::Waiting(at) | State::Retry(at) => {
                if due(tick, at) {
                    self.start(tick, hz);
                }
            }
            State::Starting(_) | State::Playing => {
                if !due(tick, self.next_poll) {
                    return;
                }
                let near_end =
                    self.running && self.song_ms(tick, hz) + END_WATCH_MS >= self.duration_ms;
                let every = if near_end {
                    1
                } else {
                    (hz / POLLS_PER_SECOND).max(1)
                };
                self.next_poll = tick.wrapping_add(every);
                match self.player.poll() {
                    Event::Looped => {
                        // The restart is a seek: quiet until the head is back
                        // inside the file, and the bars with it.
                        self.running = false;
                        self.state = State::Starting(tick);
                        self.loops = self.loops.saturating_add(1);
                        crate::game_trace("magikarp: xa looped");
                    }
                    Event::Finished => self.start(tick, hz),
                    Event::Idle => {}
                    Event::Playing => {
                        if self.player.is_streaming() {
                            self.state = State::Playing;
                            let ms = self.player.elapsed_millis().saturating_sub(HEAD_LEAD_MS);
                            self.observe(tick, hz, ms);
                        } else if let State::Starting(since) = self.state {
                            if tick.wrapping_sub(since) > START_TIMEOUT_TICKS {
                                self.stalls = self.stalls.saturating_add(1);
                                self.start(tick, hz);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Begin the song from the top, looping.
    fn start(&mut self, tick: u32, hz: u32) {
        self.running = false;
        self.player.stop();
        let Some(song) = self.song else {
            self.state = State::Silent;
            return;
        };
        self.next_poll = tick.wrapping_add((hz / POLLS_PER_SECOND).max(1));
        match self.player.play(song, true) {
            Ok(()) => {
                self.state = State::Starting(tick);
                crate::game_trace("magikarp: xa ok");
            }
            Err(_) => {
                self.refusals = self.refusals.saturating_add(1);
                self.state = State::Retry(tick.wrapping_add(RETRY_TICKS));
            }
        }
    }

    /// The drive says the song is `song_ms` in, at tick `tick`. Pull toward it
    /// rather than jump, so one late poll moves the bars a few milliseconds
    /// and not a sector.
    fn observe(&mut self, tick: u32, hz: u32, song_ms: u32) {
        if self.running {
            let predicted = self.song_ms(tick, hz);
            let error = song_ms.abs_diff(predicted);
            self.anchor_ms = if error > SNAP_MS {
                song_ms
            } else if song_ms >= predicted {
                predicted + error / 4
            } else {
                predicted - error / 4
            };
        } else {
            self.anchor_ms = song_ms;
            self.running = true;
        }
        self.anchor_tick = tick;
    }
}

/// Whether wrapping tick `now` has reached `at`.
fn due(now: u32, at: u32) -> bool {
    now.wrapping_sub(at) < u32::MAX / 2
}

/// Look the song's file up in the root directory. `Some` when it is there at
/// the size the table says.
fn find_song(reader: &mut SectorReader) -> Option<Song> {
    let mut sector = [0u32; SECTOR_WORDS];
    if !read_one(reader, iso::PVD_LBA, &mut sector) {
        return None;
    }
    let (root_lba, _) = iso::root_directory(as_bytes(&sector))?;
    if !read_one(reader, root_lba, &mut sector) {
        return None;
    }
    let name = pack::XA_FILE_NAMES[0];
    let (lba, bytes) = iso::find_in_directory(as_bytes(&sector), name)?;
    if bytes / 2048 != pack::XA_FILE_SECTORS[0] {
        crate::game_trace("magikarp: xa file does not match the table, music off");
        return None;
    }
    let (file_index, channel, _) = pack::XA_SONGS[0];
    let speed = if pack::XA_SPEED_DOUBLE {
        DriveSpeed::Double
    } else {
        DriveSpeed::Single
    };
    Some(
        File::new(
            lba,
            bytes / 2048,
            pack::XA_FILE_NUMBERS[file_index as usize],
            speed,
        )
        .song(channel),
    )
}

fn read_one(reader: &mut SectorReader, lba: u32, into: &mut [u32; SECTOR_WORDS]) -> bool {
    let ok = reader.prepare() && reader.start_read(lba) && reader.read_sector(into);
    reader.stop();
    ok
}

fn as_bytes(sector: &[u32; SECTOR_WORDS]) -> &[u8] {
    // SAFETY: a [u32; 512] is 2048 initialised bytes, and u8 has no alignment.
    unsafe { core::slice::from_raw_parts(sector.as_ptr() as *const u8, SECTOR_WORDS * 4) }
}
