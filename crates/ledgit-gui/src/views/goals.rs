//! Targets and alerts, wherever they show: a ledger's own page, the
//! dashboard's alert list, and the commit report.

use super::num;
use crate::app::{Screen, Session};
use crate::fmt;
use egui::{RichText, Ui};
use ledgit_core::expr::eval_money;
use ledgit_core::goals::{self, FiredAlert};
use ledgit_core::prelude::*;

/// A ledger's target and alerts as they are being edited.
#[derive(Clone, Default)]
pub struct GoalsDraft {
    pub ledger: Option<LedgerUid>,
    pub target: String,
    pub alerts: Vec<AlertRow>,
}

#[derive(Clone)]
pub struct AlertRow {
    pub when: AlertWhen,
    pub level: String,
    pub message: String,
}

impl GoalsDraft {
    fn load(l: &Budget, ix: ledgit_core::id::LedgerIx) -> GoalsDraft {
        GoalsDraft {
            ledger: Some(l.ledgers.uid[ix.get()]),
            target: l.ledgers.target[ix.get()].map(|t| t.to_string()).unwrap_or_default(),
            alerts: l.ledgers.alerts[ix.get()]
                .iter()
                .map(|a| AlertRow {
                    when: a.when,
                    level: a.level.to_string(),
                    message: a.message.clone(),
                })
                .collect(),
        }
    }

    fn finish(
        &self,
        vars: &ledgit_core::state::VarArena,
    ) -> std::result::Result<(Option<Money>, Vec<Alert>), String> {
        let target = match self.target.trim() {
            "" => None,
            t => Some(eval_money(t, vars).map_err(|e| format!("Target: {e}"))?),
        };
        let alerts = self
            .alerts
            .iter()
            .enumerate()
            .map(|(i, a)| {
                Ok(Alert {
                    when: a.when,
                    level: eval_money(a.level.trim(), vars)
                        .map_err(|e| format!("Alert {}: {e}", i + 1))?,
                    message: a.message.trim().to_string(),
                })
            })
            .collect::<std::result::Result<Vec<_>, String>>()?;
        Ok((target, alerts))
    }
}

/// The "Target and alerts" section of a ledger's page.
pub fn ledger_section(ui: &mut Ui, s: &mut Session, uid: LedgerUid) {
    let Some(ix) = s.budget().ledgers.ix(uid) else { return };
    if s.goals_draft.ledger != Some(uid) {
        s.goals_draft = GoalsDraft::load(s.budget(), ix);
    }
    let l = s.budget();
    let balance = l.ledgers.balance(ix);
    let target = l.ledgers.target[ix.get()];
    let alerts = l.ledgers.alerts[ix.get()].clone();

    let title = match target {
        Some(t) => format!("Target {}  \u{b7}  {} alert(s)", fmt::amount(t), alerts.len()),
        None => format!("Target and alerts  \u{b7}  {} alert(s)", alerts.len()),
    };
    egui::CollapsingHeader::new(title)
        .id_salt("ledger_goals")
        .default_open(target.is_some() || !alerts.is_empty())
        .show(ui, |ui| {
            for a in alerts.iter().filter(|a| a.fires(balance)) {
                ui.colored_label(
                    fmt::bad(),
                    format!(
                        "\u{26A0} {} ({} {})",
                        message_or_level(a),
                        a.when,
                        fmt::amount(a.level)
                    ),
                );
            }

            if let Some(t) = target {
                ui.horizontal(|ui| {
                    let left = Money((t.0 - balance.0).abs());
                    if left.is_zero() {
                        ui.colored_label(fmt::good(), "At its target.");
                    } else {
                        ui.label(format!(
                            "{} to go to reach {}.",
                            fmt::amount(left),
                            fmt::amount(t)
                        ));
                    }
                    ui.separator();
                    ui.checkbox(&mut s.show_target_reached, "Show when the target is reached")
                        .on_hover_text(
                        "Works it out from the history and a simulation of the running issuers.",
                    );
                });
                if s.show_target_reached {
                    target_reached(ui, s, uid, t);
                }
            }

            ui.add_space(6.0);
            editor(ui, s, uid);
        });
}

