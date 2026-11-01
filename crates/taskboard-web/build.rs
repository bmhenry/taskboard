//! With the `embed` feature, checks that trunk's bundle is there to embed,
//! and has the daemon rebuilt when it changes.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_EMBED").is_none() {
        return;
    }
    let dist = Path::new("../taskboard-wasm/dist");
    // A directory is scanned whole, so new files count as a change.
    println!("cargo:rerun-if-changed={}", dist.display());
    if !dist.join("index.html").is_file() {
        panic!(
            "the `embed` feature needs the web UI built first, in crates/taskboard-wasm/dist; \
             run scripts/build-web.sh, which builds it and then the daemon"
        );
    }
}
