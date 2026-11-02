#!/usr/bin/env bash
# Builds the browser client into web/dist (wasm + JS glue + index.html).
# Same steps as web/build.ps1; this is what CI runs.
#
#   1. cargo build --profile wasm-release --target wasm32-unknown-unknown
#   2. wasm-bindgen --target web   (CLI version must equal the wasm-bindgen crate in Cargo.lock)
#   3. wasm-opt -O3                (only with WASM_OPT=1; see web/README.md)
#   4. copy web/index.html (and web/CNAME if present)
#
# One-time setup:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <version from Cargo.lock> --locked
#   (optional) binaryen, for WASM_OPT=1
#
# Set TRONZERO_MANAGER_URL to bake the lobby URL into the build.
# CARGO_PROFILE=<name> picks another profile for quick iteration (default wasm-release).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dist="$root/web/dist"
name=tron-zero-client
profile="${CARGO_PROFILE:-wasm-release}"
cd "$root"

lock_version="$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p' | head -n1 | tr -d '\r')"
cli_version="$(wasm-bindgen --version | awk '{print $2}' | tr -d '\r')"
if [ "$cli_version" != "$lock_version" ]; then
  echo "wasm-bindgen CLI is $cli_version but Cargo.lock has $lock_version." >&2
  echo "Run: cargo install wasm-bindgen-cli --version $lock_version --locked" >&2
  exit 1
fi

cargo build --profile "$profile" --target wasm32-unknown-unknown -p tron-zero-client --locked

rm -rf "$dist"
mkdir -p "$dist"
# Names and producers sections are debugging aids only: 11 MB of the raw wasm.
wasm-bindgen --target web --no-typescript --remove-name-section --remove-producers-section \
  --out-dir "$dist" --out-name "$name" \
  "target/wasm32-unknown-unknown/$profile/$name.wasm"

wasm="$dist/${name}_bg.wasm"
if [ "${WASM_OPT:-0}" = 1 ]; then
  # Explicit features: --all-features also enables ones browsers reject
  # (e.g. compact imports). These are what rustc emits by default.
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals --enable-multivalue --enable-reference-types -o "$wasm" "$wasm"
fi

cp web/index.html "$dist/"
if [ -f web/CNAME ]; then cp web/CNAME "$dist/"; fi
printf '%s: %s bytes\n' "$(basename "$wasm")" "$(wc -c < "$wasm")"
