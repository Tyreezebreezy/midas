//! Monthly category budgets (Phase 4B, matured into Budget Intelligence in
//! Batch 7). Deliberately not an envelope system - no fund transfers
//! between categories: one planned amount per category per month, compared
//! against actual spending computed by `accounting::spending_in_range`
//! (reused, not reimplemented, so budgets automatically exclude transfers,
//! correctly net out refunds, and - since Batch 7's own reconnaissance fix -
//! correctly attribute a split transaction's spending to each split's own
//! category rather than the parent's pre-split label).
//!
//! Every month is its own row (`UNIQUE(category, month)`, unchanged from
//! 0005) - this is deliberately kept, not replaced with a "base budget plus
//! monthly overrides" model, because it already gives two things Batch 7
//! needs for free: editing this month's planned amount can never rewrite
//! what a past month's budget was (nothing else reads "the base"), and
//! planning one exceptional month (a December gift budget, say) is just a
//! row for December - no separate override concept required. The real gap
//! this batch closes is UX, not schema: `copy_month` gives a one-click way
//! to carry a steady budget forward instead of re-typing it every month.
//!
//! Rollover, pace, and the historical typical-spending baseline are all
//! computed live from this same one-row-per-month history, never stored -
//! the same correlated-computation philosophy `accounts::balance_cents` and
//! `recurring::RecurringSeries`'s derived fields already established.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::accounting;
use crate::dates;
use crate::db::DbState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Fixed,
    Flexible,
    Discretionary,
}

impl Classification {
    fn as_sql_str(self) -> &'static str {
        match self {
            Classification::Fixed => "fixed",
            Classification::Flexible => "flexible",
            Classification::Discretionary => "discretionary",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "fixed" => Classification::Fixed,
            "flexible" => Classification::Flexible,
            "discretionary" => Classification::Discretionary,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown classification '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }
}

/// Under 90% of what's actually available this month (planned + rollover),
/// at/over 90%, or over entirely. A fixed convention rather than a
/// per-budget configurable threshold - nothing in the request asked for
/// per-category tuning, and a stored config field nobody sets is exactly
/// the kind of unused flexibility to avoid. Meaningful for any month
/// (past, current, or future-with-transactions); `pace` below is the
/// separate, current-month-only concept of whether spending is on schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStatus {
    Under,
    Near,
    Over,
}

const WARNING_THRESHOLD_RATIO: f64 = 0.9;

/// Whether spending is ahead of, behind, or roughly matching how far the
/// month has gotten - only ever populated for the real, currently-in-progress
/// calendar month (section 48/49: a future month has no pace yet, a
/// completed month's own final `BudgetStatus` already answers "how did it
/// go" better than a stale pace reading would).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaceStatus {
    OnTrack,
    Faster,
    Slower,
}

/// How far percent-used is allowed to drift from percent-elapsed before
/// pace flips from "on track" - in percentage points, not a ratio, so it
/// stays easy to reason about. The handoff's own worked example (52% used
/// at 43% elapsed) must still read as on track, which only needs a ~9-point
/// window; 10 leaves a little headroom without being so wide it stops
/// meaning anything. A fixed, documented tolerance rather than a
/// per-category configurable one - nothing asked for per-category tuning.
const PACE_TOLERANCE_POINTS: i64 = 10;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PaceInfo {
    pub percent_used: i64,
    pub percent_elapsed: i64,
    pub status: PaceStatus,
}

/// Minimum qualifying months of history before a typical-spending baseline
/// is considered meaningful enough to show at all (section 53 - one data
/// point is not a baseline). "Qualifying" already excludes both one-time-
/// flagged spending and months with no data at all, so this is a floor on
/// genuinely comparable months, not just elapsed calendar time.
const MIN_MONTHS_FOR_TYPICAL: usize = 2;

/// Default lookback for both the standalone "typical spending" baseline and
/// budget suggestions (section 8 - documented default). Six rather than
/// three: `historical_typical` uses a median (section 54), which benefits
/// from a few more data points without letting ancient behavior dominate -
/// six months is still "recent." `pub(crate)` so Insights' category detail
/// (Batch 8) uses this exact same default rather than a second hardcoded "6".
pub(crate) const DEFAULT_LOOKBACK_MONTHS: i32 = 6;

/// Safety cap on how far `rollover_in_for` will ever walk back through
/// consecutive rollover-enabled months - purely defensive (a real chain
/// breaks at the first gap or disabled month anyway, see below), so a
/// pathological multi-decade chain can't turn one query into an unbounded
/// walk.
const ROLLOVER_LOOKBACK_LIMIT: i32 = 36;

#[derive(Debug, Clone, Serialize)]
pub struct Budget {
    pub id: i64,
    pub category: String,
    pub month: String,
    pub planned_amount_cents: i64,
    pub classification: Classification,
    pub rollover_enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub actual_amount_cents: i64,
    pub status: BudgetStatus,
    // --- Batch 7: all computed live from history, never stored ---
    /// Surplus (positive) or deficit (negative) carried in from prior
    /// rollover-enabled months. Always 0 when `rollover_enabled` is false.
    pub rollover_in_cents: i64,
    /// `planned_amount_cents + rollover_in_cents` - the actual capacity
    /// this month has, which `status`/`pace`/"remaining" are all judged
    /// against instead of the raw planned figure.
    pub effective_amount_cents: i64,
    pub typical_amount_cents: Option<i64>,
    pub typical_months_of_history: i32,
    pub pace: Option<PaceInfo>,
}

#[derive(Debug, Deserialize)]
pub struct BudgetInput {
    pub category: String,
    pub month: String,
    pub planned_amount_cents: i64,
    pub classification: Classification,
    pub rollover_enabled: bool,
}

#[derive(Debug)]
pub enum BudgetError {
    Validation(String),
    Duplicate,
    Db(rusqlite::Error),
    NotFound(i64),
}

impl std::fmt::Display for BudgetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BudgetError::Validation(msg) => write!(f, "{msg}"),
            BudgetError::Duplicate => {
                write!(f, "A budget for this category and month already exists.")
            }
            BudgetError::Db(e) => write!(f, "database error: {e}"),
            BudgetError::NotFound(id) => write!(f, "budget {id} not found"),
        }
    }
}

impl From<rusqlite::Error> for BudgetError {
    fn from(e: rusqlite::Error) -> Self {
        BudgetError::Db(e)
    }
}

fn validate(input: &BudgetInput) -> Result<(), BudgetError> {
    if input.category.trim().is_empty() {
        return Err(BudgetError::Validation("Category is required.".to_string()));
    }
    if input.planned_amount_cents < 0 {
        return Err(BudgetError::Validation(
            "Planned amount cannot be negative.".to_string(),
        ));
    }
    if crate::dates::parse_iso(&input.month).is_none() {
        return Err(BudgetError::Validation(
            "Month must be a valid date.".to_string(),
        ));
    }
    Ok(())
}

fn duplicate_exists(
    conn: &Connection,
    category: &str,
    month: &str,
    exclude_id: Option<i64>,
) -> Result<bool, BudgetError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM budgets WHERE category = ?1 AND month = ?2 AND id != ?3",
        params![category, month, exclude_id.unwrap_or(0)],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

