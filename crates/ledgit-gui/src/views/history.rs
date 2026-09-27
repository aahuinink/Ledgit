//! The work tree: history, branches, and the three operations that rewrite or
//! undo it.
//!
//! Revert is safe and always available. Switching branch with changes staged
//! asks first: shelve them on the branch you are leaving (they come back when
//! you return), or bring them along. Rebase refuses while anything is staged.

use super::{empty, heading, split};
use crate::app::{Screen, Session};
use crate::fmt;
use egui::{RichText, Ui};
use ledgit_core::prelude::*;

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(ui, "History", "Every commit, every branch, and the two ways to take something back.");

    let commits = match s.repo.log(Some(200)) {
        Ok(c) => c,
        Err(e) => {
            ui.colored_label(fmt::bad(), e.to_string());
            return;
        }
    };

    // Three panes, the outer two draggable, so the commit detail - the one
    // with the most to say - gets whatever the window can spare.
    split(ui, "history_branches", 250.0, s, branch_panel, |ui, s| {
        if commits.is_empty() {
            empty(ui, "No commits yet. Stage something and commit it.");
            return;
        }
        split(
            ui,
            "history_log",
            320.0,
            s,
            |ui, s| log_list(ui, s, &commits),
            |ui, s| {
                egui::ScrollArea::vertical()
                    .id_salt("commit_detail")
                    .show(ui, |ui| commit_detail(ui, s, &commits));
            },
        );
    });
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

fn log_list(ui: &mut Ui, s: &mut Session, commits: &[Commit]) {
    ui.label(RichText::new("COMMITS").small().color(fmt::dim()));
    ui.add_space(4.0);
    if s.selected_commit.is_none() {
        s.selected_commit = commits.first().map(|c| c.id);
    }
    let branches = s.repo.branches().unwrap_or_default();

    egui::ScrollArea::vertical().id_salt("commit_log").auto_shrink([false, false]).show(ui, |ui| {
        // One line per commit however narrow the pane; the rest on hover.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        for c in commits {
            let tips: Vec<&str> =
                branches.iter().filter(|(_, id)| *id == c.id).map(|(n, _)| n.as_str()).collect();
            let selected = s.selected_commit == Some(c.id);
            let tips =
                if tips.is_empty() { String::new() } else { format!("  [{}]", tips.join(", ")) };
            let text = format!("{}  {}{tips}", c.id.short(), c.summary());
            if ui.selectable_label(selected, &text).on_hover_text(&text).clicked() {
                s.selected_commit = Some(c.id);
            }
        }
    });
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
    ui.label(
        RichText::new(
            "Reverting posts a new transaction with debit and credit swapped, so both the \
             mistake and the correction stay in the register.",
        )
        .small()
        .color(fmt::dim()),
    );
}
