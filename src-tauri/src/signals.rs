//! Financial Signals (Batch 9, specs/phase-016): deterministic, explainable
//! observations computed entirely from existing, already-tested services -
//! never a second, parallel calculation (section 59). No AI, no adaptive
//! thresholds, no numeric confidence/risk score: the same ledger data plus
//! the same reference date always produces the same signal set (section
//! 32), and every signal names the real numbers behind it (section 29).

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::accounting;
use crate::accounts;
use crate::budgets::{self, BudgetStatus, PaceStatus};
use crate::dates;
use crate::db::DbState;
use crate::forecast;
use crate::insights;
use crate::recurring::{self, SeriesStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalSeverity {
    // Declaration order is ascending severity - sort descending for display.
    Info,
    Attention,
    Important,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalNavigation {
    Insights,
    Budgets,
    Forecast,
    Recurring,
}

#[derive(Debug, Clone, Serialize)]
pub struct Signal {
    pub key: String,
    pub kind: String,
    pub severity: SignalSeverity,
    pub title: String,
    pub evidence: String,
    pub navigation: SignalNavigation,
    /// A category name, series id, or similar - enough for the frontend to
    /// deep-link (section 78-81), never a raw route/internal id contract
    /// beyond a plain string.
    pub navigation_context: Option<String>,
}

/// Combined threshold (section 34): a category (or income shortfall) must
/// clear BOTH a percentage and a dollar floor to signal, so a typical $2
/// coffee habit doubling to $4 never reads as urgent. Chosen from MIDAS's
/// own established amounts (a $50 swing is already a real grocery-trip-sized
/// difference; 25% is comfortably past ordinary week-to-week noise) and
/// shared across every combined-threshold signal below rather than
/// invented per-signal.
const SIGNAL_MIN_PERCENT: i64 = 25;
const SIGNAL_MIN_CENTS: i64 = 5000;

/// A category total is compared against its historical typical *prorated
/// to how much of the month has elapsed* (section 29's own "through this
/// point" framing) - comparing a still-partial month against a full
/// month's typical would make the signal meaningless for the first three
/// weeks of every month.
const MIN_ELAPSED_DAYS_FOR_CATEGORY_SIGNAL: i64 = 3;

/// Net worth decline threshold (section 37: "if implemented, use a
/// meaningful threshold") - $200 and 3%, deliberately smaller than the
/// spending/income combined threshold since net worth already aggregates
/// every account, so a single meaningful expense should still be visible
/// rather than needing to individually clear a $50 bar twice over.
const NET_WORTH_DECLINE_MIN_CENTS: i64 = 20000;
const NET_WORTH_DECLINE_MIN_PERCENT: i64 = 3;

/// The horizon Forecast's own signal uses - matches Forecast's own default
/// (section 61's layout target shows 30 days first).
const FORECAST_SIGNAL_HORIZON_DAYS: i64 = 30;

fn dollars(cents: i64) -> String {
    format!("${}", crate::transactions::format_dollars(cents.abs()))
}

fn today(conn: &Connection) -> rusqlite::Result<String> {
    conn.query_row("SELECT date('now')", [], |row| row.get(0))
}

/// Category spending materially above its own recent-history baseline
/// (section 33 - reuses `budgets::historical_typical`, never a second
/// "normal spending" formula). Uncategorized is excluded (section 74 - that
/// belongs to Review/Data Health, not a Signal).
fn category_spending_signals(conn: &Connection, today: &str) -> Result<Vec<Signal>, String> {
    let month_start = dates::start_of_month(today);
    let days_in_month = dates::days_between(&month_start, &dates::end_of_month(today)) + 1;
    let elapsed_days = dates::days_between(&month_start, today) + 1;
    if elapsed_days < MIN_ELAPSED_DAYS_FOR_CATEGORY_SIGNAL {
        return Ok(Vec::new());
    }

    let spending =
        insights::spending_insights(conn, &month_start, today).map_err(|e| e.to_string())?;
    let mut signals = Vec::new();
    for category in &spending.categories {
        if category.category == "Uncategorized" {
            continue;
        }
        let (typical, _months) = budgets::historical_typical(
            conn,
            &category.category,
            &month_start,
            budgets::DEFAULT_LOOKBACK_MONTHS,
        )
        .map_err(|e| e.to_string())?;
        let Some(typical) = typical else { continue };
        if typical <= 0 {
            continue;
        }
        let prorated_typical = (typical * elapsed_days) / days_in_month;
        if prorated_typical <= 0 {
            continue;
        }
        let excess = category.spent_cents - prorated_typical;
        let percent = (excess * 100) / prorated_typical;
        if excess >= SIGNAL_MIN_CENTS && percent >= SIGNAL_MIN_PERCENT {
            signals.push(Signal {
                key: format!("category_high_spend:{}:{month_start}", category.category),
                kind: "category_high_spend".to_string(),
                severity: SignalSeverity::Attention,
                title: format!("{} running above normal", category.category),
                evidence: format!(
                    "{} is {} this month, compared with a typical {} through this point (+{percent}%).",
                    category.category,
                    dollars(category.spent_cents),
                    dollars(prorated_typical)
                ),
                navigation: SignalNavigation::Insights,
                navigation_context: Some(category.category.clone()),
            });
        }
    }
    Ok(signals)
}

/// Budget pace/over-budget (section 35 - reuses `Budget::status`/`pace`
/// exactly as Budgets computes them, never recalculated). Only the two
/// meaningful states signal; On Track/Slower/Under are never noise.
fn budget_pace_signals(conn: &Connection, today: &str) -> Result<Vec<Signal>, String> {
    let month = dates::start_of_month(today);
    let mut signals = Vec::new();
    for b in budgets::list_for_month(conn, &month).map_err(|e| e.to_string())? {
        if b.status == BudgetStatus::Over {
            let over_by = b.actual_amount_cents - b.effective_amount_cents;
            signals.push(Signal {
                key: format!("budget_over:{}:{month}", b.category),
                kind: "budget_over".to_string(),
                severity: SignalSeverity::Important,
                title: format!("{} budget exceeded", b.category),
                evidence: format!(
                    "{} is over budget by {} this month.",
                    b.category,
                    dollars(over_by)
                ),
                navigation: SignalNavigation::Budgets,
                navigation_context: Some(b.category.clone()),
            });
        } else if let Some(pace) = &b.pace {
            if pace.status == PaceStatus::Faster {
                signals.push(Signal {
                    key: format!("budget_pace_faster:{}:{month}", b.category),
                    kind: "budget_pace_faster".to_string(),
                    severity: SignalSeverity::Attention,
                    title: format!("{} running faster than planned", b.category),
                    evidence: format!(
                        "{} has used {}% of its budget with {}% of the month elapsed.",
                        b.category, pace.percent_used, pace.percent_elapsed
                    ),
                    navigation: SignalNavigation::Budgets,
                    navigation_context: Some(b.category.clone()),
                });
            }
        }
    }
    Ok(signals)
}

/// A recurring price increase (section 39 - reuses `RecurringSeries::
/// is_price_changed`, which Batch 6 already suppresses for `Kind::Bill`
/// variable utilities, so that protection is inherited for free rather
/// than reimplemented).
fn recurring_price_signals(conn: &Connection) -> Result<Vec<Signal>, String> {
    Ok(recurring::list(conn, None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|s| s.status == SeriesStatus::Confirmed && s.is_price_changed)
        .filter_map(|s| {
            let previous = s.previous_typical_amount_cents?;
            let latest = s.last_amount_cents?.abs();
            let delta = latest - previous;
            Some(Signal {
                key: format!("recurring_price_change:{}", s.id),
                kind: "recurring_price_change".to_string(),
                severity: SignalSeverity::Info,
                title: format!("{} price changed", s.merchant),
                evidence: format!(
                    "{} to {} ({}{}/{}).",
                    dollars(previous),
                    dollars(latest),
                    if delta >= 0 { "+" } else { "-" },
                    dollars(delta),
                    "mo"
                ),
                navigation: SignalNavigation::Recurring,
                navigation_context: Some(s.id.to_string()),
            })
        })
        .collect())
}

/// Income materially below the same point last month (section 36 - the
/// exact same-point rule Insights' frontend period model already applies,
/// ported to Rust for this one deterministic reference-date case).
fn income_decline_signal(conn: &Connection, today: &str) -> Result<Option<Signal>, String> {
    let month_start = dates::start_of_month(today);
    let current =
        accounting::income_in_range(conn, &month_start, today).map_err(|e| e.to_string())?;
    let (prev_from, prev_to) = dates::same_point_previous_month(today);
    let previous =
        accounting::income_in_range(conn, &prev_from, &prev_to).map_err(|e| e.to_string())?;
    if previous <= 0 {
        return Ok(None);
    }
    let shortfall = previous - current;
    let percent = (shortfall * 100) / previous;
    if shortfall >= SIGNAL_MIN_CENTS && percent >= SIGNAL_MIN_PERCENT {
        Ok(Some(Signal {
            key: format!("income_decline:{month_start}"),
            kind: "income_decline".to_string(),
            severity: SignalSeverity::Attention,
            title: "Income lower than usual".to_string(),
            evidence: format!(
                "Income is {} this month, compared with {} through the same point last month.",
                dollars(current),
                dollars(previous)
            ),
            navigation: SignalNavigation::Insights,
            navigation_context: Some("income".to_string()),
        }))
    } else {
        Ok(None)
    }
}

/// Projected cash pressure (section 38 - reuses `forecast::project`
/// directly, at Forecast's own default 30-day horizon over every eligible
/// cash account combined). A negative projected balance is always
/// meaningful even with no reserve configured; a configured reserve
/// crossing is equally meaningful and takes the same severity.
fn forecast_signal(conn: &Connection) -> Result<Option<Signal>, String> {
    let cash_ids: Vec<i64> = accounts::list(conn, false)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|a| a.account_type.is_eligible_cash())
        .map(|a| a.id)
        .collect();
    if cash_ids.is_empty() {
        return Ok(None);
    }
    let result = forecast::project(conn, &cash_ids, FORECAST_SIGNAL_HORIZON_DAYS)
        .map_err(|e| e.to_string())?;

    if result.lowest_balance_cents < 0 {
        // `dollars()` always renders a magnitude (every other call site here
        // passes an already-positive difference) - a raw negative balance
        // needs its own explicit, unambiguous phrasing instead (section 69's
        // own example: "Checking is projected below $0 on Sep 19"), or the
        // sign silently disappears and a $903 overdraft reads as a
        // reassuring positive $903.
        return Ok(Some(Signal {
            key: "forecast_negative".to_string(),
            kind: "forecast_negative".to_string(),
            severity: SignalSeverity::Important,
            title: "Cash pressure ahead".to_string(),
            evidence: format!(
                "Projected cash goes {} negative on {}.",
                dollars(result.lowest_balance_cents),
                result.lowest_balance_date
            ),
            navigation: SignalNavigation::Forecast,
            navigation_context: None,
        }));
    }
    if let Some(warning) = &result.reserve_warning {
        return Ok(Some(Signal {
            key: "forecast_below_reserve".to_string(),
            kind: "forecast_below_reserve".to_string(),
            severity: SignalSeverity::Important,
            title: "Cash pressure ahead".to_string(),
            evidence: format!(
                "Projected cash falls below your {} reserve on {}.",
                dollars(warning.reserve_cents),
                warning.date
            ),
            navigation: SignalNavigation::Forecast,
            navigation_context: None,
        }));
    }
    Ok(None)
}

/// Meaningful net-worth decline this month (section 37 - reuses
/// `accounting::net_worth_change` directly).
fn net_worth_decline_signal(conn: &Connection, today: &str) -> Result<Option<Signal>, String> {
    let month_start = dates::start_of_month(today);
    let change =
        accounting::net_worth_change(conn, &month_start, today).map_err(|e| e.to_string())?;
    if change.change_cents >= 0 || change.start_cents <= 0 {
        return Ok(None);
    }
    let decline = -change.change_cents;
    let percent = (decline * 100) / change.start_cents.abs();
    if decline >= NET_WORTH_DECLINE_MIN_CENTS && percent >= NET_WORTH_DECLINE_MIN_PERCENT {
        Ok(Some(Signal {
            key: format!("net_worth_decline:{month_start}"),
            kind: "net_worth_decline".to_string(),
            severity: SignalSeverity::Attention,
            title: "Net worth declined this month".to_string(),
            evidence: format!(
                "Net worth is down {} ({}%) this month.",
                dollars(decline),
                percent
            ),
            navigation: SignalNavigation::Insights,
            navigation_context: Some("networth".to_string()),
        }))
    } else {
        Ok(None)
    }
}

pub fn compute_signals(conn: &Connection) -> Result<Vec<Signal>, String> {
    let today = today(conn).map_err(|e| e.to_string())?;
    let mut signals = Vec::new();
    signals.extend(category_spending_signals(conn, &today)?);
    signals.extend(budget_pace_signals(conn, &today)?);
    signals.extend(recurring_price_signals(conn)?);
    signals.extend(income_decline_signal(conn, &today)?);
    signals.extend(forecast_signal(conn)?);
    signals.extend(net_worth_decline_signal(conn, &today)?);
    signals.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.key.cmp(&b.key)));
    Ok(signals)
}

