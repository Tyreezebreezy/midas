# Phase 005: Overview and Reports

- Status: COMPLETE
- Owner gate: APPROVED — "This is where all the machinery Claude just built comes together. The Overview should finally become the MIDAS home screen, prioritizing: 1. Safe to Spend 2. Upcoming Bills 3. Current Pay Cycle 4. Spending 5. Needs Review 6. Savings Goals. Then supporting information: cash position, net worth, assets/liabilities, budget health, recent transactions, income vs spending, simple trends. And Reports/Trends should provide useful drilldowns rather than a bunch of decorative finance charts."

## Goal

Turn every feature built across Phases 1-4C into one glanceable home screen, and give Reports a genuine "where did my money go" drilldown instead of a second dashboard of charts.

## Scope

- **Overview page** (`overview.rs`, new `get_overview` command): a single aggregation over every existing module - Safe to Spend, Upcoming Bills, Current Pay Cycle, Spending, Needs Review, and Savings Goals as the six priority cards, in that order; then cash position/net worth, budget health, income vs. spending this month, a 6-month net worth trend, and recent transactions as supporting sections. No new calculation logic - every figure is a direct call into an already-tested Phase 4 function.
- **Overview becomes the default tab** - `App.tsx`'s initial `Tab` state changed from `"accounts"` to `"overview"`, per the owner's explicit "should finally become the MIDAS home screen."
- **Reports page** (`reports.rs`, two new commands): spending-by-category for a selected month, highest first, with each category row expandable inline to show the real transactions behind it (the "drilldown"); an income-vs-spending trend for the last 6 months plus the current month to date.
- Two small new domain primitives nothing existing computed: `accounting::income_in_range` (income has no aggregate the way `spending_effect` gives spending one) and `pay_cycle::upcoming_bills` (the same "confirmed, negative amount" definition `obligations_before` already sums, exposed as the list itself rather than a total).
- `src/lib/monthValue.ts`: the month-value/month-range helpers `BudgetsPage.tsx` already had, extracted so `ReportsPage.tsx` - a second real caller - doesn't duplicate them.

## Explicit non-goals

- A charting library or any decorative visualization - every number here is a table or a plain list, matching the owner's explicit "not a bunch of decorative finance charts" and the app's existing visual language (no chart dependency exists anywhere in MIDAS).
- A second transaction-browsing backend for the drilldown - it reuses `transactions::list`'s existing category+date filter, the same one `TransactionsPage` already calls.
- Duplicating Net Worth's own dedicated page/breakdown inside Reports - Net Worth already has a full tab; Reports covers the two things nothing else does (category breakdown, income vs. spending).
- Cross-tab "jump to X" links from Overview's cards - the nav bar is already one click away for every section; wiring a `setTab` callback through every card for marginal convenience wasn't worth the plumbing. Skipped, add if the owner asks.
- A `cash_position_cents` field duplicating `eligible_cash_cents` - `SafeToSpendResult` already carries it on both variants, so the Overview response reuses that field instead of computing the same number twice.

## Verified current state

- Builds on Phase 4C (`db2a442`).
- New Rust dependencies: none. New npm dependencies: none.
- `src-tauri/capabilities/default.json`: still zero plugin permissions.
- Schema: no migration - Overview and Reports are pure reads over the existing tables.
- 45 app-defined commands total (42 from Phase 4B/4C + 3 new: `get_overview`, `get_spending_by_category`, `get_income_vs_spending_trend`).
- New Rust modules: `overview.rs` (one aggregating `build`/`get_overview`), `reports.rs` (`spending_by_category`/`income_vs_spending_trend` plus their commands). `accounting.rs` gained `income_in_range`; `pay_cycle.rs` gained `upcoming_bills`.
- New frontend pages: `OverviewPage.tsx`, `ReportsPage.tsx`. New lib: `lib/monthValue.ts` (extracted from `BudgetsPage.tsx`, which now imports from it instead of holding its own copy). New CSS: `.overview-grid`/`.overview-card`/`.overview-mini-list` (the card grid), `.category-row` (the clickable drilldown row) - everything else reuses Phase 1-4C table/list/badge classes.
- `App.tsx`: two new tabs (`overview`, `reports`); default tab changed to `overview`.

## Constraints and architecture boundaries

- Every figure in `OverviewResult` is a direct call into an existing, already-tested Phase 4 function (`safe_to_spend::calculate`, `pay_cycle::summarize`/`upcoming_bills`, `savings_goals::list`, `budgets::list_for_month`, `accounting::net_worth_breakdown`/`net_worth_history`/`income_in_range`/`spending_in_range`, `needs_review::list`, `transactions::list`) - `overview.rs` composes, it never recomputes.
- List fields that could grow unbounded on a long-lived account are capped for a glanceable screen (`upcoming_bills` to 5, `needs_review_items` preview to 5, `recent_transactions` to 8) - but `needs_review_count` reports the true total separately, so the count badge is never silently truncated along with the preview list.
- `reports::spending_by_category` reuses `TransactionType::from_sql_str` and `accounting::spending_effect` directly rather than re-deriving spending classification, so a category total is never classified differently than `spending_in_range` would classify the same transaction.
- `income_vs_spending_trend` mirrors `net_worth_history`'s existing "N months plus today" shape rather than inventing a different trend convention.
- All calculation lives in Rust; both new pages only format and lay out server-computed numbers, same as every Phase 4B page.

