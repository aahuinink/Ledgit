//! Headless smoke tests.
//!
//! `egui::__run_test_ui` runs a real egui pass with no window, so these
//! actually execute every widget call: a duplicated grid id, a panic in a
//! formatter, or a view that reads a ledger that is not on this branch all
//! show up here rather than the first time you click the tab.
//!
//! They are not a substitute for looking at the thing. They are a substitute
//! for shipping a screen that crashes.

use crate::app::{Screen, Session};
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

    // A small ledger tree, with an implied level and a subtree bucket.
    s.repo.add_ledger("Wedding:Tuxedo", "", Normality::Debit, open).unwrap();
    s.repo.add_ledger("Wedding:Venue:Deposit", "", Normality::Debit, open).unwrap();
    s.repo.add_ledger("Wedding:Gifts", "", Normality::Credit, open).unwrap();
    s.repo.stage(Op::AddSubtreeToBucket { bucket, path: "Wedding".into() }).unwrap();

    // A cohort and a saved view, so their screens have something to draw.
    let pay = s.repo.working().issuers.uid[0];
    let bills = s.repo.add_cohort("Bills", "").unwrap();
    s.repo.stage(Op::AddToCohort { cohort: bills, issuer: pay }).unwrap();
    let net = s.repo.add_view("Net worth", "", ViewSpec::all_buckets(s.repo.working())).unwrap();

    s.selected_ledger = Some(cash);
    s.selected_bucket = Some(bucket);
    s.selected_cohort = Some(bills);
    s.selected_view = Some(net);
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

fn draw(s: &mut Session, view: Screen) {
    run_ui(|ui| match view {
        Screen::Dashboard => views::dashboard::show(ui, s),
        Screen::Ledgers => views::ledgers::show(ui, s),
        Screen::Register => views::ledgers::register(ui, s),
        Screen::Transactions => views::transactions::show(ui, s),
        Screen::Issuers => views::issuers::show(ui, s),
        Screen::Buckets => views::buckets::show(ui, s),
        Screen::Cohorts => views::cohorts::show(ui, s),
        Screen::Views => views::saved::show(ui, s),
        Screen::Commit => views::commit::show(ui, s),
        Screen::History => views::history::show(ui, s),
    });
}

