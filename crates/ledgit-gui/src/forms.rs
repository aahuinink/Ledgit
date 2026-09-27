//! Entry forms.
//!
//! Each form collects text, validates it into exactly one [`Op`], and hands it
//! back. They never touch the repo - the caller stages what comes out. That
//! keeps "what the user typed" and "what the budget does" separable, which is
//! why the same forms can later be reused for an import or an edit dialog.

use crate::datepick::DateField;
use crate::fmt;
use crate::picker::{Pick, Picker};
use egui::Ui;
use ledgit_core::expr::{eval_money, is_plain_amount};
use ledgit_core::prelude::*;
use ledgit_core::state::VarArena;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormKind {
    Ledger,
    Transaction,
    Bucket,
    Issuer,
    Cohort,
    View,
    Move,
}

impl FormKind {
    /// How wide the modal needs to be. The entry forms carry a leg editor -
    /// a debit/credit toggle, a ledger picker, an amount and a remove
    /// button on one row - which does not fit the width a name-and-description
    /// form wants.
    pub fn width(self) -> f32 {
        match self {
            FormKind::Ledger
            | FormKind::Bucket
            | FormKind::Cohort
            | FormKind::View
            | FormKind::Move => 460.0,
            FormKind::Transaction | FormKind::Issuer => 640.0,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            FormKind::Ledger => "New ledger",
            FormKind::Transaction => "New transaction",
            FormKind::Bucket => "New bucket",
            FormKind::Issuer => "New issuer",
            FormKind::Cohort => "New cohort",
            FormKind::View => "New view",
            FormKind::Move => "Move ledgers",
        }
    }
}

pub enum Outcome {
    /// Still open.
    Pending,
    Cancelled,
    /// Validated. Stage these.
    Submit(Vec<Op>),
}

#[derive(Default)]
pub struct Forms {
    pub open: Option<FormKind>,
    pub error: Option<String>,
    /// Set while the open form is editing a staged change rather than making
    /// a new one: its position in the stage. What the form submits then
    /// replaces that change in place, under the same uid.
    pub editing: Option<usize>,
    ledger: LedgerForm,
    transaction: TxForm,
    bucket: BucketForm,
    issuer: IssuerForm,
    cohort: CohortForm,
    view: ViewForm,
    moving: MoveForm,
}

impl Forms {
    /// Open a form, seeding date fields with today so the common case is one
    /// amount and two clicks.
    pub fn open(&mut self, kind: FormKind, budget: &Budget) {
        self.editing = None;
        let today = Date::today_utc().to_string();
        match kind {
            FormKind::Ledger => self.open_ledger_under("", budget),
            FormKind::Transaction => {
                self.transaction =
                    TxForm { date: today, legs: LegEditor::seed(budget), ..Default::default() };
            }
            FormKind::Bucket => self.bucket = BucketForm::default(),
            FormKind::Cohort => self.cohort = CohortForm::default(),
            FormKind::View => self.view = ViewForm { all_buckets: true, ..Default::default() },
            FormKind::Move => self.moving = MoveForm::default(),
            FormKind::Issuer => {
                self.issuer = IssuerForm {
                    start: today,
                    legs: LegEditor::seed(budget),
                    every_n_days: "14".into(),
                    day_of_month: "1".into(),
                    every_n_months: 1,
                    ..Default::default()
                };
            }
        }
        self.error = None;
        self.open = Some(kind);
    }

    /// Open the ledger form with its place in the tree already chosen, e.g.
    /// from a level's "+" on the Ledgers screen.
    pub fn open_ledger_under(&mut self, parent: &str, budget: &Budget) {
        self.editing = None;
        self.ledger = LedgerForm {
            parent: parent.to_string(),
            normality: normality_under(budget, parent),
            opened: Date::today_utc().to_string(),
            ..Default::default()
        };
        self.error = None;
        self.open = Some(FormKind::Ledger);
    }

    /// Open the move form with a subtree already chosen, e.g. from its row
    /// on the Ledgers screen.
    pub fn open_move(&mut self, from: &str) {
        self.editing = None;
        self.moving = MoveForm { from: from.to_string(), to: from.to_string() };
        self.error = None;
        self.open = Some(FormKind::Move);
    }

