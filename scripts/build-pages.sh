#!/usr/bin/env bash
#
# Build the GitHub Pages site locally -- everything .github/workflows/pages.yml
# publishes, in the same order: the documentation in both locales into site/,
# then the workspace rustdoc into site/api/, then the WebGPU demo into site/demo/.
#
#   scripts/build-pages.sh                  # docs + rustdoc + demo
#   scripts/build-pages.sh api=no demo=no   # documentation only, the fast loop
#   scripts/build-pages.sh docs=no          # re-export the demo/rustdoc onto the site
#   scripts/build-pages.sh serve=yes        # ... and serve the finished tree on :8000
#
# Three trees, three toolchains, one output directory. They are assembled in this
# order because the ENGLISH DOCS BUILD CLEARS site/, and both the Japanese site
# (site/ja) and the two grafts (site/api, site/demo) live inside it. Any other
# order publishes a site that is missing one of them.
#
#   site/         zensical, docs/en          (English)
#   site/ja/      zensical, docs/ja          (Japanese)
#   site/api/     cargo doc                  (the Rust API of both published crates)
#   site/demo/    build.sh platform=web      (examples/web, running the npm package)
#
# `build-pages.sh` means "build what pages.yml publishes" in every repository in
# the SpriteStudio family. What it does NOT reproduce is the deploy job
# (actions/deploy-pages) -- there is no local equivalent of publishing.
#
# The demo is the expensive half: it needs the SDK submodule (for the converter
# that produces the animation data), the wasm32 target, wasm-pack and pnpm. Use
# demo=no while writing prose.
set -euo pipefail

APP="$(basename "$0")"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SITE_DIR="$ROOT_DIR/site"
API_DIR="$SITE_DIR/api"
DEMO_DIR="$SITE_DIR/demo"

# --- options --------------------------------------------------------------
PREPARE="auto"
DOCS="yes"
API="yes"
DEMO="yes"
ASSETS="auto"
VENV=".venv"
SERVE="no"
PORT="8000"

usage () {
  echo "Usage: $APP [options]"
  echo "$APP options:"
  echo "  prepare=<auto|yes|no>   Run prepare-docs.sh first (default: $PREPARE)."
  echo "                          auto = only when no zensical is found."
  echo "  docs=<yes|no>           Build both documentation locales (default: $DOCS)."
  echo "                          docs=no keeps whatever is already in site/, which is"
  echo "                          how you re-check only a graft."
  echo "  api=<yes|no>            Build the rustdoc for ssplayer-wgpu and"
  echo "                          ssplayer-bevy into site/api/ (default: $API)."
  echo "  demo=<yes|no>           Build examples/web into site/demo/ (default: $DEMO)."
  echo "                          Needs the wasm32 target, wasm-pack and pnpm."
  echo "  assets=<auto|yes|no>    Run deploy-examples.sh before the demo, which converts"
  echo "                          the SDK's test projects into the .ssab the demo plays"
  echo "                          (default: $ASSETS). auto = only when that data is missing"
  echo "                          or another SDK commit converted it (deploy-examples.sh"
  echo "                          when=stale). Needs the SDK submodule."
  echo "  venv=<dir>              Virtualenv for the docs toolchain (default: $VENV)."
  echo "  serve=<yes|no>          Serve site/ over HTTP when the build is done (default: $SERVE)."
  echo "  port=<n>                Port for serve=yes (default: $PORT)"
  echo "  -h | --help             Show this help"
  echo ""
  echo "Output: site/ (en) + site/ja/ (ja) + site/api/ (rustdoc) + site/demo/ (WebGPU"
  echo "demo) -- the exact tree pages.yml uploads. Both documentation builds are"
  echo "--strict, so a broken internal link fails here rather than on a publish."
  echo ""
  echo "The rustdoc is deliberately built WITHOUT RUSTDOCFLAGS=-D warnings: a broken"
  echo "intra-doc link should not stop a documentation publish."
}

