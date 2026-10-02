//! Posted and available.
//!
//! A ledger's *posted* balance is what its entries add up to. Its *available*
//! balance takes off what is already spoken for: money that will leave
//! because of something already decided, even though no entry has been made
//! yet. Two things are decided in that sense:
//!
//! * a **scheduled** one-off - a payment set up from a transaction ("pay the
//!   vet from chequing on the 15th"), or an entry set to post on a later
//!   date - from the day it is made until it posts;
//! * a **statement** payment, once its statement has closed: the card's
//!   amount is known from the 25th, and is spoken for until it is paid on
//!   the 10th.
//!
//! Recurring issuers are not commitments: next month's rent has not been
//! decided by anything yet, and the projection in a saved view is where
//! that shows.
//!
//! Only the *credit* side of a commitment is held back - the side money
//! leaves from, or a debt grows on. Its debit side, the money arriving or the
//! debt shrinking, counts when it posts and not before. So chequing's
//! available balance drops by the card payment, and the card does not look
//! paid off until it is.

use crate::date::Date;
use crate::id::{BucketUid, IssuerIx, LedgerIx};
use crate::issuer::{self, Statement};
use crate::model::{magnitude, Leg, Schedule};
use crate::money::Money;
use crate::query::{roll_up, LedgerSort, Order, RollUp};
use crate::state::Budget;

/// How far ahead a statement payment can fall due after its statement
/// closes: a close on the 31st paid on the 30th of the month after.
const STATEMENT_REACH: i32 = 62;

/// One payment already decided but not yet posted.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Commitment {
    pub issuer: IssuerIx,
    /// The day it falls due. On or before today, it is overdue: run the
    /// issuers to post it.
    pub date: Date,
    pub legs: Vec<Leg>,
    /// Set for a statement payment: the statement it pays.
    pub statement: Option<Statement>,
}

impl Commitment {
    pub fn amount(&self) -> Money {
        magnitude(&self.legs)
    }

    /// The raw amount (debit-positive) held back on `ledger`: the sum of
    /// this commitment's credits to it, so zero or negative.
    pub fn held_on(&self, l: &Budget, ledger: LedgerIx) -> Money {
        let uid = l.ledgers.uid[ledger.get()];
        self.legs.iter().filter(|g| g.ledger == uid && !g.is_debit()).map(|g| g.amount).sum()
    }
}

/// Every commitment as of `today`, and what each ledger has held back.
#[derive(Clone, Debug, Default)]
pub struct Availability {
    pub commitments: Vec<Commitment>,
    /// Raw amount held back per ledger row: zero or negative.
    held: Vec<Money>,
}

impl Availability {
    pub fn of(l: &Budget, today: Date) -> Availability {
        let commitments = commitments(l, today);
        let mut held = vec![Money::ZERO; l.ledgers.len()];
        for c in &commitments {
            for g in c.legs.iter().filter(|g| !g.is_debit()) {
                if let Some(ix) = l.ledgers.ix(g.ledger) {
                    held[ix.get()] += g.amount;
                }
            }
        }
        Availability { commitments, held }
    }

    /// The ledger's available balance, as displayed for its normality.
    pub fn available(&self, l: &Budget, ix: LedgerIx) -> Money {
        let raw = l.ledgers.raw_balance[ix.get()] + self.held_raw(ix);
        l.ledgers.normality[ix.get()].present(raw)
    }

    /// How far available sits from posted, as displayed: negative for an
    /// asset with money spoken for, positive for a debt about to grow.
    pub fn held(&self, l: &Budget, ix: LedgerIx) -> Money {
        l.ledgers.normality[ix.get()].present(self.held_raw(ix))
    }

    /// The raw amount held back on a ledger.
    pub fn held_raw(&self, ix: LedgerIx) -> Money {
        self.held.get(ix.get()).copied().unwrap_or(Money::ZERO)
    }

    /// A bucket's total as available, under the same roll-up as its posted
    /// total. `None` if the bucket is not on this branch.
    pub fn bucket(&self, l: &Budget, bucket: BucketUid, roll: RollUp) -> Option<Money> {
        let posted = roll_up(l, bucket, roll, LedgerSort::Name, Order::Asc)?.total;
        let bix = l.buckets.ix(bucket)?;
        let held: Money = l.buckets.members[bix.get()]
            .iter()
            .map(|ix| match roll {
                RollUp::ByNormality => self.held_raw(*ix),
                RollUp::Sum => self.held(l, *ix),
            })
            .sum();
        Some(posted + held)
    }

    /// The commitments that hold something back on `ix`, oldest first.
    pub fn against(&self, l: &Budget, ix: LedgerIx) -> Vec<&Commitment> {
        self.commitments.iter().filter(|c| !c.held_on(l, ix).is_zero()).collect()
    }
}

