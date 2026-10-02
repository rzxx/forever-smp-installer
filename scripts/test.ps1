#requires -Version 7
param([Alias('Handoff')][switch]$Full)
$ErrorActionPreference = 'Stop'
$sourceRoot = Split-Path -Parent $PSScriptRoot
$previousFxcPath = $env:GPUI_FXC_PATH
Push-Location $sourceRoot
try {
    . (Join-Path $PSScriptRoot 'windows-build.ps1')
    Initialize-WindowsBuild
    $elapsed = [Diagnostics.Stopwatch]::StartNew()
    & cargo test --locked --workspace
    if ($LASTEXITCODE) { throw 'Installer tests failed.' }
    Write-Host ('Tests: {0:N1}s' -f $elapsed.Elapsed.TotalSeconds)
    $elapsed.Restart()
    & cargo clippy --locked --workspace --all-targets -- -D warnings
    if ($LASTEXITCODE) { throw 'Installer lint checks failed.' }
    Write-Host ('Lint: {0:N1}s' -f $elapsed.Elapsed.TotalSeconds)
    if ($Full) {
        $elapsed.Restart()
        & (Join-Path $PSScriptRoot 'test-app-handoff.ps1')
        Write-Host ('Windows replacement/rollback: {0:N1}s' -f $elapsed.Elapsed.TotalSeconds)
    }
} finally { $env:GPUI_FXC_PATH = $previousFxcPath; Pop-Location }
