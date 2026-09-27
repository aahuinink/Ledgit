//! Application shell: window chrome, navigation, and the session that owns the
//! open budget.
//!
//! The rule the whole UI follows: **every view reads `repo.working()`**, which
//! is committed history plus whatever is staged. So an entry shows up in the
//! dashboard, the register and the bucket totals the instant it is made, and
//! the Commit screen is the only place that talks about what is permanent.

use crate::brand::Brand;
use crate::fmt;
use crate::forms::{FormKind, Forms, Outcome};
use crate::views;
use egui::{Color32, RichText};
use ledgit_core::prelude::*;
use ledgit_sqlite::SqliteStore;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Dashboard,
    Ledgers,
    Register,
    Transactions,
    Issuers,
    Buckets,
    Cohorts,
    Views,
    Variables,
    Commit,
    History,
}

fn screen_name(v: Screen) -> &'static str {
    match v {
        Screen::Register => "Register",
        _ => Screen::NAV.iter().find(|(s, _)| *s == v).map(|(_, n)| *n).unwrap_or("?"),
    }
}

impl Screen {
    const NAV: [(Screen, &'static str); 10] = [
        (Screen::Dashboard, "Dashboard"),
        (Screen::Ledgers, "Ledgers"),
        (Screen::Transactions, "Transactions"),
        (Screen::Issuers, "Issuers"),
        (Screen::Buckets, "Buckets"),
        (Screen::Cohorts, "Cohorts"),
        (Screen::Views, "Views"),
        (Screen::Variables, "Variables"),
        (Screen::Commit, "Commit"),
        (Screen::History, "History"),
    ];
}

pub struct Status {
    pub message: String,
    pub is_error: bool,
}

pub struct Session {
    pub repo: Repo<SqliteStore>,
    pub path: PathBuf,
    pub view: Screen,
    pub search: String,
    pub selected_ledger: Option<LedgerUid>,
    pub selected_bucket: Option<BucketUid>,
    pub selected_cohort: Option<CohortUid>,
    pub selected_view: Option<ViewUid>,
    pub selected_commit: Option<CommitId>,
    pub commit_message: String,
    pub issuer_through: String,
    pub new_branch: String,
    pub rebase_onto: String,
    pub status: Option<Status>,
    pub forms: Forms,
    pub pins: Vec<LedgerUid>,
    /// Set by a view to ask for a jump after the frame is done drawing.
    pub goto: Option<Screen>,

    // --- view state. Kept on the session rather than inside the views so that
    // navigating away and back does not silently reset a filter you set.
    pub ledger_sort: LedgerSort,
    /// Show the Ledgers screen as the path tree rather than a flat list.
    pub ledger_tree: bool,
    /// Tree levels folded shut, by lowercased path.
    pub collapsed: std::collections::HashSet<String>,
    pub bucket_roll: RollUp,
    /// Show the selected bucket's members' targets added up.
    pub bucket_targets: bool,
    /// Buckets being totalled together on the Buckets screen, in the order
    /// picked so the formula reads the way it was built. Empty means the
    /// screen is showing a single bucket instead.
    pub bucket_combo: Vec<Term>,
    pub tx_from: String,
    pub tx_to: String,
    pub tx_ledger: Option<LedgerUid>,
    pub tx_source: crate::views::transactions::SourceFilter,
    /// First day of the month the cohort calendar is showing.
    pub calendar_month: Date,
    /// The selected view's spec as it is being edited. The chart draws from
    /// this, so every pick redraws at once; nothing is staged until "Stage
    /// changes", which keeps an afternoon of fiddling out of the op log.
    pub view_draft: Option<(ViewUid, ViewSpec)>,
    /// "Simulate until" override for the Views screen; empty means the
    /// view's own horizon.
    pub view_until: String,
    /// What the Views screen lays the selected view over, if anything.
    pub view_compare: Option<crate::views::saved::CompareWith>,
    /// The budget at the compared commit, folded once and kept.
    pub compare_cache: Option<(CommitId, Budget)>,
    pub var_draft: crate::views::variables::VarDraft,
    pub goals_draft: crate::views::goals::GoalsDraft,
    /// Show on a ledger's page when its target is (or will be) reached.
    pub show_target_reached: bool,
    /// How far ahead to look for it, in years.
    pub target_horizon_years: u32,

