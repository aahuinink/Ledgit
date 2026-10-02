//! Merging one branch into another: Replace, Reconcile and Adopt.
//!
//! History is never deleted, so a merge is always *one new commit on the
//! destination* made of ordinary ops - reversals, posts, creates, edits -
//! that record where they came from in [`crate::commit::MergeInfo`]. The
//! branch is not made a parent: a budget is the fold of every ancestor, and a
//! second parent would replay the whole branch on top.
//!
//! # Rows
//!
//! A transaction is *live* unless it is a reversal or has been reversed
//! ([`Budget::live_transactions`]). The rows of a merge are the live entries
//! one side has and the other does not, matched by uid - a cherry-picked
//! entry keeps its uid, so it matches and is not listed. A ledger is
//! *changed* on a side when one of that side's rows touches it, and it
//! *clashes* when changed on both. Then:
//!
//! 1. a destination row touching no ledger the branch changed: **Keep**
//!    (nothing) or **Revert** (post its reversal);
//! 2. a branch row touching no ledger the destination changed: **Keep**
//!    (post it on the destination) or **Drop** (leave it out);
//! 3. a row touching a clashing ledger: decided per [`ClashGroup`] - the
//!    ledgers its rows link together - as **Force** (keep both sides, may
//!    double-post), **Revert** (reverse the destination's, post the
//!    branch's) or **Drop** (keep the destination's), with per-row
//!    overrides. Nothing merges until each group is decided, unless the
//!    undecided ones are settled the way Replace would.
//!
//! **Replace** takes the branch's side of everything. **Adopt** has no rows:
//! it brings issuers and settings - plans - and moves no balance.
//!
//! # Issuers cannot rewind
//!
//! If the destination ran an issuer further than the branch did, reverting
//! the destination's posts would leave a gap the issuer will never fill
//! (`AdvanceIssuer` never goes back). So before rows are worked out, the
//! branch's issuers are run up to the destination's date on the branch's
//! budget, and those posts become branch rows ([`Row::catch_up`]).

use crate::commit::CommitId;
use crate::error::{Error, Result};
use crate::id::{BucketUid, IssuerIx, IssuerUid, LedgerIx, LedgerUid, TxUid, ViewUid};
use crate::model::{Parent, Transaction};
use crate::op::Op;
use crate::report::ChangeReport;
use crate::state::Budget;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// How a merge brings a branch's changes across.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeKind {
    /// The destination ends exactly like the branch.
    Replace,
    /// Row by row: keep, revert or drop each entry the two sides disagree on.
    Reconcile,
    /// Plans only: issuers and settings. No balance moves.
    Adopt,
}

impl MergeKind {
    pub const ALL: [MergeKind; 3] = [MergeKind::Replace, MergeKind::Reconcile, MergeKind::Adopt];

    pub fn name(self) -> &'static str {
        match self {
            MergeKind::Replace => "Replace",
            MergeKind::Reconcile => "Reconcile",
            MergeKind::Adopt => "Adopt",
        }
    }
}

/// Which branch a row comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// The branch being merged into: `main`.
    Destination,
    /// The branch being merged: `fix-sept`.
    Source,
}

/// Where a row falls, and so which choices it offers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Case {
    /// A destination entry on ledgers the branch left alone: Keep or Revert.
    DestinationOnly,
    /// A branch entry on ledgers the destination left alone: Keep or Drop.
    SourceOnly,
    /// An entry on a ledger both sides changed: decided by its group.
    Clash,
}

/// One entry the two sides disagree on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Row {
    pub side: Side,
    pub tx: Transaction,
    pub case: Case,
    /// Its clash group, for [`Case::Clash`].
    pub group: Option<usize>,
    /// Posted by the branch's issuers during the merge, to catch them up to
    /// the destination.
    pub catch_up: bool,
}

/// Ledgers both sides changed, linked by the rows that touch them, and the
/// rows to decide together.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClashGroup {
    pub ledgers: Vec<LedgerUid>,
    pub rows: Vec<usize>,
}

/// What happens to one row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowChoice {
    /// Destination: leave it. Branch: post it on the destination.
    Keep,
    /// Destination only: post its reversal.
    Revert,
    /// Branch only: leave it out.
    Drop,
}

/// What happens to a whole clash group.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GroupChoice {
    /// Keep the destination's and bring the branch's. May double-post.
    Force,
    /// Reverse the destination's and bring the branch's.
    Revert,
    /// Keep the destination's and leave the branch's out.
    Drop,
}

impl GroupChoice {
    fn for_side(self, side: Side) -> RowChoice {
        match (self, side) {
            (GroupChoice::Revert, Side::Destination) => RowChoice::Revert,
            (GroupChoice::Drop, Side::Source) => RowChoice::Drop,
            _ => RowChoice::Keep,
        }
    }
}

/// An issuer the branch has and the destination does not.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IssuerItem {
    pub uid: IssuerUid,
    pub name: String,
    /// Running destination issuers that post to the same set of ledgers -
    /// most likely the same bill, twice.
    pub clashes: Vec<IssuerUid>,
}

