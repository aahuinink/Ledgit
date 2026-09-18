//! Formatting shared by every view. Numbers on a budget screen have to line up
//! and read the same way everywhere, so the rules live in one place.

use egui::{Color32, RichText};
use ledgit_core::prelude::*;

/// `1,234.56` - grouped, two decimals, minus sign in front.
pub fn amount(m: Money) -> String {
    let neg = m.cents() < 0;
    let abs = m.cents().unsigned_abs();
    let (whole, cents) = (abs / 100, abs % 100);
    let digits = whole.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{}{grouped}.{cents:02}", if neg { "-" } else { "" })
}

/// The same, but with an explicit sign, for deltas where direction is the point.
pub fn signed(m: Money) -> String {
    if m.cents() > 0 {
        format!("+{}", amount(m))
    } else {
        amount(m)
    }
}

pub fn good() -> Color32 {
    Color32::from_rgb(46, 160, 67)
}

pub fn bad() -> Color32 {
    Color32::from_rgb(220, 80, 70)
}

pub fn dim() -> Color32 {
    Color32::from_gray(140)
}

/// Money coloured by direction. Red for down, green for up, plain for zero -
/// and never colour alone: the sign is always there too, for the ~8% of men
/// who would otherwise see two identical columns.
pub fn money_text(m: Money) -> RichText {
    let t = RichText::new(amount(m)).monospace();
    match m.cents() {
        c if c < 0 => t.color(bad()),
        c if c > 0 => t.color(good()),
        _ => t.color(dim()),
    }
}

pub fn delta_text(m: Money) -> RichText {
    let t = RichText::new(signed(m)).monospace();
    match m.cents() {
        c if c < 0 => t.color(bad()),
        c if c > 0 => t.color(good()),
        _ => t.color(dim()),
    }
}

pub fn mono(s: impl Into<String>) -> RichText {
    RichText::new(s.into()).monospace()
}

/// The two halves of an entry as text: what it debited, what it credited.
pub fn sides(l: &Budget, tix: ledgit_core::id::TxIx) -> (String, String) {
    let mut debits = Vec::new();
    let mut credits = Vec::new();
    for p in l.transactions.leg_range(tix) {
        let name = l.ledgers.name[l.postings.ledger[p].get()].clone();
        if l.postings.amount[p].cents() > 0 {
            debits.push(name);
        } else {
            credits.push(name);
        }
    }
    (debits.join(", "), credits.join(", "))
}

/// An entry read as a sentence: "what gave value -> what received it".
pub fn flow(l: &Budget, tix: ledgit_core::id::TxIx) -> String {
    let (dr, cr) = sides(l, tix);
    format!("{cr} \u{2192} {dr}")
}

/// The same for an issuer, which holds its legs as uids.
pub fn issuer_flow(l: &Budget, ix: ledgit_core::id::IssuerIx) -> String {
    let name = |uid: LedgerUid| match l.ledgers.ix(uid) {
        Some(a) => l.ledgers.name[a.get()].clone(),
        None => uid.short(),
    };
    let legs = &l.issuers.legs[ix.get()];
    let dr: Vec<String> = legs.iter().filter(|l| l.is_debit()).map(|l| name(l.ledger)).collect();
    let cr: Vec<String> = legs.iter().filter(|l| !l.is_debit()).map(|l| name(l.ledger)).collect();
    format!("{} \u{2192} {}", cr.join(", "), dr.join(", "))
}

/// Clip a cell so one long value - a split naming four ledgers - cannot
/// stretch a table column past the window.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}\u{2026}")
    }
}

/// Ledger label as it appears in pickers: name plus a short uid, because two
/// ledgers are allowed to share a name and picking the wrong one is expensive.
pub fn ledger_label(l: &Budget, uid: LedgerUid) -> String {
    match l.ledgers.ix(uid) {
        Some(ix) => format!("{}  ({})", l.ledgers.name[ix.get()], uid.short()),
        None => format!("(unknown ledger {})", uid.short()),
    }
}