const COLUMNS: &str = "id, category, month, planned_amount_cents, classification, \
    rollover_enabled, created_at, updated_at";

/// Actual spending for `category` within the calendar month containing
/// `month` - delegates to `accounting::spending_in_range`, the one shared
/// place "spending in a date range" is computed (also used by pay_cycle.rs).
fn actual_spending_for(conn: &Connection, category: &str, month: &str) -> rusqlite::Result<i64> {
    accounting::spending_in_range(
        conn,
        Some(category),
        &dates::start_of_month(month),
        &dates::end_of_month(month),
    )
}

fn today(conn: &Connection) -> rusqlite::Result<String> {
    conn.query_row("SELECT date('now')", [], |row| row.get(0))
}

struct RawBudgetRow {
    planned_amount_cents: i64,
    rollover_enabled: bool,
}

/// The stored fields for one category/month, with none of `hydrate`'s live
/// computation - used only by `rollover_in_for`'s backward walk, which
/// needs prior months' own *planned* amounts, never their fully-hydrated
/// (and mutually recursive) `Budget`.
fn find_raw(
    conn: &Connection,
    category: &str,
    month: &str,
) -> rusqlite::Result<Option<RawBudgetRow>> {
    conn.query_row(
        "SELECT planned_amount_cents, rollover_enabled FROM budgets WHERE category = ?1 AND month = ?2",
        params![category, dates::start_of_month(month)],
        |row| {
            Ok(RawBudgetRow {
                planned_amount_cents: row.get(0)?,
                rollover_enabled: row.get::<_, i64>(1)? != 0,
            })
        },
    )
    .optional()
}

/// The surplus (positive) or deficit (negative) carried into `month` for
/// `category`, derived entirely from prior months' own stored planned
/// amounts and live-computed actuals - never a separately stored, mutable
/// running total, so it is always reproducible from history alone (section
/// 21). Walks backward one month at a time, accumulating each qualifying
/// month's own `planned - actual`; stops at the first month with no budget
/// row for this category (nothing to roll over from) or where rollover
/// wasn't enabled (a deliberate one-month opt-out breaks the chain).
///
/// This simple sum is not an approximation of compounding - it *is*
/// compounding. If `carry(k) = planned(k) - actual(k) + carry(k-1)`, the
/// recurrence telescopes into exactly `sum(planned(k) - actual(k))` over
/// the qualifying chain, so a plain backward loop already gives the same
/// answer a "prior month's carry feeds into this one" description would,
/// without needing actual recursion.
fn rollover_in_for(conn: &Connection, category: &str, month: &str) -> rusqlite::Result<i64> {
    let mut total_carry = 0i64;
    let mut cursor_month = month.to_string();
    for _ in 0..ROLLOVER_LOOKBACK_LIMIT {
        let prior_month = dates::add_months(&cursor_month, -1);
        let Some(prior) = find_raw(conn, category, &prior_month)? else {
            break;
        };
        if !prior.rollover_enabled {
            break;
        }
        let prior_actual = actual_spending_for(conn, category, &prior_month)?;
        total_carry += prior.planned_amount_cents - prior_actual;
        cursor_month = prior_month;
    }
    Ok(total_carry)
}

/// Pure threshold classification, deliberately separated from `pace_for`'s
/// date math below so it can be tested with fully controlled inputs -
/// `pace_for` itself depends on the real wall-clock day of the month, which
/// a test can't pin down without either mocking `today()` (no override
/// mechanism exists) or picking values that happen to work on every
/// possible day, section 85's own warning about wall-clock-dependent tests.
fn classify_pace(percent_used: i64, percent_elapsed: i64) -> PaceStatus {
    let diff = percent_used - percent_elapsed;
    if diff > PACE_TOLERANCE_POINTS {
        PaceStatus::Faster
    } else if diff < -PACE_TOLERANCE_POINTS {
        PaceStatus::Slower
    } else {
        PaceStatus::OnTrack
    }
}

/// Only populated for the real, currently-in-progress calendar month
/// (sections 48/49) and only when there's a positive effective amount to
/// measure progress against. Percent-elapsed uses the simplest acceptable
/// definition (section 47): the current calendar day number divided by the
/// number of days in the month, e.g. Sep 13 in a 30-day month is 43%.
fn pace_for(
    conn: &Connection,
    month: &str,
    effective_amount_cents: i64,
    actual_amount_cents: i64,
) -> rusqlite::Result<Option<PaceInfo>> {
    if effective_amount_cents <= 0 {
        return Ok(None);
    }
    let today = today(conn)?;
    let month_start = dates::start_of_month(&today);
    if dates::start_of_month(month) != month_start {
        return Ok(None); // a past or future month never gets a live pace reading
    }
    let day_of_month = dates::days_between(&month_start, &today) + 1;
    let days_in_month = dates::days_between(&month_start, &dates::end_of_month(&today)) + 1;
    let percent_elapsed = (day_of_month * 100) / days_in_month;
    let percent_used = (actual_amount_cents * 100) / effective_amount_cents;
    Ok(Some(PaceInfo {
        percent_used,
        percent_elapsed,
        status: classify_pace(percent_used, percent_elapsed),
    }))
}

fn median(mut values: Vec<i64>) -> i64 {
    values.sort_unstable();
    let mid = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2
    } else {
        values[mid]
    }
}

/// A deterministic "typical monthly spending" baseline for `category`,
/// looking back `lookback_months` months before (and not including)
/// `target_month`. Excludes any transaction (or split) flagged
/// `is_one_time` (section 9 - one unusual event must not permanently
/// distort the baseline) and only counts months that actually had
/// qualifying spending (a sporadic category's silent months are never
/// treated as fabricated $0 data points). Uses the *median* of qualifying
/// months rather than a mean (section 54), so a single remaining outlier
/// month still can't single-handedly drag the baseline. Returns `None`
/// when fewer than `MIN_MONTHS_FOR_TYPICAL` qualifying months exist
/// (section 53 - one data point is not a baseline), alongside however many
/// qualifying months were actually found either way.
/// `pub(crate)` (not just private) so Insights' category detail (Batch 8)
/// can show the exact same "typical spending" figure the Budgets page does
/// for that category, rather than a second, possibly-divergent definition
/// of "typical" (section 14: "do not create conflicting definitions").
pub(crate) fn historical_typical(
    conn: &Connection,
    category: &str,
    target_month: &str,
    lookback_months: i32,
) -> rusqlite::Result<(Option<i64>, i32)> {
    let mut monthly_totals = Vec::new();
    let mut cursor = target_month.to_string();
    for _ in 0..lookback_months {
        cursor = dates::add_months(&cursor, -1);
        let from = dates::start_of_month(&cursor);
        let to = dates::end_of_month(&cursor);
        let total =
            accounting::spending_in_range_excluding_one_time(conn, Some(category), &from, &to)?;
        if total != 0 {
            monthly_totals.push(total);
        }
    }
    let months_of_history = monthly_totals.len() as i32;
    if monthly_totals.len() < MIN_MONTHS_FOR_TYPICAL {
        return Ok((None, months_of_history));
    }
    Ok((Some(median(monthly_totals)), months_of_history))
}

