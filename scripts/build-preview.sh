#!/usr/bin/env bash
#
# Assemble the self-contained `.ssab` preview shell: one HTML file with the
# wasm and the glue already inside it.
#
#   scripts/build-preview.sh                 # -> bin/ssplayer-preview.html
#   scripts/build-preview.sh out=/tmp/p.html # somewhere else
#   scripts/build-preview.sh build=debug     # a much larger, unoptimised wasm
#
# Why this is its own build rather than a mode of build.sh: the page has to run
# from `file://`, where an external module script is blocked and an inline one
# is not -- so it needs wasm-pack's `no-modules` target, and `build.sh
# platform=web` builds `--target web` for the npm package. Two targets, two
# artifacts, no shared output. Everything else in the page (markup, CSS, the
# page's own script) lives in examples/preview/shell.html and ships verbatim;
# this script only substitutes four markers into it.
#
# The version stamp is load-bearing rather than decoration. The `.ssab` schema
# is still moving at version 1, so a stale shell does not fail -- it quietly
# draws something else. A host compares what the page reports against the
# version it converted with. See PREVIEW.md.
set -euo pipefail

APP="$(basename "$0")"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# --- options --------------------------------------------------------------
OUT="$ROOT_DIR/bin/ssplayer-preview.html"
BUILD="release"

usage () {
  echo "Usage: $APP [options]"
  echo "$APP options:"
  echo "  out=<file>          Where to write the page (default: bin/ssplayer-preview.html)"
  echo "  build=<release|debug>"
  echo "                      Cargo profile for the wasm (default: $BUILD). debug is"
  echo "                      several times larger and is only for reading a panic."
  echo "  -h | --help         Show this help"
}

for arg in "$@"; do
  case "$arg" in
    out=*)   OUT="${arg#out=}" ;;
    build=*) BUILD="${arg#build=}" ;;
    -h|--help) usage; exit 0 ;;
    *) echo "$APP: unknown option '$arg' (see $APP --help)" >&2; exit 2 ;;
  esac
done
case "$BUILD" in release|debug) ;; *) echo "$APP: build must be release or debug (got '$BUILD')" >&2; exit 2 ;; esac

command -v wasm-pack >/dev/null 2>&1 || { echo "$APP: wasm-pack not found -- cargo install wasm-pack" >&2; exit 1; }

SHELL_SRC="$ROOT_DIR/examples/preview/shell.html"
[ -f "$SHELL_SRC" ] || { echo "$APP: $SHELL_SRC is missing" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "$APP: building the no-modules wasm ($BUILD)"
wasm-pack build "$ROOT_DIR/libs/ssplayer-wgpu-ffi" \
  --target no-modules --"$BUILD" \
  --out-name ssplayer_wgpu --out-dir "$WORK/pkg" \
  -- --no-default-features --features web

GLUE="$WORK/pkg/ssplayer_wgpu.js"
WASM="$WORK/pkg/ssplayer_wgpu_bg.wasm"

# --- the stamp ------------------------------------------------------------
# The player's version, and the SDK commit it was built against: the submodule
# is a commit rather than a release, so the commit is the only thing that
# identifies what converted-data version this page agrees with.
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/libs/ssplayer-wgpu/Cargo.toml" | head -1)"
SDK="$(git -C "$ROOT_DIR/SpriteStudio-SDK" rev-parse --short HEAD 2>/dev/null || echo unknown)"
STAMP="ssplayer-wgpu $VERSION · SDK $SDK"
BUILT="$(date -u +%Y-%m-%d)"

# --- substitute -----------------------------------------------------------
# python3 rather than sed: the glue is 160 KB and the base64 is over a megabyte,
# and both carry every character sed would treat as a replacement escape.
mkdir -p "$(dirname "$OUT")"
python3 - "$SHELL_SRC" "$GLUE" "$WASM" "$OUT" "$STAMP" "$BUILT" <<'PY'
import base64, pathlib, sys

shell, glue, wasm, out, stamp, built = sys.argv[1:7]
page = pathlib.Path(shell).read_text(encoding="utf-8")
for marker, value in (
    ("__SS_GLUE__", pathlib.Path(glue).read_text(encoding="utf-8")),
    ("__SS_WASM__", base64.b64encode(pathlib.Path(wasm).read_bytes()).decode("ascii")),
    ("__SS_VERSION__", stamp),
    ("__SS_BUILT__", built),
):
    # Exactly one, never "at least one": a second mention -- in a comment, say --
    # would be substituted too, and two copies of the wasm is a page twice the
    # size that still works, so nothing downstream would notice.
    found = page.count(marker)
    if found != 1:
        raise SystemExit(f"{marker} appears {found} times in {shell}; it must appear exactly once")
    page = page.replace(marker, value)

# Nothing of OURS may reach outside the file: at file:// `fetch` and an external
# script are both blocked, and a slip would fail only on the machine that opened
# the artifact, never here. The glue is exempt from the `fetch` check on purpose
# -- wasm-bindgen carries a fetch path for loading by URL, and passing the bytes
# in is exactly what stops it being taken.
own = pathlib.Path(shell).read_text(encoding="utf-8")
for forbidden in ("fetch(", "import(", "XMLHttpRequest"):
    if forbidden in own:
        raise SystemExit(f"{shell} contains {forbidden!r}, which cannot work at file://")
if "<script" in page and " src=" in page.split("<script", 1)[1][:200]:
    raise SystemExit("the assembled page loads a script by src, which cannot work at file://")

pathlib.Path(out).write_text(page, encoding="utf-8")
PY

SIZE="$(wc -c < "$OUT" | tr -d ' ')"
echo "$APP: done -> $OUT ($(( SIZE / 1024 )) KiB, $STAMP)"
echo "$APP: open it directly -- no server, no install. Drop a .ssab on it."
echo "$APP: a generator fills the <script id=\"ssab\"> hole to ship a pack inside it."
