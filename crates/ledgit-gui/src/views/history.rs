//! The work tree: history, branches, and the three operations that rewrite or
//! undo it.
//!
//! Revert is safe and always available. Switching branch with changes staged
//! asks first: shelve them on the branch you are leaving (they come back when
//! you return), or bring them along. Rebase refuses while anything is staged.

use super::graph::{self, Graph};
use super::{empty, heading, split};
use crate::app::{Screen, Session};
use crate::fmt;
use egui::{RichText, Ui};
use ledgit_core::prelude::*;
use std::rc::Rc;

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(ui, "History", "Every commit, every branch, and the two ways to take something back.");

    merged_banner(ui, s);

    let graph = match graph(s) {
        Ok(g) => g,
        Err(e) => {
            ui.colored_label(fmt::bad(), e.to_string());
            return;
        }
    };

    // Three panes, the outer two draggable, so the commit detail - the one
    // with the most to say - gets whatever the window can spare.
    split(ui, "history_branches", 250.0, s, branch_panel, |ui, s| {
        if graph.commits.is_empty() {
            empty(ui, "No commits yet. Stage something and commit it.");
            return;
        }
        split(
            ui,
            "history_log",
            380.0,
            s,
            |ui, s| log_list(ui, s, &graph),
            |ui, s| {
                egui::ScrollArea::vertical()
                    .id_salt("commit_detail")
                    .show(ui, |ui| commit_detail(ui, s, &graph.commits));
            },
        );
    });
}

/// Just merged a branch: offer to delete it.
fn merged_banner(ui: &mut Ui, s: &mut Session) {
    let Some(source) = s.merged.clone() else { return };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(format!("Merged {source}. It is still there to look back at."));
            if ui.button(format!("Delete branch {source}")).clicked() {
                match s.repo.delete_branch(&source) {
                    Ok(()) => s.note(format!("Deleted branch {source}.")),
                    Err(e) => s.fail(e),
                }
                s.merged = None;
            }
            if ui.button("Keep it").clicked() {
                s.merged = None;
            }
        });
    });
    ui.add_space(6.0);
}

/// How far back the graph reaches.
const GRAPH_LIMIT: usize = 500;

/// The commit graph, from the session's cache while no branch has moved.
fn graph(s: &mut Session) -> Result<Rc<Graph>> {
    let key = Graph::key(&s.repo);
    if let Some((k, g)) = &s.graph {
        if *k == key {
            return Ok(Rc::clone(g));
        }
    }
    let g = Rc::new(Graph::build(&s.repo, GRAPH_LIMIT)?);
    s.graph = Some((key, Rc::clone(&g)));
    Ok(g)
}

