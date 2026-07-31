#!/usr/bin/env bash
# Build the WebGPU demo: compile ssplayer-wgpu's `web` feature to wasm (wasm-bindgen) and
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
repo="$(cd "$here/../.." && pwd)"

profile="--release"
[ "${1:-}" = "--dev" ] && profile="--dev"

echo "==> building wasm package (ssplayer-wgpu, feature=web) -> examples/web/pkg"
wasm-pack build "$repo/libs/ssplayer-wgpu" \
  --target web $profile \
  --out-name ssplayer_wgpu \
  --out-dir "$here/pkg" \
  -- --no-default-features --features web

# Embedded ssab bakes the atlas in, so the demo needs no sibling texture files.
if [ ! -d "$repo/assets/overall_emb" ]; then
  echo "==> assets/overall_emb missing; running scripts/deploy-examples.sh"
  "$repo/scripts/deploy-examples.sh"
fi

echo "==> staging embedded .ssab demo assets -> examples/web/assets"
rm -rf "$here/assets"; mkdir -p "$here/assets"
cp "$repo/assets/overall_emb/"*.ssab "$here/assets/"

# The other export mode, so the page can exercise it: reference-mode packs name
# sibling PNGs, which `loadSsab` resolves against the `.ssab`'s own URL and
# fetches (SPEC §8.1). Nothing else on the page covers that branch.
echo "==> staging reference .ssab + atlases -> examples/web/assets-ref"
rm -rf "$here/assets-ref"; mkdir -p "$here/assets-ref"
cp "$repo/assets/overall/"*.ssab "$here/assets-ref/"
cp "$repo/assets/overall/"*.png "$here/assets-ref/"

echo "==> done. serve this directory over http (e.g. python3 -m http.server) and open index.html"