    /// Open the form that made a staged change, filled in from it, to edit
    /// it in place. Returns false for a change that has no form - bucket
    /// membership, a pause, an issuer run - which is simply dropped instead.
    pub fn edit_staged(&mut self, index: usize, op: &Op) -> bool {
        let kind = match op.clone() {
            Op::CreateLedger { uid, name, description, normality, opened } => {
                let (parent, leaf) = match name.rsplit_once(':') {
                    Some((p, l)) => (p.to_string(), l.to_string()),
                    None => (String::new(), name),
                };
                self.ledger = LedgerForm {
                    uid: Some(uid),
                    parent,
                    name: leaf,
                    description,
                    normality: match normality {
                        Normality::Debit => NormalityChoice::Debit,
                        Normality::Credit => NormalityChoice::Credit,
                    },
                    opened: opened.to_string(),
                };
                FormKind::Ledger
            }
            Op::PostTransaction { uid, name, description, date, legs, parent } => {
                self.transaction = TxForm {
                    uid: Some((uid, parent)),
                    name,
                    description,
                    date: date.to_string(),
                    legs: LegEditor::from_legs(&legs),
                };
                FormKind::Transaction
            }
            Op::CreateIssuer { uid, name, description, legs, schedule, start, rule } => {
                let mut f = IssuerForm {
                    uid: Some(uid),
                    name,
                    description,
                    legs: LegEditor::from_legs(&legs),
                    start: start.to_string(),
                    every_n_days: "14".into(),
                    day_of_month: "1".into(),
                    every_n_months: 1,
                    ..Default::default()
                };
                match schedule {
                    Schedule::EveryNDays { n } => {
                        f.repeat = Repeat::EveryNDays;
                        f.every_n_days = n.to_string();
                    }
                    Schedule::MonthlyOn { day, every_n_months } => {
                        f.repeat = Repeat::Monthly;
                        f.day_of_month = day.to_string();
                        f.every_n_months = every_n_months;
                    }
                    Schedule::Once => f.repeat = Repeat::Once,
                }
                if let Some(rule) = rule {
                    f.rule_debit = legs.iter().find(|l| l.is_debit()).map(|l| l.ledger);
                    f.rule_credit = legs.iter().find(|l| !l.is_debit()).map(|l| l.ledger);
                    f.rule_of = Some(rule.of());
                    let (mode, rate) = match rule {
                        AmountRule::Interest { apr, .. } => (AmountMode::Interest, apr),
                        AmountRule::ShareOfBalance { rate, .. } => (AmountMode::Share, rate),
                    };
                    f.amount = mode;
                    f.rate = rate.to_string().trim_end_matches('%').to_string();
                }
                self.issuer = f;
                FormKind::Issuer
            }
            Op::CreateBucket { uid, name, description } => {
                self.bucket = BucketForm { uid: Some(uid), name, description };
                FormKind::Bucket
            }
            Op::CreateCohort { uid, name, description } => {
                self.cohort = CohortForm { uid: Some(uid), name, description };
                FormKind::Cohort
            }
            Op::CreateView { uid, name, description, spec } => {
                self.view =
                    ViewForm { uid: Some((uid, spec)), name, description, all_buckets: false };
                FormKind::View
            }
            _ => return false,
        };
        self.error = None;
        self.editing = Some(index);
        self.open = Some(kind);
        true
    }

    /// The modal's title: "New ledger", or "Edit staged ledger".
    pub fn title(&self) -> String {
        let Some(kind) = self.open else { return String::new() };
        match self.editing {
            Some(_) => kind.title().replacen("New ", "Edit staged ", 1),
            None => kind.title().to_string(),
        }
    }

    pub fn show(&mut self, ui: &mut Ui, budget: &Budget) -> Outcome {
        let Some(kind) = self.open else {
            return Outcome::Pending;
        };
        let outcome = match kind {
            FormKind::Ledger => self.ledger.show(ui, budget),
            FormKind::Transaction => self.transaction.show(ui, budget),
            FormKind::Bucket => self.bucket.show(ui),
            FormKind::Issuer => self.issuer.show(ui, budget),
            FormKind::Cohort => self.cohort.show(ui),
            FormKind::View => self.view.show(ui, budget),
            FormKind::Move => self.moving.show(ui, budget),
        };
        match outcome {
            Ok(o) => {
                if !matches!(o, Outcome::Pending) {
                    self.open = None;
                    self.error = None;
                    if matches!(o, Outcome::Cancelled) {
                        self.editing = None;
                    }
                }
                o
            }
            Err(msg) => {
                self.error = Some(msg);
                Outcome::Pending
            }
        }
    }
}

type Filled = std::result::Result<Outcome, String>;

/// The buttons every form ends with. Returns `true` when Save was pressed.
fn footer(ui: &mut Ui, save_label: &str) -> (bool, bool) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let save = ui.button(save_label).clicked();
        let cancel = ui.button("Cancel").clicked();
        (save, cancel)
    })
    .inner
}

fn date_row(ui: &mut Ui, label: &str, id: &str, value: &mut String) {
    ui.label(label);
    DateField::new(id, value).show(ui);
    ui.end_row();
}

fn label_row(ui: &mut Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(280.0));
    ui.end_row();
}

/// The editor for the sides of an entry.
///
/// Two rows by default, which is an ordinary transfer. Add rows and it is a
/// paycheque. The running "out by" readout is the whole point: an entry that
/// does not balance is not a thing you can save, so the form says so while you
/// are still typing rather than after you press the button.
#[derive(Clone)]
struct LegEditor {
    rows: Vec<LegRow>,
}

#[derive(Clone, Default)]
struct LegRow {
    ledger: Option<LedgerUid>,
    amount: String,
    is_debit: bool,
}

impl Default for LegEditor {
    fn default() -> Self {
        LegEditor {
            rows: vec![
                LegRow { is_debit: true, ..Default::default() },
                LegRow { is_debit: false, ..Default::default() },
            ],
        }
    }
}

impl LegEditor {
    /// Seed the two default rows with the first two ledgers, so the common
    /// case is "type an amount and press save".
    fn seed(budget: &Budget) -> LegEditor {
        let mut e = LegEditor::default();
        e.rows[0].ledger = budget.ledgers.uid.first().copied();
        e.rows[1].ledger = budget.ledgers.uid.get(1).copied().or(e.rows[0].ledger);
        e
    }

    /// Rows for an existing entry's legs, e.g. a staged one being edited.
    fn from_legs(legs: &[Leg]) -> LegEditor {
        LegEditor {
            rows: legs
                .iter()
                .map(|l| LegRow {
                    ledger: Some(l.ledger),
                    amount: Money(l.amount.cents().abs()).to_string(),
                    is_debit: l.amount.cents() > 0,
                })
                .collect(),
        }
    }

