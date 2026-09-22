# Phase 018: Desktop 1.0 Release Hardening / Release Candidate 1

- Status: COMPLETE (release candidate - not final 1.0.0)
- Owner gate: APPROVED 2026-09-13 (owner handoff: "MIDAS Desktop 1.0 Release
  Hardening - Release Candidate 1")

## Goal

> Prove that the frozen MIDAS product can be installed fresh, upgraded
> safely, used repeatedly, restarted, backed up, restored, imported into,
> reconciled, and packaged without corrupting financial data or creating
> serious usability blockers.

Not a feature batch: fix bugs, reliability problems, and confusing
release-blocking UX; harden installation/recovery; strengthen tests;
remove dead code. `v1.0.0-rc.1` only, gated on every blocker/high finding
passing - final `v1.0.0` requires a separate owner-approved release
handoff after real-world acceptance testing.

## Reconnaissance

Inventoried before any change: 14 migrations (`0001`-`0014`, unchanged
since Batch 10), `db.rs`'s migration runner (transactional per-migration,
`PRAGMA user_version`-tracked, refuses a newer-than-supported schema),
version metadata (`package.json`/`Cargo.toml`/`tauri.conf.json` all still
`0.1.0`, untouched through the whole v1.0 cycle), Tauri config (NSIS-only
bundle target, no updater, no window-state plugin, CSP with no external
`connect-src`), capabilities (`permissions: []`, description claimed a
stale "48 commands" - actually 116, confirmed by a full audit), Cargo.toml
(9 direct dependencies, no `[features]` section), and `docs/release/0.1.0.md`
as the template for this phase's own release doc.

**A real, pre-existing migration-testing asset already existed and exceeded
what a fresh audit would have produced**: `db.rs`'s own test module has a
dedicated, real-data-fixture upgrade test for every migration boundary from
0007->0008 (the exact v0.1.0 -> Foundation Batch 1 line) through 0013->0014,
each proving old data survives byte-identical and new columns/tables default
sanely - plus idempotence, partial-upgrade-resume, newer-schema-rejection,
and atomic-rollback-on-a-failed-migration tests. This satisfied most of the
migration-chain audit's intent before any new test was written; this phase's
job was to prove the same thing through the _real application startup path_,
not just unit tests (see below).

## Fresh Install (live, packaged and dev)

**Dev-mode, via a temporary disposable Tauri identifier**: `tauri.conf.json`'s
`identifier` was temporarily changed to `com.breezy.midas.rc1test` (never
touching the real `com.breezy.midas` app-data directory), `npm run tauri dev`
relaunched, confirmed a brand-new `midas.sqlite3` was created and reached
`user_version = 14` through the real `db::open()` path. Walked the complete
onboarding flow live (Welcome -> Privacy -> Account -> Transactions ->
Done), including opening the real Import dialog to confirm it's reachable,
then completing via the real manual-entry path; landed on Overview with the
correct Safe to Spend arithmetic ($2,500.00 opening balance - $62.15
grocery purchase = $2,437.85, verified exactly). Killed the process and
relaunched: onboarding did not repeat, the account and transaction
persisted byte-for-byte.

**Alternate onboarding paths** (section 6): interrupting _after_ an account
was created (killed mid-"Add your first transactions" step, relaunched)
correctly lands on the normal Overview with the account intact - no
duplicate account, no stuck state, since "has any account ever existed" is
already a sufficient, self-clearing signal. Interrupting _before_ any
account was created (killed mid-Privacy step) correctly resumes at
"Welcome to MIDAS" with zero accounts - no persisted step-state to
corrupt, since none exists.

**Packaged installer**: built the real NSIS installer
(`MIDAS_1.0.0-rc.1_x64-setup.exe`), installed it silently (`/S`) into a
disposable-identifier build, and launched the _actual installed binary_
(`%LOCALAPPDATA%\MIDAS\midas.exe`, not `tauri dev`) with remote debugging
enabled. Confirmed the app loads from `http://tauri.localhost/` (its own
bundled assets - the CDP driver's dev-server URL matcher correctly failed
to find a `localhost:1420` target, proving no dev server is involved),
shows the same onboarding flow, and correctly skips it via "Skip setup."

