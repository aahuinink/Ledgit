//! Tutorial mode: a guided walk through every feature, on sample budgets
//! built for it, that doubles as a test pass - each step can be marked as
//! working or not, with notes, and the whole report saved and copied.
//!
//! The tutorial lives beside the app, not in a budget: [`Tutorial`] is held
//! by `LedgitApp` and survives switching budgets, which it does itself -
//! each chapter opens its own copy of a sample budget from a folder in the
//! app's data directory. Progress and notes persist across runs.

mod steps;

pub use steps::{chapters, Baseline, Chapter, Go, Step};

use crate::app::{Screen, Session};
use crate::fmt;
use egui::{RichText, Ui};
use ledgit_core::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How a step went, in the tester's words.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Verdict {
    Works,
    Problem,
}

/// What persists between runs.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Progress {
    pub chapter: usize,
    pub step: usize,
    /// Steps whose check passed, or that were moved past, by step id.
    pub done: std::collections::BTreeSet<String>,
    pub verdicts: BTreeMap<String, Verdict>,
    pub notes: BTreeMap<String, String>,
}

/// What the panel asks the app to do after the frame: only the app can swap
/// the budget on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open a chapter's budget, building it first if it is missing - or
    /// always, with `fresh`.
    Open {
        chapter: usize,
        fresh: bool,
    },
    End,
}

pub struct Tutorial {
    pub dir: PathBuf,
    pub progress: Progress,
    chapters: Vec<Chapter>,
    /// The session as the current step found it.
    baseline: Option<Baseline>,
    /// The budget the current chapter runs on, once opened.
    opened: Option<PathBuf>,
    /// Shown under the feedback buttons after saving or copying.
    status: Option<String>,
}

impl Tutorial {
    pub fn new(dir: PathBuf, progress: Progress) -> Tutorial {
        let chapters = chapters();
        let mut progress = progress;
        progress.chapter = progress.chapter.min(chapters.len() - 1);
        progress.step = progress.step.min(chapters[progress.chapter].steps.len() - 1);
        Tutorial { dir, progress, chapters, baseline: None, opened: None, status: None }
    }

    /// Where the tutorial keeps its budgets and the feedback report: a
    /// folder in the app's data directory, or the temp directory if the
    /// platform gives none.
    pub fn default_dir() -> PathBuf {
        eframe::storage_dir("Ledgit").unwrap_or_else(std::env::temp_dir).join("tutorial")
    }

    #[cfg(test)]
    pub fn chapters(&self) -> &[Chapter] {
        &self.chapters
    }

    pub fn chapter(&self) -> &Chapter {
        &self.chapters[self.progress.chapter]
    }

    pub fn step(&self) -> &Step {
        &self.chapter().steps[self.progress.step]
    }

    /// The file a chapter's budget lives in: `03-first-entries.ledgit`.
    pub fn chapter_path(&self, chapter: usize) -> PathBuf {
        self.dir.join(format!("{:02}-{}.ledgit", chapter, self.chapters[chapter].id))
    }

    pub fn feedback_path(&self) -> PathBuf {
        self.dir.join("tutorial-feedback.md")
    }

