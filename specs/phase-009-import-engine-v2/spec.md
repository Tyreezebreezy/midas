# Phase 009: Import Engine v2 (CSV source adapter)

- Status: COMPLETE
- Owner gate: APPROVED 2026-09-12 (owner handoff: "MIDAS Development Batch 2 - Import Engine v2")

## Goal

Evolve the CSV importer from a self-contained workflow into the first source
adapter feeding a normalized MIDAS ingestion pipeline: profiles, deterministic
matching, provenance, import history, and a safe undo - without regressing
anything the v0.1.0 importer already did correctly, and without building
OFX/QFX/PDF/SimpleFIN/AI (explicitly out of scope this batch).

## Reconnaissance (before writing code)

The existing `import.rs` already had a real staged pipeline: `read_csv` ->
`detect_mapping` -> `build_staging_rows` (parses + classifies) -> `commit`
(writes to `transactions`, records an `imports` row). Duplicate detection was
two-tier: an exact SHA-256 row hash (account + date + amount + description),
and a "probable duplicate" match on date+amount alone, ignoring description
entirely - which both worked correctly for existing tests but would flag two
genuinely different same-day, same-amount purchases as a duplicate, and
would miss recognizing a slightly-reformatted re-export as the same
transaction. `import_profiles.rs` (Foundation Batch 1) was storage-only -
`ColumnMapping` had no external-ID or sign-inversion field, nothing matched a
profile to a file, and no frontend code referenced it at all.
`transactions.rs`'s Foundation Batch 1 columns (`payee_id`,
`external_transaction_id`, `reconciliation_status`, `is_one_time`,
`split_parent_transaction_id`) all existed but nothing had ever written to
them. There was no import-history listing, no undo, and the CSV preview
table's date column was still raw ISO text (a known limitation recorded in
the v0.1.0 release notes).

## Architecture

```
CSV Source Adapter (read_csv, detect_mapping)
        v
NormalizedCandidateTransaction  <- the ingestion boundary; source-agnostic
        v
Matching (match_candidate: New / ExactDuplicate / StrongMatch / PossibleMatch)
        v
Rules / Payee resolution (existing rules::best_match; payee link by exact
        name match against the Payee Library)
        v
Review where uncertain (existing needs_review_items, reused not duplicated)
        v
Commit (writes transactions + payee_id + external_transaction_id +
        reconciliation_status default + provenance)
        v
Import History / Undo (imports table extended; undo removes untouched rows)
```

No `SourceAdapter` trait was introduced - CSV is the only adapter, and a
trait with one implementor is exactly the speculative abstraction the
handoff's simplification section warns against. The extension point for a
future OFX/QFX adapter is the shared `NormalizedCandidateTransaction` shape
and the source-agnostic matching/commit/undo/history functions, which any
future adapter constructs into and calls without touching the CSV-specific
parsing code.

## Backend changes

- **`ColumnMapping`** (`import.rs`): added `external_id_column` and
  `invert_amount`, both `#[serde(default)]` so mappings saved before this
  batch (in `imports.column_mapping_json` / `import_profiles`) still
  deserialize unchanged. `detect_mapping` also recognizes common
  transaction-ID headers. `required_headers()`/`missing_headers()` added and
  reused by both profile recognition and a fail-fast "this file doesn't
  match this mapping" commit-time check.
- **`NormalizedCandidateTransaction`** (renamed/evolved from
  `ProposedTransaction`): the ingestion-boundary shape from product handoff
  section 6, with `source_type`, `account_id`, `external_transaction_id`,
  and `candidate_payee_id` added.
- **Matching** (`match_candidate`, `MatchOutcome`): four deterministic
  outcomes, no scoring. Only a byte-identical hash or a matching source
  `external_transaction_id` auto-skips without asking - a "strong" match
  (same account/amount/date/description) is high-confidence but still
  surfaced to the owner as a `PossibleMatch`-status row, because two
  genuine same-merchant same-amount purchases on the same day are
  indistinguishable from this signal alone (product handoff section 16: a
  false-positive Review item is always preferred over silently discarding
  real data). Description similarity (`descriptions_are_similar`) is exact
  match or one description a prefix of the other (>= 4 chars) - reuses this
  codebase's own exact/prefix vocabulary rather than a scored heuristic.
