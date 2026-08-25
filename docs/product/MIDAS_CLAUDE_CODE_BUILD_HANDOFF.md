> **Owner correction (2026-08-25, recorded in ADR-0002):** Section 1 below states "The Forge is the current name of the former App Foundry." That collapses two distinct things. For this project: **The Forge** is the reusable application code/tooling/golden-path; **App Foundry** is the broader development methodology, standards, governance, gates, and workflow. MIDAS uses both and depends on neither at runtime. The rest of this document is preserved verbatim as originally handed off; this note is the correction, not an edit to the historical text below.

# MIDAS — Claude Code Build Handoff
**Audience:** Claude Code  
**Purpose:** Build MIDAS using the existing Forge workflow and standards  
**Prerequisite:** Read `MIDAS_PRODUCT_CONTEXT_HANDOFF.md` completely first  
**Target:** Windows desktop application, locally complete initial release candidate

---

## 0. Mission

Build **MIDAS**, the local-first personal finance desktop application defined in the product-context handoff.

Use the established **The Forge** golden path and existing project standards. Do not create a new development framework for MIDAS.

The objective is a polished, secure, independently runnable desktop application with a strong initial financial workflow, not a speculative “platform.”

---

## 1. Existing Environment and Source of Truth

### The Forge

**The Forge is the current name of the former App Foundry.**

Canonical local Forge source:

`the-forge`

Treat that directory as the **read-only reference/golden-path source** unless the owner explicitly asks you to modify The Forge itself.

MIDAS should be created as a separate project:

`C:\Dev\midas`

MIDAS must remain independently:
- buildable,
- testable,
- runnable,
- packageable,
- and releasable.

Do not create a runtime dependency on The Forge.

### Gitea

Use the owner's **private local Gitea** as source control and release source of truth.

- Project/repository name: `midas`
- Keep the repository private.
- Use `main` as the default branch unless the existing Forge workflow says otherwise.
- Derive authentication/remote information from the working local environment.
- **Never invent, print, request in plaintext, or commit credentials/tokens.**

Do not use GitHub as the canonical MIDAS repo unless explicitly instructed.

---

## 2. Preflight: Verify Access Before Building

Do this immediately so the build does not stall later.

Verify that you can:

- read `the-forge`
- create/write `C:\Dev\midas`
- run Git
- access the owner's local Gitea over the existing network configuration
- use existing Gitea credentials without exposing them
- run the installed Node/npm toolchain
- run Rust/Cargo
- run the Tauri toolchain
- install ordinary project dependencies from the package ecosystem when they are already justified by the Forge baseline
- execute PowerShell/shell commands required for build/test
- run the project's linters, formatters, tests, security scans, and packaging commands
- create a Windows installer/artifact
- write project documentation
- use Claude Code project plugins/hooks

If a capability is missing, identify the **specific permission or tool** required before proceeding into the part of the build that needs it. Do not ask for broad administrator access when narrow access is enough.

### Do not request up front unless actually needed

- unrestricted administrator privileges
- cloud credentials
- banking credentials
- external finance API keys
- public web hosting credentials
- production secrets

MIDAS v1 should not need those.

---

## 3. Use The Forge, Do Not Recreate It

Before scaffolding MIDAS:

1. Inspect `the-forge`.
2. Read its entry-point documentation.
3. Inspect its starter application.
4. Inspect its package manifests and lockfiles.
5. Inspect its theme, accessibility, testing, security, packaging, docs, and release conventions.
6. Identify reusable files/components/configuration.
7. Copy/adapt the golden path into MIDAS.
8. Do **not** edit the Forge source merely to make MIDAS easier to build.

Use actual Forge contents as authority when this handoff and the Forge disagree on a low-level implementation detail.

For product behavior and MIDAS-specific requirements, the MIDAS handoffs take precedence.

---

## 4. Required Engineering Baseline

Use the established desktop golden path:

