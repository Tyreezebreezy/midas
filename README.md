# MIDAS

MIDAS is a private, local-first personal finance desktop application built with Tauri 2, React, TypeScript, and Rust. It is the single place to answer "where do I stand financially?" in about 30 seconds — Safe to Spend, upcoming bills, the current pay cycle, spending, Needs Review, and savings goals.

No cloud account, no bank sync, no telemetry. Local data is the source of truth.

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

Run the desktop app with `npm.cmd run tauri dev`. Build the installer only inside an approved release phase with `npm.cmd run tauri build`. Run the complete gate with `powershell -ExecutionPolicy Bypass -File scripts\validate.ps1`.

## Repository map

- `constitution/`: non-negotiable MIDAS rules (adapted from The Forge).
- `standards/`: security, testing, accessibility, dependencies, and release requirements.
- `templates/`: reusable phase, ADR, handoff, QA, owner-gate, and release records.
- `gates/`: repeatable lifecycle checklists.
- `docs/product/`: the canonical MIDAS product and build handoffs.
- `docs/architecture/`: architecture notes as they are established.
- `docs/decisions/`: ADRs for durable technical decisions.
- `specs/`: phase specs and evidence.
- `src/`, `src-tauri/`: the application.

## Origin

MIDAS reuses `the-forge`'s `tauri-react-windows` golden path as a read-only reference (Tauri 2 + React + TypeScript + Vite, System/Light/Dark theming, narrow production CSP, no exposed Tauri commands by default, NSIS packaging, deterministic checks). MIDAS does not depend on The Forge, SERANA, ICARUS, Claude Code, or Codex at runtime.

No remote is configured yet. The intended private Gitea remote is `[private-git-server]`.
