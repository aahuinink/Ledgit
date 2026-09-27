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
fn reverting_a_ledger_creation_explains_why_it_cannot() {
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
fn switching_can_shelve_staged_work_until_you_come_back() {
    let mut f = fixture();
    f.repo.branch("side", None).unwrap();
    f.repo.post("rent", "", d("2024-01-09"), Money::from_major(900), f.cash, f.salary).unwrap();
    let staged = f.repo.staged().to_vec();

    assert_eq!(f.repo.checkout_with("side", StagedWork::Shelve).unwrap(), 0);
    assert!(!f.repo.has_staged_changes(), "the work stays behind on main");
    assert_eq!(f.repo.working().transactions.len(), 0);
    assert_eq!(f.repo.shelved("main").unwrap(), 1);
    // A branch holding shelved work is not thrown away with it.
    assert!(f.repo.delete_branch("main").is_err());

    // Work on the other branch meanwhile, then go back.
    f.repo.post("side entry", "", d("2024-01-10"), Money::from_major(5), f.cash, f.loan).unwrap();
    f.repo.commit("on the side").unwrap();
    assert_eq!(f.repo.checkout_with("main", StagedWork::Refuse).unwrap(), 1);
    assert_eq!(f.repo.staged(), &staged[..], "the shelf comes back as it was");
    assert_eq!(f.repo.shelved("main").unwrap(), 0);
    assert_eq!(f.repo.working().transactions.len(), 1);
}

#[test]
fn switching_can_bring_staged_work_along() {
    let mut f = fixture();
    f.repo.branch("side", None).unwrap();
    f.repo.post("rent", "", d("2024-01-09"), Money::from_major(900), f.cash, f.salary).unwrap();

    assert_eq!(f.repo.checkout_with("side", StagedWork::Bring).unwrap(), 0);
    assert_eq!(f.repo.head().branch_name(), Some("side"));
    assert_eq!(f.repo.staged().len(), 1);
    assert_eq!(f.repo.working().transactions.len(), 1);
    assert_eq!(f.repo.shelved("main").unwrap(), 0);

    // Brought work that does not apply on the new branch is flagged, not lost:
    // an entry posting to a ledger only main has.
    f.repo.checkout_with("main", StagedWork::Shelve).unwrap();
    let only_main =
        f.repo.add_ledger("Only on main", "", Normality::Debit, d("2024-01-01")).unwrap();
    f.repo.commit("main-only ledger").unwrap();
    f.repo.post("uses it", "", d("2024-01-11"), Money::from_major(1), only_main, f.cash).unwrap();
    f.repo.checkout_with("side", StagedWork::Bring).unwrap();
    assert_eq!(f.repo.staged().len(), 2, "brought along, plus side's own shelf");
    assert_eq!(f.repo.broken().len(), 1);
}

#[test]
fn branching_here_takes_the_staged_work_to_the_new_branch() {
    let mut f = fixture();
    f.repo.post("what if", "", d("2024-01-09"), Money::from_major(50), f.cash, f.salary).unwrap();
    f.repo.checkout_new("what-if").unwrap();
    assert_eq!(f.repo.head().branch_name(), Some("what-if"));
    assert_eq!(f.repo.staged().len(), 1);
    assert!(f.repo.broken().is_empty());
}

#[test]
fn shelving_needs_a_branch_to_shelve_on() {
    let mut f = fixture();
    let first = f.repo.head_commit().unwrap().unwrap().to_string();
    f.repo.checkout(&first).unwrap();
    f.repo.post("detached", "", d("2024-01-09"), Money::from_major(1), f.cash, f.salary).unwrap();
    let err = f.repo.checkout_with("main", StagedWork::Shelve).unwrap_err();
    assert!(err.to_string().contains("detached"), "{err}");
    assert_eq!(f.repo.staged().len(), 1, "a refused switch changes nothing");
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
fn unstaging_a_dependency_flags_what_it_breaks_and_blocks_the_commit() {
    let mut f = fixture();
    let scratch = f.repo.add_ledger("Scratch", "", Normality::Debit, d("2024-01-01")).unwrap();
    f.repo
        .post("uses scratch", "", d("2024-01-02"), Money::from_major(5), scratch, f.cash)
        .unwrap();
    f.repo.post("fine", "", d("2024-01-03"), Money::from_major(7), f.cash, f.salary).unwrap();

    // Dropping the ledger is allowed; the entry that used it is kept, flagged.
    f.repo.unstage_at(0).unwrap();
    assert_eq!(f.repo.staged().len(), 2, "nothing else was thrown away");
    assert_eq!(f.repo.broken().len(), 1);
    assert_eq!(f.repo.broken()[0].index, 0);
    assert!(f.repo.broken()[0].reason.contains("ledger"), "{:?}", f.repo.broken());

    // The working budget holds only what applies: the healthy entry.
    let cash = f.repo.working().ledgers.ix(f.cash).unwrap();
    assert_eq!(f.repo.working().ledgers.balance(cash), Money::from_major(7));
    let r = f.repo.report().unwrap();
    assert_eq!(r.lines.len(), 2, "the report still lists the broken change");
    assert_eq!(r.manual_transactions, 1, "but counts only the healthy one");
    assert!(!r.can_commit());

    let err = f.repo.commit("should not go through").unwrap_err();
    assert!(err.to_string().contains("no longer apply"), "{err}");
    assert!(f.repo.head_commit().is_ok());
    assert_eq!(f.repo.staged().len(), 2, "a refused commit keeps the stage");

    // Fixing the entry - pointing it at a ledger that exists - mends it.
    let Op::PostTransaction { uid, name, description, date, parent, .. } =
        f.repo.staged()[0].clone()
    else {
        panic!("expected the transaction first");
    };
    let fixed = Op::PostTransaction {
        uid,
        name,
        description,
        date,
        legs: simple_legs(f.loan, f.cash, Money::from_major(5)),
        parent,
    };
    f.repo.replace_staged(0, fixed).unwrap();
    assert!(f.repo.broken().is_empty());
    assert!(f.repo.report().unwrap().can_commit());
    f.repo.commit("fixed").unwrap();
}

#[test]
fn an_edit_that_does_not_apply_is_refused_and_leaves_the_stage_alone() {
    let mut f = fixture();
    f.repo.post("rent", "", d("2024-01-02"), Money::from_major(5), f.loan, f.cash).unwrap();
    let before = f.repo.staged().to_vec();
    let bad = Op::PostTransaction {
        uid: TxUid::new(),
        name: "rent".into(),
        description: String::new(),
        date: d("2024-01-02"),
        legs: simple_legs(LedgerUid::new(), f.cash, Money::from_major(5)),
        parent: Parent::Manual,
    };
    assert!(f.repo.replace_staged(0, bad).is_err());
    assert_eq!(f.repo.staged(), &before[..]);
    assert!(f.repo.broken().is_empty());
    assert!(f.repo.replace_staged(9, before[0].clone()).is_err());
}

#[test]
fn a_whole_stage_can_be_put_back_even_when_part_of_it_is_broken() {
    let mut f = fixture();
    let scratch = f.repo.add_ledger("Scratch", "", Normality::Debit, d("2024-01-01")).unwrap();
    f.repo
        .post("uses scratch", "", d("2024-01-02"), Money::from_major(5), scratch, f.cash)
        .unwrap();
    let snapshot = f.repo.staged().to_vec();
    f.repo.unstage_at(0).unwrap();
    assert_eq!(f.repo.broken().len(), 1);
    // Undo: put the ledger back and the entry applies again.
    f.repo.set_stage(snapshot).unwrap();
    assert!(f.repo.broken().is_empty());
    assert_eq!(f.repo.staged().len(), 2);
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

/// `combine` is the answer to "can buckets contain other buckets": they cannot,
/// but several can be totalled together at read time.
mod combining_buckets {
    use super::*;

    struct Combo {
        repo: Repo<MemStore>,
        cash: BucketUid,
        owed: BucketUid,
        chequing: LedgerUid,
        savings: LedgerUid,
        receivable: LedgerUid,
    }

    /// Two asset ledgers in a "Cash" bucket, one in "Receivables".
    fn combo() -> Combo {
        let mut repo = Repo::init(MemStore::new(), "tester").unwrap();
        let open = d("2024-01-01");
        let chequing = repo.add_ledger("Chequing", "", Normality::Debit, open).unwrap();
        let savings = repo.add_ledger("Savings", "", Normality::Debit, open).unwrap();
        let receivable = repo.add_ledger("Receivable", "", Normality::Debit, open).unwrap();
        let equity = repo.add_ledger("Opening Balances", "", Normality::Credit, open).unwrap();

        repo.post("Open chequing", "", open, Money::from_major(3_000), chequing, equity).unwrap();
        repo.post("Open savings", "", open, Money::from_major(7_000), savings, equity).unwrap();
        repo.post("Invoice", "", open, Money::from_major(2_000), receivable, equity).unwrap();

        let cash = repo.add_bucket("Cash", "").unwrap();
        let owed = repo.add_bucket("Receivables", "").unwrap();
        for ledger in [chequing, savings] {
            repo.stage(Op::AddToBucket { bucket: cash, ledger }).unwrap();
        }
        repo.stage(Op::AddToBucket { bucket: owed, ledger: receivable }).unwrap();
        repo.commit("set up").unwrap();

        Combo { repo, cash, owed, chequing, savings, receivable }
    }

    fn total(c: &Combo, terms: &[Term]) -> Money {
        combine(c.repo.working(), terms, RollUp::ByNormality, LedgerSort::Name, Order::Asc).total
    }

    #[test]
    fn one_bucket_minus_another() {
        let c = combo();
        // 10,000 in cash less 2,000 still owed to us.
        assert_eq!(total(&c, &[Term::plus(c.cash), Term::minus(c.owed)]), Money::from_major(8_000));
        // Order of the terms does not change the arithmetic.
        assert_eq!(total(&c, &[Term::minus(c.owed), Term::plus(c.cash)]), Money::from_major(8_000));
    }

    #[test]
    fn a_ledger_in_two_added_buckets_is_counted_once() {
        let mut c = combo();
        // "Liquid" overlaps "Cash" on Chequing.
        let liquid = c.repo.add_bucket("Liquid", "").unwrap();
        for ledger in [c.chequing, c.receivable] {
            c.repo.stage(Op::AddToBucket { bucket: liquid, ledger }).unwrap();
        }
        c.repo.commit("overlap").unwrap();

        let combined = combine(
            c.repo.working(),
            &[Term::plus(c.cash), Term::plus(liquid)],
            RollUp::ByNormality,
            LedgerSort::Name,
            Order::Asc,
        );
        // Chequing 3,000 + Savings 7,000 + Receivable 2,000, with Chequing
        // counted once despite being in both buckets.
        assert_eq!(combined.total, Money::from_major(12_000));
        assert_eq!(combined.lines.len(), 3);
        assert!(combined.cancelled.is_empty());
    }

    #[test]
    fn a_ledger_on_both_sides_cancels_and_is_reported() {
        let mut c = combo();
        // Savings is in Cash, and now also in the bucket being subtracted.
        c.repo.stage(Op::AddToBucket { bucket: c.owed, ledger: c.savings }).unwrap();
        c.repo.commit("overlap both ways").unwrap();

        let combined = combine(
            c.repo.working(),
            &[Term::plus(c.cash), Term::minus(c.owed)],
            RollUp::ByNormality,
            LedgerSort::Name,
            Order::Asc,
        );
        // Chequing 3,000 - Receivable 2,000. Savings appears on both sides and
        // contributes nothing rather than being counted twice.
        assert_eq!(combined.total, Money::from_major(1_000));
        assert_eq!(combined.cancelled.len(), 1);
        assert_eq!(combined.cancelled[0].name, "Savings");
        assert_eq!(combined.cancelled[0].contribution, Money(0));
        assert!(combined.lines.iter().all(|l| l.name != "Savings"));
    }

    #[test]
    fn a_single_added_term_agrees_with_rolling_that_bucket_up() {
        let c = combo();
        let l = c.repo.working();
        for roll in [RollUp::ByNormality, RollUp::Sum] {
            let rolled = roll_up(l, c.cash, roll, LedgerSort::Name, Order::Asc).unwrap();
            let combined = combine(l, &[Term::plus(c.cash)], roll, LedgerSort::Name, Order::Asc);
            assert_eq!(rolled.total, combined.total, "{roll:?}");
            assert_eq!(rolled.lines, combined.lines, "{roll:?}");
        }
    }

    #[test]
    fn a_deleted_bucket_is_reported_rather_than_silently_zeroed() {
        let mut c = combo();
        let terms = [Term::plus(c.cash), Term::minus(c.owed)];
        c.repo.stage(Op::DeleteBucket { uid: c.owed }).unwrap();
        c.repo.commit("drop receivables").unwrap();

        let combined =
            combine(c.repo.working(), &terms, RollUp::ByNormality, LedgerSort::Name, Order::Asc);
        // The surviving term still totals, but the caller can say why the
        // number moved instead of quietly showing cash as the whole answer.
        assert_eq!(combined.total, Money::from_major(10_000));
        assert_eq!(combined.missing, vec![c.owed]);
    }

    #[test]
    fn no_terms_is_an_empty_total_not_a_panic() {
        let c = combo();
        let combined =
            combine(c.repo.working(), &[], RollUp::ByNormality, LedgerSort::Name, Order::Asc);
        assert_eq!(combined.total, Money(0));
        assert!(combined.lines.is_empty());
    }
}

// ------------------------------------------------------- cohorts and views

#[test]
fn cohorts_and_views_are_versioned_like_buckets() {
    let mut f = fixture();
    let open = d("2024-01-01");
    let loan_pay = f
        .repo
        .add_issuer(
            "Car payment",
            "",
            f.loan,
            f.cash,
            Money::from_major(400),
            Schedule::EveryNDays { n: 14 },
            open,
        )
        .unwrap();
    let debts = f.repo.add_cohort("Debts", "").unwrap();
    f.repo.stage(Op::AddToCohort { cohort: debts, issuer: loan_pay }).unwrap();
    let net = f.repo.add_bucket("Net Worth", "").unwrap();
    f.repo.stage(Op::AddToBucket { bucket: net, ledger: f.cash }).unwrap();
    let view = f
        .repo
        .add_view(
            "Debt runway",
            "",
            ViewSpec {
                buckets: vec![Term::plus(net)],
                cohorts: vec![debts],
                ..ViewSpec::default()
            },
        )
        .unwrap();

    let report = f.repo.report().unwrap();
    assert_eq!((report.new_cohorts, report.new_views), (1, 1));
    f.repo.commit("track the car loan").unwrap();

    // A branch where the view looks further ahead does not change trunk's.
    f.repo.checkout_new("longer").unwrap();
    let longer =
        ViewSpec { horizon: Span::Years(5), ..f.repo.working().views.get(ix_of(&f, view)).spec };
    f.repo
        .stage(Op::EditView { uid: view, name: None, description: None, spec: Some(longer) })
        .unwrap();
    f.repo.commit("look five years out").unwrap();
    f.repo.checkout(DEFAULT_BRANCH).unwrap();
    assert_eq!(f.repo.working().views.get(ix_of(&f, view)).spec.horizon, Span::Months(12));

    // Deleting the cohort and the view, then reverting, brings both back whole.
    f.repo.stage(Op::DeleteCohort { uid: debts }).unwrap();
    f.repo.stage(Op::DeleteView { uid: view }).unwrap();
    f.repo.commit("tidy up").unwrap();
    assert!(f.repo.working().cohorts.is_empty());
    assert!(f.repo.working().views.is_empty());
    f.repo.revert("HEAD").unwrap();
    f.repo.commit("undo tidy up").unwrap();
    let w = f.repo.working();
    let c = w.cohorts.ix(debts).expect("cohort restored");
    assert_eq!(w.cohorts.get(c, &w.issuers).members, vec![loan_pay]);
    assert_eq!(w.views.get(ix_of(&f, view)).spec.cohorts, vec![debts]);
}

fn ix_of(f: &Fixture, view: ViewUid) -> ledgit_core::id::ViewIx {
    f.repo.working().views.ix(view).expect("view is live")
}

#[test]
fn a_view_simulates_without_staging_anything() {
    let mut f = fixture();
    f.repo
        .post("Opening", "", d("2024-01-01"), Money::from_major(5_000), f.cash, f.salary)
        .unwrap();
    f.repo
        .add_issuer(
            "Car payment",
            "",
            f.loan,
            f.cash,
            Money::from_major(400),
            Schedule::EveryNDays { n: 14 },
            d("2024-01-05"),
        )
        .unwrap();
    f.repo.commit("set up").unwrap();

    let spec = ViewSpec {
        ledgers: vec![f.cash],
        lookback: Span::Days(0),
        horizon: Span::Weeks(4),
        ..ViewSpec::default()
    };
    let r = ledgit_core::view::evaluate(f.repo.working(), &spec, d("2024-01-01"));
    // Jan 5 and Jan 19 fall inside four weeks of Jan 1; Feb 2 does not.
    assert_eq!(r.series[0].at_end, Money::from_major(5_000 - 800));
    assert!(!f.repo.has_staged_changes(), "a simulation is a read");
    assert_eq!(f.repo.working().transactions.len(), 1);
}

#[test]
fn reverting_a_bucket_deletion_restores_its_members() {
    let mut f = fixture();
    let net = f.repo.add_bucket("Net Worth", "").unwrap();
    for a in [f.cash, f.loan] {
        f.repo.stage(Op::AddToBucket { bucket: net, ledger: a }).unwrap();
    }
    f.repo.commit("track net worth").unwrap();
    f.repo.stage(Op::DeleteBucket { uid: net }).unwrap();
    f.repo.commit("drop it").unwrap();

    // The inverse is "create, then add each member" - in that order.
    f.repo.revert("HEAD").unwrap();
    let w = f.repo.working();
    let b = w.buckets.ix(net).expect("bucket restored");
    assert_eq!(w.buckets.get(b, &w.ledgers).members, vec![f.cash, f.loan]);
}

// ------------------------------------------------------------ ledger tree

/// A bucket that includes a subtree keeps including it: a ledger opened
/// under `Wedding` next month joins the Wedding bucket with no further work.
#[test]
fn a_subtree_bucket_picks_up_ledgers_created_later() {
    let mut f = fixture();
    let open = d("2024-01-01");
    let tux = f.repo.add_ledger("Wedding:Tuxedo", "", Normality::Debit, open).unwrap();
    let wedding = f.repo.add_bucket("Wedding", "").unwrap();
    f.repo.stage(Op::AddSubtreeToBucket { bucket: wedding, path: "wedding".into() }).unwrap();
    f.repo.post("Tux rental", "", open, Money::from_major(250), tux, f.cash).unwrap();
    f.repo.commit("start planning").unwrap();

    let total = |r: &Repo<MemStore>| {
        roll_up(r.working(), wedding, RollUp::ByNormality, LedgerSort::Name, Order::Asc)
            .unwrap()
            .total
    };
    assert_eq!(total(&f.repo), Money::from_major(250));

    // A new ledger under the path, typed a little carelessly.
    let venue = f.repo.add_ledger(" Wedding : Venue ", "", Normality::Debit, open).unwrap();
    let w = f.repo.working();
    assert_eq!(w.ledgers.name[w.ledgers.ix(venue).unwrap().get()], "Wedding:Venue");
    f.repo.post("Deposit", "", open, Money::from_major(2_000), venue, f.cash).unwrap();
    assert_eq!(total(&f.repo), Money::from_major(2_250));

    // The commit report says the bucket's membership moved, not just its total.
    let report = f.repo.report().unwrap();
    let effect = report.bucket_effects.iter().find(|b| b.name == "Wedding").unwrap();
    assert!(effect.membership_changed);
    f.repo.commit("book the venue").unwrap();

    // Renaming a ledger out of the subtree takes it out of the bucket.
    f.repo
        .stage(Op::EditLedger {
            uid: venue,
            name: Some("Reception:Venue".into()),
            description: None,
        })
        .unwrap();
    assert_eq!(total(&f.repo), Money::from_major(250));
}

#[test]
fn removing_one_ledger_does_not_override_its_subtree() {
    let mut f = fixture();
    let open = d("2024-01-01");
    let tux = f.repo.add_ledger("Wedding:Tuxedo", "", Normality::Debit, open).unwrap();
    let b = f.repo.add_bucket("Wedding", "").unwrap();
    f.repo.stage(Op::AddToBucket { bucket: b, ledger: tux }).unwrap();
    f.repo.stage(Op::AddSubtreeToBucket { bucket: b, path: "Wedding".into() }).unwrap();
    f.repo.stage(Op::RemoveFromBucket { bucket: b, ledger: tux }).unwrap();
    let w = f.repo.working();
    let bix = w.buckets.ix(b).unwrap().get();
    assert!(w.buckets.explicit[bix].is_empty());
    assert_eq!(w.buckets.members[bix].len(), 1, "still under the subtree");

    f.repo.stage(Op::RemoveSubtreeFromBucket { bucket: b, path: "WEDDING".into() }).unwrap();
    assert!(f.repo.working().buckets.members[bix].is_empty());
}

#[test]
fn reverting_a_subtree_bucket_deletion_restores_the_subtree_not_a_snapshot() {
    let mut f = fixture();
    let open = d("2024-01-01");
    f.repo.add_ledger("Wedding:Tuxedo", "", Normality::Debit, open).unwrap();
    let b = f.repo.add_bucket("Wedding", "").unwrap();
    f.repo.stage(Op::AddSubtreeToBucket { bucket: b, path: "Wedding".into() }).unwrap();
    f.repo.stage(Op::AddToBucket { bucket: b, ledger: f.cash }).unwrap();
    f.repo.commit("track it").unwrap();
    f.repo.stage(Op::DeleteBucket { uid: b }).unwrap();
    f.repo.commit("drop it").unwrap();

    f.repo.revert("HEAD").unwrap();
    let w = f.repo.working();
    let bix = w.buckets.ix(b).expect("restored").get();
    assert_eq!(w.buckets.subtrees[bix], vec!["Wedding".to_string()]);
    assert_eq!(w.buckets.explicit[bix], vec![w.ledgers.ix(f.cash).unwrap()]);
    assert_eq!(w.buckets.members[bix].len(), 2);
}

#[test]
fn renaming_a_subtree_moves_every_ledger_and_keeps_every_posting() {
    let mut f = fixture();
    let open = d("2024-01-01");
    let tux = f.repo.add_ledger("Wedding:Tuxedo", "", Normality::Debit, open).unwrap();
    f.repo.add_ledger("Wedding:Venue", "", Normality::Debit, open).unwrap();
    f.repo.add_ledger("Wedding-fund", "", Normality::Debit, open).unwrap();
    f.repo.post("Tux", "", open, Money::from_major(250), tux, f.cash).unwrap();
    f.repo.commit("plan").unwrap();

    assert_eq!(f.repo.rename_subtree("wedding", "Marriage").unwrap(), 2);
    assert!(f.repo.rename_subtree("Nope", "X").is_err());
    let w = f.repo.working();
    let names: Vec<&str> = w.ledgers.name.iter().map(String::as_str).collect();
    assert!(names.contains(&"Marriage:Tuxedo") && names.contains(&"Marriage:Venue"));
    assert!(names.contains(&"Wedding-fund"), "not under Wedding");
    let tix = w.ledgers.ix(tux).unwrap();
    assert_eq!(w.ledgers.balance(tix), Money::from_major(250));

    // And the tree filters see the new shape.
    let under = LedgerQuery::new().filter(LedgerFilter::Under("marriage".into())).run(w);
    assert_eq!(under.len(), 2);
    let spent = TxQuery::new().filter(TxFilter::TouchesUnder("Marriage".into())).run(w);
    assert_eq!(spent.len(), 1);
}

#[test]
#[allow(clippy::inconsistent_digit_grouping)]
fn variables_are_versioned_and_price_entries_when_they_are_made() {
    use ledgit_core::expr::eval_money;
    let mut f = fixture();
    let set =
        |name: &str, v: &str| Op::SetVariable { name: name.into(), value: VarValue::guess(v) };
    f.repo.stage(set("Car_Km_Rate", "0.68")).unwrap();
    f.repo.stage(set("Home", "Toronto")).unwrap();
    let r = f.repo.report().unwrap();
    assert_eq!(r.variables_changed, 2);
    f.repo.commit("rates").unwrap();

    let vars = &f.repo.working().variables;
    let amount = eval_money("200 * car_km_rate", vars).unwrap();
    assert_eq!(amount, Money(136_00));
    assert_eq!(
        vars.substitute("Mileage from {Home} at {Car_Km_Rate}/km").unwrap(),
        "Mileage from Toronto at 0.68/km"
    );
    assert!(vars.substitute("{Nope}").is_err());
    assert_eq!(vars.substitute("literal {{ brace").unwrap(), "literal { brace");
    assert!(eval_money("2 * Home", vars).unwrap_err().to_string().contains("text"));
    f.repo.post("Mileage", "", d("2024-02-01"), amount, f.loan, f.cash).unwrap();
    f.repo.commit("mileage").unwrap();

    // A new rate re-prices nothing already posted.
    f.repo.stage(set("CAR_KM_RATE", "0.70")).unwrap();
    let raised = f.repo.commit("rate up").unwrap();
    assert_eq!(f.repo.working().variables.len(), 2, "same variable, whatever the case");
    assert_eq!(
        f.repo.working().variables.get("car_km_rate"),
        Some(&VarValue::Number("0.70".into()))
    );
    let loan = f.repo.working().ledgers.ix(f.loan).unwrap();
    assert_eq!(f.repo.working().ledgers.balance(loan), Money(-136_00));

    // Reverting the change puts the old rate back; reverting a delete restores it.
    f.repo.revert(&raised.to_string()).unwrap();
    f.repo.commit("undo the raise").unwrap();
    assert_eq!(
        f.repo.working().variables.get("Car_Km_Rate"),
        Some(&VarValue::Number("0.68".into()))
    );
    f.repo.stage(Op::DeleteVariable { name: "home".into() }).unwrap();
    let deleted = f.repo.commit("no home").unwrap();
    assert!(f.repo.working().variables.get("Home").is_none());
    f.repo.revert(&deleted.to_string()).unwrap();
    assert_eq!(f.repo.working().variables.get("Home"), Some(&VarValue::Text("Toronto".into())));

    // Bad names and bad numbers never land.
    assert!(f.repo.stage(set("Km Rate", "1")).is_err());
    assert!(f.repo.stage(set("2fast", "1")).is_err());
    assert!(f
        .repo
        .stage(Op::SetVariable { name: "X".into(), value: VarValue::Number("abc".into()) })
        .is_err());
    assert!(f.repo.stage(Op::DeleteVariable { name: "Nope".into() }).is_err());
}

#[test]
fn rates_read_and_print_as_percentages() {
    assert_eq!(Rate::parse_percent("6.45").unwrap(), Rate(64_500));
    assert_eq!(Rate::parse_percent(" 5% ").unwrap(), Rate(50_000));
    assert_eq!(Rate::parse_percent("0.0001").unwrap(), Rate(1));
    assert!(Rate::parse_percent("0.00001").is_err(), "finer than a millionth");
    assert!(Rate::parse_percent("abc").is_err());
    assert_eq!(Rate(64_500).to_string(), "6.45%");
    assert_eq!(Rate(50_000).to_string(), "5%");
    assert_eq!(Rate(1_250).to_string(), "0.125%");
}

/// Interest compounds on interest already charged, and reads the balance
/// after a payment another issuer made earlier in the run.
#[test]
#[allow(clippy::inconsistent_digit_grouping)]
fn interest_is_worked_out_in_date_order_across_issuers() {
    use ledgit_core::expr::Ratio;
    let mut f = fixture();
    let interest = f.repo.add_ledger("Interest", "", Normality::Debit, d("2024-01-01")).unwrap();
    f.repo.post("Car", "", d("2024-01-01"), Money::from_major(10_000), f.cash, f.loan).unwrap();
    let monthly = |day| Schedule::MonthlyOn { day, every_n_months: 1 };
    let apr = Rate::parse_percent("6.45").unwrap();
    f.repo
        .add_rule_issuer(
            "Loan interest",
            "",
            interest,
            f.loan,
            AmountRule::Interest { of: f.loan, apr },
            monthly(1),
            d("2024-02-01"),
        )
        .unwrap();
    f.repo
        .add_issuer(
            "Payment",
            "",
            f.loan,
            f.cash,
            Money::from_major(1_000),
            monthly(15),
            d("2024-01-15"),
        )
        .unwrap();
    f.repo.commit("loan").unwrap();

    let runs = f.repo.run_issuers(d("2024-03-01")).unwrap();
    assert_eq!(runs.iter().map(|r| r.dates.len()).sum::<usize>(), 4);

    // By hand: Jan 15 payment leaves 9,000; Feb 1 charges 31 days on it;
    // Feb 15 pays 1,000; Mar 1 charges 29 days (2024 is a leap year).
    let charge = |balance: Money, days: i64| {
        Ratio::from_money(balance)
            .checked_mul(apr.ratio())
            .unwrap()
            .checked_mul(Ratio::int(days))
            .unwrap()
            .checked_div(Ratio::int(365))
            .unwrap()
            .to_money()
            .unwrap()
    };
    let feb = charge(Money::from_major(9_000), 31);
    let mar = charge(Money::from_major(8_000) + feb, 29);
    assert_eq!(feb, Money(49_30));
    let l = f.repo.working();
    let loan = l.ledgers.ix(f.loan).unwrap();
    let interest = l.ledgers.ix(interest).unwrap();
    assert_eq!(l.ledgers.balance(interest), feb + mar);
    assert_eq!(l.ledgers.balance(loan), Money::from_major(8_000) + feb + mar);
    assert!(l.is_balanced());

    // A saved view simulating forward prices each month the same way.
    let spec = ViewSpec { ledgers: vec![f.loan], horizon: Span::Months(3), ..ViewSpec::default() };
    let report = ledgit_core::view::evaluate(l, &spec, d("2024-03-02"));
    let apr_end = report.series[0].at_end;
    let mut expected = Money::from_major(8_000) + feb + mar;
    // Paid on the 15th; charged on the 1st for the month before.
    for days in [31, 30, 31] {
        expected -= Money::from_major(1_000);
        expected += charge(expected, days);
    }
    assert_eq!(apr_end, expected);
}

#[test]
#[allow(clippy::inconsistent_digit_grouping)]
fn a_share_of_a_balance_moves_each_time_and_stops_at_zero() {
    let mut f = fixture();
    let savings = f.repo.add_ledger("Savings", "", Normality::Debit, d("2024-01-01")).unwrap();
    f.repo.post("seed", "", d("2024-01-01"), Money::from_major(1_000), savings, f.salary).unwrap();
    let rate = Rate::parse_percent("5").unwrap();
    let sweep = f
        .repo
        .add_rule_issuer(
            "Sweep",
            "",
            f.cash,
            savings,
            AmountRule::ShareOfBalance { of: savings, rate },
            Schedule::EveryNDays { n: 7 },
            d("2024-01-08"),
        )
        .unwrap();
    // A share of something empty is nothing, and a rule needs a real rate.
    let empty = f.repo.add_ledger("Empty", "", Normality::Debit, d("2024-01-01")).unwrap();
    let idle = f
        .repo
        .add_rule_issuer(
            "Idle",
            "",
            f.cash,
            empty,
            AmountRule::ShareOfBalance { of: empty, rate },
            Schedule::EveryNDays { n: 7 },
            d("2024-01-08"),
        )
        .unwrap();
    assert!(f
        .repo
        .add_rule_issuer(
            "Zero",
            "",
            f.cash,
            empty,
            AmountRule::ShareOfBalance { of: empty, rate: Rate(0) },
            Schedule::Once,
            d("2024-01-08"),
        )
        .is_err());
    assert!(
        f.repo
            .add_rule_issuer(
                "Once",
                "",
                f.cash,
                empty,
                AmountRule::Interest { of: empty, apr: rate },
                Schedule::Once,
                d("2024-01-08"),
            )
            .is_err(),
        "interest needs a period to accrue over"
    );

    f.repo.run_issuers(d("2024-01-22")).unwrap();
    let l = f.repo.working();
    let sav = l.ledgers.ix(savings).unwrap();
    // 1000 -> 950 -> 902.50 -> 857.38 (45.125 rounds to 45.13)
    assert_eq!(l.ledgers.balance(sav), Money(857_37));
    let posted_by = |uid: IssuerUid| {
        (0..l.transactions.len())
            .filter(|i| l.transactions.parent[*i] == Parent::Issuer(uid))
            .count()
    };
    assert_eq!(posted_by(sweep), 3);
    assert_eq!(posted_by(idle), 0, "nothing to take, nothing posted");
    let idle_ix = l.issuers.ix(idle).unwrap();
    assert_eq!(
        l.issuers.emitted_through[idle_ix.get()],
        Some(d("2024-01-22")),
        "but it is caught up"
    );
    assert!(ledgit_core::issuer::estimate(l, l.issuers.ix(sweep).unwrap()) == Money(42_87));
}

#[test]
fn a_rule_scales_a_split_entry_and_keeps_it_balanced() {
    use ledgit_core::issuer::scale_legs;
    let (a, b, c) = (LedgerUid::new(), LedgerUid::new(), LedgerUid::new());
    let legs = vec![
        Leg::debit(a, Money::from_major(2)),
        Leg::debit(b, Money::from_major(1)),
        Leg::credit(c, Money::from_major(3)),
    ];
    let scaled = scale_legs(&legs, Money(100));
    assert_eq!(
        scaled,
        vec![Leg::debit(a, Money(67)), Leg::debit(b, Money(33)), Leg::credit(c, Money(100))]
    );
    assert!(validate_legs(&scaled).is_ok());
}

/// Issuers made before amount rules existed must encode exactly as they did,
/// or every old commit would stop matching its own hash.
#[test]
fn an_issuer_without_a_rule_encodes_as_it_always_did() {
    let op = Op::CreateIssuer {
        uid: IssuerUid::new(),
        name: "Rent".into(),
        description: String::new(),
        legs: simple_legs(LedgerUid::new(), LedgerUid::new(), Money(100)),
        schedule: Schedule::Once,
        start: d("2024-01-01"),
        rule: None,
    };
    let json = serde_json::to_string(&op).unwrap();
    assert!(!json.contains("rule"), "{json}");
    let back: Op = serde_json::from_str(&json).unwrap();
    assert_eq!(back, op);
}

#[test]
fn targets_and_alerts_are_settings_that_travel_with_the_ledger() {
    use ledgit_core::goals;
    let mut f = fixture();
    f.repo.post("Car", "", d("2024-01-01"), Money::from_major(3_000), f.cash, f.loan).unwrap();
    let low = Alert {
        when: AlertWhen::Below,
        level: Money::from_major(2_500),
        message: "Top up chequing".into(),
    };
    f.repo
        .stage(Op::SetLedgerGoals { uid: f.cash, target: None, alerts: vec![low.clone()] })
        .unwrap();
    f.repo
        .stage(Op::SetLedgerGoals { uid: f.loan, target: Some(Money::ZERO), alerts: vec![] })
        .unwrap();
    let goals_commit = f.repo.commit("goals").unwrap();
    assert!(goals::fired(f.repo.working()).is_empty());

    // A payment that takes chequing under its level shows up in the report.
    f.repo.post("Payment", "", d("2024-01-20"), Money::from_major(600), f.loan, f.cash).unwrap();
    let r = f.repo.report().unwrap();
    assert_eq!(r.alerts.len(), 1);
    assert_eq!(r.alerts[0].alert, low);
    assert_eq!(r.alerts[0].balance, Money::from_major(2_400));
    f.repo.commit("paid").unwrap();
    assert_eq!(goals::fired(f.repo.working()).len(), 1);

    // Pay 600 a month from Feb 1: 2,400 left owing is gone by May 1.
    f.repo
        .add_issuer(
            "Loan payment",
            "",
            f.loan,
            f.salary,
            Money::from_major(600),
            Schedule::MonthlyOn { day: 1, every_n_months: 1 },
            d("2024-02-01"),
        )
        .unwrap();
    let spec =
        ViewSpec { ledgers: vec![f.loan, f.cash], horizon: Span::Months(6), ..ViewSpec::default() };
    let report = ledgit_core::view::evaluate(f.repo.working(), &spec, d("2024-01-25"));
    let loan = report.series.iter().find(|s| s.label == "Car Loan").unwrap();
    assert_eq!(loan.target, Some(Money::ZERO));
    assert_eq!(loan.target_reached, Some(d("2024-05-01")));
    let cash = report.series.iter().find(|s| s.label == "Cash").unwrap();
    assert_eq!(cash.target, None, "chequing has alerts but no target");

    // A bucket adds up the targets its members have, the way it adds balances.
    let debt = f.repo.add_bucket("Debt", "").unwrap();
    for ledger in [f.loan, f.cash] {
        f.repo.stage(Op::AddToBucket { bucket: debt, ledger }).unwrap();
    }
    let t = goals::bucket_targets(f.repo.working(), debt, RollUp::Sum).unwrap();
    assert_eq!((t.members, t.lines.len()), (2, 1), "only the loan has a target");
    assert_eq!(t.balance, Money::from_major(2_400));
    assert_eq!(t.target, Money::ZERO);
    assert_eq!(t.remaining(), Money::from_major(2_400));

    // Reverting the goals commit takes them off again.
    f.repo.clear_stage().unwrap();
    f.repo.revert(&goals_commit.to_string()).unwrap();
    f.repo.commit("no goals").unwrap();
    let l = f.repo.working();
    assert!(l.ledgers.target.iter().all(Option::is_none));
    assert!(l.ledgers.alerts.iter().all(Vec::is_empty));
}

/// "How does a lump-sum payment change when the loan is paid off?" - the
/// same view read off the commit before it and the budget after.
#[test]
fn a_view_compares_against_an_earlier_commit() {
    let mut f = fixture();
    f.repo.post("Car", "", d("2024-01-01"), Money::from_major(6_000), f.cash, f.loan).unwrap();
    f.repo
        .stage(Op::SetLedgerGoals { uid: f.loan, target: Some(Money::ZERO), alerts: vec![] })
        .unwrap();
    f.repo
        .add_issuer(
            "Payment",
            "",
            f.loan,
            f.salary,
            Money::from_major(500),
            Schedule::MonthlyOn { day: 1, every_n_months: 1 },
            d("2024-02-01"),
        )
        .unwrap();
    let before = f.repo.commit("loan and plan").unwrap();

    f.repo
        .post("Lump sum", "", d("2024-01-20"), Money::from_major(2_000), f.loan, f.salary)
        .unwrap();
    f.repo.commit("bonus goes on the loan").unwrap();

    let spec = ViewSpec { ledgers: vec![f.loan], horizon: Span::Years(2), ..ViewSpec::default() };
    let today = d("2024-01-25");
    let now_budget = f.repo.working().clone();
    let now = ledgit_core::view::evaluate(&now_budget, &spec, today);
    let then_budget = f.repo.budget_at(before).unwrap();
    let (then, pairs) = ledgit_core::view::compare(&then_budget, &spec, &now_budget, &now);

    assert_eq!(pairs, vec![Some(0)]);
    assert_eq!((then.start, then.end, then.today), (now.start, now.end, now.today));
    // 6,000 at 500 a month from Feb 1 takes twelve payments; 4,000 takes eight.
    assert_eq!(then.series[0].target_reached, Some(d("2025-01-01")));
    assert_eq!(now.series[0].target_reached, Some(d("2024-09-01")));
    assert_eq!(then.series[0].now - now.series[0].now, Money::from_major(2_000));

    // A ledger that did not exist then has no earlier line.
    let extra = f.repo.add_ledger("New", "", Normality::Debit, d("2024-01-01")).unwrap();
    let spec = ViewSpec { ledgers: vec![f.loan, extra], ..spec };
    let now_budget = f.repo.working().clone();
    let now = ledgit_core::view::evaluate(&now_budget, &spec, today);
    let (_, pairs) = ledgit_core::view::compare(&then_budget, &spec, &now_budget, &now);
    // Total, the loan, and the new ledger: only the last is missing then.
    assert_eq!(pairs.len(), 3);
    assert_eq!(pairs.iter().filter(|p| p.is_none()).count(), 1);
}

#[test]
fn the_graph_holds_every_branch_children_before_parents() {
    let mut f = fixture();
    let root = f.repo.head_commit().unwrap().unwrap();
    f.repo.checkout_new("side").unwrap();
    f.repo.post("side 1", "", d("2024-01-09"), Money::from_major(1), f.cash, f.salary).unwrap();
    let side = f.repo.commit("side 1").unwrap();
    f.repo.checkout("main").unwrap();
    f.repo.post("main 1", "", d("2024-01-10"), Money::from_major(2), f.cash, f.salary).unwrap();
    let main = f.repo.commit("main 1").unwrap();

    let g = f.repo.graph(None).unwrap();
    let ids: Vec<CommitId> = g.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), 3, "both branches and their shared root");
    assert!(ids.contains(&side) && ids.contains(&main));
    assert_eq!(*ids.last().unwrap(), root, "the root comes after everything built on it");
    for (i, c) in g.iter().enumerate() {
        for p in &c.parents {
            let at = ids.iter().position(|x| x == p).unwrap();
            assert!(at > i, "{} is listed before its child", p.short());
        }
    }
    assert_eq!(f.repo.graph(Some(2)).unwrap().len(), 2);

    // Rebasing side leaves its original commit unreachable, and out.
    f.repo.checkout("side").unwrap();
    f.repo.rebase("side", "main").unwrap();
    let g = f.repo.graph(None).unwrap();
    assert!(!g.iter().any(|c| c.id == side), "the pre-rebase commit is gone");
    assert_eq!(g.len(), 3, "root, main 1, and side 1 replayed on top");
}
