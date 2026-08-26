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

// balance_cents is deliberately absent: it's computed live from the ledger,
// never set directly (ADR-0007). Use accountingApi.reconcileBalance instead.
export interface AccountInput {
  name: string;
  institution: string | null;
  account_type: AccountType;
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
export type TransactionType =
  | "purchase"
  | "income"
  | "transfer"
  | "refund"
  | "opening_balance"
  | "balance_adjustment";

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

// --- Accounting boundary (Phase 4A, ADR-0007) ---
export interface NetWorthBreakdown {
  total_assets_cents: number;
  total_liabilities_cents: number;
  net_worth_cents: number;
}

export interface NetWorthPoint {
  as_of_date: string;
  net_worth_cents: number;
}

export const accountingApi = {
  reconcileBalance: (
    accountId: number,
    knownCurrentBalanceCents: number,
    asOfDate: string,
  ) =>
    invoke<Account>("reconcile_account_balance", {
      accountId,
      knownCurrentBalanceCents,
      asOfDate,
    }),
  getNetWorth: () => invoke<NetWorthBreakdown>("get_net_worth"),
  getNetWorthHistory: () => invoke<NetWorthPoint[]>("get_net_worth_history"),
  getTotalSpending: (filter: TransactionFilter) =>
    invoke<number>("get_total_spending", { filter }),
};

// --- Budgets (Phase 4B) ---
export type BudgetClassification = "fixed" | "flexible";
export type BudgetStatus = "under" | "near" | "over";

export interface Budget {
  id: number;
  category: string;
  month: string;
  planned_amount_cents: number;
  classification: BudgetClassification;
  created_at: string;
  updated_at: string;
  actual_amount_cents: number;
  status: BudgetStatus;
}

export interface BudgetInput {
  category: string;
  month: string;
  planned_amount_cents: number;
  classification: BudgetClassification;
}

export const budgetsApi = {
  list: (month: string) => invoke<Budget[]>("list_budgets", { month }),
  create: (input: BudgetInput) => invoke<Budget>("create_budget", { input }),
  update: (id: number, input: BudgetInput) =>
    invoke<Budget>("update_budget", { id, input }),
  delete: (id: number) => invoke<void>("delete_budget", { id }),
};

// --- Savings Goals (Phase 4B) ---
export type GoalStatus = "in_progress" | "completed" | "overdue";

export interface SavingsGoal {
  id: number;
  name: string;
  target_amount_cents: number;
  current_amount_cents: number;
  target_date: string | null;
  reserved_amount_cents: number | null;
  is_archived: boolean;
  created_at: string;
  updated_at: string;
  status: GoalStatus;
}

export interface SavingsGoalInput {
  name: string;
  target_amount_cents: number;
  current_amount_cents: number;
  target_date: string | null;
  reserved_amount_cents: number | null;
}

export const savingsGoalsApi = {
  list: (includeArchived: boolean) =>
    invoke<SavingsGoal[]>("list_savings_goals", { includeArchived }),
  create: (input: SavingsGoalInput) =>
    invoke<SavingsGoal>("create_savings_goal", { input }),
  update: (id: number, input: SavingsGoalInput) =>
    invoke<SavingsGoal>("update_savings_goal", { id, input }),
  setArchived: (id: number, archived: boolean) =>
    invoke<SavingsGoal>("set_savings_goal_archived", { id, archived }),
};

// --- Pay Cycle (Phase 4B) ---
export type PaySchedule =
  | { schedule_type: "weekly"; anchor_date: string }
  | { schedule_type: "biweekly"; anchor_date: string }
  | { schedule_type: "monthly"; day_of_month: number }
  | {
      schedule_type: "semimonthly";
      day_of_month_1: number;
      day_of_month_2: number;
    };

export interface CycleSummary {
  schedule: PaySchedule;
  cycle_start: string;
  next_payday: string;
  spending_this_cycle_cents: number;
  obligations_before_payday_cents: number;
  prior_cycle_spending_cents: number;
}

export const payCycleApi = {
  getSchedule: () => invoke<PaySchedule | null>("get_pay_schedule"),
  setSchedule: (schedule: PaySchedule) =>
    invoke<void>("set_pay_schedule", { schedule }),
  getSummary: () => invoke<CycleSummary | null>("get_pay_cycle_summary"),
  getLikelyIncomeSeries: () =>
    invoke<RecurringSeries[]>("get_likely_income_series"),
};

// --- Safe to Spend (Phase 4B) ---
export type SafeToSpendResult =
  | {
      status: "complete";
      safe_to_spend_cents: number;
      eligible_cash_cents: number;
      obligations_before_payday_cents: number;
      reserved_goal_commitments_cents: number;
      owner_required_reserve_cents: number;
      next_payday: string;
    }
  | {
      status: "incomplete";
      reason: string;
      eligible_cash_cents: number;
    };

export const safeToSpendApi = {
  get: () => invoke<SafeToSpendResult>("get_safe_to_spend"),
  getReserve: () => invoke<number>("get_safe_to_spend_reserve"),
  setReserve: (cents: number) =>
    invoke<void>("set_safe_to_spend_reserve", { cents }),
};

// --- Overview (Phase 5) ---
export interface OverviewResult {
  safe_to_spend: SafeToSpendResult;
  upcoming_bills: RecurringSeries[];
  pay_cycle: CycleSummary | null;
  needs_review_count: number;
  needs_review_items: NeedsReviewItem[];
  savings_goals: SavingsGoal[];
  net_worth: NetWorthBreakdown;
  budget_health: Budget[];
  recent_transactions: Transaction[];
  income_this_month_cents: number;
  spending_this_month_cents: number;
  net_worth_trend: NetWorthPoint[];
}

export const overviewApi = {
  get: () => invoke<OverviewResult>("get_overview"),
};

// --- Reports and Trends (Phase 5) ---
export interface CategoryTotal {
  category: string;
  spent_cents: number;
}

export interface MonthlyFlow {
  month: string;
  income_cents: number;
  spending_cents: number;
}

export const reportsApi = {
  getSpendingByCategory: (from: string, to: string) =>
    invoke<CategoryTotal[]>("get_spending_by_category", { from, to }),
  getIncomeVsSpendingTrend: () =>
    invoke<MonthlyFlow[]>("get_income_vs_spending_trend"),
};
