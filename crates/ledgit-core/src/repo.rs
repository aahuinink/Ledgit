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
//!
//! An op that applied when it was staged can stop applying later: drop the
//! staged ledger a staged transaction posts to, and the transaction has
//! nowhere to go. It is not thrown away. It stays in the stage, *broken* -
//! left out of `working`, listed by [`Repo::broken`] - and nothing can be
//! committed until it is edited back into shape or dropped.

use crate::commit::{validate_branch_name, Commit, CommitId, Head, MergeInfo};
use crate::date::Date;
use crate::error::{Error, Result};
use crate::id::{BucketUid, IssuerUid, LedgerUid, TxUid, ViewUid};
use crate::issuer::{self, IssuerRun};
use crate::merge::{self, Choices, MergeKind, MergePreview};
use crate::model::{simple_legs, AmountRule, Leg, Normality, Parent, Schedule, ViewSpec};
use crate::money::Money;
use crate::op::Op;
use crate::report::{self, Broken, ChangeReport};
use crate::state::Budget;
use crate::store::{Store, DEFAULT_BRANCH};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

/// What a checkout does with staged work.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StagedWork {
    /// Refuse to switch while anything is staged.
    Refuse,
    /// Set it aside on the branch being left; it comes back when you return.
    Shelve,
    /// Take it to the other branch. Changes that do not apply there - an
    /// entry posting to a ledger that branch never opened - are flagged as
    /// broken, not dropped.
    Bring,
}

/// What a merge did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MergeOutcome {
    /// The new commit, or for a fast-forward, the source's tip.
    pub commit: CommitId,
    pub fast_forward: bool,
    /// Ops in the merge commit.
    pub ops: usize,
}

pub struct Repo<S: Store> {
    store: S,
    author: String,
    head: Head,
    base: Budget,
    stage: Vec<Op>,
    /// Staged ops that do not apply, in stage order. Derived; see above.
    broken: Vec<Broken>,
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
            broken: Vec::new(),
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
                    broken: Vec::new(),
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
        // A staged op can become invalid under a new base (it may reference a
        // ledger that is not on this branch). It is flagged, not dropped.
        self.rebuild_working();
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

