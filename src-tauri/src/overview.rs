//! Phase 5: the Overview home screen - pure composition over the modules
//! Phase 4 already built, in the owner's own priority order (Safe to
//! Spend, Upcoming Bills, Current Pay Cycle, Spending, Needs Review,
//! Savings Goals, then supporting cash/net-worth/budget/trend context). No
//! new calculation happens here; every figure is a direct call into an
//! existing module's already-tested function, capped where a list needs to
//! stay glanceable rather than becoming a second, redundant list page.

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::accounting::{self, NetWorthBreakdown, NetWorthPoint};
use crate::accounts;
use crate::budgets::{self, Budget};
use crate::dates;
use crate::db::DbState;
use crate::needs_review::{self, NeedsReviewItem};
use crate::pay_cycle::{self, CycleSummary};
use crate::recurring::RecurringSeries;
use crate::safe_to_spend::{self, SafeToSpendResult};
use crate::savings_goals::{self, SavingsGoal};
use crate::transactions::{self, Transaction, TransactionFilter};

const UPCOMING_BILLS_LIMIT: usize = 5;
const NEEDS_REVIEW_PREVIEW_LIMIT: usize = 5;
const RECENT_TRANSACTIONS_LIMIT: usize = 8;

fn today(conn: &Connection) -> rusqlite::Result<String> {
    conn.query_row("SELECT date('now')", [], |row| row.get(0))
}

#[derive(Debug, Serialize)]
pub struct OverviewResult {
    // Priority 1-6, per the owner's own ordering. `safe_to_spend` carries
    // eligible_cash_cents on both variants, which doubles as the "cash
    // position" supporting figure - no separate field duplicating it.
    pub safe_to_spend: SafeToSpendResult,
    pub upcoming_bills: Vec<RecurringSeries>,
    pub pay_cycle: Option<CycleSummary>,
    pub needs_review_count: usize,
    pub needs_review_items: Vec<NeedsReviewItem>,
    pub savings_goals: Vec<SavingsGoal>,
    // Supporting information.
    pub net_worth: NetWorthBreakdown,
    pub budget_health: Vec<Budget>,
    pub recent_transactions: Vec<Transaction>,
    pub income_this_month_cents: i64,
    pub spending_this_month_cents: i64,
    pub net_worth_trend: Vec<NetWorthPoint>,
}

pub fn build(conn: &Connection) -> Result<OverviewResult, String> {
    let today = today(conn).map_err(|e| e.to_string())?;
    let month_start = dates::start_of_month(&today);

    let safe_to_spend = safe_to_spend::calculate(conn).map_err(|e| e.to_string())?;
    let upcoming_bills =
        pay_cycle::upcoming_bills(conn, UPCOMING_BILLS_LIMIT).map_err(|e| e.to_string())?;
    let pay_cycle = pay_cycle::summarize(conn).map_err(|e| e.to_string())?;

    let all_needs_review = needs_review::list(conn, true).map_err(|e| e.to_string())?;
    let needs_review_count = all_needs_review.len();
    let needs_review_items = all_needs_review
        .into_iter()
        .take(NEEDS_REVIEW_PREVIEW_LIMIT)
        .collect();

    let savings_goals = savings_goals::list(conn, false).map_err(|e| e.to_string())?;

    let account_list = accounts::list(conn, false).map_err(|e| e.to_string())?;
    let net_worth = accounting::net_worth_breakdown(&account_list);

    let budget_health = budgets::list_for_month(conn, &month_start).map_err(|e| e.to_string())?;

    let recent_transactions = transactions::list(conn, &TransactionFilter::default())
        .map_err(|e| e.to_string())?
        .into_iter()
        .take(RECENT_TRANSACTIONS_LIMIT)
        .collect();

    let income_this_month_cents =
        accounting::income_in_range(conn, &month_start, &today).map_err(|e| e.to_string())?;
    let spending_this_month_cents = accounting::spending_in_range(conn, None, &month_start, &today)
        .map_err(|e| e.to_string())?;

    let net_worth_trend = accounting::net_worth_history(conn).map_err(|e| e.to_string())?;

    Ok(OverviewResult {
        safe_to_spend,
        upcoming_bills,
        pay_cycle,
        needs_review_count,
        needs_review_items,
        savings_goals,
        net_worth,
        budget_health,
        recent_transactions,
        income_this_month_cents,
        spending_this_month_cents,
        net_worth_trend,
    })
}

