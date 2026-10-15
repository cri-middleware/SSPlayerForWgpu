#!/usr/bin/env pwsh
#
# Regenerate THIRD_PARTY_NOTICES.md from the dependency tree.
# The PowerShell twin of generate-licenses.sh — see that file for the reasoning.
#
# Usage:
#   scripts/generate-licenses.ps1              # -> THIRD_PARTY_NOTICES.md
#   scripts/generate-licenses.ps1 out=/tmp/x   # somewhere else
#   scripts/generate-licenses.ps1 --help

$ErrorActionPreference = "Stop"

$APP = Split-Path -Leaf $PSCommandPath
$baseDirectory = Split-Path -Parent $PSCommandPath
$rootDirectory = Split-Path -Parent $baseDirectory

$Out = Join-Path $rootDirectory "THIRD_PARTY_NOTICES.md"

foreach ($item in $args) {
    if ($item -in "-h", "--help") {
        Get-Content $PSCommandPath | Select-Object -Skip 1 -First 8 | ForEach-Object { $_ -replace '^# ?', '' }
        exit 0
    } elseif ($item -like "out=*") {
        $Out = $item.Substring(4)
    } else {
        [Console]::Error.WriteLine("${APP}: unknown option '$item' (try --help)")
        exit 2
    }
}

Push-Location $rootDirectory
try {
    if (-not (Get-Command "cargo-about" -ErrorAction SilentlyContinue)) {
        Write-Host "cargo-about is not installed. Installing it now..."
        cargo install cargo-about --locked --features cli
        if ($LASTEXITCODE -ne 0) { [Console]::Error.WriteLine("${APP}: cargo install cargo-about failed ($LASTEXITCODE)"); exit $LASTEXITCODE }
    }

    # The union of the three shipped faces: embedded (the `image` decoder the
    # Rust library and the C face carry), ffi (bin/libssplayer_wgpu.*) and web
    # (the wasm behind @spritestudio/ssplayer-wgpu). The Bevy adapter is a
    # separate crate and an app's own dependency, so it is not folded in.
    Write-Host "Generating $Out..."
    cargo about generate `
        --manifest-path libs/ssplayer-wgpu/Cargo.toml `
        --config about.toml `
        --features embedded,ffi,web `
        about-markdown.hbs `
        -o $Out
    # A native command's failure is not an error to PowerShell, so without this a
    # run that wrote nothing still said "Done" -- where the .sh stops on `set -e`.
    if ($LASTEXITCODE -ne 0) { [Console]::Error.WriteLine("${APP}: cargo about generate failed ($LASTEXITCODE)"); exit $LASTEXITCODE }

    Write-Host "Done. Review the diff before committing."
}
finally {
    Pop-Location
}
