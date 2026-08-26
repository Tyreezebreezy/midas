//! Shared calendar-date arithmetic (ADR-0008: `chrono::NaiveDate`, no
//! timezone/clock features enabled - MIDAS has no timezone concept, only
//! bare `YYYY-MM-DD` dates parsed from statements or typed by the user).
//! "What is today" still comes from SQLite's own `date('now')` at each call
//! site that needs it, not from chrono, since the `"clock"` feature that
//! would provide wall-clock access is deliberately not enabled.
//!
//! Every function here takes already-validated ISO date strings (the output
//! of `money::parse_date`, or a value already persisted to the database) and
//! panics on malformed input rather than silently defaulting - a corrupted
//! or mistakenly-unvalidated date should be loud, never quietly miscomputed.

use chrono::{Days, NaiveDate};

/// Parses a strict ISO `YYYY-MM-DD` string into a real calendar date.
pub fn parse_iso(date: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

pub fn format_iso(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// Whether (year, month, day) is a real calendar date within the range a
/// statement date could plausibly fall in (guards against, e.g., a typo'd
/// far-future or far-past year - not a calendar-correctness concern).
pub fn is_valid_calendar_date(year: i32, month: u32, day: u32) -> bool {
    (1900..=2100).contains(&year) && NaiveDate::from_ymd_opt(year, month, day).is_some()
}

fn expect_iso(date: &str) -> NaiveDate {
    parse_iso(date).unwrap_or_else(|| panic!("'{date}' is not a valid ISO date"))
}

/// Days from `earlier` to `later` (negative if `later` comes first).
pub fn days_between(earlier: &str, later: &str) -> i64 {
    expect_iso(later)
        .signed_duration_since(expect_iso(earlier))
        .num_days()
}

pub fn add_days(date: &str, days: i64) -> String {
    let d = expect_iso(date);
    let shifted = if days >= 0 {
        d.checked_add_days(Days::new(days as u64))
    } else {
        d.checked_sub_days(Days::new(days.unsigned_abs()))
    };
    format_iso(shifted.expect("date arithmetic overflowed the supported range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_between_matches_known_reference_points() {
        assert_eq!(days_between("1970-01-01", "1970-01-01"), 0);
        assert_eq!(days_between("1970-01-01", "1970-01-02"), 1);
        assert_eq!(days_between("2026-01-01", "2026-02-01"), 31);
        assert_eq!(days_between("2024-02-28", "2024-03-01"), 2); // 2024 is a leap year
        assert_eq!(days_between("2023-02-28", "2023-03-01"), 1); // 2023 is not
        assert_eq!(days_between("2026-02-01", "2026-01-01"), -31);
    }

    #[test]
    fn add_days_round_trips_with_days_between() {
        let start = "2026-01-15";
        let result = add_days(start, 30);
        assert_eq!(result, "2026-02-14");
        assert_eq!(days_between(start, &result), 30);
        assert_eq!(add_days(start, -15), "2025-12-31");
    }

    #[test]
    fn is_valid_calendar_date_rejects_impossible_dates_and_out_of_range_years() {
        assert!(is_valid_calendar_date(2026, 2, 28));
        assert!(is_valid_calendar_date(2024, 2, 29));
        assert!(!is_valid_calendar_date(2026, 2, 30));
        assert!(!is_valid_calendar_date(2026, 13, 1));
        assert!(!is_valid_calendar_date(1899, 1, 1));
        assert!(!is_valid_calendar_date(2101, 1, 1));
    }
}
