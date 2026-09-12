//! Pay cycle (Phase 4B). The schedule itself lives in the existing
//! `settings` key-value table (key `"pay_schedule"`, JSON-encoded) rather
//! than a new table - it is exactly one small structured record, and
//! `settings.rs` already exists for precisely this. Cycle boundaries are
//! pure calendar arithmetic over that schedule; spending/obligations read
//! the real ledger through `accounting::spending_in_range` and
//! `recurring::list` - nothing here reimplements either.
//!
//! User-defined values always outrank inference: `likely_income_series`
//! only ever proposes a detected pattern for the owner to adopt manually;
//! every cycle calculation reads whatever schedule was actually saved, or
//! nothing at all if none was.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::accounting;
use crate::dates;
use crate::db::DbState;
use crate::recurring::{self, RecurringSeries, SeriesStatus};
use crate::settings;

const SETTINGS_KEY: &str = "pay_schedule";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "schedule_type", rename_all = "snake_case")]
pub enum PaySchedule {
    Weekly {
        anchor_date: String,
    },
    Biweekly {
        anchor_date: String,
    },
    Monthly {
        day_of_month: u32,
    },
    Semimonthly {
        day_of_month_1: u32,
        day_of_month_2: u32,
    },
}

#[derive(Debug)]
pub enum PayCycleError {
    Validation(String),
    Db(rusqlite::Error),
    Other(String),
}

impl std::fmt::Display for PayCycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayCycleError::Validation(msg) => write!(f, "{msg}"),
            PayCycleError::Db(e) => write!(f, "database error: {e}"),
            PayCycleError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<rusqlite::Error> for PayCycleError {
    fn from(e: rusqlite::Error) -> Self {
        PayCycleError::Db(e)
    }
}

fn validate(schedule: &PaySchedule) -> Result<(), PayCycleError> {
    match schedule {
        PaySchedule::Weekly { anchor_date } | PaySchedule::Biweekly { anchor_date } => {
            if dates::parse_iso(anchor_date).is_none() {
                return Err(PayCycleError::Validation(
                    "Anchor date must be a valid date.".to_string(),
                ));
            }
        }
        PaySchedule::Monthly { day_of_month } => {
            if !(1..=31).contains(day_of_month) {
                return Err(PayCycleError::Validation(
                    "Day of month must be between 1 and 31.".to_string(),
                ));
            }
        }
        PaySchedule::Semimonthly {
            day_of_month_1,
            day_of_month_2,
        } => {
            if !(1..=31).contains(day_of_month_1) || !(1..=31).contains(day_of_month_2) {
                return Err(PayCycleError::Validation(
                    "Days of month must be between 1 and 31.".to_string(),
                ));
            }
            if day_of_month_1 == day_of_month_2 {
                return Err(PayCycleError::Validation(
                    "The two paydays must be different days.".to_string(),
                ));
            }
        }
    }
    Ok(())
}

pub fn get_schedule(conn: &Connection) -> Result<Option<PaySchedule>, PayCycleError> {
    match settings::get(conn, SETTINGS_KEY)? {
        Some(json) => serde_json::from_str(&json)
            .map(Some)
            .map_err(|e| PayCycleError::Other(format!("Stored pay schedule is corrupted: {e}"))),
        None => Ok(None),
    }
}

pub fn set_schedule(conn: &Connection, schedule: &PaySchedule) -> Result<(), PayCycleError> {
    validate(schedule)?;
    let json = serde_json::to_string(schedule).expect("PaySchedule always serializes");
    settings::set(conn, SETTINGS_KEY, &json)?;
    Ok(())
}

fn period_cycle_start(anchor: &str, period: i64, today: &str) -> String {
    let gap = dates::days_between(anchor, today);
    let periods_elapsed = gap.div_euclid(period);
    dates::add_days(anchor, periods_elapsed * period)
}

