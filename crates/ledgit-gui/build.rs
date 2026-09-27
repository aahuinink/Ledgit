//! Choose the app icon, and embed it in `ledgit-gui.exe`.
//!
//! Explorer, the Start menu, the installer's shortcuts and the `.ledgit` file
//! association read the icon *resource* inside the exe, which only a build
//! step can put there. The running window's icon (title bar, taskbar) is set
//! by the app from the same file, which this script names for it through the
//! `LEDGIT_APP_ICON` environment variable - so the two can never disagree.
//!
//! Only when building for Windows. If the resource compiler cannot be found
//! (no Windows SDK, or a cross build without `windres`), the build warns and
//! carries on without the icon rather than failing: a missing icon is not
//! worth a broken build.

#[path = "build/ico.rs"]
mod ico;

use std::path::PathBuf;

/// The app icon, everywhere Windows shows one: Explorer, the Start menu, the
/// taskbar and the title bar. Windows reads a single icon for all of them, so
/// this picks one. `Icon.svg` reads on light surfaces, `Icon_dark.svg` on dark.
///
/// This is the only place to change it; the running app follows.
const ICON: &str = "../../assets/Icon_dark.svg";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/ico.rs");
    println!("cargo:rerun-if-changed={ICON}");

    // For every target: the app includes this file as its window icon.
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    // Joined rather than canonicalised: on Windows `canonicalize` returns a
    // `\\?\` path, which is more than `include_bytes!` needs to be trusted with.
    let icon = manifest.join(ICON);
    assert!(icon.exists(), "{} is not in assets/", icon.display());
    println!("cargo:rustc-env=LEDGIT_APP_ICON={}", icon.display());

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let svg = std::fs::read(&icon).expect("the icon SVG is readable");
    let out =
        PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("ledgit.ico");
    std::fs::write(&out, ico::ico(&svg)).expect("OUT_DIR is writable");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(out.to_str().expect("OUT_DIR is valid UTF-8"));
    if let Err(e) = res.compile() {
        println!("cargo:warning=ledgit-gui.exe will have no icon: the Windows resource compiler failed ({e})");
    }
}
