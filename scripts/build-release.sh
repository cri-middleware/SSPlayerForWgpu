#!/usr/bin/env bash
#
# Assemble the release: take the per-platform trees the build matrix produced,
# lay each native one out as the drop-in a C or C++ host expects and zip it,
# pack the npm package into the tarball npm is given, and pass the preview shell
# through as the one file it is.
#
#   scripts/build-release.sh                       # assemble from artifacts/
#   scripts/build-release.sh out=/tmp/rel          # somewhere else
#   scripts/build-release.sh verify=no             # skip re-opening the archives
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
# The static library on every platform, and on macOS the shared one beside it.
# A C++ host links `libssplayer_wgpu.a` (or the MSVC `.lib`) into its own
# binary, which is the whole point of the ffi face owning its wgpu device. A
# host that loads the library instead -- Python, .NET or Dart through an FFI --
# needs a shared one, and on macOS a downloaded one loads only once it is signed
# and notarized, which scripts/release-macos.sh does in the macOS job; this
# script can only check its shape, since codesign does not run here. The
# Windows zip still carries the static library alone: its DLL would bring the
# import library `ssplayer_wgpu.dll.lib` beside the static `ssplayer_wgpu.lib`,
# a pairing a release has not taken on. build.sh still collects every library
# for local work.

set -eu

APP="$(basename "$0")"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# The matrix jobs, and the order they are reported in. Each name is both the
# upload-artifact name and the directory `gh run download` creates for it.
# Linux has none: a Linux host builds the library from source, which takes
# nothing but a Rust toolchain.
NATIVE="windows-x86_64 macos-universal"
WEB="web-wasm32"
# The self-contained preview shell (PREVIEW.md), which the web job builds with
# scripts/build-preview.sh beside the package, and its name on the Release.
PREVIEW="preview-shell"
SHELL_HTML="ssplayer-preview.html"

# The static library each platform's toolchain emits. MSVC names the staticlib
# `ssplayer_wgpu.lib`; the import library that sits next to a DLL is
# `ssplayer_wgpu.dll.lib`, which is a different file and is not shipped.
lib_for () {
  case "$1" in
    windows-*) echo "ssplayer_wgpu.lib" ;;
    *)         echo "libssplayer_wgpu.a" ;;
  esac
}

# The shared library a platform's zip carries beside the static one, if any.
shared_for () {
  case "$1" in
    macos-*) echo "libssplayer_wgpu.dylib" ;;
    *)       echo "" ;;
  esac
}

# --- options --------------------------------------------------------------

IN="artifacts"
OUT="release-dist"
VERIFY="yes"
CLEAN="yes"
SIGN="yes"

usage () {
  echo "Usage: $APP [options]"
  echo "$APP options:"
  echo "  in=<dir>                Where the per-platform artifact trees are (default:"
  echo "                          $IN). One subdirectory per upload-artifact name, which"
  echo "                          is exactly what 'gh run download -D <dir>' produces."
  echo "  out=<dir>               Where the archives and SHA256SUMS land (default: $OUT)."
  echo "                          Relative paths resolve against the current directory."
  echo "  verify=<yes|no>         Re-open the finished archives and check them (default:"
  echo "                          $VERIFY)."
  echo "  clean=<yes|no>          Delete <out> before assembling (default: $CLEAN)."
  echo "  sign=<yes|auto>         Whether the macOS dylib must carry a Developer ID"
  echo "                          signature (default: $SIGN), as scripts/release-macos.sh"
  echo "                          leaves it. yes is a release; auto reports what it"
  echo "                          carries, for a dry run without the signing secrets."
  echo "                          Either way it must be universal with an @rpath name."
    echo "  sdk= skip=              Accepted and ignored here. The family's build-release"
  echo "                          scripts share out= clean= verify= in= sdk= skip=,"
  echo "                          so one command line drives all of them."
echo "  -h | --help             Show this help"
  echo ""
  echo "Output, in <out>:"
  echo "  ssplayer-wgpu-<platform>-<arch>.zip   include/, lib/ (the library and VERSION; on"
  echo "                                        macOS the signed dylib as well),"
  echo "                                        LICENSE.md, LICENSE.ja.md, THIRD_PARTY_NOTICES.md,"
  echo "                                        README.md and docs/ (the documentation)"
  echo "  spritestudio-ssplayer-wgpu-<version>.tgz"
  echo "                                        the npm package, packed by 'npm pack'"
  echo "  ssplayer-preview.html                 the preview shell, as build-preview.sh wrote it"
  echo "  SHA256SUMS                            covers every file above"
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
        sign)   SIGN="$(printf '%s' "$value" | tr '[:upper:]' '[:lower:]')" ;;
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
case "$SIGN"   in yes|auto) ;; *) echo "$APP: sign must be yes or auto (got '$SIGN')" >&2; exit 2 ;; esac

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
echo "  sign    => $SIGN"
echo "  version => $VERSION"
echo "  tag     => ${TAG:-(none)}"
echo "             ($TAG_SOURCE)"

