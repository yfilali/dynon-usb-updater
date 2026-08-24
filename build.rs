//! Templates are pulled in by `#[template(file = ...)]` at macro-expansion
//! time, which Cargo does not track. Without this, editing a .ui file leaves
//! the old markup compiled into the binary and the change silently does
//! nothing.
fn main() {
    println!("cargo:rerun-if-changed=src/ui");
    if let Ok(entries) = std::fs::read_dir("src/ui") {
        for entry in entries.flatten() {
            println!("cargo:rerun-if-changed={}", entry.path().display());
        }
    }
}
