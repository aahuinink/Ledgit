//! Targets and alerts: where you want a ledger's balance to get to, and the
//! levels you want to hear about when it crosses them.
//!
//! Both are settings on a ledger ([`Op::SetLedgerGoals`](crate::op::Op)),
//! versioned like its name, and both are about the balance *as displayed* -
//! a loan's target of 0 means "paid off", whatever its normality.
//!
//! A target has no direction of its own. It is reached going whichever way
//! the balance has to travel from where it stands: savings at 4,000 with a
//! target of 10,000 reach it going up; a loan at 18,000 with a target of 0
//! reaches it going down.

use crate::date::Date;
use crate::id::{BucketUid, LedgerIx};
use crate::model::Alert;
use crate::money::Money;
use crate::query::RollUp;
use crate::state::Budget;

/// An alert whose level a balance is past.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FiredAlert {
    pub ledger: LedgerIx,
    pub alert: Alert,
    pub balance: Money,
}

/// Every alert past its level right now, in ledger order.
pub fn fired(l: &Budget) -> Vec<FiredAlert> {
    let a = &l.ledgers;
    let mut out = Vec::new();
    for ix in a.indices() {
        let balance = a.balance(ix);
        for alert in &a.alerts[ix.get()] {
            if alert.fires(balance) {
                out.push(FiredAlert { ledger: ix, alert: alert.clone(), balance });
            }
        }
    }
    out
}

/// Alerts that fire in `after` but did not in `before` - what committing
/// the staged changes would set off. Matched by ledger uid, so ledgers new
/// in `after` count as not firing before.
pub fn newly_fired(before: &Budget, after: &Budget) -> Vec<FiredAlert> {
    fired(after)
        .into_iter()
        .filter(|f| {
            let uid = after.ledgers.uid[f.ledger.get()];
            match before.ledgers.ix(uid) {
                Some(b) => !f.alert.fires(before.ledgers.balance(b)),
                None => true,
            }
        })
        .collect()
}

/// Whether `value` has reached `target`, travelling from `from`.
pub fn is_reached(from: Money, target: Money, value: Money) -> bool {
    if from <= target {
        value >= target
    } else {
        value <= target
    }
}

/// The first date on or after `from` at which a step series reaches
/// `target`, going the way it must from its value on `from`. `Some(from)`
/// when it is already there; `None` when it never gets there in the series.
///
/// `points` is a balance line as the views draw it: the value at the end of
/// each date it changed, oldest first.
pub fn reached_on(points: &[(Date, Money)], from: Date, target: Money) -> Option<Date> {
    let start = value_at(points, from);
    if start == target {
        return Some(from);
    }
    points
        .iter()
        .filter(|(d, _)| *d >= from)
        .find(|(_, v)| is_reached(start, target, *v))
        .map(|(d, _)| *d)
}

/// For each alert not already firing on `from`, the first date after it on
/// which the series sets it off.
pub fn alerts_ahead<'a>(
    points: &[(Date, Money)],
    from: Date,
    alerts: &'a [Alert],
) -> Vec<(&'a Alert, Date)> {
    let now = value_at(points, from);
    alerts
        .iter()
        .filter(|a| !a.fires(now))
        .filter_map(|a| {
            points
                .iter()
                .filter(|(d, _)| *d > from)
                .find(|(_, v)| a.fires(*v))
                .map(|(d, _)| (a, *d))
        })
        .collect()
}

fn value_at(points: &[(Date, Money)], date: Date) -> Money {
    match points.partition_point(|(d, _)| *d <= date) {
        0 => points.first().map(|(_, v)| *v).unwrap_or_default(),
        i => points[i - 1].1,
    }
}

/// A series' target, when every ledger it adds up has one: the same
/// weighted sum as its balance, over targets instead. `weights` apply to raw
/// (debit-positive) amounts, as in a saved view.
pub fn weighted_target(l: &Budget, weights: &[(LedgerIx, i64)]) -> Option<Money> {
    if weights.is_empty() {
        return None;
    }
    weights
        .iter()
        .map(|(ix, w)| {
            let t = l.ledgers.target[ix.get()]?;
            // Displayed to raw: normality's sign is its own inverse.
            Some(Money(w * l.ledgers.normality[ix.get()].present(t).0))
        })
        .sum()
}

