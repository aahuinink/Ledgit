//! The ledger tree: hierarchy read out of names, the way hledger does it.
//!
//! A ledger called `Wedding:Tuxedo` sits under `Wedding`. That is all a tree
//! is here - there is no parent field and no op that sets one. The hierarchy
//! is *derived* from names every time it is needed, which buys three things:
//!
//! * **No graph in the op log.** A parent pointer would have to be checked for
//!   cycles on every replay; a prefix of a string cannot form a cycle.
//! * **Parents need not exist.** `Wedding` is a node because `Wedding:Tuxedo`
//!   names it. It holds no money of its own unless a ledger is actually called
//!   `Wedding`, which is also allowed.
//! * **Reorganising is renaming.** Moving a subtree is a batch of
//!   `EditLedger` ops ([`rename_ops`]); every balance and posting stays where
//!   it was.
//!
//! Paths compare case-insensitively (ASCII), matching how ledgers are looked
//! up by name everywhere else.

use crate::id::LedgerIx;
use crate::model::Normality;
use crate::money::Money;
use crate::op::Op;
use crate::state::Budget;
use std::cmp::Ordering;
use std::ops::Range;

/// What separates the levels of a path.
pub const SEP: char = ':';

/// Tidy a ledger name into a path: trim each segment and drop empty ones, so
/// `" Wedding : Tuxedo "` and `"Wedding::Tuxedo"` both become
/// `"Wedding:Tuxedo"`. Returns an empty string for a name with no segments.
///
/// This normalises rather than rejects, deliberately. It runs on replay, and
/// a budget created before paths existed may hold a name with a stray colon;
/// refusing it would make that file unloadable.
pub fn normalize(name: &str) -> String {
    name.split(SEP).map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(":")
}

/// Whether `name` is `path` itself or anything below it. `Wedding` covers
/// `Wedding` and `Wedding:Tuxedo`, but not `Weddings` or `Wedding-fund`.
pub fn is_under(name: &str, path: &str) -> bool {
    let (n, p) = (name.as_bytes(), path.as_bytes());
    !p.is_empty()
        && n.len() >= p.len()
        && n[..p.len()].eq_ignore_ascii_case(p)
        && (n.len() == p.len() || n[p.len()] == SEP as u8)
}

/// Order two paths level by level, so every subtree is contiguous.
///
/// Plain string order is not enough: `-` sorts before `:`, which would put
/// `Wedding-fund` between `Wedding` and `Wedding:Tuxedo` and split the subtree.
pub fn path_cmp(a: &str, b: &str) -> Ordering {
    let mut x = a.split(SEP);
    let mut y = b.split(SEP);
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(s), Some(t)) => {
                let o = s.to_ascii_lowercase().cmp(&t.to_ascii_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

/// The last segment of a path: `Tuxedo` for `Wedding:Tuxedo`.
pub fn leaf(path: &str) -> &str {
    path.rsplit(SEP).next().unwrap_or(path)
}

/// One node of the tree: a real ledger, or a level that only exists because
/// ledgers below it name it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Node {
    /// The full path to this node.
    pub path: String,
    /// 0 for a top-level node.
    pub depth: u32,
    /// The ledger with exactly this path, if there is one.
    pub ledger: Option<LedgerIx>,
    /// This node's subtree, itself included, as a range of
    /// [`LedgerTree::order`].
    pub ledgers: Range<u32>,
    /// The index of the first node after this subtree in
    /// [`LedgerTree::nodes`]; the subtree is `self_index..end`.
    pub end: u32,
}

impl Node {
    pub fn name(&self) -> &str {
        leaf(&self.path)
    }
}

/// Every ledger arranged by path, with the implied levels filled in.
///
/// Both arrays are in depth-first order. A subtree is a contiguous range of
/// each, so totalling `Wedding` is a sum over one slice of row numbers - no
/// recursion, no per-node allocation.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct LedgerTree {
    /// Every ledger, sorted by path.
    pub order: Vec<LedgerIx>,
    /// Every node, depth first.
    pub nodes: Vec<Node>,
}