/// When the balance reached the target, or when the issuers will get it there.
fn target_reached(ui: &mut Ui, s: &mut Session, uid: LedgerUid, target: Money) {
    let l = s.budget();
    let Some(ix) = l.ledgers.ix(uid) else { return };
    let today = Date::today_utc();
    let years = s.target_horizon_years.clamp(1, 50);
    let opened = l.ledgers.opened[ix.get()];
    let lookback_days = (today.0 - opened.0).max(1) as u32;
    let spec = ViewSpec {
        ledgers: vec![uid],
        lookback: Span::Days(lookback_days.min(365 * 50)),
        horizon: Span::Years(years),
        ..ViewSpec::default()
    };
    let report = ledgit_core::view::evaluate(l, &spec, today);
    let Some(series) = report.series.first() else { return };

    let text = match series.target_reached {
        Some(d) if d == today => {
            // Already there: say since when, from the history.
            let first = series.points.first().map(|(_, v)| *v).unwrap_or_default();
            let since = series
                .points
                .iter()
                .find(|(_, v)| goals::is_reached(first, target, *v))
                .map(|(d, _)| *d);
            match since {
                Some(d) if d < today => format!("Target reached on {d}."),
                _ => "Target reached.".to_string(),
            }
        }
        Some(d) => {
            let months = (d.0 - today.0) as f32 / 30.44;
            format!(
                "Projected to reach its target on {d} - in about {months:.0} month(s) - with the \
                 issuers as they are."
            )
        }
        None => format!("Not reached within {years} year(s) at the current issuers."),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(text).strong());
        ui.label(RichText::new("look ahead").small().color(fmt::dim()));
        ui.add(egui::DragValue::new(&mut s.target_horizon_years).range(1..=50).suffix(" y"));
    });
    for (a, d) in &series.alerts_ahead {
        ui.label(
            RichText::new(format!(
                "\u{26A0} {d}: {} ({} {})",
                message_or_level(a),
                a.when,
                fmt::amount(a.level)
            ))
            .color(fmt::dim()),
        );
    }
}

fn editor(ui: &mut Ui, s: &mut Session, uid: LedgerUid) {
    let d = &mut s.goals_draft;
    let mut remove = None;
    egui::Grid::new("goals_editor").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Target");
        ui.add(egui::TextEdit::singleline(&mut d.target).hint_text("none").desired_width(110.0))
            .on_hover_text("The balance you want, as displayed. A formula works here too.");
        ui.label(RichText::new("a loan's target is usually 0").small().color(fmt::dim()));
        ui.end_row();

        for (i, a) in d.alerts.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label("Alert when");
                ui.selectable_value(&mut a.when, AlertWhen::Below, "below");
                ui.selectable_value(&mut a.when, AlertWhen::Above, "above");
            });
            ui.add(
                egui::TextEdit::singleline(&mut a.level).hint_text("500.00").desired_width(110.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut a.message)
                    .hint_text("message, e.g. Top up from savings")
                    .desired_width(260.0),
            );
            if ui.small_button("\u{1F5D9}").on_hover_text("Remove this alert").clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = remove {
        d.alerts.remove(i);
    }
    ui.horizontal(|ui| {
        if ui.button("Add an alert").clicked() {
            s.goals_draft.alerts.push(AlertRow {
                when: AlertWhen::Below,
                level: String::new(),
                message: String::new(),
            });
        }
        if ui.button("Stage target and alerts").clicked() {
            match s.goals_draft.finish(&s.budget().variables) {
                Ok((target, alerts)) => {
                    s.stage(
                        vec![Op::SetLedgerGoals { uid, target, alerts }],
                        "a target and alerts",
                    );
                }
                Err(e) => s.fail(e),
            }
        }
    });
}

fn message_or_level(a: &Alert) -> String {
    if a.message.is_empty() {
        "Alert".to_string()
    } else {
        a.message.clone()
    }
}

/// The alert list: every alert past its level, with its message.
pub fn fired_list(ui: &mut Ui, s: &mut Session, fired: &[FiredAlert]) {
    let mut open = None;
    egui::Grid::new("fired_alerts").num_columns(3).spacing([14.0, 4.0]).show(ui, |ui| {
        for f in fired {
            let l = s.budget();
            ui.colored_label(fmt::bad(), format!("\u{26A0} {}", message_or_level(&f.alert)));
            if ui.link(&l.ledgers.name[f.ledger.get()]).clicked() {
                open = Some(l.ledgers.uid[f.ledger.get()]);
            }
            num(
                ui,
                RichText::new(format!(
                    "{}  ({} {})",
                    fmt::amount(f.balance),
                    f.alert.when,
                    fmt::amount(f.alert.level)
                ))
                .monospace()
                .color(fmt::dim()),
            );
            ui.end_row();
        }
    });
    if let Some(uid) = open {
        s.selected_ledger = Some(uid);
        s.goto = Some(Screen::Register);
    }
}
