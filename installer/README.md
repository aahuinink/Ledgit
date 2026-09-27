# Installer

Produces `Ledgit-<version>-setup.exe`, which installs:

- `ledgit.exe` - the desktop app
- `ledgit.exe` - the command line tool (optionally added to `PATH`)
- a Start Menu entry, an optional desktop shortcut
- a `.ledgit` file association, so double-clicking a budget opens it

## Build it

On Windows, with [Inno Setup 6](https://jrsoftware.org/isdl.php) installed:

```powershell
powershell -ExecutionPolicy Bypass -File installer\build.ps1
```

The script builds release binaries, runs the tests, and refuses to package a
build whose tests fail. Output lands in `target\installer\`.

## Update an installed copy

The Start menu and desktop shortcuts, and double-clicking a `.ledgit` file,
all run the *installed* copy, not `target\release`. To put a new build there
without running the installer again:

```powershell
powershell -ExecutionPolicy Bypass -File installer\update.ps1          # build and copy
powershell -ExecutionPolicy Bypass -File installer\update.ps1 -Test    # run the tests first
powershell -ExecutionPolicy Bypass -File installer\update.ps1 -NoBuild # copy what is built
```

It finds the install from the uninstall entry Inno Setup wrote, builds, asks
a running Ledgit to close (so it saves its settings; staged work is already in
the budget file), copies both exes and the docs over, and offers to reopen it.
A per-machine install under Program Files needs an elevated PowerShell.

Run the full installer again only when `ledgit.iss` changes - new shortcuts,
file associations or registry entries are its job, not this script's.

## What it deliberately does not do

- **No admin requirement.** It installs per-user unless you elevate it. A
  personal budget has no business demanding administrator rights.
- **No bundled runtime.** SQLite is compiled into the binaries and eframe draws
  through OpenGL, so there is no Visual C++ redistributable, no .NET, and no
  WebView2 to chase.
- **No code signing.** Unsigned installers get a SmartScreen warning. Signing
  needs a certificate you have to buy; when you want one, add `SignTool` to the
  `[Setup]` section.
