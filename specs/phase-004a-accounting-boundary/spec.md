# Phase 004A: Accounting boundary hardening

- Status: COMPLETE
- Owner gate: APPROVED — "Proceed to Phase 4, but begin with the accounting-boundary hardening below before implementing budgets, Safe to Spend, or net worth," with an explicit required-case list (checking purchase, credit-card purchase, credit-card payment, net worth across all account types + inclusion toggle, opening/baseline balances) and an explicit instruction to reassess the Phase 3 date-arithmetic decision before extending it.

## Goal

Make ADR-0004's own stated invariant - "keeps `balance = sum(transactions)` a true invariant" - actually true, tested, and true for every account type uniformly; and give MIDAS a deterministic, auditable way to establish a trustworthy balance when only partial transaction history is available.

## Scope

- Account balance becomes computed live from the transaction ledger, never a separately-maintained stored value (ADR-0007).
- A new `opening_balance` transaction type: the single mechanism for both setting a new account's starting balance and correcting/reconciling an existing one later.
- `accounting::spending_effect`/`total_spending`: the shared, tested definition of what counts as spending (purchases add, refunds subtract, income/transfers/opening-balance never count) that Phase 4B's budgets will read rather than reimplementing.
- `accounting::net_worth`: included assets minus included liabilities, never a blind sum.
- Reassessment of ADR-0006 (hand-rolled date arithmetic): superseded by ADR-0008 (adopt `chrono::NaiveDate`, zero timezone/clock surface) now that Phase 4B's calendar needs exceed simple day-counting.
- Minimal UI surface to make the mechanism real and end-to-end testable: a "starting balance" field at account creation, and an "Update balance" reconcile dialog for existing accounts.

## Explicit non-goals

