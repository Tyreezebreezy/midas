//! Unified Review Center (Batch 9, specs/phase-016). One user-facing
//! destination aggregates every "needs a decision" fact across MIDAS,
//! rather than a separate inbox per domain (section 5). Two kinds of
//! source feed it, deliberately not unified into one storage model
//! (section 61's hybrid design):
//!
//!  - Genuinely persisted decisions (`needs_review_items`, Phase 2) - facts
//!    with no other durable home (a suspected-duplicate import row, an
//!    ambiguous recurring match, a payee-merge conflict). These need
//!    explicit resolve/dismiss because nothing else remembers "this was
//!    flagged."
//!  - Derived facts already durably tracked by their own domain
//!    (`recurring_series.status`/`is_possibly_missed`,
//!    `reconciliations.status`, an uncategorized transaction's own
//!    `category IS NULL`) - recomputed on every read, so they can never go
//!    stale (section 11) and never need a second, redundant persisted copy
//!    (section 60).
//!
//! A real stale-item bug was found during reconnaissance: an
//! ambiguous-recurring-match `needs_review_items` row only ever closed
//! because the one dedicated resolver UI happened to call `resolve()` as a
//! side effect of its own button - resolving the ambiguity through any
//! other path would have left it stranded open forever. `reconcile_stale_
//! persisted_items` fixes this generally, by checking the item's own live
//! underlying fact (whether any of its candidate occurrences are still
//! actually `needs_review`) rather than trusting the row's stored status.
//!
//! A second real bug was found during native verification: every derived
//! source below is scoped to non-archived accounts (via
//! `pay_cycle::active_account_series` for recurring, a direct join for
//! uncategorized/reconciliation). Section 56/58's "never exclude an
//! archived account's real history from Insights" is about *analytics*
//! (a past fact stays a real fact); it does not extend to the *active*
//! review queue, which exists to ask the user to act *now* - asking them to
//! categorize a transaction or fix a reconciliation on an account they
//! already closed is exactly the kind of stale-test-account noise Batch 8
//! also found, just in a different shape (reconciliation records and
//! recurring series, not transactions).

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::State;

