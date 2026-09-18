use crate::id::Uid;
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// An operation referenced an entity that does not exist in this budget.
    NoSuchEntity { kind: &'static str, uid: Uid },
    /// An operation would have created an entity that already exists.
    Duplicate { kind: &'static str, uid: Uid },
    /// The operation is malformed (zero amount, self-transfer, empty name...).
    Invalid(String),
    /// A named branch, commit, or revision could not be resolved.
    NoSuchRef(String),
    /// The requested history rewrite cannot be performed.
    History(String),
    /// The backing store failed.
    Store(String),
    /// Serialisation of a commit or operation failed.
    Encoding(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoSuchEntity { kind, uid } => write!(f, "no such {kind}: {}", uid.short()),
            Error::Duplicate { kind, uid } => write!(f, "{kind} already exists: {}", uid.short()),
            Error::Invalid(m) => write!(f, "invalid operation: {m}"),
            Error::NoSuchRef(r) => write!(f, "no such revision: {r}"),
            Error::History(m) => write!(f, "cannot rewrite history: {m}"),
            Error::Store(m) => write!(f, "store error: {m}"),
            Error::Encoding(m) => write!(f, "encoding error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::Encoding(e.to_string())
    }
}

pub(crate) fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}
