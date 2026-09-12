//! Staged CSV import: parse -> validate -> normalize -> duplicate analysis
//! -> commit. Nothing here writes to the `transactions` table until
//! `commit_import` - `stage_import` and `preview_csv` are read-only against
//! the ledger (they still read existing transactions, to classify
//! duplicates against real history).
//!
//! Staging is deliberately stateless/ephemeral (product handoff section 9):
//! `commit_import` re-parses the same CSV content rather than trusting a
//! previously-returned staging result, so staging and commit can never
//! drift apart, and there is no staging table to keep consistent.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use tauri::State;

use crate::accounts;
use crate::db::DbState;
use crate::money;
use crate::rules::{self, MerchantRule};
use crate::transactions::TransactionType;

const MAX_CSV_BYTES: usize = 20 * 1024 * 1024; // 20MB - generous for a statement, bounded against pathological input.
const MAX_ROWS: usize = 50_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ColumnMapping {
    pub date_column: Option<String>,
    pub posted_date_column: Option<String>,
    pub description_column: Option<String>,
    pub amount_column: Option<String>,
    pub debit_column: Option<String>,
    pub credit_column: Option<String>,
    pub category_column: Option<String>,
}

impl ColumnMapping {
    fn is_usable(&self) -> bool {
        self.date_column.is_some()
            && self.description_column.is_some()
            && (self.amount_column.is_some()
                || (self.debit_column.is_some() && self.credit_column.is_some()))
    }
}

fn header_matches(header: &str, candidates: &[&str]) -> bool {
    let normalized = header.trim().to_lowercase();
    candidates.iter().any(|c| normalized == *c)
}

/// Deterministic header-pattern matching only - no fuzzy/AI guessing. A
/// header that doesn't match a known pattern is left unmapped, and the UI
/// asks the user rather than assuming (section 6).
pub fn detect_mapping(headers: &[String]) -> ColumnMapping {
    let mut mapping = ColumnMapping::default();
    for h in headers {
        if mapping.date_column.is_none()
            && header_matches(h, &["date", "transaction date", "trans date"])
        {
            mapping.date_column = Some(h.clone());
        } else if mapping.posted_date_column.is_none()
            && header_matches(h, &["posted date", "post date", "posting date"])
        {
            mapping.posted_date_column = Some(h.clone());
        } else if mapping.description_column.is_none()
            && header_matches(
                h,
                &[
                    "description",
                    "memo",
                    "payee",
                    "merchant",
                    "details",
                    "narrative",
                ],
            )
        {
            mapping.description_column = Some(h.clone());
        } else if mapping.amount_column.is_none()
            && header_matches(h, &["amount", "transaction amount"])
        {
            mapping.amount_column = Some(h.clone());
        } else if mapping.debit_column.is_none()
            && header_matches(h, &["debit", "withdrawal", "debit amount"])
        {
            mapping.debit_column = Some(h.clone());
        } else if mapping.credit_column.is_none()
            && header_matches(h, &["credit", "deposit", "credit amount"])
        {
            mapping.credit_column = Some(h.clone());
        } else if mapping.category_column.is_none() && header_matches(h, &["category"]) {
            mapping.category_column = Some(h.clone());
        }
    }
    mapping
}

#[derive(Debug, Serialize)]
pub struct CsvPreview {
    pub headers: Vec<String>,
    pub row_count: usize,
    pub sample_rows: Vec<HashMap<String, String>>,
    pub suggested_mapping: ColumnMapping,
    pub mapping_confident: bool,
}

type CsvRow = HashMap<String, String>;

fn read_csv(csv_content: &str) -> Result<(Vec<String>, Vec<CsvRow>), String> {
    if csv_content.len() > MAX_CSV_BYTES {
        return Err(format!(
            "File is too large ({} bytes; limit is {MAX_CSV_BYTES}).",
            csv_content.len()
        ));
    }
    let mut reader = csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(csv_content.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| format!("Could not read CSV headers: {e}"))?
        .iter()
        .map(|h| h.to_string())
        .collect();
    if headers.is_empty() {
        return Err("CSV file has no header row.".to_string());
    }

    let mut rows = Vec::new();
    for (i, result) in reader.records().enumerate() {
        if i >= MAX_ROWS {
            return Err(format!(
                "File has more than {MAX_ROWS} rows; split it before importing."
            ));
        }
        let record = result.map_err(|e| format!("Malformed CSV at row {}: {e}", i + 2))?;
        let mut row = HashMap::new();
        for (col, value) in headers.iter().zip(record.iter()) {
            row.insert(col.clone(), value.to_string());
        }
        rows.push(row);
    }
    Ok((headers, rows))
}

