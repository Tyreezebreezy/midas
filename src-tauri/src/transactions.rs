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
}

impl TransactionType {
    fn as_sql_str(self) -> &'static str {
        match self {
            TransactionType::Purchase => "purchase",
            TransactionType::Income => "income",
            TransactionType::Transfer => "transfer",
            TransactionType::Refund => "refund",
        }
    }

    pub fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "purchase" => TransactionType::Purchase,
            "income" => TransactionType::Income,
            "transfer" => TransactionType::Transfer,
            "refund" => TransactionType::Refund,
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
    source_import_id, source_row_hash, transfer_pair_transaction_id, refund_of_transaction_id, created_at, updated_at";

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
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn validate_and_normalize(input: &TransactionInput) -> Result<(String, String), TransactionError> {
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
    let _ = posted; // validated below alongside date via caller when needed
    Ok((date, input.raw_description.trim().to_string()))
}

pub fn create(
    conn: &Connection,
    input: &TransactionInput,
) -> Result<Transaction, TransactionError> {
    let (date, description) = validate_and_normalize(input)?;
    let posted = match &input.posted_date {
        Some(p) if !p.trim().is_empty() => {
            Some(money::parse_date(p).map_err(TransactionError::Validation)?)
        }
        _ => None,
    };
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

pub fn update(
    conn: &Connection,
    id: i64,
    input: &TransactionInput,
) -> Result<Transaction, TransactionError> {
    let (date, description) = validate_and_normalize(input)?;
    let posted = match &input.posted_date {
        Some(p) if !p.trim().is_empty() => {
            Some(money::parse_date(p).map_err(TransactionError::Validation)?)
        }
        _ => None,
    };
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
        conn
    }

    fn make_account(conn: &Connection, account_type: AccountType) -> i64 {
        accounts::create(
            conn,
            &AccountInput {
                name: "Test Account".to_string(),
                institution: None,
                account_type,
                balance_cents: 0,
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
