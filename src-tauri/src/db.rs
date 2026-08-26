use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub(crate) const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_transactions.sql"),
    include_str!("../migrations/0003_rules_and_recurring.sql"),
    include_str!("../migrations/0004_accounting_boundary.sql"),
    include_str!("../migrations/0005_budgets.sql"),
    include_str!("../migrations/0006_savings_goals.sql"),
    include_str!("../migrations/0007_balance_adjustment.sql"),
];

/// The schema version this build knows how to read and write - always
/// `MIGRATIONS.len()`. Exposed so `backup.rs` can tell a restore candidate
/// apart from a database this build doesn't understand yet, using the same
/// number `migrate` itself checks against.
pub fn current_schema_version() -> i32 {
    MIGRATIONS.len() as i32
}

#[derive(Debug)]
pub enum MigrationError {
    Db(rusqlite::Error),
    /// The database's own `PRAGMA user_version` is higher than this
    /// build's `MIGRATIONS.len()` - it was created or migrated by a newer
    /// MIDAS. Proceeding would mean reading/writing a schema this build
    /// doesn't actually know the shape of (new columns, new constraints),
    /// silently, since SQLite has no way to detect that on its own -
    /// opening it here is refused rather than risking that.
    NewerSchema {
        found: i32,
        supported: i32,
    },
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationError::Db(e) => write!(f, "database error: {e}"),
            MigrationError::NewerSchema { found, supported } => write!(
                f,
                "this database (schema {found}) was created by a newer version of MIDAS than this one supports (schema {supported}) - update MIDAS before opening it"
            ),
        }
    }
}

impl std::error::Error for MigrationError {}

impl From<rusqlite::Error> for MigrationError {
    fn from(e: rusqlite::Error) -> Self {
        MigrationError::Db(e)
    }
}

/// Tauri-managed state wrapping the single app-wide SQLite connection.
/// One mutex-guarded connection is deliberately simple: MIDAS is a local,
/// single-user desktop app with no meaningful write concurrency to pool for.
pub struct DbState(pub Mutex<Connection>);

/// Opens (creating if needed) the SQLite database at `path` and applies any
/// pending migrations. Safe to call every app startup: migrations only run
/// once each, tracked via SQLite's built-in `PRAGMA user_version`.
pub fn open(path: &Path) -> Result<Connection, MigrationError> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<(), MigrationError> {
    migrate_with(conn, MIGRATIONS)
}

/// The real body of `migrate`, parameterized on the migration list so
/// tests can exercise a deliberately-broken migration without touching the
/// real schema - `migrate` itself always calls this with `MIGRATIONS`.
fn migrate_with(conn: &Connection, migrations: &[&str]) -> Result<(), MigrationError> {
    let current: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let target = migrations.len() as i32;
    if current > target {
        return Err(MigrationError::NewerSchema {
            found: current,
            supported: target,
        });
    }
    if current == target {
        return Ok(());
    }
    for (i, sql) in migrations.iter().enumerate().skip(current as usize) {
        let version = i as i32 + 1;
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn migrate_creates_expected_tables() {
        let conn = memory_conn();
        let table_count: i32 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ('accounts', 'settings')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 2);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = memory_conn();
        // Running again must not error or reapply (would fail on CREATE TABLE
        // if it did, since the tables already exist).
        migrate(&conn).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i32);
    }

    #[test]
    fn open_creates_file_and_applies_migrations() {
        let dir = std::env::temp_dir().join(format!("midas-db-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&path);

        let conn = open(&path).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i32);
        drop(conn);

        assert!(path.exists());
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn migrate_rejects_a_database_from_a_newer_schema_version() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", MIGRATIONS.len() as i32 + 1)
            .unwrap();

        let err = migrate(&conn).unwrap_err();
        assert!(matches!(
            err,
            MigrationError::NewerSchema { found, supported }
                if found == MIGRATIONS.len() as i32 + 1 && supported == MIGRATIONS.len() as i32
        ));
    }

    #[test]
    fn migrate_resumes_from_a_partial_upgrade_and_preserves_existing_data() {
        // Representative "earlier MIDAS schema" upgrade: a real installation
        // that only ever got as far as migration 3 (rules and recurring),
        // with real data already in it, now opened by a build carrying all
        // seven migrations.
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in &MIGRATIONS[0..3] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 3).unwrap();

        conn.execute(
            "INSERT INTO accounts (id, name, account_type) VALUES (1, 'Old Checking', 'checking')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transactions (id, account_id, transaction_date, amount_cents, transaction_type, raw_description, merchant)
             VALUES (1, 1, '2026-01-01', -500, 'purchase', 'Coffee', 'Coffee')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i32);

        let (name, merchant, amount): (String, String, i64) = conn
            .query_row(
                "SELECT a.name, t.merchant, t.amount_cents FROM accounts a JOIN transactions t ON t.account_id = a.id WHERE a.id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(name, "Old Checking");
        assert_eq!(merchant, "Coffee");
        assert_eq!(amount, -500);
    }

    #[test]
    fn a_failed_migration_leaves_the_database_at_its_last_good_version_not_partially_applied() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        let broken_migrations: Vec<&str> = vec![
            MIGRATIONS[0],
            MIGRATIONS[1],
            "CREATE TABLE partially_created (id INTEGER); THIS IS NOT VALID SQL;",
        ];

        let err = migrate_with(&conn, &broken_migrations).unwrap_err();
        assert!(matches!(err, MigrationError::Db(_)));

        // The failed migration's own transaction must have rolled back -
        // user_version stays at the last successfully-committed migration,
        // not incremented for the one that failed partway through.
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);

        // Nothing from the broken migration's statement leaked through.
        let leaked: i32 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'partially_created'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(leaked, 0);

        // Retrying with the real migrations from here must still work -
        // a failed migration must not have left the database in a state
        // the real chain can no longer resume from.
        migrate(&conn).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i32);
    }
}
