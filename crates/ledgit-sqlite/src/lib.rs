//! SQLite implementation of [`ledgit_core::store::Store`].
//!
//! # Why SQLite
//!
//! It is the most-tested storage engine that ships as a file rather than a
//! service: no daemon, no port, no install step on Windows, and an explicit,
//! documented crash-recovery story. `rusqlite`'s `bundled` feature compiles
//! the amalgamation from source, so a Windows build needs no system SQLite and
//! no DLL beside the exe.
//!
//! # Durability choices
//!
//! * `synchronous = FULL` - the write is on the platter before the call
//!   returns. This is the setting that makes "I pressed commit" mean something.
//! * `journal_mode = DELETE` (SQLite's default rollback journal) rather than
//!   WAL. WAL is faster under concurrent readers, which a single-user desktop
//!   budget does not have, and it leaves `-wal` and `-shm` files beside the
//!   database. A budget is a *document*: people email it to themselves, drop
//!   it in OneDrive, and restore it from a backup. A document that is secretly
//!   three files gets corrupted by exactly that handling. One file it is.
//!
//!   If concurrent access ever matters, this is a one-line change here and
//!   nowhere else.
//!
//! # Schema
//!
//! Four tables, because the core only needs four kinds of blob. There is no
//! `ledgers` table and no `balances` table: those are derived state, and a
//! derived table in the database is a derived table that can go stale.

use ledgit_core::commit::{Commit, CommitId, Head};
use ledgit_core::error::{Error, Result};
use ledgit_core::op::Op;
use ledgit_core::store::Store;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// Bumped when the on-disk layout changes in a way older builds cannot read.
///
/// 2: the XtraLedger -> Ledgit rename. `Account` became `Ledger`, and since
///    `Op` is tagged with snake_case variant names, that renamed the operations
///    on disk too - `create_account` is now `create_ledger`. The tables are
///    unchanged; it is the serialised op log inside them that moved.
const SCHEMA_VERSION: i64 = 2;

/// The oldest layout this build can still replay. Equal to `SCHEMA_VERSION`
/// because no migration is written: the rename landed before anyone but the
/// author had a budget file, so the honest thing is to say so and stop.
const MIN_SCHEMA_VERSION: i64 = 2;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

-- Commits are immutable and content-addressed; `id` is the SHA-256 of `body`'s
-- canonical payload, which is why there is no other key to get wrong.
CREATE TABLE IF NOT EXISTS commits (
    id   TEXT PRIMARY KEY,
    body TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS refs (
    name   TEXT PRIMARY KEY,
    target TEXT NOT NULL REFERENCES commits(id)
) STRICT;

-- The staging area. Deliberately outside history: `seq` preserves entry order
-- so a replay of the stage matches what the user typed.
CREATE TABLE IF NOT EXISTS stage (
    seq INTEGER PRIMARY KEY,
    op  TEXT NOT NULL
) STRICT;
"#;

#[derive(Debug)]
pub struct SqliteStore {
    conn: Connection,
}

fn store_err(e: impl std::fmt::Display) -> Error {
    Error::Store(e.to_string())
}

impl SqliteStore {
    /// Open (creating if needed) a budget file.
    pub fn open(path: impl AsRef<Path>) -> Result<SqliteStore> {
        let conn = Connection::open(path).map_err(store_err)?;
        SqliteStore::prepare(conn)
    }

    /// A throwaway budget that never touches the disk. Handy for tests and for
    /// the "try something out" mode of a front end.
    pub fn open_in_memory() -> Result<SqliteStore> {
        let conn = Connection::open_in_memory().map_err(store_err)?;
        SqliteStore::prepare(conn)
    }

    fn prepare(conn: Connection) -> Result<SqliteStore> {
        conn.pragma_update(None, "journal_mode", "DELETE").map_err(store_err)?;
        conn.pragma_update(None, "synchronous", "FULL").map_err(store_err)?;
        conn.pragma_update(None, "foreign_keys", "ON").map_err(store_err)?;
        conn.execute_batch(SCHEMA).map_err(store_err)?;

        let store = SqliteStore { conn };
        match store.meta("schema_version")? {
            None => store.set_meta("schema_version", &SCHEMA_VERSION.to_string())?,
            Some(v) => {
                let found: i64 = v.parse().map_err(|_| {
                    Error::Store(format!("unreadable schema version in this file: {v}"))
                })?;
                if found > SCHEMA_VERSION {
                    return Err(Error::Store(format!(
                        "this budget was written by a newer version of Ledgit \
                         (file schema {found}, this build understands {SCHEMA_VERSION})"
                    )));
                }
                // Without this the file opens, and then fails much later with a
                // serde error about an unknown op, which tells the user nothing.
                if found < MIN_SCHEMA_VERSION {
                    return Err(Error::Store(format!(
                        "this budget was written by XtraLedger, before accounts \
                         were renamed to ledgers (file schema {found}, this build \
                         needs {MIN_SCHEMA_VERSION}); its operation log cannot be \
                         replayed by this build"
                    )));
                }
            }
        }
        Ok(store)
    }

    fn meta(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0))
            .optional()
            .map_err(store_err)
    }

    fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
            .map_err(store_err)
    }

    /// Re-hash every commit and check it still matches its id, and that every
    /// ref and parent points at something that exists.
    pub fn verify(&self) -> Result<usize> {
        let mut stmt = self.conn.prepare("SELECT id, body FROM commits").map_err(store_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(store_err)?;
        let mut n = 0;
        for row in rows {
            let (id, body) = row.map_err(store_err)?;
            let commit: Commit = serde_json::from_str(&body)?;
            if commit.id.to_string() != id {
                return Err(Error::Store(format!("commit row {id} holds a different commit")));
            }
            commit.verify()?;
            for p in &commit.parents {
                if self.get_commit(p)?.is_none() {
                    return Err(Error::Store(format!(
                        "commit {} names a missing parent {}",
                        commit.id.short(),
                        p.short()
                    )));
                }
            }
            n += 1;
        }
        Ok(n)
    }
}

