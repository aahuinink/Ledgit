//! Reading the budget.
//!
//! Queries are typed builders that lower to a scan over the arena columns.
//! They return row indices, not cloned rows: a front end that wants to show a
//! table of 20 transactions out of 50,000 should pay for 20 clones, not 50,000.
//!
//! Every filter here is *data*, which matters for the same reason ops are data:
//! a saved view, a dashboard tile, or a query sent to a future background
//! indexer is just a serialised `TxQuery`. A closure would be none of those.
//! Front ends still get arbitrary logic - they run it over the returned slice,
//! where it belongs.

use crate::date::Date;
use crate::id::{BucketUid, IssuerIx, IssuerUid, LedgerIx, LedgerUid, PostingIx, TxIx};
use crate::model::{Normality, Parent};
use crate::money::Money;
use crate::state::{Budget, LedgerArena};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Order {
    #[default]
    Asc,
    Desc,
}

impl Order {
    fn apply(self, o: std::cmp::Ordering) -> std::cmp::Ordering {
        match self {
            Order::Asc => o,
            Order::Desc => o.reverse(),
        }
    }
}

// ---------------------------------------------------------------- ledgers

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum LedgerFilter {
    Uid(LedgerUid),
    /// Case-insensitive substring of the name or description.
    Text(String),
    Normality(Normality),
    /// Compares the *presented* balance, the one shown in the UI.
    BalanceAtLeast(Money),
    BalanceAtMost(Money),
    OpenedOnOrAfter(Date),
    OpenedOnOrBefore(Date),
    InBucket(BucketUid),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum LedgerSort {
    #[default]
    Name,
    Balance,
    Normality,
    Opened,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct LedgerQuery {
    pub filters: Vec<LedgerFilter>,
    pub sort: LedgerSort,
    pub order: Order,
    pub limit: Option<usize>,
}

impl LedgerQuery {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn filter(mut self, f: LedgerFilter) -> Self {
        self.filters.push(f);
        self
    }
    pub fn sort_by(mut self, s: LedgerSort, o: Order) -> Self {
        (self.sort, self.order) = (s, o);
        self
    }
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }

    pub fn run(&self, l: &Budget) -> Vec<LedgerIx> {
        let a = &l.ledgers;
        let mut out: Vec<LedgerIx> = a
            .indices()
            .filter(|ix| self.filters.iter().all(|f| ledger_matches(l, *ix, f)))
            .collect();
        out.sort_by(|x, y| {
            let o = match self.sort {
                LedgerSort::Name => {
                    a.name[x.get()].to_lowercase().cmp(&a.name[y.get()].to_lowercase())
                }
                LedgerSort::Balance => a.balance(*x).cmp(&a.balance(*y)),
                LedgerSort::Normality => a.normality[x.get()].cmp(&a.normality[y.get()]),
                LedgerSort::Opened => a.opened[x.get()].cmp(&a.opened[y.get()]),
            };
            self.order.apply(o).then(x.0.cmp(&y.0))
        });
        if let Some(n) = self.limit {
            out.truncate(n);
        }
        out
    }
}

fn ledger_matches(l: &Budget, ix: LedgerIx, f: &LedgerFilter) -> bool {
    let a = &l.ledgers;
    let i = ix.get();
    match f {
        LedgerFilter::Uid(u) => a.uid[i] == *u,
        LedgerFilter::Text(t) => contains_ci(&a.name[i], t) || contains_ci(&a.description[i], t),
        LedgerFilter::Normality(n) => a.normality[i] == *n,
        LedgerFilter::BalanceAtLeast(m) => a.balance(ix) >= *m,
        LedgerFilter::BalanceAtMost(m) => a.balance(ix) <= *m,
        LedgerFilter::OpenedOnOrAfter(d) => a.opened[i] >= *d,
        LedgerFilter::OpenedOnOrBefore(d) => a.opened[i] <= *d,
        LedgerFilter::InBucket(b) => {
            l.buckets.ix(*b).is_some_and(|bix| l.buckets.members[bix.get()].contains(&ix))
        }
    }
}

// ------------------------------------------------------------ transactions

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum TxFilter {
    Text(String),
    /// Any leg of the entry is against this ledger.
    Touches(LedgerUid),
    /// This ledger is debited by the entry.
    DebitedTo(LedgerUid),
    /// This ledger is credited by the entry.
    CreditedFrom(LedgerUid),
    /// Any leg of the entry is against a ledger in this bucket.
    TouchesBucket(BucketUid),
    /// Entries with more than two sides. "Show me my paycheques."
    IsSplit(bool),
    OnOrAfter(Date),
    OnOrBefore(Date),
    AmountAtLeast(Money),
    AmountAtMost(Money),
    /// `None` matches manual entries; `Some(uid)` matches one issuer's output.
    FromIssuer(Option<IssuerUid>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum TxSort {
    #[default]
    Date,
    Amount,
    Name,
    /// The order the transactions were posted in, i.e. commit order.
    Posted,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TxQuery {
    pub filters: Vec<TxFilter>,
    pub sort: TxSort,
    pub order: Order,
    pub limit: Option<usize>,
}

impl Default for TxQuery {
    fn default() -> Self {
        // Newest first: what anyone opening a register wants to see.
        TxQuery { filters: vec![], sort: TxSort::Date, order: Order::Desc, limit: None }
    }
}

impl TxQuery {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn filter(mut self, f: TxFilter) -> Self {
        self.filters.push(f);
        self
    }
    pub fn sort_by(mut self, s: TxSort, o: Order) -> Self {
        (self.sort, self.order) = (s, o);
        self
    }
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }

    pub fn run(&self, l: &Budget) -> Vec<TxIx> {
        let t = &l.transactions;
        // If the query pins one ledger, walk that ledger's posting list
        // instead of every transaction in the budget. `validate_legs` forbids
        // a ledger appearing twice in one entry, so this yields each
        // transaction at most once with no deduplication pass.
        let candidates: Vec<TxIx> = match self.pinned_ledger(l) {
            Some(aix) => {
                l.ledgers.postings[aix.get()].iter().map(|p| l.postings.tx[p.get()]).collect()
            }
            None => t.indices().collect(),
        };
        let mut out: Vec<TxIx> = candidates
            .into_iter()
            .filter(|ix| self.filters.iter().all(|f| tx_matches(l, *ix, f)))
            .collect();
        out.sort_by(|x, y| {
            let o = match self.sort {
                TxSort::Date => t.date[x.get()].cmp(&t.date[y.get()]),
                TxSort::Amount => l.amount_of(*x).cmp(&l.amount_of(*y)),
                TxSort::Name => t.name[x.get()].to_lowercase().cmp(&t.name[y.get()].to_lowercase()),
                TxSort::Posted => x.0.cmp(&y.0),
            };
            self.order.apply(o).then(x.0.cmp(&y.0))
        });
        if let Some(n) = self.limit {
            out.truncate(n);
        }
        out
    }

    fn pinned_ledger(&self, l: &Budget) -> Option<LedgerIx> {
        self.filters.iter().find_map(|f| match f {
            TxFilter::Touches(u) | TxFilter::DebitedTo(u) | TxFilter::CreditedFrom(u) => {
                l.ledgers.ix(*u)
            }
            _ => None,
        })
    }
}

fn tx_matches(l: &Budget, ix: TxIx, f: &TxFilter) -> bool {
    let t = &l.transactions;
    let i = ix.get();
    // Legs of an entry are contiguous, so every ledger test below is a short
    // linear scan of one slice - no allocation, no pointer chasing.
    let legs = t.leg_range(ix);
    let has = |uid: LedgerUid, want_debit: Option<bool>| {
        l.ledgers.ix(uid).is_some_and(|a| {
            legs.clone().any(|p| {
                l.postings.ledger[p] == a
                    && want_debit.is_none_or(|d| (l.postings.amount[p].0 > 0) == d)
            })
        })
    };
    match f {
        TxFilter::Text(s) => contains_ci(&t.name[i], s) || contains_ci(&t.description[i], s),
        TxFilter::Touches(u) => has(*u, None),
        TxFilter::DebitedTo(u) => has(*u, Some(true)),
        TxFilter::CreditedFrom(u) => has(*u, Some(false)),
        TxFilter::TouchesBucket(b) => l.buckets.ix(*b).is_some_and(|bix| {
            let m = &l.buckets.members[bix.get()];
            legs.clone().any(|p| m.contains(&l.postings.ledger[p]))
        }),
        TxFilter::IsSplit(want) => t.is_split(ix) == *want,
        TxFilter::OnOrAfter(d) => t.date[i] >= *d,
        TxFilter::OnOrBefore(d) => t.date[i] <= *d,
        TxFilter::AmountAtLeast(m) => l.amount_of(ix) >= *m,
        TxFilter::AmountAtMost(m) => l.amount_of(ix) <= *m,
        TxFilter::FromIssuer(None) => t.parent[i] == Parent::Manual,
        TxFilter::FromIssuer(Some(u)) => t.parent[i] == Parent::Issuer(*u),
    }
}

// ----------------------------------------------------------------- issuers

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum IssuerFilter {
    Text(String),
    Paused(bool),
    Touches(LedgerUid),
    /// Has at least one occurrence not yet emitted as of this date.
    DueOnOrBefore(Date),
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct IssuerQuery {
    pub filters: Vec<IssuerFilter>,
    pub order: Order,
}

impl IssuerQuery {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn filter(mut self, f: IssuerFilter) -> Self {
        self.filters.push(f);
        self
    }

    pub fn run(&self, l: &Budget) -> Vec<IssuerIx> {
        let s = &l.issuers;
        let mut out: Vec<IssuerIx> = s
            .indices()
            .filter(|ix| {
                let i = ix.get();
                self.filters.iter().all(|f| match f {
                    IssuerFilter::Text(t) => {
                        contains_ci(&s.name[i], t) || contains_ci(&s.description[i], t)
                    }
                    IssuerFilter::Paused(p) => s.paused[i] == *p,
                    IssuerFilter::Touches(u) => s.legs[i].iter().any(|leg| leg.ledger == *u),
                    IssuerFilter::DueOnOrBefore(d) => {
                        !s.paused[i] && crate::issuer::next_due(l, *ix).is_some_and(|due| due <= *d)
                    }
                })
            })
            .collect();
        out.sort_by(|x, y| {
            self.order.apply(s.name[x.get()].to_lowercase().cmp(&s.name[y.get()].to_lowercase()))
        });
        out
    }
}

// ----------------------------------------------------------------- buckets

/// How a bucket combines its members' balances.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum RollUp {
    /// Add debit-normal members, subtract credit-normal ones. This is the
    /// net-worth reading: assets minus liabilities.
    #[default]
    ByNormality,
    /// Add every member's presented balance as-is. Use for "total spending"
    /// buckets where every member points the same way.
    Sum,
}

/// One line of a bucket roll-up.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BucketLine {
    pub ledger: LedgerIx,
    pub name: String,
    pub normality: Normality,
    /// The balance as displayed for that ledger.
    pub balance: Money,
    /// Its signed contribution to the bucket total under the chosen roll-up.
    pub contribution: Money,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BucketRollUp {
    pub uid: BucketUid,
    pub name: String,
    pub total: Money,
    pub lines: Vec<BucketLine>,
}

/// Compute a bucket's total and its per-ledger breakdown.
///
/// This is the single hottest read in the app - a dashboard recomputes every
/// pinned bucket on every keystroke of a what-if - and it is a scan of one
/// `i64` column plus one `u8` column.
pub fn roll_up(
    l: &Budget,
    bucket: BucketUid,
    roll: RollUp,
    sort: LedgerSort,
    order: Order,
) -> Option<BucketRollUp> {
    let bix = l.buckets.ix(bucket)?;
    let a = &l.ledgers;

    let mut lines: Vec<BucketLine> =
        l.buckets.members[bix.get()].iter().map(|ix| bucket_line(l, *ix, roll)).collect();
    sort_lines(&mut lines, a, sort, order);

    let total = lines.iter().map(|l| l.contribution).sum();
    Some(BucketRollUp { uid: bucket, name: l.buckets.name[bix.get()].clone(), total, lines })
}

/// One ledger's row, before any sign from a combination is applied.
fn bucket_line(l: &Budget, ix: LedgerIx, roll: RollUp) -> BucketLine {
    let a = &l.ledgers;
    let balance = a.balance(ix);
    BucketLine {
        ledger: ix,
        name: a.name[ix.get()].clone(),
        normality: a.normality[ix.get()],
        balance,
        contribution: match roll {
            RollUp::Sum => balance,
            RollUp::ByNormality => Money(a.raw_balance[ix.get()].0),
        },
    }
}

/// Ties are broken by row index so a redraw never reshuffles equal rows.
fn sort_lines(lines: &mut [BucketLine], a: &LedgerArena, sort: LedgerSort, order: Order) {
    lines.sort_by(|x, y| {
        let o = match sort {
            LedgerSort::Name => x.name.to_lowercase().cmp(&y.name.to_lowercase()),
            LedgerSort::Balance => x.balance.cmp(&y.balance),
            LedgerSort::Normality => x.normality.cmp(&y.normality),
            LedgerSort::Opened => a.opened[x.ledger.get()].cmp(&a.opened[y.ledger.get()]),
        };
        order.apply(o).then(x.ledger.0.cmp(&y.ledger.0))
    });
}

// ------------------------------------------------------ combining buckets

/// Which way a bucket enters a [`Combination`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Sign {
    #[default]
    Plus,
    Minus,
}

impl Sign {
    /// `-1` or `+1`, for multiplying a contribution.
    fn apply(self, m: Money) -> Money {
        match self {
            Sign::Plus => m,
            Sign::Minus => -m,
        }
    }
}

/// One bucket's part in a combination: "plus Cash", "minus Receivables".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Term {
    pub bucket: BucketUid,
    pub sign: Sign,
}

impl Term {
    pub fn plus(bucket: BucketUid) -> Term {
        Term { bucket, sign: Sign::Plus }
    }
    pub fn minus(bucket: BucketUid) -> Term {
        Term { bucket, sign: Sign::Minus }
    }
}

/// Totals across several buckets at once, e.g. `Cash - Receivables`.
///
/// This is a *read*, not an entity: nothing here is stored in the budget and
/// no operation creates it. Buckets stay flat, which keeps the op log free of
/// a graph that would have to be checked for cycles on every replay.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Combination {
    pub total: Money,
    /// One row per contributing ledger, deduplicated: a ledger in two added
    /// buckets is counted once, not twice.
    pub lines: Vec<BucketLine>,
    /// Ledgers that appeared on both sides and so contribute nothing. Kept
    /// rather than dropped, because a silently vanishing ledger looks like a
    /// bug in the totals.
    pub cancelled: Vec<BucketLine>,
    /// Terms naming a bucket that is not on this branch. A combination can
    /// outlive the bucket it names - buckets are deletable - so this is
    /// reported rather than treated as an error.
    pub missing: Vec<BucketUid>,
}