    /// Signed amounts for every row that works out, debit-positive. An
    /// amount may be a formula over the budget's variables.
    fn parsed(&self, vars: &VarArena) -> Vec<(Option<LedgerUid>, Option<Money>)> {
        self.rows
            .iter()
            .map(|r| {
                let m = eval_money(&r.amount, vars).ok().filter(|m| m.cents() > 0).map(|m| {
                    if r.is_debit {
                        m
                    } else {
                        Money(-m.cents())
                    }
                });
                (r.ledger, m)
            })
            .collect()
    }

    /// How far the entry is from balancing, over the rows that parse.
    fn imbalance(&self, vars: &VarArena) -> Money {
        self.parsed(vars).iter().filter_map(|(_, m)| *m).sum()
    }

    fn show(&mut self, ui: &mut Ui, budget: &Budget, id: &str) {
        let mut remove: Option<usize> = None;
        // Read before the rows are borrowed mutably below.
        let removable = self.rows_removable();
        egui::Grid::new(id).num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
            for (i, row) in self.rows.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut row.is_debit, true, "Debit");
                    ui.selectable_value(&mut row.is_debit, false, "Credit");
                });

                let text = row
                    .ledger
                    .map(|u| fmt::ledger_label(budget, u))
                    .unwrap_or_else(|| "choose a ledger".to_string());
                if let Some(Pick::Ledger(uid)) = Picker::new(format!("{id}_acct_{i}"), budget)
                    .selected_text(text)
                    .width(220.0)
                    .show(ui)
                {
                    row.ledger = Some(uid);
                }

                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut row.amount)
                            .hint_text("0.00")
                            .desired_width(110.0),
                    )
                    .on_hover_text("An amount, or a formula: 200 * Car_Km_Rate");
                    // A formula shows what it works out to, as you type.
                    let text = row.amount.trim();
                    if !text.is_empty() && !is_plain_amount(text) {
                        match eval_money(text, &budget.variables) {
                            Ok(m) => ui.label(
                                egui::RichText::new(format!("= {}", fmt::amount(m)))
                                    .small()
                                    .color(fmt::dim()),
                            ),
                            Err(e) => ui.label(
                                egui::RichText::new(e.to_string()).small().color(fmt::bad()),
                            ),
                        };
                    }
                });

                // Never let the form drop below a two-sided entry.
                if ui.add_enabled(removable, egui::Button::new("\u{1F5D9}").small()).clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            self.rows.remove(i);
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Add a side").clicked() {
                self.rows.push(LegRow { is_debit: true, ..Default::default() });
            }

            let out = self.imbalance(&budget.variables);
            if out.is_zero() {
                ui.colored_label(fmt::good(), "balanced");
            } else {
                ui.colored_label(fmt::bad(), format!("out by {}", fmt::amount(out)));
                if ui
                    .small_button("balance the last side")
                    .on_hover_text("Set the last side to whatever makes the entry balance")
                    .clicked()
                {
                    self.balance_last(&budget.variables);
                }
            }
        });
    }

    fn rows_removable(&self) -> bool {
        self.rows.len() > 2
    }

    /// Give the last row whatever amount squares the entry.
    fn balance_last(&mut self, vars: &VarArena) {
        let others: Money =
            self.parsed(vars).iter().take(self.rows.len() - 1).filter_map(|(_, m)| *m).sum();
        if let Some(last) = self.rows.last_mut() {
            if others.is_zero() {
                return;
            }
            last.is_debit = others.cents() < 0;
            last.amount = Money(others.cents().abs()).to_string();
        }
    }

    fn finish(&self, vars: &VarArena) -> std::result::Result<Vec<Leg>, String> {
        let mut legs = Vec::with_capacity(self.rows.len());
        for (i, row) in self.rows.iter().enumerate() {
            let ledger = row.ledger.ok_or_else(|| format!("Side {} has no ledger", i + 1))?;
            if row.amount.trim().is_empty() {
                return Err(format!("Side {} needs an amount like 42.50", i + 1));
            }
            let amount =
                eval_money(&row.amount, vars).map_err(|e| format!("Side {}: {e}", i + 1))?;
            if amount.cents() <= 0 {
                return Err(format!(
                    "Side {} must be positive - Debit or Credit already says which way it goes",
                    i + 1
                ));
            }
            legs.push(Leg {
                ledger,
                amount: if row.is_debit { amount } else { Money(-amount.cents()) },
            });
        }
        validate_legs(&legs)?;
        Ok(legs)
    }
}

// ------------------------------------------------------------------ ledger

#[derive(Default)]
struct LedgerForm {
    /// Kept when editing a staged ledger, so what refers to it still does.
    uid: Option<LedgerUid>,
    /// Where in the tree it goes; blank is the top level. The name is the
    /// last part only, though a colon typed into it still nests further.
    parent: String,
    name: String,
    description: String,
    normality: NormalityChoice,
    opened: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum NormalityChoice {
    #[default]
    Debit,
    Credit,
}

impl From<NormalityChoice> for Normality {
    fn from(c: NormalityChoice) -> Normality {
        match c {
            NormalityChoice::Debit => Normality::Debit,
            NormalityChoice::Credit => Normality::Credit,
        }
    }
}

/// The normality a new ledger under `parent` most likely wants: that of the
/// ledgers already there, when they agree. Under `Expenses`, debit; under
/// `Liabilities`, credit.
fn normality_under(budget: &Budget, parent: &str) -> NormalityChoice {
    let parent = ledgit_core::tree::normalize(parent);
    let mut under = budget
        .ledgers
        .indices()
        .filter(|ix| {
            !parent.is_empty()
                && ledgit_core::tree::is_under(&budget.ledgers.name[ix.get()], &parent)
        })
        .map(|ix| budget.ledgers.normality[ix.get()]);
    match under.next() {
        Some(first) if under.all(|n| n == first) && first == Normality::Credit => {
            NormalityChoice::Credit
        }
        _ => NormalityChoice::Debit,
    }
}

impl LedgerForm {
    /// The full path the ledger will be created at.
    fn path(&self) -> String {
        let parent = ledgit_core::tree::normalize(&self.parent);
        let name = self.name.trim();
        match (parent.is_empty(), name.is_empty()) {
            (true, _) => ledgit_core::tree::normalize(name),
            (false, true) => String::new(),
            (false, false) => ledgit_core::tree::normalize(&format!("{parent}:{name}")),
        }
    }

