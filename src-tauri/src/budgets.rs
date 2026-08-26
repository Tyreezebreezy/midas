//! Monthly category budgets (Phase 4B). Deliberately not an envelope
//! system - no rollover, no fund transfers between categories: one planned
//! amount per category per month, compared against actual spending computed
//! by `accounting::total_spending` (reused, not reimplemented, so budgets
//! automatically exclude transfers and correctly net out refunds).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::accounting;
use crate::db::DbState;
use crate::transactions::TransactionType;

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

/// Sums `accounting::spending_effect` over every transaction in `category`
/// within the calendar month containing `month` (a query direct against
/// `transactions`, not `transactions::list`, so the whole call stays in
/// `rusqlite::Result` and can be composed with `?` inside `hydrate`'s own
/// `query_map` closure - reuses the actually-important logic, `spending_effect`,
/// without a cross-module error-type mismatch).
fn actual_spending_for(conn: &Connection, category: &str, month: &str) -> rusqlite::Result<i64> {
    let month_start = crate::dates::start_of_month(month);
    let month_end = crate::dates::end_of_month(month);
    let mut stmt = conn.prepare(
        "SELECT amount_cents, transaction_type FROM transactions
         WHERE category = ?1 AND transaction_date >= ?2 AND transaction_date <= ?3 AND is_archived = 0",
    )?;
    let rows = stmt.query_map(params![category, month_start, month_end], |row| {
        let amount_cents: i64 = row.get(0)?;
        let transaction_type = TransactionType::from_sql_str(&row.get::<_, String>(1)?)?;
        Ok(accounting::spending_effect(transaction_type, amount_cents))
    })?;
    rows.collect::<rusqlite::Result<Vec<i64>>>()
        .map(|amounts| amounts.iter().sum())
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

#[tauri::command]
pub fn list_budgets(state: State<DbState>, month: String) -> Result<Vec<Budget>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list_for_month(&conn, &month).map_err(|e| e.to_string())
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
}
