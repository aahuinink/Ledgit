//! Charts of Ledgit views, drawn to SVG and rasterised to PNG.
//!
//! The GUI draws its live charts with egui; this crate draws the *exported*
//! ones - the file you print, attach, or keep. Both share the palette and the
//! axis arithmetic defined here, so a chart looks the same on screen and on
//! paper.
//!
//! Conventions, all deliberate:
//!
//! * balances are **steps**, not slopes - a balance holds its value until the
//!   next posting, and a diagonal between two paydays would invent money that
//!   never existed on the days in between;
//! * what has happened is a **solid** line, what the simulation expects is
//!   **dashed**, over a faintly shaded future - so the projection reads as a
//!   projection even in greyscale;
//! * series colours come from one fixed, colour-blind-checked order and follow
//!   the series, never its rank;
//! * exports are light-mode. They are for printing.

use ledgit_core::date::Date;
use ledgit_core::money::Money;
use ledgit_core::view::ViewReport;
use std::fmt::Write as _;
use std::path::Path;

// ---------------------------------------------------------------- palette

/// Categorical series colours, light and dark steps of the same eight hues.
/// The order is the colour-blindness safeguard: adjacent slots stay
/// distinguishable under the common deficiencies. Never cycle it; a ninth
/// series is folded away rather than given a generated colour.
pub const SERIES_LIGHT: [[u8; 3]; 8] = [
    [0x2a, 0x78, 0xd6],
    [0xeb, 0x68, 0x34],
    [0x1b, 0xaf, 0x7a],
    [0xed, 0xa1, 0x00],
    [0xe8, 0x7b, 0xa4],
    [0x00, 0x83, 0x00],
    [0x4a, 0x3a, 0xa7],
    [0xe3, 0x49, 0x48],
];
pub const SERIES_DARK: [[u8; 3]; 8] = [
    [0x39, 0x87, 0xe5],
    [0xd9, 0x59, 0x26],
    [0x19, 0x9e, 0x70],
    [0xc9, 0x85, 0x00],
    [0xd5, 0x51, 0x81],
    [0x00, 0x83, 0x00],
    [0x90, 0x85, 0xe9],
    [0xe6, 0x67, 0x67],
];
pub const MAX_SERIES: usize = SERIES_LIGHT.len();

const SURFACE: &str = "#fcfcfb";
const INK: &str = "#0b0b0b";
const INK_SECONDARY: &str = "#52514e";
const INK_MUTED: &str = "#898781";
const GRID: &str = "#e1e0d9";
const AXIS: &str = "#c3c2b7";

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

// ------------------------------------------------------------------ model

/// One line on a chart.
#[derive(Clone, Debug)]
pub struct Line {
    pub label: String,
    /// Step points: the value holds from each date until the next.
    pub points: Vec<(Date, Money)>,
}

#[derive(Clone, Debug)]
pub struct Chart {
    pub title: String,
    pub subtitle: String,
    pub lines: Vec<Line>,
    /// Where history ends and the projection begins, if the chart has one.
    pub today: Option<Date>,
    pub start: Date,
    pub end: Date,
    /// Lines beyond [`MAX_SERIES`] that were left off. Said on the chart, so
    /// a missing line is never silent.
    pub folded: usize,
}

impl Chart {
    /// The balance chart for an evaluated view.
    pub fn from_view(title: &str, r: &ViewReport) -> Chart {
        let lines: Vec<Line> = r
            .series
            .iter()
            .map(|s| Line { label: s.label.clone(), points: s.points.clone() })
            .collect();
        let folded = lines.len().saturating_sub(MAX_SERIES);
        Chart {
            title: title.to_string(),
            subtitle: format!(
                "{} to {} \u{b7} projected from {} using {} issuer{}",
                r.start,
                r.end,
                r.today,
                r.simulated_issuers,
                if r.simulated_issuers == 1 { "" } else { "s" }
            ),
            lines: lines.into_iter().take(MAX_SERIES).collect(),
            today: Some(r.today),
            start: r.start,
            end: r.end,
            folded,
        }
    }

