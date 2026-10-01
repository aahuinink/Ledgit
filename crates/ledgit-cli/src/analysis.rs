//! Printing issuer rates, calendars and saved views.

use crate::show::{amt, truncate};
use ledgit_core::dues::{self, DueStatus};
use ledgit_core::id::IssuerIx;
use ledgit_core::prelude::*;
use ledgit_core::view;
use std::path::Path;

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Every listed issuer's rate in every unit, and what the running ones total.
pub fn rates(l: &Budget, issuers: &[IssuerIx]) {
    let lines = dues::rate_lines(l, issuers);
    if lines.is_empty() {
        println!("No issuers.");
        return;
    }
    print!("  {:<24} {:>12} {:<20}", "issuer", "amount", "schedule");
    for p in Period::ALL {
        print!(" {:>12}", format!("per {p}"));
    }
    println!();
    for line in &lines {
        print!(
            "  {:<24} {:>12} {:<20}",
            truncate(&line.name, 24),
            amt(line.amount),
            truncate(&line.schedule.describe(), 20)
        );
        for p in Period::ALL {
            print!(" {:>12}", line.rate(p).map_or("one-off".into(), amt));
        }
        println!("{}", if line.paused { "  (paused)" } else { "" });
    }
    print!("\n  {:<58}", "total, running issuers");
    for p in Period::ALL {
        print!(" {:>12}", amt(dues::total(&lines, p)));
    }
    println!();
    if lines.iter().any(|l| l.paused) {
        println!(
            "  paused issuers would add {} a month",
            amt(dues::paused_total(&lines, Period::Month))
        );
    }
    println!("\n  Rates are averages: a month is 30.44 days and a year 365.24.");
}

/// Every due date of `issuers` in a window, grouped by day.
pub fn calendar(l: &Budget, issuers: &[IssuerIx], from: Date, to: Date, today: Date) {
    let entries = dues::calendar(l, issuers, from, to, today);
    if entries.is_empty() {
        println!("Nothing falls due between {from} and {to}.");
        return;
    }
    let mut last: Option<Date> = None;
    let mut total = Money::ZERO;
    for e in &entries {
        let day = if last == Some(e.date) {
            String::new()
        } else {
            format!("{} {}", e.date, WEEKDAYS[e.date.weekday() as usize])
        };
        last = Some(e.date);
        let status = match e.status {
            DueStatus::Posted => "posted",
            DueStatus::Overdue => "OVERDUE - run issuers",
            DueStatus::Upcoming => "",
            DueStatus::Paused => "paused",
        };
        if e.status != DueStatus::Paused {
            total += e.amount;
        }
        println!(
            "{:<15} {:<28} {:>12}  {status}",
            day,
            truncate(&l.issuers.name[e.issuer.get()], 28),
            amt(e.amount)
        );
    }
    println!("\n{} occurrence(s), {} in total (paused excluded).", entries.len(), amt(total));
}

pub fn views(l: &Budget) {
    let live: Vec<_> = l.views.live().collect();
    if live.is_empty() {
        println!("No saved views.");
        return;
    }
    println!("{:<10} {:<28} looks at", "uid", "name");
    for ix in live {
        let v = l.views.get(ix);
        println!("{:<10} {:<28} {}", v.uid.short(), truncate(&v.name, 28), describe(&v.spec));
    }
}

/// "2 bucket(s), 1 issuer(s); 6 months back, 1 year ahead, per month".
pub fn describe(spec: &ViewSpec) -> String {
    let mut parts = Vec::new();
    let names = |n: usize, what: &str| (n > 0).then(|| format!("{n} {what}"));
    parts.extend(names(spec.buckets.len(), "bucket(s)"));
    parts.extend(names(spec.ledgers.len(), "ledger(s)"));
    parts.extend(names(spec.issuers.len(), "issuer(s)"));
    parts.extend(names(spec.transactions.len(), "filter(s)"));
    if parts.is_empty() {
        parts.push("nothing yet".into());
    }
    format!(
        "{}; {} back, {} ahead, per {}{}",
        parts.join(", "),
        spec.lookback,
        spec.horizon,
        spec.period,
        if spec.only_selected_issuers { "; simulates its own issuers only" } else { "" }
    )
}

