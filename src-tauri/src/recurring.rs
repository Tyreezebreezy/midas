//! Recurring-obligation detection (product handoff section 10). Detection
//! only ever produces `proposed` rows - never silent truth - and the user
//! must explicitly confirm or reject each one. No AI, nothing scheduled in
//! the background: detection runs when the user asks for it.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;

use crate::db::DbState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cadence {
    Weekly,
    Biweekly,
    Monthly,
    Quarterly,
    Annual,
}

impl Cadence {
    fn as_sql_str(self) -> &'static str {
        match self {
            Cadence::Weekly => "weekly",
            Cadence::Biweekly => "biweekly",
            Cadence::Monthly => "monthly",
            Cadence::Quarterly => "quarterly",
            Cadence::Annual => "annual",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "weekly" => Cadence::Weekly,
            "biweekly" => Cadence::Biweekly,
            "monthly" => Cadence::Monthly,
            "quarterly" => Cadence::Quarterly,
            "annual" => Cadence::Annual,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown cadence '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }

    /// (min_days, max_days, typical_days) tolerance window for classifying
    /// a gap between two occurrences as this cadence. Ranges are disjoint by
    /// construction so a gap classifies to at most one cadence.
    fn day_range(self) -> (i64, i64, i64) {
        match self {
            Cadence::Weekly => (5, 9, 7),
            Cadence::Biweekly => (10, 18, 14),
            Cadence::Monthly => (25, 35, 30),
            Cadence::Quarterly => (80, 100, 91),
            Cadence::Annual => (350, 380, 365),
        }
    }

    fn classify(gap_days: i64) -> Option<Cadence> {
        for c in [
            Cadence::Weekly,
            Cadence::Biweekly,
            Cadence::Monthly,
            Cadence::Quarterly,
            Cadence::Annual,
        ] {
            let (min, max, _) = c.day_range();
            if (min..=max).contains(&gap_days) {
                return Some(c);
            }
        }
        None
    }
}

use crate::dates::{add_days, days_between};

fn amount_tolerance(reference_cents: i64) -> i64 {
    (reference_cents.abs() / 5).max(200) // 20% of magnitude, or $2.00, whichever is larger
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesStatus {
    Proposed,
    Confirmed,
    Rejected,
}

impl SeriesStatus {
    fn as_sql_str(self) -> &'static str {
        match self {
            SeriesStatus::Proposed => "proposed",
            SeriesStatus::Confirmed => "confirmed",
            SeriesStatus::Rejected => "rejected",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "proposed" => SeriesStatus::Proposed,
            "confirmed" => SeriesStatus::Confirmed,
            "rejected" => SeriesStatus::Rejected,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown status '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RecurringSeries {
    pub id: i64,
    pub account_id: i64,
    pub merchant: String,
    pub cadence: Cadence,
    pub expected_amount_cents: i64,
    pub next_expected_date: String,
    pub category: Option<String>,
    pub status: SeriesStatus,
    pub created_at: String,
    pub updated_at: String,
    pub last_occurrence_date: Option<String>,
    pub last_amount_cents: Option<i64>,
    pub is_price_changed: bool,
    pub is_possibly_missed: bool,
}

#[derive(Debug)]
pub enum RecurringError {
    Db(rusqlite::Error),
    NotFound(i64),
}

impl std::fmt::Display for RecurringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecurringError::Db(e) => write!(f, "database error: {e}"),
            RecurringError::NotFound(id) => write!(f, "recurring series {id} not found"),
        }
    }
}

impl From<rusqlite::Error> for RecurringError {
    fn from(e: rusqlite::Error) -> Self {
        RecurringError::Db(e)
    }
}

const COLUMNS: &str =
    "id, account_id, merchant, cadence, expected_amount_cents, next_expected_date, category, status, created_at, updated_at";

fn today(conn: &Connection) -> rusqlite::Result<String> {
    conn.query_row("SELECT date('now')", [], |row| row.get(0))
}

