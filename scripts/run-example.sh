#!/usr/bin/env bash
#
# Take a fresh checkout to a running example.
#
#   scripts/run-example.sh                  # examples/hello: Ringo, the first frame
#   scripts/run-example.sh example=viewer   # the development viewer
#   scripts/run-example.sh example=web      # the browser demo (also scripts/run-web.sh)
#
# example=<hello|viewer|viewer-bevy|overall-bevy|web>, hello by default. In order:
#
#   1. converts the SDK's test projects if assets/ has not been generated
#      (scripts/deploy-examples.sh)
#   2. web only: builds the npm package into examples/web if it is not there
#      (scripts/build.sh platform=web)
#   3. runs the example: cargo run for the native ones; for web, serves
#      examples/web on http://localhost:8000 and opens it (Ctrl-C stops the server)
#
# Each step runs only when its output is missing, and nothing checks whether an
# existing one is stale: after changing the Rust, run build.sh platform=web yourself.
# The SDK submodule has to be checked out (git submodule update --init).
set -euo pipefail

APP="$(basename "$0")"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WEB_DIR="$ROOT_DIR/examples/web"
PORT=8000

EXAMPLE="hello"
for arg in "$@"; do
  case "$arg" in
    example=*) EXAMPLE="${arg#example=}" ;;
    -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "$APP: unknown argument '$arg' (see $APP --help)" >&2; exit 2 ;;
  esac
done

case "$EXAMPLE" in
  hello)        CRATE="ssplayer-wgpu-hello" ;;
  viewer)       CRATE="ssplayer-wgpu-viewer" ;;
  viewer-bevy)  CRATE="ssplayer-bevy-viewer" ;;
  overall-bevy) CRATE="ssplayer-bevy-overall" ;;
  web)          CRATE="" ;;
  *) echo "$APP: example must be hello, viewer, viewer-bevy, overall-bevy or web (got '$EXAMPLE')" >&2; exit 2 ;;
esac

# --- 1. the playback data -------------------------------------------------
# examples/web/assets is deploy-examples.sh's output too, so one marker per side.
if [ ! -f "$ROOT_DIR/assets/Ringo/Ringo.ssab" ] || { [ "$EXAMPLE" = "web" ] && [ ! -f "$WEB_DIR/assets/Ringo/Ringo.ssab" ]; }; then
  echo ">> $APP: converting the SDK's test projects (scripts/deploy-examples.sh)"
  "$ROOT_DIR/scripts/deploy-examples.sh"
fi

# --- 2 and 3 --------------------------------------------------------------
if [ -n "$CRATE" ]; then
  echo ">> $APP: cargo run -p $CRATE"
  cd "$ROOT_DIR"
  exec cargo run -p "$CRATE"
fi

if [ ! -f "$WEB_DIR/ssplayer-wgpu/wasm/ssplayer_wgpu_bg.wasm" ]; then
  echo ">> $APP: building the npm package into examples/web (scripts/build.sh platform=web)"
  "$ROOT_DIR/scripts/build.sh" platform=web
fi

command -v python3 > /dev/null 2>&1 || { echo "$APP: python3 not found -- it serves examples/web" >&2; exit 1; }
URL="http://localhost:$PORT/"
echo ">> $APP: serving examples/web on $URL (Ctrl-C to stop) -- it needs a WebGPU browser"
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$WEB_DIR" &
SERVER=$!
trap 'kill "$SERVER" 2> /dev/null || true' EXIT INT TERM
sleep 1
kill -0 "$SERVER" 2> /dev/null || { echo "$APP: the server did not start -- is port $PORT taken?" >&2; exit 1; }
if command -v open > /dev/null 2>&1; then
  open "$URL"
elif command -v xdg-open > /dev/null 2>&1; then
  xdg-open "$URL" > /dev/null 2>&1 || true
else
  echo ">> $APP: open $URL in a browser"
fi
wait "$SERVER"
