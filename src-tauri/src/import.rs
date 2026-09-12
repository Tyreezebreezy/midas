//! Import Engine v2 (specs/phase-009): CSV source adapter -> normalized
//! candidate -> matching/duplicate detection -> rules/payee resolution ->
//! commit -> provenance/history. CSV is the only adapter today; a future
//! OFX/QFX adapter would parse into the same `NormalizedCandidateTransaction`
//! shape and reuse every function below that takes one rather than
//! duplicating matching/commit/undo/history logic (product handoff section
//! 5) - there is deliberately no `SourceAdapter` trait for a single
//! implementation, per this batch's own simplification instructions.
//!
//! Nothing here writes to the `transactions` table until `commit_import` -
//! `stage_import` and `preview_csv` are read-only against the ledger (they
//! still read existing transactions, to classify duplicates against real
//! history).
//!
//! Staging is deliberately stateless/ephemeral (product handoff section 9):
//! `commit_import` re-parses the same CSV content rather than trusting a
//! previously-returned staging result, so staging and commit can never
//! drift apart, and there is no staging table to keep consistent.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use tauri::State;

use crate::accounts;
use crate::db::DbState;
use crate::import_profiles::ImportSourceType;
use crate::money;
use crate::payees;
use crate::rules::{self, MerchantRule};
use crate::transactions::TransactionType;

const MAX_CSV_BYTES: usize = 20 * 1024 * 1024; // 20MB - generous for a statement, bounded against pathological input.
const MAX_ROWS: usize = 50_000;

/// A candidate is a "possible match" against an existing transaction (not
/// certain enough to auto-skip like `StrongMatch`) when it falls within
/// this many days of an existing same-account/same-amount/matching-
/// description transaction - wide enough to catch a manual entry made a
/// day or two before the bank posts the real transaction, narrow enough
/// that a monthly/weekly recurring charge never falls inside it (product
/// handoff section 15/16).
const POSSIBLE_MATCH_DAY_WINDOW: i64 = 3;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ColumnMapping {
    pub date_column: Option<String>,
    pub posted_date_column: Option<String>,
    pub description_column: Option<String>,
    pub amount_column: Option<String>,
    pub debit_column: Option<String>,
    pub credit_column: Option<String>,
    pub category_column: Option<String>,
    /// The source's own unique transaction ID column, when the file
    /// provides one (e.g. a bank "Reference Number"). Becomes the strongest
    /// duplicate identity available - see `ExistingLedger`.
    #[serde(default)]
    pub external_id_column: Option<String>,
    /// Escape hatch for a statement whose amount sign is the opposite of
    /// what MIDAS's normal parsing would produce for this account type -
    /// applied after amount/debit-credit parsing, before the sign-convention
    /// default-type inference. `#[serde(default)]` so a mapping saved before
    /// this field existed still deserializes (defaults to no inversion,
    /// i.e. identical behavior to before).
    #[serde(default)]
    pub invert_amount: bool,
}

impl ColumnMapping {
    fn is_usable(&self) -> bool {
        self.date_column.is_some()
            && self.description_column.is_some()
            && (self.amount_column.is_some()
                || (self.debit_column.is_some() && self.credit_column.is_some()))
    }

    /// Every header name this mapping actually depends on - used both by
    /// profile recognition (`import_profiles::recognize`) and to fail fast
    /// with a clear error when an applied mapping doesn't match the file
    /// actually being staged (product handoff section 28: "incompatible
    /// saved profile" should be its own clear error, not 5,000 rows that
    /// are each individually invalid).
    pub(crate) fn required_headers(&self) -> Vec<&str> {
        [
            &self.date_column,
            &self.posted_date_column,
            &self.description_column,
            &self.amount_column,
            &self.debit_column,
            &self.credit_column,
            &self.category_column,
            &self.external_id_column,
        ]
        .into_iter()
        .filter_map(|c| c.as_deref())
        .collect()
    }

