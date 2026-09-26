#!/usr/bin/env pwsh
#
# Build the GitHub Pages site locally -- everything .github/workflows/pages.yml
# publishes, in the same order: the documentation in both locales into site/,
# the rustdoc grafted on at site/api/, and the WebGPU demo at site/demo/.
# PowerShell twin of build-pages.sh.
#
#   scripts/build-pages.ps1              # the tree pages.yml uploads
#   scripts/build-pages.ps1 serve=yes    # ... and serve it on :8000
#   scripts/build-pages.ps1 demo=no      # docs + rustdoc only, no wasm toolchain
#   scripts/build-pages.ps1 docs=no      # re-check one graft against the site as it is
#
# What this does NOT reproduce is the deploy job (actions/deploy-pages) -- there
# is no local equivalent of publishing. Rerunning is safe.
$ErrorActionPreference = "Stop"

$APP = Split-Path -Leaf $PSCommandPath
$ROOT_DIR = Split-Path -Parent (Split-Path -Parent $PSCommandPath)

$SITE_DIR = Join-Path $ROOT_DIR "site"
$API_DIR = Join-Path $SITE_DIR "api"
$DEMO_DIR = Join-Path $SITE_DIR "demo"

function Fail {
    param([string] $Message, [int] $Code = 1)
    [Console]::Error.WriteLine("${APP}: $Message")
    exit $Code
}

# --- options --------------------------------------------------------------
$opts = [ordered]@{
    prepare = "auto"
    docs    = "yes"
    api     = "yes"
    demo    = "yes"
    assets  = "auto"
    venv    = ".venv"
    serve   = "no"
    port    = "8000"
}

function Show-Usage {
    Write-Host "Usage: $APP [options]"
    Write-Host "$APP options:"
    Write-Host "  prepare=<auto|yes|no>   Run prepare-docs.ps1 first (default: auto)."
    Write-Host "                          auto = only when no zensical is found."
    Write-Host "  docs=<yes|no>           Build both documentation locales (default: yes)."
    Write-Host "                          docs=no keeps whatever is already in site/, which is"
    Write-Host "                          how you re-check only a graft."
    Write-Host "  api=<yes|no>            Build the rustdoc for ssplayer-wgpu and"
    Write-Host "                          ssplayer-bevy into site/api/ (default: yes)."
    Write-Host "  demo=<yes|no>           Build examples/web into site/demo/ (default: yes)."
    Write-Host "                          Needs the wasm32 target, wasm-pack and pnpm."
    Write-Host "  assets=<auto|yes|no>    Run deploy-examples.ps1 before the demo, which converts"
    Write-Host "                          the SDK's test projects into the .ssab the demo plays"
    Write-Host "                          (default: auto). auto = only when examples/web/assets"
    Write-Host "                          does not exist yet. Needs the SDK submodule."
    Write-Host "  venv=<dir>              Virtualenv for the docs toolchain (default: .venv)."
    Write-Host "  serve=<yes|no>          Serve site/ over HTTP when the build is done (default: no)."
    Write-Host "  port=<n>                Port for serve=yes (default: 8000)"
    Write-Host "  -h | --help             Show this help"
    Write-Host ""
    Write-Host "Output: site/ (en) + site/ja/ (ja) + site/api/ (rustdoc) + site/demo/ (WebGPU"
    Write-Host "demo) -- the exact tree pages.yml uploads. Both documentation builds are"
    Write-Host "--strict, so a broken internal link fails here rather than on a publish."
    Write-Host ""
    Write-Host "The rustdoc is deliberately built WITHOUT RUSTDOCFLAGS=-D warnings: a broken"
    Write-Host "intra-doc link should not stop a documentation publish."
}

