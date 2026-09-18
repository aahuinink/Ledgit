//! End-to-end behaviour of the version-controlled budget.
//!
//! These are the tests that would catch a real regression: they drive `Repo`
//! the way a front end does and assert on money, not on internals.

use ledgit_core::issuer;
use ledgit_core::prelude::*;

fn d(s: &str) -> Date {
    s.parse().unwrap()
}

struct Fixture {
    repo: Repo<MemStore>,
    cash: LedgerUid,
    loan: LedgerUid,
    salary: LedgerUid,
}

/// A small but realistic budget: one asset, one liability, one income ledger.
fn fixture() -> Fixture {
    let mut repo = Repo::init(MemStore::new(), "tester").unwrap();
    let open = d("2024-01-01");
    let cash = repo.add_ledger("Cash", "Chequing", Normality::Debit, open).unwrap();
    let loan = repo.add_ledger("Car Loan", "", Normality::Credit, open).unwrap();
    let salary = repo.add_ledger("Salary", "", Normality::Credit, open).unwrap();
    repo.commit("open the books").unwrap();
    Fixture { repo, cash, loan, salary }
}

#[test]
fn nothing_is_history_until_commit() {
    let mut f = fixture();
    f.repo
        .post("Paycheque", "", d("2024-01-05"), Money::from_major(2_000), f.cash, f.salary)
        .unwrap();

    // The working view moves immediately; the committed view does not.
    let cash_ix = f.repo.working().ledgers.ix(f.cash).unwrap();
    assert_eq!(f.repo.working().ledgers.balance(cash_ix), Money::from_major(2_000));
    assert_eq!(f.repo.committed().ledgers.balance(cash_ix), Money::ZERO);
    assert_eq!(f.repo.log(None).unwrap().len(), 1);

    f.repo.commit("january pay").unwrap();
    assert_eq!(f.repo.committed().ledgers.balance(cash_ix), Money::from_major(2_000));
    assert_eq!(f.repo.log(None).unwrap().len(), 2);
    assert!(!f.repo.has_staged_changes());
}

#[test]
fn the_precommit_report_names_every_affected_bucket() {
    let mut f = fixture();
    let net = f.repo.add_bucket("Net Worth", "everything that counts").unwrap();
    for a in [f.cash, f.loan] {
        f.repo.stage(Op::AddToBucket { bucket: net, ledger: a }).unwrap();
    }
    f.repo.commit("track net worth").unwrap();

    // Borrow $10,000 against the car, then spend $400 of it.
    f.repo
        .post("Car loan drawdown", "", d("2024-01-10"), Money::from_major(10_000), f.cash, f.loan)
        .unwrap();
    f.repo
        .post("Car payment", "", d("2024-01-20"), Money::from_major(400), f.loan, f.cash)
        .unwrap();

    let r = f.repo.report().unwrap();
    assert_eq!(r.manual_transactions, 2);
    assert_eq!(r.issuer_transactions, 0);
    assert_eq!(r.total_manual, Money::from_major(10_400));
    assert!(r.balanced);

    let bucket = r.bucket_effects.iter().find(|b| b.name == "Net Worth").unwrap();
    // Borrowing is net-worth neutral; paying $400 off the loan is too (cash
    // down 400, debt down 400). The bucket should not move at all.
    assert_eq!(bucket.before, Money::ZERO);
    assert_eq!(bucket.after, Money::ZERO);

    let cash_delta = r.ledger_deltas.iter().find(|a| a.name == "Cash").unwrap();
    assert_eq!(cash_delta.before, Money::ZERO);
    assert_eq!(cash_delta.after, Money::from_major(9_600));
    assert_eq!(cash_delta.postings, 2);
}

#[test]
fn net_worth_rolls_up_by_normality() {
    let mut f = fixture();
    let net = f.repo.add_bucket("Net Worth", "").unwrap();
    for a in [f.cash, f.loan] {
        f.repo.stage(Op::AddToBucket { bucket: net, ledger: a }).unwrap();
    }
    // $5,000 in the bank, $12,000 owed on the car.
    f.repo
        .post("Opening cash", "", d("2024-01-02"), Money::from_major(5_000), f.cash, f.salary)
        .unwrap();
    f.repo
        .post("Car purchase", "", d("2024-01-03"), Money::from_major(12_000), f.cash, f.loan)
        .unwrap();
    f.repo.commit("set up").unwrap();

    let l = f.repo.working();
    let roll = roll_up(l, net, RollUp::ByNormality, LedgerSort::Name, Order::Asc).unwrap();
    // Cash 17,000 - debt 12,000.
    assert_eq!(roll.total, Money::from_major(5_000));

    let plain = roll_up(l, net, RollUp::Sum, LedgerSort::Name, Order::Asc).unwrap();
    // Sum of presented balances: 17,000 + 12,000 owed, which is *not* net
    // worth - that is exactly why the choice exists.
    assert_eq!(plain.total, Money::from_major(29_000));
}

