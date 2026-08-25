# Phase 003: Merchant rules and recurring/bills detection

- Status: COMPLETE
- Owner gate: APPROVED — Phase 3 scope derives from the original Build Handoff's own Build Order line ("Merchant normalization. Categories. Reusable corrections/rules. Recurring candidate detection. Recurring confirmation. Bills/upcoming view."), authorized by the owner's terse "push your existing main branch... then continue with the approved MIDAS work" once Gitea was verified reachable.

## Goal

Deterministic, explainable cleanup on top of the Phase 2 ledger: reusable merchant-normalization rules applied during CSV staging (never rewriting history silently), and recurring-bill detection that proposes candidates for the owner to confirm or reject rather than asserting a pattern as fact.

## Scope

- Merchant rules: exact/contains/starts-with matching on `raw_description`, optional account scoping (account-specific outranks global), priority tiebreak, most-recent as final tiebreak.
- Rule application inside CSV staging only — a rule's `normalized_merchant`/`category` overrides the CSV's own raw values for newly staged rows. Rules never rewrite transactions already committed to the ledger.
- Recurring detection: group by merchant within an account, require >= 3 occurrences whose date gaps all classify to the same cadence (weekly/biweekly/monthly/quarterly/annual, each with a tolerance window) and whose amounts stay within tolerance (20% of the average or $2, whichever is larger).
- Detected patterns are always `status = 'proposed'` — never silently promoted to a trusted bill. Confirm retroactively links matching un-linked transactions to the series; reject does not link anything.
- Bills/upcoming view: confirmed series only, sorted by next expected date, flagged when the next occurrence is overdue (`is_possibly_missed`) or the most recent amount drifted from the expected amount (`is_price_changed`).
- Rules UI (list/create/delete) and Recurring & Bills UI (detect, proposed list with confirm/reject, upcoming bills table), wired into the main nav.

## Explicit non-goals

- AI-based or fuzzy/similarity merchant matching — exact/contains/starts-with only, per the constitution's no-AI-categorization rule.
- Editing or deleting a rule after creation, or reordering priorities via drag/drop — create/delete only; the owner did not ask for edit, and a delete-and-recreate covers correction for the rule volumes a personal ledger accumulates.
- Automatically applying a newly created/edited rule retroactively to already-committed transactions — rules affect staging only (see Scope); retroactive re-normalization of history is a distinct, larger operation (would need its own confirmation flow) that was not requested.
- A "create rule from this transaction correction" shortcut in the transaction editor — a real usability nice-to-have, but nothing in the Build Order line requires it and no owner request named it.
- Editing a confirmed/rejected recurring series' cadence or amount by hand — reject and let re-detection propose again covers correction without a second parallel editing path.
- Bill reminders/notifications — "Bills/upcoming view" asked for visibility (the table, the overdue/price-changed flags), not a notification system.

## Verified current state

- Builds on Phase 3 checkpoint 1 (`8e3bdf4`, backend: `rules.rs`, `recurring.rs`, migration `0003_rules_and_recurring.sql`, rules wired into `import.rs` staging).
- New Rust dependencies: none — `rules.rs`/`recurring.rs` use only `rusqlite`/`serde`, already present since Phase 1/2. Date arithmetic is hand-rolled (Howard Hinnant `days_from_civil`/`civil_from_days`) rather than adding a date/time crate, since MIDAS only needs bare-ISO-date day-counting with no timezones (ADR-0006).
- New npm dependencies: none.
- `src-tauri/capabilities/default.json`: still zero plugin permissions. 23 app-defined commands total (16 from Phase 2 + 7 new: `list_rules`/`create_rule`/`delete_rule`, `list_recurring`/`detect_recurring`/`confirm_recurring`/`reject_recurring`).
- New frontend: `src/RulesPage.tsx`, `src/RecurringPage.tsx`, `rulesApi`/`recurringApi` in `src/api/midas.ts`, two new nav tabs in `src/App.tsx`, one new CSS rule (`.section-heading`) in `src/App.css` — everything else reuses existing table/dialog/list/badge styling from Phase 1/2 rather than inventing new UI patterns.

## Constraints and architecture boundaries

- `rules.rs` splits DB access from matching: `relevant_rules(conn, account_id)` fetches once per staging call, `best_match(rules, raw_description)` is a pure function with no DB access, called per row. An earlier single `apply_rules()` that combined both was deleted after clippy flagged it as dead code once the split landed (only its own tests called it) — its callers moved to the real fetch-once-match-many pattern.
- `recurring.rs` follows the same pure-function-plus-thin-command pattern as every other domain module: `detect_candidates`, `confirm`, `reject`, `list` all take `&Connection`, testable via `Connection::open_in_memory()` with no Tauri runtime.
- Detection is idempotent: a merchant that already has any series row (proposed, confirmed, or rejected) for an account is skipped on the next detect call, so re-running detection never produces duplicate proposals.
- `RecurringSeries`'s `last_occurrence_date`/`last_amount_cents`/`is_price_changed`/`is_possibly_missed` are computed at read time in `hydrate()` (a direct query against linked transactions plus `SELECT date('now')`), not stored — there is nothing to keep in sync if the ledger changes underneath a series.

