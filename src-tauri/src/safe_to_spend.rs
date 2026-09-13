//! Safe to Spend (Phase 4B): the headline MIDAS number. Eligible cash minus
//! confirmed obligations before the next payday, minus protected
//! savings-goal reservations, minus the owner's own required reserve.
//! Every line item is exposed, never just the total (the owner's explicit
//! "clear breakdown" requirement) - see `SafeToSpendResult::Complete`.
//!
//! Reuses `pay_cycle::obligations_before` directly (via
//! `pay_cycle::summarize`) rather than a second implementation, so "known
//! upcoming bills are deducted only once" is guaranteed by construction.
//! When no pay schedule is configured, there is no well-defined "next
//! payday" to measure obligations against, so this returns
//! `SafeToSpendResult::Incomplete` - an honest missing-input state, not a
//! guessed number.

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::accounts;
use crate::db::DbState;
use crate::pay_cycle;
use crate::savings_goals;
use crate::settings;

const RESERVE_SETTINGS_KEY: &str = "safe_to_spend_reserve_cents";

#[derive(Debug)]
pub enum SafeToSpendError {
    Validation(String),
    Db(rusqlite::Error),
    Other(String),
}

impl std::fmt::Display for SafeToSpendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SafeToSpendError::Validation(msg) => write!(f, "{msg}"),
            SafeToSpendError::Db(e) => write!(f, "database error: {e}"),
            SafeToSpendError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<rusqlite::Error> for SafeToSpendError {
    fn from(e: rusqlite::Error) -> Self {
        SafeToSpendError::Db(e)
    }
}

pub fn get_reserve(conn: &Connection) -> Result<i64, SafeToSpendError> {
    match settings::get(conn, RESERVE_SETTINGS_KEY)? {
        Some(s) => s.parse::<i64>().map_err(|_| {
            SafeToSpendError::Other("Stored reserve amount is corrupted.".to_string())
        }),
        None => Ok(0),
    }
}

pub fn set_reserve(conn: &Connection, cents: i64) -> Result<(), SafeToSpendError> {
    if cents < 0 {
        return Err(SafeToSpendError::Validation(
            "Reserve cannot be negative.".to_string(),
        ));
    }
    settings::set(conn, RESERVE_SETTINGS_KEY, &cents.to_string())?;
    Ok(())
}

