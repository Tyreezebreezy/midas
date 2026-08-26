# Phase 006: Hardening

- Status: COMPLETE
- Owner gate: APPROVED — a detailed hardening mandate covering backup/export/restore, migration hardening, error handling, accessibility, desktop/laptop layout, performance, security/privacy, a Ponytail simplification pass, native verification, and documentation, with the explicit framing: "Do not add new finance features unless required to fix a verified defect... make the existing application resilient, recoverable, accessible, performant, and safe enough for a release candidate." Explicit instruction to stop and report before Phase 7 rather than continue automatically.

## Goal

Turn five phases of feature work into something a real user could trust with their real financial data: recoverable if something goes wrong, honest when something fails, usable by keyboard, legible at realistic window sizes, fast enough at realistic scale, and free of unnecessary attack surface - without adding a single new finance feature.

## Scope

- **Backup, export, and restore** (`backup.rs`, new): a full-database backup via SQLite's own `VACUUM INTO`; a CSV transaction export (`transactions::export_csv`); a two-step restore (`preview_restore` validates a candidate file against a temporary, separate connection with zero side effects; `restore_backup` re-validates and only then swaps it in, after renaming the previous live database aside rather than deleting it). All three move data over the app's existing zero-capability IPC pattern (ADR-0005) - no new Tauri plugin capability.
- **Migration hardening** (`db.rs`): `migrate` now rejects a database with a schema version newer than this build supports instead of silently proceeding (`MigrationError::NewerSchema`); new tests cover a representative partial-schema upgrade with real preexisting data, and a deliberately-failing migration proving the transaction rolls back to the last good version with no partial state.
- **Error handling review**: every workflow the owner listed was read end-to-end, not sampled. Found and fixed four real gaps (detailed under Acceptance criteria).
- **Accessibility**: a live keyboard-only pass against the running app (tab order, focus visibility, dialog focus-trap, Escape, focus restoration) plus a precise WCAG contrast computation for every theme/token pair - found and fixed two real AA failures in the light theme.
- **Desktop/laptop layout**: live-rendered screenshots at 1280x800 and a smaller 1024x640 laptop size across Overview, Reports, and Backup - found and fixed a real horizontal-overflow bug in the main nav bar and a toolbar-actions row, both missing `flex-wrap`.
- **Performance**: a one-time benchmark (not a permanent suite - timing assertions don't belong in a regression gate) against ~3,000 fabricated transactions across 5 accounts, budgets, and review items - every aggregate query completed in single-digit-to-low-double-digit milliseconds. No optimization was applied because none was justified by the numbers.
- **Security/privacy**: capabilities and CSP re-reviewed (still zero plugin permissions, still no external `connect-src`); confirmed no telemetry/analytics and no logging exists anywhere in the app; full scan (Gitleaks, Trivy, `cargo audit`, `npm audit`, `cargo clippy -D warnings`) clean.
- **Ponytail**: continuous inline review during construction (multiple findings fixed as they were found) plus a dedicated repo-wide `/ponytail-audit` pass at the end - found and removed two genuinely dead Tauri commands.
- **Documentation**: README rewritten with a "Your data" section (location, backup, restore, export, security/privacy, known limitations) and its stale Phase-1-era Gitea-push note corrected; this spec; PROJECT_STATE updated.

## Explicit non-goals

- Any new finance feature, calculation, or UI surface beyond what hardening required - the Backup tab is infrastructure, not a finance feature.
- A `dialog`/`fs` Tauri plugin capability for backup or restore - both move file content as plain strings/base64 over the app's own commands, matching CSV import's existing zero-capability precedent (ADR-0005), which also kept this phase inside the owner's own pre-authorized "no material new dependencies or permissions" boundary.
- A permanent performance-regression test suite - one-time measurement against a realistic dataset, documented here, is what was asked for; timing assertions in the normal gate would be environment-fragile and weren't requested.
- Scheduled/automatic backups, at-rest database encryption, or multi-device sync - real gaps, but new features, not hardening of what exists; documented as known limitations in the README instead.
- Fixing `RUSTSEC-2024-0429` (glib, pre-existing since Phase 0) - unchanged, Linux-only backend dependency, tracked since ADR-0002.

## Verified current state

- Builds on Phase 5 (`5cc8758`).
- New Rust dependency: `base64` (0.22.1) - promoted from an already-present transitive dependency (via Tauri itself) to direct use; not a new supply-chain addition.
- `src-tauri/capabilities/default.json`: still zero Tauri plugin permissions. Description updated to reflect 47 commands (down from 49 - two dead ones removed, see Ponytail below) and the file-handling pattern.
- Schema: no new migration - `db.rs` gained a schema-version guard and `pub(crate) MIGRATIONS`/`current_schema_version()` for `backup.rs` to reuse, not a new table.
- New Rust module: `backup.rs` (`create_backup`, `preview_restore`, `restore_backup`, plus their three commands). `transactions.rs` gained `export_csv`/`export_transactions_csv`.
- New frontend page: `BackupPage.tsx`, wired as a new "Backup" tab.
- 47 app-defined commands total (45 from Phase 5 + 4 new [backup, preview-restore, restore, CSV export] − 2 removed dead ones).
- Removed: `settings::get_setting`/`set_setting`, two generic Tauri commands nothing in the frontend ever called (every real setting had its own typed, validated command already).

## Backup, export, and restore

- `create_backup`: `VACUUM INTO` a scratch path in the OS temp directory (never a user-chosen path), read the bytes back, delete the scratch file, return base64 plus metadata (schema version, account/transaction counts, a timestamped filename). The frontend triggers a normal browser download via a Blob and `<a download>` - the same "content crosses IPC as a string, the browser handles the actual save" shape CSV import already established, just in the opposite direction.
- `preview_restore`: decodes the candidate to its own scratch file, opens it as a **separate** connection, confirms it has MIDAS's core tables and a schema version this build understands, and returns a summary - the live database and live connection are never touched. The frontend shows this summary and requires an explicit second click before anything real happens.
- `restore_backup`: re-validates (never trusts a prior preview call), then - only if valid - drops the live connection, renames the current live database aside to `midas.sqlite3.before-restore-<timestamp>.bak` (never deletes it), copies the validated candidate into place, reopens it (running `migrate` automatically if it came from an older schema), and swaps the live connection over. An invalid candidate never reaches any of this - the live database and file are provably untouched (regression-tested).
- `export_csv`: a plain, human-readable CSV (date, account name, merchant, category, subcategory, type, dollar amount, notes) of every non-archived transaction - not intended to round-trip back through the bank-CSV importer, which is deliberately fuzzy about incoming column shapes rather than matching MIDAS's own export format.

## Migration hardening

- `migrate` now compares the database's `PRAGMA user_version` against `MIGRATIONS.len()` in both directions: too low, catch it up (existing behavior); too high, refuse with `MigrationError::NewerSchema` naming both versions, rather than silently proceeding to read/write a schema this build was never compiled against.
- `migrate_resumes_from_a_partial_upgrade_and_preserves_existing_data`: applies only migrations 1-3 to a fresh connection, inserts a real account and transaction at that schema shape, then runs the full `migrate` and confirms both the final version and the preexisting data survive - the "representative upgrade from an earlier MIDAS schema" the owner asked for.
- `a_failed_migration_leaves_the_database_at_its_last_good_version_not_partially_applied`: a `migrate_with` helper (the same body, parameterized on the migration list) lets a test inject a deliberately-broken final migration and confirm the failure doesn't advance `user_version`, doesn't leave partial schema objects behind, and that the real migration chain can still resume cleanly afterward - "failed migrations do not leave silently corrupted state," proven, not assumed from the transaction wrapping already being correct.

## Error handling: what was found

Every workflow the owner listed was read end-to-end. Four real, confirmed gaps:

1. **CSV import duplicate-review message** (`import.rs`): `row.duplicate_of_transaction_id.unwrap_or(0)` could have produced "possible duplicate of transaction #0" - a fabricated, misleading reference - if that field were ever genuinely absent for a `ProbableDuplicate` row. Fixed to omit the reference gracefully instead of inventing one.
2. **`RecurringPage.tsx`**: its initial account-list fetch (`accountsApi.list(false).then(...)`) had no `.catch()` at all - a failure would leave the account dropdown silently empty forever with no visible error, an unhandled promise rejection. Fixed with a `.catch(setError)`.
3. **`ReportsPage.tsx`'s category drilldown**: its transaction fetch also had no `.catch()` - worse than a silent empty state, a failure would leave the drilldown permanently showing "Loading…" since the component's only exit from that state was a successful response. Fixed with an error state and a proper error render.
4. **`backup.rs`'s own tests** (found via the native-verification pass, not code reading): three tests called `std::fs::remove_dir_all(&dir).ok()` while a `Mutex<Connection>` (and once, a raw `Connection`) still held an open handle on a file inside that directory. Unix silently allows removing an open file; Windows does not - the `.ok()` masked the resulting cleanup failure completely, leaving real leftover `.sqlite3`/`.bak` files scattered in the OS temp directory after every `cargo test` run on this machine. Fixed by explicitly dropping every connection before cleanup.

Also simplified (found during the same pass, not a defect but adjacent accidental complexity): `transactions.rs`'s `validate_and_normalize` computed and validated a normalized `posted_date`, then discarded it (`let _ = posted;`) while both `create` and `update` independently re-parsed the exact same field right after calling it. Now `validate_and_normalize` returns the value once; both callers use it.

Every other named workflow (account operations, manual transactions, duplicate handling, Needs Review, rules, recurring detection, budgets, pay cycles, reconciliation, Safe to Spend) was read and found already correctly handled - errors surfaced via `setError`, never converted to a silent `$0`/empty list/apparent success. The remaining `.unwrap_or_default()`/`.unwrap_or(0)` sites found by grep were individually verified as defensively-unreachable (e.g. `money::parse_date`'s digit parsing, gated by an `all_numeric` check immediately above it, with the calendar-date validator downstream catching even a hypothetical overflow) or legitimate non-error defaults (`exclude_id.unwrap_or(0)` as a harmless SQL sentinel), not silent-failure bugs - not "fixed" because there was nothing wrong with them.

## Accessibility

Live-verified against the running app over the WebView2 CDP debug port, not inferred from code alone:

- **Keyboard tab order**: from a clean focus state, Tab correctly walks the theme control then every nav tab in order, each with a visible focus outline.
- **Dialog focus trap**: opening the "Add account" dialog moves focus to its first field; 8 subsequent Tab presses stay inside the dialog for the entire cycle (all 7 focusable fields/buttons, wrapping back to the first) - never escaping to the page behind it.
- **Escape and focus restoration**: Escape closes the dialog and focus returns to the exact button that opened it - native `<dialog>` + `showModal()` behavior, confirmed working in this WebView2 build, not just assumed from the spec.
- **Status indicators**: every status badge (`badge-review`/`badge-valid`, `amount-owed`/`amount-positive`) always wraps real text ("Over"/"Under"/"Near", the signed dollar amount itself) - color is a supplementary cue, never the only channel.
- **Reduced motion**: the app has no CSS transitions or animations anywhere outside the existing `prefers-reduced-motion` override, so there is nothing left to guard beyond what's already there.
- **Contrast**: computed precisely (WCAG relative-luminance formula, not eyeballed) for every text/background token pair in both themes. Dark theme passed everywhere already. Light theme failed 4.5:1 in two real, frequently-encountered places: `--midas-gold` as used by `.btn-link` (every "Edit"/"Delete"/"Confirm"/"Reject" text-link app-wide) at 3.56:1, and `--midas-positive` as used by `.amount-positive` (every positive dollar amount in a table) at 4.44:1. Both darkened just enough to clear 4.5:1 with margin (`#856716` → 4.71:1, `#137238` → 5.33:1) while staying visually the same gold/green; `--midas-focus` (aliased to gold) improved as a side effect.

## Desktop/laptop layout

Live screenshots at 1280x800 (the stated baseline) and 1024x640 (a smaller realistic laptop size) across Overview, Reports, and Backup, in the light theme (to also confirm the contrast fix visually). Found a real, pre-existing overflow bug: `.main-nav` had no `flex-wrap`, so as tabs accumulated across phases (12 by now) it silently overflowed horizontally at 1024px, pushing the last several nav tabs entirely off-screen with only a barely-visible bottom scrollbar as a clue. `.accounts-toolbar-actions` (Reports' month-navigation row, reused from Accounts) had the same gap - its parent wrapped, but the actions row itself didn't, clipping the "Next month" button. Both fixed with one `flex-wrap: wrap` each; re-verified at both sizes, and the fix turned out to be a strict improvement even at 1280px (tab labels no longer awkwardly wrap mid-word across 3 lines).

## Performance

A one-time benchmark (not a permanent suite) seeded ~3,000 transactions across 5 accounts, 8 categories, 8 budgets, and 30 Needs Review items, then timed every major aggregate query in release mode:

| Operation                                           | Time    |
| --------------------------------------------------- | ------- |
| `transactions::list` (all, unfiltered)              | 11.1 ms |
| `overview::build` (the whole home screen)           | 20.8 ms |
| `reports::spending_by_category`                     | 0.33 ms |
| `reports::income_vs_spending_trend`                 | 2.6 ms  |
| `accounting::net_worth_history`                     | 6.6 ms  |
| `budgets::list_for_month`                           | 1.4 ms  |
| `needs_review::list`                                | 0.12 ms |
| `transactions::export_csv`                          | 3.2 ms  |
| `backup::create_backup` (`VACUUM INTO` + read-back) | 23.7 ms |

Every figure is comfortably under any threshold a user would perceive as slow. No index was added and no query was rewritten, because the numbers didn't justify it - adding either preemptively would have contradicted the phase's own instruction to optimize only when timings justify it.

## Ponytail

Continuous inline review during construction, findings applied as found (not batched): a scratch-file-naming race in `backup.rs` that used `process::id()` alone (constant for the app's whole lifetime) and would collide under concurrent calls - fixed before it ever shipped by adding a nanosecond-timestamp component. A dedicated repo-wide `/ponytail-audit` pass at the end cross-checked every CSS class against component usage, every exported TypeScript type against its consumers, and every registered Tauri command against actual frontend `invoke()` calls. Most apparent hits were false positives once verified individually (multi-class attribute strings, template-literal class construction, structural type inference) - genuinely nothing to cut there. One real finding: `settings::get_setting`/`set_setting`, two generic pass-through commands nothing in the frontend ever called, left over from before every real setting grew its own typed, validated command. Removed (net -13 lines, 0 dependencies). Full regression suite (174 Rust tests, full frontend check) reran clean afterward.

## Security and privacy

- Capabilities (`src-tauri/capabilities/default.json`): still zero Tauri plugin permissions, unchanged by this entire phase despite adding backup/restore/export - all three move data as plain strings over existing app-defined commands.
- CSP: unchanged, still no external `connect-src` - `default-src 'self'; connect-src ipc: http://ipc.localhost; ...`.
- No telemetry or analytics: confirmed by grep across the full source tree (Rust and TypeScript) for every common analytics SDK name and for any direct `fetch`/`XMLHttpRequest` call - the app makes zero HTTP requests of its own; every network-shaped call is Tauri's local IPC to Rust.
- No logging anywhere: confirmed by grep for `println!`/`eprintln!`/`dbg!`/`console.log`/`console.error`/`console.warn` across the entire app - none exist, so there is no log for financial data to leak into in the first place.
- File handling: no user-controlled file _path_ ever crosses the IPC boundary in either direction, for CSV import, CSV export, backup, or restore - only file _content_ does (text or base64), and Rust always decides where things are actually written (its own generated temp paths, or the app's own `app_data_dir()`), eliminating path-traversal risk by construction rather than by validation.
- Gitleaks (8.30.1): full history (30 commits) and working-tree diff both clean.
- Trivy (0.72.0, vuln+secret): 0 secrets; 1 pre-existing MEDIUM (`glib`, unchanged since Phase 0/1). No new findings - the one new dependency (`base64`) was already present transitively.
- `cargo audit`: 0 vulnerabilities, same 17 pre-existing allowed transitive warnings, unchanged.
- `npm audit`: 0 vulnerabilities.
- `cargo clippy --all-targets -- -D warnings`: clean.

## Native verification

Every workflow above that touches the real app was exercised against the actual running Tauri app over the WebView2 CDP debug port, not assumed from unit tests alone:

- **Backup → state change → restore → data verification**: seeded a real account with a real balance and transaction; created a real backup; materially changed the live state (archived that account, created a different one); confirmed `preview_restore` reported the backup's original contents correctly _without_ changing the now-different live state; confirmed `restore_backup`; verified the live database now held exactly the original account (correct name, correct balance, correct transaction) and the account created after the backup was gone. Cleaned up afterward, including a stray safety-copy `.bak` file this round-trip itself created.
- **Keyboard navigation, dialog focus-trap, Escape, and focus restoration**: detailed under Accessibility above - driven by real synthetic keyboard events against the live DOM, not inferred.
- **Layout**: real screenshots at two window sizes, in the light theme, across three pages - the nav-wrap bug was found this way, not by reading the CSS.
- **Post-cleanup sanity check**: after removing the two dead settings commands (Ponytail), confirmed live that `get_setting` now correctly returns "command not found" while `get_pay_schedule`, `get_safe_to_spend_reserve`, and the full `get_overview` aggregation all still work.

## Documentation

- README: added a "Your data" section (location, backup, restore, export, security/privacy, known limitations - the exact list the owner asked for) and corrected a Phase-1-era stale statement claiming Gitea pushing was still blocked (it's worked since Phase 2).
- This spec.
- `PROJECT_STATE.md` updated for Phase 6 completion.

## Owner decisions

None required - fully specified in the owner's Phase 6 authorization message.

## Hard-stop conditions

- Any need for a new Tauri plugin capability (`dialog`/`fs`) - none arose; the zero-capability IPC pattern covered backup/export/restore completely.
- Any accessibility or layout defect requiring a structural redesign rather than a targeted fix - none arose; both real findings (contrast, nav wrap) were one-property fixes.
- Any performance finding requiring a new index or query rewrite - none arose; every timing was already comfortably fast.
- Any security finding requiring a new dependency, a capability grant, or a scope change - none arose.