foreach ($item in $args) {
    if ($item -in "-h", "--help", "help") {
        Show-Usage
        exit 0
    } elseif ($item -match "=") {
        $key, $value = $item -split "=", 2
        if (-not $opts.Contains($key)) {
            Fail "unknown option '$key' (see $APP --help)" 2
        }
        # venv= is a path; every other option is a keyword, so case-fold those.
        $opts[$key] = if ($key -eq "venv") { $value } else { $value.ToLower() }
    } else {
        Fail "unknown argument '$item' (options are key=value; see $APP --help)" 2
    }
}

if ($opts.prepare -notin @("auto", "yes", "no")) { Fail "prepare must be auto, yes or no (got '$($opts.prepare)')" 2 }
if ($opts.docs    -notin @("yes", "no"))         { Fail "docs must be yes or no (got '$($opts.docs)')" 2 }
if ($opts.api     -notin @("yes", "no"))         { Fail "api must be yes or no (got '$($opts.api)')" 2 }
if ($opts.demo    -notin @("yes", "no"))         { Fail "demo must be yes or no (got '$($opts.demo)')" 2 }
if ($opts.assets  -notin @("auto", "yes", "no")) { Fail "assets must be auto, yes or no (got '$($opts.assets)')" 2 }
if ($opts.serve   -notin @("yes", "no"))         { Fail "serve must be yes or no (got '$($opts.serve)')" 2 }
if ($opts.port -notmatch '^\d+$')                { Fail "port must be a number (got '$($opts.port)')" 2 }

Write-Host "options"
Write-Host "  prepare => $($opts.prepare)"
Write-Host "  docs    => $($opts.docs)"
Write-Host "  api     => $($opts.api)"
Write-Host "  demo    => $($opts.demo)"
Write-Host "  assets  => $($opts.assets)"
Write-Host "  serve   => $($opts.serve)"
Write-Host ""

# --- preflight ------------------------------------------------------------
if (-not (Test-Path (Join-Path $ROOT_DIR "mkdocs.base.yml"))) {
    Fail "mkdocs.base.yml not found -- is this the SSPlayerForWgpu repo?"
}

