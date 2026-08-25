# ADR-0006: Hand-rolled date arithmetic instead of a date/time crate

- Status: accepted
- Date: 2026-08-25
- Supersedes: none

## Context and problem

Recurring-obligation detection (product handoff section 10) needs to compute the number of days between two ISO dates (to classify a gap as weekly/monthly/etc.) and add a day-count to a date (to compute the next expected occurrence). MIDAS has no date/time dependency yet - `money.rs`'s calendar validation (ADR-0004 era) already hand-rolled a leap-year check and days-in-month table rather than adding one.

## Options considered

1. Add `chrono` or `time` (the two mainstream Rust date/time crates) and use their date-difference and date-arithmetic APIs.
2. Hand-roll the two operations needed (`days_from_civil` / `civil_from_days`) using Howard Hinnant's well-known, public-domain algorithm for proleptic-Gregorian date-to-day-number conversion and its inverse.

## Decision

Option 2. `src-tauri/src/recurring.rs` implements `days_from_civil`, `civil_from_days`, and the `days_between`/`add_days` helpers built on them - about 30 lines total, each verified against known reference points in tests (`days_between("1970-01-01", "1970-01-01") == 0`, leap-year boundaries, etc.).

## Rationale

- The actual surface area needed is exactly two operations: "days between two calendar dates" and "a date plus N days." Both reduce to the same day-number conversion run twice. A full date/time crate's real value - timezone handling, parsing dozens of formats, duration types, calendar edge cases beyond the Gregorian proleptic calendar - is unneeded here: MIDAS already normalizes every date to a bare ISO `YYYY-MM-DD` string with no time-of-day or timezone component (transaction dates, not timestamps), so there is no timezone problem to solve.
- The algorithm itself is not obscure or risky to hand-roll: it is the standard, widely-cited "days_from_civil" formula, small enough to verify completely with unit tests against known reference dates rather than trusting an opaque library call.
- Matches the standing pattern already set in `money.rs` (ADR-0004-era: leap-year/days-in-month hand-rolled instead of a dependency for the same reason) - MIDAS's Rust dependency list has stayed to exactly what each feature concretely required (`rusqlite`, `serde`, `csv`, `sha2`, `serde_json`), and this would be the first dependency added for convenience rather than because hand-rolling was genuinely risky.

## Consequences and accepted tradeoffs

- If a future phase needs real timezone-aware timestamps (not just calendar dates) or broader date parsing, that is a new requirement calling for its own ADR and likely a real date/time crate at that point - this decision does not try to anticipate it.
- The valid range is the proleptic Gregorian calendar for any date MIDAS would realistically encounter (statement dates are not going to be BC-era or far-future); no explicit range guard was added beyond `money.rs`'s existing 1900-2100 validation on all parsed dates.
