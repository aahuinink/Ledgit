//! Ledgit core: a double-entry budget that is version controlled the way
//! source code is.
//!
//! # The one idea
//!
//! Every change is an [`Op`](op::Op) - a plain, serialisable value. A
//! [`Commit`](commit::Commit) is a list of ops plus its parents, identified by
//! the hash of its own content. The budget you look at is not stored; it is
//! *derived*, by folding ops in DAG order into columnar arenas
//! ([`Budget`](state::Budget)).
//!
//! Everything else falls out of that:
//!
//! | You want | It is |
//! |---|---|
//! | undo a mistake | append the inverse ops (a reversing entry) |
//! | branch a what-if budget | a second ref into the same DAG |
//! | rebase | replay ops onto a different parent |
//! | "nothing saved until I commit" | the staging area is a `Vec<Op>` |
//! | the pre-commit report | fold the staged ops and diff the balances |
//! | swap the database | persist four kinds of blob, not a schema |
//!
//! # Getting started
//!
//! ```
//! use ledgit_core::prelude::*;
//!
//! let mut repo = Repo::init(MemStore::new(), "me")?;
//! let opened = "2024-01-01".parse::<Date>().unwrap();
//! let cash = repo.add_ledger("Cash", "Chequing", Normality::Debit, opened)?;
//! let loan = repo.add_ledger("Car Loan", "", Normality::Credit, opened)?;
//! repo.post("Car payment", "", opened, Money::from_major(400), loan, cash)?;
//!
//! // A paycheque is one entry with four sides, not three transactions.
//! let tax = repo.add_ledger("Tax withheld", "", Normality::Debit, opened)?;
//! let gross = repo.add_ledger("Gross pay", "", Normality::Credit, opened)?;
//! repo.post_split("Paycheque", "", opened, vec![
//!     Leg::debit(cash, Money::from_major(1_800)),
//!     Leg::debit(tax, Money::from_major(600)),
//!     Leg::credit(gross, Money::from_major(2_400)),
//! ])?;
//!
//! // Nothing is history until you commit.
//! let report = repo.report()?;
//! assert_eq!(report.manual_transactions, 2);
//! repo.commit("open ledgers and make the first payment")?;
//! # Ok::<(), ledgit_core::Error>(())
//! ```

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod commit;
pub mod date;
pub mod error;
pub mod id;
pub mod issuer;
pub mod model;
pub mod money;
pub mod op;
pub mod query;
pub mod repo;
pub mod report;
pub mod state;
pub mod store;

pub use error::{Error, Result};

/// Everything a front end normally needs, in one `use`.
pub mod prelude {
    pub use crate::commit::{Commit, CommitId, Head};
    pub use crate::date::Date;
    pub use crate::error::{Error, Result};
    pub use crate::id::{BucketUid, IssuerUid, LedgerUid, TxUid};
    pub use crate::model::{
        magnitude, simple_legs, validate_legs, Bucket, Issuer, Ledger, Leg, Normality, Parent,
        Schedule, Transaction,
    };
    pub use crate::money::Money;
    pub use crate::op::Op;
    pub use crate::query::{
        balance_as_of, combine, register, roll_up, search, BucketLine, BucketRollUp, Combination,
        IssuerFilter, IssuerQuery, LedgerFilter, LedgerQuery, LedgerSort, Order, RegisterLine,
        RollUp, Sign, Term, TxFilter, TxQuery, TxSort,
    };
    pub use crate::repo::Repo;
    pub use crate::report::ChangeReport;
    pub use crate::state::Budget;
    pub use crate::store::{MemStore, Store, DEFAULT_BRANCH};
}
