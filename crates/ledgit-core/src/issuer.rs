//! Recurring transactions.
//!
//! An issuer never mutates the budget directly. It *proposes* ops, which land
//! in the staging area exactly like hand-entered ones and appear in the
//! pre-commit report. That is deliberate: a budget where the software silently
//! posts money while you are not looking is a budget you stop trusting.
//!
//! Each batch ends with an `AdvanceIssuer` op recording the date emitted
//! through, so replaying history - or rebasing it - never double-posts rent.
//!
//! Most issuers post the same entry every time. One with an [`AmountRule`]
//! works its amount out from a balance - interest on a loan, a share of
//! savings - so its occurrences cannot be priced one issuer at a time: this
//! month's interest depends on last month's, and on the payment another
//! issuer made in between. [`project`] therefore walks every occurrence of
//! every issuer in date order, keeping the balances it has moved, and is the
//! one place amounts are worked out - for posting ([`run_all`]) and for the
//! simulation in saved views alike.

use crate::date::Date;
use crate::expr::Ratio;
use crate::id::{IssuerIx, LedgerIx, TxUid};
use crate::model::{magnitude, AmountRule, Leg, Parent, Schedule};
use crate::money::Money;
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
///
/// An occurrence whose rule comes to nothing - interest on a loan already
/// paid off - posts no entry, but still counts as done.
pub fn run_all(l: &Budget, through: Date) -> Vec<IssuerRun> {
    let active: Vec<IssuerIx> =
        l.issuers.indices().filter(|ix| !l.issuers.paused[ix.get()]).collect();
    let occurrences = project(l, &active, through);
    active
        .into_iter()
        .filter_map(|ix| {
            let mine: Vec<&Occurrence> = occurrences.iter().filter(|o| o.issuer == ix).collect();
            let last = mine.last()?.date;
            let i = ix.get();
            let s = &l.issuers;
            let mut ops: Vec<Op> = mine
                .iter()
                .filter_map(|o| {
                    // The whole entry, splits and all, is copied from the
                    // issuer: a recurring paycheque posts its tax and pension
                    // legs every time, not just its net.
                    let legs = o.legs.clone()?;
                    Some(Op::PostTransaction {
                        uid: TxUid::new(),
                        name: s.name[i].clone(),
                        description: s.description[i].clone(),
                        date: o.date,
                        legs,
                        parent: Parent::Issuer(s.uid[i]),
                    })
                })
                .collect();
            ops.push(Op::AdvanceIssuer { uid: s.uid[i], through: last });
            Some(IssuerRun { issuer: ix, dates: mine.iter().map(|o| o.date).collect(), ops })
        })
        .collect()
}

/// One occurrence an issuer owes, priced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Occurrence {
    pub issuer: IssuerIx,
    pub date: Date,
    /// What it posts, or `None` when its rule comes to nothing.
    pub legs: Option<Vec<Leg>>,
}

/// Every occurrence `issuers` owe on or before `through`, in date order, with
/// amounts worked out against the balances as they will stand - each rule
/// reading its ledger after every earlier occurrence in the list.
pub fn project(l: &Budget, issuers: &[IssuerIx], through: Date) -> Vec<Occurrence> {
    let mut due: Vec<(Date, IssuerIx)> = issuers
        .iter()
        .flat_map(|ix| due_dates(l, *ix, through).into_iter().map(move |d| (d, *ix)))
        .collect();
    due.sort_by_key(|(d, ix)| (*d, ix.0));

    // Raw movement from occurrences already projected, per ledger: settled
    // ones (earlier days) and today's, which a rule on the same day must not
    // see - it reads the balance at the start of its day.
    let mut settled = vec![Money::ZERO; l.ledgers.len()];
    let mut today: Vec<(LedgerIx, Money)> = Vec::new();
    let mut day = None;

    let mut out = Vec::with_capacity(due.len());
    for (date, ix) in due {
        if day != Some(date) {
            for (a, m) in today.drain(..) {
                settled[a.get()] += m;
            }
            day = Some(date);
        }
        let i = ix.get();
        let legs = match l.issuers.rule[i] {
            None => Some(l.issuers.legs[i].clone()),
            Some(rule) => l.ledgers.ix(rule.of()).and_then(|of| {
                let raw = raw_before(l, of, date) + settled[of.get()];
                let balance = l.ledgers.normality[of.get()].present(raw);
                let amount = rule_amount(rule, l.issuers.schedule[i], date, balance)?;
                Some(scale_legs(&l.issuers.legs[i], amount))
            }),
        };
        if let Some(legs) = &legs {
            for leg in legs {
                if let Some(a) = l.ledgers.ix(leg.ledger) {
                    today.push((a, leg.amount));
                }
            }
        }
        out.push(Occurrence { issuer: ix, date, legs });
    }
    out
}

