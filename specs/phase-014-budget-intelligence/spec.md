# Phase 014: Budget Intelligence

- Status: COMPLETE
- Owner gate: APPROVED 2026-09-13 (owner handoff: "MIDAS Development Batch 7
  - Budget Intelligence")

## Goal

> Budgets should tell the user not only how much they have spent, but
> whether they are on pace, what is normal for that category, what remains
> available, and how irregular/future spending should be planned.

Mature the existing budget feature - historical averages, spending pace,
rollover, future-month adjustments, category-level behavior - without
turning MIDAS into an envelope-accounting application. No AI, no
forecasting engine, no Insights redesign.

## Reconnaissance (before writing code)

- `budgets` (migration 0005, Phase 4B) was already one row per category
  **per month** (`UNIQUE(category, month)`), not a single "base" row a
  monthly override would sit on top of. This single fact reshaped the whole
  batch: historical stability (section 71) and future-month overrides
  (sections 22-25) are both already structural properties of the existing
  schema, not new mechanisms to build. Editing October's row cannot
  possibly rewrite August's, because there is no shared row for it to
  rewrite - each month already stands alone. The real, only gap was UX:
  nothing carried a steady budget forward automatically, so the owner would
  have had to retype it every month (`copy_month` closes this).
- `budgets.classification` already existed with `'fixed'`/`'flexible'` -
  the Fixed/Flexible/Discretionary request (section 26) was a one-value
  CHECK-constraint widening, not a new column or a Category Groups
  migration.
- **A real, pre-existing bug was found**: `accounting::spending_in_range`
  (which `budgets::actual_spending_for` calls for every budget's actual
  figure) filtered `transactions.category` directly - the _pre-split_
  label. `reports::spending_by_category` had already been made split-aware
  in Batch 5 (a `UNION ALL` expanding each split transaction into its
  children's own category/amount), but that fix was never carried over to
  `spending_in_range`. A split transaction's spending was therefore
  attributed to whichever category the parent happened to carry before
  being split, never divided across its real categories - directly
  contradicting section 33/82's explicit requirement and section 84's "Budget
  and Reports must not disagree." Fixed at the root (see "Actual spending"
  below) before any budget-specific feature was built on top of it, since
  every subsequent feature (pace, historical baseline, suggestions) depends
  on this figure being correct.
- `categories::rename` already updated `budgets.category` (skipping any
  month where a collision would occur) - section 61's requirement was
  already met, confirmed by its existing test.
- Safe to Spend has zero coupling to budgets today (confirmed by grep, not
  assumed) - section 59's "do not redefine Safe to Spend" required no
  change at all, since there was nothing to preserve compatibility with.
- `overview.rs` already returned `budget_health: Vec<Budget>` reusing
  `budgets::list_for_month` directly - the Overview integration (section 60) only needed a new small card on the frontend, no new backend field.
- `ReportsPage.tsx`'s `CategoryDrilldown` (category+date-range transaction
  list) was the exact "see the real transactions behind a total" component
  Budgets also needed (section 81) - extracted to `CategoryDrilldown.tsx`
  and shared, rather than a second implementation.
- `lib/monthValue.ts` (month-value <-> ISO helpers, shared by Reports)
  already covered everything needed for month navigation.

## Data model

No new table. Migration 0014 rebuilds `budgets` (SQLite has no `ALTER
TABLE ... ALTER COLUMN` for CHECK constraints, so this uses the same
table-recreation technique as migrations 0004/0007/0013) to widen
`classification` to `'fixed' | 'flexible' | 'discretionary'` and add
`rollover_enabled INTEGER NOT NULL DEFAULT 0`. Every other field the
handoff asked for - pacing, rollover amount, typical spending, effective
capacity - is computed live in `hydrate()` from this same one-row-per-month
history and the real ledger, never stored, matching the correlated-
computation philosophy `accounts::balance_cents` and `recurring::
RecurringSeries`'s derived fields already established. This is the
"minimum additive schema" section 70 asked for, and it is genuinely
minimal: two columns, no new table.

## Historical stability

