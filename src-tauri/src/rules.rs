//! Deterministic merchant normalization / categorization rules (product
//! handoff section 8). Preferred hierarchy: explicit user decision (already
//! applied before a rule would run) > existing user rule (this module) >
//! built-in logic > Needs Review. No AI inference anywhere here.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::DbState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchType {
    Exact,
    Contains,
    StartsWith,
}

impl MatchType {
    fn as_sql_str(self) -> &'static str {
        match self {
            MatchType::Exact => "exact",
            MatchType::Contains => "contains",
            MatchType::StartsWith => "starts_with",
        }
    }

    fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "exact" => MatchType::Exact,
            "contains" => MatchType::Contains,
            "starts_with" => MatchType::StartsWith,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown match_type '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }

    fn matches(self, description_lower: &str, pattern_lower: &str) -> bool {
        match self {
            MatchType::Exact => description_lower == pattern_lower,
            MatchType::Contains => description_lower.contains(pattern_lower),
            MatchType::StartsWith => description_lower.starts_with(pattern_lower),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MerchantRule {
    pub id: i64,
    pub match_type: MatchType,
    pub match_value: String,
    pub account_id: Option<i64>,
    pub normalized_merchant: Option<String>,
    pub category: Option<String>,
    pub subcategory: Option<String>,
    pub priority: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct MerchantRuleInput {
    pub match_type: MatchType,
    pub match_value: String,
    pub account_id: Option<i64>,
    pub normalized_merchant: Option<String>,
    pub category: Option<String>,
    pub subcategory: Option<String>,
    pub priority: i64,
}

#[derive(Debug)]
pub enum RuleError {
    Validation(String),
    Db(rusqlite::Error),
    NotFound(i64),
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleError::Validation(msg) => write!(f, "{msg}"),
            RuleError::Db(e) => write!(f, "database error: {e}"),
            RuleError::NotFound(id) => write!(f, "rule {id} not found"),
        }
    }
}

impl From<rusqlite::Error> for RuleError {
    fn from(e: rusqlite::Error) -> Self {
        RuleError::Db(e)
    }
}

const COLUMNS: &str = "id, match_type, match_value, account_id, normalized_merchant, category, subcategory, priority, created_at, updated_at";

fn row_to_rule(row: &rusqlite::Row) -> rusqlite::Result<MerchantRule> {
    Ok(MerchantRule {
        id: row.get("id")?,
        match_type: MatchType::from_sql_str(&row.get::<_, String>("match_type")?)?,
        match_value: row.get("match_value")?,
        account_id: row.get("account_id")?,
        normalized_merchant: row.get("normalized_merchant")?,
        category: row.get("category")?,
        subcategory: row.get("subcategory")?,
        priority: row.get("priority")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn validate(input: &MerchantRuleInput) -> Result<(), RuleError> {
    if input.match_value.trim().is_empty() {
        return Err(RuleError::Validation(
            "Match text must not be empty.".to_string(),
        ));
    }
    let merchant_empty = input
        .normalized_merchant
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty();
    let category_empty = input
        .category
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty();
    if merchant_empty && category_empty {
        return Err(RuleError::Validation(
            "A rule must rewrite the merchant, assign a category, or both.".to_string(),
        ));
    }
    Ok(())
}

pub fn create(conn: &Connection, input: &MerchantRuleInput) -> Result<MerchantRule, RuleError> {
    validate(input)?;
    let id = conn.query_row(
        "INSERT INTO merchant_rules (match_type, match_value, account_id, normalized_merchant, category, subcategory, priority)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING id",
        params![
            input.match_type.as_sql_str(),
            input.match_value.trim(),
            input.account_id,
            input.normalized_merchant.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            input.category.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            input.subcategory.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            input.priority,
        ],
        |row| row.get::<_, i64>(0),
    )?;
    get(conn, id)?.ok_or(RuleError::NotFound(id))
}

fn get(conn: &Connection, id: i64) -> Result<Option<MerchantRule>, RuleError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM merchant_rules WHERE id = ?1"),
        params![id],
        row_to_rule,
    )
    .optional()
    .map_err(RuleError::from)
}

pub fn list(conn: &Connection) -> Result<Vec<MerchantRule>, RuleError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM merchant_rules ORDER BY account_id IS NULL, priority DESC, id DESC"
    ))?;
    let rows = stmt.query_map([], row_to_rule)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(RuleError::from)
}

