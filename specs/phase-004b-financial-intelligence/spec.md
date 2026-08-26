# Phase 004B: Core financial intelligence

- Status: COMPLETE
- Owner gate: APPROVED — "After the accounting-boundary tests pass, implement: 1. Budgets 2. Pay Cycle 3. Safe to Spend 4. Credit-card safeguards 5. Savings Goals 6. Net Worth," each with a detailed sub-specification, plus an explicit closing instruction to "Proceed autonomously through Phase 4 within the existing MIDAS scope."

## Goal

Turn the Phase 4A accounting boundary into the features an owner actually checks day to day: what am I planning to spend by category, when do I get paid next, how much can I actually spend right now, am I on track toward a goal, and what am I worth.

## Scope

- **Budgets**: monthly category budgets, planned vs. actual, fixed/flexible classification, a fixed under/near/over status convention, and historical comparison against real spending in prior months (not just prior budgets).
- **Savings Goals**: name, target amount, current amount, optional target date, optional protected/reserved amount, computed progress and status.
- **Pay Cycle**: an editable pay schedule (weekly/biweekly anchored to a real payday, monthly/semimonthly by day-of-month), current cycle start, next payday, spending this cycle, obligations before payday, prior-cycle comparison, and income-pattern detection that only ever proposes.
- **Safe to Spend**: the headline number - eligible cash minus confirmed obligations before the next payday minus protected goal reservations minus the owner's required reserve - with every line item exposed and an honest incomplete state when no pay schedule exists yet.
- **Credit-card safeguards**: no dedicated feature was specified beyond what Phase 4A's accounting boundary and Safe to Spend's own requirements already establish (credit availability never counted as cash, liabilities handled correctly, no double-counting) - folded into those two rather than built as a separate, redundant mechanism. See Explicit non-goals.
- **Net Worth**: a current snapshot (total assets, total liabilities, net worth) built on Phase 4A's `accounting::net_worth`, enhanced into a full breakdown, plus a simple 6-month-and-today trend recomputed from the ledger.

## Explicit non-goals