Push-Location $ROOT_DIR
try {
    function Step {
        param([string] $What, [scriptblock] $Body)
        Write-Host ""
        Write-Host ">> $What"
        & $Body
        if ($LASTEXITCODE -ne 0) { Fail "$What failed ($LASTEXITCODE)" $LASTEXITCODE }
    }

    function Test-Zensical {
        $venvDir = Join-Path $ROOT_DIR $opts.venv
        $venvBin = if (Test-Path (Join-Path $venvDir "Scripts")) {
            Join-Path $venvDir "Scripts"
        } else {
            Join-Path $venvDir "bin"
        }
        foreach ($candidate in @((Join-Path $venvBin "zensical.exe"), (Join-Path $venvBin "zensical"))) {
            if (Test-Path $candidate) { return $true }
        }
        return [bool](Get-Command zensical -ErrorAction SilentlyContinue)
    }

    $prepareScript = Join-Path $ROOT_DIR "scripts/prepare-docs.ps1"
    $buildDocsScript = Join-Path $ROOT_DIR "scripts/build-docs.ps1"
    $deployScript = Join-Path $ROOT_DIR "scripts/deploy-examples.ps1"
    $buildScript = Join-Path $ROOT_DIR "scripts/build.ps1"

    # --- documentation ----------------------------------------------------
    if ($opts.docs -eq "yes") {
        switch ($opts.prepare) {
            "yes" { Step "prepare-docs.ps1 venv=$($opts.venv)" { & $prepareScript "venv=$($opts.venv)" } }
            "auto" {
                if (Test-Zensical) {
                    Write-Host "${APP}: zensical already available; skipping prepare-docs.ps1 (prepare=yes forces it)"
                } else {
                    Step "prepare-docs.ps1 venv=$($opts.venv)" { & $prepareScript "venv=$($opts.venv)" }
                }
            }
            "no" { }
        }

        # Both locales, English first -- build-docs.ps1 owns that order, because
        # the English build clears site/ and the Japanese site lives inside it.
        Step "build-docs.ps1 venv=$($opts.venv)" { & $buildDocsScript "venv=$($opts.venv)" }
    } else {
        Write-Host ""
        Write-Host "${APP}: docs=no -- keeping whatever is already in site/"
        if (-not (Test-Path (Join-Path $SITE_DIR "index.html"))) {
            [Console]::Error.WriteLine("${APP}: ...and there is no site/index.html, so what this leaves is a bare graft")
        }
    }

    # --- rustdoc ----------------------------------------------------------
    # `cargo doc` renders into <target>/doc; the site wants it at site/api/. A
    # local CARGO_TARGET_DIR is honoured, because plenty of working trees set one
    # and the hardcoded `target/` pages.yml can rely on would then be empty.
    #
    # Two invocations, not one `-p a -p b`: `--all-features` is right for the
    # player (it is what documents the `ffi` and `web` faces) and wrong for the
    # adapter, which has none to turn on.
    if ($opts.api -eq "yes") {
        $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $ROOT_DIR "target" }
        $docDir = Join-Path $targetDir "doc"

        # Cleared first, because `cargo doc` only ever ADDS to this directory: a
        # crate that has since been renamed or removed would still be sitting
        # there from an earlier build, and the copy below would publish it. Costs
        # a re-render of two crates, not a rebuild -- the compiled artifacts it
        # reads live in target/debug.
        if (Test-Path $docDir) { Remove-Item -Recurse -Force $docDir }

        Step "cargo doc --no-deps --all-features -p ssplayer-wgpu" {
            & cargo doc --no-deps --all-features -p ssplayer-wgpu
        }
        Step "cargo doc --no-deps -p ssplayer-bevy" {
            & cargo doc --no-deps -p ssplayer-bevy
        }

        if (-not (Test-Path $docDir)) { Fail "cargo doc produced no $docDir" }

        Write-Host ""
        Write-Host ">> copying $docDir/ into site/api/"
        if (Test-Path $API_DIR) { Remove-Item -Recurse -Force $API_DIR }
        New-Item -ItemType Directory -Force -Path $API_DIR | Out-Null
        Copy-Item -Path (Join-Path $docDir "*") -Destination $API_DIR -Recurse -Force

        # `cargo doc` writes no index.html at the root of a workspace -- with
        # several crates there is no single page to land on. The nav entry in both
        # locale configs links straight to the player, and this covers anyone who
        # trims the URL back to /api/. Keep it identical to the one in pages.yml
        # and in the .sh twin.
        #
        # Only the two published crates are here. ssplayer-wgpu-ffi sets
        # `doc = false` (it is a staticlib/cdylib whose surface is the C header,
        # and it renames its lib to `ssplayer_wgpu`, so documenting it would
        # clobber the real one), and the viewers under examples/ are hosts, not
        # products.
        $landing = @'
<!doctype html>
<meta charset="utf-8">
<title>SpriteStudio Player for wgpu — Rust API</title>
<h1>SpriteStudio Player for wgpu — Rust API</h1>
<ul>
  <li><a href="ssplayer_wgpu/index.html">ssplayer_wgpu</a> — the player</li>
  <li><a href="ssplayer_bevy/index.html">ssplayer_bevy</a> — the Bevy adapter</li>
</ul>
'@
        # LF and a trailing newline, so the file matches the .sh twin's heredoc
        # byte for byte.
        [System.IO.File]::WriteAllText((Join-Path $API_DIR "index.html"), ($landing -replace "`r`n", "`n") + "`n")
    } else {
        Write-Host ""
        Write-Host "${APP}: api=no -- site/api is not part of this build"
    }

    # --- demo -------------------------------------------------------------
    # examples/web is a static page that imports the built npm package from
    # ./ssplayer-wgpu/ and its animation data from ./assets/ and ./assets_emb/,
    # all by relative path -- so the whole directory works unchanged under /demo/
    # on any host, including a fork's Pages and a local http.server.
    if ($opts.demo -eq "yes") {
        $needAssets = $opts.assets
        if ($needAssets -eq "auto") {
            if (Test-Path (Join-Path $ROOT_DIR "examples/web/assets")) {
                $needAssets = "no"
                Write-Host ""
                Write-Host "${APP}: examples/web/assets exists; skipping deploy-examples.ps1 (assets=yes forces it)"
            } else {
                $needAssets = "yes"
            }
        }

        # Builds ssconverter-cli from the SDK submodule and converts its test
        # projects into .ssab, in BOTH export modes, deploying them into
        # examples/web.
        if ($needAssets -eq "yes") {
            Step "deploy-examples.ps1" { & $deployScript }
        }

        # wasm-pack + tsdown, then deploy=yes stages the finished npm package into
        # examples/web/ssplayer-wgpu -- so the demo runs the package as published,
        # not a second copy of the js layer that could drift from it.
        Step "build.ps1 platform=web build=release" { & $buildScript platform=web build=release }

        Write-Host ""
        Write-Host ">> copying examples/web/ into site/demo/"
        if (Test-Path $DEMO_DIR) { Remove-Item -Recurse -Force $DEMO_DIR }
        New-Item -ItemType Directory -Force -Path $DEMO_DIR | Out-Null
        Copy-Item -Path (Join-Path $ROOT_DIR "examples/web/*") -Destination $DEMO_DIR -Recurse -Force
        # Repository furniture, not part of the published demo.
        foreach ($junk in @("README.md", ".gitignore")) {
            $path = Join-Path $DEMO_DIR $junk
            if (Test-Path $path) { Remove-Item -Force $path }
        }
    } else {
        Write-Host ""
        Write-Host "${APP}: demo=no -- site/demo is not part of this build"
    }
} finally {
    Pop-Location
}

