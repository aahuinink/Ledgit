//! The commit DAG. Same shape as git: a commit names its parents, carries a
//! payload, and is identified by the hash of its own content.
//!
//! The payload here is a list of [`Op`]s rather than a tree of files. That is
//! the only structural difference, and it is what makes a budget commit
//! meaningful: the diff *is* the content.

use crate::error::{Error, Result};
use crate::id::decode_hex;
use crate::op::Op;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// SHA-256 of a commit's canonical encoding.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CommitId(pub [u8; 32]);

impl CommitId {
    pub fn short(&self) -> String {
        self.to_string()[..10].to_string()
    }

    pub fn parse(s: &str) -> Option<CommitId> {
        let v = decode_hex(s, 32)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(&v);
        Some(CommitId(out))
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.short())
    }
}

impl From<CommitId> for String {
    fn from(c: CommitId) -> String {
        c.to_string()
    }
}

impl TryFrom<String> for CommitId {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<CommitId, String> {
        CommitId::parse(&s).ok_or_else(|| format!("not a commit id: {s}"))
    }
}

/// A commit, with its identity precomputed.
///
/// Field order matters: it defines the canonical JSON that gets hashed. Adding
/// a field in the middle changes every id in every existing budget, so append
/// only, and bump the format version in the store if you must.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Commit {
    pub id: CommitId,
    pub parents: Vec<CommitId>,
    pub author: String,
    /// Unix seconds. Display only - ordering comes from the DAG, never the clock.
    pub timestamp: i64,
    pub message: String,
    pub ops: Vec<Op>,
}

/// The hashed part of a commit: everything except the id itself.
#[derive(Serialize)]
struct Payload<'a> {
    parents: &'a [CommitId],
    author: &'a str,
    timestamp: i64,
    message: &'a str,
    ops: &'a [Op],
}

impl Commit {
    pub fn new(
        parents: Vec<CommitId>,
        author: impl Into<String>,
        timestamp: i64,
        message: impl Into<String>,
        ops: Vec<Op>,
    ) -> Result<Commit> {
        let (author, message) = (author.into(), message.into());
        let id = hash_payload(&Payload {
            parents: &parents,
            author: &author,
            timestamp,
            message: &message,
            ops: &ops,
        })?;
        Ok(Commit { id, parents, author, timestamp, message, ops })
    }

    /// Recompute the id and compare. Catches a corrupted or tampered store.
    pub fn verify(&self) -> Result<()> {
        let expect = hash_payload(&Payload {
            parents: &self.parents,
            author: &self.author,
            timestamp: self.timestamp,
            message: &self.message,
            ops: &self.ops,
        })?;
        if expect == self.id {
            Ok(())
        } else {
            Err(Error::Store(format!(
                "commit {} does not hash to its own id (store is corrupt)",
                self.id.short()
            )))
        }
    }

    pub fn is_root(&self) -> bool {
        self.parents.is_empty()
    }

    pub fn summary(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

fn hash_payload(p: &Payload<'_>) -> Result<CommitId> {
    let json = serde_json::to_vec(p)?;
    let mut h = Sha256::new();
    // Domain separation, so a commit id can never collide with some other
    // hash we might store later.
    h.update(b"ledgit.commit.v1\0");
    h.update(&json);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    Ok(CommitId(out))
}

/// Where the working budget currently sits.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Head {
    /// On a branch: committing moves the branch forward.
    Branch { name: String },
    /// Detached at a commit: committing moves nothing but `HEAD` itself.
    Detached { at: CommitId },
}

impl Head {
    pub fn branch_name(&self) -> Option<&str> {
        match self {
            Head::Branch { name } => Some(name),
            Head::Detached { .. } => None,
        }
    }
}

impl fmt::Display for Head {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Head::Branch { name } => write!(f, "{name}"),
            Head::Detached { at } => write!(f, "detached at {}", at.short()),
        }
    }
}

/// Branch names live in one flat namespace. Reject the characters that would
/// make a name ambiguous with a revision expression.
pub fn validate_branch_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::Invalid("branch name cannot be empty".into()));
    }
    let bad = |c: char| c.is_whitespace() || c.is_control() || "~^:?*[]\\\"'".contains(c);
    if name.chars().any(bad) || name.starts_with('-') || CommitId::parse(name).is_some() {
        return Err(Error::Invalid(format!("not a usable branch name: {name}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(msg: &str, parents: Vec<CommitId>) -> Commit {
        Commit::new(parents, "tester", 1_700_000_000, msg, vec![]).unwrap()
    }

    #[test]
    fn identical_content_hashes_identically() {
        assert_eq!(commit("a", vec![]).id, commit("a", vec![]).id);
        assert_ne!(commit("a", vec![]).id, commit("b", vec![]).id);
    }

    #[test]
    fn parents_are_part_of_identity() {
        let root = commit("root", vec![]);
        let a = commit("same", vec![root.id]);
        let b = commit("same", vec![]);
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn verify_catches_tampering() {
        let mut c = commit("original", vec![]);
        assert!(c.verify().is_ok());
        c.message = "rewritten".into();
        assert!(c.verify().is_err());
    }

    #[test]
    fn commit_ids_round_trip_through_text() {
        let c = commit("x", vec![]);
        assert_eq!(CommitId::parse(&c.id.to_string()), Some(c.id));
        assert_eq!(c.id.short().len(), 10);
    }

    #[test]
    fn branch_names_reject_ambiguity() {
        assert!(validate_branch_name("main").is_ok());
        assert!(validate_branch_name("feature/car-loan").is_ok());
        assert!(validate_branch_name("has space").is_err());
        assert!(validate_branch_name("head~1").is_err());
        assert!(validate_branch_name("").is_err());
    }
}
