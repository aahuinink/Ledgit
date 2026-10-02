//! What the tutorial says, chapter by chapter, and how it knows a step was
//! done.
//!
//! Each chapter runs on its own fresh copy of a sample budget
//! (`ledgit_core::demo`), so what a step points at is really there. A step
//! that asks for an action carries a check over the session - usually "has
//! something been staged since the step began?" - so the panel can tick it
//! off; a step that only asks you to look is ticked by moving on.

use crate::app::{Screen, Session};
use ledgit_core::prelude::*;

/// The session as it stood when a step began, for checks that ask "since".
#[derive(Clone, Debug, Default)]
pub struct Baseline {
    pub staged: Vec<Op>,
    pub head: Option<CommitId>,
    pub branches: Vec<String>,
    pub pins: Vec<LedgerUid>,
}

impl Baseline {
    pub fn of(s: &Session) -> Baseline {
        Baseline {
            staged: s.repo.staged().to_vec(),
            head: s.repo.head_commit().ok().flatten(),
            branches: s.repo.branches().unwrap_or_default().into_iter().map(|(n, _)| n).collect(),
            pins: s.pins.clone(),
        }
    }
}

pub type Check = fn(&Session, &Baseline) -> bool;

/// Where a step takes you when it starts, or when "Show me" is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Go {
    Stay,
    Screen(Screen),
    /// A ledger's page, by name.
    Ledger(&'static str),
    /// The Issuers screen with one issuer open, by name.
    Issuer(&'static str),
    /// The Views screen with one view open, by name.
    View(&'static str),
    /// The Buckets screen with one bucket open, by name.
    Bucket(&'static str),
    /// History with a commit selected, by the start of its message.
    Commit(&'static str),
}

pub struct Step {
    /// Stable, for progress and feedback: "issuers.statement".
    pub id: &'static str,
    pub title: &'static str,
    /// What the feature is.
    pub about: &'static str,
    /// What to do.
    pub try_it: &'static str,
    /// What you should see if it works.
    pub expect: &'static str,
    pub go: Go,
    /// `None`: there is nothing to detect; reading it is the step.
    pub check: Option<Check>,
}

pub struct Chapter {
    pub id: &'static str,
    pub title: &'static str,
    /// The sample budget it runs on (`ledgit_core::demo::NAMES`).
    pub budget: &'static str,
    pub intro: &'static str,
    pub steps: Vec<Step>,
}

// ----------------------------------------------------------------- checks

/// Something matching `f` was staged since the step began.
fn staged(s: &Session, b: &Baseline, f: impl Fn(&Op, &Budget) -> bool) -> bool {
    s.repo.staged().iter().filter(|op| !b.staged.contains(op)).any(|op| f(op, s.budget()))
}

/// HEAD moved since the step began, to a commit matching `f`.
fn committed(s: &Session, b: &Baseline, f: impl Fn(&Commit) -> bool) -> bool {
    let head = s.repo.head_commit().ok().flatten();
    head != b.head && head.and_then(|h| s.repo.get_commit(h).ok()).is_some_and(|c| f(&c))
}

fn names_ledger(b: &Budget, legs: &[Leg], name: &str) -> bool {
    legs.iter().any(|g| b.ledgers.ix(g.ledger).is_some_and(|ix| b.ledgers.name[ix.get()] == name))
}

fn merged(c: &Commit, kind: MergeKind) -> bool {
    c.merged.is_some_and(|m| m.kind == kind)
}

fn tip(s: &Session, branch: &str) -> Option<CommitId> {
    s.repo.branches().ok()?.into_iter().find(|(n, _)| n == branch).map(|(_, id)| id)
}

// --------------------------------------------------------------- chapters

pub fn chapters() -> Vec<Chapter> {
    vec![
        welcome(),
        getting_around(),
        first_entries(),
        ledgers(),
        issuers(),
        calendar_buckets(),
        views(),
        history(),
        wrap_up(),
    ]
}

fn welcome() -> Chapter {
    Chapter {
        id: "welcome",
        title: "How the tutorial works",
        budget: "household",
        intro: "A walk through everything Ledgit does, on sample budgets built for it.",
        steps: vec![Step {
            id: "welcome.how",
            title: "This panel",
            about: "Each chapter opens its own fresh copy of a sample budget, kept apart from your real ones, so nothing you do here touches them. \"Restart chapter\" builds it again from scratch.\n\nEach step says what a feature is, what to try, and what you should see. Steps that ask you to do something tick themselves off when you have done it.",
            try_it: "Under \"How did it go?\" mark each step Works or Problem, and write what looked wrong. Your notes are saved as you type, to tutorial-feedback.md - \"Copy feedback\" puts the whole report on the clipboard to send back.",
            expect: "This panel on the right, a sample budget open behind it (its name is in the title bar), and the Dashboard showing.",
            go: Go::Screen(Screen::Dashboard),
            check: None,
        }],
    }
}

fn getting_around() -> Chapter {
    Chapter {
        id: "getting-around",
        title: "Getting around",
        budget: "household",
        intro: "Fifteen months of an ordinary household budget: a split paycheque, a mortgage, a car loan, a card paid by statement, a wedding being planned.",
        steps: vec![
            Step {
                id: "around.dashboard",
                title: "The dashboard",
                about: "What you see on opening a budget: alerts that are firing, how each pace is going this period, a tile per bucket (with what is available once payments already decided are taken off), pinned ledgers, and what the issuers owe soon.",
                try_it: "Read down the dashboard. Click a bucket tile's \"open\".",
                expect: "An alert on chequing, a groceries pace, six bucket tiles - some showing an \"available\" line under the total - and a banner saying changes are not committed.",
                go: Go::Screen(Screen::Dashboard),
                check: None,
            },
            Step {
                id: "around.fresh",
                title: "Fresh through",
                about: "The top bar says how current the budget is: the newest transaction, and how far the issuers have been run. It turns red when an issuer owes something dated before today - the balances on screen are then not quite true.",
                try_it: "Find \"Fresh through ...\" in the top bar. Hover it.",
                expect: "Red: this budget's issuers are about six weeks behind.",
                go: Go::Stay,
                check: None,
            },
            Step {
                id: "around.search",
                title: "Search",
                about: "One box searches ledgers, transactions, issuers and buckets at once, from any screen.",
                try_it: "Type \"groceries\" into the search box in the top bar. Then clear it.",
                expect: "Results grouped by kind; clearing the box brings back the screen you were on.",
                go: Go::Stay,
                check: Some(|s, _| !s.search.trim().is_empty()),
            },
            Step {
                id: "around.nav",
                title: "Screens, and back",
                about: "The list on the left is every screen. Alt+Left (or the mouse's back button) goes back to the screen before.",
                try_it: "Visit a few screens from the left, then press Alt+Left.",
                expect: "Back returns to the previous screen each time.",
                go: Go::Stay,
                check: None,
            },
            Step {
                id: "around.zoom",
                title: "Zoom",
                about: "The whole interface scales from 50% to 300%.",
                try_it: "Ctrl + mouse wheel, or Ctrl + and Ctrl -. Ctrl 0 resets. Off 100%, click the zoom figure in the status bar to reset.",
                expect: "Everything scales together; nothing clips or overlaps at 150%.",
                go: Go::Stay,
                check: None,
            },
            Step {
                id: "around.file",
                title: "The File menu",
                about: "The logo in the top left is the File menu: a new budget, open one, the recent ones, close, and this tutorial.",
                try_it: "Click the logo and look - don't pick anything that leaves this budget.",
                expect: "New budget..., Open budget..., Open recent, Close budget, Tutorial.",
                go: Go::Stay,
                check: None,
            },
        ],
    }
}

fn first_entries() -> Chapter {
    Chapter {
        id: "first-entries",
        title: "First entries",
        budget: "starter",
        intro: "A fresh budget with nine ledgers and two opening balances, committed. Everything you add here is staged - nothing is permanent until you commit.",
        steps: vec![
            Step {
                id: "entries.ledger",
                title: "A new ledger",
                about: "Ledgers are named with colons to sit in a tree, as in hledger: Expenses:Car:Insurance sits under Expenses:Car. Ledgers can never be deleted.",
                try_it: "New (top bar) > Ledger. Put it under Expenses:Car, name it Insurance, debit-normal. Stage it.",
                expect: "A note that it was staged, and the Ledgers screen listing Expenses:Car:Insurance.",
                go: Go::Screen(Screen::Ledgers),
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::CreateLedger { .. }))),
            },
            Step {
                id: "entries.tree",
                title: "The ledger tree",
                about: "The tree shows a subtotal on every level. \"+\" on a level starts a ledger under it; \"move\" renames a whole subtree, and buckets follow it.",
                try_it: "On Ledgers, fold and unfold a level; switch to List and back to Tree.",
                expect: "Indented levels with subtotals; Expenses has Car, Groceries, Rent and Tax under it.",
                go: Go::Screen(Screen::Ledgers),
                check: None,
            },
            Step {
                id: "entries.simple",
                title: "A transaction",
                about: "Every entry debits what receives value and credits what gives it. A purchase on the card debits Groceries and credits Visa.",
                try_it: "New > Transaction: Groceries, 84.20, debit Expenses:Groceries, credit Liabilities:Visa. Stage it.",
                expect: "The form shows \"balanced\" before you can stage it; the dashboard and Ledgers update at once.",
                go: Go::Stay,
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::PostTransaction { legs, reverses: None, .. } if legs.len() == 2))),
            },
            Step {
                id: "entries.split",
                title: "A split entry",
                about: "One entry can have any number of sides. A paycheque debits chequing and the tax withheld, and credits gross pay - one entry, not three.",
                try_it: "New > Transaction: Paycheque. Debit Assets:Bank:Chequing 1800, add a side: debit Expenses:Tax:Income Tax 400, credit Income:Salary with the amount left blank, then \"balance the last side\". Stage it.",
                expect: "The running \"out by\" readout reaching zero; the entry listed with three sides.",
                go: Go::Stay,
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::PostTransaction { legs, .. } if legs.len() > 2))),
            },
            Step {
                id: "entries.variable",
                title: "Variables",
                about: "A variable is a number or some text to use in entries: amounts can be formulas over numbers, names can include text.",
                try_it: "Variables screen: set Fuel_Price to 1.65 and stage it.",
                expect: "Fuel_Price listed as a number.",
                go: Go::Screen(Screen::Variables),
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::SetVariable { .. }))),
            },
            Step {
                id: "entries.formula",
                title: "A formula",
                about: "An amount can be a formula: 40 * Fuel_Price, 5%, (1850-400)/2. It is worked out when you stage the entry, so changing the variable later changes no posted entry.",
                try_it: "New > Transaction: Fuel, amount 40 * Fuel_Price, debit Expenses:Car:Fuel, credit Liabilities:Visa. Stage it.",
                expect: "The form shows what the formula comes to (66.00) as you type.",
                go: Go::Stay,
                check: Some(|s, b| staged(s, b, |op, l| matches!(op, Op::PostTransaction { legs, .. } if names_ledger(l, legs, "Expenses:Car:Fuel")))),
            },
            Step {
                id: "entries.report",
                title: "The commit report",
                about: "Before anything is permanent, the Commit screen shows everything staged and what it does: every ledger it moves, every bucket that may be affected, alerts it sets off, paces it breaks, and money it speaks for.",
                try_it: "Open the Commit screen from the list on the left, and read the report.",
                expect: "Your entries listed in order, the ledgers they move with before and after balances.",
                go: Go::Stay,
                check: Some(|s, _| s.view == Screen::Commit),
            },
            Step {
                id: "entries.edit",
                title: "Edit, drop and undo",
                about: "A staged change can be reopened in its form (\"edit\") or dropped. Ctrl+Z undoes the last change to the staging area; Ctrl+Y redoes it.",
                try_it: "Click \"edit\" on the groceries entry and change the amount. Then \"drop\" one entry, and press Ctrl+Z to get it back.",
                expect: "The edit replaces the entry in place; the dropped one comes back on undo.",
                go: Go::Screen(Screen::Commit),
                check: None,
            },
            Step {
                id: "entries.commit",
                title: "Commit",
                about: "Committing turns the staging area into one commit in the history. That is the only thing that makes a change permanent.",
                try_it: "Write a message and commit.",
                expect: "The report empties; History shows your commit on top.",
                go: Go::Screen(Screen::Commit),
                check: Some(|s, b| committed(s, b, |_| true)),
            },
        ],
    }
}

