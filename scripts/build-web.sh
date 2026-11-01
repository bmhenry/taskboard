#!/usr/bin/env bash
# Builds the web UI (crates/taskboard-wasm) with trunk, then the
# taskboard-web daemon in release mode with the UI built into it.
#
# trunk already runs wasm-opt on release builds (it downloads it if need
# be). This adds gzip copies of the bundle's files, and brotli ones if
# `brotli` is installed, which the daemon sends to browsers that take them.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# From the repository, so cargo reads its .cargo/config.toml, if any.
cd "$root"
dist="crates/taskboard-wasm/dist"

die() {
    echo "build-web.sh: $*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null || die "$1 isn't installed; $2"
}
need cargo "install Rust from https://rustup.rs"
need rustc "install Rust from https://rustup.rs"
need trunk "install it with: cargo install trunk --locked"
need jq "install it with your package manager (it reads cargo's build output)"
sysroot="$(rustc --print sysroot)"
[[ -d "$sysroot/lib/rustlib/wasm32-unknown-unknown" ]] ||
    die "the wasm32-unknown-unknown target isn't installed; install it with: rustup target add wasm32-unknown-unknown"

echo "==> Building the web UI with trunk"
# From scratch, so no compressed copy of an old build is left behind.
rm -rf "$dist"
(cd crates/taskboard-wasm && trunk build --release)
[[ -f "$dist/index.html" ]] || die "trunk didn't make $dist/index.html"

echo "==> Compressing it"
brotli=$(command -v brotli || true)
[[ -n "$brotli" ]] || echo "    (brotli isn't installed, so only gzip copies are made)"
find "$dist" -type f \( -name '*.gz' -o -name '*.br' \) -delete
while IFS= read -r -d '' file; do
    gzip -9 -n -k -f "$file"
    [[ -z "$brotli" ]] || "$brotli" -q 11 -k -f "$file"
done < <(find "$dist" -type f \( -name '*.html' -o -name '*.js' -o -name '*.wasm' \
    -o -name '*.css' -o -name '*.svg' -o -name '*.json' \) -print0)

echo "==> Building taskboard-web with it"
bin=$(cargo build -p taskboard-web --release --features embed --message-format=json-render-diagnostics |
    jq -r 'select(.reason == "compiler-artifact" and .target.name == "taskboard-web"
        and (.target.kind | index("bin"))) | .executable')
[[ -n "$bin" && -x "$bin" ]] || die "cargo didn't say where it put taskboard-web"

echo
echo "Web UI bundle ($root/$dist):"
find "$dist" -type f -printf '  %10s  %P\n' | sort -k2
echo "Daemon: $bin ($(stat -c %s "$bin") bytes)"
echo "Install it with: install -Dm755 '$bin' ~/.cargo/bin/taskboard-web"
