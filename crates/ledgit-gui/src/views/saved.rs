//! Saved views: balances across time, flows per period, and a simulation of
//! the issuers forward - with the chart redrawn on every pick.
//!
//! The screen edits a *draft* of the selected view's spec, held on the
//! session. Evaluating a view is a read that takes well under a millisecond,
//! so the chart follows the draft live; staging an `EditView` happens only
//! when you ask, so trying ten horizons leaves one op in the log, not ten.

use super::{empty, heading, num};
use crate::app::Session;
use crate::fmt;
use crate::forms::FormKind;
use crate::picker::{Pick, Picker};
use crate::table::{figures, text, Height, Table};
use egui::{Color32, ComboBox, RichText, Ui};
use egui_plot::{GridInput, GridMark, HLine, Line, LineStyle, Plot, VLine};
use ledgit_core::id::IssuerIx;
use ledgit_core::prelude::*;
use ledgit_core::view;
use ledgit_plot::{date_ticks, money_short, MAX_SERIES, SERIES_DARK, SERIES_LIGHT};

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(
        ui,
        "Views",
        "Charts of your buckets and ledgers across time, with your issuers simulated forward. A view moves no money.",
    );
    if ui.button("New view").clicked() {
        s.forms.open(FormKind::View, s.repo.working());
    }
    ui.add_space(8.0);

    let live: Vec<ViewUid> =
        s.budget().views.live().map(|ix| s.budget().views.uid[ix.get()]).collect();
    if live.is_empty() {
        empty(ui, "No saved views yet. \"New view\" starts one with every bucket in it.");
        return;
    }
    if s.selected_view.is_none_or(|v| !live.contains(&v)) {
        s.selected_view = live.first().copied();
    }
    let selected = s.selected_view.expect("set above");

    // Start a fresh draft whenever the selection moves, or the view it was
    // drafted from is no longer here (a checkout, a revert).
    if s.view_draft.as_ref().is_none_or(|(uid, _)| *uid != selected) {
        let l = s.budget();
        let spec = l.views.spec[l.views.ix(selected).expect("live").get()].clone();
        s.view_draft = Some((selected, spec));
    }

    super::split(
        ui,
        "views",
        200.0,
        s,
        |ui, s| {
            for uid in &live {
                let Some(ix) = s.budget().views.ix(*uid) else { continue };
                let name = s.budget().views.name[ix.get()].clone();
                let about = s.budget().views.description[ix.get()].clone();
                let item = ui.selectable_label(s.selected_view == Some(*uid), name);
                let item = if about.is_empty() { item } else { item.on_hover_text(about) };
                if item.clicked() {
                    s.selected_view = Some(*uid);
                }
            }
        },
        |ui, s| {
            egui::ScrollArea::vertical().id_salt("view_detail").show(ui, |ui| detail(ui, s));
        },
    );
}

