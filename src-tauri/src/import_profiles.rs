//! Named, reusable CSV/OFX/QFX column-mapping memory per institution/account.
//! Storage and CRUD landed in Foundation Batch 1 (specs/phase-008); matching
//! a profile to an incoming file (`recognize`, below) is Import Engine v2
//! (specs/phase-009). `account_id: None` means the profile applies to any
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
    pub(crate) fn as_sql_str(self) -> &'static str {
        match self {
            ImportSourceType::Csv => "csv",
            ImportSourceType::Ofx => "ofx",
            ImportSourceType::Qfx => "qfx",
        }
    }

    pub(crate) fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
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
    pub is_archived: bool,
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
    "id, name, account_id, source_type, column_mapping_json, is_archived, created_at, updated_at";

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
        is_archived: row.get::<_, i64>("is_archived")? != 0,
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
/// existing global-vs-account-specific ordering. Archived profiles are
/// excluded by default, matching every other archivable model in this
/// codebase (`accounts::list`, `payees::list`).
pub fn list(
    conn: &Connection,
    include_archived: bool,
) -> Result<Vec<ImportProfile>, ImportProfileError> {
    let sql = if include_archived {
        format!("SELECT {COLUMNS} FROM import_profiles ORDER BY account_id IS NULL, name COLLATE NOCASE")
    } else {
        format!("SELECT {COLUMNS} FROM import_profiles WHERE is_archived = 0 ORDER BY account_id IS NULL, name COLLATE NOCASE")
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_profile)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ImportProfileError::from)
}

pub fn set_archived(
    conn: &Connection,
    id: i64,
    archived: bool,
) -> Result<ImportProfile, ImportProfileError> {
    let affected = conn.execute(
        "UPDATE import_profiles SET is_archived = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?2",
        params![archived as i64, id],
    )?;
    if affected == 0 {
        return Err(ImportProfileError::NotFound(id));
    }
    get(conn, id)?.ok_or(ImportProfileError::NotFound(id))
}