    fn show(&mut self, ui: &mut Ui, budget: &Budget) -> Filled {
        ui.label(
            egui::RichText::new("Ledgers can never be deleted. Choose the name carefully.")
                .color(fmt::dim()),
        );
        ui.add_space(6.0);
        egui::Grid::new("ledger_form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Under");
            let place = if self.parent.trim().is_empty() {
                "(top level)".to_string()
            } else {
                ledgit_core::tree::normalize(&self.parent)
            };
            if let Some(Pick::Subtree(path)) = Picker::new("ledger_parent", budget)
                .paths(true)
                .selected_text(place)
                .width(280.0)
                .show(ui)
            {
                if path != ledgit_core::tree::normalize(&self.parent) {
                    self.normality = normality_under(budget, &path);
                }
                self.parent = path;
            }
            ui.end_row();

            label_row(ui, "Name", &mut self.name, "Chequing, or Wedding:Tuxedo");
            ui.label("");
            let path = self.path();
            ui.label(if path.is_empty() {
                egui::RichText::new("").small()
            } else {
                egui::RichText::new(format!("will be created as {path}")).small().color(fmt::dim())
            });
            ui.end_row();
            label_row(ui, "Description", &mut self.description, "optional");

            ui.label("Normality");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.normality, NormalityChoice::Debit, "Debit");
                ui.selectable_value(&mut self.normality, NormalityChoice::Credit, "Credit");
            });
            ui.end_row();

            date_row(ui, "Opened", "ledger_opened", &mut self.opened);
        });
        ui.label(
            egui::RichText::new(match self.normality {
                NormalityChoice::Debit => "Debit-normal: assets and expenses. Money in makes it bigger.",
                NormalityChoice::Credit => {
                    "Credit-normal: liabilities, income and equity. Shown as a positive amount owed or earned."
                }
            })
            .color(fmt::dim())
            .small(),
        );

        let (save, cancel) = footer(ui, "Stage ledger");
        if cancel {
            return Ok(Outcome::Cancelled);
        }
        if !save {
            return Ok(Outcome::Pending);
        }
        let opened: Date =
            self.opened.parse().map_err(|_| "Opened must be YYYY-MM-DD".to_string())?;
        if self.name.trim().is_empty() {
            return Err("A ledger needs a name".into());
        }
        Ok(Outcome::Submit(vec![Op::CreateLedger {
            uid: self.uid.unwrap_or_default(),
            name: self.path(),
            description: self.description.trim().to_string(),
            normality: self.normality.into(),
            opened,
        }]))
    }
}

// -------------------------------------------------------------- transaction

#[derive(Default)]
struct TxForm {
    /// Kept when editing a staged entry: its uid, and who made it.
    uid: Option<(TxUid, Parent)>,
    name: String,
    description: String,
    date: String,
    legs: LegEditor,
}

impl TxForm {
    fn show(&mut self, ui: &mut Ui, budget: &Budget) -> Filled {
        if budget.ledgers.len() < 2 {
            ui.label("Create at least two ledgers first.");
            let (_, cancel) = footer(ui, "Stage transaction");
            return if cancel { Ok(Outcome::Cancelled) } else { Ok(Outcome::Pending) };
        }
        egui::Grid::new("tx_form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            label_row(ui, "Name", &mut self.name, "Groceries");
            date_row(ui, "Date", "tx_date", &mut self.date);
            label_row(ui, "Description", &mut self.description, "optional");
        });

        ui.add_space(10.0);
        ui.label(egui::RichText::new("Sides").strong());
        ui.label(
            egui::RichText::new(
                "Debit what receives value, credit what gives it. Add sides for a split: \
                 a paycheque debits your chequing ledger, tax and pension, and credits \
                 gross pay - one entry, not three.",
            )
            .color(fmt::dim())
            .small(),
        );
        ui.add_space(6.0);
        self.legs.show(ui, budget, "tx_legs");

        let (save, cancel) = footer(ui, "Stage transaction");
        if cancel {
            return Ok(Outcome::Cancelled);
        }
        if !save {
            return Ok(Outcome::Pending);
        }
        let date: Date = self.date.parse().map_err(|_| "Date must be YYYY-MM-DD".to_string())?;
        let legs = self.legs.finish(&budget.variables)?;
        let name =
            budget.variables.substitute(self.name.trim()).map_err(|e| format!("Name: {e}"))?;
        let description = budget
            .variables
            .substitute(self.description.trim())
            .map_err(|e| format!("Description: {e}"))?;
        let (uid, parent) = self.uid.unwrap_or_else(|| (TxUid::new(), Parent::Manual));
        Ok(Outcome::Submit(vec![Op::PostTransaction {
            uid,
            name,
            description,
            date,
            legs,
            parent,
        }]))
    }
}

// ------------------------------------------------------- cohort and view

#[derive(Default)]
struct CohortForm {
    uid: Option<CohortUid>,
    name: String,
    description: String,
}