enum Action {
    Stage(Box<Op>, &'static str),
    Discard,
    Note(String),
    Fail(String),
}

fn detail(ui: &mut Ui, s: &mut Session) {
    let Some((uid, mut spec)) = s.view_draft.clone() else { return };
    let l = s.repo.working();
    let Some(ix) = l.views.ix(uid) else { return };
    let name = l.views.name[ix.get()].clone();
    let stored = &l.views.spec[ix.get()];
    let dirty = *stored != spec;
    let mut actions: Vec<Action> = Vec::new();

    ui.horizontal(|ui| {
        ui.heading(&name);
        ui.add_space(8.0);
        if ui
            .add_enabled(dirty, egui::Button::new("Stage changes"))
            .on_hover_text("Save what this view looks at. Like everything, it is committed on the Commit screen.")
            .clicked()
        {
            let op = Op::EditView { uid, name: None, description: None, spec: Some(spec.clone()) };
            actions.push(Action::Stage(Box::new(op), "a change to a view"));
        }
        if ui.add_enabled(dirty, egui::Button::new("Discard edits")).clicked() {
            actions.push(Action::Discard);
        }
        if ui.small_button("Delete view").on_hover_text("A view is a reading; no money moves.").clicked() {
            actions.push(Action::Stage(Box::new(Op::DeleteView { uid }), "deleting a view"));
        }
        if dirty {
            ui.label(RichText::new("edited, not staged").color(fmt::dim()));
        }
    });
    description(ui, &mut s.view_description, uid, &l.views.description[ix.get()], &mut actions);
    ui.add_space(4.0);

    egui::CollapsingHeader::new("What this view looks at")
        .default_open(true)
        .show(ui, |ui| editor(ui, l, &mut spec));
    ui.add_space(6.0);

    // Window, and exports.
    let today = Date::today_utc();
    ui.horizontal(|ui| {
        ui.label("Simulate until");
        let horizon = spec.horizon.after(today).to_string();
        crate::datepick::DateField::new("view_until", &mut s.view_until)
            .optional(&horizon)
            .show(ui);
    });
    let until = s.view_until.trim();
    let report = match until.parse::<Date>() {
        _ if until.is_empty() => view::evaluate(l, &spec, today),
        Ok(end) if end >= today => {
            view::evaluate_between(l, &spec, today, spec.lookback.before(today), end)
        }
        Ok(_) => {
            ui.colored_label(fmt::bad(), "Simulate until a date after today.");
            view::evaluate(l, &spec, today)
        }
        Err(_) => {
            ui.colored_label(fmt::bad(), "That date must be YYYY-MM-DD.");
            view::evaluate(l, &spec, today)
        }
    };

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{} to {} \u{b7} {} issuer(s) simulated",
                report.start, report.end, report.simulated_issuers
            ))
            .color(fmt::dim()),
        );
        ui.separator();
        if ui.button("Save chart...").on_hover_text("PNG or SVG, for printing or sharing").clicked()
        {
            actions.push(export_chart(&name, &report));
        }
        if ui
            .button("Save CSV...")
            .on_hover_text("Every balance series, one row per change")
            .clicked()
        {
            actions.push(export_csv(&name, &report));
        }
    });
    ui.add_space(6.0);

    // Compare with another point in history: an earlier commit, or the
    // budget as committed, without what is staged.
    compare_picker(ui, s);
    let then_budget: Option<&Budget> = match s.view_compare {
        None => None,
        Some(CompareWith::Committed) => Some(s.repo.committed()),
        Some(CompareWith::Commit(id)) => {
            // Folding history is the expensive part; do it once per pick.
            if s.compare_cache.as_ref().is_none_or(|(c, _)| *c != id) {
                s.compare_cache = s.repo.budget_at(id).ok().map(|b| (id, b));
            }
            s.compare_cache.as_ref().map(|(_, b)| b)
        }
    };
    let l = s.repo.working();
    let compared = then_budget.map(|then| view::compare(then, &spec, l, &report));

    chart(ui, &report, compared.as_ref().map(|(r, p)| (r, p.as_slice())));
    ui.add_space(10.0);
    if let Some((then, pairs)) = &compared {
        comparison(ui, &report, then, pairs, &compare_label(s));
        ui.add_space(10.0);
    }
    balances(ui, &report);
    ui.add_space(10.0);
    flows(ui, &report);
    ui.add_space(10.0);
    egui::CollapsingHeader::new(format!("Timeline, per {}", report.period))
        .default_open(false)
        .show(ui, |ui| timeline(ui, &report));
    notes(ui, l, &report);

    // When the issuers in the flow table fall due. A view with no flows has
    // nothing to put on a calendar.
    let issuers: Vec<IssuerIx> = report.flows.iter().map(|f| f.issuer).collect();
    if !issuers.is_empty() {
        ui.add_space(10.0);
        egui::CollapsingHeader::new("Calendar")
            .id_salt(("view_calendar", uid))
            .default_open(false)
            .show(ui, |ui| super::calendar::calendar(ui, s, "view", &issuers));
    }

    s.view_draft = Some((uid, spec));
    for a in actions {
        match a {
            Action::Stage(op, what) => {
                s.stage(vec![*op], what);
            }
            Action::Discard => s.view_draft = None,
            Action::Note(m) => s.note(m),
            Action::Fail(m) => s.fail(m),
        }
    }
}

/// What the view is for, in your words - shown under its name, written or
/// rewritten in place, and staged like any other edit.
fn description(
    ui: &mut Ui,
    draft: &mut Option<(ViewUid, String)>,
    uid: ViewUid,
    stored: &str,
    actions: &mut Vec<Action>,
) {
    let editing = draft.as_ref().is_some_and(|(v, _)| *v == uid);
    if !editing {
        ui.horizontal_wrapped(|ui| {
            if !stored.is_empty() {
                ui.label(RichText::new(stored).color(fmt::dim()));
            }
            let label = if stored.is_empty() { "add a description" } else { "edit description" };
            if ui
                .small_button(label)
                .on_hover_text("Note what this view looks at and why")
                .clicked()
            {
                *draft = Some((uid, stored.to_string()));
                ui.data_mut(|d| {
                    d.insert_temp(egui::Id::new(("view_description_focus", uid)), true)
                });
            }
        });
        return;
    }
    let Some((_, text)) = draft.as_mut() else { return };
    let edit = crate::textbox::LongText::new(("view_description", uid), text)
        .hint("What this view looks at, e.g. net worth with the car loan paid off early")
        .title("View description")
        .width(640.0)
        .rows(3)
        .show(ui);
    // Ready to type the moment it opens - once, so clicking away still works.
    let focus_id = egui::Id::new(("view_description_focus", uid));
    if ui.data_mut(|d| d.remove_temp::<bool>(focus_id)).unwrap_or(false) {
        edit.request_focus();
    }
    let text = text.trim().to_string();
    ui.horizontal(|ui| {
        let changed = text != stored;
        if ui
            .add_enabled(changed, egui::Button::new("Stage description"))
            .on_hover_text("Committed with everything else on the Commit screen.")
            .clicked()
        {
            let op = Op::EditView { uid, name: None, description: Some(text), spec: None };
            actions.push(Action::Stage(Box::new(op), "a view's description"));
            *draft = None;
        }
        if ui.button("Cancel").clicked() {
            *draft = None;
        }
    });
}

