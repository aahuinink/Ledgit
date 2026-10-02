//! Writes the throwaway budgets used to check the GUI by hand.
//!
//! ```sh
//! cargo run -p ledgit-cli --example fixtures            # into ./fixtures
//! cargo run -p ledgit-cli --example fixtures -- C:\tmp  # somewhere else
//! ```
//!
//! Every date is relative to the day it runs, so "today" on the chart, the
//! overdue marks on the calendar and the issuers that are behind all line up
//! with the clock. Re-run it rather than keeping old copies. Existing files of
//! the same name are replaced.
//!
//! Which file exercises which checklist item is in docs/ROADMAP.md.

// Amounts are written as cents with the last group split off: `2_318_45` is
// $2,318.45.
#![allow(clippy::inconsistent_digit_grouping)]

use ledgit_core::prelude::*;
use ledgit_sqlite::SqliteStore;
use std::path::{Path, PathBuf};

type R<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
type Budget = Repo<SqliteStore>;

fn main() -> R {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "fixtures".into()));
    std::fs::create_dir_all(&dir)?;
    let today = Date::today_utc();

    for (name, build) in [
        ("empty.ledgit", empty as fn(&mut Budget, Date) -> R),
        ("household.ledgit", household),
        ("stress.ledgit", stress),
    ] {
        let path = dir.join(name);
        let mut repo = fresh(&path)?;
        build(&mut repo, today)?;
        println!(
            "{:<18} {:>5} ledgers {:>6} transactions {:>4} issuers {:>4} staged",
            name,
            repo.working().ledgers.len(),
            repo.working().transactions.len(),
            repo.working().issuers.len(),
            repo.staged().len(),
        );
    }
    println!("written to {}", dir.display());
    Ok(())
}

fn fresh(path: &Path) -> R<Budget> {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
    Ok(Repo::open(SqliteStore::open(path)?, "fixtures")?)
}

/// A tiny deterministic generator, so amounts vary without a dependency and
/// the same run always writes the same budget.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn one_in(&mut self, n: u64) -> bool {
        self.next().is_multiple_of(n)
    }
    /// Cents in `lo..hi` dollars.
    fn money(&mut self, lo: i64, hi: i64) -> Money {
        let span = ((hi - lo) * 100) as u64;
        Money(lo * 100 + (self.next() % span) as i64)
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next() % xs.len() as u64) as usize]
    }
}

fn cents(c: i64) -> Money {
    Money(c)
}

fn first_of_month(d: Date) -> Date {
    Date::from_ymd(d.year(), d.month(), 1).unwrap()
}

// ------------------------------------------------------------------ empty

/// A new file with nothing in it: every screen's empty state.
fn empty(_: &mut Budget, _: Date) -> R {
    Ok(())
}

// -------------------------------------------------------------- household

struct Books {
    chequing: LedgerUid,
    savings: LedgerUid,
    emergency: LedgerUid,
    tfsa: LedgerUid,
    cash: LedgerUid,
    card: LedgerUid,
    car_loan: LedgerUid,
    mortgage: LedgerUid,
    groceries: LedgerUid,
    restaurants: LedgerUid,
    fuel: LedgerUid,
    gifts: LedgerUid,
    health: LedgerUid,
    side_work: LedgerUid,
    interest: LedgerUid,
    tuxedo: LedgerUid,
    opening: LedgerUid,
}

