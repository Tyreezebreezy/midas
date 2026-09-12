# Phase 010: OFX/QFX source adapter + import engine hardening

- Status: COMPLETE
- Owner gate: APPROVED 2026-09-12 (owner handoff: "MIDAS Development Batch 3 - OFX/QFX Source Adapter + Import Engine Hardening")

## Goal

> OFX/QFX becomes another source feeding the exact same normalized MIDAS
> ingestion, matching, Review, provenance, history, and Undo systems already
> used by CSV.

Not a second ledger pipeline - one more producer of `NormalizedCandidateTransaction`
and one more caller of the shared `commit_staged`/`match_candidate`/history/undo
machinery Phase 009 built.

## Reconnaissance (before writing code)

**Parser dependency validation.** The handoff preferred `ofx-rs 0.2.x` but
required validating it first. `cargo add ofx-rs@0.2.0` resolved a real
dependency tree (`cargo tree`, not the misleading `cargo tree --target all`
which speculatively resolves every platform including wasm32 and surfaced
unrelated `reqwest`/`rand`/`wasm-bindgen` noise) of `quick-xml 0.39.4` /
`rust_decimal` / `time`. `cargo audit` found two new HIGH (7.5) advisories on
that pinned `quick-xml` version: RUSTSEC-2026-0194 (quadratic-runtime DoS)
and RUSTSEC-2026-0195 (unbounded-memory DoS), both fixed only in
`quick-xml >= 0.41.0`. `ofx-rs 0.2.0`'s own `Cargo.toml` hard-pins
`quick-xml = "^0.39.2"` - confirmed unfixable by attempting
`cargo update -p quick-xml@0.39.4 --precise 0.41.0`, which failed to select a
version. crates.io/GitHub also showed the crate itself is thin (two releases
ever, both from March, three GitHub stars, no commits since). Per the
handoff's own pre-authorized fallback ("if `ofx-rs` proves concretely
incompatible, a hand-rolled parser is acceptable"), `ofx-rs` was fully
reverted (`cargo remove`, confirmed zero `Cargo.toml`/`Cargo.lock` diff) and
`quick-xml 0.41.0` added directly instead - `cargo add quick-xml@0.41.0`
added exactly one line to `midas`'s own dependency list with **zero new
package entries** in `Cargo.lock`, since it was already resolved transitively
through Tauri's own `plist`/`tauri-utils` chain. Net dependency change for
this entire batch: one version bump on an already-present transitive crate,
now a direct one.

**Format internals.** OFX 1.x is SGML: a leaf tag has no closing tag
(`<TRNAMT>-4.50` runs to the next `<`). OFX 2.x and QFX's XML variant are
well-formed XML using the identical tag vocabulary
(`STMTTRNRS`/`STMTRS`/`CCSTMTRS`/`BANKACCTFROM`/`CCACCTFROM`/`BANKTRANLIST`/
`STMTTRN`/`LEDGERBAL`/`INVSTMTMSGSRSV1`). One normalization
(`sgml_to_xml`, character-scanning tokenizer that synthesizes closing tags for
leaf values) turns both eras into the same shape before a single `quick_xml`
event-loop walker (`extract`) handles both - no format-specific extraction
path, no format terminology anywhere above `ofx.rs`.

## Architecture

```
CSV ──────┐
          │
OFX/QFX ──┼──> NormalizedCandidateTransaction
          │                │
          │          match_candidate (New/ExactDuplicate/StrongMatch/PossibleMatch)
          │                │
          │          Rules / Payees
          │                │
          │              Review
          │                │
          └──────────── commit_staged ──> Ledger
                               │
                        Import History / Undo
```