// ------------------------------------------------------------------ editor

/// Chips that wrap: as many lines as they need, up to this, then they scroll.
const CHIP_LINES: f32 = 4.0;

/// One labelled line of the editor: the label on the left, the control
/// beside it, taking only the height it needs.
fn field(ui: &mut Ui, label: &str, hover: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        let r = ui
            .allocate_ui_with_layout(
                egui::vec2(130.0, ui.spacing().interact_size.y),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(130.0);
                    ui.label(label)
                },
            )
            .inner;
        if !hover.is_empty() {
            r.on_hover_text(hover);
        }
        ui.vertical(add);
    });
    ui.add_space(4.0);
}

/// A wrapping run of chips that grows to fit them, then scrolls - in whole
/// lines, so the last one on show is never sliced through the middle.
fn chips(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    let line = ui.spacing().interact_size.y;
    let gap = ui.spacing().item_spacing.y;
    egui::ScrollArea::vertical()
        .id_salt(("view_chips", id))
        .max_height(CHIP_LINES * (line + gap) - gap)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal_wrapped(add);
        });
}

fn editor(ui: &mut Ui, l: &Budget, spec: &mut ViewSpec) {
    field(ui, "Buckets", "Click to cycle: not included, added (+), subtracted (-).", |ui| {
        let live: Vec<_> = l.buckets.live().collect();
        if live.is_empty() {
            ui.label(RichText::new("none yet").color(fmt::dim()));
            return;
        }
        chips(ui, "buckets", |ui| {
            for b in live {
                let uid = l.buckets.uid[b.get()];
                bucket_chip(ui, spec, uid, &l.buckets.name[b.get()]);
            }
        });
    });

    field(
        ui,
        "Ledgers",
        "Each gets its own line. With no buckets, the ledgers together are the view's total.",
        |ui| ledger_picks(ui, l, spec),
    );

    field(ui, "Issuers", "Their flow is broken down per period.", |ui| {
        if l.issuers.is_empty() {
            ui.label(RichText::new("none yet").color(fmt::dim()));
            return;
        }
        chips(ui, "issuers", |ui| {
            for i in l.issuers.indices() {
                toggle_chip(
                    ui,
                    &mut spec.issuers,
                    l.issuers.uid[i.get()],
                    &l.issuers.name[i.get()],
                );
            }
        });
    });

    field(
        ui,
        "Past transactions",
        "Which posted transactions the timeline counts. Blank means everything that moves this view's money.",
        |ui| {
            let mut text = spec
                .transactions
                .iter()
                .find_map(|f| match f {
                    TxFilter::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut text)
                        .hint_text("name contains...")
                        .desired_width(220.0),
                )
                .changed()
            {
                spec.transactions.retain(|f| !matches!(f, TxFilter::Text(_)));
                if !text.trim().is_empty() {
                    spec.transactions.push(TxFilter::Text(text));
                }
            }
        },
    );

    field(ui, "Balances", "", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(
                &mut spec.roll,
                RollUp::ByNormality,
                "net (assets \u{2212} liabilities)",
            );
            ui.radio_value(&mut spec.roll, RollUp::Sum, "as shown");
        });
    });

    field(ui, "Flows per", "", |ui| {
        ComboBox::from_id_salt("view_period").selected_text(spec.period.noun()).show_ui(ui, |ui| {
            for p in Period::ALL {
                ui.selectable_value(&mut spec.period, p, p.noun());
            }
        });
    });

    field(ui, "Look back", "", |ui| span_editor(ui, "lookback", &mut spec.lookback));
    field(ui, "Simulate ahead", "", |ui| span_editor(ui, "horizon", &mut spec.horizon));

    field(ui, "Simulate", "", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(&mut spec.only_selected_issuers, false, "every running issuer")
                .on_hover_text("What will actually happen.");
            ui.radio_value(&mut spec.only_selected_issuers, true, "only this view's issuers")
                .on_hover_text("What if these were all that happened?");
        });
    });
}

