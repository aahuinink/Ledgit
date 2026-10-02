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
    println!("{}", freshness(repo.working(), Date::today_utc()));

    let fired = ledgit_core::goals::fired(repo.working());
    if !fired.is_empty() {
        println!("\nAlerts:");
        for f in &fired {
            let l = repo.working();
            println!(
                "  ! {:<28} {:>14}  {} {}  {}",
                l.ledgers.name[f.ledger.get()],
                amt(f.balance),
                f.alert.when,
                amt(f.alert.level),
                f.alert.message
            );
        }
    }

    let paces = ledgit_core::goals::paces(repo.working(), Date::today_utc());
    if !paces.is_empty() {
        println!("\nPaces this period:");
        for p in &paces {
            use ledgit_core::goals::PaceState;
            let l = repo.working();
            let note = match p.state() {
                PaceState::Over => format!("over by {}", amt(p.over())),
                PaceState::Met => "met".into(),
                PaceState::OffPace if p.bound == Bound::AtMost => {
                    format!("ahead of pace ({} by today)", amt(p.due_by_now))
                }
                PaceState::OffPace => format!("behind pace ({} by today)", amt(p.due_by_now)),
                PaceState::OnPace => format!("{} left", amt(p.left())),
            };
            println!(
                "  {} {:<28} {:>14} of {} a {} {}  {note}",
                if p.state() == PaceState::Over { "!" } else { " " },
                truncate(&l.ledgers.name[p.ledger.get()], 28),
                amt(p.period.flow),
                amt(p.amount),
                p.per,
                p.bound,
            );
        }
    }

    let r = repo.report()?;
    if r.is_empty() {
        println!("\nNothing staged. The budget on disk is what you see.");
        return Ok(());
    }

    println!("\nStaged changes ({}):", r.lines.len());
    for (i, line) in r.lines.iter().enumerate() {
        match r.broken.iter().find(|b| b.index == i) {
            Some(b) => println!("  {i:>3}. BROKEN {line}\n         {}", b.reason),
            None => println!("  {i:>3}. {line}"),
        }
    }
    if !r.broken.is_empty() {
        println!(
            "\n{} staged change(s) no longer apply and are left out below. \
             Nothing can be committed until they are fixed in the app or dropped with `ledgit drop <n>`.",
            r.broken.len()
        );
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
    if r.new_views + r.deleted_views > 0 {
        println!("Views: {} new, {} deleted", r.new_views, r.deleted_views);
    }

    if !r.commitments.is_empty() {
        let l = repo.working();
        println!("\nMoney spoken for:");
        for c in &r.commitments {
            let held: Vec<&str> =
                c.ledgers.iter().map(|ix| l.ledgers.name[ix.get()].as_str()).collect();
            let what = match (c.before, c.after) {
                (None, Some(m)) => format!("holds back {} on {}", amt(m), held.join(", ")),
                (Some(m), None) => format!("pays {} - no longer held", amt(m)),
                (Some(a), Some(b)) => format!("held {} -> {}", amt(a), amt(b)),
                (None, None) => continue,
            };
            println!(
                "  {:<12} {:<28} {what}",
                c.date,
                truncate(&l.issuers.name[c.issuer.get()], 28)
            );
        }
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

    if !r.paces.is_empty() {
        println!("\nPaces this breaks:");
        let l = repo.working();
        for b in &r.paces {
            println!(
                "  ! {:<28} {} a {} {}: the {} from {} goes from {} to {}",
                truncate(&l.ledgers.name[b.ledger.get()], 28),
                amt(b.amount),
                b.per,
                b.bound,
                b.per,
                b.start,
                amt(b.before),
                amt(b.after)
            );
        }
    }

    if !r.balanced {
        println!("\n!! Debits do not equal credits. Refusing to commit. This is a bug - please report it.");
    }
    Ok(())
}

/// "Fresh through 2026-09-24 · issuers through 2026-09-15": the last
/// recorded transaction, and how far the recurring payments have been run.
pub fn freshness(l: &Budget, today: Date) -> String {
    let mut s = match l.latest_transaction_date() {
        Some(d) => format!("Fresh through {d}"),
        None => "No transactions yet".to_string(),
    };
    if let Some(d) = ledgit_core::dues::caught_up_through(l) {
        s.push_str(&format!(" \u{b7} issuers through {d}"));
        let behind = ledgit_core::dues::overdue_issuers(l, today);
        if behind > 0 {
            s.push_str(&format!(" ({behind} overdue - `ledgit issuer run`)"));
        }
    }
    s
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
    let a = Availability::of(l, Date::today_utc());
    println!(
        "{:<10} {:<28} {:<7} {:>14} {:>14}  opened",
        "uid", "name", "normal", "posted", "available"
    );
    for ix in rows {
        let i = ix.get();
        // Blank where nothing is held, so the ones with money spoken for
        // stand out.
        let available = match a.held_raw(*ix).is_zero() {
            true => String::new(),
            false => amt(a.available(l, *ix)),
        };
        println!(
            "{:<10} {:<28} {:<7} {:>14} {:>14}  {}",
            l.ledgers.uid[i].short(),
            truncate(&l.ledgers.name[i], 28),
            &l.ledgers.normality[i],
            amt(l.ledgers.balance(*ix)),
            available,
            l.ledgers.opened[i],
        );
    }
    if !a.commitments.is_empty() {
        println!("\n`ledgit committed` lists what is held back.");
    }
}

/// Every payment already decided, and what it holds back where.
pub fn committed(l: &Budget, only: Option<ledgit_core::id::LedgerIx>, today: Date) {
    let a = Availability::of(l, today);
    let list: Vec<&Commitment> = match only {
        Some(ix) => a.against(l, ix),
        None => a.commitments.iter().collect(),
    };
    if list.is_empty() {
        println!("Nothing is spoken for.");
        return;
    }
    println!("{:<12} {:<28} {:>12}  holds back", "due", "payment", "amount");
    for c in list {
        let held: Vec<String> = c
            .legs
            .iter()
            .filter(|g| !g.is_debit())
            .filter_map(|g| l.ledgers.ix(g.ledger))
            .map(|ix| l.ledgers.name[ix.get()].clone())
            .collect();
        let why = match c.statement {
            Some(st) => format!("  (statement of {})", st.closed),
            None => String::new(),
        };
        let overdue = if c.date < today { "  OVERDUE - run issuers" } else { "" };
        println!(
            "{:<12} {:<28} {:>12}  {}{why}{overdue}",
            c.date,
            truncate(&l.issuers.name[c.issuer.get()], 28),
            amt(c.amount()),
            held.join(", ")
        );
    }
    if let Some(ix) = only {
        println!(
            "\n{}: posted {}, available {}",
            l.ledgers.name[ix.get()],
            amt(l.ledgers.balance(ix)),
            amt(a.available(l, ix))
        );
    }
}

/// An issuer's next occurrences, priced as they will post.
pub fn upcoming(l: &Budget, ix: ledgit_core::id::IssuerIx, count: usize) {
    let i = ix.get();
    println!("{} - {}", l.issuers.name[i], l.issuers.schedule[i].describe());
    if let Some(tx) = l.issuers.settles[i].and_then(|t| l.transactions.ix(t)) {
        println!(
            "Pays off \"{}\" from {}.",
            l.transactions.name[tx.get()],
            l.transactions.date[tx.get()]
        );
    }
    if l.issuers.paused[i] {
        println!("Paused: nothing will post.");
        return;
    }
    let mine = ledgit_core::issuer::upcoming(l, ix, count);
    if mine.is_empty() {
        println!("Nothing left to post.");
        return;
    }
    let statement = mine.iter().any(|o| o.statement.is_some());
    if statement {
        println!(
            "\n  {:<12} {:>12}  {:<12} {:>12} {:>12} {:>12} {:>12}",
            "due", "pays", "closed", "statement", "paid since", "owed", "minimum"
        );
    } else {
        println!("\n  {:<12} {:>12}", "due", "pays");
    }
    for o in &mine {
        let set = match o.set_ahead {
            Some(m) if m == o.amount() => "  (set ahead)".to_string(),
            Some(m) => format!("  (set ahead to {}, raised to the minimum)", amt(m)),
            None => String::new(),
        };
        match o.statement {
            Some(st) => println!(
                "  {:<12} {:>12}  {:<12} {:>12} {:>12} {:>12} {:>12}{set}",
                o.date,
                amt(o.amount()),
                st.closed,
                amt(st.balance),
                amt(st.paid_since),
                amt(st.owed),
                amt(st.minimum)
            ),
            None => println!("  {:<12} {:>12}{set}", o.date, amt(o.amount())),
        }
    }
    println!("\nChange one with `ledgit issuer override`. Future statements are projections.");
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
        // A rule's amount depends on a balance; show the next one, marked
        // as an estimate, and the rule itself after the state.
        let (amount, rule) = match l.issuers.rule[i] {
            Some(r) => {
                let of = l.ledgers.ix(r.of()).map(|a| l.ledgers.name[a.get()].clone());
                (
                    format!("~{}", amt(ledgit_core::issuer::estimate(l, ix))),
                    format!("  ({} {})", r.describe(), of.unwrap_or_default()),
                )
            }
            None => {
                let pays = l.issuers.settles[i]
                    .and_then(|t| l.transactions.ix(t))
                    .map(|t| format!("  (pays off \"{}\")", l.transactions.name[t.get()]))
                    .unwrap_or_default();
                (amt(ledgit_core::issuer::estimate(l, ix)), pays)
            }
        };
        println!(
            "{:<10} {:<24} {:>12} {:<22} {:<12} {}{rule}",
            l.issuers.uid[i].short(),
            truncate(&l.issuers.name[i], 24),
            amount,
            l.issuers.schedule[i].describe(),
            due,
            if l.issuers.paused[i] { "paused" } else { "active" },
        );
    }
}

