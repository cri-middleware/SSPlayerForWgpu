#!/usr/bin/env bash
#
# Make the universal macOS shared library in bin/ shippable: point its install
# name at @rpath, sign it with the organisation's Developer ID, notarize it, and
# check that each of those took.
#
#   scripts/build.sh build=release arch=universal   # first: bin/libs/macos/universal/
#   scripts/release-macos.sh                        # signs when the environment can
#   scripts/release-macos.sh sign=yes               # and fails when it cannot
#
# build.sh builds and collects; this is the release-<platform>.sh its header
# names, the macOS half of a release as in the sibling repositories.
# release.yml's macOS job runs it on bin/ before the upload, so what
# build-release.sh zips is the library this script signed. It is macOS only --
# codesign and notarytool exist nowhere else -- which is why it has no .ps1.
#
# Only the dylib is signed. The static library is an archive of object files
# that nothing loads: a host's linker copies it into a binary of the host's own,
# which the host signs.
#
# What a user's Gatekeeper sees. A zip downloaded in a browser quarantines
# everything extracted from it, and Gatekeeper assesses quarantined code when it
# is loaded -- by an app that links the dylib, or by Python, .NET or Dart
# opening it through an FFI:
#
#   ad-hoc signed, all the linker gives it    refused
#   Developer ID signed, not notarized        refused
#   Developer ID signed and notarized         loads
#
# A bare dylib cannot be stapled -- the stapler takes .app, .dmg and .pkg -- so
# its ticket stays on Apple's servers, and Gatekeeper looks it up the first time
# it meets a quarantined copy. A Mac that is offline at that moment refuses the
# copy as it would an unnotarized one, until it can reach them. The SDK's dylibs
# are in the same position. An app that embeds the dylib signs it again with
# its own identity and is notarized as a whole, so none of this reaches its
# users.
#
# Environment, which release.yml fills from the organisation's secrets:
#
#   APPLE_SIGNING_IDENTITY  "Developer ID Application: ... (TEAMID)", looked up in
#                           the keychain search list -- an ephemeral keychain in
#                           CI, your login keychain locally
#   APPLE_API_KEY_PATH      the App Store Connect API key (.p8) notarytool uses
#   APPLE_API_KEY           its key ID
#   APPLE_API_ISSUER        its issuer ID
set -euo pipefail

APP="$(basename "$0")"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

LIB="$ROOT_DIR/bin/libs/macos/universal/libssplayer_wgpu.dylib"
INSTALL_NAME="@rpath/libssplayer_wgpu.dylib"
ARCHS="arm64 x86_64"

# --- options --------------------------------------------------------------
SIGN="auto"

usage () {
  echo "Usage: $APP [options]"
  echo "$APP options:"
  echo "  sign=<auto|yes|no>      Code signing and notarization (default: $SIGN). auto"
  echo "                          signs when APPLE_SIGNING_IDENTITY is set and notarizes"
  echo "                          when the APPLE_API_* credentials are set as well. yes"
  echo "                          requires all four and fails without them -- what"
  echo "                          release.yml passes on a release, since a missing"
  echo "                          credential otherwise looks exactly like signing that"
  echo "                          was never asked for. no ignores the APPLE_* variables."
  echo "                          Either way, whatever was attempted is checked, and a"
  echo "                          check that fails is fatal."
  echo "  -h | --help             Show this help"
  echo ""
  echo "Works on bin/libs/macos/universal/libssplayer_wgpu.dylib, in place, which"
  echo "scripts/build.sh build=release arch=universal produces. Without a Developer ID"
  echo "it is signed ad-hoc: it loads on the Mac that built it, and nowhere a download"
  echo "has quarantined it."
}

while [ $# -gt 0 ]; do
  item="$1"
  shift
  case "$item" in
    -h|--help|help) usage; exit 0 ;;
    *=*)
      key="${item%%=*}"
      value="$(printf '%s' "${item#*=}" | tr '[:upper:]' '[:lower:]')"
      case "$key" in
        sign) SIGN="$value" ;;
        *) echo "$APP: unknown option '$key' (see $APP --help)" >&2; exit 2 ;;
      esac
      ;;
    *) echo "$APP: unknown argument '$item' (options are key=value; see $APP --help)" >&2; exit 2 ;;
  esac
