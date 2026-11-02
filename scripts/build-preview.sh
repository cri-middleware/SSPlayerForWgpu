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
# this script only substitutes five markers into it.
#
# The version stamp is load-bearing rather than decoration. The `.ssab` schema
# is still moving at version 1, so a stale shell does not fail -- it quietly
# draws something else. The page compares the SDK release it links against the
# converter version a host puts in its URL. See PREVIEW.md.
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

# The SDK version the wasm links, named the way build.sh names it: the page
# reads it back as `ctx.sdkVersion` and compares it with the converter's.
[ -f "$ROOT_DIR/SpriteStudio-SDK/Cargo.toml" ] || { echo "$APP: SpriteStudio-SDK is not checked out" >&2; exit 1; }
SDK_VERSION="v$("$ROOT_DIR/SpriteStudio-SDK/scripts/release-version.sh")" || exit 1
export SSPLAYER_SDK_VERSION="$SDK_VERSION"

echo "$APP: building the no-modules wasm ($BUILD, SDK $SDK_VERSION)"
wasm-pack build "$ROOT_DIR/libs/ssplayer-wgpu-ffi" \
  --target no-modules --"$BUILD" \
  --out-name ssplayer_wgpu --out-dir "$WORK/pkg" \
  -- --no-default-features --features web

GLUE="$WORK/pkg/ssplayer_wgpu.js"
WASM="$WORK/pkg/ssplayer_wgpu_bg.wasm"

# --- the stamp ------------------------------------------------------------
# The player's version, and the SDK as `ss_converter_version()` spells it --
# the release, `+`, the first seven characters of the commit -- so the stamp
# reads the same as the converter version a host shows beside it.
#
# The version is the one cargo resolves, not a line read out of a manifest: the
# crate inherits it from the workspace, and reading `version = "..."` from its
# own Cargo.toml found nothing once it did, which stamped every page with no
# version at all.
VERSION="$(cargo metadata --no-deps --format-version 1 --manifest-path "$ROOT_DIR/Cargo.toml" |
  python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "ssplayer-wgpu"))')"
[ -n "$VERSION" ] || { echo "$APP: cargo metadata reported no version for ssplayer-wgpu" >&2; exit 1; }
SDK_COMMIT="$(git -C "$ROOT_DIR/SpriteStudio-SDK" rev-parse HEAD 2>/dev/null | cut -c1-7 || true)"
[ -n "$SDK_COMMIT" ] || { echo "$APP: SpriteStudio-SDK is not checked out" >&2; exit 1; }
STAMP="ssplayer-wgpu $VERSION · SDK ${SDK_VERSION#v}+$SDK_COMMIT"
BUILT="$(date -u +%Y-%m-%d)"

# --- substitute -----------------------------------------------------------
# python3 rather than sed: the glue is over 200 KB and the base64 is over a
# megabyte, and both carry every character sed would treat as a replacement
# escape.
mkdir -p "$(dirname "$OUT")"
python3 - "$SHELL_SRC" "$GLUE" "$WASM" "$OUT" "$STAMP" "$BUILT" "$ROOT_DIR/LICENSE.md" "$ROOT_DIR/THIRD_PARTY_NOTICES.md" <<'PY'
import base64, json, pathlib, re, sys

shell, glue, wasm, out, stamp, built, license, notices = sys.argv[1:9]
own = pathlib.Path(shell).read_text(encoding="utf-8")

# The page carries its own notices, since it travels alone: as a Release asset
# and inside each Player's package. A JSON string with every `<` escaped, so no
# license text can close the <script> that holds it.
text = pathlib.Path(license).read_text(encoding="utf-8") + "\n\n" + pathlib.Path(notices).read_text(encoding="utf-8")
values = {
    "__SS_GLUE__": pathlib.Path(glue).read_text(encoding="utf-8"),
    "__SS_WASM__": base64.b64encode(pathlib.Path(wasm).read_bytes()).decode("ascii"),
    "__SS_VERSION__": stamp,
    "__SS_BUILT__": built,
    "__SS_NOTICES__": json.dumps(text, ensure_ascii=False).replace("<", "\\u003c"),
}
# Exactly one, never "at least one": a second mention -- in a comment, say --
# would be substituted too, and two copies of the wasm is a page twice the size
# that still works, so nothing downstream would notice. One pass over the
# source, so a marker's spelling inside a substituted value stays as it is.
for marker in sorted(set(re.findall(r"__SS_[A-Z]+__", own)) | set(values)):
    found = own.count(marker)
    if marker not in values or found != 1:
        raise SystemExit(f"{marker} appears {found} times in {shell}; it must be one of {', '.join(values)} and appear exactly once")
page = re.sub(r"__SS_[A-Z]+__", lambda match: values[match.group(0)], own)

# Nothing of OURS may reach outside the file unasked: at file:// an external
# script is blocked, and so is `fetch` unless Chrome was started with
# --allow-file-access-from-files, so a slip would fail only on the machine that
# opened the artifact, never here. The one `fetch` is the URL entrance's, which
# a host opens in exactly that Chrome. The glue is exempt on purpose --
# wasm-bindgen carries a fetch path for loading by URL, and passing the bytes in
# is exactly what stops it being taken.
for forbidden in ("import(", "XMLHttpRequest"):
    if forbidden in own:
        raise SystemExit(f"{shell} contains {forbidden!r}, which cannot work at file://")
fetches = len(re.findall(r"\bfetch\(", own))
if fetches != 1:
    raise SystemExit(f"{shell} calls fetch {fetches} times; the URL entrance's one call is the only one that works at file://")
if "<script" in page and " src=" in page.split("<script", 1)[1][:200]:
    raise SystemExit("the assembled page loads a script by src, which cannot work at file://")

# Every method the page calls on the player's objects must be one the glue it
# ships with defines. Those names are the js face's, so renaming one in the Rust
# compiles, builds this page, and leaves it throwing a TypeError in a browser --
# the one place no check of ours opens it. Getters do not count: the page calls
# what it names.
source = pathlib.Path(glue).read_text(encoding="utf-8")
classes = set(re.findall(r"^\s*class (\w+)", source, re.M))
methods = set(re.findall(r"^\s+(?:static\s+)?(?:async\s+)?(\w+)\s*\([^)]*\)\s*\{", source, re.M))
calls = set(re.findall(r"\b(?:ctx|surface|player)\??\.(\w+)\(", own))
statics = set(re.findall(r"\bwasm_bindgen\.(\w+)\.(\w+)\(", own))
if not calls or not statics:
    raise SystemExit(f"no call on wasm_bindgen, ctx, surface or player found in {shell}; the check below would prove nothing")
unknown = sorted({m for m in calls if m not in methods} |
                 {f"{c}.{m}" for c, m in statics if c not in classes or m not in methods})
if unknown:
    raise SystemExit(f"{shell} calls {', '.join(unknown)}, which the js face's glue does not define")

pathlib.Path(out).write_text(page, encoding="utf-8")
PY

SIZE="$(wc -c < "$OUT" | tr -d ' ')"
echo "$APP: done -> $OUT ($(( SIZE / 1024 )) KiB, $STAMP)"
echo "$APP: open it directly -- no server, no install. Drop a .ssab on it."
echo "$APP: or open it as a host does, by path: see examples/preview/README.md."
echo "$APP: a generator fills the <script id=\"ssab\"> hole to ship a pack inside it."
