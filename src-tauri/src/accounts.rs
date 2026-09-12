use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::DbState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountType {
    Checking,
    Savings,
    CreditCard,
    Loan,
    Cash,
    Investment,
    Retirement,
    Property,
    OtherLiability,
}

impl AccountType {
    fn as_sql_str(self) -> &'static str {
        match self {
            AccountType::Checking => "checking",
            AccountType::Savings => "savings",
            AccountType::CreditCard => "credit_card",
            AccountType::Loan => "loan",
            AccountType::Cash => "cash",
            AccountType::Investment => "investment",
            AccountType::Retirement => "retirement",
            AccountType::Property => "property",
            AccountType::OtherLiability => "other_liability",
        }
    }

    pub(crate) fn from_sql_str(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "checking" => AccountType::Checking,
            "savings" => AccountType::Savings,
            "credit_card" => AccountType::CreditCard,
            "loan" => AccountType::Loan,
            "cash" => AccountType::Cash,
            "investment" => AccountType::Investment,
            "retirement" => AccountType::Retirement,
            "property" => AccountType::Property,
            "other_liability" => AccountType::OtherLiability,
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    format!("unknown account_type '{other}' in database"),
                    rusqlite::types::Type::Text,
                ))
            }
        })
    }

    /// Whether this account's balance represents money owed rather than
    /// money held - see ADR-0004 (transaction sign convention).
    pub fn is_liability(self) -> bool {
        matches!(
            self,
            AccountType::CreditCard | AccountType::Loan | AccountType::OtherLiability
        )
    }
}

/// Narrow lookup for callers (the import pipeline) that only need to know
/// an account's type, not its full record.
pub fn account_type_of(conn: &Connection, id: i64) -> Result<AccountType, AccountError> {
    conn.query_row(
        "SELECT account_type FROM accounts WHERE id = ?1",
        params![id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .ok_or(AccountError::NotFound(id))
    .and_then(|s| AccountType::from_sql_str(&s).map_err(AccountError::from))
}

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub institution: Option<String>,
    pub account_type: AccountType,
    /// Computed live from `SUM(transactions.amount_cents)`, never stored -
    /// see ADR-0007. To set or correct it, use
    /// `accounting::reconcile_balance`, not this struct.
    pub balance_cents: i64,
    pub include_in_net_worth: bool,
    pub notes: Option<String>,
    pub is_archived: bool,
    pub last_synced_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct AccountInput {
    pub name: String,
    pub institution: Option<String>,
    pub account_type: AccountType,
    pub include_in_net_worth: bool,
    pub notes: Option<String>,
}

#[derive(Debug)]
pub enum AccountError {
    Validation(String),
    Db(rusqlite::Error),
    NotFound(i64),
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountError::Validation(msg) => write!(f, "{msg}"),
            AccountError::Db(e) => write!(f, "database error: {e}"),
            AccountError::NotFound(id) => write!(f, "account {id} not found"),
        }
    }
}

impl From<rusqlite::Error> for AccountError {
    fn from(e: rusqlite::Error) -> Self {
        AccountError::Db(e)
    }
}

fn row_to_account(row: &rusqlite::Row) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get("id")?,
        name: row.get("name")?,
        institution: row.get("institution")?,
        account_type: AccountType::from_sql_str(&row.get::<_, String>("account_type")?)?,
        balance_cents: row.get("balance_cents")?,
        include_in_net_worth: row.get::<_, i64>("include_in_net_worth")? != 0,
        notes: row.get("notes")?,
        is_archived: row.get::<_, i64>("is_archived")? != 0,
        last_synced_at: row.get("last_synced_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn validate(input: &AccountInput) -> Result<(), AccountError> {
    if input.name.trim().is_empty() {
        return Err(AccountError::Validation(
            "Account name must not be empty.".to_string(),
        ));
    }
    Ok(())
}

// balance_cents is a correlated subquery, not a stored column (ADR-0007) -
// aliased so row_to_account's `row.get("balance_cents")` is unchanged.
const SELECT_ACCOUNT: &str = "SELECT
    id, name, institution, account_type,
    COALESCE((SELECT SUM(amount_cents) FROM transactions
              WHERE transactions.account_id = accounts.id AND transactions.is_archived = 0), 0) AS balance_cents,
    include_in_net_worth, notes, is_archived, last_synced_at, created_at, updated_at
    FROM accounts";

