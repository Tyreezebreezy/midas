# MIDAS Constitution v1

Adapted from App Foundry governance conventions, versioned alongside the Forge golden-path code at `the-forge\constitution\FORGE_CONSTITUTION.md`. The Forge is the reusable application code/tooling/golden-path; App Foundry is the broader development methodology, standards, governance, and gates. MIDAS uses both and does not depend on either at runtime; these principles are reused as process discipline, not as a code dependency.

## Non-negotiable principles

1. **State first.** Approved scope, decisions, phase status, architecture, security requirements, tests, and release state live in repository files, not private chat history.
2. **Spec before implementation.** Major work has a goal, scope, non-goals, constraints, acceptance criteria, security considerations, test strategy, owner decisions, and hard stops before code changes.
3. **Agent independence.** Agent files are thin adapters. `docs/product/` holds the canonical MIDAS product and build handoffs; no critical rule may exist only in private chat history or proprietary memory.
4. **Owner-controlled architecture.** New frameworks, major dependencies, services, broad permissions, destructive changes, security exceptions, publication, and release acceptance require explicit owner approval.
5. **Tool quarantine.** Use the minimum justified dependency and permission set. Prefer upstream capabilities over private forks.
6. **Local first and least privilege.** Add network, filesystem, native-command, secret, or OS authority only when an approved product requirement needs it. MIDAS has no ordinary reason to reach the network.
7. **Evidence over assumption.** Test materially uncertain behavior — especially money math — and preserve repeatable evidence.
8. **Converge to the spec.** Identify missing, partial, contradictory, and unrequested behavior before release.
9. **Simplify after success.** Remove unnecessary abstraction, duplication, dependencies, and speculative layers without weakening safety, accessibility, validation, error handling, tests, or user control.
10. **Release deliberately.** A release requires deterministic regression, security/privacy, packaging, artifact, documentation, and owner gates, then stops.
11. **MIDAS remains independent.** MIDAS may reuse Forge patterns but never requires The Forge, Claude Code, Codex, or any agent runtime to function.
12. **Financial correctness outranks convenience.** Credit availability is never cash. Transfers and card payments are never double-counted as spending. Uncertain data goes to Needs Review instead of being silently guessed.

## Governance

This constitution outranks local convenience. Changes require an ADR, migration impact, and explicit owner approval. The MIDAS product and build handoffs in `docs/product/` are the canonical product spec this constitution serves.

**Version**: 1.0.0 | **Ratified**: 2026-08-25
