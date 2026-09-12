#!/bin/bash
#
# Regenerate THIRD_PARTY_NOTICES.md from the dependency tree.
#
# Unlike the SDK's script of the same name — whose notices are throwaway build
# artifacts bundled into release packages — this one writes a file that IS
# committed, because that is what the sibling Players' READMEs link to.
#
# Usage:
#   scripts/generate-licenses.sh              # -> THIRD_PARTY_NOTICES.md
#   scripts/generate-licenses.sh out=/tmp/x   # somewhere else
#   scripts/generate-licenses.sh --help

set -e

BASEDIR=$(cd "$(dirname "$0")" && pwd -P)
ROOTDIR=$(cd "${BASEDIR}/.." && pwd -P)

OUT="${ROOTDIR}/THIRD_PARTY_NOTICES.md"

for arg in "$@"; do
    case "${arg}" in
        --help|-h)
            sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        out=*) OUT="${arg#out=}" ;;
        *) echo "$(basename "$0"): unknown option '${arg}' (try --help)" >&2; exit 2 ;;
    esac
done

cd "${ROOTDIR}"

if ! command -v cargo-about &> /dev/null; then
    echo "cargo-about is not installed. Installing it now..."
    cargo install cargo-about --locked --features cli
fi

# The union of the three shipped faces, so one file covers every artifact:
#   embedded — the `image` decoder the Rust library and the C face carry
#   ffi      — the C face (bin/libssplayer_wgpu.*)
#   web      — the wasm behind @spritestudio/ssplayer-wgpu
# The Bevy adapter is a separate crate and an app's own dependency, so it is
# deliberately not folded in here.
echo "Generating ${OUT}..."
cargo about generate \
    --manifest-path libs/ssplayer-wgpu/Cargo.toml \
    --config about.toml \
    --features embedded,ffi,web \
    about-markdown.hbs \
    -o "${OUT}"

echo "Done. Review the diff before committing."