/// What happens to a branch issuer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IssuerChoice {
    /// Bring it. With a clash, both run: "keep both".
    Bring,
    /// Leave it on the branch. With a clash: "keep the destination's".
    Leave,
    /// Bring it and pause the destination issuers it clashes with.
    TakeBranch,
}

/// A setting the branch changed: a name, a goal, a view, a bucket's
/// members, a variable, an issuer's pause or an amount set ahead.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SettingItem {
    /// Stable within one preview, for choosing.
    pub key: String,
    pub label: String,
    /// The destination's value, and the branch's, in words.
    pub ours: String,
    pub theirs: String,
    /// The destination changed it too since the branches split.
    pub both_changed: bool,
    ops: Vec<Op>,
}

/// Everything a merge would do, before choosing.
#[derive(Clone, Debug)]
pub struct MergePreview {
    pub kind: MergeKind,
    /// The branch being merged, as named.
    pub source: String,
    pub source_tip: Option<CommitId>,
    pub dest_head: Option<CommitId>,
    /// The destination has not moved since the branch split: a Replace just
    /// moves it to the branch's tip.
    pub fast_forward: bool,
    pub rows: Vec<Row>,
    pub groups: Vec<ClashGroup>,
    pub issuers: Vec<IssuerItem>,
    pub settings: Vec<SettingItem>,
    dest: Budget,
    /// The branch, with its issuers caught up to the destination's.
    src: Budget,
}

/// The choices made on a preview. Anything not chosen takes its default:
/// Keep for rows outside a clash, Bring for an issuer that clashes with
/// nothing, and for a setting, the branch's value unless the destination
/// changed it too.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Choices {
    pub rows: HashMap<TxUid, RowChoice>,
    pub groups: HashMap<usize, GroupChoice>,
    pub issuers: HashMap<IssuerUid, IssuerChoice>,
    /// Setting key -> take the branch's value.
    pub settings: HashMap<String, bool>,
    /// Settle every undecided group and issuer the way Replace would.
    pub unresolved_as_replace: bool,
}

/// What still needs deciding.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Unresolved {
    pub groups: Vec<usize>,
    pub issuers: Vec<IssuerUid>,
}

impl Unresolved {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.issuers.is_empty()
    }
    pub fn len(&self) -> usize {
        self.groups.len() + self.issuers.len()
    }
}

/// Work out what merging `src` into `dest` involves, given the budget at
/// the commit they share, `base` (empty if they share none).
pub fn preview(
    kind: MergeKind,
    dest: &Budget,
    src: &Budget,
    base: &Budget,
    source: &str,
) -> MergePreview {
    let mut caught_up = src.clone();
    let catch_up = match kind {
        MergeKind::Adopt => HashSet::new(),
        _ => catch_up_issuers(dest, &mut caught_up),
    };
    let (rows, groups) = match kind {
        MergeKind::Adopt => (Vec::new(), Vec::new()),
        _ => rows(dest, &caught_up, &catch_up),
    };
    let issuers = match kind {
        MergeKind::Replace => Vec::new(),
        _ => issuer_items(dest, &caught_up),
    };
    let settings = settings(kind, dest, &caught_up, base);
    MergePreview {
        kind,
        source: source.to_string(),
        source_tip: None,
        dest_head: None,
        fast_forward: false,
        rows,
        groups,
        issuers,
        settings,
        dest: dest.clone(),
        src: caught_up,
    }
}

/// Run the branch's issuers up to wherever the destination has run them.
/// Returns the uids of the entries that posted.
fn catch_up_issuers(dest: &Budget, src: &mut Budget) -> HashSet<TxUid> {
    let mut needed: HashMap<IssuerIx, crate::date::Date> = HashMap::new();
    for six in src.issuers.indices() {
        let Some(dix) = dest.issuers.ix(src.issuers.uid[six.get()]) else { continue };
        let (Some(d), s) =
            (dest.issuers.emitted_through[dix.get()], src.issuers.emitted_through[six.get()])
        else {
            continue;
        };
        if !src.issuers.paused[six.get()] && s.is_none_or(|s| s < d) {
            needed.insert(six, d);
        }
    }
    let mut posted = HashSet::new();
    let Some(&through) = needed.values().max() else { return posted };
    for run in crate::issuer::run_all(src, through) {
        let Some(&until) = needed.get(&run.issuer) else { continue };
        let uid = src.issuers.uid[run.issuer.get()];
        let mut ops: Vec<Op> = run
            .ops
            .into_iter()
            .filter(|op| matches!(op, Op::PostTransaction { date, .. } if *date <= until))
            .collect();
        ops.push(Op::AdvanceIssuer { uid, through: until });
        for op in ops {
            if let Op::PostTransaction { uid, .. } = &op {
                posted.insert(*uid);
            }
            // A post that does not apply on the branch is simply not made.
            let _ = src.apply(&op);
        }
    }
    posted
}

