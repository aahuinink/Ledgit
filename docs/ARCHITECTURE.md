# Architecture

## The one idea

Every change to the budget is an **operation** - a plain, serialisable value.
A **commit** is a list of operations plus its parents, identified by the SHA-256
of its own content. The budget you look at is not stored anywhere; it is
**derived**, by folding operations in DAG order into columnar arenas.

```
                 ops                       fold
  user action  ------->  staging area  ------------>  working budget  ---> UI
                              |                            ^
                           commit                          |
                              v                            |
                        commit DAG  --------- fold --------+
                        (on disk)
```

Everything in the feature list falls out of that:

| Requirement | Implementation |
|---|---|
| Undo a mistake without deleting | append the inverse ops (a reversing entry) |
| What-if budgets | a second ref into the same DAG |
| Rebase | replay a branch's ops onto a different parent |
| "Nothing saved until I press commit" | the staging area is a `Vec<Op>` |
| Pre-commit report | fold the staged ops, diff the balance columns |
| Swap the database | persist four kinds of blob, not a schema |
| Ledgers/transactions/issuers are never deleted | there is no delete op for them |
| Split entries (a paycheque) | one op with N legs summing to zero |
| Cohorts, saved views | ops, like buckets; read by pure functions, results never stored |
| A ledger tree (`Wedding:Tuxedo`) | paths in names; the tree is derived, never stored |

## Crates

```
ledgit-core     domain types, ops, commit DAG, queries, the Repo API.  No I/O.
ledgit-sqlite   implements ledgit_core::store::Store against a SQLite file.
ledgit-plot     exported charts: SVG by hand, PNG through resvg. CSV too.
ledgit-cli      `ledgit` - a front end, and the fastest way to exercise the core.
ledgit-gui      the desktop app (egui); live charts through egui_plot.
```

`ledgit-plot` is separate for the same reason `ledgit-sqlite` is: the core
stays at three dependencies, and rasterising text is not a budget concern.

`ledgit-core` has three dependencies (`serde`, `serde_json`, `sha2`) and no
knowledge that SQLite exists. The boundary is a separate crate rather than a
module so it cannot quietly leak.

## Entries have N sides

A transaction is a list of **legs**, each a ledger and a signed amount:

```rust
pub struct Leg {
    pub ledger: LedgerUid,
    pub amount: Money,   // debit-positive: + debits, - credits
}
```

The only rule is that the legs sum to zero. Two legs is an ordinary transfer;
four is a paycheque - gross credited, chequing and tax and pension debited - as
**one** entry, which is what it actually is. Three separate two-legged
transactions would drift apart the first time one of them was edited.

Signed, debit-positive legs mean applying an entry is:

```rust
for leg in legs {
    raw_balance[leg.ledger] += leg.amount;
}
```

No branch on direction, no branch on normality, and reversing an entry is
negating every leg - correct for four sides for exactly the reason it is
correct for two.

The *size* of an entry is what it debits (`magnitude`), not the sum of every
leg: a $2,400 paycheque is $2,400, not $4,800.

## Buckets do not nest

A bucket holds ledgers, never other buckets: `BucketArena.members` is
`Vec<Vec<LedgerIx>>` and the ops are `AddToBucket { bucket, ledger }`. Nesting
would put a graph in the op log, and a graph in the op log has to be checked for
cycles on *every* replay - admit one cycle once and the file never loads again.

Roll-ups across several buckets are a read instead. `query::combine` takes a
slice of signed `Term`s and treats membership as a set:

* in added buckets only - counted once, positive;
* in subtracted buckets only - counted once, negative;
* in both - cancelled, and reported separately rather than dropped.

So `Cash - Receivables` is a question you ask, not an entity you create. Nothing
is stored, nothing needs validating on replay, and a combination naming a
deleted bucket degrades to a warning instead of an unloadable budget.

## The ledger tree lives in the names

`Wedding:Tuxedo` sits under `Wedding` because of its name, exactly as in
hledger. There is no parent field and no op that sets one:

* **No graph in the op log.** A parent pointer would need a cycle check on
  every replay - the reason buckets do not nest. A string prefix cannot form a
  cycle.
* **Levels need not exist.** `Wedding` is a node because something below it
  names it. It holds money only if a ledger is actually called `Wedding`.
* **Reorganising is renaming.** `tree::rename_ops` is a batch of `EditLedger`
  ops; uids, balances and postings never move.

Names are normalised on apply (`tree::normalize`: trim each segment, drop
empty ones), never rejected - it runs on replay, and a budget from before
paths existed may hold a stray colon. Paths compare case-insensitively (ASCII)
and level by level (`tree::path_cmp`): plain string order would put
`Wedding-fund` between `Wedding` and `Wedding:Tuxedo` and split the subtree.

`LedgerTree::build` sorts ledgers by path into `order` and lists nodes depth
first, implied levels included. Every subtree is then a contiguous range of
both arrays, so a subtotal is a sum over one slice of row numbers. A subtree
of one normality totals as its ledgers read; a mixed one (wedding costs and
the gifts that paid for them) totals as the debit-positive net and says so.

**Buckets can hold a subtree.** `AddSubtreeToBucket { path }` includes
everything at or below `path`, including ledgers created or renamed into it
later. The arena keeps what was put in (`explicit` ledgers and `subtrees`)
apart from what the bucket counts (`members`, their union). `apply` recomputes
`members` when a bucket changes and whenever a ledger is created or renamed,
so every reader - roll-ups, combinations, views, the commit report - still
sees one plain list and none of them knows paths exist. Two consequences,
both deliberate:

* removing a ledger that is also under one of the bucket's subtrees removes
  only the individual entry; the subtree still counts it;
* reverting a bucket deletion restores the *subtrees*, not a snapshot of their
  members, so ledgers added under the path since come back too.

`rename_ops` re-points a bucket's subtree along with the ledgers, so moving
`Wedding` to `Events:Wedding` does not quietly empty the Wedding bucket.

## Cohorts: buckets for issuers

A cohort is a named, flat group of issuers - `CohortArena.members` is
`Vec<Vec<IssuerIx>>`, built by `CreateCohort`/`AddToCohort`/... ops that
mirror the bucket ones exactly, deletable because it moves no money. Two
reads cover what a cohort is for (`ledgit_core::cohort`), and both take any
slice of issuers, so a view or the "every issuer" calendar uses them with no
cohort in sight:

* **Rates.** `period::per_period(amount, schedule, period)` converts a
  schedule to any unit in exact rational arithmetic, rounded once at the end.
  Month and year are Gregorian averages (146,097 days per 400 years), so
  $10/day is $70/week and $304.37/month, a monthly $100 is exactly $100 a
  month, and nothing drifts on a round trip. A cohort's total counts only
  running, recurring members; paused ones are totalled separately.
* **Calendar.** `Schedule::dates_between(start, from, to)` lists every
  occurrence in a window, jumping straight to `from` rather than stepping
  there. `cohort::calendar` marks each one against `emitted_through` and
  today: posted, overdue, upcoming, or paused.

A **rate** and a **calendar period** are kept apart on purpose. "Biweekly
$400 is $869.63 a month" is a rate; "March had three paydays" is a calendar
fact. `Period::start_of`/`next_start` carve time into calendar periods, and
only the timeline tables use them.

## Saved views: a reading across time

A `ViewSpec` is plain data inside `CreateView`/`EditView` ops: signed bucket
terms, ledgers, issuers, cohorts, `TxFilter`s, a period, and a **lookback and
horizon as `Span`s** ("6 months back, 1 year ahead"), never as dates - so a
saved view keeps meaning the same thing whenever it is opened. It is
versioned, so it travels with the file and can differ per branch, but it
holds no results: `view::evaluate(budget, spec, today)` recomputes the
`ViewReport` every time, in well under a millisecond.

**Scope and direction.** "Spent" only means something relative to a pot of
money, so a view has a scope - its bucket combination (same set rules as
`query::combine`, shared via `query::members`), or else its ledgers as
displayed. Scope is one integer weight per ledger, and every effect is

