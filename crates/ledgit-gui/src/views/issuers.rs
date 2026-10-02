//! Recurring payments, and the review step before they become real.

use super::{empty, heading, num};
use crate::app::Session;
use crate::fmt;
use crate::forms::FormKind;
use crate::table::{figures, text, Height, Table};
use egui::{RichText, Ui};
use ledgit_core::id::IssuerIx;
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
    let mut select: Option<IssuerUid> = None;

    let l = s.budget();
    let all: Vec<ledgit_core::id::IssuerIx> = l.issuers.indices().collect();
    let selected = s.selected_issuer.and_then(|u| l.issuers.ix(u));
    // With one picked, the list shares the screen with its upcoming payments.
    let height = match selected {
        Some(_) => Height::Max((ui.available_height() * 0.45).max(160.0)),
        None => Height::Fill,
    };
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
    .height(height)
    .show(ui, all.len(), |row| {
        let ix = all[row.index()];
        let i = ix.get();
        let paused = l.issuers.paused[i];
        let uid = l.issuers.uid[i];
        row.col(|ui| {
            let name = RichText::new(&l.issuers.name[i]);
            let name = if paused { name.color(fmt::dim()).strikethrough() } else { name };
            if ui
                .selectable_label(selected == Some(ix), name)
                .on_hover_text("Its upcoming payments, and amounts set ahead")
                .clicked()
            {
                select = Some(uid);
            }
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

    if let Some(uid) = select {
        s.selected_issuer = if s.selected_issuer == Some(uid) { None } else { Some(uid) };
        s.issuer_override = None;
    }
    if let Some(ix) = selected {
        ui.add_space(10.0);
        egui::ScrollArea::vertical().id_salt("issuer_upcoming").show(ui, |ui| upcoming(ui, s, ix));
    }

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

/// The selected issuer's next payments, priced as they will post, with a way
/// to set any of them ahead.
fn upcoming(ui: &mut Ui, s: &mut Session, ix: IssuerIx) {
    let l = s.budget();
    let i = ix.get();
    let uid = l.issuers.uid[i];
    ui.horizontal(|ui| {
        ui.heading(&l.issuers.name[i]);
        ui.label(RichText::new(l.issuers.schedule[i].describe()).color(fmt::dim()));
    });
    if let Some(t) = l.issuers.settles[i].and_then(|t| l.transactions.ix(t)) {
        ui.label(
            RichText::new(format!(
                "Pays off \"{}\" of {}.",
                l.transactions.name[t.get()],
                l.transactions.date[t.get()]
            ))
            .color(fmt::dim()),
        );
    }
    let next = issuer::upcoming(l, ix, 6);
    if next.is_empty() {
        empty(ui, "Nothing left for it to post.");
        return;
    }
    let statement = next.iter().any(|o| o.statement.is_some());
    let mut cols = vec![text("due"), figures("pays")];
    if statement {
        cols.extend([
            text("closed"),
            figures("statement"),
            figures("paid since"),
            figures("minimum"),
        ]);
    }
    cols.push(text("set ahead").max(320.0));
    let mut set: Option<(Date, Option<Money>)> = None;
    let mut draft = s.issuer_override.clone();
    let mut error: Option<String> = None;
    Table::new(("issuer_upcoming", uid), cols).show(ui, next.len(), |row| {
        let o = &next[row.index()];
        row.col(|ui| {
            ui.label(fmt::mono(o.date.to_string()));
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(o.amount())).strong());
        });
        if statement {
            let st = o.statement;
            row.col(|ui| {
                ui.label(st.map_or(String::new(), |st| st.closed.to_string()));
            });
            for m in [st.map(|st| st.balance), st.map(|st| st.paid_since), st.map(|st| st.minimum)]
            {
                row.col(|ui| {
                    num(ui, fmt::mono(m.map_or(String::new(), fmt::amount)).color(fmt::dim()));
                });
            }
        }
        row.col(|ui| match &mut draft {
            Some((d, text)) if *d == o.date => {
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(90.0));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.small_button("Set").clicked() || enter {
                        match Money::parse(text.trim()) {
                            Ok(m) if m.cents() > 0 => set = Some((o.date, Some(m))),
                            _ => error = Some("An amount above zero, like 300 or 300.00".into()),
                        }
                    }
                    if ui.small_button("Cancel").clicked() {
                        set = Some((o.date, o.set_ahead));
                    }
                });
            }
            _ => {
                ui.horizontal(|ui| {
                    if let Some(m) = o.set_ahead {
                        let raised = m < o.amount();
                        let t = RichText::new(fmt::amount(m)).monospace();
                        ui.label(if raised { t.color(fmt::bad()) } else { t }).on_hover_text(
                            if raised {
                                "Under the minimum, so the minimum is paid."
                            } else {
                                "Set ahead."
                            },
                        );
                        if ui.small_button("Clear").clicked() {
                            set = Some((o.date, None));
                        }
                    }
                    if ui
                        .small_button(if o.set_ahead.is_some() { "Change" } else { "Set amount" })
                        .clicked()
                    {
                        let start = o.set_ahead.unwrap_or(o.amount());
                        draft = Some((o.date, start.to_string()));
                    }
                });
            }
        });
    });
    ui.label(
        RichText::new(if statement {
            "Future statements are projections: the issuers' own payments and charges are \
             counted in, purchases you have not made yet are not. An amount under the minimum \
             is raised to it."
        } else {
            "An amount set ahead replaces what it would otherwise post that day."
        })
        .small()
        .color(fmt::dim()),
    );

    s.issuer_override = draft;
    if let Some(e) = error {
        s.fail(e);
    }
    if let Some((date, amount)) = set {
        s.issuer_override = None;
        let unchanged = next.iter().any(|o| o.date == date && o.set_ahead == amount);
        if !unchanged {
            let what = if amount.is_some() {
                "setting a payment ahead"
            } else {
                "clearing a payment set ahead"
            };
            s.stage(vec![Op::SetIssuerOverride { uid, date, amount }], what);
        }
    }
}
