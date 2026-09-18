//! The work tree: history, branches, and the three operations that rewrite or
//! undo it.
//!
//! Revert is safe and always available. Checkout and rebase refuse while
//! anything is staged, because the staging area belongs to the branch it was
//! entered on.

use super::{empty, heading};
use crate::app::{Session, View};
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

    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(300.0);
            branch_panel(ui, s);
        });
        ui.separator();
        ui.vertical(|ui| {
            if commits.is_empty() {
                empty(ui, "No commits yet. Stage something and commit it.");
                return;
            }
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(360.0);
                    log_list(ui, s, &commits);
                });
                ui.separator();
                ui.vertical(|ui| commit_detail(ui, s, &commits));
            });
        });
    });
}

fn branch_panel(ui: &mut Ui, s: &mut Session) {
    ui.label(RichText::new("BRANCHES").small().color(fmt::dim()));
    ui.add_space(4.0);

    let branches = s.repo.branches().unwrap_or_default();
    let current = s.repo.head().branch_name().map(|n| n.to_string());
    let dirty = s.repo.has_staged_changes();

    for (name, id) in &branches {
        ui.horizontal(|ui| {
            let is_current = current.as_deref() == Some(name.as_str());
            ui.label(if is_current {
                RichText::new(format!("\u{25cf} {name}")).strong()
            } else {
                RichText::new(format!("   {name}"))
            });
            ui.label(RichText::new(id.short()).monospace().small().color(fmt::dim()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !is_current
                    && ui
                        .add_enabled(!dirty, egui::Button::new("switch").small())
                        .on_disabled_hover_text("Commit or discard your staged changes first")
                        .clicked()
                {
                    match s.repo.checkout(name) {
                        Ok(()) => {
                            s.selected_commit = None;
                            s.note(format!("Now on {name}."));
                        }
                        Err(e) => s.fail(e),
                    }
                }
            });
        });
    }
    if s.repo.head().branch_name().is_none() {
        ui.label(RichText::new(format!("HEAD is {}", s.repo.head())).color(fmt::dim()).small());
    }

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

fn log_list(ui: &mut Ui, s: &mut Session, commits: &[Commit]) {
    ui.label(RichText::new("COMMITS").small().color(fmt::dim()));
    ui.add_space(4.0);
    if s.selected_commit.is_none() {
        s.selected_commit = commits.first().map(|c| c.id);
    }
    let branches = s.repo.branches().unwrap_or_default();

    egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
        for c in commits {
            let tips: Vec<&str> =
                branches.iter().filter(|(_, id)| *id == c.id).map(|(n, _)| n.as_str()).collect();
            let selected = s.selected_commit == Some(c.id);
            let response = ui.selectable_label(
                selected,
                RichText::new(format!(
                    "{}  {}{}",
                    c.id.short(),
                    c.summary(),
                    if tips.is_empty() {
                        String::new()
                    } else {
                        format!("  [{}]", tips.join(", "))
                    }
                )),
            );
            if response.clicked() {
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
                        s.goto = Some(View::Commit);
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
