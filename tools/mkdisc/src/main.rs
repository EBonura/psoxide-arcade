//! Build the demo disc: one launcher, N chain-loadable programs, one image.
//!
//! A program that ships on its own disc has that disc's geometry baked in:
//! pack LBAs. Rather than re-cook every program, `mkdisc` drops each game's
//! *existing* disc image onto the big one unchanged and records how far its
//! image moved. The loader hands that to the game's `_start`, and
//! `psx_io::disc_base` applies it inside the SDK's disc entry points. Nothing
//! in a game's own source knows any of this happened.
//!
//! Disc layout:
//!
//! ```text
//! 21   SYSTEM.CNF     boot record, points the BIOS at PSX.EXE
//! 22   DEMOTOC.BIN    the table the launcher reads (fixed LBA, see disc-toc)
//! 23+  PSX.EXE        the launcher
//! ..   bare EXEs      programs with no disc data of their own
//! ..   game images    each game's whole data track, verbatim but re-addressed
//! ..   XA files       music (--xa), as ISO files in the launcher's root
//! ```
//!
//! Usage:
//!
//! ```text
//! mkdisc --launcher <exe> --out <bin> [--volume ID]
//!        [--game NAME=<exe>] [--image NAME=<cue>] ...
//! ```
//!
//! `--game` embeds a bare PSX-EXE, for programs that never touch the disc
//! after boot. `--image` takes a whole `.cue`, for programs that stream data
//! (data only; the collection's music is XA files in the root directory, see
//! `--xa`). `NAME` is what the menu shows; `=` splits it from the path.

mod cue;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use disc_toc::Entry;
use psx_iso::iso9660::{default_system_cnf, IsoBuilder, SECTOR_SIZE};
use psx_iso::SECTOR_BYTES;

/// Where the chain-load blob runs (`loader/loader.ld`). A program whose
/// payload reaches this address would be overwritten by the very code loading
/// it, so the disc build refuses it.
const LOADER_BASE: u32 = 0x801F_0000;

/// Lowest legal PSX-EXE load address (`sdk/psoxide.ld`).
const MIN_LOAD_ADDR: u32 = 0x8001_0000;

/// Every PSoXide disc puts its boot EXE here (`psx_iso::PLAYTEST_BOOT_EXE_START_LBA`).
/// Checked against the magic when an image is placed, so an image that breaks
/// the convention fails the build instead of booting into noise.
const IMAGE_BOOT_EXE_LBA: u32 = psx_iso::PLAYTEST_BOOT_EXE_START_LBA;

/// Frames the drive's absolute time runs ahead of an LBA (the two-second
/// lead-in every disc has before LBA 0).
const PREGAP_FRAMES: u32 = 150;

/// Bytes of one raw XA sector as `psx-audio-cook` writes it: the 8-byte
/// subheader, 2324 bytes of audio and the 4-byte EDC.
const XA_SECTOR_BYTES: usize = 2336;

const EXE_MAGIC: &[u8; 8] = b"PS-X EXE";

/// FNV-1a-32 over `bytes`: the payload checksum the chain loader recomputes
/// over RAM before jumping. Must match the loader's implementation.
fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811C_9DC5;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Cooked menu-backdrop screenshots, one after another on sector boundaries.
const SHOTS_FILE_NAME: &str = "SHOTS.BIN";

enum Source {
    /// A bare PSX-EXE, embedded as an ISO file.
    Exe(PathBuf),
    /// A whole disc image, placed verbatim.
    Image(PathBuf),
}

struct Program {
    name: String,
    source: Source,
}

struct Args {
    launcher: PathBuf,
    out: PathBuf,
    volume: String,
    programs: Vec<Program>,
    /// XA-ADPCM song files (`psx-audio-cook xa-pack`) the launcher's ISO
    /// carries as root files, for a program to find by name.
    xa: Vec<PathBuf>,
    /// `(name, english, italian)` blurbs shown under the carousel.
    descriptions: Vec<(String, String, String)>,
    /// `NAME=VERSION`, the version each program's own source declares.
    versions: Vec<(String, String)>,
    /// Display names pressed onto the disc but held off the carousel until
    /// the cheat code reveals them.
    gates: Vec<String>,
    /// `(name, cooked blob)` menu backdrops, in slideshow order per name.
    shots: Vec<(String, PathBuf)>,
}

