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
| Swap the database | persist five kinds of blob, not a schema |
| Switch branch with work staged | shelve the stage on the branch it belongs to, beside history |
| Ledgers/transactions/issuers are never deleted | there is no delete op for them |
| Split entries (a paycheque) | one op with N legs summing to zero |
| Saved views | ops, like buckets; read by pure functions, results never stored |
| A ledger tree (`Wedding:Tuxedo`) | paths in names; the tree is derived, never stored |
| Undo in the app | put back an earlier `Vec<Op>` as the stage |
| Edit a staged entry | replace one op in the stage, under the same uid |
| Variables, targets, alerts | ops, like a ledger's name; versioned, never money |
| Interest and percentage issuers | a rule on the issuer, priced in date order at run time |

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

Roll-ups across several buckets are a saved view instead. A view's scope is a
slice of signed `Term`s, and `query::members` treats membership as a set:

* in added buckets only - counted once, positive;
* in subtracted buckets only - counted once, negative;
* in both - cancelled, and reported separately rather than dropped.

So `Cash - Receivables` is a view over buckets, not a bucket of buckets. The
view holds bucket uids, not members, so nothing needs validating on replay, and
a view naming a deleted bucket degrades to a warning instead of an unloadable
budget.

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

## Dues: what issuers cost, and when they land

There is no group-of-issuers entity: a saved view names the issuers it breaks
down, and that is the grouping. Two reads (`ledgit_core::dues`) take any
slice of issuers - a view's, or every issuer for the Calendar screen:

* **Rates.** `period::per_period(amount, schedule, period)` converts a
  schedule to any unit in exact rational arithmetic, rounded once at the end.
  Month and year are Gregorian averages (146,097 days per 400 years), so
  $10/day is $70/week and $304.37/month, a monthly $100 is exactly $100 a
  month, and nothing drifts on a round trip. `dues::total` counts only
  running, recurring issuers; `dues::paused_total` counts the paused ones.
* **Calendar.** `Schedule::dates_between(start, from, to)` lists every
  occurrence in a window, jumping straight to `from` rather than stepping
  there. `dues::calendar` marks each one against `emitted_through` and
  today: posted, overdue, upcoming, or paused.

A **rate** and a **calendar period** are kept apart on purpose. "Biweekly
$400 is $869.63 a month" is a rate; "March had three paydays" is a calendar
fact. `Period::start_of`/`next_start` carve time into calendar periods, and
only the timeline tables use them.

## Saved views: a reading across time

A `ViewSpec` is plain data inside `CreateView`/`EditView` ops: signed bucket
terms, ledgers, issuers, `TxFilter`s, a period, and a **lookback and
horizon as `Span`s** ("6 months back, 1 year ahead"), never as dates - so a
saved view keeps meaning the same thing whenever it is opened. It is
versioned, so it travels with the file and can differ per branch, but it
holds no results: `view::evaluate(budget, spec, today)` recomputes the
`ViewReport` every time, in well under a millisecond.

**Scope and direction.** "Spent" only means something relative to a pot of
money, so a view has a scope - its bucket combination (the set rules of
`query::members`, above), or else its ledgers as displayed. Scope is one integer weight per ledger, and every effect is

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
it to the view's own issuers (the what-if).

What comes out: step **series** (scope total, each bucket, each ledger) with
today, the end, and the lowest point ahead; **flow lines** per selected
issuer, signed against the scope; and a **timeline** of calendar periods
with actual and projected in/out side by side.

**Validation is deliberately partial.** Saving a view checks that its
ledgers and issuers exist, since those can never disappear. It does not check
buckets: they are deletable, evaluation already reports a missing one, and
checking would make history order-sensitive - reverting "delete bucket,
delete view" recreates the view before the bucket.

**Charts.** Balances are drawn as steps (a balance holds until the next
posting; a slope would invent money between paydays), solid up to today and
dashed after, over a faint shading of the future. Series colours are one
fixed, colour-blind-checked order, shared by the GUI and the exports.

## The stage can hold broken changes

Every op applies when it is staged. It may stop applying later: drop the
staged ledger that a staged transaction posts to, and the transaction has
nowhere to go. Refusing the drop (the old behaviour) makes you unpick
everything in reverse; dropping the transaction silently loses work. So the
op stays in the stage, **broken**: `rebuild_working` folds the stage,
skips what fails, and records each failure (`Repo::broken()`). `working`,
the report and every screen show only what applies; `commit` refuses while
anything is broken. Editing the op (`replace_staged`, same uid) or dropping
it clears the flag.

The same fold handles a stage that stops applying after a checkout, which
used to drop those ops with an error.

### Shelves

The stage belongs to the branch it was entered on, so a plain `checkout`
still refuses while anything is staged. `checkout_with` says what to do
instead (`StagedWork`):

* **Shelve** - the stage is set aside *on the branch being left*
  (`Store::set_shelf(branch, ops)`), and comes back when you switch to that
  branch again. The shelf is written before the stage is cleared, so a crash
  between the two leaves the work in both places, never in neither.
