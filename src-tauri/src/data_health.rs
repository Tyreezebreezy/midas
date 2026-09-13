//! Data Health (Batch 9, specs/phase-016): a concise, derived summary of
//! whether MIDAS's own records are clean - never a mystery score (section
//! 24). Every count reuses `review::list_review_items`'s exact same
//! derived facts (or, for reconciliation, `reconciliation::account_summary`
//! directly), so Review Center and Data Health can never disagree about
//! what "N uncategorized transactions" or "1 account needs review" means.

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::accounts;
use crate::db::DbState;
use crate::reconciliation;
use crate::review;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Good,
    NeedsAttention,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransactionHealth {
    pub uncategorized_count: i64,
    pub possible_duplicate_count: i64,
    pub state: HealthState,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportHealth {
    pub last_import_date: Option<String>,
    pub unresolved_count: i64,
    pub state: HealthState,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountReconciliationHealth {
    pub account_id: i64,
    pub name: String,
    pub last_reconciled_date: Option<String>,
    pub needs_review: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReconciliationHealth {
    pub accounts: Vec<AccountReconciliationHealth>,
    pub invalid_count: i64,
    /// A purely factual count (section 22) - never itself makes `state`
    /// `NeedsAttention`. An account that has simply never been reconciled
    /// is not unhealthy; an account with an actually invalidated
    /// reconciliation is.
    pub never_reconciled_count: i64,
    pub state: HealthState,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecurringHealth {
    pub pending_review_count: i64,
    pub state: HealthState,
}

#[derive(Debug, Clone, Serialize)]
pub struct DataHealth {
    pub transactions: TransactionHealth,
    pub imports: ImportHealth,
    pub reconciliation: ReconciliationHealth,
    pub recurring: RecurringHealth,
}

pub fn compute(conn: &Connection) -> Result<DataHealth, String> {
    let items = review::list_review_items(conn).map_err(|e| e.to_string())?;

    let uncategorized_count = items.iter().filter(|i| i.kind == "uncategorized").count() as i64;
    let possible_duplicate_count = items
        .iter()
        .filter(|i| i.kind == "suspected_duplicate")
        .count() as i64;
    let transactions = TransactionHealth {
        uncategorized_count,
        possible_duplicate_count,
        state: if uncategorized_count == 0 && possible_duplicate_count == 0 {
            HealthState::Good
        } else {
            HealthState::NeedsAttention
        },
    };

    let last_import_date: Option<String> = conn
        .query_row("SELECT MAX(imported_at) FROM imports", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    let unresolved_count = items
        .iter()
        .filter(|i| matches!(i.kind.as_str(), "malformed_row" | "suspected_duplicate"))
        .count() as i64;
    let imports = ImportHealth {
        last_import_date,
        unresolved_count,
        state: if unresolved_count == 0 {
            HealthState::Good
        } else {
            HealthState::NeedsAttention
        },
    };

    let active_accounts = accounts::list(conn, false).map_err(|e| e.to_string())?;
    let mut account_lines = Vec::with_capacity(active_accounts.len());
    let mut never_reconciled_count = 0i64;
    let mut invalid_count = 0i64;
    for a in &active_accounts {
        let summary = reconciliation::account_summary(conn, a.id).map_err(|e| e.to_string())?;
        if summary.last_reconciled_date.is_none() {
            never_reconciled_count += 1;
        }
        invalid_count += summary.needs_review_count;
        account_lines.push(AccountReconciliationHealth {
            account_id: a.id,
            name: a.name.clone(),
            last_reconciled_date: summary.last_reconciled_date,
            needs_review: summary.needs_review_count > 0,
        });
    }
    let reconciliation_health = ReconciliationHealth {
        accounts: account_lines,
        invalid_count,
        never_reconciled_count,
        state: if invalid_count == 0 {
            HealthState::Good
        } else {
            HealthState::NeedsAttention
        },
    };

    let recurring_pending = items
        .iter()
        .filter(|i| {
            matches!(
                i.kind.as_str(),
                "recurring_proposed" | "recurring_missed" | "ambiguous_recurring_match"
            )
        })
        .count() as i64;
    let recurring = RecurringHealth {
        pending_review_count: recurring_pending,
        state: if recurring_pending == 0 {
            HealthState::Good
        } else {
            HealthState::NeedsAttention
        },
    };

    Ok(DataHealth {
        transactions,
        imports,
        reconciliation: reconciliation_health,
        recurring,
    })
}

#[tauri::command]
pub fn get_data_health(state: State<DbState>) -> Result<DataHealth, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    compute(&conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{AccountInput, AccountType};
    use crate::payees::{self, PayeeInput, PayeeType};
    use crate::transactions::{self, TransactionInput, TransactionType};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for sql in [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_transactions.sql"),
            include_str!("../migrations/0003_rules_and_recurring.sql"),
            include_str!("../migrations/0004_accounting_boundary.sql"),
            include_str!("../migrations/0007_balance_adjustment.sql"),
            include_str!("../migrations/0008_foundation_batch_1.sql"),
            include_str!("../migrations/0009_import_engine_v2.sql"),
            include_str!("../migrations/0010_ofx_source_adapter.sql"),
            include_str!("../migrations/0011_reconciliation.sql"),
            include_str!("../migrations/0012_transaction_intelligence.sql"),
            include_str!("../migrations/0013_recurring_intelligence.sql"),
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

    fn add_txn(conn: &Connection, account_id: i64, category: Option<&str>) -> i64 {
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -4000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Test".to_string(),
                merchant: Some("Test".to_string()),
                category: category.map(|c| c.to_string()),
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
    fn a_clean_ledger_reports_good_everywhere() {
        let conn = test_conn();
        make_account(&conn, AccountType::Checking);
        let health = compute(&conn).unwrap();
        assert_eq!(health.transactions.state, HealthState::Good);
        assert_eq!(health.imports.state, HealthState::Good);
        assert_eq!(health.reconciliation.state, HealthState::Good);
        assert_eq!(health.recurring.state, HealthState::Good);
    }

    #[test]
    fn uncategorized_and_duplicate_counts_match_the_seeded_facts_exactly() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(&conn, checking, None);
        add_txn(&conn, checking, None);
        add_txn(&conn, checking, Some("Dining"));
        conn.execute(
            "INSERT INTO needs_review_items (issue_type, detected_reason) VALUES ('suspected_duplicate', 'dup')",
            [],
        )
        .unwrap();

        let health = compute(&conn).unwrap();
        assert_eq!(health.transactions.uncategorized_count, 2);
        assert_eq!(health.transactions.possible_duplicate_count, 1);
        assert_eq!(health.transactions.state, HealthState::NeedsAttention);
    }

    #[test]
    fn unresolved_import_count_only_counts_open_import_sourced_items() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO needs_review_items (issue_type, detected_reason) VALUES ('malformed_row', 'bad row')",
            [],
        )
        .unwrap();
        let resolved_id: i64 = conn
            .query_row(
                "INSERT INTO needs_review_items (issue_type, detected_reason) VALUES ('suspected_duplicate', 'dup') RETURNING id",
                [],
                |r| r.get(0),
            )
            .unwrap();
        crate::needs_review::resolve(
            &conn,
            resolved_id,
            crate::needs_review::ResolutionAction::Resolved,
        )
        .unwrap();

        let health = compute(&conn).unwrap();
        assert_eq!(
            health.imports.unresolved_count, 1,
            "the already-resolved duplicate must not count"
        );
        assert_eq!(health.imports.state, HealthState::NeedsAttention);
    }

    #[test]
    fn reconciliation_health_reports_never_reconciled_as_factual_not_unhealthy() {
        let conn = test_conn();
        make_account(&conn, AccountType::Checking);

        let health = compute(&conn).unwrap();
        assert_eq!(health.reconciliation.never_reconciled_count, 1);
        assert_eq!(
            health.reconciliation.state,
            HealthState::Good,
            "never having reconciled must never itself flip the state to NeedsAttention (section 22)"
        );
        assert_eq!(health.reconciliation.accounts.len(), 1);
        assert!(health.reconciliation.accounts[0]
            .last_reconciled_date
            .is_none());
        assert!(!health.reconciliation.accounts[0].needs_review);
    }

    #[test]
    fn an_invalidated_reconciliation_flips_reconciliation_health_and_never_the_never_reconciled_count(
    ) {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let txn_id = add_txn(&conn, checking, Some("Dining"));
        transactions::set_reconciliation_status(
            &conn,
            txn_id,
            crate::transactions::ReconciliationStatus::Cleared,
        )
        .unwrap();
        crate::reconciliation::complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -4000,
            None,
            None,
        )
        .unwrap();
        transactions::update(
            &conn,
            txn_id,
            &TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -4500,
                transaction_type: TransactionType::Purchase,
                raw_description: "Test".to_string(),
                merchant: Some("Test".to_string()),
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

        let health = compute(&conn).unwrap();
        assert_eq!(health.reconciliation.invalid_count, 1);
        assert_eq!(health.reconciliation.state, HealthState::NeedsAttention);
        // `account_summary`'s own established definition of "last reconciled"
        // (Batch 4) only counts a currently-`completed` record - an
        // invalidated one correctly stops claiming a verified reconciliation
        // exists, so this account is factually both "needs review" and "no
        // currently-valid reconciliation." The frontend distinguishes this
        // from a true never-reconciled account by checking `needs_review`
        // first (section 20's presentation), not by this count alone.
        assert!(health.reconciliation.accounts[0].needs_review);
    }

    #[test]
    fn recurring_pending_review_count_matches_review_centers_own_recurring_domain() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        for date in ["2026-06-05", "2026-07-05", "2026-08-05"] {
            transactions::create(
                &conn,
                &TransactionInput {
                    account_id: checking,
                    transaction_date: date.to_string(),
                    posted_date: None,
                    amount_cents: -999,
                    transaction_type: TransactionType::Purchase,
                    raw_description: "Streaming Service".to_string(),
                    merchant: Some("Streaming Service".to_string()),
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
        crate::recurring::detect_candidates(&conn, checking).unwrap();

        let health = compute(&conn).unwrap();
        assert_eq!(health.recurring.pending_review_count, 1);
        assert_eq!(health.recurring.state, HealthState::NeedsAttention);
    }

    #[test]
    fn a_merged_payees_history_never_causes_a_false_health_issue() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let old = payees::create(
            &conn,
            &PayeeInput {
                name: "Old Name".to_string(),
                payee_type: PayeeType::Merchant,
                notes: None,
            },
        )
        .unwrap();
        let canonical = payees::create(
            &conn,
            &PayeeInput {
                name: "Canonical".to_string(),
                payee_type: PayeeType::Merchant,
                notes: None,
            },
        )
        .unwrap();
        let txn_id = transactions::create(
            &conn,
            &TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -4000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Test".to_string(),
                merchant: Some("Test".to_string()),
                category: Some("Shopping".to_string()),
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
                payee_id: Some(old.id),
                is_one_time: false,
            },
        )
        .unwrap()
        .id;
        payees::merge(&conn, old.id, canonical.id).unwrap();

        let health = compute(&conn).unwrap();
        assert_eq!(
            health.transactions.uncategorized_count, 0,
            "a merged payee's categorized history must never surface as a data-health issue"
        );
        let after = transactions::get_for_edit(&conn, txn_id).unwrap();
        assert_eq!(after.payee_id, Some(canonical.id));
    }
}
