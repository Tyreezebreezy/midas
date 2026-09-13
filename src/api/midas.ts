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
  external_transaction_id: string | null;
  reconciliation_status: ReconciliationStatus;
  reconciliation_id: number | null;
  payee_id: number | null;
  is_one_time: boolean;
  has_splits: boolean;
  created_at: string;
  updated_at: string;
}

export type ReconciliationStatus = "unreconciled" | "cleared" | "reconciled";

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
  payee_id: number | null;
  is_one_time: boolean;
}

export interface TransactionFilter {
  account_id?: number | null;
  search?: string | null;
  category?: string | null;
  date_from?: string | null;
  date_to?: string | null;
  include_archived?: boolean;
  needs_review_only?: boolean;
  source_import_id?: number | null;
  payee_id?: number | null;
  tag_id?: number | null;
  reconciliation_status?: ReconciliationStatus | null;
  is_one_time?: boolean | null;
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
  setReconciliationStatus: (id: number, status: ReconciliationStatus) =>
    invoke<Transaction>("set_transaction_reconciliation_status", {
      id,
      status,
    }),
  delete: (id: number) => invoke<void>("delete_transaction", { id }),
  exportCsv: () => invoke<string>("export_transactions_csv"),
};

// --- Import Engine v2: CSV source adapter, matching, profiles, history ---
export interface ColumnMapping {
  date_column: string | null;
  posted_date_column: string | null;
  description_column: string | null;
  amount_column: string | null;
  debit_column: string | null;
  credit_column: string | null;
  category_column: string | null;
  external_id_column: string | null;
  invert_amount: boolean;
}

export type ImportSourceType = "csv" | "ofx" | "qfx";

export interface ImportProfile {
  id: number;
  name: string;
  account_id: number | null;
  source_type: ImportSourceType;
  column_mapping: ColumnMapping;
  is_archived: boolean;
  created_at: string;
  updated_at: string;
}

export interface ImportProfileInput {
  name: string;
  account_id: number | null;
  source_type: ImportSourceType;
  column_mapping: ColumnMapping;
}

export const importProfilesApi = {
  list: (includeArchived: boolean) =>
    invoke<ImportProfile[]>("list_import_profiles", { includeArchived }),
  create: (input: ImportProfileInput) =>
    invoke<ImportProfile>("create_import_profile", { input }),
  update: (id: number, input: ImportProfileInput) =>
    invoke<ImportProfile>("update_import_profile", { id, input }),
  delete: (id: number) => invoke<void>("delete_import_profile", { id }),
  setArchived: (id: number, archived: boolean) =>
    invoke<ImportProfile>("set_import_profile_archived", { id, archived }),
};

// Conservative profile recognition (product handoff section 8) - never a
// silent reinterpretation, only ever "clear" when exactly one saved profile
// fits this file's actual headers.
export type ProfileRecognition =
  | { outcome: "clear"; profile: ImportProfile }
  | { outcome: "ambiguous"; profiles: ImportProfile[] }
  | { outcome: "none" };

export interface CsvPreview {
  headers: string[];
  row_count: number;
  sample_rows: Record<string, string>[];
  suggested_mapping: ColumnMapping;
  mapping_confident: boolean;
  profile_recognition: ProfileRecognition;
}

// The 4 simple, user-facing statuses (product handoff section 10). Duplicate
// covers both a byte-identical row and a high-confidence "strong match" -
// only a byte-identical/external-ID match is ever auto-skipped without
// asking; a strong match still goes through possible_match's review/force
// mechanism (see match_detail for which kind it actually was).
export type RowStatus = "new" | "duplicate" | "possible_match" | "needs_review";

export interface NormalizedCandidateTransaction {
  source_type: ImportSourceType;
  account_id: number;
  transaction_date: string;
  posted_date: string | null;
  amount_cents: number;
  transaction_type: TransactionType;
  raw_description: string;
  merchant: string;
  category: string | null;
  subcategory: string | null;
  external_transaction_id: string | null;
  candidate_payee_id: number | null;
  source_row_hash: string;
}

export interface StagingRow {
  row_index: number;
  raw: Record<string, string>;
  status: RowStatus;
  errors: string[];
  candidate: NormalizedCandidateTransaction | null;
  duplicate_of_transaction_id: number | null;
  match_detail: string | null;
}

export interface StagingResult {
  rows: StagingRow[];
  new_count: number;
  needs_review_count: number;
  duplicate_count: number;
  possible_match_count: number;
}

