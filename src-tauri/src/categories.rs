//! Free-text category cleanup (Transaction Intelligence, specs/phase-012).
//!
//! MIDAS's category model is deliberately still plain text on
//! `transactions`/`merchant_rules`/`budgets` (see the phase-012 spec's
//! reconnaissance section for why a relational category-group model was
//! not introduced this batch - it would touch budgets, reports, rules, CSV
//! mapping, and OFX pervasively for a batch explicitly told to avoid a
//! disruptive redesign). "Merge" here means renaming every reference from
//! one category name to another - not a relational merge of rows, since no
//! `categories` table exists to merge rows in.
use rusqlite::{params, Connection};

#[derive(Debug)]
pub enum CategoryError {
    Validation(String),
    Db(rusqlite::Error),
}

impl std::fmt::Display for CategoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CategoryError::Validation(msg) => write!(f, "{msg}"),
            CategoryError::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl From<rusqlite::Error> for CategoryError {
    fn from(e: rusqlite::Error) -> Self {
        CategoryError::Db(e)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RenameSummary {
    pub transactions_updated: i64,
    pub splits_updated: i64,
    pub rules_updated: i64,
    pub budgets_updated: i64,
    /// A budget for `to` already existed in some month(s) that also had a
    /// budget for `from` - each such month's `from` budget is left
    /// untouched rather than erroring the whole rename out or silently
    /// deleting one of the two (`budgets.(category, month)` is unique, so
    /// both can never coexist under the same name). The owner resolves
    /// these manually in Budgets - a rare edge case, not a reason to block
    /// consolidating the common case.
    pub budgets_skipped_due_to_conflict: i64,
}

/// Renames every reference to category `from` to `to`, across
/// transactions, their splits, saved rules, and (where it does not collide
/// with an existing budget) budgets. An explicit, single user action - not
/// a side effect of anything else - so updating all of these together is
/// exactly what the owner asked for, not an unrelated silent rewrite
/// (section 8).
pub fn rename(conn: &mut Connection, from: &str, to: &str) -> Result<RenameSummary, CategoryError> {
    let from = from.trim();
    let to = to.trim();
    if from.is_empty() || to.is_empty() {
        return Err(CategoryError::Validation(
            "Both the source and target category names are required.".to_string(),
        ));
    }
    if from.eq_ignore_ascii_case(to) {
        return Err(CategoryError::Validation(
            "The source and target category names must be different.".to_string(),
        ));
    }

    let tx = conn.transaction()?;

    let transactions_updated = tx.execute(
        "UPDATE transactions SET category = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE category = ?2",
        params![to, from],
    )? as i64;
    let splits_updated = tx.execute(
        "UPDATE transaction_splits SET category = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE category = ?2",
        params![to, from],
    )? as i64;
    let rules_updated = tx.execute(
        "UPDATE merchant_rules SET category = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE category = ?2",
        params![to, from],
    )? as i64;

    // Budgets: rename month by month, skipping any month where a `to`
    // budget already exists (the unique (category, month) index would
    // otherwise reject the rename outright for every month, not just the
    // conflicting one).
    let conflicting_months: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT f.month FROM budgets f
             JOIN budgets t ON t.month = f.month AND t.category = ?2
             WHERE f.category = ?1",
        )?;
        let rows = stmt.query_map(params![from, to], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let budgets_skipped_due_to_conflict = conflicting_months.len() as i64;

    let budgets_updated = if conflicting_months.is_empty() {
        tx.execute(
            "UPDATE budgets SET category = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE category = ?2",
            params![to, from],
        )? as i64
    } else {
        let placeholders = conflicting_months
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE budgets SET category = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE category = ? AND month NOT IN ({placeholders})"
        );
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(to.to_string()), Box::new(from.to_string())];
        for month in &conflicting_months {
            params_vec.push(Box::new(month.clone()));
        }
        let arg_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        tx.execute(&sql, arg_refs.as_slice())? as i64
    };

    tx.commit()?;

    Ok(RenameSummary {
        transactions_updated,
        splits_updated,
        rules_updated,
        budgets_updated,
        budgets_skipped_due_to_conflict,
    })
}

