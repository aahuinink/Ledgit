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
//!
//! A statement issuer - a credit card paid on the 10th for what it stood at
//! on the 25th - reads the history of its ledger rather than one balance, so
//! [`project`] also keeps the date of everything it has moved. An amount set
//! ahead for one occurrence ([`Op::SetIssuerOverride`]) replaces what the
//! issuer would have worked out, raised to the statement's minimum if it has
//! one.

use crate::date::Date;
use crate::expr::Ratio;
use crate::id::{IssuerIx, IssuerUid, LedgerIx, LedgerUid, TxUid};
use crate::model::{magnitude, simple_legs, AmountRule, Leg, Parent, Schedule};
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
    /// What it posts, or `None` when it comes to nothing: interest on a loan
    /// already paid off, or a statement with nothing owing.
    pub legs: Option<Vec<Leg>>,
    /// For a statement issuer, the statement this occurrence pays.
    pub statement: Option<Statement>,
    /// The amount set ahead for this date, as it was set - before any
    /// minimum raised it.
    pub set_ahead: Option<Money>,
}

impl Occurrence {
    pub fn amount(&self) -> Money {
        self.legs.as_deref().map_or(Money::ZERO, magnitude)
    }
}

/// What one statement says, as shown for its ledger's normality.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Statement {
    /// The day it closed: the last closing day before the due date.
    pub closed: Date,
    /// The ledger's balance at the end of that day.
    pub balance: Money,
    /// Everything that brought the balance down after it closed and before
    /// the due date: payments, refunds.
    pub paid_since: Money,
    /// What is still owed on it: never below zero.
    pub owed: Money,
    /// The least that may be paid: the greater of the fixed minimum and the
    /// percentage, and never more than is owed.
    pub minimum: Money,
}

/// A movement already projected: on `date`, `amount` (debit-positive) to
/// a ledger.
type Moved = (Date, LedgerIx, Money);

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
    // see - it reads the balance at the start of its day. `moved` keeps every
    // one with its date, for statements, which read history.
    let mut settled = vec![Money::ZERO; l.ledgers.len()];
    let mut today: Vec<(LedgerIx, Money)> = Vec::new();
    let mut moved: Vec<Moved> = Vec::new();
    let mut day = None;

    let mut out = Vec::with_capacity(due.len());
    for (date, ix) in due {
        if day != Some(date) {
            for (a, m) in today.drain(..) {
                settled[a.get()] += m;
            }
            day = Some(date);
        }
        let o = price(l, ix, date, &settled, &moved);
        if let Some(legs) = &o.legs {
            for leg in legs {
                if let Some(a) = l.ledgers.ix(leg.ledger) {
                    today.push((a, leg.amount));
                    moved.push((date, a, leg.amount));
                }
            }
        }
        out.push(o);
    }
    out
}

/// Work out one occurrence, given what earlier projected occurrences moved:
/// `settled` per ledger before today, and `moved` with dates.
fn price(l: &Budget, ix: IssuerIx, date: Date, settled: &[Money], moved: &[Moved]) -> Occurrence {
    let i = ix.get();
    let s = &l.issuers;
    let set_ahead = s.override_on(ix, date);
    let mut statement = None;
    let worked_out: Option<Money> = match s.rule[i] {
        None => Some(magnitude(&s.legs[i])),
        Some(rule @ AmountRule::Statement { .. }) => l.ledgers.ix(rule.of()).and_then(|of| {
            let st = read_statement(l, rule, of, date, moved);
            statement = Some(st);
            (st.owed.cents() > 0).then_some(st.owed)
        }),
        Some(rule) => l.ledgers.ix(rule.of()).and_then(|of| {
            let raw = raw_before(l, of, date) + settled[of.get()];
            let balance = l.ledgers.normality[of.get()].present(raw);
            rule_amount(rule, s.schedule[i], date, balance)
        }),
    };
    let amount = match (set_ahead, statement) {
        // Nothing owing: an amount set ahead has nothing to pay off.
        (Some(_), Some(st)) if st.owed.cents() <= 0 => None,
        (Some(m), Some(st)) => Some(m.max(st.minimum)),
        (Some(m), None) => Some(m),
        (None, _) => worked_out,
    };
    let legs = amount.map(|m| {
        if s.rule[i].is_none() && set_ahead.is_none() {
            s.legs[i].clone()
        } else {
            scale_legs(&s.legs[i], m)
        }
    });
    Occurrence { issuer: ix, date, legs, statement, set_ahead }
}