/// Every payment already decided as of `today`, in date order: one-off
/// issuers not yet run, and statement payments whose statement has closed.
/// Paused issuers decide nothing.
pub fn commitments(l: &Budget, today: Date) -> Vec<Commitment> {
    let s = &l.issuers;
    let running: Vec<IssuerIx> = s.indices().filter(|ix| !s.paused[ix.get()]).collect();
    let is_statement =
        |ix: IssuerIx| s.rule[ix.get()].is_some_and(|r| r.closes_before(today).is_some());
    let once = |ix: IssuerIx| s.schedule[ix.get()] == Schedule::Once;
    if !running.iter().any(|ix| once(*ix) || is_statement(*ix)) {
        return Vec::new();
    }
    // Priced the way they will post: in date order alongside every running
    // issuer, so a statement includes the charges other issuers make first.
    let through = running
        .iter()
        .filter(|ix| once(**ix))
        .filter_map(|ix| issuer::next_due(l, *ix))
        .fold(today.add_days(STATEMENT_REACH), Date::max);
    issuer::project(l, &running, through)
        .into_iter()
        .filter(|o| match o.statement {
            Some(st) => st.closed <= today,
            None => once(o.issuer),
        })
        .filter_map(|o| {
            Some(Commitment {
                issuer: o.issuer,
                date: o.date,
                legs: o.legs?,
                statement: o.statement,
            })
        })
        .collect()
}

/// A commitment a set of changes makes, drops or changes the size of.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommitmentChange {
    /// Its issuer's row in the budget *after* the changes. Issuers are never
    /// deleted, so every one from before is there too.
    pub issuer: IssuerIx,
    pub date: Date,
    /// Its size before and after: `None` where it did not exist - new, or
    /// paid by the changes.
    pub before: Option<Money>,
    pub after: Option<Money>,
    /// The ledgers it holds money back on, rows in the budget after.
    pub ledgers: Vec<LedgerIx>,
}

