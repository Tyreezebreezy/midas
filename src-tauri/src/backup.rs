//! Phase 6: backup, export, and restore. A user must never be trapped
//! inside MIDAS to get at their own data - `create_backup` produces a
//! complete, restorable SQLite snapshot (via SQLite's own `VACUUM INTO`,
//! which itself refuses to overwrite an existing file); `export_csv`
//! (`transactions.rs`) gives a plain, spreadsheet-readable copy nothing
//! but a text editor is needed to open.
//!
//! Restore never touches the live database until a *separate* connection
//! to the candidate file has confirmed it actually looks like a MIDAS
//! database, at a schema version this build understands. Even then, the
//! previous live database is renamed aside, never deleted, before the
//! swap - a bad restore is always recoverable from disk.
//!
//! Both directions move file content over Tauri's ordinary IPC as a plain
//! command argument (base64 for the binary backup file, a plain string for
//! CSV) - the same zero-extra-capability pattern CSV import already
//! established (ADR-0005): no `fs` or `dialog` plugin capability is
//! granted for any of this.

use base64::Engine;
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};

use crate::db::{self, DbState, MigrationError};

const DB_FILENAME: &str = "midas.sqlite3";

/// Tables that must exist for a file to be treated as a MIDAS database,
/// not just "some SQLite file the user happened to pick." Intentionally a
/// short, structural check rather than a full schema diff - the point is
/// to catch "wrong file" mistakes with a clear message, not to duplicate
/// the migrations themselves.
const REQUIRED_TABLES: &[&str] = &["accounts", "transactions", "settings"];

#[derive(Debug)]
pub enum BackupError {
    Db(rusqlite::Error),
    Io(std::io::Error),
    NotAMidasDatabase(String),
    Migration(MigrationError),
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackupError::Db(e) => write!(f, "database error: {e}"),
            BackupError::Io(e) => write!(f, "file error: {e}"),
            BackupError::NotAMidasDatabase(reason) => {
                write!(f, "this doesn't look like a MIDAS backup file: {reason}")
            }
            BackupError::Migration(e) => write!(f, "{e}"),
        }
    }
}

impl From<rusqlite::Error> for BackupError {
    fn from(e: rusqlite::Error) -> Self {
        BackupError::Db(e)
    }
}
impl From<std::io::Error> for BackupError {
    fn from(e: std::io::Error) -> Self {
        BackupError::Io(e)
    }
}
impl From<MigrationError> for BackupError {
    fn from(e: MigrationError) -> Self {
        BackupError::Migration(e)
    }
}

/// A scratch file path unique enough that two concurrent calls (e.g. the
/// frontend firing `preview_restore` twice in a row before the first
/// response lands) never collide - `process::id()` alone is constant for
/// the app's whole lifetime, so it isn't enough on its own.
fn unique_scratch_path(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}.sqlite3", std::process::id()))
}

fn db_path(app: &AppHandle) -> Result<PathBuf, BackupError> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| BackupError::Io(std::io::Error::other(e.to_string())))?;
    Ok(dir.join(DB_FILENAME))
}

#[derive(Debug, Serialize)]
pub struct BackupResult {
    pub filename: String,
    pub content_base64: String,
    pub schema_version: i32,
    pub account_count: i64,
    pub transaction_count: i64,
}

