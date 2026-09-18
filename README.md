# Ledgit

A double-entry budget with version control. Every change is a commit; mistakes
are undone by posting reversing entries, never by deleting records; what-if
budgets are branches.

```
crates/ledgit-core     the library: domain model, operations, commit DAG, queries
crates/ledgit-sqlite   SQLite-backed storage
crates/ledgit-cli      `ledgit`, the command line front end
crates/ledgit-gui      `ledgit-gui`, the desktop app
installer/             Inno Setup script producing a Windows installer
docs/                  architecture, design review, roadmap
```

Start with [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how it works and
[docs/DESIGN-REVIEW.md](docs/DESIGN-REVIEW.md) for why it does not look like the
original spec.

## Build

```sh
cargo build --release      # Windows or Linux
cargo test                 # 69 tests
cargo run -p ledgit-gui        # the desktop app
```

Needs Rust 1.88 or newer (`rustup update`). SQLite is compiled from source by
`rusqlite`'s `bundled` feature and the GUI draws through OpenGL, so a Windows
build needs only the MSVC build tools `rustup` already asks for - no DLL beside
the exe, no WebView2, no redistributable.

To build the Windows installer, see [installer/README.md](installer/README.md).

## Quickstart

```sh
export LEDGIT_BUDGET=~/budget.ledgit      # or pass --file every time
ledgit init

ledgit ledger add "Chequing"  --normality debit  --opened 2024-01-01
ledgit ledger add "Car Loan"  --normality credit --opened 2024-01-01
ledgit ledger add "Salary"    --normality credit --opened 2024-01-01
ledgit commit -m "open the books"

ledgit post "January pay" 2400.00 --debit Chequing --credit Salary --date 2024-01-05
ledgit bucket add "Net Worth"
ledgit bucket include "Net Worth" Chequing
ledgit bucket include "Net Worth" "Car Loan"

ledgit status          # everything staged, and every bucket it touches
ledgit commit -m "january pay"
```

Which ledger to debit and which to credit: **debit what receives value, credit
what gives it.** Your pay makes chequing bigger, so chequing is debited and
salary is credited.

Ledger normality decides which direction reads as positive. Assets and expenses
are `debit`-normal; liabilities, income and equity are `credit`-normal, so a
credit card with $500 owed shows as `500.00`, not `-500.00`.

## Recurring payments

```sh
ledgit issuer add "Car payment" 400 \
    --debit "Car Loan" --credit Chequing \
    --every biweekly --start 2024-01-12

# Splits work here too - a recurring paycheque keeps its tax and pension legs.
ledgit issuer add "Salary" \
    --debit Chequing:1800 --debit "Tax withheld":600 --credit "Gross pay" \
    --every biweekly --start 2024-01-05

ledgit issuer run --through 2024-03-01   # stages what is owed; posts nothing
ledgit status                            # review it
ledgit commit -m "february car payments"
```

Issuers *propose*. They never post on their own - their transactions land in the
staging area next to your manual entries and wait for you to approve them.
Schedules: `daily`, `weekly`, `biweekly`, `14d`, `monthly`, `monthly:15`,
`quarterly:1`, `yearly`, `once`.

## Version control

```sh
ledgit log                          # history, newest first
ledgit show HEAD~2                  # one commit, op by op
ledgit revert <commit>              # stages the reversing entries
ledgit branch what-if-payoff        # branch the budget
ledgit checkout -b what-if-payoff
ledgit rebase what-if-payoff --onto main
ledgit verify                       # re-hash every commit in the file
```

`ledgit revert` never deletes. It posts a mirror-image transaction - debit and
credit swapped - so both the mistake and its correction stay in the register,
which is what an auditor expects to see and what makes the balance trustworthy.

Things with no inverse come back as a note rather than a lie: ledgers stay
open, and reverting a commit that created an issuer pauses it instead.

## Reading the budget

```sh
ledgit ledger list --sort balance
ledgit register Chequing            # every posting, with a running balance
ledgit bucket show "Net Worth"      # totals, netting assets against liabilities
ledgit bucket show "Spending" --sum # or just add every member up
ledgit search rent                  # ledgers, transactions, issuers, buckets
```

## The desktop app

```sh
cargo run -p ledgit-gui -- ~/budget.ledgit    # or pick a file from the welcome screen
```

- **Dashboard** - bucket tiles, pinned ledgers, what the issuers owe, and a
  banner for anything uncommitted.
- **Ledgers / Register** - balances, and every posting against a ledger with
  a running balance and the other side of each entry.
- **Entry form** - a side-by-side leg editor with a live "out by $X" readout and
  a "balance the last side" button, so a split is typed once and checked as you
  go.
- **Transactions** - date range, ledger, split and source filters over the
  whole book.
- **Issuers** - schedules, next due dates, pause and resume, and the button that
  stages what is owed.
- **Commit** - the report: every staged change, every ledger it moves, and
  every bucket that might be affected. Nothing is permanent until you press it.
- **History** - the commit log, branches, revert, and rebase.

Every screen reads the *working* budget: committed history plus whatever is
staged. So an entry shows up in the dashboard and the bucket totals the moment
you make it, and the Commit screen is the only place that talks about what is
permanent.

**Zoom.** `Ctrl` + scroll wheel scales the whole interface between 50% and
300%; `Ctrl` `+` / `Ctrl` `-` step it, and `Ctrl` `0` puts it back to 100%. Once
you are off 100% the current setting shows in the status bar and clicking it
resets. On a trackpad, pinch works too.

Zoom and pinning are UI preferences, stored beside the app rather than in the
budget - neither is a fact about your money, so neither has any business in the
commit history.

## Status

Core, storage, version control, CLI, GUI and installer are built and tested.
See [docs/ROADMAP.md](docs/ROADMAP.md) for what is deliberately deferred and the
open questions.
