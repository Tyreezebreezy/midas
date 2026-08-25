# Phase 002: Transactions and CSV import pipeline

- Status: COMPLETE
- Owner gate: APPROVED — explicitly directed by the owner after Phase 1 acceptance ("Proceed directly into Phase 2... Do not pause for routine implementation decisions already covered by the MIDAS handoffs").

## Goal

A trustworthy transaction-ingestion foundation: manual transaction entry, and a staged CSV import pipeline (source -> staging -> validation -> normalization -> duplicate analysis -> commit -> Needs Review / canonical ledger) that never writes unvalidated data to the ledger and never silently duplicates a re-imported statement.

## Scope

- Canonical transaction model and one documented sign convention (ADR-0004), applied uniformly across manual entry, CSV import, and every account type.
- Manual transaction entry, edit, archive, and a data-integrity-justified delete policy (ADR-only-for-manual-entries, imports are archive-only).
- CSV file access via a plain native file input - zero new Tauri capabilities (ADR-0005).
- Staged CSV import: column-mapping auto-detection with user override, preview, three-tier duplicate classification, transactional commit with rollback, Needs Review routing for anything unresolved.
- Import/source metadata (`imports` table) for traceability.
- Needs Review foundation: malformed rows and suspected duplicates, with what was detected, why, the original data, and resolve/dismiss actions.
- A lightweight, non-enforced suggested category list (free text underneath).
- Transaction UI: dense list, search/account/category/date filters, clear non-color-alone positive/negative representation.
- Import UI: Select -> Map -> Preview -> Result, full visibility before commit.

## Explicit non-goals (per the owner's own hedges in this phase's instructions)

- Split transactions (schema/UI) - the instruction's own "where implementation remains clean" hedge; building it cleanly needs a parent/child table and sum-validation across import, dedup, and UI at once, which is real Phase-3-scale work, not a Phase 2 add-on.
- Automatic transfer-pair matching and automatic refund-to-purchase linking - schema carries the relationship (`transfer_pair_transaction_id`, `refund_of_transaction_id`), both nullable and unpopulated by any Phase 2 code path. The instruction explicitly warned against "claiming automatic transfer matching is complete unless it actually is."
- A rule engine or elaborate reusable-import-profile framework - category assignment is a free-text field with a suggested list; deterministic merchant normalization and rules are explicitly Phase 3 (product handoff build order).
- AI-assisted resolution of any kind, anywhere in Needs Review.
- A `tauri-plugin-dialog`/`tauri-plugin-fs` capability grant - not needed at all (ADR-0005).

## Verified current state

