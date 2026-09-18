//! Turning what a person typed into a uid.
//!
//! Names are not unique and uids are not memorable, so every entity argument
//! accepts either: an exact-ish name (case-insensitive) or a uid prefix. When
//! a name is ambiguous the command fails and lists the candidates rather than
//! guessing - picking the wrong ledger is how money ends up in the wrong place.

use ledgit_core::prelude::*;

pub fn ledger(l: &Budget, s: &str) -> Result<LedgerUid> {
    let by_name: Vec<usize> = l
        .ledgers
        .name
        .iter()
        .enumerate()
        .filter(|(_, n)| n.eq_ignore_ascii_case(s))
        .map(|(i, _)| i)
        .collect();
    match by_name.len() {
        1 => return Ok(l.ledgers.uid[by_name[0]]),
        n if n > 1 => {
            return Err(Error::Invalid(format!(
                "\"{s}\" matches {n} ledgers; use a uid: {}",
                by_name.iter().map(|i| l.ledgers.uid[*i].short()).collect::<Vec<_>>().join(", ")
            )))
        }
        _ => {}
    }
    let by_uid: Vec<LedgerUid> = l
        .ledgers
        .uid
        .iter()
        .copied()
        .filter(|u| u.to_string().starts_with(&s.to_lowercase()))
        .collect();
    match by_uid.len() {
        1 => Ok(by_uid[0]),
        0 => Err(Error::Invalid(format!("no ledger called \"{s}\""))),
        n => Err(Error::Invalid(format!("\"{s}\" matches {n} ledgers by uid"))),
    }
}

pub fn bucket(l: &Budget, s: &str) -> Result<BucketUid> {
    if let Some(ix) = l.bucket_by_name(s) {
        return Ok(l.buckets.uid[ix.get()]);
    }
    let hits: Vec<BucketUid> = l
        .buckets
        .live()
        .map(|ix| l.buckets.uid[ix.get()])
        .filter(|u| u.to_string().starts_with(&s.to_lowercase()))
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(Error::Invalid(format!("no bucket called \"{s}\""))),
        n => Err(Error::Invalid(format!("\"{s}\" matches {n} buckets"))),
    }
}

pub fn issuer(l: &Budget, s: &str) -> Result<IssuerUid> {
    if let Some(ix) = l.issuer_by_name(s) {
        return Ok(l.issuers.uid[ix.get()]);
    }
    let hits: Vec<IssuerUid> = l
        .issuers
        .uid
        .iter()
        .copied()
        .filter(|u| u.to_string().starts_with(&s.to_lowercase()))
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(Error::Invalid(format!("no issuer called \"{s}\""))),
        n => Err(Error::Invalid(format!("\"{s}\" matches {n} issuers"))),
    }
}

/// `14d`, `weekly`, `biweekly`, `monthly`, `monthly:15`, `quarterly:1`, `once`.
pub fn schedule(s: &str) -> Result<Schedule> {
    let s = s.trim().to_lowercase();
    let bad = || {
        Error::Invalid(format!("unrecognised schedule \"{s}\" (try 14d, weekly, monthly:15, once)"))
    };
    if s == "once" {
        return Ok(Schedule::Once);
    }
    if s == "daily" {
        return Ok(Schedule::EveryNDays { n: 1 });
    }
    if s == "weekly" {
        return Ok(Schedule::EveryNDays { n: 7 });
    }
    if s == "biweekly" {
        return Ok(Schedule::EveryNDays { n: 14 });
    }
    if let Some(days) = s.strip_suffix('d') {
        return days
            .parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .map(|n| Schedule::EveryNDays { n })
            .ok_or_else(bad);
    }
    let (word, day) = match s.split_once(':') {
        Some((w, d)) => (w, Some(d.parse::<u32>().map_err(|_| bad())?)),
        None => (s.as_str(), None),
    };
    let every_n_months = match word {
        "monthly" => 1,
        "quarterly" => 3,
        "yearly" | "annually" => 12,
        _ => return Err(bad()),
    };
    Ok(Schedule::MonthlyOn { day: day.unwrap_or(1), every_n_months })
}