#[tauri::command]
pub fn get_overview(state: State<DbState>) -> Result<OverviewResult, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    build(&conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{AccountInput, AccountType};
    use crate::budgets::{BudgetInput, Classification};
    use crate::needs_review;
    use crate::pay_cycle::PaySchedule;
    use crate::recurring;
    use crate::savings_goals::SavingsGoalInput;
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
        conn.execute_batch(include_str!("../migrations/0006_savings_goals.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../migrations/0007_balance_adjustment.sql"))
            .unwrap();
        conn
    }

    fn today_str(conn: &Connection) -> String {
        today(conn).unwrap()
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
        add_txn_with_merchant(
            conn,
            account_id,
            date,
            amount_cents,
            transaction_type,
            category,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn add_txn_with_merchant(
        conn: &Connection,
        account_id: i64,
        date: &str,
        amount_cents: i64,
        transaction_type: TransactionType,
        category: Option<&str>,
        merchant: Option<&str>,
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
                merchant: merchant.map(|m| m.to_string()),
                category: category.map(|c| c.to_string()),
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn build_composes_every_section_from_real_data() {
        let conn = test_conn();
        let today = today_str(&conn);
        let account = make_account(&conn);
        accounting::reconcile_balance(&conn, account, 500_000, &dates::add_days(&today, -60))
            .unwrap();
        pay_cycle::set_schedule(
            &conn,
            &PaySchedule::Biweekly {
                anchor_date: today.clone(),
            },
        )
        .unwrap();
        add_txn(
            &conn,
            account,
            &today,
            -4000,
            TransactionType::Purchase,
            Some("Groceries"),
        );
        add_txn(
            &conn,
            account,
            &today,
            300_000,
            TransactionType::Income,
            None,
        );
        budgets::create(
            &conn,
            &BudgetInput {
                category: "Groceries".to_string(),
                month: dates::start_of_month(&today),
                planned_amount_cents: 40000,
                classification: Classification::Flexible,
            },
        )
        .unwrap();
        savings_goals::create(
            &conn,
            &SavingsGoalInput {
                name: "Emergency Fund".to_string(),
                target_amount_cents: 1_000_000,
                current_amount_cents: 200_000,
                target_date: None,
                reserved_amount_cents: None,
            },
        )
        .unwrap();
        needs_review::list(&conn, true).unwrap(); // sanity: table exists, starts empty

        let overview = build(&conn).unwrap();

        assert!(matches!(
            overview.safe_to_spend,
            SafeToSpendResult::Complete { .. }
        ));
        assert_eq!(overview.savings_goals.len(), 1);
        assert_eq!(overview.savings_goals[0].name, "Emergency Fund");
        assert_eq!(overview.net_worth.total_assets_cents, 796_000);
        assert_eq!(overview.budget_health.len(), 1);
        assert_eq!(overview.budget_health[0].actual_amount_cents, 4000);
        assert_eq!(overview.income_this_month_cents, 300_000);
        assert_eq!(overview.spending_this_month_cents, 4000);
        assert_eq!(overview.needs_review_count, 0);
        assert!(overview.recent_transactions.len() >= 2);
        assert_eq!(overview.net_worth_trend.len(), 7);
        let cycle = overview.pay_cycle.expect("schedule was set");
        assert_eq!(cycle.spending_this_cycle_cents, 4000);
    }

    #[test]
    fn capped_lists_respect_their_limits_while_counts_stay_accurate() {
        let conn = test_conn();
        let today = today_str(&conn);
        let account = make_account(&conn);

        // Seed more confirmed bills than the cap.
        for (i, merchant) in ["Rent", "Streaming", "Gym", "Insurance", "Phone", "Internet"]
            .iter()
            .enumerate()
        {
            let base = dates::add_days(&today, -200 + (i as i64));
            for n in 0..3 {
                add_txn_with_merchant(
                    &conn,
                    account,
                    &dates::add_months(&base, n),
                    -1000,
                    TransactionType::Purchase,
                    None,
                    Some(merchant),
                );
            }
        }
        for candidate in recurring::detect_candidates(&conn, account).unwrap() {
            recurring::confirm(&conn, candidate.id).unwrap();
        }

        // Seed more open needs-review items than the preview cap.
        for _ in 0..(NEEDS_REVIEW_PREVIEW_LIMIT + 3) {
            conn.execute(
                "INSERT INTO needs_review_items (issue_type, detected_reason) VALUES ('other', 'test')",
                [],
            )
            .unwrap();
        }

        // Seed more transactions than the recent-transactions cap.
        for i in 0..(RECENT_TRANSACTIONS_LIMIT + 3) {
            add_txn(
                &conn,
                account,
                &dates::add_days(&today, -(i as i64)),
                -100,
                TransactionType::Purchase,
                None,
            );
        }

        let overview = build(&conn).unwrap();

        assert_eq!(overview.upcoming_bills.len(), UPCOMING_BILLS_LIMIT);
        assert_eq!(
            overview.needs_review_items.len(),
            NEEDS_REVIEW_PREVIEW_LIMIT
        );
        assert_eq!(
            overview.needs_review_count,
            NEEDS_REVIEW_PREVIEW_LIMIT + 3,
            "the true count must not be truncated even though the preview list is"
        );
        assert_eq!(
            overview.recent_transactions.len(),
            RECENT_TRANSACTIONS_LIMIT
        );
    }

    #[test]
    fn income_and_spending_this_month_are_month_to_date_and_exclude_other_months() {
        let conn = test_conn();
        let today = today_str(&conn);
        let account = make_account(&conn);
        let month_start = dates::start_of_month(&today);
        let last_month = dates::add_months(&today, -1);

        add_txn(
            &conn,
            account,
            &today,
            100_000,
            TransactionType::Income,
            None,
        );
        add_txn(
            &conn,
            account,
            &today,
            -2500,
            TransactionType::Purchase,
            None,
        );
        // Prior month must never leak into this-month figures.
        add_txn(
            &conn,
            account,
            &last_month,
            999_000,
            TransactionType::Income,
            None,
        );
        add_txn(
            &conn,
            account,
            &last_month,
            -99_900,
            TransactionType::Purchase,
            None,
        );
        // A balance adjustment this month must never count as either.
        accounting::reconcile_balance(&conn, account, 5_000_000, &month_start).unwrap();
        accounting::reconcile_balance(&conn, account, 6_000_000, &today).unwrap();

        let overview = build(&conn).unwrap();
        assert_eq!(overview.income_this_month_cents, 100_000);
        assert_eq!(overview.spending_this_month_cents, 2500);
    }
}
