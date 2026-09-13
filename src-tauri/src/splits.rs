//! Split transactions (Transaction Intelligence, specs/phase-012). A split
//! is a reporting-layer breakdown of what a single ledger movement was
//! actually for - never a second set of ledger rows. The parent
//! transaction's own `amount_cents` remains the one and only thing that
//! ever affects account balance or reconciliation (section 14: "do not
//! double-count split children as separate ledger movements" - this module
//! never writes to `transactions` at all, only to the separate
//! `transaction_splits` child table). `reports.rs` is the one reader that
//! expands a split transaction into its children for category totals;
//! everything else (balance, reconciliation, budgets) reads the parent
//! exactly as it always has.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::DbState;
use crate::transactions::format_dollars;

#[derive(Debug, Clone, Serialize)]
pub struct Split {
    pub id: i64,
    pub transaction_id: i64,
    pub category: Option<String>,
    pub subcategory: Option<String>,
    pub amount_cents: i64,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct SplitInput {
    pub category: Option<String>,
    pub subcategory: Option<String>,
    pub amount_cents: i64,
    pub notes: Option<String>,
}

#[derive(Debug)]
pub enum SplitError {
    Validation(String),
    Db(rusqlite::Error),
    NotFound(i64),
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SplitError::Validation(msg) => write!(f, "{msg}"),
            SplitError::Db(e) => write!(f, "database error: {e}"),
            SplitError::NotFound(id) => write!(f, "transaction {id} not found"),
        }
    }
}

impl From<rusqlite::Error> for SplitError {
    fn from(e: rusqlite::Error) -> Self {
        SplitError::Db(e)
    }
}

const COLUMNS: &str =
    "id, transaction_id, category, subcategory, amount_cents, notes, created_at, updated_at";

fn row_to_split(row: &rusqlite::Row) -> rusqlite::Result<Split> {
    Ok(Split {
        id: row.get("id")?,
        transaction_id: row.get("transaction_id")?,
        category: row.get("category")?,
        subcategory: row.get("subcategory")?,
        amount_cents: row.get("amount_cents")?,
        notes: row.get("notes")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

pub fn list_for_transaction(
    conn: &Connection,
    transaction_id: i64,
) -> Result<Vec<Split>, SplitError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM transaction_splits WHERE transaction_id = ?1 ORDER BY id"
    ))?;
    let rows = stmt.query_map(params![transaction_id], row_to_split)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(SplitError::from)
}

/// Replaces every split for `transaction_id` with `splits` in one
/// transaction - the only way splits are ever written, so "edit the split
/// composition" and "remove all splits" (passing an empty list) are the
/// same operation. Validates the new set sums *exactly* to the parent's own
/// `amount_cents` before writing anything (section 13: never allow a
/// committed mismatch) - integer cents throughout, no floating point.
/// Never touches the parent row itself.
pub fn replace_for_transaction(
    conn: &mut Connection,
    transaction_id: i64,
    splits: &[SplitInput],
) -> Result<Vec<Split>, SplitError> {
    let parent_amount: Option<i64> = conn
        .query_row(
            "SELECT amount_cents FROM transactions WHERE id = ?1",
            params![transaction_id],
            |row| row.get(0),
        )
        .optional()?;
    let parent_amount = parent_amount.ok_or(SplitError::NotFound(transaction_id))?;

    if !splits.is_empty() {
        if splits.len() < 2 {
            return Err(SplitError::Validation(
                "A split needs at least two parts - otherwise there is nothing to split."
                    .to_string(),
            ));
        }
        if splits.iter().any(|s| s.amount_cents == 0) {
            return Err(SplitError::Validation(
                "Each split amount must be non-zero.".to_string(),
            ));
        }
        let sum: i64 = splits.iter().map(|s| s.amount_cents).sum();
        if sum != parent_amount {
            let remaining = parent_amount - sum;
            return Err(SplitError::Validation(format!(
                "Splits must sum exactly to the transaction's amount ({}) - currently assigned {}, {} remaining.",
                format_dollars(parent_amount),
                format_dollars(sum),
                format_dollars(remaining)
            )));
        }
    }

    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM transaction_splits WHERE transaction_id = ?1",
        params![transaction_id],
    )?;
    for s in splits {
        tx.execute(
            "INSERT INTO transaction_splits (transaction_id, category, subcategory, amount_cents, notes)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                transaction_id,
                s.category.as_deref().map(str::trim).filter(|c| !c.is_empty()),
                s.subcategory.as_deref().map(str::trim).filter(|c| !c.is_empty()),
                s.amount_cents,
                s.notes.as_deref().map(str::trim).filter(|n| !n.is_empty()),
            ],
        )?;
    }
    tx.commit()?;

    list_for_transaction(conn, transaction_id)
}

