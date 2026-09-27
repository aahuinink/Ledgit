//! Every data table in the app goes through here.
//!
//! One place decides how a table behaves, because a budget screen is mostly
//! tables and they must all read the same way:
//!
//! * **Columns fit their content, between a floor and a cap.** The floor is
//!   [`MIN_CHARS`] characters, so no column is ever squeezed unreadable and
//!   nothing needs dragging open to be read; a table wider than the window
//!   scrolls sideways instead. Past the cap a cell is cut short with an
//!   ellipsis and shows the whole text on hover, so one long ledger name
//!   cannot push the figures off the screen or spill into the next column.
//!   Every column can be dragged wider.
//! * **Figures are right-aligned inside their own column**, not against the
//!   window edge, so a balance sits under its header.
//! * **Stripes run under every column**, the last one included.
//! * **Long tables scroll** - inside the table, with the header kept in view,
//!   or as part of the page they sit on (see [`Height`]).
//!
//! Built on `egui_extras::TableBuilder`. `egui::Grid` is still right for
//! forms, where there is no data to size to.

use crate::fmt;
use egui::{Align, Layout, RichText, Ui, WidgetText};
use egui_extras::{Column, TableBuilder, TableRow};
use std::hash::{Hash, Hasher};

/// The narrowest a column gets, in characters: room for `-1,234,567.89`
/// with space to spare, or a date, or most names.
pub const MIN_CHARS: f32 = 16.0;

/// One column: its header, which side its cells hug, and how wide it may grow
/// before its cells are cut short.
pub struct Col {
    head: String,
    right: bool,
    max: f32,
    /// Exempt from the [`MIN_CHARS`] floor: a pin, a row number, a button.
    narrow: bool,
}

/// A column of text, left-aligned.
pub fn text(head: impl Into<String>) -> Col {
    Col { head: head.into(), right: false, max: 340.0, narrow: false }
}

/// A column of figures, right-aligned. Put figures in it with [`num`].
pub fn figures(head: impl Into<String>) -> Col {
    Col { head: head.into(), right: true, max: 240.0, narrow: false }
}

impl Col {
    /// The widest this column fits itself to. Wider cells are cut short.
    pub fn max(mut self, width: f32) -> Col {
        self.max = width;
        self
    }

    /// Only as wide as what is in it: for a pin star, a row number, a
    /// button - not for anything that is read.
    pub fn narrow(mut self) -> Col {
        self.narrow = true;
        self
    }
}

/// How tall a table may get.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Height {
    /// As tall as its rows; the page around it scrolls. For a table that
    /// shares a scrolling page with others.
    Content,
    /// Up to this tall, then the rows scroll under a fixed header.
    Max(f32),
    /// Whatever height is left on the screen, scrolling past that. For the
    /// one big list a screen is about.
    Fill,
}

pub struct Table {
    id: egui::Id,
    cols: Vec<Col>,
    height: Height,
    row_height: Option<f32>,
    heights: Option<Vec<f32>>,
    fit: u64,
}

impl Table {
    pub fn new(id: impl Hash, cols: Vec<Col>) -> Table {
        Table {
            id: egui::Id::new(("table", id)),
            cols,
            height: Height::Content,
            row_height: None,
            heights: None,
            fit: 0,
        }
    }

    pub fn height(mut self, height: Height) -> Table {
        self.height = height;
        self
    }

    /// Rows of differing heights, one entry per row - e.g. a row carrying a
    /// second line of explanation.
    pub fn row_heights(mut self, heights: Vec<f32>) -> Table {
        self.heights = Some(heights);
        self
    }

    /// Refit the columns to their content whenever `key` changes - another
    /// bucket selected, a level unfolded. A table always refits when its
    /// row count changes; this is for what else changes the content.
    pub fn fit_to(mut self, key: impl Hash) -> Table {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut h);
        self.fit = h.finish();
        self
    }

    /// Draw `rows` rows. `add_row` fills one - call `row.col` once per
    /// column, in order; `row.index()` says which row it is.
    pub fn show(self, ui: &mut Ui, rows: usize, mut add_row: impl FnMut(&mut TableRow<'_, '_>)) {
        let Table { id, cols, height, row_height, heights, fit } = self;
        let row_height = row_height.unwrap_or_else(|| row_height_for(ui));
        let floor = min_column_width(ui);
        let n = heights.as_ref().map_or(rows, Vec::len);

        // Refit when the content changes shape. Columns the user has dragged
        // keep their width otherwise - that is the point of dragging them.
        let key = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (n, fit, cols.len()).hash(&mut h);
            h.finish()
        };
        let key_id = id.with("fit");
        let refit = ui.data(|d| d.get_temp::<u64>(key_id)) != Some(key);
        if refit {
            ui.data_mut(|d| d.insert_temp(key_id, key));
            // Throw this pass away rather than show it: a sizing pass lays
            // cells out at their natural width, overlapping for one frame.
            ui.ctx().request_discard("table refit");
        }

        egui::ScrollArea::horizontal().id_salt(id.with("h")).auto_shrink([true, true]).show(
            ui,
            |ui| {
                let mut tb = TableBuilder::new(ui)
                    .id_salt(id)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(Layout::left_to_right(Align::Center));
                tb = match height {
                    Height::Content => tb.vscroll(false),
                    Height::Max(h) => tb.max_scroll_height(h),
                    Height::Fill => tb.max_scroll_height(f32::INFINITY),
                };
                for c in &cols {
                    let min = if c.narrow { 0.0 } else { floor };
                    tb = tb.column(
                        Column::auto()
                            .at_least(min)
                            .at_most(c.max.max(min))
                            .clip(true)
                            .auto_size_this_frame(refit),
                    );
                }
                tb.header(row_height, |mut header| {
                    for c in &cols {
                        header.col(|ui| {
                            let t = RichText::new(&c.head).small().color(fmt::dim());
                            if c.right {
                                num(ui, t);
                            } else {
                                ui.label(t);
                            }
                        });
                    }
                })
                .body(|body| match heights {
                    Some(h) => body.heterogeneous_rows(h.into_iter(), |mut row| add_row(&mut row)),
                    None => body.rows(row_height, rows, |mut row| add_row(&mut row)),
                });
            },
        );
    }
}

/// [`MIN_CHARS`] characters of body text, in points - so it follows the
/// text size rather than assuming one.
pub fn min_column_width(ui: &Ui) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let digit = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    (digit * MIN_CHARS).round()
}

/// The ordinary row: one line of text or a small button, with a little air.
pub fn row_height_for(ui: &Ui) -> f32 {
    ui.spacing().interact_size.y + 4.0
}

/// A figure, right-aligned in its cell so decimal points line up down a
/// column. While a table is measuring its columns it is laid out plainly,
/// so the column is sized to the figure rather than to whatever room the
/// cell was offered.
pub fn num(ui: &mut Ui, text: impl Into<WidgetText>) -> egui::Response {
    if ui.is_sizing_pass() {
        ui.label(text)
    } else {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(text)).inner
    }
}
