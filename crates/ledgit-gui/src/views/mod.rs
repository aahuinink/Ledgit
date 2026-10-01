//! The screens. Each one is a free function taking `&mut Ui` and `&mut
//! Session`; none of them own state, so navigating away and back is free and
//! there is exactly one copy of the truth.

pub mod buckets;
pub mod calendar;
pub mod commit;
pub mod dashboard;
pub mod goals;
pub mod graph;
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

pub use crate::table::num;

pub fn empty(ui: &mut Ui, message: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(message).color(fmt::dim()));
    });
}

/// A list beside its detail: the list on the left in a panel you can drag
/// wider or narrower, scrolling when it is long; the detail takes the rest.
/// `state` is handed to each side in turn, since both usually need the
/// session.
pub fn split<T>(
    ui: &mut Ui,
    id: &str,
    width: f32,
    state: &mut T,
    list: impl FnOnce(&mut Ui, &mut T),
    detail: impl FnOnce(&mut Ui, &mut T),
) {
    egui::SidePanel::left(ui.id().with(("split", id)))
        .resizable(true)
        // Wide enough to read on a big window, not so wide on a small one
        // that the detail is squeezed out; after that it is yours to drag.
        .default_width(width.min(ui.available_width() * 0.35))
        .width_range(120.0..=(width * 2.0).max(240.0))
        .frame(egui::Frame::NONE.inner_margin(egui::Margin { right: 6, ..Default::default() }))
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("split_list", id))
                .auto_shrink([false, false])
                .show(ui, |ui| list(ui, state));
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 8, ..Default::default() }))
        .show_inside(ui, |ui| detail(ui, state));
}
