# Phase 012: Transaction Intelligence

- Status: COMPLETE
- Owner gate: APPROVED 2026-09-12 (owner handoff: "MIDAS Development Batch 5 - Transaction Intelligence")

## Goal

> Make transactions easy to understand, clean up, categorize, split,
> organize, and correct - without ever compromising ledger integrity.

Payees, Categories, Tags, Splits, Bulk cleanup, correction-to-rule, one-time
expense designation, and better transaction presentation. Explicitly not
Recurring Intelligence, Budget redesign, Insights redesign, forecasting, or
mobile work.

## Reconnaissance (before writing code)

- `transactions.payee_id` and `transactions.is_one_time` already existed as
  columns (Foundation Batch 1) but were never settable from any UI or
  `TransactionInput` field - this batch's job was to make them real, not
  invent new columns.
- `transactions.split_parent_transaction_id` also already existed, but its
  doc comment described a design (each split as its own archived-parent-
  plus-child-rows transaction) that directly contradicts this batch's
  explicit, non-negotiable requirement: split children must never be
  double-counted as separate ledger movements. Rather than repurpose a
  column whose original design is incompatible, or edit the migration that
  created it (against this project's migration-immutability discipline),
  a new `transaction_splits` child table (migration 0012) was added and the
  old column's doc comment corrected to explain the supersession - the
  column stays, permanently unused, at zero cost (nullable, never written).
- No `categories` table exists - categories have always been free text on
  `transactions.category`. A relational Category Group -> Category hierarchy
  would touch budgets, reports, rules, CSV import, and OFX pervasively for a
  benefit ("group by group") nothing in this batch's own scope needs.
  Deferred per the handoff's own escape hatch (section 7): "if not cleanly
  supported, document and defer rather than force a disruptive redesign."
  The smallest prerequisite - a table with a `parent_group` FK and a
  migration to backfill every existing free-text category into it - is
  documented here for whichever future Budget/Insights batch actually needs
  grouped reporting.
- Same deferral for Origin-inspired Fixed/Flexible/Discretionary
  classification (section 30): it doesn't fit cleanly into a free-text
  category model without first having Category Groups, so it is documented
  as a prerequisite for that same future batch rather than bolted on now.
- `tags`/`transaction_tags` (Batch 3) already existed as a working backend
  with zero UI - this batch's Tags feature is a UI on existing plumbing, not
  a new system.
- `rules.rs`'s private `MatchType::matches()` was the correct thing to reuse
  for correction-to-rule (fetch candidates via SQL, filter in Rust), rather
  than translating every `MatchType` variant into a SQL `LIKE` pattern - the
  same matching code CSV import already trusts, so there is exactly one
  rule-matching implementation in the codebase, not two.
- `import.rs`'s Undo-safety query (`removable_transaction_ids`) was
  re-audited specifically because a new child-table concept
  (`transaction_splits`) was being introduced - the same class of gap Batch
  3 anticipated when tags were added. Found and fixed: the query checked the
  old, never-populated `split_parent_transaction_id` column but had no
  `NOT EXISTS` check against the real `transaction_splits` table, meaning a
  freshly-imported-then-split transaction was not actually protected from
  Undo removal. Fixed before it could ship.
- `accounts::balance_cents`'s correlated-subquery pattern was reused twice
  more: `transactions.has_splits` (`EXISTS(...)`) and `payees.transaction_count`
  (`(SELECT COUNT(*) ...)`) - both computed live, never a stored count that
  could drift from reality.

## Model

One new table, `transaction_splits` (migration 0012):

```
id, transaction_id, category, subcategory, amount_cents, notes,
created_at, updated_at
```

The parent transaction's own `amount_cents`, `account_id`, and
`reconciliation_id` are never touched by splitting - a split is purely a
reporting-layer breakdown of one ledger movement, read by category/reports
queries, never summed into the account balance a second time. Deleting and
re-inserting the full set on every edit (`replace_for_transaction`) keeps
"what does this transaction's split look like" a single, always-consistent
read with no incremental-update drift to reason about.

`payees` (already existed, unused by any UI) gained a live `transaction_count`
column (correlated subquery, non-archived transactions only). `tags` (Batch
3) gained no schema changes - only a UI.

## Payees

`PayeesPage.tsx` (embedded in Settings, no new sidebar destination):
list/search/type/archive-restore/transaction-count, plus a merge dialog.
Merge (`payees::merge`) redirects every transaction (`UPDATE transactions
SET payee_id = target WHERE payee_id = source`) and archives the source
(`merged_into_payee_id` set, `is_archived = 1`) - never deleted, so it stays
resolvable and its history intact. A merged payee can never be merge target
or merge source again (self-merge and chained-merge both rejected), and
disappears from every normal picker while still showing up, clearly labeled
"Merged", under "Show archived".

## Categories

No relational model (see Reconnaissance) - category cleanup is a rename
applied everywhere a category string appears at once:
`categories::rename(from, to)` updates `transactions.category`,
`transaction_splits.category`, and `merchant_rules.category`
unconditionally, and `budgets.category` conditionally - skipping any month
where the target category already has a budget row (a `(category, month)`
UNIQUE constraint would otherwise fail outright) rather than aborting the
whole rename. Returns a `RenameSummary` (counts per table plus
budgets-skipped-due-to-conflict) surfaced directly in
`CategoryCleanupPage.tsx`. Renaming a category to an existing one is exactly
how two categories get merged - there is no separate "merge categories"
command.

## Tags

`TagsPage.tsx` (embedded in Settings): create/list/delete on the existing
Batch 3 backend. Delete is a real hard delete (tags are organizational
labels, not financial records) that explicitly cleans up
`transaction_tags` first, since the foreign key carries no
`ON DELETE CASCADE`. Tag assignment lives in the transaction editor as a
checkbox group, and tag filtering is a dropdown on Transactions
(`tag_id` on `TransactionFilter`) - one tag at a time, matching the
handoff's "no over-complicated boolean logic."

## Splits

`SplitEditor.tsx`, opened from the transaction editor ("Split this
transaction" / "Edit split"). Invariants enforced both client-side (for
immediate feedback) and server-side (`splits::replace_for_transaction`, the
actual authority): at least two parts, no zero-amount part, and the parts
must sum to **exactly** the parent's own signed amount - checked with
integer cents, never floats. Save is blocked until balanced; there is no
miscellaneous/remainder split invented to force a save through. "Assign
remainder" is a convenience that fills one row with whatever is left, never
an auto-balancing shortcut that hides a real mismatch.

Reporting integration: `reports::spending_by_category` is a `UNION ALL` of
a split-expanding branch (joins `transaction_splits`, one row per split) and
an unsplit branch (guarded by `NOT EXISTS (SELECT 1 FROM transaction_splits
WHERE transaction_id = t.id)`) - a split parent's own category never also
appears once split, and nothing is ever double-counted. Splits never
invalidate reconciliation (see Reconciliation integration below) since they
structurally cannot touch the `transactions` table at all.

## Bulk Actions

`bulk.rs`: a generic `update_each` helper wraps N single-item mutations in
one `rusqlite::Transaction` (all-or-nothing, verified by a test that fails
one item mid-batch via a nonexistent id and confirms the real item's own
change also rolled back) while calling through each field's own existing
single-item function (`transactions::update` via a new
`transactions::get_for_edit` fetch-and-reshape helper, or `tags::attach`/
`detach`/`transactions::set_archived` directly) - never a second, bulk-only
mutation path that could drift from what a single edit already validates
and invalidates. `TransactionsPage.tsx` gained row checkboxes, a select-all-
visible header checkbox, and a compact bulk toolbar (Set category / Set
payee / Add tag / Remove tag / Mark one-time / Clear one-time / Archive)
that only appears once something is selected.

**Reconciled-selection gate**: before running any bulk action,
`TransactionsPage.tsx` checks how many selected transactions are already
linked to a reconciliation; if any are, `BulkReconciledWarning.tsx` (the
bulk analogue of Batch 4's `ReconciledEditWarning.tsx`, naming a count
rather than a single period since a selection can span many different
reconciliations) requires an explicit "Continue Anyway" before the bulk
mutation runs at all - Cancel leaves the selection untouched and nothing is
written. The actual invalidation mechanism is still the same server-side,
field-based hooks from Batch 4, fired identically whether an edit arrives
through a bulk call or a single edit, because bulk actions call through the
exact same single-item update functions.

## Correction-to-rule

Reuses the existing Rules system entirely - no second matching engine.
Editing a transaction's merchant or category offers "Reuse this
correction?" with four scopes: **This transaction only** (no rule),
**Matching past transactions** (no rule, applies the new merchant/category
to every other non-archived transaction whose raw description matches via
the existing private `MatchType::matches()`, optionally scoped to one
account), **Matching future transactions** (saves a rule via the existing
`rules::create`, touches nothing already imported), and **Past + future**
(both). A user's explicit correction always wins over whatever a rule or
prior import guess produced - this offer only ever appears after a save
already succeeded with the user's typed values.

## One-time expense

`is_one_time` is now settable from the transaction editor and bulk actions,
with copy making the distinction explicit: it still counts as real spending
today; only a future "normal monthly" baseline (Insights, not this batch)
may choose to exclude it. `TransactionFilter.is_one_time` and a
Transactions-page filter make one-time transactions independently
searchable.

## Filters and presentation

`TransactionFilter` gained `payee_id`, `tag_id`, `reconciliation_status`,
`is_one_time` - all reusing the same `list_transactions` query and its
existing search, never a second query language. The transactions table
gained a compact, non-color-only status column (`Needs review` / `One-time`
/ `Split` / `Archived` badges, text-labeled) and a `Details` action opening
`TransactionDetails.tsx` - a small read-only surface for raw description,
payee, import source filename, external ID, and timestamps, kept off the
main row entirely so the row stays scannable.

## Reconciliation integration (Batch 4 protections, re-verified)

Every new editing surface (bulk actions, payee/category/tag/one-time edits,
splitting) routes through the same `transactions::update`/`get_for_edit`
functions Batch 4's invalidation hooks already watch - nothing bypasses
them. Live-verified: merchant/category edits (descriptive metadata) and
`is_one_time` changes, including through bulk actions, never flip a
reconciliation to `needs_review`; an amount change still does, identically
to Batch 4, whether made through the plain editor or after a correction
offer. Splitting a reconciled transaction shows a courtesy note
("Splitting only affects category reporting, not the account balance, so it
will not invalidate that reconciliation") and, confirmed live, genuinely
never invalidates - splits cannot touch `transactions` at all, so there is
nothing for the existing field-based hooks to even see.

## Import integration

`import.rs`'s Undo safety query now also excludes any transaction with rows
in `transaction_splits` (see Reconnaissance), with a new regression test
proving a split survives Undo with its splits intact even though splitting
never touches `updated_at`. Correction-to-rule's "Matching future
transactions" rules are ordinary rows in the same `merchant_rules` table
CSV import already reads - live-verified end to end: created a rule via a
real correction, then imported a fresh CSV row matching it, and the rule
applied automatically with no separate code path.

## Bugs found and fixed during native verification

Both were introduced in this batch's own frontend work (not pre-existing)
and were only reachable by actually driving the real compiled app, not by
any unit test:

1. **Tag changes silently never saved.** `TransactionForm.tsx`'s
   `handleSubmit` computed the "previous tags" baseline for `syncTags` as
   `new Set(selectedTagIds)` taken at submit time - by then `selectedTagIds`
   already reflected every checkbox the user had just toggled, so the
   diff against "itself" was always empty and `tagsApi.attach`/`detach`
   were never called. Fixed by capturing the original tag set once, in a
   `useRef`, at the moment the form loads (before any user toggles), and
   diffing against that instead. Found by attaching a tag through the real
   editor and then filtering Transactions by that tag - it matched nothing
   until fixed.
2. **Re-saving an already-split transaction failed with a confusing
   backend error.** `SplitEditor.tsx`'s `rowFromSplit` displayed a loaded
   split's amount as `(s.amount_cents / 100).toFixed(2)` without taking the
   magnitude first, so an expense split's negative `amount_cents` rendered
   as e.g. `-90.00` in the input. The live "fully assigned" check happened
   to be robust to this (it already normalized with `Math.abs`), so the UI
   showed "Fully assigned" and enabled Save - but `handleSave`'s payload
   construction was not equally defensive, and re-applied the parent's sign
   on top of the already-negative display value, sending the backend a
   total with the wrong sign. `splits::replace_for_transaction`'s own
   sum-must-equal-parent validation correctly rejected the mismatched save
   server-side (no corrupted data ever reached the database), but the user-
   visible result was a confusing failure on an unmodified split. Fixed by
   introducing one `magnitudeCents()` helper - `amountInput` is now read as
   a magnitude everywhere (initial load, live balance check, and save), with
   the parent's sign applied exactly once. Found by reopening "Edit split"
   on a transaction split earlier in this same verification pass and
   clicking Save without changing anything.

Both fixes were verified live immediately after: tagging and filtering
round-tripped correctly, and Save on a freshly-reopened, untouched split
succeeded.

## A pre-existing issue found, not fixed (out of scope for this batch)

Native-verifying credit-card reconciliation surfaced that
`TransactionForm.tsx`'s manual-entry amount sign
(`moneyIn ? magnitude : -magnitude`) has no account-type awareness at all,
for every batch back to Foundation Batch 1/Phase 2 (confirmed unchanged
across this file's entire git history). Per ADR-0004 and a passing backend
test (`credit_card_purchase_is_positive_and_still_classified_as_spending`),
a credit-card purchase must be **positive** `amount_cents` (balance owed
goes up); the form always sends **negative** for the intuitive "Money out"
choice regardless of account type. Manually entering a credit-card purchase
through the real form therefore understates what's owed rather than
increasing it - confirmed empirically (a $50 opening balance plus a
manually-entered $32 "Money out" purchase displayed $18.00 owed, not
$82.00). This is unrelated to anything Transaction Intelligence added or
touched - splits/payees/tags/bulk/rules/reconciliation-integration all sit
correctly on top of whatever balance an account already has - but it is a
real, pre-existing financial-correctness gap in manual liability-account
entry, out of this batch's scope to fix (Foundation-Batch-1-era form logic,
with net-worth/reports/reconciliation-fixture consequences needing their
own careful audit). Flagged as a separate follow-up task rather than fixed
or silently ignored.

## Tests

289 pre-existing Rust tests unchanged; 29 new (10 `splits.rs`, 5 `bulk.rs`,
5 `rules.rs` correction-to-rule, 4 `categories.rs`, 3 `transactions.rs`
payee/one-time/has_splits/filters, 1 `reports.rs` split-expansion, 1
`import.rs` Undo-with-splits regression) - 318 total. Coverage: exact-total
split success, under/over-allocation rejected with nothing written,
single-split and zero-amount rejection, edit-replaces-entirely, unsplit,
splitting never changes the parent's own ledger amount or account balance,
splits never touch the `transactions` table at all; bulk category/tag/one-
time/archive updates, atomic rollback on a mid-batch failure; payee/one-
time/tag/reconciliation-status filtering; category rename across
transactions/splits/rules, budget-conflict skipping, identical-name
rejection; all four correction scopes, account-scoping, and a regression
test proving a saved correction rule matches identically through the normal
import-staging path afterward; the split-expanding Reports UNION never
double-counting; the Undo-protects-a-split regression.

## Native verification

Real `tauri dev` binary, real WebView2 CDP (a fresh driver script, the
prior batches' having lived only in an ephemeral scratch location) -
screen-control/computer-use access was declined for this app, so every
interaction here is DOM-level scripting against the real running app and
its real SQLite-backed IPC, never simulated input; a safety copy of the
live app-data database was taken first. Disposable accounts ("Batch5 Test
Checking", "Batch5 Test Credit Card") archived afterward, alongside every
payee/tag/rule created during the pass, matching prior batches' convention.

Covered live, end to end: created payees and merged one into another,
confirming both transactions' payee reassigned and the source payee shows
"Merged" without disappearing; tagged and bulk-tagged/untagged
transactions, confirmed via the tag filter both before and after the fix
above; renamed a category and confirmed the returned counts and the
transactions table both updated; split a transaction and confirmed the
account balance was byte-identical before and after, and that Reports
expanded it into its two split categories with no double-counting; ran
every bulk action at least once, including the reconciled-selection warning
firing correctly and blocking on Cancel, then proceeding on Continue
Anyway; drove the correction-to-rule flow past + future, confirmed the rule
appeared in the same Rules UI CSV import uses, then ran a real CSV import
that matched it and confirmed the imported row was normalized automatically
with the other, non-matching row unaffected; confirmed Import Undo's
tag-based protection round-trip on a freshly-imported row; reconciled both
a checking and a credit-card test account end to end, confirmed the
"MIDAS is $X above/below the statement" language never forces sign
reasoning on the reader; confirmed merchant/category/one-time edits
(including via bulk actions) never invalidate a reconciliation while an
amount edit still does, and that splitting a reconciled transaction never
invalidates it either; killed and relaunched the real binary and confirmed
every one of the above (accounts, transactions, payees, tags, the rule,
reconciliation status) was byte-identical afterward. Accessibility: the
bulk toolbar carries `role="toolbar"`/`aria-label`, every selection
checkbox has a descriptive `aria-label`, every new dialog's
`aria-labelledby` resolves to a real heading, and the split editor's status
line carries `role="status"`. Screen sizes: 1280x800 (default) and 1024x640
both checked for the transactions table with the bulk toolbar active and
for the split editor - no page-level horizontal overflow at either size.

## Explicit non-goals (unchanged from the handoff)

Category Groups (documented prerequisite above, deferred), Fixed/Flexible/
Discretionary classification (same), Recurring Intelligence, Budget
redesign, Insights redesign, forecasting, mobile work, per-split tags
(the model would cleanly support it, but nothing in this batch's own scope
needs it yet), full transaction-table column customization (the smallest-
useful set of columns plus the existing opt-in running-balance column was
judged sufficient).

## Acceptance criteria

- [x] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` all
      pass (318 tests: 289 existing + 29 new).
- [x] `npm run check`-equivalent (format, typecheck, tests, build) passes.
- [x] Migration 0012 applies cleanly to a fresh database; a representative
      pre-migration transaction survives byte-identical.
- [x] Payee merge, category rename, tag CRUD, split invariants, and every
      bulk action live-verified end to end, not just unit-tested.
- [x] Bulk actions never bypass reconciliation warnings - live-verified
      with a mixed reconciled/unreconciled selection, both Cancel and
      Continue Anyway paths.
- [x] Splits never double-count in reporting and never affect account
      balance or reconciliation - live-verified, not just asserted by test.
- [x] Correction-to-rule live-verified against a real subsequent CSV
      import, confirming no regression to normal rule-staging behavior.
- [x] Import Undo protection live-verified for a freshly-imported,
      then-tagged transaction.
- [x] Two real, batch-introduced bugs found via native verification (tag
      sync, split re-save sign) were fixed and re-verified live before
      sign-off - not deferred.
- [x] A real, pre-existing (not batch-introduced) financial-correctness gap
      was found, was not silently fixed or silently ignored, and was
      flagged as a separate, explicit follow-up.
- [x] Accessibility and both required screen sizes verified live for every
      new UI surface.
- [x] All native-verification test data (accounts, payees, tags, rules)
      archived or deleted afterward, matching prior batches' convention.

## Owner decisions

None required. The Category Groups / Fixed-Flexible-Discretionary
deferrals were resolvable directly from the handoff's own explicit escape
hatches; the pre-existing credit-card sign gap was surfaced as a flagged
follow-up rather than requiring an in-the-moment decision, since fixing it
is a separate, larger piece of work with its own audit scope.

## Hard-stop conditions

None encountered. No destructive or irreversible action was taken against
the live application-data database - a safety copy was taken before this
batch's verification pass, all verification used disposable, clearly-named
test accounts on the same real, continuing app-data database prior batches
established, and all of it was archived or deleted afterward. Screen-
control/computer-use access to the app was requested and declined; the
verification approach was changed (to CDP-level DOM scripting) rather than
proceeding without the owner's input on how to continue.