/// The rows, sorted by date, and the clash groups among them.
fn rows(dest: &Budget, src: &Budget, catch_up: &HashSet<TxUid>) -> (Vec<Row>, Vec<ClashGroup>) {
    let live = |b: &Budget| -> Vec<Transaction> {
        b.live_transactions().into_iter().map(|ix| b.transaction(ix)).collect()
    };
    let (d_live, s_live) = (live(dest), live(src));
    let d_uids: HashSet<TxUid> = d_live.iter().map(|t| t.uid).collect();
    let s_uids: HashSet<TxUid> = s_live.iter().map(|t| t.uid).collect();

    let mut rows: Vec<Row> = Vec::new();
    for t in d_live.into_iter().filter(|t| !s_uids.contains(&t.uid)) {
        rows.push(Row {
            side: Side::Destination,
            tx: t,
            case: Case::DestinationOnly,
            group: None,
            catch_up: false,
        });
    }
    for t in s_live.into_iter().filter(|t| !d_uids.contains(&t.uid)) {
        let catch_up = catch_up.contains(&t.uid);
        rows.push(Row { side: Side::Source, tx: t, case: Case::SourceOnly, group: None, catch_up });
    }
    rows.sort_by(|a, b| {
        (a.tx.date, a.side == Side::Source, a.tx.name.to_lowercase(), a.tx.uid).cmp(&(
            b.tx.date,
            b.side == Side::Source,
            b.tx.name.to_lowercase(),
            b.tx.uid,
        ))
    });

    let touched = |side: Side| -> BTreeSet<LedgerUid> {
        rows.iter()
            .filter(|r| r.side == side)
            .flat_map(|r| r.tx.legs.iter().map(|g| g.ledger))
            .collect()
    };
    let clash: BTreeSet<LedgerUid> =
        touched(Side::Destination).intersection(&touched(Side::Source)).copied().collect();

    // Link clashing ledgers through the rows that touch more than one.
    let mut parent: BTreeMap<LedgerUid, LedgerUid> = clash.iter().map(|l| (*l, *l)).collect();
    fn find(p: &mut BTreeMap<LedgerUid, LedgerUid>, x: LedgerUid) -> LedgerUid {
        let up = p[&x];
        if up == x {
            return x;
        }
        let root = find(p, up);
        p.insert(x, root);
        root
    }
    for r in &rows {
        let mine: Vec<LedgerUid> =
            r.tx.legs.iter().map(|g| g.ledger).filter(|l| clash.contains(l)).collect();
        for pair in mine.windows(2) {
            let (a, b) = (find(&mut parent, pair[0]), find(&mut parent, pair[1]));
            if a != b {
                parent.insert(a.max(b), a.min(b));
            }
        }
    }
    let mut groups: Vec<ClashGroup> = Vec::new();
    let mut group_of: BTreeMap<LedgerUid, usize> = BTreeMap::new();
    for (k, r) in rows.iter_mut().enumerate() {
        let Some(first) = r.tx.legs.iter().map(|g| g.ledger).find(|l| clash.contains(l)) else {
            r.case = match r.side {
                Side::Destination => Case::DestinationOnly,
                Side::Source => Case::SourceOnly,
            };
            continue;
        };
        let root = find(&mut parent, first);
        let g = *group_of.entry(root).or_insert_with(|| {
            groups.push(ClashGroup { ledgers: Vec::new(), rows: Vec::new() });
            groups.len() - 1
        });
        r.case = Case::Clash;
        r.group = Some(g);
        groups[g].rows.push(k);
    }
    for l in &clash {
        let root = find(&mut parent, *l);
        if let Some(g) = group_of.get(&root) {
            groups[*g].ledgers.push(*l);
        }
    }
    (rows, groups)
}

/// Branch issuers the destination does not have, and what each clashes with.
fn issuer_items(dest: &Budget, src: &Budget) -> Vec<IssuerItem> {
    let ledgers = |b: &Budget, ix: IssuerIx| -> BTreeSet<LedgerUid> {
        b.issuers.legs[ix.get()].iter().map(|g| g.ledger).collect()
    };
    src.issuers
        .indices()
        .filter(|ix| dest.issuers.ix(src.issuers.uid[ix.get()]).is_none())
        .map(|six| {
            let mine = ledgers(src, six);
            let clashes = dest
                .issuers
                .indices()
                .filter(|d| !dest.issuers.paused[d.get()] && ledgers(dest, *d) == mine)
                .map(|d| dest.issuers.uid[d.get()])
                .collect();
            IssuerItem {
                uid: src.issuers.uid[six.get()],
                name: src.issuers.name[six.get()].clone(),
                clashes,
            }
        })
        .collect()
}

// ------------------------------------------------------------- settings

/// One setting's value on each side - in full, to compare, and in words,
/// to show - and the ops that set the destination's to the branch's.
struct Diff {
    key: String,
    label: String,
    base: Option<String>,
    dest: Option<String>,
    src: Option<String>,
    ours: Option<String>,
    theirs: Option<String>,
    ops: Vec<Op>,
}