fn ledgers() -> Chapter {
    Chapter {
        id: "ledgers",
        title: "Ledgers, registers and goals",
        budget: "household",
        intro: "Back to the household budget, fresh.",
        steps: vec![
            Step {
                id: "ledgers.list",
                title: "Posted and available",
                about: "Posted is what a ledger's entries add up to. Available also takes off payments already decided: ones scheduled from an entry or for a later date, and a card statement once it has closed. Recurring payments are not counted - next month's rent is a forecast, not a decision.",
                try_it: "On Ledgers, sort by balance. Pin a ledger with the star.",
                expect: "An \"available\" figure only where money is held back - chequing, here. The pinned ledger appears on the dashboard.",
                go: Go::Screen(Screen::Ledgers),
                check: Some(|s, b| s.pins != b.pins),
            },
            Step {
                id: "ledgers.register",
                title: "A ledger's page",
                about: "Every posting against the ledger with a running balance, its targets and alerts, and - when money is held back - what holds it.",
                try_it: "Read Chequing's page: posted, available, and the \"Spoken for\" list.",
                expect: "The tuxedo fitting paid off later, the photographer deposit, and Visa statement payments listed as holding money back.",
                go: Go::Ledger("Assets:Bank:Chequing"),
                check: None,
            },
            Step {
                id: "ledgers.goals",
                title: "Targets, paces and alerts",
                about: "A target is a balance to reach; a pace is a limit per period (\"250 a week at most\"); an alert fires when a balance passes a level.",
                try_it: "On Restaurants, set a pace of 200 a month at most, and an alert below or above some level. Stage it.",
                expect: "The page shows this month against the pace and past months; a broken pace shows on the Commit screen.",
                go: Go::Ledger("Expenses:Food:Restaurants"),
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::SetLedgerGoals { .. }))),
            },
            Step {
                id: "ledgers.reverse",
                title: "Reverse one entry",
                about: "Nothing is ever deleted. To take an entry back, post its reversal - every side negated - so the mistake and the correction both stay on record.",
                try_it: "On the register, press \"Reverse\" on any entry.",
                expect: "A staged \"Reversal of ...\"; in the list the original is marked reversed and the new one reversal.",
                go: Go::Ledger("Expenses:Food:Restaurants"),
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::PostTransaction { reverses: Some(_), .. }))),
            },
            Step {
                id: "ledgers.transactions",
                title: "The transaction list",
                about: "Every entry in the book, with date range, ledger, split and source filters.",
                try_it: "Filter to one ledger and the last month; show only entries made by issuers.",
                expect: "The count and total under the filters follow them.",
                go: Go::Screen(Screen::Transactions),
                check: None,
            },
        ],
    }
}