/// Total several buckets together, adding some and subtracting others.
///
/// Membership is treated as a set, so the answer does not depend on how many
/// buckets a ledger happens to belong to:
///
/// * in added buckets only - counted once, positive;
/// * in subtracted buckets only - counted once, negative;
/// * in both - cancelled, and listed in [`Combination::cancelled`].
///
/// `roll` applies per ledger exactly as it does for a single bucket, so a
/// `ByNormality` combination still nets assets against liabilities within each
/// term before the term's own sign is applied.
pub fn combine(
    l: &Budget,
    terms: &[Term],
    roll: RollUp,
    sort: LedgerSort,
    order: Order,
) -> Combination {
    let mut missing = Vec::new();
    // Membership per side. `LedgerIx` is a u32 row index, so these stay small
    // even for a combination spanning every bucket in the budget.
    let mut plus: Vec<LedgerIx> = Vec::new();
    let mut minus: Vec<LedgerIx> = Vec::new();

    for term in terms {
        let Some(bix) = l.buckets.ix(term.bucket) else {
            if !missing.contains(&term.bucket) {
                missing.push(term.bucket);
            }
            continue;
        };
        let side = match term.sign {
            Sign::Plus => &mut plus,
            Sign::Minus => &mut minus,
        };
        for ix in &l.buckets.members[bix.get()] {
            if !side.contains(ix) {
                side.push(*ix);
            }
        }
    }

    let mut lines = Vec::new();
    let mut cancelled = Vec::new();

    for ix in &plus {
        if minus.contains(ix) {
            cancelled.push(zeroed(bucket_line(l, *ix, roll)));
        } else {
            lines.push(bucket_line(l, *ix, roll));
        }
    }
    for ix in &minus {
        if plus.contains(ix) {
            continue; // already recorded as cancelled above
        }
        let mut line = bucket_line(l, *ix, roll);
        line.contribution = Sign::Minus.apply(line.contribution);
        lines.push(line);
    }

    let a = &l.ledgers;
    sort_lines(&mut lines, a, sort, order);
    sort_lines(&mut cancelled, a, sort, order);

    let total = lines.iter().map(|l| l.contribution).sum();
    Combination { total, lines, cancelled, missing }
}