    /// Write the chart to `path`, as PNG or SVG by its extension.
    pub fn save(&self, path: &Path, width: u32, height: u32) -> Result<(), String> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let bytes = match ext.as_str() {
            "svg" => self.svg(width, height).into_bytes(),
            "png" => self.png(width, height)?,
            _ => return Err(format!("{}: save as .svg or .png", path.display())),
        };
        std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Render to a PNG at twice the nominal size, so it stays sharp when
    /// printed or viewed on a high-density screen.
    pub fn png(&self, width: u32, height: u32) -> Result<Vec<u8>, String> {
        use resvg::{tiny_skia, usvg};
        let mut opt = usvg::Options { font_family: FONT_FAMILY_PNG.into(), ..Default::default() };
        opt.fontdb_mut().load_font_data(epaint_default_fonts::UBUNTU_LIGHT.to_vec());
        let tree =
            usvg::Tree::from_str(&self.svg(width, height), &opt).map_err(|e| e.to_string())?;
        let scale = 2.0;
        let mut pixmap = tiny_skia::Pixmap::new(width * 2, height * 2)
            .ok_or_else(|| "chart size must be positive".to_string())?;
        resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        pixmap.encode_png().map_err(|e| e.to_string())
    }

    /// Render to a self-contained SVG document.
    pub fn svg(&self, width: u32, height: u32) -> String {
        let (w, h) = (width as f64, height as f64);
        let many = self.lines.len() >= 2;
        let direct = many && self.lines.len() <= 4;

        // Frame: title and subtitle, then a legend row, then the plot.
        let left = 76.0;
        let right = if direct { 150.0 } else { 28.0 };
        let top = if many { 92.0 } else { 70.0 };
        let bottom = 36.0;
        let (px0, px1, py0, py1) = (left, w - right, top, h - bottom);

        let mut s = String::new();
        let _ = write!(
            s,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" font-family="{FONT_FAMILY_SVG}">"#
        );
        let _ = write!(s, r#"<rect width="100%" height="100%" fill="{SURFACE}"/>"#);
        let _ = write!(
            s,
            r#"<text x="{left}" y="28" font-size="17" font-weight="600" fill="{INK}">{}</text>"#,
            esc(&self.title)
        );
        let mut sub = self.subtitle.clone();
        if self.folded > 0 {
            let _ = write!(sub, " \u{b7} {} more line(s) not drawn", self.folded);
        }
        let _ = write!(
            s,
            r#"<text x="{left}" y="48" font-size="12" fill="{INK_SECONDARY}">{}</text>"#,
            esc(&sub)
        );

        let values = self.lines.iter().flat_map(|l| l.points.iter().map(|p| p.1.cents()));
        let (lo, hi) = values.fold((i64::MAX, i64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        if lo > hi {
            let _ = write!(
                s,
                r#"<text x="{}" y="{}" font-size="13" fill="{INK_MUTED}" text-anchor="middle">Nothing to chart</text></svg>"#,
                w / 2.0,
                h / 2.0
            );
            return s;
        }
        let ticks = nice_ticks(lo as f64 / 100.0, hi as f64 / 100.0, 6);
        let (y_lo, y_hi) = (ticks[0], *ticks.last().expect("at least two ticks"));
        let x = |d: Date| {
            let span = (self.end.0 - self.start.0).max(1) as f64;
            px0 + (d.0 - self.start.0) as f64 / span * (px1 - px0)
        };
        let y = |cents: i64| py1 - (cents as f64 / 100.0 - y_lo) / (y_hi - y_lo) * (py1 - py0);

        // Future: a faint wash, and a hairline at today.
        if let Some(t) = self.today.filter(|t| *t >= self.start && *t < self.end) {
            let tx = x(t);
            let _ = write!(
                s,
                r#"<rect x="{tx:.1}" y="{py0}" width="{:.1}" height="{:.1}" fill="{INK}" fill-opacity="0.035"/>"#,
                px1 - tx,
                py1 - py0
            );
            let _ = write!(
                s,
                r#"<line x1="{tx:.1}" x2="{tx:.1}" y1="{py0}" y2="{py1}" stroke="{AXIS}" stroke-width="1"/><text x="{:.1}" y="{:.1}" font-size="11" fill="{INK_MUTED}">projected from today</text>"#,
                tx + 6.0,
                py0 + 14.0
            );
        }

        // Grid and y labels. Solid hairlines; the zero line one step stronger.
        for v in &ticks {
            let cy = y((v * 100.0).round() as i64);
            let stroke = if *v == 0.0 { AXIS } else { GRID };
            let _ = write!(
                s,
                r#"<line x1="{px0}" x2="{px1}" y1="{cy:.1}" y2="{cy:.1}" stroke="{stroke}" stroke-width="1"/><text x="{:.1}" y="{:.1}" font-size="11" fill="{INK_MUTED}" text-anchor="end">{}</text>"#,
                px0 - 8.0,
                cy + 4.0,
                money_short(*v)
            );
        }
        // X labels.
        for (d, label) in date_ticks(self.start, self.end) {
            let _ = write!(
                s,
                r#"<text x="{:.1}" y="{:.1}" font-size="11" fill="{INK_MUTED}" text-anchor="middle">{label}</text>"#,
                x(d),
                py1 + 20.0
            );
        }

        // Lines: solid to today, dashed after.
        let split = self.today.unwrap_or(self.end);
        for (i, line) in self.lines.iter().enumerate() {
            let colour = hex(SERIES_LIGHT[i]);
            let (past, future) = step_paths(&line.points, split, &x, &y);
            for (d, dash) in [(past, ""), (future, r#" stroke-dasharray="6 4""#)] {
                if !d.is_empty() {
                    let _ = write!(
                        s,
                        r#"<path d="{d}" fill="none" stroke="{colour}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"{dash}/>"#
                    );
                }
            }
        }

        // Legend: always, for two or more lines.
        if many {
            let mut lx = left;
            for (i, line) in self.lines.iter().enumerate() {
                let _ = write!(
                    s,
                    r#"<line x1="{lx}" x2="{:.1}" y1="70" y2="70" stroke="{}" stroke-width="3" stroke-linecap="round"/><text x="{:.1}" y="74" font-size="12" fill="{INK_SECONDARY}">{}</text>"#,
                    lx + 16.0,
                    hex(SERIES_LIGHT[i]),
                    lx + 22.0,
                    esc(&line.label)
                );
                lx += 34.0 + 7.0 * line.label.chars().count() as f64;
            }
        }

        // Direct labels at each line's end, nudged apart so they never collide.
        if direct {
            let mut ends: Vec<(f64, usize)> = self
                .lines
                .iter()
                .enumerate()
                .filter_map(|(i, l)| l.points.last().map(|p| (y(p.1.cents()), i)))
                .collect();
            ends.sort_by(|a, b| a.0.total_cmp(&b.0));
            for k in 1..ends.len() {
                ends[k].0 = ends[k].0.max(ends[k - 1].0 + 15.0);
            }
            for (ly, i) in ends {
                let line = &self.lines[i];
                let last = line.points.last().expect("filtered on last").1;
                let _ = write!(
                    s,
                    r#"<circle cx="{:.1}" cy="{ly:.1}" r="3.5" fill="{}"/><text x="{:.1}" y="{:.1}" font-size="11" fill="{INK_SECONDARY}">{} {}</text>"#,
                    px1 + 10.0,
                    hex(SERIES_LIGHT[i]),
                    px1 + 18.0,
                    ly + 4.0,
                    esc(&clip(&line.label, 12)),
                    money_short(last.cents() as f64 / 100.0)
                );
            }
        }

        s.push_str("</svg>");
        s
    }
}

/// egui names its bundled font "Ubuntu-Light"; the SVG asks for the system
/// UI face first, and the PNG renderer falls back to the bundled one.
const FONT_FAMILY_SVG: &str = "system-ui, 'Segoe UI', Ubuntu, sans-serif";
const FONT_FAMILY_PNG: &str = "Ubuntu";

/// Two SVG path strings for a step line: up to `split`, and from it on.
fn step_paths(
    points: &[(Date, Money)],
    split: Date,
    x: &impl Fn(Date) -> f64,
    y: &impl Fn(i64) -> f64,
) -> (String, String) {
    let mut past = String::new();
    let mut future = String::new();
    let mut prev: Option<(f64, f64)> = None;
    for (d, v) in points {
        let (cx, cy) = (x(*d), y(v.cents()));
        let out = if *d <= split { &mut past } else { &mut future };
        match prev {
            None => {
                let _ = write!(out, "M{cx:.1} {cy:.1}");
            }
            Some((_, py)) => {
                if out.is_empty() {
                    // The first projected segment starts where history ended.
                    let (px, _) = prev.expect("matched Some");
                    let _ = write!(out, "M{px:.1} {py:.1}");
                }
                let _ = write!(out, "H{cx:.1}V{cy:.1}");
            }
        }
        prev = Some((cx, cy));
    }
    (past, future)
}

// ----------------------------------------------------------------- axes

/// Round tick values spanning `lo..=hi`, about `target` of them, always at
/// least two. Steps are 1, 2 or 5 times a power of ten, as on any printed
/// chart, so the labels read as round money.
pub fn nice_ticks(lo: f64, hi: f64, target: usize) -> Vec<f64> {
    let (lo, hi) = if (hi - lo).abs() < f64::EPSILON {
        let pad = if lo == 0.0 { 1.0 } else { lo.abs() * 0.1 };
        (lo - pad, hi + pad)
    } else {
        (lo, hi)
    };
    let raw = (hi - lo) / target.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step =
        [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag);
    let first = (lo / step).floor() * step;
    // Step until the top tick covers `hi`, so no line ever leaves the plot.
    let mut out = Vec::new();
    let mut k = 0.0;
    loop {
        let v = first + k * step;
        // Snap away float noise so labels never read -0 or 1999.9999.
        out.push(if v.abs() < step * 1e-9 { 0.0 } else { v });
        if (v >= hi - step * 1e-9 && out.len() >= 2) || out.len() >= 50 {
            break;
        }
        k += 1.0;
    }
    out
}

/// "$1.2k", "-$350", "$2.5M": short enough for an axis.
pub fn money_short(v: f64) -> String {
    let sign = if v < 0.0 { "-" } else { "" };
    let a = v.abs();
    let body = if a >= 1_000_000.0 {
        trim(format!("{:.2}", a / 1_000_000.0)) + "M"
    } else if a >= 10_000.0 {
        trim(format!("{:.0}", a / 1_000.0)) + "k"
    } else if a >= 1_000.0 {
        trim(format!("{:.1}", a / 1_000.0)) + "k"
    } else {
        trim(format!("{a:.0}"))
    };
    format!("{sign}${body}")
}

fn trim(s: String) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// First-of-month ticks between two dates, thinned to about ten. A January
/// tick, or the first one, carries the year.
pub fn date_ticks(start: Date, end: Date) -> Vec<(Date, String)> {
    let months = (end.year() - start.year()) * 12 + end.month() as i32 - start.month() as i32;
    if months < 2 {
        // Short windows: weekly ticks, on Mondays.
        let mut d = start.add_days((7 - start.weekday() as i32) % 7);
        let mut out = Vec::new();
        while d <= end {
            out.push((d, format!("{} {}", MONTHS[d.month() as usize - 1], d.day())));
            d = d.add_days(7);
        }
        return out;
    }
    let every = [1, 2, 3, 6, 12, 24, 60].into_iter().find(|m| months / m <= 10).unwrap_or(120);
    let mut d = Date::from_ymd(start.year(), start.month(), 1).expect("the 1st exists");
    if d < start {
        d = d.add_months(1);
    }
    // Align to the step so quarters land on Jan/Apr/Jul/Oct.
    while (d.month() as i32 - 1) % every.min(12) != 0 {
        d = d.add_months(1);
    }
    let mut out = Vec::new();
    while d <= end {
        let m = MONTHS[d.month() as usize - 1];
        let label = if every >= 12 {
            d.year().to_string()
        } else if out.is_empty() || d.month() == 1 {
            format!("{m} {}", d.year())
        } else {
            m.to_string()
        };
        out.push((d, label));
        d = d.add_months(every);
    }
    out
}

/// Every series of a view as CSV: one row per date on which any of them
/// changed, with a column saying whether that row is projected.
pub fn csv(r: &ViewReport) -> String {
    let mut dates: Vec<Date> = r.series.iter().flat_map(|s| s.points.iter().map(|p| p.0)).collect();
    dates.sort_unstable();
    dates.dedup();
    let mut out = String::from("date,projected");
    for s in &r.series {
        let _ = write!(out, ",\"{}\"", s.label.replace('"', "\"\""));
    }
    out.push('\n');
    for d in dates {
        let _ = write!(out, "{d},{}", d > r.today);
        for s in &r.series {
            let _ = write!(out, ",{}", s.value_at(d));
        }
        out.push('\n');
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "\u{2026}"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> Date {
        Date::from_ymd(y, m, day).unwrap()
    }

    fn chart(lines: usize) -> Chart {
        let pts = |k: i64| {
            vec![
                (d(2024, 1, 1), Money::from_major(1_000 * k)),
                (d(2024, 3, 1), Money::from_major(1_500 * k)),
                (d(2024, 3, 10), Money::from_major(1_500 * k)),
                (d(2024, 5, 1), Money::from_major(-200 * k)),
                (d(2024, 9, 1), Money::from_major(-200 * k)),
            ]
        };
        Chart {
            title: "Net <worth> & more".into(),
            subtitle: "test".into(),
            lines: (1..=lines as i64)
                .map(|k| Line { label: format!("L{k}"), points: pts(k) })
                .collect(),
            today: Some(d(2024, 3, 10)),
            start: d(2024, 1, 1),
            end: d(2024, 9, 1),
            folded: 0,
        }
    }

    #[test]
    fn ticks_are_round_and_cover_the_range() {
        let t = nice_ticks(-200.0, 1_500.0, 6);
        assert!(t[0] <= -200.0 && *t.last().unwrap() >= 1_500.0, "{t:?}");
        assert!(t.contains(&0.0));
        assert!(t.windows(2).all(|w| (w[1] - w[0] - 500.0).abs() < 1e-9), "{t:?}");
        // The top tick must cover the maximum, not stop short of it.
        let big = nice_ticks(-980.0, 54_050.0, 6);
        assert!(*big.last().unwrap() >= 54_050.0, "{big:?}");
        // A flat line still gets a usable axis.
        assert!(nice_ticks(5.0, 5.0, 6).len() >= 2);
        assert!(nice_ticks(0.0, 0.0, 6).len() >= 2);
    }

    #[test]
    fn money_labels_are_short() {
        assert_eq!(money_short(0.0), "$0");
        assert_eq!(money_short(1_500.0), "$1.5k");
        assert_eq!(money_short(-2_000.0), "-$2k");
        assert_eq!(money_short(25_000.0), "$25k");
        assert_eq!(money_short(3_250_000.0), "$3.25M");
    }

    #[test]
    fn date_ticks_thin_out_over_long_windows() {
        let t = date_ticks(d(2024, 1, 1), d(2024, 12, 31));
        assert_eq!(t[0].1, "Jan 2024");
        assert!(t.len() <= 12);
        let long = date_ticks(d(2020, 1, 1), d(2030, 1, 1));
        assert!(long.len() <= 11, "{long:?}");
        assert!(long.iter().all(|(d, _)| d.month() == 1));
        let short = date_ticks(d(2024, 3, 1), d(2024, 3, 31));
        assert!(!short.is_empty() && short.iter().all(|(d, _)| d.weekday() == 0));
    }

    #[test]
    fn the_projection_is_drawn_dashed_and_labels_are_escaped() {
        let svg = chart(2).svg(900, 420);
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains("Net &lt;worth&gt; &amp; more"));
        assert_eq!(svg.matches("stroke-dasharray").count(), 2, "one dashed tail per line");
        assert!(svg.contains("projected from today"));
    }

    #[test]
    fn a_single_line_has_no_legend() {
        let one = chart(1).svg(900, 420);
        assert!(!one.contains(">L1 "), "no direct label for a lone line");
        let two = chart(2).svg(900, 420);
        assert!(two.contains(">L1<"), "legend entry");
    }

    #[test]
    fn png_renders_with_the_bundled_font() {
        let png = chart(3).png(600, 300).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(png.len() > 1_000);
    }

    #[test]
    fn saving_picks_the_format_from_the_extension() {
        let dir = std::env::temp_dir();
        let svg = dir.join(format!("ledgit-plot-{}.svg", std::process::id()));
        chart(2).save(&svg, 800, 400).unwrap();
        assert!(std::fs::read_to_string(&svg).unwrap().starts_with("<svg"));
        let _ = std::fs::remove_file(&svg);
        assert!(chart(2).save(&dir.join("x.gif"), 800, 400).is_err());
    }
}
