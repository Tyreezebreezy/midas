//! Insights (Batch 8, specs/phase-015): Phase 5's Reports evolved into a
//! full "what happened" analytics surface - Spending, Income, Merchants,
//! Cash Flow. Net Worth's own period-scoped change lives in `accounting.rs`
//! (`net_worth_change`) since accounting.rs already owns net worth entirely;
//! this module covers everything else.
//!
//! Every period (Month/Quarter/Year, "same point last month" for a still-
//! incomplete current period, previous-equivalent-period, etc.) is a
//! FRONTEND concern (`src/lib/period.ts`) - every command here just takes
//! plain `from`/`to` ISO date bounds, exactly like
//! `reports::spending_by_category` already did before this batch. This
//! module never needs its own notion of a period, so Spending/Income/Cash
//! Flow/Net Worth can never disagree about what "this month" means.
//!
//! Category totals reuse `reports::spending_by_category`
//! (`accounting::split_expanded_rows` underneath) - the one split-aware
//! source of truth Budgets already depends on too. Merchant/payee and
//! income-source totals are deliberately NOT split-aware (section 93 - a
//! split transaction's categories must never multiply its own merchant/payee
//! total): they read `transactions` directly at the parent level, which is
//! already that transaction's one true ledger amount regardless of how many
//! categories it's broken into for reporting.

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::State;

use crate::accounting;
use crate::budgets;
use crate::db::DbState;
use crate::reports::{self, CategoryTotal};
use crate::transactions::TransactionType;

// ---------- Spending ----------

#[derive(Debug, Clone, Serialize)]
pub struct SpendingInsights {
    pub total_cents: i64,
    pub one_time_cents: i64,
    pub recurring_cents: i64,
    pub non_recurring_cents: i64,
    pub categories: Vec<CategoryTotal>,
}

