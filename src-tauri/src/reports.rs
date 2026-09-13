//! Phase 5: Reports and Trends - useful drilldowns over the existing
//! ledger, not decorative charts. Every figure here either reuses
//! `accounting.rs`'s existing range primitives directly or is the smallest
//! new aggregate nothing else in the app currently computes (a
//! category-by-category breakdown; a multi-month income/spending trend).
//! The actual "drilldown" - seeing the real transactions behind a total -
//! is left to the frontend re-using `transactions::list`'s existing
//! category+date filter, not a second, redundant transaction-browsing
//! backend.

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::State;

use crate::accounting;
use crate::dates;
use crate::db::DbState;
use crate::transactions::TransactionType;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategoryTotal {
    pub category: String,
    pub spent_cents: i64,
}

/// Spending by category for a date range, highest first - the "where did
/// my money go" drilldown. A category with zero net spending (e.g. fully
/// offset by a refund) is omitted rather than shown as a useless zero row.
/// Reuses `TransactionType::from_sql_str`/`accounting::spending_effect`
/// directly, so a category total is never classified differently than
/// `spending_in_range` would classify the same transaction.
///
/// Split-aware (Transaction Intelligence, specs/phase-012): a transaction
/// with rows in `transaction_splits` contributes its *children's*
/// category/amount pairs instead of its own single one - the parent's own
/// `amount_cents` is never also counted, so a split transaction's total
/// contribution here is still exactly its own ledger amount, just broken
/// across categories rather than lumped into one. An unsplit transaction's
/// query branch is byte-identical to before this batch.
pub fn spending_by_category(
    conn: &Connection,
    from: &str,
    to: &str,
) -> rusqlite::Result<Vec<CategoryTotal>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(s.category, 'Uncategorized'), s.amount_cents, t.transaction_type
         FROM transactions t
         JOIN transaction_splits s ON s.transaction_id = t.id
         WHERE t.transaction_date >= ?1 AND t.transaction_date <= ?2 AND t.is_archived = 0
         UNION ALL
         SELECT COALESCE(t.category, 'Uncategorized'), t.amount_cents, t.transaction_type
         FROM transactions t
         WHERE t.transaction_date >= ?1 AND t.transaction_date <= ?2 AND t.is_archived = 0
           AND NOT EXISTS (SELECT 1 FROM transaction_splits s WHERE s.transaction_id = t.id)",
    )?;
    let rows: Vec<(String, i64, String)> = stmt
        .query_map(params![from, to], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut totals: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for (category, amount_cents, type_str) in rows {
        let transaction_type = TransactionType::from_sql_str(&type_str)?;
        let effect = accounting::spending_effect(transaction_type, amount_cents);
        if effect != 0 {
            *totals.entry(category).or_insert(0) += effect;
        }
    }

    let mut result: Vec<CategoryTotal> = totals
        .into_iter()
        .filter(|(_, spent)| *spent != 0)
        .map(|(category, spent_cents)| CategoryTotal {
            category,
            spent_cents,
        })
        .collect();
    result.sort_by_key(|c| std::cmp::Reverse(c.spent_cents));
    Ok(result)
}

#[derive(Debug, Clone, Serialize)]
pub struct MonthlyFlow {
    pub month: String,
    pub income_cents: i64,
    pub spending_cents: i64,
}

const TREND_MONTHS: i64 = 6;

fn today(conn: &Connection) -> rusqlite::Result<String> {
    conn.query_row("SELECT date('now')", [], |row| row.get(0))
}

/// Income vs. spending for each of the last `TREND_MONTHS` full months, plus
/// the current month to date - the same "N months plus today" shape
/// `accounting::net_worth_history` already established, just for flow
/// instead of a point-in-time balance.
pub fn income_vs_spending_trend(conn: &Connection) -> rusqlite::Result<Vec<MonthlyFlow>> {
    let today = today(conn)?;
    let mut points = Vec::new();
    for i in (1..=TREND_MONTHS).rev() {
        let month_date = dates::add_months(&today, -i);
        let from = dates::start_of_month(&month_date);
        let to = dates::end_of_month(&month_date);
        points.push(MonthlyFlow {
            income_cents: accounting::income_in_range(conn, &from, &to)?,
            spending_cents: accounting::spending_in_range(conn, None, &from, &to)?,
            month: from,
        });
    }
    let this_month_start = dates::start_of_month(&today);
    points.push(MonthlyFlow {
        income_cents: accounting::income_in_range(conn, &this_month_start, &today)?,
        spending_cents: accounting::spending_in_range(conn, None, &this_month_start, &today)?,
        month: this_month_start,
    });
    Ok(points)
}