#[test]
fn reverting_a_commit_posts_a_mirror_entry_and_deletes_nothing() {
    let mut f = fixture();
    f.repo
        .post("Oops, wrong ledger", "", d("2024-02-01"), Money::from_major(250), f.cash, f.loan)
        .unwrap();
    let bad = f.repo.commit("mistake").unwrap();

    let cash_ix = f.repo.working().ledgers.ix(f.cash).unwrap();
    assert_eq!(f.repo.working().ledgers.balance(cash_ix), Money::from_major(250));

    let notes = f.repo.revert(&bad.to_string()).unwrap();
    assert!(notes.is_empty(), "a plain transaction revert needs no caveats: {notes:?}");
    f.repo.commit("undo the mistake").unwrap();

    // Balance is back to zero...
    assert_eq!(f.repo.working().ledgers.balance(cash_ix), Money::ZERO);
    // ...and both facts survive in the register. Nothing was deleted.
    assert_eq!(f.repo.working().transactions.len(), 2);
    assert!(f.repo.working().transactions.name.iter().any(|n| n.starts_with("Reversal of")));
    assert!(f.repo.working().is_balanced());
}

#[test]
fn reverting_an_ledger_creation_explains_why_it_cannot() {
    let mut f = fixture();
    f.repo.add_ledger("Typo Acount", "", Normality::Debit, d("2024-03-01")).unwrap();
    let c = f.repo.commit("add a ledger with a typo").unwrap();

    let notes = f.repo.revert(&c.to_string()).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("ledgers are permanent"), "{}", notes[0]);
    assert!(!f.repo.has_staged_changes());
    assert_eq!(f.repo.working().ledgers.len(), 4);
}

#[test]
fn branches_are_independent_what_if_budgets() {
    let mut f = fixture();
    f.repo
        .post("Paycheque", "", d("2024-01-05"), Money::from_major(2_000), f.cash, f.salary)
        .unwrap();
    f.repo.commit("january pay").unwrap();

    f.repo.checkout_new("what-if-new-car").unwrap();
    f.repo.post("New car", "", d("2024-02-01"), Money::from_major(30_000), f.cash, f.loan).unwrap();
    f.repo.commit("buy the car").unwrap();

    let cash_ix = f.repo.working().ledgers.ix(f.cash).unwrap();
    assert_eq!(f.repo.working().ledgers.balance(cash_ix), Money::from_major(32_000));

    f.repo.checkout("main").unwrap();
    let cash_ix = f.repo.working().ledgers.ix(f.cash).unwrap();
    assert_eq!(f.repo.working().ledgers.balance(cash_ix), Money::from_major(2_000));
    assert_eq!(f.repo.working().transactions.len(), 1);
}

#[test]
fn checkout_refuses_to_carry_staged_work_across_branches() {
    let mut f = fixture();
    f.repo.commit_if_needed();
    f.repo.branch("side", None).unwrap();
    f.repo
        .post("half-typed entry", "", d("2024-01-09"), Money::from_major(10), f.cash, f.salary)
        .unwrap();

    let err = f.repo.checkout("side").unwrap_err();
    assert!(err.to_string().contains("staged changes"), "{err}");
}

#[test]
fn rebase_replays_a_branch_onto_a_moved_trunk() {
    let mut f = fixture();
    f.repo.branch("side", None).unwrap();

    // main moves on.
    f.repo
        .post("Paycheque", "", d("2024-01-05"), Money::from_major(2_000), f.cash, f.salary)
        .unwrap();
    f.repo.commit("january pay").unwrap();

    // side does its own thing from the old base.
    f.repo.checkout("side").unwrap();
    f.repo.post("Groceries", "", d("2024-01-06"), Money::from_major(150), f.loan, f.cash).unwrap();
    f.repo.commit("groceries on the card").unwrap();
    assert_eq!(f.repo.working().transactions.len(), 1, "side cannot see main's pay yet");

    let replayed = f.repo.rebase("side", "main").unwrap();
    assert_eq!(replayed, 1);

    // Now side contains both, in history order.
    assert_eq!(f.repo.working().transactions.len(), 2);
    let log = f.repo.log(None).unwrap();
    let messages: Vec<&str> = log.iter().map(|c| c.summary()).collect();
    assert_eq!(messages, vec!["groceries on the card", "january pay", "open the books"]);

    let cash_ix = f.repo.working().ledgers.ix(f.cash).unwrap();
    assert_eq!(f.repo.working().ledgers.balance(cash_ix), Money::from_major(1_850));
}