export interface ImportSummary {
  import_id: number;
  committed_count: number;
  duplicate_count: number;
  sent_to_review_count: number;
  invalid_count: number;
  excluded_count: number;
}

export const importApi = {
  previewCsv: (csvContent: string, accountId: number | null) =>
    invoke<CsvPreview>("preview_csv", { csvContent, accountId }),
  stage: (accountId: number, csvContent: string, mapping: ColumnMapping) =>
    invoke<StagingResult>("stage_import", { accountId, csvContent, mapping }),
  commit: (
    accountId: number,
    sourceFilename: string,
    csvContent: string,
    mapping: ColumnMapping,
    importProfileId: number | null,
    forceImportRowIndices: number[],
    excludedRowIndices: number[],
  ) =>
    invoke<ImportSummary>("commit_import", {
      accountId,
      sourceFilename,
      csvContent,
      mapping,
      importProfileId,
      forceImportRowIndices,
      excludedRowIndices,
    }),
};

// --- Import History / Undo ---
export interface ImportRecord {
  id: number;
  account_id: number;
  account_name: string;
  source_type: string;
  source_filename: string;
  import_profile_id: number | null;
  import_profile_name: string | null;
  imported_at: string;
  row_count: number;
  committed_count: number;
  duplicate_count: number;
  needs_review_count: number;
  excluded_count: number;
  undone_at: string | null;
  undone_removed_count: number | null;
  remaining_transaction_count: number;
}

export interface UndoPreview {
  import_id: number;
  total_transaction_count: number;
  removable_count: number;
  protected_count: number;
}

export interface UndoResult {
  import_id: number;
  removed_count: number;
  protected_count: number;
}

export const importHistoryApi = {
  list: () => invoke<ImportRecord[]>("list_imports"),
  previewUndo: (importId: number) =>
    invoke<UndoPreview>("preview_undo_import", { importId }),
  undo: (importId: number) => invoke<UndoResult>("undo_import", { importId }),
};

// --- OFX/QFX source adapter (Import Engine v2 batch 3) ---
export interface OfxAccountPreview {
  statement_index: number;
  masked_account_id: string;
  kind: "bank" | "credit_card";
  account_type: string | null;
  currency: string | null;
  existing_mapping_account_id: number | null;
  transaction_count: number;
}

export interface OfxPreview {
  accounts: OfxAccountPreview[];
  source_type: ImportSourceType;
}