impl Diff {
    /// A setting whose full value is also how it reads.
    fn plain(
        key: String,
        label: String,
        [base, dest, src]: [Option<String>; 3],
        ops: Vec<Op>,
    ) -> Diff {
        Diff { key, label, ours: dest.clone(), theirs: src.clone(), base, dest, src, ops }
    }
}

fn settings(kind: MergeKind, dest: &Budget, src: &Budget, base: &Budget) -> Vec<SettingItem> {
    let mut diffs: Vec<Diff> = Vec::new();
    ledger_diffs(dest, src, base, &mut diffs);
    issuer_diffs(kind, dest, src, base, &mut diffs);
    bucket_diffs(dest, src, base, &mut diffs);
    view_diffs(dest, src, base, &mut diffs);
    variable_diffs(dest, src, base, &mut diffs);
    diffs
        .into_iter()
        .filter(|d| d.dest != d.src && !d.ops.is_empty())
        // Replace makes the destination match in everything; the others
        // bring only what the branch changed.
        .filter(|d| kind == MergeKind::Replace || d.base != d.src)
        .map(|d| SettingItem {
            both_changed: d.base != d.dest,
            key: d.key,
            label: d.label,
            ours: d.ours.unwrap_or_else(|| "(none)".into()),
            theirs: d.theirs.unwrap_or_else(|| "(none)".into()),
            ops: d.ops,
        })
        .collect()
}

fn ledger_diffs(dest: &Budget, src: &Budget, base: &Budget, out: &mut Vec<Diff>) {
    for six in src.ledgers.indices() {
        let uid = src.ledgers.uid[six.get()];
        let Some(dix) = dest.ledgers.ix(uid) else { continue };
        let bix = base.ledgers.ix(uid);
        let label = &src.ledgers.name[six.get()];
        let three = |f: &dyn Fn(&Budget, LedgerIx) -> String| {
            [bix.map(|b| f(base, b)), Some(f(dest, dix)), Some(f(src, six))]
        };
        let name = |b: &Budget, ix: LedgerIx| b.ledgers.name[ix.get()].clone();
        let desc = |b: &Budget, ix: LedgerIx| b.ledgers.description[ix.get()].clone();
        out.push(Diff::plain(
            format!("ledger {uid} name"),
            format!("Ledger {label}: name"),
            three(&name),
            vec![Op::EditLedger { uid, name: Some(name(src, six)), description: None }],
        ));
        out.push(Diff::plain(
            format!("ledger {uid} description"),
            format!("Ledger {label}: description"),
            three(&desc),
            vec![Op::EditLedger { uid, name: None, description: Some(desc(src, six)) }],
        ));
        // A target and its alerts are set together, so they compare whole.
        let full = |b: &Budget, ix: LedgerIx| {
            format!("{:?} {:?}", b.ledgers.target[ix.get()], b.ledgers.alerts[ix.get()])
        };
        let words = |b: &Budget, ix: LedgerIx| {
            let t = b.ledgers.target[ix.get()].map_or("no target".to_string(), |t| t.to_string());
            format!("{t}, {} alert(s)", b.ledgers.alerts[ix.get()].len())
        };
        let [base_v, dest_v, src_v] = three(&full);
        out.push(Diff {
            key: format!("ledger {uid} goals"),
            label: format!("Ledger {label}: target and alerts"),
            base: base_v,
            dest: dest_v,
            src: src_v,
            ours: Some(words(dest, dix)),
            theirs: Some(words(src, six)),
            ops: vec![Op::SetLedgerGoals {
                uid,
                target: src.ledgers.target[six.get()],
                alerts: src.ledgers.alerts[six.get()].clone(),
            }],
        });
    }
}

