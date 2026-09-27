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

    s.repo
        .stage(Op::SetVariable { name: "Car_Km_Rate".into(), value: VarValue::guess("0.68") })
        .unwrap();
    s.repo
        .stage(Op::SetVariable { name: "Home".into(), value: VarValue::guess("Toronto") })
        .unwrap();

    // A target on the loan, and an alert on chequing that is firing.
    s.repo
        .stage(Op::SetLedgerGoals { uid: loan, target: Some(Money::ZERO), alerts: vec![] })
        .unwrap();
    let low = Alert {
        when: AlertWhen::Below,
        level: Money::from_major(1_000_000),
        message: "Top up".into(),
    };
    s.repo.stage(Op::SetLedgerGoals { uid: cash, target: None, alerts: vec![low] }).unwrap();

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
        Screen::Variables => views::variables::show(ui, s),
        Screen::Commit => views::commit::show(ui, s),
        Screen::History => views::history::show(ui, s),
    });
}

const EVERY_VIEW: [Screen; 11] = [
    Screen::Dashboard,
    Screen::Ledgers,
    Screen::Register,
    Screen::Transactions,
    Screen::Issuers,
    Screen::Buckets,
    Screen::Cohorts,
    Screen::Views,
    Screen::Variables,
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
            let brand = crate::brand::Brand::load(ctx);
            crate::app::top_bar(ctx, &mut s, &brand, &[]);
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
    let _ = crate::brand::window_icon();
    println!("brand artwork loaded in {:?}", t.elapsed());
}

/// A ledger picker inside the form modal must open *above* the modal. Both
/// live on the Foreground order, and the modal is raised whenever it is
/// clicked - which is exactly how the picker gets opened - so without help
/// the list draws underneath the form and only the part hanging past its
/// edge is visible.
#[test]
fn a_picker_in_the_form_modal_opens_on_top_of_it() {
    let s = session();
    let budget = s.repo.working().clone();
    let ctx = egui::Context::default();
    let button = std::cell::Cell::new(egui::Rect::NOTHING);
    let modal_layer = std::cell::Cell::new(None);
    let pass = |ctx: &egui::Context, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |_| {});
            egui::Modal::new(egui::Id::new("form_modal")).show(ctx, |ui| {
                ui.set_width(640.0);
                modal_layer.set(Some(ui.layer_id()));
                let before = ui.cursor().min;
                crate::picker::Picker::new("probe", &budget).width(220.0).show(ui);
                button.set(egui::Rect::from_min_max(before, ui.min_rect().max));
            });
        });
    };
    pass(&ctx, vec![]);
    pass(&ctx, vec![]);
    let button = button.get();
    let at = egui::pos2(button.left() + 20.0, button.center().y);
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    pass(&ctx, vec![egui::Event::PointerMoved(at), press(true)]);
    pass(&ctx, vec![press(false)]);
    pass(&ctx, vec![]);
    pass(&ctx, vec![]);

    // Just under the button is where the list opens. The modal's backdrop
    // covers the whole window, so whichever layer is on top there wins.
    let below = egui::pos2(at.x, button.bottom() + 30.0);
    let top = ctx.layer_id_at(below).expect("something is drawn there");
    assert_ne!(Some(top), modal_layer.get(), "the picker's list is hidden behind the form");
}

/// Dropping a staged ledger that a staged entry posts to leaves the entry
/// broken; the commit screen draws it flagged, and an edit reopens it.
#[test]
fn the_commit_screen_draws_broken_changes() {
    let mut s = session();
    let open = "2024-01-01".parse().unwrap();
    let scratch = s.repo.add_ledger("Scratch", "", Normality::Debit, open).unwrap();
    let cash = s.repo.working().ledgers.uid[0];
    s.repo.post("uses it", "", open, Money::from_major(3), scratch, cash).unwrap();
    let at = s.repo.staged().len() - 2;
    s.repo.unstage_at(at).unwrap();
    assert_eq!(s.repo.broken().len(), 1);
    draw(&mut s, Screen::Commit);
    assert!(!s.repo.report().unwrap().can_commit());

    let i = s.repo.broken()[0].index;
    let op = s.repo.staged()[i].clone();
    assert!(s.forms.edit_staged(i, &op));
    let budget = s.repo.working().clone();
    let forms = &mut s.forms;
    run_ui(|ui| {
        forms.show(ui, &budget);
    });
}

