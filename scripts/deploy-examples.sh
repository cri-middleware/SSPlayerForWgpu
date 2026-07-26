#!/usr/bin/env bash
#
# Convert the SpriteStudio test projects in the SDK into playback data under
# assets/ (.ssab + referenced textures), using ssconverter-cli built from the
# SDK submodule with cargo.
#
#   assets/overall/        <- tests/overall, reference textures (sibling .png)
#   assets/overall_emb/    <- tests/overall, textures embedded in the .ssab
#   assets/Ringo/          <- tests/Ringo, reference textures
#   assets/Ringo_emb/      <- tests/Ringo, textures embedded in the .ssab
#
# assets/ is gitignored. The core tests read assets/overall and
# assets/overall_emb; the viewer plays assets/overall.
#
# Usage: scripts/deploy-examples.sh [release|debug]   (ssconverter-cli build mode; default release)
#
# SDK location: $SSSDK, else ./SpriteStudio-SDK (submodule), else ../SpriteStudio-SDK.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ASSET_DIR="$ROOT_DIR/assets"

MODE="release"
for arg in "$@"; do
  case "$arg" in
    release|debug) MODE="$arg" ;;
    -h|--help) sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "deploy-examples.sh: unknown arg '$arg'" >&2; exit 2 ;;
  esac
done

# --- locate the SDK -------------------------------------------------------
SDK_DIR="${SSSDK:-}"
if [ -z "$SDK_DIR" ]; then
  for cand in "$ROOT_DIR/SpriteStudio-SDK" "$ROOT_DIR/../SpriteStudio-SDK"; do
    if [ -f "$cand/Cargo.toml" ]; then SDK_DIR="$(cd "$cand" && pwd)"; break; fi
  done
fi
if [ -z "$SDK_DIR" ] || [ ! -f "$SDK_DIR/Cargo.toml" ]; then
  echo "deploy-examples.sh: SpriteStudio-SDK not found (git submodule update --init, or set SSSDK)" >&2
  exit 1
fi

# --- build the converter --------------------------------------------------
# Built inside the SDK checkout so it uses the SDK's own workspace and target
# dir; this repo's workspace does not depend on ssconverter.
echo ">> building ssconverter-cli ($MODE) from $SDK_DIR"
CARGO_FLAGS=()
[ "$MODE" = "release" ] && CARGO_FLAGS+=(--release)
( cd "$SDK_DIR" && cargo build -p ssconverter-cli "${CARGO_FLAGS[@]}" )
CLI="$SDK_DIR/target/$MODE/ssconverter-cli"
[ -x "$CLI" ] || { echo "deploy-examples.sh: $CLI not found after build" >&2; exit 1; }

# --- convert --------------------------------------------------------------
convert () { # <sspj-relative-to-sdk> <dest-name> [--embedded]
  local sspj="$1" name="$2"; shift 2
  local dest="$ASSET_DIR/$name"
  echo ">> converting $sspj -> assets/$name $*"
  rm -rf "$dest"; mkdir -p "$dest"
  "$CLI" "$@" --out-dir "$dest" "$SDK_DIR/$sspj"
}

convert "tests/overall/overall.sspj" "overall"
convert "tests/overall/overall.sspj" "overall_emb" --embedded
convert "tests/Ringo/Ringo.sspj"     "Ringo"
convert "tests/Ringo/Ringo.sspj"     "Ringo_emb" --embedded

echo "deploy-examples.sh: done -> assets/{overall,overall_emb,Ringo,Ringo_emb}/"