`ofx.rs` owns structural parsing only (SGML/XML -> `ParsedOfx`, a
MIDAS-agnostic list of statements/accounts/raw transaction fields) - it knows
nothing about matching, sign conventions, or MIDAS accounts, exactly
mirroring how CSV's `read_csv`/`detect_mapping` stay separate from
`build_staging_rows`. `import.rs` gained an OFX section
(`build_ofx_staging_rows`, `preview_ofx_content`, `stage_ofx_content`,
`commit_ofx`) that turns `ParsedOfx` into the same
`NormalizedCandidateTransaction` shape CSV produces and calls the exact same
`match_candidate`/`commit_staged` functions - `commit()` was split into
CSV-specific parsing plus a new shared `commit_staged` that both adapters
call, rather than either duplicating the commit loop or introducing a
`SourceAdapter` trait for its two implementors. `ofx_accounts.rs` is a small,
separate concept from `import_profiles.rs`: profiles describe how to
interpret CSV _columns_; this remembers which MIDAS account a _structured_
source account (bank id + account number/type) feeds, keyed by identity, not
by file shape.

## Backend changes

- **`ofx.rs`** (new): `parse(content) -> Result<ParsedOfx, OfxError>`.
  `SourceAccount { bank_id, source_account_id, source_account_type, kind,
currency }`, `OfxTransaction { fitid, trntype, dtposted, dtuser, trnamt,
name, memo, checknum }` - deliberately only the fields product handoff
  section 8 names as used, not the full OFX field set. Investment statements
  (`INVSTMTMSGSRSV1`/`INVSTMTTRNRS`/`INVSTMTRS`) are detected and skipped,
  never partially imported - `OfxError::InvestmentOnly` when that's the only
  content, `has_investment_statement` flagged when it sits alongside a
  supported statement so the supported ones still import cleanly.
