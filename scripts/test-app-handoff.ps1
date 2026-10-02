#requires -Version 7
param()
$ErrorActionPreference = 'Stop'
$sourceRoot = Split-Path -Parent $PSScriptRoot
Push-Location $sourceRoot
try {
    . (Join-Path $PSScriptRoot 'windows-build.ps1')
    & cargo build --locked -p forever-core --example handoff-fixture
    if ($LASTEXITCODE) { throw 'Handoff fixture build failed.' }
    & cargo build --locked -p forever-release
    if ($LASTEXITCODE) { throw 'Release tool build failed.' }
    $targetDirectory = Get-CargoTargetDirectory
    $tool = Join-Path $targetDirectory 'debug/forever-release.exe'
    $fixture = Join-Path $targetDirectory 'debug/examples/handoff-fixture.exe'
    $line = Get-Content Cargo.toml | Where-Object { $_ -match '^version = "([^"]+)"$' } | Select-Object -First 1
    if ($line -notmatch '^version = "([^"]+)"$') { throw 'Missing app version' }
    $current = $Matches[1]
    $version = [version]$current
    $future = "$($version.Major).$($version.Minor).$($version.Build + 1)"
    $checkRoot = Join-Path $sourceRoot "dist/checks/handoff[$([guid]::NewGuid())]"
    New-Item -ItemType Directory -Path $checkRoot | Out-Null
    $notes = Join-Path $checkRoot 'notes.toml'
    @'
notes_en = "Isolated Windows helper fixture."
notes_ru = "Изолированная проверка обновления Windows."
'@ | Set-Content -LiteralPath $notes -Encoding utf8
    $results = @()
    foreach ($scenario in @('success', 'rollback')) {
        $elapsed = [Diagnostics.Stopwatch]::StartNew()
        $folder = Join-Path $checkRoot $scenario
        New-Item -ItemType Directory -Path $folder | Out-Null
        $target = Join-Path $folder 'Forever-SMP.exe'
        $old = [byte[]]([IO.File]::ReadAllBytes($fixture) + [Text.Encoding]::UTF8.GetBytes('old PE overlay for isolated rehearsal'))
        [IO.File]::WriteAllBytes($target, $old)
        $oldHash = (Get-FileHash -LiteralPath $target -Algorithm SHA512).Hash
        $personal = Join-Path $folder 'personal-settings.txt'
        [IO.File]::WriteAllText($personal, 'keep my folder, language and optional choices')
        $metadata = Join-Path $folder 'installer.json'
        $signed = Join-Path $folder 'installer.signed.json'
        $candidate = if ($scenario -eq 'success') { $current } else { $future }
        & $tool build-installer $fixture $candidate rzxx/forever-smp-installer $notes $metadata
        if ($LASTEXITCODE) { throw 'Fixture export failed' }
        & $fixture sign $metadata $signed
        if ($LASTEXITCODE) { throw 'Fixture signing failed' }
        $output = & $fixture rehearse $target $signed $fixture
        if ($LASTEXITCODE) { throw 'Helper launch failed' }
        $job = [string]($output | Select-Object -Last 1)
        $stage = Split-Path -Parent $job
        $statusPath = Join-Path $stage 'status.txt'
        $marker = Join-Path $stage $(if ($scenario -eq 'success') { 'smoke-ready.txt' } else { 'smoke-failure.txt' })
        $expected = if ($scenario -eq 'success') { 'confirmed' } else { 'rolled-back' }
        $deadline = [DateTime]::UtcNow.AddSeconds(25)
        do {
            $status = if (Test-Path -LiteralPath $statusPath) { [IO.File]::ReadAllText($statusPath) } else { '' }
            if ($status -eq $expected -and (Test-Path -LiteralPath $marker)) { break }
            Start-Sleep -Milliseconds 200
        } while ([DateTime]::UtcNow -lt $deadline)
        if ($status -ne $expected -or -not (Test-Path -LiteralPath $marker)) { throw "Scenario $scenario failed; inspect $stage" }
        $newHash = (Get-FileHash -LiteralPath $fixture -Algorithm SHA512).Hash
        $actualHash = (Get-FileHash -LiteralPath $target -Algorithm SHA512).Hash
        if ($actualHash -ne $(if ($scenario -eq 'success') { $newHash } else { $oldHash })) { throw 'Incorrect executable after handoff' }
        if ((Get-FileHash -LiteralPath (Join-Path $stage 'old.exe') -Algorithm SHA512).Hash -ne $oldHash) { throw 'Invalid backup' }
        if ([IO.File]::ReadAllText($personal) -ne 'keep my folder, language and optional choices') { throw 'Personal file changed' }
        Write-Host ('Windows updater {0}: {1:N1}s' -f $scenario, $elapsed.Elapsed.TotalSeconds)
        $results += [pscustomobject]@{ scenario = $scenario; status = $status; executable_verified = $true; backup_verified = $true; personal_file_preserved = $true; job = $job }
    }
    $results | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $checkRoot 'results.json') -Encoding utf8
    $results | Select-Object scenario, status, executable_verified, backup_verified, personal_file_preserved | Format-Table
    Write-Host "Evidence: $checkRoot"
} finally { Pop-Location }
