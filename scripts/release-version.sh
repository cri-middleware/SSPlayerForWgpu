#!/usr/bin/env bash
#
# Print the player's version -- what a release is tagged as.
#
#   scripts/release-version.sh          # 7.0.0-beta.1
#   v$(scripts/release-version.sh)      # v7.0.0-beta.1  -- the git tag
#
# It is `[workspace.package] version` in Cargo.toml. ssplayer-wgpu and
# ssplayer-wgpu-ffi inherit it, and packages/ssplayer-wgpu/package.json carries
# the copy npm needs; this prints nothing but an error while either crate
# declares a version of its own or the copy disagrees. ssplayer-bevy is
# versioned on its own and is not read.
#
# build-release.sh refuses a tag that disagrees, so a Release called v1.2.3
# cannot hold an npm package that says 1.2.4.
set -euo pipefail

APP="$(basename "$0")"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

if [ "$#" -gt 0 ]; then
  echo "$APP: takes no arguments (got '$1')" >&2
  exit 2
fi

# The `version` line of one [section] of a manifest, whitespace and comment removed.
version_line () {
  awk -v want="[$2]" '
    /^[[:space:]]*\[/ { line = $0; gsub(/[[:space:]]/, "", line); here = (line == want); next }
    here && /^[[:space:]]*version[[:space:]]*[.=]/ {
      sub(/[[:space:]]*#.*$/, ""); gsub(/[[:space:]]/, ""); print; exit
    }
  ' "$1"
}

VERSION="$(version_line "$ROOT_DIR/Cargo.toml" workspace.package)"
VERSION="${VERSION#version=}"
VERSION="${VERSION//\"/}"
VERSION="${VERSION//\'/}"
if [ -z "$VERSION" ]; then
  echo "$APP: Cargo.toml has no [workspace.package] version" >&2
  exit 1
fi

for crate in ssplayer-wgpu ssplayer-wgpu-ffi; do
  line="$(version_line "$ROOT_DIR/libs/$crate/Cargo.toml" package)"
  if [ "$line" != "version.workspace=true" ]; then
    echo "$APP: libs/$crate/Cargo.toml has '$line' -- it inherits the player's version (version.workspace = true)" >&2
    exit 1
  fi
done

if ! command -v node > /dev/null 2>&1; then
  echo "$APP: node not found -- it is what reads packages/ssplayer-wgpu/package.json" >&2
  exit 1
fi
NPM_VERSION="$(cd "$ROOT_DIR/packages/ssplayer-wgpu" && node -p "require('./package.json').version || ''")"
if [ "$NPM_VERSION" != "$VERSION" ]; then
  echo "$APP: packages/ssplayer-wgpu/package.json says '$NPM_VERSION', Cargo.toml says '$VERSION' -- bump them together" >&2
  exit 1
fi

printf '%s\n' "$VERSION"
