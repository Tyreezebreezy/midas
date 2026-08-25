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