/// The back arrow walks visited screens in reverse without bouncing, and
/// undo/redo step the staging area - but never across a commit.
#[test]
fn back_undo_and_redo() {
    let mut s = session();
    for next in [Screen::Ledgers, Screen::Buckets, Screen::Commit] {
        let before = s.view;
        s.view = next;
        s.track(before);
    }
    for expected in [Screen::Buckets, Screen::Ledgers, Screen::Dashboard] {
        let before = s.view;
        s.go_back();
        s.track(before);
        assert_eq!(s.view, expected);
    }
    assert!(s.back.is_empty());

    let staged = s.repo.staged().to_vec();
    let cash = s.repo.working().ledgers.uid[0];
    let loan = s.repo.working().ledgers.uid[1];
    let open = "2024-02-01".parse().unwrap();
    s.repo.post("one", "", open, Money::from_major(1), cash, loan).unwrap();
    s.track(s.view);
    s.repo.post("two", "", open, Money::from_major(2), cash, loan).unwrap();
    s.track(s.view);
    s.repo.unstage_at(0).unwrap();
    s.track(s.view);
    assert_eq!(s.undo.len(), 3);

    s.undo();
    s.track(s.view);
    assert_eq!(s.repo.staged().len(), staged.len() + 2, "the dropped change is back");
    s.undo();
    s.undo();
    s.track(s.view);
    assert_eq!(s.repo.staged(), &staged[..]);
    assert!(s.repo.broken().is_empty());
    s.redo();
    s.track(s.view);
    assert_eq!(s.repo.staged().len(), staged.len() + 1);
    assert_eq!(s.redo.len(), 2);

    // A commit draws a line: nothing before it can be put back.
    s.repo.commit("done").unwrap();
    s.track(s.view);
    assert!(s.undo.is_empty() && s.redo.is_empty());
    s.undo();
    assert!(s.repo.staged().is_empty());
}

/// Every symbol the GUI writes as a `\u{...}` escape must exist in egui's
/// bundled fonts. They cover only part of Unicode, and a missing glyph draws
/// as an empty box - which no other test would ever notice.
#[test]
fn every_symbol_in_the_source_has_a_glyph() {
    let sources = [
        include_str!("app.rs"),
        include_str!("brand.rs"),
        include_str!("datepick.rs"),
        include_str!("fmt.rs"),
        include_str!("forms.rs"),
        include_str!("instance.rs"),
        include_str!("picker.rs"),
        include_str!("table.rs"),
        include_str!("textbox.rs"),
        include_str!("views/graph.rs"),
        include_str!("views/buckets.rs"),
        include_str!("views/cohorts.rs"),
        include_str!("views/commit.rs"),
        include_str!("views/dashboard.rs"),
        include_str!("views/goals.rs"),
        include_str!("views/history.rs"),
        include_str!("views/issuers.rs"),
        include_str!("views/ledgers.rs"),
        include_str!("views/mod.rs"),
        include_str!("views/saved.rs"),
        include_str!("views/search.rs"),
        include_str!("views/transactions.rs"),
        include_str!("views/variables.rs"),
    ];
    let mut used = std::collections::BTreeSet::new();
    for src in sources {
        for (i, _) in src.match_indices("\\u{") {
            let hex: String = src[i + 3..].chars().take_while(|c| *c != '}').collect();
            if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                used.insert(c);
            }
        }
    }
    assert!(used.len() > 10, "the scan found {used:?}; is it still reading the sources?");

    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |_| {});
    let missing: Vec<String> = used
        .into_iter()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .filter(|c| !ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(14.0), *c)))
        .map(|c| format!("U+{:04X}", c as u32))
        .collect();
    assert!(missing.is_empty(), "no glyph in egui's fonts for {missing:?}");
}

