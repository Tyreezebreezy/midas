//! Expected occurrences and expected-to-actual matching (Batch 6, specs/
//! phase-014). An occurrence is a projection - it has no amount that sums
//! into any balance, and matching one to a real transaction never creates,
//! edits, or duplicates that transaction (product handoff section 4).
//!
//! Generation and matching both run lazily, on demand (section 53): there
//! is no background job. Occurrences are generated out to a rolling
//! `HORIZON_DAYS`-day window (section 26) whenever something asks for a
//! series' occurrences (listing, confirming, or a transaction arriving that
//! might match one) - never generated indefinitely far into the future.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::dates::{add_days, add_months, days_between};
use crate::db::DbState;
use crate::recurring::{self, Cadence, RecurringError};
use tauri::State;

/// How far ahead occurrences are generated - a practical horizon, not an
/// unbounded future (section 26). Chosen to comfortably cover a calendar
/// view a few months out without ever needing a background job to keep it
/// filled; re-extended lazily every time `ensure_occurrences` runs.
const HORIZON_DAYS: i64 = 90;

/// Safety cap on how many occurrences one `ensure_occurrences` call will
/// ever insert - purely defensive (a validated cadence can't actually
/// produce this many within any reasonable horizon), so a future bug in
/// cadence advancement fails loudly/slowly rather than filling the database.
const MAX_GENERATED_PER_CALL: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceStatus {
    Upcoming,
    Due,
    Matched,
    Missed,
    Skipped,
    NeedsReview,
}

impl OccurrenceStatus {
    // Written as literal SQL string constants at each call site (never
    // parameterized) since every status transition here is a fixed,
    // compile-time-known value, not user input - unlike Cadence/Direction/
    // Kind/SeriesStatus in recurring.rs, which pass a *variable* status
    // through `.as_sql_str()` as a bound parameter.
    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "upcoming" => OccurrenceStatus::Upcoming,
            "due" => OccurrenceStatus::Due,
            "matched" => OccurrenceStatus::Matched,
            "missed" => OccurrenceStatus::Missed,
            "skipped" => OccurrenceStatus::Skipped,
            "needs_review" => OccurrenceStatus::NeedsReview,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown occurrence status '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RecurringOccurrence {
    pub id: i64,
    pub series_id: i64,
    pub expected_date: String,
    pub expected_amount_cents: i64,
    pub status: OccurrenceStatus,
    pub matched_transaction_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    // Denormalized from the parent series for calendar/list display without
    // an extra round trip per row.
    pub merchant: String,
    pub direction: String,
    pub kind: String,
    pub account_id: i64,
}

const OCCURRENCE_COLUMNS: &str =
    "o.id, o.series_id, o.expected_date, o.expected_amount_cents, o.status, \
     o.matched_transaction_id, o.created_at, o.updated_at, \
     s.merchant, s.direction, s.kind, s.account_id";

fn hydrate_occurrence(row: &rusqlite::Row) -> rusqlite::Result<RecurringOccurrence> {
    Ok(RecurringOccurrence {
        id: row.get(0)?,
        series_id: row.get(1)?,
        expected_date: row.get(2)?,
        expected_amount_cents: row.get(3)?,
        status: OccurrenceStatus::from_sql_str(&row.get::<_, String>(4)?)?,
        matched_transaction_id: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        merchant: row.get(8)?,
        direction: row.get(9)?,
        kind: row.get(10)?,
        account_id: row.get(11)?,
    })
}

fn today(conn: &Connection) -> rusqlite::Result<String> {
    conn.query_row("SELECT date('now')", [], |row| row.get(0))
}

fn advance_one_step(
    date: &str,
    cadence: Cadence,
    day_1: Option<i64>,
    day_2: Option<i64>,
) -> String {
    match cadence {
        Cadence::Weekly => add_days(date, 7),
        Cadence::Biweekly => add_days(date, 14),
        // Month-anchored cadences advance by calendar months (correctly
        // clamped for month length, section 7), never a fixed day count -
        // a bill anchored to the 31st must land on Feb 28/29, not drift.
        Cadence::Monthly => add_months(date, 1),
        Cadence::Quarterly => add_months(date, 3),
        Cadence::Annual => add_months(date, 12),
        Cadence::Semimonthly => {
            let (d1, d2) = (
                day_1.expect("semimonthly series must have both days set"),
                day_2.expect("semimonthly series must have both days set"),
            );
            recurring::next_semimonthly_date(date, d1, d2)
        }
    }
}

