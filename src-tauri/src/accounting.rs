//! Accounting-boundary hardening (Phase 4A): the shared rules every later
//! financial calculation (budgets, Safe to Spend, net worth) is built on top
//! of, rather than each reimplementing its own notion of "spending" or
//! "balance." See ADR-0007 for the balance/opening-balance design and
//! ADR-0009 for the opening-balance/balance-adjustment split.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::accounts::{self, Account, AccountError};
use crate::dates;
use crate::transactions::{Transaction, TransactionType};

const OPENING_BALANCE_LABEL: &str = "Opening balance";
const ADJUSTMENT_LABEL: &str = "Balance adjustment";

/// How much a single transaction contributes to a "total spending" figure.
/// Purchases add their magnitude; refunds subtract theirs (a refund is
/// money back, reducing net spending, regardless of which specific purchase
/// it corresponds to - MIDAS does not auto-link refunds to purchases, per
/// Phase 2's explicit non-goal). Income, transfers, and the opening-balance/
/// balance-adjustment anchors never count as spending - this is what keeps
/// a credit-card payment from ever becoming a second expense on top of the
/// purchases it pays off, and what keeps a reconciliation correction out of
/// every budget and spending total (ADR-0009).
pub fn spending_effect(transaction_type: TransactionType, amount_cents: i64) -> i64 {
    match transaction_type {
        TransactionType::Purchase => amount_cents.abs(),
        TransactionType::Refund => -amount_cents.abs(),
        TransactionType::Income
        | TransactionType::Transfer
        | TransactionType::OpeningBalance
        | TransactionType::BalanceAdjustment => 0,
    }
}

pub fn total_spending(transactions: &[Transaction]) -> i64 {
    transactions
        .iter()
        .map(|t| spending_effect(t.transaction_type, t.amount_cents))
        .sum()
}

fn row_to_spending_effect(row: &rusqlite::Row) -> rusqlite::Result<i64> {
    let amount_cents: i64 = row.get(0)?;
    let transaction_type = TransactionType::from_sql_str(&row.get::<_, String>(1)?)?;
    Ok(spending_effect(transaction_type, amount_cents))
}

/// Sums `spending_effect` over every transaction dated within `[from, to]`
/// (inclusive), optionally scoped to one category - a query directly
/// against `transactions` (not `transactions::list`) so it stays entirely
/// in `rusqlite::Result` and composes with `?` inside a caller's own
/// `query_map` closure if needed. The one shared place budgets, pay cycles,
/// and Safe to Spend all read "spending in a date range" from, instead of
/// each hand-rolling the same query.
pub fn spending_in_range(
    conn: &Connection,
    category: Option<&str>,
    from: &str,
    to: &str,
) -> rusqlite::Result<i64> {
    let amounts: Vec<i64> = if let Some(cat) = category {
        let mut stmt = conn.prepare(
            "SELECT amount_cents, transaction_type FROM transactions
             WHERE category = ?1 AND transaction_date >= ?2 AND transaction_date <= ?3 AND is_archived = 0",
        )?;
        let rows = stmt
            .query_map(params![cat, from, to], row_to_spending_effect)?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        rows
    } else {
        let mut stmt = conn.prepare(
            "SELECT amount_cents, transaction_type FROM transactions
             WHERE transaction_date >= ?1 AND transaction_date <= ?2 AND is_archived = 0",
        )?;
        let rows = stmt
            .query_map(params![from, to], row_to_spending_effect)?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        rows
    };
    Ok(amounts.iter().sum())
}

/// Total income in a date range - the mirror of `spending_in_range` for the
/// Overview/Reports "income vs spending" comparison, which `spending_effect`
/// deliberately excludes income from (income isn't spending). Income is
/// already stored as a positive `amount_cents` per ADR-0004's sign
/// convention, so this is a plain sum, no per-row classification needed.
pub fn income_in_range(conn: &Connection, from: &str, to: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(SUM(amount_cents), 0) FROM transactions
         WHERE transaction_type = 'income' AND transaction_date >= ?1 AND transaction_date <= ?2 AND is_archived = 0",
        params![from, to],
        |row| row.get(0),
    )
}

/// Net worth: included assets minus included liabilities - never a blind
/// sum of every account balance regardless of type. ADR-0004's sign
/// convention already makes a liability's balance "positive = owe more," so
/// it is subtracted rather than added. Archived accounts and accounts with
/// `include_in_net_worth = false` are excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NetWorthBreakdown {
    pub total_assets_cents: i64,
    pub total_liabilities_cents: i64,
    pub net_worth_cents: i64,
}