- Builds on Phase 2 checkpoint 1 (`2004e3a`, backend) and checkpoint 2 (`be52eed`, UI).
- New Rust dependencies: `csv` (RFC4180 parsing - hand-rolling comma/quote escaping correctly for financial data was the real risk, not the dependency), `sha2` (deterministic row fingerprinting - chosen over `std::collections::hash_map::DefaultHasher` specifically because std's own docs do not guarantee its algorithm stays stable across Rust releases, and a duplicate-detection fingerprint needs to stay valid for years of accumulated import history; SHA-256 is deliberately more dependency than the bare minimum for this one reason), `serde_json` (import/review metadata serialization).
- New npm dependencies: none.
- `src-tauri/capabilities/default.json`: still zero plugin permissions. 11 app-defined commands now total (6 from Phase 1 + 5 new: `preview_csv`, `stage_import`, `commit_import`, `list_needs_review`, `resolve_needs_review`; `list_transactions`/`create_transaction`/`update_transaction`/`set_transaction_archived`/`delete_transaction` bring the transaction CRUD total to 5 as well - 16 commands, 0 plugins).

## Constraints and architecture boundaries

- `stage`/`commit` in `import.rs` are plain functions taking `&Connection`/`&mut Connection` - testable without Tauri, same pattern as every other domain module. Thin `#[tauri::command]` wrappers only lock the mutex and delegate.
- Staging is stateless: `commit` re-parses the same CSV content rather than trusting a previously-returned staging result from the frontend, so staging and commit can never drift apart, and there is no staging table to keep consistent with the ledger.
- The canonical transaction table is never written to before the commit step - `stage`/`preview_csv` only read.

## Security and privacy considerations

- No new network authority, no new fs/dialog plugin permissions (ADR-0005) - CSV import needed _less_ native authority than the build handoff anticipated, not more.
- All verification used obviously-fake fixture data (e.g. "COFFEE SHOP #4471", "Phase2 UI Test Checking"); no real financial data touched this repository or any dev database. Dev databases were deleted after each verification pass.
- Gitleaks (8.30.1, full history, 7 commits): no leaks found.
- Trivy (0.72.0, vuln+secret): 0 secrets; 1 MEDIUM (`glib`, pre-existing since Phase 0/1, Linux-only backend, does not compile into the Windows binary - see ADR-0002/Phase 1 spec). No new findings from `csv`/`sha2`/`serde_json`.
- `cargo audit`: 0 vulnerabilities, same 17 pre-existing "unmaintained" warnings as Phase 1 (all transitive through Tauri's own dependency graph), no new ones.

## Acceptance criteria (owner's 15-item completion list)

- [x] Open MIDAS.
- [x] Create/use an account from Phase 1.
- [x] Add a transaction manually.
- [x] Select a CSV through a narrow user-initiated native file flow (zero new capabilities - ADR-0005).
- [x] Map unfamiliar columns (auto-detection + live-verified manual override of the mapping dropdowns).
- [x] Preview parsed transactions.
- [x] See validation failures before import (malformed amount shown as "Invalid" in the preview table, with the parse error, before any commit).
- [x] See duplicate warnings/protection (probable duplicate correctly flagged in preview; committing without forcing it sends it to Needs Review instead of the ledger).
- [x] Commit valid transactions.
- [x] Reimport the same source without duplicating history (live-verified: re-importing the identical file shows 0 ready / 3 exact duplicates, commit button correctly disabled since there is nothing to commit).
- [x] See problematic records in Needs Review (malformed row and suspected duplicate both present, each with its detected reason and original source data).
- [x] Search/filter the resulting transaction ledger (live-verified: search "coffee" correctly narrows to the one matching row).
- [x] Edit/correct transaction information (live-verified: editing a category persists correctly, and `raw_description` is preserved unchanged - only `merchant` is user-edited).
- [x] Correctly represent transfers, credit-card payments, and refunds at the model level (`transactions::tests::credit_card_purchase_is_positive_and_still_classified_as_spending`, `credit_card_payment_is_modeled_as_transfer_not_a_second_expense`, `refund_is_not_income_by_default_classification`).
- [x] Restart MIDAS and see the persisted ledger intact (live-verified: full process kill + relaunch, account/4 transactions/2 Needs Review items/the edited category all confirmed present afterward).

## Test strategy

1. Rust unit tests: 55 total (14 after Phase 1; +41 in Phase 2), covering money/date parsing edge cases, all 9 account types x transaction creation, transfer/refund/credit-card accounting classification, search/date-range filtering, the delete-policy split (manual vs. imported), and the full import pipeline (mapping detection, signed-amount and debit/credit parsing by account-type, exact/probable/legitimate-similar duplicate classification, successful commit, rollback mechanism, exact re-import idempotence, forced-import override, invalid rows to Needs Review).
2. `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo audit` - all clean.
3. `npm run check` (prettier, tsc, node test, vite build) - clean.
4. Real end-to-end verification against the live native app over the WebView2 CDP debug port (not mocked, not assumed from code reading): manual transaction entry through the actual dialog; CSV import through the actual wizard using `DOM.setFileInputFiles` to drive the real `<input type="file">` (no OS-dialog automation exists in this environment, so this exercises the identical code path a real file pick would - reading `file.text()` in the browser, then IPC to the real backend); exact re-import idempotence; a second file exercising the probable-duplicate path end-to-end (flagged, correctly excluded from commit, correctly landed in Needs Review); live search filtering; live transaction editing with `raw_description` preservation confirmed; and a full process restart with all data confirmed intact.

## Owner decisions

None required for this phase — explicitly pre-authorized to proceed autonomously per the owner's Phase 2 message.

## Hard-stop conditions

- Any need for a new Tauri plugin or broader filesystem authority - none arose (ADR-0005 achieved the opposite: less authority than anticipated).
- Any need for AI-assisted categorization or resolution - none built, none needed.
- Split transactions or automatic transfer/refund matching turning out to be required for Phase 2 correctness - they are not; the schema carries the relationship fields for a later phase to populate.
