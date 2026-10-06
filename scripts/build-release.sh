#!/usr/bin/env bash
#
# Assemble the release: take the per-platform trees the build matrix produced,
# lay each one out as the drop-in a C or C++ host expects, and zip them.
#
#   scripts/build-release.sh                       # assemble from artifacts/
#   scripts/build-release.sh out=/tmp/rel          # somewhere else
#   scripts/build-release.sh verify=no             # skip re-opening the zips
#
# release.yml's package job runs this script for the assets it uploads, so CI
# and a local run cannot drift. The workflow keeps only what is GitHub's -- the
# checkout, the artifact download and the Release itself.
#
# This is the phase AFTER it has been decided what is being released. Which
# commit and which build's binaries are both settled before this script runs --
# by the versions release.yml set and the download that populated in=. So there
# is no option here that re-decides either:
#
#   git checkout ssplayer-wgpu-v0.2.0         # which release
#   gh run download <run-id> -D artifacts     # which build's binaries
#   scripts/build-release.sh                  # this
#
# What this does NOT do is build anything. Windows and macOS need an operating
# system each, so there is no local equivalent of the matrix and a "local
# release" would only ever be a partial one.
#
# Static libraries only. A C++ host links `libssplayer_wgpu.a` (or the MSVC
# `.lib`) into its own binary, which is the whole point of the ffi face owning
# its wgpu device; shipping the `.dylib`/`.so`/`.dll` alongside would add code
# signing, SONAMEs and import-library pairing to every release for a second way
# to do the same thing. build.sh still collects them for local work.

set -eu

APP="$(basename "$0")"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# The matrix jobs, and the order they are reported in. Each name is both the
# upload-artifact name and the directory `gh run download` creates for it.
# Linux has none: a Linux host builds the library from source, which takes
# nothing but a Rust toolchain.
NATIVE="windows-x86_64 macos-universal"
WEB="web-wasm32"

# The static library each platform's toolchain emits. MSVC names the staticlib
# `ssplayer_wgpu.lib`; the import library that sits next to a DLL is
# `ssplayer_wgpu.dll.lib`, which is a different file and is not shipped.
lib_for () {
  case "$1" in
    windows-*) echo "ssplayer_wgpu.lib" ;;
    *)         echo "libssplayer_wgpu.a" ;;
  esac
}

# --- options --------------------------------------------------------------

IN="artifacts"
OUT="release-dist"
VERIFY="yes"
CLEAN="yes"

usage () {
  echo "Usage: $APP [options]"
  echo "$APP options:"
  echo "  in=<dir>                Where the per-platform artifact trees are (default:"
  echo "                          $IN). One subdirectory per upload-artifact name, which"
  echo "                          is exactly what 'gh run download -D <dir>' produces."
  echo "  out=<dir>               Where the zips and SHA256SUMS land (default: $OUT)."
  echo "                          Relative paths resolve against the current directory."
  echo "  verify=<yes|no>         Re-open the finished zips and check them (default:"
  echo "                          $VERIFY)."
  echo "  clean=<yes|no>          Delete <out> before assembling (default: $CLEAN)."
    echo "  sdk= skip=              Accepted and ignored here. The family's build-release"
  echo "                          scripts share out= clean= verify= in= sdk= skip=,"
  echo "                          so one command line drives all of them."
echo "  -h | --help             Show this help"
  echo ""
  echo "Output, in <out>:"
  echo "  ssplayer-wgpu-<platform>-<arch>.zip   include/, lib/ (the library and VERSION),"
  echo "                                        LICENSE.md, LICENSE.ja.md, THIRD_PARTY_NOTICES.md"
  echo "  ssplayer-wgpu-web-wasm32.zip          the npm package (dist/, wasm/, package.json)"
  echo "  SHA256SUMS                            covers every zip above"
  echo ""
  echo "Every platform the matrix builds must be present: a partial release is not a"
  echo "thing worth producing, so a missing one is an error that lists everything else"
  echo "missing alongside it rather than failing on the first."
}

