//! Application shell: window chrome, navigation, and the session that owns the
//! open budget.
//!
//! The rule the whole UI follows: **every view reads `repo.working()`**, which
//! is committed history plus whatever is staged. So an entry shows up in the
//! dashboard, the register and the bucket totals the instant it is made, and
//! the Commit screen is the only place that talks about what is permanent.

use crate::fmt;
use crate::forms::{FormKind, Forms, Outcome};
use crate::views;
use egui::{Color32, RichText};
use ledgit_core::prelude::*;
use ledgit_sqlite::SqliteStore;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Dashboard,
    Ledgers,
    Register,
    Transactions,
    Issuers,
    Buckets,
    Commit,
    History,
}

impl View {
    const NAV: [(View, &'static str); 7] = [
        (View::Dashboard, "Dashboard"),
        (View::Ledgers, "Ledgers"),
        (View::Transactions, "Transactions"),
        (View::Issuers, "Issuers"),
        (View::Buckets, "Buckets"),
        (View::Commit, "Commit"),
        (View::History, "History"),
    ];
}

pub struct Status {
    pub message: String,
    pub is_error: bool,
}

pub struct Session {
    pub repo: Repo<SqliteStore>,
    pub path: PathBuf,
    pub view: View,
    pub search: String,
    pub selected_ledger: Option<LedgerUid>,
    pub selected_bucket: Option<BucketUid>,
    pub selected_commit: Option<CommitId>,
    pub commit_message: String,
    pub issuer_through: String,
    pub new_branch: String,
    pub rebase_onto: String,
    pub status: Option<Status>,
    pub forms: Forms,
    pub pins: Vec<LedgerUid>,
    /// Set by a view to ask for a jump after the frame is done drawing.
    pub goto: Option<View>,

    // --- view state. Kept on the session rather than inside the views so that
    // navigating away and back does not silently reset a filter you set.
    pub ledger_sort: LedgerSort,
    pub bucket_roll: RollUp,
    pub tx_from: String,
    pub tx_to: String,
    pub tx_ledger: Option<LedgerUid>,
    pub tx_source: crate::views::transactions::SourceFilter,
}

impl Session {
    pub fn open(path: PathBuf, author: &str) -> Result<Session> {
        let repo = Repo::open(SqliteStore::open(&path)?, author)?;
        Ok(Session::new(repo, path))
    }

    /// Wrap an already-open repo. Split out so the smoke tests can drive every
    /// screen against an in-memory budget.
    pub fn new(repo: Repo<SqliteStore>, path: PathBuf) -> Session {
        Session {
            repo,
            path,
            view: View::Dashboard,
            search: String::new(),
            selected_ledger: None,
            selected_bucket: None,
            selected_commit: None,
            commit_message: String::new(),
            issuer_through: Date::today_utc().to_string(),
            new_branch: String::new(),
            rebase_onto: String::new(),
            status: None,
            forms: Forms::default(),
            pins: Vec::new(),
            goto: None,
            ledger_sort: LedgerSort::Name,
            bucket_roll: RollUp::ByNormality,
            tx_from: String::new(),
            tx_to: String::new(),
            tx_ledger: None,
            tx_source: crate::views::transactions::SourceFilter::default(),
        }
    }

    pub fn note(&mut self, message: impl Into<String>) {
        self.status = Some(Status { message: message.into(), is_error: false });
    }

    pub fn fail(&mut self, e: impl std::fmt::Display) {
        self.status = Some(Status { message: e.to_string(), is_error: true });
    }

    /// Stage ops, reporting the outcome in the status bar. Returns whether it
    /// worked, so callers can clear their own inputs only on success.
    pub fn stage(&mut self, ops: Vec<Op>, what: &str) -> bool {
        let n = ops.len();
        match self.repo.stage_all(ops) {
            Ok(()) => {
                self.note(format!(
                    "Staged {what} ({n} change(s)). Review it on the Commit screen."
                ));
                true
            }
            Err(e) => {
                self.fail(e);
                false
            }
        }
    }

