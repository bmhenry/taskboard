//! With the `embed` feature, checks that trunk's bundle is there to embed,
//! with no stale compressed copies in it, and has the daemon rebuilt when it
//! changes.

use std::path::{Path, PathBuf};

const REBUILD: &str = "run scripts/build-web.sh, which builds it and then the daemon";

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
             {REBUILD}"
        );
    }
    // A copy older than its file, or of a file that's gone, would be sent
    // in its place to browsers that take its encoding.
    for copy in files(dist) {
        let name = copy.to_string_lossy();
        let Some(original) = name
            .strip_suffix(".gz")
            .or_else(|| name.strip_suffix(".br"))
        else {
            continue;
        };
        let modified = |p: &Path| p.metadata().and_then(|m| m.modified()).ok();
        let stale = match modified(Path::new(original)) {
            None => true,
            Some(original) => modified(&copy).is_none_or(|copy| copy < original),
        };
        if stale {
            panic!("{name} is out of date with {original}; {REBUILD}");
        }
    }
}

/// Every file under `dir`.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.push(path);
        }
    }
    found
}
