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
    include_str!("../migrations/0008_foundation_batch_1.sql"),
    include_str!("../migrations/0009_import_engine_v2.sql"),
    include_str!("../migrations/0010_ofx_source_adapter.sql"),
    include_str!("../migrations/0011_reconciliation.sql"),
    include_str!("../migrations/0012_transaction_intelligence.sql"),
    include_str!("../migrations/0013_recurring_intelligence.sql"),
    include_str!("../migrations/0014_budget_intelligence.sql"),
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

    /// A representative MIDAS 0.1.0 database (every table from migrations
    /// 1-7, one row each, realistic values including the opening_balance/
    /// balance_adjustment types Phase 4A/4C added) upgraded by migration
    /// 0008 (Foundation Batch 1). Every prior value must survive
    /// byte-identical, and every new column must default to a value that
    /// reproduces today's behavior for old rows (specs/phase-008,
    /// section 26 of the owner's handoff: "construct a representative v0.1
    /// database fixture and verify the upgrade path").
    #[test]
    fn migration_0008_preserves_a_representative_v0_1_0_database_and_defaults_new_columns_sanely() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in &MIGRATIONS[0..7] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 7).unwrap();
        assert_eq!(MIGRATIONS.len(), 14, "this test targets exactly the v0.1.0 -> Foundation Batch 1 boundary; update the slice above if more migrations are added before this one");

        conn.execute(
            "INSERT INTO accounts (id, name, institution, account_type, include_in_net_worth, notes)
             VALUES (1, 'Everyday Checking', 'Test Credit Union', 'checking', 1, 'primary account')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO accounts (id, name, account_type, include_in_net_worth) VALUES (2, 'Rewards Card', 'credit_card', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('theme', 'dark')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO imports (id, account_id, source_filename, file_hash, row_count, committed_count, duplicate_count, needs_review_count, column_mapping_json)
             VALUES (1, 1, 'august.csv', 'deadbeef', 10, 8, 1, 1, '{\"date_column\":\"Date\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transactions (id, account_id, transaction_date, amount_cents, transaction_type, raw_description, merchant, category, source_import_id, source_row_hash)
             VALUES (1, 1, '2026-08-05', -4500, 'purchase', 'SQ *COFFEE SHOP', 'Coffee Shop', 'Dining', 1, 'hash1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transactions (id, account_id, transaction_date, amount_cents, transaction_type, raw_description, merchant)
             VALUES (2, 1, '2026-08-01', 200000, 'income', 'PAYCHECK', 'Employer')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transactions (id, account_id, transaction_date, amount_cents, transaction_type, raw_description, merchant)
             VALUES (3, 1, '2026-07-31', 155000, 'opening_balance', 'Opening balance', 'Opening balance')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transactions (id, account_id, transaction_date, amount_cents, transaction_type, raw_description, merchant)
             VALUES (4, 1, '2026-08-20', 500, 'balance_adjustment', 'Balance adjustment', 'Balance adjustment')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO needs_review_items (id, issue_type, transaction_id, import_id, detected_reason)
             VALUES (1, 'suspected_duplicate', 1, 1, 'matches an existing transaction')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO merchant_rules (id, match_type, match_value, normalized_merchant, category, priority)
             VALUES (1, 'contains', 'coffee shop', 'Coffee Shop', 'Dining', 5)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO recurring_series (id, account_id, merchant, cadence, expected_amount_cents, next_expected_date, category, status)
             VALUES (1, 1, 'Netflix', 'monthly', 1549, '2026-09-17', 'Subscriptions', 'confirmed')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO budgets (id, category, month, planned_amount_cents, classification)
             VALUES (1, 'Dining', '2026-08-01', 30000, 'flexible')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO savings_goals (id, name, target_amount_cents, current_amount_cents, target_date, reserved_amount_cents)
             VALUES (1, 'Emergency Fund', 1000000, 420000, '2027-01-01', 20000)",
            [],
        )
        .unwrap();

        // Applies only migration 0008 directly (not the shared `migrate`,
        // which would also carry this fixture through every later migration
        // too) so this test stays isolated to exactly the 0007 -> 0008
        // boundary it names, regardless of how many migrations exist after
        // it.
        conn.execute_batch(MIGRATIONS[7]).unwrap();
        conn.pragma_update(None, "user_version", 8).unwrap();

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 8);

        // Every prior table's data is untouched.
        let (name, institution, notes): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT name, institution, notes FROM accounts WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(name, "Everyday Checking");
        assert_eq!(institution.as_deref(), Some("Test Credit Union"));
        assert_eq!(notes.as_deref(), Some("primary account"));

        let account_count: i64 = conn
            .query_row("SELECT count(*) FROM accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(account_count, 2);

        let (merchant, category, amount): (String, Option<String>, i64) = conn
            .query_row(
                "SELECT merchant, category, amount_cents FROM transactions WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(merchant, "Coffee Shop");
        assert_eq!(category.as_deref(), Some("Dining"));
        assert_eq!(amount, -4500);

        let transaction_count: i64 = conn
            .query_row("SELECT count(*) FROM transactions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(transaction_count, 4);

        let opening_balance_amount: i64 = conn
            .query_row(
                "SELECT amount_cents FROM transactions WHERE transaction_type = 'opening_balance'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(opening_balance_amount, 155000);

        let recurring_status: String = conn
            .query_row(
                "SELECT status FROM recurring_series WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(recurring_status, "confirmed");

        let goal_current: i64 = conn
            .query_row(
                "SELECT current_amount_cents FROM savings_goals WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(goal_current, 420000);

        // New transaction columns default to values that reproduce today's
        // behavior for every pre-existing row.
        let (payee_id, split_parent, external_id, reconciliation_status, is_one_time): (
            Option<i64>,
            Option<i64>,
            Option<String>,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT payee_id, split_parent_transaction_id, external_transaction_id, reconciliation_status, is_one_time
                 FROM transactions WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(payee_id, None);
        assert_eq!(split_parent, None);
        assert_eq!(external_id, None);
        assert_eq!(reconciliation_status, "unreconciled");
        assert_eq!(is_one_time, 0);

        // New imports columns default to 'csv' (every prior import really
        // was CSV) with no profile linkage.
        let (source_type, profile_id): (String, Option<i64>) = conn
            .query_row(
                "SELECT source_type, import_profile_id FROM imports WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(source_type, "csv");
        assert_eq!(profile_id, None);

        // New tables exist and start empty.
        for table in ["payees", "tags", "transaction_tags", "import_profiles"] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table} must exist and start empty");
        }
    }

    /// Import Engine v2 (specs/phase-009) adds three small columns to
    /// `imports` only - no new tables, no transaction columns. Same proof
    /// shape as the 0008 test above, scoped to the 0008 -> 0009 boundary.
    #[test]
    fn migration_0009_preserves_existing_imports_and_defaults_new_columns_sanely() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in &MIGRATIONS[0..8] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 8).unwrap();
        assert_eq!(MIGRATIONS.len(), 14, "this test targets exactly the Foundation Batch 1 -> Import Engine v2 boundary; update the slice above if more migrations are added before this one");

        conn.execute(
            "INSERT INTO accounts (id, name, account_type, include_in_net_worth) VALUES (1, 'Checking', 'checking', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO imports (id, account_id, source_filename, file_hash, row_count, committed_count, duplicate_count, needs_review_count, column_mapping_json)
             VALUES (1, 1, 'august.csv', 'deadbeef', 10, 8, 1, 1, '{\"date_column\":\"Date\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO import_profiles (id, name, source_type, column_mapping_json)
             VALUES (1, 'My Bank', 'csv', '{\"date_column\":\"Date\"}')",
            [],
        )
        .unwrap();

        // Applies only migration 0009 directly (not the shared `migrate`,
        // which would also carry this fixture through every later
        // migration) so this test stays isolated to exactly the 0008 ->
        // 0009 boundary it names, regardless of how many migrations exist
        // after it.
        conn.execute_batch(MIGRATIONS[8]).unwrap();
        conn.pragma_update(None, "user_version", 9).unwrap();

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 9);

        let (filename, committed_count): (String, i64) = conn
            .query_row(
                "SELECT source_filename, committed_count FROM imports WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(filename, "august.csv");
        assert_eq!(committed_count, 8);

        let (excluded_count, undone_at, undone_removed_count): (i64, Option<String>, Option<i64>) =
            conn.query_row(
                "SELECT excluded_count, undone_at, undone_removed_count FROM imports WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(excluded_count, 0);
        assert_eq!(undone_at, None);
        assert_eq!(undone_removed_count, None);

        let profile_is_archived: i64 = conn
            .query_row(
                "SELECT is_archived FROM import_profiles WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(profile_is_archived, 0);
    }

    /// The OFX/QFX source adapter (specs/phase-010) adds one new table
    /// (`ofx_account_mappings`) and two nullable columns on `imports` -
    /// same proof shape as the 0008/0009 tests above, scoped to the 0009
    /// -> 0010 boundary.
    #[test]
    fn migration_0010_preserves_existing_data_and_defaults_new_columns_sanely() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in &MIGRATIONS[0..9] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 9).unwrap();
        assert_eq!(MIGRATIONS.len(), 14, "this test targets exactly the Import Engine v2 -> OFX source adapter boundary; update the slice above if more migrations are added before this one");

        conn.execute(
            "INSERT INTO accounts (id, name, account_type, include_in_net_worth) VALUES (1, 'Checking', 'checking', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO imports (id, account_id, source_filename, file_hash, row_count, committed_count, duplicate_count, needs_review_count, column_mapping_json, source_type)
             VALUES (1, 1, 'august.csv', 'deadbeef', 10, 8, 1, 1, '{}', 'csv')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        // Unlike the 0008/0009/0011 tests, this one runs the full `migrate`
        // chain rather than one isolated migration, so it lands on
        // whatever the current total is, not a fixed boundary - asserted
        // dynamically so a later migration never has to touch this line.
        assert_eq!(version, MIGRATIONS.len() as i32);

        let (filename, committed_count): (String, i64) = conn
            .query_row(
                "SELECT source_filename, committed_count FROM imports WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(filename, "august.csv");
        assert_eq!(committed_count, 8);

        let (balance_cents, balance_date): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT statement_balance_cents, statement_balance_date FROM imports WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(balance_cents, None);
        assert_eq!(balance_date, None);

        let mapping_count: i64 = conn
            .query_row("SELECT count(*) FROM ofx_account_mappings", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            mapping_count, 0,
            "ofx_account_mappings must exist and start empty"
        );
    }

    /// Account reconciliation (specs/phase-011) adds one new table
    /// (`reconciliations`), one nullable column on `transactions`
    /// (`reconciliation_id`), and two nullable columns on `imports`
    /// (`statement_start_date`/`statement_end_date`) - same proof shape as
    /// the 0008/0009/0010 tests above, scoped to the 0010 -> 0011 boundary.
    #[test]
    fn migration_0011_preserves_existing_data_and_defaults_new_columns_sanely() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in &MIGRATIONS[0..10] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 10).unwrap();

        conn.execute(
            "INSERT INTO accounts (id, name, account_type, include_in_net_worth) VALUES (1, 'Checking', 'checking', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO imports (id, account_id, source_filename, file_hash, row_count, committed_count, duplicate_count, needs_review_count, column_mapping_json, source_type, statement_balance_cents, statement_balance_date)
             VALUES (1, 1, 'august.qfx', 'deadbeef', 10, 8, 1, 1, '{}', 'qfx', 421819, '2026-08-31')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transactions (id, account_id, transaction_date, amount_cents, transaction_type, raw_description, merchant, source_import_id)
             VALUES (1, 1, '2026-08-05', -4500, 'purchase', 'Coffee', 'Coffee', 1)",
            [],
        )
        .unwrap();

        conn.execute_batch(MIGRATIONS[10]).unwrap();
        conn.pragma_update(None, "user_version", 11).unwrap();

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 11);

        // Existing rows are untouched.
        let (merchant, amount): (String, i64) = conn
            .query_row(
                "SELECT merchant, amount_cents FROM transactions WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(merchant, "Coffee");
        assert_eq!(amount, -4500);
        let statement_balance: i64 = conn
            .query_row(
                "SELECT statement_balance_cents FROM imports WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(statement_balance, 421819);

        // New columns default to NULL for every pre-existing row.
        let reconciliation_id: Option<i64> = conn
            .query_row(
                "SELECT reconciliation_id FROM transactions WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reconciliation_id, None);
        let (start, end): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT statement_start_date, statement_end_date FROM imports WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(start, None);
        assert_eq!(end, None);

        let reconciliation_count: i64 = conn
            .query_row("SELECT count(*) FROM reconciliations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            reconciliation_count, 0,
            "reconciliations must exist and start empty"
        );
    }

    /// Transaction Intelligence (specs/phase-012) adds exactly one new
    /// table, `transaction_splits` - no columns touched on any existing
    /// table (payee_id/is_one_time already existed from Foundation Batch 1;
    /// this batch only makes them settable in application code). Same proof
    /// shape as the migrations above, scoped to the 0011 -> 0012 boundary.
    #[test]
    fn migration_0012_preserves_existing_data_and_adds_only_the_splits_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in &MIGRATIONS[0..11] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 11).unwrap();
        assert_eq!(MIGRATIONS.len(), 14, "this test targets exactly the Reconciliation -> Transaction Intelligence boundary; update the slice above if more migrations are added before this one");

        conn.execute(
            "INSERT INTO accounts (id, name, account_type, include_in_net_worth) VALUES (1, 'Checking', 'checking', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transactions (id, account_id, transaction_date, amount_cents, transaction_type, raw_description, merchant)
             VALUES (1, 1, '2026-08-05', -12000, 'purchase', 'Walmart', 'Walmart')",
            [],
        )
        .unwrap();

        conn.execute_batch(MIGRATIONS[11]).unwrap();
        conn.pragma_update(None, "user_version", 12).unwrap();

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 12);

        let (merchant, amount): (String, i64) = conn
            .query_row(
                "SELECT merchant, amount_cents FROM transactions WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(merchant, "Walmart");
        assert_eq!(amount, -12000);

        let split_count: i64 = conn
            .query_row("SELECT count(*) FROM transaction_splits", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            split_count, 0,
            "transaction_splits must exist and start empty"
        );
    }

    /// Recurring Intelligence (specs/phase-014) rebuilds `recurring_series`
    /// (new payee_id/direction/kind/semimonthly/planning_to_cancel columns,
    /// an expanded cadence/status CHECK) and adds `recurring_occurrences` -
    /// same proof shape as the migrations above, scoped to the 0012 -> 0013
    /// boundary. A pre-existing series (income, positive amount, exactly
    /// the shape of the 0008 fixture's Netflix row but flipped positive to
    /// prove the sign-based direction backfill) must survive with its new
    /// columns defaulted sanely.
    #[test]
    fn migration_0013_preserves_existing_recurring_series_and_backfills_direction() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in &MIGRATIONS[0..12] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 12).unwrap();
        assert_eq!(MIGRATIONS.len(), 14, "this test targets exactly the Transaction Intelligence -> Recurring Intelligence boundary; update the slice above if more migrations are added before this one");

        conn.execute(
            "INSERT INTO accounts (id, name, account_type, include_in_net_worth) VALUES (1, 'Checking', 'checking', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO recurring_series (id, account_id, merchant, cadence, expected_amount_cents, next_expected_date, category, status)
             VALUES (1, 1, 'Employer Payroll', 'biweekly', 190000, '2026-09-25', NULL, 'confirmed')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO recurring_series (id, account_id, merchant, cadence, expected_amount_cents, next_expected_date, category, status)
             VALUES (2, 1, 'Netflix', 'monthly', -1549, '2026-09-17', 'Subscriptions', 'proposed')",
            [],
        )
        .unwrap();

        conn.execute_batch(MIGRATIONS[12]).unwrap();
        conn.pragma_update(None, "user_version", 13).unwrap();

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 13);

        let (merchant, cadence, amount, status, direction, kind, payee_id, planning_to_cancel): (
            String,
            String,
            i64,
            String,
            String,
            String,
            Option<i64>,
            i64,
        ) = conn
            .query_row(
                "SELECT merchant, cadence, expected_amount_cents, status, direction, kind, payee_id, planning_to_cancel
                 FROM recurring_series WHERE id = 1",
                [],
                |r| {
                    Ok((
                        r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?,
                        r.get(6)?, r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(merchant, "Employer Payroll");
        assert_eq!(cadence, "biweekly");
        assert_eq!(amount, 190000);
        assert_eq!(status, "confirmed");
        assert_eq!(
            direction, "income",
            "a positive pre-existing series backfills to income"
        );
        assert_eq!(
            kind, "other",
            "new kind column defaults sanely for old rows"
        );
        assert_eq!(payee_id, None);
        assert_eq!(planning_to_cancel, 0);

        let netflix_direction: String = conn
            .query_row(
                "SELECT direction FROM recurring_series WHERE id = 2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            netflix_direction, "expense",
            "a negative pre-existing series backfills to expense"
        );

        let occurrence_count: i64 = conn
            .query_row("SELECT count(*) FROM recurring_occurrences", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            occurrence_count, 0,
            "recurring_occurrences must exist and start empty"
        );
    }

    /// Budget Intelligence (specs/phase-015) rebuilds `budgets` to widen its
    /// `classification` CHECK (adds 'discretionary') and add a
    /// `rollover_enabled` flag - same proof shape as the migrations above,
    /// scoped to the 0013 -> 0014 boundary. A pre-existing budget row must
    /// survive with its own classification untouched and rollover
    /// defaulted off.
    #[test]
    fn migration_0014_preserves_existing_budgets_and_defaults_rollover_off() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in &MIGRATIONS[0..13] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 13).unwrap();
        assert_eq!(MIGRATIONS.len(), 14, "this test targets exactly the Recurring Intelligence -> Budget Intelligence boundary; update the slice above if more migrations are added before this one");

        conn.execute(
            "INSERT INTO budgets (id, category, month, planned_amount_cents, classification)
             VALUES (1, 'Groceries', '2026-08-01', 45000, 'fixed')",
            [],
        )
        .unwrap();

        conn.execute_batch(MIGRATIONS[13]).unwrap();
        conn.pragma_update(None, "user_version", 14).unwrap();

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 14);

        let (category, month, planned, classification, rollover_enabled): (
            String,
            String,
            i64,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT category, month, planned_amount_cents, classification, rollover_enabled
                 FROM budgets WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(category, "Groceries");
        assert_eq!(month, "2026-08-01");
        assert_eq!(planned, 45000);
        assert_eq!(
            classification, "fixed",
            "an existing classification must survive untouched"
        );
        assert_eq!(
            rollover_enabled, 0,
            "new rollover flag defaults off for old rows"
        );

        // The widened CHECK constraint must actually accept the new value.
        conn.execute(
            "INSERT INTO budgets (category, month, planned_amount_cents, classification, rollover_enabled)
             VALUES ('Dining', '2026-08-01', 30000, 'discretionary', 1)",
            [],
        )
        .unwrap();
    }
}