/// Sum of every non-archived account's balance that is genuinely liquid
/// cash - checking, savings, cash. Never a liability (credit availability
/// is never counted as cash - it simply isn't one of these three types),
/// never an illiquid asset like investment/retirement/property.
fn eligible_cash(conn: &Connection) -> Result<i64, SafeToSpendError> {
    let accounts =
        accounts::list(conn, false).map_err(|e| SafeToSpendError::Other(e.to_string()))?;
    Ok(accounts
        .iter()
        .filter(|a| a.account_type.is_eligible_cash())
        .map(|a| a.balance_cents)
        .sum())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SafeToSpendResult {
    Complete {
        safe_to_spend_cents: i64,
        eligible_cash_cents: i64,
        obligations_before_payday_cents: i64,
        reserved_goal_commitments_cents: i64,
        owner_required_reserve_cents: i64,
        next_payday: String,
    },
    Incomplete {
        reason: String,
        eligible_cash_cents: i64,
    },
}

pub fn calculate(conn: &Connection) -> Result<SafeToSpendResult, SafeToSpendError> {
    let eligible_cash_cents = eligible_cash(conn)?;

    let cycle = pay_cycle::summarize(conn).map_err(|e| SafeToSpendError::Other(e.to_string()))?;
    let Some(cycle) = cycle else {
        return Ok(SafeToSpendResult::Incomplete {
            reason: "No pay schedule configured yet - set one under Settings \u{2192} Financial Preferences to see Safe to Spend.".to_string(),
            eligible_cash_cents,
        });
    };

    let reserved_goal_commitments_cents =
        savings_goals::total_reserved(conn).map_err(|e| SafeToSpendError::Other(e.to_string()))?;
    let owner_required_reserve_cents = get_reserve(conn)?;

    let safe_to_spend_cents = eligible_cash_cents
        - cycle.obligations_before_payday_cents
        - reserved_goal_commitments_cents
        - owner_required_reserve_cents;

    Ok(SafeToSpendResult::Complete {
        safe_to_spend_cents,
        eligible_cash_cents,
        obligations_before_payday_cents: cycle.obligations_before_payday_cents,
        reserved_goal_commitments_cents,
        owner_required_reserve_cents,
        next_payday: cycle.next_payday,
    })
}

#[tauri::command]
pub fn get_safe_to_spend(state: State<DbState>) -> Result<SafeToSpendResult, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    calculate(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_safe_to_spend_reserve(state: State<DbState>) -> Result<i64, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    get_reserve(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_safe_to_spend_reserve(state: State<DbState>, cents: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    set_reserve(&conn, cents).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self as accounts_mod, AccountInput, AccountType};
    use crate::pay_cycle::{set_schedule, PaySchedule};
    use crate::recurring;
    use crate::savings_goals::{self, SavingsGoalInput};
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
        conn.execute_batch(include_str!("../migrations/0006_savings_goals.sql"))
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

    fn make_account(conn: &Connection, name: &str, account_type: AccountType) -> i64 {
        accounts_mod::create(
            conn,
            &AccountInput {
                name: name.to_string(),
                institution: None,
                account_type,
                include_in_net_worth: true,
                notes: None,
            },
        )
        .unwrap()
        .id
    }

    fn add_txn(conn: &Connection, account_id: i64, date: &str, amount_cents: i64, merchant: &str) {
        crate::transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: date.to_string(),
                posted_date: None,
                amount_cents,
                transaction_type: if amount_cents >= 0 {
                    TransactionType::Income
                } else {
                    TransactionType::Purchase
                },
                raw_description: merchant.to_string(),
                merchant: Some(merchant.to_string()),
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

    fn today(conn: &Connection) -> String {
        conn.query_row("SELECT date('now')", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn incomplete_when_no_pay_schedule_but_still_reports_known_eligible_cash() {
        let conn = test_conn();
        let checking = make_account(&conn, "Checking", AccountType::Checking);
        add_txn(&conn, checking, "2026-08-01", 150_000, "Deposit");

        let result = calculate(&conn).unwrap();
        match result {
            SafeToSpendResult::Incomplete {
                eligible_cash_cents,
                reason,
            } => {
                assert_eq!(eligible_cash_cents, 150_000);
                assert!(!reason.is_empty());
            }
            SafeToSpendResult::Complete { .. } => {
                panic!("expected Incomplete with no schedule set")
            }
        }
    }

    #[test]
    fn safe_to_spend_with_no_bills_is_just_eligible_cash() {
        let conn = test_conn();
        let checking = make_account(&conn, "Checking", AccountType::Checking);
        add_txn(&conn, checking, "2026-08-01", 100_000, "Deposit");
        set_schedule(
            &conn,
            &PaySchedule::Biweekly {
                anchor_date: today(&conn),
            },
        )
        .unwrap();

        let result = calculate(&conn).unwrap();
        let SafeToSpendResult::Complete {
            safe_to_spend_cents,
            obligations_before_payday_cents,
            ..
        } = result
        else {
            panic!("expected Complete");
        };
        assert_eq!(obligations_before_payday_cents, 0);
        assert_eq!(safe_to_spend_cents, 100_000);
    }

    #[test]
    fn safe_to_spend_with_obligations_subtracts_confirmed_upcoming_bills() {
        let mut conn = test_conn();
        let checking = make_account(&conn, "Checking", AccountType::Checking);
        add_txn(&conn, checking, &today(&conn), 500_000, "Deposit");

        // Three monthly occurrences (exact 30-day gaps between them, day-
        // based so classification is deterministic regardless of the real
        // calendar date the test runs on) ending 20 days ago. The next
        // occurrence is projected one *calendar* month past the last one
        // (section 7 - add_months, not a naive +30 days), which lands
        // 28-31 days later depending on the months involved; starting 20
        // days back guarantees the projected date is always <= today + 14
        // (the biweekly payday anchored today) regardless of that
        // variance, since 20 + 28 already covers it.
        let last_bill = crate::dates::add_days(&today(&conn), -20);
        let bill_dates = [
            crate::dates::add_days(&last_bill, -60),
            crate::dates::add_days(&last_bill, -30),
            last_bill,
        ];
        for d in &bill_dates {
            add_txn(&conn, checking, d, -1000, "Bill");
        }
        let candidates = recurring::detect_candidates(&conn, checking).unwrap();
        recurring::confirm(&mut conn, candidates[0].id).unwrap();

        set_schedule(
            &conn,
            &PaySchedule::Biweekly {
                anchor_date: today(&conn),
            },
        )
        .unwrap();

        let result = calculate(&conn).unwrap();
        let SafeToSpendResult::Complete {
            safe_to_spend_cents,
            eligible_cash_cents,
            obligations_before_payday_cents,
            ..
        } = result
        else {
            panic!("expected Complete");
        };
        assert_eq!(obligations_before_payday_cents, 1000);
        assert_eq!(safe_to_spend_cents, eligible_cash_cents - 1000);
    }

    #[test]
    fn safe_to_spend_with_owner_reserve_subtracts_it() {
        let conn = test_conn();
        let checking = make_account(&conn, "Checking", AccountType::Checking);
        add_txn(&conn, checking, "2026-08-01", 100_000, "Deposit");
        set_schedule(
            &conn,
            &PaySchedule::Biweekly {
                anchor_date: today(&conn),
            },
        )
        .unwrap();
        set_reserve(&conn, 20_000).unwrap();

        let result = calculate(&conn).unwrap();
        let SafeToSpendResult::Complete {
            safe_to_spend_cents,
            owner_required_reserve_cents,
            ..
        } = result
        else {
            panic!("expected Complete");
        };
        assert_eq!(owner_required_reserve_cents, 20_000);
        assert_eq!(safe_to_spend_cents, 80_000);
    }

    #[test]
    fn safe_to_spend_with_reserve_affects_the_number_only_when_a_goal_reserves_it() {
        let conn = test_conn();
        let checking = make_account(&conn, "Checking", AccountType::Checking);
        add_txn(&conn, checking, "2026-08-01", 100_000, "Deposit");
        set_schedule(
            &conn,
            &PaySchedule::Biweekly {
                anchor_date: today(&conn),
            },
        )
        .unwrap();

        savings_goals::create(
            &conn,
            &SavingsGoalInput {
                name: "Unreserved Goal".to_string(),
                target_amount_cents: 500_000,
                current_amount_cents: 10_000,
                target_date: None,
                reserved_amount_cents: None,
            },
        )
        .unwrap();
        let unreserved_result = calculate(&conn).unwrap();
        let SafeToSpendResult::Complete {
            safe_to_spend_cents: with_unreserved_goal,
            ..
        } = unreserved_result
        else {
            panic!("expected Complete");
        };
        assert_eq!(
            with_unreserved_goal, 100_000,
            "a goal with no reservation must not change the number"
        );

        savings_goals::create(
            &conn,
            &SavingsGoalInput {
                name: "Reserved Goal".to_string(),
                target_amount_cents: 500_000,
                current_amount_cents: 30_000,
                target_date: None,
                reserved_amount_cents: Some(30_000),
            },
        )
        .unwrap();
        let reserved_result = calculate(&conn).unwrap();
        let SafeToSpendResult::Complete {
            safe_to_spend_cents: with_reserved_goal,
            reserved_goal_commitments_cents,
            ..
        } = reserved_result
        else {
            panic!("expected Complete");
        };
        assert_eq!(reserved_goal_commitments_cents, 30_000);
        assert_eq!(with_reserved_goal, 70_000);
    }

    #[test]
    fn credit_card_debt_never_reduces_eligible_cash_or_safe_to_spend() {
        let conn = test_conn();
        let checking = make_account(&conn, "Checking", AccountType::Checking);
        add_txn(&conn, checking, "2026-08-01", 100_000, "Deposit");
        let card = make_account(&conn, "Credit Card", AccountType::CreditCard);
        add_txn(&conn, card, "2026-08-05", 50_000, "Big charge"); // $500 owed
        set_schedule(
            &conn,
            &PaySchedule::Biweekly {
                anchor_date: today(&conn),
            },
        )
        .unwrap();

        let result = calculate(&conn).unwrap();
        let SafeToSpendResult::Complete {
            safe_to_spend_cents,
            eligible_cash_cents,
            ..
        } = result
        else {
            panic!("expected Complete");
        };
        assert_eq!(
            eligible_cash_cents, 100_000,
            "credit availability/debt is never counted as cash, in either direction"
        );
        assert_eq!(safe_to_spend_cents, 100_000);
    }

    #[test]
    fn illiquid_and_archived_accounts_are_excluded_from_eligible_cash() {
        let conn = test_conn();
        let checking = make_account(&conn, "Checking", AccountType::Checking);
        add_txn(&conn, checking, "2026-08-01", 100_000, "Deposit");
        let investment = make_account(&conn, "Investment", AccountType::Investment);
        add_txn(&conn, investment, "2026-08-01", 9_999_999, "Big holding");
        let closed = make_account(&conn, "Closed Savings", AccountType::Savings);
        add_txn(&conn, closed, "2026-08-01", 50_000, "Old savings");
        accounts_mod::set_archived(&conn, closed, true).unwrap();

        set_schedule(
            &conn,
            &PaySchedule::Biweekly {
                anchor_date: today(&conn),
            },
        )
        .unwrap();

        let result = calculate(&conn).unwrap();
        let SafeToSpendResult::Complete {
            eligible_cash_cents,
            ..
        } = result
        else {
            panic!("expected Complete");
        };
        assert_eq!(eligible_cash_cents, 100_000);
    }

    #[test]
    fn safe_to_spend_can_go_negative_rather_than_clamping_a_real_shortfall() {
        let conn = test_conn();
        let checking = make_account(&conn, "Checking", AccountType::Checking);
        add_txn(&conn, checking, "2026-08-01", 5_000, "Small deposit");
        set_schedule(
            &conn,
            &PaySchedule::Biweekly {
                anchor_date: today(&conn),
            },
        )
        .unwrap();
        set_reserve(&conn, 10_000).unwrap();

        let result = calculate(&conn).unwrap();
        let SafeToSpendResult::Complete {
            safe_to_spend_cents,
            ..
        } = result
        else {
            panic!("expected Complete");
        };
        assert_eq!(safe_to_spend_cents, -5_000);
    }

    #[test]
    fn reserve_get_set_round_trips_and_rejects_negative() {
        let conn = test_conn();
        assert_eq!(get_reserve(&conn).unwrap(), 0);

        set_reserve(&conn, 15_000).unwrap();
        assert_eq!(get_reserve(&conn).unwrap(), 15_000);

        assert!(matches!(
            set_reserve(&conn, -1),
            Err(SafeToSpendError::Validation(_))
        ));
    }

    #[test]
    fn safe_to_spend_uses_the_corrected_current_cash_position_after_a_later_reconciliation() {
        // ADR-0009 regression: a reconciliation made after the pay schedule
        // and prior balance were already established must still flow
        // through to Safe to Spend's eligible cash.
        let conn = test_conn();
        let checking = make_account(&conn, "Checking", AccountType::Checking);
        crate::accounting::reconcile_balance(&conn, checking, 100_000, "2026-01-01").unwrap();
        set_schedule(
            &conn,
            &PaySchedule::Biweekly {
                anchor_date: today(&conn),
            },
        )
        .unwrap();

        let before = calculate(&conn).unwrap();
        let SafeToSpendResult::Complete {
            eligible_cash_cents: eligible_before,
            safe_to_spend_cents: safe_before,
            ..
        } = before
        else {
            panic!("expected Complete");
        };
        assert_eq!(eligible_before, 100_000);
        assert_eq!(safe_before, 100_000);

        // The owner checks their bank and it actually shows $750, not $1,000.
        crate::accounting::reconcile_balance(&conn, checking, 75_000, &today(&conn)).unwrap();

        let after = calculate(&conn).unwrap();
        let SafeToSpendResult::Complete {
            eligible_cash_cents: eligible_after,
            safe_to_spend_cents: safe_after,
            ..
        } = after
        else {
            panic!("expected Complete");
        };
        assert_eq!(eligible_after, 75_000);
        assert_eq!(safe_after, 75_000);
    }
}