/// The statement a payment on `due` settles, reading `of`'s postings and
/// anything already projected onto it.
fn read_statement(
    l: &Budget,
    rule: AmountRule,
    of: LedgerIx,
    due: Date,
    moved: &[Moved],
) -> Statement {
    let AmountRule::Statement { min, min_rate, .. } = rule else {
        unreachable!("only statements are read as statements")
    };
    let closed = rule.closes_before(due).expect("a statement rule");
    let normality = l.ledgers.normality[of.get()];
    let real = l.ledgers.postings[of.get()]
        .iter()
        .map(|p| (l.transactions.date[l.postings.tx[p.get()].get()], l.postings.amount[p.get()]));
    let projected = moved.iter().filter(|(_, a, _)| *a == of).map(|(d, _, m)| (*d, *m));
    let (mut raw, mut paid_since) = (Money::ZERO, Money::ZERO);
    for (date, amount) in real.chain(projected) {
        if date <= closed {
            raw += amount;
        } else if date < due {
            let shown = normality.present(amount);
            if shown.cents() < 0 {
                paid_since += -shown;
            }
        }
    }
    let balance = normality.present(raw);
    let owed = (balance - paid_since).max(Money::ZERO);
    let by_rate = Ratio::from_money(owed)
        .checked_mul(min_rate.ratio())
        .ok()
        .and_then(|r| r.to_money().ok())
        .unwrap_or(Money::ZERO);
    let minimum = min.max(by_rate).min(owed);
    Statement { closed, balance, paid_since, owed, minimum }
}

/// The next `count` occurrences one issuer owes, priced the way they will
/// post - alongside every running issuer, so a statement includes what the
/// others charge first, and an amount set ahead shows on its date. Nothing
/// for a paused issuer; fewer than `count` for one that ends, or that falls
/// due less than `count` times in the next five years.
pub fn upcoming(l: &Budget, ix: IssuerIx, count: usize) -> Vec<Occurrence> {
    let s = &l.issuers;
    let i = ix.get();
    let Some(first) = next_due(l, ix).filter(|_| !s.paused[i] && count > 0) else {
        return Vec::new();
    };
    let anchor = s.schedule[i].first_on_or_after(s.start[i]);
    let (mut last, mut n) = (first, 1);
    while n < count {
        match s.schedule[i].next_after(anchor, last) {
            Some(next) if next > last && next <= first.add_months(60) => (last, n) = (next, n + 1),
            _ => break,
        }
    }
    let running: Vec<IssuerIx> = s.indices().filter(|x| !s.paused[x.get()]).collect();
    project(l, &running, last).into_iter().filter(|o| o.issuer == ix).collect()
}

/// The statement a statement issuer's occurrence on `due` pays, read off
/// the entries on record. `None` for any other issuer.
pub fn statement_on(l: &Budget, ix: IssuerIx, due: Date) -> Option<Statement> {
    let rule = l.issuers.rule[ix.get()].filter(|r| r.closes_before(due).is_some())?;
    let of = l.ledgers.ix(rule.of())?;
    Some(read_statement(l, rule, of, due, &[]))
}

/// The op that schedules paying off a transaction: on `date`, `amount` from
/// `from` onto `owed_on` - out of chequing, onto the card it was put on.
pub fn settlement(
    tx: TxUid,
    tx_name: &str,
    owed_on: LedgerUid,
    from: LedgerUid,
    amount: Money,
    date: Date,
) -> Op {
    Op::CreateIssuer {
        uid: IssuerUid::new(),
        name: format!("Pay off {tx_name}"),
        description: String::new(),
        legs: simple_legs(owed_on, from, amount),
        schedule: Schedule::Once,
        start: date,
        rule: None,
        settles: Some(tx),
    }
}

/// What one issuer's next entry comes to, going by balances as they stand
/// now. Exact for a fixed issuer; for one with a rule, an estimate - the
/// real amount depends on what happens before it fires.
pub fn estimate(l: &Budget, ix: IssuerIx) -> Money {
    magnitude(&estimate_legs(l, ix))
}

