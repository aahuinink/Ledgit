//! Buckets: named groups of ledgers, and the totals you read off them.

use super::{empty, heading, num};
use crate::app::{Screen, Session};
use crate::fmt;
use crate::forms::FormKind;
use crate::picker::{Pick, Picker};
use egui::{RichText, Ui};
use ledgit_core::id::LedgerIx;
use ledgit_core::prelude::*;

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(
        ui,
        "Buckets",
        "A bucket is a view over ledgers. Creating or deleting one moves no money.",
    );

    ui.horizontal(|ui| {
        if ui.button("New bucket").clicked() {
            s.forms.open(FormKind::Bucket, s.repo.working());
        }
        ui.separator();
        // Buckets deliberately cannot contain other buckets, so this is how you
        // get a figure spanning several: total them together at read time.
        let combining = !s.bucket_combo.is_empty();
        if ui
            .selectable_label(combining, "Combine buckets")
            .on_hover_text("Total several buckets at once, e.g. Cash - Receivables")
            .clicked()
        {
            if combining {
                s.bucket_combo.clear();
            } else if let Some(uid) = s.selected_bucket {
                // Seed with whatever is on screen, so the first click shows the
                // same number rather than an empty panel.
                s.bucket_combo = vec![Term::plus(uid)];
            }
        }
    });
    ui.add_space(8.0);

    let live: Vec<BucketUid> =
        s.budget().buckets.live().map(|ix| s.budget().buckets.uid[ix.get()]).collect();
    if live.is_empty() {
        empty(ui, "No buckets yet.");
        return;
    }
    if s.selected_bucket.is_none_or(|b| !live.contains(&b)) {
        s.selected_bucket = live.first().copied();
    }

    let combining = !s.bucket_combo.is_empty();
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(200.0);
            for uid in &live {
                let Some(ix) = s.budget().buckets.ix(*uid) else { continue };
                let name = s.budget().buckets.name[ix.get()].clone();
                if combining {
                    term_picker(ui, s, *uid, &name);
                } else if ui.selectable_label(s.selected_bucket == Some(*uid), name).clicked() {
                    s.selected_bucket = Some(*uid);
                }
            }
        });
        ui.separator();
        ui.vertical(|ui| if combining { combined(ui, s) } else { detail(ui, s) });
    });
}

/// One bucket's row while combining: click to cycle out -> plus -> minus.
fn term_picker(ui: &mut Ui, s: &mut Session, uid: BucketUid, name: &str) {
    let current = s.bucket_combo.iter().find(|t| t.bucket == uid).map(|t| t.sign);
    let (mark, colour) = match current {
        Some(Sign::Plus) => ("+", fmt::good()),
        Some(Sign::Minus) => ("-", fmt::bad()),
        None => ("·", fmt::dim()),
    };
    let label = format!("{mark}  {name}");
    if ui
        .selectable_label(current.is_some(), RichText::new(label).color(colour))
        .on_hover_text("Click to cycle: not included, added, subtracted")
        .clicked()
    {
        s.bucket_combo.retain(|t| t.bucket != uid);
        match current {
            None => s.bucket_combo.push(Term::plus(uid)),
            Some(Sign::Plus) => s.bucket_combo.push(Term::minus(uid)),
            Some(Sign::Minus) => {}
        }
    }
}

/// The combined reading: several buckets added and subtracted together.
fn combined(ui: &mut Ui, s: &mut Session) {
    let c = combine(s.budget(), &s.bucket_combo, s.bucket_roll, s.ledger_sort, Order::Asc);

    ui.heading(formula(s));
    ui.label(
        RichText::new(
            "Buckets cannot contain buckets. This totals them at read time: a ledger in \
             several added buckets counts once, and one on both sides cancels.",
        )
        .color(fmt::dim())
        .small(),
    );
    ui.add_space(4.0);

    ui.horizontal_wrapped(|ui| {
        ui.label("Total by");
        ui.selectable_value(&mut s.bucket_roll, RollUp::ByNormality, "net worth");
        ui.selectable_value(&mut s.bucket_roll, RollUp::Sum, "plain sum");
        ui.separator();
        ui.label("Sort");
        for (sort, label) in [
            (LedgerSort::Name, "name"),
            (LedgerSort::Balance, "balance"),
            (LedgerSort::Normality, "normality"),
            (LedgerSort::Opened, "opened"),
        ] {
            ui.selectable_value(&mut s.ledger_sort, sort, label);
        }
    });
    ui.add_space(8.0);

    ui.label(fmt::money_text(c.total).size(26.0));
    ui.add_space(8.0);

    if c.lines.is_empty() {
        empty(ui, "No ledgers in the chosen buckets.");
    }

    egui::Grid::new("combo_lines").num_columns(4).striped(true).spacing([16.0, 6.0]).show(
        ui,
        |ui| {
            for h in ["ledger", "normal"] {
                ui.label(RichText::new(h).small().color(fmt::dim()));
            }
            num(ui, RichText::new("balance").small().color(fmt::dim()));
            num(ui, RichText::new("contributes").small().color(fmt::dim()));
            ui.end_row();

            for line in &c.lines {
                let ledger_uid = s.budget().ledgers.uid[line.ledger.get()];
                if ui.link(&line.name).clicked() {
                    s.selected_ledger = Some(ledger_uid);
                    s.goto = Some(Screen::Register);
                }
                ui.label(RichText::new(line.normality.to_string()).color(fmt::dim()));
                num(ui, fmt::money_text(line.balance));
                num(ui, fmt::delta_text(line.contribution));
                ui.end_row();
            }
        },
    );

    // Shown rather than dropped: a ledger that quietly disappears from a total
    // looks like an arithmetic bug.
    if !c.cancelled.is_empty() {
        ui.add_space(12.0);
        ui.label(RichText::new("ON BOTH SIDES, CONTRIBUTING NOTHING").small().color(fmt::dim()));
        ui.separator();
        for line in &c.cancelled {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&line.name).color(fmt::dim()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(fmt::money_text(line.balance).color(fmt::dim()));
                });
            });
        }
    }

    for uid in &c.missing {
        ui.colored_label(fmt::bad(), format!("bucket {} is not on this branch", uid.short()));
    }
}

