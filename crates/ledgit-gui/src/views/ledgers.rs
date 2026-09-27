//! The ledger list and the per-ledger register.

use super::{empty, heading, num};
use crate::app::{Screen, Session};
use crate::fmt;
use crate::forms::FormKind;
use crate::table::{figures, text, Height, Table};
use egui::{RichText, Ui};
use ledgit_core::prelude::*;

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(
        ui,
        "Ledgers",
        "Ledgers can never be deleted. Balances only move through transactions.",
    );

    ui.horizontal(|ui| {
        if ui.button("New ledger").clicked() {
            s.forms.open(FormKind::Ledger, s.repo.working());
        }
        ui.separator();
        ui.selectable_value(&mut s.ledger_tree, true, "Tree")
            .on_hover_text("Grouped by path: Wedding:Tuxedo sits under Wedding");
        ui.selectable_value(&mut s.ledger_tree, false, "List");
        ui.separator();
        if s.ledger_tree {
            if ui.button("Move ledgers...").on_hover_text("Rename a whole subtree").clicked() {
                s.forms.open_move("");
            }
            return;
        }
        ui.label(RichText::new("Sort by").color(fmt::dim()));
        for (sort, label) in [
            (LedgerSort::Name, "name"),
            (LedgerSort::Balance, "balance"),
            (LedgerSort::Normality, "normality"),
            (LedgerSort::Opened, "opened"),
        ] {
            let selected = s.ledger_sort == sort;
            if ui.selectable_label(selected, label).clicked() {
                s.ledger_sort = sort;
            }
        }
    });
    ui.add_space(8.0);

    if s.budget().ledgers.is_empty() {
        empty(ui, "No ledgers yet.");
        return;
    }
    if s.ledger_tree {
        tree(ui, s);
        return;
    }

    let rows = LedgerQuery::new().sort_by(s.ledger_sort, Order::Asc).run(s.budget());
    let mut pin: Option<LedgerUid> = None;
    let mut open: Option<LedgerUid> = None;
    let l = s.budget();
    Table::new(
        "ledgers",
        vec![
            text("").narrow(),
            text("ledger").max(420.0),
            text("normal"),
            text("opened"),
            figures("postings"),
            figures("balance"),
        ],
    )
    .height(Height::Fill)
    .show(ui, rows.len(), |row| {
        let ix = rows[row.index()];
        let i = ix.get();
        let uid = l.ledgers.uid[i];
        row.col(|ui| {
            let pinned = s.is_pinned(uid);
            if ui
                .selectable_label(pinned, if pinned { "\u{2605}" } else { "\u{2606}" })
                .on_hover_text("Pin to the dashboard and the sidebar")
                .clicked()
            {
                pin = Some(uid);
            }
        });
        row.col(|ui| {
            if ui.link(&l.ledgers.name[i]).clicked() {
                open = Some(uid);
            }
        });
        row.col(|ui| {
            ui.label(RichText::new(l.ledgers.normality[i].to_string()).color(fmt::dim()));
        });
        row.col(|ui| {
            ui.label(fmt::mono(l.ledgers.opened[i].to_string()));
        });
        row.col(|ui| {
            num(ui, RichText::new(l.ledgers.postings[i].len().to_string()).color(fmt::dim()));
        });
        row.col(|ui| {
            num(ui, fmt::money_text(l.ledgers.balance(ix)));
        });
    });
    if let Some(uid) = pin {
        s.toggle_pin(uid);
    }
    if let Some(uid) = open {
        s.selected_ledger = Some(uid);
        s.goto = Some(Screen::Register);
    }
}

