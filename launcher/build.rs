fn main() {
    // Neither is a file cargo can track: without these a version or table change leaves a stale
    // launcher behind.
    println!("cargo:rerun-if-env-changed=DISC_VERSION");
    println!("cargo:rerun-if-env-changed=ARCADE_TOC");
    if let Ok(path) = std::env::var("ARCADE_TOC") {
        println!("cargo:rerun-if-changed={path}");
    }
}
