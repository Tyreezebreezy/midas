# Phase 001: Persistence and accounts

- Status: COMPLETE
- Owner gate: APPROVED — explicitly directed by the owner after Phase 0 acceptance ("Proceed to Phase 1... Continue autonomously through Phase 1").

## Goal

Durable local SQLite persistence, a full Account domain model covering all nine required account types, account CRUD with archive/unarchive, basic key-value application settings storage, and a polished Account UI in the MIDAS green/gold design system — with a narrow, documented native/Tauri authority surface.

## Scope

- SQLite via `rusqlite` (bundled), one mutex-guarded connection as Tauri-managed state (ADR-0003).
- Deterministic schema migrations via `PRAGMA user_version` (`src-tauri/migrations/0001_initial.sql`).
- `accounts` table: name, institution, account_type (9-way CHECK constraint), balance_cents (integer, never float), include_in_net_worth, notes, is_archived, last_synced_at, created_at, updated_at.
- `settings` key-value table + `get_setting`/`set_setting` commands (storage mechanism only; no consuming UI yet - nothing to configure until later phases).
- Account domain logic + validation in Rust (`src-tauri/src/accounts.rs`), independent of Tauri/UI and unit-tested.
- 6 Tauri commands: `list_accounts`, `create_account`, `update_account`, `set_account_archived`, `get_setting`, `set_setting`.
- Account UI (`src/AccountsPage.tsx`, `src/AccountForm.tsx`) replacing the Phase 0 placeholder as the app's main content: list/table, create/edit dialog, archive/unarchive, show-archived toggle.

## Explicit non-goals

- Advanced investment tracking (per owner instruction).
- Net worth _calculation_ (summing/subtracting across accounts) - Phase 4. Phase 1 only stores and displays the per-account inclusion flag.
- Account deletion (hard delete). The product model uses archive/unarchive; delete was never requested for accounts.
- CSV import, transactions, budgets, recurring detection, Safe to Spend, pay cycle, goals - later phases.
- Any file dialog / fs plugin capability (no import in this phase, so no reason to request that authority yet).

## Verified current state

- Builds on Phase 0 commit `b1f99cd` (owner corrections applied: `com.breezy.midas` identifier, Forge/App Foundry terminology, `cargo-audit` wired in).
- New Rust dependencies: `rusqlite 0.40.2` (bundled), `serde 1.0.229` (derive) - both added via `cargo add`, not hand-pinned.
- New npm dependency: `@tauri-apps/api ^2.11.1` (official Tauri JS bindings, required for any IPC call at all).
- `src-tauri/capabilities/default.json` still grants zero Tauri plugin permissions. The 6 new commands are plain app-defined commands (`invoke_handler(generate_handler![...])`), not plugin-gated - confirmed empirically, not just asserted (see IPC verification below).

## Constraints and architecture boundaries

- Domain logic (`accounts.rs`, `settings.rs`) takes `&Connection` and returns plain Rust types - testable with `Connection::open_in_memory()`, no Tauri runtime needed. Tauri `#[tauri::command]` wrappers are thin (lock the mutex, call the domain function, map the error to `String`).
- Money is `balance_cents: i64` throughout - never a float, anywhere, per the constitution's financial-correctness principle.
- Liability accounts (credit_card, loan, other_liability) store balance as a positive "amount owed"; net worth _math_ that flips the sign for liabilities is deliberately deferred to Phase 4 (see `docs/architecture/MIDAS_V1.md`), not implemented speculatively now.

## Security and privacy considerations

- No new network authority. No new fs/dialog plugin permissions.
- Database lives at the OS per-app data directory (`app.path().app_data_dir()`, resolves to `%APPDATA%\com.breezy.midas\midas.sqlite3` on Windows) - not under source control, already covered by `.gitignore`'s `*.sqlite*` pattern from Phase 0.
- All test/verification data used obviously-fake fixture values (e.g. "CDP Test Visa"); no real financial data touched this repository or this machine's dev database at any point. The dev database was deleted after verification.

## Acceptance criteria

- [x] SQLite persists across restarts (proven: dev DB file exists at the app data dir between separate `tauri dev` launches until deliberately deleted).
- [x] Migrations are deterministic and idempotent (`db::tests::migrate_is_idempotent`, `db::tests::migrate_creates_expected_tables`).
- [x] Account CRUD works for all 9 required types (`accounts::tests::all_nine_account_types_round_trip`, plus live-verified for `checking`, `credit_card`, and `savings` via real IPC calls - see Verification).
- [x] Archive/unarchive works and default listing excludes archived accounts (`accounts::tests::archive_then_unarchive_round_trips`, `list_excludes_archived_by_default_and_includes_when_asked`; live-verified via real IPC).
- [x] Net worth inclusion flag persists (`accounts::tests::net_worth_inclusion_flag_persists`; live-verified: flipped to `false` via a real update call, confirmed both in the API response and in the rendered "Excluded" table cell).
- [x] Basic settings storage works (`settings::tests::*`; live-verified round trip via real IPC: `set_setting('cdp_test','hello')` then `get_setting('cdp_test')` returned `"hello"`).
- [x] Narrow native/Tauri authority: exactly 6 app-defined commands, 0 plugin permissions.
- [x] Tests for migrations and Account behavior: 14 Rust unit tests, all passing.
- [x] Polished Account UI consistent with the MIDAS green/gold design system: verified rendered (not just compiled) - see Verification.

## Test strategy

1. Rust unit tests (`cargo test`, in-memory SQLite) for migrations, all 9 account types, CRUD, validation, archive/unarchive, net-worth flag, settings round-trip, not-found handling.
2. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`.
3. `cargo audit` (advisory-only gate; see Security below for the one pre-existing transitive finding).
4. Real end-to-end IPC verification against the actual running `tauri dev` app, driven over the WebView2 remote-debugging CDP port rather than mocked: direct `invoke()` calls for create/list/update/archive/unarchive/settings/validation-rejection, then a real DOM click-and-fill-and-submit pass through the actual "Add account" button and dialog. This specifically exercises the Rust-snake_case-to-JS-camelCase argument mapping that unit tests on either side alone cannot catch.

## Owner decisions

None required for this phase - explicitly pre-authorized to proceed autonomously.

## Hard-stop conditions

- Any need to add a Tauri plugin (fs, dialog, shell) - none arose in Phase 1; will arise in Phase 2 for CSV import and requires its own ADR + capability grant.
- Any need for async DB architecture - none arose; single sync mutex-guarded connection was sufficient (ADR-0003).
