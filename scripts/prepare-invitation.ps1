#requires -Version 7
param(
    [string]$ServerAddress,
    [string]$Repository = '',
    [string]$PublicKey = '',
    [string]$InstructionsEn = '',
    [string]$InstructionsRu = '',
    [string]$Output = ''
)
$ErrorActionPreference = 'Stop'
$sourceRoot = Split-Path -Parent $PSScriptRoot
if (-not $ServerAddress) { $ServerAddress = Read-Host 'Server address (private invitation only)' }
if (-not $Output) { $Output = Join-Path $sourceRoot '.private/invitation.private.json' }
if ($Repository -and ($Repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$' -or $PublicKey -notmatch '^[0-9a-fA-F]{64}$')) { throw 'Use owner/repository and the 64-character public release key' }
if (Test-Path -LiteralPath $Output) { throw 'Choose a new output path; an invitation already exists there.' }
New-Item -ItemType Directory -Path (Split-Path -Parent ([IO.Path]::GetFullPath($Output))) -Force | Out-Null
[ordered]@{
    repository = $Repository
    public_key = $PublicKey
    server = $ServerAddress
    instructions_en = $InstructionsEn
    instructions_ru = $InstructionsRu
} | ConvertTo-Json | Set-Content -LiteralPath $Output -Encoding utf8NoBOM
Write-Host "Send privately: $Output"
Write-Host 'Send the global registration password separately. Do not put passwords in this file.'