while [ $# -gt 0 ]; do
  item="$1"
  shift
  case "$item" in
    -h|--help|help) usage; exit 0 ;;
    *=*)
      key="${item%%=*}"
      value="${item#*=}"
      case "$key" in
        in)     IN="$value" ;;
        out)    OUT="$value" ;;
        verify) VERIFY="$(printf '%s' "$value" | tr '[:upper:]' '[:lower:]')" ;;
        clean)  CLEAN="$(printf '%s' "$value" | tr '[:upper:]' '[:lower:]')" ;;
        # Accepted so one command line drives build-release in every repository
        # of the family; this one has no use for them.
        sdk|skip) echo "$APP: ${key}= does not apply here; ignored" >&2 ;;
        *) echo "$APP: unknown option '$key' (see $APP --help)" >&2; exit 2 ;;
      esac
      ;;
    *) echo "$APP: unknown argument '$item' (options are key=value; see $APP --help)" >&2; exit 2 ;;
  esac
done

case "$VERIFY" in yes|no) ;; *) echo "$APP: verify must be yes or no (got '$VERIFY')" >&2; exit 2 ;; esac
case "$CLEAN"  in yes|no) ;; *) echo "$APP: clean must be yes or no (got '$CLEAN')" >&2; exit 2 ;; esac

