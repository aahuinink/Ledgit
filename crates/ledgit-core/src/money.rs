//! Money is stored as a signed count of minor units (cents), never as a float.
//!
//! Floating point cannot represent 0.10 exactly, so a budget built on `f64`
//! drifts: sum a few thousand transactions and the balance sheet stops
//! balancing. `i64` cents gives exact arithmetic up to ~92 quadrillion dollars,
//! which is enough for a personal budget.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

/// A signed monetary amount in minor units (cents).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Money(pub i64);

impl Money {
    pub const ZERO: Money = Money(0);

    /// Build from whole major units, e.g. `Money::from_major(12)` == $12.00.
    pub const fn from_major(units: i64) -> Money {
        Money(units * 100)
    }

    pub const fn cents(self) -> i64 {
        self.0
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub const fn abs(self) -> Money {
        Money(self.0.abs())
    }

    /// Parse a human amount: "12", "12.5", "12.50", "-3.07", "$1,200.00".
    ///
    /// Rejects more than two fractional digits rather than silently rounding;
    /// a budget app that quietly drops a half-cent is a budget app that
    /// disagrees with your bank.
    pub fn parse(s: &str) -> Result<Money, ParseMoneyError> {
        let cleaned: String = s.chars().filter(|c| !matches!(c, ',' | '$' | '_' | ' ')).collect();
        let (sign, digits) = match cleaned.strip_prefix('-') {
            Some(rest) => (-1i64, rest),
            None => (1i64, cleaned.strip_prefix('+').unwrap_or(&cleaned)),
        };
        if digits.is_empty() {
            return Err(ParseMoneyError);
        }
        let (whole, frac) = match digits.split_once('.') {
            Some((w, f)) => (if w.is_empty() { "0" } else { w }, f),
            None => (digits, ""),
        };
        if frac.len() > 2 || !whole.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseMoneyError);
        }
        if !frac.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseMoneyError);
        }
        let whole: i64 = whole.parse().map_err(|_| ParseMoneyError)?;
        let frac: i64 = match frac.len() {
            0 => 0,
            1 => frac.parse::<i64>().map_err(|_| ParseMoneyError)? * 10,
            _ => frac.parse::<i64>().map_err(|_| ParseMoneyError)?,
        };
        whole
            .checked_mul(100)
            .and_then(|w| w.checked_add(frac))
            .and_then(|v| v.checked_mul(sign))
            .map(Money)
            .ok_or(ParseMoneyError)
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let abs = self.0.unsigned_abs();
        let (whole, cents) = (abs / 100, abs % 100);
        let sign = if self.0 < 0 { "-" } else { "" };
        // `pad` so that `{:>14}` right-aligns an amount in a table.
        f.pad(&format!("{sign}{whole}.{cents:02}"))
    }
}

impl fmt::Debug for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Money({self})")
    }
}

impl Add for Money {
    type Output = Money;
    fn add(self, rhs: Money) -> Money {
        Money(self.0 + rhs.0)
    }
}

impl Sub for Money {
    type Output = Money;
    fn sub(self, rhs: Money) -> Money {
        Money(self.0 - rhs.0)
    }
}

impl Neg for Money {
    type Output = Money;
    fn neg(self) -> Money {
        Money(-self.0)
    }
}

impl AddAssign for Money {
    fn add_assign(&mut self, rhs: Money) {
        self.0 += rhs.0;
    }
}

impl SubAssign for Money {
    fn sub_assign(&mut self, rhs: Money) {
        self.0 -= rhs.0;
    }
}

impl std::iter::Sum for Money {
    fn sum<I: Iterator<Item = Money>>(iter: I) -> Money {
        Money(iter.map(|m| m.0).sum())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseMoneyError;

impl fmt::Display for ParseMoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a valid amount (expected e.g. 12.34)")
    }
}

impl std::error::Error for ParseMoneyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        assert_eq!(Money::parse("12").unwrap(), Money(1200));
        assert_eq!(Money::parse("12.5").unwrap(), Money(1250));
        assert_eq!(Money::parse("12.50").unwrap(), Money(1250));
        assert_eq!(Money::parse("-3.07").unwrap(), Money(-307));
        assert_eq!(Money::parse("$1,200.00").unwrap(), Money(120_000));
        assert_eq!(Money::parse(".99").unwrap(), Money(99));
    }

    #[test]
    fn rejects_sub_cent_precision() {
        assert!(Money::parse("1.234").is_err());
        assert!(Money::parse("").is_err());
        assert!(Money::parse("abc").is_err());
        assert!(Money::parse("1.2.3").is_err());
    }

    #[test]
    fn display_respects_width_so_columns_line_up() {
        assert_eq!(format!("{:>10}|", Money(-1250)), "    -12.50|");
        assert_eq!(format!("{:<10}|", Money(5)), "0.05      |");
    }

    #[test]
    fn display_round_trips() {
        for cents in [0i64, 5, 99, 100, -1, -99, -100, 123_456] {
            let m = Money(cents);
            assert_eq!(Money::parse(&m.to_string()).unwrap(), m);
        }
    }
}
