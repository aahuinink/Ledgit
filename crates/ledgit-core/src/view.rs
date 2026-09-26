//! Saved views: readings of the budget across time.
//!
//! A [`ViewSpec`] names some buckets, ledgers, issuers, cohorts and a filter
//! over transactions. [`evaluate`] turns it into a [`ViewReport`]:
//!
//! * **series** - the balance of the view's scope, of each bucket and of each
//!   named ledger, from `lookback` ago to `horizon` ahead. Up to today they
//!   come from posted transactions; after today, from simulating the issuers
//!   forward;
//! * **flows** - what each selected issuer brings in or takes out per period;
//! * **timeline** - per calendar period, what actually moved and what the
//!   simulation expects to move.
//!
//! # Scope, and why every number has a direction
//!
//! "Spent" and "received" only mean something relative to a pot of money: rent
//! is spending to your chequing account and income to your landlord. A view's
//! pot is its **scope** - its bucket combination if it has one, otherwise its
//! ledgers - and every posting is weighed against it by one integer per
//! ledger:
//!
//! ```text
//! effect of a posting = weight[ledger] * amount      (amount is debit-positive)
//! ```
//!
//! The weight is `+1`/`-1` from the term's sign, times the ledger's normality
//! sign if the roll-up presents balances as shown. A ledger out of scope has
//! weight 0, so a transfer between two in-scope ledgers nets to nothing -
//! moving money between your own accounts is neither spending nor income -
//! with no special case. A view with no scope at all is *undirected*, and its
//! flows are reported as volume only.
//!
//! # The simulation
//!
//! Nothing is staged or written. The simulation lists every occurrence the
//! chosen issuers still owe up to the horizon (`issuer::due_dates`, the same
//! function that posts them for real), turns each into its legs, and sweeps
//! those together with the real postings in date order. An occurrence already
//! overdue is placed on today: it has not happened yet, so it cannot move a
//! past balance, but it will land as soon as the issuers are run.

use crate::date::Date;
use crate::id::{BucketUid, CohortUid, IssuerIx, LedgerIx, TxIx};
use crate::model::{magnitude, Schedule, ViewSpec};
use crate::money::Money;
use crate::period::{per_period, Period};
use crate::query::{members, Members, RollUp, TxFilter, TxQuery};
use crate::state::Budget;

/// What one charted line tracks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeriesKind {
    /// The whole scope: the bucket combination, or the ledgers summed.
    Scope,
    Bucket(BucketUid),
    Ledger(LedgerIx),
}

/// One line on the chart: a balance through time.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Series {
    pub label: String,
    pub kind: SeriesKind,
    /// The balance at the end of each date on which it changed, oldest first,
    /// plus the window's first day, today and last day. Draw it as steps: a
    /// balance holds its value until the next point.
    pub points: Vec<(Date, Money)>,
    pub now: Money,
    pub at_end: Money,
    /// The lowest point from today to the end, and when it happens. The
    /// question a projection exists to answer is usually "do I run dry?".
    pub lowest_ahead: (Date, Money),
}

impl Series {
    /// The balance at the end of `date`.
    pub fn value_at(&self, date: Date) -> Money {
        value_at(&self.points, date)
    }
}

fn value_at(points: &[(Date, Money)], date: Date) -> Money {
    match points.partition_point(|(d, _)| *d <= date) {
        0 => points.first().map_or(Money::ZERO, |p| p.1),
        n => points[n - 1].1,
    }
}

/// One issuer's flow into or out of the view's scope.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FlowLine {
    pub issuer: IssuerIx,
    pub name: String,
    /// Names of the view's cohorts this issuer was selected through. Empty if
    /// it was picked directly.
    pub via: Vec<String>,
    pub schedule: Schedule,
    /// The size of each entry.
    pub amount: Money,
    /// Each entry's signed effect on the scope: positive brings money in.
    /// `None` when the view is undirected.
    pub effect: Option<Money>,
    pub paused: bool,
    /// Whether the simulation runs this issuer forward.
    pub simulated: bool,
}

impl FlowLine {
    /// Signed rate per `period` (volume, if undirected). `None` for a one-off.
    pub fn rate(&self, period: Period) -> Option<Money> {
        per_period(self.effect.unwrap_or(self.amount), self.schedule, period)
    }
}

/// Money moving in one direction or the other over some stretch of time.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Flow {
    pub received: Money,
    /// Positive: the amount that left.
    pub spent: Money,
    /// Total size of the entries counted, regardless of direction.
    pub volume: Money,
    pub entries: usize,
}