- **Tauri 2** desktop shell
- **React**
- **TypeScript**
- **Vite**
- **Rust** for native/backend authority
- **SQLite** for durable financial application data
- a small existing config-store/file approach for simple preferences when appropriate
- **Playwright** for important end-to-end UI workflows where useful
- private **Gitea** for source and releases

Do not introduce:
- Electron,
- a local web server,
- a second database,
- a daemon,
- an orchestration layer,
- a cloud service,
- or a new frontend framework

unless there is a concrete requirement that the current stack cannot reasonably satisfy.

### Dependency rule

Before adding a dependency:
1. Check whether The Forge already includes an appropriate solution.
2. Check whether the platform/standard library can do it.
3. Check whether the existing project can do it with a small amount of code.
4. Add a dependency only when it materially reduces risk or complexity.

Document meaningful dependency additions.

---

## 5. Ponytail Is Required

Use the **Ponytail Claude Code plugin** as an always-on simplification constraint.

If not already installed in this Claude Code environment, use the official plugin flow:

1. `/plugin marketplace add DietrichGebert/ponytail`
2. `/plugin install ponytail@ponytail`

Use **full** intensity for the build unless there is a specific reason to change it.

Ponytail is not permission to remove:
- validation,
- error handling,
- security,
- accessibility,
- durable data handling,
- tests required for important logic,
- or required product behavior.

### Required Ponytail passes

After each meaningful feature slice:
- run the normal correctness review,
- then run `/ponytail-review` on the diff.

Before the release candidate:
- run `/ponytail-audit`,
- remove unjustified complexity,
- rerun regression/security checks after simplification.

The objective is **less unnecessary code**, not less product quality.

---

## 6. Repository Documentation

Keep project truth in the repository.

At minimum create/adapt:

- `README.md`
- concise `AGENTS.md` as the agent-agnostic entry point
- thin `CLAUDE.md` that points Claude Code to the same repository truth rather than duplicating it
- product/spec documentation
- architecture documentation
- security/privacy notes
- testing instructions
- release checklist
- phase/status documentation
- lightweight ADRs only for meaningful product/dependency/architecture decisions

Use the existing Forge structure/names if they differ.

Do not produce a large bureaucracy of documents. This is a sole-builder project.

---

## 7. Architecture Rules

Keep clean boundaries between:

1. **Presentation**
   - React UI
   - visual state
   - user interaction

2. **Application/domain logic**
   - financial calculations
   - normalization
   - recurring detection
   - import validation
   - budgets
   - pay-cycle logic
   - Safe-to-Spend logic

3. **Persistence**
   - SQLite repositories/migrations
   - source/import metadata
   - settings appropriate for durable storage

4. **Native authority**
   - narrow Tauri/Rust commands
   - file access
   - privileged OS actions
   - database/file operations where the Forge architecture expects native authority

Do not put financial truth inside UI components.

Do not create “services” and interfaces merely because enterprise architecture often has them. Add boundaries when they provide real value.

---

## 8. Data Integrity Requirements

Financial data requires stronger persistence discipline than a disposable utility app.

Required:
- deterministic schema migrations
- foreign-key/data-integrity constraints where appropriate
- import transactions/rollback behavior
- duplicate handling
- safe deletion semantics
- export/backup capability
- source/import traceability
- tests around important calculations
- clear failure states

Never silently drop malformed financial records.

---

## 9. Initial Release Scope

Build a coherent locally complete v0.1.x product around the following flow:

**Create accounts → import/manual transactions → validate/review → understand recurring obligations → budget/pay-cycle calculations → dashboard → backup/export**

### Required functional areas

#### A. Accounts
Implement:
- account creation/edit/archive
- account type
- institution/name
- balances
- net-worth inclusion
- notes
- active/archive state

Account types must accommodate:
- checking
- savings
- credit card
- loan
- cash
- investment
- retirement
- property/manual asset
- other liability

Advanced investment tracking is out of scope.

#### B. Transactions
Implement:
- transaction list/search/filter
- manual entry
- category/subcategory
- notes/tags if supported cleanly
- transfer handling
- refund/reimbursement handling
- split transactions where feasible without destabilizing the core
- bulk categorization/editing
- source/import linkage