impl LedgerTree {
    pub fn build(l: &Budget) -> LedgerTree {
        let names = &l.ledgers.name;
        let mut order: Vec<LedgerIx> = l.ledgers.indices().collect();
        order.sort_by(|a, b| path_cmp(&names[a.get()], &names[b.get()]).then(a.0.cmp(&b.0)));

        let mut nodes: Vec<Node> = Vec::new();
        // The chain of open ancestors: `open[d]` is the node at depth `d`.
        let mut open: Vec<usize> = Vec::new();

        for (k, ix) in order.iter().enumerate() {
            let k = k as u32;
            let segs: Vec<&str> = names[ix.get()].split(SEP).collect();
            // Close every open node that is not a strict ancestor of this one.
            while let Some(&top) = open.last() {
                let depth = nodes[top].depth as usize;
                let ancestor = depth + 1 < segs.len()
                    && nodes[top]
                        .path
                        .split(SEP)
                        .zip(&segs)
                        .all(|(a, b)| a.eq_ignore_ascii_case(b));
                if ancestor {
                    break;
                }
                let n = nodes.len();
                close(&mut nodes[top], k, n);
                open.pop();
            }
            // Fill in any levels nobody has named as a ledger yet.
            for d in open.len()..segs.len() - 1 {
                open.push(nodes.len());
                nodes.push(Node {
                    path: segs[..=d].join(":"),
                    depth: d as u32,
                    ledger: None,
                    ledgers: k..k,
                    end: 0,
                });
            }
            open.push(nodes.len());
            nodes.push(Node {
                path: names[ix.get()].clone(),
                depth: (segs.len() - 1) as u32,
                ledger: Some(*ix),
                ledgers: k..k,
                end: 0,
            });
        }
        let (k, n) = (order.len() as u32, nodes.len());
        for top in open.into_iter().rev() {
            close(&mut nodes[top], k, n);
        }
        LedgerTree { order, nodes }
    }

    /// The ledgers in a node's subtree, itself included.
    pub fn subtree(&self, node: usize) -> &[LedgerIx] {
        let r = &self.nodes[node].ledgers;
        &self.order[r.start as usize..r.end as usize]
    }

    /// A node's direct children.
    pub fn children(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        let depth = self.nodes[node].depth + 1;
        let end = self.nodes[node].end as usize;
        let mut i = node + 1;
        std::iter::from_fn(move || {
            while i < end {
                let here = i;
                i = self.nodes[here].end as usize;
                if self.nodes[here].depth == depth {
                    return Some(here);
                }
            }
            None
        })
    }

    /// The top-level nodes.
    pub fn roots(&self) -> impl Iterator<Item = usize> + '_ {
        let mut i = 0;
        std::iter::from_fn(move || {
            let here = i;
            (here < self.nodes.len()).then(|| {
                i = self.nodes[here].end as usize;
                here
            })
        })
    }

    /// The first node with exactly this path.
    pub fn find(&self, path: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.path.eq_ignore_ascii_case(path))
    }

    /// A subtree's total, and the normality it is presented in.
    ///
    /// If every ledger in it shares a normality, the total reads the way each
    /// of them does - a tree of expenses totals as a positive spend. If they
    /// are mixed (a wedding's costs and the gifts that paid for them), it is
    /// the net, debit-positive, exactly as a `ByNormality` bucket reads, and
    /// the normality is `None`.
    pub fn total(&self, l: &Budget, node: usize) -> (Money, Option<Normality>) {
        let members = self.subtree(node);
        let raw: Money = members.iter().map(|ix| l.ledgers.raw_balance[ix.get()]).sum();
        let mut kinds = members.iter().map(|ix| l.ledgers.normality[ix.get()]);
        match kinds.next() {
            Some(first) if kinds.all(|n| n == first) => (first.present(raw), Some(first)),
            Some(_) => (raw, None),
            None => (Money::ZERO, None),
        }
    }
}

fn close(node: &mut Node, ledger_end: u32, node_end: usize) {
    node.ledgers.end = ledger_end;
    node.end = node_end as u32;
}

