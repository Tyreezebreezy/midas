# ADR-0002: Owner corrections after Phase 0 acceptance

- Status: accepted
- Date: 2026-08-25
- Supersedes: none (amends context established in ADR-0001)

## Context and problem

The owner accepted Phase 0 and, in the same review, corrected one factual error and directed three setup changes before Phase 1: the Forge/App Foundry relationship was mis-stated, the application identifier needed to change, the Gitea remote needed to be established, and `cargo-audit` needed to be added to the security tooling. These are owner decisions and belong in repository files, not only in chat history.

## Decision

1. **Forge vs. App Foundry.** The Forge is the reusable application code/tooling/golden-path. App Foundry is the broader development methodology: standards, governance, gates, and workflow. MIDAS uses both and depends on neither at runtime. The build handoff's line "The Forge is the current name of the former App Foundry" is incorrect and is not MIDAS's understanding going forward. Fixed the small number of places in MIDAS's own authored docs that attributed governance/standards/gates content to "The Forge" by name (`constitution/MIDAS_CONSTITUTION.md`, `README.md`, `AGENTS.md`, `standards/DEPENDENCIES.md`, ADR-0001) to instead attribute it to App Foundry, while keeping "The Forge" for the golden-path starter code specifically. Left the verbatim copy of the original handoff in `docs/product/` untouched and added a correction note above it rather than editing history.
2. **Application identifier.** Changed `src-tauri/tauri.conf.json` `identifier` from the Phase 0 placeholder `com.midas.app` to `com.breezy.midas`. Product name (`MIDAS`), window title (`MIDAS`), and window sizing (1280x800 / min 960x600) are unchanged. The icon remains the Phase 0 placeholder until a later visual-polish/release phase.
3. **Gitea remote.** Added `origin` -> `[private-git-server]` and attempted `git push -u origin main` using the existing cached Windows Credential Manager credential (`git:[private-git-server]`) that already authenticates the-forge's own remote — no credential was invented, printed, or committed. The push could not complete: requests to `[private-git-server]` hang indefinitely (confirmed independent of git/curl/tooling via a native PowerShell `Invoke-WebRequest`, and independent of DNS since `[private-git-server]` resolves correctly and the same host responds instantly on `http://192.168.1.102/` with a generic reverse-proxy "Default Site" placeholder, not Gitea). This is a server-side vhost/routing or backend-availability issue on the owner's home infrastructure, not a credentials or permissions problem, and not something to route around by guessing at the raw IP against a service that isn't confirmed to be Gitea. `origin` is left configured correctly; the push is deferred until the owner confirms the Gitea host is reachable again.
4. **cargo-audit.** Installed via `cargo install cargo-audit --locked` (normal Rust tooling, no new project dependency — this is a dev-tool, not a `Cargo.toml` entry). Wired into `scripts/validate.ps1` alongside the existing Gitleaks/Trivy usage, which are unchanged.

## Rationale

Each change is either a direct owner instruction (identifier, Gitea, cargo-audit) or a factual correction the owner supplied (Forge/App Foundry). None expands scope beyond what was asked. The Gitea push was attempted rather than skipped, using only the existing, already-proven local credential mechanism — never an invented or requested credential — and stopped at a concretely diagnosed infrastructure blocker rather than a workaround that could push MIDAS's history to the wrong service.

## Consequences and accepted tradeoffs

- `origin` is configured but Phase 0 + Phase 1 commits are not yet pushed to Gitea. Local history is the only copy until the owner's Gitea host is reachable again and a push is retried.
- The Forge/App Foundry correction touched five files with one or two lines each; the verbatim handoff copy was annotated, not rewritten, to preserve an honest historical record.
