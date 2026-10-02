#requires -Version 7
param([string]$Repository = 'rzxx/forever-smp-installer')
$ErrorActionPreference = 'Stop'
$sourceRoot = Split-Path -Parent $PSScriptRoot
$previousFxcPath = $env:GPUI_FXC_PATH
Push-Location $sourceRoot
try {
    . (Join-Path $PSScriptRoot 'windows-build.ps1')
    Initialize-WindowsBuild
    & (Join-Path $PSScriptRoot 'test.ps1')
    & cargo build --locked --release -p forever-smp -p forever-release
    if ($LASTEXITCODE) { throw 'Release build failed.' }
    $targetDirectory = Get-CargoTargetDirectory
    $appVersionLine = Get-Content Cargo.toml | Where-Object { $_ -match '^version = "([^"]+)"$' } | Select-Object -First 1
    if ($appVersionLine -notmatch '^version = "([^"]+)"$') { throw 'Missing app version.' }
    $appVersion = $Matches[1]
    $releaseDirectory = Join-Path $sourceRoot "dist/releases/$appVersion"
    New-Item -ItemType Directory -Path $releaseDirectory -Force | Out-Null
    $executable = Join-Path $releaseDirectory "Forever-SMP-Setup-$appVersion-Windows-x64.exe"
    Copy-Item -LiteralPath (Join-Path $targetDirectory 'release/forever-smp.exe') -Destination $executable -Force
    & (Join-Path $targetDirectory 'release/forever-release.exe') build-installer $executable $appVersion $Repository app-release.toml (Join-Path $releaseDirectory 'installer.json')
    if ($LASTEXITCODE) { throw 'Installer metadata export failed.' }
    $archive = Join-Path $releaseDirectory "Forever-SMP-Setup-$appVersion-Windows-x64.zip"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $temporaryArchive = "$archive.new"
    if (Test-Path -LiteralPath $temporaryArchive) { Remove-Item -LiteralPath $temporaryArchive }
    $zip = [IO.Compression.ZipFile]::Open($temporaryArchive, [IO.Compression.ZipArchiveMode]::Create)
    try {
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $executable, 'Forever-SMP.exe') | Out-Null
        $writer = [IO.StreamWriter]::new($zip.CreateEntry('READ-ME.md').Open(), [Text.UTF8Encoding]::new($false))
        try {
            $writer.WriteLine('# Forever SMP Setup')
            $writer.WriteLine('Extract this ZIP into a writable folder and run Forever-SMP.exe. Your launcher handles Minecraft, Fabric, Java and accounts. Close Minecraft before updating.')
            $writer.WriteLine('Распакуйте ZIP и запустите Forever-SMP.exe. Minecraft, Fabric, Java и учётные записи настраивает лаунчер. Перед обновлением закройте Minecraft.')
            $writer.WriteLine('Source and downloads: https://github.com/rzxx/forever-smp-installer')
        } finally { $writer.Dispose() }
    } finally { $zip.Dispose() }
    Move-Item -LiteralPath $temporaryArchive -Destination $archive -Force
    Write-Host "Unsigned release candidate: $releaseDirectory"
    Write-Host 'Sign and review before publishing. See docs/releases.md.'
} finally { $env:GPUI_FXC_PATH = $previousFxcPath; Pop-Location }
