# Dependency standard

1. Confirm the capability is required by the approved MIDAS product/build handoff.
2. Reuse the platform, standard library, or an existing dependency before adding another.
3. Prefer official upstream packages; pin reproducibly and commit lockfiles.
4. Record purpose, source, license, maintenance signal, transitive impact, permissions, and removal path for material dependencies (an ADR in `docs/decisions/`).
5. Review lockfile diffs and vulnerability results before merge or release.

New frameworks, major developer tooling, services, network authority, or dependencies that materially change architecture require owner approval. MIDAS must not depend on The Forge, SERANA, ICARUS, Claude Code, or Codex at runtime.
