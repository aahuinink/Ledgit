# Roadmap

## For the next session

**Aaron is running the GUI by hand for the first time.** Everything below the
checklist is built, tested and committed; the job this session is to fix what
he reports, not to start new work. Take his findings at face value - no test
here can tell whether a column is the right width.

Context you would otherwise have to rediscover:

- **You cannot run the GUI.** There is no display in this WSL environment.
  Verify GUI changes with `cargo test -p ledgit-gui`, which runs a real headless
  egui pass over every screen (`crates/ledgit-gui/src/smoke.rs`), and exercise the
  same core paths through `cargo run -p ledgit-cli`. Aaron builds and runs natively
  on Windows.
- **egui is pinned to 0.33 on purpose.** 0.36 needs rustc 1.95 and the
  toolchain here is 1.90. Do not bump it. In 0.33 the app implements
  `App::update(ctx, frame)` and panels take `ctx`, not `&mut Ui`.
- **`cargo check --target x86_64-pc-windows-gnu` fails**, and that is not a code
  problem: bundled SQLite needs a mingw C compiler that is not installed. The
  native MSVC build on Windows is fine.
- **The invariant that governs every change:** a change to the budget is an
  `Op` - plain, serialisable data. If a fix tempts you to add a closure, a
  delete operation for a ledger/transaction/issuer, or a derived table in
  SQLite, it is the wrong fix. `docs/ARCHITECTURE.md` says why.
- **Keep it green.** 134 tests and `cargo clippy --all-targets` clean. A GUI fix
  that needs a new behaviour usually wants a case added to `smoke.rs`.

If he reports nothing and wants to move on, the next work is the GUI gaps
below: editing, charts, keyboard.

## Checklist: the first real run of the GUI

Ranked by how likely I think they are to be wrong. Everything here compiled and
drew without panicking; what no test can check is whether it *looks* right.

1. **Right-aligned number columns.** Every table puts its amounts in a
   right-to-left layout inside a `Grid` cell. That is the construct most likely
   to size itself strangely - look for a balance column that is too wide, or
   numbers that drift away from their header.
   *`views/mod.rs`, `num()` - every table calls it.*
2. **The transaction and issuer forms.** They carry the leg editor - toggle,
   ledger picker, amount, remove button on one row - and the modal is widened
   to 640px for them. If a row wraps or the remove button falls off the edge,
   that number is the thing to change.
   *`forms.rs`, `FormKind::width` and `LegEditor::show`.*
3. **The "New" menu closing.** It calls `ui.close()`, whose exact semantics in
   egui 0.33 I could not verify without running it. If the menu stays open
   after you pick something, that is the line.
   *`app.rs`, the `New` menu in `top_bar`.*
4. **The History screen's three columns.** Branches (300px), the commit list
   (360px), then detail. Below about 1100px wide the detail pane will get
   cramped before anything else does.
   *`views/history.rs`.*
5. **Split rendering.** Cells naming several ledgers are clipped with the full
   text on hover. Check the hover actually shows, and that clipping at 22-26
   characters is not cutting off something you need to read.
   *`fmt.rs`, `clip()`; callers in `views/transactions.rs`, `views/ledgers.rs`,
   `views/search.rs`.*

6. **The cohort calendar.** Day cells are fixed at 118x72 with three entries
   and a "+N more"; a busy month may want taller cells or a list instead.
   *`views/cohorts.rs`, `calendar()` and `day_cell()`.*
7. **The Views chart.** egui_plot 0.34, step lines, solid to today and dashed
   after, month grid lines. Check the dashed tail joins the solid line at the
   today marker, and that the legend (top left) does not sit on the data.
   *`views/saved.rs`, `chart()`.*
8. **The ledger tree picker.** A searchable tree inside a combo box, used for
   entry sides and bucket members. Check the popup's height (360px) and that
   typing in its search box does not close it.
   *`picker.rs`.*
9. **The Ledgers tree.** Indented rows with fold arrows, a "move" button on
   every level with children, subtotals right-aligned. *`views/ledgers.rs`,
   `tree()`.*
10. **The top-bar freshness label** ("through ... · issuers ...") may crowd
   the "New" menu on a narrow window. *`app.rs`, `freshness()`.*

Run it against a throwaway budget rather than a real one:

```sh
cargo run -p ledgit-gui -- /tmp/scratch.ledgit
```


## Done

- **Core library** (`ledgit-core`): money, dates, ledgers, transactions, issuers,
  buckets, the operation log, the commit DAG, staging, the pre-commit report,
  and the query layer. No I/O, 3 dependencies.
