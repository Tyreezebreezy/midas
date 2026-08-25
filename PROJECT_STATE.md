# Project state

- Product: MIDAS v0.1 (private, local-first personal finance desktop app)
- Status: PHASE 1 COMPLETE
- Active phase: `specs/phase-001-persistence-accounts/spec.md` (complete); next is Phase 2 (transactions and CSV pipeline)
- Owner gate: Phase 0 APPROVED by owner on 2026-08-25, with corrections (see ADR-0002). Phase 1 pre-authorized by owner in the same message ("Proceed to Phase 1... Continue autonomously through Phase 1").
- App identifier: `com.breezy.midas` (corrected from placeholder `com.midas.app` per ADR-0002)
- Canonical intended remote: `[private-git-server]/Tyreezebreezy/midas`
- Configured remote: `origin` -> `[private-git-server]` (added 2026-08-25). **Push blocked**: `[private-git-server]` hangs with no response (confirmed via curl and native PowerShell `Invoke-WebRequest`, independent of DNS/proxy); the same host answers instantly on `http://192.168.1.102/` with a generic reverse-proxy placeholder, not Gitea. Local commits are ahead of the (nonexistent) remote until the owner's Gitea host is reachable again. See ADR-0002.
- Reference golden path: `the-forge\starters\tauri-react-windows` (read-only; not a runtime dependency). Governance conventions (constitution/standards/gates/templates) are App Foundry's, not the Forge's — see ADR-0002.
- Last verified: 2026-08-25 — `scripts/validate.ps1` passes (`MIDAS_VALIDATION=PASS`): frontend check, `cargo fmt`/`check`/`clippy -D warnings`/`test` (14/14 passing)/`audit` (0 vulnerabilities, 17 allowed transitive "unmaintained" warnings, see ADR-0002/ADR-0003) all green. Account persistence and UI verified against the real running app (not mocked) over the WebView2 CDP debug port: direct IPC calls for create/list/update/archive/unarchive/settings/validation, plus a real button-click-fill-submit pass through the actual dialog - see `specs/phase-001-persistence-accounts/spec.md`.

## Current boundary

Phase 0 and Phase 1 are complete and checkpointed locally. Do not begin Phase 2+ product features (transactions, CSV import) without re-checking this file first. Do not tag, publish a release, or make the repo public without an explicit owner decision. Push to Gitea once the remote is reachable (routine once unblocked, not an approval gate).

## Phase log

- Phase 0 — Inspect and scaffold: COMPLETE (2026-08-25). Forge golden path inspected and reused; MIDAS scaffold, theme, docs, and local Git history in place; full check suite green. Accepted by owner 2026-08-25 with corrections (ADR-0002): Forge/App Foundry terminology, identifier -> `com.breezy.midas`, Gitea remote added (push blocked - infra), `cargo-audit` added to the gate.
- Phase 1 — Persistence and accounts: COMPLETE (2026-08-25). SQLite via rusqlite (ADR-0003), accounts + settings tables, 6 Tauri commands, Account UI (list/create/edit/archive) in the MIDAS design system. 14 Rust tests + real end-to-end IPC/UI verification. Ponytail-reviewed (3 findings applied: over-broad `pub`, unused frontend settings wrapper, premature index).
