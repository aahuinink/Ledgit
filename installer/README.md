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

## What it deliberately does not do

- **No admin requirement.** It installs per-user unless you elevate it. A
  personal budget has no business demanding administrator rights.
- **No bundled runtime.** SQLite is compiled into the binaries and eframe draws
  through OpenGL, so there is no Visual C++ redistributable, no .NET, and no
  WebView2 to chase.
- **No code signing.** Unsigned installers get a SmartScreen warning. Signing
  needs a certificate you have to buy; when you want one, add `SignTool` to the
  `[Setup]` section.
