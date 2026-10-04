#!/usr/bin/env pwsh
#
# Build the documentation site -- one run per locale, in the order that works.
# PowerShell twin of build-docs.sh.
#
#   scripts/build-docs.ps1              # English then Japanese, both strict
#   scripts/build-docs.ps1 locale=ja    # Japanese only, into site/ja
#   scripts/build-docs.ps1 serve=yes    # ... and serve the finished tree on :8000
#
# Two things this script exists to get right:
#
#   1. Order. The Japanese site is written *inside* the English one (site/ja),
#      and a build clears its own output directory first. English first,
#      Japanese second; the other order publishes a site with no Japanese pages.
#   2. --strict, always. Nothing builds the docs on a pull request, so this is
#      the only thing standing between a broken internal link and production.
#      mkdocs.base.yml sets strict: true as well; the flag is spelled out
#      here so the build stays strict if the config ever loses it.
$ErrorActionPreference = "Stop"

$APP = Split-Path -Leaf $PSCommandPath
$ROOT_DIR = Split-Path -Parent (Split-Path -Parent $PSCommandPath)

function Fail {
    param([string] $Message, [int] $Code = 1)
    [Console]::Error.WriteLine("${APP}: $Message")
    exit $Code
}

# --- options --------------------------------------------------------------
$opts = [ordered]@{
    locale = "all"
    venv   = ".venv"
    serve  = "no"
    port   = "8000"
}

function Show-Usage {
    Write-Host "Usage: $APP [options]"
    Write-Host "$APP options:"
    Write-Host "  locale=<all|en|ja>      Which locale to build (default: all)"
    Write-Host "                          all = English then Japanese, the only safe order"
    Write-Host "  venv=<dir>              Virtualenv to run zensical from, relative to the repo"
    Write-Host "                          root (default: .venv). Falls back to zensical on PATH."
    Write-Host "  serve=<yes|no>          Serve site/ over HTTP when the build is done (default: no)"
    Write-Host "  port=<n>                Port for serve=yes (default: 8000)"
    Write-Host "  -h | --help             Show this help"
    Write-Host ""
    Write-Host "Output: site/ (English) and site/ja/ (Japanese) -- the exact tree pages.yml"
    Write-Host "uploads. Both builds are --strict."
    Write-Host ""
    Write-Host "locale=en on its own DELETES site/ja. Use locale=all before publishing."
    Write-Host ""
    Write-Host "Set the venv up first with scripts/prepare-docs.ps1."
}

foreach ($item in $args) {
    if ($item -in "-h", "--help", "help") {
        Show-Usage
        exit 0
    } elseif ($item -match "=") {
        $key, $value = $item -split "=", 2
        if (-not $opts.Contains($key)) { Fail "unknown option '$key' (see $APP --help)" 2 }
        $opts[$key] = $value
    } else {
        Fail "unknown argument '$item' (options are key=value; see $APP --help)" 2
    }
}

$LOCALE = $opts["locale"].ToLower()
switch ($LOCALE) {
    { $_ -in "all", "both" }    { $LOCALE = "all" }
    { $_ -in "en", "english" }  { $LOCALE = "en" }
    { $_ -in "ja", "japanese" } { $LOCALE = "ja" }
    default { Fail "locale must be all, en or ja (got '$LOCALE')" 2 }
}
$VENV = $opts["venv"]
$SERVE = $opts["serve"].ToLower()
if ($SERVE -notin "yes", "no") { Fail "serve must be yes or no (got '$SERVE')" 2 }
$PORT = $opts["port"]
if ($PORT -notmatch '^\d+$') { Fail "port must be a number (got '$PORT')" 2 }

Write-Host "options"
Write-Host "  locale => $LOCALE"
Write-Host "  venv   => $VENV"
Write-Host "  serve  => $SERVE"
Write-Host ""

# --- preflight ------------------------------------------------------------
$VENV_BIN = Join-Path (Join-Path $ROOT_DIR $VENV) "Scripts"
if (-not (Test-Path $VENV_BIN)) { $VENV_BIN = Join-Path (Join-Path $ROOT_DIR $VENV) "bin" }

$ZENSICAL = $null
foreach ($candidate in @((Join-Path $VENV_BIN "zensical.exe"), (Join-Path $VENV_BIN "zensical"))) {
    if (Test-Path $candidate) { $ZENSICAL = $candidate; break }
}
if (-not $ZENSICAL) {
    $onPath = Get-Command zensical -ErrorAction SilentlyContinue
    if ($onPath) {
        $ZENSICAL = $onPath.Source
        Write-Host "${APP}: no venv at $VENV; using zensical from PATH"
    }
}
if (-not $ZENSICAL) {
    [Console]::Error.WriteLine("${APP}: zensical not found in $VENV or on PATH")
    Fail "run 'scripts/prepare-docs.ps1' first"
}

# Every path in the configs is relative to the repo root.
Push-Location $ROOT_DIR
try {
    # --- build ------------------------------------------------------------
    if ($LOCALE -in "all", "en") {
        Write-Host ">> zensical build --strict (en) -> site/"
        & $ZENSICAL build --strict
        if ($LASTEXITCODE -ne 0) { Fail "English build failed" }
    }
    if ($LOCALE -in "all", "ja") {
        Write-Host ">> zensical build -f mkdocs.ja.yml --strict (ja) -> site/ja/"
        & $ZENSICAL build -f mkdocs.ja.yml --strict
        if ($LASTEXITCODE -ne 0) { Fail "Japanese build failed" }
    }

    Write-Host ""
    switch ($LOCALE) {
        "all" { Write-Host "${APP}: done -> site/ (en) + site/ja/ (ja)" }
        "en"  {
            Write-Host "${APP}: done -> site/ (en)"
            [Console]::Error.WriteLine("${APP}: WARNING: this build cleared site/, so site/ja no longer exists.")
            [Console]::Error.WriteLine("${APP}:          run '$APP locale=ja' before serving or publishing.")
        }
        "ja"  {
            Write-Host "${APP}: done -> site/ja/ (ja)"
            Write-Host "${APP}: site/ still holds whatever the last English build left there."
        }
    }

    # --- serve ------------------------------------------------------------
    Write-Host ""
    if ($SERVE -eq "yes") {
        $python = Get-Command python3 -ErrorAction SilentlyContinue
        if (-not $python) { $python = Get-Command python -ErrorAction SilentlyContinue }
        if (-not $python) { Fail "no python on PATH to serve with -- use any static server on site/" }
        Write-Host "${APP}: http://localhost:$PORT/     (English)"
        Write-Host "${APP}: http://localhost:$PORT/ja/  (Japanese)"
        Write-Host "${APP}: Ctrl-C to stop"
        Write-Host ""
        & $python.Source -m http.server $PORT -d site
    } else {
        Write-Host "${APP}: serve it (one server over both locales, so the language selector resolves):"
        Write-Host "${APP}:   $APP serve=yes"
    }
} finally {
    Pop-Location
}