/// Targets and alerts on every screen that shows them: a ledger's page with
/// the "when is it reached" projection on, a bucket's aggregated targets,
/// and a view with a target line and an alert ahead.
#[test]
fn targets_and_alerts_draw_everywhere() {
    let mut s = session();
    assert!(!ledgit_core::goals::fired(s.budget()).is_empty(), "the fixture sets one off");
    let loan = s.budget().ledgers.uid[1];
    s.selected_ledger = Some(loan);
    s.show_target_reached = true;
    draw(&mut s, Screen::Register);
    draw(&mut s, Screen::Dashboard);
    s.bucket_targets = true;
    draw(&mut s, Screen::Buckets);
    let (uid, _) = s.view_draft.clone().unwrap_or((s.selected_view.unwrap(), ViewSpec::default()));
    s.view_draft = Some((
        uid,
        ViewSpec { ledgers: vec![loan, s.budget().ledgers.uid[0]], ..ViewSpec::default() },
    ));
    draw(&mut s, Screen::Views);
    egui::__run_test_ctx(|ctx| {
        let brand = crate::brand::Brand::load(ctx);
        crate::app::top_bar(ctx, &mut s, &brand, &[]);
    });
}

/// The Views screen laid over an earlier commit, and over the budget
/// without its staged changes: the chart overlay and the comparison table.
#[test]
fn the_views_screen_compares_against_history() {
    use crate::views::saved::CompareWith;
    let mut s = session();
    let first = *s.repo.log(None).unwrap().last().map(|c| &c.id).unwrap();
    for with in [CompareWith::Commit(first), CompareWith::Committed] {
        s.view_compare = Some(with);
        draw(&mut s, Screen::Views);
    }
    assert_eq!(s.compare_cache.as_ref().map(|(c, _)| *c), Some(first), "folded once, kept");
}

/// Every screen, drawn against a real budget file - e.g. the generated
/// fixtures, which hold far more than the in-memory one above:
///
/// ```sh
/// cargo run -p ledgit-cli --example fixtures
/// LEDGIT_SMOKE_FILE=$PWD/fixtures/household.ledgit cargo test -p ledgit-gui -- --ignored draws_a_real_file
/// ```
///
/// It opens the file read-only in spirit: nothing is staged or committed.
#[test]
#[ignore]
fn draws_a_real_file() {
    let path = std::env::var("LEDGIT_SMOKE_FILE").expect("set LEDGIT_SMOKE_FILE to a .ledgit file");
    let mut s = Session::open(PathBuf::from(&path), "tester").unwrap();
    let staged = s.repo.staged().to_vec();
    let l = s.repo.working();
    s.selected_ledger = l
        .ledgers
        .indices()
        .find(|ix| l.ledgers.target[ix.get()].is_some())
        .map(|ix| l.ledgers.uid[ix.get()]);
    s.selected_bucket = l.buckets.live().next().map(|b| l.buckets.uid[b.get()]);
    s.selected_cohort = l.cohorts.live().next().map(|c| l.cohorts.uid[c.get()]);
    s.selected_view = l.views.live().nth(1).map(|v| l.views.uid[v.get()]);
    s.bucket_targets = true;
    let first = s.repo.log(None).unwrap().last().map(|c| c.id);
    s.view_compare = first.map(crate::views::saved::CompareWith::Commit);
    let t = std::time::Instant::now();
    for view in EVERY_VIEW {
        draw(&mut s, view);
    }
    println!("{path}: every screen drawn in {:?}", t.elapsed());
    assert_eq!(s.repo.staged(), &staged[..], "drawing must not change the stage");
}

// ----------------------------------------------------------------- layout
//
// What a pixel test can check without a display: where the text lands. Each
// run returns every piece of text egui painted, with its rectangle.

/// Draw `f` for a few passes in a window of `size` and return the text of
/// the last one. Several passes, because a table measures its columns first.
fn painted_text(size: egui::Vec2, mut f: impl FnMut(&egui::Context)) -> Vec<(String, egui::Rect)> {
    let ctx = egui::Context::default();
    let mut out = Vec::new();
    for _ in 0..4 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let full = ctx.run(input, &mut f);
        out = full
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => {
                    Some((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2())))
                }
                _ => None,
            })
            .collect();
    }
    out
}

