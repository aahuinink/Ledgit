//! A date field: the `YYYY-MM-DD` text box every form already had, with a
//! calendar button beside it.
//!
//! The text stays the source of truth, so typing a date still works, a field
//! can still be left blank where blank means "no limit", and every caller
//! keeps parsing exactly as it did. The calendar only writes into it.

use crate::fmt;
use egui::{RichText, Ui};
use ledgit_core::date::days_in_month;
use ledgit_core::prelude::*;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub struct DateField<'a> {
    id: egui::Id,
    value: &'a mut String,
    hint: &'a str,
    /// Offer "Clear", for fields where blank means "no limit".
    optional: bool,
}

impl<'a> DateField<'a> {
    pub fn new(id: impl std::hash::Hash, value: &'a mut String) -> Self {
        DateField {
            id: egui::Id::new(("date_field", id)),
            value,
            hint: "YYYY-MM-DD",
            optional: false,
        }
    }

    pub fn optional(mut self, hint: &'a str) -> Self {
        self.optional = true;
        self.hint = hint;
        self
    }

    /// Returns true when the value changed this frame, typed or picked.
    pub fn show(self, ui: &mut Ui) -> bool {
        let DateField { id, value, hint, optional } = self;
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let bad = !value.trim().is_empty() && value.trim().parse::<Date>().is_err();
            let mut edit = egui::TextEdit::singleline(value).hint_text(hint).desired_width(96.0);
            if bad {
                edit = edit.text_color(fmt::bad());
            }
            changed |= ui.add(edit).on_hover_text("YYYY-MM-DD").changed();

            let button = ui.button("\u{1F4C5}").on_hover_text("Pick from a calendar");
            let popup_id = id.with("popup");
            if !egui::Popup::is_id_open(ui.ctx(), popup_id) {
                // Closed: next time it opens, start again from the field.
                ui.data_mut(|d| d.remove::<Date>(id.with("month")));
            }
            let shown = egui::Popup::menu(&button)
                .id(popup_id)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    if let Some(picked) = calendar(ui, id, value, optional) {
                        *value = picked;
                        changed = true;
                        egui::Popup::close_id(ui.ctx(), popup_id);
                    }
                    ui.layer_id()
                });
            if let Some(r) = shown {
                crate::picker::keep_above(ui, r.inner);
            }
        });
        changed
    }
}

/// The month grid. Returns the new text when a day, Today or Clear is picked.
fn calendar(ui: &mut Ui, id: egui::Id, value: &str, optional: bool) -> Option<String> {
    let today = Date::today_utc();
    let selected = value.trim().parse::<Date>().ok();
    let month_id = id.with("month");
    // The month on show is remembered while the popup is open, and starts at
    // the field's date, or this month when it is blank.
    let mut month: Date = ui
        .data(|d| d.get_temp(month_id))
        .unwrap_or_else(|| Period::Month.start_of(selected.unwrap_or(today)));
    let mut picked = None;

    ui.horizontal(|ui| {
        if ui.small_button("\u{00AB}").on_hover_text("Previous year").clicked() {
            month = month.add_months(-12);
        }
        if ui.small_button("\u{2039}").on_hover_text("Previous month").clicked() {
            month = month.add_months(-1);
        }
        let title = format!("{} {}", MONTHS[month.month() as usize - 1], month.year());
        ui.add_sized([128.0, 18.0], egui::Label::new(RichText::new(title).strong()));
        if ui.small_button("\u{203A}").on_hover_text("Next month").clicked() {
            month = month.add_months(1);
        }
        if ui.small_button("\u{00BB}").on_hover_text("Next year").clicked() {
            month = month.add_months(12);
        }
    });
    ui.add_space(4.0);

    egui::Grid::new(id.with("grid")).num_columns(7).spacing([2.0, 2.0]).show(ui, |ui| {
        for d in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] {
            ui.add_sized(
                [28.0, 16.0],
                egui::Label::new(RichText::new(d).small().color(fmt::dim())),
            );
        }
        ui.end_row();
        for _ in 0..month.weekday() {
            ui.label("");
        }
        for day in 1..=days_in_month(month.year(), month.month()) {
            let date = month.add_days(day as i32 - 1);
            let mut text = RichText::new(day.to_string());
            if date == today {
                text = text.strong().underline();
            }
            let is_selected = selected == Some(date);
            if ui.add_sized([28.0, 22.0], egui::Button::selectable(is_selected, text)).clicked() {
                picked = Some(date.to_string());
            }
            if date.weekday() == 6 {
                ui.end_row();
            }
        }
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Today").clicked() {
            picked = Some(today.to_string());
        }
        if optional && ui.button("Clear").clicked() {
            picked = Some(String::new());
        }
    });

    if picked.is_some() {
        ui.data_mut(|d| d.remove::<Date>(month_id));
    } else {
        ui.data_mut(|d| d.insert_temp(month_id, month));
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every month shape draws: one starting on a Monday, one on a Sunday, a
    /// leap February, and a field holding nothing or nonsense.
    #[test]
    fn the_calendar_draws_any_month_and_any_field() {
        for value in ["2024-02-10", "2023-01-01", "2025-06-30", "", "not a date"] {
            egui::__run_test_ui(|ui| {
                let picked = calendar(ui, egui::Id::new("t"), value, true);
                assert!(picked.is_none(), "drawing alone picks nothing");
            });
        }
    }

    #[test]
    fn a_field_draws_with_bad_and_blank_text() {
        for v in ["2024-01-01", "", "13/01/2024"] {
            egui::__run_test_ui(|ui| {
                let mut t = v.to_string();
                DateField::new("t", &mut t).optional("any").show(ui);
            });
        }
    }
}