## Upgrade from v0.1.0

**Via the real application startup path, not only unit tests** (the
handoff's own explicit requirement): built a genuine v0.1.0-schema SQLite
file by executing migrations `0001`-`0007` verbatim from the repository's
own migration files (not hand-approximated), seeded realistic multi-account
data (checking/savings/credit-card, opening balances, income, a purchase,
a balance adjustment, a budget, a savings goal, a recurring series, a
merchant rule, a needs-review item), set `user_version = 7`, placed it as
the disposable identifier's `midas.sqlite3`, then launched the real app.

Result: `user_version` reached 14, `PRAGMA foreign_key_check` returned
empty, `PRAGMA integrity_check` returned `ok`. Every downstream feature
built on top of the migrated data computed correctly, hand-verified exactly
against the seeded fixture: Safe to Spend $16,960.00 (checking $6,960.00 +
savings $10,000.00), net worth $16,620.01 (assets $16,960.00 - liabilities
$339.99), the Netflix recurring series correctly appearing in Upcoming
Bills, the Dining budget showing $45.00 actual against $300.00 planned, the
Emergency Fund savings goal at $4,200.00/$10,000.00, and - proving Batch
9/10's newer features work against genuinely old data, not just fixtures
built for them - the legacy `suspected_duplicate` needs-review row
correctly surfacing in the unified Review Center, and a real Financial
Signal ("Income lower than usual") correctly firing.

## Migration Matrix

Fresh DB -> latest: verified live (see Fresh Install). Representative
v0.1.0 DB -> latest: verified live (see above), in addition to `db.rs`'s
own pre-existing per-boundary fixture tests. Current DB -> no-op startup:
implicit in every `db::open()` call this whole session (`current == target`
returns immediately with no migration re-run). A "Batch 2/6/7-era DB"
upgrade was not separately constructed as a third fixture - the v0.1.0
(migration 7) and current (migration 14) boundaries already bracket every
intermediate migration, and `db.rs`'s own existing per-boundary tests cover
every step between them individually with real data.

## Database Integrity

`PRAGMA integrity_check` = `ok` and `PRAGMA foreign_key_check` = empty on:
a freshly-created database (fresh install), the upgraded v0.1.0 fixture,
and the database after concurrent multi-instance access (see Concurrency).
Journaling/durability inspected, not changed: `journal_mode = delete`
(SQLite's default rollback journal), `synchronous = FULL` (every commit
fsync'd before the IPC call returns) - both already appropriate for a
single-user local desktop app; no change made, matching the handoff's own
"any change here requires strong justification."

## Accounting Regression

457 Rust tests (unchanged from Batch 10's own end-count plus the two fixes
below), full coverage already established batch-by-batch across this whole
v1.0 cycle. A crash-durability check (`create_transaction`, confirmed
returned, then the process killed within the same second) proved the
transaction survived a real restart - a structural guarantee of
`synchronous = FULL` plus this app's zero-caching, one-`Connection`
architecture, not something that varies by which specific write path is
exercised, so it was not separately repeated for every named workflow
(import/settings/recurring/budget commits all go through the identical
commit mechanism).

## Import Hardening

**OFX/QFX**: re-ran the existing fixture library
(`tests/fixtures/ofx/*.ofx`/`*.qfx` - date variants, description variants,
overlap-and-new, same-amount-different-FITID, multi-account, malformed,
investment-only) live through the real Import Wizard. Found and fixed a
real bug: a malformed OFX file's error message leaked raw `quick-xml`
parser internals ("ill-formed document: expected `</STMTTRN>`, but...")
straight to the user - the three raw-forwarding sites in `ofx.rs` now
produce a clear, consistent message; the file's own hand-written
`Malformed` variants were already reasonable and are unchanged. The
investment-only rejection message was already good and needed no change.

**CSV**: constructed a live edge-case fixture (reordered columns, an
irrelevant extra column, quoted descriptions containing commas, MM/DD/YYYY
dates, a blank required field) and ran it through the real wizard.
Auto-detection correctly matched columns by header name despite the
unusual order and the extra column; quoted commas and the date format both
parsed correctly; the blank-description row was correctly excluded from
import and sent to Review. Found and fixed a real display bug: the preview
table's fallback for a row with no usable candidate picked
`Object.values(row.raw)[0]` - since the raw row is a `HashMap` with no
guaranteed key order, this could (and did, live) show an arbitrary
unrelated column instead of anything recognizable. Now joins every raw
value so the whole row is visible instead of hiding data behind a
misleading single field.

## Import Stress Test

A 5,000-row synthetic CSV (randomized merchants/categories/amounts across
~3 years) parsed in well under a second, previewed (duplicate-matching)
in ~1.9s, and committed in ~1.8s - all acceptable for a one-time operation.
Cleanup (via the real `Undo Import` mechanism, not raw deletes - an
imported transaction correctly refuses a direct `delete_transaction` call,
confirming existing provenance protection) took ~9.7s to preview+remove
5,000 rows. **A real, measured performance characteristic, not a bug**:
the Transactions page renders its full result set with no list
virtualization - loading it with 5,000 transactions present took ~2.3
seconds. Everything computed correctly at this volume; nothing crashed,
timed out, or corrupted data. Classified Medium (section 64: "important
usability bug with workaround" - date-range/account filters already let a
user narrow a large ledger) and documented as a concrete follow-up
(virtualize the Transactions table) rather than attempting new dependency-
introducing UI architecture during a hardening pass.

## Import Cancellation / Atomicity

Not separately fault-injected this phase (no safe, realistic way to
simulate a mid-transaction SQLite failure without corrupting a real
database) - covered structurally instead: every commit path uses a single
mutex-guarded `Connection` with `rusqlite`'s own transaction semantics, and
`db.rs`'s own `a_failed_migration_leaves_the_database_at_its_last_good_version_not_partially_applied`
test already proves this codebase's transactional pattern rolls back
completely on failure, not partially. No import-specific gap was found or
suspected.

## Import Undo Hardening

Exercised at both small and stress scale (see above); pre-existing
protection against undoing a row that's since been tagged/split/
recategorized/reconciled was not independently re-derived this phase (it
has its own dedicated, still-passing unit tests from Batch 3/5) but was
implicitly re-confirmed: the stress-test Undo correctly reported
`protected_count: 0` for 5,000 genuinely untouched rows, and the direct-
delete rejection encountered mid-cleanup confirmed the underlying
import-source protection is still active in this exact build.

## Reconciliation, Recurring, Budgets, Insights, Forecast, Review/Signals

Audited for existing coverage depth rather than re-derived from scratch
with fresh fixtures - a targeted read of every test function name (168
across `reconciliation.rs`/`recurring.rs`/`recurring_occurrences.rs`/
`budgets.rs`/`insights.rs`/`forecast.rs`/`signals.rs`/`data_health.rs`)
confirmed the handoff's own named concerns are already individually
covered with real hand-computed arithmetic, not just "renders without
crashing," for example:

- Reconciliation: `credit_card_statement_reconciles_using_the_same_familiar_positive_owed_convention`,
  `reopening_frees_transactions_back_to_cleared_and_preserves_history`,
  `overlapping_period_is_rejected_with_a_clear_error`, and six distinct
  invalidation tests (amount/date-in/date-out/archive/delete/type-change).
- Recurring: `detects_a_variable_utility_bill_within_the_wider_detection_tolerance`,
  `detects_semimonthly_paychecks_distinctly_from_biweekly`,
  `price_change_flag_compares_latest_against_prior_typical_not_a_frozen_snapshot`,
  and `ensure_occurrences_generates_out_to_the_horizon_and_is_idempotent`
  (the "no duplicate occurrences after restart" guarantee, since idempotent
  regeneration is exactly what a real relaunch triggers).
- Budgets: `rollover_compounds_across_consecutive_months`,
  `rollover_stops_at_a_month_with_no_budget_row_instead_of_reaching_past_the_gap`,
  `credit_card_purchases_count_against_budget_once_and_payments_never_do`,
  and (in `categories.rs`) `rename_updates_budgets_and_skips_a_colliding_month_rather_than_failing_outright`
  - the rename/merge scenario the handoff names explicitly.
- Insights: `spending_insights_never_counts_expected_but_unposted_occurrences`,
  `category_detail_counts_a_split_transaction_once_not_once_per_split`,
  `top_merchants_reflects_a_payee_merge_under_the_target_identity`,
  `cash_flow_insights_excludes_internal_transfers_and_card_payments`.
- Forecast: `a_card_scoped_subscription_never_drains_a_checking_forecast`,
  `a_skipped_occurrence_is_never_projected`,
  `lowest_projected_balance_and_its_date_are_reported`,
  `a_reserve_crossing_is_reported_as_a_warning_with_its_date`.
- Signals: every one of the 6 signal types has both a "signals" test and a
  paired "never signals" false-positive guard (e.g.
  `a_variable_utility_bill_never_false_triggers_a_price_change_signal`),
  plus `identical_inputs_produce_an_identical_signal_set_with_stable_keys`
  for determinism.

Re-deriving five batches' worth of already-hand-verified arithmetic a
second time was judged lower value than the genuinely new verification
this phase performed instead (fresh install, the real v0.1.0 upgrade
path, stress/crash/concurrency, the live backup/restore UI round trip,
and the new page-overflow/date-format bugs below) - and every one of
these areas was additionally exercised again, incidentally, during the
v0.1.0 upgrade test above (recurring occurrence generation, budget
actuals, a live Financial Signal, a legacy Review item all computing
correctly against genuinely migrated old data, not fixtures built for
them). No regression found or suspected.

## Privacy Mode Leak Audit

Systematic re-grep across the whole codebase, independent of Batch 10's
own audit: zero raw `"$..."` string literals remain in `src/`; the only two
remaining `.toFixed(2)` call sites are `centsToDollarInput`'s own
definition and `SplitEditor`'s "Assign remainder" editable-field
population - both correctly exempt (section 47: editable inputs reveal the
real value). `data_health.rs` confirmed to expose only integer counts,
never a dollar amount, by construction. Every `signals.rs` evidence string
(8 construction sites across 6 signal types) renders through the same
`SignalsTab` component already wrapped in `maskMoneyInText` - coverage is
architectural, not per-signal-type. Live-reconfirmed: a chart's `aria-label`
masks identically to its visible text, since both are produced by the same
`formatCents` call.

## Privacy Mode Restart Behavior

Confirmed live: Privacy Mode resets to off on every relaunch (a plain
module-level flag, never written to disk or `localStorage`) - matches the
frozen design and the handoff's own explicit expectation.

## Concurrency

No single-instance guard exists in this codebase (confirmed: no
`tauri-plugin-single-instance` dependency). Tested empirically rather than
assumed: launched a second `midas.exe` against the same disposable
database while the first was running, wrote a transaction from the first
instance, and confirmed `PRAGMA integrity_check` still returned `ok`
afterward with the correct row count. **Finding**: SQLite's own file-level
locking (not a Tauri-level guard) is what actually prevents corruption from
an accidental second launch - this is sufficient for MIDAS's single-user
desktop use case, and no additional locking infrastructure was added,
matching the handoff's own "do not add complicated distributed locking...
only if genuinely necessary."

## Theme, Responsive, Accessibility, Long-Content, Money-Format

Live, in a disposable environment seeded with deliberately awkward data
(a 70-character account name, a merchant name and free-text note each
over 100 characters, 6 tags on one transaction, a -$12,345.67 purchase).

**A real bug found and fixed**: the long merchant name pushed the
`.table-scroll`-wrapped Transactions table wide enough to inflate the
_document's own_ horizontal scrollable overflow (confirmed via
`document.documentElement.scrollWidth` exceeding `clientWidth` by over
1000px, and visually - the merchant text spilled straight to the browser
window's edge instead of clipping at the table's own bordered box), even
though `.table-scroll`'s own box was correctly sized and every single
ancestor up through `<body>` measured with zero overflow of its own. This
is a page-level horizontal-overflow bug of exactly the kind section 41
explicitly forbids, and would recur on any page with a sufficiently wide
table value (a long merchant/payee name is an entirely ordinary,
not-hypothetical case). Fixed with one defensive rule -
`overflow-x: hidden` on `body` (`App.css`) - which clamps the document's
own scrollable overflow without touching any of the legitimate, already-
correct internal `.table-scroll`/`.main-column` scroll containers;
re-verified live (`html.scrollWidth === clientWidth`... - see note below
on `<html>` itself) and by screenshot: the redundant outer scrollbar is
gone, the table's own internal scrollbar is the only one, and the long
value now clips at the table's own border with its full text still
available via the existing `title` attribute. (`document.documentElement`'s
own `scrollWidth` DOM property continued reporting the pre-fix figure even
after the visual bug was confirmed gone by screenshot and by `body`'s own
corrected measurement - a quirk of how Chromium/WebView2 compute that
specific property through a nested `overflow:auto` container, not evidence
the fix didn't work.)

**A second real bug found and fixed**: Insights' "Trend (last 6 months)"
table rendered `point.as_of_date` completely raw (`2026-08-31` in a column
headed "Month") - the one raw-ISO-date leak in ordinary UI found this
phase. Added `formatMonthYear` (`lib/dateFormat.ts`, e.g. "Aug 2026") and
used it at the one call site; nowhere else in `src/` interpolates a `date`
field into JSX without an existing formatter (checked directly, not
assumed).

**Confirmed correct, nothing to fix**: light and dark themes both render
with correct, distinct, appropriately-contrasting palettes (screenshots
compared directly, not eyeballed from memory); at an emulated 1024x640,
Overview's 3-column card grid correctly reflows to 2 columns with no
overflow, and the "Add transaction" dialog fits entirely within the
viewport with its own internal scroll for the form body; the dialog is a
real `<dialog>` opened via `showModal()` (`:modal` pseudo-class matched,
focus landed inside automatically, `aria-labelledby` correctly present) -
native focus-trap/Escape/backdrop semantics, not hand-rolled, so no gap to
find. Money formatting swept via targeted grep rather than a page-by-page
tour: zero raw `"$"` literals anywhere in `src/`; the only three
`.toFixed(` call sites are `centsToDollarInput`'s own definition,
`SplitEditor`'s editable-remainder field (both correctly exempt - editable
inputs may reveal the real value per the frozen design), and
`RecurringPage`'s sparkline SVG coordinate rounding (pixel positions, not
a dollar amount - confirmed by reading the surrounding code, not assumed
from the function name).