/// The view's own ledgers: the ones picked, as chips to click off, and the
/// ledger tree picker to add more - one ledger, or everything under a path.
/// Every ledger in the budget as a chip was a wall, cut off wherever the
/// box happened to end.
fn ledger_picks(ui: &mut Ui, l: &Budget, spec: &mut ViewSpec) {
    // In tree order, so a subtree's ledgers sit together.
    let picked: Vec<(LedgerUid, &str)> = LedgerTree::build(l)
        .order
        .into_iter()
        .map(|a| (l.ledgers.uid[a.get()], l.ledgers.name[a.get()].as_str()))
        .filter(|(uid, _)| spec.ledgers.contains(uid))
        .collect();

    ui.horizontal(|ui| {
        let add = Picker::new("view_ledger_add", l)
            .selected_text("add a ledger...")
            .width(240.0)
            .subtrees(true)
            .hide(spec.ledgers.clone())
            .show(ui);
        match add {
            Some(Pick::Ledger(uid)) => spec.ledgers.push(uid),
            Some(Pick::Subtree(path)) => {
                for a in LedgerTree::build(l).order {
                    let uid = l.ledgers.uid[a.get()];
                    let name = &l.ledgers.name[a.get()];
                    let under = name.eq_ignore_ascii_case(&path)
                        || ledgit_core::tree::is_under(name, &path);
                    if under && !spec.ledgers.contains(&uid) {
                        spec.ledgers.push(uid);
                    }
                }
            }
            None => {}
        }
        if picked.is_empty() {
            ui.label(RichText::new("none picked").small().color(fmt::dim()));
        } else {
            ui.label(RichText::new(format!("{} picked", picked.len())).small().color(fmt::dim()));
            if ui.small_button("clear").on_hover_text("Take every ledger off this view").clicked() {
                spec.ledgers.clear();
            }
        }
    });
    if picked.is_empty() {
        return;
    }
    let mut drop = None;
    chips(ui, "ledgers", |ui| {
        for (uid, name) in &picked {
            let chip = RichText::new(format!("{}  \u{00D7}", fmt::clip(name, 40)));
            if ui
                .selectable_label(true, chip)
                .on_hover_text(format!("{name}\nClick to take it off this view."))
                .clicked()
            {
                drop = Some(*uid);
            }
        }
    });
    if let Some(uid) = drop {
        spec.ledgers.retain(|x| *x != uid);
    }
}

fn span_editor(ui: &mut Ui, id: &str, span: &mut Span) {
    ui.horizontal(|ui| {
        let mut n = span.count();
        let mut unit = span.unit();
        ui.add(egui::DragValue::new(&mut n).range(0..=600));
        ComboBox::from_id_salt(("span", id)).selected_text(format!("{}s", unit.noun())).show_ui(
            ui,
            |ui| {
                for p in Period::ALL {
                    ui.selectable_value(&mut unit, p, format!("{}s", p.noun()));
                }
            },
        );
        *span = Span::new(n, unit);
        if let Err(e) = span.validate() {
            ui.colored_label(fmt::bad(), e);
        }
    });
}

/// A bucket's place in the view: out, added, or subtracted.
fn bucket_chip(ui: &mut Ui, spec: &mut ViewSpec, uid: BucketUid, name: &str) {
    let at = spec.buckets.iter().position(|t| t.bucket == uid);
    let (mark, colour) = match at.map(|i| spec.buckets[i].sign) {
        Some(Sign::Plus) => ("+", fmt::good()),
        Some(Sign::Minus) => ("\u{2212}", fmt::bad()),
        None => ("\u{b7}", fmt::dim()),
    };
    let label = RichText::new(format!("{mark} {name}")).color(colour);
    if ui.selectable_label(at.is_some(), label).clicked() {
        match at {
            None => spec.buckets.push(Term::plus(uid)),
            Some(i) if spec.buckets[i].sign == Sign::Plus => spec.buckets[i].sign = Sign::Minus,
            Some(i) => {
                spec.buckets.remove(i);
            }
        }
    }
}

fn toggle_chip<T: PartialEq + Copy>(ui: &mut Ui, list: &mut Vec<T>, item: T, name: &str) {
    let on = list.contains(&item);
    if ui.selectable_label(on, name).clicked() {
        if on {
            list.retain(|x| *x != item);
        } else {
            list.push(item);
        }
    }
}

// ------------------------------------------------------------------- chart

pub fn series_colour(i: usize, dark: bool) -> Color32 {
    let [r, g, b] = if dark { SERIES_DARK[i] } else { SERIES_LIGHT[i] };
    Color32::from_rgb(r, g, b)
}

/// Step vertices for one series, split at today: solid behind, dashed ahead.
fn steps(points: &[(Date, Money)], today: Date) -> (Vec<[f64; 2]>, Vec<[f64; 2]>) {
    let mut past: Vec<[f64; 2]> = Vec::new();
    let mut ahead: Vec<[f64; 2]> = Vec::new();
    let mut prev: Option<[f64; 2]> = None;
    for (d, v) in points {
        let (x, y) = (d.0 as f64, v.cents() as f64 / 100.0);
        let out = if *d <= today { &mut past } else { &mut ahead };
        if let Some(p) = prev {
            if out.is_empty() {
                out.push(p);
            }
            out.push([x, p[1]]);
        }
        out.push([x, y]);
        prev = Some([x, y]);
    }
    (past, ahead)
}

/// What the Views screen compares the selected view against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompareWith {
    /// The budget as committed: everything but what is staged.
    Committed,
    Commit(CommitId),
}

fn commit_date(c: &Commit) -> Date {
    Date(c.timestamp.div_euclid(86_400) as i32)
}