fn branch_panel(ui: &mut Ui, s: &mut Session) {
    ui.label(RichText::new("BRANCHES").small().color(fmt::dim()));
    ui.add_space(4.0);

    let branches = s.repo.branches().unwrap_or_default();
    let current = s.repo.head().branch_name().map(|n| n.to_string());
    let dirty = s.repo.has_staged_changes();

    let mut switch: Option<String> = None;
    for (name, id) in &branches {
        let is_current = current.as_deref() == Some(name.as_str());
        let shelved = if is_current { 0 } else { s.repo.shelved(name).unwrap_or(0) };
        ui.horizontal(|ui| {
            if !is_current
                && ui
                    .small_button("switch")
                    .on_hover_text(if dirty {
                        "You have staged changes; you will be asked what to do with them"
                    } else {
                        "Make this the budget you are looking at"
                    })
                    .clicked()
            {
                switch = Some(name.clone());
            }
            let label = if is_current {
                RichText::new(format!("\u{23FA} {name}")).strong()
            } else {
                RichText::new(name.as_str())
            };
            // Its colour in the graph: the branch list is the graph's key.
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            let at = branches.iter().position(|(n, _)| n == name);
            ui.painter().circle_filled(
                dot.center(),
                4.5,
                graph::colour(at, ui.visuals().dark_mode),
            );
            ui.add(egui::Label::new(label).truncate()).on_hover_text(name.as_str());
            ui.label(RichText::new(id.short()).monospace().small().color(fmt::dim()));
            if shelved > 0 {
                ui.label(RichText::new(format!("{shelved} shelved")).small().color(fmt::warn()))
                    .on_hover_text(
                        "Staged changes set aside when you switched away. They come back when \
                         you switch to this branch.",
                    );
            }
        });
    }
    if s.repo.head().branch_name().is_none() {
        ui.label(RichText::new(format!("HEAD is {}", s.repo.head())).color(fmt::dim()).small());
    }
    if let Some(to) = switch {
        if dirty {
            s.pending_switch = Some(to);
        } else {
            checkout(s, &to, StagedWork::Refuse);
        }
    }
    switch_prompt(ui, s, current.as_deref());

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut s.new_branch)
                .hint_text("new branch name")
                .desired_width(150.0),
        );
        if ui.button("Branch here").clicked() {
            let name = s.new_branch.trim().to_string();
            match s.repo.checkout_new(&name) {
                Ok(()) => {
                    s.new_branch.clear();
                    s.note(format!("Created {name} and switched to it."));
                }
                Err(e) => s.fail(e),
            }
        }
    });
    ui.label(
        RichText::new("A branch is a what-if budget: the same ledgers, a different future.")
            .small()
            .color(fmt::dim()),
    );

    ui.add_space(16.0);
    ui.label(RichText::new("REBASE").small().color(fmt::dim()));
    ui.label(
        RichText::new("Replay this branch's commits on top of another one.")
            .small()
            .color(fmt::dim()),
    );
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut s.rebase_onto)
                .hint_text("onto...")
                .desired_width(150.0),
        );
        let can = current.is_some() && !dirty && !s.rebase_onto.trim().is_empty();
        if ui
            .add_enabled(can, egui::Button::new("Rebase"))
            .on_disabled_hover_text("Needs a branch, a target, and a clean staging area")
            .clicked()
        {
            let (branch, onto) =
                (current.clone().unwrap_or_default(), s.rebase_onto.trim().to_string());
            match s.repo.rebase(&branch, &onto) {
                Ok(0) => s.note(format!("{branch} was already up to date with {onto}.")),
                Ok(n) => {
                    s.rebase_onto.clear();
                    s.note(format!("Replayed {n} commit(s) onto {onto}."));
                }
                Err(e) => s.fail(e),
            }
        }
    });

    merge_panel(ui, s, current.as_deref(), &branches);
}

/// Bring another branch's changes into this one, three ways.
fn merge_panel(
    ui: &mut Ui,
    s: &mut Session,
    current: Option<&str>,
    branches: &[(String, CommitId)],
) {
    ui.add_space(16.0);
    let here = current.unwrap_or("HEAD");
    ui.label(
        RichText::new(format!("MERGE INTO {}", here.to_uppercase())).small().color(fmt::dim()),
    );
    let others: Vec<&str> =
        branches.iter().map(|(n, _)| n.as_str()).filter(|n| Some(*n) != current).collect();
    if others.is_empty() {
        ui.label(RichText::new("No other branch to merge.").small().color(fmt::dim()));
        return;
    }
    if !others.contains(&s.merge_source.as_str()) {
        s.merge_source = others[0].to_string();
    }
    egui::ComboBox::from_id_salt("merge_source")
        .selected_text(s.merge_source.clone())
        .width(180.0)
        .show_ui(ui, |ui| {
            for name in &others {
                ui.selectable_value(&mut s.merge_source, name.to_string(), *name);
            }
        });
    let dirty = s.repo.has_staged_changes();
    let source = s.merge_source.clone();
    ui.horizontal_wrapped(|ui| {
        for (kind, hover) in [
            (MergeKind::Replace, "End up exactly like the other branch."),
            (MergeKind::Reconcile, "Decide each entry the two branches disagree on."),
            (MergeKind::Adopt, "Bring its issuers and settings only. No money moves."),
        ] {
            if ui
                .add_enabled(!dirty, egui::Button::new(kind.name()))
                .on_hover_text(hover)
                .on_disabled_hover_text("Commit or discard what is staged first")
                .clicked()
            {
                super::merge::start(s, &source, kind);
            }
        }
    });
    ui.label(
        RichText::new(
            "Nothing here is deleted: a merge is one new commit, and you review it first.",
        )
        .small()
        .color(fmt::dim()),
    );
}

