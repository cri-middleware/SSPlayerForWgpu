#!/usr/bin/env pwsh
#
# Convert the SpriteStudio test projects in the SDK into playback data under
# assets/ (.ssab + referenced textures), using ssconverter-cli built from the
# SDK submodule with cargo. PowerShell twin of deploy-examples.sh.
#
#   assets/overall/        <- tests/overall, reference textures (sibling .png)
#   assets/overall_emb/    <- tests/overall, textures embedded in the .ssab
#   assets/Ringo/          <- tests/Ringo, reference textures
#   assets/Ringo_emb/      <- tests/Ringo, textures embedded in the .ssab
#   assets/Doll/           <- tests/Doll, reference textures
#   assets/Doll_emb/       <- tests/Doll, textures embedded in the .ssab
#
# and then deploys them to the examples that cannot read assets/ in place:
#
#   examples/web/assets/       <- assets/overall (reference mode, + its .png atlases)
#   examples/web/assets/Ringo/ <- assets/Ringo   (reference mode, + Ringo.png)
#   examples/web/assets/Doll/  <- assets/Doll    (reference mode, + Doll.png)
#   examples/web/assets_emb/   <- assets/overall_emb (embedded mode)
#   examples/web/assets_emb/Ringo/ <- assets/Ringo_emb (embedded mode)
#   examples/web/assets_emb/Doll/  <- assets/Doll_emb  (embedded mode)
#
# Both modes go out because the demo page has a selector for the two: an embedded
# pack carries its atlas, a reference pack names sibling files that `loadSsab`
# fetches relative to the .ssab's own URL (SPEC 8.1), and nothing else on the page
# covers that second branch.
#
# assets/ is gitignored. libs/ssplayer-wgpu's tests read assets/overall and
# assets/overall_emb; examples/viewer and examples/viewer-metal-cpp play
# assets/overall from the repository, so only the browser needs a copy.
#
# Usage: scripts/deploy-examples.ps1 [release|debug]   (ssconverter-cli build mode; default release)
#
# SDK location: $env:SSSDK, else ./SpriteStudio-SDK (submodule), else ../SpriteStudio-SDK.
$ErrorActionPreference = "Stop"

$APP = Split-Path -Leaf $PSCommandPath
$ROOT_DIR = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$ASSET_DIR = Join-Path $ROOT_DIR "assets"
$WEB_DIR = Join-Path $ROOT_DIR "examples/web"

# Plain stderr + an exit code, rather than Write-Error's stack trace; matches
# deploy-examples.sh (2 = bad usage, 1 = the environment is not ready).
function Fail {
    param([string] $Message, [int] $Code = 1)
    [Console]::Error.WriteLine("${APP}: $Message")
    exit $Code
}

$MODE = "release"
foreach ($arg in $args) {
    switch ($arg) {
        { $_ -in "release", "debug" } { $MODE = $_ }
        { $_ -in "-h", "--help" } {
            Get-Content $PSCommandPath | Select-Object -Skip 1 -First 31 |
                ForEach-Object { $_ -replace '^# ?', '' }
            exit 0
        }
        default { Fail "unknown arg '$_'" 2 }
    }
}

# --- locate the SDK -------------------------------------------------------
$SDK_DIR = $env:SSSDK
if ([string]::IsNullOrEmpty($SDK_DIR)) {
    foreach ($cand in @((Join-Path $ROOT_DIR "SpriteStudio-SDK"), (Join-Path $ROOT_DIR "../SpriteStudio-SDK"))) {
        if (Test-Path (Join-Path $cand "Cargo.toml")) {
            $SDK_DIR = (Resolve-Path $cand).Path
            break
        }
    }
}
if ([string]::IsNullOrEmpty($SDK_DIR) -or !(Test-Path (Join-Path $SDK_DIR "Cargo.toml"))) {
    Fail "SpriteStudio-SDK not found (git submodule update --init, or set SSSDK)"
}

# --- build the converter --------------------------------------------------
# Built inside the SDK checkout so it uses the SDK's own workspace and target
# dir; this repo's workspace does not depend on ssconverter.
Write-Host ">> building ssconverter-cli ($MODE) from $SDK_DIR"
$cargoArgs = @("build", "-p", "ssconverter-cli")
if ($MODE -eq "release") { $cargoArgs += "--release" }
Push-Location $SDK_DIR
try {
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { Fail "cargo build failed ($LASTEXITCODE)" $LASTEXITCODE }
} finally {
    Pop-Location
}