/// The ops that move everything under `from` to sit under `to`.
///
/// Every ledger at or below `from` is renamed, and every bucket that includes
/// a subtree at or below `from` is re-pointed - otherwise renaming `Wedding`
/// would quietly empty the bucket that tracks it. Nothing else changes: uids,
/// balances and postings all stay put.
pub fn rename_ops(l: &Budget, from: &str, to: &str) -> Vec<Op> {
    let (from, to) = (normalize(from), normalize(to));
    let moved = |path: &str| format!("{to}{}", &path[from.len()..]);
    let mut ops: Vec<Op> = l
        .ledgers
        .indices()
        .filter(|ix| is_under(&l.ledgers.name[ix.get()], &from))
        .map(|ix| Op::EditLedger {
            uid: l.ledgers.uid[ix.get()],
            name: Some(moved(&l.ledgers.name[ix.get()])),
            description: None,
        })
        .collect();
    for b in l.buckets.live() {
        for path in &l.buckets.subtrees[b.get()] {
            if is_under(path, &from) {
                let bucket = l.buckets.uid[b.get()];
                ops.push(Op::RemoveSubtreeFromBucket { bucket, path: path.clone() });
                ops.push(Op::AddSubtreeToBucket { bucket, path: moved(path) });
            }
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::Date;
    use crate::id::{BucketUid, LedgerUid};

    fn budget(names: &[&str]) -> Budget {
        let ops: Vec<Op> = names
            .iter()
            .map(|n| Op::CreateLedger {
                uid: LedgerUid::new(),
                name: n.to_string(),
                description: String::new(),
                normality: Normality::Debit,
                opened: Date::from_ymd(2024, 1, 1).unwrap(),
            })
            .collect();
        Budget::replay(&ops).unwrap()
    }

    fn paths(t: &LedgerTree) -> Vec<(u32, &str, bool)> {
        t.nodes.iter().map(|n| (n.depth, n.path.as_str(), n.ledger.is_some())).collect()
    }

    #[test]
    fn names_are_tidied_into_paths() {
        assert_eq!(normalize(" Wedding : Tuxedo "), "Wedding:Tuxedo");
        assert_eq!(normalize("Wedding::Tuxedo:"), "Wedding:Tuxedo");
        assert_eq!(normalize(" : "), "");
        assert_eq!(normalize("Cash"), "Cash");
    }

    #[test]
    fn under_means_the_path_or_below_it_on_a_segment_boundary() {
        assert!(is_under("Wedding", "wedding"));
        assert!(is_under("Wedding:Tuxedo", "Wedding"));
        assert!(is_under("Wedding:Tuxedo:Rental", "wedding:tuxedo"));
        assert!(!is_under("Weddings", "Wedding"));
        assert!(!is_under("Wedding-fund", "Wedding"));
        assert!(!is_under("Wed", "Wedding"));
        assert!(!is_under("Wedding", ""));
    }

    #[test]
    fn the_tree_fills_in_levels_nobody_named() {
        let l = budget(&["Wedding:Venue", "Cash", "Wedding:Tuxedo:Rental", "Wedding-fund"]);
        let t = LedgerTree::build(&l);
        assert_eq!(
            paths(&t),
            vec![
                (0, "Cash", true),
                (0, "Wedding", false),
                (1, "Wedding:Tuxedo", false),
                (2, "Wedding:Tuxedo:Rental", true),
                (1, "Wedding:Venue", true),
                (0, "Wedding-fund", true),
            ]
        );
        let wedding = t.find("wedding").unwrap();
        assert_eq!(t.subtree(wedding).len(), 2, "Wedding-fund is not under Wedding");
        assert_eq!(t.nodes[wedding].end, 5);
        let kids: Vec<&str> = t.children(wedding).map(|n| t.nodes[n].name()).collect();
        assert_eq!(kids, ["Tuxedo", "Venue"]);
        let roots: Vec<&str> = t.roots().map(|n| t.nodes[n].name()).collect();
        assert_eq!(roots, ["Cash", "Wedding", "Wedding-fund"]);
    }

    #[test]
    fn a_ledger_may_be_a_parent_too() {
        let l = budget(&["Wedding:Tuxedo", "Wedding"]);
        let t = LedgerTree::build(&l);
        assert_eq!(paths(&t), vec![(0, "Wedding", true), (1, "Wedding:Tuxedo", true)]);
        assert_eq!(t.subtree(0).len(), 2);
    }

    #[test]
    fn two_ledgers_may_share_a_path() {
        let l = budget(&["A:B", "A:B", "A:C"]);
        let t = LedgerTree::build(&l);
        assert_eq!(
            paths(&t),
            vec![(0, "A", false), (1, "A:B", true), (1, "A:B", true), (1, "A:C", true)]
        );
        assert_eq!(t.subtree(0).len(), 3);
        assert_eq!(t.children(0).count(), 3);
    }

    #[test]
    fn renaming_a_subtree_carries_its_buckets_along() {
        let mut l = budget(&["Wedding:Venue", "Wedding:Tuxedo", "Cash"]);
        let b = BucketUid::new();
        l.apply(&Op::CreateBucket { uid: b, name: "Big day".into(), description: String::new() })
            .unwrap();
        l.apply(&Op::AddSubtreeToBucket { bucket: b, path: "Wedding".into() }).unwrap();
        for op in rename_ops(&l, "Wedding", "Marriage:Ceremony") {
            l.apply(&op).unwrap();
        }
        let t = LedgerTree::build(&l);
        let m = t.find("Marriage:Ceremony").unwrap();
        assert_eq!(t.subtree(m).len(), 2);
        let bix = l.buckets.ix(b).unwrap().get();
        assert_eq!(l.buckets.subtrees[bix], vec!["Marriage:Ceremony".to_string()]);
        assert_eq!(l.buckets.members[bix].len(), 2, "the bucket followed the rename");
    }
}