fn issuer_diffs(kind: MergeKind, dest: &Budget, src: &Budget, base: &Budget, out: &mut Vec<Diff>) {
    for six in src.issuers.indices() {
        let uid = src.issuers.uid[six.get()];
        let Some(dix) = dest.issuers.ix(uid) else { continue };
        let bix = base.issuers.ix(uid);
        let name = &src.issuers.name[six.get()];
        let names = |b: &Budget, ix: IssuerIx| b.issuers.name[ix.get()].clone();
        let descs = |b: &Budget, ix: IssuerIx| b.issuers.description[ix.get()].clone();
        let paused = |b: &Budget, ix: IssuerIx| {
            if b.issuers.paused[ix.get()] { "paused" } else { "running" }.to_string()
        };
        out.push(Diff::plain(
            format!("issuer {uid} name"),
            format!("Issuer {name}: name"),
            [bix.map(|b| names(base, b)), Some(names(dest, dix)), Some(names(src, six))],
            vec![Op::EditIssuer { uid, name: Some(names(src, six)), description: None }],
        ));
        out.push(Diff::plain(
            format!("issuer {uid} description"),
            format!("Issuer {name}: description"),
            [bix.map(|b| descs(base, b)), Some(descs(dest, dix)), Some(descs(src, six))],
            vec![Op::EditIssuer { uid, name: None, description: Some(descs(src, six)) }],
        ));
        out.push(Diff::plain(
            format!("issuer {uid} paused"),
            format!("Issuer {name}"),
            [bix.map(|b| paused(base, b)), Some(paused(dest, dix)), Some(paused(src, six))],
            vec![Op::SetIssuerPaused { uid, paused: src.issuers.paused[six.get()] }],
        ));
        // Amounts set ahead, one date at a time, for dates the destination
        // still owes.
        let owed = |date: crate::date::Date| {
            dest.issuers.emitted_through[dix.get()].is_none_or(|done| date > done)
        };
        let dates: BTreeSet<crate::date::Date> = src.issuers.overrides[six.get()]
            .iter()
            .chain(&dest.issuers.overrides[dix.get()])
            .map(|(d, _)| *d)
            .filter(|d| owed(*d))
            .collect();
        for date in dates {
            let at =
                |b: &Budget, ix: IssuerIx| b.issuers.override_on(ix, date).map(|m| m.to_string());
            let theirs = src.issuers.override_on(six, date);
            out.push(Diff::plain(
                format!("issuer {uid} amount {date}"),
                format!("Issuer {name}: amount on {date}"),
                [bix.and_then(|b| at(base, b)), at(dest, dix), at(src, six)],
                vec![Op::SetIssuerOverride { uid, date, amount: theirs }],
            ));
        }
    }
    // Replace pauses what only the destination runs: it cannot be deleted.
    if kind == MergeKind::Replace {
        for dix in dest.issuers.indices() {
            let uid = dest.issuers.uid[dix.get()];
            if src.issuers.ix(uid).is_none() && !dest.issuers.paused[dix.get()] {
                out.push(Diff::plain(
                    format!("issuer {uid} paused"),
                    format!("Issuer {}", dest.issuers.name[dix.get()]),
                    [None, Some("running".into()), Some("paused (not on the branch)".into())],
                    vec![Op::SetIssuerPaused { uid, paused: true }],
                ));
            }
        }
    }
}

fn bucket_diffs(dest: &Budget, src: &Budget, base: &Budget, out: &mut Vec<Diff>) {
    // Members as uids, so the same bucket compares equal across budgets.
    let state = |b: &Budget,
                 uid: BucketUid|
     -> Option<(String, String, BTreeSet<LedgerUid>, BTreeSet<String>)> {
        let ix = b.buckets.ix(uid)?.get();
        Some((
            b.buckets.name[ix].clone(),
            b.buckets.description[ix].clone(),
            b.buckets.explicit[ix].iter().map(|l| b.ledgers.uid[l.get()]).collect(),
            b.buckets.subtrees[ix].iter().map(|p| p.to_lowercase()).collect(),
        ))
    };
    let words = |s: &Option<(String, String, BTreeSet<LedgerUid>, BTreeSet<String>)>| {
        s.as_ref().map(|(n, _, m, t)| format!("{n}: {} ledger(s), {} subtree(s)", m.len(), t.len()))
    };
    let all: BTreeSet<BucketUid> = [src, dest]
        .iter()
        .flat_map(|b| b.buckets.live().map(|ix| b.buckets.uid[ix.get()]))
        .collect();
    for uid in all {
        let (d, s, b) = (state(dest, uid), state(src, uid), state(base, uid));
        if d == s {
            continue;
        }
        let name = s.as_ref().or(d.as_ref()).map(|x| x.0.clone()).unwrap_or_default();
        let mut ops = Vec::new();
        match (&d, &s) {
            (Some(_), None) => ops.push(Op::DeleteBucket { uid }),
            (dv, Some((n, desc, members, trees))) => {
                let (dm, dt) = match dv {
                    Some((dn, dd, dm, dt)) => {
                        if dn != n || dd != desc {
                            ops.push(Op::EditBucket {
                                uid,
                                name: (dn != n).then(|| n.clone()),
                                description: (dd != desc).then(|| desc.clone()),
                            });
                        }
                        (dm.clone(), dt.clone())
                    }
                    None => {
                        ops.push(Op::CreateBucket {
                            uid,
                            name: n.clone(),
                            description: desc.clone(),
                        });
                        (BTreeSet::new(), BTreeSet::new())
                    }
                };
                for l in members.difference(&dm) {
                    ops.push(Op::AddToBucket { bucket: uid, ledger: *l });
                }
                for l in dm.difference(members) {
                    ops.push(Op::RemoveFromBucket { bucket: uid, ledger: *l });
                }
                let six = src.buckets.ix(uid).expect("on the branch").get();
                for p in &src.buckets.subtrees[six] {
                    if !dt.contains(&p.to_lowercase()) {
                        ops.push(Op::AddSubtreeToBucket { bucket: uid, path: p.clone() });
                    }
                }
                for p in dt.difference(trees) {
                    ops.push(Op::RemoveSubtreeFromBucket { bucket: uid, path: p.clone() });
                }
            }
            (None, None) => {}
        }
        // Compared in full, shown in words.
        let full = |v: &Option<(String, String, BTreeSet<LedgerUid>, BTreeSet<String>)>| {
            v.as_ref().map(|v| format!("{v:?}"))
        };
        out.push(Diff {
            key: format!("bucket {uid}"),
            label: format!("Bucket {name}"),
            base: full(&b),
            dest: full(&d),
            src: full(&s),
            ours: words(&d),
            theirs: words(&s),
            ops,
        });
    }
}