- A statement-by-statement or transaction-matching reconciliation subsystem - explicitly ruled out by the owner ("Avoid a large reconciliation/accounting subsystem at this stage"). One recomputed adjusting entry is the entire mechanism.
- Migrating any prior `accounts.balance_cents` value into a synthetic opening-balance transaction during the schema rebuild - this repository has never held real financial data (every phase's Security section records dev/test fixtures deleted after verification), so there was nothing real to preserve.
- Exposing `net_worth`/`total_spending` through a dedicated UI page - the owner's own Phase 4A text asks for these "with tests," while the actual Net Worth _view_ is explicitly Phase 4B's own bullet. Phase 4A ships them as real, tested, callable commands (avoiding dead code) without building the page that will consume them more fully in 4B.
- Editing an opening-balance transaction's date independently of its amount - reconciliation always recomputes the anchor date from the current ledger; there is no user-facing reason yet to decouple the two.

## Verified current state

- Builds on Phase 3 (`3d6b6ea`).
- New Rust dependency: `chrono` (`default-features = false, features = ["std"]`) - `NaiveDate` only, no timezone database, no OS clock bindings. `cargo tree -p chrono` shows exactly one transitive dependency (`num-traits`). See ADR-0008.
- New npm dependencies: none.
- `src-tauri/capabilities/default.json`: still zero plugin permissions.
- 26 app-defined commands total (23 from Phase 3 + 3 new: `reconcile_account_balance`, `get_net_worth`, `get_total_spending`).
- Schema: migration `0004_accounting_boundary.sql` widens `transactions.transaction_type` to include `'opening_balance'` (a table rebuild - SQLite has no `ALTER TABLE ... ALTER COLUMN` for CHECK constraints), adds a partial unique index enforcing at most one opening-balance row per account, and drops the now-superseded `accounts.balance_cents` stored column.
- New Rust module `accounting.rs` (spending/net-worth/reconcile domain logic + 3 commands), new module `dates.rs` (chrono-backed `parse_iso`/`days_between`/`add_days`/`is_valid_calendar_date` - deliberately scoped to only what Phase 4A calls; month-aware arithmetic is added when Phase 4B's Pay Cycle/Budgets first need it, not preemptively).
- `money.rs`'s hand-rolled `is_leap_year`/`days_in_month`/`valid_calendar_date` retired in favor of `dates::is_valid_calendar_date` (one `chrono` call replacing three hand-written functions). `recurring.rs`'s hand-rolled Howard Hinnant `days_from_civil`/`civil_from_days` retired in favor of `dates::days_between`/`add_days` - MIDAS now has exactly one calendar implementation, not two running in parallel.
- New frontend: `src/ReconcileBalanceForm.tsx`; `AccountForm.tsx`'s balance field is now create-only ("Starting balance, as of today"); `AccountsPage.tsx` gained an "Update balance" row action.

## Constraints and architecture boundaries

- `accounting::reconcile_balance` takes `&Connection`, testable via `Connection::open_in_memory()`, matching every other domain module's pure-function-plus-thin-command pattern.
- `Account.balance_cents` is a correlated subquery aliased into the existing `SELECT` (`row_to_account`'s `row.get("balance_cents")` is unchanged) rather than a second code path - `get()` and `list()` compute it identically, with no risk of the two disagreeing.
- `reconcile_balance` upserts a single row (create/update/delete based on whether a gap still exists) rather than ever accumulating history - re-running it with the same inputs is a no-op, and running it after the ledger becomes complete removes the now-unneeded anchor entirely.

## Security and privacy considerations

- No new network authority, no new fs/dialog plugin permissions - the accounting boundary is pure local computation over the existing ledger.
- All verification used obviously-fake fixture data ("Phase4A Checking," a $75 "Test grocery run" purchase); no real financial data touched this repository or any dev database. The test account was deleted/archived through the app's own commands immediately after verification.
- Gitleaks (8.30.1, full history + working tree, 12 commits): no leaks found.
- Trivy (0.72.0, vuln+secret): 0 secrets; 1 MEDIUM (`glib`, pre-existing since Phase 0/1, Linux-only backend - ADR-0002). No new findings from `chrono`/`num-traits`.
- `cargo audit`: 0 vulnerabilities, same 17 pre-existing "unmaintained"/"unsound" transitive warnings as Phase 3, no new ones - confirms the chrono addition stayed as minimal as ADR-0008 claimed.

## Acceptance criteria (owner's required cases, each verified against the live app)

- [x] Checking purchase reduces balance by the purchase amount and counts as spending - live-verified: a real $75.00 purchase through the actual Transaction form dropped a $500.00-opening-balance account to $425.00; `get_total_spending` confirmed $75.00.
- [x] Credit-card purchase increases amount owed and counts as spending - `accounting::tests::credit_card_purchase_increases_amount_owed_and_counts_as_spending`.
- [x] Credit-card payment reduces checking cash and credit-card liability, is a transfer, and does not double-count as spending - `accounting::tests::credit_card_payment_moves_cash_to_liability_without_double_counting_spending` (asserts both balances move correctly _and_ `total_spending` still reflects only the original charge).
- [x] Net worth = included assets − included liabilities, never a blind sum - `net_worth_sums_included_assets_minus_included_liabilities` (checking, savings, cash, property as assets; credit card, loan, other-liability as liabilities) and `net_worth_excludes_accounts_with_inclusion_toggled_off_and_archived_accounts` (adds investment, exercised via its `include_in_net_worth = false` exclusion, and an archived savings account).
- [x] Opening/baseline balances: deterministic, auditable, no spending/income masquerade, no double-counting, works for assets and liabilities - `reconcile_establishes_an_opening_balance_when_none_existed`, `reconcile_anchors_before_partial_imported_history_without_double_counting` (the literal "3 months imported, real balance known" scenario from the owner's own text), `reconcile_is_idempotent_and_updates_in_place_rather_than_accumulating`, `reconcile_removes_the_anchor_once_it_is_no_longer_needed`, `opening_balance_never_counts_as_spending`, `at_most_one_opening_balance_transaction_per_account_is_enforced_by_schema`, `works_symmetrically_for_a_liability_account`. Live-verified end-to-end: created an account with a $500 starting balance (real opening-balance transaction appeared with the correct amount and date), then reconciled to a corrected $430.00 real-world balance - the _same_ anchor updated in place (`anchorCount: 1`), not a duplicate.
- [x] Date-arithmetic decision reassessed, not automatically extended - ADR-0008 written; `chrono::NaiveDate` adopted with a verified single-transitive-dependency footprint; hand-rolled calendar code deleted rather than kept in parallel.

## Test strategy

1. Rust unit tests: 94 total (78 after Phase 3; +16 net: +14 in `accounting.rs`, +3 in `dates.rs`, +1 in `accounts.rs`, −2 redundant date-arithmetic tests removed from `recurring.rs` once `dates.rs` became the single implementation).
2. `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo audit` - all clean. (`--all-targets` matters here specifically: an earlier draft of `net_worth`/`total_spending` had only test callers and was correctly flagged as dead code in the non-test build until `get_net_worth`/`get_total_spending` commands gave them a real one.)
3. `npm run check` (prettier, tsc, node test, vite build) - clean.
4. Ponytail review: one finding applied (`accounting.rs`'s `reconcile_balance` ran two separate queries against the identical `WHERE` clause - merged into one `SELECT COALESCE(SUM(...),0), MIN(...)`, net −8 lines). Nothing else to cut.
5. Real end-to-end verification against the live native app over the WebView2 CDP debug port: created an account through the real "Add account" dialog with a $500.00 starting balance, confirmed the resulting `opening_balance` transaction via direct backend inspection; entered a real $75.00 purchase through the actual Transaction form and confirmed the displayed balance dropped to exactly $425.00; opened the real "Update balance" dialog (confirmed it correctly displays "MIDAS currently shows $425.00"), reconciled to $430.00, and confirmed via the backend that the same anchor transaction updated in place rather than a second one being created, and that `get_total_spending`/`get_net_worth` both computed correctly against the final state. Test account archived and its transactions deleted through the app's own commands afterward.

## Owner decisions

None required for this phase - fully specified in the owner's Phase 4A authorization message.

## Hard-stop conditions

- Any need for a statement-matching/multi-transaction reconciliation subsystem - none arose; the single-anchor mechanism covered every required case.
- Any need for `chrono`'s timezone-aware types - none arose and none are enabled; MIDAS still has no timezone concept.
- Migrating historical `balance_cents` data turning out to be necessary - it was not; no real data existed to migrate.