    /// Build a chapter's budget afresh, replacing any earlier copy. The app
    /// must not have it open.
    pub fn build_chapter(&self, chapter: usize) -> Result<PathBuf> {
        let path = self.chapter_path(chapter);
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| Error::Store(format!("{}: {e}", self.dir.display())))?;
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let mut p = path.as_os_str().to_owned();
            p.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(p));
        }
        // Built in memory and written out in one go: hundreds of durable
        // writes, one per staged change and commit, take seconds on disk.
        let mut repo = Repo::init(MemStore::new(), "tutorial")?;
        ledgit_core::demo::build(self.chapters[chapter].budget, &mut repo, Date::today_utc())?;
        ledgit_sqlite::SqliteStore::open(&path)?.import(repo.store())?;
        Ok(path)
    }

    /// The chapter's budget is open in `s`: start its current step.
    pub fn opened(&mut self, chapter: usize, s: &mut Session) {
        self.progress.chapter = chapter;
        self.opened = Some(s.path.clone());
        self.enter(s);
    }

    /// Whether `s` is the current chapter's budget.
    pub fn is_open_in(&self, s: &Session) -> bool {
        self.opened.as_deref().is_some_and(|p| same_path(p, &s.path))
    }

    /// Begin the current step: remember where things stand, and go where
    /// it points.
    pub fn enter(&mut self, s: &mut Session) {
        self.baseline = Some(Baseline::of(s));
        go(self.step().go, s);
    }

    /// Whether the current step's check has passed. A step without one
    /// passes by being moved past.
    pub fn check(&mut self, s: &Session) -> bool {
        let id = self.step().id.to_string();
        if self.progress.done.contains(&id) {
            return true;
        }
        let Some(check) = self.step().check else { return false };
        let Some(b) = &self.baseline else { return false };
        if check(s, b) {
            self.progress.done.insert(id);
            return true;
        }
        false
    }

    /// Move to step `step` of the current chapter, or into the next or
    /// previous chapter. Returns a request when the budget must change.
    fn move_to(&mut self, chapter: usize, step: usize, s: &mut Session) -> Option<Request> {
        let changes = chapter != self.progress.chapter;
        self.progress.chapter = chapter;
        self.progress.step = step;
        if changes {
            return Some(Request::Open { chapter, fresh: false });
        }
        self.enter(s);
        None
    }

    fn next(&mut self, s: &mut Session) -> Option<Request> {
        let id = self.step().id.to_string();
        self.progress.done.insert(id);
        let (c, k) = (self.progress.chapter, self.progress.step);
        if k + 1 < self.chapter().steps.len() {
            self.move_to(c, k + 1, s)
        } else if c + 1 < self.chapters.len() {
            self.move_to(c + 1, 0, s)
        } else {
            None
        }
    }

    fn back(&mut self, s: &mut Session) -> Option<Request> {
        let (c, k) = (self.progress.chapter, self.progress.step);
        if k > 0 {
            self.move_to(c, k - 1, s)
        } else if c > 0 {
            let last = self.chapters[c - 1].steps.len() - 1;
            self.move_to(c - 1, last, s)
        } else {
            None
        }
    }

    // ------------------------------------------------------------ feedback

    /// The whole report, as markdown.
    pub fn report(&self) -> String {
        let mut out = format!(
            "# Ledgit tutorial feedback\n\nLedgit {}, {}\n",
            env!("CARGO_PKG_VERSION"),
            Date::today_utc()
        );
        for (c, chapter) in self.chapters.iter().enumerate() {
            out.push_str(&format!("\n## {}. {}\n", c, chapter.title));
            for step in &chapter.steps {
                let verdict = match self.progress.verdicts.get(step.id) {
                    Some(Verdict::Works) => "works",
                    Some(Verdict::Problem) => "PROBLEM",
                    None if self.progress.done.contains(step.id) => "done, not marked",
                    None => "not tried",
                };
                out.push_str(&format!("\n- **{}** (`{}`): {verdict}\n", step.title, step.id));
                if let Some(note) =
                    self.progress.notes.get(step.id).filter(|n| !n.trim().is_empty())
                {
                    for line in note.trim().lines() {
                        out.push_str(&format!("    {line}\n"));
                    }
                }
            }
        }
        out
    }

    /// Write the report next to the tutorial's budgets.
    pub fn save_report(&mut self) {
        let r = std::fs::create_dir_all(&self.dir)
            .and_then(|_| std::fs::write(self.feedback_path(), self.report()));
        if let Err(e) = r {
            self.status = Some(format!("Could not save the feedback: {e}"));
        }
    }

    // --------------------------------------------------------------- panel

    /// The panel down the right of the window.
    pub fn panel(&mut self, ctx: &egui::Context, s: &mut Session) -> Option<Request> {
        let mut request = None;
        egui::SidePanel::right("tutorial")
            .resizable(true)
            .default_width(360.0)
            .min_width(280.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().id_salt("tutorial_panel").show(ui, |ui| {
                    request = self.ui(ui, s);
                });
            });
        request
    }

    /// The panel's contents.
    pub fn ui(&mut self, ui: &mut Ui, s: &mut Session) -> Option<Request> {
        let mut request = None;
        let (c, k) = (self.progress.chapter, self.progress.step);
        let (chapters, steps) = (self.chapters.len(), self.chapter().steps.len());

        ui.horizontal(|ui| {
            ui.label(RichText::new("TUTORIAL").small().strong().color(fmt::dim()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("End")
                    .on_hover_text("Close the tutorial. Progress and notes are kept.")
                    .clicked()
                {
                    request = Some(Request::End);
                }
            });
        });
        ui.label(
            RichText::new(format!("Chapter {c} of {}: {}", chapters - 1, self.chapter().title))
                .strong(),
        );

        if !self.is_open_in(s) {
            ui.add_space(6.0);
            ui.label(
                RichText::new("The budget on screen is not this chapter's.").color(fmt::warn()),
            );
            if ui.button("Open the chapter's budget").clicked() {
                request = Some(Request::Open { chapter: c, fresh: false });
            }
            return request;
        }
        if k == 0 {
            ui.label(RichText::new(self.chapter().intro).color(fmt::dim()));
        }
        ui.add_space(6.0);

        let done = self.check(s);
        let step = self.step();
        ui.label(RichText::new(format!("Step {} of {steps}", k + 1)).small().color(fmt::dim()));
        ui.heading(step.title);
        ui.add_space(4.0);
        ui.label(step.about);
        ui.add_space(8.0);
        ui.label(RichText::new("Try it").strong());
        ui.label(step.try_it);
        ui.add_space(6.0);
        ui.label(RichText::new("You should see").strong());
        ui.label(step.expect);
        ui.add_space(8.0);
        match (step.check.is_some(), done) {
            (_, true) => ui.label(RichText::new("\u{2714} Done").color(fmt::good()).strong()),
            (true, false) => {
                ui.label(RichText::new("Waiting for you to try it.").color(fmt::warn()))
            }
            (false, false) => ui.label(
                RichText::new("Nothing to detect here: press Next when you have looked.")
                    .small()
                    .color(fmt::dim()),
            ),
        };
        let has_go = step.go != Go::Stay;

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let first = c == 0 && k == 0;
            if ui.add_enabled(!first, egui::Button::new("\u{2190} Back")).clicked() {
                request = self.back(s);
            }
            if has_go
                && ui.button("Show me").on_hover_text("Go back to where this step starts").clicked()
            {
                go(self.step().go, s);
            }
            let last = c + 1 == chapters && k + 1 == steps;
            if ui.add_enabled(!last, egui::Button::new("Next \u{2192}")).clicked() {
                request = self.next(s);
            }
        });

        // How it went.
        ui.add_space(10.0);
        ui.separator();
        ui.label(RichText::new("How did it go?").strong());
        let id = self.step().id.to_string();
        let mut changed = false;
        ui.horizontal(|ui| {
            let now = self.progress.verdicts.get(&id).copied();
            for (v, label) in [(Verdict::Works, "Works"), (Verdict::Problem, "Problem")] {
                let text = match v {
                    Verdict::Works => RichText::new(label).color(fmt::good()),
                    Verdict::Problem => RichText::new(label).color(fmt::bad()),
                };
                if ui.selectable_label(now == Some(v), text).clicked() {
                    if now == Some(v) {
                        self.progress.verdicts.remove(&id);
                    } else {
                        self.progress.verdicts.insert(id.clone(), v);
                    }
                    changed = true;
                }
            }
        });
        let note = self.progress.notes.entry(id.clone()).or_default();
        let r = ui.add(
            egui::TextEdit::multiline(note)
                .hint_text("What looked wrong, or could be better?")
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        changed |= r.changed();
        if changed {
            self.save_report();
        }

        // Chapters, and the report.
        ui.add_space(10.0);
        ui.separator();
        egui::CollapsingHeader::new("Chapters").id_salt("tutorial_chapters").show(ui, |ui| {
            for (j, ch) in self.chapters.iter().enumerate() {
                let done = ch.steps.iter().filter(|st| self.progress.done.contains(st.id)).count();
                let problems = ch
                    .steps
                    .iter()
                    .filter(|st| self.progress.verdicts.get(st.id) == Some(&Verdict::Problem))
                    .count();
                ui.horizontal(|ui| {
                    let label = format!("{j}. {}  {done}/{}", ch.title, ch.steps.len());
                    if ui.selectable_label(j == c, label).clicked() && j != c {
                        self.progress.chapter = j;
                        self.progress.step = 0;
                        request = Some(Request::Open { chapter: j, fresh: false });
                    }
                    if problems > 0 {
                        ui.label(
                            RichText::new(format!("{problems} problem(s)"))
                                .small()
                                .color(fmt::bad()),
                        );
                    }
                });
            }
        });
        if ui
            .button("Restart chapter")
            .on_hover_text("Build this chapter's budget again from scratch. Your notes are kept.")
            .clicked()
        {
            // A fresh budget undoes what the steps did; the ticks go with
            // it. Verdicts and notes stay.
            for st in &self.chapters[c].steps {
                self.progress.done.remove(st.id);
            }
            self.progress.step = 0;
            request = Some(Request::Open { chapter: c, fresh: true });
        }

        ui.add_space(10.0);
        ui.separator();
        ui.label(RichText::new("Feedback").strong());
        ui.label(
            RichText::new(self.feedback_path().display().to_string())
                .small()
                .monospace()
                .color(fmt::dim()),
        );
        ui.horizontal(|ui| {
            if ui.button("Copy feedback").on_hover_text("The whole report, as markdown").clicked() {
                ui.ctx().copy_text(self.report());
                self.status = Some("Copied.".into());
            }
            if ui.button("Copy folder path").clicked() {
                ui.ctx().copy_text(self.dir.display().to_string());
                self.status = Some("Copied the folder's path.".into());
            }
        });
        if let Some(st) = &self.status {
            ui.label(RichText::new(st).small().color(fmt::dim()));
        }
        request
    }
}

