//! A searchable ledger picker laid out as the ledger tree.
//!
//! One widget, used wherever a ledger is chosen: the sides of an entry, a
//! bucket's members, the Views editor. Typing filters by any part of the
//! path, and a match keeps its ancestors on screen, so "tux" still shows you
//! that it is `Wedding > Tuxedo` and not some other tuxedo.

use crate::fmt;
use egui::{RichText, Ui};
use ledgit_core::prelude::*;
use ledgit_core::tree::LedgerTree;

/// What was picked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Pick {
    Ledger(LedgerUid),
    /// A whole subtree, by path. Only offered when the caller allows it.
    Subtree(String),
}

pub struct Picker<'a> {
    id: egui::Id,
    budget: &'a Budget,
    selected_text: String,
    width: f32,
    subtrees: bool,
    paths: bool,
    hide: Vec<LedgerUid>,
}

impl<'a> Picker<'a> {
    pub fn new(id: impl std::hash::Hash, budget: &'a Budget) -> Self {
        Picker {
            id: egui::Id::new(("ledger_picker", id)),
            budget,
            selected_text: "choose a ledger...".into(),
            width: 240.0,
            subtrees: false,
            paths: false,
            hide: Vec::new(),
        }
    }

    pub fn selected_text(mut self, text: impl Into<String>) -> Self {
        self.selected_text = text.into();
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Offer "everything under here" on every level that has children.
    pub fn subtrees(mut self, on: bool) -> Self {
        self.subtrees = on;
        self
    }

    /// Pick a *place* in the tree rather than a ledger: every level is
    /// offered, including ones no ledger sits on, plus the top level. Picks
    /// come back as [`Pick::Subtree`], the top level as an empty path.
    pub fn paths(mut self, on: bool) -> Self {
        self.paths = on;
        self
    }

    /// Ledgers not to offer, e.g. those already in a bucket.
    pub fn hide(mut self, uids: Vec<LedgerUid>) -> Self {
        self.hide = uids;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Option<Pick> {
        let mut picked = None;
        let search_id = self.id.with("search");
        let mut list_layer = None;
        egui::ComboBox::from_id_salt(self.id)
            .selected_text(self.selected_text.clone())
            .width(self.width)
            .height(360.0)
            // A combo box closes on any click by default - including the
            // click that puts the cursor in its own search box. Close on a
            // click outside, or on a pick, instead.
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show_ui(ui, |ui| {
                list_layer = Some(ui.layer_id());
                let mut needle: String = ui.data_mut(|d| d.get_temp(search_id)).unwrap_or_default();
                let search = ui.add(
                    egui::TextEdit::singleline(&mut needle)
                        .hint_text("search, e.g. wedding or tux")
                        .desired_width(self.width - 16.0),
                );
                // Ready to type the moment the list opens.
                let opened_id = search_id.with("opened");
                if !ui.data(|d| d.get_temp::<bool>(opened_id).unwrap_or(false)) {
                    search.request_focus();
                    ui.data_mut(|d| d.insert_temp(opened_id, true));
                }
                ui.separator();
                picked = self.list(ui, &needle);
                if picked.is_some() {
                    needle.clear();
                    ui.close();
                }
                ui.data_mut(|d| d.insert_temp(search_id, needle));
            });
        if !egui::ComboBox::is_open(ui.ctx(), ui.make_persistent_id(self.id)) {
            ui.data_mut(|d| d.remove::<bool>(search_id.with("opened")));
        }
        if let Some(layer) = list_layer {
            keep_above(ui, layer);
        }
        picked
    }

    fn list(&self, ui: &mut Ui, needle: &str) -> Option<Pick> {
        let l = self.budget;
        let tree = LedgerTree::build(l);
        let visible = visible_nodes(&tree, needle);
        if visible.iter().all(|v| !v) {
            ui.label(RichText::new("nothing matches").color(fmt::dim()));
            return None;
        }
        let mut picked = None;
        if self.paths {
            if needle.trim().is_empty()
                && ui.selectable_label(false, RichText::new("(top level)").italics()).clicked()
            {
                picked = Some(Pick::Subtree(String::new()));
            }
            for (n, node) in tree.nodes.iter().enumerate() {
                if !visible[n] {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.add_space(14.0 * node.depth as f32);
                    let text = match node.ledger {
                        Some(_) => RichText::new(node.name()),
                        None => RichText::new(node.name()).strong(),
                    };
                    if ui.selectable_label(false, text).on_hover_text(&node.path).clicked() {
                        picked = Some(Pick::Subtree(node.path.clone()));
                    }
                });
            }
            return picked;
        }
        for (n, node) in tree.nodes.iter().enumerate() {
            if !visible[n] {
                continue;
            }
            let has_children = node.end as usize > n + 1;
            ui.horizontal(|ui| {
                ui.add_space(14.0 * node.depth as f32);
                match node.ledger {
                    Some(ix) if !self.hide.contains(&l.ledgers.uid[ix.get()]) => {
                        let label = format!("{}  [{}]", node.name(), l.ledgers.normality[ix.get()]);
                        if ui.selectable_label(false, label).on_hover_text(&node.path).clicked() {
                            picked = Some(Pick::Ledger(l.ledgers.uid[ix.get()]));
                        }
                    }
                    // A level with no ledger of its own, or one already taken:
                    // shown for context, not pickable as a ledger.
                    _ => {
                        ui.label(RichText::new(node.name()).color(fmt::dim()));
                    }
                }
                if self.subtrees && has_children {
                    let n_under = tree.subtree(n).len();
                    if ui
                        .small_button(format!("all {n_under} under"))
                        .on_hover_text(format!(
                            "Everything under {} - including ledgers added there later",
                            node.path
                        ))
                        .clicked()
                    {
                        picked = Some(Pick::Subtree(node.path.clone()));
                    }
                }
            });
        }
        picked
    }
}

/// Keep a popup opened from `ui` drawn directly above it.
///
/// A popup and the form modal both sit on egui's Foreground order, where the
/// last layer raised wins - and the modal is raised whenever it is clicked,
/// which is exactly how a popup inside it gets opened. Left alone, a list
/// can end up under the form, visible only where it hangs past the edge.
/// A sublayer is always drawn straight above its parent.
pub fn keep_above(ui: &Ui, popup: egui::LayerId) {
    let parent = ui.layer_id();
    if parent != popup && parent.order == popup.order {
        ui.ctx().set_sublayer(parent, popup);
    }
}

/// Which nodes to draw for a search: every match, plus its ancestors so the
/// match is shown in place. An empty search shows everything.
pub fn visible_nodes(tree: &LedgerTree, needle: &str) -> Vec<bool> {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        return vec![true; tree.nodes.len()];
    }
    let mut visible = vec![false; tree.nodes.len()];
    // Open ancestors by depth, so a hit can light up its whole chain.
    let mut chain: Vec<usize> = Vec::new();
    for (n, node) in tree.nodes.iter().enumerate() {
        chain.truncate(node.depth as usize);
        chain.push(n);
        if node.path.to_lowercase().contains(&needle) {
            for a in &chain {
                visible[*a] = true;
            }
        }
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_keeps_the_path_to_each_match() {
        let mk = |name: &str| Op::CreateLedger {
            uid: LedgerUid::new(),
            name: name.into(),
            description: String::new(),
            normality: Normality::Debit,
            opened: Date::from_ymd(2024, 1, 1).unwrap(),
        };
        let l = Budget::replay(&[mk("Wedding:Tuxedo:Rental"), mk("Wedding:Venue"), mk("Cash")])
            .unwrap();
        let t = LedgerTree::build(&l);
        let shown: Vec<&str> = visible_nodes(&t, "rent")
            .iter()
            .zip(&t.nodes)
            .filter(|(v, _)| **v)
            .map(|(_, n)| n.path.as_str())
            .collect();
        assert_eq!(shown, ["Wedding", "Wedding:Tuxedo", "Wedding:Tuxedo:Rental"]);
        assert!(visible_nodes(&t, "").iter().all(|v| *v));
        assert!(visible_nodes(&t, "zzz").iter().all(|v| !v));
    }
}
