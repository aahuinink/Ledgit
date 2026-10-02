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
use crate::id::{BucketUid, IssuerUid, LedgerUid, TxUid, ViewUid};
use crate::model::{
    magnitude, Alert, AmountRule, Leg, Normality, Parent, Schedule, Target, VarValue, ViewSpec,
};
use crate::money::Money;
use serde::{Deserialize, Serialize};

/// One atomic change to the budget.
///
/// Note what is absent: there is no `DeleteLedger`, `DeleteTransaction` or
/// `DeleteIssuer`. Those entities are permanent by design; the only way to
/// take back a transaction is to post its reversal, which leaves both facts in
/// the record. Buckets and saved views are pure readings of the
/// budget, so they may be deleted.
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
    /// Set a ledger's target - a balance or a pace - and alerts, replacing
    /// what it had.
    /// Like a ledger's name, these are settings on it, not money: they move
    /// nothing, and an empty set clears them.
    SetLedgerGoals {
        uid: LedgerUid,
        target: Option<Target>,
        alerts: Vec<Alert>,
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
        /// Set on a reversal: the transaction whose every side this one
        /// negates. Absent - not `null` - when unset, so every entry
        /// committed before reversals were linked encodes, and hashes,
        /// exactly as it did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reverses: Option<TxUid>,
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
        /// Work each amount out from a balance instead. Absent - not `null` -
        /// when unset, so every issuer committed before rules existed still
        /// encodes, and hashes, exactly as it did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<AmountRule>,
        /// The transaction this issuer pays off, if it was scheduled from
        /// one. Absent when unset, for the same reason as `rule`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settles: Option<TxUid>,
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
    /// Set the amount of one future occurrence ahead of time, or with `None`
    /// put it back to what the issuer would work out. For a statement, an
    /// amount under the minimum is raised to the minimum when it is posted.
    SetIssuerOverride {
        uid: IssuerUid,
        date: Date,
        amount: Option<Money>,
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
    /// Include every ledger at or below a path - `Wedding` takes in
    /// `Wedding:Tuxedo` and `Wedding:Venue` - including ledgers created or
    /// renamed into it later. The path need not name any ledger yet.
    AddSubtreeToBucket {
        bucket: BucketUid,
        path: String,
    },
    RemoveSubtreeFromBucket {
        bucket: BucketUid,
        path: String,
    },
    /// Save a view: a reading of the budget across time. It is versioned so
    /// that it travels with the file and can differ between branches, but
    /// like a bucket it moves no money and may be deleted.
    CreateView {
        uid: ViewUid,
        name: String,
        description: String,
        spec: ViewSpec,
    },
    /// A view's spec is small, so an edit replaces it whole rather than
    /// patching it field by field. `None` leaves a field untouched.
    EditView {
        uid: ViewUid,
        name: Option<String>,
        description: Option<String>,
        spec: Option<ViewSpec>,
    },
    DeleteView {
        uid: ViewUid,
    },
    /// Create a variable, or change its value. Names are matched without
    /// regard to case, so `car_km_rate` sets `Car_Km_Rate`.
    SetVariable {
        name: String,
        value: VarValue,
    },
    /// Variables only help you type entries; deleting one changes no entry
    /// already made with it.
    DeleteVariable {
        name: String,
    },
}

impl Op {
    /// The reversal of an entry: every side negated, linked back to it.
    ///
    /// Dated to match the original, not to today, so a correction lands in
    /// the period it belongs to and monthly totals stay true. A business
    /// would date it on the day of discovery; a personal budget wants the
    /// month to read correctly. Negating every side is correct for a
    /// four-leg paycheque for the same reason it is for a transfer: sum-zero
    /// in means sum-zero out.
    pub fn reversal(of: TxUid, name: &str, date: Date, legs: &[Leg], description: String) -> Op {
        Op::PostTransaction {
            uid: TxUid::new(),
            name: format!("Reversal of {name}"),
            description,
            date,
            legs: legs.iter().map(|l| Leg { ledger: l.ledger, amount: -l.amount }).collect(),
            parent: Parent::Manual,
            reverses: Some(of),
        }
    }

    /// A short line for the pre-commit report and the history view.
    pub fn summary(&self) -> String {
        match self {
            Op::CreateLedger { name, normality, .. } => {
                format!("create {normality}-normal ledger \"{name}\"")
            }
            Op::EditLedger { uid, .. } => format!("edit ledger {}", uid.short()),
            Op::SetLedgerGoals { uid, target, alerts } => {
                let target = match target {
                    Some(t) => format!("target {t}"),
                    None => "no target".into(),
                };
                format!("set ledger {} goals: {target}, {} alert(s)", uid.short(), alerts.len())
            }
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
            Op::CreateIssuer { name, rule: Some(rule), schedule, .. } => format!(
                "create issuer \"{name}\" for {} ledger {} {}",
                rule.describe(),
                rule.of().short(),
                schedule.describe()
            ),
            Op::CreateIssuer { name, legs, schedule, .. } => {
                format!("create issuer \"{name}\" for {} {}", magnitude(legs), schedule.describe())
            }
            Op::EditIssuer { uid, .. } => format!("edit issuer {}", uid.short()),
            Op::SetIssuerPaused { uid, paused: true } => format!("pause issuer {}", uid.short()),
            Op::SetIssuerPaused { uid, paused: false } => format!("resume issuer {}", uid.short()),
            Op::SetIssuerOverride { uid, date, amount: Some(m) } => {
                format!("pay {m} on {date} from issuer {}", uid.short())
            }
            Op::SetIssuerOverride { uid, date, amount: None } => {
                format!("clear issuer {}'s amount for {date}", uid.short())
            }
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
            Op::AddSubtreeToBucket { bucket, path } => {
                format!("add everything under \"{path}\" to bucket {}", bucket.short())
            }
            Op::RemoveSubtreeFromBucket { bucket, path } => {
                format!("remove everything under \"{path}\" from bucket {}", bucket.short())
            }
            Op::CreateView { name, .. } => format!("save view \"{name}\""),
            Op::EditView { uid, .. } => format!("edit view {}", uid.short()),
            Op::DeleteView { uid } => format!("delete view {}", uid.short()),
            Op::SetVariable { name, value: VarValue::Number(v) } => format!("set {name} = {v}"),
            Op::SetVariable { name, value: VarValue::Text(v) } => format!("set {name} = \"{v}\""),
            Op::DeleteVariable { name } => format!("delete variable {name}"),
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
