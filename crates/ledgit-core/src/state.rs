//! The materialised budget: what you get by folding a list of [`Op`]s.
//!
//! Layout is struct-of-arrays. A budget question is almost always "this one
//! field, across every row" - sum the balances in a bucket, find transactions
//! in a date range, list credit-normal ledgers - so each field lives in its
//! own contiguous `Vec`. Summing a bucket touches one `i64` array and nothing
//! else; the names and descriptions never enter cache.
//!
//! The arenas are the *only* mutable state, and the only way to mutate them is
//! [`Budget::apply`]. Everything else in the crate reads.

use crate::date::Date;
use crate::error::{invalid, Error, Result};
use crate::id::{
    BucketIx, BucketUid, IssuerIx, IssuerUid, LedgerIx, LedgerUid, PostingIx, TxIx, TxUid,
};
use crate::model::{
    magnitude, validate_legs, Bucket, Issuer, Ledger, Leg, Normality, Parent, Schedule, Transaction,
};
use crate::money::Money;
use crate::op::Op;
use std::collections::HashMap;

/// Columnar storage for ledgers. Row `i` of every vector describes ledger `i`.
#[derive(Clone, Default, Debug)]
pub struct LedgerArena {
    pub uid: Vec<LedgerUid>,
    pub name: Vec<String>,
    pub description: Vec<String>,
    pub normality: Vec<Normality>,
    pub opened: Vec<Date>,
    /// Debit-positive running total, the column bucket maths sums over.
    pub raw_balance: Vec<Money>,
    /// Postings against each ledger, in the order they were entered. These
    /// index the flat [`PostingArena`], so walking a ledger's register reads
    /// two contiguous arrays and never touches a transaction it does not need.
    pub postings: Vec<Vec<PostingIx>>,
    by_uid: HashMap<LedgerUid, u32>,
}

impl LedgerArena {
    pub fn len(&self) -> usize {
        self.uid.len()
    }
    pub fn is_empty(&self) -> bool {
        self.uid.is_empty()
    }
    pub fn ix(&self, uid: LedgerUid) -> Option<LedgerIx> {
        self.by_uid.get(&uid).copied().map(LedgerIx)
    }
    pub fn get(&self, ix: LedgerIx) -> Ledger {
        let i = ix.get();
        Ledger {
            uid: self.uid[i],
            name: self.name[i].clone(),
            description: self.description[i].clone(),
            normality: self.normality[i],
            opened: self.opened[i],
            raw_balance: self.raw_balance[i],
        }
    }
    pub fn indices(&self) -> impl Iterator<Item = LedgerIx> {
        (0..self.uid.len() as u32).map(LedgerIx)
    }
    /// The balance as displayed for this ledger's normality.
    pub fn balance(&self, ix: LedgerIx) -> Money {
        self.normality[ix.get()].present(self.raw_balance[ix.get()])
    }
}

/// The flat arena of postings: every side of every transaction, in one place.
///
/// A transaction's legs are *contiguous* here, which is the whole point of
/// flattening. The obvious encoding, a `Vec<Vec<Leg>>` on the transaction
/// arena, would put every entry's sides in their own heap allocation, so
/// totalling a month of spending would chase one pointer per transaction.
/// Here it is a linear walk of two arrays.
#[derive(Clone, Default, Debug)]
pub struct PostingArena {
    pub ledger: Vec<LedgerIx>,
    /// Debit-positive, and guaranteed to sum to zero within one transaction.
    pub amount: Vec<Money>,
    /// Which transaction this posting belongs to.
    pub tx: Vec<TxIx>,
}

impl PostingArena {
    pub fn len(&self) -> usize {
        self.ledger.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ledger.is_empty()
    }
    pub fn leg(&self, ix: PostingIx, ledgers: &LedgerArena) -> Leg {
        Leg { ledger: ledgers.uid[self.ledger[ix.get()].get()], amount: self.amount[ix.get()] }
    }
}