fn compare_label(s: &Session) -> String {
    match s.view_compare {
        None => "nothing".into(),
        Some(CompareWith::Committed) => "the last commit, without staged changes".into(),
        Some(CompareWith::Commit(id)) => match s.repo.get_commit(id) {
            Ok(c) => format!("{} ({}, {})", c.summary(), id.short(), commit_date(&c)),
            Err(_) => id.short(),
        },
    }
}

fn compare_picker(ui: &mut Ui, s: &mut Session) {
    let label = compare_label(s);
    ui.horizontal(|ui| {
        ui.label("Compare with").on_hover_text(
            "Lay this view over itself as it stood at another commit - say, before a \
             lump-sum payment - with the same window and the same today.",
        );
        ComboBox::from_id_salt("view_compare").selected_text(label).width(360.0).show_ui(
            ui,
            |ui| {
                // Read only while the list is open: every commit read is
                // re-hashed, and this would otherwise run on every frame.
                let log = s.repo.log(Some(60)).unwrap_or_default();
                ui.selectable_value(&mut s.view_compare, None, "nothing");
                if s.repo.has_staged_changes() {
                    ui.selectable_value(
                        &mut s.view_compare,
                        Some(CompareWith::Committed),
                        "the last commit, without staged changes",
                    );
                }
                for c in &log {
                    ui.selectable_value(
                        &mut s.view_compare,
                        Some(CompareWith::Commit(c.id)),
                        format!(
                            "{}  {}  {}",
                            c.id.short(),
                            commit_date(c),
                            fmt::clip(c.summary(), 48)
                        ),
                    );
                }
            },
        );
    });
}

/// Each line then and now: where it stands, where it ends, and when it
/// reaches its target - the question a lump-sum payment is meant to answer.
fn comparison(
    ui: &mut Ui,
    now: &ViewReport,
    then: &ViewReport,
    pairs: &[Option<usize>],
    label: &str,
) {
    ui.label(RichText::new(format!("Compared with {label}")).strong());
    Table::new(
        "view_compare_table",
        vec![
            text("").max(300.0),
            figures("today"),
            figures("at end"),
            figures("change at end"),
            figures("target").max(320.0),
        ],
    )
    .fit_to(label)
    .show(ui, now.series.len(), |row| {
        let i = row.index();
        let s = &now.series[i];
        row.col(|ui| {
            ui.label(&s.label);
        });
        let Some(t) = pairs[i].map(|j| &then.series[j]) else {
            row.col(|ui| {
                ui.label(RichText::new("not in that commit").small().color(fmt::dim()));
            });
            for _ in 0..3 {
                row.col(|_| {});
            }
            return;
        };
        row.col(|ui| {
            num(
                ui,
                fmt::mono(format!("{} \u{27A1} {}", fmt::amount(t.now), fmt::amount(s.now)))
                    .small(),
            );
        });
        row.col(|ui| {
            num(
                ui,
                fmt::mono(format!("{} \u{27A1} {}", fmt::amount(t.at_end), fmt::amount(s.at_end)))
                    .small(),
            );
        });
        row.col(|ui| {
            num(ui, fmt::delta_text(s.at_end - t.at_end));
        });
        let target = match (t.target_reached, s.target_reached) {
            _ if s.target.is_none() && t.target.is_none() => RichText::new(""),
            (Some(a), Some(b)) if a == b => RichText::new(format!("{b}, unchanged")).small(),
            (Some(a), Some(b)) => {
                let days = a.0 - b.0;
                let (text, colour) = if days > 0 {
                    (format!("{b}, {} sooner", span_words(days)), fmt::good())
                } else {
                    (format!("{b}, {} later", span_words(-days)), fmt::bad())
                };
                RichText::new(text).small().color(colour).strong()
            }
            (None, Some(b)) => {
                RichText::new(format!("{b}, was not reached")).small().color(fmt::good())
            }
            (Some(a), None) => {
                RichText::new(format!("not reached, was {a}")).small().color(fmt::bad())
            }
            (None, None) => RichText::new("not reached either way").small().color(fmt::dim()),
        };
        row.col(|ui| {
            num(ui, target);
        });
    });
}

/// "3 months", "12 days", "1 year 2 months".
fn span_words(days: i32) -> String {
    if days < 45 {
        return format!("{days} day(s)");
    }
    let months = (days as f64 / 30.44).round() as i32;
    match (months / 12, months % 12) {
        (0, m) => format!("{m} month(s)"),
        (y, 0) => format!("{y} year(s)"),
        (y, m) => format!("{y} year(s) {m} month(s)"),
    }
}

/// How tall the chart is, and so how far its legend runs before scrolling.
const CHART_HEIGHT: f32 = 320.0;

