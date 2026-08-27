# MIDAS

MIDAS is a private, local-first personal finance desktop application built with Tauri 2, React, TypeScript, and Rust. It is the single place to answer "where do I stand financially?" in about 30 seconds — Safe to Spend, upcoming bills, the current pay cycle, spending, Needs Review, and savings goals.

No cloud account, no bank sync, no telemetry. Local data is the source of truth.

## Your data

- **Location**: a single SQLite file at your OS's per-app data directory - on Windows, `%APPDATA%\com.breezy.midas\midas.sqlite3` (`Connection::open` in `src-tauri/src/db.rs`, path from Tauri's `app_data_dir()`). Nothing else - no cloud copy, no second database, no cache that also holds financial data.
- **Backup**: the Backup tab downloads a complete, consistent snapshot of that file (SQLite's own `VACUUM INTO`, which refuses to overwrite an existing file) as a timestamped `.sqlite3` download. Keep it anywhere you like - a synced folder, a USB drive, another computer's Downloads folder. You are never required to keep MIDAS open or installed to hold onto your own data.
- **Restore**: choose a backup file on the same tab. MIDAS opens it as its own, separate, temporary connection first and shows you what's in it (schema version, account and transaction counts) before anything changes. Only on your explicit confirmation does it become the live database - and your previous live database is renamed aside with a timestamp (`midas.sqlite3.before-restore-<timestamp>.bak`, next to the real one), never deleted, so a restore is itself always recoverable.
- **Export**: the same tab exports every transaction as a plain CSV (date, account, merchant, category, type, dollar amount, notes) - readable in Excel or Google Sheets with or without MIDAS installed. This is a portability export, not a backup: it doesn't include budgets, goals, rules, or enough structure to restore from - use Backup for that.
- **Security and privacy**: MIDAS makes zero outbound network calls of any kind (the production Content-Security-Policy has no external `connect-src`) and requests zero Tauri plugin capabilities (`src-tauri/capabilities/default.json`) - every backend operation, including import/export/backup/restore, moves data through the app's own narrow, typed commands as plain strings, never a raw filesystem or dialog API. There is no logging anywhere in the app (no `println!`, no `console.log`) that could write financial data to disk outside the database itself.
- **Known limitations**: backups are manual, not scheduled or automatic - MIDAS doesn't run in the background. The database file itself is not encrypted at rest (relies on your OS-level disk/account security). CSV export is one-way - it's for reading elsewhere, not for re-importing back into MIDAS (the CSV importer expects arbitrary bank-export formats, not MIDAS's own export shape). There is no multi-device sync; each installation has its own independent database unless you manually back up and restore between them. Windows is the only platform actually built and tested so far. Reinstalling silently (`/S`) directly over an existing installation hangs on a confirmation dialog silent mode doesn't suppress - a normal interactive install, or installing after a full uninstall, both work correctly; see `docs/release/0.1.0.md` for detail.

## Installing

Download `MIDAS_<version>_x64-setup.exe` and its matching `.sha256` file from the release. Verify the hash before running it:

```powershell
Get-FileHash .\MIDAS_<version>_x64-setup.exe -Algorithm SHA256
```

Compare the output against the `.sha256` file. Run the installer - it installs per-user to `%LOCALAPPDATA%\MIDAS` (no administrator rights needed) and adds a Start Menu shortcut. Uninstall from _Settings → Apps_ (or the Start Menu shortcut's right-click menu) like any other Windows app - **uninstalling removes the application files only, never your data** (`%APPDATA%\com.breezy.midas\midas.sqlite3` is untouched; back it up first anyway if you're about to reformat the whole machine, not just uninstall one app).

## Start here

1. Read [PROJECT_STATE.md](PROJECT_STATE.md) for current phase status.
2. Read [docs/product/MIDAS_PRODUCT_CONTEXT_HANDOFF.md](docs/product/MIDAS_PRODUCT_CONTEXT_HANDOFF.md) for product intent.
3. Read [docs/product/MIDAS_CLAUDE_CODE_BUILD_HANDOFF.md](docs/product/MIDAS_CLAUDE_CODE_BUILD_HANDOFF.md) for the build plan.
4. Follow [AGENTS.md](AGENTS.md) and the standards it links.

## Development

Requirements: Node.js/npm, Rust/Cargo, and the Windows MSVC build tools / Tauri 2 Windows prerequisites (WebView2).

```powershell
npm.cmd install
npm.cmd run check
cargo fmt --manifest-path src-tauri\Cargo.toml -- --check
cargo check --locked --manifest-path src-tauri\Cargo.toml
```

Run the desktop app with `npm.cmd run tauri dev`. Build the installer only inside an approved release phase with `npm.cmd run tauri build` (produces `src-tauri\target\release\bundle\nsis\MIDAS_<version>_x64-setup.exe`; hash it with `scripts\hash-artifacts.ps1`). Run the complete gate with `powershell -ExecutionPolicy Bypass -File scripts\validate.ps1` (frontend check, Rust fmt/check/clippy/test/audit). Security scans run separately each phase and aren't part of that script: `gitleaks detect`/`gitleaks protect`, `trivy fs`, and `npm audit`.

## Repository map

- `constitution/`: non-negotiable MIDAS rules (adapted from App Foundry governance conventions).
- `standards/`: security, testing, accessibility, dependencies, and release requirements.
- `templates/`: reusable phase, ADR, handoff, QA, owner-gate, and release records.
- `gates/`: repeatable lifecycle checklists.
- `docs/product/`: the canonical MIDAS product and build handoffs.
- `docs/architecture/`: architecture notes as they are established.
- `docs/decisions/`: ADRs for durable technical decisions.
- `docs/release/`: filled-in release checklists, one per version.
- `specs/`: phase specs and evidence.
- `src/`, `src-tauri/`: the application.

## Origin

MIDAS reuses `the-forge`'s `tauri-react-windows` golden path as a read-only reference (Tauri 2 + React + TypeScript + Vite, System/Light/Dark theming, narrow production CSP, no exposed Tauri commands by default, NSIS packaging, deterministic checks). MIDAS also follows the App Foundry development methodology (the standards/gates/governance discipline behind `constitution/`, `standards/`, and `gates/`) without depending on it at runtime either. The Forge is the code/tooling side of this; App Foundry is the methodology side — MIDAS does not depend on The Forge, App Foundry, SERANA, ICARUS, Claude Code, or Codex at runtime.

`origin` is configured to the private Gitea remote at `[private-git-server]`. Pushing has worked since Phase 2 (see ADR-0002 for the infrastructure issue that blocked it early on, and how it was resolved) — `origin/main` is kept in sync with local `HEAD` at the end of every phase; see `PROJECT_STATE.md` for the current commit.