## Security and privacy considerations

- No new network authority, no new fs/dialog plugin permissions - both new modules are local computation over the existing ledger.
- All verification used obviously-fake fixture data (accounts named "Checking"/"Credit Card", merchants like "ACME Payroll," "Grocery Store," "Landlord LLC"); no real financial data touched this repository or any dev database. Every test account, transaction, budget, savings goal, and recurring series created during live verification was deleted, archived, or rejected through the app's own commands before this session ended - confirmed via a final zero-state check (`txnCount: 0, activeAccounts: 0, budgetCount: 0, activeGoals: 0`).
- A cleanup gap from the Phase 4C live-verification session (a script that threw partway through its cleanup loop, archiving one leftover test account without deleting its transaction first) was found and fixed during this phase's verification pass, not left for later.
- Gitleaks (8.30.1): full history and working-tree diff both clean.
- Trivy (0.72.0, vuln+secret): 0 secrets; 1 MEDIUM (`glib`, pre-existing since Phase 0/1), unchanged. No new findings - Phase 5 added zero dependencies.
- `cargo audit`: 0 vulnerabilities, same 17 pre-existing allowed transitive warnings, unchanged.

## Acceptance criteria (owner's priority list, each verified against the live app)

- [x] **Overview is the home screen** - `App.tsx`'s default tab is `overview`; live-verified the app opens directly on it.
- [x] **Priority 1-6 in the owner's exact order** - live-verified with a realistic seeded scenario (two accounts, income, categorized purchases, a confirmed recurring bill, a budget, a reserved savings goal): Safe to Spend correctly went negative (-$1,157.00 = $843 eligible cash − $1,500 obligation − $500 reserved goal) rather than false-precision-clamping to zero; Upcoming Bills showed the confirmed "Landlord LLC" bill with its correctly-extrapolated next date; Current Pay Cycle showed the right cycle start/next payday; Spending showed the correct month-to-date total including a credit-card purchase (per ADR-0004, a CC purchase is spending even though its amount is stored positive); Needs Review and Savings Goals both rendered correctly.
- [x] **Supporting information** - cash position, net worth/assets/liabilities, budget health, income vs. spending, a 6-month trend, and recent transactions all present and, in the same live run, hand-verified against the actual ledger arithmetic including a deliberately messy transaction-date ordering (historical purchases dated before the account's own opening-balance anchor) - `net_worth_as_of` produced the mathematically correct snapshot at every trend point despite that.
- [x] **Reports provides real drilldowns, not decorative charts** - live-verified: spending-by-category rendered three categories sorted highest-first with correct percentages; clicking "Groceries" expanded it inline to the two real transactions behind that $115.00 total, with correct dates and amounts; the income-vs-spending trend table matched the same 6-month-plus-today shape as Net Worth's own trend.

## Test strategy

1. Rust unit tests: 161 total, up from 153 at the end of Phase 4C (+8) - confirmed via `cargo test` output. New coverage: `accounting.rs` (`income_in_range`, +1), `pay_cycle.rs` (`upcoming_bills`, +1), `overview.rs` (+3: full composition, capped-list behavior with accurate counts, month-to-date scoping), `reports.rs` (+3: category sorting/omission of zero-net categories/exclusion of transfers and balance adjustments, uncategorized bucket, 6-month trend scoping).
2. `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo audit` - all clean.
3. `npm run check` (prettier, tsc, node test, vite build) - clean.
4. Ponytail review of the full diff: two findings applied - a stray test bug where seeded recurring-bill transactions never set a merchant (all six fell under one default merchant, so the detector correctly rejected them as inconsistent, masking the intended coverage) fixed by threading a real merchant through the test helper; and `OverviewPage.tsx`'s pay-cycle cadence labels, which had been copied from `RecurringPage`'s bill-cadence `Cadence` labels (weekly/biweekly/monthly/quarterly/annual) instead of matching `PaySchedule`'s actual, narrower type (weekly/biweekly/monthly/**semimonthly**) - a semimonthly schedule would have silently rendered its raw type string. Fixed by keying the label map to `PaySchedule["schedule_type"]` directly so the compiler enforces exhaustiveness.
5. Real end-to-end verification against the live native app over the WebView2 CDP debug port: seeded a realistic multi-account scenario, reloaded to pick up the Rust-side commands, and hand-verified every figure on both pages against the actual ledger arithmetic (detailed above) - not simulated, not assumed from code reading. A genuine cleanup gap left over from the _previous_ phase's live verification (one leftover transaction on an archived test account) was discovered and fixed as part of this pass's own zero-state check, not silently ignored.

## Owner decisions

None required - fully specified in the owner's Phase 5 authorization message, delivered in the same message as Phase 4C's authorization with an explicit instruction to proceed into Phase 5 once Phase 4C was complete.

## Hard-stop conditions

- Any requirement that couldn't be satisfied by composing existing Phase 4 functions - none arose; the only new logic was two small primitives (`income_in_range`, `upcoming_bills`) that were pure gaps, not redesigns.
- Any need for a charting library or new Tauri plugin/broader filesystem/network authority - none arose.
