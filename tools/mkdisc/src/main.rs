//! The Arcade's disc helper: the launcher's table of contents, and the library install.
//!
//! The disc itself is a scene disc pressed by the SDK's `mkisopsx --scenes` (`scenes.manifest`):
//! `SCENES.TBL` at LBA 22, the launcher (`PSX.EXE`, scene 0) at 23, then the three games, each a
//! Program row of the same table that the launcher starts with a core switch, and `MUSIC1.XA`.
//!
//! ```text
//! mkdisc toc --out <DEMOTOC.BIN> --game NAME [--game NAME]... [--describe NAME=EN|IT]
//!            [--version-of NAME=VERSION] [--gate NAME]
//! mkdisc install --cue <cue> --games-dir <dir>
//! ```
//!
//! `toc` writes the table the launcher compiles in (`psx-disc-toc`, one sector): one entry per
//! `--game`, in order, whose `exe_lba` field holds the scene that starts it (game `i` is scene
//! `i + 1`, as `scenes.manifest` lists them). The launcher reads no table of contents from the disc.
//!
//! `install` copies a finished disc into a games library as `PSoXide Arcade (psoxide-arcade)`,
//! replacing the previous install in place and writing its provenance record beside it (see the
//! `psx-library` crate).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use disc_toc::Entry;

/// What `toc` builds the table from.
struct TocArgs {
    out: PathBuf,
    /// Display names, in scene order (the first is scene 1).
    games: Vec<String>,
    /// `(name, english, italian)` blurbs shown under the carousel.
    descriptions: Vec<(String, String, String)>,
    /// `NAME=VERSION`, the version each program's own source declares.
    versions: Vec<(String, String)>,
    /// Display names held off the carousel until the cheat code reveals them.
    gates: Vec<String>,
}

fn parse_toc_args(mut it: impl Iterator<Item = String>) -> Result<TocArgs, String> {
    let mut out = None;
    let mut games = Vec::new();
    let mut descriptions = Vec::new();
    let mut versions = Vec::new();
    let mut gates = Vec::new();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out takes a path")?)),
            "--game" => games.push(it.next().ok_or("--game takes a display NAME")?),
            "--version-of" => {
                let spec = it.next().ok_or("--version-of takes NAME=VERSION")?;
                let (name, version) = spec
                    .split_once('=')
                    .ok_or_else(|| format!("--version-of wants NAME=VERSION, got {spec:?}"))?;
                versions.push((name.to_string(), version.trim().to_string()));
            }
            "--gate" => gates.push(it.next().ok_or("--gate takes a display NAME")?),
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
            other => return Err(format!("toc: unknown argument {other:?}")),
        }
    }
    Ok(TocArgs {
        out: out.ok_or("toc needs --out <file>")?,
        games,
        descriptions,
        versions,
        gates,
    })
}

/// The table of contents for `args`: entry `i` starts scene `i + 1`.
fn build_toc(args: &TocArgs) -> Result<[u8; disc_toc::TOC_BYTES], String> {
    if args.games.is_empty() {
        return Err("toc needs at least one --game".to_string());
    }
    let mut entries: Vec<Entry> = args
        .games
        .iter()
        .enumerate()
        .map(|(index, name)| Entry::new(name, index as u32 + 1, 0, 0))
        .collect();
    let names: Vec<&str> = args.games.iter().map(String::as_str).collect();
    apply_descriptions(&mut entries, &names, &args.descriptions)?;
    apply_versions(&mut entries, &names, &args.versions)?;
    apply_gates(&mut entries, &names, &args.gates)?;
    // No menu track, credit, level meter or backdrops: the collection's menu is silent and its
    // one song belongs to Magikaaaaarp Pong.
    disc_toc::encode(&entries, 0, 0, "", &[], &[], 0, &[], 0).ok_or_else(|| {
        format!(
            "{} programs is more than the {} that fit in the table sector",
            entries.len(),
            disc_toc::MAX_ENTRIES
        )
    })
}

fn toc(it: impl Iterator<Item = String>) -> Result<(), String> {
    let args = parse_toc_args(it)?;
    let bytes = build_toc(&args)?;
    fs::write(&args.out, bytes).map_err(|e| format!("write {}: {e}", args.out.display()))?;
    println!("wrote {} ({} programs)", args.out.display(), args.games.len());
    Ok(())
}

fn print_usage() {
    println!(
        "mkdisc toc --out <DEMOTOC.BIN> --game NAME... [--describe NAME=EN|IT] \\\n\
        \x20          [--version-of NAME=VERSION] [--gate NAME]\n\
         mkdisc install --cue <cue> --games-dir <dir>   (copy a finished disc into the library)"
    );
}

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

/// Library name of the standalone disc: `Game Name (repo)`.
const LIBRARY_NAME: &str = "PSoXide Arcade (psoxide-arcade)";
/// The GitHub repository recorded in the provenance.
const LIBRARY_REPOSITORY: &str = "EBonura/psoxide-arcade";
/// The library name before 2026-10-23 (flat `psoxide-arcade.bin`); an install removes a stale pair.
const LEGACY_LIBRARY_STEM: &str = "psoxide-arcade";

/// `mkdisc install --cue <cue> --games-dir <dir>`: copy a finished disc into the games library.
fn install(mut it: impl Iterator<Item = String>) -> Result<(), String> {
    let mut cue = None;
    let mut games_dir = std::env::var_os("GAMES_DIR").map(PathBuf::from);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--cue" => cue = Some(PathBuf::from(it.next().ok_or("--cue takes a path")?)),
            "--games-dir" => {
                games_dir = Some(PathBuf::from(it.next().ok_or("--games-dir takes a path")?));
            }
            other => return Err(format!("install: unknown argument {other:?}")),
        }
    }
    let cue = cue.ok_or("install needs --cue <the disc's cue>")?;
    let games_dir = games_dir.ok_or("install needs --games-dir <dir> (or GAMES_DIR)")?;
    // The source recorded in the provenance is the checkout `mkdisc` runs in (the repository
    // root for `make install`).
    let source = psx_library::Source::from_git(Path::new("."), LIBRARY_REPOSITORY)
        .map_err(|error| error.to_string())?;
    let installed = psx_library::install(
        &psx_library::Request::new(&cue, &games_dir, LIBRARY_NAME, source)
            .with_legacy_stem(LEGACY_LIBRARY_STEM),
    )
    .map_err(|error| error.to_string())?;
    for path in &installed.removed {
        println!("REMOVED stale {}", path.display());
    }
    println!("INSTALLED -> {}", installed.cue.display());
    Ok(())
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("toc") => toc(arguments),
        Some("install") => install(arguments),
        Some("--help" | "-h") => {
            print_usage();
            Ok(())
        }
        _ => {
            print_usage();
            Err("mkdisc takes `toc` or `install`".to_string())
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mkdisc: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn each_game_names_the_scene_that_starts_it() {
        let args = TocArgs {
            out: PathBuf::new(),
            games: vec!["BREAKOUT".into(), "PONG".into()],
            descriptions: Vec::new(),
            versions: vec![("PONG".into(), "0.1.0".into())],
            gates: Vec::new(),
        };
        let bytes = build_toc(&args).expect("two programs fit");
        let mut entries = [Entry::new("", 0, 0, 0); disc_toc::MAX_ENTRIES];
        let header = disc_toc::decode(&bytes, &mut entries).expect("decodes");
        assert_eq!(header.count, 2);
        assert_eq!(entries[0].exe_lba, 1);
        assert_eq!(entries[1].exe_lba, 2);
        assert_eq!(entries[1].version_str(), "0.1.0");
    }
}