done

case "$SIGN" in auto|yes|no) ;; *) echo "$APP: sign must be auto, yes or no (got '$SIGN')" >&2; exit 2 ;; esac

# --- preflight ------------------------------------------------------------
if [ "$(uname -s)" != "Darwin" ]; then
  echo "$APP: macOS only -- codesign and notarytool exist nowhere else" >&2
  exit 1
fi
if [ ! -f "$LIB" ]; then
  echo "$APP: ${LIB#"$ROOT_DIR"/} is missing" >&2
  echo "$APP: build it first: scripts/build.sh build=release arch=universal" >&2
  exit 1
fi

if [ "$SIGN" = "no" ]; then
  unset APPLE_SIGNING_IDENTITY APPLE_API_KEY_PATH APPLE_API_KEY APPLE_API_ISSUER
fi
IDENTITY="${APPLE_SIGNING_IDENTITY:-}"

# Every credential at once, so a release that is missing two hears about both.
if [ "$SIGN" = "yes" ]; then
  missing=""
  for name in APPLE_SIGNING_IDENTITY APPLE_API_KEY_PATH APPLE_API_KEY APPLE_API_ISSUER; do
    [ -n "${!name:-}" ] || missing="$missing $name"
  done
  if [ -n "$missing" ]; then
    echo "$APP: sign=yes, and the environment has no$missing" >&2
    exit 1
  fi
fi

NOTARIZE="no"
if [ -n "$IDENTITY" ] && [ -n "${APPLE_API_KEY_PATH:-}" ] && [ -n "${APPLE_API_KEY:-}" ] && [ -n "${APPLE_API_ISSUER:-}" ]; then
  NOTARIZE="yes"
  if [ ! -f "$APPLE_API_KEY_PATH" ]; then
    echo "$APP: APPLE_API_KEY_PATH names $APPLE_API_KEY_PATH, which does not exist" >&2
    exit 1
  fi
fi

echo "options"
echo "  lib      => ${LIB#"$ROOT_DIR"/}"
echo "  sign     => $SIGN"
echo "  identity => ${IDENTITY:-(none: ad-hoc)}"
echo "  notarize => $NOTARIZE"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

step () { echo ""; echo ">> $*"; }

FAILED=""
fail () { FAILED="${FAILED}
  $1"; }
stop_on_failures () {
  if [ -n "$FAILED" ]; then
    echo "$APP: ${LIB#"$ROOT_DIR"/} is not right:$FAILED" >&2
    exit 1
  fi
}

# --- install name ---------------------------------------------------------
# cargo leaves the install name as the path the library was linked at, inside
# the build machine's target/, and a host that links the dylib records that
# path and looks there at run time. @rpath hands the choice to the host's own
# rpath. This goes first, because rewriting the Mach-O invalidates its
# signature, and the warning saying so is the one thing install_name_tool
# prints on success.
step "install name -> $INSTALL_NAME"
if ! out="$(install_name_tool -id "$INSTALL_NAME" "$LIB" 2>&1)"; then
  echo "$out" >&2
  exit 1
fi
present="$(lipo -archs "$LIB")"
for a in $ARCHS; do
  case " $present " in *" $a "*) ;; *) fail "it has no $a slice (it has: $present)" ;; esac
  id="$(otool -D -arch "$a" "$LIB" | sed -n '2p')"
  [ "$id" = "$INSTALL_NAME" ] || fail "its $a install name is '$id'"
done
stop_on_failures
echo "   $present, both $INSTALL_NAME"

# --- sign -----------------------------------------------------------------
if [ -n "$IDENTITY" ]; then
  step "signing as $IDENTITY"
  # The hardened runtime and a secure timestamp, as the sibling repositories
  # sign: notarization refuses a signature without the timestamp.
  codesign --force --sign "$IDENTITY" --options runtime --timestamp "$LIB"
