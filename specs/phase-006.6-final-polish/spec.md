# Phase 006.6: Final product polish (pre-v0.1.0)

- Status: COMPLETE (feature work, verification, and reporting) — release packaging (installer rebuild, `v0.1.0` tag, Gitea release) remains explicitly out of scope, held for the owner's separate visual/product approval.
- Owner gate: APPROVED — a small, scope-frozen handoff issued after Phase 6.5's UI consolidation, adding exactly five things: richer account/source setup (largely already present - confirm and close gaps), deterministic budget suggestions from spending history, one spending-by-category visualization, small contextual CSV-import help, and optional lightweight display-name personalization. Explicit scope freeze against bank sync, AI categorization, cloud sync, multi-user, tax tools, investment feeds, money movement, a new plugin architecture, and a full charting dashboard.

## Goal

Close the small remaining gaps between "MIDAS works" and "MIDAS is easy to understand and act on immediately" - without opening a new open-ended feature cycle.

## Scope

- **Account/source setup**: audited the existing `AccountForm`/`AccountInput` - name, institution (optional, freeform), account type, starting balance, "include in net worth," and notes were already all present and already exactly matched the handoff's "required/optional" split. No account-form changes were needed. Confirmed the CSV import account selector already shows real custom account names (`a.name`), not generic types; relabeled it "Import into account" for clarity per the handoff's suggested wording.
- **CSV import prerequisite/help**: `TransactionsPage.tsx`'s existing "create an account first" empty state (which already correctly disables Import CSV/Add transaction when there are no accounts) now includes an inline "Add account" link that opens the real `AccountForm` directly - no second account-creation implementation. Added a small `<details>` "Where do I get a CSV?" disclosure inside the import wizard's first step, matching the handoff's suggested copy exactly.
- **Budget suggestions**: new `suggest_budgets` in `src-tauri/src/budgets.rs`, exposed as the `get_budget_suggestions` command. Reuses `reports::spending_by_category` for each of the last 3 calendar months before the target month, so a suggestion is classified identically to Reports (transfers/opening balances/balance adjustments already excluded there, refunds already netted, credit-card payments already modeled as transfers) - zero duplicated classification logic. Averages only over the months that actually had spending in a given category (no fabricated zero months), rounds to the nearest $5, and reports each existing budget's current planned amount alongside the suggestion without ever touching it. New "Suggest from spending" UI on the Budget page: a review table (category / recent average / current / editable suggested amount / months of history) with per-row checkboxes defaulting to selected only for categories with no existing budget, and a single "Apply N selected" action - nothing is written until that button is pressed.
- **Spending visualization**: a native SVG donut chart on the Reports page (`CategoryDonut` in `ReportsPage.tsx`), built from the existing `spending_by_category` data already fetched for the pre-existing category list - no new backend endpoint, no charting dependency. A small curated 6-color palette (`--chart-1`..`--chart-6`, plus a neutral border-gray for "Other") keeps it visually cohesive with the deep-green-and-gold identity rather than reading as a generic chart-library rainbow. Categories beyond the top 5 consolidate into an "Other" slice in the chart only; the accessible list below always shows every real category individually, so no data is ever hidden. Clicking a chart slice (or a list row - both drive the same state) opens the exact same `CategoryDrilldown` component Reports already had, and dims the other slices to reinforce which one is selected. "Other" itself isn't clickable, since it doesn't correspond to one real category.
- **Display-name personalization**: a "Personalization" section in Settings with a single "Display name" field, stored in `localStorage` exactly like the existing theme preference (no backend, no new Tauri capability). When set, Overview shows one small muted line ("Good morning/afternoon/evening, {name}") below the page title - visually far below the Safe-to-Spend hero, never competing with it.
- **One verified-defect-adjacent test fix**: none needed - all new code is additive.

## Explicit non-goals

- A "Purpose/Role" account field - the handoff explicitly permitted skipping optional fields that don't fit cleanly, and the existing free-text Notes field already covers "VA compensation is deposited here"-style context without building a categorical income-source taxonomy the handoff separately warned against.
- Any change to `accounts.rs`, the accounts table schema, or account-level classification - the existing fields already satisfied every "required" item in the handoff.
- Any change to the accounting engine, sign convention, or existing Reports/Budgets classification logic - `suggest_budgets` and the donut both consume `spending_by_category` exactly as Reports already did.
- A generic chart abstraction, a recommendation-engine framework, or a settings/profile framework - one `CategoryDonut` function, one averaging function, one text field.
- Rebuilding the installer, creating/pushing the `v0.1.0` tag, or publishing a Gitea release.

