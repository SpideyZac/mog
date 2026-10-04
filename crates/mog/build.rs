//! Records the target triple so the updater knows which release archive to download.

use std::env;

/// Passes the target triple to the crate as `MOG_TARGET`.
fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=MOG_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
