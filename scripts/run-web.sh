#!/usr/bin/env bash
#
# Take a fresh checkout to the browser demo: scripts/run-example.sh example=web.
set -euo pipefail
exec "$(dirname "$0")/run-example.sh" example=web "$@"
