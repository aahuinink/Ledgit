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

## Crates

```
ledgit-core     domain types, ops, commit DAG, queries, the Repo API.  No I/O.
ledgit-sqlite   implements ledgit_core::store::Store against a SQLite file.
ledgit-cli      `ledgit` - a front end, and the fastest way to exercise the core.
```

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
   or issuer. Buckets are pure views, so they may be deleted.
5. **Every entry's legs sum to zero**, number at least two, contain no zero
   amount, and name no ledger twice. The last one matters: two legs against
   one ledger is always either a typo or a sum the user should have done
   themselves, and netting them silently would hide the typo.
6. **`AdvanceIssuer` never rewinds.** Replaying an older advance is a no-op, so
   history can be replayed in any valid order without re-posting rent.

## Performance, and when to worry

Folding the whole DAG on open is O(total ops). For a personal budget - call it
5,000 transactions a year, a decade of history - that is under a hundred
thousand ops, which folds in single-digit milliseconds. There is deliberately no
snapshot cache yet, because a cache is a second copy of the truth.

When it does start to hurt, the fix is already shaped: add a `snapshots` table
keyed by `CommitId` holding a serialised `Budget`, fold forward from the nearest
one, and treat it as a pure cache that can be deleted at any time without
changing a single balance. Measure before building it.