    pub fn budget(&self) -> &Budget {
        self.repo.working()
    }

    pub fn toggle_pin(&mut self, uid: LedgerUid) {
        match self.pins.iter().position(|p| *p == uid) {
            Some(i) => {
                self.pins.remove(i);
            }
            None => self.pins.push(uid),
        }
    }

    pub fn is_pinned(&self, uid: LedgerUid) -> bool {
        self.pins.contains(&uid)
    }
}

pub struct LedgitApp {
    session: Option<Session>,
    author: String,
    /// Pinned ledgers, per budget file. Purely a UI preference, so it lives in
    /// eframe's storage rather than in the budget - pinning something is not a
    /// fact about your money and has no business in the commit history.
    pins: HashMap<String, Vec<String>>,
    recent: Vec<String>,
    /// Last zoom factor, mirrored out of the egui context so `save` can reach
    /// it without a `Context`. Like pins, it is a preference about eyesight,
    /// not a fact about money, so it lives in eframe's storage.
    zoom: f32,
    startup_error: Option<String>,
}

impl LedgitApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial: Option<PathBuf>,
        author: String,
    ) -> LedgitApp {
        let (pins, recent, zoom) = match cc.storage {
            Some(s) => (
                eframe::get_value(s, "pins").unwrap_or_default(),
                eframe::get_value(s, "recent").unwrap_or_default(),
                eframe::get_value(s, "zoom").unwrap_or(1.0),
            ),
            None => (HashMap::new(), Vec::new(), 1.0),
        };
        // A stored zoom from an older build could be anything; clamp it rather
        // than trust it, or one bad value leaves the app unreadable on start.
        let zoom = clamp_zoom(zoom);
        cc.egui_ctx.set_zoom_factor(zoom);
        let mut app = LedgitApp { session: None, author, pins, recent, zoom, startup_error: None };
        if let Some(p) = initial {
            app.open_path(p);
        }
        app
    }

    fn open_path(&mut self, path: PathBuf) {
        match Session::open(path.clone(), &self.author) {
            Ok(mut s) => {
                let key = path.to_string_lossy().to_string();
                s.pins = self
                    .pins
                    .get(&key)
                    .map(|v| v.iter().filter_map(|t| LedgerUid::parse(t)).collect())
                    .unwrap_or_default();
                // Drop pins for ledgers that are not on this branch.
                let budget_ledgers: Vec<LedgerUid> = s.budget().ledgers.uid.clone();
                s.pins.retain(|p| budget_ledgers.contains(p));
                self.recent.retain(|r| *r != key);
                self.recent.insert(0, key);
                self.recent.truncate(8);
                self.session = Some(s);
                self.startup_error = None;
            }
            Err(e) => self.startup_error = Some(format!("{}: {e}", path.display())),
        }
    }

    fn remember_pins(&mut self) {
        if let Some(s) = &self.session {
            self.pins.insert(
                s.path.to_string_lossy().to_string(),
                s.pins.iter().map(|p| p.to_string()).collect(),
            );
        }
    }
}

impl eframe::App for LedgitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        apply_zoom(ctx);
        // Read it back rather than tracking it ourselves: this also picks up
        // Ctrl+Plus/Minus/0, which egui handles on its own.
        self.zoom = ctx.zoom_factor();
        match &mut self.session {
            None => self.welcome(ctx),
            Some(_) => self.workspace(ctx),
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.remember_pins();
        eframe::set_value(storage, "pins", &self.pins);
        eframe::set_value(storage, "recent", &self.recent);
        eframe::set_value(storage, "zoom", &self.zoom);
    }
}

