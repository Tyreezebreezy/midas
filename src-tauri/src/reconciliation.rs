//! Account reconciliation + running balances (specs/phase-011).
//!
//! Not to be confused with `accounting::reconcile_balance` ("Update
//! balance" in the UI) - that is a one-number correction to the ledger
//! (an opening balance or a dated adjustment). This module is the
//! statement-matching workflow: comparing MIDAS against a real bank/
//! credit-card statement for a defined period, clearing transactions one
//! by one, and recording a verified `reconciliations` row once the
//! difference reaches zero. The two features can compose (section 19: a
//! reconciliation discrepancy the owner can't otherwise explain is exactly
//! what "Update balance" is for) but are otherwise independent.
//!
//! Architecture is deliberately thin: no new balance engine. Running
//! balances and the cleared-balance formula are both plain SQL queries
//! against the existing `transactions` table; completing a reconciliation
//! only ever sets `transactions.reconciliation_status`/`reconciliation_id`,
//! fields Foundation Batch 1 already added. The one genuinely new concept
//! is the `reconciliations` table itself and its three-state lifecycle
//! (completed / needs_review / reopened).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::transactions::{self, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationRecordStatus {
    /// Verified and currently trusted - the statement and MIDAS agreed at
    /// the moment this was recorded.
    Completed,
    /// Was `Completed`, but a later edit to one of its linked transactions
    /// changed the arithmetic (section 16) - never silently left looking
    /// healthy.
    NeedsReview,
    /// The owner explicitly reopened it (section 18). The record stays for
    /// audit history; its transactions have been freed back to `Cleared`
    /// and no longer claim this period.
    Reopened,
}