while [ $# -gt 0 ]; do
  item="$1"
  shift

  case "$item" in
    -h|--help|help)
      usage
      exit 0
      ;;
    *=*)
      key="${item%%=*}"
      value="$(printf '%s' "${item#*=}" | tr '[:upper:]' '[:lower:]')"
      case "$key" in
        prepare) PREPARE="$value" ;;
        docs)    DOCS="$value" ;;
        api)     API="$value" ;;
        demo)    DEMO="$value" ;;
        assets)  ASSETS="$value" ;;
        venv)    VENV="${item#*=}" ;;
        serve)   SERVE="$value" ;;
        port)    PORT="$value" ;;
        *)
          echo "$APP: unknown option '$key' (see $APP --help)" >&2
          exit 2
          ;;
      esac
      ;;
    *)
      echo "$APP: unknown argument '$item' (options are key=value; see $APP --help)" >&2
      exit 2
      ;;
  esac
done

case "$PREPARE" in auto|yes|no) ;; *) echo "$APP: prepare must be auto, yes or no (got '$PREPARE')" >&2; exit 2 ;; esac
case "$DOCS"    in yes|no) ;;      *) echo "$APP: docs must be yes or no (got '$DOCS')" >&2; exit 2 ;; esac
case "$API"     in yes|no) ;;      *) echo "$APP: api must be yes or no (got '$API')" >&2; exit 2 ;; esac
case "$DEMO"    in yes|no) ;;      *) echo "$APP: demo must be yes or no (got '$DEMO')" >&2; exit 2 ;; esac
case "$ASSETS"  in auto|yes|no) ;; *) echo "$APP: assets must be auto, yes or no (got '$ASSETS')" >&2; exit 2 ;; esac
case "$SERVE"   in yes|no) ;;      *) echo "$APP: serve must be yes or no (got '$SERVE')" >&2; exit 2 ;; esac
case "$PORT" in ''|*[!0-9]*) echo "$APP: port must be a number (got '$PORT')" >&2; exit 2 ;; esac

echo "options"
echo "  prepare => $PREPARE"
echo "  docs    => $DOCS"
echo "  api     => $API"
echo "  demo    => $DEMO"
echo "  assets  => $ASSETS"
echo "  serve   => $SERVE"
echo ""

# --- preflight ------------------------------------------------------------
if [ ! -f "$ROOT_DIR/mkdocs.base.yml" ]; then
  echo "$APP: mkdocs.base.yml not found -- is this the SSPlayerForWgpu repo?" >&2
  exit 1
fi
cd "$ROOT_DIR"

step () {
  echo ""
  echo ">> $*"
  "$@"
}

have_zensical () {
  local venv_bin="$ROOT_DIR/$VENV/bin"
  [ -d "$ROOT_DIR/$VENV/Scripts" ] && venv_bin="$ROOT_DIR/$VENV/Scripts"
  [ -x "$venv_bin/zensical" ] || [ -x "$venv_bin/zensical.exe" ] \
    || command -v zensical > /dev/null 2>&1
}

# --- documentation --------------------------------------------------------
if [ "$DOCS" = "yes" ]; then
  case "$PREPARE" in
    yes) step ./scripts/prepare-docs.sh venv="$VENV" ;;
    auto)
      if have_zensical; then
        echo "$APP: zensical already available; skipping prepare-docs.sh (prepare=yes forces it)"
      else
        step ./scripts/prepare-docs.sh venv="$VENV"
      fi
      ;;
    no) ;;
  esac

  # Both locales, English first -- build-docs.sh owns that order, because the
  # English build clears site/ and the Japanese site lives inside it.
  step ./scripts/build-docs.sh venv="$VENV"
else
  echo ""
  echo "$APP: docs=no -- keeping whatever is already in site/"
  if [ ! -f "$SITE_DIR/index.html" ]; then
    echo "$APP: ...and there is no site/index.html, so what this leaves is a bare graft" >&2
  fi