/// Fifteen months of an ordinary budget: a split paycheque, a mortgage, a car
/// loan, a credit card, a ledger tree, buckets, views, two branches,
/// a reverted mistake, issuers six weeks behind and a few staged edits.
fn household(repo: &mut Budget, today: Date) -> R {
    use Normality::{Credit as C, Debit as D};
    let mut rng = Rng(0x5eed_1ed9);
    let start = first_of_month(today).add_months(-15);

    let l = |repo: &mut Budget, name: &str, desc: &str, n| repo.add_ledger(name, desc, n, start);
    let b = Books {
        chequing: l(repo, "Assets:Bank:Chequing", "day to day", D)?,
        savings: l(repo, "Assets:Bank:Savings", "", D)?,
        emergency: l(repo, "Assets:Bank:Emergency Fund", "three months of expenses", D)?,
        tfsa: l(repo, "Assets:Investments:TFSA", "", D)?,
        cash: l(repo, "Assets:Cash", "wallet", D)?,
        card: l(repo, "Liabilities:Credit Card", "Visa", C)?,
        car_loan: l(repo, "Liabilities:Car Loan", "60 months", C)?,
        mortgage: l(repo, "Liabilities:Mortgage", "", C)?,
        groceries: l(repo, "Expenses:Food:Groceries", "", D)?,
        restaurants: l(repo, "Expenses:Food:Restaurants", "", D)?,
        fuel: l(repo, "Expenses:Transport:Fuel", "", D)?,
        gifts: l(repo, "Expenses:Gifts", "", D)?,
        health: l(repo, "Expenses:Health", "dentist, pharmacy", D)?,
        side_work: l(repo, "Income:Side Work", "", C)?,
        interest: l(repo, "Income:Interest", "", C)?,
        tuxedo: l(repo, "Wedding:Tuxedo", "", D)?,
        opening: l(repo, "Equity:Opening Balances", "where the starting balances came from", C)?,
    };
    let salary = l(repo, "Income:Salary", "", C)?;
    let income_tax = l(repo, "Expenses:Tax:Income Tax", "", D)?;
    let cpp = l(repo, "Expenses:Tax:CPP", "", D)?;
    let ei = l(repo, "Expenses:Tax:EI", "", D)?;
    let hydro = l(repo, "Expenses:Housing:Utilities:Hydro", "", D)?;
    let internet = l(repo, "Expenses:Housing:Utilities:Internet", "", D)?;
    let property_tax = l(repo, "Expenses:Housing:Property Tax", "", D)?;
    let insurance = l(repo, "Expenses:Transport:Insurance", "", D)?;
    let streaming = l(repo, "Expenses:Subscriptions:Streaming", "", D)?;
    let gym = l(repo, "Expenses:Subscriptions:Gym", "", D)?;
    let venue = l(repo, "Wedding:Venue:Deposit", "", D)?;

    for (ledger, amount) in [
        (b.chequing, 3_200),
        (b.savings, 8_500),
        (b.emergency, 12_000),
        (b.tfsa, 21_000),
        (b.cash, 140),
    ] {
        repo.post("Opening balance", "", start, Money::from_major(amount), ledger, b.opening)?;
    }
    for (ledger, amount) in [(b.car_loan, 24_000), (b.mortgage, 312_000), (b.card, 1_150)] {
        repo.post("Opening balance", "", start, Money::from_major(amount), b.opening, ledger)?;
    }
    repo.commit("Open the books")?;

    // Buckets, including subtree ones that pick up ledgers made later.
    let net_worth = repo.add_bucket("Net Worth", "assets less liabilities")?;
    let liquid = repo.add_bucket("Liquid", "money I can spend this week")?;
    let debt = repo.add_bucket("Debt", "")?;
    let spending = repo.add_bucket("Spending", "everything under Expenses")?;
    let wedding = repo.add_bucket("Wedding", "")?;
    let tax = repo.add_bucket("Tax", "")?;
    for (bucket, path) in [
        (net_worth, "Assets"),
        (net_worth, "Liabilities"),
        (debt, "Liabilities"),
        (spending, "Expenses"),
        (wedding, "Wedding"),
        (tax, "Expenses:Tax"),
    ] {
        repo.stage(Op::AddSubtreeToBucket { bucket, path: path.into() })?;
    }
    for ledger in [b.chequing, b.savings, b.cash, b.card] {
        repo.stage(Op::AddToBucket { bucket: liquid, ledger })?;
    }

    // Issuers. The paycheque is a four-way split; the gym gets paused later.
    let monthly = |day| Schedule::MonthlyOn { day, every_n_months: 1 };
    let paycheque = repo.add_issuer_split(
        "Paycheque",
        "gross pay, with deductions",
        vec![
            Leg::debit(b.chequing, cents(2_318_45)),
            Leg::debit(income_tax, cents(682_10)),
            Leg::debit(cpp, cents(171_20)),
            Leg::debit(ei, cents(52_25)),
            Leg::credit(salary, cents(3_224_00)),
        ],
        Schedule::EveryNDays { n: 14 },
        start.add_days(4),
    )?;
    let mortgage_pay = repo.add_issuer(
        "Mortgage",
        "",
        b.mortgage,
        b.chequing,
        cents(1_847_33),
        monthly(1),
        start,
    )?;
    let car = repo.add_issuer(
        "Car payment",
        "",
        b.car_loan,
        b.chequing,
        Money::from_major(400),
        Schedule::EveryNDays { n: 14 },
        start.add_days(8),
    )?;
    let hydro_bill =
        repo.add_issuer("Hydro", "", hydro, b.chequing, cents(96_40), monthly(15), start)?;
    let internet_bill =
        repo.add_issuer("Internet", "", internet, b.card, cents(79_99), monthly(20), start)?;
    let prop_tax = repo.add_issuer(
        "Property tax",
        "quarterly instalment",
        property_tax,
        b.chequing,
        cents(1_120_00),
        Schedule::MonthlyOn { day: 31, every_n_months: 3 },
        start,
    )?;
    let insure =
        repo.add_issuer("Car insurance", "", insurance, b.card, cents(142_18), monthly(28), start)?;
    let stream =
        repo.add_issuer("Streaming", "", streaming, b.card, cents(18_99), monthly(3), start)?;
    let gym_fee =
        repo.add_issuer("Gym", "", gym, b.card, Money::from_major(45), monthly(1), start)?;
    let save = repo.add_issuer(
        "Pay myself first",
        "into savings every payday",
        b.savings,
        b.chequing,
        Money::from_major(300),
        Schedule::EveryNDays { n: 14 },
        start.add_days(5),
    )?;
    let tfsa_contrib =
        repo.add_issuer("TFSA", "", b.tfsa, b.savings, Money::from_major(500), monthly(10), start)?;
    // A one-off in the future, so the calendar has an upcoming mark on its own.
    let venue_due = repo.add_issuer(
        "Venue balance",
        "due two months out",
        venue,
        b.savings,
        Money::from_major(4_500),
        Schedule::Once,
        today.add_days(60),
    )?;

    // Issuers whose amount follows a balance: interest charged on both
    // loans, and a sweep of a share of chequing into the emergency fund.
    let car_interest = repo.add_ledger("Expenses:Interest:Car Loan", "", D, start)?;
    let mortgage_interest = repo.add_ledger("Expenses:Interest:Mortgage", "", D, start)?;
    let apr = |s: &str| Rate::parse_percent(s).expect("a valid rate");
    let car_apr = repo.add_rule_issuer(
        "Car loan interest",
        "6.45% APR, charged monthly",
        car_interest,
        b.car_loan,
        AmountRule::Interest { of: b.car_loan, apr: apr("6.45") },
        monthly(1),
        start.add_months(1),
    )?;
    let mortgage_apr = repo.add_rule_issuer(
        "Mortgage interest",
        "4.89% APR",
        mortgage_interest,
        b.mortgage,
        AmountRule::Interest { of: b.mortgage, apr: apr("4.89") },
        monthly(1),
        start.add_months(1),
    )?;
    let sweep = repo.add_rule_issuer(
        "Sweep to emergency fund",
        "2% of chequing each month",
        b.emergency,
        b.chequing,
        AmountRule::ShareOfBalance { of: b.chequing, rate: apr("2") },
        monthly(28),
        start,
    )?;
    // The card pays itself off: on the 10th, what its statement closed at on
    // the 25th, from chequing. Minimum $10 or 2%.
    let visa_pay = repo.add_rule_issuer(
        "Visa statement",
        "paid in full on the 10th",
        b.card,
        b.chequing,
        AmountRule::Statement {
            of: b.card,
            close_day: 25,
            min: Money::from_major(10),
            min_rate: apr("2"),
        },
        monthly(10),
        start,
    )?;

    // Variables for formulas and names.
    for (name, value) in [("Car_Km_Rate", "0.68"), ("Home", "Toronto"), ("Grocery_Budget", "650")] {
        repo.stage(Op::SetVariable { name: name.into(), value: VarValue::guess(value) })?;
    }

    // Targets and alerts. Every debt has a target, so the Debt bucket and
    // the Debt payoff view's total get one too; chequing's alert fires now
    // and then as the card is paid. Food has paces: groceries a weekly
    // budget the bigger shops break, restaurants a monthly one.
    let alert = |when, level: i64, message: &str| Alert {
        when,
        level: Money::from_major(level),
        message: message.into(),
    };
    let balance = |n: i64| Some(Target::Balance(Money::from_major(n)));
    let at_most = |n: i64, per| {
        Some(Target::Pace { amount: Money::from_major(n), per, bound: Bound::AtMost })
    };
    for (uid, target, alerts) in [
        (b.car_loan, balance(0), vec![]),
        (b.mortgage, balance(0), vec![]),
        (b.card, balance(0), vec![alert(AlertWhen::Above, 3_000, "Pay the card down")]),
        (b.emergency, balance(15_000), vec![]),
        (b.groceries, at_most(180, Period::Week), vec![]),
        (b.restaurants, at_most(150, Period::Month), vec![]),
        (
            b.chequing,
            None,
            vec![
                alert(AlertWhen::Below, 1_000, "Move money over from savings"),
                alert(AlertWhen::Above, 25_000, "Too much sitting in chequing"),
            ],
        ),
    ] {
        repo.stage(Op::SetLedgerGoals { uid, target, alerts })?;
    }

    // The issuers each view breaks down.
    let bills =
        vec![mortgage_pay, car, hydro_bill, internet_bill, prop_tax, insure, car_apr, mortgage_apr];
    let subs = vec![stream, gym_fee, internet_bill];
    let saving = vec![save, tfsa_contrib, venue_due, sweep];

    let views = [
        (
            "Net worth",
            "where it is heading",
            ViewSpec {
                buckets: vec![Term::plus(net_worth)],
                lookback: Span::Months(15),
                horizon: Span::Months(12),
                ..ViewSpec::default()
            },
        ),
        (
            "Debt payoff",
            "the loans on their own lines",
            ViewSpec {
                buckets: vec![Term::plus(debt)],
                ledgers: vec![b.car_loan, b.mortgage, b.card],
                issuers: bills.clone(),
                lookback: Span::Months(12),
                horizon: Span::Years(5),
                ..ViewSpec::default()
            },
        ),
        (
            "Liquid, bills only",
            "what if only the bills and the paycheque happened",
            ViewSpec {
                buckets: vec![Term::plus(liquid)],
                issuers: [bills.as_slice(), &[paycheque]].concat(),
                only_selected_issuers: true,
                period: Period::Week,
                lookback: Span::Weeks(12),
                horizon: Span::Weeks(26),
                ..ViewSpec::default()
            },
        ),
        (
            "Subscriptions",
            "what the subscriptions cost, and when they land",
            ViewSpec {
                buckets: vec![Term::plus(liquid)],
                issuers: subs,
                lookback: Span::Months(6),
                horizon: Span::Months(6),
                ..ViewSpec::default()
            },
        ),
        (
            "Saving",
            "the savings ledgers, and what feeds or drains them",
            ViewSpec {
                ledgers: vec![b.savings, b.emergency, b.tfsa],
                issuers: saving,
                lookback: Span::Months(6),
                horizon: Span::Months(12),
                ..ViewSpec::default()
            },
        ),
        (
            "Spending money",
            "chequing and cash, posted and available",
            ViewSpec {
                ledgers: vec![b.chequing, b.cash],
                lookback: Span::Months(3),
                horizon: Span::Months(3),
                show_available: true,
                ..ViewSpec::default()
            },
        ),
        (
            "Spending by month",
            "",
            ViewSpec {
                buckets: vec![Term::plus(spending), Term::minus(tax)],
                roll: RollUp::Sum,
                lookback: Span::Months(12),
                horizon: Span::Months(3),
                ..ViewSpec::default()
            },
        ),
    ];
    for (name, desc, spec) in views {
        repo.add_view(name, desc, spec)?;
    }
    repo.commit("Buckets, issuers and views")?;

    // Month by month, stopping six weeks short of today so the issuers are
    // behind when the file is opened.
    let stop = today.add_days(-42);
    let branch_at = start.add_months(9);
    let mut month = start;
    let mut n = 0;
    while month < stop {
        let end = month.add_months(1).add_days(-1).min(stop);
        everyday_spending(repo, &b, &mut rng, month, end)?;
        repo.run_issuers(end)?;
        n += 1;
        let (y, m, _) = month.to_ymd();
        repo.commit(format!("{y}-{m:02}"))?;

        match n {
            4 => {
                // A mistake, then its reversal: the History screen's revert.
                repo.post(
                    "Groceries",
                    "typed 1500 for 150",
                    end,
                    Money::from_major(1_500),
                    b.groceries,
                    b.card,
                )?;
                let bad = repo.commit("Groceries at Costco")?;
                repo.revert(&bad.to_string())?;
                repo.post(
                    "Groceries",
                    "the real amount",
                    end,
                    Money::from_major(150),
                    b.groceries,
                    b.card,
                )?;
                repo.commit("Revert the Costco typo and post it properly")?;
            }
            7 => repo.stage(Op::SetIssuerPaused { uid: gym_fee, paused: true })?,
            11 => {
                // A lump sum on the car loan, in a commit of its own: compare
                // the Debt payoff view against the commit before it.
                repo.post(
                    "Bonus to the car loan",
                    "",
                    end,
                    Money::from_major(3_000),
                    b.car_loan,
                    b.savings,
                )?;
                repo.commit("Lump sum on the car loan")?;
            }
            _ => {}
        }
        if month == branch_at {
            what_if_branches(repo, &b, month)?;
        }
        month = month.add_months(1);
    }

    // A mistake to repair on a branch: a $9,000 invoice that was $900,
    // committed alongside good groceries. `fix-side-job` branches from the
    // commit before, cherry-picks that commit, reverses the bad invoice and
    // enters the right one - ready to Reconcile into main.
    let before = repo.head_commit()?.expect("history so far").to_string();
    let when = stop.add_days(3);
    let bad = repo.post(
        "Side job",
        "invoice - typo",
        when,
        Money::from_major(9_000),
        b.chequing,
        b.side_work,
    )?;
    repo.post("Groceries", "Loblaws", when, cents(118_40), b.groceries, b.card)?;
    let mistake = repo.commit("Side job and groceries")?;
    repo.branch("fix-side-job", Some(&before))?;
    repo.checkout("fix-side-job")?;
    repo.cherry_pick(&mistake.to_string())?;
    repo.reverse_transaction(bad)?;
    repo.post("Side job", "invoice", when, Money::from_major(900), b.chequing, b.side_work)?;
    repo.commit("Side job, fixed")?;
    repo.checkout(DEFAULT_BRANCH)?;

    // Leave a few edits staged: the commit screen's report.
    repo.post("Groceries", "this week", today.add_days(-2), cents(143_62), b.groceries, b.card)?;
    repo.post_split(
        "Dinner out",
        "split with Sam, who paid me back in cash",
        today.add_days(-1),
        vec![
            Leg::debit(b.restaurants, cents(64_50)),
            Leg::debit(b.cash, cents(64_50)),
            Leg::credit(b.card, cents(129_00)),
        ],
    )?;
    let tux = repo.post("Tuxedo fitting", "", today, Money::from_major(250), b.tuxedo, b.card)?;
    // Paid off on its own in two weeks, not left for the statement.
    repo.pay_later(tux, b.card, b.chequing, Money::from_major(250), today.add_days(14))?;
    let photo = repo.add_ledger("Wedding:Photographer", "not booked yet", D, today)?;
    // Nothing posts now: the deposit is scheduled for its day.
    repo.add_issuer(
        "Photographer deposit",
        "due when booked",
        photo,
        b.chequing,
        Money::from_major(800),
        Schedule::Once,
        today.add_days(45),
    )?;
    // Next month's card payment, set ahead to less than the statement.
    let card_ix = repo.working().issuers.ix(visa_pay).expect("staged");
    if let Some(o) = ledgit_core::issuer::upcoming(repo.working(), card_ix, 3).last() {
        let uid = visa_pay;
        repo.stage(Op::SetIssuerOverride {
            uid,
            date: o.date,
            amount: Some(Money::from_major(400)),
        })?;
    }
    // An entry worked out from variables, as the forms do it.
    let vars = &repo.working().variables;
    let mileage = ledgit_core::expr::eval_money("180 * Car_Km_Rate", vars)?;
    let name = vars.substitute("Mileage to {Home}")?;
    repo.post(name, "180 km at Car_Km_Rate", today, mileage, b.fuel, b.side_work)?;
    // And one that no longer applies: its ledger was staged, then dropped.
    // The commit screen flags it and will not commit until it is fixed.
    let flowers = repo.add_ledger("Wedding:Flowers", "", D, today)?;
    repo.post("Flowers deposit", "", today, Money::from_major(400), flowers, b.card)?;
    let at = repo.staged().len() - 2;
    repo.unstage_at(at)?;
    Ok(())
}

