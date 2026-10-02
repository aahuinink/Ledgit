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
pub mod merge;
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

/// Stage the reversal of one entry, saying how it went.
pub fn stage_reversal(s: &mut crate::app::Session, tx: ledgit_core::id::TxUid) {
    match s.repo.reverse_transaction(tx) {
        Ok(_) => s.note("Staged its reversal. Review it on the Commit screen."),
        Err(e) => s.fail(e),
    }
}

/// Which entries can be reversed, worked out once per frame for a list:
/// not a reversal, and not reversed already.
pub struct Reversible {
    live: Vec<bool>,
}

impl Reversible {
    pub fn of(l: &ledgit_core::state::Budget) -> Reversible {
        let mut live = vec![false; l.transactions.len()];
        for ix in l.live_transactions() {
            live[ix.get()] = true;
        }
        Reversible { live }
    }

    /// The cell at the end of an entry's row: "Reverse", or why not.
    /// Returns the entry to reverse when the button is pressed.
    pub fn cell(
        &self,
        ui: &mut Ui,
        l: &ledgit_core::state::Budget,
        ix: ledgit_core::id::TxIx,
    ) -> Option<ledgit_core::id::TxUid> {
        let i = ix.get();
        if l.transactions.reverses[i].is_some() {
            ui.label(egui::RichText::new("reversal").small().color(crate::fmt::dim()));
            return None;
        }
        if !self.live.get(i).copied().unwrap_or(true) {
            ui.label(egui::RichText::new("reversed").small().color(crate::fmt::dim()));
            return None;
        }
        ui.small_button("Reverse")
            .on_hover_text("Stage its reversal - every side negated. Both stay on record; review it on the Commit screen.")
            .clicked()
            .then(|| l.transactions.uid[i])
    }
}

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