#[tauri::command]
pub fn preview_csv(csv_content: String) -> Result<CsvPreview, String> {
    let (headers, rows) = read_csv(&csv_content)?;
    let suggested_mapping = detect_mapping(&headers);
    Ok(CsvPreview {
        row_count: rows.len(),
        sample_rows: rows.into_iter().take(10).collect(),
        mapping_confident: suggested_mapping.is_usable(),
        headers,
        suggested_mapping,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowStatus {
    Valid,
    Invalid,
    ExactDuplicate,
    ProbableDuplicate,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProposedTransaction {
    pub transaction_date: String,
    pub posted_date: Option<String>,
    pub amount_cents: i64,
    pub transaction_type: TransactionType,
    pub raw_description: String,
    pub merchant: String,
    pub category: Option<String>,
    pub subcategory: Option<String>,
    pub source_row_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StagingRow {
    pub row_index: usize,
    pub raw: HashMap<String, String>,
    pub status: RowStatus,
    pub errors: Vec<String>,
    pub proposed: Option<ProposedTransaction>,
    pub duplicate_of_transaction_id: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct StagingResult {
    pub rows: Vec<StagingRow>,
    pub valid_count: usize,
    pub invalid_count: usize,
    pub exact_duplicate_count: usize,
    pub probable_duplicate_count: usize,
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn row_hash(account_id: i64, date: &str, amount_cents: i64, description: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(account_id.to_le_bytes());
    hasher.update(date.as_bytes());
    hasher.update(amount_cents.to_le_bytes());
    hasher.update(description.trim().to_lowercase().as_bytes());
    to_hex(&hasher.finalize())
}

fn default_transaction_type(amount_cents: i64, is_liability: bool) -> TransactionType {
    match (amount_cents.is_positive(), is_liability) {
        (true, false) => TransactionType::Income, // asset account, money in
        (false, false) => TransactionType::Purchase, // asset account, money out
        (true, true) => TransactionType::Purchase, // liability account, balance up = a charge
        (false, true) => TransactionType::Transfer, // liability account, balance down = a payment
    }
}

struct ExistingLedger {
    hash_to_id: HashMap<String, i64>,
    by_date_amount: HashMap<(String, i64), Vec<i64>>,
}

fn load_existing_ledger(conn: &Connection, account_id: i64) -> Result<ExistingLedger, String> {
    let mut stmt = conn
        .prepare("SELECT id, transaction_date, amount_cents, source_row_hash FROM transactions WHERE account_id = ?1")
        .map_err(|e| e.to_string())?;
    let mut hash_to_id = HashMap::new();
    let mut by_date_amount: HashMap<(String, i64), Vec<i64>> = HashMap::new();
    let rows = stmt
        .query_map(params![account_id], |row| {
            let id: i64 = row.get(0)?;
            let date: String = row.get(1)?;
            let amount: i64 = row.get(2)?;
            let hash: Option<String> = row.get(3)?;
            Ok((id, date, amount, hash))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (id, date, amount, hash) = row.map_err(|e| e.to_string())?;
        if let Some(h) = hash {
            hash_to_id.insert(h, id);
        }
        by_date_amount.entry((date, amount)).or_default().push(id);
    }
    Ok(ExistingLedger {
        hash_to_id,
        by_date_amount,
    })
}

/// Parses every row, computes the proposed transaction, and classifies it
/// against `ledger` (rows already committed for this account) plus rows
/// seen earlier *within this same batch* - so a CSV that genuinely contains
/// the same row twice (an export bug) is still caught, without flagging two
/// separate legitimate same-day/same-amount purchases at the same merchant
/// against each other (section 10's explicit non-goal).
fn build_staging_rows(
    account_id: i64,
    is_liability: bool,
    headers: &[String],
    raw_rows: &[HashMap<String, String>],
    mapping: &ColumnMapping,
    ledger: &ExistingLedger,
    merchant_rules: &[MerchantRule],
) -> StagingResult {
    let _ = headers;
    let mut rows = Vec::with_capacity(raw_rows.len());
    // Tracks hashes seen earlier in *this* batch, mapped to their row_index,
    // so a CSV that genuinely repeats a row (an export bug) is still caught
    // even though none of it is in the ledger yet.
    let mut batch_hashes: HashMap<String, usize> = HashMap::new();
    let (mut valid, mut invalid, mut exact_dup, mut probable_dup) = (0, 0, 0, 0);

    for (row_index, raw) in raw_rows.iter().enumerate() {
        let mut errors = Vec::new();

        let date_str = mapping
            .date_column
            .as_deref()
            .and_then(|c| raw.get(c))
            .map(String::as_str);
        let date = match date_str {
            Some(d) if !d.trim().is_empty() => {
                money::parse_date(d).map_err(|e| errors.push(e)).ok()
            }
            _ => {
                errors.push("Missing required date value.".to_string());
                None
            }
        };

        let posted_date = mapping
            .posted_date_column
            .as_deref()
            .and_then(|c| raw.get(c))
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .and_then(|s| money::parse_date(s).map_err(|e| errors.push(e)).ok());

        let description = mapping
            .description_column
            .as_deref()
            .and_then(|c| raw.get(c))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if description.is_none() {
            errors.push("Missing required description value.".to_string());
        }

        let amount = if let Some(col) = mapping.amount_column.as_deref() {
            match raw.get(col).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                Some(v) => money::parse_amount_cents(v)
                    .map_err(|e| errors.push(e))
                    .ok(),
                None => {
                    errors.push("Missing required amount value.".to_string());
                    None
                }
            }
        } else {
            let debit = mapping
                .debit_column
                .as_deref()
                .and_then(|c| raw.get(c))
                .map(String::as_str);
            let credit = mapping
                .credit_column
                .as_deref()
                .and_then(|c| raw.get(c))
                .map(String::as_str);
            money::combine_debit_credit(debit, credit, is_liability)
                .map_err(|e| errors.push(e))
                .ok()
        };

        let csv_category = mapping
            .category_column
            .as_deref()
            .and_then(|c| raw.get(c))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let (status, proposed, duplicate_of_transaction_id) = match (date, description, amount) {
            (Some(date), Some(description), Some(amount_cents)) => {
                // Rule hierarchy (product handoff section 8): an existing
                // user rule outranks whatever category text the statement
                // itself happened to export - a rule reflects the owner's
                // own deliberate correction, a CSV category column is just
                // upstream source data.
                let rule_match = rules::best_match(merchant_rules, &description);
                let merchant = rule_match
                    .as_ref()
                    .and_then(|m| m.merchant.clone())
                    .unwrap_or_else(|| description.clone());
                let category = rule_match
                    .as_ref()
                    .and_then(|m| m.category.clone())
                    .or(csv_category);
                let subcategory = rule_match.and_then(|m| m.subcategory);
                let hash = row_hash(account_id, &date, amount_cents, &description);
                let transaction_type = default_transaction_type(amount_cents, is_liability);

                if let Some(existing_id) = ledger.hash_to_id.get(&hash) {
                    (RowStatus::ExactDuplicate, None, Some(*existing_id))
                } else if batch_hashes.contains_key(&hash) {
                    // Same row appears twice within this one file (an export
                    // bug). Still correctly excluded from commit; there's no
                    // ledger transaction id to point at since neither
                    // occurrence has been committed yet.
                    (RowStatus::ExactDuplicate, None, None)
                } else if let Some(candidates) =
                    ledger.by_date_amount.get(&(date.clone(), amount_cents))
                {
                    (
                        RowStatus::ProbableDuplicate,
                        Some(ProposedTransaction {
                            transaction_date: date,
                            posted_date,
                            amount_cents,
                            transaction_type,
                            raw_description: description,
                            merchant,
                            category,
                            subcategory: subcategory.clone(),
                            source_row_hash: hash,
                        }),
                        candidates.first().copied(),
                    )
                } else {
                    batch_hashes.insert(hash.clone(), row_index);
                    (
                        RowStatus::Valid,
                        Some(ProposedTransaction {
                            transaction_date: date,
                            posted_date,
                            amount_cents,
                            transaction_type,
                            raw_description: description,
                            merchant,
                            category,
                            subcategory: subcategory.clone(),
                            source_row_hash: hash,
                        }),
                        None,
                    )
                }
            }
            _ => (RowStatus::Invalid, None, None),
        };

        match status {
            RowStatus::Valid => valid += 1,
            RowStatus::Invalid => invalid += 1,
            RowStatus::ExactDuplicate => exact_dup += 1,
            RowStatus::ProbableDuplicate => probable_dup += 1,
        }

        rows.push(StagingRow {
            row_index,
            raw: raw.clone(),
            status,
            errors,
            proposed,
            duplicate_of_transaction_id,
        });
    }

    StagingResult {
        rows,
        valid_count: valid,
        invalid_count: invalid,
        exact_duplicate_count: exact_dup,
        probable_duplicate_count: probable_dup,
    }
}

pub fn stage(
    conn: &Connection,
    account_id: i64,
    csv_content: &str,
    mapping: &ColumnMapping,
) -> Result<StagingResult, String> {
    let is_liability = accounts::account_type_of(conn, account_id)
        .map_err(|e| e.to_string())?
        .is_liability();
    let (headers, rows) = read_csv(csv_content)?;
    let ledger = load_existing_ledger(conn, account_id)?;
    let merchant_rules = rules::relevant_rules(conn, account_id).map_err(|e| e.to_string())?;
    Ok(build_staging_rows(
        account_id,
        is_liability,
        &headers,
        &rows,
        mapping,
        &ledger,
        &merchant_rules,
    ))
}

#[tauri::command]
pub fn stage_import(
    state: State<DbState>,
    account_id: i64,
    csv_content: String,
    mapping: ColumnMapping,
) -> Result<StagingResult, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    stage(&conn, account_id, &csv_content, &mapping)
}

#[derive(Debug, Serialize)]
pub struct ImportSummary {
    pub import_id: i64,
    pub committed_count: usize,
    pub exact_duplicate_count: usize,
    pub sent_to_review_count: usize,
    pub invalid_count: usize,
}

#[allow(clippy::too_many_arguments)]
pub fn commit(
    conn: &mut Connection,
    account_id: i64,
    source_filename: &str,
    csv_content: &str,
    mapping: &ColumnMapping,
    force_import_row_indices: &HashSet<usize>,
) -> Result<ImportSummary, String> {
    let is_liability = accounts::account_type_of(conn, account_id)
        .map_err(|e| e.to_string())?
        .is_liability();
    let (headers, rows) = read_csv(csv_content)?;
    let ledger = load_existing_ledger(conn, account_id)?;
    let merchant_rules = rules::relevant_rules(conn, account_id).map_err(|e| e.to_string())?;
    let staged = build_staging_rows(
        account_id,
        is_liability,
        &headers,
        &rows,
        mapping,
        &ledger,
        &merchant_rules,
    );
    let force_import = force_import_row_indices;

    let file_hash = {
        let mut hasher = Sha256::new();
        hasher.update(csv_content.as_bytes());
        to_hex(&hasher.finalize())
    };
    let mapping_json = serde_json::to_string(mapping).map_err(|e| e.to_string())?;

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut committed = 0usize;
    let mut exact_duplicates = 0usize;
    let mut sent_to_review = 0usize;
    let mut invalid = 0usize;

    let import_id: i64 = tx
        .query_row(
            "INSERT INTO imports (account_id, source_filename, file_hash, row_count, committed_count, duplicate_count, needs_review_count, column_mapping_json)
             VALUES (?1, ?2, ?3, ?4, 0, 0, 0, ?5) RETURNING id",
            params![account_id, source_filename, file_hash, staged.rows.len() as i64, mapping_json],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    for row in &staged.rows {
        match &row.status {
            RowStatus::ExactDuplicate => {
                exact_duplicates += 1;
            }
            RowStatus::Invalid => {
                invalid += 1;
                insert_review_item(
                    &tx,
                    "malformed_row",
                    None,
                    Some(import_id),
                    row,
                    &format!(
                        "Row {} could not be parsed: {}",
                        row.row_index + 1,
                        row.errors.join("; ")
                    ),
                )
                .map_err(|e| e.to_string())?;
            }
            RowStatus::ProbableDuplicate if !force_import.contains(&row.row_index) => {
                sent_to_review += 1;
                // duplicate_of_transaction_id should always be set for a
                // ProbableDuplicate row, but the reason text must never
                // fabricate a fake "#0" reference if that assumption is
                // ever wrong somewhere upstream - an honest, slightly
                // vaguer message beats a misleading specific one.
                let reason = match row.duplicate_of_transaction_id {
                    Some(existing_id) => format!(
                        "Row {} matches an existing transaction on the same date for the same amount (possible duplicate of transaction #{existing_id}).",
                        row.row_index + 1
                    ),
                    None => format!(
                        "Row {} matches an existing transaction on the same date for the same amount.",
                        row.row_index + 1
                    ),
                };
                insert_review_item(
                    &tx,
                    "suspected_duplicate",
                    None,
                    Some(import_id),
                    row,
                    &reason,
                )
                .map_err(|e| e.to_string())?;
            }
            RowStatus::Valid | RowStatus::ProbableDuplicate => {
                if let Some(proposed) = &row.proposed {
                    let transaction_id: i64 = tx
                        .query_row(
                            "INSERT INTO transactions (
                                account_id, transaction_date, posted_date, amount_cents, transaction_type,
                                raw_description, merchant, category, subcategory, source_import_id, source_row_hash
                            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) RETURNING id",
                            params![
                                account_id,
                                proposed.transaction_date,
                                proposed.posted_date,
                                proposed.amount_cents,
                                proposed.transaction_type.as_sql_str(),
                                proposed.raw_description,
                                proposed.merchant,
                                proposed.category,
                                proposed.subcategory,
                                import_id,
                                proposed.source_row_hash,
                            ],
                            |r| r.get(0),
                        )
                        .map_err(|e| e.to_string())?;
                    committed += 1;
                    let _ = transaction_id;
                }
            }
        }
    }

    tx.execute(
        "UPDATE imports SET committed_count = ?1, duplicate_count = ?2, needs_review_count = ?3 WHERE id = ?4",
        params![committed as i64, exact_duplicates as i64, (sent_to_review + invalid) as i64, import_id],
    )
    .map_err(|e| e.to_string())?;

    tx.commit().map_err(|e| e.to_string())?;

    Ok(ImportSummary {
        import_id,
        committed_count: committed,
        exact_duplicate_count: exact_duplicates,
        sent_to_review_count: sent_to_review,
        invalid_count: invalid,
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn commit_import(
    state: State<DbState>,
    account_id: i64,
    source_filename: String,
    csv_content: String,
    mapping: ColumnMapping,
    force_import_row_indices: Vec<usize>,
) -> Result<ImportSummary, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let force_set: HashSet<usize> = force_import_row_indices.into_iter().collect();
    commit(
        &mut conn,
        account_id,
        &source_filename,
        &csv_content,
        &mapping,
        &force_set,
    )
}

fn insert_review_item(
    conn: &Connection,
    issue_type: &str,
    transaction_id: Option<i64>,
    import_id: Option<i64>,
    row: &StagingRow,
    reason: &str,
) -> rusqlite::Result<()> {
    let source_row_json = serde_json::to_string(&row.raw).unwrap_or_default();
    let proposed_json = row
        .proposed
        .as_ref()
        .map(|p| serde_json::to_string(p).unwrap_or_default());
    conn.execute(
        "INSERT INTO needs_review_items (issue_type, transaction_id, import_id, source_row_json, detected_reason, proposed_resolution_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![issue_type, transaction_id, import_id, source_row_json, reason, proposed_json],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::rules::{MatchType, MerchantRuleInput};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute_batch(include_str!("../migrations/0001_initial.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../migrations/0002_transactions.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../migrations/0003_rules_and_recurring.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../migrations/0004_accounting_boundary.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../migrations/0007_balance_adjustment.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../migrations/0008_foundation_batch_1.sql"))
            .unwrap();
        conn
    }

    fn make_account(conn: &Connection, account_type: AccountType) -> i64 {
        accounts::create(
            conn,
            &AccountInput {
                name: "Test Account".to_string(),
                institution: None,
                account_type,
                include_in_net_worth: true,
                notes: None,
            },
        )
        .unwrap()
        .id
    }

    fn signed_amount_mapping() -> ColumnMapping {
        ColumnMapping {
            date_column: Some("Date".to_string()),
            posted_date_column: None,
            description_column: Some("Description".to_string()),
            amount_column: Some("Amount".to_string()),
            debit_column: None,
            credit_column: None,
            category_column: None,
        }
    }

    fn debit_credit_mapping() -> ColumnMapping {
        ColumnMapping {
            date_column: Some("Date".to_string()),
            posted_date_column: None,
            description_column: Some("Description".to_string()),
            amount_column: None,
            debit_column: Some("Debit".to_string()),
            credit_column: Some("Credit".to_string()),
            category_column: None,
        }
    }

    #[test]
    fn detect_mapping_recognizes_common_signed_amount_headers() {
        let headers = vec![
            "Date".to_string(),
            "Description".to_string(),
            "Amount".to_string(),
        ];
        let mapping = detect_mapping(&headers);
        assert_eq!(mapping.date_column.as_deref(), Some("Date"));
        assert_eq!(mapping.description_column.as_deref(), Some("Description"));
        assert_eq!(mapping.amount_column.as_deref(), Some("Amount"));
        assert!(mapping.is_usable());
    }

    #[test]
    fn detect_mapping_recognizes_debit_credit_headers() {
        let headers = vec![
            "Transaction Date".to_string(),
            "Memo".to_string(),
            "Debit".to_string(),
            "Credit".to_string(),
        ];
        let mapping = detect_mapping(&headers);
        assert_eq!(mapping.date_column.as_deref(), Some("Transaction Date"));
        assert_eq!(mapping.description_column.as_deref(), Some("Memo"));
        assert_eq!(mapping.debit_column.as_deref(), Some("Debit"));
        assert_eq!(mapping.credit_column.as_deref(), Some("Credit"));
        assert!(mapping.is_usable());
    }

    #[test]
    fn detect_mapping_is_not_confident_on_unrecognized_headers() {
        let headers = vec!["Col A".to_string(), "Col B".to_string()];
        let mapping = detect_mapping(&headers);
        assert!(!mapping.is_usable());
    }

    #[test]
    fn stages_a_normal_signed_amount_csv_as_valid() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv =
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n2026-03-02,PAYCHECK,2000.00\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(result.valid_count, 2);
        assert_eq!(result.invalid_count, 0);
        let proposed = result.rows[0].proposed.as_ref().unwrap();
        assert_eq!(proposed.amount_cents, -450);
        assert_eq!(proposed.transaction_type, TransactionType::Purchase);
        let income = result.rows[1].proposed.as_ref().unwrap();
        assert_eq!(income.amount_cents, 200000);
        assert_eq!(income.transaction_type, TransactionType::Income);
    }

    #[test]
    fn stages_separate_debit_credit_columns_correctly_by_account_type() {
        let conn = test_conn();
        let checking_id = make_account(&conn, AccountType::Checking);
        let csv = "Date,Description,Debit,Credit\n2026-03-01,GROCERY STORE,42.10,\n2026-03-02,REFUND,,10.00\n";
        let result = stage(&conn, checking_id, csv, &debit_credit_mapping()).unwrap();
        assert_eq!(result.valid_count, 2);
        assert_eq!(
            result.rows[0].proposed.as_ref().unwrap().amount_cents,
            -4210
        );
        assert_eq!(result.rows[1].proposed.as_ref().unwrap().amount_cents, 1000);

        // Same debit/credit values on a credit card: sign flips (ADR-0004).
        let card_id = make_account(&conn, AccountType::CreditCard);
        let card_result = stage(&conn, card_id, csv, &debit_credit_mapping()).unwrap();
        assert_eq!(
            card_result.rows[0].proposed.as_ref().unwrap().amount_cents,
            4210
        );
        assert_eq!(
            card_result.rows[1].proposed.as_ref().unwrap().amount_cents,
            -1000
        );
    }

    #[test]
    fn a_matching_rule_normalizes_merchant_and_category_during_staging() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        rules::create(
            &conn,
            &MerchantRuleInput {
                match_type: MatchType::Contains,
                match_value: "coffee shop".to_string(),
                account_id: None,
                normalized_merchant: Some("Coffee Shop".to_string()),
                category: Some("Dining & Restaurants".to_string()),
                subcategory: None,
                priority: 0,
            },
        )
        .unwrap();

        let csv = "Date,Description,Amount\n2026-03-01,SQ *COFFEE SHOP 123 MIAMI FL,-4.50\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        let proposed = result.rows[0].proposed.as_ref().unwrap();
        assert_eq!(proposed.merchant, "Coffee Shop");
        assert_eq!(proposed.category.as_deref(), Some("Dining & Restaurants"));
        assert_eq!(
            proposed.raw_description, "SQ *COFFEE SHOP 123 MIAMI FL",
            "original text is preserved"
        );
    }

    #[test]
    fn a_rule_category_overrides_the_csvs_own_category_column() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        rules::create(
            &conn,
            &MerchantRuleInput {
                match_type: MatchType::Exact,
                match_value: "netflix".to_string(),
                account_id: None,
                normalized_merchant: None,
                category: Some("Subscriptions".to_string()),
                subcategory: None,
                priority: 0,
            },
        )
        .unwrap();

        let mut mapping = signed_amount_mapping();
        mapping.category_column = Some("Category".to_string());
        let csv = "Date,Description,Amount,Category\n2026-03-01,Netflix,-15.49,Entertainment\n";
        let result = stage(&conn, account_id, csv, &mapping).unwrap();
        let proposed = result.rows[0].proposed.as_ref().unwrap();
        assert_eq!(
            proposed.category.as_deref(),
            Some("Subscriptions"),
            "the rule wins over the CSV's own category"
        );
    }

    #[test]
    fn currency_symbols_and_parentheses_negatives_parse_in_staging() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv = "Date,Description,Amount\n2026-03-01,STORE,\"($42.10)\"\n2026-03-02,DEPOSIT,\"$1,000.00\"\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(result.valid_count, 2);
        assert_eq!(
            result.rows[0].proposed.as_ref().unwrap().amount_cents,
            -4210
        );
        assert_eq!(
            result.rows[1].proposed.as_ref().unwrap().amount_cents,
            100000
        );
    }

    #[test]
    fn malformed_amount_and_missing_required_values_are_invalid_not_zero() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv = "Date,Description,Amount\n2026-03-01,BAD AMOUNT,not-a-number\n2026-03-02,,5.00\n,MISSING DATE,5.00\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(result.invalid_count, 3);
        assert_eq!(result.valid_count, 0);
        for row in &result.rows {
            assert!(matches!(row.status, RowStatus::Invalid));
            assert!(row.proposed.is_none());
            assert!(!row.errors.is_empty());
        }
    }

    #[test]
    fn supported_and_malformed_dates_in_staging() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv = "Date,Description,Amount\n03/04/2026,US SLASH,5.00\n2026-13-40,BAD DATE,5.00\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(result.valid_count, 1);
        assert_eq!(
            result.rows[0].proposed.as_ref().unwrap().transaction_date,
            "2026-03-04"
        );
        assert_eq!(result.invalid_count, 1);
    }

    #[test]
    fn similar_legitimate_transactions_on_different_dates_are_not_flagged() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        // Same merchant, same amount, different dates: two genuine separate purchases.
        let csv =
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n2026-03-08,COFFEE SHOP,-4.50\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(result.valid_count, 2);
        assert_eq!(result.probable_duplicate_count, 0);
        assert_eq!(result.exact_duplicate_count, 0);
    }

    #[test]
    fn exact_duplicate_within_the_same_file_is_flagged() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        // An export bug: the identical row appears twice.
        let csv =
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n2026-03-01,COFFEE SHOP,-4.50\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(result.valid_count, 1);
        assert_eq!(result.exact_duplicate_count, 1);
    }

    #[test]
    fn successful_commit_creates_transactions_and_records_import_metadata() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv =
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n2026-03-02,PAYCHECK,2000.00\n";
        let summary = commit(
            &mut conn,
            account_id,
            "statement.csv",
            csv,
            &signed_amount_mapping(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(summary.committed_count, 2);
        assert_eq!(summary.exact_duplicate_count, 0);
        assert_eq!(summary.sent_to_review_count, 0);

        let committed = crate::transactions::list(
            &conn,
            &crate::transactions::TransactionFilter {
                account_id: Some(account_id),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(committed.len(), 2);
        assert_eq!(committed[0].source_import_id, Some(summary.import_id));

        let (row_count, filename): (i64, String) = conn
            .query_row(
                "SELECT row_count, source_filename FROM imports WHERE id = ?1",
                params![summary.import_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row_count, 2);
        assert_eq!(filename, "statement.csv");
    }

    #[test]
    fn exact_reimport_of_the_same_file_does_not_duplicate_the_ledger() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv =
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n2026-03-02,PAYCHECK,2000.00\n";

        let first = commit(
            &mut conn,
            account_id,
            "statement.csv",
            csv,
            &signed_amount_mapping(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(first.committed_count, 2);

        // Owner re-imports the exact same statement, e.g. by mistake.
        let second = commit(
            &mut conn,
            account_id,
            "statement.csv",
            csv,
            &signed_amount_mapping(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(second.committed_count, 0);
        assert_eq!(second.exact_duplicate_count, 2);

        let all = crate::transactions::list(
            &conn,
            &crate::transactions::TransactionFilter {
                account_id: Some(account_id),
                include_archived: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 2, "re-import must not have added new rows");
    }

    #[test]
    fn probable_duplicate_goes_to_needs_review_unless_forced() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let first_csv = "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n";
        commit(
            &mut conn,
            account_id,
            "first.csv",
            first_csv,
            &signed_amount_mapping(),
            &HashSet::new(),
        )
        .unwrap();

        // A second, overlapping export: same date/amount but a slightly
        // different description - not byte-identical, so not an exact
        // duplicate, but a strong duplicate signal worth a human decision.
        let second_csv = "Date,Description,Amount\n2026-03-01,COFFEE SHOP #4471,-4.50\n";
        let staged = stage(&conn, account_id, second_csv, &signed_amount_mapping()).unwrap();
        assert_eq!(staged.probable_duplicate_count, 1);

        let summary_review = commit(
            &mut conn,
            account_id,
            "second.csv",
            second_csv,
            &signed_amount_mapping(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(summary_review.committed_count, 0);
        assert_eq!(summary_review.sent_to_review_count, 1);

        let review_items = crate::needs_review::list(&conn, true).unwrap();
        assert_eq!(review_items.len(), 1);
        assert_eq!(review_items[0].issue_type, "suspected_duplicate");

        // The user decides it's legitimate and forces the import.
        let force: HashSet<usize> = [0].into_iter().collect();
        let summary_forced = commit(
            &mut conn,
            account_id,
            "second.csv",
            second_csv,
            &signed_amount_mapping(),
            &force,
        )
        .unwrap();
        assert_eq!(summary_forced.committed_count, 1);
    }

    #[test]
    fn invalid_rows_go_to_needs_review_and_are_never_committed() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv = "Date,Description,Amount\n2026-03-01,BAD ROW,not-a-number\n";
        let summary = commit(
            &mut conn,
            account_id,
            "bad.csv",
            csv,
            &signed_amount_mapping(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(summary.committed_count, 0);
        assert_eq!(summary.invalid_count, 1);

        let review_items = crate::needs_review::list(&conn, true).unwrap();
        assert_eq!(review_items.len(), 1);
        assert_eq!(review_items[0].issue_type, "malformed_row");

        let committed = crate::transactions::list(
            &conn,
            &crate::transactions::TransactionFilter {
                account_id: Some(account_id),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(committed.is_empty());
    }

    #[test]
    fn a_dropped_transaction_never_commits_partial_writes() {
        // Proves the mechanism commit_import relies on for "rollback on
        // failure": a rusqlite Transaction that is dropped without an
        // explicit .commit() rolls back everything done inside it.
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        {
            let tx = conn.transaction().unwrap();
            tx.execute(
                "INSERT INTO transactions (account_id, transaction_date, amount_cents, transaction_type, raw_description, merchant)
                 VALUES (?1, '2026-01-01', -100, 'purchase', 'test', 'test')",
                params![account_id],
            )
            .unwrap();
            // tx is dropped here without .commit() - simulates an error
            // path returning early via `?` before reaching tx.commit().
        }
        let count: i64 = conn
            .query_row("SELECT count(*) FROM transactions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "uncommitted transaction must not persist");
    }
}
