//! Deciding a merge: what the two branches disagree on, and what to do
//! about each of it.
//!
//! Reached from History with a source branch and a kind picked. Nothing is
//! written until "Merge"; the screen only edits the choices on the session,
//! and the ledgers-affected tables redraw from them as they change.

use super::{empty, heading, num};
use crate::app::{Screen, Session};
use crate::fmt;
use crate::table::{figures, text, Height, Table};
use egui::{RichText, Ui};
use ledgit_core::merge::{Case, Row};
use ledgit_core::prelude::*;

/// A merge being decided.
pub struct Draft {
    pub preview: MergePreview,
    pub choices: Choices,
    pub message: String,
    /// The report for the choices it was worked out for: building it
    /// applies the whole merge to a copy of the budget, so it is redone only
    /// when a choice changes, not every frame.
    report: Option<(Choices, std::result::Result<ChangeReport, String>)>,
}

impl Draft {
    pub fn new(preview: MergePreview) -> Draft {
        Draft { preview, choices: Choices::default(), message: String::new(), report: None }
    }

    fn report(&mut self) -> &std::result::Result<ChangeReport, String> {
        if self.report.as_ref().is_none_or(|(c, _)| *c != self.choices) {
            let r = self.preview.report(&self.choices).map_err(|e| e.to_string());
            self.report = Some((self.choices.clone(), r));
        }
        &self.report.as_ref().expect("just set").1
    }
}

