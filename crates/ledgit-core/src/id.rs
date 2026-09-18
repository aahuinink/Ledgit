//! Two kinds of identity, and they are not interchangeable.
//!
//! * A [`Uid`] is *stable*: it is minted once, written into the operation that
//!   creates an entity, and never changes. Operations reference entities by
//!   uid, so replaying the log in a different order - which is exactly what
//!   rebase does - still binds every transaction to the right ledger.
//!
//! * An index (`LedgerIx`, `TxIx`, ...) is *positional*: it is the row number
//!   in the in-memory arena, valid only for one materialisation of the budget.
//!   Indices are what the query engine and the balance columns use, because a
//!   `u32` row number is one cache line away from the data it points at.
//!
//! Mixing them up is the classic way to corrupt a versioned store, so they are
//! distinct types and the conversion only exists through the budget.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A 128-bit stable identifier, rendered as lowercase hex.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Uid(#[serde(with = "hex16")] pub [u8; 16]);

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl Uid {
    /// Mint a fresh uid. Time + a process-local counter + the process id are
    /// hashed together so that two budgets edited on two machines and later
    /// merged do not collide.
    pub fn new() -> Uid {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut h = Sha256::new();
        h.update(nanos.to_le_bytes());
        h.update(n.to_le_bytes());
        h.update(std::process::id().to_le_bytes());
        let digest = h.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        Uid(bytes)
    }

    /// A short, human-quotable prefix, the way git shows abbreviated hashes.
    pub fn short(&self) -> String {
        self.to_string()[..8].to_string()
    }

    pub fn parse(s: &str) -> Option<Uid> {
        let bytes = decode_hex(s, 16)?;
        let mut out = [0u8; 16];
        out.copy_from_slice(&bytes);
        Some(Uid(out))
    }
}

impl Default for Uid {
    fn default() -> Uid {
        Uid::new()
    }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.short())
    }
}

pub(crate) fn decode_hex(s: &str, len: usize) -> Option<Vec<u8>> {
    if s.len() != len * 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..len).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()).collect()
}

mod hex16 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 16], s: S) -> Result<S::Ok, S::Error> {
        let mut out = String::with_capacity(32);
        for b in bytes {
            out.push_str(&format!("{b:02x}"));
        }
        s.serialize_str(&out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 16], D::Error> {
        let s = String::deserialize(d)?;
        let v = super::decode_hex(&s, 16).ok_or_else(|| serde::de::Error::custom("bad uid"))?;
        let mut out = [0u8; 16];
        out.copy_from_slice(&v);
        Ok(out)
    }
}

/// Generates a newtype over `Uid` plus its positional counterpart, so an
/// `LedgerUid` can never be passed where a `TxUid` is expected.
macro_rules! entity_ids {
    ($(#[$m:meta])* $uid:ident, $ix:ident, $tag:literal) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $uid(pub Uid);

        impl $uid {
            pub fn new() -> Self {
                Self(Uid::new())
            }
            pub fn short(&self) -> String {
                self.0.short()
            }
            pub fn parse(s: &str) -> Option<Self> {
                Uid::parse(s).map(Self)
            }
            pub const TAG: &'static str = $tag;
        }

        impl Default for $uid {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $uid {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl fmt::Debug for $uid {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}:{}", $tag, self.0.short())
            }
        }

        #[doc = "Row index into the corresponding arena. Valid for one materialisation only."]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $ix(pub u32);

        impl $ix {
            pub fn get(self) -> usize {
                self.0 as usize
            }
        }
    };
}

entity_ids!(
    /// Stable identity of a ledger.
    LedgerUid, LedgerIx, "acct");
entity_ids!(
    /// Stable identity of a transaction.
    TxUid, TxIx, "txn");
entity_ids!(
    /// Stable identity of an issuer.
    IssuerUid, IssuerIx, "issr");
entity_ids!(
    /// Stable identity of a bucket.
    BucketUid, BucketIx, "bkt");

/// Row index into the flat posting arena - one side of one transaction.
///
/// Postings have no uid of their own: they are not addressable entities, they
/// are the contents of a transaction, and a transaction is addressed by its
/// [`TxUid`]. Giving them stable ids would invite operations that edit one side
/// of an entry, which is exactly the thing double-entry exists to prevent.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PostingIx(pub u32);

impl PostingIx {
    pub fn get(self) -> usize {
        self.0 as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uids_are_unique_and_round_trip() {
        let a = Uid::new();
        let b = Uid::new();
        assert_ne!(a, b);
        assert_eq!(Uid::parse(&a.to_string()), Some(a));
        assert_eq!(a.short().len(), 8);
        assert_eq!(Uid::parse("nope"), None);
    }

    #[test]
    fn uid_json_is_a_hex_string() {
        let a = LedgerUid::new();
        let j = serde_json::to_string(&a).unwrap();
        assert!(j.starts_with('"') && j.len() == 34, "{j}");
        assert_eq!(serde_json::from_str::<LedgerUid>(&j).unwrap(), a);
    }
}