$CLI = Join-Path $SDK_DIR "target/$MODE/ssconverter-cli"
if ($IsWindows) { $CLI += ".exe" }
if (!(Test-Path $CLI)) { Fail "$CLI not found after build" }

# --- convert --------------------------------------------------------------
function Convert-Project {
    param(
        [string] $Sspj,           # relative to the SDK
        [string] $Name,           # destination under assets/
        [string[]] $ExtraArgs = @()
    )
    $dest = Join-Path $ASSET_DIR $Name
    Write-Host ">> converting $Sspj -> assets/$Name $ExtraArgs"
    if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
    New-Item -ItemType Directory -Force -Path $dest > $null
    & $CLI @ExtraArgs --out-dir $dest (Join-Path $SDK_DIR $Sspj)
    if ($LASTEXITCODE -ne 0) { Fail "ssconverter-cli failed for $Sspj ($LASTEXITCODE)" $LASTEXITCODE }
}

Convert-Project "tests/overall/overall.sspj" "overall"
Convert-Project "tests/overall/overall.sspj" "overall_emb" @("--embedded")
Convert-Project "tests/Ringo/Ringo.sspj"     "Ringo"
Convert-Project "tests/Ringo/Ringo.sspj"     "Ringo_emb" @("--embedded")
Convert-Project "tests/Doll/Doll.sspj"       "Doll"
Convert-Project "tests/Doll/Doll.sspj"       "Doll_emb" @("--embedded")

# --- deploy to the examples ----------------------------------------------
# A page can only fetch what it serves, so the web demo gets its own copy. The
# `_emb` suffix mirrors assets/overall vs assets/overall_emb, so the two modes
# read the same way at both ends of the copy.
function Reset-Dir {
    param([string] $Path)
    if (Test-Path $Path) { Remove-Item -Recurse -Force $Path }
    New-Item -ItemType Directory -Force -Path $Path > $null
}

# Reference mode, atlases included: that is the branch the page exercises by
# resolving each .png against the .ssab's own URL and fetching it.
Write-Host ">> deploying reference .ssab + atlases -> examples/web/assets"
$dest = Join-Path $WEB_DIR "assets"
Reset-Dir $dest
Copy-Item -Force (Join-Path $ASSET_DIR "overall/*.ssab") $dest
Copy-Item -Force (Join-Path $ASSET_DIR "overall/*.png") $dest
# Ringo is an authored character rather than a feature fixture, which is what the
# layout pages want on the page. Kept in a subdirectory of its own: sibling packs
# and atlases resolve against the .ssab's own directory, so two projects sharing
# one flat directory would be one atlas filename away from colliding.
$ringo = Join-Path $dest "Ringo"
New-Item -ItemType Directory -Force -Path $ringo | Out-Null
Copy-Item -Force (Join-Path $ASSET_DIR "Ringo/*.ssab") $ringo
Copy-Item -Force (Join-Path $ASSET_DIR "Ringo/*.png") $ringo
$doll = Join-Path $dest "Doll"
New-Item -ItemType Directory -Force -Path $doll | Out-Null
Copy-Item -Force (Join-Path $ASSET_DIR "Doll/*.ssab") $doll
Copy-Item -Force (Join-Path $ASSET_DIR "Doll/*.png") $doll

# Embedded mode: the atlas is baked in, so these need no sibling files.
Write-Host ">> deploying embedded .ssab -> examples/web/assets_emb"
$dest = Join-Path $WEB_DIR "assets_emb"
Reset-Dir $dest
Copy-Item -Force (Join-Path $ASSET_DIR "overall_emb/*.ssab") $dest
$ringo = Join-Path $dest "Ringo"
New-Item -ItemType Directory -Force -Path $ringo | Out-Null
Copy-Item -Force (Join-Path $ASSET_DIR "Ringo_emb/*.ssab") $ringo
$doll = Join-Path $dest "Doll"
New-Item -ItemType Directory -Force -Path $doll | Out-Null
Copy-Item -Force (Join-Path $ASSET_DIR "Doll_emb/*.ssab") $doll

Write-Host "${APP}: done -> assets/{overall,overall_emb,Ringo,Ringo_emb,Doll,Doll_emb}/"
Write-Host "${APP}:      -> examples/web/{assets,assets_emb}/{,Ringo/,Doll/}"
