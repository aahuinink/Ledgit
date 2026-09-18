//! Ledgit desktop app.
//!
//! Pass a budget file as the first argument, or set `LEDGIT_BUDGET`, or pick one
//! from the welcome screen.

// No console window behind the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

mod app;
mod fmt;
mod forms;
#[cfg(test)]
mod smoke;
mod views;

use app::LedgitApp;

fn main() -> eframe::Result {
    let author = std::env::var("LEDGIT_AUTHOR").unwrap_or_else(|_| whoami());
    let initial = app::initial_path();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Ledgit")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Ledgit",
        options,
        Box::new(move |cc| Ok(Box::new(LedgitApp::new(cc, initial, author)))),
    )
}

/// Best-effort user name for commit authorship, with no dependency on a crate
/// that reads the password database.
fn whoami() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "me".to_string())
}
