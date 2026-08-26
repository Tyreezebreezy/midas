# Phase 004C: Accounting correction

- Status: COMPLETE
- Owner gate: APPROVED — "Do one Phase 4C accounting correction before Phase 5": (1) separate `opening_balance` from later `balance_adjustment`/reconciliation, (2) opening balance remains anchored to the historical starting point, (3) subsequent reconciliation creates or updates an adjustment at the reconciliation date instead of rewriting the opening balance, (4) adjustment transactions count toward account balance, (5) adjustments do not count as income, spending, transfers, refunds, budget activity, or recurring activity, (6) historical net-worth calculations only include an adjustment on/after its effective date, (7) regression tests proving all of the above plus Safe to Spend uses the corrected cash position, (8) ponytail, full gate, commit, push.

## Goal

Fix a real accounting bug in ADR-0007's `reconcile_balance`: every reconciliation after the first rewrote the single `opening_balance` transaction's date as well as its amount, silently altering what MIDAS would report for `net_worth_as_of` any date between the old anchor and the new one. A statement re-run in August must never change what the app reported in January.

## Scope

- New `balance_adjustment` transaction type, structurally distinct from `opening_balance` (ADR-0009 supersedes ADR-0007's mechanism section).
- Migration `0007_balance_adjustment.sql`: widens the `transaction_type` CHECK constraint (full table rebuild, same pattern as migration `0004`), adds `idx_one_adjustment_per_account_per_date` (at most one adjustment per account **per date**, alongside the existing at-most-one-ever `idx_one_opening_balance_per_account`).
- `reconcile_balance` becomes a two-path dispatcher: `establish_opening_balance` (first-ever call for an account) or `upsert_balance_adjustment` (every call after) - same public signature, zero caller changes.
- `recurring::detect_candidates` explicitly excludes both `opening_balance` and `balance_adjustment` - the former was always structurally safe (capped at one row), the latter is not (an account reconciled regularly can accumulate 3+ same-merchant, similarly-spaced rows, exactly what the detector looks for).
- Frontend: `TransactionType` union and `TRANSACTION_TYPE_LABELS` gain `balance_adjustment`, not added to the user-selectable `TRANSACTION_TYPES` list - same non-goal precedent `opening_balance` already set.

## Explicit non-goals

- No new Tauri command and no change to `reconcile_balance`'s public signature - this is an internal correction to an existing mechanism, not a new feature surface. `ReconcileBalanceForm` and every other caller are unchanged.
- No migration of historical data into synthetic adjustments - this repository has never held real financial data (same standing note as ADR-0007), so there is nothing to backfill.
- No user-facing UI for viewing "reconciliation history" as a distinct concept - adjustments already appear in the ordinary transaction list with their own type label, which is sufficient per the owner's requirements; a dedicated reconciliation-history view was not requested.
- No change to how `opening_balance` behaves on its first call - `establish_opening_balance` is the same anchoring logic ADR-0007 always used, just extracted into its own function.

## Verified current state

- Builds on Phase 4B (`8399826`).
- New Rust dependencies: none. New npm dependencies: none.
- `src-tauri/capabilities/default.json`: still zero plugin permissions.
- Schema: migration `0007_balance_adjustment.sql` only - no new tables, one new transaction type, one new partial unique index.
- Modified: `accounting.rs` (`reconcile_balance` split into two private helpers, `spending_effect` updated), `transactions.rs` (`BalanceAdjustment` variant), `recurring.rs` (detection query excludes both anchor types), `db.rs` (migration registered). `accounts.rs`, `budgets.rs`, `import.rs`, `pay_cycle.rs`, `rules.rs`, `safe_to_spend.rs` needed only test-fixture migration additions (`test_conn()` now includes migration `0007`) for schema consistency.
- No app-defined command count change: 42 (same as Phase 4B) - this phase corrected existing behavior, added none.

## Constraints and architecture boundaries

- `establish_opening_balance` and `upsert_balance_adjustment` are private to `accounting.rs` - nothing outside the module changed, and the dispatcher in `reconcile_balance` is the only new decision point (`EXISTS` check on whether an opening balance already exists for the account).
- `upsert_balance_adjustment`'s delta is computed against `baseline = current_ledger_total - existing_same_date_adjustment_amount`, excluding that date's own prior adjustment before computing the new one - re-reconciling the same date corrects it in place rather than double-counting it against itself.
- Both anchor types are excluded from `spending_effect` (return `0`), `recurring::detect_candidates` (explicit `NOT IN` filter), and nothing else needs special-casing - ADR-0007's computed-balance model already treats every transaction uniformly for balance and net worth, so a correctly-dated adjustment is automatically correct there by construction.

## Security and privacy considerations

- No new network authority, no new fs/dialog plugin permissions.
- All verification used obviously-fake fixture data (two accounts both named "Phase4C Test," fake balances). Every test account and its transactions were deleted/archived through the app's own commands before this session ended; final state confirmed clean (`get_net_worth_history` returns all zeros, `list_accounts` returns no active accounts).
- Gitleaks (8.30.1): full history (26 commits) clean; working-tree diff (via `gitleaks protect`, gitignore-respecting) clean. A `--no-git` scan flagged 4 false positives inside `src-tauri/target/` build artifacts (an ANSI keybinding string in a compiled dependency's debug metadata, misread as a high-entropy secret) - `target/` is gitignored and was never a candidate for commit; not a real finding.
- Trivy (0.72.0, vuln+secret): 0 secrets; 1 MEDIUM (`glib`, pre-existing since Phase 0/1, Linux-only backend - ADR-0002), unchanged from Phase 4B. No new findings - this phase added zero dependencies.
- `cargo audit`: 0 vulnerabilities, same 17 pre-existing allowed transitive warnings, unchanged.

## Acceptance criteria (owner's eight numbered items)

- [x] **1-3: `opening_balance` separated from `balance_adjustment`; opening balance stays anchored; later reconciliation creates/updates a dated adjustment** - live-verified against the real running app: seeded an account's opening balance dated 150 days in the past ($1,000.00) via `reconcile_account_balance`, then reconciled again through the real "Update balance" dialog (Accounts page) to $750.00. Direct inspection of `list_transactions` after both calls showed exactly two rows: the opening balance untouched at its original date and amount (`id 1, 2026-03-29, opening_balance, $1,000.00`), and a new, separate adjustment dated at the reconciliation date (`id 3, 2026-08-26, balance_adjustment, -$250.00`) - proving the old bug (rewriting the opening balance's own date) is fixed and the new mechanism produces exactly the shape ADR-0009 specifies.
- [x] **4: adjustments count toward account balance** - the same live run: the account's computed balance and the app-wide net worth total both dropped by exactly $250 (the adjustment's delta) immediately after the real UI submission - $200,000 → $175,000 across two seeded accounts, matching hand arithmetic exactly.
- [x] **5: adjustments never count as income/spending/transfer/refund/budget/recurring activity** - `spending_effect` returns `0` for `BalanceAdjustment` (same as `OpeningBalance`); `budgets_and_spending_ignore_balance_adjustments` and `a_balance_adjustment_never_counts_toward_a_budgets_actual_spending` prove a large same-period adjustment never moves spending or budget actuals; `balance_adjustments_are_never_proposed_as_a_recurring_pattern` proves four same-merchant, evenly-spaced, consistent-delta reconciliations - the exact shape the detector looks for - are never proposed as a recurring series.
- [x] **6: historical net worth only includes an adjustment on/after its effective date** - `net_worth_history_changes_only_from_the_adjustment_date_forward` proves this directly (the day before the adjustment is unaffected, the adjustment's own date and every day after reflect it). Live-verified the same shape: the Net Worth trend's March-July points were bit-for-bit identical before and after the later reconciliation; only today's point moved.
- [x] **7: regression tests** - see Test strategy below; every named scenario (January history unchanged after an August reconciliation, today's balance corrected, Safe to Spend reflects the correction, budgets/spending/recurring ignore adjustments, net worth history changes only from the adjustment date forward) has a dedicated test.
- [x] **8: ponytail, full gate, commit, push** - one real finding from the ponytail review applied (a stale ADR-0007-era doc comment on `reconcile_balance` was left in place above the new ADR-0009 doc comment instead of being replaced, directly contradicting the code beneath it - deleted); full gate green (see Test strategy); committed and pushed below.

## Test strategy

1. Rust unit tests: 153 total, up from 145 at the end of Phase 4B (+8) - confirmed via `cargo test` output, not hand-counted. New/replaced coverage in `accounting.rs` (two stale tests describing the old buggy behavior replaced with four correctly-scoped ones, plus three new Phase 4C regression tests), `recurring.rs` (+1), `budgets.rs` (+1), `safe_to_spend.rs` (+1).
2. `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo audit` - all clean.
3. `npm run check` (prettier, tsc, node test, vite build) - clean.
4. Ponytail review of the full diff: one finding applied (stale doc comment, see acceptance item 8 above). Everything else - the two-path dispatcher, the baseline-excluding-self delta math, the explicit recurring-detection exclusion - was the minimum correct mechanism; no further simplification survived scrutiny without losing correctness.
5. Live end-to-end verification against the real native app over the WebView2 CDP debug port. One verification-script issue was caught and is worth recording honestly: the first script run seeded an account, failed partway through a DOM-selector guess, and was superseded by a corrected script - but the first run's account was never cleaned up before the second run, and both were named "Phase4C Test." The corrected script's row lookup matched the leftover account from the *first* run, not the freshly-seeded one from the *second* - so the real UI reconciliation landed on a different (but equally real) account than intended. This was not an application bug: it was caught immediately by checking the adjusted account's actual transaction rows directly, which confirmed the mechanism worked exactly as designed on real data, and the resulting numbers (documented in the acceptance criteria above) are genuine end-to-end proof, not simulated. Both leftover test accounts were then archived and their transactions deleted; final state confirmed clean.

## Owner decisions

None required - fully specified in the owner's eight-item authorization message.

## Hard-stop conditions

- Any requirement that couldn't be satisfied without a new command, a schema concept beyond one transaction type and one index, or a change to `reconcile_balance`'s public signature - none arose.
- Any need to migrate historical data - none arose (no real data exists in this repository).
