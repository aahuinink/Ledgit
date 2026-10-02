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
cargo test                 # 169 tests
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

An issuer's amount can follow a balance instead: interest on a loan, or a
share of an account, worked out each time it fires.

```sh
ledgit issuer add "Loan interest" --debit Interest --credit "Car Loan" --apr 6.45 --every monthly:1
ledgit issuer add "Sweep" --debit Investments --credit Savings --share 5 --every monthly:28
```

Issuers *propose*. They never post on their own - their transactions land in the
staging area next to your manual entries and wait for you to approve them.
Schedules: `daily`, `weekly`, `biweekly`, `14d`, `monthly`, `monthly:15`,
`quarterly:1`, `yearly`, `once`.

### Statements, payments scheduled from an entry, and amounts set ahead

An issuer can stand for someone you owe, or who owes you. A card that closes
on the 25th and is paid on the 10th pays what the statement closed at, less
anything paid since; whatever is left stays on the card.

```sh
ledgit issuer add "Visa" --debit Visa --credit Chequing \
    --statement Visa --close 25 --every monthly:10 --min 10 --min-percent 2
ledgit issuer upcoming Visa                     # each statement: closed, owed, minimum
ledgit issuer override Visa 2026-11-10 300      # pay 300 that month; never under the minimum
ledgit issuer override Visa 2026-11-10 --clear  # back to the statement

# Bought today, paid off from chequing on the 15th:
ledgit post "Vet" 1200 --debit Vet --credit Visa --pay-from Chequing --pay-on 2026-10-15
# Nothing now; the whole entry posts on its date:
ledgit post "Couch" 900 --debit Furniture --credit Chequing --date 2026-11-02 --later
```

### Posted and available

Money already spoken for comes off what you can spend. A ledger's *posted*
balance is its entries; its *available* balance also takes off payments
already decided - ones scheduled from an entry or for a later date, and a
statement payment once its statement has closed - until they post.
Recurring issuers are not counted: next month's rent is a projection, not a
decision.

```sh
ledgit ledger list           # posted and available side by side
ledgit committed Chequing    # what is held back, and why
ledgit view add Cash --ledger Chequing --available true
```

## Variables, targets and alerts

```sh
ledgit var set Car_Km_Rate 0.68
ledgit var set Home Toronto
ledgit post "Mileage to {Home}" "200*Car_Km_Rate" --debit Travel --credit Owed

ledgit ledger goals "Car Loan" --target 0
ledgit ledger goals Groceries --target 250 --per week           # a budget: at most
ledgit ledger goals Savings --target 500 --per month --at-least # a habit
ledgit ledger goals Chequing --below "500:Top up from savings"
ledgit view show "Debt payoff" --compare <commit>   # how much sooner is it paid off?
```

A formula is worked out when the entry is made; changing the variable later
re-prices nothing already posted.

A target is either a balance to reach or a *pace*: how far the balance may
(at most) or should (at least) move in each calendar day, week, month or
year. Weeks start on Monday. `ledgit status` shows how each pace's current
period is going, and which paces the staged entries would break.

## Version control

```sh
ledgit log                          # history, newest first
ledgit show HEAD~2                  # one commit, op by op
ledgit revert <commit>              # stages the reversing entries
ledgit branch what-if-payoff        # branch the budget
ledgit checkout -b what-if-payoff    # staged work comes along
ledgit checkout main --shelve       # leave staged work on this branch for later
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

## The ledger tree

Name a ledger with colons and it sits in a tree, as in hledger:

```sh
ledgit ledger add "Wedding:Tuxedo" --normality debit
ledgit ledger add "Wedding:Venue"  --normality debit
ledgit post "Tux rental" 450 --debit Tuxedo --credit Chequing   # a unique last segment is enough
ledgit post "Deposit" --debit Wedding:Venue:3000 --credit Chequing
ledgit ledger tree                        # indented, with a subtotal on every level
ledgit bucket include-tree Wedding Wedding   # everything under Wedding, now and later
ledgit ledger move Wedding Events:Wedding    # rename a whole subtree; buckets follow
```

## Views

A view charts buckets and ledgers across time and simulates your issuers
forward. It is also how you total several buckets at once (`--plus` some,
`--minus` others) and how you group issuers: name the ones it should break
down.

```sh
ledgit view add "Net worth" --plus Liquid --plus Debt --lookback 6m --horizon 2y
ledgit view add "Bills" --plus Liquid --issuer Rent --issuer Hydro --only-selected true
ledgit view show "Net worth"             # balances, flows, month by month
ledgit view show "Net worth" --until 2030-01-01 --chart nw.png --csv nw.csv

ledgit issuer rates --view Bills         # each issuer per day, week, month, year
ledgit issuer calendar --view Bills --to 2026-12-31   # every due date, overdue flagged
```

Leave `--view` off `issuer rates` and `issuer calendar` to see every issuer.

Views are staged and committed like everything else, so they travel with the
file and can differ between branches. Showing one is a read: the simulation
never stages a thing.

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
  stages what is owed. Pick one to see its next payments - for a statement,
  what it closed at, what was paid since and the minimum - and set any of
  them ahead.
- **Entry form** "When": post now, post now and pay it off later, or post the
  whole entry on its date. A staged statement payment can be edited down,
  never under its minimum.
- **Posted / Available** on the Ledgers screen, a ledger's page (with what
  holds money back), pinned ledgers and bucket tiles, the Commit screen
  ("money spoken for"), and as a dashed line in any view that asks for it.
- **Buckets** - totals, targets and paces over a group of ledgers. Buckets do
  not nest; to total several at once (`Cash - Receivables`), add them to a
  view.
- **Calendar** - what every issuer costs per day, week, month and year, and a
  month of due dates marked posted, overdue, upcoming or paused.
- **Views** - balances across time with the issuers simulated forward, flows
  per period, and a calendar of the issuers the view breaks down.
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