else
  step "signing ad-hoc (no APPLE_SIGNING_IDENTITY)"
  # The install name broke the linker's ad-hoc signature, and Apple Silicon
  # loads no code without one.
  codesign --force --sign - "$LIB"
fi
# --verify checks every architecture of a universal file unless told otherwise.
codesign --verify --strict --verbose=2 "$LIB"

if [ -n "$IDENTITY" ]; then
  for a in $ARCHS; do
    info="$(codesign -dvv --architecture "$a" "$LIB" 2>&1)"
    grep -q '^Authority=Developer ID Application' <<< "$info" || fail "its $a slice is not signed by a Developer ID Application certificate"
    grep -q '^Timestamp=' <<< "$info" || fail "its $a slice's signature has no secure timestamp"
    grep -q '^CodeDirectory .*flags=[^ ]*runtime' <<< "$info" || fail "its $a slice's signature does not ask for the hardened runtime"
  done
  stop_on_failures
  echo "   $(codesign -dvv "$LIB" 2>&1 | sed -n 's/^Authority=\(Developer ID Application.*\)/\1/p'), on $present"
fi

# --- notarize -------------------------------------------------------------
if [ "$NOTARIZE" = "yes" ]; then
  step "notarizing"
  # notarytool takes a zip, and ditto is what makes one of a Mach-O as macOS
  # expects. The ticket is issued per architecture's code directory hash, so
  # the one submission covers both slices.
  ditto -c -k --keepParent "$LIB" "$WORK/notarize.zip"

  # `notarytool submit --wait` is not reliably non-zero on a rejected
  # submission, so the status it reports is what decides, as in SpriteStudio-SDK,
  # and a non-zero exit is kept rather than fatal until that has been read.
  # plutil parses the JSON: it ships with macOS, and jq does not.
  #
  # --timeout bounds the wait: a submission still in progress then ends here
  # with its status reported below, rather than holding the job until the
  # workflow's timeout kills it without the notary log.
  rc=0
  xcrun notarytool submit "$WORK/notarize.zip" \
    --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER" \
    --wait --timeout 60m --output-format json > "$WORK/notarize.json" || rc=$?
  cat "$WORK/notarize.json"; echo ""
  status="$(plutil -extract status raw -o - -- "$WORK/notarize.json" 2>/dev/null || true)"
  id="$(plutil -extract id raw -o - -- "$WORK/notarize.json" 2>/dev/null || true)"
  if [ "$status" != "Accepted" ]; then
    echo "$APP: notarization was not accepted (status ${status:-unknown}, notarytool exit $rc)" >&2
    if [ -n "$id" ]; then
      echo "$APP: the notary log for submission $id:" >&2
      xcrun notarytool log "$id" \
        --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER" >&2 || true
    fi
    exit 1
  fi
  echo "   accepted (submission $id)"

  # What Gatekeeper does on a user's Mac, asked the same way: look the ticket up
  # online by the code's hash. An accepted ticket is published within moments
  # but not always by the time notarytool returns, so it is asked a few times.
  step "checking the ticket is published"
  published="no"
  for attempt in 1 2 3 4 5 6; do
    if codesign --verify --check-notarization -R='notarized' "$LIB" 2> "$WORK/check.log"; then
      published="yes"
      break
    fi
    if [ "$attempt" -lt 6 ]; then sleep 10; fi
  done
  if [ "$published" != "yes" ]; then
    cat "$WORK/check.log" >&2
    echo "$APP: notarization was accepted, and Gatekeeper's online check still finds no ticket" >&2
    exit 1
  fi
  echo "   Gatekeeper finds it: the dylib loads where a download has quarantined it"
fi

echo ""
if [ "$NOTARIZE" = "yes" ]; then
  echo "$APP: done -> ${LIB#"$ROOT_DIR"/}, signed and notarized"
elif [ -n "$IDENTITY" ]; then
  echo "$APP: done -> ${LIB#"$ROOT_DIR"/}, signed and NOT notarized: Gatekeeper refuses it once a download has quarantined it"
else
  echo "$APP: done -> ${LIB#"$ROOT_DIR"/}, signed ad-hoc: it loads on this Mac, and nowhere a download has quarantined it"
fi