pub fn spending_insights(
    conn: &Connection,
    from: &str,
    to: &str,
) -> rusqlite::Result<SpendingInsights> {
    let total_cents = accounting::spending_in_range(conn, None, from, to)?;
    let ordinary_cents = accounting::spending_in_range_excluding_one_time(conn, None, from, to)?;
    let rows = fetch_spending_rows(conn, from, to)?;
    let (recurring_cents, non_recurring_cents) = recurring_split(&rows);
    let categories = reports::spending_by_category(conn, from, to)?;
    Ok(SpendingInsights {
        total_cents,
        one_time_cents: total_cents - ordinary_cents,
        recurring_cents,
        non_recurring_cents,
        categories,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryDetail {
    pub category: String,
    pub spent_cents: i64,
    pub previous_spent_cents: i64,
    /// `None` below `budgets::MIN_MONTHS_FOR_TYPICAL` qualifying months of
    /// history - identical "not enough history yet" rule Budgets uses, so
    /// the two surfaces never show conflicting typical-spending figures.
    pub typical_cents: Option<i64>,
    pub typical_months_of_history: i32,
    pub transaction_count: i64,
    pub one_time_cents: i64,
}

/// `historical_typical` looks backward from a single calendar month, so this
/// is most meaningful when `[from, to]` is itself one month (Month period
/// mode) - for a quarter/year selection it looks back from whichever month
/// `to` falls in, a reasonable approximation the frontend is free to hide
/// outside Month mode rather than something this function needs to gate.
pub fn category_detail(
    conn: &Connection,
    category: &str,
    from: &str,
    to: &str,
    previous_from: &str,
    previous_to: &str,
) -> rusqlite::Result<CategoryDetail> {
    let spent_cents = accounting::spending_in_range(conn, Some(category), from, to)?;
    let previous_spent_cents =
        accounting::spending_in_range(conn, Some(category), previous_from, previous_to)?;
    let ordinary_cents =
        accounting::spending_in_range_excluding_one_time(conn, Some(category), from, to)?;
    let (typical_cents, typical_months_of_history) = budgets::historical_typical(
        conn,
        category,
        &crate::dates::start_of_month(to),
        budgets::DEFAULT_LOOKBACK_MONTHS,
    )?;
    let transaction_count = category_transaction_count(conn, category, from, to)?;
    Ok(CategoryDetail {
        category: category.to_string(),
        spent_cents,
        previous_spent_cents,
        typical_cents,
        typical_months_of_history,
        transaction_count,
        one_time_cents: spent_cents - ordinary_cents,
    })
}

/// Distinct transactions touching `category`, whether directly or via one of
/// its splits - the same split-or-unsplit UNION shape
/// `accounting::split_expanded_rows` uses, but counting whole transactions
/// (`UNION`, not `UNION ALL`) rather than summing per-split amounts.
fn category_transaction_count(
    conn: &Connection,
    category: &str,
    from: &str,
    to: &str,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM (
            SELECT t.id FROM transactions t
            JOIN transaction_splits s ON s.transaction_id = t.id
            WHERE COALESCE(s.category, 'Uncategorized') = ?1
              AND t.transaction_date >= ?2 AND t.transaction_date <= ?3 AND t.is_archived = 0
            UNION
            SELECT t.id FROM transactions t
            WHERE COALESCE(t.category, 'Uncategorized') = ?1
              AND t.transaction_date >= ?2 AND t.transaction_date <= ?3 AND t.is_archived = 0
              AND NOT EXISTS (SELECT 1 FROM transaction_splits s WHERE s.transaction_id = t.id)
         )",
        params![category, from, to],
        |row| row.get(0),
    )
}

// ---------- Shared: parent-level identity rows (merchants + income sources) ----------

struct IdentityRow {
    payee_id: Option<i64>,
    payee_name: Option<String>,
    merchant: String,
    category: String,
    recurring_series_id: Option<i64>,
    amount_cents: i64,
    transaction_type: TransactionType,
}

const IDENTITY_COLUMNS: &str =
    "t.payee_id, p.name, t.merchant, COALESCE(t.category, 'Uncategorized'), \
    t.recurring_series_id, t.amount_cents, t.transaction_type";

fn hydrate_identity_row(row: &rusqlite::Row) -> rusqlite::Result<IdentityRow> {
    Ok(IdentityRow {
        payee_id: row.get(0)?,
        payee_name: row.get(1)?,
        merchant: row.get(2)?,
        category: row.get(3)?,
        recurring_series_id: row.get(4)?,
        amount_cents: row.get(5)?,
        transaction_type: TransactionType::from_sql_str(&row.get::<_, String>(6)?)?,
    })
}

/// Purchases and refunds only (never transfers/opening balances/adjustments),
/// the same expense-side types `accounting::spending_effect` treats as
/// spending, at the parent transaction level (never split-expanded; see the
/// module doc comment).
fn fetch_spending_rows(
    conn: &Connection,
    from: &str,
    to: &str,
) -> rusqlite::Result<Vec<IdentityRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {IDENTITY_COLUMNS} FROM transactions t
         LEFT JOIN payees p ON p.id = t.payee_id
         WHERE t.transaction_date >= ?1 AND t.transaction_date <= ?2 AND t.is_archived = 0
           AND t.transaction_type IN ('purchase', 'refund')"
    ))?;
    let rows = stmt
        .query_map(params![from, to], hydrate_identity_row)?
        .collect();
    rows
}

/// Income only - never a transfer, and never a merchant refund (section 17).
fn fetch_income_rows(
    conn: &Connection,
    from: &str,
    to: &str,
) -> rusqlite::Result<Vec<IdentityRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {IDENTITY_COLUMNS} FROM transactions t
         LEFT JOIN payees p ON p.id = t.payee_id
         WHERE t.transaction_date >= ?1 AND t.transaction_date <= ?2 AND t.is_archived = 0
           AND t.transaction_type = 'income'"
    ))?;
    let rows = stmt
        .query_map(params![from, to], hydrate_identity_row)?
        .collect();
    rows
}

/// This row's own net contribution: `accounting::spending_effect` for a
/// purchase/refund (positive = spent, negative = refunded - so a category
/// or merchant with more refunded than spent nets negative rather than
/// silently inflating both sides, the same "refunds netted" rule the rest
/// of MIDAS already follows) or the plain already-positive amount for
/// income. `fetch_spending_rows`/`fetch_income_rows` never return any other
/// transaction type, so nothing else reaches this match.
fn signed(row: &IdentityRow) -> i64 {
    match row.transaction_type {
        TransactionType::Income => row.amount_cents,
        _ => accounting::spending_effect(row.transaction_type, row.amount_cents),
    }
}

