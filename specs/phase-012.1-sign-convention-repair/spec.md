# Phase 012.1: Credit card sign convention repair

- Status: COMPLETE
- Owner gate: APPROVED 2026-09-12 (owner handoff: "MIDAS Development Batch 5.1 - Credit Card Sign Convention Repair + Financial Regression Audit")

## Goal

> Manual credit-card transactions, imported credit-card transactions,
> transfers/payments, reports, net worth, reconciliation, and running
> balances must all agree on one consistent MIDAS accounting convention -
> proven, not assumed, before more financial intelligence is built on top.

A targeted correctness repair, not a feature batch. Triggered by a finding
from Batch 5's own native verification (specs/phase-012, "A pre-existing
issue found, not fixed"): a manually-entered credit-card purchase
understated what was owed instead of increasing it.

## Reconnaissance: tracing the real convention end to end

ADR-0004 (`docs/decisions/0004-transaction-sign-convention.md`) already
states the canonical rule precisely and correctly: `amount_cents` is always
signed relative to the effect on the owning account's own balance - for a
liability (credit_card/loan/other_liability), a purchase/charge is
_positive_ (owed goes up) and a payment/credit is _negative_ (owed goes
down). The ADR was not wrong; nothing about it needed changing. The defect
was a single implementation gap that never conformed to it.

Traced every consumer directly (not inferred from UI labels) to establish
exactly what was and was not affected:

**Correct already, confirmed by reading the code:**

- `money.rs::combine_debit_credit` - already account-type-aware for the
  separate debit/credit CSV column case.
- `import.rs`'s single-signed-amount CSV path - trusts the file's own
  already-balance-relative sign as-is, per the ADR's own rationale.
- `import.rs::default_transaction_type` - already infers
  Income/Purchase/Transfer correctly per (sign, is_liability).
- `ofx.rs`/`import.rs`'s OFX/QFX path - `TRNAMT` trusted as-is, reuses the
  identical `default_transaction_type`, with an explicit `XFER` override.
- `transactions.rs::create`/`update` - intentionally trust whatever
  `amount_cents` they're given (the same design CSV/OFX already rely on);
  there was never backend-side sign correction to be broken.
- `accounting::spending_effect` (and everything built on it - `reports.rs`,
  `budgets.rs`, `safe_to_spend.rs`) - classifies by `transaction_type` and
  takes `.abs()`, never trusting the raw sign. **This is why Batch 5's own
  native-verification spending/budget/Safe-to-Spend numbers looked correct
  even with the bug present** - those figures were never at risk.
- `safe_to_spend.rs::eligible_cash` - filters to
  Checking/Savings/Cash before summing; a liability's `balance_cents` is
  never read at all, buggy or not.
- `splits.rs`/`SplitEditor.tsx` - mirrors the _parent's_ own sign
  unconditionally, never independently re-derives "is this money in or
  out," so it was already correct and needed no change.

**Genuinely wrong, confirmed by reading the code:**

- `TransactionForm.tsx` alone. `amount_cents: moneyIn ? magnitude :
-magnitude` had zero account-type awareness - present, unchanged, since
  this file's very first commit (`be52eed`, Phase 2/Foundation Batch 1;
  confirmed via `git log`), not a regression from any later batch. Every
  raw-sign-based consumer downstream was affected for a liability account
  with a manually-entered transaction: `accounts::balance_cents`
  (`SUM(amount_cents)`), `accounting::net_worth_breakdown`/
  `net_worth_as_of`/`net_worth_history`, and
  `reconciliation::cleared_balance_as_of`/`running_balances`.

This precise split - type-based figures immune, raw-sign-based balance
figures affected - meant the actual blast radius was narrower than "every
number is possibly wrong," and the fix could be scoped to exactly where the
sign is decided: the manual entry form, once, rather than auditing or
changing any downstream consumer.

## The fix: one shared, tested primitive

`lib/accountTypes.ts` gained two pure functions next to the existing
`LIABILITY_TYPES` set:

- `signedAmountCents(magnitudeCents, moneyIn, accountType)` - for an asset,
  `moneyIn` maps directly to sign (unchanged behavior). For a liability, it
  inverts: `moneyIn` (a payment/credit) is negative, `!moneyIn` (a
  charge/purchase) is positive.
- `amountIsMoneyIn(amountCents, accountType)` - the exact inverse, for
  initializing the direction toggle when opening an existing transaction
  (of either sign, from any source) for editing.

`TransactionForm.tsx` now derives `selectedAccountType` from the currently
selected account on every render (never stored state, so switching an
existing transaction's account mid-edit - section 7's explicit concern -
immediately uses the new account's own rule, with no stale sign residue
from the account it used to belong to) and uses these two functions for
both the direction toggle's initial state and the submitted `amount_cents`.
The direction radio labels also became account-type-aware - "Charge or
purchase (increases what you owe)" / "Payment or credit (decreases what
you owe)" for a liability, unchanged "Money out (spending, transfer out)" /
"Money in (income, refund, transfer in)" for an asset - so the user keeps
picking based on the real-world action, never internal sign reasoning
(section 8), while a refund on a liability naturally also reads as
"Payment or credit" (both reduce what's owed the same way).

No backend change was made or needed - manual entry now does, before ever
calling `create()`/`update()`, exactly what CSV/OFX already did: decide the
correct sign once, upstream, then hand the backend an already-canonical
value. This is the "shared normalization" section 9 asked for, achieved
without literal cross-language code sharing (Rust and TypeScript can't
share a function directly) and without touching the ledger engine itself.

## Existing data safety (section 22)

Checked the real, live application-data database directly (read-only,
`node:sqlite`) before writing any code: every account in it - all nine
rows - is an already-archived disposable test/sample fixture left over
from prior batches' own native verification (`Sample Credit Card`, `Sample
Rewards Card`, `Batch5 Test Credit Card`, etc.). **There is no active, real,
owner-entered financial data in the database at all**, liability or
otherwise, and therefore no historical remediation, migration, or review
workflow to design. The one manually-entered, bug-affected transaction
found (`Batch5 Test Credit Card`'s "Restaurant XYZ", stored as -3200
instead of +3200) is disposable test data from this same project's own
Batch 5 verification pass, already archived; it was left exactly as found,
per this section's explicit "never bulk-flip existing transaction signs"
instruction - there was nothing ambiguous to decide, so no owner escalation
was needed. No migration was written; this was a purely behavioral fix.

## Tests

325 Rust tests (318 existing + 7 new) - clean clippy/fmt. New coverage,
targeted at the confirmed-gap list from re-auditing existing liability
coverage first (`accounting.rs` already had single-purchase liability
tests; `reconciliation.rs` already covered `cleared_balance_as_of`/
`running_balances` for a card):

- `accounting.rs`: a full purchase+refund+payment cycle on one credit card
  in one test (net worth falls on the charge, improves on the refund, is
  _unchanged_ by the payment itself); `net_worth_history`'s multi-month
  trend with a liability account (previously Checking-only); a direct
  manual-vs-imported equivalence test asserting byte-identical
  balance/net-worth/spending effects regardless of `source_import_id`.
- `transactions.rs`: `update()` moving a transaction from checking to a
  credit card correctly uses the new account's own balance with no stale
  sign (previously only `create()` had any liability coverage).
- `reconciliation.rs`: `preview()` (not just `complete()`) reaching exactly
  zero difference for a card statement combining an opening balance,
  purchase, refund, and payment together.
- `reports.rs`/`budgets.rs`: previously zero liability-account coverage in
  either file - a credit-card purchase's positive `amount_cents` now has an
  explicit test proving it lands in its category/budget at the correct
  magnitude, and that the payment leg never counts a second time.

`tests/accountTypes.test.mjs` (new, TypeScript side): asset and liability
direction/sign round-trips, and an explicit "manual and imported converge"
example matching the exact values CSV/OFX already produce.

## Native verification

Real `tauri dev` binary, real WebView2 CDP (the same driver built during
Batch 5's own verification pass), disposable accounts, a safety copy of the
live database taken first, all test accounts archived afterward.

Walked the full section 20 checklist on `B51 Test Checking` (asset) and
`B51 Test Credit Card` (liability): a manual $100.00 purchase displayed and
stored as **positive** `$100.00`(previously would have shown negative),
card balance $100.00, net worth fell from $1,000.00 to $900.00; a $25.00
refund correctly displayed as`-$25.00`, card balance $75.00, net worth
_improved_ to $925.00, Dining spending correctly netted the refund; a
$50.00 checking-to-card payment moved checking to $950.00 and the card to
$25.00 while net worth stayed at exactly $925.00 (unchanged by the
transfer) and Dining spending was unaffected; a further $60.00 purchase
split into Electronics/Household left the card balance at $85.00
byte-identical before and after splitting, with Reports showing the two
split categories at their correct amounts and no double count; the account
reconciled to a $85.00 statement with a $0.00 difference and completed
successfully. Restarted the real binary and confirmed every balance, net
worth figure, and the reconciliation history entry were byte-identical
afterward. Finally, built and imported a real QFX fixture
(`b51-manual-vs-import.qfx`) carrying the identical $100/-$25/-$50
sequence into a third disposable account: it landed at $25.00, exactly
matching the manually-entered account's balance at the equivalent point in
its own history - direct, empirical proof that manual entry and QFX import
now converge on identical results for the same real-world transactions.

The account-type-aware radio labels ("Charge or purchase (increases what
you owe)" / "Payment or credit (decreases what you owe)") were confirmed
rendering correctly for the credit-card account via the live DOM, not
assumed from the source change alone.

## Simplification review

No manual-only or import-only sign hack was left behind: manual entry now
uses the same small, testable, documented rule
(`lib/accountTypes.ts::signedAmountCents`) that CSV/OFX already encode in
their own Rust-side inference, rather than each guessing independently.
No duplicated liability logic was introduced - one new pair of pure
functions, called from exactly one place (`TransactionForm.tsx`), is the
entire fix; nothing in Reports/Budgets/Safe-to-Spend/Reconciliation needed
to change or compensate for anything, because they were never the problem.
No backend change, no new migration, no new dependency.

## Explicit non-goals (unchanged from the handoff)

Recurring Intelligence, new Budget UX, Insights redesign, forecasting,
Category Groups, Financial Signals, weekly/monthly review, AI.

## Acceptance criteria

- [x] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` all
      pass (325 tests: 318 existing + 7 new). `cargo audit`: same accepted
      baseline, zero new findings, zero dependency changes.
- [x] Frontend gate (prettier, `tsc --noEmit`, `node --test`, `vite build`)
      passes, including 4 new tests for the shared sign helper.
- [x] A manual credit-card purchase now stores and displays as positive
      (increases what's owed), live-verified against the real app, not
      just unit-tested.
- [x] Net worth, Reports/spending, Budgets, Safe to Spend, and
      Reconciliation all live-verified correct across a full
      purchase/refund/payment/split cycle on a real liability account.
- [x] A real QFX import and an equivalent manual entry sequence were
      directly compared and produced identical final balances.
- [x] No historical data required migration or review - confirmed by
      reading the live database directly, not assumed.
- [x] No new backend surface, no new migration, no new dependency.
- [x] All native-verification test data archived afterward.

## Owner decisions

None required. The "does existing data need remediation" question (the
handoff's one named stop-and-ask condition) resolved unambiguously by
reading the live database: it holds no real financial data at all, only
already-archived test fixtures from prior batches' own verification.

## Hard-stop conditions

None encountered. No destructive or irreversible action was taken - a
safety copy of the live database was taken before this batch's
verification pass, all verification used disposable, clearly-named test
accounts, and every one was archived afterward. No existing transaction's
stored sign was altered.
