//! The commit screen: the report of everything you have done and every bucket
//! it touches, and the one button that makes it permanent.

use super::{empty, heading, num};
use crate::app::Session;
use crate::fmt;
use crate::table::{figures, text, Height, Table};
use egui::{RichText, Ui};
use ledgit_core::prelude::*;

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(
        ui,
        "Commit",
        "Nothing above this screen is permanent yet. This is where it becomes history.",
    );

    let report = match s.repo.report() {
        Ok(r) => r,
        Err(e) => {
            ui.colored_label(fmt::bad(), format!("Could not build the report: {e}"));
            return;
        }
    };

    if report.is_empty() {
        empty(ui, "Nothing staged. Everything you can see is already committed.");
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        summary(ui, &report);
        ui.add_space(14.0);
        staged_list(ui, s);
        ui.add_space(14.0);
        ledgers_table(ui, &report);
        ui.add_space(14.0);
        buckets_table(ui, &report);
        if !report.alerts.is_empty() {
            ui.add_space(14.0);
            ui.label(RichText::new("ALERTS THIS SETS OFF").small().color(fmt::dim()));
            ui.add_space(4.0);
            super::goals::fired_list(ui, s, &report.alerts);
        }
        ui.add_space(18.0);
        commit_box(ui, s, &report);
    });
}

fn summary(ui: &mut Ui, r: &ChangeReport) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            tile(
                ui,
                "manual entries",
                &r.manual_transactions.to_string(),
                &fmt::amount(r.total_manual),
            );
            tile(
                ui,
                "from issuers",
                &r.issuer_transactions.to_string(),
                &fmt::amount(r.total_issued),
            );
            tile(ui, "new ledgers", &r.new_ledgers.to_string(), "permanent once committed");
            tile(ui, "new issuers", &r.new_issuers.to_string(), "");
            tile(
                ui,
                "buckets",
                &format!("+{} / -{}", r.new_buckets, r.deleted_buckets),
                "views only",
            );
            if r.new_cohorts + r.deleted_cohorts + r.new_views + r.deleted_views > 0 {
                tile(
                    ui,
                    "cohorts / views",
                    &format!(
                        "+{} / -{}  \u{b7}  +{} / -{}",
                        r.new_cohorts, r.deleted_cohorts, r.new_views, r.deleted_views
                    ),
                    "readings only",
                );
            }
        });
    });
    if !r.balanced {
        ui.add_space(8.0);
        ui.colored_label(
            fmt::bad(),
            "Debits do not equal credits. Committing is blocked - this is a bug in Ledgit, please report it.",
        );
    }
}

fn tile(ui: &mut Ui, label: &str, value: &str, sub: &str) {
    ui.vertical(|ui| {
        ui.set_min_width(130.0);
        ui.label(RichText::new(label).small().color(fmt::dim()));
        ui.label(RichText::new(value).size(20.0));
        if !sub.is_empty() {
            ui.label(RichText::new(sub).small().color(fmt::dim()));
        }
    });
}

fn staged_list(ui: &mut Ui, s: &mut Session) {
    ui.label(RichText::new("STAGED CHANGES").small().color(fmt::dim()));
    let broken = s.repo.broken().to_vec();
    if !broken.is_empty() {
        ui.colored_label(
            fmt::bad(),
            format!(
                "{} change(s) below no longer apply - usually because something they use was \
                 dropped. They are left out of the totals, and nothing can be committed until \
                 each is edited or dropped.",
                broken.len()
            ),
        );
    }
    ui.add_space(4.0);
    let mut drop: Option<usize> = None;
    let mut edit: Option<usize> = None;
    let row = crate::table::row_height_for(ui);
    let heights: Vec<f32> = (0..s.repo.staged().len())
        .map(|i| if broken.iter().any(|b| b.index == i) { row * 2.0 } else { row })
        .collect();
    let staged = s.repo.staged();
    // Its own scroll past a screenful, so a long issuer run does not bury
    // the report and the commit box under it.
    Table::new("staged", vec![text(""), text("change").max(640.0), text(""), text("")])
        .height(Height::Max(360.0))
        .row_heights(heights)
        .fit_to(broken.len())
        .show(ui, 0, |row| {
            let i = row.index();
            let op = &staged[i];
            row.col(|ui| {
                ui.label(RichText::new(format!("{}.", i + 1)).color(fmt::dim()).monospace());
            });
            row.col(|ui| match broken.iter().find(|b| b.index == i) {
                Some(b) => {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("\u{26A0} {}", op.summary())).color(fmt::bad()),
                        );
                        ui.label(RichText::new(&b.reason).small().color(fmt::bad()));
                    });
                }
                None => {
                    ui.label(op.summary());
                }
            });
            row.col(|ui| {
                if editable(op) && ui.small_button("edit").clicked() {
                    edit = Some(i);
                }
            });
            row.col(|ui| {
                if ui.small_button("drop").clicked() {
                    drop = Some(i);
                }
            });
        });
    if let Some(i) = edit {
        let op = s.repo.staged()[i].clone();
        s.forms.edit_staged(i, &op);
    }
    if let Some(i) = drop {
        match s.repo.unstage_at(i) {
            Ok(op) => {
                let n = s.repo.broken().len();
                if n > 0 {
                    s.fail(format!(
                        "Dropped: {}. {n} staged change(s) no longer apply; fix or drop them before committing.",
                        op.summary()
                    ));
                } else {
                    s.note(format!("Dropped: {}", op.summary()));
                }
            }
            Err(e) => s.fail(e),
        }
    }
}

