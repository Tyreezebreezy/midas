//! Deterministic parsing of CSV/user-typed money and date strings into the
//! canonical persisted forms: integer cents (never floating point, see
//! ADR-0004) and ISO `YYYY-MM-DD` dates. Every function here rejects
//! malformed input with a descriptive error rather than guessing or
//! silently coercing to zero/today.

/// Parses a dollar-amount string into integer cents. Accepts a leading `$`,
/// thousands-separator commas, a leading `-`, or parentheses for negative
/// (`($125.42)` -> -12542). Rejects anything with more than two decimal
/// places or that otherwise isn't a plain decimal number - malformed input
/// is an error, never a silent zero.
pub fn parse_amount_cents(raw: &str) -> Result<i64, String> {
    let original = raw.trim();
    if original.is_empty() {
        return Err("Amount is empty.".to_string());
    }

    let mut s = original;
    let mut negative = false;

    if let Some(inner) = s.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        negative = true;
        s = inner.trim();
    }
    if let Some(rest) = s.strip_prefix('-') {
        negative = true;
        s = rest.trim();
    }
    if let Some(rest) = s.strip_prefix('$') {
        s = rest.trim();
        // Handles "$-125.42" in addition to the "-$125.42" case above.
        // Deliberately nested here (not a third top-level check) so a
        // malformed double dash like "--5" is left with a stray '-' that
        // fails the digit check below, rather than being accepted.
        if let Some(rest2) = s.strip_prefix('-') {
            negative = true;
            s = rest2.trim();
        }
    }

    if s.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
    {
        return Err(format!("'{original}' is not a valid amount."));
    }

    let no_commas: String = s.chars().filter(|c| *c != ',').collect();
    let mut parts = no_commas.splitn(2, '.');
    let int_part = parts.next().unwrap_or("");
    let frac_part = parts.next();
    if parts.next().is_some() {
        return Err(format!("'{original}' is not a valid amount."));
    }

    let int_digits = if int_part.is_empty() { "0" } else { int_part };
    if int_digits.is_empty() || !int_digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("'{original}' is not a valid amount."));
    }
    let int_value: i64 = int_digits
        .parse()
        .map_err(|_| format!("'{original}' is not a valid amount."))?;

    let frac_cents: i64 = match frac_part {
        None => 0,
        Some(f) if f.len() <= 2 && f.chars().all(|c| c.is_ascii_digit()) => {
            format!("{f:0<2}").parse().unwrap_or(0)
        }
        Some(_) => {
            return Err(format!(
                "'{original}' has more precision than currency supports."
            ))
        }
    };

    let total = int_value
        .checked_mul(100)
        .and_then(|v| v.checked_add(frac_cents))
        .ok_or_else(|| format!("'{original}' is too large."))?;
    Ok(if negative { -total } else { total })
}