#[tauri::command]
pub fn list_splits(state: State<DbState>, transaction_id: i64) -> Result<Vec<Split>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list_for_transaction(&conn, transaction_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_splits(
    state: State<DbState>,
    transaction_id: i64,
    splits: Vec<SplitInput>,
) -> Result<Vec<Split>, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    replace_for_transaction(&mut conn, transaction_id, &splits).map_err(|e| e.to_string())
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

    fn make_transaction(conn: &Connection, amount_cents: i64) -> i64 {
        let account_id = accounts::create(
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
        .id;
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents,
                transaction_type: TransactionType::Purchase,
                raw_description: "Walmart".to_string(),
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
        .unwrap()
        .id
    }

    fn split(category: &str, amount_cents: i64) -> SplitInput {
        SplitInput {
            category: Some(category.to_string()),
            subcategory: None,
            amount_cents,
            notes: None,
        }
    }

    #[test]
    fn exact_total_splits_write_successfully_and_round_trip() {
        let mut conn = test_conn();
        let txn_id = make_transaction(&conn, -12000);

        let result = replace_for_transaction(
            &mut conn,
            txn_id,
            &[
                split("Groceries", -8000),
                split("Household", -2500),
                split("Clothing", -1500),
            ],
        )
        .unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(
            list_for_transaction(&conn, txn_id).unwrap().len(),
            3,
            "must round-trip via a fresh read, not just the write's return value"
        );
    }

    #[test]
    fn under_allocation_is_rejected_and_writes_nothing() {
        let mut conn = test_conn();
        let txn_id = make_transaction(&conn, -12000);

        let err = replace_for_transaction(
            &mut conn,
            txn_id,
            &[split("Groceries", -8000), split("Household", -2000)],
        )
        .unwrap_err();
        assert!(matches!(err, SplitError::Validation(_)));
        assert_eq!(list_for_transaction(&conn, txn_id).unwrap().len(), 0);
    }

    #[test]
    fn over_allocation_is_rejected_and_writes_nothing() {
        let mut conn = test_conn();
        let txn_id = make_transaction(&conn, -12000);

        let err = replace_for_transaction(
            &mut conn,
            txn_id,
            &[split("Groceries", -8000), split("Household", -5000)],
        )
        .unwrap_err();
        assert!(matches!(err, SplitError::Validation(_)));
        assert_eq!(list_for_transaction(&conn, txn_id).unwrap().len(), 0);
    }

    #[test]
    fn a_single_split_is_rejected_there_is_nothing_to_split() {
        let mut conn = test_conn();
        let txn_id = make_transaction(&conn, -12000);

        let err =
            replace_for_transaction(&mut conn, txn_id, &[split("Groceries", -12000)]).unwrap_err();
        assert!(matches!(err, SplitError::Validation(_)));
    }

    #[test]
    fn a_zero_amount_split_is_rejected() {
        let mut conn = test_conn();
        let txn_id = make_transaction(&conn, -12000);

        let err = replace_for_transaction(
            &mut conn,
            txn_id,
            &[split("Groceries", -12000), split("Household", 0)],
        )
        .unwrap_err();
        assert!(matches!(err, SplitError::Validation(_)));
    }

    #[test]
    fn editing_split_composition_replaces_the_previous_set_entirely() {
        let mut conn = test_conn();
        let txn_id = make_transaction(&conn, -12000);
        replace_for_transaction(
            &mut conn,
            txn_id,
            &[split("Groceries", -8000), split("Household", -4000)],
        )
        .unwrap();

        replace_for_transaction(
            &mut conn,
            txn_id,
            &[
                split("Groceries", -6000),
                split("Household", -4000),
                split("Clothing", -2000),
            ],
        )
        .unwrap();

        let final_splits = list_for_transaction(&conn, txn_id).unwrap();
        assert_eq!(final_splits.len(), 3);
        assert_eq!(
            final_splits.iter().map(|s| s.amount_cents).sum::<i64>(),
            -12000
        );
    }

    #[test]
    fn passing_an_empty_list_removes_all_splits_unsplitting_the_transaction() {
        let mut conn = test_conn();
        let txn_id = make_transaction(&conn, -12000);
        replace_for_transaction(
            &mut conn,
            txn_id,
            &[split("Groceries", -8000), split("Household", -4000)],
        )
        .unwrap();
        assert_eq!(list_for_transaction(&conn, txn_id).unwrap().len(), 2);

        replace_for_transaction(&mut conn, txn_id, &[]).unwrap();
        assert_eq!(list_for_transaction(&conn, txn_id).unwrap().len(), 0);
    }

    #[test]
    fn splitting_never_changes_the_parents_own_ledger_amount_or_account_balance() {
        let mut conn = test_conn();
        let txn_id = make_transaction(&conn, -12000);
        let account_id = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                ..Default::default()
            },
        )
        .unwrap()[0]
            .account_id;

        replace_for_transaction(
            &mut conn,
            txn_id,
            &[split("Groceries", -8000), split("Household", -4000)],
        )
        .unwrap();

        let account = accounts::get(&conn, account_id).unwrap().unwrap();
        assert_eq!(
            account.balance_cents, -12000,
            "the account balance must be driven only by the parent's own amount_cents, never by summing split children again"
        );
    }

    #[test]
    fn splitting_a_nonexistent_transaction_is_a_clear_not_found_error() {
        let mut conn = test_conn();
        let err = replace_for_transaction(&mut conn, 999, &[split("Groceries", -100)]).unwrap_err();
        assert!(matches!(err, SplitError::NotFound(999)));
    }

    #[test]
    fn splits_never_touch_the_transactions_table_at_all() {
        let mut conn = test_conn();
        let txn_id = make_transaction(&conn, -12000);
        let before: i64 = conn
            .query_row("SELECT count(*) FROM transactions", [], |r| r.get(0))
            .unwrap();

        replace_for_transaction(
            &mut conn,
            txn_id,
            &[split("Groceries", -8000), split("Household", -4000)],
        )
        .unwrap();

        let after: i64 = conn
            .query_row("SELECT count(*) FROM transactions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            before, after,
            "splits must never create additional rows in transactions - section 14's no-double-counting requirement"
        );
    }
}
