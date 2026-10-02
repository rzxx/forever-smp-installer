# Shared by development, tests and release packaging.
function Initialize-WindowsBuild {
    if (-not $IsWindows) { throw 'The desktop installer currently builds on Windows only.' }
    if ($env:GPUI_FXC_PATH) { return }
    $sdkBin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
    $compiler = Get-ChildItem -LiteralPath $sdkBin -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | ForEach-Object { Join-Path $_.FullName 'x64/fxc.exe' } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    if (-not $compiler) { throw 'Install the Windows SDK and MSVC build tools: GPUI needs fxc.exe.' }
    $env:GPUI_FXC_PATH = $compiler
}

function Get-CargoTargetDirectory {
    $metadata = & cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE) { throw 'Unable to read Cargo workspace metadata.' }
    return ($metadata | ConvertFrom-Json).target_directory
}
