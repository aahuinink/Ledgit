//! The screens. Each one is a free function taking `&mut Ui` and `&mut
//! Session`; none of them own state, so navigating away and back is free and
//! there is exactly one copy of the truth.

pub mod buckets;
pub mod cohorts;
pub mod commit;
pub mod dashboard;
pub mod goals;
pub mod history;
pub mod issuers;
pub mod ledgers;
pub mod saved;
pub mod search;
pub mod transactions;
pub mod variables;

use crate::fmt;
use egui::{RichText, Ui};

/// Section heading with an optional subtitle underneath.
pub fn heading(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.add_space(6.0);
    ui.heading(title);
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).color(fmt::dim()));
    }
    ui.add_space(8.0);
}

/// A right-aligned monospace cell, so decimal points line up down a column.
pub fn num(ui: &mut Ui, text: RichText) -> egui::Response {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(text)).inner
}

pub fn empty(ui: &mut Ui, message: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(message).color(fmt::dim()));
    });
}
