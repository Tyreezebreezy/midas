//! Normalized Payee Library (Foundation Batch 1 - specs/phase-008). A payee
//! is a separate, mergeable identity from a transaction's raw/merchant text;
//! nothing links a transaction to one yet (`transactions.payee_id` stays
//! unpopulated until a later batch teaches import/rules to set it), so this
//! module is pure CRUD plus the one operation that needs care: merging two
//! payees together without ever deleting a row other data might reference.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::DbState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayeeType {
    Merchant,
    Company,
    Person,
}

impl PayeeType {
    fn as_sql_str(self) -> &'static str {
        match self {
            PayeeType::Merchant => "merchant",
            PayeeType::Company => "company",
            PayeeType::Person => "person",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "merchant" => PayeeType::Merchant,
            "company" => PayeeType::Company,
            "person" => PayeeType::Person,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown payee_type '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Payee {
    pub id: i64,
    pub name: String,
    pub payee_type: PayeeType,
    pub notes: Option<String>,
    pub is_archived: bool,
    /// `Some(target_id)` once merged - the row is kept (never deleted) so
    /// anything still naming it can resolve to where it went.
    pub merged_into_payee_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct PayeeInput {
    pub name: String,
    pub payee_type: PayeeType,
    pub notes: Option<String>,
}

#[derive(Debug)]
pub enum PayeeError {
    Validation(String),
    Db(rusqlite::Error),
    NotFound(i64),
}

impl std::fmt::Display for PayeeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayeeError::Validation(msg) => write!(f, "{msg}"),
            PayeeError::Db(e) => write!(f, "database error: {e}"),
            PayeeError::NotFound(id) => write!(f, "payee {id} not found"),
        }
    }
}

impl From<rusqlite::Error> for PayeeError {
    fn from(e: rusqlite::Error) -> Self {
        PayeeError::Db(e)
    }
}

const COLUMNS: &str =
    "id, name, payee_type, notes, is_archived, merged_into_payee_id, created_at, updated_at";