/// Preview a merge of `source` into the current branch and open the screen
/// on it.
pub fn start(s: &mut Session, source: &str, kind: MergeKind) {
    match s.repo.merge_preview(source, kind) {
        Ok(p) => {
            s.merge = Some(Draft::new(p));
            s.goto = Some(Screen::Merge);
        }
        Err(e) => s.fail(e),
    }
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    let Some(mut draft) = s.merge.take() else {
        heading(ui, "Merge", "");
        empty(ui, "Pick a branch to merge on the History screen.");
        return;
    };
    let here = s.repo.head().to_string();
    let p = &draft.preview;
    heading(
        ui,
        &format!("{} {} into {here}", p.kind.name(), p.source),
        match p.kind {
            MergeKind::Replace => {
                "This branch ends exactly like the other one. Nothing here is deleted: what differs is reversed, and what is missing is posted, in one new commit."
            }
            MergeKind::Reconcile => {
                "Decide each entry the two branches disagree on. Nothing here is deleted: Revert posts a reversal, and the merge is one new commit."
            }
            MergeKind::Adopt => {
                "Bring the other branch's plans - its issuers and settings - and none of its transactions. No balance moves."
            }
        },
    );

    let mut action: Option<Action> = None;
    let unresolved = p.unresolved(&draft.choices);

    if p.fast_forward {
        ui.label(format!(
            "Nothing has happened on {here} since {} split off, so this just moves {here} up to it.",
            p.source
        ));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(format!("Move {here} up to {}", p.source)).clicked() {
                action = Some(Action::Merge);
            }
            if ui.button("Cancel").clicked() {
                action = Some(Action::Cancel);
            }
        });
    } else if p.is_empty() {
        empty(ui, "The two branches already agree. There is nothing to merge.");
        if ui.button("Back to History").clicked() {
            action = Some(Action::Cancel);
        }
    } else {
        // The bar: message, and the buttons that act.
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut draft.message)
                    .hint_text(format!("{} {} into {here}", p.kind.name(), p.source))
                    .desired_width(320.0),
            );
            let ready = unresolved.is_empty();
            if ui
                .add_enabled(ready, egui::Button::new("Merge"))
                .on_disabled_hover_text("Decide every highlighted clash first")
                .clicked()
            {
                action = Some(Action::Merge);
            }
            if !ready {
                ui.label(
                    RichText::new(format!("{} clash(es) to decide", unresolved.len()))
                        .color(fmt::warn())
                        .strong(),
                );
                if ui
                    .button("Merge unresolved as Replace")
                    .on_hover_text(
                        "Your choices stand. Each undecided clash takes the other branch's side: this branch's entries are reversed and the other's posted; a clashing issuer replaces this branch's.",
                    )
                    .clicked()
                {
                    action = Some(Action::MergeAsReplace);
                }
            }
            if ui.button("Cancel").clicked() {
                action = Some(Action::Cancel);
            }
        });
        ui.add_space(8.0);
        egui::ScrollArea::vertical().id_salt("merge_body").show(ui, |ui| {
            body(ui, &mut draft, &here);
        });
    }

    match action {
        None => s.merge = Some(draft),
        Some(Action::Cancel) => s.goto = Some(Screen::History),
        Some(a) => {
            if a == Action::MergeAsReplace {
                draft.choices.unresolved_as_replace = true;
            }
            let message = draft.message.trim().to_string();
            match s.repo.merge(&draft.preview, &draft.choices, message) {
                Ok(out) => {
                    let source = draft.preview.source.clone();
                    s.note(if out.fast_forward {
                        format!("Moved {here} up to {source}.")
                    } else {
                        format!(
                            "Merged {source} into {here} in commit {} ({} change(s)).",
                            out.commit.short(),
                            out.ops
                        )
                    });
                    // Offer to delete it only if it is a branch, not a
                    // commit picked by id.
                    if s.repo.branches().unwrap_or_default().iter().any(|(n, _)| *n == source) {
                        s.merged = Some(source);
                    }
                    s.selected_commit = Some(out.commit);
                    s.goto = Some(Screen::History);
                }
                Err(e) => {
                    draft.choices.unresolved_as_replace = false;
                    s.fail(e);
                    s.merge = Some(draft);
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Merge,
    MergeAsReplace,
    Cancel,
}

fn body(ui: &mut Ui, d: &mut Draft, here: &str) {
    let p = &d.preview;
    let source = p.source.clone();
    let rows_in = |case: Case| -> Vec<usize> {
        (0..p.rows.len()).filter(|k| p.rows[*k].case == case).collect()
    };

    let mut set: Vec<(TxUid, RowChoice)> = Vec::new();
    let mut set_group: Vec<(usize, GroupChoice)> = Vec::new();

    let only_here = rows_in(Case::DestinationOnly);
    if !only_here.is_empty() {
        section(ui, &format!("ONLY ON {}", here.to_uppercase()), "On ledgers the other branch left alone. Keep leaves them; Revert posts a reversal in the merge.");
        rows_table(ui, p, &d.choices, &only_here, "merge_here", here, &source, &mut set);
    }
    let only_there = rows_in(Case::SourceOnly);
    if !only_there.is_empty() {
        section(
            ui,
            &format!("ONLY ON {}", source.to_uppercase()),
            "On ledgers left alone here. Keep posts them here; Drop leaves them behind.",
        );
        rows_table(ui, p, &d.choices, &only_there, "merge_there", here, &source, &mut set);
    }

    for (g, group) in p.groups.iter().enumerate() {
        let names: Vec<String> = group.ledgers.iter().map(|u| ledger_name(p, *u)).collect();
        let chosen = d.choices.groups.get(&g).copied();
        let open = group.rows.iter().any(|k| p.choice(*k, &d.choices).is_none());
        ui.add_space(12.0);
        let frame = egui::Frame::group(ui.style()).stroke(egui::Stroke::new(
            if open { 1.5_f32 } else { 1.0_f32 },
            if open { fmt::warn() } else { ui.visuals().widgets.noninteractive.bg_stroke.color },
        ));
        frame.show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("Both branches changed {}", names.join(", ")))
                        .strong()
                        .color(if open { fmt::warn() } else { ui.visuals().text_color() }),
                );
                if p.kind == MergeKind::Replace {
                    ui.label(RichText::new("- the other branch's side").color(fmt::dim()));
                    return;
                }
                ui.separator();
                for (c, label, hover) in [
                    (GroupChoice::Force, "Force", "Keep this branch's and post the other's. Both stand - this can double-post."),
                    (GroupChoice::Revert, "Revert", "Reverse this branch's entries and post the other's."),
                    (GroupChoice::Drop, "Drop", "Keep this branch's; leave the other's behind."),
                ] {
                    if ui.selectable_label(chosen == Some(c), label).on_hover_text(hover).clicked() {
                        set_group.push((g, c));
                    }
                }
                if open {
                    ui.label(RichText::new("undecided").color(fmt::warn()));
                }
            });
            if chosen == Some(GroupChoice::Force) {
                ui.label(
                    RichText::new("Force keeps both sides: if they are the same payment, it is now posted twice.")
                        .color(fmt::bad())
                        .small(),
                );
            }
            rows_table(ui, p, &d.choices, &group.rows, &format!("merge_group_{g}"), here, &source, &mut set);
        });
    }

    let mut set_issuer: Vec<(IssuerUid, IssuerChoice)> = Vec::new();
    if !p.issuers.is_empty() {
        section(ui, &format!("ISSUERS FROM {}", source.to_uppercase()), "Bring them, or leave them behind. One that posts to the same ledgers as an issuer here is probably the same bill: keep both, keep this branch's, or take the other's (this branch's is paused - issuers are never deleted).");
        for item in &p.issuers {
            let chosen = p.issuer_choice(item, &d.choices);
            ui.horizontal_wrapped(|ui| {
                let open = chosen.is_none();
                ui.label(RichText::new(&item.name).strong().color(if open {
                    fmt::warn()
                } else {
                    ui.visuals().text_color()
                }));
                let options: &[(IssuerChoice, &str)] = if item.clashes.is_empty() {
                    &[(IssuerChoice::Bring, "Bring"), (IssuerChoice::Leave, "Leave")]
                } else {
                    &[
                        (IssuerChoice::Bring, "Keep both"),
                        (IssuerChoice::Leave, "Keep this branch's"),
                        (IssuerChoice::TakeBranch, "Take the other's"),
                    ]
                };
                for (c, label) in options {
                    if ui.selectable_label(chosen == Some(*c), *label).clicked() {
                        set_issuer.push((item.uid, *c));
                    }
                }
                if !item.clashes.is_empty() {
                    let dest = p.destination();
                    let names: Vec<String> = item
                        .clashes
                        .iter()
                        .filter_map(|u| dest.issuers.ix(*u))
                        .map(|ix| dest.issuers.name[ix.get()].clone())
                        .collect();
                    ui.label(
                        RichText::new(format!("same ledgers as {}", names.join(", ")))
                            .small()
                            .color(if open { fmt::warn() } else { fmt::dim() }),
                    );
                }
            });
        }
    }

    let mut set_setting: Vec<(String, bool)> = Vec::new();
    if !p.settings.is_empty() {
        section(ui, "SETTINGS", "Names, goals, views, buckets, variables, pauses and amounts set ahead that the other branch changed.");
        Table::new(
            "merge_settings",
            vec![
                text("setting").max(360.0),
                text(here).max(260.0),
                text(source.as_str()).max(260.0),
                text("").max(220.0),
            ],
        )
        .height(Height::Content)
        .show(ui, p.settings.len(), |row| {
            let item = &p.settings[row.index()];
            let takes = p.takes(item, &d.choices);
            row.col(|ui| {
                ui.label(&item.label);
            });
            row.col(|ui| {
                ui.label(RichText::new(&item.ours).color(if takes {
                    fmt::dim()
                } else {
                    ui.visuals().text_color()
                }));
            });
            row.col(|ui| {
                ui.label(RichText::new(&item.theirs).color(if takes {
                    ui.visuals().text_color()
                } else {
                    fmt::dim()
                }));
            });
            row.col(|ui| {
                if p.kind == MergeKind::Replace {
                    ui.label(RichText::new("theirs").color(fmt::dim()));
                    return;
                }
                ui.horizontal(|ui| {
                    if ui.selectable_label(!takes, "ours").clicked() {
                        set_setting.push((item.key.clone(), false));
                    }
                    if ui.selectable_label(takes, "theirs").clicked() {
                        set_setting.push((item.key.clone(), true));
                    }
                    if item.both_changed {
                        ui.label(RichText::new("changed here too").small().color(fmt::warn()));
                    }
                });
            });
        });
    }

    // What it comes to, once everything is decided.
    ui.add_space(14.0);
    if p.unresolved(&d.choices).is_empty() {
        match d.report() {
            Ok(r) if r.ledger_deltas.is_empty() => {
                ui.label(RichText::new("No balance changes.").color(fmt::dim()));
            }
            Ok(r) => {
                super::commit::ledgers_table(ui, r);
                ui.add_space(10.0);
                super::commit::buckets_table(ui, r);
            }
            Err(e) => {
                ui.colored_label(fmt::bad(), e.to_string());
            }
        }
    } else {
        ui.label(
            RichText::new("The balances this merge changes show here once every clash is decided.")
                .color(fmt::dim()),
        );
    }

    for (tx, c) in set {
        d.choices.rows.insert(tx, c);
    }
    for (g, c) in set_group {
        // A group choice resets the rows inside it to follow the group.
        for k in &d.preview.groups[g].rows {
            d.choices.rows.remove(&d.preview.rows[*k].tx.uid);
        }
        d.choices.groups.insert(g, c);
    }
    for (u, c) in set_issuer {
        d.choices.issuers.insert(u, c);
    }
    for (k, take) in set_setting {
        d.choices.settings.insert(k, take);
    }
}