/// One member's part in a bucket's combined target.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TargetLine {
    pub ledger: LedgerIx,
    pub name: String,
    /// As displayed for the ledger.
    pub balance: Money,
    pub target: Money,
    /// What each adds to the bucket's totals under its roll-up.
    pub balance_part: Money,
    pub target_part: Money,
}

/// A bucket's targets, added up the way its balance is.
///
/// Only members with a target take part - on both sides of the comparison -
/// so a bucket half of whose ledgers have targets reports progress on that
/// half rather than mixing in money nobody set a goal for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TargetRollUp {
    pub lines: Vec<TargetLine>,
    /// How many members the bucket has, targets or not.
    pub members: usize,
    pub balance: Money,
    pub target: Money,
}

impl TargetRollUp {
    /// How far is left to go, in the direction the target lies.
    pub fn remaining(&self) -> Money {
        Money((self.target.0 - self.balance.0).abs())
    }

    pub fn is_reached(&self) -> bool {
        self.balance == self.target
    }
}

pub fn bucket_targets(l: &Budget, bucket: BucketUid, roll: RollUp) -> Option<TargetRollUp> {
    let bix = l.buckets.ix(bucket)?;
    let a = &l.ledgers;
    let members = &l.buckets.members[bix.get()];
    let part = |ix: LedgerIx, displayed: Money| match roll {
        RollUp::Sum => displayed,
        RollUp::ByNormality => a.normality[ix.get()].present(displayed),
    };
    let lines: Vec<TargetLine> = members
        .iter()
        .filter_map(|ix| {
            let target = a.target[ix.get()]?;
            let balance = a.balance(*ix);
            Some(TargetLine {
                ledger: *ix,
                name: a.name[ix.get()].clone(),
                balance,
                target,
                balance_part: part(*ix, balance),
                target_part: part(*ix, target),
            })
        })
        .collect();
    Some(TargetRollUp {
        members: members.len(),
        balance: lines.iter().map(|t| t.balance_part).sum(),
        target: lines.iter().map(|t| t.target_part).sum(),
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AlertWhen;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }

    fn m(n: i64) -> Money {
        Money::from_major(n)
    }

    #[test]
    fn a_target_is_reached_whichever_way_the_balance_must_go() {
        let loan = [(d("2024-01-01"), m(300)), (d("2024-02-01"), m(150)), (d("2024-03-01"), m(0))];
        assert_eq!(reached_on(&loan, d("2024-01-15"), m(0)), Some(d("2024-03-01")));
        assert_eq!(reached_on(&loan, d("2024-01-15"), m(200)), Some(d("2024-02-01")));
        let savings =
            [(d("2024-01-01"), m(100)), (d("2024-02-01"), m(900)), (d("2024-03-01"), m(1_200))];
        assert_eq!(reached_on(&savings, d("2024-01-01"), m(1_000)), Some(d("2024-03-01")));
        assert_eq!(reached_on(&savings, d("2024-01-01"), m(5_000)), None, "not within the series");
        assert_eq!(
            reached_on(&savings, d("2024-02-10"), m(900)),
            Some(d("2024-02-10")),
            "already there"
        );
    }

    #[test]
    fn alerts_ahead_skip_what_already_fires() {
        let low = Alert { when: AlertWhen::Below, level: m(500), message: "top up".into() };
        let high = Alert { when: AlertWhen::Above, level: m(50), message: "".into() };
        let pts = [(d("2024-01-01"), m(800)), (d("2024-01-20"), m(450)), (d("2024-02-01"), m(900))];
        let alerts = [low.clone(), high];
        let ahead = alerts_ahead(&pts, d("2024-01-05"), &alerts);
        assert_eq!(ahead, vec![(&low, d("2024-01-20"))], "the high one is already firing");
        assert!(!low.fires(m(500)), "at the level is not past it");
    }
}
