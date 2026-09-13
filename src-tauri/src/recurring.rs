//! Recurring pattern detection and lifecycle (Batch 6, specs/phase-014).
//! Extends Phase 3's foundation (merchant-based detection, >=3 consistent
//! occurrences, proposed/confirmed/rejected) rather than replacing it - that
//! shape already works. Detection only ever produces `proposed` rows or
//! updates the owner's own confirmed pattern - never silent truth - and the
//! owner must explicitly confirm, reject, or edit. No AI, nothing scheduled
//! in the background: detection and occurrence generation both run lazily,
//! on demand, from whatever the frontend is already asking for.
//!
//! Three distinct concepts, never the same row (product handoff section 4):
//! a `RecurringSeries` (this file) is MIDAS's belief about a repeating
//! pattern; a `RecurringOccurrence` (`recurring_occurrences.rs`) is a
//! projection of one future/past expected instance of it, never a ledger
//! movement; a `Transaction` (`transactions.rs`) is what actually happened.
//! Linking an occurrence to a transaction never edits, creates, or
//! duplicates that transaction.

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
    /// Two fixed days per month (`semimonthly_day_1`/`_2` on the series) -
    /// distinct from Biweekly (every 14 days, drifts across months).
    /// Paychecks commonly use this shape (pay_cycle.rs's own
    /// `PaySchedule::Semimonthly` already established the same two-field
    /// representation).
    Semimonthly,
}

