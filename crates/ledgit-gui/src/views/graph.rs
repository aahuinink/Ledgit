//! The commit graph on the History screen: every branch at once, drawn as
//! lanes the way `git log --graph` draws them, each branch in its own colour.
//!
//! Layout is a pure function of the commits and the branch tips, worked out
//! once per change to either and kept on the session; drawing is per row, so
//! only the rows on screen cost anything.

use crate::fmt;
use crate::views::saved::series_colour;
use egui::{Color32, RichText, Sense, Stroke, Ui};
use ledgit_core::prelude::*;
use ledgit_plot::MAX_SERIES;
use std::collections::HashMap;

/// One row of the graph: where its commit sits, and the line pieces that
/// cross the row - from its top edge to its middle (`up`), and from its
/// middle to its bottom edge (`down`). Each piece is `(from lane, to lane,
/// branch, dashed)`; a dashed piece leads from a merge to the branch tip it
/// brought changes from - not a parent, so not a solid line. Keeping pieces
/// inside their row is what lets rows be drawn on their own, off-screen ones
/// not at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub lane: usize,
    pub branch: Option<usize>,
    pub up: Vec<Piece>,
    pub down: Vec<Piece>,
}

/// `(from lane, to lane, branch colour, dashed)`.
pub type Piece = (usize, usize, Option<usize>, bool);

/// What a lane is waiting for, the colour of the line leading there, and
/// whether that line is a merge's dashed one.
type Lane = Option<(CommitId, Option<usize>, bool)>;

pub struct Graph {
    /// Newest first, children before parents: `Repo::graph`.
    pub commits: Vec<Commit>,
    pub rows: Vec<Row>,
    /// Branch names, in the order their colours are assigned.
    pub branches: Vec<(String, CommitId)>,
    /// The widest the graph gets, in lanes.
    pub lanes: usize,
    pub head: Option<CommitId>,
}

impl Graph {
    pub fn build(repo: &Repo<ledgit_sqlite::SqliteStore>, limit: usize) -> Result<Graph> {
        let commits = repo.graph(Some(limit))?;
        let branches = repo.branches()?;
        let head = repo.head_commit()?;
        let rows = layout(&commits, &branches);
        let lanes = rows.iter().map(lanes_used).max().unwrap_or(1);
        Ok(Graph { commits, rows, branches, lanes, head })
    }

    /// What decides whether a cached graph is stale: the tips and HEAD.
    pub fn key(repo: &Repo<ledgit_sqlite::SqliteStore>) -> String {
        format!("{:?} {:?}", repo.branches().unwrap_or_default(), repo.head_commit().ok().flatten())
    }

    /// Branches whose tip is this commit.
    pub fn tips(&self, id: CommitId) -> impl Iterator<Item = (usize, &str)> {
        self.branches
            .iter()
            .enumerate()
            .filter(move |(_, (_, tip))| *tip == id)
            .map(|(i, (name, _))| (i, name.as_str()))
    }
}

fn lanes_used(r: &Row) -> usize {
    let widest = r.up.iter().chain(&r.down).map(|(a, b, ..)| (*a).max(*b)).max().unwrap_or(0);
    widest.max(r.lane) + 1
}

/// Which branch each commit belongs to, for its colour. The default branch
/// claims its history first, so shared history reads as trunk; then each
/// other branch claims what is left along its first parents - its own work.
/// Returns an index into `branches`.
pub fn owners(commits: &[Commit], branches: &[(String, CommitId)]) -> HashMap<CommitId, usize> {
    let by_id: HashMap<CommitId, &Commit> = commits.iter().map(|c| (c.id, c)).collect();
    let mut order: Vec<usize> = (0..branches.len()).collect();
    order.sort_by_key(|i| (branches[*i].0 != DEFAULT_BRANCH, *i));
    let mut owner = HashMap::new();
    for b in order {
        let mut cursor = Some(branches[b].1);
        while let Some(id) = cursor {
            if owner.contains_key(&id) {
                break;
            }
            let Some(c) = by_id.get(&id) else { break };
            owner.insert(id, b);
            cursor = c.parents.first().copied();
        }
    }
    owner
}