#[test]
fn rebase_fast_forwards_instead_of_duplicating() {
    let mut f = fixture();
    f.repo.branch("behind", None).unwrap();
    f.repo
        .post("Paycheque", "", d("2024-01-05"), Money::from_major(2_000), f.cash, f.salary)
        .unwrap();
    f.repo.commit("january pay").unwrap();

    assert_eq!(f.repo.rebase("behind", "main").unwrap(), 0);
    let branches = f.repo.branches().unwrap();
    let behind = branches.iter().find(|(n, _)| n == "behind").unwrap().1;
    let main = branches.iter().find(|(n, _)| n == "main").unwrap().1;
    assert_eq!(behind, main);
}

#[test]
fn issuers_stage_their_work_for_review_before_it_is_committed() {
    let mut f = fixture();
    f.repo
        .add_issuer(
            "Car payment",
            "biweekly",
            f.loan,
            f.cash,
            Money::from_major(400),
            Schedule::EveryNDays { n: 14 },
            d("2024-01-05"),
        )
        .unwrap();
    f.repo.commit("set up the car payment").unwrap();

    let runs = f.repo.run_issuers(d("2024-02-05")).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].dates.len(), 3);

    // Staged, reviewable, not yet history.
    let r = f.repo.report().unwrap();
    assert_eq!(r.issuer_transactions, 3);
    assert_eq!(r.manual_transactions, 0);
    assert_eq!(r.total_issued, Money::from_major(1_200));
    assert_eq!(f.repo.committed().transactions.len(), 0);

    f.repo.commit("post january's car payments").unwrap();
    assert_eq!(f.repo.committed().transactions.len(), 3);

    // Running again over the same window is a no-op: no double posting.
    assert!(f.repo.run_issuers(d("2024-02-05")).unwrap().is_empty());
    assert!(!f.repo.has_staged_changes());
}

#[test]
fn pausing_an_issuer_stops_it_without_erasing_its_past() {
    let mut f = fixture();
    let issuer_uid = f
        .repo
        .add_issuer(
            "Gym",
            "",
            f.loan,
            f.cash,
            Money::from_major(50),
            Schedule::MonthlyOn { day: 1, every_n_months: 1 },
            d("2024-01-01"),
        )
        .unwrap();
    f.repo.run_issuers(d("2024-03-15")).unwrap();
    f.repo.commit("three months of gym").unwrap();
    assert_eq!(f.repo.committed().transactions.len(), 3);

    f.repo.stage(Op::SetIssuerPaused { uid: issuer_uid, paused: true }).unwrap();
    f.repo.commit("cancelled the gym").unwrap();

    assert!(f.repo.run_issuers(d("2025-01-01")).unwrap().is_empty());
    assert_eq!(f.repo.working().transactions.len(), 3, "history is untouched");

    let ix = f.repo.working().issuers.ix(issuer_uid).unwrap();
    assert_eq!(issuer::next_due(f.repo.working(), ix), Some(d("2024-04-01")));
}

#[test]
fn queries_read_the_working_view_not_the_committed_one() {
    let mut f = fixture();
    f.repo
        .post("Coffee", "the good stuff", d("2024-01-08"), Money::from_major(6), f.loan, f.cash)
        .unwrap();

    let hits = search(f.repo.working(), "coffee", 10);
    assert_eq!(hits.transactions.len(), 1);
    assert_eq!(search(f.repo.committed(), "coffee", 10).transactions.len(), 0);

    let rows = TxQuery::new()
        .filter(TxFilter::Touches(f.cash))
        .filter(TxFilter::OnOrAfter(d("2024-01-01")))
        .run(f.repo.working());
    assert_eq!(rows.len(), 1);
}

