use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::DbState;
use crate::money;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionType {
    Purchase,
    Income,
    Transfer,
    Refund,
    /// A structural anchor at the account's true historical starting point -
    /// see ADR-0007/ADR-0009. Established once by
    /// `accounting::reconcile_balance` and never moved or rewritten by a
    /// later reconciliation (ADR-0009's correction). Never user-selectable
    /// in the manual transaction form and never spending/income (see
    /// `accounting::spending_effect`).
    OpeningBalance,
    /// A dated correction for drift discovered after the opening balance was
    /// already established - see ADR-0009. Unlike `OpeningBalance`, MIDAS
    /// may hold several of these over an account's life (at most one per
    /// reconciliation date), each landing on its own `transaction_date`
    /// rather than backdated - so an adjustment made today never changes
    /// what net worth or balance looked like before today. Never
    /// user-selectable in the manual transaction form and never
    /// spending/income/recurring activity.
    BalanceAdjustment,
}

/// The owner's own statement-reconciliation lifecycle (Foundation Batch 1 -
/// specs/phase-008). Distinct from `is_pending`, which is a bank-reported
/// temporary-hold state independent of whether the owner has reconciled
/// against a statement. Not settable via `TransactionInput` - like
/// `is_archived`, it has its own narrow setter
/// (`set_transaction_reconciliation_status`) rather than living in the
/// general create/update path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus {
    Unreconciled,
    Cleared,
    Reconciled,
}

impl ReconciliationStatus {
    fn as_sql_str(self) -> &'static str {
        match self {
            ReconciliationStatus::Unreconciled => "unreconciled",
            ReconciliationStatus::Cleared => "cleared",
            ReconciliationStatus::Reconciled => "reconciled",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "unreconciled" => ReconciliationStatus::Unreconciled,
            "cleared" => ReconciliationStatus::Cleared,
            "reconciled" => ReconciliationStatus::Reconciled,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown reconciliation_status '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }
}

impl TransactionType {
    pub(crate) fn as_sql_str(self) -> &'static str {
        match self {
            TransactionType::Purchase => "purchase",
            TransactionType::Income => "income",
            TransactionType::Transfer => "transfer",
            TransactionType::Refund => "refund",
            TransactionType::OpeningBalance => "opening_balance",
            TransactionType::BalanceAdjustment => "balance_adjustment",
        }
    }

    pub fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "purchase" => TransactionType::Purchase,
            "income" => TransactionType::Income,
            "transfer" => TransactionType::Transfer,
            "refund" => TransactionType::Refund,
            "opening_balance" => TransactionType::OpeningBalance,
            "balance_adjustment" => TransactionType::BalanceAdjustment,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown transaction_type '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Transaction {
    pub id: i64,
    pub account_id: i64,
    pub transaction_date: String,
    pub posted_date: Option<String>,
    pub amount_cents: i64,
    pub transaction_type: TransactionType,
    pub raw_description: String,
    pub merchant: String,
    pub category: Option<String>,
    pub subcategory: Option<String>,
    pub notes: Option<String>,
    pub tags: Option<String>,
    pub is_pending: Option<bool>,
    pub is_archived: bool,
    pub needs_review: bool,
    pub source_import_id: Option<i64>,
    pub source_row_hash: Option<String>,
    pub transfer_pair_transaction_id: Option<i64>,
    pub refund_of_transaction_id: Option<i64>,
    /// Foundation Batch 1 (specs/phase-008) - not yet populated by any
    /// feature. `merchant` above remains the authoritative display value
    /// until a later batch implements Payee Library normalization/linking.
    pub payee_id: Option<i64>,
    /// Foundation Batch 1 - not yet populated. A future split-transaction
    /// feature will point each child row at the original it was split from;
    /// the original is expected to be archived at that point (already
    /// excluded from every balance/spending query), so a populated parent
    /// is never double-counted once that feature lands.
    pub split_parent_transaction_id: Option<i64>,
    /// Foundation Batch 1 - not yet populated. The source's own unique
    /// transaction ID (e.g. OFX `<TRNUID>`), for a future importer that has
    /// a stronger dedup signal than `source_row_hash`.
    pub external_transaction_id: Option<String>,
    pub reconciliation_status: ReconciliationStatus,
    /// Foundation Batch 1 - not yet settable from the UI. An analytical
    /// flag a future feature can use to exclude a one-off purchase from a
    /// budget-suggestion or recurring-detection baseline.
    pub is_one_time: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct TransactionInput {
    pub account_id: i64,
    pub transaction_date: String,
    pub posted_date: Option<String>,
    pub amount_cents: i64,
    pub transaction_type: TransactionType,
    pub raw_description: String,
    pub merchant: Option<String>,
    pub category: Option<String>,
    pub subcategory: Option<String>,
    pub notes: Option<String>,
    pub tags: Option<String>,
    pub is_pending: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct TransactionFilter {
    pub account_id: Option<i64>,
    pub search: Option<String>,
    pub category: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
    #[serde(default)]
    pub needs_review_only: bool,
    /// Import History's detail view (Import Engine v2, specs/phase-009):
    /// every transaction a specific import produced, regardless of what
    /// else has happened to them since.
    #[serde(default)]
    pub source_import_id: Option<i64>,
}

#[derive(Debug)]
pub enum TransactionError {
    Validation(String),
    Db(rusqlite::Error),
    NotFound(i64),
    DeleteRequiresNoSource(i64),
}

impl std::fmt::Display for TransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionError::Validation(msg) => write!(f, "{msg}"),
            TransactionError::Db(e) => write!(f, "database error: {e}"),
            TransactionError::NotFound(id) => write!(f, "transaction {id} not found"),
            TransactionError::DeleteRequiresNoSource(id) => write!(
                f,
                "transaction {id} came from an import and can only be archived, not deleted, to preserve import history"
            ),
        }
    }
}