impl Flow {
    pub fn net(&self) -> Money {
        self.received - self.spent
    }

    fn add(&mut self, effect: Option<Money>, size: Money) {
        match effect {
            Some(e) if e.0 > 0 => self.received += e,
            Some(e) => self.spent += -e,
            None => {}
        }
        self.volume += size;
        self.entries += 1;
    }
}

/// One calendar period of the timeline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PeriodRow {
    pub start: Date,
    /// Posted transactions matching the view.
    pub actual: Flow,
    /// Simulated issuer occurrences.
    pub projected: Flow,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ViewReport {
    pub today: Date,
    pub start: Date,
    pub end: Date,
    pub period: Period,
    /// The view has a scope, so flows have a direction.
    pub directed: bool,
    pub series: Vec<Series>,
    pub flows: Vec<FlowLine>,
    pub timeline: Vec<PeriodRow>,
    /// How many issuers drive the simulation.
    pub simulated_issuers: usize,
    /// Occurrences already due that the simulation placed on today.
    pub overdue_occurrences: usize,
    /// Ledgers that fell on both sides of the bucket combination and so count
    /// for nothing. Reported, as `query::combine` does, rather than hidden.
    pub cancelled: Vec<LedgerIx>,
    pub missing_buckets: Vec<BucketUid>,
    pub missing_cohorts: Vec<CohortUid>,
}

impl ViewReport {
    /// The combined rate of every running, recurring flow line.
    pub fn flow_totals(&self, period: Period) -> Flow {
        let mut f = Flow::default();
        for line in self.flows.iter().filter(|l| !l.paused) {
            let (Some(size), rate) =
                (per_period(line.amount, line.schedule, period), line.rate(period))
            else {
                continue;
            };
            f.add(line.effect.and(rate), size);
        }
        f
    }

    /// The timeline summed over the whole window.
    pub fn timeline_totals(&self) -> (Flow, Flow) {
        let mut actual = Flow::default();
        let mut projected = Flow::default();
        for r in &self.timeline {
            for (into, from) in [(&mut actual, &r.actual), (&mut projected, &r.projected)] {
                into.received += from.received;
                into.spent += from.spent;
                into.volume += from.volume;
                into.entries += from.entries;
            }
        }
        (actual, projected)
    }
}

/// Evaluate a view over the window its spec describes, relative to `today`.
pub fn evaluate(l: &Budget, spec: &ViewSpec, today: Date) -> ViewReport {
    let start = spec.lookback.before(today);
    let end = spec.horizon.after(today);
    evaluate_between(l, spec, today, start, end)
}

