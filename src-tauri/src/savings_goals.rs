//! Savings goals (Phase 4B). A goal is plain, user-set tracking - no
//! auto-linkage to an account or category. It only affects Safe to Spend
//! when the owner explicitly sets `reserved_amount_cents` (Safe to Spend
//! reads this once built; nothing in this module computes a cross-goal
//! total yet, since nothing calls it until then - see the Phase 4A lesson
//! about writing library code ahead of a real caller).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::DbState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    InProgress,
    Completed,
    Overdue,
}

#[derive(Debug, Clone, Serialize)]
pub struct SavingsGoal {
    pub id: i64,
    pub name: String,
    pub target_amount_cents: i64,
    pub current_amount_cents: i64,
    pub target_date: Option<String>,
    pub reserved_amount_cents: Option<i64>,
    pub is_archived: bool,
    pub created_at: String,
    pub updated_at: String,
    pub status: GoalStatus,
}

#[derive(Debug, Deserialize)]
pub struct SavingsGoalInput {
    pub name: String,
    pub target_amount_cents: i64,
    pub current_amount_cents: i64,
    pub target_date: Option<String>,
    pub reserved_amount_cents: Option<i64>,
}

#[derive(Debug)]
pub enum GoalError {
    Validation(String),
    Db(rusqlite::Error),
    NotFound(i64),
}

impl std::fmt::Display for GoalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoalError::Validation(msg) => write!(f, "{msg}"),
            GoalError::Db(e) => write!(f, "database error: {e}"),
            GoalError::NotFound(id) => write!(f, "savings goal {id} not found"),
        }
    }
}

impl From<rusqlite::Error> for GoalError {
    fn from(e: rusqlite::Error) -> Self {
        GoalError::Db(e)
    }
}

fn validate(input: &SavingsGoalInput) -> Result<(), GoalError> {
    if input.name.trim().is_empty() {
        return Err(GoalError::Validation("Name is required.".to_string()));
    }
    if input.target_amount_cents <= 0 {
        return Err(GoalError::Validation(
            "Target amount must be greater than zero.".to_string(),
        ));
    }
    if input.current_amount_cents < 0 {
        return Err(GoalError::Validation(
            "Current amount cannot be negative.".to_string(),
        ));
    }
    if let Some(d) = &input.target_date {
        if !d.trim().is_empty() && crate::dates::parse_iso(d).is_none() {
            return Err(GoalError::Validation(
                "Target date must be a valid date.".to_string(),
            ));
        }
    }
    if let Some(r) = input.reserved_amount_cents {
        if r < 0 {
            return Err(GoalError::Validation(
                "Reserved amount cannot be negative.".to_string(),
            ));
        }
    }
    Ok(())
}

fn today(conn: &Connection) -> rusqlite::Result<String> {
    conn.query_row("SELECT date('now')", [], |row| row.get(0))
}

const COLUMNS: &str = "id, name, target_amount_cents, current_amount_cents, target_date, \
    reserved_amount_cents, is_archived, created_at, updated_at";

fn hydrate(conn: &Connection, row: &rusqlite::Row) -> rusqlite::Result<SavingsGoal> {
    let target_amount_cents: i64 = row.get("target_amount_cents")?;
    let current_amount_cents: i64 = row.get("current_amount_cents")?;
    let target_date: Option<String> = row.get("target_date")?;

    let status = if current_amount_cents >= target_amount_cents {
        GoalStatus::Completed
    } else {
        let overdue = match &target_date {
            Some(d) => *d < today(conn)?,
            None => false,
        };
        if overdue {
            GoalStatus::Overdue
        } else {
            GoalStatus::InProgress
        }
    };

    Ok(SavingsGoal {
        id: row.get("id")?,
        name: row.get("name")?,
        target_amount_cents,
        current_amount_cents,
        target_date,
        reserved_amount_cents: row.get("reserved_amount_cents")?,
        is_archived: row.get::<_, i64>("is_archived")? != 0,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        status,
    })
}

fn get(conn: &Connection, id: i64) -> Result<Option<SavingsGoal>, GoalError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM savings_goals WHERE id = ?1"),
        params![id],
        |row| hydrate(conn, row),
    )
    .optional()
    .map_err(GoalError::from)
}