#[tauri::command]
pub fn rename_category(
    state: tauri::State<crate::db::DbState>,
    from: String,
    to: String,
) -> Result<RenameSummary, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    rename(&mut conn, &from, &to).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::budgets::{self, BudgetInput, Classification};
    use crate::rules::{self, MatchType, MerchantRuleInput};
    use crate::splits;
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
        conn.execute_batch(include_str!(
            "../migrations/0013_recurring_intelligence.sql"
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

    fn make_txn(conn: &Connection, account_id: i64, category: &str) -> i64 {
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
                category: Some(category.to_string()),
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
    fn rename_updates_transactions_splits_and_rules() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        let t1 = make_txn(&conn, account_id, "Restaurants");
        let t2 = make_txn(&conn, account_id, "Restaurants");
        make_txn(&conn, account_id, "Groceries"); // must be unaffected
        splits::replace_for_transaction(
            &mut conn,
            t1,
            &[
                splits::SplitInput {
                    category: Some("Restaurants".to_string()),
                    subcategory: None,
                    amount_cents: -600,
                    notes: None,
                },
                splits::SplitInput {
                    category: Some("Groceries".to_string()),
                    subcategory: None,
                    amount_cents: -400,
                    notes: None,
                },
            ],
        )
        .unwrap();
        rules::create(
            &conn,
            &MerchantRuleInput {
                match_type: MatchType::Contains,
                match_value: "diner".to_string(),
                account_id: None,
                normalized_merchant: None,
                category: Some("Restaurants".to_string()),
                subcategory: None,
                priority: 0,
            },
        )
        .unwrap();

        let summary = rename(&mut conn, "Restaurants", "Dining").unwrap();
        assert_eq!(summary.transactions_updated, 2);
        assert_eq!(summary.splits_updated, 1);
        assert_eq!(summary.rules_updated, 1);

        let t1_after = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .find(|t| t.id == t1)
        .unwrap();
        assert_eq!(t1_after.category.as_deref(), Some("Dining"));
        let t2_after_category = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .find(|t| t.id == t2)
        .unwrap()
        .category;
        assert_eq!(t2_after_category.as_deref(), Some("Dining"));

        let groceries_split_untouched = splits::list_for_transaction(&conn, t1)
            .unwrap()
            .into_iter()
            .find(|s| s.amount_cents == -400)
            .unwrap();
        assert_eq!(
            groceries_split_untouched.category.as_deref(),
            Some("Groceries"),
            "a split in an unrelated category must never be touched by the rename"
        );
    }

    #[test]
    fn rename_rejects_identical_names_and_empty_names() {
        let mut conn = test_conn();
        assert!(matches!(
            rename(&mut conn, "Dining", "Dining"),
            Err(CategoryError::Validation(_))
        ));
        assert!(matches!(
            rename(&mut conn, "", "Dining"),
            Err(CategoryError::Validation(_))
        ));
    }

    #[test]
    fn rename_updates_budgets_and_skips_a_colliding_month_rather_than_failing_outright() {
        let mut conn = test_conn();
        budgets::create(
            &conn,
            &BudgetInput {
                category: "Restaurants".to_string(),
                month: "2026-08-01".to_string(),
                planned_amount_cents: 20000,
                classification: Classification::Flexible,
            },
        )
        .unwrap();
        budgets::create(
            &conn,
            &BudgetInput {
                category: "Restaurants".to_string(),
                month: "2026-09-01".to_string(),
                planned_amount_cents: 20000,
                classification: Classification::Flexible,
            },
        )
        .unwrap();
        // September already has a "Dining" budget - renaming into it there
        // would violate the (category, month) uniqueness.
        budgets::create(
            &conn,
            &BudgetInput {
                category: "Dining".to_string(),
                month: "2026-09-01".to_string(),
                planned_amount_cents: 30000,
                classification: Classification::Flexible,
            },
        )
        .unwrap();

        let summary = rename(&mut conn, "Restaurants", "Dining").unwrap();
        assert_eq!(summary.budgets_updated, 1, "only August's had no conflict");
        assert_eq!(summary.budgets_skipped_due_to_conflict, 1);

        let august = budgets::list_for_month(&conn, "2026-08-01").unwrap();
        assert_eq!(august[0].category, "Dining");
        let september = budgets::list_for_month(&conn, "2026-09-01").unwrap();
        assert_eq!(
            september.len(),
            2,
            "the untouched September Restaurants budget must survive alongside the pre-existing Dining one"
        );
        assert!(september.iter().any(|b| b.category == "Restaurants"));
        assert!(september.iter().any(|b| b.category == "Dining"));
    }

    #[test]
    fn rename_never_touches_an_unrelated_category() {
        let mut conn = test_conn();
        let account_id = make_account(&conn);
        make_txn(&conn, account_id, "Groceries");

        rename(&mut conn, "Restaurants", "Dining").unwrap();

        let groceries_txn = transactions::list(
            &conn,
            &transactions::TransactionFilter {
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(groceries_txn[0].category.as_deref(), Some("Groceries"));
    }
}
