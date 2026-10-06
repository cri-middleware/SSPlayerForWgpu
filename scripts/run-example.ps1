#!/usr/bin/env pwsh
#
# Take a fresh checkout to a running example. PowerShell twin of run-example.sh.
#
#   scripts/run-example.ps1                  # examples/hello: Ringo, the first frame
#   scripts/run-example.ps1 example=viewer   # the development viewer
#   scripts/run-example.ps1 example=web      # the browser demo (also scripts/run-web.ps1)
#
# example=<hello|viewer|viewer-bevy|overall-bevy|web>, hello by default. In order:
#
#   1. converts the SDK's test projects if assets/ is missing or another SDK
#      commit converted it (scripts/deploy-examples.ps1 when=stale)
#   2. web only: builds the npm package into examples/web if it is not there, or
#      if step 1 converted (scripts/build.ps1 platform=web)
#   3. runs the example: cargo run for the native ones; for web, serves
#      examples/web on http://localhost:8000 and opens it (Ctrl-C stops the server)
#
# The data follows the SDK commit because its format does. Nothing checks the
# package against the Rust: after changing the Rust, run build.ps1 platform=web
# yourself. The SDK submodule has to be checked out (git submodule update --init).
$ErrorActionPreference = "Stop"

$APP = Split-Path -Leaf $PSCommandPath
$ROOT_DIR = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$WEB_DIR = Join-Path $ROOT_DIR "examples/web"
$PORT = 8000

# Plain stderr + an exit code, rather than Write-Error's stack trace; matches
# run-example.sh (2 = bad usage, 1 = the environment is not ready).
function Fail {
    param([string] $Message, [int] $Code = 1)
    [Console]::Error.WriteLine("${APP}: $Message")
    exit $Code
}

$EXAMPLE = "hello"
foreach ($arg in $args) {
    if ($arg -in "-h", "--help") {
        Get-Content $PSCommandPath | Select-Object -Skip 1 -First 19 | ForEach-Object { $_ -replace '^# ?', '' }
        exit 0
    } elseif ($arg -like "example=*") {
        $EXAMPLE = $arg.Substring(8)
    } else {
        Fail "unknown argument '$arg' (see $APP --help)" 2
    }
}

$CRATE = switch ($EXAMPLE) {
    "hello" { "ssplayer-wgpu-hello" }
    "viewer" { "ssplayer-wgpu-viewer" }
    "viewer-bevy" { "ssplayer-bevy-viewer" }
    "overall-bevy" { "ssplayer-bevy-overall" }
    "web" { "" }
    default { Fail "example must be hello, viewer, viewer-bevy, overall-bevy or web (got '$EXAMPLE')" 2 }
}

# --- 1. the playback data -------------------------------------------------
# The examples are built from the SDK checked out now, and the .ssab format moves
# with it. When this converts, the stamp changes, and an existing web package --
# built from the old SDK -- can no longer read the data either.
$stamp = Join-Path $ROOT_DIR "assets/.sdk-rev"
function Read-Stamp { if (Test-Path $stamp) { (Get-Content -Raw $stamp).Trim() } else { "" } }
$before = Read-Stamp
Write-Host ">> ${APP}: the playback data (scripts/deploy-examples.ps1 when=stale)"
& (Join-Path $PSScriptRoot "deploy-examples.ps1") when=stale
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$converted = (Read-Stamp) -ne $before

# --- 2 and 3 --------------------------------------------------------------
if ($CRATE) {
    Write-Host ">> ${APP}: cargo run -p $CRATE"
    Push-Location $ROOT_DIR
    try { cargo run -p $CRATE } finally { Pop-Location }
    exit $LASTEXITCODE
}

if ($converted -or -not (Test-Path (Join-Path $WEB_DIR "ssplayer-wgpu/wasm/ssplayer_wgpu_bg.wasm"))) {
    Write-Host ">> ${APP}: building the npm package into examples/web (scripts/build.ps1 platform=web)"
    & (Join-Path $PSScriptRoot "build.ps1") platform=web
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

# On Windows `python3` is often the Store's stub, which exits without serving.
$candidates = if ($IsWindows) { "py", "python" } else { "python3", "python" }
$python = Get-Command $candidates -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $python) { Fail "python not found -- it serves examples/web" }
$url = "http://localhost:$PORT/"
Write-Host ">> ${APP}: serving examples/web on $url (Ctrl-C to stop) -- it needs a WebGPU browser"
$server = Start-Process -FilePath $python.Source -ArgumentList "-m", "http.server", "$PORT", "--bind", "127.0.0.1", "--directory", "`"$WEB_DIR`"" -NoNewWindow -PassThru
try {
    Start-Sleep -Seconds 1
    if ($server.HasExited) { Fail "the server did not start -- is port $PORT taken?" }
    Start-Process $url
    $server.WaitForExit()
} finally {
    if (-not $server.HasExited) { $server.Kill() }
}
