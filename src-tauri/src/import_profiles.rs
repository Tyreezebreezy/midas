//! Named, reusable CSV/OFX/QFX column-mapping memory per institution/account
//! (Foundation Batch 1 - specs/phase-008). Product handoff: "select bank
//! file -> MIDAS recognizes the profile -> preview -> import." This batch
//! is the storage only - matching a profile to an incoming file is Import
//! Engine v2's job. `account_id: None` means the profile applies to any
//! account, mirroring `merchant_rules`' existing global-vs-account-specific
//! convention (`rules.rs`). Reuses `import::ColumnMapping` as its stored
//! shape rather than inventing a second one - both round-trip through the
//! same `column_mapping_json` text column `imports` already has.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::DbState;
use crate::import::ColumnMapping;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSourceType {
    Csv,
    Ofx,
    Qfx,
}

impl ImportSourceType {
    fn as_sql_str(self) -> &'static str {
        match self {
            ImportSourceType::Csv => "csv",
            ImportSourceType::Ofx => "ofx",
            ImportSourceType::Qfx => "qfx",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "csv" => ImportSourceType::Csv,
            "ofx" => ImportSourceType::Ofx,
            "qfx" => ImportSourceType::Qfx,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown source_type '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportProfile {
    pub id: i64,
    pub name: String,
    pub account_id: Option<i64>,
    pub source_type: ImportSourceType,
    pub column_mapping: ColumnMapping,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ImportProfileInput {
    pub name: String,
    pub account_id: Option<i64>,
    pub source_type: ImportSourceType,
    pub column_mapping: ColumnMapping,
}

#[derive(Debug)]
pub enum ImportProfileError {
    Validation(String),
    Db(rusqlite::Error),
    NotFound(i64),
}

impl std::fmt::Display for ImportProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportProfileError::Validation(msg) => write!(f, "{msg}"),
            ImportProfileError::Db(e) => write!(f, "database error: {e}"),
            ImportProfileError::NotFound(id) => write!(f, "import profile {id} not found"),
        }
    }
}

impl From<rusqlite::Error> for ImportProfileError {
    fn from(e: rusqlite::Error) -> Self {
        ImportProfileError::Db(e)
    }
}

const COLUMNS: &str =
    "id, name, account_id, source_type, column_mapping_json, created_at, updated_at";

fn row_to_profile(row: &rusqlite::Row) -> rusqlite::Result<ImportProfile> {
    let mapping_json: String = row.get("column_mapping_json")?;
    let column_mapping: ColumnMapping = serde_json::from_str(&mapping_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(ImportProfile {
        id: row.get("id")?,
        name: row.get("name")?,
        account_id: row.get("account_id")?,
        source_type: ImportSourceType::from_sql_str(&row.get::<_, String>("source_type")?)?,
        column_mapping,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn validate(input: &ImportProfileInput) -> Result<(), ImportProfileError> {
    if input.name.trim().is_empty() {
        return Err(ImportProfileError::Validation(
            "Profile name must not be empty.".to_string(),
        ));
    }
    Ok(())
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<ImportProfile>, ImportProfileError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM import_profiles WHERE id = ?1"),
        params![id],
        row_to_profile,
    )
    .optional()
    .map_err(ImportProfileError::from)
}

pub fn create(
    conn: &Connection,
    input: &ImportProfileInput,
) -> Result<ImportProfile, ImportProfileError> {
    validate(input)?;
    let mapping_json = serde_json::to_string(&input.column_mapping).map_err(|e| {
        ImportProfileError::Db(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })?;
    let id = conn.query_row(
        "INSERT INTO import_profiles (name, account_id, source_type, column_mapping_json)
         VALUES (?1, ?2, ?3, ?4) RETURNING id",
        params![
            input.name.trim(),
            input.account_id,
            input.source_type.as_sql_str(),
            mapping_json,
        ],
        |row| row.get::<_, i64>(0),
    )?;
    get(conn, id)?.ok_or(ImportProfileError::NotFound(id))
}

/// Every profile, account-specific ones first - mirrors `rules::list`'s
/// existing global-vs-account-specific ordering. Matching one to an
/// incoming file is Import Engine v2's job, not this batch's.
pub fn list(conn: &Connection) -> Result<Vec<ImportProfile>, ImportProfileError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM import_profiles ORDER BY account_id IS NULL, name COLLATE NOCASE"
    ))?;
    let rows = stmt.query_map([], row_to_profile)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ImportProfileError::from)
}

