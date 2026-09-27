//! The value types of the domain: what a ledger, transaction, issuer and
//! bucket *are*, independent of how they are stored or versioned.

use crate::date::{days_in_month, Date};
use crate::id::{BucketUid, CohortUid, IssuerUid, LedgerUid, TxUid, ViewUid};
use crate::money::Money;
use crate::period::{Period, Span};
use crate::query::{RollUp, Term, TxFilter};
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

    /// Every occurrence of a schedule anchored at `start` that falls in
    /// `from..=to`, oldest first - whether or not it has been emitted yet.
    ///
    /// This is the calendar question ("when is this due?"), so unlike
    /// `issuer::due_dates` it knows nothing about what has been posted. It
    /// jumps straight to `from` instead of stepping there, so asking about
    /// next month for a daily issuer started in 1990 costs one month of work.
    pub fn dates_between(&self, start: Date, from: Date, to: Date) -> Vec<Date> {
        /// Same safety valve as the issuer runner: two centuries of days.
        const MAX: usize = 75_000;
        let anchor = self.first_on_or_after(start);
        let mut out = Vec::new();
        if to < from || to < anchor {
            return out;
        }
        let mut cursor = match *self {
            Schedule::Once => anchor,
            Schedule::EveryNDays { n } => {
                let behind = (from.0 - anchor.0).max(0);
                let steps = (behind + n as i32 - 1) / n as i32;
                anchor.add_days(steps * n as i32)
            }
            Schedule::MonthlyOn { day, every_n_months } => {
                // Land one step short of `from` and walk the rest, so the
                // day-of-month clamping is always done by `next_after`.
                let whole = (months_between(anchor, from) / every_n_months as i32 - 1).max(0);
                match whole {
                    0 => anchor,
                    k => month_day(anchor.add_months(k * every_n_months as i32), day),
                }
            }
        };
        while cursor <= to && out.len() < MAX {
            if cursor >= from {
                out.push(cursor);
            }
            match self.next_after(anchor, cursor) {
                Some(next) if next > cursor => cursor = next,
                _ => break,
            }
        }
        out
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
    /// Where you want the balance to get to, as displayed, if anywhere.
    pub target: Option<Money>,
    pub alerts: Vec<Alert>,
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
    /// How each occurrence's amount is worked out, when it is not simply
    /// `legs` as written.
    pub rule: Option<AmountRule>,
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
    /// Every ledger the bucket counts: those added one by one, then those
    /// under its subtrees.
    pub members: Vec<LedgerUid>,
    /// Paths whose whole subtree the bucket includes, now and later.
    pub subtrees: Vec<String>,
}

/// A read-only snapshot of one cohort: a bucket, but of issuers.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Cohort {
    pub uid: CohortUid,
    pub name: String,
    pub description: String,
    pub members: Vec<IssuerUid>,
}

/// What a saved view looks at, and over how much time.
///
/// Plain data, like every other thing in an op. A view names entities by uid,
/// never by row, and it names *time* by [`Span`] rather than by date, so the
/// same saved view keeps meaning "the last six months and the next year"
/// whenever it is opened.
///
/// Buckets and cohorts can be deleted after a view names them; evaluating the
/// view then reports them as missing rather than failing, exactly as
/// `query::combine` does for a combination.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewSpec {
    /// Buckets to total, each added or subtracted. Together with `roll` these
    /// define the view's *scope*: the pot of money whose balance is charted
    /// and whose ins and outs the flow tables count.
    pub buckets: Vec<Term>,
    /// Ledgers charted on their own line. If the view has no buckets, these
    /// become the scope instead, each counted as displayed.
    pub ledgers: Vec<LedgerUid>,
    /// Issuers whose flow the view breaks down.
    pub issuers: Vec<IssuerUid>,
    /// Cohorts whose members' flow the view breaks down.
    pub cohorts: Vec<CohortUid>,
    /// Which past transactions count as the view's actual history. Empty
    /// means every transaction that moves the scope.
    pub transactions: Vec<TxFilter>,
    pub roll: RollUp,
    /// The unit flows are reported in: "$ per month".
    pub period: Period,
    /// How far back the chart and the history table reach.
    pub lookback: Span,
    /// How far ahead to simulate.
    pub horizon: Span,
    /// Simulate only the view's own issuers and cohorts rather than every
    /// active issuer. Off, the projected balances are what will actually
    /// happen; on, they answer "what if these were all that happened?"
    pub only_selected_issuers: bool,
}