fn issuers() -> Chapter {
    Chapter {
        id: "issuers",
        title: "Issuers, statements and available",
        budget: "household",
        intro: "Issuers make transactions on a schedule. They never post on their own: what they owe is staged for you to review.",
        steps: vec![
            Step {
                id: "issuers.run",
                title: "Run the issuers",
                about: "This budget's issuers are about six weeks behind. Running them stages every transaction they owe, priced in date order - interest on the balance as it will stand, the card statement as it closed.",
                try_it: "Issuers screen: \"Stage what is owed\" through today.",
                expect: "A note saying how many were staged; the top bar's \"Fresh through\" no longer red.",
                go: Go::Screen(Screen::Issuers),
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::AdvanceIssuer { .. }))),
            },
            Step {
                id: "issuers.statement",
                title: "A statement issuer",
                about: "The Visa is paid by an issuer that knows it is paying a statement: on the 10th it pays what the card stood at when the statement closed on the 25th, less anything paid since. A minimum - the greater of $10 or 2% - limits how low a payment can go.",
                try_it: "Read Visa statement's upcoming payments.",
                expect: "Columns: due, pays, closed, statement, paid since, minimum. One month already set ahead to $400.",
                go: Go::Issuer("Visa statement"),
                check: None,
            },
            Step {
                id: "issuers.override",
                title: "Set an amount ahead",
                about: "Any one payment can be set ahead of time - pay less this month, more next. A statement is never paid below its minimum: set less, and the minimum is paid.",
                try_it: "\"Set amount\" on a later month, type 5, and Set.",
                expect: "The amount shown in red - under the minimum - and \"pays\" showing the minimum instead.",
                go: Go::Issuer("Visa statement"),
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::SetIssuerOverride { .. }))),
            },
            Step {
                id: "issuers.pause",
                title: "Pause and resume",
                about: "Issuers can be paused - they stop making transactions - but never deleted.",
                try_it: "Pause one issuer, then resume it.",
                expect: "A paused issuer shows struck through and posts nothing when run.",
                go: Go::Screen(Screen::Issuers),
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::SetIssuerPaused { .. }))),
            },
            Step {
                id: "issuers.rule",
                title: "An issuer with a rule",
                about: "An issuer's amount can follow a balance: interest at an APR on a loan, a share of an account, or a statement of a card.",
                try_it: "New > Issuer. Amount: Interest (APR) - 6.45% on Liabilities:Car Loan, debit an interest expense, credit the loan. Monthly. Stage it.",
                expect: "The form says what it would charge on today's balance.",
                go: Go::Stay,
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::CreateIssuer { rule: Some(_), .. }))),
            },
            Step {
                id: "issuers.pay_later",
                title: "Post now, pay later",
                about: "An entry can schedule its own payoff: bought on the card today, paid from chequing on a day you pick. Until it is paid, that money is held back from what is available.",
                try_it: "New > Transaction: Dentist, 300, debit Expenses:Health, credit Liabilities:Credit Card. Under When pick \"Post now, pay later\", pay from Assets:Bank:Chequing in two weeks. Stage it.",
                expect: "Two changes staged: the entry, and a one-off issuer \"Pay off Dentist\". Chequing's available drops by 300.",
                go: Go::Stay,
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::CreateIssuer { settles: Some(_), .. }))),
            },
            Step {
                id: "issuers.post_later",
                title: "Post on its date",
                about: "Or nothing posts now: the whole entry is scheduled for a later date, and posts when issuers are run on or after it.",
                try_it: "New > Transaction with a date next month; When: \"Post on its date\". Stage it.",
                expect: "A one-off issuer staged, and the money held back from available until then.",
                go: Go::Stay,
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::CreateIssuer { schedule: Schedule::Once, settles: None, rule: None, .. }))),
            },
            Step {
                id: "issuers.minimum",
                title: "Editing a statement payment",
                about: "A staged statement payment can be edited down, but not under its minimum.",
                try_it: "On Commit, \"edit\" a staged Visa statement payment, set it to 1, and try to stage it.",
                expect: "Refused, saying the minimum. Cancel the edit.",
                go: Go::Screen(Screen::Commit),
                check: None,
            },
            Step {
                id: "issuers.available",
                title: "Available everywhere",
                about: "Money already spoken for shows wherever balances do.",
                try_it: "Look at the Commit screen's \"Money spoken for\", Ledgers' available column, and the dashboard's pinned ledgers and bucket tiles.",
                expect: "The dentist payment and the scheduled entry listed as holding money back on chequing.",
                go: Go::Screen(Screen::Commit),
                check: None,
            },
        ],
    }
}

