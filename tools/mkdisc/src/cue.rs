//! Just enough CUE parsing for the discs PSoXide builds: one BINARY file, a
//! MODE2/2352 data track, then audio tracks appended in order.
//!
//! Everything is measured in frames (sectors), relative to the start of the
//! BIN, because that is what `mkdisc` needs to slice an image apart and
//! re-time it somewhere else on a bigger disc.

use std::path::{Path, PathBuf};

/// One CD-DA track inside a game's own image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioTrack {
    /// Pregap start, frames from the start of the audio region.
    pub index00: u32,
    /// Playback start, frames from the start of the audio region.
    pub index01: u32,
}

/// A parsed CUE: where the BIN is, how much of it is track 1, and the audio
/// tracks that follow.
#[derive(Debug, Clone)]
pub struct Sheet {
    /// The BIN the CUE points at, resolved against the CUE's own directory.
    pub bin: PathBuf,
    /// Frames of track 1 (the data track).
    pub data_frames: u32,
    /// Audio tracks, in disc order, timed from the end of the data track.
    pub audio: Vec<AudioTrack>,
}

fn parse_msf(text: &str) -> Result<u32, String> {
    let mut parts = text.split(':');
    let mut next = |what: &str| -> Result<u32, String> {
        parts
            .next()
            .ok_or_else(|| format!("timestamp {text:?}: missing {what}"))?
            .parse::<u32>()
            .map_err(|_| format!("timestamp {text:?}: {what} is not a number"))
    };
    let minutes = next("minutes")?;
    let seconds = next("seconds")?;
    let frames = next("frames")?;
    if parts.next().is_some() {
        return Err(format!("timestamp {text:?}: too many fields"));
    }
    Ok((minutes * 60 + seconds) * 75 + frames)
}

/// Parse `path`. Only the shapes PSoXide's own disc builder emits are
/// accepted; anything else is an error rather than a guess.
pub fn parse(path: &Path) -> Result<Sheet, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));

    let mut bin = None;
    let mut audio = Vec::new();
    let mut data_frames = None;
    // Which track the INDEX lines currently belong to: None until the first
    // TRACK, then Some(is_audio).
    let mut in_audio_track = false;
    let mut seen_data_track = false;

    for line in text.lines() {
        let line = line.trim();
        let mut words = line.split_whitespace();
        match words.next() {
            Some("FILE") => {
                let quoted = line
                    .split('"')
                    .nth(1)
                    .ok_or_else(|| format!("{}: FILE without a quoted name", path.display()))?;
                if bin.replace(dir.join(quoted)).is_some() {
                    return Err(format!("{}: more than one FILE", path.display()));
                }
            }
            Some("TRACK") => {
                let _number = words.next();
                match words.next() {
                    Some("AUDIO") => {
                        in_audio_track = true;
                        audio.push(AudioTrack {
                            index00: u32::MAX,
                            index01: u32::MAX,
                        });
                    }
                    Some(mode) if mode.starts_with("MODE") => {
                        if seen_data_track {
                            return Err(format!("{}: more than one data track", path.display()));
                        }
                        seen_data_track = true;
                        in_audio_track = false;
                    }
                    other => {
                        return Err(format!("{}: unsupported TRACK type {other:?}", path.display()))
                    }
                }
            }
            Some("INDEX") => {
                let number = words.next().unwrap_or("");
                let at = parse_msf(words.next().unwrap_or(""))?;
                if !in_audio_track {
                    continue;
                }
                // The first audio track's earliest index marks the end of the
                // data track: everything before it is track 1.
                if data_frames.is_none() {
                    data_frames = Some(at);
                }
                let base = data_frames.unwrap_or(0);
                let track = audio.last_mut().expect("INDEX inside an audio track");
                let relative = at.saturating_sub(base);
                match number {
                    "00" => track.index00 = relative,
                    "01" => track.index01 = relative,
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let bin = bin.ok_or_else(|| format!("{}: no FILE line", path.display()))?;
    if !seen_data_track {
        return Err(format!("{}: no data track", path.display()));
    }
    for track in &mut audio {
        if track.index01 == u32::MAX {
            return Err(format!("{}: an audio track has no INDEX 01", path.display()));
        }
        // A track without a pregap starts where it plays.
        if track.index00 == u32::MAX {
            track.index00 = track.index01;
        }
    }

    let bin_frames = {
        let len = std::fs::metadata(&bin)
            .map_err(|e| format!("stat {}: {e}", bin.display()))?
            .len();
        (len / psx_iso::SECTOR_BYTES as u64) as u32
    };
    Ok(Sheet {
        // With no audio tracks the whole BIN is data.
        data_frames: data_frames.unwrap_or(bin_frames),
        bin,
        audio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_sheet(dir: &Path, cue: &str, bin_sectors: usize) -> PathBuf {
        let bin = dir.join("game.bin");
        std::fs::File::create(&bin)
            .unwrap()
            .write_all(&vec![0u8; bin_sectors * psx_iso::SECTOR_BYTES])
            .unwrap();
        let path = dir.join("game.cue");
        std::fs::write(&path, cue).unwrap();
        path
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mkdisc-cue-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn splits_data_from_audio_and_times_tracks_relatively() {
        let dir = scratch("split");
        let path = write_sheet(
            &dir,
            "FILE \"game.bin\" BINARY\n\
               TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n\
               TRACK 02 AUDIO\n    INDEX 00 00:10:00\n    INDEX 01 00:12:00\n\
               TRACK 03 AUDIO\n    INDEX 00 00:20:00\n    INDEX 01 00:22:00\n",
            2000,
        );
        let sheet = parse(&path).expect("parses");
        assert_eq!(sheet.data_frames, 10 * 75);
        assert_eq!(sheet.audio.len(), 2);
        assert_eq!(sheet.audio[0], AudioTrack { index00: 0, index01: 150 });
        assert_eq!(
            sheet.audio[1],
            AudioTrack {
                index00: 10 * 75,
                index01: 10 * 75 + 150
            }
        );
    }

    #[test]
    fn a_data_only_disc_is_all_data() {
        let dir = scratch("dataonly");
        let path = write_sheet(
            &dir,
            "FILE \"game.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
            361,
        );
        let sheet = parse(&path).expect("parses");
        assert_eq!(sheet.data_frames, 361);
        assert!(sheet.audio.is_empty());
    }

    #[test]
    fn an_audio_track_without_a_pregap_starts_where_it_plays() {
        let dir = scratch("nopregap");
        let path = write_sheet(
            &dir,
            "FILE \"game.bin\" BINARY\n\
               TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n\
               TRACK 02 AUDIO\n    INDEX 01 00:10:00\n",
            1000,
        );
        let sheet = parse(&path).expect("parses");
        assert_eq!(sheet.audio[0], AudioTrack { index00: 0, index01: 0 });
    }

    #[test]
    fn rejects_a_sheet_with_no_data_track() {
        let dir = scratch("nodata");
        let path = write_sheet(
            &dir,
            "FILE \"game.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
            10,
        );
        assert!(parse(&path).is_err());
    }
}