pub fn create(conn: &Connection, input: &AccountInput) -> Result<Account, AccountError> {
    validate(input)?;
    let id = conn.query_row(
        "INSERT INTO accounts (name, institution, account_type, include_in_net_worth, notes)
         VALUES (?1, ?2, ?3, ?4, ?5)
         RETURNING id",
        params![
            input.name.trim(),
            input.institution,
            input.account_type.as_sql_str(),
            input.include_in_net_worth as i64,
            input.notes,
        ],
        |row| row.get::<_, i64>(0),
    )?;
    get(conn, id)?.ok_or(AccountError::NotFound(id))
}

/// Fetches one account with its live-computed balance. `pub` for
/// cross-module callers that need a full `Account` (e.g.
/// `accounting::reconcile_balance`) - narrower lookups should use
/// `account_type_of` instead of pulling the whole record.
pub fn get(conn: &Connection, id: i64) -> Result<Option<Account>, AccountError> {
    conn.query_row(
        &format!("{SELECT_ACCOUNT} WHERE id = ?1"),
        params![id],
        row_to_account,
    )
    .optional()
    .map_err(AccountError::from)
}

pub fn list(conn: &Connection, include_archived: bool) -> Result<Vec<Account>, AccountError> {
    let sql = if include_archived {
        format!("{SELECT_ACCOUNT} ORDER BY is_archived, name COLLATE NOCASE")
    } else {
        format!("{SELECT_ACCOUNT} WHERE is_archived = 0 ORDER BY name COLLATE NOCASE")
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_account)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AccountError::from)
}

pub fn update(conn: &Connection, id: i64, input: &AccountInput) -> Result<Account, AccountError> {
    validate(input)?;
    let affected = conn.execute(
        "UPDATE accounts SET
            name = ?1, institution = ?2, account_type = ?3,
            include_in_net_worth = ?4, notes = ?5, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?6",
        params![
            input.name.trim(),
            input.institution,
            input.account_type.as_sql_str(),
            input.include_in_net_worth as i64,
            input.notes,
            id,
        ],
    )?;
    if affected == 0 {
        return Err(AccountError::NotFound(id));
    }
    get(conn, id)?.ok_or(AccountError::NotFound(id))
}

pub fn set_archived(conn: &Connection, id: i64, archived: bool) -> Result<Account, AccountError> {
    let affected = conn.execute(
        "UPDATE accounts SET is_archived = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?2",
        params![archived as i64, id],
    )?;
    if affected == 0 {
        return Err(AccountError::NotFound(id));
    }
    get(conn, id)?.ok_or(AccountError::NotFound(id))
}

