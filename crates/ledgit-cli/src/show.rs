//! Printing. Kept apart from the command dispatch so the formatting decisions
//! live in one place and the commands stay readable.

use ledgit_core::prelude::*;
use ledgit_sqlite::SqliteStore;

/// Money with thousands separators, so five-figure balances stay scannable.
pub fn amt(m: Money) -> String {
    let neg = m.cents() < 0;
    let abs = m.cents().unsigned_abs();
    let (whole, cents) = (abs / 100, abs % 100);
    let digits = whole.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{}{grouped}.{cents:02}", if neg { "-" } else { "" })
}

pub fn ledger_sort(s: &str) -> Result<LedgerSort> {
    match s.to_lowercase().as_str() {
        "name" => Ok(LedgerSort::Name),
        "balance" => Ok(LedgerSort::Balance),
        "normality" => Ok(LedgerSort::Normality),
        "opened" | "date" => Ok(LedgerSort::Opened),
        other => Err(Error::Invalid(format!(
            "sort must be name, balance, normality or opened, not \"{other}\""
        ))),
    }
}

pub fn status(repo: &Repo<SqliteStore>) -> Result<()> {
    println!("On {}", repo.head());
    match repo.head_commit()? {
        Some(id) => {
            let c = repo.get_commit(id)?;
            println!("Last commit {} - {}", id.short(), c.summary());
        }
        None => println!("No commits yet."),
    }

    let r = repo.report()?;
    if r.is_empty() {
        println!("\nNothing staged. The budget on disk is what you see.");
        return Ok(());
    }

    println!("\nStaged changes ({}):", r.lines.len());
    for (i, line) in r.lines.iter().enumerate() {
        println!("  {i:>3}. {line}");
    }

    println!(
        "\nTransactions: {} manual ({}), {} from issuers ({})",
        r.manual_transactions,
        amt(r.total_manual),
        r.issuer_transactions,
        amt(r.total_issued)
    );
    if r.new_ledgers + r.new_issuers + r.new_buckets + r.deleted_buckets > 0 {
        println!(
            "New: {} ledger(s), {} issuer(s), {} bucket(s); {} bucket(s) deleted",
            r.new_ledgers, r.new_issuers, r.new_buckets, r.deleted_buckets
        );
    }

    if !r.ledger_deltas.is_empty() {
        println!("\nLedgers affected:");
        println!("  {:<28} {:>14} {:>14} {:>14}", "ledger", "before", "after", "change");
        for d in &r.ledger_deltas {
            println!(
                "  {:<28} {:>14} {:>14} {:>14}{}",
                truncate(&d.name, 28),
                amt(d.before),
                amt(d.after),
                amt(d.change()),
                if d.is_new { "  (new)" } else { "" }
            );
        }
    }

    if !r.bucket_effects.is_empty() {
        println!("\nBuckets that may be affected:");
        println!("  {:<28} {:>14} {:>14} {:>14}", "bucket", "before", "after", "change");
        for b in &r.bucket_effects {
            println!(
                "  {:<28} {:>14} {:>14} {:>14}{}",
                truncate(&b.name, 28),
                amt(b.before),
                amt(b.after),
                amt(b.change()),
                if b.membership_changed { "  (membership changed)" } else { "" }
            );
        }
    }

    if !r.balanced {
        println!("\n!! Debits do not equal credits. Refusing to commit. This is a bug - please report it.");
    }
    Ok(())
}

pub fn log(repo: &Repo<SqliteStore>, limit: usize) -> Result<()> {
    let commits = repo.log(Some(limit))?;
    if commits.is_empty() {
        println!("No commits yet.");
        return Ok(());
    }
    let branches = repo.branches()?;
    for c in &commits {
        let heads: Vec<&str> =
            branches.iter().filter(|(_, id)| *id == c.id).map(|(n, _)| n.as_str()).collect();
        let decoration =
            if heads.is_empty() { String::new() } else { format!("  ({})", heads.join(", ")) };
        println!("{}  {:<50}{decoration}", c.id.short(), truncate(c.summary(), 50));
        println!("          {} op(s) by {}", c.ops.len(), c.author);
    }
    Ok(())
}

