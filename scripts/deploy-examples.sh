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
# and then deploys them to the examples that cannot read assets/ in place:
#
#   examples/web/assets/       <- assets/overall (reference mode, + its .png atlases)
#   examples/web/assets_emb/   <- assets/overall_emb (embedded mode)
#
# Both modes go out because the demo page has a selector for the two: an embedded
# pack carries its atlas, a reference pack names sibling files that `loadSsab`
# fetches relative to the .ssab's own URL (SPEC 8.1), and nothing else on the page
# covers that second branch.
#
# assets/ is gitignored. libs/ssplayer-wgpu's tests read assets/overall and
# assets/overall_emb; examples/viewer and examples/viewer-metal-cpp play
# assets/overall from the repository, so only the browser needs a copy.
#
# Usage: scripts/deploy-examples.sh [release|debug]   (ssconverter-cli build mode; default release)
#
# SDK location: $SSSDK, else ./SpriteStudio-SDK (submodule), else ../SpriteStudio-SDK.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ASSET_DIR="$ROOT_DIR/assets"
WEB_DIR="$ROOT_DIR/examples/web"

MODE="release"
for arg in "$@"; do
  case "$arg" in
    release|debug) MODE="$arg" ;;
    -h|--help) sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
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
# One token or nothing: an empty array would trip `set -u` on bash 3.2 (macOS).
PROFILE_FLAG=""
[ "$MODE" = "release" ] && PROFILE_FLAG="--release"
# shellcheck disable=SC2086
( cd "$SDK_DIR" && cargo build -p ssconverter-cli $PROFILE_FLAG )
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

# --- deploy to the examples ----------------------------------------------
# A page can only fetch what it serves, so the web demo gets its own copy. The
# `_emb` suffix mirrors assets/overall vs assets/overall_emb, so the two modes
# read the same way at both ends of the copy.
# Reference mode, atlases included: that is the branch the page exercises by
# resolving each .png against the .ssab's own URL and fetching it.
echo ">> deploying reference .ssab + atlases -> examples/web/assets"
rm -rf "$WEB_DIR/assets"; mkdir -p "$WEB_DIR/assets"
cp "$ASSET_DIR/overall/"*.ssab "$WEB_DIR/assets/"
cp "$ASSET_DIR/overall/"*.png  "$WEB_DIR/assets/"

# Embedded mode: the atlas is baked in, so these need no sibling files.
echo ">> deploying embedded .ssab -> examples/web/assets_emb"
rm -rf "$WEB_DIR/assets_emb"; mkdir -p "$WEB_DIR/assets_emb"
cp "$ASSET_DIR/overall_emb/"*.ssab "$WEB_DIR/assets_emb/"

echo "deploy-examples.sh: done -> assets/{overall,overall_emb,Ringo,Ringo_emb}/"
echo "deploy-examples.sh:      -> examples/web/{assets,assets_emb}/"
