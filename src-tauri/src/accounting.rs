//! Accounting-boundary hardening (Phase 4A): the shared rules every later
//! financial calculation (budgets, Safe to Spend, net worth) is built on top
//! of, rather than each reimplementing its own notion of "spending" or
//! "balance." See ADR-0007 for the balance/opening-balance design.

use rusqlite::{params, Connection, OptionalExtension};

use crate::accounts::{self, Account, AccountError};
use crate::dates;
use crate::transactions::{Transaction, TransactionType};

const OPENING_BALANCE_LABEL: &str = "Opening balance";

/// How much a single transaction contributes to a "total spending" figure.
/// Purchases add their magnitude; refunds subtract theirs (a refund is
/// money back, reducing net spending, regardless of which specific purchase
/// it corresponds to - MIDAS does not auto-link refunds to purchases, per
/// Phase 2's explicit non-goal). Income, transfers, and the opening-balance
/// anchor never count as spending - this is what keeps a credit-card
/// payment from ever becoming a second expense on top of the purchases it
/// pays off.
pub fn spending_effect(transaction_type: TransactionType, amount_cents: i64) -> i64 {
    match transaction_type {
        TransactionType::Purchase => amount_cents.abs(),
        TransactionType::Refund => -amount_cents.abs(),
        TransactionType::Income | TransactionType::Transfer | TransactionType::OpeningBalance => 0,
    }
}

pub fn total_spending(transactions: &[Transaction]) -> i64 {
    transactions
        .iter()
        .map(|t| spending_effect(t.transaction_type, t.amount_cents))
        .sum()
}

/// Net worth: included assets minus included liabilities - never a blind
/// sum of every account balance regardless of type. ADR-0004's sign
/// convention already makes a liability's balance "positive = owe more," so
/// it is subtracted rather than added. Archived accounts and accounts with
/// `include_in_net_worth = false` are excluded.
pub fn net_worth(accounts: &[Account]) -> i64 {
    accounts
        .iter()
        .filter(|a| a.include_in_net_worth && !a.is_archived)
        .map(|a| {
            if a.account_type.is_liability() {
                -a.balance_cents
            } else {
                a.balance_cents
            }
        })
        .sum()
}

/// Establishes or corrects an account's balance baseline (ADR-0007) - the
/// smallest mechanism for the "partial history" problem, where MIDAS only
/// holds some of an account's real transaction history. Computes the gap
/// between `known_current_balance_cents` (what the owner's bank actually
/// shows as of `as_of_date`) and the sum of every other transaction already
/// on record, then upserts a single `opening_balance` anchor transaction
/// for exactly that gap - dated one day before the account's earliest other
/// transaction (or `as_of_date` if there are none yet) so it always sorts
/// strictly first, with no same-day tiebreak ambiguity anywhere else in the
/// app. Deletes the anchor if reconciliation now computes a zero gap,
/// keeping the ledger's shape identical to "never needed one."
pub fn reconcile_balance(
    conn: &Connection,
    account_id: i64,
    known_current_balance_cents: i64,
    as_of_date: &str,
) -> Result<Account, AccountError> {
    let (sum_of_real_transactions, earliest_other_date): (i64, Option<String>) = conn.query_row(
        "SELECT COALESCE(SUM(amount_cents), 0), MIN(transaction_date) FROM transactions
         WHERE account_id = ?1 AND transaction_type != 'opening_balance' AND is_archived = 0",
        params![account_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let delta = known_current_balance_cents - sum_of_real_transactions;

    let anchor_date = match earliest_other_date {
        Some(d) => dates::add_days(&d, -1),
        None => as_of_date.to_string(),
    };

    let existing_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM transactions WHERE account_id = ?1 AND transaction_type = 'opening_balance'",
            params![account_id],
            |row| row.get(0),
        )
        .optional()?;

    match (existing_id, delta) {
        (Some(id), 0) => {
            conn.execute("DELETE FROM transactions WHERE id = ?1", params![id])?;
        }
        (Some(id), _) => {
            conn.execute(
                "UPDATE transactions SET amount_cents = ?1, transaction_date = ?2,
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?3",
                params![delta, anchor_date, id],
            )?;
        }
        (None, 0) => {}
        (None, _) => {
            conn.execute(
                "INSERT INTO transactions (
                    account_id, transaction_date, amount_cents, transaction_type,
                    raw_description, merchant
                 ) VALUES (?1, ?2, ?3, 'opening_balance', ?4, ?4)",
                params![account_id, anchor_date, delta, OPENING_BALANCE_LABEL],
            )?;
        }
    }

    accounts::get(conn, account_id)?.ok_or(AccountError::NotFound(account_id))
}

#[tauri::command]
pub fn get_net_worth(state: tauri::State<crate::db::DbState>) -> Result<i64, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let accounts = accounts::list(&conn, false).map_err(|e| e.to_string())?;
    Ok(net_worth(&accounts))
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
        // assets: 100_000 + 500_000 + 20_000 + 3_000_000 = 3_620_000
        // liabilities: 40_000 + 1_000_000 + 10_000 = 1_050_000
        assert_eq!(net_worth(&accounts), 3_620_000 - 1_050_000);
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
        assert_eq!(net_worth(&accounts), 100_000);
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
    fn reconcile_is_idempotent_and_updates_in_place_rather_than_accumulating() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        reconcile_balance(&conn, checking, 100_000, "2026-08-25").unwrap();
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
        let anchors: Vec<_> = txns
            .iter()
            .filter(|t| t.transaction_type == TransactionType::OpeningBalance)
            .collect();
        assert_eq!(
            anchors.len(),
            1,
            "must upsert, never accumulate a second anchor"
        );
        assert_eq!(balance_of(&conn, checking), 150_000);
    }

    #[test]
    fn reconcile_removes_the_anchor_once_it_is_no_longer_needed() {
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

        // Now the real transactions already sum to the known balance -
        // reconciling again should clean up the now-unnecessary anchor.
        reconcile_balance(&conn, checking, 50_000, "2026-08-25").unwrap();

        let txns = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                account_id: Some(checking),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(txns
            .iter()
            .all(|t| t.transaction_type != TransactionType::OpeningBalance));
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
}
