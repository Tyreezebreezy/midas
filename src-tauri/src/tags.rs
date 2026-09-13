//! Contextual tags (Foundation Batch 1 - specs/phase-008): a normalized
//! many-to-many, replacing free-text tagging in spirit so a future Tags view
//! can filter/join efficiently. `transactions.tags` (the existing free-text
//! column) is left in place - unrelated to this new structure, and nothing
//! requires removing it for this foundation to work. Tags are organizational
//! labels, not financial records, so unlike accounts/transactions a tag can
//! be hard-deleted; its `transaction_tags` links are cleaned up explicitly
//! first, since the foreign key has no `ON DELETE CASCADE`.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;

use crate::db::DbState;

#[derive(Debug, Clone, Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub created_at: String,
}

#[derive(Debug)]
pub enum TagError {
    Validation(String),
    Duplicate,
    Db(rusqlite::Error),
    NotFound(i64),
}

impl std::fmt::Display for TagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TagError::Validation(msg) => write!(f, "{msg}"),
            TagError::Duplicate => write!(f, "A tag with this name already exists."),
            TagError::Db(e) => write!(f, "database error: {e}"),
            TagError::NotFound(id) => write!(f, "tag {id} not found"),
        }
    }
}

impl From<rusqlite::Error> for TagError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::SqliteFailure(err, _)
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                TagError::Duplicate
            }
            other => TagError::Db(other),
        }
    }
}

fn row_to_tag(row: &rusqlite::Row) -> rusqlite::Result<Tag> {
    Ok(Tag {
        id: row.get("id")?,
        name: row.get("name")?,
        created_at: row.get("created_at")?,
    })
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Tag>, TagError> {
    conn.query_row(
        "SELECT id, name, created_at FROM tags WHERE id = ?1",
        params![id],
        row_to_tag,
    )
    .optional()
    .map_err(TagError::from)
}

pub fn create(conn: &Connection, name: &str) -> Result<Tag, TagError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(TagError::Validation(
            "Tag name must not be empty.".to_string(),
        ));
    }
    let id = conn.query_row(
        "INSERT INTO tags (name) VALUES (?1) RETURNING id",
        params![trimmed],
        |row| row.get::<_, i64>(0),
    )?;
    get(conn, id)?.ok_or(TagError::NotFound(id))
}

pub fn list(conn: &Connection) -> Result<Vec<Tag>, TagError> {
    let mut stmt =
        conn.prepare("SELECT id, name, created_at FROM tags ORDER BY name COLLATE NOCASE")?;
    let rows = stmt.query_map([], row_to_tag)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(TagError::from)
}

pub fn delete(conn: &Connection, id: i64) -> Result<(), TagError> {
    conn.execute(
        "DELETE FROM transaction_tags WHERE tag_id = ?1",
        params![id],
    )?;
    let affected = conn.execute("DELETE FROM tags WHERE id = ?1", params![id])?;
    if affected == 0 {
        return Err(TagError::NotFound(id));
    }
    Ok(())
}

/// Idempotent - tagging an already-tagged transaction again is a no-op, not
/// an error, since the UI's natural action is "make sure this tag is on
/// this transaction" rather than "add a row."
pub fn attach(conn: &Connection, transaction_id: i64, tag_id: i64) -> Result<(), TagError> {
    conn.execute(
        "INSERT OR IGNORE INTO transaction_tags (transaction_id, tag_id) VALUES (?1, ?2)",
        params![transaction_id, tag_id],
    )?;
    Ok(())
}

pub fn detach(conn: &Connection, transaction_id: i64, tag_id: i64) -> Result<(), TagError> {
    conn.execute(
        "DELETE FROM transaction_tags WHERE transaction_id = ?1 AND tag_id = ?2",
        params![transaction_id, tag_id],
    )?;
    Ok(())
}

pub fn list_for_transaction(conn: &Connection, transaction_id: i64) -> Result<Vec<Tag>, TagError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, t.created_at FROM tags t
         JOIN transaction_tags tt ON tt.tag_id = t.id
         WHERE tt.transaction_id = ?1
         ORDER BY t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(params![transaction_id], row_to_tag)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(TagError::from)
}

#[tauri::command]
pub fn list_tags(state: State<DbState>) -> Result<Vec<Tag>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_tag(state: State<DbState>, name: String) -> Result<Tag, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    create(&conn, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_tag(state: State<DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    delete(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn attach_tag(state: State<DbState>, transaction_id: i64, tag_id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    attach(&conn, transaction_id, tag_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn detach_tag(state: State<DbState>, transaction_id: i64, tag_id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    detach(&conn, transaction_id, tag_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_tags_for_transaction(
    state: State<DbState>,
    transaction_id: i64,
) -> Result<Vec<Tag>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list_for_transaction(&conn, transaction_id).map_err(|e| e.to_string())
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
        conn.execute_batch(include_str!("../migrations/0011_reconciliation.sql"))
            .unwrap();
        conn.execute_batch(include_str!(
            "../migrations/0012_transaction_intelligence.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!(
            "../migrations/0013_recurring_intelligence.sql"
        ))
        .unwrap();
        conn
    }

    fn make_transaction(conn: &Connection) -> i64 {
        let account_id = accounts::create(
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
        .id;
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -4200,
                transaction_type: TransactionType::Purchase,
                raw_description: "Test".to_string(),
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
        .unwrap()
        .id
    }

    #[test]
    fn create_list_and_delete_round_trip() {
        let conn = test_conn();
        let tag = create(&conn, "Vacation").unwrap();
        assert_eq!(tag.name, "Vacation");
        assert_eq!(list(&conn).unwrap().len(), 1);

        delete(&conn, tag.id).unwrap();
        assert_eq!(list(&conn).unwrap().len(), 0);
    }

    #[test]
    fn create_rejects_empty_name_and_duplicate_name() {
        let conn = test_conn();
        assert!(matches!(create(&conn, "  "), Err(TagError::Validation(_))));
        create(&conn, "Business").unwrap();
        assert!(matches!(
            create(&conn, "Business"),
            Err(TagError::Duplicate)
        ));
    }

    #[test]
    fn attach_is_idempotent_and_detach_removes_the_link() {
        let conn = test_conn();
        let txn_id = make_transaction(&conn);
        let tag = create(&conn, "Reimbursable").unwrap();

        attach(&conn, txn_id, tag.id).unwrap();
        attach(&conn, txn_id, tag.id).unwrap(); // must not error on a duplicate attach
        assert_eq!(list_for_transaction(&conn, txn_id).unwrap().len(), 1);

        detach(&conn, txn_id, tag.id).unwrap();
        assert_eq!(list_for_transaction(&conn, txn_id).unwrap().len(), 0);
    }

    #[test]
    fn deleting_a_tag_removes_its_transaction_links_too() {
        let conn = test_conn();
        let txn_id = make_transaction(&conn);
        let tag = create(&conn, "Trip").unwrap();
        attach(&conn, txn_id, tag.id).unwrap();

        delete(&conn, tag.id).unwrap();

        // The foreign key has no ON DELETE CASCADE - if delete() did not
        // clean up transaction_tags itself, this would fail instead of
        // returning an empty list.
        assert_eq!(list_for_transaction(&conn, txn_id).unwrap().len(), 0);
    }

    #[test]
    fn delete_on_missing_tag_returns_not_found() {
        let conn = test_conn();
        assert!(matches!(delete(&conn, 999), Err(TagError::NotFound(999))));
    }
}
