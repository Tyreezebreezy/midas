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

- `constitution/`: non-negotiable MIDAS rules (adapted from App Foundry governance conventions).
- `standards/`: security, testing, accessibility, dependencies, and release requirements.
- `templates/`: reusable phase, ADR, handoff, QA, owner-gate, and release records.
- `gates/`: repeatable lifecycle checklists.
- `docs/product/`: the canonical MIDAS product and build handoffs.
- `docs/architecture/`: architecture notes as they are established.
- `docs/decisions/`: ADRs for durable technical decisions.
- `specs/`: phase specs and evidence.
- `src/`, `src-tauri/`: the application.

## Origin

MIDAS reuses `the-forge`'s `tauri-react-windows` golden path as a read-only reference (Tauri 2 + React + TypeScript + Vite, System/Light/Dark theming, narrow production CSP, no exposed Tauri commands by default, NSIS packaging, deterministic checks). MIDAS also follows the App Foundry development methodology (the standards/gates/governance discipline behind `constitution/`, `standards/`, and `gates/`) without depending on it at runtime either. The Forge is the code/tooling side of this; App Foundry is the methodology side — MIDAS does not depend on The Forge, App Foundry, SERANA, ICARUS, Claude Code, or Codex at runtime.

`origin` is configured to the private Gitea remote at `[private-git-server]`. As of Phase 1 start, pushing is blocked by an infrastructure issue on that host (see `PROJECT_STATE.md` and ADR-0002) — local commits are ahead of the remote until it's reachable again.