/// A cancelled row keeps its balance for display but contributes nothing.
fn zeroed(mut line: BucketLine) -> BucketLine {
    line.contribution = Money(0);
    line
}

/// Balance of one ledger restricted to transactions on or before `as_of`.
///
/// Walks the ledger's posting list rather than the whole budget.
pub fn balance_as_of(l: &Budget, ledger: LedgerIx, as_of: Date) -> Money {
    let raw: Money = l.ledgers.postings[ledger.get()]
        .iter()
        .filter(|p| l.transactions.date[l.postings.tx[p.get()].get()] <= as_of)
        .map(|p| l.postings.amount[p.get()])
        .sum();
    l.ledgers.normality[ledger.get()].present(raw)
}

/// One line of a ledger register: the posting, the transaction it came from,
/// and the ledger's balance immediately after it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RegisterLine {
    pub posting: PostingIx,
    pub transaction: TxIx,
    /// This ledger's share of the entry, debit-positive.
    pub change: Money,
    /// The ledger's balance after this posting, as it is displayed.
    pub balance: Money,
}

/// Every posting against a ledger, oldest first, with a running balance.
pub fn register(l: &Budget, ledger: LedgerIx) -> Vec<RegisterLine> {
    let mut rows: Vec<PostingIx> = l.ledgers.postings[ledger.get()].clone();
    // Date first, then entry order, so two entries on the same day read in the
    // order they were committed rather than an arbitrary one.
    rows.sort_by_key(|p| {
        let tix = l.postings.tx[p.get()];
        (l.transactions.date[tix.get()], tix.0, p.0)
    });
    let normality = l.ledgers.normality[ledger.get()];
    let mut raw = Money::ZERO;
    rows.into_iter()
        .map(|p| {
            let change = l.postings.amount[p.get()];
            raw += change;
            RegisterLine {
                posting: p,
                transaction: l.postings.tx[p.get()],
                change,
                balance: normality.present(raw),
            }
        })
        .collect()
}

// ------------------------------------------------------------------ search

/// What the GUI search bar returns: every entity kind, one pass, ranked by
/// nothing more clever than "name matches beat description matches".
#[derive(Clone, Debug, Default)]
pub struct SearchHits {
    pub ledgers: Vec<LedgerIx>,
    pub transactions: Vec<TxIx>,
    pub issuers: Vec<IssuerIx>,
    pub buckets: Vec<crate::id::BucketIx>,
}

pub fn search(l: &Budget, needle: &str, limit_each: usize) -> SearchHits {
    let needle = needle.trim();
    if needle.is_empty() {
        return SearchHits::default();
    }
    SearchHits {
        ledgers: LedgerQuery::new()
            .filter(LedgerFilter::Text(needle.into()))
            .limit(limit_each)
            .run(l),
        transactions: TxQuery::new().filter(TxFilter::Text(needle.into())).limit(limit_each).run(l),
        issuers: IssuerQuery::new().filter(IssuerFilter::Text(needle.into())).run(l),
        buckets: l
            .buckets
            .live()
            .filter(|ix| {
                contains_ci(&l.buckets.name[ix.get()], needle)
                    || contains_ci(&l.buckets.description[ix.get()], needle)
            })
            .take(limit_each)
            .collect(),
    }
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}