/// Groceries weekly, fuel every ten days, the odd dinner, all on the card -
/// which its statement issuer pays off.
fn everyday_spending(repo: &mut Budget, b: &Books, rng: &mut Rng, from: Date, to: Date) -> R {
    let mut d = from;
    while d <= to {
        let day = d.0 - from.0;
        if day % 7 == 2 {
            let shop = *rng.pick(&["No Frills", "Costco", "Farmers market", "Loblaws"]);
            repo.post("Groceries", shop, d, rng.money(60, 210), b.groceries, b.card)?;
        }
        if day % 10 == 4 {
            repo.post("Fuel", "", d, rng.money(45, 90), b.fuel, b.card)?;
        }
        if rng.one_in(9) {
            let place = *rng.pick(&["Pho", "Pizza", "Brunch", "Sushi", "Pub"]);
            repo.post(place, "", d, rng.money(18, 95), b.restaurants, b.card)?;
        }
        if rng.one_in(40) {
            repo.post("Pharmacy", "", d, rng.money(12, 80), b.health, b.card)?;
        }
        if rng.one_in(30) {
            repo.post("Side job", "invoice", d, rng.money(200, 900), b.chequing, b.side_work)?;
        }
        if day == 27 {
            repo.post("Interest", "", d, rng.money(8, 20), b.savings, b.interest)?;
        }
        d = d.add_days(1);
    }
    if from.month() == 12 {
        repo.post("Christmas", "", from.add_days(20), Money::from_major(640), b.gifts, b.card)?;
    }
    Ok(())
}

