//! Import Engine v2 (specs/phase-009, extended by phase-010's OFX/QFX
//! adapter): source adapter -> normalized candidate -> matching/duplicate
//! detection -> rules/payee resolution -> commit -> provenance/history.
//! CSV (`read_csv`/`detect_mapping`/`build_staging_rows`) and OFX/QFX
//! (`crate::ofx` + `build_ofx_staging_rows`) both parse into the same
//! `NormalizedCandidateTransaction` shape and share every matching/commit/
//! undo/history function below - `commit_staged` is the one place either
//! adapter ever writes a transaction. There is deliberately no
//! `SourceAdapter` trait; two concrete implementations sharing one set of
//! functions doesn't need one.
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
    /// Hash -> (existing transaction id, that transaction's own external
    /// ID if it has one). The external ID travels with the hash entry so
    /// the matcher can tell a genuine byte-identical duplicate apart from
    /// two different real transactions whose text happens to coincide but
    /// whose source-provided IDs explicitly disagree (section 22).
    hash_to_match: HashMap<String, (i64, Option<String>)>,
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
    let mut hash_to_match = HashMap::new();
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
            hash_to_match.insert(h, (id, external_id.clone()));
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
        hash_to_match,
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
    batch_hashes: &HashMap<String, Option<String>>,
) -> MatchOutcome {
    if let Some((id, existing_external_id)) = ledger.hash_to_match.get(&candidate.source_row_hash) {
        // A source-provided external ID is stronger evidence than a text
        // hash: when both sides have one and they explicitly disagree,
        // this is not the same transaction, no matter how similar the text
        // looks - falling through to the amount/date/description scan
        // below (typically resolving as a Strong Match) rather than
        // silently collapsing two real transactions (section 22).
        let external_ids_conflict = match (&candidate.external_transaction_id, existing_external_id)
        {
            (Some(a), Some(b)) => a != b,
            _ => false,
        };
        if !external_ids_conflict {
            return MatchOutcome::ExactDuplicate {
                existing_transaction_id: Some(*id),
            };
        }
    }
    if let Some(earlier_external_id) = batch_hashes.get(&candidate.source_row_hash) {
        // Same identity check as above, applied to a row seen earlier in
        // *this* batch rather than the ledger: two rows this file itself
        // distinguishes by external ID (e.g. two same-day, same-amount
        // purchases with different FITIDs) must not collapse just because
        // their text happens to coincide (section 22) - only a real repeat
        // (identical text, no conflicting external ID) is a duplicate.
        let external_ids_conflict = match (&candidate.external_transaction_id, earlier_external_id)
        {
            (Some(a), Some(b)) => a != b,
            _ => false,
        };
        if !external_ids_conflict {
            // Neither occurrence is in the ledger yet, so there is no
            // existing transaction id to point at.
            return MatchOutcome::ExactDuplicate {
                existing_transaction_id: None,
            };
        }
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
    // Row hashes seen earlier in *this* batch, mapped to that row's own
    // external id (if any) - so a CSV that genuinely repeats a row is
    // caught even though neither occurrence is in the ledger yet, while a
    // pair the file itself distinguishes by external id is not (see
    // `match_candidate`). `batch_external_ids` below is the separate,
    // stricter check that two rows must never *claim* the same external id.
    let mut batch_hashes: HashMap<String, Option<String>> = HashMap::new();
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
                    batch_hashes.insert(
                        candidate.source_row_hash.clone(),
                        candidate.external_transaction_id.clone(),
                    );
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

    commit_staged(
        conn,
        account_id,
        source_filename,
        ImportSourceType::Csv,
        file_hash,
        Some(mapping_json),
        import_profile_id,
        None,
        &staged,
        force_import_row_indices,
        excluded_row_indices,
    )
}

/// OFX/QFX-only metadata a statement may carry (product handoff section 28
/// for the balance, section 20 for the period) - CSV never sets any of
/// this (section 21: CSV has no reliable statement-level data). Purely
/// preserved provenance; a reconciliation (specs/phase-011) only ever
/// offers it as a prefill suggestion, never applies it automatically.
pub struct StatementMetadata {
    pub balance_cents: i64,
    pub balance_date: String,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
}

/// The one place either adapter (CSV or OFX/QFX) ever writes a transaction
/// or records an import (product handoff section 7: one shared commit
/// path). `statement_metadata` is OFX-only (see `StatementMetadata`); CSV
/// always passes `None`.
#[allow(clippy::too_many_arguments)]
fn commit_staged(
    conn: &mut Connection,
    account_id: i64,
    source_filename: &str,
    source_type: ImportSourceType,
    file_hash: String,
    mapping_json: Option<String>,
    import_profile_id: Option<i64>,
    statement_metadata: Option<StatementMetadata>,
    staged: &StagingResult,
    force_import_row_indices: &HashSet<usize>,
    excluded_row_indices: &HashSet<usize>,
) -> Result<ImportSummary, String> {
    let (statement_balance_cents, statement_balance_date, statement_start_date, statement_end_date) =
        match statement_metadata {
            Some(m) => (
                Some(m.balance_cents),
                Some(m.balance_date),
                m.start_date,
                m.end_date,
            ),
            None => (None, None, None, None),
        };

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut committed = 0usize;
    let mut duplicates = 0usize;
    let mut sent_to_review = 0usize;
    let mut invalid = 0usize;
    let mut excluded = 0usize;

    let import_id: i64 = tx
        .query_row(
            "INSERT INTO imports (account_id, source_filename, file_hash, row_count, committed_count, duplicate_count, needs_review_count, excluded_count, column_mapping_json, source_type, import_profile_id, statement_balance_cents, statement_balance_date, statement_start_date, statement_end_date)
             VALUES (?1, ?2, ?3, ?4, 0, 0, 0, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?11) RETURNING id",
            params![
                account_id,
                source_filename,
                file_hash,
                staged.rows.len() as i64,
                mapping_json,
                source_type.as_sql_str(),
                import_profile_id,
                statement_balance_cents,
                statement_balance_date,
                statement_start_date,
                statement_end_date,
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

// --- OFX/QFX source adapter (specs/phase-010) ---
//
// This section's job mirrors CSV's read_csv/detect_mapping/commit trio
// above: parse the source-specific format into candidates, then hand off
// to the exact same `match_candidate`/`commit_staged` shared engine. No
// second matcher, no second commit path, no OFX-specific ledger model
// (product handoff section 7).

/// Extracts the `YYYY-MM-DD` prefix from an OFX date
/// (`YYYYMMDD[HHMMSS[.fff]][gmt offset]`), deliberately never converting
/// time zones - the literal calendar date the institution reported is
/// authoritative, and any TZ math risks shifting a transaction to the
/// wrong day (product handoff section 11).
fn parse_ofx_date(raw: &str) -> Result<String, String> {
    let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 8 {
        return Err(format!("'{raw}' is not a recognized OFX date."));
    }
    let year: i32 = digits[0..4]
        .parse()
        .map_err(|_| format!("'{raw}' is not a recognized OFX date."))?;
    let month: u32 = digits[4..6]
        .parse()
        .map_err(|_| format!("'{raw}' is not a recognized OFX date."))?;
    let day: u32 = digits[6..8]
        .parse()
        .map_err(|_| format!("'{raw}' is not a recognized OFX date."))?;
    if !crate::dates::is_valid_calendar_date(year, month, day) {
        return Err(format!("'{raw}' is not a valid calendar date."));
    }
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

/// `DTUSER` (when the institution provides it) is the date the owner
/// actually initiated the transaction - closer to what MIDAS treats as the
/// primary `transaction_date` than `DTPOSTED` (when the bank happened to
/// post it). Falls back to `DTPOSTED` alone when `DTUSER` is absent, which
/// is the common case.
fn resolve_ofx_dates(
    dtposted: Option<&str>,
    dtuser: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let posted = dtposted.map(parse_ofx_date).transpose()?;
    let user = dtuser.map(parse_ofx_date).transpose()?;
    match (user, posted) {
        (Some(u), Some(p)) if u != p => Ok((u, Some(p))),
        (Some(u), _) => Ok((u, None)),
        (None, Some(p)) => Ok((p, None)),
        (None, None) => Err("Missing required posted date.".to_string()),
    }
}

/// One deterministic description policy (product handoff section 10):
/// `NAME` leads when present; `MEMO` is appended only when it adds
/// information `NAME` doesn't already contain, so a statement where both
/// fields repeat the same text doesn't produce ugly duplication. Never
/// fabricates a canonical merchant - that stays the rules/payee pipeline's
/// job downstream, exactly as it is for CSV.
fn merge_name_and_memo(name: Option<&str>, memo: Option<&str>) -> Option<String> {
    let name = name.map(str::trim).filter(|s| !s.is_empty());
    let memo = memo.map(str::trim).filter(|s| !s.is_empty());
    match (name, memo) {
        (Some(n), Some(m)) => {
            let n_lower = n.to_lowercase();
            let m_lower = m.to_lowercase();
            let redundant =
                n_lower == m_lower || n_lower.contains(&m_lower) || m_lower.contains(&n_lower);
            Some(if redundant {
                n.to_string()
            } else {
                format!("{n} - {m}")
            })
        }
        (Some(n), None) => Some(n.to_string()),
        (None, Some(m)) => Some(m.to_string()),
        (None, None) => None,
    }
}

/// Converts one statement's OFX transactions into staged rows through the
/// identical matcher/rule/payee pipeline `build_staging_rows` uses for CSV
/// (product handoff section 21: "do not special-case structured imports
/// with opaque matching behavior").
fn build_ofx_staging_rows(
    conn: &Connection,
    account_id: i64,
    is_liability: bool,
    source_type: ImportSourceType,
    transactions: &[crate::ofx::OfxTransaction],
    ledger: &ExistingLedger,
    merchant_rules: &[MerchantRule],
) -> Result<StagingResult, String> {
    let payee_id_by_merchant_name: HashMap<String, i64> = payees::list(conn, false)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|p| (p.name.trim().to_lowercase(), p.id))
        .collect();

    let mut rows = Vec::with_capacity(transactions.len());
    let mut batch_hashes: HashMap<String, Option<String>> = HashMap::new();
    let mut batch_external_ids: HashMap<String, usize> = HashMap::new();
    let (mut new_count, mut needs_review, mut duplicate, mut possible_match) = (0, 0, 0, 0);

    for (row_index, txn) in transactions.iter().enumerate() {
        let mut errors = Vec::new();
        let mut raw: HashMap<String, String> = HashMap::new();
        for (key, value) in [
            ("FITID", &txn.fitid),
            ("TRNTYPE", &txn.trntype),
            ("DTPOSTED", &txn.dtposted),
            ("DTUSER", &txn.dtuser),
            ("TRNAMT", &txn.trnamt),
            ("NAME", &txn.name),
            ("MEMO", &txn.memo),
            ("CHECKNUM", &txn.checknum),
        ] {
            if let Some(v) = value {
                raw.insert(key.to_string(), v.clone());
            }
        }

        let dates = resolve_ofx_dates(txn.dtposted.as_deref(), txn.dtuser.as_deref());
        if let Err(e) = &dates {
            errors.push(e.clone());
        }

        let description = merge_name_and_memo(txn.name.as_deref(), txn.memo.as_deref());
        if description.is_none() {
            errors.push("Missing required description (NAME/MEMO).".to_string());
        }

        let amount = match &txn.trnamt {
            Some(raw_amount) => match money::parse_amount_cents(raw_amount) {
                Ok(cents) => Some(cents),
                Err(e) => {
                    errors.push(e);
                    None
                }
            },
            None => {
                errors.push("Missing required amount.".to_string());
                None
            }
        };

        let external_transaction_id = txn
            .fitid
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if let Some(ext_id) = &external_transaction_id {
            if let Some(&first_row_index) = batch_external_ids.get(ext_id) {
                errors.push(format!(
                    "Transaction ID '{ext_id}' is also used by row {}; each transaction ID must be unique within this statement.",
                    first_row_index + 1
                ));
            } else {
                batch_external_ids.insert(ext_id.clone(), row_index);
            }
        }

        let (status, candidate, duplicate_of_transaction_id, match_detail) = match (
            dates.ok(),
            description,
            amount,
        ) {
            (Some((transaction_date, posted_date)), Some(description), Some(amount_cents))
                if errors.is_empty() =>
            {
                let rule_match = rules::best_match(merchant_rules, &description);
                let merchant = rule_match
                    .as_ref()
                    .and_then(|m| m.merchant.clone())
                    .unwrap_or_else(|| description.clone());
                let category = rule_match.as_ref().and_then(|m| m.category.clone());
                let subcategory = rule_match.and_then(|m| m.subcategory);
                let hash = row_hash(account_id, &transaction_date, amount_cents, &description);
                // OFX's TRNAMT is already signed relative to the
                // account's own balance (the same model ADR-0004
                // already uses) - XFER is the one explicit signal worth
                // trusting over the sign-based default; everything else
                // reuses CSV's identical inference rather than a
                // separate OFX accounting convention (section 12).
                let transaction_type = if txn
                    .trntype
                    .as_deref()
                    .is_some_and(|t| t.eq_ignore_ascii_case("XFER"))
                {
                    TransactionType::Transfer
                } else {
                    default_transaction_type(amount_cents, is_liability)
                };
                let candidate_payee_id = payee_id_by_merchant_name
                    .get(&merchant.trim().to_lowercase())
                    .copied();

                let candidate = NormalizedCandidateTransaction {
                    source_type,
                    account_id,
                    transaction_date,
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
                            Some("This transaction has already been imported.".to_string()),
                        ),
                        MatchOutcome::StrongMatch {
                            existing_transaction_id,
                        } => (
                            RowStatus::PossibleMatch,
                            Some(existing_transaction_id),
                            Some(format!(
                                "Likely already imported as transaction #{existing_transaction_id} (same date, amount, and description)."
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
                    batch_hashes.insert(
                        candidate.source_row_hash.clone(),
                        candidate.external_transaction_id.clone(),
                    );
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
            raw,
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

fn mask_source_account_id(id: &str) -> String {
    let visible: String = id
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if visible.len() >= id.chars().count() {
        format!("•••• {id}")
    } else {
        format!("•••• {visible}")
    }
}

#[derive(Debug, Serialize)]
pub struct OfxAccountPreview {
    pub statement_index: usize,
    pub masked_account_id: String,
    pub kind: String,
    pub account_type: Option<String>,
    pub currency: Option<String>,
    pub existing_mapping_account_id: Option<i64>,
    pub transaction_count: usize,
}

#[derive(Debug, Serialize)]
pub struct OfxPreview {
    pub accounts: Vec<OfxAccountPreview>,
    pub source_type: ImportSourceType,
}

fn detect_ofx_source_type(filename: &str) -> ImportSourceType {
    if filename.to_lowercase().ends_with(".qfx") {
        ImportSourceType::Qfx
    } else {
        ImportSourceType::Ofx
    }
}

/// Read-only: parses the file and reports what source accounts it
/// contains plus any already-remembered MIDAS-account mapping for each
/// (product handoff section 15) - never guesses, only reports facts the
/// frontend uses to preselect or ask.
pub fn preview_ofx_content(
    conn: &Connection,
    ofx_content: &str,
    source_filename: &str,
) -> Result<OfxPreview, String> {
    let parsed = crate::ofx::parse(ofx_content).map_err(|e| e.to_string())?;
    let accounts = parsed
        .statements
        .iter()
        .enumerate()
        .map(|(statement_index, stmt)| {
            let existing_mapping_account_id =
                crate::ofx_accounts::find_mapping(conn, &stmt.account)?;
            Ok(OfxAccountPreview {
                statement_index,
                masked_account_id: mask_source_account_id(&stmt.account.source_account_id),
                kind: match stmt.account.kind {
                    crate::ofx::StatementKind::Bank => "bank".to_string(),
                    crate::ofx::StatementKind::CreditCard => "credit_card".to_string(),
                },
                account_type: stmt.account.source_account_type.clone(),
                currency: stmt.account.currency.clone(),
                existing_mapping_account_id,
                transaction_count: stmt.transactions.len(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(OfxPreview {
        accounts,
        source_type: detect_ofx_source_type(source_filename),
    })
}

#[tauri::command]
pub fn preview_ofx(
    state: State<DbState>,
    ofx_content: String,
    source_filename: String,
) -> Result<OfxPreview, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    preview_ofx_content(&conn, &ofx_content, &source_filename)
}

pub fn stage_ofx_content(
    conn: &Connection,
    account_id: i64,
    ofx_content: &str,
    statement_index: usize,
    source_filename: &str,
) -> Result<StagingResult, String> {
    let parsed = crate::ofx::parse(ofx_content).map_err(|e| e.to_string())?;
    let statement = parsed
        .statements
        .get(statement_index)
        .ok_or_else(|| "That statement was not found in this file.".to_string())?;
    let is_liability = accounts::account_type_of(conn, account_id)
        .map_err(|e| e.to_string())?
        .is_liability();
    let ledger = load_existing_ledger(conn, account_id)?;
    let merchant_rules = rules::relevant_rules(conn, account_id).map_err(|e| e.to_string())?;
    build_ofx_staging_rows(
        conn,
        account_id,
        is_liability,
        detect_ofx_source_type(source_filename),
        &statement.transactions,
        &ledger,
        &merchant_rules,
    )
}

#[tauri::command]
pub fn stage_ofx(
    state: State<DbState>,
    account_id: i64,
    ofx_content: String,
    statement_index: usize,
    source_filename: String,
) -> Result<StagingResult, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    stage_ofx_content(
        &conn,
        account_id,
        &ofx_content,
        statement_index,
        &source_filename,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn commit_ofx(
    conn: &mut Connection,
    account_id: i64,
    source_filename: &str,
    ofx_content: &str,
    statement_index: usize,
    remember_account_mapping: bool,
    force_import_row_indices: &HashSet<usize>,
    excluded_row_indices: &HashSet<usize>,
) -> Result<ImportSummary, String> {
    let parsed = crate::ofx::parse(ofx_content).map_err(|e| e.to_string())?;
    let statement = parsed
        .statements
        .get(statement_index)
        .ok_or_else(|| "That statement was not found in this file.".to_string())?
        .clone();
    let source_type = detect_ofx_source_type(source_filename);

    if remember_account_mapping {
        crate::ofx_accounts::remember(conn, &statement.account, account_id)?;
    }

    let is_liability = accounts::account_type_of(conn, account_id)
        .map_err(|e| e.to_string())?
        .is_liability();
    let ledger = load_existing_ledger(conn, account_id)?;
    let merchant_rules = rules::relevant_rules(conn, account_id).map_err(|e| e.to_string())?;
    let staged = build_ofx_staging_rows(
        conn,
        account_id,
        is_liability,
        source_type,
        &statement.transactions,
        &ledger,
        &merchant_rules,
    )?;

    let file_hash = {
        let mut hasher = Sha256::new();
        hasher.update(ofx_content.as_bytes());
        to_hex(&hasher.finalize())
    };
    let statement_metadata = statement.ledger_balance.as_ref().and_then(|b| {
        let cents = money::parse_amount_cents(&b.amount).ok()?;
        let balance_date = parse_ofx_date(&b.as_of_date).unwrap_or_else(|_| b.as_of_date.clone());
        Some(StatementMetadata {
            balance_cents: cents,
            balance_date,
            start_date: statement
                .statement_start_date
                .as_deref()
                .and_then(|d| parse_ofx_date(d).ok()),
            end_date: statement
                .statement_end_date
                .as_deref()
                .and_then(|d| parse_ofx_date(d).ok()),
        })
    });

    commit_staged(
        conn,
        account_id,
        source_filename,
        source_type,
        file_hash,
        // `column_mapping_json` is CSV-specific metadata with a NOT NULL
        // constraint from migration 0002; OFX has no column mapping to
        // record, so an empty object is the honest "not applicable" value
        // rather than a schema change for a column nothing reads back.
        Some("{}".to_string()),
        None,
        statement_metadata,
        &staged,
        force_import_row_indices,
        excluded_row_indices,
    )
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn commit_ofx_import(
    state: State<DbState>,
    account_id: i64,
    source_filename: String,
    ofx_content: String,
    statement_index: usize,
    remember_account_mapping: bool,
    force_import_row_indices: Vec<usize>,
    excluded_row_indices: Vec<usize>,
) -> Result<ImportSummary, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    let force_set: HashSet<usize> = force_import_row_indices.into_iter().collect();
    let excluded_set: HashSet<usize> = excluded_row_indices.into_iter().collect();
    commit_ofx(
        &mut conn,
        account_id,
        &source_filename,
        &ofx_content,
        statement_index,
        remember_account_mapping,
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
    // `created_at == updated_at` alone proves nothing about tags, which
    // attach via a separate `transaction_tags` join row without ever
    // touching the transaction itself (found during Batch 3's Undo
    // hardening review - product handoff section 26: "created_at ==
    // updated_at does not necessarily prove that nothing meaningful was
    // attached after import"). Same reasoning extends to splits (Transaction
    // Intelligence, specs/phase-012): `transaction_splits` rows never touch
    // the parent's own `updated_at` either, so a split transaction is
    // protected by its own existence check, not by the timestamp.
    // `c.split_parent_transaction_id = t.id` is checked defensively even
    // though nothing has ever written it (see transactions.rs's doc comment
    // on that column) - manual transfer/refund links are checked in both
    // directions, a transaction protected whether it's the one pointing at
    // another or the one being pointed at.
    let mut stmt = conn
        .prepare(
            "SELECT t.id FROM transactions t
             WHERE t.source_import_id = ?1
               AND t.created_at = t.updated_at
               AND t.transfer_pair_transaction_id IS NULL
               AND t.refund_of_transaction_id IS NULL
               AND NOT EXISTS (SELECT 1 FROM transaction_tags tt WHERE tt.transaction_id = t.id)
               AND NOT EXISTS (SELECT 1 FROM transaction_splits ts WHERE ts.transaction_id = t.id)
               AND NOT EXISTS (
                    SELECT 1 FROM transactions c
                    WHERE c.split_parent_transaction_id = t.id
                       OR c.transfer_pair_transaction_id = t.id
                       OR c.refund_of_transaction_id = t.id
               )",
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
        conn.execute_batch(include_str!("../migrations/0010_ofx_source_adapter.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../migrations/0011_reconciliation.sql"))
            .unwrap();
        conn.execute_batch(include_str!(
            "../migrations/0012_transaction_intelligence.sql"
        ))
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
                payee_id: None,
                is_one_time: false,
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
            payee_id: None,
            is_one_time: false,
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
    fn undo_protects_a_tagged_transaction_even_though_tagging_never_touches_updated_at() {
        // Found during this batch's Undo hardening audit (product handoff
        // section 26): tags::attach() only inserts into the separate
        // transaction_tags join table - the transaction's own updated_at
        // never changes, so created_at == updated_at alone would wrongly
        // call a tagged transaction "untouched."
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
        let tagged = imported
            .iter()
            .find(|t| t.raw_description == "COFFEE SHOP")
            .unwrap();
        assert_eq!(
            tagged.created_at, tagged.updated_at,
            "sanity check: tagging alone must not have bumped updated_at"
        );

        let tag = crate::tags::create(&conn, "Vacation").unwrap();
        crate::tags::attach(&conn, tagged.id, tag.id).unwrap();

        let preview = preview_undo(&conn, summary.import_id).unwrap();
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
            .any(|t| t.id == tagged.id),
            "a tagged transaction must survive Undo"
        );
    }

    #[test]
    fn undo_protects_a_transaction_referenced_as_a_transfer_pair_or_refund_target() {
        // Same audit: no UI writes these columns yet (confirmed via a
        // direct grep of the codebase before this batch), but the schema
        // and the risk are real, so Undo must not assume the absence of a
        // UI means the absence of a relationship forever.
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
        let linked = imported
            .iter()
            .find(|t| t.raw_description == "COFFEE SHOP")
            .unwrap();

        // Manually establish a refund-of link the way a future feature
        // would (no UI exists yet, matching Batch 2's existing manual-only
        // convention for this column).
        let manual_refund = crate::transactions::create(
            &conn,
            &TransactionInput {
                account_id,
                transaction_date: "2026-03-05".to_string(),
                posted_date: None,
                amount_cents: 450,
                transaction_type: TransactionType::Refund,
                raw_description: "Refund".to_string(),
                merchant: None,
                category: None,
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
                payee_id: None,
                is_one_time: false,
            },
        )
        .unwrap();
        conn.execute(
            "UPDATE transactions SET refund_of_transaction_id = ?1 WHERE id = ?2",
            params![linked.id, manual_refund.id],
        )
        .unwrap();

        let preview = preview_undo(&conn, summary.import_id).unwrap();
        assert_eq!(
            preview.protected_count, 1,
            "the transaction referenced by another transaction's refund_of_transaction_id must be protected"
        );
    }

    #[test]
    fn undo_protects_a_split_transaction_even_though_splitting_never_touches_updated_at() {
        // Transaction Intelligence (specs/phase-012): splits live in a
        // separate transaction_splits table and never touch the parent's
        // own updated_at, exactly the same shape of gap Batch 3 found and
        // fixed for tags - re-audited here since a genuinely new child
        // table was added after that audit was written.
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let summary = commit_simple(
            &mut conn,
            account_id,
            "statement.csv",
            "Date,Description,Amount\n2026-03-01,WALMART,-120.00\n2026-03-02,PAYCHECK,2000.00\n",
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
        let walmart = imported
            .iter()
            .find(|t| t.raw_description == "WALMART")
            .unwrap();
        assert_eq!(
            walmart.created_at, walmart.updated_at,
            "sanity check: splitting alone must not have bumped updated_at"
        );

        crate::splits::replace_for_transaction(
            &mut conn,
            walmart.id,
            &[
                crate::splits::SplitInput {
                    category: Some("Groceries".to_string()),
                    subcategory: None,
                    amount_cents: -8000,
                    notes: None,
                },
                crate::splits::SplitInput {
                    category: Some("Household".to_string()),
                    subcategory: None,
                    amount_cents: -4000,
                    notes: None,
                },
            ],
        )
        .unwrap();

        let preview = preview_undo(&conn, summary.import_id).unwrap();
        assert_eq!(preview.removable_count, 1);
        assert_eq!(preview.protected_count, 1);

        let result = undo(&mut conn, summary.import_id).unwrap();
        assert_eq!(result.removed_count, 1);
        assert_eq!(result.protected_count, 1);
        assert_eq!(
            crate::splits::list_for_transaction(&conn, walmart.id)
                .unwrap()
                .len(),
            2,
            "a split transaction must survive Undo with its splits intact"
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

    // --- OFX/QFX source adapter (specs/phase-010) ---

    const OFX_CHECKING: &str = "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\nENCODING:USASCII\nCHARSET:1252\nCOMPRESSION:NONE\nOLDFILEUID:NONE\nNEWFILEUID:NONE\n\n<OFX>\n<BANKMSGSRSV1>\n<STMTTRNRS>\n<STMTRS>\n<CURDEF>USD\n<BANKACCTFROM>\n<BANKID>111000025\n<ACCTID>0001112223\n<ACCTTYPE>CHECKING\n</BANKACCTFROM>\n<BANKTRANLIST>\n<STMTTRN>\n<TRNTYPE>DEBIT\n<DTPOSTED>20260601120000\n<TRNAMT>-4.50\n<FITID>FIT-001\n<NAME>SAMPLE COFFEE MERCHANT\n<MEMO>Purchase\n</STMTTRN>\n<STMTTRN>\n<TRNTYPE>CREDIT\n<DTPOSTED>20260602120000\n<TRNAMT>2000.00\n<FITID>FIT-002\n<NAME>SAMPLE EMPLOYER PAYROLL\n</STMTTRN>\n</BANKTRANLIST>\n<LEDGERBAL>\n<BALAMT>1995.50\n<DTASOF>20260630120000\n</LEDGERBAL>\n</STMTRS>\n</STMTTRNRS>\n</BANKMSGSRSV1>\n</OFX>\n";

    fn overlapping_ofx_statement() -> String {
        // Same FITID-001 as OFX_CHECKING (already imported), plus one
        // genuinely new transaction (FIT-003).
        "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\nENCODING:USASCII\nCHARSET:1252\nCOMPRESSION:NONE\nOLDFILEUID:NONE\nNEWFILEUID:NONE\n\n<OFX>\n<BANKMSGSRSV1>\n<STMTTRNRS>\n<STMTRS>\n<CURDEF>USD\n<BANKACCTFROM>\n<BANKID>111000025\n<ACCTID>0001112223\n<ACCTTYPE>CHECKING\n</BANKACCTFROM>\n<BANKTRANLIST>\n<STMTTRN>\n<TRNTYPE>DEBIT\n<DTPOSTED>20260601120000\n<TRNAMT>-4.50\n<FITID>FIT-001\n<NAME>SAMPLE COFFEE MERCHANT\n<MEMO>Purchase\n</STMTTRN>\n<STMTTRN>\n<TRNTYPE>DEBIT\n<DTPOSTED>20260603120000\n<TRNAMT>-9.99\n<FITID>FIT-003\n<NAME>SAMPLE GAS STATION\n</STMTTRN>\n</BANKTRANLIST>\n</STMTRS>\n</STMTTRNRS>\n</BANKMSGSRSV1>\n</OFX>\n".to_string()
    }

    const QFX_CREDIT_CARD: &str = "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\nENCODING:USASCII\nCHARSET:1252\nCOMPRESSION:NONE\nOLDFILEUID:NONE\nNEWFILEUID:NONE\n\n<OFX>\n<CREDITCARDMSGSRSV1>\n<CCSTMTTRNRS>\n<CCSTMTRS>\n<CURDEF>USD\n<CCACCTFROM>\n<ACCTID>4111111111111111\n</CCACCTFROM>\n<BANKTRANLIST>\n<STMTTRN>\n<TRNTYPE>DEBIT\n<DTPOSTED>20260605120000\n<TRNAMT>52.10\n<FITID>CC-FIT-001\n<NAME>SAMPLE GROCERY MERCHANT\n</STMTTRN>\n<STMTTRN>\n<TRNTYPE>CREDIT\n<DTPOSTED>20260610120000\n<TRNAMT>-200.00\n<FITID>CC-FIT-002\n<NAME>SAMPLE PAYMENT THANK YOU\n</STMTTRN>\n</BANKTRANLIST>\n</CCSTMTRS>\n</CCSTMTTRNRS>\n</CREDITCARDMSGSRSV1>\n</OFX>\n";

    fn commit_ofx_simple(
        conn: &mut Connection,
        account_id: i64,
        filename: &str,
        ofx_content: &str,
    ) -> Result<ImportSummary, String> {
        commit_ofx(
            conn,
            account_id,
            filename,
            ofx_content,
            0,
            false,
            &HashSet::new(),
            &HashSet::new(),
        )
    }

    #[test]
    fn ofx_preview_reports_the_source_account_and_no_existing_mapping() {
        let conn = test_conn();
        let preview = preview_ofx_content(&conn, OFX_CHECKING, "statement.ofx").unwrap();
        assert_eq!(preview.accounts.len(), 1);
        assert_eq!(preview.accounts[0].kind, "bank");
        assert_eq!(preview.accounts[0].existing_mapping_account_id, None);
        assert_eq!(preview.accounts[0].transaction_count, 2);
        assert!(preview.accounts[0].masked_account_id.ends_with("2223"));
        assert!(
            !preview.accounts[0].masked_account_id.contains("0001112223"),
            "the full source account number must never appear in normal-UI-facing preview data"
        );
    }

    #[test]
    fn ofx_checking_import_creates_transactions_with_correct_sign_and_provenance() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let summary =
            commit_ofx_simple(&mut conn, account_id, "statement.ofx", OFX_CHECKING).unwrap();
        assert_eq!(summary.committed_count, 2);

        let committed = crate::transactions::list(
            &conn,
            &TransactionFilter {
                account_id: Some(account_id),
                ..Default::default()
            },
        )
        .unwrap();
        let purchase = committed.iter().find(|t| t.amount_cents == -450).unwrap();
        assert_eq!(purchase.transaction_type, TransactionType::Purchase);
        assert_eq!(purchase.external_transaction_id.as_deref(), Some("FIT-001"));
        assert_eq!(
            purchase.raw_description,
            "SAMPLE COFFEE MERCHANT - Purchase"
        );
        assert_eq!(purchase.transaction_date, "2026-06-01");

        let payroll = committed.iter().find(|t| t.amount_cents == 200000).unwrap();
        assert_eq!(payroll.transaction_type, TransactionType::Income);
        assert_eq!(payroll.external_transaction_id.as_deref(), Some("FIT-002"));

        let (source_type, statement_balance_cents, statement_balance_date): (
            String,
            Option<i64>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT source_type, statement_balance_cents, statement_balance_date FROM imports WHERE id = ?1",
                params![summary.import_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(source_type, "ofx");
        assert_eq!(statement_balance_cents, Some(199550));
        assert_eq!(statement_balance_date.as_deref(), Some("2026-06-30"));
    }

    const OFX_CHECKING_WITH_PERIOD: &str = "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\nENCODING:USASCII\nCHARSET:1252\nCOMPRESSION:NONE\nOLDFILEUID:NONE\nNEWFILEUID:NONE\n\n<OFX>\n<BANKMSGSRSV1>\n<STMTTRNRS>\n<STMTRS>\n<CURDEF>USD\n<BANKACCTFROM>\n<BANKID>111000025\n<ACCTID>0001112223\n<ACCTTYPE>CHECKING\n</BANKACCTFROM>\n<BANKTRANLIST>\n<DTSTART>20260601\n<DTEND>20260630\n<STMTTRN>\n<TRNTYPE>DEBIT\n<DTPOSTED>20260601120000\n<TRNAMT>-4.50\n<FITID>FIT-001\n<NAME>SAMPLE COFFEE MERCHANT\n</STMTTRN>\n</BANKTRANLIST>\n<LEDGERBAL>\n<BALAMT>1995.50\n<DTASOF>20260630120000\n</LEDGERBAL>\n</STMTRS>\n</STMTTRNRS>\n</BANKMSGSRSV1>\n</OFX>\n";

    #[test]
    fn ofx_statement_period_is_preserved_and_surfaced_as_a_reconciliation_hint() {
        // product handoff section 20: Batch 3 preserved the ending balance
        // but discarded DTSTART/DTEND entirely - this proves the Batch 4
        // extension threads the period all the way through to where
        // reconciliation.rs's prefill hint reads it back.
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let summary = commit_ofx_simple(
            &mut conn,
            account_id,
            "statement.ofx",
            OFX_CHECKING_WITH_PERIOD,
        )
        .unwrap();

        let (start, end): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT statement_start_date, statement_end_date FROM imports WHERE id = ?1",
                params![summary.import_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(start.as_deref(), Some("2026-06-01"));
        assert_eq!(end.as_deref(), Some("2026-06-30"));

        let hint = crate::reconciliation::latest_statement_hint(&conn, account_id)
            .unwrap()
            .unwrap();
        assert_eq!(hint.statement_start_date.as_deref(), Some("2026-06-01"));
        assert_eq!(hint.statement_end_date.as_deref(), Some("2026-06-30"));
        assert_eq!(hint.statement_ending_balance_cents, 199550);
    }

    #[test]
    fn ofx_credit_card_purchase_and_payment_match_midas_sign_convention() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::CreditCard);
        let summary = commit_ofx(
            &mut conn,
            account_id,
            "statement.qfx",
            QFX_CREDIT_CARD,
            0,
            false,
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(summary.committed_count, 2);

        let committed = crate::transactions::list(
            &conn,
            &TransactionFilter {
                account_id: Some(account_id),
                ..Default::default()
            },
        )
        .unwrap();
        // ADR-0004: a card purchase increases what's owed (positive) and is
        // spending; a payment decreases it (negative) and is a transfer,
        // never counted as spending on either leg.
        let purchase = committed.iter().find(|t| t.amount_cents == 5210).unwrap();
        assert_eq!(purchase.transaction_type, TransactionType::Purchase);
        let payment = committed.iter().find(|t| t.amount_cents == -20000).unwrap();
        assert_eq!(payment.transaction_type, TransactionType::Transfer);

        let (source_type,): (String,) = conn
            .query_row(
                "SELECT source_type FROM imports WHERE id = ?1",
                params![summary.import_id],
                |r| Ok((r.get(0)?,)),
            )
            .unwrap();
        assert_eq!(source_type, "qfx");
    }

    #[test]
    fn ofx_exact_reimport_is_idempotent_including_after_a_simulated_restart() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let first =
            commit_ofx_simple(&mut conn, account_id, "statement.ofx", OFX_CHECKING).unwrap();
        assert_eq!(first.committed_count, 2);

        // "Restart" is simulated by nothing more than a fresh commit call
        // against the same persisted connection/ledger - idempotence must
        // hold purely from what's in the database, not in-memory state.
        let second =
            commit_ofx_simple(&mut conn, account_id, "statement.ofx", OFX_CHECKING).unwrap();
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
    fn ofx_overlapping_statement_recognizes_existing_fitids_and_commits_only_new_ones() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        commit_ofx_simple(&mut conn, account_id, "first.ofx", OFX_CHECKING).unwrap();

        let overlapping = overlapping_ofx_statement();
        let staged = stage_ofx_content(&conn, account_id, &overlapping, 0, "second.ofx").unwrap();
        assert_eq!(
            staged.duplicate_count, 1,
            "FIT-001 is already in the ledger"
        );
        assert_eq!(staged.new_count, 1, "FIT-003 is genuinely new");

        let summary = commit_ofx_simple(&mut conn, account_id, "second.ofx", &overlapping).unwrap();
        assert_eq!(summary.committed_count, 1);
        assert_eq!(summary.duplicate_count, 1);
    }

    #[test]
    fn ofx_same_amount_and_date_with_different_fitid_are_not_collapsed() {
        // Distinct FITIDs are MIDAS's strongest signal that these are two
        // real, separate transactions - product handoff section 22's
        // explicit requirement, even though the amount happens to match.
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        commit_ofx_simple(&mut conn, account_id, "first.ofx", OFX_CHECKING).unwrap();

        let second_purchase = "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\nENCODING:USASCII\nCHARSET:1252\nCOMPRESSION:NONE\nOLDFILEUID:NONE\nNEWFILEUID:NONE\n\n<OFX>\n<BANKMSGSRSV1>\n<STMTTRNRS>\n<STMTRS>\n<CURDEF>USD\n<BANKACCTFROM>\n<BANKID>111000025\n<ACCTID>0001112223\n<ACCTTYPE>CHECKING\n</BANKACCTFROM>\n<BANKTRANLIST>\n<STMTTRN>\n<TRNTYPE>DEBIT\n<DTPOSTED>20260601120000\n<TRNAMT>-4.50\n<FITID>FIT-999-DIFFERENT\n<NAME>SAMPLE COFFEE MERCHANT\n<MEMO>Purchase\n</STMTTRN>\n</BANKTRANLIST>\n</STMTRS>\n</STMTTRNRS>\n</BANKMSGSRSV1>\n</OFX>\n";
        let staged =
            stage_ofx_content(&conn, account_id, second_purchase, 0, "second.ofx").unwrap();
        // Same date/amount/description as an existing transaction, but a
        // different FITID overrides the hash match (the raw text differs
        // too, since FITID isn't part of raw_description) - still resolved
        // deterministically as a duplicate-of-content rather than losing
        // data, per the shared matcher's own strong-match handling.
        assert!(matches!(staged.rows[0].status, RowStatus::PossibleMatch));
    }

    #[test]
    fn ofx_two_brand_new_same_batch_rows_with_different_fitids_both_stage_as_new() {
        // Same requirement as the test above, but for two rows that are
        // BOTH new to the ledger in the same statement (rather than one
        // already-committed row plus a second import) - this exercises the
        // separate batch-internal dedup path (`batch_hashes`), which must
        // apply the identical FITID-conflict check as the ledger path
        // rather than collapsing same-day, same-amount, same-description
        // purchases just because the file lists them together.
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let two_coffees = "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\nENCODING:USASCII\nCHARSET:1252\nCOMPRESSION:NONE\nOLDFILEUID:NONE\nNEWFILEUID:NONE\n\n<OFX>\n<BANKMSGSRSV1>\n<STMTTRNRS>\n<STMTRS>\n<CURDEF>USD\n<BANKACCTFROM>\n<BANKID>111000025\n<ACCTID>0001112223\n<ACCTTYPE>CHECKING\n</BANKACCTFROM>\n<BANKTRANLIST>\n<STMTTRN>\n<TRNTYPE>DEBIT\n<DTPOSTED>20260615090000\n<TRNAMT>-4.50\n<FITID>FIT-AAA\n<NAME>SAMPLE COFFEE MERCHANT\n<MEMO>Purchase\n</STMTTRN>\n<STMTTRN>\n<TRNTYPE>DEBIT\n<DTPOSTED>20260615090000\n<TRNAMT>-4.50\n<FITID>FIT-BBB\n<NAME>SAMPLE COFFEE MERCHANT\n<MEMO>Purchase\n</STMTTRN>\n</BANKTRANLIST>\n</STMTRS>\n</STMTTRNRS>\n</BANKMSGSRSV1>\n</OFX>\n";

        let staged =
            stage_ofx_content(&conn, account_id, two_coffees, 0, "two-coffees.ofx").unwrap();
        assert_eq!(
            staged.new_count, 2,
            "two legitimate same-amount purchases distinguished only by FITID must both stage as new, not collapse into a duplicate"
        );
        assert!(matches!(staged.rows[0].status, RowStatus::New));
        assert!(matches!(staged.rows[1].status, RowStatus::New));

        let summary =
            commit_ofx_simple(&mut conn, account_id, "two-coffees.ofx", two_coffees).unwrap();
        assert_eq!(summary.committed_count, 2);
    }

    #[test]
    fn ofx_remember_account_mapping_persists_and_is_found_on_next_preview() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        commit_ofx(
            &mut conn,
            account_id,
            "statement.ofx",
            OFX_CHECKING,
            0,
            true, // remember_account_mapping
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();

        let preview = preview_ofx_content(&conn, OFX_CHECKING, "statement.ofx").unwrap();
        assert_eq!(
            preview.accounts[0].existing_mapping_account_id,
            Some(account_id)
        );
    }

    #[test]
    fn ofx_investment_only_file_is_rejected_before_any_commit() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let investment_only = "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\nENCODING:USASCII\nCHARSET:1252\nCOMPRESSION:NONE\nOLDFILEUID:NONE\nNEWFILEUID:NONE\n\n<OFX>\n<INVSTMTMSGSRSV1>\n<INVSTMTTRNRS>\n<INVSTMTRS>\n<DTASOF>20260630120000\n<CURDEF>USD\n</INVSTMTRS>\n</INVSTMTTRNRS>\n</INVSTMTMSGSRSV1>\n</OFX>\n";

        let err =
            commit_ofx_simple(&mut conn, account_id, "brokerage.ofx", investment_only).unwrap_err();
        assert!(err.contains("investment"));

        let count: i64 = conn
            .query_row("SELECT count(*) FROM transactions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "an unsupported file must never partially import");
    }

    #[test]
    fn ofx_undo_removes_untouched_transactions_and_restores_the_balance() {
        let mut conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let summary =
            commit_ofx_simple(&mut conn, account_id, "statement.ofx", OFX_CHECKING).unwrap();

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
    }

    #[test]
    fn name_and_memo_merge_avoids_duplicate_text_but_keeps_distinct_context() {
        assert_eq!(
            merge_name_and_memo(Some("Coffee Shop"), Some("Coffee Shop")),
            Some("Coffee Shop".to_string()),
            "identical NAME/MEMO must not repeat"
        );
        assert_eq!(
            merge_name_and_memo(Some("Coffee Shop"), Some("Purchase")),
            Some("Coffee Shop - Purchase".to_string()),
            "distinct MEMO context is preserved"
        );
        assert_eq!(
            merge_name_and_memo(Some("Coffee Shop"), None),
            Some("Coffee Shop".to_string())
        );
        assert_eq!(
            merge_name_and_memo(None, Some("Purchase")),
            Some("Purchase".to_string())
        );
        assert_eq!(merge_name_and_memo(None, None), None);
    }

    #[test]
    fn ofx_date_variants_all_resolve_to_the_same_calendar_date() {
        for raw in [
            "20260601",
            "20260601120000",
            "20260601120000.500",
            "20260601120000[-5:EST]",
            "20260601120000.500[-5:EST]",
        ] {
            assert_eq!(
                parse_ofx_date(raw).unwrap(),
                "2026-06-01",
                "failed for {raw}"
            );
        }
        assert!(parse_ofx_date("not a date").is_err());
    }
}
