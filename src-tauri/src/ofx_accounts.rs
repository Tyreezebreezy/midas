//! OFX/QFX source-account -> MIDAS-account mapping (Import Engine v2 batch
//! 3, specs/phase-010). A distinct, smaller concept from CSV's
//! `import_profiles` (which describe how to interpret file *columns*):
//! this remembers which MIDAS account a *structured* source account
//! (institution + account number/type) should feed, and only when the
//! owner explicitly opts in - never guessed, per product handoff section
//! 15's "never weakly guess which account should receive financial
//! transactions."

use rusqlite::{params, Connection, OptionalExtension};

use crate::ofx::SourceAccount;

/// Looks up a previously-remembered mapping for this exact source account
/// identity. `None` means MIDAS has never seen this source account before -
/// the caller must ask the owner to choose, never assume.
pub fn find_mapping(conn: &Connection, account: &SourceAccount) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT account_id FROM ofx_account_mappings
         WHERE COALESCE(bank_id, '') = COALESCE(?1, '')
           AND source_account_id = ?2
           AND COALESCE(source_account_type, '') = COALESCE(?3, '')",
        params![
            account.bank_id,
            account.source_account_id,
            account.source_account_type,
        ],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Remembers that this source account identity feeds `account_id` - called
/// only when the owner explicitly checks "Remember this account mapping".
/// Upserts rather than erroring on a repeat mapping, since correcting a
/// previous choice is a legitimate, explicit action, not a conflict.
pub fn remember(conn: &Connection, account: &SourceAccount, account_id: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO ofx_account_mappings (bank_id, source_account_id, source_account_type, account_id)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (COALESCE(bank_id, ''), source_account_id, COALESCE(source_account_type, ''))
         DO UPDATE SET account_id = excluded.account_id, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![
            account.bank_id,
            account.source_account_id,
            account.source_account_type,
            account_id,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountInput, AccountType};
    use crate::ofx::StatementKind;

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

    fn sample_account() -> SourceAccount {
        SourceAccount {
            bank_id: Some("111000025".to_string()),
            source_account_id: "0001112223".to_string(),
            source_account_type: Some("CHECKING".to_string()),
            kind: StatementKind::Bank,
            currency: Some("USD".to_string()),
        }
    }

    #[test]
    fn unknown_source_account_has_no_mapping() {
        let conn = test_conn();
        assert_eq!(find_mapping(&conn, &sample_account()).unwrap(), None);
    }

    #[test]
    fn remember_then_find_round_trips() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        remember(&conn, &sample_account(), account_id).unwrap();
        assert_eq!(
            find_mapping(&conn, &sample_account()).unwrap(),
            Some(account_id)
        );
    }

    #[test]
    fn remembering_again_updates_rather_than_duplicating() {
        let conn = test_conn();
        let first_account = make_account(&conn);
        let second_account = make_account(&conn);
        remember(&conn, &sample_account(), first_account).unwrap();
        remember(&conn, &sample_account(), second_account).unwrap();

        assert_eq!(
            find_mapping(&conn, &sample_account()).unwrap(),
            Some(second_account)
        );
        let count: i64 = conn
            .query_row("SELECT count(*) FROM ofx_account_mappings", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1, "correcting a mapping must not duplicate rows");
    }

    #[test]
    fn a_credit_card_account_with_no_bank_id_or_type_is_a_distinct_identity() {
        let conn = test_conn();
        let account_id = make_account(&conn);
        let cc = SourceAccount {
            bank_id: None,
            source_account_id: "4111111111111111".to_string(),
            source_account_type: None,
            kind: StatementKind::CreditCard,
            currency: Some("USD".to_string()),
        };
        remember(&conn, &cc, account_id).unwrap();
        assert_eq!(find_mapping(&conn, &cc).unwrap(), Some(account_id));

        // A different card number must never match this mapping.
        let other_cc = SourceAccount {
            source_account_id: "4222222222222222".to_string(),
            ..cc
        };
        assert_eq!(find_mapping(&conn, &other_cc).unwrap(), None);
    }

    #[test]
    fn the_same_account_id_from_two_different_banks_are_distinct_identities() {
        // Product handoff section 9: never assume a source account id from
        // one institution can't collide with a different one's.
        let conn = test_conn();
        let account_a = make_account(&conn);
        let account_b = make_account(&conn);
        let mut bank_a = sample_account();
        bank_a.bank_id = Some("111000025".to_string());
        let mut bank_b = sample_account();
        bank_b.bank_id = Some("999000111".to_string());

        remember(&conn, &bank_a, account_a).unwrap();
        remember(&conn, &bank_b, account_b).unwrap();

        assert_eq!(find_mapping(&conn, &bank_a).unwrap(), Some(account_a));
        assert_eq!(find_mapping(&conn, &bank_b).unwrap(), Some(account_b));
    }
}
