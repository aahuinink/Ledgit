//! The value types of the domain: what a ledger, transaction, issuer and
//! bucket *are*, independent of how they are stored or versioned.

use crate::date::{days_in_month, Date};
use crate::id::{BucketUid, IssuerUid, LedgerUid, TxUid};
use crate::money::Money;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Which direction increases a ledger's balance.
///
/// In double-entry bookkeeping every ledger has a "normal" side. Assets and
/// expenses are debit-normal (a debit makes them bigger); liabilities, equity
/// and income are credit-normal. The budget stores one raw, debit-positive
/// number per ledger and flips the sign at the edge, so the arithmetic in the
/// hot path never branches on normality.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum Normality {
    Debit,
    Credit,
}

impl Normality {
    /// +1 for debit-normal, -1 for credit-normal.
    pub const fn sign(self) -> i64 {
        match self {
            Normality::Debit => 1,
            Normality::Credit => -1,
        }
    }

    /// Present a raw (debit-positive) figure the way a human expects to read
    /// this ledger: a credit card with $500 owed shows 500, not -500.
    pub const fn present(self, raw: Money) -> Money {
        Money(raw.0 * self.sign())
    }
}

impl fmt::Display for Normality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Normality::Debit => "debit",
            Normality::Credit => "credit",
        })
    }
}

impl std::str::FromStr for Normality {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s.to_ascii_lowercase().as_str() {
            "debit" | "dr" | "asset" | "expense" => Ok(Normality::Debit),
            "credit" | "cr" | "liability" | "income" | "equity" => Ok(Normality::Credit),
            _ => Err(()),
        }
    }
}

/// One side of an entry: a ledger and the signed amount posted to it.
///
/// The sign is debit-positive, the same convention the balance column uses, so
/// applying a leg is `raw_balance[ledger] += amount` with no branch. A
/// positive amount debits the ledger; a negative one credits it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Leg {
    pub ledger: LedgerUid,
    pub amount: Money,
}

impl Leg {
    pub const fn debit(ledger: LedgerUid, amount: Money) -> Leg {
        Leg { ledger, amount }
    }

    pub const fn credit(ledger: LedgerUid, amount: Money) -> Leg {
        Leg { ledger, amount: Money(-amount.0) }
    }

    pub const fn is_debit(&self) -> bool {
        self.amount.0 > 0
    }
}

/// The two legs of an ordinary transfer, in the order a human says it:
/// "move `amount` from `credit` to `debit`".
pub fn simple_legs(debit: LedgerUid, credit: LedgerUid, amount: Money) -> Vec<Leg> {
    vec![Leg::debit(debit, amount), Leg::credit(credit, amount)]
}

/// The size of an entry: the total debited, which equals the total credited.
///
/// This is the number a human means by "how big was that transaction" - a
/// paycheque of $2,400 gross split into $1,800 net, $500 tax and $100 pension
/// is a $2,400 entry, not a $4,800 one.
pub fn magnitude(legs: &[Leg]) -> Money {
    legs.iter().filter(|l| l.is_debit()).map(|l| l.amount).sum()
}

/// Reject anything that is not a valid double entry.
///
/// The rule that matters is the third one. Every other check exists to stop a
/// user producing a technically balanced entry that means nothing.
pub fn validate_legs(legs: &[Leg]) -> Result<(), String> {
    if legs.len() < 2 {
        return Err("an entry needs at least two sides".into());
    }
    if legs.iter().any(|l| l.amount.is_zero()) {
        return Err("an entry cannot have a side of zero".into());
    }
    if legs.iter().map(|l| l.amount).sum::<Money>() != Money::ZERO {
        let debits = magnitude(legs);
        let credits: Money = legs.iter().filter(|l| !l.is_debit()).map(|l| -l.amount).sum();
        return Err(format!(
            "debits ({debits}) do not equal credits ({credits}); the entry is out by {}",
            debits - credits
        ));
    }
    for (i, leg) in legs.iter().enumerate() {
        if legs[i + 1..].iter().any(|other| other.ledger == leg.ledger) {
            // Two sides against one ledger is always a mistake or a
            // simplification the user should make themselves; silently netting
            // them would hide a typo.
            return Err("a ledger appears twice in the same entry".into());
        }
    }
    Ok(())
}

/// What caused a transaction to exist.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Parent {
    /// Entered by hand in the app.
    Manual,
    /// Emitted by a recurring issuer on one of its due dates.
    Issuer(IssuerUid),
}

impl Parent {
    pub fn issuer(self) -> Option<IssuerUid> {
        match self {
            Parent::Issuer(u) => Some(u),
            Parent::Manual => None,
        }
    }
}