/// Evaluate a view over an explicit window, e.g. "simulate until 2027-06-30".
pub fn evaluate_between(
    l: &Budget,
    spec: &ViewSpec,
    today: Date,
    start: Date,
    end: Date,
) -> ViewReport {
    let end = end.max(today);
    let start = start.min(today);
    let n = l.ledgers.len();

    // ------------------------------------------------------------ scope
    let Members { plus, minus, missing: missing_buckets } = members(l, &spec.buckets);
    let roll_weight = |ix: LedgerIx| match spec.roll {
        RollUp::ByNormality => 1,
        RollUp::Sum => l.ledgers.normality[ix.get()].sign(),
    };
    let presented = |ix: LedgerIx| l.ledgers.normality[ix.get()].sign();

    let mut scope = vec![0i64; n];
    let mut cancelled = Vec::new();
    if spec.buckets.is_empty() {
        for uid in &spec.ledgers {
            if let Some(ix) = l.ledgers.ix(*uid) {
                scope[ix.get()] = presented(ix);
            }
        }
    } else {
        for ix in &plus {
            if minus.contains(ix) {
                cancelled.push(*ix);
            } else {
                scope[ix.get()] = roll_weight(*ix);
            }
        }
        for ix in minus.iter().filter(|ix| !plus.contains(ix)) {
            scope[ix.get()] = -roll_weight(*ix);
        }
    }
    let directed = scope.iter().any(|w| *w != 0);

    // ----------------------------------------------------------- series
    // Each series is a sparse list of (ledger, weight). Its value is the
    // weighted sum of those ledgers' raw balances.
    let mut defs: Vec<SeriesDef> = Vec::new();
    if directed {
        let weights = sparse(&scope);
        // A scope of one ledger is that ledger; call it by its name.
        let label = match (spec.buckets.is_empty(), weights.as_slice()) {
            (true, [(only, _)]) => l.ledgers.name[only.get()].clone(),
            _ => "Total".into(),
        };
        defs.push((label, SeriesKind::Scope, weights));
    }
    let live_terms: Vec<_> =
        spec.buckets.iter().filter_map(|t| l.buckets.ix(t.bucket).map(|b| (t.bucket, b))).collect();
    if live_terms.len() > 1 {
        for (uid, bix) in &live_terms {
            let weights =
                l.buckets.members[bix.get()].iter().map(|ix| (*ix, roll_weight(*ix))).collect();
            defs.push((l.buckets.name[bix.get()].clone(), SeriesKind::Bucket(*uid), weights));
        }
    }
    for uid in &spec.ledgers {
        let Some(ix) = l.ledgers.ix(*uid) else { continue };
        // A single ledger with no buckets *is* the scope; do not draw it twice.
        if spec.buckets.is_empty() && spec.ledgers.len() == 1 {
            continue;
        }
        defs.push((
            l.ledgers.name[ix.get()].clone(),
            SeriesKind::Ledger(ix),
            vec![(ix, presented(ix))],
        ));
    }

    // Which series each ledger feeds, and with what weight. Dense by ledger
    // row, so the sweep below does one index per posting.
    let mut feeds: Vec<Vec<(u32, i64)>> = vec![Vec::new(); n];
    for (si, (_, _, weights)) in defs.iter().enumerate() {
        for (ix, w) in weights {
            feeds[ix.get()].push((si as u32, *w));
        }
    }

    // ----------------------------------------------------------- issuers
    let mut flow_set: Vec<(IssuerIx, Vec<String>)> = Vec::new();
    let mut missing_cohorts = Vec::new();
    for uid in &spec.issuers {
        if let Some(ix) = l.issuers.ix(*uid) {
            if !flow_set.iter().any(|(s, _)| *s == ix) {
                flow_set.push((ix, Vec::new()));
            }
        }
    }
    for uid in &spec.cohorts {
        let Some(cix) = l.cohorts.ix(*uid) else {
            missing_cohorts.push(*uid);
            continue;
        };
        let cname = &l.cohorts.name[cix.get()];
        for ix in &l.cohorts.members[cix.get()] {
            match flow_set.iter_mut().find(|(s, _)| s == ix) {
                Some((_, via)) => via.push(cname.clone()),
                None => flow_set.push((*ix, vec![cname.clone()])),
            }
        }
    }
    let selected: Vec<IssuerIx> = flow_set.iter().map(|(s, _)| *s).collect();

    // The effect of one issuer entry on the scope.
    let issuer_effect = |ix: IssuerIx| -> Money {
        l.issuers.legs[ix.get()]
            .iter()
            .filter_map(|leg| {
                l.ledgers.ix(leg.ledger).map(|a| Money(scope[a.get()] * leg.amount.0))
            })
            .sum()
    };

    let simulated: Vec<IssuerIx> = if spec.only_selected_issuers {
        selected.iter().copied().filter(|ix| !l.issuers.paused[ix.get()]).collect()
    } else {
        l.issuers.indices().filter(|ix| !l.issuers.paused[ix.get()]).collect()
    };

    // With nothing selected, the flow table shows whatever moves the scope.
    if flow_set.is_empty() && directed {
        flow_set = simulated
            .iter()
            .filter(|ix| !issuer_effect(**ix).is_zero())
            .map(|ix| (*ix, Vec::new()))
            .collect();
    }
    let mut flows: Vec<FlowLine> = flow_set
        .into_iter()
        .map(|(ix, via)| {
            let i = ix.get();
            FlowLine {
                issuer: ix,
                name: l.issuers.name[i].clone(),
                via,
                schedule: l.issuers.schedule[i],
                amount: l.issuers.amount(ix),
                effect: directed.then(|| issuer_effect(ix)),
                paused: l.issuers.paused[i],
                simulated: simulated.contains(&ix),
            }
        })
        .collect();
    flows.sort_by(|a, b| {
        a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.issuer.0.cmp(&b.issuer.0))
    });

    // ------------------------------------------------------------ events
    // Flat columns, one row per posting that moves a charted ledger: the real
    // ones from the posting arena, then the simulated ones.
    let mut ev_date: Vec<Date> = Vec::new();
    let mut ev_ledger: Vec<LedgerIx> = Vec::new();
    let mut ev_amount: Vec<Money> = Vec::new();
    let mut opening = vec![Money::ZERO; defs.len()];

    for (a, fed) in feeds.iter().enumerate() {
        if fed.is_empty() {
            continue;
        }
        for p in &l.ledgers.postings[a] {
            let date = l.transactions.date[l.postings.tx[p.get()].get()];
            let amount = l.postings.amount[p.get()];
            if date < start {
                for (si, w) in fed {
                    opening[*si as usize] += Money(w * amount.0);
                }
            } else if date <= end {
                ev_date.push(date);
                ev_ledger.push(LedgerIx(a as u32));
                ev_amount.push(amount);
            }
        }
    }

    // Occurrences the simulation expects: (date, issuer), overdue on today.
    let mut overdue_occurrences = 0;
    let mut occurrences: Vec<(Date, IssuerIx)> = Vec::new();
    for ix in &simulated {
        for date in crate::issuer::due_dates(l, *ix, end) {
            if date < today {
                overdue_occurrences += 1;
            }
            occurrences.push((date.max(today), *ix));
        }
    }
    for (date, ix) in &occurrences {
        for leg in &l.issuers.legs[ix.get()] {
            let Some(a) = l.ledgers.ix(leg.ledger) else { continue };
            if !feeds[a.get()].is_empty() {
                ev_date.push(*date);
                ev_ledger.push(a);
                ev_amount.push(leg.amount);
            }
        }
    }

    // ------------------------------------------------------------- sweep
    let mut order: Vec<u32> = (0..ev_date.len() as u32).collect();
    order.sort_by_key(|e| ev_date[*e as usize]);

    let mut value = opening.clone();
    let mut points: Vec<Vec<(Date, Money)>> = opening.iter().map(|v| vec![(start, *v)]).collect();
    let mut changed = vec![false; defs.len()];
    let mut k = 0;
    while k < order.len() {
        let date = ev_date[order[k] as usize];
        while k < order.len() && ev_date[order[k] as usize] == date {
            let e = order[k] as usize;
            for (si, w) in &feeds[ev_ledger[e].get()] {
                value[*si as usize] += Money(w * ev_amount[e].0);
                changed[*si as usize] = true;
            }
            k += 1;
        }
        for si in 0..defs.len() {
            if std::mem::take(&mut changed[si]) {
                push_point(&mut points[si], date, value[si]);
            }
        }
    }

    let series = defs
        .into_iter()
        .zip(points)
        .map(|((label, kind, _), mut pts)| {
            // Pin today and the last day, so every line reaches both.
            for d in [today, end] {
                let v = value_at(&pts, d);
                let at = pts.partition_point(|(pd, _)| *pd < d);
                if pts.get(at).is_none_or(|(pd, _)| *pd != d) {
                    pts.insert(at, (d, v));
                }
            }
            let lowest_ahead = pts
                .iter()
                .filter(|(d, _)| *d >= today)
                .min_by_key(|(d, v)| (*v, *d))
                .copied()
                .unwrap_or((today, Money::ZERO));
            Series {
                label,
                kind,
                now: value_at(&pts, today),
                at_end: value_at(&pts, end),
                lowest_ahead,
                points: pts,
            }
        })
        .collect();

    // ---------------------------------------------------------- timeline
    let mut timeline: Vec<PeriodRow> = Vec::new();
    let mut p = spec.period.start_of(start);
    while p <= end {
        timeline.push(PeriodRow { start: p, actual: Flow::default(), projected: Flow::default() });
        p = spec.period.next_start(p);
    }
    let starts: Vec<Date> = timeline.iter().map(|r| r.start).collect();
    let row_of = |d: Date| starts.partition_point(|s| *s <= d).saturating_sub(1);

    let tx_effect = |t: TxIx| -> Option<Money> {
        directed.then(|| {
            l.transactions
                .leg_range(t)
                .map(|p| Money(scope[l.postings.ledger[p].get()] * l.postings.amount[p].0))
                .sum()
        })
    };
    for t in history_transactions(l, spec, &scope, &selected, directed, start, end) {
        let effect = tx_effect(t);
        if effect.is_some_and(|e| e.is_zero()) {
            continue; // an internal transfer, or not in scope at all
        }
        let r = row_of(l.transactions.date[t.get()]);
        timeline[r].actual.add(effect, l.amount_of(t));
    }
    // Undirected, the simulated side counts only what the view selected;
    // otherwise it would be every issuer's volume and say nothing about it.
    for (date, ix) in &occurrences {
        if !directed && !selected.contains(ix) {
            continue;
        }
        let effect = directed.then(|| issuer_effect(*ix));
        if effect.is_some_and(|e| e.is_zero()) {
            continue;
        }
        let r = row_of(*date);
        timeline[r].projected.add(effect, magnitude(&l.issuers.legs[ix.get()]));
    }

    ViewReport {
        today,
        start,
        end,
        period: spec.period,
        directed,
        series,
        flows,
        timeline,
        simulated_issuers: simulated.len(),
        overdue_occurrences,
        cancelled,
        missing_buckets,
        missing_cohorts,
    }
}