#### C. CSV Import
This is a flagship workflow.

Implement:
- file selection
- preview
- column mapping when needed
- validation/staging
- account assignment
- date/amount parsing
- duplicate detection
- import summary
- commit/rollback behavior
- source/import record
- errors and Needs Review routing

Do not silently import ambiguous data.

#### D. Needs Review
Build a dedicated review inbox for:
- uncertain category
- uncertain merchant
- suspected duplicate
- malformed import
- ambiguous transfer
- recurring ambiguity
- unusual recurring amount
- other deterministic confidence failures

Resolution should be fast.

When appropriate, allow the resolution to create/update a reusable deterministic rule.

#### E. Merchant Normalization and Rules
Implement a simple deterministic rule system:
- exact match
- pattern match
- account-aware matching where justified
- reusable user corrections

Do not build an AI inference service.

#### F. Recurring / Bills
Implement:
- recurring candidate detection from transaction history
- user confirm/reject
- expected amount or range
- cadence
- next expected date/window
- price-change indication
- possible missed occurrence
- upcoming bills list/calendar

No bill payment.

#### G. Budget
Implement:
- monthly category budgets
- planned vs actual
- fixed/flexible distinction where useful
- warning thresholds
- historical comparison if the data is available

Budget recommendations may be simple and deterministic from history.

#### H. Pay Cycle
Implement:
- editable income/pay schedule
- detection/proposal from recurring income where reliable
- current cycle start
- expected next payday
- spending since cycle start
- bills before next payday
- comparison with prior comparable cycles

#### I. Safe to Spend
Implement as an explainable calculation.

Initial conceptual model:

`eligible cash`
`- confirmed obligations before next payday`
`- protected reserves/committed spending`
`- owner-required reserve`
`= Safe to Spend`

Requirements:
- exclude credit availability
- avoid card-payment/transfer double counting
- show calculation components
- handle missing information explicitly
- unit test edge cases
- keep the formula isolated from presentation
- document assumptions

Do not make the calculation look precise if the inputs are incomplete.

#### J. Savings Goals
Implement:
- goal name
- target
- current progress
- optional target date
- optional reserved contribution
- status/progress display

#### K. Overview
The dashboard must prioritize:

1. Safe to Spend
2. Upcoming Bills
3. Current Pay Cycle
4. Spending
5. Needs Review
6. Savings Goals

Supporting information can include:
- cash position
- net worth
- income/spending/net cash flow
- budget status
- recent activity
- trend snapshot

#### L. Backup / Export
Provide a simple, understandable path to:
- export user data in a portable form
- back up the local MIDAS data
- restore according to the project's documented method

Do not make the user depend on MIDAS forever to access their own financial records.

---

## 10. Explicitly Deferred Scope

Do not hold v0.1 hostage to these:

- live bank synchronization
- Plaid/SimpleFIN-style integration
- bill payment
- money movement
- investment trading
- tax filing
- cloud sync
- multi-user accounts
- mobile companion app
- SERANA/ICARUS runtime integration
- LLM categorization
- OCR-heavy scanned statement support
- elaborate plugin architecture
- financial “marketplace”
- enterprise roles/permissions

### PDF statements

Preserve the architecture for later statement ingestion.

If digital PDF extraction can be added cleanly after the core CSV/manual workflow is stable, it can be a later v0.x slice. Scanned-PDF/OCR fallback should not compromise the first release.

---

## 11. UI and Visual System

### MIDAS identity

Use the **green + gold** product identity defined in the product handoff.

Primary visual feel:
- deep money-green application background
- gold signature accents
- light warm text
- clean Swiss-style typography/layout
- Bento information grouping
- selective glass only
- excellent number readability
- restrained motion

### Starting theme tokens

Use/adapt:

```css
--midas-bg-deep: #071A12;
--midas-bg: #0B2418;
--midas-surface: #102E20;
--midas-surface-raised: #173A29;
--midas-gold: #D4AF37;
--midas-gold-soft: #BFA45A;
--midas-text: #F4F1E8;
--midas-text-muted: #AAB8AE;
```

