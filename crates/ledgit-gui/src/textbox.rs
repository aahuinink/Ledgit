//! A text box for prose - an alert's message, a view's description - with a
//! button to pop it out into a full-size editor.
//!
//! The inline box is roomy enough for a sentence or two. The pop-out edits
//! the very same text, so there is nothing to copy back: close it and the
//! box shows what you wrote.

use egui::{Id, Ui};

pub struct LongText<'a> {
    id: Id,
    text: &'a mut String,
    hint: &'a str,
    title: &'a str,
    width: f32,
    rows: usize,
}

impl<'a> LongText<'a> {
    pub fn new(id: impl std::hash::Hash, text: &'a mut String) -> Self {
        LongText {
            id: Id::new(("long_text", id)),
            text,
            hint: "",
            title: "Edit",
            width: 520.0,
            rows: 2,
        }
    }

    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }

    /// The pop-out's heading: what is being written.
    pub fn title(mut self, title: &'a str) -> Self {
        self.title = title;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows;
        self
    }

    /// Draw the box and its pop-out button. Returns the inline box's response.
    pub fn show(self, ui: &mut Ui) -> egui::Response {
        let LongText { id, text, hint, title, width, rows } = self;
        let open_id = id.with("open");
        let focus_id = id.with("focus");

        let response = ui
            .horizontal_top(|ui| {
                let width = width.min(ui.available_width() - 32.0).max(120.0);
                let r = ui.add(
                    egui::TextEdit::multiline(&mut *text)
                        .id(id.with("inline"))
                        .hint_text(hint)
                        .desired_width(width)
                        .desired_rows(rows),
                );
                if ui.small_button("\u{2197}").on_hover_text("Open in a full-size editor").clicked()
                {
                    ui.data_mut(|d| {
                        d.insert_temp(open_id, true);
                        d.insert_temp(focus_id, true);
                    });
                }
                r
            })
            .inner;

        if ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false) {
            let modal = egui::Modal::new(id.with("modal")).show(ui.ctx(), |ui| {
                // Most of the window, whatever size it is.
                let screen = ui.ctx().content_rect().size();
                let w = (screen.x * 0.7).clamp(360.0, 900.0);
                let h = (screen.y * 0.6).clamp(200.0, 700.0);
                ui.set_width(w);
                ui.heading(title);
                ui.add_space(6.0);
                let rows = (h / ui.text_style_height(&egui::TextStyle::Body)).floor() as usize;
                let big = egui::ScrollArea::vertical()
                    .max_height(h)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut *text)
                                .id(id.with("popped"))
                                .hint_text(hint)
                                .desired_width(f32::INFINITY)
                                .desired_rows(rows.max(8)),
                        )
                    })
                    .inner;
                if ui.data_mut(|d| d.remove_temp::<bool>(focus_id)).unwrap_or(false) {
                    big.request_focus();
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let done = ui.button("Done").clicked();
                    ui.label(
                        egui::RichText::new(format!("{} characters", text.chars().count()))
                            .small()
                            .color(crate::fmt::dim()),
                    );
                    done
                })
                .inner
            });
            if modal.inner || modal.should_close() {
                ui.data_mut(|d| d.remove_temp::<bool>(open_id));
            }
        }
        response
    }
}
