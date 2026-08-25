# ADR-0001: Reuse The Forge's tauri-react-windows golden path as the MIDAS foundation

- Status: accepted
- Date: 2026-08-25
- Supersedes: none

## Context and problem

The MIDAS build handoff requires using the established Forge golden path rather than inventing a new stack, and requires MIDAS to remain independently buildable/runnable without a runtime dependency on The Forge. `the-forge\starters\tauri-react-windows` is the Forge's single v1 golden path: Tauri 2, React 19, TypeScript, Vite, System/Light/Dark theme plumbing, a narrow production CSP, zero exposed Tauri commands/plugins by default, NSIS packaging, and `node --test` + `cargo fmt`/`cargo check` as the deterministic check set.

## Options considered

1. Copy and adapt the Forge starter's file structure, configuration, and conventions into `C:\Dev\midas` (rebrand only what is product-specific).
2. Generate a fresh Tauri+React scaffold from `create-tauri-app` and re-derive the same conventions independently.
3. Adopt the Forge's full Spec Kit meta-tooling (`.specify/`, `specify` CLI) in addition to the starter.

## Decision

Option 1. Copied `package.json`, `tsconfig*.json`, `vite.config.ts`, `index.html`, `.gitignore`/`.prettierignore`, the theme plumbing (`theme.ts`, `App.tsx`, `App.css`), `tests/theme.test.mjs`, and the `src-tauri` skeleton (`Cargo.toml`, `build.rs`, `tauri.conf.json`, `capabilities/default.json`, `main.rs`, `lib.rs`, icons) from the starter, then rebranded product identity (name, identifier, window title/size, CSS tokens for the MIDAS green/gold palette, copy). Reused `constitution/`, `standards/`, `gates/`, `templates/`, and `scripts/validate.ps1` / `scripts/hash-artifacts.ps1` shapes — App Foundry governance conventions, physically versioned in the Forge repository root alongside its golden-path code — adapted only where MIDAS-specific (financial test requirements, MIDAS remote, MIDAS constitution addenda). Did not adopt Spec Kit/`.specify/` — that tooling manages The Forge's own meta-development process; the MIDAS product and build handoffs already serve as the canonical spec, and the build handoff's own documentation list (README, AGENTS.md, CLAUDE.md, spec/architecture docs, lightweight ADRs) does not call for it.

## Rationale

The build handoff is explicit: "Use the established Forge golden path... Do not create a new development framework for MIDAS" and "reuse its established components/configuration rather than inventing replacements." Option 1 satisfies that directly with the least new surface area. Option 2 would silently re-derive (and likely drift from) decisions the Forge already made deliberately (narrow CSP, no default Tauri authority, NSIS-only bundling, theme mechanism). Option 3 would add a meta-development framework MIDAS's own handoff never asks for, and the Forge's own ADR-0001 (`the-forge/docs/decisions/0001-use-upstream-spec-kit.md`) frames Spec Kit as infrastructure for building The Forge, not a requirement for every downstream app.

## Consequences and accepted tradeoffs

- MIDAS starts with zero exposed Tauri commands/plugins; Phase 1 will need to add narrowly-scoped capabilities (likely `dialog` for file/CSV selection and a scoped `fs` or custom Rust commands for SQLite access) and must document each with its own ADR per `standards/DEPENDENCIES.md` and `standards/SECURITY.md`.
- MIDAS's window sizing and icons are placeholders copied from the starter pattern; real icons/branding are a later, explicit product decision, not a Phase 0 blocker. The identifier was corrected to `com.breezy.midas` and product name/window title to `MIDAS` per owner direction after Phase 0 (see ADR-0002).
- No SQLite, Rust command, or persistence pattern exists yet in either the Forge starter or the sibling SIREN project to copy from (SIREN persists via webview local storage, not SQLite), so Phase 1's persistence layer is new design work, not a reuse of an existing Forge pattern.
