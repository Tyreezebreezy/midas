# Phase 015: Insights + Deterministic Forecasting

- Status: COMPLETE
- Owner gate: APPROVED 2026-09-13 (owner handoff: "MIDAS Development Batch 8
  - Insights + Deterministic Forecasting")

## Goal

> MIDAS should clearly explain what happened financially, how the current
> period compares with history, where money came from and went, how net
> worth changed, and what the user's near-term cash position is likely to
> look like based only on known deterministic data.

Evolve Phase 5's Reports into a full Insights surface (Spending, Income,
Merchants, Cash Flow, Net Worth) and add MIDAS's first deterministic,
near-term cash Forecast. No AI, no Monte Carlo simulation, no bank sync, no
speculative investment forecasting.

## Reconnaissance (before writing code)

- `accounts.include_in_net_worth` already existed (Phase 4B) - section 28's
  "Include in Net Worth" toggle needed zero new work.
- No charting dependency exists anywhere in the app - Phase 6.6's
  `CategoryDonut` is hand-rolled SVG. Confirmed via `package.json`: no
  `dependencies` beyond React/Tauri. Kept it that way; Insights/Forecast add
  zero new npm packages.
- `accounting::split_expanded_rows` (Batch 7) was already the one shared,
  split-aware source both `spending_in_range` and
  `reports::spending_by_category` build on. Spending Insights' totals and
  category breakdown reuse it directly rather than a third implementation.
- `budgets::historical_typical`/`DEFAULT_LOOKBACK_MONTHS` (Batch 7, a
  median of the last 6 qualifying months excluding `is_one_time` spending)
  is the exact "typical spending" concept section 14 asks Insights to
  reuse rather than re-defining. Bumped from private to `pub(crate)` - the
  only visibility change needed to share it.
- `accounting::spending_in_range_excluding_one_time` took a required
  `category: &str` (Batch 7 only ever needed one category at a time).
  Generalized to `Option<&str>`, mirroring `spending_in_range`'s own
  existing pattern, so Spending Insights' whole-ledger one-time total
  (section 13) has an all-categories variant to call instead of a second,
  parallel query.
- `accounting::net_worth_as_of` was a private `fn` returning one summed
  i64. Refactored into a new `account_balances_as_of` (per-account raw
  balances at a date) that both `net_worth_as_of` and a new
  `net_worth_change` build on - one query, two consumers, so a period's
  net-worth change and its current total can never disagree about what an
  account's balance was on a given date.
- `recurring_occurrences::upcoming_in_range(conn, from, to, account_id)`
  already existed (Batch 6) and already scopes by confirmed series and
  account - Forecast's event list is this function's output, filtered to
  `Upcoming`/`Due` status and non-`transfer` direction, not a second
  occurrence query.
- `safe_to_spend::eligible_cash`'s Checking/Savings/Cash type filter is the
  exact "genuinely liquid cash" definition section 37 wants Forecast to use
  too. Extracted to `AccountType::is_eligible_cash()` (mirroring the
  existing `is_liability()` method) so Safe to Spend and Forecast can never
  drift apart on what counts as cash - the same class of bug Batch 7 found
  between `spending_in_range` and `spending_by_category`.
- `safe_to_spend`'s reserve setting (`safe_to_spend_reserve_cents`,
  Phase 4B) is the "reserve/preferred minimum balance concept" section 50
  says to reuse - Forecast's warning reads it directly via
  `safe_to_spend::get_reserve`, no new setting.
- `RecurringSeries` already computes `min_amount_cents`/`max_amount_cents`/
  `median_amount_cents` live from linked transaction history (Batch 6, used
  for price-change detection). Forecast's "variable bill -> Estimated"
  distinction (section 41) reuses these fields and the exact same
  `amount_tolerance` window rather than a second variability statistic.
