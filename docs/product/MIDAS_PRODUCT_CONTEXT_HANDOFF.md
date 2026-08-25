# MIDAS — Product Context Handoff
**Audience:** Claude Code  
**Purpose:** Understand MIDAS before writing or changing code  
**Status:** Product context and design intent  
**Read this file completely before reading the build handoff.**

---

## 1. What MIDAS Is

MIDAS is a private, local-first personal finance desktop application.

The goal is not to make another spreadsheet with prettier charts and it is not to make the user become a full-time budget administrator. MIDAS should become the single place where the owner can open the app and understand, within roughly 30 seconds:

- Where do I stand financially?
- How much cash is actually available?
- What bills are coming before I am paid again?
- How am I doing in the current pay cycle?
- What have I been spending money on?
- Which transactions need my attention?
- What recurring charges have changed?
- Am I progressing toward my savings goals?
- What is my recent financial trend?

The original placeholder name was **Breezy Money**. That name is retired. The application is **MIDAS**, inspired by the “Midas touch” and the idea of turning messy financial data into something understandable and useful.

MIDAS is meant for one primary owner, not a finance department, household administration team, or enterprise.

---

## 2. Core Product Philosophy

MIDAS must be:

1. **Private by default**
   - Financial data stays under the user's control.
   - No advertising.
   - No data resale.
   - No unnecessary telemetry.
   - No cloud account required for the application to function.

2. **Local-first**
   - The desktop application must remain useful offline.
   - Local data is the source of truth.
   - A network connection must not be required for ordinary financial review.

3. **Statement-driven and manual-first**
   - Initial use starts from manual account creation and imported financial data.
   - CSV/manual entry are first-class workflows.
   - The broader design should support bank and credit-card statements, including digital statements and later scanned-document fallback.
   - **Do not build real-time bank synchronization into the initial release.**
   - Optional bank connectivity may come later as a separate capability.

4. **Explainable**
   - MIDAS should prefer deterministic calculations, mappings, and rules.
   - The user should be able to understand why MIDAS categorized something or calculated a number.
   - AI must never be required to perform basic accounting, budgeting, importing, categorization, or forecasting.

5. **User-overridable**
   - A user correction wins over an inferred result.
   - Corrections may become reusable rules when appropriate.
   - Uncertain data should go to **Needs Review**, not be silently guessed.

6. **Useful without constant maintenance**
   - Import, review exceptions, inspect the dashboard, make occasional adjustments.
   - Avoid workflows that require micromanaging every transaction.

7. **Read-only before transactional**
   - MIDAS initially understands and organizes finances.
   - It does **not** move money or pay bills in v1.

---

## 3. Relationship to ICARUS and SERANA

MIDAS is its own application and its own financial source of truth.

**SERANA** is the finance-planning agent in the wider ICARUS ecosystem. In the future, SERANA may be allowed to inspect MIDAS and propose changes or perform specifically approved actions.

That future integration must follow this model:

**MIDAS owns financial data → SERANA reads/proposes through controlled interfaces → owner approves consequential changes → MIDAS records what happened.**

MIDAS must **not** depend on SERANA, ICARUS, Claude Code, Codex, or The Forge at runtime.

Future agent access should be:
- narrow,
- auditable,
- permissioned,
- reversible where practical,
- and separated from the core ledger.

Do not build a large agent framework into MIDAS now. Leave clean boundaries for later integration.

---

## 4. The Mental Model

MIDAS should treat imported financial information as a pipeline:

**Source statement/file → import/extraction → staging and validation → normalized transactions → review exceptions → recurring/bill analysis → budgets/pay-cycle calculations → dashboard and reports**

Important implication:

**Never throw imported information directly into the canonical ledger without validation.**

The original source should remain traceable so that a questionable transaction can be connected back to the import that produced it.

---

## 5. Account Model

MIDAS should be capable of representing:

- Checking
- Savings
- Credit cards
- Loans
- Cash
- Investments
- Retirement
- Property or manually valued assets
- Other liabilities

Not every account type needs advanced automation in the first release.

An account can include, as applicable:

- Display name
- Institution
- Account type
- Current/manual balance
- Last imported/synchronized timestamp
- Whether it is included in net worth
- Notes
- Archived/active status

### Credit Cards Are Not Checking Accounts

Credit-card handling must be financially correct.

MIDAS must distinguish:
- purchases/charges,
- refunds,
- card payments,
- transfers,
- outstanding card balance,
- and available credit.

A credit-card payment from checking must not become new spending after the original purchases have already been counted.

**Credit availability is not cash and must never increase Safe to Spend.**

---

## 6. Transaction Model

A normalized transaction should be able to represent:

- Date
- Posted date when available
- Amount
- Direction/type
- Account
- Raw statement description
- Normalized merchant/payee
- Category
- Subcategory
- Tags
- Notes
- Pending/cleared state when known
- Transfer status
- Refund/reimbursement relationship where applicable
- Split transaction data
- Source/import reference
- Review status
- Confidence or explanation for inferred fields where useful