/// Asked when a switch is picked with changes staged.
fn switch_prompt(ui: &mut Ui, s: &mut Session, current: Option<&str>) {
    let Some(to) = s.pending_switch.clone() else { return };
    if !s.repo.has_staged_changes() {
        s.pending_switch = None;
        checkout(s, &to, StagedWork::Refuse);
        return;
    }
    let staged = s.repo.staged().len();
    ui.add_space(8.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(format!("Switch to {to}?")).strong());
        ui.label(
            RichText::new(format!(
                "You have {staged} staged change(s). They belong to the budget you entered them on."
            ))
            .small(),
        );
        ui.add_space(4.0);
        match current {
            Some(here) => {
                if ui
                    .button(format!("Shelve them on {here}"))
                    .on_hover_text(format!(
                        "Set them aside. They come back, as they are, when you switch to {here} again."
                    ))
                    .clicked()
                {
                    checkout(s, &to, StagedWork::Shelve);
                }
            }
            None => {
                ui.label(
                    RichText::new("HEAD is detached, so there is no branch to shelve them on.")
                        .small()
                        .color(fmt::dim()),
                );
            }
        }
        if ui
            .button(format!("Bring them to {to}"))
            .on_hover_text(
                "Stage them on the other branch instead. Any that do not apply there - an entry \
                 to a ledger it never opened - are flagged on the Commit screen, not lost.",
            )
            .clicked()
        {
            checkout(s, &to, StagedWork::Bring);
        }
        if ui.button("Cancel").clicked() {
            s.pending_switch = None;
        }
    });
}

fn checkout(s: &mut Session, to: &str, work: StagedWork) {
    let staged = s.repo.staged().len();
    let leaving = s.repo.head().to_string();
    match s.repo.checkout_with(to, work) {
        Ok(restored) => {
            s.pending_switch = None;
            s.selected_commit = None;
            let mut msg = format!("Now on {to}.");
            match work {
                StagedWork::Shelve if staged > 0 => {
                    msg.push_str(&format!(" Shelved {staged} change(s) on {leaving}."))
                }
                StagedWork::Bring if staged > 0 => {
                    msg.push_str(&format!(" Brought {staged} staged change(s) along."))
                }
                _ => {}
            }
            if restored > 0 {
                msg.push_str(&format!(" Put back {restored} change(s) shelved here."));
            }
            let broken = s.repo.broken().len();
            if broken > 0 {
                msg.push_str(&format!(" {broken} no longer apply; see Commit."));
            }
            s.note(msg);
        }
        Err(e) => s.fail(e),
    }
}

fn log_list(ui: &mut Ui, s: &mut Session, g: &Graph) {
    ui.label(RichText::new("COMMITS, EVERY BRANCH").small().color(fmt::dim()));
    ui.add_space(4.0);
    if s.selected_commit.is_none_or(|id| !g.commits.iter().any(|c| c.id == id)) {
        s.selected_commit = g.head.or(g.commits.first().map(|c| c.id));
    }
    super::graph::show(ui, g, &mut s.selected_commit);
}

