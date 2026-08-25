# ADR-0004: Transaction amount sign convention

- Status: accepted
- Date: 2026-08-25
- Supersedes: none

## Context and problem

Phase 2 needs one canonical, testable rule for what a transaction's signed `amount_cents` means, consistent across every account type (asset and liability), manual entry, CSV import, and every future report (spending, income, net worth, Safe to Spend). Getting this wrong, or letting importers/UI invent their own rule, is exactly the class of bug the product handoff calls out by name: credit-card payments must never become double-counted spending, and credit availability must never look like cash.

## Options considered

1. **Balance-relative sign**: `amount_cents` is signed relative to _this account's own balance_ - positive always means the balance increases, negative always means it decreases - for every account type uniformly. A separate `transaction_type` field (`purchase` / `income` / `transfer` / `refund`) carries the semantic classification used for spending/income reporting.
2. **Spending-relative sign**: negative always means "this was spending," positive always means "this was not spending," regardless of account type.
3. **Unsigned magnitude + direction enum**: store `amount_cents` as a positive magnitude and a separate `direction` (debit/credit) field.

## Decision

Option 1. `transactions.amount_cents` (`INTEGER`, never `REAL`/float) is always signed relative to the effect on the owning account's own `accounts.balance_cents`:

- **Asset accounts** (checking, savings, cash, investment, retirement, property): positive = money in (income, refund, transfer in), negative = money out (purchase, transfer out).
- **Liability accounts** (credit_card, loan, other_liability): positive = balance owed increases (a purchase/charge), negative = balance owed decreases (a payment, which is modeled as a `transfer` - see ADR context below, not a separate "payment" type).

`transaction_type` (`purchase` | `income` | `transfer` | `refund`) is the field every report actually reads to decide what counts as spending or income - never the raw sign. A credit-card purchase has a _positive_ `amount_cents` (balance went up) but _is_ spending because `transaction_type = purchase`. A credit-card payment from checking is `transaction_type = transfer` on both legs (checking: negative/money out; credit card: negative/balance down) and is excluded from spending on both sides.

## Rationale

- **`balance = initial_balance + sum(that account's transaction amount_cents)` becomes a true, auditable invariant**, for every account type without exception. This is a direct, testable form of "explainable" and "evidence over assumption" - a user (or MIDAS itself) can always reconstruct a stated balance from transaction history.
- **It matches how real bank/card CSV exports already sign amounts** in practice: checking exports commonly show deposits positive/withdrawals negative, and card exports commonly show charges positive/payments-or-credits negative - both already balance-relative. Choosing Option 1 means the common CSV case needs no institution-specific sign-flipping; Phase 2's parser can trust a single signed-amount column as given (see Phase 2 spec, Amount Parsing) in the common case, and only needs account-type-aware sign logic for the separate debit/credit-column case, where "debit"/"credit" banking terminology itself flips meaning between asset and liability accounts.
- **Option 2 (spending-relative sign) breaks the balance invariant**: a credit-card purchase would need to be negative to mean "spending," but the balance owed actually goes _up_ on a purchase, so `balance = sum(amounts)` would go backwards for every liability account. Every future net-worth/reconciliation calculation would need special-casing per account type instead of a uniform sum.
- **Option 3 (magnitude + direction)** carries the same information as a signed integer with an extra field and extra invariant to keep consistent (magnitude must always be non-negative; direction must always agree with sign) for no additional safety - a plain signed `i64` is simpler and is exactly what "integer minor currency units, never floating point" already implies.
- Credit-card payments are modeled as `transfer`, not a distinct `payment` type, because the product handoff's own phrasing treats them as one concept ("a transfer/payment of liability, not new spending") and a payment _is_ a transfer between two owned accounts (checking -> credit card) in every way that matters for reporting: excluded from spending on both legs, no separate rule needed.

## Consequences and accepted tradeoffs

- CSV import must know an account's type (asset vs. liability) to correctly sign amounts parsed from separate debit/credit columns; it does not need this for a single already-signed amount column, which is the common case. The exact rule lives in `src-tauri/src/money.rs::combine_debit_credit` and is covered by tests.
- `transaction_type` inference on import is a deterministic default (positive amount on an asset account -> `income`; negative -> `purchase`; positive on a liability -> `purchase`; negative on a liability -> `transfer`), never a guess dressed up as certainty - the user can always correct it after import (`update_transaction`), matching "a user correction wins over an inferred result."
- Automatic detection of which two transactions form a transfer's two legs, and which purchase a refund relates to, is **not** implemented in Phase 2. The schema carries the relationship (`transfer_pair_transaction_id`, `refund_of_transaction_id`, both nullable), but only manual correction populates them this phase - consistent with "avoid claiming automatic transfer matching is complete unless it actually is."