fn row_to_payee(row: &rusqlite::Row) -> rusqlite::Result<Payee> {
    Ok(Payee {
        id: row.get("id")?,
        name: row.get("name")?,
        payee_type: PayeeType::from_sql_str(&row.get::<_, String>("payee_type")?)?,
        notes: row.get("notes")?,
        is_archived: row.get::<_, i64>("is_archived")? != 0,
        merged_into_payee_id: row.get("merged_into_payee_id")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn validate(input: &PayeeInput) -> Result<(), PayeeError> {
    if input.name.trim().is_empty() {
        return Err(PayeeError::Validation(
            "Payee name must not be empty.".to_string(),
        ));
    }
    Ok(())
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Payee>, PayeeError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM payees WHERE id = ?1"),
        params![id],
        row_to_payee,
    )
    .optional()
    .map_err(PayeeError::from)
}

pub fn create(conn: &Connection, input: &PayeeInput) -> Result<Payee, PayeeError> {
    validate(input)?;
    let id = conn.query_row(
        "INSERT INTO payees (name, payee_type, notes) VALUES (?1, ?2, ?3) RETURNING id",
        params![
            input.name.trim(),
            input.payee_type.as_sql_str(),
            input.notes
        ],
        |row| row.get::<_, i64>(0),
    )?;
    get(conn, id)?.ok_or(PayeeError::NotFound(id))
}

pub fn list(conn: &Connection, include_archived: bool) -> Result<Vec<Payee>, PayeeError> {
    let sql = if include_archived {
        format!("SELECT {COLUMNS} FROM payees ORDER BY is_archived, name COLLATE NOCASE")
    } else {
        format!("SELECT {COLUMNS} FROM payees WHERE is_archived = 0 ORDER BY name COLLATE NOCASE")
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_payee)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(PayeeError::from)
}

pub fn update(conn: &Connection, id: i64, input: &PayeeInput) -> Result<Payee, PayeeError> {
    validate(input)?;
    let affected = conn.execute(
        "UPDATE payees SET name = ?1, payee_type = ?2, notes = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?4",
        params![input.name.trim(), input.payee_type.as_sql_str(), input.notes, id],
    )?;
    if affected == 0 {
        return Err(PayeeError::NotFound(id));
    }
    get(conn, id)?.ok_or(PayeeError::NotFound(id))
}

pub fn set_archived(conn: &Connection, id: i64, archived: bool) -> Result<Payee, PayeeError> {
    let affected = conn.execute(
        "UPDATE payees SET is_archived = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?2",
        params![archived as i64, id],
    )?;
    if affected == 0 {
        return Err(PayeeError::NotFound(id));
    }
    get(conn, id)?.ok_or(PayeeError::NotFound(id))
}

/// Redirects `source` into `target`: every transaction currently linked to
/// `source` is repointed at `target`, then `source` is marked merged (never
/// deleted, so it stays resolvable for anything that still names it) and
/// archived so it stops appearing as a normal pick-a-payee option. Merging
/// into an already-merged payee is rejected rather than silently chased to
/// its ultimate target - keeps "what does this point at" a single lookup,
/// never a chain, at the cost of asking the owner to merge in the right
/// order on the rare double-merge.
pub fn merge(conn: &Connection, source_id: i64, target_id: i64) -> Result<Payee, PayeeError> {
    if source_id == target_id {
        return Err(PayeeError::Validation(
            "A payee cannot be merged into itself.".to_string(),
        ));
    }
    let source = get(conn, source_id)?.ok_or(PayeeError::NotFound(source_id))?;
    let target = get(conn, target_id)?.ok_or(PayeeError::NotFound(target_id))?;
    if source.merged_into_payee_id.is_some() {
        return Err(PayeeError::Validation(
            "This payee has already been merged.".to_string(),
        ));
    }
    if target.merged_into_payee_id.is_some() {
        return Err(PayeeError::Validation(
            "Cannot merge into a payee that has itself been merged - merge into its target instead.".to_string(),
        ));
    }

    conn.execute(
        "UPDATE transactions SET payee_id = ?1 WHERE payee_id = ?2",
        params![target_id, source_id],
    )?;
    conn.execute(
        "UPDATE payees SET merged_into_payee_id = ?1, is_archived = 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?2",
        params![target_id, source_id],
    )?;
    get(conn, source_id)?.ok_or(PayeeError::NotFound(source_id))
}

#[tauri::command]
pub fn list_payees(state: State<DbState>, include_archived: bool) -> Result<Vec<Payee>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list(&conn, include_archived).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_payee(state: State<DbState>, input: PayeeInput) -> Result<Payee, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    create(&conn, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_payee(state: State<DbState>, id: i64, input: PayeeInput) -> Result<Payee, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    update(&conn, id, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_payee_archived(state: State<DbState>, id: i64, archived: bool) -> Result<Payee, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    set_archived(&conn, id, archived).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn merge_payees(
    state: State<DbState>,
    source_id: i64,
    target_id: i64,
) -> Result<Payee, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    merge(&conn, source_id, target_id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::transactions::{self, TransactionInput, TransactionType};

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
        conn
    }

    fn sample_input() -> PayeeInput {
        PayeeInput {
            name: "Whole Foods".to_string(),
            payee_type: PayeeType::Merchant,
            notes: None,
        }
    }

    fn make_account(conn: &Connection) -> i64 {
        accounts::create(
            conn,
            &AccountInput {
                name: "Checking".to_string(),
                institution: None,
                account_type: AccountType::Checking,
                include_in_net_worth: true,
                notes: None,
            },
        )
        .unwrap()
        .id
    }

    fn make_transaction_with_payee(conn: &Connection, account_id: i64, payee_id: i64) -> i64 {
        let created = transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -4200,
                transaction_type: TransactionType::Purchase,
                raw_description: "WF #123".to_string(),
                merchant: Some("Whole Foods".to_string()),
                category: None,
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
            },
        )
        .unwrap();
        conn.execute(
            "UPDATE transactions SET payee_id = ?1 WHERE id = ?2",
            params![payee_id, created.id],
        )
        .unwrap();
        created.id
    }

    #[test]
    fn create_and_get_round_trips_all_fields() {
        let conn = test_conn();
        let created = create(&conn, &sample_input()).unwrap();
        assert_eq!(created.name, "Whole Foods");
        assert_eq!(created.payee_type, PayeeType::Merchant);
        assert!(!created.is_archived);
        assert_eq!(created.merged_into_payee_id, None);

        let fetched = get(&conn, created.id).unwrap().unwrap();
        assert_eq!(fetched.id, created.id);
    }

    #[test]
    fn create_rejects_empty_name() {
        let conn = test_conn();
        let mut input = sample_input();
        input.name = "   ".to_string();
        assert!(matches!(
            create(&conn, &input),
            Err(PayeeError::Validation(_))
        ));
    }

    #[test]
    fn all_three_payee_types_round_trip() {
        let conn = test_conn();
        for t in [PayeeType::Merchant, PayeeType::Company, PayeeType::Person] {
            let mut input = sample_input();
            input.payee_type = t;
            input.name = format!("{t:?} payee");
            assert_eq!(create(&conn, &input).unwrap().payee_type, t);
        }
    }

    #[test]
    fn list_excludes_archived_by_default_and_includes_when_asked() {
        let conn = test_conn();
        let a = create(&conn, &sample_input()).unwrap();
        let mut second = sample_input();
        second.name = "Old Merchant".to_string();
        let b = create(&conn, &second).unwrap();
        set_archived(&conn, b.id, true).unwrap();

        assert_eq!(
            list(&conn, false)
                .unwrap()
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec![a.id]
        );
        assert_eq!(list(&conn, true).unwrap().len(), 2);
    }

    #[test]
    fn update_and_archive_and_not_found_paths() {
        let conn = test_conn();
        let created = create(&conn, &sample_input()).unwrap();

        let mut changed = sample_input();
        changed.name = "Whole Foods Market".to_string();
        assert_eq!(
            update(&conn, created.id, &changed).unwrap().name,
            "Whole Foods Market"
        );

        assert!(set_archived(&conn, created.id, true).unwrap().is_archived);

        assert!(matches!(
            update(&conn, 999, &sample_input()),
            Err(PayeeError::NotFound(999))
        ));
        assert!(matches!(
            set_archived(&conn, 999, true),
            Err(PayeeError::NotFound(999))
        ));
    }

    #[test]
    fn merge_redirects_linked_transactions_and_archives_the_source_without_deleting_it() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        let source = create(&conn, &sample_input()).unwrap();
        let mut target_input = sample_input();
        target_input.name = "Whole Foods Market".to_string();
        let target = create(&conn, &target_input).unwrap();
        let txn_id = make_transaction_with_payee(&conn, account_id, source.id);

        let merged_source = merge(&conn, source.id, target.id).unwrap();
        assert_eq!(merged_source.merged_into_payee_id, Some(target.id));
        assert!(
            merged_source.is_archived,
            "a merged payee stops appearing as a normal pick option"
        );
        // Never deleted - still resolvable.
        assert!(get(&conn, source.id).unwrap().is_some());

        let txn = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                account_id: None,
                search: None,
                category: None,
                date_from: None,
                date_to: None,
                include_archived: false,
                needs_review_only: false,
                source_import_id: None,
            },
        )
        .unwrap()
        .into_iter()
        .find(|t| t.id == txn_id)
        .unwrap();
        assert_eq!(
            txn.payee_id,
            Some(target.id),
            "the transaction must follow the merge to the target"
        );
    }

    #[test]
    fn merge_rejects_self_merge_and_chained_merges() {
        let conn = test_conn();
        let a = create(&conn, &sample_input()).unwrap();
        let mut b_input = sample_input();
        b_input.name = "B".to_string();
        let b = create(&conn, &b_input).unwrap();
        let mut c_input = sample_input();
        c_input.name = "C".to_string();
        let c = create(&conn, &c_input).unwrap();

        assert!(matches!(
            merge(&conn, a.id, a.id),
            Err(PayeeError::Validation(_))
        ));

        merge(&conn, a.id, b.id).unwrap();
        // a is now merged into b; merging it again, or merging something
        // into it, must not build a chain.
        assert!(matches!(
            merge(&conn, a.id, c.id),
            Err(PayeeError::Validation(_))
        ));
        assert!(matches!(
            merge(&conn, c.id, a.id),
            Err(PayeeError::Validation(_))
        ));
    }
}