MIDAS should support:
- manual transaction entry,
- imported transactions,
- duplicate detection,
- bulk categorization/editing,
- transfers excluded from spending,
- refunds/reimbursements handled correctly,
- transaction splitting,
- and durable user corrections.

---

## 7. Import and Statement Workflow

The owner may initially supply roughly **three months of bank and credit-card history** so MIDAS can establish a baseline.

### First-class v1 import paths
- CSV import
- Manual entry

### Broader statement roadmap
- Digital PDF statement extraction
- Explicit scanned-PDF fallback

Do not sacrifice the quality of CSV/manual workflows in order to rush document OCR.

### Import workflow

1. Select account or create one.
2. Select source file.
3. MIDAS inspects columns/data.
4. User confirms or maps fields if necessary.
5. Data enters a **staging area**.
6. Validate:
   - required fields,
   - dates,
   - amounts,
   - account relationship,
   - probable duplicates,
   - malformed records.
7. Show an import summary.
8. User commits valid rows.
9. Ambiguous records go to **Needs Review**.
10. Preserve source/import metadata.

Imports should be repeatable and resilient. A user must not be terrified of accidentally importing the same file twice.

---

## 8. Merchant Normalization and Rules

Real statements often contain processor noise and inconsistent merchant names.

Example concept:

`SQ *COFFEE SHOP 123 MIAMI FL`
→
`Coffee Shop`

MIDAS should support deterministic normalization and categorization rules such as:

- exact match,
- pattern match,
- account-aware match,
- amount/date-aware rule where justified.

Preferred hierarchy:

1. Explicit user decision
2. Existing user rule
3. Deterministic built-in logic
4. Historical mapping
5. Optional future AI suggestion
6. Otherwise: **Needs Review**

Do not guess when confidence is poor.

---

## 9. Needs Review

**Needs Review** is a first-class inbox, not an error dump.

It should collect things such as:

- uncertain merchant normalization,
- uncategorized transactions,
- ambiguous transfers,
- suspected duplicates,
- malformed imports,
- recurring-charge ambiguity,
- unusual amount changes,
- possible refunds/reimbursements.

The owner should be able to resolve an item quickly and, when useful, turn the correction into a reusable rule.

The app should learn from explicit corrections without becoming opaque.

---

## 10. Recurring Payments and Bills

MIDAS should identify and track repeating obligations.

Possible schedules include:

- weekly,
- biweekly,
- monthly,
- quarterly,
- annual,
- custom recurring patterns.

Track:
- normalized merchant,
- expected amount or amount range,
- expected date/window,
- linked account,
- category,
- last occurrence,
- next expected occurrence,
- price change,
- potentially missed occurrence.

Recurring detection should result in **proposals**, not silent truth. The user can confirm or reject detected recurring items.

### Bill calendar

MIDAS should provide a calendar/list of upcoming known obligations so the owner can see what is due before the next payday and later in the month.

No bill payment in v1.

---

## 11. Safe to Spend

**Safe to Spend** is one of MIDAS's headline numbers.

It means: **the amount of real cash that can reasonably be spent before the next payday after protecting known near-term obligations and reserves.**

Initial implementation should be transparent and conservative.

Conceptually:

`eligible cash available`
`- known bills due before next payday`
`- protected/reserved spending commitments`
`- owner-defined required reserves`
`= Safe to Spend`

Rules:
- Do not include available credit as cash.
- Avoid double-counting transfers or card payments.
- Show the components behind the number.
- If the calculation lacks enough information, show that clearly instead of presenting false precision.
- The exact calculation must be documented in the product docs and covered by tests.

The user should be able to click the number and answer, “Why is this my Safe to Spend?”

---

## 12. Pay-Cycle View

MIDAS should not only think in calendar months.

The current pay-cycle view should answer:

- When was the most recent income/pay event?
- When is the expected next one?
- How much has been spent since the current cycle began?
- How does current spending compare with previous comparable cycles?
- Which bills must clear before the next pay event?
- What is Safe to Spend for the remainder of this cycle?

Pay-cycle detection can be inferred from recurring income but must remain editable.

---

## 13. Budgets

Budgets should be useful without forcing extreme envelope-budget administration.

Support:
- monthly category limits,
- planned vs. actual,
- fixed vs. flexible groups,
- optional rollover where implemented,
- thresholds/warnings,
- reusable budget templates where helpful,
- historical comparison.

MIDAS may **propose** a starting budget based on imported history, but the owner decides.

Budget visuals should be easy to understand at a glance.

---

## 14. Savings Goals

Goals can include:
- goal name,
- target amount,
- current amount,
- optional target date,
- contribution/reserve amount,
- progress,
- status.

Goals may later influence Safe to Spend when the owner explicitly chooses to reserve money for them.