    // --- back and undo.
    /// Screens visited, most recent last, for the back arrow.
    pub back: Vec<Screen>,
    /// Set by the back arrow, so that `track` does not record going back
    /// as a visit - which would make the arrow bounce between two screens.
    went_back: bool,
    /// Earlier states of the staging area, most recent last. Everything you
    /// do before committing is an edit to the stage - an entry staged, a
    /// change dropped or edited, issuers run, everything discarded - so
    /// undoing any of it is putting back the stage as it was.
    pub undo: Vec<Vec<Op>>,
    pub redo: Vec<Vec<Op>>,
    /// The stage as of the end of the last frame, to notice it change.
    last_stage: Vec<Op>,
    /// Where HEAD was at the end of the last frame. Once it moves - a
    /// commit, a checkout, a rebase - earlier stages belong to another
    /// base, and putting one back would re-post what is now history.
    last_head: String,
}

/// How far back undo and the back arrow reach.
const HISTORY_LIMIT: usize = 100;

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
            view: Screen::Dashboard,
            search: String::new(),
            selected_ledger: None,
            selected_bucket: None,
            selected_cohort: None,
            selected_view: None,
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
            ledger_tree: true,
            collapsed: Default::default(),
            bucket_roll: RollUp::ByNormality,
            bucket_targets: false,
            bucket_combo: Vec::new(),
            tx_from: String::new(),
            tx_to: String::new(),
            tx_ledger: None,
            tx_source: crate::views::transactions::SourceFilter::default(),
            calendar_month: Period::Month.start_of(Date::today_utc()),
            view_draft: None,
            view_until: String::new(),
            view_compare: None,
            compare_cache: None,
            var_draft: Default::default(),
            goals_draft: Default::default(),
            show_target_reached: true,
            target_horizon_years: 5,
            back: Vec::new(),
            went_back: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_stage: Vec::new(),
            last_head: String::new(),
        }
        .tracking()
    }

    fn tracking(mut self) -> Session {
        self.last_stage = self.repo.staged().to_vec();
        self.last_head = self.head_key();
        self
    }

    fn head_key(&self) -> String {
        let id = self.repo.head_commit().ok().flatten().map(|c| c.to_string());
        format!("{} {}", self.repo.head(), id.unwrap_or_default())
    }

    /// Called once a frame, after everything has drawn: record a screen
    /// change for the back arrow, and a stage change for undo.
    pub fn track(&mut self, screen_before: Screen) {
        if self.view != screen_before && !self.went_back {
            push_bounded(&mut self.back, screen_before);
        }
        self.went_back = false;
        let head = self.head_key();
        if head != self.last_head {
            self.last_head = head;
            self.undo.clear();
            self.redo.clear();
            self.last_stage = self.repo.staged().to_vec();
        } else if self.repo.staged() != self.last_stage.as_slice() {
            let before = std::mem::replace(&mut self.last_stage, self.repo.staged().to_vec());
            push_bounded(&mut self.undo, before);
            self.redo.clear();
        }
    }

    pub fn go_back(&mut self) {
        if let Some(v) = self.back.pop() {
            self.view = v;
            self.went_back = true;
        }
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            let now = self.repo.staged().to_vec();
            if self.restore(prev, "Undid") {
                push_bounded(&mut self.redo, now);
            }
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            let now = self.repo.staged().to_vec();
            if self.restore(next, "Redid") {
                push_bounded(&mut self.undo, now);
            }
        }
    }

    fn restore(&mut self, ops: Vec<Op>, verb: &str) -> bool {
        let before = self.repo.staged().len();
        match self.repo.set_stage(ops) {
            Ok(()) => {
                self.last_stage = self.repo.staged().to_vec();
                let after = self.repo.staged().len();
                let broken = self.repo.broken().len();
                let mut msg =
                    format!("{verb} the last change: {before} staged change(s) -> {after}.");
                if broken > 0 {
                    msg.push_str(&format!(" {broken} of them no longer apply; see Commit."));
                }
                self.note(msg);
                true
            }
            Err(e) => {
                self.fail(e);
                false
            }
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

fn push_bounded<T>(stack: &mut Vec<T>, item: T) {
    stack.push(item);
    if stack.len() > HISTORY_LIMIT {
        stack.remove(0);
    }
}

pub struct LedgitApp {
    session: Option<Session>,
    author: String,
    /// Pinned ledgers, per budget file. Purely a UI preference, so it lives in
    /// eframe's storage rather than in the budget - pinning something is not a
    /// fact about your money and has no business in the commit history.
    pins: HashMap<String, Vec<String>>,
    /// Bucket combinations, per budget file. A combination is a *reading* of
    /// the budget, not a fact in it, so like pins it lives beside the app and
    /// never reaches the commit history.
    combos: HashMap<String, Vec<Term>>,
    recent: Vec<String>,
    /// Last zoom factor, mirrored out of the egui context so `save` can reach
    /// it without a `Context`. Like pins, it is a preference about eyesight,
    /// not a fact about money, so it lives in eframe's storage.
    zoom: f32,
    startup_error: Option<String>,
    brand: Brand,
}

impl LedgitApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial: Option<PathBuf>,
        author: String,
    ) -> LedgitApp {
        let (pins, combos, recent, zoom) = match cc.storage {
            Some(s) => (
                eframe::get_value(s, "pins").unwrap_or_default(),
                eframe::get_value(s, "combos").unwrap_or_default(),
                eframe::get_value(s, "recent").unwrap_or_default(),
                eframe::get_value(s, "zoom").unwrap_or(1.0),
            ),
            None => (HashMap::new(), HashMap::new(), Vec::new(), 1.0),
        };
        // A stored zoom from an older build could be anything; clamp it rather
        // than trust it, or one bad value leaves the app unreadable on start.
        let zoom = clamp_zoom(zoom);
        cc.egui_ctx.set_zoom_factor(zoom);
        let brand = Brand::load(&cc.egui_ctx);
        let mut app = LedgitApp {
            session: None,
            author,
            pins,
            combos,
            recent,
            zoom,
            startup_error: None,
            brand,
        };
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
                // Same for a saved combination. `combine` reports a missing
                // bucket rather than failing, but a term that cannot resolve on
                // this branch is just noise, so drop it on the way in.
                let live_buckets: Vec<BucketUid> =
                    s.budget().buckets.live().map(|ix| s.budget().buckets.uid[ix.get()]).collect();
                s.bucket_combo = self.combos.get(&key).cloned().unwrap_or_default();
                s.bucket_combo.retain(|t| live_buckets.contains(&t.bucket));
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
            let key = s.path.to_string_lossy().to_string();
            self.pins.insert(key.clone(), s.pins.iter().map(|p| p.to_string()).collect());
            self.combos.insert(key, s.bucket_combo.clone());
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
        eframe::set_value(storage, "combos", &self.combos);
        eframe::set_value(storage, "recent", &self.recent);
        eframe::set_value(storage, "zoom", &self.zoom);
    }
}

