//! Targets and alerts: where you want a ledger's balance to get to - or how
//! fast you want it to move - and the levels you want to hear about when it
//! crosses them.
//!
//! Both are settings on a ledger ([`Op::SetLedgerGoals`](crate::op::Op)),
//! versioned like its name, and both are about the balance *as displayed* -
//! a loan's target of 0 means "paid off", whatever its normality.
//!
//! A balance target has no direction of its own. It is reached going
//! whichever way the balance has to travel from where it stands: savings at
//! 4,000 with a target of 10,000 reach it going up; a loan at 18,000 with a
//! target of 0 reaches it going down.
//!
//! A pace ([`Target::Pace`]) is a target with a time dimension: how far the
//! balance should move in each calendar week, month, ... - "$250 a week at
//! most" on Expenses:Food. It is measured per calendar period, never as a
//! rolling window, so a week's budget starts fresh every Monday.

use crate::date::Date;
use crate::id::{BucketUid, LedgerIx, LedgerUid};
use crate::model::{Alert, Bound, Target};
use crate::money::Money;
use crate::period::{convert, div_round, Period};
use crate::query::RollUp;
use crate::state::Budget;
use std::collections::BTreeSet;

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

/// A series' target, when every ledger it adds up has a balance target: the
/// same weighted sum as its balance, over targets instead. `weights` apply to
/// raw (debit-positive) amounts, as in a saved view. A pace is no balance to
/// draw a line at, so one ledger with a pace leaves the series without.
pub fn weighted_target(l: &Budget, weights: &[(LedgerIx, i64)]) -> Option<Money> {
    if weights.is_empty() {
        return None;
    }
    weights
        .iter()
        .map(|(ix, w)| {
            let t = l.ledgers.target[ix.get()]?.balance()?;
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
/// Only members with a balance target take part - on both sides of the
/// comparison - so a bucket half of whose ledgers have targets reports
/// progress on that half rather than mixing in money nobody set a goal for.
/// Paces add up separately, in [`bucket_paces`].
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
            let target = a.target[ix.get()]?.balance()?;
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

// ------------------------------------------------------------------ paces

/// One calendar period of a ledger's movement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PacePeriod {
    pub start: Date,
    /// The first day of the next period.
    pub end: Date,
    /// How far the balance moved in it, as displayed.
    pub flow: Money,
}

/// How far a ledger's displayed balance moved between `from` and `to`, `to`
/// not included.
pub fn flow_between(l: &Budget, ix: LedgerIx, from: Date, to: Date) -> Money {
    let raw: Money = l.ledgers.postings[ix.get()]
        .iter()
        .filter(|p| {
            let d = l.transactions.date[l.postings.tx[p.get()].get()];
            from <= d && d < to
        })
        .map(|p| l.postings.amount[p.get()])
        .sum();
    l.ledgers.normality[ix.get()].present(raw)
}

/// A ledger's movement over the last `periods` calendar `per`s, ending with
/// the one `today` falls in, oldest first. One walk over its postings.
pub fn pace_history(
    l: &Budget,
    ix: LedgerIx,
    per: Period,
    today: Date,
    periods: usize,
) -> Vec<PacePeriod> {
    let mut starts = vec![per.start_of(today)];
    for _ in 1..periods.max(1) {
        let last = *starts.last().expect("never empty");
        starts.push(per.start_of(last.add_days(-1)));
    }
    starts.reverse();
    let end = per.next_start(*starts.last().expect("never empty"));
    let mut raw = vec![0i64; starts.len()];
    for p in &l.ledgers.postings[ix.get()] {
        let d = l.transactions.date[l.postings.tx[p.get()].get()];
        if d < starts[0] || d >= end {
            continue;
        }
        raw[starts.partition_point(|s| *s <= d) - 1] += l.postings.amount[p.get()].0;
    }
    let norm = l.ledgers.normality[ix.get()];
    starts
        .iter()
        .enumerate()
        .map(|(k, start)| PacePeriod {
            start: *start,
            end: starts.get(k + 1).copied().unwrap_or(end),
            flow: norm.present(Money(raw[k])),
        })
        .collect()
}

/// Where the current period stands against its pace.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaceState {
    /// A ceiling already passed. Nothing later in the period can undo it
    /// short of a refund.
    Over,
    /// Moving faster than a ceiling allows, or slower than a floor needs,
    /// for the share of the period gone. Fine by its end only if that
    /// changes.
    OffPace,
    /// Within the pace for the days gone.
    OnPace,
    /// A floor already reached.
    Met,
}

/// A ledger's pace and how the period `today` falls in is going.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PaceStatus {
    pub ledger: LedgerIx,
    pub amount: Money,
    pub per: Period,
    pub bound: Bound,
    pub period: PacePeriod,
    /// The pace's share of the days gone, today included: $250 a week is
    /// $107.14 by the end of Wednesday.
    pub due_by_now: Money,
}

impl PaceStatus {
    /// What a ceiling has left, or a floor still needs. Zero once past it.
    pub fn left(&self) -> Money {
        Money((self.amount.0 - self.period.flow.0).max(0))
    }

    /// How far past a ceiling the period has gone. Zero for a floor.
    pub fn over(&self) -> Money {
        match self.bound {
            Bound::AtMost => Money((self.period.flow.0 - self.amount.0).max(0)),
            Bound::AtLeast => Money::ZERO,
        }
    }

    pub fn state(&self) -> PaceState {
        let flow = self.period.flow;
        match self.bound {
            Bound::AtMost if flow > self.amount => PaceState::Over,
            Bound::AtMost if flow > self.due_by_now => PaceState::OffPace,
            Bound::AtLeast if flow >= self.amount => PaceState::Met,
            Bound::AtLeast if flow < self.due_by_now => PaceState::OffPace,
            _ => PaceState::OnPace,
        }
    }
}