fn month_cycle_start(days_of_month: &[u32], today: &str) -> String {
    let this_month: Vec<String> = days_of_month
        .iter()
        .map(|d| dates::day_in_month(today, *d))
        .collect();
    if let Some(latest) = this_month.iter().filter(|d| d.as_str() <= today).max() {
        latest.clone()
    } else {
        let prev_ref = dates::add_months(today, -1);
        days_of_month
            .iter()
            .map(|d| dates::day_in_month(&prev_ref, *d))
            .max()
            .unwrap()
    }
}

fn month_next_payday(days_of_month: &[u32], today: &str) -> String {
    let this_month: Vec<String> = days_of_month
        .iter()
        .map(|d| dates::day_in_month(today, *d))
        .collect();
    if let Some(soonest) = this_month.iter().filter(|d| d.as_str() > today).min() {
        soonest.clone()
    } else {
        let next_ref = dates::add_months(today, 1);
        days_of_month
            .iter()
            .map(|d| dates::day_in_month(&next_ref, *d))
            .min()
            .unwrap()
    }
}

/// The payday that started the cycle `today` falls in.
pub fn current_cycle_start(schedule: &PaySchedule, today: &str) -> String {
    match schedule {
        PaySchedule::Weekly { anchor_date } => period_cycle_start(anchor_date, 7, today),
        PaySchedule::Biweekly { anchor_date } => period_cycle_start(anchor_date, 14, today),
        PaySchedule::Monthly { day_of_month } => month_cycle_start(&[*day_of_month], today),
        PaySchedule::Semimonthly {
            day_of_month_1,
            day_of_month_2,
        } => month_cycle_start(&[*day_of_month_1, *day_of_month_2], today),
    }
}

/// The next payday strictly after `today` (if `today` is itself a payday,
/// that payday already started the current cycle - "next" is the one
/// after it).
pub fn next_payday(schedule: &PaySchedule, today: &str) -> String {
    match schedule {
        PaySchedule::Weekly { anchor_date } => {
            dates::add_days(&period_cycle_start(anchor_date, 7, today), 7)
        }
        PaySchedule::Biweekly { anchor_date } => {
            dates::add_days(&period_cycle_start(anchor_date, 14, today), 14)
        }
        PaySchedule::Monthly { day_of_month } => month_next_payday(&[*day_of_month], today),
        PaySchedule::Semimonthly {
            day_of_month_1,
            day_of_month_2,
        } => month_next_payday(&[*day_of_month_1, *day_of_month_2], today),
    }
}

/// The payday that started the cycle immediately before `cycle_start`.
pub fn prior_cycle_start(schedule: &PaySchedule, cycle_start: &str) -> String {
    match schedule {
        PaySchedule::Weekly { .. } => dates::add_days(cycle_start, -7),
        PaySchedule::Biweekly { .. } => dates::add_days(cycle_start, -14),
        PaySchedule::Monthly { day_of_month } => {
            dates::day_in_month(&dates::add_months(cycle_start, -1), *day_of_month)
        }
        PaySchedule::Semimonthly {
            day_of_month_1,
            day_of_month_2,
        } => {
            let (first, second) = if day_of_month_1 <= day_of_month_2 {
                (*day_of_month_1, *day_of_month_2)
            } else {
                (*day_of_month_2, *day_of_month_1)
            };
            let second_candidate = dates::day_in_month(cycle_start, second);
            if cycle_start == second_candidate {
                dates::day_in_month(cycle_start, first)
            } else {
                dates::day_in_month(&dates::add_months(cycle_start, -1), second)
            }
        }
    }
}

fn today(conn: &Connection) -> rusqlite::Result<String> {
    conn.query_row("SELECT date('now')", [], |row| row.get(0))
}

/// Every recurring series belonging to a non-archived account. A closed
/// account's bills are not real ongoing obligations or income sources, so
/// both `obligations_before` and `likely_income_series` read through this
/// rather than `recurring::list` directly - otherwise a series left over
/// from before an account was archived would silently keep counting.
fn active_account_series(conn: &Connection) -> Result<Vec<RecurringSeries>, PayCycleError> {
    let active_account_ids: std::collections::HashSet<i64> = crate::accounts::list(conn, false)
        .map_err(|e| PayCycleError::Other(e.to_string()))?
        .into_iter()
        .map(|a| a.id)
        .collect();
    let all = recurring::list(conn, None).map_err(|e| PayCycleError::Other(e.to_string()))?;
    Ok(all
        .into_iter()
        .filter(|s| active_account_ids.contains(&s.account_id))
        .collect())
}