fn view_diffs(dest: &Budget, src: &Budget, base: &Budget, out: &mut Vec<Diff>) {
    let state = |b: &Budget, uid: ViewUid| {
        let ix = b.views.ix(uid)?.get();
        Some((b.views.name[ix].clone(), b.views.description[ix].clone(), b.views.spec[ix].clone()))
    };
    let all: BTreeSet<ViewUid> =
        [src, dest].iter().flat_map(|b| b.views.live().map(|ix| b.views.uid[ix.get()])).collect();
    for uid in all {
        let (d, s, b) = (state(dest, uid), state(src, uid), state(base, uid));
        if d == s {
            continue;
        }
        let words = |v: &Option<(String, String, crate::model::ViewSpec)>| {
            v.as_ref().map(|(n, desc, spec)| format!("{n} {desc} {spec:?}"))
        };
        let shown = |v: &Option<(String, String, crate::model::ViewSpec)>| {
            v.as_ref().map(|(n, ..)| n.clone()).unwrap_or_else(|| "(none)".into())
        };
        let name = s.as_ref().or(d.as_ref()).map(|v| v.0.clone()).unwrap_or_default();
        let ops = match (&d, &s) {
            (Some(_), None) => vec![Op::DeleteView { uid }],
            (None, Some((n, desc, spec))) => vec![Op::CreateView {
                uid,
                name: n.clone(),
                description: desc.clone(),
                spec: spec.clone(),
            }],
            (Some((dn, dd, dspec)), Some((n, desc, spec))) => vec![Op::EditView {
                uid,
                name: (dn != n).then(|| n.clone()),
                description: (dd != desc).then(|| desc.clone()),
                spec: (dspec != spec).then(|| spec.clone()),
            }],
            (None, None) => Vec::new(),
        };
        let edited = d.is_some() && s.is_some() && shown(&d) == shown(&s);
        out.push(Diff {
            key: format!("view {uid}"),
            label: if edited {
                format!("View {name}: what it looks at")
            } else {
                format!("View {name}")
            },
            base: words(&b),
            dest: words(&d),
            src: words(&s),
            ours: d.as_ref().map(|_| shown(&d)),
            theirs: s.as_ref().map(|_| shown(&s)),
            ops,
        });
    }
}

fn variable_diffs(dest: &Budget, src: &Budget, base: &Budget, out: &mut Vec<Diff>) {
    let value = |b: &Budget, name: &str| b.variables.get(name).map(|v| v.to_string());
    let names: BTreeSet<String> = [src, dest]
        .iter()
        .flat_map(|b| b.variables.name.iter().map(|n| n.to_lowercase()))
        .collect();
    for lower in names {
        let display = src
            .variables
            .name
            .iter()
            .chain(&dest.variables.name)
            .find(|n| n.to_lowercase() == lower)
            .cloned()
            .unwrap_or(lower.clone());
        let ops = match src.variables.get(&display) {
            Some(v) => vec![Op::SetVariable { name: display.clone(), value: v.clone() }],
            None => vec![Op::DeleteVariable { name: display.clone() }],
        };
        out.push(Diff::plain(
            format!("variable {lower}"),
            format!("Variable {display}"),
            [value(base, &display), value(dest, &display), value(src, &display)],
            ops,
        ));
    }
}

// ---------------------------------------------------------------- choose

impl MergePreview {
    /// The budget being merged into, as committed.
    pub fn destination(&self) -> &Budget {
        &self.dest
    }

    /// The branch, with its issuers caught up to the destination.
    pub fn branch(&self) -> &Budget {
        &self.src
    }

    /// The row an entry is in, if any.
    pub fn row(&self, tx: TxUid) -> Option<&Row> {
        self.rows.iter().find(|r| r.tx.uid == tx)
    }

    /// What a row ends up as under `choices`, or `None` while undecided.
    pub fn choice(&self, k: usize, choices: &Choices) -> Option<RowChoice> {
        let r = &self.rows[k];
        if self.kind == MergeKind::Replace {
            return Some(replace_rule(r.side));
        }
        if let Some(c) = choices.rows.get(&r.tx.uid) {
            return Some(*c);
        }
        match r.case {
            Case::DestinationOnly | Case::SourceOnly => Some(RowChoice::Keep),
            Case::Clash => match r.group.and_then(|g| choices.groups.get(&g)) {
                Some(g) => Some(g.for_side(r.side)),
                None if choices.unresolved_as_replace => Some(replace_rule(r.side)),
                None => None,
            },
        }
    }