#[test]
fn balance_as_of_ignores_later_transactions() {
    let mut f = fixture();
    f.repo
        .post("Jan pay", "", d("2024-01-31"), Money::from_major(2_000), f.cash, f.salary)
        .unwrap();
    f.repo
        .post("Feb pay", "", d("2024-02-29"), Money::from_major(2_000), f.cash, f.salary)
        .unwrap();
    f.repo.commit("two months").unwrap();

    let l = f.repo.working();
    let cash = l.ledgers.ix(f.cash).unwrap();
    assert_eq!(balance_as_of(l, cash, d("2024-02-01")), Money::from_major(2_000));
    assert_eq!(balance_as_of(l, cash, d("2024-03-01")), Money::from_major(4_000));
    assert_eq!(register(l, cash).len(), 2);
}

#[test]
fn unstaging_a_dependency_is_refused_rather_than_corrupting_the_stage() {
    let mut f = fixture();
    let scratch = f.repo.add_ledger("Scratch", "", Normality::Debit, d("2024-01-01")).unwrap();
    f.repo
        .post("uses scratch", "", d("2024-01-02"), Money::from_major(5), scratch, f.cash)
        .unwrap();

    let err = f.repo.unstage_at(0).unwrap_err();
    assert!(err.to_string().contains("depends on it"), "{err}");
    assert_eq!(f.repo.staged().len(), 2, "the stage is intact after the refusal");

    // Dropping them newest-first works.
    f.repo.unstage_last().unwrap();
    f.repo.unstage_last().unwrap();
    assert_eq!(f.repo.staged().len(), 0);
}

#[test]
fn a_paycheque_is_one_split_entry() {
    let mut f = fixture();
    let open = d("2024-01-01");
    let tax = f.repo.add_ledger("Tax withheld", "", Normality::Debit, open).unwrap();
    let pension = f.repo.add_ledger("Pension", "", Normality::Debit, open).unwrap();

    f.repo
        .post_split(
            "Paycheque",
            "january, gross",
            d("2024-01-31"),
            vec![
                Leg::debit(f.cash, Money::from_major(1_800)),
                Leg::debit(tax, Money::from_major(500)),
                Leg::debit(pension, Money::from_major(100)),
                Leg::credit(f.salary, Money::from_major(2_400)),
            ],
        )
        .unwrap();

    let r = f.repo.report().unwrap();
    assert_eq!(r.manual_transactions, 1);
    assert_eq!(r.split_transactions, 1);
    // The report sizes the entry by what it debits, not by every leg.
    assert_eq!(r.total_manual, Money::from_major(2_400));
    assert!(r.balanced);
    // Three new ledgers are named plus the one that already existed.
    assert_eq!(r.ledger_deltas.len(), 4);

    f.repo.commit("january pay").unwrap();
    let l = f.repo.working();
    assert_eq!(l.transactions.len(), 1, "one entry, not four");
    assert_eq!(l.postings.len(), 4);
    assert_eq!(l.ledgers.balance(l.ledgers.ix(f.cash).unwrap()), Money::from_major(1_800));
    assert_eq!(l.ledgers.balance(l.ledgers.ix(tax).unwrap()), Money::from_major(500));
    assert_eq!(l.ledgers.balance(l.ledgers.ix(f.salary).unwrap()), Money::from_major(2_400));
    assert!(l.is_balanced());
}

#[test]
fn reverting_a_split_mirrors_every_leg() {
    let mut f = fixture();
    let tax = f.repo.add_ledger("Tax withheld", "", Normality::Debit, d("2024-01-01")).unwrap();
    f.repo.commit("open the tax ledger").unwrap();
    f.repo
        .post_split(
            "Paycheque with the wrong tax",
            "",
            d("2024-01-31"),
            vec![
                Leg::debit(f.cash, Money::from_major(1_900)),
                Leg::debit(tax, Money::from_major(500)),
                Leg::credit(f.salary, Money::from_major(2_400)),
            ],
        )
        .unwrap();
    let bad = f.repo.commit("a wrong paycheque").unwrap();

    let notes = f.repo.revert(&bad.to_string()).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    f.repo.commit("undo it").unwrap();

    let l = f.repo.working();
    // Every ledger is back to zero, and nothing was deleted.
    for uid in [f.cash, tax, f.salary] {
        assert_eq!(l.ledgers.balance(l.ledgers.ix(uid).unwrap()), Money::ZERO);
    }
    assert_eq!(l.transactions.len(), 2);
    assert_eq!(l.postings.len(), 6, "the reversal mirrors all three legs");
    assert!(l.is_balanced());
}

