//! Cohorts: buckets for issuers.
//!
//! A bucket answers "how much is in these ledgers?"; a cohort answers "what
//! do these recurring payments cost, and when do they land?". Two reads cover
//! that, and both work on any list of issuers, so a saved view can ask them
//! of its own selection without a cohort existing:
//!
//! * [`rate_lines`] - each issuer's cost per day, week, month or year, and
//!   [`CohortBreakdown`] to total them;
//! * [`calendar`] - every date each issuer falls due in a window, marked as
//!   already posted, overdue, upcoming or paused.
//!
//! Like buckets, cohorts are flat: they hold issuers, never other cohorts.

use crate::date::Date;
use crate::id::{CohortUid, IssuerIx};
use crate::model::Schedule;
use crate::money::Money;
use crate::period::{per_period, Period};
use crate::state::Budget;

/// One issuer's line in a rate breakdown.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RateLine {
    pub issuer: IssuerIx,
    pub name: String,
    /// The size of each entry it posts; for an issuer with an amount rule,
    /// what the next one comes to on today's balances.
    pub amount: Money,
    pub schedule: Schedule,
    pub paused: bool,
}

impl RateLine {
    /// What this issuer comes to per `period` on average, or `None` for a
    /// one-off, which has no rate.
    pub fn rate(&self, period: Period) -> Option<Money> {
        per_period(self.amount, self.schedule, period)
    }

    /// Whether this line counts towards a total: running, and recurring.
    pub fn is_active(&self) -> bool {
        !self.paused && self.schedule.interval_days().is_some()
    }
}

/// The rate lines for a list of issuers, sorted by name.
pub fn rate_lines(l: &Budget, issuers: &[IssuerIx]) -> Vec<RateLine> {
    let s = &l.issuers;
    let mut lines: Vec<RateLine> = issuers
        .iter()
        .map(|ix| {
            let i = ix.get();
            RateLine {
                issuer: *ix,
                name: s.name[i].clone(),
                amount: crate::issuer::estimate(l, *ix),
                schedule: s.schedule[i],
                paused: s.paused[i],
            }
        })
        .collect();
    lines.sort_by(|a, b| {
        a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.issuer.0.cmp(&b.issuer.0))
    });
    lines
}

/// A cohort's members and what they add up to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CohortBreakdown {
    pub uid: CohortUid,
    pub name: String,
    pub lines: Vec<RateLine>,
}

impl CohortBreakdown {
    /// The combined rate of the members that are actually paying: paused
    /// issuers post nothing, and a one-off has no rate to add.
    pub fn total(&self, period: Period) -> Money {
        self.lines.iter().filter(|l| l.is_active()).filter_map(|l| l.rate(period)).sum()
    }

    /// What the paused members would add if they were resumed.
    pub fn paused_total(&self, period: Period) -> Money {
        self.lines.iter().filter(|l| l.paused).filter_map(|l| l.rate(period)).sum()
    }
}

pub fn breakdown(l: &Budget, cohort: CohortUid) -> Option<CohortBreakdown> {
    let cix = l.cohorts.ix(cohort)?;
    Some(CohortBreakdown {
        uid: cohort,
        name: l.cohorts.name[cix.get()].clone(),
        lines: rate_lines(l, &l.cohorts.members[cix.get()]),
    })
}

/// Where one occurrence stands relative to what has been posted and today.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DueStatus {
    /// The issuer has already emitted this one.
    Posted,
    /// Not emitted, and its date has passed. Running the issuers will post it.
    Overdue,
    /// Today or later, and not emitted yet.
    Upcoming,
    /// Not emitted, and the issuer is paused, so it will not be.
    Paused,
}

/// One date an issuer falls due.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CalendarEntry {
    pub date: Date,
    pub issuer: IssuerIx,
    pub amount: Money,
    pub status: DueStatus,
}