impl From<rusqlite::Error> for TransactionError {
    fn from(e: rusqlite::Error) -> Self {
        TransactionError::Db(e)
    }
}

const COLUMNS: &str = "id, account_id, transaction_date, posted_date, amount_cents, transaction_type, \
    raw_description, merchant, category, subcategory, notes, tags, is_pending, is_archived, review_status, \
    source_import_id, source_row_hash, transfer_pair_transaction_id, refund_of_transaction_id, \
    payee_id, split_parent_transaction_id, external_transaction_id, reconciliation_status, is_one_time, \
    created_at, updated_at";

fn row_to_transaction(row: &rusqlite::Row) -> rusqlite::Result<Transaction> {
    Ok(Transaction {
        id: row.get("id")?,
        account_id: row.get("account_id")?,
        transaction_date: row.get("transaction_date")?,
        posted_date: row.get("posted_date")?,
        amount_cents: row.get("amount_cents")?,
        transaction_type: TransactionType::from_sql_str(
            &row.get::<_, String>("transaction_type")?,
        )?,
        raw_description: row.get("raw_description")?,
        merchant: row.get("merchant")?,
        category: row.get("category")?,
        subcategory: row.get("subcategory")?,
        notes: row.get("notes")?,
        tags: row.get("tags")?,
        is_pending: row.get::<_, Option<i64>>("is_pending")?.map(|v| v != 0),
        is_archived: row.get::<_, i64>("is_archived")? != 0,
        needs_review: row.get::<_, String>("review_status")? == "needs_review",
        source_import_id: row.get("source_import_id")?,
        source_row_hash: row.get("source_row_hash")?,
        transfer_pair_transaction_id: row.get("transfer_pair_transaction_id")?,
        refund_of_transaction_id: row.get("refund_of_transaction_id")?,
        payee_id: row.get("payee_id")?,
        split_parent_transaction_id: row.get("split_parent_transaction_id")?,
        external_transaction_id: row.get("external_transaction_id")?,
        reconciliation_status: ReconciliationStatus::from_sql_str(
            &row.get::<_, String>("reconciliation_status")?,
        )?,
        is_one_time: row.get::<_, i64>("is_one_time")? != 0,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

type NormalizedInput = (String, Option<String>, String);

fn validate_and_normalize(input: &TransactionInput) -> Result<NormalizedInput, TransactionError> {
    if input.raw_description.trim().is_empty() {
        return Err(TransactionError::Validation(
            "Description is required.".to_string(),
        ));
    }
    let date = money::parse_date(&input.transaction_date).map_err(TransactionError::Validation)?;
    let posted = match &input.posted_date {
        Some(p) if !p.trim().is_empty() => {
            Some(money::parse_date(p).map_err(TransactionError::Validation)?)
        }
        _ => None,
    };
    Ok((date, posted, input.raw_description.trim().to_string()))
}

pub fn create(
    conn: &Connection,
    input: &TransactionInput,
) -> Result<Transaction, TransactionError> {
    let (date, posted, description) = validate_and_normalize(input)?;
    let merchant = input
        .merchant
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or(&description)
        .to_string();

    let id = conn.query_row(
        "INSERT INTO transactions (
            account_id, transaction_date, posted_date, amount_cents, transaction_type,
            raw_description, merchant, category, subcategory, notes, tags, is_pending
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        RETURNING id",
        params![
            input.account_id,
            date,
            posted,
            input.amount_cents,
            input.transaction_type.as_sql_str(),
            description,
            merchant,
            input.category,
            input.subcategory,
            input.notes,
            input.tags,
            input.is_pending.map(|v| v as i64),
        ],
        |row| row.get::<_, i64>(0),
    )?;
    get(conn, id)?.ok_or(TransactionError::NotFound(id))
}

fn get(conn: &Connection, id: i64) -> Result<Option<Transaction>, TransactionError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM transactions WHERE id = ?1"),
        params![id],
        row_to_transaction,
    )
    .optional()
    .map_err(TransactionError::from)
}

pub fn list(
    conn: &Connection,
    filter: &TransactionFilter,
) -> Result<Vec<Transaction>, TransactionError> {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if !filter.include_archived {
        clauses.push("is_archived = 0".to_string());
    }
    if let Some(account_id) = filter.account_id {
        clauses.push(format!("account_id = ?{}", args.len() + 1));
        args.push(Box::new(account_id));
    }
    if let Some(search) = filter.search.as_deref().filter(|s| !s.trim().is_empty()) {
        let pattern = format!("%{}%", search.trim());
        clauses.push(format!(
            "(merchant LIKE ?{} ESCAPE '\\' OR raw_description LIKE ?{} ESCAPE '\\' OR notes LIKE ?{} ESCAPE '\\')",
            args.len() + 1,
            args.len() + 2,
            args.len() + 3,
        ));
        args.push(Box::new(pattern.clone()));
        args.push(Box::new(pattern.clone()));
        args.push(Box::new(pattern));
    }
    if let Some(category) = filter.category.as_deref().filter(|c| !c.is_empty()) {
        clauses.push(format!("category = ?{}", args.len() + 1));
        args.push(Box::new(category.to_string()));
    }
    if let Some(from) = filter.date_from.as_deref().filter(|d| !d.is_empty()) {
        clauses.push(format!("transaction_date >= ?{}", args.len() + 1));
        args.push(Box::new(from.to_string()));
    }
    if let Some(to) = filter.date_to.as_deref().filter(|d| !d.is_empty()) {
        clauses.push(format!("transaction_date <= ?{}", args.len() + 1));
        args.push(Box::new(to.to_string()));
    }
    if filter.needs_review_only {
        clauses.push("review_status = 'needs_review'".to_string());
    }
    if let Some(source_import_id) = filter.source_import_id {
        clauses.push(format!("source_import_id = ?{}", args.len() + 1));
        args.push(Box::new(source_import_id));
    }

    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    let sql = format!(
        "SELECT {COLUMNS} FROM transactions {where_sql} ORDER BY transaction_date DESC, id DESC"
    );

    let mut stmt = conn.prepare(&sql)?;
    let arg_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(arg_refs.as_slice(), row_to_transaction)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(TransactionError::from)
}

fn format_dollars(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

/// Every non-archived transaction as CSV, newest first - the "don't get
/// trapped inside MIDAS" export. Human-readable columns (account name,
/// dollars not cents) rather than a raw table dump; not intended to round
/// -trip back through the bank-CSV importer, which is deliberately fuzzy
/// about column names and doesn't expect this exact shape.
pub fn export_csv(conn: &Connection) -> Result<String, TransactionError> {
    let mut stmt = conn.prepare(
        "SELECT t.transaction_date, a.name, t.merchant, t.category, t.subcategory,
                t.transaction_type, t.amount_cents, t.notes
         FROM transactions t JOIN accounts a ON a.id = t.account_id
         WHERE t.is_archived = 0
         ORDER BY t.transaction_date DESC, t.id DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;

    let mut writer = csv::WriterBuilder::new().from_writer(vec![]);
    writer
        .write_record([
            "Date",
            "Account",
            "Merchant",
            "Category",
            "Subcategory",
            "Type",
            "Amount",
            "Notes",
        ])
        .map_err(|e| TransactionError::Validation(e.to_string()))?;
    for row in rows {
        let (date, account, merchant, category, subcategory, transaction_type, amount_cents, notes) =
            row?;
        writer
            .write_record([
                date,
                account,
                merchant,
                category.unwrap_or_default(),
                subcategory.unwrap_or_default(),
                transaction_type,
                format_dollars(amount_cents),
                notes.unwrap_or_default(),
            ])
            .map_err(|e| TransactionError::Validation(e.to_string()))?;
    }

    let bytes = writer
        .into_inner()
        .map_err(|e| TransactionError::Validation(e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| TransactionError::Validation(e.to_string()))
}

pub fn update(
    conn: &Connection,
    id: i64,
    input: &TransactionInput,
) -> Result<Transaction, TransactionError> {
    let (date, posted, description) = validate_and_normalize(input)?;
    let merchant = input
        .merchant
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or(&description)
        .to_string();

    let affected = conn.execute(
        "UPDATE transactions SET
            account_id = ?1, transaction_date = ?2, posted_date = ?3, amount_cents = ?4,
            transaction_type = ?5, raw_description = ?6, merchant = ?7, category = ?8,
            subcategory = ?9, notes = ?10, tags = ?11, is_pending = ?12,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?13",
        params![
            input.account_id,
            date,
            posted,
            input.amount_cents,
            input.transaction_type.as_sql_str(),
            description,
            merchant,
            input.category,
            input.subcategory,
            input.notes,
            input.tags,
            input.is_pending.map(|v| v as i64),
            id,
        ],
    )?;
    if affected == 0 {
        return Err(TransactionError::NotFound(id));
    }
    get(conn, id)?.ok_or(TransactionError::NotFound(id))
}

pub fn set_archived(
    conn: &Connection,
    id: i64,
    archived: bool,
) -> Result<Transaction, TransactionError> {
    let affected = conn.execute(
        "UPDATE transactions SET is_archived = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?2",
        params![archived as i64, id],
    )?;
    if affected == 0 {
        return Err(TransactionError::NotFound(id));
    }
    get(conn, id)?.ok_or(TransactionError::NotFound(id))
}

/// The owner's own reconciliation lifecycle - editable even on a
/// `Reconciled` transaction (per product handoff section 19/new-cycle
/// section 7's reconciliation rules, MIDAS never blocks an edit, only warns
/// in the UI that a verified period may need re-checking - the warning
/// itself is a future batch's job, this is just the data mutation).
pub fn set_reconciliation_status(
    conn: &Connection,
    id: i64,
    status: ReconciliationStatus,
) -> Result<Transaction, TransactionError> {
    let affected = conn.execute(
        "UPDATE transactions SET reconciliation_status = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?2",
        params![status.as_sql_str(), id],
    )?;
    if affected == 0 {
        return Err(TransactionError::NotFound(id));
    }
    get(conn, id)?.ok_or(TransactionError::NotFound(id))
}

/// Hard delete is allowed only for transactions with no import source - a
/// manual entry the user made in error. Imported transactions can only be
/// archived, never deleted, so import history always stays reconstructable
/// (product handoff section 19: "no silent destructive actions").
pub fn delete(conn: &Connection, id: i64) -> Result<(), TransactionError> {
    let source: Option<Option<i64>> = conn
        .query_row(
            "SELECT source_import_id FROM transactions WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    match source {
        None => Err(TransactionError::NotFound(id)),
        Some(Some(_)) => Err(TransactionError::DeleteRequiresNoSource(id)),
        Some(None) => {
            conn.execute("DELETE FROM transactions WHERE id = ?1", params![id])?;
            Ok(())
        }
    }
}

#[tauri::command]
pub fn list_transactions(
    state: State<DbState>,
    filter: TransactionFilter,
) -> Result<Vec<Transaction>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list(&conn, &filter).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_transactions_csv(state: State<DbState>) -> Result<String, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    export_csv(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_transaction(
    state: State<DbState>,
    input: TransactionInput,
) -> Result<Transaction, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    create(&conn, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_transaction(
    state: State<DbState>,
    id: i64,
    input: TransactionInput,
) -> Result<Transaction, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    update(&conn, id, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_transaction_archived(
    state: State<DbState>,
    id: i64,
    archived: bool,
) -> Result<Transaction, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    set_archived(&conn, id, archived).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_transaction_reconciliation_status(
    state: State<DbState>,
    id: i64,
    status: ReconciliationStatus,
) -> Result<Transaction, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    set_reconciliation_status(&conn, id, status).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_transaction(state: State<DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    delete(&conn, id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};

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

    fn sample_input(account_id: i64) -> TransactionInput {
        TransactionInput {
            account_id,
            transaction_date: "2026-03-04".to_string(),
            posted_date: None,
            amount_cents: -5000,
            transaction_type: TransactionType::Purchase,
            raw_description: "SQ *COFFEE SHOP 123".to_string(),
            merchant: None,
            category: None,
            subcategory: None,
            notes: None,
            tags: None,
            is_pending: None,
        }
    }

    #[test]
    fn export_csv_includes_account_name_dollars_and_excludes_archived() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let created = create(&conn, &sample_input(account_id)).unwrap();

        let archived_input = TransactionInput {
            transaction_date: "2026-01-01".to_string(),
            amount_cents: -100,
            ..sample_input(account_id)
        };
        let archived = create(&conn, &archived_input).unwrap();
        set_archived(&conn, archived.id, true).unwrap();

        let csv = export_csv(&conn).unwrap();
        assert!(csv.starts_with("Date,Account,Merchant,Category,Subcategory,Type,Amount,Notes"));
        assert!(csv.contains("Test Account"));
        assert!(
            csv.contains("-50.00"),
            "amount must be human-readable dollars, not cents: {csv}"
        );
        assert!(
            !csv.contains("-1.00"),
            "archived transactions must never appear in the export: {csv}"
        );
        assert_eq!(
            csv.lines().count(),
            2,
            "header plus exactly one non-archived row"
        );
        let _ = created;
    }

    #[test]
    fn format_dollars_handles_negative_zero_and_positive_cents_correctly() {
        assert_eq!(format_dollars(-50000), "-500.00");
        assert_eq!(format_dollars(0), "0.00");
        assert_eq!(format_dollars(5), "0.05");
        assert_eq!(format_dollars(-5), "-0.05");
    }

    #[test]
    fn create_and_get_round_trips_and_defaults_merchant_to_description() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let created = create(&conn, &sample_input(account_id)).unwrap();
        assert_eq!(created.account_id, account_id);
        assert_eq!(created.amount_cents, -5000);
        assert_eq!(created.transaction_type, TransactionType::Purchase);
        assert_eq!(created.merchant, "SQ *COFFEE SHOP 123");
        assert!(!created.is_archived);
        assert!(!created.needs_review);

        let fetched = get(&conn, created.id).unwrap().unwrap();
        assert_eq!(fetched.id, created.id);
    }

    #[test]
    fn explicit_merchant_overrides_raw_description_default() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let mut input = sample_input(account_id);
        input.merchant = Some("Coffee Shop".to_string());
        let created = create(&conn, &input).unwrap();
        assert_eq!(created.merchant, "Coffee Shop");
        assert_eq!(created.raw_description, "SQ *COFFEE SHOP 123");
    }

    #[test]
    fn rejects_empty_description_and_invalid_date() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let mut input = sample_input(account_id);
        input.raw_description = "   ".to_string();
        assert!(matches!(
            create(&conn, &input),
            Err(TransactionError::Validation(_))
        ));

        let mut input2 = sample_input(account_id);
        input2.transaction_date = "not a date".to_string();
        assert!(matches!(
            create(&conn, &input2),
            Err(TransactionError::Validation(_))
        ));
    }

    #[test]
    fn account_relationship_is_enforced_by_foreign_key() {
        let conn = test_conn();
        let input = sample_input(999_999); // no such account
        let err = create(&conn, &input).unwrap_err();
        assert!(matches!(err, TransactionError::Db(_)));
    }

    #[test]
    fn update_changes_fields() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let created = create(&conn, &sample_input(account_id)).unwrap();

        let mut changed = sample_input(account_id);
        changed.amount_cents = -7500;
        changed.category = Some("Dining".to_string());
        let updated = update(&conn, created.id, &changed).unwrap();
        assert_eq!(updated.amount_cents, -7500);
        assert_eq!(updated.category.as_deref(), Some("Dining"));
    }

    #[test]
    fn archive_hides_from_default_list_and_delete_is_blocked_for_imported() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let manual = create(&conn, &sample_input(account_id)).unwrap();

        set_archived(&conn, manual.id, true).unwrap();
        let active = list(
            &conn,
            &TransactionFilter {
                account_id: Some(account_id),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(active.is_empty());

        let all = list(
            &conn,
            &TransactionFilter {
                account_id: Some(account_id),
                include_archived: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 1);

        // Manual entry (no source_import_id) can be hard-deleted.
        delete(&conn, manual.id).unwrap();
        assert!(get(&conn, manual.id).unwrap().is_none());
    }

    #[test]
    fn delete_rejects_transactions_with_an_import_source() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let created = create(&conn, &sample_input(account_id)).unwrap();
        conn.execute(
            "INSERT INTO imports (account_id, source_filename, file_hash, row_count, committed_count, duplicate_count, needs_review_count, column_mapping_json)
             VALUES (?1, 'test.csv', 'hash', 1, 1, 0, 0, '{}')",
            params![account_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE transactions SET source_import_id = 1 WHERE id = ?1",
            params![created.id],
        )
        .unwrap();

        let err = delete(&conn, created.id).unwrap_err();
        assert!(matches!(err, TransactionError::DeleteRequiresNoSource(_)));
        assert!(get(&conn, created.id).unwrap().is_some());
    }

    #[test]
    fn search_filters_by_merchant_and_description_and_notes() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let mut coffee = sample_input(account_id);
        coffee.merchant = Some("Coffee Shop".to_string());
        create(&conn, &coffee).unwrap();
        let mut grocery = sample_input(account_id);
        grocery.raw_description = "WHOLE FOODS #42".to_string();
        grocery.merchant = Some("Whole Foods".to_string());
        create(&conn, &grocery).unwrap();

        let results = list(
            &conn,
            &TransactionFilter {
                search: Some("coffee".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].merchant, "Coffee Shop");
    }

    #[test]
    fn date_range_filters_are_inclusive() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let mut early = sample_input(account_id);
        early.transaction_date = "2026-01-01".to_string();
        create(&conn, &early).unwrap();
        let mut late = sample_input(account_id);
        late.transaction_date = "2026-06-01".to_string();
        create(&conn, &late).unwrap();

        let results = list(
            &conn,
            &TransactionFilter {
                date_from: Some("2026-01-01".to_string()),
                date_to: Some("2026-01-01".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].transaction_date, "2026-01-01");
    }

    #[test]
    fn source_import_id_filter_isolates_one_imports_transactions() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let manual = create(&conn, &sample_input(account_id)).unwrap();
        conn.execute(
            "INSERT INTO imports (account_id, source_filename, file_hash, row_count, committed_count, duplicate_count, needs_review_count, column_mapping_json)
             VALUES (?1, 'test.csv', 'hash', 1, 1, 0, 0, '{}')",
            params![account_id],
        )
        .unwrap();
        let imported = create(&conn, &sample_input(account_id)).unwrap();
        conn.execute(
            "UPDATE transactions SET source_import_id = 1 WHERE id = ?1",
            params![imported.id],
        )
        .unwrap();

        let results = list(
            &conn,
            &TransactionFilter {
                source_import_id: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, imported.id);
        assert_ne!(results[0].id, manual.id);
    }

    #[test]
    fn credit_card_purchase_is_positive_and_still_classified_as_spending() {
        // ADR-0004: liability balance goes UP on a purchase (positive), but
        // transaction_type = Purchase is what marks it as spending, not sign.
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::CreditCard);
        let mut charge = sample_input(account_id);
        charge.amount_cents = 4200;
        charge.transaction_type = TransactionType::Purchase;
        let created = create(&conn, &charge).unwrap();
        assert_eq!(created.amount_cents, 4200);
        assert_eq!(created.transaction_type, TransactionType::Purchase);
    }

    #[test]
    fn credit_card_payment_is_modeled_as_transfer_not_a_second_expense() {
        let conn = test_conn();
        let checking_id = make_account(&conn, AccountType::Checking);
        let card_id = make_account(&conn, AccountType::CreditCard);

        let mut checking_leg = sample_input(checking_id);
        checking_leg.amount_cents = -20000;
        checking_leg.transaction_type = TransactionType::Transfer;
        checking_leg.raw_description = "PAYMENT TO CREDIT CARD".to_string();
        let checking_tx = create(&conn, &checking_leg).unwrap();

        let mut card_leg = sample_input(card_id);
        card_leg.amount_cents = -20000;
        card_leg.transaction_type = TransactionType::Transfer;
        card_leg.raw_description = "PAYMENT RECEIVED".to_string();
        let card_tx = create(&conn, &card_leg).unwrap();

        // Neither leg is a Purchase, so neither counts as spending - the
        // accounting invariant the product handoff requires.
        assert_ne!(checking_tx.transaction_type, TransactionType::Purchase);
        assert_ne!(card_tx.transaction_type, TransactionType::Purchase);
    }

    #[test]
    fn refund_is_not_income_by_default_classification() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let mut refund = sample_input(account_id);
        refund.amount_cents = 1999;
        refund.transaction_type = TransactionType::Refund;
        let created = create(&conn, &refund).unwrap();
        assert_eq!(created.transaction_type, TransactionType::Refund);
        assert_ne!(created.transaction_type, TransactionType::Income);
    }

    #[test]
    fn new_foundation_batch_1_fields_default_sanely_on_a_freshly_created_transaction() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let created = create(&conn, &sample_input(account_id)).unwrap();
        assert_eq!(created.payee_id, None);
        assert_eq!(created.split_parent_transaction_id, None);
        assert_eq!(created.external_transaction_id, None);
        assert_eq!(
            created.reconciliation_status,
            ReconciliationStatus::Unreconciled
        );
        assert!(!created.is_one_time);
    }

    #[test]
    fn set_reconciliation_status_updates_and_is_independent_of_is_pending() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let created = create(&conn, &sample_input(account_id)).unwrap();
        assert_eq!(
            created.reconciliation_status,
            ReconciliationStatus::Unreconciled
        );

        let cleared =
            set_reconciliation_status(&conn, created.id, ReconciliationStatus::Cleared).unwrap();
        assert_eq!(cleared.reconciliation_status, ReconciliationStatus::Cleared);
        assert_eq!(cleared.is_pending, None, "unrelated to is_pending");

        let reconciled =
            set_reconciliation_status(&conn, created.id, ReconciliationStatus::Reconciled).unwrap();
        assert_eq!(
            reconciled.reconciliation_status,
            ReconciliationStatus::Reconciled
        );

        // Editing a reconciled transaction must still succeed (the UI, not
        // the data layer, is responsible for warning that a verified period
        // may need re-checking) - the constitution's "no silent destructive
        // behavior" is about not losing data, not about freezing edits.
        let mut edit = sample_input(account_id);
        edit.amount_cents = -9999;
        let updated = update(&conn, created.id, &edit).unwrap();
        assert_eq!(updated.amount_cents, -9999);
        assert_eq!(
            updated.reconciliation_status,
            ReconciliationStatus::Reconciled,
            "a normal field edit must not silently reset reconciliation status"
        );
    }

    #[test]
    fn set_reconciliation_status_on_missing_transaction_errors() {
        let conn = test_conn();
        assert!(matches!(
            set_reconciliation_status(&conn, 999, ReconciliationStatus::Cleared),
            Err(TransactionError::NotFound(999))
        ));
    }

    #[test]
    fn needs_review_only_filter_works() {
        let conn = test_conn();
        let account_id = make_account(&conn, AccountType::Checking);
        let plain = create(&conn, &sample_input(account_id)).unwrap();
        let flagged = create(&conn, &sample_input(account_id)).unwrap();
        conn.execute(
            "UPDATE transactions SET review_status = 'needs_review' WHERE id = ?1",
            params![flagged.id],
        )
        .unwrap();

        let results = list(
            &conn,
            &TransactionFilter {
                needs_review_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, flagged.id);
        assert_ne!(results[0].id, plain.id);
    }
}