#[tauri::command]
pub fn list_accounts(
    state: State<DbState>,
    include_archived: bool,
) -> Result<Vec<Account>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list(&conn, include_archived).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_account(state: State<DbState>, input: AccountInput) -> Result<Account, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    create(&conn, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_account(
    state: State<DbState>,
    id: i64,
    input: AccountInput,
) -> Result<Account, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    update(&conn, id, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_account_archived(
    state: State<DbState>,
    id: i64,
    archived: bool,
) -> Result<Account, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    set_archived(&conn, id, archived).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        conn
    }

    fn sample_input() -> AccountInput {
        AccountInput {
            name: "Everyday Checking".to_string(),
            institution: Some("Test Credit Union".to_string()),
            account_type: AccountType::Checking,
            include_in_net_worth: true,
            notes: None,
        }
    }

    #[test]
    fn create_and_get_round_trips_all_fields() {
        let conn = test_conn();
        let created = create(&conn, &sample_input()).unwrap();
        assert_eq!(created.name, "Everyday Checking");
        assert_eq!(created.institution.as_deref(), Some("Test Credit Union"));
        assert_eq!(created.account_type, AccountType::Checking);
        assert_eq!(
            created.balance_cents, 0,
            "a new account with no transactions has a $0 computed balance"
        );
        assert!(created.include_in_net_worth);
        assert!(!created.is_archived);

        let fetched = get(&conn, created.id).unwrap().unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.updated_at, created.updated_at);
    }

    #[test]
    fn balance_is_computed_live_from_transactions_not_stored() {
        let conn = test_conn();
        let account = create(&conn, &sample_input()).unwrap();
        assert_eq!(get(&conn, account.id).unwrap().unwrap().balance_cents, 0);

        transactions::create(
            &conn,
            &TransactionInput {
                account_id: account.id,
                transaction_date: "2026-03-01".to_string(),
                posted_date: None,
                amount_cents: -2500,
                transaction_type: TransactionType::Purchase,
                raw_description: "Test purchase".to_string(),
                merchant: None,
                category: None,
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
            },
        )
        .unwrap();
        assert_eq!(
            get(&conn, account.id).unwrap().unwrap().balance_cents,
            -2500
        );

        transactions::create(
            &conn,
            &TransactionInput {
                account_id: account.id,
                transaction_date: "2026-03-02".to_string(),
                posted_date: None,
                amount_cents: 1000,
                transaction_type: TransactionType::Income,
                raw_description: "Test income".to_string(),
                merchant: None,
                category: None,
                subcategory: None,
                notes: None,
                tags: None,
                is_pending: None,
            },
        )
        .unwrap();
        assert_eq!(
            list(&conn, false).unwrap()[0].balance_cents,
            -1500,
            "list() must reflect the same live computation as get()"
        );
    }

    #[test]
    fn create_rejects_empty_name() {
        let conn = test_conn();
        let mut input = sample_input();
        input.name = "   ".to_string();
        let err = create(&conn, &input).unwrap_err();
        assert!(matches!(err, AccountError::Validation(_)));
    }

    #[test]
    fn all_nine_account_types_round_trip() {
        let conn = test_conn();
        let types = [
            AccountType::Checking,
            AccountType::Savings,
            AccountType::CreditCard,
            AccountType::Loan,
            AccountType::Cash,
            AccountType::Investment,
            AccountType::Retirement,
            AccountType::Property,
            AccountType::OtherLiability,
        ];
        for t in types {
            let mut input = sample_input();
            input.account_type = t;
            input.name = format!("{t:?} account");
            let created = create(&conn, &input).unwrap();
            assert_eq!(created.account_type, t);
        }
    }

    #[test]
    fn list_excludes_archived_by_default_and_includes_when_asked() {
        let conn = test_conn();
        let a = create(&conn, &sample_input()).unwrap();
        let mut second = sample_input();
        second.name = "Old Savings".to_string();
        let b = create(&conn, &second).unwrap();
        set_archived(&conn, b.id, true).unwrap();

        let active = list(&conn, false).unwrap();
        assert_eq!(active.iter().map(|a| a.id).collect::<Vec<_>>(), vec![a.id]);

        let all = list(&conn, true).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn archive_then_unarchive_round_trips() {
        let conn = test_conn();
        let created = create(&conn, &sample_input()).unwrap();

        let archived = set_archived(&conn, created.id, true).unwrap();
        assert!(archived.is_archived);

        let unarchived = set_archived(&conn, created.id, false).unwrap();
        assert!(!unarchived.is_archived);
    }

    #[test]
    fn update_changes_fields_and_bumps_updated_at() {
        let conn = test_conn();
        let created = create(&conn, &sample_input()).unwrap();

        let mut changed = sample_input();
        changed.name = "Renamed Checking".to_string();
        changed.include_in_net_worth = false;
        let updated = update(&conn, created.id, &changed).unwrap();

        assert_eq!(updated.name, "Renamed Checking");
        assert!(!updated.include_in_net_worth);
    }

    #[test]
    fn update_and_archive_on_missing_id_return_not_found() {
        let conn = test_conn();
        let err = update(&conn, 999, &sample_input()).unwrap_err();
        assert!(matches!(err, AccountError::NotFound(999)));

        let err = set_archived(&conn, 999, true).unwrap_err();
        assert!(matches!(err, AccountError::NotFound(999)));
    }

    #[test]
    fn net_worth_inclusion_flag_persists() {
        let conn = test_conn();
        let mut input = sample_input();
        input.include_in_net_worth = false;
        let created = create(&conn, &input).unwrap();
        assert!(!created.include_in_net_worth);

        let fetched = get(&conn, created.id).unwrap().unwrap();
        assert!(!fetched.include_in_net_worth);
    }
}