use crate::db::DbState;
use crate::needs_review::{self, NeedsReviewItem, ResolutionAction};
use crate::pay_cycle;
use crate::recurring::SeriesStatus;
use crate::transactions::{self, TransactionInput};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPriority {
    // Derived `Ord` sorts Low < Normal < High - callers sort descending.
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDomain {
    Transactions,
    Imports,
    Recurring,
    Reconciliation,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewItem {
    /// Stable across requests (section 68's identity requirement, applied
    /// to Review too) - `"persisted:<id>"` for a real `needs_review_items`
    /// row, `"<kind>:<natural-id>"` for a derived one. Never a random UUID.
    pub key: String,
    pub domain: ReviewDomain,
    pub priority: ReviewPriority,
    pub kind: String,
    pub title: String,
    pub evidence: String,
    /// Deterministic sort key (section 9) - the fact's own date, not
    /// "when this query ran."
    pub sort_date: String,
    pub needs_review_item_id: Option<i64>,
    pub transaction_id: Option<i64>,
    pub account_id: Option<i64>,
    pub series_id: Option<i64>,
    pub occurrence_ids: Vec<i64>,
    pub reconciliation_id: Option<i64>,
    pub duplicate_of_transaction_id: Option<i64>,
}

fn dollars(cents: i64) -> String {
    format!("${}", transactions::format_dollars(cents.abs()))
}

// ---------- Persisted items (needs_review_items) ----------

fn occurrence_is_needs_review(conn: &Connection, occurrence_id: i64) -> bool {
    conn.query_row(
        "SELECT status = 'needs_review' FROM recurring_occurrences WHERE id = ?1",
        params![occurrence_id],
        |row| row.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

fn json_kind(item: &NeedsReviewItem) -> Option<serde_json::Value> {
    item.proposed_resolution_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
}

/// Section 11: closes any open ambiguous-recurring-match item whose
/// candidate occurrences have all already left `needs_review` status
/// through some other path, so it can never stay wrong.
fn reconcile_stale_persisted_items(conn: &Connection) -> rusqlite::Result<()> {
    for item in needs_review::list(conn, true)? {
        if item.issue_type != "other" {
            continue;
        }
        let Some(json) = json_kind(&item) else {
            continue;
        };
        if json.get("kind").and_then(|k| k.as_str()) != Some("ambiguous_recurring_match") {
            continue;
        }
        let occurrence_ids: Vec<i64> = json
            .get("occurrence_ids")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
            .unwrap_or_default();
        let still_ambiguous = occurrence_ids
            .iter()
            .any(|id| occurrence_is_needs_review(conn, *id));
        if !still_ambiguous {
            let _ = needs_review::resolve(conn, item.id, ResolutionAction::Resolved);
        }
    }
    Ok(())
}

fn map_persisted_item(item: NeedsReviewItem) -> ReviewItem {
    let base = ReviewItem {
        key: format!("persisted:{}", item.id),
        domain: ReviewDomain::Imports,
        priority: ReviewPriority::High,
        kind: item.issue_type.clone(),
        title: String::new(),
        evidence: item.detected_reason.clone(),
        sort_date: item.created_at.clone(),
        needs_review_item_id: Some(item.id),
        transaction_id: item.transaction_id,
        account_id: None,
        series_id: None,
        occurrence_ids: Vec::new(),
        reconciliation_id: None,
        duplicate_of_transaction_id: None,
    };

    match item.issue_type.as_str() {
        "malformed_row" => ReviewItem {
            title: "Import row could not be used".to_string(),
            ..base
        },
        "suspected_duplicate" => {
            let duplicate_of_transaction_id = json_kind(&item).and_then(|v| {
                v.get("duplicate_of_transaction_id")
                    .and_then(|x| x.as_i64())
            });
            ReviewItem {
                title: "Possible duplicate transaction".to_string(),
                duplicate_of_transaction_id,
                ..base
            }
        }
        _ => {
            let json = json_kind(&item);
            let kind = json
                .as_ref()
                .and_then(|v| v.get("kind"))
                .and_then(|k| k.as_str())
                .unwrap_or("other")
                .to_string();
            match kind.as_str() {
                "ambiguous_recurring_match" => {
                    let occurrence_ids: Vec<i64> = json
                        .as_ref()
                        .and_then(|v| v.get("occurrence_ids"))
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
                        .unwrap_or_default();
                    ReviewItem {
                        domain: ReviewDomain::Recurring,
                        priority: ReviewPriority::Normal,
                        kind,
                        title: "This transaction could match more than one expected bill"
                            .to_string(),
                        occurrence_ids,
                        ..base
                    }
                }
                "payee_merge_recurring_conflict" => ReviewItem {
                    domain: ReviewDomain::Recurring,
                    priority: ReviewPriority::Normal,
                    kind,
                    title: "Payee merge needs a recurring-pattern decision".to_string(),
                    ..base
                },
                _ => ReviewItem {
                    domain: ReviewDomain::Recurring,
                    priority: ReviewPriority::Normal,
                    kind,
                    title: "Needs review".to_string(),
                    ..base
                },
            }
        }
    }
}

// ---------- Derived items ----------

/// Purchase-type transactions with no category and no splits (a split
/// transaction is categorized at the split level, Batch 5) - never
/// transfers, income, or the opening-balance/adjustment anchors, which
/// don't carry a spending category to begin with (section 13).
fn uncategorized_items(conn: &Connection) -> rusqlite::Result<Vec<ReviewItem>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.account_id, t.transaction_date, t.amount_cents, t.merchant
         FROM transactions t
         JOIN accounts a ON a.id = t.account_id
         WHERE t.transaction_type = 'purchase' AND t.category IS NULL AND t.is_archived = 0
           AND a.is_archived = 0
           AND NOT EXISTS (SELECT 1 FROM transaction_splits s WHERE s.transaction_id = t.id)
         ORDER BY t.transaction_date DESC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let account_id: i64 = row.get(1)?;
            let date: String = row.get(2)?;
            let amount_cents: i64 = row.get(3)?;
            let merchant: String = row.get(4)?;
            Ok(ReviewItem {
                key: format!("uncategorized:{id}"),
                domain: ReviewDomain::Transactions,
                priority: ReviewPriority::Normal,
                kind: "uncategorized".to_string(),
                title: format!("{merchant} needs a category"),
                evidence: format!("{} on {}", dollars(amount_cents), date),
                sort_date: date,
                needs_review_item_id: None,
                transaction_id: Some(id),
                account_id: Some(account_id),
                series_id: None,
                occurrence_ids: Vec::new(),
                reconciliation_id: None,
                duplicate_of_transaction_id: None,
            })
        })?
        .collect();
    rows
}

/// Newly detected, not-yet-confirmed recurring patterns (section 16) -
/// confirmed patterns never appear here once accepted or rejected.
fn recurring_proposed_items(conn: &Connection) -> rusqlite::Result<Vec<ReviewItem>> {
    Ok(pay_cycle::active_account_series(conn)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.status == SeriesStatus::Proposed)
        .map(|s| ReviewItem {
            key: format!("recurring_proposed:{}", s.id),
            domain: ReviewDomain::Recurring,
            // Low, not Normal (section 8): nothing is wrong here, MIDAS is
            // only offering an easy one-click confirm - unlike an
            // ambiguous match or an uncategorized transaction, there is no
            // decision actively blocking clean data.
            priority: ReviewPriority::Low,
            kind: "recurring_proposed".to_string(),
            title: format!("MIDAS detected a recurring pattern: {}", s.merchant),
            evidence: format!("{} · {:?}", dollars(s.expected_amount_cents), s.cadence),
            sort_date: s.created_at.clone(),
            needs_review_item_id: None,
            transaction_id: None,
            account_id: Some(s.account_id),
            series_id: Some(s.id),
            occurrence_ids: Vec::new(),
            reconciliation_id: None,
            duplicate_of_transaction_id: None,
        })
        .collect())
}

