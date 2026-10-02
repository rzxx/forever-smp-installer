#requires -Version 7
param([switch]$Handoff)
$ErrorActionPreference = 'Stop'
$sourceRoot = Split-Path -Parent $PSScriptRoot
$previousFxcPath = $env:GPUI_FXC_PATH
Push-Location $sourceRoot
try {
    . (Join-Path $PSScriptRoot 'windows-build.ps1')
    Initialize-WindowsBuild
    & cargo test --locked --workspace
    if ($LASTEXITCODE) { throw 'Installer tests failed.' }
    & cargo clippy --locked --workspace --all-targets -- -D warnings
    if ($LASTEXITCODE) { throw 'Installer lint checks failed.' }
    if ($Handoff) { & (Join-Path $PSScriptRoot 'test-app-handoff.ps1') }
} finally { $env:GPUI_FXC_PATH = $previousFxcPath; Pop-Location }