/// Included assets minus included liabilities, with each side of the split
/// exposed separately - never just the net total, so the UI can show a
/// useful current snapshot rather than one opaque number.
pub fn net_worth_breakdown(accounts: &[Account]) -> NetWorthBreakdown {
    let (mut total_assets_cents, mut total_liabilities_cents) = (0i64, 0i64);
    for a in accounts
        .iter()
        .filter(|a| a.include_in_net_worth && !a.is_archived)
    {
        if a.account_type.is_liability() {
            total_liabilities_cents += a.balance_cents;
        } else {
            total_assets_cents += a.balance_cents;
        }
    }
    NetWorthBreakdown {
        total_assets_cents,
        total_liabilities_cents,
        net_worth_cents: total_assets_cents - total_liabilities_cents,
    }
}

/// Net worth as of a past date: each included, non-archived account's
/// balance is recomputed from only the transactions dated on or before
/// `as_of_date` - the same "balance = sum of transactions" principle
/// `accounts::get` uses for the current balance, just anchored to a past
/// point. An account with no transactions yet at that date contributes 0,
/// which is correct (nothing to backfill or guess).
fn net_worth_as_of(conn: &Connection, as_of_date: &str) -> rusqlite::Result<i64> {
    let mut stmt = conn.prepare(
        "SELECT a.account_type, COALESCE(SUM(t.amount_cents), 0) FROM accounts a
         LEFT JOIN transactions t
             ON t.account_id = a.id AND t.is_archived = 0 AND t.transaction_date <= ?1
         WHERE a.is_archived = 0 AND a.include_in_net_worth = 1
         GROUP BY a.id",
    )?;
    let rows = stmt.query_map(params![as_of_date], |row| {
        let account_type = crate::accounts::AccountType::from_sql_str(&row.get::<_, String>(0)?)?;
        let balance: i64 = row.get(1)?;
        Ok(if account_type.is_liability() {
            -balance
        } else {
            balance
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<i64>>>()
        .map(|v| v.iter().sum())
}

#[derive(Debug, Clone, Serialize)]
pub struct NetWorthPoint {
    pub as_of_date: String,
    pub net_worth_cents: i64,
}

const NET_WORTH_HISTORY_MONTHS: i64 = 6;

/// Net worth at the end of each of the last `NET_WORTH_HISTORY_MONTHS`
/// months, plus today as the final, current point - a simple trend with no
/// separate snapshot table to keep in sync, since it's recomputed from the
/// ledger every time it's asked for.
pub fn net_worth_history(conn: &Connection) -> rusqlite::Result<Vec<NetWorthPoint>> {
    let today: String = conn.query_row("SELECT date('now')", [], |row| row.get(0))?;
    let mut points = Vec::new();
    for i in (1..=NET_WORTH_HISTORY_MONTHS).rev() {
        let as_of_date = dates::end_of_month(&dates::add_months(&today, -i));
        points.push(NetWorthPoint {
            net_worth_cents: net_worth_as_of(conn, &as_of_date)?,
            as_of_date,
        });
    }
    points.push(NetWorthPoint {
        net_worth_cents: net_worth_as_of(conn, &today)?,
        as_of_date: today,
    });
    Ok(points)
}

/// Establishes or corrects an account's balance (ADR-0009). The *first*
/// time this is called for an account, it establishes the opening balance -
/// anchored before the account's earliest other transaction (or
/// `as_of_date` if there are none yet) and never touched again. Every
/// *subsequent* call is a dated adjustment landing on `as_of_date` itself,
/// leaving the opening balance and every earlier adjustment untouched - so
/// reconciling today never changes what MIDAS reported yesterday.
pub fn reconcile_balance(
    conn: &Connection,
    account_id: i64,
    known_current_balance_cents: i64,
    as_of_date: &str,
) -> Result<Account, AccountError> {
    let opening_balance_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM transactions WHERE account_id = ?1 AND transaction_type = 'opening_balance')",
        params![account_id],
        |row| row.get(0),
    )?;

    if opening_balance_exists {
        upsert_balance_adjustment(conn, account_id, known_current_balance_cents, as_of_date)?;
    } else {
        establish_opening_balance(conn, account_id, known_current_balance_cents, as_of_date)?;
    }

    accounts::get(conn, account_id)?.ok_or(AccountError::NotFound(account_id))
}

fn establish_opening_balance(
    conn: &Connection,
    account_id: i64,
    known_current_balance_cents: i64,
    as_of_date: &str,
) -> Result<(), AccountError> {
    let (sum_so_far, earliest_date): (i64, Option<String>) = conn.query_row(
        "SELECT COALESCE(SUM(amount_cents), 0), MIN(transaction_date) FROM transactions
         WHERE account_id = ?1 AND is_archived = 0",
        params![account_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let delta = known_current_balance_cents - sum_so_far;
    if delta == 0 {
        return Ok(());
    }

    let anchor_date = match earliest_date {
        Some(d) => dates::add_days(&d, -1),
        None => as_of_date.to_string(),
    };
    conn.execute(
        "INSERT INTO transactions (
            account_id, transaction_date, amount_cents, transaction_type,
            raw_description, merchant
         ) VALUES (?1, ?2, ?3, 'opening_balance', ?4, ?4)",
        params![account_id, anchor_date, delta, OPENING_BALANCE_LABEL],
    )?;
    Ok(())
}

fn upsert_balance_adjustment(
    conn: &Connection,
    account_id: i64,
    known_current_balance_cents: i64,
    as_of_date: &str,
) -> Result<(), AccountError> {
    let existing: Option<(i64, i64)> = conn
        .query_row(
            "SELECT id, amount_cents FROM transactions
             WHERE account_id = ?1 AND transaction_type = 'balance_adjustment' AND transaction_date = ?2",
            params![account_id, as_of_date],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let current_total: i64 = conn.query_row(
        "SELECT COALESCE(SUM(amount_cents), 0) FROM transactions WHERE account_id = ?1 AND is_archived = 0",
        params![account_id],
        |row| row.get(0),
    )?;
    // Exclude this same-date adjustment's own prior amount from the
    // baseline, so re-reconciling on a date that already has one doesn't
    // count it twice against itself.
    let baseline = current_total - existing.map(|(_, amount)| amount).unwrap_or(0);
    let delta = known_current_balance_cents - baseline;

    match (existing, delta) {
        (Some((id, _)), 0) => {
            // Removing an already-reconciled adjustment is exactly the
            // "balance adjustment changes" case product handoff section 16
            // names as invalidating.
            crate::reconciliation::invalidate_if_linked(conn, id)?;
            conn.execute("DELETE FROM transactions WHERE id = ?1", params![id])?;
        }
        (Some((id, _)), _) => {
            crate::reconciliation::invalidate_if_linked(conn, id)?;
            conn.execute(
                "UPDATE transactions SET amount_cents = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?2",
                params![delta, id],
            )?;
        }
        (None, 0) => {}
        (None, _) => {
            // A brand-new adjustment has no reconciliation_id of its own
            // yet, but if it lands on a date an already-completed
            // reconciliation covers, it still changes that reconciliation's
            // cleared-balance sum (balance adjustments always count -
            // reconciliation::cleared_balance_as_of).
            crate::reconciliation::invalidate_completed_covering(conn, account_id, as_of_date)?;
            conn.execute(
                "INSERT INTO transactions (
                    account_id, transaction_date, amount_cents, transaction_type,
                    raw_description, merchant
                 ) VALUES (?1, ?2, ?3, 'balance_adjustment', ?4, ?4)",
                params![account_id, as_of_date, delta, ADJUSTMENT_LABEL],
            )?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn get_net_worth(state: tauri::State<crate::db::DbState>) -> Result<NetWorthBreakdown, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let accounts = accounts::list(&conn, false).map_err(|e| e.to_string())?;
    Ok(net_worth_breakdown(&accounts))
}

#[tauri::command]
pub fn get_net_worth_history(
    state: tauri::State<crate::db::DbState>,
) -> Result<Vec<NetWorthPoint>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    net_worth_history(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_total_spending(
    state: tauri::State<crate::db::DbState>,
    filter: crate::transactions::TransactionFilter,
) -> Result<i64, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let txns = crate::transactions::list(&conn, &filter).map_err(|e| e.to_string())?;
    Ok(total_spending(&txns))
}

#[tauri::command]
pub fn reconcile_account_balance(
    state: tauri::State<crate::db::DbState>,
    account_id: i64,
    known_current_balance_cents: i64,
    as_of_date: String,
) -> Result<Account, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    reconcile_balance(&conn, account_id, known_current_balance_cents, &as_of_date)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{AccountInput, AccountType};
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
    ) {
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
        .unwrap();
    }

    fn balance_of(conn: &Connection, account_id: i64) -> i64 {
        accounts::get(conn, account_id)
            .unwrap()
            .unwrap()
            .balance_cents
    }

    // --- Required case: checking purchase ---
    #[test]
    fn checking_purchase_reduces_balance_and_counts_as_spending() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(
            &conn,
            checking,
            "2026-03-01",
            -5000,
            TransactionType::Purchase,
        );

        assert_eq!(balance_of(&conn, checking), -5000);
        let txns = transactions::list(&conn, &Default::default()).unwrap();
        assert_eq!(total_spending(&txns), 5000);
    }

    // --- Required case: credit-card purchase ---
    #[test]
    fn credit_card_purchase_increases_amount_owed_and_counts_as_spending() {
        let conn = test_conn();
        let card = make_account(&conn, AccountType::CreditCard);
        add_txn(&conn, card, "2026-03-01", 5000, TransactionType::Purchase);

        assert_eq!(balance_of(&conn, card), 5000); // owes $50 more
        let txns = transactions::list(&conn, &Default::default()).unwrap();
        assert_eq!(total_spending(&txns), 5000);
    }

    // --- Required case: credit-card payment ---
    #[test]
    fn credit_card_payment_moves_cash_to_liability_without_double_counting_spending() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let card = make_account(&conn, AccountType::CreditCard);
        add_txn(&conn, card, "2026-03-01", 5000, TransactionType::Purchase); // the $50 charge

        add_txn(
            &conn,
            checking,
            "2026-03-15",
            -5000,
            TransactionType::Transfer,
        ); // payment leaves checking
        add_txn(&conn, card, "2026-03-15", -5000, TransactionType::Transfer); // payment reduces what's owed

        assert_eq!(
            balance_of(&conn, checking),
            -5000,
            "checking cash down by $50"
        );
        assert_eq!(
            balance_of(&conn, card),
            0,
            "liability paid off, owes nothing"
        );

        let txns = transactions::list(&conn, &Default::default()).unwrap();
        assert_eq!(
            total_spending(&txns),
            5000,
            "only the original $50 charge counts as spending - the payment is a transfer, not a second expense"
        );
    }

    #[test]
    fn transfers_never_count_as_spending() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let savings = make_account(&conn, AccountType::Savings);
        add_txn(
            &conn,
            checking,
            "2026-03-01",
            -20000,
            TransactionType::Transfer,
        );
        add_txn(
            &conn,
            savings,
            "2026-03-01",
            20000,
            TransactionType::Transfer,
        );

        let txns = transactions::list(&conn, &Default::default()).unwrap();
        assert_eq!(total_spending(&txns), 0);
    }

    #[test]
    fn refund_reduces_net_spending() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(
            &conn,
            checking,
            "2026-03-01",
            -8000,
            TransactionType::Purchase,
        );
        add_txn(&conn, checking, "2026-03-10", 3000, TransactionType::Refund);

        let txns = transactions::list(&conn, &Default::default()).unwrap();
        assert_eq!(
            total_spending(&txns),
            5000,
            "$80 spent minus $30 refunded = $50 net"
        );
        assert_eq!(balance_of(&conn, checking), -5000);
    }

    // --- Net worth: all account types + inclusion toggle ---
    #[test]
    fn net_worth_sums_included_assets_minus_included_liabilities() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let savings = make_account(&conn, AccountType::Savings);
        let cash = make_account(&conn, AccountType::Cash);
        let property = make_account(&conn, AccountType::Property);
        let card = make_account(&conn, AccountType::CreditCard);
        let loan = make_account(&conn, AccountType::Loan);
        let other_liability = make_account(&conn, AccountType::OtherLiability);

        add_txn(
            &conn,
            checking,
            "2026-01-01",
            100_000,
            TransactionType::Income,
        );
        add_txn(
            &conn,
            savings,
            "2026-01-01",
            500_000,
            TransactionType::Income,
        );
        add_txn(&conn, cash, "2026-01-01", 20_000, TransactionType::Income);
        add_txn(
            &conn,
            property,
            "2026-01-01",
            3_000_000,
            TransactionType::Income,
        );
        add_txn(&conn, card, "2026-01-01", 40_000, TransactionType::Purchase);
        add_txn(
            &conn,
            loan,
            "2026-01-01",
            1_000_000,
            TransactionType::Purchase,
        );
        add_txn(
            &conn,
            other_liability,
            "2026-01-01",
            10_000,
            TransactionType::Purchase,
        );

        let accounts = accounts::list(&conn, false).unwrap();
        let breakdown = net_worth_breakdown(&accounts);
        assert_eq!(breakdown.total_assets_cents, 3_620_000);
        assert_eq!(breakdown.total_liabilities_cents, 1_050_000);
        assert_eq!(breakdown.net_worth_cents, 3_620_000 - 1_050_000);
    }

    #[test]
    fn net_worth_excludes_accounts_with_inclusion_toggled_off_and_archived_accounts() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(
            &conn,
            checking,
            "2026-01-01",
            100_000,
            TransactionType::Income,
        );

        let excluded = accounts::create(
            &conn,
            &AccountInput {
                name: "Excluded Investment".to_string(),
                institution: None,
                account_type: AccountType::Investment,
                include_in_net_worth: false,
                notes: None,
            },
        )
        .unwrap();
        add_txn(
            &conn,
            excluded.id,
            "2026-01-01",
            9_999_999,
            TransactionType::Income,
        );

        let archived = make_account(&conn, AccountType::Savings);
        add_txn(
            &conn,
            archived,
            "2026-01-01",
            500_000,
            TransactionType::Income,
        );
        accounts::set_archived(&conn, archived, true).unwrap();

        let accounts = accounts::list(&conn, true).unwrap();
        assert_eq!(net_worth_breakdown(&accounts).net_worth_cents, 100_000);
    }

    // --- Opening / baseline balances ---
    #[test]
    fn reconcile_establishes_an_opening_balance_when_none_existed() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        assert_eq!(balance_of(&conn, checking), 0);

        let account = reconcile_balance(&conn, checking, 120_000, "2026-08-25").unwrap();
        assert_eq!(account.balance_cents, 120_000);

        let txns = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                account_id: Some(checking),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(txns.len(), 1);
        assert_eq!(txns[0].transaction_type, TransactionType::OpeningBalance);
        assert_eq!(txns[0].amount_cents, 120_000);
        assert_eq!(txns[0].transaction_date, "2026-08-25");
    }

    // --- Partial-history import + known current balance ---
    #[test]
    fn reconcile_anchors_before_partial_imported_history_without_double_counting() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        // Only 3 months imported - net change over that window, not the full
        // account history.
        add_txn(
            &conn,
            checking,
            "2026-06-01",
            -50_000,
            TransactionType::Purchase,
        );
        add_txn(
            &conn,
            checking,
            "2026-07-01",
            200_000,
            TransactionType::Income,
        );
        add_txn(
            &conn,
            checking,
            "2026-08-01",
            -30_000,
            TransactionType::Purchase,
        );
        // sum of imported = -50_000 + 200_000 - 30_000 = 120_000

        // The owner's bank actually shows $5,000.00 today.
        let account = reconcile_balance(&conn, checking, 500_000, "2026-08-25").unwrap();
        assert_eq!(
            account.balance_cents, 500_000,
            "reconciled balance must equal the real bank balance, not just the imported window"
        );

        let txns = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                account_id: Some(checking),
                ..Default::default()
            },
        )
        .unwrap();
        let anchor = txns
            .iter()
            .find(|t| t.transaction_type == TransactionType::OpeningBalance)
            .unwrap();
        assert_eq!(anchor.amount_cents, 500_000 - 120_000);
        assert!(
            anchor.transaction_date.as_str() < "2026-06-01",
            "anchor must sort before every imported transaction, not among them"
        );
    }

    #[test]
    fn first_reconcile_establishes_opening_balance_and_is_never_touched_again() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        reconcile_balance(&conn, checking, 100_000, "2026-08-25").unwrap();
        // A second call on the SAME date now establishes an opening balance
        // already existing, so it must go through the adjustment path
        // instead of rewriting the opening balance in place (ADR-0009).
        reconcile_balance(&conn, checking, 150_000, "2026-08-25").unwrap();

        let txns = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                account_id: Some(checking),
                include_archived: true,
                ..Default::default()
            },
        )
        .unwrap();
        let opening: Vec<_> = txns
            .iter()
            .filter(|t| t.transaction_type == TransactionType::OpeningBalance)
            .collect();
        assert_eq!(opening.len(), 1);
        assert_eq!(
            opening[0].amount_cents, 100_000,
            "the opening balance's own amount must be exactly what the first call set - untouched by the second"
        );
        assert_eq!(balance_of(&conn, checking), 150_000);
    }

    #[test]
    fn repeated_adjustments_on_the_same_date_upsert_in_place_rather_than_accumulating() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        reconcile_balance(&conn, checking, 100_000, "2026-01-01").unwrap(); // opening balance
        reconcile_balance(&conn, checking, 150_000, "2026-08-25").unwrap(); // first adjustment
        reconcile_balance(&conn, checking, 175_000, "2026-08-25").unwrap(); // corrects the same adjustment

        let txns = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                account_id: Some(checking),
                ..Default::default()
            },
        )
        .unwrap();
        let adjustments: Vec<_> = txns
            .iter()
            .filter(|t| t.transaction_type == TransactionType::BalanceAdjustment)
            .collect();
        assert_eq!(
            adjustments.len(),
            1,
            "must upsert the same date's adjustment, never accumulate a second one"
        );
        assert_eq!(adjustments[0].amount_cents, 75_000);
        assert_eq!(balance_of(&conn, checking), 175_000);
    }

    #[test]
    fn second_reconcile_on_a_new_date_creates_no_adjustment_when_ledger_already_matches() {
        // ADR-0009: unlike the pre-correction design, a second reconcile
        // call never deletes or touches the opening balance - it only ever
        // creates/updates/removes an adjustment dated at its own as_of_date.
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        reconcile_balance(&conn, checking, 50_000, "2026-08-25").unwrap();
        add_txn(
            &conn,
            checking,
            "2026-08-20",
            50_000,
            TransactionType::Income,
        );

        reconcile_balance(&conn, checking, 100_000, "2026-09-01").unwrap();

        let txns = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                account_id: Some(checking),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            txns.iter()
                .any(|t| t.transaction_type == TransactionType::OpeningBalance),
            "the opening balance must still exist, untouched"
        );
        assert!(
            txns.iter()
                .all(|t| t.transaction_type != TransactionType::BalanceAdjustment),
            "the ledger already matched the target, so no adjustment should have been created"
        );
        assert_eq!(balance_of(&conn, checking), 100_000);
    }

    #[test]
    fn re_reconciling_the_same_date_removes_the_no_longer_needed_adjustment() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        reconcile_balance(&conn, checking, 50_000, "2026-01-01").unwrap();

        // A later reconciliation on 2026-08-25 that doesn't match the
        // ledger creates a real adjustment dated then.
        reconcile_balance(&conn, checking, 80_000, "2026-08-25").unwrap();
        let after_first = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                account_id: Some(checking),
                ..Default::default()
            },
        )
        .unwrap();
        let adjustments: Vec<_> = after_first
            .iter()
            .filter(|t| t.transaction_type == TransactionType::BalanceAdjustment)
            .collect();
        assert_eq!(adjustments.len(), 1);
        assert_eq!(adjustments[0].transaction_date, "2026-08-25");

        // Re-reconciling on that same date back to the original ledger
        // total removes that date's now-unnecessary adjustment.
        reconcile_balance(&conn, checking, 50_000, "2026-08-25").unwrap();
        let after_second = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                account_id: Some(checking),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(after_second
            .iter()
            .all(|t| t.transaction_type != TransactionType::BalanceAdjustment));
        assert!(
            after_second
                .iter()
                .any(|t| t.transaction_type == TransactionType::OpeningBalance),
            "the opening balance itself must never be touched by this"
        );
        assert_eq!(balance_of(&conn, checking), 50_000);
    }

    #[test]
    fn opening_balance_never_counts_as_spending() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        reconcile_balance(&conn, checking, 100_000, "2026-08-25").unwrap();

        let txns = transactions::list(&conn, &Default::default()).unwrap();
        assert_eq!(total_spending(&txns), 0);
    }

    #[test]
    fn at_most_one_opening_balance_transaction_per_account_is_enforced_by_schema() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        conn.execute(
            "INSERT INTO transactions (account_id, transaction_date, amount_cents, transaction_type, raw_description, merchant)
             VALUES (?1, '2026-01-01', 100, 'opening_balance', 'Opening balance', 'Opening balance')",
            params![checking],
        )
        .unwrap();
        let second = conn.execute(
            "INSERT INTO transactions (account_id, transaction_date, amount_cents, transaction_type, raw_description, merchant)
             VALUES (?1, '2026-01-02', 200, 'opening_balance', 'Opening balance', 'Opening balance')",
            params![checking],
        );
        assert!(
            second.is_err(),
            "the partial unique index must reject a second anchor"
        );
    }

    #[test]
    fn works_symmetrically_for_a_liability_account() {
        let conn = test_conn();
        let card = make_account(&conn, AccountType::CreditCard);
        add_txn(&conn, card, "2026-08-01", 10_000, TransactionType::Purchase);
        // Statement shows $250.00 owed today, more than the $100 MIDAS knows about.
        let account = reconcile_balance(&conn, card, 25_000, "2026-08-25").unwrap();
        assert_eq!(account.balance_cents, 25_000);
    }

    #[test]
    fn spending_in_range_respects_the_date_bounds_and_optional_category() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(
            &conn,
            checking,
            "2026-08-05",
            -5000,
            TransactionType::Purchase,
        );
        add_txn(
            &conn,
            checking,
            "2026-08-10",
            -3000,
            TransactionType::Transfer,
        );
        add_txn(
            &conn,
            checking,
            "2026-09-01",
            -9000,
            TransactionType::Purchase,
        ); // out of range

        assert_eq!(
            spending_in_range(&conn, None, "2026-08-01", "2026-08-31").unwrap(),
            5000,
            "the transfer must not count, and the September purchase is out of range"
        );
    }

    #[test]
    fn income_in_range_sums_only_income_within_the_date_bounds() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(
            &conn,
            checking,
            "2026-08-01",
            200_000,
            TransactionType::Income,
        );
        add_txn(
            &conn,
            checking,
            "2026-08-15",
            -5000,
            TransactionType::Purchase,
        );
        add_txn(
            &conn,
            checking,
            "2026-09-01",
            200_000,
            TransactionType::Income,
        ); // out of range

        assert_eq!(
            income_in_range(&conn, "2026-08-01", "2026-08-31").unwrap(),
            200_000,
            "the purchase must not count, and September's income is out of range"
        );
    }

    #[test]
    fn spending_in_range_category_filter_matches_budgets_own_scoping() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        crate::transactions::create(
            &conn,
            &crate::transactions::TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -4000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Test".to_string(),
                merchant: None,
                category: Some("Dining".to_string()),
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
                payee_id: None,
                is_one_time: false,
            },
        )
        .unwrap();
        add_txn(
            &conn,
            checking,
            "2026-08-06",
            -1000,
            TransactionType::Purchase,
        ); // category None

        assert_eq!(
            spending_in_range(&conn, Some("Dining"), "2026-08-01", "2026-08-31").unwrap(),
            4000
        );
        assert_eq!(
            spending_in_range(&conn, None, "2026-08-01", "2026-08-31").unwrap(),
            5000
        );
    }

    fn today_str(conn: &Connection) -> String {
        conn.query_row("SELECT date('now')", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn net_worth_as_of_ignores_transactions_after_the_cutoff() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today = today_str(&conn);
        add_txn(
            &conn,
            checking,
            &dates::add_days(&today, -10),
            100_000,
            TransactionType::Income,
        );
        add_txn(
            &conn,
            checking,
            &dates::add_days(&today, -1),
            50_000,
            TransactionType::Income,
        );
        add_txn(&conn, checking, &today, 25_000, TransactionType::Income); // after most cutoffs below

        assert_eq!(
            net_worth_as_of(&conn, &dates::add_days(&today, -10)).unwrap(),
            100_000
        );
        assert_eq!(
            net_worth_as_of(&conn, &dates::add_days(&today, -1)).unwrap(),
            150_000
        );
        assert_eq!(net_worth_as_of(&conn, &today).unwrap(), 175_000);
    }

    #[test]
    fn net_worth_as_of_subtracts_liabilities_the_same_as_current_net_worth() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let card = make_account(&conn, AccountType::CreditCard);
        let today = today_str(&conn);
        add_txn(&conn, checking, &today, 100_000, TransactionType::Income);
        add_txn(&conn, card, &today, 30_000, TransactionType::Purchase); // owes $300

        assert_eq!(net_worth_as_of(&conn, &today).unwrap(), 70_000);
    }

    #[test]
    fn net_worth_as_of_excludes_accounts_not_included_in_net_worth() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today = today_str(&conn);
        add_txn(&conn, checking, &today, 100_000, TransactionType::Income);

        let excluded = accounts::create(
            &conn,
            &AccountInput {
                name: "Excluded".to_string(),
                institution: None,
                account_type: AccountType::Investment,
                include_in_net_worth: false,
                notes: None,
            },
        )
        .unwrap();
        add_txn(
            &conn,
            excluded.id,
            &today,
            9_999_999,
            TransactionType::Income,
        );

        assert_eq!(net_worth_as_of(&conn, &today).unwrap(), 100_000);
    }

    #[test]
    fn net_worth_history_has_seven_points_ending_at_todays_current_net_worth() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today = today_str(&conn);
        add_txn(&conn, checking, &today, 50_000, TransactionType::Income);

        let history = net_worth_history(&conn).unwrap();
        assert_eq!(history.len(), 7, "6 months back plus today");
        assert_eq!(history.last().unwrap().as_of_date, today);
        assert_eq!(history.last().unwrap().net_worth_cents, 50_000);
        // Every earlier point predates the deposit, so net worth was 0 then.
        for point in &history[..6] {
            assert_eq!(point.net_worth_cents, 0);
        }
    }

    // --- Phase 4C regression tests (ADR-0009) ---

    #[test]
    fn january_history_is_unchanged_after_a_later_reconciliation_and_todays_balance_is_correct() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today = today_str(&conn);
        let old_date = dates::add_days(&today, -200);

        reconcile_balance(&conn, checking, 50_000, &old_date).unwrap();
        let historical_view_before = net_worth_as_of(&conn, &old_date).unwrap();
        assert_eq!(historical_view_before, 50_000);

        // A much later reconciliation with a different, corrected balance.
        reconcile_balance(&conn, checking, 80_000, &today).unwrap();

        let historical_view_after = net_worth_as_of(&conn, &old_date).unwrap();
        assert_eq!(
            historical_view_after, 50_000,
            "the old date's net worth must be exactly what it was before the later reconciliation ever happened"
        );
        assert_eq!(
            balance_of(&conn, checking),
            80_000,
            "today's balance must reflect the correction"
        );
    }

    #[test]
    fn net_worth_history_changes_only_from_the_adjustment_date_forward() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today = today_str(&conn);
        let opening_date = dates::add_days(&today, -200);
        let adjustment_date = dates::add_days(&today, -10);

        reconcile_balance(&conn, checking, 50_000, &opening_date).unwrap();
        let before_cutoff = dates::add_days(&adjustment_date, -1);
        let net_worth_before_cutoff_pre = net_worth_as_of(&conn, &before_cutoff).unwrap();
        let net_worth_on_adjustment_date_pre = net_worth_as_of(&conn, &adjustment_date).unwrap();
        assert_eq!(net_worth_before_cutoff_pre, 50_000);
        assert_eq!(net_worth_on_adjustment_date_pre, 50_000);

        reconcile_balance(&conn, checking, 90_000, &adjustment_date).unwrap();

        assert_eq!(
            net_worth_as_of(&conn, &before_cutoff).unwrap(),
            50_000,
            "the day before the adjustment must be unaffected"
        );
        assert_eq!(
            net_worth_as_of(&conn, &adjustment_date).unwrap(),
            90_000,
            "the adjustment's own effective date must include it"
        );
        assert_eq!(net_worth_as_of(&conn, &today).unwrap(), 90_000);
    }

    #[test]
    fn budgets_and_spending_ignore_balance_adjustments() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today = today_str(&conn);
        reconcile_balance(&conn, checking, 50_000, &dates::add_days(&today, -200)).unwrap();

        crate::transactions::create(
            &conn,
            &crate::transactions::TransactionInput {
                account_id: checking,
                transaction_date: today.clone(),
                posted_date: None,
                amount_cents: -2000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Lunch".to_string(),
                merchant: None,
                category: Some("Dining".to_string()),
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
                payee_id: None,
                is_one_time: false,
            },
        )
        .unwrap();

        let spending_before = spending_in_range(&conn, Some("Dining"), &today, &today).unwrap();
        assert_eq!(spending_before, 2000);

        // A large adjustment that would badly distort spending if it leaked in.
        reconcile_balance(&conn, checking, 500_000, &today).unwrap();

        let spending_after = spending_in_range(&conn, Some("Dining"), &today, &today).unwrap();
        assert_eq!(
            spending_after, 2000,
            "the balance adjustment must never be counted as spending"
        );
        let spending_all_categories = spending_in_range(&conn, None, &today, &today).unwrap();
        assert_eq!(spending_all_categories, 2000);
    }
}