/// How `ix` is doing against its pace in the period `today` falls in, or
/// `None` when its target is not a pace.
pub fn pace_status(l: &Budget, ix: LedgerIx, today: Date) -> Option<PaceStatus> {
    let Target::Pace { amount, per, bound } = l.ledgers.target[ix.get()]? else {
        return None;
    };
    let period = pace_history(l, ix, per, today, 1)[0];
    let gone = (today.0 - period.start.0 + 1) as i128;
    let days = (period.end.0 - period.start.0) as i128;
    let due_by_now = Money(div_round(amount.0 as i128 * gone, days) as i64);
    Some(PaceStatus { ledger: ix, amount, per, bound, period, due_by_now })
}

/// Every ledger with a pace, and how its current period is going, in ledger
/// order.
pub fn paces(l: &Budget, today: Date) -> Vec<PaceStatus> {
    l.ledgers.indices().filter_map(|ix| pace_status(l, ix, today)).collect()
}

/// A period that kept to its ledger's pace before some change and does not
/// after it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PaceBreach {
    pub ledger: LedgerIx,
    pub amount: Money,
    pub per: Period,
    pub bound: Bound,
    pub start: Date,
    pub before: Money,
    pub after: Money,
}

/// Paces that `after` breaks and `before` did not, in the periods of the
/// postings named by `touched` - what committing staged entries would do.
/// Each pace is judged against `after`'s target, so staging a new budget
/// alongside the spending that breaks it still reports it. A floor's period
/// counts as broken only once it is below its amount, so this reports a
/// floor only when a change takes money back out of a period that had met it.
/// Ledger rows index `after`.
pub fn newly_broken(
    before: &Budget,
    after: &Budget,
    touched: impl IntoIterator<Item = (LedgerUid, Date)>,
) -> Vec<PaceBreach> {
    let mut periods = BTreeSet::new();
    for (uid, date) in touched {
        let Some(ix) = after.ledgers.ix(uid) else { continue };
        if let Some(Target::Pace { per, .. }) = after.ledgers.target[ix.get()] {
            periods.insert((ix, per.start_of(date)));
        }
    }
    periods
        .into_iter()
        .filter_map(|(ix, start)| {
            let Some(Target::Pace { amount, per, bound }) = after.ledgers.target[ix.get()] else {
                return None;
            };
            let end = per.next_start(start);
            let now = flow_between(after, ix, start, end);
            let was = match before.ledgers.ix(after.ledgers.uid[ix.get()]) {
                Some(b) => flow_between(before, b, start, end),
                None => Money::ZERO,
            };
            (bound.keeps(amount, was) && !bound.keeps(amount, now)).then_some(PaceBreach {
                ledger: ix,
                amount,
                per,
                bound,
                start,
                before: was,
                after: now,
            })
        })
        .collect()
}

/// One member's part in a bucket's combined pace.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PaceLine {
    pub ledger: LedgerIx,
    pub name: String,
    pub bound: Bound,
    /// The member's own pace, as set.
    pub own: (Money, Period),
    /// That pace as an average in the roll-up's unit.
    pub amount: Money,
    /// The member's movement this period, as displayed.
    pub flow: Money,
    /// What each adds to the bucket's totals under its roll-up.
    pub amount_part: Money,
    pub flow_part: Money,
}

/// A bucket's paces, in one unit and added up the way its balance is.
///
/// Members' paces are converted to `per` by average lengths ($250 a week is
/// $1,087.04 a month) and compared with what actually moved in the calendar
/// `per` that `today` falls in. As with [`bucket_targets`], only members with
/// a pace take part.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PaceRollUp {
    pub lines: Vec<PaceLine>,
    pub members: usize,
    pub per: Period,
    pub start: Date,
    pub end: Date,
    pub amount: Money,
    pub flow: Money,
    /// The members' shared bound, when they agree. Adding a food budget to a
    /// savings habit gives a number, but not one either bound can judge.
    pub bound: Option<Bound>,
}

pub fn bucket_paces(
    l: &Budget,
    bucket: BucketUid,
    roll: RollUp,
    per: Period,
    today: Date,
) -> Option<PaceRollUp> {
    let bix = l.buckets.ix(bucket)?;
    let a = &l.ledgers;
    let members = &l.buckets.members[bix.get()];
    let (start, end) = (per.start_of(today), per.next_start(per.start_of(today)));
    let part = |ix: LedgerIx, displayed: Money| match roll {
        RollUp::Sum => displayed,
        RollUp::ByNormality => a.normality[ix.get()].present(displayed),
    };
    let lines: Vec<PaceLine> = members
        .iter()
        .filter_map(|ix| {
            let Target::Pace { amount: own, per: own_per, bound } = a.target[ix.get()]? else {
                return None;
            };
            let amount = convert(own, own_per, per);
            let flow = flow_between(l, *ix, start, end);
            Some(PaceLine {
                ledger: *ix,
                name: a.name[ix.get()].clone(),
                bound,
                own: (own, own_per),
                amount,
                flow,
                amount_part: part(*ix, amount),
                flow_part: part(*ix, flow),
            })
        })
        .collect();
    let bound = match lines.first() {
        Some(first) if lines.iter().all(|x| x.bound == first.bound) => Some(first.bound),
        _ => None,
    };
    Some(PaceRollUp {
        members: members.len(),
        per,
        start,
        end,
        amount: lines.iter().map(|x| x.amount_part).sum(),
        flow: lines.iter().map(|x| x.flow_part).sum(),
        bound,
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
