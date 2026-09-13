//! Monthly category budgets (Phase 4B). Deliberately not an envelope
//! system - no rollover, no fund transfers between categories: one planned
//! amount per category per month, compared against actual spending computed
//! by `accounting::spending_in_range` (reused, not reimplemented, so budgets
//! automatically exclude transfers and correctly net out refunds).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::accounting;
use crate::db::DbState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Fixed,
    Flexible,
}

impl Classification {
    fn as_sql_str(self) -> &'static str {
        match self {
            Classification::Fixed => "fixed",
            Classification::Flexible => "flexible",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "fixed" => Classification::Fixed,
            "flexible" => Classification::Flexible,
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

/// Under 90% of planned, at/over 90%, or over planned entirely. A fixed
/// convention rather than a per-budget configurable threshold - nothing in
/// the request asked for per-category tuning, and a stored config field
/// nobody sets is exactly the kind of unused flexibility to avoid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStatus {
    Under,
    Near,
    Over,
}

const WARNING_THRESHOLD_RATIO: f64 = 0.9;

#[derive(Debug, Clone, Serialize)]
pub struct Budget {
    pub id: i64,
    pub category: String,
    pub month: String,
    pub planned_amount_cents: i64,
    pub classification: Classification,
    pub created_at: String,
    pub updated_at: String,
    pub actual_amount_cents: i64,
    pub status: BudgetStatus,
}

#[derive(Debug, Deserialize)]
pub struct BudgetInput {
    pub category: String,
    pub month: String,
    pub planned_amount_cents: i64,
    pub classification: Classification,
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

const COLUMNS: &str =
    "id, category, month, planned_amount_cents, classification, created_at, updated_at";

/// Actual spending for `category` within the calendar month containing
/// `month` - delegates to `accounting::spending_in_range`, the one shared
/// place "spending in a date range" is computed (also used by pay_cycle.rs).
fn actual_spending_for(conn: &Connection, category: &str, month: &str) -> rusqlite::Result<i64> {
    accounting::spending_in_range(
        conn,
        Some(category),
        &crate::dates::start_of_month(month),
        &crate::dates::end_of_month(month),
    )
}

fn hydrate(conn: &Connection, row: &rusqlite::Row) -> rusqlite::Result<Budget> {
    let category: String = row.get("category")?;
    let month: String = row.get("month")?;
    let planned_amount_cents: i64 = row.get("planned_amount_cents")?;
    let actual_amount_cents = actual_spending_for(conn, &category, &month)?;

    let status = if actual_amount_cents > planned_amount_cents {
        BudgetStatus::Over
    } else if actual_amount_cents as f64 >= planned_amount_cents as f64 * WARNING_THRESHOLD_RATIO {
        BudgetStatus::Near
    } else {
        BudgetStatus::Under
    };

    Ok(Budget {
        id: row.get("id")?,
        category,
        month,
        planned_amount_cents,
        classification: Classification::from_sql_str(&row.get::<_, String>("classification")?)?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        actual_amount_cents,
        status,
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
        "INSERT INTO budgets (category, month, planned_amount_cents, classification)
         VALUES (?1, ?2, ?3, ?4) RETURNING id",
        params![
            category,
            input.month,
            input.planned_amount_cents,
            input.classification.as_sql_str(),
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
            classification = ?4, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?5",
        params![
            category,
            input.month,
            input.planned_amount_cents,
            input.classification.as_sql_str(),
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
    pub average_actual_cents: i64,
    pub suggested_cents: i64,
    pub months_of_history: i32,
    pub existing_planned_cents: Option<i64>,
}

/// Nearest $5 - simple and explainable, not a jagged three-decimal average.
fn round_to_nearest_five_dollars(cents: i64) -> i64 {
    let unit = 500;
    ((cents + unit / 2) / unit) * unit
}

/// Deterministic budget suggestions from recent categorized spending - a
/// proposal only, never applied without the owner's explicit action.
/// Reuses `reports::spending_by_category` for each lookback month so a
/// suggestion is never classified differently than the Reports page would
/// classify the same spending (transfers/opening balances/balance
/// adjustments already excluded there, refunds already netted). A category
/// absent from every lookback month simply never appears - no invented
/// zero-spending history.
pub fn suggest_budgets(
    conn: &Connection,
    target_month: &str,
    lookback_months: i32,
) -> Result<Vec<BudgetSuggestion>, BudgetError> {
    let mut totals: std::collections::BTreeMap<String, (i64, i32)> =
        std::collections::BTreeMap::new();
    let mut cursor = target_month.to_string();
    for _ in 0..lookback_months {
        cursor = crate::dates::add_months(&cursor, -1);
        let from = crate::dates::start_of_month(&cursor);
        let to = crate::dates::end_of_month(&cursor);
        for c in crate::reports::spending_by_category(conn, &from, &to)? {
            let entry = totals.entry(c.category).or_insert((0, 0));
            entry.0 += c.spent_cents;
            entry.1 += 1;
        }
    }

    let existing_planned: std::collections::HashMap<String, i64> =
        list_for_month(conn, target_month)?
            .into_iter()
            .map(|b| (b.category, b.planned_amount_cents))
            .collect();

    let mut result: Vec<BudgetSuggestion> = totals
        .into_iter()
        .map(|(category, (sum_cents, months))| {
            let average_actual_cents = sum_cents / months as i64;
            BudgetSuggestion {
                existing_planned_cents: existing_planned.get(&category).copied(),
                suggested_cents: round_to_nearest_five_dollars(average_actual_cents),
                average_actual_cents,
                category,
                months_of_history: months,
            }
        })
        .collect();
    result.sort_by_key(|s| std::cmp::Reverse(s.average_actual_cents));
    Ok(result)
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

    fn sample_input() -> BudgetInput {
        BudgetInput {
            category: "Groceries".to_string(),
            month: "2026-08-01".to_string(),
            planned_amount_cents: 40000,
            classification: Classification::Flexible,
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
        assert_eq!(groceries.average_actual_cents, 45366); // integer division truncates ($453.66)
        assert_eq!(groceries.suggested_cents, 45500);
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
            dining.average_actual_cents, 7000,
            "($60 + $80) / 2 months, not / 3"
        );
    }

    #[test]
    fn suggest_budgets_excludes_transfers_openings_adjustments_and_nets_refunds() {
        let conn = test_conn();
        let account_id = make_account(&conn);
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
            shopping.average_actual_cents, 8000,
            "$100 spent - $20 refunded = $80; the transfer, opening balance, and adjustment must not count"
        );
        assert!(
            !suggestions.iter().any(|s| s.category == "Uncategorized"),
            "opening balance/adjustment rows must not surface as a spurious 'Uncategorized' suggestion"
        );
    }

    #[test]
    fn suggest_budgets_reports_the_existing_planned_amount_without_changing_it() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(&conn, &sample_input()).unwrap(); // Groceries, August, $400 planned
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
}