/// Posted transactions in the window that the view counts as its history.
///
/// The spec's own filters come first. With none, a directed view counts
/// everything that touches its scope, found through the scope's posting lists
/// rather than by scanning every transaction; an undirected one counts what
/// its selected issuers posted, or everything if it selected none.
fn history_transactions(
    l: &Budget,
    spec: &ViewSpec,
    scope: &[i64],
    selected: &[IssuerIx],
    directed: bool,
    start: Date,
    end: Date,
) -> Vec<TxIx> {
    if !spec.transactions.is_empty() || (!directed && selected.is_empty()) {
        let mut q = TxQuery::new();
        q.filters = spec.transactions.clone();
        q.filters.push(TxFilter::OnOrAfter(start));
        q.filters.push(TxFilter::OnOrBefore(end));
        return q.run(l);
    }
    let in_window = |t: &TxIx| {
        let d = l.transactions.date[t.get()];
        d >= start && d <= end
    };
    if directed {
        let mut out: Vec<TxIx> = scope
            .iter()
            .enumerate()
            .filter(|(_, w)| **w != 0)
            .flat_map(|(a, _)| l.ledgers.postings[a].iter().map(|p| l.postings.tx[p.get()]))
            .filter(in_window)
            .collect();
        // A transfer between two scope ledgers is listed by both.
        out.sort_unstable_by_key(|t| t.0);
        out.dedup();
        return out;
    }
    let uids: Vec<_> = selected.iter().map(|ix| l.issuers.uid[ix.get()]).collect();
    l.transactions
        .indices()
        .filter(in_window)
        .filter(|t| l.transactions.parent[t.get()].issuer().is_some_and(|u| uids.contains(&u)))
        .collect()
}