fn hydrate(conn: &Connection, row: &rusqlite::Row) -> rusqlite::Result<Budget> {
    let category: String = row.get("category")?;
    let month: String = row.get("month")?;
    let planned_amount_cents: i64 = row.get("planned_amount_cents")?;
    let rollover_enabled: bool = row.get::<_, i64>("rollover_enabled")? != 0;
    let actual_amount_cents = actual_spending_for(conn, &category, &month)?;

    let rollover_in_cents = if rollover_enabled {
        rollover_in_for(conn, &category, &month)?
    } else {
        0
    };
    let effective_amount_cents = planned_amount_cents + rollover_in_cents;

    let status = if actual_amount_cents > effective_amount_cents {
        BudgetStatus::Over
    } else if actual_amount_cents as f64 >= effective_amount_cents as f64 * WARNING_THRESHOLD_RATIO
    {
        BudgetStatus::Near
    } else {
        BudgetStatus::Under
    };

    let (typical_amount_cents, typical_months_of_history) =
        historical_typical(conn, &category, &month, DEFAULT_LOOKBACK_MONTHS)?;
    let pace = pace_for(conn, &month, effective_amount_cents, actual_amount_cents)?;

    Ok(Budget {
        id: row.get("id")?,
        category,
        month,
        planned_amount_cents,
        classification: Classification::from_sql_str(&row.get::<_, String>("classification")?)?,
        rollover_enabled,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        actual_amount_cents,
        status,
        rollover_in_cents,
        effective_amount_cents,
        typical_amount_cents,
        typical_months_of_history,
        pace,
    })
}

fn get(conn: &Connection, id: i64) -> Result<Option<Budget>, BudgetError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM budgets WHERE id = ?1"),
        params![id],
        |row| hydrate(conn, row),
    )
    .optional()
    .map_err(BudgetError::from)
}

pub fn create(conn: &Connection, input: &BudgetInput) -> Result<Budget, BudgetError> {
    validate(input)?;
    let category = input.category.trim();
    if duplicate_exists(conn, category, &input.month, None)? {
        return Err(BudgetError::Duplicate);
    }
    let id = conn.query_row(
        "INSERT INTO budgets (category, month, planned_amount_cents, classification, rollover_enabled)
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
        params![
            category,
            input.month,
            input.planned_amount_cents,
            input.classification.as_sql_str(),
            input.rollover_enabled as i64,
        ],
        |row| row.get::<_, i64>(0),
    )?;
    get(conn, id)?.ok_or(BudgetError::NotFound(id))
}

pub fn update(conn: &Connection, id: i64, input: &BudgetInput) -> Result<Budget, BudgetError> {
    validate(input)?;
    let category = input.category.trim();
    if duplicate_exists(conn, category, &input.month, Some(id))? {
        return Err(BudgetError::Duplicate);
    }
    let affected = conn.execute(
        "UPDATE budgets SET category = ?1, month = ?2, planned_amount_cents = ?3,
            classification = ?4, rollover_enabled = ?5, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?6",
        params![
            category,
            input.month,
            input.planned_amount_cents,
            input.classification.as_sql_str(),
            input.rollover_enabled as i64,
            id,
        ],
    )?;
    if affected == 0 {
        return Err(BudgetError::NotFound(id));
    }
    get(conn, id)?.ok_or(BudgetError::NotFound(id))
}

pub fn delete(conn: &Connection, id: i64) -> Result<(), BudgetError> {
    let affected = conn.execute("DELETE FROM budgets WHERE id = ?1", params![id])?;
    if affected == 0 {
        return Err(BudgetError::NotFound(id));
    }
    Ok(())
}

/// Every budget row for one month (`YYYY-MM-01`), each hydrated with actual
/// spending and status. Also the "historical comparison" primitive - the
/// frontend calls this once per past month it wants to show; a month with no
/// budget rows simply returns empty, which is exactly "where data permits."
pub fn list_for_month(conn: &Connection, month: &str) -> Result<Vec<Budget>, BudgetError> {
    let month_start = crate::dates::start_of_month(month);
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM budgets WHERE month = ?1 ORDER BY category COLLATE NOCASE"
    ))?;
    let rows = stmt.query_map(params![month_start], |row| hydrate(conn, row))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(BudgetError::from)
}

#[derive(Debug, Clone, Serialize)]
pub struct BudgetSuggestion {
    pub category: String,
    pub typical_amount_cents: i64,
    pub suggested_cents: i64,
    pub months_of_history: i32,
    pub existing_planned_cents: Option<i64>,
}

/// Nearest $5 - simple and explainable, not a jagged three-decimal median.
fn round_to_nearest_five_dollars(cents: i64) -> i64 {
    let unit = 500;
    ((cents + unit / 2) / unit) * unit
}

/// Deterministic budget suggestions from recent categorized spending - a
/// proposal only, never applied without the owner's explicit action.
/// Built directly on `historical_typical` (median of qualifying months,
/// one-time-flagged spending excluded, section 54/9) so "typical spending"
/// and "suggested budget" are always the same number, just rounded - never
/// two different historical calculations that could quietly disagree. A
/// category needs at least `MIN_MONTHS_FOR_TYPICAL` qualifying months to
/// appear at all (section 53 - never suggest from one data point); the
/// candidate-category set itself still comes from `reports::
/// spending_by_category` across the lookback window, so a suggestion is
/// never classified differently than the Reports page would classify the
/// same spending.
pub fn suggest_budgets(
    conn: &Connection,
    target_month: &str,
    lookback_months: i32,
) -> Result<Vec<BudgetSuggestion>, BudgetError> {
    let mut candidate_categories: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    let mut cursor = target_month.to_string();
    for _ in 0..lookback_months {
        cursor = dates::add_months(&cursor, -1);
        let from = dates::start_of_month(&cursor);
        let to = dates::end_of_month(&cursor);
        for c in crate::reports::spending_by_category(conn, &from, &to)? {
            // "Uncategorized" can never be meaningfully budgeted - there is
            // no real category to type into the editor for it (section 40,
            // found live: it was otherwise appearing as a suggestable
            // category with a "suggested amount," implying a workflow that
            // doesn't actually make sense).
            if c.category != "Uncategorized" {
                candidate_categories.insert(c.category);
            }
        }
    }

    let existing_planned: std::collections::HashMap<String, i64> =
        list_for_month(conn, target_month)?
            .into_iter()
            .map(|b| (b.category, b.planned_amount_cents))
            .collect();

    let mut result = Vec::new();
    for category in candidate_categories {
        let (typical, months) = historical_typical(conn, &category, target_month, lookback_months)?;
        let Some(typical_amount_cents) = typical else {
            continue; // insufficient qualifying history - never fabricate a suggestion
        };
        result.push(BudgetSuggestion {
            existing_planned_cents: existing_planned.get(&category).copied(),
            suggested_cents: round_to_nearest_five_dollars(typical_amount_cents),
            typical_amount_cents,
            category,
            months_of_history: months,
        });
    }
    result.sort_by_key(|s| std::cmp::Reverse(s.typical_amount_cents));
    Ok(result)
}

