//! The persistence boundary.
//!
//! Note how small it is. Because the budget is *derived* by folding the commit
//! DAG, the only things that must survive a power cut are commits, refs, HEAD,
//! the staging area, and staged work shelved on a branch you switched away from. There are no ledger rows, no balance columns, no
//! indexes to keep in sync - and therefore no way for the database to disagree
//! with the budget.
//!
//! That is the whole reason to swap SQLite out later: the replacement has to
//! store five kinds of blob, not a schema.

use crate::commit::{Commit, CommitId, Head};
use crate::error::Result;
use crate::op::Op;
use std::collections::HashMap;

/// The default branch created by `Repo::init`.
pub const DEFAULT_BRANCH: &str = "main";

pub trait Store {
    /// Write a commit. Commits are immutable and content-addressed, so writing
    /// one that already exists must succeed and change nothing.
    fn put_commit(&mut self, commit: &Commit) -> Result<()>;
    fn get_commit(&self, id: &CommitId) -> Result<Option<Commit>>;
    fn all_commit_ids(&self) -> Result<Vec<CommitId>>;

    /// Point a branch at a commit, or delete it with `None`.
    fn set_ref(&mut self, name: &str, target: Option<CommitId>) -> Result<()>;
    fn get_ref(&self, name: &str) -> Result<Option<CommitId>>;
    fn list_refs(&self) -> Result<Vec<(String, CommitId)>>;

    fn get_head(&self) -> Result<Option<Head>>;
    fn set_head(&mut self, head: &Head) -> Result<()>;

    /// The staging area: work in progress that is deliberately *not* history.
    /// It is persisted so a crash does not lose an afternoon of data entry,
    /// but it never appears in the log until it is committed.
    fn get_stage(&self) -> Result<Vec<Op>>;
    fn set_stage(&mut self, ops: &[Op]) -> Result<()>;

    /// Staged work set aside on a branch: shelved when you switch away from
    /// it, put back when you return. Like the stage, never history. An empty
    /// list means nothing is shelved there.
    fn get_shelf(&self, branch: &str) -> Result<Vec<Op>>;
    fn set_shelf(&mut self, branch: &str, ops: &[Op]) -> Result<()>;

    /// Make everything written so far durable.
    fn flush(&mut self) -> Result<()>;
}

/// A store that forgets everything when dropped. Used by the tests, and by any
/// front end that wants a scratch budget.
#[derive(Default, Debug)]
pub struct MemStore {
    commits: HashMap<CommitId, Commit>,
    refs: HashMap<String, CommitId>,
    head: Option<Head>,
    stage: Vec<Op>,
    shelves: HashMap<String, Vec<Op>>,
}

impl MemStore {
    pub fn new() -> MemStore {
        MemStore::default()
    }
}

impl Store for MemStore {
    fn put_commit(&mut self, commit: &Commit) -> Result<()> {
        self.commits.insert(commit.id, commit.clone());
        Ok(())
    }

    fn get_commit(&self, id: &CommitId) -> Result<Option<Commit>> {
        Ok(self.commits.get(id).cloned())
    }

    fn all_commit_ids(&self) -> Result<Vec<CommitId>> {
        Ok(self.commits.keys().copied().collect())
    }

    fn set_ref(&mut self, name: &str, target: Option<CommitId>) -> Result<()> {
        match target {
            Some(id) => self.refs.insert(name.to_string(), id),
            None => self.refs.remove(name),
        };
        Ok(())
    }

    fn get_ref(&self, name: &str) -> Result<Option<CommitId>> {
        Ok(self.refs.get(name).copied())
    }

    fn list_refs(&self) -> Result<Vec<(String, CommitId)>> {
        let mut v: Vec<_> = self.refs.iter().map(|(k, c)| (k.clone(), *c)).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(v)
    }

    fn get_head(&self) -> Result<Option<Head>> {
        Ok(self.head.clone())
    }

    fn set_head(&mut self, head: &Head) -> Result<()> {
        self.head = Some(head.clone());
        Ok(())
    }

    fn get_stage(&self) -> Result<Vec<Op>> {
        Ok(self.stage.clone())
    }

    fn set_stage(&mut self, ops: &[Op]) -> Result<()> {
        self.stage = ops.to_vec();
        Ok(())
    }

    fn get_shelf(&self, branch: &str) -> Result<Vec<Op>> {
        Ok(self.shelves.get(branch).cloned().unwrap_or_default())
    }

    fn set_shelf(&mut self, branch: &str, ops: &[Op]) -> Result<()> {
        if ops.is_empty() {
            self.shelves.remove(branch);
        } else {
            self.shelves.insert(branch.to_string(), ops.to_vec());
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