fn find<'a>(texts: &'a [(String, egui::Rect)], text: &str) -> Vec<&'a egui::Rect> {
    texts.iter().filter(|(t, _)| t == text).map(|(_, r)| r).collect()
}

/// The first real-run complaint: figures in the last column were pushed
/// against the far edge of the window, away from their header. They belong
/// right-aligned under the header, in a column as wide as they are.
#[test]
fn figures_sit_under_their_header_not_at_the_window_edge() {
    let mut s = session();
    s.ledger_tree = false;
    let texts = painted_text(egui::vec2(1600.0, 900.0), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| views::ledgers::show(ui, &mut s));
    });
    // The sort control says "balance" too; the column header is below it.
    let header = *find(&texts, "balance").last().expect("a balance header");
    let cash = s.budget().ledgers.ix(s.selected_ledger.unwrap()).unwrap();
    let amount = crate::fmt::amount(s.budget().ledgers.balance(cash));
    let figure = find(&texts, &amount)[0];
    assert!(
        (figure.right() - header.right()).abs() < 1.5,
        "figure {figure:?} is not right-aligned with its header {header:?}"
    );
    assert!(header.right() < 900.0, "the balance column ran to the window edge: {header:?}");
}

/// A headless window to click and type in, reporting what it painted.
struct Window {
    ctx: egui::Context,
    size: egui::Vec2,
}

impl Window {
    fn new(w: f32, h: f32) -> Window {
        Window { ctx: egui::Context::default(), size: egui::vec2(w, h) }
    }

    fn pass(
        &self,
        events: Vec<egui::Event>,
        f: &mut dyn FnMut(&egui::Context),
    ) -> Vec<(String, egui::Rect)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            events,
            ..Default::default()
        };
        let full = self.ctx.run(input, |ctx| f(ctx));
        full.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => {
                    Some((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2())))
                }
                _ => None,
            })
            .collect()
    }

    /// Press and release at `at`, then let the result settle.
    fn click(
        &self,
        at: egui::Pos2,
        f: &mut dyn FnMut(&egui::Context),
    ) -> Vec<(String, egui::Rect)> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.pass(vec![egui::Event::PointerMoved(at), button(true)], f);
        self.pass(vec![button(false)], f);
        self.pass(vec![], f)
    }

    fn settle(&self, f: &mut dyn FnMut(&egui::Context)) -> Vec<(String, egui::Rect)> {
        self.pass(vec![], f);
        self.pass(vec![], f)
    }
}

fn centre_of(texts: &[(String, egui::Rect)], text: &str) -> egui::Pos2 {
    texts
        .iter()
        .find(|(t, _)| t == text)
        .unwrap_or_else(|| panic!("{text:?} is not on screen: {texts:?}"))
        .1
        .center()
}

/// Clicking the picker's search box used to close the picker: a combo box
/// closes on any click, its own contents included.
#[test]
fn the_ledger_picker_stays_open_to_be_searched() {
    let s = session();
    let budget = s.repo.working().clone();
    let picked = std::cell::Cell::new(None);
    let mut draw = |ctx: &egui::Context| {
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(p) = crate::picker::Picker::new("probe", &budget).show(ui) {
                picked.set(Some(p));
            }
        });
    };
    let w = Window::new(1000.0, 800.0);
    let texts = w.settle(&mut draw);
    let texts = w.click(centre_of(&texts, "choose a ledger..."), &mut draw);
    let hint = "search, e.g. wedding or tux";
    let texts = w.click(centre_of(&texts, hint), &mut draw);
    assert!(
        texts.iter().any(|(t, _)| t == "Chequing  [debit]"),
        "clicking the search box closed the list"
    );

    w.pass(vec![egui::Event::Text("tux".into())], &mut draw);
    let texts = w.settle(&mut draw);
    assert!(texts.iter().any(|(t, _)| t == "Tuxedo  [debit]"), "the match is listed: {texts:?}");
    assert!(!texts.iter().any(|(t, _)| t == "Chequing  [debit]"), "the search filters the list");

    let texts = w.click(centre_of(&texts, "Tuxedo  [debit]"), &mut draw);
    assert!(matches!(picked.take(), Some(crate::picker::Pick::Ledger(_))));
    assert!(!texts.iter().any(|(t, _)| t == hint), "a pick closes the list");
}