pub fn delete(conn: &Connection, id: i64) -> Result<(), RuleError> {
    let affected = conn.execute("DELETE FROM merchant_rules WHERE id = ?1", params![id])?;
    if affected == 0 {
        return Err(RuleError::NotFound(id));
    }
    Ok(())
}

/// The result of a rule matching a description: whichever fields the rule
/// sets. A caller applies these on top of the deterministic default
/// (merchant = raw_description, category = None), never guessing beyond
/// what the rule explicitly says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMatch {
    pub merchant: Option<String>,
    pub category: Option<String>,
    pub subcategory: Option<String>,
}

/// Loads every rule relevant to `account_id` (account-specific plus global),
/// already ordered account-specific-first, then priority, then most recent -
/// i.e. exactly the order `best_match` should check them in. Callers that
/// need to match many descriptions against the same account (CSV staging)
/// should call this once and reuse the result via `best_match`, rather than
/// re-querying per row.
pub fn relevant_rules(conn: &Connection, account_id: i64) -> Result<Vec<MerchantRule>, RuleError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM merchant_rules
         WHERE account_id IS NULL OR account_id = ?1
         ORDER BY account_id IS NULL, priority DESC, id DESC"
    ))?;
    let rules = stmt
        .query_map(params![account_id], row_to_rule)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rules)
}

/// Finds the first (highest-priority) rule in `rules` - already ordered by
/// `relevant_rules` - that matches `raw_description`. Pure and DB-free, so
/// it can run once per staged CSV row without a query each time. Returns
/// `None` when no rule matches - callers fall through to the existing
/// deterministic default, never to a guess.
pub fn best_match(rules: &[MerchantRule], raw_description: &str) -> Option<RuleMatch> {
    let description_lower = raw_description.trim().to_lowercase();
    for rule in rules {
        let pattern_lower = rule.match_value.trim().to_lowercase();
        if rule.match_type.matches(&description_lower, &pattern_lower) {
            return Some(RuleMatch {
                merchant: rule.normalized_merchant.clone(),
                category: rule.category.clone(),
                subcategory: rule.subcategory.clone(),
            });
        }
    }
    None
}

