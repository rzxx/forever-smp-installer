#requires -Version 7
param(
    [string]$PackRoot = (Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'modpack'),
    [switch]$BuildPack,
    [switch]$BuildOnly
)
$ErrorActionPreference = 'Stop'
$sourceRoot = Split-Path -Parent $PSScriptRoot
$previousFxcPath = $env:GPUI_FXC_PATH
$previousDataDirectory = $env:FOREVER_SMP_DATA_DIR
Push-Location $sourceRoot
try {
    . (Join-Path $PSScriptRoot 'windows-build.ps1')
    Initialize-WindowsBuild
    & cargo build --locked -p forever-smp -p forever-release
    if ($LASTEXITCODE) { throw 'Development build failed.' }
    $targetDirectory = Get-CargoTargetDirectory
    $executable = Join-Path $targetDirectory 'debug/forever-smp.exe'
    if ($BuildOnly) { Write-Host "Built current source: $executable"; return }

    $PackRoot = (Resolve-Path -LiteralPath $PackRoot).Path
    if ($BuildPack) { & (Join-Path $PackRoot 'scripts/build.ps1') }
    $line = Get-Content -LiteralPath (Join-Path $PackRoot 'pack.toml') |
        Where-Object { $_ -match '^version = "([^"]+)"$' } | Select-Object -First 1
    if ($line -notmatch '^version = "([^"]+)"$') { throw 'Missing pack version.' }
    $pack = Join-Path $PackRoot "dist/Forever-SMP-$($Matches[1]).mrpack"
    if (-not (Test-Path -LiteralPath $pack -PathType Leaf)) {
        throw 'Build the modpack first, or run dev.ps1 -BuildPack. Use -PackRoot for a different checkout.'
    }
    $devRoot = Join-Path $sourceRoot 'dist/dev'
    $profile = Join-Path $devRoot 'profile'
    $game = Join-Path $devRoot 'game'
    New-Item -ItemType Directory -Path $profile,$game -Force | Out-Null
    & (Join-Path $targetDirectory 'debug/forever-release.exe') build $PackRoot $pack $devRoot
    if ($LASTEXITCODE) { throw 'Development recipe export failed.' }
    $preferencesPath = Join-Path $profile 'preferences.json'
    $preferences = if (Test-Path -LiteralPath $preferencesPath) {
        Get-Content -LiteralPath $preferencesPath -Raw | ConvertFrom-Json -AsHashtable
    } else { @{ language = ''; nickname = ''; game = $game } }
    $preferences.recipe = Join-Path $devRoot 'release.json'
    $preferences.use_local_release = $true
    $preferences.resume_update = $null
    $preferences | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $preferencesPath -Encoding utf8NoBOM
    $env:FOREVER_SMP_DATA_DIR = $profile
    Write-Host "Running current source: $executable"
    Write-Host "Local recipe: $($preferences.recipe)"
    Write-Host "Isolated preferences: $profile; game: $($preferences.game)"
    & $executable
    if ($LASTEXITCODE) { throw 'Development app exited with an error.' }
} finally {
    $env:GPUI_FXC_PATH = $previousFxcPath
    $env:FOREVER_SMP_DATA_DIR = $previousDataDirectory
    Pop-Location
}