fn chart(ui: &mut Ui, r: &ViewReport, then: Option<(&ViewReport, &[Option<usize>])>) {
    if r.series.is_empty() {
        ui.label(
            RichText::new(
                "Add a bucket or a ledger to chart balances; issuers alone give flows only.",
            )
            .color(fmt::dim()),
        );
        return;
    }
    let dark = ui.visuals().dark_mode;
    let today = r.today;
    // Lines switched off in the legend, by label. Kept per chart while the
    // app runs; a preference about reading, not a fact about money.
    let hidden_id = ui.id().with("view_chart_hidden");
    let mut hidden: std::collections::BTreeSet<String> =
        ui.data(|d| d.get_temp(hidden_id)).unwrap_or_default();

    ui.horizontal_top(|ui| {
        // The legend sits beside the chart, never on it: with a dozen lines
        // an overlaid legend covers the data it is naming.
        let legend_width = 210.0;
        let plot_width = (ui.available_width() - legend_width - 12.0).max(240.0);
        Plot::new("view_chart")
            .height(CHART_HEIGHT)
            .width(plot_width)
            .allow_scroll(false)
            .x_grid_spacer(month_marks)
            .x_axis_formatter(|mark, _| date_label(Date(mark.value.round() as i32)))
            .y_axis_formatter(|mark, _| money_short(mark.value))
            .label_formatter(|name, p| {
                let when = Date(p.x.round() as i32);
                let tag = if when > today { "  (projected)" } else { "" };
                format!("{name}\n{when}{tag}\n{}", fmt::amount(Money((p.y * 100.0).round() as i64)))
            })
            .show(ui, |plot| {
                for (i, s) in r.series.iter().take(MAX_SERIES).enumerate() {
                    if hidden.contains(&s.label) {
                        continue;
                    }
                    let colour = series_colour(i, dark);
                    let (past, ahead) = steps(&s.points, today);
                    plot.line(Line::new(s.label.clone(), past).color(colour).width(2.0_f32));
                    plot.line(
                        Line::new(s.label.clone(), ahead)
                            .color(colour)
                            .width(2.0_f32)
                            .style(LineStyle::Dashed { length: 8.0 }),
                    );
                    if let Some(j) = then.and_then(|(_, pairs)| pairs[i]) {
                        let earlier = &then.expect("paired").0.series[j];
                        let (past, ahead) = steps(&earlier.points, today);
                        let mut line = past;
                        line.extend(ahead);
                        plot.line(
                            Line::new(format!("{} (then)", s.label), line)
                                .color(colour.gamma_multiply(0.45))
                                .width(1.5_f32)
                                .style(LineStyle::Dotted { spacing: 4.0 }),
                        );
                    }
                    if let Some(t) = s.target {
                        plot.hline(
                            HLine::new(format!("{} target", s.label), t.cents() as f64 / 100.0)
                                .color(colour.gamma_multiply(0.7))
                                .width(1.0_f32)
                                .style(LineStyle::Dotted { spacing: 6.0 }),
                        );
                    }
                }
                plot.vline(VLine::new("today", today.0 as f64).color(fmt::dim()).width(1.0_f32));
            });

        ui.vertical(|ui| {
            ui.set_width(legend_width);
            legend(ui, r, then.is_some(), dark, &mut hidden);
        });
    });
    ui.data_mut(|d| d.insert_temp(hidden_id, hidden));

    if r.series.len() > MAX_SERIES {
        ui.label(
            RichText::new(format!(
                "{} more line(s) are in the table below but not drawn.",
                r.series.len() - MAX_SERIES
            ))
            .color(fmt::dim()),
        );
    }
}

/// The chart's key, beside it: a line per series, scrolling once it is
/// longer than the chart is tall. Click a line to hide it or bring it back.
fn legend(
    ui: &mut Ui,
    r: &ViewReport,
    comparing: bool,
    dark: bool,
    hidden: &mut std::collections::BTreeSet<String>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("LINES").small().color(fmt::dim()));
        if !hidden.is_empty() && ui.small_button("show all").clicked() {
            hidden.clear();
        }
    });
    let drawn = r.series.len().min(MAX_SERIES);
    egui::ScrollArea::vertical()
        .id_salt("view_legend")
        .max_height(CHART_HEIGHT - 44.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (i, s) in r.series.iter().take(drawn).enumerate() {
                let off = hidden.contains(&s.label);
                let colour = if off { fmt::dim() } else { series_colour(i, dark) };
                let clicked = ui
                    .horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 10.0), egui::Sense::hover());
                        ui.painter().hline(
                            rect.x_range(),
                            rect.center().y,
                            egui::Stroke::new(3.0_f32, colour),
                        );
                        let text = RichText::new(&s.label);
                        let text = if off { text.color(fmt::dim()).strikethrough() } else { text };
                        ui.add(egui::Label::new(text).truncate().sense(egui::Sense::click()))
                            .on_hover_text(format!(
                                "{}\nClick to {} this line.",
                                s.label,
                                if off { "show" } else { "hide" }
                            ))
                            .clicked()
                    })
                    .inner;
                if clicked && !hidden.remove(&s.label) {
                    hidden.insert(s.label.clone());
                }
            }
        });
    ui.add_space(4.0);
    let key = if comparing {
        "solid: to today \u{b7} dashed: simulated \u{b7} faint dotted: then"
    } else {
        "solid: to today \u{b7} dashed: simulated \u{b7} dotted: target"
    };
    ui.label(RichText::new(key).small().color(fmt::dim()));
}