pub fn normality(s: &str) -> Result<Normality> {
    s.parse().map_err(|_| Error::Invalid(format!("normality must be debit or credit, not \"{s}\"")))
}

pub fn date(s: &str) -> Result<Date> {
    s.parse().map_err(|_| Error::Invalid(format!("date must be YYYY-MM-DD, not \"{s}\"")))
}

pub fn money(s: &str) -> Result<Money> {
    Money::parse(s).map_err(|_| Error::Invalid(format!("amount must look like 12.34, not \"{s}\"")))
}

/// Build the legs of an entry from repeated `--debit`/`--credit` arguments.
///
/// Each argument is `LEDGER` or `LEDGER:AMOUNT`. Exactly one leg may leave
/// its amount off, and it takes whatever balances the entry - which is how you
/// want to type a paycheque:
///
/// ```text
/// ledgit post "Paycheque" --debit Chequing:1800 --debit Tax:500 --credit "Gross pay"
/// ```
///
/// A bare positional amount is the two-leg shorthand and may only be used when
/// there is one debit and one credit, neither carrying its own amount.
pub fn legs(
    l: &Budget,
    debits: &[String],
    credits: &[String],
    positional: Option<&str>,
) -> Result<Vec<Leg>> {
    if debits.is_empty() || credits.is_empty() {
        return Err(Error::Invalid("an entry needs at least one --debit and one --credit".into()));
    }

    let mut parsed: Vec<(LedgerUid, Option<Money>, i64)> = Vec::new();
    for (side, sign) in [(debits, 1i64), (credits, -1i64)] {
        for spec in side {
            let (name, amount) = match spec.rsplit_once(':') {
                Some((n, a)) => (n, Some(money(a)?)),
                None => (spec.as_str(), None),
            };
            if amount.is_some_and(|m| m.cents() <= 0) {
                return Err(Error::Invalid(format!(
                    "\"{spec}\": each side names a positive amount; --debit and --credit \
                     already say which way it goes"
                )));
            }
            parsed.push((ledger(l, name)?, amount, sign));
        }
    }

    match positional {
        Some(text) => {
            if parsed.len() != 2 || parsed.iter().any(|(_, a, _)| a.is_some()) {
                return Err(Error::Invalid(
                    "a bare amount only works with one --debit and one --credit; \
                     otherwise put the amount on each side, as LEDGER:AMOUNT"
                        .into(),
                ));
            }
            let amount = money(text)?;
            if amount.cents() <= 0 {
                return Err(Error::Invalid("the amount must be positive".into()));
            }
            Ok(simple_legs(parsed[0].0, parsed[1].0, amount))
        }
        None => {
            let blanks = parsed.iter().filter(|(_, a, _)| a.is_none()).count();
            if blanks > 1 {
                return Err(Error::Invalid(
                    "only one side may leave its amount off; that side takes the remainder".into(),
                ));
            }
            let known: Money =
                parsed.iter().filter_map(|(_, a, sign)| a.map(|m| Money(m.cents() * sign))).sum();
            if blanks == 0 && !known.is_zero() {
                return Err(Error::Invalid(format!(
                    "the entry is out by {known}; debits must equal credits"
                )));
            }
            let out = parsed
                .iter()
                .map(|(ledger, amount, sign)| {
                    let cents = match amount {
                        Some(m) => m.cents() * sign,
                        // The blank leg absorbs whatever the rest left over.
                        None => -known.cents(),
                    };
                    Leg { ledger: *ledger, amount: Money(cents) }
                })
                .collect::<Vec<Leg>>();
            validate_legs(&out).map_err(Error::Invalid)?;
            Ok(out)
        }
    }
}
