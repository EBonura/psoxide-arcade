//! `psx-audio-cook` built from the pinned SDK (see Cargo.toml).

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    psx_audio_cook::cli::run(&args)
}
