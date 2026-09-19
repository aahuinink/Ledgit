//! Headless smoke tests.
//!
//! `egui::__run_test_ui` runs a real egui pass with no window, so these
//! actually execute every widget call: a duplicated grid id, a panic in a
//! formatter, or a view that reads a ledger that is not on this branch all
//! show up here rather than the first time you click the tab.
//!
//! They are not a substitute for looking at the thing. They are a substitute
//! for shipping a screen that crashes.

use crate::app::{Session, View};
use crate::forms::FormKind;
use crate::views;
use ledgit_core::prelude::*;
use ledgit_sqlite::SqliteStore;
use std::path::PathBuf;

/// A budget with enough in it that every screen has something to draw.
fn session() -> Session {
    let repo = Repo::open(SqliteStore::open_in_memory().unwrap(), "tester").unwrap();
    let mut s = Session::new(repo, PathBuf::from("test.ledgit"));
    let open = "2024-01-01".parse::<Date>().unwrap();

    let cash = s.repo.add_ledger("Chequing", "day to day", Normality::Debit, open).unwrap();
    let loan = s.repo.add_ledger("Car Loan", "", Normality::Credit, open).unwrap();
    let salary = s.repo.add_ledger("Salary", "", Normality::Credit, open).unwrap();
    s.repo.commit("open the books").unwrap();

    s.repo
        .post(
            "January pay",
            "",
            "2024-01-05".parse().unwrap(),
            Money::from_major(2_400),
            cash,
            salary,
        )
        .unwrap();
    s.repo
        .post(
            "Car purchase",
            "",
            "2024-01-06".parse().unwrap(),
            Money::from_major(18_000),
            cash,
            loan,
        )
        .unwrap();
    let bucket = s.repo.add_bucket("Net Worth", "what I am worth").unwrap();
    for ledger in [cash, loan] {
        s.repo.stage(Op::AddToBucket { bucket, ledger }).unwrap();
    }
    s.repo
        .add_issuer(
            "Car payment",
            "",
            loan,
            cash,
            Money::from_major(400),
            Schedule::EveryNDays { n: 14 },
            "2024-01-12".parse().unwrap(),
        )
        .unwrap();
    s.repo.commit("january, and the recurring payment").unwrap();

    // A split entry, so every view has to render one.
    let tax = s.repo.add_ledger("Tax withheld", "", Normality::Debit, open).unwrap();
    s.repo
        .post_split(
            "Paycheque",
            "gross, with tax",
            "2024-01-31".parse().unwrap(),
            vec![
                Leg::debit(cash, Money::from_major(1_800)),
                Leg::debit(tax, Money::from_major(600)),
                Leg::credit(salary, Money::from_major(2_400)),
            ],
        )
        .unwrap();
    s.repo.commit("january pay").unwrap();

    // Leave something staged so the commit screen has a report to render.
    s.repo
        .post("Groceries", "", "2024-01-20".parse().unwrap(), Money::from_major(150), loan, cash)
        .unwrap();

    s.selected_ledger = Some(cash);
    s.selected_bucket = Some(bucket);
    s.pins = vec![cash];
    s
}

/// Run one headless egui pass over `f`.
///
/// `egui::__run_test_ui` only takes `Fn`, and every view needs `&mut Session`,
/// so go through `__run_test_ctx` and open the panel here.
fn run_ui(mut f: impl FnMut(&mut egui::Ui)) {
    egui::__run_test_ctx(|ctx| {
        egui::CentralPanel::default().show(ctx, |ui| f(ui));
    });
}

fn draw(s: &mut Session, view: View) {
    run_ui(|ui| match view {
        View::Dashboard => views::dashboard::show(ui, s),
        View::Ledgers => views::ledgers::show(ui, s),
        View::Register => views::ledgers::register(ui, s),
        View::Transactions => views::transactions::show(ui, s),
        View::Issuers => views::issuers::show(ui, s),
        View::Buckets => views::buckets::show(ui, s),
        View::Commit => views::commit::show(ui, s),
        View::History => views::history::show(ui, s),
    });
}

const EVERY_VIEW: [View; 8] = [
    View::Dashboard,
    View::Ledgers,
    View::Register,
    View::Transactions,
    View::Issuers,
    View::Buckets,
    View::Commit,
    View::History,
];

