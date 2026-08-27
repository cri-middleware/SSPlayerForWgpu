#!/usr/bin/env pwsh
#
# Create the local virtualenv and install the pinned documentation toolchain.
# PowerShell twin of prepare-docs.sh.
#
#   scripts/prepare-docs.ps1            # .venv + docs/requirements.txt
#   scripts/prepare-docs.ps1 force=yes  # discard the venv and build it again
#
# The documentation site needs neither the SDK submodule, nor cargo, nor Node:
# building the library is build.ps1, and the demo's playback data is
# deploy-examples.ps1. Neither is a prerequisite here.
#
# Then: scripts/build-docs.ps1, which runs zensical out of the venv this created.
$ErrorActionPreference = "Stop"

$APP = Split-Path -Leaf $PSCommandPath
$ROOT_DIR = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$REQUIREMENTS = "docs/requirements.txt"

function Fail {
    param([string] $Message, [int] $Code = 1)
    [Console]::Error.WriteLine("${APP}: $Message")
    exit $Code
}

# --- options --------------------------------------------------------------
$opts = [ordered]@{
    python = "py"
    venv   = ".venv"
    force  = "no"
}

function Show-Usage {
    Write-Host "Usage: $APP [options]"
    Write-Host "$APP options:"
    Write-Host "  python=<exe>            Interpreter to create the venv with (default: py)"
    Write-Host "  venv=<dir>              Virtualenv location, relative to the repo root"
    Write-Host "                          (default: .venv)"
    Write-Host "  force=<yes|no>          Delete an existing venv and recreate it, rather than"
    Write-Host "                          installing into it (default: no)"
    Write-Host "  -h | --help             Show this help"
    Write-Host ""
    Write-Host "Installs $REQUIREMENTS, whose pin is exact. Run it again whenever the pin"
    Write-Host "changes; force=yes is only for a venv that has gone wrong."
    Write-Host ""
    Write-Host "Building the site is scripts/build-docs.ps1, not this script."
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

$PYTHON = $opts["python"]
$VENV = $opts["venv"]
$FORCE = $opts["force"].ToLower()
if ($FORCE -notin "yes", "no") { Fail "force must be yes or no (got '$FORCE')" 2 }

Write-Host "options"
Write-Host "  python => $PYTHON"
Write-Host "  venv   => $VENV"
Write-Host "  force  => $FORCE"
Write-Host ""

# --- preflight ------------------------------------------------------------
if (-not (Test-Path (Join-Path $ROOT_DIR $REQUIREMENTS))) {
    Fail "$REQUIREMENTS not found -- is this the SSPlayerForWgpu repo?"
}
if (-not (Get-Command $PYTHON -ErrorAction SilentlyContinue)) {
    Fail "'$PYTHON' not found (install Python 3, or pass python=<exe>)"
}

$VENV_DIR = Join-Path $ROOT_DIR $VENV

# --- venv -----------------------------------------------------------------
if ($FORCE -eq "yes" -and (Test-Path $VENV_DIR)) {
    Write-Host ">> removing the existing venv ($VENV)"
    Remove-Item -Recurse -Force $VENV_DIR
}

if (Test-Path $VENV_DIR) {
    Write-Host ">> reusing the existing venv ($VENV)"
} else {
    Write-Host ">> $PYTHON -m venv $VENV"
    & $PYTHON -m venv $VENV_DIR
    if ($LASTEXITCODE -ne 0) { Fail "venv creation failed" }
}

# Windows venvs put their executables in Scripts/, POSIX ones in bin/. Probe
# rather than branch on the OS: pwsh runs on all three.
$VENV_BIN = Join-Path $VENV_DIR "Scripts"
if (-not (Test-Path $VENV_BIN)) { $VENV_BIN = Join-Path $VENV_DIR "bin" }

$VENV_PY = Join-Path $VENV_BIN "python.exe"
if (-not (Test-Path $VENV_PY)) { $VENV_PY = Join-Path $VENV_BIN "python" }
if (-not (Test-Path $VENV_PY)) {
    Fail "no interpreter in $VENV -- the venv looks broken; retry with force=yes"
}

# --- toolchain ------------------------------------------------------------
Write-Host ">> pip install -r $REQUIREMENTS"
& $VENV_PY -m pip install -r (Join-Path $ROOT_DIR $REQUIREMENTS)
if ($LASTEXITCODE -ne 0) { Fail "pip install failed" }

Write-Host ""
Write-Host ">> installed:"
& $VENV_PY -m pip show zensical 2>$null | Select-String -Pattern '^(Name|Version):'

Write-Host ""
Write-Host "${APP}: done -> $VENV"
Write-Host "${APP}: build both locales: scripts/build-docs.ps1"
Write-Host "${APP}: preview one:        $VENV/$(Split-Path -Leaf $VENV_BIN)/zensical serve  (add -f mkdocs.ja.yml for Japanese)"