/// Copies every budget row from `from_month` into `to_month` for any
/// category that doesn't already have one there - an explicit, one-click
/// action (never automatic) for the common case of carrying a steady
/// budget forward without re-typing every category every month (sections
/// 22-25's real underlying friction; the schema itself already makes every
/// month independent, so this is a UX convenience, not a new concept).
/// Never touches a category that already has a row for `to_month` - a
/// deliberate one-month exception the owner already set up is left
/// completely alone.
fn copy_month(conn: &Connection, from_month: &str, to_month: &str) -> Result<i64, BudgetError> {
    let source = list_for_month(conn, from_month)?;
    let existing: std::collections::HashSet<String> = list_for_month(conn, to_month)?
        .into_iter()
        .map(|b| b.category)
        .collect();
    let mut copied = 0i64;
    for b in source {
        if existing.contains(&b.category) {
            continue;
        }
        create(
            conn,
            &BudgetInput {
                category: b.category,
                month: to_month.to_string(),
                planned_amount_cents: b.planned_amount_cents,
                classification: b.classification,
                rollover_enabled: b.rollover_enabled,
            },
        )?;
        copied += 1;
    }
    Ok(copied)
}

/// Categories with real spending in `month` but no budget row for that
/// month - spending happening outside the plan (section 38), never
/// silently hidden. `Uncategorized` is deliberately included rather than
/// excluded: it can never have a budget row of its own (there is no
/// category text to type in), so it is unbudgeted by definition - the
/// frontend gives it its own call to action toward Needs Review instead of
/// "Set budget." A category with a net-negative total (pure refunds, no
/// purchases) is excluded - that is not spending happening outside a plan.
fn unbudgeted_spending_for_month(
    conn: &Connection,
    month: &str,
) -> Result<Vec<crate::reports::CategoryTotal>, BudgetError> {
    let from = dates::start_of_month(month);
    let to = dates::end_of_month(month);
    let all_spending = crate::reports::spending_by_category(conn, &from, &to)?;
    let budgeted: std::collections::HashSet<String> = list_for_month(conn, month)?
        .into_iter()
        .map(|b| b.category)
        .collect();
    Ok(all_spending
        .into_iter()
        .filter(|c| !budgeted.contains(&c.category) && c.spent_cents > 0)
        .collect())
}