/// Columnar storage for transactions.
///
/// The legs are not here; they live in the [`PostingArena`], and each
/// transaction records the half-open range it owns. That keeps this arena's
/// rows fixed-size, so scanning by date stays a walk over one `i32` column.
#[derive(Clone, Default, Debug)]
pub struct TxArena {
    pub uid: Vec<TxUid>,
    pub name: Vec<String>,
    pub description: Vec<String>,
    pub date: Vec<Date>,
    pub parent: Vec<Parent>,
    /// First posting of this transaction in the posting arena.
    pub leg_start: Vec<u32>,
    /// How many postings it has. Always at least two.
    pub leg_len: Vec<u32>,
    by_uid: HashMap<TxUid, u32>,
}

impl TxArena {
    pub fn len(&self) -> usize {
        self.uid.len()
    }
    pub fn is_empty(&self) -> bool {
        self.uid.is_empty()
    }
    pub fn ix(&self, uid: TxUid) -> Option<TxIx> {
        self.by_uid.get(&uid).copied().map(TxIx)
    }
    pub fn indices(&self) -> impl Iterator<Item = TxIx> {
        (0..self.uid.len() as u32).map(TxIx)
    }
    /// The half-open range of postings belonging to transaction `ix`.
    pub fn leg_range(&self, ix: TxIx) -> std::ops::Range<usize> {
        let i = ix.get();
        let start = self.leg_start[i] as usize;
        start..start + self.leg_len[i] as usize
    }
    pub fn is_split(&self, ix: TxIx) -> bool {
        self.leg_len[ix.get()] > 2
    }
}

/// Columnar storage for issuers.
#[derive(Clone, Default, Debug)]
pub struct IssuerArena {
    pub uid: Vec<IssuerUid>,
    pub name: Vec<String>,
    pub description: Vec<String>,
    /// The entry this issuer posts each time it fires, held as uids rather
    /// than row indices because that is what it emits into an op. There are a
    /// handful of issuers, so a nested `Vec` costs nothing measurable here -
    /// unlike on the posting arena, which is scanned.
    pub legs: Vec<Vec<Leg>>,
    pub schedule: Vec<Schedule>,
    pub start: Vec<Date>,
    pub emitted_through: Vec<Option<Date>>,
    pub paused: Vec<bool>,
    by_uid: HashMap<IssuerUid, u32>,
}

impl IssuerArena {
    pub fn len(&self) -> usize {
        self.uid.len()
    }
    pub fn is_empty(&self) -> bool {
        self.uid.is_empty()
    }
    pub fn ix(&self, uid: IssuerUid) -> Option<IssuerIx> {
        self.by_uid.get(&uid).copied().map(IssuerIx)
    }
    pub fn indices(&self) -> impl Iterator<Item = IssuerIx> {
        (0..self.uid.len() as u32).map(IssuerIx)
    }
    pub fn get(&self, ix: IssuerIx) -> Issuer {
        let i = ix.get();
        Issuer {
            uid: self.uid[i],
            name: self.name[i].clone(),
            description: self.description[i].clone(),
            legs: self.legs[i].clone(),
            schedule: self.schedule[i],
            start: self.start[i],
            emitted_through: self.emitted_through[i],
            paused: self.paused[i],
        }
    }

    /// The size of the entry this issuer posts.
    pub fn amount(&self, ix: IssuerIx) -> Money {
        magnitude(&self.legs[ix.get()])
    }
}

/// Columnar storage for buckets. Deleted buckets keep their row (so indices
/// stay stable) and are marked not `alive`.
#[derive(Clone, Default, Debug)]
pub struct BucketArena {
    pub uid: Vec<BucketUid>,
    pub name: Vec<String>,
    pub description: Vec<String>,
    pub members: Vec<Vec<LedgerIx>>,
    pub alive: Vec<bool>,
    by_uid: HashMap<BucketUid, u32>,
}

