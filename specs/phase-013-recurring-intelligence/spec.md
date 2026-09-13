# Phase 013: Recurring Intelligence

- Status: COMPLETE
- Owner gate: APPROVED 2026-09-13 (owner handoff: "MIDAS Development Batch 6
  - Recurring Intelligence")

## Goal

> MIDAS should recognize repeating financial behavior, distinguish
> bills/subscriptions/recurring income, predict expected occurrences
> deterministically, match expected occurrences to real posted transactions,
> track price changes, and present upcoming recurring money in a useful
> calendar/list.

No AI, no bank sync, no automatic cancellation service. Everything
deterministic, explainable, and conservative - detection only ever proposes,
the owner always confirms/rejects/edits, and a projection never touches a
real balance before an actual transaction posts.

## Reconnaissance (before writing code)

- Phase 3 (`specs/phase-003-rules-recurring/spec.md`) already built a working
  foundation: `recurring_series` (merchant-based, >=3 consistent occurrences,
  `proposed`/`confirmed`/`rejected`), wired into a Bills page and
  `pay_cycle.rs`'s obligations/income-suggestion consumers. This shape
  already worked and was extended (table-rebuild migration, same technique
  as 0004/0007), not replaced - the handoff's own "do not rewrite correct
  working components without a reason" was taken literally.
- Batch 5.1 (`specs/phase-012.1-sign-convention-repair/spec.md`) had just
  fixed manual transaction entry's credit-card sign convention. Reconnaissance
  found the exact same class of bug already latent in `pay_cycle.rs`:
  `obligations_before`/`upcoming_bills`/`likely_income_series` all classified
  bills/income by raw sign (`expected_amount_cents < 0`/`> 0`) rather than
  account-type-aware direction. Fixed alongside the new `Direction` enum
  (`Expense`/`Income`/`Transfer`) so `recurring_series` now stores its
  classification as data instead of re-deriving it from sign at every read
  site - the same "decide once, upstream" discipline Batch 5.1 established.
- `transactions.recurring_series_id` already existed as a column (unused)
  from an earlier phase - reused as the link from a real transaction back to
  the pattern that produced it, rather than adding a second column.
- `needs_review_items` (Phase 2) already supports an `issue_type='other'` +
  freeform `proposed_resolution_json` shape - reused directly for ambiguous-
  match routing and payee-merge conflicts. No second Review inbox.
- `pay_cycle.rs`'s `PaySchedule::Semimonthly` already modeled two fixed
  paydays per month with the same two-field shape `recurring_series` now
  uses for semimonthly cadence - the frontend's schedule-suggestion logic
  was extended to also recognize a detected semimonthly income series
  instead of only weekly/biweekly/monthly.
- `dates::add_months`/`day_in_month` (Phase 4A/ADR-0008, `chrono`-backed)
  already correctly clamp month-end overflow (Jan 31 + 1 month -> Feb 28,
  or Feb 29 in a leap year) - reused directly for month-anchored cadence
  advancement instead of the old Phase 3 detector's `add_days(date, 30)`,
  which is a genuine behavior improvement this batch made deliberately
  (see "Cadence rules" below).
- No background job/scheduler infrastructure exists anywhere in MIDAS, and
  none was added - occurrence generation and matching both run lazily, from
  whatever the frontend or a transaction write already triggers.

## Data model

Three concepts, never the same database row (this was the single most
important architectural decision in the whole batch):

- **`RecurringSeries`** (`recurring.rs`) - MIDAS's belief about a repeating
  pattern: merchant/payee, `Direction` (expense/income/transfer), `Kind`
  (subscription/bill/membership/recurring_income/other), `Cadence`, amount,
  next expected date, status, lifecycle flags. Detected or manually created,
  identical model either way (`create_manual` and `detect_candidates` write
  the same table through the same columns).
- **`RecurringOccurrence`** (`recurring_occurrences.rs`, new table, migration 0013) - a projection of one future or past expected instance of a series.
  Has no effect on any balance, budget, or reconciliation. Generated lazily
  to a rolling 90-day horizon (`HORIZON_DAYS`), never a background job, and
  idempotently (`INSERT OR IGNORE` + `UNIQUE(series_id, expected_date)`) so
  re-running generation, restarting MIDAS, or importing the same file twice
  never duplicates a row.
- **`Transaction`** - ledger truth, unchanged. Linking an occurrence to a
  transaction (`matched_transaction_id` on the occurrence,
  `recurring_series_id` on the transaction) never edits, creates, or
  duplicates that transaction.

`RecurringSeries`'s derived fields (`last_occurrence_date`,
`last_amount_cents`, `min`/`max`/`median_amount_cents`, `occurrence_count`,
`is_price_changed`, `previous_typical_amount_cents`, `is_possibly_missed`,
`monthly_equivalent_cents`, `annual_equivalent_cents`) are all computed live
in `hydrate()` from linked transactions, never separately stored - the same
correlated-computation philosophy `accounts::balance_cents` already
established, so nothing can drift out of sync with the ledger it's derived
from.

