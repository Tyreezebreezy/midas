# Phase 008: Foundation Batch 1 (post-1.0-cycle schema/model groundwork)

- Status: COMPLETE
- Owner gate: APPROVED 2026-09-12 (owner handoff: "MIDAS Desktop v1.0 Development Handoff — Start of the Back-to-Back Development Cycle")

## Goal

Lay the schema/model groundwork the next several development batches need
(Import Engine v2, reconciliation, payees, tags, splits, recurring
intelligence, provenance, Insights) without building any of those features
yet, and without regressing anything MIDAS 0.1.0 already does correctly.

## Scope

- New tables: `payees`, `tags`, `transaction_tags`, `import_profiles`.
- New `transactions` columns: `payee_id`, `split_parent_transaction_id`,
  `external_transaction_id`, `reconciliation_status`, `is_one_time`.
- New `imports` columns: `source_type`, `import_profile_id`.
- One narrow Tauri command (`set_transaction_reconciliation_status`),
  mirroring the existing `set_transaction_archived`/`set_account_archived`
  pattern - the only new column with an obvious, narrow toggle operation.
- Full Rust model/persistence layers for the new tables (`payees.rs`,
  `tags.rs`, `import_profiles.rs`), each with the same CRUD-plus-Tauri-command
  shape every existing module in this codebase uses (`accounts.rs`,
  `budgets.rs`, `rules.rs`, `savings_goals.rs`) - see "Revision" below for why
  this differs from an earlier draft of this spec.
- A migration upgrade test built from a representative v0.1.0-shaped
  fixture (every table, realistic multi-type data) proving the upgrade
  preserves everything and defaults new columns sanely.

## Revision (session handoff, 2026-09-12)

This batch was built across two concurrent Claude sessions on the same
machine, discovered mid-work when both were found editing `C:\Dev\midas` at
once. The owner stopped one session and directed the other (this one) to
take sole ownership, re-verify everything already in the working tree
against the actual codebase rather than trust it as-is, and finish the
batch. Two things from the stopped session's own draft of this document
were corrected on that re-verification:

- **The stated reason for withholding Tauri commands on `payees.rs`/
  `tags.rs` did not hold up.** The draft cited `AGENTS.md`'s "no command
  exposed until a concrete UI workflow needs it" - that sentence does not
  exist in `AGENTS.md`, the constitution, or any `standards/` file (checked
  directly). On its own merits it also cuts against this codebase's own
  established, consistent pattern: every existing model
  (`accounts`/`transactions`/`budgets`/`rules`/`savings_goals`/`recurring`)
  ships its full CRUD command surface as part of being built, not deferred
  to whichever later batch adds a UI for it. A registered-but-UI-less
  command is not meaningfully different from an unregistered one from a
  security standpoint either - this app has no auth boundary between its
  own frontend and its own backend by design, and the same
  `window.__TAURI_INTERNALS__.invoke` path used to verify every command in
  this project natively already reaches unregistered `pub fn`s directly
  from Rust test code regardless. Commands were added back and registered
  in `lib.rs`; `import_profiles.rs` was written the same way for
  consistency. Everything else in the stopped session's work (the
  migration, `transactions.rs`'s `reconciliation_status` support, the
  v0.1.0 upgrade fixture test) was reviewed and kept as-is - it was sound.
