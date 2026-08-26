import type { TransactionType } from "../api/midas";

export const TRANSACTION_TYPES: TransactionType[] = [
  "purchase",
  "income",
  "transfer",
  "refund",
];

export const TRANSACTION_TYPE_LABELS: Record<TransactionType, string> = {
  purchase: "Purchase",
  income: "Income",
  transfer: "Transfer",
  refund: "Refund",
  // Neither is in TRANSACTION_TYPES - structural anchors only reconcile
  // creates (ADR-0007/ADR-0009), never user-selectable in the manual entry
  // form. Still need labels so they render correctly wherever transactions
  // are listed.
  opening_balance: "Opening Balance",
  balance_adjustment: "Balance Adjustment",
};