- **Payee linkage**: `build_staging_rows` loads every non-archived payee
  into a name-indexed map once (mirrors the existing load-once pattern for
  rules and the ledger) and links a candidate when its rule-resolved
  merchant exactly matches a payee name. Never fabricates a payee.
- **`imports` table** (migration 0009): `excluded_count`, `undone_at`,
  `undone_removed_count`. `commit` now records `import_profile_id` and
  `source_type`, and writes `payee_id`/`external_transaction_id` onto
  committed transactions.
- **Import History** (`import::list`, `ImportRecord`): every import stays a
  permanent record, including ones that committed nothing (an exact
  reimport) or were later undone - `remaining_transaction_count` is
  computed live from `transactions.source_import_id`, the same
  computed-not-stored approach ADR-0007 uses for account balance, rather
  than a count that could drift from reality.
- **Undo** (`preview_undo`, `undo`): a transaction is removable only if
  untouched since creation (`created_at == updated_at`, which already
  covers an edit, an archive, or a reconciliation-status change - every one
  of those bumps `updated_at`) and not itself split into a later
  transaction. Always removes exactly the safe subset in one DB
  transaction; refuses to run twice on the same import so a repeat call
  can never overwrite the true `undone_removed_count` with a smaller
  number. The normal `transactions::delete` guard blocking deletion of
  anything with a `source_import_id` is untouched - undo's raw delete is
  the one narrow, explicitly justified exception, used only here.
- **`TransactionFilter.source_import_id`**: one new optional field so the
  Import History detail view and undo both reuse the existing, tested
  `transactions::list` rather than a parallel query.
- **`import_profiles.rs`**: `recognize()` (conservative profile
  recognition, product handoff section 8) added. A profile is compatible
  only when every column it depends on is verbatim present in the file's
  headers; a profile scoped to a specific account is never even considered
  for a different account (see "Bugs found" below - this was corrected
  after native verification caught it suggesting a profile across
  accounts). `is_archived` added (see below).

## Bugs found and fixed during native verification

Both were caught by actually exercising the feature through real IPC against
a live database, not by code review (a third, the unlabeled mapping
selects, is documented separately below since it surfaced during the
accessibility pass rather than the functional one):

1. **Cross-account profile recognition.** The original `recognize()` only
   used account scope as a _tie-breaker_ among ambiguous matches, not a hard
   filter - so a profile explicitly scoped to Account A was still suggested
   as a "clear" match for Account B whenever it was the only
   header-compatible profile in existence. A profile's `account_id` is the
   user's own explicit statement that the mapping is for that account only;
   suggesting it elsewhere is exactly the "weak signal" section 8 says must
   never be acted on. Fixed by filtering to profiles usable for the target
   account (global, or scoped to it) _before_ checking header compatibility.
   Regression test: `recognize_never_suggests_a_profile_scoped_to_a_different_account`.
2. **Undeletable used profiles.** `import_profiles` had no `is_archived`
   column. The moment a profile is used for even one import,
   `imports.import_profile_id`'s foreign key makes `delete_import_profile`
   permanently fail - discovered when my own test cleanup couldn't remove a
   profile it had already used. Added `is_archived` (migration 0009,
   amended before it was ever committed - see "Migration notes" below),
   `set_archived`, and `list(include_archived)`, matching the exact pattern
   every other archivable model in this codebase already uses
   (`accounts`, `payees`). Regression tests:
   `set_archived_toggles_and_is_excluded_from_list_by_default`,
   `a_profile_used_by_an_import_cannot_be_deleted_but_can_be_archived`.

## Migration notes

Migration 0009 was amended twice before ever being committed or pushed (once
to add the three `imports` columns, once more to add
`import_profiles.is_archived` after the bug above was found) - both changes
happened before this migration reached git, so the "migrations are immutable
once committed/pushed" rule was never violated. Amending it _after_ it had
already been applied to this machine's local dev database did produce real
schema drift in that one local file (a live demonstration of exactly why the
rule exists); that database held zero real financial data (confirmed
repeatedly across sessions) and was rebuilt fresh by relaunching the app,
with every prior state backed up first. See the completion report's Risks
section for the one related environmental oddity found along the way.