impl Cadence {
    fn as_sql_str(self) -> &'static str {
        match self {
            Cadence::Weekly => "weekly",
            Cadence::Biweekly => "biweekly",
            Cadence::Monthly => "monthly",
            Cadence::Quarterly => "quarterly",
            Cadence::Annual => "annual",
            Cadence::Semimonthly => "semimonthly",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "weekly" => Cadence::Weekly,
            "biweekly" => Cadence::Biweekly,
            "monthly" => Cadence::Monthly,
            "quarterly" => Cadence::Quarterly,
            "annual" => Cadence::Annual,
            "semimonthly" => Cadence::Semimonthly,
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
    /// construction so a gap classifies to at most one cadence. Semimonthly
    /// has no single window - its gaps alternate between two lengths - so
    /// it is classified separately (`classify_semimonthly`) and is never
    /// returned here.
    fn day_range(self) -> (i64, i64, i64) {
        match self {
            Cadence::Weekly => (5, 9, 7),
            Cadence::Biweekly => (10, 18, 14),
            Cadence::Monthly => (25, 35, 30),
            Cadence::Quarterly => (80, 100, 91),
            Cadence::Annual => (350, 380, 365),
            Cadence::Semimonthly => (0, 0, 0),
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

    /// The date-tolerance window used to decide whether a real transaction
    /// could satisfy an occurrence expected on a given date (section 29) -
    /// deliberately tighter than `day_range`'s detection-time classification
    /// window, and asymmetric from cadence to cadence: a weekly charge
    /// shifting 4 days could collide with the *next* week's occurrence, so
    /// it gets a narrower matching window than a monthly one.
    pub(crate) fn matching_window_days(self) -> i64 {
        match self {
            Cadence::Weekly => 2,
            _ => 4,
        }
    }
}

use crate::dates::{add_days, add_months, day_in_month, days_between};

/// Tolerance used when *detecting* whether a run of historical amounts is
/// consistent enough to be one recurring pattern: 40% of the average, or
/// $5, whichever is larger. Deliberately wider than a subscription's own
/// price-change tolerance (below) - a variable utility bill (product
/// handoff section 62, e.g. $124/$151/$168/$143) must still be recognized
/// as one recurring bill, not rejected as "too inconsistent" or split into
/// four unrelated one-off charges.
fn detection_tolerance(reference_cents: i64) -> i64 {
    (reference_cents.abs() * 40 / 100).max(500)
}

/// Tolerance used when deciding whether the *latest* occurrence of an
/// already-established pattern represents a meaningful price change, and
/// when matching a real transaction's amount against what an occurrence
/// expected: 20% of the reference amount, or $2, whichever is larger.
/// Tighter than `detection_tolerance` on purpose - once a pattern is
/// established, a $5 jump on a $10 subscription is real news; the same $5
/// swing on a $150 utility bill is not (kind-aware suppression happens at
/// the call site, not here).
pub(crate) fn amount_tolerance(reference_cents: i64) -> i64 {
    (reference_cents.abs() / 5).max(200)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Expense,
    Income,
    /// A recurring movement between the owner's own accounts (e.g. checking
    /// -> savings, or a credit-card payment) - never spending or income
    /// (product handoff sections 23/24), excluded from both totals.
    Transfer,
}

impl Direction {
    pub(crate) fn as_sql_str(self) -> &'static str {
        match self {
            Direction::Expense => "expense",
            Direction::Income => "income",
            Direction::Transfer => "transfer",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "expense" => Direction::Expense,
            "income" => Direction::Income,
            "transfer" => Direction::Transfer,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown direction '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }

    /// The direction a transaction's own (account-type-aware, ADR-0004)
    /// sign implies - never inferred from raw sign alone, since a
    /// liability's recurring charge is a *positive* amount_cents but is
    /// still an expense, not income (the exact bug Batch 5.1 fixed for
    /// manual entry; detection must not reintroduce it).
    fn from_amount(amount_cents: i64, is_liability: bool) -> Direction {
        let increases_balance = amount_cents >= 0;
        let is_expense = if is_liability {
            increases_balance
        } else {
            !increases_balance
        };
        if is_expense {
            Direction::Expense
        } else {
            Direction::Income
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Subscription,
    Bill,
    Membership,
    RecurringIncome,
    Other,
}

impl Kind {
    fn as_sql_str(self) -> &'static str {
        match self {
            Kind::Subscription => "subscription",
            Kind::Bill => "bill",
            Kind::Membership => "membership",
            Kind::RecurringIncome => "recurring_income",
            Kind::Other => "other",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "subscription" => Kind::Subscription,
            "bill" => Kind::Bill,
            "membership" => Kind::Membership,
            "recurring_income" => Kind::RecurringIncome,
            "other" => Kind::Other,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown kind '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesStatus {
    Proposed,
    Confirmed,
    Rejected,
    /// A confirmed pattern that has stopped recurring (product handoff
    /// section 35) - distinct from Rejected (never wanted in the first
    /// place). Set only explicitly by the owner; MIDAS never auto-
    /// deactivates after a single miss.
    Inactive,
}

impl SeriesStatus {
    fn as_sql_str(self) -> &'static str {
        match self {
            SeriesStatus::Proposed => "proposed",
            SeriesStatus::Confirmed => "confirmed",
            SeriesStatus::Rejected => "rejected",
            SeriesStatus::Inactive => "inactive",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "proposed" => SeriesStatus::Proposed,
            "confirmed" => SeriesStatus::Confirmed,
            "rejected" => SeriesStatus::Rejected,
            "inactive" => SeriesStatus::Inactive,
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
    pub payee_id: Option<i64>,
    pub merchant: String,
    pub direction: Direction,
    pub kind: Kind,
    pub cadence: Cadence,
    pub semimonthly_day_1: Option<i64>,
    pub semimonthly_day_2: Option<i64>,
    pub expected_amount_cents: i64,
    pub next_expected_date: String,
    pub category: Option<String>,
    pub status: SeriesStatus,
    pub planning_to_cancel: bool,
    pub notes: Option<String>,
    pub created_source: String,
    pub created_at: String,
    pub updated_at: String,
    // --- Computed live from linked transactions, never separately stored
    // (same correlated-computation philosophy as accounts::balance_cents) ---
    pub last_occurrence_date: Option<String>,
    pub last_amount_cents: Option<i64>,
    pub min_amount_cents: Option<i64>,
    pub max_amount_cents: Option<i64>,
    pub median_amount_cents: Option<i64>,
    pub occurrence_count: i64,
    pub is_price_changed: bool,
    pub previous_typical_amount_cents: Option<i64>,
    pub is_possibly_missed: bool,
    pub monthly_equivalent_cents: i64,
    pub annual_equivalent_cents: i64,
}

#[derive(Debug)]
pub enum RecurringError {
    Validation(String),
    Db(rusqlite::Error),
    NotFound(i64),
}

impl std::fmt::Display for RecurringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecurringError::Validation(msg) => write!(f, "{msg}"),
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

const COLUMNS: &str = "id, account_id, payee_id, merchant, direction, kind, cadence, \
    semimonthly_day_1, semimonthly_day_2, expected_amount_cents, next_expected_date, \
    category, status, planning_to_cancel, notes, created_source, created_at, updated_at";

fn today(conn: &Connection) -> rusqlite::Result<String> {
    conn.query_row("SELECT date('now')", [], |row| row.get(0))
}

fn median(sorted_amounts: &[i64]) -> i64 {
    let n = sorted_amounts.len();
    if n % 2 == 1 {
        sorted_amounts[n / 2]
    } else {
        (sorted_amounts[n / 2 - 1] + sorted_amounts[n / 2]) / 2
    }
}

/// Monthly-equivalent cents for presentation/comparison only (product
/// handoff section 38) - integer-safe division, rounded to the nearest
/// cent. Never written back to the ledger; purely a display computation.
fn monthly_equivalent(amount_cents: i64, cadence: Cadence) -> i64 {
    match cadence {
        Cadence::Weekly => (amount_cents * 52 + 6) / 12,
        Cadence::Biweekly => (amount_cents * 26 + 6) / 12,
        Cadence::Semimonthly => amount_cents * 2,
        Cadence::Monthly => amount_cents,
        Cadence::Quarterly => (amount_cents + 1) / 3,
        Cadence::Annual => (amount_cents + 6) / 12,
    }
}

fn annual_equivalent(amount_cents: i64, cadence: Cadence) -> i64 {
    match cadence {
        Cadence::Weekly => amount_cents * 52,
        Cadence::Biweekly => amount_cents * 26,
        Cadence::Semimonthly => amount_cents * 24,
        Cadence::Monthly => amount_cents * 12,
        Cadence::Quarterly => amount_cents * 4,
        Cadence::Annual => amount_cents,
    }
}

fn hydrate(conn: &Connection, row: &rusqlite::Row) -> rusqlite::Result<RecurringSeries> {
    let id: i64 = row.get("id")?;
    let expected_amount_cents: i64 = row.get("expected_amount_cents")?;
    let status = SeriesStatus::from_sql_str(&row.get::<_, String>("status")?)?;
    let next_expected_date: String = row.get("next_expected_date")?;
    let cadence = Cadence::from_sql_str(&row.get::<_, String>("cadence")?)?;
    let kind = Kind::from_sql_str(&row.get::<_, String>("kind")?)?;

    let mut linked_amounts: Vec<(String, i64)> = conn
        .prepare(
            "SELECT transaction_date, amount_cents FROM transactions
             WHERE recurring_series_id = ?1 AND is_archived = 0
             ORDER BY transaction_date ASC, id ASC",
        )?
        .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    linked_amounts.sort_by(|a, b| a.0.cmp(&b.0));

    let last_occurrence_date = linked_amounts.last().map(|(d, _)| d.clone());
    let last_amount_cents = linked_amounts.last().map(|(_, a)| *a);
    let occurrence_count = linked_amounts.len() as i64;

    let mut sorted_magnitudes: Vec<i64> = linked_amounts.iter().map(|(_, a)| a.abs()).collect();
    sorted_magnitudes.sort_unstable();
    let (min_amount_cents, max_amount_cents, median_amount_cents) = if sorted_magnitudes.is_empty()
    {
        (None, None, None)
    } else {
        (
            Some(*sorted_magnitudes.first().unwrap()),
            Some(*sorted_magnitudes.last().unwrap()),
            Some(median(&sorted_magnitudes)),
        )
    };

    // Price change: compare the latest linked amount against the median of
    // every *prior* linked amount (never a value frozen at first detection,
    // which would drift stale as more occurrences link over time).
    // Deliberately suppressed for Bill - normal utility variation is not a
    // price change (section 62) - and for anything with fewer than two
    // prior occurrences to compare against.
    let (is_price_changed, previous_typical_amount_cents) =
        if kind == Kind::Bill || linked_amounts.len() < 2 {
            (false, None)
        } else {
            let prior: Vec<i64> = linked_amounts[..linked_amounts.len() - 1]
                .iter()
                .map(|(_, a)| a.abs())
                .collect();
            let mut prior_sorted = prior.clone();
            prior_sorted.sort_unstable();
            let prior_typical = median(&prior_sorted);
            let latest = last_amount_cents.unwrap().abs();
            let changed = (latest - prior_typical).abs() > amount_tolerance(prior_typical);
            (changed, Some(prior_typical))
        };

    let is_possibly_missed = status == SeriesStatus::Confirmed && {
        let today = today(conn)?;
        days_between(&next_expected_date, &today) > 5
    };

    Ok(RecurringSeries {
        id,
        account_id: row.get("account_id")?,
        payee_id: row.get("payee_id")?,
        merchant: row.get("merchant")?,
        direction: Direction::from_sql_str(&row.get::<_, String>("direction")?)?,
        kind,
        cadence,
        semimonthly_day_1: row.get("semimonthly_day_1")?,
        semimonthly_day_2: row.get("semimonthly_day_2")?,
        expected_amount_cents,
        next_expected_date,
        category: row.get("category")?,
        status,
        planning_to_cancel: row.get::<_, i64>("planning_to_cancel")? != 0,
        notes: row.get("notes")?,
        created_source: row.get("created_source")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        last_occurrence_date,
        last_amount_cents,
        min_amount_cents,
        max_amount_cents,
        median_amount_cents,
        occurrence_count,
        is_price_changed,
        previous_typical_amount_cents,
        is_possibly_missed,
        monthly_equivalent_cents: monthly_equivalent(expected_amount_cents.abs(), cadence),
        annual_equivalent_cents: annual_equivalent(expected_amount_cents.abs(), cadence),
    })
}

pub(crate) fn get(conn: &Connection, id: i64) -> Result<Option<RecurringSeries>, RecurringError> {
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

/// Retroactively links every not-yet-linked transaction on this series'
/// account that matches its identity (payee if the series has one, else
/// exact merchant text) - so confirming a pattern immediately reflects its
/// full history, not just occurrences generated from here forward.
fn link_matching_history(conn: &Connection, series: &RecurringSeries) -> rusqlite::Result<()> {
    if let Some(payee_id) = series.payee_id {
        conn.execute(
            "UPDATE transactions SET recurring_series_id = ?1
             WHERE account_id = ?2 AND payee_id = ?3 AND recurring_series_id IS NULL",
            params![series.id, series.account_id, payee_id],
        )?;
    } else {
        conn.execute(
            "UPDATE transactions SET recurring_series_id = ?1
             WHERE account_id = ?2 AND merchant = ?3 AND recurring_series_id IS NULL",
            params![series.id, series.account_id, series.merchant],
        )?;
    }
    Ok(())
}

pub fn confirm(conn: &mut Connection, id: i64) -> Result<RecurringSeries, RecurringError> {
    let series = get(conn, id)?.ok_or(RecurringError::NotFound(id))?;
    let tx = conn.transaction()?;
    let affected = tx.execute(
        "UPDATE recurring_series SET status = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![id, SeriesStatus::Confirmed.as_sql_str()],
    )?;
    if affected == 0 {
        return Err(RecurringError::NotFound(id));
    }
    link_matching_history(&tx, &series)?;
    tx.commit()?;
    crate::recurring_occurrences::ensure_occurrences(conn, id)?;
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

pub fn set_inactive(conn: &Connection, id: i64) -> Result<RecurringSeries, RecurringError> {
    let affected = conn.execute(
        "UPDATE recurring_series SET status = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![id, SeriesStatus::Inactive.as_sql_str()],
    )?;
    if affected == 0 {
        return Err(RecurringError::NotFound(id));
    }
    get(conn, id)?.ok_or(RecurringError::NotFound(id))
}

/// Reactivates an Inactive series back to Confirmed (the owner noticed a
/// charge came back). Refuses from any other status - Proposed/Rejected
/// go through `confirm`/were never active, so "reactivate" would be
/// ambiguous for them.
pub fn reactivate(conn: &mut Connection, id: i64) -> Result<RecurringSeries, RecurringError> {
    let series = get(conn, id)?.ok_or(RecurringError::NotFound(id))?;
    if series.status != SeriesStatus::Inactive {
        return Err(RecurringError::Validation(
            "Only an inactive pattern can be reactivated.".to_string(),
        ));
    }
    conn.execute(
        "UPDATE recurring_series SET status = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![id, SeriesStatus::Confirmed.as_sql_str()],
    )?;
    crate::recurring_occurrences::ensure_occurrences(conn, id)?;
    get(conn, id)?.ok_or(RecurringError::NotFound(id))
}

pub fn set_planning_to_cancel(
    conn: &Connection,
    id: i64,
    planning_to_cancel: bool,
) -> Result<RecurringSeries, RecurringError> {
    let series = get(conn, id)?.ok_or(RecurringError::NotFound(id))?;
    if series.status != SeriesStatus::Confirmed {
        return Err(RecurringError::Validation(
            "Only a confirmed pattern can be marked planning-to-cancel.".to_string(),
        ));
    }
    conn.execute(
        "UPDATE recurring_series SET planning_to_cancel = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![id, planning_to_cancel as i64],
    )?;
    get(conn, id)?.ok_or(RecurringError::NotFound(id))
}

#[derive(Debug, Deserialize)]
pub struct PatternEditInput {
    pub merchant: String,
    pub payee_id: Option<i64>,
    pub kind: Kind,
    pub category: Option<String>,
    pub cadence: Cadence,
    pub semimonthly_day_1: Option<i64>,
    pub semimonthly_day_2: Option<i64>,
    pub expected_amount_cents: i64,
    pub next_expected_date: String,
    pub notes: Option<String>,
}

fn validate_pattern_input(input: &PatternEditInput) -> Result<(), RecurringError> {
    if input.merchant.trim().is_empty() {
        return Err(RecurringError::Validation("Name is required.".to_string()));
    }
    if crate::dates::parse_iso(&input.next_expected_date).is_none() {
        return Err(RecurringError::Validation(
            "Next expected date must be a valid date.".to_string(),
        ));
    }
    if input.cadence == Cadence::Semimonthly {
        let (Some(d1), Some(d2)) = (input.semimonthly_day_1, input.semimonthly_day_2) else {
            return Err(RecurringError::Validation(
                "Semi-monthly needs two days of the month.".to_string(),
            ));
        };
        if !(1..=31).contains(&d1) || !(1..=31).contains(&d2) || d1 == d2 {
            return Err(RecurringError::Validation(
                "Days of the month must be between 1 and 31 and different from each other."
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// A user-created pattern (product handoff section 16) uses exactly the
/// same model and table as a detected one - there is no separate "manual
/// bills" engine. Starts Confirmed immediately (the owner is asserting this
/// is real, not proposing it to themselves) and generates occurrences right
/// away.
pub fn create_manual(
    conn: &mut Connection,
    account_id: i64,
    direction: Direction,
    input: &PatternEditInput,
) -> Result<RecurringSeries, RecurringError> {
    validate_pattern_input(input)?;
    let id: i64 = conn.query_row(
        "INSERT INTO recurring_series (
            account_id, payee_id, merchant, direction, kind, cadence,
            semimonthly_day_1, semimonthly_day_2, expected_amount_cents,
            next_expected_date, category, status, notes, created_source
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'confirmed', ?12, 'manual')
         RETURNING id",
        params![
            account_id,
            input.payee_id,
            input.merchant.trim(),
            direction.as_sql_str(),
            input.kind.as_sql_str(),
            input.cadence.as_sql_str(),
            input.semimonthly_day_1,
            input.semimonthly_day_2,
            input.expected_amount_cents,
            input.next_expected_date,
            input.category,
            input.notes,
        ],
        |row| row.get(0),
    )?;
    crate::recurring_occurrences::ensure_occurrences(conn, id)?;
    get(conn, id)?.ok_or(RecurringError::NotFound(id))
}

/// Edits an existing pattern's fields (product handoff section 57). Once
/// edited, the owner's values are authoritative - detection never
/// overwrites them (section 56), since `detect_candidates` already skips
/// any merchant/payee with an existing row of any status.
pub fn update_pattern(
    conn: &Connection,
    id: i64,
    input: &PatternEditInput,
) -> Result<RecurringSeries, RecurringError> {
    validate_pattern_input(input)?;
    let affected = conn.execute(
        "UPDATE recurring_series SET
            merchant = ?2, payee_id = ?3, kind = ?4, category = ?5, cadence = ?6,
            semimonthly_day_1 = ?7, semimonthly_day_2 = ?8, expected_amount_cents = ?9,
            next_expected_date = ?10, notes = ?11, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1",
        params![
            id,
            input.merchant.trim(),
            input.payee_id,
            input.kind.as_sql_str(),
            input.category,
            input.cadence.as_sql_str(),
            input.semimonthly_day_1,
            input.semimonthly_day_2,
            input.expected_amount_cents,
            input.next_expected_date,
            input.notes,
        ],
    )?;
    if affected == 0 {
        return Err(RecurringError::NotFound(id));
    }
    get(conn, id)?.ok_or(RecurringError::NotFound(id))
}

/// Moves a pattern to a different account. Kept separate from
/// `update_pattern`/`PatternEditInput` rather than folded in: changing
/// account is a bigger decision than editing a name or amount (it changes
/// which ledger a future transaction must belong to in order to match), so
/// the frontend asks for an explicit confirmation before calling this
/// (product handoff: "allow legitimate account/card changes with
/// confirmation"). Occurrences need no update - they denormalize
/// `account_id` live from the series via a join, never store their own copy.
pub fn move_account(
    conn: &Connection,
    id: i64,
    new_account_id: i64,
) -> Result<RecurringSeries, RecurringError> {
    let affected = conn.execute(
        "UPDATE recurring_series SET account_id = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![id, new_account_id],
    )?;
    if affected == 0 {
        return Err(RecurringError::NotFound(id));
    }
    get(conn, id)?.ok_or(RecurringError::NotFound(id))
}

#[derive(Debug, Clone, Serialize)]
pub struct PriceHistoryPoint {
    pub transaction_id: i64,
    pub date: String,
    pub amount_cents: i64,
}

/// Derived entirely from linked transactions - never a separate stored
/// history (product handoff section 14: "do not duplicate transaction
/// history unnecessarily").
pub fn price_history(
    conn: &Connection,
    series_id: i64,
) -> Result<Vec<PriceHistoryPoint>, RecurringError> {
    let mut stmt = conn.prepare(
        "SELECT id, transaction_date, amount_cents FROM transactions
         WHERE recurring_series_id = ?1 AND is_archived = 0
         ORDER BY transaction_date ASC, id ASC",
    )?;
    let rows = stmt
        .query_map(params![series_id], |r| {
            Ok(PriceHistoryPoint {
                transaction_id: r.get(0)?,
                date: r.get(1)?,
                amount_cents: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn amounts_consistent(amounts: &[i64]) -> bool {
    let min = *amounts.iter().min().unwrap();
    let max = *amounts.iter().max().unwrap();
    let avg = amounts.iter().sum::<i64>() / amounts.len() as i64;
    (max - min) <= detection_tolerance(avg)
}

/// Whether the evidence would also have passed the *narrower*
/// `amount_tolerance` (the one price-change detection and matching use),
/// not just the wider `detection_tolerance` used to group evidence in the
/// first place. A pattern that only qualifies thanks to the wider bound is,
/// by construction, one whose real-world amount moves around from cycle to
/// cycle - a variable utility bill, not a fixed-price subscription - so
/// `propose_from_occurrences` uses this to default its `kind` to `Bill`
/// (which `hydrate` treats tolerantly) instead of `Other` (which would flag
/// every fresh reading as a "price change").
fn amounts_within_narrow_tolerance(amounts: &[i64]) -> bool {
    let min = *amounts.iter().min().unwrap();
    let max = *amounts.iter().max().unwrap();
    let avg = amounts.iter().sum::<i64>() / amounts.len() as i64;
    (max - min) <= amount_tolerance(avg)
}

/// Checks whether a sorted list of >=3 dates looks semimonthly: gaps
/// alternate between two values that are each a plausible "half a month"
/// (10-21 days) and sum close to a full month (26-34 days). Distinct from
/// Biweekly, whose single ~14-day gap would otherwise also pass a naive
/// per-gap window check for one of the two alternating lengths.
fn classify_semimonthly(dates: &[String]) -> Option<(Cadence, i64, i64)> {
    if dates.len() < 3 {
        return None;
    }
    let gaps: Vec<i64> = dates
        .windows(2)
        .map(|w| days_between(&w[0], &w[1]))
        .collect();
    if gaps.len() < 2 {
        return None;
    }
    // Every consecutive pair of gaps must sum to roughly a month, and
    // neither individual gap may itself look like a plain weekly/biweekly
    // interval on its own (otherwise a genuine biweekly series - one
    // constant 14-day gap - would misclassify as semimonthly too).
    let all_alternate_to_a_month = gaps.windows(2).all(|pair| {
        let sum = pair[0] + pair[1];
        (26..=34).contains(&sum) && (10..=21).contains(&pair[0]) && (10..=21).contains(&pair[1])
    });
    let gap_variance = gaps.iter().max().unwrap() - gaps.iter().min().unwrap();
    if !all_alternate_to_a_month || gap_variance < 2 {
        return None;
    }
    let day1 = crate::dates::parse_iso(&dates[0])?;
    let day2 = crate::dates::parse_iso(&dates[1])?;
    use chrono::Datelike;
    Some((Cadence::Semimonthly, day1.day() as i64, day2.day() as i64))
}

/// Scans an account's transaction history for repeating identities and
/// proposes new `recurring_series` rows for patterns that look confident:
/// at least 3 occurrences, consistent gap-between-dates (all classifying to
/// the same cadence, or matching the semimonthly shape), and consistent
/// amounts. Groups by canonical payee first when every candidate occurrence
/// shares one (product handoff section 47 - survives raw-description
/// variants), falling back to exact merchant text otherwise. Skips any
/// identity that already has a series (of any status) for this account, so
/// re-running detection is idempotent rather than piling up duplicate
/// proposals. Transfers (and the structural opening_balance/
/// balance_adjustment anchors) are never candidates - a recurring transfer
/// or credit-card payment must never surface as a bill/subscription/income
/// (sections 23/24). One-time-flagged transactions are excluded too
/// (section 46) - they are explicitly not "normal" evidence.
pub fn detect_candidates(
    conn: &Connection,
    account_id: i64,
) -> Result<Vec<RecurringSeries>, RecurringError> {
    let is_liability: bool = conn.query_row(
        "SELECT account_type IN ('credit_card', 'loan', 'other_liability') FROM accounts WHERE id = ?1",
        params![account_id],
        |r| r.get(0),
    )?;

    let mut stmt = conn.prepare(
        "SELECT merchant, payee_id, transaction_date, amount_cents, category FROM transactions
         WHERE account_id = ?1 AND is_archived = 0 AND trim(merchant) != ''
             AND transaction_type NOT IN ('opening_balance', 'balance_adjustment', 'transfer')
             AND is_one_time = 0
         ORDER BY merchant, transaction_date",
    )?;
    type EvidenceRow = (String, Option<i64>, String, i64, Option<String>);
    let rows: Vec<EvidenceRow> = stmt
        .query_map(params![account_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut existing_merchants =
        conn.prepare("SELECT DISTINCT merchant FROM recurring_series WHERE account_id = ?1")?;
    let already_by_merchant: std::collections::HashSet<String> = existing_merchants
        .query_map(params![account_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .collect();
    let mut existing_payees = conn.prepare(
        "SELECT DISTINCT payee_id FROM recurring_series WHERE account_id = ?1 AND payee_id IS NOT NULL",
    )?;
    let already_by_payee: std::collections::HashSet<i64> = existing_payees
        .query_map(params![account_id], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .collect();

    // Group by payee when a transaction has one assigned; a raw-merchant
    // group is the fallback for transactions with no payee yet.
    let mut by_payee: HashMap<i64, Vec<(String, i64, Option<String>)>> = HashMap::new();
    let mut by_merchant: HashMap<String, Vec<(String, i64, Option<String>)>> = HashMap::new();
    for (merchant, payee_id, date, amount, category) in rows {
        if let Some(pid) = payee_id {
            by_payee
                .entry(pid)
                .or_default()
                .push((date, amount, category));
        } else {
            by_merchant
                .entry(merchant)
                .or_default()
                .push((date, amount, category));
        }
    }

    let mut created = Vec::new();

    for (payee_id, occurrences) in by_payee {
        if already_by_payee.contains(&payee_id) || occurrences.len() < 3 {
            continue;
        }
        let payee_name: String = conn.query_row(
            "SELECT name FROM payees WHERE id = ?1",
            params![payee_id],
            |r| r.get(0),
        )?;
        if already_by_merchant.contains(&payee_name) {
            continue;
        }
        if let Some(series) = propose_from_occurrences(
            conn,
            account_id,
            Some(payee_id),
            &payee_name,
            occurrences,
            is_liability,
        )? {
            created.push(series);
        }
    }

    for (merchant, occurrences) in by_merchant {
        if already_by_merchant.contains(&merchant) || occurrences.len() < 3 {
            continue;
        }
        if let Some(series) =
            propose_from_occurrences(conn, account_id, None, &merchant, occurrences, is_liability)?
        {
            created.push(series);
        }
    }

    created.sort_by(|a, b| a.merchant.cmp(&b.merchant));
    Ok(created)
}

fn propose_from_occurrences(
    conn: &Connection,
    account_id: i64,
    payee_id: Option<i64>,
    display_name: &str,
    mut occurrences: Vec<(String, i64, Option<String>)>,
    is_liability: bool,
) -> Result<Option<RecurringSeries>, RecurringError> {
    occurrences.sort_by(|a, b| a.0.cmp(&b.0));
    let dates: Vec<String> = occurrences.iter().map(|o| o.0.clone()).collect();

    let (cadence, semimonthly_days) = if let Some((c, d1, d2)) = classify_semimonthly(&dates) {
        (c, Some((d1, d2)))
    } else {
        let gaps: Vec<i64> = dates
            .windows(2)
            .map(|w| days_between(&w[0], &w[1]))
            .collect();
        let cadences: Option<Vec<Cadence>> = gaps.iter().map(|&g| Cadence::classify(g)).collect();
        let Some(cadences) = cadences else {
            return Ok(None);
        };
        let cadence = cadences[0];
        if !cadences.iter().all(|&c| c == cadence) {
            return Ok(None);
        }
        (cadence, None)
    };

    let amounts: Vec<i64> = occurrences.iter().map(|o| o.1).collect();
    if !amounts_consistent(&amounts) {
        return Ok(None);
    }

    let last = occurrences.last().unwrap();
    let direction = Direction::from_amount(last.1, is_liability);
    // Conservative, deterministic, tied to the same tolerance math already
    // computed above - never a merchant-name guess (section 15/46).
    let kind = if direction == Direction::Expense && !amounts_within_narrow_tolerance(&amounts) {
        Kind::Bill
    } else {
        Kind::Other
    };
    let next_expected_date = match cadence {
        Cadence::Semimonthly => {
            let (d1, d2) = semimonthly_days.unwrap();
            next_semimonthly_date(&last.0, d1, d2)
        }
        Cadence::Monthly => add_months(&last.0, 1),
        _ => {
            let (_, _, typical_days) = cadence.day_range();
            add_days(&last.0, typical_days)
        }
    };

    let id: i64 = conn.query_row(
        "INSERT INTO recurring_series (
            account_id, payee_id, merchant, direction, kind, cadence,
            semimonthly_day_1, semimonthly_day_2, expected_amount_cents,
            next_expected_date, category
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) RETURNING id",
        params![
            account_id,
            payee_id,
            display_name,
            direction.as_sql_str(),
            kind.as_sql_str(),
            cadence.as_sql_str(),
            semimonthly_days.map(|(d, _)| d),
            semimonthly_days.map(|(_, d)| d),
            last.1,
            next_expected_date,
            last.2,
        ],
        |row| row.get(0),
    )?;
    Ok(Some(get(conn, id)?.ok_or(RecurringError::NotFound(id))?))
}

/// The next semimonthly date after `after_date`, given the pattern's two
/// fixed days-of-month - mirrors `pay_cycle.rs`'s own cycle-boundary logic
/// for the identical two-day shape.
pub(crate) fn next_semimonthly_date(after_date: &str, day_1: i64, day_2: i64) -> String {
    let (first, second) = if day_1 <= day_2 {
        (day_1 as u32, day_2 as u32)
    } else {
        (day_2 as u32, day_1 as u32)
    };
    let this_month_first = day_in_month(after_date, first);
    let this_month_second = day_in_month(after_date, second);
    if days_between(after_date, &this_month_first) > 0 {
        this_month_first
    } else if days_between(after_date, &this_month_second) > 0 {
        this_month_second
    } else {
        day_in_month(&add_months(after_date, 1), first)
    }
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
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    confirm(&mut conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reject_recurring(state: State<DbState>, id: i64) -> Result<RecurringSeries, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    reject(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_recurring_inactive(state: State<DbState>, id: i64) -> Result<RecurringSeries, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    set_inactive(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reactivate_recurring(state: State<DbState>, id: i64) -> Result<RecurringSeries, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    reactivate(&mut conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_recurring_planning_to_cancel(
    state: State<DbState>,
    id: i64,
    planning_to_cancel: bool,
) -> Result<RecurringSeries, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    set_planning_to_cancel(&conn, id, planning_to_cancel).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_manual_recurring(
    state: State<DbState>,
    account_id: i64,
    direction: Direction,
    input: PatternEditInput,
) -> Result<RecurringSeries, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    create_manual(&mut conn, account_id, direction, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_recurring_pattern(
    state: State<DbState>,
    id: i64,
    input: PatternEditInput,
) -> Result<RecurringSeries, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    update_pattern(&conn, id, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn move_recurring_account(
    state: State<DbState>,
    id: i64,
    new_account_id: i64,
) -> Result<RecurringSeries, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    move_account(&conn, id, new_account_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_recurring_price_history(
    state: State<DbState>,
    series_id: i64,
) -> Result<Vec<PriceHistoryPoint>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    price_history(&conn, series_id).map_err(|e| e.to_string())
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

    fn make_account(conn: &Connection) -> i64 {
        make_account_of_type(conn, AccountType::Checking)
    }

    fn make_account_of_type(conn: &Connection, account_type: AccountType) -> i64 {
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

    fn add_txn(conn: &Connection, account_id: i64, date: &str, amount_cents: i64, merchant: &str) {
        add_txn_typed(
            conn,
            account_id,
            date,
            amount_cents,
            merchant,
            TransactionType::Purchase,
        );
    }

    fn add_txn_typed(
        conn: &Connection,
        account_id: i64,
        date: &str,
        amount_cents: i64,
        merchant: &str,
        transaction_type: TransactionType,
    ) {
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: date.to_string(),
                posted_date: None,
                amount_cents,
                transaction_type,
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
        assert_eq!(candidates[0].direction, Direction::Expense);
        // Monthly + Feb 5 -> Mar 5 correctly clamps/advances via add_months.
        assert_eq!(candidates[0].next_expected_date, "2026-04-05");
        assert_eq!(
            candidates[0].kind,
            Kind::Other,
            "a byte-identical amount easily passes the narrow tolerance too, \
             so this must not default to Bill"
        );
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
    fn detects_a_variable_utility_bill_within_the_wider_detection_tolerance() {
        // Product handoff section 62's own example: one recurring bill, not
        // four price changes - the 40%-or-$5 detection tolerance (widened
        // from the original 20%-or-$2) must still accept this spread,
        // while does_not_propose_wildly_inconsistent_amounts above (a
        // >2x spread) must still be rejected.
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-06-15", -12400, "FPL");
        add_txn(&conn, account_id, "2026-07-15", -15100, "FPL");
        add_txn(&conn, account_id, "2026-08-15", -16800, "FPL");
        add_txn(&conn, account_id, "2026-09-15", -14300, "FPL");

        let candidates = detect_candidates(&conn, account_id).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].cadence, Cadence::Monthly);
        assert_eq!(
            candidates[0].kind,
            Kind::Bill,
            "a spread this wide only qualifies under the wider detection \
             tolerance, so it must default to Bill (tolerant of variation) \
             rather than Other (which would flag every reading as a price change)"
        );
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
    fn transfers_are_never_proposed_as_a_recurring_pattern() {
        // Section 23/24: a recurring checking->savings transfer or a
        // recurring credit-card payment must never surface as a bill,
        // subscription, or income.
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn_typed(
            &conn,
            account_id,
            "2026-01-01",
            -30000,
            "Transfer to Savings",
            TransactionType::Transfer,
        );
        add_txn_typed(
            &conn,
            account_id,
            "2026-02-01",
            -30000,
            "Transfer to Savings",
            TransactionType::Transfer,
        );
        add_txn_typed(
            &conn,
            account_id,
            "2026-03-01",
            -30000,
            "Transfer to Savings",
            TransactionType::Transfer,
        );

        assert!(detect_candidates(&conn, account_id).unwrap().is_empty());
    }

    #[test]
    fn one_time_transactions_are_excluded_from_detection_evidence() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Occasional Store");
        add_txn(&conn, account_id, "2026-02-05", -999, "Occasional Store");
        transactions::create(
            &conn,
            &TransactionInput {
                account_id,
                transaction_date: "2026-03-05".to_string(),
                posted_date: None,
                amount_cents: -999,
                transaction_type: TransactionType::Purchase,
                raw_description: "Occasional Store".to_string(),
                merchant: Some("Occasional Store".to_string()),
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

        assert!(
            detect_candidates(&conn, account_id).unwrap().is_empty(),
            "a one-time-flagged occurrence must not count as the third piece of evidence"
        );
    }

    #[test]
    fn credit_card_recurring_charge_is_classified_as_expense_not_income() {
        // ADR-0004: a liability's recurring charge is a *positive*
        // amount_cents but must still be an expense (the exact class of
        // bug Batch 5.1 fixed for manual entry - detection must agree).
        let conn = test_conn();
        let account_id = make_account_of_type(&conn, AccountType::CreditCard);
        add_txn(&conn, account_id, "2026-01-10", 1599, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-10", 1599, "Streaming Service");
        add_txn(&conn, account_id, "2026-03-10", 1599, "Streaming Service");

        let candidates = detect_candidates(&conn, account_id).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].direction, Direction::Expense);
    }

    #[test]
    fn detects_recurring_income_with_positive_amounts_on_an_asset_account() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn_typed(
            &conn,
            account_id,
            "2026-01-02",
            190000,
            "Employer Payroll",
            TransactionType::Income,
        );
        add_txn_typed(
            &conn,
            account_id,
            "2026-01-16",
            190000,
            "Employer Payroll",
            TransactionType::Income,
        );
        add_txn_typed(
            &conn,
            account_id,
            "2026-01-30",
            190000,
            "Employer Payroll",
            TransactionType::Income,
        );

        let candidates = detect_candidates(&conn, account_id).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].direction, Direction::Income);
        assert_eq!(candidates[0].cadence, Cadence::Biweekly);
    }

    #[test]
    fn detects_semimonthly_paychecks_distinctly_from_biweekly() {
        // 1st and 15th of the month: gaps alternate 14, 17, 14, 17... -
        // must classify as Semimonthly, not Biweekly.
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn_typed(
            &conn,
            account_id,
            "2026-01-01",
            190000,
            "Semimonthly Employer",
            TransactionType::Income,
        );
        add_txn_typed(
            &conn,
            account_id,
            "2026-01-15",
            190000,
            "Semimonthly Employer",
            TransactionType::Income,
        );
        add_txn_typed(
            &conn,
            account_id,
            "2026-02-01",
            190000,
            "Semimonthly Employer",
            TransactionType::Income,
        );
        add_txn_typed(
            &conn,
            account_id,
            "2026-02-15",
            190000,
            "Semimonthly Employer",
            TransactionType::Income,
        );

        let candidates = detect_candidates(&conn, account_id).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].cadence, Cadence::Semimonthly);
        assert_eq!(candidates[0].semimonthly_day_1, Some(1));
        assert_eq!(candidates[0].semimonthly_day_2, Some(15));
    }

    #[test]
    fn a_genuine_biweekly_series_is_never_misclassified_as_semimonthly() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn_typed(
            &conn,
            account_id,
            "2026-01-02",
            190000,
            "Biweekly Employer",
            TransactionType::Income,
        );
        add_txn_typed(
            &conn,
            account_id,
            "2026-01-16",
            190000,
            "Biweekly Employer",
            TransactionType::Income,
        );
        add_txn_typed(
            &conn,
            account_id,
            "2026-01-30",
            190000,
            "Biweekly Employer",
            TransactionType::Income,
        );
        add_txn_typed(
            &conn,
            account_id,
            "2026-02-13",
            190000,
            "Biweekly Employer",
            TransactionType::Income,
        );

        let candidates = detect_candidates(&conn, account_id).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].cadence, Cadence::Biweekly);
    }

    #[test]
    fn confirm_links_matching_transactions_and_reject_does_not() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-03-05", -999, "Streaming Service");
        let candidate = detect_candidates(&conn, account_id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

        let confirmed = confirm(&mut conn, candidate.id).unwrap();
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
    fn price_change_flag_compares_latest_against_prior_typical_not_a_frozen_snapshot() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-03-05", -999, "Streaming Service");
        let candidate = detect_candidates(&conn, account_id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        confirm(&mut conn, candidate.id).unwrap();

        add_txn(&conn, account_id, "2026-04-05", -2999, "Streaming Service");
        conn.execute(
            "UPDATE transactions SET recurring_series_id = ?1 WHERE transaction_date = '2026-04-05'",
            params![candidate.id],
        )
        .unwrap();

        let refreshed = get(&conn, candidate.id).unwrap().unwrap();
        assert!(refreshed.is_price_changed);
        assert_eq!(refreshed.last_amount_cents, Some(-2999));
        assert_eq!(refreshed.previous_typical_amount_cents, Some(999));
    }

    #[test]
    fn bill_kind_never_flags_a_price_change_for_normal_variation() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-06-15", -12400, "FPL");
        add_txn(&conn, account_id, "2026-07-15", -15100, "FPL");
        add_txn(&conn, account_id, "2026-08-15", -16800, "FPL");
        let candidate = detect_candidates(&conn, account_id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        confirm(&mut conn, candidate.id).unwrap();
        update_pattern(
            &conn,
            candidate.id,
            &PatternEditInput {
                merchant: candidate.merchant.clone(),
                payee_id: None,
                kind: Kind::Bill,
                category: None,
                cadence: candidate.cadence,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: candidate.expected_amount_cents,
                next_expected_date: candidate.next_expected_date.clone(),
                notes: None,
            },
        )
        .unwrap();

        add_txn(&conn, account_id, "2026-09-15", -14300, "FPL");
        conn.execute(
            "UPDATE transactions SET recurring_series_id = ?1 WHERE transaction_date = '2026-09-15'",
            params![candidate.id],
        )
        .unwrap();

        let refreshed = get(&conn, candidate.id).unwrap().unwrap();
        assert!(
            !refreshed.is_price_changed,
            "a Bill must never flag ordinary utility variation as a price change"
        );
        assert_eq!(refreshed.min_amount_cents, Some(12400));
        assert_eq!(refreshed.max_amount_cents, Some(16800));
    }

    #[test]
    fn set_inactive_and_reactivate_round_trip() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-05", -999, "Streaming Service");
        add_txn(&conn, account_id, "2026-03-05", -999, "Streaming Service");
        let candidate = detect_candidates(&conn, account_id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        confirm(&mut conn, candidate.id).unwrap();

        let inactive = set_inactive(&conn, candidate.id).unwrap();
        assert_eq!(inactive.status, SeriesStatus::Inactive);

        let reactivated = reactivate(&mut conn, candidate.id).unwrap();
        assert_eq!(reactivated.status, SeriesStatus::Confirmed);

        assert!(matches!(
            reactivate(&mut conn, candidate.id).unwrap_err(),
            RecurringError::Validation(_)
        ));
    }

    #[test]
    fn planning_to_cancel_requires_a_confirmed_pattern() {
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

        assert!(matches!(
            set_planning_to_cancel(&conn, candidate.id, true).unwrap_err(),
            RecurringError::Validation(_)
        ));
    }

    #[test]
    fn manual_pattern_creation_starts_confirmed_and_generates_occurrences() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let series = create_manual(
            &mut conn,
            account_id,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Rent".to_string(),
                payee_id: None,
                kind: Kind::Bill,
                category: Some("Housing".to_string()),
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -165000,
                next_expected_date: "2026-10-01".to_string(),
                notes: None,
            },
        )
        .unwrap();
        assert_eq!(series.status, SeriesStatus::Confirmed);
        assert_eq!(series.created_source, "manual");

        let occurrence_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM recurring_occurrences WHERE series_id = ?1",
                params![series.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(occurrence_count > 0);
    }

    #[test]
    fn move_account_updates_the_series_and_future_occurrence_listings() {
        let mut conn = test_conn();
        let checking = make_account(&conn);
        let savings = make_account(&conn);
        let series = create_manual(
            &mut conn,
            checking,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Gym".to_string(),
                payee_id: None,
                kind: Kind::Membership,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -4500,
                next_expected_date: "2026-10-01".to_string(),
                notes: None,
            },
        )
        .unwrap();

        let moved = move_account(&conn, series.id, savings).unwrap();
        assert_eq!(moved.account_id, savings);

        // Occurrences denormalize account_id live from the series via a
        // join - moving the series must be immediately reflected, with no
        // separate occurrence-table update needed.
        let occurrences = crate::recurring_occurrences::list_for_series(&conn, series.id).unwrap();
        assert!(!occurrences.is_empty());
        assert!(occurrences.iter().all(|o| o.account_id == savings));
    }

    #[test]
    fn manual_creation_rejects_incomplete_semimonthly_days() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let err = create_manual(
            &mut conn,
            account_id,
            Direction::Income,
            &PatternEditInput {
                merchant: "Paycheck".to_string(),
                payee_id: None,
                kind: Kind::RecurringIncome,
                category: None,
                cadence: Cadence::Semimonthly,
                semimonthly_day_1: Some(1),
                semimonthly_day_2: None,
                expected_amount_cents: 190000,
                next_expected_date: "2026-10-01".to_string(),
                notes: None,
            },
        )
        .unwrap_err();
        assert!(matches!(err, RecurringError::Validation(_)));
    }

    #[test]
    fn monthly_and_annual_equivalents_use_defined_integer_rounding() {
        assert_eq!(monthly_equivalent(2299, Cadence::Monthly), 2299);
        assert_eq!(annual_equivalent(2299, Cadence::Monthly), 27588);
        assert_eq!(monthly_equivalent(500, Cadence::Weekly), 2167); // 500*52/12 rounded
        assert_eq!(annual_equivalent(500, Cadence::Weekly), 26000);
        assert_eq!(monthly_equivalent(190000, Cadence::Biweekly), 411667);
    }

    #[test]
    fn next_semimonthly_date_advances_correctly_across_month_boundaries() {
        assert_eq!(next_semimonthly_date("2026-01-01", 1, 15), "2026-01-15");
        assert_eq!(next_semimonthly_date("2026-01-15", 1, 15), "2026-02-01");
        assert_eq!(next_semimonthly_date("2026-01-31", 1, 15), "2026-02-01");
    }

    #[test]
    fn price_history_is_derived_from_linked_transactions_in_date_order() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(&conn, account_id, "2026-01-05", -1549, "Streaming Service");
        add_txn(&conn, account_id, "2026-02-05", -1549, "Streaming Service");
        add_txn(&conn, account_id, "2026-03-05", -1699, "Streaming Service");
        let candidate = detect_candidates(&conn, account_id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        confirm(&mut conn, candidate.id).unwrap();

        let history = price_history(&conn, candidate.id).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].date, "2026-01-05");
        assert_eq!(history[2].amount_cents, -1699);
    }
}