/// The legs behind [`estimate`]: the next occurrence, priced on its own.
pub fn estimate_legs(l: &Budget, ix: IssuerIx) -> Vec<Leg> {
    next_occurrence(l, ix).and_then(|o| o.legs).unwrap_or_default()
}

/// The next occurrence this issuer owes, priced on its own - or, if it has
/// none left, what its next one would have been from its start date, so a
/// spent one-off still shows its size.
pub fn next_occurrence(l: &Budget, ix: IssuerIx) -> Option<Occurrence> {
    let date = next_due(l, ix).unwrap_or(l.issuers.start[ix.get()]);
    Some(price(l, ix, date, &vec![Money::ZERO; l.ledgers.len()], &[]))
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
        // A statement reads history, not one balance: see `read_statement`.
        AmountRule::Statement { .. } => return None,
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
                settles: None,
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

    mod statements {
        use super::*;
        use crate::model::Rate;

        struct Card {
            l: Budget,
            chequing: LedgerUid,
            visa: LedgerUid,
            pay: IssuerUid,
        }

        fn buy(l: &mut Budget, card: LedgerUid, food: LedgerUid, date: Date, dollars: i64) {
            l.apply(&Op::PostTransaction {
                uid: TxUid::new(),
                name: "groceries".into(),
                description: String::new(),
                date,
                legs: simple_legs(food, card, Money::from_major(dollars)),
                parent: Parent::Manual,
            })
            .unwrap();
        }

        /// A Visa that closes on the 25th and is paid from chequing on the
        /// 10th, minimum the greater of $10 and 2%. $300 is charged before
        /// the January statement closes, $50 after.
        fn card() -> Card {
            let [chequing, visa, food] = std::array::from_fn(|_| LedgerUid::new());
            let pay = IssuerUid::new();
            let mk = |uid, name: &str, n| Op::CreateLedger {
                uid,
                name: name.into(),
                description: String::new(),
                normality: n,
                opened: d(2024, 1, 1),
            };
            let mut l = Budget::replay(&[
                mk(chequing, "Chequing", Normality::Debit),
                mk(visa, "Visa", Normality::Credit),
                mk(food, "Groceries", Normality::Debit),
                Op::CreateIssuer {
                    uid: pay,
                    name: "Visa payment".into(),
                    description: String::new(),
                    legs: simple_legs(visa, chequing, Money::from_major(1)),
                    schedule: Schedule::MonthlyOn { day: 10, every_n_months: 1 },
                    start: d(2024, 1, 1),
                    rule: Some(AmountRule::Statement {
                        of: visa,
                        close_day: 25,
                        min: Money::from_major(10),
                        min_rate: Rate(20_000),
                    }),
                    settles: None,
                },
            ])
            .unwrap();
            for (day, dollars) in [(5, 100), (20, 200), (28, 50)] {
                buy(&mut l, visa, food, d(2024, 1, day), dollars);
            }
            Card { l, chequing, visa, pay }
        }

        fn february(c: &Card) -> Occurrence {
            let all: Vec<IssuerIx> = c.l.issuers.indices().collect();
            project(&c.l, &all, d(2024, 2, 10))
                .into_iter()
                .find(|o| o.date == d(2024, 2, 10))
                .unwrap()
        }

        #[test]
        fn pays_the_balance_the_statement_closed_on() {
            let c = card();
            let all: Vec<IssuerIx> = c.l.issuers.indices().collect();
            let due = project(&c.l, &all, d(2024, 2, 10));
            // January 10 pays the December statement, which had nothing on it.
            assert_eq!(due[0].date, d(2024, 1, 10));
            assert_eq!(due[0].legs, None);
            let feb = &due[1];
            let st = feb.statement.unwrap();
            assert_eq!(st.closed, d(2024, 1, 25));
            assert_eq!(st.balance, Money::from_major(300), "the $50 on the 28th waits");
            assert_eq!(st.minimum, Money::from_major(10), "2% of 300 is only $6");
            assert_eq!(feb.legs, Some(simple_legs(c.visa, c.chequing, Money::from_major(300))));
        }

        #[test]
        fn a_payment_since_the_close_comes_off() {
            let mut c = card();
            c.l.apply(&Op::PostTransaction {
                uid: TxUid::new(),
                name: "paid early".into(),
                description: String::new(),
                date: d(2024, 2, 3),
                legs: simple_legs(c.visa, c.chequing, Money::from_major(40)),
                parent: Parent::Manual,
            })
            .unwrap();
            let st = february(&c).statement.unwrap();
            assert_eq!((st.paid_since, st.owed), (Money::from_major(40), Money::from_major(260)));
            assert_eq!(february(&c).amount(), Money::from_major(260));
        }

        #[test]
        fn an_amount_set_ahead_is_paid_but_never_under_the_minimum() {
            let mut c = card();
            let set = |l: &mut Budget, dollars| {
                l.apply(&Op::SetIssuerOverride {
                    uid: c.pay,
                    date: d(2024, 2, 10),
                    amount: Some(Money::from_major(dollars)),
                })
                .unwrap()
            };
            set(&mut c.l, 100);
            assert_eq!(february(&c).amount(), Money::from_major(100));
            assert_eq!(february(&c).set_ahead, Some(Money::from_major(100)));
            set(&mut c.l, 5);
            assert_eq!(february(&c).amount(), Money::from_major(10), "raised to the minimum");

            // What was not paid is still on the card at the next close: $300
            // less $10, plus the $50 bought after January closed.
            let all: Vec<IssuerIx> = c.l.issuers.indices().collect();
            let march = project(&c.l, &all, d(2024, 3, 10)).pop().unwrap();
            assert_eq!(march.statement.unwrap().balance, Money::from_major(340));
            assert_eq!(march.amount(), Money::from_major(340));

            // Cleared, it pays the statement again.
            c.l.apply(&Op::SetIssuerOverride { uid: c.pay, date: d(2024, 2, 10), amount: None })
                .unwrap();
            assert_eq!(february(&c).amount(), Money::from_major(300));
        }

        #[test]
        fn an_amount_can_only_be_set_for_a_date_still_owed() {
            let mut c = card();
            let at = |date| Op::SetIssuerOverride {
                uid: c.pay,
                date,
                amount: Some(Money::from_major(50)),
            };
            assert!(c.l.apply(&at(d(2024, 2, 11))).is_err(), "not a due date");
            c.l.apply(&Op::AdvanceIssuer { uid: c.pay, through: d(2024, 2, 10) }).unwrap();
            assert!(c.l.apply(&at(d(2024, 2, 10))).is_err(), "already posted");
            assert!(c.l.apply(&at(d(2024, 3, 10))).is_ok());
        }

        #[test]
        fn a_statement_needs_a_monthly_schedule() {
            let closing = |close_day| AmountRule::Statement {
                of: LedgerUid::new(),
                close_day,
                min: Money::ZERO,
                min_rate: Rate(0),
            };
            let rule = closing(25);
            assert!(rule.validate(&Schedule::EveryNDays { n: 30 }).is_err());
            assert!(rule.validate(&Schedule::MonthlyOn { day: 10, every_n_months: 1 }).is_ok());
            // Closing on the 31st of a month that has none is its last day.
            let late = closing(31);
            assert_eq!(late.closes_before(d(2024, 3, 10)), Some(d(2024, 2, 29)));
            assert_eq!(rule.closes_before(d(2024, 3, 10)), Some(d(2024, 2, 25)));
            let early = closing(5);
            assert_eq!(early.closes_before(d(2024, 3, 10)), Some(d(2024, 3, 5)));
        }

        #[test]
        fn an_override_on_a_fixed_issuer_replaces_its_amount() {
            let (mut l, ix) = fixture(Schedule::EveryNDays { n: 14 }, d(2024, 1, 5));
            let uid = l.issuers.uid[0];
            l.apply(&Op::SetIssuerOverride {
                uid,
                date: d(2024, 1, 19),
                amount: Some(Money::from_major(450)),
            })
            .unwrap();
            let amounts: Vec<Money> =
                project(&l, &[ix], d(2024, 2, 2)).iter().map(|o| o.amount()).collect();
            assert_eq!(amounts, [400, 450, 400].map(Money::from_major));
        }
    }
}
