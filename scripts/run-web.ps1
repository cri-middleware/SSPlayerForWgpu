#!/usr/bin/env pwsh
#
# Take a fresh checkout to the browser demo: scripts/run-example.ps1 example=web.
& (Join-Path $PSScriptRoot "run-example.ps1") example=web @args
exit $LASTEXITCODE
