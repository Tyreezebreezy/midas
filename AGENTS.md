# Worker entry point

1. Read `PROJECT_STATE.md` and the active spec it names.
2. Read `docs/product/MIDAS_PRODUCT_CONTEXT_HANDOFF.md` and `docs/product/MIDAS_CLAUDE_CODE_BUILD_HANDOFF.md` — the canonical product and build spec.
3. Read `constitution/MIDAS_CONSTITUTION.md`.
4. Follow `standards/SECURITY.md`, `standards/TESTING.md`, `standards/ACCESSIBILITY.md`, `standards/DEPENDENCIES.md`, and `standards/RELEASE.md` as applicable.
5. Check `docs/decisions/` before changing architecture and record only durable decisions as ADRs.
6. Work only inside the approved phase and its owner gate. Stop before new frameworks, major dependencies, services, broad permissions, destructive changes, failed security exceptions, publication, or release acceptance.
7. Validate with `powershell -ExecutionPolicy Bypass -File scripts/validate.ps1` and record evidence in the active phase.

MIDAS reuses The Forge's golden path (`the-forge\starters\tauri-react-windows`) as a read-only reference for code/tooling, and App Foundry's governance conventions (steps 3-4 above) for process discipline. MIDAS does not depend on either at runtime. Claude and Codex files are adapters. Repository files above are canonical.