    fn missing_headers(&self, headers: &[String]) -> Vec<String> {
        self.required_headers()
            .into_iter()
            .filter(|needed| !headers.iter().any(|h| h == needed))
            .map(str::to_string)
            .collect()
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
        } else if mapping.external_id_column.is_none()
            && header_matches(
                h,
                &["transaction id", "reference number", "reference id", "id"],
            )
        {
            mapping.external_id_column = Some(h.clone());
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
    /// Conservative saved-profile recognition against this file's actual
    /// headers (product handoff section 8) - `None` when there is no
    /// account to scope recognition to yet, otherwise MIDAS's best
    /// determination of Clear/Ambiguous/None. The frontend decides how to
    /// present it; MIDAS never silently applies a profile on its own.
    pub profile_recognition: crate::import_profiles::ProfileRecognition,
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
pub fn preview_csv(
    state: State<DbState>,
    csv_content: String,
    account_id: Option<i64>,
) -> Result<CsvPreview, String> {
    let (headers, rows) = read_csv(&csv_content)?;
    let suggested_mapping = detect_mapping(&headers);
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let profiles = crate::import_profiles::list(&conn, false).map_err(|e| e.to_string())?;
    let profile_recognition = crate::import_profiles::recognize(&profiles, &headers, account_id);
    Ok(CsvPreview {
        row_count: rows.len(),
        sample_rows: rows.into_iter().take(10).collect(),
        mapping_confident: suggested_mapping.is_usable(),
        headers,
        suggested_mapping,
        profile_recognition,
    })
}

/// The simple, user-facing status the preview shows (product handoff
/// section 10: "the normal user should see simple statuses... detailed
/// source information may be available through drilldown where useful").
/// The finer-grained `MatchOutcome` below is what actually decides commit
/// behavior; `PossibleMatch` covers both its `StrongMatch` and
/// `PossibleMatch` outcomes, since both are decisions for the owner, not
/// MIDAS, to make - only a certain (`ExactDuplicate`) match is ever
/// auto-skipped without asking (section 16: a false-positive Review item is
/// always preferred over silently discarding real data). The confidence
/// distinction is available via `match_detail`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowStatus {
    New,
    Duplicate,
    PossibleMatch,
    NeedsReview,
}

/// The ingestion boundary between source-specific parsing and MIDAS's
/// financial logic (product handoff section 6). CSV is the only producer
/// today; a future OFX/QFX adapter constructs the same shape and reuses
/// every matching/commit/undo/history function that takes one.
#[derive(Debug, Clone, Serialize)]
pub struct NormalizedCandidateTransaction {
    pub source_type: ImportSourceType,
    pub account_id: i64,
    pub transaction_date: String,
    pub posted_date: Option<String>,
    pub amount_cents: i64,
    pub transaction_type: TransactionType,
    pub raw_description: String,
    pub merchant: String,
    pub category: Option<String>,
    pub subcategory: Option<String>,
    /// The source's own unique transaction ID, when the mapping provides
    /// one and this row has it - see `ColumnMapping::external_id_column`.
    pub external_transaction_id: Option<String>,
    /// A payee deterministically resolved by exact name match against the
    /// Payee Library (product handoff section 19) - `None` far more often
    /// than not until a Payee Library UI exists for the user to populate it
    /// from. Never fabricated: an unmatched name is `None`, not a guess.
    pub candidate_payee_id: Option<i64>,
    pub source_row_hash: String,
}

/// The deterministic matching outcome (product handoff section 15) - kept
/// internal (not serialized) so the frontend only ever sees the simplified
/// `RowStatus` plus a plain-language `match_detail` reason; this is what
/// tests assert against for precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchOutcome {
    New,
    /// `None` only for a row repeated within the same file, ahead of either
    /// occurrence reaching the ledger - there is no existing transaction id
    /// yet for either one to point at.
    ExactDuplicate {
        existing_transaction_id: Option<i64>,
    },
    StrongMatch {
        existing_transaction_id: i64,
    },
    PossibleMatch {
        existing_transaction_id: i64,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct StagingRow {
    pub row_index: usize,
    pub raw: HashMap<String, String>,
    pub status: RowStatus,
    pub errors: Vec<String>,
    pub candidate: Option<NormalizedCandidateTransaction>,
    pub duplicate_of_transaction_id: Option<i64>,
    /// A short, plain-language explanation for `Duplicate`/`PossibleMatch`
    /// rows (e.g. "Matches an existing transaction on the same date for the
    /// same amount and description.") - the drilldown detail section 10
    /// allows for, without exposing raw matching internals in the main
    /// status column.
    pub match_detail: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StagingResult {
    pub rows: Vec<StagingRow>,
    pub new_count: usize,
    pub needs_review_count: usize,
    pub duplicate_count: usize,
    pub possible_match_count: usize,
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

struct LedgerRow {
    id: i64,
    date: String,
    amount_cents: i64,
    merchant_lower: String,
    raw_description_lower: String,
}

struct ExistingLedger {
    hash_to_id: HashMap<String, i64>,
    external_id_to_id: HashMap<String, i64>,
    rows: Vec<LedgerRow>,
}

fn load_existing_ledger(conn: &Connection, account_id: i64) -> Result<ExistingLedger, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, transaction_date, amount_cents, source_row_hash, external_transaction_id, merchant, raw_description
             FROM transactions WHERE account_id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let mut hash_to_id = HashMap::new();
    let mut external_id_to_id = HashMap::new();
    let mut rows = Vec::new();
    let query_rows = stmt
        .query_map(params![account_id], |row| {
            let id: i64 = row.get(0)?;
            let date: String = row.get(1)?;
            let amount: i64 = row.get(2)?;
            let hash: Option<String> = row.get(3)?;
            let external_id: Option<String> = row.get(4)?;
            let merchant: String = row.get(5)?;
            let raw_description: String = row.get(6)?;
            Ok((
                id,
                date,
                amount,
                hash,
                external_id,
                merchant,
                raw_description,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in query_rows {
        let (id, date, amount_cents, hash, external_id, merchant, raw_description) =
            row.map_err(|e| e.to_string())?;
        if let Some(h) = hash {
            hash_to_id.insert(h, id);
        }
        if let Some(ext_id) = external_id {
            external_id_to_id.insert(ext_id, id);
        }
        rows.push(LedgerRow {
            id,
            date,
            amount_cents,
            merchant_lower: merchant.trim().to_lowercase(),
            raw_description_lower: raw_description.trim().to_lowercase(),
        });
    }
    Ok(ExistingLedger {
        hash_to_id,
        external_id_to_id,
        rows,
    })
}

/// Deterministic, non-fuzzy description similarity (product handoff section
/// 15: "avoid opaque AI-like confidence scoring") - reuses this codebase's
/// own existing exact/prefix vocabulary (`rules::MatchType`) rather than a
/// scored heuristic. Exactly equal, or one is a prefix of the other, e.g.
/// "COFFEE SHOP" / "COFFEE SHOP #4471" (a common real-world pattern: a
/// re-export or the source's own transaction ID appended to the same base
/// description). The 4-character floor keeps a very short description
/// (rare in practice - already unlikely to reach here at all, since an
/// empty description is rejected earlier) from prefix-matching everything.
fn descriptions_are_similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    shorter.len() >= 4 && longer.starts_with(shorter)
}

/// Deterministic matching (product handoff section 15/16) - four
/// explainable outcomes, no confidence scoring:
///
/// - `ExactDuplicate`: a byte-identical row already exists (by hash), or
///   the same source-provided external ID already exists for this account -
///   the only outcome certain enough to skip without asking.
/// - `StrongMatch`: no exact identity, but same account + same amount +
///   same date + a matching description - high confidence (e.g. a
///   slightly-reformatted re-export of the same statement), but not
///   certain: two genuine same-merchant same-amount purchases on the same
///   day would look identical to this signal, so it is still surfaced for
///   the owner to decide rather than silently skipped (section 16).
/// - `PossibleMatch`: same amount + matching description within a few days
///   of an existing transaction, but not the same date - plausible (a
///   manual entry vs. the bank's later posting) but not certain, so it is
///   surfaced for the owner to decide rather than silently resolved either
///   way.
/// - `New`: no meaningful match, including two transactions that merely
///   share a date and amount but have different descriptions - two
///   legitimate same-amount purchases must not collapse into each other.
fn match_candidate(
    candidate: &NormalizedCandidateTransaction,
    ledger: &ExistingLedger,
    batch_hashes: &HashMap<String, usize>,
) -> MatchOutcome {
    if let Some(&id) = ledger.hash_to_id.get(&candidate.source_row_hash) {
        return MatchOutcome::ExactDuplicate {
            existing_transaction_id: Some(id),
        };
    }
    if batch_hashes.contains_key(&candidate.source_row_hash) {
        // Repeated within this same file (an export bug); neither
        // occurrence is in the ledger yet, so there is no existing
        // transaction id to point at.
        return MatchOutcome::ExactDuplicate {
            existing_transaction_id: None,
        };
    }
    if let Some(external_id) = &candidate.external_transaction_id {
        if let Some(&id) = ledger.external_id_to_id.get(external_id) {
            return MatchOutcome::ExactDuplicate {
                existing_transaction_id: Some(id),
            };
        }
    }

    let candidate_merchant_lower = candidate.merchant.trim().to_lowercase();
    let candidate_description_lower = candidate.raw_description.trim().to_lowercase();
    let mut possible: Option<i64> = None;
    for row in ledger
        .rows
        .iter()
        .filter(|r| r.amount_cents == candidate.amount_cents)
    {
        let description_matches =
            descriptions_are_similar(&row.merchant_lower, &candidate_merchant_lower)
                || descriptions_are_similar(
                    &row.raw_description_lower,
                    &candidate_description_lower,
                );
        if !description_matches {
            continue;
        }
        if row.date == candidate.transaction_date {
            return MatchOutcome::StrongMatch {
                existing_transaction_id: row.id,
            };
        }
        if possible.is_none()
            && crate::dates::days_between(&row.date, &candidate.transaction_date).abs()
                <= POSSIBLE_MATCH_DAY_WINDOW
        {
            possible = Some(row.id);
        }
    }
    match possible {
        Some(existing_transaction_id) => MatchOutcome::PossibleMatch {
            existing_transaction_id,
        },
        None => MatchOutcome::New,
    }
}

/// Parses every row into a `NormalizedCandidateTransaction` and classifies
/// it against `ledger` (rows already committed for this account) plus rows
/// seen earlier *within this same batch*, so a CSV that genuinely contains
/// the same row twice (an export bug) is still caught.
#[allow(clippy::too_many_arguments)]
fn build_staging_rows(
    conn: &Connection,
    account_id: i64,
    is_liability: bool,
    raw_rows: &[HashMap<String, String>],
    mapping: &ColumnMapping,
    ledger: &ExistingLedger,
    merchant_rules: &[MerchantRule],
) -> Result<StagingResult, String> {
    let payee_id_by_merchant_name: HashMap<String, i64> = payees::list(conn, false)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|p| (p.name.trim().to_lowercase(), p.id))
        .collect();

    let mut rows = Vec::with_capacity(raw_rows.len());
    // Hashes and external IDs seen earlier in *this* batch, mapped to their
    // row_index - so a CSV that genuinely repeats a row, or claims the same
    // external ID twice, is caught even though neither occurrence is in the
    // ledger yet.
    let mut batch_hashes: HashMap<String, usize> = HashMap::new();
    let mut batch_external_ids: HashMap<String, usize> = HashMap::new();
    let (mut new_count, mut needs_review, mut duplicate, mut possible_match) = (0, 0, 0, 0);

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
        }
        .map(|cents| if mapping.invert_amount { -cents } else { cents });

        let csv_category = mapping
            .category_column
            .as_deref()
            .and_then(|c| raw.get(c))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let external_transaction_id = mapping
            .external_id_column
            .as_deref()
            .and_then(|c| raw.get(c))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(ext_id) = &external_transaction_id {
            if let Some(&first_row_index) = batch_external_ids.get(ext_id) {
                errors.push(format!(
                    "Transaction ID '{ext_id}' is also used by row {}; each transaction ID must be unique within a file.",
                    first_row_index + 1
                ));
            } else {
                batch_external_ids.insert(ext_id.clone(), row_index);
            }
        }

        let (status, candidate, duplicate_of_transaction_id, match_detail) = match (
            date,
            description,
            amount,
        ) {
            (Some(date), Some(description), Some(amount_cents)) if errors.is_empty() => {
                // Rule hierarchy (product handoff section 8): an existing
                // user rule outranks whatever category text the statement
                // itself happened to export - a rule reflects the owner's
                // own deliberate correction, a CSV category column is
                // just upstream source data.
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
                let candidate_payee_id = payee_id_by_merchant_name
                    .get(&merchant.trim().to_lowercase())
                    .copied();

                let candidate = NormalizedCandidateTransaction {
                    source_type: ImportSourceType::Csv,
                    account_id,
                    transaction_date: date,
                    posted_date,
                    amount_cents,
                    transaction_type,
                    raw_description: description,
                    merchant,
                    category,
                    subcategory,
                    external_transaction_id,
                    candidate_payee_id,
                    source_row_hash: hash,
                };

                let outcome = match_candidate(&candidate, ledger, &batch_hashes);
                let (status, duplicate_id, detail) = match outcome {
                        MatchOutcome::New => (RowStatus::New, None, None),
                        MatchOutcome::ExactDuplicate {
                            existing_transaction_id,
                        } => (
                            RowStatus::Duplicate,
                            existing_transaction_id,
                            Some("This exact row has already been imported.".to_string()),
                        ),
                        // Strong Match is high-confidence but, per product
                        // handoff section 16, never auto-skipped: two
                        // legitimate same-merchant same-amount purchases on
                        // the same day are indistinguishable from this
                        // signal alone, and "a false-positive Review item"
                        // is explicitly preferred over silently discarding a
                        // real transaction. Only a byte-identical or
                        // external-ID exact match (above) is certain enough
                        // to skip without asking.
                        MatchOutcome::StrongMatch {
                            existing_transaction_id,
                        } => (
                            RowStatus::PossibleMatch,
                            Some(existing_transaction_id),
                            Some(format!(
                                "Likely already imported as transaction #{existing_transaction_id} (same date, amount, and description) - your decision."
                            )),
                        ),
                        MatchOutcome::PossibleMatch {
                            existing_transaction_id,
                        } => (
                            RowStatus::PossibleMatch,
                            Some(existing_transaction_id),
                            Some(format!(
                                "Close to existing transaction #{existing_transaction_id} (same amount and a matching description, within a few days) - your decision."
                            )),
                        ),
                    };
                if outcome == MatchOutcome::New {
                    batch_hashes.insert(candidate.source_row_hash.clone(), row_index);
                }
                (status, Some(candidate), duplicate_id, detail)
            }
            _ => (RowStatus::NeedsReview, None, None, None),
        };

        match status {
            RowStatus::New => new_count += 1,
            RowStatus::NeedsReview => needs_review += 1,
            RowStatus::Duplicate => duplicate += 1,
            RowStatus::PossibleMatch => possible_match += 1,
        }

        rows.push(StagingRow {
            row_index,
            raw: raw.clone(),
            status,
            errors,
            candidate,
            duplicate_of_transaction_id,
            match_detail,
        });
    }

    Ok(StagingResult {
        rows,
        new_count,
        needs_review_count: needs_review,
        duplicate_count: duplicate,
        possible_match_count: possible_match,
    })
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
    check_mapping_matches_headers(mapping, &headers)?;
    let ledger = load_existing_ledger(conn, account_id)?;
    let merchant_rules = rules::relevant_rules(conn, account_id).map_err(|e| e.to_string())?;
    build_staging_rows(
        conn,
        account_id,
        is_liability,
        &rows,
        mapping,
        &ledger,
        &merchant_rules,
    )
}

/// Fails fast and clearly when an applied mapping (typically a saved
/// profile) doesn't fit this file, rather than staging every row as
/// individually invalid (product handoff section 28: "incompatible saved
/// profile" is its own error).
fn check_mapping_matches_headers(
    mapping: &ColumnMapping,
    headers: &[String],
) -> Result<(), String> {
    let missing = mapping.missing_headers(headers);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "This file doesn't have the column(s) this mapping expects: {}. Choose a different profile or map the columns manually.",
            missing.join(", ")
        ))
    }
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
    pub duplicate_count: usize,
    pub sent_to_review_count: usize,
    pub invalid_count: usize,
    pub excluded_count: usize,
}