Treat these as the initial design system, not a reason to hard-code colors throughout components.

### Reuse the Forge UI foundation

Inspect and reuse:
- typography
- spacing scale
- responsive layout primitives
- buttons
- form controls
- dialogs
- navigation
- tables
- theme system
- icons
- focus behavior
- motion conventions
- accessibility utilities

Do not add a new UI/component framework merely because you prefer it.

If The Forge already uses a library, use that library consistently.

### Important financial UX

- Currency and balances need clear hierarchy and tabular alignment where useful.
- Negative/positive values must not rely on color alone.
- Transactions need efficient keyboard/mouse review.
- Error, warning, and Needs Review states must be unmistakable.
- Do not bury the Safe-to-Spend explanation.
- Do not overuse gold.
- Do not turn the app into a neon dashboard.
- Avoid neumorphism for functional/critical UI.
- Avoid fake glass everywhere.

### Themes

Preserve the Forge System / Light / Dark capability.

MIDAS dark mode is the flagship design. Light/System variants should remain usable and accessible rather than being afterthoughts.

---

## 12. Security and Privacy Requirements

Treat financial data as sensitive local information.

Required:
- least-privilege Tauri permissions/capabilities
- narrow CSP
- no unnecessary outbound network calls
- no analytics/telemetry unless explicitly approved
- no secrets in repo/logs
- sanitized production logging
- safe file handling
- defensive import parsing
- safe path handling
- dependency review
- deterministic migrations
- secure release process
- no hidden background service

If a new permission is needed, document exactly why.

If MIDAS has no reason to access the network during ordinary local operation, do not give it broad network authority.

---

## 13. Testing and Quality Gates

Mirror the Forge/SIREN quality discipline.

Run the applicable project commands for:

- frontend unit/integration tests
- Rust tests
- TypeScript type-checking
- formatting
- linting
- Vite production build
- `cargo check`
- `cargo test`
- `cargo clippy`
- dependency audit
- Gitleaks
- Trivy
- important Playwright workflows
- Tauri packaging

### Financial logic tests are mandatory

Cover at minimum:

- transfer exclusion
- credit-card payment double-counting prevention
- refunds
- duplicate imports
- Safe-to-Spend calculations
- bills crossing payday boundaries
- missing payday information
- recurring amount changes
- recurring cadence
- budget planned vs actual
- account inclusion/exclusion from net worth
- import rollback on failure

Use realistic fixture data, but **never real personal bank statements or credentials in the repository.**

---

## 14. Build Order

Use small, testable vertical slices.

Recommended order:

### Phase 0 — Inspect and scaffold
- Read both handoffs.
- Inspect The Forge.
- Record current toolchain.
- Scaffold `C:\Dev\midas`.
- Initialize local Git.
- Create repo docs.
- Confirm app launches.
- Establish MIDAS theme shell.

### Phase 1 — Persistence and accounts
- SQLite schema/migrations.
- Account model.
- App settings.
- Basic data access.
- Account UI.
- Tests.

### Phase 2 — Transactions and CSV pipeline
- Transaction model.
- Manual transaction workflow.
- CSV import staging/mapping/validation.
- Duplicate detection.
- Import history/source references.
- Needs Review foundation.

### Phase 3 — Rules and recurring
- Merchant normalization.
- Categories.
- Reusable corrections/rules.
- Recurring candidate detection.
- Recurring confirmation.
- Bills/upcoming view.

### Phase 4 — Core financial intelligence
- Budgets.
- Pay cycle.
- Safe to Spend.
- Credit-card accounting safeguards.
- Savings goals.
- Net worth.

### Phase 5 — Overview and reports
- Final dashboard hierarchy.
- Basic trends/reporting.
- Drilldowns from cards.
- Empty/loading/error states.

