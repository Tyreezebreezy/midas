# Phase 011: Account reconciliation + running balances

- Status: COMPLETE
- Owner gate: APPROVED 2026-09-12 (owner handoff: "MIDAS Development Batch 4 - Account Reconciliation + Running Balances")

## Goal

> A user should be able to compare MIDAS against a real bank or credit-card
> statement, identify discrepancies, and mark a verified statement period
> as reconciled when the difference reaches zero.

Plus deterministic running balances so discrepancies can be traced through
account history. Explicitly not Transaction Intelligence, Recurring
Intelligence, Budget redesign, or Insights redesign.

## Reconnaissance (before writing code)

- `transactions.reconciliation_status` (`unreconciled`/`cleared`/`reconciled`)
  already existed from Foundation Batch 1, with a fully-wired setter
  (`transactions::set_reconciliation_status` /
  `set_transaction_reconciliation_status`) - but nothing had ever called it.
  Its own doc comment already anticipated this batch: "the UI, not the data
  layer, is responsible for warning that a verified period may need
  re-checking - the warning itself is a future batch's job." Distinct from
  `is_pending` (a bank-reported hold state, unrelated to reconciliation).
- No running-balance logic existed anywhere in the codebase.
- `transactions::list` orders `transaction_date DESC, id DESC` - reused as
  the deterministic ascending mirror (`ASC, id ASC`) for running balances.
- Account balance (`accounts::SELECT_ACCOUNT`) is a live, undated
  `SUM(amount_cents) WHERE is_archived = 0` - the _current_ total, distinct
  from the _cleared-as-of-a-date_ figure reconciliation needs.
- `accounting::reconcile_balance` ("Update balance" in the UI) is a
  separate, pre-existing feature - a one-number correction that creates or
  upserts an `opening_balance`/`balance_adjustment` transaction. Not to be
  confused with this batch's statement-reconciliation workflow; both are
  named "reconcile" in different senses, so the UI never uses "Reconcile"
  for the balance-correction button (it stays "Update balance") and this
  module's own commands/types are named distinctly (`reconciliation.rs`,
  `ReconciliationRecordStatus`, never overloading `accounting::`'s names).
- Batch 3's OFX/QFX parser (`ofx.rs`) preserved the statement's ending
  balance and as-of date (`imports.statement_balance_cents`/
  `statement_balance_date`) but walked past `BANKTRANLIST`'s `DTSTART`/
  `DTEND` (the statement _period_) without extracting them at all -
  confirmed by reading the parser directly, not assumed. Closed as a small,
  justified extension (see "OFX integration" below).
- CSV imports never set any statement metadata (confirmed: the CSV path
  always passes `None` into `commit_staged`).
- No existing code checked whether a transaction being edited/archived/
  deleted belonged to a verified statement - `set_reconciliation_status`'s
  own test explicitly proved an edit "must not silently reset reconciliation
  status," meaning nothing downstream reacted to it either.
- `TransactionForm.tsx` and `AccountsPage.tsx`/`TransactionsPage.tsx`'s
  existing dialog/table patterns were reused directly - no second edit form,
  no new sidebar destination (section 28).

## Model

One new table, `reconciliations` (migration 0011):

```
id, account_id, statement_start_date, statement_end_date,
statement_ending_balance_cents, cleared_balance_cents, status,
reconciled_at, source_import_id, note, created_at, updated_at
```

`status` is `completed` / `needs_review` / `reopened` - never a boolean.
`cleared_balance_cents` is a deliberate _snapshot_, not a value recomputed
on every read: comparing a fresh recomputation against this stored snapshot
is not how invalidation is detected (see "Invalidation" below, which is
field-based, not diff-based), but the snapshot is what makes the history
list honest about what MIDAS actually agreed to at the time, independent of
what the ledger looks like now.

`transactions` gained one nullable FK, `reconciliation_id` - not a join
table, since a transaction belongs to at most one reconciliation at a time
(periods never overlap, enforced in `reconciliation.rs`). Reopening a
reconciliation clears this FK on its transactions, freeing that period.

