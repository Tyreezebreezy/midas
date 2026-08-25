use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_transactions.sql"),
    include_str!("../migrations/0003_rules_and_recurring.sql"),
];

/// Tauri-managed state wrapping the single app-wide SQLite connection.
/// One mutex-guarded connection is deliberately simple: MIDAS is a local,
/// single-user desktop app with no meaningful write concurrency to pool for.
pub struct DbState(pub Mutex<Connection>);

/// Opens (creating if needed) the SQLite database at `path` and applies any
/// pending migrations. Safe to call every app startup: migrations only run
/// once each, tracked via SQLite's built-in `PRAGMA user_version`.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let current: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let target = MIGRATIONS.len() as i32;
    if current >= target {
        return Ok(());
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
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
}