### Phase 6 — Hardening
- Backup/export/restore workflow.
- Accessibility.
- responsive desktop sizing.
- performance.
- error handling.
- migration tests.
- security review.
- Ponytail audit.

### Phase 7 — Release candidate
- full regression
- clean security scans
- production packaging
- Windows installer
- docs
- versioning
- artifact hash
- release checklist

Do not publish a final Gitea release or mark the project v1.0.0 without owner approval.

---

## 15. Autonomy and Approval Rules

The owner has authorized Claude Code to build MIDAS within this handoff.

You may proceed without interrupting for routine implementation choices that:
- stay inside this scope,
- use the existing Forge stack,
- are reversible,
- do not expose data,
- and do not create material new dependencies or permissions.

### Stop and request owner approval before:

- changing the product scope materially
- adding a new major framework
- adding cloud/network services
- adding bank synchronization
- adding a major dependency that changes the architecture
- granting broad OS permissions
- deleting or modifying unrelated projects
- modifying `the-forge`
- using real financial credentials/data
- destructive migration/reset of existing user data
- publishing a release
- making the repo public
- changing privacy/security posture

Do not ask approval for formatting, internal filenames, ordinary component decomposition, or similarly reversible implementation details.

---

## 16. Git Discipline

Use small, meaningful commits at verified checkpoints.

A commit should normally represent a coherent tested slice.

Do not:
- commit broken generated clutter,
- commit secrets,
- commit personal financial data,
- rewrite history unnecessarily,
- or touch unrelated repositories.

Before major refactors, ensure the current state is committed and recoverable.

---

## 17. UX Validation

Do not assume “it compiles” means the UI is good.

Validate major screens at realistic Windows desktop sizes.

Check:
- dashboard hierarchy
- transaction density
- CSV import flow
- Needs Review flow
- account setup
- budget editing
- bill/calendar readability
- Safe-to-Spend explanation
- keyboard navigation
- focus states
- empty states
- error states
- System/Light/Dark themes

Fix visual regressions before release.

---

## 18. Completion Definition

The build is not complete because a scaffold exists.

A MIDAS initial release candidate is complete when:

- app installs and launches as a desktop application
- no Forge/Claude/Codex dependency is required at runtime
- local persistence works
- schema migrations work
- accounts work
- manual transactions work
- CSV import/staging/validation works
- duplicate protection works
- Needs Review works
- recurring/bills work
- budgets work
- pay-cycle view works
- Safe to Spend works and is explainable
- credit-card payment accounting avoids double counting
- savings goals work
- overview is polished and useful
- backup/export path works
- accessibility baseline passes
- test suite passes
- build/lint/type checks pass
- security scans have no unresolved high/critical findings
- Ponytail review/audit has been completed
- Windows packaging succeeds
- documentation matches reality
- release artifact hash is produced

Do not paper over failing checks. Report remaining defects explicitly.

---

## 19. Final Build Report Required

At the end of the execution block, produce a concise handoff containing:

### Status
PASS / PARTIAL / BLOCKED

### What Was Built
Feature list.

### Architecture
Major implementation choices and any ADRs.

### Verification
Exact checks run and results.

### Security
Gitleaks/Trivy/audit findings and resolution status.

### Ponytail
What simplification review removed or changed.

### Git
Repo path, branch, commit/tag.

### Artifact
Installer path/version and SHA-256 if produced.

### Known Issues
Anything incomplete or uncertain.

### Owner Decisions Needed
Only decisions that genuinely require the owner.

### Next Recommended Slice
The smallest sensible next step.

---

## 20. First Actions

Start here:

1. Read `MIDAS_PRODUCT_CONTEXT_HANDOFF.md`.
2. Read this entire build handoff.
3. Inspect `the-forge`.
4. Verify the preflight access list.
5. Confirm Ponytail is installed and active.
6. Create `C:\Dev\midas`.
7. Reuse the Forge golden path.
8. Initialize the MIDAS repository and project docs.
9. Get the themed application shell launching.
10. Begin Phase 1.

**Do not start by designing a new framework. Build MIDAS.**
