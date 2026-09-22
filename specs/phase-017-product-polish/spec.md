# Phase 017: Major UI/UX Polish + First-Run Onboarding + Privacy Mode

- Status: COMPLETE
- Owner gate: APPROVED 2026-09-13 (owner handoff: "MIDAS Development Batch 10
  - Major UI/UX Polish + First-Run Onboarding + Privacy Mode")

## Goal

> MIDAS should now feel like one coherent premium personal-finance
> application rather than a series of separately-developed feature phases.

A polish pass, not a redesign: consolidate the design system that grew
incrementally across 9 prior batches, add first-run onboarding and a
display-only Privacy Mode, and fix the real inconsistencies reconnaissance
found - without touching the accounting engine, the navigation model, or
the green/gold visual identity.

## Reconnaissance

Two parallel passes: a live CDP visual tour of every surface (both themes,
1280x800 and 1024x640, empty and populated data), and a full code-level
audit of `App.css` (1642 lines) plus every `.tsx` file for duplicated
patterns. Findings that drove the rest of this batch:

- **A real, pre-existing bug**: `recurring_occurrences::upcoming_in_range`
  had no `accounts.is_archived` filter at all. Long-archived test accounts
  from Batches 5-9 kept surfacing their still-`confirmed` series on the
  Recurring page's "Upcoming" tab forever - found live (stale "Ambiguous
  Test"/"Netflix"/"City Water Utility" rows with no corresponding current
  test data). Forecast and Signals were never affected, since they always
  pass an explicit, already-filtered account id list; only this function's
  own `account_id: None` "everything" path was exposed. Fixed by joining
  `accounts` and filtering `is_archived = 0` unconditionally, with a
  regression test.
- **At least 7 different page-header shapes**, one CSS class
  (`.accounts-toolbar`) misleadingly reused by pages with nothing to do
  with accounts, and 5 "Settings sub-pages" (Payees/Tags/Rules/Backup/
  CategoryCleanup) that were never built against the same page contract as
  the sidebar-driven pages.
- **10+ visually distinct "card" implementations** with radii ranging
  0.5-1rem and no rule for surface vs. surface-raised background.
- **Badge overload**: `.badge-archived` alone labeled 7 unrelated meanings
  (Archived/Merged/One-time/Split/Undone/Inactive/Duplicate).
- **No destructive button tier at all** - every Delete/Undo/Restore/Reopen
  action used the same visual weight as a harmless Edit/Cancel link.
- **RecurringPage's tab strip used `role="group"`+`aria-pressed`** (a
  toggle-button pattern) for what is structurally identical content-tab
  switching to Insights/Review, which correctly use
  `role="tablist"`/`role="tab"`/`aria-selected`.
- **A real formatting bug**: `ReviewPage.tsx`'s ambiguous-match picker
  rendered a bare `42.50` with no `$` sign - the one place in the app that
  bypassed the shared `formatCents` helper.
- **A real semantic-color bug**: Overview's Safe to Spend hero rendered a
  _negative_ balance in the same reassuring gold as a healthy positive one.
- **One shared money-formatting helper (`formatCents`) is already used
  almost everywhere** - the load-bearing fact that made a centralized
  Privacy Mode architecturally cheap (see below).
- **Import Profiles management** (promised in Batch 3, per
  `PROJECT_STATE.md`) was still entirely unbuilt - the backend commands
  (list/create/update/delete/setArchived) existed but no UI ever called
  four of them.
- Naming: sidebar said "Budget" while the page itself said "Budgets."

## Design System

- **`.accounts-toolbar`/`.accounts-toolbar-actions` renamed to
  `.page-toolbar`/`.page-toolbar-actions`** everywhere (10 files) - the
  same shape, an honest name, now applied to the Settings sub-pages too.
- **`.data-health-row` retired in favor of the existing `.review-item`**
  (one CSS class deleted, one JSX class swap) - it was an accidental
  near-duplicate of a card style that already existed for the exact same
  feature family.