pub fn commit(repo: &Repo<SqliteStore>, rev: &str) -> Result<()> {
    let id = repo.resolve(rev)?;
    let c = repo.get_commit(id)?;
    println!("commit  {}", c.id);
    println!("author  {}", c.author);
    println!(
        "parents {}",
        if c.parents.is_empty() {
            "(root)".to_string()
        } else {
            c.parents.iter().map(|p| p.short()).collect::<Vec<_>>().join(", ")
        }
    );
    println!("\n    {}\n", c.message);
    for (i, op) in c.ops.iter().enumerate() {
        println!("  {i:>3}. {}", op.summary());
    }
    Ok(())
}

pub fn branches(repo: &Repo<SqliteStore>) -> Result<()> {
    let here = repo.head().branch_name();
    let list = repo.branches()?;
    if list.is_empty() {
        println!("No branches yet (make a commit first).");
        return Ok(());
    }
    for (name, id) in list {
        let marker = if Some(name.as_str()) == here { "*" } else { " " };
        println!("{marker} {:<24} {}", name, id.short());
    }
    if here.is_none() {
        println!("\nHEAD is detached: {}", repo.head());
    }
    Ok(())
}

pub fn ledgers(l: &Budget, rows: &[ledgit_core::id::LedgerIx]) {
    if rows.is_empty() {
        println!("No ledgers.");
        return;
    }
    println!("{:<10} {:<28} {:<7} {:>14}  opened", "uid", "name", "normal", "balance");
    for ix in rows {
        let i = ix.get();
        println!(
            "{:<10} {:<28} {:<7} {:>14}  {}",
            l.ledgers.uid[i].short(),
            truncate(&l.ledgers.name[i], 28),
            &l.ledgers.normality[i],
            amt(l.ledgers.balance(*ix)),
            l.ledgers.opened[i],
        );
    }
}

pub fn issuers(l: &Budget) {
    if l.issuers.is_empty() {
        println!("No issuers.");
        return;
    }
    println!(
        "{:<10} {:<24} {:>12} {:<22} {:<12} state",
        "uid", "name", "amount", "schedule", "next due"
    );
    for ix in l.issuers.indices() {
        let i = ix.get();
        let due = match crate::next_due(l, ix) {
            Some(d) if !l.issuers.paused[i] => d.to_string(),
            _ => "-".to_string(),
        };
        println!(
            "{:<10} {:<24} {:>12} {:<22} {:<12} {}",
            l.issuers.uid[i].short(),
            truncate(&l.issuers.name[i], 24),
            amt(l.issuers.amount(ix)),
            l.issuers.schedule[i].describe(),
            due,
            if l.issuers.paused[i] { "paused" } else { "active" },
        );
    }
}

pub fn buckets(l: &Budget) {
    let live: Vec<_> = l.buckets.live().collect();
    if live.is_empty() {
        println!("No buckets.");
        return;
    }
    println!("{:<10} {:<28} {:>8} {:>16}", "uid", "name", "ledgers", "net");
    for ix in live {
        let i = ix.get();
        let uid = l.buckets.uid[i];
        let total = roll_up(l, uid, RollUp::ByNormality, LedgerSort::Name, Order::Asc)
            .map(|r| r.total)
            .unwrap_or(Money::ZERO);
        println!(
            "{:<10} {:<28} {:>8} {:>16}",
            uid.short(),
            truncate(&l.buckets.name[i], 28),
            l.buckets.members[i].len(),
            amt(total)
        );
    }
}

pub fn bucket(l: &Budget, uid: BucketUid, roll: RollUp, sort: LedgerSort) -> Result<()> {
    let r = roll_up(l, uid, roll, sort, Order::Asc)
        .ok_or_else(|| Error::Invalid("no such bucket".into()))?;
    println!("{} ({} ledger(s))\n", r.name, r.lines.len());
    println!("  {:<28} {:<7} {:>14} {:>14}", "ledger", "normal", "balance", "contributes");
    for line in &r.lines {
        println!(
            "  {:<28} {:<7} {:>14} {:>14}",
            truncate(&line.name, 28),
            &line.normality,
            amt(line.balance),
            amt(line.contribution)
        );
    }
    let label = match roll {
        RollUp::ByNormality => "net (assets - liabilities)",
        RollUp::Sum => "sum of balances",
    };
    println!("\n  {:<50} {:>14}", label, amt(r.total));
    Ok(())
}