```
effect of a posting = weight[ledger] * amount     (debit-positive amount)
```

A ledger outside the scope weighs 0, so a transfer between two of your own
accounts nets to nothing - neither spending nor income - with no special
case. A view with no scope is *undirected* and reports volume only.

**The simulation** stages nothing. It asks `issuer::due_dates` - the same
function that posts issuers for real - for every occurrence still owed up to
the horizon, expands each into its legs, and sweeps those with the real
postings in date order through flat `(date, ledger, amount)` columns. Each
ledger row maps to the series it feeds, so one posting is one index plus a
multiply-add per series. Overdue occurrences are placed on today: they have
not happened, so they must not move a past balance. By default every running
issuer is simulated (the honest forecast); `only_selected_issuers` restricts
it to the view's own issuers and cohorts (the what-if).

What comes out: step **series** (scope total, each bucket, each ledger) with
today, the end, and the lowest point ahead; **flow lines** per selected
issuer, signed against the scope; and a **timeline** of calendar periods
with actual and projected in/out side by side.

**Validation is deliberately partial.** Saving a view checks that its
ledgers and issuers exist, since those can never disappear. It does not check
buckets or cohorts: they are deletable, evaluation already reports a missing
one, and checking would make history order-sensitive - reverting "delete
cohort, delete view" recreates the view before the cohort.

**Charts.** Balances are drawn as steps (a balance holds until the next
posting; a slope would invent money between paydays), solid up to today and
dashed after, over a faint shading of the future. Series colours are one
fixed, colour-blind-checked order, shared by the GUI and the exports.

## "Up to date through"

The top bar reads `through 2026-09-24 · issuers 2026-09-25`:

* the first date is `Budget::latest_transaction_date()` - the newest entry on
  this branch, staged ones included;
* the second is `cohort::caught_up_through()` - the day before the earliest
  occurrence any running issuer still owes, i.e. every recurring payment on
  or before it is in the budget.

It turns red when any running issuer owes something dated before today,
because that is when the balances on screen stop being true.

## Data-oriented layout

`Budget` is struct-of-arrays. Row `i` of every column describes entity `i`:

```rust
pub struct LedgerArena {
    pub uid:         Vec<LedgerUid>,
    pub name:        Vec<String>,
    pub normality:   Vec<Normality>,
    pub raw_balance: Vec<Money>,          // <- the column everything sums
    pub postings:    Vec<Vec<PostingIx>>, // <- per-ledger index
    ...
}
```

The legs themselves live in one flat arena, not in the transaction rows:

```rust
pub struct PostingArena {
    pub ledger: Vec<LedgerIx>,
    pub amount:  Vec<Money>,
    pub tx:      Vec<TxIx>,
}

pub struct TxArena {
    pub date:      Vec<Date>,
    pub leg_start: Vec<u32>,   // half-open range into PostingArena
    pub leg_len:   Vec<u32>,
    ...
}
```

The obvious encoding, `Vec<Vec<Leg>>` hanging off each transaction, would put
every entry's sides in their own heap allocation - so totalling a month of
spending would chase one pointer per transaction. Flattened, an entry's legs
are contiguous, and the transaction rows stay fixed-size so scanning by date is
still a walk down one `i32` column.

A budget question is nearly always "one field, across every row": total a
bucket, find transactions in a date range, list the credit-normal ledgers.
Summing a bucket touches one `i64` array; the names and descriptions never enter
cache. Sorting produces a `Vec<LedgerIx>` of row numbers, so the UI clones the
20 rows it displays rather than the 50,000 it filtered.

Two payoffs worth calling out:

- **`postings` turns ledger queries from O(all transactions) into O(that
  ledger's transactions).** `TxQuery` notices when a filter pins a ledger and
  walks that list instead of scanning. Because a ledger may appear only once
  per entry, that walk yields each transaction exactly once - no deduplication
  pass.
- **Net worth is literally the sum of the raw balance column.** Balances are
  stored debit-positive and the sign is flipped at the display edge, so "add
  assets, subtract liabilities" needs no branch at all: it is
  `members.map(|i| raw_balance[i]).sum()`.

### Two kinds of identity

- `LedgerUid` and friends are **stable**: minted once, written into the op that
  creates the entity, never changed. Ops reference entities by uid, so replaying
  history in a different order - which is exactly what rebase does - still binds
  every transaction to the right ledger.
- `LedgerIx` and friends are **positional**: the row number in one
  materialisation of the budget. This is what the columns and the query engine
  use.

They are distinct types. Confusing them is the classic way to corrupt a
versioned store, and here it does not compile.

## Storage

Four tables, because the core only needs four kinds of blob:

```sql
meta(key, value)          -- schema version, HEAD
commits(id, body)         -- id = SHA-256 of body's canonical payload
refs(name, target)        -- branches
stage(seq, op)            -- work in progress; deliberately not history
```

There is no `ledgers` table and no `balances` table. Derived state in the
database is derived state that can go stale, and a budget whose stored balance
disagrees with its transactions is worse than no budget.

Durability choices, and why:

- `synchronous = FULL` - the write is on the platter before the call returns.
  This is what makes "I pressed commit" mean something.
- `journal_mode = DELETE` (the classic rollback journal), **not** WAL. WAL is
  faster under concurrent readers, which a single-user desktop budget does not
  have, and it leaves `-wal` and `-shm` files beside the database. A budget is a
  *document*: people email it to themselves, drop it in OneDrive, restore it
  from a backup. A document that is secretly three files gets corrupted by
  exactly that handling. It is a one-line change in `ledgit-sqlite` if that ever
  stops being true.
- Every commit is re-hashed when it is read. A bit-flip in the file becomes an
  error at the read, not a wrong balance three screens later. `ledgit verify` does
  the whole file.

## Decisions that are settled

These come up every time someone reads the code, so they are written down
rather than rediscovered:

- **Dates are UTC.** `Date::today_utc()` takes no time-zone dependency. The
  worst case is an entry made just before midnight being dated the next day,
  and every entry form lets you type the date. Do not add a time zone crate
  for this.
- **A reversal is dated to match the entry it reverses**, not to the day you
  noticed. A business dates the correction on the day of discovery; a personal
  budget wants last month's total to still read correctly after you fix last
  month's mistake.
- **The staging area is persisted, but it is not history.** It lives in its own
  table, never in the commit DAG. "Nothing is written until I commit" means
  nothing becomes permanent, shared, reportable state - not that an afternoon
  of data entry dies with the process.

## Invariants

1. **Debits equal credits.** `Budget::is_balanced()` sums the raw balance column
   and must get zero. `commit` refuses if it does not, and calls it a bug in the
   code, because it is.
2. **`apply` is all-or-nothing.** An operation validates fully before it writes,
   so a rejected op leaves the budget untouched.
3. **Replay is deterministic.** Same ops, same order, same budget. The tests
   rely on this and so does rebase.
4. **Nothing is deleted.** There is no op that removes a ledger, transaction
   or issuer. Buckets, cohorts and saved views are pure readings, so they may
   be deleted.
5. **Every entry's legs sum to zero**, number at least two, contain no zero
   amount, and name no ledger twice. The last one matters: two legs against
   one ledger is always either a typo or a sum the user should have done
   themselves, and netting them silently would hide the typo.
6. **`AdvanceIssuer` never rewinds.** Replaying an older advance is a no-op, so
   history can be replayed in any valid order without re-posting rent.
7. **Reading never writes.** Evaluating a view or a cohort, however far
   ahead it simulates, stages nothing and stores nothing.

## Performance, and when to worry

Folding the whole DAG on open is O(total ops). For a personal budget - call it
5,000 transactions a year, a decade of history - that is under a hundred
thousand ops, which folds in single-digit milliseconds. There is deliberately no
snapshot cache yet, because a cache is a second copy of the truth.

When it does start to hurt, the fix is already shaped: add a `snapshots` table
keyed by `CommitId` holding a serialised `Budget`, fold forward from the nearest
one, and treat it as a pure cache that can be deleted at any time without
changing a single balance. Measure before building it.
