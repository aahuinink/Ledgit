# Roadmap

## For the next session

**Aaron has run the GUI once and reported back; everything he reported is
fixed below.** The job next session is the same as last: take the second run's
findings at face value and fix them. No test here can tell whether a column is
the right width.

Context you would otherwise have to rediscover:

- **You cannot run the GUI.** There is no display in this WSL environment.
  Verify GUI changes with `cargo test -p ledgit-gui`, which runs real headless
  egui passes over every screen (`crates/ledgit-gui/src/smoke.rs`). Those tests
  can also click, type and read back where text was painted (`Window`,
  `painted_text`) - use that to pin a layout complaint down before fixing it.
  Exercise the same core paths through `cargo run -p ledgit-cli`. Aaron builds
  and runs natively on Windows.
- **egui is pinned to 0.33 on purpose**, and `egui_extras` with it. 0.36 needs
  rustc 1.95 and the toolchain here is 1.90. Do not bump them. In 0.33 the app
  implements `App::update(ctx, frame)` and panels take `ctx`, not `&mut Ui`.
- **Every data table goes through `table.rs`.** Columns fit their content up
  to a cap, then clip with the full text on hover; figures go in `figures(..)`
  columns via `num()`. Do not bring back `egui::Grid` for data - it is what put
  the figures at the window edge. Grid is fine for forms.
- **`cargo check --target x86_64-pc-windows-gnu` fails**, and that is not a code
  problem: bundled SQLite needs a mingw C compiler that is not installed. The
  native MSVC build on Windows is fine.
- **The invariant that governs every change:** a change to the budget is an
  `Op` - plain, serialisable data. If a fix tempts you to add a closure, a
  delete operation for a ledger/transaction/issuer, or a derived table in
  SQLite, it is the wrong fix. `docs/ARCHITECTURE.md` says why.
- **Keep it green.** 189 tests and `cargo clippy --all-targets` clean, and
  `cargo fmt` applied. A GUI fix that needs a new behaviour usually wants a
  case added to `smoke.rs`.

If he reports nothing and wants to move on, the next work is the GUI gaps
below: editing, charts, keyboard.

## Checklist: the second run of the GUI

Everything from the first run, fixed, and what to look at to confirm it. The
first run's feedback is summarised in italics.

1. **Figures at the window edge; stripes stopping short** (*Balance, subtree
   total and Entries far to the right; cell shading not reaching the last
   column*). Every data table moved from `egui::Grid` to one helper,
   `table.rs`. Columns now size to their content, so the last figure column
   ends where its widest figure does, right-aligned under its header, and the
   stripes run under every column. Columns can be dragged wider (the thin
   lines between them are the handles).
   *Look at:* Ledgers (tree and list), a ledger's register, the Commit
   screen's "Ledgers affected", Buckets.
2. **A long bucket ran off the screen.** The member list now scrolls inside
   the window with its header kept, and "Add" / "Delete bucket" moved *above*
   it so they cannot be pushed out of reach.
3. **Switch buttons greyed out on History.** They were disabled because the
   household fixture has staged changes. Switching now asks what to do with
   them: **shelve** them on the branch you are leaving (they come back when
   you switch back, and the branch shows "N shelved" meanwhile), or **bring**
   them along. "Branch here" brings them. CLI: `ledgit checkout NAME --shelve`
   or `--bring`. See ARCHITECTURE.md, "Shelves".
4. **History cramped; the window would not go smaller.** The three panes are
   now panels: drag the lines between them. Their starting widths follow the
   window, the commit list truncates to one line per commit (full text on
   hover), and the detail scrolls. The minimum window size dropped from
   900x560 to 720x480. Buckets, Cohorts and Views use the same draggable,
   scrolling list on the left.
5. **The Views editor** (*too much space under Buckets; ledger chips bleeding
   into the rows below*; then, second pass: *the ledger section cuts off
   strangely*). The editor is no longer a Grid. Ledgers are no longer a chip
   for every ledger in the budget: the view's own ledgers show as chips
   (click one to take it off, "clear" for all), and "add a ledger..." is the
   searchable tree picker, which also adds a whole subtree. Chip sections
   grow to four whole lines, then scroll - never cut through a line.
6. **Busy calendar months.** Cohorts has Month / List beside the month
   arrows. List is one line per payment, the date shown once per day, with
   status. A month with a day over three payments says so under the grid,
   with a button to switch.
