//! Hydrate the exact PSoXide runtime used by this collection.

use std::path::PathBuf;
use std::process::ExitCode;

const REV: &str = "588d7637a4cb49209d3072438110683b921bb634";

fn main() -> ExitCode {
    let into = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../.psoxide"));
    match psoxide_link::hydrate_pinned(&into, REV, true) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("psoxide-arcade-pin: {error}");
            ExitCode::FAILURE
        }
    }
}