#[tauri::command]
pub fn list_rules(state: State<DbState>) -> Result<Vec<MerchantRule>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_rule(
    state: State<DbState>,
    input: MerchantRuleInput,
) -> Result<MerchantRule, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    create(&conn, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_rule(state: State<DbState>, id: i64) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    delete(&conn, id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};

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
        conn
    }

    fn make_account(conn: &Connection) -> i64 {
        accounts::create(
            conn,
            &AccountInput {
                name: "Test".to_string(),
                institution: None,
                account_type: AccountType::Checking,
                include_in_net_worth: true,
                notes: None,
            },
        )
        .unwrap()
        .id
    }

    /// Mirrors the real call pattern used by import staging: load the
    /// account's rules once, then match against them.
    fn apply(conn: &Connection, account_id: i64, description: &str) -> Option<RuleMatch> {
        let rules = relevant_rules(conn, account_id).unwrap();
        best_match(&rules, description)
    }

    fn global_rule(
        match_type: MatchType,
        value: &str,
        merchant: Option<&str>,
        category: Option<&str>,
    ) -> MerchantRuleInput {
        MerchantRuleInput {
            match_type,
            match_value: value.to_string(),
            account_id: None,
            normalized_merchant: merchant.map(str::to_string),
            category: category.map(str::to_string),
            subcategory: None,
            priority: 0,
        }
    }

    #[test]
    fn create_rejects_empty_match_value_and_empty_effect() {
        let conn = test_conn();
        let mut bad = global_rule(MatchType::Contains, "", Some("X"), None);
        assert!(matches!(create(&conn, &bad), Err(RuleError::Validation(_))));

        bad.match_value = "coffee".to_string();
        bad.normalized_merchant = None;
        bad.category = None;
        assert!(matches!(create(&conn, &bad), Err(RuleError::Validation(_))));
    }

    #[test]
    fn contains_match_normalizes_the_classic_processor_noise_example() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(
            &conn,
            &global_rule(
                MatchType::Contains,
                "coffee shop",
                Some("Coffee Shop"),
                Some("Dining & Restaurants"),
            ),
        )
        .unwrap();

        let result = apply(&conn, account_id, "SQ *COFFEE SHOP 123 MIAMI FL").unwrap();
        assert_eq!(result.merchant.as_deref(), Some("Coffee Shop"));
        assert_eq!(result.category.as_deref(), Some("Dining & Restaurants"));
    }

    #[test]
    fn no_matching_rule_returns_none_not_a_guess() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(
            &conn,
            &global_rule(MatchType::Exact, "netflix", Some("Netflix"), None),
        )
        .unwrap();
        assert_eq!(apply(&conn, account_id, "SOME OTHER MERCHANT"), None);
    }

    #[test]
    fn exact_match_does_not_match_a_substring() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(
            &conn,
            &global_rule(MatchType::Exact, "netflix", Some("Netflix"), None),
        )
        .unwrap();
        assert_eq!(apply(&conn, account_id, "NETFLIX.COM"), None);
    }

    #[test]
    fn starts_with_match_works() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(
            &conn,
            &global_rule(
                MatchType::StartsWith,
                "amzn",
                Some("Amazon"),
                Some("Shopping"),
            ),
        )
        .unwrap();
        let result = apply(&conn, account_id, "AMZN MKTP US*1A2B3").unwrap();
        assert_eq!(result.merchant.as_deref(), Some("Amazon"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(
            &conn,
            &global_rule(MatchType::Contains, "COFFEE", Some("Coffee Shop"), None),
        )
        .unwrap();
        assert!(apply(&conn, account_id, "coffee shop downtown").is_some());
    }

    #[test]
    fn account_specific_rule_wins_over_global_rule() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        create(
            &conn,
            &global_rule(MatchType::Contains, "store", Some("Generic Store"), None),
        )
        .unwrap();
        create(
            &conn,
            &MerchantRuleInput {
                match_type: MatchType::Contains,
                match_value: "store".to_string(),
                account_id: Some(account_id),
                normalized_merchant: Some("My Specific Store".to_string()),
                category: None,
                subcategory: None,
                priority: 0,
            },
        )
        .unwrap();

        let result = apply(&conn, account_id, "THE STORE #42").unwrap();
        assert_eq!(result.merchant.as_deref(), Some("My Specific Store"));
    }

    #[test]
    fn higher_priority_wins_among_equally_specific_rules() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        let mut low = global_rule(MatchType::Contains, "shop", Some("Low Priority"), None);
        low.priority = 0;
        create(&conn, &low).unwrap();
        let mut high = global_rule(MatchType::Contains, "shop", Some("High Priority"), None);
        high.priority = 10;
        create(&conn, &high).unwrap();

        let result = apply(&conn, account_id, "COFFEE SHOP").unwrap();
        assert_eq!(result.merchant.as_deref(), Some("High Priority"));
    }

    #[test]
    fn list_and_delete_round_trip() {
        let conn = test_conn();
        let created = create(
            &conn,
            &global_rule(MatchType::Exact, "netflix", Some("Netflix"), None),
        )
        .unwrap();
        assert_eq!(list(&conn).unwrap().len(), 1);
        delete(&conn, created.id).unwrap();
        assert_eq!(list(&conn).unwrap().len(), 0);
        assert!(matches!(
            delete(&conn, created.id),
            Err(RuleError::NotFound(_))
        ));
    }
}