# A tag that disagrees with the version is not a warning: the npm package
# carries the version, so the Release would be called one thing and hold a
# package that says another.
if [ -n "$TAG" ] && [ "$TAG" != "ssplayer-wgpu-v$VERSION" ]; then
  echo "" >&2
  echo "$APP: tag $TAG does not match the player's version (ssplayer-wgpu-v$VERSION)." >&2
  echo "$APP: bump Cargo.toml's [workspace.package] version and packages/ssplayer-wgpu/package.json" >&2
  echo "$APP: together, or retag." >&2
  exit 1
fi

# --- preflight ------------------------------------------------------------

for tool in zip unzip tar; do
  command -v "$tool" > /dev/null 2>&1 || {
    echo "$APP: $tool not found -- it is what packages and checks the archives" >&2
    exit 1
  }
done
command -v npm > /dev/null 2>&1 || {
  echo "$APP: npm not found -- it is what packs the npm package" >&2
  exit 1
}

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

# The preview shell's stamp, holes and self-containment, read from the page.
# Prints the stamp, or the problems on stderr and fails. A function of its own,
# because bash 3.2 -- macOS's -- cannot parse a here-document inside $( ).
#   check_preview <file> <version> <SDK commit>
check_preview () {
  node - "$1" "$2" "$3" <<'JS'
const [file, version, sdk] = process.argv.slice(2);
const page = require('fs').readFileSync(file, 'utf8');
const problems = [];
const stamp = (page.match(/<span class="stamp" id="stamp">([^<]*)<\/span>/) || [])[1];
const parts = stamp && stamp.match(/^ssplayer-wgpu (\S+) \u00b7 SDK ([0-9a-f]{7,40}) \u00b7 \d{4}-\d{2}-\d{2}$/);
if (!parts) problems.push(`its stamp is ${JSON.stringify(stamp ?? null)}, not "ssplayer-wgpu <version> \u00b7 SDK <commit> \u00b7 <date>"`);
else {
  if (parts[1] !== version) problems.push(`it is stamped ${parts[1]}, where this checkout says ${version}`);
  if (!sdk.startsWith(parts[2])) problems.push(`it was built against SDK ${parts[2]}, where this checkout pins ${sdk.slice(0, 12)}`);
}
const left = page.match(/__SS_[A-Z]+__/g);
if (left) problems.push(`it still holds ${[...new Set(left)].join(', ')}, which the build substitutes`);
const wasm = (page.match(/<script id="wasm" type="application\/base64">([^<]*)<\/script>/) || [])[1] || '';
const bytes = Buffer.from(wasm, 'base64');
if (bytes.length < 8 || bytes.readUInt32BE(0) !== 0x0061736d) problems.push('its wasm hole holds no wasm module');
if (!page.includes('<script id="ssab" type="application/base64"></script>')) problems.push('its .ssab hole is not empty: a filled preview, not the shell');
if (/<script\b[^>]*\bsrc\s*=|<link\b[^>]*\bhref\s*=/i.test(page)) problems.push('it loads something by src or href, which file:// blocks');
if (problems.length) { console.error(problems.join('\n')); process.exit(1); }
console.log(stamp);
JS
}

# Every page a front door links to, looked up in the archive's own listing, so
# renaming or dropping a page fails the release instead of shipping a link that
# goes nowhere. <base> is where the links resolve inside the archive.
#   check_front_door <archive> <front door's text> <listing> <base>
check_front_door () {
  links="$(printf '%s\n' "$2" | grep -oE ']\([^):#]+\.md' | sed 's/^](//')"
  if [ -z "$links" ]; then
    fail "$1: no page links parsed out of its front door -- the check proved nothing"
    return
  fi
  for link in $links; do
    printf '%s\n' "$3" | grep -qxF "$4$link" || fail "$1: the front door links to $link, which is not in it"
  done
}

