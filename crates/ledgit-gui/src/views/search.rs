//! Global search. Shown whenever the search box has text in it, over whatever
//! view is selected, because that is what a search bar is for.

use super::{empty, heading, num};
use crate::app::{Screen, Session};
use crate::fmt;
use crate::table::{figures, text, Height, Table};
use egui::{RichText, Ui};
use ledgit_core::prelude::LedgerUid;

pub fn show(ui: &mut Ui, s: &mut Session) {
    let needle = s.search.clone();
    heading(
        ui,
        &format!("Results for \u{201c}{needle}\u{201d}"),
        "Press Escape or clear the box to go back",
    );

    let hits = ledgit_core::query::search(s.budget(), &needle, 50);
    if hits.ledgers.is_empty()
        && hits.transactions.is_empty()
        && hits.issuers.is_empty()
        && hits.buckets.is_empty()
    {
        empty(ui, "Nothing matched.");
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        if !hits.ledgers.is_empty() {
            ui.label(RichText::new("LEDGERS").small().color(fmt::dim()));
            let mut open: Option<LedgerUid> = None;
            let l = s.budget();
            Table::new(
                "search_ledgers",
                vec![text("ledger").max(420.0), text("normal"), figures("balance")],
            )
            .height(Height::Max(300.0))
            .fit_to(&needle)
            .show(ui, hits.ledgers.len(), |row| {
                let ix = hits.ledgers[row.index()];
                row.col(|ui| {
                    if ui.link(&l.ledgers.name[ix.get()]).clicked() {
                        open = Some(l.ledgers.uid[ix.get()]);
                    }
                });
                row.col(|ui| {
                    ui.label(
                        RichText::new(l.ledgers.normality[ix.get()].to_string()).color(fmt::dim()),
                    );
                });
                row.col(|ui| {
                    num(ui, fmt::money_text(l.ledgers.balance(ix)));
                });
            });
            if let Some(uid) = open {
                s.selected_ledger = Some(uid);
                s.goto = Some(Screen::Register);
                s.search.clear();
            }
            ui.add_space(12.0);
        }

        if !hits.transactions.is_empty() {
            ui.label(RichText::new("TRANSACTIONS").small().color(fmt::dim()));
            let l = s.budget();
            Table::new(
                "search_tx",
                vec![
                    text("date"),
                    text("name").max(320.0),
                    text("flow").max(360.0),
                    figures("amount"),
                ],
            )
            .height(Height::Max(360.0))
            .fit_to(&needle)
            .show(ui, hits.transactions.len(), |row| {
                let ix = hits.transactions[row.index()];
                let i = ix.get();
                row.col(|ui| {
                    ui.label(fmt::mono(l.transactions.date[i].to_string()));
                });
                row.col(|ui| {
                    ui.label(&l.transactions.name[i]);
                });
                row.col(|ui| {
                    ui.label(RichText::new(fmt::flow(l, ix)).color(fmt::dim()).small());
                });
                row.col(|ui| {
                    num(ui, fmt::money_text(l.amount_of(ix)));
                });
            });
            ui.add_space(12.0);
        }

        if !hits.issuers.is_empty() {
            ui.label(RichText::new("ISSUERS").small().color(fmt::dim()));
            for ix in &hits.issuers {
                let name = s.budget().issuers.name[ix.get()].clone();
                if ui.link(name).clicked() {
                    s.goto = Some(Screen::Issuers);
                    s.search.clear();
                }
            }
            ui.add_space(12.0);
        }

        if !hits.buckets.is_empty() {
            ui.label(RichText::new("BUCKETS").small().color(fmt::dim()));
            for ix in &hits.buckets {
                let uid = s.budget().buckets.uid[ix.get()];
                let name = s.budget().buckets.name[ix.get()].clone();
                if ui.link(name).clicked() {
                    s.selected_bucket = Some(uid);
                    s.goto = Some(Screen::Buckets);
                    s.search.clear();
                }
            }
        }
    });
}
