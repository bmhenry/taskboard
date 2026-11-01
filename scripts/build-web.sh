#!/usr/bin/env bash
# Builds the web UI (crates/taskboard-wasm) with trunk, then the
# taskboard-web daemon in release mode with the UI built into it.
#
# trunk already runs wasm-opt on release builds (it downloads it if need
# be). This adds gzip copies of the bundle's files, and brotli ones if
# `brotli` is installed, which the daemon sends to browsers that take them.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
wasm="$root/crates/taskboard-wasm"
dist="$wasm/dist"

die() {
    echo "build-web.sh: $*" >&2
    exit 1
}

command -v trunk >/dev/null ||
    die "trunk isn't installed; install it with: cargo install trunk --locked"
command -v cargo >/dev/null || die "cargo isn't installed"
sysroot="$(rustc --print sysroot)"
[[ -d "$sysroot/lib/rustlib/wasm32-unknown-unknown" ]] ||
    die "the wasm32-unknown-unknown target isn't installed; install it with: rustup target add wasm32-unknown-unknown"

echo "==> Building the web UI with trunk"
(cd "$wasm" && trunk build --release)
[[ -f "$dist/index.html" ]] || die "trunk didn't make $dist/index.html"

echo "==> Compressing it"
brotli=$(command -v brotli || true)
[[ -n "$brotli" ]] || echo "    (brotli isn't installed, so only gzip copies are made)"
shopt -s nullglob
for file in "$dist"/*.{html,js,wasm,css}; do
    gzip -9 -n -k -f "$file"
    [[ -z "$brotli" ]] || "$brotli" -q 11 -k -f "$file"
done

echo "==> Building taskboard-web with it"
cargo build --manifest-path "$root/Cargo.toml" -p taskboard-web --release --features embed

target=$(cargo metadata --manifest-path "$root/Cargo.toml" --format-version 1 --no-deps |
    sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
bin="$target/release/taskboard-web"
[[ -x "$bin" ]] || die "expected the daemon at $bin, but it isn't there"

echo
echo "Web UI bundle ($dist):"
for file in "$dist"/*; do
    printf '  %10s  %s\n' "$(stat -c %s "$file")" "${file##*/}"
done
echo "Daemon: $bin ($(stat -c %s "$bin") bytes)"
echo "Install it with: install -Dm755 '$bin' ~/.cargo/bin/taskboard-web"