* **Bring** - the stage is kept and refolded on the new base. Ops that do not
  apply there are flagged broken like any other, not dropped.

Arriving on a branch always puts its shelf back, after anything brought
along. `checkout_new` brings the stage: a branch made here starts from the
same base, so the work applies unchanged, and "this is a what-if" is the
usual reason to branch. A branch holding shelved work cannot be deleted.
Shelves are keyed by branch name; a detached HEAD has nowhere to shelve.

The GUI's undo is built on this. Every change to the stage - an entry, a
drop, an edit, an issuer run, "discard everything" - is a new `Vec<Op>`, so
undo puts the previous one back with `Repo::set_stage`, which never refuses:
a stage that no longer fully applies comes back flagged, not lost. Undo
stops at a commit, because an older stage on a new base would re-post
history.

## Variables and formulas

`SetVariable { name, value }` stores a number (as the decimal text it was
typed as) or some text. An amount field may hold a formula over the numbers -
`200 * Car_Km_Rate`, `5%`, `(1850 - 400) / 2` - worked out by `expr` in exact
fractions of two `i128`s and rounded once, to the cent, half away from zero.
A name or description may use text as `{Home}`.

Both are worked out **when the entry is made**, and the entry stores the
resulting money and text. Changing a rate later re-prices nothing already
posted. That is the ledger's promise, and it means an entry never depends on
a variable existing.

## Issuers whose amount follows a balance

An issuer may carry an `AmountRule`: a share of a ledger's balance each time
(`5% of savings`), or interest at an APR for the days since the previous
occurrence, actual/365 (`6.45% APR on the car loan`). Its legs then only say
which ledgers move and which way, as proportions; the rule sets the total.

Such an amount depends on everything that happened before it - last month's
interest, and the payment another issuer made in between - so occurrences
cannot be priced one issuer at a time. `issuer::project` lists every
occurrence of every issuer in date order and walks them once, reading each
rule's ledger at the start of its day with every earlier projected occurrence
included. Posting (`run_all`) and the saved-view simulation both go through
it, so a view predicts exactly what a run will post. A balance at or below
zero produces no entry that time; the issuer still advances.

`CreateIssuer.rule` is left out of the encoding when absent (not written as
`null`), so every issuer committed before rules existed encodes - and hashes
- exactly as it did. `CreateIssuer.settles` and `ViewSpec.show_available` are
left out the same way.

### Statements

`AmountRule::Statement { of, close_day, min, min_rate }` reads history, not
one balance: for an occurrence due on D, the statement closed on the last
`close_day` before D (`AmountRule::closes_before`); it pays `of`'s balance at
the end of that day, less everything that brought the balance down after the
close and before D. Unpaid remainder is simply still in the balance at the
next close, so there is no carry-over state to keep. `project` keeps a dated
list of everything it has already moved so a projected statement counts the
charges and payments other issuers make first.

### Amounts set ahead

`Op::SetIssuerOverride { uid, date, amount }` fixes one occurrence's amount
(or with `None` clears it). It must name a real due date not yet posted, and
is stored per issuer as a sorted `Vec<(Date, Money)>`. Pricing goes through
one function, `issuer::price`: the override replaces what the issuer would
work out, raised to a statement's minimum. Posting, projections, the
calendar and views all see the same number. Reverting puts back the date's
previous amount.

### Scheduled from an entry

"Post now, pay later" stages the entry and a one-off issuer
(`issuer::settlement`) with `settles: Some(tx)`. "Post on its date" stages
only a one-off issuer with the entry's legs. Both post when issuers are run on
or after their date, like any other issuer.

## Posted and available

`available::Availability::of(budget, today)` lists the **commitments** -
payments already decided but not posted - and holds back their credit legs:

* every one-off issuer not yet run (scheduled payments and entries);
* a statement issuer's occurrence once its statement has closed
  (`closed <= today`).

Recurring issuers are not commitments. Only the credit side is held: money
leaving chequing counts now, the card going down counts when the payment
posts. A ledger's available balance is its raw balance plus what is held,
presented for its normality; a bucket's uses the same roll-up as its total.
Commitments are priced by `project` over every running issuer, so they agree
with what will post.

A view with `show_available` draws each series as available from today: the
series' value at t plus every held amount whose occurrence has not landed in
the simulation by t (or never will, if the view does not simulate its
issuer). It meets the posted line as the last commitment posts.
`available::changes(base, working, today)` gives the pre-commit report its
"money spoken for": commitments made, paid, or resized by what is staged.

## Targets and alerts

`SetLedgerGoals { uid, target, alerts }` replaces a ledger's target and its
alerts (below/above a level, with a message). Both are about the balance as
displayed, so a loan's target of 0 means "paid off".

