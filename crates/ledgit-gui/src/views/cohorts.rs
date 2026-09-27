//! Cohorts: groups of issuers, read for what they cost and when they land.

use super::{empty, heading, num};
use crate::app::Session;
use crate::fmt;
use crate::forms::FormKind;
use crate::table::{figures, text, Table};
use egui::{RichText, Ui};
use ledgit_core::cohort::{self, CalendarEntry, DueStatus};
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
        "Cohorts",
        "A cohort groups issuers the way a bucket groups ledgers. Creating or deleting one changes no payment.",
    );
    if ui.button("New cohort").clicked() {
        s.forms.open(FormKind::Cohort, s.repo.working());
    }
    ui.add_space(8.0);

    if s.budget().issuers.is_empty() {
        empty(ui, "No issuers yet. A cohort groups issuers, so make one of those first.");
        return;
    }

    let live: Vec<CohortUid> =
        s.budget().cohorts.live().map(|ix| s.budget().cohorts.uid[ix.get()]).collect();
    if s.selected_cohort.is_some_and(|c| !live.contains(&c)) {
        s.selected_cohort = None;
    }

    super::split(
        ui,
        "cohorts",
        200.0,
        s,
        |ui, s| {
            // "Every issuer" is not a cohort, but it is the calendar you most
            // often want, so it sits at the top of the list.
            if ui.selectable_label(s.selected_cohort.is_none(), "Every issuer").clicked() {
                s.selected_cohort = None;
            }
            ui.separator();
            for uid in &live {
                let Some(ix) = s.budget().cohorts.ix(*uid) else { continue };
                let name = s.budget().cohorts.name[ix.get()].clone();
                if ui.selectable_label(s.selected_cohort == Some(*uid), name).clicked() {
                    s.selected_cohort = Some(*uid);
                }
            }
            if live.is_empty() {
                ui.label(RichText::new("No cohorts yet.").color(fmt::dim()));
            }
        },
        |ui, s| {
            egui::ScrollArea::vertical().id_salt("cohort_detail").show(ui, |ui| detail(ui, s));
        },
    );
}

fn detail(ui: &mut Ui, s: &mut Session) {
    let l = s.budget();
    let (title, members, uid) =
        match s.selected_cohort.and_then(|u| l.cohorts.ix(u).map(|ix| (u, ix))) {
            Some((uid, ix)) => {
                (l.cohorts.name[ix.get()].clone(), l.cohorts.members[ix.get()].clone(), Some(uid))
            }
            None => ("Every issuer".to_string(), l.issuers.indices().collect::<Vec<_>>(), None),
        };
    let lines = cohort::rate_lines(l, &members);
    let breakdown = CohortBreakdown { uid: uid.unwrap_or_default(), name: title.clone(), lines };

    let mut ops: Vec<(Op, &'static str)> = Vec::new();

    ui.horizontal(|ui| {
        ui.heading(&title);
        if let Some(uid) = uid {
            if ui.small_button("Delete cohort").on_hover_text("Its issuers carry on.").clicked() {
                ops.push((Op::DeleteCohort { uid }, "deleting a cohort"));
            }
        }
    });
    if let Some(ix) = uid.and_then(|u| l.cohorts.ix(u)) {
        let d = &l.cohorts.description[ix.get()];
        if !d.is_empty() {
            ui.label(RichText::new(d).color(fmt::dim()));
        }
    }
    ui.add_space(6.0);

    // --- rates
    if breakdown.lines.is_empty() {
        ui.label(RichText::new("No issuers in this cohort yet.").color(fmt::dim()));
    } else {
        let mut cols =
            vec![text("issuer").max(280.0), text("schedule").max(240.0), figures("each")];
        cols.extend(Period::ALL.iter().map(|p| figures(format!("per {p}"))));
        cols.push(text("").narrow());
        let n = breakdown.lines.len();
        Table::new(("cohort_rates", uid), cols).show(ui, n + 1, |row| {
            let Some(line) = breakdown.lines.get(row.index()) else {
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
                        num(ui, fmt::mono(fmt::amount(breakdown.total(p))).strong());
                    });
                }
                row.col(|_| {});
                return;
            };
            row.col(|ui| {
                let name = RichText::new(&line.name);
                let r = ui.label(if line.paused {
                    name.color(fmt::dim()).strikethrough()
                } else {
                    name
                });
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
            row.col(|ui| {
                if let Some(cohort) = uid {
                    if ui.small_button("Remove").clicked() {
                        let issuer = l.issuers.uid[line.issuer.get()];
                        ops.push((
                            Op::RemoveFromCohort { cohort, issuer },
                            "removing an issuer from a cohort",
                        ));
                    }
                }
            });
        });
        let paused = breakdown.paused_total(Period::Month);
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

    // --- add a member
    if let Some(cohort) = uid {
        let outside: Vec<IssuerIx> =
            l.issuers.indices().filter(|ix| !members.contains(ix)).collect();
        if !outside.is_empty() {
            ui.add_space(6.0);
            egui::ComboBox::from_id_salt("cohort_add")
                .selected_text("Add an issuer...")
                .width(240.0)
                .show_ui(ui, |ui| {
                    for ix in outside {
                        if ui.selectable_label(false, &l.issuers.name[ix.get()]).clicked() {
                            let issuer = l.issuers.uid[ix.get()];
                            ops.push((
                                Op::AddToCohort { cohort, issuer },
                                "adding an issuer to a cohort",
                            ));
                        }
                    }
                });
        }
    }

    // --- calendar
    ui.add_space(16.0);
    calendar(ui, s, &members);

    for (op, what) in ops {
        s.stage(vec![op], what);
    }
}

/// A month grid of every date the issuers fall due.
fn calendar(ui: &mut Ui, s: &mut Session, members: &[IssuerIx]) {
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
    let entries = cohort::calendar(l, members, month, last, today);

    ui.add_space(4.0);
    if s.calendar_list {
        payment_list(ui, l, &entries, today);
    } else {
        let busiest = (0..=last.0 - month.0)
            .map(|d| entries.iter().filter(|e| e.date.0 == month.0 + d).count())
            .max()
            .unwrap_or(0);
        let cell = egui::vec2(118.0, 72.0);
        egui::Grid::new("cohort_calendar").num_columns(7).spacing([4.0, 4.0]).show(ui, |ui| {
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
fn payment_list(ui: &mut Ui, l: &Budget, entries: &[CalendarEntry], today: Date) {
    if entries.is_empty() {
        ui.label(RichText::new("Nothing falls due this month.").color(fmt::dim()));
        return;
    }
    Table::new(
        "cohort_payments",
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