The reconciliation _workspace_ itself has no persisted "draft" - period,
statement balance, and the live difference are computed fresh on every
`preview()` call; nothing is written until `complete()` succeeds. Clearing
a transaction during the workspace session writes immediately via the
already-existing `set_transaction_reconciliation_status`, so clearing work
is never lost even if the workspace is closed without finishing.

## Balance calculation

**Cleared balance as of a date** (`reconciliation::cleared_balance_as_of`):

```sql
SELECT COALESCE(SUM(amount_cents), 0) FROM transactions
WHERE account_id = ?1
  AND transaction_date <= ?2
  AND is_archived = 0
  AND (is_pending IS NULL OR is_pending = 0)
  AND (
       transaction_type IN ('opening_balance', 'balance_adjustment')
       OR reconciliation_status IN ('cleared', 'reconciled')
  )
```

`opening_balance` and `balance_adjustment` always count, regardless of
their own clearing state - they are the owner's own already-confirmed
anchors (ADR-0007/ADR-0009), not a bank line item that needs checking off.
Without this, every account's _first_ reconciliation would start short by
its entire opening balance, since nothing would ever have cleared it.
Every other transaction counts only once `cleared` or `reconciled` -
exactly what makes ticking transactions off during the workspace move the
displayed difference. A transaction already `reconciled` from an earlier,
completed period keeps counting forward automatically (its status doesn't
revert), matching how real reconciliation software treats prior periods as
permanently settled. Archived and pending transactions never count.

**Difference**: `statement_ending_balance_cents - cleared_balance_cents`,
recomputed server-side at both preview and completion time - the frontend's
own displayed value is never trusted as authorization to close (section 31:
no AI, no heuristics, exact integer-cents arithmetic only).

**Credit cards need no special-casing.** ADR-0004 already makes a
liability's `amount_cents` positive when a purchase increases what's owed
and negative when a payment reduces it - the exact "amount owed" a real
credit-card statement displays. The statement-balance input reuses the
identical "enter what you owe as a positive number" convention
`ReconcileBalanceForm.tsx` already established, and the difference formula
is applied completely unmodified for liability accounts.

## Status semantics

- `unreconciled` / `cleared` / `reconciled` (transaction-level, unchanged
  from Foundation Batch 1): the owner's own progressive confirmation
  against a statement, independent of `is_pending`.
- `completed` / `needs_review` / `reopened` (reconciliation-record-level,
  new this batch): whether MIDAS still trusts a specific closed statement.
  Deliberately _not_ modeled as new transaction-level states - invalidation
  is a property of the _reconciliation record_, so a transaction that was
  genuinely part of a verified period keeps saying `reconciled` even after
  that period needs review; only the reconciliation's own row changes.

## Reconciliation workflow

Account-level "Reconcile" button (`AccountsPage.tsx`, disabled for archived
accounts) opens `ReconciliationWorkspace.tsx`: period + statement-balance
inputs (defaulting to the current calendar month), a dominant difference
readout (large, colored, "Ready to reconcile" when zero - section 35), the
period's transactions with a running-balance column and a `Cleared`
checkbox per row (wired straight to the existing status setter), inline
`Edit`/`Add transaction` reusing `TransactionForm.tsx` unmodified, and a
`Reconcile Statement` button enabled only when the difference is exactly
zero. Completing calls `reconciliation::complete`, which re-validates
everything server-side, inserts the `reconciliations` row, and promotes
every `Cleared` transaction in the period (plus any `balance_adjustment`
dated within it - always-counted, so always linked too) to `Reconciled`.

## Invalidation

Reconciled does not mean immutable (section 15). Editing a transaction with
a non-null `reconciliation_id` first shows `ReconciledEditWarning.tsx`
("This transaction belongs to a reconciled statement... Changing it may
invalidate the verified balance for `<period>`" - Cancel / Edit Anyway) -
reused for both the transaction editor and the Archive action. The warning
is a courtesy; the actual safety mechanism is server-side and fires
regardless of whether the owner ever saw a warning dialog:

**Invalidating** (per the handoff's explicit list, checked field-by-field,
never by diffing a recomputed total against the snapshot): `amount_cents`,
`account_id`, `transaction_type`, the date moving _outside_ the
reconciliation's own period (moving within it is harmless - the cleared
sum doesn't care what day within the period a row lands on, so this is
deliberately not invalidating), archiving, deleting a manual transaction,
and changing or removing an already-linked `balance_adjustment` (hooked
into `accounting::upsert_balance_adjustment`'s update/delete paths, plus a
date-covering check for the one case with no existing link yet: a
brand-new adjustment landing on a date an already-completed reconciliation
covers).

