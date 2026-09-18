//! `ledgit` - the command line front end.
//!
//! It exists for two reasons: it is the fastest way to exercise the core while
//! the GUI is being built, and it is the thing you reach for when you want to
//! script an import or check a balance without opening a window.
//!
//! Every command that changes something *stages* it. Nothing reaches history
//! until `ledgit commit`, exactly as in the GUI.

mod resolve;
mod show;

use clap::{Parser, Subcommand};
use ledgit_core::issuer;
use ledgit_core::prelude::*;
use ledgit_sqlite::SqliteStore;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(name = "ledgit", version, about = "A version-controlled double-entry budget")]
struct Cli {
    /// Budget file to work on. Defaults to $LEDGIT_BUDGET, else ./budget.ledgit
    #[arg(long, short = 'f', global = true)]
    file: Option<PathBuf>,

    /// Name recorded as the author of commits.
    #[arg(long, global = true, env = "LEDGIT_AUTHOR", default_value = "me")]
    author: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create a new budget file.
    Init,
    /// Show the staging area and what committing would do.
    Status,
    /// Turn the staging area into a commit.
    Commit {
        #[arg(short, long)]
        message: String,
    },
    /// Discard everything staged.
    Reset,
    /// Show history, newest first.
    Log {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Show one commit in full.
    Show { rev: String },
    /// List branches, or create one.
    Branch {
        name: Option<String>,
        /// Create the branch here instead of at HEAD.
        #[arg(long)]
        at: Option<String>,
        /// Delete the named branch.
        #[arg(long)]
        delete: bool,
    },
    /// Switch to a branch or commit.
    Checkout {
        rev: String,
        /// Create the branch first.
        #[arg(short = 'b', long)]
        create: bool,
    },
    /// Stage the reversal of a commit.
    Revert { rev: String },
    /// Replay a branch onto another commit.
    Rebase {
        branch: String,
        #[arg(long)]
        onto: String,
    },
    /// Check every commit in the file still hashes to its own id.
    Verify,
    /// Ledgers.
    #[command(subcommand)]
    Ledger(LedgerCmd),
    /// Post a transaction. Two sides, or as many as you like.
    ///
    /// Each side is LEDGER or LEDGER:AMOUNT. One side may leave its amount
    /// off and take the remainder:
    ///
    ///   ledgit post "Groceries" 42.50 --debit Groceries --credit Visa
    ///   ledgit post "Paycheque" --debit Chequing:1800 --debit Tax:600 --credit "Gross pay"
    Post {
        name: String,
        /// Amount for the simple two-sided form, e.g. 42.50
        amount: Option<String>,
        /// Ledger to debit (the one receiving value). Repeatable.
        #[arg(long)]
        debit: Vec<String>,
        /// Ledger to credit (the one giving value). Repeatable.
        #[arg(long)]
        credit: Vec<String>,
        #[arg(long)]
        date: Option<String>,
        #[arg(long, default_value = "")]
        desc: String,
    },
    /// Recurring transactions.
    #[command(subcommand)]
    Issuer(IssuerCmd),
    /// Groups of ledgers.
    #[command(subcommand)]
    Bucket(BucketCmd),
    /// Every posting against one ledger, with a running balance.
    Register {
        ledger: String,
        #[arg(short = 'n', long)]
        limit: Option<usize>,
    },
    /// Search ledgers, transactions, issuers and buckets at once.
    Search { text: String },
}

#[derive(Subcommand, Debug)]
enum LedgerCmd {
    /// Open a ledger. Ledgers can never be deleted.
    Add {
        name: String,
        /// debit (assets, expenses) or credit (liabilities, income, equity)
        #[arg(long)]
        normality: String,
        #[arg(long, default_value = "")]
        desc: String,
        #[arg(long)]
        opened: Option<String>,
    },
    /// Rename a ledger or change its description.
    Edit {
        ledger: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        desc: Option<String>,
    },
    List {
        /// Only credit- or debit-normal ledgers.
        #[arg(long)]
        normality: Option<String>,
        /// name | balance | normality | opened
        #[arg(long, default_value = "name")]
        sort: String,
    },
}

#[derive(Subcommand, Debug)]
enum IssuerCmd {
    /// Create an issuer. Issuers can be paused but never deleted.
    ///
    /// Sides work exactly as they do for `ledgit post`, so a recurring paycheque
    /// keeps its tax and pension legs.
    Add {
        name: String,
        amount: Option<String>,
        #[arg(long)]
        debit: Vec<String>,
        #[arg(long)]
        credit: Vec<String>,
        /// 14d, weekly, biweekly, monthly, monthly:15, quarterly:1, yearly, once
        #[arg(long)]
        every: String,
        #[arg(long)]
        start: Option<String>,
        #[arg(long, default_value = "")]
        desc: String,
    },
    List,
    Pause {
        issuer: String,
    },
    Resume {
        issuer: String,
    },
    /// Stage every transaction the issuers owe up to a date.
    Run {
        #[arg(long)]
        through: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum BucketCmd {
    Add {
        name: String,
        #[arg(long, default_value = "")]
        desc: String,
    },
    /// Buckets are views, so deleting one moves no money.
    Remove {
        bucket: String,
    },
    Include {
        bucket: String,
        ledger: String,
    },
    Exclude {
        bucket: String,
        ledger: String,
    },
    List,
    /// Total a bucket and break it down by ledger.
    Show {
        bucket: String,
        /// Add every member's balance as shown, instead of netting assets
        /// against liabilities. Use for buckets where everything points the
        /// same way, e.g. total spending.
        #[arg(long)]
        sum: bool,
        /// name | balance | normality | opened
        #[arg(long, default_value = "name")]
        sort: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ledgit: {e}");
            ExitCode::FAILURE
        }
    }
}

fn budget_path(cli: &Cli) -> PathBuf {
    cli.file
        .clone()
        .or_else(|| std::env::var_os("LEDGIT_BUDGET").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("budget.ledgit"))
}

fn run(cli: Cli) -> Result<()> {
    let path = budget_path(&cli);

    if matches!(cli.command, Command::Init) {
        if path.exists() {
            return Err(Error::Store(format!("{} already exists", path.display())));
        }
        Repo::init(SqliteStore::open(&path)?, &cli.author)?;
        println!("Initialised an empty budget in {}", path.display());
        return Ok(());
    }

    if !path.exists() {
        return Err(Error::Store(format!(
            "no budget at {} - run `ledgit init` or pass --file",
            path.display()
        )));
    }
    let mut repo = Repo::open(SqliteStore::open(&path)?, &cli.author)?;

    match cli.command {
        Command::Init => unreachable!("handled above"),

        Command::Status => show::status(&repo)?,

        Command::Commit { message } => {
            let report = repo.report()?;
            let id = repo.commit(message)?;
            println!("[{} {}] {} change(s) committed", repo.head(), id.short(), report.lines.len());
        }

        Command::Reset => {
            let n = repo.staged().len();
            repo.clear_stage()?;
            println!("Discarded {n} staged change(s).");
        }

        Command::Log { limit } => show::log(&repo, limit)?,

        Command::Show { rev } => show::commit(&repo, &rev)?,

        Command::Branch { name, at, delete } => match (name, delete) {
            (Some(n), true) => {
                repo.delete_branch(&n)?;
                println!("Deleted branch {n}.");
            }
            (Some(n), false) => {
                let id = repo.branch(&n, at.as_deref())?;
                println!("Created branch {n} at {}.", id.short());
            }
            (None, _) => show::branches(&repo)?,
        },

        Command::Checkout { rev, create } => {
            if create {
                repo.checkout_new(&rev)?;
            } else {
                repo.checkout(&rev)?;
            }
            println!("Now on {}.", repo.head());
        }

        Command::Revert { rev } => {
            let notes = repo.revert(&rev)?;
            for n in &notes {
                println!("note: {n}");
            }
            println!(
                "Staged {} reversing change(s). Review with `ledgit status`.",
                repo.staged().len()
            );
        }

        Command::Rebase { branch, onto } => {
            let n = repo.rebase(&branch, &onto)?;
            match n {
                0 => println!("Nothing to replay; {branch} is already up to date."),
                n => println!("Replayed {n} commit(s) of {branch} onto {onto}."),
            }
        }

        Command::Verify => {
            let n = repo.store().verify()?;
            println!("{n} commit(s) verified; the file is intact.");
        }

        Command::Ledger(cmd) => ledger_cmd(&mut repo, cmd)?,
        Command::Issuer(cmd) => issuer_cmd(&mut repo, cmd)?,
        Command::Bucket(cmd) => bucket_cmd(&mut repo, cmd)?,

        Command::Post { name, amount, debit, credit, date, desc } => {
            let legs = resolve::legs(repo.working(), &debit, &credit, amount.as_deref())?;
            let when = match date {
                Some(s) => resolve::date(&s)?,
                None => Date::today_utc(),
            };
            let total = ledgit_core::model::magnitude(&legs);
            repo.post_split(&name, desc, when, legs.clone())?;
            println!("Staged: {total} on {when}, {} sides.", legs.len());
            for leg in &legs {
                let ix = repo.working().ledgers.ix(leg.ledger).expect("just staged");
                println!(
                    "  {:<6} {:<28} {}",
                    if leg.is_debit() { "debit" } else { "credit" },
                    repo.working().ledgers.name[ix.get()],
                    leg.amount.abs(),
                );
            }
        }

        Command::Register { ledger, limit } => {
            let uid = resolve::ledger(repo.working(), &ledger)?;
            show::register(repo.working(), uid, limit)?;
        }

        Command::Search { text } => show::search(repo.working(), &text),
    }
    Ok(())
}

fn ledger_cmd(repo: &mut Repo<SqliteStore>, cmd: LedgerCmd) -> Result<()> {
    match cmd {
        LedgerCmd::Add { name, normality, desc, opened } => {
            let n = resolve::normality(&normality)?;
            let opened = match opened {
                Some(s) => resolve::date(&s)?,
                None => Date::today_utc(),
            };
            let uid = repo.add_ledger(&name, desc, n, opened)?;
            println!("Staged ledger \"{name}\" ({n}-normal), uid {}.", uid.short());
        }
        LedgerCmd::Edit { ledger, name, desc } => {
            let uid = resolve::ledger(repo.working(), &ledger)?;
            repo.stage(Op::EditLedger { uid, name, description: desc })?;
            println!("Staged edit to ledger {}.", uid.short());
        }
        LedgerCmd::List { normality, sort } => {
            let mut q = LedgerQuery::new().sort_by(show::ledger_sort(&sort)?, Order::Asc);
            if let Some(n) = normality {
                q = q.filter(LedgerFilter::Normality(resolve::normality(&n)?));
            }
            show::ledgers(repo.working(), &q.run(repo.working()));
        }
    }
    Ok(())
}

fn issuer_cmd(repo: &mut Repo<SqliteStore>, cmd: IssuerCmd) -> Result<()> {
    match cmd {
        IssuerCmd::Add { name, amount, debit, credit, every, start, desc } => {
            let legs = resolve::legs(repo.working(), &debit, &credit, amount.as_deref())?;
            let schedule = resolve::schedule(&every)?;
            let start = match start {
                Some(s) => resolve::date(&s)?,
                None => Date::today_utc(),
            };
            let total = ledgit_core::model::magnitude(&legs);
            repo.add_issuer_split(&name, desc, legs, schedule, start)?;
            println!("Staged issuer \"{name}\": {total} {} from {start}.", schedule.describe());
        }
        IssuerCmd::List => show::issuers(repo.working()),
        IssuerCmd::Pause { issuer } => {
            let uid = resolve::issuer(repo.working(), &issuer)?;
            repo.stage(Op::SetIssuerPaused { uid, paused: true })?;
            println!("Staged: pause issuer {}.", uid.short());
        }
        IssuerCmd::Resume { issuer } => {
            let uid = resolve::issuer(repo.working(), &issuer)?;
            repo.stage(Op::SetIssuerPaused { uid, paused: false })?;
            println!("Staged: resume issuer {}.", uid.short());
        }
        IssuerCmd::Run { through } => {
            let through = match through {
                Some(s) => resolve::date(&s)?,
                None => Date::today_utc(),
            };
            let runs = repo.run_issuers(through)?;
            if runs.is_empty() {
                println!("No issuer owes anything through {through}.");
                return Ok(());
            }
            let l = repo.working();
            for r in &runs {
                let i = r.issuer.get();
                println!(
                    "{:<24} {:>3} occurrence(s)  {} .. {}",
                    l.issuers.name[i],
                    r.dates.len(),
                    r.dates.first().expect("non-empty"),
                    r.dates.last().expect("non-empty"),
                );
            }
            println!("\nStaged. Review with `ledgit status`, then `ledgit commit`.");
        }
    }
    Ok(())
}

fn bucket_cmd(repo: &mut Repo<SqliteStore>, cmd: BucketCmd) -> Result<()> {
    match cmd {
        BucketCmd::Add { name, desc } => {
            let uid = repo.add_bucket(&name, desc)?;
            println!("Staged bucket \"{name}\", uid {}.", uid.short());
        }
        BucketCmd::Remove { bucket } => {
            let uid = resolve::bucket(repo.working(), &bucket)?;
            repo.stage(Op::DeleteBucket { uid })?;
            println!("Staged: delete bucket {}. (No balances change.)", uid.short());
        }
        BucketCmd::Include { bucket, ledger } => {
            let l = repo.working();
            let (b, a) = (resolve::bucket(l, &bucket)?, resolve::ledger(l, &ledger)?);
            repo.stage(Op::AddToBucket { bucket: b, ledger: a })?;
            println!("Staged: add {ledger} to {bucket}.");
        }
        BucketCmd::Exclude { bucket, ledger } => {
            let l = repo.working();
            let (b, a) = (resolve::bucket(l, &bucket)?, resolve::ledger(l, &ledger)?);
            repo.stage(Op::RemoveFromBucket { bucket: b, ledger: a })?;
            println!("Staged: remove {ledger} from {bucket}.");
        }
        BucketCmd::List => show::buckets(repo.working()),
        BucketCmd::Show { bucket, sum, sort } => {
            let uid = resolve::bucket(repo.working(), &bucket)?;
            let roll = if sum { RollUp::Sum } else { RollUp::ByNormality };
            show::bucket(repo.working(), uid, roll, show::ledger_sort(&sort)?)?;
        }
    }
    Ok(())
}

/// Re-exported for `show`, which needs it to describe pending issuer work.
pub(crate) fn next_due(l: &Budget, ix: ledgit_core::id::IssuerIx) -> Option<Date> {
    issuer::next_due(l, ix)
}