#[tauri::command]
pub fn get_spending_by_category(
    state: State<DbState>,
    from: String,
    to: String,
) -> Result<Vec<CategoryTotal>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    spending_by_category(&conn, &from, &to).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_income_vs_spending_trend(state: State<DbState>) -> Result<Vec<MonthlyFlow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    income_vs_spending_trend(&conn).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::transactions::{self, TransactionInput};

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
        conn: &Connection,
        account_id: i64,
        date: &str,
        amount_cents: i64,
        transaction_type: TransactionType,
        category: Option<&str>,
    ) {
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: date.to_string(),
                posted_date: None,
                amount_cents,
                transaction_type,
                raw_description: "Test txn".to_string(),
                merchant: None,
                category: category.map(|c| c.to_string()),
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
    fn spending_by_category_sums_and_sorts_highest_first_and_omits_zero_net_categories() {
        let conn = test_conn();
        let account = make_account(&conn);
        add_txn(
            &conn,
            account,
            "2026-08-05",
            -3000,
            TransactionType::Purchase,
            Some("Dining"),
        );
        add_txn(
            &conn,
            account,
            "2026-08-10",
            -12000,
            TransactionType::Purchase,
            Some("Groceries"),
        );
        add_txn(
            &conn,
            account,
            "2026-08-12",
            -4000,
            TransactionType::Purchase,
            Some("Dining"),
        );
        // A category fully offset by its own refund must not appear.
        add_txn(
            &conn,
            account,
            "2026-08-14",
            -1000,
            TransactionType::Purchase,
            Some("Returns"),
        );
        add_txn(
            &conn,
            account,
            "2026-08-15",
            1000,
            TransactionType::Refund,
            Some("Returns"),
        );
        // A transfer must never be classified as spending in any category.
        add_txn(
            &conn,
            account,
            "2026-08-16",
            -50000,
            TransactionType::Transfer,
            Some("Transfers"),
        );

        let totals = spending_by_category(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(
            totals,
            vec![
                CategoryTotal {
                    category: "Groceries".to_string(),
                    spent_cents: 12000
                },
                CategoryTotal {
                    category: "Dining".to_string(),
                    spent_cents: 7000
                },
            ]
        );
    }

    #[test]
    fn spending_by_category_expands_a_split_transaction_into_its_childrens_categories() {
        let mut conn = test_conn();
        let account = make_account(&conn);
        let walmart_id = transactions::create(
            &conn,
            &TransactionInput {
                account_id: account,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -12000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Walmart".to_string(),
                merchant: None,
                category: Some("Shopping".to_string()),
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
                payee_id: None,
                is_one_time: false,
            },
        )
        .unwrap()
        .id;
        crate::splits::replace_for_transaction(
            &mut conn,
            walmart_id,
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
        add_txn(
            &conn,
            account,
            "2026-08-06",
            -3000,
            TransactionType::Purchase,
            Some("Dining"),
        );

        let totals = spending_by_category(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert!(
            totals.iter().all(|c| c.category != "Shopping"),
            "the parent's own category must not also appear once it has been split: {totals:?}"
        );
        assert_eq!(
            totals
                .iter()
                .find(|c| c.category == "Groceries")
                .unwrap()
                .spent_cents,
            8000
        );
        assert_eq!(
            totals
                .iter()
                .find(|c| c.category == "Household")
                .unwrap()
                .spent_cents,
            4000
        );
        assert_eq!(
            totals
                .iter()
                .find(|c| c.category == "Dining")
                .unwrap()
                .spent_cents,
            3000,
            "an ordinary unsplit transaction in the same range must be completely unaffected"
        );
        let total_spending: i64 = totals.iter().map(|c| c.spent_cents).sum();
        assert_eq!(
            total_spending, 15000,
            "12000 (split across two categories) + 3000 (unsplit) - never double-counted"
        );
    }

    #[test]
    fn spending_by_category_uncategorized_bucket_and_balance_adjustments_excluded() {
        let conn = test_conn();
        let account = make_account(&conn);
        add_txn(
            &conn,
            account,
            "2026-08-05",
            -2000,
            TransactionType::Purchase,
            None,
        );
        crate::accounting::reconcile_balance(&conn, account, 1, "2026-01-01").unwrap();
        crate::accounting::reconcile_balance(&conn, account, 500_000, "2026-08-10").unwrap();

        let totals = spending_by_category(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].category, "Uncategorized");
        assert_eq!(totals[0].spent_cents, 2000);
    }

    #[test]
    fn income_vs_spending_trend_has_seven_points_scoped_to_their_own_month() {
        let conn = test_conn();
        let account = make_account(&conn);
        let today: String = conn
            .query_row("SELECT date('now')", [], |r| r.get(0))
            .unwrap();
        let last_month = dates::add_months(&today, -1);

        add_txn(
            &conn,
            account,
            &dates::start_of_month(&last_month),
            250_000,
            TransactionType::Income,
            None,
        );
        add_txn(
            &conn,
            account,
            &dates::start_of_month(&last_month),
            -80000,
            TransactionType::Purchase,
            None,
        );
        add_txn(
            &conn,
            account,
            &today,
            300_000,
            TransactionType::Income,
            None,
        );
        add_txn(
            &conn,
            account,
            &today,
            -1000,
            TransactionType::Purchase,
            None,
        );

        let trend = income_vs_spending_trend(&conn).unwrap();
        assert_eq!(trend.len(), 7);

        let last_month_point = &trend[TREND_MONTHS as usize - 1];
        assert_eq!(last_month_point.income_cents, 250_000);
        assert_eq!(last_month_point.spending_cents, 80000);

        let current_point = &trend[TREND_MONTHS as usize];
        assert_eq!(current_point.income_cents, 300_000);
        assert_eq!(current_point.spending_cents, 1000);
    }
}
