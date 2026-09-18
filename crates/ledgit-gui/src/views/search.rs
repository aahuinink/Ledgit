//! Global search. Shown whenever the search box has text in it, over whatever
//! view is selected, because that is what a search bar is for.

use super::{empty, heading, num};
use crate::app::{Session, View};
use crate::fmt;
use egui::{RichText, Ui};

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
            egui::Grid::new("search_ledgers").num_columns(3).striped(true).show(ui, |ui| {
                for ix in &hits.ledgers {
                    let uid = s.budget().ledgers.uid[ix.get()];
                    if ui.link(&s.budget().ledgers.name[ix.get()]).clicked() {
                        s.selected_ledger = Some(uid);
                        s.goto = Some(View::Register);
                        s.search.clear();
                    }
                    ui.label(
                        RichText::new(s.budget().ledgers.normality[ix.get()].to_string())
                            .color(fmt::dim()),
                    );
                    num(ui, fmt::money_text(s.budget().ledgers.balance(*ix)));
                    ui.end_row();
                }
            });
            ui.add_space(12.0);
        }

        if !hits.transactions.is_empty() {
            ui.label(RichText::new("TRANSACTIONS").small().color(fmt::dim()));
            egui::Grid::new("search_tx").num_columns(4).striped(true).show(ui, |ui| {
                for ix in &hits.transactions {
                    let l = s.budget();
                    let i = ix.get();
                    ui.label(fmt::mono(l.transactions.date[i].to_string()));
                    ui.label(&l.transactions.name[i]);
                    let flow = fmt::flow(l, *ix);
                    ui.label(RichText::new(fmt::clip(&flow, 44)).color(fmt::dim()).small())
                        .on_hover_text(flow);
                    num(ui, fmt::money_text(l.amount_of(*ix)));
                    ui.end_row();
                }
            });
            ui.add_space(12.0);
        }

        if !hits.issuers.is_empty() {
            ui.label(RichText::new("ISSUERS").small().color(fmt::dim()));
            for ix in &hits.issuers {
                let name = s.budget().issuers.name[ix.get()].clone();
                if ui.link(name).clicked() {
                    s.goto = Some(View::Issuers);
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
                    s.goto = Some(View::Buckets);
                    s.search.clear();
                }
            }
        }
    });
}
