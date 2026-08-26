# ADR-0009: Separate `balance_adjustment` from `opening_balance`

- Status: accepted
- Date: 2026-08-26
- Supersedes: ADR-0007's mechanism section (§3, "one mechanism serves both") - the rest of ADR-0007 (computed balance, opening balance as a transaction, the rationale against the requirements) stands unchanged.

## Context and problem

ADR-0007 built `reconcile_balance` as one upsert mechanism for both "set a starting balance" and "reconcile it later." That mechanism kept exactly one `opening_balance` transaction per account and rewrote its `amount_cents` **and its `transaction_date`** on every subsequent call. This is a real accounting bug, not a style concern: the opening balance's date is what every historical net-worth-as-of-a-past-date calculation anchors on. Moving it forward on a later reconciliation silently rewrites what MIDAS would have reported for every date between the old anchor and the new one - a January statement re-run in August would come back different, with no error, no log, and no way to tell it had happened.

The owner named this precisely: reconciling today must never change what the app reported for a past date, and the fix must be a real dated correction, not a relocated anchor.

## Decision

**Split into two transaction types, one mechanism, two paths.**

1. **`opening_balance`** keeps ADR-0007's exact semantics: the account's true historical starting point, anchored once before its earliest other transaction (or `as_of_date` if none exist yet), enforced to at most one per account by `idx_one_opening_balance_per_account`. After this session's fix, it is **never touched again** by a later reconciliation - not its amount, not its date.

2. **`balance_adjustment`** is new: a dated correction for drift discovered after the opening balance already exists. It lands on `as_of_date` itself, never backdated to the account's origin. A partial unique index, `idx_one_adjustment_per_account_per_date`, allows at most one per account **per date** rather than per account - an account can accumulate many of these over its life (one per reconciliation), unlike the single opening-balance anchor.

3. **`reconcile_balance` becomes a two-path dispatcher**, not a single upsert: it checks whether an `opening_balance` already exists for the account and routes to `establish_opening_balance` (first-ever call) or `upsert_balance_adjustment` (every call after). The public signature and every caller are unchanged - this is an internal correction, not a new command.

4. **The adjustment's delta excludes its own prior amount from the baseline.** `upsert_balance_adjustment` computes `baseline = current_ledger_total - existing_same_date_adjustment_amount` before computing `delta = known_current_balance_cents - baseline`, so re-reconciling the same date twice corrects that date's adjustment in place rather than double-counting it against itself.

5. **Both types are excluded from every derived calculation the same way opening balance always was**: `spending_effect` returns 0 for both, `recurring::detect_candidates` excludes both explicitly (see below), and both simply count toward `SUM(amount_cents)` for balance and net worth like any other transaction - no special-casing needed there since ADR-0007's computed-balance model already treats every transaction uniformly.

## A second risk this surfaced: recurring-detection false positives

`opening_balance` was always structurally safe from `recurring::detect_candidates` (at most one row per account, and detection requires 3+ occurrences to propose anything). `balance_adjustment` is not: a user who reconciles regularly produces several rows sharing the same merchant label ("Balance adjustment"), potentially with consistent gaps and amounts - exactly what the detector looks for. Left unhandled, MIDAS could propose a user's own reconciliation habit back to them as a "recurring bill." Fixed with an explicit `AND transaction_type NOT IN ('opening_balance', 'balance_adjustment')` filter in the detector's query, plus a regression test (`balance_adjustments_are_never_proposed_as_a_recurring_pattern`) that reconciles the same account four times at consistent monthly-ish intervals with consistent deltas - the exact shape that would otherwise trip the detector - and asserts nothing is proposed.

## Rationale against the requirement

- **Historical net worth is now genuinely immutable once past**: `net_worth_as_of(date)` only ever sums transactions with `transaction_date <= date`; since a `balance_adjustment` is dated at its own reconciliation date rather than backdated, it structurally cannot appear in any `net_worth_as_of` call for a date before it happened. `net_worth_history_changes_only_from_the_adjustment_date_forward` proves this directly.
- **Today's balance is still exactly correct**: the adjustment still lands in `SUM(amount_cents)` like every other transaction, so `balance_of` reflects the correction immediately - only the _history_ is protected, not the _current_ number.
- **No new schema concept**: this is one more `transaction_type` value and one more partial unique index, the same shape ADR-0007 already established - not a new table, not a new command, not a parallel reconciliation subsystem.

## Consequences and accepted tradeoffs

- Migration `0007_balance_adjustment.sql` rebuilds `transactions` again (SQLite has no `ALTER TABLE ... ALTER COLUMN` for `CHECK` constraints), the same pattern as migration `0004`. No data loss risk: this repository has never held real financial data, per the same standing note in ADR-0007.
- `upsert_balance_adjustment` runs two queries (an existing-row lookup, then a ledger-total sum) rather than one merged query. A single CASE-based query is possible but strictly harder to read for a personal-finance-scale table where the extra indexed lookup is irrelevant - the same "not a preemptive optimization" stance ADR-0007 already took for computed balance.
- `reconcile_balance`'s two new helpers (`establish_opening_balance`, `upsert_balance_adjustment`) are private, not part of the public module surface - nothing outside `accounting.rs` needed to change, and no caller anywhere in the app had to be touched.