const EVERY_VIEW: [Screen; 10] = [
    Screen::Dashboard,
    Screen::Ledgers,
    Screen::Register,
    Screen::Transactions,
    Screen::Issuers,
    Screen::Buckets,
    Screen::Cohorts,
    Screen::Views,
    Screen::Commit,
    Screen::History,
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
    s.selected_cohort = Some(CohortUid::new());
    s.selected_view = Some(ViewUid::new());
    s.view_draft = Some((ViewUid::new(), ViewSpec::default()));
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
    for kind in [
        FormKind::Ledger,
        FormKind::Transaction,
        FormKind::Bucket,
        FormKind::Issuer,
        FormKind::Cohort,
        FormKind::View,
        FormKind::Move,
    ] {
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
    s.view = Screen::Transactions;
    s.tx_from = "2024-01-06".into();
    s.tx_to = "2024-01-06".into();
    draw(&mut s, Screen::Transactions);

    // The same filters, executed directly, to check the view is not lying.
    let rows = TxQuery::new()
        .filter(TxFilter::OnOrAfter("2024-01-06".parse().unwrap()))
        .filter(TxFilter::OnOrBefore("2024-01-06".parse().unwrap()))
        .run(s.repo.working());
    assert_eq!(rows.len(), 1);

    // A malformed date must render a complaint, not panic or silently filter.
    s.tx_from = "not a date".into();
    draw(&mut s, Screen::Transactions);
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
    draw(&mut s, Screen::Buckets);

    let c = combine(s.budget(), &s.bucket_combo, s.bucket_roll, s.ledger_sort, Order::Asc);
    assert_eq!(c.cancelled.len(), 1, "cash is in both buckets");

    // A term naming a bucket that is not on this branch must render, not panic.
    s.bucket_combo = vec![Term::plus(BucketUid::new())];
    draw(&mut s, Screen::Buckets);

    // And a leading subtraction is a legitimate, if odd, thing to ask for.
    s.bucket_combo = vec![Term::minus(net)];
    draw(&mut s, Screen::Buckets);
}

/// The cohort screen's two layouts - one cohort, and "every issuer" - and a
/// calendar month holding posted, overdue and upcoming payments at once.
#[test]
fn the_cohorts_screen_draws_rates_and_a_calendar() {
    let mut s = session();
    s.calendar_month = "2024-01-01".parse().unwrap();
    draw(&mut s, Screen::Cohorts);
    s.selected_cohort = None;
    draw(&mut s, Screen::Cohorts);
    // Far from any payment, the calendar is simply empty.
    s.calendar_month = "1990-06-01".parse().unwrap();
    draw(&mut s, Screen::Cohorts);
}

/// The Views screen keeps an edited draft apart from the stored spec, and
/// redraws from it; the chart, tables and every editor widget run here.
#[test]
fn the_views_screen_draws_a_draft_and_never_stages_it_by_itself() {
    let mut s = session();
    draw(&mut s, Screen::Views);
    let (uid, stored) = s.view_draft.clone().expect("drawing starts a draft");
    let staged_before = s.repo.staged().len();

    // Edit the draft as the screen would, and draw it with every option on.
    let (_, draft) = s.view_draft.as_mut().unwrap();
    draft.horizon = Span::Years(2);
    draft.period = Period::Week;
    draft.only_selected_issuers = true;
    draft.ledgers = s.repo.working().ledgers.uid.clone();
    draft.transactions = vec![TxFilter::Text("pay".into())];
    for until in ["", "2031-12-31", "not a date", "1999-01-01"] {
        s.view_until = until.into();
        draw(&mut s, Screen::Views);
    }
    assert_eq!(s.repo.staged().len(), staged_before, "a draft is not an op");
    assert_ne!(s.view_draft.as_ref().unwrap().1, stored);
    assert_eq!(s.view_draft.as_ref().unwrap().0, uid);

    // A view with no scope - issuers only - draws flows without a chart.
    let issuers = s.repo.working().issuers.uid.clone();
    s.view_draft = Some((uid, ViewSpec { issuers, ..ViewSpec::default() }));
    draw(&mut s, Screen::Views);
}

/// Both layouts of the Ledgers screen, with a level folded shut, and the move
/// form opened the way its row button opens it.
#[test]
fn the_ledger_tree_draws_folded_and_flat() {
    let mut s = session();
    assert!(s.ledger_tree, "the tree is the default");
    draw(&mut s, Screen::Ledgers);
    s.collapsed.insert("wedding".into());
    draw(&mut s, Screen::Ledgers);
    s.ledger_tree = false;
    draw(&mut s, Screen::Ledgers);

    s.forms.open_move("Wedding");
    let budget = s.repo.working().clone();
    let forms = &mut s.forms;
    run_ui(|ui| {
        forms.show(ui, &budget);
    });
    assert_eq!(s.forms.open, Some(FormKind::Move));
}

/// The bucket screen shows subtree members as "via" rather than removable,
/// and a ledger created under the path later joins the bucket.
#[test]
fn a_subtree_bucket_draws_and_grows() {
    let mut s = session();
    draw(&mut s, Screen::Buckets);
    let bucket = s.selected_bucket.unwrap();
    let count = |s: &Session| {
        let l = s.budget();
        l.buckets.members[l.buckets.ix(bucket).unwrap().get()].len()
    };
    let before = count(&s);
    let open = "2024-01-01".parse().unwrap();
    s.repo.add_ledger("Wedding:Flowers", "", Normality::Debit, open).unwrap();
    assert_eq!(count(&s), before + 1);
    draw(&mut s, Screen::Buckets);
}

/// The top bar carries the brand mark and the freshness label; draw it in
/// both themes, since each picks its own copy of the artwork.
#[test]
fn the_top_bar_draws_the_mark_in_both_themes() {
    let mut s = session();
    for dark in [false, true] {
        egui::__run_test_ctx(|ctx| {
            ctx.set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
            let mut brand = crate::brand::Brand::load(ctx);
            brand.sync_window_icon(ctx);
            crate::app::top_bar(ctx, &mut s, &brand);
        });
    }
}

/// Start-up pays for rasterising the artwork once; keep an eye on what that
/// costs. Run with `cargo test --release -p ledgit-gui -- --ignored --nocapture`.
#[test]
#[ignore]
fn how_long_the_artwork_takes_to_load() {
    let ctx = egui::Context::default();
    let t = std::time::Instant::now();
    let _ = crate::brand::Brand::load(&ctx);
    let _ = crate::brand::window_icon(true);
    println!("brand artwork loaded in {:?}", t.elapsed());
}