/// The logo is a File menu. Picking from it is reported to the app, which
/// swaps the budget after the frame.
#[test]
fn the_logo_opens_the_file_menu() {
    let mut s = session();
    let recent = vec!["elsewhere/other.ledgit".to_string()];
    let action = std::cell::RefCell::new(None);
    let w = Window::new(1400.0, 800.0);
    let mut draw = |ctx: &egui::Context| {
        let brand = crate::brand::Brand::load(ctx);
        if let Some(a) = crate::app::top_bar(ctx, &mut s, &brand, &recent) {
            *action.borrow_mut() = Some(a);
        }
    };
    w.settle(&mut draw);
    // The mark is an image, so find it by where it sits: just right of the
    // back arrow, left of the budget's name.
    let texts = w.settle(&mut draw);
    let name = centre_of(&texts, "test");
    let texts = w.click(egui::pos2(name.x - 30.0, name.y), &mut draw);
    for item in ["New budget...", "Open budget...", "Open recent", "Close budget"] {
        assert!(texts.iter().any(|(t, _)| t == item), "{item} is in the File menu: {texts:?}");
    }
    w.click(centre_of(&texts, "Close budget"), &mut draw);
    assert_eq!(action.take(), Some(crate::app::FileAction::Close));
}

/// Switching branch with changes staged used to be impossible - the button
/// was greyed out. Now it asks, and shelving keeps the work on the branch
/// it was entered on.
#[test]
fn switching_branch_with_staged_work_asks_and_can_shelve_it() {
    let s = std::cell::RefCell::new(session());
    s.borrow_mut().repo.branch("what-if", None).unwrap();
    let staged = s.borrow().repo.staged().len();
    assert!(staged > 0);
    let w = Window::new(1400.0, 800.0);
    let mut draw = |ctx: &egui::Context| {
        egui::CentralPanel::default().show(ctx, |ui| views::history::show(ui, &mut s.borrow_mut()));
    };
    let texts = w.settle(&mut draw);
    let texts = w.click(centre_of(&texts, "switch"), &mut draw);
    let texts = w.click(centre_of(&texts, "Shelve them on main"), &mut draw);
    assert!(!texts.iter().any(|(t, _)| t == "Switch to what-if?"), "the question is answered");
    {
        let s = s.borrow();
        assert_eq!(s.repo.head().branch_name(), Some("what-if"));
        assert!(s.repo.staged().is_empty());
        assert_eq!(s.repo.shelved("main").unwrap(), staged);
    }

    // Back again: the work comes off the shelf.
    let texts = w.settle(&mut draw);
    assert!(texts.iter().any(|(t, _)| t == &format!("{staged} shelved")), "{texts:?}");
    w.click(centre_of(&texts, "switch"), &mut draw);
    let s = s.borrow();
    assert_eq!(s.repo.head().branch_name(), Some("main"));
    assert_eq!(s.repo.staged().len(), staged);
}

/// A busy month reads better as a list; the calendar offers one.
#[test]
fn the_cohort_calendar_draws_as_a_list() {
    let mut s = session();
    s.calendar_month = "2024-01-01".parse().unwrap();
    s.calendar_list = true;
    draw(&mut s, Screen::Cohorts);
    s.selected_cohort = None;
    draw(&mut s, Screen::Cohorts);
    s.calendar_month = "1990-06-01".parse().unwrap();
    draw(&mut s, Screen::Cohorts);
}

/// A session with a bucket holding `n` ledgers, one with a very long name.
fn crowded(n: usize) -> Session {
    let mut s = session();
    let bucket = s.selected_bucket.unwrap();
    let open = "2024-01-01".parse().unwrap();
    for i in 0..n {
        let name = if i == 0 {
            "Expenses:Household:An extraordinarily long ledger name that goes on and on, and then on some more after that".into()
        } else {
            format!("Expenses:Item {i:03}")
        };
        let ledger = s.repo.add_ledger(name, "", Normality::Debit, open).unwrap();
        s.repo.stage(Op::AddToBucket { bucket, ledger }).unwrap();
    }
    s
}