impl Default for ViewSpec {
    fn default() -> Self {
        ViewSpec {
            buckets: Vec::new(),
            ledgers: Vec::new(),
            issuers: Vec::new(),
            cohorts: Vec::new(),
            transactions: Vec::new(),
            roll: RollUp::ByNormality,
            period: Period::Month,
            lookback: Span::Months(6),
            horizon: Span::Months(12),
            only_selected_issuers: false,
        }
    }
}

impl ViewSpec {
    /// Checks that need no budget. Whether the named entities exist is
    /// checked when the op is applied.
    pub fn validate(&self) -> Result<(), String> {
        self.lookback.validate()?;
        self.horizon.validate()?;
        Ok(())
    }
}

/// A read-only snapshot of one saved view.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SavedView {
    pub uid: ViewUid,
    pub name: String,
    pub description: String,
    pub spec: ViewSpec,
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
    fn dates_between_jumps_to_the_window() {
        let s = Schedule::EveryNDays { n: 14 };
        let start = d(2024, 1, 5);
        assert_eq!(
            s.dates_between(start, d(2024, 2, 1), d(2024, 3, 1)),
            vec![d(2024, 2, 2), d(2024, 2, 16), d(2024, 3, 1)]
        );
        // A window that ends before the schedule starts is empty.
        assert!(s.dates_between(start, d(2023, 1, 1), d(2023, 12, 31)).is_empty());
        // A window opening before the start begins at the start.
        assert_eq!(s.dates_between(start, d(2023, 1, 1), d(2024, 1, 5)), vec![start]);
    }

    #[test]
    fn dates_between_keeps_monthly_clamping_intact() {
        let s = Schedule::MonthlyOn { day: 31, every_n_months: 1 };
        let start = d(2020, 1, 1);
        assert_eq!(
            s.dates_between(start, d(2024, 2, 1), d(2024, 4, 30)),
            vec![d(2024, 2, 29), d(2024, 3, 31), d(2024, 4, 30)]
        );
        let quarterly = Schedule::MonthlyOn { day: 1, every_n_months: 3 };
        // Anchored in January: Jan, Apr, Jul, Oct - never Feb or May.
        assert_eq!(
            quarterly.dates_between(d(2023, 1, 1), d(2024, 2, 1), d(2024, 12, 31)),
            vec![d(2024, 4, 1), d(2024, 7, 1), d(2024, 10, 1)]
        );
        assert_eq!(
            Schedule::Once.dates_between(d(2024, 5, 5), d(2024, 1, 1), d(2024, 12, 1)),
            vec![d(2024, 5, 5)]
        );
        assert!(Schedule::Once
            .dates_between(d(2024, 5, 5), d(2024, 6, 1), d(2024, 12, 1))
            .is_empty());
    }

    #[test]
    fn once_never_repeats() {
        let s = Schedule::Once;
        assert_eq!(s.next_after(d(2024, 1, 1), d(2024, 1, 1)), None);
    }
}

/// A named value you can use when making entries: a number in an amount
/// (`200 * Car_Km_Rate`), or text in a name or description
/// (`Mileage at {Car_Km_Rate}/km`).
///
/// Variables are versioned with the budget, so a rate changes in a commit
/// like anything else. Entries store what their formula worked out to, so a
/// later change re-prices nothing already posted.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum VarValue {
    /// Kept as the decimal text it was entered as, e.g. `"0.68"`; always
    /// parses with [`crate::expr::Ratio::parse_decimal`].
    Number(String),
    Text(String),
}

impl VarValue {
    /// Read what someone typed: a number if it is one, else text.
    pub fn guess(s: &str) -> VarValue {
        let t = s.trim();
        match crate::expr::Ratio::parse_decimal(t) {
            Ok(_) if !t.is_empty() => VarValue::Number(t.replace([',', '$', '_', ' '], "")),
            _ => VarValue::Text(s.to_string()),
        }
    }

    pub fn is_number(&self) -> bool {
        matches!(self, VarValue::Number(_))
    }

    pub fn as_str(&self) -> &str {
        match self {
            VarValue::Number(s) | VarValue::Text(s) => s,
        }
    }
}

impl fmt::Display for VarValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// A variable name is an identifier - letters, digits and underscores, not
/// starting with a digit - so it can sit in a formula unquoted.
pub fn validate_var_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    match chars.next() {
        None => Err("a variable needs a name".into()),
        Some(c) if !(c.is_alphabetic() || c == '_') => {
            Err(format!("\"{name}\" must start with a letter or _"))
        }
        _ if !name.chars().all(|c| c.is_alphanumeric() || c == '_') => Err(format!(
            "\"{name}\" may only hold letters, digits and _ (try {})",
            name.replace(|c: char| !(c.is_alphanumeric() || c == '_'), "_")
        )),
        _ => Ok(()),
    }
}