    /// Staged ops that no longer apply. Committing is refused while there
    /// are any.
    pub fn broken(&self) -> &[Broken] {
        &self.broken
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

    /// Every commit reachable from a branch or from HEAD, newest first, and
    /// always a child before its parents - the order a commit graph is drawn
    /// in. `log` walks only HEAD's first parents; this is the whole tree.
    /// Commits that nothing reaches any more (a rebase's originals) are left
    /// out, as `git log --all` would.
    pub fn graph(&self, limit: Option<usize>) -> Result<Vec<Commit>> {
        let mut tips: Vec<CommitId> = self.branches()?.into_iter().map(|(_, id)| id).collect();
        tips.extend(self.head_commit()?);

        // Everything reachable, and how many children each has among it.
        let mut commits: HashMap<CommitId, Commit> = HashMap::new();
        let mut children: HashMap<CommitId, usize> = HashMap::new();
        let mut queue: VecDeque<CommitId> = tips.iter().copied().collect();
        while let Some(id) = queue.pop_front() {
            if commits.contains_key(&id) {
                continue;
            }
            let c = self.get_commit(id)?;
            for p in &c.parents {
                *children.entry(*p).or_default() += 1;
                queue.push_back(*p);
            }
            commits.insert(id, c);
        }

        // Kahn's algorithm, taking the newest ready commit each time, so
        // branches interleave by date but no parent precedes a child.
        let key = |c: &Commit| (c.timestamp, c.id);
        let mut ready: std::collections::BinaryHeap<(i64, CommitId)> = commits
            .values()
            .filter(|c| children.get(&c.id).copied().unwrap_or(0) == 0)
            .map(key)
            .collect();
        let mut out = Vec::new();
        while let Some((_, id)) = ready.pop() {
            if limit.is_some_and(|n| out.len() >= n) {
                break;
            }
            let c = commits.remove(&id).expect("queued commits are collected");
            for p in &c.parents {
                let n = children.get_mut(p).expect("counted above");
                *n -= 1;
                if *n == 0 {
                    ready.push(key(&commits[p]));
                }
            }
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
        self.stage_all([op])
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
            self.rebuild_working();
            self.persist_stage()?;
        }
        Ok(popped)
    }

    /// Drop the staged op at `index`, e.g. from a list in the UI.
    ///
    /// Later ops that depended on it are kept but become [broken](Repo::broken).
    pub fn unstage_at(&mut self, index: usize) -> Result<Op> {
        if index >= self.stage.len() {
            return Err(Error::Invalid(format!("no staged change at position {index}")));
        }
        let removed = self.stage.remove(index);
        self.rebuild_working();
        self.persist_stage()?;
        Ok(removed)
    }

    /// Replace the staged op at `index` with an edited version of it, in
    /// place. The edit itself must apply; ops after it are re-checked, so an
    /// edit can break a later op or mend one.
    pub fn replace_staged(&mut self, index: usize, op: Op) -> Result<Op> {
        if index >= self.stage.len() {
            return Err(Error::Invalid(format!("no staged change at position {index}")));
        }
        let old = std::mem::replace(&mut self.stage[index], op);
        self.rebuild_working();
        if let Some(b) = self.broken.iter().find(|b| b.index == index) {
            let reason = b.reason.clone();
            self.stage[index] = old;
            self.rebuild_working();
            return Err(Error::Invalid(format!("that edit does not apply: {reason}")));
        }
        self.persist_stage()?;
        Ok(old)
    }

    /// Swap the whole staging area for `ops`, e.g. to undo the last edit to
    /// it. Unlike [`Repo::stage_all`] this never refuses: ops that do not
    /// apply are kept, flagged as broken.
    pub fn set_stage(&mut self, ops: Vec<Op>) -> Result<()> {
        self.stage = ops;
        self.rebuild_working();
        self.persist_stage()
    }

    pub fn clear_stage(&mut self) -> Result<()> {
        self.stage.clear();
        self.broken.clear();
        self.working = self.base.clone();
        self.persist_stage()
    }

    /// Refold `working` from `base` and the stage, flagging every op that
    /// does not apply rather than stopping at the first.
    fn rebuild_working(&mut self) {
        let mut w = self.base.clone();
        self.broken.clear();
        for (index, op) in self.stage.iter().enumerate() {
            if let Err(e) = w.apply(op) {
                self.broken.push(Broken { index, reason: e.to_string() });
            }
        }
        self.working = w;
    }

    fn persist_stage(&mut self) -> Result<()> {
        self.store.set_stage(&self.stage)?;
        self.store.flush()
    }

    /// What committing right now would do.
    pub fn report(&self) -> Result<ChangeReport> {
        let healthy: Vec<Op> = self
            .stage
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.broken.iter().any(|b| b.index == *i))
            .map(|(_, op)| op.clone())
            .collect();
        let mut r = report::build(&self.base, &healthy)?;
        r.lines = self.stage.iter().map(|o| o.summary()).collect();
        r.broken = self.broken.clone();
        r.commitments = crate::available::changes(&self.base, &self.working, Date::today_utc());
        Ok(r)
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
        if let Some(b) = self.broken.first() {
            return Err(Error::Invalid(format!(
                "{} staged change(s) no longer apply; fix or drop them first. \
                 First: {} ({})",
                self.broken.len(),
                self.stage[b.index].summary(),
                b.reason
            )));
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
        let shelved = self.shelved(name)?;
        if shelved > 0 {
            return Err(Error::Invalid(format!(
                "{name} has {shelved} shelved change(s); switch to it and commit or discard them first"
            )));
        }
        self.store.set_ref(name, None)?;
        self.store.flush()
    }

    /// Switch to a branch, or detach at a commit.
    ///
    /// Refuses while anything is staged: the staging area belongs to the
    /// branch it was entered on, and silently carrying it across is how people
    /// post rent to the wrong budget. [`Repo::checkout_with`] says what to do
    /// with it instead.
    pub fn checkout(&mut self, rev: &str) -> Result<()> {
        self.checkout_with(rev, StagedWork::Refuse).map(|_| ())
    }

    /// Switch to a branch, or detach at a commit, deciding what happens to
    /// anything staged. Arriving on a branch puts back whatever was shelved
    /// there, after anything brought along. Returns how many changes came back
    /// off the shelf.
    pub fn checkout_with(&mut self, rev: &str, staged: StagedWork) -> Result<usize> {
        let head = match self.store.get_ref(rev)? {
            Some(_) => Head::Branch { name: rev.to_string() },
            None => Head::Detached { at: self.resolve(rev)? },
        };
        if head == self.head {
            return Ok(0);
        }
        let mut carried = Vec::new();
        if !self.stage.is_empty() {
            match staged {
                StagedWork::Refuse => {
                    return Err(Error::Invalid(
                        "you have staged changes; commit, discard or shelve them before switching"
                            .into(),
                    ))
                }
                StagedWork::Shelve => {
                    let Some(branch) = self.head.branch_name().map(str::to_string) else {
                        return Err(Error::Invalid(
                            "staged work can only be shelved on a branch, and HEAD is detached; \
                             bring it along, commit it or discard it"
                                .into(),
                        ));
                    };
                    // Shelf first, then the stage: a crash in between leaves the
                    // work in both places rather than in neither.
                    let mut shelf = self.store.get_shelf(&branch)?;
                    shelf.extend(self.stage.iter().cloned());
                    self.store.set_shelf(&branch, &shelf)?;
                }
                StagedWork::Bring => carried = self.stage.clone(),
            }
        }
        let shelf = match head.branch_name() {
            Some(name) => self.store.get_shelf(name)?,
            None => Vec::new(),
        };
        let restored = shelf.len();
        carried.extend(shelf);
        self.stage = carried;
        self.store.set_stage(&self.stage)?;
        if let Some(name) = head.branch_name() {
            if restored > 0 {
                self.store.set_shelf(name, &[])?;
            }
        }
        self.head = head;
        self.store.set_head(&self.head)?;
        self.store.flush()?;
        self.reload()?;
        Ok(restored)
    }

    /// How many staged changes are shelved on `branch`, waiting for you to
    /// switch back to it.
    pub fn shelved(&self, branch: &str) -> Result<usize> {
        Ok(self.store.get_shelf(branch)?.len())
    }

    /// Create a branch at HEAD and switch to it. Anything staged comes too:
    /// the new branch starts where this one is, so it applies unchanged - the
    /// usual reason to branch is "this work is a what-if".
    pub fn checkout_new(&mut self, name: &str) -> Result<()> {
        self.branch(name, None)?;
        self.checkout_with(name, StagedWork::Bring).map(|_| ())
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

        // One group per original op. The groups are undone newest first, but
        // each group keeps its own order: restoring a deleted bucket must
        // create it before adding its members back.
        let mut groups: Vec<Vec<Op>> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for op in &commit.ops {
            match invert(op, &before, &commit.id.short()) {
                Inverse::Op(inv) => groups.push(vec![inv]),
                Inverse::Many(v) => groups.push(v),
                Inverse::Nothing(why) => notes.push(why),
            }
            before.apply(op)?;
        }
        let inverses: Vec<Op> = groups.into_iter().rev().flatten().collect();

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

    /// Stage the reversal of one entry: every side negated, linked back to
    /// it. Returns the reversal's uid.
    pub fn reverse_transaction(&mut self, tx: TxUid) -> Result<TxUid> {
        let l = &self.working;
        let ix =
            l.transactions.ix(tx).ok_or(Error::NoSuchEntity { kind: "transaction", uid: tx.0 })?;
        if l.transactions.reverses[ix.get()].is_some() {
            return Err(Error::Invalid(
                "that entry is itself a reversal; reverse the commit that made it instead".into(),
            ));
        }
        if let Some(r) = l.reversed_by(ix) {
            return Err(Error::Invalid(format!(
                "\"{}\" is already reversed (by {})",
                l.transactions.name[ix.get()],
                l.transactions.uid[r.get()].short()
            )));
        }
        let t = l.transaction(ix);
        let op = Op::reversal(
            tx,
            &t.name,
            t.date,
            &t.legs,
            format!("Reverses transaction {}", tx.short()),
        );
        let Op::PostTransaction { uid, .. } = op else { unreachable!("a reversal posts") };
        self.stage(op)?;
        Ok(uid)
    }

    /// Stage a commit's ops on the current branch, exactly as they were -
    /// same uids, so a later reconcile sees them as the same entries. Ops
    /// already here (an entry or ledger that exists) are skipped; ops that
    /// do not apply are staged anyway and flagged, to fix or drop. Returns
    /// (staged, skipped).
    pub fn cherry_pick(&mut self, rev: &str) -> Result<(usize, usize)> {
        let commit = self.get_commit(self.resolve(rev)?)?;
        let mut probe = self.working.clone();
        let mut take = Vec::new();
        let mut skipped = 0;
        for op in commit.ops {
            match probe.apply(&op) {
                Ok(()) => take.push(op),
                Err(Error::Duplicate { .. }) => skipped += 1,
                Err(_) => take.push(op),
            }
        }
        let staged = take.len();
        let mut stage = std::mem::take(&mut self.stage);
        stage.extend(take);
        self.set_stage(stage)?;
        Ok((staged, skipped))
    }

    // --------------------------------------------------------------- merging

    /// What merging `source` into the current branch would involve. Reads
    /// the committed budget: anything staged is not part of a merge.
    pub fn merge_preview(&self, source: &str, kind: MergeKind) -> Result<MergePreview> {
        let tip = self.resolve(source)?;
        let head = self.head_commit()?;
        if head == Some(tip) {
            return Err(Error::Invalid(format!("{source} is where you already are")));
        }
        let base_id = match head {
            Some(h) => self.merge_base(h, tip)?,
            None => None,
        };
        let base = match base_id {
            Some(b) => self.budget_at(b)?,
            None => Budget::new(),
        };
        let src = self.budget_at(tip)?;
        let mut p = merge::preview(kind, &self.base, &src, &base, source);
        p.source_tip = Some(tip);
        p.dest_head = head;
        // Nothing has happened here since the branch split: a Replace is
        // just moving up to the branch.
        p.fast_forward = kind == MergeKind::Replace && (head.is_none() || base_id == head);
        Ok(p)
    }

    /// Make the merge `preview` describes, as chosen. One commit on the
    /// current branch, recording where it came from - or, for a Replace
    /// with nothing new here, the branch simply moves up to the source.
    pub fn merge(
        &mut self,
        preview: &MergePreview,
        choices: &Choices,
        message: impl Into<String>,
    ) -> Result<MergeOutcome> {
        if !self.stage.is_empty() {
            return Err(Error::Invalid(
                "you have staged changes; commit or discard them before merging".into(),
            ));
        }
        let tip = preview.source_tip.ok_or_else(|| Error::Invalid("no source to merge".into()))?;
        if self.head_commit()? != preview.dest_head
            || self.resolve(&preview.source).ok() != Some(tip)
        {
            return Err(Error::Invalid(
                "a branch has moved since this merge was previewed; preview it again".into(),
            ));
        }
        if preview.fast_forward {
            self.advance_head(tip)?;
            self.store.flush()?;
            self.reload()?;
            return Ok(MergeOutcome { commit: tip, fast_forward: true, ops: 0 });
        }
        let ops = preview.ops(choices)?;
        if ops.is_empty() {
            return Err(Error::Invalid("with these choices nothing comes across".into()));
        }
        let mut after = self.base.clone();
        for op in &ops {
            after.apply(op)?;
        }
        if !after.is_balanced() {
            return Err(Error::Invalid(
                "refusing to merge: debits do not equal credits (this is a bug, please report it)"
                    .into(),
            ));
        }
        let message = message.into();
        let message = if message.trim().is_empty() {
            format!("{} {} into {}", preview.kind.name(), preview.source, self.head)
        } else {
            message
        };
        let n = ops.len();
        let commit = Commit::with_merge(
            self.head_commit()?.into_iter().collect(),
            self.author.clone(),
            now_seconds(),
            message,
            ops,
            Some(MergeInfo { from: tip, kind: preview.kind }),
        )?;
        self.store.put_commit(&commit)?;
        self.advance_head(commit.id)?;
        self.store.flush()?;
        self.base = after.clone();
        self.working = after;
        Ok(MergeOutcome { commit: commit.id, fast_forward: false, ops: n })
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
            reverses: None,
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
            rule: None,
            settles: None,
        })?;
        Ok(uid)
    }