fn parse_args() -> Result<Args, String> {
    let mut launcher = None;
    let mut out = None;
    let mut volume = String::from("PSXDEMO");
    let mut programs = Vec::new();
    let mut xa: Vec<PathBuf> = Vec::new();
    let mut descriptions = Vec::new();
    let mut versions = Vec::new();
    let mut gates: Vec<String> = Vec::new();
    let mut shots: Vec<(String, PathBuf)> = Vec::new();

    let split = |spec: &str, flag: &str| -> Result<(String, PathBuf), String> {
        let (name, path) = spec
            .split_once('=')
            .ok_or_else(|| format!("{flag} wants NAME=path, got {spec:?}"))?;
        Ok((name.to_string(), PathBuf::from(path)))
    };

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--launcher" => {
                launcher = Some(PathBuf::from(
                    it.next().ok_or("--launcher takes a path".to_string())?,
                ))
            }
            "--out" => {
                out = Some(PathBuf::from(
                    it.next().ok_or("--out takes a path".to_string())?,
                ))
            }
            "--volume" => volume = it.next().ok_or("--volume takes a string".to_string())?,
            "--game" => {
                let (name, path) = split(&it.next().ok_or("--game takes NAME=path")?, "--game")?;
                programs.push(Program {
                    name,
                    source: Source::Exe(path),
                });
            }
            "--image" => {
                let (name, path) = split(&it.next().ok_or("--image takes NAME=path")?, "--image")?;
                programs.push(Program {
                    name,
                    source: Source::Image(path),
                });
            }
            "--xa" => xa.push(PathBuf::from(
                it.next().ok_or("--xa takes a path".to_string())?,
            )),
            "--version-of" => {
                let spec = it.next().ok_or("--version-of takes NAME=VERSION")?;
                let (name, version) = spec
                    .split_once('=')
                    .ok_or_else(|| format!("--version-of wants NAME=VERSION, got {spec:?}"))?;
                versions.push((name.to_string(), version.trim().to_string()));
            }
            "--gate" => gates.push(it.next().ok_or("--gate takes a display NAME".to_string())?),
            "--shot" => {
                let (name, path) = split(&it.next().ok_or("--shot takes NAME=path")?, "--shot")?;
                shots.push((name, path));
            }
            "--describe" => {
                let spec = it.next().ok_or("--describe takes NAME=ENGLISH|ITALIAN")?;
                let (name, text) = spec.split_once('=').ok_or_else(|| {
                    format!("--describe wants NAME=ENGLISH|ITALIAN, got {spec:?}")
                })?;
                let (english, italian) = text.split_once('|').ok_or_else(|| {
                    format!("--describe wants the two languages split by '|', got {text:?}")
                })?;
                descriptions.push((
                    name.to_string(),
                    english.trim().to_string(),
                    italian.trim().to_string(),
                ));
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    Ok(Args {
        launcher: launcher.ok_or("--launcher is required".to_string())?,
        out: out.ok_or("--out is required".to_string())?,
        volume,
        programs,
        xa,
        descriptions,
        versions,
        gates,
        shots,
    })
}

fn print_usage() {
    println!(
        "mkdisc --launcher <exe> --out <bin> [--volume ID]\n\
        \x20      [--game NAME=<exe>] [--image NAME=<cue>] [--xa <file.XA>] ...\n\n\
         --game        embeds a bare PSX-EXE (programs that never read the disc)\n\
         --image       places a whole game disc image (data track only)\n\
         --xa          adds an XA-ADPCM song file (psx-audio-cook xa-pack) to the\n\
        \x20             launcher's ISO as a root file, named as the file is\n\
         --describe    NAME=ENGLISH|ITALIAN, the blurb under the carousel\n\
         --gate        NAME stays off the carousel until the cheat code\n\
         --shot        NAME=<blob> adds a menu backdrop for the program; repeat\n\
        \x20             for a slideshow (cook the blob with tools/cook-shots.py)"
    );
}

/// The bits of a PSX-EXE header the disc build cares about.
struct ExeHeader {
    load_addr: u32,
    payload_bytes: u32,
}

fn parse_exe_header(bytes: &[u8], what: &Path) -> Result<ExeHeader, String> {
    if bytes.len() < SECTOR_SIZE {
        return Err(format!("{}: shorter than a PSX-EXE header", what.display()));
    }
    if &bytes[..8] != EXE_MAGIC {
        return Err(format!("{}: not a PSX-EXE (bad magic)", what.display()));
    }
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    Ok(ExeHeader {
        load_addr: word(0x18),
        payload_bytes: word(0x1C),
    })
}

