export type AccountType =
  | "checking"
  | "savings"
  | "credit_card"
  | "loan"
  | "cash"
  | "investment"
  | "retirement"
  | "property"
  | "other_liability";

export const ACCOUNT_TYPES: AccountType[] = [
  "checking",
  "savings",
  "credit_card",
  "loan",
  "cash",
  "investment",
  "retirement",
  "property",
  "other_liability",
];

export const ACCOUNT_TYPE_LABELS: Record<AccountType, string> = {
  checking: "Checking",
  savings: "Savings",
  credit_card: "Credit Card",
  loan: "Loan",
  cash: "Cash",
  investment: "Investment",
  retirement: "Retirement",
  property: "Property / Manual Asset",
  other_liability: "Other Liability",
};

// Credit cards and loans are liabilities: a positive balance means money
// owed, not money held. Kept separate from net-worth inclusion, which is
// the user's own choice per account.
export const LIABILITY_TYPES: ReadonlySet<AccountType> = new Set([
  "credit_card",
  "loan",
  "other_liability",
]);

// Genuinely liquid cash - the same set Safe to Spend and the deterministic
// Forecast (Batch 8) both treat as eligible. Never available credit.
export const CASH_TYPES: ReadonlySet<AccountType> = new Set([
  "checking",
  "savings",
  "cash",
]);

// ADR-0004: amount_cents is signed relative to the effect on the account's
// OWN balance, never "was this spending" - uniformly for every account
// type. For an asset account, the everyday meaning of "money in"/"money
// out" already matches this (income increases the balance, spending
// decreases it). For a liability account the same everyday phrases invert:
// a charge/purchase increases what's owed (positive, per
// credit_card_purchase_is_positive_and_still_classified_as_spending in
// transactions.rs) even though the user experiences it as spending, and a
// payment/credit decreases what's owed (negative) even though it reads
// like money arriving. CSV/OFX import already apply this same rule
// (money.rs::combine_debit_credit, import.rs::default_transaction_type) -
// this is manual entry's equivalent, so the two never disagree about what
// the same real-world transaction should be stored as.
export function signedAmountCents(
  magnitudeCents: number,
  moneyIn: boolean,
  accountType: AccountType,
): number {
  const increasesBalance = LIABILITY_TYPES.has(accountType)
    ? !moneyIn
    : moneyIn;
  return increasesBalance ? magnitudeCents : -magnitudeCents;
}

// Inverse of signedAmountCents - derives the "money in" direction from an
// already-stored amount, for initializing the direction toggle when
// opening an existing transaction (of either sign, from any source) for
// editing.
export function amountIsMoneyIn(
  amountCents: number,
  accountType: AccountType,
): boolean {
  const increasesBalance = amountCents >= 0;
  return LIABILITY_TYPES.has(accountType)
    ? !increasesBalance
    : increasesBalance;
}
