# Phase 016: Review Center + Data Health + Financial Signals

- Status: COMPLETE
- Owner gate: APPROVED 2026-09-13 (owner handoff: "MIDAS Development Batch 9
  - Review Center + Data Health + Financial Signals")

## Goal

> MIDAS should clearly identify what needs the user's attention, explain why
> it matters, provide the correct next action, and show whether the
> underlying financial data is trustworthy.

Three deliberately separate concepts, never conflated: a **Review Item**
(something requiring a user decision - possible duplicate, ambiguous
recurring match, uncategorized transaction, broken reconciliation); **Data
Health** (a factual summary of whether MIDAS's own records are clean -
counts, never a score); and a **Financial Signal** (a deterministic
observation worth attention but not necessarily an error - high category
spend, a price increase, a projected low balance). No AI, no arbitrary
financial-health score, no second inbox, no notification infrastructure.

## Reconnaissance (before writing code)

- `needs_review_items` (Phase 2, migration 0002) already exists with an
  `issue_type` CHECK constraint limited to 8 literal values, but only
  `malformed_row`/`suspected_duplicate` were ever actually inserted from
  that list - Batch 6 had already established the real pattern for a new
  kind: `issue_type = 'other'` plus a JSON `"kind"` discriminator inside
  `proposed_resolution_json` (`ambiguous_recurring_match`,
  `payee_merge_recurring_conflict`). This batch needed zero genuinely new
  persisted kinds, so it continues that exact precedent rather than
  widening the CHECK constraint via a migration.
- Most of what the handoff calls "Review Items" already has a durable home
  elsewhere and never needed a second copy of the truth: a proposed
  recurring pattern is `recurring_series.status = 'proposed'` (Batch 6); a
  missed occurrence is `recurring_occurrences.status = 'missed'` (Batch 6);
  an invalidated reconciliation is `reconciliations.status = 'needs_review'`
  (Batch 4); an uncategorized purchase is just `transactions.category IS
NULL` on a `Purchase`-type row. Only facts with no other durable home
  (a possible duplicate, a malformed import row, an ambiguous recurring
  match, a payee-merge conflict) go through `needs_review_items` at all.
  This directly enables the handoff's own "likely cleanest architecture"
  (a hybrid: persist only what has no other home, derive everything else
  live) without inventing anything new.
- A real, pre-existing bug: an `ambiguous_recurring_match` row only ever
  closed because `NeedsReviewPage.tsx`'s one dedicated resolver happened to
  call `resolve()` as a side effect of its own "pick a candidate" button -
  resolving the ambiguity through any other path (or the underlying
  occurrence changing state for an unrelated reason) left the row
  permanently stranded "open." Fixed generally, not just for the one UI
  path that used to trigger it (see Review Lifecycle below).
- `pay_cycle::active_account_series` (Batch 6, private) already encodes the
  correct non-archived-account filter Safe to Spend/Forecast/obligations
  all use - bumped to `pub(crate)` so Review's own recurring queries reuse
  the identical filter rather than a fourth reimplementation.
- Current migration count: 14 (`0014_budget_intelligence.sql`). This batch
  needed **zero** new migrations - every derived item recomputes from
  already-durable state, and the only genuinely new persisted fact
  (`StagedRowProposal`, see Review Items below) fits inside the existing
  `proposed_resolution_json` column.

## Review Center (`review.rs`, sections 1-22)

One unified destination (`ReviewPage.tsx`, "Review" in the sidebar,
replacing the old flat `NeedsReviewPage.tsx`) with domain filters
(All/Transactions/Imports/Recurring/Reconciliation) inside a single "Needs
Attention" tab, rather than a separate inbox per domain - net zero sidebar
growth versus the prior single "Needs Review" entry.

- `ReviewItem { key, domain, priority, kind, title, evidence, sort_date,
... }` is the one shape both persisted and derived sources map into.
  `ReviewPriority` (Low/Normal/High) and `ReviewDomain`
  (Transactions/Imports/Recurring/Reconciliation) are small closed enums,
  never a 0-100 score.
- **Persisted sources** (`needs_review_items`, unchanged table): malformed
  import row and suspected duplicate (both High/Imports, existing Phase 2
  data), ambiguous recurring match and payee-merge conflict (both
  Normal/Recurring, existing Batch 6 data, unpacked from their `'other'` +
  JSON-`kind` encoding).
- **Derived sources**, recomputed on every read, never a second copy:
  uncategorized purchases (Purchase-type, `category IS NULL`, no split
  rows, non-archived account - Normal), proposed recurring patterns
  (`status = 'proposed'`, non-archived account - **Low**, since nothing is
  actually wrong, MIDAS is only offering an easy one-click confirm),
  missed recurring occurrences (see below - Normal), and reconciliations
  with `status = 'needs_review'` (non-archived account - **High**, the
  handoff's own explicit priority for a broken reconciliation).
- `list_review_items` calls a stale-item reconciliation pass first, then
  aggregates all five sources and sorts deterministically: priority
  descending, then `sort_date` ascending (oldest/most-relevant first), then
  `key` ascending as a stable tie-break - never random, never flickering
  between reads of identical data.
- **"Keep Both" real gap fixed** (section 14): a CSV/OFX row that isn't
  force-imported and isn't an exact duplicate is never committed as a
  transaction - Import Engine v2 holds it back, creating only a
  `needs_review_items` row with no `transaction_id`. Its candidate data was
  already serialized into `proposed_resolution_json`, but the id of the
  transaction it was flagged as a possible duplicate of was not. Fixed by
  introducing `import::StagedRowProposal { candidate, duplicate_of_transaction_id
}`, serialized in place of the bare candidate; `review::keep_duplicate_transaction`
  deserializes it and calls `transactions::create` directly - the exact
  same insert path every other transaction creation already uses, never a
  second commit mechanism.

## Review lifecycle, deduplication, and staleness (sections 10-13)

Persisted items keep `needs_review_items`' existing
Open/Resolved/Dismissed lifecycle unchanged (no deleting historical
evidence). Derived items have no lifecycle of their own to manage at all -
they stop appearing the instant their own domain's durable state changes
(categorize the transaction, confirm the pattern, repair the
reconciliation), which is what "automatic stale-item cleanup" reduces to
when there's no second copy of the truth to go stale in the first place.

**Bug found in reconnaissance, fixed generally**: `ambiguous_recurring_match`
is the one persisted kind whose truth is _also_ mirrored in derived state
(the occurrence's own `status`). `reconcile_stale_persisted_items` checks,
before trusting a persisted "open" row, whether any of its own candidate
`occurrence_ids` are still actually `status = 'needs_review'` in
`recurring_occurrences` - the real source of truth - and auto-resolves the
row if not. This closes the item no matter which path resolved the
underlying ambiguity, not just the one dedicated UI button that used to be
the only thing calling `resolve()`.

**Bug found in native verification, fixed at the root**: `recurring_missed`
originally reused `RecurringSeries::is_possibly_missed`, a _computed_
field off `next_expected_date` - a live cursor that
`recurring_occurrences::ensure_occurrences` eagerly advances the instant
_anything_ reads occurrences for that account, including totally unrelated
reads like opening Forecast or Signals. A genuinely missed item could
silently vanish from Review merely because the user glanced at Forecast.
Fixed by querying `recurring_occurrences.status = 'missed'` directly - a
truly durable fact nothing reverts except the user's own explicit
skip/link/inactive action - with a regression test
(`a_missed_item_survives_an_unrelated_occurrence_refresh`) that calls the
Forecast-equivalent occurrence refresh mid-test and asserts the item is
still there afterward.

Idempotence/restart-safety is structural for the same reason Batch 8's
Forecast was: nothing here writes a derived row, so recomputing from the
same underlying state always produces the same items (proven directly for
sort order in `items_sort_high_before_normal_before_low_deterministically`,
and independently confirmed durable via a direct read-only inspection of
the on-disk database in native verification - see below).

## Data Health (`data_health.rs`, sections 24-31)

A concise checklist, not a score: `TransactionHealth`
(uncategorized/possible-duplicate counts), `ImportHealth` (last import
date, unresolved count), `ReconciliationHealth` (per-account status list,
invalid count, **factual** never-reconciled count), `RecurringHealth`
(pending-review count) - each with a `HealthState` of only `Good` or
`NeedsAttention`. `compute()` reuses `review::list_review_items` filtered
by `.kind`/`.domain` for the transactions/imports/recurring counts (so
Data Health can never drift from what Review itself is showing) and
`accounts::list(conn, false)` + `reconciliation::account_summary` (Batch 4,
unchanged) per active account for reconciliation - `false` for
"don't include archived," so an archived account can never surface as a
live Data Health problem.

`never_reconciled_count` is documented and tested as purely factual: it
never by itself flips `state` to `NeedsAttention` (a never-reconciled
account is not unhealthy, just unreconciled). One test-writing correction
worth recording: `account_summary`'s own pre-existing definition of "last
reconciled" only counts a currently-`completed` record, so invalidating a
reconciliation correctly increments `never_reconciled_count` too (there is
no longer a _current_ completed reconciliation) - the test was fixed to
assert `needs_review: true` instead of a wrong assumption that the
never-reconciled count should stay at zero.

## Financial Signals (`signals.rs`, sections 33-73)

`Existing deterministic services -> compute_signals() -> UI` - every
signal is computed from an already-shipped, already-tested calculation,
never a second formula. `SignalSeverity` (Info/Attention/Important, no risk
scores) and a stable derived `key` (e.g. `budget-pace:dining:2026-09`,
never a random UUID) so identical inputs always produce an identical
signal set (tested directly).

Six signals, deliberately not "every imaginable signal":

- **`category_high_spend`** - `budgets::historical_typical` (Batch 7) +
  `insights::spending_insights` (Batch 8), compared **prorated** by
  elapsed-days-in-month. This proration was not spelled out as a formula
  in the handoff but is required by its own "$318 ... through this point"
  phrasing - comparing a partial month's actual spend against a full
  month's historical typical would make the signal meaningless for three
  weeks of every month. Gated by `MIN_ELAPSED_DAYS_FOR_CATEGORY_SIGNAL = 3`
  (avoid single-transaction day-1 noise) and a combined threshold
  (`SIGNAL_MIN_PERCENT = 25`, `SIGNAL_MIN_CENTS = 5000`) so a $2-to-$4
  coffee habit's "+100%" never signals - both a meaningful percentage _and_
  a meaningful dollar amount are required.
- **`budget_over` / `budget_pace_faster`** - `Budget::status`/`Budget::pace`
  (Batch 7) directly. Only Faster/Over ever signal; every on-track or
  no-budget category is silent, per the handoff's explicit "never signal
  every on-track category."
- **`recurring_price_change`** - `RecurringSeries::is_price_changed`/
  `previous_typical_amount_cents` (Batch 6) directly, inheriting Batch 6's
  variable-utility protection (a `Bill`-kind series with wide natural
  variance) for free - no separate false-positive guard needed.
- **`income_decline`** - new `dates::same_point_previous_month` +
  `accounting::income_in_range`, the same "partial period vs. same elapsed
  days" rule Batch 8's period model already established, never
  partial-current vs. full-previous.
- **`forecast_negative` / `forecast_below_reserve`** -
  `forecast::project` (Batch 8) directly at a 30-day horizon over every
  eligible-cash account combined.
- **`net_worth_decline`** - `accounting::net_worth_change` (Batch 8)
  directly, gated by a documented combined threshold
  (`NET_WORTH_DECLINE_MIN_CENTS = 20000`, `NET_WORTH_DECLINE_MIN_PERCENT = 3`).

Deliberately **not** built, each for a documented reason: a standalone
"total spending above previous period" signal (redundant with the
category-level signals plus Insights' own comparison); "unusually large
one-time expense" (would need a new, undocumented threshold unlike every
other signal, which reuses an existing one verbatim); "unbudgeted
spending" (already surfaced natively in Budgets); "new recurring detected"
(stays Review-only while unconfirmed - signaling immediately after the
user's own confirm action would be pointless). Signal dismissal was
considered and omitted for v1 as disproportionate complexity, per the
handoff's own escape hatch.

## Signals vs. Review vs. Data Health (sections 74-80)

Enforced by construction, not just convention: uncategorized transactions,
possible duplicates, and reconciliation issues are Review/Data-Health
concerns only and are never also computed as Signals (`signals.rs` has no
query touching `needs_review_items`, uncategorized counts, or
reconciliation status at all) - a fact never appears as two cards.

## Overview integration (sections 81-83)

A single "Needs attention" card replaces the old flat "Needs review" card:
total count (`needs_review_count + (top_signal.is_some() as usize)`),
uncategorized items collapsed into one summary line ("4 uncategorized
transactions") rather than listed individually, up to 3 other item titles,
the top signal's own title appended if present, one "Review all ->" link.
Safe to Spend remains the unambiguous hero above it, unchanged in size or
position. Empty state reads "You're caught up." - never a manufactured
warning to avoid looking empty.

## Navigation (sections 84-86)

Every `onNavigate` in this codebase, before and after this batch, is a
plain `(tab: Tab) => void` tab switch - there is no existing deep-linking
mechanism anywhere in the app (grepped every call site). Signal
`navigation_context` is carried on the wire (per the handoff's explicit
Signal-shape requirement) but the frontend does not yet consume it for
routing, since building category/account-level deep-linking would be new
cross-page infrastructure invented solely for Signals - exactly what the
handoff says to avoid ("no new detail screens built solely for Signals").
Every Signal and Review item still lands on its correct destination tab
(Insights/Budgets/Forecast/Recurring/Accounts), verified live for all five
targets.

## Archived-account semantics (sections 56-58, restated from Batch 8)

Two distinct rules, kept distinct: Batch 8 established that an archived
account's _historical_ transactions must never disappear from Insights -
that rule is untouched. This batch adds a second, different rule for the
_active_ Review queue: a derived review item asks the user to act _now_,
which is meaningless for an account nobody manages anymore. All four
derived-item queries and Data Health's reconciliation summary filter to
non-archived accounts; none of them touch historical reporting.

## UI (`ReviewPage.tsx`)

Three tabs (Needs Attention / Signals / Data Health) reusing the existing
`.insights-tab-button` styling from Batch 8 - zero new tab-strip CSS.
Every action reuses an existing form/flow, never a parallel
implementation: "Categorize" opens the real `TransactionForm`; "Keep
Both"/"Mark Duplicate" call the real resolve/keep-duplicate commands;
ambiguous-match candidates render via a small `AmbiguousMatchResolver`
that calls Batch 6's own `linkOccurrence`; "Mark Skipped"/"Mark Pattern
Inactive" call Batch 6's existing occurrence/series commands. Priority and
severity badges carry their state as literal text ("HIGH PRIORITY", "GOOD")
in addition to color. Calm presentation throughout - short titles, evidence
beneath in muted text, one or two plain-text action buttons, no red-alert
dashboard styling, no icons-as-decoration.

## Tests

456 Rust tests total (+32 from Batch 8: 1 in `dates.rs`, 10 in `review.rs`,
7 in `data_health.rs`, 14 in `signals.rs`), clean `clippy -D warnings`,
clean `tsc`/`vite build`. Coverage highlights matching the handoff's own
matrix: uncategorized appear/disappear on categorize; a split transaction
is never flagged uncategorized; income/transfers are never flagged
uncategorized; a proposed pattern is Low priority and closes on confirm; a
missed occurrence appears and **survives an unrelated occurrence refresh**
(the bug-4 regression test); an ambiguous match auto-closes through any
resolution path, not just the dedicated button; an invalidated
reconciliation is High priority; deterministic High-before-Normal-before-Low
sorting; "Keep Both" commits the held row through the real create path and
resolves the item; a clean ledger reports Good everywhere in Data Health;
counts match Review's own counts exactly; an archived/merged payee's
history never causes a false Data Health issue; category-high-spend
signals only above the combined percent-and-dollar threshold (both a tiny
percentage-only case and a meaningful-dollars-but-below-percent case are
proven silent); over-budget and faster-pace signal, on-track never does;
a subscription price increase signals, a variable utility bill never
false-triggers; income decline uses the same-point comparison exactly; a
negative or below-reserve forecast signals as Important; a meaningful net
worth decline signals, a tiny fluctuation never does; identical inputs
produce an identical signal set with stable keys.

## Native Verification

Real Tauri/WebView2 app via the established CDP driver, disposable data
seeded through the app's own commands (one account, recurring series
including a genuine two-series ambiguous-match collision and a genuinely
missed Water Bill occurrence anchored in the past at creation time, an import
with a malformed row and a possible duplicate, an invalidated
reconciliation, two over/near-pace budgets).

**Four real bugs found, all fixed before sign-off** - three only via
driving the live app, not code review alone:

1. The ambiguous-recurring-match staleness bug (reconnaissance, described
   above).
2. **Archived-account leakage**: derived queries had no archived-account
   filter at all, so already-archived test accounts from Batches 5-8 kept
   surfacing stale `reconciliation_needs_review` items forever - found
   live (9 items where only 4 should exist), fixed by filtering every
   derived query to non-archived accounts, re-verified live (count
   corrected to exactly the real seed).
3. **Evidence sign-dropping**: the shared `dollars()` helper's unconditional
   `.abs()` (correct at every other call site) made a genuine $903.19
   overdraft read as the reassuring "Projected cash falls to $903.19" -
   found live, fixed to "Projected cash goes $903.19 negative on
   {date}." matching the handoff's own good-example phrasing, with a
   regression test asserting the word "negative" appears.
4. **Self-healing missed item** (described above under Review Lifecycle) -
   the most subtle of the four, since the triggering action (opening
   Forecast) has nothing to do with Review Center at all.

Every Review action was exercised live end to end: Categorize (through the
real `TransactionForm`, including its pre-existing Batch 5
correction-to-rule dialog firing correctly as an unrelated existing
feature, not a regression), Keep Both (verified the held CSV row commits
as a real new transaction alongside the original, both independently
confirmed via a read-only inspection of the on-disk database), Dismiss,
the ambiguous-match candidate picker (verified the chosen occurrence
becomes `matched` and the _other_ candidate correctly reverts to
`upcoming`, not left dangling), Mark Skipped, and all five Signal/Review
navigation targets (Accounts, Forecast, Insights, Budgets). "Mark Pattern
Inactive" and the `payee_merge_recurring_conflict` action pair were not
separately re-seeded, since both reuse wiring patterns (a mutation button
and a navigation button, respectively) already proven correct by sibling
actions in the same batch - re-manufacturing a scenario solely to click an
identically-wired sibling button was judged not worth the disposable-data
overhead.

One test-harness-only false alarm is worth recording so it isn't mistaken
for a product bug later: two navigation clicks initially appeared to do
nothing when checked via a screenshot taken in the same driver call as the
click. Both resolved cleanly once state was confirmed via a direct,
separate DOM query before screenshotting - an artifact of screenshot/paint
timing in the verification tooling itself, not the application (`onNavigate`
wiring was independently confirmed correct by reading the source directly,
and by every subsequent click succeeding when checked this way).

**Persistence**: rather than restart the actual `midas.exe` process (which
runs under a multi-process `tauri dev` supervision chain - `node
tauri.js` -> `cargo run` -> `cargo run` -> `midas.exe`, alongside a
separately-spawned Vite dev server - with no guarantee it would cleanly
relaunch itself if killed externally, and no need to take that risk against
a shared dev environment), persistence was instead verified by opening the
live SQLite file directly from a second, completely independent read-only
connection (Python's stdlib `sqlite3`, a process that never touched the
running app). Every mutation made through the live UI during this
session's own verification - the kept duplicate transaction, both resolved
`needs_review_items` rows, the ambiguous match's winning/losing occurrence
states, the skipped occurrence, the invalidated reconciliation - was
confirmed durably committed to disk with the exact expected values. This
is a strictly stronger guarantee than a same-process restart would have
been, since the architecture has no in-memory cache anywhere for this data
in the first place (every command takes a fresh `Connection` and queries
directly) - the only real open question was disk durability, which this
directly proves.

**Cleanup**: all disposable data was removed, not merely archived,
following the standing lesson restated in this batch's own handoff -
16 of 17 seeded transactions hard-deleted via `delete_transaction`; the
17th (still referenced by a matched recurring occurrence's foreign key)
archived via `set_transaction_archived` instead, a legitimate alternative
the same instruction explicitly allows; all 6 seeded recurring series set
inactive; both seeded budgets hard-deleted via `delete_budget`; the test
account archived. Verified afterward: `list_review`/`get_signals` both
return empty, `get_data_health` reports Good in all four domains, and the
whole database returns to the same genuine zero-active-account,
zero-current-transaction baseline every prior batch's own cleanup has
left it in.

## Accessibility

Review's three tabs use `role="tablist"`/`role="tab"`/`aria-selected`
(confirmed via direct DOM inspection, not assumed); domain filter buttons
use `aria-pressed`. Priority, severity, and health-state badges all carry
their value as literal text content in addition to color
("high priority", "needs_attention", etc. - confirmed present in the
accessibility-tree text, never color-only). Every action is a real
`<button>` element (no `<div onClick>` anywhere in the new code), so
keyboard operability is native, not custom-built.

## Screen sizes

1280x800 and 1024x640 verified live for both Review (filters wrap, cards
stay full-width and readable, no horizontal overflow -
`scrollWidth === clientWidth` confirmed programmatically) and Overview
(the Needs Attention card reflows without ever pushing Safe to Spend out
of the top position).

## Migration

None. Every new fact is either derived live from already-durable state, or
(for the one case that needed a new persisted field - `duplicate_of_transaction_id`
threaded through "Keep Both") fits inside the existing
`proposed_resolution_json` JSON column, per the handoff's own explicit
preference to avoid a migration when existing persistence already suffices.

## Security

No new Tauri capabilities, no network calls, no telemetry. Every new IPC
command (`list_review`, `keep_duplicate_review_transaction`,
`get_data_health`, `get_signals`) is one the real UI actually calls - no
debug/mutation-only command was added. Signal/Data-Health calculations
stay internal to their Rust modules except their one concrete
query-and-return command each.

## Simplification

Reused rather than duplicated: `pay_cycle::active_account_series`,
`budgets::historical_typical`, `insights::spending_insights`,
`Budget::status`/`pace`, `RecurringSeries::is_price_changed`,
`accounting::income_in_range`, `forecast::project`,
`accounting::net_worth_change`, the existing `TransactionForm`/duplicate
resolve/occurrence-link/occurrence-skip commands, and Batch 8's
`.insights-tab-button` styling. Explicitly rejected per the handoff's own
list: a financial-health score, a second Review system, a separate inbox
per domain, persisted copies of every derived Signal, duplicate
Budget/Insights/Forecast formulas, a generic rules engine for Signals, AI
classification, a notification framework, arbitrary confidence scores, and
a speculative analytics cache.

## Explicit non-goals (unchanged from the handoff)

An AI explainer, Weekly Snapshot, Monthly Review, full Privacy Mode, a
major global redesign, SimpleFIN, mobile, CITADEL sync, a notification
system, and an arbitrary financial-health score.

## Owner decisions applied without escalation

- Phase numbering: `phase-016` (correcting the handoff's own guess of
  "phase-017," since phase-015 was Batch 8, not phase-016).
- `recurring_proposed` items classified as `ReviewPriority::Low` rather
  than the handoff's suggested Normal - independently justified (nothing
  is actively wrong, only an easy one-click confirmation is being
  offered) and incidentally the only thing that gives the `Low` variant a
  genuine live use.
- Signal `navigation_context` carried on the wire but not yet consumed for
  deep-linking, since no page in the app supports it today and building
  that mechanism solely for Signals would be new infrastructure the
  handoff explicitly says to avoid.
- Persistence verified via an independent read-only database inspection
  rather than a full `midas.exe` process restart, given the real risk of
  disrupting the shared multi-process `tauri dev` supervision chain for no
  additional guarantee the architecture doesn't already provide by
  construction (see Native Verification above).
- "Mark Pattern Inactive" and the payee-merge-conflict action pair were
  spot-checked by code/wiring inspection rather than live-reseeded, since
  both reuse action patterns (a mutation button, a navigation button)
  already proven correct live by sibling actions in this same batch.