/// A rate, in millionths: 6.45% is `Rate(64_500)`, 5% is `Rate(50_000)`.
///
/// Millionths are exact for any rate written to four decimal places of a
/// percent, which covers every rate a bank will quote you.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Rate(pub i64);

impl Rate {
    /// Parse a percentage: `"6.45"` or `"6.45%"` is 6.45%.
    pub fn parse_percent(s: &str) -> Result<Rate, String> {
        use crate::expr::Ratio;
        let t = s.trim().trim_end_matches('%').trim();
        let pct = Ratio::parse_decimal(t).map_err(|_| format!("\"{s}\" is not a percentage"))?;
        let millionths = pct.checked_mul(Ratio::int(10_000)).map_err(|e| e.to_string())?;
        let whole = millionths.to_money().map_err(|e| e.to_string())?.cents();
        // `to_money` scales by 100; undo it, and insist nothing was rounded.
        if whole % 100 != 0 {
            return Err(format!("\"{s}\" has more than four decimal places"));
        }
        Ok(Rate(whole / 100))
    }

    /// As a fraction of one: 6.45% is 0.0645.
    pub fn ratio(self) -> crate::expr::Ratio {
        crate::expr::Ratio::new(self.0 as i128, 1_000_000).expect("non-zero denominator")
    }
}

impl fmt::Display for Rate {
    /// `6.45%`, `5%`, `0.125%`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pct = crate::expr::Ratio::new(self.0 as i128, 10_000).expect("non-zero denominator");
        f.pad(&format!("{pct}%"))
    }
}

/// How an issuer works out its amount each time it fires, when that depends
/// on a balance rather than being written down once.
///
/// The issuer's legs still say which ledgers move and which way; with a rule
/// their amounts are only proportions, scaled so the entry comes to the
/// worked-out total. A plain two-sided issuer is simply "all of it, here".
///
/// The balance read is `of`'s, as displayed for its normality, at the start
/// of the day the occurrence falls on - after every earlier occurrence,
/// including other issuers', so interest compounds on interest already
/// charged. A balance at or below zero produces nothing that time.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AmountRule {
    /// A share of a balance each time: "move 5% of savings".
    ShareOfBalance { of: LedgerUid, rate: Rate },
    /// Interest at an annual rate, for the days since the previous
    /// occurrence (actual/365): "6.45% APR on the car loan".
    Interest { of: LedgerUid, apr: Rate },
}

impl AmountRule {
    pub fn of(&self) -> LedgerUid {
        match *self {
            AmountRule::ShareOfBalance { of, .. } | AmountRule::Interest { of, .. } => of,
        }
    }

    pub fn validate(&self, schedule: &Schedule) -> Result<(), &'static str> {
        let rate = match *self {
            AmountRule::ShareOfBalance { rate, .. } => rate,
            AmountRule::Interest { apr, .. } => {
                if *schedule == Schedule::Once {
                    return Err("interest needs a repeating schedule to know how long it accrues");
                }
                apr
            }
        };
        if rate.0 <= 0 {
            return Err("the rate must be above zero");
        }
        Ok(())
    }

    /// "6.45% APR on", "5% of" - completed by the ledger's name.
    pub fn describe(&self) -> String {
        match *self {
            AmountRule::ShareOfBalance { rate, .. } => format!("{rate} of"),
            AmountRule::Interest { apr, .. } => format!("{apr} APR on"),
        }
    }
}

/// Which side of a level an [`Alert`] watches.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertWhen {
    Below,
    Above,
}

impl fmt::Display for AlertWhen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            AlertWhen::Below => "below",
            AlertWhen::Above => "above",
        })
    }
}

/// "Tell me when chequing drops below 500": a level on a ledger's displayed
/// balance, and what to say when the balance is past it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Alert {
    pub when: AlertWhen,
    pub level: Money,
    pub message: String,
}

impl Alert {
    /// Whether a displayed balance is past the level. Strictly: a balance of
    /// exactly 500 is not below 500.
    pub fn fires(&self, balance: Money) -> bool {
        match self.when {
            AlertWhen::Below => balance < self.level,
            AlertWhen::Above => balance > self.level,
        }
    }
}