Non-negotiable per section 71, and proven directly: `editing_a_later_
months_budget_never_rewrites_an_earlier_months_stored_amount` creates a
January and an August row, updates August, and asserts January's own
`planned_amount_cents` is untouched. This isn't a defensive check against a
plausible bug - it's structurally impossible for it to fail, since the two
months are different database rows with no shared "base" between them. The
native-verification session confirmed the same thing live across a full
`Gifts` example spanning October/December/January (see "Native
verification").

## Actual spending

`accounting::spending_in_range` and `reports::spending_by_category` now
both build on one new shared helper, `accounting::split_expanded_rows`: for
a split transaction, one row per split (that split's own category and
amount, the parent's own `is_one_time`); for an unsplit transaction, the
transaction's own row. This is the fix described above - a split
transaction's spending is now correctly divided across its real categories
for budgets exactly as it already was for Reports, and the two can no
longer disagree, since they now read from the identical query.
`spending_in_range_is_split_aware_for_a_category_scoped_query` and
`historical_typical_and_suggestions_use_a_splits_own_allocation_not_the_
parents_full_amount` both prove this; a third test
(`spending_in_range_is_split_aware...`) also proves the unscoped total is
unchanged, since split amounts always sum to the parent's own ledger
amount.

Transfers/refunds are unchanged from Phase 4B: `spending_effect` already
excludes transfers and nets refunds against purchases, reused as-is.
Reconciliation status and pending status are never consulted - actual
spending includes every real, unarchived transaction regardless (section
65/66), matching Reports' own behavior exactly.

## Pace

`percent_elapsed = day_of_month / days_in_month` (section 47's own
"simplest acceptable approach"), `percent_used = actual / effective *
100`. Status is `OnTrack` unless the gap exceeds a documented ±10
percentage points (`PACE_TOLERANCE_POINTS`), in which case it's `Faster` or
`Slower` - chosen so the handoff's own worked example (52% used, 43%
elapsed) reads as on track with a little headroom to spare. The pure
threshold logic (`classify_pace`) is deliberately factored out of the
date-dependent wrapper (`pace_for`) specifically so it can be tested with
fully controlled inputs - `pace_for` itself depends on the real wall-clock
day of the month, which cannot be pinned down in a test without either
mocking `today()` (no override mechanism exists) or picking values that
happen to work on every possible day of the month, exactly the class of
fragility section 85 warns about and Batch 6 already learned the hard way.

Pace is only ever populated for the real, currently-in-progress calendar
month (`pace_is_populated_only_for_the_real_current_month`) - a future
month shows no pace or status language at all (section 48,
`statusLine`'s `"Not started yet"`), and a completed month shows a final
verdict (`Under budget` / `On budget` / `Over budget`) instead of a stale
pace reading (section 49). `BudgetStatus` (Under/Near/Over, unchanged
Phase-4B thresholds) is now judged against the _effective_ amount
(planned + rollover) rather than the raw planned figure - `status_is_
judged_against_the_effective_amount_including_rollover_not_the_raw_
planned_figure` proves a $310 actual against a $300 plan reads as `Under`
once a $200 rollover credit is in play. Once a budget is actually over,
that always wins over pace language (`Over by $X`, section 16's direct
wording) - pace is moot once exceeded.

## Historical baseline

`historical_typical(category, target_month, lookback_months)`: for each of
the last `lookback_months` months, sums that month's category spending
_excluding_ any transaction (or split) flagged `is_one_time`
(`accounting::spending_in_range_excluding_one_time`, a thin sibling of
`spending_in_range`), skips months with no qualifying spending (a sporadic
category's silent months are never fabricated as zero data points,
matching `suggest_budgets`'s pre-existing behavior), and takes the
**median** of the qualifying months (section 54) rather than a mean, so
one remaining outlier month can't dominate on its own even without a
one-time flag catching it
(`historical_typical_uses_the_median_so_one_outlier_month_cannot_
dominate`). Returns `None` (never a misleadingly-precise number) when
fewer than `MIN_MONTHS_FOR_TYPICAL` (2) qualifying months exist (section
53). Default lookback is 6 months (documented, section 8's second option -
chosen over 3 because the median benefits from a few more data points
without letting genuinely old behavior dominate).

Live-verified end to end: a Dining category with four normal ~$240 months
plus one flagged $600 anniversary dinner showed `Typical: $240.00` exactly

- the one-time exclusion working precisely through the real UI, not just
  in isolation.

## Suggestions

`suggest_budgets` is now built directly on `historical_typical` (median,
one-time-excluding) instead of its old separate mean-based calculation, so
"typical spending" and "suggested budget" can never quietly disagree -
`suggested_cents` is simply `typical_amount_cents` rounded to the nearest
$5 (`round_to_nearest_five_dollars`, unchanged). A category needs the same
2-qualifying-month minimum to appear at all - never fabricated from one
data point. **A second real bug was found live**: "Uncategorized" was
appearing as a suggestable category with its own "suggested amount," even
though there is no real category value to type into the editor for it
(section 40's whole point). Fixed by excluding it from the candidate set
before ranking, with a direct regression test
(`suggest_budgets_never_suggests_uncategorized_even_with_real_qualifying_
spending`) using a genuine uncategorized transaction, not just the
existing opening-balance/adjustment edge case. Suggestions remain entirely
optional - nothing is ever applied without the owner selecting categories
and confirming, and an existing planned amount is always shown for
comparison, never silently overwritten.

## Rollover

Opt-in per budget row (`rollover_enabled`), not silently on for anything.
`rollover_in_for(category, month)` walks backward one month at a time,
accumulating each qualifying prior month's own `planned_amount_cents -
actual_amount_cents`, stopping at the first month with no budget row for
that category (nothing to relay a chain through) or where rollover wasn't
enabled (a deliberate one-month opt-out breaks the chain from that point
forward). This plain backward sum is not an approximation of real
compounding - it _is_ compounding: if `carry(k) = planned(k) - actual(k) +
carry(k-1)`, the recurrence telescopes exactly into `sum(planned(k) -
actual(k))` over the qualifying chain, so a bounded iterative loop
(capped defensively at `ROLLOVER_LOOKBACK_LIMIT` = 36 months) gives the
identical answer a "prior month's carry feeds into this one" description
would, without needing actual recursion or a stored running total whose
provenance could drift (section 21). Never a mutable counter - always
re-derived from the immutable prior months' own figures, so it is exactly
as reproducible historically as any other query in the app.

Five tests cover surplus, deficit, multi-month compounding, a gap breaking
the chain, and a mid-chain opt-out breaking it from that point on. Native-
verified live: an August `Travel Fund` ($200 planned, $0 spent, rollover
on) fed a September row showing `Carried from last month: +$200.00` and
`$0.00 / $400.00` - section 77's own example, byte-for-byte, through the
real UI.

## Future overrides

No new mechanism - see "Data model." A `Gifts` budget was created live at
$100/month for October and January and $500 for December alone; navigating
month-by-month showed exactly $100 / (nothing in November) / $500 /
(nothing in November... January) $100, matching section 22's example
exactly, confirmed again after a full process restart.

## Splits

`accounting::split_expanded_rows` (see "Actual spending") is the single
place a split's own category/amount is expanded, shared by actual spending,
historical baseline, and suggestions alike - a split transaction contributes
its own children's allocations everywhere, never the parent's full amount,
and never once as parent-plus-children (double-counted).

## Refunds

Unchanged from Phase 4A/4B: `accounting::spending_effect` already nets a
refund against a purchase's magnitude for every budget calculation, exactly
as Reports has always shown it - no separate budget-specific refund policy
was invented, per section 32's explicit instruction.

## Credit cards / transfers

Unchanged, pre-existing coverage (`credit_card_purchases_count_against_
budget_once_and_payments_never_do`, `budget_calculations_exclude_transfers_
and_account_for_refunds`) confirmed still passing under the new split-aware
`spending_in_range`. A credit-card purchase's positive `amount_cents`
(ADR-0004) still counts once via `spending_effect`'s type-based
classification; the payment that later clears it is a `transfer` and
`spending_effect` returns 0 for transfers regardless of sign.

## Recurring integration

Deliberately deferred, using the handoff's own explicit escape hatch
(section 29: "If this crosses too far into forecasting, keep it
informational or defer"). Showing "known upcoming subscriptions: ~$35" per
category would require a new cross-feature query joining Batch 6's
recurring-pattern schema to Batch 7's free-text budget categories - two
systems whose category strings have no guaranteed relationship today (a
recurring series' own `category` field is rarely populated in practice) -
and section 105's own Ponytail checklist explicitly rejects "a generic
forecasting engine." The core, non-optional Budget Intelligence surface
(pace, rollover, historical baseline, suggestions, split-awareness, future
overrides, historical stability) is already substantial; layering a
best-effort recurring-obligations estimate on top without a reliable join
key would be exactly the kind of speculative, half-reliable feature this
project's own discipline warns against. Documented here as a prerequisite
for a future batch: a reliable category (or payee) link between
`recurring_series` and `budgets` would need to exist first.

## UI

`BudgetsPage.tsx` rebuilt from a dense table into a card list (matching
the Recurring page's own Batch 6 precedent, avoiding the "wide ERP table"
the handoff explicitly warns against): a small summary strip (Budgeted /
Spent / Remaining / Month elapsed, section 35), month navigation (reusing
`lib/monthValue.ts`), a "Copy last month's budgets" prompt whenever the
current month is missing a category the previous month had, the existing
suggestion-application flow adapted to the new median-based figures, one
card per category (progress bar with a month-elapsed marker, pace/status
text, typical spending, remaining or over-by amount, rollover line when
applicable, an inline transaction drilldown), and an "Unbudgeted spending"
section with a one-click "Set budget" per category (Uncategorized gets its
own explanatory note instead, per section 40). `BudgetProgressBar` carries
`role="img"` and a full descriptive `aria-label` stating both percentages
in words, so the visual marker+fill is never the only way to know where
things stand. `CategoryDrilldown` was extracted out of `ReportsPage.tsx`
into its own shared file rather than duplicated. Overview's dense "Budget
health" table was replaced with one small "Monthly budget" card (Spent
$X / $Y, N categories need attention, "View budget →") matching section
60's own example - Safe to Spend remains the unchallenged hero.

## Tests

384 Rust tests total (+21 net over Batch 6's 363, several existing tests
updated for the new split-aware/median-based behavior). New/updated
coverage spans every area in the handoff's own test matrix: core period
behavior (actual spend, remaining, over/under, month navigation, historical
effective amount, future override, historical stability), pace (the
documented tolerance band tested with fully controlled inputs, current-
month-only population, zero/negative-effective-amount edge cases), history/
suggestions (one-time exclusion, insufficient history, median-not-mean,
split allocation, refund netting, the Uncategorized exclusion), rollover
(surplus, deficit, compounding, a gap, a mid-chain opt-out, effective-amount-
based status), credit cards/transfers (pre-existing, reconfirmed), category
rename (pre-existing, reconfirmed), and the two real bugs found live
(split-aware `spending_in_range`, Uncategorized-suggestion exclusion) with
their own direct regression tests.

## Native verification

Verified live against the real compiled Tauri app via the established
WebView2 CDP driver, using two disposable accounts (`Batch7 Test
Checking`, `Batch7 Test Credit Card`) created through the app's own IPC,
archived and all test budgets deleted afterward. A pre-verification safety
copy of the live application-data database was taken first; the database
was inspected directly beforehand and held only already-archived test
fixtures from every prior batch, so no historical remediation question
arose.

Covered, matching the handoff's own scenario list: creating a Groceries
budget and watching spent/remaining/pace populate correctly against real
transactions (including a split Walmart purchase); a Dining budget whose
typical-spending figure correctly excluded a one-time $600 anniversary
dinner while its actual spending correctly still included it, pushing it
over budget with direct "Over by $16.25" language; the suggestion flow
(and the Uncategorized bug found and fixed here); a rollover round trip
(August surplus carrying $200 into September, shown exactly per section
77's own example); a three-month future-override sequence (Gifts at
$100/$500/$100 across October/December/January, including the gap months
in between correctly showing no budget at all); the "Copy last month's
budgets" prompt appearing and working; and a full process restart with
every figure - including the rollover carry and the override sequence -
confirmed byte-for-byte identical.

## Accessibility

Every button is a real `<button>`, every form control has a real
associated label, the progress bar carries `role="img"` with a complete
percent-used-and-elapsed `aria-label`, and status is always also plain
text (never color-only) - a red "Over by $16.25" is still legible as plain
text with `color: var(--midas-negative)`, and the "faster/slower/on track"
language never depends on the reader distinguishing gold from anything
else. Verified with the same automated DOM accessible-name/status-text
script established in Batch 6, run against the main page, the budget
editor dialog, and the expanded suggestion table - zero issues.

## Screen sizes

1280x800 and 1024x640 both checked live - the toolbar (month nav, Suggest,
Add budget) wraps via the existing flexbox toolbar pattern rather than
overflowing, and `document.documentElement.scrollWidth` never exceeded
`clientWidth` at either size on the main list or the suggestion table.

## Migration

Migration 0014, additive-only (two new columns, no new table). Propagated
mechanically to every test fixture that already included migration 0005
(`budgets`) - a genuine reconnaissance-learned distinction from Batch 6's
propagation, where migration 0013 could safely assume near-universal
inclusion of its own dependency; `budgets` is not nearly universal, so this
batch's propagation script was scoped file-by-file instead of blindly
appending everywhere. `migration_0014_preserves_existing_budgets_and_
defaults_rollover_off` proves an existing row survives with its
classification untouched and rollover defaulted off, and that the widened
CHECK constraint actually accepts `'discretionary'`.

## Security

No new Tauri capabilities - every new command (`copy_budgets_from_month`,
`get_unbudgeted_spending`) follows the existing `State<DbState>` +
`.map_err(|e| e.to_string())` pattern already used throughout. No network
access, no telemetry, no logging of transaction or budget content.

## Simplification (Ponytail)

Explicitly avoided every anti-pattern the handoff's own checklist named: no
envelope-ledger duplicate system (budgets read the real ledger directly, as
before), no AI, no generic forecasting engine (the one adjacent temptation -
recurring-obligations context - was deliberately deferred rather than
half-built), no daily budget records (monthly only), no unnecessary
snapshot duplication (rollover/pace/typical are all computed live, exactly
like `accounts::balance_cents`), no Category Groups redesign, no separate
budget transaction table, no cache invalidation machinery, no complicated
statistical model (median plus a documented percentage-point tolerance,
nothing more), no arbitrary health score (plain human status text
throughout), and no second Reports engine - if anything, this batch
_reduced_ duplication by unifying `spending_in_range` and
`spending_by_category` onto one shared split-expansion query where two
independent, silently-diverging implementations existed before. Two small
visibility cleanups applied during self-review: a `pub` constant and two
`pub fn`s introduced this batch but never called outside `budgets.rs` were
narrowed to their actual (private) usage - pre-existing `pub` functions
this batch didn't touch were left alone rather than churned for
consistency's sake.

## Git

Coherent commits by actual implementation boundary (schema/engine, UI,
docs) - see the completion report for exact hashes. Pushed to
`origin/main`; `v0.1.0` untouched; no new tag created.

## Explicit non-goals (unchanged from the handoff)

Full Insights redesign, full cash-flow forecasting, Financial Signals UI,
Weekly Snapshot, Monthly Review, a Category Groups migration (the
classification widening needed none), AI, SimpleFIN, mobile, CITADEL sync.
Recurring-obligations budget context (see "Recurring integration" above) -
explicitly deferred, not silently dropped, with its prerequisite named.

## Acceptance criteria

- [x] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` all
      pass (384 tests).
- [x] `npm run check`-equivalent (format, typecheck, tests, build) passes.
- [x] Migration 0014 applies cleanly to a fresh database; an existing
      budget row survives with its classification untouched and rollover
      defaulted off.
- [x] Historical stability proven directly: editing one month's budget
      never changes another month's stored figures, by construction and by
      test.
- [x] A split transaction's spending is correctly divided across its real
      categories for every budget calculation (actual, typical, suggested) - a real, pre-existing gap found and fixed at the root, not patched
      per call site.
- [x] Rollover compounds correctly across consecutive months, respects a
      gap or a mid-chain opt-out, and is always re-derived from history,
      never a stored mutable total.
- [x] Pace and status are computed from fully deterministic inputs
      wherever a test needs to assert on them - no test depends on which
      real calendar day it happens to run.
- [x] A second real bug (Uncategorized appearing as a suggestable budget
      category) was found live and fixed with a direct regression test,
      not deferred.
- [x] Accessibility and both required screen sizes verified live for every
      new UI surface.
- [x] All native-verification test data (two accounts, all test budgets)
      archived/deleted afterward; the live application-data database was
      inspected before any code was written and confirmed to hold no real
      financial data.

## Owner decisions

None required. The one genuinely open judgment call - how far to take
recurring-obligations budget context - was resolvable directly from the
handoff's own explicit escape hatch (section 29) and its Ponytail
checklist's explicit rejection of a forecasting engine (section 105).

## Hard-stop conditions

None encountered. No destructive or irreversible action was taken against
the live application-data database - a safety copy was taken before this
batch's verification pass, all verification used disposable, clearly-named
test accounts and budgets on the same real, continuing app-data database
prior batches established, and all of it was archived or deleted
afterward. Screen-control/computer-use access remains declined for this
project; verification continued via the same CDP-level approach
established in Batches 5/5.1/6.