fn calendar_buckets() -> Chapter {
    Chapter {
        id: "calendar-buckets",
        title: "Calendar and buckets",
        budget: "household",
        intro: "Two ways to read the budget whole: when things fall due, and groups of ledgers.",
        steps: vec![
            Step {
                id: "calendar.month",
                title: "The calendar",
                about: "What every issuer costs per day, week, month and year, and a month of due dates marked posted, overdue, upcoming or paused.",
                try_it: "Open Calendar. Step back a month, then switch to List.",
                expect: "Overdue payments in red - this budget is behind - and the month's totals by state underneath.",
                go: Go::Screen(Screen::Calendar),
                check: Some(|s, _| s.view == Screen::Calendar && s.calendar_list),
            },
            Step {
                id: "buckets.read",
                title: "A bucket",
                about: "A bucket totals a group of ledgers: netting assets against liabilities, or as a plain sum. Ledgers can be in many buckets; buckets do not nest - to total several, use a view.",
                try_it: "Read Net Worth. Switch between net worth and plain sum, sort by balance, and look at its targets.",
                expect: "The total, each member's contribution, and the targets of its members added up.",
                go: Go::Bucket("Net Worth"),
                check: None,
            },
            Step {
                id: "buckets.subtree",
                title: "A bucket that grows",
                about: "A bucket can include a whole subtree: everything under Wedding, now and later.",
                try_it: "New > Bucket called Car. Then add the subtree Liabilities:Car Loan - or any level - with the picker on its page.",
                expect: "The subtree listed on the bucket, and its ledgers counted.",
                go: Go::Screen(Screen::Buckets),
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::AddSubtreeToBucket { .. } | Op::AddToBucket { .. }))),
            },
        ],
    }
}