/// How often an issuer fires. Granularity is one day, as specified.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Schedule {
    /// Every `n` days from the start date. `n = 14` is the biweekly car payment.
    EveryNDays { n: u32 },
    /// Every `n` months, on `day` (clamped to the end of short months, so
    /// `day = 31` means "the last day of the month").
    MonthlyOn { day: u32, every_n_months: u32 },
    /// Once, on the start date. Useful for a scheduled future payment.
    Once,
}

impl Schedule {
    pub fn validate(&self) -> Result<(), &'static str> {
        match *self {
            Schedule::EveryNDays { n: 0 } => Err("interval must be at least one day"),
            Schedule::MonthlyOn { day, .. } if !(1..=31).contains(&day) => {
                Err("day of month must be 1..=31")
            }
            Schedule::MonthlyOn { every_n_months: 0, .. } => {
                Err("interval must be at least one month")
            }
            _ => Ok(()),
        }
    }

    /// The first occurrence on or after `start`.
    pub fn first_on_or_after(&self, start: Date) -> Date {
        match *self {
            Schedule::EveryNDays { .. } | Schedule::Once => start,
            Schedule::MonthlyOn { day, .. } => {
                let (y, m, d) = start.to_ymd();
                let clamped = day.min(days_in_month(y, m));
                if d <= clamped {
                    Date::from_ymd(y, m, clamped).expect("clamped day is valid")
                } else {
                    month_day(start.add_months(1), day)
                }
            }
        }
    }

    /// The occurrence after `current`, or `None` if the schedule is exhausted.
    pub fn next_after(&self, anchor: Date, current: Date) -> Option<Date> {
        match *self {
            Schedule::Once => None,
            Schedule::EveryNDays { n } => Some(current.add_days(n as i32)),
            Schedule::MonthlyOn { day, every_n_months } => {
                // Step from the anchor's month, not from `current`, so that a
                // clamped February does not drag every later month to the 28th.
                let months = months_between(anchor, current) + every_n_months as i32;
                Some(month_day(anchor.add_months(months), day))
            }
        }
    }

    pub fn describe(&self) -> String {
        match *self {
            Schedule::Once => "once".into(),
            Schedule::EveryNDays { n: 1 } => "daily".into(),
            Schedule::EveryNDays { n: 7 } => "weekly".into(),
            Schedule::EveryNDays { n: 14 } => "biweekly".into(),
            Schedule::EveryNDays { n } => format!("every {n} days"),
            Schedule::MonthlyOn { day, every_n_months: 1 } => format!("monthly on the {day}"),
            Schedule::MonthlyOn { day, every_n_months } => {
                format!("every {every_n_months} months on the {day}")
            }
        }
    }
}

fn month_day(in_month: Date, day: u32) -> Date {
    let (y, m, _) = in_month.to_ymd();
    Date::from_ymd(y, m, day.min(days_in_month(y, m))).expect("clamped day is valid")
}

fn months_between(a: Date, b: Date) -> i32 {
    let (ay, am, _) = a.to_ymd();
    let (by, bm, _) = b.to_ymd();
    (by - ay) * 12 + (bm as i32 - am as i32)
}

/// A read-only snapshot of one ledger.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Ledger {
    pub uid: LedgerUid,
    pub name: String,
    pub description: String,
    pub normality: Normality,
    pub opened: Date,
    /// Debit-positive running total. Use [`Ledger::balance`] to display it.
    pub raw_balance: Money,
}

impl Ledger {
    /// The balance as a human reads it for this ledger's normality.
    pub fn balance(&self) -> Money {
        self.normality.present(self.raw_balance)
    }
}

/// A read-only snapshot of one transaction.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Transaction {
    pub uid: TxUid,
    pub name: String,
    pub description: String,
    pub date: Date,
    /// Two or more sides, summing to zero. Debit-positive.
    pub legs: Vec<Leg>,
    pub parent: Parent,
}

impl Transaction {
    /// The size of the entry: total debited.
    pub fn amount(&self) -> Money {
        magnitude(&self.legs)
    }

    pub fn is_split(&self) -> bool {
        self.legs.len() > 2
    }

    pub fn debits(&self) -> impl Iterator<Item = &Leg> {
        self.legs.iter().filter(|l| l.is_debit())
    }

    pub fn credits(&self) -> impl Iterator<Item = &Leg> {
        self.legs.iter().filter(|l| !l.is_debit())
    }
}

