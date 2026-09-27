# Put a fresh build over an installed Ledgit, without running the installer.
#
#   powershell -ExecutionPolicy Bypass -File installer\update.ps1
#   powershell -ExecutionPolicy Bypass -File installer\update.ps1 -Test
#
# The Start menu shortcut, the desktop shortcut and double-clicking a .ledgit
# file all run the installed copy, not target\release. This builds release
# binaries and copies them over that copy, so those all pick up the new build.
# Re-run the real installer (build.ps1) only when ledgit.iss itself changes.
#
#   -Test    run the test suite first, and stop if it fails.
#   -NoBuild copy what is already in target\release.

param(
    [switch]$Test,
    [switch]$NoBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$exes = @('ledgit-gui.exe', 'ledgit.exe')

# The AppId from ledgit.iss. Inno Setup records where it installed under this
# key: HKCU for a per-user install, HKLM for a per-machine one.
$appId = '{08c002af-6c79-4592-8c94-a350b0f0ad7a}_is1'

function Find-InstallDir {
    foreach ($hive in 'HKCU:', 'HKLM:') {
        $key = "$hive\Software\Microsoft\Windows\CurrentVersion\Uninstall\$appId"
        $entry = Get-ItemProperty -Path $key -ErrorAction SilentlyContinue
        if ($entry -and $entry.InstallLocation -and (Test-Path $entry.InstallLocation)) {
            return $entry.InstallLocation.TrimEnd('\')
        }
    }
    # No uninstall entry (say, an installer from before AppId was fixed):
    # try where a per-user install goes by default.
    $guess = Join-Path $env:LOCALAPPDATA 'Programs\Ledgit'
    if (Test-Path (Join-Path $guess 'ledgit-gui.exe')) { return $guess }
    return $null
}

$dir = Find-InstallDir
if (-not $dir) {
    throw 'Ledgit does not look installed. Run the installer once (installer\build.ps1), then use this for updates.'
}
Write-Host "Installed at $dir" -ForegroundColor Cyan

Push-Location $root
try {
    if ($Test) {
        Write-Host 'Running tests...' -ForegroundColor Cyan
        cargo test --release
        if ($LASTEXITCODE -ne 0) { throw 'tests failed - not installing a broken build' }
    }
    if (-not $NoBuild) {
        Write-Host 'Building release binaries...' -ForegroundColor Cyan
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    }
}
finally {
    Pop-Location
}

# Windows will not overwrite a running exe - and with only one Ledgit allowed,
# launching the new build while the old one runs just raises the old window.
# Ask it to close the way its close button would, so it saves its settings
# (pins, zoom) on the way out. Its staged work is already in the budget file.
$running = Get-Process -Name 'ledgit-gui' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($dir, [StringComparison]::OrdinalIgnoreCase) }
if ($running) {
    Write-Host 'Closing the running Ledgit...' -ForegroundColor Cyan
    foreach ($p in $running) { [void]$p.CloseMainWindow() }
    foreach ($p in $running) {
        if (-not $p.WaitForExit(10000)) {
            throw 'Ledgit did not close within 10 seconds. Close it yourself and run this again.'
        }
    }
}

foreach ($exe in $exes) {
    $from = Join-Path $root "target\release\$exe"
    if (-not (Test-Path $from)) { throw "$from is missing - build first, or drop -NoBuild." }
    try {
        Copy-Item -Path $from -Destination $dir -Force
    }
    catch [System.UnauthorizedAccessException] {
        throw "Could not write to $dir. A per-machine install needs an elevated PowerShell for this."
    }
}
# The docs ship with the app too; keep them in step with the build.
Copy-Item -Path (Join-Path $root 'README.md') -Destination $dir -Force
Copy-Item -Path (Join-Path $root 'docs\*') -Destination (Join-Path $dir 'docs') -Recurse -Force

$built = (Get-Item (Join-Path $dir 'ledgit-gui.exe')).LastWriteTime
Write-Host "Updated. The Start menu Ledgit is now the build from $built." -ForegroundColor Green
if ($running) {
    $answer = Read-Host 'Open it again? [Y/n]'
    if ($answer -notmatch '^[nN]') { Start-Process (Join-Path $dir 'ledgit-gui.exe') }
}
