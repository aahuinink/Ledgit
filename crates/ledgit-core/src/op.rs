//! Operations: every change to the budget, reified as plain data.
//!
//! This is the single most important type in the crate. A change is *not* a
//! method call and *not* a closure - it is a serialisable value. That one
//! decision buys the whole feature list:
//!
//! * a commit is a list of ops, so history is a DAG of data, not of diffs;
//! * `revert` is "append the inverse ops", never "delete a row";
//! * `rebase` is "replay these ops onto a different parent";
//! * the pre-commit report is a fold over the staged ops;
//! * the staging area survives a crash, because it can be written to disk.
//!
//! A closure could do none of those things.

use crate::date::Date;
use crate::id::{BucketUid, IssuerUid, LedgerUid, TxUid};
use crate::model::{magnitude, Leg, Normality, Parent, Schedule};
use crate::money::Money;
use serde::{Deserialize, Serialize};

/// One atomic change to the budget.
///
/// Note what is absent: there is no `DeleteLedger`, `DeleteTransaction` or
/// `DeleteIssuer`. Those entities are permanent by design; the only way to
/// take back a transaction is to post its reversal, which leaves both facts in
/// the record. Buckets are pure views, so they may be deleted.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    CreateLedger {
        uid: LedgerUid,
        name: String,
        description: String,
        normality: Normality,
        opened: Date,
    },
    /// Rename and/or redescribe a ledger. `None` leaves a field untouched.
    EditLedger {
        uid: LedgerUid,
        name: Option<String>,
        description: Option<String>,
    },
    /// Post an entry. `legs` holds two or more sides that sum to zero; a plain
    /// transfer is two legs, a paycheque is four.
    PostTransaction {
        uid: TxUid,
        name: String,
        description: String,
        date: Date,
        legs: Vec<Leg>,
        parent: Parent,
    },
    /// Only the name and description of a posted transaction may change.
    EditTransaction {
        uid: TxUid,
        name: Option<String>,
        description: Option<String>,
    },
    CreateIssuer {
        uid: IssuerUid,
        name: String,
        description: String,
        legs: Vec<Leg>,
        schedule: Schedule,
        start: Date,
    },
    EditIssuer {
        uid: IssuerUid,
        name: Option<String>,
        description: Option<String>,
    },
    SetIssuerPaused {
        uid: IssuerUid,
        paused: bool,
    },
    /// Records that the issuer has emitted every occurrence up to this date.
    /// Emitted as a sibling of the `PostTransaction` ops it explains, so that
    /// replaying history never double-posts a recurring payment.
    AdvanceIssuer {
        uid: IssuerUid,
        through: Date,
    },
    CreateBucket {
        uid: BucketUid,
        name: String,
        description: String,
    },
    EditBucket {
        uid: BucketUid,
        name: Option<String>,
        description: Option<String>,
    },
    /// Buckets are views over ledgers, so deleting one destroys no money.
    DeleteBucket {
        uid: BucketUid,
    },
    AddToBucket {
        bucket: BucketUid,
        ledger: LedgerUid,
    },
    RemoveFromBucket {
        bucket: BucketUid,
        ledger: LedgerUid,
    },
}

impl Op {
    /// A short line for the pre-commit report and the history view.
    pub fn summary(&self) -> String {
        match self {
            Op::CreateLedger { name, normality, .. } => {
                format!("create {normality}-normal ledger \"{name}\"")
            }
            Op::EditLedger { uid, .. } => format!("edit ledger {}", uid.short()),
            Op::PostTransaction { name, legs, date, .. } => {
                let amount = magnitude(legs);
                let split = if legs.len() > 2 {
                    format!(" split {} ways", legs.len())
                } else {
                    String::new()
                };
                format!("post {amount} on {date}{split} - \"{name}\"")
            }
            Op::EditTransaction { uid, .. } => format!("edit transaction {}", uid.short()),
            Op::CreateIssuer { name, legs, schedule, .. } => {
                format!("create issuer \"{name}\" for {} {}", magnitude(legs), schedule.describe())
            }
            Op::EditIssuer { uid, .. } => format!("edit issuer {}", uid.short()),
            Op::SetIssuerPaused { uid, paused: true } => format!("pause issuer {}", uid.short()),
            Op::SetIssuerPaused { uid, paused: false } => format!("resume issuer {}", uid.short()),
            Op::AdvanceIssuer { uid, through } => {
                format!("advance issuer {} through {through}", uid.short())
            }
            Op::CreateBucket { name, .. } => format!("create bucket \"{name}\""),
            Op::EditBucket { uid, .. } => format!("edit bucket {}", uid.short()),
            Op::DeleteBucket { uid } => format!("delete bucket {}", uid.short()),
            Op::AddToBucket { bucket, ledger } => {
                format!("add ledger {} to bucket {}", ledger.short(), bucket.short())
            }
            Op::RemoveFromBucket { bucket, ledger } => {
                format!("remove ledger {} from bucket {}", ledger.short(), bucket.short())
            }
        }
    }

    /// The sides of the entry, if this operation posts one. Used by the report
    /// to decide which ledgers and buckets a commit would disturb.
    pub fn legs(&self) -> Option<&[Leg]> {
        match self {
            Op::PostTransaction { legs, .. } => Some(legs),
            _ => None,
        }
    }

    /// The size of the entry this operation posts, if it posts one.
    pub fn amount(&self) -> Option<Money> {
        self.legs().map(magnitude)
    }
}
