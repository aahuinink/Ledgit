//! Units of time for reading money *across* time: "$70 a week", "the last six
//! months", "project a year ahead".
//!
//! Two different questions hide in "per month", and this module keeps them
//! apart:
//!
//! * a **rate** - what a schedule costs on average. A biweekly $400 car
//!   payment is $868.94 a month, even though any one calendar month sees two
//!   or three payments. [`per_period`] answers this, exactly, in integer
//!   arithmetic.
//! * a **calendar period** - the actual month of March. [`Period::start_of`]
//!   and [`Period::next_start`] carve a timeline into those, for tables of
//!   what really happened.
//!
//! Month and year lengths are the Gregorian averages (400 years is 146,097
//! days), so a $10/day issuer reads as $70/week and $304.37/month, and
//! twelve of those months make exactly one year.

use crate::date::Date;
use crate::model::Schedule;
use crate::money::Money;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Days in 400 Gregorian years. Every average length below is a fraction of
/// this, so conversions between units round-trip without drift.
const GREGORIAN_CYCLE: i64 = 146_097;

#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Serialize, Deserialize,
)]
pub enum Period {
    Day,
    Week,
    #[default]
    Month,
    Year,
}

impl Period {
    pub const ALL: [Period; 4] = [Period::Day, Period::Week, Period::Month, Period::Year];

    /// Average length in days, as an exact fraction `(numerator, denominator)`.
    pub const fn days(self) -> (i64, i64) {
        match self {
            Period::Day => (1, 1),
            Period::Week => (7, 1),
            Period::Month => (GREGORIAN_CYCLE, 4_800),
            Period::Year => (GREGORIAN_CYCLE, 400),
        }
    }

    /// First day of the calendar period containing `d`. Weeks start on Monday.
    pub fn start_of(self, d: Date) -> Date {
        match self {
            Period::Day => d,
            Period::Week => d.add_days(-(d.weekday() as i32)),
            Period::Month => Date::from_ymd(d.year(), d.month(), 1).expect("the 1st exists"),
            Period::Year => Date::from_ymd(d.year(), 1, 1).expect("Jan 1 exists"),
        }
    }

    /// First day of the calendar period after the one starting at `start`.
    pub fn next_start(self, start: Date) -> Date {
        match self {
            Period::Day => start.add_days(1),
            Period::Week => start.add_days(7),
            Period::Month => start.add_months(1),
            Period::Year => start.add_months(12),
        }
    }

    /// "day", "week", ... for "$70 per week".
    pub const fn noun(self) -> &'static str {
        match self {
            Period::Day => "day",
            Period::Week => "week",
            Period::Month => "month",
            Period::Year => "year",
        }
    }

    /// "daily", "weekly", ... for column headings.
    pub const fn adjective(self) -> &'static str {
        match self {
            Period::Day => "daily",
            Period::Week => "weekly",
            Period::Month => "monthly",
            Period::Year => "yearly",
        }
    }
}

impl fmt::Display for Period {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.noun())
    }
}

impl std::str::FromStr for Period {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s.to_ascii_lowercase().as_str() {
            "d" | "day" | "daily" => Ok(Period::Day),
            "w" | "week" | "weekly" => Ok(Period::Week),
            "m" | "month" | "monthly" => Ok(Period::Month),
            "y" | "year" | "yearly" | "annual" => Ok(Period::Year),
            _ => Err(()),
        }
    }
}

impl Schedule {
    /// Average days between occurrences, as an exact fraction, or `None` for
    /// a schedule that does not recur.
    pub const fn interval_days(&self) -> Option<(i64, i64)> {
        match *self {
            Schedule::Once => None,
            Schedule::EveryNDays { n } => Some((n as i64, 1)),
            Schedule::MonthlyOn { every_n_months, .. } => {
                let (num, den) = Period::Month.days();
                Some((num * every_n_months as i64, den))
            }
        }
    }
}

