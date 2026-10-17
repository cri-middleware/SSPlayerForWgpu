#!/usr/bin/env bash
#
# Print the npm dist-tag a package version is published under.
#
#   scripts/npm-dist-tag.sh <package> <version>    # latest, latest-7.0, beta, rc, ...
#
# A full release takes `latest`, unless npm's `latest` is a newer full release:
# a patch to an earlier line then takes `latest-<major>.<minor>`, as npm's own
# `latest-6` does, so a bare `npm install` keeps getting the newest. A
# prerelease takes `latest` too while the package's `latest` is a prerelease, or
# the package is not on npm yet: a bare `npm install` then gets the newest
# version of a line with no full release. Once `latest` is a full release, a
# prerelease takes the word after its `-` (`rc` for 7.1.0-rc.2) and leaves
# `latest` alone.
#
# Any npm error but the package not existing stops it, since reading one as
# "not on npm" would put a version on `latest` over a newer one.
set -euo pipefail

APP="$(basename "$0")"

if [ "$#" -ne 2 ] || [ -z "$1" ] || [ -z "$2" ]; then
  echo "$APP: takes <package> <version>" >&2
  exit 2
fi
package="$1"
version="$2"

err="$(mktemp)"
trap 'rm -f "$err"' EXIT
if ! current="$(npm view "$package" dist-tags.latest 2>"$err")"; then
  grep -q E404 "$err" || { cat "$err" >&2; exit 1; }
  current=""
fi

case "$version" in
  *-*)
    case "$current" in
      "" | *-*) echo latest ;;
      *) word="${version#*-}"; echo "${word%%.*}" ;;
    esac
    ;;
  *)
    case "$current" in
      "" | *-* | "$version") echo latest ;;
      *)
        newest="$(printf '%s\n%s\n' "$current" "$version" | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)"
        if [ "$newest" = "$version" ]; then
          echo latest
        else
          echo "latest-${version%.*}"
        fi
        ;;
    esac
    ;;
esac
