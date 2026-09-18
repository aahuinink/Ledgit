//! `Repo` is the public API: the thing a GUI, a CLI, or a test drives.
//!
//! It owns three pieces of state and keeps them consistent:
//!
//! * `base`    - the budget as of HEAD, folded from the commit DAG;
//! * `stage`   - ops entered since, not yet history;
//! * `working` - `base` with `stage` folded in; what the UI displays.
//!
//! Staging an op applies it to `working` immediately, so an invalid entry is
//! rejected at the moment it is made rather than at commit time.

use crate::commit::{validate_branch_name, Commit, CommitId, Head};
use crate::date::Date;
use crate::error::{Error, Result};
use crate::id::{BucketUid, IssuerUid, LedgerUid, TxUid};
use crate::issuer::{self, IssuerRun};
use crate::model::{simple_legs, Leg, Normality, Parent, Schedule};
use crate::money::Money;
use crate::op::Op;
use crate::report::{self, ChangeReport};
use crate::state::Budget;
use crate::store::{Store, DEFAULT_BRANCH};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Repo<S: Store> {
    store: S,
    author: String,
    head: Head,
    base: Budget,
    stage: Vec<Op>,
    working: Budget,
}

impl<S: Store> std::fmt::Debug for Repo<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repo")
            .field("author", &self.author)
            .field("head", &self.head)
            .field("staged", &self.stage.len())
            .field("ledgers", &self.working.ledgers.len())
            .field("transactions", &self.working.transactions.len())
            .finish_non_exhaustive()
    }
}

impl<S: Store> Repo<S> {
    /// Create a fresh budget on an empty store.
    pub fn init(mut store: S, author: impl Into<String>) -> Result<Repo<S>> {
        if store.get_head()?.is_some() {
            return Err(Error::Store("this store already holds a budget".into()));
        }
        let head = Head::Branch { name: DEFAULT_BRANCH.to_string() };
        store.set_head(&head)?;
        store.flush()?;
        Ok(Repo {
            store,
            author: author.into(),
            head,
            base: Budget::new(),
            stage: Vec::new(),
            working: Budget::new(),
        })
    }

    /// Open an existing budget, or initialise one if the store is empty.
    pub fn open(store: S, author: impl Into<String>) -> Result<Repo<S>> {
        let author = author.into();
        match store.get_head()? {
            None => Repo::init(store, author),
            Some(head) => {
                let stage = store.get_stage()?;
                let mut r = Repo {
                    store,
                    author,
                    head,
                    base: Budget::new(),
                    stage,
                    working: Budget::new(),
                };
                r.reload()?;
                Ok(r)
            }
        }
    }

    /// Re-fold the budget from the DAG. Called after anything that moves HEAD.
    fn reload(&mut self) -> Result<()> {
        self.base = match self.head_commit()? {
            Some(id) => self.budget_at(id)?,
            None => Budget::new(),
        };
        self.working = self.base.clone();
        // A staged op can become invalid after a checkout (it may reference an
        // ledger that does not exist on this branch). Keep what still applies
        // and report the rest rather than silently dropping work.
        let mut kept = Vec::with_capacity(self.stage.len());
        let mut dropped = Vec::new();
        for op in std::mem::take(&mut self.stage) {
            match self.working.apply(&op) {
                Ok(()) => kept.push(op),
                Err(e) => dropped.push((op, e)),
            }
        }
        self.stage = kept;
        if !dropped.is_empty() {
            self.store.set_stage(&self.stage)?;
            self.store.flush()?;
            let first = &dropped[0];
            return Err(Error::History(format!(
                "{} staged change(s) do not apply here and were dropped; first: {} ({})",
                dropped.len(),
                first.0.summary(),
                first.1
            )));
        }
        Ok(())
    }

    // ------------------------------------------------------------- reading

    /// The budget as the user sees it: committed history plus staged edits.
    pub fn working(&self) -> &Budget {
        &self.working
    }

    /// The budget as of the last commit, with no staged edits.
    pub fn committed(&self) -> &Budget {
        &self.base
    }