- `Split` (transaction_splits) carries no `payee_id`/`merchant` of its own -
  confirmed a split transaction's category breakdown is a reporting-layer
  concern only, never touching the parent's identity. This is exactly why
  Merchant Insights (section 93: "split categories do not duplicate merchant
  transaction amount") must read `transactions` at the parent level, never
  `split_expanded_rows` - a genuinely different requirement from category
  totals, not an oversight.
- `payees::merge` reassigns `transactions.payee_id` from source to target
  directly - Merchant Insights groups by `payee_id` (falling back to raw
  merchant text), so a merge is reflected automatically with no Insights-side
  merge-awareness code needed.
- Current navigation had a "Insights" sidebar group containing two
  destinations, "Reports" and "Net Worth". Collapsing Reports into a
  5-tab Insights page and folding Net Worth's own page into a 6th... in
  practice a 5th tab (Net Worth) keeps the same **two** sidebar
  destinations the group already had - Insights and a new Forecast - so
  section 4's "do not create five new sidebar destinations" is satisfied
  with zero net growth in sidebar items.
- Neither `"reports"` nor `"networth"` tab ids were referenced by any
  `onNavigate` caller elsewhere in the frontend (grepped, not assumed) -
  renaming them to `"insights"`/`"forecast"` was a safe, zero-blast-radius
  rename, not a compatibility hazard.
- Current migration count: 14 (`0014_budget_intelligence.sql`). This batch
  needed **zero** new migrations - every new figure (spending/income/
  merchant/cash-flow/net-worth-change/forecast) is computed live from the
  existing ledger, matching section 102's explicit preference.

## Shared period model (sections 5, 30-32, 87)

Lives entirely in a new frontend module, `src/lib/period.ts` - the backend
has no period concept at all, exactly as it didn't before this batch.
Every Insights command takes plain `from`/`to` ISO date bounds, identical to
how `reports::spending_by_category(from, to)` already worked; Spending,
Income, Cash Flow, and Net Worth therefore share one date-range computation
by construction and can never disagree about what "this month" means.

- `PeriodMode = "month" | "quarter" | "year"`, values `"YYYY-MM"`,
  `"YYYY-Qn"`, `"YYYY"`.
- `periodRange(mode, value)` -> `{ from, to }`; `shiftPeriod`;
  `currentPeriodValue`; `periodLabel`.
- `periodComparison(mode, value)` implements the one comparison rule
  (section 32): a **completed** period compares its full range against the
  full previous period; the **current, still-in-progress** period compares
  itself against the *same elapsed number of days* in the previous period
  (`Sep 1-13` vs `Aug 1-13`, never `Sep 1-13` vs all of August) - `isPartial`
  tells the UI which language to use ("vs same point last month" vs "vs
  August 2026").
- Tested in `tests/period.test.mjs`: fixed historical values for
  `periodRange`/`shiftPeriod` (including leap February), a completed-period
  comparison, and a live "current period" comparison that asserts the
  *day-count-elapsed invariant* rather than a hardcoded date, so it can
  never become wall-clock-fragile (the Batch 6 lesson, applied here
  proactively).

## Spending Insights (sections 8-15, `insights.rs`)

- `spending_insights(from, to)`: `total_cents` (`spending_in_range`,
  split-aware), `one_time_cents` (`total - spending_in_range_excluding_one_time`),
  `recurring_cents`/`non_recurring_cents` (partitioned by
  `transactions.recurring_series_id IS NOT NULL`), `categories`
  (`reports::spending_by_category`, unchanged).
- Recurring/non-recurring and merchant totals are **not** split-expanded -
  they read `transactions` at the parent level. A split parent's
  `recurring_series_id` (if any) applies to its whole amount; this is a
  documented simplification, since a genuinely recurring transaction being
  split is rare and the amount is never double-counted either way.
- `category_detail(category, from, to, previous_from, previous_to)`:
  current/previous spend, `typical_cents` (`budgets::historical_typical`,
  `None` below 2 qualifying months - identical threshold to Budgets),
  `transaction_count` (a `UNION` of the same split/unsplit shape
  `split_expanded_rows` uses, counting whole transactions rather than
  summing per-split rows - so a split transaction counts once, not twice),
  `one_time_cents`.
- One-time spending always counts in the official total (section 13) - the
  UI never subtracts it, only shows it as a separate line.
- Uncategorized spending surfaces as an ordinary category row (already
  produced by `spending_by_category`'s `COALESCE(..., 'Uncategorized')`)
  with a link to Needs Review, never silently distributed (section 67).

## Income Insights (sections 16-17)

- `income_insights(from, to)`: total, recurring/non-recurring split (same
  `recurring_series_id` partition as spending), `sources` (grouped by
  canonical payee, falling back to merchant text).
- `fetch_income_rows` only ever selects `transaction_type = 'income'` - a
  refund is a different type and can never appear here (section 17); a
  transfer is a third type and is likewise structurally excluded.

## Merchants Insights (sections 18-20, 93)

- `top_merchants`/`merchant_detail` group `transactions` (Purchase/Refund
  only) by `payee_id` when set, else by raw `merchant` text - never
  split-expanded, so a $120 split Walmart transaction shows as one $120
  merchant row, not $80 + $40 stacked on top of the parent (verified live
  and in a dedicated regression test).
- Per-row amount is `accounting::spending_effect` summed per transaction
  (a purchase adds, a refund subtracts) - a merchant that issued more in
  refunds than it was paid nets negative rather than double-showing both
  sides.
- `merchant_detail` additionally reports a category mix, built from each
  matching transaction's own (pre-split) category - a documented
  simplification for the same reason recurring/non-recurring is
  parent-level: category mix is supporting detail, and the headline amount
  it sits under is still exactly correct.
- A payee merge is reflected automatically (see reconnaissance) - verified
  live: merging "Amazon Marketplace" into "Amazon" collapses two rows into
  one under the target's name with the combined total, no double-listing.

## Cash Flow Insights (sections 21-23, 75, 94)

- `cash_flow_insights(from, to)`: `income_cents` (`income_in_range`),
  `spending_cents` (`spending_in_range`), `net_cents` = income - spending.
  Both underlying functions already exclude transfers and card payments
  (Phase 4A), so cash flow inherits that exclusion for free.
- Explicitly documented and UI-labeled as distinct from net worth change
  (section 21) - a manual asset revaluation or a balance adjustment moves
  net worth without ever appearing in cash flow.
- The Cash Flow tab's trend chart reuses `reports::income_vs_spending_trend`
  (Phase 5's existing 6-month table) rendered as a small paired-bar SVG-free
  CSS visualization (`MiniBarTrend`) - no new backend query, no new chart
  dependency (section 62).

## Net Worth Insights (sections 24-29, 76-77, 95)

- Current snapshot/6-month history: unchanged, reused as-is
  (`accounting::get_net_worth`/`get_net_worth_history`).
- New `accounting::net_worth_change(from, to)`: `start_cents` (as of the day
  *before* `from`, so `from` itself is inclusive like every other Insights
  range), `end_cents` (as of `to`), `change_cents`, and per-account
  `contributions` already expressed as **net-worth impact** (a liability
  that grew shows negative, even though its own ledger amount moved
  positive) - contributions are proven in a test to always sum to exactly
  `change_cents`.
- Liabilities are shown as plain positive owed amounts in the UI breakdown
  (unchanged from the existing Net Worth page's convention); manual assets
  follow existing recorded-valuation semantics, never fabricated daily
  appreciation (section 76) - nothing new needed, since valuation is just
  another ledger transaction on the account.

## Deterministic Forecast (sections 33-52, `forecast.rs`)

- **Truth boundary (section 36, non-negotiable)**: `forecast.rs` never
  writes to the database. Every figure is computed on demand from the
  current ledger and Recurring Intelligence's existing occurrence table -
  restarting MIDAS or opening Forecast repeatedly can never create or
  duplicate anything (section 83's idempotence requirement is structural,
  not tested-in separately).
- **Inputs**: current balance of one or more eligible-cash accounts
  (`AccountType::is_eligible_cash` - Checking/Savings/Cash, never a credit
  limit) plus `recurring_occurrences::upcoming_in_range` for those same
  accounts, filtered to `Upcoming`/`Due` status. This single filter
  correctly excludes, by construction: rejected/inactive series
  (`upcoming_in_range` only queries `status = 'confirmed'` series),
  ambiguous matches (`NeedsReview` status), already-posted bills
  (`Matched` status - the real transaction is already inside the current
  balance), skipped occurrences, and stale missed ones.
- **Transfers deliberately excluded** (section 45): a `Direction::Transfer`
  occurrence is skipped. MIDAS has no reliable link from a transfer-direction
  series to the specific *other* account/series it pairs with, so
  projecting only one leg would misrepresent the balance - omitting it is
  more honest than guessing, per the handoff's own escape hatch.
- **Credit-card behavior (section 46)**: a subscription billed to a credit
  card is a series whose `account_id` is the card, not a cash account - it
  is therefore never fetched when forecasting a checking/savings account,
  automatically satisfying "a card charge must not directly drain checking
  cash" with no special-case code. Recurring card payments are out of scope
  for the same reason transfers are (section 47's own "do not guess"
  escape hatch): detection never proposes a series from `transfer`-typed
  transactions at all, so no reliable payment series exists to project.
- **Fixed vs. variable amounts (sections 41-42)**: `is_variable(series)`
  compares the series' own `max_amount_cents - min_amount_cents` against
  the same `amount_tolerance` window price-change detection already uses.
  A variable series projects its **median** magnitude with `Certainty::
  Estimated`; a consistent one projects its literal `expected_amount_cents`
  with `Certainty::Expected`. Only two tiers are surfaced (never a third
  "Known" tag on events) - "known" describes the current starting balance
  itself, a fact rather than a projection, and isn't a per-event label.
- **Ordering/idempotence (section 44)**: events are sorted by
  `(expected_date, merchant)` - the same tiebreak `upcoming_in_range`
  already uses - so two events on the same day always render in the same
  order, and calling `project` twice with no state change produces
  byte-identical output (proven in a test).
- **Lowest balance**: a running minimum over `[current_balance, ...each
  event's post-balance]`, so a mid-horizon dip that later recovers is still
  correctly identified, not just the final balance.
- **Reserve warning (sections 50-51)**: reuses `safe_to_spend::get_reserve`
  directly; a `0` (unset) reserve produces no warning (matching how Safe to
  Spend already treats an unset reserve), otherwise the first date the
  projection drops below it is reported.
- **Horizon**: `horizon_days` is clamped to `[1, 90]` - 90 is
  `recurring_occurrences::HORIZON_DAYS`, the ceiling occurrences are ever
  actually generated to, so a request beyond it could never silently miss a
  real bill that simply hasn't been materialized yet.
- **Scope (section 39)**: one function, `project(conn, account_ids,
  horizon_days)`, serves both the per-account view (one id) and the
  combined view (every eligible-cash id) - not two near-duplicate
  functions.
- **What-if scenarios (section 52) deliberately deferred** - explicitly
  optional in the handoff ("if this meaningfully expands scope, defer it"),
  and this batch's scope (5 Insights tabs plus Forecast, each with its own
  test matrix) already justified stopping at the core forecast.

## UI (sections 55-65, 105)

- `InsightsPage.tsx` replaces `ReportsPage.tsx`/`NetWorthPage.tsx` (both
  deleted) with one destination and 5 internal tabs sharing one
  `PeriodNav`. `CategoryDonut`/`buildDonutSlices` extracted out of
  `ReportsPage.tsx` into their own file so Spending Insights reuses the
  exact existing chart rather than a second one. `CategoryDrilldown`
  (existing) and a new, structurally identical `MerchantDrilldown` reuse
  `transactions::list`'s filter for "see the real transactions," per
  section 19's explicit "no new implementation."
- `ForecastPage.tsx` is a new, separate destination (section 53: Insights
  is "what happened," Forecast is "what's expected") with its own account
  selector and 7/14/30-day horizon control; an event whose amount is
  `Estimated` gets a dashed left border *in addition to* its explicit
  "(Estimated)" text label - never color/line-style alone (sections 54,
  64, 100).
- Comparison lines spell out direction in words ("$174.22 (36.7%) less
  vs...") with a purely decorative arrow, and every positive/negative
  monetary figure that carries directional meaning (Forecast event amounts,
  Net Worth per-account contributions) shows an explicit `+`/`-` rather than
  relying on color alone.
- A real layout bug was found live (see Native Verification): the Insights
  tab strip reused the sidebar's own `.sidebar-link` class, which is
  `display: block; width: 100%` for a *vertical* nav list - inside a `flex`
  tab strip this rendered as a stacked column instead of a row. Fixed with
  dedicated `.insights-tab-button`/`.insights-tab-button-active` styles
  (gold underline, sized to content).

## Tests

424 Rust tests (+40 from Batch 7: 3 in `accounting.rs`, 20 in `insights.rs`,
17 in `forecast.rs`), clean `cargo fmt --check`/`clippy -D warnings`. 13
frontend tests (+6, all in `tests/period.test.mjs`), clean
`prettier`/`tsc --noEmit`/`vite build`. Coverage highlights matching the
handoff's own test matrix (sections 90-96): checking/credit-card/refund/
transfer/split spending; recurring-vs-actual (never counting an unposted
occurrence); one-time exclusion; category-detail zero-baseline handling
(no divide-by-zero); income transfer/refund exclusion; canonical-payee,
raw-merchant-fallback, payee-merge, and split-non-duplication merchant
aggregation; cash-flow transfer/card-payment exclusion and negative net;
net-worth-change contribution-sum invariant and net-worth-exclusion
inheritance; every Forecast exclusion (skipped/matched/inactive/proposed/
transfer/card-scoped), fixed vs. variable certainty, same-day ordering plus
idempotence, lowest-balance tracking, reserve-warning triggering and
non-triggering, and the combined multi-account view.

## Native Verification

Real Tauri/WebView2 app via the established CDP driver
(`window.__TAURI_INTERNALS__.invoke` plus DOM scripting), disposable data
seeded through the app's own commands (3 accounts, 4 recurring series - one
confirmed via detect+confirm specifically to exercise real variable-amount
linkage - splits, a one-time expense, a card purchase/payment pair, and a
payee reassignment).

**A real, pre-existing data-hygiene gap was found and fixed**: prior
batches' own native-verification cleanup only ever archived their test
*accounts*, never the transactions on them - and since neither
`spending_in_range` nor `income_in_range` filter by `account.is_archived`
(only by the transaction's own archive flag, which is correct for
per-account balances but means a *global*, date-scoped total still counts
an archived account's transactions), 80 leftover transactions from Batches
5/5.1/6/7 were silently inflating every current-month global figure -
invisible until Insights' Spending tab became the first feature to
prominently surface a whole-ledger "total spent this month" number. Fixed
by deleting (or archiving, where an import/FK link required it) all 80
stray transactions; this batch's own disposable data was afterward deleted/
archived thoroughly for the same reason, so the ledger ends this batch at a
genuine, verified zero-transaction, zero-account state.

Every headline figure across all 5 Insights tabs and Forecast was
hand-computed from the seeded data and matched the live UI exactly,
including: Spending's same-point comparison (`$300.00`, `-36.7%` vs Aug
1-13), the split-Walmart-never-duplicated merchant total, the payee-merge
collapse, Cash Flow's 6-month trend cross-checked month-by-month against
Spending's own per-month totals, Net Worth's per-account contributions
summing to its own reported change, and Forecast's 7/14/30-day horizon
boundaries (an event dated exactly 7 days out correctly included), its
Estimated-median amount, its reserve-warning date (verified by temporarily
raising the reserve to force a real crossing, then restoring it), and its
per-account vs. combined views. Quarter and Year period modes were also
verified against independently hand-summed totals. A second real bug -
the tab-strip layout regression described above - was found via screenshot,
not text output alone, and fixed before sign-off. Overview, Budgets, and
Recurring were spot-checked afterward and showed no regression (Budget's
"Unbudgeted spending" figures matched Insights' own category breakdown
exactly, as expected since both now ultimately read the same underlying
category totals).

## Accessibility

Donut and trend charts carry full-sentence `aria-label`s (values, not just
"a chart"); Insights tabs use `role="tablist"`/`role="tab"`/
`aria-selected`; mode/horizon/account toggle groups use `role="group"` with
`aria-pressed` on each button. No status is color-only (see UI section
above).

## Screen sizes

1280x800 and 1024x640 verified programmatically
(`document.documentElement.scrollWidth <= window.innerWidth`, not
eyeballed) across every Insights tab and Forecast, both before and after
the tab-strip fix.

## Migration

None. Every new figure is computed live; no new table or column was
needed (section 102's explicit preference).

## Security

No new capabilities, no network calls, no new dependencies. `cargo audit`/
`npm audit` unchanged from Batch 7's accepted baseline.

## Simplification

Reused rather than duplicated: `split_expanded_rows`, `historical_typical`,
`upcoming_in_range`, `eligible_cash`'s type definition, the reserve
setting, the variability/`amount_tolerance` statistic, `CategoryDonut`,
`CategoryDrilldown`'s pattern, and the 6-month income/spending trend.
Explicitly rejected per section 104's list: no second accounting engine, no
per-page spending formula, no analytics cache, no Monte Carlo/AI, no daily
stored forecast rows, no arbitrary scoring, no new charting dependency, no
duplicate transaction-list component, no separate recurring-forecast model,
and budgets are never treated as scheduled spending anywhere in Forecast.

## Explicit non-goals (unchanged from the handoff)

Full Data Health, Financial Signals UI, Weekly Snapshot, Monthly Review, an
AI explainer, SimpleFIN, mobile, CITADEL sync, long-term retirement/
investment-return/tax forecasting, a notification system, a "what-if"
scenario tool, and a full UI redesign outside the affected surfaces.

## Owner decisions applied without escalation

- Phase numbering: `phase-015` (the handoff's own guess, confirmed correct
  since phase-014 was Budget Intelligence).
- Nav shape: one Insights destination (5 tabs) plus one sibling Forecast
  destination - net zero sidebar growth versus the prior Reports/Net Worth
  pair, satisfying "do not create five new sidebar destinations" literally
  and in spirit.
- Recurring/non-recurring and merchant category-mix figures computed at
  the parent-transaction level rather than split-expanded, documented as a
  deliberate simplification (see Spending/Merchants sections above).
- The 80 stray leftover transactions from prior batches were deleted/
  archived as a data-hygiene fix rather than left in place or escalated -
  they were unambiguously disposable test fixtures on already-archived
  accounts, explicitly documented as such in every prior batch's own
  `PROJECT_STATE.md` entry.