/// A confirmed pattern with at least one occurrence still sitting in
/// `missed` status. One item per pattern (its single most recent missed
/// occurrence), not per missed occurrence, so an ongoing gap doesn't pile
/// up repeat cards.
///
/// Deliberately queries `recurring_occurrences.status` directly rather than
/// `RecurringSeries::is_possibly_missed` (Batch 6) - a real bug found via
/// native verification: `is_possibly_missed` reads the series' own
/// `next_expected_date`, which `ensure_occurrences` silently advances
/// forward (self-healing past the gap) the instant *anything* calls it for
/// this account - including Forecast or a Signals check that has nothing to
/// do with this pattern. A review item disappearing because the user
/// merely glanced at Forecast is exactly the kind of stale/flickering
/// behavior section 11 and section 32 both rule out. An occurrence's own
/// `missed` status is durable - nothing reverts it except the user's own
/// skip/link/inactive action - so it can never vanish out from under Review
/// as a side effect of an unrelated read.
fn recurring_missed_items(conn: &Connection) -> rusqlite::Result<Vec<ReviewItem>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.account_id, s.merchant, MAX(o.expected_date) FROM recurring_occurrences o
         JOIN recurring_series s ON s.id = o.series_id
         JOIN accounts a ON a.id = s.account_id
         WHERE o.status = 'missed' AND s.status = 'confirmed' AND a.is_archived = 0
         GROUP BY s.id
         ORDER BY MAX(o.expected_date) DESC",
    )?;
    let rows: Vec<(i64, i64, String, String)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut items = Vec::with_capacity(rows.len());
    for (series_id, account_id, merchant, expected_date) in rows {
        let missed_occurrence_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM recurring_occurrences
                 WHERE series_id = ?1 AND status = 'missed'
                 ORDER BY expected_date DESC LIMIT 1",
                params![series_id],
                |row| row.get(0),
            )
            .ok();
        items.push(ReviewItem {
            key: format!("recurring_missed:{series_id}"),
            domain: ReviewDomain::Recurring,
            priority: ReviewPriority::Normal,
            kind: "recurring_missed".to_string(),
            title: format!("{merchant} - no matching transaction found"),
            evidence: format!("Expected {expected_date}"),
            sort_date: expected_date,
            needs_review_item_id: None,
            transaction_id: None,
            account_id: Some(account_id),
            series_id: Some(series_id),
            occurrence_ids: missed_occurrence_id.into_iter().collect(),
            reconciliation_id: None,
            duplicate_of_transaction_id: None,
        });
    }
    Ok(items)
}