#[tauri::command]
pub fn list_budgets(state: State<DbState>, month: String) -> Result<Vec<Budget>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list_for_month(&conn, &month).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_budget_suggestions(
    state: State<DbState>,
    target_month: String,
    lookback_months: i32,
) -> Result<Vec<BudgetSuggestion>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    suggest_budgets(&conn, &target_month, lookback_months).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_budget(state: State<DbState>, input: BudgetInput) -> Result<Budget, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    create(&conn, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_budget(state: State<DbState>, id: i64, input: BudgetInput) -> Result<Budget, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    update(&conn, id, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_budget(state: State<DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    delete(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn copy_budgets_from_month(
    state: State<DbState>,
    from_month: String,
    to_month: String,
) -> Result<i64, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    copy_month(&conn, &from_month, &to_month).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_unbudgeted_spending(
    state: State<DbState>,
    month: String,
) -> Result<Vec<crate::reports::CategoryTotal>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    unbudgeted_spending_for_month(&conn, &month).map_err(|e| e.to_string())
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
        conn.execute_batch(include_str!("../migrations/0005_budgets.sql"))
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
        conn.execute_batch(include_str!("../migrations/0014_budget_intelligence.sql"))
            .unwrap();
        conn
    }

    fn make_account(conn: &Connection) -> i64 {
        accounts::create(
            conn,
            &AccountInput {
                name: "Test Checking".to_string(),
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
        conn: &Connection,
        account_id: i64,
        date: &str,
        amount_cents: i64,
        transaction_type: TransactionType,
        category: &str,
    ) {
        crate::transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: date.to_string(),
                posted_date: None,
                amount_cents,
                transaction_type,
                raw_description: "Test".to_string(),
                merchant: None,
                category: Some(category.to_string()),
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

    fn add_one_time_txn(
        conn: &Connection,
        account_id: i64,
        date: &str,
        amount_cents: i64,
        category: &str,
    ) {
        crate::transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: date.to_string(),
                posted_date: None,
                amount_cents,
                transaction_type: TransactionType::Purchase,
                raw_description: "Test one-time".to_string(),
                merchant: None,
                category: Some(category.to_string()),
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
                payee_id: None,
                is_one_time: true,
            },
        )
        .unwrap();
    }

    fn sample_input() -> BudgetInput {
        BudgetInput {
            category: "Groceries".to_string(),
            month: "2026-08-01".to_string(),
            planned_amount_cents: 40000,
            classification: Classification::Flexible,
            rollover_enabled: false,
        }
    }

    #[test]
    fn create_and_get_round_trip_with_zero_actual_when_no_transactions() {
        let conn = test_conn();
        let budget = create(&conn, &sample_input()).unwrap();
        assert_eq!(budget.category, "Groceries");
        assert_eq!(budget.planned_amount_cents, 40000);
        assert_eq!(budget.actual_amount_cents, 0);
        assert_eq!(budget.status, BudgetStatus::Under);
    }

    #[test]
    fn rejects_duplicate_category_and_month() {
        let conn = test_conn();
        create(&conn, &sample_input()).unwrap();
        let err = create(&conn, &sample_input()).unwrap_err();
        assert!(matches!(err, BudgetError::Duplicate));
    }

    #[test]
    fn rejects_empty_category_and_negative_amount() {
        let conn = test_conn();
        let mut input = sample_input();
        input.category = "  ".to_string();
        assert!(matches!(
            create(&conn, &input),
            Err(BudgetError::Validation(_))
        ));

        let mut input2 = sample_input();
        input2.planned_amount_cents = -100;
        assert!(matches!(
            create(&conn, &input2),
            Err(BudgetError::Validation(_))
        ));
    }

    #[test]
    fn actual_spending_only_counts_matching_category_and_month() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &sample_input()).unwrap();

        add_txn(
            &conn,
            account_id,
            "2026-08-05",
            -10000,
            TransactionType::Purchase,
            "Groceries",
        );
        add_txn(
            &conn,
            account_id,
            "2026-08-10",
            -5000,
            TransactionType::Purchase,
            "Groceries",
        );
        add_txn(
            &conn,
            account_id,
            "2026-08-15",
            -9999,
            TransactionType::Purchase,
            "Dining",
        ); // different category
        add_txn(
            &conn,
            account_id,
            "2026-07-31",
            -2000,
            TransactionType::Purchase,
            "Groceries",
        ); // different month
        add_txn(
            &conn,
            account_id,
            "2026-09-01",
            -2000,
            TransactionType::Purchase,
            "Groceries",
        ); // different month

        let budgets = list_for_month(&conn, "2026-08-01").unwrap();
        assert_eq!(budgets.len(), 1);
        assert_eq!(budgets[0].actual_amount_cents, 15000);
    }

    // Batch 5.1: budgets.rs had zero liability-account coverage - a
    // credit-card purchase's positive amount_cents must still count once,
    // correctly, against its category budget, and the payment that later
    // pays the card off must never count a second time.
    #[test]
    fn credit_card_purchases_count_against_budget_once_and_payments_never_do() {
        let conn = test_conn();
        let card = accounts::create(
            &conn,
            &AccountInput {
                name: "Test Card".to_string(),
                institution: None,
                account_type: AccountType::CreditCard,
                include_in_net_worth: true,
                notes: None,
            },
        )
        .unwrap()
        .id;
        create(&conn, &sample_input()).unwrap(); // Groceries / 2026-08-01

        add_txn(
            &conn,
            card,
            "2026-08-05",
            8422,
            TransactionType::Purchase,
            "Groceries",
        );
        add_txn(
            &conn,
            card,
            "2026-08-20",
            -8422,
            TransactionType::Transfer,
            "Groceries",
        );

        let budgets = list_for_month(&conn, "2026-08-01").unwrap();
        assert_eq!(budgets.len(), 1);
        assert_eq!(budgets[0].actual_amount_cents, 8422);
    }

    #[test]
    fn budget_calculations_exclude_transfers_and_account_for_refunds() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &sample_input()).unwrap();

        add_txn(
            &conn,
            account_id,
            "2026-08-05",
            -20000,
            TransactionType::Purchase,
            "Groceries",
        );
        add_txn(
            &conn,
            account_id,
            "2026-08-06",
            5000,
            TransactionType::Refund,
            "Groceries",
        );
        add_txn(
            &conn,
            account_id,
            "2026-08-07",
            -30000,
            TransactionType::Transfer,
            "Groceries",
        ); // e.g. mis-tagged transfer, must not count

        let budgets = list_for_month(&conn, "2026-08-01").unwrap();
        assert_eq!(
            budgets[0].actual_amount_cents, 15000,
            "$200 spent minus $50 refunded = $150; the $300 transfer must not count at all"
        );
    }

    #[test]
    fn status_reflects_under_near_and_over_thresholds() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &sample_input()).unwrap(); // $400 planned

        add_txn(
            &conn,
            account_id,
            "2026-08-05",
            -10000,
            TransactionType::Purchase,
            "Groceries",
        ); // $100 - under
        assert_eq!(
            list_for_month(&conn, "2026-08-01").unwrap()[0].status,
            BudgetStatus::Under
        );

        add_txn(
            &conn,
            account_id,
            "2026-08-06",
            -27000,
            TransactionType::Purchase,
            "Groceries",
        ); // total $370 - near (>=90% of $400)
        assert_eq!(
            list_for_month(&conn, "2026-08-01").unwrap()[0].status,
            BudgetStatus::Near
        );

        add_txn(
            &conn,
            account_id,
            "2026-08-07",
            -5000,
            TransactionType::Purchase,
            "Groceries",
        ); // total $420 - over
        assert_eq!(
            list_for_month(&conn, "2026-08-01").unwrap()[0].status,
            BudgetStatus::Over
        );
    }

    #[test]
    fn update_changes_fields_and_delete_removes_the_row() {
        let conn = test_conn();
        let created = create(&conn, &sample_input()).unwrap();

        let mut changed = sample_input();
        changed.planned_amount_cents = 50000;
        changed.classification = Classification::Fixed;
        let updated = update(&conn, created.id, &changed).unwrap();
        assert_eq!(updated.planned_amount_cents, 50000);
        assert_eq!(updated.classification, Classification::Fixed);

        delete(&conn, created.id).unwrap();
        assert!(get(&conn, created.id).unwrap().is_none());
    }

    #[test]
    fn update_rejects_colliding_with_a_different_existing_budget() {
        let conn = test_conn();
        create(&conn, &sample_input()).unwrap();
        let mut other = sample_input();
        other.category = "Dining".to_string();
        let dining = create(&conn, &other).unwrap();

        let mut collide = sample_input();
        collide.category = "Groceries".to_string();
        let err = update(&conn, dining.id, &collide).unwrap_err();
        assert!(matches!(err, BudgetError::Duplicate));
    }

    #[test]
    fn historical_comparison_returns_empty_for_months_with_no_budget_permitting_graceful_absence() {
        let conn = test_conn();
        create(&conn, &sample_input()).unwrap(); // only August

        assert_eq!(list_for_month(&conn, "2026-08-15").unwrap().len(), 1);
        assert!(list_for_month(&conn, "2026-07-01").unwrap().is_empty());
        assert!(list_for_month(&conn, "2026-06-01").unwrap().is_empty());
    }

    #[test]
    fn suggest_budgets_averages_three_complete_months_and_rounds_to_nearest_five() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        // May/June/July groceries: $438, $472, $451 -> average $453.67 -> $455
        add_txn(
            &conn,
            account_id,
            "2026-05-10",
            -43800,
            TransactionType::Purchase,
            "Groceries",
        );
        add_txn(
            &conn,
            account_id,
            "2026-06-10",
            -47200,
            TransactionType::Purchase,
            "Groceries",
        );
        add_txn(
            &conn,
            account_id,
            "2026-07-10",
            -45100,
            TransactionType::Purchase,
            "Groceries",
        );

        let suggestions = suggest_budgets(&conn, "2026-08-01", 3).unwrap();
        let groceries = suggestions
            .iter()
            .find(|s| s.category == "Groceries")
            .unwrap();
        assert_eq!(groceries.months_of_history, 3);
        assert_eq!(
            groceries.typical_amount_cents, 45100,
            "median of $438/$472/$451 is the middle value, $451 - never the $453.67 mean"
        );
        assert_eq!(groceries.suggested_cents, 45000);
        assert_eq!(groceries.existing_planned_cents, None);
    }

    #[test]
    fn suggest_budgets_averages_only_over_months_that_actually_have_data() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        // Only June and July have any Dining spending - May must not be
        // treated as a fabricated $0 month diluting the average.
        add_txn(
            &conn,
            account_id,
            "2026-06-10",
            -6000,
            TransactionType::Purchase,
            "Dining",
        );
        add_txn(
            &conn,
            account_id,
            "2026-07-10",
            -8000,
            TransactionType::Purchase,
            "Dining",
        );

        let suggestions = suggest_budgets(&conn, "2026-08-01", 3).unwrap();
        let dining = suggestions.iter().find(|s| s.category == "Dining").unwrap();
        assert_eq!(dining.months_of_history, 2);
        assert_eq!(
            dining.typical_amount_cents, 7000,
            "median of $60/$80 (2 months, not fabricated against 3)"
        );
    }

    #[test]
    fn suggest_budgets_excludes_transfers_openings_adjustments_and_nets_refunds() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        // A second qualifying month so this still clears the new
        // "at least 2 months" minimum (section 53) - the actual behavior
        // under test (transfer/opening/adjustment exclusion, refund
        // netting) lives entirely in July's transactions below.
        add_txn(
            &conn,
            account_id,
            "2026-06-05",
            -12000,
            TransactionType::Purchase,
            "Shopping",
        );
        add_txn(
            &conn,
            account_id,
            "2026-07-05",
            -10000,
            TransactionType::Purchase,
            "Shopping",
        );
        add_txn(
            &conn,
            account_id,
            "2026-07-06",
            2000,
            TransactionType::Refund,
            "Shopping",
        );
        add_txn(
            &conn,
            account_id,
            "2026-07-07",
            -50000,
            TransactionType::Transfer,
            "Shopping",
        ); // credit-card payment shape
        accounting::reconcile_balance(&conn, account_id, 1, "2026-01-01").unwrap(); // opening balance
        accounting::reconcile_balance(&conn, account_id, 99_900, "2026-07-20").unwrap(); // balance adjustment

        let suggestions = suggest_budgets(&conn, "2026-08-01", 3).unwrap();
        let shopping = suggestions
            .iter()
            .find(|s| s.category == "Shopping")
            .unwrap();
        assert_eq!(
            shopping.typical_amount_cents, 10000,
            "June $120, July $100 spent - $20 refunded = $80; median of $120/$80 is $100 - \
             the transfer, opening balance, and adjustment must not count at all"
        );
        assert!(
            !suggestions.iter().any(|s| s.category == "Uncategorized"),
            "opening balance/adjustment rows must not surface as a spurious 'Uncategorized' suggestion"
        );
    }

    #[test]
    fn suggest_budgets_never_suggests_uncategorized_even_with_real_qualifying_spending() {
        // Found live: a real, honest-to-goodness uncategorized purchase (not
        // just an opening-balance edge case) still must never appear as a
        // suggestable category - there is no real category value to type
        // into the editor for it (section 40).
        let conn = test_conn();
        let account_id = make_account(&conn);
        for date in ["2026-06-10", "2026-07-10"] {
            crate::transactions::create(
                &conn,
                &TransactionInput {
                    account_id,
                    transaction_date: date.to_string(),
                    posted_date: None,
                    amount_cents: -5000,
                    transaction_type: TransactionType::Purchase,
                    raw_description: "Mystery charge".to_string(),
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
        }

        let suggestions = suggest_budgets(&conn, "2026-08-01", 3).unwrap();
        assert!(!suggestions.iter().any(|s| s.category == "Uncategorized"));
    }

    #[test]
    fn suggest_budgets_reports_the_existing_planned_amount_without_changing_it() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &sample_input()).unwrap(); // Groceries, August, $400 planned
        add_txn(
            &conn,
            account_id,
            "2026-06-10",
            -43000,
            TransactionType::Purchase,
            "Groceries",
        );
        add_txn(
            &conn,
            account_id,
            "2026-07-10",
            -45000,
            TransactionType::Purchase,
            "Groceries",
        );

        let suggestions = suggest_budgets(&conn, "2026-08-01", 3).unwrap();
        let groceries = suggestions
            .iter()
            .find(|s| s.category == "Groceries")
            .unwrap();
        assert_eq!(groceries.existing_planned_cents, Some(40000));

        // Merely asking for suggestions must never touch the stored budget.
        let unchanged = list_for_month(&conn, "2026-08-01").unwrap();
        assert_eq!(unchanged[0].planned_amount_cents, 40000);
    }

    #[test]
    fn suggest_budgets_fabricates_nothing_for_a_category_with_no_history() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(
            &conn,
            account_id,
            "2026-07-10",
            -1000,
            TransactionType::Purchase,
            "Dining",
        );

        let suggestions = suggest_budgets(&conn, "2026-08-01", 3).unwrap();
        assert!(!suggestions.iter().any(|s| s.category == "Groceries"));
    }

    #[test]
    fn suggest_budgets_returns_empty_when_there_is_no_spending_history_at_all() {
        let conn = test_conn();
        make_account(&conn);
        let suggestions = suggest_budgets(&conn, "2026-08-01", 3).unwrap();
        assert!(suggestions.is_empty());
    }

    #[test]
    fn a_balance_adjustment_never_counts_toward_a_budgets_actual_spending() {
        // ADR-0009 regression: reconciling an account's balance must not
        // leak into budget actuals, no matter how large the correction is.
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &sample_input()).unwrap(); // Groceries, August, $400 planned

        add_txn(
            &conn,
            account_id,
            "2026-08-05",
            -10000,
            TransactionType::Purchase,
            "Groceries",
        );
        accounting::reconcile_balance(&conn, account_id, 50_000, "2026-07-01").unwrap();

        let before = list_for_month(&conn, "2026-08-01").unwrap();
        assert_eq!(before[0].actual_amount_cents, 10000);

        // A large same-month adjustment that would badly distort the
        // budget if it were ever counted as spending.
        accounting::reconcile_balance(&conn, account_id, 999_000, "2026-08-20").unwrap();

        let after = list_for_month(&conn, "2026-08-01").unwrap();
        assert_eq!(
            after[0].actual_amount_cents, 10000,
            "the adjustment must never be counted as budget activity"
        );
    }

    // --- Classification ---

    #[test]
    fn classification_round_trips_including_discretionary() {
        let conn = test_conn();
        let mut input = sample_input();
        input.classification = Classification::Discretionary;
        let created = create(&conn, &input).unwrap();
        assert_eq!(created.classification, Classification::Discretionary);
        let fetched = get(&conn, created.id).unwrap().unwrap();
        assert_eq!(fetched.classification, Classification::Discretionary);
    }

    // --- Rollover ---

    fn rollover_input(
        category: &str,
        month: &str,
        planned_cents: i64,
        rollover_enabled: bool,
    ) -> BudgetInput {
        BudgetInput {
            category: category.to_string(),
            month: month.to_string(),
            planned_amount_cents: planned_cents,
            classification: Classification::Flexible,
            rollover_enabled,
        }
    }

    #[test]
    fn rollover_disabled_carries_nothing_forward_even_after_a_large_underspend() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &rollover_input("Travel", "2026-01-01", 20000, false)).unwrap();
        add_txn(
            &conn,
            account_id,
            "2026-01-05",
            -1000,
            TransactionType::Purchase,
            "Travel",
        );
        create(&conn, &rollover_input("Travel", "2026-02-01", 20000, false)).unwrap();

        let feb = list_for_month(&conn, "2026-02-01").unwrap();
        assert_eq!(feb[0].rollover_in_cents, 0);
        assert_eq!(feb[0].effective_amount_cents, 20000);
    }

    #[test]
    fn rollover_enabled_carries_a_surplus_forward() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &rollover_input("Travel", "2026-01-01", 30000, true)).unwrap();
        add_txn(
            &conn,
            account_id,
            "2026-01-05",
            -20000,
            TransactionType::Purchase,
            "Travel",
        ); // $200 of $300
        create(&conn, &rollover_input("Travel", "2026-02-01", 30000, true)).unwrap();

        let feb = list_for_month(&conn, "2026-02-01").unwrap();
        assert_eq!(
            feb[0].rollover_in_cents, 10000,
            "January banked a $100 surplus"
        );
        assert_eq!(feb[0].effective_amount_cents, 40000);
    }

    #[test]
    fn rollover_enabled_carries_a_deficit_forward() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &rollover_input("Travel", "2026-01-01", 30000, true)).unwrap();
        add_txn(
            &conn,
            account_id,
            "2026-01-05",
            -35000,
            TransactionType::Purchase,
            "Travel",
        ); // $350 of $300
        create(&conn, &rollover_input("Travel", "2026-02-01", 30000, true)).unwrap();

        let feb = list_for_month(&conn, "2026-02-01").unwrap();
        assert_eq!(
            feb[0].rollover_in_cents, -5000,
            "January overspent by $50, which must reduce February's available amount"
        );
        assert_eq!(feb[0].effective_amount_cents, 25000);
    }

    #[test]
    fn rollover_compounds_across_consecutive_months() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &rollover_input("Travel", "2026-01-01", 20000, true)).unwrap();
        add_txn(
            &conn,
            account_id,
            "2026-01-05",
            -10000,
            TransactionType::Purchase,
            "Travel",
        ); // +$100 surplus
        create(&conn, &rollover_input("Travel", "2026-02-01", 20000, true)).unwrap();
        add_txn(
            &conn,
            account_id,
            "2026-02-05",
            -10000,
            TransactionType::Purchase,
            "Travel",
        ); // +$100 surplus, on top of Jan's carry
        create(&conn, &rollover_input("Travel", "2026-03-01", 20000, true)).unwrap();

        let feb = list_for_month(&conn, "2026-02-01").unwrap();
        assert_eq!(feb[0].rollover_in_cents, 10000);

        let march = list_for_month(&conn, "2026-03-01").unwrap();
        assert_eq!(
            march[0].rollover_in_cents, 20000,
            "two consecutive $100 surpluses must compound to $200, not reset each month"
        );
        assert_eq!(march[0].effective_amount_cents, 40000);
    }

    #[test]
    fn rollover_stops_at_a_month_with_no_budget_row_instead_of_reaching_past_the_gap() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &rollover_input("Travel", "2026-01-01", 10000, true)).unwrap();
        add_txn(
            &conn,
            account_id,
            "2026-01-05",
            0,
            TransactionType::Purchase,
            "Travel",
        ); // technically a $0 txn just to exercise the path; no spending
           // February has no budget row for Travel at all - a gap.
        create(&conn, &rollover_input("Travel", "2026-03-01", 10000, true)).unwrap();

        let march = list_for_month(&conn, "2026-03-01").unwrap();
        assert_eq!(
            march[0].rollover_in_cents, 0,
            "a gap month breaks the chain - January's leftover has nothing to relay through"
        );
    }

    #[test]
    fn rollover_disabled_on_a_month_stops_the_chain_from_that_point_forward() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &rollover_input("Travel", "2026-01-01", 20000, true)).unwrap();
        add_txn(
            &conn,
            account_id,
            "2026-01-05",
            -10000,
            TransactionType::Purchase,
            "Travel",
        );
        create(&conn, &rollover_input("Travel", "2026-02-01", 20000, false)).unwrap(); // owner turns rollover off
        add_txn(
            &conn,
            account_id,
            "2026-02-05",
            -10000,
            TransactionType::Purchase,
            "Travel",
        );
        create(&conn, &rollover_input("Travel", "2026-03-01", 20000, true)).unwrap();

        let march = list_for_month(&conn, "2026-03-01").unwrap();
        assert_eq!(
            march[0].rollover_in_cents, 0,
            "February opted out, so nothing flows past it even though March re-enabled rollover"
        );
    }

    #[test]
    fn editing_a_later_months_budget_never_rewrites_an_earlier_months_stored_amount() {
        // Section 71/24's non-negotiable requirement, proven directly: each
        // month is its own row, so there is no shared "base" for an edit to
        // retroactively rewrite.
        let conn = test_conn();
        let january = create(
            &conn,
            &rollover_input("Groceries", "2026-01-01", 25000, false),
        )
        .unwrap();
        let august = create(
            &conn,
            &rollover_input("Groceries", "2026-08-01", 25000, false),
        )
        .unwrap();

        update(
            &conn,
            august.id,
            &rollover_input("Groceries", "2026-08-01", 30000, false),
        )
        .unwrap();

        let january_after = get(&conn, january.id).unwrap().unwrap();
        assert_eq!(
            january_after.planned_amount_cents, 25000,
            "August's edit must never change what January's own budget was"
        );
    }

    #[test]
    fn status_is_judged_against_the_effective_amount_including_rollover_not_the_raw_planned_figure()
    {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &rollover_input("Travel", "2026-01-01", 30000, true)).unwrap();
        add_txn(
            &conn,
            account_id,
            "2026-01-05",
            -10000,
            TransactionType::Purchase,
            "Travel",
        ); // +$200 surplus carried in
        create(&conn, &rollover_input("Travel", "2026-02-01", 30000, true)).unwrap();
        // $310 spent against a raw $300 plan would normally be "Over," but
        // the $200 rollover credit makes $500 the real capacity.
        add_txn(
            &conn,
            account_id,
            "2026-02-05",
            -31000,
            TransactionType::Purchase,
            "Travel",
        );

        let feb = list_for_month(&conn, "2026-02-01").unwrap();
        assert_eq!(feb[0].effective_amount_cents, 50000);
        assert_eq!(feb[0].status, BudgetStatus::Under);
    }

    // --- Pace ---

    #[test]
    fn classify_pace_uses_the_documented_tolerance_band() {
        assert_eq!(
            classify_pace(52, 43),
            PaceStatus::OnTrack,
            "the handoff's own worked example must read as on track"
        );
        assert_eq!(classify_pace(50, 50), PaceStatus::OnTrack);
        assert_eq!(
            classify_pace(53, 43),
            PaceStatus::OnTrack,
            "exactly at the +10 boundary is still on track"
        );
        assert_eq!(
            classify_pace(54, 43),
            PaceStatus::Faster,
            "just past the +10 boundary"
        );
        assert_eq!(classify_pace(30, 43), PaceStatus::Slower);
        assert_eq!(classify_pace(200, 50), PaceStatus::Faster);
    }

    #[test]
    fn pace_is_populated_only_for_the_real_current_month() {
        let conn = test_conn();
        let today_str = today(&conn).unwrap();
        let current_month = dates::start_of_month(&today_str);
        let past_month = dates::start_of_month(&dates::add_months(&today_str, -2));
        let future_month = dates::start_of_month(&dates::add_months(&today_str, 2));

        assert!(pace_for(&conn, &current_month, 10000, 5000)
            .unwrap()
            .is_some());
        assert!(
            pace_for(&conn, &past_month, 10000, 5000).unwrap().is_none(),
            "a completed month's own final status already answers this, not a stale pace reading"
        );
        assert!(
            pace_for(&conn, &future_month, 10000, 5000)
                .unwrap()
                .is_none(),
            "a future month has not started yet - pace is meaningless"
        );
    }

    #[test]
    fn pace_is_none_for_a_zero_or_negative_effective_amount() {
        let conn = test_conn();
        let today_str = today(&conn).unwrap();
        let current_month = dates::start_of_month(&today_str);
        assert!(pace_for(&conn, &current_month, 0, 0).unwrap().is_none());
        assert!(pace_for(&conn, &current_month, -500, 0).unwrap().is_none());
    }

    #[test]
    fn a_confirmed_budget_in_the_current_month_carries_a_live_pace_reading() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        let today_str = today(&conn).unwrap();
        let current_month = dates::start_of_month(&today_str);
        create(
            &conn,
            &rollover_input("Dining", &current_month, 10000, false),
        )
        .unwrap();
        add_txn(
            &conn,
            account_id,
            &today_str,
            -5000,
            TransactionType::Purchase,
            "Dining",
        );

        let budgets = list_for_month(&conn, &current_month).unwrap();
        assert!(
            budgets[0].pace.is_some(),
            "the current month must carry a live pace reading"
        );
    }

    // --- Historical typical baseline ---

    #[test]
    fn historical_typical_excludes_one_time_flagged_spending() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(
            &conn,
            account_id,
            "2026-05-10",
            -24000,
            TransactionType::Purchase,
            "Dining",
        );
        add_txn(
            &conn,
            account_id,
            "2026-06-10",
            -24000,
            TransactionType::Purchase,
            "Dining",
        );
        // A one-off $600 event, flagged one-time - must never distort the baseline.
        add_one_time_txn(&conn, account_id, "2026-07-10", -60000, "Dining");
        add_txn(
            &conn,
            account_id,
            "2026-07-15",
            -24000,
            TransactionType::Purchase,
            "Dining",
        );

        let (typical, months) = historical_typical(&conn, "Dining", "2026-08-01", 6).unwrap();
        assert_eq!(
            months, 3,
            "three qualifying months, the one-time flag doesn't remove July entirely"
        );
        assert_eq!(
            typical,
            Some(24000),
            "every qualifying month nets to exactly $240 once the one-time $600 is excluded"
        );
    }

    #[test]
    fn historical_typical_is_none_with_fewer_than_two_qualifying_months() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(
            &conn,
            account_id,
            "2026-07-10",
            -5000,
            TransactionType::Purchase,
            "Dining",
        );

        let (typical, months) = historical_typical(&conn, "Dining", "2026-08-01", 6).unwrap();
        assert_eq!(months, 1);
        assert_eq!(
            typical, None,
            "one month is not a meaningful baseline (section 53)"
        );
    }

    #[test]
    fn historical_typical_uses_the_median_so_one_outlier_month_cannot_dominate() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        add_txn(
            &conn,
            account_id,
            "2026-04-10",
            -20000,
            TransactionType::Purchase,
            "Shopping",
        );
        add_txn(
            &conn,
            account_id,
            "2026-05-10",
            -21000,
            TransactionType::Purchase,
            "Shopping",
        );
        add_txn(
            &conn,
            account_id,
            "2026-06-10",
            -22000,
            TransactionType::Purchase,
            "Shopping",
        );
        // Not one-time-flagged, so the median (not the one-time exclusion)
        // is what has to keep this from dominating.
        add_txn(
            &conn,
            account_id,
            "2026-07-10",
            -900000,
            TransactionType::Purchase,
            "Shopping",
        );

        let (typical, _) = historical_typical(&conn, "Shopping", "2026-08-01", 6).unwrap();
        assert_eq!(
            typical,
            Some(21500),
            "median of 4 values averages the middle two: ($200/$210/$220/$9000 sorted, \
             ($210+$220)/2 = $215) - never dragged toward the $9000 outlier"
        );
    }

    // --- copy_month ---

    #[test]
    fn copy_month_copies_missing_categories_and_never_touches_an_existing_one() {
        let conn = test_conn();
        create(
            &conn,
            &rollover_input("Groceries", "2026-08-01", 45000, false),
        )
        .unwrap();
        create(&conn, &rollover_input("Dining", "2026-08-01", 30000, true)).unwrap();
        // September already has its own, deliberately different, Dining budget.
        create(&conn, &rollover_input("Dining", "2026-09-01", 99900, false)).unwrap();

        let copied = copy_month(&conn, "2026-08-01", "2026-09-01").unwrap();
        assert_eq!(copied, 1, "only Groceries was missing from September");

        let september = list_for_month(&conn, "2026-09-01").unwrap();
        let groceries = september
            .iter()
            .find(|b| b.category == "Groceries")
            .unwrap();
        assert_eq!(groceries.planned_amount_cents, 45000);
        let dining = september.iter().find(|b| b.category == "Dining").unwrap();
        assert_eq!(
            dining.planned_amount_cents, 99900,
            "September's own pre-existing Dining budget must be left completely alone"
        );
    }

    // --- Unbudgeted spending ---

    #[test]
    fn unbudgeted_spending_lists_categories_with_no_budget_row_including_uncategorized() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(
            &conn,
            &rollover_input("Groceries", "2026-08-01", 45000, false),
        )
        .unwrap();
        add_txn(
            &conn,
            account_id,
            "2026-08-05",
            -10000,
            TransactionType::Purchase,
            "Groceries",
        ); // budgeted
        add_txn(
            &conn,
            account_id,
            "2026-08-06",
            -5000,
            TransactionType::Purchase,
            "Entertainment",
        ); // unbudgeted
        crate::transactions::create(
            &conn,
            &TransactionInput {
                account_id,
                transaction_date: "2026-08-07".to_string(),
                posted_date: None,
                amount_cents: -1500,
                transaction_type: TransactionType::Purchase,
                raw_description: "Mystery charge".to_string(),
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
        // A category with only a refund nets negative - never "unbudgeted spending."
        add_txn(
            &conn,
            account_id,
            "2026-08-08",
            3000,
            TransactionType::Refund,
            "Warranty Credit",
        );

        let unbudgeted = unbudgeted_spending_for_month(&conn, "2026-08-01").unwrap();
        let categories: Vec<&str> = unbudgeted.iter().map(|c| c.category.as_str()).collect();
        assert!(categories.contains(&"Entertainment"));
        assert!(categories.contains(&"Uncategorized"));
        assert!(!categories.contains(&"Groceries"), "Groceries has a budget");
        assert!(
            !categories.contains(&"Warranty Credit"),
            "a net-negative category is not spending outside the plan"
        );
    }

    // --- Split allocation (section 82) ---

    #[test]
    fn historical_typical_and_suggestions_use_a_splits_own_allocation_not_the_parents_full_amount()
    {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        // Two months of a Walmart trip split Groceries/Household, so the
        // Groceries baseline must be built from the $80 splits, never the
        // $120 parent totals.
        for date in ["2026-06-05", "2026-07-05"] {
            let parent = crate::transactions::create(
                &conn,
                &TransactionInput {
                    account_id,
                    transaction_date: date.to_string(),
                    posted_date: None,
                    amount_cents: -12000,
                    transaction_type: TransactionType::Purchase,
                    raw_description: "Walmart".to_string(),
                    merchant: None,
                    category: Some("Groceries".to_string()),
                    subcategory: None,
                    notes: None,
                    tags: None,
                    is_pending: None,
                    payee_id: None,
                    is_one_time: false,
                },
            )
            .unwrap();
            crate::splits::replace_for_transaction(
                &mut conn,
                parent.id,
                &[
                    crate::splits::SplitInput {
                        category: Some("Groceries".to_string()),
                        subcategory: None,
                        amount_cents: -8000,
                        notes: None,
                    },
                    crate::splits::SplitInput {
                        category: Some("Household".to_string()),
                        subcategory: None,
                        amount_cents: -4000,
                        notes: None,
                    },
                ],
            )
            .unwrap();
        }

        let (typical, _) = historical_typical(&conn, "Groceries", "2026-08-01", 6).unwrap();
        assert_eq!(
            typical,
            Some(8000),
            "each month's Groceries baseline must be the $80 split, never the $120 parent"
        );
        let (household_typical, _) =
            historical_typical(&conn, "Household", "2026-08-01", 6).unwrap();
        assert_eq!(household_typical, Some(4000));
    }
}