# --- what landed ----------------------------------------------------------
Write-Host ""
Write-Host "${APP}: done -> site/"
function Report {
    param([string] $Path, [string] $Label)
    if (Test-Path (Join-Path $ROOT_DIR $Path)) {
        Write-Host "  ${Label}: $Path"
    } else {
        Write-Host "  ${Label}: MISSING ($Path)"
    }
}
Report "site/index.html" "en  "
Report "site/ja/index.html" "ja  "
Report "site/api/ssplayer_wgpu/index.html" "api "
Report "site/demo/index.html" "demo"

# --- serve ----------------------------------------------------------------
# py first: it is the Windows launcher, and what build-docs.ps1 already tells
# people to serve with. python/python3 are the fallbacks elsewhere -- on Windows
# a bare `python` can be the Store stub that does nothing.
$PYTHON_CMD = $null
foreach ($candidate in @("py", "python", "python3")) {
    if (Get-Command $candidate -ErrorAction SilentlyContinue) { $PYTHON_CMD = $candidate; break }
}

Write-Host ""
if ($opts.serve -eq "yes") {
    if (-not $PYTHON_CMD) {
        Fail "no python on PATH to serve with -- use any static server on site/"
    }
    Write-Host "${APP}: http://localhost:$($opts.port)/       (English)"
    Write-Host "${APP}: http://localhost:$($opts.port)/ja/    (Japanese)"
    Write-Host "${APP}: http://localhost:$($opts.port)/api/   (Rust API)"
    Write-Host "${APP}: http://localhost:$($opts.port)/demo/  (WebGPU demo -- needs a WebGPU browser)"
    Write-Host "${APP}: Ctrl-C to stop"
    Write-Host ""
    Push-Location $ROOT_DIR
    try {
        & $PYTHON_CMD -m http.server $opts.port -d site
    } finally {
        Pop-Location
    }
    exit $LASTEXITCODE
} else {
    $shown = if ($PYTHON_CMD) { $PYTHON_CMD } else { "python" }
    Write-Host "${APP}: serve it (one server over the whole tree, so every link resolves):"
    Write-Host "${APP}:   $APP serve=yes"
    Write-Host "${APP}:   $shown -m http.server $($opts.port) -d site"
}
