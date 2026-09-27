//! Variables: named numbers and text to use while making entries.
//!
//! A number goes in any amount field, inside a formula - `200 * Car_Km_Rate`.
//! Text goes in a name or description as `{Name}`. Both are worked out when
//! the entry is made, so changing a variable here re-prices nothing already
//! posted. Like everything else, a change is staged and then committed.

use super::{empty, heading};
use crate::app::Session;
use crate::fmt;
use crate::table::{text, Height, Table};
use egui::{RichText, Ui};
use ledgit_core::model::validate_var_name;
use ledgit_core::prelude::*;

/// The add/edit row at the top of the screen.
#[derive(Default)]
pub struct VarDraft {
    pub name: String,
    pub value: String,
    /// Keep the value as text even if it looks like a number - a postcode,
    /// say, or an account number with leading zeros.
    pub as_text: bool,
}

impl VarDraft {
    fn value(&self) -> VarValue {
        if self.as_text {
            VarValue::Text(self.value.clone())
        } else {
            VarValue::guess(&self.value)
        }
    }
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(
        ui,
        "Variables",
        "Numbers for formulas in amount fields (200 * Car_Km_Rate), and text for names and \
         descriptions ({Home}). They are worked out when an entry is made; changing one later \
         re-prices nothing already posted.",
    );

    editor(ui, s);
    ui.add_space(12.0);

    if s.budget().variables.is_empty() {
        empty(ui, "No variables yet. Name one above, e.g. Car_Km_Rate = 0.68.");
        return;
    }

    let mut edit: Option<usize> = None;
    let mut delete: Option<String> = None;
    let v = &s.budget().variables;
    Table::new(
        "variables",
        vec![text("name"), text("value").max(420.0), text("kind"), text("").narrow()],
    )
    .height(Height::Fill)
    .show(ui, v.len(), |row| {
        let i = row.index();
        row.col(|ui| {
            ui.label(RichText::new(&v.name[i]).monospace().strong());
        });
        row.col(|ui| {
            ui.label(RichText::new(v.value[i].as_str()).monospace());
        });
        row.col(|ui| {
            ui.label(
                RichText::new(if v.value[i].is_number() { "number" } else { "text" })
                    .color(fmt::dim()),
            );
        });
        row.col(|ui| {
            if ui.small_button("edit").clicked() {
                edit = Some(i);
            }
            if ui.small_button("delete").clicked() {
                delete = Some(v.name[i].clone());
            }
        });
    });

    if let Some(i) = edit {
        let v = &s.budget().variables;
        s.var_draft = VarDraft {
            name: v.name[i].clone(),
            value: v.value[i].as_str().to_string(),
            as_text: !v.value[i].is_number(),
        };
    }
    if let Some(name) = delete {
        s.stage(vec![Op::DeleteVariable { name }], "deleting a variable");
    }
}

fn editor(ui: &mut Ui, s: &mut Session) {
    let exists = s.budget().variables.get(s.var_draft.name.trim()).is_some();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut s.var_draft.name)
                .hint_text("Car_Km_Rate")
                .desired_width(180.0),
        );
        ui.label("=");
        ui.add(
            egui::TextEdit::singleline(&mut s.var_draft.value)
                .hint_text("0.68, or some text")
                .desired_width(200.0),
        );
        ui.checkbox(&mut s.var_draft.as_text, "text")
            .on_hover_text("Keep it as text even if it looks like a number");
        let label = if exists { "Stage change" } else { "Stage variable" };
        if ui.button(label).clicked() {
            let name = s.var_draft.name.trim().to_string();
            match validate_var_name(&name) {
                Err(e) => s.fail(e),
                Ok(()) => {
                    let op = Op::SetVariable { name, value: s.var_draft.value() };
                    if s.stage(vec![op], "a variable") {
                        s.var_draft = VarDraft::default();
                    }
                }
            }
        }
    });
    let name = s.var_draft.name.trim();
    if !name.is_empty() {
        let hint = match validate_var_name(name) {
            Err(e) => RichText::new(e).color(fmt::bad()),
            Ok(()) => match s.var_draft.value() {
                VarValue::Number(_) => {
                    RichText::new(format!("a number: use it as  100 * {name}")).color(fmt::dim())
                }
                VarValue::Text(_) => {
                    RichText::new(format!("text: use it as  {{{name}}}")).color(fmt::dim())
                }
            },
        };
        ui.label(hint.small());
    }
}