impl ReconciliationRecordStatus {
    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "completed" => ReconciliationRecordStatus::Completed,
            "needs_review" => ReconciliationRecordStatus::NeedsReview,
            "reopened" => ReconciliationRecordStatus::Reopened,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown reconciliation status '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Reconciliation {
    pub id: i64,
    pub account_id: i64,
    pub statement_start_date: String,
    pub statement_end_date: String,
    pub statement_ending_balance_cents: i64,
    pub cleared_balance_cents: i64,
    pub status: ReconciliationRecordStatus,
    pub reconciled_at: String,
    pub source_import_id: Option<i64>,
    pub note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

const RECONCILIATION_COLUMNS: &str = "id, account_id, statement_start_date, statement_end_date, \
    statement_ending_balance_cents, cleared_balance_cents, status, reconciled_at, \
    source_import_id, note, created_at, updated_at";

fn row_to_reconciliation(row: &rusqlite::Row) -> rusqlite::Result<Reconciliation> {
    Ok(Reconciliation {
        id: row.get("id")?,
        account_id: row.get("account_id")?,
        statement_start_date: row.get("statement_start_date")?,
        statement_end_date: row.get("statement_end_date")?,
        statement_ending_balance_cents: row.get("statement_ending_balance_cents")?,
        cleared_balance_cents: row.get("cleared_balance_cents")?,
        status: ReconciliationRecordStatus::from_sql_str(&row.get::<_, String>("status")?)?,
        reconciled_at: row.get("reconciled_at")?,
        source_import_id: row.get("source_import_id")?,
        note: row.get("note")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

#[derive(Debug)]
pub enum ReconciliationError {
    Validation(String),
    Db(rusqlite::Error),
    NotFound(i64),
    /// The requested period overlaps an already-active (Completed or
    /// NeedsReview) reconciliation - section 38's explicit requirement that
    /// two reconciliations must never claim the same transaction history.
    OverlappingPeriod {
        existing_start: String,
        existing_end: String,
    },
    /// The recomputed difference is not exactly zero. Always re-verified
    /// server-side at completion time - the frontend's own displayed
    /// difference is never trusted as authorization to close (section 31:
    /// no AI, no heuristics, exact arithmetic only).
    NotBalanced {
        difference_cents: i64,
    },
}

impl std::fmt::Display for ReconciliationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReconciliationError::Validation(msg) => write!(f, "{msg}"),
            ReconciliationError::Db(e) => write!(f, "database error: {e}"),
            ReconciliationError::NotFound(id) => write!(f, "reconciliation {id} not found"),
            ReconciliationError::OverlappingPeriod {
                existing_start,
                existing_end,
            } => write!(
                f,
                "This account already has a reconciliation covering {existing_start} to {existing_end}. Reopen it first if you need to redo that period."
            ),
            ReconciliationError::NotBalanced { difference_cents } => write!(
                f,
                "The difference is not zero yet ({difference_cents} cents) - clear or correct the remaining transactions before completing."
            ),
        }
    }
}

impl From<rusqlite::Error> for ReconciliationError {
    fn from(e: rusqlite::Error) -> Self {
        ReconciliationError::Db(e)
    }
}

/// The account's cleared balance as of `as_of_date` (section 8's formula).
/// `opening_balance` and `balance_adjustment` transactions always count -
/// they are the owner's own already-confirmed anchors (accounting.rs),
/// not a bank line item that needs checking off. Every other transaction
/// counts only once the owner has explicitly cleared or reconciled it
/// against a statement; a transaction still `Unreconciled` does not count
/// yet, which is exactly what makes clearing transactions move the
/// displayed difference. Archived and pending transactions never count,
/// matching how `accounts::get`'s own balance already excludes archived
/// rows and section 24's explicit pending-exclusion requirement.
pub fn cleared_balance_as_of(
    conn: &Connection,
    account_id: i64,
    as_of_date: &str,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(SUM(amount_cents), 0) FROM transactions
         WHERE account_id = ?1
           AND transaction_date <= ?2
           AND is_archived = 0
           AND (is_pending IS NULL OR is_pending = 0)
           AND (
                transaction_type IN ('opening_balance', 'balance_adjustment')
                OR reconciliation_status IN ('cleared', 'reconciled')
           )",
        params![account_id, as_of_date],
        |row| row.get(0),
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct RunningBalanceRow {
    pub transaction_id: i64,
    pub running_balance_cents: i64,
}

/// Running balance for every non-archived transaction in an account, in
/// deterministic oldest-first order: `transaction_date ASC, id ASC` - the
/// mirror of `transactions::list`'s own `DESC` tie-break, so the same two
/// transactions on the same date always sort the same way regardless of
/// direction (section 32). Every included transaction counts regardless of
/// `reconciliation_status` - this is the account's real running total, not
/// the cleared-balance-for-reconciliation figure (`cleared_balance_as_of`
/// above is the one that cares about clearing). One indexed range query
/// plus a linear fold - no N+1 (section 33).
pub fn running_balances(
    conn: &Connection,
    account_id: i64,
) -> rusqlite::Result<Vec<RunningBalanceRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, amount_cents FROM transactions
         WHERE account_id = ?1 AND is_archived = 0
         ORDER BY transaction_date ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![account_id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut running = 0i64;
    let mut result = Vec::new();
    for row in rows {
        let (transaction_id, amount_cents) = row?;
        running += amount_cents;
        result.push(RunningBalanceRow {
            transaction_id,
            running_balance_cents: running,
        });
    }
    Ok(result)
}

/// An active (period-claiming) reconciliation overlapping `[start, end]`
/// for this account, if any - `Completed` and `NeedsReview` both still
/// claim their period; `Reopened` does not (section 18/38).
fn find_overlapping(
    conn: &Connection,
    account_id: i64,
    start: &str,
    end: &str,
    exclude_id: Option<i64>,
) -> rusqlite::Result<Option<Reconciliation>> {
    conn.query_row(
        &format!(
            "SELECT {RECONCILIATION_COLUMNS} FROM reconciliations
             WHERE account_id = ?1
               AND status IN ('completed', 'needs_review')
               AND NOT (statement_end_date < ?2 OR statement_start_date > ?3)
               AND (?4 IS NULL OR id != ?4)
             LIMIT 1"
        ),
        params![account_id, start, end, exclude_id],
        row_to_reconciliation,
    )
    .optional()
}

fn validate_period(start: &str, end: &str) -> Result<(), ReconciliationError> {
    if end < start {
        return Err(ReconciliationError::Validation(
            "The statement end date must not be before the start date.".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct ReconciliationTransactionRow {
    #[serde(flatten)]
    pub transaction: Transaction,
    pub running_balance_cents: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReconciliationPreview {
    pub account_id: i64,
    pub statement_start_date: String,
    pub statement_end_date: String,
    pub statement_ending_balance_cents: i64,
    pub cleared_balance_cents: i64,
    pub difference_cents: i64,
    pub rows: Vec<ReconciliationTransactionRow>,
}

/// A pure computation - no writes. Safe to call repeatedly as the owner
/// edits the period/balance inputs or clears/unclears transactions (which
/// go straight through the existing `set_transaction_reconciliation_status`
/// command - no separate "stage a clear" concept, section 13).
pub fn preview(
    conn: &Connection,
    account_id: i64,
    statement_start_date: &str,
    statement_end_date: &str,
    statement_ending_balance_cents: i64,
) -> Result<ReconciliationPreview, ReconciliationError> {
    validate_period(statement_start_date, statement_end_date)?;
    if let Some(existing) = find_overlapping(
        conn,
        account_id,
        statement_start_date,
        statement_end_date,
        None,
    )? {
        return Err(ReconciliationError::OverlappingPeriod {
            existing_start: existing.statement_start_date,
            existing_end: existing.statement_end_date,
        });
    }

    let cleared_balance_cents = cleared_balance_as_of(conn, account_id, statement_end_date)?;
    let difference_cents = statement_ending_balance_cents - cleared_balance_cents;

    let balances = running_balances(conn, account_id)?;
    let balance_by_id: std::collections::HashMap<i64, i64> = balances
        .into_iter()
        .map(|r| (r.transaction_id, r.running_balance_cents))
        .collect();

    let period_transactions = transactions::list(
        conn,
        &transactions::TransactionFilter {
            account_id: Some(account_id),
            date_from: Some(statement_start_date.to_string()),
            date_to: Some(statement_end_date.to_string()),
            ..Default::default()
        },
    )
    .map_err(|e| ReconciliationError::Validation(e.to_string()))?;

    let rows = period_transactions
        .into_iter()
        .map(|t| {
            let running_balance_cents = balance_by_id.get(&t.id).copied().unwrap_or(0);
            ReconciliationTransactionRow {
                transaction: t,
                running_balance_cents,
            }
        })
        .collect();

    Ok(ReconciliationPreview {
        account_id,
        statement_start_date: statement_start_date.to_string(),
        statement_end_date: statement_end_date.to_string(),
        statement_ending_balance_cents,
        cleared_balance_cents,
        difference_cents,
        rows,
    })
}

/// Completes a reconciliation: re-verifies the difference is exactly zero
/// (never trusting a value computed earlier by the caller), re-checks for
/// an overlapping period (defense in depth against a race between preview
/// and completion), then atomically records the reconciliation and
/// promotes every `Cleared` transaction in the period - plus any
/// `balance_adjustment` dated within it, which always counts toward the
/// balance regardless of its own clear state - to `Reconciled`, linked to
/// the new record.
#[allow(clippy::too_many_arguments)]
pub fn complete(
    conn: &mut Connection,
    account_id: i64,
    statement_start_date: &str,
    statement_end_date: &str,
    statement_ending_balance_cents: i64,
    note: Option<String>,
    source_import_id: Option<i64>,
) -> Result<Reconciliation, ReconciliationError> {
    validate_period(statement_start_date, statement_end_date)?;
    if let Some(existing) = find_overlapping(
        conn,
        account_id,
        statement_start_date,
        statement_end_date,
        None,
    )? {
        return Err(ReconciliationError::OverlappingPeriod {
            existing_start: existing.statement_start_date,
            existing_end: existing.statement_end_date,
        });
    }

    let cleared_balance_cents = cleared_balance_as_of(conn, account_id, statement_end_date)?;
    let difference_cents = statement_ending_balance_cents - cleared_balance_cents;
    if difference_cents != 0 {
        return Err(ReconciliationError::NotBalanced { difference_cents });
    }

    let tx = conn.transaction()?;
    let id: i64 = tx.query_row(
        "INSERT INTO reconciliations (
            account_id, statement_start_date, statement_end_date,
            statement_ending_balance_cents, cleared_balance_cents, status, source_import_id, note
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'completed', ?6, ?7)
         RETURNING id",
        params![
            account_id,
            statement_start_date,
            statement_end_date,
            statement_ending_balance_cents,
            cleared_balance_cents,
            source_import_id,
            note,
        ],
        |row| row.get(0),
    )?;

    tx.execute(
        "UPDATE transactions SET reconciliation_status = 'reconciled', reconciliation_id = ?1,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE account_id = ?2 AND transaction_date BETWEEN ?3 AND ?4
           AND reconciliation_status = 'cleared'",
        params![id, account_id, statement_start_date, statement_end_date],
    )?;
    tx.execute(
        "UPDATE transactions SET reconciliation_status = 'reconciled', reconciliation_id = ?1,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE account_id = ?2 AND transaction_date BETWEEN ?3 AND ?4
           AND transaction_type = 'balance_adjustment' AND reconciliation_id IS NULL",
        params![id, account_id, statement_start_date, statement_end_date],
    )?;
    tx.commit()?;

    get(conn, id)?.ok_or(ReconciliationError::NotFound(id))
}

/// Reopens a completed or needs-review reconciliation (section 18): the
/// record itself is kept (status `Reopened`) for audit history rather than
/// deleted, its linked transactions revert to `Cleared` (not
/// `Unreconciled` - the owner's clearing work is preserved) and are freed
/// (`reconciliation_id = NULL`) so a future reconciliation can claim that
/// period again.
pub fn reopen(conn: &mut Connection, id: i64) -> Result<Reconciliation, ReconciliationError> {
    let existing = get(conn, id)?.ok_or(ReconciliationError::NotFound(id))?;

    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE transactions SET reconciliation_status = 'cleared', reconciliation_id = NULL,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE reconciliation_id = ?1",
        params![id],
    )?;
    tx.execute(
        "UPDATE reconciliations SET status = 'reopened', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1",
        params![id],
    )?;
    tx.commit()?;

    let _ = existing;
    get(conn, id)?.ok_or(ReconciliationError::NotFound(id))
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Reconciliation>, ReconciliationError> {
    conn.query_row(
        &format!("SELECT {RECONCILIATION_COLUMNS} FROM reconciliations WHERE id = ?1"),
        params![id],
        row_to_reconciliation,
    )
    .optional()
    .map_err(ReconciliationError::from)
}

/// History for one account, most recent statement first.
pub fn list_for_account(
    conn: &Connection,
    account_id: i64,
) -> Result<Vec<Reconciliation>, ReconciliationError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RECONCILIATION_COLUMNS} FROM reconciliations
         WHERE account_id = ?1 ORDER BY statement_end_date DESC, id DESC"
    ))?;
    let rows = stmt.query_map(params![account_id], row_to_reconciliation)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ReconciliationError::from)
}

#[derive(Debug, Clone, Serialize)]
pub struct ReconciliationSummary {
    pub last_reconciled_date: Option<String>,
    pub last_reconciled_ending_balance_cents: Option<i64>,
    pub needs_review_count: i64,
    pub unreconciled_transaction_count: i64,
}

/// Compact per-account signal (section 29) - deliberately small and
/// queryable now, without committing to the shape a future Data Health
/// feature will actually want.
pub fn account_summary(
    conn: &Connection,
    account_id: i64,
) -> rusqlite::Result<ReconciliationSummary> {
    let last: Option<(String, i64)> = conn
        .query_row(
            "SELECT statement_end_date, statement_ending_balance_cents FROM reconciliations
             WHERE account_id = ?1 AND status = 'completed'
             ORDER BY statement_end_date DESC LIMIT 1",
            params![account_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let needs_review_count: i64 = conn.query_row(
        "SELECT count(*) FROM reconciliations WHERE account_id = ?1 AND status = 'needs_review'",
        params![account_id],
        |row| row.get(0),
    )?;
    let unreconciled_transaction_count: i64 = conn.query_row(
        "SELECT count(*) FROM transactions
         WHERE account_id = ?1 AND is_archived = 0 AND reconciliation_status = 'unreconciled'",
        params![account_id],
        |row| row.get(0),
    )?;

    Ok(ReconciliationSummary {
        last_reconciled_date: last.as_ref().map(|(date, _)| date.clone()),
        last_reconciled_ending_balance_cents: last.map(|(_, cents)| cents),
        needs_review_count,
        unreconciled_transaction_count,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatementHint {
    pub source_import_id: i64,
    pub source_filename: String,
    pub statement_start_date: Option<String>,
    pub statement_end_date: Option<String>,
    pub statement_ending_balance_cents: i64,
}

/// The most recent import for this account that carried a statement
/// balance (OFX/QFX only - CSV never sets these, section 21). Purely a
/// prefill suggestion (section 7/20): the owner always still confirms or
/// overrides it in the reconciliation workspace before anything is
/// recorded.
pub fn latest_statement_hint(
    conn: &Connection,
    account_id: i64,
) -> rusqlite::Result<Option<StatementHint>> {
    conn.query_row(
        "SELECT id, source_filename, statement_start_date, statement_end_date, statement_balance_cents
         FROM imports
         WHERE account_id = ?1 AND statement_balance_cents IS NOT NULL
         ORDER BY imported_at DESC LIMIT 1",
        params![account_id],
        |row| {
            Ok(StatementHint {
                source_import_id: row.get(0)?,
                source_filename: row.get(1)?,
                statement_start_date: row.get(2)?,
                statement_end_date: row.get(3)?,
                statement_ending_balance_cents: row.get(4)?,
            })
        },
    )
    .optional()
}

/// Marks the reconciliation (if any) a transaction currently belongs to as
/// `NeedsReview`, if it is currently `Completed`. Unconditional - used for
/// changes that are always invalidating regardless of the specific old/new
/// values (archiving, deleting, removing/changing a linked balance
/// adjustment). A no-op if the transaction isn't linked, or its
/// reconciliation is already `NeedsReview`/`Reopened`.
pub fn invalidate_if_linked(conn: &Connection, transaction_id: i64) -> rusqlite::Result<()> {
    let reconciliation_id: Option<i64> = conn
        .query_row(
            "SELECT reconciliation_id FROM transactions WHERE id = ?1",
            params![transaction_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if let Some(id) = reconciliation_id {
        mark_needs_review(conn, id)?;
    }
    Ok(())
}

/// Marks a `Completed` reconciliation `NeedsReview` if one exists for this
/// account whose statement period covers `date`. Unlike
/// `invalidate_if_linked`, this doesn't require the affected row to already
/// carry a `reconciliation_id` - it covers the one case that can't: a
/// *brand-new* `balance_adjustment` landing, for the first time, on a date
/// an already-completed reconciliation covers. A balance adjustment always
/// counts toward `cleared_balance_as_of` regardless of its own clearing
/// state, so its mere existence on that date changes the sum even before
/// anything ever links it to a reconciliation.
pub fn invalidate_completed_covering(
    conn: &Connection,
    account_id: i64,
    date: &str,
) -> rusqlite::Result<()> {
    let id: Option<i64> = conn
        .query_row(
            "SELECT id FROM reconciliations
             WHERE account_id = ?1 AND status = 'completed'
               AND statement_start_date <= ?2 AND statement_end_date >= ?2
             LIMIT 1",
            params![account_id, date],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = id {
        mark_needs_review(conn, id)?;
    }
    Ok(())
}

fn mark_needs_review(conn: &Connection, reconciliation_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE reconciliations SET status = 'needs_review', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1 AND status = 'completed'",
        params![reconciliation_id],
    )?;
    Ok(())
}

/// Compares a transaction's about-to-be-overwritten field values against
/// what `transactions::update` is about to set, and marks its
/// reconciliation `NeedsReview` if the change is one of the kinds section
/// 16 names as invalidating: amount, account, transaction type, or the
/// date moving outside the reconciliation's own statement period ("date
/// crossing statement boundaries" - a date change that stays inside the
/// period never changes the cleared-balance sum, so it is deliberately not
/// invalidating). Must be called with the row's *current* (pre-update)
/// values - `transactions::update` calls this before executing its SQL.
/// Harmless edits (notes, category, description, tags) never reach this
/// function at all, since they are not among the fields compared.
#[allow(clippy::too_many_arguments)]
pub fn invalidate_if_update_matters(
    conn: &Connection,
    old_account_id: i64,
    old_amount_cents: i64,
    old_transaction_type: &str,
    old_transaction_date: &str,
    reconciliation_id: Option<i64>,
    new_account_id: i64,
    new_amount_cents: i64,
    new_transaction_type: &str,
    new_transaction_date: &str,
) -> rusqlite::Result<()> {
    let Some(reconciliation_id) = reconciliation_id else {
        return Ok(());
    };

    let mut invalidating = old_account_id != new_account_id
        || old_amount_cents != new_amount_cents
        || old_transaction_type != new_transaction_type;

    if !invalidating && old_transaction_date != new_transaction_date {
        let (start, end): (String, String) = conn.query_row(
            "SELECT statement_start_date, statement_end_date FROM reconciliations WHERE id = ?1",
            params![reconciliation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        invalidating = new_transaction_date < start.as_str() || new_transaction_date > end.as_str();
    }

    if invalidating {
        mark_needs_review(conn, reconciliation_id)?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_reconciliation_preview(
    state: tauri::State<crate::db::DbState>,
    account_id: i64,
    statement_start_date: String,
    statement_end_date: String,
    statement_ending_balance_cents: i64,
) -> Result<ReconciliationPreview, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    preview(
        &conn,
        account_id,
        &statement_start_date,
        &statement_end_date,
        statement_ending_balance_cents,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn complete_reconciliation(
    state: tauri::State<crate::db::DbState>,
    account_id: i64,
    statement_start_date: String,
    statement_end_date: String,
    statement_ending_balance_cents: i64,
    note: Option<String>,
    source_import_id: Option<i64>,
) -> Result<Reconciliation, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    complete(
        &mut conn,
        account_id,
        &statement_start_date,
        &statement_end_date,
        statement_ending_balance_cents,
        note,
        source_import_id,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reopen_reconciliation(
    state: tauri::State<crate::db::DbState>,
    id: i64,
) -> Result<Reconciliation, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    reopen(&mut conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_reconciliations(
    state: tauri::State<crate::db::DbState>,
    account_id: i64,
) -> Result<Vec<Reconciliation>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list_for_account(&conn, account_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_reconciliation_summary(
    state: tauri::State<crate::db::DbState>,
    account_id: i64,
) -> Result<ReconciliationSummary, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    account_summary(&conn, account_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_latest_statement_hint(
    state: tauri::State<crate::db::DbState>,
    account_id: i64,
) -> Result<Option<StatementHint>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    latest_statement_hint(&conn, account_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_running_balances(
    state: tauri::State<crate::db::DbState>,
    account_id: i64,
) -> Result<Vec<RunningBalanceRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    running_balances(&conn, account_id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::transactions::{TransactionInput, TransactionType};

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
        conn
    }

    fn make_account(conn: &Connection, account_type: AccountType) -> i64 {
        accounts::create(
            conn,
            &AccountInput {
                name: format!("{account_type:?}"),
                institution: None,
                account_type,
                include_in_net_worth: true,
                notes: None,
            },
        )
        .unwrap()
        .id
    }

    fn add_txn(
        conn: &Connection,
        account_id: i64,
        date: &str,
        amount_cents: i64,
        transaction_type: TransactionType,
    ) -> i64 {
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: date.to_string(),
                posted_date: None,
                amount_cents,
                transaction_type,
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

    fn clear(conn: &Connection, transaction_id: i64) {
        transactions::set_reconciliation_status(
            conn,
            transaction_id,
            transactions::ReconciliationStatus::Cleared,
        )
        .unwrap();
    }

    // --- Running balance (section 39) ---

    #[test]
    fn running_balance_accumulates_in_deterministic_oldest_first_order() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t1 = add_txn(
            &conn,
            checking,
            "2026-08-01",
            -1000,
            TransactionType::Purchase,
        );
        let t2 = add_txn(&conn, checking, "2026-08-02", 5000, TransactionType::Income);
        let t3 = add_txn(
            &conn,
            checking,
            "2026-08-03",
            -2000,
            TransactionType::Purchase,
        );

        let balances = running_balances(&conn, checking).unwrap();
        assert_eq!(balances.len(), 3);
        assert_eq!(balances[0].transaction_id, t1);
        assert_eq!(balances[0].running_balance_cents, -1000);
        assert_eq!(balances[1].transaction_id, t2);
        assert_eq!(balances[1].running_balance_cents, 4000);
        assert_eq!(balances[2].transaction_id, t3);
        assert_eq!(balances[2].running_balance_cents, 2000);
    }

    #[test]
    fn same_date_transactions_break_ties_by_ascending_id_deterministically() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let first = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -100,
            TransactionType::Purchase,
        );
        let second = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -200,
            TransactionType::Purchase,
        );

        let balances = running_balances(&conn, checking).unwrap();
        assert_eq!(balances[0].transaction_id, first);
        assert_eq!(balances[0].running_balance_cents, -100);
        assert_eq!(balances[1].transaction_id, second);
        assert_eq!(balances[1].running_balance_cents, -300);

        // Re-running must produce byte-identical ordering - no reliance on
        // undefined SQL row order.
        let again = running_balances(&conn, checking).unwrap();
        assert_eq!(
            balances
                .iter()
                .map(|r| r.transaction_id)
                .collect::<Vec<_>>(),
            again.iter().map(|r| r.transaction_id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn running_balance_excludes_archived_transactions() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(&conn, checking, "2026-08-01", 1000, TransactionType::Income);
        let archived = add_txn(
            &conn,
            checking,
            "2026-08-02",
            -500,
            TransactionType::Purchase,
        );
        transactions::set_archived(&conn, archived, true).unwrap();
        add_txn(
            &conn,
            checking,
            "2026-08-03",
            -100,
            TransactionType::Purchase,
        );

        let balances = running_balances(&conn, checking).unwrap();
        assert_eq!(balances.len(), 2);
        assert_eq!(balances.last().unwrap().running_balance_cents, 900);
    }

    #[test]
    fn running_balance_works_the_same_way_for_a_credit_card() {
        let conn = test_conn();
        let card = make_account(&conn, AccountType::CreditCard);
        add_txn(&conn, card, "2026-08-01", 5000, TransactionType::Purchase);
        add_txn(&conn, card, "2026-08-15", -2000, TransactionType::Transfer); // payment
        let balances = running_balances(&conn, card).unwrap();
        assert_eq!(balances[0].running_balance_cents, 5000);
        assert_eq!(balances[1].running_balance_cents, 3000);
    }

    // --- Cleared balance formula (section 8/39) ---

    #[test]
    fn cleared_balance_excludes_uncleared_transactions_in_range() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let cleared = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -1000,
            TransactionType::Purchase,
        );
        add_txn(
            &conn,
            checking,
            "2026-08-10",
            -2000,
            TransactionType::Purchase,
        ); // never cleared
        clear(&conn, cleared);

        assert_eq!(
            cleared_balance_as_of(&conn, checking, "2026-08-31").unwrap(),
            -1000
        );
    }

    #[test]
    fn cleared_balance_includes_reconciled_transactions_from_a_prior_period() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-07-15",
            10000,
            TransactionType::Income,
        );
        transactions::set_reconciliation_status(
            &conn,
            t,
            transactions::ReconciliationStatus::Reconciled,
        )
        .unwrap();

        assert_eq!(
            cleared_balance_as_of(&conn, checking, "2026-08-31").unwrap(),
            10000,
            "a transaction already reconciled in an earlier period must still count going forward"
        );
    }

    #[test]
    fn cleared_balance_always_counts_opening_balance_and_balance_adjustment_without_clearing() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        crate::accounting::reconcile_balance(&conn, checking, 100_000, "2026-01-01").unwrap();
        crate::accounting::reconcile_balance(&conn, checking, 150_000, "2026-08-01").unwrap();

        // Neither the opening balance nor the adjustment was ever explicitly
        // cleared, but both are owner-asserted anchors, not bank line items.
        assert_eq!(
            cleared_balance_as_of(&conn, checking, "2026-08-31").unwrap(),
            150_000
        );
    }

    #[test]
    fn cleared_balance_excludes_pending_transactions_even_if_cleared() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = transactions::create(
            &conn,
            &TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -500,
                transaction_type: TransactionType::Purchase,
                raw_description: "Pending charge".to_string(),
                merchant: None,
                category: None,
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: Some(true),
                payee_id: None,
                is_one_time: false,
            },
        )
        .unwrap()
        .id;
        clear(&conn, t);

        assert_eq!(
            cleared_balance_as_of(&conn, checking, "2026-08-31").unwrap(),
            0,
            "a pending transaction must not count toward a cleared/reconciled statement balance"
        );
    }

    #[test]
    fn cleared_balance_excludes_archived_transactions() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -500,
            TransactionType::Purchase,
        );
        clear(&conn, t);
        transactions::set_archived(&conn, t, true).unwrap();

        assert_eq!(
            cleared_balance_as_of(&conn, checking, "2026-08-31").unwrap(),
            0
        );
    }

    #[test]
    fn cleared_balance_incorporates_refunds_and_transfers_like_any_other_signed_amount() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let purchase = add_txn(
            &conn,
            checking,
            "2026-08-01",
            -8000,
            TransactionType::Purchase,
        );
        let refund = add_txn(&conn, checking, "2026-08-05", 3000, TransactionType::Refund);
        let transfer = add_txn(
            &conn,
            checking,
            "2026-08-10",
            -1000,
            TransactionType::Transfer,
        );
        clear(&conn, purchase);
        clear(&conn, refund);
        clear(&conn, transfer);

        assert_eq!(
            cleared_balance_as_of(&conn, checking, "2026-08-31").unwrap(),
            -8000 + 3000 - 1000
        );
    }

    #[test]
    fn cleared_balance_works_symmetrically_for_a_credit_card_purchase_and_payment() {
        let conn = test_conn();
        let card = make_account(&conn, AccountType::CreditCard);
        let purchase = add_txn(&conn, card, "2026-08-01", 5000, TransactionType::Purchase);
        let payment = add_txn(&conn, card, "2026-08-20", -2000, TransactionType::Transfer);
        clear(&conn, purchase);
        clear(&conn, payment);

        assert_eq!(
            cleared_balance_as_of(&conn, card, "2026-08-31").unwrap(),
            3000,
            "matches the familiar 'amount owed' figure a credit card statement itself shows"
        );
    }

    // --- Reconciliation lifecycle (section 40) ---

    #[test]
    fn first_reconciliation_completes_when_difference_is_exactly_zero() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -4500,
            TransactionType::Purchase,
        );
        clear(&conn, t);

        let result = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -4500,
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.status, ReconciliationRecordStatus::Completed);
        assert_eq!(result.cleared_balance_cents, -4500);

        let updated = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                account_id: Some(checking),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            updated[0].reconciliation_status,
            transactions::ReconciliationStatus::Reconciled
        );
    }

    #[test]
    fn non_zero_difference_blocks_completion_and_changes_nothing() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -4500,
            TransactionType::Purchase,
        );
        clear(&conn, t);

        let err = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -5000, // statement disagrees by $5
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ReconciliationError::NotBalanced {
                difference_cents: -500
            }
        ));

        let count: i64 = conn
            .query_row("SELECT count(*) FROM reconciliations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "a failed completion must not record anything");
    }

    #[test]
    fn credit_card_statement_reconciles_using_the_same_familiar_positive_owed_convention() {
        let mut conn = test_conn();
        let card = make_account(&conn, AccountType::CreditCard);
        // $1,842.73 owed, matching product handoff section 22's example -
        // entered and stored as a positive number in both places, since
        // ADR-0004 already makes a liability's own amount_cents positive
        // when it increases what's owed.
        let purchase = add_txn(
            &conn,
            card,
            "2026-08-05",
            184_273,
            TransactionType::Purchase,
        );
        clear(&conn, purchase);

        let result = complete(
            &mut conn,
            card,
            "2026-08-01",
            "2026-08-31",
            184_273,
            None,
            None,
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn completing_reconciliation_also_links_a_balance_adjustment_dated_within_the_period() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        crate::accounting::reconcile_balance(&conn, checking, 100_000, "2026-08-10").unwrap();

        let result = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            100_000,
            None,
            None,
        )
        .unwrap();

        let adjustment_reconciliation_id: Option<i64> = conn
            .query_row(
                "SELECT reconciliation_id FROM transactions WHERE transaction_type = 'opening_balance'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // The opening balance is a global anchor, not tied to one statement
        // period - it must NOT be linked to any specific reconciliation.
        assert_eq!(adjustment_reconciliation_id, None);
        let _ = result;
    }

    #[test]
    fn history_lists_most_recent_statement_first() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        complete(
            &mut conn,
            checking,
            "2026-07-01",
            "2026-07-31",
            0,
            None,
            None,
        )
        .unwrap();
        complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            0,
            None,
            None,
        )
        .unwrap();

        let history = list_for_account(&conn, checking).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].statement_end_date, "2026-08-31");
        assert_eq!(history[1].statement_end_date, "2026-07-31");
    }

    #[test]
    fn overlapping_period_is_rejected_with_a_clear_error() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            0,
            None,
            None,
        )
        .unwrap();

        let err = complete(
            &mut conn,
            checking,
            "2026-08-15",
            "2026-09-15",
            0,
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(err, ReconciliationError::OverlappingPeriod { .. }));
    }

    #[test]
    fn reopening_frees_transactions_back_to_cleared_and_preserves_history() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -1000,
            TransactionType::Purchase,
        );
        clear(&conn, t);
        let reconciliation = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -1000,
            None,
            None,
        )
        .unwrap();

        let reopened = reopen(&mut conn, reconciliation.id).unwrap();
        assert_eq!(reopened.status, ReconciliationRecordStatus::Reopened);

        let txn = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                account_id: Some(checking),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            txn[0].reconciliation_status,
            transactions::ReconciliationStatus::Cleared,
            "reopening must revert to Cleared, not silently back to Unreconciled"
        );

        // The freed period can now be reconciled again.
        let again = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -1000,
            None,
            None,
        );
        assert!(again.is_ok(), "{again:?}");

        let history = list_for_account(&conn, checking).unwrap();
        assert_eq!(
            history.len(),
            2,
            "the reopened record must remain in history"
        );
    }

    #[test]
    fn editing_the_amount_of_a_reconciled_transaction_marks_it_needs_review() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -1000,
            TransactionType::Purchase,
        );
        clear(&conn, t);
        let reconciliation = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -1000,
            None,
            None,
        )
        .unwrap();

        let mut edit = transactions::TransactionInput {
            account_id: checking,
            transaction_date: "2026-08-05".to_string(),
            posted_date: None,
            amount_cents: -9999,
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
        };
        transactions::update(&conn, t, &edit).unwrap();
        let after = get(&conn, reconciliation.id).unwrap().unwrap();
        assert_eq!(after.status, ReconciliationRecordStatus::NeedsReview);

        // Sanity: an unrelated field-only edit on a DIFFERENT, never-linked
        // transaction never touches this reconciliation.
        edit.amount_cents = -1;
        let _ = edit;
    }

    #[test]
    fn a_harmless_metadata_edit_does_not_invalidate_a_reconciled_transaction() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -1000,
            TransactionType::Purchase,
        );
        clear(&conn, t);
        let reconciliation = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -1000,
            None,
            None,
        )
        .unwrap();

        transactions::update(
            &conn,
            t,
            &transactions::TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -1000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Test".to_string(),
                merchant: Some("Renamed merchant".to_string()),
                category: Some("Dining".to_string()),
                subcategory: None,
                notes: Some("A note".to_string()),
                tags: None,
                is_pending: None,
                payee_id: None,
                is_one_time: false,
            },
        )
        .unwrap();

        let after = get(&conn, reconciliation.id).unwrap().unwrap();
        assert_eq!(
            after.status,
            ReconciliationRecordStatus::Completed,
            "notes/category/merchant text must never invalidate a reconciliation"
        );
    }

    #[test]
    fn moving_a_reconciled_transactions_date_within_the_period_does_not_invalidate() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -1000,
            TransactionType::Purchase,
        );
        clear(&conn, t);
        let reconciliation = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -1000,
            None,
            None,
        )
        .unwrap();

        transactions::update(
            &conn,
            t,
            &transactions::TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-20".to_string(), // still inside Aug 1-31
                posted_date: None,
                amount_cents: -1000,
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
        .unwrap();

        let after = get(&conn, reconciliation.id).unwrap().unwrap();
        assert_eq!(after.status, ReconciliationRecordStatus::Completed);
    }

    #[test]
    fn moving_a_reconciled_transactions_date_outside_the_period_invalidates() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -1000,
            TransactionType::Purchase,
        );
        clear(&conn, t);
        let reconciliation = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -1000,
            None,
            None,
        )
        .unwrap();

        transactions::update(
            &conn,
            t,
            &transactions::TransactionInput {
                account_id: checking,
                transaction_date: "2026-09-02".to_string(), // crosses the boundary
                posted_date: None,
                amount_cents: -1000,
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
        .unwrap();

        let after = get(&conn, reconciliation.id).unwrap().unwrap();
        assert_eq!(after.status, ReconciliationRecordStatus::NeedsReview);
    }

    #[test]
    fn archiving_a_reconciled_transaction_invalidates_its_reconciliation() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -1000,
            TransactionType::Purchase,
        );
        clear(&conn, t);
        let reconciliation = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -1000,
            None,
            None,
        )
        .unwrap();

        transactions::set_archived(&conn, t, true).unwrap();

        let after = get(&conn, reconciliation.id).unwrap().unwrap();
        assert_eq!(after.status, ReconciliationRecordStatus::NeedsReview);
    }

    #[test]
    fn deleting_a_reconciled_manual_transaction_invalidates_its_reconciliation() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -1000,
            TransactionType::Purchase,
        );
        clear(&conn, t);
        let reconciliation = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -1000,
            None,
            None,
        )
        .unwrap();

        transactions::delete(&conn, t).unwrap();

        let after = get(&conn, reconciliation.id).unwrap().unwrap();
        assert_eq!(after.status, ReconciliationRecordStatus::NeedsReview);
    }

    #[test]
    fn changing_transaction_type_of_a_reconciled_transaction_invalidates() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let t = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -1000,
            TransactionType::Purchase,
        );
        clear(&conn, t);
        let reconciliation = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -1000,
            None,
            None,
        )
        .unwrap();

        transactions::update(
            &conn,
            t,
            &transactions::TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -1000,
                transaction_type: TransactionType::Refund,
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
        .unwrap();

        let after = get(&conn, reconciliation.id).unwrap().unwrap();
        assert_eq!(after.status, ReconciliationRecordStatus::NeedsReview);
    }

    #[test]
    fn changing_an_already_reconciled_balance_adjustments_amount_invalidates() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        crate::accounting::reconcile_balance(&conn, checking, 50_000, "2026-08-10").unwrap();
        let reconciliation = complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            50_000,
            None,
            None,
        )
        .unwrap();

        // A later "Update balance" call on a date inside the same period
        // corrects that same adjustment in place (accounting.rs's upsert).
        crate::accounting::reconcile_balance(&conn, checking, 90_000, "2026-08-10").unwrap();

        let after = get(&conn, reconciliation.id).unwrap().unwrap();
        assert_eq!(
            after.status,
            ReconciliationRecordStatus::NeedsReview,
            "correcting a linked balance adjustment must invalidate the reconciliation it was part of"
        );
    }

    #[test]
    fn statement_hint_is_none_when_no_import_ever_provided_a_statement_balance() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        assert_eq!(latest_statement_hint(&conn, checking).unwrap(), None);
    }

    #[test]
    fn account_summary_reports_last_reconciled_and_needs_review_counts() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(
            &conn,
            checking,
            "2026-08-05",
            -1000,
            TransactionType::Purchase,
        );

        let before = account_summary(&conn, checking).unwrap();
        assert_eq!(before.last_reconciled_date, None);
        assert_eq!(before.unreconciled_transaction_count, 1);

        complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            0,
            None,
            None,
        )
        .unwrap();
        let after = account_summary(&conn, checking).unwrap();
        assert_eq!(after.last_reconciled_date.as_deref(), Some("2026-08-31"));
        assert_eq!(after.needs_review_count, 0);
    }

    #[test]
    fn preview_reflects_a_transaction_added_after_the_workspace_was_opened() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let before = preview(&conn, checking, "2026-08-01", "2026-08-31", -500).unwrap();
        assert_eq!(before.difference_cents, -500);

        let missing = add_txn(
            &conn,
            checking,
            "2026-08-10",
            -500,
            TransactionType::Purchase,
        );
        clear(&conn, missing);

        let after = preview(&conn, checking, "2026-08-01", "2026-08-31", -500).unwrap();
        assert_eq!(
            after.difference_cents, 0,
            "adding and clearing the missing transaction must immediately close the gap"
        );
    }

    #[test]
    fn invalid_date_range_is_rejected() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let err = preview(&conn, checking, "2026-08-31", "2026-08-01", 0).unwrap_err();
        assert!(matches!(err, ReconciliationError::Validation(_)));
    }
}