impl BucketArena {
    pub fn len(&self) -> usize {
        self.uid.len()
    }
    pub fn is_empty(&self) -> bool {
        self.live().next().is_none()
    }
    pub fn ix(&self, uid: BucketUid) -> Option<BucketIx> {
        self.by_uid.get(&uid).copied().filter(|i| self.alive[*i as usize]).map(BucketIx)
    }
    pub fn live(&self) -> impl Iterator<Item = BucketIx> + '_ {
        (0..self.uid.len() as u32).filter(|i| self.alive[*i as usize]).map(BucketIx)
    }
    pub fn get(&self, ix: BucketIx, ledgers: &LedgerArena) -> Bucket {
        let i = ix.get();
        Bucket {
            uid: self.uid[i],
            name: self.name[i].clone(),
            description: self.description[i].clone(),
            members: self.members[i].iter().map(|a| ledgers.uid[a.get()]).collect(),
        }
    }
}

/// The whole budget, materialised.
#[derive(Clone, Default, Debug)]
pub struct Budget {
    pub ledgers: LedgerArena,
    pub transactions: TxArena,
    pub postings: PostingArena,
    pub issuers: IssuerArena,
    pub buckets: BucketArena,
}

impl Budget {
    pub fn new() -> Budget {
        Budget::default()
    }