/// Grid lines on the first of each month (or thinner, zoomed out; weekly,
/// zoomed in) rather than at arbitrary day counts.
fn month_marks(input: GridInput) -> Vec<GridMark> {
    let (lo, hi) = input.bounds;
    if !(lo.is_finite() && hi.is_finite()) || hi - lo > 365.0 * 200.0 {
        return Vec::new();
    }
    date_ticks(Date(lo.floor() as i32), Date(hi.ceil() as i32))
        .into_iter()
        .map(|(d, _)| GridMark { value: d.0 as f64, step_size: 30.0 })
        .collect()
}

fn date_label(d: Date) -> String {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let m = MONTHS[d.month() as usize - 1];
    match d.day() {
        1 => format!("{m} {}", d.year()),
        n => format!("{m} {n}"),
    }
}

// ------------------------------------------------------------------ tables

fn balances(ui: &mut Ui, r: &ViewReport) {
    if r.series.is_empty() {
        return;
    }
    let dark = ui.visuals().dark_mode;
    ui.label(RichText::new("Balances").strong());
    Table::new(
        "view_balances",
        vec![
            text("").max(300.0),
            figures("today"),
            figures("at end"),
            figures("change"),
            figures("lowest ahead"),
            figures("target").max(300.0),
        ],
    )
    .fit_to((r.start, r.end))
    .show(ui, r.series.len(), |row| {
        let i = row.index();
        let s = &r.series[i];
        row.col(|ui| {
            // A line key beside the name: identity is never colour on the
            // text itself.
            let colour = if i < MAX_SERIES { series_colour(i, dark) } else { fmt::dim() };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 10.0), egui::Sense::hover());
            ui.painter().hline(rect.x_range(), rect.center().y, egui::Stroke::new(3.0_f32, colour));
            ui.label(&s.label);
        });
        row.col(|ui| {
            num(ui, fmt::money_text(s.now));
        });
        row.col(|ui| {
            num(ui, fmt::money_text(s.at_end));
        });
        row.col(|ui| {
            num(ui, fmt::delta_text(s.at_end - s.now));
        });
        row.col(|ui| {
            let (d, low) = s.lowest_ahead;
            num(ui, fmt::mono(format!("{} on {d}", fmt::amount(low))).small());
        });
        row.col(|ui| {
            match (s.target, s.target_reached) {
                (None, _) => {}
                (Some(t), Some(d)) if d == r.today => {
                    num(
                        ui,
                        RichText::new(format!("{} reached", fmt::amount(t)))
                            .small()
                            .color(fmt::good()),
                    );
                }
                (Some(t), Some(d)) => {
                    num(ui, RichText::new(format!("{} on {d}", fmt::amount(t))).small().strong())
                        .on_hover_text(format!(
                            "Reaches its target in {} day(s), going by the simulation.",
                            d.0 - r.today.0
                        ));
                }
                (Some(t), None) => {
                    num(
                        ui,
                        RichText::new(format!("{} not by {}", fmt::amount(t), r.end))
                            .small()
                            .color(fmt::dim()),
                    );
                }
            };
        });
    });
    let ahead: Vec<_> =
        r.series.iter().flat_map(|s| s.alerts_ahead.iter().map(move |a| (s, a))).collect();
    if !ahead.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new("Alerts ahead").strong());
        for (s, (a, d)) in ahead {
            let msg = if a.message.is_empty() { "Alert".to_string() } else { a.message.clone() };
            ui.label(format!(
                "\u{26A0} {d}  {}: {msg}  ({} {})",
                s.label,
                a.when,
                fmt::amount(a.level)
            ));
        }
    }
}

fn flows(ui: &mut Ui, r: &ViewReport) {
    if r.flows.is_empty() {
        return;
    }
    let p = r.period;
    ui.label(RichText::new(format!("Flows per {p}")).strong());
    Table::new(
        "view_flows",
        vec![
            text("issuer").max(280.0),
            text("schedule").max(240.0),
            figures("each"),
            figures(format!("per {p}")),
            text("notes").max(360.0),
        ],
    )
    .fit_to(p.noun())
    .show(ui, r.flows.len(), |row| {
        let f = &r.flows[row.index()];
        row.col(|ui| {
            let name = RichText::new(&f.name);
            ui.label(if f.paused { name.color(fmt::dim()).strikethrough() } else { name });
        });
        row.col(|ui| {
            ui.label(RichText::new(f.schedule.describe()).color(fmt::dim()));
        });
        row.col(|ui| {
            match f.effect {
                Some(e) => num(ui, fmt::money_text(e)),
                None => num(ui, fmt::mono(fmt::amount(f.amount))),
            };
        });
        row.col(|ui| {
            match (f.rate(p), f.effect.is_some()) {
                (Some(m), true) => num(ui, fmt::money_text(m)),
                (Some(m), false) => num(ui, fmt::mono(fmt::amount(m))),
                (None, _) => num(ui, RichText::new("one-off").color(fmt::dim())),
            };
        });
        row.col(|ui| {
            let mut notes = Vec::new();
            if f.paused {
                notes.push("paused".to_string());
            } else if !f.simulated {
                notes.push("not simulated".to_string());
            }
            if f.effect.is_some_and(|e| e.is_zero()) {
                notes.push("does not move this view's money".to_string());
            }
            ui.label(RichText::new(notes.join("; ")).small().color(fmt::dim()));
        });
    });
    let t = r.flow_totals(p);
    ui.add_space(4.0);
    if r.directed {
        ui.horizontal(|ui| {
            ui.label(format!("Per {p}:"));
            ui.label(fmt::money_text(t.received));
            ui.label("in,");
            ui.label(fmt::money_text(-t.spent));
            ui.label("out, net");
            ui.label(fmt::delta_text(t.net()).strong());
        });
    } else {
        ui.label(format!("Per {p}: {} moved.", fmt::amount(t.volume)));
    }
}