fn commit_detail(ui: &mut Ui, s: &mut Session, commits: &[Commit]) {
    let Some(id) = s.selected_commit else { return };
    let Some(c) = commits.iter().find(|c| c.id == id) else { return };

    ui.label(RichText::new(c.id.to_string()).monospace().small().color(fmt::dim()));
    ui.heading(c.summary());
    ui.label(RichText::new(format!("by {}", c.author)).color(fmt::dim()));
    if c.message.lines().count() > 1 {
        ui.add_space(6.0);
        ui.label(c.message.lines().skip(1).collect::<Vec<_>>().join("\n"));
    }
    if let Some(m) = c.merged {
        let from = s
            .repo
            .branches()
            .unwrap_or_default()
            .into_iter()
            .find(|(_, tip)| *tip == m.from)
            .map(|(n, _)| n)
            .unwrap_or_else(|| m.from.short());
        ui.label(RichText::new(format!("{} merge from {from}", m.kind.name())).color(fmt::good()))
            .on_hover_text(format!("Its changes came from commit {}", m.from.short()));
    }
    ui.add_space(10.0);

    ui.label(RichText::new(format!("{} operation(s)", c.ops.len())).small().color(fmt::dim()));
    egui::ScrollArea::vertical().max_height(340.0).id_salt("ops").show(ui, |ui| {
        for op in &c.ops {
            ui.label(RichText::new(format!("\u{2022} {}", op.summary())).small());
        }
    });

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if ui
            .button("Revert this commit")
            .on_hover_text(
                "Stages the mirror-image entries. Nothing is deleted, and you review it before it lands.",
            )
            .clicked()
        {
            let rev = c.id.to_string();
            match s.repo.revert(&rev) {
                Ok(notes) => {
                    let staged = s.repo.staged().len();
                    let mut msg = format!("Staged {staged} reversing change(s).");
                    for n in &notes {
                        msg.push_str("  Note: ");
                        msg.push_str(n);
                    }
                    s.note(msg);
                    if staged > 0 {
                        s.goto = Some(Screen::Commit);
                    }
                }
                Err(e) => s.fail(e),
            }
        }
    });
    let on_branch = s.repo.head().branch_name().map(|n| n.to_string());
    if let Some(here) = on_branch {
        if ui
            .button(format!("Cherry-pick onto {here}"))
            .on_hover_text(
                "Stage this commit's changes on the branch you are on, exactly as they were - the same entries, so a later merge knows them.",
            )
            .clicked()
        {
            match s.repo.cherry_pick(&c.id.to_string()) {
                Ok((0, skipped)) => s.note(format!("Nothing to pick: all {skipped} change(s) are already here.")),
                Ok((staged, skipped)) => {
                    let broken = s.repo.broken().len();
                    let mut msg = format!("Staged {staged} change(s) from {}.", c.id.short());
                    if skipped > 0 {
                        msg.push_str(&format!(" {skipped} already here, skipped."));
                    }
                    if broken > 0 {
                        msg.push_str(&format!(" {broken} do not apply here: fix or drop them on the Commit screen."));
                    }
                    s.note(msg);
                    s.goto = Some(Screen::Commit);
                }
                Err(e) => s.fail(e),
            }
        }
    }
    ui.label(
        RichText::new(
            "Reverting posts a new transaction with debit and credit swapped, so both the \
             mistake and the correction stay in the register.",
        )
        .small()
        .color(fmt::dim()),
    );

    ui.add_space(16.0);
    branch_from(ui, s, c);
}

/// Start a branch at this commit: the budget exactly as it stood then. A
/// ledger opened after it does not exist there - the way to "delete" one
/// without rewriting history, since history keeps everything.
fn branch_from(ui: &mut Ui, s: &mut Session, c: &Commit) {
    ui.label(RichText::new("BRANCH FROM HERE").small().color(fmt::dim()));
    ui.label(
        RichText::new(
            "A new branch starting from this commit: the budget as it stood then. Anything \
             created after it - a ledger you regret, say - is not on that branch. The branch \
             you are on is not touched.",
        )
        .small()
        .color(fmt::dim()),
    );
    let mut create: Option<bool> = None;
    ui.horizontal(|ui| {
        let edit = ui.add(
            egui::TextEdit::singleline(&mut s.branch_at)
                .hint_text("new branch name")
                .desired_width(180.0),
        );
        let named = !s.branch_at.trim().is_empty();
        if ui.add_enabled(named, egui::Button::new("Create branch")).clicked() {
            create = Some(false);
        }
        if ui
            .add_enabled(named, egui::Button::new("Create and switch"))
            .on_hover_text("If you have staged changes you will be asked what to do with them")
            .clicked()
            || (named && edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        {
            create = Some(true);
        }
    });
    let Some(switch) = create else { return };
    let name = s.branch_at.trim().to_string();
    match s.repo.branch(&name, Some(&c.id.to_string())) {
        Ok(at) => {
            s.branch_at.clear();
            s.note(format!("Created {name} at {}.", at.short()));
            if switch {
                if s.repo.has_staged_changes() {
                    // The branch panel asks: shelve or bring.
                    s.pending_switch = Some(name);
                } else {
                    checkout(s, &name, StagedWork::Refuse);
                }
            }
        }
        Err(e) => s.fail(e),
    }
}