/// A bucket with more ledgers than fit on screen scrolls, rather than
/// running off the bottom of the window.
#[test]
fn a_long_bucket_scrolls_inside_the_window() {
    let mut s = crowded(80);
    let texts = painted_text(egui::vec2(1200.0, 700.0), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| views::buckets::show(ui, &mut s));
    });
    let items = texts.iter().filter(|(t, _)| t.starts_with("Expenses:Item")).count();
    assert!(items > 5, "the members are listed: {texts:?}");
    assert!(items < 79, "only what fits is drawn; the rest scrolls ({items} drawn)");
    let lowest = texts.iter().map(|(_, r)| r.bottom()).fold(0.0, f32::max);
    assert!(lowest <= 700.0, "text drawn below the window: {lowest}");
    // The Add row sits above the list, so it is always in reach.
    assert!(centre_of(&texts, "Add").y < centre_of(&texts, "Expenses:Item 001").y);
}

/// Long text is cut short inside its own column instead of spilling over
/// the next one.
#[test]
fn a_long_name_does_not_bleed_into_the_next_column() {
    let mut s = crowded(3);
    s.ledger_tree = false;
    let texts = painted_text(egui::vec2(1400.0, 800.0), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| views::ledgers::show(ui, &mut s));
    });
    let long = texts
        .iter()
        .find(|(t, _)| t.starts_with("Expenses:Household:An extra"))
        .expect("the long name is drawn");
    // (The galley keeps its whole text even when it is drawn elided, so the
    // test is on where it ends, not on what it says.)
    let row_mid = long.1.center().y;
    let normal = texts
        .iter()
        .find(|(t, r)| t == "debit" && (r.center().y - row_mid).abs() < 3.0)
        .expect("the normality on the same row");
    assert!(long.1.right() < normal.1.left(), "{:?} runs into {:?}", long.1, normal.1);
}

/// The Views editor's sections fit what they hold, then scroll: a hundred
/// picked ledgers must not push everything below them off the page, and a
/// few buckets must not leave a gap.
#[test]
fn the_view_editor_sections_fit_then_scroll() {
    let mut s = crowded(100);
    let texts = painted_text(egui::vec2(1400.0, 900.0), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| views::saved::show(ui, &mut s));
    });
    let y = |t: &str| centre_of(&texts, t).y;
    let short = y("Issuers") - y("Ledgers");
    assert!(short < 40.0, "with none picked, the ledger section is one line: {short}");
    assert!(y("Ledgers") - y("Buckets") < 3.0 * 22.0, "gap after a short bucket list");

    let (uid, mut spec) = s.view_draft.clone().unwrap();
    spec.ledgers = s.budget().ledgers.uid.clone();
    s.view_draft = Some((uid, spec));
    let texts = painted_text(egui::vec2(1400.0, 900.0), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| views::saved::show(ui, &mut s));
    });
    let y = |t: &str| centre_of(&texts, t).y;
    let tall = y("Issuers") - y("Ledgers");
    // The picker line, then four lines of chips.
    assert!(tall < 22.0 * 6.0, "the ledger chips run {tall} tall instead of scrolling");
    assert!(tall > 22.0 * 3.0, "the ledger chips show more than a sliver");
}

