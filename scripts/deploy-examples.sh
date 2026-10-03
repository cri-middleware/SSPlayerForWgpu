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
#   assets/Doll/           <- tests/Doll, reference textures
#   assets/Doll_emb/       <- tests/Doll, textures embedded in the .ssab
#
# and then deploys them to the examples that cannot read assets/ in place:
#
#   examples/web/assets/       <- assets/overall (reference mode, + its .png atlases)
#   examples/web/assets/Ringo/ <- assets/Ringo   (reference mode, + Ringo.png)
#   examples/web/assets/Doll/  <- assets/Doll    (reference mode, + Doll.png)
#   examples/web/assets_emb/   <- assets/overall_emb (embedded mode)
#   examples/web/assets_emb/Ringo/ <- assets/Ringo_emb (embedded mode)
#   examples/web/assets_emb/Doll/  <- assets/Doll_emb  (embedded mode)
#
# Both modes go out because the demo page has a selector for the two: an embedded
# pack carries its atlas, a reference pack names sibling files that `loadSsab`
# fetches relative to the .ssab's own URL, and nothing else on the page
# covers that second branch.
#
# assets/ is gitignored. libs/ssplayer-wgpu's tests read assets/overall and
# assets/overall_emb; examples/viewer and examples/viewer-metal-cpp play
# assets/overall from the repository, so only the browser needs a copy.
#
# Usage: scripts/deploy-examples.sh [build=release|debug]   (ssconverter-cli build mode; default release)
#
# SDK location: $SSSDK, else ./SpriteStudio-SDK (submodule), else ../SpriteStudio-SDK.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ASSET_DIR="$ROOT_DIR/assets"
WEB_DIR="$ROOT_DIR/examples/web"

MODE="release"
for arg in "$@"; do
  case "$arg" in
    build=release|build=debug) MODE="${arg#*=}" ;;
    build=*) echo "deploy-examples.sh: build= takes release or debug, not '${arg#*=}'" >&2; exit 2 ;;
    -h|--help) sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
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
convert "tests/Doll/Doll.sspj"       "Doll"
convert "tests/Doll/Doll.sspj"       "Doll_emb" --embedded

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
# Ringo is an authored character rather than a feature fixture, which is what the
# layout pages want on the page. Kept in a subdirectory of its own: sibling packs
# and atlases resolve against the .ssab's own directory, so two projects sharing
# one flat directory would be one atlas filename away from colliding.
mkdir -p "$WEB_DIR/assets/Ringo"
cp "$ASSET_DIR/Ringo/"*.ssab "$WEB_DIR/assets/Ringo/"
cp "$ASSET_DIR/Ringo/"*.png  "$WEB_DIR/assets/Ringo/"
mkdir -p "$WEB_DIR/assets/Doll"
cp "$ASSET_DIR/Doll/"*.ssab "$WEB_DIR/assets/Doll/"
cp "$ASSET_DIR/Doll/"*.png  "$WEB_DIR/assets/Doll/"

# Embedded mode: the atlas is baked in, so these need no sibling files.
echo ">> deploying embedded .ssab -> examples/web/assets_emb"
rm -rf "$WEB_DIR/assets_emb"; mkdir -p "$WEB_DIR/assets_emb"
cp "$ASSET_DIR/overall_emb/"*.ssab "$WEB_DIR/assets_emb/"
mkdir -p "$WEB_DIR/assets_emb/Ringo"
cp "$ASSET_DIR/Ringo_emb/"*.ssab "$WEB_DIR/assets_emb/Ringo/"
mkdir -p "$WEB_DIR/assets_emb/Doll"
cp "$ASSET_DIR/Doll_emb/"*.ssab "$WEB_DIR/assets_emb/Doll/"

echo "deploy-examples.sh: done -> assets/{overall,overall_emb,Ringo,Ringo_emb,Doll,Doll_emb}/"
echo "deploy-examples.sh:      -> examples/web/{assets,assets_emb}/{,Ringo/,Doll/}"