# A universal dylib's slices, install names and signatures, read from the
# Mach-O, since this runs where codesign does not. Prints one line naming each
# slice and its signature, or the problems on stderr and fails. A function of
# its own, because bash 3.2 -- macOS's -- cannot parse a here-document inside
# $( ).
#   check_dylib <file> <sign: yes|auto>
check_dylib () {
  node - "$1" "$2" <<'JS'
const [file, sign] = process.argv.slice(2);
const bytes = require('fs').readFileSync(file);
const want = { 0x0100000c: 'arm64', 0x01000007: 'x86_64' };
const problems = [], slices = [];
if (bytes.readUInt32BE(0) !== 0xcafebabe) problems.push('it is not a universal Mach-O');
else {
  for (let i = 0, n = bytes.readUInt32BE(4); i < n; i++) {
    const at = 8 + i * 20, cpu = bytes.readUInt32BE(at);
    const arch = want[cpu] ?? `cpu 0x${cpu.toString(16)}`;
    const slice = bytes.subarray(bytes.readUInt32BE(at + 8), bytes.readUInt32BE(at + 8) + bytes.readUInt32BE(at + 12));
    // mach_header_64: magic, cputype, cpusubtype, filetype, ncmds, sizeofcmds, flags, reserved.
    if (slice.readUInt32LE(0) !== 0xfeedfacf || slice.readUInt32LE(12) !== 6) { problems.push(`its ${arch} slice is not a 64-bit dylib`); continue; }
    let name = '', signature = 'unsigned';
    for (let c = 0, cmd = 32, n = slice.readUInt32LE(16); c < n; c++, cmd += slice.readUInt32LE(cmd + 4)) {
      const kind = slice.readUInt32LE(cmd);
      if (kind === 0x0d) name = slice.toString('latin1', cmd + slice.readUInt32LE(cmd + 8)).split('\0')[0];  // LC_ID_DYLIB
      if (kind === 0x1d) {  // LC_CODE_SIGNATURE: the blob's offset and size in the slice
        const blob = slice.subarray(slice.readUInt32LE(cmd + 8), slice.readUInt32LE(cmd + 8) + slice.readUInt32LE(cmd + 12));
        signature = blob.includes('Developer ID Application') ? 'Developer ID' : 'ad-hoc';
      }
    }
    if (name !== '@rpath/libssplayer_wgpu.dylib') problems.push(`its ${arch} install name is '${name}'`);
    if (sign === 'yes' && signature !== 'Developer ID') problems.push(`its ${arch} slice is ${signature}, not signed with a Developer ID`);
    slices.push(`${arch} (${signature})`);
  }
  for (const arch of Object.values(want)) if (!slices.some((s) => s.startsWith(`${arch} `))) problems.push(`it has no ${arch} slice`);
}
if (problems.length) { console.error(problems.join('\n')); process.exit(1); }
console.log(slices.join(', '));
JS
}

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
  shared="$(shared_for "$p")"
  if [ ! -d "$dir" ]; then
    fail "$p/ is missing"
  else
    [ -f "$dir/libs/$os/$arch/$lib" ] || fail "$p/libs/$os/$arch/$lib is missing (the static library this platform ships)"
    [ -z "$shared" ] || [ -f "$dir/libs/$os/$arch/$shared" ] || fail "$p/libs/$os/$arch/$shared is missing (the shared library this platform ships)"
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
  [ -f "$IN/$WEB/THIRD_PARTY_NOTICES.md" ] || fail "$WEB/THIRD_PARTY_NOTICES.md is missing (the package's files list it)"
  [ -f "$IN/$WEB/docs/README.md" ] || fail "$WEB/docs/README.md is missing (the documentation build.sh platform=web copies)"
fi
[ -f "$IN/$PREVIEW/$SHELL_HTML" ] || fail "$PREVIEW/$SHELL_HTML is missing (the preview shell scripts/build-preview.sh builds)"

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

