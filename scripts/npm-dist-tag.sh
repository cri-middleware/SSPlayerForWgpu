#!/usr/bin/env bash
#
# Print the npm dist-tag a package version is published under.
#
#   scripts/npm-dist-tag.sh <package> <version>    # latest, beta, rc, ...
#
# A full release takes `latest`. So does a prerelease while the package's
# `latest` is a prerelease too, or the package is not on npm yet: a bare
# `npm install` then gets the newest version of a line with no full release.
# Once `latest` is a full release, a prerelease takes the word after its `-`
# (`rc` for 7.1.0-rc.2) and leaves `latest` alone.
#
# Any npm error but the package not existing stops it, since reading one as
# "not on npm" would put a prerelease on `latest` over a full release.
set -euo pipefail

APP="$(basename "$0")"

if [ "$#" -ne 2 ] || [ -z "$1" ] || [ -z "$2" ]; then
  echo "$APP: takes <package> <version>" >&2
  exit 2
fi
package="$1"
version="$2"

case "$version" in
  *-*) ;;
  *) echo latest; exit 0 ;;
esac

err="$(mktemp)"
trap 'rm -f "$err"' EXIT
if ! current="$(npm view "$package" dist-tags.latest 2>"$err")"; then
  grep -q E404 "$err" || { cat "$err" >&2; exit 1; }
  current=""
fi

case "$current" in
  "" | *-*) echo latest ;;
  *) word="${version#*-}"; echo "${word%%.*}" ;;
esac