A `Target` is a `Balance` or a `Pace { amount, per, bound }`. A balance
target has no direction: it is reached going whichever way the balance has
to travel from where it stands. A pace is a target with a time dimension -
"250 a week at most" on groceries, "500 a month at least" into savings - and
so needs a `Bound`, because nothing about either ledger says which way is
good. It is judged per *calendar* period (`Period::start_of`), never a
rolling window. `Target` is `#[serde(untagged)]` and a balance encodes as the
bare amount, exactly as `target` did before paces existed, so every earlier
commit decodes and hashes unchanged.

They are read, never stored as results: `goals::fired` for the alert list,
`goals::newly_fired(base, after)` for "this commit sets off...", the view
series (`target`, `target_reached`, `alerts_ahead`) for the simulation, and
`goals::bucket_targets` for a bucket's combined target - over only the
members that have one, on both sides, so progress is never mixed with money
nobody set a goal for. A view's total or bucket line gets a target only when
every ledger in it has one. Paces never enter those balance sums.

Paces have their own readings: `goals::pace_status` / `goals::paces` for the
period today falls in (with the pace's pro-rata share of the days gone, so a
budget can be "ahead of pace" before it is over), `goals::pace_history` for
the periods before it, `goals::newly_broken(base, after, touched)` for "this
commit breaks..." - judged only in the periods the staged entries land in -
and `goals::bucket_paces`, which converts members' paces to one unit by
average lengths (`period::convert`) and compares them with the calendar
period.

Comparing a view with an earlier commit (`view::compare`) evaluates the same
spec against `Repo::budget_at(commit)` over the same window and the same
today, and pairs the lines by uid rather than by row.

## "Fresh through"

The top bar reads `Fresh through 2026-09-24 · issuers 2026-09-25`:

* the first date is `Budget::latest_transaction_date()` - the newest entry on
  this branch, staged ones included;
* the second is `dues::caught_up_through()` - the day before the earliest
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

Five tables, because the core only needs five kinds of blob:

```sql
meta(key, value)          -- schema version, HEAD
commits(id, body)         -- id = SHA-256 of body's canonical payload
refs(name, target)        -- branches
stage(seq, op)            -- work in progress; deliberately not history
shelves(branch, seq, op)  -- work set aside on a branch you switched away from
```

`shelves` arrived without a schema bump: it is created if missing on open,
and an older build simply never looks at it.

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
  of data entry dies with the process. Shelves are the same, per branch.
- **One Ledgit at a time.** Two copies on one file would each hold their own
  stage and undo history, and the last to write would win. The first copy
  listens on a loopback port picked from the user's name; a second launch
  hands its budget path to the first (which opens it and comes to the front)
  and exits. A socket rather than a lock file because it carries the path
  across and frees itself however the first copy ends. Something else on the
  port means Ledgit runs unguarded rather than not at all. `instance.rs`.
- **Tables fit their content, up to a cap, then cut it short.** Every data
  table goes through `table.rs` (egui_extras' `TableBuilder`): columns size
  to what they hold, clip past a per-column cap with the whole text on hover,
  and can be dragged wider; figures are right-aligned inside their own column;
  long tables scroll with the header kept. `egui::Grid` is for forms only.

## Invariants

1. **Debits equal credits.** `Budget::is_balanced()` sums the raw balance column
   and must get zero. `commit` refuses if it does not, and calls it a bug in the
   code, because it is.
2. **`apply` is all-or-nothing.** An operation validates fully before it writes,
   so a rejected op leaves the budget untouched.
3. **Replay is deterministic.** Same ops, same order, same budget. The tests
   rely on this and so does rebase.
4. **Nothing is deleted.** There is no op that removes a ledger, transaction
   or issuer. Buckets and saved views are pure readings, so they may be
   deleted.
5. **Every entry's legs sum to zero**, number at least two, contain no zero
   amount, and name no ledger twice. The last one matters: two legs against
   one ledger is always either a typo or a sum the user should have done
   themselves, and netting them silently would hide the typo.
6. **`AdvanceIssuer` never rewinds.** Replaying an older advance is a no-op, so
   history can be replayed in any valid order without re-posting rent.
7. **Reading never writes.** Evaluating a view, however far
   ahead it simulates, stages nothing and stores nothing.
8. **Nothing broken is committed.** A staged op that does not apply is kept
   and flagged, and `commit` refuses until there are none.
9. **Old commits keep their hashes.** A field added to an op is left out of
   the encoding when it holds its default, so ops written before it existed
   encode byte for byte as they did.

## Performance, and when to worry

Folding the whole DAG on open is O(total ops). For a personal budget - call it
5,000 transactions a year, a decade of history - that is under a hundred
thousand ops, which folds in single-digit milliseconds. There is deliberately no
snapshot cache yet, because a cache is a second copy of the truth.

When it does start to hurt, the fix is already shaped: add a `snapshots` table
keyed by `CommitId` holding a serialised `Budget`, fold forward from the nearest
one, and treat it as a pure cache that can be deleted at any time without
changing a single balance. Measure before building it.