/// What one issuer's next entry comes to, going by balances as they stand
/// now. Exact for a fixed issuer; for one with a rule, an estimate - the
/// real amount depends on what happens before it fires.
pub fn estimate(l: &Budget, ix: IssuerIx) -> Money {
    magnitude(&estimate_legs(l, ix))
}

/// The legs behind [`estimate`].
pub fn estimate_legs(l: &Budget, ix: IssuerIx) -> Vec<Leg> {
    let i = ix.get();
    let Some(rule) = l.issuers.rule[i] else {
        return l.issuers.legs[i].clone();
    };
    let Some(of) = l.ledgers.ix(rule.of()) else {
        return Vec::new();
    };
    let date = next_due(l, ix).unwrap_or(l.issuers.start[i]);
    match rule_amount(rule, l.issuers.schedule[i], date, l.ledgers.balance(of)) {
        Some(amount) => scale_legs(&l.issuers.legs[i], amount),
        None => Vec::new(),
    }
}

/// The ledger's raw balance from postings dated before `date`.
fn raw_before(l: &Budget, of: LedgerIx, date: Date) -> Money {
    l.ledgers.postings[of.get()]
        .iter()
        .filter(|p| l.transactions.date[l.postings.tx[p.get()].get()] < date)
        .map(|p| l.postings.amount[p.get()])
        .sum()
}

/// What a rule charges on `date` against `balance`, or `None` for nothing.
pub fn rule_amount(
    rule: AmountRule,
    schedule: Schedule,
    date: Date,
    balance: Money,
) -> Option<Money> {
    if balance.cents() <= 0 {
        return None;
    }
    let b = Ratio::from_money(balance);
    let amount = match rule {
        AmountRule::ShareOfBalance { rate, .. } => b.checked_mul(rate.ratio()),
        AmountRule::Interest { apr, .. } => {
            let days = Ratio::int(accrual_days(schedule, date) as i64);
            b.checked_mul(apr.ratio())
                .and_then(|x| x.checked_mul(days))
                .and_then(|x| x.checked_div(Ratio::int(365)))
        }
    };
    amount.ok()?.to_money().ok().filter(|m| m.cents() > 0)
}

/// Days of interest an occurrence on `date` covers: back to the occurrence
/// before it.
pub fn accrual_days(schedule: Schedule, date: Date) -> i32 {
    match schedule {
        Schedule::EveryNDays { n } => n as i32,
        Schedule::MonthlyOn { every_n_months, .. } => {
            date.0 - date.add_months(-(every_n_months as i32)).0
        }
        Schedule::Once => 0,
    }
}

/// Scale an entry so it comes to `total`: each leg keeps its share of its
/// side, debits and credits each summing to exactly `total`. Cents that do
/// not divide evenly go to the legs with the largest remainders.
pub fn scale_legs(legs: &[Leg], total: Money) -> Vec<Leg> {
    let mut out: Vec<Leg> = legs.to_vec();
    for debit in [true, false] {
        let side: Vec<usize> = (0..legs.len()).filter(|i| legs[*i].is_debit() == debit).collect();
        let weight: i128 =
            side.iter().map(|i| legs[*i].amount.cents().unsigned_abs() as i128).sum();
        if weight == 0 {
            continue;
        }
        let t = total.cents() as i128;
        let mut given = 0i128;
        let mut rem: Vec<(i128, usize)> = Vec::with_capacity(side.len());
        for i in &side {
            let w = legs[*i].amount.cents().unsigned_abs() as i128;
            let share = t * w / weight;
            rem.push((t * w % weight, *i));
            out[*i].amount = Money(share as i64);
            given += share;
        }
        rem.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, i) in rem.iter().take((t - given) as usize) {
            out[*i].amount.0 += 1;
        }
        if !debit {
            for i in &side {
                out[*i].amount.0 = -out[*i].amount.0;
            }
        }
    }
    out
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
                rule: None,
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