/// A reconciliation a later edit invalidated (Batch 4C) - the account's own
/// history, reused as-is (section 20/21), never a second health state.
fn reconciliation_needs_review_items(conn: &Connection) -> rusqlite::Result<Vec<ReviewItem>> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.account_id, r.statement_end_date, a.name FROM reconciliations r
         JOIN accounts a ON a.id = r.account_id
         WHERE r.status = 'needs_review' AND a.is_archived = 0
         ORDER BY r.statement_end_date DESC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let account_id: i64 = row.get(1)?;
            let statement_end: String = row.get(2)?;
            let account_name: String = row.get(3)?;
            Ok(ReviewItem {
                key: format!("reconciliation:{id}"),
                domain: ReviewDomain::Reconciliation,
                priority: ReviewPriority::High,
                kind: "reconciliation_needs_review".to_string(),
                title: format!("{account_name} reconciliation needs review"),
                evidence: format!(
                    "A cleared transaction changed since the {statement_end} statement was reconciled."
                ),
                sort_date: statement_end,
                needs_review_item_id: None,
                transaction_id: None,
                account_id: Some(account_id),
                series_id: None,
                occurrence_ids: Vec::new(),
                reconciliation_id: Some(id),
                duplicate_of_transaction_id: None,
            })
        })?
        .collect();
    rows
}

/// The full unified list (section 5), sorted High-priority-first, then
/// oldest-relevant-fact-first within a priority tier, then the item's own
/// stable key as a final deterministic tie-break (section 9 - never a
/// random or launch-order-dependent sequence).
pub fn list_review_items(conn: &Connection) -> rusqlite::Result<Vec<ReviewItem>> {
    reconcile_stale_persisted_items(conn)?;

    let mut items: Vec<ReviewItem> = needs_review::list(conn, true)?
        .into_iter()
        .map(map_persisted_item)
        .collect();
    items.extend(uncategorized_items(conn)?);
    items.extend(recurring_proposed_items(conn)?);
    items.extend(recurring_missed_items(conn)?);
    items.extend(reconciliation_needs_review_items(conn)?);

    items.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.sort_date.cmp(&b.sort_date))
            .then_with(|| a.key.cmp(&b.key))
    });
    Ok(items)
}

/// "Keep Both" for a suspected-duplicate import row (section 14): the row
/// was never committed as a transaction (Import Engine v2 held it back
/// pending this decision) - reconstructs it from the exact candidate data
/// captured at staging time and commits it through the same
/// `transactions::create` every other transaction path already uses, then
/// resolves the review item. Never a second insert path.
pub fn keep_duplicate_transaction(
    conn: &Connection,
    needs_review_item_id: i64,
) -> Result<transactions::Transaction, String> {
    let items = needs_review::list(conn, false).map_err(|e| e.to_string())?;
    let item = items
        .into_iter()
        .find(|i| i.id == needs_review_item_id)
        .ok_or_else(|| format!("Review item {needs_review_item_id} not found"))?;
    let json = item
        .proposed_resolution_json
        .as_deref()
        .ok_or("This review item has no candidate transaction to add.")?;
    let proposal: crate::import::StagedRowProposal =
        serde_json::from_str(json).map_err(|e| e.to_string())?;
    let candidate = proposal
        .candidate
        .ok_or("This review item has no candidate transaction to add.")?;

    let input = TransactionInput {
        account_id: candidate.account_id,
        transaction_date: candidate.transaction_date,
        posted_date: candidate.posted_date,
        amount_cents: candidate.amount_cents,
        transaction_type: candidate.transaction_type,
        raw_description: candidate.raw_description,
        merchant: Some(candidate.merchant),
        category: candidate.category,
        subcategory: candidate.subcategory,
        notes: None,
        tags: None,
        is_pending: None,
        payee_id: candidate.candidate_payee_id,
        is_one_time: false,
    };
    let created = transactions::create(conn, &input).map_err(|e| e.to_string())?;
    needs_review::resolve(conn, needs_review_item_id, ResolutionAction::Resolved)?;
    Ok(created)
}

