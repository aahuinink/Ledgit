//! `ledgit` - the command line front end.
//!
//! It exists for two reasons: it is the fastest way to exercise the core while
//! the GUI is being built, and it is the thing you reach for when you want to
//! script an import or check a balance without opening a window.
//!
//! Every command that changes something *stages* it. Nothing reaches history
//! until `ledgit commit`, exactly as in the GUI.

mod analysis;
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
    /// Drop one staged change, by its number in `ledgit status`. Changes that
    /// depended on it are kept but flagged as broken.
    Drop { index: usize },
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
    /// Switch to a branch or commit. Anything shelved on the branch you
    /// switch to comes back into the staging area.
    Checkout {
        rev: String,
        /// Create the branch first (staged work comes along).
        #[arg(short = 'b', long)]
        create: bool,
        /// Shelve staged work on the branch you are leaving, until you return.
        #[arg(long, conflicts_with = "bring")]
        shelve: bool,
        /// Take staged work to the other branch.
        #[arg(long)]
        bring: bool,
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
    ///
    /// An amount may be a formula over your variables, and a name or
    /// description may use one as {Name}:
    ///
    ///   ledgit post "Mileage to {Home}" "200*Car_Km_Rate" --debit Travel --credit Owed
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
    /// Groups of issuers: what they cost per day, week, month or year, and
    /// when they fall due.
    #[command(subcommand)]
    Cohort(CohortCmd),
    /// Saved views: balances across time, flows per period, and a
    /// simulation of the issuers forward to a date.
    #[command(subcommand)]
    View(ViewCmd),
    /// Variables: numbers for amount formulas, text for names.
    #[command(subcommand)]
    Var(VarCmd),
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
    ///
    /// Use colons to place it in the tree: `Wedding:Tuxedo` sits under
    /// `Wedding`, which need not exist as a ledger itself.
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
    /// The ledgers as a tree, with a subtotal at every level.
    Tree {
        /// Show only this subtree, e.g. Wedding.
        path: Option<String>,
    },
    /// Move everything at or under one path to another, e.g.
    /// `ledgit ledger move Wedding Events:Wedding`. Buckets that include the
    /// subtree follow it. Only names change; no money moves.
    Move { from: String, to: String },
    /// Set a ledger's target balance and alerts, replacing what it had.
    ///
    ///   ledgit ledger goals "Car Loan" --target 0
    ///   ledgit ledger goals Chequing --below "500:Top up from savings" --above 20000
    Goals {
        ledger: String,
        /// The balance you are aiming for, as displayed.
        #[arg(long, conflicts_with = "no_target")]
        target: Option<String>,
        #[arg(long)]
        no_target: bool,
        /// Alert when the balance drops below AMOUNT[:MESSAGE]. Repeatable.
        #[arg(long)]
        below: Vec<String>,
        /// Alert when the balance goes above AMOUNT[:MESSAGE]. Repeatable.
        #[arg(long)]
        above: Vec<String>,
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
// Parsed once from the command line; boxing a variant would buy nothing.
#[allow(clippy::large_enum_variant)]
enum IssuerCmd {
    /// Create an issuer. Issuers can be paused but never deleted.
    ///
    /// Sides work exactly as they do for `ledgit post`, so a recurring paycheque
    /// keeps its tax and pension legs.
    ///
    /// Or let the amount follow a balance, with one --debit and one --credit
    /// and no amount:
    ///
    ///   ledgit issuer add "Loan interest" --debit Interest --credit "Car Loan" --apr 6.45 --every monthly:1
    ///   ledgit issuer add "Sweep" --debit Investments --credit Savings --share 5 --every monthly:28
    Add {
        name: String,
        amount: Option<String>,
        #[arg(long)]
        debit: Vec<String>,
        #[arg(long)]
        credit: Vec<String>,
        /// Charge interest at this APR (a percentage) on --of's balance,
        /// for the days since the previous occurrence.
        #[arg(long, conflicts_with_all = ["share", "amount"])]
        apr: Option<String>,
        /// Move this percentage of --of's balance each time.
        #[arg(long, conflicts_with = "amount")]
        share: Option<String>,
        /// Whose balance --apr or --share reads. Defaults to the --credit side.
        #[arg(long)]
        of: Option<String>,
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
    /// Include every ledger at or under a path - now, and any created there
    /// later. `ledgit bucket include-tree Wedding Wedding`
    IncludeTree {
        bucket: String,
        path: String,
    },
    ExcludeTree {
        bucket: String,
        path: String,
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
    /// Total several buckets at once, adding some and subtracting others.
    ///
    /// Buckets cannot contain other buckets; this combines them at read time
    /// instead. A ledger in more than one bucket is counted once, and one that
    /// lands on both sides cancels out.
    ///
    ///   ledgit bucket combine --plus Cash --minus Receivables
    Combine {
        /// A bucket to add. Repeat for several.
        #[arg(long = "plus", short = 'p', value_name = "BUCKET")]
        plus: Vec<String>,
        /// A bucket to subtract. Repeat for several.
        #[arg(long = "minus", short = 'm', value_name = "BUCKET")]
        minus: Vec<String>,
        /// Add every member's balance as shown, instead of netting assets
        /// against liabilities.
        #[arg(long)]
        sum: bool,
        /// name | balance | normality | opened
        #[arg(long, default_value = "name")]
        sort: String,
    },
}

#[derive(Subcommand, Debug)]
enum CohortCmd {
    Add {
        name: String,
        #[arg(long, default_value = "")]
        desc: String,
    },
    /// Cohorts are views, so deleting one stops no issuer.
    Remove {
        cohort: String,
    },
    Include {
        cohort: String,
        issuer: String,
    },
    Exclude {
        cohort: String,
        issuer: String,
    },
    List,
    /// Every member's rate per day, week, month and year, and the total.
    Show {
        cohort: String,
    },
    /// Every date the members fall due in a window.
    ///
    /// Leave the cohort off to see every issuer.
    Calendar {
        cohort: Option<String>,
        /// First day to show. Defaults to today.
        #[arg(long)]
        from: Option<String>,
        /// Last day to show. Defaults to a month after --from.
        #[arg(long)]
        to: Option<String>,
    },
}

/// What a view looks at. Shared by `view add` and `view edit`; on an edit,
/// only the options given replace what the view already has.
#[derive(clap::Args, Debug, Default)]
struct SpecArgs {
    /// A bucket to add to the view's total. Repeatable.
    #[arg(long = "plus", short = 'p', value_name = "BUCKET")]
    plus: Vec<String>,
    /// A bucket to subtract from the view's total. Repeatable.
    #[arg(long = "minus", short = 'm', value_name = "BUCKET")]
    minus: Vec<String>,
    /// A ledger to chart on its own line. Repeatable.
    #[arg(long, value_name = "LEDGER")]
    ledger: Vec<String>,
    /// An issuer whose flow to break down. Repeatable.
    #[arg(long, value_name = "ISSUER")]
    issuer: Vec<String>,
    /// A cohort whose members' flow to break down. Repeatable.
    #[arg(long, value_name = "COHORT")]
    cohort: Vec<String>,
    /// Only count past transactions whose name or description contains this.
    #[arg(long)]
    text: Option<String>,
    /// Report flows per day, week, month or year.
    #[arg(long)]
    per: Option<String>,
    /// How far back to look, e.g. 6m, 90d, 1y.
    #[arg(long)]
    lookback: Option<String>,
    /// How far ahead to simulate, e.g. 12m, 2y.
    #[arg(long)]
    horizon: Option<String>,
    /// Simulate only this view's issuers and cohorts ("what if these were
    /// all that happened?") instead of every running issuer.
    #[arg(long)]
    only_selected: Option<bool>,
    /// Add bucket balances as shown instead of netting assets against
    /// liabilities.
    #[arg(long)]
    sum: Option<bool>,
}

#[derive(Subcommand, Debug)]
enum VarCmd {
    /// Create or change a variable: `ledgit var set Car_Km_Rate 0.68`.
    Set {
        name: String,
        value: String,
        /// Keep the value as text even if it looks like a number.
        #[arg(long)]
        text: bool,
    },
    Delete {
        name: String,
    },
    List,
}

#[derive(Subcommand, Debug)]
enum ViewCmd {
    /// Save a view.
    ///
    ///   ledgit view add "Net worth" --plus Assets --plus Debts --horizon 2y
    ///   ledgit view add "Bills" --ledger Chequing --cohort Bills --per week
    Add {
        name: String,
        #[arg(long, default_value = "")]
        desc: String,
        #[command(flatten)]
        spec: SpecArgs,
    },
    /// Change a view. Any list option given replaces that whole list.
    Edit {
        view: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        desc: Option<String>,
        #[command(flatten)]
        spec: SpecArgs,
    },
    /// Views are readings, so deleting one changes no balance.
    Remove {
        view: String,
    },
    List,
    /// Evaluate a view: balances, flows, timeline - and optionally a chart.
    Show {
        view: String,
        /// Simulate to this date instead of the view's own horizon.
        #[arg(long)]
        until: Option<String>,
        /// Pretend today is this date.
        #[arg(long)]
        today: Option<String>,
        /// Save the balance chart. .svg or .png, by extension.
        #[arg(long, value_name = "FILE")]
        chart: Option<PathBuf>,
        /// Chart size in pixels, WIDTHxHEIGHT.
        #[arg(long, default_value = "1000x480")]
        size: String,
        /// Save every balance series as CSV.
        #[arg(long, value_name = "FILE")]
        csv: Option<PathBuf>,
        /// Also read the view as it stood at this commit (or branch), over
        /// the same window, and show what changed - e.g. how much sooner a
        /// loan reaches its target after a lump-sum payment.
        #[arg(long, value_name = "REV")]
        compare: Option<String>,
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

        Command::Drop { index } => {
            let op = repo.unstage_at(index)?;
            println!("Dropped: {}", op.summary());
            if !repo.broken().is_empty() {
                println!(
                    "{} staged change(s) no longer apply; see `ledgit status`.",
                    repo.broken().len()
                );
            }
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

        Command::Checkout { rev, create, shelve, bring } => {
            let staged = repo.staged().len();
            let leaving = repo.head().to_string();
            let restored = if create {
                repo.checkout_new(&rev)?;
                0
            } else {
                let work = match (shelve, bring) {
                    (true, _) => StagedWork::Shelve,
                    (_, true) => StagedWork::Bring,
                    _ => StagedWork::Refuse,
                };
                repo.checkout_with(&rev, work).map_err(|e| match e {
                    Error::Invalid(m) if m.contains("staged changes") => Error::Invalid(format!(
                        "{m} (--shelve keeps them on {leaving}, --bring takes them along)"
                    )),
                    e => e,
                })?
            };
            println!("Now on {}.", repo.head());
            if shelve && staged > 0 {
                println!("Shelved {staged} staged change(s) on {leaving}.");
            }
            if restored > 0 {
                println!("Put back {restored} change(s) shelved here. See `ledgit status`.");
            }
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
        Command::Cohort(cmd) => cohort_cmd(&mut repo, cmd)?,
        Command::View(cmd) => view_cmd(&mut repo, cmd)?,
        Command::Var(cmd) => match cmd {
            VarCmd::Set { name, value, text } => {
                let value = if text { VarValue::Text(value) } else { VarValue::guess(&value) };
                let op = Op::SetVariable { name, value };
                println!("Staged: {}", op.summary());
                repo.stage(op)?;
            }
            VarCmd::Delete { name } => {
                let op = Op::DeleteVariable { name };
                println!("Staged: {}", op.summary());
                repo.stage(op)?;
            }
            VarCmd::List => {
                let v = &repo.working().variables;
                if v.is_empty() {
                    println!("No variables.");
                }
                for i in 0..v.len() {
                    let kind = if v.value[i].is_number() { "number" } else { "text" };
                    println!("  {:<24} {:<7} {}", v.name[i], kind, v.value[i]);
                }
            }
        },

        Command::Post { name, amount, debit, credit, date, desc } => {
            let legs = resolve::legs(repo.working(), &debit, &credit, amount.as_deref())?;
            let when = match date {
                Some(s) => resolve::date(&s)?,
                None => Date::today_utc(),
            };
            let total = ledgit_core::model::magnitude(&legs);
            let vars = &repo.working().variables;
            let name = vars.substitute(&name).map_err(Error::Invalid)?;
            let desc = vars.substitute(&desc).map_err(Error::Invalid)?;
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
        LedgerCmd::Goals { ledger, target, no_target, below, above } => {
            let l = repo.working();
            let uid = resolve::ledger(l, &ledger)?;
            let ix = l.ledgers.ix(uid).expect("just resolved");
            let target = match (target, no_target) {
                (Some(t), _) => Some(resolve::amount(l, &t)?),
                (None, true) => None,
                (None, false) => l.ledgers.target[ix.get()],
            };
            let mut alerts = Vec::new();
            for (when, specs) in [(AlertWhen::Below, below), (AlertWhen::Above, above)] {
                for spec in specs {
                    let (level, message) = spec.split_once(':').unwrap_or((&spec, ""));
                    alerts.push(Alert {
                        when,
                        level: resolve::amount(l, level.trim())?,
                        message: message.trim().to_string(),
                    });
                }
            }
            let op = Op::SetLedgerGoals { uid, target, alerts };
            println!("Staged: {}", op.summary());
            repo.stage(op)?;
        }
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
        LedgerCmd::Tree { path } => show::tree(repo.working(), path.as_deref())?,
        LedgerCmd::Move { from, to } => {
            let n = repo.rename_subtree(&from, &to)?;
            println!("Staged: move {n} ledger(s) from {from} to {to}.");
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
        IssuerCmd::Add { name, amount, debit, credit, apr, share, of, every, start, desc } => {
            let schedule = resolve::schedule(&every)?;
            let start = match start {
                Some(s) => resolve::date(&s)?,
                None => Date::today_utc(),
            };
            if apr.is_some() || share.is_some() {
                let l = repo.working();
                let [d] = debit.as_slice() else {
                    return Err(Error::Invalid("a rate needs exactly one --debit".into()));
                };
                let [c] = credit.as_slice() else {
                    return Err(Error::Invalid("a rate needs exactly one --credit".into()));
                };
                let (d, c) = (resolve::ledger(l, d)?, resolve::ledger(l, c)?);
                let of = match of {
                    Some(o) => resolve::ledger(l, &o)?,
                    None => c,
                };
                let pct = |s: &str| Rate::parse_percent(s).map_err(Error::Invalid);
                let rule = match (apr, share) {
                    (Some(a), _) => AmountRule::Interest { of, apr: pct(&a)? },
                    (None, Some(s)) => AmountRule::ShareOfBalance { of, rate: pct(&s)? },
                    (None, None) => unreachable!("checked above"),
                };
                repo.add_rule_issuer(&name, desc, d, c, rule, schedule, start)?;
                println!(
                    "Staged issuer \"{name}\": {} {} {} from {start}.",
                    rule.describe(),
                    l_name(repo, of),
                    schedule.describe()
                );
                return Ok(());
            }
            let legs = resolve::legs(repo.working(), &debit, &credit, amount.as_deref())?;
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
        BucketCmd::IncludeTree { bucket, path } => {
            let b = resolve::bucket(repo.working(), &bucket)?;
            // Store the tree's own spelling when the path exists, so the
            // bucket reads "Wedding" however it was typed.
            let tree = LedgerTree::build(repo.working());
            let normal = ledgit_core::tree::normalize(&path);
            let path = tree.find(&normal).map_or(normal, |n| tree.nodes[n].path.clone());
            repo.stage(Op::AddSubtreeToBucket { bucket: b, path: path.clone() })?;
            let l = repo.working();
            let n = l.buckets.members[l.buckets.ix(b).expect("resolved").get()].len();
            println!("Staged: {bucket} includes everything under {path} ({n} ledger(s) now).");
        }
        BucketCmd::ExcludeTree { bucket, path } => {
            let b = resolve::bucket(repo.working(), &bucket)?;
            repo.stage(Op::RemoveSubtreeFromBucket { bucket: b, path: path.clone() })?;
            println!("Staged: {bucket} no longer includes {path}.");
        }
        BucketCmd::List => show::buckets(repo.working()),
        BucketCmd::Show { bucket, sum, sort } => {
            let uid = resolve::bucket(repo.working(), &bucket)?;
            let roll = if sum { RollUp::Sum } else { RollUp::ByNormality };
            show::bucket(repo.working(), uid, roll, show::ledger_sort(&sort)?)?;
        }
        BucketCmd::Combine { plus, minus, sum, sort } => {
            if plus.is_empty() && minus.is_empty() {
                return Err(Error::Invalid(
                    "nothing to combine - pass at least one --plus or --minus".into(),
                ));
            }
            let l = repo.working();
            let mut terms = Vec::with_capacity(plus.len() + minus.len());
            for name in &plus {
                terms.push(Term::plus(resolve::bucket(l, name)?));
            }
            for name in &minus {
                terms.push(Term::minus(resolve::bucket(l, name)?));
            }
            let roll = if sum { RollUp::Sum } else { RollUp::ByNormality };
            show::combination(l, &terms, roll, show::ledger_sort(&sort)?);
        }
    }
    Ok(())
}

fn cohort_cmd(repo: &mut Repo<SqliteStore>, cmd: CohortCmd) -> Result<()> {
    match cmd {
        CohortCmd::Add { name, desc } => {
            let uid = repo.add_cohort(&name, desc)?;
            println!("Staged cohort \"{name}\", uid {}.", uid.short());
        }
        CohortCmd::Remove { cohort } => {
            let uid = resolve::cohort(repo.working(), &cohort)?;
            repo.stage(Op::DeleteCohort { uid })?;
            println!("Staged: delete cohort {}. (Its issuers carry on.)", uid.short());
        }
        CohortCmd::Include { cohort, issuer } => {
            let l = repo.working();
            let (c, s) = (resolve::cohort(l, &cohort)?, resolve::issuer(l, &issuer)?);
            repo.stage(Op::AddToCohort { cohort: c, issuer: s })?;
            println!("Staged: add {issuer} to {cohort}.");
        }
        CohortCmd::Exclude { cohort, issuer } => {
            let l = repo.working();
            let (c, s) = (resolve::cohort(l, &cohort)?, resolve::issuer(l, &issuer)?);
            repo.stage(Op::RemoveFromCohort { cohort: c, issuer: s })?;
            println!("Staged: remove {issuer} from {cohort}.");
        }
        CohortCmd::List => analysis::cohorts(repo.working()),
        CohortCmd::Show { cohort } => {
            let uid = resolve::cohort(repo.working(), &cohort)?;
            analysis::cohort(repo.working(), uid)?;
        }
        CohortCmd::Calendar { cohort, from, to } => {
            let l = repo.working();
            let today = Date::today_utc();
            let from = from.as_deref().map(resolve::date).transpose()?.unwrap_or(today);
            let to = to.as_deref().map(resolve::date).transpose()?.unwrap_or(from.add_months(1));
            let issuers = match cohort {
                Some(c) => {
                    let ix = l.cohorts.ix(resolve::cohort(l, &c)?).expect("resolved");
                    l.cohorts.members[ix.get()].clone()
                }
                None => l.issuers.indices().collect(),
            };
            analysis::calendar(l, &issuers, from, to, today);
        }
    }
    Ok(())
}

/// Apply the given options on top of `spec`.
fn build_spec(l: &Budget, mut spec: ViewSpec, a: SpecArgs) -> Result<ViewSpec> {
    if !a.plus.is_empty() || !a.minus.is_empty() {
        spec.buckets.clear();
        for b in &a.plus {
            spec.buckets.push(Term::plus(resolve::bucket(l, b)?));
        }
        for b in &a.minus {
            spec.buckets.push(Term::minus(resolve::bucket(l, b)?));
        }
    }
    if !a.ledger.is_empty() {
        spec.ledgers = a.ledger.iter().map(|x| resolve::ledger(l, x)).collect::<Result<_>>()?;
    }
    if !a.issuer.is_empty() {
        spec.issuers = a.issuer.iter().map(|x| resolve::issuer(l, x)).collect::<Result<_>>()?;
    }
    if !a.cohort.is_empty() {
        spec.cohorts = a.cohort.iter().map(|x| resolve::cohort(l, x)).collect::<Result<_>>()?;
    }
    if let Some(t) = a.text {
        spec.transactions = if t.is_empty() { vec![] } else { vec![TxFilter::Text(t)] };
    }
    if let Some(p) = a.per {
        spec.period = resolve::period(&p)?;
    }
    if let Some(s) = a.lookback {
        spec.lookback = resolve::span(&s)?;
    }
    if let Some(s) = a.horizon {
        spec.horizon = resolve::span(&s)?;
    }
    if let Some(b) = a.only_selected {
        spec.only_selected_issuers = b;
    }
    if let Some(b) = a.sum {
        spec.roll = if b { RollUp::Sum } else { RollUp::ByNormality };
    }
    Ok(spec)
}

fn view_cmd(repo: &mut Repo<SqliteStore>, cmd: ViewCmd) -> Result<()> {
    match cmd {
        ViewCmd::Add { name, desc, spec } => {
            let spec = build_spec(repo.working(), ViewSpec::default(), spec)?;
            let summary = analysis::describe(&spec);
            let uid = repo.add_view(&name, desc, spec)?;
            println!("Staged view \"{name}\", uid {}: {summary}.", uid.short());
        }
        ViewCmd::Edit { view, name, desc, spec } => {
            let l = repo.working();
            let uid = resolve::view(l, &view)?;
            let old = l.views.spec[l.views.ix(uid).expect("resolved").get()].clone();
            let new = build_spec(l, old.clone(), spec)?;
            let spec = (new != old).then_some(new);
            repo.stage(Op::EditView { uid, name, description: desc, spec })?;
            println!("Staged edit to view {}.", uid.short());
        }
        ViewCmd::Remove { view } => {
            let uid = resolve::view(repo.working(), &view)?;
            repo.stage(Op::DeleteView { uid })?;
            println!("Staged: delete view {}. (No balances change.)", uid.short());
        }
        ViewCmd::List => analysis::views(repo.working()),
        ViewCmd::Show { view, until, today, chart, size, csv, compare } => {
            let l = repo.working();
            let v = l.views.get(l.views.ix(resolve::view(l, &view)?).expect("resolved"));
            let today =
                today.as_deref().map(resolve::date).transpose()?.unwrap_or_else(Date::today_utc);
            let until = until.as_deref().map(resolve::date).transpose()?;
            let r = analysis::evaluate(l, &v.spec, today, until);
            analysis::view(l, &v.name, &r);
            if let Some(rev) = compare {
                let id = repo.resolve(&rev)?;
                let then_budget = repo.budget_at(id)?;
                let (then, pairs) = ledgit_core::view::compare(&then_budget, &v.spec, l, &r);
                println!("\nCompared with {} ({}):", id.short(), repo.get_commit(id)?.summary());
                println!(
                    "  {:<24} {:>14} {:>14} {:>14} {:>14}  target reached then -> now",
                    "", "today then", "today now", "end then", "end now"
                );
                for (s, pair) in r.series.iter().zip(&pairs) {
                    let Some(t) = pair.map(|j| &then.series[j]) else {
                        println!("  {:<24} not in that commit", s.label);
                        continue;
                    };
                    let when = |d: Option<Date>| d.map(|d| d.to_string()).unwrap_or("-".into());
                    let target = if s.target.is_some() {
                        format!("{} -> {}", when(t.target_reached), when(s.target_reached))
                    } else {
                        String::new()
                    };
                    println!(
                        "  {:<24} {:>14} {:>14} {:>14} {:>14}  {target}",
                        s.label,
                        t.now.to_string(),
                        s.now.to_string(),
                        t.at_end.to_string(),
                        s.at_end.to_string()
                    );
                }
            }
            if let Some(path) = chart {
                let (w, h) = size
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .filter(|(w, h): &(u32, u32)| {
                        (200..=8000).contains(w) && (150..=8000).contains(h)
                    })
                    .ok_or_else(|| {
                        Error::Invalid(format!("size must look like 1000x480, not \"{size}\""))
                    })?;
                ledgit_plot::Chart::from_view(&v.name, &r)
                    .save(&path, w, h)
                    .map_err(Error::Store)?;
                println!("\nChart saved to {}.", path.display());
            }
            if let Some(path) = csv {
                analysis::csv(&r, &path)?;
                println!("Series saved to {}.", path.display());
            }
        }
    }
    Ok(())
}

/// Re-exported for `show`, which needs it to describe pending issuer work.
pub(crate) fn next_due(l: &Budget, ix: ledgit_core::id::IssuerIx) -> Option<Date> {
    issuer::next_due(l, ix)
}

fn l_name(repo: &Repo<SqliteStore>, uid: LedgerUid) -> String {
    let l = repo.working();
    l.ledgers.ix(uid).map(|ix| l.ledgers.name[ix.get()].clone()).unwrap_or_default()
}
