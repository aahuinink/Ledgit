//! The commit screen: the report of everything you have done and every bucket
//! it touches, and the one button that makes it permanent.

use super::{empty, heading, num};
use crate::app::Session;
use crate::fmt;
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
    ui.add_space(4.0);
    let mut drop: Option<usize> = None;
    egui::Grid::new("staged").num_columns(3).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
        for (i, op) in s.repo.staged().iter().enumerate() {
            ui.label(RichText::new(format!("{}.", i + 1)).color(fmt::dim()).monospace());
            ui.label(op.summary());
            if ui.small_button("drop").clicked() {
                drop = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = drop {
        match s.repo.unstage_at(i) {
            Ok(op) => s.note(format!("Dropped: {}", op.summary())),
            Err(e) => s.fail(e),
        }
    }
}

fn ledgers_table(ui: &mut Ui, r: &ChangeReport) {
    if r.ledger_deltas.is_empty() {
        return;
    }
    ui.label(RichText::new("LEDGERS AFFECTED").small().color(fmt::dim()));
    ui.add_space(4.0);
    egui::Grid::new("report_ledgers").num_columns(5).striped(true).spacing([16.0, 4.0]).show(
        ui,
        |ui| {
            ui.label(RichText::new("ledger").small().color(fmt::dim()));
            for h in ["before", "after", "change", "entries"] {
                num(ui, RichText::new(h).small().color(fmt::dim()));
            }
            ui.end_row();

            for d in &r.ledger_deltas {
                let name = RichText::new(&d.name);
                ui.label(if d.is_new { name.italics() } else { name }).on_hover_text(if d.is_new {
                    "new ledger"
                } else {
                    ""
                });
                num(ui, fmt::mono(fmt::amount(d.before)));
                num(ui, fmt::mono(fmt::amount(d.after)));
                num(ui, fmt::delta_text(d.change()));
                num(ui, RichText::new(d.postings.to_string()).color(fmt::dim()));
                ui.end_row();
            }
        },
    );
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
    egui::Grid::new("report_buckets").num_columns(5).striped(true).spacing([16.0, 4.0]).show(
        ui,
        |ui| {
            ui.label(RichText::new("bucket").small().color(fmt::dim()));
            for h in ["before", "after", "change"] {
                num(ui, RichText::new(h).small().color(fmt::dim()));
            }
            ui.label("");
            ui.end_row();

            for b in &r.bucket_effects {
                ui.label(&b.name);
                num(ui, fmt::mono(fmt::amount(b.before)));
                num(ui, fmt::mono(fmt::amount(b.after)));
                num(ui, fmt::delta_text(b.change()));
                ui.label(if b.membership_changed {
                    RichText::new("membership changed").small().color(fmt::dim())
                } else {
                    RichText::new("")
                });
                ui.end_row();
            }
        },
    );
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
        let can_commit = r.balanced && !s.commit_message.trim().is_empty();
        if ui
            .add_enabled(can_commit, egui::Button::new("Commit"))
            .on_disabled_hover_text(if r.balanced {
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