/// What `amount` paid on `schedule` comes to per `period`, on average.
///
/// Exact rational arithmetic, rounded half away from zero to the cent at the
/// very end, so a monthly $100 reads as exactly $100.00 a month rather than
/// $99.99 after a round trip through days. A one-off schedule has no rate.
pub fn per_period(amount: Money, schedule: Schedule, period: Period) -> Option<Money> {
    let (in_num, in_den) = schedule.interval_days()?;
    let (p_num, p_den) = period.days();
    // amount * (p_num / p_den) / (in_num / in_den)
    let num = amount.0 as i128 * p_num as i128 * in_den as i128;
    let den = p_den as i128 * in_num as i128;
    Some(Money(div_round(num, den) as i64))
}

/// `amount` a `from`, as an average amount a `to`: $250 a week is
/// $1,087.04 a month. Same exact arithmetic as [`per_period`].
pub fn convert(amount: Money, from: Period, to: Period) -> Money {
    let (f_num, f_den) = from.days();
    let (t_num, t_den) = to.days();
    let num = amount.0 as i128 * f_den as i128 * t_num as i128;
    let den = f_num as i128 * t_den as i128;
    Money(div_round(num, den) as i64)
}

pub(crate) fn div_round(num: i128, den: i128) -> i128 {
    let q = num / den;
    let r = num % den;
    if 2 * r.abs() >= den.abs() {
        q + num.signum() * den.signum()
    } else {
        q
    }
}

/// A length of time counted from a date, in the unit a person would say it:
/// "the last 6 months", "2 years ahead".
///
/// Saved views store spans rather than dates, so a view made in March still
/// means "the next year" when it is opened in November.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Span {
    Days(u32),
    Weeks(u32),
    Months(u32),
    Years(u32),
}

impl Default for Span {
    fn default() -> Self {
        Span::Months(12)
    }
}

impl Span {
    /// The longest span a view may reach. Fifty years of a daily issuer is
    /// about 18,000 simulated postings, which is still instant; anything
    /// longer is a typo, not a plan.
    pub const MAX_DAYS: i32 = 50 * 366;

    pub fn after(self, d: Date) -> Date {
        match self {
            Span::Days(n) => d.add_days(n as i32),
            Span::Weeks(n) => d.add_days(7 * n as i32),
            Span::Months(n) => d.add_months(n as i32),
            Span::Years(n) => d.add_months(12 * n as i32),
        }
    }

    pub fn before(self, d: Date) -> Date {
        match self {
            Span::Days(n) => d.add_days(-(n as i32)),
            Span::Weeks(n) => d.add_days(-7 * n as i32),
            Span::Months(n) => d.add_months(-(n as i32)),
            Span::Years(n) => d.add_months(-12 * n as i32),
        }
    }

    pub fn count(self) -> u32 {
        match self {
            Span::Days(n) | Span::Weeks(n) | Span::Months(n) | Span::Years(n) => n,
        }
    }

    pub fn unit(self) -> Period {
        match self {
            Span::Days(_) => Period::Day,
            Span::Weeks(_) => Period::Week,
            Span::Months(_) => Period::Month,
            Span::Years(_) => Period::Year,
        }
    }

    pub fn new(n: u32, unit: Period) -> Span {
        match unit {
            Period::Day => Span::Days(n),
            Period::Week => Span::Weeks(n),
            Period::Month => Span::Months(n),
            Period::Year => Span::Years(n),
        }
    }

    pub fn validate(self) -> Result<(), String> {
        let from = Date::EPOCH;
        if self.after(from).0 - from.0 > Span::MAX_DAYS {
            return Err(format!("{self} is too far; a view can reach at most 50 years"));
        }
        Ok(())
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.count();
        let unit = self.unit().noun();
        f.pad(&if n == 1 { format!("1 {unit}") } else { format!("{n} {unit}s") })
    }
}

impl std::str::FromStr for Span {
    type Err = String;