- A `v1.0.0` mention in the original "Verified current state" section below
  was a same-sentence self-corrected slip (immediately followed by "i.e.
  `v0.1.0`") - confirmed via `git tag -l` that only `v0.1.0` exists. Not a
  real discrepancy, noted here only because this revision is auditing every
  claim in the document rather than assuming good faith on typos too.

## Explicit non-goals

- No Import Engine v2 UI/logic (OFX/QFX/PDF parsing, profile
  auto-recognition, provenance display). `import_profiles.rs` is storage
  and CRUD only - nothing yet matches a profile to an incoming file.
- No reconciliation workflow/UI (statement entry, cleared-balance math).
  `set_transaction_reconciliation_status` is a data mutation only; the
  "editing a reconciled transaction should warn" UX from the handoff is a
  future UI batch's job.
- No Payee Library UI or auto-population from import/rules -
  `transactions.payee_id` stays unpopulated by anything until a later batch
  teaches import/rules to set it.
- No tag-based filtering/UI.
- No split-transaction UI/creation logic - `split_parent_transaction_id`
  exists and is indexed, nothing writes to it yet.
- No Recurring Intelligence lifecycle expansion (`recurring_series.status`
  stays a 3-state enum this batch - the richer Keep/Review
  Later/Planning-to-Cancel/No-Longer-Active/Not-Recurring lifecycle belongs
  to that feature's own dedicated batch, not this schema-foundation one).
- No UUIDs. Nothing in this batch's target systems (Import Engine v2,
  reconciliation, payees, tags, splits, provenance, Insights) needs one;
  the only plausible reason would be future multi-device sync (CITADEL),
  which this cycle explicitly excludes. Adding one now would be exactly the
  speculative field the handoff warns against.
- Dropping the legacy free-text `transactions.tags` column: confirmed dead
  (`TransactionForm.tsx` always sends `tags: null`; nothing else reads it),
  but a real user database already exists on this machine, and "no silent
  destructive behavior" outranks a cosmetic cleanup. Left in place, flagged
  for removal in a later batch.

## Verified current state

Read before writing anything (both sessions, between them): `PROJECT_STATE.md`,
both product/build handoffs, `MIDAS_CONSTITUTION.md`, `AGENTS.md`, all
`standards/`, all 9 ADRs, all migrations, and
`db.rs`/`accounts.rs`/`transactions.rs`/`accounting.rs`/`import.rs`/
`rules.rs`/`needs_review.rs`/`recurring.rs` in full.

MIDAS 0.1.0 is genuinely released: git tag `v0.1.0` exists both locally and
on `origin`, and a Gitea release with 2 assets (installer + `.sha256`) is
published. 180 Rust tests before this batch, not a scaffold. Balance is
always a live `SUM(transactions.amount_cents) WHERE is_archived = 0`
(ADR-0007); spending/budget/Safe-to-Spend math all route through the single
`accounting::spending_effect` function keyed on `transaction_type`. Neither
is touched by this batch - every new column is nullable or has a default
that changes nothing for existing rows, and nothing yet reads the new
columns, so the accounting boundary's existing tests stay valid unmodified
evidence.

A real database exists at `%APPDATA%\com.breezy.midas\midas.sqlite3`, at
schema version 7 before this batch (one archived test account left over
from the v0.1.0 release cycle's own native verification, zero live
transactions). Backed up (SHA-256 verified against the live file) before
any work began; never opened directly during development - all new tests
use in-memory fixtures, matching existing convention. The real database
_was_ used for one round of native verification (see Native verification
below), after which it was migrated to schema version 8, still holds only
archived/cleaned test artifacts, and remains backed up.

## Constraints and architecture boundaries

- SQLite via rusqlite, one mutex-guarded connection - unchanged.
- Migrations tracked via `PRAGMA user_version`, append-only, each in its own
  transaction - unchanged mechanism (`db.rs`).
- Zero Tauri plugin capabilities - unchanged; every new command is a plain
  app-defined command like every existing one, still `permissions: []`.
- No new dependencies.

## Security and privacy considerations

No new attack surface: no new Tauri capabilities, no new file/network
access. The new commands are the same shape as every existing command
(typed input, one mutex-guarded SQLite connection, no filesystem/dialog/
shell access). Existing dependency/audit/Gitleaks/Trivy posture unchanged
(no new crates).

## Acceptance criteria

- [x] Migration 0008 applies cleanly to a fresh database.
- [x] Migration 0008 applies cleanly to a representative v0.1.0-shaped
      fixture (every existing table populated) with zero data loss and
      correct defaults on every new column (`db.rs` test).
- [x] Migration 0008 applies cleanly to the real machine-local v0.1.0
      database (native verification, not just the synthetic fixture).
- [x] `cargo fmt --check`, `cargo check --locked`, `cargo clippy --locked
--all-targets -- -D warnings`, `cargo test --locked` all pass (201
      tests: 180 existing + 21 new, 0 failed).
- [x] `cargo audit`: exit 0, same pre-existing accepted advisory class as
      every prior phase (unmaintained/unsound transitive deps, no new
      crates added).
- [x] `npm run check` (frontend) passes unchanged - this batch does not
      touch the frontend (confirmed no `src/` files modified).
- [x] Gitleaks (0 leaks, 40 commits) and Trivy (0 vulnerabilities, 0
      secrets, both lockfiles) clean.
- [x] Existing 180 tests still pass unmodified (proves no regression to
      accounting/import/rules/recurring/budgets/goals/net-worth/backup).

## Test strategy

New `#[cfg(test)]` coverage in each new module (`payees.rs`: 7 tests
including merge and chained-merge rejection; `tags.rs`: 5 tests including
the no-`ON DELETE CASCADE` cleanup path; `import_profiles.rs`: 5 tests
including full `ColumnMapping` JSON round-trip) plus 3 new tests in
`transactions.rs` for `reconciliation_status` and 1 new `db.rs` migration
test seeding a realistic multi-table v0.1.0 fixture and asserting every
prior value is byte-identical after migrating, with new columns defaulted
correctly.

## Native verification

Launched the real `tauri dev` binary (WebView2 remote debugging, this
project's standing verification method) against the actual machine-local
v0.1.0 database, not a fixture. Confirmed: the existing archived account
survived the v7->v8 migration untouched; every new command
(`create_payee`/`list_payees`/`merge_payees`, `create_tag`/`list_tags`/
`attach_tag`/`list_tags_for_transaction`,
`create_import_profile`/`list_import_profiles` with a full column-mapping
round trip, `set_transaction_reconciliation_status`) works correctly
end-to-end through real IPC; existing regression paths
(`get_overview`, `get_net_worth`, `list_transactions`) still return correct
results. All test data created during this pass was deleted or archived
before closing the app - the real database holds no new visible test
artifacts beyond what was already there from the v0.1.0 release cycle.

## Owner decisions

None required - every choice in this batch, including the one correction
in "Revision" above, was resolvable from the handoff, the existing
codebase's own conventions, and direct verification against project
documents.

## Hard-stop conditions

None encountered.