/// Reject a program that would be loaded on top of the chain-load blob.
///
/// The blob streams the payload in and then jumps, so only the loaded payload
/// matters here: `.bss` is zeroed by the target's own `_start`, after the blob
/// has stopped running.
fn check_fits_below_loader(header: &ExeHeader, what: &Path) -> Result<(), String> {
    if header.load_addr < MIN_LOAD_ADDR {
        return Err(format!(
            "{}: loads at {:#010X}, below the {:#010X} floor",
            what.display(),
            header.load_addr,
            MIN_LOAD_ADDR
        ));
    }
    let end = header.load_addr.saturating_add(header.payload_bytes);
    if end > LOADER_BASE {
        return Err(format!(
            "{}: payload ends at {:#010X}, past the chain-load blob at {:#010X}. \
             Shrink the program or move LOADER_BASE (loader/loader.ld, launcher, mkdisc).",
            what.display(),
            end,
            LOADER_BASE
        ));
    }
    Ok(())
}

fn sectors_for(bytes: usize) -> u32 {
    bytes.div_ceil(SECTOR_SIZE) as u32
}

/// ISO 9660 short name for a menu entry: uppercase alphanumerics, truncated
/// to 6 characters, plus a 2-digit index and `.EXE`.
fn iso_name(display: &str, index: usize) -> String {
    let stem: String = display
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(6)
        .collect::<String>()
        .to_ascii_uppercase();
    if stem.is_empty() {
        format!("GAME{index:02}.EXE")
    } else {
        format!("{stem}{index:02}.EXE")
    }
}

/// Rewrite a relocated data sector's BCD MSF header so the drive's seeks land
/// where the sector actually is.
///
/// Safe to do in place: Mode 2 Form 1 ECC is computed with these four bytes
/// zeroed and the EDC covers `0x10..0x818`, so neither depends on them (see
/// `encode_mode2_form1_edc_ecc` in `psx-iso`).
fn rewrite_sector_msf(sector: &mut [u8], lba: u32) {
    let bcd = |v: u32| (((v / 10) << 4) | (v % 10)) as u8;
    let absolute = lba + PREGAP_FRAMES;
    sector[0x0C] = bcd(absolute / (60 * 75));
    sector[0x0D] = bcd((absolute / 75) % 60);
    sector[0x0E] = bcd(absolute % 75);
}

/// Place `data` (a whole data track, raw 2352-byte sectors) at `lba`,
/// re-addressing every sector on the way.
fn place_data_track(image: &mut Vec<u8>, data: &[u8], lba: u32) -> Result<(), String> {
    if data.len() % SECTOR_BYTES != 0 {
        return Err(format!(
            "data track is {} bytes, not a whole number of {SECTOR_BYTES}-byte sectors",
            data.len()
        ));
    }
    for (index, sector) in data.chunks(SECTOR_BYTES).enumerate() {
        let at = image.len();
        image.extend_from_slice(sector);
        rewrite_sector_msf(&mut image[at..at + SECTOR_BYTES], lba + index as u32);
    }
    Ok(())
}

fn msf(frames: u32) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        frames / (60 * 75),
        (frames / 75) % 60,
        frames % 75
    )
}