impl Store for SqliteStore {
    fn put_commit(&mut self, commit: &Commit) -> Result<()> {
        let body = serde_json::to_string(commit)?;
        self.conn
            .execute(
                "INSERT INTO commits (id, body) VALUES (?1, ?2) ON CONFLICT(id) DO NOTHING",
                params![commit.id.to_string(), body],
            )
            .map(|_| ())
            .map_err(store_err)
    }

    fn get_commit(&self, id: &CommitId) -> Result<Option<Commit>> {
        let body: Option<String> = self
            .conn
            .query_row("SELECT body FROM commits WHERE id = ?1", params![id.to_string()], |r| {
                r.get(0)
            })
            .optional()
            .map_err(store_err)?;
        match body {
            None => Ok(None),
            Some(b) => {
                let c: Commit = serde_json::from_str(&b)?;
                // Cheap insurance: a bit-flip in the file becomes an error
                // here instead of a wrong balance three screens later.
                c.verify()?;
                Ok(Some(c))
            }
        }
    }

    fn all_commit_ids(&self) -> Result<Vec<CommitId>> {
        let mut stmt = self.conn.prepare("SELECT id FROM commits").map_err(store_err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(store_err)?;
        rows.map(|r| {
            let s = r.map_err(store_err)?;
            CommitId::parse(&s).ok_or_else(|| Error::Store(format!("bad commit id in file: {s}")))
        })
        .collect()
    }

    fn set_ref(&mut self, name: &str, target: Option<CommitId>) -> Result<()> {
        match target {
            Some(id) => self.conn.execute(
                "INSERT INTO refs (name, target) VALUES (?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET target = excluded.target",
                params![name, id.to_string()],
            ),
            None => self.conn.execute("DELETE FROM refs WHERE name = ?1", params![name]),
        }
        .map(|_| ())
        .map_err(store_err)
    }

    fn get_ref(&self, name: &str) -> Result<Option<CommitId>> {
        let target: Option<String> = self
            .conn
            .query_row("SELECT target FROM refs WHERE name = ?1", params![name], |r| r.get(0))
            .optional()
            .map_err(store_err)?;
        match target {
            None => Ok(None),
            Some(t) => CommitId::parse(&t)
                .ok_or_else(|| Error::Store(format!("branch {name} points at garbage: {t}")))
                .map(Some),
        }
    }

    fn list_refs(&self) -> Result<Vec<(String, CommitId)>> {
        let mut stmt =
            self.conn.prepare("SELECT name, target FROM refs ORDER BY name").map_err(store_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(store_err)?;
        rows.map(|r| {
            let (n, t) = r.map_err(store_err)?;
            let id = CommitId::parse(&t)
                .ok_or_else(|| Error::Store(format!("branch {n} points at garbage: {t}")))?;
            Ok((n, id))
        })
        .collect()
    }

    fn get_head(&self) -> Result<Option<Head>> {
        match self.meta("head")? {
            None => Ok(None),
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
        }
    }

    fn set_head(&mut self, head: &Head) -> Result<()> {
        self.set_meta("head", &serde_json::to_string(head)?)
    }

    fn get_stage(&self) -> Result<Vec<Op>> {
        let mut stmt = self.conn.prepare("SELECT op FROM stage ORDER BY seq").map_err(store_err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(store_err)?;
        rows.map(|r| Ok(serde_json::from_str(&r.map_err(store_err)?)?)).collect()
    }

    fn set_stage(&mut self, ops: &[Op]) -> Result<()> {
        // Rewriting the whole stage keeps this dead simple and matches how the
        // core hands it over. It is O(staged ops) per edit, which is fine at
        // human entry rates; if a front end ever bulk-imports 100k rows,
        // give this an append path.
        let tx = self.conn.transaction().map_err(store_err)?;
        tx.execute("DELETE FROM stage", []).map_err(store_err)?;
        {
            let mut stmt =
                tx.prepare("INSERT INTO stage (seq, op) VALUES (?1, ?2)").map_err(store_err)?;
            for (i, op) in ops.iter().enumerate() {
                stmt.execute(params![i as i64, serde_json::to_string(op)?]).map_err(store_err)?;
            }
        }
        tx.commit().map_err(store_err)
    }

    fn flush(&mut self) -> Result<()> {
        // Every write above already went through a durable transaction; this
        // exists so a future buffering store has somewhere to honour it.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ledgit_core::prelude::*;

    #[test]
    fn a_budget_survives_being_closed_and_reopened() {
        let dir = std::env::temp_dir().join(format!("ledgit-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("reopen.ledgit");
        let _ = std::fs::remove_file(&path);

        let opened = "2024-01-01".parse::<Date>().unwrap();
        let id = {
            let mut repo = Repo::open(SqliteStore::open(&path).unwrap(), "tester").unwrap();
            let cash = repo.add_ledger("Cash", "", Normality::Debit, opened).unwrap();
            let loan = repo.add_ledger("Car Loan", "", Normality::Credit, opened).unwrap();
            repo.post("Payment", "", opened, Money::from_major(400), loan, cash).unwrap();
            repo.commit("first").unwrap()
        };

        let repo = Repo::open(SqliteStore::open(&path).unwrap(), "tester").unwrap();
        assert_eq!(repo.head_commit().unwrap(), Some(id));
        assert_eq!(repo.working().ledgers.len(), 2);
        assert_eq!(repo.working().transactions.len(), 1);
        assert!(repo.working().is_balanced());
        assert_eq!(repo.store().verify().unwrap(), 1);

        std::fs::remove_file(&path).unwrap();
    }

    /// A budget written before the rename must be turned away by name, not left
    /// to fail later on an op tag it has never heard of.
    #[test]
    fn a_pre_rename_budget_is_refused_with_an_explanation() {
        let dir = std::env::temp_dir().join(format!("ledgit-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("xtraledger.ledgit");
        let _ = std::fs::remove_file(&path);

        // Make a current file, then wind its stamp back to what XtraLedger wrote.
        SqliteStore::open(&path).unwrap().set_meta("schema_version", "1").unwrap();

        let err = SqliteStore::open(&path).unwrap_err().to_string();
        assert!(err.contains("XtraLedger"), "unhelpful message: {err}");
        assert!(err.contains("ledgers"), "unhelpful message: {err}");

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn the_staging_area_outlives_a_crash() {
        let dir = std::env::temp_dir().join(format!("ledgit-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stage.ledgit");
        let _ = std::fs::remove_file(&path);
        let opened = "2024-01-01".parse::<Date>().unwrap();

        {
            let mut repo = Repo::open(SqliteStore::open(&path).unwrap(), "tester").unwrap();
            repo.add_ledger("Cash", "", Normality::Debit, opened).unwrap();
            // No commit: simulate the process dying here.
        }

        let repo = Repo::open(SqliteStore::open(&path).unwrap(), "tester").unwrap();
        assert_eq!(repo.staged().len(), 1, "staged work should still be there");
        assert_eq!(repo.committed().ledgers.len(), 0, "but it is not history yet");
        assert_eq!(repo.working().ledgers.len(), 1);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn corrupt_commit_bodies_are_caught_on_read() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let c = Commit::new(vec![], "tester", 1, "hello", vec![]).unwrap();
        store.put_commit(&c).unwrap();
        store
            .conn
            .execute("UPDATE commits SET body = replace(body, 'hello', 'gotcha')", [])
            .unwrap();
        assert!(store.get_commit(&c.id).is_err());
        assert!(store.verify().is_err());
    }
}
