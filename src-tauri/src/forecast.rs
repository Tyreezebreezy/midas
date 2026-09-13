//! Deterministic near-term cash forecast (Batch 8, specs/phase-015).
//!
//! A forecast event is a *projection*, never a ledger transaction (section
//! 36): this module never writes anything, so opening Forecast repeatedly
//! can never create or duplicate a database record (section 83 -
//! idempotence falls out for free from being purely a read/compute path).
//! Its only inputs are already-real, already-deterministic facts: an
//! eligible cash account's current live balance, and Recurring
//! Intelligence's own confirmed, not-yet-posted expected occurrences
//! (`recurring_occurrences::upcoming_in_range`, reused as-is rather than a
//! second recurring-forecast model). Recurring transfers are deliberately
//! excluded (section 45's own escape hatch: MIDAS has no reliable way to
//! associate a transfer-direction series with the specific other account or
//! series it pairs with, so guessing would misrepresent the balance rather
//! than just omitting an edge case).

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::accounts;
use crate::dates;
use crate::db::DbState;
use crate::recurring;
use crate::recurring_occurrences::{self, OccurrenceStatus};
use crate::safe_to_spend;

/// Plain-language distinction (section 42 - never a fake numeric confidence
/// score): `Expected` for a series whose amount has been consistent,
/// `Estimated` when it's variable enough that the projected amount is a
/// median rather than a promise. Reuses the exact variability signal price-
/// change detection already computes (`RecurringSeries::median_amount_cents`
/// and the same narrow `amount_tolerance` window) - never a second,
/// divergent definition of "variable."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    Expected,
    Estimated,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectedEvent {
    pub date: String,
    pub label: String,
    /// Signed exactly like a ledger transaction on this account would be
    /// (negative = cash out, positive = cash in) - never a magnitude the
    /// frontend has to reinterpret.
    pub amount_cents: i64,
    pub certainty: Certainty,
    pub projected_balance_cents: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReserveWarning {
    pub reserve_cents: i64,
    pub date: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForecastResult {
    pub current_balance_cents: i64,
    pub events: Vec<ProjectedEvent>,
    pub ending_balance_cents: i64,
    pub lowest_balance_cents: i64,
    pub lowest_balance_date: String,
    /// Reuses Safe to Spend's own existing reserve setting (section 50) -
    /// `None` whenever no reserve is configured (a $0 reserve is treated as
    /// "not set," matching how Safe to Spend already treats it) or the
    /// projection never actually crosses it.
    pub reserve_warning: Option<ReserveWarning>,
}

#[derive(Debug)]
pub enum ForecastError {
    Db(rusqlite::Error),
    Validation(String),
}

impl std::fmt::Display for ForecastError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForecastError::Db(e) => write!(f, "database error: {e}"),
            ForecastError::Validation(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<rusqlite::Error> for ForecastError {
    fn from(e: rusqlite::Error) -> Self {
        ForecastError::Db(e)
    }
}

impl From<recurring::RecurringError> for ForecastError {
    fn from(e: recurring::RecurringError) -> Self {
        match e {
            recurring::RecurringError::Db(e) => ForecastError::Db(e),
            other => ForecastError::Validation(other.to_string()),
        }
    }
}

fn today(conn: &Connection) -> rusqlite::Result<String> {
    conn.query_row("SELECT date('now')", [], |row| row.get(0))
}

/// Whether this series' own linked-transaction history is variable enough
/// that its `expected_amount_cents` shouldn't be projected as an exact
/// promise - the same amount-tolerance window `recurring::hydrate` already
/// uses to flag a price change. Fewer than two linked transactions (no
/// min/max/median yet) is treated as not-yet-variable, since there is
/// nothing to compare against - `expected_amount_cents` is the only figure
/// available either way.
fn is_variable(series: &recurring::RecurringSeries) -> bool {
    match (
        series.min_amount_cents,
        series.max_amount_cents,
        series.median_amount_cents,
    ) {
        (Some(min), Some(max), Some(median)) => (max - min) > recurring::amount_tolerance(median),
        _ => false,
    }
}

struct RawEvent {
    date: String,
    merchant: String,
    series_id: i64,
}

/// Projects `account_ids`' combined current cash balance forward through
/// every confirmed, not-yet-posted expected occurrence due in the next
/// `horizon_days` (clamped to 90 - `recurring_occurrences`' own generation
/// horizon, so a request beyond it could never see a bill that hasn't been
/// generated yet and would silently understate what's coming). Every
/// `account_ids` entry must be a genuinely liquid cash account (section 37 -
/// "do not treat available credit as cash"); pass a single id for the
/// per-account view or every eligible cash account's id for the combined
/// view (section 39) - both share this one function.
pub fn project(
    conn: &Connection,
    account_ids: &[i64],
    horizon_days: i64,
) -> Result<ForecastResult, ForecastError> {
    if account_ids.is_empty() {
        return Err(ForecastError::Validation(
            "At least one account is required to forecast.".to_string(),
        ));
    }
    let horizon_days = horizon_days.clamp(1, 90);
    let today_str = today(conn)?;
    let horizon = dates::add_days(&today_str, horizon_days);

    let mut current_balance_cents = 0i64;
    for &id in account_ids {
        let account = accounts::get(conn, id)
            .map_err(|e| ForecastError::Validation(e.to_string()))?
            .ok_or_else(|| ForecastError::Validation(format!("Account {id} not found.")))?;
        if !account.account_type.is_eligible_cash() {
            return Err(ForecastError::Validation(format!(
                "\"{}\" isn't a liquid cash account, so it can't be forecasted.",
                account.name
            )));
        }
        current_balance_cents += account.balance_cents;
    }

    let mut raw_events: Vec<RawEvent> = Vec::new();
    for &account_id in account_ids {
        let occurrences =
            recurring_occurrences::upcoming_in_range(conn, &today_str, &horizon, Some(account_id))?;
        for occ in occurrences {
            // Only genuinely still-open expectations (section 40): never a
            // matched occurrence (its real transaction is already truth in
            // the current balance - section 43), never missed/skipped/
            // needing review, never a rejected/inactive series (excluded
            // upstream by `upcoming_in_range` itself).
            if !matches!(
                occ.status,
                OccurrenceStatus::Upcoming | OccurrenceStatus::Due
            ) {
                continue;
            }
            // A recurring transfer's other leg isn't reliably known - see
            // the module doc comment. Direction is denormalized onto the
            // occurrence as a plain string (the same convention
            // `recurring::Direction::as_sql_str` writes).
            if occ.direction == "transfer" {
                continue;
            }
            raw_events.push(RawEvent {
                date: occ.expected_date,
                merchant: occ.merchant,
                series_id: occ.series_id,
            });
        }
    }
    // Stable, deterministic ordering regardless of how many accounts were
    // merged in (section 44) - date first, then merchant name as the same
    // tiebreak `upcoming_in_range` itself already sorts by.
    raw_events.sort_by(|a, b| {
        (a.date.as_str(), a.merchant.as_str()).cmp(&(b.date.as_str(), b.merchant.as_str()))
    });

    let mut balance = current_balance_cents;
    let mut events = Vec::with_capacity(raw_events.len());
    for raw in raw_events {
        let series = recurring::get(conn, raw.series_id)?.ok_or_else(|| {
            ForecastError::Validation("Recurring pattern no longer exists.".to_string())
        })?;
        let income = series.direction == recurring::Direction::Income;
        let (amount_cents, certainty) = if is_variable(&series) {
            let magnitude = series.median_amount_cents.unwrap();
            (
                if income { magnitude } else { -magnitude },
                Certainty::Estimated,
            )
        } else {
            (series.expected_amount_cents, Certainty::Expected)
        };
        balance += amount_cents;
        events.push(ProjectedEvent {
            date: raw.date,
            label: raw.merchant,
            amount_cents,
            certainty,
            projected_balance_cents: balance,
        });
    }

    let mut lowest_balance_cents = current_balance_cents;
    let mut lowest_balance_date = today_str.clone();
    for event in &events {
        if event.projected_balance_cents < lowest_balance_cents {
            lowest_balance_cents = event.projected_balance_cents;
            lowest_balance_date = event.date.clone();
        }
    }

    let reserve_cents =
        safe_to_spend::get_reserve(conn).map_err(|e| ForecastError::Validation(e.to_string()))?;
    let reserve_warning = if reserve_cents > 0 {
        if current_balance_cents < reserve_cents {
            Some(ReserveWarning {
                reserve_cents,
                date: today_str.clone(),
            })
        } else {
            events
                .iter()
                .find(|e| e.projected_balance_cents < reserve_cents)
                .map(|e| ReserveWarning {
                    reserve_cents,
                    date: e.date.clone(),
                })
        }
    } else {
        None
    };

    Ok(ForecastResult {
        current_balance_cents,
        ending_balance_cents: events
            .last()
            .map(|e| e.projected_balance_cents)
            .unwrap_or(current_balance_cents),
        events,
        lowest_balance_cents,
        lowest_balance_date,
        reserve_warning,
    })
}

#[tauri::command]
pub fn get_forecast(
    state: State<DbState>,
    account_ids: Vec<i64>,
    horizon_days: i64,
) -> Result<ForecastResult, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    project(&conn, &account_ids, horizon_days).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::recurring::{Cadence, Direction, Kind, PatternEditInput};
    use crate::recurring_occurrences;
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

    fn today(conn: &Connection) -> String {
        conn.query_row("SELECT date('now')", [], |r| r.get(0))
            .unwrap()
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

    fn deposit(conn: &Connection, account_id: i64, amount_cents: i64) {
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: "2020-01-01".to_string(),
                posted_date: None,
                amount_cents,
                transaction_type: TransactionType::Income,
                raw_description: "Seed balance".to_string(),
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

    #[allow(clippy::too_many_arguments)]
    fn make_series(
        conn: &mut Connection,
        account_id: i64,
        direction: Direction,
        merchant: &str,
        amount_cents: i64,
        next_date: &str,
        cadence: Cadence,
    ) -> i64 {
        let series = crate::recurring::create_manual(
            conn,
            account_id,
            direction,
            &PatternEditInput {
                merchant: merchant.to_string(),
                payee_id: None,
                kind: Kind::Other,
                category: None,
                cadence,
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

    // A monthly cadence with three prior linked amounts differing enough to
    // count as "variable" under `amount_tolerance` - min/max/median then
    // come from real linked history rather than being `None`.
    fn make_variable_bill(conn: &mut Connection, account_id: i64, next_date: &str) -> i64 {
        let series_id = make_series(
            conn,
            account_id,
            Direction::Expense,
            "Electric",
            -20000,
            next_date,
            Cadence::Monthly,
        );

        let today = today(conn);
        let d1 = crate::dates::add_months(&today, -3);
        let d2 = crate::dates::add_months(&today, -2);
        let d3 = crate::dates::add_months(&today, -1);
        for (date, amount) in [
            (d1.as_str(), -10000i64),
            (d2.as_str(), -15000),
            (d3.as_str(), -20000),
        ] {
            let txn = transactions::create(
                conn,
                &TransactionInput {
                    account_id,
                    transaction_date: date.to_string(),
                    posted_date: None,
                    amount_cents: amount,
                    transaction_type: TransactionType::Purchase,
                    raw_description: "Electric".to_string(),
                    merchant: Some("Electric".to_string()),
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
            // `create_manual` doesn't retroactively link matching history
            // the way `confirm` does - link these directly, exactly as
            // Batch 6's `link_matching_history` would have.
            conn.execute(
                "UPDATE transactions SET recurring_series_id = ?1 WHERE id = ?2",
                rusqlite::params![series_id, txn.id],
            )
            .unwrap();
        }
        series_id
    }

    // --- Horizons ---

    #[test]
    fn projects_7_14_and_30_day_horizons_deterministically() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 300_000);
        let next = crate::dates::add_days(&today(&conn), 10);
        make_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Netflix",
            -1599,
            &next,
            Cadence::Monthly,
        );

        let seven = project(&conn, &[checking], 7).unwrap();
        assert_eq!(
            seven.events.len(),
            0,
            "the bill is 10 days out, outside a 7-day horizon"
        );
        assert_eq!(seven.ending_balance_cents, 300_000);

        let fourteen = project(&conn, &[checking], 14).unwrap();
        assert_eq!(fourteen.events.len(), 1);
        assert_eq!(fourteen.ending_balance_cents, 300_000 - 1599);

        let thirty = project(&conn, &[checking], 30).unwrap();
        assert_eq!(
            thirty.events.len(),
            1,
            "still only the one occurrence within 30 days"
        );
    }

    // --- Fixed vs. variable amounts (sections 41/42) ---

    #[test]
    fn a_consistent_recurring_expense_is_projected_as_expected_at_its_exact_amount() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 300_000);
        let next = crate::dates::add_days(&today(&conn), 5);
        make_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Netflix",
            -1599,
            &next,
            Cadence::Monthly,
        );

        let result = project(&conn, &[checking], 14).unwrap();
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].certainty, Certainty::Expected);
        assert_eq!(result.events[0].amount_cents, -1599);
    }

    #[test]
    fn a_variable_bill_is_projected_as_estimated_using_the_median() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 300_000);
        let next = crate::dates::add_days(&today(&conn), 5);
        make_variable_bill(&mut conn, checking, &next);

        let result = project(&conn, &[checking], 14).unwrap();
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].certainty, Certainty::Estimated);
        assert_eq!(
            result.events[0].amount_cents, -15000,
            "the median of 100/150/200, not the raw expected_amount_cents"
        );
    }

    #[test]
    fn recurring_income_increases_the_projected_balance() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 100_000);
        let next = crate::dates::add_days(&today(&conn), 5);
        make_series(
            &mut conn,
            checking,
            Direction::Income,
            "Employer Payroll",
            190000,
            &next,
            Cadence::Biweekly,
        );

        let result = project(&conn, &[checking], 14).unwrap();
        assert_eq!(result.events[0].amount_cents, 190000);
        assert_eq!(result.ending_balance_cents, 100_000 + 190000);
    }

    // --- Exclusions (section 40) ---

    #[test]
    fn a_skipped_occurrence_is_never_projected() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 100_000);
        let next = crate::dates::add_days(&today(&conn), 5);
        let series_id = make_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Gym",
            -3000,
            &next,
            Cadence::Monthly,
        );
        let occurrences = recurring_occurrences::list_for_series(&conn, series_id).unwrap();
        recurring_occurrences::skip_occurrence(&conn, occurrences[0].id).unwrap();

        let result = project(&conn, &[checking], 14).unwrap();
        assert!(result.events.is_empty());
        assert_eq!(result.ending_balance_cents, 100_000);
    }

    #[test]
    fn a_matched_occurrence_is_never_double_projected_since_its_real_transaction_is_already_truth()
    {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 100_000);
        let next = crate::dates::add_days(&today(&conn), 5);
        make_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Netflix",
            -1599,
            &next,
            Cadence::Monthly,
        );

        // The bill already posted for real, and matched its occurrence.
        transactions::create(
            &conn,
            &TransactionInput {
                account_id: checking,
                transaction_date: next.clone(),
                posted_date: None,
                amount_cents: -1599,
                transaction_type: TransactionType::Purchase,
                raw_description: "Netflix".to_string(),
                merchant: Some("Netflix".to_string()),
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

        // The real balance already reflects the charge.
        let result = project(&conn, &[checking], 14).unwrap();
        assert!(
            result.events.is_empty(),
            "a matched occurrence must never be projected again"
        );
        assert_eq!(result.current_balance_cents, 100_000 - 1599);
        assert_eq!(result.ending_balance_cents, 100_000 - 1599);
    }

    #[test]
    fn an_inactive_pattern_contributes_no_projected_events() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 100_000);
        let next = crate::dates::add_days(&today(&conn), 5);
        let series_id = make_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Gym",
            -3000,
            &next,
            Cadence::Monthly,
        );
        crate::recurring::set_inactive(&conn, series_id).unwrap();

        let result = project(&conn, &[checking], 14).unwrap();
        assert!(result.events.is_empty());
    }

    #[test]
    fn an_unconfirmed_proposed_pattern_never_contributes_a_projected_event() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 100_000);
        let next = crate::dates::add_days(&today(&conn), 5);
        // detect_candidates would leave this Proposed - simulate directly
        // via a raw insert rather than needing 3 real historical occurrences.
        conn.execute(
            "INSERT INTO recurring_series (account_id, merchant, direction, kind, cadence, expected_amount_cents, next_expected_date, status)
             VALUES (?1, 'Maybe Bill', 'expense', 'other', 'monthly', -2000, ?2, 'proposed')",
            rusqlite::params![checking, next],
        )
        .unwrap();

        let result = project(&conn, &[checking], 14).unwrap();
        assert!(
            result.events.is_empty(),
            "a proposed (unconfirmed) series must never generate occurrences"
        );
    }

    #[test]
    fn a_recurring_transfer_is_excluded_since_its_other_leg_is_not_reliably_known() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 100_000);
        let next = crate::dates::add_days(&today(&conn), 5);
        make_series(
            &mut conn,
            checking,
            Direction::Transfer,
            "Savings Transfer",
            -20000,
            &next,
            Cadence::Monthly,
        );

        let result = project(&conn, &[checking], 14).unwrap();
        assert!(result.events.is_empty());
        assert_eq!(result.ending_balance_cents, 100_000);
    }

    #[test]
    fn a_card_scoped_subscription_never_drains_a_checking_forecast() {
        // Section 46: Netflix charged to a credit card increases the card's
        // liability, never checking cash directly - here that's simply a
        // consequence of scoping occurrences to the checking account_id, so
        // a series that belongs to the card never appears at all.
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let card = make_account(&conn, AccountType::CreditCard);
        deposit(&conn, checking, 100_000);
        let next = crate::dates::add_days(&today(&conn), 5);
        make_series(
            &mut conn,
            card,
            Direction::Expense,
            "Netflix",
            1599,
            &next,
            Cadence::Monthly,
        );

        let result = project(&conn, &[checking], 14).unwrap();
        assert!(result.events.is_empty());
        assert_eq!(result.ending_balance_cents, 100_000);
    }

    #[test]
    fn forecasting_a_non_cash_account_is_refused() {
        let conn = test_conn();
        let card = make_account(&conn, AccountType::CreditCard);
        let err = project(&conn, &[card], 14).unwrap_err();
        assert!(matches!(err, ForecastError::Validation(_)));
    }

    // --- Ordering, lowest balance, idempotence ---

    #[test]
    fn same_day_events_are_ordered_deterministically_by_merchant_name() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 100_000);
        let next = crate::dates::add_days(&today(&conn), 5);
        make_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Zeta Bill",
            -1000,
            &next,
            Cadence::Monthly,
        );
        make_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Alpha Bill",
            -2000,
            &next,
            Cadence::Monthly,
        );

        let first = project(&conn, &[checking], 14).unwrap();
        let second = project(&conn, &[checking], 14).unwrap();
        assert_eq!(first.events.len(), 2);
        assert_eq!(
            first.events[0].label, "Alpha Bill",
            "alphabetical tiebreak on the same date"
        );
        assert_eq!(first.events[1].label, "Zeta Bill");
        assert_eq!(
            first
                .events
                .iter()
                .map(|e| e.projected_balance_cents)
                .collect::<Vec<_>>(),
            second
                .events
                .iter()
                .map(|e| e.projected_balance_cents)
                .collect::<Vec<_>>(),
            "restart/idempotence: repeated calls must produce identical output"
        );
    }

    #[test]
    fn lowest_projected_balance_and_its_date_are_reported() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 100_000);
        let today_str = today(&conn);
        let bill_date = crate::dates::add_days(&today_str, 5);
        let payday = crate::dates::add_days(&today_str, 10);
        make_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Rent",
            -80000,
            &bill_date,
            Cadence::Monthly,
        );
        make_series(
            &mut conn,
            checking,
            Direction::Income,
            "Payroll",
            150000,
            &payday,
            Cadence::Biweekly,
        );

        let result = project(&conn, &[checking], 14).unwrap();
        assert_eq!(result.lowest_balance_cents, 100_000 - 80000);
        assert_eq!(result.lowest_balance_date, bill_date);
        assert_eq!(result.ending_balance_cents, 100_000 - 80000 + 150000);
    }

    #[test]
    fn lowest_balance_is_the_current_balance_when_nothing_ever_dips_below_it() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 100_000);
        let payday = crate::dates::add_days(&today(&conn), 5);
        make_series(
            &mut conn,
            checking,
            Direction::Income,
            "Payroll",
            150000,
            &payday,
            Cadence::Biweekly,
        );

        let result = project(&conn, &[checking], 14).unwrap();
        assert_eq!(result.lowest_balance_cents, 100_000);
        assert_eq!(result.lowest_balance_date, today(&conn));
    }

    // --- Reserve warning (section 51) ---

    #[test]
    fn a_reserve_crossing_is_reported_as_a_warning_with_its_date() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 60_000);
        crate::safe_to_spend::set_reserve(&conn, 50_000).unwrap();
        let bill_date = crate::dates::add_days(&today(&conn), 5);
        make_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Rent",
            -20000,
            &bill_date,
            Cadence::Monthly,
        );

        let result = project(&conn, &[checking], 14).unwrap();
        let warning = result
            .reserve_warning
            .expect("balance falls below the reserve");
        assert_eq!(warning.reserve_cents, 50_000);
        assert_eq!(warning.date, bill_date);
    }

    #[test]
    fn no_reserve_configured_means_no_warning_even_when_balance_goes_negative() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, 10_000);
        let bill_date = crate::dates::add_days(&today(&conn), 5);
        make_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Rent",
            -20000,
            &bill_date,
            Cadence::Monthly,
        );

        let result = project(&conn, &[checking], 14).unwrap();
        assert_eq!(result.reserve_warning, None);
    }

    // --- Combined multi-account view (section 39) ---

    #[test]
    fn a_combined_forecast_sums_balances_and_merges_events_across_accounts() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let savings = make_account(&conn, AccountType::Savings);
        deposit(&conn, checking, 50_000);
        deposit(&conn, savings, 200_000);
        let next = crate::dates::add_days(&today(&conn), 5);
        make_series(
            &mut conn,
            checking,
            Direction::Expense,
            "Rent",
            -80000,
            &next,
            Cadence::Monthly,
        );

        let combined = project(&conn, &[checking, savings], 14).unwrap();
        assert_eq!(combined.current_balance_cents, 250_000);
        assert_eq!(combined.events.len(), 1);
        assert_eq!(combined.ending_balance_cents, 250_000 - 80000);
    }
}