impl LedgitApp {
    fn welcome(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| {
                let logo = self.brand.logo(ui);
                let width = 300.0;
                let height = width * logo.size()[1] as f32 / logo.size()[0] as f32;
                ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(width, height)))
                    .on_hover_text("Ledgit");
                ui.add_space(6.0);
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
        let screen_before = session.view;
        shortcuts(ctx, &mut session);

        top_bar(ctx, &mut session, &self.brand);
        nav_panel(ctx, &mut session);
        status_bar(ctx, &mut session);

        egui::CentralPanel::default().show(ctx, |ui| {
            // A search bar with text in it beats whatever tab is selected;
            // that is what people expect a search bar to do.
            if !session.search.trim().is_empty() && session.view != Screen::Transactions {
                views::search::show(ui, &mut session);
                return;
            }
            match session.view {
                Screen::Dashboard => views::dashboard::show(ui, &mut session),
                Screen::Ledgers => views::ledgers::show(ui, &mut session),
                Screen::Register => views::ledgers::register(ui, &mut session),
                Screen::Transactions => views::transactions::show(ui, &mut session),
                Screen::Issuers => views::issuers::show(ui, &mut session),
                Screen::Buckets => views::buckets::show(ui, &mut session),
                Screen::Cohorts => views::cohorts::show(ui, &mut session),
                Screen::Views => views::saved::show(ui, &mut session),
                Screen::Variables => views::variables::show(ui, &mut session),
                Screen::Commit => views::commit::show(ui, &mut session),
                Screen::History => views::history::show(ui, &mut session),
            }
        });