/// A consistent, standalone SQLite snapshot. `VACUUM INTO` runs entirely
/// inside SQLite's own C code against a real filesystem path, so it can
/// never capture a half-written row the way copying the raw `.sqlite3`
/// file's bytes could while a write is in flight - and it refuses outright
/// if the target path already exists, so this can't silently overwrite
/// anything even at the SQLite level. The target here is always a scratch
/// path in the OS temp directory (never one the user chose), read back
/// into memory and deleted immediately after - the actual "save to disk"
/// happens on the frontend via a normal browser download.
pub fn create_backup(conn: &Connection) -> Result<BackupResult, BackupError> {
    let scratch = unique_scratch_path("midas-backup");
    let _ = std::fs::remove_file(&scratch);

    conn.execute("VACUUM INTO ?1", [scratch.to_string_lossy().as_ref()])?;
    let bytes = std::fs::read(&scratch)?;
    std::fs::remove_file(&scratch).ok();

    let schema_version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let account_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))?;
    let transaction_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM transactions", [], |row| row.get(0))?;
    let timestamp: String =
        conn.query_row("SELECT strftime('%Y-%m-%d-%H%M%S', 'now')", [], |row| {
            row.get(0)
        })?;

    Ok(BackupResult {
        filename: format!("midas-backup-{timestamp}.sqlite3"),
        content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        schema_version,
        account_count,
        transaction_count,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct RestoreValidation {
    pub schema_version: i32,
    pub account_count: i64,
    pub transaction_count: i64,
    pub needs_migration: bool,
}

/// Opens `path` as its own, separate connection - never the live app
/// connection - and confirms it actually looks like a MIDAS database
/// before anything is allowed to touch the real one.
fn validate_candidate(path: &Path) -> Result<RestoreValidation, BackupError> {
    let conn = Connection::open(path)?;

    let placeholders = REQUIRED_TABLES
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let table_count: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ({placeholders})"
        ),
        rusqlite::params_from_iter(REQUIRED_TABLES.iter()),
        |row| row.get(0),
    )?;
    if table_count != REQUIRED_TABLES.len() as i64 {
        return Err(BackupError::NotAMidasDatabase(
            "it doesn't have the tables MIDAS expects".to_string(),
        ));
    }

    let schema_version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let supported = db::current_schema_version();
    if schema_version > supported {
        return Err(BackupError::Migration(MigrationError::NewerSchema {
            found: schema_version,
            supported,
        }));
    }

    let account_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))?;
    let transaction_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM transactions", [], |row| row.get(0))?;

    Ok(RestoreValidation {
        schema_version,
        account_count,
        transaction_count,
        needs_migration: schema_version < supported,
    })
}

fn decode_to_scratch_file(content_base64: &str) -> Result<PathBuf, BackupError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content_base64)
        .map_err(|e| BackupError::NotAMidasDatabase(format!("corrupted file data ({e})")))?;
    let scratch = unique_scratch_path("midas-restore");
    std::fs::write(&scratch, bytes)?;
    Ok(scratch)
}

/// Validates a candidate backup without touching the live database at all -
/// the frontend calls this to show the user what they're about to restore
/// (account/transaction counts, schema version) before they confirm.
pub fn preview_restore(content_base64: &str) -> Result<RestoreValidation, BackupError> {
    let scratch = decode_to_scratch_file(content_base64)?;
    let result = validate_candidate(&scratch);
    std::fs::remove_file(&scratch).ok();
    result
}

/// Performs the actual restore: re-validates the candidate (never trusts a
/// prior preview call), then swaps it in as the live database at
/// `live_path`. The previous live database is renamed aside with a
/// timestamp, never deleted, so a bad restore is always recoverable from
/// disk - and nothing about the live database changes at all unless
/// validation passes.
pub fn restore_backup(
    live_path: &Path,
    db_state: &Mutex<Connection>,
    content_base64: &str,
) -> Result<RestoreValidation, BackupError> {
    let scratch = decode_to_scratch_file(content_base64)?;
    let validation = match validate_candidate(&scratch) {
        Ok(v) => v,
        Err(e) => {
            std::fs::remove_file(&scratch).ok();
            return Err(e);
        }
    };

    let mut conn_guard = db_state
        .lock()
        .map_err(|_| BackupError::Io(std::io::Error::other("database lock was poisoned")))?;

    // Drop the live connection before touching its file on disk (SQLite on
    // Windows holds the file open for as long as the connection lives).
    *conn_guard = Connection::open_in_memory()?;

    let timestamp: String =
        conn_guard.query_row("SELECT strftime('%Y-%m-%d-%H%M%S', 'now')", [], |row| {
            row.get(0)
        })?;
    if live_path.exists() {
        let safety_copy =
            live_path.with_file_name(format!("midas.sqlite3.before-restore-{timestamp}.bak"));
        std::fs::rename(live_path, &safety_copy)?;
    }

    std::fs::copy(&scratch, live_path)?;
    std::fs::remove_file(&scratch).ok();

    let restored = db::open(live_path)?; // re-applies migrate() if needed
    *conn_guard = restored;

    Ok(validation)
}