/// Two branches off the main line: a what-if that stays diverged, and an old
/// experiment that can be rebased onto main.
fn what_if_branches(repo: &mut Budget, b: &Books, at: Date) -> R {
    repo.checkout_new("what-if-new-car")?;
    let new_loan =
        repo.add_ledger("Liabilities:New Car Loan", "the 2027 model", Normality::Credit, at)?;
    repo.post("Trade in", "", at, Money::from_major(9_000), b.car_loan, b.chequing)?;
    repo.post("New car", "", at, Money::from_major(38_000), b.chequing, new_loan)?;
    repo.commit("What if I trade the car in")?;
    repo.add_issuer(
        "New car payment",
        "",
        new_loan,
        b.chequing,
        Money::from_major(640),
        Schedule::EveryNDays { n: 14 },
        at.add_days(14),
    )?;
    repo.commit("and pay it off biweekly")?;

    repo.checkout(DEFAULT_BRANCH)?;
    repo.checkout_new("emergency-fund-plan")?;
    let bucket = repo.add_bucket("Rainy day", "savings and the emergency fund")?;
    for ledger in [b.savings, b.emergency] {
        repo.stage(Op::AddToBucket { bucket, ledger })?;
    }
    repo.commit("A bucket for the rainy-day money")?;
    repo.post("Top up", "", at.add_days(3), Money::from_major(2_000), b.emergency, b.savings)?;
    repo.commit("Top up the emergency fund")?;

    repo.checkout(DEFAULT_BRANCH)?;
    Ok(())
}