/// Lay the commits out in lanes. A lane is a column that expects a
/// particular commit further down; a commit takes the first lane expecting
/// it (or a free one, if it is a branch tip), every other lane expecting it
/// joins it, and its first parent carries on in its lane.
pub fn layout(commits: &[Commit], branches: &[(String, CommitId)]) -> Vec<Row> {
    let owner = owners(commits, branches);
    let shown: std::collections::HashSet<CommitId> = commits.iter().map(|c| c.id).collect();
    let mut lanes: Vec<Lane> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for c in commits {
        let branch = owner.get(&c.id).copied();
        let expecting: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_some_and(|(id, ..)| id == c.id))
            .map(|(j, _)| j)
            .collect();
        let lane = match expecting.first() {
            Some(j) => *j,
            None => free_lane(&mut lanes),
        };

        let up =
            lanes
                .iter()
                .enumerate()
                .filter_map(|(j, l)| {
                    l.map(|(id, colour, dashed)| {
                        if id == c.id {
                            (j, lane, colour, dashed)
                        } else {
                            (j, j, colour, dashed)
                        }
                    })
                })
                .collect();

        for j in &expecting {
            lanes[*j] = None;
        }
        let mut from_node: Vec<(usize, bool)> = Vec::new();
        // Parents, then - dashed - the branch a merge came from, if it is on
        // the graph at all (a deleted branch's commits are not).
        let merged = c.merged.map(|m| m.from).filter(|f| shown.contains(f));
        let leads = c.parents.iter().map(|p| (*p, false)).chain(merged.map(|f| (f, true)));
        for (k, (p, dashed)) in leads.enumerate() {
            let j = if k == 0 {
                lane
            } else {
                match lanes.iter().position(|l| l.is_some_and(|(id, _, d)| id == p && d == dashed))
                {
                    Some(j) => j,
                    None => free_lane(&mut lanes),
                }
            };
            lanes[j] = Some((p, branch, dashed));
            from_node.push((j, dashed));
        }
        let down = lanes
            .iter()
            .enumerate()
            .filter_map(|(j, l)| {
                l.map(|(_, colour, dashed)| match from_node.iter().find(|(k, _)| *k == j) {
                    Some((_, d)) => (lane, j, branch, *d),
                    None => (j, j, colour, dashed),
                })
            })
            .collect();
        while lanes.last().is_some_and(|l| l.is_none()) {
            lanes.pop();
        }
        rows.push(Row { lane, branch, up, down });
    }
    rows
}

