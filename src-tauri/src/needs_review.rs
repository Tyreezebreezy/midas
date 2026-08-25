use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::DbState;

#[derive(Debug, Clone, Serialize)]
pub struct NeedsReviewItem {
    pub id: i64,
    pub issue_type: String,
    pub transaction_id: Option<i64>,
    pub import_id: Option<i64>,
    pub source_row_json: Option<String>,
    pub detected_reason: String,
    pub proposed_resolution_json: Option<String>,
    pub resolution_status: String,
    pub resolved_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionAction {
    Resolved,
    Dismissed,
}

impl ResolutionAction {
    fn as_sql_str(self) -> &'static str {
        match self {
            ResolutionAction::Resolved => "resolved",
            ResolutionAction::Dismissed => "dismissed",
        }
    }
}

const COLUMNS: &str =
    "id, issue_type, transaction_id, import_id, source_row_json, detected_reason, \
    proposed_resolution_json, resolution_status, resolved_at, created_at";

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<NeedsReviewItem> {
    Ok(NeedsReviewItem {
        id: row.get("id")?,
        issue_type: row.get("issue_type")?,
        transaction_id: row.get("transaction_id")?,
        import_id: row.get("import_id")?,
        source_row_json: row.get("source_row_json")?,
        detected_reason: row.get("detected_reason")?,
        proposed_resolution_json: row.get("proposed_resolution_json")?,
        resolution_status: row.get("resolution_status")?,
        resolved_at: row.get("resolved_at")?,
        created_at: row.get("created_at")?,
    })
}

pub fn list(conn: &Connection, open_only: bool) -> rusqlite::Result<Vec<NeedsReviewItem>> {
    let sql = if open_only {
        format!("SELECT {COLUMNS} FROM needs_review_items WHERE resolution_status = 'open' ORDER BY created_at DESC")
    } else {
        format!("SELECT {COLUMNS} FROM needs_review_items ORDER BY created_at DESC")
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_item)?;
    rows.collect()
}

pub fn resolve(
    conn: &Connection,
    id: i64,
    action: ResolutionAction,
) -> Result<NeedsReviewItem, String> {
    let affected = conn
        .execute(
            "UPDATE needs_review_items SET resolution_status = ?1, resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?2",
            params![action.as_sql_str(), id],
        )
        .map_err(|e| e.to_string())?;
    if affected == 0 {
        return Err(format!("Needs Review item {id} not found"));
    }
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM needs_review_items WHERE id = ?1"),
        params![id],
        row_to_item,
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("Needs Review item {id} not found"))
}

#[tauri::command]
pub fn list_needs_review(
    state: State<DbState>,
    open_only: bool,
) -> Result<Vec<NeedsReviewItem>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list(&conn, open_only).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn resolve_needs_review(
    state: State<DbState>,
    id: i64,
    action: ResolutionAction,
) -> Result<NeedsReviewItem, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    resolve(&conn, id, action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute_batch(include_str!("../migrations/0001_initial.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../migrations/0002_transactions.sql"))
            .unwrap();
        conn
    }

    fn insert_item(conn: &Connection, issue_type: &str) -> i64 {
        conn.query_row(
            "INSERT INTO needs_review_items (issue_type, detected_reason) VALUES (?1, 'test reason') RETURNING id",
            params![issue_type],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn list_open_only_excludes_resolved_and_dismissed() {
        let conn = test_conn();
        let open_id = insert_item(&conn, "malformed_row");
        let resolved_id = insert_item(&conn, "suspected_duplicate");
        resolve(&conn, resolved_id, ResolutionAction::Resolved).unwrap();

        let open_items = list(&conn, true).unwrap();
        assert_eq!(open_items.len(), 1);
        assert_eq!(open_items[0].id, open_id);

        let all_items = list(&conn, false).unwrap();
        assert_eq!(all_items.len(), 2);
    }

    #[test]
    fn resolve_sets_status_and_timestamp() {
        let conn = test_conn();
        let id = insert_item(&conn, "ambiguous_transfer");
        let resolved = resolve(&conn, id, ResolutionAction::Dismissed).unwrap();
        assert_eq!(resolved.resolution_status, "dismissed");
        assert!(resolved.resolved_at.is_some());
    }

    #[test]
    fn resolve_missing_item_errors() {
        let conn = test_conn();
        assert!(resolve(&conn, 999, ResolutionAction::Resolved).is_err());
    }
}
