#!/usr/bin/env bash
# Build the WebGPU demo: compile core's `web` feature to wasm (wasm-bindgen) and
# generate the embedded `.ssab` demo assets. Requires wasm-pack + the wasm32
# target (`rustup target add wasm32-unknown-unknown`).
#
#   ./build.sh          # release wasm (smaller; runs wasm-opt)
#   ./build.sh --dev    # dev wasm (fast; skips wasm-opt)
#
# Then serve this directory over http and open it in a WebGPU browser:
#   python3 -m http.server   # → http://localhost:8000
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"

profile="--release"
[ "${1:-}" = "--dev" ] && profile="--dev"

echo "==> building wasm package (core, feature=web) -> examples/web/pkg"
wasm-pack build "$repo/ssplayer-wgpu/core" \
  --target web $profile \
  --out-name ssplayer_wgpu \
  --out-dir "$here/pkg" \
  -- --no-default-features --features web

echo "==> generating embedded .ssab demo assets -> examples/web/assets"
mkdir -p "$here/assets"
( cd "$repo" && cargo run -p ssconverter-cli -- --embedded --out-dir "$here/assets" tests/overall/overall.sspj )

echo "==> done. serve this directory over http (e.g. python3 -m http.server) and open index.html"