7. **A big legend covered the chart.** The legend is now beside the chart,
   not on it, and scrolls when longer than the chart is tall. Click a line
   in it to hide it (and again to bring it back); "show all" resets. The
   Views tables got the same clipping as everywhere else.
8. **The picker closed when its search box was clicked.** An egui combo box
   closes on any click, its own contents included; it now closes only on a
   click outside or a pick, and the cursor starts in the search box. A test
   clicks the box, types "tux" and picks the match.
9. **Ledgers tree far-right column** - same fix as 1.
10. **"Fresh through ..."** in the top bar, and in `ledgit status`.
13. **No squashed columns** (*the Views "Balances" table was crushed*). Every
    table column is at least 16 characters wide (`table::MIN_CHARS`), except
    pins, row numbers and buttons; a table wider than the window scrolls
    sideways instead.
14. **View descriptions.** Under a view's name: "add a description" / "edit
    description" writes it in place and stages it. The list of views shows
    it on hover.

New:

11. **The logo is the File menu.** Click the mark at the top left: New
    budget..., Open budget..., Open recent, Close budget (back to the
    welcome screen). The budget on screen is swapped in place; its staged
    work is already in its file, so nothing is lost. The window title now
    names the open budget.
12. **One Ledgit at a time.** Launching Ledgit while it is running - from
    the Start menu, or by double-clicking a `.ledgit` file - hands the file
    to the running window, which opens it and comes to the front (or
    flashes on the taskbar, if Windows will not let it take focus), and the
    new launch exits. *Look at:* double-click a second `.ledgit` in Explorer
    with Ledgit open. How it works, and why a socket: ARCHITECTURE.md,
    "One Ledgit at a time".

Still open from the first run:

- **The exe icon** (first-run item 12): not reported on. After
  `cargo build --release`, check there is no `ledgit-gui.exe will have no
  icon` warning, that `target\release\ledgit-gui.exe` shows the icon in
  Explorer at small and large sizes, and after installing, that the
  Start-menu and desktop shortcuts, Settings > Apps and a `.ledgit` file all
  show it. Explorer caches icons per path: if an old one sticks, run
  `ie4uinit.exe -show`, or delete `%LOCALAPPDATA%\IconCache.db` and the
  `iconcache_*.db` files in `%LOCALAPPDATA%\Microsoft\Windows\Explorer`
  and sign out and in.
- **The taskbar showing the light icon while running** - Aaron: probably an
  icon cache; good enough, not pursued.

Run it against a throwaway budget rather than a real one. Three are generated
for you, dated relative to the day you run the generator, so re-run it rather
than keeping old copies (it overwrites them):

```sh
cargo run -p ledgit-cli --example fixtures          # writes .\fixtures\*.ledgit
cargo run -p ledgit-gui -- fixtures\household.ledgit
```

