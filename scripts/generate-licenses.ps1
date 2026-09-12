#!/usr/bin/env pwsh
#
# Regenerate THIRD_PARTY_NOTICES.md from the dependency tree.
# The PowerShell twin of generate-licenses.sh — see that file for the reasoning.

param(
    # Where to write the notices. Defaults to the committed THIRD_PARTY_NOTICES.md.
    [string]$Out
)

$ErrorActionPreference = "Stop"

$baseDirectory = Split-Path -Parent $PSCommandPath
$rootDirectory = Split-Path -Parent $baseDirectory

Push-Location $rootDirectory
try {
    if (-not $Out) {
        $Out = Join-Path $rootDirectory "THIRD_PARTY_NOTICES.md"
    }

    if (-not (Get-Command "cargo-about" -ErrorAction SilentlyContinue)) {
        Write-Host "cargo-about is not installed. Installing it now..."
        cargo install cargo-about --locked --features cli
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

    Write-Host "Done. Review the diff before committing."
}
finally {
    Pop-Location
}