fn views() -> Chapter {
    Chapter {
        id: "views",
        title: "Views",
        budget: "household",
        intro: "A view charts buckets and ledgers across time and simulates the issuers forward.",
        steps: vec![
            Step {
                id: "views.chart",
                title: "A view",
                about: "Solid lines up to today, dashed after - the simulation. A dotted line is a target. Click a line in the legend to hide it.",
                try_it: "Hover the chart; hide a line from the legend.",
                expect: "Hover shows the date, the amount, and \"(projected)\" after today.",
                go: Go::View("Net worth"),
                check: None,
            },
            Step {
                id: "views.edit",
                title: "What a view looks at",
                about: "Pick buckets (added or subtracted), ledgers, issuers, filters, how far back and ahead. The chart follows the draft as you pick; nothing is staged until you say.",
                try_it: "Add a ledger with the picker, change the lookback, then \"Stage changes\".",
                expect: "The chart redrawing on every pick.",
                go: Go::View("Net worth"),
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::EditView { .. }))),
            },
            Step {
                id: "views.compare",
                title: "Compare with history",
                about: "Lay the view as it was at an earlier commit over the one now: how much sooner is the car paid off since the lump sum?",
                try_it: "Open Debt payoff, and under \"Compare with\" pick an earlier commit.",
                expect: "A faded dotted line for each series, and a comparison table.",
                go: Go::View("Debt payoff"),
                check: Some(|s, _| s.view_compare.is_some()),
            },
            Step {
                id: "views.available",
                title: "Posted and available, charted",
                about: "A view can show each line as available too: below the balance by what is spoken for, meeting it as each payment posts.",
                try_it: "Open Spending money. Find the available line and column.",
                expect: "A thin dashed line below chequing that rejoins it as the scheduled payments land.",
                go: Go::View("Spending money"),
                check: None,
            },
            Step {
                id: "views.more",
                title: "Simulate, calendar, export",
                about: "Simulate until any date; open the issuers the view breaks down as a calendar; save the chart as PNG or SVG and the series as CSV.",
                try_it: "Set \"Simulate until\" a year out. Open the Calendar section. Try \"Save chart...\".",
                expect: "The chart reaching the date; the calendar showing only this view's issuers.",
                go: Go::View("Subscriptions"),
                check: None,
            },
            Step {
                id: "views.new",
                title: "A new view",
                about: "Views are versioned like everything else, so they travel with the budget and can differ per branch.",
                try_it: "New view, starting with every bucket. Stage it.",
                expect: "The new view selected with all buckets charted.",
                go: Go::Screen(Screen::Views),
                check: Some(|s, b| staged(s, b, |op, _| matches!(op, Op::CreateView { .. }))),
            },
        ],
    }
}

