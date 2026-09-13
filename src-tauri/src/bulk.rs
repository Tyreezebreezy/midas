//! Bulk transaction cleanup (Transaction Intelligence, specs/phase-012).
//! Every operation here re-fetches each transaction and routes the change
//! back through the same single-item functions (`transactions::update`,
//! `transactions::set_archived`, `tags::attach`/`detach`) that already
//! carry reconciliation-invalidation and validation logic - no separate
//! bulk-specific mutation path exists to accidentally bypass those
//! protections (section 35). All rows in one call are wrapped in a single
//! SQL transaction, so a mid-batch failure leaves nothing partially applied
//! (section 43's "rollback on failure").
//!
//! Deliberately narrow: only the six actions the product handoff actually
//! names (section 16). No generic "bulk-edit any field" API.

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::db::DbState;
use crate::{tags, transactions};

#[derive(Debug, Clone, Serialize)]
pub struct BulkResult {
    pub updated_count: i64,
}

fn update_each<F>(conn: &mut Connection, ids: &[i64], mut apply: F) -> Result<BulkResult, String>
where
    F: FnMut(&Connection, i64) -> Result<(), String>,
{
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for &id in ids {
        apply(&tx, id)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(BulkResult {
        updated_count: ids.len() as i64,
    })
}

fn with_updated_category(
    conn: &Connection,
    id: i64,
    category: Option<String>,
) -> Result<(), String> {
    let mut input = transactions::get_for_edit(conn, id)?;
    input.category = category;
    transactions::update(conn, id, &input).map_err(|e| e.to_string())?;
    Ok(())
}

fn with_updated_payee(conn: &Connection, id: i64, payee_id: Option<i64>) -> Result<(), String> {
    let mut input = transactions::get_for_edit(conn, id)?;
    input.payee_id = payee_id;
    transactions::update(conn, id, &input).map_err(|e| e.to_string())?;
    Ok(())
}

fn with_updated_one_time(conn: &Connection, id: i64, is_one_time: bool) -> Result<(), String> {
    let mut input = transactions::get_for_edit(conn, id)?;
    input.is_one_time = is_one_time;
    transactions::update(conn, id, &input).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn bulk_set_category(
    state: State<DbState>,
    ids: Vec<i64>,
    category: Option<String>,
) -> Result<BulkResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    update_each(&mut conn, &ids, |c, id| {
        with_updated_category(c, id, category.clone())
    })
}

#[tauri::command]
pub fn bulk_set_payee(
    state: State<DbState>,
    ids: Vec<i64>,
    payee_id: Option<i64>,
) -> Result<BulkResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    update_each(&mut conn, &ids, |c, id| with_updated_payee(c, id, payee_id))
}

#[tauri::command]
pub fn bulk_set_one_time(
    state: State<DbState>,
    ids: Vec<i64>,
    is_one_time: bool,
) -> Result<BulkResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    update_each(&mut conn, &ids, |c, id| {
        with_updated_one_time(c, id, is_one_time)
    })
}

#[tauri::command]
pub fn bulk_set_archived(
    state: State<DbState>,
    ids: Vec<i64>,
    archived: bool,
) -> Result<BulkResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    update_each(&mut conn, &ids, |c, id| {
        transactions::set_archived(c, id, archived)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

#[tauri::command]
pub fn bulk_add_tag(
    state: State<DbState>,
    ids: Vec<i64>,
    tag_id: i64,
) -> Result<BulkResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    update_each(&mut conn, &ids, |c, id| {
        tags::attach(c, id, tag_id).map_err(|e| e.to_string())
    })
}

#[tauri::command]
pub fn bulk_remove_tag(
    state: State<DbState>,
    ids: Vec<i64>,
    tag_id: i64,
) -> Result<BulkResult, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    update_each(&mut conn, &ids, |c, id| {
        tags::detach(c, id, tag_id).map_err(|e| e.to_string())
    })
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

    fn make_txn(conn: &Connection, account_id: i64) -> i64 {
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -1000,
                transaction_type: TransactionType::Purchase,
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
        .unwrap()
        .id
    }

    #[test]
    fn bulk_set_category_updates_every_selected_transaction() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let a = make_txn(&conn, account_id);
        let b = make_txn(&conn, account_id);
        let untouched = make_txn(&conn, account_id);

        let result = update_each(&mut conn, &[a, b], |c, id| {
            with_updated_category(c, id, Some("Dining".to_string()))
        })
        .unwrap();
        assert_eq!(result.updated_count, 2);

        let all = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                ..Default::default()
            },
        )
        .unwrap();
        let category_of = |id: i64| all.iter().find(|t| t.id == id).unwrap().category.clone();
        assert_eq!(category_of(a).as_deref(), Some("Dining"));
        assert_eq!(category_of(b).as_deref(), Some("Dining"));
        assert_eq!(
            category_of(untouched),
            None,
            "a transaction not in the selection must be unaffected"
        );
    }

    #[test]
    fn bulk_add_and_remove_tag_round_trips() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let a = make_txn(&conn, account_id);
        let b = make_txn(&conn, account_id);
        let tag = tags::create(&conn, "Business").unwrap();

        update_each(&mut conn, &[a, b], |c, id| {
            tags::attach(c, id, tag.id).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(tags::list_for_transaction(&conn, a).unwrap().len(), 1);
        assert_eq!(tags::list_for_transaction(&conn, b).unwrap().len(), 1);

        update_each(&mut conn, &[a], |c, id| {
            tags::detach(c, id, tag.id).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(tags::list_for_transaction(&conn, a).unwrap().len(), 0);
        assert_eq!(
            tags::list_for_transaction(&conn, b).unwrap().len(),
            1,
            "removing from one selected transaction must not affect another"
        );
    }

    #[test]
    fn a_failure_partway_through_rolls_back_the_whole_batch() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let real = make_txn(&conn, account_id);
        let missing = 999_999; // does not exist - update() will error NotFound

        let err = update_each(&mut conn, &[real, missing], |c, id| {
            with_updated_category(c, id, Some("Dining".to_string()))
        })
        .unwrap_err();
        assert!(err.contains("999999") || err.to_lowercase().contains("not found"));

        let all = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            all[0].category, None,
            "the real transaction's own change must have rolled back along with the failed one"
        );
    }

    #[test]
    fn bulk_set_one_time_and_bulk_archive_work() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let a = make_txn(&conn, account_id);

        update_each(&mut conn, &[a], |c, id| with_updated_one_time(c, id, true)).unwrap();
        let after = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                ..Default::default()
            },
        )
        .unwrap();
        assert!(after[0].is_one_time);

        update_each(&mut conn, &[a], |c, id| {
            transactions::set_archived(c, id, true)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .unwrap();
        let archived = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                include_archived: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(archived[0].is_archived);
    }
}