#[allow(clippy::too_many_arguments)]
pub fn commit(
    conn: &mut Connection,
    account_id: i64,
    source_filename: &str,
    csv_content: &str,
    mapping: &ColumnMapping,
    import_profile_id: Option<i64>,
    force_import_row_indices: &HashSet<usize>,
    excluded_row_indices: &HashSet<usize>,
) -> Result<ImportSummary, String> {
    let is_liability = accounts::account_type_of(conn, account_id)
        .map_err(|e| e.to_string())?
        .is_liability();
    let (headers, rows) = read_csv(csv_content)?;
    check_mapping_matches_headers(mapping, &headers)?;
    if let Some(profile_id) = import_profile_id {
        if crate::import_profiles::get(conn, profile_id)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Err(format!("Import profile {profile_id} no longer exists."));
        }
    }
    let ledger = load_existing_ledger(conn, account_id)?;
    let merchant_rules = rules::relevant_rules(conn, account_id).map_err(|e| e.to_string())?;
    let staged = build_staging_rows(
        conn,
        account_id,
        is_liability,
        &rows,
        mapping,
        &ledger,
        &merchant_rules,
    )?;

    let file_hash = {
        let mut hasher = Sha256::new();
        hasher.update(csv_content.as_bytes());
        to_hex(&hasher.finalize())
    };
    let mapping_json = serde_json::to_string(mapping).map_err(|e| e.to_string())?;

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut committed = 0usize;
    let mut duplicates = 0usize;
    let mut sent_to_review = 0usize;
    let mut invalid = 0usize;
    let mut excluded = 0usize;

    let import_id: i64 = tx
        .query_row(
            "INSERT INTO imports (account_id, source_filename, file_hash, row_count, committed_count, duplicate_count, needs_review_count, excluded_count, column_mapping_json, source_type, import_profile_id)
             VALUES (?1, ?2, ?3, ?4, 0, 0, 0, 0, ?5, 'csv', ?6) RETURNING id",
            params![
                account_id,
                source_filename,
                file_hash,
                staged.rows.len() as i64,
                mapping_json,
                import_profile_id,
            ],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    for row in &staged.rows {
        if excluded_row_indices.contains(&row.row_index) {
            excluded += 1;
            continue;
        }
        match &row.status {
            RowStatus::Duplicate => {
                duplicates += 1;
            }
            RowStatus::NeedsReview => {
                invalid += 1;
                insert_review_item(
                    &tx,
                    "malformed_row",
                    None,
                    Some(import_id),
                    row,
                    &format!(
                        "Row {} could not be used: {}",
                        row.row_index + 1,
                        row.errors.join("; ")
                    ),
                )
                .map_err(|e| e.to_string())?;
            }
            RowStatus::PossibleMatch if !force_import_row_indices.contains(&row.row_index) => {
                sent_to_review += 1;
                // match_detail should always be set for a PossibleMatch row,
                // but the fallback must never fabricate a fake reference if
                // that assumption is ever wrong somewhere upstream - an
                // honest, slightly vaguer message beats a misleading
                // specific one.
                let reason = row.match_detail.clone().unwrap_or_else(|| {
                    format!(
                        "Row {} may already exist in this account - review before importing.",
                        row.row_index + 1
                    )
                });
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
            RowStatus::New | RowStatus::PossibleMatch => {
                if let Some(candidate) = &row.candidate {
                    tx.execute(
                        "INSERT INTO transactions (
                            account_id, transaction_date, posted_date, amount_cents, transaction_type,
                            raw_description, merchant, category, subcategory, source_import_id, source_row_hash,
                            payee_id, external_transaction_id
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                        params![
                            account_id,
                            candidate.transaction_date,
                            candidate.posted_date,
                            candidate.amount_cents,
                            candidate.transaction_type.as_sql_str(),
                            candidate.raw_description,
                            candidate.merchant,
                            candidate.category,
                            candidate.subcategory,
                            import_id,
                            candidate.source_row_hash,
                            candidate.candidate_payee_id,
                            candidate.external_transaction_id,
                        ],
                    )
                    .map_err(|e| {
                        let msg = e.to_string();
                        if msg.contains("UNIQUE constraint failed") {
                            format!(
                                "Row {} has a transaction ID already used by another transaction in this account.",
                                row.row_index + 1
                            )
                        } else {
                            msg
                        }
                    })?;
                    committed += 1;
                }
            }
        }
    }

    tx.execute(
        "UPDATE imports SET committed_count = ?1, duplicate_count = ?2, needs_review_count = ?3, excluded_count = ?4 WHERE id = ?5",
        params![
            committed as i64,
            duplicates as i64,
            (sent_to_review + invalid) as i64,
            excluded as i64,
            import_id,
        ],
    )
    .map_err(|e| e.to_string())?;

    tx.commit().map_err(|e| e.to_string())?;

    Ok(ImportSummary {
        import_id,
        committed_count: committed,
        duplicate_count: duplicates,
        sent_to_review_count: sent_to_review,
        invalid_count: invalid,
        excluded_count: excluded,
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
    import_profile_id: Option<i64>,
    force_import_row_indices: Vec<usize>,
    excluded_row_indices: Vec<usize>,
) -> Result<ImportSummary, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let force_set: HashSet<usize> = force_import_row_indices.into_iter().collect();
    let excluded_set: HashSet<usize> = excluded_row_indices.into_iter().collect();
    commit(
        &mut conn,
        account_id,
        &source_filename,
        &csv_content,
        &mapping,
        import_profile_id,
        &force_set,
        &excluded_set,
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
        .candidate
        .as_ref()
        .map(|p| serde_json::to_string(p).unwrap_or_default());
    conn.execute(
        "INSERT INTO needs_review_items (issue_type, transaction_id, import_id, source_row_json, detected_reason, proposed_resolution_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![issue_type, transaction_id, import_id, source_row_json, reason, proposed_json],
    )?;
    Ok(())
}

// --- Import History (product handoff section 11/12) ---

/// One past ingestion event - stays visible in history even after an Undo
/// removes some or all of its transactions, so "this import happened, then
/// was undone" remains an honest, reconstructable fact rather than erased.
#[derive(Debug, Serialize)]
pub struct ImportRecord {
    pub id: i64,
    pub account_id: i64,
    pub account_name: String,
    pub source_type: String,
    pub source_filename: String,
    pub import_profile_id: Option<i64>,
    pub import_profile_name: Option<String>,
    pub imported_at: String,
    pub row_count: i64,
    pub committed_count: i64,
    pub duplicate_count: i64,
    pub needs_review_count: i64,
    pub excluded_count: i64,
    pub undone_at: Option<String>,
    pub undone_removed_count: Option<i64>,
    /// How many of this import's transactions still exist right now -
    /// computed live (the same "computed, not stored" approach ADR-0007
    /// uses for account balance) rather than trusted from `committed_count`,
    /// which never changes even after an Undo removes some of them.
    pub remaining_transaction_count: i64,
}

const IMPORT_RECORD_COLUMNS: &str =
    "i.id, i.account_id, a.name, i.source_type, i.source_filename, \
    i.import_profile_id, p.name, i.imported_at, i.row_count, i.committed_count, i.duplicate_count, \
    i.needs_review_count, i.excluded_count, i.undone_at, i.undone_removed_count, \
    (SELECT count(*) FROM transactions t WHERE t.source_import_id = i.id)";

fn row_to_import_record(row: &rusqlite::Row) -> rusqlite::Result<ImportRecord> {
    Ok(ImportRecord {
        id: row.get(0)?,
        account_id: row.get(1)?,
        account_name: row.get(2)?,
        source_type: row.get(3)?,
        source_filename: row.get(4)?,
        import_profile_id: row.get(5)?,
        import_profile_name: row.get(6)?,
        imported_at: row.get(7)?,
        row_count: row.get(8)?,
        committed_count: row.get(9)?,
        duplicate_count: row.get(10)?,
        needs_review_count: row.get(11)?,
        excluded_count: row.get(12)?,
        undone_at: row.get(13)?,
        undone_removed_count: row.get(14)?,
        remaining_transaction_count: row.get(15)?,
    })
}

pub fn list(conn: &Connection) -> Result<Vec<ImportRecord>, String> {
    let sql = format!(
        "SELECT {IMPORT_RECORD_COLUMNS} FROM imports i
         JOIN accounts a ON a.id = i.account_id
         LEFT JOIN import_profiles p ON p.id = i.import_profile_id
         ORDER BY i.imported_at DESC, i.id DESC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], row_to_import_record)
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_imports(state: State<DbState>) -> Result<Vec<ImportRecord>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list(&conn)
}

// --- Undo Import (product handoff section 22-25) ---

#[derive(Debug, Serialize)]
pub struct UndoPreview {
    pub import_id: i64,
    pub total_transaction_count: i64,
    pub removable_count: i64,
    pub protected_count: i64,
}

#[derive(Debug, Serialize)]
pub struct UndoResult {
    pub import_id: i64,
    pub removed_count: i64,
    pub protected_count: i64,
}

fn get_undone_at(conn: &Connection, import_id: i64) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT undone_at FROM imports WHERE id = ?1",
        params![import_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("Import {import_id} not found."))
}

/// A transaction from this import is removable only if it has never been
/// touched since MIDAS created it (`created_at == updated_at` - covers a
/// manual edit, an archive, or a reconciliation status change, since every
/// one of those already updates `updated_at`) and nothing later split from
/// it. Anything else is protected and left exactly as-is (product handoff
/// section 22).
fn removable_transaction_ids(conn: &Connection, import_id: i64) -> Result<(Vec<i64>, i64), String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.id FROM transactions t
             WHERE t.source_import_id = ?1
               AND t.created_at = t.updated_at
               AND NOT EXISTS (SELECT 1 FROM transactions c WHERE c.split_parent_transaction_id = t.id)",
        )
        .map_err(|e| e.to_string())?;
    let removable: Vec<i64> = stmt
        .query_map(params![import_id], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let total: i64 = conn
        .query_row(
            "SELECT count(*) FROM transactions WHERE source_import_id = ?1",
            params![import_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let protected = total - removable.len() as i64;
    Ok((removable, protected))
}

/// Read-only - safe to call any number of times, including after the
/// import has already been undone (it just reports fewer/no removable
/// transactions).
pub fn preview_undo(conn: &Connection, import_id: i64) -> Result<UndoPreview, String> {
    get_undone_at(conn, import_id)?;
    let (removable, protected) = removable_transaction_ids(conn, import_id)?;
    Ok(UndoPreview {
        import_id,
        total_transaction_count: removable.len() as i64 + protected,
        removable_count: removable.len() as i64,
        protected_count: protected,
    })
}

/// Removes every removable transaction from `import_id` in one database
/// transaction and records the outcome on the `imports` row - always a
/// partial-or-full undo of exactly the safe subset, never all-or-nothing
/// (product handoff section 23-24): "Never display 'Import undone' when
/// meaningful records still remain" is the caller's job, using
/// `protected_count` in the result. Refuses to run twice on the same
/// import so a second accidental call can never overwrite the true
/// `undone_removed_count` of the first with a smaller/wrong number.
pub fn undo(conn: &mut Connection, import_id: i64) -> Result<UndoResult, String> {
    if get_undone_at(conn, import_id)?.is_some() {
        return Err(format!("Import {import_id} has already been undone."));
    }
    let (removable, protected) = removable_transaction_ids(conn, import_id)?;

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for id in &removable {
        tx.execute("DELETE FROM transactions WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
    }
    tx.execute(
        "UPDATE imports SET undone_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), undone_removed_count = ?1 WHERE id = ?2",
        params![removable.len() as i64, import_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;

    Ok(UndoResult {
        import_id,
        removed_count: removable.len() as i64,
        protected_count: protected,
    })
}

#[tauri::command]
pub fn preview_undo_import(state: State<DbState>, import_id: i64) -> Result<UndoPreview, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    preview_undo(&conn, import_id)
}

#[tauri::command]
pub fn undo_import(state: State<DbState>, import_id: i64) -> Result<UndoResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    undo(&mut conn, import_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::import_profiles::{ImportProfileInput, ImportSourceType};
    use crate::payees::{self, PayeeInput, PayeeType};
    use crate::rules::{MatchType, MerchantRuleInput};
    use crate::transactions::{TransactionFilter, TransactionInput};

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
        conn.execute_batch(include_str!("../migrations/0009_import_engine_v2.sql"))
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
            external_id_column: None,
            invert_amount: false,
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
            external_id_column: None,
            invert_amount: false,
        }
    }

    fn commit_simple(
        conn: &mut Connection,
        account_id: i64,
        source_filename: &str,
        csv_content: &str,
        mapping: &ColumnMapping,
    ) -> Result<ImportSummary, String> {
        commit(
            conn,
            account_id,
            source_filename,
            csv_content,
            mapping,
            None,
            &HashSet::new(),
            &HashSet::new(),
        )
    }

    fn commit_forcing(
        conn: &mut Connection,
        account_id: i64,
        source_filename: &str,
        csv_content: &str,
        mapping: &ColumnMapping,
        force: &HashSet<usize>,
    ) -> Result<ImportSummary, String> {
        commit(
            conn,
            account_id,
            source_filename,
            csv_content,
            mapping,
            None,
            force,
            &HashSet::new(),
        )
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
    fn detect_mapping_recognizes_external_id_headers() {
        let headers = vec![
            "Date".to_string(),
            "Description".to_string(),
            "Amount".to_string(),
            "Reference Number".to_string(),
        ];
        let mapping = detect_mapping(&headers);
        assert_eq!(
            mapping.external_id_column.as_deref(),
            Some("Reference Number")
        );
    }

    #[test]
    fn detect_mapping_is_not_confident_on_unrecognized_headers() {
        let headers = vec!["Col A".to_string(), "Col B".to_string()];
        let mapping = detect_mapping(&headers);
        assert!(!mapping.is_usable());
    }

    #[test]
    fn stages_a_normal_signed_amount_csv_as_new() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv =
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n2026-03-02,PAYCHECK,2000.00\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(result.new_count, 2);
        assert_eq!(result.needs_review_count, 0);
        let candidate = result.rows[0].candidate.as_ref().unwrap();
        assert_eq!(candidate.amount_cents, -450);
        assert_eq!(candidate.transaction_type, TransactionType::Purchase);
        let income = result.rows[1].candidate.as_ref().unwrap();
        assert_eq!(income.amount_cents, 200000);
        assert_eq!(income.transaction_type, TransactionType::Income);
    }

    #[test]
    fn stages_separate_debit_credit_columns_correctly_by_account_type() {
        let conn = test_conn();
        let checking_id = make_account(&conn, AccountType::Checking);
        let csv = "Date,Description,Debit,Credit\n2026-03-01,GROCERY STORE,42.10,\n2026-03-02,REFUND,,10.00\n";
        let result = stage(&conn, checking_id, csv, &debit_credit_mapping()).unwrap();
        assert_eq!(result.new_count, 2);
        assert_eq!(
            result.rows[0].candidate.as_ref().unwrap().amount_cents,
            -4210
        );
        assert_eq!(
            result.rows[1].candidate.as_ref().unwrap().amount_cents,
            1000
        );

        // Same debit/credit values on a credit card: sign flips (ADR-0004).
        let card_id = make_account(&conn, AccountType::CreditCard);
        let card_result = stage(&conn, card_id, csv, &debit_credit_mapping()).unwrap();
        assert_eq!(
            card_result.rows[0].candidate.as_ref().unwrap().amount_cents,
            4210
        );
        assert_eq!(
            card_result.rows[1].candidate.as_ref().unwrap().amount_cents,
            -1000
        );
    }

    #[test]
    fn invert_amount_flips_the_parsed_sign() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let mut mapping = signed_amount_mapping();
        mapping.invert_amount = true;
        // The statement's own convention is backwards relative to MIDAS's
        // normal parsing for this account - a purchase exported as positive.
        let csv = "Date,Description,Amount\n2026-03-01,COFFEE SHOP,4.50\n";
        let result = stage(&conn, account_id, csv, &mapping).unwrap();
        assert_eq!(
            result.rows[0].candidate.as_ref().unwrap().amount_cents,
            -450
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
        let candidate = result.rows[0].candidate.as_ref().unwrap();
        assert_eq!(candidate.merchant, "Coffee Shop");
        assert_eq!(candidate.category.as_deref(), Some("Dining & Restaurants"));
        assert_eq!(
            candidate.raw_description, "SQ *COFFEE SHOP 123 MIAMI FL",
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
        let candidate = result.rows[0].candidate.as_ref().unwrap();
        assert_eq!(
            candidate.category.as_deref(),
            Some("Subscriptions"),
            "the rule wins over the CSV's own category"
        );
    }

    #[test]
    fn payee_is_linked_when_the_resolved_merchant_matches_an_existing_payee() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let payee = payees::create(
            &conn,
            &PayeeInput {
                name: "Whole Foods".to_string(),
                payee_type: PayeeType::Merchant,
                notes: None,
            },
        )
        .unwrap();
        rules::create(
            &conn,
            &MerchantRuleInput {
                match_type: MatchType::Contains,
                match_value: "whole foods".to_string(),
                account_id: None,
                normalized_merchant: Some("Whole Foods".to_string()),
                category: None,
                subcategory: None,
                priority: 0,
            },
        )
        .unwrap();

        let csv = "Date,Description,Amount\n2026-03-01,WHOLE FOODS #4471,-52.10\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(
            result.rows[0]
                .candidate
                .as_ref()
                .unwrap()
                .candidate_payee_id,
            Some(payee.id)
        );

        // The link is persisted on commit, not just visible in preview.
        let mut conn = conn;
        let summary = commit_simple(
            &mut conn,
            account_id,
            "wf.csv",
            csv,
            &signed_amount_mapping(),
        )
        .unwrap();
        assert_eq!(summary.committed_count, 1);
        let committed = crate::transactions::list(
            &conn,
            &TransactionFilter {
                account_id: Some(account_id),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(committed[0].payee_id, Some(payee.id));
    }

    #[test]
    fn currency_symbols_and_parentheses_negatives_parse_in_staging() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv = "Date,Description,Amount\n2026-03-01,STORE,\"($42.10)\"\n2026-03-02,DEPOSIT,\"$1,000.00\"\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(result.new_count, 2);
        assert_eq!(
            result.rows[0].candidate.as_ref().unwrap().amount_cents,
            -4210
        );
        assert_eq!(
            result.rows[1].candidate.as_ref().unwrap().amount_cents,
            100000
        );
    }

    #[test]
    fn malformed_amount_and_missing_required_values_need_review_not_zero() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv = "Date,Description,Amount\n2026-03-01,BAD AMOUNT,not-a-number\n2026-03-02,,5.00\n,MISSING DATE,5.00\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(result.needs_review_count, 3);
        assert_eq!(result.new_count, 0);
        for row in &result.rows {
            assert!(matches!(row.status, RowStatus::NeedsReview));
            assert!(row.candidate.is_none());
            assert!(!row.errors.is_empty());
        }
    }

    #[test]
    fn supported_and_malformed_dates_in_staging() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv = "Date,Description,Amount\n03/04/2026,US SLASH,5.00\n2026-13-40,BAD DATE,5.00\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(result.new_count, 1);
        assert_eq!(
            result.rows[0].candidate.as_ref().unwrap().transaction_date,
            "2026-03-04"
        );
        assert_eq!(result.needs_review_count, 1);
    }

    #[test]
    fn two_different_merchants_same_day_same_amount_are_not_flagged() {
        // The central safety case (product handoff section 16): two
        // legitimate, unrelated purchases must never collapse into each
        // other merely because they share a date and amount.
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        commit_simple(
            &mut conn,
            account_id,
            "first.csv",
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-9.99\n",
            &signed_amount_mapping(),
        )
        .unwrap();

        let result = stage(
            &conn,
            account_id,
            "Date,Description,Amount\n2026-03-01,GAS STATION,-9.99\n",
            &signed_amount_mapping(),
        )
        .unwrap();
        assert_eq!(result.new_count, 1);
        assert_eq!(result.possible_match_count, 0);
        assert_eq!(result.duplicate_count, 0);
    }

    #[test]
    fn recurring_identical_charges_on_different_dates_are_not_flagged() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        commit_simple(
            &mut conn,
            account_id,
            "jan.csv",
            "Date,Description,Amount\n2026-01-15,NETFLIX,-15.49\n",
            &signed_amount_mapping(),
        )
        .unwrap();

        // A month later - same merchant, same amount, far outside the
        // possible-match date window.
        let result = stage(
            &conn,
            account_id,
            "Date,Description,Amount\n2026-02-15,NETFLIX,-15.49\n",
            &signed_amount_mapping(),
        )
        .unwrap();
        assert_eq!(result.new_count, 1);
        assert_eq!(result.possible_match_count, 0);
    }

    #[test]
    fn a_manual_entry_close_in_date_to_a_bank_posting_is_a_possible_match() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        // The owner wrote a check and logged it manually the day it was sent.
        crate::transactions::create(
            &conn,
            &TransactionInput {
                account_id,
                transaction_date: "2026-03-01".to_string(),
                posted_date: None,
                amount_cents: -8500,
                transaction_type: TransactionType::Purchase,
                raw_description: "Electric Company".to_string(),
                merchant: None,
                category: None,
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
            },
        )
        .unwrap();

        // The bank posts the same payment two days later.
        let result = stage(
            &conn,
            account_id,
            "Date,Description,Amount\n2026-03-03,ELECTRIC COMPANY,-85.00\n",
            &signed_amount_mapping(),
        )
        .unwrap();
        assert_eq!(result.possible_match_count, 1);
        assert!(matches!(result.rows[0].status, RowStatus::PossibleMatch));
        assert!(result.rows[0]
            .match_detail
            .as_deref()
            .unwrap()
            .contains("Close to existing transaction"));
    }

    #[test]
    fn a_slightly_reformatted_reexport_is_a_strong_match_and_goes_to_review() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        commit_simple(
            &mut conn,
            account_id,
            "first.csv",
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n",
            &signed_amount_mapping(),
        )
        .unwrap();

        // A second, overlapping export: same date/amount and the same base
        // description with a reference suffix appended - not byte-identical
        // (so not an exact duplicate), but high-confidence, never silently
        // auto-skipped (section 16) - surfaced for a decision instead.
        let second_csv = "Date,Description,Amount\n2026-03-01,COFFEE SHOP #4471,-4.50\n";
        let staged = stage(&conn, account_id, second_csv, &signed_amount_mapping()).unwrap();
        assert_eq!(staged.possible_match_count, 1);
        assert!(staged.rows[0]
            .match_detail
            .as_deref()
            .unwrap()
            .contains("Likely already imported"));

        let summary_review = commit_simple(
            &mut conn,
            account_id,
            "second.csv",
            second_csv,
            &signed_amount_mapping(),
        )
        .unwrap();
        assert_eq!(summary_review.committed_count, 0);
        assert_eq!(summary_review.sent_to_review_count, 1);

        let review_items = crate::needs_review::list(&conn, true).unwrap();
        assert_eq!(review_items.len(), 1);
        assert_eq!(review_items[0].issue_type, "suspected_duplicate");

        // The owner decides it's legitimate and forces the import.
        let force: HashSet<usize> = [0].into_iter().collect();
        let summary_forced = commit_forcing(
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
    fn exact_duplicate_within_the_same_file_is_flagged() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        // An export bug: the identical row appears twice.
        let csv =
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n2026-03-01,COFFEE SHOP,-4.50\n";
        let result = stage(&conn, account_id, csv, &signed_amount_mapping()).unwrap();
        assert_eq!(result.new_count, 1);
        assert_eq!(result.duplicate_count, 1);
    }

    #[test]
    fn external_transaction_id_is_the_strongest_duplicate_identity() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let mut mapping = signed_amount_mapping();
        mapping.external_id_column = Some("RefID".to_string());

        commit_simple(
            &mut conn,
            account_id,
            "first.csv",
            "Date,Description,Amount,RefID\n2026-03-01,COFFEE SHOP,-4.50,TXN001\n",
            &mapping,
        )
        .unwrap();

        // A completely different date and description, but the bank's own
        // transaction ID is unchanged - this is certain enough to skip
        // without asking, unlike a Strong/Possible match.
        let result = stage(
            &conn,
            account_id,
            "Date,Description,Amount,RefID\n2026-04-15,COFFEE SHOP RENAMED,-4.50,TXN001\n",
            &mapping,
        )
        .unwrap();
        assert_eq!(result.duplicate_count, 1);
        assert_eq!(result.new_count, 0);
        assert_eq!(result.possible_match_count, 0);
    }

    #[test]
    fn duplicate_external_transaction_id_within_the_same_file_needs_review() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let mut mapping = signed_amount_mapping();
        mapping.external_id_column = Some("RefID".to_string());
        let csv = "Date,Description,Amount,RefID\n2026-03-01,COFFEE SHOP,-4.50,TXN001\n2026-03-02,GROCERY STORE,-20.00,TXN001\n";
        let result = stage(&conn, account_id, csv, &mapping).unwrap();
        assert_eq!(result.new_count, 1, "the first occurrence is unaffected");
        assert_eq!(result.needs_review_count, 1);
        assert!(result.rows[1].errors[0].contains("also used by row 1"));
    }

    #[test]
    fn commit_rejects_a_mapping_that_does_not_match_the_files_headers() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv = "TransDate,Memo,Amt\n2026-03-01,COFFEE SHOP,-4.50\n";
        let err = commit_simple(
            &mut conn,
            account_id,
            "x.csv",
            csv,
            &signed_amount_mapping(),
        )
        .unwrap_err();
        assert!(err.contains("doesn't have the column"));
    }

    #[test]
    fn excluded_rows_are_skipped_and_counted_without_review_or_commit() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv =
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n2026-03-02,PAYCHECK,2000.00\n";
        let excluded: HashSet<usize> = [0].into_iter().collect();
        let summary = commit(
            &mut conn,
            account_id,
            "statement.csv",
            csv,
            &signed_amount_mapping(),
            None,
            &HashSet::new(),
            &excluded,
        )
        .unwrap();
        assert_eq!(summary.committed_count, 1);
        assert_eq!(summary.excluded_count, 1);
        assert!(crate::needs_review::list(&conn, true).unwrap().is_empty());

        let committed = crate::transactions::list(
            &conn,
            &TransactionFilter {
                account_id: Some(account_id),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].raw_description, "PAYCHECK");
    }

    #[test]
    fn commit_records_the_import_profile_used() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let profile = crate::import_profiles::create(
            &conn,
            &ImportProfileInput {
                name: "Test Bank".to_string(),
                account_id: Some(account_id),
                source_type: ImportSourceType::Csv,
                column_mapping: signed_amount_mapping(),
            },
        )
        .unwrap();

        let summary = commit(
            &mut conn,
            account_id,
            "statement.csv",
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n",
            &signed_amount_mapping(),
            Some(profile.id),
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();

        let stored_profile_id: Option<i64> = conn
            .query_row(
                "SELECT import_profile_id FROM imports WHERE id = ?1",
                params![summary.import_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored_profile_id, Some(profile.id));
    }

    #[test]
    fn commit_rejects_a_nonexistent_import_profile_id() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let err = commit(
            &mut conn,
            account_id,
            "statement.csv",
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n",
            &signed_amount_mapping(),
            Some(999_999),
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap_err();
        assert!(err.contains("no longer exists"));
    }

    #[test]
    fn successful_commit_creates_transactions_and_records_import_metadata() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv =
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n2026-03-02,PAYCHECK,2000.00\n";
        let summary = commit_simple(
            &mut conn,
            account_id,
            "statement.csv",
            csv,
            &signed_amount_mapping(),
        )
        .unwrap();
        assert_eq!(summary.committed_count, 2);
        assert_eq!(summary.duplicate_count, 0);
        assert_eq!(summary.sent_to_review_count, 0);

        let committed = crate::transactions::list(
            &conn,
            &TransactionFilter {
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

        let first = commit_simple(
            &mut conn,
            account_id,
            "statement.csv",
            csv,
            &signed_amount_mapping(),
        )
        .unwrap();
        assert_eq!(first.committed_count, 2);

        // Owner re-imports the exact same statement, e.g. by mistake -
        // including after a simulated restart (a fresh call against the
        // same persisted ledger, not any in-memory state).
        let second = commit_simple(
            &mut conn,
            account_id,
            "statement.csv",
            csv,
            &signed_amount_mapping(),
        )
        .unwrap();
        assert_eq!(second.committed_count, 0);
        assert_eq!(second.duplicate_count, 2);

        let all = crate::transactions::list(
            &conn,
            &TransactionFilter {
                account_id: Some(account_id),
                include_archived: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 2, "re-import must not have added new rows");
    }

    #[test]
    fn invalid_rows_go_to_needs_review_and_are_never_committed() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let csv = "Date,Description,Amount\n2026-03-01,BAD ROW,not-a-number\n";
        let summary = commit_simple(
            &mut conn,
            account_id,
            "bad.csv",
            csv,
            &signed_amount_mapping(),
        )
        .unwrap();
        assert_eq!(summary.committed_count, 0);
        assert_eq!(summary.invalid_count, 1);

        let review_items = crate::needs_review::list(&conn, true).unwrap();
        assert_eq!(review_items.len(), 1);
        assert_eq!(review_items[0].issue_type, "malformed_row");

        let committed = crate::transactions::list(
            &conn,
            &TransactionFilter {
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

    // --- Import History ---

    #[test]
    fn list_returns_imports_newest_first_with_account_and_profile_names() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let profile = crate::import_profiles::create(
            &conn,
            &ImportProfileInput {
                name: "Test Bank".to_string(),
                account_id: Some(account_id),
                source_type: ImportSourceType::Csv,
                column_mapping: signed_amount_mapping(),
            },
        )
        .unwrap();

        commit_simple(
            &mut conn,
            account_id,
            "first.csv",
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n",
            &signed_amount_mapping(),
        )
        .unwrap();
        commit(
            &mut conn,
            account_id,
            "second.csv",
            "Date,Description,Amount\n2026-03-05,GROCERY STORE,-20.00\n",
            &signed_amount_mapping(),
            Some(profile.id),
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();

        let records = list(&conn).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].source_filename, "second.csv", "newest first");
        assert_eq!(records[0].account_name, "Test Account");
        assert_eq!(records[0].import_profile_name.as_deref(), Some("Test Bank"));
        assert_eq!(records[0].remaining_transaction_count, 1);
        assert_eq!(records[1].source_filename, "first.csv");
        assert_eq!(records[1].import_profile_name, None);
    }

    // --- Undo Import ---

    #[test]
    fn undo_removes_untouched_imported_transactions_and_restores_the_balance() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let summary = commit_simple(
            &mut conn,
            account_id,
            "statement.csv",
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n2026-03-02,PAYCHECK,2000.00\n",
            &signed_amount_mapping(),
        )
        .unwrap();

        let result = undo(&mut conn, summary.import_id).unwrap();
        assert_eq!(result.removed_count, 2);
        assert_eq!(result.protected_count, 0);

        let remaining: i64 = conn
            .query_row(
                "SELECT count(*) FROM transactions WHERE source_import_id = ?1",
                params![summary.import_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);

        let (undone_at, undone_removed_count): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT undone_at, undone_removed_count FROM imports WHERE id = ?1",
                params![summary.import_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(undone_at.is_some());
        assert_eq!(undone_removed_count, Some(2));
    }

    #[test]
    fn undo_protects_a_transaction_edited_after_import() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let summary = commit_simple(
            &mut conn,
            account_id,
            "statement.csv",
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n2026-03-02,PAYCHECK,2000.00\n",
            &signed_amount_mapping(),
        )
        .unwrap();
        let imported = crate::transactions::list(
            &conn,
            &TransactionFilter {
                source_import_id: Some(summary.import_id),
                ..Default::default()
            },
        )
        .unwrap();
        let edited = imported
            .iter()
            .find(|t| t.raw_description == "COFFEE SHOP")
            .unwrap();

        // The owner corrects the category after import - a real edit.
        let mut edit_input = TransactionInput {
            account_id,
            transaction_date: edited.transaction_date.clone(),
            posted_date: None,
            amount_cents: edited.amount_cents,
            transaction_type: edited.transaction_type,
            raw_description: edited.raw_description.clone(),
            merchant: Some(edited.merchant.clone()),
            category: Some("Dining".to_string()),
            subcategory: None,
            notes: None,
            tags: None,
            is_pending: None,
        };
        crate::transactions::update(&conn, edited.id, &edit_input).unwrap();
        edit_input.category = None;

        let preview = preview_undo(&conn, summary.import_id).unwrap();
        assert_eq!(preview.total_transaction_count, 2);
        assert_eq!(preview.removable_count, 1);
        assert_eq!(preview.protected_count, 1);

        let result = undo(&mut conn, summary.import_id).unwrap();
        assert_eq!(result.removed_count, 1);
        assert_eq!(result.protected_count, 1);
        assert!(
            crate::transactions::list(
                &conn,
                &TransactionFilter {
                    source_import_id: Some(summary.import_id),
                    include_archived: true,
                    ..Default::default()
                },
            )
            .unwrap()
            .iter()
            .any(|t| t.id == edited.id),
            "the edited transaction must survive"
        );
    }

    #[test]
    fn undo_refuses_to_run_twice() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let summary = commit_simple(
            &mut conn,
            account_id,
            "statement.csv",
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n",
            &signed_amount_mapping(),
        )
        .unwrap();
        undo(&mut conn, summary.import_id).unwrap();
        let err = undo(&mut conn, summary.import_id).unwrap_err();
        assert!(err.contains("already been undone"));
    }

    #[test]
    fn undo_and_preview_undo_error_on_a_missing_import() {
        let mut conn = test_conn();
        assert!(preview_undo(&conn, 999_999).is_err());
        assert!(undo(&mut conn, 999_999).is_err());
    }

    #[test]
    fn undo_never_touches_an_unrelated_imports_transactions() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let first = commit_simple(
            &mut conn,
            account_id,
            "first.csv",
            "Date,Description,Amount\n2026-03-01,COFFEE SHOP,-4.50\n",
            &signed_amount_mapping(),
        )
        .unwrap();
        let second = commit_simple(
            &mut conn,
            account_id,
            "second.csv",
            "Date,Description,Amount\n2026-04-01,GROCERY STORE,-20.00\n",
            &signed_amount_mapping(),
        )
        .unwrap();

        undo(&mut conn, first.import_id).unwrap();

        let second_still_present = crate::transactions::list(
            &conn,
            &TransactionFilter {
                source_import_id: Some(second.import_id),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            second_still_present.len(),
            1,
            "a different import's data must be untouched"
        );
    }
}
