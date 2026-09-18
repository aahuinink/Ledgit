//! A proleptic-Gregorian civil date stored as a day number.
//!
//! The whole app needs exactly one date granularity: the day. Storing a date as
//! `i32` days since 1970-01-01 makes the two operations we actually perform
//! (compare, and add N days) single instructions, and makes a date one column in
//! a packed array instead of a struct with padding.
//!
//! Conversion uses Howard Hinnant's `days_from_civil` / `civil_from_days`,
//! which is exact for the whole i32 range and has no branches on leap years.
//! <https://howardhinnant.github.io/date_algorithms.html>

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Days since the Unix epoch (1970-01-01). Negative values are before it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Date(pub i32);

impl Date {
    pub const EPOCH: Date = Date(0);

    /// Construct from a civil year/month/day. Returns `None` if the date does
    /// not exist (e.g. 2023-02-30).
    pub fn from_ymd(y: i32, m: u32, d: u32) -> Option<Date> {
        if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
            return None;
        }
        Some(Date(days_from_civil(y, m, d)))
    }

    pub fn to_ymd(self) -> (i32, u32, u32) {
        civil_from_days(self.0)
    }

    pub fn year(self) -> i32 {
        self.to_ymd().0
    }

    pub fn month(self) -> u32 {
        self.to_ymd().1
    }

    pub fn day(self) -> u32 {
        self.to_ymd().2
    }

    pub fn add_days(self, n: i32) -> Date {
        Date(self.0 + n)
    }

    /// Add `n` calendar months, clamping the day to the end of the target
    /// month: 2024-01-31 + 1 month == 2024-02-29.
    pub fn add_months(self, n: i32) -> Date {
        let (y, m, d) = self.to_ymd();
        let total = y as i64 * 12 + (m as i64 - 1) + n as i64;
        let (ny, nm) = (total.div_euclid(12) as i32, total.rem_euclid(12) as u32 + 1);
        let nd = d.min(days_in_month(ny, nm));
        Date(days_from_civil(ny, nm, nd))
    }

    /// Today, in UTC.
    ///
    /// Deliberately not local time: reading the machine's time zone needs a
    /// platform dependency, and for a budget the worst case is that an entry
    /// made just before midnight is dated the next day. Front ends that care
    /// pass an explicit date, which every entry form already lets you do.
    ///
    /// Settled, not a placeholder - do not add a time zone crate for this.
    pub fn today_utc() -> Date {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Date(secs.div_euclid(86_400) as i32)
    }

    /// 0 = Monday. 1970-01-01 was a Thursday, so shift by 3.
    pub fn weekday(self) -> u32 {
        (self.0 + 3).rem_euclid(7) as u32
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = self.to_ymd();
        // Through `pad`, not `write!`, so that `{:<12}` in a table actually
        // pads. A `Display` impl that ignores width silently breaks every
        // column its type appears in.
        f.pad(&format!("{y:04}-{m:02}-{d:02}"))
    }
}

impl fmt::Debug for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Date({self})")
    }
}

impl FromStr for Date {
    type Err = ParseDateError;

    /// Accepts `YYYY-MM-DD` only. One format in, one format out; a budget that
    /// guesses whether 03/04 is March or April is a budget that lies to you.
    fn from_str(s: &str) -> Result<Date, ParseDateError> {
        let mut parts = s.split('-');
        let (y, m, d) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(y), Some(m), Some(d), None) => (y, m, d),
            _ => return Err(ParseDateError),
        };
        let y: i32 = y.parse().map_err(|_| ParseDateError)?;
        let m: u32 = m.parse().map_err(|_| ParseDateError)?;
        let d: u32 = d.parse().map_err(|_| ParseDateError)?;
        Date::from_ymd(y, m, d).ok_or(ParseDateError)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseDateError;

impl fmt::Display for ParseDateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a valid date (expected YYYY-MM-DD)")
    }
}

impl std::error::Error for ParseDateError {}

pub fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    const LENGTHS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if m == 2 && is_leap(y) {
        29
    } else {
        LENGTHS[(m - 1) as usize]
    }
}

/// Hinnant's algorithm: shift the year to start in March so the leap day lands
/// at the end of the era, which removes every leap-year special case.
fn days_from_civil(y: i32, m: u32, d: u32) -> i32 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i32 - 719_468
}

fn civil_from_days(z: i32) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i32 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_1970() {
        assert_eq!(Date::EPOCH.to_ymd(), (1970, 1, 1));
        assert_eq!(Date::from_ymd(1970, 1, 1).unwrap(), Date(0));
    }

    #[test]
    fn round_trips_across_centuries() {
        let mut d = Date::from_ymd(1800, 1, 1).unwrap();
        let end = Date::from_ymd(2200, 1, 1).unwrap();
        while d < end {
            let (y, m, dd) = d.to_ymd();
            assert_eq!(Date::from_ymd(y, m, dd).unwrap(), d, "{y}-{m}-{dd}");
            d = d.add_days(1);
        }
    }

    #[test]
    fn leap_days_exist_only_when_they_should() {
        assert!(Date::from_ymd(2024, 2, 29).is_some());
        assert!(Date::from_ymd(2023, 2, 29).is_none());
        assert!(Date::from_ymd(2000, 2, 29).is_some());
        assert!(Date::from_ymd(1900, 2, 29).is_none());
    }

    #[test]
    fn month_arithmetic_clamps_to_month_end() {
        let jan31 = Date::from_ymd(2024, 1, 31).unwrap();
        assert_eq!(jan31.add_months(1), Date::from_ymd(2024, 2, 29).unwrap());
        assert_eq!(jan31.add_months(13), Date::from_ymd(2025, 2, 28).unwrap());
        assert_eq!(jan31.add_months(-1), Date::from_ymd(2023, 12, 31).unwrap());
    }

    #[test]
    fn weekday_monday_is_zero() {
        // 2024-01-01 was a Monday.
        assert_eq!(Date::from_ymd(2024, 1, 1).unwrap().weekday(), 0);
        assert_eq!(Date::from_ymd(2024, 1, 7).unwrap().weekday(), 6);
    }

    #[test]
    fn display_respects_width_so_tables_line_up() {
        let d = Date::from_ymd(2024, 3, 9).unwrap();
        assert_eq!(format!("{d:<12}|"), "2024-03-09  |");
        assert_eq!(format!("{d:>12}|"), "  2024-03-09|");
    }

    #[test]
    fn parses_iso_only() {
        assert_eq!("2024-03-09".parse::<Date>().unwrap(), Date::from_ymd(2024, 3, 9).unwrap());
        assert!("03/09/2024".parse::<Date>().is_err());
        assert!("2024-13-01".parse::<Date>().is_err());
    }
}
