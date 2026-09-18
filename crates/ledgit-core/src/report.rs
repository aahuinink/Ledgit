//! The pre-commit report: "here is everything you are about to make permanent."
//!
//! Computed by materialising the budget twice - once at HEAD, once with the
//! staged ops folded in - and diffing the balance columns. Two folds of a few
//! thousand ops is microseconds, and it means the report can never disagree
//! with what the commit will actually do, because it *is* what the commit will
//! do.

use crate::error::Result;
use crate::id::{BucketIx, LedgerIx};
use crate::model::{magnitude, Normality, Parent};
use crate::money::Money;
use crate::op::Op;
use crate::query::{roll_up, LedgerSort, Order, RollUp};
use crate::state::Budget;
use std::collections::HashSet;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LedgerDelta {
    pub ledger: LedgerIx,
    pub name: String,
    pub normality: Normality,
    pub before: Money,
    pub after: Money,
    /// Number of staged transactions touching this ledger.
    pub postings: usize,
    /// True if this ledger did not exist before the staged ops.
    pub is_new: bool,
}

impl LedgerDelta {
    pub fn change(&self) -> Money {
        self.after - self.before
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BucketEffect {
    pub bucket: BucketIx,
    pub name: String,
    pub before: Money,
    pub after: Money,
    /// The bucket's membership changed, not just its members' balances.
    pub membership_changed: bool,
}

impl BucketEffect {
    pub fn change(&self) -> Money {
        self.after - self.before
    }
}

/// Everything the commit screen needs to show.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ChangeReport {
    /// One line per staged op, in order.
    pub lines: Vec<String>,
    pub new_ledgers: usize,
    pub new_buckets: usize,
    pub deleted_buckets: usize,
    pub new_issuers: usize,
    pub manual_transactions: usize,
    pub issuer_transactions: usize,
    /// How many of the above have more than two sides.
    pub split_transactions: usize,
    pub total_manual: Money,
    pub total_issued: Money,
    pub ledger_deltas: Vec<LedgerDelta>,
    pub bucket_effects: Vec<BucketEffect>,
    /// Debits equal credits in the resulting budget. Should always be true;
    /// if it is not, refuse to commit and file a bug.
    pub balanced: bool,
}

impl ChangeReport {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Diff `base` against `base + staged`.
pub fn build(base: &Budget, staged: &[Op]) -> Result<ChangeReport> {
    let mut after = base.clone();
    for op in staged {
        after.apply(op)?;
    }

    let mut r = ChangeReport {
        lines: staged.iter().map(|o| o.summary()).collect(),
        balanced: after.is_balanced(),
        ..Default::default()
    };

    // Ledgers one of the staged transactions actually posts against. A
    // bucket containing one of these is "affected" even if its total happens
    // to net to zero - that is information the commit screen must not hide.
    let mut touched: HashSet<usize> = HashSet::new();
    let mut postings_per_ledger = vec![0usize; after.ledgers.len()];

    for op in staged {
        match op {
            Op::CreateLedger { .. } => r.new_ledgers += 1,
            Op::CreateBucket { .. } => r.new_buckets += 1,
            Op::DeleteBucket { .. } => r.deleted_buckets += 1,
            Op::CreateIssuer { .. } => r.new_issuers += 1,
            Op::PostTransaction { parent, legs, .. } => {
                // The size of a split entry is what it debits, not the sum of
                // every leg - a $2,400 paycheque is $2,400, not $4,800.
                let amount = magnitude(legs);
                match parent {
                    Parent::Manual => {
                        r.manual_transactions += 1;
                        r.total_manual += amount;
                    }
                    Parent::Issuer(_) => {
                        r.issuer_transactions += 1;
                        r.total_issued += amount;
                    }
                }
                if legs.len() > 2 {
                    r.split_transactions += 1;
                }
                for leg in legs {
                    if let Some(ix) = after.ledgers.ix(leg.ledger) {
                        postings_per_ledger[ix.get()] += 1;
                        touched.insert(ix.get());
                    }
                }
            }
            _ => {}
        }
    }

    // Any ledger this batch created, posted against, or moved the balance of.
    let rows: Vec<usize> = (0..after.ledgers.len())
        .filter(|i| {
            *i >= base.ledgers.len()
                || touched.contains(i)
                || base.ledgers.raw_balance[*i] != after.ledgers.raw_balance[*i]
        })
        .collect();

    r.ledger_deltas = rows
        .into_iter()
        .map(|i| {
            let ix = LedgerIx(i as u32);
            let is_new = i >= base.ledgers.len();
            LedgerDelta {
                ledger: ix,
                name: after.ledgers.name[i].clone(),
                normality: after.ledgers.normality[i],
                before: if is_new { Money::ZERO } else { base.ledgers.balance(ix) },
                after: after.ledgers.balance(ix),
                postings: postings_per_ledger[i],
                is_new,
            }
        })
        .collect();

    // Every live bucket in either budget, so newly created and newly deleted
    // buckets both show up.
    let mut effects = Vec::new();
    for bix in after.buckets.live() {
        let uid = after.buckets.uid[bix.get()];
        let before_roll = roll_up(base, uid, RollUp::ByNormality, LedgerSort::Name, Order::Asc);
        let after_roll = roll_up(&after, uid, RollUp::ByNormality, LedgerSort::Name, Order::Asc)
            .expect("bucket is live");
        let membership_changed = match base.buckets.ix(uid) {
            Some(b) => base.buckets.members[b.get()] != after.buckets.members[bix.get()],
            None => true,
        };
        let before = before_roll.as_ref().map_or(Money::ZERO, |b| b.total);
        let holds_touched_ledger =
            after.buckets.members[bix.get()].iter().any(|a| touched.contains(&a.get()));
        if before != after_roll.total || membership_changed || holds_touched_ledger {
            effects.push(BucketEffect {
                bucket: bix,
                name: after_roll.name,
                before,
                after: after_roll.total,
                membership_changed,
            });
        }
    }
    for bix in base.buckets.live() {
        let uid = base.buckets.uid[bix.get()];
        if after.buckets.ix(uid).is_none() {
            let before = roll_up(base, uid, RollUp::ByNormality, LedgerSort::Name, Order::Asc)
                .map_or(Money::ZERO, |b| b.total);
            effects.push(BucketEffect {
                bucket: bix,
                name: base.buckets.name[bix.get()].clone(),
                before,
                after: Money::ZERO,
                membership_changed: true,
            });
        }
    }
    effects.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    r.bucket_effects = effects;

    Ok(r)
}