impl LedgitApp {
    fn welcome(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| {
                ui.heading("Ledgit");
                ui.label(
                    RichText::new("A budget you can commit, branch and revert.").color(fmt::dim()),
                );
                ui.add_space(24.0);

                if ui.button("Open a budget...").clicked() {
                    if let Some(p) =
                        rfd::FileDialog::new().add_filter("Ledgit budget", &["ledgit"]).pick_file()
                    {
                        self.open_path(p);
                    }
                }
                if ui.button("Create a new budget...").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Ledgit budget", &["ledgit"])
                        .set_file_name("budget.ledgit")
                        .save_file()
                    {
                        self.open_path(p);
                    }
                }

                if !self.recent.is_empty() {
                    ui.add_space(24.0);
                    ui.label(RichText::new("Recent").color(fmt::dim()));
                    let recent = self.recent.clone();
                    for r in recent {
                        if ui.link(&r).clicked() {
                            self.open_path(PathBuf::from(r));
                        }
                    }
                }

                if let Some(e) = &self.startup_error {
                    ui.add_space(16.0);
                    ui.colored_label(fmt::bad(), e);
                }
            });
        });
    }

    fn workspace(&mut self, ctx: &egui::Context) {
        // Take the session out so views get `&mut Session` without fighting the
        // borrow checker over `self`.
        let mut session = self.session.take().expect("workspace only runs with a session");

        top_bar(ctx, &mut session);
        nav_panel(ctx, &mut session);
        status_bar(ctx, &mut session);

        egui::CentralPanel::default().show(ctx, |ui| {
            // A search bar with text in it beats whatever tab is selected;
            // that is what people expect a search bar to do.
            if !session.search.trim().is_empty() && session.view != View::Transactions {
                views::search::show(ui, &mut session);
                return;
            }
            match session.view {
                View::Dashboard => views::dashboard::show(ui, &mut session),
                View::Ledgers => views::ledgers::show(ui, &mut session),
                View::Register => views::ledgers::register(ui, &mut session),
                View::Transactions => views::transactions::show(ui, &mut session),
                View::Issuers => views::issuers::show(ui, &mut session),
                View::Buckets => views::buckets::show(ui, &mut session),
                View::Commit => views::commit::show(ui, &mut session),
                View::History => views::history::show(ui, &mut session),
            }
        });

        show_form_modal(ctx, &mut session);

        if let Some(v) = session.goto.take() {
            session.view = v;
        }
        self.session = Some(session);
        self.remember_pins();
    }
}

/// Zoom limits. egui itself allows 0.2x-5.0x, but a dense table of figures is
/// illegible long before either end, so the usable range is tightened here.
const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 3.0;

fn clamp_zoom(zoom: f32) -> f32 {
    // `clamp` panics on NaN, and a corrupt stored value is exactly where a NaN
    // would come from, so screen for it first.
    if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    }
}

/// Fold Ctrl+scroll (and trackpad pinch) into the global zoom factor.
///
/// egui already recognises the gesture: it folds Ctrl+scroll into `zoom_delta`
/// and withholds that delta from the scroll areas, so a zooming wheel does not
/// also scroll the page underneath. What egui deliberately does not do is act
/// on it, because only the app knows what a sane zoom range is. Ctrl+Plus,
/// Ctrl+Minus and Ctrl+0 are handled by egui without our help.
fn apply_zoom(ctx: &egui::Context) {
    let delta = ctx.input(|i| i.zoom_delta());
    if delta == 1.0 {
        return;
    }
    let current = ctx.zoom_factor();
    let wanted = clamp_zoom(current * delta);
    // egui spreads one wheel notch over several frames, so this runs repeatedly.
    // That compounds correctly - `set_zoom_factor` stages the value and egui
    // applies it at the start of the next pass, so `current` is up to date each
    // time - but once clamped the delta keeps arriving with nothing left to do.
    // Writing the same factor back would request a repaint on every frame the
    // user holds Ctrl and keeps scrolling at the limit.
    if wanted != current {
        ctx.set_zoom_factor(wanted);
    }
}