    pub fn head(&self) -> &Head {
        &self.head
    }

    pub fn author(&self) -> &str {
        &self.author
    }

    pub fn staged(&self) -> &[Op] {
        &self.stage
    }

    pub fn has_staged_changes(&self) -> bool {
        !self.stage.is_empty()
    }

    /// The commit HEAD points at, if the branch has any commits yet.
    pub fn head_commit(&self) -> Result<Option<CommitId>> {
        match &self.head {
            Head::Detached { at } => Ok(Some(*at)),
            Head::Branch { name } => self.store.get_ref(name),
        }
    }

    pub fn get_commit(&self, id: CommitId) -> Result<Commit> {
        self.store.get_commit(&id)?.ok_or_else(|| Error::NoSuchRef(id.short()))
    }

    /// Fold every ancestor of `id` into a budget.
    ///
    /// Ancestors are applied in topological order, oldest first, so a merge
    /// commit's two sides both land before anything that depends on them.
    pub fn budget_at(&self, id: CommitId) -> Result<Budget> {
        let mut budget = Budget::new();
        for cid in self.topo_order(id)? {
            let c = self.get_commit(cid)?;
            for op in &c.ops {
                budget.apply(op)?;
            }
        }
        Ok(budget)
    }

    /// Ancestors of `id` including itself, parents before children.
    fn topo_order(&self, id: CommitId) -> Result<Vec<CommitId>> {
        let mut parents: HashMap<CommitId, Vec<CommitId>> = HashMap::new();
        let mut stack = vec![id];
        while let Some(c) = stack.pop() {
            if parents.contains_key(&c) {
                continue;
            }
            let commit = self.get_commit(c)?;
            parents.insert(c, commit.parents.clone());
            stack.extend(commit.parents.iter().copied());
        }
        // Kahn's algorithm over "parent -> child" edges.
        let mut indegree: HashMap<CommitId, usize> =
            parents.iter().map(|(c, p)| (*c, p.len())).collect();
        let mut children: HashMap<CommitId, Vec<CommitId>> = HashMap::new();
        for (c, ps) in &parents {
            for p in ps {
                children.entry(*p).or_default().push(*c);
            }
        }
        let mut ready: Vec<CommitId> =
            indegree.iter().filter(|(_, d)| **d == 0).map(|(c, _)| *c).collect();
        ready.sort_by_key(|c| (self.timestamp_of(*c), *c));
        let mut queue: VecDeque<CommitId> = ready.into();
        let mut out = Vec::with_capacity(parents.len());
        while let Some(c) = queue.pop_front() {
            out.push(c);
            for ch in children.get(&c).into_iter().flatten() {
                let d = indegree.get_mut(ch).expect("child was discovered");
                *d -= 1;
                if *d == 0 {
                    queue.push_back(*ch);
                }
            }
        }
        if out.len() != parents.len() {
            return Err(Error::Store("commit history contains a cycle".into()));
        }
        Ok(out)
    }

    fn timestamp_of(&self, id: CommitId) -> i64 {
        self.store.get_commit(&id).ok().flatten().map_or(0, |c| c.timestamp)
    }

    /// History from HEAD backwards along first parents, newest first.
    pub fn log(&self, limit: Option<usize>) -> Result<Vec<Commit>> {
        let mut out = Vec::new();
        let mut cursor = self.head_commit()?;
        while let Some(id) = cursor {
            if limit.is_some_and(|n| out.len() >= n) {
                break;
            }
            let c = self.get_commit(id)?;
            cursor = c.parents.first().copied();
            out.push(c);
        }
        Ok(out)
    }

    pub fn branches(&self) -> Result<Vec<(String, CommitId)>> {
        self.store.list_refs()
    }

