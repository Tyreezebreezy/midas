use rusqlite::{params, Connection, OptionalExtension};

pub fn get(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
}

pub fn set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/0001_initial.sql"))
            .unwrap();
        conn
    }

    #[test]
    fn missing_key_returns_none() {
        let conn = test_conn();
        assert_eq!(get(&conn, "nope").unwrap(), None);
    }

    #[test]
    fn set_then_get_round_trips() {
        let conn = test_conn();
        set(&conn, "reserve_cents", "50000").unwrap();
        assert_eq!(
            get(&conn, "reserve_cents").unwrap().as_deref(),
            Some("50000")
        );
    }

    #[test]
    fn set_overwrites_existing_value() {
        let conn = test_conn();
        set(&conn, "reserve_cents", "50000").unwrap();
        set(&conn, "reserve_cents", "75000").unwrap();
        assert_eq!(
            get(&conn, "reserve_cents").unwrap().as_deref(),
            Some("75000")
        );
    }
}