/// Conservative compatibility check (product handoff section 8): every
/// column name the profile depends on (`ColumnMapping::required_headers`)
/// must be verbatim present in the new file's own header row. No fuzzy
/// matching, no partial credit - a profile missing even one of its expected
/// columns is not a match. A profile with nothing mapped can never match -
/// that's not a real profile the wizard would have offered to save, since
/// saving requires a usable mapping first.
fn is_compatible(profile: &ImportProfile, headers: &[String]) -> bool {
    let required = profile.column_mapping.required_headers();
    !required.is_empty()
        && required
            .iter()
            .all(|needed| headers.iter().any(|h| h == needed))
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ProfileRecognition {
    Clear { profile: Box<ImportProfile> },
    Ambiguous { profiles: Vec<ImportProfile> },
    None,
}

/// Conservative profile recognition (product handoff section 8): suggests a
/// saved profile only when it is unambiguous, and never reinterprets a file
/// on a weak signal. A profile scoped to a specific account (`account_id:
/// Some`) is never even considered for a different account - that scoping
/// is the user's own explicit statement that the mapping is for that
/// account only, so a header-only match elsewhere would be exactly the kind
/// of weak signal this function must not act on. Among profiles usable for
/// this account (global, or scoped to it), a tie beyond one compatible
/// match is surfaced to the user rather than silently picked.
pub fn recognize(
    profiles: &[ImportProfile],
    headers: &[String],
    account_id: Option<i64>,
) -> ProfileRecognition {
    let usable_for_account: Vec<&ImportProfile> = profiles
        .iter()
        .filter(|p| p.account_id.is_none() || p.account_id == account_id)
        .collect();
    let compatible: Vec<&ImportProfile> = usable_for_account
        .into_iter()
        .filter(|p| is_compatible(p, headers))
        .collect();
    if compatible.is_empty() {
        return ProfileRecognition::None;
    }
    if compatible.len() == 1 {
        return ProfileRecognition::Clear {
            profile: Box::new(compatible[0].clone()),
        };
    }

    // Still ambiguous: prefer a profile scoped exactly to this account over
    // an equally-compatible global one, if that narrows it to one.
    let same_account: Vec<&ImportProfile> = compatible
        .iter()
        .copied()
        .filter(|p| account_id.is_some() && p.account_id == account_id)
        .collect();
    if same_account.len() == 1 {
        return ProfileRecognition::Clear {
            profile: Box::new(same_account[0].clone()),
        };
    }
    let ambiguous = if !same_account.is_empty() {
        same_account
    } else {
        compatible
    };
    ProfileRecognition::Ambiguous {
        profiles: ambiguous.into_iter().cloned().collect(),
    }
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
pub fn list_import_profiles(
    state: State<DbState>,
    include_archived: bool,
) -> Result<Vec<ImportProfile>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list(&conn, include_archived).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_import_profile_archived(
    state: State<DbState>,
    id: i64,
    archived: bool,
) -> Result<ImportProfile, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    set_archived(&conn, id, archived).map_err(|e| e.to_string())
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
                external_id_column: None,
                invert_amount: false,
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

        let all = list(&conn, false).unwrap();
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

    #[test]
    fn set_archived_toggles_and_is_excluded_from_list_by_default() {
        let conn = test_conn();
        let created = create(&conn, &sample_input()).unwrap();

        let archived = set_archived(&conn, created.id, true).unwrap();
        assert!(archived.is_archived);
        assert!(list(&conn, false).unwrap().is_empty());
        assert_eq!(list(&conn, true).unwrap().len(), 1);

        let restored = set_archived(&conn, created.id, false).unwrap();
        assert!(!restored.is_archived);
        assert_eq!(list(&conn, false).unwrap().len(), 1);

        assert!(matches!(
            set_archived(&conn, 999, true),
            Err(ImportProfileError::NotFound(999))
        ));
    }

    #[test]
    fn a_profile_used_by_an_import_cannot_be_deleted_but_can_be_archived() {
        // Found via native verification: once a profile has been used for
        // an import, imports.import_profile_id's foreign key makes hard
        // delete impossible - archiving is the only way to retire it.
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
        let profile = create(&conn, &sample_input()).unwrap();
        conn.execute(
            "INSERT INTO imports (account_id, source_filename, file_hash, row_count, committed_count, duplicate_count, needs_review_count, column_mapping_json, import_profile_id)
             VALUES (?1, 'test.csv', 'hash', 1, 1, 0, 0, '{}', ?2)",
            params![account_id, profile.id],
        )
        .unwrap();

        assert!(matches!(
            delete(&conn, profile.id),
            Err(ImportProfileError::Db(_))
        ));
        assert!(set_archived(&conn, profile.id, true).is_ok());
        assert!(list(&conn, false).unwrap().is_empty());
    }

    fn headers(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn recognize_is_clear_when_exactly_one_profile_is_compatible() {
        let conn = test_conn();
        let profile = create(&conn, &sample_input()).unwrap(); // Date, Description, Amount
        let all = list(&conn, false).unwrap();

        let result = recognize(&all, &headers(&["Date", "Description", "Amount"]), None);
        assert!(matches!(
            result,
            ProfileRecognition::Clear { profile: p } if p.id == profile.id
        ));

        // Extra columns in the file beyond what the profile needs are fine.
        let result = recognize(
            &all,
            &headers(&["Date", "Description", "Amount", "Balance"]),
            None,
        );
        assert!(matches!(result, ProfileRecognition::Clear { .. }));
    }

    #[test]
    fn recognize_is_none_when_a_required_column_is_missing() {
        let conn = test_conn();
        create(&conn, &sample_input()).unwrap();
        let all = list(&conn, false).unwrap();

        // No "Amount" column - the profile cannot be applied as-is.
        let result = recognize(&all, &headers(&["Date", "Description"]), None);
        assert!(matches!(result, ProfileRecognition::None));
    }

    #[test]
    fn recognize_is_ambiguous_with_two_equally_compatible_global_profiles() {
        let conn = test_conn();
        let mut second = sample_input();
        second.name = "Second Bank".to_string();
        create(&conn, &sample_input()).unwrap();
        create(&conn, &second).unwrap();
        let all = list(&conn, false).unwrap();

        let result = recognize(&all, &headers(&["Date", "Description", "Amount"]), None);
        match result {
            ProfileRecognition::Ambiguous { profiles } => assert_eq!(profiles.len(), 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn recognize_prefers_the_profile_scoped_to_the_selected_account() {
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
        let specific = create(&conn, &specific).unwrap();

        let all = list(&conn, false).unwrap();
        let result = recognize(
            &all,
            &headers(&["Date", "Description", "Amount"]),
            Some(account_id),
        );
        assert!(matches!(
            result,
            ProfileRecognition::Clear { profile: p } if p.id == specific.id
        ));
    }

    #[test]
    fn recognize_returns_none_with_no_saved_profiles() {
        assert!(matches!(
            recognize(&[], &headers(&["Date"]), None),
            ProfileRecognition::None
        ));
    }

    #[test]
    fn recognize_never_suggests_a_profile_scoped_to_a_different_account() {
        // Found via native verification: a profile scoped to one account is
        // the user's own explicit statement that it's for that account only
        // - matching headers alone must never be enough to suggest it for
        // an unrelated account, even when it's the only compatible profile
        // that exists at all.
        let conn = test_conn();
        let account_a = accounts::create(
            &conn,
            &AccountInput {
                name: "Account A".to_string(),
                institution: None,
                account_type: AccountType::Checking,
                include_in_net_worth: true,
                notes: None,
            },
        )
        .unwrap()
        .id;
        let account_b = accounts::create(
            &conn,
            &AccountInput {
                name: "Account B".to_string(),
                institution: None,
                account_type: AccountType::Checking,
                include_in_net_worth: true,
                notes: None,
            },
        )
        .unwrap()
        .id;

        let mut scoped = sample_input();
        scoped.account_id = Some(account_a);
        create(&conn, &scoped).unwrap();

        let all = list(&conn, false).unwrap();
        let result = recognize(
            &all,
            &headers(&["Date", "Description", "Amount"]),
            Some(account_b),
        );
        assert!(
            matches!(result, ProfileRecognition::None),
            "a profile scoped to account A must not be suggested for account B"
        );

        // The same profile IS recognized for the account it's actually scoped to.
        let result_correct_account = recognize(
            &all,
            &headers(&["Date", "Description", "Amount"]),
            Some(account_a),
        );
        assert!(matches!(
            result_correct_account,
            ProfileRecognition::Clear { .. }
        ));
    }
}