fi

# --- rustdoc --------------------------------------------------------------
# `cargo doc` renders into <target>/doc; the site wants it at site/api/. A local
# CARGO_TARGET_DIR is honoured, because plenty of working trees set one and the
# hardcoded `target/` pages.yml can rely on would then be empty.
#
# Two invocations, not one `-p a -p b`: `--all-features` is right for the player
# (it is what documents the `ffi` and `web` faces) and wrong for the adapter,
# which has none to turn on.
build_api () {
  local doc_dir="${CARGO_TARGET_DIR:-$ROOT_DIR/target}/doc"

  # Cleared first, because `cargo doc` only ever ADDS to this directory: a crate
  # that has since been renamed or removed would still be sitting there from an
  # earlier build, and the copy below would publish it. Costs a re-render of two
  # crates, not a rebuild -- the compiled artifacts it reads live in target/debug.
  rm -rf "$doc_dir"

  step cargo doc --no-deps --all-features -p ssplayer-wgpu
  step cargo doc --no-deps -p ssplayer-bevy

  if [ ! -d "$doc_dir" ]; then
    echo "$APP: cargo doc produced no $doc_dir" >&2
    exit 1
  fi

  echo ""
  echo ">> copying $doc_dir/ into site/api/"
  rm -rf "$API_DIR"
  mkdir -p "$API_DIR"
  cp -R "$doc_dir/." "$API_DIR/"

  # `cargo doc` writes no index.html at the root of a workspace -- with several
  # crates there is no single page to land on. The nav entry in both locale
  # configs links straight to the player, and this covers anyone who trims the
  # URL back to /api/. Keep it identical to the one in the .ps1 twin.
  #
  # Only the two published crates are here. ssplayer-wgpu-ffi sets `doc = false`
  # (it is a staticlib/cdylib whose surface is the C header, and it renames its
  # lib to `ssplayer_wgpu`, so documenting it would clobber the real one), and
  # the viewers under examples/ are hosts, not products.
  cat > "$API_DIR/index.html" <<'HTML'
<!doctype html>
<meta charset="utf-8">
<title>SpriteStudio Player for wgpu — Rust API</title>
<h1>SpriteStudio Player for wgpu — Rust API</h1>
<ul>
  <li><a href="ssplayer_wgpu/index.html">ssplayer_wgpu</a> — the player</li>
  <li><a href="ssplayer_bevy/index.html">ssplayer_bevy</a> — the Bevy adapter</li>
</ul>
HTML
}

if [ "$API" = "yes" ]; then
  build_api
else
  echo ""
  echo "$APP: api=no -- site/api is not part of this build"
fi