/// Print a whole evaluated view.
pub fn view(l: &Budget, name: &str, r: &ViewReport) {
    println!("{name}: {} to {}, today {}\n", r.start, r.end, r.today);

    if !r.series.is_empty() {
        println!("Balances");
        println!(
            "  {:<24} {:>14} {:>14} {:>14}  {:<26} trend",
            "", "today", "at end", "change", "lowest ahead"
        );
        for s in &r.series {
            let (low_d, low) = s.lowest_ahead;
            println!(
                "  {:<24} {:>14} {:>14} {:>14}  {:<26} {}",
                truncate(&s.label, 24),
                amt(s.now),
                amt(s.at_end),
                amt(s.at_end - s.now),
                format!("{} on {low_d}", amt(low)),
                sparkline(s, r.start, r.end, 32)
            );
        }
        println!();
    }

    let p = r.period;
    if !r.flows.is_empty() {
        let heading =
            if r.directed { format!("in/out per {p}") } else { format!("volume per {p}") };
        println!("Flows");
        println!("  {:<24} {:<20} {:>12} {:>14}  notes", "issuer", "schedule", "each", heading);
        for f in &r.flows {
            let mut notes = Vec::new();
            if f.paused {
                notes.push("paused".to_string());
            } else if !f.simulated {
                notes.push("not simulated".to_string());
            }
            if f.effect.is_some_and(|e| e.is_zero()) {
                notes.push("does not move this view's money".to_string());
            }
            println!(
                "  {:<24} {:<20} {:>12} {:>14}  {}",
                truncate(&f.name, 24),
                truncate(&f.schedule.describe(), 20),
                amt(f.effect.unwrap_or(f.amount)),
                f.rate(p).map_or("one-off".into(), amt),
                notes.join("; ")
            );
        }
        let t = r.flow_totals(p);
        if r.directed {
            println!(
                "\n  per {p}: {} in, {} out, net {}",
                amt(t.received),
                amt(t.spent),
                amt(t.net())
            );
        } else {
            println!("\n  per {p}: {} moved", amt(t.volume));
        }
        println!();
    }

    println!("Timeline, per {p}");
    if r.directed {
        println!(
            "  {:<12} {:>12} {:>12} {:>12}   {:>12} {:>12} {:>12}",
            "", "in", "out", "net", "proj. in", "proj. out", "proj. net"
        );
    } else {
        println!("  {:<12} {:>12}   {:>12}", "", "volume", "proj. volume");
    }
    for row in &r.timeline {
        let (a, f) = (&row.actual, &row.projected);
        let label = period_label(p, row.start);
        if r.directed {
            println!(
                "  {:<12} {:>12} {:>12} {:>12}   {:>12} {:>12} {:>12}",
                label,
                blank(a.received),
                blank(a.spent),
                blank(a.net()),
                blank(f.received),
                blank(f.spent),
                blank(f.net()),
            );
        } else {
            println!("  {:<12} {:>12}   {:>12}", label, blank(a.volume), blank(f.volume));
        }
    }

    let mut notes = Vec::new();
    if r.overdue_occurrences > 0 {
        notes.push(format!(
            "{} overdue occurrence(s) are projected as landing today; run `ledgit issuer run` to post them.",
            r.overdue_occurrences
        ));
    }
    for ix in &r.cancelled {
        notes.push(format!(
            "{} is in both an added and a subtracted bucket, so it counts for nothing.",
            l.ledgers.name[ix.get()]
        ));
    }
    for b in &r.missing_buckets {
        notes.push(format!("bucket {} no longer exists on this branch.", b.short()));
    }
    if !notes.is_empty() {
        println!();
        for n in notes {
            println!("note: {n}");
        }
    }
}

fn blank(m: Money) -> String {
    if m.is_zero() {
        "-".into()
    } else {
        amt(m)
    }
}

fn period_label(p: Period, start: Date) -> String {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    match p {
        Period::Month => format!("{} {}", MONTHS[start.month() as usize - 1], start.year()),
        Period::Year => start.year().to_string(),
        Period::Week | Period::Day => start.to_string(),
    }
}

/// A terminal-width glimpse of a series: eight levels, sampled evenly.
fn sparkline(s: &Series, start: Date, end: Date, width: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let span = (end.0 - start.0).max(1) as i64;
    let samples: Vec<i64> = (0..width as i64)
        .map(|k| s.value_at(Date(start.0 + (span * k / (width as i64 - 1).max(1)) as i32)).cents())
        .collect();
    let (lo, hi) = samples.iter().fold((i64::MAX, i64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    samples
        .iter()
        .map(|v| if hi == lo { BARS[3] } else { BARS[((v - lo) * 7 / (hi - lo)) as usize] })
        .collect()
}

/// Every series as CSV: one row per date on which any of them changed.
pub fn csv(r: &ViewReport, path: &Path) -> Result<()> {
    std::fs::write(path, ledgit_plot::csv(r))
        .map_err(|e| Error::Store(format!("{}: {e}", path.display())))
}

pub fn evaluate(l: &Budget, spec: &ViewSpec, today: Date, until: Option<Date>) -> ViewReport {
    match until {
        Some(end) => view::evaluate_between(l, spec, today, spec.lookback.before(today), end),
        None => view::evaluate(l, spec, today),
    }
}
