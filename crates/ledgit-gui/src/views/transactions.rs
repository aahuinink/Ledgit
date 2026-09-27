//! Every transaction, with the filters that matter most.
//!
//! The filter controls build a `TxQuery` - the same value the core would
//! execute from a saved view or a dashboard tile. Nothing here is a closure,
//! which is why the same filter could later be persisted or pushed into an
//! index without touching this file.

use super::{empty, heading, num};
use crate::app::{Screen, Session};
use crate::datepick::DateField;
use crate::fmt;
use crate::forms::FormKind;
use crate::table::{figures, text, Height, Table};
use egui::{ComboBox, RichText, Ui};
use ledgit_core::prelude::*;

/// The first ledger on one side of an entry - what a click on that cell
/// navigates to when the entry is a split.
fn first_ledger(l: &Budget, tix: ledgit_core::id::TxIx, debit: bool) -> Option<LedgerUid> {
    l.transactions
        .leg_range(tix)
        .find(|p| (l.postings.amount[*p].cents() > 0) == debit)
        .map(|p| l.ledgers.uid[l.postings.ledger[p].get()])
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceFilter {
    #[default]
    Any,
    Manual,
    Issuers,
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(ui, "Transactions", "Posted entries, including anything staged.");

    ui.horizontal_wrapped(|ui| {
        if ui.button("New transaction").clicked() {
            s.forms.open(FormKind::Transaction, s.repo.working());
        }
        ui.separator();

        ui.label("From");
        DateField::new("tx_from", &mut s.tx_from).optional("any").show(ui);
        ui.label("to");
        DateField::new("tx_to", &mut s.tx_to).optional("any").show(ui);

        ui.separator();
        ui.label("Ledger");
        let label = match s.tx_ledger {
            Some(u) => fmt::ledger_label(s.budget(), u),
            None => "any".to_string(),
        };
        ComboBox::from_id_salt("tx_ledger_filter").selected_text(label).width(200.0).show_ui(
            ui,
            |ui| {
                ui.selectable_value(&mut s.tx_ledger, None, "any");
                let uids: Vec<LedgerUid> = s.budget().ledgers.uid.clone();
                for uid in uids {
                    let name = fmt::ledger_label(s.budget(), uid);
                    ui.selectable_value(&mut s.tx_ledger, Some(uid), name);
                }
            },
        );

        ui.separator();
        for (choice, label) in [
            (SourceFilter::Any, "all"),
            (SourceFilter::Manual, "manual"),
            (SourceFilter::Issuers, "from issuers"),
        ] {
            ui.selectable_value(&mut s.tx_source, choice, label);
        }
    });
    ui.add_space(8.0);

    let mut q = TxQuery::new().sort_by(TxSort::Date, Order::Desc);
    let mut complaints: Vec<&str> = Vec::new();
    if !s.search.trim().is_empty() {
        q = q.filter(TxFilter::Text(s.search.trim().to_string()));
    }
    if !s.tx_from.trim().is_empty() {
        match s.tx_from.parse::<Date>() {
            Ok(d) => q = q.filter(TxFilter::OnOrAfter(d)),
            Err(_) => complaints.push("\"From\" is not a YYYY-MM-DD date"),
        }
    }
    if !s.tx_to.trim().is_empty() {
        match s.tx_to.parse::<Date>() {
            Ok(d) => q = q.filter(TxFilter::OnOrBefore(d)),
            Err(_) => complaints.push("\"To\" is not a YYYY-MM-DD date"),
        }
    }
    if let Some(a) = s.tx_ledger {
        q = q.filter(TxFilter::Touches(a));
    }
    match s.tx_source {
        SourceFilter::Any => {}
        SourceFilter::Manual => q = q.filter(TxFilter::FromIssuer(None)),
        SourceFilter::Issuers => {
            // "Anything an issuer made" is not one filter, it is the complement
            // of the manual one, so it is applied after the query runs.
        }
    }

    for c in &complaints {
        ui.colored_label(fmt::bad(), *c);
    }

    let l = s.budget();
    let mut rows = q.run(l);
    if s.tx_source == SourceFilter::Issuers {
        rows.retain(|ix| l.transactions.parent[ix.get()] != Parent::Manual);
    }

    if rows.is_empty() {
        empty(ui, "No transactions match.");
        return;
    }

    let total: Money = rows.iter().map(|ix| l.amount_of(*ix)).sum();
    ui.label(
        RichText::new(format!("{} entries, {} moved in total", rows.len(), fmt::amount(total)))
            .color(fmt::dim()),
    );
    ui.add_space(6.0);

    let mut open: Option<Option<LedgerUid>> = None;
    Table::new(
        "transactions",
        vec![
            text("date"),
            text("name").max(360.0),
            text("debit").max(300.0),
            text("credit").max(300.0),
            figures("amount"),
        ],
    )
    .height(Height::Fill)
    .show(ui, rows.len(), |row| {
        let ix = rows[row.index()];
        let i = ix.get();
        let issued = l.transactions.parent[i] != Parent::Manual;
        let split = l.transactions.is_split(ix);
        row.col(|ui| {
            ui.label(fmt::mono(l.transactions.date[i].to_string()));
        });
        row.col(|ui| {
            let name = RichText::new(&l.transactions.name[i]);
            ui.label(if issued { name.italics() } else { name }).on_hover_text(if issued {
                "posted by an issuer"
            } else {
                "entered by hand"
            });
        });
        // One side per column normally; a split names every ledger it
        // touches rather than hiding them behind a label, and only the
        // first is clickable.
        let (dr, cr) = fmt::sides(l, ix);
        row.col(|ui| {
            let link = ui.link(&dr);
            let link = if split { link.on_hover_text("split entry") } else { link };
            if link.clicked() {
                open = Some(first_ledger(l, ix, true));
            }
        });
        row.col(|ui| {
            if ui.link(&cr).clicked() {
                open = Some(first_ledger(l, ix, false));
            }
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(l.amount_of(ix))));
        });
    });
    if let Some(uid) = open {
        s.selected_ledger = uid;
        s.goto = Some(Screen::Register);
    }
}