fn top_bar(ctx: &egui::Context, s: &mut Session) {
    egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let name = s
                .path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "budget".into());
            ui.heading(name);
            ui.label(RichText::new(format!("on {}", s.repo.head())).color(fmt::dim()));

            ui.separator();
            ui.menu_button("New", |ui| {
                for (kind, label) in [
                    (FormKind::Transaction, "Transaction"),
                    (FormKind::Ledger, "Ledger"),
                    (FormKind::Bucket, "Bucket"),
                    (FormKind::Issuer, "Issuer"),
                ] {
                    if ui.button(label).clicked() {
                        s.forms.open(kind, s.repo.working());
                        ui.close();
                    }
                }
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let staged = s.repo.staged().len();
                let text = if staged == 0 {
                    RichText::new("nothing staged").color(fmt::dim())
                } else {
                    RichText::new(format!("{staged} staged")).color(Color32::from_rgb(220, 170, 60))
                };
                if ui.button(text).clicked() {
                    s.goto = Some(View::Commit);
                }
                ui.separator();
                ui.add(
                    egui::TextEdit::singleline(&mut s.search)
                        .hint_text("Search everything")
                        .desired_width(240.0),
                );
            });
        });
        ui.add_space(4.0);
    });
}

fn nav_panel(ctx: &egui::Context, s: &mut Session) {
    egui::SidePanel::left("nav").resizable(false).exact_width(180.0).show(ctx, |ui| {
        ui.add_space(8.0);
        for (view, label) in View::NAV {
            let selected = s.view == view || (view == View::Ledgers && s.view == View::Register);
            if ui.selectable_label(selected, label).clicked() {
                s.view = view;
            }
        }

        if !s.pins.is_empty() {
            ui.add_space(16.0);
            ui.label(RichText::new("PINNED").small().color(fmt::dim()));
            ui.separator();
            let pins = s.pins.clone();
            for uid in pins {
                let Some(ix) = s.budget().ledgers.ix(uid) else { continue };
                let name = s.budget().ledgers.name[ix.get()].clone();
                let balance = s.budget().ledgers.balance(ix);
                let clicked = ui
                    .vertical(|ui| {
                        let r = ui.selectable_label(s.selected_ledger == Some(uid), name);
                        ui.label(fmt::money_text(balance).small());
                        r.clicked()
                    })
                    .inner;
                if clicked {
                    s.selected_ledger = Some(uid);
                    s.view = View::Register;
                }
            }
        }
    });
}

fn status_bar(ctx: &egui::Context, s: &mut Session) {
    egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            match &s.status {
                Some(st) => {
                    let color = if st.is_error { fmt::bad() } else { fmt::good() };
                    ui.colored_label(color, &st.message);
                }
                None => {
                    ui.label(RichText::new(s.path.to_string_lossy()).color(fmt::dim()).small());
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if s.status.is_some() && ui.small_button("dismiss").clicked() {
                    s.status = None;
                }
                let l = s.budget();
                ui.label(
                    RichText::new(format!(
                        "{} ledgers  ·  {} transactions",
                        l.ledgers.len(),
                        l.transactions.len()
                    ))
                    .color(fmt::dim())
                    .small(),
                );
                // Only shown once zoomed: otherwise it is chrome that tells you
                // nothing. It doubles as the way back to 100%.
                let zoom = ui.ctx().zoom_factor();
                if (zoom - 1.0).abs() > 0.005 {
                    ui.separator();
                    if ui
                        .small_button(format!("{:.0}%", zoom * 100.0))
                        .on_hover_text("Ctrl+scroll to zoom, Ctrl+0 to reset. Click to reset.")
                        .clicked()
                    {
                        ui.ctx().set_zoom_factor(1.0);
                    }
                }
            });
        });
        ui.add_space(2.0);
    });
}

