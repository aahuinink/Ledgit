# Build Ledgit and package it into an installer.
#
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1
#
# Needs: a Rust toolchain (rustup), the MSVC build tools, and Inno Setup 6
# (https://jrsoftware.org/isdl.php) with iscc.exe reachable.

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

Write-Host 'Building release binaries...' -ForegroundColor Cyan
Push-Location $root
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    cargo test --release
    if ($LASTEXITCODE -ne 0) { throw 'tests failed - not packaging a broken build' }
}
finally {
    Pop-Location
}

$iscc = Get-Command iscc.exe -ErrorAction SilentlyContinue
if (-not $iscc) {
    $guess = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'
    if (Test-Path $guess) {
        $iscc = $guess
    } else {
        throw 'ISCC.exe not found. Install Inno Setup 6, or add it to PATH.'
    }
}

Write-Host 'Packaging installer...' -ForegroundColor Cyan
& $iscc (Join-Path $PSScriptRoot 'ledgit.iss')
if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }

Write-Host "Done. Look in $root\target\installer" -ForegroundColor Green
