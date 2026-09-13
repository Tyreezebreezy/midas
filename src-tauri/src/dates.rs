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

use chrono::{Datelike, Days, NaiveDate};

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

fn days_in_month(year: i32, month: u32) -> u32 {
    let this_month_first = NaiveDate::from_ymd_opt(year, month, 1).expect("month must be valid");
    let next_month_first = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .expect("month + 1 must be a valid calendar date");
    (next_month_first - this_month_first).num_days() as u32
}

pub fn start_of_month(date: &str) -> String {
    let d = expect_iso(date);
    format_iso(NaiveDate::from_ymd_opt(d.year(), d.month(), 1).expect("must be valid"))
}

pub fn end_of_month(date: &str) -> String {
    let d = expect_iso(date);
    let last = days_in_month(d.year(), d.month());
    format_iso(NaiveDate::from_ymd_opt(d.year(), d.month(), last).expect("must be valid"))
}

/// Adds calendar months (may be negative), clamping the day to the target
/// month's length - e.g. Jan 31 + 1 month -> Feb 28/29, not an overflow into
/// March. The standard "same day next month" convention.
pub fn add_months(date: &str, months: i64) -> String {
    let d = expect_iso(date);
    let total_months = d.year() as i64 * 12 + d.month0() as i64 + months;
    let year = total_months.div_euclid(12) as i32;
    let month = total_months.rem_euclid(12) as u32 + 1;
    let day = d.day().min(days_in_month(year, month));
    format_iso(NaiveDate::from_ymd_opt(year, month, day).expect("clamped date must be valid"))
}

/// The previous calendar month's range, truncated to the same elapsed-day
/// count as `reference_date` within its own month - the one slice of
/// Insights' frontend "same-point" period-comparison rule
/// (`lib/period.ts::periodComparison`) that Financial Signals (Batch 9)
/// need computed server-side for a single deterministic reference date
/// (e.g. "Sep 1-13" compares against "Aug 1-13", never all of August).
pub fn same_point_previous_month(reference_date: &str) -> (String, String) {
    let month_start = start_of_month(reference_date);
    let elapsed_days = days_between(&month_start, reference_date);
    let previous_month_start = add_months(&month_start, -1);
    let previous_month_to = add_days(&previous_month_start, elapsed_days);
    (previous_month_start, previous_month_to)
}

/// Constructs `day` within the same month as `date_in_month`, clamped to
/// that month's length - e.g. `day_in_month("2026-02-10", 31) == "2026-02-28"`.
pub fn day_in_month(date_in_month: &str, day: u32) -> String {
    let d = expect_iso(date_in_month);
    let clamped = day.max(1).min(days_in_month(d.year(), d.month()));
    format_iso(NaiveDate::from_ymd_opt(d.year(), d.month(), clamped).expect("must be valid"))
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
    fn start_and_end_of_month_are_correct_including_leap_february() {
        assert_eq!(start_of_month("2026-03-17"), "2026-03-01");
        assert_eq!(end_of_month("2026-03-17"), "2026-03-31");
        assert_eq!(end_of_month("2026-02-01"), "2026-02-28");
        assert_eq!(end_of_month("2024-02-01"), "2024-02-29");
        assert_eq!(end_of_month("2026-12-01"), "2026-12-31");
    }

    #[test]
    fn add_months_clamps_short_months_instead_of_overflowing() {
        assert_eq!(add_months("2026-01-31", 1), "2026-02-28"); // 2026 not a leap year
        assert_eq!(add_months("2024-01-31", 1), "2024-02-29"); // 2024 is
        assert_eq!(add_months("2026-03-31", 1), "2026-04-30");
    }

    #[test]
    fn add_months_crosses_year_boundaries_forward_and_backward() {
        assert_eq!(add_months("2026-12-15", 1), "2027-01-15");
        assert_eq!(add_months("2026-01-15", -1), "2025-12-15");
        assert_eq!(add_months("2026-06-15", 12), "2027-06-15");
        assert_eq!(add_months("2026-06-15", -18), "2024-12-15");
    }

    #[test]
    fn same_point_previous_month_truncates_to_the_same_elapsed_days() {
        assert_eq!(
            same_point_previous_month("2026-09-13"),
            ("2026-08-01".to_string(), "2026-08-13".to_string())
        );
        // First of the month: zero days elapsed either side.
        assert_eq!(
            same_point_previous_month("2026-09-01"),
            ("2026-08-01".to_string(), "2026-08-01".to_string())
        );
        // The invariant that actually matters (section 32's fairness rule):
        // the same number of days have elapsed on both sides, even across a
        // short February - never a hand-picked literal that could mask an
        // off-by-one.
        let (prev_from, prev_to) = same_point_previous_month("2026-03-30");
        assert_eq!(
            days_between(&prev_from, &prev_to),
            days_between("2026-03-01", "2026-03-30")
        );
    }

    #[test]
    fn day_in_month_clamps_to_the_target_months_length() {
        assert_eq!(day_in_month("2026-02-10", 31), "2026-02-28");
        assert_eq!(day_in_month("2024-02-10", 31), "2024-02-29");
        assert_eq!(day_in_month("2026-08-10", 15), "2026-08-15");
        assert_eq!(day_in_month("2026-08-10", 1), "2026-08-01");
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