# add_demo_links <dir> <path from its pages to the site root>
# Puts a fixed "Powered by" box, linking to the docs and the repository, into
# every *.html directly in <dir>, ahead of the last </body>.
add_demo_links () {
  local dir="$1" root="$2" page
  [ -d "$dir" ] || return 0
  local a='target="_blank" rel="noopener" style="color:#fff"'
  local links='<div id="ss-demo-links" style="position:fixed;left:12px;bottom:12px;z-index:2147483647;padding:4px 10px;border-radius:6px;background:rgba(0,0,0,.65);color:#fff;font:12px/1.6 system-ui,sans-serif">'
  links+="Powered by SpriteStudio Player for wgpu &middot; <a href=\"$root\" $a>Docs</a>"
  links+=" &middot; <a href=\"https://github.com/cri-middleware/SSPlayerForWgpu\" $a>GitHub</a></div>"
  for page in "$dir"/*.html; do
    [ -f "$page" ] || continue
    grep -q 'id="ss-demo-links"' "$page" && continue
    awk -v links="$links" '
      { line[NR] = $0 }
      END {
        for (at = NR; at > 0; at--) if (tolower(line[at]) ~ /<\/body>/) break
        for (i = 1; i <= NR; i++) {
          if (i != at) { print line[i]; continue }
          lc = tolower(line[i]); j = 0
          while ((k = index(substr(lc, j + 1), "</body>")) > 0) j += k
          print substr(line[i], 1, j - 1) links substr(line[i], j)
        }
        if (at == 0) print links
      }' "$page" > "$page.tmp"
    mv "$page.tmp" "$page"
  done
  echo "   links to the docs and the repository -> site/${dir#"$SITE_DIR"/}/*.html"
}

# --- demo -----------------------------------------------------------------
# examples/web is a static page that imports the built npm package from
# ./ssplayer-wgpu/ and its animation data from ./assets/ and ./assets_emb/, all
# by relative path -- so the whole directory works unchanged under /demo/ on any
# host, including a fork's Pages and a local http.server.
build_demo () {
  # Builds ssconverter-cli from the SDK submodule and converts its test projects
  # into .ssab, in BOTH export modes, deploying them into examples/web. auto is
  # its when=stale: the wasm below always comes from the SDK checked out now, and
  # data that another SDK commit converted can make it panic.
  case "$ASSETS" in
    yes)  step ./scripts/deploy-examples.sh ;;
    auto) step ./scripts/deploy-examples.sh when=stale ;;
  esac

  # wasm-pack + tsdown, then deploy=yes stages the finished npm package into
  # examples/web/ssplayer-wgpu -- so the demo runs the package as published,
  # not a second copy of the js layer that could drift from it.
  step ./scripts/build.sh platform=web build=release

  echo ""
  echo ">> copying examples/web/ into site/demo/"
  rm -rf "$DEMO_DIR"
  mkdir -p "$DEMO_DIR"
  cp -R "$ROOT_DIR/examples/web/." "$DEMO_DIR/"
  # Repository furniture, not part of the published demo.
  rm -f "$DEMO_DIR/README.md" "$DEMO_DIR/.gitignore"
  add_demo_links "$DEMO_DIR" "../"
}

if [ "$DEMO" = "yes" ]; then
  build_demo
else
  echo ""
  echo "$APP: demo=no -- site/demo is not part of this build"
fi

# --- what landed ----------------------------------------------------------
echo ""
echo "$APP: done -> site/"
report () { # <path> <label>
  if [ -e "$ROOT_DIR/$1" ]; then
    echo "  $2: $1"
  else
    echo "  $2: MISSING ($1)"
  fi
}
report "site/index.html" "en  "
report "site/ja/index.html" "ja  "
report "site/api/ssplayer_wgpu/index.html" "api "
report "site/demo/index.html" "demo"

# --- serve ----------------------------------------------------------------
# Bound to loopback, in the command it suggests as well. On every interface,
# http.server announces http://[::]:PORT/, and neither [::] nor 0.0.0.0 is a
# secure context in Chrome: opened there, the demo has no navigator.gpu at all.
PYTHON_CMD="python3"
command -v "$PYTHON_CMD" > /dev/null 2>&1 || PYTHON_CMD="python"

echo ""
if [ "$SERVE" = "yes" ]; then
  command -v "$PYTHON_CMD" > /dev/null 2>&1 || {
    echo "$APP: no python on PATH to serve with -- use any static server on site/" >&2
    exit 1
  }
  echo "$APP: http://localhost:$PORT/       (English)"
  echo "$APP: http://localhost:$PORT/ja/    (Japanese)"
  echo "$APP: http://localhost:$PORT/api/   (Rust API)"
  echo "$APP: http://localhost:$PORT/demo/  (WebGPU demo -- needs a WebGPU browser)"
  echo "$APP: Ctrl-C to stop"
  echo ""
  exec "$PYTHON_CMD" -m http.server "$PORT" --bind 127.0.0.1 -d site
else
  echo "$APP: serve it (one server over the whole tree, so every link resolves):"
  echo "$APP:   $APP serve=yes"
  echo "$APP:   $PYTHON_CMD -m http.server $PORT --bind 127.0.0.1 -d site"
fi