/// Every occurrence of every listed issuer in `from..=to`, by date and then
/// by issuer row, so a redraw never reshuffles two payments on the same day.
pub fn calendar(
    l: &Budget,
    issuers: &[IssuerIx],
    from: Date,
    to: Date,
    today: Date,
) -> Vec<CalendarEntry> {
    let s = &l.issuers;
    let mut out: Vec<CalendarEntry> = Vec::new();
    for ix in issuers {
        let i = ix.get();
        let amount = crate::issuer::estimate(l, *ix);
        for date in s.schedule[i].dates_between(s.start[i], from, to) {
            let status = if s.emitted_through[i].is_some_and(|done| date <= done) {
                DueStatus::Posted
            } else if s.paused[i] {
                DueStatus::Paused
            } else if date < today {
                DueStatus::Overdue
            } else {
                DueStatus::Upcoming
            };
            out.push(CalendarEntry { date, issuer: *ix, amount, status });
        }
    }
    out.sort_by_key(|e| (e.date, e.issuer.0));
    out
}

/// The day before the earliest unposted occurrence of any running issuer:
/// every recurring payment on or before this date is in the budget.
///
/// `None` if nothing is running or nothing has been posted. With no running
/// issuer there is nothing to be behind on; a running issuer that has never
/// fired is behind from its first due date, which this reports as the day
/// before it.
pub fn caught_up_through(l: &Budget) -> Option<Date> {
    l.issuers
        .indices()
        .filter(|ix| !l.issuers.paused[ix.get()])
        .filter_map(|ix| crate::issuer::next_due(l, ix))
        .min()
        .map(|d| d.add_days(-1))
}