/// The ledger tree, indented, with a subtotal at every level that has
/// children. A subtotal over mixed normalities is marked "net".
pub fn tree(l: &Budget, path: Option<&str>) -> Result<()> {
    let t = LedgerTree::build(l);
    let range = match path {
        Some(p) => {
            let n = t
                .find(&ledgit_core::tree::normalize(p))
                .ok_or_else(|| Error::Invalid(format!("nothing in the tree at \"{p}\"")))?;
            n..t.nodes[n].end as usize
        }
        None => 0..t.nodes.len(),
    };
    if range.is_empty() {
        println!("No ledgers.");
        return Ok(());
    }
    let base = t.nodes[range.start].depth;
    for n in range {
        let node = &t.nodes[n];
        let indent = "  ".repeat((node.depth - base) as usize);
        let label = format!("{indent}{}", node.name());
        let own = node.ledger.map(|ix| amt(l.ledgers.balance(ix))).unwrap_or_default();
        let parent = t.nodes[n].end as usize > n + 1;
        let subtotal = if parent {
            let (m, kind) = t.total(l, n);
            format!("{}{}", amt(m), if kind.is_none() { " net" } else { "" })
        } else {
            String::new()
        };
        println!("{:<40} {:>14} {:>18}", truncate(&label, 40), own, subtotal);
    }
    Ok(())
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
    let bix = l.buckets.ix(uid).expect("rolled up").get();
    let subtrees = &l.buckets.subtrees[bix];
    println!("{} ({} ledger(s))", r.name, r.lines.len());
    if !subtrees.is_empty() {
        println!("Includes everything under: {}", subtrees.join(", "));
    }
    println!("\n  {:<28} {:<7} {:>14} {:>14}", "ledger", "normal", "balance", "contributes");
    for line in &r.lines {
        let via = if l.buckets.explicit[bix].contains(&line.ledger) {
            String::new()
        } else {
            subtrees
                .iter()
                .find(|p| ledgit_core::tree::is_under(&line.name, p))
                .map(|p| format!("  (via {p})"))
                .unwrap_or_default()
        };
        println!(
            "  {:<28} {:<7} {:>14} {:>14}{via}",
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
        "{:<10} {:<12} {:<26} {:<26} {:>13} {:>15} source",
        "uid", "date", "description", "other side", "change", "balance"
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
            "{:<10} {:<12} {:<26} {:<26} {:>13} {:>15} {source}{}",
            l.transactions.uid[t].short(),
            l.transactions.date[t],
            truncate(&l.transactions.name[t], 26),
            truncate(&others.join(", "), 26),
            amt(shown),
            amt(line.balance),
            reversal_note(l, line.transaction),
        );
    }
    Ok(())
}

/// "  (reversed)" or "  (reverses 1a2b3c4d)", for a listing.
fn reversal_note(l: &Budget, t: ledgit_core::id::TxIx) -> String {
    match (l.transactions.reverses[t.get()], l.reversed_by(t)) {
        (Some(of), _) => format!("  (reverses {})", of.short()),
        (None, Some(_)) => "  (reversed)".into(),
        _ => String::new(),
    }
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
        println!("  {:<10} {:<12} {:<30} {:>13}  dr / cr", "uid", "date", "name", "amount");
        for tix in &hits.transactions {
            let i = tix.get();
            let (dr, cr) = sides(l, *tix);
            println!(
                "  {:<10} {:<12} {:<30} {:>13}  {} / {}{}",
                l.transactions.uid[i].short(),
                &l.transactions.date[i],
                truncate(&l.transactions.name[i], 30),
                amt(l.amount_of(*tix)),
                truncate(&dr, 20),
                truncate(&cr, 20),
                reversal_note(l, *tix),
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

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}~")
    }
}