fn history() -> Chapter {
    Chapter {
        id: "history",
        title: "History, branches and merges",
        budget: "repairs",
        intro: "A small budget with history to rewrite: a typo to revert, a month to repair on a branch, a what-if to adopt, and an old plan to bring back.",
        steps: vec![
            Step {
                id: "history.graph",
                title: "The history",
                about: "Every commit and branch, as lanes. Pick a commit to see what it did.",
                try_it: "Click a few commits. Note the branches: fix-month, new-car, old-plan.",
                expect: "Each branch in its own colour; commit detail listing its operations.",
                go: Go::Screen(Screen::History),
                check: None,
            },
            Step {
                id: "history.revert",
                title: "Revert a commit",
                about: "\"Groceries at Costco\" was typed as 1,500 for 150. Reverting posts the mirror image of every entry in the commit; both stay on record.",
                try_it: "With that commit selected, \"Revert this commit\", then commit the reversal.",
                expect: "A \"Reversal of Groceries at Costco\" staged, then committed.",
                go: Go::Commit("Groceries at Costco"),
                check: Some(|s, b| committed(s, b, |c| c.ops.iter().any(|op| matches!(op, Op::PostTransaction { reverses: Some(_), .. })))),
            },
            Step {
                id: "history.branch",
                title: "Branch from a commit",
                about: "A branch starts from any commit: the budget exactly as it stood then. Switching with changes staged asks whether to shelve them on the branch you leave (they come back when you return) or bring them along.",
                try_it: "Select \"Open the books\", type a name under Branch from here, and \"Create and switch\". Look around, then switch back to main.",
                expect: "On the branch, only the opening balances; back on main, everything.",
                go: Go::Commit("Open the books"),
                check: Some(|s, b| s.repo.branches().unwrap_or_default().len() > b.branches.len()),
            },
            Step {
                id: "history.cherry",
                title: "Cherry-pick",
                about: "Cherry-picking stages another commit's changes on the branch you are on, exactly as they were - the same entries, so a later merge knows them.",
                try_it: "Switch to the branch you made, select \"Venue deposit\" and \"Cherry-pick onto\" it. Commit, and switch back to main.",
                expect: "The venue deposit staged on your branch; skipped as \"already here\" if you pick it twice.",
                go: Go::Commit("Venue deposit"),
                check: Some(|s, _| s.repo.head().branch_name().is_some_and(|n| n != DEFAULT_BRANCH) && !s.repo.staged().is_empty()),
            },
            Step {
                id: "history.reconcile",
                title: "Reconcile a repair",
                about: "Last month's bonus was typed as 5,000 for 500, and the 2% sweep moved 2% of the inflated balance. fix-month branched from before it, cherry-picked the month, reversed the bonus, entered 500 and re-ran the sweep.\n\nReconcile lists what the branches disagree on. Only on main: keep or revert. Only on the branch: keep or drop. On ledgers both changed - a clash - you must choose: Force (keep both), Revert (reverse main's, take the branch's) or Drop (keep main's).",
                try_it: "On main, History > Merge into MAIN: pick fix-month, Reconcile. On the clash choose Revert. Keep the venue deposit and the florist quote. Merge.",
                expect: "Merge stays disabled until the clash is decided; the balances it will change listed underneath; afterwards, the bad bonus and its reversal both in main's history.",
                go: Go::Screen(Screen::History),
                check: Some(|s, b| committed(s, b, |c| merged(c, MergeKind::Reconcile))),
            },
            Step {
                id: "history.delete",
                title: "Delete the branch",
                about: "After a merge, History offers to delete the branch you merged. Its commits stay in the merge's record.",
                try_it: "\"Delete branch fix-month\" on the banner.",
                expect: "fix-month gone from the branch list; the merge commit still says where it came from.",
                go: Go::Screen(Screen::History),
                check: Some(|s, _| tip(s, "fix-month").is_none()),
            },
            Step {
                id: "history.adopt",
                title: "Adopt a what-if",
                about: "new-car is a plan: a car loan, its payments, a bigger flat, a view. Adopt brings its issuers and settings - never its transactions; no balance moves. Its \"Rent, new place\" posts to the same ledgers as main's Rent: keep both, keep main's, or take the branch's (main's is paused - issuers are never deleted).",
                try_it: "Merge new-car with Adopt. For the rent choose \"Take the other's\". Merge.",
                expect: "New car payment and Rent, new place on Issuers; main's Rent paused; the Car budget view; no balance changed.",
                go: Go::Screen(Screen::History),
                check: Some(|s, b| committed(s, b, |c| merged(c, MergeKind::Adopt))),
            },
            Step {
                id: "history.rebase",
                title: "Rebase",
                about: "old-plan branched off two months ago. Rebasing replays its commits on top of main as it is now.",
                try_it: "Switch to old-plan, type main under Rebase and press Rebase. Switch back to main.",
                expect: "old-plan's lane now starting from main's latest commit.",
                go: Go::Screen(Screen::History),
                check: Some(|s, _| match (tip(s, "old-plan"), tip(s, DEFAULT_BRANCH)) {
                    (Some(old), Some(main)) => s.repo.merge_base(main, old).ok().flatten() == Some(main) && old != main,
                    _ => false,
                }),
            },
            Step {
                id: "history.replace",
                title: "Replace",
                about: "Replace makes main end exactly like the other branch - by reversing what differs and posting what is missing, never by deleting. When main has nothing the branch lacks, it simply moves up to it.",
                try_it: "On main, merge old-plan with Replace.",
                expect: "\"Move main up to old-plan\": main now has the lump sum on the car.",
                go: Go::Screen(Screen::History),
                check: Some(|s, b| s.repo.head_commit().ok().flatten() != b.head && s.repo.head_commit().ok().flatten() == tip(s, "old-plan")),
            },
        ],
    }
}

fn wrap_up() -> Chapter {
    Chapter {
        id: "wrap-up",
        title: "Sending feedback",
        budget: "household",
        intro: "That is everything.",
        steps: vec![Step {
            id: "wrap.feedback",
            title: "Your notes",
            about: "Every step's verdict and note is in tutorial-feedback.md, in the folder shown below.",
            try_it: "\"Copy feedback\" and paste it where you keep notes for the next build - or open the file.",
            expect: "One section per chapter, each step marked works, problem or not tried.",
            go: Go::Stay,
            check: None,
        }],
    }
}
