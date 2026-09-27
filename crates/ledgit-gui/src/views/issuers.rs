//! Recurring payments, and the review step before they become real.

use super::{empty, heading, num};
use crate::app::Session;
use crate::fmt;
use crate::forms::FormKind;
use crate::table::{figures, text, Height, Table};
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
        crate::datepick::DateField::new("issuer_through", &mut s.issuer_through).show(ui);
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

    let l = s.budget();
    let all: Vec<ledgit_core::id::IssuerIx> = l.issuers.indices().collect();
    Table::new(
        "issuers",
        vec![
            text("issuer").max(300.0),
            text("schedule").max(260.0),
            text("moves").max(380.0),
            text("next due"),
            text("").narrow(),
            figures("amount"),
        ],
    )
    .height(Height::Fill)
    .show(ui, all.len(), |row| {
        let ix = all[row.index()];
        let i = ix.get();
        let paused = l.issuers.paused[i];
        let uid = l.issuers.uid[i];
        row.col(|ui| {
            let name = RichText::new(&l.issuers.name[i]);
            ui.label(if paused { name.color(fmt::dim()).strikethrough() } else { name });
        });
        row.col(|ui| {
            ui.label(RichText::new(l.issuers.schedule[i].describe()).color(fmt::dim()));
        });
        row.col(|ui| {
            let flow = match fmt::issuer_rule(l, ix) {
                Some(rule) => format!("{}  ({rule})", fmt::issuer_flow(l, ix)),
                None => fmt::issuer_flow(l, ix),
            };
            let r = ui.label(RichText::new(flow).small().color(fmt::dim()));
            if l.issuers.legs[i].len() > 2 {
                r.on_hover_text(format!(
                    "posts a split entry of {} sides",
                    l.issuers.legs[i].len()
                ));
            }
        });
        row.col(|ui| {
            match (paused, issuer::next_due(l, ix)) {
                (true, _) => ui.label(RichText::new("paused").color(fmt::dim())),
                (false, None) => ui.label(RichText::new("finished").color(fmt::dim())),
                (false, Some(d)) => {
                    let t = fmt::mono(d.to_string());
                    ui.label(if d <= today { t.color(fmt::bad()) } else { t })
                }
            };
        });
        row.col(|ui| {
            if ui.small_button(if paused { "Resume" } else { "Pause" }).clicked() {
                toggles.push((uid, !paused));
            }
        });
        row.col(|ui| {
            match fmt::issuer_rule(l, ix) {
                Some(rule) => num(
                    ui,
                    RichText::new(format!("~{}", fmt::amount(issuer::estimate(l, ix)))).monospace(),
                )
                .on_hover_text(format!(
                    "{rule}. Worked out each time it fires; this is what the next one \
                     comes to on today's balance."
                )),
                None => num(ui, fmt::mono(fmt::amount(l.issuers.amount(ix)))),
            };
        });
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
