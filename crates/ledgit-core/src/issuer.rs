//! Recurring transactions.
//!
//! An issuer never mutates the budget directly. It *proposes* ops, which land
//! in the staging area exactly like hand-entered ones and appear in the
//! pre-commit report. That is deliberate: a budget where the software silently
//! posts money while you are not looking is a budget you stop trusting.
//!
//! Each batch ends with an `AdvanceIssuer` op recording the date emitted
//! through, so replaying history - or rebasing it - never double-posts rent.

use crate::date::Date;
use crate::id::{IssuerIx, TxUid};
use crate::model::Parent;
use crate::op::Op;
use crate::state::Budget;

/// Safety valve: an issuer that somehow produced a zero-length step would
/// otherwise emit forever. Two centuries of daily payments is plenty.
const MAX_OCCURRENCES: usize = 75_000;

/// The first date this issuer owes a transaction for, or `None` if it is
/// exhausted (a `Once` schedule that already fired).
pub fn next_due(l: &Budget, ix: IssuerIx) -> Option<Date> {
    let s = &l.issuers;
    let i = ix.get();
    let anchor = s.schedule[i].first_on_or_after(s.start[i]);
    match s.emitted_through[i] {
        None => Some(anchor),
        Some(done) => s.schedule[i].next_after(anchor, done),
    }
}

/// Every date this issuer owes on or before `through`, oldest first.
pub fn due_dates(l: &Budget, ix: IssuerIx, through: Date) -> Vec<Date> {
    let s = &l.issuers;
    let i = ix.get();
    let anchor = s.schedule[i].first_on_or_after(s.start[i]);
    let mut out = Vec::new();
    let mut cursor = match next_due(l, ix) {
        Some(d) => d,
        None => return out,
    };
    while cursor <= through && out.len() < MAX_OCCURRENCES {
        out.push(cursor);
        match s.schedule[i].next_after(anchor, cursor) {
            Some(next) if next > cursor => cursor = next,
            _ => break,
        }
    }
    out
}

/// One issuer's worth of pending work.
#[derive(Clone, Debug)]
pub struct IssuerRun {
    pub issuer: IssuerIx,
    pub dates: Vec<Date>,
    pub ops: Vec<Op>,
}

/// Generate the ops for every issuer that is due on or before `through`.
///
/// Paused issuers produce nothing, and stay exactly where they were - so
/// resuming one does not retroactively post the payments it missed. If you
/// want those, unpause and then run `through` an earlier date first.
pub fn run_all(l: &Budget, through: Date) -> Vec<IssuerRun> {
    l.issuers
        .indices()
        .filter(|ix| !l.issuers.paused[ix.get()])
        .filter_map(|ix| {
            let dates = due_dates(l, ix, through);
            if dates.is_empty() {
                return None;
            }
            let i = ix.get();
            let s = &l.issuers;
            let mut ops: Vec<Op> = dates
                .iter()
                .map(|d| Op::PostTransaction {
                    uid: TxUid::new(),
                    name: s.name[i].clone(),
                    description: s.description[i].clone(),
                    date: *d,
                    // The whole entry, splits and all, is copied from the
                    // issuer: a recurring paycheque posts its tax and pension
                    // legs every time, not just its net.
                    legs: s.legs[i].clone(),
                    parent: Parent::Issuer(s.uid[i]),
                })
                .collect();
            ops.push(Op::AdvanceIssuer {
                uid: s.uid[i],
                through: *dates.last().expect("non-empty"),
            });
            Some(IssuerRun { issuer: ix, dates, ops })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{IssuerUid, LedgerUid};
    use crate::model::{simple_legs, Normality, Schedule};
    use crate::money::Money;

    fn d(y: i32, m: u32, day: u32) -> Date {
        Date::from_ymd(y, m, day).unwrap()
    }

    fn fixture(schedule: Schedule, start: Date) -> (Budget, IssuerIx) {
        let (cash, loan) = (LedgerUid::new(), LedgerUid::new());
        let mk = |uid, name: &str, n| Op::CreateLedger {
            uid,
            name: name.into(),
            description: String::new(),
            normality: n,
            opened: d(2024, 1, 1),
        };
        let l = Budget::replay(&[
            mk(cash, "Cash", Normality::Debit),
            mk(loan, "Car Loan", Normality::Credit),
            Op::CreateIssuer {
                uid: IssuerUid::new(),
                name: "Car payment".into(),
                description: String::new(),
                legs: simple_legs(loan, cash, Money::from_major(400)),
                schedule,
                start,
            },
        ])
        .unwrap();
        (l, IssuerIx(0))
    }

    #[test]
    fn biweekly_emits_every_fourteen_days() {
        let (l, ix) = fixture(Schedule::EveryNDays { n: 14 }, d(2024, 1, 5));
        let dates = due_dates(&l, ix, d(2024, 2, 5));
        assert_eq!(dates, vec![d(2024, 1, 5), d(2024, 1, 19), d(2024, 2, 2)]);
    }

    #[test]
    fn running_twice_does_not_double_post() {
        let (mut l, _) = fixture(Schedule::EveryNDays { n: 14 }, d(2024, 1, 5));
        let runs = run_all(&l, d(2024, 2, 5));
        assert_eq!(runs.len(), 1);
        for op in &runs[0].ops {
            l.apply(op).unwrap();
        }
        assert_eq!(l.transactions.len(), 3);
        // Second run over the same window: nothing left to do.
        assert!(run_all(&l, d(2024, 2, 5)).is_empty());
        // A later window picks up only the new occurrence.
        let more = run_all(&l, d(2024, 2, 16));
        assert_eq!(more[0].dates, vec![d(2024, 2, 16)]);
    }

    #[test]
    fn paused_issuers_emit_nothing() {
        let (mut l, _) = fixture(Schedule::EveryNDays { n: 14 }, d(2024, 1, 5));
        let uid = l.issuers.uid[0];
        l.apply(&Op::SetIssuerPaused { uid, paused: true }).unwrap();
        assert!(run_all(&l, d(2025, 1, 1)).is_empty());
    }

    #[test]
    fn once_fires_exactly_once() {
        let (mut l, ix) = fixture(Schedule::Once, d(2024, 3, 1));
        let runs = run_all(&l, d(2030, 1, 1));
        assert_eq!(runs[0].dates, vec![d(2024, 3, 1)]);
        for op in &runs[0].ops {
            l.apply(op).unwrap();
        }
        assert_eq!(next_due(&l, ix), None);
        assert!(run_all(&l, d(2030, 1, 1)).is_empty());
    }

    #[test]
    fn issuer_output_is_attributed_to_its_parent() {
        let (l, _) = fixture(Schedule::EveryNDays { n: 30 }, d(2024, 1, 1));
        let runs = run_all(&l, d(2024, 1, 1));
        let uid = l.issuers.uid[0];
        assert!(
            matches!(runs[0].ops[0], Op::PostTransaction { parent: Parent::Issuer(p), .. } if p == uid)
        );
    }
}