/// The CUE for the finished disc: one data track. The collection's music is
/// XA files inside that track, so there are no audio tracks to list.
fn write_cue(path: &Path, bin_name: &str) -> Result<(), String> {
    let text =
        format!("FILE \"{bin_name}\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n");
    fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

/// A game image sliced out of its own disc, ready to be re-placed.
struct LoadedImage {
    data: Vec<u8>,
}

fn load_image(path: &Path) -> Result<LoadedImage, String> {
    let sheet = cue::parse(path)?;
    let bytes = fs::read(&sheet.bin).map_err(|e| format!("read {}: {e}", sheet.bin.display()))?;
    let split = sheet.data_frames as usize * SECTOR_BYTES;
    if bytes.len() < split {
        return Err(format!(
            "{}: claims {} data frames but the BIN only holds {}",
            path.display(),
            sheet.data_frames,
            bytes.len() / SECTOR_BYTES
        ));
    }
    if !sheet.audio.is_empty() || bytes.len() != split {
        return Err(format!(
            "{}: carries CD-DA audio. The collection's music is XA (--xa), so an image \
             must be a data track only.",
            path.display()
        ));
    }
    Ok(LoadedImage {
        data: bytes[..split].to_vec(),
    })
}

/// How many lines `text` takes at `columns`, wrapping on spaces the same way
/// the launcher does. Fitting the byte budget is not enough on its own: a
/// description can be short and still wrap to one line more than there is
/// room for, and the tail would silently vanish.
fn wrapped_lines(text: &str, columns: usize) -> usize {
    let mut rest = text;
    let mut lines = 0;
    while !rest.is_empty() {
        lines += 1;
        if rest.len() <= columns {
            break;
        }
        rest = match rest[..columns].rfind(' ') {
            Some(at) => rest[at + 1..].trim_start(),
            None => rest[columns..].trim_start(),
        };
    }
    lines
}

/// Attach each blurb to its program.
/// Stamp each program with the version its own source declares.
///
/// The disc could always say which pressing it was, from `git describe` in the
/// launcher's header, but not which build of anything it carried. Eleven
/// programs come from eight repositories at eight different moments, so
/// "v0.16" answered a question nobody was asking when a single game looked
/// wrong.
fn apply_versions(
    entries: &mut [Entry],
    names: &[&str],
    versions: &[(String, String)],
) -> Result<(), String> {
    for (name, version) in versions {
        let at = names
            .iter()
            .position(|n| n == name)
            .ok_or_else(|| format!("--version-of names {name:?}, which is not on this disc"))?;
        if version.is_empty() {
            return Err(format!(
                "--version-of gives {name:?} an empty version. Omit the flag instead: a program \
                 that declares no version should not have the disc claim one."
            ));
        }
        if version.len() > disc_toc::VERSION_BYTES {
            return Err(format!(
                "{name}: version {version:?} is {} characters, {} over the {} the table holds",
                version.len(),
                version.len() - disc_toc::VERSION_BYTES,
                disc_toc::VERSION_BYTES
            ));
        }
        entries[at] = entries[at].versioned(version);
    }
    Ok(())
}

/// Mark gated programs hidden. An unknown name fails the build: a gate that
/// silently does not take would ship the one thing it existed to hold back.
fn apply_gates(entries: &mut [Entry], names: &[&str], gates: &[String]) -> Result<(), String> {
    for name in gates {
        let at = names
            .iter()
            .position(|n| n == name)
            .ok_or_else(|| format!("--gate names {name:?}, which is not on this disc"))?;
        entries[at] = entries[at].gated();
    }
    Ok(())
}

/// Point each entry at its slice of the shot region.
fn apply_shots(
    entries: &mut [Entry],
    names: &[&str],
    spans: &[(String, u8, u8)],
) -> Result<(), String> {
    for (name, first, count) in spans {
        let at = names
            .iter()
            .position(|n| n == name)
            .ok_or_else(|| format!("--shot names {name:?}, which is not on this disc"))?;
        entries[at] = entries[at].with_shots(*first, *count);
    }
    Ok(())
}

fn apply_descriptions(
    entries: &mut [Entry],
    names: &[&str],
    descriptions: &[(String, String, String)],
) -> Result<(), String> {
    for (name, english, italian) in descriptions {
        let at = names
            .iter()
            .position(|n| n == name)
            .ok_or_else(|| format!("--describe names {name:?}, which is not on this disc"))?;
        // The table pads to a fixed width, so an overlong blurb would be
        // quietly cut mid-word on screen. Say so instead.
        for (language, text) in [("English", english), ("Italian", italian)] {
            if text.len() > disc_toc::DESC_BYTES {
                return Err(format!(
                    "{name}: the {language} description is {} characters, {} over the {} the \
                     table holds:\n  {text}",
                    text.len(),
                    text.len() - disc_toc::DESC_BYTES,
                    disc_toc::DESC_BYTES
                ));
            }
            let lines = wrapped_lines(text, disc_toc::DESC_COLUMNS);
            if lines > disc_toc::DESC_LINES {
                return Err(format!(
                    "{name}: the {language} description wraps to {lines} lines of {}, and the \
                     menu has room for {}. The tail would simply not be drawn:\n  {text}",
                    disc_toc::DESC_COLUMNS,
                    disc_toc::DESC_LINES
                ));
            }
        }
        entries[at] = entries[at].described(english, italian);
    }
    Ok(())
}

/// An XA song file from `psx-audio-cook xa-pack`: its ISO name (the file's own
/// name) and its raw 2336-byte sectors.
fn read_xa_file(path: &Path) -> Result<(String, Vec<u8>), String> {
    let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if bytes.is_empty() || bytes.len() % XA_SECTOR_BYTES != 0 {
        return Err(format!(
            "{}: {} bytes is not a whole number of {XA_SECTOR_BYTES}-byte XA sectors; \
             give it a file psx-audio-cook xa-pack wrote",
            path.display(),
            bytes.len()
        ));
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{}: no file name", path.display()))?;
    Ok((name.to_string(), bytes))
}

fn run() -> Result<(), String> {
    let args = parse_args()?;

    let launcher =
        fs::read(&args.launcher).map_err(|e| format!("read {}: {e}", args.launcher.display()))?;
    parse_exe_header(&launcher, &args.launcher)?;

    let system_cnf = default_system_cnf();
    let toc_lba = psx_iso::PLAYTEST_FIRST_FILE_LBA + sectors_for(system_cnf.len());
    if toc_lba != disc_toc::TOC_LBA {
        return Err(format!(
            "table of contents would land at LBA {toc_lba}, but the launcher \
             reads LBA {}. Adjust disc_toc::TOC_LBA.",
            disc_toc::TOC_LBA
        ));
    }

    // Bare EXEs ride inside the launcher's own ISO, so their LBAs follow from
    // the file order. Whole images are appended after the ISO ends, which the
    // ISO's own length decides -- so build the ISO first, then place them.
    // The music: XA song files that ride in the launcher's own ISO as root
    // files, found by name by the program that plays them.
    let mut xa_files: Vec<(String, Vec<u8>)> = Vec::new();
    for path in &args.xa {
        xa_files.push(read_xa_file(path)?);
    }

    // Screenshots, cooked by tools/cook-shots.py. Grouped by program (in
    // first-appearance order, keeping each program's own slideshow order) and
    // padded to sector boundaries, so the launcher addresses shot `i` as
    // `shots_lba + i * SHOT_SECTORS` with nothing but the table in hand.
    let mut shot_groups: Vec<(String, Vec<Vec<u8>>)> = Vec::new();
    for (name, path) in &args.shots {
        let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        if bytes.len() != disc_toc::SHOT_BYTES {
            return Err(format!(
                "--shot {}: {} bytes, but a cooked shot is exactly {} (CLUT then {}x{} \
                 pixels). Cook it with tools/cook-shots.py.",
                path.display(),
                bytes.len(),
                disc_toc::SHOT_BYTES,
                disc_toc::SHOT_W,
                disc_toc::SHOT_H
            ));
        }
        match shot_groups.iter_mut().find(|(n, _)| n == name) {
            Some((_, group)) => group.push(bytes),
            None => shot_groups.push((name.clone(), vec![bytes])),
        }
    }
    let total_shots: usize = shot_groups.iter().map(|(_, g)| g.len()).sum();
    if total_shots > disc_toc::MAX_SHOTS {
        return Err(format!(
            "{total_shots} --shot blobs is more than the {} the launcher's RAM cache holds",
            disc_toc::MAX_SHOTS
        ));
    }
    let mut shots_bin = Vec::new();
    let mut shot_spans: Vec<(String, u8, u8)> = Vec::new();
    for (name, group) in &shot_groups {
        let first = (shots_bin.len() / (disc_toc::SHOT_SECTORS as usize * SECTOR_SIZE)) as u8;
        shot_spans.push((name.clone(), first, group.len() as u8));
        for shot in group {
            shots_bin.extend_from_slice(shot);
            shots_bin.resize(shots_bin.len().next_multiple_of(SECTOR_SIZE), 0);
        }
    }

    let shots_lba = toc_lba + disc_toc::TOC_SECTORS;
    let shots_sectors = sectors_for(shots_bin.len());
    let mut next_lba = shots_lba + shots_sectors + sectors_for(launcher.len());
    let mut entries: Vec<Option<Entry>> = vec![None; args.programs.len()];
    let mut iso_files = Vec::new();
    let mut map = Vec::new();

    for (index, program) in args.programs.iter().enumerate() {
        let Source::Exe(path) = &program.source else {
            continue;
        };
        let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let header = parse_exe_header(&bytes, path)?;
        check_fits_below_loader(&header, path)?;
        let payload_end = 2048usize + header.payload_bytes as usize;
        if bytes.len() < payload_end {
            return Err(format!(
                "{}: header claims {} payload bytes but the file holds {}",
                path.display(),
                header.payload_bytes,
                bytes.len().saturating_sub(2048)
            ));
        }
        let fnv = fnv1a32(&bytes[2048..payload_end]);
        entries[index] = Some(Entry::new(&program.name, next_lba, 0, 0).with_payload_fnv(fnv));
        map.push((
            next_lba,
            bytes.len(),
            program.name.clone(),
            String::from("exe"),
        ));
        next_lba += sectors_for(bytes.len());
        iso_files.push((iso_name(&program.name, index), bytes));
    }

    // Load every image up front: their audio has to be concatenated after all
    // the data, and the track base for each depends on the ones before it.
    let mut images = Vec::new();
    for (index, program) in args.programs.iter().enumerate() {
        let Source::Image(path) = &program.source else {
            continue;
        };
        let image = load_image(path)?;
        let boot_at = IMAGE_BOOT_EXE_LBA as usize * SECTOR_BYTES;
        let boot_sector = image
            .data
            .get(boot_at + 24..boot_at + 24 + SECTOR_SIZE)
            .ok_or_else(|| {
                format!(
                    "{}: data track is too short to hold a boot EXE",
                    path.display()
                )
            })?;
        let header = parse_exe_header(boot_sector, path).map_err(|_| {
            format!(
                "{}: no PSX-EXE at LBA {IMAGE_BOOT_EXE_LBA}. mkdisc places images \
                 verbatim and expects PSoXide's boot layout.",
                path.display()
            )
        })?;
        check_fits_below_loader(&header, path)?;
        // Checksum the payload as the loader will read it: the 2048-byte
        // data windows of the raw sectors after the header sector.
        let mut remaining = header.payload_bytes as usize;
        let mut hash: u32 = 0x811C_9DC5;
        let mut sector = IMAGE_BOOT_EXE_LBA as usize + 1;
        while remaining > 0 {
            let at = sector * SECTOR_BYTES + 24;
            let take = remaining.min(SECTOR_SIZE);
            let data = image.data.get(at..at + take).ok_or_else(|| {
                format!("{}: image ends inside its boot EXE payload", path.display())
            })?;
            for &b in data {
                hash ^= b as u32;
                hash = hash.wrapping_mul(0x0100_0193);
            }
            remaining -= take;
            sector += 1;
        }
        images.push((index, path.clone(), image, hash));
    }

    let build_iso = |toc: Vec<u8>| {
        let mut builder = IsoBuilder::new()
            .volume_id(&args.volume)
            .system_id("PLAYSTATION");
        builder.add_file("SYSTEM.CNF", system_cnf.clone());
        builder.add_file(disc_toc::TOC_FILE_NAME, toc);
        if !shots_bin.is_empty() {
            builder.add_file(SHOTS_FILE_NAME, shots_bin.clone());
        }
        builder.add_file("PSX.EXE", launcher.clone());
        for (name, bytes) in &iso_files {
            builder.add_file(name, bytes.clone());
        }
        // After the bare EXEs, so adding music never moves a program.
        for (name, bytes) in &xa_files {
            builder
                .add_xa_file(name, bytes.clone())
                .expect("sizes were checked when the files were read");
        }
        builder.build_bin()
    };

    // Sizing pass: the ISO's length fixes where the first image lands, and the
    // table has to name that. The table is exactly one sector either way, so
    // the second build comes out the same length.
    let iso_frames = (build_iso(vec![0u8; disc_toc::TOC_BYTES]).len() / SECTOR_BYTES) as u32;

    let mut image_lba = iso_frames;
    for (index, _path, image, payload_fnv) in &images {
        let frames = (image.data.len() / SECTOR_BYTES) as u32;
        let program = &args.programs[*index];
        entries[*index] = Some(
            Entry::new(&program.name, image_lba + IMAGE_BOOT_EXE_LBA, image_lba, 0)
                .with_payload_fnv(*payload_fnv),
        );
        map.push((
            image_lba,
            image.data.len(),
            program.name.clone(),
            String::from("image"),
        ));
        image_lba += frames;
    }

    let mut entries: Vec<Entry> = entries
        .into_iter()
        .map(|e| e.expect("every program is either an exe or an image"))
        .collect();

    let names: Vec<&str> = args.programs.iter().map(|p| p.name.as_str()).collect();
    apply_descriptions(&mut entries, &names, &args.descriptions)?;
    apply_versions(&mut entries, &names, &args.versions)?;
    apply_gates(&mut entries, &names, &args.gates)?;
    apply_shots(&mut entries, &names, &shot_spans)?;
    // No menu track, credit or level meter: the collection's menu is silent
    // and its one song belongs to Magikaaaaarp Pong.
    let toc = disc_toc::encode(
        &entries,
        0,
        0,
        "",
        &[],
        &[],
        0,
        &[],
        if shots_bin.is_empty() { 0 } else { shots_lba },
    )
    .ok_or_else(|| {
        format!(
            "{} programs is more than the {} that fit in the table sector",
            entries.len(),
            disc_toc::MAX_ENTRIES
        )
    })?;

    let mut disc = build_iso(toc.to_vec());
    if (disc.len() / SECTOR_BYTES) as u32 != iso_frames {
        return Err("ISO length changed between the sizing and final builds".to_string());
    }

    let mut lba = iso_frames;
    for (_, _, image, _) in &images {
        place_data_track(&mut disc, &image.data, lba)?;
        lba += (image.data.len() / SECTOR_BYTES) as u32;
    }

    fs::write(&args.out, &disc).map_err(|e| format!("write {}: {e}", args.out.display()))?;
    let cue_path = args.out.with_extension("cue");
    let bin_name = args
        .out
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("output path has no file name")?;
    write_cue(&cue_path, bin_name)?;

    let frames = (disc.len() / SECTOR_BYTES) as u32;
    println!(
        "wrote {} ({frames} sectors, {}, {:.1} MiB)",
        args.out.display(),
        msf(frames),
        disc.len() as f64 / (1024.0 * 1024.0),
    );
    println!("wrote {}", cue_path.display());
    if !shots_bin.is_empty() {
        println!(
            "backdrops: {total_shots} shot(s) over {} program(s) at LBA {shots_lba} ({} KiB)",
            shot_spans.len(),
            shots_bin.len() / 1024
        );
    }
    for (name, bytes) in &xa_files {
        println!("music: {name}, {} sectors", bytes.len() / XA_SECTOR_BYTES);
    }
    println!(
        "{} program(s), one data track, table of contents at LBA {}:",
        entries.len(),
        disc_toc::TOC_LBA
    );
    map.sort_by_key(|(lba, ..)| *lba);
    for (lba, bytes, name, kind) in map {
        println!("  LBA {lba:>7}  {:>7} KiB  {name:<26} {kind}", bytes / 1024);
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mkdisc: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(load_addr: u32, payload: u32) -> ExeHeader {
        ExeHeader {
            load_addr,
            payload_bytes: payload,
        }
    }

    #[test]
    fn accepts_a_program_that_clears_the_blob() {
        let h = header(0x8001_0000, 1_500_000);
        assert!(check_fits_below_loader(&h, Path::new("cortex")).is_ok());
    }

    #[test]
    fn rejects_a_program_that_would_overwrite_the_blob() {
        let h = header(0x8001_0000, LOADER_BASE - 0x8001_0000 + 1);
        let err = check_fits_below_loader(&h, Path::new("huge")).unwrap_err();
        assert!(err.contains("chain-load blob"), "{err}");
    }

    #[test]
    fn parses_a_psx_exe_header() {
        let mut bytes = vec![0u8; SECTOR_SIZE];
        bytes[..8].copy_from_slice(EXE_MAGIC);
        bytes[0x18..0x1C].copy_from_slice(&0x8001_0000u32.to_le_bytes());
        bytes[0x1C..0x20].copy_from_slice(&4096u32.to_le_bytes());
        let h = parse_exe_header(&bytes, Path::new("x")).expect("valid");
        assert_eq!(h.load_addr, 0x8001_0000);
        assert_eq!(h.payload_bytes, 4096);
    }

    #[test]
    fn rejects_a_file_that_is_not_a_psx_exe() {
        let bytes = vec![0u8; SECTOR_SIZE];
        assert!(parse_exe_header(&bytes, Path::new("x")).is_err());
    }

    #[test]
    fn iso_names_stay_within_8_3() {
        assert_eq!(iso_name("Magikaaaaarp Pong", 7), "MAGIKA07.EXE");
        assert_eq!(iso_name("!!!", 3), "GAME03.EXE");
    }

    #[test]
    fn relocated_sectors_carry_their_new_address() {
        // Absolute time is LBA + 150 frames, BCD-encoded.
        let msf_at = |lba: u32| {
            let mut sector = vec![0u8; SECTOR_BYTES];
            rewrite_sector_msf(&mut sector, lba);
            [sector[0x0C], sector[0x0D], sector[0x0E]]
        };
        assert_eq!(msf_at(0), [0x00, 0x02, 0x00]);
        // 4350 + 150 = 4500 frames = exactly one minute.
        assert_eq!(msf_at(4350), [0x01, 0x00, 0x00]);
        // BCD, not hex: 59 seconds is 0x59.
        assert_eq!(msf_at(4349), [0x00, 0x59, 0x74]);
    }

    #[test]
    fn placing_a_track_re_addresses_every_sector_and_keeps_the_payload() {
        let mut data = vec![0u8; 3 * SECTOR_BYTES];
        for (i, sector) in data.chunks_mut(SECTOR_BYTES).enumerate() {
            sector[24] = i as u8 + 1;
        }
        let mut image = Vec::new();
        place_data_track(&mut image, &data, 1000).expect("aligned");
        assert_eq!(image.len(), data.len());
        for (i, sector) in image.chunks(SECTOR_BYTES).enumerate() {
            assert_eq!(sector[24], i as u8 + 1, "payload survived");
            let mut expect = vec![0u8; SECTOR_BYTES];
            rewrite_sector_msf(&mut expect, 1000 + i as u32);
            assert_eq!(
                sector[0x0C..0x0F],
                expect[0x0C..0x0F],
                "sector {i} re-addressed"
            );
        }
    }

    #[test]
    fn rejects_a_track_that_is_not_whole_sectors() {
        let mut image = Vec::new();
        assert!(place_data_track(&mut image, &vec![0u8; 100], 0).is_err());
    }

    #[test]
    fn counting_wrapped_lines_matches_a_greedy_wrap() {
        assert_eq!(wrapped_lines("", 10), 0);
        assert_eq!(wrapped_lines("short", 10), 1);
        assert_eq!(wrapped_lines("exactly ten", 11), 1);
        assert_eq!(wrapped_lines("one two three four", 10), 2);
        // A word longer than the line still has to go somewhere.
        assert_eq!(wrapped_lines("supercalifragilistic", 10), 2);
    }

    #[test]
    fn descriptions_land_on_the_named_program() {
        let mut entries = [Entry::new("PONG", 0, 0, 0), Entry::new("VOXIDE", 0, 0, 0)];
        apply_descriptions(
            &mut entries,
            &["PONG", "VOXIDE"],
            &[(
                "VOXIDE".into(),
                "Voxel sandbox".into(),
                "Sandbox a voxel".into(),
            )],
        )
        .expect("the name exists");
        assert_eq!(entries[1].desc_en_str(), "Voxel sandbox");
        assert_eq!(entries[1].desc_it_str(), "Sandbox a voxel");
        assert_eq!(entries[0].desc_en_str(), "", "others untouched");
    }

    #[test]
    fn an_overlong_description_is_an_error_rather_than_a_silent_trim() {
        let mut entries = [Entry::new("PONG", 0, 0, 0)];
        let long = "x".repeat(disc_toc::DESC_BYTES + 1);
        let err = apply_descriptions(
            &mut entries,
            &["PONG"],
            &[("PONG".into(), long, "ok".into())],
        )
        .unwrap_err();
        assert!(err.contains("English"), "{err}");
        assert!(err.contains("1 over"), "{err}");
    }

    #[test]
    fn describing_a_program_that_is_not_on_the_disc_is_an_error() {
        let mut entries = [Entry::new("PONG", 0, 0, 0)];
        let err = apply_descriptions(
            &mut entries,
            &["PONG"],
            &[("VOXIDE".into(), "a".into(), "b".into())],
        )
        .unwrap_err();
        assert!(err.contains("VOXIDE"), "{err}");
    }

    #[test]
    fn the_cue_lists_one_data_track_and_no_audio() {
        let dir = std::env::temp_dir().join("mkdisc-cue-out");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demo.cue");
        write_cue(&path, "demo.bin").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("FILE \"demo.bin\" BINARY"));
        assert!(text.contains("TRACK 01 MODE2/2352"));
        assert!(
            !text.contains("AUDIO"),
            "the music is XA inside the data track"
        );
    }

    #[test]
    fn an_xa_file_must_be_whole_raw_sectors() {
        let dir = std::env::temp_dir().join("mkdisc-xa-out");
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("MUSIC1.XA");
        std::fs::write(&good, vec![0u8; 3 * XA_SECTOR_BYTES]).unwrap();
        let (name, bytes) = read_xa_file(&good).unwrap();
        assert_eq!(name, "MUSIC1.XA");
        assert_eq!(bytes.len(), 3 * XA_SECTOR_BYTES);
        let bad = dir.join("BAD.XA");
        std::fs::write(&bad, vec![0u8; XA_SECTOR_BYTES + 1]).unwrap();
        assert!(read_xa_file(&bad).unwrap_err().contains("xa-pack"));
        let empty = dir.join("EMPTY.XA");
        std::fs::write(&empty, []).unwrap();
        assert!(read_xa_file(&empty).is_err());
    }

    #[test]
    fn an_xa_file_lands_in_the_iso_under_its_name_with_form_two_sectors() {
        let mut builder = IsoBuilder::new().volume_id("TEST").system_id("PLAYSTATION");
        builder.add_file("PSX.EXE", vec![1u8; 100]);
        let mut raw = vec![0u8; 2 * XA_SECTOR_BYTES];
        // Subheader: file 1, channel 0, submode with the Form 2 bit.
        for sector in raw.chunks_mut(XA_SECTOR_BYTES) {
            sector[..8].copy_from_slice(&[1, 0, 0x64, 0x01, 1, 0, 0x64, 0x01]);
        }
        builder
            .add_xa_file("music1.xa", raw)
            .expect("whole sectors");
        let disc = builder.build_bin();
        assert_eq!(disc.len() % SECTOR_BYTES, 0);
        let text = String::from_utf8_lossy(&disc);
        assert!(
            text.contains("MUSIC1.XA"),
            "directory record carries the upper-case name"
        );
    }
}