pub(crate) fn obligations_before(
    conn: &Connection,
    next_payday: &str,
) -> Result<i64, PayCycleError> {
    let series = active_account_series(conn)?;
    Ok(series
        .iter()
        .filter(|s| s.status == SeriesStatus::Confirmed)
        .filter(|s| s.next_expected_date.as_str() <= next_payday)
        .filter(|s| s.expected_amount_cents < 0)
        .map(|s| s.expected_amount_cents.abs())
        .sum())
}

/// The confirmed recurring bills soonest due, for the Overview's "Upcoming
/// Bills" widget - the same definition of "bill" `obligations_before` sums
/// (confirmed, negative expected amount), just returned as the list itself
/// rather than a total, capped to `limit` for a glanceable screen.
pub(crate) fn upcoming_bills(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<RecurringSeries>, PayCycleError> {
    let mut series: Vec<RecurringSeries> = active_account_series(conn)?
        .into_iter()
        .filter(|s| s.status == SeriesStatus::Confirmed && s.expected_amount_cents < 0)
        .collect();
    series.sort_by(|a, b| a.next_expected_date.cmp(&b.next_expected_date));
    series.truncate(limit);
    Ok(series)
}

/// Confirmed or proposed recurring patterns that look like income (a
/// positive amount, per ADR-0004 - money increasing the account's own
/// balance), for the frontend to offer as a one-click pay-schedule
/// suggestion. Reuses `recurring::detect_candidates`'s existing confidence
/// bar entirely - no separate detection algorithm.
pub fn likely_income_series(conn: &Connection) -> Result<Vec<RecurringSeries>, PayCycleError> {
    let all = active_account_series(conn)?;
    Ok(all
        .into_iter()
        .filter(|s| s.status != SeriesStatus::Rejected)
        .filter(|s| s.expected_amount_cents > 0)
        .collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct CycleSummary {
    pub schedule: PaySchedule,
    pub cycle_start: String,
    pub next_payday: String,
    pub spending_this_cycle_cents: i64,
    pub obligations_before_payday_cents: i64,
    pub prior_cycle_spending_cents: i64,
}

pub fn summarize(conn: &Connection) -> Result<Option<CycleSummary>, PayCycleError> {
    let Some(schedule) = get_schedule(conn)? else {
        return Ok(None);
    };
    let today = today(conn)?;
    let cycle_start = current_cycle_start(&schedule, &today);
    let next = next_payday(&schedule, &today);
    let prior_start = prior_cycle_start(&schedule, &cycle_start);
    let prior_end = dates::add_days(&cycle_start, -1);

    let spending_this_cycle_cents =
        accounting::spending_in_range(conn, None, &cycle_start, &today)?;
    let prior_cycle_spending_cents =
        accounting::spending_in_range(conn, None, &prior_start, &prior_end)?;
    let obligations_before_payday_cents = obligations_before(conn, &next)?;

    Ok(Some(CycleSummary {
        schedule,
        cycle_start,
        next_payday: next,
        spending_this_cycle_cents,
        obligations_before_payday_cents,
        prior_cycle_spending_cents,
    }))
}

#[tauri::command]
pub fn get_pay_schedule(state: State<DbState>) -> Result<Option<PaySchedule>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    get_schedule(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_pay_schedule(state: State<DbState>, schedule: PaySchedule) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    set_schedule(&conn, &schedule).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_pay_cycle_summary(state: State<DbState>) -> Result<Option<CycleSummary>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    summarize(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_likely_income_series(state: State<DbState>) -> Result<Vec<RecurringSeries>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    likely_income_series(&conn).map_err(|e| e.to_string())
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
            },
        )
        .unwrap();
    }

    // --- Schedule persistence ---
    #[test]
    fn get_schedule_returns_none_when_unset_and_round_trips_when_set() {
        let conn = test_conn();
        assert_eq!(get_schedule(&conn).unwrap(), None);

        let schedule = PaySchedule::Biweekly {
            anchor_date: "2026-01-02".to_string(),
        };
        set_schedule(&conn, &schedule).unwrap();
        assert_eq!(get_schedule(&conn).unwrap(), Some(schedule));
    }

    #[test]
    fn rejects_invalid_schedules() {
        let conn = test_conn();
        assert!(matches!(
            set_schedule(
                &conn,
                &PaySchedule::Weekly {
                    anchor_date: "not-a-date".to_string()
                }
            ),
            Err(PayCycleError::Validation(_))
        ));
        assert!(matches!(
            set_schedule(&conn, &PaySchedule::Monthly { day_of_month: 32 }),
            Err(PayCycleError::Validation(_))
        ));
        assert!(matches!(
            set_schedule(
                &conn,
                &PaySchedule::Semimonthly {
                    day_of_month_1: 15,
                    day_of_month_2: 15
                }
            ),
            Err(PayCycleError::Validation(_))
        ));
    }

    // --- Weekly / biweekly cycle math ---
    #[test]
    fn biweekly_cycle_start_and_next_payday_from_an_anchor() {
        let schedule = PaySchedule::Biweekly {
            anchor_date: "2026-01-02".to_string(),
        };
        // Exactly on a payday.
        assert_eq!(current_cycle_start(&schedule, "2026-01-30"), "2026-01-30");
        assert_eq!(next_payday(&schedule, "2026-01-30"), "2026-02-13");
        // Mid-cycle.
        assert_eq!(current_cycle_start(&schedule, "2026-02-05"), "2026-01-30");
        assert_eq!(next_payday(&schedule, "2026-02-05"), "2026-02-13");
    }

    #[test]
    fn weekly_cycle_handles_an_anchor_in_the_future() {
        // Anchor is ahead of "today" - div_euclid must still find the most
        // recent occurrence at or before today, not panic or go forward.
        // 2026-06-03 is 33 days after 2026-05-01 (not an exact multiple of
        // 7), so today lands mid-cycle rather than exactly on a payday.
        let schedule = PaySchedule::Weekly {
            anchor_date: "2026-06-03".to_string(),
        };
        assert_eq!(current_cycle_start(&schedule, "2026-05-01"), "2026-04-29");
        assert_eq!(next_payday(&schedule, "2026-05-01"), "2026-05-06");
    }

    #[test]
    fn prior_cycle_start_steps_back_exactly_one_period() {
        let schedule = PaySchedule::Weekly {
            anchor_date: "2026-01-02".to_string(),
        };
        let start = current_cycle_start(&schedule, "2026-03-10");
        assert_eq!(
            prior_cycle_start(&schedule, &start),
            dates::add_days(&start, -7)
        );
    }

    // --- Monthly cycle math ---
    #[test]
    fn monthly_cycle_start_and_next_payday_including_clamped_short_months() {
        let schedule = PaySchedule::Monthly { day_of_month: 31 };
        assert_eq!(current_cycle_start(&schedule, "2026-02-10"), "2026-01-31");
        assert_eq!(next_payday(&schedule, "2026-02-10"), "2026-02-28");
    }

    #[test]
    fn monthly_prior_cycle_reclamps_using_the_original_schedule_day_not_the_clamped_one() {
        let schedule = PaySchedule::Monthly { day_of_month: 31 };
        // Current cycle starts Feb 28 (clamped from 31). One cycle back
        // must be Jan 31, not Jan 28.
        let start = current_cycle_start(&schedule, "2026-02-28");
        assert_eq!(start, "2026-02-28");
        assert_eq!(prior_cycle_start(&schedule, &start), "2026-01-31");
    }

    // --- Semimonthly cycle math ---
    #[test]
    fn semimonthly_cycle_boundaries_and_month_wraparound() {
        let schedule = PaySchedule::Semimonthly {
            day_of_month_1: 1,
            day_of_month_2: 15,
        };
        assert_eq!(current_cycle_start(&schedule, "2026-08-20"), "2026-08-15");
        assert_eq!(next_payday(&schedule, "2026-08-20"), "2026-09-01");
        assert_eq!(current_cycle_start(&schedule, "2026-08-01"), "2026-08-01");
        assert_eq!(next_payday(&schedule, "2026-08-01"), "2026-08-15");
    }

    #[test]
    fn semimonthly_prior_cycle_alternates_within_and_across_months() {
        let schedule = PaySchedule::Semimonthly {
            day_of_month_1: 1,
            day_of_month_2: 15,
        };
        assert_eq!(prior_cycle_start(&schedule, "2026-08-15"), "2026-08-01");
        assert_eq!(prior_cycle_start(&schedule, "2026-08-01"), "2026-07-15");
    }

    // --- Summary: spending, obligations, prior-cycle comparison ---
    #[test]
    fn summarize_is_none_when_no_schedule_is_configured() {
        let conn = test_conn();
        assert!(summarize(&conn).unwrap().is_none());
    }

    #[test]
    fn summarize_computes_spending_and_prior_cycle_comparison() {
        let conn = test_conn();
        let account = make_account(&conn);
        set_schedule(
            &conn,
            &PaySchedule::Biweekly {
                anchor_date: dates::add_days(&today(&conn).unwrap(), -70), // 5 periods back, lands on a payday today or recently
            },
        )
        .unwrap();

        let summary_before = summarize(&conn).unwrap().unwrap();
        let cycle_start = summary_before.cycle_start.clone();
        let prior_start = prior_cycle_start(
            &PaySchedule::Biweekly {
                anchor_date: dates::add_days(&today(&conn).unwrap(), -70),
            },
            &cycle_start,
        );

        add_txn(&conn, account, &cycle_start, -5000, "This cycle purchase");
        add_txn(
            &conn,
            account,
            &dates::add_days(&prior_start, 1),
            -8000,
            "Prior cycle purchase",
        );

        let summary = summarize(&conn).unwrap().unwrap();
        assert_eq!(summary.spending_this_cycle_cents, 5000);
        assert_eq!(summary.prior_cycle_spending_cents, 8000);
    }

    #[test]
    fn obligations_before_payday_sums_only_confirmed_upcoming_bills() {
        let conn = test_conn();
        let account = make_account(&conn);

        add_txn(&conn, account, "2026-06-05", -999, "Streaming Service");
        add_txn(&conn, account, "2026-07-05", -999, "Streaming Service");
        add_txn(&conn, account, "2026-08-05", -999, "Streaming Service");
        let candidates = recurring::detect_candidates(&conn, account).unwrap();
        let bill = candidates.into_iter().next().unwrap();
        assert_eq!(bill.next_expected_date, "2026-09-04");

        // Proposed (not yet confirmed) must not count as an obligation,
        // even though its date falls before the chosen payday.
        assert_eq!(obligations_before(&conn, "2026-09-10").unwrap(), 0);

        recurring::confirm(&conn, bill.id).unwrap();
        // A payday before the bill's next date: not yet an obligation.
        assert_eq!(obligations_before(&conn, "2026-09-01").unwrap(), 0);
        // A payday on/after the bill's next date: counted, in cents, as a
        // positive obligation despite the bill's own amount being negative.
        assert_eq!(obligations_before(&conn, "2026-09-04").unwrap(), 999);
        assert_eq!(obligations_before(&conn, "2026-09-10").unwrap(), 999);
    }

    #[test]
    fn upcoming_bills_returns_confirmed_bills_soonest_due_first_capped_to_limit() {
        let conn = test_conn();
        let account = make_account(&conn);

        add_txn(&conn, account, "2026-06-05", -999, "Streaming Service");
        add_txn(&conn, account, "2026-07-05", -999, "Streaming Service");
        add_txn(&conn, account, "2026-08-05", -999, "Streaming Service");
        add_txn(&conn, account, "2026-06-10", -5000, "Rent");
        add_txn(&conn, account, "2026-07-10", -5000, "Rent");
        add_txn(&conn, account, "2026-08-10", -5000, "Rent");
        add_txn(&conn, account, "2026-06-01", 250000, "ACME Payroll");
        add_txn(&conn, account, "2026-06-15", 250000, "ACME Payroll");
        add_txn(&conn, account, "2026-06-29", 250000, "ACME Payroll");

        for candidate in recurring::detect_candidates(&conn, account).unwrap() {
            if candidate.expected_amount_cents < 0 {
                recurring::confirm(&conn, candidate.id).unwrap();
            }
            // Payroll is left proposed - income must never appear as a bill.
        }

        let bills = upcoming_bills(&conn, 10).unwrap();
        assert_eq!(
            bills.len(),
            2,
            "only the two confirmed bills, never the proposed income series"
        );
        assert!(bills.iter().all(|b| b.expected_amount_cents < 0));
        assert!(
            bills[0].next_expected_date <= bills[1].next_expected_date,
            "soonest due must come first"
        );

        assert_eq!(
            upcoming_bills(&conn, 1).unwrap().len(),
            1,
            "must respect the cap"
        );
    }

    #[test]
    fn likely_income_series_filters_to_positive_amounts_and_excludes_rejected() {
        let conn = test_conn();
        let account = make_account(&conn);

        add_txn(&conn, account, "2026-06-01", 250000, "ACME Payroll");
        add_txn(&conn, account, "2026-06-15", 250000, "ACME Payroll");
        add_txn(&conn, account, "2026-06-29", 250000, "ACME Payroll");
        add_txn(&conn, account, "2026-06-05", -999, "Streaming Service");
        add_txn(&conn, account, "2026-07-05", -999, "Streaming Service");
        add_txn(&conn, account, "2026-08-05", -999, "Streaming Service");

        let candidates = recurring::detect_candidates(&conn, account).unwrap();
        assert_eq!(
            candidates.len(),
            2,
            "both the payroll and the bill should be detected"
        );

        let income = likely_income_series(&conn).unwrap();
        assert_eq!(income.len(), 1);
        assert_eq!(income[0].merchant, "ACME Payroll");

        recurring::reject(&conn, income[0].id).unwrap();
        assert!(likely_income_series(&conn).unwrap().is_empty());
    }

    #[test]
    fn archived_accounts_never_contribute_obligations_or_income_suggestions() {
        // Regression: a confirmed series left over on an account that was
        // later archived must not keep counting toward either figure -
        // caught during live verification when an archived-account leftover
        // from an earlier phase silently inflated obligations_before.
        let conn = test_conn();
        let closed_account = make_account(&conn);
        add_txn(
            &conn,
            closed_account,
            "2026-06-05",
            -999,
            "Old Streaming Service",
        );
        add_txn(
            &conn,
            closed_account,
            "2026-07-05",
            -999,
            "Old Streaming Service",
        );
        add_txn(
            &conn,
            closed_account,
            "2026-08-05",
            -999,
            "Old Streaming Service",
        );
        let candidates = recurring::detect_candidates(&conn, closed_account).unwrap();
        recurring::confirm(&conn, candidates[0].id).unwrap();
        accounts::set_archived(&conn, closed_account, true).unwrap();

        assert_eq!(obligations_before(&conn, "2099-01-01").unwrap(), 0);

        let active_account = make_account(&conn);
        add_txn(&conn, active_account, "2026-06-01", 250000, "ACME Payroll");
        add_txn(&conn, active_account, "2026-06-15", 250000, "ACME Payroll");
        add_txn(&conn, active_account, "2026-06-29", 250000, "ACME Payroll");
        recurring::detect_candidates(&conn, active_account).unwrap();
        accounts::set_archived(&conn, active_account, true).unwrap();

        assert!(likely_income_series(&conn).unwrap().is_empty());
    }
}
