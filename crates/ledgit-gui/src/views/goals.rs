//! Targets and alerts, wherever they show: a ledger's own page, the
//! dashboard's alert and pace lists, and the commit report.

use super::num;
use crate::app::{Screen, Session};
use crate::fmt;
use egui::{RichText, Ui};
use ledgit_core::expr::eval_money;
use ledgit_core::goals::{self, FiredAlert, PaceBreach, PaceState, PaceStatus};
use ledgit_core::prelude::*;

/// A ledger's target and alerts as they are being edited.
#[derive(Clone, Default)]
pub struct GoalsDraft {
    pub ledger: Option<LedgerUid>,
    pub target: String,
    /// `Some` makes the target a pace: this much per period.
    pub per: Option<Period>,
    pub bound: Bound,
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
        let (target, per, bound) = match l.ledgers.target[ix.get()] {
            None => (String::new(), None, Bound::default()),
            Some(Target::Balance(t)) => (t.to_string(), None, Bound::default()),
            Some(Target::Pace { amount, per, bound }) => (amount.to_string(), Some(per), bound),
        };
        GoalsDraft {
            ledger: Some(l.ledgers.uid[ix.get()]),
            target,
            per,
            bound,
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
    ) -> std::result::Result<(Option<Target>, Vec<Alert>), String> {
        let target = match self.target.trim() {
            "" => None,
            t => {
                let amount = eval_money(t, vars).map_err(|e| format!("Target: {e}"))?;
                let target = match self.per {
                    None => Target::Balance(amount),
                    Some(per) => Target::Pace { amount, per, bound: self.bound },
                };
                target.validate().map_err(|e| format!("Target: {e}"))?;
                Some(target)
            }
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
        Some(t) => format!("Target {}  \u{b7}  {} alert(s)", describe(t), alerts.len()),
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

            if let Some(Target::Pace { .. }) = target {
                pace(ui, s, ix);
            }
            if let Some(t) = target.and_then(Target::balance) {
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

/// "250.00 a week at most", "0.00".
pub fn describe(t: Target) -> String {
    match t {
        Target::Balance(m) => fmt::amount(m),
        Target::Pace { amount, per, bound } => format!("{} a {per} {bound}", fmt::amount(amount)),
    }
}

/// "This week", "This month", ...
fn this(per: Period) -> &'static str {
    match per {
        Period::Day => "Today",
        Period::Week => "This week",
        Period::Month => "This month",
        Period::Year => "This year",
    }
}

/// How a pace stands, in words and a colour. `None` for no colour.
fn standing(p: &PaceStatus) -> (String, Option<egui::Color32>) {
    let by_now = fmt::amount(p.due_by_now);
    match (p.state(), p.bound) {
        (PaceState::Over, _) => (format!("over by {}", fmt::amount(p.over())), Some(fmt::bad())),
        (PaceState::Met, _) => ("met".into(), Some(fmt::good())),
        (PaceState::OffPace, Bound::AtMost) => {
            (format!("ahead of pace: {by_now} would be on pace by today"), Some(fmt::warn()))
        }
        (PaceState::OffPace, Bound::AtLeast) => {
            (format!("behind pace: {by_now} due by today"), Some(fmt::warn()))
        }
        (PaceState::OnPace, Bound::AtMost) => (format!("{} left", fmt::amount(p.left())), None),
        (PaceState::OnPace, Bound::AtLeast) => (format!("{} to go", fmt::amount(p.left())), None),
    }
}

/// How full a pace's period is, for a progress bar.
fn fill(flow: Money, amount: Money) -> f32 {
    if amount.0 <= 0 {
        return if flow.0 > 0 { 1.0 } else { 0.0 };
    }
    (flow.0 as f32 / amount.0 as f32).clamp(0.0, 1.0)
}

/// A period's bar: settled ones green or red, one still running neutral
/// until it is settled - a ceiling passed, or a floor met.
fn period_colour(bound: Bound, amount: Money, flow: Money, running: bool) -> egui::Color32 {
    match (bound.keeps(amount, flow), bound) {
        (true, Bound::AtMost) if running => fmt::dim(),
        (false, Bound::AtLeast) if running => fmt::dim(),
        (true, _) => fmt::good(),
        (false, _) => fmt::bad(),
    }
}

/// The current period against the pace, and the periods before it.
fn pace(ui: &mut Ui, s: &mut Session, ix: ledgit_core::id::LedgerIx) {
    let l = s.budget();
    let today = Date::today_utc();
    let Some(p) = goals::pace_status(l, ix, today) else { return };
    let (words, colour) = standing(&p);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{}: {} of {}",
                this(p.per),
                fmt::amount(p.period.flow),
                fmt::amount(p.amount)
            ))
            .strong(),
        );
        ui.separator();
        match colour {
            Some(c) => ui.colored_label(c, words),
            None => ui.label(words),
        };
    });
    let bar = egui::ProgressBar::new(fill(p.period.flow, p.amount))
        .desired_width(360.0)
        .fill(colour.unwrap_or(ui.visuals().selection.bg_fill));
    ui.add(bar).on_hover_text(format!(
        "{} to {}. The pace's share of the days gone is {}.",
        p.period.start,
        p.period.end.add_days(-1),
        fmt::amount(p.due_by_now)
    ));

    let n = match p.per {
        Period::Day => 14,
        Period::Week | Period::Month => 12,
        Period::Year => 5,
    };
    let mut history = goals::pace_history(l, ix, p.per, today, n);
    // Only whole periods the ledger was open for count towards the record.
    let opened = l.ledgers.opened[ix.get()];
    history.retain(|h| h.end > opened);
    let past = &history[..history.len().saturating_sub(1)];
    if !past.is_empty() {
        let kept = past.iter().filter(|h| p.bound.keeps(p.amount, h.flow)).count();
        let average: Money = past.iter().map(|h| h.flow).sum::<Money>();
        let average = Money(average.0 / past.len() as i64);
        ui.label(
            RichText::new(format!(
                "Kept to it in {kept} of the last {} {}s; {} a {} on average.",
                past.len(),
                p.per,
                fmt::amount(average),
                p.per
            ))
            .color(fmt::dim()),
        );
    }
    egui::CollapsingHeader::new(format!("{} by {}", p.per.adjective(), p.per))
        .id_salt(("pace_history", ix.get()))
        .show(ui, |ui| {
            crate::table::Table::new(
                ("pace_history", ix.get()),
                vec![
                    crate::table::text(p.per.noun()),
                    crate::table::figures("moved"),
                    crate::table::figures("vs pace"),
                    crate::table::text("").max(220.0),
                ],
            )
            .height(crate::table::Height::Max(260.0))
            .show(ui, history.len(), |row| {
                // Newest first.
                let h = history[history.len() - 1 - row.index()];
                let current = h.end > today;
                row.col(|ui| {
                    ui.label(fmt::mono(super::saved::period_label(p.per, h.start)));
                });
                row.col(|ui| {
                    num(ui, fmt::mono(fmt::amount(h.flow)));
                });
                row.col(|ui| {
                    num(ui, fmt::mono(fmt::signed(h.flow - p.amount)));
                });
                row.col(|ui| {
                    ui.add(
                        egui::ProgressBar::new(fill(h.flow, p.amount))
                            .desired_width(120.0)
                            .fill(period_colour(p.bound, p.amount, h.flow, current)),
                    );
                    if current {
                        ui.label(RichText::new("so far").small().color(fmt::dim()));
                    }
                });
            });
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
            .on_hover_text(match d.per {
                None => "The balance you want, as displayed. A formula works here too.",
                Some(_) => "How far the balance may move each period. A formula works here too.",
            });
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(("goals_per", uid))
                .selected_text(match d.per {
                    None => "balance".to_string(),
                    Some(p) => format!("per {p}"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.per, None, "balance")
                        .on_hover_text("A balance to get to");
                    for p in Period::ALL {
                        ui.selectable_value(&mut d.per, Some(p), format!("per {p}"));
                    }
                });
            if d.per.is_some() {
                ui.selectable_value(&mut d.bound, Bound::AtMost, "at most")
                    .on_hover_text("A budget: spend no more than this");
                ui.selectable_value(&mut d.bound, Bound::AtLeast, "at least")
                    .on_hover_text("A habit: put in at least this");
            }
        });
        ui.label(
            RichText::new(match d.per {
                None => "a loan's target is usually 0",
                Some(Period::Week) => "weeks start on Monday",
                Some(_) => "calendar periods, not rolling ones",
            })
            .small()
            .color(fmt::dim()),
        );
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
            crate::textbox::LongText::new(("alert_message", uid, i), &mut a.message)
                .hint("message, e.g. Top up from savings before the mortgage comes out")
                .title("Alert message")
                .show(ui);
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