/// A read-only snapshot of one issuer.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Issuer {
    pub uid: IssuerUid,
    pub name: String,
    pub description: String,
    /// The entry this issuer posts each time it fires.
    pub legs: Vec<Leg>,
    pub schedule: Schedule,
    /// First date the issuer is eligible to fire.
    pub start: Date,
    /// Last date it has already emitted a transaction for, if any.
    pub emitted_through: Option<Date>,
    pub paused: bool,
}

impl Issuer {
    pub fn amount(&self) -> Money {
        magnitude(&self.legs)
    }
}

/// A read-only snapshot of one bucket.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Bucket {
    pub uid: BucketUid,
    pub name: String,
    pub description: String,
    pub members: Vec<LedgerUid>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> Date {
        Date::from_ymd(y, m, day).unwrap()
    }

    fn acct() -> LedgerUid {
        LedgerUid::new()
    }

    #[test]
    fn a_simple_transfer_is_two_balanced_legs() {
        let (a, b) = (acct(), acct());
        let legs = simple_legs(a, b, Money::from_major(400));
        assert!(validate_legs(&legs).is_ok());
        assert_eq!(magnitude(&legs), Money::from_major(400));
        assert_eq!(legs[0].amount, Money::from_major(400));
        assert_eq!(legs[1].amount, Money::from_major(-400));
    }

    #[test]
    fn a_paycheque_splits_across_several_ledgers() {
        let (net, tax, pension, gross) = (acct(), acct(), acct(), acct());
        let legs = vec![
            Leg::debit(net, Money::from_major(1_800)),
            Leg::debit(tax, Money::from_major(500)),
            Leg::debit(pension, Money::from_major(100)),
            Leg::credit(gross, Money::from_major(2_400)),
        ];
        assert!(validate_legs(&legs).is_ok());
        // The entry is $2,400, not $4,800.
        assert_eq!(magnitude(&legs), Money::from_major(2_400));
    }

    #[test]
    fn unbalanced_entries_say_what_they_are_out_by() {
        let (a, b) = (acct(), acct());
        let legs = vec![Leg::debit(a, Money(500)), Leg::credit(b, Money(400))];
        let err = validate_legs(&legs).unwrap_err();
        assert!(err.contains("out by 1.00"), "{err}");
    }

    #[test]
    fn degenerate_entries_are_rejected() {
        let (a, b) = (acct(), acct());
        assert!(validate_legs(&[]).is_err());
        assert!(validate_legs(&[Leg::debit(a, Money(100))]).is_err());
        // Zero-amount side.
        assert!(validate_legs(&[
            Leg::debit(a, Money(100)),
            Leg::credit(b, Money(100)),
            Leg::debit(acct(), Money::ZERO),
        ])
        .is_err());
        // Same ledger on both sides.
        assert!(validate_legs(&[Leg::debit(a, Money(100)), Leg::credit(a, Money(100))]).is_err());
    }

    #[test]
    fn credit_normal_ledgers_present_flipped() {
        // A credit card: two credits of $100 leaves $200 owed, shown as 200.
        let raw = Money::from_major(-200);
        assert_eq!(Normality::Credit.present(raw), Money::from_major(200));
        assert_eq!(Normality::Debit.present(raw), Money::from_major(-200));
    }

    #[test]
    fn biweekly_steps_by_fourteen_days() {
        let s = Schedule::EveryNDays { n: 14 };
        let start = d(2024, 1, 5);
        assert_eq!(s.first_on_or_after(start), start);
        assert_eq!(s.next_after(start, start), Some(d(2024, 1, 19)));
    }

    #[test]
    fn monthly_on_the_31st_does_not_drift_after_february() {
        let s = Schedule::MonthlyOn { day: 31, every_n_months: 1 };
        let anchor = d(2024, 1, 31);
        let feb = s.next_after(anchor, anchor).unwrap();
        assert_eq!(feb, d(2024, 2, 29));
        // The bug this guards: stepping from Feb 29 must give Mar 31, not Mar 29.
        assert_eq!(s.next_after(anchor, feb), Some(d(2024, 3, 31)));
    }

    #[test]
    fn monthly_first_occurrence_skips_to_next_month_when_past() {
        let s = Schedule::MonthlyOn { day: 5, every_n_months: 1 };
        assert_eq!(s.first_on_or_after(d(2024, 3, 9)), d(2024, 4, 5));
        assert_eq!(s.first_on_or_after(d(2024, 3, 1)), d(2024, 3, 5));
    }

    #[test]
    fn once_never_repeats() {
        let s = Schedule::Once;
        assert_eq!(s.next_after(d(2024, 1, 1), d(2024, 1, 1)), None);
    }
}
