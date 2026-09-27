//! Embed the app icon in `ledgit-gui.exe`.
//!
//! The window icon is set at run time (`src/brand.rs`), but Explorer, the
//! Start menu, the installer's shortcuts and the `.ledgit` file association
//! all read the icon *resource* inside the exe, which only a build step can
//! put there.
//!
//! Only when building for Windows. If the resource compiler cannot be found
//! (no Windows SDK, or a cross build without `windres`), the build warns and
//! carries on without the icon rather than failing: a missing icon is not
//! worth a broken build.

#[path = "build/ico.rs"]
mod ico;

use std::path::PathBuf;

/// Which artwork the exe carries. Unlike the window icon it cannot follow
/// the system theme - Windows reads one icon for Explorer, the Start menu and
/// the taskbar alike - so this picks one. The original reads on light
/// surfaces; `Icon_dark.svg` on dark ones.
const ICON: &str = "../../assets/Icon.svg";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/ico.rs");
    println!("cargo:rerun-if-changed={ICON}");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let svg = std::fs::read(ICON).expect("the icon SVG is in assets/");
    let out =
        PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("ledgit.ico");
    std::fs::write(&out, ico::ico(&svg)).expect("OUT_DIR is writable");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(out.to_str().expect("OUT_DIR is valid UTF-8"));
    if let Err(e) = res.compile() {
        println!("cargo:warning=ledgit-gui.exe will have no icon: the Windows resource compiler failed ({e})");
    }
}
