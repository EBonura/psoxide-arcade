//! Installing a built disc into the PSoXide games library.
//!
//! A library disc is named `Game Name (repo)`: `<name>.bin` and `<name>.cue`, the cue quoting the
//! bin. An install replaces both in place (written under a temporary name, checked against the
//! source digest, then renamed over the live name) and never leaves a copy under the old flat
//! name (`<legacy_stem>.bin` and friends).

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Install the disc built at `cue` (and the `.bin` beside it) into `games_dir` as `name`.
///
/// A stale pair under the old flat name `legacy_stem` is removed after the install only when its
/// bin is byte-identical to, or older than, the bin being installed. Otherwise the install is
/// refused before anything is written. Returns the installed cue.
pub fn install(cue: &Path, games_dir: &Path, name: &str, legacy_stem: &str) -> Result<PathBuf> {
    let image = cue.with_extension("bin");
    let image_digest = sha256_file(&image)?;
    let cue_text = retarget_cue(&fs::read_to_string(cue)?, name)
        .map_err(|error| format!("{}: {error}", cue.display()))?;
    fs::create_dir_all(games_dir)?;
    let stale = stale_legacy_files(games_dir, legacy_stem, name, &image, &image_digest)?;

    let live_bin = games_dir.join(format!("{name}.bin"));
    let live_cue = games_dir.join(format!("{name}.cue"));
    publish(&live_bin, &image_digest, |temporary| {
        fs::copy(&image, temporary).map(drop)
    })?;
    publish(&live_cue, &sha256_bytes(cue_text.as_bytes()), |temporary| {
        fs::write(temporary, &cue_text)
    })?;
    for path in stale {
        fs::remove_file(&path)
            .map_err(|error| format!("remove stale {}: {error}", path.display()))?;
        println!("REMOVED stale {}", path.display());
    }
    Ok(live_cue)
}

