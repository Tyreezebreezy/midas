# MIDAS v1 architecture

Status: Phase 0 — foundation only. This document grows with each phase rather than being written speculatively ahead of the code.

## Layers

1. **Presentation** — React UI in `src/`. Visual state and user interaction only; no financial calculation lives here.
2. **Application/domain logic** — financial calculations, normalization, recurring detection, import validation, budgets, pay-cycle logic, Safe-to-Spend logic. Lives in Rust (`src-tauri/src/`) so a single implementation is authoritative and testable independent of the UI.
3. **Persistence** — SQLite via a Rust repository layer, with deterministic migrations. Introduced in Phase 1.
4. **Native authority** — narrow Tauri commands for file access, dialogs, and database operations. No command is exposed until a concrete UI workflow needs it, and each new capability is documented in `docs/decisions/`.

## Phase 0 state

- Tauri 2 + React 19 + TypeScript + Vite shell, adapted from The Forge's `tauri-react-windows` starter.
- System/Light/Dark theme plumbing with the MIDAS green/gold palette (`src/App.css`).
- No Tauri commands or plugin permissions are exposed yet (`src-tauri/capabilities/default.json` grants none).
- No persistence yet. No network authority.

## Planned data flow (from the product handoff)

`Source statement/file -> import/extraction -> staging and validation -> normalized transactions -> review exceptions -> recurring/bill analysis -> budgets/pay-cycle calculations -> dashboard and reports`

Imported data is never written directly to the canonical ledger without passing through staging/validation. Source/import metadata stays traceable.

## Open decisions carried into Phase 1

- Exact SQLite schema and migration mechanism (see `docs/decisions/` once an ADR is recorded).
- Which Tauri capabilities (fs scope, dialog) Phase 1 needs, scoped to the exact commands that require them.

Semantic status colors (positive/negative/warning) and a raised-surface token are deferred, not forgotten: add each one in the phase that first renders a signed amount or a card, so it can be picked against real content instead of guessed in the abstract.
