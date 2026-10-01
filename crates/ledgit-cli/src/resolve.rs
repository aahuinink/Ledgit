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
        1 => return Ok(by_uid[0]),
        0 => {}
        n => return Err(Error::Invalid(format!("\"{s}\" matches {n} ledgers by uid"))),
    }
    // Last, the final segment of a path: "Tuxedo" for "Wedding:Tuxedo", if
    // exactly one ledger ends that way.
    let by_leaf: Vec<usize> = (0..l.ledgers.len())
        .filter(|i| ledgit_core::tree::leaf(&l.ledgers.name[*i]).eq_ignore_ascii_case(s))
        .collect();
    match by_leaf.len() {
        1 => Ok(l.ledgers.uid[by_leaf[0]]),
        0 => Err(Error::Invalid(format!("no ledger called \"{s}\""))),
        n => Err(Error::Invalid(format!(
            "\"{s}\" ends {n} paths; give the whole path: {}",
            by_leaf.iter().map(|i| l.ledgers.name[*i].as_str()).collect::<Vec<_>>().join(", ")
        ))),
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

/// An amount, or a formula over the budget's variables: `200*Car_Km_Rate`.
pub fn amount(l: &Budget, s: &str) -> Result<Money> {
    if let Ok(m) = Money::parse(s) {
        return Ok(m);
    }
    ledgit_core::expr::eval_money(s, &l.variables)
        .map_err(|e| Error::Invalid(format!("amount \"{s}\": {e}")))
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
/// Split `LEDGER` or `LEDGER:AMOUNT`, now that a ledger name may itself hold
/// colons (`Wedding:Venue`, `Wedding:Venue:3000`).
///
/// The whole argument is tried as a ledger first, so a path is never
/// mistaken for an amount. Only if that fails is a trailing `:AMOUNT` split
/// off - and only if it actually parses as money, so a typo in a path reads
/// as "no such ledger" rather than as a baffling amount error.
fn side_spec<'a>(l: &Budget, spec: &'a str) -> Result<(&'a str, Option<Money>)> {
    if ledger(l, spec).is_ok() {
        return Ok((spec, None));
    }
    match spec.rsplit_once(':') {
        Some((n, a)) if amount(l, a.trim()).is_ok() => Ok((n, Some(amount(l, a.trim())?))),
        _ => Ok((spec, None)),
    }
}

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
            let (name, amount) = side_spec(l, spec)?;
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
            let amount = amount(l, text)?;
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

pub fn view(l: &Budget, s: &str) -> Result<ViewUid> {
    if let Some(ix) = l.view_by_name(s) {
        return Ok(l.views.uid[ix.get()]);
    }
    let hits: Vec<ViewUid> = l
        .views
        .live()
        .map(|ix| l.views.uid[ix.get()])
        .filter(|u| u.to_string().starts_with(&s.to_lowercase()))
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(Error::Invalid(format!("no view called \"{s}\""))),
        n => Err(Error::Invalid(format!("\"{s}\" matches {n} views"))),
    }
}

/// `day`, `week`, `month`, `year`.
pub fn period(s: &str) -> Result<Period> {
    s.parse().map_err(|_| {
        Error::Invalid(format!("period must be day, week, month or year, not \"{s}\""))
    })
}

/// `90d`, `6w`, `12m`, `2y`.
pub fn span(s: &str) -> Result<Span> {
    s.parse::<Span>().map_err(Error::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> Budget {
        let mk = |name: &str| Op::CreateLedger {
            uid: LedgerUid::new(),
            name: name.into(),
            description: String::new(),
            normality: Normality::Debit,
            opened: Date::from_ymd(2024, 1, 1).unwrap(),
        };
        Budget::replay(&[
            mk("Chequing"),
            mk("Wedding:Venue"),
            mk("Wedding:Tuxedo"),
            mk("Gifts:Tuxedo"),
        ])
        .unwrap()
    }

    /// A path's colons must never be read as the `LEDGER:AMOUNT` separator.
    #[test]
    fn a_path_is_a_ledger_and_a_trailing_number_is_an_amount() {
        let l = budget();
        let venue = ledger(&l, "Wedding:Venue").unwrap();
        assert_eq!(side_spec(&l, "Wedding:Venue").unwrap(), ("Wedding:Venue", None));
        let (name, amount) = side_spec(&l, "Wedding:Venue:120.50").unwrap();
        assert_eq!((ledger(&l, name).unwrap(), amount), (venue, Some(Money(12_050))));
        // A typo stays a ledger problem, not an amount one.
        assert!(ledger(&l, side_spec(&l, "Wedding:Venu").unwrap().0).is_err());
    }

    #[test]
    fn a_unique_last_segment_names_its_ledger() {
        let l = budget();
        assert_eq!(ledger(&l, "venue").unwrap(), ledger(&l, "Wedding:Venue").unwrap());
        let err = ledger(&l, "Tuxedo").unwrap_err().to_string();
        assert!(err.contains("Wedding:Tuxedo") && err.contains("Gifts:Tuxedo"), "{err}");
    }
}