Do not automatically lock money away without a visible rule.

---

## 15. Net Worth and Trends

MIDAS should be able to produce a simple net-worth picture from included assets and liabilities.

Trend/reporting can include:
- income,
- spending,
- net cash flow,
- category movement,
- account balances,
- net-worth movement,
- recurring-cost changes,
- budget performance.

Charts serve the decision. Do not add charts solely because finance dashboards usually have them.

---

## 16. Dashboard / Overview

The overview is the heart of MIDAS.

The user should be able to answer “Where do I stand?” quickly.

Priority cards/panels:

1. **Safe to Spend**
2. **Upcoming Bills**
3. **Current Pay Cycle**
4. **Spending**
5. **Needs Review**
6. **Savings Goals**

Supporting information can include:
- cash position,
- net worth,
- income/spending/net cash flow,
- recent activity,
- budget status,
- trend snapshot.

Do not turn the home screen into a wall of equal-weight cards. Financial hierarchy matters.

---

## 17. Main Navigation

Expected product areas:

- Overview
- Transactions
- Needs Review
- Statements / Imports
- Recurring / Bills
- Budget
- Calendar
- Accounts
- Goals
- Reports / Trends
- Rules
- Settings

Navigation can be consolidated where it improves usability. Do not preserve separate screens merely because this list exists.

---

## 18. Visual Identity

### Core visual direction

MIDAS uses:

- **deep money-green background**
- **gold as the signature accent**
- Swiss-inspired clarity
- Bento-style information grouping
- highly legible financial numbers
- restrained, selective glass effects
- strong hierarchy
- generous spacing
- polished desktop application behavior

This should feel like a high-end personal financial command center, not:
- a casino,
- a crypto exchange,
- an accounting ERP,
- a neon cyberpunk terminal,
- or a generic banking website.

### Initial palette direction

Use these as design starting tokens, not sacred values:

- `--midas-bg-deep`: `#071A12`
- `--midas-bg`: `#0B2418`
- `--midas-surface`: `#102E20`
- `--midas-surface-raised`: `#173A29`
- `--midas-gold`: `#D4AF37`
- `--midas-gold-soft`: `#BFA45A`
- `--midas-text`: `#F4F1E8`
- `--midas-text-muted`: `#AAB8AE`

Use semantic status colors independently from the brand palette. Do not make “everything positive” gold or green.

Contrast and readability outrank decoration.

### UI rules

- The green background is an intentional MIDAS identity element.
- Gold should guide attention, not cover every surface.
- Use glass only when it improves layering.
- **Do not use neumorphism for forms, navigation, alerts, tables, review states, or critical financial information.**
- Avoid tiny low-contrast labels.
- Financial values must be immediately readable.
- Dense transaction tables must remain efficient to scan.
- Keyboard accessibility and visible focus states are required.
- Dark appearance is the flagship MIDAS look.
- Preserve the shared application support for System / Light / Dark where inherited from The Forge unless the product-specific implementation demonstrates a better compliant solution.

---

## 19. Product Trust

MIDAS handles private financial information. Trust is part of the product.

Important behavior:
- clear import previews,
- no silent destructive actions,
- confirmations for consequential deletion,
- reliable local persistence,
- deterministic migrations,
- export/backup path,
- useful errors,
- auditability for important changes,
- no hidden network dependency,
- no secret uploads,
- no unnecessary permissions.

If an operation fails, data integrity matters more than pretending the operation succeeded.

---

## 20. What MIDAS Is Not

Do **not** turn MIDAS v1 into:

- a bank,
- a bill-payment platform,
- an investment trading platform,
- a tax-preparation system,
- a crypto dashboard,
- a cloud SaaS product,
- a multi-user household finance server,
- an ICARUS agent shell,
- an LLM-first categorization system,
- a live Plaid/SimpleFIN-style sync client,
- or an enterprise accounting package.

Future capability must not pollute the initial architecture with speculative abstractions.

---

## 21. Product Success Criteria

MIDAS is succeeding when the owner can:

1. Create the relevant accounts.
2. Import real transaction history without fear of duplicates or silent corruption.
3. Correct uncertain data quickly.
4. See recurring obligations.
5. Understand the current pay cycle.
6. See a trustworthy Safe-to-Spend number and its explanation.
7. Track budgets and savings goals.
8. Understand where money is going without maintaining a spreadsheet.
9. Back up/export the data.
10. Use the application privately without any external service.

The product should feel calm, trustworthy, fast, and intentional.

---

## 22. Final Instruction to the Builder

Do not begin by inventing architecture.

First understand the product above. Then inspect **The Forge** and the MIDAS build handoff. Reuse established standards and components before adding anything new.

When a requirement is ambiguous:
- prefer the smallest implementation that preserves the intended product behavior,
- record material assumptions,
- and do not silently introduce new product scope.

**MIDAS should be sophisticated in the experience and conservative in the architecture.**