/// Every pace and how its period is going, most urgent first.
pub fn pace_list(ui: &mut Ui, s: &mut Session, paces: &[PaceStatus]) {
    let mut order: Vec<&PaceStatus> = paces.iter().collect();
    order.sort_by_key(|p| match p.state() {
        PaceState::Over => 0,
        PaceState::OffPace => 1,
        PaceState::OnPace => 2,
        PaceState::Met => 3,
    });
    let mut open = None;
    let l = s.budget();
    crate::table::Table::new(
        "paces",
        vec![
            crate::table::text("ledger").max(300.0),
            crate::table::figures("so far"),
            crate::table::text("pace").max(220.0),
            crate::table::text("").max(320.0),
        ],
    )
    .height(crate::table::Height::Max(220.0))
    .show(ui, order.len(), |row| {
        let p = order[row.index()];
        let (words, colour) = standing(p);
        row.col(|ui| {
            if ui.link(&l.ledgers.name[p.ledger.get()]).clicked() {
                open = Some(l.ledgers.uid[p.ledger.get()]);
            }
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(p.period.flow)));
        });
        row.col(|ui| {
            ui.label(
                RichText::new(describe(Target::Pace {
                    amount: p.amount,
                    per: p.per,
                    bound: p.bound,
                }))
                .color(fmt::dim()),
            );
        });
        row.col(|ui| {
            ui.add(
                egui::ProgressBar::new(fill(p.period.flow, p.amount))
                    .desired_width(90.0)
                    .fill(colour.unwrap_or(ui.visuals().selection.bg_fill)),
            );
            match colour {
                Some(c) => ui.colored_label(c, words),
                None => ui.label(words),
            };
        });
    });
    if let Some(uid) = open {
        s.selected_ledger = Some(uid);
        s.goto = Some(Screen::Register);
    }
}

