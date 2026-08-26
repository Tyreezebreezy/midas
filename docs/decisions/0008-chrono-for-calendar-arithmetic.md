# ADR-0008: Adopt `chrono::NaiveDate` for calendar arithmetic

- Status: accepted
- Date: 2026-08-25
- Supersedes: ADR-0006 (hand-rolled date arithmetic)

## Context and problem

ADR-0006 hand-rolled exactly two operations for Phase 3's recurring detection: days-between and add-N-days, both built on the well-known `days_from_civil`/`civil_from_days` conversion. It explicitly scoped itself narrowly and named its own exit condition: "If a future phase needs... broader date parsing [or arithmetic], that is a new requirement calling for its own ADR and likely a real date/time crate at that point."

Phase 4B is that future phase. Pay cycles, monthly budgets, savings-goal target dates, and historical cycle/month comparisons all need **month- and year-aware** arithmetic - "same day next month," "start of this budget month," "N pay cycles ago" - not just day-counting. Month arithmetic is exactly where hand-rolled calendar code gets subtly wrong: adding a month to January 31st has no single obviously-correct answer (clamp to Feb 28/29? overflow to March 3rd?), and every such edge case is a fresh opportunity for a silent, hard-to-notice bug in code nobody has independently audited.

## Options considered

1. Keep hand-rolling: add `add_months`, `start_of_month`/`end_of_month`, and day-of-week to the existing `recurring.rs` primitives.
2. Adopt `chrono` (`NaiveDate`), the de facto standard Rust date/time crate.
3. Adopt `time`, a well-regarded alternative with a similar scope.

## Decision

Option 2, scoped tightly: `chrono = { version = "0.4", default-features = false, features = ["std"] }`. This enables `NaiveDate`/`NaiveDateTime` with no timezone database, no OS clock bindings, and no locale support - checked with `cargo tree -p chrono`, which shows exactly one transitive dependency (`num-traits`). MIDAS still has no timezone concept (transaction dates remain bare `YYYY-MM-DD` strings, as ADR-0006 established and this decision does not revisit); only the "hand-roll vs. depend on a crate" half of the original tradeoff changes, not the "no timezones" half.

`chrono` over `time`: both are mature and well-audited; `chrono` is the more widely adopted default across the Rust ecosystem, which matters more here than a marginal API-design preference for a solo-maintained personal project - the easier choice to find help with later wins a close call.

## Rationale

- The trigger ADR-0006 itself named - broader arithmetic needed beyond simple day-counting - has arrived. Month-aware arithmetic is genuinely more failure-prone to hand-roll correctly than the day-counting Phase 3 needed, and it is no longer "a small, fully-verifiable-by-unit-test surface area" the way `days_from_civil` was: the space of month/year boundary edge cases (leap years crossed by an annual goal date, 31-day-month clamping every single month for a recurring monthly budget) is large enough that "verify a handful of reference points" stops being convincing evidence of correctness.
- The dependency cost is genuinely minimal (one transitive crate, no timezone/OS surface), which was the deciding factor ADR-0006 asked future phases to re-weigh - this is not "add a library reflexively," it is confirming the narrow, audited option is in fact narrow before taking it.
- `recurring.rs`'s existing hand-rolled `days_from_civil`/`civil_from_days`/`days_between`/`add_days` are replaced by `chrono::NaiveDate` equivalents (`.signed_duration_since()`, `+ Days::new(n)`) rather than kept alongside the new dependency - two parallel date implementations in the same codebase would be a real maintenance hazard the owner's review would rightly flag.

## Consequences and accepted tradeoffs

- `money.rs`'s existing hand-rolled calendar _validation_ (`is_leap_year`, `days_in_month`, `valid_calendar_date`, used to validate a parsed date string before it enters the ledger) is retired in favor of `NaiveDate::from_ymd_opt(...).is_some()`, which is exactly the same check with one call instead of three hand-written functions - not left duplicated alongside the new dependency.
- `recurring.rs`, `import.rs`/`money.rs` date parsing, and the new Phase 4B pay-cycle/budget/goal-date code all now share one calendar implementation instead of MIDAS maintaining its own.
- If a future phase needs actual timestamps (time-of-day, timezones), that remains a distinct, larger decision this ADR does not make - `chrono`'s timezone-aware types are not enabled here, and won't be reached for accidentally since the `"clock"`/`"wasmbind"` features that pull them in are off.