fn section(ui: &mut Ui, title: &str, note: &str) {
    ui.add_space(12.0);
    ui.label(RichText::new(title).small().color(fmt::dim()));
    ui.label(RichText::new(note).small().color(fmt::dim()));
    ui.add_space(4.0);
}

fn ledger_name(p: &MergePreview, uid: LedgerUid) -> String {
    let (d, s) = (p.destination(), p.branch());
    match (d.ledgers.ix(uid), s.ledgers.ix(uid)) {
        (Some(ix), _) => d.ledgers.name[ix.get()].clone(),
        (None, Some(ix)) => s.ledgers.name[ix.get()].clone(),
        _ => uid.short(),
    }
}

#[allow(clippy::too_many_arguments)]
fn rows_table(
    ui: &mut Ui,
    p: &MergePreview,
    choices: &Choices,
    rows: &[usize],
    id: &str,
    here: &str,
    source: &str,
    set: &mut Vec<(TxUid, RowChoice)>,
) {
    Table::new(
        id,
        vec![
            text("branch").max(160.0),
            text("date"),
            text("entry").max(300.0),
            text("ledgers").max(320.0),
            figures("amount"),
            text("").max(220.0),
        ],
    )
    .height(Height::Content)
    .show(ui, rows.len(), |row| {
        let k = rows[row.index()];
        let r: &Row = &p.rows[k];
        let choice = p.choice(k, choices);
        let undone = matches!(choice, Some(RowChoice::Revert | RowChoice::Drop));
        row.col(|ui| {
            let (name, colour) = match r.side {
                Side::Destination => (here, ui.visuals().text_color()),
                Side::Source => (source, fmt::good()),
            };
            ui.label(RichText::new(name).color(colour));
        });
        row.col(|ui| {
            ui.label(fmt::mono(r.tx.date.to_string()));
        });
        row.col(|ui| {
            let t = RichText::new(&r.tx.name);
            let t = if undone { t.strikethrough().color(fmt::dim()) } else { t };
            let resp = ui.label(t);
            if r.catch_up {
                resp.on_hover_text("Its issuer, run on the other branch up to where it has run here, so reverting this branch's run leaves no gap.");
            }
        });
        row.col(|ui| {
            let names: Vec<String> = r.tx.legs.iter().map(|g| ledger_name(p, g.ledger)).collect();
            ui.label(RichText::new(names.join(", ")).small().color(fmt::dim()));
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(r.tx.amount())));
        });
        row.col(|ui| {
            if p.kind == MergeKind::Replace {
                let t = match choice {
                    Some(RowChoice::Revert) => "reversed",
                    _ => "kept",
                };
                ui.label(RichText::new(t).color(fmt::dim()));
                return;
            }
            ui.horizontal(|ui| {
                let options: &[(RowChoice, &str)] = match r.side {
                    Side::Destination => &[(RowChoice::Keep, "Keep"), (RowChoice::Revert, "Revert")],
                    Side::Source => &[(RowChoice::Keep, "Keep"), (RowChoice::Drop, "Drop")],
                };
                for (c, label) in options {
                    if ui.selectable_label(choice == Some(*c), *label).clicked() {
                        set.push((r.tx.uid, *c));
                    }
                }
            });
        });
    });
}