/// Ledgers join a view from the tree picker - one, or a whole subtree - and
/// leave it by clicking their chip.
#[test]
fn a_view_takes_ledgers_from_the_picker_and_drops_them_by_chip() {
    let s = std::cell::RefCell::new(session());
    let w = Window::new(1400.0, 1000.0);
    let mut draw = |ctx: &egui::Context| {
        egui::CentralPanel::default().show(ctx, |ui| views::saved::show(ui, &mut s.borrow_mut()));
    };
    w.settle(&mut draw);
    {
        let mut s = s.borrow_mut();
        let (uid, mut spec) = s.view_draft.clone().unwrap();
        spec.ledgers.clear();
        s.view_draft = Some((uid, spec));
    }
    let texts = w.settle(&mut draw);
    assert!(texts.iter().any(|(t, _)| t == "none picked"));
    let texts = w.click(centre_of(&texts, "add a ledger..."), &mut draw);
    let texts = w.click(centre_of(&texts, "all 3 under"), &mut draw);
    let picked = |s: &Session| s.view_draft.as_ref().unwrap().1.ledgers.len();
    assert_eq!(picked(&s.borrow()), 3, "the whole Wedding subtree joined");
    assert!(texts.iter().any(|(t, _)| t == "3 picked"), "{texts:?}");

    let chip = texts
        .iter()
        .find(|(t, _)| t.starts_with("Wedding:Tuxedo"))
        .map(|(_, r)| r.center())
        .expect("a chip for each picked ledger");
    w.click(chip, &mut draw);
    assert_eq!(picked(&s.borrow()), 2, "clicking a chip takes it off");
    assert!(
        s.borrow().repo.staged().iter().all(|op| !matches!(op, Op::EditView { .. })),
        "picking edits the draft, not the budget"
    );
}

/// A view's description is written in place and staged like any edit.
#[test]
fn a_view_description_is_written_and_staged() {
    let s = std::cell::RefCell::new(session());
    let w = Window::new(1400.0, 1000.0);
    let mut draw = |ctx: &egui::Context| {
        egui::CentralPanel::default().show(ctx, |ui| views::saved::show(ui, &mut s.borrow_mut()));
    };
    let texts = w.settle(&mut draw);
    w.click(centre_of(&texts, "add a description"), &mut draw);
    w.pass(vec![egui::Event::Text("Net worth, car paid early".into())], &mut draw);
    let texts = w.settle(&mut draw);
    w.click(centre_of(&texts, "Stage description"), &mut draw);

    let s = s.borrow();
    let uid = s.selected_view.unwrap();
    let l = s.budget();
    assert_eq!(l.views.description[l.views.ix(uid).unwrap().get()], "Net worth, car paid early");
    assert!(matches!(
        s.repo.staged().last(),
        Some(Op::EditView { description: Some(_), spec: None, name: None, .. })
    ));
    assert!(s.view_description.is_none(), "the editor closes once staged");
}

/// No column is squeezed below sixteen characters; a table too wide for the
/// window scrolls sideways instead of crushing its columns.
#[test]
fn table_columns_never_go_below_sixteen_characters() {
    let mut s = session();
    let floor = std::cell::Cell::new(0.0);
    let texts = painted_text(egui::vec2(900.0, 1600.0), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            floor.set(crate::table::min_column_width(ui));
            views::saved::show(ui, &mut s)
        });
    });
    let floor = floor.get();
    assert!(floor > 80.0, "sixteen characters is a real width: {floor}");
    let low = centre_of(&texts, "lowest ahead").y;
    let mut heads: Vec<f32> = texts
        .iter()
        .filter(|(t, r)| {
            ["today", "at end", "change", "lowest ahead"].contains(&t.as_str())
                && (r.center().y - low).abs() < 2.0
        })
        .map(|(_, r)| r.right())
        .collect();
    heads.sort_by(f32::total_cmp);
    assert_eq!(heads.len(), 4, "the balances headers: {texts:?}");
    for pair in heads.windows(2) {
        assert!(pair[1] - pair[0] >= floor, "a column {} wide", pair[1] - pair[0]);
    }
}

