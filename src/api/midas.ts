import { invoke } from "@tauri-apps/api/core";
import type { AccountType } from "../lib/accountTypes";

export interface Account {
  id: number;
  name: string;
  institution: string | null;
  account_type: AccountType;
  balance_cents: number;
  include_in_net_worth: boolean;
  notes: string | null;
  is_archived: boolean;
  last_synced_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface AccountInput {
  name: string;
  institution: string | null;
  account_type: AccountType;
  balance_cents: number;
  include_in_net_worth: boolean;
  notes: string | null;
}

export const accountsApi = {
  list: (includeArchived: boolean) =>
    invoke<Account[]>("list_accounts", { includeArchived }),
  create: (input: AccountInput) => invoke<Account>("create_account", { input }),
  update: (id: number, input: AccountInput) =>
    invoke<Account>("update_account", { id, input }),
  setArchived: (id: number, archived: boolean) =>
    invoke<Account>("set_account_archived", { id, archived }),
};

// --- Transactions ---
// Sign convention (ADR-0004): amount_cents is signed relative to the
// EFFECT ON THE ACCOUNT'S OWN BALANCE, not "was this spending" - positive
// increases the balance, negative decreases it, for every account type.
// transaction_type is what spending/income reports actually read.
export type TransactionType = "purchase" | "income" | "transfer" | "refund";

export interface Transaction {
  id: number;
  account_id: number;
  transaction_date: string;
  posted_date: string | null;
  amount_cents: number;
  transaction_type: TransactionType;
  raw_description: string;
  merchant: string;
  category: string | null;
  subcategory: string | null;
  notes: string | null;
  tags: string | null;
  is_pending: boolean | null;
  is_archived: boolean;
  needs_review: boolean;
  source_import_id: number | null;
  source_row_hash: string | null;
  transfer_pair_transaction_id: number | null;
  refund_of_transaction_id: number | null;
  created_at: string;
  updated_at: string;
}

export interface TransactionInput {
  account_id: number;
  transaction_date: string;
  posted_date: string | null;
  amount_cents: number;
  transaction_type: TransactionType;
  raw_description: string;
  merchant: string | null;
  category: string | null;
  subcategory: string | null;
  notes: string | null;
  tags: string | null;
  is_pending: boolean | null;
}

export interface TransactionFilter {
  account_id?: number | null;
  search?: string | null;
  category?: string | null;
  date_from?: string | null;
  date_to?: string | null;
  include_archived?: boolean;
  needs_review_only?: boolean;
}

export const transactionsApi = {
  list: (filter: TransactionFilter) =>
    invoke<Transaction[]>("list_transactions", { filter }),
  create: (input: TransactionInput) =>
    invoke<Transaction>("create_transaction", { input }),
  update: (id: number, input: TransactionInput) =>
    invoke<Transaction>("update_transaction", { id, input }),
  setArchived: (id: number, archived: boolean) =>
    invoke<Transaction>("set_transaction_archived", { id, archived }),
  delete: (id: number) => invoke<void>("delete_transaction", { id }),
};

// --- CSV import ---
export interface ColumnMapping {
  date_column: string | null;
  posted_date_column: string | null;
  description_column: string | null;
  amount_column: string | null;
  debit_column: string | null;
  credit_column: string | null;
  category_column: string | null;
}

export interface CsvPreview {
  headers: string[];
  row_count: number;
  sample_rows: Record<string, string>[];
  suggested_mapping: ColumnMapping;
  mapping_confident: boolean;
}

export type RowStatus =
  "valid" | "invalid" | "exact_duplicate" | "probable_duplicate";

export interface ProposedTransaction {
  transaction_date: string;
  posted_date: string | null;
  amount_cents: number;
  transaction_type: TransactionType;
  raw_description: string;
  merchant: string;
  category: string | null;
  source_row_hash: string;
}

export interface StagingRow {
  row_index: number;
  raw: Record<string, string>;
  status: RowStatus;
  errors: string[];
  proposed: ProposedTransaction | null;
  duplicate_of_transaction_id: number | null;
}

export interface StagingResult {
  rows: StagingRow[];
  valid_count: number;
  invalid_count: number;
  exact_duplicate_count: number;
  probable_duplicate_count: number;
}

export interface ImportSummary {
  import_id: number;
  committed_count: number;
  exact_duplicate_count: number;
  sent_to_review_count: number;
  invalid_count: number;
}

export const importApi = {
  previewCsv: (csvContent: string) =>
    invoke<CsvPreview>("preview_csv", { csvContent }),
  stage: (accountId: number, csvContent: string, mapping: ColumnMapping) =>
    invoke<StagingResult>("stage_import", { accountId, csvContent, mapping }),
  commit: (
    accountId: number,
    sourceFilename: string,
    csvContent: string,
    mapping: ColumnMapping,
    forceImportRowIndices: number[],
  ) =>
    invoke<ImportSummary>("commit_import", {
      accountId,
      sourceFilename,
      csvContent,
      mapping,
      forceImportRowIndices,
    }),
};

// --- Needs Review ---
export type IssueType =
  | "malformed_row"
  | "missing_required_field"
  | "uncertain_category"
  | "uncertain_merchant"
  | "suspected_duplicate"
  | "ambiguous_debit_credit"
  | "ambiguous_transfer"
  | "other";

export interface NeedsReviewItem {
  id: number;
  issue_type: IssueType;
  transaction_id: number | null;
  import_id: number | null;
  source_row_json: string | null;
  detected_reason: string;
  proposed_resolution_json: string | null;
  resolution_status: "open" | "resolved" | "dismissed";
  resolved_at: string | null;
  created_at: string;
}

export type ResolutionAction = "resolved" | "dismissed";

export const needsReviewApi = {
  list: (openOnly: boolean) =>
    invoke<NeedsReviewItem[]>("list_needs_review", { openOnly }),
  resolve: (id: number, action: ResolutionAction) =>
    invoke<NeedsReviewItem>("resolve_needs_review", { id, action }),
};

// --- Merchant rules ---
export type MatchType = "exact" | "contains" | "starts_with";

export interface MerchantRule {
  id: number;
  match_type: MatchType;
  match_value: string;
  account_id: number | null;
  normalized_merchant: string | null;
  category: string | null;
  subcategory: string | null;
  priority: number;
  created_at: string;
  updated_at: string;
}

export interface MerchantRuleInput {
  match_type: MatchType;
  match_value: string;
  account_id: number | null;
  normalized_merchant: string | null;
  category: string | null;
  subcategory: string | null;
  priority: number;
}

export const rulesApi = {
  list: () => invoke<MerchantRule[]>("list_rules"),
  create: (input: MerchantRuleInput) =>
    invoke<MerchantRule>("create_rule", { input }),
  delete: (id: number) => invoke<void>("delete_rule", { id }),
};

// --- Recurring series / bills ---
export type Cadence =
  "weekly" | "biweekly" | "monthly" | "quarterly" | "annual";
export type SeriesStatus = "proposed" | "confirmed" | "rejected";

export interface RecurringSeries {
  id: number;
  account_id: number;
  merchant: string;
  cadence: Cadence;
  expected_amount_cents: number;
  next_expected_date: string;
  category: string | null;
  status: SeriesStatus;
  created_at: string;
  updated_at: string;
  last_occurrence_date: string | null;
  last_amount_cents: number | null;
  is_price_changed: boolean;
  is_possibly_missed: boolean;
}

export const recurringApi = {
  list: (accountId: number | null) =>
    invoke<RecurringSeries[]>("list_recurring", { accountId }),
  detect: (accountId: number) =>
    invoke<RecurringSeries[]>("detect_recurring", { accountId }),
  confirm: (id: number) => invoke<RecurringSeries>("confirm_recurring", { id }),
  reject: (id: number) => invoke<RecurringSeries>("reject_recurring", { id }),
};