## Cadence rules

`Cadence`: `Weekly`, `Biweekly`, `Monthly`, `Quarterly`, `Annual`,
`Semimonthly`.

Month-anchored cadences (`Monthly`/`Quarterly`/`Annual`) advance via
`dates::add_months`, never a fixed day-count - a bill anchored to the 31st
lands on Feb 28 (or Feb 29 in a leap year), never drifts forward into March.
This directly replaces Phase 3's old `add_days(date, 30)` detector logic,
which is a genuine, deliberate behavior improvement this batch made (see
`month_end_anchored_pattern_advances_correctly_through_february` and
`...reaches_february_29_in_a_leap_year` in `recurring_occurrences.rs`).
Week-anchored cadences (`Weekly`/`Biweekly`) advance by a fixed day count,
which is correct for them since a week never varies in length.

`Semimonthly` (two fixed days-of-month, e.g. the 1st and 15th) is distinct
from `Biweekly` (a constant 14-day gap that drifts across months) and is
classified separately (`classify_semimonthly`): it checks whether
consecutive gaps in a sorted date list alternate between two "half-month"
lengths (10-21 days each) summing to a full month (26-34 days), with a
minimum gap-variance requirement so a genuine constant-14-day Biweekly
series is never misclassified as semimonthly (and vice versa - both
directions are covered by tests). Paychecks commonly use this shape, so
`PayCycleSettings.tsx`'s schedule-suggestion logic was extended to offer a
detected semimonthly income series as a one-click pay-schedule option,
alongside the pre-existing weekly/biweekly/monthly cases.

Each occurrence's matching window (`Cadence::matching_window_days`) is 2
days for `Weekly`, 4 days for everything else - used both for expected-to-
actual date tolerance and for the missed/due status cutoff.

## Detection

`detect_candidates` groups a confirmed-not-yet-tracked account's evidence by
`payee_id` first (falling back to raw merchant text for transactions with no
payee), requires >=3 occurrences, excludes
`opening_balance`/`balance_adjustment`/`transfer` transaction types and any
transaction flagged `is_one_time` (none of these are legitimate recurring
evidence), and skips any merchant/payee that already has an existing series
of any status (so a rejected pattern never re-proposes itself, satisfying
"respect rejection, no nagging," and confirming an already-detected pattern
is idempotent across reruns).

Two deliberately different amount tolerances:

- **`detection_tolerance`** (40% of the reference amount, or $5, whichever
  is larger) - used only to decide whether a group of historical occurrences
  is consistent enough to propose as one pattern at all. Widened from an
  initial 20%-or-$2 specifically so a genuinely variable utility bill (the
  handoff's own example: $124/$151/$168/$143) is recognized as **one**
  recurring bill, not rejected or fragmented into several unrelated ones.
- **`amount_tolerance`** (20% of the reference amount, or $2) - used for
  matching a real transaction to an expected occurrence, and for deciding
  whether the *latest* occurrence of an already-established pattern
  represents a meaningful price change. Kept tighter on purpose: once a
  pattern is established, a $5 jump on a $10 subscription is real news; the
  same $5 swing on a $150 utility bill is not.

**Kind heuristic** (added after native verification surfaced a real gap -
see "Native verification" below): a freshly-detected expense pattern
defaults to `Kind::Bill` if its evidence only passed the _wider_
`detection_tolerance` and would not also have passed the narrower
`amount_tolerance` - i.e. its amounts genuinely move around from cycle to
cycle - and to `Kind::Other` otherwise. This is deterministic and tied to
tolerance math already computed during detection, never a merchant-name
guess. `Kind::Bill` is treated tolerantly by price-change detection (below);
`Kind::Other`/`Kind::Subscription`/`Kind::Membership` are not, until the
owner reclassifies a series via the pattern editor (which immediately takes
effect, since price-change is computed live, never cached).

Detection never fabricates a specific service name - the merchant text is
whatever the owner's own transactions/payee already say. If a raw
description is genuinely ambiguous (e.g. "APPLE.COM/BILL"), it is shown
as-is; renaming to something recognizable is a normal pattern-editor edit,
the same as renaming a payee.

## Amount model

`RecurringSeries` never predicts an amount with a model beyond
"deterministic and explainable": `last_amount_cents` (most recent linked
transaction), `min`/`max_amount_cents` (full range across linked history),
`median_amount_cents` (used for both the "typical" figure shown for a
variable pattern and as the price-change comparison baseline). No mean, no
weighted average, no forecasting - median is robust to one outlier month
without needing a separate outlier-detection step.

Price-change detection (`hydrate`) compares the _latest_ linked amount
against the median of every _prior_ linked amount - never a value frozen at
first detection, which would drift stale as a pattern ages - and is
suppressed entirely for `Kind::Bill` (normal utility variation is not a
"price change") and for fewer than 2 prior occurrences (nothing to compare
against yet).

Monthly/annual equivalents (`monthly_equivalent_cents`/
`annual_equivalent_cents`) use defined, tested integer rounding
(`(amount_cents * factor + half_denominator) / denominator`, e.g. an annual
subscription's monthly figure rounds to the nearest cent) - presentation
only, never written back into any ledger calculation.

## Subscriptions, Bills, and Recurring Income

One data model, one lifecycle, no separate ledger system per `Kind` - the
frontend's Subscriptions/Bills/Income tabs are pure client-side filters over
the same `RecurringSeries` list (`direction === "expense" && kind in
{subscription, membership}"` for Subscriptions, `{bill, other}` for Bills,
`direction === "income"` for Income). A subscription card shows merchant,
amount, monthly/annual equivalent, next expected date, and a price-change
note when applicable. A bill card shows "Typically $X (range $Y-$Z)" instead
of a single fixed amount whenever its linked history is actually variable,
so a utility bill is never misrepresented as guaranteed-fixed. Recurring
income is first-class (its own tab, its own `Direction::Income` value,
already consumed by `pay_cycle.rs`'s existing income-suggestion feature) -
not a special case bolted onto the expense model.

## Transfers and credit-card payments

`Direction::Transfer` exists on the backend (excluded from detection
evidence, excluded from matching, excluded from both expense and income
totals) so the data model can represent a recurring transfer between the
owner's own accounts correctly if one is ever created - but the frontend's
manual-creation form deliberately only offers Expense/Income as pickable
directions, since building a full recurring-transfer management UI wasn't a
named deliverable. A credit-card payment is already modeled as a `transfer`
transaction type (pre-existing convention, unrelated to this batch) and is
therefore automatically excluded from ever appearing as a recurring bill,
with a live-verified regression test proving it (`transfer_transactions_
never_auto_match`, `transfers_are_never_proposed_as_a_recurring_pattern`).

A recurring _charge_ on a credit card is still correctly classified as an
expense despite `amount_cents` being positive under ADR-0004 (a charge
increases what's owed) - `Direction::from_amount(amount_cents, is_liability)`
mirrors the exact account-type-aware logic Batch 5.1 established for manual
transaction entry, now expressed once and stored as data instead of
re-derived from sign at every consumption site. Native-verified live (see
below) with a real credit-card "Gym Membership" charge.

## Expected occurrences

State model: `Upcoming` -> `Due` (expected_date has arrived) -> `Matched`
(linked to a real transaction) or `Missed` (expected_date passed the
cadence's matching window with nothing linked) or `Skipped` (owner action,
never fabricates a $0 transaction, never disables the whole pattern) or
`NeedsReview` (ambiguous match candidate, below).

Generated lazily by `ensure_occurrences`, called whenever something asks for
a series' occurrences (listing, confirming, or a transaction arriving that
might match one) - out to `today + 90 days`, capped defensively at 200
inserts per call (a validated cadence can never actually approach this
within any reasonable horizon; the cap exists purely so a future bug in
cadence advancement fails loudly/slowly rather than filling the database).
A series that isn't `Confirmed` generates nothing.

## Matching

`evaluate_transaction_for_matching`, called from the single shared
`transactions::create()` (covers manual entry and every importer through
one code path - no separate recurring-import pipeline) and, when a
financially-relevant field changes, from `transactions::update()`
(`reevaluate_after_edit` unlinks the old match and re-evaluates fresh; a
harmless metadata-only edit like notes/category/tags never disturbs an
existing match).

Candidate series are scoped by **account** first (`account_id = ?`), then by
payee or exact merchant text - a transaction on one account can never match
another account's series even if every other signal lines up (live- and
unit-verified: `a_transaction_on_a_different_account_never_matches_
another_accounts_series`). Within a candidate series, an unmatched
occurrence must fall within the cadence's date window (documented above)
and `amount_tolerance` of the transaction - never amount alone.

Ambiguity (more than one plausible occurrence, whether from one series with
occurrences unusually close together or from two independently-detected
series that happen to share a merchant) is never silently resolved: every
candidate occurrence flips to `NeedsReview` and one `needs_review_items` row
(`issue_type='other'`, `proposed_resolution_json`:
`{"kind":"ambiguous_recurring_match","occurrence_ids":[...]}`) is created,
surfaced on the existing Needs Review page with a per-candidate "This is
[date] - [amount]" choice. One transaction can never satisfy two
occurrences (`link_occurrence_to_transaction` refuses a second link).

**A real bug was found and fixed during native verification here**: when
the owner picked one candidate, the _other_ flagged occurrence(s) had no
path back out of `NeedsReview` - they stayed stranded forever, invisible to
both future matching and the Upcoming/Calendar views. Fixed with
`revert_other_ambiguous_candidates`, called after a successful link: it
re-derives "was this occurrence also a candidate for this transaction" the
same way detection did (same account, same payee-or-merchant) and reverts
every other `NeedsReview` occurrence back to a normal open status via the
same `refresh_statuses` logic `unlink_occurrence` already uses. Covered by
`resolving_an_ambiguous_match_reverts_the_other_candidate_instead_of_
stranding_it` and re-verified live end to end (see below).

## Price history

`recurring::price_history` is a pure query over linked transactions
(`WHERE recurring_series_id = ?`), never a separately stored or duplicated
history - satisfying the handoff's explicit "do not duplicate transaction
history unnecessarily." The frontend renders it as a small hand-rolled SVG
polyline sparkline (`PriceHistorySparkline` in `RecurringPage.tsx`, ~20
lines, no charting dependency) plus a plain date/amount list underneath for
explainability.

## User overrides and lifecycle

`SeriesStatus`: `Proposed` -> `Confirmed` (owner action; also retroactively
links matching existing history via `link_matching_history`) or `Rejected`
(permanent - dedup at detection time means the same merchant/payee never
re-proposes) -> `Inactive` (owner-only; MIDAS never auto-deactivates after a
single miss, satisfying the handoff's explicit conservatism requirement) ->
`Reactivate` (validated to only apply to an `Inactive` series). A separate
`planning_to_cancel` boolean (purely organizational - MIDAS never actually
cancels anything) is settable only on a `Confirmed` series.

Manual pattern creation (`create_manual`) uses the exact same model and
table as detection - no separate "Bills engine." A pattern editor
(`RecurringPatternEditor.tsx`) edits name/payee/kind/category/cadence/
amount/next-date/notes (`update_pattern`/`PatternEditInput`) without
exposing any internal threshold (`detection_tolerance`, `amount_tolerance`,
etc. are never surfaced). Once edited, the owner's values are authoritative

- `detect_candidates` already skips any merchant/payee with an existing row
  of any status, so detection can never silently overwrite an edit.

Direction and account are intentionally **not** part of the general edit
path: changing either mid-life would silently rewrite what a pattern's
amount has meant historically, or which ledger a future transaction must
belong to in order to match. Direction is fixed at creation. Account can be
changed via a small, separate, explicitly-confirmed `move_account` action
(its own dialog, its own function) - satisfying the handoff's "allow
legitimate account/card changes with confirmation" without folding it into
the low-friction general save.

## Calendar and Upcoming

The Recurring page (renamed from "Bills" in the sidebar; same nav slot, no
new sidebar destination) has five subviews: **Upcoming** (a flat, near-term
list, next 30 days from today), **Subscriptions**, **Bills**, **Income**
(client-side filters, described above, each with search/show-inactive/sort-
by-next-date-or-name-or-monthly-or-annual-cost), and **Calendar** (a
date-grouped agenda for a navigable month, not a grid). The handoff
explicitly permits an agenda/list fallback over a full desktop grid; a grid
was judged unnecessary complexity (leading/trailing blank-cell math, per-day
overflow handling) for a personal-finance app's calendar of a few dozen
occurrences a month, and the agenda cleanly reuses the same occurrence-row
component as Upcoming. Each occurrence row shows date, merchant, amount, and
a **text** status label - never color alone - only when the status is
noteworthy (`Due`, `Posted`, `Missed`, `Skipped`, `Needs review`; the
default `Upcoming` case shows no badge, avoiding a "rainbow of pills" for
the common case). `Skip`/`Unlink` actions appear only where valid
(`Skip` for `Upcoming`/`Due`, `Unlink` for `Matched`).

Overview's pre-existing "Upcoming bills" card (`pay_cycle::upcoming_bills`,
already correctly reading the new `Direction::Expense` field after this
batch's own reconnaissance-stage fix) already satisfied the handoff's
"restrained supporting card, Safe to Spend remains the hero" requirement
without any frontend change - left untouched rather than rebuilt without a
reason.

## Review integration

No second inbox. The two new proposal shapes
(`ambiguous_recurring_match` on `recurring_occurrences.rs`,
`payee_merge_recurring_conflict` on `payees.rs`, both `issue_type='other'`)
render inside the existing `NeedsReviewPage.tsx` list. The ambiguous-match
shape gets a small dedicated resolver component (fetches each candidate
occurrence's date/amount via a new thin `get_recurring_occurrence` command,
renders one button per candidate); the payee-merge-conflict shape needs no
special UI - its `detected_reason` is already a complete, actionable
sentence, and the generic resolve/dismiss actions are sufficient once the
owner has looked at both patterns.

## Payee-merge integration

`payees::merge` now also reconciles recurring evidence: if the source payee
has recurring series and the target has none, they're repointed to the
target payee (otherwise a series stays keyed to the now-archived source and
silently stops matching once a future transaction's raw merchant text no
longer matches what detection captured - the whole reason `payee_id` exists
instead of relying on raw text forever). If **both** payees already have
their own recurring series, they are never silently combined - a
`payee_merge_recurring_conflict` review item is created instead, satisfying
"surface duplicates for review rather than silently combining."

## Migration

Migration 0013 (additive only): `recurring_series` rebuilt (SQLite has no
`ALTER TABLE ... ALTER COLUMN` for CHECK constraints, so this uses the same
table-recreation technique as migrations 0004/0007) to add `payee_id`,
`direction` (backfilled from existing rows' amount sign - the last
sign-based classification anywhere in this feature, done once, at rest, on
data that already existed only as disposable test fixtures per the
existing-data-safety check below), `kind`, `semimonthly_day_1/2`,
`planning_to_cancel`, `notes`, `created_source`; a new `recurring_occurrences`
table. `migration_0013_preserves_existing_recurring_series_and_backfills_
direction` proves an existing income/expense series survives with correct
values. No historical remediation needed - the live application-data
database was checked directly before writing any code and holds no real
financial data, only already-archived test fixtures from prior batches (see
"Native verification" below).

## Tests

362 Rust tests total. New/changed coverage across every area named in the
handoff's test matrix: detection (weekly/biweekly/monthly/quarterly/annual/
semimonthly classification, semimonthly-vs-biweekly in both directions,
insufficient-history, one-time-exclusion, transfer-exclusion, variable-
utility-bill grouping, credit-card expense-not-income classification,
income-on-an-asset-account, idempotence across reruns), expected occurrences
(horizon generation and idempotence, month-end February including a real
leap year, skip, restart persistence via the same disk-backed database every
other batch's persistence relies on), matching (exact, date-shifted, weeks-
away rejection, amount-tolerance rejection, wrong-account rejection, one-
transaction-cannot-satisfy-two, ambiguous-routes-to-review, ambiguous-
resolution-reverts-the-sibling), price history (derived-from-transactions
ordering, price-change-compares-against-prior-typical-not-a-frozen-snapshot,
Bill-kind never flags normal variation), transfers (never proposed, never
auto-matched), lifecycle (inactive/reactivate round trip, planning-to-cancel
requires confirmed, manual creation and its semimonthly-completeness
validation, move-account), and payee-merge coherence (repoints when the
target has no series of its own, routes to review instead of combining when
both do).

Frontend: 6 existing tests unchanged (no new pure-function frontend logic
was added this batch beyond what Batch 5.1 already covers - the sign-
convention helpers it added are reused as-is for the pattern editor's
amount field).

## Native verification

Verified live against the real compiled Tauri app via the established
WebView2 CDP driver (screen-control/computer-use access remains declined
for this project), using two disposable accounts (`Batch6 Test Checking`,
`Batch6 Test Credit Card`) created through the app's own IPC, archived
afterward. A pre-verification safety copy of the live application-data
database was taken before touching it, and the database was inspected
directly first (`node:sqlite`, read-only) - it held zero real financial
data, only already-archived test fixtures from every prior batch, so no
historical migration/remediation question ever arose.

Covered: detection across four realistic patterns in one scan (a fixed
subscription, a variable utility bill, a biweekly income, a credit-card
charge) with correct direction/next-date shown before confirming; confirm
moving all four into their correct Subscriptions/Bills/Income tabs;
occurrence matching (an exact real transaction flipping an occurrence to
"Posted"); skip; Calendar month navigation forward and back with correct
occurrence continuity across a skip; manual annual-subscription creation
with correct monthly/annual equivalents; the full lifecycle round trip (set
inactive, show-inactive filter, reactivate, planning-to-cancel toggle, both
shown together as separate text badges); move-to-a-different-account with
live confirmation; a deliberately-constructed ambiguous match, confirmed to
route to Needs Review with a working per-candidate resolver - **which is
where the sibling-stranding bug above was found and fixed, then
re-verified live with a second, independent ambiguous scenario**; a full
app restart (killing and relaunching the real `cargo tauri dev` process,
not just the page) with every pattern, occurrence status, and the bug fix's
effect confirmed byte-for-byte persistent; a recurring-looking transfer
transaction confirmed to never be proposed; and light/dark theme rendering.

The Kind-heuristic gap above was also found here, not by code review: a
variable utility bill correctly grouped into one pattern still showed a
"Price changed" badge every time, because detection always defaulted `kind`
to `Other` and only `Kind::Bill` suppresses that flag - directly
contradicting the handoff's own "one recurring bill, not four price
changes" framing in spirit even though the record count was already
correct. Fixed with the deterministic tolerance-based heuristic described
above, verified live with a second variable-bill merchant (auto-classified
correctly on first detection) and by manually reclassifying the
already-existing one (confirming the pattern editor's Kind field
immediately suppresses the flag for existing series too).

## Accessibility

Every button is a real `<button>`, every form control has a real
associated label (explicit `htmlFor`/`id` or wrapping `<label>`, matching
`TransactionForm.tsx`'s established pattern), the direction radio group in
the pattern editor uses a real `<fieldset>`/`<legend>` (a small, deliberate
improvement over the app's pre-existing bare-span radio-group convention,
since this is new code and the accessibility bar for new surfaces is
explicit), the price-history SVG carries `role="img"` and a descriptive
`aria-label`, and every status indicator is a text badge, never color
alone. Verified with a small automated DOM script (every button/input/
select's accessible name computed and asserted non-empty; every status
badge asserted to carry real text) run against every subview, the pattern
editor, the move-account dialog, and the Needs Review page's ambiguous-match
resolver - zero issues after correcting one false positive in the checker
itself (it initially missed implicit label-wrapping).

## Screen sizes

1280x800 and 1024x640 both checked live (Calendar, Bills-tab cards, and the
pattern editor dialog, which scrolls internally rather than overflowing) -
`document.documentElement.scrollWidth` never exceeded `clientWidth` at
either size. The filters row (search/show-inactive/sort) and the tab
switcher both wrap via flexbox at the narrower size rather than clipping.

## Security

No new Tauri capabilities - every new command follows the existing
`State<DbState>` + `.map_err(|e| e.to_string())` pattern already used
throughout. No network access, no telemetry, no logging of transaction
content. No subscription-cancellation integration, no banking credentials -
MIDAS only ever reads/writes its own local SQLite database.

## Simplification (Ponytail)

Explicitly avoided everything the handoff's own anti-pattern list named: no
cron engine or generic scheduling framework (occurrence generation is a
plain function call from whatever already needed it), no AI/ML
classification (every decision is a documented threshold or account-type
rule), no second Rules engine (reused `needs_review_items` as-is), no
duplicate ledger rows or recurring-specific transaction-duplicating table
(occurrences are a separate, effect-free table; price history is a query,
not stored data), no confidence scores (booleans and thresholds only), no
per-cadence detector class (one `propose_from_occurrences` function
classifies cadence as one step), no calendar library (a hand-rolled ~10-line
month-bounds helper plus a date-grouped agenda), no notification
infrastructure, no cancellation integration, no second Review inbox. One
small cleanup applied during self-review: an accidentally-`export`ed
`KIND_LABELS` constant (only ever used inside its own file) had its export
removed.

## Explicit non-goals (unchanged from the handoff)

Budget Intelligence redesign, Insights redesign, a forecasting engine,
Financial Signals UI, Weekly Snapshot, Monthly Review, Category Groups
migration, Fixed/Flexible/Discretionary classification, AI, SimpleFIN,
mobile, CITADEL sync, notification infrastructure, subscription-cancellation
services. Recurring-transfer management UI (the data model supports
`Direction::Transfer` defensively; no creation UI was built for it, since it
wasn't a named deliverable and the handoff's own transfer requirements are
about _exclusion_, not a dedicated management surface).

## Acceptance criteria

- [x] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` all
      pass (362 tests: 355 existing + 7 net new/changed test functions,
      several existing tests updated for the new `Direction`/`Kind` model).
- [x] `npm run check`-equivalent (format, typecheck, tests, build) passes.
- [x] Migration 0013 applies cleanly to a fresh database; an existing
      recurring series survives with a correctly backfilled `direction`.
- [x] Three distinct concepts (series/occurrence/transaction) never share a
      row; verified structurally (separate tables) and by test (linking
      never edits/creates/duplicates a transaction).
- [x] Transfers and credit-card payments never appear as recurring expense/
      income, live- and unit-verified.
- [x] A variable utility bill is recognized as one pattern, not fragmented
      or repeatedly flagged as a price change - live-verified with two
      independent merchants.
- [x] Ambiguous matches never silently guess, are routed to Needs Review,
      and resolving one correctly frees the other candidate(s) rather than
      stranding them - a real bug here was found and fixed before sign-off,
      not deferred.
- [x] Manual pattern creation uses the identical model/table as detection.
- [x] Account and direction changes on an existing pattern are deliberately
      excluded from the low-friction edit path; account changes go through
      a separate, explicitly-confirmed action.
- [x] Full restart-persistence verified for patterns, occurrences, and the
      ambiguity-fix's effect, not just for the ledger.
- [x] Accessibility and both required screen sizes verified live for every
      new UI surface.
- [x] All native-verification test data archived, matching prior batches'
      convention; the live application-data database was inspected before
      any code was written and confirmed to hold no real financial data.

## Owner decisions

None required. Every judgment call (extend vs. replace Phase 3's table,
agenda vs. grid calendar, where the Kind heuristic's boundary sits, how to
resolve the ambiguous-match sibling bug) was resolvable directly from the
handoff's own explicit guidance, its named anti-pattern list, or direct
native-verification evidence.

## Hard-stop conditions

None encountered. No destructive or irreversible action was taken against
the live application-data database - a safety copy was taken before this
batch's verification pass, all verification used disposable, clearly-named
test accounts on the same real, continuing app-data database prior batches
established, and all of it was archived afterward. No historical sign/data
remediation question arose, since the live database held no real financial
data. Screen-control/computer-use access remains declined for this project;
verification continued via the same CDP-level approach established in
Batch 5/5.1.