/// The combination written out, e.g. `Cash - Receivables`.
fn formula(s: &Session) -> String {
    let mut out = String::new();
    for (i, term) in s.bucket_combo.iter().enumerate() {
        let name = s
            .budget()
            .buckets
            .ix(term.bucket)
            .map(|bix| s.budget().buckets.name[bix.get()].clone())
            .unwrap_or_else(|| term.bucket.short());
        match (i, term.sign) {
            // A leading plus reads as noise; a leading minus is meaningful, but
            // must not be pushed with the separating space the others need.
            (0, Sign::Plus) => out.push_str(&name),
            (0, Sign::Minus) => out.push_str(&format!("-{name}")),
            (_, Sign::Plus) => out.push_str(&format!(" + {name}")),
            (_, Sign::Minus) => out.push_str(&format!(" - {name}")),
        }
    }
    out
}

/// The bucket's members' targets, totalled under its roll-up.
fn targets(ui: &mut Ui, s: &mut Session, uid: BucketUid) {
    let Some(t) = ledgit_core::goals::bucket_targets(s.budget(), uid, s.bucket_roll) else {
        return;
    };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        if t.lines.is_empty() {
            ui.label(
                RichText::new("No member has a target. Set one on a ledger's page.")
                    .color(fmt::dim()),
            );
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Target {}", fmt::amount(t.target))).strong());
            ui.separator();
            if t.is_reached() {
                ui.colored_label(fmt::good(), "reached");
            } else {
                ui.label(format!(
                    "{} now, {} to go",
                    fmt::amount(t.balance),
                    fmt::amount(t.remaining())
                ));
            }
        });
        if t.lines.len() < t.members {
            ui.label(
                RichText::new(format!(
                    "{} of {} members have a target; the others are left out of both figures.",
                    t.lines.len(),
                    t.members
                ))
                .small()
                .color(fmt::dim()),
            );
        }
        egui::Grid::new("bucket_targets").num_columns(4).spacing([16.0, 4.0]).show(ui, |ui| {
            ui.label("");
            for h in ["balance", "target", "to go"] {
                num(ui, RichText::new(h).small().color(fmt::dim()));
            }
            ui.end_row();
            for line in &t.lines {
                ui.label(&line.name);
                num(ui, fmt::mono(fmt::amount(line.balance)));
                num(ui, fmt::mono(fmt::amount(line.target)));
                num(ui, fmt::mono(fmt::amount(Money((line.target.0 - line.balance.0).abs()))));
                ui.end_row();
            }
        });
    });
}

