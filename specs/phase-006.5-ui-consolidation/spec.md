# Phase 006.5: UI consolidation (pre-v0.1.0 product polish)

- Status: COMPLETE (UI work, verification, and reporting) — release packaging (installer rebuild, `v0.1.0` tag, Gitea release) is explicitly out of scope for this phase and remains held for the owner's separate release gate.
- Owner gate: APPROVED — a detailed 29-section product-polish handoff issued after Phase 7's technical release candidate passed. Core mandate: replace the 13-item top navigation with a grouped left sidebar, make Safe to Spend the dominant Overview hero, adopt a darker deep-green-and-gold identity with a new raised-surface token, fold Safe to Spend/Pay Cycle/Rules/Backup/Theme controls out of primary navigation into Overview/Settings, polish (not redesign) the remaining pages, validate at 1280x800 and 1024x640, reverify Phase 6 accessibility work, and stop for owner visual approval before any release work resumes. Explicitly no backend/accounting logic changes except to fix a verified defect the UI work exposes, no new finance features, no installer rebuild, no tag, no publish.

## Goal

Make MIDAS feel like one coherent financial home rather than thirteen separate subsystems exposed at once, without touching the validated accounting engine underneath it.

## Scope

- **Navigation shell**: `src/App.tsx` rewritten around a left sidebar (`src/App.css` `.sidebar*` rules), grouped Primary (Overview) / Money (Transactions, Accounts) / Planning (Budget, Bills, Goals) / Insights (Reports, Net Worth) / Review (Needs Review), with Settings pinned at the bottom. Reduced the routed tab set from 13 to 10. Removed the old top `.main-nav`/`.nav-tab` bar and the header entirely (each page already carries its own `<h1>`; a separate header title would have duplicated it - see Findings).
- **Overview hierarchy**: `src/OverviewPage.tsx` rebuilt around a dominant Safe-to-Spend hero (`.overview-hero`, raised surface, headline number, one-line cash/bills/reserved summary, a native `<details>/<summary>` "View breakdown" disclosure reusing the exact breakdown table the old standalone Safe to Spend page showed), a Tier-2 row of equally-sized cards (Upcoming Bills, Current Pay Cycle, Spending - now also showing income), and a quieter Tier-3 (Needs Review, Savings Goals, then Cash position/net worth as a compact `<dl>` stat strip instead of a table, Budget health, Recent transactions). Dropped the "Net worth, last 6 months" table from Overview since it duplicated the standalone Net Worth page almost exactly (`net_worth_trend` stays in the API response, unused by Overview - see Non-goals).
- **Palette**: added `--midas-surface-raised` to both themes (dark matches the handoff's own suggested values exactly; light is a barely-tinted `#fffdf7`), used for the Overview hero and the sidebar to create a two-level elevation system. Changed `.btn-link`'s default color from solid gold to muted text, with gold only on hover/focus - gold is now reserved for the brand mark, primary buttons, active nav, progress fill, and Safe-to-Spend emphasis, matching the handoff's "restrained, not a generic white replacement" guidance.
- **Settings consolidation**: new `src/SettingsPage.tsx` with five sections - Appearance (the theme fieldset, moved verbatim from the old header), Financial Preferences (pay-schedule status/edit and the Safe-to-Spend reserve form), Categorization (`RulesPage` embedded as-is), Data & Backup (`BackupPage` embedded as-is), About (static version/privacy/data-path text - no new Tauri capability added just to read the version at runtime). `PayCyclePage.tsx` and `SafeToSpendPage.tsx` were deleted; their genuinely reusable pieces (the `ScheduleForm` dialog, `suggestScheduleFromSeries`, `SCHEDULE_TYPE_LABELS`, the reserve-editing form) survive as trimmed, renamed files (`PayCycleSettings.tsx`, `SafeToSpendReserveForm.tsx`) with their old read-only summary displays removed, since Overview's hero and Tier-2 card now cover that ground.
- **Secondary-page polish**: Bills (`RecurringPage.tsx`) heading changed from "Recurring & Bills" to "Bills" and its sections reordered so the actual upcoming bills table leads, with the detection queue ("Proposed") second - detection is implementation detail, not the primary frame. Every other secondary page (Accounts, Transactions, Budget, Goals, Reports, Net Worth) was checked for oversized empty states and left untouched - they were already compact single-line messages, not the large wasted cards the handoff described.
- **One verified-defect backend fix**: `src-tauri/src/safe_to_spend.rs`'s `Incomplete` reason string told the owner to "set one on the Pay Cycle page" - a page that no longer exists in primary navigation after this phase's own restructuring. Updated the string to point at Settings -> Financial Preferences. One line, no logic change, test unaffected (asserts non-empty, not literal text).
- **Verification**: live CDP verification against the real running dev app (WebView2 remote-debugging, the same discipline used every phase this session) at 1280x800 and 1024x640, in both themes, with seeded realistic sample data (3 accounts, ~25 transactions, 3 budgets, 1 savings goal) created through the app's own IPC. Full automated gate: frontend (`prettier`, `tsc`, `node --test`, `vite build`), Rust (`fmt --check`, `clippy -D warnings`, `cargo test` - 174/174, unchanged from Phase 7), `cargo audit` (same 17 pre-existing accepted transitive-dependency advisories as Phase 7, nothing new), `npm audit` (0), Gitleaks (0 real leaks across 35 commits), Trivy (0 vulnerabilities in both lockfiles).

## Explicit non-goals

- Any change to accounting logic, the sign convention, Safe-to-Spend math, pay-cycle calculation, net-worth calculation, recurring detection, the CSV import pipeline, or the backup/restore architecture - untouched, per the owner's explicit list.
- Any new finance feature (bank sync, AI categorization, new reports, investment tracking, tax tools, money movement, multi-user, cloud sync) - none added.
- Trimming the now-unused `net_worth_trend` field out of `OverviewResult`/`overview.rs` - it's still legitimately used by the standalone Net Worth page's own history call, and removing it from the Overview response specifically would mean touching backend aggregation code for a UI-only reason, which the owner's mandate reserves for verified defects only. Left in place, unread by the new Overview.
- A settings framework, generic `SettingsSection` abstraction, or icon library for the sidebar - plain `<section>`/`<div>` composition and text-only nav labels were sufficient at this app's scale (10 items, 5 sections).
- Rebuilding the installer, creating/pushing the `v0.1.0` tag, or publishing a Gitea release - explicitly held for the owner's visual approval first.

## Findings

- **Sidebar grew to match main-content height instead of staying pinned.** `.app-shell` used `min-height: 100vh` with an implicit single grid row, so the row (and therefore the sidebar, via default `align-items: stretch`) sized to the taller of its two columns - the Overview page's full content, well over 1179px. The Settings link at the bottom of the sidebar was pushed off-screen. Fixed by making `.app-shell` `height: 100vh` (definite, not minimum) and giving `.main-column` its own `overflow-y: auto` so only the content column scrolls while the sidebar stays fixed at viewport height. Caught via live measurement (`getBoundingClientRect()` on the sidebar and its footer), not by eyeballing a screenshot.
- **Footer floated in the middle of short pages instead of sitting at the bottom.** `<main>` sized to its own content with no `flex: 1`, so on a page shorter than the viewport (e.g. Budget), the footer appeared right after the content with dead empty space below it rather than pinned to the bottom of the viewport. Fixed with `flex: 1 0 auto` on `main`.
- **Stale UI cross-reference in a backend string.** Covered above under "one verified-defect backend fix."

## Verified current state

- 174 Rust tests, unchanged from Phase 7 (no accounting/backend logic touched beyond the one string).
- Frontend gate clean: `prettier --check`, `tsc --noEmit` (with `noUnusedLocals`/`noUnusedParameters` on), `node --test`, `vite build`.
- `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean.
- `cargo audit`: exit 0, 17 pre-existing warnings (unmaintained/unsound transitive dependencies inside the Tauri/webview tree), identical set to Phase 7's accepted baseline - no new advisories.
- `npm audit`: 0 vulnerabilities.
- Gitleaks: 0 leaks across 35 commits (an initial run against uncommitted `src-tauri/target/` build artifacts surfaced 5 false positives from a `muda` crate keyboard-shortcut string - re-run respecting `.gitignore`, which already excludes `target/`, came back clean; this is scan-methodology noise, not a repository finding).
- Trivy: 0 vulnerabilities across `package-lock.json` and `Cargo.lock`, 0 secrets.
- WCAG AA contrast verified (same relative-luminance-formula script as Phase 6) for every new/changed color pairing in both themes - gold-on-raised-surface, muted-text-on-raised-surface, and the new `.btn-link` default color against every background it appears on. All pass, several with wide margin.
- Live-verified via WebView2 CDP against real seeded data: sidebar navigation (mouse and keyboard, including a corrected Enter-key CDP dispatch sequence needed to reliably trigger native button activation), the Overview hero's `<details>` breakdown disclosure (keyboard and mouse), dialog Escape-closes-and-restores-focus from within its new Settings context (`RuleForm`, real mouse trigger), 1280x800 and 1024x640 layouts (no horizontal scroll, no nav items lost, Settings link reachable at both sizes), both themes.

## Acceptance criteria (owner's completion checklist)

1. Giant top nav gone - [x] replaced by the grouped sidebar.
2. Sidebar navigation in place - [x].
3. Primary navigation substantially reduced - [x] 13 -> 10 routed items, 5 moved into Settings/Overview.
4. Safe to Spend is the Overview hero - [x] dominant card, raised surface, gold headline.
5. Supporting information has clear visual hierarchy - [x] hero / Tier-2 (equal cards) / Tier-3 (quiet).
6. Bright-green sheet appearance is gone - [x] darker canvas, raised-surface depth, restrained gold.
7. Dark green + gold identity is evident - [x], live-verified in dark theme.
8. Secondary tools consolidated into Settings/contextual flows - [x] Pay Cycle, Safe to Spend reserve, Rules, Backup, Theme all moved.
9. Empty states are compact - [x] audited across every secondary page; Overview's new empty states use small actionable links instead of dead-end text.
10. 1280x800 works - [x] live-verified.
11. 1024x640 works - [x] live-verified, no horizontal scroll, no lost nav items.
12. Keyboard/accessibility behavior still works - [x] reverified: tab order, focus-visible, dialog focus-trap/Escape/focus-restore, disclosure keyboard toggle.
13. Existing finance functionality remains intact - [x] 174/174 Rust tests unchanged; every page still reads/writes through the same untouched API surface.
14. Full automated gate passes - [x].
15. No new security findings - [x].
16. Ponytail review passes - [x] net negative line count across modified files (-303 before counting new-file consolidation); one real duplication removed (`SCHEDULE_TYPE_LABELS`, previously defined separately in two files, now defined once and imported).
17. Working tree is clean, changes committed and pushed to Gitea - [x] (see commit).

## Test strategy

Same live-verification discipline as every prior phase: real `tauri dev` process with WebView2 remote debugging, driven by a small purpose-built CDP client (raw WebSocket, `Runtime.evaluate`/`Input.dispatchMouseEvent`/`Input.dispatchKeyEvent`/`Page.captureScreenshot`/`Emulation.setDeviceMetricsOverride`), never simulated. Sample data was created through the app's own `create_account`/`create_transaction`/`create_budget`/`create_savings_goal` commands, not fabricated in the database directly. WCAG contrast checked with the same relative-luminance-formula Node script as Phase 6, not eyeballed.

## Owner decisions

1. Review the 8 required screenshots (Overview at 1280x800 and 1024x640, Transactions, Budget, Settings, Bills, one Light-theme shot, one Dark-theme shot) and approve or request changes to the visual direction.
2. Once approved: authorize resuming Phase 7's packaging steps - rebuilding the installer against this UI, re-hashing it (the existing `8755efce...` hash is Phase 7 technical evidence only, not the final artifact), and only then the `v0.1.0` tag and any Gitea release publication.

## Hard-stop conditions

- Any change to accounting logic beyond the one verified-defect string fix - none arose.
- Rebuilding the installer, tagging, or publishing without the owner's visual approval - not done.