fn timeline(ui: &mut Ui, r: &ViewReport) {
    let cols: &[&str] = if r.directed {
        &["in", "out", "net", "projected in", "projected out", "projected net"]
    } else {
        &["volume", "projected volume"]
    };
    let mut columns = vec![text("")];
    columns.extend(cols.iter().map(|h| figures(*h)));
    // Its own scroll: a daily view over ten years is thousands of rows.
    Table::new("view_timeline", columns)
        .height(Height::Max(420.0))
        .fit_to((r.period.noun(), r.directed))
        .show(ui, r.timeline.len(), |row| {
            let line = &r.timeline[row.index()];
            let cell = |row: &mut egui_extras::TableRow<'_, '_>, m: Money| {
                row.col(|ui| {
                    if m.is_zero() {
                        num(ui, RichText::new("-").color(fmt::dim()));
                    } else {
                        num(ui, fmt::mono(fmt::amount(m)));
                    }
                });
            };
            row.col(|ui| {
                ui.label(fmt::mono(period_label(r.period, line.start)));
            });
            let (a, f) = (&line.actual, &line.projected);
            if r.directed {
                cell(row, a.received);
                cell(row, a.spent);
                row.col(|ui| {
                    num(ui, fmt::delta_text(a.net()));
                });
                cell(row, f.received);
                cell(row, f.spent);
                row.col(|ui| {
                    num(ui, fmt::delta_text(f.net()));
                });
            } else {
                cell(row, a.volume);
                cell(row, f.volume);
            }
        });
}

pub(crate) fn period_label(p: Period, start: Date) -> String {
    match p {
        Period::Month => date_label(start),
        Period::Year => start.year().to_string(),
        Period::Week | Period::Day => start.to_string(),
    }
}

fn notes(ui: &mut Ui, l: &Budget, r: &ViewReport) {
    let mut notes = Vec::new();
    if r.overdue_occurrences > 0 {
        notes.push(format!(
            "{} overdue occurrence(s) are shown landing today. Run the issuers to post them.",
            r.overdue_occurrences
        ));
    }
    for ix in &r.cancelled {
        notes.push(format!(
            "{} is in both an added and a subtracted bucket, so it counts for nothing.",
            l.ledgers.name[ix.get()]
        ));
    }
    if !r.missing_buckets.is_empty() {
        notes.push(format!(
            "{} bucket(s) this view names no longer exist on this branch.",
            r.missing_buckets.len()
        ));
    }
    if !notes.is_empty() {
        ui.add_space(8.0);
        for n in notes {
            ui.label(RichText::new(n).color(fmt::dim()));
        }
    }
}

// ----------------------------------------------------------------- exports

fn export_chart(name: &str, r: &ViewReport) -> Action {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("PNG image", &["png"])
        .add_filter("SVG image", &["svg"])
        .set_file_name(format!("{name}.png"))
        .save_file()
    else {
        return Action::Note("Chart not saved.".into());
    };
    match ledgit_plot::Chart::from_view(name, r).save(&path, 1200, 560) {
        Ok(()) => Action::Note(format!("Chart saved to {}.", path.display())),
        Err(e) => Action::Fail(e),
    }
}

fn export_csv(name: &str, r: &ViewReport) -> Action {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("CSV", &["csv"])
        .set_file_name(format!("{name}.csv"))
        .save_file()
    else {
        return Action::Note("CSV not saved.".into());
    };
    match std::fs::write(&path, ledgit_plot::csv(r)) {
        Ok(()) => Action::Note(format!("Series saved to {}.", path.display())),
        Err(e) => Action::Fail(format!("{}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_hold_each_value_until_the_next_point_and_split_at_today() {
        let d = |n| Date(n);
        let pts = [(d(0), Money(100)), (d(5), Money(300)), (d(10), Money(200))];
        let (past, ahead) = steps(&pts, d(5));
        assert_eq!(past, vec![[0.0, 1.0], [5.0, 1.0], [5.0, 3.0]]);
        // The dashed part starts where the solid part ended.
        assert_eq!(ahead, vec![[5.0, 3.0], [10.0, 3.0], [10.0, 2.0]]);
    }
}