impl CohortForm {
    fn show(&mut self, ui: &mut Ui) -> Filled {
        ui.label(
            egui::RichText::new(
                "A cohort groups issuers, the way a bucket groups ledgers. It stops or changes no payment.",
            )
            .color(fmt::dim()),
        );
        ui.add_space(6.0);
        egui::Grid::new("cohort_form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            label_row(ui, "Name", &mut self.name, "Bills");
            label_row(ui, "Description", &mut self.description, "optional");
        });
        let (save, cancel) = footer(ui, "Stage cohort");
        if cancel {
            return Ok(Outcome::Cancelled);
        }
        if !save {
            return Ok(Outcome::Pending);
        }
        if self.name.trim().is_empty() {
            return Err("A cohort needs a name".into());
        }
        Ok(Outcome::Submit(vec![Op::CreateCohort {
            uid: self.uid.unwrap_or_default(),
            name: self.name.trim().to_string(),
            description: self.description.trim().to_string(),
        }]))
    }
}

/// Only the name is asked for here. What a view looks at is picked on the
/// Views screen, where the chart redraws as you pick it.
#[derive(Default)]
struct ViewForm {
    /// Kept when editing a staged view; its spec is edited on the Views screen.
    uid: Option<(ViewUid, ViewSpec)>,
    name: String,
    description: String,
    all_buckets: bool,
}

impl ViewForm {
    fn show(&mut self, ui: &mut Ui, budget: &Budget) -> Filled {
        ui.label(
            egui::RichText::new(
                "A view charts buckets and ledgers across time and simulates your issuers forward. It moves no money.",
            )
            .color(fmt::dim()),
        );
        ui.add_space(6.0);
        egui::Grid::new("view_form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            label_row(ui, "Name", &mut self.name, "Net worth, next year");
            label_row(ui, "Description", &mut self.description, "optional");
            if self.uid.is_none() {
                ui.label("");
                ui.checkbox(&mut self.all_buckets, "Start with every bucket");
                ui.end_row();
            }
        });
        let (save, cancel) = footer(ui, "Stage view");
        if cancel {
            return Ok(Outcome::Cancelled);
        }
        if !save {
            return Ok(Outcome::Pending);
        }
        if self.name.trim().is_empty() {
            return Err("A view needs a name".into());
        }
        let (uid, spec) = match &self.uid {
            Some((uid, spec)) => (*uid, spec.clone()),
            None if self.all_buckets => (ViewUid::new(), ViewSpec::all_buckets(budget)),
            None => (ViewUid::new(), ViewSpec::default()),
        };
        Ok(Outcome::Submit(vec![Op::CreateView {
            uid,
            name: self.name.trim().to_string(),
            description: self.description.trim().to_string(),
            spec,
        }]))
    }
}

// --------------------------------------------------------------------- move

/// Rename a subtree: every ledger at or under one path moves under another,
/// and buckets that include it follow. Names only; no money moves.
#[derive(Default)]
struct MoveForm {
    from: String,
    to: String,
}

impl MoveForm {
    fn show(&mut self, ui: &mut Ui, budget: &Budget) -> Filled {
        ui.label(
            egui::RichText::new(
                "Renames every ledger at or under a path. Balances, postings and history stay exactly where they are.",
            )
            .color(fmt::dim()),
        );
        ui.add_space(6.0);
        egui::Grid::new("move_form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            label_row(ui, "Move", &mut self.from, "Wedding");
            label_row(ui, "To", &mut self.to, "Events:Wedding");
        });
        let ops = ledgit_core::tree::rename_ops(budget, &self.from, &self.to);
        let moving = ops.iter().filter(|o| matches!(o, Op::EditLedger { .. })).count();
        ui.label(egui::RichText::new(format!("{moving} ledger(s) will move.")).color(fmt::dim()));
        let (save, cancel) = footer(ui, "Stage move");
        if cancel {
            return Ok(Outcome::Cancelled);
        }
        if !save {
            return Ok(Outcome::Pending);
        }
        if ledgit_core::tree::normalize(&self.to).is_empty() {
            return Err("Say where to move them".into());
        }
        if moving == 0 {
            return Err(format!("No ledger is at or under \"{}\"", self.from.trim()));
        }
        Ok(Outcome::Submit(ops))
    }
}

// ------------------------------------------------------------------- bucket

#[derive(Default)]
struct BucketForm {
    uid: Option<BucketUid>,
    name: String,
    description: String,
}

impl BucketForm {
    fn show(&mut self, ui: &mut Ui) -> Filled {
        ui.label(
            egui::RichText::new(
                "A bucket is a view over ledgers. Creating or deleting one moves no money.",
            )
            .color(fmt::dim()),
        );
        ui.add_space(6.0);
        egui::Grid::new("bucket_form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            label_row(ui, "Name", &mut self.name, "Net Worth");
            label_row(ui, "Description", &mut self.description, "optional");
        });
        let (save, cancel) = footer(ui, "Stage bucket");
        if cancel {
            return Ok(Outcome::Cancelled);
        }
        if !save {
            return Ok(Outcome::Pending);
        }
        if self.name.trim().is_empty() {
            return Err("A bucket needs a name".into());
        }
        Ok(Outcome::Submit(vec![Op::CreateBucket {
            uid: self.uid.unwrap_or_default(),
            name: self.name.trim().to_string(),
            description: self.description.trim().to_string(),
        }]))
    }
}

