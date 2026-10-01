//! The calendar: what the issuers cost per period, and when each falls due.
//!
//! This screen reads every issuer; a saved view draws the same calendar for
//! the issuers it breaks down.

use super::{empty, heading, num};
use crate::app::Session;
use crate::fmt;
use crate::table::{figures, text, Table};
use egui::{RichText, Ui};
use ledgit_core::dues::{self, CalendarEntry, DueStatus, RateLine};
use ledgit_core::id::IssuerIx;
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
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

pub fn show(ui: &mut Ui, s: &mut Session) {
    heading(
        ui,
        "Calendar",
        "What every issuer costs per day, week, month and year, and the dates each falls due. To see a few on their own, pick them in a view.",
    );
    if s.budget().issuers.is_empty() {
        empty(ui, "No issuers yet.");
        return;
    }
    let every: Vec<IssuerIx> = s.budget().issuers.indices().collect();
    egui::ScrollArea::vertical().id_salt("calendar_screen").show(ui, |ui| {
        rates(ui, &dues::rate_lines(s.budget(), &every));
        ui.add_space(16.0);
        calendar(ui, s, "every", &every);
    });
}

/// Each issuer's rate in every unit, and what the running ones total.
fn rates(ui: &mut Ui, lines: &[RateLine]) {
    let mut cols = vec![text("issuer").max(280.0), text("schedule").max(240.0), figures("each")];
    cols.extend(Period::ALL.iter().map(|p| figures(format!("per {p}"))));
    Table::new("issuer_rates", cols).show(ui, lines.len() + 1, |row| {
        let Some(line) = lines.get(row.index()) else {
            // The total, as the last row.
            row.col(|ui| {
                ui.label(RichText::new("total").strong());
            });
            row.col(|ui| {
                ui.label(RichText::new("running issuers").small().color(fmt::dim()));
            });
            row.col(|_| {});
            for p in Period::ALL {
                row.col(|ui| {
                    num(ui, fmt::mono(fmt::amount(dues::total(lines, p))).strong());
                });
            }
            return;
        };
        row.col(|ui| {
            let name = RichText::new(&line.name);
            let r =
                ui.label(if line.paused { name.color(fmt::dim()).strikethrough() } else { name });
            if line.paused {
                r.on_hover_text("paused: not in the total");
            }
        });
        row.col(|ui| {
            ui.label(RichText::new(line.schedule.describe()).color(fmt::dim()));
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(line.amount)));
        });
        for p in Period::ALL {
            row.col(|ui| {
                let text = match line.rate(p) {
                    Some(m) => fmt::mono(fmt::amount(m)),
                    None => RichText::new("one-off").color(fmt::dim()),
                };
                num(ui, text);
            });
        }
    });
    let paused = dues::paused_total(lines, Period::Month);
    if !paused.is_zero() {
        ui.label(
            RichText::new(format!("Paused issuers would add {} a month.", fmt::amount(paused)))
                .color(fmt::dim()),
        );
    }
    ui.label(
        RichText::new("Rates are averages: a month is 30.44 days, a year 365.24.")
            .small()
            .color(fmt::dim()),
    );
}

/// A month grid of every date the issuers fall due.
pub fn calendar(ui: &mut Ui, s: &mut Session, id: &str, members: &[IssuerIx]) {
    let month = s.calendar_month;
    let today = Date::today_utc();
    ui.horizontal(|ui| {
        if ui.button("\u{2039}").on_hover_text("Previous month").clicked() {
            s.calendar_month = month.add_months(-1);
        }
        ui.label(
            RichText::new(format!("{} {}", MONTHS[month.month() as usize - 1], month.year()))
                .strong(),
        );
        if ui.button("\u{203a}").on_hover_text("Next month").clicked() {
            s.calendar_month = month.add_months(1);
        }
        if ui.small_button("This month").clicked() {
            s.calendar_month = Period::Month.start_of(today);
        }
        ui.separator();
        ui.selectable_value(&mut s.calendar_list, false, "Month")
            .on_hover_text("A grid of the month, a few payments per day");
        ui.selectable_value(&mut s.calendar_list, true, "List")
            .on_hover_text("Every payment in the month, one per line");
    });

    let l = s.budget();
    let last = month.add_months(1).add_days(-1);
    let entries = dues::calendar(l, members, month, last, today);

    ui.add_space(4.0);
    if s.calendar_list {
        payment_list(ui, l, id, &entries, today);
    } else {
        let busiest = (0..=last.0 - month.0)
            .map(|d| entries.iter().filter(|e| e.date.0 == month.0 + d).count())
            .max()
            .unwrap_or(0);
        let cell = egui::vec2(118.0, 72.0);
        egui::Grid::new(("calendar_grid", id)).num_columns(7).spacing([4.0, 4.0]).show(ui, |ui| {
            for w in WEEKDAYS {
                ui.label(RichText::new(w).small().color(fmt::dim()));
            }
            ui.end_row();
            for _ in 0..month.weekday() {
                ui.allocate_space(cell);
            }
            let mut day = month;
            while day <= last {
                let todays: Vec<&CalendarEntry> =
                    entries.iter().filter(|e| e.date == day).collect();
                day_cell(ui, l, day, day == today, &todays, cell);
                if day.weekday() == 6 {
                    ui.end_row();
                }
                day = day.add_days(1);
            }
        });
        if busiest > DAY_SHOWN {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Some days hold up to {busiest} payments."))
                        .small()
                        .color(fmt::dim()),
                );
                if ui.small_button("See the month as a list").clicked() {
                    s.calendar_list = true;
                }
            });
        }
    }

    // The month's totals, by what state each payment is in.
    let sum = |st: DueStatus| -> (usize, Money) {
        let hits = entries.iter().filter(|e| e.status == st);
        (hits.clone().count(), hits.map(|e| e.amount).sum())
    };
    let (n_up, up) = sum(DueStatus::Upcoming);
    let (n_over, over) = sum(DueStatus::Overdue);
    let (n_done, done) = sum(DueStatus::Posted);
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("Still to come: {} in {n_up} payment(s)", fmt::amount(up)));
        ui.separator();
        ui.label(
            RichText::new(format!("Already posted: {} in {n_done}", fmt::amount(done)))
                .color(fmt::dim()),
        );
        if n_over > 0 {
            ui.separator();
            ui.label(
                RichText::new(format!(
                    "Overdue: {} in {n_over} - run the issuers to post them",
                    fmt::amount(over)
                ))
                .color(fmt::bad()),
            );
        }
    });
}