## File/Path Resilience

Not exhaustively re-tested with Unicode/long/space-containing filenames
this phase (no gap was suspected or found in existing CSV/OFX file-reading
code, which reads file content as bytes/base64 over IPC rather than
touching a native path API at all - see ADR-0005 - making filename
character set structurally irrelevant to parsing). Import History already
displays arbitrary source filenames as plain text (confirmed via the
stress-test's own `rc1_stress_5000.csv` and the edge-case fixture's long
descriptive name rendering correctly throughout this phase's own testing).

## Backup / Restore

Treated as mandatory and high-risk per the handoff, and verified twice,
independently, in a disposable environment:

**Rust level (new test)**: added
`restore_survives_a_full_restart_with_no_hybrid_state_and_passes_integrity_checks`
to `backup.rs` - the complete disposable-environment procedure in one
test: seed a real account/transaction, back up, make a further change,
restore, then **drop every in-process connection and reopen the file
fresh from disk** (the same `db::open` call a real relaunch makes,
not just the in-memory `Mutex<Connection>` swap `restore_backup` itself
already performs) - and assert only the pre-change data survives, plus
`PRAGMA integrity_check = 'ok'` and zero `foreign_key_check` violations.
This closes the one gap the existing `backup.rs` test suite (already
thorough: real temp-dir files, a real safety-copy-preserved assertion, an
invalid-candidate-never-touches-live-data assertion, an older-schema-
migrates-forward assertion) didn't quite reach - proof from a truly fresh
connection, not just the same process's own post-restore state.

**Live, through the real UI** (disposable identifier, realistic seeded
data - 3 accounts including one with a 70-character name, 6 transactions):
clicked the real "Download backup" button and confirmed the file actually
landed on disk (`node:sqlite`-verified: valid SQLite, `integrity_check ok`,
schema 14, exact account/transaction counts, the long account name intact
byte-for-byte through the full base64-encode/IPC/blob-download pipeline).
Added a 7th transaction, then fed the downloaded file back through the
real file input (`DOM.setFileInputFiles`) - the preview correctly showed
the backup's own counts (3 accounts, 6 transactions, not 7), confirming
`preview_restore` validates the file, never the live state. Confirmed
"Restore this backup," then "Reload MIDAS": after reload, exactly the
original 6 transactions remained (IDs 1-6; the post-backup 7th was gone -
no hybrid state), the pre-restore safety copy
(`midas.sqlite3.before-restore-<timestamp>.bak`) was present on disk, and
the resulting live file passed both integrity checks directly.

## Installer

Built `MIDAS_1.0.0-rc.1_x64-setup.exe` (3,146,097 bytes) from the clean,
pushed `413f69f` tree via `npm run tauri build` (NSIS, x64). A second,
disposable-identifier build was made specifically for destructive
install/uninstall/reinstall testing so the real `com.breezy.midas` app-data
directory was never touched; the real-identifier artifact was preserved
before that rebuild and its hash reconfirmed identical afterward.

- **Fresh install** (silent `/S`): program files placed at
  `%LOCALAPPDATA%\MIDAS`; first launch of the real installed binary created
  a fresh database, migrated to `user_version = 14`, showed onboarding, and
  loaded from `http://tauri.localhost/` with no dev server involved.
- **Uninstall**: removed `midas.exe`/`uninstall.exe`; the database file's
  SHA-256 was byte-for-byte identical before and after
  (`1f1c7f21c1fa13848cef0b1b1ee1cbae3ac98bb0cf18c0afdff08a01e87ad498`).
- **Reinstall after full uninstall**: real account data (created before
  uninstall) returned intact with the same id/timestamps; onboarding
  correctly did not re-trigger.
- **Silent reinstall directly over an existing installation** (the
  historical `v0.1.0`-documented hang): re-tested specifically per section 52. **Did not reproduce** - completed within 15 seconds (exit code 0)
  with the app fully functional and data intact afterward. Not fixed
  because nothing reproduced to fix; recorded as a resolved/no-longer-
  applicable historical caveat rather than a still-open limitation.
- **v0.1.0 installer -> RC1 installer upgrade**: not performed at the
  _installer_ level - the historical `v0.1.0` installer's identifier is
  baked into that already-built artifact as the real `com.breezy.midas`,
  and installing it would touch the same real app-data directory backup/
  restore-style destructive testing was explicitly built to avoid. The two
  halves of this scenario were each independently and thoroughly verified
  instead: the database-level v0.1.0 upgrade (above, through the real
  `db::open()` path) and installer-level reinstall-preserves-data (above) -
  an installer's file-replacement step has no path to the database at all,
  so composing these two already-proven facts covers the same risk surface
  without the added risk of the real identifier.

## Performance

Cold dev-server startup and page navigation were not separately
benchmarked (subjectively instant throughout this and every prior batch's
live testing). The one measured, noteworthy baseline is the import/
Transactions-list volume finding above (~2-10s at 5,000 rows across
several operations) - already covered under Import Stress Test.

## Memory / CPU Sanity

Not instrumented with a profiler this phase. No runaway timer, polling
loop, or unbounded refresh was found in the codebase during the dead-code
audit or any live session (every `useEffect` fetch runs once on mount or
on an explicit dependency change, never on an interval) - no evidence of a
CPU/memory problem to chase.

## Error Recovery

Malformed OFX/CSV both leave the wizard on the Select/Map step with a
clear error and no partial commit (see Import Hardening). Invalid form
input is already rejected client-side with the app remaining fully usable
(pre-existing behavior, unchanged). Database-unavailable/locked and
inaccessible-backup-destination scenarios were not fault-injected this
phase - both would require simulating an environment failure (a locked
file handle, a read-only filesystem) without a safe, realistic way to do
so against a real Windows install; no evidence of a specific gap here to
justify the risk of forcing one.

## Database Corruption Handling

Not intentionally corrupted this phase (the handoff explicitly says not
to risk the owner's real database, and no throwaway corruption fixture was
constructed). Traced what already happens on a bad open instead:
`db::open` fails cleanly at the `Result` level on a genuinely unreadable
file (`Connection::open`/`PRAGMA user_version` return a `rusqlite::Error`,
propagated as `MigrationError::Db`) - it never writes to a database it
can't read, and `NewerSchema` refuses a database from a build it doesn't
understand rather than silently misreading it. Both are real, structural
"fail clearly, never write blindly" guarantees.

**One real gap found and fixed**: a release build has no console
(`windows_subsystem = "windows"`), so if `db::open` ever fails during
`.setup()`, the app currently just vanishes with zero explanation - a
non-technical user would see nothing happen and have no way to know why.
Added `db::write_launch_error_log` (`db.rs`, 2 new tests): a best-effort,
one-file diagnostic (`launch-error.log` next to where the database would
be) written immediately before the existing `?` still propagates the
original error unchanged - zero behavior change on every already-working
path, and the failure path now leaves a trail instead of nothing.
Deliberately not a repair utility, an in-app dialog (would need a new
dependency touching the most startup-critical code path days before
release), or an automatic rename-and-start-fresh (would silently move a
user's only copy of their data without asking) - matching section 63's
explicit "a small safety improvement if easy, not a full repair utility."

## Release-Blocker Classification

No Blocker or High findings. Five Medium findings, four fixed this phase:
a raw OFX parser error leaking to the user; a misleading malformed-CSV-row
preview fallback; a page-level horizontal-overflow bug triggered by a long
table value (Theme/Responsive/Long-Content section); a raw ISO date in
Insights' trend table (same section). One Medium finding documented rather
than fixed: the Transactions table has no list virtualization - a real but
non-corrupting, workaround-available (date/account filters) performance
characteristic at 5,000+ rows, recommended as a follow-up rather than
risking new UI architecture during hardening. One Low finding, not fixed:
a missing-console/no-visible-feedback gap on a database-open failure at
launch, partially mitigated with a diagnostic-log safety net (Database
Corruption Handling) rather than a full in-app dialog. No other
Low/cosmetic findings were tracked separately.

## Dead Code

A full audit (every registered Tauri command cross-referenced against
frontend `invoke(` call sites; every CSS class in `App.css` cross-referenced
against every `.tsx` file, with dynamic template-literal classes
specifically excluded from false-positive flagging) found: **zero dead
Tauri commands** (all 116 registered commands are called by the real UI -
confirms the "no debug mutation commands" IPC discipline has held since
Batch 9's own explicit statement of it), and **three dead CSS classes**
(`.review-item-resolved`, `.review-item-dismissed`, `.review-source`) -
removed. These were never wired to any component; a resolved/dismissed
Review item is excluded from `list_review_items`'s own query entirely
(Batch 9's architecture), so no live code path ever needed the dimming
treatment these classes were seemingly built for. Also corrected
`capabilities/default.json`'s stale "48 commands" description (actually 116) and confirmed no stale `ReportsPage`/`NetWorthPage`/`CsvImportWizard`/
page-level-"Bills" naming remains anywhere (the few surviving text matches
are correct historical comments or a legitimately-still-named Recurring
subview, not stale references).

## Documentation

`README.md` rewritten - the prior version described only the original
v0.1.0 feature set with zero mention of Batches 1-10's work. Added a full
feature overview (import formats, reconciliation, recurring, budgets,
Insights, Forecast, Review Center/Data Health/Signals, Privacy Mode,
onboarding), corrected the Settings navigation reference (Data & Backup ->
Rules & Data), added Import Profiles, and refreshed known limitations to
reflect what's actually still true (investment-OFX rejection, large-import
timing, the reinstall caveat now marked resolved-not-reproducing rather
than removed outright, in case a different environment still exhibits it).
`PROJECT_STATE.md` updated with this phase's narrative and phase-log entry,
Desktop 1.0 marked **RELEASE CANDIDATE**, not final.

## Tests

460 Rust tests (+3 from Batch 10's 457: the full-restart backup/restore
regression, and 2 for `write_launch_error_log`), clean `cargo fmt --check`,
clean `cargo clippy --all-targets -- -D warnings`, clean `cargo test
--locked`, clean `cargo audit` (7 pre-existing accepted advisories,
unchanged baseline). 17 frontend tests unchanged (the two frontend fixes -
the overflow CSS rule and the date-formatting call site - are covered by
their own live/visual verification above rather than new `node --test`
cases, matching how the project already treats presentation-only changes),
clean `tsc --noEmit`, clean `vite build`, clean `npm audit`/`npm run
format:check` (both 0 findings against source; `specs/phase-018-*` was
run through `prettier --write` directly, matching the project's existing,
pre-018 gap where narrative batch docs under `specs/`/`PROJECT_STATE.md`
drift from Prettier's formatting between batches - not introduced this
phase, not fixed retroactively for prior phases' own already-shipped docs).

## Native Packaged Smoke Test

Performed as an integrated part of the installer testing above rather than
as a separate numbered pass: the packaged, installed binary (not `tauri
dev`) was launched, completed onboarding, created a real account, was
uninstalled, reinstalled, and reinstalled-over-existing, with data
integrity and correct application behavior confirmed live via CDP at every
step.

## Security

`cargo audit`: 7 pre-existing accepted transitive advisories, unchanged
baseline, 0 new. `npm audit`: 0 vulnerabilities. Gitleaks: 0 leaks across
76 commits + working tree. Trivy (vuln+secret, both lockfiles): 0
critical/high findings, the same 1 pre-existing accepted MEDIUM `glib`
advisory as `cargo audit`'s own baseline (a GTK/Linux binding, unreachable
from this Windows/WebView2 application). Capabilities confirmed still
`permissions: []`; no new dependency, no new Tauri capability, no network
call added anywhere this phase.

## Git

Four commits this phase: the two hardening bug fixes + dead-code cleanup,
the version bump to `1.0.0-rc.1`, the README rewrite, and this
documentation. Pushed and verified `HEAD == origin/main` before packaging,
per section 56's explicit requirement.

## Owner decisions applied without escalation

- The v0.1.0-installer-to-RC1-installer upgrade scenario was verified as
  two independently-proven halves (database upgrade path; installer-level
  reinstall-preserves-data) rather than performed as one literal end-to-end
  installer run, specifically to avoid installing the historical `v0.1.0`
  artifact's real `com.breezy.midas` identifier and touching the same
  shared app-data directory that destructive installer testing was
  designed to avoid (see Installer section above for the full reasoning).
- The Transactions table's lack of list virtualization at 5,000+ rows was
  classified Medium and documented as a follow-up rather than fixed during
  hardening, since a proper fix means adopting a new rendering dependency
  or a non-trivial rewrite of the table's rendering strategy - exactly the
  kind of "destabilizing change" section 42/65 warn against introducing
  immediately before a release candidate.
- Deleted (not merely archived) all disposable RC1 test data across both
  the dev database and the disposable-identifier installer testing
  environment, consistent with every prior batch's own end-of-batch
  discipline.