fn free_lane(lanes: &mut Vec<Lane>) -> usize {
    match lanes.iter().position(|l| l.is_none()) {
        Some(j) => j,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

/// A branch's colour: the chart palette, in order, so the graph and the
/// branch list agree. A commit no branch reaches (a detached HEAD) is grey.
pub fn colour(branch: Option<usize>, dark: bool) -> Color32 {
    match branch {
        Some(i) => series_colour(i % MAX_SERIES, dark),
        None => fmt::dim(),
    }
}

const ROW: f32 = 24.0;
const LANE: f32 = 14.0;

/// The graph as a scrolling list. Click a row to select its commit.
pub fn show(ui: &mut Ui, g: &Graph, selected: &mut Option<CommitId>) {
    let dark = ui.visuals().dark_mode;
    let gutter = 10.0 + LANE * g.lanes as f32;
    ui.scope(|ui| {
        // Rows touch, so the lines run unbroken from one to the next.
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical().id_salt("commit_graph").auto_shrink([false, false]).show_rows(
            ui,
            ROW,
            g.rows.len(),
            |ui, range| {
                for i in range {
                    let (row, c) = (&g.rows[i], &g.commits[i]);
                    let (rect, response) = ui
                        .allocate_exact_size(egui::vec2(ui.available_width(), ROW), Sense::click());
                    let is_selected = *selected == Some(c.id);
                    if is_selected || response.hovered() {
                        let fill = if is_selected {
                            ui.visuals().selection.bg_fill
                        } else {
                            ui.visuals().widgets.hovered.weak_bg_fill
                        };
                        ui.painter().rect_filled(rect, 3.0, fill);
                    }
                    paint_lines(ui, rect, row, dark);

                    // Tags for branch tips, then the commit, cut to fit.
                    let text_rect = rect.with_min_x(rect.left() + gutter);
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(text_rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    child.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    for (b, name) in g.tips(c.id) {
                        let tag = RichText::new(name).small().strong().color(colour(Some(b), dark));
                        child.label(tag);
                    }
                    let summary = RichText::new(format!("{}  {}", c.id.short(), c.summary()));
                    child.add(egui::Label::new(summary).selectable(false));

                    let response = response.on_hover_text(format!(
                        "{}\n{}\nby {}",
                        c.id.short(),
                        c.message,
                        c.author
                    ));
                    if response.clicked() {
                        *selected = Some(c.id);
                    }
                }
            },
        );
    });
}

fn paint_lines(ui: &Ui, rect: egui::Rect, row: &Row, dark: bool) {
    let x = |lane: usize| rect.left() + 8.0 + LANE * lane as f32;
    let (top, mid, bottom) = (rect.top(), rect.center().y, rect.bottom());
    let painter = ui.painter();
    let piece = |from: usize, to: usize, b: Option<usize>, dashed: bool, y0: f32, y1: f32| {
        let ends = [egui::pos2(x(from), y0), egui::pos2(x(to), y1)];
        if dashed {
            let stroke = Stroke::new(1.5_f32, colour(b, dark).gamma_multiply(0.8));
            painter.extend(egui::Shape::dashed_line(&ends, stroke, 3.0, 3.0));
        } else {
            painter.line_segment(ends, Stroke::new(2.0_f32, colour(b, dark)));
        }
    };
    for (from, to, b, dashed) in &row.up {
        piece(*from, *to, *b, *dashed, top, mid);
    }
    for (from, to, b, dashed) in &row.down {
        piece(*from, *to, *b, *dashed, mid, bottom);
    }
    let centre = egui::pos2(x(row.lane), mid);
    let fill = colour(row.branch, dark);
    painter.circle(centre, 4.5, fill, Stroke::new(1.5_f32, ui.visuals().panel_fill));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(n: u8, parents: &[CommitId]) -> Commit {
        Commit::new(parents.to_vec(), "t", n as i64, format!("c{n}"), vec![]).unwrap()
    }

    /// main: a - b - d        side: a - b - c
    #[test]
    fn a_branch_gets_its_own_lane_and_colour_and_rejoins_at_the_fork() {
        let a = commit(1, &[]);
        let b = commit(2, &[a.id]);
        let c = commit(3, &[b.id]);
        let d = commit(4, &[b.id]);
        let branches = vec![("main".to_string(), d.id), ("side".to_string(), c.id)];
        // Newest first: d, c, b, a.
        let commits = vec![d.clone(), c.clone(), b.clone(), a.clone()];
        let rows = layout(&commits, &branches);

        assert_eq!(rows[0].lane, 0, "main's tip takes the first lane");
        assert_eq!(rows[1].lane, 1, "side's tip, a second lane beside it");
        assert_eq!(rows[0].branch, Some(0));
        assert_eq!(rows[1].branch, Some(1), "side's own commit in side's colour");
        assert_eq!(rows[2].branch, Some(0), "the shared fork point is main's");
        // At the fork both lanes close in on it.
        assert_eq!(rows[2].lane, 0);
        assert!(rows[2].up.contains(&(1, 0, Some(1), false)), "side's line bends into the fork");
        assert!(rows[2].up.contains(&(0, 0, Some(0), false)));
        assert!(rows[3].down.is_empty(), "the root has nothing below it");

        let owner = owners(&commits, &branches);
        assert_eq!(owner[&a.id], 0);
    }

    #[test]
    fn the_default_branch_claims_shared_history_whatever_its_name_sorts_as() {
        let a = commit(1, &[]);
        let b = commit(2, &[a.id]);
        // "aaa" sorts before "main" but branched off it.
        let branches = vec![("aaa".to_string(), b.id), ("main".to_string(), a.id)];
        let owner = owners(&[b.clone(), a.clone()], &branches);
        assert_eq!(owner[&a.id], 1, "main");
        assert_eq!(owner[&b.id], 0, "aaa");
    }
}