# Resolve both paths against the invoking directory, before anything cds away.
case "$IN"  in /*) ;; *) IN="$PWD/$IN" ;; esac
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT" ;; esac

# clean=yes deletes out=, so refuse the paths that would take the repository (or
# the filesystem) with them.
if [ "$OUT" = "/" ] || [ "$OUT" = "$ROOT_DIR" ]; then
  echo "$APP: refusing to use '$OUT' as out= (it is deleted when clean=yes)" >&2
  exit 2
fi

cd "$ROOT_DIR"

# --- what this release is called ------------------------------------------
#
# release.yml tags a release only after this script has passed, so in CI there
# is no tag yet and the version alone names the release. A tag is read here when
# one is checked out -- a released version rebuilt locally -- and it has to agree
# with the version.
VERSION="$(./scripts/release-version.sh)"

if TAG="$(git -C "$ROOT_DIR" describe --exact-match --match 'ssplayer-wgpu-v[0-9]*' --tags HEAD 2>/dev/null)"; then
  TAG_SOURCE="the release tag HEAD is checked out at"
else
  TAG=""
  TAG_SOURCE="HEAD is not on a release tag"
fi

echo "options"
echo "  in      => $IN"
echo "  out     => $OUT"
echo "  verify  => $VERIFY"
echo "  clean   => $CLEAN"
echo "  version => $VERSION"
echo "  tag     => ${TAG:-(none)}"
echo "             ($TAG_SOURCE)"

# A tag that disagrees with the version is not a warning: the npm package in the
# web zip carries the version, so the Release would be called one thing and hold
# a package that says another.
if [ -n "$TAG" ] && [ "$TAG" != "ssplayer-wgpu-v$VERSION" ]; then
  echo "" >&2
  echo "$APP: tag $TAG does not match the player's version (ssplayer-wgpu-v$VERSION)." >&2
  echo "$APP: bump Cargo.toml's [workspace.package] version and packages/ssplayer-wgpu/package.json" >&2
  echo "$APP: together, or retag." >&2
  exit 1
fi

# --- preflight ------------------------------------------------------------

for tool in zip unzip; do
  command -v "$tool" > /dev/null 2>&1 || {
    echo "$APP: $tool not found -- it is what packages the archives" >&2
    exit 1
  }
done

# The runners have sha256sum (coreutils); macOS ships shasum instead. Both write
# the same `<hash>  <name>` lines and both take -c, so SHA256SUMS is identical
# either way -- which is the only reason a release can be assembled from either.
if command -v sha256sum > /dev/null 2>&1; then
  sha256 () { sha256sum "$@"; }
elif command -v shasum > /dev/null 2>&1; then
  sha256 () { shasum -a 256 "$@"; }
else
  echo "$APP: neither sha256sum nor shasum found -- one of them writes SHA256SUMS" >&2
  exit 1
fi

step () { echo ""; echo ">> $*"; }

FAILED=""
fail () { FAILED="${FAILED}
  $1"; }

# --- check the input ------------------------------------------------------
# Everything at once. Finding out about the fourth missing platform on the
# fourth run of a script that takes a matrix build to feed is not a debugging
# loop worth having.
step "checking $IN"

if [ ! -d "$IN" ]; then
  echo "$APP: $IN does not exist" >&2
  echo "$APP: populate it first -- 'gh run download <run-id> -D $(basename "$IN")' -- or point in= at the artifacts" >&2
  exit 1
fi

for p in $NATIVE; do
  os="${p%%-*}"; arch="${p#*-}"
  dir="$IN/$p"
  lib="$(lib_for "$p")"
  if [ ! -d "$dir" ]; then
    fail "$p/ is missing"
  else
    [ -f "$dir/libs/$os/$arch/$lib" ] || fail "$p/libs/$os/$arch/$lib is missing (the static library this platform ships)"
    [ -f "$dir/libs/$os/$arch/VERSION" ] || fail "$p/libs/$os/$arch/VERSION is missing (the SDK release the library links)"
    [ -f "$dir/include/ssplayer_wgpu.h" ] || fail "$p/include/ssplayer_wgpu.h is missing"
    [ -f "$dir/LICENSE.md" ] || fail "$p/LICENSE.md is missing"
    [ -f "$dir/LICENSE.ja.md" ] || fail "$p/LICENSE.ja.md is missing"
  fi
done

if [ ! -d "$IN/$WEB" ]; then
  fail "$WEB/ is missing"
else
  [ -f "$IN/$WEB/wasm/ssplayer_wgpu_bg.wasm" ] || fail "$WEB/wasm/ssplayer_wgpu_bg.wasm is missing (the wasm-pack output)"
  [ -f "$IN/$WEB/dist/index.js" ] || fail "$WEB/dist/index.js is missing (the js layer was not built)"
  [ -f "$IN/$WEB/package.json" ] || fail "$WEB/package.json is missing"
  [ -f "$IN/$WEB/wasm/VERSION" ] || fail "$WEB/wasm/VERSION is missing (the SDK release the wasm links)"
fi

if [ -n "$FAILED" ]; then
  echo "$APP: $IN is missing pieces the release needs:$FAILED" >&2
  echo "" >&2
  echo "$APP: point in= at a complete matrix build." >&2
  exit 1
fi
echo "   every platform the matrix builds is present"

# The header is generated by cbindgen from one source, so the native platform jobs
# must have produced the same bytes. They will not have if one of them built an
# older commit -- which is exactly the mix-up that ships a header describing an
# API the library does not have.
step "checking the C header is the same across platforms"
FIRST=""
for p in $NATIVE; do
  h="$IN/$p/include/ssplayer_wgpu.h"
  if [ -z "$FIRST" ]; then
    FIRST="$h"
  elif ! cmp -s "$FIRST" "$h"; then
    echo "$APP: $p's ssplayer_wgpu.h differs from ${FIRST#$IN/}'s" >&2
    echo "$APP: the artifacts are not all from the same commit" >&2
    exit 1
  fi
done
echo "   identical in all $(echo $NATIVE | wc -w | tr -d ' ') native artifacts"

# Each platform names the SpriteStudio-SDK release it links in VERSION, and a
# release ships one: a platform that disagrees built another commit. `local` is
# an SDK built from a commit that is not a release, which a release does not ship.
step "checking every platform links the same SDK release"
SDK="$(cat "$IN/$WEB/wasm/VERSION")"
for p in $NATIVE; do
  v="$(cat "$IN/$p/libs/${p%%-*}/${p#*-}/VERSION")"
  if [ "$v" != "$SDK" ]; then
    echo "$APP: $p links SDK $v, where $WEB links $SDK" >&2
    echo "$APP: the artifacts are not all from the same commit" >&2
    exit 1
  fi
done
if [ "$SDK" = "local" ]; then
  echo "$APP: the SDK the artifacts link is not a release (VERSION says local)" >&2
  echo "$APP: point the SpriteStudio-SDK submodule at the SDK's release tag first" >&2
  exit 1
fi
echo "   $SDK, in every artifact"

# --- assemble -------------------------------------------------------------
step "assembling in $OUT"

if [ "$CLEAN" = "yes" ]; then
  rm -rf "$OUT"
fi
mkdir -p "$OUT"

WORK="$ROOT_DIR/build/release-staging"
rm -rf "$WORK"

ZIPS=""

for p in $NATIVE; do
  os="${p%%-*}"; arch="${p#*-}"
  lib="$(lib_for "$p")"
  stage="$WORK/$p"
  mkdir -p "$stage/include" "$stage/lib"

  # Flat: one platform per archive, so `lib/` needs no platform subdirectory and
  # a CMake host points at include/ and lib/ without a per-OS branch.
  cp "$IN/$p/include/ssplayer_wgpu.h" "$stage/include/"
  cp "$IN/$p/libs/$os/$arch/$lib" "$IN/$p/libs/$os/$arch/VERSION" "$stage/lib/"
  cp "$IN/$p/LICENSE.md" "$IN/$p/LICENSE.ja.md" "$stage/"
  cp "$ROOT_DIR/THIRD_PARTY_NOTICES.md" "$stage/"

  zipname="ssplayer-wgpu-$p.zip"
  ( cd "$stage" && zip -qry "$OUT/$zipname" . )
  ZIPS="$ZIPS $zipname"
  echo "   $zipname ($lib)"
done

# The web face is a package, not a library file: build.sh already assembled
# packages/ssplayer-wgpu -- the wasm-pack runtime under wasm/, the tsdown output
# under dist/, the manifest and the READMEs beside them -- so it ships as it is.
# The zip is therefore installable on its own (`npm i ./ssplayer-wgpu-web-wasm32`),
# which is what makes it useful to someone who cannot reach the registry.
stage="$WORK/$WEB"
mkdir -p "$stage"
cp -R "$IN/$WEB/." "$stage/"
zipname="ssplayer-wgpu-$WEB.zip"
( cd "$stage" && zip -qry "$OUT/$zipname" . )
ZIPS="$ZIPS $zipname"
echo "   $zipname (npm package)"

# Canonical checksum manifest so a consumer can verify what they downloaded
# (`sha256sum -c SHA256SUMS --ignore-missing`).
step "writing SHA256SUMS"
( cd "$OUT" && sha256 $ZIPS > SHA256SUMS )
echo "   $(wc -l < "$OUT/SHA256SUMS" | tr -d ' ') entries"

# --- verify ---------------------------------------------------------------
# Re-open what was just written. An empty archive, a library that did not make
# it in, or a header that did, are all things that zip reports as success and
# that only a consumer discovers.
if [ "$VERIFY" = "yes" ]; then
  step "verifying $OUT"

  for p in $NATIVE; do
    lib="$(lib_for "$p")"
    zipname="ssplayer-wgpu-$p.zip"
    names="$(unzip -Z1 "$OUT/$zipname")"
    for want in "include/ssplayer_wgpu.h" "lib/$lib" "lib/VERSION" "LICENSE.md" "LICENSE.ja.md" "THIRD_PARTY_NOTICES.md"; do
      printf '%s\n' "$names" | grep -qx "$want" || fail "$zipname does not contain $want"
    done
    # A static library that is a few kilobytes is a link that produced nothing.
    bytes="$(unzip -l "$OUT/$zipname" "lib/$lib" | awk 'NR==4 {print $1}')"
    [ "${bytes:-0}" -gt 262144 ] || fail "$zipname's lib/$lib is only ${bytes:-0} bytes"
  done

  names="$(unzip -Z1 "$OUT/ssplayer-wgpu-$WEB.zip")"
  for want in "wasm/ssplayer_wgpu_bg.wasm" "wasm/ssplayer_wgpu.js" "wasm/ssplayer_wgpu.d.ts" "wasm/VERSION" \
              "dist/index.js" "dist/index.d.ts" "dist/element.js" "package.json"; do
    printf '%s\n' "$names" | grep -qx "$want" || fail "ssplayer-wgpu-$WEB.zip does not contain $want"
  done

  ( cd "$OUT" && sha256 -c SHA256SUMS > /dev/null 2>&1 ) || fail "SHA256SUMS does not match the zips it covers"

  if [ -n "$FAILED" ]; then
    echo "$APP: the assembled release is not right:$FAILED" >&2
    exit 1
  fi
  echo "   every archive holds what it should, and SHA256SUMS matches"
fi

step "done"
echo "   $OUT"
( cd "$OUT" && ls -1 ) | sed 's/^/     /'
