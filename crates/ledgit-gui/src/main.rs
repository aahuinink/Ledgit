//! Ledgit desktop app.
//!
//! Pass a budget file as the first argument, or set `LEDGIT_BUDGET`, or pick one
//! from the welcome screen.

// No console window behind the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

mod app;
mod brand;
mod datepick;
mod fmt;
mod forms;
mod instance;
mod picker;
#[cfg(test)]
mod smoke;
mod table;
mod textbox;
mod views;

use app::LedgitApp;

fn main() -> eframe::Result {
    let author = std::env::var("LEDGIT_AUTHOR").unwrap_or_else(|_| whoami());
    let initial = app::initial_path();

    // One Ledgit at a time: if one is running, it opens this budget instead.
    let user = whoami();
    let inbox = match instance::claim(instance::port_for(&user), &user, initial.as_deref()) {
        instance::Claim::Forwarded => return Ok(()),
        instance::Claim::Primary(inbox) => Some(inbox),
        instance::Claim::Unguarded => None,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Ledgit")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([720.0, 480.0])
            // The same artwork as the exe's icon; `ICON` in build.rs picks it.
            .with_icon(brand::window_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "Ledgit",
        options,
        Box::new(move |cc| {
            let app = LedgitApp::new(cc, initial, author);
            Ok(Box::new(match inbox {
                Some(inbox) => app.with_inbox(inbox, &cc.egui_ctx),
                None => app,
            }))
        }),
    )
}

/// Best-effort user name for commit authorship, with no dependency on a crate
/// that reads the password database.
fn whoami() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "me".to_string())
}