fn hydrate(conn: &Connection, row: &rusqlite::Row) -> rusqlite::Result<RecurringSeries> {
    let id: i64 = row.get("id")?;
    let expected_amount_cents: i64 = row.get("expected_amount_cents")?;
    let status = SeriesStatus::from_sql_str(&row.get::<_, String>("status")?)?;
    let next_expected_date: String = row.get("next_expected_date")?;

    let (last_occurrence_date, last_amount_cents): (Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT transaction_date, amount_cents FROM transactions
             WHERE recurring_series_id = ?1 ORDER BY transaction_date DESC LIMIT 1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or((None, None));

    let is_price_changed = last_amount_cents
        .map(|last| (last - expected_amount_cents).abs() > amount_tolerance(expected_amount_cents))
        .unwrap_or(false);
    let is_possibly_missed = status == SeriesStatus::Confirmed && {
        let today = today(conn)?;
        days_between(&next_expected_date, &today) > 5
    };

    Ok(RecurringSeries {
        id,
        account_id: row.get("account_id")?,
        merchant: row.get("merchant")?,
        cadence: Cadence::from_sql_str(&row.get::<_, String>("cadence")?)?,
        expected_amount_cents,
        next_expected_date,
        category: row.get("category")?,
        status,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        last_occurrence_date,
        last_amount_cents,
        is_price_changed,
        is_possibly_missed,
    })
}

fn get(conn: &Connection, id: i64) -> Result<Option<RecurringSeries>, RecurringError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM recurring_series WHERE id = ?1"),
        params![id],
        |row| hydrate(conn, row),
    )
    .optional()
    .map_err(RecurringError::from)
}