    /// Fold a sequence of operations into a fresh budget.
    pub fn replay<'a>(ops: impl IntoIterator<Item = &'a Op>) -> Result<Budget> {
        let mut l = Budget::new();
        for op in ops {
            l.apply(op)?;
        }
        Ok(l)
    }

    fn ledger_ix(&self, uid: LedgerUid) -> Result<LedgerIx> {
        self.ledgers.ix(uid).ok_or(Error::NoSuchEntity { kind: "ledger", uid: uid.0 })
    }

    /// A full snapshot of one transaction, legs included.
    pub fn transaction(&self, ix: TxIx) -> Transaction {
        let i = ix.get();
        Transaction {
            uid: self.transactions.uid[i],
            name: self.transactions.name[i].clone(),
            description: self.transactions.description[i].clone(),
            date: self.transactions.date[i],
            legs: self
                .transactions
                .leg_range(ix)
                .map(|p| self.postings.leg(PostingIx(p as u32), &self.ledgers))
                .collect(),
            parent: self.transactions.parent[i],
        }
    }

    /// The size of an entry: the total debited, which equals the total credited.
    pub fn amount_of(&self, ix: TxIx) -> Money {
        self.transactions.leg_range(ix).map(|p| self.postings.amount[p]).filter(|m| m.0 > 0).sum()
    }

    /// The signed, debit-positive effect of one transaction on one ledger.
    ///
    /// With splits a ledger can only appear once per entry (`validate_legs`
    /// enforces it), so this is a scan of at most a handful of contiguous slots.
    pub fn effect_on(&self, ix: TxIx, ledger: LedgerIx) -> Money {
        self.transactions
            .leg_range(ix)
            .filter(|p| self.postings.ledger[*p] == ledger)
            .map(|p| self.postings.amount[p])
            .sum()
    }

    /// The other ledgers an entry touches, from one ledger's point of view.
    pub fn counterparties(&self, ix: TxIx, ledger: LedgerIx) -> Vec<LedgerIx> {
        self.transactions
            .leg_range(ix)
            .map(|p| self.postings.ledger[p])
            .filter(|a| *a != ledger)
            .collect()
    }

    /// The ledgers on the *other* side of an entry from `ledger`.
    ///
    /// This is what a register wants, and it is not the same as
    /// [`Budget::counterparties`]. Looking at chequing in a paycheque, the
    /// useful answer is "Gross pay" - the thing the money came from - not
    /// "Tax withheld, Pension", which sit on the same side as you do.
    pub fn other_side(&self, ix: TxIx, ledger: LedgerIx) -> Vec<LedgerIx> {
        let Some(mine) = self
            .transactions
            .leg_range(ix)
            .find(|p| self.postings.ledger[*p] == ledger)
            .map(|p| self.postings.amount[p].0 > 0)
        else {
            return self.counterparties(ix, ledger);
        };
        self.transactions
            .leg_range(ix)
            .filter(|p| (self.postings.amount[*p].0 > 0) != mine)
            .map(|p| self.postings.ledger[p])
            .collect()
    }

    /// Apply one operation. On `Err` the budget is unchanged: every operation
    /// validates fully before it writes anything.
    pub fn apply(&mut self, op: &Op) -> Result<()> {
        match op.clone() {
            Op::CreateLedger { uid, name, description, normality, opened } => {
                if self.ledgers.ix(uid).is_some() {
                    return Err(Error::Duplicate { kind: "ledger", uid: uid.0 });
                }
                let name = check_name(name, "ledger name")?;
                let a = &mut self.ledgers;
                a.by_uid.insert(uid, a.uid.len() as u32);
                a.uid.push(uid);
                a.name.push(name);
                a.description.push(description);
                a.normality.push(normality);
                a.opened.push(opened);
                a.raw_balance.push(Money::ZERO);
                a.postings.push(Vec::new());
            }
            Op::EditLedger { uid, name, description } => {
                let ix = self.ledger_ix(uid)?.get();
                if let Some(n) = name {
                    self.ledgers.name[ix] = check_name(n, "ledger name")?;
                }
                if let Some(d) = description {
                    self.ledgers.description[ix] = d;
                }
            }
            Op::PostTransaction { uid, name, description, date, legs, parent } => {
                if self.transactions.ix(uid).is_some() {
                    return Err(Error::Duplicate { kind: "transaction", uid: uid.0 });
                }
                validate_legs(&legs).map_err(invalid)?;
                let name = check_name(name, "transaction name")?;
                // Resolve every ledger before writing anything, so a bad leg
                // leaves the budget exactly as it was.
                let resolved: Vec<LedgerIx> =
                    legs.iter().map(|l| self.ledger_ix(l.ledger)).collect::<Result<Vec<_>>>()?;
                if let Parent::Issuer(i) = parent {
                    if self.issuers.ix(i).is_none() {
                        return Err(Error::NoSuchEntity { kind: "issuer", uid: i.0 });
                    }
                }

                let tix = TxIx(self.transactions.uid.len() as u32);
                let leg_start = self.postings.len() as u32;
                for (leg, aix) in legs.iter().zip(&resolved) {
                    let pix = PostingIx(self.postings.len() as u32);
                    self.postings.ledger.push(*aix);
                    self.postings.amount.push(leg.amount);
                    self.postings.tx.push(tix);
                    self.ledgers.raw_balance[aix.get()] += leg.amount;
                    self.ledgers.postings[aix.get()].push(pix);
                }

                let t = &mut self.transactions;
                t.by_uid.insert(uid, tix.0);
                t.uid.push(uid);
                t.name.push(name);
                t.description.push(description);
                t.date.push(date);
                t.parent.push(parent);
                t.leg_start.push(leg_start);
                t.leg_len.push(legs.len() as u32);
            }
            Op::EditTransaction { uid, name, description } => {
                let ix = self
                    .transactions
                    .ix(uid)
                    .ok_or(Error::NoSuchEntity { kind: "transaction", uid: uid.0 })?
                    .get();
                if let Some(n) = name {
                    self.transactions.name[ix] = check_name(n, "transaction name")?;
                }
                if let Some(d) = description {
                    self.transactions.description[ix] = d;
                }
            }
            Op::CreateIssuer { uid, name, description, legs, schedule, start } => {
                if self.issuers.ix(uid).is_some() {
                    return Err(Error::Duplicate { kind: "issuer", uid: uid.0 });
                }
                validate_legs(&legs).map_err(invalid)?;
                schedule.validate().map_err(invalid)?;
                let name = check_name(name, "issuer name")?;
                for leg in &legs {
                    self.ledger_ix(leg.ledger)?;
                }
                let s = &mut self.issuers;
                s.by_uid.insert(uid, s.uid.len() as u32);
                s.uid.push(uid);
                s.name.push(name);
                s.description.push(description);
                s.legs.push(legs);
                s.schedule.push(schedule);
                s.start.push(start);
                s.emitted_through.push(None);
                s.paused.push(false);
            }
            Op::EditIssuer { uid, name, description } => {
                let ix = self.issuer_ix(uid)?.get();
                if let Some(n) = name {
                    self.issuers.name[ix] = check_name(n, "issuer name")?;
                }
                if let Some(d) = description {
                    self.issuers.description[ix] = d;
                }
            }
            Op::SetIssuerPaused { uid, paused } => {
                let ix = self.issuer_ix(uid)?.get();
                self.issuers.paused[ix] = paused;
            }
            Op::AdvanceIssuer { uid, through } => {
                let ix = self.issuer_ix(uid)?.get();
                let current = self.issuers.emitted_through[ix];
                if current.is_some_and(|c| c >= through) {
                    // Idempotent: replaying an older advance must not rewind.
                    return Ok(());
                }
                self.issuers.emitted_through[ix] = Some(through);
            }
            Op::CreateBucket { uid, name, description } => {
                if self.buckets.ix(uid).is_some() {
                    return Err(Error::Duplicate { kind: "bucket", uid: uid.0 });
                }
                let name = check_name(name, "bucket name")?;
                let b = &mut self.buckets;
                match b.by_uid.get(&uid).copied() {
                    // Resurrecting a deleted bucket reuses its row.
                    Some(i) => {
                        let i = i as usize;
                        b.name[i] = name;
                        b.description[i] = description;
                        b.members[i].clear();
                        b.alive[i] = true;
                    }
                    None => {
                        b.by_uid.insert(uid, b.uid.len() as u32);
                        b.uid.push(uid);
                        b.name.push(name);
                        b.description.push(description);
                        b.members.push(Vec::new());
                        b.alive.push(true);
                    }
                }
            }
            Op::EditBucket { uid, name, description } => {
                let ix = self.bucket_ix(uid)?.get();
                if let Some(n) = name {
                    self.buckets.name[ix] = check_name(n, "bucket name")?;
                }
                if let Some(d) = description {
                    self.buckets.description[ix] = d;
                }
            }
            Op::DeleteBucket { uid } => {
                let ix = self.bucket_ix(uid)?.get();
                self.buckets.alive[ix] = false;
                self.buckets.members[ix].clear();
            }
            Op::AddToBucket { bucket, ledger } => {
                let bix = self.bucket_ix(bucket)?.get();
                let aix = self.ledger_ix(ledger)?;
                if !self.buckets.members[bix].contains(&aix) {
                    self.buckets.members[bix].push(aix);
                }
            }
            Op::RemoveFromBucket { bucket, ledger } => {
                let bix = self.bucket_ix(bucket)?.get();
                let aix = self.ledger_ix(ledger)?;
                self.buckets.members[bix].retain(|m| *m != aix);
            }
        }
        Ok(())
    }

    fn issuer_ix(&self, uid: IssuerUid) -> Result<IssuerIx> {
        self.issuers.ix(uid).ok_or(Error::NoSuchEntity { kind: "issuer", uid: uid.0 })
    }

    fn bucket_ix(&self, uid: BucketUid) -> Result<BucketIx> {
        self.buckets.ix(uid).ok_or(Error::NoSuchEntity { kind: "bucket", uid: uid.0 })
    }

    /// Debits must equal credits across the whole budget. If this ever fails,
    /// the bug is in `apply`, not in the user's data.
    pub fn is_balanced(&self) -> bool {
        self.ledgers.raw_balance.iter().copied().sum::<Money>() == Money::ZERO
    }

    /// Look up a ledger by name, case-insensitively. Names are not unique by
    /// construction, so this returns the first match; the UI resolves by uid.
    pub fn ledger_by_name(&self, name: &str) -> Option<LedgerIx> {
        self.ledgers
            .name
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
            .map(|i| LedgerIx(i as u32))
    }

    pub fn bucket_by_name(&self, name: &str) -> Option<BucketIx> {
        self.buckets.live().find(|ix| self.buckets.name[ix.get()].eq_ignore_ascii_case(name))
    }

    pub fn issuer_by_name(&self, name: &str) -> Option<IssuerIx> {
        self.issuers
            .name
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
            .map(|i| IssuerIx(i as u32))
    }
}