#[tauri::command]
pub fn list_review(state: State<DbState>) -> Result<Vec<ReviewItem>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list_review_items(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn keep_duplicate_review_transaction(
    state: State<DbState>,
    needs_review_item_id: i64,
) -> Result<transactions::Transaction, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    keep_duplicate_transaction(&conn, needs_review_item_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::import::{NormalizedCandidateTransaction, StagedRowProposal};
    use crate::import_profiles::ImportSourceType;
    use crate::reconciliation;
    use crate::recurring::{self, Cadence, Direction, Kind, PatternEditInput};
    use crate::recurring_occurrences;
    use crate::transactions::{TransactionInput, TransactionType};

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

    fn add_txn(
        conn: &Connection,
        account_id: i64,
        date: &str,
        amount_cents: i64,
        transaction_type: TransactionType,
        category: Option<&str>,
        merchant: &str,
    ) -> i64 {
        transactions::create(
            conn,
            &TransactionInput {
                account_id,
                transaction_date: date.to_string(),
                posted_date: None,
                amount_cents,
                transaction_type,
                raw_description: merchant.to_string(),
                merchant: Some(merchant.to_string()),
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

    fn find<'a>(items: &'a [ReviewItem], kind: &str) -> Option<&'a ReviewItem> {
        items.iter().find(|i| i.kind == kind)
    }

    // --- Uncategorized (derived, section 12/13) ---

    #[test]
    fn uncategorized_purchase_appears_and_disappears_once_categorized() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let id = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -4000,
            TransactionType::Purchase,
            None,
            "Mystery",
        );

        let items = list_review_items(&conn).unwrap();
        let item = find(&items, "uncategorized").expect("uncategorized item present");
        assert_eq!(item.transaction_id, Some(id));
        assert_eq!(item.priority, ReviewPriority::Normal);

        transactions::update(
            &conn,
            id,
            &TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -4000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Mystery".to_string(),
                merchant: Some("Mystery".to_string()),
                category: Some("Shopping".to_string()),
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
                payee_id: None,
                is_one_time: false,
            },
        )
        .unwrap();

        let items_after = list_review_items(&conn).unwrap();
        assert!(
            find(&items_after, "uncategorized").is_none(),
            "categorizing must close the item with no separate resolve action (section 11)"
        );
    }

    #[test]
    fn a_split_transaction_is_never_flagged_uncategorized() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let id = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -12000,
            TransactionType::Purchase,
            None,
            "Walmart",
        );
        crate::splits::replace_for_transaction(
            &mut conn,
            id,
            &[
                crate::splits::SplitInput {
                    category: Some("Groceries".to_string()),
                    subcategory: None,
                    amount_cents: -8000,
                    notes: None,
                },
                crate::splits::SplitInput {
                    category: Some("Household".to_string()),
                    subcategory: None,
                    amount_cents: -4000,
                    notes: None,
                },
            ],
        )
        .unwrap();

        let items = list_review_items(&conn).unwrap();
        assert!(find(&items, "uncategorized").is_none());
    }

    #[test]
    fn income_and_transfers_are_never_flagged_uncategorized() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(
            &conn,
            checking,
            "2026-08-01",
            250000,
            TransactionType::Income,
            None,
            "Payroll",
        );
        add_txn(
            &conn,
            checking,
            "2026-08-05",
            -5000,
            TransactionType::Transfer,
            None,
            "To Savings",
        );

        let items = list_review_items(&conn).unwrap();
        assert!(find(&items, "uncategorized").is_none());
    }

    // --- Recurring proposed / missed (sections 16, 18) ---

    #[test]
    fn a_proposed_recurring_pattern_is_low_priority_and_closes_once_confirmed() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        for date in ["2026-06-05", "2026-07-05", "2026-08-05"] {
            add_txn(
                &conn,
                checking,
                date,
                -999,
                TransactionType::Purchase,
                None,
                "Streaming Service",
            );
        }
        let candidates = recurring::detect_candidates(&conn, checking).unwrap();
        assert_eq!(candidates.len(), 1);

        let items = list_review_items(&conn).unwrap();
        let item = find(&items, "recurring_proposed").expect("proposed pattern present");
        assert_eq!(item.priority, ReviewPriority::Low);
        assert_eq!(item.series_id, Some(candidates[0].id));

        recurring::confirm(&mut conn, candidates[0].id).unwrap();
        let items_after = list_review_items(&conn).unwrap();
        assert!(find(&items_after, "recurring_proposed").is_none());
    }

    #[test]
    fn a_confirmed_pattern_with_a_missed_occurrence_appears_as_missed() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        // A past anchor: `create_manual`'s own internal `ensure_occurrences`
        // call immediately generates this first occurrence, finds it more
        // than Monthly's matching window past due with nothing linked, and
        // marks it `missed` right away (section 85: anchored to real
        // wall-clock "today", not a hardcoded date).
        let series = recurring::create_manual(
            &mut conn,
            checking,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Gym".to_string(),
                payee_id: None,
                kind: Kind::Membership,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -3000,
                next_expected_date: crate::dates::add_days(&today_str, -10),
                notes: None,
            },
        )
        .unwrap();

        let items = list_review_items(&conn).unwrap();
        let item = find(&items, "recurring_missed").expect("missed item present");
        assert_eq!(item.series_id, Some(series.id));
        assert_eq!(item.priority, ReviewPriority::Normal);

        recurring::set_inactive(&conn, series.id).unwrap();
        let items_after = list_review_items(&conn).unwrap();
        assert!(find(&items_after, "recurring_missed").is_none());
    }

    #[test]
    fn a_missed_item_survives_an_unrelated_occurrence_refresh() {
        // The exact real bug found via native verification: opening
        // Forecast (or anything else that calls `upcoming_in_range` for
        // this account) must never make an already-`missed` review item
        // silently disappear, even though it advances the series' own
        // `next_expected_date` well into the future in the process.
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let today_str = today(&conn);
        let series = recurring::create_manual(
            &mut conn,
            checking,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Gym".to_string(),
                payee_id: None,
                kind: Kind::Membership,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -3000,
                next_expected_date: crate::dates::add_days(&today_str, -10),
                notes: None,
            },
        )
        .unwrap();
        assert!(find(&list_review_items(&conn).unwrap(), "recurring_missed").is_some());

        // Simulates opening Forecast: refreshes/advances occurrences for
        // every confirmed series on the account, exactly as
        // `forecast::project` -> `upcoming_in_range` already does.
        let far_future = crate::dates::add_days(&today_str, 90);
        recurring_occurrences::upcoming_in_range(&conn, &today_str, &far_future, Some(checking))
            .unwrap();
        let refreshed = recurring::get(&conn, series.id).unwrap().unwrap();
        assert_ne!(
            refreshed.next_expected_date,
            crate::dates::add_days(&today_str, -10),
            "next_expected_date really did advance, proving this exercises the self-healing path"
        );

        let items_after = list_review_items(&conn).unwrap();
        assert!(
            find(&items_after, "recurring_missed").is_some(),
            "a missed occurrence's own durable status must survive an unrelated refresh"
        );
    }

    #[test]
    fn ambiguous_recurring_match_auto_closes_once_resolved_through_any_path() {
        // Reproduces the exact stale-item bug found during reconnaissance:
        // the review item must close even when the ambiguity is resolved
        // WITHOUT going through the one dedicated resolver action.
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let base = crate::dates::add_days(&today(&conn), 10);
        let series = recurring::create_manual(
            &mut conn,
            checking,
            Direction::Expense,
            &PatternEditInput {
                merchant: "Streaming Service".to_string(),
                payee_id: None,
                kind: Kind::Subscription,
                category: None,
                cadence: Cadence::Monthly,
                semimonthly_day_1: None,
                semimonthly_day_2: None,
                expected_amount_cents: -999,
                next_expected_date: base.clone(),
                notes: None,
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO recurring_occurrences (series_id, expected_date, expected_amount_cents) VALUES (?1, ?2, -999)",
            params![series.id, crate::dates::add_days(&base, 2)],
        )
        .unwrap();
        let txn_id = add_txn(
            &conn,
            checking,
            &crate::dates::add_days(&base, 1),
            -999,
            TransactionType::Purchase,
            None,
            "Streaming Service",
        );
        recurring_occurrences::evaluate_transaction_for_matching(&conn, txn_id).unwrap();

        let items = list_review_items(&conn).unwrap();
        let item = find(&items, "ambiguous_recurring_match").expect("ambiguous item present");
        assert_eq!(item.priority, ReviewPriority::Normal);
        assert_eq!(item.occurrence_ids.len(), 2);

        // Resolve it directly through the occurrence-linking path, NOT the
        // needs_review_items resolve() call the frontend resolver normally
        // triggers as a side effect.
        recurring_occurrences::link_occurrence_to_transaction(
            &conn,
            item.occurrence_ids[0],
            txn_id,
        )
        .unwrap();

        let items_after = list_review_items(&conn).unwrap();
        assert!(
            find(&items_after, "ambiguous_recurring_match").is_none(),
            "the review item must close itself once the ambiguity is actually resolved, regardless of path"
        );
    }

    // --- Reconciliation (sections 20-21) ---

    #[test]
    fn an_invalidated_reconciliation_is_a_high_priority_item() {
        let mut conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let txn_id = add_txn(
            &conn,
            checking,
            "2026-08-05",
            -5000,
            TransactionType::Purchase,
            Some("Dining"),
            "Cafe",
        );

        // Build and complete a reconciliation the same way the workspace
        // does: clear the transaction, then complete at the matching total.
        transactions::set_reconciliation_status(
            &conn,
            txn_id,
            crate::transactions::ReconciliationStatus::Cleared,
        )
        .unwrap();
        let reconciliation_id = reconciliation::complete(
            &mut conn,
            checking,
            "2026-08-01",
            "2026-08-31",
            -5000,
            None,
            None,
        )
        .unwrap()
        .id;

        // A later edit to the cleared transaction invalidates it (Batch 4).
        transactions::update(
            &conn,
            txn_id,
            &TransactionInput {
                account_id: checking,
                transaction_date: "2026-08-05".to_string(),
                posted_date: None,
                amount_cents: -6000,
                transaction_type: TransactionType::Purchase,
                raw_description: "Cafe".to_string(),
                merchant: Some("Cafe".to_string()),
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

        let items = list_review_items(&conn).unwrap();
        let item = find(&items, "reconciliation_needs_review")
            .expect("invalidated reconciliation present");
        assert_eq!(item.priority, ReviewPriority::High);
        assert_eq!(item.reconciliation_id, Some(reconciliation_id));
    }

    // --- Sorting (section 9) ---

    #[test]
    fn items_sort_high_before_normal_before_low_deterministically() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        add_txn(
            &conn,
            checking,
            "2026-08-05",
            -4000,
            TransactionType::Purchase,
            None,
            "Zeta",
        );
        for date in ["2026-06-05", "2026-07-05", "2026-08-06"] {
            add_txn(
                &conn,
                checking,
                date,
                -999,
                TransactionType::Purchase,
                None,
                "Streaming Service",
            );
        }
        conn.execute(
            "INSERT INTO needs_review_items (issue_type, detected_reason) VALUES ('malformed_row', 'bad row')",
            [],
        )
        .unwrap();

        let items = list_review_items(&conn).unwrap();
        let priorities: Vec<ReviewPriority> = items.iter().map(|i| i.priority).collect();
        for pair in priorities.windows(2) {
            assert!(
                pair[0] >= pair[1],
                "priorities must be non-increasing: {priorities:?}"
            );
        }

        let first = list_review_items(&conn).unwrap();
        let second = list_review_items(&conn).unwrap();
        assert_eq!(
            first.iter().map(|i| i.key.clone()).collect::<Vec<_>>(),
            second.iter().map(|i| i.key.clone()).collect::<Vec<_>>(),
            "repeated listing must be idempotent and deterministically ordered"
        );
    }

    // --- Suspected duplicate / Keep Both (section 14) ---

    #[test]
    fn keep_both_commits_the_held_row_and_resolves_the_item() {
        let conn = test_conn();
        let checking = make_account(&conn, AccountType::Checking);
        let candidate = NormalizedCandidateTransaction {
            source_type: ImportSourceType::Csv,
            account_id: checking,
            transaction_date: "2026-08-05".to_string(),
            posted_date: None,
            amount_cents: -8423,
            transaction_type: TransactionType::Purchase,
            raw_description: "Publix".to_string(),
            merchant: "Publix".to_string(),
            category: Some("Groceries".to_string()),
            subcategory: None,
            external_transaction_id: None,
            candidate_payee_id: None,
            source_row_hash: "hash1".to_string(),
        };
        let proposal_json = serde_json::to_string(&StagedRowProposal {
            candidate: Some(candidate),
            duplicate_of_transaction_id: Some(999),
        })
        .unwrap();
        let item_id: i64 = conn
            .query_row(
                "INSERT INTO needs_review_items (issue_type, import_id, detected_reason, proposed_resolution_json)
                 VALUES ('suspected_duplicate', NULL, 'looks like a duplicate', ?1) RETURNING id",
                params![proposal_json],
                |r| r.get(0),
            )
            .unwrap();

        let items = list_review_items(&conn).unwrap();
        let item = find(&items, "suspected_duplicate").unwrap();
        assert_eq!(item.duplicate_of_transaction_id, Some(999));
        assert_eq!(item.priority, ReviewPriority::High);

        let created = keep_duplicate_transaction(&conn, item_id).unwrap();
        assert_eq!(created.amount_cents, -8423);
        assert_eq!(created.merchant, "Publix");

        let items_after = list_review_items(&conn).unwrap();
        assert!(find(&items_after, "suspected_duplicate").is_none());
    }
}
