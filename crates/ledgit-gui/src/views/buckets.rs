//! Buckets: named groups of ledgers, and the totals you read off them.

use super::{empty, heading, num};
use crate::app::{Screen, Session};
use crate::fmt;
use crate::forms::FormKind;
use crate::picker::{Pick, Picker};
use crate::table::{figures, text, Height, Table};
use egui::{RichText, Ui};
use ledgit_core::id::LedgerIx;
use ledgit_core::prelude::*;

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(
        ui,
        "Buckets",
        "A bucket is a view over ledgers. Creating or deleting one moves no money.",
    );

    if ui
        .button("New bucket")
        .on_hover_text("To total several buckets together, add them to a view.")
        .clicked()
    {
        s.forms.open(FormKind::Bucket, s.repo.working());
    }
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

    super::split(
        ui,
        "buckets",
        200.0,
        s,
        |ui, s| {
            for uid in &live {
                let Some(ix) = s.budget().buckets.ix(*uid) else { continue };
                let name = s.budget().buckets.name[ix.get()].clone();
                if ui.selectable_label(s.selected_bucket == Some(*uid), name).clicked() {
                    s.selected_bucket = Some(*uid);
                }
            }
        },
        detail,
    );
}

/// The bucket's members' targets, totalled under its roll-up.
fn targets(ui: &mut Ui, s: &mut Session, uid: BucketUid) {
    let Some(t) = ledgit_core::goals::bucket_targets(s.budget(), uid, s.bucket_roll) else {
        return;
    };
    let l = s.budget();
    let has_paces = l.buckets.ix(uid).is_some_and(|bix| {
        l.buckets.members[bix.get()]
            .iter()
            .any(|ix| matches!(l.ledgers.target[ix.get()], Some(Target::Pace { .. })))
    });
    if has_paces {
        paces(ui, s, uid);
        if t.lines.is_empty() {
            return;
        }
        ui.add_space(6.0);
    }
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
        Table::new(
            ("bucket_targets", uid),
            vec![text("").max(300.0), figures("balance"), figures("target"), figures("to go")],
        )
        .height(Height::Max(180.0))
        .show(ui, t.lines.len(), |row| {
            let line = &t.lines[row.index()];
            row.col(|ui| {
                ui.label(&line.name);
            });
            row.col(|ui| {
                num(ui, fmt::mono(fmt::amount(line.balance)));
            });
            row.col(|ui| {
                num(ui, fmt::mono(fmt::amount(line.target)));
            });
            row.col(|ui| {
                num(ui, fmt::mono(fmt::amount(Money((line.target.0 - line.balance.0).abs()))));
            });
        });
    });
}

/// The bucket's members' paces, in one unit, against this period.
fn paces(ui: &mut Ui, s: &mut Session, uid: BucketUid) {
    let today = Date::today_utc();
    let per = s.bucket_pace_per;
    let Some(p) = ledgit_core::goals::bucket_paces(s.budget(), uid, s.bucket_roll, per, today)
    else {
        return;
    };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Paces").strong());
            for q in Period::ALL {
                ui.selectable_value(&mut s.bucket_pace_per, q, format!("per {q}"));
            }
            ui.separator();
            let bound = p.bound.map(|b| format!(" {b}")).unwrap_or_default();
            ui.label(format!(
                "{} so far of {} a {per}{bound}",
                fmt::amount(p.flow),
                fmt::amount(p.amount)
            ));
            match p.bound {
                Some(Bound::AtMost) if p.flow > p.amount => {
                    ui.colored_label(
                        fmt::bad(),
                        format!("over by {}", fmt::amount(p.flow - p.amount)),
                    );
                }
                Some(Bound::AtLeast) if p.flow >= p.amount => {
                    ui.colored_label(fmt::good(), "met");
                }
                _ => {}
            }
        });
        let mut notes = vec![format!(
            "The calendar {per} from {}. Paces set in other units are converted by average \
             lengths, so a weekly budget reads as about 4.35 weeks' worth a month.",
            p.start
        )];
        if p.bound.is_none() {
            notes
                .push("Members mix at-most and at-least paces, so the total is not judged.".into());
        }
        if p.lines.len() < p.members {
            notes.push(format!(
                "{} of {} members have a pace; the others are left out.",
                p.lines.len(),
                p.members
            ));
        }
        ui.label(RichText::new(notes.join(" ")).small().color(fmt::dim()));
        Table::new(
            ("bucket_paces", uid),
            vec![
                text("").max(300.0),
                text("pace").max(220.0),
                figures(format!("a {per}")),
                figures("so far"),
                figures("vs pace"),
            ],
        )
        .height(Height::Max(180.0))
        .show(ui, p.lines.len(), |row| {
            let line = &p.lines[row.index()];
            row.col(|ui| {
                ui.label(&line.name);
            });
            row.col(|ui| {
                let (amount, own) = line.own;
                let own = Target::Pace { amount, per: own, bound: line.bound };
                ui.label(RichText::new(super::goals::describe(own)).color(fmt::dim()));
            });
            row.col(|ui| {
                num(ui, fmt::mono(fmt::amount(line.amount)));
            });
            row.col(|ui| {
                num(ui, fmt::mono(fmt::amount(line.flow)));
            });
            row.col(|ui| {
                let off = !line.bound.keeps(line.amount, line.flow) && line.bound == Bound::AtMost;
                let text = fmt::mono(fmt::signed(line.flow - line.amount));
                num(ui, if off { text.color(fmt::bad()) } else { text });
            });
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

    // Adding and deleting sit above the members, so a long bucket cannot
    // push them off the bottom of the screen.
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
        ui.separator();
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
    ui.add_space(8.0);

    let mut remove: Option<LedgerUid> = None;
    let mut open: Option<LedgerUid> = None;
    let l = s.budget();
    Table::new(
        ("bucket_lines", uid),
        vec![
            text("ledger").max(420.0),
            text("normal"),
            figures("balance"),
            figures("contributes"),
            text(""),
        ],
    )
    .height(Height::Fill)
    .fit_to((s.bucket_roll == RollUp::Sum, s.ledger_sort as u8))
    .show(ui, roll.lines.len(), |row| {
        let line = &roll.lines[row.index()];
        let ledger_uid = l.ledgers.uid[line.ledger.get()];
        row.col(|ui| {
            if ui.link(&line.name).clicked() {
                open = Some(ledger_uid);
            }
        });
        row.col(|ui| {
            ui.label(RichText::new(line.normality.to_string()).color(fmt::dim()));
        });
        row.col(|ui| {
            num(ui, fmt::money_text(line.balance));
        });
        row.col(|ui| {
            num(ui, fmt::delta_text(line.contribution));
        });
        row.col(|ui| {
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
        });
    });
    if let Some(uid) = open {
        s.selected_ledger = Some(uid);
        s.goto = Some(Screen::Register);
    }

    if let Some(ledger) = remove {
        let ops = vec![Op::RemoveFromBucket { bucket: uid, ledger }];
        s.stage(ops, "removing a ledger from the bucket");
    }
    if let Some(path) = drop_subtree {
        let ops = vec![Op::RemoveSubtreeFromBucket { bucket: uid, path }];
        s.stage(ops, "removing a subtree from the bucket");
    }
}