fn recurring_split(rows: &[IdentityRow]) -> (i64, i64) {
    let mut recurring = 0i64;
    let mut other = 0i64;
    for row in rows {
        if row.recurring_series_id.is_some() {
            recurring += signed(row);
        } else {
            other += signed(row);
        }
    }
    (recurring, other)
}

#[derive(Debug, Clone, Serialize)]
pub struct PayeeTotal {
    pub payee_id: Option<i64>,
    pub name: String,
    pub amount_cents: i64,
    pub transaction_count: i64,
}

fn identity_key(row: &IdentityRow) -> String {
    match row.payee_id {
        Some(id) => format!("p{id}"),
        None => format!("m{}", row.merchant),
    }
}

fn group_by_identity(rows: &[IdentityRow]) -> Vec<PayeeTotal> {
    use std::collections::BTreeMap;
    let mut totals: BTreeMap<String, PayeeTotal> = BTreeMap::new();
    for row in rows {
        let entry = totals
            .entry(identity_key(row))
            .or_insert_with(|| PayeeTotal {
                payee_id: row.payee_id,
                name: row
                    .payee_name
                    .clone()
                    .unwrap_or_else(|| row.merchant.clone()),
                amount_cents: 0,
                transaction_count: 0,
            });
        entry.amount_cents += signed(row);
        entry.transaction_count += 1;
    }
    let mut result: Vec<PayeeTotal> = totals
        .into_values()
        .filter(|p| p.amount_cents != 0)
        .collect();
    result.sort_by_key(|p| std::cmp::Reverse(p.amount_cents));
    result
}

// ---------- Income ----------

#[derive(Debug, Clone, Serialize)]
pub struct IncomeInsights {
    pub total_cents: i64,
    pub recurring_cents: i64,
    pub non_recurring_cents: i64,
    pub sources: Vec<PayeeTotal>,
}

pub fn income_insights(
    conn: &Connection,
    from: &str,
    to: &str,
) -> rusqlite::Result<IncomeInsights> {
    let rows = fetch_income_rows(conn, from, to)?;
    let total_cents: i64 = rows.iter().map(signed).sum();
    let (recurring_cents, non_recurring_cents) = recurring_split(&rows);
    Ok(IncomeInsights {
        total_cents,
        recurring_cents,
        non_recurring_cents,
        sources: group_by_identity(&rows),
    })
}

// ---------- Merchants ----------

pub fn top_merchants(conn: &Connection, from: &str, to: &str) -> rusqlite::Result<Vec<PayeeTotal>> {
    Ok(group_by_identity(&fetch_spending_rows(conn, from, to)?))
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryShare {
    pub category: String,
    pub amount_cents: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MerchantDetail {
    pub name: String,
    pub spent_cents: i64,
    pub previous_spent_cents: i64,
    pub transaction_count: i64,
    pub average_cents: i64,
    pub categories: Vec<CategoryShare>,
    pub is_recurring: bool,
}

fn matches_identity(row: &IdentityRow, payee_id: Option<i64>, merchant: Option<&str>) -> bool {
    match payee_id {
        Some(id) => row.payee_id == Some(id),
        None => row.payee_id.is_none() && Some(row.merchant.as_str()) == merchant,
    }
}

pub fn merchant_detail(
    conn: &Connection,
    payee_id: Option<i64>,
    merchant: Option<&str>,
    from: &str,
    to: &str,
    previous_from: &str,
    previous_to: &str,
) -> rusqlite::Result<MerchantDetail> {
    let rows: Vec<IdentityRow> = fetch_spending_rows(conn, from, to)?
        .into_iter()
        .filter(|r| matches_identity(r, payee_id, merchant))
        .collect();
    let previous_rows = fetch_spending_rows(conn, previous_from, previous_to)?;
    let previous_spent_cents: i64 = previous_rows
        .iter()
        .filter(|r| matches_identity(r, payee_id, merchant))
        .map(signed)
        .sum();

    let spent_cents: i64 = rows.iter().map(signed).sum();
    let transaction_count = rows.len() as i64;
    let name = rows
        .first()
        .and_then(|r| r.payee_name.clone())
        .or_else(|| merchant.map(str::to_string))
        .unwrap_or_default();

    let mut category_totals: std::collections::BTreeMap<String, i64> =
        std::collections::BTreeMap::new();
    for row in &rows {
        *category_totals.entry(row.category.clone()).or_insert(0) += signed(row);
    }
    let mut categories: Vec<CategoryShare> = category_totals
        .into_iter()
        .map(|(category, amount_cents)| CategoryShare {
            category,
            amount_cents,
        })
        .collect();
    categories.sort_by_key(|c| std::cmp::Reverse(c.amount_cents));

    let is_recurring: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM recurring_series
         WHERE status = 'confirmed' AND ((?1 IS NOT NULL AND payee_id = ?1) OR merchant = ?2))",
        params![payee_id, merchant.unwrap_or_default()],
        |row| row.get(0),
    )?;

    Ok(MerchantDetail {
        name,
        spent_cents,
        previous_spent_cents,
        transaction_count,
        average_cents: if transaction_count > 0 {
            spent_cents / transaction_count
        } else {
            0
        },
        categories,
        is_recurring,
    })
}