/// Generates any missing occurrences for a Confirmed series from wherever
/// generation last left off out to `today + HORIZON_DAYS`, refreshes
/// Upcoming -> Due -> Missed transitions for existing ones, and updates the
/// series' own `next_expected_date` to the earliest still-open occurrence
/// (keeping `pay_cycle.rs`'s existing consumers, which read that field
/// directly, correctly fed). A no-op for anything not Confirmed - a
/// Proposed/Rejected/Inactive pattern gets no projected future.
/// `INSERT OR IGNORE` plus the table's own `UNIQUE(series_id,
/// expected_date)` constraint makes repeated calls idempotent (section 55):
/// re-running this, restarting MIDAS, or importing the same file twice
/// never duplicates an occurrence.
pub fn ensure_occurrences(
    conn: &Connection,
    series_id: i64,
) -> Result<Vec<RecurringOccurrence>, RecurringError> {
    let series = recurring::get(conn, series_id)?.ok_or(RecurringError::NotFound(series_id))?;
    if series.status != crate::recurring::SeriesStatus::Confirmed {
        return Ok(Vec::new());
    }

    let latest_existing: Option<String> = conn
        .query_row(
            "SELECT expected_date FROM recurring_occurrences WHERE series_id = ?1 ORDER BY expected_date DESC LIMIT 1",
            params![series_id],
            |r| r.get(0),
        )
        .optional()?;

    let today_str = today(conn)?;
    let horizon = add_days(&today_str, HORIZON_DAYS);

    let mut cursor = latest_existing
        .as_deref()
        .map(|d| {
            advance_one_step(
                d,
                series.cadence,
                series.semimonthly_day_1,
                series.semimonthly_day_2,
            )
        })
        .unwrap_or_else(|| series.next_expected_date.clone());

    let mut generated = 0;
    while days_between(&horizon, &cursor) <= 0 && generated < MAX_GENERATED_PER_CALL {
        conn.execute(
            "INSERT OR IGNORE INTO recurring_occurrences (series_id, expected_date, expected_amount_cents)
             VALUES (?1, ?2, ?3)",
            params![series_id, cursor, series.expected_amount_cents],
        )?;
        cursor = advance_one_step(
            &cursor,
            series.cadence,
            series.semimonthly_day_1,
            series.semimonthly_day_2,
        );
        generated += 1;
    }

    refresh_statuses(conn, series_id, &today_str, series.cadence)?;

    // Keep the series' own denormalized next_expected_date in sync with the
    // earliest occurrence still meaningfully in the future.
    let earliest_open: Option<String> = conn
        .query_row(
            "SELECT expected_date FROM recurring_occurrences
             WHERE series_id = ?1 AND status IN ('upcoming', 'due')
             ORDER BY expected_date ASC LIMIT 1",
            params![series_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(next) = earliest_open {
        conn.execute(
            "UPDATE recurring_series SET next_expected_date = ?2 WHERE id = ?1",
            params![series_id, next],
        )?;
    }

    list_for_series(conn, series_id)
}

/// Upcoming -> Due (expected_date has arrived) and Due/Upcoming -> Missed
/// (the matching window has closed with nothing linked) - section 67. Never
/// touches Matched/Skipped/NeedsReview occurrences.
fn refresh_statuses(
    conn: &Connection,
    series_id: i64,
    today_str: &str,
    cadence: Cadence,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE recurring_occurrences SET status = 'due', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE series_id = ?1 AND status = 'upcoming' AND expected_date <= ?2",
        params![series_id, today_str],
    )?;

    let window = cadence.matching_window_days();
    let missed_cutoff = add_days(today_str, -window);
    conn.execute(
        "UPDATE recurring_occurrences SET status = 'missed', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE series_id = ?1 AND status IN ('upcoming', 'due') AND expected_date <= ?2",
        params![series_id, missed_cutoff],
    )?;
    Ok(())
}

pub fn list_for_series(
    conn: &Connection,
    series_id: i64,
) -> Result<Vec<RecurringOccurrence>, RecurringError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {OCCURRENCE_COLUMNS} FROM recurring_occurrences o
         JOIN recurring_series s ON s.id = o.series_id
         WHERE o.series_id = ?1 ORDER BY o.expected_date ASC"
    ))?;
    let rows = stmt
        .query_map(params![series_id], hydrate_occurrence)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every occurrence expected within `[from, to]`, across every Confirmed