- **New `.badge-neutral`**, visually identical to the existing
  low-priority/info badge treatment, given to "Split," "One-time," and
  "Merged" labels so they stop borrowing `.badge-archived`'s literal
  meaning.
- **New `.btn-danger`** (outlined red at rest, filled on hover/focus) and
  **`.btn-link-danger`** (a red variant of the existing text-link) applied
  to every genuinely irreversible action found: Delete (Budget/Tag/Rule),
  Undo import, Restore backup, Reopen reconciliation. Archive/Unarchive
  actions were deliberately left as plain `.btn-link` - they are
  reversible and shouldn't read as alarming.
- **RecurringPage's pill tabs replaced with the existing
  `.insights-tabs`/`.insights-tab-button` classes** (already shared by
  Insights and Review) plus correct `role="tablist"`/`"tab"`/
  `aria-selected` - both a visual and an accessibility fix in one change,
  and a net deletion of the now-dead `.recurring-tab` CSS.
- **`.field-required`** (a gold `*` via `::after`) applied to every
  `required` field's `<label>` across the most-used forms (Account,
  Transaction, Budget, Rule, Goal, Payee).
- **`tabular-nums` extended** to the app's largest numbers
  (`.headline-number`, `.hero-number`, `.reconcile-difference-amount`,
  `.stat-strip-item dd`) - previously only small in-table figures had it.
- **`.hero-number-negative`**: Overview's Safe to Spend now renders in
  `--midas-negative` red, not gold, whenever the figure is actually below
  zero, in both the complete and incomplete pay-schedule states.