// ------------------------------------------------------------------- issuer

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Repeat {
    #[default]
    EveryNDays,
    Monthly,
    Once,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum AmountMode {
    #[default]
    Fixed,
    /// "Take 5% of this balance."
    Share,
    /// "6.45% APR on this loan."
    Interest,
}

#[derive(Default)]
struct IssuerForm {
    uid: Option<IssuerUid>,
    name: String,
    description: String,
    legs: LegEditor,
    amount: AmountMode,
    /// For an amount worked out from a balance: who receives it, who gives
    /// it, whose balance it reads, and the rate as a percentage.
    rule_debit: Option<LedgerUid>,
    rule_credit: Option<LedgerUid>,
    rule_of: Option<LedgerUid>,
    rate: String,
    repeat: Repeat,
    every_n_days: String,
    day_of_month: String,
    every_n_months: u32,
    start: String,
}

impl IssuerForm {
    fn show(&mut self, ui: &mut Ui, budget: &Budget) -> Filled {
        if budget.ledgers.len() < 2 {
            ui.label("Create at least two ledgers first.");
            let (_, cancel) = footer(ui, "Stage issuer");
            return if cancel { Ok(Outcome::Cancelled) } else { Ok(Outcome::Pending) };
        }
        ui.label(
            egui::RichText::new(
                "An issuer proposes transactions on a schedule. It never posts on its own - \
                 its entries land in the staging area for you to review.",
            )
            .color(fmt::dim()),
        );
        ui.add_space(6.0);
        egui::Grid::new("issuer_form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            label_row(ui, "Name", &mut self.name, "Car payment");

            ui.label("Repeats");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.repeat, Repeat::EveryNDays, "Every N days");
                ui.selectable_value(&mut self.repeat, Repeat::Monthly, "Monthly");
                ui.selectable_value(&mut self.repeat, Repeat::Once, "Once");
            });
            ui.end_row();

            match self.repeat {
                Repeat::EveryNDays => label_row(ui, "Days between", &mut self.every_n_days, "14"),
                Repeat::Monthly => {
                    label_row(ui, "Day of month", &mut self.day_of_month, "1 - 31");
                    ui.label("Every");
                    ui.horizontal(|ui| {
                        for (n, label) in [(1u32, "month"), (3, "quarter"), (12, "year")] {
                            ui.selectable_value(&mut self.every_n_months, n, label);
                        }
                    });
                    ui.end_row();
                }
                Repeat::Once => {}
            }

            date_row(ui, "Starts", "issuer_start", &mut self.start);
            label_row(ui, "Description", &mut self.description, "optional");
        });
        if self.repeat == Repeat::Monthly {
            ui.label(
                egui::RichText::new(
                    "Day 31 means the last day of the month; February will not drag the \
                     following months to the 28th.",
                )
                .color(fmt::dim())
                .small(),
            );
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Amount").strong());
            ui.selectable_value(&mut self.amount, AmountMode::Fixed, "Fixed");
            ui.selectable_value(&mut self.amount, AmountMode::Share, "% of a balance");
            ui.selectable_value(&mut self.amount, AmountMode::Interest, "Interest (APR)");
        });
        ui.add_space(4.0);
        match self.amount {
            AmountMode::Fixed => {
                ui.label(
                    egui::RichText::new(
                        "The whole entry is posted each time it fires, splits included.",
                    )
                    .color(fmt::dim())
                    .small(),
                );
                ui.add_space(6.0);
                self.legs.show(ui, budget, "issuer_legs");
            }
            mode => self.rule_editor(ui, budget, mode),
        }

        let (save, cancel) = footer(ui, "Stage issuer");
        if cancel {
            return Ok(Outcome::Cancelled);
        }
        if !save {
            return Ok(Outcome::Pending);
        }
        let start: Date = self.start.parse().map_err(|_| "Start must be YYYY-MM-DD".to_string())?;
        let (legs, rule) = match self.amount {
            AmountMode::Fixed => (self.legs.finish(&budget.variables)?, None),
            mode => self.finish_rule(budget, mode)?,
        };
        let name =
            budget.variables.substitute(self.name.trim()).map_err(|e| format!("Name: {e}"))?;
        let description = budget
            .variables
            .substitute(self.description.trim())
            .map_err(|e| format!("Description: {e}"))?;
        let schedule = match self.repeat {
            Repeat::Once => Schedule::Once,
            Repeat::EveryNDays => {
                let n: u32 = self
                    .every_n_days
                    .trim()
                    .parse()
                    .map_err(|_| "Days between must be a whole number".to_string())?;
                Schedule::EveryNDays { n }
            }
            Repeat::Monthly => {
                let day: u32 = self
                    .day_of_month
                    .trim()
                    .parse()
                    .map_err(|_| "Day of month must be a whole number".to_string())?;
                Schedule::MonthlyOn { day, every_n_months: self.every_n_months.max(1) }
            }
        };
        schedule.validate().map_err(|e| e.to_string())?;
        if let Some(r) = &rule {
            r.validate(&schedule).map_err(|e| format!("{}{}", e[..1].to_uppercase(), &e[1..]))?;
        }
        Ok(Outcome::Submit(vec![Op::CreateIssuer {
            uid: self.uid.unwrap_or_default(),
            name,
            description,
            legs,
            schedule,
            start,
            rule,
        }]))
    }

    fn rule_editor(&mut self, ui: &mut Ui, budget: &Budget, mode: AmountMode) {
        ui.label(
            egui::RichText::new(match mode {
                AmountMode::Interest => {
                    "Each time it fires: the balance of the ledger below, times the APR, times \
                     the days since the last time, over 365. Interest on a loan usually \
                     debits an interest expense and credits the loan."
                }
                _ => {
                    "Each time it fires: that share of the ledger's balance, moved from the \
                     giving ledger to the receiving one."
                }
            })
            .color(fmt::dim())
            .small(),
        );
        ui.add_space(6.0);
        let pick = |ui: &mut Ui, id: &str, value: &mut Option<LedgerUid>| {
            let text = value
                .map(|u| fmt::ledger_label(budget, u))
                .unwrap_or_else(|| "choose a ledger".to_string());
            if let Some(Pick::Ledger(uid)) =
                Picker::new(id, budget).selected_text(text).width(260.0).show(ui)
            {
                *value = Some(uid);
                true
            } else {
                false
            }
        };
        egui::Grid::new("issuer_rule").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Debit (receives)");
            pick(ui, "rule_debit", &mut self.rule_debit);
            ui.end_row();
            ui.label("Credit (gives)");
            if pick(ui, "rule_credit", &mut self.rule_credit) && self.rule_of.is_none() {
                // The ledger money leaves is usually the one it is a share
                // of, and the loan interest is charged on.
                self.rule_of = self.rule_credit;
            }
            ui.end_row();
            ui.label(if mode == AmountMode::Interest { "Interest on" } else { "Share of" });
            pick(ui, "rule_of", &mut self.rule_of);
            ui.end_row();
            ui.label(if mode == AmountMode::Interest { "APR %" } else { "Percent" });
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.rate)
                        .hint_text(if mode == AmountMode::Interest { "6.45" } else { "5" })
                        .desired_width(80.0),
                )
                .on_hover_text("A percentage, or a variable holding one");
                if let (Ok(rate), Some(of)) = (self.parse_rate(budget), self.rule_of) {
                    if let Some(ix) = budget.ledgers.ix(of) {
                        let rule = self.rule(mode, of, rate);
                        let date = Date::today_utc();
                        let schedule = self.schedule().unwrap_or(Schedule::Once);
                        let now = ledgit_core::issuer::rule_amount(
                            rule,
                            schedule,
                            date,
                            budget.ledgers.balance(ix),
                        );
                        ui.label(
                            egui::RichText::new(match now {
                                Some(m) => format!("about {} on today's balance", fmt::amount(m)),
                                None => "nothing on today's balance".to_string(),
                            })
                            .small()
                            .color(fmt::dim()),
                        );
                    }
                }
            });
            ui.end_row();
        });
    }

    fn parse_rate(&self, budget: &Budget) -> std::result::Result<Rate, String> {
        let text = self.rate.trim().trim_end_matches('%');
        Rate::parse_percent(text).or_else(|_| {
            let r = ledgit_core::expr::eval(text, &budget.variables).map_err(|e| e.to_string())?;
            Rate::parse_percent(&r.to_string())
        })
    }

    fn rule(&self, mode: AmountMode, of: LedgerUid, rate: Rate) -> AmountRule {
        match mode {
            AmountMode::Interest => AmountRule::Interest { of, apr: rate },
            _ => AmountRule::ShareOfBalance { of, rate },
        }
    }

    /// The schedule as typed, if it parses; for the rule preview.
    fn schedule(&self) -> Option<Schedule> {
        Some(match self.repeat {
            Repeat::Once => Schedule::Once,
            Repeat::EveryNDays => {
                Schedule::EveryNDays { n: self.every_n_days.trim().parse().ok()? }
            }
            Repeat::Monthly => Schedule::MonthlyOn {
                day: self.day_of_month.trim().parse().ok()?,
                every_n_months: self.every_n_months.max(1),
            },
        })
    }

    fn finish_rule(
        &self,
        budget: &Budget,
        mode: AmountMode,
    ) -> std::result::Result<(Vec<Leg>, Option<AmountRule>), String> {
        let debit = self.rule_debit.ok_or("Choose the ledger that receives it")?;
        let credit = self.rule_credit.ok_or("Choose the ledger that gives it")?;
        if debit == credit {
            return Err("It cannot move money from a ledger to itself".into());
        }
        let of = self.rule_of.ok_or("Choose whose balance it is worked out from")?;
        let rate = self.parse_rate(budget).map_err(|e| format!("Rate: {e}"))?;
        // The legs only say which way it goes; the rule sets the amount.
        Ok((simple_legs(debit, credit, Money::from_major(1)), Some(self.rule(mode, of, rate))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> Budget {
        let mk = |name: &str, normality| Op::CreateLedger {
            uid: LedgerUid::new(),
            name: name.into(),
            description: String::new(),
            normality,
            opened: Date::from_ymd(2024, 1, 1).unwrap(),
        };
        Budget::replay(&[
            mk("Liabilities:Car Loan", Normality::Credit),
            mk("Liabilities:Card", Normality::Credit),
            mk("Assets:Cash", Normality::Debit),
            mk("Mixed:A", Normality::Debit),
            mk("Mixed:B", Normality::Credit),
        ])
        .unwrap()
    }

    #[test]
    fn a_ledger_is_named_by_its_place_in_the_tree() {
        let mut f = LedgerForm {
            parent: "Assets: Bank".into(),
            name: " Savings ".into(),
            ..Default::default()
        };
        assert_eq!(f.path(), "Assets:Bank:Savings");
        f.name = "Joint:Chequing".into();
        assert_eq!(f.path(), "Assets:Bank:Joint:Chequing", "a colon in the name still nests");
        f.parent.clear();
        assert_eq!(f.path(), "Joint:Chequing");
        f.parent = "Assets".into();
        f.name.clear();
        assert_eq!(f.path(), "", "no name, no ledger - not a ledger called Assets");
    }

    #[test]
    fn a_new_ledger_takes_the_normality_of_its_neighbours() {
        let b = budget();
        assert!(normality_under(&b, "Liabilities") == NormalityChoice::Credit);
        assert!(normality_under(&b, "Assets") == NormalityChoice::Debit);
        assert!(normality_under(&b, "Mixed") == NormalityChoice::Debit, "no agreement, no guess");
        assert!(normality_under(&b, "") == NormalityChoice::Debit);
        assert!(normality_under(&b, "Nowhere") == NormalityChoice::Debit);
    }

    #[test]
    fn opening_under_a_level_seeds_the_form() {
        let b = budget();
        let mut forms = Forms::default();
        forms.open_ledger_under("Liabilities", &b);
        assert_eq!(forms.open, Some(FormKind::Ledger));
        assert_eq!(forms.ledger.parent, "Liabilities");
        assert!(forms.ledger.normality == NormalityChoice::Credit);
        let form = std::cell::RefCell::new(forms.ledger);
        egui::__run_test_ui(|ui| {
            let _ = form.borrow_mut().show(ui, &b);
        });
    }

    /// Every staged change with a form reopens in it, keeps its uid, and
    /// round-trips: legs out of the editor are the legs that went in.
    #[test]
    fn a_staged_change_reopens_in_its_form() {
        let b = budget();
        let (a, c) = (b.ledgers.uid[0], b.ledgers.uid[2]);
        let legs = vec![
            Leg::debit(a, Money(12_345)),
            Leg::debit(b.ledgers.uid[1], Money(55)),
            Leg::credit(c, Money(12_400)),
        ];
        assert_eq!(LegEditor::from_legs(&legs).finish(&b.variables).unwrap(), legs);

        let tx = TxUid::new();
        let mut forms = Forms::default();
        let op = Op::PostTransaction {
            uid: tx,
            name: "Paycheque".into(),
            description: String::new(),
            date: Date::from_ymd(2024, 3, 1).unwrap(),
            legs,
            parent: Parent::Manual,
        };
        assert!(forms.edit_staged(3, &op));
        assert_eq!(forms.editing, Some(3));
        assert_eq!(forms.title(), "Edit staged transaction");
        assert_eq!(forms.transaction.uid.map(|(u, _)| u), Some(tx));
        assert_eq!(forms.transaction.date, "2024-03-01");

        let ledger = Op::CreateLedger {
            uid: LedgerUid::new(),
            name: "Assets:Bank:Savings".into(),
            description: "d".into(),
            normality: Normality::Credit,
            opened: Date::from_ymd(2024, 1, 1).unwrap(),
        };
        assert!(forms.edit_staged(0, &ledger));
        assert_eq!(forms.ledger.parent, "Assets:Bank");
        assert_eq!(forms.ledger.path(), "Assets:Bank:Savings");
        assert!(forms.ledger.normality == NormalityChoice::Credit);

        let issuer = Op::CreateIssuer {
            uid: IssuerUid::new(),
            name: "Rent".into(),
            description: String::new(),
            legs: simple_legs(a, c, Money(100)),
            schedule: Schedule::MonthlyOn { day: 31, every_n_months: 3 },
            start: Date::from_ymd(2024, 1, 1).unwrap(),
            rule: None,
        };
        assert!(forms.edit_staged(1, &issuer));
        assert!(forms.issuer.repeat == Repeat::Monthly);
        assert_eq!((forms.issuer.day_of_month.as_str(), forms.issuer.every_n_months), ("31", 3));

        // An interest issuer reopens with its rule, and resubmits it intact.
        let rule = AmountRule::Interest { of: c, apr: Rate(64_500) };
        let interest = Op::CreateIssuer {
            uid: IssuerUid::new(),
            name: "Interest".into(),
            description: String::new(),
            legs: simple_legs(a, c, Money::from_major(1)),
            schedule: Schedule::MonthlyOn { day: 1, every_n_months: 1 },
            start: Date::from_ymd(2024, 1, 1).unwrap(),
            rule: Some(rule),
        };
        assert!(forms.edit_staged(2, &interest));
        assert_eq!(forms.issuer.amount, AmountMode::Interest);
        assert_eq!(forms.issuer.rate, "6.45");
        let (legs, back) = forms.issuer.finish_rule(&b, AmountMode::Interest).unwrap();
        assert_eq!((legs, back), (simple_legs(a, c, Money::from_major(1)), Some(rule)));
        let form = std::cell::RefCell::new(std::mem::take(&mut forms.issuer));
        egui::__run_test_ui(|ui| {
            let _ = form.borrow_mut().show(ui, &b);
        });

        // Opening a fresh form afterwards is not an edit.
        forms.open(FormKind::Bucket, &b);
        assert_eq!(forms.editing, None);
        assert!(!forms.edit_staged(0, &Op::DeleteBucket { uid: BucketUid::new() }));
    }

    #[test]
    fn an_amount_can_be_a_formula_over_the_variables() {
        let mut b = budget();
        b.apply(&Op::SetVariable { name: "Car_Km_Rate".into(), value: VarValue::guess("0.68") })
            .unwrap();
        let (a, c) = (b.ledgers.uid[2], b.ledgers.uid[0]);
        let mut e = LegEditor::from_legs(&simple_legs(a, c, Money(1)));
        e.rows[0].amount = "200 * car_km_rate".into();
        e.rows[1].amount = "136".into();
        assert!(e.imbalance(&b.variables).is_zero());
        assert_eq!(e.finish(&b.variables).unwrap(), simple_legs(a, c, Money(13_600)));
        e.rows[1].amount = "2 * Nope".into();
        let err = e.finish(&b.variables).unwrap_err();
        assert!(err.contains("Side 2") && err.contains("Nope"), "{err}");
        // The editor draws the worked-out value and the error without panicking.
        let e = std::cell::RefCell::new(e);
        egui::__run_test_ui(|ui| e.borrow_mut().show(ui, &b, "t"));
    }
}