- **`ofx_accounts.rs`** (new): `find_mapping`/`remember`, an upsert keyed on
  a COALESCE-normalized `(bank_id, source_account_id, source_account_type)`
  so a credit card with no bank id or type is still a distinct identity from
  another, and the same account number at two different banks never
  collides (migration 0010's `ofx_account_mappings` table).
- **`import.rs`** OFX section: `parse_ofx_date` extracts the `YYYYMMDD`
  prefix and nothing else - a trailing timezone suffix
  (`20260623120000[-5:EST]`) is ignored outright rather than converted,
  because converting it is exactly the day-shift bug the handoff warned
  against. `resolve_ofx_dates` prefers `DTUSER` over `DTPOSTED` when both are
  present and differ (the user-corrected date over the bank's posting date).
  `merge_name_and_memo` builds the description: NAME primary, MEMO appended
  via " - " unless one is empty or the two are redundant. OFX's `TRNAMT` is
  already signed relative to the account's own balance - the same model
  ADR-0004 already uses for MIDAS's own ledger - so no separate OFX sign
  convention exists; only `TRNTYPE == "XFER"` overrides the existing
  `default_transaction_type` heuristic.
- **Migration 0010** (`ofx_account_mappings`, `imports.statement_balance_cents`,
  `imports.statement_balance_date`): new file, since 0008/0009 are immutable
  once committed. The statement balance columns are minimal metadata
  preservation only (product handoff section 30) - MIDAS's own computed
  balance (`SUM(transactions.amount_cents)`, ADR-0007) remains the only
  source of truth; the file's stated balance is never used to override it.
- **Account-mapping UI flow**: preselects a remembered mapping
  (`existing_mapping_account_id`), never guesses when unmapped (defaults to
  asking), and "Remember this account mapping" is an explicit opt-in
  checkbox, matching the product handoff's "never weakly guess which account
  should receive financial transactions" (section 15).
- **Masked account identifiers**: `mask_source_account_id` shows only the
  last 4 characters (`•••• 2223`) anywhere a source account number would
  otherwise appear in the UI.

## Bugs found and fixed during native verification

All three were caught by actually driving the real `tauri dev` binary
through real WebView2 CDP and real IPC with sanitized fixture files - not by
code review or the Rust unit-test suite, which had already reached 256
passing tests without catching the first (and most serious) of these:

1. **The mandatory FITID non-collapse rule had a live, uncaught hole.**
   `match_candidate`'s ledger-side hash check already carried each matched
   row's external ID and refused to call two rows an `ExactDuplicate` when
   their external IDs explicitly disagreed - proven by an existing unit
   test. But a _second_, separate hash check exists for a row repeated
   within the same import batch (`batch_hashes`, originally added so a CSV
   file that genuinely repeats a row twice is still caught even though
   neither occurrence is in the ledger yet) - and that check compared only
   the text hash, with no external-ID awareness at all. Native testing
   proved this the hard way: importing a single OFX file with two real,
   legitimate same-day, same-amount coffee purchases distinguished only by
   FITID showed the second one as `DUPLICATE` in the live preview table,
   silently dropping a real transaction on commit - exactly the outcome
   section 22 calls mandatory to prevent. Fixed by giving `batch_hashes` the
   same shape and the same conflict check as the ledger-side map (hash ->
   that row's own external id, `HashMap<String, Option<String>>` - the
   `usize` row-index it previously stored was dead weight, never read
   anywhere, and dropped along with the fix). Two legitimate rows with
   different FITIDs now both stage as `New`; a CSV file with no external-ID
   column at all (both sides `None`) is completely unaffected and still
   catches a genuine repeated-row export bug exactly as before - reverified
   live with a small CSV fixture containing two byte-identical rows.
   Regression tests: `ofx_two_brand_new_same_batch_rows_with_different_fitids_both_stage_as_new`
   (new), plus the pre-existing `ofx_same_amount_and_date_with_different_fitid_are_not_collapsed`
   which only ever covered the ledger-side path.
2. **A multi-statement OFX file could strand the user mid-import.** When an
   earlier statement in a multi-account file resolved to zero new
   transactions (everything already imported), the Preview step's Import
   button is correctly disabled at `willImportCount === 0` - but there was
   no other way to reach the next statement, since advancing was only wired
   from the post-commit Result step. Fixed by adding a "Next account" action
   on the Preview step itself whenever more statements remain and nothing
   in the current one needs importing, reusing the exact same
   `continueToNextStatement` the Result step already called. Found and
   fixed live via Vite HMR against the running native app; re-verified the
   full two-statement (checking + credit card) file end to end afterward.
3. **Minor: singular/plural.** The map step's transaction count read "1
   transactions" for a single-transaction statement. Fixed inline
   alongside finding #2.

## Migration notes

Migration 0010 is new (0008/0009 were committed and pushed in prior batches
and were never touched). `db.rs`/`backup.rs` and all 16 `test_conn()`
fixtures across the backend bumped their schema-version guard from 9 to 10
and gained the new `include_str!`; a dedicated `db.rs` test applies 0010 onto
a Foundation-Batch-1-shaped fixture and confirms both new `imports` columns
default to `NULL` and `ofx_account_mappings` starts empty.

## Provenance

A committed OFX/QFX transaction preserves `external_transaction_id` (the
FITID - OFX's strongest identity signal), `source_row_hash`,
`source_import_id`, and the merged NAME/MEMO description as
`raw_description`. No complete OFX document is ever stored - only the
individual fields the ledger already models, same discipline as CSV.
Per-file privacy rules were followed throughout: nothing logs an account
number, a full OFX/QFX document, or a transaction description/amount/name.

## Undo-safety audit (product handoff section 26)

Re-examined whether "untouched since import" (`created_at == updated_at`)
actually proves nothing meaningful was attached, per the handoff's explicit
warning that it does not necessarily prove that. It didn't:
`tags::attach()` inserts a `transaction_tags` join row without ever bumping
the transaction's own `updated_at`, so a tagged transaction from an import
would have been silently deleted by Undo. Hardened
`removable_transaction_ids` to also require no `transaction_tags` row and no
`transfer_pair_transaction_id`/`refund_of_transaction_id` reference in either
direction (defensive - nothing currently writes these, but they're exactly
named in the spec and cheap to check). Verified live end-to-end, not just by
unit test: tagged one committed transaction via real IPC, ran the real Undo
flow through the UI, and confirmed the confirmation banner correctly read "1
were modified after import and will remain" before confirming - the tagged
transaction survived untouched and the untagged sibling was fully removed.
Regression tests: `undo_protects_a_tagged_transaction_even_though_tagging_never_touches_updated_at`,
`undo_protects_a_transaction_referenced_as_a_transfer_pair_or_refund_target`.

## Frontend changes

- **`CsvImportWizard.tsx` renamed to `ImportWizard.tsx`** - a deliberate,
  justified rename (not churn): the component stopped being CSV-specific,
  and the handoff explicitly warns against format terminology leaking into
  normal UI. The "Import CSV" button on Transactions is now "Import."
- A file-type choice (CSV / OFX-QFX) opens the wizard; CSV's entire existing
  select/map/preview/result flow is unchanged. OFX/QFX gets its own map
  step: masked source account, kind, transaction count, a MIDAS-account
  picker (preselected on a remembered mapping, otherwise defaulting to
  asking), and the "Remember this account mapping" checkbox. Preview and
  Result are the same shared components CSV uses - `StagingResult` and
  `ImportSummary` are identical shapes regardless of source.
- Multi-statement files loop through each source account through the exact
  same map/preview/commit steps, with per-statement summaries combined into
  one running total shown at the end (`combineSummaries`).
- **`ImportHistoryDialog.tsx`**: both the list and the detail view now show
  each import's source type (CSV/OFX/QFX) alongside filename/account/counts.

## Sanitized test fixtures

`tests/fixtures/ofx/` (10 files, all synthetic - fake account numbers,
"SAMPLE ..." merchant names, no real banking data): OFX 1.x SGML and OFX 2.x
XML checking statements with identical underlying data (proves cross-era
equivalence), a standalone QFX credit card statement, an overlapping-
statement re-import (2 already-imported FITIDs + 1 new), the mandatory
same-amount/different-FITID case, NAME-only/MEMO-only/NAME+MEMO+CHECKNUM
description variants, three date-parsing edge cases (date-only, a timezone
suffix, DTUSER differing from DTPOSTED), a two-source-account file (checking

- credit card), an investment-only file, and a deliberately malformed file.

## Explicit non-goals (unchanged from the handoff)

Investment/brokerage statements, PDF/SimpleFIN/AI ingestion, a
`SourceAdapter` trait or plugin registry, currency conversion (a
non-account-currency OFX file fails clearly rather than converting), a
dedicated "manage import profiles" page beyond the backend support that
already existed from Phase 009.

## Acceptance criteria

- [x] `cargo fmt --check`, `cargo check`, `cargo clippy -- -D warnings`,
      `cargo test` all pass (256 tests: 255 existing + regression tests for
      both bugs found this batch).
- [x] `cargo audit`: no new findings; net dependency change is one version
      bump on an already-transitively-present crate.
- [x] `npm run check` (format, typecheck, tests, build) passes.
- [x] Migration 0010 applies cleanly to a fresh database and to a
      Foundation-Batch-1-shaped fixture.
- [x] Mandatory idempotence requirements live-verified against the real
      app, not just unit tests: exact re-import of an unchanged statement
      (0 new / all duplicate), an overlapping statement (only the genuinely
      new rows commit), and the same-amount/different-FITID case (both
      import as new) - the last of which caught bug #1 above.
- [x] Cross-era equivalence live-verified: OFX 1.x SGML and OFX 2.x XML
      encoding the same transactions are recognized as the same duplicates.
- [x] Investment-only and malformed files both fail with a clear message
      and commit nothing, live-verified.
- [x] Undo-safety hardening live-verified end-to-end (tag a transaction via
      real IPC, real Undo flow through the UI, tagged row survives).
- [x] Account-mapping remember/recognize flow live-verified across two
      separate files sharing one source account identity.
- [x] CSV regression re-verified live after the shared-matcher fix: two
      byte-identical CSV rows with no external ID still collapse to one
      duplicate, unchanged from Phase 009 behavior.
- [x] Accessibility: every focusable control across the new OFX select/map/
      preview steps has a correct programmatic accessible name (verified
      live), gold focus-visible outline intact.
- [x] 1280x800 and 1024x640 checked live for the OFX map and preview steps -
      no horizontal overflow.
- [x] Import History displays source type per import, live-verified.
- [x] All native-verification test accounts/transactions archived (not
      left as clutter) once verification completed.

## Owner decisions

None required. The parser-dependency fallback was pre-authorized in the
handoff itself (section 5); the decision to use it was a factual finding
(unfixable RUSTSEC advisories), not a judgment call needing owner input.

## Hard-stop conditions

None encountered. No destructive or irreversible action was taken against
the live application-data database at any point this batch - all
verification ran against the same real, continuing app-data database
Batch 2 already established, using synthetic accounts explicitly archived
afterward.