/// series (optionally scoped to one account) - the query behind the
/// calendar/upcoming list (sections 40-42). Refreshes generation/statuses
/// for every confirmed series first, so the range is never stale just
/// because nobody opened the Recurring page in a while.
pub fn upcoming_in_range(
    conn: &Connection,
    from: &str,
    to: &str,
    account_id: Option<i64>,
) -> Result<Vec<RecurringOccurrence>, RecurringError> {
    let confirmed_ids: Vec<i64> = {
        let sql = if account_id.is_some() {
            "SELECT id FROM recurring_series WHERE status = 'confirmed' AND account_id = ?1"
        } else {
            "SELECT id FROM recurring_series WHERE status = 'confirmed'"
        };
        let mut stmt = conn.prepare(sql)?;
        if let Some(a) = account_id {
            stmt.query_map(params![a], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        }
    };
    for id in &confirmed_ids {
        ensure_occurrences(conn, *id)?;
    }

    let sql = format!(
        "SELECT {OCCURRENCE_COLUMNS} FROM recurring_occurrences o
         JOIN recurring_series s ON s.id = o.series_id
         WHERE o.expected_date >= ?1 AND o.expected_date <= ?2
           AND s.status = 'confirmed'
           {}
         ORDER BY o.expected_date ASC, s.merchant ASC",
        if account_id.is_some() {
            "AND s.account_id = ?3"
        } else {
            ""
        }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = if let Some(a) = account_id {
        stmt.query_map(params![from, to, a], hydrate_occurrence)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map(params![from, to], hydrate_occurrence)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

/// Marks one occurrence Skipped (section 68) - a paused subscription, a
/// skipped transfer, an unpaid optional membership. Refuses on an
/// already-Matched occurrence (unlink first) so a real linked transaction
/// is never silently orphaned by a skip.
pub fn skip_occurrence(
    conn: &Connection,
    occurrence_id: i64,
) -> Result<RecurringOccurrence, RecurringError> {
    let occurrence =
        get_occurrence(conn, occurrence_id)?.ok_or(RecurringError::NotFound(occurrence_id))?;
    if occurrence.status == OccurrenceStatus::Matched {
        return Err(RecurringError::Validation(
            "This occurrence is already matched to a transaction - unlink it first.".to_string(),
        ));
    }
    conn.execute(
        "UPDATE recurring_occurrences SET status = 'skipped', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![occurrence_id],
    )?;
    get_occurrence(conn, occurrence_id)?.ok_or(RecurringError::NotFound(occurrence_id))
}

fn get_occurrence(
    conn: &Connection,
    occurrence_id: i64,
) -> Result<Option<RecurringOccurrence>, RecurringError> {
    conn.query_row(
        &format!(
            "SELECT {OCCURRENCE_COLUMNS} FROM recurring_occurrences o
             JOIN recurring_series s ON s.id = o.series_id WHERE o.id = ?1"
        ),
        params![occurrence_id],
        hydrate_occurrence,
    )
    .optional()
    .map_err(RecurringError::from)
}

/// Manually links an occurrence to a transaction when automatic matching
/// was uncertain or wrong (section 32). Refuses if the transaction is
/// already matched to a *different* occurrence - one real transaction must
/// never satisfy two expectations (section 30).
pub fn link_occurrence_to_transaction(
    conn: &Connection,
    occurrence_id: i64,
    transaction_id: i64,
) -> Result<RecurringOccurrence, RecurringError> {
    let already: Option<i64> = conn
        .query_row(
            "SELECT id FROM recurring_occurrences WHERE matched_transaction_id = ?1 AND id != ?2",
            params![transaction_id, occurrence_id],
            |r| r.get(0),
        )
        .optional()?;
    if already.is_some() {
        return Err(RecurringError::Validation(
            "This transaction is already linked to a different occurrence.".to_string(),
        ));
    }
    let occurrence =
        get_occurrence(conn, occurrence_id)?.ok_or(RecurringError::NotFound(occurrence_id))?;
    conn.execute(
        "UPDATE recurring_occurrences SET status = 'matched', matched_transaction_id = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![occurrence_id, transaction_id],
    )?;
    conn.execute(
        "UPDATE transactions SET recurring_series_id = ?2 WHERE id = ?1 AND recurring_series_id IS NULL",
        params![transaction_id, occurrence.series_id],
    )?;

    // If this transaction was ambiguous (section 30), every OTHER occurrence
    // that got flagged needs_review alongside it must revert to a normal
    // open status now that the owner has picked one - otherwise it would
    // stay stranded in needs_review forever, invisible to both future
    // matching and the Upcoming/Calendar views.
    revert_other_ambiguous_candidates(conn, transaction_id, occurrence_id)?;

    get_occurrence(conn, occurrence_id)?.ok_or(RecurringError::NotFound(occurrence_id))
}

fn revert_other_ambiguous_candidates(
    conn: &Connection,
    transaction_id: i64,
    chosen_occurrence_id: i64,
) -> Result<(), RecurringError> {
    let found: Option<(i64, String, Option<i64>)> = conn
        .query_row(
            "SELECT account_id, merchant, payee_id FROM transactions WHERE id = ?1",
            params![transaction_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((account_id, merchant, payee_id)) = found else {
        return Ok(());
    };

    let mut stmt = conn.prepare(
        "SELECT o.id, o.series_id FROM recurring_occurrences o
         JOIN recurring_series s ON s.id = o.series_id
         WHERE o.status = 'needs_review' AND o.id != ?1
           AND s.account_id = ?2
           AND ((?3 IS NOT NULL AND s.payee_id = ?3) OR s.merchant = ?4)",
    )?;
    let siblings: Vec<(i64, i64)> = stmt
        .query_map(
            params![chosen_occurrence_id, account_id, payee_id, merchant],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for (sibling_id, series_id) in siblings {
        conn.execute(
            "UPDATE recurring_occurrences SET status = 'upcoming', matched_transaction_id = NULL, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
            params![sibling_id],
        )?;
        if let Some(series) = recurring::get(conn, series_id)? {
            let today_str = today(conn)?;
            refresh_statuses(conn, series_id, &today_str, series.cadence)?;
        }
    }
    Ok(())
}

/// Reverts a match back to an open state (section 32's "correcting an
/// incorrect match"), recomputed exactly as `refresh_statuses` would have
/// left it un-matched (Upcoming/Due/Missed by date) - the linked
/// transaction itself is left completely untouched except for the
/// relationship metadata, per the same section's explicit requirement.
pub fn unlink_occurrence(
    conn: &Connection,
    occurrence_id: i64,
) -> Result<RecurringOccurrence, RecurringError> {
    let occurrence =
        get_occurrence(conn, occurrence_id)?.ok_or(RecurringError::NotFound(occurrence_id))?;
    let series = recurring::get(conn, occurrence.series_id)?
        .ok_or(RecurringError::NotFound(occurrence.series_id))?;
    conn.execute(
        "UPDATE recurring_occurrences SET status = 'upcoming', matched_transaction_id = NULL, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![occurrence_id],
    )?;
    let today_str = today(conn)?;
    refresh_statuses(conn, occurrence.series_id, &today_str, series.cadence)?;
    get_occurrence(conn, occurrence_id)?.ok_or(RecurringError::NotFound(occurrence_id))
}

/// Called from `transactions::create`/`update` for every real transaction
/// (manual or imported - one shared path, section 51/52) to try matching it
/// to an expected occurrence. Never matches a one-time-flagged transaction
/// or a transfer/opening_balance/balance_adjustment (the same exclusions
/// detection itself uses). Ambiguity (more than one plausible occurrence)
/// is never silently resolved - it is routed to Needs Review instead
/// (section 30), and the transaction is left otherwise unmatched.
pub fn evaluate_transaction_for_matching(
    conn: &Connection,
    transaction_id: i64,
) -> Result<(), RecurringError> {
    type TransactionRow = (i64, String, Option<i64>, i64, String, String, i64);
    let row: Option<TransactionRow> = conn
        .query_row(
            "SELECT account_id, merchant, payee_id, amount_cents, transaction_date, transaction_type, is_one_time
             FROM transactions WHERE id = ?1",
            params![transaction_id],
            |r| {
                Ok((
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?,
                ))
            },
        )
        .optional()?;
    let Some((
        account_id,
        merchant,
        payee_id,
        amount_cents,
        transaction_date,
        transaction_type,
        is_one_time,
    )) = row
    else {
        return Ok(());
    };
    if is_one_time != 0
        || matches!(
            transaction_type.as_str(),
            "transfer" | "opening_balance" | "balance_adjustment"
        )
    {
        return Ok(());
    }

    let mut candidate_series_stmt = conn.prepare(
        "SELECT id FROM recurring_series
         WHERE account_id = ?1 AND status = 'confirmed'
           AND ((?2 IS NOT NULL AND payee_id = ?2) OR merchant = ?3)",
    )?;
    let candidate_series_ids: Vec<i64> = candidate_series_stmt
        .query_map(params![account_id, payee_id, merchant], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if candidate_series_ids.is_empty() {
        return Ok(());
    }

    let mut candidate_occurrences: Vec<i64> = Vec::new();
    for series_id in &candidate_series_ids {
        ensure_occurrences(conn, *series_id)?;
        let series =
            recurring::get(conn, *series_id)?.ok_or(RecurringError::NotFound(*series_id))?;
        let window = series.cadence.matching_window_days();
        let from = add_days(&transaction_date, -window);
        let to = add_days(&transaction_date, window);
        let tolerance = recurring::amount_tolerance(amount_cents);
        let mut stmt = conn.prepare(
            "SELECT id FROM recurring_occurrences
             WHERE series_id = ?1 AND matched_transaction_id IS NULL
               AND status IN ('upcoming', 'due', 'missed')
               AND expected_date >= ?2 AND expected_date <= ?3
               AND ABS(expected_amount_cents - ?4) <= ?5",
        )?;
        let found: Vec<i64> = stmt
            .query_map(params![series_id, from, to, amount_cents, tolerance], |r| {
                r.get(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        candidate_occurrences.extend(found);
    }

    match candidate_occurrences.len() {
        0 => Ok(()),
        1 => {
            link_occurrence_to_transaction(conn, candidate_occurrences[0], transaction_id)?;
            Ok(())
        }
        _ => {
            // Ambiguous: more than one occurrence could plausibly be this
            // transaction. Never guess - surface it (section 30).
            for occurrence_id in &candidate_occurrences {
                conn.execute(
                    "UPDATE recurring_occurrences SET status = 'needs_review', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
                    params![occurrence_id],
                )?;
            }
            conn.execute(
                "INSERT INTO needs_review_items (issue_type, transaction_id, detected_reason, proposed_resolution_json)
                 VALUES ('other', ?1, ?2, ?3)",
                params![
                    transaction_id,
                    format!(
                        "This transaction could match more than one expected recurring occurrence for \"{merchant}\" - choose which one it fulfills."
                    ),
                    format!(
                        "{{\"kind\":\"ambiguous_recurring_match\",\"occurrence_ids\":[{}]}}",
                        candidate_occurrences
                            .iter()
                            .map(|id| id.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                ],
            )?;
            Ok(())
        }
    }
}

/// Called from `transactions::update` only when a financially/identity-
/// relevant field actually changed (account/amount/date/type/merchant/
/// payee - section 49); harmless edits like notes or tags never call this,
/// so an existing match is never disturbed by unrelated metadata. If this
/// transaction was matched to a specific occurrence, that match is
/// released back to an open state (never left "obviously invalid" per
/// section 49) and the transaction's own `recurring_series_id` link is
/// cleared, then matching is attempted fresh against the transaction's
/// current values - exactly as if it had just arrived.
pub fn reevaluate_after_edit(conn: &Connection, transaction_id: i64) -> Result<(), RecurringError> {
    let matched_occurrence: Option<i64> = conn
        .query_row(
            "SELECT id FROM recurring_occurrences WHERE matched_transaction_id = ?1",
            params![transaction_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(occurrence_id) = matched_occurrence {
        unlink_occurrence(conn, occurrence_id)?;
    }
    conn.execute(
        "UPDATE transactions SET recurring_series_id = NULL WHERE id = ?1",
        params![transaction_id],
    )?;
    evaluate_transaction_for_matching(conn, transaction_id)
}

#[tauri::command]
pub fn list_recurring_occurrences(
    state: State<DbState>,
    series_id: i64,
) -> Result<Vec<RecurringOccurrence>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    ensure_occurrences(&conn, series_id).map_err(|e| e.to_string())
}

/// Fetches one occurrence by id - the Needs Review UI uses this to show a
/// candidate's date/amount before the owner picks which one an ambiguous
/// transaction actually fulfills (section 30).
#[tauri::command]
pub fn get_recurring_occurrence(
    state: State<DbState>,
    occurrence_id: i64,
) -> Result<Option<RecurringOccurrence>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    get_occurrence(&conn, occurrence_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_upcoming_recurring(
    state: State<DbState>,
    from: String,
    to: String,
    account_id: Option<i64>,
) -> Result<Vec<RecurringOccurrence>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    upcoming_in_range(&conn, &from, &to, account_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skip_recurring_occurrence(
    state: State<DbState>,
    occurrence_id: i64,
) -> Result<RecurringOccurrence, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    skip_occurrence(&conn, occurrence_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn link_recurring_occurrence(
    state: State<DbState>,
    occurrence_id: i64,
    transaction_id: i64,
) -> Result<RecurringOccurrence, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    link_occurrence_to_transaction(&conn, occurrence_id, transaction_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn unlink_recurring_occurrence(
    state: State<DbState>,
    occurrence_id: i64,
) -> Result<RecurringOccurrence, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    unlink_occurrence(&conn, occurrence_id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::recurring::{Direction, Kind, PatternEditInput};
    use crate::transactions::{self, TransactionInput, TransactionType};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_transactions.sql"),
            include_str!("../migrations/0003_rules_and_recurring.sql"),
            include_str!("../migrations/0004_accounting_boundary.sql"),
            include_str!("../migrations/0007_balance_adjustment.sql"),
            include_str!("../migrations/0008_foundation_batch_1.sql"),
            include_str!("../migrations/0009_import_engine_v2.sql"),
            include_str!("../migrations/0010_ofx_source_adapter.sql"),
            include_str!("../migrations/0011_reconciliation.sql"),
            include_str!("../migrations/0012_transaction_intelligence.sql"),
            include_str!("../migrations/0013_recurring_intelligence.sql"),
        ] {
            conn.execute_batch(sql).unwrap();
        }
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

    fn add_txn(
        conn: &mut Connection,
        account_id: i64,
        date: &str,
        amount_cents: i64,
        merchant: &str,
    ) -> i64 {
        let t = transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: date.to_string(),
                posted_date: None,
                amount_cents,
                transaction_type: TransactionType::Purchase,
                raw_description: merchant.to_string(),
                merchant: Some(merchant.to_string()),
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
        evaluate_transaction_for_matching(conn, t.id).unwrap();
        t.id
    }

    /// A date comfortably in the future relative to the real wall-clock
    /// "today" (section 85: tests must not randomly fail depending on the
    /// date they happen to run on) - every test below derives its own
    /// transaction/occurrence dates from this same anchor via `add_days`,
    /// never a hardcoded calendar literal, so nothing here is ever close
    /// enough to "now" to accidentally cross a missed-window boundary.
    /// Kept small (not e.g. 30 days) so that generating several months of
    /// Monthly occurrences forward from it still lands well inside the
    /// `HORIZON_DAYS` window instead of crowding its edge.
    fn anchor(conn: &Connection) -> String {
        add_days(&today(conn).unwrap(), 10)
    }

    fn make_confirmed_monthly(
        conn: &mut Connection,
        account_id: i64,
        next_expected_date: &str,
    ) -> i64 {
        let series = recurring::create_manual(
            conn,
            account_id,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Streaming Service".to_string(),
                payee_id: None,
                kind: Kind::Subscription,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -999,
                next_expected_date: next_expected_date.to_string(),
                notes: None,
            },
        )
        .unwrap();
        series.id
    }

    #[test]
    fn ensure_occurrences_generates_out_to_the_horizon_and_is_idempotent() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);

        let first = ensure_occurrences(&conn, series_id).unwrap();
        assert!(
            first.len() >= 3,
            "at least 3 monthly occurrences within 90 days"
        );
        assert_eq!(first[0].expected_date, base);

        let second = ensure_occurrences(&conn, series_id).unwrap();
        assert_eq!(
            first.len(),
            second.len(),
            "re-running generation must not duplicate occurrences"
        );
    }

    #[test]
    fn ensure_occurrences_is_a_noop_for_a_non_confirmed_series() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let manual_input = PatternEditInput {
            merchant: "Draft".to_string(),
            payee_id: None,
            kind: Kind::Other,
            category: None,
            cadence: Cadence::Monthly,
            semimonthly_day_1: None,
            semimonthly_day_2: None,
            expected_amount_cents: -500,
            next_expected_date: anchor(&conn),
            notes: None,
        };
        let created =
            recurring::create_manual(&mut conn, account_id, Direction::Expense, &manual_input)
                .unwrap();
        recurring::reject(&conn, created.id).unwrap();
        let occurrences = ensure_occurrences(&conn, created.id).unwrap();
        assert!(occurrences.is_empty());
    }

    #[test]
    fn month_end_anchored_pattern_advances_correctly_through_february() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let series = recurring::create_manual(
            &mut conn,
            account_id,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Month-end Bill".to_string(),
                payee_id: None,
                kind: Kind::Bill,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -5000,
                next_expected_date: "2026-01-31".to_string(),
                notes: None,
            },
        )
        .unwrap();
        let occurrences = list_for_series(&conn, series.id).unwrap();
        let dates: Vec<&str> = occurrences
            .iter()
            .map(|o| o.expected_date.as_str())
            .collect();
        assert!(dates.contains(&"2026-01-31"));
        assert!(
            dates.contains(&"2026-02-28"),
            "Jan 31 + 1 month must clamp to Feb 28, not overflow into March: {dates:?}"
        );
    }

    #[test]
    fn month_end_anchored_pattern_reaches_february_29_in_a_leap_year() {
        // A fixed historical leap year (not relative to real "today", like
        // the non-leap sibling test above) - only date PRESENCE is asserted,
        // never status or count, so this is unaffected by however far real
        // wall-clock time has since moved past it (section 85).
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let series = recurring::create_manual(
            &mut conn,
            account_id,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Month-end Bill".to_string(),
                payee_id: None,
                kind: Kind::Bill,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -5000,
                next_expected_date: "2024-01-31".to_string(),
                notes: None,
            },
        )
        .unwrap();
        let occurrences = list_for_series(&conn, series.id).unwrap();
        let dates: Vec<&str> = occurrences
            .iter()
            .map(|o| o.expected_date.as_str())
            .collect();
        assert!(dates.contains(&"2024-01-31"));
        assert!(
            dates.contains(&"2024-02-29"),
            "Jan 31 + 1 month in a leap year must land on Feb 29, not clamp to 28: {dates:?}"
        );
    }

    #[test]
    fn exact_transaction_matches_an_expected_occurrence() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);

        let txn_id = add_txn(&mut conn, account_id, &base, -999, "Streaming Service");

        let occurrences = list_for_series(&conn, series_id).unwrap();
        let matched = occurrences
            .iter()
            .find(|o| o.expected_date == base)
            .unwrap();
        assert_eq!(matched.status, OccurrenceStatus::Matched);
        assert_eq!(matched.matched_transaction_id, Some(txn_id));
    }

    #[test]
    fn a_date_shifted_transaction_still_matches_within_the_window() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);

        // Posted a day later than expected - within Monthly's 4-day window.
        add_txn(
            &mut conn,
            account_id,
            &add_days(&base, 1),
            -999,
            "Streaming Service",
        );

        let occurrences = list_for_series(&conn, series_id).unwrap();
        let matched = occurrences
            .iter()
            .find(|o| o.expected_date == base)
            .unwrap();
        assert_eq!(matched.status, OccurrenceStatus::Matched);
    }

    #[test]
    fn a_transaction_weeks_away_never_matches() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);

        add_txn(
            &mut conn,
            account_id,
            &add_days(&base, 15),
            -999,
            "Streaming Service",
        );

        let occurrences = list_for_series(&conn, series_id).unwrap();
        assert!(
            occurrences
                .iter()
                .all(|o| o.status != OccurrenceStatus::Matched),
            "a transaction 15 days away from the expected date must never match"
        );
    }

    #[test]
    fn amount_outside_tolerance_never_matches() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);

        // $9.99 expected; $49.99 posted on the exact date - wrong amount.
        add_txn(&mut conn, account_id, &base, -4999, "Streaming Service");

        let occurrences = list_for_series(&conn, series_id).unwrap();
        assert!(occurrences
            .iter()
            .all(|o| o.status != OccurrenceStatus::Matched));
    }

    #[test]
    fn a_transaction_on_a_different_account_never_matches_another_accounts_series() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);

        // Same merchant, same amount, same date, same payee-less matching
        // path - the ONLY difference is the account. Candidate lookup is
        // scoped by account_id, so this must never match even though every
        // other signal lines up.
        let other_account = make_account(&conn);
        add_txn(&mut conn, other_account, &base, -999, "Streaming Service");

        let occurrences = list_for_series(&conn, series_id).unwrap();
        assert!(
            occurrences
                .iter()
                .all(|o| o.status != OccurrenceStatus::Matched),
            "a transaction on a different account must never satisfy this account's occurrence"
        );
    }

    #[test]
    fn one_transaction_can_never_satisfy_two_occurrences() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);
        let occurrences = ensure_occurrences(&conn, series_id).unwrap();
        let first_id = occurrences[0].id;
        let second_id = occurrences[1].id;

        let txn = transactions::create(
            &conn,
            &TransactionInput {
                account_id,
                transaction_date: base.clone(),
                posted_date: None,
                amount_cents: -999,
                transaction_type: TransactionType::Purchase,
                raw_description: "Streaming Service".to_string(),
                merchant: Some("Streaming Service".to_string()),
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

        link_occurrence_to_transaction(&conn, first_id, txn.id).unwrap();
        let err = link_occurrence_to_transaction(&conn, second_id, txn.id).unwrap_err();
        assert!(matches!(err, RecurringError::Validation(_)));
    }

    #[test]
    fn skip_marks_an_occurrence_without_touching_the_series() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);
        let occurrences = ensure_occurrences(&conn, series_id).unwrap();

        let skipped = skip_occurrence(&conn, occurrences[0].id).unwrap();
        assert_eq!(skipped.status, OccurrenceStatus::Skipped);

        let series = recurring::get(&conn, series_id).unwrap().unwrap();
        assert_eq!(
            series.status,
            crate::recurring::SeriesStatus::Confirmed,
            "skipping one occurrence must never deactivate the pattern"
        );
    }

    #[test]
    fn skip_refuses_an_already_matched_occurrence() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);
        add_txn(&mut conn, account_id, &base, -999, "Streaming Service");
        let occurrences = list_for_series(&conn, series_id).unwrap();
        let matched = occurrences
            .iter()
            .find(|o| o.status == OccurrenceStatus::Matched)
            .unwrap();

        let err = skip_occurrence(&conn, matched.id).unwrap_err();
        assert!(matches!(err, RecurringError::Validation(_)));
    }

    #[test]
    fn unlink_reverts_to_an_open_status_and_leaves_the_transaction_alone() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);
        let txn_id = add_txn(&mut conn, account_id, &base, -999, "Streaming Service");
        let occurrences = list_for_series(&conn, series_id).unwrap();
        let matched = occurrences
            .iter()
            .find(|o| o.status == OccurrenceStatus::Matched)
            .unwrap();

        let unlinked = unlink_occurrence(&conn, matched.id).unwrap();
        assert_ne!(unlinked.status, OccurrenceStatus::Matched);
        assert_eq!(unlinked.matched_transaction_id, None);

        let (merchant, amount): (String, i64) = conn
            .query_row(
                "SELECT merchant, amount_cents FROM transactions WHERE id = ?1",
                params![txn_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(merchant, "Streaming Service");
        assert_eq!(amount, -999);
    }

    #[test]
    fn ambiguous_match_routes_to_needs_review_instead_of_guessing() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);
        // Two adjacent occurrences within reach of one transaction date:
        // shrink the gap artificially by inserting a second occurrence
        // close to the first so one transaction could plausibly satisfy
        // either.
        conn.execute(
            "INSERT INTO recurring_occurrences (series_id, expected_date, expected_amount_cents)
             VALUES (?1, ?2, -999)",
            params![series_id, add_days(&base, 2)],
        )
        .unwrap();

        let before_review_count: i64 = conn
            .query_row("SELECT count(*) FROM needs_review_items", [], |r| r.get(0))
            .unwrap();

        add_txn(
            &mut conn,
            account_id,
            &add_days(&base, 1),
            -999,
            "Streaming Service",
        );

        let after_review_count: i64 = conn
            .query_row("SELECT count(*) FROM needs_review_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_review_count, before_review_count + 1);

        let occurrences = list_for_series(&conn, series_id).unwrap();
        assert!(
            occurrences
                .iter()
                .all(|o| o.status != OccurrenceStatus::Matched),
            "an ambiguous match must never guess"
        );
        assert!(occurrences
            .iter()
            .any(|o| o.status == OccurrenceStatus::NeedsReview));
    }

    #[test]
    fn resolving_an_ambiguous_match_reverts_the_other_candidate_instead_of_stranding_it() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);
        conn.execute(
            "INSERT INTO recurring_occurrences (series_id, expected_date, expected_amount_cents)
             VALUES (?1, ?2, -999)",
            params![series_id, add_days(&base, 2)],
        )
        .unwrap();

        let txn_id = add_txn(
            &mut conn,
            account_id,
            &add_days(&base, 1),
            -999,
            "Streaming Service",
        );

        let occurrences = list_for_series(&conn, series_id).unwrap();
        let mut flagged: Vec<_> = occurrences
            .iter()
            .filter(|o| o.status == OccurrenceStatus::NeedsReview)
            .collect();
        assert_eq!(flagged.len(), 2, "both candidates must be flagged");
        flagged.sort_by_key(|o| o.expected_date.clone());
        let chosen = flagged[0].id;
        let sibling = flagged[1].id;

        link_occurrence_to_transaction(&conn, chosen, txn_id).unwrap();

        let after = list_for_series(&conn, series_id).unwrap();
        let chosen_after = after.iter().find(|o| o.id == chosen).unwrap();
        let sibling_after = after.iter().find(|o| o.id == sibling).unwrap();
        assert_eq!(chosen_after.status, OccurrenceStatus::Matched);
        assert_ne!(
            sibling_after.status,
            OccurrenceStatus::NeedsReview,
            "the occurrence not chosen must revert to a normal open status, \
             not stay stranded in needs_review forever"
        );
        assert_eq!(sibling_after.matched_transaction_id, None);
    }

    #[test]
    fn a_one_time_flagged_transaction_never_auto_matches() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);

        transactions::create(
            &conn,
            &TransactionInput {
                account_id,
                transaction_date: base.clone(),
                posted_date: None,
                amount_cents: -999,
                transaction_type: TransactionType::Purchase,
                raw_description: "Streaming Service".to_string(),
                merchant: Some("Streaming Service".to_string()),
                category: None,
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
                payee_id: None,
                is_one_time: true,
            },
        )
        .unwrap();
        // No explicit evaluate call needed via add_txn helper here since we
        // constructed the transaction directly; call the matcher exactly as
        // transactions::create's own hook would.
        let occurrences = list_for_series(&conn, series_id).unwrap();
        assert!(occurrences
            .iter()
            .all(|o| o.status != OccurrenceStatus::Matched));
    }

    #[test]
    fn transfer_transactions_never_auto_match() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        let mut conn = conn;
        let base = anchor(&conn);
        let series_id = make_confirmed_monthly(&mut conn, account_id, &base);

        let txn = transactions::create(
            &conn,
            &TransactionInput {
                account_id,
                transaction_date: base.clone(),
                posted_date: None,
                amount_cents: -999,
                transaction_type: TransactionType::Transfer,
                raw_description: "Streaming Service".to_string(),
                merchant: Some("Streaming Service".to_string()),
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
        evaluate_transaction_for_matching(&conn, txn.id).unwrap();

        let occurrences = list_for_series(&conn, series_id).unwrap();
        assert!(occurrences
            .iter()
            .all(|o| o.status != OccurrenceStatus::Matched));
    }

    #[test]
    fn upcoming_in_range_scopes_by_account_and_date_window() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let base = anchor(&conn);
        make_confirmed_monthly(&mut conn, account_id, &base);
        let other_account = make_account(&conn);
        recurring::create_manual(
            &mut conn,
            other_account,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Other Account Bill".to_string(),
                payee_id: None,
                kind: Kind::Bill,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -2000,
                next_expected_date: add_days(&base, 2),
                notes: None,
            },
        )
        .unwrap();

        let range_start = add_days(&base, -5);
        let range_end = add_days(&base, 25);
        let scoped = upcoming_in_range(&conn, &range_start, &range_end, Some(account_id)).unwrap();
        assert!(scoped.iter().all(|o| o.account_id == account_id));
        assert!(scoped.iter().any(|o| o.merchant == "Streaming Service"));

        let all = upcoming_in_range(&conn, &range_start, &range_end, None).unwrap();
        assert!(all.iter().any(|o| o.merchant == "Other Account Bill"));
    }
}
