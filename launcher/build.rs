fn main() {
    // Neither is a file cargo can track: without these a version or blob
    // change leaves a stale launcher behind.
    println!("cargo:rerun-if-env-changed=DISC_VERSION");
    println!("cargo:rerun-if-env-changed=LOADER_BLOB");
}