/// A series before it is swept: what to call it, and which ledgers it sums
/// with what weights.
type SeriesDef = (String, SeriesKind, Vec<(LedgerIx, i64)>);

fn sparse(weights: &[i64]) -> Vec<(LedgerIx, i64)> {
    weights
        .iter()
        .enumerate()
        .filter(|(_, w)| **w != 0)
        .map(|(i, w)| (LedgerIx(i as u32), *w))
        .collect()
}

/// Append a point, replacing the last one if it is for the same date.
fn push_point(points: &mut Vec<(Date, Money)>, date: Date, value: Money) {
    match points.last_mut() {
        Some(last) if last.0 == date => last.1 = value,
        _ => points.push((date, value)),
    }
}

impl ViewSpec {
    /// A spec that totals every live bucket: "the balances of all my
    /// buckets" in one line each, plus their combination.
    pub fn all_buckets(l: &Budget) -> ViewSpec {
        ViewSpec {
            buckets: l
                .buckets
                .live()
                .map(|b| crate::query::Term::plus(l.buckets.uid[b.get()]))
                .collect(),
            ..ViewSpec::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{BucketUid, CohortUid, IssuerUid, LedgerUid, TxUid};
    use crate::model::{simple_legs, Normality, Parent};
    use crate::op::Op;
    use crate::period::Span;
    use crate::query::Term;

    fn d(y: i32, m: u32, day: u32) -> Date {
        Date::from_ymd(y, m, day).unwrap()
    }

    fn major(n: i64) -> Money {
        Money::from_major(n)
    }

    struct Fx {
        l: Budget,
        cash: LedgerUid,
        savings: LedgerUid,
        visa: LedgerUid,
        liquid: BucketUid,
        debt: BucketUid,
        pay: IssuerUid,
        rent: IssuerUid,
        sweep: IssuerUid,
        bills: CohortUid,
    }

    /// Chequing and savings (bucket "Liquid"), a Visa (bucket "Debt"), and
    /// three monthly issuers: pay in, rent out, and a sweep from chequing to
    /// savings that should count as neither.
    fn fixture() -> Fx {
        let [cash, savings, visa, salary, rent_l, equity] =
            std::array::from_fn(|_| LedgerUid::new());
        let (liquid, debt) = (BucketUid::new(), BucketUid::new());
        let (pay, rent, sweep) = (IssuerUid::new(), IssuerUid::new(), IssuerUid::new());
        let bills = CohortUid::new();
        let ledger = |uid, name: &str, n| Op::CreateLedger {
            uid,
            name: name.into(),
            description: String::new(),
            normality: n,
            opened: d(2024, 1, 1),
        };
        let issuer = |uid, name: &str, legs, day| Op::CreateIssuer {
            uid,
            name: name.into(),
            description: String::new(),
            legs,
            schedule: Schedule::MonthlyOn { day, every_n_months: 1 },
            start: d(2024, 1, 1),
        };
        let post = |legs, date| Op::PostTransaction {
            uid: TxUid::new(),
            name: "t".into(),
            description: String::new(),
            date,
            legs,
            parent: Parent::Manual,
        };
        let bucket = |uid, name: &str| Op::CreateBucket {
            uid,
            name: name.into(),
            description: String::new(),
        };
        let l = Budget::replay(&[
            ledger(cash, "Chequing", Normality::Debit),
            ledger(savings, "Savings", Normality::Debit),
            ledger(visa, "Visa", Normality::Credit),
            ledger(salary, "Salary", Normality::Credit),
            ledger(rent_l, "Rent", Normality::Debit),
            ledger(equity, "Opening", Normality::Credit),
            bucket(liquid, "Liquid"),
            Op::AddToBucket { bucket: liquid, ledger: cash },
            Op::AddToBucket { bucket: liquid, ledger: savings },
            bucket(debt, "Debt"),
            Op::AddToBucket { bucket: debt, ledger: visa },
            // $2,000 in chequing on Jan 1; $300 on the Visa on Feb 10.
            post(simple_legs(cash, equity, major(2_000)), d(2024, 1, 1)),
            post(simple_legs(rent_l, visa, major(300)), d(2024, 2, 10)),
            issuer(pay, "Pay", simple_legs(cash, salary, major(3_000)), 1),
            issuer(rent, "Rent", simple_legs(rent_l, cash, major(1_500)), 1),
            issuer(sweep, "Sweep", simple_legs(savings, cash, major(500)), 15),
            // Every issuer has posted through March 1.
            Op::AdvanceIssuer { uid: pay, through: d(2024, 3, 1) },
            Op::AdvanceIssuer { uid: rent, through: d(2024, 3, 1) },
            Op::AdvanceIssuer { uid: sweep, through: d(2024, 2, 15) },
            Op::CreateCohort { uid: bills, name: "Bills".into(), description: String::new() },
            Op::AddToCohort { cohort: bills, issuer: rent },
        ])
        .unwrap();
        Fx { l, cash, savings, visa, liquid, debt, pay, rent, sweep, bills }
    }

    /// "Net worth": everything liquid, minus what is owed.
    fn net_worth(fx: &Fx) -> ViewSpec {
        ViewSpec {
            buckets: vec![Term::plus(fx.liquid), Term::plus(fx.debt)],
            lookback: Span::Months(3),
            horizon: Span::Months(3),
            ..ViewSpec::default()
        }
    }

    const TODAY: (i32, u32, u32) = (2024, 3, 10);

    fn today() -> Date {
        d(TODAY.0, TODAY.1, TODAY.2)
    }

    #[test]
    fn projects_every_active_issuer_forward() {
        let fx = fixture();
        let r = evaluate(&fx.l, &net_worth(&fx), today());
        assert!(r.directed);
        let total = &r.series[0];
        assert_eq!(total.kind, SeriesKind::Scope);
        // By normality: $2,000 liquid, minus $300 on the Visa.
        assert_eq!(total.now, major(1_700));
        // Apr 1, May 1, Jun 1 each net +$1,500; the sweeps move nothing.
        assert_eq!(r.end, d(2024, 6, 10));
        assert_eq!(total.at_end, major(1_700 + 3 * 1_500));
        assert_eq!(total.value_at(d(2024, 4, 1)), major(3_200));
        assert_eq!(r.simulated_issuers, 3);
        assert_eq!(r.overdue_occurrences, 0);
    }

    #[test]
    fn one_line_per_bucket_when_there_are_several() {
        let fx = fixture();
        let r = evaluate(&fx.l, &net_worth(&fx), today());
        let labels: Vec<&str> = r.series.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Total", "Liquid", "Debt"]);
        // Each bucket on its own reads by normality: the Visa owes $300.
        assert_eq!(r.series[2].now, major(-300));
        // A sweep moves money from chequing to savings, so Liquid ignores it.
        let liquid = &r.series[1];
        assert_eq!(liquid.value_at(d(2024, 3, 15)), major(2_000));

        // Savings on its own line does see it: Mar 15, Apr 15, May 15.
        let spec = ViewSpec { ledgers: vec![fx.savings], ..net_worth(&fx) };
        let r = evaluate(&fx.l, &spec, today());
        let savings = r.series.iter().find(|s| s.label == "Savings").unwrap();
        assert_eq!(savings.now, Money::ZERO);
        assert_eq!(savings.value_at(d(2024, 3, 15)), major(500));
        assert_eq!(savings.at_end, major(1_500));
    }

    #[test]
    fn a_transfer_inside_the_scope_is_neither_spent_nor_received() {
        let fx = fixture();
        let r = evaluate(&fx.l, &net_worth(&fx), today());
        // With no issuers picked, the flow table lists what moves the scope:
        // pay and rent, not the sweep.
        let names: Vec<&str> = r.flows.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Pay", "Rent"]);
        let t = r.flow_totals(Period::Month);
        assert_eq!(t.received, major(3_000));
        assert_eq!(t.spent, major(1_500));
        assert_eq!(t.net(), major(1_500));
    }

    #[test]
    fn restricting_the_simulation_answers_what_if() {
        let fx = fixture();
        let spec =
            ViewSpec { cohorts: vec![fx.bills], only_selected_issuers: true, ..net_worth(&fx) };
        let r = evaluate(&fx.l, &spec, today());
        assert_eq!(r.simulated_issuers, 1);
        assert_eq!(r.flows.len(), 1);
        assert_eq!(r.flows[0].via, vec!["Bills".to_string()]);
        assert_eq!(r.flows[0].effect, Some(major(-1_500)));
        // Rent alone, three times: 1700 - 4500.
        let total = &r.series[0];
        assert_eq!(total.at_end, major(1_700 - 4_500));
        assert_eq!(total.lowest_ahead, (d(2024, 6, 1), major(-2_800)));
    }

    #[test]
    fn paused_issuers_are_not_simulated() {
        let mut fx = fixture();
        fx.l.apply(&Op::SetIssuerPaused { uid: fx.pay, paused: true }).unwrap();
        let r = evaluate(&fx.l, &net_worth(&fx), today());
        assert_eq!(r.simulated_issuers, 2);
        assert_eq!(r.series[0].at_end, major(1_700 - 3 * 1_500));
    }

    #[test]
    fn overdue_occurrences_land_today_not_in_the_past() {
        let mut fx = fixture();
        // Pretend the issuers were last run in January: Feb 1 and Mar 1 are owed.
        let mut l = Budget::new();
        for op in replay_ops_without_advances(&fx) {
            l.apply(&op).unwrap();
        }
        fx.l = l;
        let r = evaluate(&fx.l, &net_worth(&fx), today());
        let total = &r.series[0];
        // Yesterday's balance is untouched by what has not been posted...
        assert_eq!(total.value_at(d(2024, 3, 9)), major(1_700));
        // ...and today carries every overdue occurrence at once:
        // pay and rent for Jan, Feb and Mar, and the Jan and Feb sweeps.
        assert_eq!(r.overdue_occurrences, 3 + 3 + 2);
        assert_eq!(total.now, major(1_700 + 3 * 1_500));
    }

    /// The fixture's ops, minus the advances, so every issuer is behind.
    fn replay_ops_without_advances(fx: &Fx) -> Vec<Op> {
        let mut ops = Vec::new();
        let l = &fx.l;
        for ix in l.ledgers.indices() {
            let x = l.ledgers.get(ix);
            ops.push(Op::CreateLedger {
                uid: x.uid,
                name: x.name,
                description: x.description,
                normality: x.normality,
                opened: x.opened,
            });
        }
        for b in l.buckets.live() {
            let x = l.buckets.get(b, &l.ledgers);
            ops.push(Op::CreateBucket { uid: x.uid, name: x.name, description: x.description });
            ops.extend(x.members.iter().map(|m| Op::AddToBucket { bucket: x.uid, ledger: *m }));
        }
        for t in l.transactions.indices() {
            let x = l.transaction(t);
            ops.push(Op::PostTransaction {
                uid: x.uid,
                name: x.name,
                description: x.description,
                date: x.date,
                legs: x.legs,
                parent: x.parent,
            });
        }
        for s in l.issuers.indices() {
            let x = l.issuers.get(s);
            ops.push(Op::CreateIssuer {
                uid: x.uid,
                name: x.name,
                description: x.description,
                legs: x.legs,
                schedule: x.schedule,
                start: x.start,
            });
        }
        ops
    }

    #[test]
    fn the_timeline_separates_what_happened_from_what_is_expected() {
        let fx = fixture();
        let r = evaluate(&fx.l, &net_worth(&fx), today());
        // Dec, Jan, ..., Jun.
        assert_eq!(r.timeline.first().unwrap().start, d(2023, 12, 1));
        assert_eq!(r.timeline.last().unwrap().start, d(2024, 6, 1));
        let feb = r.timeline.iter().find(|p| p.start == d(2024, 2, 1)).unwrap();
        // The $300 Visa charge is the only posting in February.
        assert_eq!(feb.actual.spent, major(300));
        assert_eq!(feb.actual.received, Money::ZERO);
        assert_eq!(feb.projected, Flow::default());
        let apr = r.timeline.iter().find(|p| p.start == d(2024, 4, 1)).unwrap();
        assert_eq!(apr.projected.received, major(3_000));
        assert_eq!(apr.projected.spent, major(1_500));
        // The sweep is volume nowhere: it never reaches the table.
        assert_eq!(apr.projected.entries, 2);
    }

    #[test]
    fn ledgers_alone_make_the_scope_as_displayed() {
        let fx = fixture();
        let spec = ViewSpec {
            ledgers: vec![fx.cash, fx.visa],
            lookback: Span::Months(1),
            horizon: Span::Months(1),
            ..ViewSpec::default()
        };
        let r = evaluate(&fx.l, &spec, today());
        let labels: Vec<&str> = r.series.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Total", "Chequing", "Visa"]);
        // As displayed: chequing 2000 and the Visa owing 300 add to 2300.
        assert_eq!(r.series[0].now, major(2_300));
        assert_eq!(r.series[2].now, major(300));
    }

    #[test]
    fn a_view_with_no_scope_counts_volume_only() {
        let fx = fixture();
        let spec = ViewSpec { issuers: vec![fx.rent, fx.sweep], ..ViewSpec::default() };
        let r = evaluate(&fx.l, &spec, today());
        assert!(!r.directed);
        assert!(r.series.is_empty());
        assert!(r.flows.iter().all(|f| f.effect.is_none()));
        let t = r.flow_totals(Period::Month);
        assert_eq!(t.volume, major(2_000));
        assert_eq!(t.received, Money::ZERO);
    }

    #[test]
    fn a_deleted_bucket_is_reported_not_fatal() {
        let mut fx = fixture();
        let spec = net_worth(&fx);
        fx.l.apply(&Op::DeleteBucket { uid: fx.debt }).unwrap();
        let r = evaluate(&fx.l, &spec, today());
        assert_eq!(r.missing_buckets, vec![fx.debt]);
        assert_eq!(r.series[0].now, major(2_000));
    }

    #[test]
    fn a_ledger_on_both_sides_is_cancelled_and_reported() {
        let fx = fixture();
        let spec = ViewSpec {
            buckets: vec![Term::plus(fx.liquid), Term::minus(fx.liquid)],
            ..ViewSpec::default()
        };
        let r = evaluate(&fx.l, &spec, today());
        assert_eq!(r.cancelled.len(), 2);
        assert!(!r.directed);
    }

    #[test]
    fn saving_a_view_checks_what_it_names() {
        let mut fx = fixture();
        let uid = crate::id::ViewUid::new();
        let bad = ViewSpec { ledgers: vec![LedgerUid::new()], ..ViewSpec::default() };
        let err = fx
            .l
            .apply(&Op::CreateView { uid, name: "x".into(), description: String::new(), spec: bad })
            .unwrap_err();
        assert!(matches!(err, crate::Error::NoSuchEntity { kind: "ledger", .. }));
        let far = ViewSpec { horizon: Span::Years(80), ..ViewSpec::default() };
        assert!(fx
            .l
            .apply(&Op::CreateView { uid, name: "x".into(), description: String::new(), spec: far })
            .is_err());
        assert!(fx.l.views.is_empty(), "a rejected view writes nothing");

        let spec = net_worth(&fx);
        fx.l.apply(&Op::CreateView {
            uid,
            name: "Net worth".into(),
            description: String::new(),
            spec,
        })
        .unwrap();
        assert_eq!(fx.l.view_by_name("net worth").map(|v| fx.l.views.uid[v.get()]), Some(uid));
    }
}