    /// Resolve a revision: a branch name, `HEAD`, `HEAD~n`, `<branch>~n`, or a
    /// commit id (full or any unambiguous prefix of at least 4 hex digits).
    pub fn resolve(&self, rev: &str) -> Result<CommitId> {
        let rev = rev.trim();
        let (name, back) = match rev.split_once('~') {
            Some((n, k)) => (n, k.parse::<usize>().map_err(|_| Error::NoSuchRef(rev.into()))?),
            None => (rev, 0),
        };
        let mut id = self.resolve_base(name)?;
        for _ in 0..back {
            let c = self.get_commit(id)?;
            id = *c.parents.first().ok_or_else(|| Error::NoSuchRef(rev.into()))?;
        }
        Ok(id)
    }

    fn resolve_base(&self, name: &str) -> Result<CommitId> {
        if name.eq_ignore_ascii_case("head") {
            return self.head_commit()?.ok_or_else(|| Error::NoSuchRef("HEAD".into()));
        }
        if let Some(id) = self.store.get_ref(name)? {
            return Ok(id);
        }
        if let Some(id) = CommitId::parse(name) {
            return Ok(id);
        }
        if name.len() >= 4 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
            let lower = name.to_ascii_lowercase();
            let matches: Vec<CommitId> = self
                .store
                .all_commit_ids()?
                .into_iter()
                .filter(|c| c.to_string().starts_with(&lower))
                .collect();
            return match matches.len() {
                1 => Ok(matches[0]),
                0 => Err(Error::NoSuchRef(name.into())),
                n => Err(Error::NoSuchRef(format!("{name} is ambiguous ({n} commits match)"))),
            };
        }
        Err(Error::NoSuchRef(name.into()))
    }

    // ------------------------------------------------------------- staging

    /// Stage one op. Rejected immediately if it does not apply.
    pub fn stage(&mut self, op: Op) -> Result<()> {
        self.working.apply(&op)?;
        self.stage.push(op);
        self.persist_stage()
    }

    pub fn stage_all(&mut self, ops: impl IntoIterator<Item = Op>) -> Result<()> {
        // All-or-nothing: a half-applied batch would leave `working` describing
        // a state the user never asked for.
        let ops: Vec<Op> = ops.into_iter().collect();
        let mut probe = self.working.clone();
        for op in &ops {
            probe.apply(op)?;
        }
        self.working = probe;
        self.stage.extend(ops);
        self.persist_stage()
    }

    /// Drop the most recently staged op.
    pub fn unstage_last(&mut self) -> Result<Option<Op>> {
        let popped = self.stage.pop();
        if popped.is_some() {
            self.rebuild_working()?;
            self.persist_stage()?;
        }
        Ok(popped)
    }

    /// Drop the staged op at `index`, e.g. from a list in the UI.
    pub fn unstage_at(&mut self, index: usize) -> Result<Op> {
        if index >= self.stage.len() {
            return Err(Error::Invalid(format!("no staged change at position {index}")));
        }
        let removed = self.stage.remove(index);
        // Removing an earlier op can invalidate a later one (unstage the
        // ledger, and the transaction using it has nowhere to go).
        if let Err(e) = self.rebuild_working() {
            self.stage.insert(index, removed);
            self.rebuild_working()?;
            return Err(Error::Invalid(format!(
                "cannot drop that change: a later staged change depends on it ({e})"
            )));
        }
        self.persist_stage()?;
        Ok(removed)
    }

    pub fn clear_stage(&mut self) -> Result<()> {
        self.stage.clear();
        self.working = self.base.clone();
        self.persist_stage()
    }

    fn rebuild_working(&mut self) -> Result<()> {
        let mut w = self.base.clone();
        for op in &self.stage {
            w.apply(op)?;
        }
        self.working = w;
        Ok(())
    }

    fn persist_stage(&mut self) -> Result<()> {
        self.store.set_stage(&self.stage)?;
        self.store.flush()
    }

    /// What committing right now would do.
    pub fn report(&self) -> Result<ChangeReport> {
        report::build(&self.base, &self.stage)
    }

    // ----------------------------------------------------------- committing

    /// Turn the staging area into a commit. This is the only call that adds to
    /// history.
    pub fn commit(&mut self, message: impl Into<String>) -> Result<CommitId> {
        if self.stage.is_empty() {
            return Err(Error::Invalid("nothing staged to commit".into()));
        }
        let message = message.into();
        if message.trim().is_empty() {
            return Err(Error::Invalid("a commit needs a message".into()));
        }
        if !self.working.is_balanced() {
            return Err(Error::Invalid(
                "refusing to commit: debits do not equal credits (this is a bug, please report it)"
                    .into(),
            ));
        }
        let parents: Vec<CommitId> = self.head_commit()?.into_iter().collect();
        let commit = Commit::new(
            parents,
            self.author.clone(),
            now_seconds(),
            message,
            std::mem::take(&mut self.stage),
        )?;
        self.store.put_commit(&commit)?;
        self.advance_head(commit.id)?;
        self.store.set_stage(&[])?;
        self.store.flush()?;
        self.base = self.working.clone();
        Ok(commit.id)
    }

    fn advance_head(&mut self, id: CommitId) -> Result<()> {
        match self.head.clone() {
            Head::Branch { name } => self.store.set_ref(&name, Some(id))?,
            Head::Detached { .. } => {
                self.head = Head::Detached { at: id };
                self.store.set_head(&self.head)?;
            }
        }
        Ok(())
    }

    // -------------------------------------------------------- branch / move

    /// Create a branch at `at` (default: HEAD) without switching to it.
    pub fn branch(&mut self, name: &str, at: Option<&str>) -> Result<CommitId> {
        validate_branch_name(name)?;
        if self.store.get_ref(name)?.is_some() {
            return Err(Error::Invalid(format!("branch {name} already exists")));
        }
        let id = match at {
            Some(rev) => self.resolve(rev)?,
            None => self
                .head_commit()?
                .ok_or_else(|| Error::History("cannot branch before the first commit".into()))?,
        };
        self.store.set_ref(name, Some(id))?;
        self.store.flush()?;
        Ok(id)
    }

    pub fn delete_branch(&mut self, name: &str) -> Result<()> {
        if self.head.branch_name() == Some(name) {
            return Err(Error::Invalid("cannot delete the branch you are on".into()));
        }
        if self.store.get_ref(name)?.is_none() {
            return Err(Error::NoSuchRef(name.into()));
        }
        self.store.set_ref(name, None)?;
        self.store.flush()
    }

    /// Switch to a branch, or detach at a commit.
    ///
    /// Refuses while anything is staged: the staging area belongs to the
    /// branch it was entered on, and silently carrying it across is how people
    /// post rent to the wrong budget.
    pub fn checkout(&mut self, rev: &str) -> Result<()> {
        if !self.stage.is_empty() {
            return Err(Error::Invalid(
                "you have staged changes; commit or discard them before switching".into(),
            ));
        }
        let head = match self.store.get_ref(rev)? {
            Some(_) => Head::Branch { name: rev.to_string() },
            None => Head::Detached { at: self.resolve(rev)? },
        };
        self.head = head;
        self.store.set_head(&self.head)?;
        self.store.flush()?;
        self.reload()
    }

    /// Create a branch at HEAD and switch to it.
    pub fn checkout_new(&mut self, name: &str) -> Result<()> {
        self.branch(name, None)?;
        self.checkout(name)
    }

    // ------------------------------------------------------------- reverting

    /// Stage the reversal of a commit.
    ///
    /// Nothing is deleted. A reverted transaction gets a mirror-image
    /// transaction posted against it, which is what an accountant does and
    /// what an auditor expects to see. Some things cannot be reversed at all -
    /// a ledger, once opened, stays open - and those come back as notes.
    pub fn revert(&mut self, rev: &str) -> Result<Vec<String>> {
        let id = self.resolve(rev)?;
        let commit = self.get_commit(id)?;
        let mut before = match commit.parents.first() {
            Some(p) => self.budget_at(*p)?,
            None => Budget::new(),
        };

        let mut inverses: Vec<Op> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for op in &commit.ops {
            match invert(op, &before, &commit.id.short()) {
                Inverse::Op(inv) => inverses.push(inv),
                Inverse::Many(mut v) => inverses.append(&mut v),
                Inverse::Nothing(why) => notes.push(why),
            }
            before.apply(op)?;
        }
        inverses.reverse();

        if inverses.is_empty() {
            if notes.is_empty() {
                notes
                    .push(format!("commit {} has nothing that can be reversed", commit.id.short()));
            }
            return Ok(notes);
        }
        self.stage_all(inverses)?;
        Ok(notes)
    }

    // -------------------------------------------------------------- rebasing

    /// Replay `branch` onto `onto`, rewriting its commits.
    ///
    /// Every replayed op is applied to the new base before the commit is
    /// written, so a rebase that would produce an impossible budget - a
    /// transaction against a ledger that only exists on the old base -
    /// fails cleanly and leaves the branch where it was.
    pub fn rebase(&mut self, branch: &str, onto: &str) -> Result<usize> {
        if !self.stage.is_empty() {
            return Err(Error::Invalid(
                "you have staged changes; commit or discard them before rebasing".into(),
            ));
        }
        let tip = self.store.get_ref(branch)?.ok_or_else(|| Error::NoSuchRef(branch.into()))?;
        let onto_id = self.resolve(onto)?;
        if tip == onto_id {
            return Ok(0);
        }

        let base = self.merge_base(tip, onto_id)?;
        if base == Some(tip) {
            // The branch is already an ancestor of `onto`: fast-forward it.
            self.store.set_ref(branch, Some(onto_id))?;
            self.store.flush()?;
            if self.head.branch_name() == Some(branch) {
                self.reload()?;
            }
            return Ok(0);
        }
        if base == Some(onto_id) {
            return Ok(0); // Already on top of `onto`.
        }

        let to_replay = self.first_parent_range(base, tip)?;
        let mut budget = self.budget_at(onto_id)?;
        let mut parent = onto_id;
        for old in &to_replay {
            for op in &old.ops {
                budget.apply(op).map_err(|e| {
                    Error::History(format!(
                        "commit {} does not apply onto {}: {e}",
                        old.id.short(),
                        onto_id.short()
                    ))
                })?;
            }
            let new = Commit::new(
                vec![parent],
                old.author.clone(),
                old.timestamp,
                old.message.clone(),
                old.ops.clone(),
            )?;
            self.store.put_commit(&new)?;
            parent = new.id;
        }
        self.store.set_ref(branch, Some(parent))?;
        self.store.flush()?;
        if self.head.branch_name() == Some(branch) {
            self.reload()?;
        }
        Ok(to_replay.len())
    }

    /// Commits on the first-parent path from `stop` (exclusive) to `tip`
    /// (inclusive), oldest first.
    fn first_parent_range(&self, stop: Option<CommitId>, tip: CommitId) -> Result<Vec<Commit>> {
        let mut out = Vec::new();
        let mut cursor = Some(tip);
        while let Some(id) = cursor {
            if Some(id) == stop {
                break;
            }
            let c = self.get_commit(id)?;
            cursor = c.parents.first().copied();
            out.push(c);
        }
        out.reverse();
        Ok(out)
    }

    /// The most recent common ancestor of two commits, if any.
    pub fn merge_base(&self, a: CommitId, b: CommitId) -> Result<Option<CommitId>> {
        let ancestors_a: HashSet<CommitId> = self.topo_order(a)?.into_iter().collect();
        // Walk b's ancestors newest-first and take the first one a also has.
        for id in self.topo_order(b)?.into_iter().rev() {
            if ancestors_a.contains(&id) {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    // -------------------------------------------------------------- issuers

    /// Stage every transaction the issuers owe on or before `through`.
    ///
    /// Returns the per-issuer breakdown so the UI can show "here is what your
    /// recurring payments did since last time" before anything is committed.
    pub fn run_issuers(&mut self, through: Date) -> Result<Vec<IssuerRun>> {
        let runs = issuer::run_all(&self.working, through);
        let ops: Vec<Op> = runs.iter().flat_map(|r| r.ops.iter().cloned()).collect();
        if !ops.is_empty() {
            self.stage_all(ops)?;
        }
        Ok(runs)
    }

    // -------------------------------------------- convenience constructors

    /// Stage a new ledger and return its stable id.
    pub fn add_ledger(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        normality: Normality,
        opened: Date,
    ) -> Result<LedgerUid> {
        let uid = LedgerUid::new();
        self.stage(Op::CreateLedger {
            uid,
            name: name.into(),
            description: description.into(),
            normality,
            opened,
        })?;
        Ok(uid)
    }

    /// Stage an ordinary two-sided transfer: move `amount` from `credit` to
    /// `debit`. Returns the transaction's stable id.
    #[allow(clippy::too_many_arguments)]
    pub fn post(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        date: Date,
        amount: Money,
        debit: LedgerUid,
        credit: LedgerUid,
    ) -> Result<TxUid> {
        // Here `amount` names a magnitude, so a negative one is a mistake even
        // though the resulting legs would balance perfectly well. Say so,
        // rather than quietly posting the entry backwards.
        if amount.0 <= 0 {
            return Err(Error::Invalid(
                "amount must be positive; swap the ledgers to move money the other way".into(),
            ));
        }
        self.post_split(name, description, date, simple_legs(debit, credit, amount))
    }

    /// Stage a transaction with any number of sides.
    ///
    /// This is the general form; [`Repo::post`] is the two-leg shorthand. The
    /// legs must sum to zero, which is the only thing that makes the entry a
    /// double entry.
    pub fn post_split(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        date: Date,
        legs: Vec<Leg>,
    ) -> Result<TxUid> {
        let uid = TxUid::new();
        self.stage(Op::PostTransaction {
            uid,
            name: name.into(),
            description: description.into(),
            date,
            legs,
            parent: Parent::Manual,
        })?;
        Ok(uid)
    }

    /// Stage an issuer that posts a two-sided entry on a schedule.
    #[allow(clippy::too_many_arguments)]
    pub fn add_issuer(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        debit: LedgerUid,
        credit: LedgerUid,
        amount: Money,
        schedule: Schedule,
        start: Date,
    ) -> Result<IssuerUid> {
        if amount.0 <= 0 {
            return Err(Error::Invalid(
                "amount must be positive; swap the ledgers to move money the other way".into(),
            ));
        }
        self.add_issuer_split(
            name,
            description,
            simple_legs(debit, credit, amount),
            schedule,
            start,
        )
    }

    /// Stage an issuer that posts a split entry - a paycheque, say - on a
    /// schedule.
    pub fn add_issuer_split(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        legs: Vec<Leg>,
        schedule: Schedule,
        start: Date,
    ) -> Result<IssuerUid> {
        let uid = IssuerUid::new();
        self.stage(Op::CreateIssuer {
            uid,
            name: name.into(),
            description: description.into(),
            legs,
            schedule,
            start,
        })?;
        Ok(uid)
    }

    pub fn add_bucket(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<BucketUid> {
        let uid = BucketUid::new();
        self.stage(Op::CreateBucket { uid, name: name.into(), description: description.into() })?;
        Ok(uid)
    }

    pub fn store(&self) -> &S {
        &self.store
    }
}

enum Inverse {
    Op(Op),
    Many(Vec<Op>),
    Nothing(String),
}

/// The inverse of one op, computed against the budget as it stood *before*
/// that op was applied.
fn invert(op: &Op, before: &Budget, source: &str) -> Inverse {
    match op {
        Op::CreateLedger { name, .. } => Inverse::Nothing(format!(
            "ledger \"{name}\" stays open: ledgers are permanent, so its creation was not reverted"
        )),
        Op::EditLedger { uid, name, description } => match before.ledgers.ix(*uid) {
            Some(ix) => Inverse::Op(Op::EditLedger {
                uid: *uid,
                name: name.as_ref().map(|_| before.ledgers.name[ix.get()].clone()),
                description: description
                    .as_ref()
                    .map(|_| before.ledgers.description[ix.get()].clone()),
            }),
            None => Inverse::Nothing(format!("ledger {} is gone; edit not reverted", uid.short())),
        },
        Op::PostTransaction { uid, name, date, legs, .. } => {
            Inverse::Op(Op::PostTransaction {
                uid: TxUid::new(),
                name: format!("Reversal of {name}"),
                description: format!("Reverses transaction {} from commit {source}", uid.short()),
                // Dated to match the original, not to today, so a correction
                // lands in the period it belongs to and monthly totals stay
                // true. A business would date it on the day of discovery; a
                // personal budget wants the month to read correctly.
                date: *date,
                // The whole reversal: negate every side. Sum-zero in means
                // sum-zero out, so this is correct for a four-leg paycheque
                // for exactly the same reason it is correct for a transfer.
                legs: legs.iter().map(|l| Leg { ledger: l.ledger, amount: -l.amount }).collect(),
                parent: Parent::Manual,
            })
        }
        Op::EditTransaction { uid, name, description } => match before.transactions.ix(*uid) {
            Some(ix) => Inverse::Op(Op::EditTransaction {
                uid: *uid,
                name: name.as_ref().map(|_| before.transactions.name[ix.get()].clone()),
                description: description
                    .as_ref()
                    .map(|_| before.transactions.description[ix.get()].clone()),
            }),
            None => {
                Inverse::Nothing(format!("transaction {} is gone; edit not reverted", uid.short()))
            }
        },
        // An issuer cannot be deleted, so the closest honest inverse is to
        // stop it from producing anything further.
        Op::CreateIssuer { uid, .. } => {
            Inverse::Op(Op::SetIssuerPaused { uid: *uid, paused: true })
        }
        Op::EditIssuer { uid, name, description } => match before.issuers.ix(*uid) {
            Some(ix) => Inverse::Op(Op::EditIssuer {
                uid: *uid,
                name: name.as_ref().map(|_| before.issuers.name[ix.get()].clone()),
                description: description
                    .as_ref()
                    .map(|_| before.issuers.description[ix.get()].clone()),
            }),
            None => Inverse::Nothing(format!("issuer {} is gone; edit not reverted", uid.short())),
        },
        Op::SetIssuerPaused { uid, .. } => match before.issuers.ix(*uid) {
            Some(ix) => Inverse::Op(Op::SetIssuerPaused {
                uid: *uid,
                paused: before.issuers.paused[ix.get()],
            }),
            None => Inverse::Nothing(format!("issuer {} is gone; pause not reverted", uid.short())),
        },
        Op::AdvanceIssuer { uid, .. } => Inverse::Nothing(format!(
            "issuer {} stays advanced, so reverting its transactions does not make it re-post them",
            uid.short()
        )),
        Op::CreateBucket { uid, .. } => Inverse::Op(Op::DeleteBucket { uid: *uid }),
        Op::EditBucket { uid, name, description } => match before.buckets.ix(*uid) {
            Some(ix) => Inverse::Op(Op::EditBucket {
                uid: *uid,
                name: name.as_ref().map(|_| before.buckets.name[ix.get()].clone()),
                description: description
                    .as_ref()
                    .map(|_| before.buckets.description[ix.get()].clone()),
            }),
            None => Inverse::Nothing(format!("bucket {} is gone; edit not reverted", uid.short())),
        },
        Op::DeleteBucket { uid } => match before.buckets.ix(*uid) {
            Some(ix) => {
                let i = ix.get();
                let mut ops = vec![Op::CreateBucket {
                    uid: *uid,
                    name: before.buckets.name[i].clone(),
                    description: before.buckets.description[i].clone(),
                }];
                ops.extend(before.buckets.members[i].iter().map(|a| Op::AddToBucket {
                    bucket: *uid,
                    ledger: before.ledgers.uid[a.get()],
                }));
                Inverse::Many(ops)
            }
            None => Inverse::Nothing(format!("bucket {} was already gone", uid.short())),
        },
        Op::AddToBucket { bucket, ledger } => {
            Inverse::Op(Op::RemoveFromBucket { bucket: *bucket, ledger: *ledger })
        }
        Op::RemoveFromBucket { bucket, ledger } => {
            Inverse::Op(Op::AddToBucket { bucket: *bucket, ledger: *ledger })
        }
    }
}

fn now_seconds() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
