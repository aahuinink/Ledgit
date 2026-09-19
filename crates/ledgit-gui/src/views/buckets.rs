//! Buckets: named groups of ledgers, and the totals you read off them.

use super::{empty, heading, num};
use crate::app::{Session, View};
use crate::fmt;
use crate::forms::FormKind;
use egui::{RichText, Ui};
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
                    s.goto = Some(View::Register);
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

    ui.label(fmt::money_text(roll.total).size(26.0));
    ui.add_space(8.0);

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
                    s.goto = Some(View::Register);
                }
                ui.label(RichText::new(line.normality.to_string()).color(fmt::dim()));
                num(ui, fmt::money_text(line.balance));
                num(ui, fmt::delta_text(line.contribution));
                if ui.small_button("remove").clicked() {
                    remove = Some(ledger_uid);
                }
                ui.end_row();
            }
        },
    );

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.label("Add a ledger");
        let members: Vec<LedgerUid> =
            roll.lines.iter().map(|l| s.budget().ledgers.uid[l.ledger.get()]).collect();
        let candidates: Vec<LedgerUid> =
            s.budget().ledgers.uid.iter().copied().filter(|a| !members.contains(a)).collect();
        let mut chosen: Option<LedgerUid> = None;
        egui::ComboBox::from_id_salt("bucket_add")
            .selected_text(if candidates.is_empty() { "every ledger is in" } else { "choose..." })
            .width(240.0)
            .show_ui(ui, |ui| {
                for uid in &candidates {
                    if ui.selectable_label(false, fmt::ledger_label(s.budget(), *uid)).clicked() {
                        chosen = Some(*uid);
                    }
                }
            });
        if let Some(ledger) = chosen {
            let ops = vec![Op::AddToBucket { bucket: uid, ledger }];
            s.stage(ops, "adding a ledger to the bucket");
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
}