    /// What a branch issuer ends up as under `choices`.
    pub fn issuer_choice(&self, item: &IssuerItem, choices: &Choices) -> Option<IssuerChoice> {
        match choices.issuers.get(&item.uid) {
            Some(c) => Some(*c),
            None if item.clashes.is_empty() => Some(IssuerChoice::Bring),
            None if choices.unresolved_as_replace => Some(IssuerChoice::TakeBranch),
            None => None,
        }
    }

    /// Whether a setting takes the branch's value under `choices`.
    pub fn takes(&self, item: &SettingItem, choices: &Choices) -> bool {
        if self.kind == MergeKind::Replace {
            return true;
        }
        choices.settings.get(&item.key).copied().unwrap_or(!item.both_changed)
    }

    /// The clash groups and issuers still to decide.
    pub fn unresolved(&self, choices: &Choices) -> Unresolved {
        let groups = (0..self.groups.len())
            .filter(|g| self.groups[*g].rows.iter().any(|k| self.choice(*k, choices).is_none()))
            .collect();
        let issuers = self
            .issuers
            .iter()
            .filter(|i| self.issuer_choice(i, choices).is_none())
            .map(|i| i.uid)
            .collect();
        Unresolved { groups, issuers }
    }

    /// Nothing differs: there is nothing to merge.
    pub fn is_empty(&self) -> bool {
        !self.fast_forward
            && self.rows.is_empty()
            && self.issuers.is_empty()
            && self.settings.is_empty()
            && (self.kind != MergeKind::Replace || self.new_branch_issuers().is_empty())
    }

    fn new_branch_issuers(&self) -> Vec<IssuerIx> {
        self.src
            .issuers
            .indices()
            .filter(|ix| self.dest.issuers.ix(self.src.issuers.uid[ix.get()]).is_none())
            .collect()
    }

    /// The ops the merge commit will hold, in an order that applies.
    pub fn ops(&self, choices: &Choices) -> Result<Vec<Op>> {
        let unresolved = self.unresolved(choices);
        if !unresolved.is_empty() {
            return Err(Error::Invalid(format!(
                "{} clash(es) still to decide before merging",
                unresolved.len()
            )));
        }
        let (dest, src) = (&self.dest, &self.src);
        let mut ops: Vec<Op> = Vec::new();

        // Replace brings every ledger the branch has.
        if self.kind == MergeKind::Replace {
            for ix in src.ledgers.indices() {
                if dest.ledgers.ix(src.ledgers.uid[ix.get()]).is_none() {
                    ops.push(create_ledger(src, ix));
                }
            }
        }

        // Issuers the branch has and the destination does not.
        let mut brought: HashSet<IssuerUid> = HashSet::new();
        let mut pause: Vec<IssuerUid> = Vec::new();
        let bring_all = self.kind == MergeKind::Replace;
        for six in self.new_branch_issuers() {
            let uid = src.issuers.uid[six.get()];
            let choice = match self.issuers.iter().find(|i| i.uid == uid) {
                _ if bring_all => IssuerChoice::Bring,
                Some(item) => self.issuer_choice(item, choices).expect("resolved above"),
                None => IssuerChoice::Bring,
            };
            if choice == IssuerChoice::Leave {
                continue;
            }
            if choice == IssuerChoice::TakeBranch {
                let item = self.issuers.iter().find(|i| i.uid == uid).expect("a clash has an item");
                pause.extend(item.clashes.iter().copied());
            }
            brought.insert(uid);
            ops.extend(self.create_issuer(six));
        }

        // Rows.
        for (k, r) in self.rows.iter().enumerate() {
            let choice = self.choice(k, choices).expect("resolved above");
            match (r.side, choice) {
                (Side::Destination, RowChoice::Keep) | (Side::Source, RowChoice::Drop) => {}
                (Side::Destination, RowChoice::Revert) => ops.push(Op::reversal(
                    r.tx.uid,
                    &r.tx.name,
                    r.tx.date,
                    &r.tx.legs,
                    format!(
                        "Reverses transaction {} in a merge from {}",
                        r.tx.uid.short(),
                        self.source
                    ),
                )),
                (Side::Destination, RowChoice::Drop) => {
                    return Err(Error::Invalid(format!(
                        "\"{}\" is already on the destination: keep or revert it",
                        r.tx.name
                    )))
                }
                (Side::Source, RowChoice::Revert) => {
                    return Err(Error::Invalid(format!(
                        "\"{}\" comes from the branch: keep or drop it",
                        r.tx.name
                    )))
                }
                (Side::Source, RowChoice::Keep) => {
                    ops.push(self.import(&r.tx, &brought)?);
                }
            }
        }

        // Issuers on both sides: as far along as either has got.
        if self.kind != MergeKind::Adopt {
            for six in src.issuers.indices() {
                let uid = src.issuers.uid[six.get()];
                let (Some(dix), Some(s)) =
                    (dest.issuers.ix(uid), src.issuers.emitted_through[six.get()])
                else {
                    continue;
                };
                if dest.issuers.emitted_through[dix.get()].is_none_or(|d| d < s) {
                    ops.push(Op::AdvanceIssuer { uid, through: s });
                }
            }
        }

        // Settings.
        for item in &self.settings {
            if self.takes(item, choices) {
                ops.extend(item.ops.iter().cloned());
            }
        }
        for uid in pause {
            ops.push(Op::SetIssuerPaused { uid, paused: true });
        }
        self.order(ops)
    }