## Security and privacy considerations

- No new network authority, no new fs/dialog plugin permissions — rules and recurring detection are pure local computation over the existing ledger.
- All verification used obviously-fake fixture data (account "Phase3 Recurring Test", merchant "NETFLIX.COM", rule matching literal string "SQ *COFFEE"); no real financial data touched this repository or any dev database. The seeded account and transactions were deleted/archived via the app's own commands immediately after verification, before the dev process was stopped.
- Gitleaks (8.30.1, full history, 10 commits): no leaks found.
- Trivy (0.72.0, vuln+secret): 0 secrets; 1 MEDIUM (`glib`, pre-existing since Phase 0/1, Linux-only backend, does not compile into the Windows binary — ADR-0002). No new findings — `Cargo.lock`/`package-lock.json` are byte-for-byte unchanged from Phase 2 since Phase 3 added no dependencies.
- `cargo audit`: 0 vulnerabilities, same 17 pre-existing "unmaintained"/"unsound" transitive warnings as Phase 2 (all inherited through Tauri's own dependency graph, e.g. the GTK3 bindings only used by the Linux backend), no new ones.

## Acceptance criteria (Build Order line, each item verified against the live app)

- [x] Merchant normalization — live-verified: a `contains "SQ *COFFEE"` rule scoped to one account, created through the real Rules UI dialog, correctly rewrote a freshly staged CSV row (`SQ *COFFEE SHOP 99 MIAMI FL`) to merchant "Coffee Shop", category "Dining" via `stage_import`.
- [x] Categories — the same rule verification also covers category assignment; `rules.rs` tests additionally cover a rule that sets category without touching merchant, and the reverse.
- [x] Reusable corrections/rules — rules persist independently of any one import and apply to every subsequent staging call; account-specific-beats-global and priority-tiebreak precedence covered by 8 backend tests plus the live UI test above.
- [x] Recurring candidate detection — live-verified: 4 seeded monthly "NETFLIX.COM" charges (same amount, ~30 days apart) on a real account produced exactly one proposed series via the real "Detect recurring" button, correctly classified `cadence: monthly`, `expected_amount_cents: -1599`.
- [x] Recurring confirmation — live-verified: clicking "Confirm" on the proposed item moved it out of the proposed list and into the upcoming-bills table; cross-checked against `list_recurring` showing `status: confirmed` with `last_occurrence_date: 2026-07-15` correctly computed (proof the backend's `UPDATE transactions SET recurring_series_id = ...` linking ran, since `hydrate()` can only find that date by querying linked transactions).
- [x] Bills/upcoming view — the confirmed series above rendered in the "Upcoming bills" table with a correctly computed "Possibly missed" badge (the algorithm's `next_expected_date`, 2026-08-14, fell before the current date, 2026-08-25 — a genuine, not fabricated, overdue case produced by ordinary date arithmetic against real seeded history).

## Test strategy

1. Rust unit tests: 78 total (55 after Phase 2; +23 in Phase 3) — `rules.rs` covers exact/contains/starts-with matching, case-insensitivity, account-specific-beats-global, priority and recency tiebreaks; `recurring.rs` covers date-math reference points (Unix epoch, a leap-year boundary), cadence classification at tolerance-window edges, detection confidence thresholds (rejects <3 occurrences, irregular gaps, inconsistent amounts), idempotence across repeated detect calls, confirm/reject linking behavior, and price-change/possibly-missed flag computation.
2. `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo audit` — all clean.
3. `npm run check` (prettier, tsc, node test, vite build) — clean.
4. Ponytail review of the frontend diff: no findings (every new UI class reuses Phase 1/2 CSS; no speculative state, memoization, or abstractions) — see chat record; backend was reviewed inline during construction (dead-code deletion of the original `apply_rules()`, `SeriesStatus::as_sql_str()` wired into real call sites instead of hardcoded SQL literals).
5. Real end-to-end verification against the live native app over the WebView2 CDP debug port: seeded an account and 4 recurring transactions via the app's own `create_transaction` command (not direct DB writes), then drove the actual Rules and Recurring & Bills pages through real DOM clicks/form fills/dialog submissions (native-setter trick for React-controlled inputs) — nav tab routing, rule creation through the real dialog, rule deletion, `stage_import` re-run to prove the rule actually affects staging (not just its own table row), detect/confirm/reject button clicks, and the resulting upcoming-bills table contents. Screenshots captured via CDP `Page.captureScreenshot` for both pages. Seeded data archived/deleted through the app's own commands afterward.

## Owner decisions

None required for this phase — Phase 3 scope was pre-derived from the Build Handoff's own Build Order line and the owner's instruction to continue with previously-approved work.

## Hard-stop conditions

- Any need for AI-assisted or fuzzy merchant matching — none arose; exact/contains/starts-with covered every case exercised.
- Any need for a new Tauri plugin or broader filesystem/network authority — none arose.
- Retroactive rule application turning out to be required for Phase 3 correctness — it is not; scoping rules to staging-only keeps the ledger's audit trail (what a CSV actually said vs. what a rule normalized it to) intact, and nothing in the Build Order line asked for retroactive rewriting.