fn check_name(name: String, what: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(invalid(format!("{what} cannot be blank")));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{simple_legs, Leg, Normality};

    fn acct(name: &str, n: Normality) -> (LedgerUid, Op) {
        let uid = LedgerUid::new();
        (
            uid,
            Op::CreateLedger {
                uid,
                name: name.into(),
                description: String::new(),
                normality: n,
                opened: Date::from_ymd(2024, 1, 1).unwrap(),
            },
        )
    }

    fn post(debit: LedgerUid, credit: LedgerUid, cents: i64) -> Op {
        split(simple_legs(debit, credit, Money(cents)))
    }

    fn split(legs: Vec<Leg>) -> Op {
        Op::PostTransaction {
            uid: TxUid::new(),
            name: "t".into(),
            description: String::new(),
            date: Date::from_ymd(2024, 1, 2).unwrap(),
            legs,
            parent: Parent::Manual,
        }
    }

    #[test]
    fn posting_moves_both_sides_and_stays_balanced() {
        let (cash, op1) = acct("Cash", Normality::Debit);
        let (loan, op2) = acct("Car Loan", Normality::Credit);
        let l = Budget::replay(&[op1, op2, post(loan, cash, 40_000)]).unwrap();

        let cash_ix = l.ledgers.ix(cash).unwrap();
        let loan_ix = l.ledgers.ix(loan).unwrap();
        // Cash was credited: it went down by $400.
        assert_eq!(l.ledgers.balance(cash_ix), Money(-40_000));
        // The loan was debited: a credit-normal ledger debited owes $400 less.
        assert_eq!(l.ledgers.balance(loan_ix), Money(-40_000));
        assert!(l.is_balanced());
        assert_eq!(l.ledgers.postings[cash_ix.get()].len(), 1);
        assert_eq!(l.postings.len(), 2, "one entry, two postings");
        assert_eq!(l.amount_of(TxIx(0)), Money(40_000));
        assert!(!l.transactions.is_split(TxIx(0)));
    }

    #[test]
    fn a_split_entry_posts_every_leg_and_stays_balanced() {
        let (net, op1) = acct("Chequing", Normality::Debit);
        let (tax, op2) = acct("Tax withheld", Normality::Debit);
        let (pension, op3) = acct("Pension", Normality::Debit);
        let (gross, op4) = acct("Gross pay", Normality::Credit);

        let l = Budget::replay(&[
            op1,
            op2,
            op3,
            op4,
            split(vec![
                Leg::debit(net, Money::from_major(1_800)),
                Leg::debit(tax, Money::from_major(500)),
                Leg::debit(pension, Money::from_major(100)),
                Leg::credit(gross, Money::from_major(2_400)),
            ]),
        ])
        .unwrap();

        assert!(l.is_balanced());
        assert_eq!(l.transactions.len(), 1, "a paycheque is one entry, not three");
        assert_eq!(l.postings.len(), 4);
        assert!(l.transactions.is_split(TxIx(0)));
        // The entry is $2,400 - what it debited - not $4,800.
        assert_eq!(l.amount_of(TxIx(0)), Money::from_major(2_400));

        let gross_ix = l.ledgers.ix(gross).unwrap();
        // A credit-normal income ledger credited $2,400 shows $2,400 earned.
        assert_eq!(l.ledgers.balance(gross_ix), Money::from_major(2_400));
        assert_eq!(l.ledgers.balance(l.ledgers.ix(tax).unwrap()), Money::from_major(500));

        // Every leg is contiguous, and each ledger sees exactly one posting.
        assert_eq!(l.transactions.leg_range(TxIx(0)), 0..4);
        for uid in [net, tax, pension, gross] {
            assert_eq!(l.ledgers.postings[l.ledgers.ix(uid).unwrap().get()].len(), 1);
        }
        assert_eq!(l.counterparties(TxIx(0), gross_ix).len(), 3);
    }

    #[test]
    fn an_unbalanced_split_is_refused_whole() {
        let (a, op1) = acct("A", Normality::Debit);
        let (b, op2) = acct("B", Normality::Debit);
        let (c, op3) = acct("C", Normality::Credit);
        let mut l = Budget::replay(&[op1, op2, op3]).unwrap();

        let err = l
            .apply(&split(vec![
                Leg::debit(a, Money(500)),
                Leg::debit(b, Money(500)),
                Leg::credit(c, Money(900)),
            ]))
            .unwrap_err();
        assert!(err.to_string().contains("out by 1.00"), "{err}");

        // Nothing was written: not the transaction, not a single posting.
        assert_eq!(l.transactions.len(), 0);
        assert_eq!(l.postings.len(), 0);
        assert!(l.ledgers.raw_balance.iter().all(|m| m.is_zero()));
    }

    #[test]
    fn a_leg_against_a_missing_ledger_writes_nothing() {
        let (a, op1) = acct("A", Normality::Debit);
        let (b, op2) = acct("B", Normality::Credit);
        let mut l = Budget::replay(&[op1, op2]).unwrap();

        // The first two legs are fine; the third names a ledger that does
        // not exist. The first two must not land.
        let err = l
            .apply(&split(vec![
                Leg::debit(a, Money(300)),
                Leg::credit(b, Money(100)),
                Leg::credit(LedgerUid::new(), Money(200)),
            ]))
            .unwrap_err();
        assert!(matches!(err, Error::NoSuchEntity { kind: "ledger", .. }));
        assert_eq!(l.postings.len(), 0);
        assert!(l.ledgers.raw_balance.iter().all(|m| m.is_zero()));
    }

    #[test]
    fn rejects_degenerate_transactions() {
        let (cash, op1) = acct("Cash", Normality::Debit);
        let (other, op2) = acct("Other", Normality::Debit);
        let mut l = Budget::replay(&[op1, op2]).unwrap();
        // Both sides against one ledger.
        assert!(l.apply(&post(cash, cash, 100)).is_err());
        // A side of zero.
        assert!(l.apply(&post(cash, other, 0)).is_err());
        // A negative simple transfer is *not* an error at this level - it is
        // just the same entry the other way round, and the legs still sum to
        // zero. Rejecting it is the convenience constructor's job, because
        // only there does "amount" mean a magnitude.
        assert!(l.apply(&post(cash, other, -5)).is_ok());
        assert_eq!(l.transactions.len(), 1);
        assert_eq!(l.ledgers.balance(l.ledgers.ix(cash).unwrap()), Money(-5));
    }

    #[test]
    fn transactions_cannot_reference_unknown_ledgers() {
        let mut l = Budget::new();
        let err = l.apply(&post(LedgerUid::new(), LedgerUid::new(), 100)).unwrap_err();
        assert!(matches!(err, Error::NoSuchEntity { kind: "ledger", .. }));
    }

    #[test]
    fn deleted_bucket_keeps_its_row_but_disappears() {
        let uid = BucketUid::new();
        let mut l = Budget::replay(&[Op::CreateBucket {
            uid,
            name: "Net Worth".into(),
            description: String::new(),
        }])
        .unwrap();
        assert!(l.buckets.ix(uid).is_some());
        l.apply(&Op::DeleteBucket { uid }).unwrap();
        assert!(l.buckets.ix(uid).is_none());
        assert_eq!(l.buckets.len(), 1);
        assert!(l.buckets.is_empty());
    }

    #[test]
    fn advancing_an_issuer_never_rewinds() {
        let (a, op1) = acct("A", Normality::Debit);
        let (b, op2) = acct("B", Normality::Credit);
        let uid = IssuerUid::new();
        let mut l = Budget::replay(&[
            op1,
            op2,
            Op::CreateIssuer {
                uid,
                name: "Rent".into(),
                description: String::new(),
                legs: simple_legs(a, b, Money(100)),
                schedule: Schedule::EveryNDays { n: 14 },
                start: Date::from_ymd(2024, 1, 1).unwrap(),
            },
        ])
        .unwrap();
        let late = Date::from_ymd(2024, 6, 1).unwrap();
        l.apply(&Op::AdvanceIssuer { uid, through: late }).unwrap();
        l.apply(&Op::AdvanceIssuer { uid, through: Date::from_ymd(2024, 2, 1).unwrap() }).unwrap();
        assert_eq!(l.issuers.emitted_through[0], Some(late));
    }
}
