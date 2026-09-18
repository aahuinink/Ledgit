//! Entry forms.
//!
//! Each form collects text, validates it into exactly one [`Op`], and hands it
//! back. They never touch the repo - the caller stages what comes out. That
//! keeps "what the user typed" and "what the budget does" separable, which is
//! why the same forms can later be reused for an import or an edit dialog.

use crate::fmt;
use egui::{ComboBox, Ui};
use ledgit_core::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormKind {
    Ledger,
    Transaction,
    Bucket,
    Issuer,
}

impl FormKind {
    /// How wide the modal needs to be. The entry forms carry a leg editor -
    /// a debit/credit toggle, a ledger picker, an amount and a remove
    /// button on one row - which does not fit the width a name-and-description
    /// form wants.
    pub fn width(self) -> f32 {
        match self {
            FormKind::Ledger | FormKind::Bucket => 460.0,
            FormKind::Transaction | FormKind::Issuer => 640.0,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            FormKind::Ledger => "New ledger",
            FormKind::Transaction => "New transaction",
            FormKind::Bucket => "New bucket",
            FormKind::Issuer => "New issuer",
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
    ledger: LedgerForm,
    transaction: TxForm,
    bucket: BucketForm,
    issuer: IssuerForm,
}

impl Forms {
    /// Open a form, seeding date fields with today so the common case is one
    /// amount and two clicks.
    pub fn open(&mut self, kind: FormKind, budget: &Budget) {
        let today = Date::today_utc().to_string();
        match kind {
            FormKind::Ledger => self.ledger = LedgerForm { opened: today, ..Default::default() },
            FormKind::Transaction => {
                self.transaction =
                    TxForm { date: today, legs: LegEditor::seed(budget), ..Default::default() };
            }
            FormKind::Bucket => self.bucket = BucketForm::default(),
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

    pub fn show(&mut self, ui: &mut Ui, budget: &Budget) -> Outcome {
        let Some(kind) = self.open else {
            return Outcome::Pending;
        };
        let outcome = match kind {
            FormKind::Ledger => self.ledger.show(ui),
            FormKind::Transaction => self.transaction.show(ui, budget),
            FormKind::Bucket => self.bucket.show(ui),
            FormKind::Issuer => self.issuer.show(ui, budget),
        };
        match outcome {
            Ok(o) => {
                if !matches!(o, Outcome::Pending) {
                    self.open = None;
                    self.error = None;
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

    /// Signed amounts for every row that parses, debit-positive.
    fn parsed(&self) -> Vec<(Option<LedgerUid>, Option<Money>)> {
        self.rows
            .iter()
            .map(|r| {
                let m = Money::parse(r.amount.trim()).ok().filter(|m| m.cents() > 0).map(|m| {
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
    fn imbalance(&self) -> Money {
        self.parsed().iter().filter_map(|(_, m)| *m).sum()
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
                ComboBox::from_id_salt(format!("{id}_acct_{i}"))
                    .selected_text(text)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for ix in budget.ledgers.indices() {
                            let uid = budget.ledgers.uid[ix.get()];
                            let label = format!(
                                "{}  [{}]",
                                budget.ledgers.name[ix.get()],
                                budget.ledgers.normality[ix.get()]
                            );
                            ui.selectable_value(&mut row.ledger, Some(uid), label);
                        }
                    });

                ui.add(
                    egui::TextEdit::singleline(&mut row.amount)
                        .hint_text("0.00")
                        .desired_width(90.0),
                );

                // Never let the form drop below a two-sided entry.
                if ui.add_enabled(removable, egui::Button::new("\u{2715}").small()).clicked() {
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

            let out = self.imbalance();
            if out.is_zero() {
                ui.colored_label(fmt::good(), "balanced");
            } else {
                ui.colored_label(fmt::bad(), format!("out by {}", fmt::amount(out)));
                if ui
                    .small_button("balance the last side")
                    .on_hover_text("Set the last side to whatever makes the entry balance")
                    .clicked()
                {
                    self.balance_last();
                }
            }
        });
    }

    fn rows_removable(&self) -> bool {
        self.rows.len() > 2
    }

    /// Give the last row whatever amount squares the entry.
    fn balance_last(&mut self) {
        let others: Money =
            self.parsed().iter().take(self.rows.len() - 1).filter_map(|(_, m)| *m).sum();
        if let Some(last) = self.rows.last_mut() {
            if others.is_zero() {
                return;
            }
            last.is_debit = others.cents() < 0;
            last.amount = Money(others.cents().abs()).to_string();
        }
    }

    fn finish(&self) -> std::result::Result<Vec<Leg>, String> {
        let mut legs = Vec::with_capacity(self.rows.len());
        for (i, row) in self.rows.iter().enumerate() {
            let ledger = row.ledger.ok_or_else(|| format!("Side {} has no ledger", i + 1))?;
            let amount = Money::parse(row.amount.trim())
                .map_err(|_| format!("Side {} needs an amount like 42.50", i + 1))?;
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

impl LedgerForm {
    fn show(&mut self, ui: &mut Ui) -> Filled {
        ui.label(
            egui::RichText::new("Ledgers can never be deleted. Choose the name carefully.")
                .color(fmt::dim()),
        );
        ui.add_space(6.0);
        egui::Grid::new("ledger_form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            label_row(ui, "Name", &mut self.name, "Chequing");
            label_row(ui, "Description", &mut self.description, "optional");

            ui.label("Normality");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.normality, NormalityChoice::Debit, "Debit");
                ui.selectable_value(&mut self.normality, NormalityChoice::Credit, "Credit");
            });
            ui.end_row();

            label_row(ui, "Opened", &mut self.opened, "YYYY-MM-DD");
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
            uid: LedgerUid::new(),
            name: self.name.trim().to_string(),
            description: self.description.trim().to_string(),
            normality: self.normality.into(),
            opened,
        }]))
    }
}

// -------------------------------------------------------------- transaction

#[derive(Default)]
struct TxForm {
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
            label_row(ui, "Date", &mut self.date, "YYYY-MM-DD");
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
        let legs = self.legs.finish()?;
        Ok(Outcome::Submit(vec![Op::PostTransaction {
            uid: TxUid::new(),
            name: self.name.trim().to_string(),
            description: self.description.trim().to_string(),
            date,
            legs,
            parent: Parent::Manual,
        }]))
    }
}

// ------------------------------------------------------------------- bucket

#[derive(Default)]
struct BucketForm {
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
            uid: BucketUid::new(),
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

#[derive(Default)]
struct IssuerForm {
    name: String,
    description: String,
    legs: LegEditor,
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

            label_row(ui, "Starts", &mut self.start, "YYYY-MM-DD");
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
        ui.label(egui::RichText::new("Sides").strong());
        ui.label(
            egui::RichText::new("The whole entry is posted each time it fires, splits included.")
                .color(fmt::dim())
                .small(),
        );
        ui.add_space(6.0);
        self.legs.show(ui, budget, "issuer_legs");

        let (save, cancel) = footer(ui, "Stage issuer");
        if cancel {
            return Ok(Outcome::Cancelled);
        }
        if !save {
            return Ok(Outcome::Pending);
        }
        let start: Date = self.start.parse().map_err(|_| "Start must be YYYY-MM-DD".to_string())?;
        let legs = self.legs.finish()?;
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
        Ok(Outcome::Submit(vec![Op::CreateIssuer {
            uid: IssuerUid::new(),
            name: self.name.trim().to_string(),
            description: self.description.trim().to_string(),
            legs,
            schedule,
            start,
        }]))
    }
}
