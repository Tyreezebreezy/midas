# MIDAS v1 architecture

Status: Phase 1 in progress. This document grows with each phase rather than being written speculatively ahead of the code.

## Layers

1. **Presentation** — React UI in `src/`. Visual state and user interaction only; no financial calculation lives here.
2. **Application/domain logic** — financial calculations, normalization, recurring detection, import validation, budgets, pay-cycle logic, Safe-to-Spend logic. Lives in Rust (`src-tauri/src/`) so a single implementation is authoritative and testable independent of the UI.
3. **Persistence** — SQLite via a Rust repository layer (`src-tauri/src/db.rs`), with deterministic migrations tracked via `PRAGMA user_version` (`src-tauri/migrations/`). See ADR-0003.
4. **Native authority** — narrow Tauri commands for file access, dialogs, and database operations. No command is exposed until a concrete UI workflow needs it, and each new capability is documented in `docs/decisions/`.

## Phase 0 state

- Tauri 2 + React 19 + TypeScript + Vite shell, adapted from The Forge's `tauri-react-windows` starter.
- System/Light/Dark theme plumbing with the MIDAS green/gold palette (`src/App.css`).

## Phase 1 state

- SQLite persistence via `rusqlite` (bundled), one mutex-guarded connection managed as Tauri state (`db::DbState`) — see ADR-0003 for why not `sqlx`/async.
- `accounts` and `settings` tables (`src-tauri/migrations/0001_initial.sql`). Money is stored as integer cents (`balance_cents`), never floats.
- Account domain logic + 6 Tauri commands in `src-tauri/src/accounts.rs` and `src-tauri/src/settings.rs`: `list_accounts`, `create_account`, `update_account`, `set_account_archived`, `get_setting`, `set_setting`. This is still the entire native authority surface — no fs/dialog/network plugin permissions have been added (`src-tauri/capabilities/default.json` still grants zero plugin permissions; these are plain app commands, not plugin-gated).
- Account UI (`src/AccountsPage.tsx`, `src/AccountForm.tsx`) is now the app's main content, replacing the Phase 0 placeholder.
- Net worth _display_ only (an "Included"/"Excluded" column reflecting the stored flag); net worth _calculation_ (summing accounts, subtracting liabilities) is not built yet - that's Phase 4.

## Planned data flow (from the product handoff)

`Source statement/file -> import/extraction -> staging and validation -> normalized transactions -> review exceptions -> recurring/bill analysis -> budgets/pay-cycle calculations -> dashboard and reports`

Imported data is never written directly to the canonical ledger without passing through staging/validation. Source/import metadata stays traceable.

## Open decisions carried into Phase 2

- CSV import will need a real file-picker: the first actual `dialog`/`fs` capability grant, scoped narrowly and documented with its own ADR when it lands.
- Transaction volume may need pagination/virtualization in the accounts-adjacent transaction list; not a concern yet at account-list scale.

`--midas-negative` was added in Phase 1 for liability balances (`.amount-owed` in `App.css`) - the first phase that actually renders a signed amount, per the Phase 0 deferral note this replaces. `--midas-positive`, `--midas-warning`, `--midas-gold-soft`, and a raised-surface token are still deferred to whichever phase first needs them (recurring/budget status, card surfaces).
