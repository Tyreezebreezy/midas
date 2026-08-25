# Project state

- Product: MIDAS v0.1 (private, local-first personal finance desktop app)
- Status: PHASE 0 ACCEPTED BY OWNER; PHASE 1 IN PROGRESS
- Active phase: Phase 1 (persistence and accounts)
- Owner gate: Phase 0 APPROVED by owner on 2026-08-25, with corrections (see ADR-0002)
- App identifier: `com.breezy.midas` (corrected from placeholder `com.midas.app` per ADR-0002)
- Canonical intended remote: `[private-git-server]/Tyreezebreezy/midas`
- Configured remote: `origin` -> `[private-git-server]` (added 2026-08-25). **Push blocked**: `[private-git-server]` hangs with no response (confirmed via curl and native PowerShell `Invoke-WebRequest`, independent of DNS/proxy); the same host answers instantly on `http://192.168.1.102/` with a generic reverse-proxy placeholder, not Gitea. Local commits are ahead of the (nonexistent) remote until the owner's Gitea host is reachable again. See ADR-0002.
- Reference golden path: `the-forge\starters\tauri-react-windows` (read-only; not a runtime dependency). Governance conventions (constitution/standards/gates/templates) are App Foundry's, not the Forge's — see ADR-0002.
- Last verified: 2026-08-25 — `scripts/validate.ps1` passes (`MIDAS_VALIDATION=PASS`), now including `cargo audit` (0 vulnerabilities; 17 allowed "unmaintained" warnings, all transitive from Tauri 2's own dependency graph - mostly the Linux GTK backend that never compiles into the Windows binary, see ADR-0002); themed shell manually verified in-browser and as a native launch (System/Light/Dark all resolve correct MIDAS tokens, clean console, correct semantic landmarks, real WebView2 window titled MIDAS).

## Current boundary

Phase 0 is accepted and checkpointed locally. Phase 1 (persistence and accounts) is in progress. Do not begin Phase 2+ product features until Phase 1 is complete and checkpointed. Do not tag, publish a release, or make the repo public without an explicit owner decision. Push to Gitea once the remote is reachable (routine once unblocked, not an approval gate).

## Phase log

- Phase 0 — Inspect and scaffold: COMPLETE (2026-08-25). Forge golden path inspected and reused; MIDAS scaffold, theme, docs, and local Git history in place; full check suite green. Accepted by owner 2026-08-25 with corrections (ADR-0002): Forge/App Foundry terminology, identifier -> `com.breezy.midas`, Gitea remote added (push blocked - infra), `cargo-audit` added to the gate.
- Phase 1 — Persistence and accounts: IN PROGRESS (started 2026-08-25).