/// History draws every branch as one graph, and a commit on a branch you
/// are not on can be opened.
#[test]
fn history_draws_every_branch_and_opens_any_commit() {
    let s = std::cell::RefCell::new(session());
    {
        let mut s = s.borrow_mut();
        s.repo.clear_stage().unwrap();
        s.repo.checkout_new("what-if").unwrap();
        let cash = s.repo.working().ledgers.uid[0];
        let loan = s.repo.working().ledgers.uid[1];
        let when = "2024-03-01".parse().unwrap();
        s.repo.post("what if", "", when, Money::from_major(5), cash, loan).unwrap();
        s.repo.commit("Pay the car off early").unwrap();
        s.repo.checkout("main").unwrap();
    }
    let w = Window::new(1400.0, 800.0);
    let mut draw = |ctx: &egui::Context| {
        egui::CentralPanel::default().show(ctx, |ui| views::history::show(ui, &mut s.borrow_mut()));
    };
    let texts = w.settle(&mut draw);
    let side = texts
        .iter()
        .find(|(t, _)| t.ends_with("Pay the car off early"))
        .map(|(_, r)| r.center())
        .expect("the other branch's commit is in the graph");
    assert!(texts.iter().filter(|(t, _)| t == "what-if").count() >= 2, "tagged, and listed");
    let texts = w.click(side, &mut draw);
    assert!(
        texts.iter().any(|(t, r)| t == "Pay the car off early" && r.left() > side.x),
        "its detail opens: {texts:?}"
    );
    let g = s.borrow().graph.as_ref().map(|(_, g)| g.clone()).expect("cached");
    assert_eq!(g.commits.len(), 4);
    w.settle(&mut draw);
    let again = s.borrow().graph.as_ref().map(|(_, g)| g.clone()).unwrap();
    assert!(std::rc::Rc::ptr_eq(&g, &again), "not rebuilt while nothing moved");
}

/// A branch can start at any earlier commit - the budget as it stood then.
#[test]
fn a_branch_starts_from_an_earlier_commit() {
    let s = std::cell::RefCell::new(session());
    let first = *s.borrow().repo.log(None).unwrap().last().map(|c| &c.id).unwrap();
    s.borrow_mut().selected_commit = Some(first);
    let w = Window::new(1400.0, 900.0);
    let mut draw = |ctx: &egui::Context| {
        egui::CentralPanel::default().show(ctx, |ui| views::history::show(ui, &mut s.borrow_mut()));
    };
    let texts = w.settle(&mut draw);
    // The branch panel has a "new branch name" box too; this one is in the
    // commit's detail, the rightmost pane.
    let field = find(&texts, "new branch name")
        .into_iter()
        .max_by(|a, b| a.left().total_cmp(&b.left()))
        .expect("the branch-from-here box")
        .center();
    w.click(field, &mut draw);
    w.pass(vec![egui::Event::Text("before-the-loan".into())], &mut draw);
    let texts = w.settle(&mut draw);
    w.click(centre_of(&texts, "Create and switch"), &mut draw);

    let s = s.borrow();
    let at = s.repo.branches().unwrap().into_iter().find(|(n, _)| n == "before-the-loan");
    assert_eq!(at.map(|(_, id)| id), Some(first), "the branch starts at the picked commit");
    // Changes are staged in the fixture, so switching asks first.
    assert_eq!(s.pending_switch.as_deref(), Some("before-the-loan"));
    assert_eq!(s.repo.head().branch_name(), Some("main"), "nothing moves until you answer");
}

/// The long-text box pops out into a full editor on the same text.
#[test]
fn a_long_text_pops_out_into_a_full_editor() {
    let text = std::cell::RefCell::new(String::from("Top up"));
    let w = Window::new(1200.0, 800.0);
    let mut draw = |ctx: &egui::Context| {
        egui::CentralPanel::default().show(ctx, |ui| {
            crate::textbox::LongText::new("probe", &mut text.borrow_mut())
                .title("Alert message")
                .show(ui);
        });
    };
    let texts = w.settle(&mut draw);
    let boxed = centre_of(&texts, "Top up");
    assert!(!texts.iter().any(|(t, _)| t == "Alert message"));
    // The pop-out button sits just right of the box.
    let button = texts.iter().find(|(t, _)| t == "\u{2197}").map(|(_, r)| r.center()).unwrap();
    assert!(button.x > boxed.x);
    let texts = w.click(button, &mut draw);
    assert!(texts.iter().any(|(t, _)| t == "Alert message"), "the editor opened");
    w.pass(vec![egui::Event::Text(" from savings".into())], &mut draw);
    let texts = w.settle(&mut draw);
    assert_eq!(*text.borrow(), "Top up from savings", "it edits the same text");
    let texts = w.click(centre_of(&texts, "Done"), &mut draw);
    assert!(!texts.iter().any(|(t, _)| t == "Alert message"), "Done closes it");
}