fn show_form_modal(ctx: &egui::Context, s: &mut Session) {
    let Some(kind) = s.forms.open else { return };
    let response = egui::Modal::new(egui::Id::new("form_modal")).show(ctx, |ui| {
        ui.set_width(kind.width());
        ui.heading(kind.title());
        ui.add_space(8.0);

        // The budget is read while the form is drawn, and staging happens after,
        // so clone nothing and borrow nothing across the call.
        let outcome = {
            let budget = s.repo.working().clone();
            s.forms.show(ui, &budget)
        };
        if let Some(err) = &s.forms.error {
            ui.add_space(6.0);
            ui.colored_label(fmt::bad(), err);
        }
        outcome
    });

    match response.inner {
        Outcome::Submit(ops) => {
            let what = kind.title().to_lowercase().replace("new ", "");
            s.stage(ops, &what);
        }
        Outcome::Cancelled => s.forms.open = None,
        Outcome::Pending => {}
    }
}

/// Path of the budget a fresh session should open, if any.
pub fn initial_path() -> Option<PathBuf> {
    std::env::args().nth(1).map(PathBuf::from).or_else(|| {
        std::env::var_os("LEDGIT_BUDGET")
            .map(PathBuf::from)
            .filter(|p: &PathBuf| Path::new(p).exists())
    })
}

#[cfg(test)]
mod tests {
    use super::{apply_zoom, clamp_zoom, MAX_ZOOM, MIN_ZOOM};

    /// Drive one real egui pass containing a single wheel event.
    ///
    /// `MouseWheelUnit::Point` with a small delta is the trackpad-shaped path,
    /// which egui applies within the same pass. `Line` would be spread across
    /// several frames by egui's smoothing and make the assertions depend on
    /// wall-clock timing.
    fn wheel(ctx: &egui::Context, modifiers: egui::Modifiers, amount: f32) {
        let input = egui::RawInput {
            events: vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, amount),
                modifiers,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, apply_zoom);
        // `set_zoom_factor` only stages the change; egui applies it at the
        // start of the following pass. Run an empty one so the caller can
        // observe the result.
        let _ = ctx.run(egui::RawInput::default(), apply_zoom);
    }

    #[test]
    fn ctrl_scroll_zooms_but_a_bare_wheel_does_not() {
        let ctx = egui::Context::default();

        wheel(&ctx, egui::Modifiers::NONE, 7.0);
        assert_eq!(ctx.zoom_factor(), 1.0, "a bare wheel scrolls, it does not zoom");

        wheel(&ctx, egui::Modifiers::COMMAND, 7.0);
        let zoomed = ctx.zoom_factor();
        assert!(zoomed > 1.0, "ctrl+wheel up zooms in, got {zoomed}");

        wheel(&ctx, egui::Modifiers::COMMAND, -7.0);
        assert!(ctx.zoom_factor() < zoomed, "ctrl+wheel down zooms back out");
    }

    #[test]
    fn zoom_stops_at_both_limits() {
        let ctx = egui::Context::default();

        for _ in 0..400 {
            wheel(&ctx, egui::Modifiers::COMMAND, 7.0);
        }
        assert_eq!(ctx.zoom_factor(), MAX_ZOOM, "runaway zoom in");

        for _ in 0..400 {
            wheel(&ctx, egui::Modifiers::COMMAND, -7.0);
        }
        assert_eq!(ctx.zoom_factor(), MIN_ZOOM, "runaway zoom out");
    }

    /// The zoom factor comes back off disk, so it is input like any other.
    #[test]
    fn a_stored_zoom_is_never_trusted() {
        assert_eq!(clamp_zoom(1.0), 1.0);
        assert_eq!(clamp_zoom(99.0), MAX_ZOOM);
        assert_eq!(clamp_zoom(0.0), MIN_ZOOM);
        assert_eq!(clamp_zoom(-1.0), MIN_ZOOM);
        assert_eq!(clamp_zoom(f32::NAN), 1.0);
        assert_eq!(clamp_zoom(f32::INFINITY), 1.0);
    }
}