/// How many running issuers owe an occurrence dated before `today`.
pub fn overdue_issuers(l: &Budget, today: Date) -> usize {
    l.issuers
        .indices()
        .filter(|ix| !l.issuers.paused[ix.get()])
        .filter(|ix| crate::issuer::next_due(l, *ix).is_some_and(|d| d < today))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{IssuerUid, LedgerUid};
    use crate::model::{simple_legs, Normality};
    use crate::op::Op;

    fn d(y: i32, m: u32, day: u32) -> Date {
        Date::from_ymd(y, m, day).unwrap()
    }

    /// Cash, Rent and Groceries, plus a monthly rent issuer and a weekly
    /// groceries issuer, both in one cohort.
    fn fixture() -> (Budget, CohortUid, IssuerUid, IssuerUid) {
        let (cash, rent_l, food_l) = (LedgerUid::new(), LedgerUid::new(), LedgerUid::new());
        let (rent, food) = (IssuerUid::new(), IssuerUid::new());
        let cohort = CohortUid::new();
        let mk = |uid, name: &str, n| Op::CreateLedger {
            uid,
            name: name.into(),
            description: String::new(),
            normality: n,
            opened: d(2024, 1, 1),
        };
        let l = Budget::replay(&[
            mk(cash, "Cash", Normality::Debit),
            mk(rent_l, "Rent", Normality::Debit),
            mk(food_l, "Groceries", Normality::Debit),
            Op::CreateIssuer {
                uid: rent,
                name: "Rent".into(),
                description: String::new(),
                legs: simple_legs(rent_l, cash, Money::from_major(1_500)),
                schedule: Schedule::MonthlyOn { day: 1, every_n_months: 1 },
                start: d(2024, 1, 1),
                rule: None,
            },
            Op::CreateIssuer {
                uid: food,
                name: "Groceries".into(),
                description: String::new(),
                legs: simple_legs(food_l, cash, Money::from_major(70)),
                schedule: Schedule::EveryNDays { n: 7 },
                start: d(2024, 1, 6),
                rule: None,
            },
            Op::CreateCohort { uid: cohort, name: "Living".into(), description: String::new() },
            Op::AddToCohort { cohort, issuer: rent },
            Op::AddToCohort { cohort, issuer: food },
        ])
        .unwrap();
        (l, cohort, rent, food)
    }

    #[test]
    fn a_cohort_totals_its_members_per_period() {
        let (l, cohort, _, _) = fixture();
        let b = breakdown(&l, cohort).unwrap();
        assert_eq!(b.lines.len(), 2);
        assert_eq!(b.lines[0].name, "Groceries", "sorted by name");
        assert_eq!(b.lines[0].rate(Period::Day), Some(Money::from_major(10)));
        // $1,500 + $70/week * 52.1775 weeks / 12 months = 1500 + 304.37
        assert_eq!(b.total(Period::Month), Money(150_000 + 30_437));
    }

    #[test]
    fn paused_members_leave_the_total_but_are_still_counted_aside() {
        let (mut l, cohort, rent, _) = fixture();
        l.apply(&Op::SetIssuerPaused { uid: rent, paused: true }).unwrap();
        let b = breakdown(&l, cohort).unwrap();
        assert_eq!(b.total(Period::Week), Money::from_major(70));
        assert_eq!(b.paused_total(Period::Month), Money::from_major(1_500));
    }

    #[test]
    fn a_calendar_marks_what_is_posted_overdue_and_upcoming() {
        let (mut l, cohort, rent, _) = fixture();
        l.apply(&Op::AdvanceIssuer { uid: rent, through: d(2024, 2, 1) }).unwrap();
        let members = l.cohorts.members[l.cohorts.ix(cohort).unwrap().get()].clone();
        let cal = calendar(&l, &members, d(2024, 2, 1), d(2024, 2, 29), d(2024, 2, 15));

        let rent_ix = l.issuers.ix(rent).unwrap();
        let rent_days: Vec<_> = cal.iter().filter(|e| e.issuer == rent_ix).collect();
        assert_eq!(rent_days.len(), 1);
        assert_eq!(rent_days[0].status, DueStatus::Posted);

        let food: Vec<(Date, DueStatus)> =
            cal.iter().filter(|e| e.issuer != rent_ix).map(|e| (e.date, e.status)).collect();
        assert_eq!(
            food,
            vec![
                (d(2024, 2, 3), DueStatus::Overdue),
                (d(2024, 2, 10), DueStatus::Overdue),
                (d(2024, 2, 17), DueStatus::Upcoming),
                (d(2024, 2, 24), DueStatus::Upcoming),
            ]
        );
        // Sorted by date across issuers.
        assert!(cal.windows(2).all(|w| w[0].date <= w[1].date));
    }

    #[test]
    fn caught_up_through_is_the_day_before_the_oldest_debt() {
        let (mut l, _, rent, food) = fixture();
        // Food never ran: it is owed from 2024-01-06.
        assert_eq!(caught_up_through(&l), Some(d(2023, 12, 31)));
        l.apply(&Op::AdvanceIssuer { uid: rent, through: d(2024, 3, 1) }).unwrap();
        l.apply(&Op::AdvanceIssuer { uid: food, through: d(2024, 3, 2) }).unwrap();
        // Next food is 03-09, next rent 04-01.
        assert_eq!(caught_up_through(&l), Some(d(2024, 3, 8)));
        assert_eq!(overdue_issuers(&l, d(2024, 3, 20)), 1);
        l.apply(&Op::SetIssuerPaused { uid: food, paused: true }).unwrap();
        assert_eq!(caught_up_through(&l), Some(d(2024, 3, 31)));
        assert_eq!(overdue_issuers(&l, d(2024, 3, 20)), 0);
    }

    #[test]
    fn deleting_a_cohort_keeps_its_issuers() {
        let (mut l, cohort, _, _) = fixture();
        l.apply(&Op::DeleteCohort { uid: cohort }).unwrap();
        assert!(breakdown(&l, cohort).is_none());
        assert_eq!(l.issuers.len(), 2);
        assert!(l.cohorts.is_empty());
    }
}
