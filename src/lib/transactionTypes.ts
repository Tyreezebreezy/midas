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
};