/// Changes that have a form to reopen them in.
fn editable(op: &Op) -> bool {
    matches!(
        op,
        Op::CreateLedger { .. }
            | Op::PostTransaction { .. }
            | Op::CreateIssuer { .. }
            | Op::CreateBucket { .. }
            | Op::CreateCohort { .. }
            | Op::CreateView { .. }
    )
}

fn ledgers_table(ui: &mut Ui, r: &ChangeReport) {
    if r.ledger_deltas.is_empty() {
        return;
    }
    ui.label(RichText::new("LEDGERS AFFECTED").small().color(fmt::dim()));
    ui.add_space(4.0);
    Table::new(
        "report_ledgers",
        vec![
            text("ledger").max(420.0),
            figures("before"),
            figures("after"),
            figures("change"),
            figures("entries"),
        ],
    )
    .show(ui, r.ledger_deltas.len(), |row| {
        let d = &r.ledger_deltas[row.index()];
        row.col(|ui| {
            let name = RichText::new(&d.name);
            let label = ui.label(if d.is_new { name.italics() } else { name });
            if d.is_new {
                label.on_hover_text("new ledger");
            }
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(d.before)));
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(d.after)));
        });
        row.col(|ui| {
            num(ui, fmt::delta_text(d.change()));
        });
        row.col(|ui| {
            num(ui, RichText::new(d.postings.to_string()).color(fmt::dim()));
        });
    });
}

fn buckets_table(ui: &mut Ui, r: &ChangeReport) {
    if r.bucket_effects.is_empty() {
        return;
    }
    ui.label(RichText::new("BUCKETS THAT MAY BE AFFECTED").small().color(fmt::dim()));
    ui.label(
        RichText::new(
            "A bucket is listed when it holds a ledger you posted against, even if its total \
             happens to net to zero.",
        )
        .small()
        .color(fmt::dim()),
    );
    ui.add_space(4.0);
    Table::new(
        "report_buckets",
        vec![
            text("bucket").max(360.0),
            figures("before"),
            figures("after"),
            figures("change"),
            text(""),
        ],
    )
    .show(ui, r.bucket_effects.len(), |row| {
        let b = &r.bucket_effects[row.index()];
        row.col(|ui| {
            ui.label(&b.name);
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(b.before)));
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(b.after)));
        });
        row.col(|ui| {
            num(ui, fmt::delta_text(b.change()));
        });
        row.col(|ui| {
            if b.membership_changed {
                ui.label(RichText::new("membership changed").small().color(fmt::dim()));
            }
        });
    });
}

fn commit_box(ui: &mut Ui, s: &mut Session, r: &ChangeReport) {
    ui.separator();
    ui.add_space(8.0);
    ui.label("Describe what this commit does");
    ui.add(
        egui::TextEdit::multiline(&mut s.commit_message)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .hint_text("january pay and the car payment"),
    );
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        let can_commit = r.can_commit() && !s.commit_message.trim().is_empty();
        if ui
            .add_enabled(can_commit, egui::Button::new("Commit"))
            .on_disabled_hover_text(if !r.broken.is_empty() {
                "Blocked: some staged changes no longer apply. Edit or drop them first."
            } else if r.balanced {
                "Write a message first"
            } else {
                "Blocked: the budget does not balance"
            })
            .clicked()
        {
            let message = s.commit_message.clone();
            match s.repo.commit(message) {
                Ok(id) => {
                    s.commit_message.clear();
                    s.note(format!("Committed {}.", id.short()));
                }
                Err(e) => s.fail(e),
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Discard everything staged")
                .on_hover_text(
                    "Throws away every change on this screen. Committed history is untouched.",
                )
                .clicked()
            {
                let n = s.repo.staged().len();
                match s.repo.clear_stage() {
                    Ok(()) => s.note(format!("Discarded {n} staged change(s).")),
                    Err(e) => s.fail(e),
                }
            }
        });
    });
}