# The preview shell names its SDK by commit rather than by tag -- the stamp is
# what a host compares against the converter it used (PREVIEW.md) -- so it is
# checked against the commit this checkout pins, which the tree records whether
# or not the submodule is checked out. It must carry the version being
# released, have every hole the build fills filled and the pack's left empty,
# and load nothing from outside itself: a page that does works over http:// and
# fails at file://, which is the only way it is opened. build-preview.sh refuses
# such a page as well; this is the file that ships.
step "checking the preview shell"
SDK_COMMIT="$(git -C "$ROOT_DIR" rev-parse HEAD:SpriteStudio-SDK)"
if ! STAMP="$(check_preview "$IN/$PREVIEW/$SHELL_HTML" "$VERSION" "$SDK_COMMIT")"; then
  echo "$APP: $PREVIEW/$SHELL_HTML is not the shell of this release" >&2
  exit 1
fi
echo "   $STAMP"

# The web job wrote the version into the package's own package.json, and that copy
# is what npm is given. One from another run would go out under a version the
# Release is not tagged with, and release.yml would ask npm whether it has the
# wrong one.
step "checking the npm package's version"
PKG_VERSION="$(node -p 'require(process.argv[1]).version' "$IN/$WEB/package.json")"
if [ "$PKG_VERSION" != "$VERSION" ]; then
  echo "$APP: $WEB/package.json says $PKG_VERSION, where this checkout says $VERSION" >&2
  echo "$APP: the artifacts are not from this commit" >&2
  exit 1
fi
echo "   $PKG_VERSION, as this checkout says"

# The macOS dylib is the one scripts/release-macos.sh finished: both slices, the
# @rpath install name, and -- on a release -- a Developer ID signature, without
# which a downloaded copy does not load. codesign does not run here, so
# check_dylib reads the Mach-O itself: it proves the signature blob carries a
# Developer ID certificate, not that the signature is valid or notarized. The
# macOS job checked both, with codesign and against Apple's servers; this
# catches an artifact that skipped those checks, or came from another run.
for p in $NATIVE; do
  shared="$(shared_for "$p")"
  [ -n "$shared" ] || continue
  step "checking $p's $shared"
  if ! summary="$(check_dylib "$IN/$p/libs/${p%%-*}/${p#*-}/$shared" "$SIGN")"; then
    echo "$APP: $p's $shared is not the one scripts/release-macos.sh finishes" >&2
    exit 1
  fi
  echo "   $summary, @rpath/$shared"
  case "$summary" in
    *ad-hoc*|*unsigned*) echo "   not signed with a Developer ID: sign=auto, and a release would stop here" ;;
  esac
done

# --- assemble -------------------------------------------------------------
step "assembling in $OUT"

if [ "$CLEAN" = "yes" ]; then
  rm -rf "$OUT"
fi
mkdir -p "$OUT"

WORK="$ROOT_DIR/build/release-staging"
rm -rf "$WORK"

ASSETS=""

for p in $NATIVE; do
  os="${p%%-*}"; arch="${p#*-}"
  lib="$(lib_for "$p")"
  shared="$(shared_for "$p")"
  stage="$WORK/$p"
  mkdir -p "$stage/include" "$stage/lib"

  # Flat: one platform per archive, so `lib/` needs no platform subdirectory and
  # a CMake host points at include/ and lib/ without a per-OS branch.
  cp "$IN/$p/include/ssplayer_wgpu.h" "$stage/include/"
  cp "$IN/$p/libs/$os/$arch/$lib" "$IN/$p/libs/$os/$arch/VERSION" "$stage/lib/"
  # The signature is inside the Mach-O, so zip carries it as it carries any bytes.
  [ -z "$shared" ] || cp "$IN/$p/libs/$os/$arch/$shared" "$stage/lib/"
  cp "$IN/$p/LICENSE.md" "$IN/$p/LICENSE.ja.md" "$stage/"
  cp "$ROOT_DIR/THIRD_PARTY_NOTICES.md" "$stage/"
  # The documentation, read offline beside the library it describes
  # (scripts/copy-docs.mjs). README.md is the zip's front door; verify checks
  # every page it links to.
  cp "$ROOT_DIR/scripts/release-docs/zip-README.md" "$stage/README.md"
  node "$ROOT_DIR/scripts/copy-docs.mjs" "$stage/docs" > /dev/null

  zipname="ssplayer-wgpu-$p.zip"
  ( cd "$stage" && zip -qry "$OUT/$zipname" . )
  ASSETS="$ASSETS $zipname"
  echo "   $zipname ($lib${shared:+, $shared})"
done