/// The month as a list: one line per payment, days grouped, so a busy
/// month reads top to bottom instead of through "+3 more".
fn payment_list(ui: &mut Ui, l: &Budget, id: &str, entries: &[CalendarEntry], today: Date) {
    if entries.is_empty() {
        ui.label(RichText::new("Nothing falls due this month.").color(fmt::dim()));
        return;
    }
    Table::new(
        ("calendar_payments", id),
        vec![text("date"), text("day"), text("issuer").max(320.0), figures("amount"), text("")],
    )
    .height(crate::table::Height::Max(460.0))
    .fit_to(entries.first().map(|e| e.date))
    .show(ui, entries.len(), |row| {
        let i = row.index();
        let e = &entries[i];
        // The date once per day, so the eye finds the day breaks.
        let first_of_day = i == 0 || entries[i - 1].date != e.date;
        row.col(|ui| {
            if first_of_day {
                let t = fmt::mono(e.date.to_string());
                ui.label(if e.date == today { t.strong() } else { t });
            }
        });
        row.col(|ui| {
            if first_of_day {
                ui.label(RichText::new(WEEKDAYS[e.date.weekday() as usize]).color(fmt::dim()));
            }
        });
        row.col(|ui| {
            ui.label(entry_text(l, e, usize::MAX).size(14.0));
        });
        row.col(|ui| {
            num(ui, fmt::mono(fmt::amount(e.amount)));
        });
        row.col(|ui| {
            let status = match e.status {
                DueStatus::Posted => RichText::new("posted").color(fmt::dim()),
                DueStatus::Overdue => RichText::new("overdue").color(fmt::bad()).strong(),
                DueStatus::Paused => RichText::new("paused").color(fmt::dim()),
                DueStatus::Upcoming => RichText::new("upcoming"),
            };
            ui.label(status.small());
        });
    });
}

/// Payments a day cell shows before "+N more".
const DAY_SHOWN: usize = 3;

fn day_cell(
    ui: &mut Ui,
    l: &Budget,
    day: Date,
    is_today: bool,
    entries: &[&CalendarEntry],
    size: egui::Vec2,
) {
    const SHOWN: usize = DAY_SHOWN;
    let stroke = if is_today {
        egui::Stroke::new(1.5_f32, ui.visuals().selection.stroke.color)
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    let frame = egui::Frame::new().stroke(stroke).corner_radius(4.0).inner_margin(4.0);
    let response = frame
        .show(ui, |ui| {
            ui.set_min_size(size - egui::vec2(8.0, 8.0));
            ui.set_max_width(size.x - 8.0);
            ui.vertical(|ui| {
                let n = RichText::new(day.day().to_string()).small();
                ui.label(if is_today { n.strong() } else { n.color(fmt::dim()) });
                for e in entries.iter().take(SHOWN) {
                    ui.label(entry_text(l, e, 12));
                }
                if entries.len() > SHOWN {
                    ui.label(
                        RichText::new(format!("+{} more", entries.len() - SHOWN))
                            .small()
                            .color(fmt::dim()),
                    );
                }
            });
        })
        .response;
    if !entries.is_empty() {
        response.on_hover_ui(|ui| {
            ui.label(RichText::new(day.to_string()).strong());
            for e in entries {
                let status = match e.status {
                    DueStatus::Posted => "  posted",
                    DueStatus::Overdue => "  overdue",
                    DueStatus::Paused => "  paused",
                    DueStatus::Upcoming => "",
                };
                ui.label(format!(
                    "{}  {}{status}",
                    l.issuers.name[e.issuer.get()],
                    fmt::amount(e.amount)
                ));
            }
        });
    }
}

/// One line in a day cell, styled by its state: overdue in red, posted and
/// paused recessive, paused struck through - never colour alone.
fn entry_text(l: &Budget, e: &CalendarEntry, max: usize) -> RichText {
    let name = fmt::clip(&l.issuers.name[e.issuer.get()], max);
    let t = RichText::new(format!("{name} {}", short_amount(e.amount))).small();
    match e.status {
        DueStatus::Upcoming => t,
        DueStatus::Overdue => t.color(fmt::bad()).strong(),
        DueStatus::Posted => t.color(fmt::dim()),
        DueStatus::Paused => t.color(fmt::dim()).strikethrough(),
    }
}

/// "1,650" rather than "1,650.00": a calendar cell has no room for cents
/// that are nearly always zero.
fn short_amount(m: Money) -> String {
    let full = fmt::amount(m);
    full.strip_suffix(".00").map(str::to_string).unwrap_or(full)
}