#[test]
fn an_issuer_can_post_a_split_every_time_it_fires() {
    let mut f = fixture();
    let tax = f.repo.add_ledger("Tax withheld", "", Normality::Debit, d("2024-01-01")).unwrap();
    f.repo
        .add_issuer_split(
            "Salary",
            "biweekly, after tax",
            vec![
                Leg::debit(f.cash, Money::from_major(900)),
                Leg::debit(tax, Money::from_major(300)),
                Leg::credit(f.salary, Money::from_major(1_200)),
            ],
            Schedule::EveryNDays { n: 14 },
            d("2024-01-05"),
        )
        .unwrap();
    f.repo.commit("set up the paycheque").unwrap();

    let runs = f.repo.run_issuers(d("2024-02-05")).unwrap();
    assert_eq!(runs[0].dates.len(), 3);

    let r = f.repo.report().unwrap();
    assert_eq!(r.issuer_transactions, 3);
    assert_eq!(r.split_transactions, 3);
    assert_eq!(r.total_issued, Money::from_major(3_600));

    f.repo.commit("three paycheques").unwrap();
    let l = f.repo.working();
    assert_eq!(l.transactions.len(), 3);
    assert_eq!(l.postings.len(), 9);
    assert_eq!(l.ledgers.balance(l.ledgers.ix(tax).unwrap()), Money::from_major(900));
    assert!(l.is_balanced());
}

#[test]
fn queries_see_each_leg_of_a_split() {
    let mut f = fixture();
    let tax = f.repo.add_ledger("Tax withheld", "", Normality::Debit, d("2024-01-01")).unwrap();
    f.repo
        .post_split(
            "Paycheque",
            "",
            d("2024-01-31"),
            vec![
                Leg::debit(f.cash, Money::from_major(1_800)),
                Leg::debit(tax, Money::from_major(600)),
                Leg::credit(f.salary, Money::from_major(2_400)),
            ],
        )
        .unwrap();
    f.repo.post("Groceries", "", d("2024-02-01"), Money::from_major(150), f.loan, f.cash).unwrap();
    let l = f.repo.working();

    // The entry is found from any of its three sides, exactly once each.
    for uid in [f.cash, tax, f.salary] {
        let rows = TxQuery::new()
            .filter(TxFilter::Touches(uid))
            .filter(TxFilter::Text("Paycheque".into()))
            .run(l);
        assert_eq!(rows.len(), 1, "looking from {uid:?}");
    }

    // Direction matters: salary is credited by it, never debited.
    assert_eq!(TxQuery::new().filter(TxFilter::CreditedFrom(f.salary)).run(l).len(), 1);
    assert_eq!(TxQuery::new().filter(TxFilter::DebitedTo(f.salary)).run(l).len(), 0);
    assert_eq!(TxQuery::new().filter(TxFilter::DebitedTo(tax)).run(l).len(), 1);

    // Splits are findable as a class.
    assert_eq!(TxQuery::new().filter(TxFilter::IsSplit(true)).run(l).len(), 1);
    assert_eq!(TxQuery::new().filter(TxFilter::IsSplit(false)).run(l).len(), 1);

    // Amount filters size the entry by what it debits.
    assert_eq!(
        TxQuery::new().filter(TxFilter::AmountAtLeast(Money::from_major(2_000))).run(l).len(),
        1
    );

    // The register shows this ledger's share, not the whole entry.
    let cash_ix = l.ledgers.ix(f.cash).unwrap();
    let rows = register(l, cash_ix);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].change, Money::from_major(1_800));
    assert_eq!(l.counterparties(rows[0].transaction, cash_ix).len(), 2);
    // From chequing's side of a paycheque, the useful counterparty is the one
    // the money came from, not the other ledgers it was split across.
    let other = l.other_side(rows[0].transaction, cash_ix);
    assert_eq!(other.len(), 1);
    assert_eq!(l.ledgers.uid[other[0].get()], f.salary);
}

/// Small helper so tests that only need a clean tree read clearly.
trait CommitIfNeeded {
    fn commit_if_needed(&mut self);
}

impl CommitIfNeeded for Repo<MemStore> {
    fn commit_if_needed(&mut self) {
        if self.has_staged_changes() {
            self.commit("wip").unwrap();
        }
    }
}