/// Combines separate debit/credit CSV columns into one signed `amount_cents`,
/// applying ADR-0004's account-type-aware rule: "debit"/"credit" banking
/// terminology flips balance-effect sign between asset and liability
/// accounts (a card "credit" - a payment - reduces what's owed, i.e. is
/// negative; a checking "credit" - a deposit - increases cash, i.e. is
/// positive).
pub fn combine_debit_credit(
    debit: Option<&str>,
    credit: Option<&str>,
    is_liability_account: bool,
) -> Result<i64, String> {
    let debit = debit.map(str::trim).filter(|s| !s.is_empty());
    let credit = credit.map(str::trim).filter(|s| !s.is_empty());
    match (debit, credit) {
        (Some(_), Some(_)) => {
            Err("Row has both a debit and a credit value; expected exactly one.".to_string())
        }
        (None, None) => Err("Row has neither a debit nor a credit value.".to_string()),
        (Some(d), None) => {
            let magnitude = parse_amount_cents(d)?.abs();
            Ok(if is_liability_account {
                magnitude
            } else {
                -magnitude
            })
        }
        (None, Some(c)) => {
            let magnitude = parse_amount_cents(c)?.abs();
            Ok(if is_liability_account {
                -magnitude
            } else {
                magnitude
            })
        }
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn valid_calendar_date(year: i32, month: u32, day: u32) -> bool {
    (1900..=2100).contains(&year)
        && (1..=12).contains(&month)
        && day >= 1
        && day <= days_in_month(year, month)
}

/// Parses a constrained, deterministic set of common US statement date
/// formats into ISO `YYYY-MM-DD`: `YYYY-MM-DD`, `MM/DD/YYYY`, `MM/DD/YY`
/// (assumed 20YY), and `MM-DD-YYYY`. Anything else - including any format
/// this parser can't unambiguously place, per Phase 2's "route for
/// correction rather than assume" rule - is an error, not a guess.
pub fn parse_date(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    let digits_and_seps: Vec<&str> = if s.contains('-') {
        s.splitn(3, '-').collect()
    } else if s.contains('/') {
        s.splitn(3, '/').collect()
    } else {
        return Err(format!("'{raw}' is not a recognized date format."));
    };
    if digits_and_seps.len() != 3 {
        return Err(format!("'{raw}' is not a recognized date format."));
    }
    let all_numeric = digits_and_seps
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    if !all_numeric {
        return Err(format!("'{raw}' is not a recognized date format."));
    }

    let (year, month, day) = if s.contains('-') && digits_and_seps[0].len() == 4 {
        // ISO: YYYY-MM-DD
        let year: i32 = digits_and_seps[0].parse().unwrap_or(0);
        let month: u32 = digits_and_seps[1].parse().unwrap_or(0);
        let day: u32 = digits_and_seps[2].parse().unwrap_or(0);
        (year, month, day)
    } else {
        // US convention: MM/DD/YYYY, MM/DD/YY, or MM-DD-YYYY.
        let month: u32 = digits_and_seps[0].parse().unwrap_or(0);
        let day: u32 = digits_and_seps[1].parse().unwrap_or(0);
        let year_raw = digits_and_seps[2];
        let year: i32 = match year_raw.len() {
            4 => year_raw.parse().unwrap_or(0),
            2 => 2000 + year_raw.parse::<i32>().unwrap_or(0),
            _ => return Err(format!("'{raw}' is not a recognized date format.")),
        };
        (year, month, day)
    };

    if !valid_calendar_date(year, month, day) {
        return Err(format!("'{raw}' is not a valid calendar date."));
    }
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_negative_amounts() {
        assert_eq!(parse_amount_cents("125.42").unwrap(), 12542);
        assert_eq!(parse_amount_cents("-125.42").unwrap(), -12542);
        assert_eq!(parse_amount_cents("0").unwrap(), 0);
        assert_eq!(parse_amount_cents("5").unwrap(), 500);
    }

    #[test]
    fn parses_currency_symbol_and_parentheses_negative() {
        assert_eq!(parse_amount_cents("$125.42").unwrap(), 12542);
        assert_eq!(parse_amount_cents("($125.42)").unwrap(), -12542);
        assert_eq!(parse_amount_cents("(125.42)").unwrap(), -12542);
    }

    #[test]
    fn parses_thousands_separators() {
        assert_eq!(parse_amount_cents("1,234.56").unwrap(), 123456);
        assert_eq!(parse_amount_cents("$12,345").unwrap(), 1234500);
    }

    #[test]
    fn rejects_malformed_amounts_instead_of_defaulting_to_zero() {
        assert!(parse_amount_cents("").is_err());
        assert!(parse_amount_cents("abc").is_err());
        assert!(parse_amount_cents("12.34.56").is_err());
        assert!(parse_amount_cents("12.345").is_err());
        assert!(parse_amount_cents("$").is_err());
        assert!(parse_amount_cents("--5").is_err());
    }

    #[test]
    fn combine_debit_credit_respects_account_type() {
        // Asset account: a debit (withdrawal) is negative, a credit (deposit) is positive.
        assert_eq!(
            combine_debit_credit(Some("50.00"), None, false).unwrap(),
            -5000
        );
        assert_eq!(
            combine_debit_credit(None, Some("50.00"), false).unwrap(),
            5000
        );
        // Liability account: a debit (charge) is positive (owe more), a credit (payment) is negative (owe less).
        assert_eq!(
            combine_debit_credit(Some("50.00"), None, true).unwrap(),
            5000
        );
        assert_eq!(
            combine_debit_credit(None, Some("50.00"), true).unwrap(),
            -5000
        );
    }

    #[test]
    fn combine_debit_credit_rejects_both_or_neither() {
        assert!(combine_debit_credit(Some("5"), Some("5"), false).is_err());
        assert!(combine_debit_credit(None, None, false).is_err());
        assert!(combine_debit_credit(Some(""), None, false).is_err());
    }

    #[test]
    fn parses_supported_date_formats_to_iso() {
        assert_eq!(parse_date("2026-03-04").unwrap(), "2026-03-04");
        assert_eq!(parse_date("03/04/2026").unwrap(), "2026-03-04");
        assert_eq!(parse_date("03/04/26").unwrap(), "2026-03-04");
        assert_eq!(parse_date("03-04-2026").unwrap(), "2026-03-04");
        assert_eq!(parse_date("1/5/2026").unwrap(), "2026-01-05");
    }

    #[test]
    fn rejects_invalid_calendar_dates() {
        assert!(parse_date("2026-02-30").is_err()); // Feb has 28/29 days
        assert!(parse_date("2026-13-01").is_err()); // no month 13
        assert!(parse_date("2026-00-01").is_err());
        assert!(parse_date("02/29/2025").is_err()); // 2025 is not a leap year
    }

    #[test]
    fn leap_year_february_29_is_valid_only_in_leap_years() {
        assert_eq!(parse_date("2024-02-29").unwrap(), "2024-02-29");
        assert!(parse_date("2023-02-29").is_err());
        assert!(parse_date("2100-02-29").is_err()); // divisible by 100, not 400
        assert_eq!(parse_date("2000-02-29").unwrap(), "2000-02-29");
    }

    #[test]
    fn rejects_unrecognized_or_ambiguous_date_strings() {
        assert!(parse_date("March 4, 2026").is_err());
        assert!(parse_date("not a date").is_err());
        assert!(parse_date("").is_err());
        assert!(parse_date("2026/03/04/05").is_err());
    }
}