/// What changes in the commitments between two budgets, both read as of
/// `today`: for the pre-commit report.
pub fn changes(before: &Budget, after: &Budget, today: Date) -> Vec<CommitmentChange> {
    let key = |l: &Budget, c: &Commitment| (l.issuers.uid[c.issuer.get()], c.date);
    let old = commitments(before, today);
    let new = commitments(after, today);
    let mut out: Vec<CommitmentChange> = Vec::new();
    let held_ledgers = |c: &Commitment| -> Vec<LedgerIx> {
        c.legs.iter().filter(|g| !g.is_debit()).filter_map(|g| after.ledgers.ix(g.ledger)).collect()
    };
    for c in &new {
        let was = old.iter().find(|o| key(before, o) == key(after, c)).map(|o| o.amount());
        if was != Some(c.amount()) {
            out.push(CommitmentChange {
                issuer: c.issuer,
                date: c.date,
                before: was,
                after: Some(c.amount()),
                ledgers: held_ledgers(c),
            });
        }
    }
    for o in &old {
        let (uid, date) = key(before, o);
        if new.iter().any(|c| key(after, c) == (uid, date)) {
            continue;
        }
        let Some(issuer) = after.issuers.ix(uid) else { continue };
        out.push(CommitmentChange {
            issuer,
            date,
            before: Some(o.amount()),
            after: None,
            ledgers: held_ledgers(o),
        });
    }
    out.sort_by_key(|c| (c.date, c.issuer.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{IssuerUid, LedgerUid, TxUid};
    use crate::model::{simple_legs, AmountRule, Normality, Parent, Rate};
    use crate::op::Op;

    fn d(y: i32, m: u32, day: u32) -> Date {
        Date::from_ymd(y, m, day).unwrap()
    }

    fn major(n: i64) -> Money {
        Money::from_major(n)
    }

    struct Fx {
        l: Budget,
        chequing: LedgerIx,
        visa: LedgerIx,
    }

    /// Chequing with $3,000. A $1,200 vet bill on the Visa, scheduled to be
    /// paid from chequing on March 15. $800 more on the Visa in February,
    /// paid by a statement issuer: closes on the 25th, due on the 10th.
    /// Rent of $1,500 recurs on the 1st.
    fn fixture() -> Fx {
        let [chequing, visa, vet, food, rent_l, opening] =
            std::array::from_fn(|_| LedgerUid::new());
        let (card_pay, rent) = (IssuerUid::new(), IssuerUid::new());
        let vet_tx = TxUid::new();
        let mk = |uid, name: &str, n| Op::CreateLedger {
            uid,
            name: name.into(),
            description: String::new(),
            normality: n,
            opened: d(2024, 1, 1),
        };
        let post = |uid, legs, date| Op::PostTransaction {
            uid,
            name: "t".into(),
            description: String::new(),
            date,
            legs,
            parent: Parent::Manual,
        };
        let issuer = |uid, legs, schedule, start, rule, settles| Op::CreateIssuer {
            uid,
            name: "i".into(),
            description: String::new(),
            legs,
            schedule,
            start,
            rule,
            settles,
        };
        let l = Budget::replay(&[
            mk(chequing, "Chequing", Normality::Debit),
            mk(visa, "Visa", Normality::Credit),
            mk(vet, "Vet", Normality::Debit),
            mk(food, "Food", Normality::Debit),
            mk(rent_l, "Rent", Normality::Debit),
            mk(opening, "Opening", Normality::Credit),
            post(TxUid::new(), simple_legs(chequing, opening, major(3_000)), d(2024, 1, 1)),
            post(TxUid::new(), simple_legs(food, visa, major(800)), d(2024, 2, 20)),
            post(vet_tx, simple_legs(vet, visa, major(1_200)), d(2024, 2, 28)),
            issuer(
                IssuerUid::new(),
                simple_legs(visa, chequing, major(1_200)),
                Schedule::Once,
                d(2024, 3, 15),
                None,
                Some(vet_tx),
            ),
            issuer(
                card_pay,
                simple_legs(visa, chequing, major(1)),
                Schedule::MonthlyOn { day: 10, every_n_months: 1 },
                d(2024, 3, 1),
                Some(AmountRule::Statement {
                    of: visa,
                    close_day: 25,
                    min: major(10),
                    min_rate: Rate(0),
                }),
                None,
            ),
            issuer(
                rent,
                simple_legs(rent_l, chequing, major(1_500)),
                Schedule::MonthlyOn { day: 1, every_n_months: 1 },
                d(2024, 4, 1),
                None,
                None,
            ),
        ])
        .unwrap();
        let ix = |u| l.ledgers.ix(u).unwrap();
        let (chequing, visa) = (ix(chequing), ix(visa));
        Fx { l, chequing, visa }
    }

    #[test]
    fn a_scheduled_payment_and_a_closed_statement_are_held_back() {
        let fx = fixture();
        let a = Availability::of(&fx.l, d(2024, 3, 1));
        // The statement closed Feb 25 at $800; the vet bill came after.
        let amounts: Vec<(Date, Money)> =
            a.commitments.iter().map(|c| (c.date, c.amount())).collect();
        assert_eq!(amounts, vec![(d(2024, 3, 10), major(800)), (d(2024, 3, 15), major(1_200))]);
        // $3,000 - $800 - $1,200. Rent is recurring, so not held.
        assert_eq!(fx.l.ledgers.balance(fx.chequing), major(3_000));
        assert_eq!(a.available(&fx.l, fx.chequing), major(1_000));
        assert_eq!(a.held(&fx.l, fx.chequing), major(-2_000));
        // Payments only shrink the card when they post.
        assert_eq!(a.available(&fx.l, fx.visa), major(2_000));
        assert_eq!(a.against(&fx.l, fx.chequing).len(), 2);
        assert!(a.against(&fx.l, fx.visa).is_empty());
    }

    #[test]
    fn a_statement_is_not_held_before_it_closes() {
        let fx = fixture();
        // Feb 24: February's statement has not closed; the vet bill is
        // already scheduled.
        let a = Availability::of(&fx.l, d(2024, 2, 24));
        assert_eq!(a.available(&fx.l, fx.chequing), major(1_800));
    }

    #[test]
    fn the_report_lists_commitments_made_and_paid() {
        let fx = fixture();
        let today = d(2024, 3, 1);
        let mut after = fx.l.clone();
        for run in issuer::run_all(&fx.l, d(2024, 3, 12)) {
            for op in &run.ops {
                after.apply(op).unwrap();
            }
        }
        // Running through the 12th pays the statement; the vet is still owed.
        let c = changes(&fx.l, &after, today);
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].date, c[0].before, c[0].after), (d(2024, 3, 10), Some(major(800)), None));
        assert_eq!(c[0].ledgers, vec![fx.chequing]);
        // And the other way round, it is a new commitment.
        let c = changes(&after, &fx.l, today);
        assert_eq!((c[0].before, c[0].after), (None, Some(major(800))));
    }

    #[test]
    fn once_posted_nothing_is_held() {
        let mut fx = fixture();
        for run in issuer::run_all(&fx.l, d(2024, 3, 20)) {
            for op in &run.ops {
                fx.l.apply(op).unwrap();
            }
        }
        let a = Availability::of(&fx.l, d(2024, 3, 20));
        assert!(a.commitments.is_empty());
        assert_eq!(a.available(&fx.l, fx.chequing), fx.l.ledgers.balance(fx.chequing));
        assert_eq!(fx.l.ledgers.balance(fx.chequing), major(1_000));
        assert_eq!(fx.l.ledgers.balance(fx.visa), Money::ZERO);
    }
}
