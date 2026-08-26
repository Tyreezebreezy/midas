# ADR-0007: Computed account balance, and opening balance as a transaction

- Status: accepted
- Date: 2026-08-25
- Supersedes: none (refines the model `accounts.balance_cents` used since Phase 1)

## Context and problem

Since Phase 1, `accounts.balance_cents` has been a free-standing, manually-typed column: set once at account creation, changed only by re-typing a new value in the account edit form. Creating, editing, importing, or deleting a transaction never touched it. This means ADR-0004's own stated invariant - "keeps `balance = sum(transactions)` a true invariant" - was never actually true in the running application; it described the sign convention's intent, not enforced behavior.

The owner's Phase 4A ("Accounting Boundary Hardening") requires this to become real and tested: a $50 checking purchase must reduce the checking balance by $50, a $50 credit-card purchase must increase what's owed by $50, and so on. It also requires solving the "partial-history problem": MIDAS may only have 3 months of imported transactions for an account that has existed for years, so `sum(imported transactions)` cannot be assumed to equal the real current balance without an explicit, auditable starting point.

## Decision

**1. Balance becomes computed, never stored.** `accounts.balance_cents` is dropped as a column. `Account.balance_cents` in the API response is now `COALESCE(SUM(transactions.amount_cents), 0)` for that account's non-archived transactions, computed at read time. There is exactly one source of truth (the ledger); no code path can let a cached balance drift from it, and every transaction-affecting operation (create, edit, archive, CSV commit) automatically keeps balance correct with zero incremental-update bookkeeping.

**2. Opening balance is a transaction, not a separate field.** A new `transaction_type` value, `opening_balance`, represents "this account's balance as of a specific date, before any transaction MIDAS has recorded." Because it is a transaction like any other, `balance = sum(transactions)` holds by construction with no special-casing anywhere balance is read.

**3. One mechanism serves both "set a starting balance" and "reconcile/correct it later."** `accounting::reconcile_balance(conn, account_id, known_current_balance_cents, as_of_date)`:

- Computes `delta = known_current_balance_cents - SUM(amount_cents) FROM transactions WHERE account_id = ? AND transaction_type != 'opening_balance' AND is_archived = 0` (every real transaction already on record).
- Anchors the opening-balance transaction's date at one day before the account's earliest other transaction (or `as_of_date` if there are none yet), guaranteeing it always sorts strictly first with no same-day tiebreak ambiguity anywhere else in the app.
- Upserts: creates the anchor if `delta != 0` and none exists, updates it in place if one exists, deletes it if reconciliation now computes `delta == 0` (keeps the ledger's shape identical to "never needed an anchor" whenever that becomes true again - e.g., the user eventually imports full history back to account opening).
- A partial unique index (`idx_one_opening_balance_per_account`, `WHERE transaction_type = 'opening_balance'`) enforces at most one anchor per account at the database level, not just in application code.

This is deliberately the same operation whether called once at account creation (nothing else exists yet, so `delta` is simply the whole starting balance) or repeatedly afterward as a correction. It is not a statement-by-statement reconciliation subsystem - it is one recomputed adjusting entry, which is the smallest mechanism that satisfies every requirement below.

## Rationale against the requirements

- **Deterministic**: `reconcile_balance` is a pure function of (known balance, existing ledger) - identical inputs always produce the identical anchor transaction.
- **Auditable**: the anchor is a normal, visible transaction row with a distinct type; nothing about it is hidden or inferred silently.
- **Does not masquerade as spending/income**: `opening_balance` is excluded from `accounting::spending_effect` entirely (contributes `0`), and is deliberately left out of `TRANSACTION_TYPES` (the frontend's manual-entry dropdown) so a user cannot casually create one by hand outside the reconcile flow.
- **Does not double-count imported history**: `delta` is computed by subtracting every transaction already on record, so adding the anchor always brings the ledger total to exactly the stated real balance - never more.
- **Works for both assets and liabilities**: the mechanism is pure signed arithmetic with no branch on `AccountType` at all; ADR-0004's uniform sign convention (positive = this account's own balance went up, for every account type) already makes assets and liabilities symmetric, so `reconcile_balance` doesn't need to know or care which kind of account it's anchoring.

## Consequences and accepted tradeoffs

- `AccountInput` no longer accepts `balance_cents` for `update_account` (renaming an account or toggling its net-worth inclusion can no longer accidentally re-anchor its balance to a stale form value). Setting a starting balance is bundled into `create_account` as an explicit initial reconciliation; correcting it later is a separate, explicit `reconcile_account_balance` command - never a side effect of an unrelated edit.
- No prior `balance_cents` values were migrated into synthetic opening-balance transactions during the schema rebuild (migration `0004_accounting_boundary.sql`). This repository has never held real financial data - every phase's Security section records dev/test fixtures deleted after verification - so there was nothing real to preserve, and building a migration heuristic for data that doesn't exist would be speculative complexity this project's own discipline argues against.
- Computing balance via `SUM(...)` on every account read is a full-table aggregate rather than an O(1) column read. Personal-finance-scale data (single user, thousands of transactions at most) makes this irrelevant in practice, and `idx_transactions_account` already indexes `(account_id, is_archived)` for it. If this ever becomes a measured problem at a scale MIDAS was never designed for, that is a new ADR, not a preemptive optimization here.