/// The ledgers as their path tree: a subtotal on every level with children,
/// levels that fold shut, and a "move" on each to rename the whole subtree.
fn tree(ui: &mut Ui, s: &mut Session) {
    let t = LedgerTree::build(s.budget());
    let mut toggle: Option<String> = None;
    let mut open: Option<LedgerUid> = None;
    let mut pin: Option<LedgerUid> = None;
    let mut move_from: Option<String> = None;
    let mut add_under: Option<String> = None;

    // The rows on show: every node, except those inside a folded level.
    let mut shown = Vec::with_capacity(t.nodes.len());
    let mut n = 0;
    while n < t.nodes.len() {
        shown.push(n);
        let node = &t.nodes[n];
        let folded = node.end as usize > n + 1 && s.collapsed.contains(&node.path.to_lowercase());
        n = if folded { node.end as usize } else { n + 1 };
    }

    let l = s.budget();
    Table::new(
        "ledger_tree",
        vec![
            text("").narrow(),
            text("ledger").max(560.0),
            text("normal"),
            figures("postings"),
            figures("balance"),
            figures("subtree total"),
        ],
    )
    .height(Height::Fill)
    .fit_to(s.collapsed.len())
    .show(ui, shown.len(), |row| {
        let n = shown[row.index()];
        let node = &t.nodes[n];
        let parent = node.end as usize > n + 1;
        let key = node.path.to_lowercase();
        let folded = parent && s.collapsed.contains(&key);

        row.col(|ui| {
            if let Some(ix) = node.ledger {
                let uid = l.ledgers.uid[ix.get()];
                let pinned = s.pins.contains(&uid);
                if ui
                    .selectable_label(pinned, if pinned { "\u{2605}" } else { "\u{2606}" })
                    .on_hover_text("Pin to the dashboard and the sidebar")
                    .clicked()
                {
                    pin = Some(uid);
                }
            }
        });

        row.col(|ui| {
            ui.add_space(16.0 * node.depth as f32);
            if parent {
                let arrow = if folded { "\u{23F5}" } else { "\u{23F7}" };
                if ui.small_button(arrow).clicked() {
                    toggle = Some(key.clone());
                }
            } else {
                ui.add_space(18.0);
            }
            match node.ledger {
                Some(ix) => {
                    if ui.link(node.name()).on_hover_text(&node.path).clicked() {
                        open = Some(l.ledgers.uid[ix.get()]);
                    }
                }
                None => {
                    ui.label(RichText::new(node.name()).strong());
                }
            }
            if ui
                .small_button("+")
                .on_hover_text(format!("New ledger under {}", node.path))
                .clicked()
            {
                add_under = Some(node.path.clone());
            }
            if parent
                && ui
                    .small_button("move")
                    .on_hover_text(format!("Rename everything under {}", node.path))
                    .clicked()
            {
                move_from = Some(node.path.clone());
            }
        });

        match node.ledger {
            Some(ix) => {
                let i = ix.get();
                row.col(|ui| {
                    ui.label(RichText::new(l.ledgers.normality[i].to_string()).color(fmt::dim()));
                });
                row.col(|ui| {
                    num(ui, RichText::new(l.ledgers.postings[i].len().to_string()).color(fmt::dim()));
                });
                row.col(|ui| {
                    num(ui, fmt::money_text(l.ledgers.balance(ix)));
                });
            }
            None => {
                for _ in 0..3 {
                    row.col(|_| {});
                }
            }
        }
        row.col(|ui| {
            if !parent {
                return;
            }
            let (total, kind) = t.total(l, n);
            let hover = match kind {
                Some(k) => format!("{} ledger(s), all {k}-normal", t.subtree(n).len()),
                None => format!(
                    "{} ledger(s) of mixed normality: the net, debit-positive, as a bucket reads it",
                    t.subtree(n).len()
                ),
            };
            let text = fmt::money_text(total).strong();
            let text = match kind {
                Some(_) => text,
                None => RichText::new(format!("net {}", fmt::amount(total)))
                    .monospace()
                    .strong()
                    .color(fmt::money_colour(total)),
            };
            num(ui, text).on_hover_text(hover);
        });
    });

    if let Some(key) = toggle {
        if !s.collapsed.remove(&key) {
            s.collapsed.insert(key);
        }
    }
    if let Some(uid) = pin {
        s.toggle_pin(uid);
    }
    if let Some(uid) = open {
        s.selected_ledger = Some(uid);
        s.goto = Some(Screen::Register);
    }
    if let Some(from) = move_from {
        s.forms.open_move(&from);
    }
    if let Some(parent) = add_under {
        s.forms.open_ledger_under(&parent, s.repo.working());
    }
}

pub fn register(ui: &mut Ui, s: &mut Session) {
    let Some(uid) = s.selected_ledger else {
        heading(ui, "Register", "");
        empty(ui, "Pick a ledger from the Ledgers screen.");
        return;
    };
    let Some(ix) = s.budget().ledgers.ix(uid) else {
        s.selected_ledger = None;
        empty(ui, "That ledger is not on this branch.");
        return;
    };

    let l = s.budget();
    let i = ix.get();
    let name = l.ledgers.name[i].clone();
    let normality = l.ledgers.normality[i];
    let description = l.ledgers.description[i].clone();
    let balance = l.ledgers.balance(ix);

    heading(ui, &name, &description);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{normality}-normal")).color(fmt::dim()));
        ui.separator();
        ui.label(fmt::money_text(balance).size(20.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("New transaction").clicked() {
                s.forms.open(FormKind::Transaction, s.repo.working());
            }
            let pinned = s.is_pinned(uid);
            if ui.button(if pinned { "Unpin" } else { "Pin" }).clicked() {
                s.toggle_pin(uid);
            }
        });
    });
    ui.add_space(4.0);
    super::goals::ledger_section(ui, s, uid);
    ui.add_space(8.0);

    let rows = ledgit_core::query::register(s.budget(), ix);
    if rows.is_empty() {
        empty(ui, "No postings yet.");
        return;
    }

    let l = s.budget();
    Table::new(
        ("register", uid),
        vec![
            text("date"),
            text("description").max(380.0),
            text("other side").max(320.0),
            figures("change"),
            figures("balance"),
        ],
    )
    .height(Height::Fill)
    .show(ui, rows.len(), |row| {
        // Newest first: the last thing that happened is the thing you are
        // usually looking for.
        let line = &rows[rows.len() - 1 - row.index()];
        let t = line.transaction.get();
        // This ledger's share of the entry, which for a split is not the
        // size of the entry.
        let shown = normality.present(line.change);
        let others: Vec<String> = l
            .counterparties(line.transaction, ix)
            .iter()
            .map(|a| l.ledgers.name[a.get()].clone())
            .collect();
        let source = match l.transactions.parent[t] {
            Parent::Manual => String::new(),
            Parent::Issuer(u) => l
                .issuers
                .ix(u)
                .map(|j| format!("  (issuer: {})", l.issuers.name[j.get()]))
                .unwrap_or_default(),
        };
        row.col(|ui| {
            ui.label(fmt::mono(l.transactions.date[t].to_string()));
        });
        row.col(|ui| {
            ui.label(format!("{}{source}", l.transactions.name[t]));
        });
        row.col(|ui| {
            let split = l.transactions.is_split(line.transaction);
            let others_text = RichText::new(others.join(", ")).color(fmt::dim());
            let r = ui.label(if split { others_text.italics() } else { others_text });
            if split {
                r.on_hover_text(format!(
                    "split entry of {}",
                    fmt::amount(l.amount_of(line.transaction))
                ));
            }
        });
        row.col(|ui| {
            num(ui, fmt::delta_text(shown));
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(line.balance)));
        });
    });
}