# The web face is a package, not a library file: build.sh already assembled
# packages/ssplayer-wgpu -- the wasm-pack runtime under wasm/, the tsdown output
# under dist/, the manifest and the READMEs beside them -- so it ships as the
# tarball `npm pack` makes of it. That one file is what release.yml attaches to
# the Release and stages on npm, and what a maintainer publishes by hand; it
# installs on its own too (`npm i ./<tarball>`), for someone who cannot reach the
# registry.
#
# The package's `prepack` refuses a package whose build outputs are missing, but
# it reaches its script by `../../scripts/`, which holds only inside a checkout.
# So the script is run here on the artifact itself, and the pack skips the hook.
node "$ROOT_DIR/scripts/verify-publish-artifacts.mjs" "$IN/$WEB"
TGZ="$(npm pack "$IN/$WEB" --pack-destination "$OUT" --ignore-scripts --json |
  node -p 'JSON.parse(require("fs").readFileSync(0, "utf8"))[0].filename')"
ASSETS="$ASSETS $TGZ"
echo "   $TGZ (npm package)"

# The shell goes out as build-preview.sh wrote it, under the name it has in a
# checkout. A plugin pins it by the Release's tag, as it would a zip, so its
# name carries no version either; the stamp inside says which it is.
cp "$IN/$PREVIEW/$SHELL_HTML" "$OUT/"
ASSETS="$ASSETS $SHELL_HTML"
echo "   $SHELL_HTML (preview shell)"

# Canonical checksum manifest so a consumer can verify what they downloaded
# (`sha256sum -c SHA256SUMS --ignore-missing`).
step "writing SHA256SUMS"
( cd "$OUT" && sha256 $ASSETS > SHA256SUMS )
echo "   $(wc -l < "$OUT/SHA256SUMS" | tr -d ' ') entries"

# --- verify ---------------------------------------------------------------
# Re-open what was just written. An empty archive, a library that did not make
# it in, or a header that did, are all things that zip reports as success and
# that only a consumer discovers.
if [ "$VERIFY" = "yes" ]; then
  step "verifying $OUT"

  for p in $NATIVE; do
    lib="$(lib_for "$p")"
    shared="$(shared_for "$p")"
    zipname="ssplayer-wgpu-$p.zip"
    names="$(unzip -Z1 "$OUT/$zipname")"
    for want in "include/ssplayer_wgpu.h" "lib/$lib" ${shared:+"lib/$shared"} "lib/VERSION" "LICENSE.md" "LICENSE.ja.md" "THIRD_PARTY_NOTICES.md" "README.md"; do
      printf '%s\n' "$names" | grep -qx "$want" || fail "$zipname does not contain $want"
    done
    check_front_door "$zipname" "$(unzip -p "$OUT/$zipname" README.md)" "$names" ""
    # A library that is a few kilobytes is a link that produced nothing.
    for want in "$lib" $shared; do
      bytes="$(unzip -l "$OUT/$zipname" "lib/$want" | awk 'NR==4 {print $1}')"
      [ "${bytes:-0}" -gt 262144 ] || fail "$zipname's lib/$want is only ${bytes:-0} bytes"
    done
  done

  # npm decides what goes in by `files` and its own ignore rules, so the tarball
  # is read back rather than the directory it was packed from.
  names="$(tar -tzf "$OUT/$TGZ")"
  for want in "wasm/ssplayer_wgpu_bg.wasm" "wasm/ssplayer_wgpu.js" "wasm/ssplayer_wgpu.d.ts" "wasm/VERSION" \
              "dist/index.js" "dist/index.d.ts" "dist/element.js" "package.json" "LICENSE.md" "THIRD_PARTY_NOTICES.md" \
              "docs/README.md"; do
    printf '%s\n' "$names" | grep -qx "package/$want" || fail "$TGZ does not contain $want"
  done
  check_front_door "$TGZ" "$(tar -xzOf "$OUT/$TGZ" package/docs/README.md)" "$names" "package/docs/"

  cmp -s "$IN/$PREVIEW/$SHELL_HTML" "$OUT/$SHELL_HTML" || fail "$SHELL_HTML is not the shell that was checked"

  ( cd "$OUT" && sha256 -c SHA256SUMS > /dev/null 2>&1 ) || fail "SHA256SUMS does not match the archives it covers"

  if [ -n "$FAILED" ]; then
    echo "$APP: the assembled release is not right:$FAILED" >&2
    exit 1
  fi
  echo "   every archive holds what it should, and SHA256SUMS matches"
fi

step "done"
echo "   $OUT"
( cd "$OUT" && ls -1 ) | sed 's/^/     /'