- **Storage** (`ledgit-sqlite`): durable single-file budgets, content verification
  on every read, `ledgit verify` for the whole file.
- **Version control**: commit, log, show, branch, checkout, revert (as
  reversing entries), rebase, merge-base, detached HEAD.
- **Issuers**: day-granular recurrence, monthly-on-a-day without end-of-month
  drift, pause/resume, no double-posting on replay.
- **CLI** (`ledgit`): enough to run a real budget from a terminal.
- **GUI** (`ledgit-gui`): egui/eframe desktop app - dashboard, ledgers, register,
  transactions with filters, issuers, buckets, the commit screen, and the
  history/branch/revert/rebase screen. Headless smoke tests render every view.
- **Installer**: Inno Setup script producing a per-user Windows installer with
  a `.ledgit` file association and an optional `PATH` entry for the CLI.
- **Split entries**: transactions and issuers carry N legs summing to zero,
  stored in one flat posting arena. Paycheques are a single entry.
- **Cohorts**: groups of issuers as ops, like buckets. Rates per day, week,
  month and year in exact arithmetic; a due-date calendar marking posted,
  overdue, upcoming and paused. CLI `ledgit cohort ...`, GUI Cohorts screen.
- **Saved views**: versioned specs over buckets, ledgers, issuers, cohorts and
  transaction filters, with a lookback and a horizon. Balances across time,
  flows per period, and a forward simulation of the issuers (all running ones,
  or only the view's own for a what-if). CLI `ledgit view ...`, GUI Views
  screen with a live chart; charts export as SVG or PNG (`ledgit-plot`) and
  series as CSV.
- **"Up to date through"** in the top bar and in `ledgit status`: the newest
  transaction, and how far the issuers have been run.
- **Ledger tree**: hledger-style paths in ledger names (`Wedding:Tuxedo`), a
  derived tree with subtotals, buckets that include a whole subtree (and pick
  up ledgers created there later), and subtree renames that carry buckets
  along. CLI `ledger tree`, `ledger move`, `bucket include-tree`; GUI tree
  view and a searchable tree picker.
- **Tests**: 134, covering the money and date edge cases, budget invariants,
  recurrence arithmetic, rate conversion, the view simulation, GUI zoom,
  bucket combination, chart rendering, and the version-control behaviours end
  to end.

## Next

### GUI gaps

The app covers every screen you asked for, but these are thin:

1. **Editing.** You can create everything and pause issuers; you cannot yet edit
   a name or description from the GUI, though `EditLedger`, `EditTransaction`,
   `EditIssuer`, `EditBucket`, `EditCohort` and `EditView` all exist in the core.
   (A view's *spec* is editable on the Views screen; its name is not.)
2. **Graphs outside Views.** Saved views now chart balances across time and
   simulate forward. A one-click "chart this ledger/bucket" from the Ledgers
   and Buckets screens would reuse `view::evaluate` with a one-item spec.
   Spending by ledger over a period is still a table-only question.
3. **Keyboard.** No shortcuts, no quick-entry. For a tool you open daily to type
   three transactions, that matters more than it sounds.
4. **Nobody has seen it run.** Every screen is exercised by headless egui
   passes, but there is no display in WSL, so the first look at actual pixels
   is Aaron's. See the checklist near the top of this file.

### Deferred deliberately

- **Snapshot cache** for fast open on large histories. The design is in
  ARCHITECTURE.md; build it when a profile says to, not before.
- **Merge.** Branching and rebase are in; `merge` with a real conflict policy is
  not. For a single-user budget, rebase covers the workflow. The commit format
  already allows multiple parents, so nothing blocks it later.
- **Multi-currency.** One currency, `i64` minor units. Adding currencies means a
  currency on every ledger and a rate table with dates - a real feature, not a
  field.

## Settled

- **Split entries** - built. An entry has N legs summing to zero.
- **Dates are UTC.** No time-zone dependency; entry forms take an explicit date.
- **Reversals match the original date**, so a correction lands in the period it
  belongs to.
- **The staging area is persisted**, in its own table, never in the commit DAG.
  "Nothing is written until I commit" means nothing becomes permanent, shared,
  reportable state - not that an afternoon of data entry dies with the process.
- **`.claude/GUI.md`** - disregarded. The GUI was built to the spec in
  `.claude/CLAUDE.md` and to the screens listed above.

Nothing is waiting on a decision. The next work is the GUI gaps above - editing,
charts, keyboard - in whatever order the first real run says matters.