        show_form_modal(ctx, &mut session);

        if let Some(v) = session.goto.take() {
            session.view = v;
        }
        session.track(screen_before);
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

/// Back: Alt+Left or the mouse's back button. Undo: Ctrl+Z; redo: Ctrl+Y or
/// Ctrl+Shift+Z. Undo and redo leave a focused text box alone, so Ctrl+Z
/// there still undoes typing rather than your last staged entry.
fn shortcuts(ctx: &egui::Context, s: &mut Session) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    let back = ctx.input_mut(|i| {
        i.consume_shortcut(&KeyboardShortcut::new(Modifiers::ALT, Key::ArrowLeft))
            || i.pointer.button_clicked(egui::PointerButton::Extra1)
    });
    if back {
        s.go_back();
    }
    if s.forms.open.is_some() || ctx.wants_keyboard_input() {
        return;
    }
    let (undo, redo) = ctx.input_mut(|i| {
        let redo = i.consume_shortcut(&KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            Key::Z,
        )) || i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Y));
        let undo = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z));
        (undo, redo)
    });
    if undo {
        s.undo();
    }
    if redo {
        s.redo();
    }
}

pub(crate) fn top_bar(ctx: &egui::Context, s: &mut Session, brand: &Brand) {
    egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let back_to = s.back.last().map(|v| screen_name(*v)).unwrap_or("nowhere yet");
            if ui
                .add_enabled(!s.back.is_empty(), egui::Button::new("\u{2B05}"))
                .on_hover_text(format!("Back to {back_to} (Alt+Left)"))
                .on_disabled_hover_text("Nothing to go back to")
                .clicked()
            {
                s.go_back();
            }
            let name = s
                .path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "budget".into());
            ui.add(egui::Image::new(brand.mark(ui)).fit_to_exact_size(egui::vec2(26.0, 26.0)));
            ui.heading(name);
            ui.label(RichText::new(format!("on {}", s.repo.head())).color(fmt::dim()));
            freshness(ui, s.budget());

            ui.separator();
            ui.menu_button("New", |ui| {
                for (kind, label) in [
                    (FormKind::Transaction, "Transaction"),
                    (FormKind::Ledger, "Ledger"),
                    (FormKind::Bucket, "Bucket"),
                    (FormKind::Issuer, "Issuer"),
                    (FormKind::Cohort, "Cohort"),
                    (FormKind::View, "View"),
                ] {
                    if ui.button(label).clicked() {
                        s.forms.open(kind, s.repo.working());
                        ui.close();
                    }
                }
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let fired = ledgit_core::goals::fired(s.budget()).len();
                if fired > 0
                    && ui
                        .button(RichText::new(format!("\u{26A0} {fired}")).color(fmt::bad()))
                        .on_hover_text(format!(
                            "{fired} alert(s) past their level. See the Dashboard."
                        ))
                        .clicked()
                {
                    s.goto = Some(Screen::Dashboard);
                }
                if ui
                    .add_enabled(!s.redo.is_empty(), egui::Button::new("\u{27F3}"))
                    .on_hover_text("Redo (Ctrl+Y)")
                    .clicked()
                {
                    s.redo();
                }
                if ui
                    .add_enabled(!s.undo.is_empty(), egui::Button::new("\u{27F2} Undo"))
                    .on_hover_text(
                        "Undo the last change to the staging area (Ctrl+Z). Commits are \
                         undone by reverting them on the History screen.",
                    )
                    .clicked()
                {
                    s.undo();
                }
                let staged = s.repo.staged().len();
                let text = if staged == 0 {
                    RichText::new("nothing staged").color(fmt::dim())
                } else {
                    RichText::new(format!("{staged} staged")).color(Color32::from_rgb(220, 170, 60))
                };
                if ui.button(text).clicked() {
                    s.goto = Some(Screen::Commit);
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

/// "through 2026-09-24 · issuers 2026-09-15": how current this branch is.
///
/// The first date is the newest transaction on record, staged or not. The
/// second is how far the issuers have been run - every recurring payment up
/// to that day is in. It turns red when an issuer owes something dated before
/// today, because that is the moment the balances on screen stop being true.
fn freshness(ui: &mut egui::Ui, l: &Budget) {
    let today = Date::today_utc();
    let latest = l.latest_transaction_date();
    let issuers = ledgit_core::cohort::caught_up_through(l);
    let overdue = ledgit_core::cohort::overdue_issuers(l, today);
    let mut text = match latest {
        Some(d) => format!("through {d}"),
        None => "no transactions yet".into(),
    };
    if let Some(d) = issuers {
        text.push_str(&format!("  \u{b7}  issuers {d}"));
    }
    let colour = if overdue > 0 { fmt::bad() } else { fmt::dim() };
    let mut hover = String::from(
        "Up to date through: the date of the newest transaction on this branch, staged ones included.",
    );
    if issuers.is_some() {
        hover.push_str(
            "\nIssuers: every recurring payment dated on or before this day has been posted.",
        );
    }
    if overdue > 0 {
        hover.push_str(&format!(
            "\n\n{overdue} issuer(s) owe payments dated before today. Run them on the Issuers screen."
        ));
    }
    ui.separator();
    ui.label(RichText::new(text).color(colour)).on_hover_text(hover);
}

fn nav_panel(ctx: &egui::Context, s: &mut Session) {
    egui::SidePanel::left("nav").resizable(false).exact_width(180.0).show(ctx, |ui| {
        ui.add_space(8.0);
        for (view, label) in Screen::NAV {
            let selected =
                s.view == view || (view == Screen::Ledgers && s.view == Screen::Register);
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
                    s.view = Screen::Register;
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
    let title = s.forms.title();
    let response = egui::Modal::new(egui::Id::new("form_modal")).show(ctx, |ui| {
        ui.set_width(kind.width());
        ui.heading(&title);
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
        Outcome::Submit(mut ops) => match s.forms.editing.take() {
            Some(index) if ops.len() == 1 => match s.repo.replace_staged(index, ops.remove(0)) {
                Ok(_) => s.note("Edited the staged change."),
                Err(e) => {
                    // Keep the form open on what was typed, so it can be fixed.
                    s.forms.error = Some(e.to_string());
                    s.forms.editing = Some(index);
                    s.forms.open = Some(kind);
                }
            },
            _ => {
                let what = kind.title().to_lowercase().replace("new ", "");
                s.stage(ops, &what);
            }
        },
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