/// The cue text with its `FILE` line pointing at `<name>.bin`, quoted (the name has spaces).
fn retarget_cue(text: &str, name: &str) -> Result<String> {
    let mut replaced = false;
    let mut output = String::new();
    for line in text.lines() {
        if !replaced && line.trim_start().starts_with("FILE ") {
            output.push_str(&format!("FILE \"{name}.bin\" BINARY\n"));
            replaced = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    if replaced {
        Ok(output)
    } else {
        Err("no FILE directive".into())
    }
}

/// Files under the old flat name that this install replaces, or an error if one of them might
/// hold something the install would lose.
fn stale_legacy_files(
    games_dir: &Path,
    legacy_stem: &str,
    name: &str,
    image: &Path,
    image_digest: &str,
) -> Result<Vec<PathBuf>> {
    let legacy_bin = games_dir.join(format!("{legacy_stem}.bin"));
    let legacy_cue = games_dir.join(format!("{legacy_stem}.cue"));
    let legacy_provenance = games_dir.join(format!("{legacy_stem}.provenance.json"));
    let mut stale = Vec::new();
    if legacy_bin.is_file() {
        let same = sha256_file(&legacy_bin)? == image_digest;
        if !same && !is_older(&legacy_bin, image)? {
            return Err(format!(
                "{} is under the old name, differs from the disc being installed and is not \
                 older than it; compare the two and remove or rename one by hand",
                legacy_bin.display()
            )
            .into());
        }
        stale.push(legacy_bin);
    }
    if legacy_cue.is_file() {
        stale.push(legacy_cue);
    }
    if legacy_provenance.is_file() {
        let renamed = games_dir.join(format!("{name}.provenance.json"));
        if !renamed.is_file() {
            return Err(format!(
                "{} is under the old name and {} does not exist; rename it first so its record \
                 survives",
                legacy_provenance.display(),
                renamed.display()
            )
            .into());
        }
        stale.push(legacy_provenance);
    }
    Ok(stale)
}

fn is_older(path: &Path, than: &Path) -> io::Result<bool> {
    let modified = |path: &Path| -> io::Result<SystemTime> { fs::metadata(path)?.modified() };
    Ok(modified(path)? < modified(than)?)
}

/// Write `live` by filling a temporary file beside it, checking that it hashes to `expected`, and
/// renaming it over `live`. A failed install leaves the previous `live` untouched.
fn publish(live: &Path, expected: &str, fill: impl FnOnce(&Path) -> io::Result<()>) -> Result<()> {
    let file_name = live.file_name().ok_or("library path has no file name")?;
    let temporary = live.with_file_name(format!(".{}.installing", file_name.to_string_lossy()));
    let outcome = fill(&temporary)
        .map_err(|error| format!("write {}: {error}", temporary.display()).into())
        .and_then(|()| {
            let actual = sha256_file(&temporary)?;
            if actual == expected {
                Ok(())
            } else {
                Err(format!(
                    "{} hashes to {actual}, expected {expected}; install aborted",
                    temporary.display()
                )
                .into())
            }
        })
        .and_then(|()| {
            fs::rename(&temporary, live)
                .map_err(|error| format!("rename onto {}: {error}", live.display()).into())
        });
    if outcome.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    outcome
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex(&hash.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const NAME: &str = "Game Name (repo)";
    const LEGACY: &str = "repo";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("library-install-{label}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A built disc in `dist/` and an empty `library/`.
    fn built(scratch: &Scratch, bin: &[u8]) -> (PathBuf, PathBuf) {
        let dist = scratch.0.join("dist");
        fs::create_dir_all(&dist).unwrap();
        fs::write(dist.join("repo.bin"), bin).unwrap();
        fs::write(
            dist.join("repo.cue"),
            "FILE repo.bin BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
        )
        .unwrap();
        (dist.join("repo.cue"), scratch.0.join("library"))
    }

    fn age(path: &Path, seconds: u64) {
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(seconds))
            .unwrap();
    }

    fn names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn installs_under_the_library_name_with_a_quoted_cue() {
        let scratch = Scratch::new("fresh");
        let (cue, library) = built(&scratch, b"disc");
        install(&cue, &library, NAME, LEGACY).unwrap();
        assert_eq!(
            names(&library),
            ["Game Name (repo).bin", "Game Name (repo).cue"]
        );
        let text = fs::read_to_string(library.join("Game Name (repo).cue")).unwrap();
        assert_eq!(
            text,
            "FILE \"Game Name (repo).bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n"
        );
    }

    #[test]
    fn replaces_the_live_disc_in_place_and_leaves_no_temporary() {
        let scratch = Scratch::new("replace");
        let (cue, library) = built(&scratch, b"first");
        install(&cue, &library, NAME, LEGACY).unwrap();
        fs::write(cue.with_extension("bin"), b"second").unwrap();
        install(&cue, &library, NAME, LEGACY).unwrap();
        assert_eq!(
            fs::read(library.join("Game Name (repo).bin")).unwrap(),
            b"second"
        );
        assert_eq!(names(&library).len(), 2);
    }

    #[test]
    fn removes_an_identical_legacy_pair() {
        let scratch = Scratch::new("identical");
        let (cue, library) = built(&scratch, b"disc");
        fs::create_dir_all(&library).unwrap();
        fs::write(library.join("repo.bin"), b"disc").unwrap();
        fs::write(library.join("repo.cue"), "FILE repo.bin BINARY\n").unwrap();
        install(&cue, &library, NAME, LEGACY).unwrap();
        assert_eq!(
            names(&library),
            ["Game Name (repo).bin", "Game Name (repo).cue"]
        );
    }

    #[test]
    fn removes_an_older_legacy_pair() {
        let scratch = Scratch::new("older");
        let (cue, library) = built(&scratch, b"new disc");
        fs::create_dir_all(&library).unwrap();
        fs::write(library.join("repo.bin"), b"old disc").unwrap();
        fs::write(library.join("repo.cue"), "FILE repo.bin BINARY\n").unwrap();
        age(&library.join("repo.bin"), 3600);
        install(&cue, &library, NAME, LEGACY).unwrap();
        assert_eq!(
            names(&library),
            ["Game Name (repo).bin", "Game Name (repo).cue"]
        );
    }

    #[test]
    fn refuses_a_newer_different_legacy_disc_and_writes_nothing() {
        let scratch = Scratch::new("newer");
        let (cue, library) = built(&scratch, b"built disc");
        age(&cue.with_extension("bin"), 3600);
        fs::create_dir_all(&library).unwrap();
        fs::write(library.join("repo.bin"), b"someone else's newer disc").unwrap();
        let error = install(&cue, &library, NAME, LEGACY).err().unwrap();
        assert!(error.to_string().contains("not older"), "{error}");
        assert_eq!(names(&library), ["repo.bin"]);
    }

    #[test]
    fn refuses_a_legacy_provenance_with_no_renamed_record() {
        let scratch = Scratch::new("provenance");
        let (cue, library) = built(&scratch, b"disc");
        fs::create_dir_all(&library).unwrap();
        fs::write(library.join("repo.provenance.json"), b"{}").unwrap();
        let error = install(&cue, &library, NAME, LEGACY).err().unwrap();
        assert!(error.to_string().contains("rename it first"), "{error}");
        assert_eq!(names(&library), ["repo.provenance.json"]);
    }

    #[test]
    fn a_cue_without_a_file_line_is_rejected() {
        assert!(retarget_cue("TRACK 01 MODE2/2352\n", NAME).is_err());
    }
}
