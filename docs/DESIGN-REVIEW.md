# Review of the proposed architecture

You asked what I think of the `Query`/`Writer` design. Short version: the
instinct behind it is right and the mechanism is wrong. Here is the long
version, then what I built instead.

## What you got right

> The Query and Writer object allow the front end to build features using
> closures and comparators without needing to implement every front end
> functionality in the core library.

This is the correct goal, and most people get it wrong in the other direction -
they grow one core method per screen until the library is a pile of
`get_ledgers_sorted_by_balance_in_bucket_since()`. You are trying to give the
front end a *vocabulary* instead of a menu. Keep that goal.

## Why the closure version cannot work

### 1. Your own version-control requirement kills it

Section 2 says everything is a commit, and section 3 wants revert, branch, and
rebase. Ask what a commit has to contain. It has to contain the change itself,
because:

- **commit** has to write the change to disk,
- **revert** has to compute the change's inverse,
- **rebase** has to *replay* the change against a different starting state,
- the pre-commit report has to *describe* the change before it happens.

A `Fn(Option<ContextTarget>) -> Result<ReturnValue>` can do none of those. You
cannot serialise it, hash it, invert it, replay it, or print it. The moment you
wrote "everything is done in commits exactly like Git", the `Writer` stopped
being allowed to be a closure.

A change has to be **data**. That is the single most important decision in this
codebase, and your spec forced it without either of us noticing.

### 2. `ReturnValue` throws away the type system

```rust
enum ReturnValue { Int(i32), Str(str), Slice(Rc<[T]>), ... }
```

Every caller does `match result { ReturnValue::Int(n) => n, _ => unreachable!() }`.
You have rebuilt `dyn Any` with extra ceremony, and you now find out at runtime
what the compiler could have told you at build time. In a budget app the failure
mode is a wrong number on a screen you trust.

Also: `Slice(Rc<[T]>)` does not compile. `T` is not bound by anything - a single
enum variant cannot be generic over an unknown element type without making the
whole enum generic, at which point `ReturnValue<T>` infects every signature and
you have gained nothing.

### 3. The traits do not carry their weight

```rust
trait QueryTarget;
impl ContextTarget for LedgerValue, TransactionTarget ...
```

A trait with no methods gives you no shared behaviour. The API still has to
`match` on the enum to do anything, so the trait is decoration on top of the
match you were trying to avoid. (Two different names, `QueryTarget` and
`ContextTarget`, are used for what looks like one idea - a sign the role is not
yet clear in your head. That is fine at spec time; it is not fine at API time.)

### 4. Closures cannot be pushed down

A predicate that is data can be turned into a SQL `WHERE`, an index probe, or a
saved dashboard tile. A predicate that is a closure forces every query to be a
full scan in Rust, forever, and can never be persisted as "the view I look at
every morning". Today my queries *are* full scans too - but because the filters
are data, that is a choice I can revisit, not a wall.

### 5. `Balance(i32)` overflows, and floats would be worse

`i32` cents tops out at about $21.4 million. That is probably fine for you and
definitely not fine as a library invariant. It is `i64` in the code. And never,
ever `f64`: `0.10` is not representable, so a few thousand transactions later
your debits stop equalling your credits and you cannot tell whether it is your
bug or your bank's.

## What I built instead

| You proposed | What is in the code | Why |
|---|---|---|
| `Writer` holding a closure | `enum Op` - one variant per change | Serialisable, hashable, invertible, replayable, printable |
| `Query` holding a closure | Typed builders (`TxQuery`, `LedgerQuery`, ...) of plain filter enums | Composable like yours, but checked at compile time and pushable to storage later |
| `ReturnValue` type erasure | Each query returns a concrete `Vec<TxIx>` / `Vec<LedgerIx>` | The compiler keeps its job |
| Closures inside the core | Closures in the **front end**, over returned slices | Arbitrary logic still lives in the UI - just on the caller's side of the boundary |

The vocabulary you wanted is still there:

```rust
let rows = TxQuery::new()
    .filter(TxFilter::Touches(chequing))
    .filter(TxFilter::OnOrAfter(jan))
    .filter(TxFilter::FromIssuer(None))   // manual entries only
    .sort_by(TxSort::Amount, Order::Desc)
    .limit(20)
    .run(budget);

// Want something the core never imagined? Do it here.
let weird: Vec<_> = rows.iter().filter(|ix| my_closure(budget, **ix)).collect();
```

The front end composes without the core growing a method - which was your actual
requirement - and nothing in that snippet can panic on a mismatched enum.

## Three more things I changed, and why

**Issuers propose, they never post.** Your spec has issuers creating
transactions at intervals. Mine generate ops into the staging area, so recurring
payments show up in the pre-commit report next to your manual entries and you
approve them. Software that moves money while you are not looking is software
you stop trusting. (You already half-asked for this: "give me the option to look
at all the transactions created by issuers since last commit.")

**The staging area is written to disk, but it is not history.** You said nothing
is written to disk until you press commit. I persist the stage to a separate
table anyway. Taken literally, your rule means an hour of data entry dies with
the process. What you actually want is "nothing becomes *permanent, shared,
reportable state* until I commit", and that is what the split between `stage`
and `commits` gives you. Say the word and I will make it memory-only.

**Reverting posts a mirror entry.** Exactly as you asked - the reversal is a new
transaction with debit and credit swapped, and both facts stay in the register.
Some things have no inverse: a ledger, once opened, stays open, so
`revert` returns a note saying so instead of pretending. Reverting a commit that
created an issuer pauses it.

## The one thing I changed about the domain model

Your spec gave a transaction "a creditted ledger and a debitted ledger" -
exactly two sides. Real entries are often n-legged: a paycheque is gross salary
credited, and chequing + tax + retirement debited, all in one atomic entry. With
two legs you have to fake that as three separate transactions that drift apart
the first time one is edited.

You asked for this before putting real data in, which was the right call - it
rewrites the on-disk op format, so it was nearly free then and would have been
painful later.

An entry is now a list of legs:

```rust
pub struct Leg {
    pub ledger: LedgerUid,
    pub amount: Money,   // debit-positive: + debits, - credits
}
```

with one invariant: **the legs sum to zero**. Two legs is the ordinary transfer
you already had; `Repo::post(name, desc, date, amount, debit, credit)` still
builds it for you, so nothing about the simple case got harder.

What fell out of it, which is the part worth noticing:

- **Applying an entry lost its branches.** It is
  `for leg in legs { raw_balance[leg.ledger] += leg.amount }`. No direction
  test, no normality test.
- **Revert got simpler, not harder.** Reversing is negating every leg. That is
  correct for a four-sided paycheque for exactly the reason it is correct for a
  two-sided transfer - sum-zero in, sum-zero out - so there is no special case
  anywhere.
- **The legs went into their own flat arena** rather than a `Vec<Leg>` per
  transaction, because a per-entry `Vec` is a per-entry heap allocation and the
  point of the columnar layout was to avoid exactly that. See ARCHITECTURE.md.
- **One genuine new rule:** a ledger may not appear twice in one entry. Two
  legs against one ledger is always either a typo or a subtraction you should
  have done yourself, and silently netting them would hide the typo.

Two things I decided while doing it, which you should overrule if you disagree:

**A negative amount is not an error at the op level.** `Leg` amounts are signed,
so "debit $-5" is just a credit of $5 and the entry still balances. Validation
at the core is purely structural. The guard lives in the *convenience*
constructors - `Repo::post`, the CLI, the GUI form - because only there does
"amount" mean a magnitude rather than a direction.

**The size of an entry is what it debits.** A $2,400 paycheque split three ways
reports as $2,400, not $4,800. Every total in the app - the register, the
pre-commit report, the amount filters - uses that definition.

The CLI grew a remainder convention for typing these, which is the bit you will
actually feel:

```sh
ledgit post "Paycheque" \
    --debit Chequing:1800 --debit "Tax withheld":500 --debit Pension:100 \
    --credit "Gross pay"
```

One side may leave its amount off and it takes whatever balances the entry. The
GUI form does the same thing with a "balance the last side" button and a live
"out by $X" readout.