pub fn list(conn: &Connection, include_archived: bool) -> Result<Vec<SavingsGoal>, GoalError> {
    let sql = if include_archived {
        format!("SELECT {COLUMNS} FROM savings_goals ORDER BY is_archived, name COLLATE NOCASE")
    } else {
        format!(
            "SELECT {COLUMNS} FROM savings_goals WHERE is_archived = 0 ORDER BY name COLLATE NOCASE"
        )
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| hydrate(conn, row))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(GoalError::from)
}

/// Sum of every non-archived goal's `reserved_amount_cents` - the "how
/// much has the owner explicitly protected from Safe to Spend" total. A
/// goal with no reservation set contributes nothing.
pub fn total_reserved(conn: &Connection) -> Result<i64, GoalError> {
    let goals = list(conn, false)?;
    Ok(goals.iter().filter_map(|g| g.reserved_amount_cents).sum())
}

pub fn create(conn: &Connection, input: &SavingsGoalInput) -> Result<SavingsGoal, GoalError> {
    validate(input)?;
    let target_date = input
        .target_date
        .as_deref()
        .filter(|d| !d.trim().is_empty());
    let id = conn.query_row(
        "INSERT INTO savings_goals (
            name, target_amount_cents, current_amount_cents, target_date, reserved_amount_cents
         ) VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
        params![
            input.name.trim(),
            input.target_amount_cents,
            input.current_amount_cents,
            target_date,
            input.reserved_amount_cents,
        ],
        |row| row.get::<_, i64>(0),
    )?;
    get(conn, id)?.ok_or(GoalError::NotFound(id))
}

pub fn update(
    conn: &Connection,
    id: i64,
    input: &SavingsGoalInput,
) -> Result<SavingsGoal, GoalError> {
    validate(input)?;
    let target_date = input
        .target_date
        .as_deref()
        .filter(|d| !d.trim().is_empty());
    let affected = conn.execute(
        "UPDATE savings_goals SET
            name = ?1, target_amount_cents = ?2, current_amount_cents = ?3,
            target_date = ?4, reserved_amount_cents = ?5,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?6",
        params![
            input.name.trim(),
            input.target_amount_cents,
            input.current_amount_cents,
            target_date,
            input.reserved_amount_cents,
            id,
        ],
    )?;
    if affected == 0 {
        return Err(GoalError::NotFound(id));
    }
    get(conn, id)?.ok_or(GoalError::NotFound(id))
}

pub fn set_archived(conn: &Connection, id: i64, archived: bool) -> Result<SavingsGoal, GoalError> {
    let affected = conn.execute(
        "UPDATE savings_goals SET is_archived = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?2",
        params![archived as i64, id],
    )?;
    if affected == 0 {
        return Err(GoalError::NotFound(id));
    }
    get(conn, id)?.ok_or(GoalError::NotFound(id))
}