pub fn list(
    conn: &Connection,
    account_id: Option<i64>,
) -> Result<Vec<RecurringSeries>, RecurringError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM recurring_series {} ORDER BY status, next_expected_date",
        if account_id.is_some() {
            "WHERE account_id = ?1"
        } else {
            ""
        }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = if let Some(id) = account_id {
        stmt.query_map(params![id], |row| hydrate(conn, row))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map([], |row| hydrate(conn, row))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

pub fn confirm(conn: &Connection, id: i64) -> Result<RecurringSeries, RecurringError> {
    let series = get(conn, id)?.ok_or(RecurringError::NotFound(id))?;
    let affected = conn.execute(
        "UPDATE recurring_series SET status = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![id, SeriesStatus::Confirmed.as_sql_str()],
    )?;
    if affected == 0 {
        return Err(RecurringError::NotFound(id));
    }
    conn.execute(
        "UPDATE transactions SET recurring_series_id = ?1
         WHERE account_id = ?2 AND merchant = ?3 AND recurring_series_id IS NULL",
        params![id, series.account_id, series.merchant],
    )?;
    get(conn, id)?.ok_or(RecurringError::NotFound(id))
}

pub fn reject(conn: &Connection, id: i64) -> Result<RecurringSeries, RecurringError> {
    let affected = conn.execute(
        "UPDATE recurring_series SET status = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![id, SeriesStatus::Rejected.as_sql_str()],
    )?;
    if affected == 0 {
        return Err(RecurringError::NotFound(id));
    }
    get(conn, id)?.ok_or(RecurringError::NotFound(id))
}

fn amounts_consistent(amounts: &[i64]) -> bool {
    let min = *amounts.iter().min().unwrap();
    let max = *amounts.iter().max().unwrap();
    let avg = amounts.iter().sum::<i64>() / amounts.len() as i64;
    (max - min) <= amount_tolerance(avg)
}

/// Scans an account's transaction history for repeating merchants and
/// proposes new `recurring_series` rows for patterns that look confident:
/// at least 3 occurrences, consistent gap-between-dates (all classifying to
/// the same cadence), and consistent amounts. Skips any merchant that
/// already has a series (of any status) for this account, so re-running
/// detection is idempotent rather than piling up duplicate proposals.
pub fn detect_candidates(
    conn: &Connection,
    account_id: i64,
) -> Result<Vec<RecurringSeries>, RecurringError> {
    // opening_balance is structurally at most one row (never reaches the
    // 3-occurrence bar anyway), but balance_adjustment can legitimately
    // recur - one per reconciliation date, often sharing the same merchant
    // label - so it must be excluded explicitly (ADR-0009).
    let mut stmt = conn.prepare(
        "SELECT merchant, transaction_date, amount_cents, category FROM transactions
         WHERE account_id = ?1 AND is_archived = 0 AND trim(merchant) != ''
             AND transaction_type NOT IN ('opening_balance', 'balance_adjustment')
         ORDER BY merchant, transaction_date",
    )?;
    let rows: Vec<(String, String, i64, Option<String>)> = stmt
        .query_map(params![account_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut existing =
        conn.prepare("SELECT DISTINCT merchant FROM recurring_series WHERE account_id = ?1")?;
    let already_proposed: std::collections::HashSet<String> = existing
        .query_map(params![account_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .collect();

    let mut by_merchant: HashMap<String, Vec<(String, i64, Option<String>)>> = HashMap::new();
    for (merchant, date, amount, category) in rows {
        by_merchant
            .entry(merchant)
            .or_default()
            .push((date, amount, category));
    }

    let mut created = Vec::new();
    for (merchant, mut occurrences) in by_merchant {
        if already_proposed.contains(&merchant) || occurrences.len() < 3 {
            continue;
        }
        occurrences.sort_by(|a, b| a.0.cmp(&b.0));

        let gaps: Vec<i64> = occurrences
            .windows(2)
            .map(|w| days_between(&w[0].0, &w[1].0))
            .collect();
        let cadences: Option<Vec<Cadence>> = gaps.iter().map(|&g| Cadence::classify(g)).collect();
        let Some(cadences) = cadences else { continue };
        let cadence = cadences[0];
        if !cadences.iter().all(|&c| c == cadence) {
            continue;
        }

        let amounts: Vec<i64> = occurrences.iter().map(|o| o.1).collect();
        if !amounts_consistent(&amounts) {
            continue;
        }

        let last = occurrences.last().unwrap();
        let (_, typical_days_max, typical_days) = cadence.day_range();
        let _ = typical_days_max;
        let next_expected_date = add_days(&last.0, typical_days);

        let id: i64 = conn.query_row(
            "INSERT INTO recurring_series (account_id, merchant, cadence, expected_amount_cents, next_expected_date, category)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
            params![account_id, merchant, cadence.as_sql_str(), last.1, next_expected_date, last.2],
            |row| row.get(0),
        )?;
        created.push(get(conn, id)?.ok_or(RecurringError::NotFound(id))?);
    }

    created.sort_by(|a, b| a.merchant.cmp(&b.merchant));
    Ok(created)
}

#[tauri::command]
pub fn list_recurring(
    state: State<DbState>,
    account_id: Option<i64>,
) -> Result<Vec<RecurringSeries>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list(&conn, account_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn detect_recurring(
    state: State<DbState>,
    account_id: i64,
) -> Result<Vec<RecurringSeries>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    detect_candidates(&conn, account_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn confirm_recurring(state: State<DbState>, id: i64) -> Result<RecurringSeries, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    confirm(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reject_recurring(state: State<DbState>, id: i64) -> Result<RecurringSeries, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    reject(&conn, id).map_err(|e| e.to_string())
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
        conn
    }

    fn make_account(conn: &Connection) -> i64 {
        accounts::create(
            conn,
            &AccountInput {
                name: "Test".to_string(),
                institution: None,
                account_type: AccountType::Checking,
                include_in_net_worth: true,
                notes: None,
            },
        )
        .unwrap()
        .id
    }

    fn add_txn(conn: &Connection, account_id: i64, date: &str, amount_cents: i64, merchant: &str) {
        transactions::create(
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
            },
        )
        .unwrap();
    }

    #[test]
    fn cadence_classifies_typical_gaps_correctly() {
        assert_eq!(Cadence::classify(7), Some(Cadence::Weekly));
        assert_eq!(Cadence::classify(14), Some(Cadence::Biweekly));
        assert_eq!(Cadence::classify(30), Some(Cadence::Monthly));
        assert_eq!(Cadence::classify(31), Some(Cadence::Monthly));
        assert_eq!(Cadence::classify(91), Some(Cadence::Quarterly));
        assert_eq!(Cadence::classify(365), Some(Cadence::Annual));
        assert_eq!(Cadence::classify(50), None); // doesn't fit any window
    }

    #[test]
    fn detects_a_confident_monthly_subscription() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-03-05", -999, "Streaming Service");

        let candidates = detect_candidates(&conn, account_id).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].merchant, "Streaming Service");
        assert_eq!(candidates[0].cadence, Cadence::Monthly);
        assert_eq!(candidates[0].expected_amount_cents, -999);
        assert_eq!(candidates[0].status, SeriesStatus::Proposed);
    }

    #[test]
    fn tolerates_a_small_price_change_across_occurrences() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-03-05", -1099, "Streaming Service"); // price went up

        let candidates = detect_candidates(&conn, account_id).unwrap();
        assert_eq!(
            candidates.len(),
            1,
            "a $1 change on a $9.99 charge is within tolerance"
        );
    }

    #[test]
    fn does_not_propose_fewer_than_three_occurrences() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-05", -999, "Streaming Service");
        assert!(detect_candidates(&conn, account_id).unwrap().is_empty());
    }

    #[test]
    fn does_not_propose_irregular_gaps() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Irregular Store");
        add_txn(&conn, account_id, "2026-01-20", -999, "Irregular Store");
        add_txn(&conn, account_id, "2026-03-25", -999, "Irregular Store");
        assert!(detect_candidates(&conn, account_id).unwrap().is_empty());
    }

    #[test]
    fn does_not_propose_wildly_inconsistent_amounts() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Variable Merchant");
        add_txn(&conn, account_id, "2026-02-05", -4500, "Variable Merchant");
        add_txn(&conn, account_id, "2026-03-05", -1200, "Variable Merchant");
        assert!(detect_candidates(&conn, account_id).unwrap().is_empty());
    }

    #[test]
    fn detection_is_idempotent_across_reruns() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-03-05", -999, "Streaming Service");

        let first = detect_candidates(&conn, account_id).unwrap();
        assert_eq!(first.len(), 1);
        let second = detect_candidates(&conn, account_id).unwrap();
        assert!(
            second.is_empty(),
            "already-proposed merchants must not be proposed again"
        );
        assert_eq!(list(&conn, Some(account_id)).unwrap().len(), 1);
    }

    #[test]
    fn balance_adjustments_are_never_proposed_as_a_recurring_pattern() {
        // ADR-0009 regression: unlike opening_balance (structurally at most
        // one row), balance_adjustment can legitimately recur - one per
        // reconciliation - and could otherwise look exactly like a
        // consistent monthly pattern to this detector.
        let conn = test_conn();
        let account_id = make_account(&conn);
        crate::accounting::reconcile_balance(&conn, account_id, 500, "2026-01-01").unwrap();
        crate::accounting::reconcile_balance(&conn, account_id, 1500, "2026-01-31").unwrap();
        crate::accounting::reconcile_balance(&conn, account_id, 2500, "2026-03-02").unwrap();
        crate::accounting::reconcile_balance(&conn, account_id, 3500, "2026-04-01").unwrap();

        let candidates = detect_candidates(&conn, account_id).unwrap();
        assert!(
            candidates.is_empty(),
            "opening_balance/balance_adjustment must never be proposed as a recurring series"
        );
    }

    #[test]
    fn confirm_links_matching_transactions_and_reject_does_not() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-03-05", -999, "Streaming Service");
        let candidate = detect_candidates(&conn, account_id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

        let confirmed = confirm(&conn, candidate.id).unwrap();
        assert_eq!(confirmed.status, SeriesStatus::Confirmed);
        assert_eq!(
            confirmed.last_occurrence_date.as_deref(),
            Some("2026-03-05")
        );

        let linked_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM transactions WHERE recurring_series_id = ?1",
                params![candidate.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(linked_count, 3);
    }

    #[test]
    fn reject_does_not_link_transactions() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-03-05", -999, "Streaming Service");
        let candidate = detect_candidates(&conn, account_id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

        let rejected = reject(&conn, candidate.id).unwrap();
        assert_eq!(rejected.status, SeriesStatus::Rejected);
        let linked_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM transactions WHERE recurring_series_id = ?1",
                params![candidate.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(linked_count, 0);
    }

    #[test]
    fn price_change_flag_reflects_the_most_recent_linked_amount() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-03-05", -999, "Streaming Service");
        let candidate = detect_candidates(&conn, account_id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        confirm(&conn, candidate.id).unwrap();

        // A big jump arrives and gets linked via a later confirm-triggered
        // re-link is not automatic, so link it directly to simulate a
        // future import already having matched this series.
        add_txn(&conn, account_id, "2026-04-05", -2999, "Streaming Service");
        conn.execute(
            "UPDATE transactions SET recurring_series_id = ?1 WHERE transaction_date = '2026-04-05'",
            params![candidate.id],
        )
        .unwrap();

        let refreshed = get(&conn, candidate.id).unwrap().unwrap();
        assert!(refreshed.is_price_changed);
        assert_eq!(refreshed.last_amount_cents, Some(-2999));
    }
}