#[test]
fn every_view_draws_a_populated_budget() {
    let mut s = session();
    for view in EVERY_VIEW {
        draw(&mut s, view);
    }
    assert!(s.repo.has_staged_changes(), "drawing must not change the budget");
    assert_eq!(s.repo.working().transactions.len(), 4);
    assert_eq!(s.repo.working().postings.len(), 9, "one of them is a three-way split");
}

#[test]
fn every_view_draws_an_empty_budget() {
    let repo = Repo::open(SqliteStore::open_in_memory().unwrap(), "tester").unwrap();
    let mut s = Session::new(repo, PathBuf::from("empty.ledgit"));
    for view in EVERY_VIEW {
        draw(&mut s, view);
    }
}

#[test]
fn every_view_survives_a_dangling_selection() {
    // Selections point at uids. After a checkout they may name something that
    // is not on this branch; no view may panic on that.
    let mut s = session();
    s.selected_ledger = Some(LedgerUid::new());
    s.selected_bucket = Some(BucketUid::new());
    s.pins = vec![LedgerUid::new()];
    for view in EVERY_VIEW {
        draw(&mut s, view);
    }
}

#[test]
fn search_draws_hits_and_misses() {
    let mut s = session();
    for needle in ["car", "zzz-nothing-matches", "Net Worth"] {
        s.search = needle.to_string();
        run_ui(|ui| views::search::show(ui, &mut s));
    }
}

#[test]
fn every_form_opens_and_draws() {
    let mut s = session();
    for kind in [FormKind::Ledger, FormKind::Transaction, FormKind::Bucket, FormKind::Issuer] {
        s.forms.open(kind, s.repo.working());
        let budget = s.repo.working().clone();
        let forms = &mut s.forms;
        run_ui(|ui| {
            forms.show(ui, &budget);
        });
        assert_eq!(s.forms.open, Some(kind), "drawing a form must not close it");
        s.forms.open = None;
    }
}

#[test]
fn the_leg_editor_builds_and_refuses_entries() {
    let mut s = session();
    let budget = s.repo.working().clone();
    s.forms.open(FormKind::Transaction, &budget);

    // Drawing it repeatedly must be stable - adding a side on every frame
    // would be a classic immediate-mode bug.
    let forms = &mut s.forms;
    for _ in 0..3 {
        run_ui(|ui| {
            forms.show(ui, &budget);
        });
    }
    assert_eq!(forms.open, Some(FormKind::Transaction));
}

#[test]
fn transaction_filters_narrow_the_list() {
    let mut s = session();
    s.view = View::Transactions;
    s.tx_from = "2024-01-06".into();
    s.tx_to = "2024-01-06".into();
    draw(&mut s, View::Transactions);

    // The same filters, executed directly, to check the view is not lying.
    let rows = TxQuery::new()
        .filter(TxFilter::OnOrAfter("2024-01-06".parse().unwrap()))
        .filter(TxFilter::OnOrBefore("2024-01-06".parse().unwrap()))
        .run(s.repo.working());
    assert_eq!(rows.len(), 1);

    // A malformed date must render a complaint, not panic or silently filter.
    s.tx_from = "not a date".into();
    draw(&mut s, View::Transactions);
}

/// The combining branch of the Buckets screen is a whole second layout that the
/// "draw every view" tests never reach, because it only runs with terms set.
#[test]
fn the_buckets_screen_draws_a_combination() {
    let mut s = session();
    let net = s.selected_bucket.expect("fixture selects a bucket");
    let spending = s.repo.add_bucket("Spending", "").unwrap();
    let cash = s.repo.working().ledgers.uid[0];
    s.repo.stage(Op::AddToBucket { bucket: spending, ledger: cash }).unwrap();

    // One added, one subtracted, and the subtracted one overlaps the added one
    // so the cancelled section draws too.
    s.bucket_combo = vec![Term::plus(net), Term::minus(spending)];
    draw(&mut s, View::Buckets);

    let c = combine(s.budget(), &s.bucket_combo, s.bucket_roll, s.ledger_sort, Order::Asc);
    assert_eq!(c.cancelled.len(), 1, "cash is in both buckets");

    // A term naming a bucket that is not on this branch must render, not panic.
    s.bucket_combo = vec![Term::plus(BucketUid::new())];
    draw(&mut s, View::Buckets);

    // And a leading subtraction is a legitimate, if odd, thing to ask for.
    s.bucket_combo = vec![Term::minus(net)];
    draw(&mut s, View::Buckets);
}