#[tauri::command]
pub fn create_backup_command(state: State<DbState>) -> Result<BackupResult, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    create_backup(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn preview_restore_command(content_base64: String) -> Result<RestoreValidation, String> {
    preview_restore(&content_base64).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn restore_backup_command(
    app: AppHandle,
    state: State<DbState>,
    content_base64: String,
) -> Result<RestoreValidation, String> {
    let live_path = db_path(&app).map_err(|e| e.to_string())?;
    restore_backup(&live_path, &state.0, &content_base64).map_err(|e| e.to_string())
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
        conn.execute_batch(include_str!("../migrations/0005_budgets.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../migrations/0006_savings_goals.sql"))
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
        conn.pragma_update(None, "user_version", 11).unwrap();
        conn
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

    #[test]
    fn create_backup_produces_a_valid_restorable_snapshot_with_correct_counts() {
        let conn = test_conn();
        let account = make_account(&conn);
        transactions::create(
            &conn,
            &TransactionInput {
                account_id: account,
                transaction_date: "2026-08-01".to_string(),
                posted_date: None,
                amount_cents: -1000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Coffee".to_string(),
                merchant: None,
                category: None,
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
            },
        )
        .unwrap();

        let backup = create_backup(&conn).unwrap();
        assert_eq!(backup.schema_version, 11);
        assert_eq!(backup.account_count, 1);
        assert_eq!(backup.transaction_count, 1);
        assert!(backup.filename.starts_with("midas-backup-"));
        assert!(backup.filename.ends_with(".sqlite3"));

        let validation = preview_restore(&backup.content_base64).unwrap();
        assert_eq!(validation.schema_version, 11);
        assert_eq!(validation.account_count, 1);
        assert_eq!(validation.transaction_count, 1);
        assert!(!validation.needs_migration);
    }

    #[test]
    fn preview_restore_rejects_a_file_that_is_not_a_midas_database() {
        let not_a_backup = base64::engine::general_purpose::STANDARD.encode(b"not a sqlite file");
        let err = preview_restore(&not_a_backup).unwrap_err();
        assert!(matches!(
            err,
            BackupError::Db(_) | BackupError::NotAMidasDatabase(_)
        ));
    }

    #[test]
    fn preview_restore_rejects_a_sqlite_file_missing_midas_tables() {
        let scratch = std::env::temp_dir().join(format!(
            "midas-test-not-midas-{}.sqlite3",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&scratch);
        let conn = Connection::open(&scratch).unwrap();
        conn.execute("CREATE TABLE unrelated (id INTEGER)", [])
            .unwrap();
        drop(conn);

        let bytes = std::fs::read(&scratch).unwrap();
        std::fs::remove_file(&scratch).ok();
        let content = base64::engine::general_purpose::STANDARD.encode(bytes);

        let err = preview_restore(&content).unwrap_err();
        assert!(matches!(err, BackupError::NotAMidasDatabase(_)));
    }

    #[test]
    fn preview_restore_rejects_a_backup_from_a_newer_schema_than_this_build_supports() {
        let conn = test_conn();
        conn.pragma_update(None, "user_version", db::current_schema_version() + 1)
            .unwrap();
        let scratch =
            std::env::temp_dir().join(format!("midas-test-newer-{}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&scratch);
        conn.execute("VACUUM INTO ?1", [scratch.to_string_lossy().as_ref()])
            .unwrap();
        let bytes = std::fs::read(&scratch).unwrap();
        std::fs::remove_file(&scratch).ok();
        let content = base64::engine::general_purpose::STANDARD.encode(bytes);

        let err = preview_restore(&content).unwrap_err();
        assert!(matches!(
            err,
            BackupError::Migration(MigrationError::NewerSchema { .. })
        ));
    }

    #[test]
    fn preview_restore_never_touches_any_live_state_it_is_pure_read_only_validation() {
        // Regression guard for "verify restore against a fresh/temporary
        // database, not merely by checking that a file was written" -
        // preview_restore must be safely callable any number of times with
        // identical results and no side effects, since the UI calls it
        // before the user has committed to anything.
        let conn = test_conn();
        let backup = create_backup(&conn).unwrap();

        for _ in 0..3 {
            let validation = preview_restore(&backup.content_base64).unwrap();
            assert_eq!(validation.schema_version, 11);
            assert_eq!(validation.account_count, 0);
        }
    }

    fn test_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("midas-backup-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn restore_backup_swaps_in_the_new_data_and_preserves_the_old_live_file() {
        let dir = test_dir("success");
        let live_path = dir.join(DB_FILENAME);

        // The "live" app database, with data that must NOT survive the restore.
        let live_seed = test_conn();
        make_account(&live_seed); // "old" account, name Checking
        std::fs::write(&live_path, {
            let scratch = dir.join("live-seed.sqlite3");
            let _ = std::fs::remove_file(&scratch);
            live_seed
                .execute("VACUUM INTO ?1", [scratch.to_string_lossy().as_ref()])
                .unwrap();
            std::fs::read(&scratch).unwrap()
        })
        .unwrap();
        let live_conn = Mutex::new(db::open(&live_path).unwrap());

        // The backup to restore, with a distinctly different account name.
        let backup_source = test_conn();
        accounts::create(
            &backup_source,
            &AccountInput {
                name: "Restored Savings".to_string(),
                institution: None,
                account_type: AccountType::Savings,
                include_in_net_worth: true,
                notes: None,
            },
        )
        .unwrap();
        let backup = create_backup(&backup_source).unwrap();

        let validation = restore_backup(&live_path, &live_conn, &backup.content_base64).unwrap();
        assert_eq!(validation.account_count, 1);

        let restored_conn = live_conn.lock().unwrap();
        let name: String = restored_conn
            .query_row("SELECT name FROM accounts LIMIT 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            name, "Restored Savings",
            "the live connection must now serve the restored data"
        );
        drop(restored_conn);

        let bak_files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bak"))
            .collect();
        assert_eq!(
            bak_files.len(),
            1,
            "the pre-restore live database must be preserved on disk, not deleted"
        );
        let bak_conn = Connection::open(bak_files[0].path()).unwrap();
        let old_name: String = bak_conn
            .query_row("SELECT name FROM accounts LIMIT 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            old_name, "Checking",
            "the .bak file must hold the pre-restore data"
        );

        // Every open handle on a file inside `dir` must be dropped before
        // cleanup - Windows refuses to delete a file (or its parent
        // directory) that's still open, unlike Unix, which allows removing
        // an open file. `.ok()` on the removal below would otherwise
        // silently mask exactly that and leave the directory behind.
        drop(bak_conn);
        drop(live_conn);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_backup_leaves_the_live_database_untouched_when_the_candidate_is_invalid() {
        let dir = test_dir("invalid");
        let live_path = dir.join(DB_FILENAME);

        let live_seed = test_conn();
        make_account(&live_seed);
        let scratch = dir.join("live-seed.sqlite3");
        live_seed
            .execute("VACUUM INTO ?1", [scratch.to_string_lossy().as_ref()])
            .unwrap();
        std::fs::copy(&scratch, &live_path).unwrap();
        let live_conn = Mutex::new(db::open(&live_path).unwrap());

        let garbage = base64::engine::general_purpose::STANDARD.encode(b"not a database");
        let err = restore_backup(&live_path, &live_conn, &garbage).unwrap_err();
        assert!(matches!(
            err,
            BackupError::Db(_) | BackupError::NotAMidasDatabase(_)
        ));

        let restored_conn = live_conn.lock().unwrap();
        let name: String = restored_conn
            .query_row("SELECT name FROM accounts LIMIT 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            name, "Checking",
            "the live connection must be completely untouched"
        );
        drop(restored_conn);

        let bak_files = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bak"))
            .count();
        assert_eq!(
            bak_files, 0,
            "an invalid candidate must never trigger the safety copy"
        );

        // See the matching comment in the success-path test above - Windows
        // won't remove a directory containing a file this process still has
        // open.
        drop(live_conn);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_backup_migrates_an_older_schema_backup_forward_to_current() {
        let dir = test_dir("older-schema");
        let live_path = dir.join(DB_FILENAME);

        let live_seed = test_conn();
        let scratch = dir.join("live-seed.sqlite3");
        live_seed
            .execute("VACUUM INTO ?1", [scratch.to_string_lossy().as_ref()])
            .unwrap();
        std::fs::copy(&scratch, &live_path).unwrap();
        let live_conn = Mutex::new(db::open(&live_path).unwrap());

        // A backup taken at an earlier schema version (migrations 1-3 only).
        let old_backup_conn = Connection::open_in_memory().unwrap();
        old_backup_conn
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        for sql in &crate::db::MIGRATIONS[0..3] {
            old_backup_conn.execute_batch(sql).unwrap();
        }
        old_backup_conn
            .pragma_update(None, "user_version", 3)
            .unwrap();
        let old_scratch = dir.join("old-backup.sqlite3");
        old_backup_conn
            .execute("VACUUM INTO ?1", [old_scratch.to_string_lossy().as_ref()])
            .unwrap();
        let content =
            base64::engine::general_purpose::STANDARD.encode(std::fs::read(&old_scratch).unwrap());

        let validation = restore_backup(&live_path, &live_conn, &content).unwrap();
        assert_eq!(validation.schema_version, 3);
        assert!(validation.needs_migration);

        let restored_conn = live_conn.lock().unwrap();
        let version: i32 = restored_conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            version,
            db::current_schema_version(),
            "restoring an older backup must migrate it forward, same as a normal upgrade"
        );

        // See the matching comment in the success-path test above.
        drop(restored_conn);
        drop(live_conn);
        std::fs::remove_dir_all(&dir).ok();
    }
}