#[tauri::command]
pub fn get_signals(state: State<DbState>) -> Result<Vec<Signal>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    compute_signals(&conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::budgets::{self, BudgetInput, Classification};
    use crate::recurring::{Cadence, Direction, Kind as RecurringKind, PatternEditInput};
    use crate::transactions::{self, TransactionInput, TransactionType};

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

    fn deposit(conn: &Connection, account_id: i64, date: &str, amount_cents: i64) {
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: date.to_string(),
                posted_date: None,
                amount_cents,
                transaction_type: TransactionType::Income,
                raw_description: "Payroll".to_string(),
                merchant: Some("Payroll".to_string()),
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

    fn spend(conn: &Connection, account_id: i64, date: &str, amount_cents: i64, category: &str) {
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: date.to_string(),
                posted_date: None,
                amount_cents,
                transaction_type: TransactionType::Purchase,
                raw_description: category.to_string(),
                merchant: Some(category.to_string()),
                category: Some(category.to_string()),
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

    fn find<'a>(signals: &'a [Signal], kind: &str) -> Option<&'a Signal> {
        signals.iter().find(|s| s.kind == kind)
    }

    // --- Category high spend (sections 33-34) ---

    #[test]
    fn category_materially_above_prorated_baseline_signals() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        let month_start = crate::dates::start_of_month(&today_str);
        // Two full prior months at $200 each -> typical $200.
        for i in 1..=2 {
            spend(
                &conn,
                checking,
                &crate::dates::add_months(&today_str, -i),
                -20000,
                "Dining",
            );
        }
        // This month: already spent $300, far above even the full $200
        // typical, comfortably clearing both thresholds regardless of how
        // far the month has elapsed.
        spend(
            &conn,
            checking,
            &crate::dates::add_days(&month_start, 2),
            -30000,
            "Dining",
        );

        let signals = compute_signals(&conn).unwrap();
        let signal = find(&signals, "category_high_spend").expect("category signal present");
        assert_eq!(signal.severity, SignalSeverity::Attention);
        assert!(signal.evidence.contains("Dining"));
        assert_eq!(signal.navigation, SignalNavigation::Insights);
        assert_eq!(signal.navigation_context.as_deref(), Some("Dining"));
    }

    #[test]
    fn a_tiny_percentage_only_anomaly_never_signals() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        let month_start = crate::dates::start_of_month(&today_str);
        // Typical $2 coffee habit doubling to $4 - a real +100% but nowhere
        // near the $50 absolute floor (section 33's own worked example).
        for i in 1..=2 {
            spend(
                &conn,
                checking,
                &crate::dates::add_months(&today_str, -i),
                -200,
                "Coffee",
            );
        }
        spend(
            &conn,
            checking,
            &crate::dates::add_days(&month_start, 5),
            -400,
            "Coffee",
        );

        let signals = compute_signals(&conn).unwrap();
        assert!(find(&signals, "category_high_spend").is_none());
    }

    #[test]
    fn a_meaningful_dollar_amount_below_percent_threshold_never_signals() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        let month_start = crate::dates::start_of_month(&today_str);
        // $1,000 typical, spending $1,060 - a real $60 (over the $50 floor)
        // but only +6%, comfortably under the 25% floor.
        for i in 1..=2 {
            spend(
                &conn,
                checking,
                &crate::dates::add_months(&today_str, -i),
                -100000,
                "Groceries",
            );
        }
        spend(
            &conn,
            checking,
            &crate::dates::add_days(&month_start, 27),
            -106000,
            "Groceries",
        );

        let signals = compute_signals(&conn).unwrap();
        assert!(find(&signals, "category_high_spend").is_none());
    }

    // --- Budget pace / over (section 35) ---

    #[test]
    fn an_over_budget_category_signals_as_important() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        let month = crate::dates::start_of_month(&today_str);
        budgets::create(
            &conn,
            &BudgetInput {
                category: "Dining".to_string(),
                month: month.clone(),
                planned_amount_cents: 20000,
                classification: Classification::Flexible,
                rollover_enabled: false,
            },
        )
        .unwrap();
        spend(&conn, checking, &today_str, -25000, "Dining");

        let signals = compute_signals(&conn).unwrap();
        let signal = find(&signals, "budget_over").expect("over-budget signal present");
        assert_eq!(signal.severity, SignalSeverity::Important);
        assert_eq!(signal.navigation, SignalNavigation::Budgets);
    }

    #[test]
    fn on_track_budgets_never_signal() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        let month = crate::dates::start_of_month(&today_str);
        budgets::create(
            &conn,
            &BudgetInput {
                category: "Dining".to_string(),
                month,
                planned_amount_cents: 100_000_000,
                classification: Classification::Flexible,
                rollover_enabled: false,
            },
        )
        .unwrap();
        spend(&conn, checking, &today_str, -1000, "Dining");

        let signals = compute_signals(&conn).unwrap();
        assert!(find(&signals, "budget_over").is_none());
        assert!(find(&signals, "budget_pace_faster").is_none());
    }

    // --- Recurring price change (section 39) ---

    #[test]
    fn a_confirmed_subscription_price_increase_signals_as_info() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        let series = crate::recurring::create_manual(
            &mut conn,
            checking,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Netflix".to_string(),
                payee_id: None,
                kind: RecurringKind::Subscription,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -2299,
                next_expected_date: crate::dates::add_days(&today_str, 10),
                notes: None,
            },
        )
        .unwrap();
        for (date, amount) in [
            (crate::dates::add_months(&today_str, -3), -1799),
            (crate::dates::add_months(&today_str, -2), -1799),
            (crate::dates::add_months(&today_str, -1), -2299),
        ] {
            let txn = transactions::create(
                &conn,
                &TransactionInput {
                    account_id: checking,
                    transaction_date: date,
                    posted_date: None,
                    amount_cents: amount,
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
            conn.execute(
                "UPDATE transactions SET recurring_series_id = ?1 WHERE id = ?2",
                rusqlite::params![series.id, txn.id],
            )
            .unwrap();
        }

        let signals = compute_signals(&conn).unwrap();
        let signal = find(&signals, "recurring_price_change").expect("price-change signal present");
        assert_eq!(signal.severity, SignalSeverity::Info);
        assert!(signal.evidence.contains("17.99"));
        assert!(signal.evidence.contains("22.99"));
    }

    #[test]
    fn a_variable_utility_bill_never_false_triggers_a_price_change_signal() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        let series = crate::recurring::create_manual(
            &mut conn,
            checking,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Electric Co".to_string(),
                payee_id: None,
                kind: RecurringKind::Bill,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -16000,
                next_expected_date: crate::dates::add_days(&today_str, 10),
                notes: None,
            },
        )
        .unwrap();
        for (date, amount) in [
            (crate::dates::add_months(&today_str, -3), -12000),
            (crate::dates::add_months(&today_str, -2), -15000),
            (crate::dates::add_months(&today_str, -1), -20000),
        ] {
            let txn = transactions::create(
                &conn,
                &TransactionInput {
                    account_id: checking,
                    transaction_date: date,
                    posted_date: None,
                    amount_cents: amount,
                    transaction_type: TransactionType::Purchase,
                    raw_description: "Electric Co".to_string(),
                    merchant: Some("Electric Co".to_string()),
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
            conn.execute(
                "UPDATE transactions SET recurring_series_id = ?1 WHERE id = ?2",
                rusqlite::params![series.id, txn.id],
            )
            .unwrap();
        }

        let signals = compute_signals(&conn).unwrap();
        assert!(
            find(&signals, "recurring_price_change").is_none(),
            "Kind::Bill must stay protected from ordinary variable-utility swings (section 39)"
        );
    }

    // --- Income decline (section 36) ---

    #[test]
    fn income_below_the_same_point_last_month_signals() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        let (prev_from, _) = dates::same_point_previous_month(&today_str);
        deposit(&conn, checking, &prev_from, 300000);
        deposit(
            &conn,
            checking,
            &crate::dates::start_of_month(&today_str),
            100000,
        );

        let signals = compute_signals(&conn).unwrap();
        let signal = find(&signals, "income_decline").expect("income decline signal present");
        assert_eq!(signal.severity, SignalSeverity::Attention);
    }

    #[test]
    fn income_at_or_above_last_months_same_point_never_signals() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        let (prev_from, _) = dates::same_point_previous_month(&today_str);
        deposit(&conn, checking, &prev_from, 100000);
        deposit(
            &conn,
            checking,
            &crate::dates::start_of_month(&today_str),
            300000,
        );

        let signals = compute_signals(&conn).unwrap();
        assert!(find(&signals, "income_decline").is_none());
    }

    // --- Forecast (section 38) ---

    #[test]
    fn a_negative_forecast_ending_balance_signals_as_important() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, "2020-01-01", 5000);
        let bill_date = crate::dates::add_days(&today(&conn), 5);
        crate::recurring::create_manual(
            &mut conn,
            checking,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Rent".to_string(),
                payee_id: None,
                kind: RecurringKind::Bill,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -800000,
                next_expected_date: bill_date,
                notes: None,
            },
        )
        .unwrap();

        let signals = compute_signals(&conn).unwrap();
        let signal = find(&signals, "forecast_negative").expect("negative-forecast signal present");
        assert_eq!(signal.severity, SignalSeverity::Important);
        assert_eq!(signal.navigation, SignalNavigation::Forecast);
        // A real bug found via native verification: `dollars()` always
        // renders a magnitude, so naively formatting the raw negative
        // balance silently dropped the sign and a real overdraft read as a
        // reassuring positive number. The evidence text must say "negative"
        // explicitly rather than just a bare dollar figure.
        assert!(
            signal.evidence.contains("negative"),
            "evidence must not silently drop the sign of a negative balance: {}",
            signal.evidence
        );
        assert!(signal.evidence.contains("7950.00"));
    }

    #[test]
    fn a_healthy_forecast_never_signals() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        deposit(&conn, checking, "2020-01-01", 10_000_000);

        let signals = compute_signals(&conn).unwrap();
        assert!(find(&signals, "forecast_negative").is_none());
        assert!(find(&signals, "forecast_below_reserve").is_none());
    }

    // --- Net worth decline (section 37) ---

    #[test]
    fn a_meaningful_net_worth_decline_signals() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        deposit(&conn, checking, "2020-01-01", 1_000_000); // $10,000 baseline
        spend(&conn, checking, &today_str, -100000, "Emergency"); // -$1,000 (10%, over both thresholds)

        let signals = compute_signals(&conn).unwrap();
        let signal = find(&signals, "net_worth_decline").expect("net worth decline signal present");
        assert_eq!(signal.severity, SignalSeverity::Attention);
    }

    #[test]
    fn a_tiny_net_worth_fluctuation_never_signals() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        deposit(&conn, checking, "2020-01-01", 1_000_000);
        spend(&conn, checking, &today_str, -500, "Coffee"); // $5, negligible

        let signals = compute_signals(&conn).unwrap();
        assert!(find(&signals, "net_worth_decline").is_none());
    }

    // --- Determinism (section 32, 68) ---

    #[test]
    fn identical_inputs_produce_an_identical_signal_set_with_stable_keys() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        let month = crate::dates::start_of_month(&today_str);
        budgets::create(
            &conn,
            &BudgetInput {
                category: "Dining".to_string(),
                month,
                planned_amount_cents: 20000,
                classification: Classification::Flexible,
                rollover_enabled: false,
            },
        )
        .unwrap();
        spend(&conn, checking, &today_str, -25000, "Dining");

        let first = compute_signals(&conn).unwrap();
        let second = compute_signals(&conn).unwrap();
        assert_eq!(
            first.iter().map(|s| s.key.clone()).collect::<Vec<_>>(),
            second.iter().map(|s| s.key.clone()).collect::<Vec<_>>(),
        );
    }
}