    /// Stage a one-off payment that settles `tx`: on `date`, `amount` moves
    /// from `from` into `owed_on` - out of chequing, onto the card the
    /// purchase was put on. It is an issuer, so it posts when issuers are run
    /// on or after `date`, and until then the money is spoken for.
    pub fn pay_later(
        &mut self,
        tx: TxUid,
        owed_on: LedgerUid,
        from: LedgerUid,
        amount: Money,
        date: Date,
    ) -> Result<IssuerUid> {
        if amount.0 <= 0 {
            return Err(Error::Invalid("a payment must be above zero".into()));
        }
        let name = self
            .working
            .transactions
            .ix(tx)
            .map(|ix| self.working.transactions.name[ix.get()].clone())
            .unwrap_or_else(|| "a transaction".into());
        let op = issuer::settlement(tx, &name, owed_on, from, amount, date);
        let Op::CreateIssuer { uid, .. } = op else {
            unreachable!("a settlement creates an issuer")
        };
        self.stage(op)?;
        Ok(uid)
    }

    /// Stage an issuer whose amount is worked out from a balance each time
    /// it fires: `debit` receives it, `credit` gives it.
    ///
    /// Interest on a loan debits an interest expense and credits the loan,
    /// with the loan as `of`; a 5% sweep from savings debits the target and
    /// credits savings, with savings as `of`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_rule_issuer(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        debit: LedgerUid,
        credit: LedgerUid,
        rule: AmountRule,
        schedule: Schedule,
        start: Date,
    ) -> Result<IssuerUid> {
        let uid = IssuerUid::new();
        self.stage(Op::CreateIssuer {
            uid,
            name: name.into(),
            description: description.into(),
            // Proportions only: all of the worked-out amount, one way.
            legs: simple_legs(debit, credit, Money::from_major(1)),
            schedule,
            start,
            rule: Some(rule),
            settles: None,
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

    pub fn add_view(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        spec: ViewSpec,
    ) -> Result<ViewUid> {
        let uid = ViewUid::new();
        self.stage(Op::CreateView {
            uid,
            name: name.into(),
            description: description.into(),
            spec,
        })?;
        Ok(uid)
    }

    /// Stage the renames that move everything under `from` to `to`, and
    /// re-point any bucket that tracks it. Returns how many ledgers moved.
    pub fn rename_subtree(&mut self, from: &str, to: &str) -> Result<usize> {
        if crate::tree::normalize(to).is_empty() {
            return Err(Error::Invalid("the new path cannot be blank".into()));
        }
        let ops = crate::tree::rename_ops(&self.working, from, to);
        let moved = ops.iter().filter(|o| matches!(o, Op::EditLedger { .. })).count();
        if moved == 0 {
            return Err(Error::Invalid(format!("no ledger is at or under \"{from}\"")));
        }
        self.stage_all(ops)?;
        Ok(moved)
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
        Op::SetLedgerGoals { uid, .. } => match before.ledgers.ix(*uid) {
            Some(ix) => Inverse::Op(Op::SetLedgerGoals {
                uid: *uid,
                target: before.ledgers.target[ix.get()],
                alerts: before.ledgers.alerts[ix.get()].clone(),
            }),
            None => Inverse::Nothing(format!("ledger {} is gone; goals not reverted", uid.short())),
        },
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
        Op::PostTransaction { uid, name, date, legs, .. } => Inverse::Op(Op::reversal(
            *uid,
            name,
            *date,
            legs,
            format!("Reverses transaction {} from commit {source}", uid.short()),
        )),
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
        // Puts back the amount the date had before. If the issuer has posted
        // that date since, the op is refused when staged and flagged.
        Op::SetIssuerOverride { uid, date, .. } => match before.issuers.ix(*uid) {
            Some(ix) => Inverse::Op(Op::SetIssuerOverride {
                uid: *uid,
                date: *date,
                amount: before.issuers.override_on(ix, *date),
            }),
            None => {
                Inverse::Nothing(format!("issuer {} is gone; amount not put back", uid.short()))
            }
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
                // What was *put* in the bucket, not what it happened to hold:
                // restoring the subtree brings back its ledgers, and any added
                // under it since.
                ops.extend(before.buckets.explicit[i].iter().map(|a| Op::AddToBucket {
                    bucket: *uid,
                    ledger: before.ledgers.uid[a.get()],
                }));
                ops.extend(
                    before.buckets.subtrees[i]
                        .iter()
                        .map(|p| Op::AddSubtreeToBucket { bucket: *uid, path: p.clone() }),
                );
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
        Op::AddSubtreeToBucket { bucket, path } => {
            Inverse::Op(Op::RemoveSubtreeFromBucket { bucket: *bucket, path: path.clone() })
        }
        Op::RemoveSubtreeFromBucket { bucket, path } => {
            Inverse::Op(Op::AddSubtreeToBucket { bucket: *bucket, path: path.clone() })
        }
        Op::CreateView { uid, .. } => Inverse::Op(Op::DeleteView { uid: *uid }),
        Op::EditView { uid, name, description, spec } => match before.views.ix(*uid) {
            Some(ix) => {
                let i = ix.get();
                Inverse::Op(Op::EditView {
                    uid: *uid,
                    name: name.as_ref().map(|_| before.views.name[i].clone()),
                    description: description.as_ref().map(|_| before.views.description[i].clone()),
                    spec: spec.as_ref().map(|_| before.views.spec[i].clone()),
                })
            }
            None => Inverse::Nothing(format!("view {} is gone; edit not reverted", uid.short())),
        },
        Op::DeleteView { uid } => match before.views.ix(*uid) {
            Some(ix) => {
                let i = ix.get();
                Inverse::Op(Op::CreateView {
                    uid: *uid,
                    name: before.views.name[i].clone(),
                    description: before.views.description[i].clone(),
                    spec: before.views.spec[i].clone(),
                })
            }
            None => Inverse::Nothing(format!("view {} was already gone", uid.short())),
        },
        Op::SetVariable { name, .. } => match before.variables.position(name.trim()) {
            Some(i) => Inverse::Op(Op::SetVariable {
                name: before.variables.name[i].clone(),
                value: before.variables.value[i].clone(),
            }),
            None => Inverse::Op(Op::DeleteVariable { name: name.trim().to_string() }),
        },
        Op::DeleteVariable { name } => match before.variables.position(name.trim()) {
            Some(i) => Inverse::Op(Op::SetVariable {
                name: before.variables.name[i].clone(),
                value: before.variables.value[i].clone(),
            }),
            None => Inverse::Nothing(format!("variable {name} was already gone")),
        },
    }
}

fn now_seconds() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