- A separate "credit-card safeguards" feature or schema. The owner's own numbered list included it, but no sub-section in the authorization message gave it independent requirements - every bullet that named it ("credit availability is never cash," "liabilities are handled correctly," "credit-card payments do not double-count spending") is either a Phase 4A accounting-boundary guarantee already shipped and tested, or a Safe to Spend requirement this phase tests directly (`credit_card_debt_never_reduces_eligible_cash_or_safe_to_spend`). Building a third, separate mechanism for the same guarantee would be redundant, not thorough.
- Envelope budgeting (rollover, inter-category fund transfers) - explicitly ruled out by the owner ("Do not build an envelope-budgeting system unless required by the product").
- A stored, user-configurable warning threshold per budget - a fixed 90% convention, not a config field nobody asked to tune.
- A contribution-history ledger for savings goals - `current_amount_cents` is a plain field the owner updates directly, matching the plain field list the owner actually specified.
- A `credit_limit`/available-credit concept anywhere in the schema - "credit availability is never counted as cash" is satisfied by never computing an available-credit figure at all, not by computing one and then subtracting it.
- Net worth stored as periodic snapshots - recomputed from the ledger every time, so there is nothing to keep in sync (same principle as ADR-0007's computed account balance).
- Configurable trend length or reservation-per-goal edit history - "simple" trend and "current amount" field, as specified.

## Verified current state

- Builds on Phase 4A (`e2e6c24`).
- New Rust dependencies: none. New npm dependencies: none.
- `src-tauri/capabilities/default.json`: still zero plugin permissions.
- 42 app-defined commands total (26 from Phase 4A + 16 new: 4 budgets, 4 savings goals, 4 pay cycle, 3 Safe to Spend, 1 net worth history).
- Schema: migration `0005_budgets.sql` (`budgets` table, `UNIQUE(category, month)`) and `0006_savings_goals.sql` (`savings_goals` table). Pay Cycle's schedule and Safe to Spend's owner reserve both live in the pre-existing `settings` key-value table (`pay_schedule`, `safe_to_spend_reserve_cents`) - no new tables for either, since each is exactly one small structured/scalar value and `settings.rs` already exists for precisely this.
- New Rust modules: `budgets.rs`, `savings_goals.rs`, `pay_cycle.rs`, `safe_to_spend.rs`. `accounting.rs` gained `spending_in_range` (the shared "spending in a date range" primitive budgets, pay cycles, and Safe to Spend all read from) and `net_worth_breakdown`/`net_worth_as_of`/`net_worth_history` (replacing the Phase 4A `net_worth` function, which had no frontend consumer yet and so was safe to reshape).
- `dates.rs` gained `add_months`/`day_in_month` (re-added after being trimmed from Phase 4A for lack of a caller - Pay Cycle's month-based schedules and prior-cycle comparison are the real callers now).
- New frontend pages: `BudgetsPage.tsx`, `SavingsGoalsPage.tsx`, `PayCyclePage.tsx`, `SafeToSpendPage.tsx`, `NetWorthPage.tsx`. New CSS: `.goal-progress-track`/`.goal-progress-fill`, `.headline-number` - everything else reuses Phase 1-4A table/dialog/badge classes.

## Constraints and architecture boundaries

- Every module follows the established pure-function-plus-thin-command pattern; every money-in-a-date-range calculation composes `accounting::spending_in_range` rather than re-deriving spending classification.
- Safe to Spend never reimplements "obligations before payday" - it calls `pay_cycle::summarize` and reads `obligations_before_payday_cents` directly, so "known upcoming bills are deducted only once" is true by construction, not by two implementations happening to agree.
- Recurring-income detection is not a new algorithm - `pay_cycle::likely_income_series` filters `recurring::list`'s existing, already-tested output (positive amounts, not rejected). "User-defined values outrank inference" is enforced structurally: nothing computed from a detected pattern is ever written to the saved schedule without an explicit save action.
- `SafeToSpendResult` and `NetWorthPoint`/`NetWorthBreakdown` expose every line item, never just a total - the owner's "clear breakdown" and "useful snapshot" requirements read directly off these structs' fields, not derived client-side from a bare number.
- All calculation lives in Rust; the two headline pages (Safe to Spend, Net Worth) only format and lay out server-computed numbers, per the owner's explicit "keep the calculation in domain logic, not React components."

## Security and privacy considerations

- No new network authority, no new fs/dialog plugin permissions across all six features - everything is local computation over the existing ledger and two new small tables.
- All verification used obviously-fake fixture data (test account names prefixed `Phase4B`/`NW`/`STS`, merchants like "ACME Payroll" and "Insurance"); no real financial data touched this repository or any dev database. Every test account, transaction, recurring series, and savings goal created during live verification was deleted/rejected/archived through the app's own commands before each dev session ended.
- Gitleaks (8.30.1, full history + working tree, 23+ commits across the phase): no leaks found at any checkpoint.
- Trivy (0.72.0, vuln+secret): 0 secrets; 1 MEDIUM (`glib`, pre-existing since Phase 0/1, Linux-only backend - ADR-0002) at every checkpoint. No new findings - Phase 4B added zero dependencies.
- `cargo audit`: 0 vulnerabilities, same 17 pre-existing transitive warnings throughout, no new ones.

## A real bug found and fixed during live verification

Building Pay Cycle's `obligations_before`/`likely_income_series` against `recurring::list` unfiltered by account surfaced a genuine correctness gap: a confirmed recurring series left over on an account archived back in Phase 3 was silently inflating obligations, because archiving an account had never rejected its recurring series. Fixed with a shared `active_account_series` helper that excludes series belonging to archived accounts, plus a regression test (`archived_accounts_never_contribute_obligations_or_income_suggestions`). This is exactly what the live-verification-over-unit-tests-alone discipline this project has followed since Phase 0 is for - a purely in-memory unit test would never have surfaced state left over from a _different feature's_ earlier live-testing pass.

## Acceptance criteria (owner's six numbered items, each verified against the live app)

- [x] **Budgets** - live-verified: a real $150 Groceries budget against a real $120 purchase read "On track"; editing the plan to $100 flipped it to "Over budget"; the recent-months comparison correctly showed a prior month's $350 actual spending with no budget required for that month. `budget_calculations_exclude_transfers_and_account_for_refunds` covers the owner's explicit transfer/refund requirement directly.
- [x] **Pay Cycle** - live-verified: seeded 3 biweekly payroll deposits and 3 monthly bills; the detected "ACME Payroll" suggestion appeared and one click saved a correct biweekly schedule; the resulting cycle start, next payday, and obligations all matched hand-verified expected values (this is the flow that caught the archived-account bug above).
- [x] **Safe to Spend** - live-verified with a realistic multi-account scenario: $2,000 checking, a $750 credit-card charge, one confirmed $45 bill before the next payday, a goal reserving $300, a $100 owner reserve. Every line item and the final total ($1,865 − $45 − $300 − $100 = $1,420) matched hand-computed values exactly. `incomplete_when_no_pay_schedule_but_still_reports_known_eligible_cash` and `safe_to_spend_can_go_negative_rather_than_clamping_a_real_shortfall` cover the owner's explicit "incomplete state" and "never false precision" requirements.
- [x] **Credit-card safeguards** - `credit_card_debt_never_reduces_eligible_cash_or_safe_to_spend` and the Phase 4A accounting-boundary suite together cover every safeguard bullet the owner listed; live-verified as part of the Safe to Spend scenario above (the $750 CC charge correctly never touched eligible cash or the final number).
- [x] **Savings Goals** - live-verified: a goal with a $500 reserved amount persisted correctly and displayed the Safe-to-Spend protection note; editing current amount up to target flipped status to Completed while the reservation stayed intact. `safe_to_spend_with_reserve_affects_the_number_only_when_a_goal_reserves_it` covers "affects the number only when explicitly configured" directly.
- [x] **Net Worth** - live-verified across 95 days of seeded history in three accounts: every snapshot figure (assets $16,500, liabilities $2,000, net worth $14,500) and all 7 trend points matched hand-computed expected values exactly, including the month the trend correctly transitions as older transactions come into range.

## Test strategy

1. Rust unit tests: 145 total, up from 94 at the end of Phase 4A - confirmed directly via `cargo test` output at every checkpoint, not hand-counted. New coverage spans `budgets.rs` (9), `savings_goals.rs` (9), `pay_cycle.rs` (14), `safe_to_spend.rs` (9), plus `dates.rs`'s re-added `add_months`/`day_in_month` and `accounting.rs`'s `spending_in_range`/`net_worth_as_of`/`net_worth_history` additions (the two pre-existing `net_worth` tests were migrated in place to assert on the richer breakdown, not net-added).
2. `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo audit` - all clean at every checkpoint.
3. `npm run check` (prettier, tsc, node test, vite build) - clean at every checkpoint.
4. Ponytail review at every checkpoint, findings applied inline: a `reconcile_balance`-style query merge (Phase 4A carryover, N/A here), a `monthRange` duplicate-computation fix in BudgetsPage, three separate inline-`style` → CSS-class fixes (SavingsGoalsPage's progress bar, SafeToSpendPage's and NetWorthPage's headline numbers) after the first one revealed it wasn't yet a habit, and a hand-rolled month-name array replaced with native `Intl.DateTimeFormat` in NetWorthPage.
5. Two real, non-deterministic test-authoring bugs caught and fixed before merging - both instances of hardcoding absolute dates against schedule/cadence math that actually depends on the real system clock at test-run time (`pay_cycle.rs`'s `weekly_cycle_handles_an_anchor_in_the_future`, `safe_to_spend.rs`'s obligations test) - both rewritten to construct dates relative to the real "today" the same way the codebase's date-relative CDP verification scripts already did, and both verified deterministic by re-running.
6. Real end-to-end verification against the live native app over the WebView2 CDP debug port for all six features, detailed per-feature above - not simulated, not assumed from code reading. Every test account, transaction, recurring series, and goal created during verification was cleaned up (deleted, rejected, or archived) through the app's own commands before each session ended.

## Owner decisions

None required for this phase - fully specified in the owner's Phase 4B authorization message, including the explicit instruction to proceed autonomously.

## Hard-stop conditions

- Any need for envelope budgeting, a contribution-history ledger, or a stored available-credit figure - none arose; every owner requirement was satisfiable with the smaller mechanisms actually built.
- Any need for a new Tauri plugin or broader filesystem/network authority - none arose.
- The archived-account leakage bug turning out to require a schema change (e.g., cascade-deleting recurring series on account archive) rather than a read-side filter - it did not; excluding archived accounts' series at read time is sufficient and preserves the series as a historical record rather than destroying it.