#[tauri::command]
pub fn list_savings_goals(
    state: State<DbState>,
    include_archived: bool,
) -> Result<Vec<SavingsGoal>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    list(&conn, include_archived).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_savings_goal(
    state: State<DbState>,
    input: SavingsGoalInput,
) -> Result<SavingsGoal, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    create(&conn, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_savings_goal(
    state: State<DbState>,
    id: i64,
    input: SavingsGoalInput,
) -> Result<SavingsGoal, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    update(&conn, id, &input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_savings_goal_archived(
    state: State<DbState>,
    id: i64,
    archived: bool,
) -> Result<SavingsGoal, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    set_archived(&conn, id, archived).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/0001_initial.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../migrations/0006_savings_goals.sql"))
            .unwrap();
        conn
    }

    fn sample_input() -> SavingsGoalInput {
        SavingsGoalInput {
            name: "Emergency Fund".to_string(),
            target_amount_cents: 500_000,
            current_amount_cents: 100_000,
            target_date: None,
            reserved_amount_cents: None,
        }
    }

    #[test]
    fn create_and_get_round_trip() {
        let conn = test_conn();
        let goal = create(&conn, &sample_input()).unwrap();
        assert_eq!(goal.name, "Emergency Fund");
        assert_eq!(goal.target_amount_cents, 500_000);
        assert_eq!(goal.current_amount_cents, 100_000);
        assert_eq!(goal.reserved_amount_cents, None);
        assert!(!goal.is_archived);
    }

    #[test]
    fn rejects_empty_name_nonpositive_target_and_negative_current() {
        let conn = test_conn();
        let mut input = sample_input();
        input.name = "   ".to_string();
        assert!(matches!(
            create(&conn, &input),
            Err(GoalError::Validation(_))
        ));

        let mut input2 = sample_input();
        input2.target_amount_cents = 0;
        assert!(matches!(
            create(&conn, &input2),
            Err(GoalError::Validation(_))
        ));

        let mut input3 = sample_input();
        input3.current_amount_cents = -1;
        assert!(matches!(
            create(&conn, &input3),
            Err(GoalError::Validation(_))
        ));
    }

    #[test]
    fn status_is_completed_when_current_meets_or_exceeds_target() {
        let conn = test_conn();
        let mut input = sample_input();
        input.current_amount_cents = 500_000;
        let goal = create(&conn, &input).unwrap();
        assert_eq!(goal.status, GoalStatus::Completed);

        let mut over = sample_input();
        over.name = "Over".to_string();
        over.current_amount_cents = 600_000;
        let goal2 = create(&conn, &over).unwrap();
        assert_eq!(goal2.status, GoalStatus::Completed);
    }

    #[test]
    fn status_is_overdue_when_target_date_has_passed_without_completion() {
        let conn = test_conn();
        let mut input = sample_input();
        input.target_date = Some("2000-01-01".to_string()); // long past
        let goal = create(&conn, &input).unwrap();
        assert_eq!(goal.status, GoalStatus::Overdue);
    }

    #[test]
    fn status_is_in_progress_with_a_future_target_date_or_no_date() {
        let conn = test_conn();
        let goal_no_date = create(&conn, &sample_input()).unwrap();
        assert_eq!(goal_no_date.status, GoalStatus::InProgress);

        let mut future = sample_input();
        future.name = "Future".to_string();
        future.target_date = Some("2100-01-01".to_string());
        let goal_future = create(&conn, &future).unwrap();
        assert_eq!(goal_future.status, GoalStatus::InProgress);
    }

    #[test]
    fn reserved_amount_defaults_to_none_and_can_be_set_and_cleared() {
        let conn = test_conn();
        let created = create(&conn, &sample_input()).unwrap();
        assert_eq!(created.reserved_amount_cents, None);

        let mut reserve = sample_input();
        reserve.reserved_amount_cents = Some(50_000);
        let updated = update(&conn, created.id, &reserve).unwrap();
        assert_eq!(updated.reserved_amount_cents, Some(50_000));

        let cleared = update(&conn, created.id, &sample_input()).unwrap();
        assert_eq!(
            cleared.reserved_amount_cents, None,
            "explicitly unreserving must actually clear it, not leave a stale value"
        );
    }

    #[test]
    fn rejects_negative_reserved_amount_and_invalid_target_date() {
        let conn = test_conn();
        let mut input = sample_input();
        input.reserved_amount_cents = Some(-100);
        assert!(matches!(
            create(&conn, &input),
            Err(GoalError::Validation(_))
        ));

        let mut input2 = sample_input();
        input2.target_date = Some("not-a-date".to_string());
        assert!(matches!(
            create(&conn, &input2),
            Err(GoalError::Validation(_))
        ));
    }

    #[test]
    fn archive_hides_from_default_list_and_update_and_archive_missing_id_errors() {
        let conn = test_conn();
        let created = create(&conn, &sample_input()).unwrap();
        set_archived(&conn, created.id, true).unwrap();

        assert!(list(&conn, false).unwrap().is_empty());
        assert_eq!(list(&conn, true).unwrap().len(), 1);

        assert!(matches!(
            update(&conn, 999, &sample_input()),
            Err(GoalError::NotFound(999))
        ));
        assert!(matches!(
            set_archived(&conn, 999, true),
            Err(GoalError::NotFound(999))
        ));
    }

    #[test]
    fn total_reserved_sums_only_reserved_non_archived_goals() {
        let conn = test_conn();
        let mut reserved = sample_input();
        reserved.reserved_amount_cents = Some(50_000);
        create(&conn, &reserved).unwrap();

        let mut unreserved = sample_input();
        unreserved.name = "Unreserved".to_string();
        create(&conn, &unreserved).unwrap();

        let mut also_reserved = sample_input();
        also_reserved.name = "Also Reserved".to_string();
        also_reserved.reserved_amount_cents = Some(20_000);
        let to_archive = create(&conn, &also_reserved).unwrap();

        assert_eq!(total_reserved(&conn).unwrap(), 70_000);

        set_archived(&conn, to_archive.id, true).unwrap();
        assert_eq!(
            total_reserved(&conn).unwrap(),
            50_000,
            "an archived goal's reservation must stop counting"
        );
    }
}