/// Paces a commit would break: the period, and what it moves by before and
/// after.
pub fn breach_list(ui: &mut Ui, s: &mut Session, breaches: &[PaceBreach]) {
    let mut open = None;
    let l = s.budget();
    crate::table::Table::new(
        "pace_breaches",
        vec![
            crate::table::text("ledger").max(300.0),
            crate::table::text("pace").max(220.0),
            crate::table::text("period"),
            crate::table::figures("before"),
            crate::table::figures("after"),
        ],
    )
    .height(crate::table::Height::Max(220.0))
    .show(ui, breaches.len(), |row| {
        let b = &breaches[row.index()];
        row.col(|ui| {
            if ui.link(&l.ledgers.name[b.ledger.get()]).clicked() {
                open = Some(l.ledgers.uid[b.ledger.get()]);
            }
        });
        row.col(|ui| {
            ui.colored_label(
                fmt::bad(),
                format!(
                    "\u{26A0} {}",
                    describe(Target::Pace { amount: b.amount, per: b.per, bound: b.bound })
                ),
            );
        });
        row.col(|ui| {
            ui.label(fmt::mono(super::saved::period_label(b.per, b.start)));
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(b.before)));
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(b.after)).color(fmt::bad()));
        });
    });
    if let Some(uid) = open {
        s.selected_ledger = Some(uid);
        s.goto = Some(Screen::Register);
    }
}

/// The alert list: every alert past its level, with its message.
pub fn fired_list(ui: &mut Ui, s: &mut Session, fired: &[FiredAlert]) {
    let mut open = None;
    let l = s.budget();
    crate::table::Table::new(
        "fired_alerts",
        vec![
            crate::table::text("alert").max(320.0),
            crate::table::text("ledger").max(300.0),
            crate::table::figures("balance"),
        ],
    )
    .height(crate::table::Height::Max(220.0))
    .show(ui, fired.len(), |row| {
        let f = &fired[row.index()];
        row.col(|ui| {
            ui.colored_label(fmt::bad(), format!("\u{26A0} {}", message_or_level(&f.alert)));
        });
        row.col(|ui| {
            if ui.link(&l.ledgers.name[f.ledger.get()]).clicked() {
                open = Some(l.ledgers.uid[f.ledger.get()]);
            }
        });
        row.col(|ui| {
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
        });
    });
    if let Some(uid) = open {
        s.selected_ledger = Some(uid);
        s.goto = Some(Screen::Register);
    }
}