| File | What is in it | Checklist items |
|---|---|---|
| `empty.ledgit` | Nothing. | Every screen's empty state |
| `household.ledgit` | 15 months of an ordinary budget: a 5-way split paycheque, mortgage, car loan, credit card, a 4-level ledger tree, 6 buckets (subtree and hand-picked), 12 issuers (one paused, one one-off 60 days out), 4 cohorts, 4 views (incl. a weekly what-if). Branches `what-if-new-car` and `emergency-fund-plan` diverge from `main` and rebase cleanly. A mistaken commit and its revert. Issuers are ~6 weeks behind (Run issuers stages a batch); staged edits waiting (so History's switch asks to shelve). | 1-12, History, Commit report, issuer runs since last commit |
| `stress.ledgit` | Things built to break layouts: a $987,654,321.09 balance, a negative one, an 80-character ledger name, an 8-level tree, a non-ASCII name, 120 expense ledgers, a 13-leg split, 17 issuers with 7 due on the 1st and 7 on the 15th plus a daily one, a 10-year daily view, ~110 commits, 10 long branch names, a commit message that wraps. | 1, 2, 4, 5, 6, 7 (at their worst), 8, 9 |

Pins are an app preference, not part of the file, so pin a few ledgers
yourself on the Dashboard.

## Aaron's first-run list

Found while working through the checklist above. All built; none of it has
been seen on a real display yet, so the notes say what to look at.

### First things first - done

Test budgets: `cargo run -p ledgit-cli --example fixtures` (see the table
above). `household.ledgit` now also carries everything below: interest and
sweep issuers, three variables, targets on every debt and the emergency fund,
alerts on chequing and the card (the card's fires), a lump-sum commit on the
car loan to compare against, a mileage entry priced from a variable, and one
broken staged entry.

### GUI fixes/features - done

1. **Back and undo.** A back arrow at the far left of the top bar (also
   Alt+Left and the mouse's back button), and Undo/Redo at the right (Ctrl+Z,
   Ctrl+Y / Ctrl+Shift+Z - not while typing in a box). Undo covers every
   change to the staging area: staging, dropping, editing, issuer runs,
   discarding. It stops at a commit; undo a commit by reverting it.
   *`app.rs`, `Session::track`, `undo`, `go_back`.*
2. **Sub-ledgers from the Create Ledger form.** An "Under" picker browses the
   tree (any level, or the top), the name is the last part, and the form
   shows "will be created as ...". Normality defaults to that of the ledgers
   already there. The Ledgers tree also has a "+" on every row that opens the
   form under it. *`forms.rs` `LedgerForm`; `picker.rs` `.paths(true)`.*
3. **Calendar date picker** on every date field: the entry forms, the
   Transactions filters, "Run everything due through", "Simulate until".
   Typing still works; the calendar writes into the box. Forms default to
   today (UTC, as settled). *`datepick.rs`.*
4. **The sides dropdown behind the form.** Could not reproduce headlessly -
   in a test the list opens on top - so the fix pins it: every popup opened
   inside the form is made a sublayer of the form, which egui always draws
   directly above it. **Check this one first.** *`picker.rs`, `keep_above`.*

   Found on the way: some symbols the app drew are not in egui's fonts and
   would have shown as empty boxes - the tree's fold arrows, the remove
   button, the "from -> to" arrow, the current-branch dot. All replaced, and
   a test now checks every symbol the GUI source uses.

### Functional fixes/features - done

1. **Dropping a ledger that staged entries use.** Allowed now; the entries
   stay, flagged in red on the Commit screen with the reason, left out of
   the totals, and Commit is disabled until each is edited or dropped. CLI:
   `ledgit status` marks them, `ledgit drop <n>` drops one.
2. **Targets and alerts.** On a ledger's page, "Target and alerts": a target
   balance and any number of below/above alerts with a message. Firing
   alerts show at the top of the Dashboard and as a red count in the top bar;
   the Commit screen lists alerts the staged changes would set off. Views
   draw each target as a dotted line, add a "target" column (reached on /
   not by the end) and list "Alerts ahead". Buckets have an "Aggregate
   targets" toggle. The ledger page's "Show when the target is reached"
   toggle says when it was or will be reached, with a look-ahead in years.
   CLI: `ledgit ledger goals`.
3. **Variables.** A Variables screen (numbers and text). Any amount field
   takes a formula - `200 * Car_Km_Rate`, `5%`, `(1850-400)/2` - and shows
   what it comes to as you type; names and descriptions take `{Home}`.
   Worked out when the entry is made, so changing a rate later changes no
   posted entry. CLI: `ledgit var`, and formulas in `ledgit post`.
4. **Edit a staged change.** "edit" beside each staged entry, ledger, issuer,
   bucket, cohort or view reopens its form filled in; saving replaces it in
   place under the same id, so everything referring to it still does.
5. **Interest and percentage issuers.** The issuer form's Amount row: Fixed,
   "% of a balance", or "Interest (APR)", with the ledger it reads and the
   rate (or a variable). Priced each time it fires from the balance at the
   start of that day, after every earlier payment - so interest compounds and
   a payment on the 15th lowers the next month's interest. Views simulate it
   the same way. CLI: `ledgit issuer add ... --apr 6.45` or `--share 5`.
6. **Compare a view between commits.** Views has "Compare with": any commit
   on this branch, or the budget without its staged changes. The earlier
   lines draw dotted under the current ones, and a table shows each line
   then and now, including how much sooner or later it reaches its target.
   CLI: `ledgit view show NAME --compare REV`.

### Worth knowing

- An issuer with a rule keeps firing after its loan is paid off; with a
  balance at or below zero it posts nothing, but a *fixed* payment issuer
  next to it will keep paying into the negative. Pause the payment when the
  target date arrives.
- Interest is simple interest per period on the balance at the start of the
  occurrence's day (actual/365), compounding period to period. It is not
  daily-average-balance interest, which is what some banks charge.
- A bucket or view total gets a target line only when every ledger in it has
  a target; the Buckets screen's aggregate uses just the members that do.

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
- **Tests**: 189, covering the money and date edge cases, budget invariants,
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
