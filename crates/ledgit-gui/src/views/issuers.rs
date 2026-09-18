//! Recurring payments, and the review step before they become real.

use super::{empty, heading, num};
use crate::app::Session;
use crate::fmt;
use crate::forms::FormKind;
use egui::{RichText, Ui};
use ledgit_core::issuer;
use ledgit_core::prelude::*;

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(
        ui,
        "Issuers",
        "An issuer proposes transactions on a schedule. Nothing it produces is posted until you commit it.",
    );

    ui.horizontal(|ui| {
        if ui.button("New issuer").clicked() {
            s.forms.open(FormKind::Issuer, s.repo.working());
        }
        ui.separator();
        ui.label("Run everything due through");
        ui.add(egui::TextEdit::singleline(&mut s.issuer_through).desired_width(110.0));
        if ui.button("Stage what is owed").clicked() {
            run_issuers(s);
        }
    });
    ui.add_space(8.0);

    if s.budget().issuers.is_empty() {
        empty(ui, "No issuers yet.");
        return;
    }

    let today = Date::today_utc();
    let mut toggles: Vec<(IssuerUid, bool)> = Vec::new();

    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("issuers").num_columns(6).striped(true).spacing([16.0, 6.0]).show(
            ui,
            |ui| {
                for h in ["issuer", "schedule", "moves", "next due", ""] {
                    ui.label(RichText::new(h).small().color(fmt::dim()));
                }
                num(ui, RichText::new("amount").small().color(fmt::dim()));
                ui.end_row();

                let l = s.budget();
                for ix in l.issuers.indices() {
                    let i = ix.get();
                    let paused = l.issuers.paused[i];
                    let uid = l.issuers.uid[i];

                    let name = RichText::new(&l.issuers.name[i]);
                    ui.label(if paused { name.color(fmt::dim()).strikethrough() } else { name });
                    ui.label(RichText::new(l.issuers.schedule[i].describe()).color(fmt::dim()));
                    ui.label(RichText::new(fmt::issuer_flow(l, ix)).small().color(fmt::dim()))
                        .on_hover_text(if l.issuers.legs[i].len() > 2 {
                            format!("posts a split entry of {} sides", l.issuers.legs[i].len())
                        } else {
                            String::new()
                        });

                    match (paused, issuer::next_due(l, ix)) {
                        (true, _) => ui.label(RichText::new("paused").color(fmt::dim())),
                        (false, None) => ui.label(RichText::new("finished").color(fmt::dim())),
                        (false, Some(d)) => {
                            let t = fmt::mono(d.to_string());
                            ui.label(if d <= today { t.color(fmt::bad()) } else { t })
                        }
                    };

                    if ui.button(if paused { "Resume" } else { "Pause" }).clicked() {
                        toggles.push((uid, !paused));
                    }
                    num(ui, fmt::mono(fmt::amount(l.issuers.amount(ix))));
                    ui.end_row();
                }
            },
        );
    });

    for (uid, paused) in toggles {
        let ops = vec![Op::SetIssuerPaused { uid, paused }];
        s.stage(ops, if paused { "pausing an issuer" } else { "resuming an issuer" });
    }
}

fn run_issuers(s: &mut Session) {
    let through = match s.issuer_through.parse::<Date>() {
        Ok(d) => d,
        Err(_) => {
            s.fail("That date must be YYYY-MM-DD");
            return;
        }
    };
    match s.repo.run_issuers(through) {
        Err(e) => s.fail(e),
        Ok(runs) if runs.is_empty() => s.note(format!("Nothing is owed through {through}.")),
        Ok(runs) => {
            let total: usize = runs.iter().map(|r| r.dates.len()).sum();
            s.note(format!(
                "Staged {total} transaction(s) from {} issuer(s). Review them on the Commit screen.",
                runs.len()
            ));
        }
    }
}