// ----------------------------------------------------------------- stress

/// Extremes: the widest numbers, the longest names, the deepest tree, a
/// twelve-way split, a crowded calendar and a long history of commits and
/// branches. Nothing here is realistic; it is there to break layouts.
fn stress(repo: &mut Budget, today: Date) -> R {
    use Normality::{Credit as C, Debit as D};
    let mut rng = Rng(0xdead_beef_cafe);
    let start = first_of_month(today).add_months(-24);

    let opening = repo.add_ledger("Equity:Opening", "", C, start)?;
    let rich = repo.add_ledger("Assets:Offshore Trust", "the widest balance", D, start)?;
    let overdrawn = repo.add_ledger("Assets:Overdrawn Chequing", "negative balance", D, start)?;
    let long = repo.add_ledger(
        "Assets:A Ledger Name So Long It Will Not Fit In Any Column Without Being Clipped",
        "and a description that goes on for quite a while, to see where it wraps and whether \
         anything overlaps it, which it should not",
        D,
        start,
    )?;
    let deep = repo.add_ledger(
        "Deep:Level 2:Level 3:Level 4:Level 5:Level 6:Level 7:Level 8:Bottom",
        "eight levels down",
        D,
        start,
    )?;
    repo.add_ledger("Deep:Level 2:Level 3:Sibling", "", D, start)?;
    let unicode = repo.add_ledger("Assets:Café Crème ☕ Fund", "non-ASCII", D, start)?;

    repo.post("Lottery", "", start, cents(987_654_321_09), rich, opening)?;
    repo.post("Opening", "", start, Money::from_major(5_000), long, opening)?;
    repo.post("Opening", "", start, Money::from_major(12), deep, opening)?;
    repo.post("Opening", "", start, cents(3_50), unicode, opening)?;
    repo.post("Overdraft", "", start, cents(2_345_67), opening, overdrawn)?;

    // A hundred and twenty expense ledgers, for the picker's height and the
    // tree's length.
    let mut expenses = Vec::new();
    for cat in 0..12 {
        for sub in 0..10 {
            expenses.push(repo.add_ledger(
                format!("Expenses:Category {cat:02}:Item {sub:02}"),
                "",
                D,
                start,
            )?);
        }
    }
    repo.commit("Open everything")?;

    // Twelve-way split, and a split whose legs are all tiny.
    let mut legs: Vec<Leg> =
        expenses.iter().step_by(10).map(|&e| Leg::debit(e, cents(1_234_56))).collect();
    let total: i64 = legs.iter().map(|l| l.amount.0).sum();
    legs.push(Leg::credit(rich, Money(total)));
    repo.post_split("Twelve-way split", "one leg per category", start.add_days(1), legs)?;
    repo.post_split(
        "Pennies",
        "",
        start.add_days(1),
        vec![
            Leg::debit(expenses[1], cents(1)),
            Leg::debit(expenses[2], cents(1)),
            Leg::debit(expenses[3], cents(1)),
            Leg::credit(overdrawn, cents(3)),
        ],
    )?;
    repo.commit("Splits")?;

    // A crowded calendar: many issuers due on the 1st and 15th, a daily one,
    // a weekly one, and some paused.
    let mut crowded = Vec::new();
    for i in 0..14 {
        let day = if i % 2 == 0 { 1 } else { 15 };
        let uid = repo.add_issuer(
            format!("Bill number {i:02} with a fairly long name"),
            "",
            expenses[i * 7],
            overdrawn,
            rng.money(10, 900),
            Schedule::MonthlyOn { day, every_n_months: 1 },
            start,
        )?;
        crowded.push(uid);
        if i % 5 == 4 {
            repo.stage(Op::SetIssuerPaused { uid, paused: true })?;
        }
    }
    let daily = repo.add_issuer(
        "Coffee",
        "every single day",
        expenses[5],
        unicode,
        cents(4_75),
        Schedule::EveryNDays { n: 1 },
        start,
    )?;
    let weekly = repo.add_issuer(
        "Weekly allowance",
        "",
        deep,
        rich,
        Money::from_major(50),
        Schedule::EveryNDays { n: 7 },
        start,
    )?;
    let big = repo.add_issuer(
        "Annual trust distribution",
        "",
        overdrawn,
        rich,
        cents(12_345_678_90),
        Schedule::MonthlyOn { day: 29, every_n_months: 12 },
        start.add_months(1),
    )?;
    crowded.extend([daily, weekly, big]);
    let everything = repo.add_bucket("Everything", "every root")?;
    for path in ["Assets", "Expenses", "Deep", "Equity"] {
        repo.stage(Op::AddSubtreeToBucket { bucket: everything, path: path.into() })?;
    }
    let cats = repo.add_bucket("Categories", "")?;
    repo.stage(Op::AddSubtreeToBucket { bucket: cats, path: "Expenses".into() })?;
    repo.add_view(
        "Ten years, daily",
        "the longest horizon at the finest period",
        ViewSpec {
            buckets: vec![Term::plus(everything)],
            ledgers: vec![rich, overdrawn, long, deep, unicode],
            issuers: crowded,
            period: Period::Day,
            lookback: Span::Years(2),
            horizon: Span::Years(10),
            ..ViewSpec::default()
        },
    )?;
    repo.commit(
        "A commit message long enough to wrap: it lists the issuers, the buckets \
         and the view, and then keeps going past the point where any sensible column would \
         have clipped it, just to see what the History screen does with it.",
    )?;

    // Weekly commits for two years, with a spray of one-off spending, and a
    // branch every couple of months.
    let mut week = start.add_days(7);
    let mut i = 0;
    while week < today.add_days(-7) {
        for _ in 0..(rng.next() % 12) {
            let e = *rng.pick(&expenses);
            let d = week.add_days((rng.next() % 7) as i32);
            repo.post(
                format!("Purchase {}", rng.next() % 10_000),
                "",
                d,
                rng.money(1, 400),
                e,
                overdrawn,
            )?;
        }
        repo.run_issuers(week)?;
        repo.commit(format!("Week of {week}"))?;
        i += 1;
        if i % 9 == 0 {
            let name = format!("experiment-{:02}-with-a-long-branch-name", i / 9);
            repo.branch(&name, None)?;
        }
        week = week.add_days(7);
    }
    Ok(())
}