// ---------- Cash Flow ----------

#[derive(Debug, Clone, Serialize)]
pub struct CashFlowInsights {
    pub income_cents: i64,
    pub spending_cents: i64,
    pub net_cents: i64,
}

/// Income minus spending - deliberately distinct from net worth's own
/// change (section 21/75), which also reflects unrealized valuation moves
/// on manual assets and isn't just "money in minus money out." Internal
/// transfers already never reach either side, since neither
/// `accounting::income_in_range` nor `spending_in_range` count them.
pub fn cash_flow_insights(
    conn: &Connection,
    from: &str,
    to: &str,
) -> rusqlite::Result<CashFlowInsights> {
    let income_cents = accounting::income_in_range(conn, from, to)?;
    let spending_cents = accounting::spending_in_range(conn, None, from, to)?;
    Ok(CashFlowInsights {
        income_cents,
        spending_cents,
        net_cents: income_cents - spending_cents,
    })
}

// ---------- Tauri commands ----------

#[tauri::command]
pub fn get_spending_insights(
    state: State<DbState>,
    from: String,
    to: String,
) -> Result<SpendingInsights, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    spending_insights(&conn, &from, &to).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_category_detail(
    state: State<DbState>,
    category: String,
    from: String,
    to: String,
    previous_from: String,
    previous_to: String,
) -> Result<CategoryDetail, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    category_detail(&conn, &category, &from, &to, &previous_from, &previous_to)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_income_insights(
    state: State<DbState>,
    from: String,
    to: String,
) -> Result<IncomeInsights, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    income_insights(&conn, &from, &to).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_top_merchants(
    state: State<DbState>,
    from: String,
    to: String,
) -> Result<Vec<PayeeTotal>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    top_merchants(&conn, &from, &to).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_merchant_detail(
    state: State<DbState>,
    payee_id: Option<i64>,
    merchant: Option<String>,
    from: String,
    to: String,
    previous_from: String,
    previous_to: String,
) -> Result<MerchantDetail, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    merchant_detail(
        &conn,
        payee_id,
        merchant.as_deref(),
        &from,
        &to,
        &previous_from,
        &previous_to,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_cash_flow_insights(
    state: State<DbState>,
    from: String,
    to: String,
) -> Result<CashFlowInsights, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    cash_flow_insights(&conn, &from, &to).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::payees::{self, PayeeInput, PayeeType};
    use crate::recurring::{self, Cadence, Direction, Kind, PatternEditInput};
    use crate::transactions::{self, TransactionInput};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_transactions.sql"),
            include_str!("../migrations/0003_rules_and_recurring.sql"),
            include_str!("../migrations/0004_accounting_boundary.sql"),
            include_str!("../migrations/0005_budgets.sql"),
            include_str!("../migrations/0007_balance_adjustment.sql"),
            include_str!("../migrations/0008_foundation_batch_1.sql"),
            include_str!("../migrations/0009_import_engine_v2.sql"),
            include_str!("../migrations/0010_ofx_source_adapter.sql"),
            include_str!("../migrations/0011_reconciliation.sql"),
            include_str!("../migrations/0012_transaction_intelligence.sql"),
            include_str!("../migrations/0013_recurring_intelligence.sql"),
            include_str!("../migrations/0014_budget_intelligence.sql"),
        ] {
            conn.execute_batch(sql).unwrap();
        }
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

    #[allow(clippy::too_many_arguments)]
    fn add_full(
        conn: &Connection,
        account_id: i64,
        date: &str,
        amount_cents: i64,
        transaction_type: TransactionType,
        merchant: Option<&str>,
        category: Option<&str>,
        payee_id: Option<i64>,
        is_one_time: bool,
    ) -> i64 {
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: date.to_string(),
                posted_date: None,
                amount_cents,
                transaction_type,
                raw_description: merchant.unwrap_or("Test").to_string(),
                merchant: merchant.map(|m| m.to_string()),
                category: category.map(|c| c.to_string()),
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
                payee_id,
                is_one_time,
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
    ) -> i64 {
        add_full(
            conn,
            account_id,
            date,
            amount_cents,
            transaction_type,
            None,
            category,
            None,
            false,
        )
    }

    fn make_payee(conn: &Connection, name: &str) -> i64 {
        payees::create(
            conn,
            &PayeeInput {
                name: name.to_string(),
                payee_type: PayeeType::Merchant,
                notes: None,
            },
        )
        .unwrap()
        .id
    }

    fn confirm_series(
        conn: &mut Connection,
        account_id: i64,
        direction: Direction,
        merchant: &str,
        amount_cents: i64,
        next_date: &str,
    ) -> i64 {
        let series = recurring::create_manual(
            conn,
            account_id,
            direction,
            &PatternEditInput {
                merchant: merchant.to_string(),
                payee_id: None,
                kind: Kind::Other,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: amount_cents,
                next_expected_date: next_date.to_string(),
                notes: None,
            },
        )
        .unwrap();
        series.id
    }

    // --- Spending (section 90) ---

    #[test]
    fn spending_insights_totals_checking_and_credit_card_purchases_together() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let card = make_account(&conn, AccountType::CreditCard);
        add_txn(
            &conn,
            checking,
            "2026-08-05",
            -3000,
            TransactionType::Purchase,
            Some("Dining"),
        );
        add_txn(
            &conn,
            card,
            "2026-08-06",
            8422,
            TransactionType::Purchase,
            Some("Dining"),
        );

        let insights = spending_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(insights.total_cents, 3000 + 8422);
    }

    #[test]
    fn spending_insights_nets_a_refund_and_excludes_transfers_and_card_payments() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let card = make_account(&conn, AccountType::CreditCard);
        add_txn(
            &conn,
            checking,
            "2026-08-05",
            -8000,
            TransactionType::Purchase,
            Some("Shopping"),
        );
        add_txn(
            &conn,
            checking,
            "2026-08-10",
            3000,
            TransactionType::Refund,
            Some("Shopping"),
        );
        // A checking-to-card payment: a transfer leg on each account, never spending.
        add_txn(
            &conn,
            checking,
            "2026-08-15",
            -5000,
            TransactionType::Transfer,
            None,
        );
        add_txn(
            &conn,
            card,
            "2026-08-15",
            -5000,
            TransactionType::Transfer,
            None,
        );

        let insights = spending_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(
            insights.total_cents, 5000,
            "$80 spent minus $30 refunded, transfers excluded"
        );
    }

    #[test]
    fn spending_insights_separates_one_time_from_ordinary_spending() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_full(
            &conn,
            checking,
            "2026-08-05",
            -20000,
            TransactionType::Purchase,
            Some("Travel"),
            Some("Travel"),
            None,
            true,
        );
        add_txn(
            &conn,
            checking,
            "2026-08-10",
            -6000,
            TransactionType::Purchase,
            Some("Dining"),
        );

        let insights = spending_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(
            insights.total_cents, 26000,
            "one-time spending still counts in the official total"
        );
        assert_eq!(insights.one_time_cents, 20000);
    }

    #[test]
    fn spending_insights_splits_recurring_from_non_recurring_actual_spending() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let series_id = confirm_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Streaming Service",
            -999,
            "2026-08-05",
        );
        // Link a real transaction to the confirmed series, exactly as matching would.
        let txn_id = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -999,
            TransactionType::Purchase,
            Some("Subscriptions"),
        );
        conn.execute(
            "UPDATE transactions SET recurring_series_id = ?1 WHERE id = ?2",
            rusqlite::params![series_id, txn_id],
        )
        .unwrap();
        add_txn(
            &conn,
            checking,
            "2026-08-10",
            -4000,
            TransactionType::Purchase,
            Some("Dining"),
        );

        let insights = spending_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(insights.recurring_cents, 999);
        assert_eq!(insights.non_recurring_cents, 4000);
    }

    #[test]
    fn spending_insights_never_counts_expected_but_unposted_occurrences() {
        // Section 15: an occurrence with no linked transaction contributes
        // nothing to actual spending - spending_insights never reads
        // recurring_occurrences at all, only real transactions.
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        confirm_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Streaming Service",
            -999,
            "2026-08-05",
        );

        let insights = spending_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(insights.total_cents, 0);
        assert_eq!(insights.recurring_cents, 0);
    }

    #[test]
    fn spending_insights_archived_transactions_are_excluded() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let id = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -5000,
            TransactionType::Purchase,
            Some("Dining"),
        );
        transactions::set_archived(&conn, id, true).unwrap();

        let insights = spending_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(insights.total_cents, 0);
    }

    #[test]
    fn spending_insights_categories_reuse_reports_split_aware_breakdown() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let walmart = transactions::create(
            &conn,
            &TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -12000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Walmart".to_string(),
                merchant: Some("Walmart".to_string()),
                category: Some("Shopping".to_string()),
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
            walmart.id,
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

        let insights = spending_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(
            insights.total_cents, 12000,
            "never double-counted across the two splits"
        );
        assert!(insights
            .categories
            .iter()
            .any(|c| c.category == "Groceries" && c.spent_cents == 8000));
        assert!(insights
            .categories
            .iter()
            .any(|c| c.category == "Household" && c.spent_cents == 4000));
    }

    // --- Category detail / comparison (sections 12, 91) ---

    #[test]
    fn category_detail_reports_current_previous_and_typical_spending() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        for month_start in [
            "2026-02-10",
            "2026-03-10",
            "2026-04-10",
            "2026-05-10",
            "2026-06-10",
            "2026-07-10",
        ] {
            add_txn(
                &conn,
                checking,
                month_start,
                -24000,
                TransactionType::Purchase,
                Some("Dining"),
            );
        }
        add_txn(
            &conn,
            checking,
            "2026-08-05",
            -48800,
            TransactionType::Purchase,
            Some("Dining"),
        );

        let detail = category_detail(
            &conn,
            "Dining",
            "2026-08-01",
            "2026-08-31",
            "2026-07-01",
            "2026-07-31",
        )
        .unwrap();
        assert_eq!(detail.spent_cents, 48800);
        assert_eq!(detail.previous_spent_cents, 24000);
        assert_eq!(detail.typical_cents, Some(24000));
        assert_eq!(detail.transaction_count, 1);
        assert_eq!(detail.one_time_cents, 0);
    }

    #[test]
    fn category_detail_handles_zero_previous_spending_without_dividing_by_it() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(
            &conn,
            checking,
            "2026-08-05",
            -5000,
            TransactionType::Purchase,
            Some("Travel"),
        );

        let detail = category_detail(
            &conn,
            "Travel",
            "2026-08-01",
            "2026-08-31",
            "2026-07-01",
            "2026-07-31",
        )
        .unwrap();
        assert_eq!(detail.spent_cents, 5000);
        assert_eq!(detail.previous_spent_cents, 0);
        assert_eq!(
            detail.typical_cents, None,
            "one month of history is not a baseline"
        );
    }

    #[test]
    fn category_detail_counts_a_split_transaction_once_not_once_per_split() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let walmart = transactions::create(
            &conn,
            &TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -12000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Walmart".to_string(),
                merchant: Some("Walmart".to_string()),
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
            walmart.id,
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

        let detail = category_detail(
            &conn,
            "Groceries",
            "2026-08-01",
            "2026-08-31",
            "2026-07-01",
            "2026-07-31",
        )
        .unwrap();
        assert_eq!(detail.spent_cents, 8000);
        assert_eq!(
            detail.transaction_count, 1,
            "one real transaction, even though it touches two categories"
        );
    }

    // --- Income (section 92) ---

    #[test]
    fn income_insights_totals_salary_and_excludes_transfers_and_refunds() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let savings = make_account(&conn, AccountType::Savings);
        add_full(
            &conn,
            checking,
            "2026-08-01",
            250000,
            TransactionType::Income,
            Some("Employer"),
            None,
            None,
            false,
        );
        add_txn(
            &conn,
            checking,
            "2026-08-10",
            -5000,
            TransactionType::Purchase,
            Some("Shopping"),
        );
        add_txn(
            &conn,
            checking,
            "2026-08-10",
            2000,
            TransactionType::Refund,
            Some("Shopping"),
        );
        add_txn(
            &conn,
            checking,
            "2026-08-15",
            -30000,
            TransactionType::Transfer,
            None,
        );
        add_txn(
            &conn,
            savings,
            "2026-08-15",
            30000,
            TransactionType::Transfer,
            None,
        );

        let insights = income_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(
            insights.total_cents, 250000,
            "refund and transfer must never be treated as income"
        );
        assert_eq!(insights.sources.len(), 1);
        assert_eq!(insights.sources[0].name, "Employer");
    }

    #[test]
    fn income_insights_splits_recurring_paycheck_from_one_time_income() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let series_id = confirm_series(
            &mut conn,
            checking,
            Direction::Income,
            "Employer",
            250000,
            "2026-08-01",
        );
        let txn_id = add_full(
            &conn,
            checking,
            "2026-08-01",
            250000,
            TransactionType::Income,
            Some("Employer"),
            None,
            None,
            false,
        );
        conn.execute(
            "UPDATE transactions SET recurring_series_id = ?1 WHERE id = ?2",
            rusqlite::params![series_id, txn_id],
        )
        .unwrap();
        add_full(
            &conn,
            checking,
            "2026-08-15",
            40000,
            TransactionType::Income,
            Some("Freelance Client"),
            None,
            None,
            true,
        );

        let insights = income_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(insights.recurring_cents, 250000);
        assert_eq!(insights.non_recurring_cents, 40000);
        assert_eq!(insights.total_cents, 290000);
    }

    // --- Merchants (section 93) ---

    #[test]
    fn top_merchants_aggregates_by_canonical_payee_across_transactions() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let amazon = make_payee(&conn, "Amazon");
        add_full(
            &conn,
            checking,
            "2026-08-05",
            -2000,
            TransactionType::Purchase,
            Some("AMZN Mktp"),
            Some("Household"),
            Some(amazon),
            false,
        );
        add_full(
            &conn,
            checking,
            "2026-08-10",
            -3000,
            TransactionType::Purchase,
            Some("Amazon.com"),
            Some("Electronics"),
            Some(amazon),
            false,
        );

        let merchants = top_merchants(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(merchants.len(), 1);
        assert_eq!(merchants[0].name, "Amazon");
        assert_eq!(merchants[0].amount_cents, 5000);
        assert_eq!(merchants[0].transaction_count, 2);
    }

    #[test]
    fn top_merchants_falls_back_to_raw_merchant_text_when_unnormalized() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_full(
            &conn,
            checking,
            "2026-08-05",
            -1500,
            TransactionType::Purchase,
            Some("Publix"),
            Some("Groceries"),
            None,
            false,
        );
        add_full(
            &conn,
            checking,
            "2026-08-12",
            -1800,
            TransactionType::Purchase,
            Some("Publix"),
            Some("Groceries"),
            None,
            false,
        );

        let merchants = top_merchants(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(
            merchants.len(),
            1,
            "same raw merchant text must still combine, not disappear"
        );
        assert_eq!(merchants[0].name, "Publix");
        assert_eq!(merchants[0].amount_cents, 3300);
    }

    #[test]
    fn top_merchants_reflects_a_payee_merge_under_the_target_identity() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let old = make_payee(&conn, "Amazon Marketplace");
        let canonical = make_payee(&conn, "Amazon");
        add_full(
            &conn,
            checking,
            "2026-08-05",
            -2000,
            TransactionType::Purchase,
            Some("AMZN"),
            None,
            Some(old),
            false,
        );
        add_full(
            &conn,
            checking,
            "2026-08-10",
            -3000,
            TransactionType::Purchase,
            Some("Amazon.com"),
            None,
            Some(canonical),
            false,
        );

        payees::merge(&conn, old, canonical).unwrap();

        let merchants = top_merchants(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(
            merchants.len(),
            1,
            "must not double-list the archived merged payee separately"
        );
        assert_eq!(merchants[0].name, "Amazon");
        assert_eq!(merchants[0].amount_cents, 5000);
    }

    #[test]
    fn top_merchants_never_multiplies_a_split_transactions_own_amount() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let walmart_payee = make_payee(&conn, "Walmart");
        let walmart = transactions::create(
            &conn,
            &TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -12000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Walmart".to_string(),
                merchant: Some("Walmart".to_string()),
                category: Some("Shopping".to_string()),
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
                payee_id: Some(walmart_payee),
                is_one_time: false,
            },
        )
        .unwrap();
        crate::splits::replace_for_transaction(
            &mut conn,
            walmart.id,
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

        let merchants = top_merchants(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(merchants.len(), 1);
        assert_eq!(
            merchants[0].amount_cents, 12000,
            "one $120 transaction, never $80 + $40 on top of it"
        );
        assert_eq!(merchants[0].transaction_count, 1);
    }

    #[test]
    fn merchant_detail_shows_the_full_category_mix() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let amazon = make_payee(&conn, "Amazon");
        add_full(
            &conn,
            checking,
            "2026-08-05",
            -2000,
            TransactionType::Purchase,
            Some("Amazon"),
            Some("Household"),
            Some(amazon),
            false,
        );
        add_full(
            &conn,
            checking,
            "2026-08-10",
            -3000,
            TransactionType::Purchase,
            Some("Amazon"),
            Some("Electronics"),
            Some(amazon),
            false,
        );
        add_full(
            &conn,
            checking,
            "2026-07-01",
            -1000,
            TransactionType::Purchase,
            Some("Amazon"),
            Some("Books"),
            Some(amazon),
            false,
        );

        let detail = merchant_detail(
            &conn,
            Some(amazon),
            None,
            "2026-08-01",
            "2026-08-31",
            "2026-07-01",
            "2026-07-31",
        )
        .unwrap();
        assert_eq!(detail.spent_cents, 5000);
        assert_eq!(detail.previous_spent_cents, 1000);
        assert_eq!(detail.transaction_count, 2);
        assert_eq!(detail.average_cents, 2500);
        assert_eq!(detail.categories.len(), 2);
        assert!(detail
            .categories
            .iter()
            .any(|c| c.category == "Household" && c.amount_cents == 2000));
        assert!(detail
            .categories
            .iter()
            .any(|c| c.category == "Electronics" && c.amount_cents == 3000));
    }

    // --- Cash Flow (section 94) ---

    #[test]
    fn cash_flow_insights_computes_income_spending_and_net() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(
            &conn,
            checking,
            "2026-08-01",
            500000,
            TransactionType::Income,
            None,
        );
        add_txn(
            &conn,
            checking,
            "2026-08-10",
            -320000,
            TransactionType::Purchase,
            Some("Rent"),
        );

        let flow = cash_flow_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(flow.income_cents, 500000);
        assert_eq!(flow.spending_cents, 320000);
        assert_eq!(flow.net_cents, 180000);
    }

    #[test]
    fn cash_flow_insights_excludes_internal_transfers_and_card_payments() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let card = make_account(&conn, AccountType::CreditCard);
        add_txn(
            &conn,
            checking,
            "2026-08-01",
            500000,
            TransactionType::Income,
            None,
        );
        add_txn(
            &conn,
            card,
            "2026-08-05",
            8000,
            TransactionType::Purchase,
            Some("Dining"),
        );
        add_txn(
            &conn,
            checking,
            "2026-08-15",
            -8000,
            TransactionType::Transfer,
            None,
        );
        add_txn(
            &conn,
            card,
            "2026-08-15",
            -8000,
            TransactionType::Transfer,
            None,
        );

        let flow = cash_flow_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(flow.income_cents, 500000);
        assert_eq!(
            flow.spending_cents, 8000,
            "only the original card charge, never the payment transfer"
        );
        assert_eq!(flow.net_cents, 492000);
    }

    #[test]
    fn cash_flow_insights_can_be_negative() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(
            &conn,
            checking,
            "2026-08-01",
            50000,
            TransactionType::Income,
            None,
        );
        add_txn(
            &conn,
            checking,
            "2026-08-10",
            -90000,
            TransactionType::Purchase,
            Some("Rent"),
        );

        let flow = cash_flow_insights(&conn, "2026-08-01", "2026-08-31").unwrap();
        assert_eq!(flow.net_cents, -40000);
    }
}
