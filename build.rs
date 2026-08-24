//! Compiles `data/resources.gresource.xml` into a `.gresource` blob that
//! `main.rs` embeds and registers at startup. This keeps `cargo build`/`cargo
//! test` working standalone (no meson required) while meson, when it wraps
//! cargo for packaging, gets the exact same bytes.
//!
//! `glib-compile-resources` is invoked by absolute path because on this
//! machine an Anaconda install shadows the system one on `PATH`.

use std::path::Path;
use std::process::Command;

const COMPILER_CANDIDATES: &[&str] = &["/usr/bin/glib-compile-resources", "glib-compile-resources"];

fn main() {
    let data_dir = Path::new("data");
    let xml = data_dir.join("resources.gresource.xml");
    println!("cargo:rerun-if-changed={}", xml.display());
    println!(
        "cargo:rerun-if-changed={}",
        data_dir.join("icons/hicolor/scalable/apps/io.github.yfilali.DynonUSBUpdater.svg").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        data_dir.join("icons/hicolor/symbolic/apps/io.github.yfilali.DynonUSBUpdater-symbolic.svg").display()
    );

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let out_file = Path::new(&out_dir).join("resources.gresource");

    let compiler = COMPILER_CANDIDATES
        .iter()
        .find(|c| Command::new(c).arg("--version").output().is_ok())
        .unwrap_or_else(|| panic!("glib-compile-resources not found; tried {COMPILER_CANDIDATES:?}"));

    let status = Command::new(compiler)
        .arg(format!("--sourcedir={}", data_dir.display()))
        .arg(format!("--target={}", out_file.display()))
        .arg(&xml)
        .status()
        .expect("failed to run glib-compile-resources");
    assert!(status.success(), "glib-compile-resources failed");
}