**Never invalidating**: notes, category, subcategory, merchant/description
text, tags (a separate table that never touches `transactions.updated_at`
at all), `is_one_time`. Split-transaction invalidation is explicitly
deferred - `split_parent_transaction_id` exists as a Foundation Batch 1
column but no split-creation feature exists yet to invalidate for.

Invalidation flips the reconciliation's own `status` to `needs_review`
(idempotent - a no-op if already `needs_review`/`reopened`); it never
touches the transaction's own `reconciliation_status`, which still
correctly says `reconciled` (it _was_ part of a verified period).

## Reopening

Explicit action only (`ReconciliationHistoryDialog.tsx`'s inline
Cancel/Confirm, matching `ImportHistoryDialog`'s established pattern - not
a nested `<dialog>`, which would render non-modal and break focus-trap
expectations). Reopening: (1) reverts every linked transaction's status
from `reconciled` back to `cleared` (never `unreconciled` - clearing work
is preserved) and clears its `reconciliation_id`, freeing that period; (2)
flips the reconciliation record's own status to `reopened`, kept in history
forever rather than deleted. A freed period can be reconciled again,
producing a _new_ `reconciliations` row - the old, `reopened` one and the
new, `completed` one both remain visible, giving a complete audit trail.

## OFX integration

Extended `ofx.rs` to also capture `BANKTRANLIST`'s `DTSTART`/`DTEND` into
`OfxStatement.statement_start_date`/`statement_end_date` (previously parsed
past and discarded), threaded through a new `StatementMetadata` struct into
two new nullable `imports` columns (`statement_start_date`/
`statement_end_date`, migration 0011). `reconciliation::latest_statement_hint`
surfaces the most recent OFX/QFX import that carried a statement balance as
a prefill banner ("Statement information found in `august.qfx`... Use for
reconciliation") - the owner always still confirms or edits it before
anything is recorded; nothing auto-reconciles from an import. CSV never
populates this (no reliable statement-level data, section 21) - manual
entry is the only path.

## Overlapping periods

`reconciliation::preview` and `complete` both reject (before any write) a
period overlapping an existing `completed` or `needs_review` reconciliation
for the same account, with a specific, actionable error naming the
conflicting dates ("This account already has a reconciliation covering
2026-08-01 to 2026-08-31. Reopen it first...") rather than a generic
failure or a raw SQLite message.

## Running balances

`reconciliation::running_balances`: one indexed range query
(`transaction_date ASC, id ASC` - the deterministic mirror of
`transactions::list`'s own `DESC, id DESC`) folded into a running sum in
Rust - no N+1, no materialized cache (none justified; a single query plus a
linear fold is already efficient). Same-date transactions always tie-break
by ascending `id`, so unrelated metadata edits can never reorder historical
balances. Available in the reconciliation workspace (always) and on
Transactions (an opt-in "Show running balance" checkbox, enabled only when
one specific account is selected, since running balance is meaningless
mixed across accounts - deferred general column customization to a future
Transaction Intelligence batch per section 12's explicit instruction).

## Account summary (Data Health groundwork)

`reconciliation::account_summary`/`get_reconciliation_summary`: last
reconciled date and ending balance, `needs_review` count, unreconciled
transaction count - small, queryable now, without committing to what a
future Data Health feature's UI will look like (section 29).

## Bugs found during native verification

None found in the reconciliation logic itself - a stale "end date must not
be before start date" error observed once during accessibility testing was
traced to the test harness's own double-dispatch of a programmatic input
event on an already-current value (a scripting artifact of driving React
controlled inputs from outside, not a real user-reachable path), confirmed
by a clean reopen showing correct behavior immediately.

## Tests

256 pre-existing Rust tests unchanged; 33 new (30 in `reconciliation.rs`,
1 in `db.rs` for the migration boundary, 2 in `import.rs`/`ofx.rs` for the
statement-period extension) - 289 total. Coverage: running balance
(checking/credit-card/same-date ordering/archived-exclusion), the cleared-
balance formula (opening balance/balance adjustment always counting,
pending exclusion, archived exclusion, refunds/transfers, credit-card
symmetry, prior-period carry-forward), first reconciliation, exact-zero
gating, non-zero rejection changing nothing, overlapping-period rejection,
reopening (transactions freed to `Cleared`, history preserved, the freed
period reconcilable again), every invalidating field individually
(amount/account/type/date-crossing-boundary/archive/delete/balance-
adjustment-change) and the harmless-edit/within-period-date-move
non-invalidation cases, the OFX statement-period extraction and its
reconciliation-hint round-trip.

## Native verification

Real `tauri dev` binary, real WebView2 CDP, disposable test accounts
(archived afterward). Checking: seeded an opening balance and 5 August
transactions matching the handoff's own worked numbers, drove the full
workspace UI (period/balance entry, clearing transactions one by one,
watching the difference move, reaching `$0.00`/"Ready to reconcile",
completing, verifying the history entry) - every value cross-checked
against direct IPC queries, not just UI text. Invalidation: edited a
reconciled transaction's amount through the real warning-gated flow,
confirmed the reconciliation flipped to `needs_review` in both the backend
and the History dialog's badge. Reopening: confirmed transactions reverted
to `Cleared`, the period became reconcilable again, and both the old
(`reopened`) and new (`completed`) records persisted side by side. Credit
card: reconciled a purchases-and-payment statement, confirmed the familiar
positive-owed sign convention needed no special UI handling, and confirmed
`get_total_spending` still excluded the payment afterward (section 23
unaffected). Restart: killed and relaunched the real binary, confirmed
reconciliation history and running balances were byte-identical afterward.
Overlap rejection was also caught live and unprompted (attempting to
reconcile an already-covered period surfaced the exact error message
above). Accessibility: every focusable control in the workspace, the
history dialog, and the invalidation warning has a correct programmatic
accessible name; gold focus-visible outline intact throughout. Screen
sizes: 1280x800 and 1024x640 both checked for the workspace - no page-level
horizontal overflow (the transaction table's own internal scroll is
expected and consistent with every other data table in the app).

## Explicit non-goals (unchanged from the handoff)

Split-transaction invalidation (no split feature exists yet to invalidate
for), general transaction-table column customization, Data Health as a
full feature, Financial Calendar/forecasting, AI/heuristic balancing.

## Acceptance criteria

- [x] `cargo fmt --check`, `cargo check`, `cargo clippy -- -D warnings`,
      `cargo test` all pass (289 tests: 256 existing + 33 new).
- [x] `cargo audit`/`npm audit`/Gitleaks/Trivy: no new findings versus the
      Batch 3 baseline (zero dependency changes this batch).
- [x] `npm run check` (format, typecheck, tests, build) passes.
- [x] Migration 0011 applies cleanly to a fresh database and to a
      Batch-3-shaped fixture with real OFX statement-balance data.
- [x] Checking and credit-card reconciliation both live-verified end to
      end, including the credit-card sign convention needing zero special
      casing.
- [x] Invalidation live-verified for a real edit through the real warning
      dialog, not just a unit test.
- [x] Reopen live-verified to preserve full audit history and re-enable
      the freed period.
- [x] Overlapping-period rejection live-verified with a clear, specific
      error message.
- [x] Running balances live-verified against hand-computed values and
      confirmed identical after a real process restart.
- [x] Spending/income totals confirmed unaffected by reconciliation status
      changes (credit-card payment still excluded from spending).
- [x] Accessibility and both required screen sizes verified live for every
      new UI surface.
- [x] All native-verification test accounts archived afterward.

## Owner decisions

None required. The workspace-has-no-persisted-draft design, the
join-table-vs-single-FK choice, and the field-based (not diff-based)
invalidation model were all resolvable directly from the handoff's own
"think carefully about what must remain historically stable" and "make
this distinction intentionally" guidance.

## Hard-stop conditions

None encountered. No destructive or irreversible action was taken against
the live application-data database - all verification used disposable
accounts on the same real, continuing app-data database prior batches
established, archived afterward.