    /// The pre-commit report for the merge as chosen.
    pub fn report(&self, choices: &Choices) -> Result<ChangeReport> {
        crate::report::build(&self.dest, &self.ops(choices)?)
    }

    /// A branch issuer, recreated: same uid, its state on the branch, and
    /// - except when adopting a plan - as far along as the branch ran it.
    fn create_issuer(&self, six: IssuerIx) -> Vec<Op> {
        let (src, i) = (&self.src, six.get());
        let uid = src.issuers.uid[i];
        let mut ops = vec![Op::CreateIssuer {
            uid,
            name: src.issuers.name[i].clone(),
            description: src.issuers.description[i].clone(),
            legs: src.issuers.legs[i].clone(),
            schedule: src.issuers.schedule[i],
            start: src.issuers.start[i],
            rule: src.issuers.rule[i],
            settles: src.issuers.settles[i],
        }];
        if src.issuers.paused[i] {
            ops.push(Op::SetIssuerPaused { uid, paused: true });
        }
        let done = match self.kind {
            MergeKind::Adopt => None,
            _ => src.issuers.emitted_through[i],
        };
        for (date, amount) in &src.issuers.overrides[i] {
            if done.is_none_or(|d| *date > d) {
                ops.push(Op::SetIssuerOverride { uid, date: *date, amount: Some(*amount) });
            }
        }
        if let Some(through) = done {
            ops.push(Op::AdvanceIssuer { uid, through });
        }
        ops
    }

    /// A branch entry, posted on the destination under its own uid. If the
    /// destination has it but reversed it, the reversal is reversed instead,
    /// which brings it back.
    fn import(&self, t: &Transaction, brought: &HashSet<IssuerUid>) -> Result<Op> {
        if let Some(dix) = self.dest.transactions.ix(t.uid) {
            let r = self.dest.reversed_by(dix).ok_or_else(|| {
                Error::Invalid(format!("\"{}\" is on both sides already", t.name))
            })?;
            let rt = self.dest.transaction(r);
            return Ok(Op::reversal(
                rt.uid,
                &rt.name,
                rt.date,
                &rt.legs,
                format!("Brings back {} in a merge from {}", t.uid.short(), self.source),
            ));
        }
        // Posted by an issuer that stays behind: it is an ordinary entry now.
        let parent = match t.parent {
            Parent::Issuer(i) if self.dest.issuers.ix(i).is_none() && !brought.contains(&i) => {
                Parent::Manual
            }
            p => p,
        };
        Ok(Op::PostTransaction {
            uid: t.uid,
            name: t.name.clone(),
            description: t.description.clone(),
            date: t.date,
            legs: t.legs.clone(),
            parent,
            reverses: None,
        })
    }

    /// Put `ops` in an order that applies to the destination: each pass
    /// applies whatever it can, creating from the branch any ledger an op
    /// needs. Fails, naming the first op, if a pass makes no progress.
    fn order(&self, ops: Vec<Op>) -> Result<Vec<Op>> {
        let mut b = self.dest.clone();
        let mut out = Vec::with_capacity(ops.len());
        let mut left = ops;
        loop {
            let mut next = Vec::new();
            let mut progress = false;
            let mut first_err: Option<(String, Error)> = None;
            for op in left {
                match b.apply(&op) {
                    Ok(()) => {
                        out.push(op);
                        progress = true;
                    }
                    Err(Error::NoSuchEntity { kind: "ledger", uid })
                        if self.src.ledgers.ix(LedgerUid(uid)).is_some() =>
                    {
                        let create = create_ledger(
                            &self.src,
                            self.src.ledgers.ix(LedgerUid(uid)).expect("checked"),
                        );
                        b.apply(&create)?;
                        out.push(create);
                        progress = true;
                        next.push(op);
                    }
                    Err(e) => {
                        if first_err.is_none() {
                            first_err = Some((op.summary(), e));
                        }
                        next.push(op);
                    }
                }
            }
            if next.is_empty() {
                return Ok(out);
            }
            if !progress {
                let (what, e) = first_err.expect("an op failed");
                return Err(Error::Invalid(format!("the merge cannot {what}: {e}")));
            }
            left = next;
        }
    }
}

fn replace_rule(side: Side) -> RowChoice {
    match side {
        Side::Destination => RowChoice::Revert,
        Side::Source => RowChoice::Keep,
    }
}

fn create_ledger(b: &Budget, ix: LedgerIx) -> Op {
    let i = ix.get();
    Op::CreateLedger {
        uid: b.ledgers.uid[i],
        name: b.ledgers.name[i].clone(),
        description: b.ledgers.description[i].clone(),
        normality: b.ledgers.normality[i],
        opened: b.ledgers.opened[i],
    }
}
