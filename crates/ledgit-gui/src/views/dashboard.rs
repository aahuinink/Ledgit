//! The screen you open the app to: what you are worth, what is pinned, what is
//! about to happen, and what you have not committed yet.

use super::{empty, heading, num};
use crate::app::{Screen, Session};
use crate::fmt;
use crate::forms::FormKind;
use crate::table::{figures, text, Table};
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
        alerts(ui, s);
        paces(ui, s);
        bucket_tiles(ui, s);
        ui.add_space(16.0);
        ui.columns(2, |cols| {
            pinned(&mut cols[0], s);
            upcoming(&mut cols[1], s);
        });
    });
}

/// Every alert past its level, at the top where it cannot be missed.
fn alerts(ui: &mut Ui, s: &mut Session) {
    let fired = ledgit_core::goals::fired(s.budget());
    if fired.is_empty() {
        return;
    }
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new("ALERTS").small().color(fmt::dim()));
        super::goals::fired_list(ui, s, &fired);
    });
    ui.add_space(12.0);
}

/// Every pace and how its week, month, ... is going.
fn paces(ui: &mut Ui, s: &mut Session) {
    let paces = ledgit_core::goals::paces(s.budget(), Date::today_utc());
    if paces.is_empty() {
        return;
    }
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new("PACES THIS PERIOD").small().color(fmt::dim()));
        super::goals::pace_list(ui, s, &paces);
    });
    ui.add_space(12.0);
}

fn pending_banner(ui: &mut Ui, s: &mut Session) {
    let staged = s.repo.staged().len();
    if staged == 0 {
        return;
    }
    egui::Frame::group(ui.style()).fill(ui.visuals().faint_bg_color).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{staged} change(s) not committed")).color(fmt::warn()));
            ui.label(
                RichText::new("Nothing below is permanent until you commit.")
                    .color(fmt::dim())
                    .small(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Review and commit").clicked() {
                    s.goto = Some(Screen::Commit);
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
                            s.goto = Some(Screen::Buckets);
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
    let mut open: Option<LedgerUid> = None;
    let l = s.budget();
    let pins: Vec<(LedgerUid, ledgit_core::id::LedgerIx)> =
        s.pins.iter().filter_map(|uid| l.ledgers.ix(*uid).map(|ix| (*uid, ix))).collect();
    Table::new("dash_pins", vec![text("ledger").max(300.0), figures("balance")]).show(
        ui,
        pins.len(),
        |row| {
            let (uid, ix) = pins[row.index()];
            row.col(|ui| {
                if ui.link(&l.ledgers.name[ix.get()]).clicked() {
                    open = Some(uid);
                }
            });
            row.col(|ui| {
                num(ui, fmt::money_text(l.ledgers.balance(ix)));
            });
        },
    );
    if let Some(uid) = open {
        s.selected_ledger = Some(uid);
        s.goto = Some(Screen::Register);
    }
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
                .map(|d| (d, l.issuers.name[ix.get()].clone(), issuer::estimate(l, ix)))
        })
        .collect();
    due.sort();

    if due.is_empty() {
        ui.label(RichText::new("No active issuers.").color(fmt::dim()).small());
        return;
    }

    let today = Date::today_utc();
    let shown = &due[..due.len().min(8)];
    Table::new("dash_due", vec![text("due"), text("issuer").max(260.0), figures("about")]).show(
        ui,
        shown.len(),
        |row| {
            let (date, name, amount) = &shown[row.index()];
            row.col(|ui| {
                let date_text = RichText::new(date.to_string()).monospace();
                ui.label(if *date <= today { date_text.color(fmt::bad()) } else { date_text });
            });
            row.col(|ui| {
                ui.label(name);
            });
            row.col(|ui| {
                num(ui, fmt::mono(fmt::amount(*amount)));
            });
        },
    );

    let overdue = due.iter().filter(|(d, _, _)| *d <= today).count();
    if overdue > 0 {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.colored_label(fmt::bad(), format!("{overdue} issuer(s) owe something"));
            if ui.small_button("Run issuers").clicked() {
                s.goto = Some(Screen::Issuers);
            }
        });
    }
}
