# Phase 000: Inspect and scaffold

- Status: APPROVED
- Owner gate: APPROVED — routine, reversible, in-scope per the build handoff's autonomy rules (section 15): uses the existing Forge stack, stays inside authorized scope, does not expose data, adds no material new dependency or permission.

## Goal

Stand up an independently runnable MIDAS shell at `C:\Dev\midas`, reusing The Forge's `tauri-react-windows` golden path, with the MIDAS visual identity, repository documentation, and local Git history in place — ready for Phase 1 (persistence and accounts).

## Scope

- Directory scaffold copied/adapted from the Forge starter (Tauri 2 + React 19 + TypeScript + Vite).
- MIDAS green/gold theme tokens with System/Light/Dark support.
- Repository documentation: README, AGENTS.md, CLAUDE.md, PROJECT_STATE.md, constitution, standards, gates, templates, docs/product (handoff copies), docs/architecture, docs/decisions (ADR-0001).
- `scripts/validate.ps1` and `scripts/hash-artifacts.ps1` adapted for MIDAS paths.
- Local Git repository initialized with an initial commit.
- Themed shell verified to render (dev server) and the Rust side verified to compile.

## Explicit non-goals

- No SQLite/persistence (Phase 1).
- No accounts, transactions, import, budgets, recurring detection, Safe to Spend, or dashboard content (Phases 1-5).
- No Gitea push, no tag, no release (explicit later step; Gitea remote is not configured in Phase 0).
- No Windows installer build (packaging is exercised later once there is a real product surface).

## Verified current state

- `the-forge` inspected read-only; not modified.
- `C:\Dev\midas` did not exist before this phase.
- Toolchain confirmed present: git 2.45.1, node v24.18.0, npm 11.16.0, rustc 1.97.1, cargo 1.97.1. `@tauri-apps/cli` is a devDependency invoked via `npm run tauri` (the Forge starter pattern); no global `cargo-tauri` subcommand is installed or required.
- Gitea reachable at `[private-git-server]` (confirmed via the Forge's own configured remote); MIDAS's own remote is not added in this phase.

## Constraints and architecture boundaries

- Do not modify `the-forge` or `C:\Dev\siren`.
- No Tauri command or plugin permission is exposed until a concrete Phase 1+ workflow needs it.
- No network authority.

## Security and privacy considerations

- No secrets, credentials, or real financial data touch this repository at any point.
- `.gitignore` excludes `*.sqlite*`, `*.db`, and a `/data/` path pre-emptively for when persistence lands in Phase 1.
- Production CSP carried over unchanged from the Forge starter (`default-src 'self'` with no remote origins).

## Acceptance criteria

- [x] `C:\Dev\midas` exists with the scaffold described in Scope.
- [x] `npm install` succeeds and produces a lockfile (72 packages, 0 vulnerabilities).
- [x] `npm run check` (format:check + tsc + node --test + vite build) passes.
- [x] `cargo fmt --check` and `cargo check --locked` pass for `src-tauri` (first `cargo check` compiled the full Tauri 2 dependency tree cleanly in 1m33s; confirmed via `scripts/validate.ps1` -> `MIDAS_VALIDATION=PASS`).
- [x] The themed shell renders in a browser against the Vite dev server, with working System/Light/Dark switching (verified via the Vite dev server: page title "MIDAS", clean console, correct semantic tree, dark applies `--midas-bg-deep` #071A12, light applies #EAE6D9 with gold #9C7A1E brand accent, theme choice persists to localStorage).
- [ ] Local Git repository initialized with an initial commit covering the scaffold.

## Evidence log

- `npm install` — 72 packages added, 0 vulnerabilities.
- `npm run check` — prettier clean, `tsc --noEmit` clean, 1/1 unit test passed, `vite build` produced `dist/` (195 KB JS / 3.5 KB CSS, gzip 61.5 KB / 1.4 KB).
- `cargo fmt --manifest-path src-tauri\Cargo.toml -- --check` — exit 0.
- `cargo check --manifest-path src-tauri\Cargo.toml` (first run, generates `Cargo.lock`) — exit 0, finished in 1m33s.
- `powershell -ExecutionPolicy Bypass -File scripts\validate.ps1` — `MIDAS_VALIDATION=PASS`.
- Browser check against `npm run dev` (localhost:1420): title "MIDAS"; console shows only Vite HMR connect + the standard React DevTools notice, no errors/warnings; accessibility tree shows `banner` (theme radios), `main`/`region` (heading + copy + status list), `contentinfo` (footer) — correct semantic landmarks; clicking Dark sets `documentElement.dataset.theme = "dark"`, persists `midas-theme=dark` to localStorage, and resolves body background to `rgb(7, 26, 18)` (#071A12); clicking Light resolves background to `rgb(234, 230, 217)` (#EAE6D9) and `.brand` color to `rgb(156, 122, 30)` (#9C7A1E) — both match the intended design tokens exactly.

## Test strategy

- `npm run check` for frontend static/unit/build checks (mirrors the Forge starter's own gate).
- `cargo fmt --check` + `cargo check` for the Rust side.
- Manual rendered-UI check of the theme shell via the Vite dev server in a browser (page identity, visible focus, all three theme options, console health).

## Owner decisions

None required for Phase 0 — see the owner gate note above.

## Hard-stop conditions

- Any need for a new framework, cloud/network service, bank sync, or major dependency beyond the Forge starter's own set.
- Any request to push/publish to Gitea (Phase 0 stops at a local, committed repository).