## Verified current state

- 180 Rust tests (+6 from Phase 6.5's 174: `suggest_budgets` averaging, partial-history averaging, transfer/opening-balance/balance-adjustment exclusion and refund netting, existing-budget-preserved, no-fabricated-suggestion, and empty-history cases).
- Frontend gate clean: `prettier --check`, `tsc --noEmit`, `node --test`, `vite build`.
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean.
- `cargo audit`: exit 0, same accepted transitive-dependency advisories as Phase 6.5/7 (no new dependency added).
- `npm audit`: 0 vulnerabilities. Trivy: 0 vulnerabilities across both lockfiles, 0 secrets. Gitleaks: 0 leaks across 36 commits.
- Live-verified via WebView2 CDP against the real dev app with realistic multi-month, multi-category seeded data (created through the app's own IPC):
  - Budget suggestions: generated suggestions for 9 categories, hand-confirmed the rounding-to-nearest-$5 on every row, confirmed the two categories with existing budgets (Groceries, Dining) defaulted unchecked and showed their real current planned amounts, applied the 7 unchecked-by-default-but-selected new categories, and confirmed afterward that Dining/Groceries/Rent's planned amounts were byte-for-byte unchanged.
  - Donut chart: confirmed exactly 6 `<circle>` elements render for 7 real categories (5 named + 1 "Other"), confirmed the accessible list still shows all 7 individually, confirmed clicking a list row both opens the real transaction drilldown and dims every other slice to `opacity 0.4`, confirmed both light and dark theme rendering.
  - CSV import: confirmed the account selector shows a real custom name ("Emergency Fund") under an "Import into account" label, confirmed the "Where do I get a CSV?" disclosure expands with the intended copy.
  - Display name: set "Tyree" in Settings -> Personalization, confirmed "Good evening, Tyree" (time-of-day-aware) appears as a small muted line on Overview, visually subordinate to the Safe-to-Spend hero.
  - 1280x800 and 1024x640: no page-level horizontal scroll at either size (`document.body.scrollWidth === innerWidth` confirmed programmatically at 1024px, not eyeballed) - the Reports donut+list layout and the Budget suggestion table both remain usable, with table overflow contained to `.table-scroll`'s own internal scroll as intended.

## Acceptance criteria (owner's completion checklist)

1. Accounts clearly support custom names/institutions - [x] already did; confirmed live.
2. CSV import clearly targets the named account - [x] relabeled "Import into account," already showed real names.
3. CSV-source help exists - [x] "Where do I get a CSV?" disclosure.
4. Budget suggestions are deterministic and user-approved - [x] nothing applied without explicit "Apply N selected."
5. Existing budgets are not silently overwritten - [x] live-verified unchanged after applying only new-category suggestions.
6. Spending donut correctly represents spending categories - [x] verified against the same total Overview/Reports already agree on.
7. Donut links into real transaction drilldown - [x] reuses `CategoryDrilldown` exactly.
8. Chart remains accessible without color dependence - [x] full-text accessible list always shows every category; donut is a visual supplement, not the only interface.
9. Optional display name is lightweight if implemented - [x] one `localStorage` key, one input, one muted line.
10. No backend finance regressions - [x] 180/180 tests, zero existing-function edits.
11. Full test/security gates pass - [x].
12. UI still looks like the approved consolidated MIDAS design - [x] verified in both themes.
13. Changes are committed and pushed to Gitea - [x] (see commit).

## Test strategy

Same live-verification discipline as every prior phase: real `tauri dev` process with WebView2 remote debugging, driven by the same purpose-built CDP client used in Phase 6.5. Sample data (accounts, multi-month multi-category transactions, budgets) created entirely through the app's own IPC commands, never written directly to the database. Backend suggestion math verified with unit tests using hand-computed expected values (including confirming integer-division truncation behavior, not idealized floating-point rounding).

## Owner decisions

1. Review the required screenshots (Add Account, CSV import help, Budget suggestions, Reports donut, Reports drilldown, Overview with greeting, Settings personalization, 1024x640 view) and approve or request changes.
2. Once approved: authorize resuming Phase 7's packaging - rebuild the installer against this code, re-hash it, then the `v0.1.0` tag and any Gitea release publication.

## Hard-stop conditions

- Any change to accounting logic - none arose; every new number flows through the exact same classification the app already had.
- Rebuilding the installer, tagging, or publishing without the owner's approval - not done.