pub fn update(
    conn: &Connection,
    id: i64,
    input: &ImportProfileInput,
) -> Result<ImportProfile, ImportProfileError> {
    validate(input)?;
    let mapping_json = serde_json::to_string(&input.column_mapping).map_err(|e| {
        ImportProfileError::Db(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })?;
    let affected = conn.execute(
        "UPDATE import_profiles SET name = ?1, account_id = ?2, source_type = ?3,
            column_mapping_json = ?4, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?5",
        params![
            input.name.trim(),
            input.account_id,
            input.source_type.as_sql_str(),
            mapping_json,
            id,
        ],
    )?;
    if affected == 0 {
        return Err(ImportProfileError::NotFound(id));
    }
    get(conn, id)?.ok_or(ImportProfileError::NotFound(id))
}

pub fn delete(conn: &Connection, id: i64) -> Result<(), ImportProfileError> {
    let affected = conn.execute("DELETE FROM import_profiles WHERE id = ?1", params![id])?;
    if affected == 0 {
        return Err(ImportProfileError::NotFound(id));
    }
    Ok(())
}

#[tauri::command]
pub fn list_import_profiles(state: State<DbState>) -> Result<Vec<ImportProfile>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_import_profile(
    state: State<DbState>,
    input: ImportProfileInput,
) -> Result<ImportProfile, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    create(&conn, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_import_profile(
    state: State<DbState>,
    id: i64,
    input: ImportProfileInput,
) -> Result<ImportProfile, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    update(&conn, id, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_import_profile(state: State<DbState>, id: i64) -> Result<(), String> {
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
        conn
    }

    fn sample_input() -> ImportProfileInput {
        ImportProfileInput {
            name: "Navy Federal Checking".to_string(),
            account_id: None,
            source_type: ImportSourceType::Csv,
            column_mapping: ColumnMapping {
                date_column: Some("Date".to_string()),
                posted_date_column: None,
                description_column: Some("Description".to_string()),
                amount_column: Some("Amount".to_string()),
                debit_column: None,
                credit_column: None,
                category_column: None,
            },
        }
    }

    #[test]
    fn create_and_get_round_trips_the_column_mapping() {
        let conn = test_conn();
        let created = create(&conn, &sample_input()).unwrap();
        assert_eq!(created.name, "Navy Federal Checking");
        assert_eq!(created.source_type, ImportSourceType::Csv);
        assert_eq!(created.column_mapping.date_column.as_deref(), Some("Date"));
        assert_eq!(
            created.column_mapping.amount_column.as_deref(),
            Some("Amount")
        );

        let fetched = get(&conn, created.id).unwrap().unwrap();
        assert_eq!(
            fetched.column_mapping.description_column,
            created.column_mapping.description_column
        );
    }

    #[test]
    fn create_rejects_empty_name() {
        let conn = test_conn();
        let mut input = sample_input();
        input.name = "  ".to_string();
        assert!(matches!(
            create(&conn, &input),
            Err(ImportProfileError::Validation(_))
        ));
    }

    #[test]
    fn all_three_source_types_round_trip() {
        let conn = test_conn();
        for t in [
            ImportSourceType::Csv,
            ImportSourceType::Ofx,
            ImportSourceType::Qfx,
        ] {
            let mut input = sample_input();
            input.source_type = t;
            input.name = format!("{t:?} profile");
            assert_eq!(create(&conn, &input).unwrap().source_type, t);
        }
    }

    #[test]
    fn account_specific_profiles_sort_before_global_ones() {
        let conn = test_conn();
        let account_id = accounts::create(
            &conn,
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

        let mut global = sample_input();
        global.name = "Global CSV".to_string();
        create(&conn, &global).unwrap();

        let mut specific = sample_input();
        specific.name = "Checking-specific".to_string();
        specific.account_id = Some(account_id);
        create(&conn, &specific).unwrap();

        let all = list(&conn).unwrap();
        assert_eq!(all[0].name, "Checking-specific");
        assert_eq!(all[1].name, "Global CSV");
    }

    #[test]
    fn update_and_delete_and_not_found_paths() {
        let conn = test_conn();
        let created = create(&conn, &sample_input()).unwrap();

        let mut changed = sample_input();
        changed.name = "Renamed Profile".to_string();
        assert_eq!(
            update(&conn, created.id, &changed).unwrap().name,
            "Renamed Profile"
        );

        delete(&conn, created.id).unwrap();
        assert!(get(&conn, created.id).unwrap().is_none());

        assert!(matches!(
            update(&conn, 999, &sample_input()),
            Err(ImportProfileError::NotFound(999))
        ));
        assert!(matches!(
            delete(&conn, 999),
            Err(ImportProfileError::NotFound(999))
        ));
    }
}
