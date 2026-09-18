//! The screen you open the app to: what you are worth, what is pinned, what is
//! about to happen, and what you have not committed yet.

use super::{empty, heading, num};
use crate::app::{Session, View};
use crate::fmt;
use crate::forms::FormKind;
use egui::{RichText, Ui};
use ledgit_core::issuer;
use ledgit_core::prelude::*;

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(ui, "Dashboard", "Everything here includes staged changes, not just committed ones.");

    if s.budget().ledgers.is_empty() {
        empty(ui, "No ledgers yet.");
        ui.vertical_centered(|ui| {
            if ui.button("Create your first ledger").clicked() {
                s.forms.open(FormKind::Ledger, s.repo.working());
            }
        });
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        pending_banner(ui, s);
        bucket_tiles(ui, s);
        ui.add_space(16.0);
        ui.columns(2, |cols| {
            pinned(&mut cols[0], s);
            upcoming(&mut cols[1], s);
        });
    });
}

fn pending_banner(ui: &mut Ui, s: &mut Session) {
    let staged = s.repo.staged().len();
    if staged == 0 {
        return;
    }
    egui::Frame::group(ui.style()).fill(ui.visuals().faint_bg_color).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{staged} change(s) not committed"))
                    .color(egui::Color32::from_rgb(220, 170, 60)),
            );
            ui.label(
                RichText::new("Nothing below is permanent until you commit.")
                    .color(fmt::dim())
                    .small(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Review and commit").clicked() {
                    s.goto = Some(View::Commit);
                }
            });
        });
    });
    ui.add_space(12.0);
}

fn bucket_tiles(ui: &mut Ui, s: &mut Session) {
    let buckets: Vec<BucketUid> =
        s.budget().buckets.live().map(|ix| s.budget().buckets.uid[ix.get()]).collect();
    if buckets.is_empty() {
        ui.horizontal(|ui| {
            ui.label(RichText::new("No buckets yet.").color(fmt::dim()));
            if ui.link("Create one").clicked() {
                s.forms.open(FormKind::Bucket, s.repo.working());
            }
            ui.label(
                RichText::new("- a bucket totals a group of ledgers, like net worth.")
                    .color(fmt::dim()),
            );
        });
        return;
    }

    ui.label(RichText::new("BUCKETS").small().color(fmt::dim()));
    ui.add_space(4.0);

    let tile_width = 220.0;
    let per_row = ((ui.available_width() / (tile_width + 12.0)).floor() as usize).max(1);
    for chunk in buckets.chunks(per_row) {
        ui.horizontal(|ui| {
            for uid in chunk {
                let Some(roll) =
                    roll_up(s.budget(), *uid, RollUp::ByNormality, LedgerSort::Name, Order::Asc)
                else {
                    continue;
                };
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(tile_width);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&roll.name).strong());
                        ui.label(fmt::money_text(roll.total).size(22.0));
                        ui.label(
                            RichText::new(format!("{} ledger(s)", roll.lines.len()))
                                .color(fmt::dim())
                                .small(),
                        );
                        if ui.small_button("open").clicked() {
                            s.selected_bucket = Some(*uid);
                            s.goto = Some(View::Buckets);
                        }
                    });
                });
            }
        });
        ui.add_space(8.0);
    }
}

fn pinned(ui: &mut Ui, s: &mut Session) {
    ui.label(RichText::new("PINNED LEDGERS").small().color(fmt::dim()));
    ui.add_space(4.0);
    if s.pins.is_empty() {
        ui.label(
            RichText::new("Pin a ledger from the Ledgers screen to keep it here.")
                .color(fmt::dim())
                .small(),
        );
        return;
    }
    let pins = s.pins.clone();
    egui::Grid::new("dash_pins").num_columns(2).striped(true).min_col_width(120.0).show(ui, |ui| {
        for uid in pins {
            let Some(ix) = s.budget().ledgers.ix(uid) else { continue };
            let name = s.budget().ledgers.name[ix.get()].clone();
            let balance = s.budget().ledgers.balance(ix);
            if ui.link(name).clicked() {
                s.selected_ledger = Some(uid);
                s.goto = Some(View::Register);
            }
            num(ui, fmt::money_text(balance));
            ui.end_row();
        }
    });
}

fn upcoming(ui: &mut Ui, s: &mut Session) {
    ui.label(RichText::new("COMING UP").small().color(fmt::dim()));
    ui.add_space(4.0);

    let l = s.budget();
    let mut due: Vec<(Date, String, Money)> = l
        .issuers
        .indices()
        .filter(|ix| !l.issuers.paused[ix.get()])
        .filter_map(|ix| {
            issuer::next_due(l, ix)
                .map(|d| (d, l.issuers.name[ix.get()].clone(), l.issuers.amount(ix)))
        })
        .collect();
    due.sort();

    if due.is_empty() {
        ui.label(RichText::new("No active issuers.").color(fmt::dim()).small());
        return;
    }

    let today = Date::today_utc();
    egui::Grid::new("dash_due").num_columns(3).striped(true).show(ui, |ui| {
        for (date, name, amount) in due.iter().take(8) {
            let overdue = *date <= today;
            let date_text = RichText::new(date.to_string()).monospace();
            ui.label(if overdue { date_text.color(fmt::bad()) } else { date_text });
            ui.label(name);
            num(ui, fmt::mono(fmt::amount(*amount)));
            ui.end_row();
        }
    });

    let overdue = due.iter().filter(|(d, _, _)| *d <= today).count();
    if overdue > 0 {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.colored_label(fmt::bad(), format!("{overdue} issuer(s) owe something"));
            if ui.small_button("Run issuers").clicked() {
                s.goto = Some(View::Issuers);
            }
        });
    }
}