fn detail(ui: &mut Ui, s: &mut Session) {
    let Some(uid) = s.selected_bucket else { return };
    let Some(roll) = roll_up(s.budget(), uid, s.bucket_roll, s.ledger_sort, Order::Asc) else {
        return;
    };
    let Some(bix) = s.budget().buckets.ix(uid) else { return };
    let description = s.budget().buckets.description[bix.get()].clone();

    ui.heading(&roll.name);
    if !description.is_empty() {
        ui.label(RichText::new(description).color(fmt::dim()));
    }
    ui.add_space(4.0);

    ui.horizontal_wrapped(|ui| {
        ui.label("Total by");
        ui.selectable_value(&mut s.bucket_roll, RollUp::ByNormality, "net worth");
        ui.selectable_value(&mut s.bucket_roll, RollUp::Sum, "plain sum");
        ui.separator();
        ui.label("Sort");
        for (sort, label) in [
            (LedgerSort::Name, "name"),
            (LedgerSort::Balance, "balance"),
            (LedgerSort::Normality, "normality"),
            (LedgerSort::Opened, "opened"),
        ] {
            ui.selectable_value(&mut s.ledger_sort, sort, label);
        }
    });
    ui.label(
        RichText::new(match s.bucket_roll {
            RollUp::ByNormality => {
                "Adds debit-normal members and subtracts credit-normal ones: assets minus liabilities."
            }
            RollUp::Sum => "Adds every member's balance exactly as shown.",
        })
        .color(fmt::dim())
        .small(),
    );
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label(fmt::money_text(roll.total).size(26.0));
        ui.add_space(16.0);
        ui.checkbox(&mut s.bucket_targets, "Aggregate targets").on_hover_text(
            "Add up the targets of the members that have one, the way the balance is added up",
        );
    });
    if s.bucket_targets {
        targets(ui, s, uid);
    }
    ui.add_space(8.0);

    let explicit: Vec<LedgerIx> = s.budget().buckets.explicit[bix.get()].clone();
    let subtrees: Vec<String> = s.budget().buckets.subtrees[bix.get()].clone();
    let mut drop_subtree: Option<String> = None;
    if !subtrees.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Includes everything under").color(fmt::dim()));
            for path in &subtrees {
                if ui
                    .button(format!("{path}  \u{1F5D9}"))
                    .on_hover_text("Stop including this subtree")
                    .clicked()
                {
                    drop_subtree = Some(path.clone());
                }
            }
        });
        ui.label(
            RichText::new("Ledgers created or renamed under these paths join automatically.")
                .small()
                .color(fmt::dim()),
        );
        ui.add_space(6.0);
    }

    let mut remove: Option<LedgerUid> = None;
    egui::Grid::new("bucket_lines").num_columns(5).striped(true).spacing([16.0, 6.0]).show(
        ui,
        |ui| {
            for h in ["ledger", "normal"] {
                ui.label(RichText::new(h).small().color(fmt::dim()));
            }
            num(ui, RichText::new("balance").small().color(fmt::dim()));
            num(ui, RichText::new("contributes").small().color(fmt::dim()));
            ui.label("");
            ui.end_row();

            for line in &roll.lines {
                let ledger_uid = s.budget().ledgers.uid[line.ledger.get()];
                if ui.link(&line.name).clicked() {
                    s.selected_ledger = Some(ledger_uid);
                    s.goto = Some(Screen::Register);
                }
                ui.label(RichText::new(line.normality.to_string()).color(fmt::dim()));
                num(ui, fmt::money_text(line.balance));
                num(ui, fmt::delta_text(line.contribution));
                if explicit.contains(&line.ledger) {
                    if ui.small_button("remove").clicked() {
                        remove = Some(ledger_uid);
                    }
                } else {
                    // In only through a subtree: removing it alone would be
                    // undone by the subtree, so say where it comes from.
                    let via = subtrees
                        .iter()
                        .find(|p| ledgit_core::tree::is_under(&line.name, p))
                        .cloned()
                        .unwrap_or_default();
                    ui.label(RichText::new(format!("via {via}")).small().color(fmt::dim()));
                }
                ui.end_row();
            }
        },
    );

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.label("Add");
        let members: Vec<LedgerUid> =
            roll.lines.iter().map(|l| s.budget().ledgers.uid[l.ledger.get()]).collect();
        let picked = Picker::new(("bucket_add", uid), s.budget())
            .selected_text("a ledger, or everything under a path...")
            .width(280.0)
            .subtrees(true)
            .hide(members)
            .show(ui);
        match picked {
            Some(Pick::Ledger(ledger)) => {
                let ops = vec![Op::AddToBucket { bucket: uid, ledger }];
                s.stage(ops, "adding a ledger to the bucket");
            }
            Some(Pick::Subtree(path)) => {
                let ops = vec![Op::AddSubtreeToBucket { bucket: uid, path }];
                s.stage(ops, "adding a subtree to the bucket");
            }
            None => {}
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Delete bucket")
                .on_hover_text("Buckets are views; no balances change.")
                .clicked()
            {
                let ops = vec![Op::DeleteBucket { uid }];
                if s.stage(ops, "deleting the bucket") {
                    s.selected_bucket = None;
                }
            }
        });
    });

    if let Some(ledger) = remove {
        let ops = vec![Op::RemoveFromBucket { bucket: uid, ledger }];
        s.stage(ops, "removing a ledger from the bucket");
    }
    if let Some(path) = drop_subtree {
        let ops = vec![Op::RemoveSubtreeFromBucket { bucket: uid, path }];
        s.stage(ops, "removing a subtree from the bucket");
    }
}