    /// `90d`, `6w`, `12m`, `2y`.
    fn from_str(s: &str) -> Result<Span, String> {
        let s = s.trim();
        let bad = || format!("\"{s}\" is not a span; try 90d, 6w, 12m or 2y");
        let split = s.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
        let n: u32 = s[..split].parse().map_err(|_| bad())?;
        let unit: Period = s[split..].parse().map_err(|_| bad())?;
        Ok(Span::new(n, unit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> Date {
        Date::from_ymd(y, m, day).unwrap()
    }

    #[test]
    fn ten_dollars_a_day_reads_the_way_a_person_expects() {
        let daily = Schedule::EveryNDays { n: 1 };
        let ten = Money::from_major(10);
        assert_eq!(per_period(ten, daily, Period::Day), Some(ten));
        assert_eq!(per_period(ten, daily, Period::Week), Some(Money::from_major(70)));
        // 30.436875 days: $304.37, not $300 (30 days) or $310 (31).
        assert_eq!(per_period(ten, daily, Period::Month), Some(Money(30_437)));
        assert_eq!(per_period(ten, daily, Period::Year), Some(Money(365_243)));
    }

    #[test]
    fn a_monthly_amount_is_exactly_itself_per_month() {
        let monthly = Schedule::MonthlyOn { day: 15, every_n_months: 1 };
        let rent = Money::from_major(1_450);
        assert_eq!(per_period(rent, monthly, Period::Month), Some(rent));
        assert_eq!(per_period(rent, monthly, Period::Year), Some(Money::from_major(17_400)));
        let quarterly = Schedule::MonthlyOn { day: 1, every_n_months: 3 };
        assert_eq!(
            per_period(Money::from_major(300), quarterly, Period::Month),
            Some(Money::from_major(100))
        );
    }

    #[test]
    fn biweekly_is_more_than_twice_a_month() {
        let biweekly = Schedule::EveryNDays { n: 14 };
        // 400 * 30.436875 / 14 = 869.625 -> $869.63
        assert_eq!(
            per_period(Money::from_major(400), biweekly, Period::Month),
            Some(Money(86_963))
        );
    }

    #[test]
    fn once_has_no_rate() {
        assert_eq!(per_period(Money::from_major(5), Schedule::Once, Period::Month), None);
    }

    #[test]
    fn converting_a_pace_between_units_uses_the_same_averages() {
        let food = Money::from_major(250);
        assert_eq!(convert(food, Period::Week, Period::Week), food);
        assert_eq!(convert(food, Period::Week, Period::Day), Money(3_571));
        // 250 * 30.436875 / 7 = 1087.0313 -> $1,087.03
        assert_eq!(convert(food, Period::Week, Period::Month), Money(108_703));
        let rent = Money::from_major(1_450);
        assert_eq!(convert(rent, Period::Month, Period::Year), Money::from_major(17_400));
        assert_eq!(convert(Money::from_major(17_400), Period::Year, Period::Month), rent);
    }

    #[test]
    fn rounding_is_half_away_from_zero_both_ways() {
        assert_eq!(div_round(5, 2), 3);
        assert_eq!(div_round(-5, 2), -3);
        assert_eq!(div_round(4, 3), 1);
        assert_eq!(div_round(-4, 3), -1);
    }

    #[test]
    fn calendar_periods_start_where_a_calendar_does() {
        // 2024-03-14 is a Thursday.
        assert_eq!(Period::Week.start_of(d(2024, 3, 14)), d(2024, 3, 11));
        assert_eq!(Period::Month.start_of(d(2024, 3, 14)), d(2024, 3, 1));
        assert_eq!(Period::Year.start_of(d(2024, 3, 14)), d(2024, 1, 1));
        assert_eq!(Period::Month.next_start(d(2024, 1, 1)), d(2024, 2, 1));
    }

    #[test]
    fn spans_parse_and_step() {
        assert_eq!("12m".parse::<Span>(), Ok(Span::Months(12)));
        assert_eq!("90d".parse::<Span>(), Ok(Span::Days(90)));
        assert!("12".parse::<Span>().is_err());
        assert!("m".parse::<Span>().is_err());
        assert_eq!(Span::Months(1).after(d(2024, 1, 31)), d(2024, 2, 29));
        assert_eq!(Span::Years(1).before(d(2024, 2, 29)), d(2023, 2, 28));
        assert!(Span::Years(51).validate().is_err());
        assert!(Span::Years(50).validate().is_ok());
    }
}