- **Correct heading hierarchy in Settings**: the 5 sub-pages nested under
  a new "Rules & Data" group (`<h2 className="settings-group-heading">`,
  styled like the sidebar's own group labels) stepped their own headings
  down from `<h2>` to `<h3>` (and BackupPage's 3 internal sub-headings from
  `<h2>` to `<h4>`), since they're no longer top-level page headings once
  visually grouped - added the missing bare `h3`/`h4` CSS rules this
  exposed were never defined.
- Four small formatting-consistency fixes found by the code audit: the
  `$`-less bug above; `ReconciliationWorkspace.tsx` duplicating
  `centsToDollarInput`'s own logic (now calls it directly, which also
  picked up its negative-zero guard); a hardcoded `"$0.00"` literal
  replaced with `formatCents(0)`; `SplitEditor.tsx`'s own duplicated
  `.toFixed(2)` similarly replaced; one raw ISO date
  (`InsightsPage.tsx`'s Net Worth trend table) now runs through the
  existing `formatDate`; a dead `.import-history-dialog` CSS class
  reference (used in JSX, never defined) given a real definition rather
  than deleted, since two independent dialogs depended on it consistently
  enough to suggest genuine original intent.

## Navigation

Sidebar label "Budget" -> "Budgets" (matches the page's own plural title,
consistent with "Transactions"/"Accounts"). No other navigation change -
the ten-tab, five-group sidebar structure is unchanged, per the explicit
"do not grow the sidebar" / "do not replace the navigation model"
boundary.

## Deep Links (section 13)

`Signal.navigation_context` (added in Batch 9) is carried on the wire but
still not consumed for routing. Every `onNavigate` in this codebase, before
and after this batch, is a plain `(tab: Tab) => void` - grepped every call
site (Overview, Recurring, Transactions, Review, Signals) and confirmed
none of them support a second parameter today. Building category/account-
level deep-linking would mean inventing new cross-page routing
infrastructure solely for Signals, which is exactly what the handoff's own
"do NOT create a generic router-state framework if current navigation
cannot support this simply" explicitly warns against. All five Signal/
Review navigation targets (Accounts, Forecast, Insights, Budgets,
Recurring) still land correctly on their destination tab.

## Overview

Unchanged hierarchy (Safe to Spend hero, then Upcoming/Pay Cycle/Spending,
then Needs Attention/Goals/Budget) - reconnaissance found this structure
already sound; the fixes here were the negative-color bug above and
Privacy Mode coverage, not a rebuild. No redundant-numbers rewrite was
needed: Spending (all categories) and Monthly Budget (budgeted categories
only) are already visually and positionally distinct cards, not stacked
duplicates of one number.

## Transactions

Reordered the table's columns from
`Date/Account/Merchant/Category/Type/Amount/Status/Actions` to
`Date/Merchant/Amount/Category/Account/Status/Actions` - Amount was
previously the 6th of 9 columns, invisible without horizontal scroll at
1280px; it's now the 3rd. The standalone "Type" column was folded into the
existing status-badge cell as a `.badge-neutral` label shown only for
non-Purchase rows (Income/Transfer/Refund), since Purchase is the
overwhelming default and doesn't need its own badge on every row (matching
section 20's "compact indicators only where they materially help
scanning"). Live-verified: Date/Merchant/Amount/Category are all visible
without scrolling at 1280x800 now, versus needing to scroll for Amount
before.

Empty state now distinguishes three real cases instead of one generic
message: zero accounts ("Create the account in MIDAS before adding or
importing its transactions"), active filters excluding everything ("No
matching transactions... try clearing one or more"), and genuinely zero
transactions ("No transactions yet. Add one manually or import a
statement." with an inline Add transaction button) - previously all three
showed the same "No transactions match these filters," which was actively
misleading in the first and third cases.

## Settings

Reorganized into named groups without changing any sub-page's own
behavior: Personalization (moved first, matching "General" positioning) ->
Appearance -> Financial Preferences -> **Rules & Data** (new grouping
label; Payees, Tags, Clean up categories, Rules, Import Profiles, Backup &
Restore) -> About. "Rules & Data" is the one genuinely new visual grouping

- the other four sections already read clearly on their own.

**New: Import Profiles management** (`ImportProfilesSection.tsx`),
fulfilling the gap flagged in Batch 3's own `PROJECT_STATE.md` entry. Lists
every saved profile (name, source type, associated account or "Any
account"), with Rename (a small dialog, matching every other rename flow
in the app) and Archive/Restore. No "Add profile" button - a profile is
created implicitly the first time a CSV mapping is saved during import, so
this is purely a management surface over data that already exists, built
entirely on Batch 3's existing backend commands with zero new IPC.

## First-Run Onboarding

`OnboardingFlow.tsx`, five steps (Welcome -> Privacy -> Account ->
Transactions -> Done), each reusing a real existing form as a modal
dialog rather than a second implementation: the Account step opens the
real `AccountForm`, the Transactions step offers the real `TransactionForm`
or `ImportWizard`. No demo data is ever fabricated.

**First-run detection needs no new persistence at all** (section 30's own
explicit allowance): a database that has never had a single account -
archived ones count, since this is "has anyone ever set this up," not "is
anything active right now" - is already an unambiguous, self-clearing
signal. `App.tsx` checks `accountsApi.list(true).length === 0` once on
mount; the moment onboarding's own account step succeeds, the same check
naturally recomputes to `false` on the next reason to ask. This correctly
does _not_ trigger on "all accounts archived" or "no transactions this
month," both explicitly named in the handoff as states that must not be
mistaken for a fresh install.

"Skip setup" (from the Welcome step) suppresses onboarding for the rest of
the session only (a plain `useState`, never persisted) - specifically so a
user who wants to restore an existing backup first isn't blocked from
reaching Settings, while a database still genuinely empty next launch is
offered onboarding again rather than treated as a dead end.

**A real bug found and fixed during verification**: `AccountForm`'s
`onSaved` callback took no arguments, so onboarding's own re-fetch-and-
guess approach for "which account did I just create" picked the wrong one
whenever other accounts already existed (exposed by the test methodology
itself - see Native Verification). Fixed by widening `onSaved` to
`(savedAccount: Account) => void` (backward-compatible - existing callers
that ignore the argument are unaffected, confirmed by a clean rebuild) and
passing the real created/updated account straight through.

Keyboard-only verified: `autoFocus` on each step's primary action means a
user can complete the entire flow with nothing but repeated Enter
presses; no swipe/carousel gesture exists anywhere.

## Privacy Mode

Display-only masking, explicitly documented as distinct from encryption.
Architecture (`lib/currency.ts`): a single module-level reactive flag,
checked directly inside the existing `formatCents` helper, so all ~110
pre-existing call sites across 20 files get masking for free with **zero
per-call-site rewrite** - each consuming component adds exactly one
`useMasked()` subscription (a thin `useSyncExternalStore` wrapper) so it
re-renders when the flag flips; every `formatCents()` call already in that
component's render then reflects the new state automatically. This was
chosen over a `<Money>` wrapper component specifically because the
existing codebase already funnels essentially all money display through
one function - centralizing at that single point is a smaller, more
robust diff than converting ~110 JSX call sites, and it can't be
"forgotten" at some future call site the way a new component's adoption
could be.

- **Masked value**: a fixed string, `$••••.••`, identical regardless of
  the real amount's magnitude, sign, or digit count (verified: a $12.54
  and a $1,284,529.87 mask identically).
- **`centsToDollarInput` (populates editable amount inputs) is a
  completely separate function and is never masked** - editing a
  transaction/account amount always shows the real figure, satisfying
  section 47 by construction rather than a runtime special case.
- **Server-composed evidence sentences** ("Dining is over budget by $57.00
  this month," from Batch 9's Review/Signals) don't call `formatCents` at
  all - they arrive as plain strings from Rust. A new `maskMoneyInText()`
  regex-replaces embedded dollar figures in these strings client-side, so
  the backend needs no notion of display privacy at all. Applied to both
  Review item evidence and Signal evidence.
- **Accessible name masking is automatic, not a parallel concern**: since
  masking happens inside `formatCents` itself, every existing `aria-label`
  built from a `formatCents()` call (the spending donut chart, price-
  history sparklines) masks identically to the visible text - verified
  live that the donut's `aria-label` reads "Spending by category,
  $••••.•• total" rather than leaking the real number.
- **Toggle**: an eye icon in the sidebar footer (`aria-pressed`, a
  `title` tooltip with the full "Hide/Show financial amounts" phrase, and
  a `.sr-only` span carrying that same full phrase as the accessible name
  - the visible label is shortened to "Hide/Show amounts" only to avoid an
    ugly two-line wrap in the fixed-width sidebar).
- **Session-only, never persisted** (the handoff's own recommendation) -
  confirmed live: reloading resets it to off every time.
- **Charts**: donut slice proportions still reflect real relative
  magnitude even when masked (the handoff's own explicitly accepted
  scope boundary - "masks numbers, not chart geometry").
- Coverage: all 19 files that call `formatCents` (Overview, Accounts,
  Transactions, Recurring, Budgets, all 5 Insights tabs, Forecast, Review
  (items + Signals evidence), Goals, reconciliation workspace/history/
  balance-preview, import wizard/history, split editor, category donut,
  category/merchant drilldowns, pay cycle) plus the two Review/Signals
  evidence-string call sites.

## Empty States

Consolidated into a shared `.empty-state` class (a generalization of the
`.review-empty-state` pattern Batch 9 already built) - heading + message +
optional inline primary action. Applied with real inline actions to
Accounts and Transactions (both previously bare text with no inline
button). Forecast, Review, and Data Health already had good copy from
prior batches and needed no rewrite. The remaining list pages (Payees,
Tags, Rules, Savings Goals, Recurring's per-view empties, Insights tabs,
the two drilldowns) were deliberately left as plain `<p className="muted">`
text - a reasonable, non-blank minimal empty state already, and rewriting
copy for every list view in the app was judged lower-value than the
pages users actually land on first.

## Accessibility

- RecurringPage tabs: `role="tablist"`/`"tab"`/`aria-selected` (was
  `role="group"`/`aria-pressed`) - verified live.
- Settings: correct `h1 > h2 > h3 > h4` heading nesting throughout,
  confirmed via direct DOM inspection of every heading on the page.
- Privacy toggle: `aria-pressed` tracks state; full accessible name
  ("Hide/Show financial amounts") preserved via `.sr-only` even though the
  visible label is shortened; masked amounts never leak through
  `aria-label` (see Privacy Mode above) - the one explicitly
  "critical" requirement in the handoff.
- Onboarding: every step's primary action receives `autoFocus`; the whole
  flow is completable with Enter alone; no carousel/gesture dependency.
- Required fields carry a visible `*` in addition to the native
  `required` attribute.
- New destructive buttons (`.btn-danger`) are real `<button>` elements
  with plain text content, not icon-only - no separate accessible-name
  work needed.

## Screen Sizes

1280x800 (primary) and 1024x640 verified for Overview, Transactions,
Recurring, Settings, and Review - no page-level horizontal overflow
(`document.body.scrollWidth === clientWidth`) at either size; Safe to
Spend remains the top element at both. Both themes spot-checked on
Overview, Budgets, Transactions, and Recurring; the badge/table/tab
changes in this batch were verified in both light and dark.

## Migration

None. Onboarding's first-run signal reuses existing account data as its
own implicit flag (see above); Privacy Mode is pure frontend state; every
other change is CSS, JSX, or a backend query filter fix. `AccountForm`'s
`onSaved` signature change is source-compatible, not a schema change.

## Tests

457 Rust tests (+1: `upcoming_in_range_excludes_confirmed_series_on_an_
archived_account`, the archived-account leakage regression), clean
`clippy -D warnings`. 17 frontend tests (+4, all in the new
`tests/currency.test.mjs`: Privacy Mode masks to a fixed placeholder
regardless of magnitude, real formatting when off, `maskMoneyInText`
masks embedded figures in a server-composed sentence without touching the
surrounding words, and masks multiple figures in one sentence). Clean
`prettier`/`tsc --noEmit`/`vite build`.

## Native Verification

Real Tauri/WebView2 app via the established CDP driver throughout. The
dev process tree (`node tauri.js dev` -> `cargo run` -> `cargo run` ->
`midas.exe`) stopped unexpectedly once mid-session for reasons outside
this session's own actions (confirmed via process inspection - nothing
was killed intentionally) and was cleanly relaunched with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`; a
genuine process restart during this relaunch incidentally re-confirmed
persistence (three seeded Batch 10 accounts and two budgets survived it
byte-for-byte).

Onboarding could not be triggered against a genuinely empty database
without moving the real dev database file aside - a filesystem action
Auto Mode's safety classifier declined, correctly treating a rename of a
live application data file as sensitive even though the intent was a
reversible, temporary swap. Per the owner's own explicit choice among the
alternatives offered, verification instead used a one-line, clearly
marked temporary override (`setNeedsOnboarding(true || ...)`, comment-
tagged `TEMP-BATCH10-TEST`) to force the onboarding branch on top of the
real (populated) dev database, exercised the complete flow end to end
including the real `AccountForm`/`TransactionForm` dialogs, found and
fixed the `onSaved` account-identity bug this exposed, verified the "Skip
setup" path and full keyboard-only completion, then reverted the override
(confirmed via `grep` and a clean rebuild) before any further work. This
means the "zero accounts" _detection_ condition itself was verified by
code inspection rather than a live empty-database run; the _flow_ once
triggered was verified completely live.

Privacy Mode verified live end to end: toggling while already on a page
(no navigation needed) correctly re-rendered every visible amount to the
fixed mask on Overview, Recurring, and Review/Signals in the same
interaction; evidence-sentence masking verified on two live Signals
("Dining is over budget by $••••.•• this month," "Projected cash goes
$••••.•• negative on..."); chart `aria-label` masking verified directly
via DOM inspection, not assumed.

Every design-system fix was re-verified live after building: the
Recurring archived-account leak (stale rows gone after the fix, "Nothing
expected in the next 30 days" once cleanup was complete), the reordered
Transactions table (Date/Merchant/Amount/Category all visible without
scrolling), the unified Recurring tab styling and ARIA roles, the Settings
heading hierarchy and new Import Profiles section, and the negative
Safe-to-Spend color fix (both themes).

All disposable Batch 10 data (5 accounts, 10 transactions, 2 budgets, 1
recurring series - including the two accounts created live during
onboarding testing) was then removed: transactions hard-deleted, the
recurring series set inactive, budgets hard-deleted, accounts archived.
Confirmed afterward: 0 active accounts, `list_review`/`get_signals` both
empty, `get_data_health` Good in all four domains, and the Recurring
Upcoming tab genuinely empty - the same clean baseline every prior batch's
own cleanup has left the ledger in.

## Security

No new Tauri capabilities, no network calls, no new dependencies. `cargo
audit` clean (7 pre-existing accepted transitive advisories, unchanged
baseline), `npm audit` 0 vulnerabilities, Gitleaks 0 leaks across 73
commits, Trivy 0 critical/high findings (1 pre-existing accepted MEDIUM
`glib` advisory, the same one already in the `cargo audit` baseline - a
GTK-related Linux binding unreachable from this Windows/WebView2 app).
Privacy Mode is explicitly documented as display-only, never claimed as
encryption or a security boundary.

## Simplification

Reused rather than duplicated throughout: the existing `formatCents`/
`centsToDollarInput` split (made Privacy Mode nearly free), the existing
`.insights-tabs` classes (unified Recurring's tabs instead of a third
tab system), the existing `.review-empty-state` pattern (generalized
instead of building a new empty-state component), `AccountForm`/
`TransactionForm`/`ImportWizard` (onboarding never reimplements a form),
and every existing Import Profile backend command (zero new IPC for that
section). Explicitly rejected per the handoff's own list: a design-system
framework rewrite, an animation/tour library, a third-party chart
library, a global state library for Privacy Mode (a module-level store
plus `useSyncExternalStore` - already in React - was sufficient), a
dashboard widget system, new sidebar destinations, and a generic deep-
linking/router framework for Signals.

## Product Freeze

Per section 105, the following are now treated as stable for Desktop 1.0
and should not be redesigned again without a new, explicit owner
decision: the ten-tab five-group sidebar navigation; the page-toolbar/
card/badge/button-tier/tab-strip/empty-state visual language established
in this batch; the Overview information hierarchy (Safe to Spend hero,
then Upcoming/Pay Cycle/Spending, then Needs Attention/Goals/Budget); the
onboarding step sequence and its reuse-only-existing-forms approach; and
the Privacy Mode architecture (formatCents-level masking plus
`maskMoneyInText` for server-composed evidence). Release hardening should
fix bugs and rough edges within this frozen shape, not rearchitect it.

## Owner decisions applied without escalation

- Phase numbering: `phase-017` (the handoff's own guess, confirmed correct
  since phase-016 was Batch 9).
- Privacy Mode implementation: centralized inside the existing
  `formatCents` primitive rather than a new `<Money>` wrapper component,
  since the codebase's existing near-universal use of one formatting
  function made that the smaller, more robust diff (see Privacy Mode
  above) - both were explicitly offered as acceptable in the handoff.
- Transaction table's "Type" column folded into the status-badge cell
  rather than dropped outright or kept as its own column, preserving the
  information (Income/Transfer/Refund are still visible) while freeing
  the width Amount needed to become visible without scrolling.
- "Rules & Data" is the only new Settings grouping label added -
  Personalization/Appearance/Financial Preferences were judged already
  clear on their own and left as top-level sections rather than nested
  under an invented "General" umbrella.
- Import Profiles section offers Rename/Archive/Restore only, no "Add" or
  "Delete" - matching the handoff's own explicit list for this feature and
  the existing backend's own established archive-not-delete convention
  for anything an import may reference.

## Owner decision required mid-batch (not escalated further after answer)

Onboarding verification against a genuinely fresh database required
either modifying the real application data file (blocked by Auto Mode's
safety classifier) or an alternative approach. Per section 110's own "ask
only if... a genuinely consequential UX decision cannot be inferred," this
qualified: four options were presented (temporary code override, owner
moves the file, owner grants the specific action, skip live verification
entirely) and the owner selected the temporary-override approach, used
exactly as described in Native Verification above.