/// Take the session where a step points.
pub fn go(to: Go, s: &mut Session) {
    let l = s.budget();
    match to {
        Go::Stay => {}
        Go::Screen(screen) => s.goto = Some(screen),
        Go::Ledger(name) => {
            if let Some(ix) = l.ledger_by_name(name) {
                s.selected_ledger = Some(l.ledgers.uid[ix.get()]);
                s.goto = Some(Screen::Register);
            }
        }
        Go::Issuer(name) => {
            if let Some(ix) = l.issuer_by_name(name) {
                s.selected_issuer = Some(l.issuers.uid[ix.get()]);
                s.goto = Some(Screen::Issuers);
            }
        }
        Go::View(name) => {
            if let Some(ix) = l.view_by_name(name) {
                s.selected_view = Some(l.views.uid[ix.get()]);
                s.view_draft = None;
                s.goto = Some(Screen::Views);
            }
        }
        Go::Bucket(name) => {
            if let Some(ix) = l.bucket_by_name(name) {
                s.selected_bucket = Some(l.buckets.uid[ix.get()]);
                s.goto = Some(Screen::Buckets);
            }
        }
        Go::Commit(prefix) => {
            let found = s
                .repo
                .log(None)
                .unwrap_or_default()
                .into_iter()
                .find(|c| c.summary().starts_with(prefix))
                .map(|c| c.id);
            if let Some(id) = found {
                s.selected_commit = Some(id);
            }
            s.goto = Some(Screen::History);
        }
    }
    // A search on screen would hide wherever the step went.
    s.search.clear();
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
