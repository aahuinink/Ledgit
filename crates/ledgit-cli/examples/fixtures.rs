//! Writes the sample budgets used to check the app by hand - the same ones
//! the tutorial builds (`ledgit_core::demo`).
//!
//! ```sh
//! cargo run -p ledgit-cli --example fixtures            # into ./fixtures
//! cargo run -p ledgit-cli --example fixtures -- C:\tmp  # somewhere else
//! ```
//!
//! Every date is relative to the day it runs, so "today" on the chart, the
//! overdue marks on the calendar and the issuers that are behind all line up
//! with the clock. Re-run it rather than keeping old copies. Existing files of
//! the same name are replaced.
//!
//! Which file exercises what is in docs/ROADMAP.md.

use ledgit_core::demo;
use ledgit_core::prelude::*;
use ledgit_sqlite::SqliteStore;
use std::path::{Path, PathBuf};

type R<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> R {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "fixtures".into()));
    std::fs::create_dir_all(&dir)?;
    let today = Date::today_utc();

    for name in demo::NAMES {
        let file = format!("{name}.ledgit");
        let path = dir.join(&file);
        // Built in memory, then written in one transaction rather than one
        // durable write per change.
        let mut repo = Repo::init(MemStore::new(), "fixtures")?;
        demo::build(name, &mut repo, today)?;
        fresh(&path)?.import(repo.store())?;
        println!(
            "{:<18} {:>5} ledgers {:>6} transactions {:>4} issuers {:>4} staged {:>3} branches",
            file,
            repo.working().ledgers.len(),
            repo.working().transactions.len(),
            repo.working().issuers.len(),
            repo.staged().len(),
            repo.branches()?.len(),
        );
    }
    println!("written to {}", dir.display());
    Ok(())
}

fn fresh(path: &Path) -> R<SqliteStore> {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
    Ok(SqliteStore::open(path)?)
}