## Provenance

A committed CSV transaction preserves: `source_import_id` (which import
produced it), `source_row_hash` (the existing exact-duplicate identity),
`external_transaction_id` (the source's own transaction ID, when the
mapping provides one), and `payee_id` (when deterministically resolved).
`raw_description` (already existed) remains the original source text
alongside the rule-normalized `merchant`. No CSV row is duplicated
wholesale into the transaction - only the fields the ledger already models.

## Frontend changes

- `CsvImportWizard.tsx`: a profile picker pre-selected by recognition
  ("Recognized as X - Map manually instead"), external-ID and
  invert-amount mapping controls, "Save this mapping as an import profile"
  (explicit opt-in, never automatic), a simplified 4-status preview
  (New/Duplicate/Possible Match/Needs Review) with per-row Include/Import
  anyway controls, and a richer result screen ("Review N" jumps to Needs
  Review). The CSV preview table's dates now go through the existing
  `formatDate` helper - the known raw-ISO-date gap from the v0.1.0 release
  notes is closed.
- `ImportHistoryDialog.tsx` (new): a compact card list (filename, account,
  counts, date) opening into a detail view with the import's transactions
  and an Undo flow that always states removable/protected counts before
  acting, matching the product handoff's own worked example.
- Reachable from Transactions via a new "Import History" button, reusing
  the existing `.account-dialog` pattern rather than a new sidebar route.

## A third bug found: unlabeled mapping selects

Live accessibility verification (walking every focusable element in the
map step and checking its computed accessible name) found that
`MappingRow`'s `<label>` and `<select>` were only ever visually associated
via CSS grid, never programmatically via `htmlFor`/`id` - a pre-existing
gap (the component was unchanged from before this batch), surfaced now
because this batch added a sixth call site (Transaction ID) and because
this was the first time the workflow was checked this way. Fixed by giving
each `MappingRow` instance a stable id via React's `useId()`. Re-verified
live: all 11 focusable controls in the map step now report a correct
accessible name, and the existing gold focus-visible outline (already
established, not touched) is intact.

## Explicit non-goals (unchanged from the handoff)

OFX/QFX/PDF parsing, Recurring Expected-vs-Posted matching, full
reconciliation UI, Payee Library UI, Insights/forecasting, mobile, CITADEL,
AI. `import_profiles.rs` still has no dedicated "manage profiles" page -
`set_archived`/`update`/`delete` exist on the backend for a future batch's
UI, matching the same accepted gap Payees already has.

## Acceptance criteria

- [x] Existing CSV import behavior (exact-hash idempotence, rule
      normalization, debit/credit sign handling) unchanged and re-verified.
- [x] `cargo fmt --check`, `cargo check`, `cargo clippy -- -D warnings`,
      `cargo test` all pass (228 tests: 201 existing + 27 new).
- [x] `cargo audit`, `npm audit`, Gitleaks, Trivy: no new findings.
- [x] `npm run check` (format, typecheck, tests, build) passes.
- [x] Migration 0009 applies cleanly to a fresh database and to a
      representative Foundation-Batch-1-shaped fixture (`db.rs` test).
- [x] Native verification: real `tauri dev` binary, real IPC, a full
      create-profile -> recognize -> import -> history -> restart ->
      idempotence -> protected-undo loop, twice (once before the two bugs
      above were found and fixed, once after on a fresh database).
- [x] 1280x800 and 1024x640 checked for the wizard's map step and Import
      History - no horizontal overflow (`scrollWidth <= innerWidth`).
- [x] Accessibility: every focusable control in the map step has a correct
      programmatic accessible name (verified live, not assumed); the
      pre-existing gold focus-visible outline is intact; status is
      conveyed by text labels, never color alone.

## Owner decisions

None required. The Strong-Match-goes-to-review-not-auto-skip choice and the
partial-undo behavior were both resolvable from the handoff's own explicit
safety directive (section 16) and worked example (section 24)
respectively.

## Hard-stop conditions

None encountered on the product side. One process-level pause: moving aside
an unexpected, unexplained second database file at the live app-data path
required explicit owner confirmation before proceeding (destructive-adjacent
operation on the owner's own local file, blocked once by the environment's
own safety classifier) - see the completion report.