pub fn register(l: &Budget, ledger: LedgerUid, limit: Option<usize>) -> Result<()> {
    let ix = l.ledgers.ix(ledger).ok_or_else(|| Error::Invalid("no such ledger".into()))?;
    // Qualified: this module has its own `register`, which prints one.
    let rows = ledgit_core::query::register(l, ix);
    let rows = match limit {
        Some(n) if rows.len() > n => &rows[rows.len() - n..],
        _ => &rows[..],
    };
    println!("{} ({}-normal)\n", l.ledgers.name[ix.get()], l.ledgers.normality[ix.get()]);
    if rows.is_empty() {
        println!("No postings yet.");
        return Ok(());
    }
    println!(
        "{:<12} {:<26} {:<26} {:>13} {:>15} source",
        "date", "description", "other side", "change", "balance"
    );
    for line in rows {
        let t = line.transaction.get();
        // The register shows this ledger's share of the entry, which for a
        // split is not the size of the entry.
        let shown = l.ledgers.normality[ix.get()].present(line.change);
        let others: Vec<String> = l
            .other_side(line.transaction, ix)
            .iter()
            .map(|a| l.ledgers.name[a.get()].clone())
            .collect();
        let source = match l.transactions.parent[t] {
            Parent::Manual => "manual".to_string(),
            Parent::Issuer(u) => format!(
                "issuer {}",
                l.issuers
                    .ix(u)
                    .map(|j| l.issuers.name[j.get()].clone())
                    .unwrap_or_else(|| u.short())
            ),
        };
        println!(
            "{:<12} {:<26} {:<26} {:>13} {:>15} {source}",
            l.transactions.date[t],
            truncate(&l.transactions.name[t], 26),
            truncate(&others.join(", "), 26),
            amt(shown),
            amt(line.balance),
        );
    }
    Ok(())
}

pub fn search(l: &Budget, needle: &str) {
    let hits = ledgit_core::query::search(l, needle, 20);
    let mut found = false;

    if !hits.ledgers.is_empty() {
        found = true;
        println!("Ledgers:");
        ledgers(l, &hits.ledgers);
        println!();
    }
    if !hits.transactions.is_empty() {
        found = true;
        println!("Transactions:");
        println!("  {:<12} {:<30} {:>13}  dr / cr", "date", "name", "amount");
        for tix in &hits.transactions {
            let i = tix.get();
            let (dr, cr) = sides(l, *tix);
            println!(
                "  {:<12} {:<30} {:>13}  {} / {}",
                &l.transactions.date[i],
                truncate(&l.transactions.name[i], 30),
                amt(l.amount_of(*tix)),
                truncate(&dr, 20),
                truncate(&cr, 20),
            );
        }
        println!();
    }
    if !hits.issuers.is_empty() {
        found = true;
        println!("Issuers:");
        for ix in &hits.issuers {
            println!("  {:<10} {}", l.issuers.uid[ix.get()].short(), l.issuers.name[ix.get()]);
        }
        println!();
    }
    if !hits.buckets.is_empty() {
        found = true;
        println!("Buckets:");
        for ix in &hits.buckets {
            println!("  {:<10} {}", l.buckets.uid[ix.get()].short(), l.buckets.name[ix.get()]);
        }
        println!();
    }
    if !found {
        println!("Nothing matched \"{needle}\".");
    }
}

/// The two halves of an entry as text: what it debited, what it credited.
/// A split shows every ledger, comma-separated, so nothing is hidden behind
/// the word "split".
pub fn sides(l: &Budget, tix: ledgit_core::id::TxIx) -> (String, String) {
    let mut debits = Vec::new();
    let mut credits = Vec::new();
    for p in l.transactions.leg_range(tix) {
        let name = l.ledgers.name[l.postings.ledger[p].get()].clone();
        if l.postings.amount[p].cents() > 0 {
            debits.push(name);
        } else {
            credits.push(name);
        }
    }
    (debits.join(", "), credits.join(", "))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}~")
    }
}