export const ofxApi = {
  preview: (ofxContent: string, sourceFilename: string) =>
    invoke<OfxPreview>("preview_ofx", { ofxContent, sourceFilename }),
  stage: (
    accountId: number,
    ofxContent: string,
    statementIndex: number,
    sourceFilename: string,
  ) =>
    invoke<StagingResult>("stage_ofx", {
      accountId,
      ofxContent,
      statementIndex,
      sourceFilename,
    }),
  commit: (
    accountId: number,
    sourceFilename: string,
    ofxContent: string,
    statementIndex: number,
    rememberAccountMapping: boolean,
    forceImportRowIndices: number[],
    excludedRowIndices: number[],
  ) =>
    invoke<ImportSummary>("commit_ofx_import", {
      accountId,
      sourceFilename,
      ofxContent,
      statementIndex,
      rememberAccountMapping,
      forceImportRowIndices,
      excludedRowIndices,
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
  applyCorrection: (ruleInput: MerchantRuleInput, scope: CorrectionScope) =>
    invoke<CorrectionResult>("apply_correction_command", { ruleInput, scope }),
};

// --- Correction-to-rule workflow (Transaction Intelligence, specs/phase-012) ---
export type CorrectionScope = "future_only" | "past_only" | "past_and_future";

export interface CorrectionResult {
  rule: MerchantRule | null;
  past_transactions_updated: number;
}

// --- Recurring series / bills (Batch 6, specs/phase-014) ---
export type Cadence =
  "weekly" | "biweekly" | "monthly" | "quarterly" | "annual" | "semimonthly";
export type RecurringDirection = "expense" | "income" | "transfer";
export type RecurringKind =
  "subscription" | "bill" | "membership" | "recurring_income" | "other";
export type SeriesStatus = "proposed" | "confirmed" | "rejected" | "inactive";
export type OccurrenceStatus =
  "upcoming" | "due" | "matched" | "missed" | "skipped" | "needs_review";

export interface RecurringSeries {
  id: number;
  account_id: number;
  payee_id: number | null;
  merchant: string;
  direction: RecurringDirection;
  kind: RecurringKind;
  cadence: Cadence;
  semimonthly_day_1: number | null;
  semimonthly_day_2: number | null;
  expected_amount_cents: number;
  next_expected_date: string;
  category: string | null;
  status: SeriesStatus;
  planning_to_cancel: boolean;
  notes: string | null;
  created_source: string;
  created_at: string;
  updated_at: string;
  // --- computed live from linked transactions, never separately stored ---
  last_occurrence_date: string | null;
  last_amount_cents: number | null;
  min_amount_cents: number | null;
  max_amount_cents: number | null;
  median_amount_cents: number | null;
  occurrence_count: number;
  is_price_changed: boolean;
  previous_typical_amount_cents: number | null;
  is_possibly_missed: boolean;
  monthly_equivalent_cents: number;
  annual_equivalent_cents: number;
}

export interface PatternEditInput {
  merchant: string;
  payee_id: number | null;
  kind: RecurringKind;
  category: string | null;
  cadence: Cadence;
  semimonthly_day_1: number | null;
  semimonthly_day_2: number | null;
  expected_amount_cents: number;
  next_expected_date: string;
  notes: string | null;
}

export interface PriceHistoryPoint {
  transaction_id: number;
  date: string;
  amount_cents: number;
}

export interface RecurringOccurrence {
  id: number;
  series_id: number;
  expected_date: string;
  expected_amount_cents: number;
  status: OccurrenceStatus;
  matched_transaction_id: number | null;
  created_at: string;
  updated_at: string;
  // Denormalized from the parent series for list/calendar display.
  merchant: string;
  direction: RecurringDirection;
  kind: RecurringKind;
  account_id: number;
}

export const recurringApi = {
  list: (accountId: number | null) =>
    invoke<RecurringSeries[]>("list_recurring", { accountId }),
  detect: (accountId: number) =>
    invoke<RecurringSeries[]>("detect_recurring", { accountId }),
  confirm: (id: number) => invoke<RecurringSeries>("confirm_recurring", { id }),
  reject: (id: number) => invoke<RecurringSeries>("reject_recurring", { id }),
  setInactive: (id: number) =>
    invoke<RecurringSeries>("set_recurring_inactive", { id }),
  reactivate: (id: number) =>
    invoke<RecurringSeries>("reactivate_recurring", { id }),
  setPlanningToCancel: (id: number, planningToCancel: boolean) =>
    invoke<RecurringSeries>("set_recurring_planning_to_cancel", {
      id,
      planningToCancel,
    }),
  createManual: (
    accountId: number,
    direction: RecurringDirection,
    input: PatternEditInput,
  ) =>
    invoke<RecurringSeries>("create_manual_recurring", {
      accountId,
      direction,
      input,
    }),
  updatePattern: (id: number, input: PatternEditInput) =>
    invoke<RecurringSeries>("update_recurring_pattern", { id, input }),
  moveAccount: (id: number, newAccountId: number) =>
    invoke<RecurringSeries>("move_recurring_account", { id, newAccountId }),
  priceHistory: (seriesId: number) =>
    invoke<PriceHistoryPoint[]>("get_recurring_price_history", { seriesId }),
  listOccurrences: (seriesId: number) =>
    invoke<RecurringOccurrence[]>("list_recurring_occurrences", { seriesId }),
  getOccurrence: (occurrenceId: number) =>
    invoke<RecurringOccurrence | null>("get_recurring_occurrence", {
      occurrenceId,
    }),
  upcoming: (from: string, to: string, accountId: number | null) =>
    invoke<RecurringOccurrence[]>("list_upcoming_recurring", {
      from,
      to,
      accountId,
    }),
  skipOccurrence: (occurrenceId: number) =>
    invoke<RecurringOccurrence>("skip_recurring_occurrence", { occurrenceId }),
  linkOccurrence: (occurrenceId: number, transactionId: number) =>
    invoke<RecurringOccurrence>("link_recurring_occurrence", {
      occurrenceId,
      transactionId,
    }),
  unlinkOccurrence: (occurrenceId: number) =>
    invoke<RecurringOccurrence>("unlink_recurring_occurrence", {
      occurrenceId,
    }),
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

// --- Budgets (Phase 4B, matured into Budget Intelligence in Batch 7) ---
export type BudgetClassification = "fixed" | "flexible" | "discretionary";
export type BudgetStatus = "under" | "near" | "over";
export type PaceStatus = "on_track" | "faster" | "slower";

export interface PaceInfo {
  percent_used: number;
  percent_elapsed: number;
  status: PaceStatus;
}

export interface Budget {
  id: number;
  category: string;
  month: string;
  planned_amount_cents: number;
  classification: BudgetClassification;
  rollover_enabled: boolean;
  created_at: string;
  updated_at: string;
  actual_amount_cents: number;
  status: BudgetStatus;
  // --- Batch 7: computed live, never stored ---
  rollover_in_cents: number;
  effective_amount_cents: number;
  typical_amount_cents: number | null;
  typical_months_of_history: number;
  pace: PaceInfo | null;
}

export interface BudgetInput {
  category: string;
  month: string;
  planned_amount_cents: number;
  classification: BudgetClassification;
  rollover_enabled: boolean;
}

export interface BudgetSuggestion {
  category: string;
  typical_amount_cents: number;
  suggested_cents: number;
  months_of_history: number;
  existing_planned_cents: number | null;
}

export const budgetsApi = {
  list: (month: string) => invoke<Budget[]>("list_budgets", { month }),
  create: (input: BudgetInput) => invoke<Budget>("create_budget", { input }),
  update: (id: number, input: BudgetInput) =>
    invoke<Budget>("update_budget", { id, input }),
  delete: (id: number) => invoke<void>("delete_budget", { id }),
  suggest: (targetMonth: string, lookbackMonths: number) =>
    invoke<BudgetSuggestion[]>("get_budget_suggestions", {
      targetMonth,
      lookbackMonths,
    }),
  copyFromMonth: (fromMonth: string, toMonth: string) =>
    invoke<number>("copy_budgets_from_month", { fromMonth, toMonth }),
  unbudgetedSpending: (month: string) =>
    invoke<CategoryTotal[]>("get_unbudgeted_spending", { month }),
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

// --- Backup and Restore (Phase 6) ---
export interface BackupResult {
  filename: string;
  content_base64: string;
  schema_version: number;
  account_count: number;
  transaction_count: number;
}

export interface RestoreValidation {
  schema_version: number;
  account_count: number;
  transaction_count: number;
  needs_migration: boolean;
}

export const backupApi = {
  create: () => invoke<BackupResult>("create_backup_command"),
  previewRestore: (contentBase64: string) =>
    invoke<RestoreValidation>("preview_restore_command", { contentBase64 }),
  restore: (contentBase64: string) =>
    invoke<RestoreValidation>("restore_backup_command", { contentBase64 }),
};

// --- Account reconciliation + running balances (Phase 011) ---
export type ReconciliationRecordStatus =
  "completed" | "needs_review" | "reopened";

export interface Reconciliation {
  id: number;
  account_id: number;
  statement_start_date: string;
  statement_end_date: string;
  statement_ending_balance_cents: number;
  cleared_balance_cents: number;
  status: ReconciliationRecordStatus;
  reconciled_at: string;
  source_import_id: number | null;
  note: string | null;
  created_at: string;
  updated_at: string;
}

export interface ReconciliationTransactionRow extends Transaction {
  running_balance_cents: number;
}

export interface ReconciliationPreview {
  account_id: number;
  statement_start_date: string;
  statement_end_date: string;
  statement_ending_balance_cents: number;
  cleared_balance_cents: number;
  difference_cents: number;
  rows: ReconciliationTransactionRow[];
}

export interface ReconciliationSummary {
  last_reconciled_date: string | null;
  last_reconciled_ending_balance_cents: number | null;
  needs_review_count: number;
  unreconciled_transaction_count: number;
}

export interface StatementHint {
  source_import_id: number;
  source_filename: string;
  statement_start_date: string | null;
  statement_end_date: string | null;
  statement_ending_balance_cents: number;
}

export interface RunningBalanceRow {
  transaction_id: number;
  running_balance_cents: number;
}

export const reconciliationApi = {
  preview: (
    accountId: number,
    statementStartDate: string,
    statementEndDate: string,
    statementEndingBalanceCents: number,
  ) =>
    invoke<ReconciliationPreview>("get_reconciliation_preview", {
      accountId,
      statementStartDate,
      statementEndDate,
      statementEndingBalanceCents,
    }),
  complete: (
    accountId: number,
    statementStartDate: string,
    statementEndDate: string,
    statementEndingBalanceCents: number,
    note: string | null,
    sourceImportId: number | null,
  ) =>
    invoke<Reconciliation>("complete_reconciliation", {
      accountId,
      statementStartDate,
      statementEndDate,
      statementEndingBalanceCents,
      note,
      sourceImportId,
    }),
  reopen: (id: number) =>
    invoke<Reconciliation>("reopen_reconciliation", { id }),
  list: (accountId: number) =>
    invoke<Reconciliation[]>("list_reconciliations", { accountId }),
  getSummary: (accountId: number) =>
    invoke<ReconciliationSummary>("get_reconciliation_summary", {
      accountId,
    }),
  getStatementHint: (accountId: number) =>
    invoke<StatementHint | null>("get_latest_statement_hint", { accountId }),
  getRunningBalances: (accountId: number) =>
    invoke<RunningBalanceRow[]>("get_running_balances", { accountId }),
};

// --- Payee Library (Transaction Intelligence, specs/phase-012) ---
export type PayeeType = "merchant" | "company" | "person";

export interface Payee {
  id: number;
  name: string;
  payee_type: PayeeType;
  notes: string | null;
  is_archived: boolean;
  merged_into_payee_id: number | null;
  transaction_count: number;
  created_at: string;
  updated_at: string;
}

export interface PayeeInput {
  name: string;
  payee_type: PayeeType;
  notes: string | null;
}

export const payeesApi = {
  list: (includeArchived: boolean) =>
    invoke<Payee[]>("list_payees", { includeArchived }),
  create: (input: PayeeInput) => invoke<Payee>("create_payee", { input }),
  update: (id: number, input: PayeeInput) =>
    invoke<Payee>("update_payee", { id, input }),
  setArchived: (id: number, archived: boolean) =>
    invoke<Payee>("set_payee_archived", { id, archived }),
  merge: (sourceId: number, targetId: number) =>
    invoke<Payee>("merge_payees", { sourceId, targetId }),
};

// --- Tags (Transaction Intelligence, specs/phase-012) ---
export interface Tag {
  id: number;
  name: string;
  created_at: string;
}

export const tagsApi = {
  list: () => invoke<Tag[]>("list_tags"),
  create: (name: string) => invoke<Tag>("create_tag", { name }),
  delete: (id: number) => invoke<void>("delete_tag", { id }),
  attach: (transactionId: number, tagId: number) =>
    invoke<void>("attach_tag", { transactionId, tagId }),
  detach: (transactionId: number, tagId: number) =>
    invoke<void>("detach_tag", { transactionId, tagId }),
  listForTransaction: (transactionId: number) =>
    invoke<Tag[]>("list_tags_for_transaction", { transactionId }),
};

// --- Split transactions (Transaction Intelligence, specs/phase-012) ---
export interface Split {
  id: number;
  transaction_id: number;
  category: string | null;
  subcategory: string | null;
  amount_cents: number;
  notes: string | null;
  created_at: string;
  updated_at: string;
}

export interface SplitInput {
  category: string | null;
  subcategory: string | null;
  amount_cents: number;
  notes: string | null;
}

export const splitsApi = {
  list: (transactionId: number) =>
    invoke<Split[]>("list_splits", { transactionId }),
  set: (transactionId: number, splits: SplitInput[]) =>
    invoke<Split[]>("set_splits", { transactionId, splits }),
};

// --- Free-text category cleanup (Transaction Intelligence, specs/phase-012) ---
export interface RenameSummary {
  transactions_updated: number;
  splits_updated: number;
  rules_updated: number;
  budgets_updated: number;
  budgets_skipped_due_to_conflict: number;
}

export const categoriesApi = {
  rename: (from: string, to: string) =>
    invoke<RenameSummary>("rename_category", { from, to }),
};

// --- Bulk transaction actions (Transaction Intelligence, specs/phase-012) ---
export interface BulkResult {
  updated_count: number;
}

export const bulkApi = {
  setCategory: (ids: number[], category: string | null) =>
    invoke<BulkResult>("bulk_set_category", { ids, category }),
  setPayee: (ids: number[], payeeId: number | null) =>
    invoke<BulkResult>("bulk_set_payee", { ids, payeeId }),
  setOneTime: (ids: number[], isOneTime: boolean) =>
    invoke<BulkResult>("bulk_set_one_time", { ids, isOneTime }),
  setArchived: (ids: number[], archived: boolean) =>
    invoke<BulkResult>("bulk_set_archived", { ids, archived }),
  addTag: (ids: number[], tagId: number) =>
    invoke<BulkResult>("bulk_add_tag", { ids, tagId }),
  removeTag: (ids: number[], tagId: number) =>
    invoke<BulkResult>("bulk_remove_tag", { ids, tagId }),
};
