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
- **Keep it green.** 169 tests and `cargo clippy --all-targets` clean. A GUI fix
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

_Feedback_

    - The right-most columns are way too far over to the right, I almost didn't notice them. For example, "Balance" and subtree total in a ledger screen and "Entries" in the Ledgers Affected section of the commit screen.

2. **The transaction and issuer forms.** They carry the leg editor - toggle,
   ledger picker, amount, remove button on one row - and the modal is widened
   to 640px for them. If a row wraps or the remove button falls off the edge,
   that number is the thing to change.
   *`forms.rs`, `FormKind::width` and `LegEditor::show`.*

_Feedback_

    - The drop-down menu in the forms display behind the pop-up and are not selectable.
    - In general, all the "x" buttons appear as simple squares without x's.

3. **The "New" menu closing.** It calls `ui.close()`, whose exact semantics in
   egui 0.33 I could not verify without running it. If the menu stays open
   after you pick something, that is the line.
   *`app.rs`, the `New` menu in `top_bar`.*

_Feedback_
    
    - No issues here

4. **The History screen's three columns.** Branches (300px), the commit list
   (360px), then detail. Below about 1100px wide the detail pane will get
   cramped before anything else does.
   *`views/history.rs`.*

_Feedback_

    - Functions as explained. I cannot resize the window to be smaller past a certain point unless i zoom out more with ctrl scroll. The detail pane is the only thing that gets cramped.

5. **Split rendering.** Cells naming several ledgers are clipped with the full
   text on hover. Check the hover actually shows, and that clipping at 22-26
   characters is not cutting off something you need to read.
   *`fmt.rs`, `clip()`; callers in `views/transactions.rs`, `views/ledgers.rs`,
   `views/search.rs`.*

_Feedback_

    - The detail screen for a specific view has way too much whitespace in the "Buckets" section (looks like its a fixed size and doesn't resize based on the number of buckets), and then when there are a lot of ledgers the ledgers bleed into cells below. 
        - Make cells resize to fit their content up to a certain point, then make them scrollable if they get too large.

6. **The cohort calendar.** Day cells are fixed at 118x72 with three entries
   and a "+N more"; a busy month may want taller cells or a list instead.
   *`views/cohorts.rs`, `calendar()` and `day_cell()`.*

_Feedback_
    
    - Yes, give me the option of a list view for busy months.

7. **The Views chart.** egui_plot 0.34, step lines, solid to today and dashed
   after, month grid lines. Check the dashed tail joins the solid line at the
   today marker, and that the legend (top left) does not sit on the data.
   *`views/saved.rs`, `chart()`.*

_Feedback_

    - Looks good

8. **The ledger tree picker.** A searchable tree inside a combo box, used for
   entry sides and bucket members. Check the popup's height (360px) and that
   typing in its search box does not close it.
   *`picker.rs`.*

_Feedback_

    - Looks good

9. **The Ledgers tree.** Indented rows with fold arrows, a "move" button on
   every level with children, subtotals right-aligned. *`views/ledgers.rs`,
   `tree()`.*
10. **The top-bar freshness label** ("through ... · issuers ...") may crowd
   the "New" menu on a narrow window. *`app.rs`, `freshness()`.*

_Feedback_

    - No crowding, but change it to say "Fresh through ... "

11. **The logo and icon.** The welcome screen shows the logo (300px wide), the
   top bar a 26px mark, each picking the `_dark` copy under the dark theme.
   The window icon (title bar, taskbar while running) is *not* themed: it is
   the exe's icon, whichever file `ICON` in `build.rs` names. An earlier
   version followed the theme egui reports, which is Windows' *app* mode, not
   the *Windows* mode the taskbar uses - so light apps on a dark taskbar got
   the light icon. Check the running app's taskbar and title-bar icon is the
   same dark one as the pinned/Start-menu shortcut.
   *`brand.rs`; artwork in `assets/`.*

_Feedback_

    - The app, app icon, and window icon use the correct dark mode logo, but the task bar still shows the light mode version, however this is probably a cache thing that I don't care enough to fix. Good enough.

12. **The exe icon.** `crates/ledgit-gui/build.rs` renders `assets/Icon_dark.svg`
   into a 7-size `.ico` and embeds it with `winresource`, which needs `rc.exe`
   from the Windows SDK (it comes with the VS Build Tools that MSVC Rust
   uses). It has never run on Windows. Check, after `cargo build --release`:
   - no `ledgit-gui.exe will have no icon` warning in the build output (if
     there is one, it names why - usually `rc.exe` not found);
   - `target\release\ledgit-gui.exe` shows the icon in Explorer, at small and
     large icon sizes (each size is rendered separately, so 16px should be
     crisp, not a blurred 256px);
   - after installing: the Start-menu and desktop shortcuts, the entry in
     Settings > Apps, and a `.ledgit` file in Explorer all show it - all four
     read the exe's icon;
   - that the hollow rings still read at 16-32px (taskbar, title bar, list
     views): at those sizes each ring is about one pixel wide, so the mark is
     lighter than it was with filled circles;
   - that it reads in Explorer's light views too: Windows uses one icon
     everywhere, and it is now the dark copy (light strokes). `ICON` in
     `build.rs` is the one place to switch back; the running app follows.
   Explorer caches icons per file path, so reinstalling over an old version
   can keep showing the old icon. If it sticks, run `ie4uinit.exe -show`, or
   uninstall, then delete `%LOCALAPPDATA%\IconCache.db` and the
   `iconcache_*.db` files in `%LOCALAPPDATA%\Microsoft\Windows\Explorer`
   and sign out and back in.


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
| `household.ledgit` | 15 months of an ordinary budget: a 5-way split paycheque, mortgage, car loan, credit card, a 4-level ledger tree, 6 buckets (subtree and hand-picked), 12 issuers (one paused, one one-off 60 days out), 4 cohorts, 4 views (incl. a weekly what-if). Branches `what-if-new-car` and `emergency-fund-plan` diverge from `main` and rebase cleanly. A mistaken commit and its revert. Issuers are ~6 weeks behind (Run issuers stages a batch); 4 edits already staged. | 1-11, History, Commit report, issuer runs since last commit |
| `stress.ledgit` | Things built to break layouts: a $987,654,321.09 balance, a negative one, an 80-character ledger name, an 8-level tree, a non-ASCII name, 120 expense ledgers, a 13-leg split, 17 issuers with 7 due on the 1st and 7 on the 15th plus a daily one, a 10-year daily view, ~110 commits, 10 long branch names, a commit message that wraps. | 1, 4, 5, 6, 7 (at its worst), 8, 9, 10 |

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
- **Tests**: 169, covering the money and date edge cases, budget invariants,
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
